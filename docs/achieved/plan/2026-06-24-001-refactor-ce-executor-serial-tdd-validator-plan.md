---
title: "refactor: ce-executor-serial preset 重写 — TDD executor + validator hat + 总体 review"
type: refactor
status: active
date: 2026-06-24
---

# refactor: ce-executor-serial preset 重写 — TDD executor + validator hat + 总体 review

## Summary

将 `ce-executor-serial` preset 从 11-hat 架构（含 debug-resolver + plan-gate）重写为 10-hat 架构：新增 `validator` hat 负责跑全量测试，executor 改为 TDD 模式（先测试后实现），fixer 合并 debug-resolver 的诊断能力，coordinator 直接推进 unit 流转（去掉 plan-gate），review 延迟到全部 unit 完成后总体执行一次。目标是消除多消费者路由竞态、降低 per-step review 冗余、让开发计划执行形成可靠闭环。

---

## Problem Frame

当前 `ce-executor-serial` preset 存在三个结构性问题：

1. **路由竞态根因未根治**：虽然 `fix.exhausted` / `debug.exhausted` 已改为单消费者，但 debug-resolver + plan-gate 两个 hat 的存在使路由拓扑仍然复杂，新增 topic 时容易再次引入多消费者竞态。
2. **per-step review 冗余**：每完成一个 unit 就触发完整 2-dimension review，对于 N 个 unit 的计划意味着 N 轮 review，大部分 review 发现的问题在后续 unit 中会自然解决。
3. **fixer 与 debug-resolver 职责割裂**：fixer 负责修代码，debug-resolver 负责诊断根因，但两者实际工作高度重叠（都需要读代码、定位问题），拆成两个 hat 增加了 handoff 开销和状态传递成本。

---

## Requirements

- R1. preset hat 数量从 11 降为 10：删除 `debug-resolver` 和 `plan-gate`，新增 `validator`
- R2. executor 采用 TDD 模式：先写/更新测试，再实现代码，完成时跑单元测试
- R3. `validator` hat 负责跑全量测试（`cargo nextest run`），发布 `test.passed` / `test.failed`
- R4. review 延迟到所有 unit 完成后总体执行一次（非 per-step）
- R5. `fixer` 合并 debug-resolver 的诊断能力：定位 max 10 + 修复 max 10，通过 `fix-log.md` 的 round marker 传递预算
- R6. `coordinator` 直接推进 unit 流转（`test.passed` → 下一个 unit 的 `work.ready`），去掉 plan-gate 中间层
- R7. fixer 预算耗尽后 `fix.exhausted` → executor 重写机会（max 10），executor 重写也耗尽 → `fix.exhausted` → shipper → 终止
- R8. 所有 strict validation（preset_lint / WAC R2-R5 / ambiguous_routing / SSOT byte-equality）必须通过
- R9. payload schema 同步更新（新事件 `test.passed` / `test.failed`，移除 `review.passed` / `review.failed` / `debug.*` / `queue.advance` 等）
- R10. 所有中文输出规则不变（instructions 面向人类的描述用中文）

---

## Scope Boundaries

- 不修改 ralph-core / ralph-cli 的 Rust 运行时逻辑（event_loop、EventBus、HatRegistry 等核心机制不变）
- 不修改 ralph-proto 类型定义
- 不新增 preset_lint 规则（仅调整 `review_terminal_coherence` 适配新架构）
- 不修改 `presets/manifest.yml`（preset 名字不变）
- 不修改其他 builtin preset（autoresearch / debug / merge-loop / ce-executor-lite）
- 不修改 web dashboard / TUI / API 相关代码

### Deferred to Follow-Up Work

- `review_terminal_coherence` 的 KTD-TTC-2 扩展（覆盖 `fix.applied` / `fix.exhausted` 等分支对）：单独 PR
- executor TDD 模式的 `complexity` 分级（trivial 跳过测试先行）：当前硬编码 `large` 行为，分级逻辑后续迭代
- `progress-steward` 去除评估：单消费者路由下 progress-steward 仍有 `loop.stalled` 唤醒价值，保留观察

---

## Context & Research

### Relevant Code and Patterns

- `presets/en/ce-executor-serial.yml` — 当前 11-hat preset（line 432-2400+），重写目标
- `presets/schemas/ce-executor-serial.yml` — SSOT schema 定义，必须与 preset 内联副本字节一致
- `crates/ralph-cli/src/presets.rs` — `PRESETS` 数组 + 22+ 个硬编码断言测试（大部分需删除）
- `crates/ralph-core/src/event_loop/tests/plan_gate_bridge.rs` — plan-gate 专用测试，整个文件删除
- `crates/ralph-core/src/preset_lint/review_terminal_coherence.rs:54` — `REVIEW_PAIR = [("review.passed", "review.complete")]`，新架构只有 `review.complete` 需调整
- `crates/ralph-core/tests/scenarios/step_handoff/` — 5 个 scenario 文件，其中 4 个与 plan-gate 相关需删除
- `crates/ralph-core/tests/scenarios/ce_executor_*.yml` — 5 个 ce_executor scenario，需更新或删除
- `AGENTS.md` / `CLAUDE.md` — builtin preset 列表描述（line 130），需同步
- `.cursor/rules/multi-hat-isolation.mdc` — ce-executor-serial 引用，需同步

### Institutional Learnings

- 多消费者 topic + round-robin 调度 = 竞态饥饿（2026-06-24 `fix.exhausted` 修复已验证）
- preset 测试过度硬编码 hat/topic 断言会导致每次架构调整都要改 20+ 测试，维护成本高于价值
- SSOT byte-equality 检查是防止 schema 漂移的硬门，必须保持

---

## Key Technical Decisions

- **去掉 plan-gate，coordinator 直接推进 unit**：plan-gate 的 `queue.advance` 机制增加了路由复杂度且与 coordinator 的 unit 解析职责重叠。coordinator 收到 `test.passed` 后直接发下一个 unit 的 `work.ready`，全部 unit 完成后发 `review.start`。
- **去掉 debug-resolver，fixer 合并诊断能力**：fixer 在 `fix-log.md` 中记录诊断过程（root cause + fix attempt），不再需要独立的 debug-resolver hat 做根因分析。fixer 预算硬编码在 instructions 中（定位 max 10 + 修复 max 10），通过 round marker 传递。
- **新增 validator hat**：executor 完成后由 validator 跑全量测试（而非 executor 自己跑），职责分离让 executor 专注实现、validator 专注验证。validator 发布 `test.passed` / `test.failed`，fixer 只在 `test.failed` 时触发。
- **review 延迟到总体完成**：所有 unit + fix-unit 完成后，coordinator 发 `review.start`，执行一次完整 2-dimension review（correctness + testing）。review 发现的问题通过 `fix_plan` 交给 coordinator 创建 fix-unit 任务。
- **executor TDD 模式**：executor instructions 要求先写/更新测试再实现，完成时跑单元测试（非全量）。全量测试由 validator 负责。
- **删除 22+ 硬编码断言测试**：只保留 5 个核心测试（preset_validates / ambiguous_routing / SSOT_merge / report_done_completion_gate / root_preset_matches_embedded），降低维护成本。
- **`review_terminal_coherence` REVIEW_PAIR 改空列表**：新架构 review-synthesizer 只发 `review.complete`（不发 review.passed/review.failed 对），REVIEW_PAIR 检查不再适用。

---

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

### 新架构事件流

```
coordinator (解析 plan, 创建 unit 任务)
  → executor (unit-01, TDD: 先测试后实现, 跑单元测试)
  → validator (跑全量测试)
    ├─ test.passed → coordinator → executor (unit-02) → ... → unit-N 全完成
    │                                          ↓
    │                                        coordinator 发 review.start
    │                                        review-coordinator → dimension-reviewer(correctness)
    │                                        → review-coordinator → dimension-reviewer(testing)
    │                                        → review-coordinator → review-synthesizer
    │                                          → review.complete + fix_plan (分 unit 的 P0/P1 修复计划)
    │                                          → coordinator (接管 fix_plan, 创建 fix-unit 任务)
    │                                          → executor (fix-unit-01) → validator
    │                                          → executor (fix-unit-02) → validator → ...
    │                                          → 全部 fix-unit 完成 + validator 通过
    │                                          → coordinator 发 plan.complete
    │                                          → shipper → reporter → [LOOP_COMPLETE]
    │
    └─ test.failed → fixer (直接修实现, 定位 max 10 + 修复 max 10)
                       ├─ fix.applied → validator (重跑测试)
                       │   ├─ test.passed → 回主线
                       │   └─ test.failed → fixer (继续)
                       └─ fix.exhausted → executor (重写机会, max 10) → validator → ...
```

### Hat 列表（10 个）

| hat | triggers | publishes | terminal_events |
|---|---|---|---|
| coordinator | work.start, task.resume, test.passed, review.complete | work.ready, review.start, plan.complete, plan.blocked | work.ready, review.start, plan.complete, plan.blocked |
| executor | work.ready, fix.exhausted | work.done, work.failed | work.done, work.failed |
| validator | work.done, fix.applied | test.passed, test.failed | test.passed, test.failed |
| fixer | test.failed | fix.applied, fix.exhausted | fix.applied, fix.exhausted |
| review-coordinator | review.start, review.dimension.done, review.dimension.failed | review.dimension.ready, review.dimensions.complete | review.dimension.ready, review.dimensions.complete |
| dimension-reviewer | review.dimension.ready | review.dimension.done, review.dimension.failed | review.dimension.done, review.dimension.failed |
| review-synthesizer | review.dimensions.complete | review.complete | review.complete |
| shipper | plan.complete, plan.blocked | REVIEW_COMPLETE | REVIEW_COMPLETE |
| reporter | REVIEW_COMPLETE | report.done, LOOP_COMPLETE | report.done, LOOP_COMPLETE |
| progress-steward | loop.stalled | task.resume, plan.blocked | plan.blocked |

---

## Implementation Units

### U1. 重写 preset 主文件

**Goal:** 将 `presets/en/ce-executor-serial.yml` 从 11-hat 重写为 10-hat 新架构

**Requirements:** R1, R2, R3, R4, R5, R6, R7, R10

**Dependencies:** None（源头文件，其他 unit 依赖此文件）

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`

**Approach:**
- 删除 `debug-resolver` hat（line 1701-1840）和 `plan-gate` hat（line 1841-2013）
- 新增 `validator` hat：triggers `["work.done", "fix.applied"]`，publishes `["test.passed", "test.failed"]`，instructions 要求跑 `cargo nextest run` 全量测试并解析结果
- 更新 `coordinator` triggers：从 `["work.start", "task.resume", "review.passed", "review.complete", "queue.advance", "work.failed", "loop.cancel"]` 改为 `["work.start", "task.resume", "test.passed", "review.complete"]`；instructions 增加 unit 流转逻辑（`test.passed` → 下一个 unit 的 `work.ready`；全部完成 → `review.start`；`review.complete` 带 fix_plan → 创建 fix-unit）
- 更新 `executor` triggers：增加 `fix.exhausted`（重写机会）；instructions 改为 TDD 模式（先写/更新测试，再实现，完成跑单元测试）
- 更新 `fixer` triggers：从 `["review.failed", "fix.retry"]` 改为 `["test.failed"]`；instructions 合并 debug-resolver 诊断能力（root cause 分析 + 修复），预算硬编码（定位 max 10 + 修复 max 10），通过 `fix-log.md` round marker 传递
- 更新 `review-coordinator` triggers：从 `["review.start", "review.dimension.done", "review.dimension.failed", "fix.applied"]` 改为 `["review.start", "review.dimension.done", "review.dimension.failed"]`（去掉 fix.applied 触发，review 只在总体完成后执行一次）
- 更新 `review-synthesizer`：publishes 只保留 `["review.complete"]`（去掉 `review.passed` / `review.failed`）
- 更新 `shipper` triggers：从 `["plan.complete", "plan.blocked", "fix.exhausted", "debug.exhausted"]` 改为 `["plan.complete", "plan.blocked"]`
- 更新 `event_policy.topic_deny_rules`：适配新 hat 列表
- 更新 `execution_contracts`：`work.done` 改为 7 字段 require_payload；新增 `test.passed` / `test.failed` 合约；`fix.applied` 改为 require_git_change commit_only
- 更新 `state_projection.actions_chain`：`work.ready` → `[ensure_task]`；`work.done` → `[close_task, mark_step_completed]`；`plan.complete` → `[plan_complete]`
- 更新 `workflow_contract.handoff_topic_seeds`：新事件列表
- 更新 `progress_steward` 配置：`steward_hat_id` 保持 `progress-steward`
- 更新顶部 `coordinator_hats` 列表和 `review_terminal_coherence_exempt_consumers`
- 所有 instructions 面向人类的描述用中文

**Test scenarios:**
- Test expectation: none -- preset lint 和 validation 由 U4 的 `test_ce_executor_serial_preset_validates` 覆盖

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 通过
- `cargo nextest run -p ralph-core -- preset_lint` 通过
- preset YAML 可被 `RalphConfig::parse_yaml` 解析

---

### U2. 重写 SSOT schema + 更新 manifest 描述

**Goal:** 同步更新 schema 定义文件，保持 SSOT 字节一致

**Requirements:** R8, R9

**Dependencies:** U1（schema 必须与 preset 内联副本一致）

**Files:**
- Modify: `presets/schemas/ce-executor-serial.yml`
- Modify: `presets/index.json`

**Approach:**
- 重写 `presets/schemas/ce-executor-serial.yml` 的 `schemas` 段：新增 `test.passed` / `test.failed` schema；删除 `review.passed` / `review.failed` / `debug.*` / `queue.advance` / `debug.retry` schema；更新 `work.ready` / `work.done` / `fix.applied` / `fix.exhausted` / `review.complete` / `plan.complete` 的 required_fields
- 更新 `workflow_contract.handoff_topic_seeds`：与新事件列表一致
- 更新 `state_projection.actions_chain`：与 preset 一致
- 更新顶部拓扑注释：10-hat 架构描述
- 更新 `presets/index.json` 中 `ce-executor-serial` 的 description（如需）

**Test scenarios:**
- Test expectation: none -- SSOT byte-equality 由 U4 的 `test_ce_executor_root_preset_matches_embedded` 覆盖

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded` 通过
- schema 与 preset 内联副本字节一致

---

### U3. 删除 plan_gate_bridge 测试 + 调整 review_terminal_coherence

**Goal:** 清理 plan-gate 专用测试代码，调整 review terminal coherence lint 适配新架构

**Requirements:** R1, R8

**Dependencies:** U1（plan-gate 已从 preset 删除）

**Files:**
- Delete: `crates/ralph-core/src/event_loop/tests/plan_gate_bridge.rs`
- Modify: `crates/ralph-core/src/preset_lint/review_terminal_coherence.rs`

**Approach:**
- 删除整个 `plan_gate_bridge.rs` 文件（plan-gate hat 不再存在）
- 检查 `crates/ralph-core/src/event_loop/tests/mod.rs` 中对 `plan_gate_bridge` 的 `mod` 声明，删除对应行
- 修改 `review_terminal_coherence.rs:54` 的 `REVIEW_PAIR`：从 `&[("review.passed", "review.complete")]` 改为 `&[]`（空列表）
- 更新文件顶部注释：说明新架构 review-synthesizer 只发 `review.complete`，没有 review.passed/review.failed 对，REVIEW_PAIR 检查不再适用
- 检查 `review_terminal_coherence.rs` 中引用 `REVIEW_PAIR` 的测试函数，更新或删除依赖 review.passed 的测试

**Test scenarios:**
- Happy path: `review_terminal_coherence` lint 对新 preset（只有 review.complete）不报 finding
- Edge case: `mutually_exclusive_terminal_pairs()` 返回空列表时，lint 跳过检查不 panic

**Verification:**
- `cargo nextest run -p ralph-core -- review_terminal_coherence` 通过
- `cargo nextest run -p ralph-core -- preset_lint` 通过
- `cargo build -p ralph-core` 无编译错误（确认 plan_gate_bridge 删除后无悬空引用）

---

### U4. 清理 presets.rs 测试函数

**Goal:** 删除 22+ 个硬编码 hat/topic 断言测试，只保留 5 个核心测试

**Requirements:** R8

**Dependencies:** U1, U2（测试断言依赖新 preset 结构）

**Files:**
- Modify: `crates/ralph-cli/src/presets.rs`

**Approach:**
- 保留以下 5 个测试函数：
  - `test_ce_executor_serial_preset_validates` — preset 能通过 `validate()`
  - `test_ce_executor_serial_preset_validates_ambiguous_routing` — 无歧义路由
  - `test_ce_executor_root_preset_matches_embedded` — en 与 embedded 一致（SSOT byte-equality）
  - `test_ce_executor_serial_has_report_done_completion_gate` — required_events 包含 `report.done`
  - SSOT merge 测试（如 `test_ce_executor_serial_schema_matches_preset_inline`）
- 删除以下 22+ 个测试函数（完整列表见 plan 末尾附录）
- 更新 `PRESETS` 数组中 `ce-executor-serial` 的 `description` 字段（如需）
- 检查 `presets.rs` 中是否有其他引用被删测试函数的代码，清理悬空引用

**Test scenarios:**
- Test expectation: none -- 保留的 5 个测试本身就是验证

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- ce_executor` 通过（只剩 5 个测试）
- `cargo build -p ralph-cli` 无编译错误

---

### U5. 清理 scenario 文件

**Goal:** 删除 plan-gate / debug-resolver 相关 scenario，更新 ce_executor_* scenario

**Requirements:** R1, R8

**Dependencies:** U1（scenario 依赖新 preset 结构）

**Files:**
- Delete: `crates/ralph-core/tests/scenarios/step_handoff/fix_exhausted_reaches_plan_gate.yml`
- Delete: `crates/ralph-core/tests/scenarios/step_handoff/debug_exhausted_reaches_plan_gate.yml`
- Delete: `crates/ralph-core/tests/scenarios/step_handoff/progress_task_mismatch.yml`
- Delete: `crates/ralph-core/tests/scenarios/step_handoff/state_projection_work_done_updates_progress.yml`
- Delete: `crates/ralph-core/tests/scenarios/step_handoff/step_advance_u1_to_u2.yml`
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_*.yml`（5 个文件，更新或删除）
- Modify: `crates/ralph-core/tests/scenarios.rs`

**Approach:**
- 删除 5 个 step_handoff scenario（全部与 plan-gate 相关）
- 检查 `ce_executor_bootstrap_recovery.yml` / `ce_executor_recovery.yml` / `ce_executor_serial_fix_applied_rereview.yml` / `ce_executor_serial_review_silent_reviewer_recovers.yml` / `ce_executor_serial_review.yml`：如果引用了 plan-gate / debug-resolver / review.passed / review.failed / queue.advance 等已删除的事件或 hat，更新为新架构等价路径或删除
- 更新 `scenarios.rs`：删除引用已删 scenario 文件的测试函数（`test_fix_exhausted_reaches_plan_gate` / `test_debug_exhausted_reaches_plan_gate` / `test_e2e_step_handoff_topology_complete` 等）
- 保留与新架构兼容的 scenario 测试

**Test scenarios:**
- Test expectation: none -- scenario 测试本身就是验证

**Verification:**
- `cargo nextest run -p ralph-core --test scenarios` 通过
- `cargo build -p ralph-core --tests` 无编译错误

---

### U6. 同步文档

**Goal:** 同步 AGENTS.md / CLAUDE.md / .cursor/rules 中 ce-executor-serial 的描述

**Requirements:** R10

**Dependencies:** U1（文档描述需反映新架构）

**Files:**
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Modify: `.cursor/rules/multi-hat-isolation.mdc`

**Approach:**
- 更新 `AGENTS.md` line 130 的 builtin preset 列表描述：`ce-executor-serial` 从 "11-hat" 改为 "10-hat（coordinator + executor + validator + fixer + review-coordinator + dimension-reviewer + review-synthesizer + shipper + reporter + progress-steward）"
- `cp AGENTS.md CLAUDE.md` 保持同步
- 检查 `.cursor/rules/multi-hat-isolation.mdc` 中对 ce-executor-serial hat 列表的引用，更新为新 10-hat 架构
- 检查 `scripts/ralph-zsh-plugin.zsh` 是否需要更新（preset 名字不变，应该不需要）

**Test scenarios:**
- Test expectation: none -- 文档同步，无行为变化

**Verification:**
- `diff AGENTS.md CLAUDE.md` 无差异
- `grep -r "debug-resolver\|plan-gate" AGENTS.md CLAUDE.md .cursor/rules/multi-hat-isolation.mdc` 无残留引用

---

## Risks

- **风险 1：executor TDD instructions 过于具体导致 agent 困惑**
  - 缓解：instructions 只描述 "先写/更新测试再实现" 的原则，不规定具体测试框架或命令（由 executor 根据项目 AGENTS.md 自行判断）
- **风险 2：fixer 合并诊断能力后 instructions 过长**
  - 缓解：fixer instructions 分两段（诊断段 + 修复段），每段聚焦核心步骤，不展开具体调试技巧
- **风险 3：coordinator 直接推进 unit 流转可能遗漏 plan-gate 原有的校验逻辑**
  - 缓解：plan-gate 原有校验（如 `review.passed` 检查）在新架构中由 validator 的 `test.passed` 隐式覆盖；coordinator 只做 unit 指针推进，不做内容校验
- **风险 4：删除 22+ 测试后覆盖度下降**
  - 缓解：保留的 5 个核心测试覆盖 preset 可解析性、无歧义路由、SSOT 一致性、completion gate；strict validation（preset_lint / WAC）提供结构性保障
- **风险 5：scenario 删除过多导致 BDD 覆盖不足**
  - 缓解：保留与新架构兼容的 ce_executor_* scenario；step_handoff scenario 的 plan-gate 逻辑已由 preset_lint 结构性检查替代

---

## System-Wide Impact

- **preset 用户**：使用 `ralph run -H builtin:ce-executor-serial` 的用户会感受到执行流程变化（TDD 模式、总体 review、validator 跑全量测试），但 CLI 接口不变
- **preset 开发者**：维护成本降低（22+ 硬编码测试 → 5 个核心测试），新增 hat/topic 时不再需要改 20+ 测试
- **CI**：测试数量减少，CI 时间略降；strict validation 不变

---

## Open Questions

### Resolved During Planning

- **Q: fixer 预算耗尽后是否直接终止？** → 否，给 executor 一次重写机会（max 10），重写也耗尽才终止
- **Q: review 是否完全不做 per-step？** → 是，所有 unit 完成后总体 review 一次
- **Q: validator 跑全量测试还是只跑受影响测试？** → 全量测试（`cargo nextest run`），简单可靠
- **Q: progress-steward 是否保留？** → 保留，`loop.stalled` 唤醒仍有价值

### Deferred to Implementation

- **executor TDD 的 complexity 分级**：当前硬编码 `large` 行为，trivial/small 分级逻辑后续迭代
- **fixer fix-log.md 的具体格式**：实现时参照现有 fixer instructions 中的 fix-log 格式，合并诊断段

---

## Verification

```bash
# 1. preset lint
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint

# 2. WAC 校验
cargo nextest run -p ralph-core -- workflow_activation

# 3. validate_ambiguous_routing
cargo nextest run -p ralph-core -- ralph_config

# 4. 剩余的 ce_executor 测试（应只剩 5 个）
cargo nextest run -p ralph-cli --bin ralph -- ce_executor

# 5. scenario 测试
cargo nextest run -p ralph-core --test scenarios

# 6. SSOT byte-equality
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded

# 7. review_terminal_coherence
cargo nextest run -p ralph-core -- review_terminal_coherence

# 8. 全量基线
./scripts/run-tests.sh
```

---

## 附录：U4 需删除的测试函数完整列表

```
test_ce_executor_publish_chain_origin_compatible
test_ce_executor_reporter_publishes_report_done
test_ce_executor_forbids_agent_branch_creation
test_ce_executor_dimension_reviewer_timeout_is_900
test_ce_executor_serial_synthesizer_triggers_on_dimensions_complete
test_ce_executor_serial_dimension_reviewer_no_concurrency_no_aggregate
test_ce_executor_serial_progress_steward_only_loop_stalled
test_ce_executor_serial_topic_ownership
test_ce_executor_serial_review_coordinator_fix_applied_must_not_allow_complete
test_ce_executor_serial_has_no_wave_topic
test_ce_executor_serial_review_sequence_is_two_dimensions
test_ce_executor_serial_plan_gate_must_not_listen_to_fix_applied
test_ce_executor_has_hard_commit_cadence
test_ce_executor_has_preflight_contract
test_ce_executor_plan_gate_exists_and_routes_correctly
test_ce_executor_shipper_triggers_finalization_only
test_ce_executor_executor_publishes_excludes_queue_advance
test_ce_executor_work_done_field_consistency
test_ce_executor_failure_topics_accept_reason_only_payloads
test_ce_executor_reporter_defensive_plan_check
test_ce_executor_verdict_gate_targets_review_complete
test_ce_executor_dimension_reviewer_passes_through_task_correlation
test_ce_executor_shipper_commit_only_on_plan_complete
test_ce_executor_executor_reads_reviewed_task_id_on_queue_advance
test_ce_executor_state_projection_enabled_serial_en
test_ce_executor_orchestrator_context_is_canonical_read_source_en
test_ce_executor_orchestrator_context_is_canonical_read_source_serial_en
test_ce_executor_u4_legacy_progress_reconcile_is_superseded
test_ce_executor_fixer_reads_task_correlation_fields
ce_executor_serial_state_projection_queue_advance_uses_next_step_pointer
test_e2e_step_handoff_topology_complete
```
