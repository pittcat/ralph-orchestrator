---
title: "ce-executor multi-step plan premature LOOP_COMPLETE due to missing plan-wide advancement gate"
date: 2026-06-02
category: developer-experience
module: ralph-cli
problem_type: design_pattern
component: presets
symptoms:
  - "Multi-step plan completes after first step review pass"
  - "LOOP_COMPLETE fires while plan.md still has pending steps"
  - "Executor never runs Step 2+ because task pool empties after Step 1"
  - "progress.md Completed Steps lags behind actual work"
root_cause: missing_gate
resolution_type: preset_topology_fix
severity: high
tags:
  - ce-executor
  - plan-gate
  - preset-topology
  - multi-step-plan
  - completion-gate
  - task-pool-vs-plan-wide
related_components:
  - ralph-cli
  - ralph-core
---

# ce-executor Plan-Wide Advancement Gate

## Problem

`ce-executor` 预设的事件链原本是单程链路：

```
work.start → coordinator → executor → review-coordinator → dimension-reviewer
  → review-synthesizer → Shipper → REVIEW_COMPLETE → Reporter → LOOP_COMPLETE
```

Coordinator 只为当前 step 创建 runtime tasks。当 Step 1 完成后，task 池为空，`ralph-core` 的 `verify_tasks_complete()` 按 task 池契约接受 `LOOP_COMPLETE`。结果：Step 2/3/4 从未被创建，循环提前终止。

这不是 `check_completion_event()` 的局部 bug，而是 `ce-executor` 预设缺少 plan-wide 推进语义。

## Symptoms

- 多步骤 plan 在第一步 review pass 后直接发布 `LOOP_COMPLETE`
- `plan.md` 中仍有 pending steps，但循环已终止
- `progress.md` 的 `Completed Steps` 为空或滞后
- Executor 的 Step Advancement 逻辑自行发布 `queue.advance`，但 review pass 后直接进入 shipper，绕过了继续判断

## Root Cause

**Step-scoped task 池 ≠ Plan-wide completion。**

- `ralph-core` 把 runtime task 池视为完成门控
- `ce-executor` 把 task 池当成当前 step 的局部队列
- 预设缺少一个专门负责「当前增量通过评审后，判断继续下一步还是全 plan 完成」的节点

## Solution

### 新增 `plan-gate` Hat

在 `review-synthesizer` 和 `shipper` 之间插入 `plan-gate`：

```
review.passed / review.complete → plan-gate
  ├─ queue.advance  → executor（继续下一步）
  ├─ plan.complete  → shipper（最终交付）
  └─ plan.blocked   → shipper（失败报告）
```

**plan-gate 职责**：
- 读取 `plan.md`、`progress.md`、runtime task 状态和当前 event payload
- 对账当前 step 完成状态（处理 `progress.md` 滞后）
- 还有后续 step → `queue.advance`
- 所有 step 完成 → `plan.complete`
- 状态不一致或 verdict fail → `plan.blocked`

**关键规则**：
- `plan-gate` **不得**监听 `fix.applied`（修复后必须先复审）
- `executor` **不再**自行发布 `queue.advance`
- `shipper` **只**从 `plan.complete` / `plan.blocked` / `fix.exhausted` 触发
- `reporter` 在发布 `LOOP_COMPLETE` 前做防御性 plan 完成检查

### 参考模式

`pdd-to-code-assist` 预设中的 `finalizer` 已采用类似模式：
- `review.passed` → `finalizer`
- `finalizer` 决定 `queue.advance`（还有任务）或 `implementation.ready`（全部完成）

## Why This Works

- 把 plan-wide 推进语义编码进预设拓扑，而不是依赖 core 的 task 池门控
- `plan-gate` 是单一职责节点：不实现、不 review、不 validate，只做「继续 vs 完成」路由
- 保留了 failure path：`plan.blocked` 和 `fix.exhausted` 仍能到达 shipper/reporter 生成失败报告
- 防御性检查：reporter 在 `LOOP_COMPLETE` 前再次核对 `plan.md` / `progress.md`

## Prevention

- 设计多步骤 preset 时，问：「当前增量完成后，谁决定继续还是结束？」
- 如果答案是「执行者自己决定」，通常是职责混淆——应该由独立 gate 节点决定
- 预设拓扑测试必须覆盖：review pass 后可达 `queue.advance`，而不是直接进入 completion promise
- 静态测试中增加 `executor.publishes` 不包含 `queue.advance` 的断言（如果 gate 独占推进）

## Related

- `docs/plans/2026-06-02-004-fix-ce-executor-plan-gate-plan.md` — 本修复的实施计划
- `docs/report/2026-06-02-ce-executor-loop-premature-termination-diagnosis.md` — 诊断报告
- `presets/ce-executor.yml` — 修复后的英文预设
- `presets/ce-executor-zh.yml` — 修复后的中文预设
- `presets/pdd-to-code-assist.yml` — 参考 `finalizer` 模式
