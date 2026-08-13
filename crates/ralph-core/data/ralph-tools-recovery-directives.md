---
name: ralph-tools-recovery-directives
description: Runtime recovery directives injected on task.resume events
metadata:
  internal: true
---

# Runtime Recovery Directives

> This skill is auto-injected by the runner when a `task.resume` event carries `recovery_directives`. You do not need to load it manually. Follow the rules below as system operating procedure.

`loop.resume` 与本 skill 无关：它只在 `ralph run --continue` 启动 loop 时作为 bootstrap 信号出现。本文档中的 `task.resume` 专指运行期间的 recovery/correction；收到 `loop.resume` 时不要执行本文档的纠错重试流程。

## 收到恢复信号时

**触发条件：**当前 activation 收到 `task.resume`。`hat` 是本次被激活、负责处理该恢复信号的工作单元；恢复 payload 是随信号提供的结构化上下文。

**执行动作：**先读取恢复 payload，以及 prompt 中的 `## CORRECTION CONTEXT`（如果存在）。优先使用 `required_action`、`reason`、`kind`、`original_trigger_topic`、`original_trigger_payload` 和 `allowed_topics` 等实际存在的字段。字段缺失时不要把缺失值推断成空字符串、`false` 或默认 topic。

如果需要重新发业务事件，必须按 `ralph-tools-emit` 的规则执行完整的 precheck → apply → confirm：预检使用 `--policy-check --output json`，正式 emit 去掉 `--policy-check` 并确认 `ok=true`、`recorded=true`。只有正式落盘回执成立才算恢复完成；不要把预检的 `ok=true` 当成已发布。不要为了恢复而修改 preset、重复广播或原样重发同一个 payload。

**失败停止条件：**恢复对象、原始触发上下文或允许的发布范围缺失且无法从当前 prompt 确认时，停止本次恢复动作；使用 `ralph inspect loop --format json` 或 `ralph tools task list` 复核当前任务状态。复核后仍无法确定合法下一步时，报告阻塞原因，不要猜测目标或继续重试。

## Correction 优先级（通用）

当 prompt 同时出现 agent narrative 与 runtime 结构化 correction 时：

1. **`## CORRECTION CONTEXT` / `task.resume.required_action` / `forbidden_action` 优先于一切自由推断** — 只执行 correction 指定的**唯一**动作。
2. **不要**在同 activation 发第二个业务事件（isolated 单事件预算）。
3. **bounded retry**：同类协议违规 signature（hat + topic + `task_key` + step + violation code）**第一次** → structured correction + 可执行 retry target；**第二次** → 阻塞 loop（`plan.blocked(reason=protocol_violation_repeated:…)`），**不得** infinite `task.resume` 或在没有 `LOOP_COMPLETE` 的情况下静默继续。
4. **post-terminal**（loop 终态 honored）：业务 emit 拒写，**不**进入 retry budget。
5. 收到 correction 后仍须先 `ralph emit --policy-check --output json`，通过后再正式 emit，并确认 `ok=true`、`recorded=true`（见 `ralph-tools-precheck`）。

Preset 专用 trigger 状态表写在各 preset 的 hat `instructions:`；本文件只写通用 recovery 语义。

## RD-PAYLOAD-CONSISTENCY-GATE

**Trigger:** `task.resume` with `target_hat=<被恢复的 hat>` and a payload-level `violation` whose `gate` 字段前缀为 `payload_consistency:`。该 gate 与现有 `event_policy:semantic_gate_violation` 走同一 recovery 通道（`task.resume` → CorrectionContext）。

**行为规范：**
- 把 `task.resume` payload 视作结构化 correction：读 `field` / `reason_code` / `message` / `gate`（与 `validation_errors[]` 字段含义一致），按命中的字段修复 payload。
- 修复后**必须**先 `ralph emit <topic> --policy-check -j '<payload>'` 通过，再正式 emit。
- 同类 violation signature（同一 `gate` 前缀 + `field` + `task_key` + step）**第 3 次**同类 task.resume 后 runtime 会阻塞 loop（`plan.blocked(reason=correction_3_strike_exhausted:…)`）。payload_consistency 拒收**不**计入协议违规重试额度——它走 correction 通道独立计数。**`protocol_violation_repeated:*`** 是协议违规路径的阻塞标记，**不**用于 payload_consistency；不要把两者混用。

**禁止：** 在没有按 `gate` 命中的字段重新对齐 payload 的情况下，机械重发同一份 payload。

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
- Emit `loop.stalled` with a concise reason and the last attempted action. `human.guidance` is no longer a valid emit target.
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
- The orchestrator detected that a handoff's target consumer does NOT subscribe to the handoff's topic (the orchestrator's `triggers:` check rejects misrouted handoffs before the pending-registration timer).
- Without this detection the handoff would silently stall for 600s, then escalate to `task.resume → recovery_exhausted:stall_recovery:...:handoff_dispatch_timeout`.
- The orchestrator now skips the 600s pending registration and emits this diagnostic immediately. The producer's topic emissions are also bypassed.

**行为规范：**
- Do NOT attempt to fix this from the consumer hat — the producer is misrouted, not the consumer.
- Surface the diagnostic payload to the operator / next human so the producer's `publishes:` scope or the consumer's `triggers:` list can be corrected.
- If you are the producer hat: stop emitting the topic that was flagged until the topology is repaired.

**禁止：** 在没有 topology 修复的情况下继续走 consumer hat 路径（会再次触发同一种 misroute）。

## RD-MANIFEST-RESUME-CONTINUE

**Trigger:** `task.resume` with `target_hat=<被恢复的 hat>` and `reason=manifest_resume` / `kind=manifest_resume`。

**如何看到本段：** 这类恢复的 payload 里 `recovery_directives` 是空数组，本节不会随恢复自动进入你的 prompt；它是恢复通道的参考资料。收到 `reason=manifest_resume` 又需要完整规范时，用 `ralph tools skill load ralph-tools-recovery-directives` 手动加载本 skill。

**What this means:**
- 这是运行复用后的**恢复引导**，**不是**事件拒收纠正：上一轮运行的归档记录了 pending hat（已被触发但尚未执行完的 hat）及其原始触发事件快照，本次启动通过了身份校验（plan 文件 / preset / 配置 / 名称一致），runtime 因此通过标准 `task.resume` 通道把你重新绑定到原始触发，而不是重启整个流程。
- payload 里的 `original_trigger_topic` / `original_trigger_payload` 就是你的原始触发快照（与你当时收到的触发 payload 同形）；`message` 字段描述恢复边界；如存在 `wave_id` / `wave_index` / `wave_total`，表示恢复点在某个 wave 中途。

**行为规范：**
- 把这条 `task.resume` 当作你的触发事件处理：读 `original_trigger_payload`，按 hat instructions 从中断处继续工作。
- 不要重做已被接受的工作，也不要重发已被接受的事件；重复同样的复用启动不会产生重复恢复，无需手动补偿。
- 恢复后 task 关闭 / wave 派发等结算权限不变，仍按 preset 既有机制执行。

**禁止：** 收到该恢复后重启整个流程，或把它当作协议违规去找 `violation` / `required_fields` 修复。

---
