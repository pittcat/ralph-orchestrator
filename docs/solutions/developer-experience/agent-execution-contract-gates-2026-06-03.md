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

## Preset 作者检查清单

- [ ] 这个 hat 忘 emit 时，默认事件是否会造成假成功？
- [ ] 这个 topic 进入下游前，有没有 Ralph-owned 验收？
- [ ] 完成状态是否能从 task/git/test 证据重建？
