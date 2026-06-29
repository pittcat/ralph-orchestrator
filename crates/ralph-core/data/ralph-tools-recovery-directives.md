---
name: ralph-tools-recovery-directives
description: Runtime recovery directives injected on task.resume events
metadata:
  internal: true
---

# Runtime Recovery Directives

> This skill is auto-injected by the runner when a `task.resume` event carries `recovery_directives`. You do not need to load it manually. Follow the rules below as system operating procedure.

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
- Emit `human.guidance` or `loop.stalled` with a concise reason and the last attempted action.
- Then wait for the orchestrator or operator to route the next step.

**禁止：** 在 stall 状态下无限循环重发同一事件。

## RD-PLAN-BLOCKED-ON-RECOVERY-EXHAUSTED

**Trigger:** `task.resume` with `target_hat=<被恢复的 hat>` and `kind=recovery_exhausted`.

**行为规范：**
- Do not attempt another retry. Emit `plan.blocked(reason="recovery_exhausted:<retry_key>")` immediately.
- Surface the blocking reason in the current task note so the next agent/human understands the terminal state.
- Treat this as a final state, not a recoverable error.

**禁止：** 在收到 recovery_exhausted 后继续重发或尝试绕过。

---

**对应 runtime 判定函数（reviewer only, not injected）：**
`recovery_runtime::dedupe_stall_recovery_with_missing_event_gate`,
`recovery_runtime::finalize_recovery_outcome_on_flapping`,
`recovery_runtime::publish_loop_stalled_business_event`,
`recovery_runtime::block_executor_resend_storm`.
