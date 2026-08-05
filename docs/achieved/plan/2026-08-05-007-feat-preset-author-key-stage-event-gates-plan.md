---
title: Preset Author 关键环节事件门禁 - Plan
type: feat
date: 2026-08-05
topic: preset-author-key-stage-event-gates
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-brainstorm
execution: docs
deepened: 2026-08-05
---

# Preset Author 关键环节事件门禁 - Plan

## Goal Capsule

- **目标：**只增强 `ralph-preset-author` 与 `ralph-preset-review` 两套 operator skill，使 author 在设计 preset 时识别关键事件位置，并逐位置询问用户是否加入 `precheck guard`、`payload consistency` 及各自的 retry budget。
- **产品权威：**author agent 负责提出基于职责信号的判断和建议；用户拥有最终选择权；reviewer 独立复核，不继承 author 的 scope 判断。
- **修改范围：**仅限 `skills/ralph-preset-author/`、`skills/ralph-preset-review/` 及其 references、fixtures、skill contract tests。
- **明确不改：**Rust runtime、配置解析、preset YAML/schema、`crates/ralph-core/data/*.md`、operator guide、事件计数、recovery 实现和现有 runtime 行为。
- **停止条件：**关键位置、用户选择的 guard 类型、各自 retry budget、降级/关闭原因和未确认停止状态未形成可审计 notes 与 review 规则时，不得宣称完成。

## Product Contract

### Summary

为 preset author 增加按关键事件位置确认事件门禁的流程。author 先识别哪些 handoff 或阶段属于关键位置，再逐位置询问是否加入 `precheck guard`、`payload consistency` 或两者，并为启用的每类 guard 确认独立 retry budget。两类 guard 的 retry、correction 和 blocked 语义继续沿用现有 runtime；本计划不修改 runtime 使其产生任何新行为。

### Requirements

- R1. author 必须基于可观察职责信号识别关键事件环节：终态 authority、阶段分支或恢复决策、生产修改后的交付、跨 hat 汇总、关键 artifact 和关键 handoff。
- R2. author 必须说明每个关键环节需要门禁的理由，以及关闭门禁的风险；不得只展示无理由开关。
- R3. author 必须把每个关键位置的 `precheck guard`、`payload consistency`、失败打回 producer、bounded correction/retry 和 blocked 作为该位置的候选方案解释。
- R4. author 必须逐个关键位置询问用户选择：加入 `precheck guard`、加入 `payload consistency`、两者都加入，或暂不加入；对每个加入的 guard 分别询问 retry budget，默认值为 3，可选 3、2、1；不得用一次 preset 全局选择替代逐位置确认。
- R5. 用户选择暂不加入某类 guard，或选择低于推荐覆盖范围/默认 budget 时，必须记录原因；两类 guard 的 retry budget 必须分别记录，不得合并为一个总预算。
- R6. 用户未完成某个关键环节的选择前，author 不得把该选择当作已确认事实，也不得继续生成依赖它的最终 YAML/schema 设计。
- R7. `preset-author-notes.md` 必须能让 reviewer 复核关键位置、agent 建议、用户选择的 guard 类型、未启用原因和后续设计是否遵守选择。
- R8. skill 只能引用现有 retry、correction 和 blocked 语义，不得发明 runtime 配置、计数器、恢复路径或绕过门禁的替代行为。

### Decision semantics

现有 Gate Scope 的 `hard / record / off` 是 AAF 质量门禁，必须保持不变，不得复用为本能力的 guard 位置选择字段。

新增的是按关键位置的 guard 选择：

```text
关键位置：<key stage / handoff>
guard 选择：precheck / payload consistency / both / neither
已启用 guard 的 retry budget：3（默认）/ 2 / 1，按 guard 分别设置
```

用户选择的是某个关键位置需要哪些 guard，以及每类已启用 guard 的独立 retry budget；默认 budget 为 3。两类 guard 不共享总预算、计数器或 exhaustion state；其余 retry、correction 语义继续引用现有 runtime。

### Key flows

1. Author 完成 Intent Confirmation 后，按 capability signal 列出关键 topic，并解释风险。
2. 用户逐关键位置选择 `precheck guard`、`payload consistency`、两者或暂不加入。
3. Author 将位置、guard 选择、各自 retry budget、建议、理由和未确认状态写入 `preset-author-notes.md`。
4. Reviewer 独立重建关键 topic scope，再将 author notes 作为用户选择证据进行对账。
5. Reviewer 发现关键位置缺少 guard 选择、notes 与 preset 设计不一致、缺少原因或未确认即继续设计时，报告明确 finding。

### Acceptance examples

- AE1. 关键 handoff 的 notes 同时包含关键性理由、门禁建议和风险说明。
- AE2. 对一个关键位置，用户可以选择只加入 `precheck guard`，并将其 retry budget 设为 3；notes 明确记录 payload consistency 未加入及理由。
- AE3. 对另一个关键位置，用户可以选择只加入 `payload consistency`，或选择两者都加入，并分别确认各自 budget。
- AE4. 用户选择暂不加入某类 guard，或将其 budget 降到 2/1 时，无理由不能继续；两类 guard 不得压缩为一个总 `retry_budget`。
- AE5. 未完成关键位置选择时，author 停止最终 YAML/schema drafting。
- AE6. reviewer 能发现关键位置缺少 guard 选择、notes 与 preset 设计不一致，或把现有 Gate Scope `off` 当成事件位置选择。

## Planning Contract

### Agent-native constraints

- author 的判断必须基于 capability inventory、实际 topic/schema、prompt visibility 和现有 skill 指南，不能依据 preset/hat 名称猜测关键性。
- author notes 必须在 preset 同目录，包含独立字段：`key_stage`、`guard_selection`、`precheck_guard`、`precheck_retry_budget`、`payload_consistency_guard`、`payload_consistency_retry_budget`、`reason`、`confirmation_status`。
- reviewer 必须先独立重建 scope，再读取 notes；notes 只能作为用户选择证据。
- author/reviewer 的新字段不得覆盖现有 `decision_gate`、`decision_gate_scope`、`hard/record/off` 语义。
- skill 只描述 agent 下一步动作、字段来源和停止条件，不描述或新增 Rust 内部实现。

### Key technical decisions

- KTD1. 按关键位置询问 guard 类型和各自 retry budget；默认 budget 为 3，precheck 与 payload consistency 分别记录，不共享总预算。
- KTD2. 只改 author/reviewer 的工作流、notes contract、finding rubric、commands/reference、fixture 和 skill anchors。
- KTD3. 不新增 YAML 字段、不改 preset schema、不改 runtime counter、不改 recovery identity；skill 只记录用户希望在哪些位置加入哪类 guard、各自使用多少 budget，并引用现有 runtime 语义。
- KTD4. reviewer 独立复核 scope 和 guard 选择字段，不把 author notes 当作 runtime 事实源。
- KTD5. 某个位置暂不加入 guard 不等于关闭既有 AAF、Payload Contract 或 mechanical lint 审查。

## Implementation Units

### U1. 更新 author skill 与 references

**Files:**

- `skills/ralph-preset-author/SKILL.md`
- `skills/ralph-preset-author/references/author-checklist.md`
- `skills/ralph-preset-author/references/commands.md`
- `skills/ralph-preset-author/references/finding-rubric.md`
- `skills/ralph-preset-author/references/patterns.md`
- `skills/ralph-preset-author/references/agent-native-model.md`

**Approach:**

1. 在现有 Workflow 0d 的 Gate Scope 之后增加按关键位置的 guard 选择步骤。
2. 明确 Gate Scope 的 `hard/record/off` 与新增 guard 选择不是同一字段、不是同一问题。
3. 让 author 对每个关键位置询问 `precheck`、`payload consistency`、`both` 或 `neither`；对已选择的每类 guard 询问 3/2/1 retry budget，默认 3。
4. 在 notes contract 中固定位置、guard 选择、各自 budget、建议、理由和确认状态。
5. 明确未确认时停止；不允许按 preset/hat 名称静默套用选择。
6. 所有 retry、correction、blocked 描述引用现有 skill/data 指南，不新增 runtime 规则。

**Verification:** author workflow、checklist、commands 和 rubric 对关键位置、四类 guard 选项、各自 3/2/1 budget、理由和停止条件保持一致；不出现共享总 `retry_budget`。

### U2. 更新 reviewer skill、fixture 与跨 skill contract

**Files:**

- `skills/ralph-preset-review/SKILL.md`
- `skills/ralph-preset-review/references/author-checklist.md`
- `skills/ralph-preset-review/references/commands.md`
- `skills/ralph-preset-review/references/finding-rubric.md`
- `skills/ralph-preset-review/references/patterns.md`
- `skills/ralph-preset-review/references/agent-native-model.md`
- `skills/ralph-preset-review/fixtures/`
- `skills/ralph-preset-review/tests/test_skill_anchors.py`
- `skills/tests/test_execution_model_contract.py`

**Approach:**

1. reviewer 独立重建关键 topic scope，不继承 author scope。
2. 对照 notes 检查每个关键位置的 guard 选择、各自 budget、建议、理由和确认状态。
3. 增加 skill-level finding：缺少位置选择、无理由选择 `neither`、缺少独立 budget、notes 与实际 guard 覆盖不一致、Gate Scope 字段复用。
4. fixture 覆盖正向、缺字段、guard 选择不一致和无理由不启用；不锁定完整 prompt 文案。
5. 保持选择 `neither` 只表示该位置不新增本次 guard，不削弱既有 AAF、Payload Contract 和 mechanical lint。

**Verification:** 正向 fixture 通过；负向 fixture 能区分缺确认、guard 覆盖不一致和无理由不启用；两套 skill 的字段与 finding anchors 一致。

## Verification Contract

| Gate | Scope | Expected proof |
|---|---|---|
| Skill contract tests | U1–U2 | author/review anchors、字段、选项、finding 和 fixture 通过 |
| Python environment | U2 | 使用仓库 `.venv` 运行受影响 skill tests |
| Static consistency | U1–U2 | `hard/record/off` 与新增事件门禁字段不混用；无 Rust/runtime 文件改动 |

不运行 Rust targeted tests、BDD、preset lint 或 `./scripts/run-tests.sh`，因为本计划不修改 Rust、preset YAML/schema 或 runtime 行为。

## Risks & Mitigations

- **字段语义混淆：**在两套 skill 中明确现有 Gate Scope 字段与新增事件门禁字段的区别，并用 anchor test 固定。
- **交互与 runtime 混淆：**notes、commands、rubric 和 fixture 记录两类 guard 各自的用户 budget，但不新增 runtime 规则或共享总预算。
- **skill 越权定义 runtime：**所有 runtime 语义只引用现有指南；review 对新增 Rust/config/runtime 内容报告越界。
- **author notes 被 reviewer 过度信任：**review 先独立重建 scope，再消费 notes 作为选择证据。
- **prompt 文案测试脆弱：**只测试结构化字段、工作流顺序、finding 和 fixture，不做完整 prompt byte equality。

## Definition of Done

- author skill 能逐关键位置识别、解释、询问并记录 guard 选择。
- notes 明确记录 `key_stage`、`guard_selection`、`precheck_guard`、`precheck_retry_budget`、`payload_consistency_guard` 和 `payload_consistency_retry_budget`；两个 budget 默认均为 3 且不共享。
- reviewer skill 能独立复核 scope，并发现 guard 覆盖不一致、缺少理由和未确认继续设计。
- author/review references、fixtures 和 anchors 已同步。
- 只修改 skill 相关文件；无 Rust、preset YAML/schema、runtime data 或 operator guide 改动。
- 受影响 skill contract tests 通过。
