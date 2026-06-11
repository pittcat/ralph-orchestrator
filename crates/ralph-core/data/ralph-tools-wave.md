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
| `--payloads <PAYLOADS>...` | string… | 二选一 | — | 每个 wave worker 一个 payload（`num_args = 1..`，至少 1 个） |
| `--payloads-stdin` | flag | 二选一 | false | 从 stdin 逐行读取 payload，适合 JSON payload 列表 |
| `--output <FMT>` | enum | 否 | `text` | 输出格式：`text`（stdout 仅 wave_id）或 `json`（stdout `{wave_id, topic, count, events_file}`，用于 U5 机器验真） |
| `--idempotency-key <KEY>` | string | 否 | — | 幂等键（U2）。同一 `(loop_id, hat, topic, key)` 重复调用只返回首个 `wave_id` 并标 `deduplicated=true`，不写新事件。键最长 256 字节、ASCII、非空非空白。未传则行为与原版一致。 |

`--payloads` 与 `--payloads-stdin` 互斥，必须提供其中一个。不要把多行 JSON 列表塞进一个 shell 变量后传给 `--payloads "$PAYLOADS"`；该用法会被拒绝。多 JSON payload 使用：

```bash
printf '%s\n' \
  '{"dimension":"correctness","focus":"..."}' \
  '{"dimension":"testing","focus":"..."}' \
  | ralph wave emit review.wave.ready --payloads-stdin
```

**事件文件解析优先级：**
1. `RALPH_EVENTS_FILE` 环境变量（非空时）
2. `.ralph/current-events` marker 文件
3. 默认 `.ralph/events.jsonl`

> 注意：`wave emit` 与 `ralph emit` 的事件文件解析逻辑不同——`wave emit` 使用 `current-events`，而 `ralph emit` 使用 `current-candidate-events`。

**约束：**
- 不能在 wave worker 内部使用（`RALPH_WAVE_WORKER=1` 时会阻止）。
- Wave worker 的结果应通过 `ralph emit` 返回，而非 `ralph wave emit`。

**幂等键（U2）：**

- `--idempotency-key` 实现基于同目录下 `.wave-idempotency.jsonl` 的持久化记录。文件锁保证并发安全。
- 推荐 review-coordinator 使用 `ce-review:{plan_name}:{task_id}:{step}:round-{fix_round}` 命名空间。
- 第一次调用返回 `deduplicated=false`，后续同 scope 同 payload 返回 `deduplicated=true` 和原 `wave_id`。
- 同 key 不同 payload 会报错（`idempotency-key conflict`），不静默覆盖。
- 跨 `loop_id` / `hat` / `topic` 不去重——通过 scope_key 哈希隔离。
- 故障恢复：如果 events 写完但 record 写失败（进程崩溃），下次同 key 调用扫 events 补 record。

**幂等示例：**

首次输出（`--output json`）：
```json
{"wave_id":"w-...","topic":"review.wave.ready","count":7,"events_file":"...","deduplicated":false}
```

重试同 key 同 payload 输出：
```json
{"wave_id":"w-...","topic":"review.wave.ready","count":7,"events_file":"...","deduplicated":true}
```

**反模式 / 注意事项：**
- 🔴 不要在 wave worker 内部调用 `ralph wave emit`。
- 🔴 不要使用 `ralph wave emit <topic> --payloads "$PAYLOADS"` 传递多行 JSON；使用 `--payloads-stdin`。
- 🔴 不要使用 `printf '%s\n' $(cat payloads.jsonl)` 后再 pipe——IFS word splitting 会把单个 JSON object 切成多个 token，触发 U1 的 JSON object 校验失败。直接 `cat payloads.jsonl` 即可。

**校验：**
```bash
# U5: 推荐用 --output json 拿 wave_id + events_file，避开 tail/grep 拼装
wave_id=$(cat payloads.jsonl | ralph wave emit review.wave.ready --payloads-stdin --output json | jq -r .wave_id)
events_file=$(cat .ralph/current-events)

# 按 wave_id 精确验真（KTD-7），不能用 tail -n "$expected_count"
expected_count=7
jq -e --arg id "$wave_id" --argjson expected "$expected_count" '
  ([. | select(.wave_id == $id)] | length) == $expected
' "$events_file"
```

---

## 错误恢复

| 错误 | 原因 | 修复 |
|------|------|------|
| `Cannot dispatch waves from inside a wave worker` | 在 `RALPH_WAVE_WORKER=1` 的子进程中调用 `ralph wave emit` | worker 应通过 `ralph emit` 返回结果（会写入 candidate-events） |
| `At least one payload is required` | `--payloads` 为空或 `--payloads-stdin` 未读到非空行 | 至少提供 1 个 payload：`ralph wave emit review.file --payloads a.txt b.txt c.txt`，或用 `--payloads-stdin` |
| ``--payloads` argument <i> contains multiple JSON payload lines` | 把多行 JSON 列表作为一个 shell 参数传给了 `--payloads` | 改用 `--payloads-stdin` |
| `payload[<i>] is not a JSON object: ...` | 输入不是合法 JSON object（数字、字符串、数组、token、截断 JSON 都拒） | 确保每个 payload 都是 `{"key": ...}` object；不要用 `printf '%s\n' $(cat file.jsonl)`（IFS word splitting 制造 token） |
| `Failed to create directory: <path>` | 父目录无写权限或路径非法 | 检查 `.ralph/` 父目录权限；或设置 `RALPH_EVENTS_FILE` 指向可写路径 |
| `Failed to open events file: <path>` | 事件文件路径不可写或不存在 | 确认 `RALPH_EVENTS_FILE` / marker 指向的路径可写；或 `mkdir -p .ralph` |
| `--idempotency-key must not be empty` | key 为空串 | 传非空字符串（推荐 preset 公式） |
| `--idempotency-key must not be whitespace-only` | key 全是空白 | 同上 |
| `--idempotency-key exceeds 256 bytes` | key 过长（> 256B） | 缩短；preset 公式远小于 256 |
| `--idempotency-key must be ASCII` | key 含非 ASCII 字节 | 改用 ASCII；如 `plan_name` 是中文，先 hash 或 percent-encode |
| `idempotency-key conflict: ...` | 同 scope 不同 payload | 改用不同 key（`round-2` 递增或换 task） |
| `incomplete prior wave emission: ...` | 上次 events 写了 N 行但 record 丢失，扫 events 也只找到少于 N 行 | 手工删除残留 events 行；或换新 key |
| 任何命令失败 | 通用恢复 | 1. `ralph wave emit --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |

> **wave worker 注意事项**：
>
> 1. **结果返回必须用 `ralph emit`**：在 `RALPH_WAVE_WORKER=1` 的子进程中，`ralph emit` 会将事件写入 **candidate-events**（不是 current-events），与 wave 调度器对 worker 输出的预期一致。`ralph wave emit` 本身在 worker 内被阻止（`crates/ralph-cli/src/wave.rs:64-69`）。
>
> 2. **candidate-events vs current-events 落点**：
>    - `ralph wave emit` → 写入 **current-events**（主循环的合并目标，3 级回退：`RALPH_EVENTS_FILE` → `.ralph/current-events` → `.ralph/events.jsonl`）
>    - `ralph emit`（在 wave worker 内）→ 写入 **candidate-events**（与 wave 调度器约定）
>    - 不要混用：worker 内不要试图设置 `RALPH_EVENTS_FILE` 把结果写到 current-events 绕过 candidate-events——这会被 `ralph emit` 的 allowlist 校验拒绝（参见 `crates/ralph-cli/src/main.rs` 的 `resolve_emit_path`）。
>
> 3. **wave_id 共享**：同一 `--payloads` 列表产生的 N 个事件共享同一个 `wave_id` 和 `wave_total`，由 `wave_index`（0..N-1）区分。聚合 hat（`aggregate.mode: wait_for_all`）据此识别同一 wave 的所有结果。
