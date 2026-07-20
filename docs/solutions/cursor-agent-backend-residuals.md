---
module: ralph-adapters
tags: cursor, agent-stream-json, residuals
problem_type: cleanup
---

# Cursor Agent Backend 残余清理项（M3 / M4 / A4 / A5）

本文记录 Cursor `agent` backend 当前不阻塞主流程的四项清理债务，供后续独立计划复访；它们不改变当前已验证的解析、PTY dispatch、headless 输出和自动发现行为。

## M3：`AgentStreamEvent` 未建模字段的 `flatten` 兜底

- **症状**：`crates/ralph-adapters/src/agent_stream.rs` 的具名事件变体使用 `#[serde(flatten)] extra: serde_json::Value` 收纳未知字段，schema 新增字段会被静默保留但不会进入类型或诊断逻辑。
- **定位**：`AgentStreamEvent` 及其 `Assistant`、`ToolCall`、`System`、`Result` 变体；派发逻辑位于同文件的 `dispatch_agent_stream_event`。
- **当前不做原因**：当前只需要稳定处理已知文本、工具生命周期和终态错误字段；对 Cursor 尚未冻结的 schema 过早建模会引入不必要的上游假设。
- **复访条件**：Cursor 发布稳定 schema，或编排器需要使用当前被 `extra` 收纳的成本、session 或 token 字段；届时应显式建模并补 schema parity 测试。

## M4：自动发现优先级中的 `agent` 决策

- **症状**：自动发现是否把 `agent` 作为默认 backend，取决于 `crates/ralph-adapters/src/auto_detect.rs` 的优先级策略；显式 backend 配置不受此项影响。
- **定位**：`DEFAULT_PRIORITY`、`detection_command`、`is_backend_available`。
- **当前不做原因**：默认优先级会改变用户未指定 backend 时的行为，必须先决定它相对于 Claude、Codex 等现有 backend 的位置，并同步安装提示与文档；此债务不应和 parser 重构混在一起。
- **复访条件**：Cursor CLI GA、安装基数足够，或用户反馈表明 `agent: auto` 的默认选择需要调整；届时应补自动发现优先级和错误提示测试。

## A4：PTY NDJSON 的 CRLF 显式处理

- **症状**：PTY 按换行符切分时可能保留行尾 `\r`，当前依赖 `AgentStreamParser::parse_line` 的 `trim()` 兜底来解析；未来严格解析或新增不经 parser 的消费者时可能暴露。
- **定位**：`crates/ralph-adapters/src/pty_executor.rs` 的 AgentStreamJson 行缓冲/残余刷新路径，以及 `agent_stream.rs::parse_line`。
- **当前不做原因**：当前所有已知 AgentStreamJson 消费路径都经过 parser，且五个 ingest 分支已收敛到共同 helper；立即在多个切分点分别加 trim 会扩大对称改动面。
- **复访条件**：parser 改为严格不 trim、出现 Windows/CRLF 实测问题，或新增 raw-line 消费者；优先在共同 dispatch 入口集中处理 `trim_end_matches('\\r')` 并补 CRLF 集成测试。

## A5：PTY 注入 PATH 的优先级

- **症状**：`inject_ralph_runtime_env` 将 Ralph 可执行文件目录放在继承 PATH 的前面，确保子进程能找到当前 Ralph 工具，但理论上可能 shadow 同名外部 CLI。
- **定位**：`crates/ralph-adapters/src/pty_executor.rs::inject_ralph_runtime_env` 的 PATH 拼接逻辑；headless 注入逻辑位于 `cli_executor.rs::inject_ralph_runtime_env`。
- **当前不做原因**：PATH 前置是子进程调用当前 Ralph runtime API 的既有契约，直接改为后置可能破坏所有 backend 的工具调用；目前没有 `agent` 同名 shadow 的实证。
- **复访条件**：出现真实 shadow 案例，或增加显式 PATH 注入策略/opt-out 配置；届时应设计兼容的 runtime bin 定位方案并补环境隔离测试。

## 建议顺序

若后续单独收尾，建议先处理 A5，再处理 M4、A4，最后处理 M3；四项相互独立，不应通过扩大当前 backend 变更范围顺带解决。
