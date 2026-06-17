---
name: ralph-tools
description: Core CLI reference and rules for Ralph orchestration agents
metadata:
  internal: true
---

# Ralph CLI 核心参考

> **前提**：本 skill 仅在 `memories.enabled` 或 `tasks.enabled` 至少一个启用时被注入（`crates/ralph-core/src/event_loop/mod.rs:4862-4873`）。速查表中的"已注入"列均受此条件约束。

> **遇到不确定的命令语法时，先 `ralph <cmd> --help` 再执行。**

## 核心规则

1. **绝不用 echo/cat 写 tasks 或 memories** — 必须用 CLI 工具
2. **emit 后必须校验** — 确认事件已写入事件文件
3. **task/memory 操作后必须确认状态** — 用 `--format json` + `jq` 验证
4. **失败时先查 `--help`** — 不要猜测参数，文档可能已更新

## 收到 `task.resume` 时（policy / origin / contract 拒收后自动注入）

编排器拒收后会在 PENDING EVENTS 注入 `task.resume`（payload 形状：`crates/ralph-core/src/event_loop/rejection.rs:324-398` `build_task_resume_payload`）。**不要重发同样 payload**，按以下顺序修复：

1. **读 PENDING EVENTS 里 `task.resume` 的 JSON payload**，关键字段：
   - `stage`：`origin` / `policy` / `execution_contract` / `payload_contract`
   - `topic`：被拒收的事件主题
   - `violation`：人类可读原因（含字段名 / 类型不匹配）
   - `required_fields`：当前 topic 缺失或类型错的字段清单
   - `allowed_topics`：当前 hat 可发布的所有 topic（**只在这列里挑**）
2. **对照 `required_fields` 补齐 payload**；用 `ralph emit <topic> --policy-check -j '...'` 在写盘前预检（U4，CLI 100% 与 loop gate 同源 schema）。
3. **确认 hat 作用域**：isolated 模式下未在 `allowed_topics`（与 hat `publishes` 交集）的 topic 越权 — 改用 hat 实际可发的 topic，不要靠 `--unsafe-no-policy-check` 绕过。
4. **不要**用 `--unsafe-no-policy-check` 绕 policy；`ce-executor-isolated` preset 默认 `allow_unsafe_cli_emit: false`，该参数直接被拒。**不要**直写 `events.jsonl` — 写完仍会被 `payload_contract` 拒。
5. **复杂 violation**（`progress_task_mismatch` / `handoff_dispatch_timeout` / `plan.blocked` / `review_passed_while_wave_open` 等）一行摘要见 `ralph-tools-handoff`；按需 `ralph tools skill load ralph-tools-handoff` 加载深参考。
6. **仍不明**：`RALPH_DIAGNOSTICS=1` 启的 loop 把 envelope 写到 `recovery.jsonl`；`ralph diagnose --session latest` 出报告（`docs/guide/runtime-diagnosis.md` §10）。

## 命令速查表

### `ralph tools` 命名空间（已注入，按需读取对应子 skill）

| 命令 | 用途 | 详细参考 |
|------|------|---------|
| `ralph tools task` | 任务管理 | 已注入（`ralph-tools-tasks` skill，仅当 `tasks.enabled`） |
| `ralph tools memory` | 记忆管理 | 已注入（`ralph-tools-memories` skill，仅当 `memories.enabled`） |
| `ralph tools skill` | 加载 skill | `ralph tools skill load ralph-tools-cmdref` |
| `ralph tools interact` | Telegram 通知 | `ralph tools skill load ralph-tools-cmdref` |

### 顶层命令（按需加载对应 skill）

| 命令 | 用途 | 详细参考 |
|------|------|---------|
| `ralph emit` | 发射事件（最常用） | `ralph tools skill load ralph-tools-emit` |
| Step handoff / ce-executor | `task.resume` 复杂 violation | `ralph tools skill load ralph-tools-handoff` |
| `ralph wave emit` | 并行 wave 调度 | `ralph tools skill load ralph-tools-wave` |
| `ralph run` | 启动编排循环 | `ralph tools skill load ralph-tools-cmdref` |
| `ralph hats validate [--strict]` | 拓扑/payload/orphan/lint 校验 | `crates/ralph-cli/src/hats.rs:170`（strict 时启用 lint 所有权检查） |

> **按需加载需要 hat 上下文**：`ralph tools skill load` 在 agent 上下文中要求 `RALPH_CURRENT_HAT` 已设置（`crates/ralph-cli/src/skill_cli.rs:78-87`），否则会以非零退出。如加载失败，先检查 `echo $RALPH_CURRENT_HAT` 是否非空。

## 事件文件解析优先级（`ralph emit` 完整规则）

`ralph emit` 写入路径解析为 3 级回退 + allowlist 校验（`crates/ralph-cli/src/cli/emit_path.rs:32-120`）：

1. 显式 `RALPH_EVENTS_FILE` 环境变量或非默认 `--file`（**必须命中 events allowlist**——来源是 `.ralph/current-candidate-events` 或 `.ralph/current-events` marker——否则 `ralph emit` 拒绝写入并打印 allowlist 内容）
2. `.ralph/current-candidate-events` marker 目标（仅当未提供显式路径时）
3. `.ralph/current-events` marker 目标（仅当未提供显式路径时）
4. `.ralph/events.jsonl` 默认路径（仅当两个 marker 都不存在时）

🔴 **绝不静默回退**：如果设置了 `RALPH_EVENTS_FILE=foo.jsonl` 但 `foo.jsonl` 不在 allowlist 中，命令会**失败**（不会改写到 marker），错误信息会列出当前 allowlist 的所有合法目标。

> `ralph wave emit` 的事件文件解析走 3 级：`RALPH_EVENTS_FILE` → `.ralph/current-events` → `.ralph/events.jsonl`（`crates/ralph-cli/src/wave.rs:551-560`），与 ralph emit 不同。**wave worker 通过 `ralph emit` 返回结果时，事件会写入 candidate-events（与 wave 调度相关），不要改写 `RALPH_EVENTS_FILE` 指向其他文件。**

### `ralph wave emit` Schema 预检（U4）

`ralph wave emit` 在 shape 校验之后、写盘之前会先对**整批** payload 做 event policy schema 预检（`crates/ralph-cli/src/policy_check.rs`），与 `ralph run` 循环内 `apply_event_policy_validation` 行为一致：

- 默认行为：当 `ralph.yml`（或合并后的 preset）开启了 `event_policy.enabled: true` 时，强制启用预检。
- 任一 payload 缺必需字段（如 `review.wave.ready` 的 `depth`）→ 整批**原子拒绝**，**不写盘**任何 line。
- `--policy-check`：显式强制预检（即便 config 未开启 `event_policy`）。
- `--unsafe-no-policy-check`：尝试绕过预检；当 config `event_policy.allow_unsafe_cli_emit: false` 时**不生效**（与 `ralph emit --unsafe-no-policy-check` 对齐）。

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

## 通用错误恢复

| 错误场景 | 可能原因 | 修复方式 |
|----------|---------|---------|
| `events file not in allowlist` | `RALPH_EVENTS_FILE`/`--file` 指向了非 allowlist 路径 | 查看错误信息中列出的 allowlist 条目；如需新路径，先 `touch` 一个 marker 或去掉显式参数 |
| `topic is required` | 缺少必需的位置参数 | 补上 topic 参数 |
| `policy check failed` | 事件不符合策略 | 读 stderr 列出违规字段（`validation_errors[].field`）；修正后用 `ralph emit <topic> --policy-check -j '...'` 预检通过再正式发出。**不要**首选 `--unsafe-no-policy-check`（`ce-executor-isolated` preset 默认 `allow_unsafe_cli_emit: false` 时该参数不生效） |
| `task not found` | task ID 不存在或属于其他 loop | `ralph tools task list` 确认当前可用任务 |
| `memory not found` | memory ID 不存在或无权访问 | `ralph tools memory list` 确认可用记忆 |
| `skill not found` | skill 名称错误或对当前 hat 不可见 | `ralph tools skill list` 确认可用 skill；检查 `RALPH_CURRENT_HAT` |
| `progress rate limited` | 5 秒内重复发送 | 等待 5 秒后重试 |
| 退出码 2 (lint gate) | preset 静态 lint 在 strict 模式下发现 error | 修复 preset 配置后重试；查看 `.ralph/diagnostics/preset-lint-error-*.json` |
| `policy validation failed` (`ralph wave emit`) | 任一 payload 违反 `event_policy.schemas.<topic>.required_fields`，整批拒绝 | 用 `--output json` 读 `validation_errors[].field` 一次性拿到全部缺失字段，修正后重发 |
| 任何命令失败 | 通用恢复 | 1. `ralph <cmd> --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |

## Agent Output Governance（2026-06-14 计划 003 — `ce-executor-isolated` only）

`ce-executor-isolated` preset 在四个卡点上加硬规则。agent 可能遇到如下机制 — 当以下块 / 变量 / 行为出现时，按对应说明处理。

### `## WAVE CONTEXT` Block（R1 — review-synthesizer only）

当 `review-synthesizer` hat 被激活时，runner 在 prompt 顶部注入固定格式块：

```text
## WAVE CONTEXT
The following wave metadata is injected by the runner. Do not count events manually — use this context.

```json
{
  "wave_id": "w-abc",
  "wave_total": 7,
  "received_count": 7,
  "expected_dimensions": ["correctness", "testing", "..."],
  "missing_dimensions": [],
  "ALL_DIMENSIONS_RECEIVED": true,
  "AGGREGATE_TIMEOUT": false
}
```
```

- 不要重新数 `events.jsonl` — 使用此上下文。`received_count` 已经包含 dimension done 事件的最新计数。
- 当 `ALL_DIMENSIONS_RECEIVED: true` 且 `AGGREGATE_TIMEOUT: false` 时，直接 emit `review.passed`。
- 当 `AGGREGATE_TIMEOUT: true` 时，list 中维度未齐，按 plan 的 `skip_reason=aggregate_timeout` 走。
- 当 `missing_dimensions` 非空时，复核是否还有未收的 worker。

**等效 env var**：`RALPH_WAVE_CONTEXT`（JSON 字符串，可 `echo $RALPH_WAVE_CONTEXT | jq`）。

#### Wave worker 环境变量

`ralph wave emit` 派发的 dimension worker 进程会注入以下 env var（与 prompt 顶部的 `## WAVE CONTEXT` / `## ASSIGNED DIMENSION` 块同源，由 wave dispatcher 盖章）：

- `RALPH_WAVE_WORKER`：固定值 `1`（标记当前进程是 wave worker；非 worker 进程不设此 var）。
- `RALPH_WAVE_ID`：当前 wave 的 `wave_id`（与 `review.wave.ready` payload 同值）。
- `RALPH_WAVE_INDEX`：当前 worker 在 wave 内的 0-based 索引。
- `RALPH_EVENTS_FILE`：当前 worker 专属的 events.jsonl 路径（worker 的 emit 落盘到此处，由 dispatcher 在 merge 时合并回主 events file）。
- `RALPH_WAVE_DIMENSION`：当前 worker 的分配维度（仅在 `review.wave.ready` 携带 `dimension` 字段时注入；与 `## ASSIGNED DIMENSION` 块同值）。dimension reviewer emit `review.dimension.done` 时必须用**精确等于**此值的 `dimension` 字段；CLI precheck 与 merge layer 双重拒绝 mismatch（`wave.worker.failed(reason=dimension_mismatch)` + `task.resume` 重试）。
- `RALPH_WAVE_CONTEXT`：wave 元数据 JSON（同 `## WAVE CONTEXT` 块内容），review-synthesizer / synthesizer 路径用。

### `## EPHEMERAL RELOCATED` Block（R3 — review-synthesizer / executor / fixer / shipper）

当 agent 在源码树下写了 `scratchpad.md` / `notes.md` / `tmp*.md` / `*.bak` 等运行时产物时，runner 在下一轮 prompt 顶部注入：

```text
## EPHEMERAL RELOCATED
The following runtime artefacts were moved out of the source tree by the runner. Do NOT recreate these files inside the source tree; write runtime notes to `.ralph/agent/` instead.

- `crates/ralph-core/scratchpad.md` → `.ralph/agent/scratchpad-{loop_id}.md` (1234 bytes appended)
```

- 已经迁移的内容在 `.ralph/agent/scratchpad-{loop_id}.md`（按 loop_id 命名空间，多 loop 不冲突）。`cat .ralph/agent/scratchpad-*.md` 可读。
- 不要再在源码树下重建这些文件（触发 review wave + P0 finding）。
- 该块被首次读取后消费（`std::mem::take`），后续 hat activation 看不到 — 这是设计。

### `ralph tools task ensure` 与 R4 Single-U 契约

**默认关闭**。当 `ce-executor-isolated` preset 启动后，`ralph run` 写 `.ralph/agent/.ralph-enforce-current-unit` marker，子进程 `ralph tools task ensure` 检测后激活契约。standalone CLI 用户可设环境变量 `RALPH_ENFORCE_CURRENT_UNIT=1`。

**契约**：

- key 形如 `ce-executor:{plan}:step-XX:uN-impl`（N 是数字）才被 gate。
- 同一 `(loop_id, plan_name, step)` 下如果已 open U1 task，再 ensure `u2-impl` 时：
  - CLI 退出非零，stderr 输出 `rejected by R4 single-U contract: ...`
  - ensure 返回已存在的 U1 task。
- 同一 U 的 sub-units（`u1a-impl` / `u1b-impl`）允许并存 — 都塌缩到 `u1`。
- 旧 key / 非 `uN-` 形状（`step-99-impl` / `review-bug-impl` 等）不被 gate，可共存 — 这是已知边界。
- 同 key 重复 ensure 是幂等的，返回同一 task。
- 失败时不要重试同一 key — 切换到下一 U 或关闭冲突 task。

## Decision Journal

使用 `.ralph/agent/decisions.md` 记录重大决策及其置信度评分。按文件顶部模板填写，ID 保持顺序（DEC-001、DEC-002、...）。

**置信度阈值：**
- **>80**：自主执行。
- **50–80**：继续执行，但需在 `decisions.md` 中记录。
- **<50**：选择最安全的默认方案，并在 `decisions.md` 中记录。
