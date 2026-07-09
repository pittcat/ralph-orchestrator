---
title: fix: Stabilize pipeline-loop main-conflict convergence
type: fix
status: active
date: 2026-07-09
---

# fix: Stabilize pipeline-loop main-conflict convergence

## Overview

`ce-executor-pipeline-loop` 现在的收敛判断太像“每轮重新开题”：只要当前轮 review 又发现 P0/P1，就容易继续生成新的 fix-plan，导致越修越审、越审越大。

本计划把收敛标准改成更稳定的一句话：

> P0/P1 先判断是不是当前 loop 的主要矛盾；只有主要矛盾继续阻塞，非主要矛盾进入最终报告。

## Problem Frame

第一轮审核出来的 P0/P1 通常是这次任务的主债务，必须修。第二轮、第三轮新冒出来的 P0/P1 可能也严重，但不一定是当前 loop 必须解决的主要矛盾。如果它不是上一轮承诺要修但没修好的问题，也不是当前修复明确引入的新回归，就不应该继续扩大 fix-plan。

目标是让 loop “越修越接近完成”，而不是让 review 每轮不断扩大工作范围。

## Requirements Trace

- R1. 第一轮 P0/P1 默认是主要矛盾，应进入 fix-plan。
- R2. 后续轮次中，上轮要求修但没修好的 P0/P1 继续阻塞。
- R3. 后续轮次中，当前 fix diff 明确引入的新 P0 回归继续阻塞。
- R4. 后续轮次中新发现但不是当前修复导致的 P0/P1，进入 residual/report，不再阻塞 loop。
- R5. `review-gate` 不再只看当前轮 `must_fix_now_count`，而是看“主要矛盾是否仍未关闭”。
- R6. 不新增脆弱的 instruction 文案测试；不要靠固定大段 prompt 文字来证明行为。

## Scope Boundaries

- 不重写 pipeline-loop 拓扑。
- 不新增新 hat。
- 不改变最大轮数 6。
- 不引入复杂 runtime 状态机。
- 不新增大量测试；尤其不加“检查 instructions 是否包含某段文字”的文本锁死测试。

## Context & Research

### Relevant Code and Patterns

- `presets/schemas/ce-executor-pipeline-loop.yml` 是该 preset 高风险 review/fix topic 的 schema SSOT。
- `presets/en/ce-executor-pipeline-loop.yml` 已有 `must_fix_now` / `residual_report_only` 分类，但目前主要靠 instructions 软约束。
- `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop.yml` 覆盖第一轮直接 accept。
- `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_fix_reentry.yml` 覆盖第一轮 fix、第二轮 accept。
- `crates/ralph-cli/src/presets.rs` 有 embedded preset / schema SSOT 合并断言。
- `skills/ralph-preset-common/references/patterns.md` 已描述 pipeline-loop，但其中“第 1-3 轮 P0/P1 阻塞”的旧口径需要同步成主要矛盾口径。

### Institutional Learnings

- `docs/achieved/brainstorms/2026-07-07-ralph-single-chain-execution-primary-requirements.md` 强调单链、单消费者、不要让 runtime 复杂化。
- `docs/achieved/brainstorms/2026-07-02-ce-executor-pipeline-preset-requirements.md` 强调 pipeline 的价值在于清晰链路和报告残留，不是无限回环。
- `docs/achieved/brainstorms/2026-06-16-ce-executor-loop-stability-requirements.md` 强调 schema/prompt/precheck/loop gate 同源，避免契约漂移。

## Key Technical Decisions

- **主要矛盾字段化**：在 schema 中表达“当前阻塞项是不是主债务”，不要只靠 reviewer 自觉。
- **后续新发现默认 report-only**：Round N > 1 的新 P0/P1，证据不足时默认不阻塞。
- **新回归必须有证据**：只有能指向当前 `fixed_from_sha..head_sha` fix diff 的新 P0，才算 `new_regression` 并继续阻塞。
- **测试保持轻量**：不加 prompt 文案锁死测试；只更新必要的 schema/preset 断言和现有场景 payload。

## High-Level Technical Design

> This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.

| Finding 类型 | 是否阻塞 | 原因 |
|---|---:|---|
| 第一轮 P0/P1 | 是 | 初始主债务 |
| 上轮 fix-plan 要求修但仍未修好 | 是 | 承诺债务未关闭 |
| 当前 fix diff 明确引入的新 P0 | 是 | 修复引入严重回归 |
| 后续新发现但非当前修复导致 | 否 | 记录到报告，避免扩大 loop |
| baseline / out-of-scope | 否 | 不属于当前 loop 主要矛盾 |

`review-gate` 的判断从：

```text
must_fix_now_count > 0 -> 继续修
```

调整为：

```text
blocking_main_conflict_count > 0 -> 继续修
blocking_main_conflict_count == 0 -> accept，残留进报告
```

## Implementation Units

- [ ] **Unit 1: 收敛字段定稿**

**Goal:** 给 `review.synthesized`、`fix.requested`、`review.complete`、`review.accepted` 增加主要矛盾相关字段。

**Requirements:** R1, R2, R3, R4, R5

**Dependencies:** None

**Files:**
- Modify: `presets/schemas/ce-executor-pipeline-loop.yml`
- Modify: `presets/en/ce-executor-pipeline-loop.yml`

**Approach:**
- 增加最小字段集，建议包括：
  - `main_conflict_count`
  - `unfixed_previous_count`
  - `new_regression_p0_count`
  - `newly_discovered_residual_count`
  - `blocking_main_conflict_count`
  - `loop_decision_basis`
- `blocking_main_conflict_count` 是 gate 的主判断字段。
- `must_fix_now_count` 可以保留兼容现有语义，但说明它不再等同于“所有当前轮 P0/P1”。

**Test scenarios:**
- Test expectation: none -- 本单元是 schema 契约设计，后续单元通过现有 lint/场景覆盖。

**Verification:**
- schema 和 preset inline block 同步。
- 字段说明能让 agent 明白“严重程度不等于主要矛盾”。

- [ ] **Unit 2: 改写 review-synthesizer / review-gate / fix-planner 规则**

**Goal:** 把主要矛盾判断写进 agent 可执行规则，避免每轮新发现都进入 fix-plan。

**Requirements:** R1, R2, R3, R4, R5

**Dependencies:** Unit 1

**Files:**
- Modify: `presets/en/ce-executor-pipeline-loop.yml`

**Approach:**
- `review-synthesizer` 分类规则改成：
  - Round 1 P0/P1 默认主债务。
  - Round N > 1 只把 `unfixed_previous` 和有证据的 `new_regression` 算作阻塞。
  - `newly_discovered`、`baseline_existing`、`out_of_scope` 进入 residual。
- `review-gate` 决策改看 `blocking_main_conflict_count`。
- `fix-planner` 只为主要矛盾生成 Unit，不为 residual 生成修复 Unit。

**Test scenarios:**
- Test expectation: none -- 不新增 instruction 文案包含测试；避免把 prompt 文字锁死。

**Verification:**
- 人读 instructions 时能清楚看出“P0/P1 要先判断是不是主要矛盾”。
- 没有要求 hat 读取不可见内部 ledger。

- [ ] **Unit 3: 最小场景同步**

**Goal:** 让现有 BDD 场景 payload 满足新 schema，并只在必要时补一个行为场景。

**Requirements:** R5, R6

**Dependencies:** Unit 1, Unit 2

**Files:**
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop.yml`
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_fix_reentry.yml`
- Modify: `crates/ralph-core/tests/scenarios.rs` only if adding one new scenario is genuinely needed

**Approach:**
- 先只更新现有场景 payload 的 required fields。
- 不新增 Rust 文案断言。
- 如实现后发现没有任何行为覆盖“第二轮出现 report-only P0 但仍 accept”，最多新增一个 BDD 场景，断言事件结果，不断言 prompt 文本。

**Test scenarios:**
- Happy path: 第一轮无主要矛盾，直接 `review.accepted`。
- Happy path: 第一轮有主要矛盾，修复后第二轮 `blocking_main_conflict_count=0`，即使有 residual 也 `review.accepted`。
- Error path: 第二轮 `new_regression_p0_count>0` 时，仍走 `fix.requested` 或达到上限后 `review.loop.blocked`。

**Verification:**
- 行为验证只看事件流和 payload 字段，不锁定 instructions 文字。

- [ ] **Unit 4: 同步 preset/operator 文档**

**Goal:** 把主要矛盾规则同步到 loop 外 preset author/review 规则，避免未来 review skill 继续按旧口径误判。

**Requirements:** R4, R5, R6

**Dependencies:** Unit 2

**Files:**
- Modify: `skills/ralph-preset-common/references/patterns.md`
- Modify: `skills/ralph-preset-common/references/finding-rubric.md` if needed
- Modify: `skills/ralph-preset-common/references/author-checklist.md` if needed

**Approach:**
- 把旧的“第 1-3 轮 P0/P1 阻塞”改成“主要矛盾阻塞”。
- 说明 P0/P1 是严重程度，不自动等于当前 loop 阻塞项。
- 只同步 operator skill；不需要改 `crates/ralph-core/data/*.md`，除非实现中改变 agent 可见通用命令或 `ralph emit` 行为。

**Test scenarios:**
- Test expectation: none -- 文档同步不新增自动测试。

**Verification:**
- operator skill 不再鼓励每轮新 P0/P1 自动进入 fix-plan。

- [ ] **Unit 5: 验收与漂移检查**

**Goal:** 用最小命令确认 schema/preset/场景没有漂移。

**Requirements:** R6

**Dependencies:** Unit 1, Unit 2, Unit 3, Unit 4

**Files:**
- Modify: none expected

**Approach:**
- 跑 preset/schema 相关的最小 nextest 子集。
- 跑相关 BDD 场景。
- 跑 CLI doc drift 检查仅在实现触及 agent guide 或 CLI 帮助时需要。
- 最终按项目硬规则，准备完成前跑 `./scripts/run-tests.sh`。

**Test scenarios:**
- Test expectation: none -- 本单元是验收动作，不写新测试。

**Verification:**
- `ce-executor-pipeline-loop` schema SSOT 与 embedded preset 不漂移。
- 相关场景通过。
- 没有新增脆弱的 prompt 文本断言。

## System-Wide Impact

- **Interaction graph:** 不改 topic 拓扑，只改变 review/fix loop 的决策字段和分类规则。
- **Error propagation:** 主要矛盾未关闭继续 fix；非主要矛盾进入 report，不再扩大 loop。
- **State lifecycle risks:** 最大风险是字段口径不一致，导致 synthesizer、gate、fix-planner 对同一 finding 分类不同。
- **Unchanged invariants:** 单消费者链、isolated mode、6 轮上限、policy-check 要求不变。

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| agent 把所有 P0 继续当 blocker | schema 字段和 instructions 同时强调主要矛盾 |
| 新字段太多导致 payload 易错 | 字段保持最小，只保留 gate 真正需要的计数和说明 |
| 测试变成文案锁死 | 明确不新增 instruction 文本断言，只测事件行为 |
| residual 被误解成不重要 | reporter 仍必须记录 residual，后续可单独开任务 |

## Documentation / Operational Notes

- 需要同步 `skills/ralph-preset-common/references/patterns.md` 的 pipeline-loop 口径。
- 不需要更新 `AGENTS.md` / `CLAUDE.md`，除非实现中改变 builtin preset 列表或用户可见命令。
- 不需要更新 zsh completion，因为不新增/删除/重命名 preset。

## Sources & References

- Related preset: `presets/en/ce-executor-pipeline-loop.yml`
- Schema SSOT: `presets/schemas/ce-executor-pipeline-loop.yml`
- Existing scenarios: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop.yml`
- Existing scenarios: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_fix_reentry.yml`
- Historical context: `docs/achieved/brainstorms/2026-07-07-ralph-single-chain-execution-primary-requirements.md`
- Historical context: `docs/achieved/brainstorms/2026-07-02-ce-executor-pipeline-preset-requirements.md`
