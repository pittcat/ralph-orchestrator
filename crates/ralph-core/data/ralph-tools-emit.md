---
name: ralph-tools-emit
description: 完整 ralph emit 参考，包含参数表、环境变量、事件文件解析优先级、反模式、校验步骤、错误恢复
metadata:
  internal: true
---

# ralph emit — 完整参考

> **NEVER use echo/cat to write tasks or memories** — always use CLI tools.

> **速查见 `ralph-tools` 自动注入段**：本 skill 是按需深参考。loop 内最常见的 `task.resume` 修复（policy / origin / contract）已在每轮自动注入的 `ralph-tools.md` 「收到 `task.resume` 时」段说明；本文件覆盖更细的 emit / schema / null-payload / isolated 越权表。

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
| `<TOPIC>` | string | 是 | — | 事件主题，如 `task.completed`、`my-event` |

> ⚠️ **Hat 作用域规则（Isolated 模式）**：在 `execution_mode: isolated` 下，每个 hat 只能发布其 `publishes` 列表中声明的 topic。发布未声明的 topic 会被 `EventOriginGuard` 拒绝，并触发指向该 hat 的 `task.resume`。连续 4 次越权将触发熔断终止。上例中的 `task.completed` 和 `my-event` 仅为通用占位符，实际可发布的 topic 以当前 hat 的 `publishes` 配置为准。
| `[PAYLOAD]` | string/json | 否 | `""` | 事件负载；配合 `-j` 可解析为 JSON 对象 |
| `-j, --json` | flag | 否 | — | 将 payload 按 JSON 对象解析而非普通字符串 |
| `--file <FILE>` | path | 否 | `.ralph/events.jsonl` | 目标事件文件路径 |
| `--policy-check` | flag | 否 | — | 发射前按当前事件策略校验；**校验通过不写盘**（dry-run 探测，与正式 `ralph emit` 区分） |
| `--unsafe-no-policy-check` | flag | 否 | — | 跳过强制策略检查（仅当配置允许时） |
| `--hat <HAT>` | string | 否 | `$RALPH_CURRENT_HAT` | 发布此事件的 hat |
| `--triggered <TRIGGERED>` | string | 否 | `$RALPH_TRIGGERED_HAT` | 被此事件触发的目标 hat |
| `--source <SOURCE>` | string | 否 | `$RALPH_EVENT_SOURCE` | 事件来源标识 |
| `--schema <TOPIC>` | string | 否 | — | 打印 `<TOPIC>` 的 embedded 协议 JSON 视图 + `protocol_hash`(U5 / R6,plan 2026-06-20-001);**只读,不写 events.jsonl,不消耗 iteration,不触发 lint**;与 `payload` / `--json` / `--policy-check` 互斥。常见用途:检测 authoring YAML 与 embedded 协议 drift、查 `required_fields` / `is_macro_edge`。详见 [`docs/handbook/serial-preset-development.md`](../../../docs/handbook/serial-preset-development.md) §"`ralph emit --schema <TOPIC>`"。 |
| `--output <MODE>` | string | 否 | `text` | policy-check / apply 响应格式。`json` → stdout 输出 [`EmitResult`](#ralph-emit-响应emitresult) JSON；`text` → 保留旧版人类可读输出（默认） |

**Schema 模式示例：**
```bash
# 查 work.done 协议视图
ralph emit --schema work.done

# 检测 drift:build 前后 protocol_hash 必变
ralph emit --schema work.done | jq -r .protocol_hash   # 改前
# 改 presets/schemas/ce-executor-pipeline.yml 后
cargo build
ralph emit --schema work.done | jq -r .protocol_hash   # 改后
```

**Agent 用法（HARD RULE）：** 在构造任何业务事件 payload 之前，**先**跑 `ralph emit --schema <topic> -H builtin:<preset>`（或在无 preset-bearing `ralph.yml` 的 workspace 跑 `ralph emit --schema <topic> --config <ralph.yml> -H builtin:<preset>`），从返回的 `required_fields` 数组取得字段清单。**不要**靠错误消息逐字段猜。`--schema` 是只读的，不写 events.jsonl、不消耗 iteration。错误消息（如 `receiver_contract.to_hat must be a non-empty string`）用来定位字段层级，`--schema` 用来确认完整字段集——两者配合使用。

**环境变量：**

| 变量 | 作用 |
|------|------|
| `RALPH_EVENTS_FILE` | 非空时，直接作为事件文件路径（最高优先级） |
| `RALPH_CURRENT_HAT` | 回退到 `--hat` |
| `RALPH_TRIGGERED_HAT` | 回退到 `--triggered`。在 `execution_mode: isolated` 下，loop runner 不再注入该变量；`ralph emit` 会根据当前 topic 在 preset 中的唯一下游消费者自动推导 `triggered`；推导不出时保持为空。你不需要关心当前是什么模式，按需显式传 `--triggered` 即可，显式值始终优先。 |
| `RALPH_EVENT_SOURCE` | 回退到 `--source` |

> **`triggered` 自动推导规则**：`ralph emit` 会读取当前 workspace 配置。如果当前是 isolated 模式、你已处于某个 hat 上下文中（`RALPH_CURRENT_HAT` 已设置）、且没有显式传 `--triggered` / `RALPH_TRIGGERED_HAT`，`ralph emit` 会尝试把 `triggered` 填为当前 topic 的唯一下游目标。只有当拓扑能唯一确定下游 hat 时才填充；多消费者、无明确下游、控制/诊断 topic 都会保持 `triggered` 为空。Coordinator 模式或没有配置时保持原有注入行为不变。
>
> **对 agent 来说**：不需要检测当前模式。只要记住两条规则：1) 若 prompt 明确要求你触发某个 hat， emit 时带上 `--triggered <hat>`；2) 没要求时直接 emit，runner / CLI 会自动处理。

### Envelope 校验（`triggered` 拓扑）

`ralph emit --triggered <hat_id>` 在 apply 路径与 `--policy-check` 路径都会被 envelope 层校验：`triggered` 字段的值必须是当前 preset 声明的 hat 之一（即出现在 `hats[]` map 里），否则返回 `triggered_not_in_topology`。

该校验按 **topic 信任层**分流（与 `check_emit_provenance` 的 ralph-control / orchestrator-diagnostic / business 三层模型对齐）：

- **ralph-control topics**（`task.resume`、`loop.cancel`、`loop.complete`、`human.*` 等）以及 **orchestrator diagnostic topics**（`event.*`）— 跳过 topology check；runtime 注入事件时 `triggered` 经常是 `ralph` 这类 pseudo-hat，不需要在 `hats[]` 中声明。
- **business topics**（`work.done`、`queue.advance`、`review.dimension.*` 等）— 严格 topology check；`triggered` 必须是当前 preset 的 hat 之一。

`ralph emit` 默认 enforce 这条规则；如需绕开，只能用 `ralph emit --unsafe-no-policy-check`，但该参数在大多数 preset（包括 `ce-executor-pipeline`）下被直接拒绝。

**事件文件解析优先级：**
1. 显式 `RALPH_EVENTS_FILE` 或非默认 `--file`（必须命中本 loop 的 events allowlist，否则 `ralph emit` 拒绝写入并报错；不静默回退到 marker）
2. `.ralph/current-candidate-events` marker 文件
3. `.ralph/current-events` marker 文件
4. `.ralph/events.jsonl` 默认路径

**`RALPH_WORKSPACE_ROOT` 锚定（plan 2026-07-06-002）：**

emit 全路径用 `resolve_workspace_root` 单一锚点 (priority：`RALPH_WORKSPACE_ROOT` env > `discover_workspace_root(cwd)` > cwd)。runner 通过 `cli_executor::inject_ralph_runtime_env` 已注入 `RALPH_WORKSPACE_ROOT` 和 `PWD` —— hat 进程**不要 unset**。当你 `unset RALPH_EVENTS_FILE; cd sorts/; ralph emit ...` 时，事件可能落到 `sorts/.ralph/events.jsonl` 孤儿路径(参见 `docs/report/2026-07-06-ce-executor-ralph-emit-pwd-sorts-diagnosis.md`)。

**fail-closed 守卫（硬拒绝 + stdout 摘要）：**

- `cwd_workspace_drift`：isolated mode + hat 上下文 + 未注入 `RALPH_EVENTS_FILE` + 默认 `--file` + `canonicalize(cwd) != canonicalize(workspace_root)` → 拒收。stdout 一行：`emit rejected [cwd_workspace_drift]: current_dir=... workspace_root=...`。
- `orphan_events_path`：resolved 候选路径落在 `subdir/.ralph/...`（非 workspace 根 `.ralph/` 且非 `.ralph/agent/` hat-channel）→ 拒收。
- 显式非默认 `--file` 命中 allowlist 的高级场景不受 `cwd_workspace_drift` 限制。

**反模式 / 注意事项：**
- 🔴 **禁止直写 `.ralph/events.jsonl`**：必须通过 `ralph emit` / `ralph wave emit` 写入事件。直接 `echo ... >> .ralph/events.jsonl` 或 heredoc 写入会绕过 CLI pre-publish check；loop 读盘时仍会触发 `event_policy` 校验，并以 `payload_contract_violation` 拒绝整行（最坏情况：`not_retriable` 终止 loop）。文档：`docs/plans/2026-06-15-001-feat-schema-aware-hat-emit-instructions-plan.md` §4.6。
- 🔴 **`task_id` 字段禁止空字符串**：任何包含 `task_id` 的 payload（`work.ready`、`work.done`、`test.passed`、`queue.advance` 等）必须传非空字符串，如 `task-{timestamp}-{hex}`。`"task_id":""` 会被 `ralph emit` 直接拒绝，且会破坏 step handoff / state projection。
- 🔴 **不要**在 wave worker 内部使用 `ralph emit` 发射 wave 事件；worker 应直接通过标准输出或 `ralph emit` 返回结果，而不是触发新 wave。
- 🔴 **禁止 `unset RALPH_EVENTS_FILE` 后从子目录 `cd sorts/; ralph emit ...`**：isolated hat 进程会被 `cwd_workspace_drift` 硬拒绝，事件落不到目标。若 runner 注入的 env 被破坏，恢复 `unset` 前的 env 或 `cd $RALPH_WORKSPACE_ROOT` 后再 emit。
- 🔴 **发射前确认 hat 作用域**：在 isolated 模式下，发射 topic 前必须确认当前 hat 的 `publishes` 列表包含该 topic。越权 topic 会被拒绝并触发 `task.resume`，连续 4 次越权将触发熔断终止 loop。运行 `ralph hats list` 查看当前 preset 各 hat 的 publishes。
- 🔴 `--unsafe-no-policy-check` 仅在配置显式允许时可用，否则会导致校验失败。
- 🔴 `ralph emit` **没有** `format` 选项。
- 🔴 试图通过 `RALPH_EVENTS_FILE` 或 `--file` 写入其他 worktree 的 events 文件会被 `ralph emit` 拒绝；错误信息会列出当前 allowlist。

**`EmitResult.target_path`（plan 2026-07-06-002 R5）：**

- `--output json` 路径下 `EmitResult.target_path` 字段在 `recorded: true` 时为绝对落盘路径;`recorded: false` 或拒收场景下整键被 `skip_serializing_if` 省略。
- text 模式成功行追加 `→ <absolute_path>`:例如 `Event emitted: test.passed → /home/.../.ralph/agent/events-hat-validator-001-1.jsonl`。stderr 截断场景下仍能肉眼核对落盘位置。

**Payload 字段自洽检查**

emit 任何 step handoff 事件前，确认 payload 内部一致：

- 如果 preset 同时定义了 `step` 字段和 `task_key` 字段，两者描述的 step 必须一致。
  反例：`step=fix-02` 但 `task_key=...:fix-01:u2` ❌
- `task_id` 必须来自当前 loop 的真实任务列表（`ralph tools task list`），不要手写、不要复用已闭合 task 的 id。
- 聚合字段（如 `completed_steps`）必须与已落盘的事件流一致；不要在 `plan.complete` 中宣称某个 step 已完成而事件流里缺少它。

**isolated mode 单业务事件 / 重发规则**

- isolated mode 下，一个 hat 每个 activation（turn）只接受 **1 个业务事件**：运行时只保留你这一回合**最先发出**的那个业务事件，**其后发出的全部静默丢弃**——**无论 topic 是否相同**。
- 因此**绝不要在你真正要发的事件之前先发别的事件**。例如要发 `plan.complete` 收尾，就不要先发一个 `work.ready`——那个 `work.ready` 会占掉唯一名额，你的 `plan.complete` 会被丢掉，loop 收不到收尾信号。
- 正确做法：**一个 activation 只发一个业务事件，发完立即停止**，不要在同一回合补发、重发或追加任何其它业务事件。
- 事件被丢弃时运行时会向你注入一条 `task.resume`（reason=`isolated_extra_business_event_dropped`）提示重发；收到后，下一个 turn **只发一个**你真正想发的事件即可。
- 如果事件发出后没有产生预期推进，先检查 schema 与字段自洽，**不要立即重发**；等收到新的 `task.resume` 或进入新的 turn 再发。

**状态驱动的 emit 规则（通用）**

- 在发射任何“已经由你或上游 hat 发射过”的事件之前，先检查事件文件：loop 可能已被重启、你的上一轮可能已经成功落盘，或者另一个 hat 已经代你发出了等价事件。盲目重发会被运行时的去重层拒绝，并在 isolated mode 下浪费唯一的业务事件槽位。
- 检查方法：`events_file="$(cat .ralph/current-events 2>/dev/null || echo .ralph/events.jsonl)"`，然后 `grep '"topic":"<TOPIC>"' "$events_file"` 并匹配关键字段（如 `plan_name`、`task_id`、`step`、`dimension` 等）。如果已存在匹配事件，**停止 emit**，等待对应的下游事件或 `task.resume`。
- **一个 turn 只发射一个业务事件**是默认纪律；去重层再强也只是兜底，不要在同一回合内通过“多发一次”来尝试修复。
- 如果某条事件被去重层拒绝并收到 `task.resume`，下一回合只发一次修正后的事件，不要在同一回合继续补发。

**NULL payload 拒收白名单**（`crates/ralph-core/src/event_policy.rs:938-948` `NULL_PAYLOAD_REJECT_TOPICS`）：以下 9 个 topic 不接受空 payload（`[PAYLOAD]` 省略 + 无 `-j`）— 必须传 JSON object：

| Topic | 出现位置 |
|-------|---------|
| `review.passed` / `review.failed` / `review.complete` | review chain 终态 |
| `work.done` / `queue.advance` | executor / step handoff |
| `review.wave.ready` | wave 入口 |
| `work.ready` / `plan.complete` / `plan.blocked` | step handoff 关键事件 |

JSON 写法：`ralph emit work.done --policy-check -j '{"plan_path": "...", "task_id": "..."}'`（dry-run 预检，不写盘；通过后去掉 `--policy-check` 再正式 emit）。

**`--policy-check` 边界**：显式 `--policy-check` 是 **dry-run 预检**——校验通过与 loop gate 同源，但**不会**把事件写入 `events.jsonl`；通过后再跑一次不带 `--policy-check` 的正式 `ralph emit` 才会落盘。配置强制 `require_policy_check_for_cli_emit: true` 时，校验通过后仍会写盘（Enforce 模式，不是 dry-run）。CLI 校验覆盖：unified validation pipeline（`event_policy.schemas.<topic>.required_fields`）、isolated hat 作用域、以及 `progress_task_gate`（`plan.complete` / `queue.advance` 的 `progress.md` ↔ `tasks.jsonl` 一致性，`emit.rs` step handoff gate）。

> **OPAC Precheck/Apply（agent context 默认 enforce）**: agent 上下文下，`ralph emit` 在无 `--policy-check` 且无 `--unsafe-no-policy-check` 写盘会被拒（U15）。`allow_unsafe_cli_emit: true` 可作为 preset opt-out（打印 deprecation warning）。详见 always-injected `ralph-tools-opac` skill Apply 段。

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
| `policy check failed` | payload 不符合策略 | 读 stderr / 用 `--output json` 取 `validation_errors[].field` 一次拿全部缺失字段；修正后用 `ralph emit <topic> --policy-check -j '...'` 预检通过再发。**不要**首选 `--unsafe-no-policy-check`（`ce-executor-pipeline` preset 默认 `allow_unsafe_cli_emit: false` 时该参数被拒） |
| `triggered_not_in_topology` | `--triggered <hat>` 不在当前 preset `hats[]` 里 | 用 `ralph hats list` 或 preset YAML 查合法 hat id；改 `--triggered` 为拓扑内 hat，或省略 `--triggered`（缺省允许）。ralph-control / orchestrator diagnostic topic 跳过此检查 |
| `agent policy-check required` (U15) | agent context + 业务 topic + 无 `--policy-check` | 先 `ralph emit <TOPIC> --policy-check -j '...'` 通过，再去掉 `--policy-check` 正式 emit。preset `allow_unsafe_cli_emit: true` 可 opt-out（deprecated warning） |
| `cannot write to events file` | 文件不存在或权限不足 | 确认 `.ralph/` 目录存在，检查权限 |
| `Invalid JSON payload` | 用 `-j` 但 payload 不是合法 JSON | 用 `jq` 验证 payload：`echo '{"a":1}' \| jq .` |
| `task_id cannot be empty` | payload 中 `task_id` 为空字符串 | 从 `.ralph/agent/tasks.jsonl` 取得真实 task id 后再 emit；任何带 `task_id` 的事件都适用 |
| `Event provenance required` | 配置要求 hat 但 `--hat` 未设且 `RALPH_CURRENT_HAT` 空 | 显式 `--hat <hat-id>` 或设置环境变量 |
| `.ralph/` 目录不存在 | 在非 ralph 工作区调用 | 确认在 ralph 编排工作区内；或 `mkdir -p .ralph` 手动初始化（不推荐） |
| `Refusing urgent steer marker` | 上轮 urgent steer 未处理 | 先处理 urgent steer 内容（参见 error 信息中的指引），再重试 emit |
| 任何命令失败 | 通用恢复 | 1. `ralph emit --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |

> **wave worker 注意**：在 `RALPH_WAVE_WORKER=1` 的子进程中通过 `ralph emit` 返回结果时，事件会写入 **candidate-events**（不是 current-events），与 wave 调度器一致。不要强行设置 `RALPH_EVENTS_FILE` 指向其他文件——会被 allowlist 拒绝。

> **诊断**：emit 拒收后无法在 CLI 层修复时，启 `RALPH_DIAGNOSTICS=1` 重新 loop；envelope 写到 `recovery.jsonl`，`ralph diagnose --session latest` 出报告。详见 `docs/guide/runtime-diagnosis.md` §10 / §12.1。

---

## Precheck Gates（事件发射 LLM 关卡）

> **不是 `ralph emit` 子功能。** `event_loop.precheck` 是 preset 配置 + runtime gate hat，与 `ralph emit --policy-check`（CLI schema 预检）无关。
>
> Loop 内出现 `<X>.proposed` / `<X>.rejected` / `precheck-<X>` 时：`ralph tools skill load ralph-tools-precheck`  
> Preset 作者启用 walkthrough：`docs/guide/precheck-gates.md`

---

## 运行时行为规范

以下规范在 loop 遇到 `task.resume` 时由 runner 自动注入（对应 `ralph-tools-recovery-directives` skill）。emit 相关操作**必须**遵守：

- **收到 `task.resume(kind=missing_event_gate)` 后**：重新 emit 同一 topic 前**必须**先用 `ralph emit --schema <TOPIC>` 确认全部 `required_fields`；**最多**重试 2 次；第 3 次仍失败则 emit `work.failed(reason="re-emit_exhausted")`。
- **收到 `task.resume(kind=stall_recovery)` 后**：stall 超过 30 秒未收到预期事件时，主动 yield 并 emit `loop.stalled`(`human.guidance` 已被 plan 2026-06-28-005 物理删除)，不要无限重发。
- **禁止**绕过 policy 直写 `.ralph/events.jsonl`；也**禁止**用 `--unsafe-no-policy-check` 作为默认修复手段。
- 更多细节见自动注入的 `## RECOVERY DIRECTIVES` 块（ID：`RD-EXECUTOR-RESEND-LIMIT`、`RD-STALL-DETECT-AND-YIELD`）。

---

## `ralph emit` 响应：`EmitResult`

> **2026-07-06 plan U1/U5/U7-U9**: `ralph emit` 通过 `--output json` 返回 **统一** `EmitResult` JSON（SSOT 在 `crates/ralph-core/src/emit_result/`）。
> 这是 agent 与脚本解析 emit 结果的单一事实源；不要自己 `tail events.jsonl | jq` 来判断 emit 是否落盘。

**启用方式**：所有 `ralph emit` 子命令 + 子路径（policy-check / apply）通过 `--output json` 输出 `EmitResult` 到 stdout，stderr 仅保留警告。
**未传 `--output`**：默认 text 模式（与旧行为兼容）。

### EmitResult 字段表

| 字段 | 类型 | 必现 | 说明 |
|------|------|------|------|
| `schema_version` | string | 是 | 恒等于 `"emit_result.v1"`。consumer 应按此版本路由解析逻辑 |
| `ok` | bool | 是 | `true` = policy 通过 + 落盘 / 仅 policy-check 通过；`false` = 拒收 |
| `recorded` | bool | 是 | **真实写盘** 信号。policy-check 阶段恒为 `false`；只有 apply 阶段成功写盘后才为 `true`。脚本判断「是否需要 reconcile」的唯一权威 |
| `topic` | string | 是 | emit 的业务 topic（如 `work.done` / `work.ready`）。拒收场景亦可填充 |
| `phase` | string | 是 | 当前 hat 所在阶段(由 preset hat 拓扑推断,非运行时 phase authority)。未识别时为 `"unknown"` |
| `allowed_next` | array<string> | 否 | 当前 hat 在 preset `publishes` 中的可发 topic 列表。**空时省略键** |
| `activate_next` | array<string> | 否 | preset 显式声明的 `activate_next` 候选。**空时省略键** |
| `errors` | array<EmitError> | 否 | 拒收场景的错误列表。**接受场景省略键** |
| `handoff` | object<EmitHandoff> | 否 | Agent 上下文交接包。**None 时省略键** |

### EmitError 字段

| 字段 | 类型 | 必现 | 说明 |
|------|------|------|------|
| `code` | string | 是 | 稳定错误码（agent 据此路由修复策略） |
| `message` | string | 是 | 人类可读错误描述 |
| `field` | string | 否 | 触发的 payload 字段名（如缺 `task_id` 时为 `"task_id"`）。`None` 时省略 |
| `suggested_command` | string | 否 | 建议 agent 执行的修复命令（含 `ralph` / 字段补全模板）。`None` 时省略 |

### EmitHandoff 字段

| 字段 | 类型 | 必现 | 说明 |
|------|------|------|------|
| `from_hat` | string | 是 | 当前 hat 稳定 id |
| `to_hat` | string | 是 | 接收交接的下游 hat 稳定 id |
| `reason` | string | 是 | 交接原因短语（preset `handoff_reasons` 表声明） |

### 路径分支矩阵

| 路径 | `ok` | `recorded` | `errors` | 备注 |
|------|------|------------|----------|------|
| `--policy-check` policy 拒收 | `false` | `false` | 非空 | agent 读 `errors[].code` 修复 |
| `--policy-check` policy 通过 | `true` | `false` | 空（省略） | dry-run 探测，可放心后续正式 emit |
| apply 落盘 | `true` | `true` | 空（省略） | 真实写盘到 events ledger |

### 示例

成功 apply:
```json
{
  "schema_version": "emit_result.v1",
  "ok": true,
  "recorded": true,
  "topic": "work.done",
  "phase": "unit_loop"
}
```

policy-check 拒收:
```json
{
  "schema_version": "emit_result.v1",
  "ok": false,
  "recorded": false,
  "topic": "work.done",
  "phase": "unknown",
  "errors": [
    {
      "code": "missing_required_field",
      "message": "payload missing required field `task_id`",
      "field": "task_id"
    }
  ]
}
```

带 handoff 的 policy-check 通过:
```json
{
  "schema_version": "emit_result.v1",
  "ok": true,
  "recorded": false,
  "topic": "work.done",
  "phase": "fix_units",
  "allowed_next": ["work.ready"],
  "handoff": {
    "from_hat": "executor",
    "to_hat": "validator",
    "reason": "fix_unit_complete"
  }
}
```

### 协议违规后的 EmitResult / correction 响应

当 emit 被 policy / execution contract / terminal guard 拒收时，用 `--output json` 读完整 `EmitResult`，**不要** `tail events.jsonl` 猜测是否落盘。

| 字段 | 拒收时如何读 | agent 动作 |
|------|-------------|-----------|
| `ok` | `false` | 事件未通过 gate；不要假设下游已激活 |
| `recorded` | 恒 `false` | 主 events **未**写入；rejected business event 不会成为事实 |
| `errors[].code` | 稳定错误码（如 `missing_required_field`、`task_not_terminal`） | 按 code 路由修复策略 |
| `errors[].field` | 触发的 payload 字段 | 补齐或修正该字段 |
| `errors[].suggested_command` | 建议命令模板（若有） | 优先采用，仍须 `--policy-check` |
| `handoff` | 拒收场景通常省略 | 不要凭 handoff 摘要推断已成功 |

**Correction 注入**：拒收后 runner 可能在下一 activation 注入 `## CORRECTION CONTEXT` 或 `task.resume`（含 `required_action` / `forbidden_action` / `target_hat` / live `task_id`+`task_key`+`step`）。规则：

1. **Correction 高于 narrative** — 只执行 correction 指定的唯一动作（见 `ralph-tools-recovery-directives`）。
2. **bounded retry** — 同类 violation signature 第一次 → correction + 一次可执行 retry；第二次 → fail-close（`protocol_violation_repeated:*`），不得 infinite retry。
3. **post-terminal** — `LOOP_COMPLETE` honored 后业务 emit 拒写，无 retry budget。
4. **修复后仍走两步 precheck** — `ralph emit <topic> --policy-check` 通过 → 去掉 `--policy-check` 正式 emit（`ralph-tools-precheck`）。

**EmitResult 与 `task.resume` 分工**：CLI 层 `EmitResult` 告诉你**本次 emit 是否落盘**；`task.resume` / correction context 告诉你**下一 activation 唯一允许动作**。两者同时出现时以 correction 的 `required_action`/`forbidden_action` 为准。

### 反模式

- 🔴 **不要** `tail events.jsonl | jq` 验证 emit 结果；用 `--output json` 直接拿到 `recorded` 字段。
- 🔴 **不要** 凭 `topic` 字符串做结果路由；用 `ok` + `recorded` 两字段组合判断（policy-check 接受 ≠ 落盘）。
- 🔴 **不要** 在脚本里 hardcode `schema_version == "emit_result.v1"` 作为版本检查；改用 `>=` / `^` 风格的 semver 比对 — schema 演进时会 bump。

---

## Unified Pipeline(U11)

`ralph emit --policy-check` 走 **unified validation pipeline**,与 loop 的 `process_parse_result` 使用同一 `ValidationPipeline::validate_pre_commit_with_view`。

> 之前的 `UNIFIED_*` env var 开关已全部移除;unified path 现在为唯一路径,不再提供 legacy compat path。

### Ledger 状态查询(U11-T1)

进程崩溃后,`StateLedger::new(workspace)` 自动调用 `replay_from_disk` 重建 `LedgerSnapshot`,恢复:
- `iteration` 计数
- `rejection_digest`(R11 retry counts)
- `handoff_accepted_log` / `handoff_tracker_log` / `flow_lifecycle_log`(U5 audit trail)
- `workflow_phases`
- counter 集合

若 ledger 损坏,降级到 `LedgerSnapshot::cold_start()` 并 `warn!` 日志。修复方式:`ralph loops clean --ledger` 截断损坏文件。

