---
name: ralph-tools-wave
description: 完整 ralph wave 参考，包含参数表、事件文件解析优先级、约束、反模式、校验步骤、错误恢复
metadata:
  internal: true
---

# ralph wave — 完整参考

> **NEVER use echo/cat to write tasks or memories** — always use CLI tools.

---

## `ralph wave`

调度 wave 事件以实现并行 hat 执行。

> `wave` 命令没有 `root` 和 `format` 选项（root，format）。

### `ralph wave emit`

将多个 payload 作为 wave 事件发射，每个 payload 成为一个独立事件，共享同一个 `wave_id`。

**语法：**
```bash
ralph wave emit [OPTIONS] <TOPIC>
```

**参数：**

| 参数 | 类型 | 必需 | 默认值 | 说明 |
|------|------|------|--------|------|
| `<TOPIC>` | string | 是 | — | 所有 wave 事件的主题（如 `review.file`） |
| `--payloads <PAYLOADS>...` | string… | **是** | — | 每个 wave worker 一个 payload（`num_args = 1..`，至少 1 个） |

**事件文件解析优先级：**
1. `RALPH_EVENTS_FILE` 环境变量（非空时）
2. `.ralph/current-events` marker 文件
3. 默认 `.ralph/events.jsonl`

> 注意：`wave emit` 与 `ralph emit` 的事件文件解析逻辑不同——`wave emit` 使用 `current-events`，而 `ralph emit` 使用 `current-candidate-events`。

**约束：**
- 不能在 wave worker 内部使用（`RALPH_WAVE_WORKER=1` 时会阻止）。
- Wave worker 的结果应通过 `ralph emit` 返回，而非 `ralph wave emit`。

**反模式 / 注意事项：**
- 🔴 ralph wave emit 没有 format 选项。
- 🔴 不要在 wave worker 内部调用 `ralph wave emit`。

**校验：**
```bash
# 1. 确定事件文件（与 wave emit 源码一致）
events_file="${RALPH_EVENTS_FILE:-}"
if [ -z "$events_file" ] && [ -f .ralph/current-events ]; then
  events_file="$(cat .ralph/current-events)"
fi
events_file="${events_file:-.ralph/events.jsonl}"

# 2. 检查 wave 事件已写入
tail -n 3 "$events_file" | jq -s 'map(select(.topic == "YOUR_TOPIC")) | length'
```

---

## 错误恢复

| 错误 | 原因 | 修复 |
|------|------|------|
| `Cannot dispatch waves from inside a wave worker` | 在 `RALPH_WAVE_WORKER=1` 的子进程中调用 `ralph wave emit` | worker 应通过 `ralph emit` 返回结果（会写入 candidate-events） |
| `At least one payload is required` | `--payloads` 为空（`num_args = 1..` 在 clap 中仍允许空数组） | 至少提供 1 个 payload：`ralph wave emit review.file --payloads a.txt b.txt c.txt` |
| `Failed to create directory: <path>` | 父目录无写权限或路径非法 | 检查 `.ralph/` 父目录权限；或设置 `RALPH_EVENTS_FILE` 指向可写路径 |
| `Failed to open events file: <path>` | 事件文件路径不可写或不存在 | 确认 `RALPH_EVENTS_FILE` / marker 指向的路径可写；或 `mkdir -p .ralph` |
| 任何命令失败 | 通用恢复 | 1. `ralph wave emit --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |

> **wave worker 注意事项**：
>
> 1. **结果返回必须用 `ralph emit`**：在 `RALPH_WAVE_WORKER=1` 的子进程中，`ralph emit` 会将事件写入 **candidate-events**（不是 current-events），与 wave 调度器对 worker 输出的预期一致。`ralph wave emit` 本身在 worker 内被阻止（`crates/ralph-cli/src/wave.rs:49-54`）。
>
> 2. **candidate-events vs current-events 落点**：
>    - `ralph wave emit` → 写入 **current-events**（主循环的合并目标，3 级回退：`RALPH_EVENTS_FILE` → `.ralph/current-events` → `.ralph/events.jsonl`）
>    - `ralph emit`（在 wave worker 内）→ 写入 **candidate-events**（与 wave 调度器约定）
>    - 不要混用：worker 内不要试图设置 `RALPH_EVENTS_FILE` 把结果写到 current-events 绕过 candidate-events——这会被 `ralph emit` 的 allowlist 校验拒绝（参见 `crates/ralph-cli/src/main.rs` 的 `resolve_emit_path`）。
>
> 3. **wave_id 共享**：同一 `--payloads` 列表产生的 N 个事件共享同一个 `wave_id` 和 `wave_total`，由 `wave_index`（0..N-1）区分。聚合 hat（`aggregate.mode: wait_for_all`）据此识别同一 wave 的所有结果。
