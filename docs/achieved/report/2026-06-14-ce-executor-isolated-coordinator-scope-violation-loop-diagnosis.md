# ce-executor-isolated Coordinator Scope Violation Loop Diagnosis

> 📅 2026-06-14 | 🔖 worktree: `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-clever-swan`

---

## 1. 总体结论

| 维度 | 状态 | 说明 |
|------|------|------|
| 任务完成 | 🔴 卡死 | coordinator 在 iteration 2 仍在循环，未进入 executor 阶段 |
| 问题定位 | 🟢 完成 | 根因已定位：coordinator 发布违规事件 + isolated 模式下 rejection 未释放 trigger |
| 修复方向 | 🟡 待确认 | 需验证 loop 基座行为 + 确认 coordinator prompt 是否误导 |

---

## 2. 问题描述

### 现象

运行 `ce-executor-isolated` preset 执行 plan `docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md` 时：

- **Iteration 1**：coordinator 激活，但 loop 停滞在 coordinator 阶段
- **Iteration 2**：coordinator 再次激活，形成无限循环
- **预期行为**：coordinator 应发布 `work.ready` 触发 executor，依次进入 review → fix → ship → report 链路

### 观察到的症状

```
Iteration 1:
  Hat: coordinator
  Event: build.done (违规发布)
  Result: isolated_scope_violation

Iteration 2:
  Hat: coordinator (再次激活)
  Event: build.done (同样违规)
  Result: 同上，循环
```

---

## 3. 根因分析

### 3.1 直接根因：coordinator 发布违规事件

从 `recovery.jsonl` iteration 1 条目：

```json
{
  "source_hat": "coordinator",
  "topic": "build.done",
  "reason_code": "isolated_scope_violation",
  "message": "isolated mode: hat 'coordinator' cannot publish topic 'build.done'"
}
```

**违规事实**：
- `ce-executor-isolated.yml` 第 274 行定义 coordinator 的 `publishes` 列表：
  ```yaml
  coordinator:
    publishes: ["work.ready", "work.failed"]
    # 注意：没有 build.done
  ```
- coordinator 实际发布了 `build.done`，不在 `publishes` 列表中
- isolated 模式下的 `workflow_guard` 正确拒绝了该事件

### 3.2 根本根因：coordinator 从未发布正确事件

检查 `events-20260614-130637.jsonl`：

```jsonl
{"hat":"coordinator","payload":{"ok":true},"topic":"build.done","ts":"..."}  // 2 行，都是 build.done
```

**关键发现**：
- 0 行 `work.ready`
- 0 行 `work.failed`
- coordinator 从未尝试发布正确的终态事件

这说明 coordinator 的行为与 preset 设计严重偏离。

### 3.3 循环根因：rejection 后 trigger 未释放

**为什么 iteration 2 还是 coordinator？**

```
触发链：
work.start (loop 启动) 
  → coordinator 激活
  → coordinator 发布 build.done (被拒绝)
  → coordinator 的 trigger "work.start" 未被满足
  → coordinator 仍处于 pending 状态
  → iteration 2 时 coordinator 再次被选中
```

问题在于：**当事件被 isolated scope violation 拒绝时，coordinator 的 trigger 事件 `work.start` 是否应该被标记为 consumed？**

当前行为：`work.start` 仍处于未决状态 → coordinator 持续 pending → 无限循环。

---

## 4. 执行链路对比

### 预期链路（ce-executor-isolated 标准流程）

```
work.start
  → [coordinator]
    → 创建 tasks (已确认：tasks.jsonl 有 1 个 open task)
    → 发布 work.ready ✓
  → [executor]
    → 发布 work.done ✓
  → [review-coordinator]
    → 发布 review.wave.ready 或 review.passed ✓
  → [dimension-reviewer × N]
    → 发布 review.dimension.done ✓
  → [review-synthesizer]
    → 发布 review.passed / review.failed / review.complete ✓
  → [plan-gate]
    → 发布 queue.advance / plan.complete / plan.blocked ✓
  → [shipper]
    → 发布 REVIEW_COMPLETE ✓
  → [reporter]
    → 发布 report.done + LOOP_COMPLETE ✓
```

### 实际链路（本次运行）

```
work.start
  → [coordinator]
    → ✗ 发布 build.done (isolated_scope_violation)
    → (未发布 work.ready 或 work.failed)
  → [coordinator] (再次激活)
    → ✗ 再次发布 build.done (同上)
    → (仍未发布正确事件)
  → 循环...
```

---

## 5. 证据清单

| 来源 | 文件 | 关键内容 |
|------|------|----------|
| Recovery 诊断 | `recovery.jsonl:2` | `isolated_scope_violation: coordinator → build.done` |
| Recovery 诊断 | `recovery.jsonl:3` | `outcome: escalated` (scope 违规不重试) |
| 事件历史 | `events-20260614-130637.jsonl` | 仅 2 个 `build.done` event，0 个 work.ready/work.failed |
| Preset 配置 | `ce-executor-isolated.yml:274` | coordinator publishes: `["work.ready", "work.failed"]` — 无 `build.done` |
| 任务状态 | `tasks.jsonl` | 有 1 个 open task（U2），说明 coordinator 确实创建了任务 |
| 上下文 | `context.md` | plan 路径确认、复杂度评估等 |

---

## 6. 问题归因表

| 层级 | 问题 | 严重度 | 证据 |
|------|------|--------|------|
| **Agent 实现** | coordinator 发布了不在 `publishes` 列表中的 `build.done` | **P0** | recovery.jsonl: `isolated_scope_violation` |
| **Agent 实现** | coordinator 从未尝试发布 `work.ready` 或 `work.failed` | **P0** | events.jsonl 中 0 行正确事件 |
| **Agent 实现** | coordinator 连续两次 emit 相同违规事件（未从拒绝中恢复） | **P1** | 2 个连续的 build.done |
| **Loop 基座** | isolated 模式下 rejected event 的 trigger 未被正确释放 | **P1** | coordinator 在 iteration 2 仍被选中 |

---

## 7. 修复建议

### 7.1 短期修复（确认 coordinator 行为）

1. **检查 coordinator 的 prompt 指令**
   - 确认是否有误导性指令导致发布 `build.done`
   - ce-executor-isolated.yml 第 277-403 行的 coordinator instructions 应明确说明只发布 `work.ready` / `work.failed`

2. **验证事件写入时序**
   - coordinator 是否在收到 `work.start` 后立即尝试发布 `build.done`？
   - 还是在某个中间步骤错误地发布了不该发布的事件？

### 7.2 中期修复（验证 loop 基座行为）

1. **验证 rejection 后 trigger 释放逻辑**
   ```rust
   // 预期行为：当事件被 workflow_guard 拒绝时，
   // coordinator 的 "work.start" trigger 应被标记为 satisfied
   // 这样 coordinator 不会再被选中

   // 当前行为：trigger 未释放，导致 coordinator 持续 pending
   ```

2. **检查 isolated 模式下的 workflow_guard 行为**
   - 当 `isolated_scope_violation` 发生时，trigger 是否被正确消费？
   - 是否需要显式地将 trigger 标记为 consumed？

### 7.3 长期改进

1. **添加 coordinator 行为验证测试**
   - 确保 coordinator 在 isolated 模式下只发布 `publishes` 列表中的事件
   - 添加 integration test 验证 rejection 后 trigger 释放

2. **增强 isolated 模式的可观测性**
   - 当发生 scope violation 时，明确告知 agent 哪些事件可以发布
   - 在 rejection envelope 中包含该 hat 的 `publishes` 列表

---

## 8. 待验证假设

| 假设 | 验证方法 | 预期结果 |
|------|----------|----------|
| coordinator prompt 有误导性指令 | 检查 ce-executor-isolated.yml coordinator instructions | 发现并修复 |
| rejection 后 trigger 未释放 | 检查 event_loop/mod.rs workflow_guard 逻辑 | 确认并修复 |
| isolated scope violation 应触发 task.resume | 检查 EventOriginGuard 代码 | 确认路由行为 |

---

## 9. 下一步

1. [ ] 检查 coordinator 的 prompt，确认为什么发布 `build.done`
2. [ ] 验证 loop 基座中 rejection 后 trigger 释放逻辑
3. [ ] 在 test 环境重现该问题
4. [ ] 实施修复并验证

---

## 附录：技术详情

### recovery.jsonl 完整内容

```json
// 条目 1: agent_doc_sync (正常)
{"schema_version":1,"envelope":{...},"iteration":0,"outcome":"recovered"}

// 条目 2: isolated_scope_violation (核心问题)
{
  "envelope": {
    "diagnosis_id": "c281275f-5f0d-4e7c-bfb2-d6b2c7f42d06",
    "iteration": 1,
    "source": "workflow_guard",
    "severity": "warning",
    "source_hat": "coordinator",
    "target_hat": "coordinator",
    "topic": "build.done",
    "reason_code": "isolated_scope_violation",
    "message": "isolated mode: hat 'coordinator' cannot publish topic 'build.done'",
    "outcome": "escalated"
  },
  "iteration": 1,
  "notes": ["scope_drop hat=coordinator topic=build.done current_isolated_hat=coordinator"]
}

// 条目 3: drift_monitor 更新
{
  "envelope": {
    "diagnosis_id": "c1e001e4-5f1d-431f-97fd-0d52d9c33300",
    "iteration": 1,
    "source": "drift_monitor",
    "topic": "build.done",
    "reason_code": "recovery_outcome_update",
    "outcome": "pending"
  },
  "iteration": 1,
  "notes": ["outcome updated to Pending"]
}
```

### 相关代码位置

| 文件 | 行号 | 说明 |
|------|------|------|
| `ce-executor-isolated.yml` | 269-403 | coordinator hat 定义与 instructions |
| `ce-executor-isolated.yml` | 148-155 | topic_deny_rules（但 build.done 不在 deny 列表） |
| `event_loop/workflow_guard.rs` | - | isolated scope 验证逻辑 |
| `event_loop/mod.rs` | - | trigger 状态管理 |
