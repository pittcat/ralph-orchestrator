---
date: 2026-06-17
topic: agent-recovery-mechanism-gaps
title: "Agent 恢复链机制边角 — CLI 预检与诊断对齐"
related:
  - docs/plans/2026-06-17-002-feat-ce-executor-step-handoff-plan.md
  - docs/plans/2026-06-17-004-feat-ralph-core-data-doc-sync-plan.md
  - docs/code-review-2026-06-17-002.md
  - docs/report/2026-06-16-systematic-review-of-recent-fixes.md
---

## Summary

近两个月已在 runtime 落地大量恢复机制（`task.resume` 路由、`SemanticGateViolation`、incomplete wave `plan.blocked`、`progress_task_gate`、`HandoffTracker` SLA 等）。**Plan A（017-004）** 补 agent 文档闭环；**本文档（Plan B）** 补 **文档教不了、必须机制层挡/记** 的边角，使 CLI 预检、loop gate、诊断三者一致。

---

## Problem Frame

| 现象 | 根因 |
|------|------|
| `ralph emit --policy-check` 通过，loop 内 `queue.advance` 仍被 `progress_task_gate` 拒 | CLI 只跑 `event_policy`，未接 step handoff gate |
| `ralph diagnose` 难查 handoff stall | `runtime-diagnosis.md` 未覆盖 `handoff_dispatch_timeout` / `progress_task_mismatch` 排查路径（finding #18） |
| `plan.blocked` 偶发 origin 拒收 | gate 注入时 `source` hat 非合法 publisher（finding #1） |
| `event.step_handoff.gate_rejected` 不可见 | 诊断 topic 未进 orchestrator 白名单（finding #2） |

**非目标**：重复 017-003 已 merge 的 wave stall / semantic gate；重复 017-004 的 `ralph-tools` 文档；大改 preset 拓扑。

**已闭合（本需求不重复）**：`plan-gate.triggers` 已含 `fix.exhausted` / `debug.exhausted`（preset 当前树）；`progress_task_gate` → `recovery.jsonl` envelope 写入路径已在 `event_loop/mod.rs` 集成（review fix #4）。

---

## Requirements

### A. CLI 与 loop gate 对齐

- **R-A1.** `ralph emit --policy-check`（及 config enforce 路径）对 `progress_task_gate::GATED_TOPICS`（`queue.advance`、`plan.complete`）增加 **与 loop 同源** 的 progress/tasks 对齐预检。
- **R-A2.** 预检失败输出结构化 `reason_code`（如 `progress_task_mismatch`），与 loop 内 `plan.blocked` payload 语义一致；**不写盘**。
- **R-A3.** 预检 **不** 替代 loop gate（双检：CLI 早失败 + loop 终裁）；行为与 `policy_check_handoff.rs` 四消费链测试可扩展。

### B. 诊断与可观测性

- **R-B1.** `docs/guide/runtime-diagnosis.md` 增加 `handoff_dispatch_timeout`、`progress_task_mismatch` 症状 → 证据文件 → 修复动作（与 017-004 guide 短链互引）。
- **R-B2.** `event.step_handoff.gate_rejected` 纳入 orchestrator 诊断 topic 白名单，避免 isolated per-turn budget 误丢。

### C. Gate 注入正确性

- **R-C1.** `apply_step_handoff_gate` 注入的 `plan.blocked` **始终** 使用合法 publisher provenance（默认 `plan-gate` hat + 与 `EventOriginGuard` 一致的 `publishes`）。
- **R-C2.** 非 JSON handoff payload 在 gate 路径 **fail-closed**（不静默 `(None, None)` 惰性通过）——对齐 finding #6 最小修复。

### D. 验收

- **R-D1.** 扩展 `crates/ralph-cli/tests/policy_check_handoff.rs` 或邻接集成测试：progress 漂移时 CLI `--policy-check` 非零。
- **R-D2.** BDD `progress_task_mismatch` scenario 仍绿；全 workspace nextest（exclude ralph-e2e）绿。
- **R-D3.** `ralph preset check --strict -H builtin:ce-executor-isolated` 绿。

---

## Success Criteria

- **SC1.** Agent 在 CLI 层试发 misaligned `queue.advance` 时，**在写盘前** 得到与 loop 一致的拒收原因。
- **SC2.** `ralph diagnose` 文档可指引 operator 定位 handoff stall / progress 漂移。
- **SC3.** Gate 注入的 `plan.blocked` 不因 origin 二次拒收。

---

## Scope Boundaries

**Deferred for later**

- `ralph hats show` 输出 `trigger_multi_consumer_topics`（finding #20）
- `trigger_multi_consumer_topics` 全局 typo 校验强化（finding #3，单独 preset lint 任务）
- `diagnosis-summary.json` recovery_count 对账（systematic review P2-5）

**Outside scope**

- 修改 `ralph-tools.md` 自动注入（017-004）
- Wave / flow lifecycle 新机制（017-001/003）

---

## Dependencies

- 017-002 step handoff 代码已合入或合入中
- 017-004 与本文档 **可并行**，但应 **分 PR 合并**
