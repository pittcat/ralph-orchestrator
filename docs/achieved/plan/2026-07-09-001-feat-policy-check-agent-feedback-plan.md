---
title: "feat: 增强 policy-check 的 agent 可修复反馈"
type: feat
status: active
date: 2026-07-09
origin: docs/achieved/brainstorms/2026-07-09-policy-check-agent-feedback-requirements.md
execution_model: strictly-sequential-isolated-tdd
---

# feat: 增强 policy-check 的 agent 可修复反馈

## Overview

本计划把 `event_policy.schemas` 从“只告诉机器缺不缺字段”增强为“也能告诉 agent 字段怎么填”，并把同一份 schema 语义用于三处：

1. `ralph emit --policy-check` / `ralph wave emit --policy-check` 的可修复错误反馈。
2. hat prompt 的 schema-aware publish section。
3. agent 注入 skill 文档与 preset instructions 的引用流程。

计划严格按 **Unit 1 -> Unit 2 -> ... -> Unit 9** 串行推进。每个 Unit 必须先写只覆盖本 Unit 输入输出的测试，当前 Unit 完全绿并完成重构后才能进入下一个 Unit。

## Problem Frame

当前 Ralph 已有 payload contract、schema-aware emit 示例、`--policy-check` 和 strict lint，但 agent 遇到拒收时经常只知道“payload 错了”，不知道“这个字段是什么意思、应该从哪里取、如何修正后重试”。这会导致多 hat loop 中的 JSON handoff 继续靠 agent 猜字段，尤其影响终态事件、handoff 事件和 review/fix loop 的收敛事件。

本计划只增强可解释性和 agent 使用路径，不改变事件是否被接受的判定标准，不引入 payload routing，也不让 runtime 自动编造业务字段。

## Execution Contract For This Plan

| 约束 | 执行要求 |
|------|----------|
| 严格串行 | 只能按 U1 -> U2 -> ... -> U9 顺序推进；禁止交替开发或并行开发 Unit。 |
| 绝对前置闭环 | Unit N 的编码、测试、重构和本 Unit 验收全部完成后，才能打开 Unit N+1。 |
| 绝对隔离 | 当前 Unit 只改本 Unit 列出的文件范围；不得提前写后置 Unit 的接线逻辑。 |
| 禁止前向依赖 | Unit N 的测试和运行不能依赖 Unit N+1 尚未实现的 API、文档或 preset 改动。 |
| 原子 TDD | 每个 feature-bearing Unit 先写 RED 测试；测试只验证当前 Unit 的输入输出。 |
| 无遗留债务 | 当前 Unit 的边界问题在当前 Unit 内解决；不得写“后续 Unit 再补”。 |

## Requirements Trace

- R1-R4: U1、U2 定义 schema agent-facing metadata 并保证不改变机器校验权威。
- R5-R10: U3、U4、U5 生成并接入字段级 policy-check 失败反馈。
- R11-R14a: U6 增强 prompt 中的 schema-aware publish section。
- R14b-R14g、R21: U7 更新 agent 注入 skill 和 instructions 引用 lint。
- R15-R17、SC6: U8 试点 `ce-executor-pipeline-loop` 高风险 topic。
- R18-R20: 每个 Unit 的测试必须包含旧 schema / 旧行为不变断言，U9 做最终回归。

## Scope Boundaries

- 不做 payload routing，不根据 payload 自动选择下一跳 prompt。
- 不自动修 payload；`suggested_payload_shape` 只能给结构和占位符，不能填业务事实。
- 不把 Ralph schema 升级为完整 JSON Schema DSL。
- 不改变 event loop 状态机、topic routing、business event budget、terminal semantics、step handoff 语义。
- 不要求所有 builtin preset 第一轮补齐字段说明；只做机制和 `ce-executor-pipeline-loop` 试点。

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/config/loop_config.rs`: `EventSchema` 当前包含 `payload`、`required_fields`、`allowed_values`、`hat_allowed_values`、`element_constraints`，所有新增 metadata 必须 `#[serde(default)]`。
- `crates/ralph-core/src/emit_schema_hint.rs`: 已是 prompt 示例和 CLI fix hint 的共享入口，应继续作为 schema-aware 文本/shape 渲染的单一入口。
- `crates/ralph-cli/src/policy_check.rs`: `ValidationError` / `ValidationFailure` 负责 wave/batch policy-check 失败输出；`PolicyCheckReport` 和 `report_to_emit_result` 负责 unified policy-check 到 EmitResult 的路径。
- `crates/ralph-cli/src/commands/emit.rs`: 单事件 `--policy-check` 与 `--output json` 接线已存在，新增反馈必须接在已有路径上。
- `crates/ralph-cli/src/wave.rs`: wave precheck 使用 `validate_batch_against_config` 和 `emit_policy_validation_failure`，必须保留 batch atomic reject 和 `payload_index`。
- `crates/ralph-core/src/instructions.rs`: `InstructionBuilder::build_custom_hat` 已通过 `build_publish_emit_section` 注入 schema-aware publish section。
- `crates/ralph-core/src/preset_lint/instructions_opac.rs`: 已有检查 hat instructions 是否引用 `ralph-tools-opac` / `ralph-tools-emit §5` 的模式，可扩展新 lint。
- `crates/ralph-core/data/ralph-tools-emit.md`: agent 深参考入口，必须同步新版错误反馈读取流程。
- `docs/solutions/developer-experience/agent-execution-contract-gates-2026-06-03.md`: 多层 contract/schema/instructions 字段集合必须一致，避免假成功和 drift。
- `docs/achieved/plan/2026-06-15-001-feat-schema-aware-hat-emit-instructions-plan.md`: 既有 B+C 策略已经证明 prompt 教对 + CLI pre-publish check 需要共享同一 schema hint。
- `docs/achieved/plan/2026-07-06-001-feat-ce-executor-serial-protocol-ssot-convergence-plan.md`: 可复用严格串行、原子 TDD 的计划格式。

### External References

- 不使用外部研究。该工作完全围绕本仓库现有 Rust config、CLI、preset lint、agent skill 文档模式展开，外部最佳实践不会比本地约束更有指导价值。

## Key Technical Decisions

- **KTD-1: Schema metadata 形状定为 `field_docs` + `examples`。**  
  `field_docs.<field>` 保存 `meaning`、`source`、`fill_rule` 三个可选字符串；`examples` 保存 topic 级示例 payload。字段是否必填和 allowed values 仍由现有机器约束决定。

- **KTD-2: 先做纯函数，再接 CLI。**  
  U1-U3 只做 config/model/render/enrichment 纯函数，不触碰 CLI 接线；U4/U5 再分别接单 emit 和 wave emit。这样每个 Unit 都能独立 TDD，避免半接线状态。

- **KTD-3: 单 emit 与 wave emit 共享错误 enrichment。**  
  不允许为 wave 单独写一套错误解释。共享 helper 根据 topic、payload、policy schema、payload index 生成 agent-facing error。

- **KTD-4: `suggested_payload_shape` 不等于自动修复。**  
  它只能保留安全的已有字段和缺失字段占位符，例如 `"<fill from synthesized review>"`，不得填 `0`、`pass` 等业务结论。

- **KTD-5: Agent adoption 走 skill + prompt builder 双入口。**  
  skill 教通用流程，prompt builder 给当轮具体 topic 的字段合同；preset instructions 只引用 skill，不复制字段说明。

## Open Questions

### Resolved During Planning

- 字段说明 YAML 名称：采用 `field_docs`，避免与 `allowed_values` / `element_constraints` 这种机器校验字段混淆。
- JSON 输出形状：保留现有外层 `ValidationFailure` / `EmitResult`，在内部 error item 上增量添加字段，降低消费者破坏面。
- 第一版 lint 严格度：只对 builtin/high-risk preset 中“publish 业务事件但未引用新版 emit skill”的场景 enforce；字段说明覆盖率先不全局 enforce。

### Deferred to Implementation

- `field_docs` 是否需要额外 `example_value` 字段：先用 topic-level `examples` 和 generated placeholder shape；实现中若发现表达不足，再在 U1 内决定是否增加，但不能影响 U2 以后。
- `PolicyCheckReport` 的 unified pipeline reason code 是否能完整反查 schema：U3 以 `EventPolicyConfig` + topic 作为输入补足 schema 信息；若某些 semantic gate 没有 schema field，保留 `code/message`，不硬造 field doc。

## High-Level Technical Design

> This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.

```text
EventSchema
  required_fields / allowed_values / element_constraints    # machine authority
  field_docs / examples                                     # agent-facing metadata
        |
        v
emit_schema_hint helpers
  - render publish section
  - render payload shape
  - enrich one validation error
        |
        +--> InstructionBuilder prompt section
        |
        +--> ralph emit --policy-check error item
        |
        +--> ralph wave emit --policy-check batch error item
        |
        +--> ralph-tools-emit docs describe how agent consumes it
```

## Implementation Units

- [ ] **Unit 1: Schema Metadata Model**

**Goal:** 给 `EventSchema` 增加 agent-facing metadata，但旧 schema 解析、默认值和机器校验行为完全不变。

**Requirements:** R1-R4, R18

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/config/loop_config.rs`
- Modify: `crates/ralph-core/src/config/ralph_config.rs`

**Approach:**
- 新增 `EventFieldDoc` 类型，字段为 `meaning`、`source`、`fill_rule`，全部 `#[serde(default)]`。
- `EventSchema` 新增 `field_docs: HashMap<String, EventFieldDoc>` 和 `examples: Vec<serde_json::Value>`，全部 `#[serde(default)]`。
- 不把 `field_docs` 纳入 runtime validation。Ralph 接受/拒绝 payload 的逻辑仍只看机器约束。
- 在 config validation 中只做轻量结构校验：`field_docs` key 不允许空字符串；不要求每个 required field 都必须有 doc。

**Execution note:** 先写 schema 解析和 backward-compat 测试，再加字段。

**Patterns to follow:**
- `ElementConstraint` 的 `#[serde(default)]` 和 backwards-compatible struct extension 模式。
- `ralph_config.rs` 中 `allowed_values` field path validation 的错误风格。

**Test scenarios:**
- Happy path: YAML 中声明 `field_docs.task_id.meaning/source/fill_rule` 和 `examples`，反序列化后字段完整保留。
- Backward compatibility: 旧 YAML 只含 `required_fields`，反序列化后 `field_docs` 和 `examples` 为空，且不报错。
- Error path: `field_docs` 含空 key，config validation 返回 `EventPolicyValidation`，错误路径指向 `event_loop.event_policy.schemas.<topic>.field_docs`。
- Non-goal guard: `field_docs` 中出现非 required field 不报错；这是说明能力，不是机器校验权威。

**Verification:**
- 本 Unit 子集测试证明新字段可读、旧 schema 不变、空 key 被拒。
- 本 Unit 不触碰 CLI、prompt builder、preset 文件。

- [ ] **Unit 2: Schema-Aware Render Helpers**

**Goal:** 扩展 `emit_schema_hint`，生成字段说明、allowed values、示例 payload 和 safe `suggested_payload_shape` 的共享渲染结果。

**Requirements:** R3, R5-R8, R11-R14, R18

**Dependencies:** Unit 1

**Files:**
- Modify: `crates/ralph-core/src/emit_schema_hint.rs`

**Approach:**
- 增加纯函数用于：
  - 按 `required_fields` 顺序渲染 field table。
  - 为单个 field 查找 `field_docs` 和 allowed values。
  - 生成 `suggested_payload_shape`：已存在字段保留原值，缺失字段使用 `<field>` 或 `<fill from ...>` 占位符。
  - topic-level `examples` 存在时优先用于 prompt 示例；policy-check suggestion 仍使用占位符 shape。
- 保留 hat-scoping：`fix_hint_for_hat_topic` 只有当前 hat publishes 匹配 topic 时才返回提示。
- 不在本 Unit 接入 `InstructionBuilder` 或 CLI。

**Execution note:** 先写纯函数测试，测试输入只用内建 `EventSchema` fixture。

**Patterns to follow:**
- 现有 `format_emit_json_example`、`build_publish_emit_section`、`fix_hint_for_hat_topic` 的共享模块定位。
- 现有 wildcard publish 权限测试。

**Test scenarios:**
- Happy path: schema 有 `field_docs.task_id`，rendered field line 包含 `task_id`、meaning、source、fill_rule。
- Happy path: schema 有 `allowed_values.verdict = ["pass", "blocked"]`，field hint 包含 allowed values。
- Happy path: payload `{ "verdict": "blocked" }` 缺 `reason`，`suggested_payload_shape` 保留 `verdict`，给 `reason` 占位符。
- Safety: 缺失 `must_fix_now_count` 时 shape 使用占位符，不填 `0`。
- Backward compatibility: 没有 `field_docs/examples` 的 schema 仍生成旧式 required-field placeholder。
- Scope guard: hat 未声明 publish topic 时 `fix_hint_for_hat_topic` 仍返回 `None`，不泄漏其它 hat payload 形状。

**Verification:**
- 本 Unit 子集测试只验证 `emit_schema_hint` 输入输出。
- 本 Unit 不修改 `instructions.rs`、`policy_check.rs`、`emit.rs` 或 `wave.rs`。

- [ ] **Unit 3: Agent-Facing Policy Error Enrichment**

**Goal:** 在不接 CLI 的前提下，把现有 `ValidationError` / schema / payload 转换为 agent 可读、可解析的 enriched error item。

**Requirements:** R5-R10, R18, R20

**Dependencies:** Unit 2

**Files:**
- Modify: `crates/ralph-cli/src/policy_check.rs`

**Approach:**
- 扩展或新增 error item 结构，保留现有 `payload_index`、`field`、`reason_code`、`message`。
- 新增可选字段：`code` 或复用 `reason_code`、`expected`、`actual`、`field_description`、`suggested_payload_shape`、`suggested_command`。
- 增加纯函数，例如“根据 topic、payload、policy、ValidationError 生成 enriched error”。函数必须能独立测试，不依赖 CLI stdout/stderr。
- 对不同错误类型的 enrichment 规则：
  - `missing_required_field`: `expected` 指向 required field，`field_description` 来自 schema，shape 给缺失字段占位符。
  - `invalid_field_value`: `expected` 来自 `allowed_values` / `hat_allowed_values` 能反查到的集合，`actual` 来自 payload 或 existing error message。
  - `payload_type_mismatch`: `expected` 来自 schema `payload`，`actual` 来自 violation/message；没有 field 时不要伪造 field doc。
  - array/element/semantic gate: 能定位 field path 则补 field doc；不能定位则保留 code/message。
- 所有 path 使用 topic/field/payload index，不输出绝对 workspace 路径。

**Execution note:** 先写 enrichment 纯函数测试，不改 `emit_policy_validation_failure` 调用方。

**Patterns to follow:**
- `ValidationFailure::from_batch` 的 top-level shape。
- `PolicyCheckReport::to_json_value` 已经避免泄漏绝对 workspace path。

**Test scenarios:**
- Happy path: missing `task_id` + `field_docs.task_id` -> enriched error 包含 field description 和 suggested shape。
- Error path: invalid `verdict` -> enriched error 包含 allowed values 和 actual `"bogus"`。
- Error path: payload type mismatch -> enriched error 包含 `expected=json_object`，field 为空且无 field description。
- Batch path: payload index 3 的错误 enrichment 保留 `payload_index=3`。
- Privacy: enriched JSON 不包含 temp workspace 绝对路径。
- Backward compatibility: 原有 `reason_code/message/field` 字段仍可序列化。

**Verification:**
- 本 Unit 子集测试只调用 enrichment helper 和 serde JSON。
- 本 Unit 不改变 `ralph emit` 或 `ralph wave emit` 实际输出。

- [ ] **Unit 4: Single Emit Policy-Check Output Wiring**

**Goal:** 把 Unit 3 的 enriched error 接入 `ralph emit --policy-check` 单事件路径，包括 text 和 JSON/EmitResult 拒收输出。

**Requirements:** R5-R10, SC1-SC3, R18-R20

**Dependencies:** Unit 3

**Files:**
- Modify: `crates/ralph-cli/src/commands/emit.rs`
- Modify: `crates/ralph-cli/src/policy_check.rs`

**Approach:**
- 在单 emit policy-check 拒收路径中，用 loaded `EventPolicyConfig` + topic + payload 调用 Unit 3 enrichment helper。
- JSON 输出路径保留现有 `EmitResult` 外层语义：`ok=false`、`recorded=false`、`topic` 不变；只增强 `errors[]` item。
- text 输出路径追加 agent-readable repair block：缺失字段、字段说明、expected/actual、最小重试提示。保持短而可执行。
- policy-check 通过路径不改变：dry-run 仍不写盘。
- 不把 wave/batch 路径接进来，留给 Unit 5。

**Execution note:** 先写单 emit 拒收 fixture 测试；测试只构造最小 workspace + one topic schema。

**Patterns to follow:**
- `emit_policy_check_reject_json_tests` 和 `emit_policy_check_accept_json_tests` 的最小 workspace fixture。
- `report_to_emit_result` 的拒收桥接方式。

**Test scenarios:**
- Error path: `work.done` schema 要求 `task_id` 且有 field doc，payload `{}`，`--policy-check --output json` 返回 Err，序列化响应中的 first error 包含 `field=task_id`、`field_description`、`suggested_payload_shape`。
- Error path: `verdict` allowed values 错误，JSON error 包含 expected allowed values 和 actual value。
- Text path: 同样缺字段时 stderr/text summary 包含字段名和 meaning，不只显示 reason code。
- Happy path: 合法 payload 通过 policy-check，events file 未写入，`recorded=false` 语义保持。
- Backward compatibility: schema 没有 field docs 时，拒收仍成功输出旧字段 + generated placeholder，不 panic。

**Verification:**
- 本 Unit 子集测试覆盖 single emit 路径。
- 本 Unit 不修改 wave、prompt builder、agent docs 或 preset。

- [ ] **Unit 5: Wave/Batch Policy-Check Output Wiring**

**Goal:** 把 Unit 3 的 enriched error 接入 `ralph wave emit --policy-check`，保留 batch atomic reject 和 `payload_index`。

**Requirements:** R5-R8, R14a, SC2a, R18-R20

**Dependencies:** Unit 4

**Files:**
- Modify: `crates/ralph-cli/src/wave.rs`
- Modify: `crates/ralph-cli/src/policy_check.rs`

**Approach:**
- 在 `ValidationFailure::from_batch` 或 wave precheck 调用点，把每个 `ValidationError` enriched 成 agent-facing error。
- 对 wave 的 JSON 输出继续保留 `validation_errors[]` 外层字段，避免破坏已有 parser；在每个 item 增加新字段。
- text 输出保留“多少 payload 失败”的摘要，并增加最常见字段的 field doc / suggested shape 提示。
- 每个 error 必须保留原 `payload_index`，让 agent 一次修完整批。
- 不改变 wave 写盘逻辑、batch fan-out、worker 聚合语义。

**Execution note:** 先写 batch helper / wave precheck 测试；不要借助单 emit 测试证明 wave。

**Patterns to follow:**
- `test_validate_batch_against_config_reports_all_missing_depth_violations`
- `test_wave_emit_json_reports_all_missing_depth_violations`
- `test_wave_emit_rejects_missing_depth_before_write`

**Test scenarios:**
- Error path: 7 个 payload 都缺 `depth`，JSON 中 7 个 `validation_errors` 都有 `payload_index`、`field_description`、`suggested_payload_shape`。
- Error path: 只有 index 3 缺字段，输出只标 index 3，整个 batch 仍拒收且 events file 不变。
- Error path: payload type mismatch 的 batch item 输出 expected shape，不伪造 field。
- Happy path: 全部 payload 合法时 precheck pass 且 events file 不变。
- Backward compatibility: 没有 field docs 的 wave schema 仍输出旧字段和 reason code。

**Verification:**
- 本 Unit 子集测试只覆盖 wave/batch 路径。
- 本 Unit 不修改 prompt builder、docs 或 preset。

- [ ] **Unit 6: Prompt Builder Publish Section**

**Goal:** 让每个 hat 当轮 prompt 中的 schema-aware publish section 展示字段说明、allowed values、示例 payload、policy-check 命令和失败修复动作。

**Requirements:** R11-R14a, R14f, SC4, R18

**Dependencies:** Unit 5

**Files:**
- Modify: `crates/ralph-core/src/emit_schema_hint.rs`
- Modify: `crates/ralph-core/src/instructions.rs`

**Approach:**
- 扩展 `build_publish_emit_section`，每个可 publish topic 输出：
  - topic 名称。
  - required fields + field docs。
  - allowed values 摘要。
  - 示例 payload。
  - 明确“先 policy-check，通过后正式 emit”的流程。
  - 简短说明失败时读取 field-level error 并修 payload。
- 对 wave/batch topic，说明每个 payload item 共享同一 topic schema，失败会带 `payload_index`。
- 保留 fallback：没有 schema match 时仍走 legacy `<summary>` 模板；没有 field docs 时展示 required fields。
- 继续只展示当前 hat publishes 的 topic。

**Execution note:** 先写 `emit_schema_hint` / `InstructionBuilder` 输出字符串测试。

**Patterns to follow:**
- `build_publish_emit_section_only_lists_schemas_topics`
- `InstructionBuilder::build_custom_hat` 当前 schema-aware replacement 逻辑。

**Test scenarios:**
- Happy path: hat publishes `review.synthesized`，schema 有 field docs，prompt section 包含 field meaning、allowed values 和 policy-check instruction。
- Scope guard: hat publishes `work.done`，schema map 同时有 `review.accepted`，prompt 不包含 `review.accepted`。
- Backward compatibility: schema 无 field docs 时仍生成 required fields 示例。
- Fallback: hat publishes topic 但 schema map 为空，legacy summary template 仍出现。
- Wave hint: batch/wave schema 输出包含 `payload_index` 或 batch item 定位说明。

**Verification:**
- 本 Unit 子集测试覆盖 prompt string。
- 本 Unit 不修改 CLI 输出、agent docs 或 preset。

- [ ] **Unit 7: Agent Skill Docs And Instruction Reference Lint**

**Goal:** 让 agent 明确知道如何使用新版 policy-check 反馈，并让 high-risk/builtin preset instructions 通过 lint 引用 skill，而不是复制字段规则。

**Requirements:** R14b-R14e, R14g, R21, SC4a-SC4b

**Dependencies:** Unit 6

**Files:**
- Modify: `crates/ralph-core/data/ralph-tools-emit.md`
- Modify: `crates/ralph-core/data/ralph-tools-cmdref.md`
- Modify: `crates/ralph-core/data/ralph-tools.md`
- Modify: `crates/ralph-core/src/preset_lint/instructions_opac.rs`
- Modify: `crates/ralph-core/src/preset_lint/finding_id.rs`
- Modify: `skills/ralph-preset-common/references/finding-rubric.md`
- Modify: `skills/ralph-preset-common/references/commands.md`

**Approach:**
- 在 `ralph-tools-emit.md` 增加 agent-facing 流程：
  1. 看 prompt 中的 schema-aware publish section。
  2. 按字段说明填 payload。
  3. 运行 `--policy-check`。
  4. 若失败，读取 `code/field/expected/actual/field_description/suggested_payload_shape/suggested_command`。
  5. 修 payload，再 policy-check。
  6. 通过后正式 emit。
- `ralph-tools-cmdref.md` 和 `ralph-tools.md` 只写简明入口，不复制完整规则。
- 在 preset lint 中新增或扩展 finding：publish 业务事件且 instructions 涉及 emit/payload，但未引用 `ralph-tools-emit` 新章节的 high-risk/builtin preset 报错。lint 不尝试判断 prompt builder 是否会自动生成 schema-aware publish section；自动 section 由 `InstructionBuilder` 测试保证，instructions 源文本只负责引用 skill。
- Lint 第一版只针对 builtin/high-risk，避免一次性要求所有用户 preset 补齐。
- 更新 preset operator skill 的 finding rubric/commands，使 preset 作者知道新 lint 的含义。

**Execution note:** 先写 lint 测试；文档更新随后让测试绿。

**Patterns to follow:**
- `check_opac_skill_reference`
- `check_fix_unit_mint_template`
- AGENTS 中“Hat instructions 必须引用 skill doc，不复述内容”的硬规则。

**Test scenarios:**
- Error path: hat publishes business event，instructions 写“build JSON payload”但不引用 `ralph-tools-emit` 新章节，strict lint 返回新 finding。
- Happy path: instructions 引用 `ralph-tools-emit` 新 policy-check feedback 章节，lint 通过。
- Non-goal: instructions 只引用 topic 名称但不涉及 payload/emit，不触发新 lint。
- Documentation check: `ralph-tools-emit.md` 包含新版 error fields 表；`ralph-tools.md` 只包含短入口。

**Verification:**
- 本 Unit 子集测试覆盖 lint。
- 本 Unit 不修改 preset/schema 试点内容；只提供机制和文档。

- [ ] **Unit 8: `ce-executor-pipeline-loop` Schema/Preset Pilot**

**Goal:** 用 `ce-executor-pipeline-loop` 的 review/fix 收敛事件验证字段说明、prompt、policy-check 和 instructions 引用的完整作者体验。

**Requirements:** R15-R17, SC4b, SC6

**Dependencies:** Unit 7

**Files:**
- Create or Modify: `presets/schemas/ce-executor-pipeline-loop.yml`
- Modify: `presets/en/ce-executor-pipeline-loop.yml`
- Modify: `crates/ralph-cli/src/presets.rs`
- Modify if needed: `presets/index.json`
- Modify if needed: `scripts/ralph-zsh-plugin.zsh`
- Modify if needed: `AGENTS.md`
- Modify if needed: `CLAUDE.md`

**Approach:**
- 优先创建 sibling schema SSOT `presets/schemas/ce-executor-pipeline-loop.yml`，把当前 inline `event_policy.schemas` 对齐进去，再为高风险 topic 补 `field_docs` 和 `examples`。
- 为避免 `schema_reference_parity` drift，第一版必须让 sibling schema 与 `presets/en/ce-executor-pipeline-loop.yml` 的 inline schema 在语义上保持一致；如果选择保留 inline topic block，就同步补同一份 `field_docs/examples`，不能只改 sibling。
- 高风险 topic 第一批至少覆盖：
  - `review.synthesized`
  - `review.accepted`
  - `fix.requested`
  - `review.complete`
  - `review.loop.blocked`
- 字段说明至少覆盖：
  - `review_round`
  - `synthesized_review_file`
  - `must_fix_now_count`
  - `residual_findings_count`
  - `fix_plan_file`
  - `verdict`
  - `reason`
- Preset instructions 只引用 `ralph-tools-emit` 新章节和 schema-aware publish section，不复制字段解释。
- 若 builtin preset 描述、manifest、index、zsh completion 或 AGENTS/CLAUDE 描述没有语义变化，记录“不需要改”；若实际改了描述或可见列表，必须同步。
- `AGENTS.md` 与 `CLAUDE.md` 必须保持完全一致。

**Execution note:** 先写/更新 preset lint 或 embedded preset 测试，再补 schema/preset。

**Patterns to follow:**
- `crates/ralph-cli/build.rs` 的 `presets/schemas/<name>.yml` SSOT merge 规则。
- `schema_parity.rs` 对 sibling schema 与 inline schema 的 drift 检查。
- 近期 `ce-executor-pipeline-loop` 收敛门控改动中新增的 `must_fix_now_count` / `residual_findings_count` 字段。

**Test scenarios:**
- Happy path: embedded `ce-executor-pipeline-loop` 的 merged schema 包含 `field_docs.must_fix_now_count`。
- Schema parity: sibling schema 与 inline/embedded schema 没有 drift；若 inline topic block 保留，inline 也包含同一份 field docs，避免 strict lint 报 schema reference parity。
- Lint path: `review-synthesizer` / `review-gate` / `fix-planner` 等 relevant hats 引用新版 emit skill，不复制完整字段说明。
- Prompt path: 针对一个 publishes `review.synthesized` 的 hat，schema-aware section 能展示 `must_fix_now_count` 和 `residual_findings_count` 说明。
- Docs sync: 若改动 AGENTS/CLAUDE，内容完全一致。

**Verification:**
- 本 Unit 子集测试覆盖 preset schema merge、lint、prompt fixture。
- 本 Unit 不新增 runtime behavior；只是试点 schema metadata 和 instructions 引用。

- [ ] **Unit 9: Final Contract Review And Regression Closure**

**Goal:** 做最终回归闭环，证明功能只增强反馈，不改变事件接受/拒绝语义，并完成项目要求的同步检查。

**Requirements:** R18-R21, all SC

**Dependencies:** Unit 8

**Files:**
- Modify only if review finds plan-implementation drift: files already touched by U1-U8.

**Approach:**
- 做跨 Unit 的最终检查，但不新增新功能：
  - 旧 schema 无 metadata 的 single emit 行为不变。
  - 旧 schema 无 metadata 的 wave emit 行为不变。
  - 新 metadata 在 prompt / policy-check / docs 三处一致。
  - `suggested_payload_shape` 不填业务事实。
  - `ce-executor-pipeline-loop` schema SSOT、inline merge、preset instructions、AGENTS/CLAUDE 同步。
- 跑项目要求的 targeted preset/schema 校验和全量测试入口。
- 若 `scripts/ralph-zsh-plugin.zsh` 被改，按项目规则安装到当前用户 zsh 插件并验证 completion loads；该步骤需要用户授权写 home 目录。

**Execution note:** 本 Unit 不写新 feature test；只跑前序 Unit 的聚合回归和项目级检查。若发现缺口，回到对应 Unit 修，不在 U9 偷塞新逻辑。

**Patterns to follow:**
- AGENTS/CLAUDE hard rules 中的 preset/schema 下游同步清单。
- `docs/guide/payload-contracts.md` 的 schema SSOT 描述。

**Test scenarios:**
- Integration: 单 emit missing required field，JSON/text 都含 enriched error；旧字段仍在。
- Integration: wave batch missing required field，所有 offending payload index 都被列出；events file 不变。
- Integration: prompt section 只展示当前 hat 有权 publish 的 topic。
- Regression: 无 metadata schema 的旧 fixture 仍按原规则接受/拒绝。
- Preset: `ce-executor-pipeline-loop` strict lint 通过。
- Documentation: agent skill docs 包含新版流程，preset instructions 引用 skill 而非复制字段说明。

**Verification:**
- 全部 Unit 的 targeted tests 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 通过。
- `cargo nextest run -p ralph-core -- preset_lint` 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded` 通过。
- 最终 `./scripts/run-tests.sh` 通过。

## System-Wide Impact

- **Interaction graph:** EventSchema metadata -> emit_schema_hint -> policy-check output / prompt builder / agent docs。运行时 event loop 接受/拒绝规则不变。
- **Error propagation:** policy-check 拒收仍返回 non-zero；新增字段只让 agent 更容易修复。
- **State lifecycle risks:** `--policy-check` 仍是 dry-run，不写 events；wave reject 仍 atomic，不能半写 batch。
- **API surface parity:** 单 emit、wave emit、prompt section、skill docs 必须用同一套字段名和语义。
- **Integration coverage:** U9 才允许做跨层回归；U1-U8 均只测本 Unit 的输入输出或本 Unit 接线。
- **Unchanged invariants:** topic routing、hat scope、single business event budget、terminal monotonicity、step handoff、policy acceptance semantics 不变。

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| metadata 被误当成校验权威 | U1 明确不接入 runtime validation；测试证明 field_docs 非 required field 不报错。 |
| JSON 输出破坏已有消费者 | 保留外层 `ValidationFailure` / `EmitResult`，只增量添加 optional error fields。 |
| suggested shape 伪造业务事实 | U2/U3 测试要求缺失字段使用占位符，不填 `0` / `pass` 等业务值。 |
| prompt 泄漏其它 hat payload 合同 | U2/U6 保留 hat-scoped publish filtering 测试。 |
| docs/preset/schema 三处漂移 | U7 让 skill 成为通用流程来源；U8 instructions 只引用 skill；字段说明来自 schema SSOT。 |
| Unit 互相缠绕 | U1-U3 纯函数，U4/U5 分别接 CLI，U6 prompt，U7 docs/lint，U8 preset pilot，U9 final regression。 |

## Documentation / Operational Notes

- 必须更新 `crates/ralph-core/data/ralph-tools-emit.md`，这是 agent 学会新版 policy-check 反馈的核心交付。
- 必须同步 `ralph-tools.md` / `ralph-tools-cmdref.md` 的短入口，避免 always-injected 文档仍指向旧行为。
- 若修改 preset/schema，必须遵守 AGENTS/CLAUDE 的 preset/schema 下游同步规则。
- 若修改 AGENTS.md 或 CLAUDE.md，必须保持两者 byte-identical。

## Sources & References

- Origin document: `docs/achieved/brainstorms/2026-07-09-policy-check-agent-feedback-requirements.md`
- Existing schema model: `crates/ralph-core/src/config/loop_config.rs`
- Shared schema hint module: `crates/ralph-core/src/emit_schema_hint.rs`
- Single emit policy-check path: `crates/ralph-cli/src/commands/emit.rs`
- Wave policy-check path: `crates/ralph-cli/src/wave.rs`
- Policy-check structures: `crates/ralph-cli/src/policy_check.rs`
- Prompt builder: `crates/ralph-core/src/instructions.rs`
- Agent emit skill: `crates/ralph-core/data/ralph-tools-emit.md`
- Lint pattern: `crates/ralph-core/src/preset_lint/instructions_opac.rs`
- Prior learning: `docs/solutions/developer-experience/agent-execution-contract-gates-2026-06-03.md`
- Prior plan: `docs/achieved/plan/2026-06-15-001-feat-schema-aware-hat-emit-instructions-plan.md`
- Strict sequential plan model: `docs/achieved/plan/2026-07-06-001-feat-ce-executor-serial-protocol-ssot-convergence-plan.md`
