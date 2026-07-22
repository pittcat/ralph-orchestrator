# Concepts

> Shared domain vocabulary for this project — entities, named processes, and status concepts with project-specific meaning. Seeded with core domain vocabulary, then accretes as ce-compound and ce-compound-refresh process learnings; direct edits are fine. Glossary only, not a spec or catch-all.

## Multi-hat orchestration

### Hat

An isolated agent role in a preset loop. Each activation gets fresh context, own `publishes`/`triggers`, and optional tool restrictions. Hats coordinate through the event bus and runtime APIs (`ralph emit`, `ralph tools task`), not shared process state.

### dimension-reviewer

A read-only review hat in `ce-executor-pipeline` (and historically `ce-executor-serial`). Reviews one quality dimension per activation, emits `review.dimension.done` or `review.dimension.failed`, and may write findings only under the plan scratchpad. Must not modify tracked plan or source files; doing so triggers scope violation audit.

### disallowed_tools

Per-hat list of tool names the role must not use. In Ralph today this drives prompt TOOL RESTRICTIONS and post-iteration file-modification audit when `Edit` or `Write` is listed; CLI backends may support additional hard denial (e.g. Claude `--disallowedTools`) when the runner merges hat config at spawn time.

### scope_violation

A hat modified tracked files despite its read-only or tool-restriction contract. For `dimension-reviewer`, U5 promotes the first violation to a hard loop termination (`ScopeViolationHardRejected`) rather than a recoverable counting failure.

### artifact-first handoff

一种跨 hat 或 hat 与 sub-agent 的交接原则：完整结果、可恢复状态、证据和关键决策依据优先写入当前 workspace/worktree 的 `.ralph/` 业务 artifact，消息与事件只传递短状态、摘要、路径、必要身份和路由字段。Ralph 的内部 ledger 不属于可供 hat 自定义读写的业务 artifact。

### wave

同一 hat 定义下、对 N 份同构 payload 的 orchestrator 级 fan-out/fan-in。共享 `wave_id`，用 `wave_index` / `wave_total` 区分 slot。需要账本级收齐、超时与诊断时用 wave；hat 内部分工优先 subagent。`events.jsonl` / `supervisor.db` 是 runtime ledger，不是 hat 业务 artifact。

### OPAC

Observe → Precheck → Apply → Confirm。isolated 模式下 state-changing 操作的纪律框架；Precheck/ACL 可由 CLI 硬闸，Confirm 对 wave/task 逐步硬化为 ticket 或公开查询证据。

### payload consistency（载荷一致性闸）

挂在 `event_policy.payload_consistency` 的同 payload 验收 checkpoint（默认关闭、preset opt-in）：在 `ralph emit` / `--policy-check` 与真实 Apply 使用同源路径上，按 preset 声明的谓词检查**当前交卷 JSON 是否自洽**（例如声称成功却与计数/状态互斥）。命中时以 `payload_consistency:<rule_id>` 拒收，agent 可读恢复说明沿用其他 `SemanticGateViolation` 拒收的同一条恢复通道；不读事件历史（跨事件互斥是 follow-up）。与 OPAC（操作纪律）和 `execution_contracts`（完成证据义务）分工，不互相替代。

### wave protocol suite（六件套）

Wave/supervisor 协议层能力集合：反压、分布式取消、状态持久化、幂等键、内容哈希去重、补偿。语义 SSOT 在 `SupervisorStore`；默认 wave 路径应吸收该套件，而非仅 `supervisor.enabled` preset。
