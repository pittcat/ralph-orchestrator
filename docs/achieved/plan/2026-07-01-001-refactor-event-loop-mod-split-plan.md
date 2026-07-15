# refactor: 拆分 event_loop/mod.rs (12669 行) 为按职责切分的子模块

> **目标**: 将 `crates/ralph-core/src/event_loop/mod.rs` 拆分为按职责切分的子模块,确保 **0 行为回归**,严格按 **串行 / 隔离 / TDD 闭环** 三约束推进。
>
> **Target repo**: ralph-orchestrator
> **Created**: 2026-07-01
> **Last refreshed**: 2026-07-02(同步 repo 状态:`mod.rs` +363 行、precheck_gate_* 模块新增、review_step_state +223 行、ts 5b55a283→b7c885b4)
> **Status**: active
> **depth**: Standard

---

## Problem Frame

`crates/ralph-core/src/event_loop/mod.rs` 现达 **12669 行**(`wc -l` 2026-07-02 实测,以 commit `b7c885b4` HEAD 实测,plan 初版以 `5b55a283` 基线 12306 行,自此 +363 行),仍是项目里最大的单体文件:
- 内含 `impl EventLoop` 80+ 个方法、2 个 `Default`/`TerminationReason` impl、若干 free function
- 已有 6 个空壳占位文件(`prompt.rs` / `dispatch.rs` / `wave.rs` / `workflow_guard.rs` / `process.rs` / `diagnostics.rs`,均为 3 行 `pub mod xxx;`)等待填充
- 已有部分拆出的子模块(`types.rs` ~381 行、`loop_state.rs` 2850 行、`rejection.rs` 1716 行、`review_step_state.rs` 1671 行,`precheck_gate_enforcement.rs` 405 行、`precheck_gate_runner.rs` 514 行、`stage_pipeline.rs` 411 行等)
- 公开 API(`pub use`)已有 6+ 个转发入口(`lifecycle::build_state_ledger_from_env`、`termination_impl::{format_duration, termination_status_text}`、`types::{EventLoop, ProcessedEvents, ...}`、`loop_state::{LINT_CIRCUIT_BREAKER_LIMIT, LoopState, ...}`、`rejection::{...}`、`policy::{build_unified_validation_pipeline, publish_correction_via_context}`、`verdict::{Verdict, VerdictParseError}`、`types::{CompletionStuck, StuckSource}`)
- `lib.rs` 重新导出 `event_loop::{EventLoop, LoopState, ProcessedEvents, TerminationReason, UserPrompt, ...}`

**根因**: 历史 plan(2026-06-10-003 / 2026-06-23-005 / 2026-06-27-001/002 / 2026-06-26)只完成「子模块壳」,impl 块和 free function 仍写回 mod.rs,导致 mod.rs 持续膨胀。**0 行为回归**是硬性要求——任何拆分都要保持公开 API、类型签名、运行时行为完全不变。

**新增约束**: 自 plan 基线 (5b55a283) 以来已新增 `precheck_gate_enforcement` (405 行) 和 `precheck_gate_runner` (514 行) 两个新子模块(对应 `2026-07-02-004-feat-event-emit-precheck-prompt-gate-plan.md` U5/U6)。它们已被 `mod.rs` 注册且经仓库 nextest 验证通过(commit `b7c885b4` 等)。**经源码核对**:
- `precheck_gate_runner.rs:16, 487` 的注释把 `process_parse_result` 和 precheck_runner 描述为"share one entry point"——说明两者并列,而不是 precheck_runner 包装 `process_parse_result`;前者走 precheck 路径,后者走 regular parse 路径。
- **本 plan 严格将 `precheck_gate_*` 视为黑盒**,`process_parse_result` 整段保留在 mod.rs(它属 U4 范畴,但 U4 已限定只迁出 11789-12160 行的辅助方法,不迁出 `process_parse_result` 主干,以避免触碰 precheck_runner 注释所指的共享契约)。

---

## Scope

### In scope

- 把 mod.rs 中按职责可独立切分的代码块迁入 `event_loop/<name>.rs` 已有空壳或新建子模块
- 严格保持 `lib.rs:142-` 公开 API 路径不变(全部通过 `pub use` 转发)
- 严格保持 `event_loop::` 模块路径下的公开 API 路径不变(impl 块方法签名、free function 名称)
- 严格保持运行时行为不变(零回归)

### Out of scope (本 plan 不做)

- 不重构任何方法内部逻辑、不改算法、不改签名、不改可见性
- 不拆 `loop_state.rs`/`rejection.rs`/`review_step_state.rs`/`precheck_gate_enforcement.rs`/`precheck_gate_runner.rs`/`types.rs`/`stage_pipeline.rs`/`policy.rs`/`audit.rs`/`recovery_finalizer.rs`/`repair_flow.rs`/`rejection_kind.rs`/`step_close_obligation.rs`/`idempotent_wiring.rs`/`legacy_task_relocate.rs`/`plan_blocked_reason.rs`/`repair_stream_sink.rs`/`termination_impl.rs`/`lifecycle.rs`/`verdict.rs`/`termination.rs`/`emit_gate.rs`/`emit_schema_gate.rs`/`flow_declaration.rs` 这些已存在的子模块
- 不改 stage 顺序、不改 stage pipeline wiring、不动 emit gate 机制
- 不动 `precheck_gate_*` 钩子(2026-07-02-004 plan U5/U6 已合并,本 plan 视为黑盒)
- 不改公开 API 路径
- 不动 `event_loop/tests/*`(测试文件保持不动,它们用 `use super::*` 拉 mod.rs 命名空间)
- 不动 `crates/ralph-cli/src/*` 对 event_loop 的引用(`policy_check.rs`、`loop_runner/{runner,hard_gate}.rs`、`tests/{hard_gate,hard_gate_payload_contract,legacy}.rs`)
- 不引入新依赖

---

## Hard Constraints (执行铁律)

### 1. 严格串行执行 (Strictly Sequential)
- **单向流水线**:U1 → U2 → U3 → U4 → U5 → U6 → U7 → U8,**绝对禁止并行**开发
- **绝对前置闭环**:任一 Unit 不达「红 → 绿 → 重构」即不许开始下一 Unit
- 任一 Unit 的失败 = 整个 plan 阻塞,必须先修复当前 Unit 才能继续

### 2. 绝对隔离与零依赖 (Absolute Isolation & Zero Dependency)
- **禁止交叉影响**:每个 Unit 切出的方法集合与其它 Unit **互不相交**(无共享代码块、无共享方法、无交叉调用)
- **禁止前向依赖**:当前 Unit 的测试和运行**绝不依赖**尚未开发的 Unit 的接口
- **自包含运行**:每个 Unit 在 dev/验证期间只需运行该 Unit 自己的 `cargo nextest run` 子集(如 `cargo nextest run -p ralph-core -- event_loop::<submodule_name>`),不需要同时启动其它 Unit 的代码
- 公开 API 路径通过 `pub use` 在 mod.rs 顶部集中转发,所有 Unit 统一遵守,无 Unit 私自添加顶层导出

### 3. 原子化 TDD 驱动 (Atomic TDD)
- **测试先行**:每个 Unit 进入开发前,先在 `<submodule_name>/tests.rs` 写**仅验证当前 Unit**的快照测试
- **红 → 绿 → 重构**:
  1. 复制 mod.rs 中要迁移的代码到新子模块(测试红,因为重复定义编译失败)
  2. 在 mod.rs 中删除原代码,让 `pub use` 转发生效(测试绿,行为不变)
  3. 删除重复定义、修正 use 路径(重构,测试仍绿)
- **无遗留债务**:当前 Unit 测试全绿 = 该 Unit 闭环。不留 cross-Unit 问题给后续 Unit

---

## Key Technical Decisions (KTD)

### KTD-1: 拆分粒度
按**职责内聚**拆,每个子模块代码量目标 **300-1500 行**(避免又出现 > 2000 行的子模块)。按方法语义聚类而非按行号硬切。

### KTD-2: 公开 API 路径完全冻结
拆分**严禁改动** `pub use` 已转发的 API(2026-07-02 实测共 8 处转发,见 `event_loop/mod.rs:71, 78, 94, 102, 115, 121, 125, 128`)。任何 Unit 拆出的新公开符号**必须**通过 `pub use` 在 mod.rs 顶部集中转发,**禁止**在 mod.rs 以外的子模块文件里写顶层 `pub fn` 而绕过 `pub use`。

### KTD-3: impl 块保留在 mod.rs
**所有 `impl EventLoop` 块不迁出** mod.rs。原因是 8 个 Unit 都会引入新方法,迁出会导致 impl 块散落,违反「一个类型一处 impl」的 Rust 惯例(KTD-1: 例外处理)。
> 这是 **本 plan 的核心架构决策**:子模块只装 **free function 与纯数据类型**,`impl EventLoop` 块继续在 mod.rs 中按方法分组。
> 例外:`impl Default for ProcessedEvents`(2026-07-02 行 174)和 `impl TerminationReason`(行 189)可保留在 mod.rs。**`recovery_responder*` accessor 不迁出**(返回 `&self.field`,无对应 free function 等价形态)。

### KTD-4: 私有占位文件清理
下列 6 个空壳私有占位文件按 KTD-1 + KTD-3 不再使用,本 plan **不创建也不填充**它们,确保它们保持当前的 2-3 行 `mod xxx;` 占位状态(它们是 2026-06-10-003 U1 scaffold 留下的占位符,本 plan 不启用):
- `flow_lifecycle.rs` (2 行,占位)
- `loop_state_active.rs` (2 行,占位)
- `loop_state_history.rs` (2 行,占位)
- `rejection_envelope.rs` (2 行,占位)
- `rejection_payload.rs` (2 行,占位)
- `review_step_gate.rs` (2 行,占位)
> **修正**:本 plan **不使用**这些私有占位符(U7 原本计划填 `loop_state_active` 现改为使用已有 `loop_state` 子模块),保持它们原状以免引入额外变更面。

### KTD-5: 测试镜像策略 (Mirror-Then-Delete)
- **红**:在新子模块 `<submodule>.rs` 文件中复制 mod.rs 中要迁移的方法实现(此时会因重复定义编译失败,确认"测试红")
- **绿**:在 mod.rs 中删除原方法,加 `pub use <submodule>::method_name;` 转发,测试应全部继续通过(确认"行为零回归")
- **重构**:清理新子模块的 `use` 路径、删除未使用的 `use`、调整 `pub(super)` 可见性到最小
- 每次状态切换都跑 `cargo nextest run -p ralph-core -- event_loop::<submodule>` 验证

### KTD-6: 测试入口硬约束(项目级 CLAUDE.md 规则继承)
- 所有 `cargo test` 跑测试必须走 `cargo nextest run`(CLAUDE.md HARD RULE 1)
- 验证命令统一为: `cargo nextest run -p ralph-core -- <substring>`
- 完成后跑全量: `./scripts/run-tests.sh`(ralph-core 走并发,其他 6 包走并发)

---

## High-Level Technical Design

### 整体架构图

```
event_loop/
├── mod.rs          ── 保留:pub mod 声明、pub use 转发、impl EventLoop 块
│                     impl Default for ProcessedEvents
│                     impl TerminationReason (exit_code, as_str, is_success)
│                     impl EventLoop { ... 80+ methods, 但按 Unit 拆出部分自由函数 }
│
├── types.rs        ── 已存在 (381 行,不动)
├── loop_state.rs   ── 已存在 (2850 行,不动)
├── rejection.rs    ── 已存在 (1716 行,不动)
├── review_step_state.rs ── 已存在 (1671 行,不动)
├── policy.rs       ── 已存在 (130 行,不动,已是 free function 容器)
├── precheck_gate_enforcement.rs ── 已存在 (405 行,2026-07-02-004 U5,不动)
├── precheck_gate_runner.rs ── 已存在 (514 行,2026-07-02-004 U6,不动)
├── audit.rs        ── 已存在 (188 行,不动)
├── stage_pipeline.rs ── 已存在 (411 行,不动)
├── flow_declaration.rs ── 已存在 (219 行,不动)
├── recovery_finalizer.rs ── 已存在 (264 行,不动)
├── repair_flow.rs  ── 已存在 (223 行,不动)
├── rejection_kind.rs ── 已存在 (197 行,不动)
├── step_close_obligation.rs ── 已存在 (191 行,不动)
├── idempotent_wiring.rs ── 已存在 (154 行,不动)
├── legacy_task_relocate.rs ── 已存在 (140 行,不动)
├── plan_blocked_reason.rs ── 已存在 (130 行,不动)
├── repair_stream_sink.rs ── 已存在 (118 行,不动)
├── termination_impl.rs ── 已存在 (73 行,不动)
├── lifecycle.rs    ── 已存在 (19 行,公开 API 入口,不动)
├── termination.rs  ── 已存在 (152 行,不动)
├── emit_gate.rs    ── 已存在 (131 行,不动)
├── emit_schema_gate.rs ── 已存在 (71 行,不动)
├── verdict.rs      ── 已存在 (330 行,不动)
├── stages/         ── U6+ wiring stages
│
├── prompt.rs       ── [U1 填充] prompt 构造 + 注入系列方法 (~5500-6070 行)
├── dispatch.rs     ── [U2 填充] rejection dispatch helpers (1628-1830 / 5067-5100 行)
├── diagnostics.rs  ── [U3 填充] recovery envelope / diagnosis 写入 (5000-5260 行)
├── process.rs      ── [U4 填充] emit gate / repair 写入辅助 (11789-12160 行)
├── wave.rs         ── [U5 填充] wave context 构造 + isolated scope (5845-6027 / 6354-6385 行)
├── workflow_guard.rs ── [U6 填充] workflow_guard_completion (7543-7600 行)
├── active_hat.rs   ── [U7 填充,新建] active hat detection (~6389-6660 行)
└── flow_step.rs    ── [U8 填充,新建] step transition + advance_plan_step (10949-11290 / 12524- 行)

└── tests/          ── 不动 (2026-07-02 实测 68 个文件,2026-07-01 初版亦为 68;`origin_guard.rs` (45K) 等若干可能在 plan 周期内追加)
    └── ... (count: 68 稳定)
```

### 拆分单元 (Implementation Units)

> **串行执行顺序**:U1 → U2 → U3 → U4 → U5 → U6 → U7 → U8

### U1. 拆分 prompt 子模块

- **Goal**: 把 mod.rs 中 `impl EventLoop` 内 prompt 构造 + 注入相关的 `&self` 方法迁出为 `<submodule>::<func>` 自由函数,接受 `&self` / `&RalphConfig` / `&LoopState` 等显式参数,保留 mod.rs 的 thin-wrapper `impl` 方法以保持调用方不变。
- **Requirements**: 公开 API 路径不变(`EventLoop::build_ralph_prompt` 等仍可在 mod.rs 调用);运行时行为零回归;prompt 字符串输出与拆分前 byte-equal。
- **Dependencies**: 无
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/prompt.rs` (从 3 行占位符扩展到 ~600 行)
  - 创建 `crates/ralph-core/src/event_loop/prompt/tests.rs`
  - 修改 `crates/ralph-core/src/event_loop/mod.rs` (迁出约 580 行 + 加 `pub use` 转发)
- **Approach**:
  1. 列出要迁移的 `impl EventLoop` 方法(2026-07-02 HEAD 实际位置):
     - `rebuild_bootstrap_flags_from_recorded_events` (4612-)
     - `update_robot_guidance` (4635-)
     - `persist_guidance_to_scratchpad` (4735-)
     - `apply_robot_guidance` (4868-)
     - `inject_phase_into_prompt` (4940-)
     - `inject_memories_and_tools_skill` (5334-)
     - `inject_custom_auto_skills` (5440-)
     - `recovery_directive_ids_from_events` (5473-)
     - `build_recovery_directives_section` (5508-)
     - `prepend_recovery_directives` (5546-)
     - `prepend_scratchpad` (5573-)
     - `prepend_ready_tasks` (5709-)
     - `prepend_state_files` (5815-)
     - `build_ralph_prompt` (5828-)
     - `prepend_rejection_digest` (5879-)
     - `prepend_correction_and_resume` (5900-)
     - `prepend_orchestrator_context` (5959-)
     - `inject_pending_lint_resume` (6065-)
  2. 把这些方法改写为自由函数 `pub fn build_xxx(&self, ...) -> ...`(签名与 impl 方法一致),迁移到 `prompt.rs`
  3. mod.rs 中保留 `impl EventLoop { pub fn build_ralph_prompt(&self, ...) { prompt::build_ralph_prompt(self, ...) } }` 作为 thin wrapper
  4. mod.rs 顶部加 `pub use prompt::{build_ralph_prompt as _, ...}`(无重命名;若 mod.rs 内 `use prompt::*` 不引入名字冲突,可改为 `pub use prompt::*;` 批量转发)
- **Execution note**: **测试先行**。在 `prompt/tests.rs` 中写 `mod tests { ... #[test] fn output_byte_equal_to_pre_split_baseline() { ... } }` 验证关键 prompt 输出与拆分前一致(用 `include_str!` 锁定 baseline)。
- **Patterns to follow**: 现有 `event_loop/policy.rs` 是 free function 容器,模仿其 `pub use` 模式(`mod.rs:115`)。
- **Test scenarios**:
  - **Happy path**: `build_ralph_prompt` 在 default config 下输出 byte-equal baseline(锁定 `include_str!`)
  - **Happy path**: `prepend_recovery_directives` 在含 3 个 directive 事件时正确注入 `## RECOVERY` 段
  - **Edge case**: `prepend_scratchpad` 在 scratchpad 不存在时仅输出 header 不报错
  - **Edge case**: `inject_phase_into_prompt` 在 phase 为空时返回原 prompt 不变
  - **Error path**: `persist_guidance_to_scratchpad` 在 IO 失败时返回 `Err` 不 panic
- **Verification**:
  - `cargo nextest run -p ralph-core -- event_loop::prompt` 全绿
  - `cargo nextest run -p ralph-core -- event_loop::tests::build_prompt` 全绿(已有 build_prompt 测试)
  - `cargo nextest run -p ralph-core -- event_loop::tests::runtime_state_injection` 全绿
  - `cargo nextest run -p ralph-core -- event_loop::tests::origin_guard` 全绿(2026-07-02 已存在的回归锁测试,U1 不能回退)

### U2. 拆分 dispatch 子模块

- **Goal**: 迁出 rejection dispatch helpers(`emit_step_handoff_rejection_side_effects`、`publish_isolated_wave_violation`、`enforce_wave_isolated_scope`)到 `dispatch.rs`。
- **Requirements**: 公开 API 不变;dispatch 行为零回归;`impl EventLoop` 中调用方签名不变。
- **Dependencies**: U1(共享 `event_loop::*` 命名空间;代码块不重叠)
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/dispatch.rs` (从 3 行扩展到 ~300 行)
  - 创建 `crates/ralph-core/src/event_loop/dispatch/tests.rs`
  - 修改 `crates/ralph-core/src/event_loop/mod.rs`
- **Approach**:
  1. 迁移方法(2026-07-02 HEAD 实际位置):
     - `enforce_wave_isolated_scope` (1628-)
     - `publish_isolated_wave_violation` (1732-)
     - `emit_step_handoff_rejection_side_effects` (5067-)
  2. 改写为 `pub fn emit_step_handoff_rejection_side_effects(state: &mut LoopState, ...) -> Result<...>` 等自由函数
  3. mod.rs impl 块保留 thin wrapper `pub fn emit_step_handoff_rejection_side_effects(&mut self, ...) { dispatch::emit_step_handoff_rejection_side_effects(&mut self.state, ...) }`
- **Execution note**: TDD 闭环。每个迁移的 dispatch 函数写一个「输入事件 → 输出 envelope」快照测试。
- **Test scenarios**:
  - **Happy path**: `emit_step_handoff_rejection_side_effects` 在 handoff 事件时产出 `RecoveryJournalEntry` 含正确 hat_id/topic
  - **Happy path**: `publish_isolated_wave_violation` 在 isolated 模式下产出 violation event
  - **Edge case**: `enforce_wave_isolated_scope` 在非 isolated 模式下 no-op 返回 `Ok(())`
  - **Error path**: dispatch 在 violation 状态下返回 `Err` 而非 panic
- **Verification**:
  - `cargo nextest run -p ralph-core -- event_loop::dispatch`
  - `cargo nextest run -p ralph-core -- event_loop::tests::handoff_dispatch`
  - `cargo nextest run -p ralph-core -- event_loop::tests::wave_isolated_scope`

### U3. 拆分 diagnostics 子模块

- **Goal**: 迁出 recovery envelope / diagnosis iteration 写入辅助(`record_recovery_envelope`、`should_dedupe_envelope`、`begin_diagnosis_iteration`、`runtime_recovery_context`、`apply_runtime_recovery_actions`)到 `diagnostics.rs`。
- **Requirements**: 公开 API 不变;envelope 字段顺序/内容零回归。
- **Dependencies**: U2(共享命名空间;代码块不重叠)
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/diagnostics.rs` (从 226 行扩展到 ~400 行)
  - 创建 `crates/ralph-core/src/event_loop/diagnostics/tests.rs`
  - 修改 `crates/ralph-core/src/event_loop/mod.rs`
- **Approach**:
  1. 迁移方法(2026-07-02 HEAD 实际位置):
     - `record_recovery_envelope` (5000-)
     - `should_dedupe_envelope` (5052-)
     - `begin_diagnosis_iteration` (5139-)
     - `runtime_recovery_context` (5163-)
     - `apply_runtime_recovery_actions` (5257-)
     - **不迁出** `recovery_responder` (5146-) / `recovery_responder_mut` (5153-) —— 它们是返回 `&self.field` / `&mut self.field` 的 getter,迁出会无可避免地把 `EventLoop` 字段提升到 `pub(crate)`,引入不必要的可见性放宽(KTD-3 例外)。Goal 段是初版列表,实际拆分时按下表为准。
  2. 自由函数签名:接受 `&mut LoopState`、`&mut DiagnosticsCollector`、`&RecoveryResponder` 等显式参数
  3. mod.rs thin wrapper 不变(对 `recovery_responder*` 直接保留 `impl EventLoop` 方法)
- **Execution note**: TDD。envelope 字段顺序需 byte-equal 锁定。**只迁出纯计算/序列化的 free function**,`recovery_responder*` getter 在 mod.rs 内保留(原因见上)。
- **Test scenarios**:
  - **Happy path**: `record_recovery_envelope` 产出 envelope 字段顺序与 baseline 一致(用 `assert_eq!` + 序列化字符串)
  - **Happy path**: `should_dedupe_envelope` 在重复 envelope_id 时返回 `true`
  - **Edge case**: `runtime_recovery_context` 在无 actions 时返回 `None`
  - **Error path**: `apply_runtime_recovery_actions` 在 IO 失败时记录 WARN 不 panic
- **Verification**:
  - `cargo nextest run -p ralph-core -- event_loop::diagnostics`
  - `cargo nextest run -p ralph-core -- event_loop::tests::recovery_envelope_u7_u8`
  - `cargo nextest run -p ralph-core -- event_loop::tests::drift_integration`

### U4. 拆分 process 子模块

- **Goal**: 迁出 emit gate / repair sink / fix-unit range finding / stage context 构造辅助到 `process.rs`。
- **Requirements**: 公开 API 不变;emit gate 行为零回归;fix-unit range guard 阈值不变。
- **Dependencies**: U3(代码块不重叠,但共享 `mod.rs` 命名空间;U4 不迁出 `process_parse_result` 主干——见 Problem Frame 末尾对 precheck_gate_* 的黑盒边界说明)
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/process.rs` (从 3 行扩展到 ~500 行)
  - 创建 `crates/ralph-core/src/event_loop/process/tests.rs`
  - 修改 `crates/ralph-core/src/event_loop/mod.rs`
- **Approach**:
  1. 迁移方法(2026-07-02 HEAD 实际位置):
     - `build_stage_context_for` (11789-)
     - `push_fix_unit_range_finding` (11866-)
     - `apply_emit_gate_on_validated` (11915-)
     - `evaluate_emit_gate_for_jsonl_event` (11978-)
     - `record_repair_event` (12034-)
     - `write_loop_termination_record` (12056-)
     - `record_stage_rejection` (12082-)
     - `loop_id_label` (12151-)
  2. 自由函数,签名以 `&mut LoopState`、`&StageContext`、`&RalphConfig` 等显式参数
  3. mod.rs thin wrapper
- **Execution note**: TDD。fix-unit range guard 阈值用 `assert_eq!(MAX_FIX_UNIT, 99)` 锁定。
- **Test scenarios**:
  - **Happy path**: `evaluate_emit_gate_for_jsonl_event` 对合规事件返回 `Allow`
  - **Happy path**: `push_fix_unit_range_finding` 对 fix-NN 步骤产生 finding
  - **Edge case**: `record_repair_event` 对 repair topic 事件不写 bus
  - **Error path**: `record_stage_rejection` 在 stage 缺失时记录 WARN
- **Verification**:
  - `cargo nextest run -p ralph-core -- event_loop::process`
  - `cargo nextest run -p ralph-core -- event_loop::tests::u2_publish_emit_gate`
  - `cargo nextest run -p ralph-core -- event_loop::tests::u3_jsonl_emit_gate`
  - `cargo nextest run -p ralph-core -- event_loop::tests::u7_repair_sink_wiring`

### U5. 拆分 wave 子模块

- **Goal**: 迁出 wave context 构造、isolated scope 检查、ephemeral relocation 辅助到 `wave.rs`。
- **Requirements**: 公开 API 不变;wave context JSON 输出与 baseline byte-equal;isolated scope 行为零回归。
- **Dependencies**: U4(代码块不重叠)
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/wave.rs` (从 3 行扩展到 ~400 行)
  - 创建 `crates/ralph-core/src/event_loop/wave/tests.rs`
  - 修改 `crates/ralph-core/src/event_loop/mod.rs`
- **Approach**:
  1. 迁移方法(2026-07-02 HEAD 实际位置):
     - `build_wave_context_for_synthesizer` (5845-)
     - `events_path_for_wave_context` (5861-)
     - `prepend_wave_context` (5869-)
     - `run_ephemeral_isolation` (5984-)
     - `prepend_ephemeral_relocations` (6027-)
     - `build_wave_context_for_synthesizer_if_match` (6354-)
     - `build_wave_context_for_synthesizer_if_match_for_test` (6372-,`pub(crate)` 测试桩)
  2. 自由函数,签名以 `&LoopState`、`&RalphConfig`、`PathBuf` 等显式参数
  3. mod.rs thin wrapper
- **Execution note**: TDD。wave context JSON 用 `serde_json::to_string` 后 `assert_eq!` baseline 锁定。
- **Test scenarios**:
  - **Happy path**: `build_wave_context_for_synthesizer` 在 3 个 wave 时输出 JSON 与 baseline 一致
  - **Happy path**: `run_ephemeral_isolation` 在 ephemeral 启用时记录 relocation
  - **Edge case**: `events_path_for_wave_context` 在 marker 文件缺失时返回 `None`
  - **Edge case**: `build_wave_context_for_synthesizer_if_match` 在 mismatch 时返回 `None`
  - **Error path**: `prepend_ephemeral_relocations` 在 IO 失败时返回原 prompt
- **Verification**:
  - `cargo nextest run -p ralph-core -- event_loop::wave`
  - `cargo nextest run -p ralph-core -- event_loop::tests::wave_context_injection`
  - `cargo nextest run -p ralph-core -- event_loop::tests::wave_context_env_var`
  - `cargo nextest run -p ralph-core -- event_loop::tests::ephemeral_isolation_integration`

### U6. 拆分 workflow_guard 子模块

- **Goal**: 迁出 `check_workflow_guard_completion` 和相关 helper 到 `workflow_guard.rs`。
- **Requirements**: 公开 API 不变;completion guard 行为零回归。
- **Dependencies**: U5(代码块不重叠)
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/workflow_guard.rs` (从 3 行扩展到 ~150 行)
  - 创建 `crates/ralph-core/src/event_loop/workflow_guard/tests.rs`
  - 修改 `crates/ralph-core/src/event_loop/mod.rs`
- **Approach**:
  1. 迁移方法(2026-07-02 HEAD 实际位置):
     - `check_workflow_guard_completion` (7543-)
     - `format_mutation_message`、`mutation_warning_reason`、`warn_on_mutation_evidence`(同行范围内)
  2. 自由函数,接受 `&RalphConfig`、`&Event` 显式参数
  3. mod.rs thin wrapper
- **Execution note**: TDD。completion guard true/false 分支各一个测试。
- **Test scenarios**:
  - **Happy path**: `check_workflow_guard_completion` 在 completion event 时返回 `true`
  - **Happy path**: `warn_on_mutation_evidence` 在有 mutation 时记录 WARN
  - **Edge case**: `check_workflow_guard_completion` 在非 completion event 时返回 `false`
  - **Error path**: `format_mutation_message` 在 `score=None` 时仅格式化 message
- **Verification**:
  - `cargo nextest run -p ralph-core -- event_loop::workflow_guard`
  - `cargo nextest run -p ralph-core -- event_loop::tests::workflow_guard`
  - `cargo nextest run -p ralph-core -- event_loop::tests::serial_lint`

### U7. 拆分 active_hat 子模块(纯 free function: detect_active_hats)

- **Goal**: 迁出 active hat 检测相关纯逻辑(`determine_active_hats`、`determine_active_hat_ids`、`effective_regular_events`、`is_kickoff_or_recovery_event`、`is_system_event`、`is_entrypoint_topic`、`peek_pending_regular_events`、`format_event`、`check_hat_exhaustion`、`record_hat_activations`、`get_active_hat_id`、`check_default_publishes`)到 `active_hat.rs`(新建,因 `loop_state_active.rs` 私有占位符按 KTD-4 保持不动)。
- **Requirements**: 公开 API 不变;hat 激活判定零回归。
- **Dependencies**: U6(代码块不重叠)
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/active_hat.rs`(新建,~400 行)
  - 创建 `crates/ralph-core/src/event_loop/active_hat/tests.rs`
  - 修改 `crates/ralph-core/src/event_loop/mod.rs`(新增 `pub mod active_hat;`)
- **Approach**:
  1. 迁移方法(2026-07-02 HEAD 实际位置):
     - `determine_active_hats` (6389-)
     - `determine_active_hat_ids` (6399-)
     - `effective_regular_events` (6445-)
     - `is_kickoff_or_recovery_event` (6462-)
     - `is_system_event` (6471-)
     - `is_entrypoint_topic` (6493-)
     - `peek_pending_regular_events` (6500-)
     - `format_event` (6514-)
     - `check_hat_exhaustion` (6528-)
     - `record_hat_activations` (6576-)
     - `get_active_hat_id` (6589-)
     - `check_default_publishes` (6631-)
  2. 自由函数,接受 `&[Event]`、`&HatRegistry` 显式参数
  3. mod.rs thin wrapper
- **Execution note**: TDD。hat activation 用 `assert_eq!` 锁定 hat_id 列表。
- **Test scenarios**:
  - **Happy path**: `determine_active_hats` 在 3 个 event 时返回正确 hat 列表
  - **Happy path**: `check_default_publishes` 在缺少 default event 时 emit 缺失事件
  - **Edge case**: `effective_regular_events` 在空 events 时返回 `vec![]`
  - **Edge case**: `is_entrypoint_topic` 在非 entrypoint topic 时返回 `false`
  - **Error path**: `check_hat_exhaustion` 在 exhaustion 时返回 `(true, Some(Event))`
- **Verification**:
  - `cargo nextest run -p ralph-core -- event_loop::active_hat`
  - `cargo nextest run -p ralph-core -- event_loop::tests::active_hat`
  - `cargo nextest run -p ralph-core -- event_loop::tests::deterministic_routing`
  - `cargo nextest run -p ralph-core -- event_loop::tests::default_publishes`

### U8. 拆分 flow_step 子模块

- **Goal**: 迁出 flow step 进度驱动(`drive_step_close_progress`、`drive_step_transition`、`flow_step_total_units`、`count_fix_unit_tasks`、`discharge_obligations_for_accepted`)和 `advance_plan_step` (12524-) free function 到 `flow_step.rs`(新建,因 `flow_lifecycle.rs` 私有占位符按 KTD-4 保持不动)。
- **Requirements**: 公开 API 不变;step transition 行为零回归。
- **Dependencies**: U7(代码块不重叠)
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/flow_step.rs`(新建,~300 行)
  - 创建 `crates/ralph-core/src/event_loop/flow_step/tests.rs`
  - 修改 `crates/ralph-core/src/event_loop/mod.rs`(新增 `pub mod flow_step;`)
- **Approach**:
  1. 迁移方法(2026-07-02 HEAD 实际位置):
     - `drive_step_close_progress` (10949-)
     - `drive_step_transition` (10977-)
     - `flow_step_total_units` (11223-)
     - `count_fix_unit_tasks` (11238-)
     - `discharge_obligations_for_accepted` (11271-)
     - `advance_plan_step` (12524-,已是 free function,加 `pub use` 转发)
  2. 自由函数,签名以 `&mut EventLoop`、`&LoopState` 显式参数
  3. mod.rs thin wrapper
- **Execution note**: TDD。step 推进序列用 `assert_eq!` 锁定 step_id 转换链。
- **Test scenarios**:
  - **Happy path**: `drive_step_transition` 在 transition event 时切换 step_id
  - **Happy path**: `advance_plan_step` 在最后一步返回 `None`
  - **Edge case**: `count_fix_unit_tasks` 在 0 个 fix-unit 时返回 `Some(0)`
  - **Error path**: `discharge_obligations_for_accepted` 在 events 为空时返回 0
- **Verification**:
  - `cargo nextest run -p ralph-core -- event_loop::flow_step`
  - `cargo nextest run -p ralph-core -- event_loop::tests::workflow_guard` (复用 transition 测试)
  - `cargo nextest run -p ralph-core -- event_loop::tests::chain_validation`

---

## Risks & Mitigation

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| 拆分中误改方法签名导致回归 | Medium | High | KTD-5: mirror-then-delete,签名冻结;每个 Unit 都跑 `event_loop::tests::*` 全集(68 个测试文件) |
| `impl EventLoop` 块分散导致 Borrow Checker 报错 | High | Medium | KTD-3: impl 块全部保留在 mod.rs,只迁出 free function,thin wrapper 转发 |
| 公开 API 路径意外变更 | Low | High | KTD-2: 严格冻结 `pub use` 列表;不改 `lib.rs:142-` 导出 |
| 单元间代码意外重叠 | Low | Medium | 串行执行 + 每 Unit 仅迁移指定行号范围;`git diff` 仅显示新子模块文件 + mod.rs 删除段 |
| 测试 snapshot 漂移 | Medium | Medium | `include_str!` 锁定 baseline 时使用 git HEAD 提交 SHA;更新 baseline 必须 explicit PR description |
| 子模块间循环依赖 | Low | High | KTD-1: 子模块只依赖 `crate::` 公开 crate,不互相 import |

---

## System-Wide Impact

- **`crates/ralph-core/src/lib.rs`**:`event_loop::*` 公开 API re-export 行,本 plan 不修改(forwarded through `pub use`)
- **`crates/ralph-cli/src/loop_runner/*`**: 引用 `event_loop::*` 公开 API,本 plan 不修改
- **`crates/ralph-cli/src/policy_check.rs`**: 同上
- **`crates/ralph-cli/src/loop_runner/tests/*`**: 同上
- **`crates/ralph-core/src/event_loop/tests/*`**(2026-07-02 68 个 test 文件): 全部 `use super::*` 拉 mod.rs 命名空间,本 plan 不修改
- **`crates/ralph-core/src/event_loop/precheck_gate_*.rs`**: 2026-07-02-004 plan U5/U6 已落地,本 plan 视为黑盒不触碰,但调用面(`process_parse_result`)需在 U3/U4 边界保持兼容
- **CI 影响**: 无 — `cargo nextest run` 入口不变,只是 mod.rs 文件缩小,新增 6 个子模块(填充原占位)+ 2 个新建子模块(`active_hat.rs` / `flow_step.rs`)
- **下游 preset/event_topology**: 无 — impl 块保留在 mod.rs,运行时行为不变

---

## Open Questions

无 — 所有关键决策(KTD-1~6)已在本 plan 内冻结。

---

## Source & Research

- **当前 mod.rs 尺寸**(2026-07-02 实测,基于 commit `b7c885b4`):
  ```
  12669 crates/ralph-core/src/event_loop/mod.rs                       (+363 / +367 INSERTIONS vs 5b55a283)
   2850 crates/ralph-core/src/event_loop/loop_state.rs                (无变化)
   1716 crates/ralph-core/src/event_loop/rejection.rs                 (无变化)
   1671 crates/ralph-core/src/event_loop/review_step_state.rs         (+223 行,自 1448 起)
  ```
- **空壳占位文件列表**(3 行 `pub mod xxx;`,需 U1/U2/U3/U4/U5/U6 填充):
  ```
    3 event_loop/diagnostics.rs
    3 event_loop/dispatch.rs
    3 event_loop/process.rs
    3 event_loop/prompt.rs
    3 event_loop/wave.rs
    3 event_loop/workflow_guard.rs
  ```
- **新拆出子模块(2026-07-02 plan 期间填充完成,本 plan 视为已存在不再触碰)**:
  ```
    405 event_loop/precheck_gate_enforcement.rs   # 2026-07-02-004 U5
    514 event_loop/precheck_gate_runner.rs        # 2026-07-02-004 U6
  ```
- **既有 / 已完成的子模块**(本 plan 不动):`policy.rs`(130 行 free function 容器)、`types.rs`(381 行)、`stage_pipeline.rs`(411 行)、`flow_declaration.rs`(219 行)、`emit_gate.rs`(131 行)、`emit_schema_gate.rs`(71 行)、`audit.rs`(188 行)、`recovery_finalizer.rs`(264 行)、`repair_flow.rs`(223 行)、`rejection_kind.rs`(197 行)、`step_close_obligation.rs`(191 行)、`idempotent_wiring.rs`(154 行)、`legacy_task_relocate.rs`(140 行)、`plan_blocked_reason.rs`(130 行)、`repair_stream_sink.rs`(118 行)、`termination_impl.rs`(73 行)、`lifecycle.rs`(19 行,公开 API 入口)、`verdict.rs`(330 行)、`termination.rs`(152 行)、`stages/`
- **私有未启用占位文件**(mod.rs 仍以 `mod` 私有声明,2 行,本 plan 严格不启用,保持原状):`flow_lifecycle.rs` / `loop_state_active.rs` / `loop_state_history.rs` / `rejection_envelope.rs` / `rejection_payload.rs` / `review_step_gate.rs`
- **测试目录实测文件数**:`crates/ralph-core/src/event_loop/tests/` 下 **68 个文件**(2026-07-02 实测;plan 初版时亦记录 68,本次刷新未观察到新增文件);所有 test 文件 `use super::*` 拉 mod.rs 命名空间,本 plan 不修改
- **mod.rs 公开 API 路径现状**(2026-07-02 实测,本 plan 冻结不变):
  - `pub use lifecycle::build_state_ledger_from_env;`(mod.rs:71)
  - `pub use termination_impl::{format_duration, termination_status_text};`(mod.rs:78)
  - `pub use loop_state::{LINT_CIRCUIT_BREAKER_LIMIT, LoopState, RejectionDigestEntry, U2_REJECTION_RETRY_LIMIT, WorkflowProgress};`(mod.rs:94)
  - `pub use rejection::{NonRetryableReason, Rejection, RejectionStage, build_task_resume_payload, enrich_task_resume_payload, enrich_task_resume_payload_full, enrich_task_resume_payload_with_stage, extract_reason_code, rejection_from_origin, resolve_target_hat, task_resume_payload_has_required_fields};`(mod.rs:102)
  - `pub use policy::{build_unified_validation_pipeline, publish_correction_via_context};`(mod.rs:115)
  - `pub use types::{EventLoop, ProcessedEvents, ProcessedEventsWithWaves, TerminationReason};`(mod.rs:121)
  - `pub use verdict::{Verdict, VerdictParseError};`(mod.rs:125)
  - `pub use types::{CompletionStuck, StuckSource};`(mod.rs:128)
  - `lib.rs` 重新导出 `event_loop::{EventLoop, LoopState, ProcessedEvents, ProcessedEventsWithWaves, TerminationReason, U2_REJECTION_RETRY_LIMIT, UserPrompt, rejection::*}`(冻结)
- **`mod.rs` 关键 impl 块入口位置**(2026-07-02 实测):
  - `impl Default for ProcessedEvents`: 行 174
  - `impl TerminationReason`: 行 189
  - `impl EventLoop`: 行 567(占主体 ~11700 行,行 12184 附近闭合)
- **CLAUDE.md 引用**:
  - HARD RULE 1 (测试入口必须 nextest)
  - HARD RULE 2 (默认并发)
  - HARD RULE 3 (worktree 复用)
  - 公开 API 稳定性原则(Backwards compatibility doesn't matter — 内部重构允许重命名,但本 plan 主动冻结)
- **commit 基线**:
  - Plan 初版引用 commit `5b55a283`(2026-07-01)→ `wc -l mod.rs` 实测 12306 行(初版 plan 内描述,未在本机复核)
  - 2026-07-02 同步时本机实测 commit `b7c885b4`(HEAD)→ `wc -l mod.rs` 报 12669 行
- **同步上游 commit 摘要**(`5b55a283..b7c885b4`,`git log --oneline` 实测,影响 plan 范围但不动其拆分设计):
  - `cd1ec7a2`  feat(precheck): wire declarative emit gate desugar and failure closure
  - `cb78b051`  merge: integrate precheck prompt gate (opt-in emit LLM gate)
  - `5a58b8ac`  fix(emit): 显式 --policy-check 改为 dry-run(P0-2) + default_publishes 注入事件持久化(P0-3)
  - `bbe1c6da`  feat(step_handoff): U5 wire disk-aware task reload 到 StepHandoffRule
  - 共同影响面:`process_parse_result` 调用 precheck_runner(U3 边界须保持兼容);`record_stage_rejection` / `drive_step_*` / `advance_plan_step` 的运行时行为变更(U4/U8 须以合并后基线为准)

---

## Plan Handoff

执行流程:
1. **预检 (必做)**:执行前重新跑 `git log --oneline 5b55a283..HEAD -- crates/ralph-core/src/event_loop/` 与 `wc -l crates/ralph-core/src/event_loop/mod.rs`,确认本 plan "Source & Research" 段数据与现实一致;若偏差 ±200 行以上,先重新同步该段
2. 创建 worktree:`ralph run --worktree --reuse-worktree --plan docs/plans/2026-07-01-001-refactor-event-loop-mod-split-plan.md`(遵循 CLAUDE.md HARD RULE 3,显式传 `--plan`,复用键 = plan basename `2026-07-01-001-refactor-event-loop-mod-split-plan`)
3. 严格按 U1 → U2 → U3 → U4 → U5 → U6 → U7 → U8 串行执行,每 Unit 通过 `cargo nextest run -p ralph-core -- event_loop::<submodule>` 验证,任一 Unit 全绿后才进入下一 Unit
4. 所有 Unit 完成后跑 `./scripts/run-tests.sh`(全 workspace 基线,符合 CLAUDE.md HARD RULE)
5. 单个 PR 提交(全 8 Unit 一并提交,因代码块互相隔离无 cross-unit conflict;若 PR review 要求拆细可拆为 8 个 commit,每个 Unit 一个 commit)

**交接注意**:
- 接手者必须先读 `2026-07-02-004-feat-event-emit-precheck-prompt-gate-plan.md` 与 `2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md` 两条最近 plan 的状态(它们与本 plan 共享 `mod.rs` 写入点)
- 若在执行中发现 precheck_gate_* 与本 plan 任一 Unit 边界冲突,**首选推迟 U3/U4/U5** 而非修改 precheck_gate_*,因 precheck 已 baseline

---