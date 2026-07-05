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
