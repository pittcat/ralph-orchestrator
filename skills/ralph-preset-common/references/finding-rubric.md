# Finding Rubric

Review skill 将 mechanical lint 与软性 AAF 缺口映射为 P0/P1/P2 + confidence。

**入表门槛：** `confidence ≥ 60`（见 `ralph-preset-review` SKILL）。

**机械 lint 默认 confidence：** Error → 95；Warn → 85。

**软性 AAF 起点：** ≤ 50，须验证后上调。

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

## finding_id 映射表（curated）

| finding_id（裸 ID / JSON 为 `lint.` + 此列） | default_severity | default_confidence | aaf_question | category |
|---|---|---|---|---|
| `preset.multi_hat_requires_isolated` | P0 | 95 | Q3 | lint |
| `preset.instructions_read_internal_ledger` | P0 | 95 | Q3 | lint |
| `preset.instructions_opac_skill_reference_missing` | P1 | 85 | Q3 | lint |
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
| `preset.review_complete_misrouted` | P0 | 95 | Q5 | topology |
| `preset.strict_reason_routing_missing` | P1 | 85 | Q4 | topology |
| `preset.metadata_runtime_drift` | P1 | 85 | — | lint |

数据源：`crates/ralph-core/src/preset_lint/finding_id.rs`。

**`ralph preset check --format json` 前缀：** lint 类 finding 的 `id` 为 `lint.preset.<snake_id>`（例如 `lint.preset.instructions_read_internal_ledger`）。本表「裸 ID」列匹配时 **strip `lint.` 前缀或两端都试**。

未知 `finding_id`：仍入报告，severity 取 lint 输出，confidence 用命令输出校准（Error 95 / Warn 85）。
