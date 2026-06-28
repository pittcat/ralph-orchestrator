---
title: "fix: ce-executor-serial fix-unit 链路与 9 个新机制失效"
type: fix
status: active
date: 2026-06-28
origin: docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md
---

# fix: ce-executor-serial fix-unit 链路与 9 个新机制失效

## Overview

`ce-executor-serial` 在 `fix-unit` 阶段发生编排冲突 + 协调器死锁 + recovery 升级未终止，`LOOP_COMPLETE` 从未触发，U3/U6/U7/U8/U9/U9.5/U11/U12/U13 全部未生效。

本次修复聚焦 **P0 必做项**：把基座三处未同步更新（`plan_gate`、`allowed_topics`、`recovery 终止`）与 CLI/preset 两处分裂（CLI emit 绕开 stage_pipeline、`total_units` 缺失、U8 未接入热路径）合并闭环。

执行方式：**8 个 P0 修复 Unit 纯粹串行、绝对隔离、TDD 驱动**，最后 1 个 Unit 做全量回归与下游同步。

---

## Problem Frame

`ce-executor-serial` 的 `fix-unit` 流程与 Ralph 基座存在结构性错位：

1. **`plan_gate` 把 fix-unit 当普通 plan step**：`review.complete(fix_plan_file)` 之后，fix-unit 没有 `review.passed`/`review.complete` terminal，导致 `plan.complete` 被 `plan_gate_review_not_terminal` 拒绝，coordinator 死锁。
2. **coordinator 使用 placeholder task_id**：`task-fix-01-placeholder` 跨 step 复用，`work.done` 触发 `TaskWrongLoop`。
3. **recovery 路由的 `allowed_topics` 与 hat publishes 不一致**：coordinator 收到的 `task.resume` 不含 `work.ready`，结果它尝试 emit 越权的 `work.start`，触发 isolated scope violation 与 circuit breaker。
4. **recovery `Final` 不终止 loop**：`RECOVERY-FINAL-WARNING` 只发 `human.guidance`，loop 继续空转。
5. **CLI `ralph emit` 绕开 stage_pipeline**：U6/U7/U9/U9.5/U12 在 CLI 路径全部失效。
6. **`state_idempotency: required` 未接入热路径**：任务写入不走 `idempotent_wiring::write_task`，U8 完全失效。
7. **preset 未声明 `total_units`**：`StepCloseObligationStage`  fail-open，U12 失效。
8. **首次跑不写 `loop-version.json`**：U11 设计性 no-op，U13 无法验证。

---

## Requirements Trace

- **R1 — fix-unit plan_gate 豁免**：fix-unit 的 `plan.complete` 不应要求 review terminal；`review.complete(fix_plan_file)` 应为所有 fix step 预填 terminal 状态。
- **R2 — coordinator 使用真实 task_id**：fix-unit 派发前必须先创建真实 task，禁止 placeholder；CLI 支持 `ralph tools task create --for-fix-unit`。
- **R3 — recovery allowed_topics 与 hat publishes 对齐**：所有注入的 `task.resume` 的 `allowed_topics` 必须等于目标 hat 的 `publishes`。
- **R4 — recovery Final 真正终止**：`EscalationLevel::Final` 必须让 loop 终止，而不是只发 warning。
- **R5 — CLI emit 走 stage_pipeline**：`run_policy_check_unified` 必须调用 `evaluate_emit_gate`，repair/reject 事件写入 recovery 流。
- **R6 — U8 接入热路径**：`TaskStore::save` 在启用 `state_idempotency` 时把 task 写入 `IdempotentLog`。
- **R7 — U12 不再 fail-open**：未声明 `total_units` 的 fix step 应能从 tasks.jsonl 的 fix-unit 计数推导 total。
- **R8 — U11 首次跑写 loop-version.json**：`EventLoop::new` 在 fresh workspace 显式初始化 `loop-version.json`。

---

## Scope Boundaries

- **本次只做 P0 必做项**，让 `ce-executor-serial` 的 fix-unit 链路能跑通、`LOOP_COMPLETE` 能触发、9 个新机制能在 CLI 与 loop 两条路径生效。
- 不改 isolated 执行模式、不改 wave 协议、不改 review dimension 协议。
- 不新增 hat、不新增 event topic。

### Deferred to Follow-Up Work

- **P1 产物/观测项**：`shipping.md` 口径对齐（R5）、progress-steward 显式声明（R6）、drift_monitor 字段识别修复（R7）、U7 自动 emit repair topic（M5）、U13 fail-closed 集成测试（M6）。
- **P2 文档/预防项**：`ralph-tools-cmdref.md` 更新（R8）、placeholder HARD RULE（M7）、IdempotentLog 独立 open（M8）。
- 把本计划中的机制推广到其他 builtin preset 的 isolated 模式。

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/event_loop/review_step_state.rs` — `ReviewStepTracker::check_semantic_gates` / `observe_accepted`。
- `crates/ralph-core/src/diagnosis/responder.rs` — `record_finding` 中的 `EscalationLevel::Final`。
- `crates/ralph-core/src/drift/engine.rs` — `check_termination_hint`、`publish_hard_recovery_event`。
- `crates/ralph-core/src/event_loop/mod.rs` — fallback stall injection、archive / idempotent log bootstrap、`drive_step_close_progress`。
- `crates/ralph-core/src/task_store.rs` — `save` / `save_with_idempotent_log`。
- `crates/ralph-core/src/state/idempotent_log.rs` — `IdempotentLog::open`。
- `crates/ralph-cli/src/policy_check.rs` — `run_policy_check_unified`。
- `crates/ralph-cli/src/task_cli.rs` — `ralph tools task create`。
- `presets/en/ce-executor-serial.yml` — coordinator fix-unit 派发 instructions。
- `crates/ralph-core/src/event_loop/tests/review_step_gate.rs`、`u11_wiring.rs`、`u8_legacy_relocate_and_close.rs`、`u6_wiring.rs` 等现有测试。

### Institutional Learnings

- `docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md` — `ralph-cli` 测试必须走 `cargo nextest run` 串行。
- `AGENTS.md` — preset/schema 改动后必须同步 7 处下游；`CLAUDE.md` 与 `AGENTS.md` 同步。
- `.cursor/rules/multi-hat-isolation.mdc` — isolated mode 下 hat publishes 是单一事实源。

### External References

- 无外部依赖；所有模式均来自本地代码与历史 solutions。

---

## Key Technical Decisions

1. **P0 先行，P1/P2 后置**：先把 fix-unit 链路跑通、机制生效，再处理产物/观测/文档漂移。
2. **8 个 Unit 严格串行、绝对隔离**：每个 Unit 只改一处行为、只验证自己的输入输出；Unit 之间不共享实现细节；U9 作为唯一集成点。
3. **测试先行**：每个 Unit 先写 failing test，再写最小实现，最后重构；不允许把当前 Unit 的边界问题留给下一 Unit。
4. **单一事实源**：hat 的 `publishes` 是 `allowed_topics` 的唯一来源；fix-unit 的 task 数量是 `total_units` 的 fallback 来源。
5. **stage_pipeline 统一 CLI 与 loop**：`run_policy_check_unified` 必须复用 loop 内同一套 `evaluate_emit_gate` facade，避免“幽灵路径”。
6. **IdempotentLog 显式初始化**：首次跑在 archive 后主动写 `loop-version.json`，打破 U11 的 no-op。

---

## Open Questions

### Resolved During Planning

- **P1/P2 是否纳入本计划？** 不纳入；本计划只覆盖 P0，P1/P2 作为 follow-up。
- **是否拆分 R3 为两个 Unit？** 合并为一个 Unit：fallback stall 注入与 hard recovery 注入都需要 allowed_topics，共用同一 helper，测试分别覆盖两条路径。
- **U8 是否必须改 `TaskStore::save` 签名？** 通过让 `EventLoop` 把一个共享 `IdempotentLog` 句柄注册给 `TaskStore` 来解决，不扩散签名改动。

### Deferred to Implementation

- `ralph tools task create --for-fix-unit` 的具体 CLI 参数形态（flag 还是子命令）实现时与现有 clap 定义对齐。
- `run_policy_check_unified` 中 repair envelope 的字段名与下游 `recovery.jsonl` 消费方是否完全兼容，实现时通过测试确认。
- fix-unit total fallback 的精确匹配规则（按 `task_key` 前缀还是 `step` 字段）实现时与 `tasks.jsonl` schema 对齐。

---

## High-Level Technical Design

> *本节用来说明改动形状，不是可复制粘贴的实现规范。*

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│  U1: plan_gate 豁免 fix-unit                                                 │
│  review.complete(fix_plan_file) → 预填 fix-{NN} synth_terminal              │
│  plan.complete(step=fix-*) → 直接 accept                                     │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U2: recovery Final 终止                                                     │
│  EscalationLevel::Final → TerminationHint severity=Critical                 │
│  drift_engine::check_termination_hint → RecoveryExhausted                   │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U3: recovery allowed_topics = hat publishes                                 │
│  fallback stall injection / hard recovery event 携带 allowed_topics         │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U4: U11 首次跑写 loop-version.json                                          │
│  archive_state_for_loop 后显式初始化 version=1                              │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U5: U8 task 写入走 IdempotentLog                                            │
│  TaskStore 持有共享 IdempotentLog；save() 同时写 JSONL + idempotent record  │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U6: U12 total_units fallback                                                │
│  fix step 未声明 total_units → 从 tasks.jsonl fix-unit 计数推导             │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U7: CLI emit 接入 stage_pipeline                                            │
│  run_policy_check_unified → evaluate_emit_gate → repair/main/reject         │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U8: coordinator 真实 task_id                                                │
│  ralph tools task create --for-fix-unit → 返回 task_id                      │
│  preset 指令要求先创建 task 再 emit work.ready                               │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U9: 全量回归与下游同步                                                      │
│  preset_lint + SSOT + BDD scenarios + run-tests.sh                          │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Implementation Units

- [ ] U1. **plan_gate 豁免 fix-unit**

**Goal:** 让 fix-unit 完成后的 `plan.complete` 不再被 `plan_gate_review_not_terminal` 拒绝。

**Requirements:** R1

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/event_loop/review_step_state.rs`
- Test: `crates/ralph-core/src/event_loop/tests/review_step_gate.rs`

**Approach:**
- 在 `ReviewStepTracker::check_semantic_gates` 的 `plan.complete` 分支中，当匹配 step 的 `step` 字段以 `fix-` 开头时直接返回 `None`。
- 在 `observe_accepted` 处理 `review.complete` 时，如果 payload 包含非空 `fix_plan_file`，按 fix-plan 中 `### U{N}.` 的数量为每个 `fix-{NN}` step key 预填 `synth_terminal = "review.complete"` 与 `synth_pass = true`。

**Execution note:** 测试先行。先写 `plan.complete(step=fix-02)` 被拒的 failing test，再实现豁免；再写 `review.complete(fix_plan_file)` 预填状态的 failing test，再实现预填。

**Patterns to follow:**
- 参考 `review_step_state.rs` 中 `observe_accepted` 对 `review.passed` / `review.complete` 的处理。
- 参考 `review_step_gate.rs` 现有 plan_gate 测试。

**Test scenarios:**
- Happy path: `plan.complete` payload 的 `step="fix-02"` 无 review terminal 时被接受。
- Happy path: `review.complete(fix_plan_file=".../fix-plan.md")` 后，tracker 中为 `fix-01`..`fix-05` 都预填了 synth_terminal。
- Edge case: `review.complete(fix_plan_file="null")` 不预填 fix step。
- Error path: 普通 plan step 的 `plan.complete` 仍需要 `review.passed` / `review.complete`。
- Integration: 完整序列 `review.complete(fix_plan_file)` → `work.ready fix-01` → `test.passed fix-01` → ... → `plan.complete` 被接受。

**Verification:**
- 新增 `review_step_gate` 测试通过。
- 现有 `review_step_state` 测试不被破坏。

---

- [ ] U2. **recovery Final 真正终止 loop**

**Goal:** `EscalationLevel::Final` 必须导致 loop 终止，而不是只发 `human.guidance`。

**Requirements:** R4

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/diagnosis/responder.rs`
- Test: 新增 `crates/ralph-core/src/diagnosis/responder/tests.rs`（若不存在）或在 `drift_integration.rs` 中新增测试

**Approach:**
- 在 `RecoveryResponder::record_finding` 的 `EscalationLevel::Final` 分支中，把生成的 `TerminationHint` 的 severity 强制设为 `Critical`（或新增 `level` 字段并让 engine 识别）。
- 保持 `EscalationLevel::Soft` / `Hard` 的 severity 不变。

**Execution note:** 测试先行。先写 Warning severity 的 Final hint 不被 engine 终止的 failing test，再修改 severity 覆盖逻辑。

**Patterns to follow:**
- 参考 `drift/engine.rs::check_termination_hint` 的 severity 判定表。
- 参考 `responder.rs` 中 `TerminationHint` 构造位置。

**Test scenarios:**
- Happy path: `Critical` severity Final hint → `check_termination_hint` 返回 `RecoveryExhausted`。
- Edge case: `Warning` severity Final hint（当前行为）→ 修改后返回 `RecoveryExhausted`。
- Error path: 非 Final 的 `Warning` hint → 仍返回 `None`，由 `human.guidance` 路径处理。
- Edge case: `Info` severity Final hint → 同样终止（Final 即终态）。

**Verification:**
- 新增 responder / drift 测试通过。
- 现有 `drift_integration` 测试不被破坏。

---

- [ ] U3. **recovery 路由 allowed_topics 与 hat publishes 对齐**

**Goal:** 所有注入的 `task.resume` 都携带目标 hat 真实 `publishes` 列表，避免 agent 被误导去 emit 越权 topic。

**Requirements:** R3

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（fallback stall injection）
- Modify: `crates/ralph-core/src/drift/engine.rs`（`publish_hard_recovery_event`）
- Modify: `crates/ralph-core/src/event_loop/rejection.rs`（必要时新增结构化 helper）
- Test: 新增 `crates/ralph-core/src/event_loop/tests/recovery_allowed_topics.rs`

**Approach:**
- 在 fallback stall injection 中，用 `EventLoop::get_hat_publishes(target_hat)` 取得 allowed topics，通过结构化 helper 写入 `task.resume` payload 的 `allowed_topics` 字段（取代当前纯文本列举）。
- 在 `publish_hard_recovery_event` 中同样把目标 hat 的 `publishes` 注入 payload。
- 保持现有 `scope_violation` recovery envelope 的 allowed_topics 不变（它已经用 publishes）。

**Execution note:** 测试先行。构造 coordinator fallback task.resume，断言 `allowed_topics` 包含 `work.ready`；构造 hard recovery action，断言 payload 含 `allowed_topics`。

**Patterns to follow:**
- 参考 `event_loop/mod.rs:2853-2871` fallback payload 中 `publishes` 的 prose 构造。
- 参考 `rejection.rs::build_task_resume_payload` 的 JSON 结构。

**Test scenarios:**
- Happy path: coordinator 的 fallback `task.resume` 包含 `work.ready`、`review.start`、`plan.complete`、`plan.blocked`。
- Error path: coordinator 的 fallback `task.resume` 不包含 `work.start`。
- Edge path: executor 的 fallback `task.resume` 只包含 executor publishes，不含 `plan.complete`。
- Integration: hard recovery action 的 payload 是合法 JSON 且含 `allowed_topics`。

**Verification:**
- 新增 recovery allowed_topics 测试通过。
- 现有 `recovery_envelope_u7_u8`、`r5_hard_gate_routing` 测试不被破坏。

---

- [ ] U4. **U11 首次跑显式写 loop-version.json**

**Goal:** 打破首次跑的 no-op，让 `IdempotentLog::open` 之前 `loop-version.json` 已存在。

**Requirements:** R8

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`with_context_and_diagnostics` 启动路径）
- Test: `crates/ralph-core/src/event_loop/tests/u11_wiring.rs` 或 `u13_archive_fail_closed.rs`

**Approach:**
- 在 `archive_state_for_loop` 调用之后、`IdempotentLog::open` 之前，若 `loop_id` 存在且 `.ralph/loop-version.json` 不存在，写入 `{loop_id, version: 1}` 的初始文件。
- 保持 archive 失败时 fail-closed 的行为不变。

**Execution note:** 测试先行。先写 fresh workspace 启动后 `loop-version.json` 不存在的 failing test，再实现显式写入。

**Patterns to follow:**
- 参考 `state/idempotent_log.rs::open` 对 `PersistedVersion` 的序列化。
- 参考 `tests/u11_wiring.rs` 对 archive/open 顺序的断言。

**Test scenarios:**
- Happy path: fresh workspace + 有 loop_id → 启动后 `.ralph/loop-version.json` 存在，内容为 `{"loop_id":"...","version":1}`。
- Edge case: 同一 loop_id 再次启动 → version 保持为 1，不重复初始化。
- Edge case: 不同 loop_id 启动 → archive 先执行，随后 version bump。
- Error path: archive 失败 → 仍返回 Err，不写入 version 文件。

**Verification:**
- 新增/扩展 U11/U13 测试通过。
- 现有 `u13_archive_fail_closed` 测试不被破坏。

---

- [ ] U5. **U8 task 写入接入 IdempotentLog 热路径**

**Goal:** 启用 `state_idempotency: required` 时，task 保存同时写入 idempotent record，带上 `_idempotency_key` 与 `_final`。

**Requirements:** R6

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/task_store.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（把 `IdempotentLog` 句柄注册给 `TaskStore`）
- Test: 新增 `crates/ralph-core/src/event_loop/tests/u8_idempotent_task_write.rs`

**Approach:**
- 在 `TaskStore` 中增加一个可选的共享 `IdempotentLog` 句柄字段（如 `Arc<std::sync::Mutex<IdempotentLog>>`）。
- `EventLoop` 在 bootstrap 时把自己持有的 `idempotent_log` 注册到 `TaskStore`。
- 在 `TaskStore::save()` 中：先走原有 JSONL 写入；如果 log 已启用，再遍历当前 tasks，调用 `idempotent_wiring::write_task` 写入 idempotent record，terminal status 对应 `_final=true`。
- 保持 `save_with_idempotent_log` 接口供批量回填继续使用。

**Execution note:** 测试先行。先写启用 idempotent log 后 task 保存不产生 `_idempotency_key` 的 failing test，再实现热路径写入。

**Patterns to follow:**
- 参考 `task_store.rs:141-185` 的 `save_with_idempotent_log`。
- 参考 `idempotent_wiring.rs::write_task`。

**Test scenarios:**
- Happy path: 启用 idempotent log，保存非 terminal task → idempotent record 含 `_idempotency_key` 且 `_final=false`。
- Happy path: task 状态变为 closed → 再次 save 后 `_final=true`。
- Edge case: `TaskStore` 未注册 idempotent log → 只写 JSONL，不 panic。
- Error path: task 无 loop_id → `write_task` 返回 `MissingLoopId`，JSONL 写入仍成功。
- Edge case: 重复 save 同一 task → idempotent log 去重，不会重复记录。

**Verification:**
- 新增 U8 热路径测试通过。
- 现有 `u8_legacy_relocate_and_close` 与 `task_store` 测试不被破坏。

---

- [ ] U6. **U12 step-close 从 fix-unit task 计数推导 total**

**Goal:** 未声明 `total_units` 的 fix step 不再 fail-open，`StepCloseObligationStage` 能正常拦截 premature emit。

**Requirements:** R7

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`flow_step_total_units`）
- Test: 新增 `crates/ralph-core/src/event_loop/tests/u12_step_close.rs`

**Approach:**
- 在 `flow_step_total_units` 中：若配置声明了 `total_units` 直接返回；否则若 `step_id` 以 `fix-` 开头，从 `TaskStore` 中统计匹配 `ce-executor:*:fix-*` 的 task 数量作为 `total`。
- 保持非 fix step 未声明 `total_units` 时的 no-op 行为，避免误伤其他 preset。

**Execution note:** 测试先行。先写 fix step 未声明 total_units 时 `drive_step_close_progress` 仍不推进的 failing test，再实现 fallback。

**Patterns to follow:**
- 参考 `event_loop/mod.rs:9118-9144` 的 `drive_step_close_progress`。
- 参考 `TaskStore` 的 task iteration API。

**Test scenarios:**
- Happy path: 5 个 fix-unit tasks 存在，未声明 total_units；连续 5 次 `work.done` 后 step-close 义务满足。
- Error path: 只完成 4 个 fix-unit 就 emit 非 `on_partial` topic → `step_close_obligation_violated` 被触发。
- Edge case: 非 fix step 未声明 total_units → `StepCloseObligationStage` 保持 no-op。
- Edge case: 配置显式声明 total_units=3，但 tasks.jsonl 有 5 个 fix-unit task → 以显式声明的 3 为准。

**Verification:**
- 新增 U12 测试通过。
- 现有 `workflow_guard`、`u6_wiring` 测试不被破坏。

---

- [ ] U7. **CLI emit 路径接入 stage_pipeline**

**Goal:** `ralph emit` 不再绕开 U6/U7/U9/U9.5/U12，reject 事件进入 recovery 流。

**Requirements:** R5

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-cli/src/policy_check.rs`（`run_policy_check_unified`）
- Modify: 必要时 `crates/ralph-cli/src/wave.rs`（wave emit 入口）
- Test: `crates/ralph-cli/src/policy_check.rs` 内联测试 或 新增 `crates/ralph-cli/src/policy_check/tests.rs`

**Approach:**
- 在 `run_policy_check_unified` 中，legacy terminal gate 通过后，构造 `StagePipeline` 与 `StageContext`，调用 `ralph_core::event_loop::emit_gate::evaluate_emit_gate`。
- 根据 outcome 生成 `PolicyCheckReport`：
  - `AcceptMainBus` → 允许。
  - `AcceptRepairStream` / `Reject` → 阻止，并写 `recovery.jsonl` repair envelope。
- 保持现有 reason code 与 CLI 输出格式兼容。

**Execution note:** 测试先行。先写 CLI emit 缺 required field 不触发 `missing_required_fields` 的 failing test，再接入 stage_pipeline。

**Patterns to follow:**
- 参考 `event_loop/mod.rs:9579-9607` 对 `evaluate_emit_gate` outcome 的路由。
- 参考 `repair_stream_sink::record_repair_event`。

**Test scenarios:**
- Happy path: `ralph emit LOOP_COMPLETE` → terminal gate + stage gate 都通过 → accepted。
- Error path: `ralph emit work.ready` payload 缺 required field → stage reject，report 含 `missing_required_fields`，`recovery.jsonl` 写入 `repair_dispatch` envelope。
- Error path: `ralph emit task.relocate_legacy` → routed to repair stream，不进入 bus。
- Edge path: partial state 下 emit 非 `on_partial` topic → `step_close_obligation_violated`。
- Edge path: legacy terminal gate 已 reject → 不再跑 stage_pipeline，保持原 reason code。

**Verification:**
- 新增 policy_check 测试通过。
- 现有 `policy_check::u6_unified_path_tests` 不被破坏。

---

- [ ] U8. **coordinator 用真实 task_id 派发 fix-unit**

**Goal:** 消灭 `task-fix-01-placeholder`，让每个 fix-unit 有独立、合法的 task 记录。

**Requirements:** R2

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-cli/src/task_cli.rs`（新增 `--for-fix-unit` 路径）
- Modify: `presets/en/ce-executor-serial.yml`（coordinator fix-unit 派发 instructions）
- Test: `crates/ralph-cli/src/task_cli.rs` 内联测试

**Approach:**
- 在 `ralph tools task create` 中支持 `--for-fix-unit <plan_name>:<fix_step>:<slug>`（或等效参数），生成：
  - `task_key = ce-executor:{plan_name}:{fix_step}:{slug}`
  - `owner_hat_id = coordinator`
  - `loop_id = current loop`
- coordinator 在 `review.complete(fix_plan_file)` 触发后，对每个 `U{N}` 先调用该命令创建 task，再 emit 第一个 fix-unit 的 `work.ready`；后续 fix-unit 在 `test.passed` 触发时也先创建下一 task 再 emit。
- 在 event_policy 或 execution contract 中增加 guard：任何事件若 `task_id` 以 `-placeholder` 结尾，直接 reject。

**Execution note:** 测试先行。先写 placeholder task_id 被接受的 failing test，再实现 guard；再写 `--for-fix-unit` 创建 task 的 failing test。

**Patterns to follow:**
- 参考 `task_cli.rs` 现有 `create` 命令与 owner/loop_id 写入逻辑。
- 参考 `execution_contract.rs` 对 `TaskWrongLoop` 的检查风格。

**Test scenarios:**
- Happy path: `ralph tools task create --for-fix-unit myplan:fix-02:patch-foo` 创建 task，key 格式正确，owner=coordinator。
- Error path: `work.done` payload 的 `task_id="task-fix-01-placeholder"` → contract/policy reject。
- Happy path: coordinator `work.ready fix-02` 使用 `fix-02` 的真实 task_id 时，`work.done` 不触发 `TaskWrongLoop`。
- Edge case: `--for-fix-unit` 在 agent context 无 loop_id 时返回错误。

**Verification:**
- 新增 task_cli 测试通过。
- preset_lint 通过；SSOT byte-equality 测试通过。

---

- [ ] U9. **全量回归与下游同步**

**Goal:** 确保 8 个 Unit 合在一起不引入回归，preset/schema/config 保持一致。

**Requirements:** R1-R8

**Dependencies:** U1, U2, U3, U4, U5, U6, U7, U8

**Files:**
- 可能修改：`presets/schemas/ce-executor-serial.yml`、`crates/ralph-cli/src/presets.rs`、`crates/ralph-cli/src/preflight.rs`、`crates/ralph-cli/src/config_resolution.rs`、`crates/ralph-core/tests/scenarios/ce_executor_serial_*.yml`、`AGENTS.md` / `CLAUDE.md`、`scripts/ralph-zsh-plugin.zsh`
- 验证入口：`./scripts/run-tests.sh`

**Approach:**
- 检查 U1-U8 是否引入新的 event_loop 配置字段或 lint finding；若有，按 `AGENTS.md` 下游同步清单更新。
- 若 preset 内容因 U8 变化，同步 `crates/ralph-cli/src/presets.rs`、`presets/manifest.yml`、`presets/index.json`、zsh 补全。
- 按 `AGENTS.md` 的校验链运行全量回归：`preset_lint`（ralph-cli + ralph-core）、SSOT byte-equality、`ce_executor_serial` BDD scenarios、`run-tests.sh`。

**Execution note:** 本 Unit 不写新功能代码，只做同步与测试。

**Patterns to follow:**
- 参考 `AGENTS.md`「preset/schema 改动后的下游同步清单」7 步。
- 参考 `.cursor/rules/multi-hat-isolation.mdc` 的 preset 同步规则。

**Test scenarios:**
- Happy path: `preset_lint`（ralph-cli + ralph-core）全部通过。
- Happy path: SSOT byte-equality 测试通过。
- Happy path: `ce_executor_serial` BDD scenarios 通过。
- Happy path: `./scripts/run-tests.sh` 全绿。

**Verification:**
- `./scripts/run-tests.sh` 返回全部通过。

---

## System-Wide Impact

- **Interaction graph:**
  - `review_step_state` 的 fix-unit 豁免影响 `plan.complete` 准入与 coordinator 死锁路径。
  - `responder` / `drift_engine` 的 Final 终止影响 loop 生命周期。
  - `task.resume` 的 `allowed_topics` 影响 agent 重试行为与 isolated scope 判定。
  - `TaskStore` 与 `IdempotentLog` 的联动影响 task 持久化与 U11/U13。
  - `policy_check` 接入 stage_pipeline 让 CLI emit 与 loop 内 emit 共享同一套 gate。
  - fix-unit task 创建方式改变 coordinator preset instructions。
- **Error propagation:**
  - fix-unit `plan.complete` 不再被错误拒绝，避免 coordinator 走 `plan.blocked`。
  - placeholder task_id 被显式拒绝，避免 `TaskWrongLoop` 级联。
  - recovery Final 直接生成 `TerminationReason::RecoveryExhausted`，runner 退出非零。
- **State lifecycle risks:**
  - `loop-version.json` 首次跑写入后，后续 resume/archive 行为可预测。
  - idempotent task record 与 JSONL 双写，需保证 JSONL 仍是主源，idempotent record 是幂等索引。
- **API surface parity:**
  - CLI `ralph tools task create` 新增 `--for-fix-unit` 语义；无破坏现有调用。
  - `PolicyCheckReport` 的 shape 保持兼容，新增 repair-stream 路径对下游透明。
- **Unchanged invariants:**
  - isolated mode 下 hat publishes 仍是 scope 唯一事实源。
  - `LOOP_COMPLETE` 仍是唯一默认终态事件。
  - `ralph-cli` 测试仍走 nextest 串行。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| fix-unit 豁免引入非 fix step 的误放行 | 仅在 `step` 以 `fix-` 前缀时豁免，并保留现有 review terminal 检查。 |
| Final severity改Critical影响其他 recovery 路径 | 只影响 `EscalationLevel::Final` 分支；Soft/Hard 不变。 |
| allowed_topics 用 publishes 导致某些 fallback 路径信息过载 | 与 preset 协调一致；publishes 是 agent 合法 emit 集合。 |
| TaskStore 双写拖慢性能 | idempotent log 是 opt-in；未启用时完全走原路径。 |
| CLI stage_pipeline 接入改变现有 CLI 行为 | 通过 policy_check 测试锁定；reject 事件写入 recovery 流是预期行为。 |
| `--for-fix-unit` CLI 形态与现有参数冲突 | 实现时与现有 clap 定义对齐，并补充内联测试。 |
| preset 内容变化导致 lint/SSOT/scenarios 失败 | U9 专门处理下游同步。 |

---

## Documentation / Operational Notes

- 若 `AGENTS.md` / `CLAUDE.md` 因 preset 或 builtin 列表变化，必须 `cp CLAUDE.md AGENTS.md` 同步。
- 更新 `crates/ralph-core/data/ralph-tools-tasks.md` 中 `ralph tools task create` 的用法说明（新增 `--for-fix-unit`）。
- 更新 `docs/solutions/integration-issues/` 下相关 solution 文档，记录本次 fix-unit 死锁与 recovery 未终止的根因。

---

## Sources & References

- **Origin document:** `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md`
- **Previous related plan:** `docs/plans/2026-06-26-001-fix-ce-executor-serial-four-recurrences-plan.md`
- **Code references:**
  - `crates/ralph-core/src/event_loop/review_step_state.rs`
  - `crates/ralph-core/src/diagnosis/responder.rs`
  - `crates/ralph-core/src/drift/engine.rs`
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/task_store.rs`
  - `crates/ralph-core/src/state/idempotent_log.rs`
  - `crates/ralph-cli/src/policy_check.rs`
  - `crates/ralph-cli/src/task_cli.rs`
  - `presets/en/ce-executor-serial.yml`
