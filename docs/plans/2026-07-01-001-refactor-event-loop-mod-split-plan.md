# refactor: 拆分 event_loop/mod.rs (12306 行) 为按职责切分的子模块

> **目标**: 将 `crates/ralph-core/src/event_loop/mod.rs` 拆分为按职责切分的子模块,确保 **0 行为回归**,严格按 **串行 / 隔离 / TDD 闭环** 三约束推进。
>
> **Target repo**: ralph-orchestrator
> **Created**: 2026-07-01
> **Status**: active
> **depth**: Standard

---

## Problem Frame

`crates/ralph-core/src/event_loop/mod.rs` 已达 **12306 行**(`wc -l` 实测,commit `5b55a283` 基线),是项目里最大的单体文件:
- 内含 `impl EventLoop` 80+ 个方法、2 个 `Default`/`TerminationReason` impl、若干 free function
- 已有 13 个空壳占位文件(`prompt.rs`、`dispatch.rs`、`wave.rs` 等只有 2-3 行 `pub mod xxx;`)等待填充
- 已有部分拆出的子模块(`loop_state.rs` 2825 行、`rejection.rs` 1716 行、`review_step_state.rs` 1448 行)但 mod.rs 主体未动
- 公开 API(`pub use`)已有 6 个转发入口(`lifecycle::build_state_ledger_from_env`、`termination_impl::{format_duration, termination_status_text}`、`types::{EventLoop, ProcessedEvents, ...}` 等)
- `lib.rs:141-148` 重新导出 `event_loop::{EventLoop, LoopState, ProcessedEvents, TerminationReason, UserPrompt, ...}`

**根因**: 历史 plan(2026-06-10-003 / 2026-06-23-005 / 2026-06-27-001/002)只完成「子模块壳」,impl 块和 free function 仍写回 mod.rs,导致 mod.rs 持续膨胀。**0 行为回归**是硬性要求——任何拆分都要保持公开 API、类型签名、运行时行为完全不变。

---

## Scope

### In scope

- 把 mod.rs 中按职责可独立切分的代码块迁入 `event_loop/<name>.rs` 已有空壳或新建子模块
- 严格保持 `lib.rs:141-148` 公开 API 路径不变(全部通过 `pub use` 转发)
- 严格保持 `event_loop::` 模块路径下的公开 API 路径不变(impl 块方法签名、free function 名称)
- 严格保持运行时行为不变(零回归)

### Out of scope (本 plan 不做)

- 不重构任何方法内部逻辑、不改算法、不改签名、不改可见性
- 不拆 `loop_state.rs`/`rejection.rs`/`review_step_state.rs` 这些已存在的子模块(它们有各自的演进计划)
- 不改 stage 顺序、不改 stage pipeline wiring、不动 emit gate 机制
- 不改公开 API 路径
- 不动 `event_loop/tests/*`(68 个 test 文件保持不动,它们用 `use super::*` 拉 mod.rs 命名空间)
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
拆分**严禁改动** `pub use` 已转发的 API(见 `event_loop/mod.rs:62, 69, 85-88, 93-98, 106, 112, 116, 119`)。任何 Unit 拆出的新公开符号**必须**通过 `pub use` 在 mod.rs 顶部集中转发,**禁止**在 mod.rs 以外的子模块文件里写顶层 `pub fn` 而绕过 `pub use`。

### KTD-3: impl 块保留在 mod.rs
**所有 `impl EventLoop` 块不迁出** mod.rs。原因是 8 个 Unit 都会引入新方法,迁出会导致 impl 块散落,违反「一个类型一处 impl」的 Rust 惯例(KTD-1: 例外处理)。
> 这是 **本 plan 的核心架构决策**:子模块只装 **free function 与纯数据类型**,`impl EventLoop` 块继续在 mod.rs 中按方法分组。
> 例外:`impl Default for ProcessedEvents`(165-178 行)和 `impl TerminationReason`(180-260 行)可保留在 mod.rs。

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
├── types.rs        ── 已存在 (18532 行,不动)
├── loop_state.rs   ── 已存在 (2825 行,不动)
├── rejection.rs    ── 已存在 (1716 行,不动)
├── review_step_state.rs ── 已存在 (1448 行,不动)
├── policy.rs       ── 已存在 (129 行,不动,已是 free function 容器)
├── types.rs/...    ── 既有子模块不动
│
├── prompt.rs       ── [U1 填充] prompt 构造 + 注入系列方法 (5130-5880 行)
├── dispatch.rs     ── [U2 填充] rejection dispatch helpers (4885-4955 行)
├── diagnostics.rs  ── [U3 填充] recovery envelope / diagnosis 写入 (4818-4980 行)
├── process.rs      ── [U4 填充] emit gate / repair 写入辅助 (11214-11475 行)
├── wave.rs         ── [U5 填充] wave context 构造 + isolated scope (5663-5946 行)
├── workflow_guard.rs ── [U6 填充] workflow_guard_completion (7244-7320 行)
│
└── tests/          ── 不动 (68 个测试文件)
    └── ... (68 files)
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
  - 修改 `crates/ralph-core/src/event_loop/mod.rs` (迁出 ~600 行 + 加 `pub use`)
- **Approach**:
  1. 列出要迁移的 `impl EventLoop` 方法:`prepend_auto_inject_skills`、`inject_memories_and_tools_skill`、`inject_custom_auto_skills`、`rebuild_bootstrap_flags_from_recorded_events`、`update_robot_guidance`、`persist_guidance_to_scratchpad`、`apply_robot_guidance`、`inject_phase_into_prompt`、`apply_runtime_diagnosis_prompt`、`recovery_directive_ids_from_events`、`build_recovery_directives_section`、`prepend_recovery_directives`、`prepend_scratchpad`、`prepend_ready_tasks`、`prepend_state_files`、`build_ralph_prompt`、`prepend_rejection_digest`、`prepend_correction_and_resume`、`prepend_orchestrator_context`、`inject_pending_lint_resume`(行 4430-5883 范围内)
  2. 把这些方法改写为自由函数 `pub fn build_xxx(&self, ...) -> ...`(签名与 impl 方法一致),迁移到 `prompt.rs`
  3. mod.rs 中保留 `impl EventLoop { pub fn build_ralph_prompt(&self, ...) { prompt::build_ralph_prompt(self, ...) } }` 作为 thin wrapper
  4. mod.rs 顶部加 `pub use prompt::{build_ralph_prompt as _, prepend_recovery_directives as _, ...};`(只转发,无重命名,因 impl 块已封装)
- **Execution note**: **测试先行**。在 `prompt/tests.rs` 中写 `mod tests { ... #[test] fn output_byte_equal_to_pre_split_baseline() { ... } }` 验证关键 prompt 输出与拆分前一致(用 `include_str!` 锁定 baseline)。
- **Patterns to follow**: 现有 `event_loop/policy.rs` 已是 free function 容器,模仿其 `pub use` 模式(`mod.rs:106`)。
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

### U2. 拆分 dispatch 子模块

- **Goal**: 迁出 rejection dispatch helpers(`emit_step_handoff_rejection_side_effects`、`publish_isolated_wave_violation`、`publish_terminate_event` 之外的 dispatch 系列辅助)到 `dispatch.rs`。
- **Requirements**: 公开 API 不变;dispatch 行为零回归;`impl EventLoop` 中调用方签名不变。
- **Dependencies**: U1(共享 `event_loop::*` 命名空间,需 U1 先稳定命名空间;但代码块不重叠)
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/dispatch.rs` (从 3 行扩展到 ~300 行)
  - 创建 `crates/ralph-core/src/event_loop/dispatch/tests.rs`
  - 修改 `crates/ralph-core/src/event_loop/mod.rs`
- **Approach**:
  1. 迁移方法:`emit_step_handoff_rejection_side_effects` (4885-4955)、`publish_isolated_wave_violation` (1674-1750)、`enforce_wave_isolated_scope` (1570-1674)
  2. 改写为 `pub fn emit_step_handoff_rejection_side_effects(state: &mut LoopState, ...) -> Result<...>` 等自由函数
  3. mod.rs impl 块保留 `pub fn emit_step_handoff_rejection_side_effects(&mut self, ...) { dispatch::emit_step_handoff_rejection_side_effects(&mut self.state, ...) }` thin wrapper
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

- **Goal**: 迁出 recovery envelope / diagnosis iteration 写入辅助(`record_recovery_envelope`、`should_dedupe_envelope`、`begin_diagnosis_iteration`、`recovery_responder*`、`runtime_recovery_context`、`apply_runtime_recovery_actions`)到 `diagnostics.rs`。
- **Requirements**: 公开 API 不变;envelope 字段顺序/内容零回归。
- **Dependencies**: U2(共享命名空间;代码块不重叠)
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/diagnostics.rs` (从 226 行扩展到 ~400 行)
  - 创建 `crates/ralph-core/src/event_loop/diagnostics/tests.rs`
  - 修改 `crates/ralph-core/src/event_loop/mod.rs`
- **Approach**:
  1. 迁移方法:`record_recovery_envelope` (4818-4870)、`should_dedupe_envelope` (4870-4885)、`begin_diagnosis_iteration` (4957-4964)、`recovery_responder/recovery_responder_mut` (4964-4980)、`runtime_recovery_context` (4981-5075)、`apply_runtime_recovery_actions` (5075-5127)
  2. 自由函数签名:接受 `&mut LoopState`、`&mut DiagnosticsCollector`、`&RecoveryResponder` 等显式参数
  3. mod.rs thin wrapper 不变
- **Execution note**: TDD。envelope 字段顺序需 byte-equal 锁定。
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
- **Dependencies**: U3(代码块不重叠,但共享 `mod.rs` 命名空间)
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/process.rs` (从 3 行扩展到 ~500 行)
  - 创建 `crates/ralph-core/src/event_loop/process/tests.rs`
  - 修改 `crates/ralph-core/src/event_loop/mod.rs`
- **Approach**:
  1. 迁移方法:`build_stage_context_for` (11214-11291)、`push_fix_unit_range_finding` (11291-11334)、`apply_emit_gate_on_validated` (11334-11397)、`evaluate_emit_gate_for_jsonl_event` (11397-11453)、`record_repair_event` (11453-11475)、`write_loop_termination_record` (11475-11501)、`record_stage_rejection` (11501-11570)、`loop_id_label` (11570-11584)
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
  1. 迁移方法:`build_wave_context_for_synthesizer` (5663-5679)、`events_path_for_wave_context` (5679-5687)、`prepend_wave_context` (5687-5697)、`run_ephemeral_isolation` (5802-5845)、`prepend_ephemeral_relocations` (5845-5883)
  2. 自由函数,签名以 `&LoopState`、`&RalphConfig`、`PathBuf` 等显式参数
  3. mod.rs thin wrapper
- **Execution note**: TDD。wave context JSON 用 `serde_json::to_string` 后 `assert_eq!` baseline 锁定。
- **Test scenarios**:
  - **Happy path**: `build_wave_context_for_synthesizer` 在 3 个 wave 时输出 JSON 与 baseline 一致
  - **Happy path**: `run_ephemeral_isolation` 在 ephemeral 启用时记录 relocation
  - **Edge case**: `events_path_for_wave_context` 在 marker 文件缺失时返回 `None`
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
  1. 迁移方法:`check_workflow_guard_completion` (7244-7309) + `format_mutation_message`、`mutation_warning_reason`、`warn_on_mutation_evidence`
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
  1. 迁移方法:`determine_active_hats` (6207-6217)、`determine_active_hat_ids` (6217-6263)、`effective_regular_events` (6263-6280)、`is_kickoff_or_recovery_event` (6280-6289)、`is_system_event` (6289-6311)、`is_entrypoint_topic` (6311-6318)、`peek_pending_regular_events` (6318-6332)、`format_event` (6332-6346)、`check_hat_exhaustion` (6346-6394)、`record_hat_activations` (6394-6407)、`get_active_hat_id` (6407-6449)、`check_default_publishes` (6449-6586)
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

- **Goal**: 迁出 flow step 进度驱动(`drive_step_close_progress`、`drive_step_transition`、`flow_step_total_units`、`count_fix_unit_tasks`、`discharge_obligations_for_accepted`)和 `advance_plan_step` (11943-) free function 到 `flow_step.rs`(新建,因 `flow_lifecycle.rs` 私有占位符按 KTD-4 保持不动)。
- **Requirements**: 公开 API 不变;step transition 行为零回归。
- **Dependencies**: U7(代码块不重叠)
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/flow_step.rs`(新建,~300 行)
  - 创建 `crates/ralph-core/src/event_loop/flow_step/tests.rs`
  - 修改 `crates/ralph-core/src/event_loop/mod.rs`(新增 `pub mod flow_step;`)
- **Approach**:
  1. 迁移方法:`drive_step_close_progress` (10624-10652)、`drive_step_transition` (10652-10691)、`flow_step_total_units` (10691-10706)、`count_fix_unit_tasks` (10706-10739)、`discharge_obligations_for_accepted` (10739-10760)
  2. 自由函数 `advance_plan_step` (11943-) 已存在,迁移到 `flow_step.rs` 并加 `pub use`
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
| 公开 API 路径意外变更 | Low | High | KTD-2: 严格冻结 `pub use` 列表;不改 `lib.rs:141-148` 导出 |
| 单元间代码意外重叠 | Low | Medium | 串行执行 + 每 Unit 仅迁移指定行号范围;`git diff` 仅显示新子模块文件 + mod.rs 删除段 |
| 测试 snapshot 漂移 | Medium | Medium | `include_str!` 锁定 baseline 时使用 git HEAD 提交 SHA;更新 baseline 必须 explicit PR description |
| 子模块间循环依赖 | Low | High | KTD-1: 子模块只依赖 `crate::` 公开 crate,不互相 import |

---

## System-Wide Impact

- **`crates/ralph-core/src/lib.rs:141-148`**: 公开 API 路径冻结,本 plan 不修改(forwarded through `pub use`)
- **`crates/ralph-cli/src/loop_runner/*`**: 引用 `event_loop::*` 公开 API,本 plan 不修改
- **`crates/ralph-cli/src/policy_check.rs`**: 同上
- **`crates/ralph-cli/src/loop_runner/tests/*`**: 同上
- **`crates/ralph-core/src/event_loop/tests/*`**(68 个 test 文件): 全部 `use super::*` 拉 mod.rs 命名空间,本 plan 不修改
- **CI 影响**: 无 — `cargo nextest run` 入口不变,只是 mod.rs 文件缩小
- **下游 preset/event_topology**: 无 — impl 块保留在 mod.rs,运行时行为不变

---

## Open Questions

无 — 所有关键决策(KTD-1~6)已在本 plan 内冻结。

---

## Source & Research

- **既有 modules 列表**(2026-07-02 实测,基于 commit `5b55a283`):
  ```
  12306 crates/ralph-core/src/event_loop/mod.rs
   2850 crates/ralph-core/src/event_loop/loop_state.rs
   1716 crates/ralph-core/src/event_loop/rejection.rs
   1448 crates/ralph-core/src/event_loop/review_step_state.rs
  ```
- **空壳占位文件列表**(3 行 `pub mod xxx;`,需 U1/U2/U4/U5/U6 填充):
  ```
    3 event_loop/diagnostics.rs
    3 event_loop/dispatch.rs
    3 event_loop/process.rs
    3 event_loop/prompt.rs
    3 event_loop/wave.rs
    3 event_loop/workflow_guard.rs
  ```
- **既有 / 已完成的子模块**(本 plan 不动):`policy.rs`(129 行 free function 容器)、`types.rs`(~18K 行)、`stage_pipeline.rs`(411 行)、`flow_declaration.rs`(219 行)、`emit_gate.rs`(131 行)、`emit_schema_gate.rs`(73 行)、`audit.rs`(188 行)、`recovery_finalizer.rs`(264 行)、`repair_flow.rs`(223 行)、`rejection_kind.rs`(197 行)、`step_close_obligation.rs`(191 行)、`idempotent_wiring.rs`(154 行)、`legacy_task_relocate.rs`(140 行)、`plan_blocked_reason.rs`(118 行)、`repair_stream_sink.rs`(118 行)、`termination_impl.rs`(73 行)、`lifecycle.rs`(19 行,公开 API 入口)、`verdict.rs`、`termination.rs`、`stages`
- **私有未启用占位文件**(mod.rs 未声明,2 行,本 plan 严格不启用,保持原状):`flow_lifecycle.rs` / `loop_state_active.rs` / `loop_state_history.rs` / `rejection_envelope.rs` / `rejection_payload.rs` / `review_step_gate.rs`
- **测试目录实测文件数**:`crates/ralph-core/src/event_loop/tests/` 下 **68 个文件**(包含子目录 `common/`、`active_hat.rs` 等);所有 test 文件 `use super::*` 拉 mod.rs 命名空间,本 plan 不修改
- **mod.rs 公开 API 路径现状**(2026-07-02 实测,本 plan 冻结不变):
  - `pub use lifecycle::build_state_ledger_from_env;`(mod.rs:62)
  - `pub use termination_impl::{format_duration, termination_status_text};`(mod.rs:69)
  - `lib.rs:141-148` 重新导出 `event_loop::{EventLoop, LoopState, ProcessedEvents, ProcessedEventsWithWaves, TerminationReason, U2_REJECTION_RETRY_LIMIT, UserPrompt, rejection::*}`(冻结)
- **CLAUDE.md 引用**:
  - HARD RULE 1 (测试入口必须 nextest)
  - HARD RULE 2 (默认并发)
  - HARD RULE 3 (worktree 复用)
  - 公开 API 稳定性原则(Backwards compatibility doesn't matter — 内部重构允许重命名,但本 plan 主动冻结)

---

## Plan Handoff

执行流程:
1. 创建 worktree:`ralph run --worktree --reuse-worktree --plan docs/plans/2026-07-01-001-refactor-event-loop-mod-split-plan.md`(遵循 CLAUDE.md HARD RULE 3,显式传 `--plan`,复用键 = plan basename `2026-07-01-001-refactor-event-loop-mod-split-plan`)
2. 严格按 U1 → U2 → U3 → U4 → U5 → U6 → U7 → U8 串行执行,每 Unit 通过 `cargo nextest run -p ralph-core -- event_loop::<submodule>` 验证
3. 所有 Unit 完成后跑 `./scripts/run-tests.sh`(全 workspace 基线,符合 CLAUDE.md HARD RULE)
4. 单个 PR 提交(全 8 Unit 一并提交,因代码块互相隔离无 cross-unit conflict;若 PR review 要求拆细可拆为 8 个 commit,每个 Unit 一个 commit)

---