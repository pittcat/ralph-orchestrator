---
name: ralph-tools-recovery-directives
description: Runtime recovery directives injected on task.resume events
metadata:
  internal: true
---

# Runtime Recovery Directives

> This skill is auto-injected by the runner when a `task.resume` event carries `recovery_directives`. You do not need to load it manually. Follow the rules below as system operating procedure.

## Correction 优先级（通用）

当 prompt 同时出现 agent narrative 与 runtime 结构化 correction 时：

1. **`## CORRECTION CONTEXT` / `task.resume.required_action` / `forbidden_action` 优先于一切自由推断** — 只执行 correction 指定的**唯一**动作。
2. **不要**在同 activation 发第二个业务事件（isolated 单事件预算）。
3. **bounded retry**：同类协议违规 signature（hat + topic + `task_key` + step + violation code）**第一次** → structured correction + 可执行 retry target；**第二次** → fail-close（`plan.blocked(reason=protocol_violation_repeated:…)`），**不得** infinite `task.resume` 或 silent-success。
4. **post-terminal**（loop 终态 honored）：业务 emit 拒写，**不**进入 retry budget。
5. 收到 correction 后仍须先 `ralph emit --policy-check`，通过后再正式 emit（见 `ralph-tools-precheck`）。

Preset 专用 trigger 状态表写在各 preset 的 hat `instructions:`；本文件只写通用 recovery 语义。

## RD-EXECUTOR-RESEND-LIMIT

**Trigger:** `task.resume` with `target_hat=<被恢复的 hat>` and `kind=missing_event_gate`.

**行为规范：**
- After receiving this resume, you may re-emit `work.done` **at most 2 times** to close the gate.
- Before each retry, read `ralph emit --schema work.done` and confirm every `required_fields` item is present and correctly typed.
- On the third failure, stop retrying and emit `work.failed` with `reason="re-emit_exhausted"`.

**禁止：** 连续重发同一 `work.done` payload 超过 2 次而不改字段内容。

## RD-TASK-ID-MUST-BE-LOOP-SCOPED

**Trigger:** `task.resume` with `target_hat=<被恢复的 hat>` and `kind=execution_contract:TaskWrongLoop`.

**行为规范：**
- Before re-emitting `work.done`, read `.ralph/agent/tasks.jsonl` and use the task id that belongs to the **current loop**.
- The `task_id` field must be a non-empty string taken from the current loop's runtime task ledger.
- Do not reuse task ids from other loops, plan files, or prompt text fragments.

**禁止：** 使用 `""`、`null`、或 `from_key:...` 形态的字符串作为 `task_id`。

## RD-STALL-DETECT-AND-YIELD

**Trigger:** `task.resume` with `target_hat=<被恢复的 hat>` and `kind=stall_recovery`.

**行为规范：**
- If no expected event (`test.passed`, `review.*.done`, etc.) arrives within approximately 30 seconds, yield instead of repeating the same emit.
- Emit `loop.stalled` with a concise reason and the last attempted action. `human.guidance` is no longer a valid emit target (plan 2026-06-28-005).
- Then wait for the orchestrator or operator to route the next step.

**禁止：** 在 stall 状态下无限循环重发同一事件。

## RD-PLAN-BLOCKED-ON-RECOVERY-EXHAUSTED

**Trigger:** `task.resume` with `target_hat=<被恢复的 hat>` and `kind=recovery_exhausted`.

**行为规范：**
- Do not attempt another retry. Emit `plan.blocked(reason="recovery_exhausted:<retry_key>")` immediately.
- Surface the blocking reason in the current task note so the next agent/human understands the terminal state.
- Treat this as a final state, not a recoverable error.

**禁止：** 在收到 recovery_exhausted 后继续重发或尝试绕过。

## RD-HANDOFF-MISROUTE-DETECTED

**Trigger:** Orchestrator emits `task.resume.misrouted` diagnostic event. The diagnostic payload contains the offending `consumer` hat ID and the `topic` that consumer's `triggers` did not declare.

**What this means:**
- The orchestrator detected that a handoff's target consumer does NOT subscribe to the handoff's topic (via the shared `check_hat_triggers` helper).
- Without this detection the handoff would silently stall for 600s, then escalate to `task.resume → recovery_exhausted:stall_recovery:...:handoff_dispatch_timeout` and route through shipper's prefix-allowlist as `REVIEW_COMPLETE(pass)` — the silent-success loop family (see `docs/report/2026-07-06-ce-executor-serial-primary-20260705-224028-diagnosis.md`).
- The orchestrator now skips the 600s pending registration and emits this diagnostic immediately. The producer's topic emissions are also bypassed.

**行为规范：**
- Do NOT attempt to fix this from the consumer hat — the producer is misrouted, not the consumer.
- Surface the diagnostic payload to the operator / next human so the producer's `publishes:` scope or the consumer's `triggers:` list can be corrected.
- If you are the producer hat: stop emitting the topic that was flagged until the topology is repaired.

**禁止：** 在没有 topology 修复的情况下继续走 consumer hat 路径（会再次触发同一种 misroute）。

---

**对应 runtime 判定函数（reviewer only, not injected）：**
`recovery_runtime::dedupe_stall_recovery_with_missing_event_gate`,
`recovery_runtime::finalize_recovery_outcome_on_flapping`,
`recovery_runtime::publish_loop_stalled_business_event`,
`recovery_runtime::block_executor_resend_storm`.
