---
title: "feat: ce-executor-pipeline 线性一条龙执行 Preset"
type: feat
status: active
date: 2026-07-02
origin: docs/brainstorms/2026-07-02-ce-executor-pipeline-preset-requirements.md
---

# feat: ce-executor-pipeline 线性一条龙执行 Preset

## Overview

新增一个 builtin isolated preset `ce-executor-pipeline`：一条严格单链路、单消费者、一环扣一环的流水线，把「拿一份计划 → 自动跑完 → 出报告」做成一条龙。相比 `ce-executor-serial`（10-hat、per-unit 迭代、独立 validator、shipper、coordinator 循环式评审），本 preset **砍掉**：per-unit 任务拆分（`tasks.enabled: false`，整份执行）、独立 validator hat、shipper、coordinator 循环（改扁平直链）；**保留**：TDD 执行 + 全量测试全绿硬门槛（内建 executor）、6 维度评审 + synthesizer 汇总；**新增**：前置计划评审修复关、报告前对齐关。

流水线（共 12 功能 hat + 1 fallback = 13 hats）：
`plan-reviewer → executor → dim(goal-alignment→correctness→testing→maintainability→project-standards→adversarial) → review-synthesizer → fixer → alignment → reporter`，外加 `progress-steward` 兜底。

关键取舍（用户 2026-07-02 修正）：reviewer **不**用单 hat + 并行 subagent，而是**每个维度一个 hat 的串行链**——事件链清晰、纯事件驱动、绕开 subagent 后端依赖，代价是 hat 数增多（用户已接受）。

---

## Problem Frame

`ce-executor-serial` 对「中小计划一把跑完」过重：状态机复杂、per-unit + fix-unit 二阶段门控是历史 flake 高发区。用户要精简线性版本，且明确：executor 必须 TDD + 全量测试全绿才交棒；评审要 6 个维度、每维一个 hat、一环扣一环串行，最后 synthesizer 汇总出修复计划（见 origin）。评审改成串行维度 hat 链后，**不再需要** hat 内 spawn subagent（纯事件驱动），也就绕开了 subagent 后端可行性问题。

---

## Requirements Trace

- R1. 12 功能 hat（`plan-reviewer`/`executor`/6×`dimension-reviewer`/`review-synthesizer`/`fixer`/`alignment`/`reporter`）+ 1 fallback（`progress-steward`）。→ U1
- R2. 事件拓扑严格单链路：每业务事件恰好一个消费者，6 维 hat 串行一环扣一环，无多消费者/分支/回环。→ U1、U2
- R3. `execution_mode: isolated`（4+ hats 硬规则），通过 `check_multi_hat_isolation`。→ U1
- R4. `plan-reviewer` 读 `-p` 计划文件，先评审再就地修复计划文档，产出定稿计划交 executor。→ U3
- R5. `plan-reviewer` 不拆 unit、不写代码，只处理计划文档。→ U3
- R6. `executor` 整份执行 + TDD（先写/更新测试再实现），不做 per-unit 拆分。→ U4
- R7. `executor` DoD = 全量测试套件全绿；不绿不得交棒；不单独起 validator hat。→ U4
- R8. 6 个 `dimension-reviewer` hat 串行链，每维一 hat：审单维、写产物文件、emit 一个事件触发下一维（isolated 单 emit）；testing/correctness 维度实际跑测试复核。→ U5
- R9. `review-synthesizer` 读全部 6 维产物 → 去重/合并/定级(P0-P3) → 写 `fix_plan_file` → emit 恰好一个 `review.complete`；空发现也产空计划。→ U6
- R10. `fixer` 按修复计划全部修复；空计划直通并显式确认「无需修复」。→ U7
- R11. `alignment` 交叉核对定稿计划与修复计划的实际执行度（对照 git diff / 证据）。→ U8
- R12. 对齐未落地项一律记残留，不回环/不重试/不阻断，继续交 reporter。→ U8
- R13. `reporter` 汇总计划改动/执行/各维度评审/修复/对齐残留，产报告并收尾 `LOOP_COMPLETE`。→ U9

**Origin actors:** A1 plan-reviewer, A2 executor, A3 dimension-reviewer(×6), A4 review-synthesizer, A5 fixer, A6 alignment, A7 reporter, A8 progress-steward（见 U1、U3-U9）。
**Origin flows:** F1 线性一条龙主流程（见 High-Level Technical Design 事件拓扑）。

---

## Scope Boundaries

- 不做 per-unit 迭代拆分（`tasks.enabled: false`，整份执行）。
- 不单独起 validator hat（全绿测试门槛内建在 executor TDD DoD）。
- 不做对齐回环 / 复杂重试 / fix→re-review 循环。**不需要** `fix_round` dedup。
- 评审**不用并行 subagent**、也**不用 coordinator 循环**；用 6 维扁平串行 hat 链。
- 不做多消费者 topic / 并行 hat / wave。
- 不改动或替换 `ce-executor-serial`。

### Deferred to Follow-Up Work

- `presets/zh/ce-executor-pipeline-zh.yml` 中文变体（非必需，`build.rs` 不 embed zh）。

---

## Context & Research

### Relevant Code and Patterns

- `presets/en/merge-loop.yml` — **骨架范式**：isolated、`tasks.enabled:false`、多 hat、单 `kind:sequence` 的 `mechanism.flow`、inline `event_policy`。本 preset 以它为结构骨架。
- `presets/en/ce-executor-serial.yml` — **维度评审模板来源**：`dimension-reviewer`（第 ~1911 行，含 `disallowed_tools:["Edit"]` 只读、`timeout`、`missing_event_grace_secs`、`instructions_inline_append`）+ `review-synthesizer`（第 ~2221 行，读各维产物→写 fix_plan_file）；executor TDD（~1168）、reporter（~2740）、progress-steward（~2931）也是模板。**注意**：ce-executor-serial 用 coordinator↔dimension 循环；本 preset 改成扁平直链（每维直接触发下一维），不搬 coordinator。
- hat 字段形态（map 键即 hat_id）：`name`/`description`/`triggers`(唤醒/订阅)/`publishes`(emit 白名单)/`exempt_topics`/`terminal_events`(emit 即结束轮)/`event_filter.events`(prompt 可见性)/`obligations`/`instructions`(块标量) + 可选 `disallowed_tools`/`timeout`/`missing_event_grace_secs`/`default_publishes`。
- `crates/ralph-cli/build.rs` — 编译期拷 `presets/en/<name>.yml` 进 `$OUT_DIR`；有 `presets/schemas/<name>.yml` 才 merge。本 preset **走 inline schemas**（无 SSOT 文件，同 merge-loop）→ 少同步点、免 byte-equality 测试。
- `crates/ralph-cli/src/presets.rs` — `PRESETS` 数组 + 计数/镜像测试（详见 U1、U10）。
- `crates/ralph-core/src/preset_lint/` — lint 家族：`multi_hat`(≥4→isolated)、`ambiguous_routing`(单消费者)、`schema_parity`(每 emit 有 schema)、`workflow_activation`(WAC 可达/egress)、`ownership`/`topic_format`/`flow_declaration`。
- `crates/ralph-core/tests/scenarios.rs` — BDD：**必须用 `run_workflow_guard_scenario`（真 EventLoop，断言 `expected.events`/`absent_events`/`completion`），禁用 `run_scenario` stub**。模板 YAML：`crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml`。

### Institutional Learnings（`docs/solutions/`）

- `logic-errors/base-runtime-must-not-parse-business-markdown.md` — **Rust 不解析计划 markdown**；LLM 理解计划/步骤，Rust 只校验事件 schema + 驱动状态机。本 preset 天然满足（`tasks.enabled:false`）。
- `integration-issues/ce-executor-isolated-preset-dispatch-gap-*.md` — **(1) dead-end trigger**：每个被 `triggers` 的 topic 必须有上游 hat `publishes`，否则卡死→`loop.cancel`；6 维链每个 done 事件都要有唯一上游+下游。**(2) isolated 每轮只留第一个业务事件**：每 hat 每轮**只 emit 一个**，终态前不得有其他 emit。
- `integration-issues/ce-executor-serial-mechanism-close-loop-*.md` — 终态/verdict：pass/fail 放**唯一**字段，`pass_with_residuals ≡ pass`；别让下游同时订阅一对姊妹终态。
- `developer-experience/wac-rollout-tiered-gates-*.md` — **WAC egress 闭包 BFS `EGRESS_MAX_HOPS=4`**；本 preset 是 ~12 跳扁平长链，**几乎必然触发 `activation_egress_missing`**。ce-executor-serial 靠 coordinator 枢纽压短跳数才过；扁平直链没有枢纽 → 处置见 Risks（大概率 `topology_exempt` 白名单）。
- `developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md` — 测试只用 nextest；`ralph-cli` 走 cli-serial。
- `2026-06-16-isolated-wave-stability-and-progress-steward.md` — `loop.stalled`→steward：emit 恰好一个恢复事件后退出、自我防重入、N 次后升级 `plan.blocked(reason=loop_stalled_max_iterations)`；`loop.stalled`/`task.resume` 是 runner 注入 system-control topic，需进内部 topic 白名单避免 lint 报无发布者。`human.guidance` 已删除，**不得**接入。

### External References

无。Ralph 自有内部工具，范式本地可循（merge-loop / ce-executor-serial）。

---

## Key Technical Decisions

- **6 维评审 = 6 个 dimension-reviewer hat 的扁平串行链 + 1 synthesizer**（用户 2026-07-02 修正）。每维 hat 触发上一维的 done 事件、审单维、写产物、emit 下一维事件；末维触发 synthesizer 汇总。**不搬** coordinator 循环、**不用** subagent。理由：单链路事件清晰、纯事件驱动、无后端依赖。
- **以 merge-loop 为结构骨架**（isolated + `tasks.enabled:false` + 单 sequence flow），dimension/synthesizer 的 instructions 从 ce-executor-serial 改编（去掉 coordinator 循环语义）。
- **inline schemas，不建 `presets/schemas/` SSOT 文件**：少同步点、免 byte-equality 测试（同 merge-loop）。
- **无 verdict_gate 硬失败门**：一条龙**总是**产报告并 `LOOP_COMPLETE`；成功/受阻编码在单一 `verdict` 字段（`pass`/`pass_with_residuals`/`blocked`）。失败路径（plan 不可用 / executor 无法全绿 / steward 升级）统一路由 reporter。
- **无 `fix_round`/re-review 循环**：对齐只记残留不回环。
- **单一完成路径**：所有终止经 reporter → `report.done`(`required_events`) → `LOOP_COMPLETE`(`completion_promise`)。
- **每维 hat 只读**（`disallowed_tools:["Edit"]`，仿 ce-executor-serial dimension-reviewer）：评审阶段不改代码，改代码留给 fixer。

---

## Open Questions

### Resolved During Planning

- 评审用并行 subagent 还是串行维度 hat？→ **串行 6 维 hat 链**（用户改定），不用 subagent。
- 用哪几维？→ **全 6 维**（goal-alignment/correctness/testing/maintainability/project-standards/adversarial）。
- 扁平直链还是 coordinator 循环？→ **扁平直链**（每维直接触发下一维）。
- unit ledger？→ 不要，`tasks.enabled:false`。
- schema SSOT 还是 inline？→ inline。

### Deferred to Implementation

- **WAC egress 闭包对 ~12 跳长链的实际判定与处置**（`topology_exempt` 白名单 vs 缩链 vs BFS 实际计跳）→ U1 骨架跑 lint 时定；预判需 `topology_exempt`。
- 6 维 done 事件的确切 topic 名与 topic-format 合规（是否需 `topic_format_whitelist`）；维度产物文件落盘路径（遵守 `ephemeral_isolation`）→ U1/U5。
- `workflow_contract.handoff_topic_seeds` 是否必填/是否需为编译期 const 超集 → U1 lint 暴露后定。
- executor 跨语言测试入口发现（本仓库固定 `cargo nextest`）→ U4。
- 维度 hat 是否需读上一维产物做上下文（默认各维独立评审、synthesizer 汇总）→ U5。

---

## Output Structure

    presets/
      en/ce-executor-pipeline.yml          # 新增：preset 本体（inline schemas，13 hats）
    presets/manifest.yml                   # 改：embedded: + ce-executor-pipeline
    presets/index.json                     # 改：用户可见条目
    crates/ralph-cli/src/presets.rs        # 改：PRESETS + 计数/镜像测试
    crates/ralph-core/tests/
      scenarios/ce_executor_pipeline.yml           # 新增：BDD happy-path（含 6 维事件）
      scenarios/ce_executor_pipeline_blocked.yml   # 新增：BDD 失败/受阻变体
      scenarios.rs                                 # 改：新增 guard 测试函数
    scripts/ralph-zsh-plugin.zsh           # 改：builtin:<TAB> 补全两并行数组
    .cursor/rules/multi-hat-isolation.mdc  # 改：builtin preset 列表
    AGENTS.md / CLAUDE.md                  # 改：Presets & Hats 段列表（cp 同步）

> 说明：**不**新增 `presets/schemas/ce-executor-pipeline.yml`（走 inline schemas）。

---

## High-Level Technical Design

> *方向性设计，供评审校准，非实现规范。*

### 事件拓扑（单消费者、单链路、6 维一环扣一环）

```
work.start ─▶ plan-reviewer ─plan.ready─▶ executor ─work.done─▶ dim:goal-alignment
     │                                        │                        │
 (plan.blocked)                          (work.failed)          review.goalalign.done
     │                                        │                        ▼
     ▼                                        ▼                 dim:correctness ─review.correctness.done─▶
  reporter ◀───────────────────────────────┘                 dim:testing ─review.testing.done─▶
     ▲                                                          dim:maintainability ─review.maintainability.done─▶
     │                                                          dim:project-standards ─review.standards.done─▶
     │                                                          dim:adversarial ─review.adversarial.done─▶
     │                                                          review-synthesizer ─review.complete─▶ fixer
     │                                                                                                   │
     │                                                                                               fix.done
     │                                                                                                   ▼
     └───────────────── align.done ◀── alignment ◀──────────────────────────────────────────────────────┘

report.done (required_events) ─▶ LOOP_COMPLETE (completion_promise)
loop.stalled ─▶ progress-steward ─(task.resume 兜底 / plan.blocked 升级)─▶ reporter
```

### 生产者 → 消费者表

| 事件 | 生产者 | 唯一消费者 | 关键 payload |
|---|---|---|---|
| `work.start` | runtime | plan-reviewer | — |
| `plan.ready` | plan-reviewer | executor | plan_name, plan_path, plan_revised, review_summary |
| `plan.blocked` | plan-reviewer, progress-steward | reporter | reason(enum) |
| `work.done` | executor | dim:goal-alignment | plan_name, plan_path, tests_run, tests_passed, changed_lines, commit_count |
| `work.failed` | executor | reporter | plan_name, reason |
| `review.goalalign.done` | dim:goal-alignment | dim:correctness | plan_name, dimension, findings_file, findings_count |
| `review.correctness.done` | dim:correctness | dim:testing | 同上 |
| `review.testing.done` | dim:testing | dim:maintainability | 同上 |
| `review.maintainability.done` | dim:maintainability | dim:project-standards | 同上 |
| `review.standards.done` | dim:project-standards | dim:adversarial | 同上 |
| `review.adversarial.done` | dim:adversarial | review-synthesizer | 同上 |
| `review.complete` | review-synthesizer | fixer | plan_name, fix_plan_file, findings_count, p0_count, p1_count, findings_summary, verdict |
| `fix.done` | fixer | alignment | plan_name, fix_plan_file, fixes_applied, fixes_skipped |
| `align.done` | alignment | reporter | plan_name, plan_executed, fix_plan_executed, residuals_count, residuals_summary |
| `report.done` | reporter | —（required_events） | report_path, verdict |
| `LOOP_COMPLETE` | reporter | —（completion_promise） | reason |
| `loop.stalled` | runtime | progress-steward | reason |
| `task.resume` | progress-steward, runtime | 目标 hat（control） | reason, target_hat, kind |

> 单消费者：每业务事件只在一个 hat 的 `triggers`；`topic_deny_rules` 对非 owner 全 deny 双保险。6 个 `review.*.done` 事件名待 U1 定稿并过 topic_format lint（必要时进 `topic_format_whitelist`）。

### 维度评审串行链（方向性伪代码）

```
dim_hat[i] on <prev_done_event>:
  read diff + plan + (可选)prior dimension products
  review ONLY dimension[i]         # 只审自己那一维
  if dimension in {correctness, testing}: run test suite (复核正确性)
  write findings product file      # .ralph/review/<loop>/<dimension>.md（路径 U5 定）
  emit EXACTLY ONE <this_done_event>   # 触发下一维；末维触发 synthesizer

review-synthesizer on review.adversarial.done:
  read all 6 dimension product files
  dedup + merge + cross-dimension consistency + rank P0-P3
  write fix_plan_file
  emit EXACTLY ONE review.complete{ fix_plan_file, counts, verdict }
```

### Preset 骨架（方向性 YAML 草图，非最终实现）

```yaml
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: pipeline
        kind: sequence
        allowed_emits: [plan.ready, plan.blocked, work.done, work.failed,
                        review.goalalign.done, review.correctness.done, review.testing.done,
                        review.maintainability.done, review.standards.done, review.adversarial.done,
                        review.complete, fix.done, align.done, report.done, LOOP_COMPLETE]
  repair_budget: 3
  enforce_schema: hard
  state_idempotency: required

tasks: { enabled: false }          # 整份执行，无 unit ledger

event_loop:
  execution_mode: isolated
  prompt_file: "PROMPT.md"
  cli: { backend: "claude" }
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
  starting_event: "work.start"
  max_iterations: 40
  enforce_hat_scope: true
  ephemeral_isolation: true
  event_policy:
    enabled: true
    mode: enforce
    terminal_topics: ["LOOP_COMPLETE"]
    business_topics: [plan.ready, plan.blocked, work.done, work.failed,
                      review.goalalign.done, review.correctness.done, review.testing.done,
                      review.maintainability.done, review.standards.done, review.adversarial.done,
                      review.complete, fix.done, align.done, report.done]
    topic_deny_rules: [ ...非 owner 全 deny... ]
    schemas: { ...每个业务/终态 topic 的 required_fields... }   # inline
# hats: plan-reviewer, executor, dim×6, review-synthesizer, fixer, alignment, reporter, progress-steward
```

---

## Implementation Units

### Phase 1 — 骨架与门禁（最高风险前置：WAC egress）

- [ ] U1. **Preset 骨架 + 注册 + preset_lint 通过（含 WAC egress 处置）**

**Goal:** 用 stub instructions 建起 13-hat 完整接线，注册进 manifest/PRESETS，让 `preset_lint` 全绿——**在写任何 prompt 之前**暴露 WAC egress / isolation / routing / schema 问题；确定 egress 处置。

**Requirements:** R1, R2, R3

**Dependencies:** None

**Files:**
- Create: `presets/en/ce-executor-pipeline.yml`（13 hats stub + 完整 `triggers`/`publishes`/`exempt_topics`/`terminal_events`/`event_filter` + inline `event_policy`(schemas/topic_deny_rules/business_topics/terminal_topics) + `mechanism.flow` 单 sequence + `tasks.enabled:false` + `event_loop` 配置；6 维 hat 串行接线；dimension hat 加 `disallowed_tools:["Edit"]`）
- Modify: `presets/manifest.yml`（`embedded:` 加 `ce-executor-pipeline`）
- Modify: `crates/ralph-cli/src/presets.rs`（`PRESETS` 加条目 `public:true`；计数测试 `test_list_presets_returns_all` 3→4、`test_preset_names_returns_all_names` 长度 + `contains`；若 egress 失败，加入 `topology_exempt` 列表（`test_all_public_presets_pass_authoring_contract`/`test_all_embedded_presets_pass_strict_lint`）并注释理由）
- Test: `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`、`cargo nextest run -p ralph-core -- preset_lint`、`cargo nextest run -p ralph-cli --bin ralph -- presets`

**Approach:**
- 结构照 `presets/en/merge-loop.yml`；6 维 hat 单链路接线（`work.done`→goal-alignment→…→adversarial→synthesizer）。
- **WAC egress 是本单元核心风险**：~12 跳 > `EGRESS_MAX_HOPS=4`，预判 `activation_egress_missing`。先核实 BFS 实际计跳；若确认失败，加 `topology_exempt`（Rust `presets.rs` + `scripts/validate-builtin-presets.sh` 两处镜像）并写清「本 preset 是有意的扁平长链、确定能终止于 LOOP_COMPLETE」理由。
- 需 `cargo build` 先跑（`build.rs` 生成 `$OUT_DIR/presets/*.yml`）再 `include_str!` 生效。

**Patterns to follow:** `presets/en/merge-loop.yml`（骨架）、`presets/en/ce-executor-serial.yml`（hat 字段）。

**Test scenarios:**
- Happy path: `ralph preset check ce-executor-pipeline` / lint 测试无 error。
- Edge: 漏某 topic schema → `schema_parity` 报错；补回通过。
- Edge: 某 topic 进两个 hat 的 `triggers` → `ambiguous_routing` 报错；改回单消费者通过。
- Error: ≥4 hats 漏 `isolated` → `check_multi_hat_isolation` 固定错误串。
- WAC: 记录 egress 判定结果与所选处置（exempt/其它）。

**Verification:** `preset_lint`（两包）与 `presets` 计数测试全绿；`ralph preset list` 含 `ce-executor-pipeline`；egress 处置有据可查。

---

- [ ] U2. **BDD 路由场景（`run_workflow_guard_scenario`，含 6 维链）**

**Goal:** 真 EventLoop 断言完整单链路（含 6 个维度事件）与完成，锁死拓扑。

**Requirements:** R2

**Dependencies:** U1

**Files:**
- Create: `crates/ralph-core/tests/scenarios/ce_executor_pipeline.yml`（每 hat `subscribes_to`/`publishes` + inline `event_loop` + `mock_responses`(按 `hat:` tag) + `expected:{iterations, events[], absent_events[], completion:true}`）
- Create: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_blocked.yml`（失败变体：`work.failed`/`plan.blocked`→reporter，断言 6 维事件 + review.complete/fix.done/align.done 全 absent）
- Modify: `crates/ralph-core/tests/scenarios.rs`（两个新测试函数，均 `run_workflow_guard_scenario`）
- Test: `cargo nextest run -p ralph-core --test scenarios -- ce_executor_pipeline`

**Approach:** 以 `ce_executor_serial_review.yml` 为形态模板。happy 有序断言：`plan.ready → work.done → review.goalalign.done → review.correctness.done → review.testing.done → review.maintainability.done → review.standards.done → review.adversarial.done → review.complete → fix.done → align.done → report.done → LOOP_COMPLETE`。

**Patterns to follow:** `ce_executor_serial_review.yml` + `..._silent_reviewer_recovers.yml`。

**Test scenarios:**
- Happy path: 全 13 事件有序、`completion:true`、iterations 匹配。
- Error/blocked: `work.failed` 直达 reporter；6 维事件与 review.complete/fix.done/align.done 全 absent；仍 `completion:true`。
- Integration: 断言 mock 每 hat emit 经真 EventLoop 路由到正确下游（stub 无法证明）。

**Verification:** 两场景通过；故意改错一条维度接线能让场景失败。

---

### Phase 2 — Hat 行为（填 instructions；由 --mock/实跑 + U2 路由验证）

- [ ] U3. **plan-reviewer instructions（评审 + 修复计划文档）**

**Goal:** 读计划 → 评审 → 就地修复计划文档 → `plan.ready`；不可用 → `plan.blocked`。

**Requirements:** R4, R5 **Dependencies:** U1

**Files:** Modify `presets/en/ce-executor-pipeline.yml`（`plan-reviewer.instructions`）

**Approach:** 只处理计划文档（不拆 unit、不写代码）。缺失/空/占位 → `plan.blocked{reason}`（enum：`plan_file_not_found`/`plan_unusable`）。isolated 单 emit。

**Patterns to follow:** ce-executor-serial coordinator 的 plan 校验/摘要段（去掉 unit 创建）。

**Test scenarios:** Test expectation: 经 U2 mock 路由 + live smoke。
- Happy(smoke): 粗糙计划 → 计划被改进 + `plan.ready`。
- Error(smoke): 计划文件不存在 → `plan.blocked{reason=plan_file_not_found}` → reporter 受阻报告。

**Verification:** --mock 下产 `plan.ready`，计划文档确有修订。

---

- [ ] U4. **executor instructions（TDD + 全量测试全绿硬门槛）**

**Goal:** 整份执行、TDD、全绿才 `work.done`；不绿 `work.failed`。

**Requirements:** R6, R7 **Dependencies:** U1

**Files:** Modify `presets/en/ce-executor-pipeline.yml`（`executor.instructions`）

**Approach:** 不做 per-unit 拆分。DoD：build + 全部测试通过（本仓库固定 `cargo nextest`，遵守 AGENTS.md nextest 硬规则；跨语言启发式 + 注释）。`work.done` payload 带 tests_run/tests_passed/changed_lines/commit_count 证据。不绿 → `work.failed{reason}` → reporter。

**Execution note:** 测试先行（TDD）写进 instructions。

**Patterns to follow:** ce-executor-serial executor（~1168）TDD 段；测试命令遵守 AGENTS.md HARD RULE 1/2。

**Test scenarios:** Test expectation: 经 U2 mock 路由 + live smoke。
- Happy(smoke): 先出现测试 → 实现 → 全绿 → `work.done`(tests_passed>0)。
- Error(smoke): 测试无法通过 → `work.failed` 而非 `work.done`。

**Verification:** `work.done` 仅在测试全绿时出现；证据字段非空。

---

- [ ] U5. **dimension-reviewer instructions ×6（串行维度链）**

**Goal:** 6 个维度 hat，每个触发上一维 done、审单维、写产物文件、emit 一个 done 触发下一维（末维触发 synthesizer）。

**Requirements:** R8 **Dependencies:** U1

**Files:** Modify `presets/en/ce-executor-pipeline.yml`（6 个 `dim:*` hat 的 `instructions`）

**Approach:**
- 6 hat 共享同一模板，仅「本维度焦点」不同：goal-alignment（是否达成计划目标）、correctness（逻辑/边界/错误传播）、testing（覆盖/断言强度）、maintainability（耦合/复杂度/命名/死码）、project-standards（AGENTS.md/CLAUDE.md 合规）、adversarial（对抗性/攻击面）。
- 每维**只读**（`disallowed_tools:["Edit"]`），审自己那一维，写产物到 loop 作用域评审文件（如 `.ralph/review/<loop>/<dimension>.md`，路径 U5 定，遵守 `ephemeral_isolation`）。
- correctness/testing 维度**实际运行测试**复核正确性（R8）。
- **isolated 单 emit**：每维一轮只 emit 自己那一个 done 事件；末维（adversarial）emit `review.adversarial.done` 触发 synthesizer。
- 借鉴 ce-executor-serial dimension-reviewer 的单维聚焦 instructions（去掉 coordinator 循环/`review.dimension.ready` 语义，改成直接触发下一维的固定事件）。
- 各维默认独立评审；是否读上一维产物做上下文留 U5 决定（默认不读，synthesizer 统一汇总）。

**Patterns to follow:** ce-executor-serial `dimension-reviewer`（~1911）+ `instructions_inline_append`/`disallowed_tools`/`timeout`/`missing_event_grace_secs`。

**Test scenarios:** Test expectation: 经 U2 mock 路由（6 维事件有序）+ live smoke。
- Happy(smoke): 每维产出各自产物文件 + emit 对应 done；链走到 synthesizer。
- Edge(smoke): 某维无发现 → 仍写「无发现」产物 + emit done（不中断链）。
- Integration: 确认每维一轮只 emit 一个事件（isolated 只留第一个）。

**Verification:** --mock/实跑下 6 维产物文件齐、事件链完整到 `review.adversarial.done`。

---

- [ ] U6. **review-synthesizer instructions（汇总 6 维 → 出修复计划）**

**Goal:** 读全部 6 维产物 → 去重/合并/跨维一致性/定级 P0-P3 → 写 `fix_plan_file` → emit 恰好一个 `review.complete`。

**Requirements:** R9 **Dependencies:** U1, U5

**Files:** Modify `presets/en/ce-executor-pipeline.yml`（`review-synthesizer.instructions`）

**Approach:** 读 6 个维度产物文件，合并去重、冲突取严、按 P0-P3 定级（含证据+建议修法），写 `fix_plan_file`。空发现也产「无需修复」空计划并照常 emit。isolated 单 emit。

**Patterns to follow:** ce-executor-serial `review-synthesizer`（~2221，读各维 findings→写 fix_plan_file→emit review.complete）。

**Test scenarios:** Test expectation: 经 U2 mock 路由 + live smoke。
- Happy(smoke): 多维有发现 → `fix_plan_file` 含 P0/P1 + 证据 → 单个 `review.complete`。
- Edge(smoke): 全维无发现 → 空 `fix_plan_file`(verdict=pass) → 仍单个 `review.complete`。

**Verification:** 每次汇总恰好一个 `review.complete`；`fix_plan_file` 落盘、结构含 P0-P3+证据。

---

- [ ] U7. **fixer instructions（按修复计划全部修复）**

**Goal:** 读 `fix_plan_file` → 全部修复 → `fix.done`；空计划直通。

**Requirements:** R10 **Dependencies:** U1

**Files:** Modify `presets/en/ce-executor-pipeline.yml`（`fixer.instructions`）

**Approach:** 依 P0-P3 全量修复（P0/P1 必修）。`fix.done` 带 fixes_applied/fixes_skipped。空计划 → `fix.done{fixes_applied:0}` 直通、注明无需修复。不回环、不 re-review。

**Patterns to follow:** ce-executor-serial fixer（~2483）读 fix 计划 + 应用段（去掉 fix-unit/fix_round 循环）。

**Test scenarios:** Test expectation: 经 U2 mock 路由 + live smoke。
- Happy(smoke): 非空计划 → `fix.done{fixes_applied>0}`。
- Edge(smoke): 空计划 → `fix.done{fixes_applied:0}` 直通。

**Verification:** `fix.done` 总出现（有修或直通）。

---

- [ ] U8. **alignment instructions（对齐关：记残留、不回环）**

**Goal:** 交叉核对定稿计划与 `fix_plan_file` 的实际执行度，未落地记残留，emit `align.done`。

**Requirements:** R11, R12 **Dependencies:** U1

**Files:** Modify `presets/en/ce-executor-pipeline.yml`（`alignment.instructions`）

**Approach:** 逐条核对 (a) 定稿计划是否执行、(b) 修复计划是否执行，依据实际代码改动/测试证据。未落地写入 `align.done` 的 residuals_summary/residuals_count。**绝不**回退/重试/阻断（R12）。只读核对（`disallowed_tools:["Edit"]` 可选）。

**Patterns to follow:** 无直接模板；参考 reporter/shipper 读进度证据方式做只读核对。

**Test scenarios:** Test expectation: 经 U2 mock 路由 + live smoke。
- Happy(smoke): 全执行 → `align.done{residuals_count:0}`。
- Edge(smoke): 部分未落地 → `align.done{residuals_count>0}`，且不产任何回环事件。

**Verification:** `align.done` 总出现、从不产生回退/重试事件。

---

- [ ] U9. **reporter + progress-steward instructions**

**Goal:** reporter 汇总产报告、emit `report.done`+`LOOP_COMPLETE`，兜底消费 `plan.blocked`/`work.failed`；steward 在 `loop.stalled` 时 emit 一个恢复事件、N 次后升级 `plan.blocked`。

**Requirements:** R13 **Dependencies:** U1

**Files:** Modify `presets/en/ce-executor-pipeline.yml`（`reporter.instructions`、`progress-steward.instructions`）

**Approach:**
- reporter `triggers:[align.done, work.failed, plan.blocked]`（唯一消费者）；汇总计划改动/执行/6 维评审/修复/对齐残留 → 写报告 → `report.done{report_path, verdict}` → `LOOP_COMPLETE{reason}`。verdict 单字段 `pass`/`pass_with_residuals`/`blocked`。**镜像 ce-executor-serial reporter 的 report.done(required_events)→LOOP_COMPLETE(completion_promise) 握手**，遵守 isolated 单 emit。
- steward `triggers:[loop.stalled]`；emit 恰好一个恢复事件（`task.resume` 兜底）；自我防重入；N 次 → `plan.blocked{reason=loop_stalled_max_iterations}`。`loop.stalled`/`task.resume` 登记 runner 内部 topic 白名单。**不接** `human.guidance`。

**Patterns to follow:** ce-executor-serial reporter（~2740）+ progress-steward（~2931）。

**Test scenarios:** Test expectation: 经 U2（含 blocked 变体）+ live smoke。
- Happy: `align.done` → `report.done`+`LOOP_COMPLETE`，报告含五段摘要（含 6 维发现）。
- Error: `work.failed`/`plan.blocked` → 受阻报告 + `LOOP_COMPLETE`(verdict=blocked)。
- Edge: `loop.stalled` → steward 一个恢复事件；连续 N 次 → `plan.blocked`。

**Verification:** U2 两场景通过；--mock 实跑走到 `LOOP_COMPLETE` 并落盘报告。

---

### Phase 3 — 下游同步与全量校验

- [ ] U10. **用户可见注册 + 计数/镜像测试**

**Goal:** 接入用户可见入口并让所有 count/mirror 测试通过。

**Requirements:** R1 **Dependencies:** U1

**Files:**
- Modify: `presets/index.json`（`{name, description, category:"development"}`）
- Modify: `scripts/ralph-zsh-plugin.zsh`（`_RALPH_BUILTIN_HAT_VALUES` + `_RALPH_BUILTIN_HAT_DESCRIPTIONS` 各加一行，等长同序、`compadd` 风格）
- Modify: `crates/ralph-cli/src/presets.rs`（`test_index_json_entries_have_zsh_completion` 硬编码集合加 `"builtin:ce-executor-pipeline"`）
- Test: `cargo nextest run -p ralph-cli --bin ralph -- presets`（含 `test_public_preset_names_in_index_json`、`test_zsh_builtin_completion_arrays_consistent`、`test_index_json_entries_have_zsh_completion`）
- 安装: `cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`

**Approach:** `public:true` 必须进 index.json + zsh；两并行数组等长同序（强校验）。

**Patterns to follow:** index.json/zsh 现有 `ce-executor-serial` 条目。

**Test scenarios:**
- Happy: `presets` 组全绿。
- Edge: zsh 两数组不等长 → `test_zsh_builtin_completion_arrays_consistent` 失败；修正通过。
- Integration: `ralph run -H builtin:<TAB>` 出现 `ce-executor-pipeline`（手工 zsh 验证）。

**Verification:** `presets` 组全绿；zsh 补全实测可见。

---

- [ ] U11. **文档同步 + drift 扫描 + 全量基线**

**Goal:** 同步文档 builtin 列表，确认无 skill-guide 漂移，跑全量基线。

**Requirements:** R1-R13（收口验证） **Dependencies:** U1-U10

**Files:**
- Modify: `AGENTS.md` + `CLAUDE.md`（Presets & Hats 段加 `ce-executor-pipeline` + 一句描述；`cp CLAUDE.md AGENTS.md` 字节一致）
- Modify: `.cursor/rules/multi-hat-isolation.mdc`（builtin preset 列表）
- 校验: `crates/ralph-core/data/ralph-tools*.md` 是否需改（预期不需要：复用既有 runtime/命令，只加新拓扑与 preset-local 事件；用 `scripts/check-cli-doc-drift.sh` 确认）
- Test: `./scripts/run-tests.sh`、`./scripts/check-cli-doc-drift.sh`、`./scripts/validate-builtin-presets.sh`

**Approach:** 文档两文件字节一致（HARD RULE）。skill-guide 仅当引入 agent 面向的新命令/事件/配置/输出格式才需改——本 preset 不新增 CLI 命令，预期免改，但跑 drift 扫描确认。

**Test scenarios:**
- Happy: `./scripts/run-tests.sh` 全绿（preset_lint + WAC + scenarios + presets 计数）。
- Edge: `check-cli-doc-drift.sh` 无新增漂移。
- Integration(可选): `ralph run -H builtin:ce-executor-pipeline -p <小计划> --mock` 端到端走到 `LOOP_COMPLETE` 并出报告。

**Verification:** 全量基线绿；`diff CLAUDE.md AGENTS.md` 空；drift 干净。

---

## System-Wide Impact

- **Interaction graph:** 纯新增 preset，不改共享 runtime 源码。触点全在 preset 注册面（manifest/PRESETS/index.json/zsh/文档）与 BDD 测试面。`event_loop`/`preset_lint`/`build.rs` 按既有机制自动适配。
- **Error propagation:** 所有失败（plan 不可用 / executor 无法全绿 / loop 卡死升级）统一路由 reporter → `report.done`+`LOOP_COMPLETE`，verdict 单字段区分。无 dead-end trigger（拓扑表保证每个被 trigger 的 topic 都有上游 producer）。
- **State lifecycle risks:** `tasks.enabled:false` → 无 unit ledger、不写 `.ralph/agent/tasks.jsonl`，规避 fix-unit/state-projection 系列坑。isolated 单 emit 是主要不变量，各 hat instructions 必须遵守；6 维产物文件需落 loop 作用域、遵守 `ephemeral_isolation`。
- **API surface parity:** 新 builtin preset 名进入 CLI 补全/索引；无破坏性接口变更。
- **Integration coverage:** U2 的 `run_workflow_guard_scenario`（happy 含 6 维事件 + blocked）是拓扑正确性的唯一自动化证明；hat prompt 行为靠 live --mock/实跑覆盖。
- **Unchanged invariants:** `ce-executor-serial` 及其 schema SSOT、其它 builtin preset 完全不动；`build.rs` 合并机制、`preset_lint` 家族不改（仅可能往 `topology_exempt` 列表加一名）。

---

## Risks & Dependencies

| Risk | Mitigation |
|---|---|
| **WAC egress 闭包（`EGRESS_MAX_HOPS=4`）对 ~12 跳扁平长链报 `activation_egress_missing`（本 preset 最大风险）** | U1 骨架先跑 lint 早暴露；核实 BFS 计跳后大概率加 `topology_exempt`（`presets.rs` + `validate-builtin-presets.sh` 两处镜像 + 理由注释：有意的扁平长链、确定终止于 LOOP_COMPLETE）。若不可豁免，退路是引入一个 review 枢纽 hat 压短跳数（偏离「扁平直链」，需回问用户） |
| isolated 每轮只留第一个业务事件，某维/synthesizer/reporter 误 emit 多个致链断/终态丢失 | 各 hat instructions 明确「一轮一 emit」；U2 guard 断言 13 事件计数与顺序 |
| 6 维 done 事件 topic-format 不合规（大小写/下划线） | U1 用纯 lowercase dotted 单 token 命名；必要时进 `topic_format_whitelist` |
| hat 数多（13）致 preset 体积/维护成本上升 | 6 维 hat 共享 instructions 模板（U5），仅焦点段不同；synthesizer 汇总，避免每维重复合成逻辑 |
| dead-end trigger（某 done 事件无上游/下游） | 拓扑表逐一核对；U1 `ambiguous_routing`/WAC 覆盖；U2 guard 实跑证明链通 |
| 计数/镜像测试遗漏（presets.rs len、zsh 双数组、index.json 镜像） | U1/U10 显式列出每个测试；`run-tests.sh` 收口 |
| **依赖:** `cargo build` 先跑让 `build.rs` 生成 `$OUT_DIR/presets/ce-executor-pipeline.yml` | 各 Phase 测试前先 `cargo build`；`run-tests.sh` 已含 |

---

## Alternative Approaches Considered

- **reviewer 单 hat + 并行 subagent（先前方案）**：hat 少、并行快，但依赖 Claude 后端 Task 原语、隐藏 subagent 行为、且 isolated 单 emit 与 subagent 协作需小心。**放弃**：用户改选串行维度 hat 链（事件更清晰、无后端依赖）。
- **coordinator↔dimension 循环（ce-executor-serial 原样）**：跳数短（利于 WAC egress）、但引入 coordinator 状态机循环，偏离「单链路一环扣一环」。**放弃**：用户要扁平直链；WAC egress 用 `topology_exempt` 处置。
- **split-schema SSOT 文件**：authoring 分离清晰，但多同步点 + byte-equality 测试。**放弃**：inline schemas 更简（merge-loop 范式）。
- **`tasks.enabled:true` + per-unit ledger**：有任务台账，但引入 unit/fix-unit 二阶段 + state_projection 复杂度。**放弃**：整份执行符合用户意图。

---

## Documentation / Operational Notes

- AGENTS.md/CLAUDE.md Presets & Hats 段、`.cursor/rules/multi-hat-isolation.mdc` builtin 列表随 U11 同步（两文档字节一致）。
- 用户用法：`ralph run -H builtin:ce-executor-pipeline -p "docs/plans/<plan>.md"`。评审为串行 6 维，无并行/无特殊后端要求。
- 运维：无新增运行时状态文件（`tasks.enabled:false`）；6 维产物文件落 loop 作用域、遵守 `ephemeral_isolation`。

---

## Sources & References

- **Origin document:** docs/brainstorms/2026-07-02-ce-executor-pipeline-preset-requirements.md
- 骨架范式: `presets/en/merge-loop.yml`；hat/instructions 模板: `presets/en/ce-executor-serial.yml`（dimension-reviewer ~1911、review-synthesizer ~2221、executor ~1168、reporter ~2740、progress-steward ~2931、fixer ~2483）
- 注册/合并: `crates/ralph-cli/src/presets.rs`、`crates/ralph-cli/build.rs`、`presets/manifest.yml`、`presets/index.json`
- Lint/测试: `crates/ralph-core/src/preset_lint/`、`crates/ralph-core/tests/scenarios.rs`(+`scenarios/ce_executor_serial_review.yml`)
- 学习: `docs/solutions/logic-errors/base-runtime-must-not-parse-business-markdown.md`、`docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-*.md`、`docs/solutions/developer-experience/wac-rollout-tiered-gates-*.md`、`docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md`
