---
title: "fix: ce-executor-serial primary-20260628-115810 全量 P0/P1/P2 修复"
type: fix
status: active
date: 2026-06-28
origin: docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md
---

# fix: ce-executor-serial primary-20260628-115810 全量 P0/P1/P2 修复

## Overview

基于 `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md` 的诊断结论，本次修复覆盖该报告中列出的全部 P0/P1/P2 问题：

- **P0 基座机制**: `FlowStepScopeStage` 误拒 `review.dimensions.complete`、漂移检测误报、修复机制无自我终止、`execution_contract` 空 `task_id` 误拒等。
- **P0 plan 模式治本**: `IdempotentLog` 生产路径失效、`RepairStateMachine` 未在热路径驱动、`CLI emit` 绕开 `stage_pipeline`、metadata 与 runtime 行为漂移。
- **P1/P2 产物接管**: `human.guidance` 清理、`projector` 接管 plan frontmatter 与 `progress.md`。

执行方式：**16 个 Implementation Unit 严格串行、绝对隔离、TDD 闭环**。每个 Unit 必须先写验收测试，测试只验证当前 Unit 的输入输出，100% 通过后才能进入下一 Unit。

---

## Problem Frame

在无人工介入的运行模型下（无 Telegram/Slack/Webhook/Email/IM），`ce-executor-serial` 的 `primary-20260628-115810` run 出现结构性失败：`LOOP_COMPLETE` 未触发，修复机制反复震荡 14+ 次，TUI 超时退出。

> **产品前提确认**：`ce-executor-serial` 在本次运行中被配置为纯自动化执行（无外部人工介入通道），诊断报告 §0 已明确此约束。本计划据此将 `human.guidance` 降级为噪音并赋予系统自我终止能力。若未来该 preset 需要支持人工介入，需重新评估 R10/R13。

核心问题分层：

1. **基座机制缺陷（~40%）**: `FlowStepScopeStage` 过度严格、`drift_monitor` 计算口径与自观测错误、`stall_recovery` 只升级不终止、`execution_contract` 不回填空 `task_id`。
2. **plan 模式缺陷（~25%）**: `2026-06-27-001` 落地的 U2/U7/U8 单元测试通过但热路径未驱动；U9 `FlowStepScopeStage` 变硬后反而成为新卡点；CLI 路径绕开 stage_pipeline；metadata 与 runtime 两层皮。
3. **preset 设计缺陷（~15%）**: `human.guidance` 无消费者却被保留；`coordinator`/`ralph` 在卡住时无法直接 emit 终态。
4. **agent 执行问题（~10%）**: executor 漏填 `task_id`、dimension-reviewer 违规写 plan status、agent 不写 `progress.md`。
5. **多因素叠加（~10%）**: 上述问题互锁放大。

---

## Requirements Trace

- **R1 — FlowStepScope 放行 review chain**: `review-coordinator` / `review-synthesizer` / `dimension-reviewer` 在合理场景下 emit `review.*` 话题时不应被 `flow_unknown_emit` 拒绝。
- **R2 — flow_lifecycle 真实推进**: `review.start` accept 后 `current_step_id` 必须切换到 `review_walk`；`plan.complete` / `REVIEW_COMPLETE` accept 后切换到 `plan_end` / `ship`。
- **R3 — drift field_completeness 低样本不告警**: 窗口事件数小于 `FIELD_COMPLETENESS_MIN_SAMPLES` 时不产生 finding。
- **R4 — drift 排除自观测**: `reason_code == "recovery_outcome_update"` 的事件不进入 drift 观测窗口。
- **R5 — stall_recovery 自我终止**: `stall_recovery_counts` 达到最终阈值后必须 emit `plan.blocked(reason="stall_recovery_exhausted")`，不再 escalate。
- **R6 — RepairStateMachine 在热路径被驱动**: 每次 stall escalation 消费 1 个 repair budget，budget 耗尽直接 emit `plan.blocked`。
- **R7 — IdempotentLog 失败即暴露**: `IdempotentLog::open` 失败必须 panic，禁止 silent fallback 到 `disabled()`。
- **R8 — execution_contract task_id fallback**: `work.done` 的 `task_id` 为空时，从 `loop_state.active_tasks` 按 `task_key` 回填；回填失败再 reject。
- **R9 — 通用 RecoveryFinalizer**: 所有"提醒型"机制（stall/drift/repair）超过 `max_escalation_count` 后统一 emit 终态事件。
- **R10 — ralph/coordinator 在无人工时能自我终止**: isolated 模式下 `ralph` 可 emit `plan.blocked` / `LOOP_COMPLETE(success=false)`；`coordinator` 可 emit `LOOP_COMPLETE(success=false)`。
- **R11 — CLI emit 走 stage_pipeline**: `run_policy_check_unified` 必须调用 `evaluate_emit_gate`，reject 事件进入 recovery 流。
- **R12 — metadata-runtime 一致性 CI**: preset 声明 `state_idempotency: required` / `repair_budget: N` / `enforce_schema: hard` 必须与运行时实际行为一致，不一致 fail-closed。
- **R13 — 清理 human.guidance**: 在 `ce-executor-serial` preset 中禁用 `human.guidance` 话题，drift 不再观测其字段缺失。
- **R14 — projector 接管 plan status**: `dimension-reviewer` 不再写 plan frontmatter status；`test.passed` 后 projector 统一写 `status: u{N}-closed-u{N+1}-pending`。
- **R15 — projector 接管 progress.md**: `work.done` accept 后 projector 写 `.agents/scratchpad/ce-executor/{plan_name}/progress.md` 的 step 勾选。

---

## Scope Boundaries

- **本次必须修复诊断报告列出的全部 P0/P1/P2 问题**，让 `ce-executor-serial` fix-unit 链路在无人工介入下能自己跑通或自己承认失败。
- 不新增 hat、不新增 event topic（除 `plan.blocked` / `LOOP_COMPLETE` 终态话题已在系统中存在）。
- 不改 isolated 执行模式、不改 wave 协议、不改 review dimension 协议。
- 不推广到其他 builtin preset；其他 preset 的同步作为 follow-up。

### Deferred to Follow-Up Work

- 把 `metadata_runtime_drift` lint 推广到所有 builtin preset。
- 把 `RecoveryFinalizer` 终态话题可配置能力推广到 `event_loop.recovery.final_outcome_topic`。
- 长期删除 `human.guidance` topic 的基座支持（本次只在 `ce-executor-serial` preset 层禁用）。

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs` — `FlowStepScopeStage::check` 硬拒逻辑。
- `crates/ralph-core/src/flow_lifecycle.rs` — `current_step_id()` 占位实现。
- `crates/ralph-core/src/drift/detector.rs` — `observe()` / `check_field_completeness()`。
- `crates/ralph-core/src/drift/engine.rs` — `build_outcome_envelope()` / `check_termination_hint()`。
- `crates/ralph-core/src/event_loop/mod.rs` — `inject_fallback_event()`、`stall_recovery_counts`、ralph hat scope、`IdempotentLog::open` fallback。
- `crates/ralph-core/src/diagnosis/responder.rs` — `EscalationLevel::Final` / `TerminationHint`。
- `crates/ralph-core/src/event_loop/repair_flow.rs` — `RepairStateMachine` / `RepairBudget`。
- `crates/ralph-core/src/execution_contract.rs` — `work.done` `task_id` 校验。
- `crates/ralph-core/src/state_projector/task.rs` — `ensure_task()` 优先采纳 payload `task_id`。
- `crates/ralph-cli/src/policy_check.rs` — `run_policy_check_unified()` 仍走 `ValidationPipeline`。
- `crates/ralph-cli/src/commands/emit.rs` — CLI emit 入口。
- `crates/ralph-core/src/event_loop/stage_pipeline.rs` / `emit_gate.rs` — `evaluate_emit_gate` facade。
- `presets/en/ce-executor-serial.yml` — coordinator/ralph `publishes`、`topic_deny_rules`、`mechanism.flow`。
- `crates/ralph-core/src/preset_lint/` — lint 框架，用于新增 `metadata_runtime_drift`。

### Institutional Learnings

- `AGENTS.md` — preset/schema 改动后必须同步 7 处下游；`CLAUDE.md` 与 `AGENTS.md` 必须 `cp` 同步。
- `.cursor/rules/multi-hat-isolation.mdc` — isolated 模式下 hat `publishes` 是 scope 唯一事实源；4+ hats 必须 `execution_mode: isolated`。
- `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` — 3 道防线（lint / runtime recovery / verdict-gate fail-back）。
- `docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md` — `ralph-cli` 测试必须走 `cargo nextest run` 串行。

### External References

- 无外部依赖；所有模式均来自本地代码与历史 solutions。

---

## Key Technical Decisions

1. **串行优先于并行**: 16 个 Unit 严格线性推进（U1 → U2 → ... → U16），即使部分 Unit 理论上可并行，也按"一个接着一个"执行，确保每个闭环都完整。
2. **绝对隔离（函数级）**: 每个 Unit 只改最小函数/逻辑集合；因 `event_loop/mod.rs` 是编排中心，多个 Unit 会触及同一文件的不同函数，但绝不相互依赖内部实现。后置 Unit 在前置 Unit 完成后基于已验证的公开行为继续。
3. **测试先行且只测当前 Unit**: 每个 Unit 的验收测试只断言该 Unit 的输入输出，不写跨 Unit 集成测试；集成验证只在 U16 进行。
4. **失败即终止**: `IdempotentLog::open` 失败从 silent fallback 改为 panic，让"required"真正成为硬约束。
5. **publishes 是 recovery 的唯一事实源**: 注入的 `task.resume` 必须携带目标 hat 的 `publishes` 列表。
6. **stage_pipeline 统一 CLI 与 loop**: `run_policy_check_unified` 必须复用 `evaluate_emit_gate` facade，消灭"幽灵路径"。
7. **human.guidance 在 serial preset 中禁用**: 在无人工介入模型下，该 topic 只会制造噪音与误报。
8. **共享文件集成检查点**: 每完成一个修改 `event_loop/mod.rs` 的 Unit（U4/U6/U7/U8/U10/U14/U15），额外运行一次最小集成测试，确保该文件当前所有改动协同工作；不把全部集成风险押在 U16。

---

## Open Questions

### Resolved During Planning

- **是否按诊断报告 15 个修复项一一对应 Unit？** 是。每个根因一个 Unit，确保原子化与可追溯。
- **U3/U4 都改 flow，是否要合并？** 不合并。U3 加临时 defensive bypass 放行 transition / 终态事件；U4 在 EventLoop 中实现真正的 `current_plan_step` 推进。U4 完成后大部分 bypass 不再触发，但保留作为安全网。
- **U6/U7 都改 `event_loop/mod.rs`，是否隔离？** U6 只动 `inject_fallback_event` 里的 RepairStateMachine 驱动；U7 只动构造函数里的 `IdempotentLog::open` fallback；两者作用于不同函数，互不干扰。
- **P2-1/P2-2 是否必须纳入？** 是。诊断报告把它们列为 P1/P2，且是 agent 执行问题的机械化收口，必须纳入。

### Deferred to Implementation

- `RecoveryFinalizer` 的具体 struct 命名与方法签名实现时确定。
- `flow_lifecycle.rs` 中 `transition_to_plan_end` / `transition_to_ship` 的触发点实现时与 event accept 代码对齐。
- `metadata_runtime_drift` lint 的 exact error message 实现时与现有 `Finding` 格式对齐。

---

## High-Level Technical Design

> *本节说明改动形状，不是可复制粘贴的实现规范。*

```text
U1  drift field_completeness min_samples
 |
 v
U2  drift recovery_outcome_update 自观测排除
 |
 v
U3  FlowStepScopeStage review-chain defensive bypass
 |
 v
U4  flow_lifecycle current_step 真实推进
 |
 v
U5  execution_contract task_id fallback
 |
 v
U6  RepairStateMachine 在 stall_recovery 热路径驱动
 |
 v
U7  IdempotentLog::open 失败 panic（不再 fallback disabled）
 |
 v
U8  stall_recovery 最终阈值 → plan.blocked
 |
 v
U9  RecoveryFinalizer 通用兜底（drift/repair 接入）
 |
 v
U10 ralph/coordinator 在 isolated 模式可 emit 终态
 |
 v
U11 CLI emit 路径接入 stage_pipeline
 |
 v
U12 metadata-runtime drift lint
 |
 v
U13 禁用 human.guidance
 |
 v
U14 projector 接管 plan frontmatter status
 |
 v
U15 projector 接管 progress.md
 |
 v
U16 全量回归与下游同步
```

---

## Implementation Units

- [ ] U1. **drift field_completeness 低样本不告警**

**Goal:** 当 drift 观测窗口事件数不足时，`field_completeness` 不产生 critical finding，消除 iter=5 的 `0/1` 误报风暴。

**Requirements:** R3

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/drift/detector.rs`
- Test: `crates/ralph-core/src/drift/detector.rs` 内联测试 或 `crates/ralph-core/src/drift/tests.rs`

**Approach:**
- 新增常量 `FIELD_COMPLETENESS_MIN_SAMPLES: usize = 5`。
- 在 `check_field_completeness` 入口加入守卫：`if window.len() < FIELD_COMPLETENESS_MIN_SAMPLES { return Vec::new(); }`。

**Execution note:** 测试先行。先写 window_size=1 且字段缺失时仍产生 finding 的 failing test，再实现 min_samples 守卫。

**Patterns to follow:**
- 参考 `drift/detector.rs` 中 `emit_cadence` 的 `EMIT_CADENCE_MIN_SAMPLES` 用法。

**Test scenarios:**
- Edge case: window 仅 1 个事件且字段缺失 → 不产生 finding。
- Edge case: window 仅 4 个事件且字段缺失 → 不产生 finding。
- Happy path: window 5 个事件且字段缺失率 > 0.85 → 产生 critical finding。
- Happy path: window 5 个事件且字段缺失率 ≤ 0.85 → 不产生 finding。

**Verification:**
- 新增 detector 测试通过。
- 现有 `drift` 测试不被破坏。

---

- [ ] U2. **drift 排除 recovery_outcome_update 自观测**

**Goal:** 阻止 drift engine 把自己写入的 `recovery_outcome_update` envelope 重新喂回观测窗口，消除 Pending↔Recovered 12+ 次震荡。

**Requirements:** R4

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/src/drift/detector.rs`
- Test: `crates/ralph-core/src/drift/detector.rs` 内联测试 或 `crates/ralph-core/src/drift/tests.rs`

**Approach:**
- 在 `DriftDetector::observe` 入口加入过滤：`if snapshot.reason_code.as_deref() == Some("recovery_outcome_update") { return Vec::new(); }`。

**Execution note:** 测试先行。先写 `recovery_outcome_update` 事件进入窗口并影响 outcome 的 failing test，再实现过滤。

**Patterns to follow:**
- 参考 `drift/detector.rs` 中对 `topic` / `hat` 的过滤写法。

**Test scenarios:**
- Edge case: `reason_code="recovery_outcome_update"` 的事件 → 不进入任何窗口。
- Happy path: 普通 `work.done` 事件 → 正常进入窗口。
- Edge case: `reason_code="recovery_outcome_update"` 的多个事件 → 都不进入窗口，outcome 不再翻转。
- Integration: 模拟 drift engine 写 outcome envelope 后立即 observe → 窗口长度不变。

**Verification:**
- 新增 detector 测试通过。
- 现有 `drift_integration` 测试不被破坏。

---

- [ ] U3. **FlowStepScopeStage review-chain defensive bypass**

**Goal:** 当 `flow_lifecycle.current_step_id` 尚未推进时，`review-coordinator` / `review-synthesizer` / `dimension-reviewer` emit 合规的 `review.*` 话题不被 `flow_unknown_emit` 拒绝。

**Requirements:** R1

**Dependencies:** U2

**Files:**
- Modify: `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs`
- Test: `crates/ralph-core/src/event_loop/tests/flow_step_scope.rs`

**Approach:**
- 在 `FlowStepScopeStage::check` 中，以下事件直接 `return Ok(Accept)`（这些是过渡事件或修复机制自我终止事件，在 U4 的 current_step 状态机完善前必须放行）：
  - `coordinator` emit `review.start` / `plan.complete`
  - `review-coordinator` emit `review.dimension.ready` / `review.dimensions.complete`
  - `review-synthesizer` emit `review.complete`
  - `dimension-reviewer` emit `review.dimension.done` / `review.dimension.failed`
  - `shipper` emit `REVIEW_COMPLETE`
  - `ralph` / `coordinator` emit `plan.blocked` / `LOOP_COMPLETE(success=false)`（自我终止）
- 这些 bypass 是临时防御性措施；U4 完成后，如果 current_step 推进正确，大部分 bypass 会自然不再触发，但保留以防止状态机遗漏。

**Execution note:** 测试先行。先写 `review.dimensions.complete` 在 `unit_loop` context 下被 reject 的 failing test，再实现 bypass。

**Patterns to follow:**
- 参考 `emit_schema_gate_stage.rs` 对 topic 的 early-return 模式。

**Test scenarios:**
- Happy path: `coordinator` emit `review.start` / `plan.complete` → accepted。
- Happy path: `review-coordinator` emit `review.dimensions.complete` → accepted。
- Happy path: `review-synthesizer` emit `review.complete` → accepted。
- Happy path: `dimension-reviewer` emit `review.dimension.done` → accepted。
- Happy path: `shipper` emit `REVIEW_COMPLETE` → accepted。
- Happy path: `ralph` emit `plan.blocked` / `LOOP_COMPLETE(success=false)` → accepted。
- Error path: `executor` emit `review.dimensions.complete` → 仍被 `flow_unknown_emit` reject。
- Error path: `review-coordinator` emit `work.done` → 仍被 reject。

**Verification:**
- 新增 FlowStepScope 测试通过。
- 现有 `stage_pipeline_order_*` 测试不被破坏。

---

- [ ] U4. **flow_lifecycle current_step 真实推进**

**Goal:** 让 `flow_lifecycle` 从占位实现变成真正的状态机：关键终端事件 accept 后推进当前 step，从根本上解决 review-chain 的 step 上下文问题。

**Requirements:** R2

**Dependencies:** U3

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（维护 current_plan_step 并构造 StageContext）
- Modify: `presets/en/ce-executor-serial.yml`（必要时在 allowed_emits 中加入 transition events）
- Modify: `presets/schemas/ce-executor-serial.yml`（同步 flow schema）
- Test: `crates/ralph-core/src/event_loop/tests/flow_lifecycle.rs`

**Approach:**
- 在 `EventLoop` 中新增 `current_plan_step: String` 字段，初始值为 `FlowDeclaration.steps[0].id`（即 `unit_loop`）。
- 在 `event_loop/mod.rs` 事件被 stage pipeline accept 后，根据 `FlowDeclaration` 的 transition 规则更新 `current_plan_step`：
  - `unit_loop` 满足 `terminal_when: all_done` 条件（所有 plan unit 完成）→ 进入 `review_walk`。
  - `review_walk` 收到 `review.complete` 且所有维度完成 → 进入 `plan_end`。
  - `plan_end` 收到 `plan.complete` → 进入 `ship`。
  - `ship` 收到 `REVIEW_COMPLETE` → 保持 `ship`。
- 用 `FlowStep::new(current_plan_step)` 构造 `StageContext.current_step`，供 `FlowStepScopeStage` 查询 `allowed_emits`。
- 若 `FlowDeclaration` 的 `allowed_emits` 未包含上述 transition 事件，同步更新 `presets/en/ce-executor-serial.yml` 的 `mechanism.flow`，在源 step 中加入 transition events（如 `unit_loop.allowed_emits` 加入 `review.start`，`review_walk.allowed_emits` 加入 `plan.complete`，`plan_end.allowed_emits` 加入 `REVIEW_COMPLETE`）。
- 保持 `flow_lifecycle.rs` 的 `current_step_id()` 作为独立 wave 生命周期接口，不混入 plan-step 逻辑。

**Execution note:** 测试先行。先写 `review.start` accept 后 `current_step_id()` 仍为 `unit_loop` 的 failing test，再实现 transition。

**Patterns to follow:**
- 参考 `flow_declaration.rs` 对 declared steps 的解析结构。

**Test scenarios:**
- Happy path: `unit_loop` 满足 terminal 条件后，`review.start` accept → `current_plan_step` 变为 `review_walk`。
- Happy path: `review.complete` accept（所有维度完成）→ `current_plan_step` 变为 `plan_end`。
- Happy path: `plan.complete` accept → `current_plan_step` 变为 `ship`。
- Happy path: `REVIEW_COMPLETE` accept → `current_plan_step` 保持 `ship`。
- Edge case: 未声明 `mechanism.flow` → `current_plan_step` 始终为 `""`，transition 是 no-op。
- Edge case: 重复 accept `review.start` → 不 panic，保持 `review_walk`。

**Verification:**
- 新增 flow_lifecycle 测试通过。
- 现有 `flow_declaration` 测试不被破坏。

---

- [ ] U5. **execution_contract task_id fallback**

**Goal:** agent 漏填 `task_id` 时，`work.done` 不被反复 reject，而是从 `loop_state.active_tasks` 按 `task_key` 自动回填。

**Requirements:** R8

**Dependencies:** U4

**Files:**
- Modify: `crates/ralph-core/src/execution_contract.rs`
- Modify: `crates/ralph-core/src/state_projector/task.rs`
- Test: `crates/ralph-core/src/execution_contract/tests.rs`（或内联测试）

**Approach:**
- 在 `execution_contract.rs` 的 `work.done` `task_id` 校验前：
  1. 若 `task_id` 以 `-placeholder` 结尾 → 直接 reject（禁止占位 task_id）。
  2. 若 `task_id` 为空/缺失 → 从 `loop_state.active_tasks` 中按 `task_key` 查找 active task 并回填 payload。
- 在 `state_projector/task.rs` 的 `ensure_task()` 中，若 payload `task_id` 为空字符串，忽略该字段，改用从 `task_key` 生成的 id。
- 回填失败时仍返回 `InvalidPayload`，但 error message 包含 hint。

**Execution note:** 测试先行。先写 `work.done` 空 `task_id` 被 reject 的 failing test，再实现 fallback。

**Patterns to follow:**
- 参考 `execution_contract.rs` 中 `relocate_legacy_tasks` 对 `loop_state` 的访问方式。
- 参考 `task_store.rs` 中按 `task_key` 索引 task 的模式。

**Test scenarios:**
- Happy path: `work.done` payload `task_id=""` 但 `active_tasks` 有匹配 `task_key` → accept，内部 task_id 被回填。
- Happy path: `work.done` payload 无 `task_id` 字段但 `active_tasks` 有匹配 → accept。
- Error path: `task_id=""` 且 `active_tasks` 无匹配 → reject，reason 含 `task_id_fallback_failed`。
- Edge case: payload 有非空 `task_id` → 保持原行为，不 fallback。
- Error path: `task_id` 以 `-placeholder` 结尾 → 直接 reject，不走 fallback。

**Verification:**
- 新增 execution_contract 测试通过。
- 现有 `execution_contract` 与 `state_projector` 测试不被破坏。

---

- [ ] U6. **RepairStateMachine 在 stall_recovery 热路径驱动**

**Goal:** 让 `repair_budget: 3` 不再是 metadata 装饰，每次 stall escalation 真实消费 budget，budget 耗尽直接触发终止。

**Requirements:** R6

**Dependencies:** U5

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`inject_fallback_event`）
- Modify: `crates/ralph-core/src/event_loop/repair_flow.rs`（必要时暴露查询接口）
- Test: `crates/ralph-core/src/event_loop/tests/repair_state_machine_hot_path.rs`

**Approach:**
- 在 `inject_fallback_event` 中，按 `task_key` 从 `repair_state_machines` 取或创建 `RepairStateMachine`（budget 来自 preset `mechanism.repair_budget`，默认 3）。
- 第一次 escalation：调用 `try_transition(RepairAction::BeginDiagnosis)`。
- 后续 escalation：调用 `try_transition(RepairAction::Retry)`。
- 若返回 `RepairTransitionResult::BudgetExhausted(BudgetExhausted)`，emit `plan.blocked(reason="repair_unrecoverable_after_N_retries", task_key=...)` 并返回。
- 若该 `task_key` 后续收到 `work.done`（任务恢复），调用 `try_transition(RepairAction::Close)` 重置 machine，避免无关 stall 提前耗尽 budget。

**Execution note:** 测试先行。先写 3 次 escalation 不消耗 budget 的 failing test，再实现 state machine 驱动。

**Patterns to follow:**
- 参考 `repair_flow.rs` 现有 `RepairStateMachine` 单元测试。
- 参考 `event_loop/mod.rs` 中 `stall_recovery_counts` 的 HashMap 用法。

**Test scenarios:**
- Happy path: 3 次 Retry 后 `try_transition` 返回 `BudgetExhausted`。
- Error path: budget 耗尽后再次 escalation 直接 emit `plan.blocked`，不增加计数。
- Happy path: 非 stall 路径不触及 RepairStateMachine。
- Edge case: 不同 `task_key` 有独立 budget。
- Edge case: 该 task_key 收到 `work.done` 后 Close 重置，后续 stall 重新计数。

**Verification:**
- 新增 repair hot path 测试通过。
- 现有 `repair_flow` 单元测试不被破坏。

---

- [ ] U7. **IdempotentLog::open 失败 panic**

**Goal:** 让 `state_idempotency: required` 成为硬约束，`IdempotentLog::open` 失败时立即 panic 而不是 silent fallback 到 disabled。

**Requirements:** R7

**Dependencies:** U6

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`with_context_and_diagnostics` 构造函数）
- Test: `crates/ralph-core/src/event_loop/tests/idempotent_log_bootstrap.rs`

**Approach:**
- 删除 `IdempotentLog::open` 失败后的 fallback 分支与 `"P0-2: ... falling back to disabled log"` 注释。
- 分三种分支处理：
  1. `state_idempotency: disabled` → 使用 `IdempotentLog::disabled()`。
  2. `state_idempotency: required` 且 `loop_id` 存在 → 调用 `IdempotentLog::open`，失败则 `.expect(...)` panic（或返回 Err）。
  3. `state_idempotency: required` 但 `loop_id` 缺失 → 返回 Err 让 runner 退出（不 panic，因为某些 legacy 入口本无 loop_id）。
- U4 已确保 fresh workspace 在 loop_id 存在时创建 `loop-version.json`，所以正常路径走分支 2。

**Execution note:** 测试先行。先写 open 失败仍 fallback disabled 的 failing test，再实现 panic。

**Patterns to follow:**
- 参考 `state/idempotent_log.rs::open` 的错误类型。
- 参考 `event_loop/mod.rs` 中其他 `expect`/`bail` 的用法。

**Test scenarios:**
- Error path: `state_idempotency: required` 且 `loop-version.json` 损坏/不可写 → panic（或返回 Err）。
- Happy path: `state_idempotency: required` 且文件系统正常 → `IdempotentLog` enabled。
- Edge case: `state_idempotency: disabled` → 允许 `IdempotentLog::disabled()`。
- Edge case: `state_idempotency: required` 但 `loop_id` 缺失 → 返回 Err，不 panic。
- Edge case: 首次 fresh workspace → U4 已确保 `loop-version.json` 存在，open 成功。

**Verification:**
- 新增 idempotent log bootstrap 测试通过。
- 现有 `idempotent_log` 测试不被破坏。

---

- [ ] U8. **stall_recovery 最终阈值自我终止**

**Goal:** 即使 RepairStateMachine 因某种原因未触发终止，`stall_recovery_counts` 达到最终阈值后也必须 emit `plan.blocked`，让 loop 自己停下来。

**Requirements:** R5

**Dependencies:** U7

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`inject_fallback_event`）
- Modify: `crates/ralph-core/src/diagnosis/responder.rs`（`EscalationLevel::Final` 语义）
- Test: `crates/ralph-core/src/event_loop/tests/stall_recovery_finalization.rs`

**Approach:**
- 新增常量 `STALL_FINAL_THRESHOLD: usize = 10`（U6 的 budget=3 应先触发；U8 是安全网）。
- 在 `inject_fallback_event` 中，每次 escalation 前检查 `stall_recovery_counts[task_key]`：若已 ≥ `STALL_FINAL_THRESHOLD` 且 U6 未触发终止，直接 emit `plan.blocked(reason="stall_recovery_exhausted")` 并返回。
- 确保 U6 与 U8 不会重复 emit：一旦某条路径 emit 了 `plan.blocked`，设置 per-loop `terminal_event_emitted` 标志，后续终止尝试直接忽略。
- 在 `diagnosis/responder.rs` 中，`EscalationLevel::Final` 构造的 `TerminationHint` 必须让 `drift/engine.rs::check_termination_hint` 返回 `RecoveryExhausted`。

**Execution note:** 测试先行。先写 escalation 超过阈值仍不终止的 failing test，再实现最终阈值检查。

**Patterns to follow:**
- 参考 `event_loop/mod.rs` 中 `STALL_HARD_THRESHOLD` 的用法。
- 参考 `diagnosis/responder.rs` 中 `TerminationHint` severity 设置。

**Test scenarios:**
- Happy path: `stall_recovery_counts` 达到 10 → emit `plan.blocked(reason="stall_recovery_exhausted")`。
- Edge case: 达到 10 后同一 task_key 再次 stall → 因 terminal_event_emitted 标志已设置，不再重复 emit。
- Happy path: 低于阈值 → 保持原 escalate 行为。
- Integration: `EscalationLevel::Final` hint → `check_termination_hint` 返回 `RecoveryExhausted`。
- Edge case: U6 已因 budget 耗尽 emit `plan.blocked` → U8 不再 emit。

**Verification:**
- 新增 stall recovery finalization 测试通过。
- 现有 `drift_integration` 测试不被破坏。

---

- [ ] U9. **RecoveryFinalizer 通用兜底组件**

**Goal:** 把所有"提醒型"修复机制（stall/drift/repair）统一接入一个 finalizer：超过 `max_escalation_count` 后 emit 终态事件，而不是无限震荡。

**Requirements:** R9

**Dependencies:** U8

**Files:**
- Create: `crates/ralph-core/src/event_loop/recovery_finalizer.rs`
- Modify: `crates/ralph-core/src/diagnosis/responder.rs`
- Modify: `crates/ralph-core/src/drift/engine.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（接入 finalizer）
- Test: `crates/ralph-core/src/event_loop/tests/recovery_finalizer.rs`

**Approach:**
- 新建 `RecoveryFinalizer`，接口：`record(mechanism, key)` → `Option<TerminalEvent>`。
- 每个机制配置：`max_escalation_count` 与 `final_outcome_topic`（默认 `plan.blocked`）。
- **RecoveryFinalizer 不覆盖 stall**：stall 的终止由 U6/U8 负责，避免重复 emit。
- `drift_monitor` critical 累计超过阈值、`repair_stream` 对同一 retry_key 重复失败超过阈值时调用 `RecoveryFinalizer::record`。
- 超过阈值时 emit `plan.blocked(reason="<mechanism>_exhausted")`；若 terminal_event_emitted 已设置则忽略。

**Execution note:** 测试先行。先写 drift critical 超过阈值不终止的 failing test，再实现 finalizer 与接入。

**Patterns to follow:**
- 参考 `drift/engine.rs` 中 `check_termination_hint` 的 termination 判定。
- 参考 `event_loop/mod.rs` 中 emit 辅助函数的风格。

**Test scenarios:**
- Happy path: drift critical finding 连续 5 次 → finalizer emit `plan.blocked(reason="drift_exhausted")`。
- Happy path: repair stream 对同一 retry_key 重发 10 次失败 → emit `plan.blocked(reason="repair_exhausted")`。
- Edge case: stall 路径不经过 RecoveryFinalizer（由 U6/U8 处理）。
- Edge case: 不同 mechanism 独立计数。
- Edge case: 达到阈值前 reset（如 drift 连续 2 iteration 无 critical）→ 计数清零。

**Verification:**
- 新增 RecoveryFinalizer 测试通过。
- 现有 drift / diagnosis / repair 测试不被破坏。

---

- [ ] U10. **ralph/coordinator 在 isolated 模式可 emit 终态**

**Goal:** 当系统卡住且无人接盘时，`ralph` 和 `coordinator` 能直接 emit `plan.blocked` / `LOOP_COMPLETE(success=false)` 作为真终止信号。

**Requirements:** R10

**Dependencies:** U9

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（ralph hat scope 检查）
- Modify: `presets/en/ce-executor-serial.yml`（coordinator / ralph publishes）
- Modify: `presets/schemas/ce-executor-serial.yml`（同步 publishes 声明）
- Test: `crates/ralph-core/src/event_loop/tests/isolation_scope.rs`

**Approach:**
- 在 `event_loop/mod.rs` 的 ralph hat scope 检查中，把 `plan.blocked` 和 `LOOP_COMPLETE(success=false)` 加入 ralph 允许 emits 列表（作为控制/终态话题）。
- 在 preset 中，`coordinator.publishes` 增加 `LOOP_COMPLETE`（失败场景）。
- `ralph` 是运行时 builtin hat，其 publishes 由 `RALPH_CONTROL_TOPICS` 与 preset 派生；在 `event_origin.rs` 的 `RALPH_CONTROL_TOPICS` 中加入 `plan.blocked`，确保 isolated scope 不拦截。
- 注意：FlowStepScopeStage 对终态事件的放行已在 U3  defensive bypass 中处理；U10 只解决 EventOriginGuard 层的隔离拦截。

**Execution note:** 测试先行。先写 ralph emit `plan.blocked` 被 isolated_scope_violation 拒绝的 failing test，再放开 scope。

**Patterns to follow:**
- 参考 `.cursor/rules/multi-hat-isolation.mdc` 中 isolated mode 的 scope 规则。
- 参考 `presets/en/ce-executor-serial.yml` 中 hats 的 `publishes` 格式。

**Test scenarios:**
- Happy path: `ralph` emit `plan.blocked` → accepted。
- Happy path: `ralph` emit `LOOP_COMPLETE(success=false)` → accepted。
- Happy path: `coordinator` emit `LOOP_COMPLETE(success=false)` → accepted。
- Error path: `ralph` emit `work.ready` → 仍被 isolated_scope_violation 拒绝。
- Error path: `executor` emit `plan.blocked` → 仍被拒绝。

**Verification:**
- 新增 isolation scope 测试通过。
- `preset_lint`（ralph-cli + ralph-core）通过。

---

- [ ] U11. **CLI emit 路径接入 stage_pipeline**

**Goal:** 消灭 CLI 与 event_loop 的两套校验，`ralph emit` 必须走 `evaluate_emit_gate`，让所有 stage（RepairDispatch / EmitSchemaGate / FlowStepScope / StepCloseObligation / VerdictGate）在 CLI 路径也生效。

**Requirements:** R11

**Dependencies:** U10

**Files:**
- Modify: `crates/ralph-cli/src/policy_check.rs`（`run_policy_check_unified`）
- Modify: `crates/ralph-cli/src/commands/emit.rs`（必要时调整 outcome 处理）
- Test: `crates/ralph-cli/src/policy_check/tests.rs` 或内联测试

**Approach:**
- 在 `run_policy_check_unified` 中，legacy terminal gate 通过后，构造 `StagePipeline` 与 `StageContext`（CLI 无 `loop_state.active_tasks`，`current_plan_step` 按 `FlowDeclaration` 首 step 初始化），调用 `ralph_core::event_loop::emit_gate::evaluate_emit_gate`。
- 根据 outcome 生成 `PolicyCheckReport`：
  - `AcceptMainBus` → 允许写入 `events.jsonl`。
  - `AcceptRepairStream` / `Reject` → 阻止，并写 `recovery.jsonl` repair envelope。
- 明确 CLI emit 不执行 U5 的 `task_id` fallback：CLI 调用者必须提供有效 `task_id`，否则 stage gate 按 `execution_contract` reject。
- 保持现有 reason code 与 CLI 输出格式兼容。

**Execution note:** 测试先行。先写 CLI emit 缺 required field 不触发 schema gate 的 failing test，再接入 stage_pipeline。

**Patterns to follow:**
- 参考 `event_loop/mod.rs` 对 `evaluate_emit_gate` outcome 的路由。
- 参考 `repair_stream_sink::record_repair_event`。

**Test scenarios:**
- Happy path: `ralph emit LOOP_COMPLETE` → terminal gate + stage gate 都通过 → accepted。
- Error path: `ralph emit work.ready` payload 缺 required field → stage reject，report 含 `missing_required_fields`，`recovery.jsonl` 写入 `repair_dispatch` envelope。
- Error path: `ralph emit work.done` payload 的 `task_id=""` → CLI 无 active_tasks，stage gate reject，不进 U5 fallback。
- Error path: `ralph emit review.complete` 在 current_step=unit_loop 时 → FlowStepScopeStage reject（验证 CLI 与 loop 路径一致）。
- Edge path: partial state 下 emit 非 `on_partial` topic → `step_close_obligation_violated`。
- Edge path: legacy terminal gate 已 reject → 不再跑 stage_pipeline，保持原 reason code。

**Verification:**
- 新增 policy_check 测试通过。
- 现有 `policy_check` 测试不被破坏。

---

- [ ] U12. **metadata-runtime drift lint**

**Goal:** 让 preset metadata 与运行时真实行为在启动/CI 阶段对齐，避免 `state_idempotency: required` 但实际 `IdempotentLog::disabled()` 的"两层皮"。

**Requirements:** R12

**Dependencies:** U11

**Files:**
- Create: `crates/ralph-core/src/preset_lint/metadata_runtime_drift.rs`
- Modify: `crates/ralph-core/src/preset_lint/mod.rs`
- Modify: `crates/ralph-core/src/preset_lint/finding_id.rs`
- Test: `crates/ralph-core/src/preset_lint/tests/metadata_runtime_drift.rs`

**Approach:**
- 新增 lint 模块 `metadata_runtime_drift.rs`，读取 preset metadata 并与运行时导出的常量/默认值对比：
  - `mechanism.state_idempotency` 的值必须在 `IdempotentLog` 支持的模式列表内；`required` 时运行时 bootstrap 必须启用（U7 已保证 panic 而非 fallback）。
  - `mechanism.repair_budget` 必须等于 `RepairBudget::DEFAULT_MAX`（若运行时导出该常量）。
  - `mechanism.enforce_schema` 的值必须在 `EmitSchemaGateStage` 支持的策略列表内。
- 为支持检查，在 `idempotent_log.rs` 新增 `pub fn is_disabled(&self) -> bool`；在 `repair_flow.rs` 确认/新增 `RepairBudget::DEFAULT_MAX` 常量。
- 任何不一致产生 `Finding::Error`，fail-closed。

**Execution note:** 测试先行。先写 `state_idempotency: required` 但运行时 disabled 不报错（当前行为）的 failing test，再实现 lint。

**Patterns to follow:**
- 参考 `preset_lint/schema_parity.rs` 对 preset schema 与 runtime default 的对比逻辑。
- 参考 `preset_lint/finding_id.rs` 中 finding 常量定义。

**Test scenarios:**
- Error path: preset `repair_budget: 5` 但 `RepairBudget::DEFAULT_MAX == 3` → lint Error。
- Error path: preset `enforce_schema: soft` 但 `EmitSchemaGateStage` 仅支持 `hard`/`none` → lint Error。
- Error path: preset `state_idempotency: required` 但 `IdempotentLog` 不支持该模式 → lint Error。
- Happy path: 所有 metadata 与 runtime 导出常量一致 → lint 无 Error。
- Edge case: `state_idempotency: disabled` → 通过。

**Verification:**
- 新增 preset_lint 测试通过。
- `cargo nextest run -p ralph-core -- preset_lint` 全绿。

---

- [ ] U13. **在 ce-executor-serial preset 中禁用 human.guidance**

**Goal:** 在无人工介入模型下，`human.guidance` 是无意义事件，直接禁掉以消除 drift 误报与 isolated scope violation 噪音。

**Requirements:** R13

**Dependencies:** U12

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`（coordinator / ralph / progress-steward publishes）
- Modify: `presets/schemas/ce-executor-serial.yml`（同步 schema）
- Modify: `crates/ralph-core/src/drift/detector.rs`（排除 human.guidance 字段缺失告警）
- Test: `crates/ralph-cli/src/presets.rs` 静态断言 + `crates/ralph-core/src/preset_lint/tests.rs`

**Approach:**
- 确认 `presets/en/ce-executor-serial.yml` 已设置 `suppress_human_guidance: true`；若未设置则添加。
- 在 `drift/detector.rs` 的 `check_field_completeness` 中，对 `topic == "human.guidance"` 的字段缺失直接忽略。
- 更新 `dimension-reviewer` / `coordinator` / `progress-steward` instructions，删除任何鼓励 emit `human.guidance` 的段落。

**Execution note:** 测试先行。先写 preset 仍声明 `human.guidance` 时不报错的 failing test，再实现移除与 lint。

**Patterns to follow:**
- 参考 `AGENTS.md` preset/schema 同步清单。

**Test scenarios:**
- Happy path: `suppress_human_guidance: true` 已设置。
- Error path: drift 观测到 `human.guidance.message` 缺失 → 不产生 finding。
- Happy path: `test_ce_executor_root_preset_matches_embedded` 仍通过。
- Edge case: 其他 topic（如 `task.resume`）的字段缺失仍正常产生 finding。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded` 通过。

---

- [ ] U14. **projector 接管 plan frontmatter status**

**Goal:** 机械化约束 plan status 修改，禁止 `dimension-reviewer` 写 plan frontmatter，改由 projector 在 `test.passed` 后统一更新。

**Requirements:** R14

**Dependencies:** U13

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（在 projector 处理 `test.passed` 时写 status）
- Modify: `presets/en/ce-executor-serial.yml`（dimension-reviewer / coordinator instructions）
- Test: `crates/ralph-core/src/event_loop/tests/plan_frontmatter_projection.rs`

**Approach:**
- 在 `test.passed` accept 后，若 payload 的 `step` 字段匹配 `fix-NN`，projector 读取当前 plan frontmatter，把 `status: u{N}-open` 改为 `status: u{N}-closed-u{N+1}-pending`；非 fix step 不更新。
- 在 preset 的 `dimension-reviewer` instructions 中增加 HARD RULE：禁止修改 plan status，只读。
- 在 `coordinator` instructions 中删除"必要时手动修复 status"的表述。

**Execution note:** 测试先行。先写 `test.passed` 后 status 未被 projector 更新的 failing test，再实现。

**Patterns to follow:**
- 参考 `event_loop/mod.rs` 中 projector 对 `tasks.jsonl` / `progress.md` 的写入模式。

**Test scenarios:**
- Happy path: `test.passed fix-02` accept 后 plan frontmatter status 变为 `u2-closed-u3-pending`。
- Edge case: `test.passed` 非 fix step（如 `test.passed setup-01`）→ 不修改 status。
- Error path: `test.passed` payload 中携带 `plan_status` 字段 → projector 忽略该字段，仍按规则计算。

**Verification:**
- 新增 plan frontmatter projection 测试通过。
- 现有 scenarios 测试不被破坏。

---

- [ ] U15. **projector 接管 progress.md**

**Goal:** agent 不再负责写 `progress.md`，改由 projector 在 `work.done` accept 时自动勾选对应 step。

**Requirements:** R15

**Dependencies:** U14

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（确保 projector 调用链包含 progress close）
- Modify: `presets/en/ce-executor-serial.yml`（executor instructions 删除"Update progress.md"）
- Test: `crates/ralph-core/src/state_projector/tests.rs` 或新增 `crates/ralph-core/src/event_loop/tests/progress_projection.rs`

**Approach:**
- 确认 `state_projector::progress::project_close_step` 已在 `work.done` accept 路径被调用；若未调用，在 event loop 的 projector 调用链中补上。
- 在 preset executor instructions 中删除"Update progress.md"相关段落，改为"progress.md 由 projector 自动维护到 `.ralph/agent/progress.md`，不要手动编辑"。
- 验证 projector 写入已幂等：重复 `work.done` 同一 step 不会重复添加 Completed Steps 条目。

**Execution note:** 测试先行。先写 `work.done` accept 后 progress.md 未被更新的 failing test，再实现 projector 写入。

**Patterns to follow:**
- 参考 `event_loop/mod.rs` 中 projector 对其他 scratchpad 文件的写入。

**Test scenarios:**
- Happy path: `work.done step-01` accept 后 `.ralph/agent/progress.md` 中 `step-01` 进入 Completed Steps。
- Happy path: `work.done fix-02` accept 后 `fix-02` 进入 Completed Steps。
- Edge case: 重复 accept 同一 step → Completed Steps 不重复添加。
- Error path: progress.md 文件不存在 → projector 创建文件并写入模板。

**Verification:**
- 新增 progress projection 测试通过。
- 现有 scenarios 测试不被破坏。

---

- [ ] U16. **全量回归与下游同步**

**Goal:** 确保 U1-U15 合在一起不引入回归，preset/schema/config/docs 保持一致。

**Requirements:** R1-R15

**Dependencies:** U1, U2, U3, U4, U5, U6, U7, U8, U9, U10, U11, U12, U13, U14, U15

**Files:**
- 可能修改：`presets/schemas/ce-executor-serial.yml`、`crates/ralph-cli/src/presets.rs`、`crates/ralph-cli/src/preflight.rs`、`crates/ralph-cli/src/config_resolution.rs`、`crates/ralph-core/tests/scenarios/ce_executor_serial_*.yml`、`crates/ralph-core/tests/scenarios.rs`、`AGENTS.md` / `CLAUDE.md`、`scripts/ralph-zsh-plugin.zsh`、`crates/ralph-core/data/ralph-tools-*.md`
- 验证入口：`./scripts/run-tests.sh`

**Approach:**
- 按 `AGENTS.md` 下游同步清单 7 步检查：runtime event loop → preset_lint → BDD scenarios → config fields → CLI presets → manifest/index → docs/zsh。
- 若 preset 内容因 U4/U10/U13/U14/U15 变化，同步 `crates/ralph-cli/src/presets.rs`、`presets/manifest.yml`、`presets/index.json`、zsh 补全。
- 更新 `crates/ralph-core/data/ralph-tools-*.md` 中相关命令说明（如 `ralph tools task create`）。
- `cp CLAUDE.md AGENTS.md` 保持两者一致。
- 运行全量回归：`preset_lint`（ralph-cli + ralph-core）、SSOT byte-equality、BDD scenarios、`run-tests.sh`。
- 特别验证：从 `unit_loop` 直接 emit `plan.blocked` / `LOOP_COMPLETE` / `review.start` / `REVIEW_COMPLETE` 能成功进入主 bus（覆盖 U3 bypass 与 U4 transition 的协同）。

**Execution note:** 本 Unit 不写新功能代码，只做同步与测试。

**Patterns to follow:**
- 参考 `AGENTS.md`「preset/schema 改动后的下游同步清单」。
- 参考 `.cursor/rules/multi-hat-isolation.mdc` 的 preset 同步规则。

**Test scenarios:**
- Happy path: `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 全部通过。
- Happy path: `cargo nextest run -p ralph-core -- preset_lint` 全部通过。
- Happy path: `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded` 通过。
- Happy path: `cargo nextest run -p ralph-core --test scenarios -- ce_executor_serial` 通过。
- Happy path: `./scripts/run-tests.sh` 全绿。

**Verification:**
- `./scripts/run-tests.sh` 返回全部通过。

---

## System-Wide Impact

- **Interaction graph:**
  - `flow_step_scope_stage` 的 defensive bypass 与 `flow_lifecycle` 的状态推进共同决定 review-chain 的 emit 准入。
  - `drift/detector.rs` 的过滤与 min_samples 影响 `drift/engine.rs` 的 outcome 与 recovery 注入。
  - `event_loop/mod.rs` 的 RepairStateMachine 驱动、stall final threshold、ralph scope 改动都集中在 loop 核心路径。
  - `TaskStore` / `IdempotentLog` 的联动影响 task 持久化与 U11/U13。
  - `policy_check` 接入 `evaluate_emit_gate` 让 CLI emit 与 loop 内 emit 共享同一套 gate。
  - projector 接管 plan status 与 progress.md 改变 agent 与状态文件的交互契约。
- **Error propagation:**
  - `review.dimensions.complete` 不再被错误拒绝，review-synthesizer 可正常激活。
  - stall/drift/repair 超过阈值后统一 emit `plan.blocked`，runner 明确退出。
  - placeholder / empty `task_id` 被自动回填或显式拒绝，避免级联 `TaskWrongLoop`。
- **State lifecycle risks:**
  - `IdempotentLog::open` 失败从 silent fallback 改为 panic，意味着文件系统异常会立即暴露。
  - projector 双写 JSONL + progress.md 需保证 JSONL 仍是主源，progress.md 是只读视图。
- **API surface parity:**
  - CLI `ralph emit` 行为改变：更多原来被 legacy validation 放过的事件现在会被 stage gate reject 并进入 recovery 流。
  - `coordinator` / `ralph` 的 `publishes` 列表扩大，需同步到 preset、schema、CLI builtin。
- **Integration coverage:**
  - CLI 路径覆盖 stage_pipeline 是跨层集成，U11 测试必须验证。
  - metadata-runtime drift lint 是 preset 与 runtime 的跨层一致性，U12 测试必须验证。
  - 全量 regression 在 U16 验证。
- **Unchanged invariants:**
  - isolated mode 下 hat `publishes` 仍是 scope 唯一事实源。
  - `LOOP_COMPLETE` 仍是默认成功终态事件。
  - `ralph-cli` 测试仍走 nextest 串行。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| FlowStepScope bypass 放错 hat/topic | bypass 列表严格限定为 `review-coordinator` / `review-synthesizer` / `dimension-reviewer` 的合规 review.* 话题。 |
| flow_lifecycle transition 触发点遗漏 | U4 明确列出 `review.start` / `plan.complete` / `REVIEW_COMPLETE` 三个触发点；测试覆盖。 |
| drift min_samples 掩盖真实低样本问题 | 阈值设为 5，与 `emit_cadence` 一致；真实问题在窗口满后仍会报。 |
| IdempotentLog panic 导致正常 fresh run 失败 | U4 确保首次跑显式写 `loop-version.json`，open 在正常情况下成功。 |
| RepairStateMachine 热路径驱动影响非 stall 场景 | 仅在 `inject_fallback_event` stall escalation 路径中驱动。 |
| U6/U8 重复 emit plan.blocked | 设置 per-loop `terminal_event_emitted` 标志；U6 budget=3 应先触发，U8 是安全网。 |
| U3 bypass 成为永久 workaround | 明确为临时防御性措施；U4 完成后 review.* / 终态事件应由 current_step 状态机自然放行。 |
| event_loop/mod.rs 多个 Unit 修改 | KTD-2 函数级隔离 + 每个 Unit 后运行最小集成测试；U16 全量回归兜底。 |
| CLI stage_pipeline 接入改变现有 CLI 行为 | 通过 U11 policy_check 测试锁定；reject 进 recovery 流是预期行为。 |
| metadata-runtime lint 误报现有 preset | lint 只在 `state_idempotency: required` / `enforce_schema: hard` 等显式声明时检查；其他 preset 不受影响。 |
| human.guidance 禁用影响其他 preset | 仅在 `ce-executor-serial.yml` 中移除；基座仍保留该 topic。 |
| projector 写 progress.md 与 agent 并行冲突 | 在 preset 中明确禁止 agent 写 progress.md，projector 幂等写入。 |
| preset 内容变化导致 lint/SSOT/scenarios 失败 | U16 专门处理下游同步。 |

---

## Documentation / Operational Notes

- `cp CLAUDE.md AGENTS.md` 同步两个文件。
- 更新 `crates/ralph-core/data/ralph-tools-tasks.md` 中 `ralph tools task create` 的用法说明。
- 更新 `crates/ralph-core/data/ralph-tools-emit.md` 中 CLI emit 现在会走 stage_pipeline 的说明。
- 在 `docs/solutions/integration-issues/` 下新增/更新 solution 文档，记录本次 fix-unit 死锁、drift 自观测、recovery 无终止的根因。
- 若新增 `RecoveryFinalizer` 或 `metadata_runtime_drift` lint，在 `.cursor/rules/architecture-modules.mdc` 中补充代码位置索引。

---

## Sources & References

- **Origin document:** `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md`
- **Previous related plan:** `docs/plans/2026-06-28-002-fix-ce-executor-serial-loop-and-mechanism-failure-plan.md`
- **Mechanism foundation plan:** `docs/plans/2026-06-27-001-feat-ralph-orchestrator-mechanism-foundation-plan.md`
- **Code references:**
  - `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs`
  - `crates/ralph-core/src/flow_lifecycle.rs`
  - `crates/ralph-core/src/drift/detector.rs`
  - `crates/ralph-core/src/drift/engine.rs`
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/diagnosis/responder.rs`
  - `crates/ralph-core/src/event_loop/repair_flow.rs`
  - `crates/ralph-core/src/execution_contract.rs`
  - `crates/ralph-core/src/state_projector/task.rs`
  - `crates/ralph-cli/src/policy_check.rs`
  - `crates/ralph-cli/src/commands/emit.rs`
  - `presets/en/ce-executor-serial.yml`
  - `presets/schemas/ce-executor-serial.yml`
