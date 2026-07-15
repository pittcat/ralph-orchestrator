---
title: 'ce-executor-pipeline: backfill field_docs + examples (agent-facing only) - Plan'
type: feat
date: 2026-07-12
origin: docs/plans/2026-07-09-003-feat-schema-backed-trigger-context-plan.md (trigger_context is loop-pilot only; not extended here)
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: 'presets/en/ce-executor-pipeline-loop.yml line 517+ 的 field_docs / examples 写法模板（仅复用这部分，不复用 trigger_context）'
execution: code
---

# ce-executor-pipeline: backfill field_docs + examples (agent-facing only) - Plan

## Goal Capsule

| Field | Value |
|---|---|
| Objective | 单文件 YAML 编辑：在 `presets/en/ce-executor-pipeline.yml` 的 inline `event_policy.schemas` 每个 emit topic 补 `field_docs`（meaning / source / fill_rule 三子字段），高风险 topic（`work.done` / `fix.done` / `review.synthesized`）补 `examples`。**不补 trigger_context / summary_fields / routing_hints / known_fields** —— 这些是 review-loop topology 上的契约，linear preset 与 loop preset 拓扑不同，本来就不该共用。 |
| Product authority | `presets/en/ce-executor-pipeline-loop.yml` line 517+ 是 field_docs / examples 的写法模板（仅这两个 block 被复用）。trigger_context 在 loop preset 是 U6 试点，与 gate hat 决策绑定，不在本计划范围。 |
| Execution profile | 单文件 YAML 编辑，**不动 Rust**。`field_docs` 与 `examples` 在 `crates/ralph-core/src/{config/loop_config.rs,emit_result/mod.rs}` 已全功能支持：policy-check reject 时把 `field_docs.<f>.meaning` 注入错误信息。 |
| Isolation rule | 只动 `presets/en/ce-executor-pipeline.yml` 一个文件。 |
| Stop conditions | `cargo nextest preset_lint` / `presets` / `scenarios` 三项 verify 任意红 → 立刻回到本计划修订或 fix YAML；不允许「先合再修」。 |
| Tail ownership | 落库后跑 3 项 verify 命令；无须改 skill doc / scenario / 下游同步清单。 |

---

## Product Contract

### Summary

`presets/en/ce-executor-pipeline-loop.yml` 在 `field_docs` / `trigger_context` / `examples` 三件套上做了 pilot。`presets/en/ce-executor-pipeline.yml`（linear 版本）当前只有 `required_fields`。

**本计划只补 `field_docs` + `examples`，刻意不引入 `trigger_context`**：
- `trigger_context` 是把 summary + routing hint 注入到下游 hat 的 prompt。这个 prompt 形状变化会改变下游 hat 的 agent 上下文 —— 即使 R9b 等双字段语义都理清，linear preset 的下游 reviewer-synthesizer / fix-planner / fixer / alignment 都不是为这个 hint 设计的，引入会出现「prompt 多了一段但 instruction 没引用」的漂移。
- `field_docs` 与 `examples` 是**纯说明**：不进入 prompt 渲染（只进入 policy-check reject 的错误信息与 `--show-schema` 类展示命令），不改变 topic routing，不改变 hat 选取，不改变 event flow。**这是真正零回归风险的改造**。
- linear 与 loop 拓扑不同（无 review 循环、无 review-gate），**本来就不该共用 trigger_context**。loop 版本做了 trigger_context pilot 是有意识的圈定范围，不是「linear 也该有」。

### Problem Frame

`review.goalalign.done` schema 当前只有 required fields：

```yaml
review.goalalign.done:
  required_fields:
    - "plan_name"
    - "plan_path"
    - "executor_head_sha"
    - "resolved_baseline_sha"
    - "diff_patch_file"
    - "dimension"
    - "findings_file"
    - "findings_count"
```

补完 `field_docs.findings_file.{meaning, source, fill_rule}` 后，policy-check reject 会附带修复提示（runtime 已实现于 `crates/ralph-core/src/emit_result/mod.rs` 的 `field_description` 字段）。下游 hat 的 prompt 不变，行为不变；只有「agent emit 失败时收到的报错」更可执行。

**为什么不补 trigger_context**：trigger_context 在 strict preset_lint（`crates/ralph-core/src/preset_lint/trigger_context.rs`）会被检查 `summary_fields` 字段是否在 `required_fields ∪ known_fields ∪ field_docs.keys() ∪ allowed_values.keys()`、hint `op` 是否在 allowlist（`eq/ne/gt/gte/lt/lte/exists/missing`）、hint 是否 no_consumer 等。一旦出错 CI 红。而 linear preset 与 loop 拓扑不同，下游 hat 集合不同，hint 集需要重新设计 —— 不是「line 517+ 写法一致」能套用的简单迁移。本计划**主动不引入这个新 contract**，让 linear preset 的 schema metadata 补齐保持在「只补说明」的最小范围。

### Requirements

**Schema metadata backfill（仅 YAML 改动）**

- R1. `presets/en/ce-executor-pipeline.yml` 内每个 inline schema 块必须补齐 `field_docs`，覆盖全部 `required_fields`。
- R2. `field_docs.<f>` 三子字段：`meaning`（agent prompt 内显示给 policy-check 错误信息）、`source`（值从哪段上游 payload 或哪个 trigger 字段来）、`fill_rule`（policy-check reject 后怎么修复）。**写作风格对齐 `presets/en/ce-executor-pipeline-loop.yml` line 517+：单行字符串 + 多段语义用分号切段**；不写成 multiline bullet。**禁止**塞命令字面值（如 `git log --reverse --format=%H -- $plan_path | tail -1`）—— 这些命令细节属于 plan-reviewer 等 hat instructions 范畴，不在 schema metadata 内复述。
- R3. `field_docs` 是 agent-facing 修复引导；只解释「字段是什么 / 哪来 / 怎么填」，**不许**复述 `ralph-tools-emit` §5 参数表，**不许**引用源码行号 / 内部函数名 / `.ralph/events.jsonl` / `.ralph/supervisor.db` / `.ralph/loops.json` 等内部 ledger。
- R3a. `field_docs.<f>` 的 key 必须严格匹配 `required_fields` 中的字段名；runtime 把 schema metadata 当 HashMap 解析，**错字（如 `filed_docs` / `findings_fil`）不会报错但会被静默忽略**。落库前必须 `grep -E '^\s+- "(\w+)"' <topic block>` 对比 `required_fields` 与 `field_docs.keys()`，确认每个 required field 都有同名 doc 条目。

**Examples（仅高风险 topic）**

- R4. `examples` 只对高风险 topic 补：`work.done` / `fix.done` / `review.synthesized`；其余 topic 省。
- R5. `examples` 内容与 `presets/en/ce-executor-pipeline-loop.yml` line 562-575 同款 —— 用真实-shaped 数字（如 `must_fix_now_count: 2`、`verdict: "blocked"`）+ 形如 `2026-07-09-001-feat-policy-check-agent-feedback-plan` 的 plan name token；这些 example 在 prompt / `--show-schema` 里**只**作为 illustration，runtime **不**当作业务事实。禁止固化为「必须 pass」「必须 0」之类的硬约束写法。

**No-trigger-context contract（核心硬约束）**

- R6. **不在** `presets/en/ce-executor-pipeline.yml` 任何 inline schema 块新增 `trigger_context` 块（包含 `summary_fields` / `routing_hints` / `known_fields`）。
- R7. **不**为 `presets/en/ce-executor-pipeline.yml` 新增独立的 `presets/schemas/ce-executor-pipeline.yml` SSOT 文件（沿用 pipeline preset 现有「inline schema」注释）。
- R8. **不**在 hat `instructions:` 引入 `## TRIGGER CONTEXT` 引用或相关 if/else 分支（既有的 `trigger_context` 引用保持原样）。
- R9. **不**为 `plan.blocked` / `work.failed` / `report.done` / `LOOP_COMPLETE` 写 `trigger_context`（即便它们有下游 reporter 消费者）—— 见 R6 整体约束。

**Schema 反模式（必须避免）**

- R10. `field_docs.<f>.fill_rule` 必须是 agent 用当前可见命令或 trigger payload 字段能真正执行的修复；不许写「上游会处理」「按惯例」「待定」。
- R11. `examples` / `field_docs` 内容**不**许复制进 hat `instructions`。

**No topology / source change**

- R12. 不改 hat topology，不改 `triggers` / `publishes`，不改 `topic_deny_rules`，不改 `event_loop`，不改 `required_fields` 集合。
- R13. 不改 `ce-executor-pipeline-loop.yml` 和 `presets/schemas/ce-executor-pipeline-loop.yml`。

### Success Criteria

- SC1. `presets/en/ce-executor-pipeline.yml` 每个 schema 块的 `field_docs` 覆盖所有 `required_fields`；`cargo nextest preset_lint` 全绿。
- SC2. `cargo nextest -p ralph-core --test scenarios`（真 EventLoop 路径）继续 PASS；`cargo nextest -p ralph-cli --bin ralph -- presets`（`presets_array_matches_manifest` / `test_all_embedded_presets_pass_strict_lint`）PASS。
- SC3. `ralph emit --policy-check` reject missing field 后错误信息带 `field_docs.<f>.meaning` 提示（field_docs enrichment 链路保留）。
- SC4. `ce-executor-pipeline-loop` preset 的 schema metadata 不被本计划改动。
- SC5. **零 prompt 形状变化**：所有下游 hat 的 prompt 不因为本计划而新增 / 删除 / 重排任何 block（因为 `trigger_context` 不被引入；`field_docs` 不进 prompt 渲染）。落库后用 BDD scenario prompt-snapshot fixture（如有）对比无 diff；如无 snapshot fixture，靠 SC2 的 scenarios PASS 反向保证。
- SC6. `presets/en/ce-executor-pipeline.yml` 不引入 `trigger_context` block。commit message 内显式声明「不含 trigger_context」。
- SC7. yaml 体积涨幅落在合理区间（review 体感检查，不设硬 KPI）。

### Scope Boundaries

- **唯一改动文件**：`presets/en/ce-executor-pipeline.yml`（只动 line 311 起 `schemas:` 子树）。
- **不含** `trigger_context` / `summary_fields` / `routing_hints` / `known_fields` 任何一个 block。
- 不改任何 `.rs` 文件。
- 不改 `instructions:` 任何字段。
- 不创建 SSOT 文件。
- 不动 `ce-executor-pipeline-loop` preset。
- 不写新测试（runtime 已有 U2/U3 覆盖；结构化 parity 由现有 lint 保证）。
- **不**为 `report.done` / `LOOP_COMPLETE` / `plan.blocked` / `work.failed` 写 trigger_context（与 R9 同因：整计划不引入 trigger_context）。
- **不**触发 `crates/ralph-core/src/preset_lint/trigger_context.rs` strict 检查 —— 因为根本无 trigger_context 声明，lint 跳过。

### Acceptance Examples

- AE1. Given `presets/en/ce-executor-pipeline.yml` schemas 补完 `field_docs`，when reviewer agent `ralph emit` 触发 missing `findings_file`，then error message 包含 `field_docs.findings_file.meaning` 内容「`path to the <dim> product written by <dim hat> Step 4`」之类。
- AE2. Given 下游 hat prompt 在补 field_docs 之前的内容，when plan 落库后，then 下游 hat prompt 中**不**出现 `## TRIGGER CONTEXT` 区块（因为 trigger_context 没引入）；其它章节 byte-identical。
- AE3. Given pipeline preset 完整，when `cargo nextest preset_lint` 跑，then 0 finding；`scenarios` 跑无 fail。
- AE4. Given `presets/en/ce-executor-pipeline.yml` 的 git diff，then 该 diff 在任一 topic 块下不出现 `trigger_context:` / `summary_fields:` / `routing_hints:` / `known_fields:` 之一（regression 哨兵）。

### Dependencies and Assumptions

- 上游：runtime U3 `field_description` enrichment 已落地（plan 2026-07-09-003，commit `d00fb3f6 test(config)` 等）。
- 上游 SSOT：`presets/en/ce-executor-pipeline-loop.yml` line 517+ 是 `field_docs` / `examples` 的写法模板；**仅这两个 block 被复用**。
- 假设：linear preset 的 13 hat 拓扑与 line 500-3100 当前一致；R6 / R9 严格保留「不写 trigger_context」决定。
- 假设：`field_docs` 不进入 prompt 渲染，是 runtime 现状（参考 `crates/ralph-core/src/emit_result/mod.rs` 的 `field_description` 字段 —— 它只在 emit reject 错误信息里出现）。

---

## Implementation Plan

### Single Step — 改一个文件

**Touch**：
- `presets/en/ce-executor-pipeline.yml`，只改 line 311 起的 `schemas:` 子树。**每个 topic 块下只新增 `field_docs:`（必选）+ 可选 `examples:`（仅 3 个高风险 topic）**。**不**新增 `trigger_context` / `summary_fields` / `routing_hints` / `known_fields`。

**Topic 写入清单**：

| topic | field_docs | examples |
|---|---|---|
| `plan.ready` | yes | — |
| `plan.blocked` | yes | — |
| `work.done` | yes | yes |
| `work.failed` | yes | — |
| `review.{6 维}.done` | yes | — |
| `review.synthesized` | yes | yes |
| `review.complete` | yes | — |
| `fix.done` | yes | yes |
| `align.done` | yes | — |
| `report.done` | yes | — |
| `LOOP_COMPLETE` | yes | — |

**字段写作风格**（R2 落地）：照 `presets/en/ce-executor-pipeline-loop.yml` line 517+ 写法 —— 单行字符串 + 多段语义用分号切段。例如：

```yaml
findings_file:
  meaning: "path to the <dim> product written by <dim hat> Step 4; absolute under .ralph/review/<plan>/"
  source: "<dim hat> Step 4 derive from `<plan>/<dim>.md`; runtime reads it via trigger payload pass-through (no agent recomputation)"
  fill_rule: "absolute path under `.ralph/review/<plan>/`; do NOT make up a filename; copy from previous dim event payload"
```

不写 `git rev-parse HEAD` / `cat $FILE` 之类具体命令字符串（命令细节属于 hat instructions）。`fill_rule` 用「做 X / 不做 Y」的 backpressure 写法。

**6 个 `review.*.done` schema**：结构完全一致，建议用 YAML anchor `&dim_done_fields` / `<<: *dim_done_fields` 收敛（与 loop 版本同段写法保持一致）。

**Verify（落库后必跑）**：

1. `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` → 0 finding
2. `cargo nextest run -p ralph-cli --bin ralph -- presets`（含 `presets_array_matches_manifest`）→ PASS
3. `cargo nextest run -p ralph-core --test scenarios` → PASS（BDD 真实 EventLoop 路径）

3 条 verify 全绿才算落库。

**commit 前自检**（R3a 落地）：
- 对每个 topic 的 `field_docs` keys 与同 topic `required_fields` items 做 `grep` 比对；任何 mismatch 即调整，**不**留 silent typo。
- `rg -n 'trigger_context|summary_fields|routing_hints|known_fields' presets/en/ce-executor-pipeline.yml` 必须 0 命中（**R6 哨兵**）。
- commit message 内显式声明「本 commit 不含 trigger_context」+ 列本 commit 触及的 schema metadata 变更清单。

---

## Hard Rules Reminder

| HARD RULE | 本计划合规点 |
|---|---|
| **预设测试规则（HARD RULE）** | 不写测试；runtime 已有的 U2/U3 测试覆盖。 |
| **AI skill guide 同步规则（HARD RULE）** | 无须改 `crates/ralph-core/data/*.md`；本计划没新增 CLI / topic / field。 |
| **AI skill guide 可读性规则（HARD RULE）** | `field_docs` 写「agent 下一步能干什么」，不写实现细节（不引用源码行号、不引用 `.ralph/events.jsonl` / `.ralph/supervisor.db`）。 |
| **AI skill guide 去计划化规则（HARD RULE）** | `field_docs` 不含 plan 编号、不含 U 编号、不含具体 plan 名、不含诊断报告路径。 |
| **preset yml 改动后必须同步 schema 并跑校验（HARD RULE）** | preset 已有 inline schema；本计划只补 metadata，不改 required_fields。3 条 verify 已覆盖。 |
| **Backpressure Over Prescription** | `fill_rule` 用 backpressure 写法（描述字段是啥 + 哪来），不给 agent 写脚本化操作步骤。 |
| **All 中文输出** | plan / notes / 报告中文；YAML 字段名英文。 |

> 注：本计划的「下游同步清单 8 步」被刻意收窄为「不适用」（lint 已有、scenario 不动、不改源码）。任何「我是不是该改 Rust」的冲动都先回看本段。

---

## Verification 一览

| 命令 | 必跑 | 用途 |
|---|---|---|
| `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | ✅ | preset structural lint |
| `cargo nextest run -p ralph-cli --bin ralph -- presets` | ✅ | `presets_array_matches_manifest` + `test_all_embedded_presets_pass_strict_lint` |
| `cargo nextest run -p ralph-core --test scenarios` | ✅ | BDD 真实 EventLoop 路径不退化 |
| `rg -n 'trigger_context\|summary_fields\|routing_hints\|known_fields' presets/en/ce-executor-pipeline.yml` | ✅ | **R6 哨兵**：本计划必须零命中 |

**非必跑**（仅在怀疑有 regression 时跑）：
- `cargo nextest run -p ralph-core --features recording --test smoke_runner` —— pipeline smoke replay；本次元数据改动不触碰策略，按需跑。
- `./scripts/run-tests.sh` —— 全 workspace，时间成本高；只有 SC 失败时再跑兜底。

---

## Rollback

单文件改动。回滚 = `git revert <commit>` 一个文件即可。

**应急**：若 verify 报错，按报错位置（哪个 topic）回退 YAML 块；不允许整体丢弃 `field_docs` 改回去消除报错（policy-check reject 体验会回归旧）。

---

## 产出物清单

| 文件 | 状态 |
|---|---|
| `presets/en/ce-executor-pipeline.yml` | **唯一改这个**，只动 `schemas:` 子树的 `field_docs` + 3 处 `examples` |

无其它文件改动。
