# Finding Rubric

Review skill 将 mechanical lint 与软性 AAF 缺口映射为 P0/P1/P2 + confidence。

**入表门槛：** `confidence ≥ 60`（见 `ralph-preset-review` SKILL）。

**机械 lint 默认 confidence：** Error → 95；Warn → 85。

**软性 AAF 起点：** ≤ 50，须验证后上调。

**软性 Payload Audit 起点：** ≤ 50，须验证后上调。

## AAF 缺口 → Severity（软性）

| 缺口 | Severity | category | aaf_question |
|---|---|---|---|
| Q2 缺 Observe 路径 | P0 | feasibility | Q2 |
| Q3 命令不存在 / 读 ledger / 跳过 precheck | P0 | feasibility | Q3 |
| Q4 topic 不在 publishes / 单事件预算违反 | P0 | feasibility | Q4 |
| Q4 `--triggered` 指向未声明 hat | P0 | feasibility | Q4 |
| Q5 handoff 字段未 emit 或未投影 | P0 | handoff | Q5 |
| isolated 下假设其它 hat 行为 | P0 | visibility | Q2 |
| Q3 缺 Confirm 或 OPAC skill 引用 | P1 | opac | Q3 |
| Q3 recovery 散文无状态表 / 缺 bounded retry 引用 | P1 | opac | Q3 |
| data skill doc 含 preset 专用 hat 名或拓扑 | P1 | style | Q3 |
| Q1 不可判定 | P1 | feasibility | Q1 |
| Q5 弱对齐（能跑易漂移） | P1 | handoff | Q5 |
| `review.dimensions.complete` 已 publish 但 `state_projection` 无对应 action | P1 | state | Q5 |
| 框架术语堆砌 | P1 | style | Q1 |
| 命名 / 冗余 hat / instructions 过长 | P2 | style | Q1 |

## Payload Audit → Severity（软性）

`ralph emit --schema` 与 `--policy-check` 只验 shape；字段可见性 / 值源 / 身份 / 语义 / 下游消费由 review 把关。

| 缺口 | Severity | category | aaf_question |
|---|---|---|---|
| emit 字段 hat 不可见（无 Observe / 无 projection） | P0 | payload-content | Q4 |
| `task_id` / `task_key` / `step` 手写而非 live 取得 | P0 | payload-content | Q4 |
| emit 引用未声明字段（schema 通过但 hat 拿不到值） | P0 | payload-content | Q4 |
| 决策字段值与下游语义不匹配（`summary: done` 类） | P1（若阻塞下游执行则升 P0） | payload-content | Q4 / Q5 |
| 多 trigger hat 未按 trigger 拆分 payload 差异 | P1 | payload-content | Q4 |
| loop preset 的 `fix.done.next_review_plan` 允许 `null` 或缺少下一轮 review 所需数组字段 | P0 | payload-content | Q4 / Q5 |
| payload audit 行缺值源 / 缺可见性证据 | P1 | payload-content | Q4 |
| 同一 hat emit 多条业务事件（违反单事件预算） | P0（**例外**：见下文「required-event-to-completion 窄例外」复核） | feasibility | Q4 |
| 终态 emit 前夹带其它业务事件 | P0（**例外**：见下文「required-event-to-completion 窄例外」复核） | feasibility | Q4 |
| report finding 无 repair surface（无字段 / 无 source / 无 fix） | 拒入主表 → Unverified Suspicions | — | — |

## Artifact-First Handoff → Severity（软性 / review-only）

按 `docs/plans/2026-07-16-003-feat-preset-artifact-first-handoffs-plan.md` Product Contract，重要信息默认必须由实际执行的 hat 或其 sub-agent 写入当前 `.ralph/` 下的业务 artifact；event payload 仅承担控制面（短状态、摘要、路径、必要身份、路由字段）。这些缺口由 review-only AAF 独立审，**不进** `ralph preset check --format json`（详见文末「lint vs review-only」段）。

| 缺口 | Severity | category | aaf_question | 备注 |
|---|---|---|---|---|
| event payload 直接携带完整结果正文（可恢复 / 长内容 / 跨 hat 摘要）而未落盘 | P0 | payload-content | Q4 | 例 AE1 |
| emitter hat `instructions` 未要求「先写 artifact 再 emit」 | P1 | feasibility | Q3 | 改写 instructions 即可修复 |
| consumer hat `instructions` 未要求「从路径读完整内容」 | P0（若阻塞下游执行则固定 P0） / P1 | payload-content | Q2 / Q5 | 例 AE2 consumer 分支 |
| artifact 路径来源对消费 hat 不可见（不在 trigger payload / projection / task view / `## TRIGGER CONTEXT`） | P0 | visibility | Q2 | 例 AE2 path-invisible 分支 |
| artifact 路径被多次声明但未指定消费方 / 保留 / 归档 / 清理责任 | P1 | payload-content | Q5 | 例 AE4 |
| preset 自身被描述为创建 artifact 的角色（实际由 hat 在 activation 创建） | P0 | style | Q3 | 重写 instructions，明确「由 X hat / X sub-agent 写入」 |
| hat `instructions` 要求把 `.ralph/events.jsonl` / `.ralph/loops.json` / `.ralph/supervisor.db` 当作业务 artifact | P0 | visibility | Q2 | 改写或参考 `preset.instructions_read_internal_ledger`；业务 artifact 必须落到 `.ralph/<plan>/<unit>/...` 等业务子目录 |
| 「不落盘」例外的理由仅为字符数 / 不具体 | P1（若阻塞下游则升 P0） | payload-content | Q4 | 例 AE5；理由必须基于恢复价值 / 审计价值 / 下游依赖 |
| passing verdict 仅因 payload 含路径字段就放行（未验证路径可见性 / 内容语义 / 消费动作） | P1（若影响终态判定则升 P0） | payload-content | Q4 / Q5 | 例 AE2；R12 强制要求路径 + 可见性 + 消费三方同时成立 |
| artifact 落盘与读取分散在多处 hat，但 preset 没有显式声明生命周期（owner / reader / retention / cleanup） | P1 | state | Q5 | 例 AE4 |
| sub-agent 完整结果通过 hat 长消息返回（未在 `.ralph/` 下落盘） | P0 | payload-content | Q4 | 例 AE1 |
| artifact 已读但内容不足以支撑 consumer Q1 决策（缺证据 / 缺结论 / 无法恢复） | P0（阻塞下游） / P1 | payload-content | Q1 / Q5 | R8 内容充分性；与「仅有路径」区分 |

## Artifact-First Handoff `field_docs` 审核点

`event_policy.schemas.<topic>.field_docs.<path_field>` 必须满足（参考 `agent-native-model.md` 的 Payload Audit 模型）：

- **`meaning`**：描述「该路径是 artifact 落盘点，值为相对 `.ralph/` 的路径」，而不是笼统的「handoff path」「output path」。必须能让 emitting hat 在不读其它 hat instructions 的前提下知道它是磁盘文件引用、不是结构化数据。
- **`source`**：必须指向当前 hat 可见输入——trigger payload 字段、`state_projection.actions` 投影后的字段、本 hat work 输出（自己写到 `.ralph/` 后再回填）。**禁止**指向其它 hat 内部状态 / runtime internal ledger。
- **`fill_rule`**：必须说明 agent 在路径未提供时如何计算（按 plan/unit 命名约定拼接）或拒绝（policy-check fail），**不得诱导** agent 伪造路径或抄业务文件名。
- **`examples[]`**：任何路径示例都应是结构占位（如 `.ralph/<plan>/<unit>/<file>.md`），不得固定具体业务文件名（避免 agent 把示例抄成真实路径后伪造写入）。

## Policy-Check Feedback → Severity

这些缺口不一定改变 runtime 接受 / 拒绝语义，但会让 agent 无法从 `--policy-check` 拒收中自修复。

| 缺口 | Severity | category | aaf_question |
|---|---|---|---|
| required handoff / identity / decision 字段缺 `field_docs`，且 injected skill 没有覆盖字段含义 | P1（导致 agent 反复拒收则升 P0） | policy-feedback | Q3 / Q4 |
| `field_docs.<field>.source` 与 Payload Audit 的可见值源冲突 | P1（会诱导伪造 live identity 则 P0） | policy-feedback | Q4 |
| `field_docs.<field>.fill_rule` 要求 agent 猜测、默认填业务事实或手写 live id | P0 | policy-feedback | Q4 |
| `examples[]` 固化业务结论（如固定 `pass` / `0`）且 agent 可能复制为事实 | P1（影响终态 / gate 判定则 P0） | policy-feedback | Q4 / Q5 |
| emitter instructions 提到 payload / emit / required fields 但未引用 `ralph-tools-emit` Policy-Check feedback | P1 | lint | Q3 |
| emitter instructions 引用 on-demand `ralph-tools-emit` 但未要求在本 activation 先执行 `ralph tools skill load ralph-tools-emit` | P1 | prompt-visibility | Q3 |
| policy-check JSON 缺 `payload_index` 导致 wave batch 无法定位失败 item | P1（batch 阻塞主路径则 P0） | policy-feedback | Q3 |

## finding_id 映射表（curated）

| finding_id（裸 ID / JSON 为 `lint.` + 此列） | default_severity | default_confidence | aaf_question | category |
|---|---|---|---|---|
| `preset.multi_hat_requires_isolated` | P0 | 95 | Q3 | lint |
| `preset.instructions_read_internal_ledger` | P0 | 95 | Q3 | lint |
| `preset.instructions_opac_skill_reference_missing` | P1 | 85 | Q3 | lint |
| `preset.instructions_emit_feedback_skill_reference_missing` | P1 | 80 | Q3 | lint |
| `preset.instructions_emit_skill_load_missing` | P1 | 85 | Q3 | prompt-visibility (review-only) |
| `preset.instructions_task_create_literal` | P1 | 85 | Q3 | lint |
| `preset.instructions_supervisor_coordination_topic` | P0 | 95 | Q4 | lint |
| `preset.handoff_pairing_broken` | P0 | 95 | Q5 | lint |
| `preset.handoff_seed_derived_conflict` | P0 | 95 | Q5 | lint |
| `preset.trigger_publish_asymmetry` | P1 | 85 | Q5 | lint |
| `preset.re_emit_trap` | P0 | 95 | Q4 | lint |
| `preset.activation_egress_missing` | P0 | 95 | Q4 | lint |
| `preset.state_projection_work_done_order` | P1 | 85 | Q5 | state |
| `preset.publishes_missing_schema` | P0 | 95 | Q4 | lint |
| `preset.schema_reference_parity` | P0 | 95 | Q4 | lint |
| `preset.instructions_schema_required_fields_drift` | P0 | 95 | Q4 | policy-feedback |
| `preset.owner_not_publisher` | P1 | 85 | Q4 | topology |
| `preset.cross_hat_unauthorized_publish` | P1 | 85 | Q4 | topology |
| `preset.owner_unknown_hat` | P0 | 95 | Q4 | topology |
| `preset.invalid_topic_format` | P1 | 85 | Q4 | topology |
| `preset.terminal_dual_subscribe` | P0 | 95 | Q4 | topology |
| `preset.terminal_publisher_incomplete` | P0 | 95 | Q4 | topology |
| `preset.hat_scope_event_filter_disabled` | P0 | 95 | Q3 | visibility |
| `preset.hat_scope_topic_deny_incomplete` | P1 | 85 | Q3 | visibility |
| `preset.hat_scope_coordinator_review_leak` | P0 | 95 | Q2 | visibility |
| `preset.flow_declaration_missing` | P1 | 85 | Q4 | topology |
| `preset.flow_unknown_emit_rejected` | P0 | 95 | Q4 | topology |
| `preset.flow_linear_positional_ambiguity` | P0 | 90 | Q4 | topology |
| `preset.supervisor_requires_isolated` | P0 | 95 | Q3 | lint |
| `preset.supervisor_hat_publishes_coord_topic` | P0 | 95 | Q4 | lint |
| `preset.supervisor_wave_consumer_low_concurrency` | P0 | 95 | Q3 | lint |
| `preset.metadata_runtime_drift` | P1 | 85 | — | lint |
| `preset.dimension_reviewer_write_plan` | P0 | 95 | Q3 | lint |
| `preset.trigger_context_unknown_field` | P0 | 90 | Q4 | lint |
| `preset.trigger_context_unsupported_predicate` | P0 | 90 | Q4 | lint |
| `preset.trigger_context_value_shape` | P0 | 90 | Q4 | lint |
| `preset.trigger_context_duplicate_label` | P0 | 90 | Q4 | lint |
| `preset.trigger_context_no_consumer` | P0 | 90 | Q4 | topology |
| `preset.payload_consistency_duplicate_id` | P0 | 90 | Q4 | lint |
| `preset.payload_consistency_unknown_topic` | P0 | 90 | Q4 | lint |
| `preset.payload_consistency_unknown_field` | P0 | 90 | Q4 | lint |
| `preset.payload_consistency_unknown_op` | P0 | 90 | Q4 | lint | `when.op` 不在 `eq`/`ne`/`gt`/`gte`/`exists`/`non_empty` 白名单 |
| `preset.payload_consistency_non_object_when` | P0 | 90 | Q4 | lint | `when` 不是 object（单谓词或 `all`/`any` 组合） |
| `preset.payload_consistency_unsafe_message` | P0 | 90 | Q4 | lint | `rule.message` 含 ANSI escape / C0/C1 控制字符 / 零宽字符 / 长度超过 1024 UTF-8 bytes |
| `preset.instructions_task_mutation_authority_conflict` | P0 | 90 | Q5 | lint | hat `instructions` 在 projector-owned batch action 或非 coordinator 角色下仍要求 `ralph tools task add` / `task ensure`；单写者冲突由 lint 兜底 |

### Key-stage event gate finding_id（review-only，lint 不直接产出）

按 `docs/plans/2026-08-05-007-feat-preset-author-key-stage-event-gates-plan.md` Product Contract（R1-R8 + AE1-AE6）补充的 review-only finding。**这些 ID 不会出现在 `ralph preset check --format json`**——lint 类无法从「per-key-stage guard 选择 + 各自 budget + 确认状态」多字段组视角判断，必须靠 reviewer 在第 4 / 5 步 AAF + Payload Audit 独立审。

| finding_id（裸 ID / JSON 不出现） | default_severity | default_confidence | aaf_question | category | 备注 |
|---|---|---|---|---|---|
| `preset.key_stage_event_gate_missing_selection` | P0 | 90 | Q3 | feasibility | review-only；关键位置未在 notes 记录 4 选 1 guard 选择（例 AE1 / AE5） |
| `preset.key_stage_event_gate_field_reuse` | P0 | 95 | Q3 | lint | review-only；`guard_selection` / `precheck_guard` / `payload_consistency_guard` / `precheck_retry_budget` / `payload_consistency_retry_budget` 字段被复用 `Gate Scope` 的 `hard/record/off` 字段，或反之；语义完全不同但 owner 错配 |
| `preset.key_stage_event_gate_shared_budget` | P0 | 90 | Q3 | feasibility | review-only；`precheck_retry_budget` 与 `payload_consistency_retry_budget` 被合并为一个 `retry_budget` 或共享 exhaustion state（例 AE4） |
| `preset.key_stage_event_gate_no_reason` | P1（阻塞下游则升 P0） | 80 | Q4 | payload-content | review-only；选 `neither` 或 budget < 3 的位置 `reason` 为空 / 「用户偏好」 / 「先这样」等空话（例 AE4） |
| `preset.key_stage_event_gate_unsupported_runtime_rule` | P0 | 95 | Q3 | feasibility | review-only；author 借 0e 段落新增 runtime 配置、计数器、恢复路径或绕过 guard 替代行为（违反 R8） |
| `preset.key_stage_event_gate_notes_preset_diverge` | P0 | 90 | Q3 | handoff | review-only；notes 记录的选择与 YAML 实际 guard 字段不一致（例 AE4）：preset 实际有 `event_loop.precheck.rules` 但 notes 记 `neither`；或反之 |
| `preset.key_stage_event_gate_pending_status` | P0 | 90 | Q3 | feasibility | review-only；存在 `confirmation_status ∈ {pending, rejected}` 的关键位置，但 author 仍后续生成依赖它的 YAML / schema（违反 R6 / AE5） |
| `preset.key_stage_event_gate_single_combined_choice` | P0 | 95 | Q3 | feasibility | review-only；author 用一个 preset 全局开关替代 per-position 询问（违反 R4 / AE1） |

### Artifact-First Handoff finding_id（review-only，lint 不直接产出）

按 `docs/plans/2026-07-16-003-feat-preset-artifact-first-handoffs-plan.md` Product Contract（R2 / R3 / R4 / R5 / R6 / R7 / R9 / R10 / R11 / R12 + AE1-AE5）补充的 review-only finding。**这些 ID 不会出现在 `ralph preset check --format json`**——lint 类无法从单 hat activation 视角判断路径可见性 / 消费动作 / 生命周期责任，靠 reviewer 在第 4 / 5 步 AAF + Payload Audit 独立审。

| finding_id（裸 ID / JSON 不出现） | default_severity | default_confidence | aaf_question | category | 备注 |
|---|---|---|---|---|---|
| `preset.artifact_path_not_in_visible_context` | P0 | 90 | Q2 | payload-content | review-only；artifact 路径对消费 hat 不可见（例 AE2 path-invisible 分支） |
| `preset.artifact_no_consumer_declared` | P1 | 80 | Q5 | payload-content | review-only；路径被声明但下游未声明读取 / 验收动作（例 AE2 consumer 分支） |
| `preset.artifact_no_lifecycle_owner` | P1 | 80 | Q5 | state | review-only；多阶段落盘但缺 owner / reader / retention / cleanup 责任（例 AE4） |
| `preset.artifact_uses_internal_ledger` | P0 | 95 | Q2 | visibility | review-only；把 `.ralph/events.jsonl` / `.ralph/loops.json` / `.ralph/supervisor.db` 当作业务 artifact；可与 `preset.instructions_read_internal_ledger` 联动 |
| `preset.payload_carries_full_content` | P0 | 90 | Q4 | payload-content | review-only；event payload 直接搬运完整结果 / 长内容 / 跨 hat 摘要（例 AE1） |
| `preset.artifact_first_field_docs_missing` | P1 | 80 | Q4 | policy-feedback | review-only；`field_docs.<path_field>.meaning / source / fill_rule / examples` 任意一项缺失或与本表「`field_docs` 审核点」不一致 |
| `preset.artifact_first_exemption_unjustified` | P1（阻塞下游则升 P0） | 75 | Q4 | payload-content | review-only；「不落盘」例外理由仅为字符数 / 不具体（例 AE5） |
| `preset.artifact_first_passed_on_path_presence` | P1 | 75 | Q4 | payload-content | review-only；仅因 payload 含路径字段就放行，未验路径可见性 / 内容语义 / 消费动作（例 AE2，R12） |
| `preset.subagent_result_returned_only_in_message` | P0 | 90 | Q4 | payload-content | review-only；sub-agent 完整结果通过 hat 长消息返回，未在 `.ralph/` 下落盘（例 AE1） |
| `preset.artifact_described_as_preset_owned` | P0 | 85 | Q3 | style | review-only；preset 文本把自身描述为 artifact 创建者，实际由 hat / sub-agent 在 activation 创建 |
| `preset.artifact_content_insufficient_for_decision` | P0（阻塞下游） / P1 | 80 | Q1 / Q5 | payload-content | review-only；路径可读但 artifact 内容不足以支撑 consumer Q1（R8） |

### Trigger Context 软性 AAF 缺口（review-only，不直接触发 lint）

| 缺口 | Severity | category | aaf_question | 备注 |
|---|---|---|---|---|
| instructions 复制了 hint 条件值（与 `## TRIGGER CONTEXT` 双写漂移） | P1 | style | Q3 | instructions 应只引用 `## TRIGGER CONTEXT` 区块 |
| `routing_hints[*].guidance` 是 runtime 控制命令 / 修改 routing / 改权限 | P1 | feasibility | Q4 | guidance 必须是 agent 行动 |
| `summary_fields` 引用字段在 trigger payload 中可见但不在 schema SSOT 内 | P0 | payload-content | Q4 | 需先补 `required_fields` ∪ `known_fields` ∪ `field_docs` |
| matched hint 的 guidance 与下游 hat 实际语义不一致 | P1（阻塞下游则 P0） | payload-content | Q5 | 验证 guidance 映射到下游决策分支 |

### Single-chain-first audit (2026-07-07-006 Unit 6)

新增 / 软性 finding（机械 lint 不直接产出，靠 AAF review 与 fixture 自检触发）：

| 缺口 | Severity | 建议 | category | aaf_question |
|---|---|---|---|---|
| `fallback_reaches_success_terminal` | P0 | delete or downgrade to diagnostic | topology | Q4 |
| `runtime_unit_loop_multiple_fact_sources` | P0 | migrate-into-executor | topology | Q5 |
| `blocked_failed_promoted_to_pass` | P0 | delete promotion path | topology | Q4 |
| `topic_multi_consumer` | P1（blast radius 大则 P0） | split consumer or remove | topology | Q5 |
| `hidden_phase_decision` | P1（改变业务事实则 P0） | lift to explicit hat transition | topology | Q4 |
| `production_correction_bypasses_independent_review` | P0 | route every production HEAD through stabilization + independent review | topology | Q5 |
| `normalized_plan_identity_not_propagated` | P0 | require and pass version/digest/normalized plan/trace on every handoff | handoff | Q5 |
| `post_fix_review_reopens_unbounded_fix` | P0 | explicit post-fix phase with accept-or-block branch | topology | Q4 |
| `prompt_wall_serial_style` | P1 | reference skill doc, do not inline | style | Q3 |

### Wave capability audit (2026-07-22-002 plan U4)

> **触发条件**：`execution_model ∈ {wave, supervisor+wave}`；或 YAML / hat `instructions` 含 `ralph wave emit` / `ralph wave verify`；或 hat 依赖 `## WAVE CONTEXT`。**capability-triggered**，**禁止**按 preset 名称点名门控。
> **未触发**：review 把本段记为 N/A（按 plan U5 SKILL §3d 规则），不假装已审。

| 缺口 | Severity | category | aaf_question | finding_id |
|---|---|---|---|---|
| 非 dispatcher hat `instructions` 要求 / 暗示调用 `ralph wave emit` 或 `ralph wave verify` | P0 | feasibility | Q3 | `preset.wave_worker_calls_wave_emit` |
| dispatcher 在 `ralph wave emit --payloads-stdin` 之前未先跑 `ralph wave verify --payloads-stdin`（拿 ticket）预检 | P0 | opac | Q3 / Q4 | `preset.wave_missing_verify_before_emit` |
| worker 完成态由 hat-channel 验证（应走 `ralph events --events-source main`） | P0 | visibility | Q2 | `preset.wave_confirm_uses_hat_channel` |
| 任何 hat `publishes` 含 `wave.*` / `exec.wave.*` 等协调 topic | P0 | topology | Q4 | `preset.wave_agent_emits_coordination_topic` |

review 命中时按上表 `finding_id` + `default_severity` + 默认 confidence 起点 60 入主表（与 `ralph-tools-wave` §「Policy-Check 反馈」联动）。若 `ralph wave verify` policy-check JSON 缺 `payload_index` 导致 wave batch 无法定位失败 item，按「Policy-Check Feedback → Severity」段 `payload_index` 缺失行入栏。

### Supervisor capability audit (2026-07-22-002 plan U4)

> **触发条件**：`event_loop.supervisor.enabled: true`；或 `execution_model ∈ {supervisor, supervisor+wave}`；或 hat `instructions` 含 supervisor 协调 topic / 引用 `supervisor.db`。**capability-triggered**，**禁止**按 preset 名称点名门控。
> **未触发**：review 把本段记为 N/A。

| 缺口 | Severity | category | aaf_question | finding_id |
|---|---|---|---|---|
| preset 启用了 `event_loop.supervisor.enabled` 但 `event_loop.execution_mode` 不是 `isolated` | P0 | feasibility | Q3 | `preset.supervisor_requires_isolated` |
| hat `publishes` 含 supervisor 协调 topic（`exec.wave.*` / `slot.*` 等） | P0 | topology | Q4 | `preset.supervisor_hat_publishes_coord_topic` |
| supervisor wave consumer hat（`triggers:` 含 `*.unit.ready`）未声明 `concurrency > 1`（默认 1） | P0 | feasibility | Q3 | `preset.supervisor_wave_consumer_low_concurrency` |
| supervisor sub-unit 状态未走 `ralph tools task list` 或业务 artifact，而是依赖读 `.ralph/supervisor.db` | P0 | visibility | Q2 / Q5 | `preset.supervisor_unit_state_not_via_task_api` |
| hat `instructions` 要求把 `.ralph/supervisor.db` 当业务 artifact 接口（写或读） | P0 | visibility | Q2 | `preset.artifact_uses_internal_ledger` |
| `execution_model` Intent 字段与 YAML 能力信号不一致（如 Intent= `single-chain` 但 `event_loop.supervisor.enabled: true`） | P0 | payload-content | Q4 | `preset.execution_model_intent_mismatch` |
| supervisor preset 缺少 `event_loop.supervisor.max_concurrent_workers` 或上限超过合理范围（生产应 ≤ 8） | P1 | feasibility | Q3 | review-only（`supervisor_missing_global_cap`） |
| integrator hat 在 `success_slots` 资源缺失 / 不可读时仍发 `work.done` | P0 | payload-content | Q4 / Q5 | review-only（`supervisor_integrator_skips_resource_verification`） |
| 主 ledger 出现多个 `LOOP_COMPLETE`、`loop_stale` 或重复协调终态 | P0 | topology | Q4 | review-only（`supervisor_duplicate_or_stale_terminal`） |
| supervisor preset 缺少 fan-in sink 失败的明确 fail-closed 路径（sink 失败仍冒充成功） | P0 | topology | Q4 | review-only（`supervisor_sink_failure_fake_success`） |
| supervisor preset 在终态（成功或失败）后未释放 permit / 未关闭 child / 未清理临时 worktree/branch | P0 | state | Q5 | review-only（`supervisor_terminal_cleanup_missing`） |
| supervisor preset 的 restart 路径会重复注入协调终态或重复消费 `success_slots` 资源 | P0 | topology | Q4 | review-only（`supervisor_restart_not_idempotent`） |
| supervisor happy path 接受 `Failed` / `Cancelled` slot 作为成功 | P0 | payload-content | Q4 | review-only（`supervisor_happy_path_accepts_failure`） |

命中按上表入主表；`preset.execution_model_intent_mismatch` 是 U4 新增 review-only 软性 finding，与既有 lint id `preset.supervisor_requires_isolated` / `preset.supervisor_hat_publishes_coord_topic` / `preset.artifact_uses_internal_ledger` 复用底层问题，但触发条件是 **capability + Intent 一致性**而非 preset 名。`presets/en/parallel-forge.yml` 等既有 supervisor-enabled builtin 仍受既有 lint 约束，不在本表新触发条件内。

### Agent skill audit（review-only，由 review SKILL Workflow 0a 弹窗默认跳过、选审触发）

按 `references/agent-skill-audit.md` 的规程，对注入给 agent 的 skill 文档（`crates/ralph-core/data/*.md` / 外仓二进制内嵌）做内容级审计。**默认不审**，review SKILL 第 0a 步必须弹出交互选择菜单，默认选项是「仅审查 preset YAML（推荐）」。命中按上表 `default_severity` + `default_confidence` 入主表（与 `ralph preset check --strict` 输出的 lint ID 分开——本表 ID 不带 `lint.` 前缀，也**不**出现在 `ralph preset check` JSON）。

| finding_id（裸 ID） | default_severity | default_confidence | aaf_question | category | 含义 |
|---|---|---|---|---|---|
| `agent_skill.leaks_internals` | P0 | 95 | Q3 | lint | skill 文档泄漏了 agent 不可见的内部实现细节（内部函数名 / 模块名 / 内部 ledger 路径 / review-only 注释 / 一次性事故报告路径 / 过窄 preset 案例） |
| `agent_skill.unreadable` | P1 | 85 | Q3 | style | skill 文档可读性差（术语未解释 / 触发条件缺失 / 失败停止条件缺失 / 未按「agent 下一步能执行什么」写） |
| `agent_skill.inject_claim_false` | P0 | 95 | Q3 / Q4 | lint | skill 文档 / hat `instructions:` 错误声称某 skill 已自动注入，或把 on-demand skill 写成 auto-inject；对账源：`ralph inspect prompt --hat <id> --format json` |
| `preset.precheck_rule_without_synthesized_gate_hat` | P1 (default mode) / P0 (strict mode) | 90 | Q3 / Q4 | lint | `event_loop.precheck.rules.<X>` 声明 `enabled: true` 且 producer 已发 `<X>.proposed`，但有效 config 缺 `precheck-<X>` 拦截 hat（半 desugar 状态，proposed 事件无消费者）；常见于手工拼装 config 或 desugar 回归 — 跑 `normalize()`（标准加载路径会自动做）或恢复合成 gate hat 可修。merge 层整块剥掉 `event_loop.precheck` 的场景由 `merge_hats_overlay_preserves_precheck_when_operator_omits_it` 集成测试覆盖，不走本 finding |

### CE pipeline review/fix artifacts（review-only 软性缺口）

Reviewer 在做 CE builtin preset review 时按本表入主表（不进 `ralph preset check` JSON，机制同 Artifact-First finding）。

| 缺口 | Severity | category | aaf_question | 备注 |
|---|---|---|---|---|
| mandatory review artifact 缺失 / 不可读 / count 不一致时 synthesizer 仍发 `review.synthesized` 并伪造 P3/ignore finding（"降级容忍"反模式） | P0 | feasibility | Q3 / Q4 | 必须改为 fail-close，发 `review.artifact.blocked` |
| mandatory review artifact 缺失 / 不可读但 preset 没有任何阻塞事件路径 | P0 | topology | Q4 | reporter 之外的下游禁止消费阻塞事件 |
| reporter 用 `ralph events --events-source main` 重建跨 hat 状态（业务字段而不是诊断字段） | P0 | visibility | Q2 | 必须只读 trigger payload 与 `report_input_file` bundle |
| reporter 的 trigger topic schema 没有 `report_input_file` required field 或 field_docs 三段不完整 | P0（缺字段）/ P1（field_docs 缺段） | payload-content | Q4 / Q5 | CE builtin 结构化契约测试在 `crates/ralph-cli/src/presets.rs` 兜底 |
| 「写操作者文件 + emit 完成 topic」的 hat，completion schema 缺路径字段 required，或 instructions 未要求 Confirm 打印 `DELIVERABLE_PATH` | P0 | visibility | Q4 / Q5 | 操作者必须能在 TUI 看到可读交付路径；触发条件以 instructions 为准，不以 hat 名为准 |
| preset 把 `report.done` + `completion_promise` 双事件配对声明，但触发 hat 身份不是 preset 的 sole 收尾 hat（多个 hat 都 publish 这对） | P0 | topology | Q4 | 只允许唯一收尾 hat 享受窄例外 |
| 同 activation 内 emit 第三个业务事件（即使前两个合法配对） | P0 | feasibility | Q4 | 窄例外只覆盖两个事件；第三仍按单事件预算被丢弃 |
| 本次新增或修改的 handoff / identity / artifact reference / decision 字段缺 `field_docs` 三段 | P0（缺 `source`）/ P1（缺 `meaning` 或 `fill_rule`） | policy-feedback | Q4 | 结构化契约测试只覆盖本次新增合同，不借机强制迁移无关历史字段 |
| writing hat 在 terminal handoff 前没有从当前 activation 重新取得真实 Git 状态，或要求读取不可见状态 | P1 | style | Q3 | 保留最小、可执行的 Git 状态检查；不要新增 preset 专用注入 skill |
| hat `instructions:` 复制大段命令参考表 / policy-check 步骤 / OPAC 四阶段规则（应引用对应注入 skill） | P1 | style | Q3 | 应引用已有 agent-facing skill 规范 |

数据源：`crates/ralph-core/src/preset_lint/finding_id.rs`。

**`ralph preset check --format json` 前缀：** lint 类 finding 的 `id` 为 `lint.preset.<snake_id>`（例如 `lint.preset.instructions_read_internal_ledger`）。本表「裸 ID」列匹配时 **strip `lint.` 前缀或两端都试**。

未知 `finding_id`：仍入报告，severity 取 lint 输出，confidence 用命令输出校准（Error 95 / Warn 85）。

artifact-first review-only finding **不**出现在 `ralph preset check` JSON；review 报告需在 Mechanical Lint Results 段注明「artifact-first 项：review-only，不进 lint JSON」（参见 `ralph-preset-review` SKILL §6）。当 review 命中 `preset.artifact_path_not_in_visible_context` / `preset.artifact_no_consumer_declared` / `preset.artifact_no_lifecycle_owner` / `preset.artifact_uses_internal_ledger` / `preset.payload_carries_full_content` / `preset.artifact_first_field_docs_missing` / `preset.artifact_first_exemption_unjustified` / `preset.artifact_first_passed_on_path_presence` / `preset.subagent_result_returned_only_in_message` / `preset.artifact_described_as_preset_owned` / `preset.artifact_content_insufficient_for_decision` 时，按上表 `default_severity` + `default_confidence` 入主表（confidence ≥ 60 门槛仍适用），并以「单 hat activation 视角 + Payload Audit」为证据来源。

### 备注：lint vs review-only

上表 Artifact-First Handoff finding_id（11 条）**全部为 review-only**，不在 `crates/ralph-core/src/preset_lint/finding_id.rs` 实现，也不出现在 `ralph preset check` JSON 输出中。它们是 `ralph-preset-review` 在第 4 / 5 步（AAF + Payload Audit）从单 hat activation 视角独立审出的软性缺口，原因是：

- 路径可见性、内容语义、消费动作、生命周期责任等属性依赖「该 hat 在自己那一轮 activation 里实际能 Observe / 调用什么」的视角判断，机械 lint 当前没有可执行的形状契约可断言。
- 业务 artifact 目录结构由 preset / hat 设计自定，lint 不强制统一约定（plan Product Contract §Scope Boundaries 明示），所以路径是否合理也是 review 的判断。

若后续 `crates/ralph-core/src/preset_lint/` 要把这些 finding 升级为 lint（实现 R8 / R9 / R10 / R11 / R12 的机械拦截），须先把 ID 加入 `crates/ralph-core/src/preset_lint/finding_id.rs` 并同步更新 `ALL_FINDING_IDS` 数组，同时把 `default_severity` / `default_confidence` 与本表保持一致；升级前 review 仍按本表入主表，并在 Remediation Plan 中标注 review-only 来源。本任务不涉及 Rust 代码修改。

### Flow declaration topology finding（`ralph preset check` JSON 直接产出）

`mechanism.flow.steps[]` 是公开 stable contract。这些 finding 在 `ralph preset check --strict` JSON 中按 `lint.preset.<id>` 输出。

| finding_id | default_severity | 含义 | 怎么改 |
|---|---|---|---|
| `preset.flow_declaration_missing` | P1 / 85 | preset 声明了多 hat handoff 但 `mechanism.flow` 整段缺失，`advance_plan_step` 退化到 positional fallback，多 topic 路径不可审计 | 增加 `mechanism.flow.steps`，把每个跨 hat handoff 拆成单 step |
| `preset.flow_unknown_emit_rejected` | P0 / 95 | 当前 step `allowed_emits` 不含该 topic，FlowStepScope 拒收 | 把该 topic 加到对应 step 的 `allowed_emits`；或拆出独立 step 显式声明 `on` |
| `preset.flow_linear_positional_ambiguity` | P0 / 90 | 非末尾 `kind: linear` step 声明了 ≥2 个 allowed emits，且后续 step 全无 `on` / `on_any_of` 引用其中任一 topic——运行时仍会按位置回退推进，隐藏真实拓扑 | 把多个顺序 handoff 拆成各自 step，使用下一 step 的 `on` 声明进入条件；多源 block 走 `on_any_of` |

`flow_linear_positional_ambiguity` 的精确触发条件（plan Unit 4 §13）：

1. 当前 step 不是 steps 数组末尾；
2. `kind == "linear"`；
3. `allowed_emits.len() >= 2`；
4. 所有后续 step 的 `on` ∪ `on_any_of` 与当前 allowed topics 交集为空。

不在上述形状内的 multi-topic linear step（如已有显式 `on` / `on_any_of` 引用其中一个 topic）不会被该 finding 命中，**也不**读取 hat prompt 或修改 severity 随 strictness 的通用机制。

### Runtime-contract topology finding（`ralph preset check` JSON 直接产出）

这些 id **不带** `lint.` 前缀，来自 `runtime_contract` / `preset_validator`：

| finding_id | default_severity | 含义 | 怎么改 |
|---|---|---|---|
| `topology.required_event_not_on_all_paths` | Error | `required_events` 某 topic 不在所有通往 completion 的路径上 | 换成真收敛 topic，或把成功脊门禁改到 `path_required_events` |
| `topology.path_required_event_not_on_all_paths` | Error | `path_required_events.require` 可被绕过到达 `anchor` | 去掉绕过边，或调整 `anchor` / `require` |
| `topology.unreachable_path_required` | Error | `path_required_events` 的 `anchor`/`require` 从起点不可达 | 补齐 publishes/triggers |

## required-event-to-completion 窄例外（review 复核条件）

上方「同一 hat emit 多条业务事件」「终态 emit 前夹带其它业务事件」两条 P0 不再 blanket 适用。当且仅当以下**全部条件**成立时，review 可在不重新触发 P0 的前提下放过该 hat 的双事件 emit：

1. **preset 显式配置**:当前 preset 的 `event_loop.required_events[]` 非空，且 `event_loop.completion_promise`（默认 `LOOP_COMPLETE`）非空。
2. **收尾 hat 身份**:当前 hat 是 preset 中**唯一负责收尾的 hat**（如 reporter / alignment）；其 `publishes` 同时包含一个 required_events 列表里的 topic 与 `completion_promise`，且 `terminal_events` 包含二者。其它 hat（executor、synthesizer、fixer 等）一律**不享受**本例外。
3. **顺序正确**:先发 required_events[] 中的 topic，再发 `event_loop.completion_promise`。任何顺序错误都视为普通多事件违规。
4. **同 hat provenance**:两个事件的 hat provenance 与当前 isolated hat 一致；跨 hat 不享受例外。
5. **正好两个事件**:同一 activation 内只能有两个业务事件；任何第三业务事件仍按单事件预算违规（P0）。
6. **policy-check 双阶段**:两个事件中每一个仍按 `ralph emit --policy-check` 流程通过后才正式 emit；本例外是 runtime budget 层面的判定，不是 policy-check 的旁路。

review 在放过此类双事件时，应在 Remediation Plan / 报告「AAF Decision Rationale」段记录：

- 引用 preset YAML 中 `event_loop.required_events[]` 与 `event_loop.completion_promise` 的具体配置值（行号或字段路径）。
- 引用相关 runtime 测试名（如 `isolated_dual_publish_handoff_required_event_to_completion` / `isolated_required_event_then_completion_same_turn_report_done`）作为行为证据。
- 标注当前 hat 是该 preset 的收尾 hat，其它 hat 不享受本例外。

不满足上述任一条件时，仍按 P0 入主表并要求 author 修复。

## Capability coverage finding_id (Unit 4 / plan 2026-07-27-002)

<!-- anchor: wave-emit -->
<!-- anchor: supervisor-emit -->
<!-- anchor: task-id-live -->
<!-- anchor: artifact-first -->
<!-- anchor: payload-consistency -->
<!-- anchor: trigger-context -->

## Capability-triggered parallel-forge review-only findings (plan 2026-08-02-001 U3)

下列 finding 全部为 review-only，不进 `ralph preset check` JSON。触发条件
按 capability（`execution_model: supervisor` / wave / terminal ownership
等）而非 preset 名称；`parallel-forge` 仅作验证样本。fixture 顶部注释与
`skills/ralph-preset-review/fixtures/README.md` §8 标注 anti-pattern 轴、
expected finding id 与本段对照命中。

| finding_id | default_severity | default_confidence | aaf_question | category |
|---|---|---|---|---|
| `preset.worktree_reuse_fabricates_settlement` | P0 | 90 | Q4 / Q5 | payload-content |
| `preset.readonly_hat_writes_artifacts` | P0 | 95 | Q3 | visibility |
| `preset.correction_round_below_final_min` | P0 | 90 | Q4 | topology |
| `preset.auditor_multi_terminal_publisher` | P0 | 95 | Q4 | topology |

### Worktree reuse evidence 段

| 缺口 | Severity | category | aaf_question | finding_id |
|---|---|---|---|---|
| hat `instructions` 把 `.ralph/reuse-history/<plan>/` 当作当前 wave 的新鲜 `forge.wave.settled` / `settled_task_ids` 证据（未重跑 wave 即宣称 settlement） | P0 | payload-content | Q4 / Q5 | `preset.worktree_reuse_fabricates_settlement` |
| hat 把 `--reuse-worktree` / reuse-history 检索结果冒充新 plan identity / 复用 prefix 误当成 retry 路径 | P0 | payload-content | Q4 | `preset.worktree_reuse_fabricates_settlement` |
| reuse 路径未声明可证据化的 artifact / commit SHA / plan identity 三件套 | P1 | payload-content | Q4 | review-only（`worktree_reuse_evidence_incomplete`） |

review 命中后必须要求：要么在当前 activation 重跑 wave 并以新 emit 形成 settlement，要么把 reuse 写为「本次 loop 不进入 settlement，request `forge.reuse.assessed`」并阻塞到 deferred 实现落地；不得伪造 `settled_task_ids`。

### Readonly hat gate 段

| 缺口 | Severity | category | aaf_question | finding_id |
|---|---|---|---|---|
| hat 声明 `readonly: true`，但 `instructions` 仍要求 `cat >` / `tee` / `sed -i` / `ralph emit` 非 `review.*` 终态 / `git add` / `git commit` | P0 | visibility | Q3 | `preset.readonly_hat_writes_artifacts` |
| readonly hat 缺 `allowed_write_paths` / `mutation_capture` 声明，且 instructions 含副作用动词 | P0 | visibility | Q3 | `preset.readonly_hat_writes_artifacts` |
| readonly hat 命中 verdict 后静默 `git checkout . && git clean -fd` | P0 | visibility | Q3 | review-only（`readonly_hat_silently_resets_workspace`） |
| readonly hat 试图用 artifact 写入代替 verdict 决策（mutation capture 越界） | P1 | state | Q5 | review-only（`readonly_artifact_overreach`） |

review 命中后必须 fail-close：要么改 hat 为非 readonly 并把 verdict 写盘动作显式列在 `allowed_write_paths`，要么把写入动作移到独立非 readonly hat；不得在 review 流程中自动 reset 用户工作区。

### Final correction 段

| 缺口 | Severity | category | aaf_question | finding_id |
|---|---|---|---|---|
| dispatcher 在 `correction_round` < 3 时发 `forge.final.correction.settled`（schema `allowed_values: correction_round=[3]` runtime 强制） | P0 | topology | Q4 | `preset.correction_round_below_final_min` |
| `forge.final.correction.settled` 的 `correction_round` 不在 schema 允许集（`{3}`） | P0 | topology | Q4 | review-only（`final_correction_round_out_of_range`） |
| round 0–2 直接发终态，跳过 `forge.correction.requested` / `forge.correction.done` 中间路径 | P0 | topology | Q4 | `preset.correction_round_below_final_min` |
| 同一 `failure_fingerprint` 在多轮 `forge.correction.done` 重复出现但 dispatcher 仍判 final | P0 | payload-content | Q4 | review-only（`final_correction_duplicate_fingerprint`） |

review 命中后必须要求 dispatcher 把 round 强制 ≥ 3，否则继续走 `forge.correction.requested` → `forge.correction.done` 循环；不得绕过 `correction_round` schema 约束。

### Auditor / reporter terminal ownership 段

| 缺口 | Severity | category | aaf_question | finding_id |
|---|---|---|---|---|
| auditor hat `publishes` 同时含 `forge.audit.done` 与 `forge.report.done`（auditor 单业务终态被破坏） | P0 | topology | Q4 | `preset.auditor_multi_terminal_publisher` |
| reporter 的 `forge.report.done` 后续 `LOOP_COMPLETE` 窄例外用在非 sole 收尾 hat 上 | P0 | topology | Q4 | review-only（`reporter_dual_emit_narrow_exception_overuse`） |
| `forge.report.done` 的 `report_path` 与 `event_loop.completion_promise` 配对声明缺 `report_input_file` 字段或路径与 trigger payload 不一致 | P0 | payload-content | Q4 / Q5 | review-only（`reporter_completion_payload_mismatch`） |
| preset 声明 `forge.audit.done` + `forge.report.done` 同时由同一 hat publish，缺一独立的 reporter | P0 | topology | Q4 | `preset.auditor_multi_terminal_publisher` |

review 命中后必须要求：auditor 仅 publish `forge.audit.done`（或 `forge.plan.blocked`）；reporter 单独 publish `forge.report.done` 并在符合 `required-event-to-completion 窄例外` 时配 `event_loop.completion_promise`；completion payload 与 trigger `report_path` 一致。

## review-only finding 默认值（capability-triggered parallel-forge 子集）

| finding_id | default_severity | default_confidence |
|---|---|---|
| `preset.worktree_reuse_fabricates_settlement` | P0 | 90 |
| `preset.readonly_hat_writes_artifacts` | P0 | 95 |
| `preset.correction_round_below_final_min` | P0 | 90 |
| `preset.auditor_multi_terminal_publisher` | P0 | 95 |

fixture README §8、fixture 顶部注释与本表 ID 一一对应；review 命中按本表 `default_severity` + `default_confidence` 入主表（confidence ≥ 60 门槛仍适用），并在 Mechanical Lint Results 段注明「capability-triggered 项：review-only，不进 lint JSON」。

| finding_id | description | severity | confidence起点 |
|---|---|---|---|
| `preset.capability_discovery_missing` | preset exercises a capability not covered in `preset-author-notes.md` | P1 | 60 |
| `preset.review_evidence_coverage_gap` | review lacks evidence path to capability audit | P1 | 60 |

### Evidence-bound correction finding_id（review-only，plan 2026-08-06-001 U4）

按 R10 surface 补充的 review-only finding。**这些 ID 不会出现在 `ralph preset check --format json`**——evidence-bound correction 依赖语义级校验（`violated_invariant` / `target_hat` / 有界 retry vs 无界循环），机械 lint 无法从 YAML 形状判断，必须靠 reviewer 在 Payload Audit + Q4 校验独立审。

| finding_id（裸 ID / JSON 不出现） | default_severity | default_confidence | aaf_question | category | 含义 |
|---|---|---|---|---|---|
| `evidence_bound_missing_invariant` | P1 | 85 | Q4 | payload-content | review-only；semantic rejection 的 `correction` payload 缺少 `violated_invariant` 字段，agent 无法定位哪个不变量被违反 |
| `evidence_bound_replacement_payload` | P0 | 90 | Q4 | payload-content | review-only；semantic rejection 的 `correction` payload 含 `replacement` / `suggested_payload` 等替代语义字段，违反「语义拒绝不得含修复建议」契约 |
| `evidence_bound_no_target` | P1 | 85 | Q4 | topology | review-only；semantic rejection 缺少 `target_hat` 字段，bounded retry 机制无法路由到正确的重试目标 |
| `evidence_bound_unbounded_retry` | P1 | 85 | Q4 | feasibility | review-only；preset 的 correction / retry 循环没有 evidence progression check（每次重试都应提供新的 `violated_invariant` / `observed`），构成无界重试循环 |

### Scope contract finding_id（review-only，plan 2026-08-08-004 U7）

按 R11 / R12 / R14 补充的 review-only finding。**这些 ID 不会出现在 `ralph preset check --format json`**——scope contract 依赖字节稳定 manifest 内容与 payload consistency 校验，必须靠 reviewer 在 Capability discovery + Payload Audit 独立审。

| finding_id（裸 ID / JSON 不出现） | default_severity | default_confidence | aaf_question | category | 含义 |
|---|---|---|---|---|---|
| `scope.contract.missing_manifest_field` | P0 | 90 | Q4 | payload-content | review-only；scope topic (`merge.integrated` / `merge.stabilized` / `postmerge.changemap.ready` / `redteam.plan.resolved`) 的 payload 缺少 `scope_manifest_path` / `scope_digest` / `scope_status` / `scope_base_sha` / `overall_confidence` / `critical_unknown_count` 任一必填字段 |
| `scope.contract.placeholder_base` | P0 | 90 | Q4 | payload-content | review-only；`scope_base_sha` 是占位符（如 `<global-baseline>`）而非真实 Git SHA（40 hex chars）；merge-boundary 相关 topic 必须有真实 SHA |
| `scope.contract.boundary_authority` | P1 | 80 | Q3 | feasibility | review-only；preset 声明 merge-boundary 作为 scope 依据，但 boundary 来源 hat 不具备独立 authority（下游不应把 merge 结果当作 scope 解析的 authority） |
| `scope.guard.unsafe_bypass` | P0 | 95 | Q3 | feasibility | review-only；hat `instructions` 提到 `--unsafe-no-policy-check` 可以跳过 scope handoff guard；该 guard 对 scope topics 是强制不可绕过的 |
| `scope.contract.confidence_gate_bypass` | P0 | 90 | Q4 | payload-content | review-only；`overall_confidence` 低于 90 或 `critical_unknown_count` 非零时仍标记 `proceed = true` 并推进 scope；threshold gate 必须同时满足三个条件 |

