---
title: "ce-executor-isolated preset dispatch gap between plan-gate and executor causes loop termination"
date: 2026-06-12
category: integration-issues
module: crates/ralph-core/src/event_loop
problem_type: integration_issue
component: development_workflow
symptoms:
  - "plan-gate emits queue.advance but executor has no legal business topic to dispatch next step"
  - "loop runner stalls 10 minutes between queue.advance emit and any hat activation"
  - "ralph hat injects task.resume fallback which is rejected by executor triggers"
  - "executor activation attempts to emit queue.advance and gets rejected by EventOriginGuard"
  - "loop terminates with loop.cancel after 74 minutes with U2-U7 plan steps uncompleted"
root_cause: missing_workflow_step
resolution_type: config_change
severity: critical
tags:
  - ce-executor-isolated
  - preset-design
  - dispatch-gap
  - plan-gate
  - executor
  - queue-advance
  - isolated-scope
  - loop-cancel
  - event-origin-guard
  - ralph-hat
  - p0
---

# ce-executor-isolated preset dispatch gap between plan-gate and executor causes loop termination

## Problem

`ce-executor-isolated` 10-hat preset has no bridge signal between `plan-gate` (which publishes `queue.advance` to advance the plan) and `executor` (whose `publishes=[work.done, work.failed]` excludes any business topic to start the next step). After U1 completes and `queue.advance (next_step=step-02)` is emitted, the loop stalls 10 minutes because no hat has a legal emit path to advance the workflow. ralph hat's `RALPH_CONTROL_TOPICS` whitelist (7 control topics only) blocks it from emitting the missing bridge, leaving `loop.cancel` as the only safe exit. Plan terminated at 74m runtime with 6/8 steps uncompleted.

## Symptoms

- **10-minute dispatch gap (event #21 → #22)**: `queue.advance` emitted at 4:44:50 but executor is not dispatched; `active-activations.json` shows `review-coordinator` iter=2 stale for 3005s. The hat selection round-robin cursor (U4) is not advancing to executor because executor.pending is empty.
- **Executor emit rejected by origin guard (event #23)**: At 5:00:20, `task.resume` with `triggered=executor` + `original_trigger_topic=queue.advance` confirms that executor was eventually started, attempted to emit `queue.advance` to advance itself, and was rejected by `EventOriginGuard` because `executor.publishes` does not include `queue.advance`.
- **3 null-payload `review.passed` fallbacks (events #17/#18/#19)**: Bypass the schema-required `payload: json_object` and `required_fields=[plan_name, task_id, task_key, step, findings_count, fix_round, verdict, skip_reason]`. `synth_terminal` state in `review_step_state.rs:234-240` is only set on full-payload events, so the gate at `review_step_state.rs:135-148` rejects subsequent `queue.advance` for 5 minutes.
- **Unintended `loop.cancel` (event #24)**: 5:12:40 DEC-005 (confidence 65) terminates the plan. Final state is `Status: Cancelled gracefully` (per `summary.md`); U2-U7 + U4.5 remain `open` in `tasks.jsonl`; U1 commit `b11d9f0` is preserved.

## What Didn't Work

- **3× null-payload `review.passed` fallback by ralph hat (4:38–4:41)**: events #17/#18/#19 were injected by `triggered=ralph` (origin guard lets ralph inject `review.passed` because it is a workflow control surface for stall recovery). But `review_step_state.rs:234-240` only sets `synth_terminal` on full-payload `review.passed`, so the gate at lines 135-148 keeps blocking `queue.advance`. After the 4th full-payload `review.passed` (event #20 at 4:42:04), the gate finally opens.
- **ralph hat emitting `work.ready` to bridge step-02**: blocked by `RALPH_CONTROL_TOPICS` (`event_origin.rs:32-45` = 7 control topics: `LOOP_COMPLETE`, `loop.cancel`, `loop.start`, `human.interact`, `human.response`, `human.guidance`, `task.resume`). The `is_ralph_control` check at `event_origin.rs:272` rejects any business-topic emit by ralph, so ralph cannot synthesize a bridge.
- **3× `task.resume` injection (#22, #23) by origin guard / stall_recovery**: `retry_key=ralph:executor:queue.advance:not_started` and `re_dispatch` did not start executor because `task.resume` is in `RALPH_CONTROL_TOPICS` but not in `executor.triggers`. The signal lands in ralph.pending (the fallback hat), not executor.pending.
- **Hard escalation path with confidence 65**: per scratchpad 05:08 UTC, ralph hat considered letting `stall_recovery` escalate to `hard_escalation` (3 stalls → safe_target=review-coordinator). The path was uncertain (commit history shows no recent changes to hard escalation in plan 06-12-001), so ralph hat chose `loop.cancel` (DEC-005) over retry. The confidence protocol table 50-80 calls for "proceed + document"; ralph hat's only legal exit per `RALPH_CONTROL_TOPICS` is `loop.cancel`.

## Solution

### Path A (recommended): preset-level fix — plan-gate double-publishes `work.ready`

Modify `presets/en/ce-executor-isolated.yml` line 1398 to add `work.ready` to `plan-gate.publishes`, with a HARD RULE in plan-gate instructions to emit `work.ready` ONLY after `queue.advance`:

```yaml
# presets/en/ce-executor-isolated.yml:1395-1466
plan-gate:
  triggers: ["review.passed", "review.complete", "work.failed", "loop.cancel"]
  publishes: ["queue.advance", "work.ready", "plan.complete", "plan.blocked"]
  #                       ^^^^^^^^^^^ NEW: bridge to executor for next step
  instructions: |
    ...
    ### Step-Advance Bridge (U8 — 2026-06-12 fix)
    HARD RULE: after emitting `queue.advance (next_step=X)`, you MUST
    immediately emit `work.ready` for the same next step, mirroring the
    coordinator's work.ready shape. Without this, the executor hat has no
    legal business topic to start X (executor.publishes = [work.done,
    work.failed]) and the loop stalls.

    work.ready payload contract (mirror coordinator at presets/...yml:338-346):
      - plan_name (string, equal to current plan)
      - next_step (string, e.g. "step-02")
      - task_id (string, the next-step runtime task id)
      - task_key (string, e.g. "ce-executor:2026-06-10-003-...:step-02:...")
      - step (string, same as next_step)
      - complexity (string, "small" | "medium" | "large")
      - preflight_checks (array of strings, the next-step scope guard)
```

This restores the `coordinator → work.ready → executor` chain that worked for step-01, generalized for every step transition.

### Path B (secondary): loop-runner preemption in `EventBus::select_next_hat_with_pending`

```rust
// crates/ralph-proto/src/event_bus.rs (select_next_hat_with_pending)
const HIGH_PRIORITY_TOPICS: &[&str] = &[
    "queue.advance", "work.ready", "work.failed", "plan.blocked",
];

fn select_next_hat_with_pending(&self) -> Option<HatId> {
    // Preempt round-robin (U4) for high-priority workflow triggers
    if let Some(event) = self.peek_pending_with_topic(HIGH_PRIORITY_TOPICS) {
        if let Some(target) = self.hat_for_topic_with_priority(event.topic) {
            return Some(target);
        }
    }
    // Fall back to U4 round-robin cursor
    self.select_next_hat_round_robin()
}
```

Trade-off: violates U4 fair scheduling. Mitigated by `queue.advance` being a true step boundary, not a routine event — high-priority preemption is justified for step transitions.

### Anti-pattern (do NOT do)

Do not extend `RALPH_CONTROL_TOPICS` to include `work.ready` / `queue.advance`. Per `event_origin.rs:32-45`, the 7-topic whitelist exists to enforce U3 isolated terminal authority — ralph hat is fallback, not a workflow hat. Adding business topics breaks the fail-closed security model and would let a single hat mask as both workflow and fallback simultaneously.

### Companion fix: strict payload policy

```yaml
# event_policy
event_policy:
  null_payload_reject_topics:
    - "review.passed"
    - "review.failed"
    - "review.complete"
    - "work.done"
    - "queue.advance"
```

Refines `event_policy` to reject null-payload `review.passed`, forcing synthesizer to emit `plan.blocked` instead of stalling. This closes the P0-3 gap from the report's §4.

## Why This Works

The root cause spans four cascading layers:

1. **Bridge signal missing**: `plan-gate.publishes = [queue.advance, plan.complete, plan.blocked]` (line 1398) emits `queue.advance`, but `executor.publishes = [work.done, work.failed]` (line 360) means executor has no legal business topic to emit upon receiving `queue.advance`. The original `coordinator → work.ready → executor` chain (which worked for step-01) is not replicated between plan-gate and executor.
2. **Asymmetric triggers/publishes**: `executor.triggers = [work.ready, queue.advance, work.retry, fix.plan.ready]` (line 359) means executor listens for `queue.advance` but cannot legally emit anything except `work.done`/`work.failed` in response — a "dead-end trigger" per the cross-reference principle in Prevention.
3. **ralph hat bound by control topics**: `event_origin.rs:32-45` defines `RALPH_CONTROL_TOPICS = [LOOP_COMPLETE, loop.cancel, loop.start, human.interact, human.response, human.guidance, task.resume]`. The `is_ralph_control` check at `event_origin.rs:272` rejects any ralph hat emit on business topics, so ralph cannot bridge `queue.advance → work.ready`. The only ralph emit that helps the workflow stop is `loop.cancel`.
4. **Synthesizer null-payload stall**: `review_step_state.rs:234-240` only sets `synth_terminal` when `review.passed` has a non-null payload. The 3 null-payload fallbacks (#17-19) bypass the gate at `review_step_state.rs:135-148`, requiring 9 minutes of stall_recovery before the 4th full-payload `review.passed` (event #20) unblocks `queue.advance`.

Path A (preset double-publish) works because it adds `work.ready` to `plan-gate.publishes` so the existing `coordinator → work.ready → executor` chain is restored for every step transition. Path B (EventBus preemption) works as a safety net for any preset that has a similar dead-end trigger pattern. The anti-pattern is explicitly excluded to preserve U3 isolated terminal authority.

## Prevention

- **Cross-reference lint rule in `preset_lint`**: warn (severity: high) if any hat's `triggers` contains a topic that no other hat's `publishes` includes — i.e., a "dead-end trigger" (e.g., `executor.triggers=queue.advance` with no hat publishing it as a transition signal). Implementation hint: build a `triggers_referenced_by_publishes: HashMap<Topic, Vec<HatId>>` and assert every entry in any hat's `triggers` is reachable as a downstream signal. If a dead-end trigger is found, suggest adding the trigger topic to the upstream hat's `publishes` (e.g., add `queue.advance` to plan-gate's `publishes`, or — better — add `work.ready` so executor has a legal emit path).
- **`EventBus` unit test**: for each `(trigger_topic, target_hat)` pair in any preset, verify `target_hat.publishes` contains at least one topic whose string is in some other hat's `triggers`. Test name: `select_next_hat_with_pending_avoids_dead_end_triggers`. Reference the `presets/en/ce-executor-isolated.yml:357-481` (executor) and `1395-1466` (plan-gate) blocks as the canonical test fixture.
- **Integration test**: `ralph run -c fixtures/ce-executor-isolated-step-bridge.yml -p "two-step plan"` — emit `queue.advance` and assert executor is dispatched within 5 seconds. Assertion: `events.jsonl` contains a `work.done` event from `executor` for `next_step=step-02` within 5000ms of `queue.advance`. If the assertion fails, the test surfaces a P0 dispatch gap.
- **Strict payload enforcement**: extend `event_policy` to support `null_payload_reject_topics: [review.passed, review.failed, review.complete, work.done, queue.advance]`. Update `crates/ralph-core/src/event_policy.rs` to add this mode and ship a test fixture. This is the P0-3 companion fix.
- **Coordinator instructions for `decisions.md`**: in `presets/en/ce-executor-isolated.yml:279` coordinator step "Create `decisions.md` — empty", change to "Create `decisions.md` IMMEDIATELY on plan start (do not rely on lazy creation)". Pair with a loop-runner assert: if `loop.cancel` is emitted, `decisions.md` must exist with the matching `DEC-NNN` entry. The current loop emitted `loop.cancel` with DEC-005 reference but `decisions.md` was never created.
- **Drift monitor field completeness**: the loop's `drift.jsonl` already caught 8 field_completeness findings on `review.passed` (`plan_name`, `task_id`, `task_key`, `step`, `findings_count`, `fix_round`, `verdict`, `skip_reason` all 0% present) — but the loop continued. Hook the drift monitor to `hard_escalation` after N consecutive critical findings (suggest N=3) to avoid silent field drift.

## Related

### Same root cause class (task/hat boundary, preset design)

- **ce-executor task ownership** — coordinator-created tasks unstartable by executor due to missing `coordinator_hats` config. Same pattern: a task exists but no hat can legally start it. The HARD RULE pattern in plan-gate Path A above mirrors the prescription in that solution. Search for: `coordinator_hats` in preset configs.
- **Payload contract preset baseline** — 8 builtin presets have 0/0 strict-validate baseline; the schema-vs-reality drift enables null-payload `review.passed` to slip through. Path A's `null_payload_reject_topics` complements the strict-validate migration.
- **Agent kill self parent ralph** — ralph hat capability boundary. Reinforces why Path A (preset fix) and NOT "extend ralph hat" is the right direction. The agent self-kill was the inverse problem (ralph hat accidentally terminating itself); this is ralph hat bound by design.

### Other relevant reports

- **Primary diagnostic report** (this learning's source): `docs/report/2026-06-12-ce-executor-isolated-dispatch-gap-diagnosis.md` — §4 P0-1/P0-2/P0-3 rows, §5 8-step causal chain, §6 fix proposals.
- **Inverse problem (ralph hat impersonation)**: `docs/report/2026-06-10-ce-executor-ralph-hat-impersonation-diagnosis.md` — ralph hat attempting business topic emit, rejected. Same hat-boundary problem viewed from the mechanism layer (origin guard); this learning is the topology layer (preset topology).
- **Multi-run parallel loop case**: `docs/report/2026-06-12-ce-executor-isolated-multi-run-diagnosis.md` — same preset in multi-run mode, no dispatch gap observed. Suggests the gap is triggered by specific hat-activation timing, not inherent to isolated mode.
- **Predecessor non-blocking anomalies**: `docs/report/2026-06-11-ce-executor-isolated-nonblocking-anomalies-corrected-diagnosis.md` — earlier anomalies (payload schema, topic_deny_rules) were corrected; P0-3 (null-payload `review.passed`) still present and is the P0-3 fix in Path A's companion.
- **Mechanism vs orchestration taxonomy**: `docs/report/2026-06-09-ce-executor-mechanism-vs-orchestration-diagnosis.md` — distinguishes mechanism problems (event_loop/* runtime) from orchestration problems (preset/* design). This learning is an orchestration problem; P1-2 (counter bug in `diagnosis-summary.json`) and P2-3 (stale `active-activations.json`) are mechanism problems in the same loop.
- **Loop premature termination (coordinator mode)**: `docs/report/2026-06-02-ce-executor-loop-premature-termination-diagnosis.md` — the coordinator→executor bridge gap manifested in the older coordinator-mode preset. This learning shows the same class of gap recurring in isolated mode at the plan-gate→executor boundary.

### Existing solutions to read first

- `docs/solutions/developer-experience/ce-executor-plan-gate-premature-completion-2026-06-02.md` — pre-isolated preset solution on the same plan-gate concept; the gap is now reproduced in isolated mode with new symptoms.
- `docs/solutions/integration-issues/ce-executor-wave-emission-must-batch-in-single-emit-2026-06-09.md` — adjacent integration issue (wave emit batch), not the same problem.

### Auto memory context (supplementary)

The following auto-memory entries informed the cross-references above (tagged per the ce-compound convention):

- (auto memory [claude]) ce-executor task ownership: coordinator creates task → executor can't start (preset lacks `coordinator_hats`). Same task/hat boundary class.
- (auto memory [claude]) payload contract preset baseline: 8 builtin strict validate 0/0. Same schema/reality drift class.
- (auto memory [claude]) agent kill self parent ralph: ralph hat self-terminated 2026-06-05 13:46. Same ralph hat capability boundary class.
