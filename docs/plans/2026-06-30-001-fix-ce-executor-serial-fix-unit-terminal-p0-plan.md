---
title: fix: ce-executor-serial fix-unit 终态处理 6 P0 修复
type: fix
status: active
date: 2026-06-30
origin: docs/report/2026-06-30-ce-executor-serial-primary-20260630-032648-diagnosis.md
---

# fix: ce-executor-serial fix-unit 终态处理 6 P0 修复

## Overview

修复 `ce-executor-serial` preset 在 `primary-20260630-032648` run 中暴露的 6 个 P0 机制缺陷。这些缺陷全部集中在 **fix-unit 完成后的终态处理路径**：coordinator 错发 `review.start`、`plan.complete` 被 plan_gate 拦截、`shipper` 把兜底 recovery reason 升级为 pass、`REVIEW_COMPLETE` 重复、`LOOP_COMPLETE` 在 `report.done` 之前抢发、任务账本出现孤儿 closed 任务、ledger 双计数器错位。

本次计划只覆盖 6 个 P0，P1/P2 可观测性/边缘硬化项留作后续 plan。

---

## Problem Frame

2026-06-30 `primary-20260630-032648` 跑 `ce-executor-serial` preset，执行 `2026-06-20-001-feat-python-sort-algorithms` plan。4 个 plan unit 正常完成，6 维 review 走完后触发 fix-unit 链（fix-01 / fix-02）。fix-02 的 `test.passed` 之后，系统没有正常进入 `plan.complete → shipper → reporter → LOOP_COMPLETE` 终态，而是被多层 recovery 兜底拉偏：

- coordinator 在 progress-steward 的 `task.resume` 引导下发出第二轮 `review.start`（违反 preset 硬规则）。
- `plan.complete` 因 `plan_gate_review_not_terminal` / `step_handoff::task_not_found` 被降级为 `plan.blocked`。
- shipper 把 `plan.blocked(reason=stall_no_events recovery ...)` 按 "recoverable reason" narrative 升级为 pass。
- `REVIEW_COMPLETE` 在 29 秒内重复发了两次字节级相同的 payload。
- ralph runner / reporter 在 `report.done` 到位前抢发/自发 `LOOP_COMPLETE`。
- `tasks.jsonl` 出现 `key=null, started_at=null, closed=...` 的孤儿任务。
- `ledger.jsonl` 中 `consecutive_no_progress_turns` 与主 iteration 序列错位。

最终交付虽然成功（commit `c6d67b5` 落地、52 测试通过），但靠的是 runtime 兜底 + shipper reason 越界升级，不是预设的正常路径。本 plan 目标是把 fix-unit 终态路径修成 "正常路径即可闭环"。

完整诊断见 `docs/report/2026-06-30-ce-executor-serial-primary-20260630-032648-diagnosis.md`。

---

## Requirements Trace

- **R1.** fix-unit 最后一个 `test.passed(fix-NN)` 之后，coordinator 必须发出 `plan.complete`，禁止再发 `review.start`。
- **R2.** `shipper` 对 `plan.blocked` 的 reason 路由必须是严格白名单 exact match，禁止 narrative/substring 越界升级为 pass。
- **R3.** `REVIEW_COMPLETE` 在 events 流中必须只出现一次；重复 emit 必须被 runtime 拒绝或标记为 duplicate。
- **R4.** `TaskStore` 禁止关闭 `started_at == None` 的任务；`project_plan_complete` 关闭 open task 前必须检查 started 状态。
- **R5.** `LOOP_COMPLETE` 的 `completion_requested` 只能在 `report.done` 已被 observed 后才能置位；ralph runner 不能抢发终态。
- **R6.** `consecutive_failures` 与 `consecutive_no_progress_turns` 在 ledger 输出中的 iteration 序列必须单调一致，不能跨双计数器错位。

---

## Scope Boundaries

### In scope

- 6 个 P0 的源码修复，按依赖顺序串行落地。
- 每个 P0 至少 1 个 BDD scenario 或单元测试防止回归。
- 涉及的 preset/schema 同步：`presets/en/ce-executor-serial.yml`、`presets/schemas/ce-executor-serial.yml`。
- 涉及的 skill guide 同步：`crates/ralph-core/data/ralph-tools*.md` 中受影响的行为描述。
- 涉及的文档同步：`CLAUDE.md` / `AGENTS.md`（如 preset builtin 列表、串并行测试入口说明无变化则无需大改，但需复核）。

### Deferred to Follow-Up Work

- P1/P2 项：`report.done` 缺 `verdict` 字段、fix-02 `commit_count=0` 时序、task ensure 强 key 约束、recovery.jsonl bucket 分桶、`triggered=ralph` 语义稀释。
- 长期架构：`DEFENSIVE_BYPASS` 收敛、progress-steward / stall_recovery / missing_event_gate 统一决策表、step 推进显式状态机重构。

### Outside this product's identity

- 重写整个 EventBus / StateMachine。
- 为 isolated/wave preset 设计新的 handoff 协议。

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/task_store.rs` — `close_by_key` / `close` 生命周期守卫。
- `crates/ralph-core/src/state_projector/progress.rs` — `project_plan_complete` 关闭剩余 open tasks。
- `crates/ralph-core/src/event_loop/loop_state.rs` — `completion_requested`、`completion_honored`、双计数器字段。
- `crates/ralph-core/src/event_loop/mod.rs` — `check_completion_event`、`run_stall_detector_on_state`、JSONL parse 中 `LOOP_COMPLETE` 处理。
- `crates/ralph-core/src/event_loop/review_step_state.rs` — `plan.complete` plan_gate（已有 fix-* 放行）。
- `crates/ralph-core/src/event_loop/stages/verdict_gate_stage.rs` — `DEFAULT_TERMINAL_EMITS` 仅含 `LOOP_COMPLETE`。
- `presets/en/ce-executor-serial.yml` — coordinator PHASE GATE 表、progress-steward 状态机表、shipper reason 路由。
- `crates/ralph-core/tests/scenarios/` + `crates/ralph-core/tests/scenarios.rs` — BDD harness，必须用 `run_workflow_guard_scenario`。

### Institutional Learnings

- `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` — lint + runtime gate + verdict gate 三层防御模式。
- `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` — plan-gate → executor dispatch gap 同源。
- `docs/achieved/plan/2026-06-29-007-fix-ce-executor-serial-mechanism-p0-p1-plan.md` — 同 preset 近期 P0/P1 修复，13 单元 TDD 模式，BDD 用 `run_workflow_guard_scenario`。
- `docs/solutions/logic-errors/ce-executor-p0-event-policy-and-projector-fanout.md` — TaskStore close 边界、event-policy / projector fanout 问题。

### External References

- 无外部依赖；全部基于 repo 内诊断报告与历史方案。

---

## Key Technical Decisions

- **KTD-1. 按依赖顺序串行修复。** P0-4（task_store 守卫）与 P0-5（completion_requested guard）是基座层改动，先落地；P0-1（progress-steward 分桶）依赖 P0-5；P0-2（shipper 白名单）和 P0-3（REVIEW_COMPLETE 唯一性）可并行但放在 P0-1 之后；P0-6（双计数器）最后收敛观测。
- **KTD-2. 机器 enforcement 优先于 prompt 约束。** P0-1、P0-2、P0-5 的根因都是 "prompt-as-state-machine" 未被 runtime 强制；本 plan 优先在 Rust 层加 guard，再用 preset prompt/lint 作为辅助。
- **KTD-3. 每个 P0 一个 BDD scenario。** 事件拓扑类 bug 必须用 `run_workflow_guard_scenario` 真 EventLoop runner 断言事件序列，禁止用 `run_scenario` stub（见 CLAUDE.md preset/schema 改动下游同步清单）。
- **KTD-4. 保持 10-hat isolated serial 编排不变。** 问题出在机制层 fail-safe，不在编排选型；不引入新 hat 或改变 review_walk 维度。

---

## Open Questions

### Resolved During Planning

- **Q1. 范围是否包含 P1/P2？** 否，本次仅 6 个 P0，P1/P2 留 follow-up。
- **Q2. P0-1 是否通过新增 progress-steward 独立行实现？** 是，新增 `fix_unit_complete_plan_complete_pending` 行，同时收紧 coordinator PHASE 2 branch gate。
- **Q3. P0-5 的 guard 是否放在 `mark_completion_requested` 或 parse 路径？** 放在 `LoopState::mark_completion_requested` 层，新增 `report_done_seen` 字段，由 `observe_accepted` 在收到 `report.done` 时置位。

### Deferred to Implementation

- **Q4. P0-3 选择哪条防线？** 待实现时根据 `verdict_gate_stage` / `event_policy.completion_after_terminal` 的现有结构决定：把 `REVIEW_COMPLETE` 纳入 terminal uniqueness 检查，或在 event_policy 层加 `duplicate_non_terminal` 规则。本 plan 只要求 "events 流中只出现一次"，具体防线由实现者选择最小改动。
- **Q5. P0-6 选择合并计数器还是统一 ledger topic？** 待实现时决定：合并 `consecutive_failures` / `consecutive_no_progress_turns` 到一个 `consecutive_stall_turns` map，或保持双轨但让 ledger `loop.batch_sync` 统一使用主 iteration 序列号。本 plan 要求 ledger iteration 单调一致。

---

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

```mermaid
flowchart TB
  subgraph fix-unit terminal path
    TP[test.passed(fix-NN)] --> CG{coordinator PHASE 2 gate}
    CG -->|last fix-unit| PC[plan.complete]
    CG -->|more fix-units| WR[work.ready(next-fix)]
    PC --> SH[shipper REVIEW_COMPLETE]
    SH --> RP[reporter report.done]
    RP --> LOOP[LOOP_COMPLETE]
  end

  subgraph progress-steward recovery
    ST[stall_no_events] --> PS{fix-unit done?}
    PS -->|yes| FIX_RESUME[task.resume reason=fix_unit_complete_plan_complete_pending target=coordinator]
    PS -->|no| REV_RESUME[task.resume reason=review_sequence_not_advanced target=coordinator]
    FIX_RESUME --> CG
  end

  subgraph completion guard
    LOOP_REQ[LOOP_COMPLETE request] --> RD{report.done seen?}
    RD -->|no| REJ_LOOP[reject completion_requested]
    RD -->|yes| SET[set completion_requested]
  end
```

---

## Implementation Units

- [ ] U1. **TaskStore 禁止关闭未开始的任务（P0-4）**

**Goal:** 消除 `tasks.jsonl` 中 `started_at=null` 的 closed 孤儿任务。

**Requirements:** R4

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/task_store.rs`
- Modify: `crates/ralph-core/src/state_projector/progress.rs`
- Test: `crates/ralph-core/src/task_store.rs`（已有 tests 模块）

**Approach:**
1. 在 `TaskStore::close_by_key` 开头增加 `started_at.is_none()` 守卫，遇到未开始任务返回 `None` 并记录 `tracing::warn!`。
2. 同步检查 `TaskStore::close` 是否需要同样守卫（若 `project_plan_complete` 调用的是 `close(id)` 而非 `close_by_key`）。
3. 在 `state_projector/progress.rs::project_plan_complete` 的 close 循环中，只关闭 `task.started_at.is_some()` 的任务。

**Patterns to follow:**
- 参考 `docs/solutions/logic-errors/ce-executor-p0-event-policy-and-projector-fanout.md` 中 `CloseOutcome` 模式。

**Test scenarios:**
- Happy path: `work.done` 正常关闭一个已 started 的任务 → 状态变为 Closed。
- Edge case: `close_by_key` 命中 `started_at=null` 的任务 → 返回 `None`，任务状态不变。
- Edge case: `project_plan_complete` 遇到 `started_at=null` 的 open 任务 → 跳过，不关闭。
- Integration: 修复后复现 `tasks.jsonl` 不再出现 L5 型孤儿任务。

**Verification:**
- `cargo nextest run -p ralph-core -- task_store` 通过。
- 新增/现有单元测试覆盖 `close_by_key` / `close` 的 null-started 行为。

---

- [ ] U2. **completion_requested 增加 report.done 前置 guard（P0-5）**

**Goal:** 防止 ralph runner / agent 在 `report.done` 到达前就把 `completion_requested` 置位并抢发 `LOOP_COMPLETE`。

**Requirements:** R5

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/event_loop/loop_state.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/event_loop/tests/completion_honored.rs`

**Approach:**
1. 在 `LoopState` 新增 `report_done_seen: bool` 字段，默认 `false`。
2. 在 `observe_accepted`（或事件 accepted 的统一入口）收到 `report.done` 时置 `report_done_seen = true`。
3. 提供 `mark_completion_requested() -> Result<(), String>`：若 `!report_done_seen`，返回 Err("completion_requested rejected: report.done has not been observed yet")。
4. 替换所有直接写 `completion_requested = true` 的调用点：text fallback、`default_publishes` 命中 completion promise、JSONL parse 到 `LOOP_COMPLETE` 等，统一走 `mark_completion_requested`。

**Patterns to follow:**
- 参考 `docs/achieved/plan/2026-06-27-002-feat-mechanism-foundation-completion-plan.md` 的终态机思想。

**Test scenarios:**
- Happy path: 收到 `report.done` 后再收到 `LOOP_COMPLETE` → `completion_requested` 成功置位。
- Error path: 未收到 `report.done` 时收到 `LOOP_COMPLETE` → `completion_requested` 被拒绝，ledger 不写入 `loop.completion_requested`。
- Error path: review chain 刚起步时 ralph 抢发 `LOOP_COMPLETE` → 与 `primary-20260630-032648` events L37 同场景，应被拒绝。
- Integration: `check_completion_event` 在 `completion_requested` 被拒绝后返回 `None`，不终止 loop。

**Verification:**
- `cargo nextest run -p ralph-core -- completion_honored` 通过。
- 新增测试 `test_loop_complete_rejected_before_report_done`。

---

- [ ] U3. **fix-unit 完成后 progress-steward 分桶 + coordinator PHASE 2 gate 收紧（P0-1）**

**Goal:** 保证最后一个 fix-unit `test.passed` 之后走 `plan.complete`，不再被 progress-steward 误注入第二轮 `review.start`。

**Requirements:** R1

**Dependencies:** U2

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`
- Modify: `crates/ralph-core/src/event_loop/stages/coordinator_decision_gate.rs`（若已存在，否则新建）
- Test: `crates/ralph-core/tests/scenarios/2026-06-30-001-u3-fix-unit-terminal.yml`
- Test: `crates/ralph-core/tests/scenarios.rs`

**Approach:**
1. 在 `presets/en/ce-executor-serial.yml` progress-steward 状态机表（line ~2758）新增独立行：
   - `kind: fix_unit_complete_plan_complete_pending`
   - `when: "all fix-units closed in tasks.jsonl AND coordinator never emitted plan.complete"`
   - `emit: "task.resume(target_hat=coordinator, reason=fix_unit_complete_plan_complete_pending)"`
   - `note: "nudges coordinator to re-emit plan.complete, NOT review.start"`
2. 在 coordinator PHASE 2 branch gate（line ~815-829）追加硬规则：若 `completed_steps` 含 fix-* 且 `plan.complete` 尚未 emit，必须 emit `plan.complete`，禁止 `review.start`。
3. 若 runtime 层有 `CoordinatorDecisionGate`，扩展它以校验 fix-unit 完成后 emit `review.start` 时 reject；否则依赖 preset prompt + BDD scenario 捕获。

**Patterns to follow:**
- 参考 `docs/achieved/plan/2026-06-29-007-fix-ce-executor-serial-mechanism-p0-p1-plan.md` U10 PHASE 2 branch gate。

**Test scenarios:**
- Happy path: 4 step + fix-01 + fix-02 → fix-02 `test.passed` 后 emit `plan.complete`，events 流中出现 1 次 `plan.complete`。
- Error path: 模拟 coordinator 在 fix-02 后 emit `review.start` → 被 runtime gate reject（或 scenario 断言 `review.start` 不出现）。
- Integration: 完整 happy path 跑通 `plan.complete → REVIEW_COMPLETE → report.done → LOOP_COMPLETE`，无第二次 review walk。

**Verification:**
- `cargo nextest run -p ralph-core -- scenarios::test_u3_fix_unit_terminal` 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 通过。

---

- [ ] U4. **shipper reason 路由严格白名单（P0-2）**

**Goal:** `plan.blocked` 的 reason 必须 exact match 白名单才能 recoverable pass，任何 narrative/substring 匹配都走 hard-fail。

**Requirements:** R2

**Dependencies:** U3（依赖 fix-unit 路径先能正常产出 `plan.complete` / `plan.blocked`）

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`（shipper instructions，line ~2491-2509）
- Create: `crates/ralph-core/src/preset_lint/strict_reason_routing.rs`（新 lint 规则）
- Modify: `crates/ralph-core/src/preset_lint/mod.rs`、`finding_id.rs`
- Test: `crates/ralph-core/src/preset_lint/tests/strict_reason_routing.rs`
- Test: `crates/ralph-core/tests/scenarios/2026-06-30-001-u4-shipper-reason-whitelist.yml`

**Approach:**
1. 在 shipper instructions 中把 "Recoverable reasons" 改为 **STRICT-MATCH whitelist**：`["loop_stalled_max_iterations", "steward_escalation", "review_terminal_drift"]`，并强调 "any other reason MUST hard-fail"，禁止把 "with recoverable reason X" 字面前缀当权威。
2. 新增 `strict_reason_routing` lint：扫描 shipper prompt，检测 reason 路由描述是否含 "strict" / "exact match"，缺失则告警（防御 prompt drift）。
3. 在 `preset_lint::mod.rs` 注册新 lint 模块和 finding。

**Patterns to follow:**
- 参考现有 `preset_lint` 模块结构（`schema_parity.rs`、`workflow_activation.rs` 等）。
- 参考 `AGENTS.md` 对 preset/schema 同步的 hard rule。

**Test scenarios:**
- Happy path: `plan.blocked(reason="loop_stalled_max_iterations")` → shipper emit `REVIEW_COMPLETE(pass_or_fail=pass, verdict=pass_with_residuals)`。
- Error path: `plan.blocked(reason="stall_no_events recovery: ...")` → shipper emit `REVIEW_COMPLETE(pass_or_fail=fail, verdict=fail)`。
- Error path: `plan.blocked(reason="Steward_Escalation")`（大小写不同）→ hard-fail（strict exact match）。
- Lint: shipper prompt 缺少 "STRICT-MATCH" / "exact match" 描述 → `preset_lint` 报 finding。

**Verification:**
- `cargo nextest run -p ralph-core -- preset_lint` 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 通过。
- BDD scenario `test_u4_shipper_reason_whitelist` 通过。

---

- [ ] U5. **REVIEW_COMPLETE 唯一性/去重 guard（P0-3）**

**Goal:** 同一 loop 内禁止两次字节级相同的 `REVIEW_COMPLETE` 写入 events 流。

**Requirements:** R3

**Dependencies:** U3、U4

**Files:**
- Modify: `crates/ralph-core/src/event_loop/loop_state.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（observe / parse 路径）
- Test: `crates/ralph-core/src/event_loop/tests/completion_honored.rs`
- Test: `crates/ralph-core/tests/scenarios/2026-06-30-001-u5-review-complete-dedup.yml`

**Approach:**
1. 在 `LoopState` 新增 `review_complete_seen_payload_hash: Option<u64>` 字段。
2. 在 observe accepted 路径收到 `REVIEW_COMPLETE` 时，计算 payload SHA1/xxhash；若与已存 hash 相同，则：
   - 不阻止 reporter 内部 `verdict_gate` dedup（保持现有行为），
   - 但记录 diagnostic event / `delta.kind=duplicate_review_complete`，并拒绝写入 events 流作为新事件。
3. 或者在 `verdict_gate_stage` / `event_policy` 层把 `REVIEW_COMPLETE` 视为 "terminal-adjacent" 唯一事件；具体防线由实现者根据现有结构选择最小改动。

**Patterns to follow:**
- 参考 `completion_honored` 的 idempotency 实现模式。
- 参考 `event_policy.completion_after_terminal.duplicate_terminal` 配置。

**Test scenarios:**
- Happy path: 正常 happy path 出现 1 次 `REVIEW_COMPLETE` → 通过。
- Error path: 同一 loop 内注入两次字节级相同的 `REVIEW_COMPLETE` → 第二次被标记为 duplicate，events 流中只保留 1 次。
- Edge case: 两次 `REVIEW_COMPLETE` payload 不同（如 verdict 从 pass 变 fail）→ 允许第二次（不属于 duplicate）。
- Integration: reporter 仍能基于第一次 `REVIEW_COMPLETE` 正常产出 `report.done` 和 `LOOP_COMPLETE`。

**Verification:**
- `cargo nextest run -p ralph-core -- completion_honored` 通过。
- BDD scenario `test_u5_review_complete_dedup` 通过。

---

- [ ] U6. **ledger 双计数器收敛（P0-6）**

**Goal:** 消除 `consecutive_failures` 与 `consecutive_no_progress_turns` 在 ledger 序列号上的错位，让 summary iter 与 ledger main iter 一致。

**Requirements:** R6

**Dependencies:** U2（completion_requested guard 改动涉及同一文件）

**Files:**
- Modify: `crates/ralph-core/src/event_loop/loop_state.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`process_output` 和 `run_stall_detector_on_state`）
- Test: `crates/ralph-core/src/event_loop/tests/progress_steward.rs`

**Approach:**
1. 方案 A（推荐）：合并 `consecutive_failures` 与 `consecutive_no_progress_turns` 到 `consecutive_stall_turns: HashMap<String, u32>`，`no_progress` 与 hat 执行失败都按 kind 自增；在 ledger 输出中统一使用主 iteration 序列号，不再单独输出 `loop.batch_sync.no_progress`。
2. 方案 B（保守）：保持双轨，但让 `loop.batch_sync.no_progress` 携带与 `loop.batch_sync` 相同的 `iteration` 值，仅在 `delta.counter` 区分 kind。
3. 实现者根据回归测试影响选择方案 A 或 B；本计划要求最终 ledger 的 iteration 序列单调一致。

**Patterns to follow:**
- 参考 `LoopState::stall_recovery_counts: HashMap<String, u32>` 已有模式。
- 参考 `progress_steward.rs` 现有测试对 counter 的断言。

**Test scenarios:**
- Happy path: 连续 no-progress 3 轮后触发 `loop.stalled`，ledger 中 iteration 序列连续递增，无 seq-37(no_progress=35) vs seq-39(main=36) 错位。
- Edge case: 一轮 hat 执行失败 + 一轮 no-progress → `consecutive_stall_turns` 按 kind 分别计数，不互相污染。
- Error path: 保持 `max_consecutive_failures` 终止门（默认 5）行为不变。
- Integration: `summary.md` 显示的 iter 数与 ledger 最后一条 `loop.batch_sync` 的 iteration 一致。

**Verification:**
- `cargo nextest run -p ralph-core -- progress_steward` 通过。
- `cargo nextest run -p ralph-core -- loop_state` 通过。
- 新增/更新测试断言 ledger iteration 单调性。

---

## System-Wide Impact

- **Task ledger (`tasks.jsonl`)**: U1 后 `started_at=null` 的占位任务不再被强制 closed，validator 看到的 open_tasks 视图更准确。
- **Completion state machine**: U2 后 `completion_requested` 受 `report_done_seen` guard 保护，任何在 `report.done` 前的 `LOOP_COMPLETE` 都不能推进终态。
- **Progress-steward state machine**: U3 后 fix-unit 完成场景有独立 recovery bucket，不再与 review sequence not advanced 混用。
- **Preset lint surface**: U4 新增 `strict_reason_routing` lint，preset maintainer 修改 shipper reason 路由时会得到早期告警。
- **Event stream uniqueness**: U5 后 `REVIEW_COMPLETE` 重复 emit 被 runtime 捕获，events.jsonl 更干净。
- **Ledger observability**: U6 后 ledger iteration 序列单调一致，`ralph diagnose` 与 `summary.md` 不再错位。
- **Unchanged invariants**: 10-hat topology、`execution_mode: isolated`、6-dim review_walk、`completion_promise: LOOP_COMPLETE`、`required_events: ["report.done"]` 均保持不变。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|-----------|
| U2 的 `report_done_seen` guard 可能让合法 text-fallback completion 路径也被拒 | 确保 `report.done` 是 `required_events` 唯一项；text fallback 仅在 agent 显式承诺 completion 时触发，正常情况下 `report.done` 必先出现 |
| U3 的 progress-steward 新 bucket 与现有 `review_sequence_not_advanced` 冲突 | 在 BDD scenario 中构造两种场景，确保 when 条件互斥 |
| U4 的 strict whitelist 可能误拦截原本合法的 recoverable reason | 白名单 exact match 前先 `trim().to_lowercase()` 规范化；场景覆盖大小写/空格边界 |
| U5 的去重防线与 reporter 内部 verdict_gate 交互 | 保持 reporter dedup 不变，只在 events 流层拒绝重复写入；BDD 断言 events 流 count=1 |
| U6 合并计数器可能影响 `max_consecutive_failures` 终止门 | 保留 `consecutive_failures >= cfg.max_consecutive_failures` 终止语义；测试覆盖 |
| Preset/schema 不同步 | 严格按 AGENTS.md 下游同步清单执行：preset_lint、SSOT byte-equality、BDD scenarios |

---

## Documentation / Operational Notes

- 修改 `presets/en/ce-executor-serial.yml` 后，必须同步 `presets/schemas/ce-executor-serial.yml`。
- 改完必须跑：
  - `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
  - `cargo nextest run -p ralph-core -- preset_lint`
  - `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`
- 若涉及 skill guide 中命令/行为描述，同步 `crates/ralph-core/data/ralph-tools*.md`，并跑 `scripts/check-cli-doc-drift.sh`。
- `CLAUDE.md` / `AGENTS.md` 若因 preset 列表或 hard rule 变化需更新，必须同步 `cp CLAUDE.md AGENTS.md`。
- 每个 U 落地后先跑 targeted tests，全部 U 完成后跑 `./scripts/run-tests.sh` 全量 baseline。

---

## Sources & References

- **Origin document:** `docs/report/2026-06-30-ce-executor-serial-primary-20260630-032648-diagnosis.md`
- **Upstream requirements:** `docs/brainstorms/2026-06-21-serial-preset-root-cause-fix-requirements.md`
- **Related achieved plan:** `docs/achieved/plan/2026-06-29-007-fix-ce-executor-serial-mechanism-p0-p1-plan.md`
- **Related solutions:**
  - `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md`
  - `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md`
  - `docs/solutions/logic-errors/ce-executor-p0-event-policy-and-projector-fanout.md`
- **Key source references:**
  - `crates/ralph-core/src/task_store.rs:436-459`
  - `crates/ralph-core/src/state_projector/progress.rs:117-128`
  - `crates/ralph-core/src/event_loop/loop_state.rs:182, 212-214, 334`
  - `crates/ralph-core/src/event_loop/mod.rs:6670-6675, 9136-9142, 10839-10987`
  - `crates/ralph-core/src/event_loop/review_step_state.rs:305-353`
  - `crates/ralph-core/src/event_loop/stages/verdict_gate_stage.rs:19-30`
  - `presets/en/ce-executor-serial.yml:815-829, 2491-2509, 2758-2764`
