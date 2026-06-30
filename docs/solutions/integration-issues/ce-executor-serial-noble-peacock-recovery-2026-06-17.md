> ⚠️ **SUPERSEDED 2026-06-28 by plan `docs/plans/2026-06-28-005-refactor-remove-human-guidance-topic-plan.md`**:
> The `human.guidance` topic was physically deleted from the runtime
> (no external operator channel exists in this build of
> ralph-orchestrator). The recovery patterns described in this
> document — injecting a `human.guidance` event so the operator
> can intervene between iterations, and tracking the operator's
> response on the loop's pending queue — are no longer applicable.
>
> Plan 2026-06-28-005 redirects the 3-strike escalation output to
> `plan.blocked(reason=correction_3_strike_exhausted)` so the
> shipper / reporter chain runs the preset's failure path; the
> drift Warning + Final hint is routed to
> `TerminationReason::RecoveryExhausted` directly without an
> intermediate guidance publish. Historical diagnoses
> (`merry-lotus`, `noble-peacock`, etc.) are preserved below for
> context, but the recovery mechanisms they describe have been
> replaced.

---
title: "ce-executor-serial noble-peacock review chain deadlock: three-mechanism gap, fail-closed provenance, and HatActivationClock"
date: 2026-06-17
category: integration-issues
module: crates/ralph-cli + crates/ralph-core
problem_type: integration_issue
component: development_workflow
symptoms:
  - "noble-peacock run (2026-06-17, worktree 2026-06-10-003-noble-peacock) terminates at iter 5 with ralph hat emitting loop.cancel after dimension-reviewer produces 0 events on review.dimension.ready(correctness)"
  - "executor emits 26 out-of-scope probes (debug.step, build.done, review.passed with skip_reason=aggregate_timeout); 24 rejected by CLI, but 2 review.passed events leak into events.jsonl because hat=None bypasses check_isolated_scope and hat_allowed_values"
  - "missing_event_gate fires ~49s after review.dimension.ready (one iteration), well before dimension-reviewer's adapter idle timeout, killing the hat while it is still legitimately working"
  - "task.resume payload after missing_event_gate has no top-level target, no stage, and no replay of the original review.dimension.ready trigger, so dimension-reviewer cannot re-emit review.dimension.done with the correct dimension context"
  - "diagnosis-summary.json reports recovery_count=0 despite 26 cli_emit rejects in .ralph/recovery.jsonl, blinding the operator to the actual recovery activity"
  - "plan 003 frontmatter status: stalled-after-U1 is still active even though U1 commit has landed — the drift is only visible to humans reading both files"
root_cause: missing_workflow_step
resolution_type: code_fix
severity: high
tags:
  - ce-executor-serial
  - noble-peacock
  - isolated-scope
  - precheck
  - provenance
  - missing_event_gate
  - hat_activation_clock
  - recovery
  - task.resume
  - plan-sync
  - diagnosis
  - fail-closed
---

# ce-executor-serial noble-peacock review chain deadlock: three-mechanism gap

## Problem

The noble-peacock run (2026-06-17, 28m 45s, 6 iterations, 7 events) terminated
at iter 5 with `loop.cancel` after `dimension-reviewer` produced zero events on
`review.dimension.ready(correctness)`. The plan
(`docs/achieved/plan/2026-06-17-004-fix-ce-executor-serial-noble-peacock-review-chain-plan.md`)
and the diagnostic report
(`docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md`)
trace the failure to **three independent mechanism gaps that compounded**.

The previous fix
(`docs/achieved/plan/2026-06-17-003-fix-ce-executor-serial-precheck-recovery-gates-plan.md`,
U1–U5 from the merry-lotus run) landed the isolated-scope precheck, the
`task.resume` schema-compliance wrapper, and the
`human.guidance → task.resume` redirect for automated recovery. noble-peacock
proved those fixes were **necessary but not sufficient**: the failure is not
recoverable from a single line of defense.

### Three gap layers

1. **CLI write-gate has a `hat=None` bypass.** `check_isolated_scope` in
   `crates/ralph-cli/src/policy_check.rs` returns `Ok(())` when `hat` is `None`
   (`policy_check.rs:115-117`). `validate_event_with_hat` similarly skips
   `hat_allowed_values` rules when no hat is provided. When a confused agent
   emits `review.passed(plan_name="p", skip_reason="aggregate_timeout")` without
   setting `RALPH_CURRENT_HAT` or passing `--hat`, **2 of 26** such probes
   land in `events.jsonl` (noble-peacock events JSONL lines 3-4). The remaining
   24 are caught by runtime origin guard, but the agent receives no actionable
   backpressure — it keeps emitting the same shape.
2. **`missing_event_gate` uses the iteration clock, not the hat-activation
   clock.** The gate fires at the end of every iteration
   (`crates/ralph-cli/src/loop_runner/hard_gate.rs`), ~49s after
   `review.dimension.ready(correctness)` for dimension-reviewer. The agent's
   adapter idle timeout is 300–900s. The gate kills the hat while it is still
   legitimately working. `hat.timeout: 1800` exists in the preset YAML but the
   main loop runner does not read it — there is no clock alignment between
   gate timing and adapter/hat timing.
3. **Recovery injection loses trigger context.** When
   `missing_event_gate` injects `task.resume` it carries `reason`+`target_hat`
   (from plan 003 U2) but no **top-level `target`** for routing, no `stage`
   field for drift diagnosis, and no replay of the original
   `review.dimension.ready` payload into
   `last_activation_events`. dimension-reviewer's trigger list does not include
   `task.resume`, so even if the routing worked, the hat would not re-activate
   on it. The hat is pinned via `pending_recovery_hat`, but it has no
   obligation context to fulfill.

The compounding effect: the gate kills the hat before grace; even if grace is
added, the recovery hint carries no obligation context; even if the hint
carried the context, there is no routing target. **All three layers must be
fixed for review-chain recovery to work**.

## Symptoms (Pre-Fix)

- noble-peacock `events-20260617-095504.jsonl` lines 3-4 contain
  `review.passed` from `executor` with `skip_reason="aggregate_timeout"`,
  `plan_name="p"`, `task_id="t"`. These bypass the `ce-executor-serial`
  precheck because the agent did not set `RALPH_CURRENT_HAT`.
- `task.resume` JSONL lines (hard_gate injections) have payload shape:
  `{"reason": "missing_event", "target_hat": "dimension-reviewer", "hint": "..."}`
  but no `stage`, no `target` at the event level, and no
  `original_trigger_topic`/`original_trigger_payload`.
- 49s after `review.dimension.ready(correctness)`, `missing_event_gate` fires
  and writes the `task.resume` event. dimension-reviewer is re-pinned (via
  `pending_recovery_hat`) but does not re-emit `review.dimension.done`. After
  3 attempts, `ralph` hat emits `loop.cancel`.
- `diagnosis-summary.json` has `recovery_count: 0` despite 26 cli_emit
  envelopes already in `.ralph/recovery.jsonl` (hard-coded literal in
  `crates/ralph-cli/src/loop_runner/runner.rs`).
- plan 003 frontmatter still reads `status: active` even though U1
  (`check_isolated_scope`) has been committed. The drift is not detectable
  by tooling; only a human comparing frontmatter to `tasks.jsonl` can spot
  it.

## Why the Merry-Lotus Fixes Were Necessary but Not Sufficient

Plan 003 U1-U5
(`docs/achieved/plan/2026-06-17-003-fix-ce-executor-serial-precheck-recovery-gates-plan.md`)
closed three merry-lotus gaps:

| Merry-lotus gap | plan 003 unit | What it fixed |
|---|---|---|
| CLI allowed out-of-scope emit (executor → debug.step × 8) | U1: `check_isolated_scope` | `can_publish` at CLI boundary, but **only when hat is provided** |
| Orchestrator `task.resume` had no `reason`/`target_hat` | U2: `build_task_resume_payload` | Schema-compliant structured payload |
| Automated recovery used `human.guidance` | U3: `inject_hard_gate_guidance` switch | `task.resume` instead of free-form text |

| noble-peacock gap | Why plan 003 didn't catch it |
|---|---|
| 2 of 26 executor probes bypass precheck | U1's `check_isolated_scope` returns `Ok(())` when `hat=None`; the merry-lotus agent always set `RALPH_CURRENT_HAT`, the noble-peacock agent did not |
| `missing_event_gate` kills at 49s | U3 changed the topic, not the clock; merry-lotus had ~13s of repeated ready events before failure, the gate was downstream of that loop |
| `task.resume` reaches dimension-reviewer but DR doesn't re-emit | U2 added `reason`+`target_hat`; merry-lotus didn't need trigger replay because the hat that needed recovery (executor) was already in its own obligation cycle |

The pattern: **merry-lotus was a topic-routing failure**, **noble-peacock is a
clock + context-replay failure**. Plan 003 closed the routing half. plan 004
closes the clock + context half, plus tightens the `hat=None` path that
merry-lotus didn't exercise.

## Solution: Six Implementation Units

The plan 004 fix is split into U1–U6; U1–U3 land in commit `c4d1811` (the
shipped code) and U4–U6 are follow-ups.

### U1 — CLI provenance fail-closed (R1, R2)

The `hat=None` bypass is closed by introducing
`check_emit_provenance(hat, topic, config)`:

- In isolated mode, when topic is a business topic (not in
  `RALPH_CONTROL_TOPICS`, not an orchestrator internal) and `hat=None`,
  return `ValidationError { reason_code: "missing_provenance" }`.
- `validate_event_with_hat` in the `hat=None` branch is required to use the
  most-strict `hat_allowed_values` interpretation (or skip only if
  provenance is present).
- preset `event_policy.require_emit_provenance: true` is set explicitly in
  `presets/en/ce-executor-serial.yml`. isolated mode defaults to
  "require provenance" for business topics (KTD-2: reuse existing config
  field, no new flag).

The fail-closed approach is preferred over a soft warning (KTD-1) because
noble-peacock's leak was structural: the agent had no signal at all. A
warning would not have been read by the prompt that was confused enough to
emit `aggregate_timeout` as a hat that doesn't publish `review.passed`.

### U2 — `HatActivationClock` for `missing_event_gate` defer (R3)

A new per-hat clock `hat_activation_started_at: HashMap<String, DateTime<Utc>>`
lives in `LoopState`. The gate is skipped if
`now - activation_started_at[hat] < grace_secs`.

- Default grace = `min(adapter_idle_timeout_secs * 0.3, 540)`. The 0.3
  multiplier aligns with the diagnostic report recommendation; the 540s
  floor prevents very short adapter configs from triggering.
- Per-hat override `hat.missing_event_grace_secs` (preset-level); serial
  preset sets `dimension-reviewer: 540` to match the recommended
  alignment.
- KTD-3: **new clock**, not `hat.timeout`. The main loop runner does not
  read `hat.timeout`; the adapter timeout exists but is at a different
  granularity. Wiring `hat.timeout` into the runner is a follow-up.

### U3 — Recovery routing with target + trigger context replay (R4, R5)

Double-track fix (KTD-5):

1. `Event::with_target(event.hat)` on the injected `task.resume` — top-level
   `target` field for routing, consistent with R5's
   `publish_policy_rejection_resume` pattern.
2. `replay_obligation_triggers_to_activation_state` snapshots the original
   `review.dimension.ready` into `LoopState.pending_obligation_triggers`
   when the hat is activated, and on gate injection replays the trigger into
   `last_activation_events`. dimension-reviewer can then re-derive its
   obligation context (which dimension to emit `review.dimension.done` for)
   on the next iteration.

`enrich_task_resume_payload` adds `stage: "missing_event"` for the
missing-event-gate path. R5 forbids adding `stage` to
`required_fields` (it would break the schema SSOT for the
`publish_policy_rejection_resume` path), so the field is added at the
payload level only.

### U4 — `diagnosis-summary.json` dual-path recovery count (R6)

`build_termination_diagnostics` reads both:

- `.ralph/recovery.jsonl` (workspace-level, CLI rejects)
- `.ralph/diagnostics/<session>/recovery.jsonl` (session-level, hard-gate injections)

and reports `recovery_count = workspace_count + session_count` plus
`recovery_journal_path` listing both files (KTD-6: dual-index, no migration).
The hard-coded `recovery_count: 0` in `runner.rs` is replaced with a
`count_recovery_entries` helper that does the read-and-sum.

### U5 — Plan frontmatter drift detection (R7)

New `ralph doctor plan-sync [--plan PATH]` subcommand:

- Reads plan YAML frontmatter `status` / `units`.
- Reads `.ralph/agent/tasks.jsonl` for the same `plan_name`.
- Rule: if a unit is `closed` in tasks.jsonl, plan frontmatter must not
  contain `stalled-after-U<that-unit>`. If tasks are all `closed` for a
  plan, plan frontmatter must be `completed` (or
  `u1-closed-u2-...-merged-into-XXX`).
- Coordinator instructions in `ce-executor-serial.yml` gain a HARD RULE
  requiring frontmatter sync at every `work.done`.

KTD-7: the doctor is a detector, not an auto-fixer. The orchestrator does
not write plan files (out-of-scope, "automatically editing docs/ is too
magical"). The detection + the explicit backpressure suffice.

### U6 — E2E validation: BDD + noble-peacock replay fixture (R8)

`crates/ralph-core/tests/scenarios/ce_executor_serial_review_silent_reviewer_recovers.yml`
exercises the silent-DR → recovery shape. A synthetic
`crates/ralph-core/tests/fixtures/noble-peacock-review-stall/replay.jsonl`
captures the wire-level shape (work.start → work.ready → work.done →
review.passed leak → review.dimension.ready → silence → task.resume →
review.dimension.done → loop.cancel) as a smoke replay. Integration test
`test_noble_peacock_executor_review_passed_never_lands` in
`integration_emit_policy.rs` guards U1 with the exact noble-peacock
payload shape. See the companion U6 validation
doc (`docs/solutions/integration-issues/ce-executor-serial-noble-peacock-review-chain-2026-06-17.md`)
for the test matrix details — this doc focuses on the
mechanism-and-context design.

## Key Technical Decisions (Why)

| ID | Decision | Why |
|---|---|---|
| KTD-1 | isolated + business topic + `hat=None` → CLI **reject** (not runtime drop) | Fail-closed at the boundary matches the runtime origin guard. `check_isolated_scope:115-117` no-op is the leak root cause; soft-warn would not reach a confused prompt. |
| KTD-2 | Reuse preset `require_emit_provenance`; isolated defaults to "require provenance" for business topics | Avoids new flag proliferation; aligns with the existing KTD-3 from plan 003. |
| KTD-3 | `HatActivationClock` per-hat, not `hat.timeout` wiring | Main runner doesn't read `hat.timeout`; wiring it is a separate concern. New clock is purpose-built for gate defer. |
| KTD-4 | grace = `min(adapter_idle * 0.3, 540)`; serial preset overrides `dimension-reviewer: 540` | Aligns with diagnostic report; 540s floor prevents tiny-adapter misfires. |
| KTD-5 | Double-track: `Event::with_target` routing + `replay_trigger_to_activation_state` | Single-track (`pending_recovery_hat` pin only) is insufficient; DR triggers don't include `task.resume`. |
| KTD-6 | Dual-path index for recovery, not migration | Avoid breaking CLI reject tooling and operator grep habits. |
| KTD-7 | `ralph doctor plan-sync` is a detector; coordinator instructions carry the obligation | Auto-editing plan files crosses the orchestrator's scope; detection + explicit backpressure is the Ralph way. |

## Acceptance Examples (AE1–AE5)

| AE | What it proves | How it's tested |
|---|---|---|
| AE1 | CLI no longer leaks executor `review.passed(aggregate_timeout)` | `test_noble_peacock_executor_review_passed_never_lands` in `integration_emit_policy.rs`; manual smoke `ralph emit` exits non-zero with `missing_provenance` or `isolated_scope_violation` |
| AE2 | dimension-reviewer first-iteration silence is not killed | `test_missing_event_gate_defers_within_grace` in `loop_runner/tests.rs`; bounded by `now < activation + grace` |
| AE3 | After grace, recovery re-activates DR with the original dimension context | `test_ce_executor_serial_review_silent_reviewer_recovers_scenario` in `scenarios.rs`; asserts `review.dimension.done` fires on iter 5 with `correctness` payload |
| AE4 | `diagnosis-summary.json` reports non-zero recovery | `test_diagnose_reports_recovery_count` in `diagnose.rs`; injects 3 cli_emit + 1 missing_event_gate, asserts `recovery_count >= 4` and `recovery_journal_path` lists both journals |
| AE5 | plan frontmatter drift is detectable | `ralph doctor plan-sync` exits non-zero on plan 003 with the shipped frontmatter; after this doc lands, exits 0 |

## Relationship to Plan 003

Plan 003 closed **the routing half** of ce-executor-serial review-chain
failure. Plan 004 closes **the clock + context half**. They are not
parallel paths; they are layered.

| Failure mode | Plan 003 unit | Plan 004 unit |
|---|---|---|
| Out-of-scope emit lands in JSONL | U1: `check_isolated_scope` (with hat) | U1: `check_emit_provenance` (no-hat path) |
| Orchestrator `task.resume` missing fields | U2: schema-compliant wrapper | U5: enrich with `stage: missing_event` |
| Automated recovery impersonates human | U3: switch to `task.resume` | (inherited) |
| Steward subscribed to `human.guidance` | U4: narrow to `loop.stalled` | (inherited) |
| Repeated `review.dimension.ready` | U5: policy-layer dedup | (inherited) |
| `hat=None` bypasses precheck | (not caught — agent always set hat) | U1 plan 004: fail-closed |
| Gate kills long-running hat | (not caught — merry-lotus had 13s repeat) | U2 plan 004: HatActivationClock + grace |
| Recovery lacks trigger context | (not caught — merry-lotus executor was already in cycle) | U3 plan 004: replay into activation state |
| `recovery_count: 0` hard-coded | (deferred as P1-3) | U4 plan 004: dual-path count |
| Plan frontmatter drift | (manual observation only) | U5 plan 004: `ralph doctor plan-sync` |

Plan 003 is now in the achieved archive with
`status: u1-closed-u2-u5-merged-into-plan-004`. U1 was shipped in plan 003;
U2–U5 are the units whose additional scope was rolled into plan 004 and
shipped there.

## Reusable Invariants (Prevention)

These are the rules that, if preserved in future code, will prevent the
noble-peacock class of failure from re-emerging:

1. **isolated + business topic ⇒ provenance required.** No emit to a
   business topic from CLI without `--hat` or `RALPH_CURRENT_HAT`. The
   `hat=None` no-op in `check_isolated_scope` must never be re-introduced
   for business topics; only orchestrator-internal and
   `RALPH_CONTROL_TOPICS` may be emitted hat-less. Add a unit test on the
   precheck's `hat=None` branch for every new business topic added to a
   preset.

2. **Long-running hats need grace.** `missing_event_gate` (and any
   iteration-bound "obligation not satisfied" check) must use a per-hat
   activation clock, not the iteration clock. The grace default
   `min(adapter_idle * 0.3, 540)` is a heuristic; the per-hat override
   mechanism is the contract. New hats with `concurrency > 1` or
   `aggregate` mode must define `missing_event_grace_secs` or inherit the
   preset default.

3. **Recovery must preserve trigger context.** Any orchestrator-injected
   `task.resume` (hard_gate, policy rejection, isolated scope rejection)
   must carry the original obligation trigger into the activated hat's
   `last_activation_events`. The double-track pattern (`Event::with_target`
   for routing + `replay_*_to_activation_state` for context) is the
   contract. Tests that only assert the topic is `task.resume` and the
   payload has `target_hat` are insufficient; they must also assert the
   activation state was extended with the original trigger.

4. **Diagnosis must aggregate multi-path recovery.** A
   `diagnosis-summary.json` that hard-codes `recovery_count: 0` (or any
   single-source count) is structurally blind. The
   `count_recovery_entries(workspace_root, session_id)` helper pattern
   is the contract. New recovery envelope sources (recovery, drift,
   session-specific JSONL) must be added to the helper, not to a
   separate counter.

5. **Plan frontmatter drift is a mechanism concern, not a human
   concern.** The orchestrator does not auto-edit plan files, but it
   must surface the drift. `ralph doctor plan-sync` is the contract.
   Coordinator instructions for any preset that emits `work.done` (i.e.
   closes implementation units) must include the frontmatter-sync HARD
   RULE.

## What Didn't Work

- **Tying recovery grace to `hat.timeout`.** `hat.timeout: 1800` is in the
  preset YAML; the main loop runner does not read it. Wiring it would
  require changes to `LoopRunner::run_iteration` and the
  `ExecutionContract` path — out of scope for this plan (KTD-3 deferred to
  follow-up). The new `HatActivationClock` is a separate clock for
  this purpose.
- **Asserting dimension-reviewer should subscribe to `task.resume`.** It
  would technically solve the routing half, but it expands hat
  subscription and creates a new obligation cycle (now DR has to emit
  *something* on every `task.resume` it sees, even ones not meant for
  it). The double-track routing + context-replay approach is narrower.
- **Migrating `.ralph/recovery.jsonl` to the session directory.** Many
  external tools (`ralph clean --diagnostics`, operator grep habits,
  CI log scrapers) depend on the workspace path. The dual-index keeps
  both paths valid.

## Related Issues

- `docs/achieved/plan/2026-06-17-004-fix-ce-executor-serial-noble-peacock-review-chain-plan.md` —
  the plan this solutions entry implements. U1-U5 are the fix units; U6
  is the validation layer (covered in a companion doc).
- `docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md` —
  the diagnostic report that drives every decision in this entry.
  `events-20260617-095504.jsonl` lines 3-4 are the literal payload that
  U1 closes; the 49s gate is the literal clock that U2 defers.
- `docs/achieved/plan/2026-06-17-003-fix-ce-executor-serial-precheck-recovery-gates-plan.md` —
  the previous plan (merry-lotus). U1-U5 here complement its U1-U5; the
  status update to
  `u1-closed-u2-u5-merged-into-plan-004` is part of U5.
- `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md` —
  the companion solutions entry for plan 003. The "Why This Works" section
  there (Single Write Gate) is the upstream principle this entry extends
  with the clock-alignment and context-replay invariants.
- `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-review-chain-2026-06-17.md` —
  the U6 validation layer (BDD scenario, smoke fixture, integration test).
  This entry is the U1-U5 mechanism layer; together they form a
  mechanism-and-validation pair.
- `docs/solutions/developer-experience/agent-execution-contract-gates-2026-06-03.md` —
  the product decision that `human.guidance` cannot drive recovery. U1
  of plan 003 closed the `task.resume`/`human.guidance` impersonation;
  this plan inherits that fix and adds the missing clock/context halves.
- `docs/brainstorms/2026-06-13-ralph-cli-test-concurrency-via-nextest-requirements.md` —
  CLI test concurrency invariants that this plan's U6 verification
  commands must respect (`cli-serial` for ralph-cli, parallel for
  ralph-core). T6.3 in the plan verification matrix is bounded by these
  rules.
