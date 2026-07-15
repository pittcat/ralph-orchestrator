---
title: Trigger Context Alignment Fix Plan
type: fix
date: 2026-07-09
origin: docs/plans/2026-07-09-003-feat-schema-backed-trigger-context-plan.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

# Trigger Context Alignment Fix Plan

## Goal Capsule

| Field | Value |
|---|---|
| Objective | 修复 Trigger Context 试点实现与原计划之间的 P1/P2 对齐缺口，让 schema-backed hints 真正成为下游 hat 判断 review/fix 分支的单一语义来源。 |
| Source of truth | `docs/plans/2026-07-09-003-feat-schema-backed-trigger-context-plan.md` 与本轮目标一致性审查结论。 |
| Scope | 只处理 P1/P2：去除试点 instructions 的分支双写、补 max-round 场景验收、明确门控字段语义、处理 source hat 可选性。 |
| Non-goals | 不新增动态 routing，不改变 event bus / hat selection / policy-check，不重做 Trigger Context builder 或 lint 架构。 |

## Problem Frame

当前功能主链路已经打通：schema 声明 `trigger_context`，runtime 注入 `## TRIGGER CONTEXT`，strict lint 校验字段、谓词和 topology，`ce-executor-pipeline-loop` 已试点 review/fix convergence hints。

剩余问题不在“能不能跑”，而在语义收口：

- 试点 hat instructions 仍保留三分支 payload 模板，和 schema hints 双写。
- 原计划写的是 `must_fix_now_count` 驱动分支，实现改成 `blocking_main_conflict_count` 驱动分支，但计划/验收尚未显式承认该业务决策。
- schema 声明了 `max_round_blocked`，但缺少真实 EventLoop 场景证明该 hint 会进入 `review-gate` prompt。
- renderer 支持 source hat，但 runtime 事件没有 origin hat，只能显示 `(unknown source hat)`；需要明确这是 v1 可接受妥协，或补真实来源。

## Requirements

- R1. `review-gate` instructions 不再复制 `accept / fix / blocked` 三分支条件；分支判断只来自 matched routing hints。
- R2. 保留必要 payload 字段清单和 `ralph emit --policy-check` 强约束，但不要在 instructions 中重复写 `B == 0`、`B > 0 && N < 6`、`B > 0 && N >= 6` 这类 hint 条件。
- R3. 明确 v1 试点的 gate-driving 字段是 `blocking_main_conflict_count`；`must_fix_now_count` 是兼容和报告字段。对应计划或维护文档必须同步，避免后续 reviewer 按旧 SC2/SC3 误判。
- R4. 增加真实 EventLoop scenario，覆盖 `review_round >= 6` 且 `blocking_main_conflict_count > 0` 时，`review-gate` prompt 包含 `max_round_blocked` guidance。
- R5. 对 source hat 做产品决策：若不补 origin hat，则将 Trigger Context contract 改为 `source_hat` optional，并确保 docs/tests 不要求真实 source hat。
- R6. 修复后必须跑 nextest 入口，不得裸跑 `cargo test -p ralph-cli`。

## Implementation Units

### U1. 收敛试点 instructions，消除分支双写

**Files**

- `presets/en/ce-executor-pipeline-loop.yml`
- 如有 byte-equality / embedded preset 断言要求，同步 `crates/ralph-cli/src/presets.rs` 相关测试期望。

**Approach**

把 `review-gate` 的三段条件化 payload 示例删除或降级为无分支字段清单。保留：

- “先读 `## TRIGGER CONTEXT`”
- “matched hint 指定要 emit 的 topic”
- 必须透传的字段清单
- `ralph emit --policy-check` 先行

不要让 instructions 再表达 `blocking_main_conflict_count` / `review_round` 的分支条件。

**Verification**

- `rg -n "Accept when|Request fixes when|Block when|B == 0|B > 0" presets/en/ce-executor-pipeline-loop.yml` 不再命中 review-gate 分支模板。
- `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`

### U2. 明确 gate field 决策

**Files**

- `docs/plans/2026-07-09-003-feat-schema-backed-trigger-context-plan.md` 或新增本修复计划下的维护说明
- `presets/schemas/ce-executor-pipeline-loop.yml`
- `presets/en/ce-executor-pipeline-loop.yml`

**Approach**

记录设计决策：`blocking_main_conflict_count` 是 v1 试点的 gate-driving count，因为它排除了 baseline / out-of-scope / newly discovered residuals；`must_fix_now_count` 仍保留在 summary fields 中，供报告和兼容旧 payload 心智使用。

如果修改 003 原计划，限制为追加 “implementation correction note”，不要大改原 Product Contract。

**Verification**

- schema `field_docs` 和 instructions 对 `blocking_main_conflict_count` 的解释一致。
- 不再出现“`must_fix_now_count > 0` 必然 request fixes”的说明。

### U3. 补 max-round Trigger Context 场景

**Files**

- `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_max_round_blocked.yml` 或扩展现有 `ce_executor_pipeline_loop*.yml`
- `crates/ralph-core/tests/scenarios.rs`

**Approach**

新增真实 EventLoop runner 场景：构造 `review.synthesized` payload：

- `review_round: 6`
- `blocking_main_conflict_count: 1`
- `must_fix_now_count: 1`
- `residual_findings_count` 任意非关键值

断言 `review-gate` prompt 包含：

- `## TRIGGER CONTEXT`
- `[max_round_blocked]`
- `review.loop.blocked`
- `Do not request another fix round`

**Verification**

- `cargo nextest run -p ralph-core --test scenarios -- ce_executor_pipeline_loop`

### U4. 决定并同步 source hat optional 语义

**Files**

- `crates/ralph-core/src/trigger_context.rs`
- `crates/ralph-core/src/event_loop/tests/u3_trigger_context_prompt.rs`
- `crates/ralph-core/data/ralph-tools.md`
- `skills/ralph-preset-common/references/agent-native-model.md`

**Approach**

建议 v1 不补事件 origin hat，直接把 contract 写清楚：`source topic` 必须准确；`source hat` 是 optional，runtime 无法确认时显示 `(unknown source hat)`，agent 不应依赖它做分支判断。

只有当后续确认 Event 本身已有可靠 origin hat 字段时，才改 runtime 注入真实 source hat；不要为了这个 P2 扩大事件模型。

**Verification**

- prompt 测试明确接受 `(unknown source hat)`。
- agent/operator docs 不再把 source hat 描述成必然存在的字段。
- `scripts/check-cli-doc-drift.sh`

## Final Verification

- `cargo nextest run -p ralph-core -- trigger_context`
- `cargo nextest run -p ralph-core -- preset_lint`
- `cargo nextest run -p ralph-core --test scenarios -- ce_executor_pipeline_loop`
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
- `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`
- `scripts/check-cli-doc-drift.sh`
- 最终收口前跑 `./scripts/run-tests.sh`

## Definition of Done

- review/fix 分支条件只在 schema `routing_hints` 中表达，hat instructions 不再复制。
- `blocking_main_conflict_count` 作为 gate-driving field 的业务决策被明确记录。
- max-round blocked hint 有真实 EventLoop prompt 场景覆盖。
- source hat optional 语义被文档和测试接受，或 runtime 已可靠注入真实 source hat。
- 所有 targeted verification 与最终 nextest baseline 通过。
