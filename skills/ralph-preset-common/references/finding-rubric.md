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
| 同一 hat emit 多条业务事件（违反单事件预算） | P0 | feasibility | Q4 |
| 终态 emit 前夹带其它业务事件 | P0 | feasibility | Q4 |
| report finding 无 repair surface（无字段 / 无 source / 无 fix） | 拒入主表 → Unverified Suspicions | — | — |

## finding_id 映射表（curated）

| finding_id（裸 ID / JSON 为 `lint.` + 此列） | default_severity | default_confidence | aaf_question | category |
|---|---|---|---|---|
| `preset.multi_hat_requires_isolated` | P0 | 95 | Q3 | lint |
| `preset.instructions_read_internal_ledger` | P0 | 95 | Q3 | lint |
| `preset.instructions_opac_skill_reference_missing` | P1 | 85 | Q3 | lint |
| `preset.instructions_emit_feedback_skill_reference_missing` | P1 | 80 | Q3 | lint |
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
| `preset.supervisor_requires_isolated` | P0 | 95 | Q3 | lint |
| `preset.supervisor_hat_publishes_coord_topic` | P0 | 95 | Q4 | lint |
| `preset.metadata_runtime_drift` | P1 | 85 | — | lint |
| `preset.dimension_reviewer_write_plan` | P0 | 95 | Q3 | lint |

### Single-chain-first audit (2026-07-07-006 Unit 6)

新增 / 软性 finding（机械 lint 不直接产出，靠 AAF review 与 fixture 自检触发）：

| 缺口 | Severity | 建议 | category | aaf_question |
|---|---|---|---|---|
| `fallback_reaches_success_terminal` | P0 | delete or downgrade to diagnostic | topology | Q4 |
| `runtime_unit_loop_multiple_fact_sources` | P0 | migrate-into-executor | topology | Q5 |
| `blocked_failed_promoted_to_pass` | P0 | delete promotion path | topology | Q4 |
| `topic_multi_consumer` | P1（blast radius 大则 P0） | split consumer or remove | topology | Q5 |
| `hidden_phase_decision` | P1（改变业务事实则 P0） | lift to explicit hat transition | topology | Q4 |
| `prompt_wall_serial_style` | P1 | reference skill doc, do not inline | style | Q3 |

数据源：`crates/ralph-core/src/preset_lint/finding_id.rs`。

**`ralph preset check --format json` 前缀：** lint 类 finding 的 `id` 为 `lint.preset.<snake_id>`（例如 `lint.preset.instructions_read_internal_ledger`）。本表「裸 ID」列匹配时 **strip `lint.` 前缀或两端都试**。

未知 `finding_id`：仍入报告，severity 取 lint 输出，confidence 用命令输出校准（Error 95 / Warn 85）。
