---
date: 2026-06-27
scope:
  - docs/plans/2026-06-27-001-feat-ralph-orchestrator-mechanism-foundation-plan.md
  - docs/plans/2026-06-27-002-feat-mechanism-foundation-completion-plan.md
reviewer: adversarial-red-team
status: REQUEST_CHANGES
risk_level: HIGH
---

# Mechanism Foundation 对抗性代码审查报告

## 审查概要
- **Commit/PR**: 承接 `2026-06-27-001` / `2026-06-27-002` mechanism foundation 计划
- **审查范围**:
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/event_loop/emit_gate.rs`
  - `crates/ralph-core/src/event_loop/stage_pipeline.rs`
  - `crates/ralph-core/src/event_loop/repair_flow.rs`
  - `crates/ralph-core/src/event_loop/stages/repair_dispatch_stage.rs`
  - `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs`
  - `crates/ralph-core/src/event_loop/stages/verdict_gate_stage.rs`
  - `crates/ralph-core/src/event_loop/stages/archive_version_stage.rs`
  - `crates/ralph-core/src/event_loop/repair_stream_sink.rs`
  - `crates/ralph-core/src/event_loop/idempotent_wiring.rs`
  - `crates/ralph-core/src/event_loop/step_close_obligation.rs`
  - `crates/ralph-core/src/state/idempotent_log.rs`
  - `crates/ralph-core/tests/scenarios/mechanism/foundation/*.yml`
- **总体结论**: **REQUEST_CHANGES**
- **风险等级**: **HIGH**

代码在"已经接线完成"的表象下隐藏了多处**语义欺骗**：gate 被调用但结果未真正阻止非法事件、幂等日志从未启用、声明式 flow 配置不生效。核心 SC-1~SC-6 在当前实现中无法成立。

---

## P0 - 阻断问题

### 1. JSONL ingest 路径对 repair topic 双次扣减 budget
- **位置**: `crates/ralph-core/src/event_loop/mod.rs:8188-8191`、`mod.rs:8786-8792`
- **详细分析**: `process_parse_result` 对同一条 JSONL 事件做**两遍**完整 stage pipeline：
  1. 第一遍 `apply_emit_gate(&event)` 运行 `RepairDispatchStage`，推进 `RepairStateMachine` 并写 repair envelope；
  2. 第二遍 `apply_emit_gate_on_validated(&event)` 在 `validated_events` 上再跑一遍，再次推进同一台状态机。
  由于 `RepairStateMachine` 是 loop 级单例，同一 repair 事件会被扣两次 budget。
- **潜在影响**: 默认 `repair_budget=3` 时，第 2 条 `task.relocate_legacy` 就可能触发 `repair_unrecoverable_after_3_retries`，直接破坏 SC-3。
- **修复建议**: JSONL 路径只做一次 gate；若需 lifecycle tracker 记录，应在第一遍记录副作用并跳过第二遍的状态机推进。
- **验证方式**: 构造连续 2 条 `task.relocate_legacy` 的 JSONL，断言不应在 budget 内触发 exhausted。

### 2. `IdempotentLog` 在运行时从未 `open`，U8 幂等写入整体失效
- **位置**: `crates/ralph-core/src/event_loop/mod.rs:700`、`mod.rs:843`；`crates/ralph-core/src/state/idempotent_log.rs:240`
- **详细分析**: `EventLoop` 把 `idempotent_log` 字段初始化为 `IdempotentLog::disabled()`，**生产代码没有任何路径调用 `IdempotentLog::open`**（全仓库调用仅存在于测试）。`drift/engine.rs`、`diagnostics/mod.rs` 拿到的 `log` 永远是 disabled，所有 `write_recovery` / `write_drift` / `write_task` 变成 no-op。
- **潜在影响**: SC-5（summary 计数 = `_final:true` 记录数）不成立；`_final` 竞争、跨进程重复写入等 2026-06-26 根因未被真正修复。
- **修复建议**: 在 `with_context_and_diagnostics` 拿到 `loop_id` 后调用 `IdempotentLog::open(&ralph_dir, loop_id)`，并把返回的 log 写回 `EventLoop.idempotent_log`。
- **验证方式**: 启动 EventLoop 后断言 `idempotent_log.lock().unwrap().version() != 0`；写 drift 后磁盘出现 `drift:*.jsonl` per-key 文件。

### 3. `FlowDeclaration` 运行时解析路径错误，声明式 flow 完全不生效
- **位置**: `crates/ralph-core/src/event_loop/mod.rs:413-422`；`crates/ralph-core/src/event_loop/flow_declaration.rs:152-162`
- **详细分析**: `build_stage_pipeline_from_config` 把 `RalphConfig` 序列化为 YAML 再喂给 `FlowDeclaration::from_yaml`，但 parser 期望**顶层** `mechanism.flow`。`RalphConfig` / `EventLoopConfig` 根本没有 `mechanism` 字段，BDD scenario 里写的 `event_loop.mechanism.flow` 序列化后也不在顶层。解析失败会静默回退到 `minimal_flow_declaration_yaml()`（空 `steps`）。
- **潜在影响**: `FlowStepScopeStage` 跑在空 flow 上，`unknown step` fail-open + `allowed_emits` 全部失效；U9/U12 形同虚设。
- **修复建议**: 在 `RalphConfig` 新增顶层 `mechanism: MechanismConfig` 字段，或让 parser 同时读取 `event_loop.mechanism.flow`。
- **验证方式**: 启动带 `mechanism.flow` 的 EventLoop 后断言 `stage_pipeline` 内的 `flow.steps` 与配置一致。

---

## P1 - 严重问题

### 4. Step-close obligation 纯逻辑未接入 pipeline
- **位置**: `crates/ralph-core/src/event_loop/step_close_obligation.rs` 存在；`crates/ralph-core/src/event_loop/stages/step_close_obligation_stage.rs` **不存在**
- **详细分析**: U12 的纯逻辑 `required_emit` / `emit_satisfies_obligation` 有单元测试，但没有任何 stage 在运行时调用它们。4/8 partial 后 coordinator 沉默的场景（iter=17）不会被拦截。
- **潜在影响**: SC-2 不成立。
- **修复建议**: 新增 `StepCloseObligationStage` 并在 `StagePipeline` 注册；在 step 切换或 idle 边界检查 obligation。
- **验证方式**: `scenario_replay_2026_06_26` 应能 wire-level 断言 `plan.blocked(reason="4_of_8_partial")` 出现。

### 5. `RepairStateMachine` 是 loop 级而非 per-task
- **位置**: `crates/ralph-core/src/event_loop/repair_flow.rs:131-137`；`crates/ralph-core/src/event_loop/stages/repair_dispatch_stage.rs:89-141`
- **详细分析**: 整个 `EventLoop` 只有一台 `RepairStateMachine`。不同 `task_key` 的 repair 事件共用同一 budget。
- **潜在影响**: 任务 A 的 retry 会耗尽任务 B 的预算，违反 R2 "per-task budget"。
- **修复建议**: `StageContext.repair_state` 改为 `HashMap<String, RepairStateMachine>`，或在 stage 内部按 `task_key` 索引。
- **验证方式**: task_key=A 触发 3 次 retry 后，task_key=B 仍能触发 retry。

### 6. `FlowStepScopeStage` 对 undeclared step fail-open
- **位置**: `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs:64-81`
- **详细分析**: 代码注释明确说"fail-closed 已回滚，因为 30+ unit tests 失败"。任何 hat 把 `current_step` 设为未在 flow 中声明的 id，即可绕过 `allowed_emits`。
- **潜在影响**: flow gate 可被轻易绕过；架构契约违反。
- **修复建议**: 恢复 fail-closed，并更新/修复测试 fixtures。
- **验证方式**: `flow_unknown_emit_rejected.yml` 恢复 `absent_events` 断言并通过。

### 7. Recovery / stage rejection 写入未走 `IdempotentLog`
- **位置**: `crates/ralph-core/src/event_loop/repair_stream_sink.rs:63-80`；`crates/ralph-core/src/event_loop/mod.rs:9562-9597`
- **详细分析**: `RepairStreamSink` 和 `record_stage_rejection` 直接向 `recovery.jsonl` 追加 envelope，格式为 `RecoveryDiagnosisEnvelope`，不是 `IdempotentRecord`。而 `IdempotentLog::replay` 会读取目录下**所有** `.jsonl` 并尝试反序列化为 `IdempotentRecord`。
- **潜在影响**: `replay()` 遇到 `recovery.jsonl` 会解析失败；SC-5 计数不可信；重复 recovery 写入无法去重。
- **修复建议**: 统一把 recovery 写入接到 `IdempotentLog`；或让 `replay` 跳过非 `IdempotentRecord` 格式文件。
- **验证方式**: 在存在 `recovery.jsonl` 时调用 `replay()` 不 panic。

### 8. `archive_state_for_loop` 不递归子目录
- **位置**: `crates/ralph-core/src/event_loop/stages/archive_version_stage.rs:88-105`
- **详细分析**: 只 move workspace 根目录下 `*.jsonl`，不处理 `.ralph/agent/` 等子目录。若 `tasks.jsonl` 位于子目录，旧 loop task 仍会污染新 run。
- **潜在影响**: SC-6 可能不成立。
- **修复建议**: 递归 archive `.ralph/` 全部内容，或至少覆盖 `context.tasks_path()` 所在目录。
- **验证方式**: `worktree_reuse_state_isolation` 断言子目录无旧 `loop_id`。

### 9. `apply_emit_gate` 语义误导
- **位置**: `crates/ralph-core/src/event_loop/mod.rs:9465-9507`
- **详细分析**: 函数名暗示"应用 gate，返回是否接受"，但 `AcceptMainBus`/`AcceptRepairStream`/`Reject` 三种结果都返回 `true`。
- **潜在影响**: 未来调用者极易误以为返回 `true` = 可上 bus。
- **修复建议**: 改名为 `emit_gate_side_effects_and_admit` 或返回明确 enum。
- **验证方式**: 代码审查 + 新增编译期/测试约束。

### 10. archive 与 `IdempotentLog::open` 调用顺序风险
- **位置**: `crates/ralph-core/src/event_loop/stages/archive_version_stage.rs:46-52`；`crates/ralph-core/src/event_loop/mod.rs:534-558`
- **详细分析**: archive 依赖读取旧的 `loop-version.json`；一旦未来某人在 archive 之前调用 `IdempotentLog::open`，旧文件会被覆盖。
- **潜在影响**: 未来改动引入 regression。
- **修复建议**: 在 mod.rs 中显式保证 `archive_state_for_loop` 在 `IdempotentLog::open` 之前，并加 assert/注释。
- **验证方式**: 审查调用顺序；必要时加顺序断言测试。

---

## P2 - 建议

1. **`record_stage_rejection` 中 `iteration.checked_add(0)` 无意义** — `mod.rs:9589`，应直接 `self.state.iteration`。
2. **`StagePipeline::is_terminal` 按名称字符串查找 stage** — `stage_pipeline.rs:222-231`，建议缓存 terminal emits 集合。
3. **`default_required_fields` 硬编码 baseline** — `emit_schema_gate_stage.rs:37-51`，与 schema 文件形成两个 SSOT，建议强制从 `presets/schemas/*.yml` 加载。
4. **BDD scenario 注释与代码行为不符** — 多个 `mechanism/foundation/*.yml` 注释声称 wiring 已生效，实则多数 wire-level 断言被降级。
5. **`write_atomic` 未 `fsync(parent_dir)`** — `idempotent_log.rs:439-453`，计划明确要求目录 dsync，当前只有文件 `sync_all`。
6. **`IdempotentLog::open` 重试逻辑简陋** — `idempotent_log.rs:246-269`，固定 5ms × 5 次，建议使用原子写读避免重试或加指数退避。

---

## 兼容性评估
- **API 变更**: 新增 `mechanism.*` 配置字段尚未加入 `RalphConfig`，当前为**无实际效果的 dead schema**。
- **数据格式变更**: `IdempotentLog` 使用 per-key `{key}.jsonl` 文件，但 `recovery.jsonl` 仍是旧 envelope 格式；两者混在同一目录，未来 `replay()` 会冲突。
- **依赖变更**: 引入 `nix` 用于 Unix `flock`（已存在），Windows 仍缺失 inter-process mutex。
- **回滚安全性**: archive fail-closed（U13）已生效，回滚时旧状态不会被新 loop 读到；但幂等日志未启用，旧 `.ralph/` 数据仍按 legacy 路径解释。

---

## 测试充分性评估
- **新增测试覆盖**: 单元测试覆盖了 `emit_schema_gate`、`repair_flow`、`idempotent_log`（独立模块）、`flow_declaration` 解析等纯逻辑。
- **缺失测试场景**:
  1. JSONL 路径 repair budget 双扣测试；
  2. `IdempotentLog` 在 `EventLoop` 生命周期中真正 open 的集成测试；
  3. `FlowDeclaration` 从 `RalphConfig` 实际加载的端到端测试；
  4. `step_close_obligation` 接入 pipeline 后的 regression scenario；
  5. `recovery.jsonl` 与 `IdempotentLog::replay` 混存的解析测试。
- **回归风险**: 当前 `stage_pipeline_order_*` 测试只能验证 stage 顺序，无法发现上述运行时接线缺失；BDD scenario 多被降级为 scaffold，会静默掩盖 bug。

---

## 对抗性审查声明
> 本审查基于对抗性原则执行，已排查语义欺骗、隐藏副作用、边界漏洞和连锁反应风险。发现的核心问题包括：gate 双扣 budget、幂等日志未启用、声明式 flow 不生效、step-close obligation 未接线、repair 写入未幂等化。代码在当前状态下不满足合入条件。
