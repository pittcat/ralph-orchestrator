---
title: ce-executor-pipeline 两次 Loop 运行链路诊断报告
date: 2026-08-02
type: diagnosis
loop_id: 2026-08-02-001 / 2026-08-02-002
preset: presets/en/ce-executor-pipeline.yml
run_dir: .worktrees/2026-08-02-001-*；.worktrees/2026-08-02-002-*
status: 根因已修复；两次运行均因 preset instructions 与 event schema 的 SSOT 漂移而未形成完整终态事件
diagnostics_mode: FULL
history_search: disabled
---

# ce-executor-pipeline 两次 Loop 运行链路诊断报告

> 诊断范围仅为两次 `.ralph/` 产物、当前 preset 和当前源码；未读取历史报告、solutions、plans 或 brainstorms。

## 0. 产物盘点（Phase 0）

| Run | execution_capabilities | trusted events | diagnostics | ledger | recovery | supervisor.db | 终态 |
|---|---|---:|---|---:|---:|---|---|
| 001 operator-skills-sync | `[supervisor, wave]`（preset 含 `ralph wave`，且有 supervisor.db） | 7 行 | FULL | 14 行 | 0 条最终记录 | 有 | `LOOP_COMPLETE`，report `blocked` |
| 002 decision-confidence-gates | `[supervisor, wave]`（同上；events 含 wave/监督协同信号） | 12 行 | FULL（另有后续 trace session） | 14 行 | workspace 1 条 repair-stream 信息记录；诊断最终记录 0 | 有 | `LOOP_COMPLETE`，report `blocked` |

Tier A：两次均有 `summary.md`、`handoff.md`；`tasks.jsonl` 为 0 行，符合 preset 的 `tasks.enabled: false` 语义。Tier C：两次均有对应 normalized plan、review 产物和 report；001 的 final-verification 明确 post-verification 为 red，002 明确 75/76 通过且 1 项为既有 mirror-path 失败。

## 1. 结论摘要

### 1.1 健康度

- 判定：**部分完成 + 终态链路阻断**，不是业务实现立即失败，也不是成功闭环。
- P0：0；P1：1；P2：1。
- 最高优先级根因置信度：**P0-001 = 95/100**。

### 1.2 强制四问

| # | 问题 | 答案 | 证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 执行与 OPAC 是否合规？ | ⚠️ | accepted events 有 work.done/stabilization.done，但 reporter 最终 verdict 均为 blocked；001 还记录 `hat_channel_empty_after_activation`。 | 84 |
| Q2 | 基座机制是否生效？ | ⚠️ | event、ledger、diagnostics、supervisor.db 均落盘；但 isolated channel 空激活回退，stall detector 仍在尾段提前关停。 | 86 |
| Q3 | 编排是否合理、正常运行？ | ❌（尾段） | 001 在 stabilization 后直接 report；002 在四维 review 后仍缺 align.done，随后 `task.resume`/blocked；报告明确 alignment 前 stall。 | 90 |
| Q4 | 归因是什么？ | **preset/schema SSOT 漂移为直接根因，stall detector 是下游保护机制** | schema 为四个 review done 事件声明了共享 required fields，但四个 `ralph emit` 示例均未要求 agent 发送这些字段；001 表现为空 channel，002 直接出现 missing-field lint。 | 95 |

### 1.3 根因一句话

 `presets/en/ce-executor-pipeline.yml` 的 `event_policy.schemas` 要求四个 review done 事件携带共享 required fields，但对应 instructions 中的四个 `ralph emit` 示例没有这些字段。agent 按示例发送时，policy-check 拒绝或无法写入有效 JSONL：001 表现为激活后的空 hat channel，002 已直接留下 `lint:missing_field` 证据。随后 stall detector 在连续 3 次无进展后发出 `plan.blocked`。因此 stall detector 不是应该被绕过的根因，而是把前面的 SSOT 漂移暴露成阻断终态的下游机制。

### 1.4 终态时序一致性

| 项目 | 001 | 002 |
|---|---|---|
| 首轮终态 | 失败/阻断：reporter `verdict=blocked` | 失败/阻断：reporter `verdict=blocked` |
| 恢复状态 | 无后续 accepted 成功事件 | 无后续 accepted 成功事件；虽有后续 diagnostics session，不改变 trusted events verdict |
| 最终代码状态 | stabilizer commit `d5ed1203` 已存在；不能据此改写 blocked | U1–U4 提交至 `2a41e525` 已存在；不能据此改写 blocked |
| 一致性告警 | ⚠️ 失败终态后存在提交/验证 artifact，但无对应 accepted 成功终态 | ⚠️ 业务验证大体通过，但缺 `align.done`，最终仍是 blocked |

## 2. 执行链路对比

```text
work.start → plan.ready → executor work.done.proposed → precheck work.done
→ test-stabilizer stabilization.done → [review/alignment 尾段]
→ reporter report.done(blocked) → LOOP_COMPLETE

001: 尾段触发 channel fallback → 3 turns no progress → blocked
002: 四维 review 已完成，但 alignment 未投递 → task.resume → stall → blocked
```

## 3. 历史问题上下文

`N/A (history disabled)`

## 4. 证据清单

| ID | 描述 | 证据锚点 | 初判 | 置信度 |
|---|---|---|---|---:|
| DEV-001 | isolated hat-channel 空激活并回退主事件流 | 001 `.ralph/diagnostics/channel-routing-fallback-2026-08-02T13-39-01.md`；同目录 ralph log ERROR | P1 | 90 |
| DEV-002 | 无进展阈值提前触发 blocked | 001 ralph log：`no progress for 3 turns ... emitting plan.blocked`；002 report：`stall-detector ... before alignment` | P1 | 86 |
| DEV-003 | reporter 终态与业务完成证据不一致 | 两次 trusted events 的 `report.done.verdict=blocked`；001/002 report.md | P2 | 82 |
| DEV-004 | 001 stabilizer 验证记录自相矛盾 | 001 events 的 `stabilization.done` 为 `tests_passed=0/tests_run=0`，但 final-verification 列出 targeted checks；缺新的 accepted 验证事件 | P2 | 78 |
| DEV-005 | instructions 的 emit payload 少于 schema required_fields | `presets/en/ce-executor-pipeline.yml` 四个 review done schema 与对应 instructions emit 示例逐项对账；002 的 `task.resume` 明确列出 missing fields | P0 | 95 |

### 4.1 OPAC 逐 hat 审计（FULL）

| Hat/阶段 | O | P | A | C | 结论 |
|---|---|---|---|---|---|
| executor/precheck | ✅ | ✅ | ✅ | ✅ | work.done payload 完整，accepted |
| test-stabilizer | ✅ | ✅ | ⚠️ | ⚠️ | 001 的 accepted stabilization payload 没有可用测试计数；002 为 75/76 |
| goal-alignment / review 尾段 | ⚠️ | ❌ | ⚠️ | ❌ | 001 channel 空回退；002 缺 align.done |
| reporter | ✅ | ✅ | ✅ | ❌ | 能生成报告并 LOOP_COMPLETE，但报告 verdict 为 blocked，不能视为成功闭环 |

## 5. 问题归因

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 | 历史关联 | 加深 |
|---|---|---|---:|---|---|---:|
| P0 | review emit instructions 与 event schema 的 required_fields 不一致，导致有效 JSONL 不产生 | preset/schema SSOT 漂移 | **95** | DEV-005；001 空 channel + fallback，002 `lint:missing_field` 均是同一漂移的运行时表现 | `N/A (history disabled)` | 1 |
| P1 | 空 channel/无有效推进后由 stall detector fail-close | mechanism（下游保护） | **95** | DEV-001 + DEV-002；stall detector 只负责在无进展时发出 blocked，不能修复上游 payload 漂移 | `N/A (history disabled)` | 1 |
| P2 | accepted stabilization/最终验证证据不一致，导致“代码完成”无法转换为 accepted 成功终态 | compound：producer evidence + terminal orchestration | **78** | DEV-003 + DEV-004；001 `tests_passed=0`，002 缺 align.done | `N/A (history disabled)` | 1 |

## 6. 修复建议

### 6.1 短期（operator workaround）

- 已修复：四个 reviewer 的 instructions emit 示例补齐 schema 声明的共享 required fields。
- 重新运行时，应确认四个 review done 事件均进入 trusted JSONL，并继续产生 alignment/reporter 终态事件。

### 6.2 中期（preset/schema/instructions）

- `ralph-preset-review` 已增加 instructions ↔ schema required-fields SSOT 对账，并将缺字段、字段漂移、错误上游引用定级为 `preset.instructions_schema_required_fields_drift` P0 / confidence 95。
- 该检查属于 preset-review 的审计规则；若需要机器在运行前硬拒绝，还应把同一对账下沉为可执行的 preset lint。
- reporter 输入必须引用 accepted transition 和最终验证 bundle；禁止仅凭 Git commit 或 mutable report 推断成功。

### 6.3 长期（机制）

- 修复 `prepare_hat_channel`/空 channel 的竞态或 crash/timeout 后恢复路径，并增加真实 runtime regression：激活、私有 channel、fallback、后续推进四步必须在 trusted events 中可重放。
- stall detector 应把“尾段 gate 已激活但 channel 为空”输出为带 target/activation 证据的 typed blocked reason，便于区分 agent 无输出、路由丢失和真正无进展。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| 空 channel 的直接触发是 hat crash、timeout、上下文耗尽还是准备竞态 | 48 | 未读取 agent stdout，当前 fallback diagnostic 只给出建议检查 | 已查 fallback diagnostic、logs、trusted events；未作机制细分结论 |
