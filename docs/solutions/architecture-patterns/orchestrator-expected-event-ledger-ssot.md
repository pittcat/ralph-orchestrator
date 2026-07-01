---
title: "Orchestrator 用账本计算 expected_event，plan 文本只扫一次缓存"
date: 2026-07-01
category: architecture-patterns
module: ralph-core
problem_type: architecture_pattern
component: development_workflow
severity: high
applies_when:
  - "多 hat preset（尤其 ce-executor-serial）在 test.passed 后 coordinator 走错分支、重发 work.ready 或漏发 plan.complete"
  - "agent 靠读 plan.md 散文或数 ### U{N}. 标题推断下一阶段"
  - "需要换项目复用同一 preset，而不 per-repo 写协调逻辑"
tags:
  - orchestrator-state
  - expected-event
  - ce-executor-serial
  - ledger-ssot
  - plan-topology-cache
  - test-passed
  - multi-hat-isolation
---

# Orchestrator 用账本计算 expected_event，plan 文本只扫一次缓存

## Context

`ce-executor-serial` 在 `primary-20260630-175407` 等业务已闭环的 run 上仍出现 coordinator 乱发事件：fix-02 `test.passed` 后重发 `work.ready`、漏发 `plan.complete`，以及 `LOOP_COMPLETE` 后的终态二次风暴。

根因链**从 agent 乱发开始**，但 P0 修复不能指望 agent 不再犯错。当前 preset 让 coordinator **自己读 plan.md** 并「数 `### U{N}.` 标题」判断 `N_total`——这是 LLM 读散文，不是机器状态。

团队对齐的设计（已写入 `docs/plans/2026-07-01-001-fix-ce-executor-serial-p0-terminal-storm-plan.md` 的 U6/R7）：**唤醒 coordinator 之前**，由 Rust 引擎算出 `expected_event`，注入只读 `orchestrator_state`；plan 文本仅在边界时刻按固定约定扫描一次并缓存。

## Guidance

### 原则：状态来自账本，不是 agent 读 plan

| 优先级 | 输入 | 用途 |
|--------|------|------|
| 1 | 刚落地的 `test.passed` payload 中的 `step` | 触发信号（`step-02` / `fix-02`） |
| 2 | `flow_lifecycle`（`unit_loop` → `review_walk` → …） | 宏观阶段（review 是否已走完） |
| 3 | `tasks.jsonl`（fix/plan unit 数量与 terminal 状态） | fix 链是否 exhausted |
| 4 | loop 启动时缓存的 plan unit 列表，或账本中 `review.start.total_units` | 判断是否为最后一个 plan unit |
| 5 | fix-plan 扫描缓存或 fix task 计数 | 判断是否为最后一个 fix unit |
| 6 | `progress.md` 窄字段（`ProgressSnapshot`） | 辅助对账，**不作为** `expected_event` 唯一依据 |

**禁止**：每次 `test.passed` 让 LLM 重读 plan 全文或数标题。  
**禁止**：让 validator 在 `test.passed` 里写「下一步请 plan.complete」（仍是 agent 手写判断）。

### plan.md 是文本，怎么处理？

- **不**在每次 activation 解析 plan 散文。
- **loop 启动一次**：对 `plan.md` 扫描固定标题 `### U{N}.` → `["step-01", "step-02", …]`，写入 `LoopState.plan_topology`（与 `review_step_state::prefill_fix_steps_from_plan` 同款规则；fix-plan 在 `review.complete` 落盘后同样扫描 → `fix-{NN}`）。
- 扫描失败或标题数为 0 → **fail-closed**（诊断 `plan_topology_unparseable`），不 silent 猜 `N_total`。
- 换项目只换 plan 文件；**preset 与引擎代码不变**。

### 查表规则（与 `CoordinatorDecisionGateStage::topic_for_phase` 对齐）

- `test.passed(step-NN)` 且 NN < plan unit 总数 → `work.ready(step-{NN+1})`
- `test.passed(step-NN)` 且 NN == plan unit 总数 → `review.start`
- `test.passed(fix-NN)` 且 NN < fix unit 总数 → `work.ready(fix-{NN+1})`
- `test.passed(fix-NN)` 且 NN == fix unit 总数 → `plan.complete`（**不是** `review.start`）
- `LOOP_COMPLETE` 已 honor → 业务 hat 不得再 emit（U2 持久 `completion_honored`）

### 三层抑制乱发

1. **U6**：唤醒前注入 `orchestrator_state.expected_event`（指令卡）
2. **U3**：emit 时将末 fix-unit 的 `work.ready` 改写为 `plan.complete` 并补全 payload
3. **U1/U2**：isolated 预算终态优先 + 跨 batch 完成守卫拒绝

单独 U6 不够；单独指望 prompt 也不够。三层缺一不可。

### 实现落点（计划 U6）

- 新增 `compute_expected_event(trigger, loop_state) -> OrchestratorState`
- 注入点：`build_prompt` / `prepend_correction_and_resume`，**每次** coordinator activation（不仅 rejection 后的 `task.resume`）
- U5 删除 preset 中「Count every `### U{N}.` heading」类 LLM 步骤，改为「读取 `orchestrator_state.expected_event`」

## Why This Matters

若继续让 coordinator 从 plan 散文推断阶段：

- fresh context 下 LLM 易猜错（175407：fix-02 后三次 `work.ready`）
- isolated 预算会把正确的 `plan.complete` 静默丢弃（stray `work.ready` 占槽）
- `progress.md` 与内存快照不一致时，agent 会发明 narrative reason（如 `progress_md_validation_stale`）并 emit `plan.blocked`

引擎确定性查表 + 账本 SSOT，把「该发什么」从 agent 推断变成**可测试、可回放、跨项目复用**的机制。

## When to Apply

- 实现或审查 `ce-executor-serial` 的 coordinator 唤醒路径、`test.passed` 后续分支
- 新增多 hat preset 且存在「同一触发事件、不同阶段语义不同」（如 PHASE 1 `step-NN` vs PHASE 2 `fix-NN`）
- 讨论「上一个 hat 要不要给下一个 hat 传状态」——应改为 **orchestrator 统一注入**，不由上游 agent 手写 handoff

## Examples

### Before（agent 猜）

```
validator → test.passed(fix-02)
coordinator 读 progress + plan 散文 → 猜错 → work.ready(fix-02) ×3
isolated 预算丢弃 plan.complete → plan.blocked → 终态风暴
```

### After（引擎算 + 注入）

```
validator → test.passed(fix-02)   # 只报事实
引擎：fix-02 是缓存拓扑中最后一个 fix，review_walk 已关闭
  → orchestrator_state.expected_event = plan.complete
coordinator prompt 顶部可见指令卡 → emit plan.complete
若仍乱发 → U3 改写 / U1 预算 / U2 拒绝
```

### 换项目

```bash
cd /other/repo
ralph run -H builtin:ce-executor-serial --plan docs/plans/other-feature-plan.md
```

引擎读**新 plan** 的拓扑缓存 + **同一 preset** flow 规则；无需为新 repo 写协调代码。

## Related

- 实施计划：`docs/plans/2026-07-01-001-fix-ce-executor-serial-p0-terminal-storm-plan.md`（U6、R6、R7）
- 诊断：`docs/report/2026-07-01-ce-executor-serial-primary-20260630-175407-diagnosis.md`
- 查表 SSOT 代码：`crates/ralph-core/src/event_loop/stages/coordinator_decision_gate_stage.rs`（`PhaseClass` / `topic_for_phase`）
- fix-plan 标题扫描：`crates/ralph-core/src/event_loop/review_step_state.rs`（`prefill_fix_steps_from_plan`）
- 邻近机制修复：`docs/solutions/logic-errors/ce-executor-p0-event-policy-and-projector-fanout.md`
- 前置 fix-unit 终态计划：`docs/plans/2026-06-30-001-fix-ce-executor-serial-fix-unit-terminal-p0-plan.md`
