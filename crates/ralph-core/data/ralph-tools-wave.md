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

Wave OPAC 与单 emit OPAC 并列——同样四阶段，差别只在 Confirm 路径与 Precheck→Apply 之间的 ticket gate：

| 阶段 | 命令 | 关键约束 |
|------|------|----------|
| **Observe** | `ralph inspect loop` + `ralph tools task list` | 不要读 ledger 文件（HARD RULE 4） |
| **Precheck** | `ralph wave verify --payloads-stdin` | 零写盘；通过后写一次性 ticket；与 emit 同源 `policy_check` / origin guard |
| **Apply** | `ralph wave emit --payloads-stdin` | 必须有匹配的 ticket；同源 payload 集合；agent context 默认 enforce `--policy-check` |
| **Confirm** | 用 emit 返回的 `wave_id` 调公开只读查询 | ticket 消费仅证明 Apply 已写盘，**不证明下游已完成** |

**Confirm 路径与单 emit 不同**：`ralph wave emit` 写入主 ledger。所以 Confirm 必须从主 ledger 验，不要看单 emit 通道。

**关键约束（agent 视角）**：

- 🔴 Precheck→Apply 之间**不可漂移**：`verify` 通过后改任何 payload（增删、修改、重排）后再 `emit` → 拒。必须重新 `verify`。
- 🔴 ticket 是**一次性**：emit 成功后 ticket 消费；下次同 topic 必须重新 verify。
- 🔴 `--unsafe-no-policy-check` 仅绕过 schema gate，**不绕过** OPAC ticket gate。
- 🔴 人类 CLI（不在 agent context）bypass ticket。
- 🔴 ticket 消费 ≠ Confirm：仅证明 Apply 已写盘，不证明下游 worker / aggregator 已完成。

**反模式**：

- ❌ 跳过 `wave verify` 直接 `wave emit`（agent 默认 enforce 会拒写盘）
- ❌ 在 worker hat 内调用 `ralph wave emit`（仅 dispatcher hat 可调用）
- ❌ Confirm 阶段读单 emit 通道（看不到 wave 写入）
- ❌ 试图让 agent emit `*.wave.complete` / `*.unit.ready` 等 supervisor 协调 topic（origin guard 拒收）
- ❌ 改 payload 后再 `emit`（fingerprint mismatch → ticket gate 拒）

### `ralph wave verify`

零写盘批预检；与 `wave emit` 共用同源 schema / origin guard。**通过后写一次性 ticket**，下一次 `wave emit` 必须使用完全相同的 payload 集合。

```bash
cat payloads.jsonl | ralph wave verify review.wave.ready --payloads-stdin --output json
# {"ok":true,"topic":"review.wave.ready","count":7}
```

捕获返回的 `topic` / `count`，然后同源 path 用同一文件 emit（不要重新生成 payload）：

```bash
# 同源 path：cat payloads.jsonl 必须与 verify 时相同字节内容
cat payloads.jsonl | ralph wave emit review.wave.ready --payloads-stdin --output json
# {"ok":true,"wave_id":"w-...","topic":"review.wave.ready","count":7,"events_file":"...","deduplicated":false}
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
| `--output <FMT>` | enum | 否 | `text` | 输出格式：`text`（stdout 仅 wave_id）或 `json`（stdout `{ok, wave_id, topic, count, events_file, deduplicated}`） |
| `--idempotency-key <KEY>` | string | 否 | — | 幂等键。同一 `(loop_id, hat, topic, key)` 重复调用只返回首个 `wave_id` 并标 `deduplicated=true`，不写新事件。键最长 256 字节、ASCII、非空非空白 |
| `--policy-check` | flag | 否 | false | 显式强制 schema 预检 |
| `--unsafe-no-policy-check` | flag | 否 | false | 尝试绕过 schema 预检。当 config `event_policy.allow_unsafe_cli_emit: false` 时**不生效**。与 `--policy-check` 互斥。**仅绕 schema，不绕 OPAC ticket** |

`--payloads` 与 `--payloads-stdin` 互斥，必须提供其中一个。

**事件文件解析优先级：**
1. `RALPH_EVENTS_FILE` 环境变量（非空时）
2. `.ralph/current-events` marker 文件
3. 默认 `.ralph/events.jsonl`

**约束：**
- agent context：必须先 `wave verify` 再 `wave emit`；二者 payload 必须同源（不重写、不增删、不重排）。
- 不能在 wave worker 内部使用（`RALPH_WAVE_WORKER=1` 时会阻止）。
- Wave worker 的结果应通过 `ralph emit` 返回，而非 `ralph wave emit`。

**幂等键：**

- **作用**：`--idempotency-key` 让「同一批 wave 内容重入」安全。同一 `(loop_id, hat, topic, key)` 且 payload 不变时重复调用，runtime 直接返回**第一次**产生的 `wave_id` 并标 `deduplicated=true`，**不重复派发** worker、不再写新事件。键最长 256 字节、ASCII、非空非空白。
- **权威去重源**：某批内容是否「已派发过」由 runtime 维护的 **wave 账本**（supervisor 的单一事实源）判定。只要该账本里已存在对应 wave，重入就返回既有 `wave_id`。agent 不需要、也不能直接读写这个账本——判断本次是否被去重，只看 `--output json` 返回里的 `deduplicated` 字段。
- **过渡兼容层（弃用，非权威）**：早期版本会在本地额外写一份幂等记录文件作为兼容层；它**不是权威**，仅为旧工具链过渡保留，带弃用语义，将在后续版本移除。**不要**读取、解析或依赖它来判断是否重入——一切以 `wave emit` 返回的 `wave_id` / `deduplicated` 为准。若在日志中看到关于该文件的弃用提示，属正常现象，无需处理。
- **如何取 key**：用能唯一标识本次 wave 内容的稳定片段拼 key（如计划名、task id、step、轮次），保证「内容相同的重入」命中同一 key、「内容不同」用不同 key。
- **作用域**：跨 `loop_id` / `hat` / `topic` 不去重——key 的去重范围由这三者加 key 本身共同限定。
- **冲突停止条件**：同 key 但 payload 不同会报错（`idempotency-key conflict`），不静默覆盖。遇到冲突说明同一 key 被用于不同内容——**停止**，改用能区分本次内容的 key（如把轮次段递增）后重发，不要用同一 key 反复重试。

**幂等示例：**

首次输出（`--output json`）：
```json
{"ok":true,"wave_id":"w-...","topic":"review.wave.ready","count":7,"events_file":"...","deduplicated":false}
```

重试同 key 同 payload 输出：
```json
{"ok":true,"wave_id":"w-...","topic":"review.wave.ready","count":7,"events_file":"...","deduplicated":true}
```

**反模式 / 注意事项：**
- 🔴 不要在 wave worker 内部调用 `ralph wave emit`。
- 🔴 不要使用 `ralph wave emit <topic> --payloads "$PAYLOADS"` 传递多行 JSON；使用 `--payloads-stdin`。
- 🔴 不要使用 `printf '%s\n' $(cat payloads.jsonl)` 后再 pipe——IFS word splitting 会把单个 JSON object 切成多个 token，触发 JSON object 校验失败。直接 `cat payloads.jsonl` 即可。
- 🔴 不要修改 `verify` 与 `emit` 之间的 payload——fingerprint mismatch 会拒。

**Schema 预检：**

`ralph wave emit` 在 shape 校验之后、写盘之前会先对**整批** payload 做 event policy schema 预检：

- 默认行为：当 `ralph.yml`（或合并后的 preset）开启 `event_policy.enabled: true` 时强制启用预检。
- 任一 payload 缺必需字段（如 `review.wave.ready` 的 `depth`）→ 整批**原子拒绝**，**不写盘**任何 line。
- `--policy-check`：显式强制预检（即便 config 未开启 `event_policy`）。
- `--unsafe-no-policy-check`：尝试绕过预检；当 config `event_policy.allow_unsafe_cli_emit: false` 时**不生效**。**仅绕 schema，不绕 OPAC ticket**。

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

`reason_code` 稳定枚举：`missing_required_field` / `invalid_field_value` / `payload_type_mismatch` / `terminal_monotonicity_violation` / `duplicate_terminal_event` / `business_event_after_completion` / `invalid_topic_format` / `topic_denied`。

**Text 失败响应**（stderr，exit ≠ 0）：`policy validation failed: 7 payloads, missing required field 'depth' in 7`。

> 设计意图：一次响应列出所有违规 payload，避免「修一个再发、又错下一个」的来回。

---

## 错误恢复

| 错误 | 原因 | 修复 |
|------|------|------|
| `Cannot dispatch waves from inside a wave worker` | 在 `RALPH_WAVE_WORKER=1` 的子进程中调用 `ralph wave emit` | worker 应通过 `ralph emit` 返回结果 |
| `At least one payload is required` | `--payloads` 为空或 `--payloads-stdin` 未读到非空行 | 至少提供 1 个 payload |
| ``--payloads` argument <i> contains multiple JSON payload lines` | 把多行 JSON 列表作为一个 shell 参数传给了 `--payloads` | 改用 `--payloads-stdin` |
| `payload[<i>] is not a JSON object: ...` | 输入不是合法 JSON object | 确保每个 payload 都是 `{"key": ...}` object |
| `Failed to create directory: <path>` | 父目录无写权限或路径非法 | 检查 `.ralph/` 父目录权限；或设置 `RALPH_EVENTS_FILE` 指向可写路径 |
| `Failed to open events file: <path>` | 事件文件路径不可写或不存在 | 确认 `RALPH_EVENTS_FILE` / marker 指向的路径可写；或 `mkdir -p .ralph` |
| `--idempotency-key must not be empty` | key 为空串 | 传非空字符串 |
| `--idempotency-key must not be whitespace-only` | key 全是空白 | 同上 |
| `--idempotency-key exceeds 256 bytes` | key 过长（>256B） | 缩短 |
| `--idempotency-key must be ASCII` | key 含非 ASCII 字节 | 改用 ASCII |
| `idempotency-key conflict: ...` | 同 scope 不同 payload | 改用不同 key |
| `policy validation failed for topic 'X'`（exit ≠ 0） | 任一 payload 违反 schema，整批拒绝、零写盘 | 用 `--output json` 读 stdout 的 `validation_errors[].field` 一次性拿到全部缺失字段 |
| `wave_verify_gate denied 'X': no verify ticket ...`（exit ≠ 0） | agent 未先 `wave verify` | 先 `ralph wave verify <topic> --payloads-stdin`，再用同一未修改的 payload `ralph wave emit <topic> --payloads-stdin` |
| `wave_verify_gate denied 'X': ticket fingerprint mismatch ...` | verify 与 emit 之间的 payload 改了 | 用当前 payload 重跑 `wave verify`，再 `wave emit` |
| `wave_verify_gate denied 'X': ticket (loop, hat) = ... but caller is ...` | verify 时与 emit 时的 (loop, hat) 不同 | 在同一 hat + loop 中执行 verify 与 emit |
| 任何命令失败 | 通用恢复 | 1. `ralph wave emit --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |

> **wave worker 注意事项**：
>
> 1. **结果返回必须用 `ralph emit`**：在 `RALPH_WAVE_WORKER=1` 的子进程中，`ralph emit` 会将事件写入 candidate-events。`ralph wave emit` 本身在 worker 内被阻止（dispatcher hat 才能调用）。
>
> 2. **落点差异**：
>    - `ralph wave emit` → 写入主 ledger（`current-events` 链）
>    - `ralph emit`（在 wave worker 内）→ 写入 candidate-events
>    - 不要混用：worker 内不要试图设置 `RALPH_EVENTS_FILE` 把结果写到 current-events 绕过 candidate-events——会被 `ralph emit` 的 allowlist 校验拒绝。
>
> 3. **wave_id 共享**：同一 `--payloads` 列表产生的 N 个事件共享同一个 `wave_id` 和 `wave_total`，由 `wave_index`（0..N-1）区分。聚合 hat 据此识别同一 wave 的所有结果。

### Confirm 阶段

`wave emit` 仅证明 Apply 已写盘，不证明下游 worker / aggregator 已完成。Confirm 必须使用 emit 返回的 `wave_id` 调**公开只读查询**确认 wave 已登记：

```bash
# 1. Apply：emit 返回 wave_id
wave_id=$(cat payloads.jsonl | ralph wave emit review.wave.ready --payloads-stdin --output json | jq -r .wave_id)

# 2. Confirm：用 wave_id 验 wave 已登记
events_file=$(cat .ralph/current-events 2>/dev/null || echo .ralph/events.jsonl)
expected_count=7
jq -e --arg id "$wave_id" --argjson expected "$expected_count" '
  ([. | select(.wave_id == $id)] | length) == $expected
' "$events_file"
```

> Confirm 仅证明本次 wave 已登记，**不证明**下游 worker / aggregator 已完成。后续 hat activation 自会处理 worker 完成事件（`*.unit.done` 等）。

### 写隔离

`isolation_mode=worktree` 时 runtime 为每个 slot 提供隔离 cwd；agent 不应自行创建 / 删除 worktree。默认（未声明 `isolation_mode`）所有 worker 共享当前 workspace，与原 wave 行为一致。

### 取消 / 补偿

`ralph wave emit` 不直接暴露取消 / 补偿命令。Wave 在 aggregate timeout / 显式 cancel / spawn failure 时由 runtime 自动标记状态；inspect / diagnose 公开返回 wave 的当前阶段（Collect / Cancelled / Failed / Done），agent 据此决定后续动作。补偿 job 由 runtime 在终态阶段执行诊断记录，**不阻塞** wave 的最终态。