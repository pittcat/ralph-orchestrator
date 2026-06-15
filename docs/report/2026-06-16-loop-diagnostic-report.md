# Ralph Loop 链路诊断报告

**Loop ID**: 2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-fresh-cedar
**Preset**: ce-executor-isolated
**诊断时间**: 2026-06-16
**工作目录**: `/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-fresh-cedar`

---

## 结论摘要

**Loop 状态：STALLED（卡死）** — coordinator hat 无法完成初始化，loop 无法进入主执行流程。

| 指标 | 状态 | 说明 |
|------|------|------|
| Loop 运行时间 | ~11 分钟 | 2026-06-15 23:40 启动，00:00 仍停滞在 iteration 1 |
| 核心事件流 | ❌ 未启动 | `work.ready` 从未发出 |
| Coordinator 激活 | ⚠️ 阻塞 | 试图发布非法事件被 isolated scope guard 拒绝 |
| 预期完成事件 | ❌ 未到达 | `REVIEW_COMPLETE` / `report.done` / `LOOP_COMPLETE` 均未发出 |

---

## 执行链路对比图

```
预期流程 (ce-executor-isolated.yml):                    实际状态:
─────────────────────────────────────────────────     ──────────────────────────────
work.start                                             ✅ 收到 (warmup phase)
    ↓                                                  ❌ coordinator 阻塞
coordinator → work.ready                               ⚠️ payload_contract_violation
    ↓                                                     (JSON 解析失败)
executor → work.done                                   ❌ 未到达
    ↓
review-coordinator → review.wave.ready / review.passed  ❌ 未到达
    ↓
dimension-reviewer × N → review.dimension.done          ❌ 未到达
    ↓
review-synthesizer → review.passed/failed/complete     ❌ 未到达
    ↓
fixer → fix.applied / fix.exhausted                    ❌ 未到达
    ↓
debug-resolver → fix.plan.ready / debug.exhausted      ❌ 未到达
    ↓
plan-gate → queue.advance / plan.complete              ❌ 未到达
    ↓
shipper → REVIEW_COMPLETE                             ❌ 未到达
    ↓
reporter → report.done / LOOP_COMPLETE                ❌ 未到达
```

---

## 证据清单

### 1. 事件文件 (.ralph/events-20260615-154034.jsonl)

```
3 行记录，均为 coordinator 尝试发布非法事件:
{"hat":"coordinator","topic":"build.done",...}   ← isolated scope violation
{"hat":"coordinator","topic":"debug.step",...}  ← isolated scope violation
{"hat":"coordinator","topic":"debug.step",...}  ← isolated scope violation
```

### 2. Recovery 诊断 (recovery.jsonl)

```json
// 关键错误 1: executor 发布 work.ready 失败
{
  "source": "cli_emit",
  "source_hat": "executor",
  "topic": "work.ready",
  "reason_code": "payload_contract_violation",
  "message": "Payload is not valid JSON: expected value at line 1 column 1"
}

// 关键错误 2: coordinator 发布 work.ready 失败
{
  "source": "cli_emit",
  "source_hat": "coordinator",
  "topic": "work.ready",
  "reason_code": "payload_contract_violation",
  "message": "Payload is not valid JSON: expected value at line 1 column 1"
}

// 关键错误 3: isolated scope violation
{
  "source": "workflow_guard",
  "source_hat": "coordinator",
  "topic": "build.done",
  "reason_code": "isolated_scope_violation",
  "message": "isolated mode: hat 'coordinator' cannot publish topic 'build.done'"
}

// 关键错误 4: isolated scope violation
{
  "source": "workflow_guard",
  "source_hat": "coordinator",
  "topic": "debug.step",
  "reason_code": "isolated_scope_violation",
  "message": "isolated mode: hat 'coordinator' cannot publish topic 'debug.step'"
}
```

### 3. Active Activations (active-activations.json)

```json
[
  {
    "hat_id": "coordinator",
    "iteration": 1,
    "duration": "652.8s"
  },
  {
    "hat_id": "ralph",
    "iteration": 1,
    "duration": "652.8s"
  }
]
```

- 两个 hat 都停留在 **iteration 1**
- coordinator 持续时间 652 秒（~11 分钟）无进展

### 4. Plan 文件状态

```
docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md  ← 存在 (162KB)
PROMPT.md: "Implement dev plan:docs/plans/2026-06-10-003..."                                   ← 指向 plan
```

### 5. Task 文件内容异常

`task.md` 包含的是**调试日志规范**而非 plan 内容，表明 agent 被 human guidance 或其他上下文干扰。

### 6. Scratchpad 内容

`.ralph/agent/scratchpad.md` 包含 human guidance 而非执行状态：

```
### HUMAN GUIDANCE (2026-06-15 15:43:11 UTC)
Focus on error handling

### HUMAN GUIDANCE (2026-06-15 15:43:11 UTC)
Keep this in mind
```

---

## 问题归因表

| # | 问题 | 归因 | 严重性 | 证据 |
|---|------|------|--------|------|
| P0-1 | `work.ready` 发布失败：`Payload is not valid JSON` | **Agent 执行问题**：coordinator/executor 尝试发布无效 JSON payload | P0 | recovery.jsonl: `payload_contract_violation` |
| P0-2 | coordinator 发布 `build.done` 和 `debug.step` 被 isolated scope guard 拒绝 | **Preset 设计问题**：coordinator 的 `publishes` 列表未声明这些 topic，且 isolated mode 不允许未声明 topic | P0 | recovery.jsonl: `isolated_scope_violation` |
| P0-3 | Loop 卡死在 iteration 1，无法推进到 executor | **多重叠加**：P0-1 导致 `work.ready` 从未发出，P0-2 导致 coordinator 无法完成初始化 | P0 | active-activations.json: `iteration: 1, duration: 652s` |
| P1-1 | `task.md` 内容异常（调试日志规范而非 plan 内容） | **Agent 上下文问题**：human guidance 或其他干扰导致 agent 未读取正确文件 | P1 | task.md 内容与预期不符 |
| P1-2 | Human guidance 干扰核心流程 | **Human Guidance 策略问题**：`Focus on error handling` 指令可能导致了 scope drop（发布调试事件） | P1 | scratchpad.md + debug_log.txt |
| P2-1 | `debug.step` 事件不在 preset schema 中 | **Preset 完整性问题**：非标准事件应该被 schema 覆盖或被 topic_deny_rules 拒绝 | P2 | 事件不在 ce-executor-isolated.yml 的任何 schema 定义中 |

---

## 修复建议

### 针对 P0-1: work.ready payload 解析失败

**问题**：coordinator/executor 尝试发布 `work.ready` 时，CLI 收到了无效的 JSON payload。

**建议**：
1. 检查 coordinator hat 的 `work.ready` 发布逻辑，确保 JSON payload 格式正确
2. 在 preset 中添加示例 payload（已有 copy-pasteable 示例，但需验证 agent 是否正确使用）
3. 在 `event_policy.schemas.work.ready` 中添加更严格的 JSON 格式校验

### 针对 P0-2: isolated scope violation

**问题**：`build.done` 和 `debug.step` 不在 coordinator 的 `publishes` 列表中。

**根因分析**：
- `build.done` 是早期 preset 的遗留事件名，ce-executor-isolated 已改用 `work.done`
- `debug.step` 是调试用的非标准事件，不应该出现在 production preset 中

**建议**：
1. **Preset 修复**：在 `ce-executor-isolated.yml` 的 `topic_deny_rules` 中添加：

```yaml
topic_deny_rules:
  - {hat_id: coordinator, topic: build.done}
  - {hat_id: coordinator, topic: debug.step}
  - {hat_id: coordinator, topic: debug.*}  # 通配符拒绝所有调试事件
```

2. **代码修复**：在 `workflow_guard` 中对 isolated mode 添加更清晰的错误消息：

```
提示：coordinator 应该发布 work.ready 或 work.failed，而非 build.done
```

### 针对 P1-1: task.md 内容异常

**问题**：agent 读取的是调试日志规范而非 plan 内容。

**建议**：
1. 验证 `work.start` payload 中的 prompt 解析逻辑
2. 在 coordinator hat 的 `work.start` 触发后，首先验证 plan 文件存在且可读
3. 添加预检查：如果 `task.md` 不包含预期的 plan 结构，拒绝执行

### 针对 P1-2: Human Guidance 策略

**问题**：`Focus on error handling` 导致 agent 发布了调试事件。

**建议**：
1. 在 preset instructions 中添加硬规则：**禁止发布调试事件**（`build.done`, `debug.step` 等）
2. Human guidance 应该聚焦于**流程层面**而非**实现细节**
3. 考虑在 isolated mode 下对 human guidance 进行 scope 过滤

---

## 根本原因总结

```
┌─────────────────────────────────────────────────────────────────┐
│                        根 因 链                                  │
├─────────────────────────────────────────────────────────────────┤
│ 1. coordinator 尝试发布 build.done / debug.step                  │
│    ↓                                                            │
│ 2. isolated scope guard 拒绝 → coordinator 无法完成初始化        │
│    ↓                                                            │
│ 3. work.ready 从未发出 → executor 未激活                        │
│    ↓                                                            │
│ 4. 整个事件流卡死在 iteration 1                                 │
│                                                                 │
│ 触发因素：Human guidance "Focus on error handling" 诱导 agent   │
│          发布调试事件，而非执行 plan                             │
└─────────────────────────────────────────────────────────────────┘
```

**最直接的修复**：
1. 在 `topic_deny_rules` 中添加 `build.done` 和 `debug.step` 拒绝规则
2. 重启 loop，确保 human guidance 不包含调试指令
3. 验证 plan 文件被正确读取

---

## 附录：关键文件位置

| 文件 | 路径 |
|------|------|
| 事件文件 | `.ralph/events-20260615-154034.jsonl` |
| Recovery 诊断 | `.ralph/recovery.jsonl` |
| Active Activations | `.ralph/diagnostics/2026-06-15T23-40-33/active-activations.json` |
| Loop 配置 | `.ralph/loops.json` |
| Plan 文件 | `docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md` |
| Preset 定义 | `presets/en/ce-executor-isolated.yml` |
