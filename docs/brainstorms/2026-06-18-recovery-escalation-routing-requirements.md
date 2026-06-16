---
title: "Recovery Escalation Routing: 重复失败时升级 target hat"
type: requirements
status: ready-for-planning
date: 2026-06-18
origin: ce-brainstorm with maintainer
related:
  - docs/plans/2026-06-17-004-feat-ralph-core-data-doc-sync-plan.md
  - docs/plans/2026-06-17-005-fix-agent-recovery-mechanism-gaps-plan.md
  - docs/guide/runtime-diagnosis.md
---

# Recovery Escalation Routing: 重复失败时升级 target hat

## Problem

Runtime diagnosis（U0–U8）已经能识别 loop 中的各类反压点，并通过 `RecoveryResponder` 三档动作（Soft / Hard / Final）进行恢复。但当前 Hard escalation 的 `task.resume` **始终路由回源 hat**（`source_hat == target_hat`）。

当某个 hat 因 instructions 理解错误、payload 习惯性错误、或陷入局部死胡同时，反复把 `task.resume` 发回同一个 hat 会导致：
- 同样的 recovery envelope 重复出现
- iteration 空转
- 本应由 specialist hat 处理的问题被拖延

## Goal

在不改变编排层（preset topology、hat instructions、event flow）的前提下，扩展机制层的 `RecoveryResponder`，使其在检测到同一 `retry_key` 连续失败达到阈值时，能够将 `task.resume` 从源 hat **升级路由**到配置中的 escalation target hat。

## Success Criteria

1. 同一 `retry_key` 重复失败 N 次后，`task.resume` 的 `target_hat` 变为 escalation target。
2. `recovery.jsonl` 完整记录 escalation 路径（`escalated_from`、`escalated_to`、`escalation_attempt`）。
3. 默认不启用 escalation 的 preset 行为与现在完全一致（向后兼容）。
4. `ce-executor-isolated` preset 提供保守的默认 escalation mapping。
5. 出现 escalation loop（A → B → A）时，runner 终止 escalation 并走 Final / `TerminationHint`。

## Non-Goals

1. **不自动修改代码、文件或任务状态**——escalation 只是换 hat 接收 `task.resume`，具体修复仍由 agent 执行。
2. **不新增 event 类型**——继续使用 `task.resume`。
3. **不改 hat instructions**——修复逻辑仍由 hats 自己实现。
4. **不替代人类判断**——最终仍失败时继续走 Final escalation。

## Proposed Behavior

### Default Escalation Mapping for `ce-executor-isolated`

| source_hat | reason_code | escalation target | after_attempts | rationale |
|------------|-------------|-------------------|----------------|-----------|
| `executor` | `*` | `debug-resolver` | 3 | payload/execution contract 重复失败多为 root-cause 理解问题 |
| `review-coordinator` | `semantic_gate_violation` | `review-synthesizer` | 2 | wave 状态判断错误需要 synthesizer 统筹 |
| `plan-gate` | `progress_task_mismatch` | `shipper` | 2 | 进度对账失败需要 shipper 级决策 |
| `*` | `handoff_dispatch_timeout` | `coordinator` | 2 | handoff 卡住时回到 coordinator 重新评估 |

> `*` 表示通配匹配；source-specific rule 优先于 wildcard rule。

### Escalation Termination

若 `escalation_chain` 检测到循环（例如 `executor → debug-resolver → executor`），立即终止 escalation，标记当前 envelope `outcome: failed`，并返回 `TerminationHint`（不覆盖已有的 `PayloadContractViolation` 等 hard reason）。

## Scope Boundaries

### In Scope

- `RecoveryResponder` escalation routing logic
- `RuntimeDiagnosisConfig` escalation routing configuration
- Config validation for escalation targets
- Audit logging in `recovery.jsonl`
- Default mapping in `ce-executor-isolated.yml`
- Unit / integration tests

### Out of Scope

- 自动改磁盘状态（progress.md / tasks.jsonl / source code）
- 新增 event 类型
- 新增 CLI 命令
- 修改 event bus 或 hat lifecycle
- 非 `ce-executor-isolated` preset 的默认启用（保持 opt-in）

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Escalation target 选错，问题转移 | 先只在 `ce-executor-isolated` 启用；默认 mapping 保守 |
| Escalation loop | 记录 escalation chain，检测到循环立即 Final |
| Audit trail 断裂 | `recovery.jsonl` 必须记录 `escalated_from` / `escalated_to` |
| Backward compatibility break | 配置缺失时行为与现在一致 |

## Implementation Notes for Planning

建议改动文件：
- `crates/ralph-core/src/diagnosis/responder.rs` — 核心 escalation 路由
- `crates/ralph-core/src/config/telemetry.rs` — 配置定义
- `crates/ralph-core/src/config/validation.rs` — target hat 校验
- `crates/ralph-core/src/diagnosis/envelope.rs` — 可选：escalation 字段
- `presets/en/ce-executor-isolated.yml` — 默认 mapping
- `crates/ralph-core/src/diagnosis/tests/` — 单元测试
- `crates/ralph-core/src/event_loop/tests/` — 集成测试

预估改动量：200–300 行，集中在 2–3 个 Rust 文件。
