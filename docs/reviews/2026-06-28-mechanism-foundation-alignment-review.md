# 目标达成度与逻辑一致性审查报告

**审查对象：** `docs/plans/2026-06-27-001-feat-ralph-orchestrator-mechanism-foundation-plan.md` +
`docs/plans/2026-06-27-002-feat-mechanism-foundation-completion-plan.md` 及其实现代码

**审查日期：** 2026-06-28

**审查结论摘要：** 机制 foundation 的大方向与模块实现基本落地，但两个核心成功标准（SC-2、SC-5）实际上未达成，且 repair budget 耗尽升级、`migrate-state` CLI、BDD wire-level 断言等关键链路偏离计划。建议完成 P0/P1 闭环后再标记 `LOOP_COMPLETE`。

---

## 🚨 P0 级风险 (目标脱轨)

### P0-1: Step-close obligation 阶段未 runtime 驱动，4/8 partial silence 仍然无法捕获

- **逻辑断点：** `StepCloseObligationStage` 已注册到 StagePipeline（第 4 位），但其 `progress` registry 永远为空，因为 EventLoop runtime 没有任何路径调用 `StepCloseObligationStage::update_progress(step_id, done, total)`。该 stage 的 `check()` 只有在 `progress.get(step_id)` 存在时才计算 obligation，否则直接 `Ok(())`。
- **目标对比推演：** 当 coordinator 完成 4/8 个 unit 后，若不再 emit `review.start` / `plan.blocked(partial_units_done)`，runtime 不会知道当前 step 处于 partial 状态，也不会在下一 business emit 时 reject 为 `step_close_obligation_violated`。2026-06-26 诊断中的 iter=17 沉默场景会原样复现——loop 只是烧到 max_iterations 后退出，没有机制强制 partial 分支必须被 emit。SC-2（4/8 完成时事件流必须含 `review.start` 或 `plan.blocked`）无法被证明成立。
- **纠偏建议：** 在 runtime 感知到 unit 完成度变化的位置（如 `state_projector/progress.rs` 的 `mark_step_completed`、或 `LoopState` 更新 unit 计数器处）调用 stage 的 `update_progress`；或者在 `process_parse_result` 末尾增加一个 "step-close hook"，当 iteration 边界没有 emit 满足 obligation 时主动 reject。

### P0-2: diagnosis-summary 计数未迁移到 IdempotentLog `_final=true` 记录

- **逻辑断点：** `build_termination_diagnostics`（`crates/ralph-cli/src/loop_runner/runner.rs:71-177`）中 `recovery_count` 仍然通过 `count_recovery_entries` 统计 `recovery.jsonl` 行数，`drift_finding_count` 直接硬编码为 0。U8 要求改为读取 `IdempotentLog.final_records()` 并调用 `DiagnosisSummary::from_final_records()`，但 `from_final_records` 只在单元测试中出现，生产代码没有调用。
- **目标对比推演：** SC-5（`diagnosis-summary.json` 计数 = `_final=true` 记录数）不成立。即使 `IdempotentLog` 成功写入 per-key final records，summary 仍按旧 JSONL 行数统计，两种数据源会 diverge；跨 worktree 复用或进程重启后，summary 可能重复计数或漏计。
- **纠偏建议：** 在 loop 终止前对 `EventLoop::idempotent_log()` 调用 `replay()` 与 `final_records()`，用 `DiagnosisSummary::from_final_records()` 构造 `recovery_count` / `drift_finding_count` / `task_count`，再写入 seed。

---

## ⚠️ P1 级风险 (计划偏离)

### P1-1: Repair budget 耗尽后没有自动升级 emit `plan.blocked`

- **逻辑断点：** `RepairDispatchStage::check()` 在 `BudgetExhausted` 时返回 `StageReject { reason_code: "repair_unrecoverable_after_N_retries" }`，`EventLoop` 将其转为 recovery envelope 写入 `recovery.jsonl`（`record_stage_rejection`）。002 plan R2 / U15 明确要求 "budget 耗尽 → `plan.blocked`"。
- **目标对比推演：** 同 task 第 4 次 repair 失败后，loop 只会多一条 rejection envelope，不会触发 `plan.blocked` 终止流程。operator 看到的是 loop 继续空转或最终 max_iterations 退出，而不是计划要求的 "budget 耗尽自动升级" 行为。
- **纠偏建议：** 在 `record_stage_rejection` 或 `RepairDispatchStage` 下游增加升级分支：当 reason_code 为 `repair_unrecoverable_after_*` 时，synthesize 一个 `plan.blocked(reason="repair_unrecoverable_after_N_retries")` 事件并路由到 shipper / bus。

### P1-2: 5 个 mechanism BDD 未恢复 wire-level 断言，仍用 iterations/completion 降期望过关

- **逻辑断点：** `plan_blocked_reason_required.yml`、`repair_budget_exhausted_blocks_plan.yml`、`flow_unknown_emit_rejected.yml`、`diagnosis_count_matches_final_state.yml`、`scenario_replay_2026_06_26.yml` 的 `expected` 段均只断言 `iterations` 与 `completion`，没有 `events:` / `absent_events:`；且文件头部注释错误声称 "process_events_from_jsonl does NOT route through the emit-time stage pipeline"。002 plan U14-U18 明确要求 "恢复 wire-level 断言" 并 "禁止再次通过降期望过关"。
- **目标对比推演：** scenario 全绿不能证明 gate 真的拦截了空 reason `plan.blocked`、repair topic 没进 bus、flow scope 真的 reject 了未知 emit、verdict gate 真的只对 `LOOP_COMPLETE` 终止。只要 loop 在 max_iterations 后退出，这些测试就会通过。
- **纠偏建议：** 移除 yml 中已过时的 "不经过 gate" 注释；为每个 scenario 添加 `expected.events` / `absent_events` 断言（例如 `absent_events: [plan.blocked]` 且 recovery 包含 `missing_required_fields`）。

### P1-3: `ralph migrate-state` CLI 子命令未注册，且只实现 task migration

- **逻辑断点：** `crates/ralph-cli/src/migrate_state.rs` 只提供 `migrate_tasks_file`；`main.rs` 的 `Commands` 枚举中没有 `Migrate` 变体，因此 `ralph migrate-state` 在 CLI 上不存在。002 plan U19 要求 "实现 `ralph migrate-state` + roundtrip 测试" 且覆盖 "task/recovery/drift"。
- **目标对比推演：** operator 无法通过 CLI 调用迁移；recovery/drift 的 legacy 记录无法被迁移。U19 验收不完整。
- **纠偏建议：** 在 `Commands` 中添加 `Migrate(MigrateStateArgs)`，并实现 `migrate_recovery_file` / `migrate_drift_file`，将 legacy `recovery.jsonl` / `drift.jsonl` 中的记录重写为带 `_idempotency_key` / `_final` 的格式。

### P1-4: `task.resume` 未纳入新的 repair stream / budget

- **逻辑断点：** `REPAIR_TOPICS`（`repair_dispatch_stage.rs:39-44`）只含 `task.relocate*` / `repair.*`，不含 `task.resume`。`task.resume` 仍通过旧的 `stall_recovery_counts` / `rejection_retry_counts` 限制。001 plan 诊断根因表明确将 "task.resume 走主 EventBus" 列为 28 次空转根因；002 plan SC-3 也写 "task.resume/repair retry ≤ repair_budget"。
- **目标对比推演：** 修复预算被拆成两套计数器（RepairStateMachine 用于 repair topics，旧计数器用于 task.resume），无法保证统一 budget；task.resume 仍可能绕过新机制。
- **纠偏建议：** 将 `task.resume` 加入 `REPAIR_TOPICS` 或通过 `RepairDispatchStage` 统一路由，并确保其 retry 消耗同一 `repair_budget`。

### P1-5: IdempotentLog 写入与 diagnosis summary 未形成闭环

- **逻辑断点：** `idempotent_wiring::write_recovery` / `write_drift` 把记录写到 per-key `.jsonl` 文件，但 loop 终止时 summary 仍从旧的 `recovery.jsonl` / `drift.jsonl` 计数。U8 要求 summary 从 final records 读取。
- **目标对比推演：** SC-5 测量命令 `jq '.recovery_count' .ralph/diagnosis-summary.json` vs `grep -c '"_final":true' .ralph/recovery.jsonl` 不会相等，因为前者数 recovery.jsonl 行，后者格式/文件都不一致。
- **纠偏建议：** 同 P0-2，让 summary 基于 `IdempotentLog.final_records()`。

---

## 🔍 P2 级风险 (次要妥协)

### P2-1: Clippy / 文档验收红线未达成

- **逻辑断点：** `cargo clippy --workspace --all-targets -- -D warnings` 当前报 341 errors；`cargo doc --no-deps -- -D missing_docs` 也可能因 warnings 未清理而不绿。AGENTS.md / 001 plan 将这两项列为 Unit 完成红线。
- **目标对比推演：** 虽然功能逻辑已落地，但项目自身的 "LOOP_COMPLETE 前全量基线" 无法通过，不能按项目规则标记完成。
- **纠偏建议：** 运行 `cargo clippy --workspace --all-targets -- -D warnings` 并修复所有 error；补全缺失的 rustdoc。

### P2-2: `flow_hat_step_scope` lint 规则缺失

- **逻辑断点：** 001 plan U5 列出四条 lint 规则，包含 `flow_hat_step_scope`（coordinator 跨 step emit）。实际实现的 lint 模块有 5 条规则，但没有跨 step scope 检查。
- **目标对比推演：** coordinator 在 `review_walk` step emit `work.ready` 不会在 preset-load 时被 lint 捕获，只能依赖 runtime 的 `FlowStepScopeStage`。
- **纠偏建议：** 在 `preset_lint/flow_declaration.rs` 中补充 `flow_hat_step_scope` 规则，检查 hat 的 `publishes` 与 step 的 `allowed_emits` 是否匹配。

### P2-3: BDD scenario 头部注释与实现矛盾

- **逻辑断点：** scenario 文件注释仍写 "process_events_from_jsonl does NOT route through the emit-time stage pipeline"，但 `EventLoop::evaluate_emit_gate_for_jsonl_event` 已接入 `process_parse_result`。
- **目标对比推演：** 维护者会被误导，认为 gate 未接入 JSONL 路径。
- **纠偏建议：** 更新注释，说明 gate 已接入，并解释为什么 wire-level 断言需要恢复。

---

## 📊 最终对齐度评估

综合判断：

- **Stage pipeline 骨架、硬契约 gate、repair stream sink、幂等日志写入器、legacy task 回填、archive fail-closed、verdict gate 退役** 都已实现并通过单元测试，大方向正确。
- 但 **两个核心成功标准（SC-2 partial silence 拦截、SC-5 summary 与 final records 一致）实际上未达成**；**repair budget 耗尽升级、`migrate-state` CLI、BDD wire-level 断言也偏离计划**。

**对齐度评分：55%**

直接拿去用，**不能**完美解决原始痛点：4/8 partial silence 仍可能沉默、diagnosis summary 与幂等记录不一致、repair budget 耗尽不会自动升级终止。建议先完成 P0/P1 的闭环再标记 `LOOP_COMPLETE`。
