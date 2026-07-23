---
date: 2026-07-23
title: 2026-07-23-002 计划 U8 完成对账与 residual
module: ralph-cli / ralph-core
tags: [supervisor, payload-consistency, u8, closure]
problem_type: plan-reconciliation
status: closed-with-documented-residuals
plan: docs/plans/2026-07-23-002-fix-payload-supervisor-review-closure-plan.md
---

# 2026-07-23-002 U8 完成对账

## Units

| Unit | 状态 | 证据 |
|---|---|---|
| U1 | 完成 | CLI/Apply disposition 同源；无 `payload_consistency:` Warn 私升 |
| U2 | 完成 | `gate` + `referenced_fields`；BDD accept/reject |
| U3 | 完成 | `safe_display` + lint `unsafe_message` |
| U4 | 完成 | `ralph-tools*.md` 公开合同；drift 脚本绿 |
| U5 | 完成（部分 N/A） | 见下表 |
| U6 | 完成 | Outside-In：non-blocking(1 fix) / blocking(2 fix) / fault；nonce markers；review shared-readonly |
| U7 | 完成（KTD8 等价实现） | isolated ralph 不 drain pending；phase Done；`finalize_terminal_cleanup`；fault `Ok+success=false` 空事件记失败 |
| U8 | 完成（本文件） | 对账 + residual；`./scripts/run-tests.sh` 全绿（6349 passed + doctest） |

## U5 同步面

| 面 | 处置 |
|---|---|
| `finding-rubric` / `author-checklist` / fixtures / `commands.md` | 已改 |
| `CONCEPTS.md` / `docs/guide/payload-consistency.md` | 已改 |
| `ralph-preset-author/SKILL.md` | N/A：已有 payload consistency 入口，细则在 checklist |
| `ralph-preset-review/SKILL.md` | N/A：走 rubric/fixture，无新增 CLI 表面 |
| `agent-native-model.md` / `patterns.md` | N/A：无 agent-native 模型变更 |
| `ralph-project-bootstrap/references` | N/A：无 bootstrap 字段变更 |
| `docs/guide/opac.md` / `presets.md` | N/A：OPAC/preset 拓扑未改；payload 细节在 payload-consistency guide |
| `.cursor/rules/*` | N/A：硬规则未改 |
| `CLAUDE.md` / `AGENTS.md` | 未改（cmp 一致） |
| diagnosis report `docs/report/2026-07-22-…` | N/A：被本计划 + solutions 取代，不改历史诊断原文 |

## U7 KTD8 合同说明

计划原文要求 store 持久化 cleanup pending/done + 崩溃重启重试。落地实现为：

- 终态时 runner 调 `finalize_terminal_cleanup`
- bridge 遍历 `list_wave_ids` + `remove_worktree`（`NotFound` 幂等）
- **未**单独持久化 cleanup pending 行；崩溃后需再次进入终态路径才会再清

等价验收：Outside-In 断言 LOOP_COMPLETE 后 slot worktree 不存在。完整 pending/restart 表留作 follow-up（非本轮 P0）。

## 明确 residual（非 P0）

1. **篡改 `success_slots` 负向 Outside-In**：integrator 资源校验的独立故障注入测未加。
2. **integrator 确定性摘要**：fake integrator 仍发固定 `work.done`；nonce 因果由 markers 证明，非 payload digest。
3. **KTD8 crash-before-cleanup restart**：见上。
4. **channel-routing-fallback / orphan-emit**：观测噪音，不挡终态断言。

## Wave 回归（本轮顺带）

- Pi 固定参数含 `--no-skills --skill .agents/skills`：更新 wave invocation 合同测。
- `record_outcome`：仅「无事件 + success=false」记失败；timeout-with-events 仍可见。
