---
title: 全量删除 hat_handoff 功能
type: refactor
status: active
date: 2026-06-23
origin: docs/brainstorms/2026-06-18-isolated-hat-handoff-requirements.md
---

# 全量删除 hat_handoff 功能

## 概述

`hat_handoff` 是 isolated 模式下帽子间宏观边（macro-edge）的磁盘交接文件机制。该功能已在 `ce-executor-serial` 中显式关闭（`hat_handoff.enabled: false`），且是近期 30 天内 6 次 `hat_handoff_filename_mismatch` 死信复发的根源。本计划将其从代码、CLI、校验管线、状态账本、预设、文档中彻底移除，同时**不触碰 `step_handoff`**，确保 `ce-executor-serial` 的计划-进度对齐契约保持不变。

---

## 问题框定

- `hat_handoff` 引入了大量专用代码路径（gate、inject、emit instructions、allocator、validator、CLI 命令、诊断、恢复信封），但业务价值已被 task ownership 取代。
- 当前 active preset 已关闭该功能，代码处于“软废弃”状态，继续保留会增加维护成本和误触发风险。
- 删除范围必须严格限定为 `hat_handoff`，避免波及 `step_handoff`、`workflow_contract` 的通用 `HandoffIndex/HandoffTracker` 以及 session 结束时的 `.ralph/agent/handoff.md`。

---

## 需求追溯

- R1. 删除 `hat_handoff` 运行时 gate、prompt 注入、emit 指令生成与 recovery 路径。
- R2. 保持 `ce-executor-serial` 的运行时行为不变（`hat_handoff.enabled: false` 已是现状）。
- R3. 保持 `step_handoff` 完整，不删除、不关闭、不改配置。
- R4. 删除所有仅服务于 `hat_handoff` 的单元/集成/BDD 测试。
- R5. 删除或更新 `hat_handoff` 相关的 skill 文档、运行手册、脚本、zsh 补全。
- R6. 全量回归通过 `./scripts/run-tests.sh`。

---

## 范围边界

- **在范围内**：`crates/ralph-core/src/hat_handoff/`、`ralph tools handoff`、`ralph audit hat-handoff`、校验管线中的 `HatHandoffRule`、event loop 中的 hat-handoff 分支、状态账本中的 hat-handoff delta、预设/模式中的 `hat_handoff` 配置块、相关测试与文档。
- **不在范围内**：
  - `step_handoff`（`crates/ralph-core/src/step_handoff/`、`validation/rules_step_handoff.rs`、相关测试与配置）。
  - `workflow_contract::HandoffIndex` 与 `HandoffTracker`（通用 WAC-U5 / WRC-U4 基础设施，本计划仅验证后保留）。
  - session 结束时由 `HandoffWriter` 生成的 `.ralph/agent/handoff.md`（属于 loop 会话交接，非 hat 间交接）。
  - `docs/report/`、`docs/achieved/` 中的历史报告与计划（仅作存档，不修改）。

### 延后工作

- 无。

---

## 背景与研究

### 相关代码与模式

- `crates/ralph-core/src/hat_handoff/` — gate、allocator、validator、inject、emit_instructions、macro_edges、payload。
- `crates/ralph-core/src/event_loop/mod.rs` — gate 调用、tracker 记录、`task.resume` / `plan.blocked` 派发、prompt 注入。
- `crates/ralph-core/src/event_loop/loop_state.rs` — `hat_handoff_seq`、`pending_handoff_artifacts`。
- `crates/ralph-core/src/event_loop/rejection.rs` — typed `RejectionKind` / `RejectionStage` handoff 变体、`RejectionEscalator`、`CoordinatorDispatcher`。
- `crates/ralph-core/src/runtime_state.rs` — `RuntimeStateSnapshot` handoff 字段与 `HAT_HANDOFF_DEFAULT_DIR`。
- `crates/ralph-core/src/validation/rules_hat_handoff.rs`、`validation/result.rs`、`validation/pipeline.rs`、`validation/rules_publisher.rs` — `HatHandoffRule`、校验阶段、stale 注释。
- `crates/ralph-core/src/state/ledger.rs`、`state/commit.rs`、`state/snapshot.rs`、`state/mod.rs` — `HandoffAccepted` delta、`CounterKind::HatHandoffSeq`、相关 re-export。
- `crates/ralph-core/src/preset/engine/{gates.rs,hint.rs,linter.rs,protocol.rs}` — `RejectionKind`、macro-edge handoff_path、`LintFailureClass::HandoffArtifact`、hash 输入。
- `crates/ralph-core/src/loop_context.rs` — `hat_handoff_dir()`。
- `crates/ralph-core/src/diagnosis/mod.rs`、`diagnosis/reporter.rs` — `hat_handoff` stage/source 映射与提示分支。
- `crates/ralph-core/src/summary_writer.rs` — 测试构造 `LoopState` 时使用 `hat_handoff_seq` / `pending_handoff_artifacts`。
- `crates/ralph-cli/src/{handoff_cli.rs,commands/audit_hat_handoff.rs,commands/mod.rs,policy_check.rs,main.rs,tools.rs,preflight.rs,config_resolution.rs}` — CLI 面与 preset opt-in 列表。
- `crates/ralph-cli/build.rs`、`src/preset_merge_table.rs` — 预设合并映射。
- `presets/schemas/ce-executor-serial.yml`、`presets/en/ce-executor-serial.yml` — 配置、注释与 hat instructions。
- `crates/ralph-core/data/ralph-tools-handoff.md`、`.claude/skills/ralph-tools/SKILL.md`、相关文档。

### 制度经验

- `docs/solutions/integration-issues/hat_handoff_filename_mismatch_recurrence.md` 记录了 30 天内 6 次复发的根因：SSOT 文件名与运行时文件名漂移。
- `docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md` 是原功能运行手册。
- `presets/en/ce-executor-serial.yml` 已在 2026-06-23 显式关闭 `hat_handoff`，说明产品决策已倾向移除。

### 外部参考

- 无。

---

## 关键技术决策

- **KTD1 — `HandoffIndex/HandoffTracker` 保留**：它们属于 `workflow_contract` 的通用调度/超时基础设施，服务于 WAC-U5 优先级抢占与 WRC-U4 超时恢复，不仅用于 `hat_handoff`。实施前用 `rg` 验证调用方；若确认无其他调用方再删除。
- **KTD2 — 直接删除而非弃用**：根据 `AGENTS.md`“Backwards compatibility doesn't matter”，直接移除 CLI 命令、校验阶段、配置块，不保留兼容层。
- **KTD3 — 先删代码再删测试**：按依赖顺序（core → cli → presets → docs/tests）逐步移除，每步后 `cargo build` 保证编译通过，最后统一清理测试。
- **KTD4 — `step_handoff` 隔离**：任何涉及 `step_handoff` 的模块只读不改；grep 时排除 `step_handoff` 路径，防止误删。
- **KTD5 — `macro_edges_resolved/is_macro_edge` 一并删除**：当前这些字段/方法仅被 hat_handoff 路径使用（`rules_hat_handoff`、linter auto_prepare、gates handoff_path 检查）。实施前再次用 `rg` 确认；确认后随 `hat_handoff` 删除，不保留死代码。
- **KTD6 — 文档只删活手册、不动历史**：`docs/solutions/` 中的运行手册与复发复盘需要清理或标注废弃；`docs/report/`/`docs/plans/` 历史文件保留。

---

## 待解决问题

### 规划中已解决

- **Q1. 是否删除 `step_handoff`？** 否。它在 `ce-executor-serial` 中启用，保护 `progress.md` ↔ `tasks.jsonl` 一致性，删除会导致行为回归。
- **Q2. `HandoffIndex/HandoffTracker` 是否随 `hat_handoff` 删除？** 否。它们是 `workflow_contract` 通用设施；本计划仅移除 `hat_handoff` 专用调用。
- **Q3. 是否保留 `ralph tools handoff` 命令？** 否。该命令仅用于生成 hat-handoff 文件，功能删除后无存在必要。
- **Q4. `macro_edges_resolved/is_macro_edge/HandoffArtifact` 是否保留？** 否。当前仅被 hat_handoff 路径使用，一并删除。

### 延后到实施

- **Q5. event loop 中 `process_parse_result` 内与 `hat_handoff` 交织的分支具体行号**：需要在实施时精读后删除。
- **Q6. 旧 `ledger.jsonl` / `recovery.jsonl` 中含已删除 `CommitDelta` / `RejectionKind` 变体时的错误处理**：实施时确认 `replay_from_disk` 不会 panic，并在 U4/U9 风险表中记录为可接受的数据废弃。
- **Q7. `serial_lint/*.yaml` 与 `correction_*.yml` 中把 `hat_handoff.enabled: true` 作为触发器的场景**：实施时分类处理，改用 event_policy 或 origin guard 触发，或直接删除配置噪音。

---

## 实施单元

- [ ] U1. **清理 CLI 面与相关脚本**

**目标：** 删除所有用户可见的 `hat_handoff` CLI 命令与工具文档。

**需求：** R1、R5

**依赖：** 无

**文件：**
- 删除：`crates/ralph-cli/src/handoff_cli.rs`
- 删除：`crates/ralph-cli/src/commands/audit_hat_handoff.rs`
- 修改：`crates/ralph-cli/src/commands/mod.rs`（移除 `pub mod audit_hat_handoff;`）
- 修改：`crates/ralph-cli/src/main.rs`（移除 `Commands::AuditHatHandoff` 分支）
- 修改：`crates/ralph-cli/src/tools.rs`（移除 `ToolsCommands::Handoff`）
- 修改：`crates/ralph-cli/src/policy_check.rs`（移除 `check_hat_handoff_gate` 及相关调用）
- 修改：`crates/ralph-cli/src/commands/emit.rs`、`crates/ralph-cli/src/wave.rs`（移除 hat-handoff policy check 调用）
- 删除：`crates/ralph-core/data/ralph-tools-handoff.md`
- 修改：`crates/ralph-core/data/ralph-tools.md`、`ralph-tools-emit.md`（删除指向 `ralph tools handoff` 的段落）
- 修改：`crates/ralph-core/data/doppelganger-functions.md`（若含 handoff 命令示例则删除）
- 删除：`scripts/audit-hat-handoff-artifacts.sh`
- 复核：`scripts/ralph-zsh-plugin.zsh`（当前已无可命中项；如未来有则移除）

**方法：**
- 先删除独立的 subcommand 文件，再从 dispatch 表与 re-export 中移除。
- `policy_check.rs` 中仅删除 hat-handoff 相关函数，保留 `check_step_handoff_gate`。

**测试场景：**
- Happy path：`ralph tools --help` 不再列出 `handoff` 子命令。
- Happy path：`ralph audit --help` 不再列出 `hat-handoff` 子命令。
- Error path：尝试执行已删除命令时返回未知子命令错误（由 clap 自动保证）。
- Integration：`ralph emit --policy-check` 对非 handoff 事件仍正常工作。

**验证：**
- `cargo build -p ralph-cli` 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- policy_check` 中仅保留的测试通过（handoff 测试在 U7 删除）。

---

- [ ] U2. **移除校验管线与诊断中的 HatHandoffRule**

**目标：** 从统一校验管线、诊断映射和拒绝阶段中注销 hat-handoff。

**需求：** R1、R4、R5

**依赖：** 无（与 U1 可并行开始）

**文件：**
- 删除：`crates/ralph-core/src/validation/rules_hat_handoff.rs`
- 修改：`crates/ralph-core/src/validation/result.rs`（移除 `ValidationStage::HatHandoff`、`ReasonCode::HAT_HANDOFF_*`）
- 修改：`crates/ralph-core/src/validation/pipeline.rs`（从 `pre_commit_rules` 移除 `HatHandoffRule` 注册）
- 修改：`crates/ralph-core/src/validation/mod.rs`（如有 re-export 则移除）
- 修改：`crates/ralph-core/src/validation/rules_publisher.rs`（删除 stale 注释中关于 SSOT hat_handoff allow-list 的描述）
- 修改：`crates/ralph-core/src/validation/tests.rs`（删除 `HatHandoffRule` 相关测试）
- 修改：`crates/ralph-core/src/event_loop/rejection.rs`（移除 `RejectionStage::HatHandoff`、handoff 变体在 `RejectionEscalator::check` 与 `CoordinatorDispatcher::dispatch` 中的分支、相关测试）
- 修改：`crates/ralph-core/src/diagnosis/mod.rs`（移除 `"hat_handoff" => "hat_handoff_gate"` 映射）
- 修改：`crates/ralph-core/src/diagnosis/reporter.rs`（移除 `("hat_handoff", _)` 提示分支及对应测试）

**方法：**
- 若 `ValidationStage` / `RejectionStage` 变体被序列化后落盘，需要确认旧数据路径；根据 KTD2 直接删除，不保留兼容层。
- 保留 `ValidationStage::StepHandoff` 与对应原因码。

**测试场景：**
- Happy path：启用 `ce-executor-serial` preset 时，`ValidationPipeline` 不再包含 `HatHandoffRule`。
- Edge case：`ValidationStage` / `RejectionStage` 变体顺序改变后，相关单元测试仍通过。
- Error path：带 `handoff_path` 的宏观边事件不再因 hat-handoff 规则被拒收。

**验证：**
- `cargo build -p ralph-core` 通过。
- `cargo nextest run -p ralph-core -- validation` 通过。

---

- [ ] U3. **从 Event Loop 与相邻模块移除 hat-handoff 分支**

**目标：** 删除 event loop 中的 gate 调用、prompt 注入、emit 指令、recovery 派发与运行时状态字段，并清理相邻模块中的死代码。

**需求：** R1、R2、R4

**依赖：** U2（校验阶段与拒绝阶段已移除）

**文件：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`
  - 移除 `handoff_index` 字段（若确认仅 hat_handoff 使用；否则保留）。
  - 移除 `process_parse_result` 中的 `hat_handoff::gate::evaluate_event` 调用。
  - 移除 `hat_handoff_seq` 自增、`HandoffTracker::on_handoff_accepted` 的 hat-handoff 专用调用。
  - 移除 `prepend_hat_handoff_from_pending` 与 `build_prompt` 中的 `hat_handoff::emit_instructions` / `inject` 调用。
  - 移除 `extract_handoff_path_from_payload` 等专用 helper。
  - 移除 `process_output` 中仅因 hat-handoff 产生的 recovery 分支（保留通用 `HandoffTracker::expired()` 超时恢复）。
- 修改：`crates/ralph-core/src/event_loop/loop_state.rs`（移除 `hat_handoff_seq`、`pending_handoff_artifacts`、`register_pending_handoff`、`consume_pending_handoff`）
- 修改：`crates/ralph-core/src/runtime_state.rs`（移除 `RuntimeStateSnapshot` handoff 字段、`HandoffSnapshotState`、`HAT_HANDOFF_DEFAULT_DIR`）
- 修改：`crates/ralph-core/src/loop_context.rs`（移除 `hat_handoff_dir()`；保留 `handoff_path()` 等 session handoff 方法）
- 修改：`crates/ralph-cli/src/loop_runner/runner.rs`（移除 `RALPH_HAT_HANDOFF_SEQ` 环境变量注入）
- 修改：`crates/ralph-core/src/summary_writer.rs`（移除测试中构造 `LoopState` 时使用的 `hat_handoff_seq` / `pending_handoff_artifacts` 字段）

**方法：**
- 使用 `rg` 定位 `hat_handoff` 在 `event_loop/mod.rs` 中的所有调用点，逐个删除并兜底为“直接放行”。
- `HandoffIndex/HandoffTracker` 的通用调用保留；只删除 `hat_handoff_seq` 等专用状态。

**测试场景：**
- Happy path：isolated 模式下 macro-edge 事件（如 `work.ready`）不带 `handoff_path` 也能正常通过。
- Integration：`ce-executor-serial` BDD scenario 中不再出现 `## HAT HANDOFF` prompt 注入块。
- Error path：handoff 相关 rejection reason 不再出现在 `task.resume` payload。

**验证：**
- `cargo build -p ralph-core` 通过。
- `cargo nextest run -p ralph-core -- event_loop` 通过（handoff 专用测试在 U7 删除）。

---

- [ ] U4. **清理状态账本与恢复日志中的 handoff delta**

**目标：** 删除状态账本中仅服务于 hat-handoff 的 commit delta、ledger 方法、计数器与恢复日志工厂。

**需求：** R1、R4

**依赖：** U3（event loop 不再调用这些方法）

**文件：**
- 修改：`crates/ralph-core/src/state/ledger.rs`（删除 `commit_handoff_artifact` 函数、`HandoffAcceptedInputs`、`HandoffCommitOutcome`）
- 修改：`crates/ralph-core/src/state/commit.rs`（删除 `CommitDelta::HandoffAccepted`、`CommitDelta::HandoffTrackerUpdated`、`CounterKind::HatHandoffSeq`）
- 修改：`crates/ralph-core/src/state/snapshot.rs`（删除 `handoff_accepted_log`、`handoff_tracker_log`、`hat_handoff_seq`，仅当确认无其他调用方）
- 修改：`crates/ralph-core/src/state/recovery_log.rs`（删除 handoff rejection 的 typed/legacy 工厂函数）
- 修改：`crates/ralph-core/src/correction/mod.rs`（简化 typed/legacy 选择逻辑）
- 修改：`crates/ralph-core/src/state/mod.rs`（移除 `HandoffAcceptedInputs`、`HandoffCommitOutcome` re-export）
- 修改：`crates/ralph-core/src/state/tests.rs`（删除 `u5_commit_handoff_artifact_*` 测试族与 `HAT_HANDOFF_DIR` import）

**方法：**
- 删除前用 `rg 'handoff_accepted_log\|handoff_tracker_log\|HandoffAccepted\|HandoffCommitOutcome\|HatHandoffSeq\|hat_handoff_seq'` 确认无其他调用方。
- 若 `LedgerSnapshot.handoff_tracker` 被通用 `HandoffTracker` 重放使用，则保留；否则删除。
- 旧 `ledger.jsonl` 中若包含已删除的 delta 变体，反序列化会报解析错误；按可接受数据废弃处理，必要时引导用户用 `ralph loops clean --ledger` 截断。

**测试场景：**
- Happy path：`state` 模块编译与单元测试通过。
- Edge case：旧 ledger 中 `"kind":"handoff_accepted"` 行被记录为 corrupt line，不阻塞启动。
- Error path：运行时不再生成 `CommitDelta::HandoffAccepted`。

**验证：**
- `cargo build -p ralph-core` 通过。
- `cargo nextest run -p ralph-core -- state` 通过。

---

- [ ] U5. **删除 hat_handoff 模块与核心配置**

**目标：** 删除 `hat_handoff` 模块目录及 `EventLoopConfig` 中的配置字段，并清理 preset engine 中的专用逻辑。

**需求：** R1、R4

**依赖：** U3、U4（所有调用方已清理）

**文件：**
- 删除：`crates/ralph-core/src/hat_handoff/` 整个目录
- 修改：`crates/ralph-core/src/lib.rs`（移除 `pub mod hat_handoff`）
- 修改：`crates/ralph-core/src/config/loop_config.rs`（移除 `EventLoopConfig.hat_handoff` 字段与 `HatHandoffConfig` 解析）
- 修改：`crates/ralph-core/src/config/mod.rs`（移除相关 re-export）
- 修改：`crates/ralph-core/src/preset/engine/protocol.rs`（移除 `ProtocolView.hat_handoff`、从 hash 输入删除 `hat_handoff`、删除 `macro_edges_resolved` / `macro_edge_consumers` / `is_macro_edge*` / `resolve_macro_edges` 等仅 hat_handoff 使用的方法、更新/删除相关测试）
- 修改：`crates/ralph-core/src/preset/engine/gates.rs`（移除 `RejectionKind` handoff 变体、macro-edge `handoff_path` 检查、`has_handoff_path`、kind 与 lint class 映射、更新 `empty_view()` 等 test helper）
- 修改：`crates/ralph-core/src/preset/engine/hint.rs`（移除 `LintFailureClass::HandoffArtifact`、对应 `from_reason` 字符串匹配路径与测试）
- 修改：`crates/ralph-core/src/preset/engine/linter.rs`（移除 hat-handoff precheck、auto-prepare、`RALPH_HAT_HANDOFF_SEQ` 环境变量读取、`LintPaths` 中对 `HAT_HANDOFF_DIR` 的引用）
- 修改：`crates/ralph-core/src/preset/engine/mod.rs`（如有 re-export 则移除）
- 修改：`crates/ralph-cli/src/preflight.rs`（从 `PRESET_OPT_IN_WHEN_OPERATOR_OMITS` / `PRESET_OPT_IN_KEYS` 移除 `"hat_handoff"`，删除相关 merge 测试）
- 修改：`crates/ralph-cli/src/config_resolution.rs`（从 preset opt-in 列表移除 `"hat_handoff"`）

**方法：**
- 删除模块目录前确认 `src/lib.rs` 中无其他内部路径引用。
- 用 `rg` 确认 `macro_edges_resolved` / `is_macro_edge` / `HandoffArtifact` / `LintFailureClass::HandoffArtifact` 的所有调用方；若仅 hat_handoff 使用则一并删除。
- `preflight.rs` 中的 opt-in 列表删除 `"hat_handoff"` 后，对应测试会编译失败，同步删除或改写。

**测试场景：**
- Happy path：`cargo build` 时找不到 `hat_handoff` 模块的错误消失。
- Edge case：`EventLoopConfig` 反序列化旧 YAML 中残留的 `hat_handoff:` 块时，未知字段按 serde 默认行为忽略；仓库中所有测试用 YAML 需同步清理。
- Error path：lint 阶段不再因 hat-handoff 产生 `HandoffArtifact` / `HandoffFilenameMismatch` rejection。

**验证：**
- `cargo build -p ralph-core` 通过。
- `cargo nextest run -p ralph-core -- preset` 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- preflight` 通过。

---

- [ ] U6. **清理预设、Schema 与构建期合并映射**

**目标：** 从预设 SSOT、内联 preset 和构建脚本中移除 `hat_handoff` 配置块与合并映射。

**需求：** R1、R2、R5

**依赖：** U5（配置类型已删除）

**文件：**
- 修改：`presets/schemas/ce-executor-serial.yml`（删除 `hat_handoff:` 块）
- 修改：`presets/en/ce-executor-serial.yml`（删除所有 `hat_handoff` 相关注释与 instructions 段落，包括但不限于第 91–97 行与 coordinator instructions 中的 `handoff_path` 指引）
- 修改：`crates/ralph-cli/build.rs`（删除 `hat_handoff` 合并映射条目）
- 修改：`crates/ralph-cli/src/preset_merge_table.rs`（删除 `("hat_handoff", ...)` 条目）
- 复核：`crates/ralph-cli/src/presets.rs` 中的 `PRESETS` 数组使用 `include_str!(concat!(env!("OUT_DIR"), "/presets/ce-executor-serial.yml"))`，因此**无需手动修改 `content` 字符串**；只需保证 YAML 与 schema 一致并在 build 后重新生成。

**方法：**
- 编辑 YAML 文件后运行 `cargo build -p ralph-cli`，让 `build.rs` 报错提示任何不一致，再修正。
- 用 `rg -i 'hat_handoff|handoff_path' presets/en/ce-executor-serial.yml presets/schemas/ce-executor-serial.yml` 确保无残留。

**测试场景：**
- Happy path：`cargo build -p ralph-cli` 不再因 `hat_handoff` 合并映射报错。
- Happy path：`ralph run -H builtin:ce-executor-serial` 能正常初始化 event loop。
- Edge case：用户项目 `ralph.yml` 中残留的 `event_loop.hat_handoff.enabled: false` 被忽略，不影响启动。

**验证：**
- `cargo build -p ralph-cli` 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- presets` 通过。

---

- [ ] U7. **删除 hat_handoff 专用测试与 BDD 场景**

**目标：** 删除所有仅验证 hat-handoff 行为的测试与场景文件，避免测试失败后需要维护废弃代码。

**需求：** R4

**依赖：** U1–U6（被测代码已删除）

**文件：**
- 删除：`crates/ralph-core/tests/scenarios/hat_handoff/` 整个目录
- 删除：`crates/ralph-core/tests/scenarios/handoff_auto_generate.yml`（hat_handoff 专用）
- 保留：`crates/ralph-core/tests/scenarios/plan_gate_dual_publish_handoff.yml`（测试双发 carve-out，与 hat_handoff 无关）
- 修改：`crates/ralph-core/tests/scenarios.rs`（移除已删除场景的 `#[test]` 函数）
- 修改/保留分类处理：`crates/ralph-core/tests/scenarios/serial_lint/*.yaml` 与 `correction_*.yml`
  - 仅把 `hat_handoff.enabled: true` 作为配置噪音的场景：删除该配置块。
  - 依赖 hat_handoff gate 触发 rejection 的场景（如 `serial_lint_2_rejection_digest.yaml`）：改用 event_policy 或 origin guard 触发，或删除该场景。
- 保留：`crates/ralph-core/src/event_loop/tests/handoff_dispatch.rs`（通用 `HandoffTracker` WRC-U4 测试）
- 保留：`crates/ralph-core/src/event_loop/tests/recovery_envelope_u7_u8.rs`（通用 HandoffTracker recovery envelope 测试）
- 保留：`crates/ralph-core/src/event_loop/tests/enrich_kind_wiring.rs`（通用 `kind` 接线测试；实施前用 grep 复核无 handoff 引用）
- 修改：`crates/ralph-core/src/event_loop/tests/serial_lint.rs`（移除 hat-handoff 专用用例与相关 fixture 引用）
- 修改：`crates/ralph-core/src/event_loop/tests/coordinator_dispatch_coverage.rs`（移除 handoff rejection kind 路由用例）
- 删除：`crates/ralph-cli/tests/policy_check_handoff.rs`
- 修改：`crates/ralph-cli/tests/integration_emit_policy.rs`（移除 handoff 相关断言）
- 修改：`crates/ralph-core/benches/protocol_view_bench.rs`（若 benchmark 仅覆盖 hat_handoff 则删除对应 case）

**方法：**
- 删除前用 `rg` 确认每个场景/测试是否也覆盖 `step_handoff` 或通用逻辑；若混合覆盖，仅删除 handoff 分支。
- 保留 `crates/ralph-core/tests/scenarios/step_handoff/` 与 `serial_lint_7_handoff_seeds_coverage.yaml`。

**测试场景：**
- Happy path：删除后 `./scripts/run-tests.sh` 不再执行已删除的 handoff 测试。
- Edge case：确保没有误删 `step_handoff` 场景（如 `progress_task_mismatch.yml`、`step_advance_u1_to_u2.yml`）。
- Integration：`HandoffTracker` 通用超时与 recovery envelope 测试仍通过。

**验证：**
- `./scripts/run-tests.sh` 通过。

---

- [ ] U8. **更新文档、运行手册与运营脚本**

**目标：** 删除或标注废弃所有仍在维护的 `hat_handoff` 文档，避免用户读到已删除功能的说明。

**需求：** R5

**依赖：** U1、U6

**文件：**
- 删除或归档：`docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md`
- 删除或归档：`docs/solutions/integration-issues/hat_handoff_filename_mismatch_recurrence.md`
- 修改：`docs/guide/runtime-diagnosis.md`（删除 hat-handoff 相关诊断表格行）
- 修改：`docs/handbook/serial-preset-development.md`（删除 `hat_handoff` 配置说明）
- 复核：`HANDOFF.md`（当前无 hat_handoff 引用；如有则删除）
- 修改：`crates/ralph-core/data/ralph-tools.md`（删除 `ralph tools handoff` 引用）
- 修改：`crates/ralph-core/data/ralph-tools-emit.md`（删除 hat-handoff policy check 说明）
- 修改：`crates/ralph-core/data/doppelganger-functions.md`（若含 handoff 命令示例则删除）
- 修改：`crates/ralph-core/src/validation/rules_publisher.rs`（删除 stale 注释）
- 修改：`crates/ralph-core/src/diagnosis/reporter.rs`（在 U2 中已处理；此处复核文档/提示字符串）
- 修改：`scripts/ralph-zsh-plugin.zsh`（在 U1 中已复核）
- 修改：`AGENTS.md` / `CLAUDE.md`（同步更新 builtin preset 列表、命令列表；当前未发现直接引用，但需复核）

**方法：**
- 历史报告 `docs/report/` 与已完成计划 `docs/achieved/` 不动。
- 若选择归档而非删除，将文件移入 `docs/achieved/solutions/` 或重命名为 `*-deprecated.md`。

**测试场景：**
- Happy path：`rg -i 'hat_handoff|## HAT HANDOFF|ralph tools handoff|ralph audit hat-handoff' docs/solutions/ docs/guide/ docs/handbook/ crates/ralph-core/data/` 无命中（排除历史报告）。
- Error path：文档中不再引导用户使用已删除命令。

**验证：**
- `cargo test --doc` 通过（文档示例中无已删除命令）。
- 手动检查 `ralph --help`、`ralph tools --help`、`ralph audit --help` 无 handoff 子命令。

---

- [ ] U9. **全量回归验证与残留扫描**

**目标：** 确保删除后无编译错误、无测试失败、无残留引用、无行为回归。

**需求：** R6

**依赖：** U1–U8

**文件：**
- 全仓库（无新增文件）

**方法：**
- 运行 `cargo fmt` 与 `cargo clippy`。
- 运行 `./scripts/run-tests.sh`。
- 使用精确模式扫描残留：
  - `rg 'hat_handoff|HatHandoff|HAT_HANDOFF' crates/ralph-core/src/ crates/ralph-cli/src/ presets/`
  - `rg 'HatHandoffSeq|hat_handoff_seq' crates/ralph-core/src/`
  - `rg 'handoff_path' crates/ralph-core/src/ crates/ralph-cli/src/ presets/ -g '!**/handoff.rs' -g '!**/landing.rs'`（过滤 session handoff 路径）
  - `rg -i '## HAT HANDOFF|HAT HANDOFF EMIT|hat-handoff' crates/ralph-core/src/ crates/ralph-cli/src/`
  - `rg -i 'ralph tools handoff|ralph audit hat-handoff' scripts/ docs/guide/ docs/handbook/ docs/solutions/ crates/ralph-core/data/`
- 扫描结果中排除 `step_handoff`、`HandoffTracker`、`HandoffIndex`、`.ralph/agent/handoff.md`、session `handoff_path` 等合法命中。

**测试场景：**
- Integration：`ce-executor-serial` 的 BDD scenarios（不含 hat_handoff）全部通过。
- Happy path：`ralph run -H builtin:ce-executor-serial` 能正常初始化 event loop。
- Regression：与删除前基线相比，`ce-executor-serial` 场景成功率不下降。
- Edge case：旧 ledger 中残留的 `handoff_accepted` / `handoff_tracker_updated` / `hat_handoff_seq` 行被识别为 corrupt line，不 panic。

**验证：**
- `./scripts/run-tests.sh` 全绿。
- `cargo clippy` 无新警告。
- 残留扫描无 hat_handoff 专用命中。

---

## 系统级影响

- **交互图**：`event_loop` 与 `validation`、`state`、`preset/engine`、`CLI` 的交互均减少一个分支；`step_handoff` 分支保持原样。
- **错误传播**：`RejectionKind` 减少 4 个 handoff 变体，`task.resume` 的 `kind` 字段不再携带这些值；`CoordinatorDispatcher` 的 typed dispatch 同步删除对应分支。
- **状态生命周期**：`LoopState.hat_handoff_seq` 与 `pending_handoff_artifacts` 删除后，event loop 不再维护 hat-handoff 专用中间状态；通用 `HandoffTracker` 的超时恢复路径保留。
- **API 表面**：CLI 删除 2 个子命令；配置文件中的 `hat_handoff` 块变为未知字段（serde 默认忽略）。
- **集成覆盖**：`ce-executor-serial` 的端到端场景是主要回归风险点；需确保 `queue.advance`/`plan.complete` 仍被 `step_handoff` 正确 gate。
- **数据兼容**：旧 `ledger.jsonl` 中可能残留的 `handoff_accepted` / `handoff_tracker_updated` / `hat_handoff_seq` 行会变为 corrupt line；按可接受数据废弃处理。
- **不变性保证**：
  - `step_handoff` 的 `progress_task_gate` 保持启用。
  - `workflow_contract.handoff_topic_seeds`、`HandoffIndex`、`HandoffTracker` 保持运行。
  - session 结束 `.ralph/agent/handoff.md` 保持生成。

---

## 风险与依赖

| 风险 | 可能性 | 影响 | 缓解措施 |
|---|---|---|---|
| 误删 `HandoffIndex/HandoffTracker` 导致 workflow_contract 超时/优先级失效 | 中 | 高 | U3/U5 实施前用 `rg` 验证所有调用方；只删除 `hat_handoff` 专用调用，保留通用设施。 |
| 误删 `step_handoff` 相关测试或配置 | 中 | 高 | U7 删除测试时排除 `step_handoff/` 目录与 `serial_lint_7_handoff_seeds_coverage.yaml`；U6 不修改 `workflow_contract.step_handoff`。 |
| 遗漏 `commands/mod.rs`、`loop_context.rs`、`diagnosis/*`、`preflight.rs`、`config_resolution.rs`、`summary_writer.rs` 等文件导致编译失败 | 高 | 高 | 本计划已将这些文件明确列入各单元；U9 残留扫描再次兜底。 |
| 预设合并映射删除后 `build.rs` panic | 中 | 中 | U6 同步更新 `build.rs`、`preset_merge_table.rs`，并通过 `cargo build -p ralph-cli` 验证。 |
| `presets/en/ce-executor-serial.yml` 中 hat_handoff 相关 instructions/注释残留 | 中 | 低 | U6 用 `rg -i 'hat_handoff\|handoff_path' presets/en/ce-executor-serial.yml` 兜底扫描。 |
| 旧 `ledger.jsonl` 含已删除的 `CommitDelta` 变体，反序列化报错 | 低 | 中 | 按可接受数据废弃处理；必要时引导用户用 `ralph loops clean --ledger` 截断。 |
| 文档中仍有指向已删除命令的说明 | 中 | 低 | U8 用 `rg` 扫描 `docs/solutions/`、`docs/guide/`、`docs/handbook/`、`crates/ralph-core/data/`。 |
| 残留 `handoff_path` 字段在事件 schema 或 prompt 中引发用户困惑 | 低 | 低 | U9 精确扫描 `handoff_path`，排除 session handoff 路径。 |

---

## 文档 / 运营说明

- 删除 `ralph tools handoff` 与 `ralph audit hat-handoff` 后，更新 `scripts/ralph-zsh-plugin.zsh` 并重新安装到 `~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`（按 `AGENTS.md` 要求）。
- `docs/solutions/` 中相关运行手册归档或删除；`docs/report/` 历史复盘保留。
- 若后续用户项目 `ralph.yml` 仍包含 `hat_handoff:` 块，serde 默认会忽略未知字段，不会阻塞启动。

---

## 来源与参考

- **原始需求文档：** `docs/brainstorms/2026-06-18-isolated-hat-handoff-requirements.md`
- **复发复盘：** `docs/solutions/integration-issues/hat_handoff_filename_mismatch_recurrence.md`
- **运行手册：** `docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md`
- **当前 preset 配置：** `presets/schemas/ce-executor-serial.yml`、`presets/en/ce-executor-serial.yml`
- **相关代码模块：** `crates/ralph-core/src/hat_handoff/`、`crates/ralph-core/src/event_loop/`、`crates/ralph-core/src/validation/`、`crates/ralph-core/src/state/`、`crates/ralph-cli/src/`
