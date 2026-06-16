# Plan-Gate Dual-Publish 阻塞诊断报告

**日期**: 2026-06-15
**问题**: Isolated mode 的「每轮仅一个 business event」规则导致 `plan-gate` 的双重发布（`queue.advance` + `work.ready`）中 `work.ready` 被丢弃，造成 executor 无法收到下一 step 的启动信号，最终 loop 因 stale 检测终止
**Loop ID**: `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-lucky-peacock`
**Events 文件**: `.worktrees/.../.ralph/events-20260615-123500.jsonl`
**Status**: `Failed: stale loop detected`（summary.md:3）

---

## 结论摘要

**确认问题存在**：plan-gate 已按 Path A 修复方案正确双重发布了 `queue.advance` + `work.ready`（两个事件均写入 events.jsonl），但 isolated mode 的 `process_events_from_jsonl` 在 `mod.rs:5703-5714` 的每轮单 business event 预算规则将 `work.ready` 丢弃。

**根因链路**：
```
plan-gate 在同一 turn 内
  → queue.advance (business event #1)  ✓ 被接受
  → work.ready     (business event #2)  ✗ 被 "extra business event dropped" 规则丢弃
  → executor 收不到 work.ready，无法启动 step-02
  → 下一 turn plan-gate 重复相同模式
  → 3 次重复后触发 stale loop 检测 (mod.rs:1793)
  → loop.terminate (loop_stale)
```

**性质**：Orchestrator 基础设施缺陷（isolated mode 的每轮事件预算与 plan-gate 双重发布拓扑不兼容）。

---

## 1. 事件序列分析

### 1.1 终止前关键事件（events-20260615-123500.jsonl）

| 行号 | 时间 (UTC) | Hat | Topic | 内容 |
|:---:|:---|:---|:---|:---|
| #17 | 12:57:44 | review-coordinator | `review.passed` | empty_diff, verdict="pass", fix_round=1 |
| #18 | 13:01:20 | executor | `work.failed` | "Step-01 completed, awaiting plan-gate to advance" |
| **#19** | **13:03:21** | **plan-gate** | **`queue.advance`** | **step-01→step-02, 首次尝试** |
| **#20** | **13:03:37** | **plan-gate** | **`work.ready`** | **task_id="task-placeholder-step-02"（非真实 task）** |
| **#21** | **13:04:32** | **plan-gate** | **`queue.advance`** | **重复第 2 次，相同 pattern** |
| **#22** | **13:04:34** | **plan-gate** | **`work.ready`** | **task_id="task-u2-placeholder"（仍非真实 task）** |
| **#23** | **13:05:25** | **plan-gate** | **`queue.advance`** | **重复第 3 次 → 触发 loop_stale** |
| #24 | 13:06:04 | plan-gate | `work.ready` | task_id="task-1781528761-2a6e"（真实 task，已太迟） |

### 1.2 Recovery 记录（recovery.jsonl）

| 行号 | 时间 | Source Hat | Topic | 错误 |
|:---:|:---|:---|:---|:---|
| #2 | 13:03:29 | plan-gate | `work.ready` | `missing_required_field: task_id`（task-placeholder-step-02 被拒绝） |
| #3 | 13:05:27 | plan-gate | `work.ready` | `missing_required_field: task_id`（task-u2-placeholder 被拒绝） |

### 1.3 终止状态

- **summary.md**: `Status: Failed: stale loop detected`
- **Iterations**: 8
- **Duration**: 31m 16s
- **Final commit**: `e070d5d`（U1 scaffold 完成）
- **未完成步骤**: step-02 ~ step-08（共 7 步未执行）

---

## 2. 根因分析

### 2.1 核心冲突

`ce-executor-isolated` preset 配置了 plan-gate 的 `publishes: ["queue.advance", "work.ready", "plan.complete", "plan.blocked"]`（Path A 修复），使 plan-gate 能在 `queue.advance` 后继发 `work.ready` 以桥接 executor。

但 isolated mode 的事件处理逻辑 `crates/ralph-core/src/event_loop/mod.rs:5287-5736` 在 `execution_mode: isolated` 下强制执行**每轮最多一个 business event**的预算规则：

```rust
// mod.rs:5703-5714
if first_business_event_accepted && !same_wave_continuation {
    warn!(
        topic = %event.topic,
        "Isolated mode: extra business event dropped — only one per turn"
    );
    let diagnostic = Event::new(
        "event.isolation.boundary_violation",
        format!(
            "Isolated mode: dropped extra event '{}' — only one business event per turn allowed",
            event.topic
        ),
    );
    self.bus.publish(diagnostic);
}
```

当 plan-gate 在同一 turn 内先后 emit `queue.advance`（business event #1）和 `work.ready`（business event #2）时，`work.ready` 被此规则丢弃。结果是：

- `queue.advance` 成功进入 event bus，但它的 trigger 是 `executor.triggers` — executor 没有被激活（executor 只触发 `work.ready`、`queue.advance`、`work.retry`、`fix.plan.ready`，但无法以仅接收 `queue.advance` 的状态启动）
- `work.ready`（应激活 executor 的信号）被丢弃，executor 永远不会被调度

### 2.2 为什么 queue.advance 不足以启动 executor

查看 preset 的 executor 配置：
```yaml
executor:
  triggers: ["work.ready", "queue.advance", "work.retry", "fix.plan.ready"]
  publishes: ["work.done", "work.failed"]
```

executor 的 `triggers` 包含 `queue.advance`，但 executor 的 `publishes` 只有 `work.done`/`work.failed` — executor 在收到 `queue.advance` 后**没有合法的 business topic 可以 emit**，这是一种「dead-end trigger」。executor 需要 `work.ready` 作为启动信号，因为 `work.ready` 提供完整的执行上下文（`plan_name`, `plan_path`, `task_id`, `task_key`, `step`, `complexity`）。

### 2.3 Stale loop 触发过程

```
Turn 1 (13:03:21-13:03:37):
  plan-gate 读取 events（review.passed + work.failed）
  → emit queue.advance ✓（business event #1）
  → emit work.ready    ✗（被 isolated budget 丢弃）
  → executor.pending 为空，loop 无进展

Turn 2 (13:04:32-13:04:34):
  plan-gate 被 ralph 重新触发
  → emit queue.advance ✓（相同 signature）
  → emit work.ready    ✗（仍被丢弃）
  → 仍无进展

Turn 3 (13:05:25):
  plan-gate 再次 emit queue.advance
  → consecutive_same_signature >= 3（mod.rs:1793）
  → TerminationReason::LoopStale
  → loop.terminate (13:06:17)
```

### 2.4 附带问题：work.ready 的 task_id 错误

`recovery.jsonl` 记录了两次 `missing_required_field: task_id` 错误（#2: 13:03:29, #3: 13:05:27）。这是因为 plan-gate 在 `work.ready` 中使用了占位符 task_id：`"task-placeholder-step-02"` 和 `"task-u2-placeholder"`，而非 task store 中注册的真实 task ID。直到第 3 次（事件 #24），task_id 才变为真实的 `"task-1781528761-2a6e"`，但此时 loop 已被 stale 检测终止。

这意味着即使 isolated mode 的预算问题解决，`work.ready` 仍可能被 execution contract 因 `task_id` 无效而拒绝。

---

## 3. 代码路径定位

| 代码位置 | 功能 | 角色 |
|:---|:---|:---|
| `event_loop/mod.rs:5287-5736` | `process_events_from_jsonl` isolated 分支 | 裁决事件接受/丢弃 |
| `event_loop/mod.rs:5703-5714` | "extra business event dropped" | **根因触发点** |
| `event_loop/mod.rs:5696-5701` | `same_wave_continuation` 检查 | wave 延续例外（不适用于 plan-gate） |
| `event_loop/mod.rs:1792-1806` | Stale loop 检测（`consecutive_same_signature >= 3`） | 终止判定 |
| `loop_state.rs:510-526` | `record_event()` — 更新`consecutive_same_signature` | 状态跟踪 |
| `event_loop/mod.rs:4520-4548` | `check_default_publishes` Gate 2 — per-turn 预算 | 补充预算检查 |
| `presets/en/ce-executor-isolated.yml:239-245` | `queue.advance` schema | 事件 schema |
| `presets/en/ce-executor-isolated.yml:170-171` | `work.ready` schema | 事件 schema |

---

## 4. 与现有解决方案的关系

`docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` 中描述了 Path A（plan-gate 双发布 `work.ready`）并已在 preset 中实施。但该方案**未覆盖 isolated mode 基础设施层的冲突**：

| 方案 | 状态 | 效果 |
|:---|:---|:---|
| Path A: plan-gate 双发布 `work.ready` | **已实施** | plan-gate 正确 emit 了两个事件（events.jsonl 证实） |
| Isolated mode 每轮单 business event 预算 | **未修改** | 丢弃了 `work.ready`，使 Path A 失效 |
| 修复建议（本报告 §5） | **待实施** | 需要在 isolated mode 中允许 `queue.advance` + `work.ready` 组合 |

---

## 5. 修复建议

### 5.1 方案 A（推荐）：在 isolated mode 中添加 dual-publish 例外

在 `event_loop/mod.rs:5703` 的预算检查前，增加对 `(queue.advance, work.ready)` 组合的放行逻辑：

```rust
// 在 first_business_event_accepted 检查前添加:
let is_dual_publish_exception = {
    let prev_topic = accepted.last().map(|e| e.topic.as_str());
    prev_topic == Some("queue.advance") && event.topic == "work.ready"
};

if first_business_event_accepted && !same_wave_continuation && !is_dual_publish_exception {
    // ... 现有丢弃逻辑
}
```

**位置**: `crates/ralph-core/src/event_loop/mod.rs`，约 5703 行
**原理**: 在 isolated mode 的每轮预算中创建白名单事件对，`queue.advance + work.ready` 作为一对合法的连续 business event 放行。

### 5.2 方案 B：通过 preset 配置化的 dual-publish 规则

在 `event_loop` 配置中增加 `dual_publish_pairs` 字段，使 preset 可声明哪些事件对应作为同一轮的业务事件放行：

```yaml
event_loop:
  execution_mode: isolated
  dual_publish_pairs:
    - ["queue.advance", "work.ready"]
```

**位置**: `crates/ralph-core/src/config/` 和 `event_loop/mod.rs`
**优势**: 通用化方案，不限于 plan-gate，其他 hat 也可使用。

### 5.3 方案 C：合并 queue.advance + work.ready 为单一事件

将 `queue.advance` 和 `work.ready` 的 payload 合并，plan-gate 仅 emit 一个事件（如 `step.advance`），该事件同时携带 advance 信息和 work.ready 的完整执行上下文。但此方案涉及 preset 拓扑设计变更，影响范围较大。

### 5.4 短期附加修复：work.ready 的 task_id 有效性

plan-gate 在 emit `work.ready` 时必须使用 task store 中已注册的真实 task ID，而非占位符。这需要 plan-gate 在 emit 前调用 `ralph tools task ensure` 获取有效 task ID，或在预设 prompt 中强制要求 plan-gate 使用 task store 注册的 ID。

---

## 6. 证据清单

| 编号 | 证据 | 路径 |
|:---|:---|:---|
| E1 | events.jsonl 显示 plan-gate 成功 emit queue.advance + work.ready | `.worktrees/.../.ralph/events-20260615-123500.jsonl:19-24` |
| E2 | recovery.jsonl 显示 work.ready 被 `missing_required_field: task_id` 拒绝 | `.worktrees/.../.ralph/recovery.jsonl:2-3` |
| E3 | summary.md 确认 loop 因 stale 失败 | `.worktrees/.../.ralph/agent/summary.md:3` |
| E4 | 源码 isolated mode 预算丢弃逻辑 | `crates/ralph-core/src/event_loop/mod.rs:5703-5714` |
| E5 | 源码 stale loop 检测（≥3 同签名） | `crates/ralph-core/src/event_loop/mod.rs:1792-1806` |
| E6 | plan-gate publishes 包含 work.ready（Path A 已实施） | `presets/en/ce-executor-isolated.yml:61` |
| E7 | 解决方案文档确认 Path A 设计 | `docs/solutions/integration-issues/...dispatch-gap-...-2026-06-12.md` |
| E8 | executor 的 publishes 不含 work.ready（dead-end trigger） | `presets/en/ce-executor-isolated.yml` executor 配置 |