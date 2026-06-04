---
title: "Agent Execution Contract Gates"
date: 2026-06-03
module: execution-contracts
tags: [ce-executor, execution-contract, hard-gate, preset]
problem_type: missing-event-detection
---

# Agent 执行契约门控

## 问题描述

### 事故现场

`ce-executor` 的 executor 被成功调度并写了代码和 scratchpad，但 events JSONL 中没有真实 `work.done` 或 `work.failed`。旧 embedded preset 的 executor 配置了 `default_publishes: "work.done"`，因此 Ralph 在 agent 没写事件时向内存 bus 注入了 `work.done`，导致 UI/后续状态像是 work pass，而 JSONL 没有真实完成事件。

### 根本原因

暴露三个独立缺口：

1. **忘操作不可见**：agent 没执行 `ralph emit` 时，当前 hard gate 只在输出文本出现 `ralph emit` 才触发；完全忘记 emit 不会被 gate
2. **默认事件语义过强**：`default_publishes` 本是兜底机制，但放在 executor 上会把"未声明结果"变成"成功完成"，掩盖真实原因
3. **完成声明未经验收**：即使 agent 发了 `work.done`，Ralph 也没有在进入 review 前核验 task、git 状态

## 解决方案

### U1-U5: 核心能力

- **Emit Obligation Gate v2**: 当 hat 有发布义务但本轮无事件时触发 hard gate，不依赖"口嗨 emit"检测
- **Execution Contract 配置模型**: 新增 `ExecutionContractRule` 配置结构
- **Work Done Contract Validator**: 验证 payload 字段、task 状态、git 证据
- **Event Loop 接入**: 在事件 publish 到 bus 前应用 contract validation

### U6-U7: 可观测性与启用

- **Loop Runner 诊断**: contract rejection 时记录 warn!，写入 diagnostics
- **ce-executor Contract 启用**: 显式启用 `work.done` execution contract

### U8-U9: 测试与文档

- **轻量回放测试（Replay-Light）**: 已覆盖 contract disabled pass-through 和 payload missing field rejection；尚未覆盖 open/closed task 的完整 runtime 路径、git evidence 拒绝路径、以及 contract rejection 与 missing-event gate 的交互。计划在后续修复中补全。
- **文档**: 明确 `default_publishes` 适用边界

## 配置变更

### ce-executor 移除 default_publishes

```yaml
# U2: executor 不再使用 default_publishes
hats:
  executor:
    publishes: ["work.done", "work.failed"]
    # default_publishes 已移除
```

### ce-executor 启用 execution contract

```yaml
event_loop:
  execution_contracts:
    enabled: true
    rules:
      work.done:
        require_payload_fields: ["plan_name", "plan_path", "task_id", "task_key", "step"]
        require_task:
          loop_scoped: true
          allowed_terminal_statuses: ["closed"]
        require_git_change:
          mode: "diff_or_commit"
          allow_empty_for_steps: ["trivial"]
```

## 经验教训

### 什么时候不用 default_publishes

| Hat 类型 | 示例 | 应该 |
|----------|------|------|
| 实施型 | executor, implementer | 无 default + execution contract |
| Gate 型 | plan-gate, fixer | fail-closed default |
| Report 型 | reporter | 谨慎使用 + 防御性检查 |

### 什么时候用 execution contract

- 实施型 hat 的完成声明必须被验证
- 下游 hat 的触发依赖于完成声明的真实性
- 需要防止"假成功"进入后续流程

### Contract rejection 的恢复路由（2026-06-04 plan 补充）

执行契约拒绝业务事件是 backpressure 行为，被拒绝的事件**绝不能**触发下游 hat。
但恢复路径必须满足：

1. **拒绝事件不进 bus**：原始事件被丢弃，下游订阅者不会激活（review 链不验证未通过的完成声明）
2. **拒绝必须附带 targeted retry**：发布一个 `task.resume` 事件，`target=原发 hat`，把"修复并重发"的责任交还给原发者
3. **不能只靠 `human.guidance`**：`human.guidance` 在 prompt 构建时被从 regular events 隔离，不参与 active hat 选择；只发 guidance 会让 Ralph coordinator 截胡
4. **fail closed on no safe target**：如果原发 hat 不在 registry、不能发布原 topic 或 `work.failed`，或 source hat 被伪造为 `ralph`，则不发布 targeted retry，只保留 diagnostic + guidance

详细实施见 `docs/plans/2026-06-04-001-fix-contract-rejection-hat-retry-plan.md`。

### Contract field 一致性（2026-06-04 plan 补充）

`execution_contracts.rules.<topic>.require_payload_fields`、`event_policy.schemas.<topic>.required_fields`、hat instructions 中的字段列表、consumer hat 的 read-state 必须保持**同一字段集合**。字段漂移会让一个 payload 通过 contract 但被 schema 拒绝，反之亦然，使契约层失去"假成功拦截"的作用。ce-executor 的 work.done 必需字段：`plan_name, plan_path, task_id, task_key, step`（5 个）。

### `d7ef7cc` 与本计划的归因区分

- `d7ef7cc test(hat): 为 ce-executor 预设固化 hat 路由契约测试` **只是 registry 路由的回归固化测试**，不是 ce-executor 现场塌缩的修复。
- 现场塌缩的真正机制修复见 `docs/plans/2026-06-04-001-fix-contract-rejection-hat-retry-plan.md`（U1-U8）。它的修复对象是 event loop 的 rejection recovery 路径，不是 `HatRegistry::get_for_topic`。
- 早期诊断（`docs/report/2026-06-04-ce-executor-worktree-prod-audit.md`）将 Ralph fallback 误判为 registry bug；该归因已在 2026-06-04 plan U8 中修正。

## Preset 作者检查清单

- [ ] 这个 hat 忘 emit 时，默认事件是否会造成假成功？
- [ ] 这个 topic 进入下游前，有没有 Ralph-owned 验收？
- [ ] 完成状态是否能从 task/git/test 证据重建？
- [ ] contract rejected 业务事件时，是否有 targeted retry 回到原发 hat？（2026-06-04 补充）
- [ ] 多个字段校验层（contract / schema / instructions / read-state）是否保持同一字段集合？（2026-06-04 补充）
- [ ] failure topic（`work.failed`）是否有明确订阅者？孤儿化会让兜底机制失效。（2026-06-04 U5 补充）
