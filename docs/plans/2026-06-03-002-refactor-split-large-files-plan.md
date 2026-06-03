---
title: "重构：拆分 4 个超大源文件为模块化结构"
type: refactor
status: active
date: 2026-06-03
origin: "用户反馈：项目里有 4 个文件超过 5000 行，loop_runner.rs 高达 14733 行，可读性和可维护性严重下降，且 IDE 索引/编译变慢"
related:
  - docs/plans/2026-06-03-001-feat-agent-execution-contract-gates-plan.md
  - docs/plans/2026-05-31-004-feat-agent-operation-guard-plan.md
---

# 重构：拆分 4 个超大源文件为模块化结构

## Overview

本计划是**纯结构性重构**：把 4 个超过 5000 行的 Rust 源文件按逻辑边界拆分成多个子模块/子文件，**严格保持现有公开 API 行为 100% 不变**。所有现有功能、所有现有测试必须继续通过；不允许引入任何行为变化或回归。

被重构的 4 个文件：

| 文件 | 行数 | 现状 |
|------|-----:|------|
| `crates/ralph-cli/src/loop_runner.rs` | 14 733 | 包含 80+ 个顶级 fn、单体 2 933 行 `run_loop_impl`、7 500 行内联测试 |
| `crates/ralph-core/src/event_loop/tests.rs` | 9 152 | 220 个集成测试散落在单文件，仅靠注释分段 |
| `crates/ralph-core/src/config.rs` | 6 278 | 配置中心，13 个逻辑集群、30+ 个 struct、46% 测试代码 |
| `crates/ralph-cli/src/main.rs` | 5 695 | 11 个子命令、570 行共享工具、38% 测试代码 |

**目标**：每个被拆分文件最终不超过 ~2 000 行（除非像 `run_loop_impl` 这种无法机械切分的单体函数），单个子文件控制在 100–500 行之间，公开 API 表面（pub fn、pub struct、pub enum、crate 重导出）逐项保持兼容。

**核心原则**：
1. **不切碎逻辑**——只在逻辑边界划分，不在函数内部机械抽取。
2. **公开 API 字节级保持**——`mod.rs` 重新导出原 `pub` 项，调用方一行 import 都不变。
3. **测试跟着生产代码走**——避免 main.rs / tests.rs 测试大量散落难以维护。
4. **渐进式拆分**——每个 U 完成时跑测试，绿灯后才能进入下一个 U。
5. **不改行为**——禁止"顺手优化"或"趁重构修复 bug"。

---

## Problem Frame

### 当前痛点

- **IDE 索引慢**：14733 行的 `loop_runner.rs` 在 JetBrains/Neovim/LSP 中打开需要数秒，符号跳转延迟明显。
- **git blame 失效**：单文件 7 500 行内联测试混在生产代码中，定位某次提交"为什么改了 loop_runner 的 hook 逻辑"需要人工过滤测试代码。
- **code review 疲劳**：在 PR 中看 14 733 行的 diff，审查者无法抓住重点。
- **新成员 onboarding 难**：新人要理解 `loop_runner.rs` 必须先读完 80+ 个独立函数的语义。
- **测试发现性差**：`event_loop/tests.rs` 单文件 9 152 行 / 220 个测试，没有按主题分类。
- **配置改动风险高**：`config.rs` 6 278 行是 13 个领域的配置中心，改一处可能影响别处。

### 为什么不直接拆分

- **`run_loop_impl` 本身有 2 933 行**——这是单体 async 函数，内部有数十个闭包、可变状态、`tokio::select!` 块。本计划**不在本次拆分 `run_loop_impl` 内部**，只把它的"调用对象"（hooks、payload inputs、late events、hard gate、wave、execution、event_logging、merge_queue 等）抽到子模块。`run_loop_impl` 内部章节抽取作为 follow-up 单独 PR。
- **配置 `pub use` 锁定**：`lib.rs:73-80` 已经一次性 re-export 27 个项，下游 `ralph-cli` 大量依赖 `ralph_core::RalphConfig` 等路径。拆分时必须保留所有现有导出名。
- **clap 派生 cross-module**：`main.rs` 的 `Cli` 派发已经穿过多个子命令文件（doctor/hats/hooks/loops/mcp/tools/wave/web 已经是独立文件），继续把剩余 11 个命令下沉是**沿用现有模式**。
- **`#[serde(untagged)]` HatBackend 顺序敏感**：任何重新排序都会破坏反序列化。
- **3 处 `#[serde(flatten)]`**：必须在原位置保留。

### 范围限定

只拆分这 4 个文件。不在本次重构：

- `event_loop/mod.rs` 4 694 行——已经接近 5 000 但不到，**留作 follow-up**。
- `event_loop/loop_state.rs` 及其他内部文件——体量在合理范围。
- 任何业务逻辑修改、任何 API 改名、任何性能优化、任何 bug 修复。

---

## Requirements

### R1. 公开 API 兼容性
所有 `pub` / `pub(crate)` 项的**完整签名、可见性、行为**在拆分后必须字节级保持一致。
- 外部 crate（`ralph-cli` 调用 `ralph-core`）的 `use` 语句路径不变。
- 内部 crate 的 `use crate::config::X` / `use crate::loop_runner::Y` 路径不变。
- `mod loop_runner;` 仍然非 `pub mod`，对外不暴露。

### R2. 测试 100% 通过
拆分完成后：
- `cargo nextest run --workspace --exclude ralph-e2e` 全部绿灯。
- `cargo test --workspace --exclude ralph-e2e --doc` 全部绿灯。
- 现有 220 个 event_loop tests、142 个 loop_runner tests、~2900 行 config tests、~2200 行 main.rs tests 全部通过。
- 任何原本跳过的测试（如 `acp_executor::tests::test_create_terminal_and_output`）继续跳过。

### R3. 单文件规模
- 4 个被拆分文件**全部降级**为目标规模：
  - `loop_runner/mod.rs` ≤ 200 行
  - `loop_runner/runner.rs` ≤ 3 000 行（容纳未拆的 `run_loop_impl`）
  - 其他子文件 100–500 行
  - `event_loop/tests/mod.rs` ≤ 50 行（仅声明）
  - `event_loop/tests/*.rs` 平均 200–400 行
  - `config/mod.rs` ≤ 800 行
  - `config/*.rs` 平均 100–400 行
  - `main.rs` ≤ 500 行
  - `cli/*.rs` 平均 50–250 行
  - `commands/*.rs` 平均 100–500 行

### R4. 行为零变化
拆分不允许修改：
- 任何函数体逻辑。
- 任何 trait impl。
- 任何 `#[derive(...)]` / `#[serde(...)]` 属性。
- 任何 `Default` 实现。
- 任何 `Display` / `From` / `TryFrom` 实现。
- 任何 panic 信息或错误消息。

### R5. 渐进式可回滚
每个 Implementation Unit（U1–U5）独立可提交、可回滚；每个 U 完成后必须跑测试通过再进入下一个 U。

### R6. 文档同步
- `docs/plans/` 不再追加新 plan。
- README、CLAUDE.md、AGENTS.md 中涉及文件行号的引用（如有）必须更新。
- `docs/guide/iteration-boundary-hooks-and-skills.md` 等引用具体函数路径的文档保持有效（不破坏 `loop_runner::xxx` 路径）。

### R7. 不修改 nextest 配置
`.config/nextest.toml` 已配置 `ralph-cli` 为 `cli-serial`，`ralph-core` 默认并行。拆分后**不引入** `serial_test` 依赖，不改变 nextest 配置。

---

## Scope Boundaries

### In Scope

- 把 4 个超大源文件拆分为多个子模块/子文件。
- 引入共享 `common` / `shared` / `helpers` 子模块（仅供子模块间共享，不暴露 crate 公共 API）。
- 重新组织 `mod.rs` 的 `pub use` 重新导出表。
- 测试代码随生产代码迁移到对应子文件。
- 更新 `.config/nextest.toml`（如果需要，但预期不需要）。
- 提交粒度建议为"一个 U 一个 commit"，便于 review。

### Out of Scope

- **`run_loop_impl`（2 933 行单体函数）内部章节抽取**——这是更深的重构，应作为后续 PR。本计划只把 `run_loop_impl` 的"调用对象"（hooks、payload inputs、late events、hard gate、wave、execution、event_logging、merge_queue 等）抽出。`run_loop_impl` 主体仍保留在 `loop_runner/runner.rs`。
- `event_loop/mod.rs` 4 694 行的进一步拆分——行数已接近但仍小于 5 000，留作 follow-up。
- 任何业务逻辑、错误处理、配置默认值、serde 属性的修改。
- `#[cfg(test)]` 之外的内联测试重构（保留原结构）。
- 性能优化、unsafe 替换、宏抽象。
- 任何依赖更新或 build script 改动。
- 修改 `lib.rs` / `Cargo.toml` 的 `pub use` 列表（保持现状）。
- 新增 benchmark、property test、fuzz test。

### Deferred Follow-Up

- 抽取 `run_loop_impl` 内部章节为命名良好的私有函数（目标 1 500 行以内）。
- 拆分 `event_loop/mod.rs`（4 694 行）按当前 Phase 1/2/3 模式。
- 抽取 `run_command` 与 `resume_command` 的"load config → apply CLI overrides → backend detect → run loop"公共块为 `cli/run_common.rs::prepare_run_config`。
- `cli/commands/` 子命令间的 schema 共享（部分 args 重复定义）。

---

## Context & Existing Patterns

### 现有模块边界

- `ralph-cli/src/` 已经有 9 个子命令独立成文件（`bot.rs` / `doctor.rs` / `hats.rs` / `hooks.rs` / `loops.rs` / `mcp.rs` / `tools.rs` / `wave.rs` / `web.rs`）。继续把剩余 11 个命令下沉是**沿用现有模式**。
- `ralph-core/src/` 没有已存在的 `config/` 或 `event_loop/tests/` 子目录模式。但 `ralph-core/src/event_loop/` 已经有 `mod.rs` + `loop_state.rs`，说明子目录模式可用。
- `ralph-core/tests/` 目录已有 7 个集成测试 `.rs` 文件，可以参考风格。

### 关键依赖关系

- `main.rs:25` 声明 `mod loop_runner;`（非 `pub mod`），所以 `loop_runner` 内部结构对 crate 外不可见。
- `lib.rs:73-80` 的 `pub use config::{...}` 是 `ralph-core` 公开 API 合约。
- `loop_runner` 文件内部 4 处 `#[cfg(test)]` 单函数 / 单结构（不是 `mod tests`），是**生产代码引用的测试 hook**，必须随对应生产代码一起迁移：
  - `detect_solo_output_completion`（L4 777–4 784）
  - `MockAcpExecution` enum（L6 615–6 687）
  - `MOCK_ACP_EXECUTIONS` / `MOCK_ACP_EXECUTION_SERIAL` 全局（L6 689–6 696）
  - `forced_test_wave_pty_failure`（L6 795–6 801）
- `event_loop/tests.rs` 唯一一个嵌套子模块是 `replay_light_integration`（L8 742–9 152），它有自己的 helper 和外部 `git` 依赖。

### 关键风险点

- `#[serde(untagged)] HatBackend` 的变体声明顺序敏感（任何重排破坏反序列化）。
- 3 处 `#[serde(flatten)] extra` 字段（`HooksConfig` / `HookSpec` / `HookMutationConfig`）必须在原位置。
- `ScratchpadConfig` 的两个 `deserialize_with` 函数（`deserialize_scratchpad_config` / `deserialize_optional_scratchpad_config`）必须保留在 `ScratchpadConfig` 同一模块。
- `RalphConfig::validate()`（500+ 行）跨子模块调用所有验证逻辑，建议**保留在 `mod.rs`** 或拆为"每个子模块的 `validate()` 自由函数 + `RalphConfig::validate` 调度"。
- `operation_guard.rs:147` 的注释引用 `loop_runner::inject_hat_execution_env` —— 拆分后该函数仍在 `loop_runner::execution` 子模块，通过 `mod.rs` `pub(crate) use` 暴露，原注释路径**自然保持有效**（因为 `inject_hat_execution_env` 在子模块内是 `pub(super)`，但 `mod.rs` 重新导出后路径变为 `loop_runner::inject_hat_execution_env`，需要 update 一行注释或保留 `mod.rs` 内的 `pub(crate) use` 转发）。

---

## Key Technical Decisions

### KTD1. 拆分方式：目录 + mod.rs 重新导出

对于三个需要拆分的"大文件"采用 Rust 标准的"文件 → 目录 + mod.rs"模式：

| 原文件 | 拆分目标 |
|--------|----------|
| `loop_runner.rs` (14 733 行) | `loop_runner/mod.rs`（200 行）+ `loop_runner/*.rs` 13 个子文件 + `loop_runner/wave/*.rs` 3 个子文件 + `loop_runner/hooks/*.rs` 5 个子文件 + `loop_runner/tests.rs` 0 个新文件（沿用原 7 500 行测试的拆分） |
| `event_loop/tests.rs` (9 152 行) | `event_loop/tests/mod.rs`（50 行）+ `event_loop/tests/common/mod.rs` + `event_loop/tests/*.rs` 28 个主题子文件 + `event_loop/tests/replay_light_integration.rs` |
| `config.rs` (6 278 行) | `config/mod.rs`（800 行）+ `config/*.rs` 18 个子文件 |
| `main.rs` (5 695 行) | `main.rs`（300–500 行）+ `cli/*.rs` 5 个共享子文件 + `commands/*.rs` 11 个子命令文件 |

**理由**：Rust 编译器同时接受 `xxx.rs` 和 `xxx/mod.rs` 两种风格。改 `mod.rs` 不需要改 `mod.rs`（生产者）的 `mod` 声明。

### KTD2. 公开 API 兼容策略：mod.rs pub use 重新导出

`mod.rs` 顶部用 `pub use` 重新导出原文件所有 `pub` 项，调用方一行 import 都不变：

```rust
// loop_runner/mod.rs 示例
pub use runner::run_loop_impl;
pub use merge_queue::process_pending_merges_cli;
pub use start_loop::start_loop;
pub(crate) use loop_owner::{register_loop_owner, register_loop_owner_with_hat};
pub(crate) use execution::ExecutionOutcome;
pub use payload_contract_gate::{
    enforce_payload_contract_gate,
    write_payload_contract_violation_report,
};
```

对 `config/mod.rs` 同样：`pub use sub::{X, Y, Z}` 把 `RalphConfig` 内嵌的所有子配置转出来。

### KTD3. 测试代码随生产代码迁移

**不**把测试集中到顶层 `tests/` 目录（那是 Cargo 集成测试目录，编译为独立 crate，启动慢）。**而**是把 `event_loop/tests.rs` → `event_loop/tests/` 子目录，`loop_runner.rs` 的内联 `mod tests` → `loop_runner/tests.rs` 或子目录，`main.rs` 的内联 `mod tests` → 各 `commands/*.rs` 的内联 `mod tests`。

每个子文件只放与该子文件生产代码相关的测试，共享 helper 提取到 `tests/common/` 目录（如 `event_loop/tests/common/mod.rs`）。

### KTD4. helper 共享策略：common 子目录 + pub(super)

`event_loop/tests.rs` 的 7 个共享 helper（`write_event_to_jsonl`、`write_event_with_hat_to_jsonl`、`write_object_event_to_jsonl`、`collect_pending_topics`、`MockRobotService`、`RestartRequestRobotService` 等）提取到 `event_loop/tests/common/mod.rs`，用 `pub(super)` 限定可见性。子文件 `use crate::event_loop::tests::common::*;`。

`loop_runner` 内联测试的 50+ 个 fixture / helper 提取到 `loop_runner/test_helpers.rs`（受 `#[cfg(test)] pub(super)` 限定）。

`main.rs` 测试主要依赖 `super::*`，跟随各 `commands/*.rs` 自动迁移即可。

### KTD5. `run_loop_impl` 不在本次拆分

`run_loop_impl` 2 933 行的内部有数十个闭包、可变状态、`tokio::select!` 块、可变状态绑定多到无法机械抽取。建议**先按本方案完成"按区域划线"的拆分**，让 `runner.rs` 成为单独文件后，再在后续 PR 中逐步抽取 `run_loop_impl` 内部章节为命名良好的私有函数。

本次拆分中，`run_loop_impl` 整体**保留在 `loop_runner/runner.rs`**，其引用的辅助函数（`enforce_payload_contract_gate`、`inject_hat_execution_env`、`execute_acp`、`execute_pty`、`process_pending_merges` 等）下沉到子模块，`runner.rs` 用 `use super::xxx::yyy;` 引用。

### KTD6. 拆分顺序：风险从低到高

按"先安全、后高风险"原则排序：

1. **U1. 公共基础设施**（最安全，只是准备）—— 提取共享 helper 路径
2. **U2. 拆 `event_loop/tests.rs`**（纯测试代码，无公开 API 影响）
3. **U3. 拆 `config.rs`**（纯数据 + serde，编译期能保证不破坏）
4. **U4. 拆 `main.rs`**（clap 派发结构调整，外部调用方不变）
5. **U5. 拆 `loop_runner.rs`**（最大、最复杂，**有 `#[cfg(test)]` 内部 hook 需随生产代码迁移**）
6. **U6. 完整验证 + 文档同步**（最终绿灯 + 行数审计）

### KTD7. `#[cfg(test)]` 内部 hook 必须随生产代码迁移

`loop_runner.rs` 文件主体中有 4 处 `#[cfg(test)]`（不是 `mod tests` 内）：

| 行 | 项目 | 引用方 |
|---:|------|--------|
| L4 777–4 784 | `fn detect_solo_output_completion` | `run_loop_impl` 在 hard gate 路径调用 |
| L6 615–6 687 | `enum MockAcpExecution` | `execute_wave_worker_acp` 在 test 模式使用 |
| L6 689–6 696 | `static MOCK_ACP_EXECUTIONS` / `MOCK_ACP_EXECUTION_SERIAL` | `MockAcpExecution` 配套 |
| L6 795–6 801 | `fn forced_test_wave_pty_failure` | `run_wave_worker_pty` 在 test 模式使用 |

这些是**生产代码引用的测试 hook**，必须随对应生产代码一起迁移：

- `detect_solo_output_completion` → `loop_runner/hard_gate.rs`（hard gate 模块）
- `MockAcpExecution` + `MOCK_ACP_EXECUTIONS` + `MOCK_ACP_EXECUTION_SERIAL` → `loop_runner/wave/acp_mock.rs`
- `forced_test_wave_pty_failure` → `loop_runner/wave/worker.rs`

### KTD8. `event_loop/tests.rs` 拆分时不引入 `serial_test`

`event_loop/tests.rs` 的 220 个测试**不依赖** `serial_test`（已确认无 `serial_test` crate 依赖，无全局 mutex 状态）。拆分到子目录后**继续并行**运行，无需调整 `.config/nextest.toml`。

---

## Proposed File Structure

### `loop_runner/`（替换原 `loop_runner.rs`）

```
crates/ralph-cli/src/
├── loop_runner.rs                 # 删除，改为目录 mod
└── loop_runner/
    ├── mod.rs                     # 200 行，pub use 重新导出
    ├── runner.rs                  # ~2 950 行，run_loop_impl 主体
    ├── payload_contract_gate.rs   # 100 行
    ├── late_events.rs             # 80 行
    ├── hard_gate.rs               # 180 行（含 detect_solo_output_completion）
    ├── payload_inputs.rs          # 150 行
    ├── suspend.rs                 # 110 行
    ├── output_parsing.rs          # 70 行
    ├── paths.rs                   # 60 行
    ├── loop_owner.rs              # 80 行
    ├── execution.rs               # 230 行（execute_acp + execute_pty + inject_hat_execution_env + prepare_tui_iteration）
    ├── event_logging.rs           # 130 行
    ├── prompt.rs                  # 170 行
    ├── merge_queue.rs             # 180 行
    ├── start_loop.rs              # 140 行（start_loop + create_robot_service）
    ├── exit_conditions.rs         # 140 行
    ├── test_helpers.rs            # ~800 行，#[cfg(test)] pub(super) helper
    ├── hooks/
    │   ├── mod.rs                 # 50 行
    │   ├── dispatch.rs            # 280 行
    │   ├── retry.rs               # 180 行
    │   ├── mutation.rs            # 200 行
    │   ├── termination.rs         # 140 行
    │   └── format.rs              # 100 行
    └── wave/
        ├── mod.rs                 # 50 行
        ├── dispatcher.rs          # 490 行
        ├── worker.rs              # 540 行（含 forced_test_wave_pty_failure）
        ├── acp_mock.rs            # 90 行（MockAcpExecution + 全局，含 #[cfg(test)]）
        └── io.rs                  # 180 行
```

### `event_loop/tests/`（替换原 `event_loop/tests.rs`）

```
crates/ralph-core/src/event_loop/
├── mod.rs                         # 已有，不变
├── loop_state.rs                  # 已有，不变
├── tests.rs                       # 删除
└── tests/
    ├── mod.rs                     # 50 行，声明所有子文件
    ├── common/
    │   └── mod.rs                 # 150 行，7 个 helper + 2 个 mock service
    ├── initialization.rs          # 段 1
    ├── termination.rs             # 段 2 + 13 + 15
    ├── build_prompt.rs            # 段 3
    ├── default_publishes.rs       # 段 4
    ├── hat_backend.rs             # 段 5 + 26
    ├── wave_results.rs            # 段 6
    ├── active_hat.rs              # 段 7 + 8
    ├── objective.rs               # 段 9
    ├── scratchpad.rs              # 段 10（scratchpad 部分）
    ├── backpressure.rs            # 段 10（backpressure 部分）
    ├── robot_skill.rs             # 段 11
    ├── persistent_mode.rs         # 段 12
    ├── loop_context.rs            # 段 16
    ├── hat_exhaustion.rs          # 段 17
    ├── scope_enforcement.rs       # 段 18
    ├── chain_validation.rs        # 段 19
    ├── human_timeout.rs           # 段 20 + 21
    ├── text_fallback.rs           # 段 22
    ├── event_filter.rs            # 段 23
    ├── workflow_guard.rs          # 段 24
    ├── payload_types.rs           # 段 27
    ├── origin_guard.rs            # 段 28 + 29
    ├── event_policy.rs            # 段 30
    ├── completion_honored.rs      # 段 31
    ├── state_machine.rs           # 段 32 + 33
    ├── structured_evidence.rs     # 段 34
    ├── stale_breaker.rs           # 段 35
    ├── ce_executor.rs             # 段 36 + 37
    ├── execution_contract.rs      # 段 38 + 39
    └── replay_light_integration.rs # 段 40
```

### `config/`（替换原 `config.rs`）

```
crates/ralph-core/src/
├── config.rs                      # 删除
└── config/
    ├── mod.rs                     # 800 行，RalphConfig + Default + 验证入口
    ├── core.rs                    # CoreConfig + ScratchpadConfig + deserialize_with
    ├── cli.rs                     # CliConfig + TuiConfig
    ├── loop_config.rs             # EventLoopConfig + Phase/PhaseConfig/WarmupConfig/VerdictGateConfig/HatExecutionMode
    ├── event_policy.rs            # EventPolicyConfig + 5 个子类型/枚举
    ├── state_machine.rs           # StateMachineConfig + 4 个子类型/枚举
    ├── workflow_guards.rs         # WorkflowGuardsConfig + 子类型
    ├── execution_contracts.rs     # ExecutionContractsConfig + 子类型
    ├── event_filter.rs            # EventFilterConfig + EventFilterMode
    ├── event_projection.rs        # EventProjectionConfig + 子类型
    ├── state_files.rs             # StateFilesConfig + 子类型
    ├── preflight_ext.rs           # PreflightExtensionsConfig + 子类型
    ├── features.rs                # FeaturesConfig + PreflightConfig
    ├── memories.rs                # MemoriesConfig + MemoriesFilter + InjectMode
    ├── tasks.rs                   # TasksConfig
    ├── skills.rs                  # SkillsConfig + SkillOverride
    ├── hooks.rs                   # HooksConfig + 7 个子类型
    ├── hat.rs                     # HatConfig + HatBackend（untagged）+ Aggregate + EventMetadata
    ├── robot.rs                   # RobotConfig + TelegramBotConfig
    ├── v1_adapters.rs             # AdaptersConfig + AdapterSettings
    ├── warning.rs                 # ConfigWarning
    ├── error.rs                   # ConfigError
    └── defaults.rs                # 30+ 个 default_* 自由函数
```

### `cli/` + `commands/`（拆分 `main.rs`）

```
crates/ralph-cli/src/
├── main.rs                        # 300–500 行，fn main + Cli + Commands 派发
├── cli/
│   ├── mod.rs                     # 50 行
│   ├── shared.rs                  # ColorMode/Verbosity/OutputFormat/ConfigSource/HatsSource
│   ├── config_loader.rs           # default_config_path / load_config_with_overrides / apply_config_overrides / ensure_scratchpad_directory / resolve_*_workspace_root
│   ├── emit_path.rs               # resolve_emit_path / resolve_marker_target
│   ├── panic_hook.rs              # install_panic_hook
│   └── process_management.rs      # 平台分支
└── commands/
    ├── mod.rs                     # 50 行
    ├── run.rs                     # ~700 行（最大）
    ├── resume.rs                  # ~120 行
    ├── init.rs                    # ~60 行
    ├── events.rs                  # ~100 行
    ├── clean.rs                   # ~90 行
    ├── emit.rs                    # ~250 行
    ├── tutorial.rs                # ~140 行
    ├── plan.rs                    # ~50 行
    ├── code_task.rs               # ~50 行
    ├── tui.rs                     # ~15 行
    └── completions.rs             # ~25 行
```

---

## Implementation Units

### U1. 公共基础设施 + 测试夹具集中

**Goal:** 为后续 U 准备共享 helper 路径，验证拆分机制可行。

**Requirements:** R5, R7

**Dependencies:** 无

**Files:**

- Create: `crates/ralph-core/src/event_loop/tests/common/mod.rs`（暂存，后续 U2 引用）

**Approach:**

- 暂不正式拆分任何文件，先在 `event_loop/tests.rs` 顶部加一个占位 `mod common { ... }` 子模块，把 7 个共享 helper + 2 个 mock service **复制**进去（**不删**原文件中的 helper）。
- 跑 `cargo nextest run -p ralph-core event_loop::tests` 确认测试通过。
- 这一步只为 U2 做准备，不引入风险。

**Test scenarios:**

- 现有 220 个 event_loop tests 全部通过。
- 编译警告：无未使用 import。

**Verification:**

- `rtk cargo nextest run -p ralph-core event_loop::tests --no-fail-fast`

### U2. 拆分 `event_loop/tests.rs` → `event_loop/tests/`

**Goal:** 把 9 152 行 / 220 个测试的巨型单文件拆为 30 个主题子文件。

**Requirements:** R1, R2, R5, R7

**Dependencies:** U1

**Files:**

- Delete: `crates/ralph-core/src/event_loop/tests.rs`
- Create: `crates/ralph-core/src/event_loop/tests/mod.rs`（50 行）
- Create: `crates/ralph-core/src/event_loop/tests/common/mod.rs`（150 行）
- Create: 28 个主题子文件（如上结构）
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（仅当 `mod tests;` 声明需要时——实际不需要，Rust 同时接受 `tests.rs` 和 `tests/mod.rs`）

**Approach:**

- 把 `tests.rs` 重命名为 `tests/mod.rs`，内容替换为：
  ```rust
  #[cfg(test)]
  mod common;
  mod initialization;
  mod termination;
  // ... 其他 27 个 mod 声明
  mod replay_light_integration;
  ```
- 把 7 个共享 helper + 2 个 mock service 移到 `tests/common/mod.rs`，用 `pub(super)` 限定可见性。
- 按上述映射表把 220 个测试函数分配到 28 个主题子文件，每个子文件 `use super::common::*;` 引用共享 helper。
- `replay_light_integration` 整体（7 个测试 + 8 个 helper）作为一个独立子文件，保留 `mod replay_light_integration { ... }` 内部结构。
- `tempfile`、`serde_yaml`、`ralph_proto::Event` 等公共 `use` 集中在子文件顶部。

**Test scenarios:**

- 全部 220 个测试通过。
- 编译无新警告。
- 单测试可独立运行：`cargo test -p ralph-core event_loop::tests::initialization::test_X`。

**Verification:**

- `rtk cargo nextest run -p ralph-core event_loop::tests --no-fail-fast`
- `rtk cargo nextest run -p ralph-core --no-fail-fast`（整个 ralph-core 测试集）
- 抽查 5 个不同主题的测试，单独运行通过。

### U3. 拆分 `config.rs` → `config/`

**Goal:** 把 6 278 行 / 13 集群的配置中心拆为 18+ 个子模块。

**Requirements:** R1, R2, R4, R5

**Dependencies:** 无（可与 U2 并行）

**Files:**

- Delete: `crates/ralph-core/src/config.rs`
- Create: `crates/ralph-core/src/config/mod.rs`（800 行）
- Create: 18 个子文件（如上结构）
- Modify: `crates/ralph-core/src/lib.rs`（仅当需要 `pub mod config;` 而非 `pub use config::*`——预期不需要，`mod config;` 即可）

**Approach:**

- **第一步**：把 `config.rs` 重命名为 `config/mod.rs`，内容**不变**。
- **第二步**：按集群顺序逐个抽到子文件（顺序见 KTD6：先 error → warning → robot → hat → core → event_policy → state_machine → workflow_guards → execution_contracts → event_filter → event_projection → state_files → preflight_ext → features → memories → tasks → skills → hooks → cli/tui/v1_adapters → defaults）：
  1. 在 `config/mod.rs` 顶部加 `mod xxx;`。
  2. 把对应 struct/enum/fn **剪切**到 `config/xxx.rs`。
  3. 在 `config/mod.rs` 加 `use xxx::Y;` 或 `pub use xxx::Y;`（依据可见性）。
  4. 跑 `cargo nextest run -p ralph-core --no-fail-fast`，确认通过。
- **关键约束**：
  - `HatBackend` 的 `#[serde(untagged)]` 变体顺序**不重排**。
  - 3 处 `#[serde(flatten)] extra` 字段**保留在原 struct 同一文件**。
  - `ScratchpadConfig` 的两个 `deserialize_with` 函数跟随 `ScratchpadConfig` 移到 `core.rs`。
  - `RalphConfig::validate()` **保留在 `mod.rs`**；如需拆为子模块 `validate()`，**逐个添加**为 `RalphConfig::validate` 内的委托调用。
  - `pub use` 在 `mod.rs` 顶部一次性重新导出所有原 `pub` 项，保持 `lib.rs:73-80` 的 `pub use config::{...}` 合约。
- **测试代码**：行内 `#[cfg(test)] mod tests` **保留在 `mod.rs`**（因为大多数测试都是 `RalphConfig` 端到端行为）。`test_hat_backend_*` 4 个测试可跟随 `HatBackend` 移到 `hat.rs` 内的 `#[cfg(test)] mod tests`。

**Test scenarios:**

- 全部 config tests（~50+ 个）通过。
- `validate_*` 系列测试通过（验证错误信息不变）。
- 序列化反序列化回归测试通过。
- v1 兼容测试通过。
- `#[serde(untagged)] HatBackend` 4 个变体测试通过。
- 3 个 `#[serde(flatten)]` 测试通过。

**Verification:**

- `rtk cargo nextest run -p ralph-core --no-fail-fast`
- `rtk cargo test -p ralph-core --doc --no-fail-fast`
- 抽查 5 个 preset 的 YAML 解析无变化。

### U4. 拆分 `main.rs` → `cli/` + `commands/`

**Goal:** 把 5 695 行 / 22 个 clap 变体 / 11 个子命令的入口文件拆为共享层 + 命令层。

**Requirements:** R1, R2, R4, R5

**Dependencies:** U3（建议；非强制）

**Files:**

- Modify: `crates/ralph-cli/src/main.rs`（缩到 300–500 行）
- Create: `crates/ralph-cli/src/cli/mod.rs`
- Create: `crates/ralph-cli/src/cli/shared.rs`
- Create: `crates/ralph-cli/src/cli/config_loader.rs`
- Create: `crates/ralph-cli/src/cli/emit_path.rs`
- Create: `crates/ralph-cli/src/cli/panic_hook.rs`
- Create: `crates/ralph-cli/src/cli/process_management.rs`
- Create: `crates/ralph-cli/src/commands/mod.rs`
- Create: 11 个 `commands/*.rs`

**Approach:**

- **第一步**：抽共享层（行 147–717）到 `cli/`：
  - `ColorMode` / `Verbosity` / `OutputFormat` / `ConfigSource` / `HatsSource` → `cli/shared.rs`
  - `default_config_path` / `load_config_with_overrides` / `apply_config_overrides` / `ensure_scratchpad_directory` / `resolve_workspace_root` / `discover_workspace_root` / `resolve_path_from_workspace` / `urgent_steer_path_from_workspace` → `cli/config_loader.rs`
  - `resolve_emit_path` / `resolve_marker_target` → `cli/emit_path.rs`
  - `install_panic_hook` → `cli/panic_hook.rs`
  - `mod process_management` → `cli/process_management.rs`
  - 跑 `cargo nextest run -p ralph-cli --no-fail-fast` 确认通过。
- **第二步**：按 KTD6 顺序逐个下沉子命令（先易后难）：
  1. `commands/completions.rs`（25 行，最小）
  2. `commands/tui.rs`（15 行）
  3. `commands/plan.rs` + `commands/code_task.rs`（各 50 行）
  4. `commands/init.rs`（60 行）
  5. `commands/tutorial.rs`（140 行）
  6. `commands/events.rs`（100 行）
  7. `commands/clean.rs`（90 行）
  8. `commands/emit.rs`（250 行）
  9. `commands/resume.rs`（120 行）
  10. `commands/run.rs`（700 行，最大）
  - 每个子命令抽完后，main.rs 顶部加 `mod commands;` 和 `use commands::xxx;`，把派发改为 `Some(Commands::Run(args)) => commands::run::run(args).await,`。
  - 测试代码跟随生产代码迁移到对应 `commands/*.rs` 的 `#[cfg(test)] mod tests`。
- **第三步**：最小化 main.rs：只保留 `Cli` / `Commands` 派发 + `fn main()` + tracing 初始化。
- **关键约束**：
  - `Cli` 和 `Commands` enum **保留在 main.rs**（clap 派生）。
  - `*Args` 结构体可以下沉到对应 `commands/*.rs`，在 `Commands` enum 中用 `Commands::Run(commands::run::RunArgs)` 引用。
  - `ralph_cli::clean_diagnostics` 路径不变（已通过 `lib.rs` 暴露）。

**Test scenarios:**

- 全部 main.rs 测试（~50+ 个）通过。
- clap 全局 flag / 子命令解析测试通过。
- `test_cli_parses_global_hats_flag` / `test_bot_daemon_parses_global_config_flag` 等 clap 测试通过。
- `check_events_clear_confirm` 测试通过（已 pub(crate) 暴露）。
- 端到端：`./target/debug/ralph --help` 输出与重构前一致。

**Verification:**

- `rtk cargo nextest run -p ralph-cli --no-fail-fast`
- `rtk cargo build -p ralph-cli`
- `./target/debug/ralph --help`
- `./target/debug/ralph completions zsh > /tmp/comp.zsh && diff <(./target/debug/ralph completions zsh) <(git show HEAD:scripts/ralph-zsh-plugin.zsh | head -50)`

### U5. 拆分 `loop_runner.rs` → `loop_runner/`

**Goal:** 把 14 733 行 / 80+ 顶级 fn / 142 个内联测试的巨型文件拆为 13 + 5 + 4 = 22 个子模块。

**Requirements:** R1, R2, R4, R5, R7

**Dependencies:** U3, U4

**Files:**

- Delete: `crates/ralph-cli/src/loop_runner.rs`
- Create: `crates/ralph-cli/src/loop_runner/mod.rs`（200 行）
- Create: 13 个一级子文件（payload_contract_gate / late_events / hard_gate / payload_inputs / suspend / output_parsing / paths / loop_owner / execution / event_logging / prompt / merge_queue / start_loop / exit_conditions）
- Create: `loop_runner/test_helpers.rs`（~800 行）
- Create: `loop_runner/hooks/{mod, dispatch, retry, mutation, termination, format}.rs`
- Create: `loop_runner/wave/{mod, dispatcher, worker, acp_mock, io}.rs`

**Approach:**

- **第一步**：把 `loop_runner.rs` 重命名为 `loop_runner/mod.rs`，内容**不变**。
- **第二步**：按 KTD6 顺序逐个抽到子文件：
  1. `payload_contract_gate.rs`（100 行，公开 API）
  2. `paths.rs`（60 行）
  3. `output_parsing.rs`（70 行）
  4. `late_events.rs`（80 行）
  5. `hard_gate.rs`（180 行，含 `detect_solo_output_completion`）
  6. `payload_inputs.rs`（150 行）
  7. `suspend.rs`（110 行）
  8. `loop_owner.rs`（80 行）
  9. `execution.rs`（230 行，含 `inject_hat_execution_env` / `execute_acp` / `execute_pty` / `prepare_tui_iteration`）
  10. `event_logging.rs`（130 行）
  11. `prompt.rs`（170 行）
  12. `merge_queue.rs`（180 行）
  13. `start_loop.rs`（140 行，含 `start_loop` + `create_robot_service`）
  14. `exit_conditions.rs`（140 行）
  15. `runner.rs`（最大块，`run_loop_impl` 整体）
  16. `hooks/{mod, dispatch, retry, mutation, termination, format}.rs`
  17. `wave/{mod, dispatcher, worker, acp_mock, io}.rs`
- **关键约束**：
  - **4 处 `#[cfg(test)]` 内部 hook 随生产代码迁移**（KTD7）：
    - `detect_solo_output_completion` → `hard_gate.rs`（生产函数 `#[cfg(test)]` 形式保留）
    - `MockAcpExecution` + `MOCK_ACP_EXECUTIONS` + `MOCK_ACP_EXECUTION_SERIAL` → `wave/acp_mock.rs`（生产代码内 `#[cfg(test)]` 形式保留）
    - `forced_test_wave_pty_failure` → `wave/worker.rs`
  - **公开 API 重新导出**（在 `mod.rs` 顶部）：
    ```rust
    pub use runner::run_loop_impl;
    pub use merge_queue::process_pending_merges_cli;
    pub use start_loop::start_loop;
    pub(crate) use loop_owner::{register_loop_owner, register_loop_owner_with_hat};
    pub(crate) use execution::ExecutionOutcome;
    pub use payload_contract_gate::{
        enforce_payload_contract_gate,
        write_payload_contract_violation_report,
    };
    ```
  - `run_loop_impl` 整体保留在 `runner.rs`，但把所有 `use` 改为 `use super::xxx;` 形式。
  - `rpc_stdin`、`process_management`、`display` 的 `use` 路径不变（`crate::rpc_stdin::*`、`crate::process_management`、`crate::display`）。
  - `RpcSharedState` 在 `mod.rs` 顶层 `struct` 声明保留，因为它是 RPC 主循环的关键状态。或者移到 `runner.rs` 内私有。
  - **测试代码**：行内 `#[cfg(test)] mod tests`（L7 217–14 733）**保留在 `mod.rs`** 或下沉到 `loop_runner/test_helpers.rs`。**建议**：测试分散到各对应子文件的 `#[cfg(test)] mod tests`，共享 helper 提取到 `loop_runner/test_helpers.rs`（受 `#[cfg(test)] pub(super)` 限定）。
  - 4 处 `#[cfg(test)]` 内部 hook（KTD7）的 helper 引用：例如 `MOCK_ACP_EXECUTIONS` 被 `execute_wave_worker_acp`（在 `wave/worker.rs`）使用，`acp_mock.rs` 用 `pub(crate) static`（生产 crate 可见但限定为 `pub(crate)`，仅在 `cfg(test)` 编译时启用）— 实际更稳妥的做法是**保持**为 `static` 但放在 `wave/acp_mock.rs`，配合 `#[cfg(test)]` 限定使用方。

**Test scenarios:**

- 全部 142 个 loop_runner tests 通过。
- 全部 142 个 test fixture / helper 仍能正常构造。
- 全部 4 处 `#[cfg(test)]` 内部 hook 仍能在生产代码中被测试模式调用。
- 公开 API 调用方 `main.rs` / `loops.rs` / `bot.rs` 行为不变。
- `loop_runner::run_loop_impl`、`loop_runner::process_pending_merges_cli`、`loop_runner::start_loop`、`loop_runner::register_loop_owner_with_hat` 这 4 个公开 API 路径仍可访问。

**Verification:**

- `rtk cargo nextest run -p ralph-cli --no-fail-fast`
- `rtk cargo nextest run -p ralph-core --no-fail-fast`
- `rtk cargo test --workspace --exclude ralph-e2e --doc --no-fail-fast`
- 抽查 5 个不同集群的测试单独运行通过。

### U6. 完整验证 + 文档同步

**Goal:** 验证整个重构在 CI 级别绿灯，文档同步更新。

**Requirements:** R1, R2, R5, R6

**Dependencies:** U2, U3, U4, U5

**Files:**

- Modify if needed: `docs/guide/iteration-boundary-hooks-and-skills.md`（如引用了具体行号）
- Modify if needed: `CLAUDE.md` / `AGENTS.md`（如"Key Files"或"Code Locations"列了具体行号）
- Modify: `.config/nextest.toml`（如需要，但预期不需要）

**Approach:**

- 跑完整测试套件：
  - `rtk cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast`
  - `rtk cargo test --workspace --exclude ralph-e2e --doc --no-fail-fast`
  - `rtk cargo clippy --workspace --all-targets --no-fail-fast`
  - `rtk cargo fmt --check`
- 跑 e2e mock 模式（CI 安全）：
  - `rtk cargo run -p ralph-e2e -- --mock --no-fail-fast`
- 跑 BDD 场景：
  - `rtk cargo test -p ralph-core scenarios --no-fail-fast`
- 行数审计：
  - 4 个原大文件全部消失。
  - `loop_runner/mod.rs` ≤ 200 行
  - `loop_runner/runner.rs` ≤ 3 000 行
  - `loop_runner/*` 单文件 ≤ 600 行
  - `event_loop/tests/mod.rs` ≤ 50 行
  - `event_loop/tests/*.rs` 平均 ≤ 400 行
  - `config/mod.rs` ≤ 800 行
  - `config/*.rs` 单文件 ≤ 500 行
  - `main.rs` ≤ 500 行
  - `cli/*.rs` 单文件 ≤ 250 行
  - `commands/*.rs` 单文件 ≤ 700 行
- 文档同步：
  - `CLAUDE.md` "Key Files" 表格移除 `loop_runner.rs` / `tests.rs` / `config.rs` / `main.rs` 单独行号（改为目录）
  - `CLAUDE.md` "Code Locations" 行号引用（如有）需更新
  - `AGENTS.md` 同步更新（CLAUDE.md / AGENTS.md 同步规则）

**Test scenarios:**

- 完整 workspace 测试通过。
- 完整 workspace doctest 通过。
- clippy 0 警告。
- fmt check 通过。
- BDD scenarios 通过。
- e2e mock 通过。
- 行数审计通过。

**Verification:**

- `rtk cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast`
- `rtk cargo test --workspace --exclude ralph-e2e --doc --no-fail-fast`
- `rtk cargo clippy --workspace --all-targets --no-fail-fast`
- `rtk cargo fmt --check`
- `rtk cargo run -p ralph-e2e -- --mock --no-fail-fast`
- `rtk cargo test -p ralph-core scenarios --no-fail-fast`
- 写一个 `scripts/audit-file-sizes.sh` 校验行数。

---

## Sequencing

1. **U1 准备**：抽出 `event_loop/tests/common/mod.rs` 路径（不删原文件），验证机制可行。**U1 必须最先**。
2. **U2 拆 event_loop/tests.rs**：风险最低，纯测试代码，无公开 API 影响。**U2 可与 U3 并行**。
3. **U3 拆 config.rs**：纯数据 + serde，编译期可保证不破坏。**U3 可与 U2 并行**。
4. **U4 拆 main.rs**：clap 派发结构调整，但外部调用方不变。**U4 建议在 U3 之后**，因为 U3 调整的 config 类型可能间接影响 main.rs 的测试。
5. **U5 拆 loop_runner.rs**：最大、最复杂。**U5 必须在 U4 之后**，因为 `main.rs` 内部对 `loop_runner` 的 import 会因 `mod loop_runner;` 保持不变而稳定。
6. **U6 最终验证 + 文档**：在 U2-U5 全部绿灯后进行。

---

## Test Matrix

| Area | Command |
|------|---------|
| Event loop 单元/集成（拆分后） | `rtk cargo nextest run -p ralph-core event_loop::tests --no-fail-fast` |
| Config 反序列化（拆分后） | `rtk cargo nextest run -p ralph-core config::tests --no-fail-fast` |
| Config doctest | `rtk cargo test -p ralph-core config --doc --no-fail-fast` |
| Main.rs clap 解析 | `rtk cargo nextest run -p ralph-cli --no-fail-fast` |
| Loop runner 全部测试 | `rtk cargo nextest run -p ralph-cli loop_runner --no-fail-fast` |
| 全 workspace 测试 | `rtk cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast` |
| 全 workspace doctest | `rtk cargo test --workspace --exclude ralph-e2e --doc --no-fail-fast` |
| Clippy | `rtk cargo clippy --workspace --all-targets --no-fail-fast` |
| 格式 | `rtk cargo fmt --check` |
| BDD 场景 | `rtk cargo test -p ralph-core scenarios --no-fail-fast` |
| E2E mock | `rtk cargo run -p ralph-e2e -- --mock --no-fail-fast` |
| 串行 fallback（无 nextest） | `rtk cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output` |
| 行数审计 | `bash scripts/audit-file-sizes.sh`（U6 引入） |

---

## Risks and Mitigations

- **Risk: 公开 API 不小心被改签名。**  
  Mitigation: 拆分前先 `rtk cargo build --workspace` 确认 baseline OK；每个 U 完成后用 `cargo doc --no-deps` 验证导出表未变；调用方 import 路径不变作为硬性验收。

- **Risk: 共享 helper 提取到 common 后出现编译错误（use 路径错误）。**  
  Mitigation: 抽 common 时**先复制不删**原文件中的 helper，验证子文件能用 `use super::common::*;` 访问后再删除原文件中的 helper。

- **Risk: 4 处 `#[cfg(test)]` 内部 hook 在拆分后找不到引用方。**  
  Mitigation: 抽每个生产函数时同步迁移它的 `#[cfg(test)]` 内部 hook；不让任何 hook"孤儿化"。

- **Risk: `#[serde(untagged)] HatBackend` 变体顺序被重排。**  
  Mitigation: 抽 `hat.rs` 时整段声明原封不动复制；用 `git diff` 验证 `HatBackend` enum 段字节级未变。

- **Risk: `RalphConfig::validate()` 跨子模块验证逻辑被遗漏。**  
  Mitigation: 验证逻辑**保留在 `mod.rs`**；如需拆分为子模块 `validate()`，**逐个添加**为 `RalphConfig::validate` 内的委托调用，不一次性迁移。

- **Risk: `run_loop_impl` 内部函数闭包状态绑定丢失。**  
  Mitigation: `run_loop_impl` 整体保留在 `runner.rs` 不切分；只切外层（它调用的辅助函数）。`run_loop_impl` 内部章节抽取作为 follow-up。

- **Risk: tests 跨子文件共享 fixture 冲突。**  
  Mitigation: 共享 helper 集中在 `tests/common/mod.rs`，用 `pub(super)` 限定可见性；每个子文件 `use super::common::*;` 独立引用。

- **Risk: 子命令派发从 main.rs 移到 commands/* 后 clap 派生失败。**  
  Mitigation: `Cli` / `Commands` enum 保留在 main.rs；`*Args` 可以下沉到 `commands/*.rs` 通过 `Commands::Run(commands::run::RunArgs)` 引用，这是已存在的 9 个子命令文件遵循的模式。

- **Risk: 拆分后编译时间反而变长。**  
  Mitigation: 接受。Rust 增量编译在子模块化后通常会**更快**，因为单文件改动只会重编译受影响的子模块。

- **Risk: 拆分引入新的循环依赖。**  
  Mitigation: 依赖图经分析是 DAG（参见 KTD1-KTD7），没有循环。每个子模块仅依赖其下层或同层。CI 编译能立即发现循环。

- **Risk: 测试运行时间增加。**  
  Mitigation: 测试并行度由 `.config/nextest.toml` 决定，本次不修改；`event_loop/tests.rs` 拆分后 `cargo test` 默认就是并行（单 binary 多 thread），nextest 也并行，**不会变慢**。

- **Risk: `replay_light_integration` 7 个测试的 `git` 外部依赖在 CI 容器中失败。**  
  Mitigation: 保留 `replay_light_integration` 为子模块原样，helper 内 `init_git_repo` 已经有 `Command::new("git")` 调用，行为不变。

- **Risk: `MOCK_ACP_EXECUTIONS` 全局 `static` 在拆分后作用域失效。**  
  Mitigation: 把 `MOCK_ACP_EXECUTIONS` 和 `MOCK_ACP_EXECUTION_SERIAL` 移到 `wave/acp_mock.rs`，用 `pub(crate) static`（生产 crate 可见但**仅在 `cfg(test)` 时被使用**）— 实际更稳妥的做法是用 `#[cfg(test)] pub(crate) static` 限定为只在 test 编译时存在。

---

## Acceptance Criteria

- **AC1** 4 个原大文件全部消失，替换为目录 + mod.rs 结构。
- **AC2** 4 个原大文件**无**单独 `.rs` 文件残留。
- **AC3** 所有现有测试通过（`cargo nextest run --workspace --exclude ralph-e2e` 0 failure）。
- **AC4** 所有现有 doctest 通过（`cargo test --workspace --exclude ralph-e2e --doc` 0 failure）。
- **AC5** clippy 0 警告（pedantic 仍开启）。
- **AC6** `cargo fmt --check` 通过。
- **AC7** e2e mock 通过。
- **AC8** BDD scenarios 通过。
- **AC9** 行数审计脚本通过：4 个原大文件对应的目录结构满足 KTD1 提出的目标规模。
- **AC10** 公开 API 路径不变（`loop_runner::run_loop_impl` / `loop_runner::process_pending_merges_cli` / `loop_runner::start_loop` / `loop_runner::register_loop_owner_with_hat` / `loop_runner::ExecutionOutcome` / `loop_runner::enforce_payload_contract_gate` / `loop_runner::write_payload_contract_violation_report` 仍可访问）。
- **AC11** `lib.rs:73-80` 的 `pub use config::{...}` 合约不变。
- **AC12** `CLAUDE.md` / `AGENTS.md` "Key Files" / "Code Locations" 表格同步更新。
- **AC13** `operation_guard.rs:147` 的注释引用 `loop_runner::inject_hat_execution_env` 仍可通过 `loop_runner::execution::inject_hat_execution_env` 或 `mod.rs` 重新导出访问。
- **AC14** `#[serde(untagged)] HatBackend` 4 个变体测试通过。
- **AC15** 3 处 `#[serde(flatten)] extra` 字段测试通过。
- **AC16** 4 处 `#[cfg(test)]` 内部 hook 仍能在生产代码中被测试模式调用。

---

## Out of Plan（明确不做）

- 不修改任何业务逻辑。
- 不修改任何函数体、trait impl、derive、serde 属性。
- 不修改 `Default` / `Display` / `From` / `TryFrom` 实现。
- 不修改错误消息或 panic 信息。
- 不修改 `lib.rs` / `Cargo.toml` 的 `pub use` 列表。
- 不修改 `.config/nextest.toml`（除非必要，预期不需要）。
- 不引入新依赖（如 `serial_test`）。
- 不优化性能、不替换 unsafe、不抽象宏。
- 不抽取 `run_loop_impl`（2 933 行）内部章节——这是 follow-up。
- 不拆分 `event_loop/mod.rs`（4 694 行）——这是 follow-up。
- 不抽取 `run_command` 与 `resume_command` 的公共块——这是 follow-up。
