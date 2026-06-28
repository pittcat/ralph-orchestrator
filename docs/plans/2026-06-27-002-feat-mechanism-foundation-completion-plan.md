---
title: Mechanism Foundation 接线完成计划
type: feat
status: active
date: 2026-06-27
origin: docs/plans/2026-06-27-001-feat-ralph-orchestrator-mechanism-foundation-plan.md
prior_validation: docs/solutions/integration-issues/mechanism-foundation-validation-2026-06-27.md
diagnostic_report: docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md
---

# Mechanism Foundation 接线完成计划

## Overview

原 plan `2026-06-27-001` 的 **纯逻辑层（U0–U5、U10）约 85% 完成**，但 **接线层与验收约 35%**：Stage pipeline 只挂在 `publish_event`，主路径 `process_events_from_jsonl` 未过 gate；U7 repair sink / budget / legacy 回填未 runtime 接通；U9.5 legacy `verdict_gate`（`additional_topics: ["report.done"]`）仍在；半完成「沉默」无 step-close enforcement；5 个 mechanism BDD 被降级为 scaffold。

本 plan **不重做已有模块**，只补齐 **6 条未完成机制链路**，使 SC-1~SC-6 与 2026-06-26 诊断中的 P0 根因真正被 runtime 挡住。

**执行模式（强制）**：纯粹串行 · 绝对隔离 · 原子化 TDD。每个 Unit 必须先红后绿，**测试只断言本 Unit 的输入/输出**；禁止把边界问题推给后置 Unit；禁止交替开发。

---

## Problem Frame

| 诊断根因 | 001 plan 药方 | 001 实现缺口 | 本 plan 对应 Unit |
|----------|---------------|--------------|-------------------|
| emit 仅 soft-check，空 reason 进事件流 | U6 硬契约 gate | 只接 `publish_event` | U1–U3 |
| recovery 28 次空转，repair 走主 EventBus | U7 独立 repair stream | stage 壳、无 sink/budget/回填 | U4–U8 |
| `report.done` 触发 `review_failed` 终止 | U9.5 terminal_emits | legacy path 仍在 | U9–U10 |
| 4/8 半完成 coordinator 沉默 | U9 flow + on_partial | 仅 passive gate，无 obligation | U11–U12 |
| worktree 复用 archive 失败仍启动 | U11 fail-closed | `warn` 后继续 | U13 |
| BDD/SC 无法证明机制生效 | 附录 D + SC 表 | yml 降级、replay 缺失 | U14–U19 |

（见 `prior_validation` §U6.5、`diagnostic_report` §2.5 iter=17）

---

## Requirements Trace

- **R1. 单一 emit gate 入口**：所有 hat 业务事件（JSONL ingest 与 `publish_event`）必须经过同一 gate 函数，reject 写 recovery envelope，不得进 EventBus。
- **R2. Repair stream 端到端**：repair topic early-return → repair sink；budget 耗尽 → `plan.blocked`；loop 启动调用 `relocate_legacy_tasks`。
- **R3. Verdict 语义切换**：仅 `LOOP_COMPLETE` 触发 loop 终止；`report.done` 不再走 legacy auto-terminate。
- **R4. 半完成 obligation**：unit 进度达 partial 时，下一步 business emit 必须符合 `on_partial` 映射，否则 reject（解决 iter=17 沉默类故障）。
- **R5. 验收恢复**：5 个 mechanism BDD 恢复 wire-level 断言；`scenario_replay_2026_06_26` 实现；SC-1~6 在新 loop 数据上可测量。
- **R6. 运维 migration**：`ralph migrate-state` roundtrip（附录 C）。

**成功标准**（继承 001 plan SC 表 + 测量命令）：

| SC | 阈值 |
|----|------|
| SC-1 | `scenario_replay_2026_06_26` 全绿；recovery_count ≤ 3 |
| SC-2 | 4/8 iter 事件流含 `review.start` 或 `plan.blocked` |
| SC-3 | 单 task `task.resume`/`repair` retry ≤ repair_budget |
| SC-4 | drift 无「必填字段 0%」 |
| SC-5 | summary 计数 = `_final:true` 记录数 |
| SC-6 | worktree 复用后 active `.ralph/` 无旧 loop_id 污染 |

**LOOP_COMPLETE 前**：`./scripts/run-tests.sh` 全绿（含 preset_lint、scenarios、doctest）。

---

## Scope Boundaries

### 本轮绝对不做

- 改 hat prompt / `ralph-adapters` / TUI / Web Dashboard
- 新增 crate 依赖（`fs2`/`nix` 已存在于 U4，不重复引入）
- 重写 U0–U5 已有纯逻辑模块（仅允许 U4 内小改以配合 U8 若必需）
- 在 Unit 1–18 内写跨多机制的全链路集成测试（留给 U19）

### Deferred to Separate Tasks

- `loop_id` 升级 UUID
- `task.resume` 的 `target_hat` 强校验
- 状态机 dot 可视化工具
- Windows inter-process mutex（U4 已文档化，单独 issue）

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/event_loop/mod.rs` — `process_parse_result`（~6156）、`publish_event`（~9105）；**单一 gate 接入点**
- `crates/ralph-core/src/event_loop/stage_pipeline.rs` — 锁定 stage 顺序；`with_default_stages`
- `crates/ralph-core/src/event_loop/stages/repair_dispatch_stage.rs` — `is_repair_topic`（当前未被 mod.rs 消费）
- `crates/ralph-core/src/event_loop/repair_flow.rs` — 真实 `RepairStateMachine`（与 stage_pipeline 空 stub 重名）
- `crates/ralph-core/src/event_loop/legacy_task_relocate.rs` — U3 已实现，runtime 未调用
- `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs` — unknown step **fail-open**（L66–69）
- `presets/schemas/ce-executor-serial.yml` — `verdict_gate.additional_topics: ["report.done"]` 待删
- `crates/ralph-core/tests/scenarios/mechanism/foundation/*.yml` — 5 个 scaffold，待恢复
- `crates/ralph-core/src/event_loop/tests/u6_wiring.rs` — `publish_event` gate 测试范式

### Institutional Learnings

- `docs/solutions/integration-issues/mechanism-foundation-validation-2026-06-27.md` — U6.5 根因与 BDD 降级记录；**禁止再次通过降期望过关**
- `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` — lint 有了 runtime 未迁移的反模式；本 plan 要求 legacy path **退役**而非叠加
- 2026-06-24 P0-2/P0-3 — stub scenario 不 assert events；U14–U18 必须恢复 `expected.events`

### External References

- 无。沿用 001 plan 附录 A `mechanism.flow` SSOT。

---

## Key Technical Decisions

1. **先抽纯函数 gate facade，再分两次接线（publish → jsonl）**：避免 U3 与 U2 纠缠；U1 零 EventLoop 依赖，U2/U3 各测一条路径。（见 origin 001「Stage pipeline」+ prior_validation U6.5）
2. **Repair 路由用显式 `EmitRoute` 枚举**，不用隐式 `Ok(())` + 注释约定：AcceptMainBus | AcceptRepairStream | Reject。
3. **合并 `RepairStateMachine`**：删除 `stage_pipeline::RepairStateMachine` 空 stub，StageContext 持有 `repair_flow::RepairStateMachine`。
4. **Step-close obligation 独立纯逻辑模块**，再接一个 stage；不在 U11 内同时改 EventLoop 多处。
5. **BDD 恢复按 scenario 拆 Unit**，每个 Unit 只改 1 个 yml + 跑 1 个 scenario 子集；不批量改 5 个 yml 在一个 commit。
6. **U19 是唯一允许的全量集成验收 Unit**。

---

## 执行约束（本 plan 特有）

与 001 plan 相同，并追加：

0. **先红**：U14 之前不得 merge 让 mechanism BDD 通过的「降期望」commit。
1. **严格串行**：U1 → U2 → … → U19，100% 完成前一 Unit 才能开始下一 Unit。
2. **绝对隔离**：Unit 测试不得 import 后置 Unit 新增符号；不得 assert 后置 Unit 的行为。
3. **原子 TDD**：每 Unit 测试文件命名 `*_u<N>_*.rs` 或 scenario 名与 Unit 对齐；Verification 只列本 Unit 子集命令。
4. **Stage hot path**：gate facade 保持 O(字段数)，禁止每 step 重扫 JSONL。

---

## High-Level Technical Design

> *本图仅说明 intended approach，是 review 方向指引，不是实现规范。*

```text
  hat event (JSONL or publish_event)
           │
           ▼
  ┌─────────────────────┐
  │ U1: evaluate_emit   │  ← 纯函数：pipeline.run + route 判定
  │     _gate(ctx, ev)  │
  └─────────┬───────────┘
            │
     ┌──────┼──────┐
     ▼      ▼      ▼
  Reject  Repair  MainBus
     │    Stream     │
     ▼      │        ▼
 recovery  U6-U8   EventBus
 .jsonl   sink
```

**Stage 顺序不变**（001 plan 锁定）：RepairDispatch → EmitSchemaGate → FlowStepScope → VerdictGate；Archive 仍在 loop start。

---

## Implementation Units

### 阶段 0：Emit Gate 统一（U1–U3）

- [ ] **U1. Emit gate facade（纯逻辑）**

**Goal：** 提供单一纯函数 `evaluate_emit_gate(pipeline, ctx, event) -> EmitGateOutcome`，封装 `StagePipeline::run` + `is_repair_topic` 路由判定。

**Requirements：** R1

**Dependencies：** 无（复用已有 stage 实现）

**Files：**
- 创建：`crates/ralph-core/src/event_loop/emit_gate.rs`
- 测试：`crates/ralph-core/src/event_loop/emit_gate/tests.rs`
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（`pub mod emit_gate`）

**Approach：**
- 定义 `EmitGateOutcome { AcceptMainBus, AcceptRepairStream, Reject(StageReject) }`
- repair topic + pipeline Ok → `AcceptRepairStream`（不是 Reject）
- 非 repair + pipeline Err → `Reject`
- 不访问文件系统、不访问 EventBus

**Execution note：** 先写 6 个单元测试（红），再实现 facade（绿）。

**Test scenarios：**
- Happy path：完整 payload + 非 repair topic → `AcceptMainBus`
- Happy path：`task.relocate_legacy` + pipeline Ok → `AcceptRepairStream`
- Error path：缺 required field → `Reject(missing_required_fields)`
- Error path：空 pipeline → 任意事件 `AcceptMainBus`
- Edge case：repair topic + pipeline Reject → `Reject`（repair 事件 schema 失败仍 reject）
- Edge case：`LOOP_COMPLETE` 通过 pipeline → `AcceptMainBus`

**Verification：**
- `cargo nextest run -p ralph-core -- emit_gate_u1` 全绿

---

- [ ] **U2. publish_event 改走 emit gate facade**

**Goal：** `EventLoop::publish_event` 唯一调用 U1 facade；Reject → `record_stage_rejection`；RepairStream → repair sink 占位（本 Unit 仅计数/日志，U6 接真 sink）。

**Requirements：** R1

**Dependencies：** U1

**Files：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（`publish_event`）
- 测试：`crates/ralph-core/src/event_loop/tests/u2_publish_emit_gate.rs`

**Approach：**
- 删除 `publish_event` 内联 `stage_pipeline.run`
- RepairStream 本 Unit：写 `repair_stream_pending` 计数或 debug 日志（**不进 EventBus**）
- 不修改 `process_parse_result`

**Test scenarios：**
- Happy path：`plan.blocked(reason="x")` → bus 收到事件，recovery 无 reject
- Error path：`plan.blocked(reason="")` → bus 无事件，recovery 含 `missing_required_fields`
- Happy path：`work.done` 完整字段 → bus 收到
- Edge case：repair topic → bus **无**该 topic（仅 U2 占位 sink 计数 +1）

**Verification：**
- `cargo nextest run -p ralph-core -- u2_publish_emit_gate` 全绿
- 既有 `u6_wiring` 测试仍绿（或合并后删除重复）

---

- [ ] **U3. process_parse_result 接入 emit gate facade**

**Goal：** JSONL ingest 路径在 `accepted.push(event)` **之前**调用 U1 facade；与 U2 行为一致。

**Requirements：** R1

**Dependencies：** U1、U2

**Files：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（`process_parse_result` isolated/coordinator 分支）
- 测试：`crates/ralph-core/src/event_loop/tests/u3_jsonl_emit_gate.rs`

**Approach：**
- 提取私有方法 `apply_emit_gate(&mut self, event) -> bool`（true=进入 accepted）
- Reject 走 `record_stage_rejection`
- RepairStream 走 U2 同款占位 sink
- **本 Unit 不**改 BDD yml

**Execution note：** 先写 u3 测试复现「jsonl 注入空 reason plan.blocked 仍进 bus」失败，再接线。

**Test scenarios：**
- Error path：经 `process_events_from_jsonl` 注入 `plan.blocked(reason="")` → `accepted` 不含该事件
- Happy path：完整 `work.done` → `accepted` 含该事件
- Error path：reject 后 `recovery.jsonl` 有 envelope（temp dir）
- Edge case：orchestrator internal topic 仍 bypass gate（与现有 `is_orchestrator_internal` 一致）

**Verification：**
- `cargo nextest run -p ralph-core -- u3_jsonl_emit_gate` 全绿

---

### 阶段 1：Repair Stream（U4–U8）

- [ ] **U4. RepairStateMachine 类型统一（纯重构）**

**Goal：** 删除 `stage_pipeline::RepairStateMachine` 空 stub；`StageContext` 与 `EventLoop.repair_state_machine` 改用 `repair_flow::RepairStateMachine`。

**Requirements：** R2

**Dependencies：** U3

**Files：**
- 修改：`crates/ralph-core/src/event_loop/stage_pipeline.rs`
- 修改：`crates/ralph-core/src/event_loop/types.rs`
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（`build_stage_context_for`）
- 测试：`crates/ralph-core/src/event_loop/tests/u4_repair_sm_unify.rs`

**Approach：**
- 仅类型与构造变更；**不改变** repair 行为
- 更新所有 test 中 `RepairStateMachine` 引用

**Test scenarios：**
- Happy path：`EventLoop` 构造后 `repair_state_machine.budget().max() == 3`（或 preset 覆盖值）
- Happy path：`build_stage_context_for` 返回的 ctx 引用有效 state machine
- Edge case：编译期无 duplicate type 名称

**Verification：**
- `cargo nextest run -p ralph-core -- u4_repair_sm_unify` 全绿
- `cargo nextest run -p ralph-core -- stage_pipeline` 全绿

---

- [ ] **U5. RepairDispatchStage 预算 transition（stage 纯逻辑）**

**Goal：** `RepairDispatchStage::check` 对 repair topic 调用 `ctx.repair_state.try_transition`；budget 耗尽返回 `StageReject { reason_code: repair_unrecoverable_after_N_retries }`。

**Requirements：** R2

**Dependencies：** U4

**Files：**
- 修改：`crates/ralph-core/src/event_loop/stages/repair_dispatch_stage.rs`
- 修改：`crates/ralph-core/src/event_loop/stage_pipeline.rs`（`StageContext` 可变借用或 transition 接口）
- 测试：`crates/ralph-core/src/event_loop/stages/repair_dispatch_stage/tests.rs`（新增 budget 场景）

**Approach：**
- 映射 topic → `RepairAction`（`task.relocate_legacy` → BeginDiagnosis 等，锁定于 stage 内常量表）
- **不**接 EventLoop；测试直接构造 `StageContext` + mock state machine

**Test scenarios：**
- Happy path：首次 repair topic → Ok（pipeline 继续）
- Error path：同一 task 第 4 次 Retry → `StageReject` reason 含 `repair_unrecoverable_after_3_retries`
- Edge case：非 repair topic → Ok 且不消耗 budget
- Error path：budget 耗尽后再次 transition → 仍 Reject，不 panic

**Verification：**
- `cargo nextest run -p ralph-core -- repair_dispatch_stage` 全绿

---

- [ ] **U6. Repair sink 写入器（纯 I/O 边界）**

**Goal：** 实现 `RepairStreamSink::record(event, loop_id) -> Result<()>`，append 到 `.ralph/recovery.jsonl`（或独立 repair 文件，与 001 plan 一致用 recovery envelope）；**不进 EventBus**。

**Requirements：** R2

**Dependencies：** U5

**Files：**
- 创建：`crates/ralph-core/src/event_loop/repair_stream_sink.rs`
- 测试：`crates/ralph-core/src/event_loop/repair_stream_sink/tests.rs`
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（`pub mod repair_stream_sink`）

**Approach：**
- 临时目录测试；不依赖 EventLoop
- 与 `record_stage_rejection` 共用 envelope 格式（reason_code=`repair_dispatch`）

**Test scenarios：**
- Happy path：写入一条 repair event → recovery 文件含该 topic
- Edge case：同文件追加两次 → 两行
- Error path：只读目录 → 返回 IO 错误

**Verification：**
- `cargo nextest run -p ralph-core -- repair_stream_sink_u6` 全绿

---

- [ ] **U7. Repair sink 接入 publish + jsonl 双路径**

**Goal：** U2/U3 的 RepairStream 占位改为调用 U6 sink；主 EventBus **永不**收到 repair topic。

**Requirements：** R2

**Dependencies：** U3、U6

**Files：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`
- 测试：`crates/ralph-core/src/event_loop/tests/u7_repair_sink_wiring.rs`

**Test scenarios：**
- Happy path：`publish_event(task.relocate_legacy)` → bus 无该 topic，recovery 有
- Happy path：`process_parse_result` 同路径一致
- Integration（本 Unit 内唯一跨路径）：publish 与 jsonl 各测 1 例，**不断言** budget 耗尽（属 U5/U8）

**Verification：**
- `cargo nextest run -p ralph-core -- u7_repair_sink_wiring` 全绿

---

- [ ] **U8. loop 启动 legacy task 回填 + repair.close 清零**

**Goal：** `EventLoop::with_context_and_diagnostics` 启动时调用 `relocate_legacy_tasks`；处理 `repair.close` 时 `stall_recovery_counts[task_key]=0`。

**Requirements：** R2、SC-3

**Dependencies：** U7

**Files：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（on_start / repair 处理）
- 修改：`crates/ralph-core/src/event_loop/loop_state.rs`（`on_repair_close`）
- 测试：`crates/ralph-core/src/event_loop/tests/u8_legacy_relocate_and_close.rs`

**Approach：**
- 临时 `tasks.jsonl` 测试回填；不依赖 git worktree
- `repair.close` 处理放在 repair sink 或专用 handler（本 Unit 只测 counter 清零）

**Test scenarios：**
- Happy path：2 legacy + 1 有 loop_id → 回填 2
- Happy path：`repair.close(task_key=k)` → `stall_recovery_counts[k]==0`
- Edge case：无 tasks 文件 → 启动不 panic
- Idempotency：二次启动同 loop_id → 回填 0

**Verification：**
- `cargo nextest run -p ralph-core -- u8_legacy_relocate` 全绿
- `cargo nextest run -p ralph-core -- relocate_legacy_tasks` 仍绿

---

### 阶段 2：Verdict 与 Flow Obligation（U9–U12）

- [ ] **U9. Legacy verdict_gate 退役（schema + runtime）**

**Goal：** 从 `presets/schemas/ce-executor-serial.yml` 删除 `verdict_gate.additional_topics`；`mod.rs` 中 `report.done` 不再触发 `ReviewFailed` auto-terminate。

**Requirements：** R3

**Dependencies：** U8

**Files：**
- 修改：`presets/schemas/ce-executor-serial.yml`
- 修改：`presets/en/ce-executor-serial.yml`（若 embedded 引用）
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（~1263 终止分支）
- 测试：`crates/ralph-core/src/event_loop/tests/u9_verdict_legacy_retire.rs`

**Approach：**
- 7 层下游同步（001 HARD RULE）：preset_lint、scenarios、`presets.rs` 若 byte 变化
- 本 Unit **不**改 VerdictGateStage

**Test scenarios：**
- Happy path：模拟 `report.done(pass_or_fail=fail)` → **不**产生 `TerminationReason::ReviewFailed`
- Happy path：`REVIEW_COMPLETE(fail)` 仍可被记录但不 auto-terminate loop
- Error path：schema lint 不再要求 additional_topics

**Verification：**
- `cargo nextest run -p ralph-core -- u9_verdict_legacy_retire` 全绿
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 全绿

---

- [ ] **U10. VerdictGate 终止 dispatcher**

**Goal：** pipeline 通过后，若 `VerdictGateStage::is_terminal(topic)` → 写 `loop-termination-reason.json` / 标记终止；仅 `LOOP_COMPLETE`。

**Requirements：** R3

**Dependencies：** U9

**Files：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（publish + jsonl 共用 helper）
- 测试：`crates/ralph-core/src/event_loop/tests/u10_verdict_dispatcher.rs`

**Test scenarios：**
- Happy path：`LOOP_COMPLETE` → termination record 写入
- Happy path：`report.done` → 无 termination record
- Edge case：`REVIEW_COMPLETE` → 无 loop-level termination

**Verification：**
- `cargo nextest run -p ralph-core -- u10_verdict_dispatcher` 全绿

---

- [ ] **U11. FlowStepScope fail-closed（unknown step）**

**Goal：** `flow.step` 缺失时 **reject** `flow_step_undeclared`，不再 fail-open accept。

**Requirements：** R4（部分）

**Dependencies：** U10

**Files：**
- 修改：`crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs`
- 测试：`crates/ralph-core/src/event_loop/stages/flow_step_scope_stage/tests.rs`

**Test scenarios：**
- Error path：`current_step.id="unknown"` + `work.ready` → Reject
- Happy path：合法 step + allowed emit → Ok

**Verification：**
- `cargo nextest run -p ralph-core -- flow_step_scope_stage` 全绿

---

- [ ] **U12. Step-close obligation（纯逻辑 + stage）**

**Goal：** 新增 `step_close_obligation.rs`：跟踪 unit 完成度（4/8 等），在 step 边界要求下一 emit ∈ `on_partial` 映射；实现 `StepCloseObligationStage` 注册为 pipeline 第 4 位之前或合入 FlowStepScope。

**Requirements：** R4、SC-2

**Dependencies：** U11

**Files：**
- 创建：`crates/ralph-core/src/event_loop/step_close_obligation.rs`
- 创建：`crates/ralph-core/src/event_loop/stages/step_close_obligation_stage.rs`
- 修改：`crates/ralph-core/src/event_loop/stage_pipeline.rs`（注册 stage，**更新顺序断言测试**）
- 测试：各模块 `#[cfg(test)]`

**Approach：**
- 纯逻辑模块先实现 `required_emit(progress) -> Option<TopicExpr>`
- stage 只调用纯逻辑；progress 来自 `LoopState` 最小字段（本 Unit 内 stub progress 亦可单测）

**Test scenarios：**
- Happy path：4/8 + emit `plan.blocked(reason="partial_units_done")` → Ok
- Error path：4/8 + emit `REVIEW_COMPLETE`（跳过 review）→ Reject `flow_partial_state_undeclared`
- Error path：4/8 + **沉默**（无 emit）→ 由 step-close hook 在 iteration 边界 Reject（需明确触发点：下一 business emit 或 idle timeout 二选一，**锁定为下一 business emit 时 reject 上一 silence**）
- Edge case：8/8 all_done → 不要求 partial emit

**Verification：**
- `cargo nextest run -p ralph-core -- step_close_obligation_u12` 全绿
- `cargo nextest run -p ralph-core -- stage_pipeline_order` 全绿

---

### 阶段 3：隔离与验收（U13–U19）

- [ ] **U13. Archive fail-closed**

**Goal：** `archive_state_for_loop` 失败 → loop **不启动**（返回 Err），替换当前 `warn` + continue。

**Requirements：** SC-6、R3（state 隔离）

**Dependencies：** U12

**Files：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（`with_context_and_diagnostics`）
- 修改：`crates/ralph-core/src/event_loop/stages/archive_version_stage.rs`（如需公开 Err 类型）
- 测试：`crates/ralph-core/tests/state_isolation_tests.rs`（新增 fail-closed case）

**Test scenarios：**
- Error path：模拟 archive IO 失败 → `EventLoop::new` 返回 Err 或 panic-free abort
- Happy path：正常 archive 仍成功

**Verification：**
- `cargo nextest run -p ralph-core -- state_isolation_archive_fail_closed` 全绿

---

- [ ] **U14. BDD 恢复 — plan_blocked_reason_required**

**Goal：** 恢复 `crates/ralph-core/tests/scenarios/mechanism/foundation/plan_blocked_reason_required.yml` wire-level 断言。

**Requirements：** R5

**Dependencies：** U3

**Files：**
- 修改：`crates/ralph-core/tests/scenarios/mechanism/foundation/plan_blocked_reason_required.yml`
- 修改：`crates/ralph-core/tests/scenarios.rs`（若需新 test fn）

**Execution note：** 先改 yml 让 scenario **失败**，再确认 U3 已绿后 scenario **变绿**。

**Test scenarios：**
- Error path：空 reason → `absent_events: [plan.blocked]` + recovery reason_code
- Happy path：（可选第二 scenario 文件或 mock 分支）非空 reason → events 含 plan.blocked

**Verification：**
- `cargo nextest run -p ralph-core --test scenarios -- plan_blocked_reason_required` 全绿

---

- [ ] **U15. BDD 恢复 — repair_budget_exhausted_blocks_plan**

**Dependencies：** U5、U7、U8

**Files：**
- 修改：`crates/ralph-core/tests/scenarios/mechanism/foundation/repair_budget_exhausted_blocks_plan.yml`

**Verification：**
- `cargo nextest run -p ralph-core --test scenarios -- repair_budget_exhausted_blocks_plan` 全绿

---

- [ ] **U16. BDD 恢复 — flow_unknown_emit_rejected**

**Dependencies：** U11、U12

**Verification：**
- `cargo nextest run -p ralph-core --test scenarios -- flow_unknown_emit_rejected` 全绿

---

- [ ] **U17. BDD 恢复 — verdict_gate_terminal_alignment**

**Dependencies：** U9、U10

**Verification：**
- `cargo nextest run -p ralph-core --test scenarios -- verdict_gate_terminal_alignment` 全绿

---

- [ ] **U18. BDD 恢复 — diagnosis_count_matches_final_state**

**Dependencies：** U8（idempotent 已存在，本 Unit 只恢复 yml 断言）

**Verification：**
- `cargo nextest run -p ralph-core --test scenarios -- diagnosis_count_matches_final_state` 全绿

---

- [ ] **U19. scenario_replay + SC 全量验收 + migrate-state**

**Goal：** 实现 `scenario_replay_2026_06_26.yml`；实现 `ralph migrate-state` + roundtrip 测试；跑 `./scripts/run-tests.sh`；采集 SC-1~6。

**Requirements：** R5、R6、全部 SC

**Dependencies：** U14–U18

**Files：**
- 创建：`crates/ralph-core/tests/scenarios/mechanism/foundation/scenario_replay_2026_06_26.yml`
- 创建：`crates/ralph-cli/src/migrate_state.rs`（或子模块）
- 测试：`crates/ralph-cli` 内 `migrate_state_roundtrip`
- 修改：`crates/ralph-cli/src/main.rs`（注册 subcommand，若 clap 变更需文档同步）

**Approach：**
- replay scenario 先红（模拟 iter=17 沉默 + 4/8）
- migration 纯函数 + CLI 薄包装

**Test scenarios：**
- SC-1：replay scenario 全绿
- migrate：旧 jsonl → migrated → 可读 → 回滚旧格式仍可读
- 全量：`./scripts/run-tests.sh` 绿

**Verification：**
- `./scripts/run-tests.sh` 全绿
- SC-1~6 测量命令在新 `.ralph/` 数据上 documented 于 `docs/solutions/integration-issues/mechanism-foundation-validation-2026-06-27.md` 更新节

---

## System-Wide Impact

- **交互图**：U1 facade 成为 `publish_event` 与 `process_parse_result` 唯一 gate；U12 obligation 依赖 `LoopState` progress 字段，需与 coordinator hat 的 unit 计数一致。
- **错误传播**：Reject 统一 → recovery envelope；archive 失败 → loop 不启动（U13）。
- **状态生命周期**：repair sink + idempotent log 共用 recovery 文件；注意 U6/U8 写入顺序。
- **API 表面对齐**：U9 改 schema `verdict_gate`；U19 新增 CLI `migrate-state`（非 breaking）。
- **集成覆盖**：仅 U19 跑全量；U1–U18 禁止全 workspace 回归作为 Unit Verification。
- **不变量**：hat prompt、adapters、TUI 不变；001 plan Stage 顺序仅 U12 可能插入 obligation stage（须更新宏与测试）。

---

## Risks & Dependencies

| 风险 | 可能性 | 影响 | 缓解 |
|------|--------|------|------|
| U3 接入后 14+ scenario 回归失败 | 高 | 高 | U3 测试隔离；U14 起逐个恢复 yml；U19 全量 |
| U12 obligation 与 preset progress 字段不一致 | 中 | 高 | 纯逻辑模块先锁定输入 struct；对照 appendix A |
| repair budget 与 inject_completion_correction 双计数 | 中 | 中 | U8 文档化 SSOT=recovery.jsonl；U19 验证 SC-3 |
| archive fail-closed 阻断 dev 临时目录 | 低 | 中 | 测试用绝对 temp path；错误信息 actionable |
| migrate-state 遗漏 consumer | 中 | 高 | U19 roundtrip 覆盖 task/recovery/drift |

---

## 分阶段交付

| 阶段 | Units | 交付物 |
|------|-------|--------|
| 0 Emit 统一 | U1–U3 | 双路径 gate 一致 |
| 1 Repair | U4–U8 | repair stream + 回填 + budget |
| 2 Verdict + Obligation | U9–U12 | P0-C 修复 + iter=17 类拦截 |
| 3 验收 | U13–U19 | BDD + SC + migration |

**分 PR 建议**：U1 单独 → U2–U3 一 PR → U4–U8 可按 2 PR → U9–U12 → U13 → U14–U18 各 scenario 可 squash → U19 单独。

---

## Documentation / Operational Notes

- 完成后更新 `docs/solutions/integration-issues/mechanism-foundation-validation-2026-06-27.md` SC 实测值
- 新增 `docs/solutions/integration-issues/mechanism-foundation-emit-gate-unification.md`（U3 根因与单入口决策）
- 001 plan 对应 checkbox 由 implementer 在 002 全部 U19 通过后回填 001 文档（可选）

---

## Sources & References

- **Origin plan:** [docs/plans/2026-06-27-001-feat-ralph-orchestrator-mechanism-foundation-plan.md](docs/plans/2026-06-27-001-feat-ralph-orchestrator-mechanism-foundation-plan.md)
- **Prior validation:** [docs/solutions/integration-issues/mechanism-foundation-validation-2026-06-27.md](docs/solutions/integration-issues/mechanism-foundation-validation-2026-06-27.md)
- **Diagnostic report:** [docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md](docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md)
- **Related code:** `crates/ralph-core/src/event_loop/mod.rs`, `stage_pipeline.rs`, `emit_gate.rs`（待建）
- **Institutional:** [ce-executor-serial-mechanism-close-loop-2026-06-23.md](docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md)

---

## 变更记录

| 版本 | 日期 | 说明 |
|------|------|------|
| v1 | 2026-06-27 | 初稿：U1–U19 串行 TDD 完成 plan，承接 001 约 60% 未完成接线 |
