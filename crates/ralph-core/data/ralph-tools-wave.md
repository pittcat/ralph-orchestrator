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

### Wave OPAC 四阶段

Wave OPAC 与单 emit OPAC 并列——同样四阶段，差别只在 Confirm 路径：

| 阶段 | 命令 | 关键约束 |
|------|------|----------|
| **Observe** | `ralph inspect loop` + `ralph tools task list` | 不要读 `.ralph/events.jsonl` / `supervisor.db`（HARD RULE 4） |
| **Precheck** | `ralph wave verify --payloads-stdin` | 零写盘；与 emit 同源 `policy_check` / origin guard |
| **Apply** | `ralph wave emit --payloads-stdin` | agent context 默认 enforce `--policy-check`；不通过 verify 不能直发 |
| **Confirm** | `ralph events --events-source main --output json \| jq 'select(.wave_id == ...)'` | wave 写主 ledger，不走 hat-channel |

**Confirm 路径与单 emit 不同**：`ralph wave emit` 写入 `current-events`（参见 `crates/ralph-cli/src/wave.rs:resolve_events_file`），不走 hat-channel。所以 Confirm 不能用 `ralph events --events-source hat-channel`——必须从 main ledger 验。

**反模式**：

- ❌ 跳过 `wave verify` 直接 `wave emit`（agent default enforce 会拒写盘；预设 opt-out 只能由 config 显式允许）
- ❌ 在 worker hat 内调用 `ralph wave emit`（仅 dispatcher hat 可调用）
- ❌ Confirm 阶段读 `current-hat-events`（那是单 emit 通道，看不到 wave 写入）
- ❌ 试图让 agent emit `*.wave.complete` / `*.unit.ready` 等 supervisor 协调 topic（origin guard 拒收，参见下方表格）

### `ralph wave verify`

零写盘批预检；与 `wave emit` 共用同源 `ValidationPipeline` / schema / origin guard。

```bash
cat payloads.jsonl | ralph wave verify review.wave.ready --payloads-stdin --output json
# {"ok":true,"topic":"review.wave.ready","count":7}
```

通过后去掉 `--verify` 真正 emit：

```bash
# 同源 path，不重写 payload
cat payloads.jsonl | ralph wave emit review.wave.ready --payloads-stdin --policy-check
```

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
| `--output <FMT>` | enum | 否 | `text` | 输出格式：`text`（stdout 仅 wave_id）或 `json`（stdout `{wave_id, topic, count, events_file, deduplicated}`，用于机器验真；失败时 stdout 改为结构化 `validation_errors`，见下方「Schema 预检」） |
| `--idempotency-key <KEY>` | string | 否 | — | 幂等键。同一 `(loop_id, hat, topic, key)` 重复调用只返回首个 `wave_id` 并标 `deduplicated=true`，不写新事件。键最长 256 字节、ASCII、非空非空白。未传则行为与原版一致。 |
| `--policy-check` | flag | 否 | false | 显式强制 schema 预检（即便 config 未开启 event policy，也走 `event_policy.schemas.<topic>.required_fields` 校验） |
| `--unsafe-no-policy-check` | flag | 否 | false | 尝试绕过 schema 预检。当 config `event_policy.allow_unsafe_cli_emit: false` 时**不生效**（与 `ralph emit --unsafe-no-policy-check` 对齐）。与 `--policy-check` 互斥。 |

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

**幂等键：**

- `--idempotency-key` 实现基于同目录下 `.<events_basename>.idempotency.jsonl` 的持久化记录（例如 `events.jsonl` → `.events.jsonl.idempotency.jsonl`）。文件锁保证并发安全。
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
- 🔴 不要使用 `printf '%s\n' $(cat payloads.jsonl)` 后再 pipe——IFS word splitting 会把单个 JSON object 切成多个 token，触发 JSON object 校验失败。直接 `cat payloads.jsonl` 即可。

**Schema 预检：**

`ralph wave emit` 在 shape 校验之后、写盘之前会先对**整批** payload 做 event policy schema 预检（`crates/ralph-cli/src/policy_check.rs`），与 `ralph run` 循环内统一校验管线 `validation::rules_event_policy::EventPolicyRule` 行为一致：

- 默认行为：当 `ralph.yml`（或合并后的 preset）开启 `event_policy.enabled: true` 时强制启用预检。`require_policy_check_for_cli_emit: true` 不改变 wave 行为——wave 始终预检。
- 任一 payload 缺必需字段（如 `review.wave.ready` 的 `depth`），或任意 payload 触发 `payload_consistency:*` gate → 整批**原子拒绝**，**不写盘**任何 line。
- `--policy-check`：显式强制预检（即便 config 未开启 `event_policy`）。
- `--unsafe-no-policy-check`：尝试绕过预检；当 config `event_policy.allow_unsafe_cli_emit: false` 时**不生效**。

**JSON 失败响应**（`--output json`，stdout，exit ≠ 0）：

```json
{
  "ok": false,
  "error": "policy_validation_failed",
  "topic": "review.wave.ready",
  "validation_errors": [
    {"payload_index": 0, "field": "depth", "reason_code": "missing_required_field", "message": "Missing required field: depth"}
  ]
}
```

`reason_code` 稳定枚举：`missing_required_field` / `invalid_field_value` / `payload_type_mismatch` / `terminal_monotonicity_violation` / `duplicate_terminal_event` / `business_event_after_completion` / `invalid_topic_format` / `topic_denied`。agent 可 `jq -r '.validation_errors[].field' | sort -u` 一次性拿到所有缺失字段清单。

**Text 失败响应**（stderr，exit ≠ 0）：`policy validation failed: 7 payloads, missing required field 'depth' in 7`。

> 设计意图：一次响应列出所有违规 payload，避免「修一个再发、又错下一个」的来回。loop 端和 CLI 端是同源 schema 校验的两侧——CLI 失败 = 100% loop 端也会被拒；CLI 成功 → loop 端也会接受。

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
| `--idempotency-key exceeds 256 bytes` | key 过长（> 256B，参见 `crates/ralph-cli/src/wave.rs:MAX_IDEMPOTENCY_KEY_BYTES`） | 缩短；preset 公式远小于 256 |
| `--idempotency-key must be ASCII` | key 含非 ASCII 字节 | 改用 ASCII；如 `plan_name` 是中文，先 hash 或 percent-encode |
| `idempotency-key conflict: ...` | 同 scope 不同 payload | 改用不同 key（`round-2` 递增或换 task） |
| `incomplete prior wave emission: ...` | 上次 events 写了 N 行但 record 丢失，扫 events 也只找到少于 N 行 | 手工删除残留 events 行；或换新 key |
| `policy validation failed for topic 'X'`（exit ≠ 0） | 任一 payload 违反 `event_policy.schemas.<topic>.required_fields`，整批拒绝、零写盘 | 用 `--output json` 读 stdout 的 `validation_errors[].field` 一次性拿到全部缺失字段，修正后重发。`--unsafe-no-policy-check` 仅在 config 显式允许时生效 |
| `agent policy-check required` | agent context + wave emit 无 precheck 成功记录 | 先 `ralph wave verify --payloads-stdin` 通过，再正式 `ralph wave emit`。worker hat 调 wave 子命令会被 deny |
| 任何命令失败 | 通用恢复 | 1. `ralph wave emit --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |

> **wave worker 注意事项**：
>
> 1. **结果返回必须用 `ralph emit`**：在 `RALPH_WAVE_WORKER=1` 的子进程中，`ralph emit` 会将事件写入 **candidate-events**（不是 current-events），与 wave 调度器对 worker 输出的预期一致。`ralph wave emit` 本身在 worker 内被阻止（`crates/ralph-cli/src/wave.rs:128-137` `execute_emit` 入口检查）。
>
> 2. **candidate-events vs current-events 落点**：
>    - `ralph wave emit` → 写入 **current-events**（主循环的合并目标，3 级回退：`RALPH_EVENTS_FILE` → `.ralph/current-events` → `.ralph/events.jsonl`）
>    - `ralph emit`（在 wave worker 内）→ 写入 **candidate-events**（与 wave 调度器约定）
>    - 不要混用：worker 内不要试图设置 `RALPH_EVENTS_FILE` 把结果写到 current-events 绕过 candidate-events——这会被 `ralph emit` 的 allowlist 校验拒绝（参见 `crates/ralph-cli/src/main.rs` 的 `resolve_emit_path`）。
>
> 3. **wave_id 共享**：同一 `--payloads` 列表产生的 N 个事件共享同一个 `wave_id` 和 `wave_total`，由 `wave_index`（0..N-1）区分。聚合 hat（`aggregate.mode: wait_for_all`）据此识别同一 wave 的所有结果。

### 校验

```bash
# 推荐用 --output json 拿 wave_id + events_file，避开 tail/grep 拼装
wave_id=$(cat payloads.jsonl | ralph wave emit review.wave.ready --payloads-stdin --output json | jq -r .wave_id)
events_file=$(cat .ralph/current-events)

# 按 wave_id 精确验真（KTD-7），不能用 tail -n "$expected_count"
expected_count=7
jq -e --arg id "$wave_id" --argjson expected "$expected_count" '
  ([. | select(.wave_id == $id)] | length) == $expected
' "$events_file"
```

### 监督态协调话题

某些协调话题只能由 supervisor 路径发射，worker hat 不应直接 emit。是否属于这类 topic，以当前 preset 的运行时约束和 lint 结果为准；如果不确定，先看 preset 文档和 `ralph hats validate --strict` 的结果，再决定是否允许由 worker 发射。
