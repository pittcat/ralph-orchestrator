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
| `--schema <TOPIC>` | string | 否 | — | 打印 `<TOPIC>` 的 embedded 协议 JSON 视图 + `protocol_hash`;**只读,不写 events.jsonl,不消耗 iteration,不触发 lint**;与 `payload` / `--json` / `--policy-check` 互斥。常见用途:检测 authoring YAML 与 embedded 协议 drift、查 `required_fields` / `is_macro_edge`。详见 [`docs/handbook/serial-preset-development.md`](../../../docs/handbook/serial-preset-development.md) §"`ralph emit --schema <TOPIC>`"。 |
| `--output <MODE>` | string | 否 | `text` | policy-check / apply 响应格式。`json` → stdout 输出 [`EmitResult`](#ralph-emit-响应emitresult) JSON；`text` → 保留旧版人类可读输出（默认） |

**Schema 模式示例：**
```bash
# 查 work.done 协议视图
ralph emit --schema work.done

# 检测 drift:build 前后 protocol_hash 必变
ralph emit --schema work.done | jq -r .protocol_hash   # 改前
# 修改对应 preset schema 后
cargo build
ralph emit --schema work.done | jq -r .protocol_hash   # 改后
```

**Agent 用法（HARD RULE）：** 在构造任何业务事件 payload 之前，**先**跑 `ralph emit --schema <topic>`，从返回的 `required_fields` 数组取得字段清单。`--schema` 的 topic 是 flag 的值；不要写成 `ralph emit <topic> --schema`。

当你正由 `ralph run` 激活为某个 **hat**（本轮角色）时，runner 已注入当前 loop 的 `RALPH_HATS_SOURCE`（hat 集合来源）和 `RALPH_CONFIG`（项目配置来源）。这两个值是当前运行上下文的权威来源：

- **不要**扫描 workspace 中的 YAML 来猜 preset。
- **不要**因为默认 `ralph.yml` 不存在就自行加 `-H builtin:<preset>`。
- **不要**用另一个 preset 的 `-H` / `--config` 覆盖 runner 注入值；这会让当前 hat、允许发布的 topic 和 schema 来自不同拓扑。
- 只有 loop 外的操作者明确指定要检查哪个 preset 时，才传 `-H <明确来源>` 或 `--config <明确文件>`。

若 `--schema` / `--policy-check` 返回 `flow_unknown_emit`、`origin:unknown_hat`，或错误中出现的 hat 与 prompt 的 `## HAT IDENTITY` 不一致，**停止 emit**并报告运行上下文不一致；不要继续试不同的 builtin preset。**不要**靠错误消息逐字段猜。`--schema` 是只读的，不写事件、不消耗 iteration。错误消息用来定位字段层级，`--schema` 用来确认完整字段集——两者配合使用。

**环境变量：**

| 变量 | 作用 |
|------|------|
| `RALPH_EVENTS_FILE` | 非空时，直接作为事件文件路径（最高优先级） |
| `RALPH_CURRENT_HAT` | 回退到 `--hat` |
| `RALPH_TRIGGERED_HAT` | 回退到 `--triggered`。在 `execution_mode: isolated` 下，loop runner 不再注入该变量；`ralph emit` 会根据当前 topic 在 preset 中的唯一下游消费者自动推导 `triggered`；推导不出时保持为空。你不需要关心当前是什么模式，按需显式传 `--triggered` 即可，显式值始终优先。 |
| `RALPH_EVENT_SOURCE` | 回退到 `--source` |

> **`triggered` 自动推导规则**：`ralph emit` 会读取当前 workspace 配置。如果当前是 isolated 模式、你已处于某个 hat 上下文中（`RALPH_CURRENT_HAT` 已设置）、且没有显式传 `--triggered` / `RALPH_TRIGGERED_HAT`，`ralph emit` 会尝试把 `triggered` 填为当前 topic 的唯一下游 hat。只有当拓扑能唯一确定真实 hat 时才填充；多消费者、无明确下游、下游由内部 runtime 汇聚、控制/诊断 topic 都会保持 `triggered` 为空。看到为空时直接执行原 emit 命令，不要猜测或显式填写内部 runtime 名称；若 policy-check 仍拒收，按反馈修正 payload 或 hat，无法确认真实目标时停止并报告。Coordinator 模式或没有配置时保持原有注入行为不变。
>
> **对 agent 来说**：不需要检测当前模式。只要记住两条规则：1) 若 prompt 明确要求你触发某个 hat， emit 时带上 `--triggered <hat>`；2) 没要求时直接 emit，runner / CLI 会自动处理。

### Policy-Check 反馈

`ralph emit --policy-check` / `ralph wave emit --policy-check` 拒收时，错误响应里**每条** `validation_errors[]` 现在带一组可机读、可修复的字段（agent 优先读这些字段再决定怎么改 payload，不要凭"error message"猜）：

| 字段 | 含义 |
|------|------|
| `field` | 触发的 payload 字段名（如 `task_id`）；空字符串表示错误在 payload / topic 层级，或错误是 `SemanticGateViolation` 类（此时读 `gate` 和 `referenced_fields`） |
| `reason_code` | 稳定错误码（`missing_required_field` / `invalid_field_value` / `payload_type_mismatch` / `terminal_monotonicity_violation` / `semantic_gate_violation` 等） |
| `message` | 人类可读描述（诊断数据，不是指令） |
| `expected` | 字段应满足的形态（allowed_values 列表 / 字段名 / payload 类型） |
| `actual` | 实际触发的值（缺字段时为 None） |
| `field_description` | `field_docs.<f>.meaning`（schema 声明时才有） |
| `suggested_payload_shape` | 已存在字段保留原值，缺失字段用 `<field>` 占位符的 JSON 骨架 —— **绝不**填业务事实（如 `0` / `pass`） |
| `suggested_command` | 修完 payload 后直接可重跑的 `ralph emit <topic> --policy-check -j '<shape>'` 命令 |
| `gate` | 当 `reason_code` 是 `semantic_gate_violation` 时，此字段携带触发的 gate 标识（如 `payload_consistency:<rule_id>` 或 `review_passed_while_wave_open`）；其它 `reason_code` 下此字段省略 |
| `referenced_fields` | 当 `gate` 是 `payload_consistency:*` 时，此字段是该规则 `when` 谓词声明的所有 payload 字段路径数组（按声明顺序去重）；agent 应检查这些字段的值是否互相矛盾。timing/state gate（如 `review_passed_while_wave_open`）下此字段为空数组。其它 `reason_code` 下此字段省略 |

**Agent 流程**：

1. 读 prompt 中的 schema-aware publish section，按 `field_docs` 填 payload。
2. 跑 `ralph emit <topic> --policy-check -j '<payload>'` 预检。
3. 拒收时先看 `reason_code`：
   - 若是 `semantic_gate_violation`：读 `gate` 判断是哪类 gate，再读 `referenced_fields` 确定要检查哪些 payload 字段。
   - 其它 `reason_code`：读 `field` / `expected` / `actual` / `field_description` / `suggested_payload_shape` / `suggested_command`。
4. 修 payload（**只**改提示的字段；**不要**复制旧 payload 重新猜字段名；**不要**从 `message` 里解析字段名）。
5. 再跑 `--policy-check`；通过后去掉 `--policy-check` 正式 emit。
6. 如果同一 hat / topic 反复触发同一类协议违规，runtime 可能阻塞 loop；不要无限重试。

**Wave batch 特殊处理**：`ralph wave emit --policy-check` 的 `validation_errors[]` 每条带 `payload_index`，对应原始 batch 的索引；整个 batch 仍 atomic reject（任何一个失败 = events.jsonl 一行都不写）。修整批后一次性重发。

**preset instructions 应引用本段**：如果 preset 的 emitter hat 会构造 payload、调用 `ralph emit` 或 `ralph wave emit`，其 `instructions` 应引用这里的字段说明；不要把字段表复制进 `instructions`。prompt 里的 schema-aware publish section 已经提供了足够的字段提示。

**新增 emitter hat 时的一致性约束**：任何会通过 `ralph emit` 或 `ralph wave emit` 发事件的 hat，`instructions` 需要引用本段；相关 lint 会检查这一点。

**Trust 边界（agent 不可伪造的字段）**：JSONL 事件里的 `system_injected` 字段**不是 agent 可声明的信任凭据**。`--policy-check` 与正式 emit 路径都会拒绝 `system_injected=true` 的业务 topic（如 `work.start` / `review.unit.done`）——只有 `<kind>.wave.{complete,failed}` 这六个 supervisor 协调 topic 可以走 system_injected runtime seam。**agent 永远不要**在事件 payload 里写 `system_injected: true` 试图绕过发布/作用域校验；这会被原 guard 拒为 `system_injected_on_business_topic`，并写进诊断。如果 hat 想发 coord topic，请走 `ralph wave` 的 merge seam（dispatcher 自动 commit `system_injected=true`），而不是手工拼字段。

### Envelope 校验（`triggered` 拓扑）

`ralph emit --triggered <hat_id>` 在 apply 路径与 `--policy-check` 路径都会被 envelope 层校验：`triggered` 字段的值必须是当前 preset 声明的 hat 之一（即出现在 `hats[]` map 里），否则返回 `triggered_not_in_topology`。

该校验按 **topic 信任层**分流（与 ralph-control / orchestrator-diagnostic / business 三层信任模型对齐）：

- **ralph-control topics**（`task.resume`、`loop.cancel`、`loop.complete`、`human.*` 等）以及 **orchestrator diagnostic topics**（`event.*`）— 跳过 topology check；runtime 注入事件时 `triggered` 经常是 `ralph` 这类 pseudo-hat，不需要在 `hats[]` 中声明。
- **business topics**（`work.done`、`queue.advance`、`review.dimension.*` 等）— 严格 topology check；`triggered` 必须是当前 preset 的 hat 之一。

`ralph emit` 默认 enforce 这条规则；如需绕开，只能用 `ralph emit --unsafe-no-policy-check`，但该参数在大多数 preset 下会被直接拒绝。

**事件文件解析优先级：**
1. 显式 `RALPH_EVENTS_FILE` 或非默认 `--file`（必须命中本 loop 的 events allowlist，否则 `ralph emit` 拒绝写入并报错；不静默回退到 marker）
2. `.ralph/current-candidate-events` marker 文件
3. `.ralph/current-events` marker 文件
4. `.ralph/events.jsonl` 默认路径

**`RALPH_WORKSPACE_ROOT` 锚定：**

事件文件路径以 `RALPH_WORKSPACE_ROOT` 为锚点（runner 已注入 `RALPH_WORKSPACE_ROOT` 和 `PWD`，hat 进程**不要 unset**）。当你 `unset RALPH_EVENTS_FILE; cd sorts/; ralph emit ...` 时，事件可能落到子目录的孤儿 events 文件。

**硬拒绝守卫（runtime 直接拒收 + stdout 摘要）：**

- `cwd_workspace_drift`：isolated mode + hat 上下文 + 未注入 `RALPH_EVENTS_FILE` + 默认 `--file` + `canonicalize(cwd) != canonicalize(workspace_root)` → 拒收。stdout 一行：`emit rejected [cwd_workspace_drift]: current_dir=... workspace_root=...`。
- `orphan_events_path`：resolved 候选路径落在 `subdir/.ralph/...`（非 workspace 根 `.ralph/` 且非 `.ralph/agent/` hat-channel）→ 拒收。
- 显式非默认 `--file` 命中 allowlist 的高级场景不受 `cwd_workspace_drift` 限制。

**反模式 / 注意事项：**
- 🔴 **禁止直写 events ledger**：必须通过 `ralph emit` / `ralph wave emit` 写入事件。直接写文件会绕过 CLI pre-publish check；loop 读盘时仍会触发事件策略校验，并以 `payload_contract_violation` 拒绝整行（最坏情况：`not_retriable` 终止 loop）。
- 🔴 **`task_id` 字段禁止空字符串**：任何包含 `task_id` 的 payload（`work.ready`、`work.done`、`test.passed`、`queue.advance` 等）必须传非空字符串，如 `task-{timestamp}-{hex}`。`"task_id":""` 会被 `ralph emit` 直接拒绝，且会破坏 step handoff / state projection。
- 🔴 **不要**在 wave worker 内部使用 `ralph emit` 发射 wave 事件；worker 应直接通过标准输出或 `ralph emit` 返回结果，而不是触发新 wave。
- 🔴 **禁止 `unset RALPH_EVENTS_FILE` 后从子目录 `cd sorts/; ralph emit ...`**：isolated hat 进程会被 `cwd_workspace_drift` 硬拒绝，事件落不到目标。若 runner 注入的 env 被破坏，恢复 `unset` 前的 env 或 `cd $RALPH_WORKSPACE_ROOT` 后再 emit。
- 🔴 **发射前确认 hat 作用域**：在 isolated 模式下，发射 topic 前必须确认当前 hat 的 `publishes` 列表包含该 topic。越权 topic 会被拒绝并触发 `task.resume`，连续 4 次越权将触发熔断终止 loop。运行 `ralph hats list` 查看当前 preset 各 hat 的 publishes。
- 🔴 `--unsafe-no-policy-check` 仅在配置显式允许时可用，否则会导致校验失败。
- 🔴 `ralph emit` **没有** `format` 选项。
- 🔴 试图通过 `RALPH_EVENTS_FILE` 或 `--file` 写入其他 worktree 的 events 文件会被 `ralph emit` 拒绝；错误信息会列出当前 allowlist。

**`EmitResult.target_path`：**

- `--output json` 路径下 `EmitResult.target_path` 字段在 `recorded: true` 时为绝对落盘路径;`recorded: false` 或拒收场景下整键被 `skip_serializing_if` 省略。
- text 模式成功行追加 `→ <absolute_path>`:例如 `Event emitted: test.passed → /home/.../.ralph/agent/events-hat-validator-001-1.jsonl`。stderr 截断场景下仍能肉眼核对落盘位置。

**Payload 字段自洽检查（runtime gate）**

`event_policy` 在 schema 校验之外，还会对 step handoff / 终态等业务事件的 payload 字段之间的**一致性**做硬检查。该 gate 在 Precheck 阶段执行，事件**不会**先落到 bus — 拒收时 `--policy-check` 与正式 apply 都返回 `validation_errors[]`，并带上 `gate: "payload_consistency:<rule_id>"` 前缀标识触发的规则族。

**触发条件**：preset 已为当前 topic 声明 `event_policy.payload_consistency.rules[]`；你提交的 payload 中某条规则的 `when` 谓词命中。**规则形状**：

- 每条 rule 是 `{id, topic, when, message}` 四字段；`message` 是命中的直观描述。
- `when` 是单谓词 `{field, op, value}` 或组合 `{all:[...]} / {any:[...]}`；`op` 限定为 `eq` / `ne` / `gt` / `gte` / `exists` / `non_empty`（其它 op 视为配置错误，runtime 直接拒收）。
- 规则只对**本次** payload 字段做检查；**不**读事件历史、不读 events ledger、不读 supervisor 状态、不读 peer topic。builtin preset 已有 fix.done 上的样例规则。

**Agent 动作**：

1. 拒收时读 `validation_errors[]` 每条的 `gate` / `referenced_fields` / `reason_code` / `message` —— 这些是 runtime 给出的可机读反馈。按 `referenced_fields` 列表检查对应 payload 字段的值是否互相矛盾，**不要**凭 `message` 字面量猜字段名。
2. 修完 payload 后再跑 `ralph emit <topic> --policy-check -j '<payload>'` 预检；通过后去掉 `--policy-check` 正式 emit。
3. 同类 violation signature（同一 `gate` 前缀 + `field` + `task_key` + step）**第 3 次**触发后 runtime 会阻塞 loop（`plan.blocked(reason=correction_3_strike_exhausted)`）。payload_consistency 拒收**不**参与协议违规重试计数（runtime 显式跳过该类拒收，不消耗重试额度）；不要无限重试。
4. **`protocol_violation_repeated:*`** 是 execution-contract 路径的 loop 阻塞标记，**不**是 payload_consistency 的阻塞标记；混用会误判修复路径。

**关键字段从哪里取得**：`validation_errors[].gate` / `referenced_fields` / `reason_code` / `message` 是 runtime 给出的可机读反馈；其它字段（`field_description` / `suggested_payload_shape` / `suggested_command`）含义见上方「Policy-Check 反馈」段。

**失败停止条件**：若你连续 3 次同类 `payload_consistency:*` 拒收，**停止**重发并按本文件「错误恢复」段处理 — runtime 已转入 `plan.blocked(reason=correction_3_strike_exhausted)`。

通用 SOFT 自洽提醒（与上述 payload_consistency gate 互补，由 schema.required_fields 与 trigger_context 评估，不由 payload_consistency gate 评估）：

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

- 在发射任何“已经由你或上游 hat 发射过”的事件之前，先用**公开只读接口**确认当前状态：loop 可能已被重启、你的上一轮可能已经成功落盘，或者另一个 hat 已经代你发出了等价事件。盲目重发会被运行时的去重层拒绝，并在 isolated mode 下浪费唯一的业务事件槽位。
- **允许的检查方法（任选，优先从上到下）：**
  1. `ralph inspect loop --format json` — 看当前 loop / hat / 投影状态
  2. `ralph tools task list` — 看 runtime task 是否已 done/failed
  3. 上一次 `ralph emit … --output json` 的 `ok` / `recorded` / `errors[]`
- **禁止**把 `.ralph/events.jsonl` / `.ralph/supervisor.db` / `.ralph/loops.json` 当业务输入：不要 `cat` / `grep` / `jq` / `tail` 这些内部 ledger 来决定是否 emit。
- 若公开接口已表明等价事件已落盘：**停止 emit**，等待下游事件或 `task.resume`。
- **一个 turn 只发射一个业务事件**是默认纪律；去重层再强也只是兜底，不要在同一回合内通过“多发一次”来尝试修复。
- 如果某条事件被去重层拒绝并收到 `task.resume`，下一回合只发一次修正后的事件，不要在同一回合继续补发。

**NULL payload 拒收白名单**：以下 9 个 topic 不接受空 payload（`[PAYLOAD]` 省略 + 无 `-j`）— 必须传 JSON object：

| Topic | 出现位置 |
|-------|---------|
| `review.passed` / `review.failed` / `review.complete` | review chain 终态 |
| `work.done` / `queue.advance` | executor / step handoff |
| `review.wave.ready` | wave 入口 |
| `work.ready` / `plan.complete` / `plan.blocked` | step handoff 关键事件 |

JSON 写法：`ralph emit work.done --policy-check -j '{"plan_path": "...", "task_id": "..."}'`（dry-run 预检，不写盘；通过后去掉 `--policy-check` 再正式 emit）。

**`--policy-check` 边界**：显式 `--policy-check` 是 **dry-run 预检**——校验通过与 loop gate 同源，但**不会**把事件写入 `events.jsonl`；通过后再跑一次不带 `--policy-check` 的正式 `ralph emit` 才会落盘。配置强制 `require_policy_check_for_cli_emit: true` 时，校验通过后仍会写盘（Enforce 模式，不是 dry-run）。CLI 校验覆盖：统一的事件策略校验、isolated hat 作用域、`progress_task_gate`（`plan.complete` / `queue.advance` 的 `progress.md` ↔ `tasks.jsonl` 一致性），以及 **P6 emit 落点 allowlist**（`RALPH_EVENTS_FILE` / `--file` 必须命中 `current-events` / `current-candidate-events` / `current-hat-events` / 默认 `events.jsonl` / dispatcher 签名的 wave channel）。`--policy-check` 与正式 emit 对落点拒绝的条件一致——预检绿了再 emit，不会因 allowlist 再挂一次。

> **OPAC Precheck/Apply（agent context 默认 enforce）**: agent 上下文下，`ralph emit` 在无 `--policy-check` 且无 `--unsafe-no-policy-check` 写盘会被拒。`allow_unsafe_cli_emit: true` 可作为 preset opt-out（打印 deprecation warning）。详见 always-injected `ralph-tools-opac` skill Apply 段。

### Confirm（普通 emit）

优先用公开反馈，不要读事件文件：

```bash
ralph emit <TOPIC> --policy-check -j '...' --output json   # 预检：看 ok / errors
ralph emit <TOPIC> -j '...' --output json                  # 落盘：看 ok=true 且 recorded=true
```

`ok` / `recorded` / `errors[]` 字段含义见下文「`ralph emit` 响应：`EmitResult`」。  
**不要**用 `tail .ralph/events.jsonl` 当作 Confirm（那是内部 ledger，不是公开接口）。

### 操作者交付文件路径（仅当本轮要交文件给操作者时）

**何时适用（必须同时满足）：**
1. 当前 hat instructions 要求你**写出**一份操作者可读文件（报告、fix-plan、blocked 说明等）；并且
2. 本 activation 会 emit 一个 **schema 要求带路径字段** 的 topic（先用 `ralph emit --schema <TOPIC>` 看 `required_fields` 里是否有 `report_path` / `artifact_path` 等路径字段）。

**何时不适用：**
- 只 emit 中间业务事件，且该 topic 的 schema **不要求**路径字段；
- 本轮不写操作者文件，只转发上游路径；
- 本 activation 只发收尾的纯完成信号，且该终态 schema **没有**路径字段——此时若上一 activation 已打印过 `DELIVERABLE_PATH`，本轮可再打印同一行提醒操作者，但**不要**伪造新路径，也**不要**往无路径字段的 schema 里塞 `report_path`。

**怎么做：**
1. **先落盘**：按 hat instructions 写出文件。
2. **确认文件可读**：
   ```bash
   test -f "<repo-relative-path>" && wc -l "<repo-relative-path>"
   ```
3. **查字段名**：对**即将 emit、且 schema 要求路径字段**的那个 topic 跑  
   `ralph emit --schema <TOPIC>`，只用返回的 `required_fields` 里的路径字段名（常见 `report_path` / `artifact_path`）。**不要两个都猜，不要自造字段名，不要把路径字段塞进 schema 未要求它的 topic。**
4. **Precheck → Apply**：先 `--policy-check`，通过后再真实 emit；路径字段值必须等于第 1 步真实文件路径（仓库相对路径，通常以 `.ralph/` 开头）。
5. **Confirm（两件事都要做，缺一不可）**：
   - 普通 emit Confirm：看 `--output json` 的 `ok` / `recorded`（见上一节）；
   - **额外**在本轮最终可见回复里单独打印一行（方便操作者在 TUI 搜索）：
     ```text
     DELIVERABLE_PATH: .ralph/.../实际文件名.md
     ```
     这一行必须与 payload 路径字段完全一致。

**重要边界（避免误解）：**
- Schema / `--policy-check` 通常只检查「路径字段是否存在且非空」，**不会**替你检查磁盘上文件是否可读。文件可读性必须你自己用第 2 步确认。
- 若 hat instructions 允许「先 `report.done`（带 `report_path`）再 `LOOP_COMPLETE`」且 `LOOP_COMPLETE` 也要求路径字段：路径字段与 `DELIVERABLE_PATH` 落在 **同一真实路径**；不要把两次 emit 写成不同值。若终态本身不要求路径字段，则不要把 `report_path` 错绑到只有 `reason` 的终态上。
- **Paired completion 字段一致性**：当 preset 启用 `completion_payload_match` 时，`LOOP_COMPLETE` 的声明字段必须与最近 accepted 的 paired topic（如 `forge.report.done`）完全一致。runtime 会拒收不一致的 completion 并注入 correction；resume 时不得重写既有报告事实来制造新的成功终态。

**停止条件：**
- 文件写不出 / 不可读 → **不要** emit 那个要求路径字段的 topic；按 hat instructions 走失败/阻塞路径（通常仍须先落一份可读的失败说明，再带真实路径 emit）。
- schema 要求路径字段但你填了不存在的路径 → 对操作者是假交付；先写文件再 emit。
- 只在心里记住路径、或只写在报告正文里却不放进 payload / 不打印 `DELIVERABLE_PATH` → 视为未完成 Confirm。

**禁止：**
- 伪造路径；
- 用读事件文件 / 内部 ledger 来「证明」交付文件存在；
- 把本节套用到每一个中间 emit 上；
- 把「打印 `DELIVERABLE_PATH`」理解成可以跳过 EmitResult Confirm。

---

## Over-Emit Recovery（isolated 模式单业务事件预算）

isolated 模式每个 activation 只允许一个业务事件最终 commit。本节解释当你多发了 event 时 runner 的实际处理，以及你怎么读懂 runner 的提示而不是凭直觉加塞。

### 触发条件
- 同一 activation 在 JSONL 里写了 **两个或更多属于本 hat `publishes:` 的业务 topic**。
- 第二（含）事件被 per-turn budget 拦截，runner 在 bus 上 publish 一条 `event.isolation.boundary_violation` 诊断（target = 当前 hat），同时视情形再追加一条 hat-targeted `task.resume`。
- 末态事件（`LOOP_COMPLETE`、`plan.blocked`、`work.failed`、其他被 preset `terminal_events` 标记的 topic）享受优先级保留：只要终态事件**作为首个 commit 业务事件**提交，额外的次级业务事件就被丢，不被认作 over-emit resume 触发条件。

### Commit-First 决策（关键）
runtime **不**在 budget 拦截那一刻决定是否发 `task.resume`；它只在 `process_parse_result` 末尾**结算最终 accepted 业务集合**之后再决定。结算规则：

- **零业务 commit**：所有候选在 origin / schema / contract gate 前后均未存活 → 记一次 bounded 拒绝，注入一条 hat-targeted `task.resume`（带 breaker 累计）；下次 activation 你应只发**一条**事件。
- **≥1 业务 commit**：第一个 commit 已锁定下游 handoff，budget 拦截的 extra 视作纯噪声 —— **不**注入 `task.resume`，你的下一轮 prompt 不会被强制改写，但会带上 `event.isolation.boundary_violation` 诊断提示你下次注意。

### 你该做什么
1. **第一轮 commit 后不要追加**：看见 `event.isolation.boundary_violation` 就**不要**试图在同一 activation 再补一条「补救 emit」，预算已经用尽。
2. **`task.resume` 到达时**：内容会包含「EXACTLY ONE business event」字样。不要把 resume payload 里的强制说明理解成「必须先解释错误」，hat instructions 允许你直接重新 emit 一条事件作为下一 activation 的回应。
3. **末态优先**：若你**确实**需要把一个事件作为终态发出（LOOP_COMPLETE、plan.blocked），把终态事件放到同一 activation 的**第一条**业务 commit；后面的次级业务事件就只是「伴随噪声」，不影响终态落地。
4. **不要**调用 `--unsafe-no-policy-check` / `allow_unsafe_cli_emit: true` 试图「绕过预算」——预算不是 policy 上的，是 `state.isolated_turn_business_event_accepted` 运行时旗，绕过会破坏下游 consumer 触发顺序。

### 反模式
- 在 `task.resume` 到达后立刻 emit 上一轮被拦的事件，结果 budget 又被消耗，可能再次 zero-commit → 第二条 resume → 死循环直到 breaker 触发 `plan.blocked(reason=protocol_violation_repeated:...)`。
- 把 `event.isolation.boundary_violation` 当成「下次记得发两条」的提示信号。
- 用 `ralph emit <TOPIC> --policy-check -j '...'` 通过后再**额外**绕开 `--policy-check` 写第二条业务事件——`--policy-check` 不控制落盘，只会通过 schema 检查。

## 错误恢复

| 错误 | 原因 | 修复 |
|------|------|------|
| `events file not in allowlist` | `RALPH_EVENTS_FILE` / `--file` 命中非 allowlist 路径 | 查看错误信息中列出的 allowlist 条目。**非 wave worker**：可去掉显式 `--file` 让 ralph emit 走 marker。**wave worker（`RALPH_WAVE_WORKER=1`）**：必须保留 runtime 注入的 `RALPH_EVENTS_FILE`，禁止 `unset` 改走 main（会触发 `wave_worker_main_fallthrough`） |
| `wave_worker_main_fallthrough` | wave worker 在仍绑定 `RALPH_WAVE_ID`/`INDEX` 时把落点解析到了 main / `current-events` | 恢复 `RALPH_EVENTS_FILE` 为 dispatcher 注入的 `.ralph/wave-<id>-<idx>.jsonl`；不要 `unset` 或改写到 main |
| `topic is required` | 缺少位置参数 | 补上 topic |
| `policy check failed` | payload 不符合策略 | 读 stderr / 用 `--output json` 取 `validation_errors[].field` 一次拿全部缺失字段；修正后用 `ralph emit <topic> --policy-check -j '...'` 预检通过再发。**不要**首选 `--unsafe-no-policy-check`（当配置未显式允许时该参数会被拒） |
| `triggered_not_in_topology` | `--triggered <hat>` 不在当前 preset `hats[]` 里 | 用 `ralph hats list` 或 preset YAML 查合法 hat id；改 `--triggered` 为拓扑内 hat，或省略 `--triggered`（缺省允许）。ralph-control / orchestrator diagnostic topic 跳过此检查 |
| `agent policy-check required` | agent context + 业务 topic + 无 `--policy-check` | 先 `ralph emit <TOPIC> --policy-check -j '...'` 通过，再去掉 `--policy-check` 正式 emit。preset `allow_unsafe_cli_emit: true` 可 opt-out（deprecated warning） |
| `cannot write to events file` | 文件不存在或权限不足 | 确认 `.ralph/` 目录存在，检查权限 |
| `Invalid JSON payload` | 用 `-j` 但 payload 不是合法 JSON | 用 `jq` 验证 payload：`echo '{"a":1}' \| jq .` |
| `task_id cannot be empty` | payload 中 `task_id` 为空字符串 | 从 `.ralph/agent/tasks.jsonl` 取得真实 task id 后再 emit；任何带 `task_id` 的事件都适用 |
| `extra business event dropped` (`event.isolation.boundary_violation`) | isolated 模式单 activation 多发业务事件 | 见下「**Over-Emit Recovery**」节，不要用回声退路 |

| `Event provenance required` | 配置要求 hat 但 `--hat` 未设且 `RALPH_CURRENT_HAT` 空 | 显式 `--hat <hat-id>` 或设置环境变量 |
| `.ralph/` 目录不存在 | 在非 ralph 工作区调用 | 确认在 ralph 编排工作区内；或 `mkdir -p .ralph` 手动初始化（不推荐） |
| `Refusing urgent steer marker` | 上轮 urgent steer 未处理 | 先处理 urgent steer 内容（参见 error 信息中的指引），再重试 emit |
| 任何命令失败 | 通用恢复 | 1. `ralph emit --help` 确认语法 2. 检查退出码 3. 查看错误信息 4. 重试 |

> **在 wave worker 子进程内调用 `ralph emit`**：当本进程的 `RALPH_WAVE_WORKER=1` 时，runtime 已为你准备好本次 slot 的事件落点（由 `RALPH_EVENTS_FILE` 指向）。**强制先 `--policy-check` 预检**：
>
> ```bash
> ralph emit --policy-check --hat "$RALPH_CURRENT_HAT" -j '<payload>' "$TOPIC"   # 预检
> ralph emit --hat "$RALPH_CURRENT_HAT" -j '<payload>' "$TOPIC"                  # 真正写盘
> ```
>
> **禁止**：
>
> - `unset RALPH_EVENTS_FILE` 或改写到其它目录 —— 事件不会落到本次 slot，runtime 会判定本 slot 无结果产出。
> - 改写 `RALPH_WAVE_WORKER` / `RALPH_WAVE_ID` / `RALPH_WAVE_INDEX` —— 这三个值由 runtime 在你启动时注入，是本次 slot 的身份凭据。
>
> **Wave worker 预检顺序**（同一进程内一次 `ralph emit` 走的检查链）：
>
> 1. **policy check** —— 按当前 `event_policy` schema 与 origin guard 校验 payload 与 topic；拒收则 bail，不进入下一步
> 2. **resolve emit path** —— 把 `RALPH_EVENTS_FILE` / `--file` / marker / 默认 `events.jsonl` 解析为具体落点；wave worker 路径上进一步走 wave 通道 registry 校验，身份必须四元组匹配（loop id + wave id + slot index + canonical path）
> 3. **write** —— 落到校验通过的路径
>
> 任一阶段失败 bail 后不会进入下一阶段；wave worker 在阶段 2 被拒时，本 slot 表现为「无结果产出」，runtime 会把它归类为 fan-in missing（不要靠补发或重试绕过）。
>
> **`wave_channel_registry_reject` 错误码**（wave worker 在阶段 2 收到的稳定错误码）：
>
> **触发条件**：wave worker 的 emit 路径中，registry 校验环节发现当前 `(loop id, wave id, slot index, canonical path)` 与 dispatcher 在准备阶段写入的 wave channel registry 不匹配。可能的原因（按概率从高到低）：
>
> - dispatcher 还没来得及写入本 wave 的 registry（spawn 比准备快）—— 这种情况下即便 `--policy-check` 已通过，仍会在阶段 2 拒收
> - worker 进程被外层注入的 `RALPH_CURRENT_LOOP_ID` / `RALPH_EVENTS_FILE` 污染（human CLI hat env leakage）
> - dispatcher 准备阶段失败（registry 文件创建 / 写盘 / 回读校验失败），整波准备失败，worker 不应被启动但已启动
>
> **Agent 动作**（按顺序，不可跳过）：
>
> 1. **停** —— 不要重试同 payload 的 `ralph emit`。本次 emit 已被 runtime 拒收，本 activation 内再次发同一事件只会得到同样的错误码，浪费 isolated 单业务事件预算
> 2. **看 dispatcher 输出** —— 本 wave 启动时 dispatcher 的 stderr / stdout 包含 registry 准备阶段的状态码（`registry_prepared` / `registry_preparation_failed`）。若 dispatcher 报告 `registry_preparation_failed`，本 wave 已被 runtime 标记为 fan-in 失败
> 3. **不重试** —— 不再发 `ralph emit`，不要尝试改 `--file` / `RALPH_EVENTS_FILE` 让它落到其它路径。emit 阶段 2 失败是 dispatcher 端故障，不是 worker 端问题
> 4. **报告** —— 在本 activation 的最终回复里，向操作者或上游 hat 报告「dispatcher 没准备好本 slot 的私有通道」，由 dispatcher 端决定是否整波重试或标记 wave 失败
>
> **停止条件**：收到 `wave_channel_registry_reject` 后**禁止**做以下动作：
>
> - 重试 `ralph emit` 同 payload / 同 topic（只会同样被拒）
> - `unset RALPH_EVENTS_FILE` 改走默认 events.jsonl（wave worker 的 marker fallthrough 已删除，会被 `wave_worker_main_fallthrough` 拒收）
> - 改写 `RALPH_WAVE_ID` / `RALPH_WAVE_INDEX` / `RALPH_CURRENT_LOOP_ID`（这三个是 dispatcher 注入的身份凭据；改写等同于伪造 slot 身份）
> - 把本 hat 的「发业务事件」动作改成手工 cat / echo 到 `.ralph/events.jsonl`（loop 读盘时仍会被策略校验拒为 `payload_contract_violation`，并终止 loop）

> **诊断**：emit 拒收后无法在 CLI 层修复时，启 `RALPH_DIAGNOSTICS=1` 重新 loop；envelope 写到 `recovery.jsonl`，`ralph diagnose --session latest` 出报告。详见 `docs/guide/runtime-diagnosis.md` §10 / §12.1。

---

## Precheck Gates（事件发射 LLM 关卡）

> **不是 `ralph emit` 子功能。** `event_loop.precheck` 是 preset 配置 + runtime gate hat，与 `ralph emit --policy-check`（CLI schema 预检）无关。
>
> Loop 内出现 `<X>.proposed` / `<X>.rejected` / `precheck-<X>` 时：`ralph tools skill load ralph-tools-precheck`  
> Preset 作者启用 walkthrough：`docs/guide/precheck-gates.md`

> **Trigger Context 不替代 `--policy-check`。** Trigger Context 是消费方 prompt context（帮助下游 hat 读懂本轮 trigger payload），与 `ralph emit --policy-check`（写盘前 schema 预检）独立。发事件仍必须先 `--policy-check` 通过，再去掉 `--policy-check` 真正写盘；Trigger Context 只告诉你怎么理解上一轮事件，不替你验证下一条 emit 的形状。

---

## 运行时行为规范

以下规范在 loop 遇到 `task.resume` 时由 runner 自动注入（对应 `ralph-tools-recovery-directives` skill）。emit 相关操作**必须**遵守：

- **收到 `task.resume(kind=missing_event_gate)` 后**：重新 emit 同一 topic 前**必须**先用 `ralph emit --schema <TOPIC>` 确认全部 `required_fields`；**最多**重试 2 次；第 3 次仍失败则 emit `work.failed(reason="re-emit_exhausted")`。
- **收到 `task.resume(kind=stall_recovery)` 后**：stall 超过 30 秒未收到预期事件时，主动 yield 并 emit `loop.stalled`（`human.guidance` 已不再是有效 emit 目标），不要无限重发。
- **禁止**绕过 policy 直写 `.ralph/events.jsonl`；也**禁止**用 `--unsafe-no-policy-check` 作为默认修复手段。
- 更多细节见自动注入的 `## RECOVERY DIRECTIVES` 块（ID：`RD-EXECUTOR-RESEND-LIMIT`、`RD-STALL-DETECT-AND-YIELD`）。

---

## `ralph emit` 响应：`EmitResult`

> `ralph emit` 通过 `--output json` 返回 **统一** `EmitResult` JSON。
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
2. **bounded retry** — 同类 violation signature 第一次 → correction + 一次可执行 retry；第二次 → 阻塞 loop（`protocol_violation_repeated:*`），不得 infinite retry。
3. **post-terminal** — `LOOP_COMPLETE` honored 后业务 emit 拒写，无 retry budget。
4. **修复后仍走两步 precheck** — `ralph emit <topic> --policy-check` 通过 → 去掉 `--policy-check` 正式 emit（`ralph-tools-precheck`）。

**EmitResult 与 `task.resume` 分工**：CLI 层 `EmitResult` 告诉你**本次 emit 是否落盘**；`task.resume` / correction context 告诉你**下一 activation 唯一允许动作**。两者同时出现时以 correction 的 `required_action`/`forbidden_action` 为准。

### 反模式

- 🔴 **不要** `tail events.jsonl | jq` 验证 emit 结果；用 `--output json` 直接拿到 `recorded` 字段。
- 🔴 **不要** 凭 `topic` 字符串做结果路由；用 `ok` + `recorded` 两字段组合判断（policy-check 接受 ≠ 落盘）。
- 🔴 **不要** 在脚本里 hardcode `schema_version == "emit_result.v1"` 作为版本检查；改用 `>=` / `^` 风格的 semver 比对 — schema 演进时会 bump。

---

## Unified Pipeline

`ralph emit --policy-check` 与 loop 内的统一校验管线行为一致。

### 进程崩溃后恢复

进程崩溃后，loop 重启时自动从 events 文件重建状态：迭代计数、rejection 重试计数、handoff 审计轨迹、workflow phase 与 counter 集合。

若状态文件损坏导致重建失败，loop 会降级到冷启动并打印 warning。修复方式：`ralph loops clean --ledger` 截断损坏文件后重试。
