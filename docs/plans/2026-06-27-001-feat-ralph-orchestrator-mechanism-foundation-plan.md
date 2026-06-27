---
title: Ralph 编排机制公共底座升级计划
type: feat
status: active
date: 2026-06-27
origin: docs/brainstorms/2026-06-27-ralph-orchestrator-mechanism-foundation-requirements.md
diagnostic_report: docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md
---

# Ralph 编排机制公共底座升级计划

## 概述

将 Ralph 编排引擎从"软提示 + 重试"升级为"声明式流转 + 幂等状态 + 硬契约 + 独立修复"的公共底座，使所有 builtin preset 自动受益，并杜绝 `ce-executor-serial` 近 30 天内 7+ 次复发的同类故障。

**机制 foundation 边界**（thin coordination layer）：只做 emit 门禁、状态幂等、修复预算、flow 声明校验；**不做** agent 决策、LLM-as-judge、新 UI、工作流平台。

---

## 问题框定

诊断报告 `2026-06-26-001`（worktree 复用 + plan 4/8 半完成 + iter=17 拓扑断点）的根因 **不是 hat 内容质量**，而是 Ralph 基座不知道半完成时该 emit 什么、修复何时该停、终止语义谁说了算。

| 诊断根因 | 运行时现象 | 药方 | Unit |
|----------|------------|------|------|
| legacy / 新写入 task 缺 `loop_id` | `TaskWrongLoop { actual_loop: None }` 反复 reject | 回填 + 写入强制 | U3、U8 |
| 无 repair budget，`task.resume` 走主 EventBus | 28 条 recovery 空转 | 独立 repair stream + 预算 | U2、U7 |
| emit 仅 soft-check，drift 只审计 | `plan.blocked(reason="")` 进事件流 | 硬契约 gate | U6 |
| 4/8 半完成无 `on_partial` enforce | iter=17 跳过 review-chain，shipper 硬接管 | 声明式 flow + runtime reject | U5、U9 |
| `verdict_gate` 按 topic 隐式终止 | `report.done` 触发 `loop.terminate(review_failed)` | `terminal_emits` 白名单 | U9.5 |
| recovery 无 `_final` 幂等语义 | summary 计数与磁盘不一致 | 幂等 JSONL | U4、U8 |
| worktree 复用无 state 隔离 | 老 loop task 污染新 run | version + archive | U11 |

**本质**：主链路能跑 4 个 unit，跑不到第 5 个——本计划用 4 个药方（声明式流转 / 幂等状态 / 硬契约 / 独立修复）在 `event_loop`、`preset_lint`、状态文件格式落地，**不改动 hats 业务逻辑**。


---

## 需求追溯

- **R1. 硬契约（药方 3）**：emit 入口必须强制校验 `required_fields`，缺字段的事件不能进入事件流（对应需求 C3.1-C3.5）。
- **R2. 独立修复（药方 4）**：修复流程与正常事件流隔离，带预算与状态机，budget 耗尽自动升级（对应需求 R4.1-R4.5）。
- **R3. 幂等状态（药方 2）**：状态文件从 append-only 日志升级为带幂等键、版本号、final 锁的记录（对应需求 S2.1-S2.5）。
- **R4. 声明式流转（药方 1）**：preset 通过 `mechanism.flow` 显式声明完整流程，运行时按声明校验 emit 合法性（对应需求 F1.1-F1.4）。
- **R5. 零业务逻辑侵入**：不改 hat prompt、不改 `ralph-adapters`、不改 TUI / Web Dashboard；除 U4 的 `fs2` 外不引入新依赖。

**成功标准**：
- SC-1：报告 2026-06-26 的 95% 历史问题不再发生。
- SC-2：4/8 半完成状态必须 emit `review.start` 或 `plan.blocked(reason non-empty)`，不再绕过 review。
- SC-3：修复类空转从 28 次降到 ≤ 3 次。
- SC-4：必填字段缺失在 emit 入口被拦，drift 不再报"必填字段 0%".
- SC-5：`diagnosis-summary.json` 计数与 `_final=true` 记录数一致。
- SC-6：worktree 复用时老 task 自动隔离，不污染新 loop。

### SC 测量命令（覆盖原 SC 定义的不精确性）

每个 SC 必须有可执行的测量命令，避免"通过 / 不通过"靠人工判定：

| SC | 测量命令 | 阈值 |
|---|---|---|
| SC-1 | `cargo nextest run -p ralph-core --test scenarios -- scenario_replay_2026_06_26` | 全绿；模拟报告场景的 recovery_count ≤ 3，drift_finding_count = 0 |
| SC-2 | `grep -c "review.start\|plan.blocked" .ralph/events.jsonl` 在 4/8 完成 iter | ≥ 1（必须出现其中一个） |
| SC-3 | `grep "task.resume" .ralph/recovery.jsonl \| grep -c <task_key>` 任意 task | ≤ `repair_budget`（默认 3）；**注意**：`task.resume` 在 `recovery.jsonl` 中的出现次数定义为 retry 计数（覆盖 P2-2） |
| SC-4 | `grep "_present in 0/" .ralph/drift.jsonl` | = 0（不再有"必填字段 0%" finding） |
| SC-5 | `jq '.recovery_count' .ralph/diagnosis-summary.json` vs `grep -c '"_final":true' .ralph/recovery.jsonl` | 相等 |
| SC-6 | `test ! -e .ralph/tasks.jsonl` 或 `.ralph/tasks.jsonl` 不包含旧 loop_id 字符串 | 通过 |

**LOOP_COMPLETE 前全量基线**（禁止仅跑子集就标记完成）：

```bash
./scripts/run-tests.sh
```

含 `preset_lint`（cli + core）、SSOT byte-equality、`scenarios`（必须用 `run_workflow_guard_scenario`，**禁止** `run_scenario` stub）、doctest。

**SC-3 retry 定义（覆盖 P2-2 模糊）**：
- "retry" = `recovery.jsonl` 中 `task.resume` 事件 payload 含 `task_key=<T>` 的记录数。
- 验证命令：`jq -s '[.[] \| select(.topic=="task.resume") \| select(.payload.task_key=="<T>")] \| length' .ralph/recovery.jsonl`。
- 阈值：≤ `repair_budget`（默认 3）。
- **不**用 `stall_recovery_counts` 字典值，因为该值是 in-memory state，跨进程重启后丢失；`recovery.jsonl` 是磁盘 SSOT。

---

## 范围边界

### 本轮绝对不做

- ❌ 改 hats 业务逻辑（hat prompt、hat_lifecycle.rs、adapters）。
- ❌ 引入新的 crate 依赖（**例外**：U4 并发安全必需的 `fs2`，见 U4 技术设计）。
- ❌ 改 TUI / Web Dashboard。
- ❌ 改 `.ralph/` 目录结构（老记录自动 archive 到子目录，不迁整体结构）。
- ❌ 改 CLI 命令的 clap 定义（只新增 `repair_budget` 等 metadata 字段）。
- ❌ 一键回滚 / undo run。

### 本轮做但不写入验收

- 给 `flow_lifecycle.rs` 补注释与示例。
- 给 `state/idempotent_log.rs` 补完整 rustdoc。
- 给新增 lint 模块写"为何需要这条 lint"的注释。

### 注释/文档验收红线（覆盖 P2-4，原"本轮做但不写入验收"的执行红线）

为防止注释任务成为"永远不做"，每个 Unit 的 Acceptance Criteria 段必须包含以下命令（**全绿才允许 Unit 标记为完成**）：

- **U0 / U1 / U2 / U4 / U5 / U7 / U9 / U9.5 / U11**：完成该 Unit 的所有公共 item 必须有 rustdoc，且：
  ```bash
  cargo doc --no-deps -- -D missing_docs -D rustdoc::broken-intra-doc-links
  ```
  必须全绿。
- **所有 Unit**：`cargo clippy --workspace --all-targets -- -D warnings` 必须全绿（含 pedantic 配置）。
- **所有 Unit**：`cargo fmt --all -- --check` 必须通过。
- **U4 / U5**：`crates/ralph-core/src/state/idempotent_log.rs` / `flow_declaration.rs` 的 module-level rustdoc 必须包含：
  - "为什么需要这条机制"（一段话，引用报告根因）。
  - 至少 1 个示例（代码块）。
  - 跨平台 / 并发语义声明。

**违反任一红线 = Unit 未完成**，即使功能测试通过也不能标记 `[x]`。

### 留到下一轮

- `task.resume` 的 `target_hat` 字段是否升级为强校验。
- `loop_id` 是否升级为 UUID。
- 状态机可视化工具（把 `_transitions[]` 渲染成 dot 图）。

---

## 上下文与研究

### 相关代码与模式

- `crates/ralph-core/src/event_loop/mod.rs`：事件循环主流程，现有 `inject_completion_correction`、stall_recovery、verdict gate 等机制。
- `crates/ralph-core/src/event_loop/policy.rs`：当前 event policy 校验是"建议"层，需升级为硬门禁。
- `crates/ralph-core/src/event_loop/flow_lifecycle.rs`：当前为占位注释，需升级为声明式流转底座。
- `crates/ralph-core/src/execution_contract.rs`：`TaskWrongLoop` 拒绝逻辑，需扩展 legacy task 回填通道。
- `crates/ralph-core/src/preset_lint/mod.rs`：lint 注册入口，新增 4 条机制层 lint 需在此注册。
- `crates/ralph-core/src/task_store.rs`：task 写入路径，需接入幂等日志。
- `crates/ralph-core/src/diagnosis/envelope.rs` + `reporter.rs`：recovery 写入路径，需接入幂等日志。
- `crates/ralph-core/src/drift/engine.rs`：drift 写入路径，需只审计不拦截。
- `presets/schemas/ce-executor-serial.yml`：schema SSOT，需标记 `required_fields` 编译期约束。
- `presets/en/ce-executor-serial.yml`：builtin preset，只加 `mechanism` metadata，不改业务逻辑。

### 机构知识

- `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md`：提出了"三道防线"思路，本轮将同样思路应用到 4 个药方。
- `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md`：progress-steward 概念，本轮扩展为修复主题权限。

### 外部参考

- 无。本项目已有足够本地模式，无需外部研究。

---

## 关键设计决策

1. **先落地硬契约与独立修复，再落声明式流转**：硬契约（R1）和修复状态机（R2）是纯逻辑单元，可快速闭环并立即减少 recovery 空转；声明式流转（R4）依赖前三个药方的稳定语义，放在后面集成（参见 origin: 需求文档 §4 末尾建议）。
2. **每个 Unit 独立可测**：所有底层机制单元（U1-U5）只处理内存数据或临时文件，不依赖事件循环完整运行；集成单元（U6-U11）只验证本单元负责的接线点。
3. **状态文件继续用 JSONL**：不引入 SQLite / sled，通过幂等键 + 版本号 + final 锁把日志改造成幂等数据库（参见 origin: 需求文档 §4.2.5）。
4. **drift_monitor 只审计不拦截**：实时拦截全部前移到 emit 入口，drift 退化为事后审计（C3.3）。
5. **修复主题仅限 progress-steward / repair-flow**：其他 hat 不允许 emit `task.relocate*` / `repair.budget.exhausted`（R4.4）。

---

## 开放问题

### 规划中已解决

- **F1.4 的"coordinator 只能 emit 当前 step"是否过严？**
  - 决议：对 `task.resume` 等修复主题走独立 `repair_stream`，不影响 flow step scope；正常业务 emit 仍按 flow step 限制。
- **S2.3 的 archive 目录清理策略？**
  - 决议：自动 archive 到 `.ralph/archive/{old_loop_id}/`，清理策略留待下一轮，本轮只保证读写隔离。
- **R4.4 的扩展点？**
  - 决议：lint 规则中显式列出 allowed_repair_publishers，未来新增 repair publisher 时改 lint + 注册表即可。

### 推迟到实现阶段

- 具体每个 schema 的 `required_fields` 列表（需对照 `presets/schemas/ce-executor-serial.yml` 当前内容精细调整）。
- `flow_declaration` YAML 的精确语法糖（如 `foreach` / `sequence` / `branch` 的字段命名）。
- `repair_stream` 与 `EventBus` 的具体接线方式（是否新增独立 bus 或 Topic 前缀）。

---

## 高层技术设计

> *本图仅用于说明 intended approach，是 review 方向指引，不是实现规范。实现者将其作为上下文，而非可复制的代码。*

```text
                    ┌─────────────────┐
  preset mechanism  │  mechanism.flow │ (declarative steps / allowed_emits)
       metadata ──→ └────────┬────────┘
                             │
                             ▼
  emit hard gate      ┌──────────────┐     repair stream    ┌─────────────┐
  (required_fields)   │ emit_schema  │◄───── isolated ─────┤ repair_flow │
       │              │    _gate     │                     │  state +    │
       ▼              └──────┬───────┘                     │   budget    │
  reject / accept           accept ──► EventBus ──► Hats   └─────────────┘
       │                                                    ▲
       ▼                                                    │
  recovery envelope                                  legacy_task_relocate
  (no events.jsonl)                                  (fill loop_id)

  state files: idempotent_log
    _idempotency_key + _version + _final + _transitions[]
```

---

## 执行约束（本计划特有）

所有开发单元必须遵循 **"纯粹串行、绝对隔离、TDD 闭环"** 模式：

0. **先红 scenario**：U0 之前提交附录 D 的 `scenario_replay_2026_06_26.yml` 与 U6 的 `plan_blocked_reason_required.yml`，确认 scenarios **失败**；runtime fix 的 commit 不得早于 failing scenario commit。
1. **严格串行**：必须按 **U0 → U1 → … → U11** 线性推进，100% 完成前一个 Unit 的编码与测试后才能进入下一个。
2. **绝对隔离**：当前 Unit 是独立孤岛，不依赖未实现的后置 Unit；每个 Unit 所需环境和数据在 Unit 内闭环（内存 / 临时文件 / 内建假数据）。
3. **原子化 TDD**：每个 Unit 必须先写验收测试，测试只能且仅能验证当前 Unit 的输入输出；Unit 测试通过（红 → 绿 → 重构）即代表该 Unit 彻底完结，不留债务给下一个 Unit。
4. **Stage hot path 禁止全量扫 JSONL**：`EmitSchemaGate` / `FlowStepScope` 只做 O(字段数) 或 O(allowed_emits) 校验，不得每 step 重读 `events.jsonl`。

### 接线层 Stage 模型（覆盖原"分支隔离"承诺）

**对接线层硬性约束**：`crates/ralph-core/src/event_loop/mod.rs` 的事件分发路径必须在 U-wiring 之前重构成 Stage pipeline。所有接线型 Unit（U6/U7/U9/U9.5/U11）只能通过 **新增 stage trait 实现 + 在 pipeline 注册表追加一项** 完成接入，**禁止**直接修改 `publish` / `loop_start` 的函数体或 `if/match` 分支。

**Stage 接口契约**（在 U0 中实现，所有 U-* wiring 单元复用）：

```rust
pub trait EmitStage: Send {
    fn name(&self) -> &'static str;
    /// 返回 Ok(()) 放行；返回 Err(reason_code, missing_fields) 拦截
    /// 拦截后由 pipeline 统一写 recovery envelope,不再调用后续 stage
    fn check(&self, ctx: &StageContext, event: &Event) -> Result<(), StageReject>;
}

pub struct StageContext {
    pub current_step: FlowStep,
    pub loop_id: String,
    pub expected_version: u64,
    pub repair_state: &RepairStateMachine,
}

pub struct StagePipeline {
    stages: Vec<Box<dyn EmitStage>>,
}
```

**Stage 顺序锁定**（实现阶段不得变更顺序，如需变更须回到本计划更新）：

| 顺序 | Stage | 对应 Unit | 拒绝时的 reason_code |
|---|---|---|---|
| 1 | `ArchiveVersionStage`（loop 启动时,不在 emit 路径） | U11 | `archive_failed` |
| 2 | `RepairDispatchStage`（early return,不走主 EventBus） | U7 | `repair_dispatch` |
| 3 | `EmitSchemaGateStage` | U6 | `missing_required_fields` |
| 4 | `FlowStepScopeStage` | U9 | `flow_unknown_emit` / `flow_partial_state_undeclared` |
| 5 | `VerdictGateStage` | U9.5 | `verdict_gate_misalignment` |

**为什么必须这样**：
- `RepairDispatchStage` 在 `EmitSchemaGateStage` 之前——修复事件不能被硬契约 gate 拒收，否则 U3 的 legacy task 回填事件被自身 gate 拒。
- `FlowStepScopeStage` 在 `VerdictGateStage` 之前——flow step scope 越步应优先于 verdict gate 语义拦截。
- `ArchiveVersionStage` 是 loop 启动路径（不是 emit 路径），单独走 `EventLoop::on_start` 钩子，不参与 emit pipeline。

**违反约束的回归测试**：每个 U-* wiring 单元完成后，必须跑 `cargo nextest run -p ralph-core -- stage_pipeline_order_*` 验证 stage 顺序未被破坏。

**新增 U0**：`Stage pipeline 骨架（纯逻辑）`——在 U1 之前落地 Stage trait + 空 pipeline + 顺序断言测试。所有 U1-U5 完成后才进入 U-wiring（U6+）。U0 的"Verification"是 `cargo nextest run -p ralph-core -- stage_pipeline_skeleton` 通过。

---

## 实现单元

- [ ] U0. **Stage pipeline 骨架（纯逻辑）**

**目标**：实现 `crates/ralph-core/src/event_loop/stage_pipeline.rs`，定义 `EmitStage` trait + `StagePipeline` 注册表 + 顺序断言，确保所有 U-* wiring 单元只能通过新增 stage 实现接入，不直接改 `publish`。

**需求**：执行约束（接线层 Stage 模型）

**依赖**：无

**文件：**
- 创建：`crates/ralph-core/src/event_loop/stage_pipeline.rs`
- 测试：`crates/ralph-core/src/event_loop/stage_pipeline_tests.rs`
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（新增 `pub mod stage_pipeline`）

**Approach：**
- 定义 `trait EmitStage: Send`，要求 `name()` + `check()` 两方法。
- 定义 `StagePipeline::new(stages: Vec<Box<dyn EmitStage>>)` + `run(ctx, event) -> Result<(), StageReject>`。
- 提供宏 `assert_stage_order!(pipeline, [ArchiveVersion, RepairDispatch, EmitSchemaGate, FlowStepScope, VerdictGate])`，编译期验证顺序。
- 单元测试仅覆盖 stage 顺序与 trait 行为，不接入真实 stage 实现。

**Execution note：** 测试先行，只验证 trait 与顺序。

**技术设计：**
- `StageReject { stage_name: &'static str, reason_code: String, missing_fields: Vec<String> }`
- `StageContext` 含 `current_step: FlowStep`（先 stub）、`loop_id: String`、`expected_version: u64`、`repair_state: &RepairStateMachine`（先 stub）。

**模式遵循：**
- Trait + 注册表模式参考 `crates/ralph-core/src/event_loop/review_step_state.rs` 的状态机风格。

**测试场景：**
- Happy path：3 个空 stage 顺序执行，全部 Accept。
- Happy path：`assert_stage_order!` 宏验证默认顺序合法。
- Error path：任一 stage Reject → 后续 stage 不调用（用计数器 stage 验证）。
- Edge case：空 pipeline（0 个 stage）→ 任何事件都 Accept。
- Error path：阶段顺序与锁定不一致 → 编译期错误（宏）。

**Verification：**
- `cargo nextest run -p ralph-core -- stage_pipeline_skeleton` 全绿。

---

- [ ] U1. **硬契约 emit 门禁（纯逻辑）**

**目标**：实现 emit-time schema 硬校验核心，给定 topic + payload + schema，返回 Accept / Reject。

**需求**：R1

**依赖**：无

**文件：**
- 创建：`crates/ralph-core/src/event_loop/emit_schema_gate.rs`
- 测试：`crates/ralph-core/src/event_loop/emit_schema_gate_tests.rs`
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（新增 `pub mod emit_schema_gate`）

**Approach：**
- 定义 `EmitSchemaGate` 结构，接收 `topic`、`payload`、`required_fields: &[String]`。
- 输出 `EmitDecision::Accept` 或 `Reject(Vec<String>)`（缺失字段列表）。
- 不访问文件系统、不访问 EventBus、不访问 config——纯函数，便于单元测试。

**Execution note：** 测试先行，只验证本模块的输入输出。

**技术设计：**
- `check(payload: &Value, required: &[String]) -> EmitDecision`
- 对非对象 payload 直接 `Reject(vec!["payload must be object"])`。
- 空 `required_fields` 恒接受。

**模式遵循：**
- 参照 `crates/ralph-core/src/execution_contract.rs` 中 `ExecutionContractDecision` 的 Accept/Reject 枚举风格。

**测试场景：**
- Happy path：完整 payload（含所有 required_fields）→ Accept。
- Edge case：空 required_fields 列表 → Accept。
- Edge case：payload 不是 JSON 对象 → Reject。
- Error path：缺 1 个 required field → Reject 且返回缺失字段名。
- Error path：缺多个 required fields → Reject 且返回全部缺失字段名。
- Error path：`reason` 字段存在但为 null → 视为缺失（与 drift 行为一致）。

**Verification：**
- `cargo nextest run -p ralph-core -- emit_schema_gate` 全绿。
- 单元测试仅覆盖 `emit_schema_gate.rs`，不触碰 `mod.rs`。

---

- [ ] U2. **独立修复状态机与预算（纯逻辑）**

**目标**：实现 repair-flow 的独立状态机 + per-task 预算，budget 耗尽产出 `plan.blocked` 决策。

**需求**：R2

**依赖**：无

**文件：**
- 创建：`crates/ralph-core/src/event_loop/repair_flow.rs`
- 测试：`crates/ralph-core/src/event_loop/repair_flow_tests.rs`
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（新增 `pub mod repair_flow`）

**Approach：**
- 定义 `RepairStateMachine`：状态 `Detected → Diagnosing → Fixing → Verifying → Closed`。
- 定义 `RepairBudget { max: u32, consumed: u32 }`，默认 `max = 3`，可被 preset `repair_budget` 覆盖。
- 提供 `transition(action) -> Result<NextState, BudgetExhausted>`。
- 纯内存状态机，不读写文件。

**Execution note：** 测试先行，只验证状态机与预算逻辑。

**技术设计：**
- `RepairBudget::default() -> 3`
- `RepairStateMachine::new(budget) -> Detected`
- `try_transition(&mut self, action: RepairAction) -> RepairTransitionResult`
- `BudgetExhausted` 携带 `reason_code: "repair_unrecoverable_after_N_retries"`。

**模式遵循：**
- 状态机风格参考 `crates/ralph-core/src/event_loop/loop_state_active.rs` 中的状态枚举。

**测试场景：**
- Happy path：`Detected → Diagnosing → Fixing → Verifying → Closed` 一次性成功。
- Happy path：默认 budget 为 3。
- Edge case：budget 可被构造参数覆盖为 5。
- Error path：同一动作重试 3 次后仍失败 → `BudgetExhausted`，reason_code 正确。
- Error path：budget 耗尽后再次 transition → 仍返回 `BudgetExhausted`，不 panic。
- Edge case：`RepairClose` 成功时状态机内部 retry counter 归零。

**Verification：**
- `cargo nextest run -p ralph-core -- repair_flow` 全绿。
- 状态转换图与 budget 行为被单元测试完全覆盖。

---

- [ ] U3. **Legacy task loop_id 回填（文件 I/O）**

**目标**：回填 legacy task 的 `loop_id`；并为 U8 的 task 写入路径定义「无 loop_id 即拒写」契约（诊断 P0：coordinator 早期 task 无身份）。

**需求**：R2、R3（诊断：TaskWrongLoop / P0-A）

**依赖**：无

**文件：**
- 修改：`crates/ralph-core/src/execution_contract.rs`
- 测试：`crates/ralph-core/src/execution_contract_tests.rs`（如不存在则创建）

**Approach：**
- 新增 `relocate_legacy_tasks(tasks_path: &Path, current_loop_id: &str) -> Result<usize, Error>`。
- 只处理 `loop_id` 为 null / 缺失且 `task_key` 匹配当前 plan 前缀的 task。
- 写入幂等：同 task 第二次调用不重复修改。
- 返回成功回填的 task 数量。

**Execution note：** 测试先行，使用临时文件，不依赖真实 worktree。

**技术设计：**
- 读取 `tasks.jsonl`，按行解析 `serde_json::Value`。
- 条件：`loop_id.is_null() || loop_id.as_str().map(|s| s.is_empty()).unwrap_or(true)`。
- 写回临时文件后原子 `rename`。

**模式遵循：**
- 文件写回模式参考 `crates/ralph-core/src/task_store.rs` 中的原子写入。

**测试场景：**
- Happy path：2 个 legacy task + 1 个已有 loop_id 的 task → 回填 2 个，已有 loop_id 不变。
- Edge case：没有 legacy task → 返回 0，文件内容不变。
- Edge case：空 tasks.jsonl → 返回 0。
- Error path：文件不存在 → 返回清晰错误。
- Idempotency：连续调用两次，第二次返回 0 且文件与第一次结果一致。

**Verification：**
- `cargo nextest run -p ralph-core -- relocate_legacy_tasks` 全绿。
- 回填逻辑不依赖 U2 / U7 即可独立运行。

---

- [ ] U4. **幂等日志写入器（文件 I/O，OS 级并发安全）**

**目标**：实现 `idempotent_log.rs`，为 JSONL 状态文件提供幂等键、版本号、final 锁、transition 日志；并发安全通过 **atomic rename** 而非内存 `Mutex` 保证。

**需求**：R3、P0-3（并发安全收紧）

**依赖**：无

**文件：**
- 创建：`crates/ralph-core/src/state/idempotent_log.rs`
- 测试：`crates/ralph-core/src/state/idempotent_log_tests.rs`
- 修改：`crates/ralph-core/src/state/mod.rs`（如存在，新增 `pub mod idempotent_log`）

**Approach：**
- 定义 `IdempotentRecord { _idempotency_key, _version, _final, _created_at, _transitions[], ...payload }`。
- 提供 `IdempotentLog::append(record) -> Result<(), IdempotentError>`。
- 同 key + `_final=true` 二次写入 → `IdempotentError::FinalAlreadySet`。
- 中间状态写入追加 `_transitions` 条目，不覆盖 `_final`。
- 文件 I/O 仅限临时目录测试。

**Execution note：** 测试先行，只验证日志写入语义，不接入真实 task_store / diagnosis。

**技术设计（核心：删除 Mutex，改 atomic rename）**：

> **⚠️ 重要修订**：原计划"使用 `std::sync::Mutex<IdempotentLog>` 包装"是**错误设计**——`Mutex` 只锁当前 op 不锁 check-then-act，多线程并发下会出现"两条 `_final=true` 同 key"竞争。本 Unit 改用 **atomic rename** 协议：

1. **写协议**：
   ```
   fn append(&mut self, record) -> Result<()> {
       // 1. 写临时文件：{path}.{key}.{nonce}.tmp
       write_atomic(tmp_path, record);
       // 2. fsync(parent_dir) 保证元数据落盘
       parent_dir.sync_all()?;
       // 3. rename(tmp_path, {path}.{key})（POSIX 原子）
       rename(tmp_path, key_path)?;
       // 4. 更新 in-memory 索引（已是关键路径外）
       self.index.insert(key, record);
   }
   ```

2. **final 唯一性保证**：
   - 写 `_final=true` 记录前，**先**读 `key_path` 的最后一条记录。
   - 若已是 `_final=true` → 返回 `FinalAlreadySet`。
   - read-modify-write 必须在单个 **OS 文件锁**（`fs2::FileExt::lock_exclusive`）持有期间完成。
   - 锁释放后 `rename`，保证 final 写入的原子性。

3. **`IdempotentLog::open()` 版本钩子**（覆盖 P2-3，下沉到 U4）：
   ```rust
   pub fn open(workspace: &Path, loop_id: &str) -> Result<Self> {
       let version_file = workspace.join(".ralph/loop-version.json");
       let expected_version = if version_file.exists() {
           let persisted: PersistedVersion = serde_json::from_str(&fs::read_to_string(&version_file)?)?;
           if persisted.loop_id != loop_id {
               persisted.version + 1
           } else {
               persisted.version
           }
       } else {
           1
       };
       // 写回 loop-version.json
       fs::write(&version_file, serde_json::to_string(&PersistedVersion { loop_id, version: expected_version })?)?;
       Ok(Self { workspace: workspace.to_path_buf(), loop_id: loop_id.to_string(), version: expected_version })
   }
   ```

4. **跨平台语义**（覆盖 P1-5）：
   - **macOS / Linux**：`std::fs::rename` 原子。
   - **Windows**：`std::fs::rename` **不**原子（返回前需要先 `MOVEFILE_REPLACE_EXISTING`）。本 Unit 在 Windows 下额外调用 `parent_dir.sync_all()` 后再 `rename`，并文档化"Windows runner 下 final 唯一性需在调用方加额外的 inter-process mutex"。
   - `Cargo.toml` 显式声明 `fs2 = "0.4"` 依赖（本计划"不引入新依赖"清单**豁免 fs2**，理由：P0-3 的根因直接由缺失 OS 锁触发，fs2 是 100% 必须）。

**模式遵循：**
- JSONL 追加模式参考 `crates/ralph-core/src/state/ledger.rs`。
- atomic rename 模式参考 `tempfile::NamedTempFile::persist` 的实现思路。

**测试场景：**
- Happy path：写入一条 `_final=true` 记录 → 文件存在且可读。
- Edge case：写入 `_final=false` 后再写同 key 的 `_final=true` → 成功，`_transitions` 保留。
- Error path：同 key 已 `_final=true` 再次写入 → `FinalAlreadySet`。
- Error path：`_idempotency_key` 缺失 → `MissingIdempotencyKey`。
- Edge case：`_version` 递增写入不同 key → 两条记录共存。
- **真线程并发**：spawn 100 个线程同时 `append(_final=true)` 同 key，断言最多 1 个成功、其余 99 个收到 `FinalAlreadySet`（**禁止**用 mock 锁代替真 OS 锁）。
- **真进程并发**：`Command::new(env::current_exe()).arg("idem_concurrent_stress")` 子进程并发写入，主进程监控最终状态唯一。
- **版本钩子**：首次 `open` → version=1；同 loop_id 二次 `open` → version 不变；loop_id 变化 → version+1。
- **跨平台标注**：在 `#[cfg(target_os = "windows")]` 下加注释测试，验证 rename 后的 fsync 序列。

**Acceptance Criteria（覆盖 P2-4）：**
- `cargo doc --no-deps -- -D missing_docs` 对 `state::idempotent_log` 全绿（所有 public item 有 rustdoc）。
- `cargo clippy -- -D warnings` 全绿。
- `cargo nextest run -p ralph-core -- idempotent_log` 全绿。

**Verification：**
- `cargo nextest run -p ralph-core -- idempotent_log` 全绿（含真线程并发测试）。
- `cargo nextest run -p ralph-core -- idempotent_log_concurrent_final` 全绿（spawn 100 线程）。
- `cargo doc --no-deps -- -D missing_docs` 全绿。

---

- [ ] U5. **声明式流转解析与 lint（纯逻辑）**

**目标**：实现 `mechanism.flow` YAML 解析器 + 4 条 flow 相关 lint。

**需求**：R4

**依赖**：无

**文件：**
- 创建：`crates/ralph-core/src/event_loop/flow_declaration.rs`
- 创建：`crates/ralph-core/src/preset_lint/flow_declaration.rs`
- 修改：`crates/ralph-core/src/preset_lint/mod.rs`（注册新 lint）
- 修改：`crates/ralph-core/src/preset_lint/finding_id.rs`（新增 finding ID 常量）
- 测试：各新模块内联 `#[cfg(test)]`

**Approach：**
- `FlowDeclaration` 结构表达 `steps`、`allowed_emits`、`terminal_emits`、`on_partial`。
- lint 模块提供 `check_flow_declaration(config) -> Vec<LintFinding>`，覆盖：
  - `flow_unknown_emit_rejected`：emit 不在 allowed_emits。
  - `flow_partial_state_undeclared`：4/8 等半完成状态未声明 `on_partial`。
  - `flow_terminal_emit_whitelist`：terminal emits 不在声明集合。
  - `flow_hat_step_scope`：coordinator 跨 step emit。
- 本单元只解析 YAML 字符串并产出 lint，不接入事件循环。

**Execution note：** 测试先行，只验证解析器与 lint 规则。

**技术设计：**
- `FlowDeclaration::from_yaml(yaml: &str) -> Result<Self, FlowParseError>`
- lint 函数接收 `RalphConfig`，读取 `mechanism.flow` 段。

**模式遵循：**
- lint 注册与输出格式参考 `crates/ralph-core/src/preset_lint/hat_scope_invariant.rs`。

**测试场景：**
- Happy path：最小合法 flow → 无 finding。
- Happy path：完整 ce-executor-serial flow（unit_loop / review_walk / plan_end / ship）→ 无 finding。
- Edge case：`allowed_emits` 为空列表 → 任何 emit 都触发 `flow_unknown_emit_rejected`。
- Error path：step 4/8 未声明 `on_partial` → `flow_partial_state_undeclared`。
- Error path：`LOOP_COMPLETE` 未在 `terminal_emits` → `flow_terminal_emit_whitelist`。
- Error path：coordinator 在 `review_walk` step emit `work.ready` → `flow_hat_step_scope`。

**Verification：**
- `cargo nextest run -p ralph-core -- flow_declaration` 全绿。
- lint finding ID 在 `finding_id.rs` 中注册并可被外部引用。

---

- [ ] U6. **将硬契约门禁接入 Stage pipeline**

**目标**：实现 `EmitSchemaGateStage`（U1 逻辑的 stage 包装），注册到 pipeline 第 3 位。缺字段事件被 reject 并写 recovery envelope，但不进入 `events.jsonl`。

**需求**：R1

**依赖**：U0、U1

**文件：**
- 创建：`crates/ralph-core/src/event_loop/stages/emit_schema_gate_stage.rs`
- 修改：`crates/ralph-core/src/event_loop/stage_pipeline.rs`（注册新 stage）
- 修改：`crates/ralph-core/src/event_loop/policy.rs`（标记旧软校验为 deprecated）
- 修改：`crates/ralph-core/src/drift/engine.rs`（确认 drift 只审计不拦截）
- 测试：BDD scenario `crates/ralph-core/tests/scenarios/mechanism/foundation/plan_blocked_reason_required.yml`

**Approach：**
- 实现 `struct EmitSchemaGateStage` 包装 U1 的 `emit_schema_gate::check`。
- 在 `StagePipeline::with_default_stages()` 中按锁定顺序注册到第 3 位（ArchiveVersion → RepairDispatch → **EmitSchemaGate** → FlowStepScope → VerdictGate）。
- Reject 时由 pipeline 统一调用 `record_recovery_envelope` 写 recovery；envelope **必须**含 `reason_code`、`stage_name`、`missing_fields`（`grep reason_code .ralph/recovery.jsonl` 可定位 gate）。
- 接线完成后审计 `event_loop/mod.rs` 的 `step-close` / `inject_completion_correction`：与 Stage reject 语义不得分叉（静态 lint 绿、runtime 仍拦 = 未完成）。
- drift engine 不再做实时拦截，只做审计。
- **严格遵守接线层 Stage 模型约束**：本单元不直接改 `publish` / `loop_start` 函数体，只新增 stage 实现 + 在注册表追加一项。

**Execution note：** 测试先行，BDD 只验证"缺 reason 的 plan.blocked 被 emit gate reject"这一接线行为。

**技术设计：**
- `EmitSchemaGateStage.check()` 调用 `emit_schema_gate::check(payload, &schema.required_fields)`，返回 `StageReject { reason_code: "missing_required_fields", missing_fields: [...] }`。

**模式遵循：**
- stage trait 实现参考 `crates/ralph-core/src/event_loop/stage_pipeline.rs`（U0 产物）。

**测试场景：**
- Happy path：`plan.blocked(reason="x")` 通过 gate，进入 EventBus。
- Error path：`plan.blocked(reason="")` 被 reject，recovery.jsonl 有 1 条 envelope，events.jsonl 无该事件。
- Error path：`task.resume` 缺 `kind` 被 reject。
- Integration：drift.jsonl 在 reject 后不出现"必填字段 0%" finding。
- **回归**：`stage_pipeline_order_*` 测试通过（验证本单元未改变锁定顺序）。

**Verification：**
- `cargo nextest run -p ralph-core --test scenarios -- plan_blocked_reason_required` 通过。
- `cargo nextest run -p ralph-core -- emit_schema_gate_stage` 通过。
- `cargo nextest run -p ralph-core -- stage_pipeline_order_*` 通过。

---

- [ ] U7. **将独立修复流程接入 Stage pipeline**

**目标**：实现 `RepairDispatchStage`（U2/U3 的 stage 包装），注册到 pipeline 第 2 位（早于 EmitSchemaGate）。形成独立 repair_stream。

**需求**：R2

**依赖**：U0、U2、U3

**文件：**
- 创建：`crates/ralph-core/src/event_loop/stages/repair_dispatch_stage.rs`
- 修改：`crates/ralph-core/src/event_loop/stage_pipeline.rs`（注册新 stage 到第 2 位）
- 修改：`crates/ralph-core/src/event_loop/loop_state.rs`（`stall_recovery_counts` key 改为 `task_key: String`，与附录 B 的 `task.relocate_legacy.task_key_prefix` 对齐；`repair.close` 时按 `task_key` 索引清零）
- 测试：BDD scenario `crates/ralph-core/tests/scenarios/mechanism/foundation/repair_budget_exhausted_blocks_plan.yml`

**Approach：**
- 实现 `struct RepairDispatchStage`：check 阶段只判断事件是否属于 repair topic（`task.relocate` / `task.relocate_legacy` / `repair.budget.exhausted` / `repair.close`）。
- 属于 repair topic 的事件：early return `Ok(())`，由 pipeline 的"repair sink" 接管（不进入主 EventBus）；同时调用 `repair_state_machine.transition(action)`。
- 不属于 repair topic 的事件：直接放行到下一 stage（EmitSchemaGate）。
- `RepairDispatchStage.check()` 内调用 `relocate_legacy_tasks`（U3），每次 transition 消费 budget。
- 监听 `task.relocate_legacy` 完成后 emit 内部事件 → 由 EmitSchemaGate 校验 `reason` 非空 → FlowStepScope 校验不在主 step 中跨步 emit → VerdictGate 校验最终 emit 语义。
- **严格遵守接线层 Stage 模型约束**：本单元不直接改 `publish` 函数体，只新增 stage 实现 + 在注册表追加一项。
- `RepairDispatchStage` 必须在 `EmitSchemaGateStage` 之前（修复事件不能被硬契约 gate 拒收）。

**Execution note：** 测试先行，BDD 只验证"budget 耗尽 → plan.blocked"与"repair close 清零 counter"。

**技术设计：**
- `RepairDispatchStage.check()` 内调用 `RepairStateMachine::try_transition(action)`；返回 `BudgetExhausted` 时由 stage 返回 `StageReject { reason_code: "repair_unrecoverable_after_N_retries", missing_fields: vec![] }`。
- `task_key` 提取规则：从 `task.relocate_legacy.payload.task_key`（**强校验必填字段，参见附录 B**）直接读取，与 `stall_recovery_counts` 的 key 对齐。`task_key_prefix` 仅作为 hint，不参与索引。

**模式遵循：**
- early return 模式参考 `crates/ralph-core/src/event_loop/policy.rs` 的现有 soft-check。

**测试场景：**
- Happy path：`task.relocate_legacy` 通过本 stage early return，由 repair sink 处理 → `work.done` 通过 contract。
- Happy path：修复成功后 `stall_recovery_counts[task_key]` counter 归零。
- Error path：同 task 修复 3 次失败 → emit `plan.blocked(reason="repair_unrecoverable_after_3_retries")`。
- Integration：repair 事件不进入主 EventBus，hat 订阅不到 repair topic。
- **回归**：`stage_pipeline_order_*` 测试通过（验证 RepairDispatch 在 EmitSchemaGate 之前）。

**Verification：**
- `cargo nextest run -p ralph-core --test scenarios -- repair_budget_exhausted_blocks_plan` 通过。
- `cargo nextest run -p ralph-core -- repair_dispatch_stage` 通过。
- `cargo nextest run -p ralph-core -- stage_pipeline_order_*` 通过。

---

- [ ] U8. **将幂等状态接入 diagnosis / recovery / drift / task_store 写入**

**目标**：让 diagnosis / recovery / drift / task_store 的写入都经过 U4 的 `IdempotentLog`，使 summary 计数基于 `_final=true` 记录。

**需求**：R3

**依赖**：U4

**文件：**
- 修改：`crates/ralph-core/src/task_store.rs`
- 修改：`crates/ralph-core/src/diagnosis/envelope.rs`
- 修改：`crates/ralph-core/src/diagnosis/reporter.rs`
- 修改：`crates/ralph-core/src/drift/engine.rs`
- 测试：BDD scenario `crates/ralph-core/tests/scenarios/diagnosis_count_matches_final_state.yml`

**Approach：**
- 本单元将**所有 JSONL 状态消费者**统一接入 U4 的 `IdempotentLog`。它们共享同一套 key/version/final 契约，因此作为一个原子 wiring unit；每个消费者的改动只是将原 `writeln!` 替换为 `IdempotentLog::append`。
- 定义 idempotency key 生成规则（锁定，U11 直接复用）：
  - task：`task:{task_id}:loop:{loop_id}`
  - recovery：`recovery:{retry_key}:loop:{loop_id}`
  - drift：`drift:{finding_id}:loop:{loop_id}`
- task_store **新写入**必须带非空 `loop_id`（来自 `StageContext.loop_id`）；缺则 `TaskStoreError::MissingLoopId`，不得落盘空字符串（诊断 iter=8/15 空 task_id 根因）。
- task_store 写入 task 时通过 `IdempotentLog`。
- `diagnosis-summary.json` 的 `recovery_count` / `drift_finding_count` 改为读 `_final=true` 的记录数。
- 本单元不处理 worktree 复用与 archive（见 U11）。

**Execution note：** 测试先行，BDD 只验证"summary 计数 = final 记录数"，不验证完整 loop。

**技术设计：**
- `DiagnosisSummary::from_final_records(records: &[IdempotentRecord])`

**模式遵循：**
- JSONL 写入模式参考 `crates/ralph-core/src/state/ledger.rs`。

**测试场景：**
- Error path：写入 task 时 `loop_id` 为空 → reject，不污染 `tasks.jsonl`。
- Happy path：写入 3 条 final recovery → summary.recovery_count = 3。
- Edge case：同一 key 多次 transition 后 final → summary 只计 1 条。
- Error path：`_idempotency_key` 缺失 → 写入 reject。
- Integration：drift engine 只写入审计记录，不拦截事件。

**Verification：**
- `cargo nextest run -p ralph-core --test scenarios -- diagnosis_count_matches_final_state` 通过。
- 单元测试仅覆盖 `task_store` / `diagnosis` / `drift` 写入接线，不覆盖 worktree。

---

- [ ] U9. **将声明式流转接入 Stage pipeline**

**目标**：实现 `FlowStepScopeStage`，注册到 pipeline 第 4 位。校验当前 flow step 与 emit 的 allowed_emits 一致性，拒绝越步 emit。

**需求**：R4

**依赖**：U0、U5、U6、U7

**文件：**
- 创建：`crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs`
- 修改：`crates/ralph-core/src/event_loop/stage_pipeline.rs`（注册新 stage 到第 4 位）
- 修改：`crates/ralph-core/src/event_loop/flow_lifecycle.rs`（从占位升级为 flow 运行时骨架）
- 测试：BDD scenario `crates/ralph-core/tests/scenarios/mechanism/foundation/flow_unknown_emit_rejected.yml`

**Approach：**
- 实现 `struct FlowStepScopeStage`：从 `StageContext.current_step` 读取当前 step，从 `FlowDeclaration.steps[current_step].allowed_emits` 读取白名单。
- 若 emit topic 不在白名单 → `StageReject { reason_code: "flow_unknown_emit", missing_fields: vec![] }`。
- 半完成状态判定：若 `current_step.terminal_when ∈ {all_done, any_failed, partial_units_done}` 中的任意一个，要求 `step.on_partial[terminal_when]` 必须映射到一个非空 emit；否则 `StageReject { reason_code: "flow_partial_state_undeclared", missing_fields: vec![] }`。
- **严格遵守接线层 Stage 模型约束**：本单元不直接改 `publish` 函数体，只新增 stage 实现 + 在注册表追加一项。
- 修复事件不参与 flow step 校验（在 RepairDispatch 已 early return，不会流到这里）。

**Execution note：** 测试先行，BDD 只验证"未知 emit 被 flow runtime reject"与"coordinator 按 on_partial 分支推进"。

**技术设计：**
- `FlowStepScopeStage.check()` 调用 `FlowDeclaration::step_for(current_step).allowed_emits.contains(topic)`。
- 半完成状态判定：精确匹配 `terminal_when ∈ {"all_done", "any_failed", "partial_units_done"}` 视为半完成（参见附录 A 的"半完成状态判定规则"）。

**模式遵循：**
- step 判定参考 `crates/ralph-core/src/event_loop/review_step_state.rs`。

**测试场景：**
- Happy path：coordinator 在 `unit_loop` step emit `work.ready` → 通过。
- Happy path：4/8 完成且 preset 声明 `on_partial: review.start` → coordinator emit `review.start` → 通过。
- Error path：coordinator 在 `review_walk` step emit `work.ready` → reject。
- Error path：`ship` step 之前未出现 `plan.complete` / `plan.blocked` → reject `REVIEW_COMPLETE`。
- Error path：`on_partial.partial_units_done` 映射到空字符串 → `flow_partial_state_undeclared` reject。
- Integration：repair stream 事件不参与 flow step 校验。
- **回归**：`stage_pipeline_order_*` 测试通过（验证 FlowStepScope 在 EmitSchemaGate 之后、VerdictGate 之前）。

**Verification：**
- `cargo nextest run -p ralph-core --test scenarios -- flow_unknown_emit_rejected` 通过。
- `cargo nextest run -p ralph-core -- flow_step_scope_stage` 通过。
- `cargo nextest run -p ralph-core -- stage_pipeline_order_*` 通过。

---

- [ ] U9.5. **verdict_gate 语义对齐（接线合并）**

**目标**：实现 `VerdictGateStage`，注册到 pipeline 第 5 位。消除 verdict_gate 与 shipper routing 的语义错位（报告 P0-C 根因：原 `loop-termination-reason.json` 写 `review_failed.topic=report.done` → verdict_gate 自动接管，导致 shipper 提前接管流程）。

**需求**：R4、P1-3（review 修复）

**依赖**：U0、U9

**文件：**
- 创建：`crates/ralph-core/src/event_loop/stages/verdict_gate_stage.rs`
- 修改：`crates/ralph-core/src/event_loop/stage_pipeline.rs`（注册新 stage 到第 5 位）
- 修改：`crates/ralph-core/src/event_loop/verdict_gate.rs`（从"自动按 topic 接管"改为"按 FlowDeclaration.terminal_emits 显式白名单接管"）
- 测试：BDD scenario `crates/ralph-core/tests/scenarios/mechanism/foundation/verdict_gate_terminal_alignment.yml`

**Approach：**
- 实现 `struct VerdictGateStage`：从 `FlowDeclaration.terminal_emits` 读取白名单（锁定为 `[LOOP_COMPLETE]`）。
- 仅当 emit topic ∈ `terminal_emits` 时，verdict_gate 才接管 → 写 `loop-termination-reason.json` 并标记 loop 终止。
- `report.done` 不再被 verdict_gate 自动接管；report topic 只走 ship step 的正常事件流。
- `REVIEW_COMPLETE` 视为 review_walk 的 terminal emit，**不**进入 `FlowDeclaration.terminal_emits`，仅作为 review_walk 的内部收口。
- **严格遵守接线层 Stage 模型约束**：本单元不直接改 `publish` 函数体，只新增 stage 实现 + 在注册表追加一项 + 改 `verdict_gate.rs` 内部行为。

**Execution note：** 测试先行，BDD 只验证"emit `LOOP_COMPLETE` → verdict_gate 接管"、"emit `report.done` → 不接管"、"emit `REVIEW_COMPLETE` → 在 review_walk 内完成收口"。

**技术设计：**
- `VerdictGateStage.check()` 调用 `flow.terminal_emits.contains(topic)`，返回 `Ok(())`（放行，由后续 sink 接管）或 `StageReject { reason_code: "verdict_gate_misalignment", missing_fields: vec![] }`。
- `terminal_emits` 锁定为 `[LOOP_COMPLETE]`（参见附录 A SSOT）。

**模式遵循：**
- 显式白名单模式参考 `crates/ralph-core/src/event_loop/review_step_state.rs` 的 step 判定。

**测试场景：**
- Happy path：coordinator emit `LOOP_COMPLETE` → verdict_gate 接管，写 termination-reason.json。
- Happy path：shipper emit `report.done` → verdict_gate 不接管，正常进入 ship step。
- Happy path：reviewer emit `REVIEW_COMPLETE` → 在 review_walk 内完成收口，不进入 terminal_emits。
- Error path：未知 topic 试图触发 verdict_gate → `verdict_gate_misalignment` reject。
- **回归**：`stage_pipeline_order_*` 测试通过（验证 VerdictGate 在 FlowStepScope 之后）。

**Verification：**
- `cargo nextest run -p ralph-core --test scenarios -- verdict_gate_terminal_alignment` 通过。
- `cargo nextest run -p ralph-core -- verdict_gate_stage` 通过。
- `cargo nextest run -p ralph-core -- stage_pipeline_order_*` 通过。

---

- [ ] U10. **Preset schema 与 builtin preset metadata 更新**

**目标**：给 `ce-executor-serial` 加 `mechanism.flow` / `repair_budget` / `enforce_schema` metadata，同步 schema `required_fields`，并确保所有 builtin preset 通过新 lint。

**需求**：R4、R5

**依赖**：U5

**文件：**
- 修改：`presets/schemas/ce-executor-serial.yml`
- 修改：`presets/en/ce-executor-serial.yml`
- 修改：`crates/ralph-cli/src/presets.rs`（如嵌入式 preset 内容变化触发 SSOT 校验）
- 修改：`scripts/ralph-zsh-plugin.zsh`（如新增 preset 名则更新，本轮无新 preset，大概率无需修改）
- 修改：`AGENTS.md` 与 `CLAUDE.md`（同步 builtin preset 列表，如内容变化）

**Approach：**
- 在 `presets/en/ce-executor-serial.yml` 顶层追加：
  ```yaml
  mechanism:
    flow:
      type: declared
      steps: [unit_loop, review_walk, plan_end, ship]
    repair_budget: 3
    enforce_schema: hard
    state_idempotency: required
  ```
- 在 `presets/schemas/ce-executor-serial.yml` 中确保 `plan.blocked` / `task.resume` 等 topic 的 `required_fields` 完整。
- 不改动 hat prompt 内容。
- **7 层下游同步**（AGENTS.md HARD RULE）：改 preset/schema 后逐项核对 runtime `event_loop/mod.rs`、`preset_lint/`、scenarios、`loop_config`/`preflight`/`config_resolution`、`presets.rs`、`manifest.yml`/`index.json`、CLAUDE/AGENTS/zsh；U10 PR 描述须勾选触及层。

**Execution note：** 测试先行，先让 lint / SSOT 测试在 metadata 缺失时失败，再加 metadata 使其通过。

**技术设计：**
- `mechanism.flow` 是纯 metadata，不进入 hat prompt。
- `repair_budget` 默认 3，可被覆盖。

**模式遵循：**
- 遵循 AGENTS.md 中"preset yml 改动后必须同步 schema 并跑校验"的 HARD RULE。

**测试场景：**
- Happy path：`cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 全绿。
- Happy path：`cargo nextest run -p ralph-core -- preset_lint` 全绿。
- Happy path：`cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded` 通过（SSOT byte-equality）。
- Error path：故意缺失 `mechanism.flow` → lint / SSOT 测试失败（红 → 绿验证）。

**Verification：**
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 通过。
- `cargo nextest run -p ralph-core -- preset_lint` 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded` 通过。

---

- [ ] U11. **worktree 复用时的状态版本隔离与 archive（loop 启动钩子）**

**目标**：worktree 复用时检测旧 loop version，自动把老记录 archive 到 `.ralph/archive/{old_loop_id}/`，新 loop 从干净版本开始。

**需求**：R3（对应 SC-6）

**依赖**：U0、U4、U8

**文件：**
- 创建：`crates/ralph-core/src/event_loop/stages/archive_version_stage.rs`（实现 `EmitStage` 但不参与 emit 顺序，作为 `EventLoop::on_start` 钩子调用）
- 修改：`crates/ralph-core/src/event_loop/stage_pipeline.rs`（提供 `archive_on_start(workspace, current_loop_id)` 方法，由 `EventLoop::on_start` 调用）
- 修改：`crates/ralph-core/src/worktree.rs`（archive 路径规则）
- 测试：BDD scenario `crates/ralph-core/tests/scenarios/mechanism/foundation/worktree_reuse_state_isolation.yml`

**Approach：**
- 实现 `fn archive_state_for_loop(workspace: &Path, current_loop_id: &str) -> Result<PathBuf, ArchiveError>`。
- 由 `EventLoop::on_start` 在 loop 启动路径调用一次（不是 emit 路径，**不**修改 `publish` 函数体）。
- version 写入时机下沉到 U4 的 `IdempotentLog::open()` 钩子（参见 P2-3）：首次打开时写入 `version=1` 并持久化 `loop_id`；之后每次 loop 启动读取 `loop-version.json`，`loop_id` 不一致时 +1 并 archive。
- archive 子目录命名规则：`{old_loop_id}.{ISO8601 微秒戳}`（参见 P2-5），避免同名 loop_id 二次 archive 覆盖。
- **严格遵守接线层 Stage 模型约束**：本单元不修改 `publish` 函数体，archive 调用走 `on_start` 钩子。

**Execution note：** 测试先行，使用临时目录模拟 worktree 复用，不依赖真实 git worktree。

**技术设计：**
- `archive_state_for_loop(workspace, current_loop_id)`：
  1. 读 `.ralph/loop-version.json`，若不存在 → 首次运行，不 archive。
  2. 若存在且 `persisted.loop_id != current_loop_id`：
     - 创建 `.ralph/archive/{persisted.loop_id}.{ISO8601}/` 目录。
     - `mv` 所有 `.ralph/*.jsonl` 到该目录（不递归）。
     - `IdempotentLog::open()` 后续会写 `version=persisted.version + 1`。
  3. 返回 archive 目录路径，便于诊断。
- 跨平台语义：macOS/Linux `rename` 原子；Windows runner 需 `fsync(parent_dir)` 后再 rename（参见 P1-5）。

**模式遵循：**
- 路径计算参考 `crates/ralph-core/src/worktree.rs`。

**测试场景：**
- Happy path：首次 run → 无 archive，写入 version=1。
- Happy path：复用同 worktree 且 loop_id 变化 → 老记录 archive 到 `.ralph/archive/{old_loop_id}.{ISO8601}/`。
- Edge case：archive 目录已存在 → 追加到 `.{ISO8601}` 子目录，不覆盖。
- Edge case：连续 archive 两次同 loop_id → 两个 `.{ISO8601}` 子目录共存。
- Error path：archive 时 IO 失败 → 返回 `ArchiveError::Io`，loop 不启动。
- Integration：archive 后的 active `.ralph/` 目录不再包含旧 loop_id 的 `*.jsonl` 文件（不验证 task_store 解析，只验证文件系统隔离）。
- **回归**：`stage_pipeline_order_*` 测试通过（验证 ArchiveVersion 不影响 emit 顺序）。

**Verification：**
- `cargo nextest run -p ralph-core --test scenarios -- worktree_reuse_state_isolation` 通过。
- `cargo nextest run -p ralph-core -- archive_state_for_loop` 通过。
- `cargo nextest run -p ralph-core -- stage_pipeline_order_*` 通过。

---

## 系统级影响

- **交互图**（Stage pipeline 视角）：
  ```
  hat emit Event
       │
       ▼
  ┌─────────────────────────────────────────────┐
  │ StagePipeline::run(ctx, event)              │
  │                                              │
  │  ┌─────────────┐  ┌─────────────┐           │
  │  │ RepairDispt │→ │ EmitSchema  │→ FlowStep │→ Verdict → EventBus
  │  │  (early-rt) │  │  Gate       │  Scope     │   Gate     (hat dispatch)
  │  └─────────────┘  └─────────────┘           │
  │       │              │ Reject → recovery.jsonl │
  │       └──────────────┴──────────────────────────┘
  └─────────────────────────────────────────────┘
       │
       └─ EventLoop::on_start → ArchiveVersionStage (loop 启动钩子，不在 emit 路径)
  ```

- **错误传播**：emit 缺字段不再进入事件流，错误以 recovery envelope 形式进入 `recovery.jsonl`；budget 耗尽时直接 emit `plan.blocked` 走终止流程；越步 emit 由 FlowStepScope reject；terminal emit 由 VerdictGate 接管。
- **状态生命周期风险**：幂等键设计错误会导致并发写竞争；U4 用 **atomic rename + fs2 OS 文件锁 + 真线程并发测试** 保证同 key final 唯一（**不再**用 `Mutex<IdempotentLog>`，参见 P0-3）。
- **API 表面对齐**：`presets/en/ce-executor-serial.yml` 新增 `mechanism` 顶层段（附录 A SSOT）；`ralph-cli` 的 embedded preset SSOT 需要同步。
- **集成覆盖**：BDD scenarios 覆盖 emit gate、repair flow、idempotent state、flow runtime、verdict gate、worktree archive 六个接线点（`tests/scenarios/mechanism/foundation/` 子目录）。
- **不变量**：hat prompt 内容、hat 行为实现、`ralph-adapters`、TUI / Web Dashboard、CLI 命令语法均不改变。

---

## 风险与依赖

| 风险 | 可能性 | 影响 | 缓解 |
|---|---|---|---|
| 4 个药方集成时互相干扰 | 中 | 高 | **Stage pipeline 锁定**（U0）+ stage 顺序断言；U6/U7/U9/U9.5/U11 各自只新增 stage 实现，不修改 `publish` 函数体。 |
| 幂等键设计导致并发写竞争 | 中 | 高 | U4 **删 `Mutex`** 改 atomic rename + fs2 OS 文件锁；100 线程真并发测试；真进程并发测试（`Command::new` 子进程）。 |
| Stage pipeline 顺序被无意改坏 | 低 | 高 | `assert_stage_order!` 编译期宏 + `stage_pipeline_order_*` 测试；每次 U-* wiring 完成跑一次。 |
| verdict_gate 与 shipper 语义错位 | 中 | 高 | 新增 U9.5 VerdictGateStage，terminal_emits 锁定 `[LOOP_COMPLETE]`；BDD `verdict_gate_terminal_alignment` 验证。 |
| `build.rs` schema hash 校验让 dev 体验变差 | 中 | 中 | 报错信息需指向 schema 修改指南；U10 包含 SSOT byte-equality 校验。 |
| `progress-steward` 权限扩展破坏现有 preset | 低 | 中 | `repair_budget` 默认 3，lint 警告但不禁用旧 preset；仅 builtin preset 强制要求 `mechanism` 段。 |
| 集成测试依赖真实 worktree | 中 | 中 | worktree 复用场景在 U11 中用临时目录 + mock loop_id 模拟；最终端到端 replay 作为可选验证。 |
| 注释任务成为"永远不做" | 中 | 中 | Acceptance Criteria 红线：`cargo doc --no-deps -- -D missing_docs` 全绿（覆盖 P2-4）。 |
| 运维升级路径缺失导致旧 `.ralph/` 无法读取 | 中 | 高 | 附录 C 提供 migration 脚本 + 回滚步骤（详见下文"运维升级路径"段）。 |
| 跨 Unit 集成测试缺失导致接线点之间失配 | 中 | 高 | 跨 Unit 集成测试章节（详见下文）+ SC-1~SC-6 全部跑通后才允许打 `LOOP_COMPLETE`。 |

---

## 分阶段交付

### 阶段 0：Stage pipeline 骨架（U0）

- U0 Stage pipeline 骨架

本阶段落地"接线层 Stage 模型"硬约束，所有后续 wiring Unit 依赖此骨架。

### 阶段 1：底层机制单元（U1-U5）

- U1 硬契约 emit gate
- U2 修复状态机
- U3 legacy task 回填
- U4 幂等日志（atomic rename + OS 文件锁）
- U5 声明式流转解析 + lint（含半完成状态判定 + reason pattern）

本阶段所有单元均为纯逻辑或文件 I/O，不依赖事件循环完整运行，可独立 review 与合并。

### 阶段 2：事件循环接线（U6-U9.5）

- U6 接入 EmitSchemaGateStage
- U7 接入 RepairDispatchStage
- U8 接入幂等状态写入
- U9 接入 FlowStepScopeStage
- U9.5 接入 VerdictGateStage

本阶段按依赖顺序逐个接线，每个接线单元只新增 stage 实现 + 在注册表追加一项；不改 `publish` 函数体。

### 阶段 3：状态隔离与 Preset 同步（U10-U11）

- U10 更新 schema 与 builtin preset metadata，跑 lint + SSOT 校验。
- U11 接入 worktree 复用时的状态版本隔离与 archive。

**分 PR 建议**（便于 revert，避免 half-old 拓扑）：U0 单独 → U1–U5 可按单元合并 → U6/U7/U8 各一 PR → U9+U9.5 一 PR → U10 单独（touch preset）→ U11 单独。revert 中间 PR 后跑该 PR 的 targeted nextest 仍须绿。

### （可选）U11 之后人工端到端验证

> 本验证**不属于任何 Unit 的 TDD 闭环**，仅作为计划整体 confidence check：在隔离 worktree 中复跑报告 2026-06-26 的 plan（worktree 复用 + 4/8 半完成），确认 `recovery_count ≤ 3`、`drift_finding_count = 0`、verdict_gate topic 语义对齐。若该验证失败，必须回到对应 Unit 修复，而不是打补丁。

---

## 文档与运维说明

- 新增/更新 `docs/solutions/integration-issues/ce-executor-serial-fail-path-verdict-gate-vs-shipper.md`：解释 verdict gate 自动终止与 shipper routing 的关系（对应报告 P0-C，U9.5 落地）。
- 更新 `AGENTS.md` / `CLAUDE.md` 中 builtin preset 列表（仅当内容变化时）。
- 为 preset author 写 `mechanism.flow` 迁移示例（可在 `presets/` 下新增 `mechanism-flow-example.yml` 或写入现有文档）。

（附录 C / D 见文末，按 A → B → C → D 顺序排列）

---

---

## 附录 A：`mechanism.flow` YAML 基线（U5 / U9 / U9.5 / U10 输入，SSOT）

**SSOT 声明**：本附录取代需求文档 §4.1.3 的 YAML 片段。需求文档 §4.1.3 视为草案，本计划落地后该节自动失效。所有 `mechanism.flow` 字段命名 / 嵌套结构 / 取值集合以本附录为准，实现阶段不得变更；如需变更必须回到本计划更新附录并重新走 review。

以下 YAML 作为 `presets/en/ce-executor-serial.yml` 的 `mechanism.flow` 段基线，U5 的解析器、U9 的 FlowStepScopeStage、U9.5 的 VerdictGateStage、U10 的 preset_lint 必须能完整解析并校验它：

```yaml
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        kind: foreach
        over: plan.units
        allowed_emits:
          - work.ready
          - work.done
          - work.failed
          - test.passed
          - test.failed
          - fix.applied
          - fix.exhausted
        body:
          - work.ready
          - work.done | work.failed
          - test.passed | test.failed
          - fix.applied (if test.failed)
        terminal_when: all_done
      - id: review_walk
        kind: sequence
        allowed_emits:
          - review.start
          - review.dimension.ready
          - review.dimension.done
          - review.dimension.failed
          - review.dimensions.complete
          - review.complete
        body:
          - review.start
          - review.dimension.ready × 6
          - review.complete
        emit_when: unit_loop.terminal == all_done
      - id: plan_end
        kind: branch
        allowed_emits:
          - plan.complete
          - plan.blocked
        on_partial:
          all_done: plan.complete
          any_failed: plan.blocked(reason="unit_failed")
          partial_units_done: plan.blocked(reason="partial_units_done")
        on_review_passed: plan.complete
        on_review_failed: plan.blocked(reason="review_failed")
        on_residual: plan.complete(verdict="pass_with_residuals")
      - id: ship
        kind: sequence
        allowed_emits:
          - REVIEW_COMPLETE
          - report.done
          - LOOP_COMPLETE
        body: [REVIEW_COMPLETE, report.done, LOOP_COMPLETE]
  repair_budget: 3
  enforce_schema: hard
  state_idempotency: required
```

**关键字段解释（实现阶段锁定）**：
- `terminal_emits`：loop 终止的合法 emit 集合，**锁定为 `[LOOP_COMPLETE]`**。不在集合中的 emit 由 verdict_gate / flow runtime 拒绝（U9.5 实现）。`REVIEW_COMPLETE` **不**在 `terminal_emits` 内，仅作为 review_walk 的内部收口事件。
- `allowed_emits`：每个 step 内 hat 可 emit 的 topic 白名单。
- `terminal_when`：step 的终结条件，**取值集合固定为 `{all_done, any_failed, partial_units_done}`**（参见下面的"半完成状态判定规则"）。
- `on_partial`：半完成状态必须显式声明分支，缺省 → lint `flow_partial_state_undeclared` reject。
- `on_partial.<key>` 的 value 必须是**非空字符串**（emit topic + 可选参数），空字符串 → lint reject。
- `repair_budget`：默认 3，可被 preset 覆盖。
- `enforce_schema: hard`：emit 入口强制校验 required_fields。
- `state_idempotency: required`：状态文件必须带 `_idempotency_key`。

### 半完成状态判定规则（锁定）

**判定函数**（U5 lint 与 U9 stage 复用）：

```
fn is_partial_state(terminal_when: &str) -> bool {
    matches!(terminal_when, "all_done" | "any_failed" | "partial_units_done")
}
```

**规则**：
1. 若 step 的 `terminal_when ∈ {all_done, any_failed, partial_units_done}` 中的**任意一个**，该 step 被判定为"半完成状态"，必须包含 `on_partial` 键。
2. `on_partial` 必须为非空映射（key → value），key 必须是三个终结状态之一，value 必须是非空字符串（emit 表达式）。
3. 若 `on_partial.<key>` 的 value 是空字符串、或仅含空白字符，lint `flow_partial_state_undeclared` reject。
4. 若 `terminal_when` 取值不在 `{all_done, any_failed, partial_units_done, all_units_done}`（注意：`all_units_done` ≠ `all_done`），**lint 警告但不 reject**（向后兼容旧 preset）。

**lint 行为（U5 `flow_partial_state_undeclared`）**：
- 触发条件：`is_partial_state(step.terminal_when) && step.on_partial.is_none()`。
- 错误消息：`step '<id>' declares terminal_when='<x>' which is a partial state but on_partial is missing or empty`。
- 修复建议：`add on_partial: { all_done: '<emit>', any_failed: '<emit>', partial_units_done: '<emit>' } to step '<id>'`。

### Reason Pattern 校验（U9 stage 收紧，参见 P1-1）

`on_partial.<key>` 的 value 若形如 `plan.blocked(reason="...")`，reason 字符串必须匹配 step 命名的 partial 模式：

| terminal_when | reason 必须包含的子串（不区分大小写） |
|---|---|
| `all_done` | `all_done` 或 `all_units_done` |
| `any_failed` | `unit_failed` 或 `any_failed` |
| `partial_units_done` | `partial` |

运行时校验：`FlowStepScopeStage.check()` 在检测到半完成状态分支 emit 时，提取 `reason` 字段并断言子串匹配；不匹配 → `StageReject { reason_code: "reason_pattern_mismatch", missing_fields: vec![topic] }`。

**示例**：
- `on_partial.partial_units_done: plan.blocked(reason="4_of_8_incomplete_continue_to_review")` → 包含 `partial`，通过。
- `on_partial.partial_units_done: plan.blocked(reason="i_give_up")` → 不包含 `partial`，reject。

---

## 附录 B：`presets/schemas/ce-executor-serial.yml` 字段变更清单（U10 输入）

### 现有 topic 无需新增字段（runtime  enforcement 升级）

| topic | 当前 `required_fields` | 变更 |
|---|---|---|
| `plan.blocked` | `reason` | 无字段变更；U6 在 emit 入口强制校验，空字符串视为缺失。 |
| `task.resume` | `reason`, `target_hat`, `kind` | 无字段变更；U6 在 emit 入口强制校验。 |
| `human.guidance` | `message` | 无字段变更；当 `suppress_human_guidance=true` 时，U6 直接 block emit（C3.5）。 |

### 新增 repair topic（U7 使用）

U10 需在 schema 中追加以下 topic，供 U7 的独立 repair stream 使用。**所有 repair topic 必须包含 `task_key: String` 字段**（覆盖 P1-2），用于 `stall_recovery_counts` 索引清零。

```yaml
  task.relocate:
    required_fields:
      - task_key
      - task_id
      - target_loop_id
      - reason
    payload: json_object

  task.relocate_legacy:
    required_fields:
      - task_key
      - task_key_prefix  # 保留作为 hint；索引实际用 task_key
      - target_loop_id
      - reason
    payload: json_object

  repair.budget.exhausted:
    required_fields:
      - task_key
      - retry_key
      - exhausted_after
      - reason
    payload: json_object

  repair.close:
    required_fields:
      - task_key
      - retry_key
      - reason
    payload: json_object
```

**说明**：
- `task.relocate_legacy` 仅限 `progress-steward` / repair-flow 模块 emit（R4.4）。
- `repair.budget.exhausted` 由 repair state machine 在 budget 耗尽时生成，转换为目标 `plan.blocked`。
- `task_key: String` 与 `stall_recovery_counts: HashMap<String, u32>` 的 key 类型对齐（覆盖 P1-2）。
- `repair.close` 是新事件，由 U7 emit，用于通知 `loop_state` 清零对应 task 的 stall counter。
- 旧 `task.relocate_legacy` 的 `task_key_prefix` 保留为 hint 字段（向后兼容），但**不**作为索引 key。

### `stall_recovery_counts` 数据结构锁定

```rust
// crates/ralph-core/src/event_loop/loop_state.rs
pub type TaskKey = String;
pub struct LoopState {
    pub stall_recovery_counts: HashMap<TaskKey, u32>,
    // ...
}
```

**U7 的清零协议**：
```
fn on_repair_close(&mut self, event: &RepairCloseEvent) {
    if let Some(counter) = self.stall_recovery_counts.get_mut(&event.task_key) {
        *counter = 0;
    }
    // 不存在的 task_key 静默忽略（幂等）
}
```

### Reason Pattern 校验的事件层落地

U9 FlowStepScopeStage 在检测到 emit topic 为 `plan.blocked` 且携带 reason 字段时：
- 读取 `StageContext.current_step.terminal_when`。
- 若属于半完成状态（参见附录 A 判定规则），断言 reason 包含 `terminal_when` 对应的子串（参见附录 A Reason Pattern 表）。
- 不匹配 → `StageReject { reason_code: "reason_pattern_mismatch", missing_fields: vec![event.topic] }`。

---

## 附录 C：运维升级路径（覆盖原"回滚安全"段的不完整性）

### 升级步骤

将本计划版本的 ralph 二进制部署到现有 worktree 时，**必须**先执行以下 migration，否则旧 `*.ralph/*.jsonl` 会因 serde 反序列化失败导致 service 起不来：

```bash
# 1. 停掉所有相关 loop
ralph loops clean --workspace .

# 2. 备份旧 .ralph/
cp -a .ralph .ralph.backup.$(date +%Y%m%d-%H%M%S)

# 3. 跑 migration 工具（U4 实现的一部分）
ralph-cli -- migrate-state --workspace . --output .ralph

# migration 行为：
#   - 读每条旧格式 jsonl 记录（无 _idempotency_key 字段）
#   - 包一层：{ "_idempotency_key": "<file>:<line_hash>:loop:<loop_id>", "_version": "v1", "_final": false, "_created_at": "<file mtime>", "_transitions": [{"from": null, "to": "legacy_migrated"}], ...原 payload }
#   - 写入 .ralph/*.migrated.jsonl
#   - 验证可读后原子 rename .ralph/*.jsonl → .ralph/*.pre-migration.bak

# 4. 启动新版本 ralph
ralph run --plan <plan.md>
```

### 回滚步骤

若新版本二进制出现严重问题需要回滚：

```bash
# 1. 停掉新版本 loop
ralph loops clean --workspace .

# 2. 恢复备份
rm -rf .ralph
mv .ralph.backup.<timestamp> .ralph

# 3. 启动旧版本 ralph
# 注意：旧版本不识别 _idempotency_key 字段，但 serde 默认行为是允许未知字段，所以不会反序列化失败；
# 旧版本的 summary 计数逻辑会读到 _final=true 字段并误报（建议旧版本回滚后只读不写）
```

### 跨平台行为差异

| 平台 | atomic rename | OS 文件锁 (fs2) | migration 行为 |
|---|---|---|---|
| macOS | ✅ 原子 | ✅ | 正常 |
| Linux | ✅ 原子 | ✅ | 正常 |
| Windows | ⚠️ 非原子（需 fsync + rename） | ⚠️ `fs2` 部分支持 | **必须在 README 警告**：Windows runner 下 final 唯一性需调用方加 inter-process mutex |

### CI / CD 集成

`./scripts/run-tests.sh` 完成后，**额外**跑：
```bash
# 验证升级路径在 CI 中可重复
cargo nextest run -p ralph-cli --bin ralph -- migrate_state_roundtrip
# 该测试：旧格式 jsonl → migration → 新版本读取 → 回写 → 旧格式回滚 → 旧版本仍可读
```

---

## 附录 D：跨 Unit 集成测试（覆盖原"测试充分性"段的缺失）

### 跨 Unit 集成测试矩阵（必须在 U11 完成后跑全）

| 测试名 | 覆盖 Unit | 输入 | 验证 |
|---|---|---|---|
| `wiring_composition_emit_to_eventbus` | U0 + U1 + U3 + U5 + U6 + U7 + U9 | `task.relocate_legacy(task_key, target_loop_id, reason="legacy_relocate")` | RepairDispatch early return → U3 relocate_legacy_tasks → work.done 通过 EmitSchemaGate + FlowStepScope → EventBus |
| `wiring_composition_partial_state` | U0 + U5 + U9 | coordinator 在 unit_loop.terminal_when=partial_units_done 时 emit `plan.blocked(reason="4_of_8_partial_done")` | FlowStepScope reason_pattern_mismatch 检查通过；EmitSchemaGate reason 非空通过；正常 emit |
| `wiring_composition_partial_state_reject` | U0 + U5 + U9 | coordinator 同上但 `reason="i_give_up"` | FlowStepScope reject → recovery.jsonl 1 条 envelope，events.jsonl 无 |
| `wiring_composition_budget_exhausted_to_blocked` | U2 + U7 + U9 | 同一 task_key 修复 3 次失败 → 第 4 次 transition | RepairDispatch return BudgetExhausted → emit `plan.blocked(reason="repair_unrecoverable_after_3_retries")` → 通过 EmitSchemaGate（reason 非空）→ FlowStepScope（reason 含 `repair_unrecoverable_after_3_retries` 子串）→ EventBus |
| `wiring_composition_idempotent_final_under_concurrency` | U4 + U8 | 100 线程并发 `append(_final=true)` 同 key | 最多 1 成功 + 99 FinalAlreadySet；最终文件只有 1 条 final 记录 |
| `wiring_composition_worktree_archive_version_bump` | U4 + U8 + U11 | 同 worktree 二次 run（loop_id 不同） | 老 `*.jsonl` archive 到 `.ralph/archive/{old_loop_id}.{ISO8601}/`；`.ralph/loop-version.json` version+1；新写入从 version=2 开始 |
| `wiring_composition_verdict_gate_terminal_alignment` | U9.5 + U10 | shipper emit `report.done`；coordinator emit `LOOP_COMPLETE` | `report.done` 不被 VerdictGate 接管；`LOOP_COMPLETE` 被 VerdictGate 接管并写 `loop-termination-reason.json` |
| `wiring_composition_schema_hash_drift_detected` | U10 + build.rs | 故意改 `presets/schemas/ce-executor-serial.yml` 但不重新生成 Rust 类型 | `cargo build` 失败，错误信息指向 schema 修改指南 |
| `wiring_composition_lint_互斥` | U5 | builtin preset 同时触发 `flow_partial_state_undeclared` 与 `flow_terminal_emit_whitelist` | lint 报告按 finding_id 排序输出，不重复不合并 |

### 跨 Unit 集成测试约束

1. **必须用真 EventLoop runner**（`run_workflow_guard_scenario`），**禁止**用 `run_scenario` stub（2026-06-24 P0-2/P0-3 根因：stub 只查 iterations 数，不断言事件）。
2. **必须跑 100 次重复**，验证非确定性事件排序下断言仍稳定（特别是 U9.5 的 VerdictGate 与 U7 的 RepairDispatch 抢同一事件时）。
3. **必须覆盖 worktree 复用场景**（用临时目录 + mock loop_id，不依赖真实 git worktree）。
4. **必须覆盖 drift engine 退化为审计后的可观测性**——drift.jsonl 在 reject 后无 "必填字段 0%" finding，但其他类型 finding 仍正常产生。

### SC-1 端到端场景回放（覆盖报告 2026-06-26 的 95% 问题）

U11 完成后，**必须**实现一个 scenario 回放：
```yaml
# crates/ralph-core/tests/scenarios/mechanism/foundation/scenario_replay_2026_06_26.yml
name: scenario_replay_2026_06_26
mock_responses:
  # 模拟报告 2026-06-26 的输入序列
  - hat: coordinator
    actions:
      - emit: work.ready  # 4 个 unit 启动
      - ...
      - emit: test.passed  # 4/8 完成
      # 故意不 emit review.start 或 plan.blocked（模拟报告 iter=17 拓扑断点）
expected:
  recovery_count_max: 3
  drift_finding_count_max: 0
  loop_terminate_reason: "verdict_gate_misalignment"  # 强制 verdict_gate 接管
  recovery_envelope_present:
    - topic: "work.ready"  # 4/8 后无 on_partial → reject
    - reason_code: "flow_partial_state_undeclared"
```

---

## 变更记录

| 版本 | 日期 | 说明 |
|------|------|------|
| v1 | 2026-06-27 | 初稿：U0–U11 + Stage pipeline + 附录 A–D |
| v1.1 | 2026-06-27 | 对抗性审查修订：强化问题框定→Unit 映射；U3/U6/U8 补诊断根因；执行约束补 scenario-first + 性能红线；SC 补全量基线 |

---

## 来源与参考

- **需求文档：** [docs/brainstorms/2026-06-27-ralph-orchestrator-mechanism-foundation-requirements.md](docs/brainstorms/2026-06-27-ralph-orchestrator-mechanism-foundation-requirements.md)
- **诊断报告：** [docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md](docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md)
- **相关代码：**
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/event_loop/policy.rs`
  - `crates/ralph-core/src/execution_contract.rs`
  - `crates/ralph-core/src/preset_lint/mod.rs`
  - `presets/en/ce-executor-serial.yml`
  - `presets/schemas/ce-executor-serial.yml`
- **相关解决方案：**
  - `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md`
  - `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md`
