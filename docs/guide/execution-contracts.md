# 执行契约（Execution Contracts）

执行契约在 `work.done` 事件进入总线之前验证代理的完成义务。当代理忘记 emit 或 emit 不完整的完成信号时，执行契约可以防止误报。

## 概述

执行契约在 `work.done` 事件进入总线之前验证三个方面：

1. **Payload 字段** — 必填字段是否存在
2. **任务状态** — 引用的任务是否已关闭
3. **Git 证据** — 是否有实质性的更改（非 trivial 步骤）

如果任何验证失败，事件将被拒绝，并注入指导以推动修正。

## 配置

```yaml
event_loop:
  execution_contracts:
    enabled: true
    rules:
      work.done:
        # 必填 payload 字段
        require_payload_fields: ["task_id", "task_key", "step"]

        # 任务验证
        require_task:
          id_field: "task_id"
          key_field: "task_key"
          loop_scoped: true
          allowed_terminal_statuses: ["closed"]
          auto_close_on_valid: false

        # Git 验证
        require_git_change:
          mode: "diff_or_commit"
          allow_empty_for_steps: ["trivial"]

        # 测试证据（未来功能）
        require_test_evidence:
          mode: "optional"
```

## 字段参考

### `require_payload_fields`

事件 payload 中必须存在的 JSON 字段名列表。

### `require_task`

| 字段 | 类型 | 默认值 | 描述 |
|------|------|---------|-------------|
| `id_field` | string | `"task_id"` | 包含任务 ID 的 JSON 字段 |
| `key_field` | string | `"task_key"` | 包含任务 key 的 JSON 字段 |
| `loop_scoped` | bool | `true` | 任务必须属于当前循环 |
| `allowed_terminal_statuses` | list | `["closed"]` | 有效的任务状态 |
| `auto_close_on_valid` | bool | `false` | 契约通过时自动关闭任务 |

### `require_git_change`

| 字段 | 类型 | 默认值 | 描述 |
|------|------|---------|-------------|
| `mode` | string | `"diff_or_commit"` | Git 证据模式 |
| `allow_empty_for_steps` | list | `[]` | 不需要 git 证据的步骤 |

**模式：**
- `diff_or_commit`：如果 `git diff` 或 `git log` 显示有更改，则接受
- `diff_only`：**尚未实现**，当前行为等同于 `diff_or_commit`
- `commit_only`：**尚未实现**，当前行为等同于 `diff_or_commit`

### `require_test_evidence`

| 字段 | 类型 | 默认值 | 描述 |
|------|------|---------|-------------|
| `mode` | string | `"optional"` | 证据要求级别 |

**模式：**
- `optional`：不需要测试证据
- `required_payload_field`：检查 payload 中的 `tests` 字段（未来功能）

## 拒绝行为

当契约被拒绝时：

1. 原始事件**不会**发布到总线
2. 诊断事件发布到 `event.execution_contract.rejected`
3. 指导发布到 `plan.blocked`（2026-06-28-005 之前是 `human.guidance`，已废弃）
4. 下游 hat **不会**收到该事件

## ce-executor-pipeline 示例

`ce-executor-pipeline` 预设使用执行契约来保护 executor：

```yaml
event_loop:
  execution_contracts:
    enabled: true
    rules:
      work.done:
        require_payload_fields: ["plan_name", "plan_path", "task_id", "task_key", "step"]
        require_task:
          id_field: "task_id"
          key_field: "task_key"
          loop_scoped: true
          allowed_terminal_statuses: ["closed"]
        require_git_change:
          mode: "diff_or_commit"
          allow_empty_for_steps: ["trivial"]
```

这可以防止：
- 当 executor 忘记 emit 时的虚假 `work.done`
- 带有未完成任务（未关闭）的 `work.done`
- 没有实质性 git 更改的 `work.done`

**注意：** 当前 git 证据验证检查工作目录中是否存在未提交的更改或自循环开始以来的新提交。如果工作目录不是 git 仓库，git 证据检查将被跳过（视为不适用）。

## 诊断

契约拒绝会被记录并可见于：

1. **控制台警告**：以 `warn!` 级别记录，包含 topic、hat 和违反原因
2. **结构化诊断事件**：当 `RALPH_DIAGNOSTICS=1` 时，通过 `DiagnosticsCollector::log_execution_contract_rejections` 写入 `.ralph/diagnostics/<session>/orchestration.jsonl`（事件类型为 `ExecutionContractRejected`）
3. **TUI / RPC 可见性**:TUI 通过 EventBus observer 消费上述 `event.execution_contract.rejected` 事件(`human.guidance` 已废弃 — plan 2026-06-28-005)
4. **人工指导**:发布到 `plan.blocked` 供下一次迭代参考(`human.guidance` 已废弃)

## 测试

运行执行契约测试：

```bash
cargo nextest run -p ralph-core -- execution_contract
cargo nextest run -p ralph-core --test scenarios -- execution_contract
```
