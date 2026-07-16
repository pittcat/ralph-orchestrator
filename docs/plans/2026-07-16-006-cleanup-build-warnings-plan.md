---
title: 清理 ralph-core / ralph-cli 编译警告（dead_code / unused_imports / unused_variables） - Plan
type: chore
date: 2026-07-16
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

## Goal Capsule

**Objective.** 清理 `cargo build -p ralph-cli -p ralph-core` 暴露的全部 74 条编译警告（35 条来自 `ralph-core` lib,39 条来自 `ralph-cli` bin "ralph"），按"非破坏性最小变更"原则逐类处理，让 `cargo build` 重新变绿，同时不引入任何回归。

**Authority hierarchy.** 本计划以 CLAUDE.md 的 HARD RULE 为顶层契约：
- 任何更改不得破坏已有测试或运行时行为(HARD RULE 1+2 nextest 隔离、回归保护)。
- Backwards compatibility doesn't matter —— it adds clutter for no reason(允许删除无用公共 API)。
- 修改后必须跑 `cargo nextest run` 验证。

**Stop conditions.**
1. `cargo build -p ralph-cli -p ralph-core` 退出 0 且 stderr 不含 `warning:`。
2. `cargo nextest run -p ralph-cli -p ralph-core` 全部包测试 0 失败、0 skipped(已 skipped 不新增)。
3. 全 workspace `./scripts/run-tests.sh` 通过 —— 任何新失败即视为回归。
4. 零 console of any public API 实测被回滚者(HARD:不能因为"精简"删掉正在被 helper crate / 测试依赖的入口)。

**Execution profile.** 标准深度(Standard),5 个串行 Unit,每单元 ≤ 1 commit,跑全量测试再推进。

**Tail ownership.** 本计划到 U5 终止,后续若发现老 plan 留下的半成品 dead code,另开 plan;不在本次范围。

---

## Problem Frame

今天 `cargo build` 暴露的警告分布如下:

| crate | warning 数 | 类别分布 |
|---|---|---|
| `ralph-core` lib | 35 | unused_imports × 11,unused_variables × 4,use of deprecated × 3,never_used (fn/struct/method/field/variant) × 17 |
| `ralph-cli` bin "ralph" | 39 | unused_imports × 8,unused_variables × 9,named argument warning × 1,never_used × 21 |

**为什么 warnings 不是轻量债务。**
- 后续维护者会把 warning 当作"现状即正确",沿用死代码 API 引入新 bug(2026-07-13 已观察到 `hat_command_policy::ConfigFault` 的 hint 方法在测试中调用但生产链无意义,误导新人)。
- CI 早期 `cargo clippy -- -D warnings` 校验一旦接入,会一次性爆出 n 个发版阻断项;在 CI 之前消掉,避免改一次 clamp 改 100 处。
- `find` 工具识别"还在用"路径时,大量 dead code 会把"真问题"埋没。

**约束。**
- 不能新增 `pub` API(回 cli/tui 跨 crate 影响面)。
- 不能改测试 fixture 的语义(回测试稳定性)。
- 删公共 API 之前必须用 ripgrep 全仓验证(本计划研究阶段已对 21 个 flagged fn/struct/method 全部完成)。

**已知风险。**
- `super::snapshot::ViolationKind`(on_accepted.rs:19)看似 unused,但行 154+291 在 inner `mod tests` 中使用了——研究阶段已确认这是 false alarm,需要降级 import 而非删除。
- 8 个 `supervisor::*` 旧 feature-gated 类型(`compensation`/`event_count`/`OnTimeout` 等)在 `--features supervisor-db` 下使用 —— 必须加 `#[allow(dead_code)]` 而非删除。
- 5 个 CLI emit `legacy_resolve`/`load_policy_config_from_hats_only`/`ValidationError::new` 是给后续契约重构留的入口 —— 也是 allow 而非删除。

---

## Product Contract

### Summary

把 74 条编译警告按"action=delete / allow / rename-prefix / preserve"四类分流:delete 用于纯私有死代码(`LocalFunction`),allow 用于 feature-gated / 公共-API-stable 入口,rename-prefix 用于 unused variables,named-arg fix 是单行格式化串纠正。

### Requirements

**R1.** `cargo build -p ralph-cli -p ralph-core` 完成后 stderr 中 `warning:` 计数降至 0。

**R2.** 全部现有测试保持 0 失败、0 新跳过。Covers 测试入口: `cargo nextest run -p ralph-cli -p ralph-core --no-fail-fast`。

**R3.** 全 workspace `./scripts/run-tests.sh` 结束后 0 失败(7 包并行 + nextest 默认并发,符合 CLAUDE.md HARD RULE 2)。

**R4.** 公共 API 改动最小化:任何 `pub`/`pub(crate)` 函数的删除必须先经过 ripgrep 全仓验证(reference count = 0 in `ralph-core` / `ralph-cli` / `ralph-tui` / `ralph-api` / `ralph-e2e` / `ralph-bench` / `ralph-proto` / `ralph-adapters`)。

**R5.** Supervisor 模块的 dead code 处理必须保持 `--features supervisor-db` 构建路径仍能编译。

**R6.** 任何 `#[allow(dead_code)]` 添加必须就近注释理由(`// reason: ...`),指出是 feature-gated / 公共契约 / 测试 fixture 守门这三类之一,不允许无声 allow。

### Actors

- A1. Coding Agent(执行 U1-U5 串行实现)
- A2. CI(回归兜底)
- A3. 后续维护者(消费 allow 的"理由"注释决定能否二次降级)

### Flows

- F1. U1→U2→U3→U4→U5 严格串行,每单元独立验收 → 升级。
- F2. 每 Unit 完成后追加`cargo nextest run -p <affected>` → 验收门通过 → 进入下一 Unit。

### Acceptance Examples

- AE1. U1 完成后,13 条 `unused_imports` 中能直接删的 n 条已删,不能删的(在 inner `mod tests` 引用、或宏展开)的加 `#[allow(unused_imports)]`,`rg "warning:"` 集中看 unused_imports 类下降到 ≤3 条。
- AE2. U5 完成后 `cargo build` 0 warning;`./scripts/run-tests.sh` exit 0。
- AE3. U3 中删除 `migrate_tasks_file` / `MigrationReport` 后,build 不破坏 `crates/ralph-cli/src/migrate_state/tests.rs`(其内部测试用,不依赖外部 crate)。

### Success Criteria

1. `cargo build` 0 warnings(grep `-c '^warning:'` = 0)。
2. `cargo nextest run -p ralph-cli -p ralph-core` exit 0,无新增 skipped。
3. `./scripts/run-tests.sh` exit 0。
4. `cargo clippy -p ralph-cli -p ralph-core --tests --all-features -- -W clippy::all`(不带 -D)在已有 baseline 上不引入新 warning 数(允许外加 dead_code 类别减少)。

### Scope Boundaries

**In scope.**
- 修复全部 74 条警告(reference 触发的 `unused_imports`、函数/结构/字段/变体的 dead code、未用变量)。
- 关闭 2 个 deprecated constant 警告(FINDING_REVIEW_TERMINAL_DUAL_SUBSCRIBE / _PUBLISHER_INCOMPLETE → 改用新名)。
- 1 个命名参数格式化串纠正(`task_cli.rs:1780` 的 `caller_hat`)。

**Out of scope (this plan).**
- 引入 `cargo clippy -- -D warnings` 至 CI(应单独立项,本次只清理 baseline)。
- 修复 `flow_lifecycle::current_step_id_fallback` 这条 deprecated method 警告(替换上游调用点超出本计划边界,允许在 plan 末尾 `## Deferred` 留项)。
- 重构 `(半成品)`计划特有的 dead code(由 plan N-1 commit 但未交付的 sentinel code)—— 仅清理 commit-audited 可删者。
- supervisor feature 内部结构 refactor(只允许加 `#[allow(dead_code)]`)。

**Deferred to follow-up work.**
- 把 `flow_lifecycle::FlowLifecycleRegistry::current_step_id_fallback` 的两个 caller(rgrep 见 2026-07-12 commit)迁移到 `current_step_id()`,并随后删除 deprecated 入口。需要独立 plan 评估 caller 行为差异。
- `ralph-cli/src/loop_runner/wave/worker.rs::WaveWorkerStreamHandler` 的实际插入路径(若确需保留则加 allow;若已废弃则开新 plan 删 fixture)。

---

## Planning Contract

### Key Technical Decisions

**KTD-1: 三类处置策略,不允许第四类。"** 任何 warning 必须落在 delete / allow-prefix / rename-underscore 三桶之一,且桶选择有可文档化的判定。
- **delete**(actions=立即删除):全仓 ripgrep 引用计数 = 0(`rg -l "<symbol>" --type rust` 输出仅其定义文件或仅测试模块)。例外:测试本身是消费方的不算"无人引"。
- **allow**(动作:加 `#[allow(reason)]` + 注释):feature-gated(在 `--features supervisor-db` 或 `--features X` 启用时被引用)、公共契约(super-trait method、稳定 ffi shape)、测试 fixture 守门(被 `#[cfg(test)] mod tests` 引用)。
- **rename-prefix**(动作:`_name`):`unused variable: <x>`,且变量在 pattern match / closure body 内不可用但参与类型推断(不能简单删除)。

**理由**: 三桶足以覆盖 74 条警告;放宽到第 4 桶(`#[allow(unused)]` 静默)会埋债务。

**KTD-2: 不破坏 supervisor feature gate.** --features supervisor-db 是 CLAUDE.md 显式支持的 feature(见"ce-executor-supervisor" preset 与测试入口"scripts/run-tests.sh"中的 feature-gate 描述)。任何 supervisor 模块的 dead code 处置必须以 `cargo build --features supervisor-db -p ralph-cli` 0 warning 为次级验收点。

**KTD-3: emit/policy_check 模块的 `legacy_resolve` 等"未来契约"入口不删.** 理由:这些是 U15/U7 迭代产生的过渡 API,被 `report_legacy_serialiser` 之类的同伴 API 间接引用(`rg "PolicyCheckMode" crates/ralph-cli | wc -l` 显示有 7 个 deps)且本计划范围不允许 broad refactor。一律 `#[allow(dead_code)]` + 注释标记。
- 受影响:`resolve_policy_check_mode`、`legacy_resolve`、`load_policy_config_from_hats_only`、`ValidationError::new`、`should_policy_check_emit`、`paths_canonical_differ`、`resolve_project_config_path_with_env`、`hat_command_policy::ConfigFault`、`hat_command_policy::ALL`、`hat_command_policy::hint/is_allow/is_deny`、`enforce_preset_lint_gate`(2-arg)。

**KTD-4: dead code 在迁移中点(`migrate_state.rs`)。** `migrate_tasks_file` 与 `MigrationReport` 是 U19 (2026-06-27 plan) 引入的 standalone migrate 工具。`Cargo clap` 子命令 `ralph migrate-state` 必须存在;研究显示目前主代码无调用。研究阶段确认:
- `crates/ralph-cli/src/migrate_state/tests.rs` 内部测试使用 `migrate_tasks_file` 与 `MigrationReport` —— 删除前需要先确认 `clap` 路径上仍有该命令(cmd 入口在 `commands/migrate_state_cmd.rs`)。
- 若 cmd 入口仍在(grep `MigrateStateArgs|cmd_migrate_state` 验证),则**保留** API 与 test,加 allow 守门测试 module;若 cmd 也已删除,则整套作废。

**KTD-5: deprecated constants.** 2 个 finding 常量已有非 deprecated 替代(`FINDING_TERMINAL_DUAL_SUBSCRIBE` / `FINDING_TERMINAL_PUBLISHER_INCOMPLETE`)。commit 验证:rg 显示 0 引用,删除 deprecated alias,合并调用点为新名。

**KTD-6: 命名参数格式化串警告.** `task_cli.rs:1780` 的 `caller_hat={caller_hat}` 在原始格式串中是 positional,clippy/rustc 升级后会警告"named argument referred by position"。要么改名匹配 positional param 名,要么改 `format!("{0}")` 用 positional。优先改名(读起来更清晰)。

**KTD-7: Supervisor 模块的 dead code allow 必须就近维护.** 默认 build 不走 feature,意味着默认 build 下 dead code 直接产生 warning;必须用 `#[cfg_attr(not(feature = "supervisor-db"), allow(dead_code))]` 或宏层 allow,而非全局 `#[allow(dead_code)]`。

**KTD-8: 测试模块的 `super::*` 可以改成显式 import.** `emit_result/tests.rs:12` 的 `use super::*;` 提示 unused,是测试模块"macros via glob"反例。改成显式逐项 import(参考 U1-006 模式)。

### High-Level Technical Design

无 —— 本计划是"清理"型,无新增组件、状态机或协议;按 KTD 桶三桶法一条一条执行。

### Assumptions

1. clap 子命令 `ralph migrate-state`(对应 `crates/ralph-cli/src/migrate_state.rs`)确实曾在主入口(`crates/ralph-cli/src/main.rs` 或 `commands/mod.rs`)注册 —— U5 之前将先行 rg 验证,若证实已删除则绕过 KTD-4 删整套 migrate_state。
2. `#[allow(dead_code)]` 注释理由足够解释未来维护者(已存在的同类型注释样例见 `crates/ralph-core/src/drift/alert.rs:343-355`,证明本项目允许 allow+注释)。
3. CI `cargo nextest run` 入口与本计划用的 `cargo nextest run -p ralph-cli -p ralph-core --no-fail-fast` 同质,无 hidden setup。
4. `cargo build` warning 数 = 0 是强目标;CI 阶段会单独评估是否引入 `-D warnings`,不在本计划范围。

---

## Implementation Units

| U-ID | 一句话 | Files touched | depends-on |
|---|---|---|---|
| U1 | 删"真 unused"imports(13 条) | `crates/ralph-core/src/{emit_result/tests,event_loop/phase_authority/{on_accepted,step_transition},event_loop/mod,supervisor/{coordinator,phase,recover},preset_validator,state_projector/review,runtime_state,config/precheck}.rs`、`crates/ralph-cli/src/{commands/{events,inspect,run},loop_runner/{hard_gate,hooks/mod,mod},emit_result/tests,config/precheck}.rs` | – |
| U2 | 改 unused_variables 为 `_name` 前缀(13 条) | `crates/ralph-core/src/{event_loop/mod,execution_contract,handoff_envelope,preset_lint/instructions_opac}.rs`、`crates/ralph-cli/src/{loop_runner/wave/dispatcher,task_cli}.rs` | U1 |
| U3 | 删确认无引用的私有 dead code | `crates/ralph-core/src/{state/idempotent_log,preset_lint/instructions_opac,supervisor/{coordinator,recover},handoff_envelope,event_policy}.rs` 的私有 fn/struct/variant/field;`crates/ralph-cli/src/{migrate_state,policy_check,emit}.rs` 部分 | U1 |
| U4 | 给 feature-gated / 公共契约的 dead code 加 `#[allow(dead_code)]` + 注释 | 上列表剩余项(supervisor feature-gated、`legacy_resolve`/`ConfigFault`/`ALL` 等保留 API、tests fixture 守门) | U3 |
| U5 | 删 deprecated constants、改命名参数格式、跑全量回归 | `crates/ralph-core/src/preset_lint/finding_id.rs`、`crates/ralph-cli/src/task_cli.rs:1780` | U4 |

---

### U1. 删 pure unused imports

**Goal.** 把 13 条 `unused_imports` 中所有"全仓无引用"的项直接从源文件删除,降低 warning 计数 ~17%。

**Requirements.** R1,R4。

**Files.** (全部 repo-relative):
- `crates/ralph-core/src/emit_result/tests.rs`(12-13)
- `crates/ralph-core/src/event_loop/phase_authority/on_accepted.rs`(19 - ViolationKind 行内未使用,但注意 inner mod tests 用 —— 见 Approach)
- `crates/ralph-core/src/event_loop/phase_authority/step_transition.rs`(11 - StepKind 行内使用,确认 error 信息)
- `crates/ralph-core/src/event_loop/mod.rs`(148, 153)
- `crates/ralph-core/src/supervisor/coordinator.rs`(25, 28)
- `crates/ralph-core/src/supervisor/phase.rs`(10 - WavePhase 仅在 tests mod 用 —— 需 `cfg(test) pub(crate)` 或迁移)
- `crates/ralph-core/src/supervisor/recover.rs`(40 - WaveSnapshot)
- `crates/ralph-core/src/preset_validator.rs`(19 - Hat)
- `crates/ralph-core/src/state_projector/review.rs`(14 - Mutex)
- `crates/ralph-core/src/runtime_state.rs`(25 - TaskStatus 在文件层未用,在 tests mod 用 —— 同 on_accepted 模式)
- `crates/ralph-core/src/config/precheck.rs`(115 - HashMap)
- `crates/ralph-cli/src/commands/events.rs`(2)
- `crates/ralph-cli/src/commands/inspect.rs`(33, 35)
- `crates/ralph-cli/src/commands/run.rs`(12)
- `crates/ralph-cli/src/loop_runner/hard_gate.rs`(2, 4)
- `crates/ralph-cli/src/loop_runner/hooks/mod.rs`(16)
- `crates/ralph-cli/src/loop_runner/mod.rs`(36)

**Approach.** U1 之前先并行跑下列校验,确认每条 import 真无引用(rim 避免 U1 删错):
```bash
rg -n "WavePhase" crates/ralph-core/src/supervisor/phase.rs
rg -n "ViolationKind" crates/ralph-core/src/event_loop/phase_authority/on_accepted.rs
rg -n "Hat\b" crates/ralph-core/src/preset_validator.rs
```
对每个被规划的 import,核实行内无使用再删除;行内有 inner mod tests 用的(如 `ViolationKind` 在 `use super::super::snapshot::ViolationKind` inner 中),把外层 import 行删除即可,保留 inner。`emit_result/tests.rs` 的 `use super::*;` 改为显式列出 ExportItem,逐项改 rustfmt。

**Test scenarios.**
- 每条 import 删除后 `cargo build -p ralph-core -p ralph-cli` 不出新 warning,旧 warning 取消。
- 覆盖 happy: `cargo build -p ralph-core -p ralph-cli` exit 0,unused_imports 类 ≤3。
- 覆盖 inner mod: `on_accepted.rs` 在 inner mod 内引用 `ViolationKind` 必须仍编译通过(`rg "use super::super::snapshot::ViolationKind" crates/ralph-core/src/event_loop/phase_authority/on_accepted.rs` 应仍有匹配)。
- 覆盖 cfg(test) pub(crate): `phase.rs` `WavePhase` 移到 `#[cfg(test)] pub(crate) use crate::supervisor::WavePhase;` 内部(类似 runtime_state.rs 的 `TaskStatus` 模式)。

**Verification.** `cargo nextest run -p ralph-core --no-fail-fast` 至少 0 失败,unused_imports warning 数 ≤ 3。

---

### U2. 改 unused variables 为 `_name` 前缀

**Goal.** 13 条 `unused variable` 警告 → 全部用下划线前缀解决,signature 不变。

**Requirements.** R1,R4。

**Files.**
- `crates/ralph-core/src/event_loop/mod.rs`(5461 `hat_id`,10765 `protocol_count`,10955 `policy_enabled_for_gate`)
- `crates/ralph-core/src/execution_contract.rs`(777 `other`)
- `crates/ralph-core/src/handoff_envelope.rs`(449 `root_goal`)
- `crates/ralph-core/src/preset_lint/instructions_opac.rs`(322 `preset_name`)
- `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`(310 `config_path`)
- `crates/ralph-cli/src/task_cli.rs`(1209/1532/1825/1982 `config_sources`,1653 `owner_hat`,2036 `config`)

**Approach.** 直接 rename + 加 `#[allow(unused_variables)]` 仅在 rustc 决定变量参与类型推断时(`let _ = fn_returning_unit()` 改写为 `let _ = ...;` 等)。优先 underscore-rename;若变量被 closure 捕获或 pattern-match 强制需要 named binding,则保持命名 + 添加行的 `let _ = name;` 抑制警告。

**Test scenarios.**
- 全部 rename 后 `cargo build` exit 0,unused_variables 类 ≤ 2(残留是测试 helper)。
- 涉及 closure / pattern match 的:`rg "fn.*hat_id\b" crates/ralph-core/src/event_loop/mod.rs | sed -n '5461p'` —— 确认 rename 后 binding 仍使用。

**Verification.** `cargo build -p ralph-core -p ralph-cli 2>&1 | grep -c '^warning:'` ≤ 5 (允许剩余 deprecate + 死代码残留等待 U3-U5)。

---

### U3. 删"全仓 0 引用 + 生产无 caller"的 dead code

**Goal.** 把无须保留的纯 dead code 删除(含 migrate_state、WaveWorkerStreamHandler、MockSupervisorBridge 等已废弃 sentinel)。处理前再次 ripgrep 校验引用,避免回归。

**Requirements.** R1,R2,R4。

**Files.**
- `crates/ralph-cli/src/migrate_state.rs`(28 `MigrationReport`,50 `migrate_tasks_file`) —— 前提: `rg -n "migrate_state|MigrateStateArgs" crates/ralph-cli/src/{main.rs,commands/mod.rs,commands/migrate_state_cmd.rs} 2>/dev/null` 输出空集(确认 clap 子命令也删除),整体删除。若 clap 入口仍在,则仅删 MigrationReport 字段保留结构(由 U4 处置)。
- `crates/ralph-cli/src/policy_check.rs`(1882 `ValidationError::new` —— 在 feature-gated 入口使用,迁移到 U4)
- `crates/ralph-cli/src/loop_runner/wave/worker.rs`(30 `WaveWorkerStreamHandler`,37 `new`/`emit_delta`)
- `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`(1447 `SupervisorFanInOutcome`,1465 `run_supervisor_fan_in`)
- `crates/ralph-cli/src/loop_runner/wave/supervisor_bridge.rs`(60 `with_in_memory_store`/`coordinator`,155 `MockSupervisorBridge`,167 `new`/`push_actions`/`snapshot`)

**Approach.** 对每个 fn / struct / method,先 ripgrep 验证引用计数 = 0,再整段删除并连带删除未使用 imports:
```bash
rg -n "WaveWorkerStreamHandler" --type rust      # 应只输出 worker.rs 自身
rg -n "SupervisorFanInOutcome" --type rust        # 应只输出 dispatcher.rs
```
对结构性资源(struct),同步检查 impl block + tuple field 引用,一并消除(对 Vec<(K,V)> 中的 V 引用、结构体 literal 等)。

**Test scenarios.**
- 删 struct + impl 后,模块仍编译。
- 测试用例不再以 `MockSupervisorBridge::new()` 等做 fixture:验证 `rg "MockSupervisorBridge" crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` 应只输出 path(确认未依赖)。
- `migrate_state.rs`:若 clap 命令也已废弃,删除整个 `migrate_state.rs` + `migrate_state/tests.rs` + `migrate_state_cmd.rs`;若任一保留则走 KTD-4。

**Verification.** `cargo build -p ralph-cli --tests` exit 0,`cargo nextest run -p ralph-cli --no-fail-fast` 0 失败。

---

### U4. 给保留 API / feature-gated dead code 加 `#[allow(dead_code)]` + 注释

**Goal.** 把不便删除但当前默认构建 unused 的项允许化,稳定允许的同时不掩盖真问题。

**Requirements.** R1,R5,R6。

**Files.**
- `crates/ralph-core/src/{handoff_envelope,event_policy,supervisor/coordinator,supervisor/recover,supervisor/memory,preset_lint/instructions_opac,event_loop/mod}.rs`
- `crates/ralph-cli/src/{policy_check,emit,config_resolution,hat_command_policy,loop_runner/preset_lint_gate,task_cli}.rs`

**Approach.** 逐文件按下表处置 + 紧贴 `#[allow]` 行写一行注释说明理由(详见 KTD-7):

| Symbol | 处置 | 注释模式 |
|---|---|---|
| `with_in_memory_store`/`coordinator`/`MockSupervisorBridge`/`new`/`push_actions`/`snapshot`(supervisor_bridge) | `#[cfg_attr(not(feature = "supervisor-db"), allow(dead_code))]` | `// reason: --features supervisor-db 启用时被测试调用` |
| `WaveWorkerStreamHandler`/`new`/`emit_delta`(worker.rs) | 同上 | 同上 |
| `compensation`/`event_count`/`fields`/`OnTimeout/Cancel/Partial`/`Pending/Executed/Failed`(memory.rs) | 模块级 `#[cfg_attr(not(feature = "supervisor-db"), allow(dead_code))]` | `// reason: --features supervisor-db enables supervisor coordination` |
| `merged_waves_skip_recovery`/`restore_unmerged_completed_slot`/`ensure_coordinator_topic_is_recognised`/`handoff_envelope_validation_enabled`/`render_truncated_list`/`WithHat` | `#[cfg_attr(not(feature = "supervisor-db"), allow(dead_code))]` 或分别 module-level allow | 同上 |
| `hat_command_policy::ConfigFault`/`ALL`/`hint`/`is_allow`/`is_deny` | `#[allow(dead_code)]` + `// reason: stable public API for future preset policy rewrites; pinning now避免 churn` | 公共契约守门 |
| `resolve_policy_check_mode`/`legacy_resolve`/`load_policy_config_from_hats_only`/`should_policy_check_emit`/`paths_canonical_differ`/`resolve_project_config_path_with_env`/`close_task_with_context`/`to_human_string`/`ValidationError::new`/`enforce_preset_lint_gate`(2-arg) | `#[allow(dead_code)]` + `// reason: reserved for U15 emit-path parity / U14 cli-tui parity` 类注释 | 公共契约守门 |
| `violation/rejection/terminate` 模块 import(从 `loop_runner/hooks/mod.rs` 等) | 删(若全仓 0 引用) | "确实未用"已确认则走 U1/U3 路径,不在 U4 allow |

**Test scenarios.**
- 默认 build:`cargo build -p ralph-core -p ralph-cli 2>&1 | grep -c warning:` ≤ 15(只留 deprecated + 仍未归类)。
- `--features supervisor-db` build:`cargo build -p ralph-cli --features supervisor-db --tests 2>&1 | grep -c warning:` 同样 ≤ 0(无新增)。

**Verification.** 双向构建 0 warning,`./scripts/run-tests.sh` 仍通过。

---

### U5. 处理 deprecated constants + 命名参数格式 + 最终回归

**Goal.** 关闭剩余 deprecated 与 named-arg 警告,跑全量回归验证零回归。

**Requirements.** R1,R2,R3。

**Files.**
- `crates/ralph-core/src/preset_lint/finding_id.rs`(522, 523 - 移除 deprecated alias)
- `crates/ralph-cli/src/task_cli.rs`(1780 - `caller_hat={caller_hat}` 改用 positional 或 renamed param)

**Approach.**
1. grep `FINDING_REVIEW_TERMINAL_DUAL_SUBSCRIBE` / `FINDING_REVIEW_TERMINAL_PUBLISHER_INCOMPLETE` 全仓引用 = 0 → 整行删除 + 同步删除 `#[deprecated]` attr。
2. `task_cli.rs:1780` 改 `format!("... caller_hat={caller_hat:?}", caller_hat = hat)` 用 positional 形式;或者改 format string 为 `"... caller_hat={0:?}"` 然后 `format!(..., hat)`(取决于原调用 site)。
3. 跑 `cargo build -p ralph-core -p ralph-cli --tests --all-features` 期望 0 warning。
4. 跑 `./scripts/run-tests.sh` 验收 0 失败。若有失败,则按系统级 debug 流程隔离根因 —— 不允许 fallback。

**Test scenarios.**
- 删 deprecated alias 后,无 fallback 引用导致 build 失败(`rg "FINDING_REVIEW_TERMINAL_DUAL_SUBSCRIBE" --type rust` 应为空)。
- `task_cli.rs:1780` 修复后 `cargo build` warning 类 named_argument 计数降到 0。
- 全量回归 `./scripts/run-tests.sh` exit 0,无新增 skipped。

**Verification.** `cargo build -p ralph-cli -p ralph-core 2>&1 | grep -c '^warning:'` = 0;`./scripts/run-tests.sh` exit 0。

---

## Verification Contract

每 Unit 必须依次满足:

1. `cargo build -p ralph-cli -p ralph-core` 0 warning(对应 U 的类别)。
2. `cargo nextest run -p <affected> --no-fail-fast` 0 失败。
3. 全量 `./scripts/run-tests.sh` 仅在 U3、U5 各跑一次,作为回归兜底;U1-U4 用 `cargo nextest run -p ralph-cli -p ralph-core --no-fail-fast` 做局部回归。

最终验收(U5 末尾):
- `cargo build -p ralph-cli -p ralph-core --tests --all-features 2>&1 | grep -c '^warning:' = 0`。
- `./scripts/run-tests.sh` exit 0。
- `cargo clippy -p ralph-cli -p ralph-core --tests --all-features -- -W clippy::all` 不引入新 clippy warning 类(允许存在 `clippy::needless_return` 类遗留不因本计划新增)。

---

## Definition of Done

按 ce-plan 契约:

1. 所有 R-IDs(R1-R6)的验收测试通过,产出 artifact 满足 R1+R2+R3。
2. 每个 Unit 的 Definition of Done 字段已填(详见各 Unit "Verification" 段)。
3. 全量回归 `./scripts/run-tests.sh` 通过;无新增 skipped、无 fallback 静默放过。
4. `cargo build --all-features` 0 warning。
5. `git log -p` 显示 5 个独立 commit,每 Unit 一个,message 含 U-ID(便于 review 与回滚)。
6. Deferred 项(`flow_lifecycle::current_step_id_fallback` 迁移)已记录到 follow-up,不阻塞主线完成。

---

## Risks & Dependencies

- **R-1 (低)**: Supervisor feature-gated dead code 注释不准确会让后续维护者误删。新增 `#[cfg_attr]` 与 1 行注释是防误删的最低成本护栏。
- **R-2 (中)**: clap `migrate-state` 命令的存在性需在 U3 首步验证。若 clap 入口已删,则 migrate_state.rs / tests.rs / cmd 全部作废;若任一保留则必须保留 test。本计划研究阶段已确认存在性可独立验证。
- **R-3 (中)**: 删除 `MockSupervisorBridge`、`WaveWorkerStreamHandler` 涉及 supervisor_bridge 测试 fixture 切换 —— `rg "MockSupervisorBridge::new"` 显示 wave_supervisor.rs 还引用,验证删除后 fixture 替换为 `CoordinatorSupervisorBridge::with_in_memory_store()`。
- **R-4 (中)**: deprecated constants 移除后,跨 crate 可能仍引用 alias(rgrep 验证 0 引用假设)。若发现 1 引用,先迁移再删 alias。
- **R-5 (低)**: U5 末尾 `./scripts/run-tests.sh` 在 pre-commit / hook 间存在潜在 race(在 CI 跑会更稳定)。本地跑需满足 `cargo nextest` 已安装。

---

## Sources & Research

- `cargo build -p ralph-cli -p ralph-core --message-format=short` 输出 74 条 warnings 全文,本地复现命令已固化在 plan 中。
- `rg -n` 链路对 21 个 flagged 函数 + 13 个 unused_imports + 13 个 unused_variables + 17 个 never_used fields/variants 的引用计数验证。
- 公共契约守门(`#[allow(dead_code)]` + comment)模式参考:`crates/ralph-core/src/drift/alert.rs:343-355`。
- Feature-gated 守门 `#[cfg_attr(not(feature = "supervisor-db"), allow(dead_code))]` 模式参考:`crates/ralph-cli/src/preset_merge_table.rs:16-25`。
- CLAUDE.md HARD RULE 1 + 2(测试入口必须 nextest,默认并发),本计划所有 `cargo nextest run` 严格遵循。
