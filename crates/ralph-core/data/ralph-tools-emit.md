---
name: ralph-tools-emit
description: 完整 ralph emit 参考，包含参数表、环境变量、事件文件解析优先级、反模式、校验步骤、错误恢复
metadata:
  internal: true
---

# ralph emit — 完整参考

> **NEVER use echo/cat to write tasks or memories** — always use CLI tools.

---

## `ralph emit`

向当前运行的事件文件发射一个结构化事件。这是 agent 与编排器通信的核心机制。

**语法：**
```bash
ralph emit [OPTIONS] <TOPIC> [PAYLOAD]
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<TOPIC>` | string | 是 | — | 事件主题，如 `build.done`、`review.complete` |
| `[PAYLOAD]` | string/json | 否 | `""` | 事件负载；配合 `-j` 可解析为 JSON 对象 |
| `-j, --json` | flag | 否 | — | 将 payload 按 JSON 对象解析而非普通字符串 |
| `--file <FILE>` | path | 否 | `.ralph/events.jsonl` | 目标事件文件路径 |
| `--policy-check` | flag | 否 | — | 发射前按当前事件策略校验 |
| `--unsafe-no-policy-check` | flag | 否 | — | 跳过强制策略检查（仅当配置允许时） |
| `--hat <HAT>` | string | 否 | `$RALPH_CURRENT_HAT` | 发布此事件的 hat |
| `--triggered <TRIGGERED>` | string | 否 | `$RALPH_TRIGGERED_HAT` | 被此事件触发的目标 hat |
| `--source <SOURCE>` | string | 否 | `$RALPH_EVENT_SOURCE` | 事件来源标识 |

**环境变量：**

| 变量 | 作用 |
|------|------|
| `RALPH_EVENTS_FILE` | 非空时，直接作为事件文件路径（最高优先级） |
| `RALPH_CURRENT_HAT` | 回退到 `--hat` |
| `RALPH_TRIGGERED_HAT` | 回退到 `--triggered` |
| `RALPH_EVENT_SOURCE` | 回退到 `--source` |

**事件文件解析优先级：**
1. 显式 `RALPH_EVENTS_FILE` 或非默认 `--file`（必须命中本 loop 的 events allowlist，否则 `ralph emit` 拒绝写入并报错；不静默回退到 marker）
2. `.ralph/current-candidate-events` marker 文件
3. `.ralph/current-events` marker 文件
4. `.ralph/events.jsonl` 默认路径

**反模式 / 注意事项：**
- 🔴 **不要**在 wave worker 内部使用 `ralph emit` 发射 wave 事件；worker 应直接通过标准输出或 `ralph emit` 返回结果，而不是触发新 wave。
- 🔴 `--unsafe-no-policy-check` 仅在配置显式允许时可用，否则会导致校验失败。
- 🔴 `ralph emit` **没有** `format` 选项。
- 🔴 试图通过 `RALPH_EVENTS_FILE` 或 `--file` 写入其他 worktree 的 events 文件会被 `ralph emit` 拒绝；错误信息会列出当前 allowlist。

**校验：**
```bash
# 1. 确定实际写入的事件文件（与 ralph emit 源码一致）
events_file="${RALPH_EVENTS_FILE:-}"
if [ -z "$events_file" ] && [ -f .ralph/current-candidate-events ]; then
  events_file="$(cat .ralph/current-candidate-events)"
elif [ -z "$events_file" ] && [ -f .ralph/current-events ]; then
  events_file="$(cat .ralph/current-events)"
fi
events_file="${events_file:-.ralph/events.jsonl}"

# 2. 确认事件已追加到文件末尾
tail -n 1 "$events_file" | jq -e ".topic == \"YOUR_TOPIC\""

# 3. 确认 payload 格式正确（若使用了 -j）
tail -n 1 "$events_file" | jq -e '.payload | type == "object"'
```

---

## 错误恢复

| 错误 | 原因 | 修复 |
|------|------|------|
| `events file not in allowlist` | `RALPH_EVENTS_FILE` / `--file` 命中非 allowlist 路径 | 查看错误信息中列出的 allowlist 条目；优先移除显式参数让 ralph emit 走 marker 解析 |
| `topic is required` | 缺少位置参数 | 补上 topic |
| `policy check failed` | payload 不符合策略 | 检查格式或用 `--unsafe-no-policy-check` |
| `cannot write to events file` | 文件不存在或权限不足 | 确认 `.ralph/` 目录存在，检查权限 |
| `Invalid JSON payload` | 用 `-j` 但 payload 不是合法 JSON | 用 `jq` 验证 payload：`echo '{"a":1}' \| jq .` |
| `Event provenance required` | 配置要求 hat 但 `--hat` 未设且 `RALPH_CURRENT_HAT` 空 | 显式 `--hat <hat-id>` 或设置环境变量 |
| `.ralph/` 目录不存在 | 在非 ralph 工作区调用 | 确认在 ralph 编排工作区内；或 `mkdir -p .ralph` 手动初始化（不推荐） |
| `Refusing urgent steer marker` | 上轮 urgent steer 未处理 | 先处理 urgent steer 内容（参见 error 信息中的指引），再重试 emit |
| 任何命令失败 | 通用恢复 | 1. `ralph emit --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |

> **wave worker 注意**：在 `RALPH_WAVE_WORKER=1` 的子进程中通过 `ralph emit` 返回结果时，事件会写入 **candidate-events**（不是 current-events），与 wave 调度器一致。不要强行设置 `RALPH_EVENTS_FILE` 指向其他文件——会被 allowlist 拒绝。
