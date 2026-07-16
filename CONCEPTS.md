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
