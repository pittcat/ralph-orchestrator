# 示例：LOGS_ONLY 布局（无 timestamp session）

典型外部 worktree / 未开 `RALPH_DIAGNOSTICS=1` 的 run：

```
.ralph/
├── agent/          handoff.md, summary.md, tasks.jsonl, progress.md, plan-baseline-*.sha
├── current-events  → .ralph/events-YYYYMMDD-HHMMSS.jsonl
├── events-history-*.jsonl
├── history.jsonl
├── ledger.jsonl
├── recovery.jsonl  （可能 0 行：无拒收时仍可能存在空文件）
├── loops.json, loop.lock, current-loop-id
└── diagnostics/
    ├── logs/ralph-*.log          ← OPAC/scope 主证据
    ├── agent_doc_sync.json
    └── channel-routing-fallback-*.md

.agents/scratchpad/.../            ← ce-executor-serial 等 preset 的 Tier C
```

**诊断含义**：无 `orchestration.jsonl` / `agent-output.jsonl` 是**预期**，不是 run 损坏。OPAC 审计降级见 [opac-audit-by-mode.md](../opac-audit-by-mode.md)。

## 示例：activation outcome 证据（合成示例）

以下内容只展示报告/诊断 agent 可读取的最小 raw artifact 形状，**不代表真实 run 已发生**：

```jsonl
{"phase":"activation","kind":"hat_activation_outcome","hat":"executor","status":"empty","source_ref":".ralph/agent/events-hat-executor.jsonl","fields":{"channel_bytes":0,"backend_success":true,"backend_exit_code":0,"candidate_event_count":0,"accepted_event_count":0,"rejected_event_count":0,"terminal_obligation_topics":["work.done"]}}
```

遇到 `status=empty` 时，必须继续核对 events、recovery 和 fallback evidence；单独一条 empty row 只能形成 evidence gap，不能直接归因于 agent。
