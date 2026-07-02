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
| `--policy-check` | flag | 否 | — | 发射前按当前事件策略校验 |
| `--unsafe-no-policy-check` | flag | 否 | — | 跳过强制策略检查（仅当配置允许时） |
| `--hat <HAT>` | string | 否 | `$RALPH_CURRENT_HAT` | 发布此事件的 hat |
| `--triggered <TRIGGERED>` | string | 否 | `$RALPH_TRIGGERED_HAT` | 被此事件触发的目标 hat |
| `--source <SOURCE>` | string | 否 | `$RALPH_EVENT_SOURCE` | 事件来源标识 |
| `--schema <TOPIC>` | string | 否 | — | 打印 `<TOPIC>` 的 embedded 协议 JSON 视图 + `protocol_hash`(U5 / R6,plan 2026-06-20-001);**只读,不写 events.jsonl,不消耗 iteration,不触发 lint**;与 `payload` / `--json` / `--policy-check` 互斥。常见用途:检测 authoring YAML 与 embedded 协议 drift、查 `required_fields` / `is_macro_edge`。详见 [`docs/handbook/serial-preset-development.md`](../../../docs/handbook/serial-preset-development.md) §"`ralph emit --schema <TOPIC>`"。 |

**Schema 模式示例：**
```bash
# 查 work.done 协议视图
ralph emit --schema work.done

# 检测 drift:build 前后 protocol_hash 必变
ralph emit --schema work.done | jq -r .protocol_hash   # 改前
# 改 presets/schemas/ce-executor-serial.yml 后
cargo build
ralph emit --schema work.done | jq -r .protocol_hash   # 改后
```

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
- 🔴 **禁止直写 `.ralph/events.jsonl`**：必须通过 `ralph emit` / `ralph wave emit` 写入事件。直接 `echo ... >> .ralph/events.jsonl` 或 heredoc 写入会绕过 CLI pre-publish check；loop 读盘时仍会触发 `event_policy` 校验，并以 `payload_contract_violation` 拒绝整行（最坏情况：`not_retriable` 终止 loop）。文档：`docs/plans/2026-06-15-001-feat-schema-aware-hat-emit-instructions-plan.md` §4.6。
- 🔴 **`task_id` 字段禁止空字符串**：任何包含 `task_id` 的 payload（`work.ready`、`work.done`、`test.passed`、`queue.advance` 等）必须传非空字符串，如 `task-{timestamp}-{hex}`。`"task_id":""` 会被 `ralph emit` 直接拒绝，且会破坏 step handoff / state projection。
- 🔴 **不要**在 wave worker 内部使用 `ralph emit` 发射 wave 事件；worker 应直接通过标准输出或 `ralph emit` 返回结果，而不是触发新 wave。
- 🔴 **发射前确认 hat 作用域**：在 isolated 模式下，发射 topic 前必须确认当前 hat 的 `publishes` 列表包含该 topic。越权 topic 会被拒绝并触发 `task.resume`，连续 4 次越权将触发熔断终止 loop。运行 `ralph hats list` 查看当前 preset 各 hat 的 publishes。
- 🔴 `--unsafe-no-policy-check` 仅在配置显式允许时可用，否则会导致校验失败。
- 🔴 `ralph emit` **没有** `format` 选项。
- 🔴 试图通过 `RALPH_EVENTS_FILE` 或 `--file` 写入其他 worktree 的 events 文件会被 `ralph emit` 拒绝；错误信息会列出当前 allowlist。

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

**NULL payload 拒收白名单**（`crates/ralph-core/src/event_policy.rs:907-917` `NULL_PAYLOAD_REJECT_TOPICS`）：以下 9 个 topic 不接受空 payload（`[PAYLOAD]` 省略 + 无 `-j`）— 必须传 JSON object：

| Topic | 出现位置 |
|-------|---------|
| `review.passed` / `review.failed` / `review.complete` | review chain 终态 |
| `work.done` / `queue.advance` | executor / step handoff |
| `review.wave.ready` | wave 入口 |
| `work.ready` / `plan.complete` / `plan.blocked` | step handoff 关键事件 |

JSON 写法：`ralph emit work.done --policy-check -j '{"plan_path": "...", "task_id": "..."}'`（事件文件已存在 + allowlist 命中）。

**`--policy-check` 边界**：CLI `--policy-check` 与 `ralph run` 循环内统一校验管线 `validation::rules_event_policy::EventPolicyRule` 行为**同源**（都走 `event_policy.schemas.<topic>.required_fields`），但**不覆盖** step handoff 的 `progress_task_gate`（`progress.md` ↔ `tasks.jsonl` 一致性）。完整预检（含 `progress_task_gate` 在 CLI 入口预检）见计划 `docs/plans/2026-06-17-005-fix-agent-recovery-mechanism-gaps-plan.md`。

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
| `policy check failed` | payload 不符合策略 | 读 stderr / 用 `--output json` 取 `validation_errors[].field` 一次拿全部缺失字段；修正后用 `ralph emit <topic> --policy-check -j '...'` 预检通过再发。**不要**首选 `--unsafe-no-policy-check`（`ce-executor-serial` preset 默认 `allow_unsafe_cli_emit: false` 时该参数被拒） |
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

> 2026-07-02-004 计划（U1–U6）。**opt-in**：未声明 `event_loop.precheck` 一切行为与今天字节等价；出厂 builtin preset 默认不带 precheck。

### 启用与关停

```yaml
event_loop:
  precheck:
    enabled: true
    rules:
      <TOPIC>:                                # 被守 topic X
        prompt:                               # 渲染进 gate hat 的 checklist
          - "检查 1: ..."
          - "检查 2: ..."
        on_fail:
          target: <hat_id>                    # 打回的目标 hat（接 task.resume）
          retry_budget: 3                     # 默认 3，与 mechanism.repair_budget 对齐
          on_exhausted: "plan.blocked(reason=precheck_failed)"
          reason: "short human-readable"
```

- **环境变量 kill switch**：`RALPH_PRECHECK_MODE=off` 让整个 precheck 块**严格 no-op**（即使配置 `enabled: true` 也不脱糖）。用于紧急停用 gate 而不动 YAML。
- **脱糖**（`RalphConfig::apply_precheck_desugar`）：每个 `rules.<X>` 把所有 `publishes/terminal_events` 含 `X` 的 producer 改写为发 `X.proposed`，并合成一个 `precheck-<X>` hat：triggers=`[X.proposed]`、publishes=`[X, X.rejected]`、terminal_events=`[X, X.rejected]`。
- **失败闭环**（U6）：gate 发 `X.rejected` → runtime 注入 `task.resume(target=on_fail.target)`；连续 `retry_budget` 次仍未过 → 发 `on_exhausted`（默认 `plan.blocked(reason=precheck_failed)`）。`precheck_failed` 已加入 `presets/schemas/ce-executor-serial.yml` 的 `plan.blocked.allowed_values.reason` 白名单。

### agent 在 loop 里会看到的新 topic

| Topic | 谁发 | 何时 | agent 应该做什么 |
|-------|------|------|------------------|
| `<X>.proposed` | producer（脱糖后） | producer 想发 `<X>` 时 | **不要处理**——gate hat 会消费 |
| `<X>` | `precheck-<X>` gate | gate LLM 通过 checklist | 正常处理 |
| `<X>.rejected` | `precheck-<X>` gate | gate LLM 不通过 | **不要处理**——runtime 已自动注入 `task.resume` 打回 producer；如果看到连续 `<X>.rejected` 说明 `retry_budget` 即将耗尽，不要自己重发 `<X>` |

### 反模式

- ❌ 把机械检查（如 git diff、test pass、task 状态）塞进 `precheck.rules.<X>.prompt`——那是确定性门的职责（plan R11）。precheck 只做主观 checklist。
- ❌ 多个 producer topic 同时挂同一个 `<X>.rejected` 路由——一个 rule 的 `on_fail.target` 是单数；多 producer 各自一个 rule。
- ❌ 试图手动 emit `X.proposed`（绕过 producer 改写）——runtime 会拒收。

---

## 运行时行为规范

以下规范在 loop 遇到 `task.resume` 时由 runner 自动注入（对应 `ralph-tools-recovery-directives` skill）。emit 相关操作**必须**遵守：

- **收到 `task.resume(kind=missing_event_gate)` 后**：重新 emit 同一 topic 前**必须**先用 `ralph emit --schema <TOPIC>` 确认全部 `required_fields`；**最多**重试 2 次；第 3 次仍失败则 emit `work.failed(reason="re-emit_exhausted")`。
- **收到 `task.resume(kind=stall_recovery)` 后**：stall 超过 30 秒未收到预期事件时，主动 yield 并 emit `loop.stalled`(`human.guidance` 已被 plan 2026-06-28-005 物理删除)，不要无限重发。
- **禁止**绕过 policy 直写 `.ralph/events.jsonl`；也**禁止**用 `--unsafe-no-policy-check` 作为默认修复手段。
- 更多细节见自动注入的 `## RECOVERY DIRECTIVES` 块（ID：`RD-EXECUTOR-RESEND-LIMIT`、`RD-STALL-DETECT-AND-YIELD`）。

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

