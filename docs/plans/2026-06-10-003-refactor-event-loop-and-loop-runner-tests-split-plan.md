---
title: 拆分 event_loop/mod.rs 与 loop_runner/tests.rs（零回归分模块）
type: refactor
status: stalled-after-U1
date: 2026-06-10
baseline_refreshed: 2026-06-15
baseline_head: eb5a49a
baseline_head_v1: 37bd281
baseline_head_v2: 918192a
baseline_head_v3: 9799bf9
baseline_head_v4: dbe6f35
baseline_head_v5: 40b856c
baseline_head_v6: ab44494
baseline_head_v7: eb5a49a
completion:
  - U1: scaffold 仅在 `ralph/2026-06-10-003-...-merry-wren` 分支 commit `b11d9f0` 落地，**未合并**到 pittcat-dev / main
  - U2-U7: 未开工
landed_in_HEAD:
  - event_loop/mod.rs 仍为单文件 (7 536 行)
  - loop_runner/tests.rs 仍为单文件 (11 800 行 / 203 测试)
  - audit-file-sizes.sh 仅 wc event_loop/tests/* (未含 event_loop/ 根子文件)
---

> ⚠️ **2026-06-15 状态确认**：
>
> 1. **U1 scaffold 未进 HEAD**：`git merge-base --is-ancestor b11d9f0 HEAD` → false；
>    commit `b11d9f0` 只活在分支 `ralph/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-merry-wren`，
>    没有 rebase / merge 到 `pittcat-dev`。早期文档（baseline v3 之前）写的
>    "U1 scaffold 已在分支 `lucky-reed` 落地" 也是历史分支命名，**当前实际分支是 `merry-wren`**。
> 2. **U2-U7 全部未启动**：当前 HEAD `eb5a49a` 上
>    `crates/ralph-core/src/event_loop/` 只有 `loop_state.rs / mod.rs / rejection.rs / review_step_state.rs / tests/`，
>    没有任何新的 placeholder 子文件（`types.rs / workflow_guard.rs / policy.rs / ...` 全部不存在）。
>    `crates/ralph-cli/src/loop_runner/tests.rs` 仍是单文件 11 800 行。
> 3. **baseline 已漂到 v7（HEAD = `eb5a49a`）**：v4 → v7 期间累计 +5 个关键变更（v5 R1/R3/R4/R5 + circuit breaker / v6 plan 001 schema-aware / v7 plan-gate dual-publish 修复），
>    本计划的 enum / struct / 行号锚点全部需要重新校准（详见下面 v5 / v6 / v7 baseline refresh 段）。

## v5 Baseline 实测数据（2026-06-15，HEAD = `40b856c`）

> 本段是 2026-06-15 重新校准的事实数据，**取代**下面 Summary / Problem Frame / Requirements 段中所有 v0-v4 baseline 数字。
> 凡正文与本段冲突的数字（行号 / 测试数 / 字段数 / 变体数 / 行号区间）一律以本段为准；正文未更新部分保留作为 v4 历史快照供 diff 参考。

| 指标 | v3 (9799bf9) | v4 (dbe6f35) | **v5 (40b856c)** | v4→v5 增量 |
|---|---|---|---|---|
| `event_loop/mod.rs` 总行数 | 6 723 | 7 171 | **7 496** | +325 |
| `loop_runner/tests.rs` 总行数 | 11 606 | 11 796 | **11 796** | 0（未变） |
| `loop_runner/tests.rs` `#[test]` 数 | 201 | 203 | **203** | 0（v5 表格原标 204 实测为 203，校正） |
| `EventLoop` struct 字段数 | 14 | 14 | **15** | +1（新增 `ephemeral_isolation`） |
| `TerminationReason` 变体数 | 16 | 16 | **17** | +1（新增 `ScopeViolationCircuitBreakerTripped`） |
| `impl EventLoop` 方法数 | 118 | 120 | **129** | +9（新增 R1/R3/R4/R5 路径方法 + circuit breaker） |
| `event_loop/tests/` 子文件数 | 41 | 44 | **49** | +5（新增 `ephemeral_isolation_integration` / `r5_hard_gate_routing` / `wave_context_env_var` / `wave_context_injection` / `wave_isolated_scope`，可能略有出入） |
| `process_parse_result` 起始行 | ~3 304 | 4 921 | **5 184** | +263（被前置代码推移） |
| `process_parse_result` 结束行 | ~4 921 | 6 780 | **~7 448**（impl 闭合，下一 fn `format_duration` 在 7 450） | v5 表格原标 ~7 102 系估值误差，已校正 |
| `process_parse_result` 行数 | ~1 617 | ~1 860 | **~2 264** | v5 表格原标 ~1 918 系估值误差，已校正（method 相对位置不变，绝对下移 +263 = v4→v5 总行数差） |
| `impl EventLoop` 起 / 止 | ?  | 962 / 7 114 | **1 019 / 7 436** | +57 / +322 |
| 自由函数行号锚点 | — | 324 / 407 / 587 / 893 | `extract_correlation_key` 390 / `apply_workflow_guard_validation` 473 / `apply_event_policy_validation` 652 / `finding_to_payload_contract_violation` 950 | 全部 +60~80 |
| **`publish_policy_rejection_resume`** | 未存在 | 未存在 | **344**（新增自由函数） | 新增 1 个 |
| `lib.rs` config / event_loop re-export 行号 | 80 / 104 | 80 / 104 | **80 / 104** | 0（不变） |
| `loop_runner/` `.rs` 总数 | 29 | 29 | **29** | 0（不变） |

**v4 → v5 关键变更（2 个 commit）**：

- `5929fcd feat(ralph-core): 接入 R1/R3/R4/R5 机制到 event loop` — +R1 wave context、+R3 ephemeral isolation、+R4 enforce_current_unit、+R5 hard-gate 路由稳定性。新增字段 `ephemeral_isolation`、新增方法（wave context build / ephemeral isolation run / 多个 publish_policy_rejection_resume 路由点）。
- `6b03b92 fix(ce-executor-isolated): U1 docs + U2 circuit breaker for isolated scope violations` — 新增 `TerminationReason::ScopeViolationCircuitBreakerTripped` 变体 + 配套 circuit breaker 路径。

**对原计划的核心影响**：

1. **R2 字节级锁定的 enum/struct 已变**：`TerminationReason` 16→17 变体（新增 `ScopeViolationCircuitBreakerTripped`，体积大、含多个字段），3 处 `match` 表达式（`exit_code` / `as_str` / `is_success`）行号 + 变体覆盖列表全部变化。`EventLoop` 14→15 字段，顺序末尾追加 `ephemeral_isolation`。
2. **R7 行数审计脚本未升级**：`scripts/audit-file-sizes.sh` 仍只 wc `event_loop/tests/*.rs` 不含 `event_loop/*.rs`（U1 计划要做的覆盖范围扩展未落地）。
3. **U3 / U4 行号区间需重新切片**：types 段从 v4 的 59-323 推到 v5 的 ~131-389（粗估）；4 个自由函数行号全部 +60~80。U3 / U4 实施前**必须** `grep -nE "^pub (enum|struct)|^fn (extract|apply|finding)" crates/ralph-core/src/event_loop/mod.rs` 重新对齐。
4. **U5 `process_parse_result` 行号区间已变**：5184-7102（~1918 行）；KTD12 归属规则不变，但 v5 因 R5 hard-gate 在内增加了 `publish_policy_rejection_resume` 大量调用点（grep 显示 mod.rs 中 ~9 处），需要确认这些调用是否要随 `process_parse_result` 整段进 `prompt.rs`，还是把 `publish_policy_rejection_resume` 独立放 `event_loop/diagnostics.rs`。
5. **U1 scaffold 不能直接 cherry-pick**：commit `b11d9f0` 基于 `4029be3` 创建 placeholder，与 v5 之间相隔 R1/R3/R4/R5 接入 + circuit breaker 的多个 mod.rs 大改，rebase 必然冲突。**推荐**：放弃 cherry-pick，在 pittcat-dev 直接重新做 U1（创建 10 个 placeholder 子文件 + audit-script 扩展 + 顶部 `mod xxx;` 声明），代价 < 1 小时。
6. **`event_loop/tests/` 子文件数已从 v4 的 44 涨到 v5 的 49**（新增 5 个 R1/R3/R4/R5 集成测试文件），R3 列出的"44 个 `event_loop/tests/` 子文件"过时，但本计划不动这个目录的内容，仅需更新数字。

**`loop_runner/tests.rs` 11 796 行 / 203 测试（v5 表格原标 204 系 awk 计数误差，v6 校实测 = 203）vs v4 的 203 测试**：v5 实际测试数与 v4 一致；总行数 11 796 与 v4 一致是因为 v5 期间测试增加被其他重构抵消（v5 表格中 v4→v5 +0 / +1 实际是 +0 / +0）。

## Summary

把项目里两个最大的源文件按职责拆成多个子模块，全程零回归：

- `crates/ralph-core/src/event_loop/mod.rs`（**7 171 行**（baseline @ commit dbe6f35；2026-06-10 立项时为 5 733 行、2026-06-12 v1 baseline @ 37bd281 时为 6 361 行、2026-06-12 v2 baseline @ 918192a 时为 6 501 行、2026-06-13 v3 baseline @ 9799bf9 时为 6 723 行；v4 推进 +448 行来自 wave attribution 修复 `e695b6c` + CLI emit policy gate `7225aab`），主要是 ~6 153 行的 `impl EventLoop` 块（mod.rs:962-7114））拆为 **10 个新建子文件**（`types` / `workflow_guard` / `policy` / `lifecycle` / `termination` / `dispatch` / `prompt` / `diagnostics` / `process` / `wave`；`payload_contract` ~70 行并入 `policy.rs`；保留**已存在**的 `loop_state` / `rejection` / `review_step_state` / `tests` **四个** mod 声明；**U1 scaffold 已在分支 `ralph/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-lucky-reed` 落地为 commit `464b4d6`，10 个 placeholder 子文件已就位**；早期 plan 曾考虑把 `process` 作为 `prompt.rs` 内的嵌套 mod，但 U1 scaffold 已创建独立 `process.rs`，U6 将在此文件内填充 6 个 `validate_*` 自由函数）。
- `crates/ralph-cli/src/loop_runner/tests.rs`（**11 796 行、203 个测试** + 大量 helper）拆为 **17 个 `.rs`**：1 个 `tests/mod.rs`（保留 mutex 文档 + tests.rs 内**仅 2 个** `FAKE_PATH_BACKEND_*` Mutex；`MOCK_ACP_*` 已迁到 `crates/ralph-cli/src/loop_runner/wave/acp_mock.rs:97-104` 为 `pub static`，本轮**不动**）+ 1 个 `tests/common.rs`（真正跨子文件共享的 helper）+ 15 个主题子文件（user_interactive + pty 合并、output_processing + state_machine 合并等，**0 个子文件 < 200 行**）。

采用 **U1→U7 分阶段**执行：U1 建公共基础设施、U2 拆 tests 的 5 个子文件、U3 锁 types、U4 拆自由函数子模块、U5 拆 `impl EventLoop` 块（**包含 `process_parse_result` 归 `prompt.rs`**）、U6 拆 wave 辅助（**唯一**允许字节级改写 6 个 inline validation 层为自由函数，由 characterization test 锁定行为）、U7 验证 + 文档同步。
每 U 独立 commit、独立跑全套 nextest 套件作为回滚点；**禁止**改公开 API、行为、日志、错误信息、依赖、nextest 配置、process-global Mutex 形式。

## Problem Frame

项目目前两个文件显著超出 5 000 行，阻碍可读性、code review 局部性、新人 onboarding。
`event_loop/mod.rs` 的 ~6 153 行 `impl EventLoop` 块（mod.rs:962-7114，v4 baseline @ dbe6f35）尤其难维护——单文件改动 PR 难以快速 review、容易误碰无关逻辑、且子方法之间的领域边界（lifecycle / policy / dispatch / diagnostics）已经清晰可见。
`loop_runner/tests.rs` 把 **203** 个跨多个领域的测试挤在一个文件里，无法按领域局部 review；新加测试时需要 1 万行单文件编辑。

项目在 `docs/achieved/plan/2026-06-03-002-refactor-split-large-files-plan.md` 已经做过一轮同样模式的拆分（拆 `loop_runner.rs` / `main.rs` / `config.rs`），并明确了可复用的 KTD1-8（多文件 `impl` 块、re-export 兼容、process-global Mutex 不可动、serde 顺序敏感、文档反向验证、U1→U6 风险递增）。**注意**：6-03-002 U1-U6 已**完成**，`crates/ralph-cli/src/loop_runner/` 现在已经是 18 个子模块的目录结构（`event_logging / execution / exit_conditions / hard_gate / hooks/ / late_events / loop_owner / merge_queue / output_parsing / paths / payload_contract_gate / payload_inputs / preset_lint_gate / prompt / runner / start_loop / suspend / wave/`），但 `tests.rs` 与 `event_loop/mod.rs` 仍是单文件，是本轮聚焦的两个**剩余热点**。
本轮是该模式的**第二轮 follow-up**：专门承接 R3 未达标项（`loop_runner/tests.rs` + `event_loop/mod.rs` 主 `impl EventLoop` 块），复用 6-03-002 全部已有模式与禁忌（详见 KTD13 对照表）。

零回归意味着**所有 203 个测试（v4 baseline @ dbe6f35 实测，v3 baseline @ 9799bf9 写 201，v4 +2 来自 wave attribution 修复 `e695b6c` + CLI emit policy gate `7225aab`）+ 44 个 `event_loop/tests/` 子文件（v4 新增 3 个：`guidance_dedup.rs` / `incident_fixture.rs` / `recovery_envelope_u7_u8.rs`，v3 baseline 41）+ 所有使用 `EventLoop` 的下游代码（`runner.rs` / `mod.rs` / `operation_guard.rs` / `adapters/pty_executor.rs`）必须保持编译并通过全部测试**。
任何序列化、状态机、错误信息、日志格式、退出码改动都属于回归（U6 例外条款见 R2.b）。

## Requirements

- R1. `event_loop/mod.rs` 与 `loop_runner/tests/mod.rs` 拆分后**总行数显著下降**（目标：两个主文件都 ≤ 1 000 行；新子文件单文件 ≤ 2 000 行，最大不超过 2 200 行；总新建子文件数 27 个左右：event_loop 10 + tests 17）。
- R2. **零行为变化**：所有现有 `#[test]` 函数签名 / 断言 / 输出 / 错误信息**逐字节不变**；`TerminationReason` **16 变体**顺序 + 3 处 `match` 表达式覆盖顺序（`exit_code` mod.rs:206-223 / `as_str` mod.rs:233-248 / `is_success` mod.rs:254）不变；`EventLoop` **14 字段**（v4 baseline @ dbe6f35 实测，与 v3 一致）顺序不变。
- R2.b（U6 唯一例外）：U6 把 `process_parse_result` 内部 6 个 inline validation 层抽为 `event_loop/process/*` 自由函数**允许字节级改写**，但**行为不变**（由 characterization test 锁定，详见 R-Refactor-10 缓解策略）。
- R3. **零公开 API 破坏**：`lib.rs:80` 的 `pub use config::{...}` 列表（v3 baseline 与 v2 一致，v1 baseline 写 79）、**`lib.rs:104` 的 `pub use event_loop::{...}` 列表**（v3 baseline 与 v2 一致，v1 baseline 写 103）、`event_loop::*` / `loop_runner::*` 全部 `pub` 与 `pub use` 路径**保持不变**；`runner.rs` / `summary_writer.rs` / `drift/engine.rs` / `diagnostics/integration_tests.rs` / `event_loop/tests/replay_light_integration.rs` 等下游文件不修改 import。**注**：v2→v3 期间 commit `a722fe0` 在 `lib.rs` 调整了 1 个 `use` 重排（`PolicyRejection` re-export），与 R3 公开 API 列表本身不冲突——本计划范围**不**涉及该 re-export 的二次调整。
- R4. **测试基础设施保留**（doc-review 校正）：`crates/ralph-cli/src/loop_runner/tests.rs` 头部 **1-40 行的 mutex 文档**（`// ────` 包围的 contributor-facing 块，描述 4 个 process-global Mutex + `cli-serial` test group + `--test-threads=1` 串行运行约束）**整段复制**到拆分后 `crates/ralph-cli/src/loop_runner/tests/mod.rs` 顶部，作为 rustdoc 注释。**修正**：原 plan 写"头部 55 行"——实测 tests.rs:41-55 行不是 mutex 文档（是 use 段末尾 + U5 payload contract hard gate 段注释 + 第一个 `#[test] fn hard_gate_passes_when_no_hats`），"逐字节一致" 的 verification 范围应明确为 **1-40 行**。
- R5. **2 个 process-global Mutex 不变**（修正：原计划写 4 个，但 `MOCK_ACP_EXECUTIONS` / `MOCK_ACP_EXECUTION_SERIAL` 已经在 `crates/ralph-cli/src/loop_runner/wave/acp_mock.rs:97-104` 作为 `pub static` 存在，本轮**不动**；本计划只关注 `tests.rs` 内的 2 个 `FAKE_PATH_BACKEND_*`）：拆分后形式 `static LazyLock<Mutex<...>>`（**实测为 private `static`，非 `pub(crate)`**）逐字节不变；**不**改 `serial_test`、**不**改 `.config/nextest.toml` 的 `cli-serial` test group、不改 Mutex 公共可见性。`wave/acp_mock.rs` 内的 2 个 `pub static MOCK_ACP_*` 同样**禁止修改**（虽然不在 `tests.rs` 内，但 tests 头部文档仍引用它们）。
- R6. **每 U 独立 commit、独立验证**：每个 Implementation Unit 完成后必须 `cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast` 全绿才能进入下一个 U；失败时 `git revert` 即可。
- R7. **拆分后行数审计通过**（adversarial finding #12 校正）：`bash scripts/audit-file-sizes.sh` 通过；新增子文件全部落在 R1 行数阈值内（脚本本身**无阈值断言**，需手动 `awk '$1>2200{print}'` 校验）。**审计脚本覆盖范围校正**（实测 37bd281 baseline）：当前 `scripts/audit-file-sizes.sh` 只 wc `event_loop/tests/*.rs` 不含 `event_loop/*.rs`（根目录子文件），新拆的 10 个 `event_loop/` 根子文件完全不在自动化审计范围。**U1 / U7 必做的两件事**：(a) 在 audit 脚本中追加 `wc -l crates/ralph-core/src/event_loop/*.rs` 段（与 `loop_runner/` / `config/` 段同等待遇）；(b) U7 步骤 10 手动 `awk '$1>2200{print}' crates/ralph-core/src/event_loop/*.rs crates/ralph-cli/src/loop_runner/tests/*.rs` 校验所有新子文件。
- R8. **文档反向验证**：每个 U 完成后用 `git grep -nE "event_loop/mod\.rs:|loop_runner/tests\.rs:|event_loop::[a-z_]+|loop_runner::tests::" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null` 列出所有引用，逐条同步；**每 U 完成后立即追加 "U<n> Drift Sub-Note"**（U7 时合并为完整 Repo Drift Note）。
- R9. **CLAUDE.md ↔ AGENTS.md 同步**：U7 完成时 `diff -u CLAUDE.md AGENTS.md` 必须 0 差异（CLAUDE.md 顶部 "IMPORTANT" 段已有硬约束）。
- R10. **E2E 与 smoke 全绿**：`cargo run -p ralph-e2e -- --mock` 与 `cargo test -p ralph-core scenarios` 通过；`cargo clippy --workspace --all-targets` 0 warning；`cargo fmt --check` 通过；`cargo test --workspace --exclude ralph-e2e --doc` 全绿。

## Key Technical Decisions

- KTD1. **多文件 `impl EventLoop` 块模式**：Rust 允许同一 struct 在多个模块加 `impl`，5 个 `impl EventLoop { ... }` 子块各自落到 `event_loop/{lifecycle, termination, dispatch, prompt, diagnostics}.rs`，**不**在 `mod.rs` 留 forwarder。这样 `&self` / `&mut self` 调用语义保持完整（同 crate 内 inherent impl 跨文件 OK），IDE 跳转直达方法体，下游 `runner.rs` 调用 `ralph_core::EventLoop::check_termination()` 等无需 forwarder。**同 crate 内多文件 inherent impl 零成本、无 orphan rule 冲突**，但需注意：跨文件**不允许同名方法**（即使签名不同）——U5 实施时按 KTD12 边界规则判定。
- KTD2. **re-export 兼容策略**：`event_loop/mod.rs` 顶部继续用 `pub use` 集中重新导出（参考已有 `loop_runner/mod.rs:26-58` 模式）；自由函数与子结构体**默认 `pub(crate)`**，只对外部确实需要的项用 `pub use` 提升；`lib.rs:80` 公开 API 列表（v2 baseline）**禁止修改**；`lib.rs:104` 的 `pub use event_loop::{...}` 列表（v2 baseline）**禁止修改**。
- KTD3. **测试代码随生产代码迁移**：`loop_runner/tests.rs` → `crates/ralph-cli/src/loop_runner/tests/{mod.rs, common.rs, fake_path.rs, wave.rs, hooks.rs, suspend.rs, hard_gate.rs, hard_gate_payload_contract.rs, pty_user_interactive.rs, resolve_loop_id_and_iteration.rs, loop_termination.rs, async_pty.rs, diagnostics.rs, recovery.rs, preset_lint_gate.rs, merge_queue.rs, prompt_handling.rs, event_logging_and_planning_session.rs, late_events_and_hat_selection.rs, event_pipeline.rs}.rs` 共 19 个 .rs（mod 1 + common 1 + 主题 17）；**不**集中到顶层 `tests/`（编译为独立 crate，启动慢）；子文件用 `use super::common::*;` 引用 helper。
- KTD4. **共享 helper 拓扑**（doc-review 校正）：`loop_runner/tests/common.rs` 用 `pub(super)` 限定真正跨子文件共享的 helper。原 plan 把 `install_mock_acp_executions` / `MockAcpExecutionGuard` / `MockAcpExecution` 划为 wave 专属是**错的**——实测在 tests.rs:4848/4901/5519/6449/6466/6608/6809 共 7 处调用，覆盖 wave / hard_gate / async_pty / preset_lint_gate 等多主题，U2 实施时若把它放 `tests/wave.rs` 内的 `pub(super) within wave`，其他子文件会触发"function is private"编译错误。**修正归类**：(a) `install_mock_acp_executions` / `MockAcpExecutionGuard` / `MockAcpExecution` 放 `tests/common.rs`（pub(super)，跨子文件可用）；(b) `acp_test_payload` / `make_worker_event` 等 wave 特定 helper 放 `tests/wave.rs` 模块内；(c) `write_fake_executable` / `FakePathBackendsGuard` / `install_fake_path_backends` 放 `tests/fake_path.rs` 模块内。**真正跨子文件共享的 helper** 还有：`dispatch_test_event_loop*` / `suspend_outcome*` / `build_*_payload_input` / `empty_hook_metadata` / `block_on_test_future` 等统一放 `common.rs`。
- KTD5. **类型/字段顺序敏感（修正伪风险）**：抽 `event_loop/types.rs` 时**整段声明原封不动复制**；`TerminationReason` **16 变体**顺序 + 3 处 `match` 表达式覆盖顺序（`exit_code` 行 206-223 / `as_str` 行 233-248 / `is_success` 行 254）不变；`EventLoop` **14 字段**（v4 baseline @ dbe6f35 实测，含 `pub(crate)` 与辅助字段；v0/v1 baseline 写 13 是早期错算，v2 baseline 写 14 是 v1 校正后稳定，v3 维持 14）顺序不变。**注**：`event_loop/mod.rs` 中**无任何 `#[serde(...)]` / `#[serde(flatten)]` / `#[serde(default)]` / `#[serde(untagged)]` 标注**（grep 验证为空），KTD 早期草稿中的 "serde attribute 位置不变" 是伪风险——已删除。U3 / U4 完成后用 `git diff` 字节级 + `git grep -A 1 "TerminationReason" types.rs | head -50` 验证变体顺序。**baseline 变更说明（v4 校正）**：v0 plan 写 "17 变体 / 14 字段" 是 2026-06-10 立项时的早期错算（v1 时已校正为 16 / 13），v2 baseline 实测 16 / 14——`EventLoop` 在 v0→v2 期间新增 1 个字段（最可能为 `hat_lifecycle_tracker` 或 `recovery_responder`，v1→v2 之间 `recovery_responder` 由 ce-executor-isolated 闭环 commit `f8887ed`/`fe9ae37` 引入）；v3/v4 baseline 字段数维持 14（v2→v4 期间未新增字段，新增逻辑全部落在 `process_parse_result` 与 dispatcher / gate / attribution 边界）。U3 / U5 实施前必须**重新跑** `awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs` 确认输出 14（v4 baseline）。
- KTD6. **U1→U7 风险递增顺序**：
  1. **U1**：建公共基础设施（`event_loop/mod.rs` 顶部加 10 个 `mod xxx;` 声明 + 10 个空 placeholder 子文件 + 顶部 1 个 `pub use` 转发占位 + 全套测试基线建立）—— 仅有 placeholder 文件，无逻辑改动
  2. **U2**：拆 `loop_runner/tests.rs` 为 `tests/{mod.rs, common.rs, fake_path.rs, wave.rs, hooks.rs}.rs`（5 个子文件）—— 仅测试，零公开 API 影响
  3. **U3**：抽 `event_loop/types.rs`（数据结构 + `TerminationReason` 锁定）—— `TerminationReason` **16 变体**顺序风险点
  4. **U4**：拆 `event_loop/{workflow_guard.rs, policy.rs}.rs`（自由函数子模块；`payload_contract.rs` 70 行内容并入 `policy.rs`）—— 编译期可保证
  5. **U5**：拆 `event_loop/{lifecycle.rs, termination.rs, dispatch.rs, prompt.rs, diagnostics.rs}.rs`（`impl EventLoop` 块按方法域分块；**`process_parse_result` 1 860 行单方法归 `prompt.rs`**（v4 baseline mod.rs:4921-6780，v3 baseline 1 617 行 → v4 +243 行，主要来自 wave attribution 修复），因 prompt 域是它主语义归属）—— 主体改动
  6. **U6**：拆 `event_loop/wave.rs`（wave 辅助独立子模块）+ 在 `event_loop/process.rs`（或 `prompt.rs` 内的嵌套 `pub(crate) mod process`）中**新增** 6 个 `event_loop::process::*` 自由函数（仅声明 + 调用，方法体迁移到 U5 的 `prompt.rs::process_parse_result` 调用点处抽 6 个内联块为 `process::validate_*`）
  7. **U7**：完整验证 + 文档反向验证 + `audit-file-sizes.sh` + Repo Drift Note 合并
- KTD7. **process-global Mutex 拓扑与可见性**（修正）：本计划**只动 `tests.rs` 内的 2 个** Mutex（`FAKE_PATH_BACKEND_SERIAL` / `FAKE_PATH_BACKEND_BIN`，均为 private `static`，**非** `pub(crate)`）：拆分后形式 `static LazyLock<Mutex<...>>` 逐字节不变；位置从 `tests.rs:605-609` 迁到 `tests/fake_path.rs` 模块内。`MOCK_ACP_EXECUTIONS` / `MOCK_ACP_EXECUTION_SERIAL` 已在上一轮（commit 不详，本计划立项前）作为 `pub static` 迁到 `crates/ralph-cli/src/loop_runner/wave/acp_mock.rs:97-104`（`MOCK_ACP_EXECUTIONS` 97-100 / `MOCK_ACP_EXECUTION_SERIAL` 102-104，两者都在 `#[cfg(test)]` 守卫下，仅测试构建可见），并由 `wave/mod.rs:10` 通过 `pub use acp_mock::{MOCK_ACP_EXECUTION_SERIAL, MOCK_ACP_EXECUTIONS, MockAcpExecution};` 暴露——**本计划完全不动这两个**，但 tests `mod.rs` 顶部文档需引用它们以保留 contributor 上下文；`cli-serial` test group 保留；不引入 `serial_test` crate。
- KTD8. **nextest 配置不可改**：`.config/nextest.toml` 的 `cli-serial` / `max-threads = 1` 是项目硬约束；`event_loop` 包仍并行运行（无 process-global Mutex），`ralph-cli` 包仍走 `cli-serial` 串行组。
- KTD9. **文档反向验证（每 U 立即 + U7 合并）**（doc-review 校正）：每 U 完成后立即用 `git grep -nE "event_loop/mod\.rs\b|loop_runner/tests\.rs\b|event_loop::[a-z_]+|loop_runner::tests::" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null` 列出本 U 引入的失效引用，并**立即追加** "U<n> Drift Sub-Note" 段（本 plan 文档 `## Repo Drift Note` 之前的子段）；U7 完成时合并为完整 Repo Drift Note。**grep 模式修正**：原 plan 的 `event_loop/mod\.rs:|loop_runner/tests\.rs:` 模式带尾冒号实际命中 0 行（实测 37bd281 baseline），改用 `\b` 词边界符后才能匹配 `event_loop/mod.rs)` / `event_loop/mod.rs,` / 裸 `event_loop/mod.rs` 等所有行号 / 链接 / 注释引用形式。**引用规模估计**（实测 37bd281 baseline）：`event_loop/mod.rs` 252 处引用 / 59 个文件；`loop_runner/tests.rs` 68 处引用 / 20 个文件；U7 文档同步工作量原 plan 估"14+ 引用文档"严重低估（实测 70+ 引用文件，U7 步骤 13 必须用上述 grep 列出**完整**清单后逐条处理，不可凭印象抽样）。
- KTD10. **不切 `process_parse_result` 内部单方法**：U5 把它整体迁到 `event_loop/prompt.rs`（`impl EventLoop` 块内），方法体不切。U6 在 `prompt.rs::process_parse_result` 调用点处把 6 个 inline validation 层（origin guard / topic format / event policy / state machine / workflow guard / execution contract）抽为 `event_loop::process::validate_*` 自由函数（可放在 `event_loop/process.rs` 或 `prompt.rs` 内的嵌套 `pub(crate) mod process`，由 R-Refactor-10 characterization test 锁定行为）。
- KTD11. **不引入新依赖**：拆分不引入任何 `Cargo.toml` 依赖变更；不升级 `cargo-nextest`、不引入 `serial_test`、不动 `tokio` / `serde` 版本。
- KTD12. **5 个 `impl EventLoop` 域的边界规则**（U5 实施时遵循）：
  - **lifecycle.rs**：方法主返回是 `LoopState` / `WorkflowProgress` / 涉及 `activate` / `next` / 状态机转换的，归属 lifecycle
  - **termination.rs**：方法主返回是 `TerminationReason` / 涉及 `check_termination` / `is_terminal` / `mark_terminated` 的，归属 termination
  - **dispatch.rs**：方法主返回是 hat 选择 / 订阅匹配 / 队列派发的，归属 dispatch
  - **prompt.rs**：方法主返回是 `UserPrompt` / `process_parse_result` / 涉及 prompt 构建与解析的，归属 prompt（**`process_parse_result` 1 860 行整体归 prompt**（v4 baseline mod.rs:4921-6780；v3 baseline 1 617 行 / v2 baseline 1 618 行 / v1 baseline 1 564 行 → v4 较 v3 +243 行 / 较 v1 +296 行），因它是 prompt 解析主入口）
  - **diagnostics.rs**：方法主返回是 telemetry / metrics / recovery 信号的，归属 diagnostics
  - **跨域方法**：若方法同时影响 ≥ 2 个域（例如 `activate` 改 lifecycle 也写 diagnostics），**以方法主返回 / 主副作用归属**，并在 PR description 中标注 "跨域"；不可调和的歧义记入 R3 follow-up
- KTD13. **6-03-002 KTD 对照表**（平移 + 调整）：

  | 6-03-002 KTD | 本 plan 对应 KTD | 调整 |
  |---|---|---|
  | KTD1 多文件 `impl` 块 | KTD1 | **新增同 crate 内孤儿规则 / coherence / 同名方法冲突 3 条具体规则** |
  | KTD2 re-export 兼容 | KTD2 | **新增 `lib.rs:104` 公开 re-export 列表覆盖**（v2 baseline，v1 baseline 写 103） |
  | KTD3 测试代码迁移 | KTD3 | **新增** `tests/{mod, common}` 子目录 vs `tests.rs` 同名处理（Rust 自动把 `tests.rs` 视为 `tests/mod.rs`） |
  | KTD4 共享 helper | KTD4 | **调整**为"按主题就近"（wave / fake_path 特定 helper 移到子文件内） |
  | KTD5 serde 顺序 | KTD5 | **修正**：删除 `#[serde(flatten)]` 伪风险，改为 `TerminationReason` **16 变体** + 3 处 match + `EventLoop` **14 字段**顺序（**v4 baseline @ dbe6f35 实测**，v0 立项 17 / 14 中 17 为早期错算，14→13 由 v1 doc-review 触发，**v2 校正回 14**——`hat_lifecycle_tracker` / `recovery_responder` 等 v0→v2 期间增量字段；**v3/v4 维持 14**，v2→v4 期间未新增字段） |
  | KTD6 U1→U6 风险递增 | KTD6 | **扩展**到 U1→U7（tests 拆两批：U2 + U7） |
  | KTD7 4 个 Mutex 不可动 | KTD7 | **大幅修正**：实测只有 2 个 Mutex 在 `tests.rs` 内（`FAKE_PATH_BACKEND_SERIAL` / `FAKE_PATH_BACKEND_BIN`，均 private `static`），`MOCK_ACP_*` 已在 `wave/acp_mock.rs` 为 `pub static` 由上一轮迁出，本计划不动；只需把 `tests.rs` 内 2 个迁到 `tests/fake_path.rs` |
  | KTD8 nextest 配置不可改 | KTD8 | 完全沿用 |
  | KTD9 文档反向验证 | KTD9 | **调整**：从 U7 才追加 → 每 U 追加 Sub-Note，U7 合并 |
  | （无对应） | KTD10 | **新增**：`process_parse_result` 不切单方法内部 |
  | （无对应） | KTD11 | **新增**：不引入新依赖（从 6-03-002 反模式推导） |
  | （无对应） | KTD12 | **新增**：5 个 `impl EventLoop` 域的边界规则（本次新场景需要） |
  | （无对应） | KTD13 | **新增**：6-03-002 KTD 对照表 |

## High-Level Technical Design

### `event_loop/mod.rs` 拆分后的最终结构

```mermaid
flowchart TB
  subgraph event_loop["event_loop/"]
    mod_rs["mod.rs<br/>≈ 80 行<br/>模块声明 + re-exports"]
    types["types.rs<br/>≈ 270 行<br/>数据结构 + TerminationReason (16 变体)<br/>🆕 新建"]
    workflow["workflow_guard.rs<br/>≈ 260 行<br/>apply_workflow_guard_validation<br/>🆕 新建"]
    policy["policy.rs<br/>≈ 600 行<br/>apply_event_policy_validation<br/>+ finding_to_payload_contract_violation<br/>🆕 新建"]
    lifecycle["lifecycle.rs<br/>≈ 1 360 行<br/>impl EventLoop 生命周期<br/>🆕 新建"]
    termination["termination.rs<br/>≈ 960 行<br/>impl EventLoop 终止条件<br/>🆕 新建"]
    dispatch["dispatch.rs<br/>≈ 700 行<br/>impl EventLoop 调度<br/>🆕 新建"]
    prompt["prompt.rs<br/>≈ 3 520 行<br/>impl EventLoop prompt 处理<br/>+ process_parse_result 1 860 行<br/>🆕 新建"]
    diagnostics["diagnostics.rs<br/>≈ 420 行<br/>impl EventLoop 诊断 + review_aggregate_timeouts<br/>🆕 新建"]
    wave["wave.rs<br/>≈ 160 行<br/>wave 辅助 + 6 个 validate_*<br/>🆕 新建"]
    loop_state["loop_state.rs<br/>= 已存在 (845 行), 不动"]
    rejection["rejection.rs<br/>= 已存在 (593 行), 不动"]
    review_step["review_step_state.rs<br/>= 已存在 (605 行, commit 37bd281), 不动"]
    tests["tests/<br/>= 已存在 (44 个子文件), 不动"]
  end
  mod_rs --> types
  mod_rs --> workflow
  mod_rs --> policy
  mod_rs --> lifecycle
  mod_rs --> termination
  mod_rs --> dispatch
  mod_rs --> prompt
  mod_rs --> diagnostics
  mod_rs --> wave
  mod_rs --> loop_state
  mod_rs --> rejection
  mod_rs --> review_step
  mod_rs --> tests
  prompt -.抽 6 个 free fn.-> wave
```

图例：🆕 新建 10 个独立子文件（含 `process.rs`，U6 将填充 6 个 `validate_*` 自由函数）；= 已存在 4 个（含 2026-06-12 新加入清单的 `review_step_state.rs`）。`payload_contract.rs` 内容并入 `policy.rs`。
**预估行数依据（v4 baseline @ dbe6f35）**：mod.rs 当前 7 171 行 - types 270 - workflow_guard 260 - policy 600 = ~6 041 行属于 `impl EventLoop` 块（实测 6 153 行），按 lifecycle 1 360 + termination 960 + dispatch 700 + prompt 3 520 + diagnostics 420 = 6 960 行（含 use 段重复 + 跨域方法的小量复制冗余 ~1 007 行，符合实际多文件 impl 拆分的开销；v1 baseline 行数估 6 430 行，v2 +120，v3 +30，v4 +320）。

### `loop_runner/tests.rs` 拆分后的最终结构

```mermaid
flowchart TB
  subgraph tests_dir["loop_runner/tests/"]
    mod_rs["mod.rs<br/>≈ 200 行<br/>顶部 mutex 文档 + 子模块 mod 声明<br/>🆕 新建"]
    common["common.rs<br/>≈ 250 行<br/>真正跨子文件共享 helper<br/>🆕 新建"]
    fake_path["fake_path.rs<br/>≈ 220 行<br/>write_fake_executable +<br/>FakePathBackendsGuard +<br/>FAKE_PATH_BACKEND_* Mutex (2 个)<br/>🆕 新建"]
    wave["wave.rs<br/>≈ 2 200 行<br/>wave 相关测试<br/>(MOCK_ACP_* 引用 wave/acp_mock.rs,<br/>不持有 Mutex)<br/>🆕 新建"]
    hooks["hooks.rs<br/>≈ 1 800 行<br/>dispatch_phase_event_hooks 测试族<br/>🆕 新建"]
    suspend["suspend.rs<br/>≈ 380 行<br/>🆕 U7"]
    hard_gate["hard_gate.rs<br/>≈ 750 行<br/>🆕 U7"]
    hard_gate_payload["hard_gate_payload_contract.rs<br/>≈ 900 行<br/>U5 硬门 + U6 payload 报告<br/>🆕 U7"]
    pty_user["pty_user_interactive.rs<br/>≈ 280 行<br/>pty + user_interactive 合并<br/>🆕 U7"]
    resolve_iter["resolve_loop_id_and_iteration.rs<br/>≈ 550 行<br/>resolve_loop_id + iteration 合并<br/>🆕 U7"]
    loop_termination["loop_termination.rs<br/>≈ 280 行<br/>🆕 U7"]
    async_pty["async_pty.rs<br/>≈ 500 行<br/>🆕 U7"]
    diagnostics["diagnostics.rs<br/>≈ 500 行<br/>🆕 U7"]
    recovery["recovery.rs<br/>≈ 600 行<br/>recovery 测试族<br/>🆕 U7"]
    preset_lint["preset_lint_gate.rs<br/>≈ 600 行<br/>preset_lint_gate 测试族<br/>🆕 U7"]
    merge_queue["merge_queue.rs<br/>≈ 400 行<br/>🆕 U7"]
    prompt_handling["prompt_handling.rs<br/>≈ 280 行<br/>🆕 U7"]
    event_log_ps["event_logging_and_planning_session.rs<br/>≈ 550 行<br/>🆕 U7"]
    late_hat["late_events_and_hat_selection.rs<br/>≈ 550 行<br/>🆕 U7"]
    event_pipeline["event_pipeline.rs<br/>≈ 350 行<br/>output_processing + state_machine +<br/>inject_hat_execution_env 合并<br/>🆕 U7"]
  end
  mod_rs --> common
  mod_rs --> fake_path
  mod_rs --> wave
  mod_rs --> hooks
  mod_rs --> suspend
  mod_rs --> hard_gate
  mod_rs --> hard_gate_payload
  mod_rs --> pty_user
  mod_rs --> resolve_iter
  mod_rs --> loop_termination
  mod_rs --> async_pty
  mod_rs --> diagnostics
  mod_rs --> recovery
  mod_rs --> preset_lint
  mod_rs --> merge_queue
  mod_rs --> prompt_handling
  mod_rs --> event_log_ps
  mod_rs --> late_hat
  mod_rs --> event_pipeline
```

共 19 个 `.rs`：1 mod + 1 common + 17 主题（**0 个 < 200 行**）。
**预估行数依据（v4 baseline @ dbe6f35）**：tests.rs 当前 11 796 行 / 203 个测试（v3 baseline 11 606 行 / 201 个测试，v2 baseline 11 015 行 / 192 个测试，v1 baseline 10 993 行 / 193 个测试；v3→v4 +190 行 / +2 测试，v2→v3 +591 行 / +9 测试，v1→v2 +22 行 / -1 测试，v3→v4 主要来自 wave attribution 修复 `e695b6c` + CLI emit policy gate `7225aab`），按主题聚类的平均规模 50-60 行/测试 + helper（fake_path 占 220、wave 占 2 350、hooks 占 1 800、其余按行数分布）。

### 拆分顺序（U1→U7）流水图

```mermaid
flowchart LR
  U1[U1 公共基础设施<br/>10 个 placeholder] --> U2[U2 拆 tests 5 个]
  U2 --> U3[U3 抽 types.rs]
  U3 --> U4[U4 拆 workflow/policy]
  U4 --> U5[U5 拆 impl EventLoop 分块<br/>+ process_parse_result 归 prompt]
  U5 --> U6[U6 拆 wave.rs<br/>+ 抽 6 个 validate_* 自由函数]
  U6 --> U7[U7 补全剩余 14 个<br/>tests 子文件 + 文档同步]
  U7 -.失败可逆.-> Revert[git revert 单 U commit]
```

## Implementation Units

> 📌 **2026-06-15 接力指引**（在 v5 baseline 之上重启时必读）：
>
> - **U1 必须重做**：`b11d9f0` 不能直接 cherry-pick（与 R1/R3/R4/R5 接入 commit `5929fcd`/`6b03b92` 冲突）。
>   直接在 pittcat-dev 上重新 scaffold 10 个 placeholder + audit-script 扩展 + `mod xxx;` 声明即可（< 1 小时）。
> - **U3 重做前**：用 `awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs`
>   重新校准字段数（v5 = **15**），并把 `TerminationReason` v5 = **17** 变体的新 variant `ScopeViolationCircuitBreakerTripped` 加入字节级锁定清单。
> - **U4 重做前**：`grep -nE "^fn (extract_correlation_key|apply_workflow_guard|apply_event_policy|finding_to_payload_contract|publish_policy_rejection_resume)"`
>   重新对齐 v5 行号（粗值：390 / 473 / 652 / 950 + **新增** `publish_policy_rejection_resume` 在 344）。
>   **新决策点**：`publish_policy_rejection_resume` 应归 `policy.rs` 还是 `diagnostics.rs`？根据 KTD12 主副作用规则
>   （写 `task.resume` 到 bus + 路由到源 hat），建议归 `policy.rs`（与 `apply_event_policy_validation` 同域）。
> - **U5 重做前**：`process_parse_result` v5 行号区间 = **5184-7102（~1918 行）**；v5 因 R5 在内多了 ~9 处
>   `publish_policy_rejection_resume(...)` 调用点，确认这些调用是否随方法整段进 `prompt.rs`（建议：随，符合 KTD10）。
> - **U6 仍保留**：`run_ephemeral_isolation` / `inject_review_aggregate_timeouts` 等 R1/R3 方法在 v5 新增，
>   按 KTD12 应归 **diagnostics**（telemetry + 副作用是写 `.ralph/agent/scratchpad-{loop_id}.md` + 注入 `## EPHEMERAL RELOCATED` prompt 块，
>   主副作用属于 diagnostics + prompt 跨域，按 KTD12 主副作用归 diagnostics）。
>
> 上面 5 条只是 v4 → v5 baseline 漂移引起的局部修订，**KTD1-KTD13 / R1-R10 全部主体仍然有效**，本次重启不需要重新设计架构。

## Implementation Units（细则）

### U1. 公共基础设施：建立拆分脚手架 + 全套测试基线

- **状态**：**已在分支 `ralph/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-lucky-reed` 落地为 commit `464b4d6`**，但尚未 rebase 到 v4 baseline `dbe6f35`。进入 U2 前必须先把该分支 rebase 到 `dbe6f35`，并处理 `process.rs` placeholder 文件（见下方 Files 第 3 项）。
- **Goal**: 在 `crates/ralph-core/src/event_loop/mod.rs` 顶部（在**已存在**的 `mod loop_state; pub mod review_step_state; pub mod rejection; #[cfg(test)] mod tests;` 声明**之后**）预声明 10 个目标子模块（`mod types;` / `mod workflow_guard;` / `mod policy;` / `mod lifecycle;` / `mod termination;` / `mod dispatch;` / `mod prompt;` / `mod diagnostics;` / `mod process;` / `mod wave;`），每个子模块暂为空 placeholder；记录**全套测试基线快照**为后续 U 提供零回归锚点。
- **Requirements**: R6 / R8（基线建立）
- **Dependencies**: 无（最优先）
- **Files**:
  - 创建 `crates/ralph-core/src/event_loop/{types,workflow_guard,policy,lifecycle,termination,dispatch,prompt,diagnostics,process,wave}.rs`（10 个 placeholder，每个文件 5-10 行 `// placeholder, will be populated in U<N>`）
  - **注意**：U1 scaffold commit `464b4d6` 把 `process.rs` 也创建为独立 placeholder 文件（与早期 plan "不新增 `process.rs`" 略有调整），文件内注释说明 U6 将填充 6 个 `validate_*` 自由函数。U6 实施时可选择：(a) 在 `process.rs` 内实现；或 (b) 把内容迁到 `prompt.rs` 内的嵌套 `pub(crate) mod process`。无论哪种，必须保证 `mod.rs` 的 `pub use process::*;` 路径不变。
  - 修改 `crates/ralph-core/src/event_loop/mod.rs` 顶部 5-19 行的 `mod xxx;` / `pub use` 段（在已存在的 `mod loop_state; pub mod review_step_state; pub mod rejection; mod tests;` **之后**加 10 个 `mod xxx;` 声明 + 10 个 `pub use xxx::*;` 转发占位）
  - 临时文件 `/tmp/event-loop-split-baseline.txt`（不提交）记录全套测试基线输出
- **Approach**: 仅添加 `mod xxx;` 声明和 `pub use` re-export 转发点，不删除 `mod.rs` 任何现有代码；**不动**已存在的 4 个 mod 声明（`loop_state` / `review_step_state` / `rejection` / `tests`）。
- **Patterns to follow**:
  - `crates/ralph-cli/src/loop_runner/mod.rs:7-24` 的 `mod xxx;` 声明顺序
  - `crates/ralph-cli/src/loop_runner/mod.rs:26-58` 的 `pub use` 重新导出模式
  - 2026-06-03-002 plan 的 U1（commit `4ba6e37`）建 `crates/ralph-core/src/event_loop/tests/common/mod.rs` 的"复制不删"模式
- **Test scenarios**:
  - **Happy path**: `cargo build -p ralph-core` 通过；`cargo build -p ralph-cli` 通过
  - **Regression baseline**: `cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast` 全绿；记录 passed/failed 数字快照到 `/tmp/event-loop-split-baseline.txt`
  - **No new failures**: 与拆分前基线对比，新文件不能引入任何编译错误或测试失败
  - **Existing mods untouched**: `git diff` 显示已存在的 4 行 mod 声明（`mod loop_state;` / `pub mod review_step_state;` / `pub mod rejection;` / `#[cfg(test)] mod tests;`）字节级未动
- **Verification**:
  - `cargo build -p ralph-core` 与 `cargo build -p ralph-cli` 0 error / 0 warning
  - `cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast` 与基线数字完全一致
  - `git diff --stat` 仅显示 10 个新空文件 + `mod.rs` 顶部 mod 声明变化（< 50 行变更）
  - 临时基线文件 `/tmp/event-loop-split-baseline.txt` 已生成

### U2. 拆 `loop_runner/tests.rs` 为 `tests/{mod.rs, common.rs, fake_path.rs, wave.rs, hooks.rs}.rs`（5 个子文件）

- **Goal**: 把 `tests.rs` 拆为 `crates/ralph-cli/src/loop_runner/tests/` 子目录，包含 `mod.rs`（顶部 mutex 文档 + 子模块 mod 声明）+ `common.rs`（真正跨子文件共享 helper）+ `fake_path.rs`（FAKE_PATH 后端 + `FAKE_PATH_BACKEND_*` Mutex，**仅 2 个**）+ `wave.rs`（wave 测试；**通过 `use super::*` 或 `use crate::loop_runner::wave::{...}` 引用 `wave/acp_mock.rs` 的 `pub static MOCK_ACP_*`，不在 wave.rs 持有 Mutex 声明**）+ `hooks.rs`（dispatch_phase_event_hooks 测试族）。其余 14 个测试子文件留到 U7 实施。
- **Requirements**: R1 / R2 / R3 / R4 / R5 / R6
- **Dependencies**: U1（子模块声明已就绪）
- **Files**:
  - 创建 `crates/ralph-cli/src/loop_runner/tests/mod.rs`（~200 行：mutex 文档 + `mod common; mod fake_path; mod wave; mod hooks;` 声明；**mod.rs 内不持有任何 `static LazyLock<Mutex<...>>` 声明**——所有 Mutex 都在子文件或 `wave/acp_mock.rs`）
  - 创建 `crates/ralph-cli/src/loop_runner/tests/common.rs`（~250 行：`pub(super) fn` 真正跨子文件共享的 helper）
  - 创建 `crates/ralph-cli/src/loop_runner/tests/fake_path.rs`（~220 行：`write_fake_executable` / `FakePathBackendsGuard` / `install_fake_path_backends` + **2 个** `FAKE_PATH_BACKEND_SERIAL` / `FAKE_PATH_BACKEND_BIN` private `static` Mutex，从原 `tests.rs:605-609` 迁过来）
  - 创建 `crates/ralph-cli/src/loop_runner/tests/wave.rs`（~2200 行：所有 `wave_*` / `acp_*` / `MockAcpExecution` / `forced_test_wave_pty_failure` 测试 + wave 特定 helper（`MockAcpExecution` 构造器 / `acp_test_payload`）；**通过 `use crate::loop_runner::wave::{MOCK_ACP_EXECUTIONS, MOCK_ACP_EXECUTION_SERIAL, MockAcpExecution};` 引用 `wave/acp_mock.rs` 的 `pub static`，不重新声明**）
  - 创建 `crates/ralph-cli/src/loop_runner/tests/hooks.rs`（~1800 行：所有 `dispatch_phase_event_hooks` / `loop_start_dispatch` / `iteration_start_dispatch` / `plan_created_lifecycle_hooks` / `human_interact_lifecycle_hooks` / `loop_termination_lifecycle_hooks` / `iteration_start_suspend` / `dispatch_phase_event_hooks_retry_backoff` 测试）
  - 删除 `crates/ralph-cli/src/loop_runner/tests.rs`（改为目录 `tests/`）
- **Approach**:
  1. `mkdir crates/ralph-cli/src/loop_runner/tests/`
  2. 把 `tests.rs` 头部 1-55 行的 mutex 文档**完整复制**到 `tests/mod.rs` 顶部
  3. `FAKE_PATH_BACKEND_SERIAL` / `FAKE_PATH_BACKEND_BIN`（仅这 2 个）从 `tests.rs:605-609` 整段迁到 `tests/fake_path.rs` 顶层
  4. `MOCK_ACP_EXECUTIONS` / `MOCK_ACP_EXECUTION_SERIAL` 在 `wave/acp_mock.rs:97-104` 中**保持不动**；wave.rs 子文件只用 `use crate::loop_runner::wave::{...}` 引用
  5. wave 特定 helper（`MockAcpExecution` 构造器 / `acp_test_payload` 等）放到 `tests/wave.rs` 模块内
  6. fake_path 特定 helper 放到 `tests/fake_path.rs` 模块内
  7. 真正跨子文件共享的 helper 放到 `tests/common.rs`
  8. fake-path / wave / hooks 三个主题测试族分别迁出
  9. 剩余 14 个主题（suspend / hard_gate / hard_gate_payload / pty_user / resolve_iter / loop_termination / async_pty / diagnostics / recovery / preset_lint / merge_queue / prompt_handling / event_log_ps / late_hat / event_pipeline）暂时保留在 `tests/mod.rs` 末尾，U7 实施
  10. 删除 `crates/ralph-cli/src/loop_runner/tests.rs`
  11. **U2 step 11**（doc-review 派生）：更新 `.config/nextest.toml:2` 注释——把原文"touch four process-global Mutexes (MOCK_ACP_EXECUTIONS, MOCK_ACP_EXECUTION_SERIAL, FAKE_PATH_BACKEND_SERIAL, FAKE_PATH_BACKEND_BIN)" 改写为分账说明："touch four process-global Mutexes: FAKE_PATH_BACKEND_SERIAL/FAKE_PATH_BACKEND_BIN (in tests/fake_path.rs) + MOCK_ACP_EXECUTIONS/MOCK_ACP_EXECUTION_SERIAL (in wave/acp_mock.rs) — 2 in-scope + 2 adjacent, all `#[cfg(test)]` guarded"
  12. **U2 step 12**（adversarial finding #15 实际落地为 step 11，#13 引用工作量为 U7 单独负责——已记入 U7 步骤 13）
- **Patterns to follow**:
  - `loop_runner/tests/mod.rs` 顶部 mutex 文档的 `--test-threads=1` 模式
  - `crates/ralph-core/src/event_loop/tests/` 已有的 44 个子文件 + `common/mod.rs` 模式（参考 2026-06-03-002 U1）
  - `pub(super)` 限定 + 子文件 `use super::common::*;` 模式（KTD3 / KTD4）
  - KTD7 Mutex 拓扑修正（只动 tests.rs 内 2 个 fake_path Mutex；wave Mutex 已在 `wave/acp_mock.rs` 由上一轮处理）
- **Test scenarios**:
  - **Happy path**: `cargo build -p ralph-cli` 通过
  - **Regression baseline**: `cargo nextest run -p ralph-cli -E 'test(loop_runner::)' --no-fail-fast` 与基线数字完全一致
  - **Mutex preservation (tests.rs side)**: 2 个 `FAKE_PATH_BACKEND_*` Mutex 形式 `static LazyLock<Mutex<...>>` 逐字节不变（位置从 `tests.rs:605-609` 改为 `tests/fake_path.rs` 顶层）
  - **Mutex preservation (wave/acp_mock.rs side)**: `git diff -- crates/ralph-cli/src/loop_runner/wave/acp_mock.rs` 显示 0 行变更
  - **No new test failures**: 14 个暂留主题测试族全部通过
  - **Test documentation preserved**: `tests/mod.rs` 顶部 1-40 行的 mutex 文档**整段**与原 `crates/ralph-cli/src/loop_runner/tests.rs:1-40` 一致（修正：原 plan 写 1-55 行包含 `U5: payload contract hard gate` 段标记 + 第一个 test 函数，verification 命令无法稳定 0 差异）
- **Verification**:
  - `cargo nextest run -p ralph-cli -E 'test(loop_runner::)' --no-fail-fast` 全绿，passed 数字与基线完全一致
  - `git diff` 显示 `tests.rs` 删除 + 5 个新文件 + 14 个暂留测试仍在 `mod.rs` 末尾
  - 2 个 fake_path Mutex 形式不变（`grep -A3 "static FAKE_PATH_BACKEND" crates/ralph-cli/src/loop_runner/tests/fake_path.rs`）
  - `git diff --stat -- crates/ralph-cli/src/loop_runner/wave/acp_mock.rs` 显示 0 行变更

### U3. 抽 `event_loop/types.rs`（数据结构 + TerminationReason 锁定）

- **Goal**: 把 `crates/ralph-core/src/event_loop/mod.rs:59-323`（v4 baseline @ dbe6f35，实测：`TerminationReason` 枚举在 131 行 / 枚举段 131-194 / `impl TerminationReason` 196 行起 / `EventLoop` struct 266 行起 / `extract_correlation_key` 起点 324；v3 baseline 59-192 / enum 129-192 / impl 194 起 / extract_correlation_key 322）抽到 `crates/ralph-core/src/event_loop/types.rs`。U5 实施前必须**重新跑** `grep -nE "^pub (enum|struct) (ProcessedEvents|ProcessedEventsWithWaves|TerminationReason|WorkflowGuardRejection)" crates/ralph-core/src/event_loop/mod.rs` 确认行号未漂移。
- **Requirements**: R1 / R2 / R3 / R5
- **Dependencies**: U1；U2（分离关注点）
- **Files**:
  - 修改 `crates/ralph-core/src/event_loop/types.rs`（从 placeholder 改为实质内容，~270 行）
  - 修改 `crates/ralph-core/src/event_loop/mod.rs:59-323` 段（删除 + 顶部 `pub use types::{ProcessedEvents, ProcessedEventsWithWaves, TerminationReason};`；v4 baseline types 段 59-323、enum 131-194、impl 196 起、EventLoop 266 起、extract_correlation_key 324 起点，比 v3 baseline types 段末 192 +131 行——主要来自 attribution / recovery 新增数据结构）
- **Approach**:
  1. 整段（动手前重新 grep 定位）原封不动复制到 `types.rs`
  2. `mod.rs` 顶部加 `pub use types::{ProcessedEvents, ProcessedEventsWithWaves, TerminationReason};`
  3. `mod.rs` 删除原 59-323 段（v4 baseline，到 extract_correlation_key 起点 324 之前）
  4. `git diff` 字节级验证 `TerminationReason` **16 变体**顺序、3 处 `match` 表达式覆盖顺序（`exit_code` / `as_str` / `is_success`）字节级未变
- **Test scenarios**:
  - **Happy path**: `cargo build -p ralph-core` 通过
  - **Regression baseline**: `cargo nextest run -p ralph-core --no-fail-fast` 全绿
  - **Variant order preservation**: `git grep -A 1 "TerminationReason" crates/ralph-core/src/event_loop/types.rs | head -50` **16 个变体**顺序未变（修正：原 plan 写 17 是早期错算）
  - **Public API stable**: `lib.rs:104` 的 `pub use event_loop::{...}` 列表（v2 baseline，v1 baseline 写 103）+ `git grep "ralph_core::event_loop::" crates/ ralph-cli/ --include="*.rs" 2>/dev/null` 所有引用 `TerminationReason` / `ProcessedEvents` / `ProcessedEventsWithWaves` 的路径不变
- **Verification**:
  - `cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast` 全绿
  - `wc -l crates/ralph-core/src/event_loop/{mod,types}.rs` 显示 `mod.rs` 减约 220 行，`types.rs` 约 230 行
  - `cargo doc --no-deps` 0 warning
  - 追加 U3 Drift Sub-Note

### U4. 拆自由函数子模块：`workflow_guard.rs` / `policy.rs`（含 `payload_contract` 内容）

- **Goal**: 把 `crates/ralph-core/src/event_loop/mod.rs:324-961`（v4 baseline @ dbe6f35，实测：`extract_correlation_key` 324 / `apply_workflow_guard_validation` 407 / `apply_event_policy_validation` 587 / `finding_to_payload_contract_violation` 893 / `impl EventLoop` 起点 962；v3 baseline 322-959 / extract 322 / workflow 405 / policy 585 / payload 891 / impl 960）的自由函数抽到 2 个新子文件（`payload_contract` ~70 行内容并入 `policy.rs`）。U4 实施前必须**重新跑** `grep -nE "^fn (extract_correlation_key|apply_workflow_guard|apply_event_policy|finding_to_payload_contract)" crates/ralph-core/src/event_loop/mod.rs` 确认行号未漂移。
- **Requirements**: R1 / R2 / R3
- **Dependencies**: U3
- **Files**:
  - 修改 `crates/ralph-core/src/event_loop/workflow_guard.rs`（~280 行：`extract_correlation_key` + `WorkflowGuardRejectionDetail` + `WorkflowGuardOutcome` + `apply_workflow_guard_validation`，对应 mod.rs:324-586）
  - 修改 `crates/ralph-core/src/event_loop/policy.rs`（~620 行：`apply_event_policy_validation` + `finding_to_payload_contract_violation` + 相关 helper，对应 mod.rs:587-961）
  - 修改 `crates/ralph-core/src/event_loop/mod.rs:324-961` 段（删除 + 顶部 `pub use` 重新导出）
- **Approach**:
  1. 按函数段整段复制：324-586 → `workflow_guard.rs`；587-961 → `policy.rs`（含原 893-961 payload_contract 段）
  2. 函数内 `use crate::*` 引用按需保留或重写为 `use super::*`（如果跨子模块）
  3. `mod.rs` 顶部加 `pub use workflow_guard::*; pub use policy::*;`
  4. `mod.rs` 删除原 322-959 段
  5. `git diff` 验证每段字节级未变
- **Test scenarios**:
  - **Happy path**: `cargo build -p ralph-core` 通过
  - **Regression baseline**: `cargo nextest run -p ralph-core --no-fail-fast` 全绿；`crates/ralph-core/src/event_loop/tests/` 下 44 个子文件全部测试通过
  - **Byte-level preservation**: `git diff` 中 2 个新子文件内容字节级 = 原 `mod.rs` 对应段
  - **No dead code**: `cargo build -p ralph-core` 0 `dead_code` / 0 `unused_imports` warning
- **Verification**:
  - `cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast` 全绿
  - `wc -l crates/ralph-core/src/event_loop/{mod,workflow_guard,policy}.rs` 显示 `mod.rs` 减约 640 行，2 个新子文件总和 ~900 行
  - `cargo clippy -p ralph-core --all-targets` 0 warning
  - 追加 U4 Drift Sub-Note

### U4.5. pre-U5 设计 review：6 个 `process_parse_result` validation 层在 KTD12 五域中的归属矩阵（adversarial finding #8 补救）

- **Goal**: 在 U5 实施 **1 860 行** `process_parse_result`（v4 baseline mod.rs:4921-6780，v3 baseline 1 617 行 → v4 +243 行，主要来自 wave attribution 修复）整体归 `prompt.rs` 之前，先在 plan 中显式列出 6 个 inline validation 层（origin guard / topic format / event policy / state machine / workflow guard / execution contract）的 KTD12 归属矩阵，让 reviewer 提前判定是否真的能按"主返回 / 主副作用归属 prompt 域"统一处理；如有歧义记入 R3 follow-up 而非在 PR 中临时判定（避免 R-Refactor-9 "U5 主体改动 1 860 行方法归错域后无法在不动业务的前提下重新分块" 风险）。
- **Requirements**: KTD12 可证伪性（plan-level 校正 adversarial finding #8）
- **Dependencies**: U4
- **Files**:
  - 修改本 plan 文档追加 "U4.5 归属矩阵" 段（实施前在 U5 之前完成），格式：
    ```
    | validation 层 | 主返回 | 主副作用 | KTD12 目标子模块 | 跨域标记 |
    | origin guard | () | 拒绝 + emit rejection envelope | prompt.rs | 跨 dispatch（reject 后 hat 不激活）|
    | topic format | () | 拒绝 unknown topic | prompt.rs | 跨 lifecycle（state 不前进）|
    | event policy | PolicyDecision | 修改 self.bus（forward event） | prompt.rs | 跨 diagnostics（写 recovery envelope）|
    | state machine | StateMachineDecision | 修改 self.state | prompt.rs | 跨 lifecycle（直接 state transition）|
    | workflow guard | Vec<PolicyFinding> | 拒绝 + 写 self.diagnostics | prompt.rs | 跨 diagnostics（直接写 collector）|
    | execution contract | ExecutionContractDecision | 拒绝 + 写 self.diagnostics | prompt.rs | 跨 diagnostics + lifecycle |
    ```
- **Approach**:
  1. U4.5 实施时（U4 commit 之后 / U5 commit 之前）用 `grep -nE "origin guard|topic format|event policy|state machine|workflow guard|execution contract" crates/ralph-core/src/event_loop/mod.rs` 验证 6 个 inline 块位置
  2. 由 plan author + 1 名 reviewer 共同审核矩阵（reviewer 可在 PR 评论区逐行 approve/reject）
  3. 矩阵任一行未达成共识时，U5 暂停：要么把 process_parse_result 改归"跨域"标注（KTD12 例外：process_parse_result 因体量过大需独立 review），要么增加 U5a/U5b 拆分
- **Verification**:
  - 本 plan 文档 "U4.5 归属矩阵" 段 6 行 + 1 共识签字
  - U5 实施时 grep 6 个 inline 块位置与矩阵匹配
  - U5 commit message 引用 "U4.5 矩阵" 作为过程证据

### U5. 拆 `impl EventLoop` 块按方法域：5 个子文件（含 `process_parse_result` 归 `prompt.rs`）

- **Goal**: 把 `crates/ralph-core/src/event_loop/mod.rs:962-7115`（v4 baseline @ dbe6f35，实测：`impl EventLoop` 主块 962-7114（6 153 行）；尾部 `format_duration` 7128 / `termination_status_text` 7144 作为自由函数随 `termination.rs` 一并迁出）的 ~6 153 行按方法域拆到 5 个新子文件（lifecycle / termination / dispatch / prompt / diagnostics）；**`process_parse_result` 1 860 行单方法（mod.rs:4921-6780）整体归 `prompt.rs`**（按 KTD12 边界规则：方法主语义是 prompt 解析；v4 较 v3 baseline +243 行，主要来自 wave attribution 修复）。**新增方法注意**：commit `37bd281` 引入的 `inject_review_aggregate_timeouts` 归 **diagnostics**（KTD12 规则：返回 bool + 副作用是 telemetry / 注入 recovery，diagnostics 域）；`review_step_tracker.check_semantic_gates` / `observe_accepted` 由 `review_step_state` 模块自管，不在本拆分范围。U5 实施前必须**重新 grep** `impl EventLoop` 起止 + 所有 120 个方法名（`awk '/^impl EventLoop/{f=1;next} f && /^}/{exit} f && /^    (pub |pub\(crate\) )?fn /{c++} END{print c}'`，v4 baseline @ dbe6f35 实测输出 120，比 v3 baseline 118 +2——v3→v4 期间 wave attribution 修复 `e695b6c` 新增 2 个方法，大概率在 `process_parse_result` 内或 `next_hat` / attribution 路径），确认与 KTD12 规则对应一致。
- **Requirements**: R1 / R2 / R3
- **Dependencies**: U4
- **Files**:
  - 修改 `crates/ralph-core/src/event_loop/lifecycle.rs`（~1 360 行：生命周期方法 + KTD12 规则；v4 较 v3 +20）
  - 修改 `crates/ralph-core/src/event_loop/termination.rs`（~960 行：终止条件方法 + `format_duration` + `termination_status_text` + KTD12 规则；v4 +20）
  - 修改 `crates/ralph-core/src/event_loop/dispatch.rs`（~700 行：调度方法 + KTD12 规则；v4 +20）
  - 修改 `crates/ralph-core/src/event_loop/prompt.rs`（~3 520 行：prompt 处理方法 + **`process_parse_result` 1 860 行整体方法体**）
  - 修改 `crates/ralph-core/src/event_loop/diagnostics.rs`（~420 行：诊断方法 + `inject_review_aggregate_timeouts` + KTD12 规则；v4 +20）
  - 修改 `crates/ralph-core/src/event_loop/mod.rs:962-7115` 段（删除整块 impl + 末尾两个自由函数；v4 baseline mod.rs 7 171 行）
- **Approach**:
  1. 按方法名（`grep '    pub fn' / '    fn' crates/ralph-core/src/event_loop/mod.rs`）聚合到 5 个域，遵循 KTD12 边界规则
  2. 每个子文件顶部 `use super::*; use crate::...` 按需
  3. 每个子文件结构：`//! <域描述>` 顶部 rustdoc + `use` 段 + `impl EventLoop { ... }` 块
  4. `mod.rs` 顶部已有 `mod lifecycle; mod termination; mod dispatch; mod prompt; mod diagnostics;` 声明（U1 已加）
  5. `mod.rs` 删除原 962-7115 整段 impl + 末尾两个自由函数
  6. `git diff` 验证每个方法体字节级未变（`diff -u <(sed -n '962,7115p' mod.rs.bak) <(cat lifecycle.rs termination.rs dispatch.rs prompt.rs diagnostics.rs)`）
  7. 跨文件 `self.method()` 解析验证：`cargo build -p ralph-cli` 0 error
- **Test scenarios**:
  - **Happy path**: `cargo build -p ralph-core` 通过
  - **Regression baseline**: `cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast` 全绿
  - **Method-level byte preservation**: 5 个子文件方法体总字节 = 原 `mod.rs:962-7115` 段
  - **Method visibility preserved**: **120 个方法**（v4 baseline @ dbe6f35 实测，比 v3 baseline 118 +2）的 `pub` / `pub(crate)` / private 可见性逐个未变
  - **Cross-module call preserved**: `grep -nE "fn check_termination|fn activate|fn dispatch_hat|fn build_prompt|fn emit_telemetry|fn inject_review_aggregate_timeouts" crates/ralph-core/src/event_loop/{lifecycle,termination,dispatch,prompt,diagnostics}.rs` 全部能找到（同名方法冲突检测）
  - **`process_parse_result` 完整性**: `crates/ralph-core/src/event_loop/prompt.rs` 中 `process_parse_result` 方法体**未切**（U5 仅迁位置，U6 才抽 6 个 free fn）
  - **review_step_state 不动**: `git diff -- crates/ralph-core/src/event_loop/review_step_state.rs` 0 行变更（v2 baseline 605 行，比 v1 +93 行，但本计划依然不动）
- **Verification**:
  - `cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast` 全绿
  - `wc -l crates/ralph-core/src/event_loop/{mod,lifecycle,termination,dispatch,prompt,diagnostics}.rs` 显示 `mod.rs` 减约 6 153 行，5 个新子文件总和 ~6 960 行（v4 较 v3 baseline 估 +320）
  - `cargo build -p ralph-cli` 0 error
  - `cargo clippy -p ralph-core --all-targets` 0 warning
  - 追加 U5 Drift Sub-Note

### U6. 拆 `event_loop/wave.rs` + 抽 6 个 validate_* 自由函数（含 characterization test）

- **Goal**: 在 U5 把 `process_parse_result` 迁到 `prompt.rs` 后，**新增** `event_loop/wave.rs`（wave 辅助独立子模块）+ 在 `prompt.rs::process_parse_result` 调用点把 6 个 inline validation 层（origin guard / topic format / event policy / state machine / workflow guard / execution contract）抽为 `event_loop::process::validate_*` 自由函数（在 `event_loop/process.rs` placeholder 内实现，或 `prompt.rs` 内的嵌套 `pub(crate) mod process`；U1 scaffold 已创建 `process.rs` placeholder，优先复用该文件）。`process_parse_result` 方法体在 U6 中**字节级改写**（行为不变，U6 唯一例外）。
- **Requirements**: R1 / R2.b（U6 例外条款）/ R3
- **Dependencies**: U5
- **Execution note**: U6 实施前**必须先**写 characterization test 锁定 `process_parse_result` 当前行为的 golden sample（输入 → 输出快照），否则 6 个 free fn 抽取后**没有 golden 对照**——R-Refactor-9 描述的 "nextest 全绿" 不能保证行为等价（nextest 只测"通过"，不测"等价"）。
- **Files**:
  - 修改 `crates/ralph-core/src/event_loop/wave.rs`（~160 行：wave 辅助从 `prompt.rs` 末尾或其他位置迁出）
  - 修改 `crates/ralph-core/src/event_loop/process.rs`（从 U1 placeholder 改为实质内容：6 个 `validate_*` 自由函数；若选择嵌套 mod 方案，则改为在 `prompt.rs` 内实现并删除 `process.rs`）
  - 修改 `crates/ralph-core/src/event_loop/prompt.rs`（`process_parse_result` 方法体**字节级改写**：6 个 inline validation 层抽为 `fn validate_origin_guard(&mut self, ...) -> Result<(), ...>` 等 6 个 `pub(crate)` 自由函数，`process_parse_result` 主体改为调用这 6 个函数）
  - 新增 `crates/ralph-core/src/event_loop/tests/process_characterization.rs`（U6 实施前先写，记录 `process_parse_result` 行为的 golden sample，U6 完成后用此测试验证行为不变）
  - 修改 `crates/ralph-core/src/event_loop/mod.rs` 顶部加 `pub mod wave;` 等（`process` mod 声明已由 U1 就位）
- **Approach**:
  1. **U6 步骤 0**（characterization test 先行，**adversarial finding #9 三步扩展**）：
     - **步骤 0a**（mutation 分数基线）：用 mutation 引擎（参考 `scripts/hooks-mutation-gate.sh`）跑 `process_parse_result` 当前 mutation 分数作为基线；U6 完成后 mutation 分数不得显著下降（>5% 下降视为 silent regression）
     - **步骤 0b**（间接调用链补充）：用 `git grep -nE "process_parse_result|validate_.*\(.*&mut" crates/ --include="*.rs" --include="*.md"` 列出 6 个 validation 层的所有 helper；为每层补充 ≥3 个 characterization test（happy / edge / error 三类）共 ≥18 个新测试，覆盖：payload 截断 / 并发 race / unicode 边界 / serde 反序列化失败后 state 残留 / `Ok(Some(internal))` 与 `Ok(Some(internal_dup))` 字符串 typo 等 edge case
     - **步骤 0c**（golden sample 记录）：在 `crates/ralph-core/src/event_loop/tests/process_characterization.rs` 中写 golden sample 测试，记录 `process_parse_result` 当前所有输入 → 输出的快照（用 `insta` crate 或同等 snapshot diff 工具显式存储，U6 完成后 reviewer 必须逐条 review 每一处 `Ok(...)` payload 字符串）
  2. U6 步骤 1：在 `wave.rs`（新建）中加 wave 辅助代码（从 `prompt.rs` 末尾或其他位置迁出）
  3. U6 步骤 2：在 `process.rs`（U1 placeholder）内实现 6 个 `validate_*` 自由函数；若选择嵌套 mod 方案，则在 `prompt.rs` 内声明 `pub(crate) mod process` 并把函数体放入其中
  4. **U6 步骤 2.5**（adversarial finding #10 硬规则——U6 字节级改写护栏）：
     - **6 个 `validate_*` 自由函数签名由本 plan 在 U6 启动前一次性锁定**（参数名 / 参数类型 / 返回类型逐字预定义），U6 实施时不允许改签名（除非 reviewer 在 U6 commit message 显式签字）
     - **每个 `validate_*` 函数体与原 inline 块**通过 `diff -u <(echo "原 inline 块") <(echo "新 fn 体")` 显示 0 行差异（仅允许缩进变化，函数体外任何 token 改动都需在 commit message 列出 1 行 rationale）
     - **`process_parse_result` 主体调用顺序**显式预定义为：`origin_guard → topic_format → event_policy → state_machine → workflow_guard → execution_contract`，U6 实施时用 `grep -nE "validate_(origin|topic|event|state|workflow|execution)" prompt.rs` 验证顺序字节级匹配
     - **anti-pattern 联合校验**：若任一 `validate_*` 函数体被改的同时又修改了 6 层之外的代码（如修一个 unicode 路径 bug / 把 `if let Some(x) = y` 换成 `if y.is_some() { let x = y.unwrap() }`），U6 commit 视为违反"拆分时'顺手优化'或'乘机修 bug'"（2026-06-03-002 R4 明确禁止），强制 `git revert` 并把改动拆到独立 commit
  5. U6 步骤 3：把 `process_parse_result` 方法体内 6 个 inline validation 块**整段抽出**到 6 个自由函数（**字节级不保留**，U6 唯一例外）；`process_parse_result` 主体改为调用这 6 个函数（**调用顺序严格保持**：origin → topic → policy → state → workflow → execution）
  5. U6 步骤 4：跑 `process_characterization.rs` 验证行为不变；不通过立即 `git revert U6 commit`
  6. U6 步骤 5：`git diff` 中 `process_parse_result` 调用顺序、参数传递、返回结果需逐处 review
- **Patterns to follow**:
  - KTD10 不切 `process_parse_result` 内部单方法（U6 抽 6 个 free fn 不算"切"单方法，仍是单一 `process_parse_result` 入口）
  - 2026-06-03-002 plan 对 `run_loop_impl` 2 950 行的处理（同样不切单方法内部）
- **Test scenarios**:
  - **Happy path**: `cargo build -p ralph-core` 通过
  - **Characterization test passes**: `process_characterization.rs` 全绿（这是 U6 的"零行为变化"硬保证）
  - **Regression baseline**: `cargo nextest run -p ralph-core --no-fail-fast` 全绿
  - **Free fn call order**: 6 个 `validate_*` 调用顺序与原 inline 块顺序逐处一致（grep 验证）
  - **Free fn signatures**: 6 个自由函数入参、出参与原 inline 块使用的 `&mut self` 字段 + 局部变量对应
- **Verification**:
  - `cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast` 全绿
  - `process_characterization.rs` 全绿（U6 行为不变性硬保证）
  - `wc -l crates/ralph-core/src/event_loop/{mod,prompt,process,wave}.rs` 显示 `mod.rs` ≤ 100 行、`process.rs` ≤ 600 行
  - `cargo clippy -p ralph-core --all-targets` 0 warning
  - 追加 U6 Drift Sub-Note

### U7. 补全剩余 14 个 tests 子文件 + 完整验证 + 文档反向验证 + Repo Drift Note

- **Goal**: 把 U2 末尾保留在 `tests/mod.rs` 的 14 个主题测试族全部迁出到独立子文件；跑完整验证套件（E2E mock / smoke / scenarios / clippy / fmt / doctest / audit-file-sizes）；执行文档反向验证 + Repo Drift Note 追加。
- **Requirements**: R1 / R2 / R3 / R4 / R5 / R6 / R7 / R8 / R9 / R10
- **Dependencies**: U6
- **Files**:
  - 创建 `crates/ralph-cli/src/loop_runner/tests/{suspend.rs, hard_gate.rs, hard_gate_payload_contract.rs, pty_user_interactive.rs, resolve_loop_id_and_iteration.rs, loop_termination.rs, async_pty.rs, diagnostics.rs, recovery.rs, preset_lint_gate.rs, merge_queue.rs, prompt_handling.rs, event_logging_and_planning_session.rs, late_events_and_hat_selection.rs, event_pipeline.rs}.rs`（14 个新子文件，把 U2 末尾保留在 `tests/mod.rs` 的测试族全部迁出）
  - 修改 `crates/ralph-cli/src/loop_runner/tests/mod.rs`（删除迁出的测试段，保留 mutex 文档 + mod 声明 + 14 个 `mod xxx;` 声明）
  - 修改本 plan 文档追加 `## Repo Drift Note` 段（合并 U3-U6 的 Sub-Note）
  - 修改 `CLAUDE.md` / `AGENTS.md`（如有必要）
  - 修改 `docs/achieved/plan/2026-06-03-002-refactor-split-large-files-plan.md`（追加 Implementation Status 更新）
  - 修改 `docs/achieved/plan/2026-05-12-001-feat-harness-extension-plan.md` 等 70+ 个引用文件（实测 v2 baseline 37bd281：`event_loop/mod.rs` 252 处引用 / 59 个文件 + `loop_runner/tests.rs` 68 处引用 / 20 个文件；v3 增量 commits 不涉及 `event_loop::*` / `loop_runner::*` 公开 API 名变更，引用面维持 70+）
- **Approach**:
  1. U2 末尾 `tests/mod.rs` 保留的 14 个测试族按主题迁出到独立子文件（按 KTD3 列表的 14 个主题）
  2. 每个子文件用 `use super::*; use super::common::*;` 引用共享 helper
  3. `tests/mod.rs` 末尾的测试段清空（仅保留 mutex 文档 + mod 声明）
  4. 跑 `cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast`
  5. 跑 `cargo run -p ralph-e2e -- --mock`
  6. 跑 `cargo test -p ralph-core scenarios`
  7. 跑 `cargo clippy --workspace --all-targets`
  8. 跑 `cargo fmt --check`
  9. 跑 `cargo test --workspace --exclude ralph-e2e --doc`
  10. 跑 `bash scripts/audit-file-sizes.sh` + 手动 `awk '$1>2200{print}' crates/ralph-core/src/event_loop/*.rs crates/ralph-cli/src/loop_runner/tests/*.rs`（脚本无阈值断言）
  11. 跑 `diff -u CLAUDE.md AGENTS.md`（必须 0 差异）
  12. `git grep -nE "event_loop/mod\.rs:|loop_runner/tests\.rs:" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null` 列出所有引用
  13. 逐条更新失效引用
  14. 在本 plan 文档 `## Repo Drift Note` 段合并 U3-U6 Sub-Note 为完整表格
- **Test scenarios**:
  - **Full regression suite**: 所有 10 个验证命令全绿
  - **Audit script**: `bash scripts/audit-file-sizes.sh` 通过，2 个主文件 ≤ 1 000 行
  - **Documentation sync**: `diff -u CLAUDE.md AGENTS.md` 0 差异
  - **Drift note complete**: `git grep -nE "event_loop/mod\.rs:|loop_runner/tests\.rs:" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null` 输出全部已被处理
- **Verification**:
  - 所有 10 个验证命令（步骤 4-13）全绿
  - `wc -l crates/ralph-core/src/event_loop/{mod,types,workflow_guard,policy,lifecycle,termination,dispatch,prompt,diagnostics,wave}.rs` 显示 `mod.rs` ≤ 100 行
  - `wc -l crates/ralph-cli/src/loop_runner/tests/*.rs` 显示 `mod.rs` ≤ 250 行
  - `bash scripts/audit-file-sizes.sh` PASS
  - `diff -u CLAUDE.md AGENTS.md` 0 差异
  - 本 plan 文档 `## Repo Drift Note` 段已追加
  - 70+ 引用文档已同步（实测 37bd281 baseline grep 命中 252+68=320 引用位置，去重后约 70 个文件）

## Scope Boundaries

### 范围内（in-scope）

- `crates/ralph-core/src/event_loop/mod.rs` 拆为 **10 个新建**子模块（types / workflow_guard / policy / lifecycle / termination / dispatch / prompt / diagnostics / process / wave；`payload_contract` 70 行内容并入 `policy.rs`）
- `crates/ralph-cli/src/loop_runner/tests.rs` 拆为 **19 个** `tests/*.rs` 子文件（mod + common + 17 主题；按主题合并使 0 个 < 200 行）
- 6 个 inline validation 层从 `process_parse_result` 抽为 `event_loop::process::validate_*` 自由函数（**U6 唯一允许的字节级改写**；行为不变，由 characterization test 锁定）
- **2 个** `tests.rs` 内 process-global Mutex 按 KTD7 拓扑迁到 `tests/fake_path.rs` 模块内（位置改变但形式不变）；`wave/acp_mock.rs` 的 2 个 `MOCK_ACP_*` 完全**不动**
- 文档反向验证 + Repo Drift Note 追加（U3-U6 每 U 追加 Sub-Note，U7 合并）
- CLAUDE.md ↔ AGENTS.md 同步检查
- `audit-file-sizes.sh` 阈值复检（手动 `awk` 校验）

### 范围外（out-of-scope）

- 任何公开行为、错误信息、日志、退出码、`TerminationReason` **16 变体**顺序、`EventLoop` **14 字段**（v4 baseline 与 v3 一致）顺序改动（U6 字节级改写除外）
- 任何当前已通过的测试或运行时 bug 修复
- 重构 `crates/ralph-core/src/event_loop/loop_state.rs`（845 行）或 `crates/ralph-core/src/event_loop/rejection.rs`（593 行）或 `crates/ralph-core/src/event_loop/review_step_state.rs`（605 行，commit 37bd281 引入）或 `crates/ralph-core/src/event_loop/tests/`（44 个子文件）内部组织
- 拆分 `crates/ralph-cli/src/loop_runner/runner.rs`（170 KB 主 runner）—— 仅核对 imports 兼容，不拆
- 拆分 `crates/ralph-cli/src/loop_runner/runner.rs` 中的 `run_loop_impl` 2 950 行（2026-06-03-002 已决策不切）
- 拆分 `crates/ralph-cli/src/loop_runner/hard_gate.rs`（24 KB）/ `preset_lint_gate.rs`（20 KB）
- 拆分 `crates/ralph-cli/src/loop_runner/wave/` 5 个子文件
- 拆分 `crates/ralph-cli/src/commands/{run.rs, emit.rs}`（1 500-1 700 行）
- 拆分 `crates/ralph-core/src/config/ralph_config.rs`（3 660 行）
- 任何 `Cargo.toml` 依赖变更、`serial_test` 引入、`.config/nextest.toml` 修改

### 推迟到 Follow-Up Work

- `crates/ralph-cli/src/loop_runner/runner.rs`（170 KB）拆分：留作 R3 第二轮
- `crates/ralph-core/src/config/ralph_config.rs`（3 660 行）拆分：留作 R3 第二轮
- `crates/ralph-cli/src/commands/{run.rs, emit.rs}`（1 500-1 700 行）拆分：留作 R3 第二轮
- **未来**：本 plan U5 创建的 `crates/ralph-core/src/event_loop/prompt.rs`（含 `process_parse_result` 后约 3 000 行）若 R1 阈值被改写，可进一步拆为 `prompt/{build.rs, sections/}` —— 留作 R3 第二轮
- `crates/ralph-cli/src/loop_runner/{hard_gate.rs, preset_lint_gate.rs, wave/}` 拆分：留作 R3 第二轮
- `crates/ralph-core/src/event_loop/prompt.rs` 中 `process_parse_result` 的 6 个 `validate_*` 自由函数已在 U1 预留给 `event_loop/process.rs`；U6 将在此文件（或 `prompt.rs` 嵌套 mod）中实现

## Risks & Dependencies

### Risks

- **R-Refactor-1**（高）：`TerminationReason` **16 变体**顺序 + 3 处 `match` 表达式覆盖顺序（`exit_code` 行 168-191 / `as_str` 行 198-215 / `is_success` 行 218-220）在 U3 抽 `types.rs` 时漂移 → `match` 行为变化。**检测机制**：U3 完成后用 `git grep -A 1 "TerminationReason" crates/ralph-core/src/event_loop/types.rs | head -50` + 字节级 `git diff` 验证。**Escape hatch**：若真发现变体顺序错（历史 bug），决策树是"report-only 不修"（保持零回归）而非"fix-with-rationale"（引入逻辑变更）。任何 attribute 位置变动立即 `git revert`。
- **R-Refactor-2**（高）：`EventLoop` **14 字段**顺序在 U5 拆 impl 时漂移 → `Default` / `Debug` / `PartialEq` 派生输出变化。**检测机制**：U5 完成后用 `git diff` 字节级 + `awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs` 输出 **14** 验证（v4 baseline @ dbe6f35 实测，与 v3 一致；v0 baseline 写 13 是早期错算，v1 baseline 也写 13 是 doc-review 触发后仍未补正，**v2 校正回 14** 并在 v3/v4 维持）。Escape hatch 同 R-Refactor-1。
- **R-Refactor-3**（中）：**2 个** `tests.rs` 内 process-global Mutex（`FAKE_PATH_BACKEND_SERIAL` / `FAKE_PATH_BACKEND_BIN`）在 U2 拆分时形式变化（`pub` ↔ private 改 `static` ↔ 改 `Mutex<...>` 不带 `LazyLock`）→ `cli-serial` test group 失效。**检测机制**：U2 完成后用 `grep -A3 "static FAKE_PATH_BACKEND" crates/ralph-cli/src/loop_runner/tests/fake_path.rs` 验证 2 个 Mutex 形式逐字节不变（**实测为 private `static`，非 `pub(crate)`**）。`wave/acp_mock.rs:97-104` 的 2 个 `pub static MOCK_ACP_*` **不在本计划范围**，但 U2 必须验证 `git diff --stat -- crates/ralph-cli/src/loop_runner/wave/acp_mock.rs` 0 行变更。
- **R-Refactor-4**（中）：`tests.rs` 头部 1-40 行的 contributor-facing mutex 文档在 U2 拆分时被裁剪或移位 → 后续贡献者跑测试时踩坑。**检测机制**：U2 完成后用 `git diff` 验证 `tests/mod.rs` 顶部 1-40 行**整段**与原 `tests.rs` 头部 1-40 行一致（`diff <(sed -n '1,40p' tests/mod.rs) <(git show HEAD:tests.rs | sed -n '1,40p')` 0 差异，**修正原 plan "1-55p" 因实测 1-40 行才是 mutex 文档范围**）。**额外缓解**：U7 时考虑在 `README.md` 或 `CONTRIBUTING.md` 顶部也放同样警告（双写）。
- **R-Refactor-5**（中）：runner.rs 中 `event_loop.check_termination()` 等方法调用在 U5 拆 impl 后找不到（多文件 impl 解析失败 / 同名方法冲突）。**检测机制**：U5 完成后跑 `cargo build -p ralph-cli` 必须 0 error；同名方法冲突用 `grep -nE "fn <method_name>" crates/ralph-core/src/event_loop/{lifecycle,termination,dispatch,prompt,diagnostics}.rs` 快速定位。
- **R-Refactor-6**（中）：文档反向验证遗漏某些 `event_loop::xxx` 引用 → docs 漂移。**检测机制**：U3-U7 每 U 用 `git grep -nE "event_loop/mod\.rs:|loop_runner/tests\.rs:|event_loop::[a-z_]+|loop_runner::tests::" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null` 完整列出引用并逐条处理；立即追加 Sub-Note。
- **R-Refactor-7**（低）：`scripts/audit-file-sizes.sh` 阈值在 U7 时发现不满足（2 个主文件 > 1 000 行）。**检测机制**：脚本本身**无阈值断言**，U7 必跑 + 手动 `awk '$1>2200{print}' crates/ralph-core/src/event_loop/*.rs crates/ralph-cli/src/loop_runner/tests/*.rs` 校验子文件行数；如不满足，调整 `mod.rs` 顶部注释 / 删除冗余 import 把 `mod.rs` 压到 ≤ 100 行。
- **R-Refactor-8**（低）：U6 抽 6 个 validation 层为自由函数时引入逻辑 bug（参数传递、调用顺序、返回结果偏差）。**检测机制**：U6 步骤 0 先写 characterization test 锁定 `process_parse_result` 当前行为的 golden sample（输入 → 输出快照）；U6 步骤 5 跑 `process_characterization.rs` 验证行为不变；不通过立即 `git revert U6 commit`。
- **R-Refactor-9**（低）：5 个 `impl EventLoop` 域边界判定歧义（跨域方法归属）。**检测机制**：U5 实施时严格按 KTD12 规则；以方法主返回 / 主副作用归属；不可调和的歧义记入 R3 follow-up，**不**在 PR 中临时判定。
- **R-Refactor-10**（低）：`tests/common.rs` helper 修改影响 17 个子文件（耦合放大）。**检测机制**：U2 / U7 实施时把 wave 特定 / fake_path 特定 helper 移到子文件模块内（KTD4），仅在 `common.rs` 留真正跨子文件共享的 helper。

### Dependencies

- **D-Refactor-1**（外部）：项目 `cli-serial` test group + **4 个 process-global Mutex**（2 个 `FAKE_PATH_BACKEND_*` 在 `tests.rs` 内 **本计划范围内**，U2 迁移到 `tests/fake_path.rs`；2 个 `MOCK_ACP_*` 在 `wave/acp_mock.rs` 内 **不在本计划范围**，由上一轮处理，本计划完全不动）配置不变（项目硬约束，由 `.config/nextest.toml` + `docs/achieved/plan/2026-06-01-001-feat-parallel-test-execution-plan.md` 锁定）
- **D-Refactor-2**（内部）：U1 公共子模块声明必须先于 U2-U7（提供占位 + pub use 转发点）
- **D-Refactor-3**（内部）：U2 必须先于 U3-U7 完成（tests 拆分是隔离关注点的第一步）
- **D-Refactor-4**（内部）：U3（types 锁定）必须先于 U4-U6（自由函数引用 types）
- **D-Refactor-5**（内部）：U4（自由函数）必须先于 U5（impl 块）—— impl 块中的方法调用自由函数，需先就位
- **D-Refactor-6**（内部）：U5（impl 块）必须先于 U6（wave + 6 个自由函数）—— `process_parse_result` 必须在 `prompt.rs` 就位
- **D-Refactor-7**（跨 crate 并行可能性）：U2 改 `loop_runner/tests.rs`（`ralph-cli` 包）与 U3 改 `event_loop/mod.rs`（`ralph-core` 包）可并行，**但** U3 依赖 U1 的 mod 声明、U2 依赖 U1 的 mod 声明——所以 U1 → (U2 || U3) → U4 → U5 → U6 → U7 是更紧凑的依赖链。本 plan 保持 U1→U7 顺序不并行化（验证命令**单 commit** 更易 review）。

## Verification（plan-level）

- **Workspace build**: `cargo build --workspace` 0 error
- **Workspace tests**: `cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast` 全绿；passed/failed 数字与 U1 基线完全一致
- **Workspace doctest**: `cargo test --workspace --exclude ralph-e2e --doc` 全绿
- **Clippy**: `cargo clippy --workspace --all-targets` 0 warning
- **Format**: `cargo fmt --check` 通过
- **E2E mock**: `cargo run -p ralph-e2e -- --mock` 全绿
- **Scenarios**: `cargo test -p ralph-core scenarios` 全绿
- **Audit script**: `bash scripts/audit-file-sizes.sh` PASS + 手动 `awk '$1>2200{print}' crates/ralph-core/src/event_loop/*.rs crates/ralph-cli/src/loop_runner/tests/*.rs` 0 输出
- **Documentation sync**: `diff -u CLAUDE.md AGENTS.md` 0 差异
- **Repo drift**: `git grep -nE "event_loop/mod\.rs:|loop_runner/tests\.rs:" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null` 输出全部已被处理
- **Process-global Mutex**: `grep -A3 "static FAKE_PATH_BACKEND" crates/ralph-cli/src/loop_runner/tests/fake_path.rs` **2 个** Mutex 形式不变；`git diff --stat -- crates/ralph-cli/src/loop_runner/wave/acp_mock.rs` 0 行变更（确认 `MOCK_ACP_*` 未受本计划影响）
- **Test infrastructure doc**: `diff <(sed -n '1,40p' crates/ralph-cli/src/loop_runner/tests/mod.rs) <(git show HEAD:crates/ralph-cli/src/loop_runner/tests.rs | sed -n '1,40p')` 0 差异（修正：原 plan 写 `1,55p`——实测 mutex 文档在 1-40 行，55 行范围包含 U5 段标记 + 第一个 test 函数，verification 命令无法稳定 0 差异）
- **Public API stable**: `cargo doc --no-deps` 0 warning；`git grep "ralph_core::event_loop::" crates/ ralph-cli/ --include="*.rs" 2>/dev/null` 全部可达
- **lib.rs:104 stable**（v4 baseline 与 v3 一致，均为 104；v1 baseline 写 103）：`git diff HEAD~1 -- crates/ralph-core/src/lib.rs | grep -E "^\+.*pub use event_loop::|^-.*pub use event_loop::"` 0 输出
- **TerminationReason order**: `git grep -A 1 "TerminationReason" crates/ralph-core/src/event_loop/types.rs | head -50` **16 个变体**顺序未变
- **EventLoop field order**: `awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs` 输出 **14**（v4 baseline @ dbe6f35 实测，与 v3 一致）+ `git diff HEAD -- crates/ralph-core/src/event_loop/mod.rs` 字节级验证字段顺序未变
- **process_parse_result behavior**: U6 后 `process_characterization.rs` 全绿
- **review_step_state 不动**: `git diff HEAD~7 -- crates/ralph-core/src/event_loop/review_step_state.rs` 0 行变更（U1-U7 全过程不动；v4 baseline 605 行，与 v3 一致）

## Sources & Research

- **核心 plan（模式来源，必读）**：
  - `docs/achieved/plan/2026-06-03-002-refactor-split-large-files-plan.md`（KTD1-8 + Risks + AC1-16 + U1-U6 实施状态 + commit `4ba6e37` / `723230f` / `d68da3c` / `fc89516` / `a05d753` / `d8495ac` / `fda37f4` / `73e8a6f`）
  - `docs/plans/2026-06-03-003-refactor-schema-refs-replace-regex-plan.md`（Repo Drift Note 格式范例 + 反向验证流程 + commit `1470e49` / `773556e` / `70184f9`）
  - `docs/achieved/plan/2026-06-01-001-feat-parallel-test-execution-plan.md`（cli-serial + process-global Mutex 背景）
- **模式与组织（已存在）**：
  - `crates/ralph-cli/src/loop_runner/mod.rs:7-24`（`mod xxx;` 声明顺序）
  - `crates/ralph-cli/src/loop_runner/mod.rs:26-58`（`pub use` 重新导出模式 + `pub use xxx::*;` 通配）
  - `crates/ralph-core/src/lib.rs:80`（v4 baseline，`pub use config::{...}` 公开 API 列表起点；与 v3 一致，v1 baseline 写 79；**禁止修改**）
  - `crates/ralph-core/src/lib.rs:104`（v4 baseline，`pub use event_loop::{...}` 公开 API 列表起点；与 v3 一致，v1 baseline 写 103；**禁止修改**）
  - `crates/ralph-cli/src/loop_runner/wave/acp_mock.rs:97-104`（`pub static MOCK_ACP_EXECUTIONS` 97-100 / `MOCK_ACP_EXECUTION_SERIAL` 102-104，本计划**不动**）
  - `crates/ralph-cli/src/loop_runner/wave/mod.rs:10`（`pub use acp_mock::{...}`，本计划**不动**）
  - `crates/ralph-core/src/event_loop/review_step_state.rs:1-605`（v4 baseline，commit 37bd281 引入，v1 baseline 512 行 → v2 605 行 → v3/v4 605 行不变；本计划**不动**）
  - `crates/ralph-cli/src/operation_guard.rs:147`（注释引用 `loop_runner::inject_hat_execution_env`，AC13 验证点）
  - `crates/ralph-core/src/event_loop/loop_state.rs:2255-2277`（已有 `activate` 端代码，commit `f4bee78`，需保持路径兼容）
- **项目硬约束（不可改）**：
  - `.config/nextest.toml`（cli-serial + max-threads=1）
  - **2 个** `tests.rs` 内 process-global Mutex（`FAKE_PATH_BACKEND_SERIAL` / `FAKE_PATH_BACKEND_BIN`，位于 tests.rs:605-610）—— 实测为 private `static`
  - **2 个** `wave/acp_mock.rs` 内 process-global Mutex（`MOCK_ACP_EXECUTIONS` / `MOCK_ACP_EXECUTION_SERIAL`，位于 acp_mock.rs:97-104）—— 实测为 `pub static`，由上一轮迁出，本计划**完全不动**
  - `lib.rs:80` 的 `pub use config::{...}` 公开 API 列表（v4 baseline 与 v3 一致）
  - `lib.rs:104` 的 `pub use event_loop::{...}` 公开 API 列表（v4 baseline 与 v3 一致）
  - `Cli` / `Commands` enum 位置（保留在 `main.rs`，clap 派生）
- **反模式（必须避免）**：
  - 拆分时"顺手优化"或"乘机修 bug"——2026-06-03-002 R4 明确禁止
  - 保留 fallback / 双轨兼容——2026-06-03-003 schema_refs plan 明确禁止
  - 改 `run_loop_impl` 单体函数内部（参考 2026-06-03-002 R5）
  - 改 `lib.rs:80` / `lib.rs:104` 的 `pub use` 列表（v2 baseline）
  - 批量提交未跑测试就推进
  - 拆分时不显式审计 `#[cfg(test)]` 内部 hook
  - 在 `cli/` 拆分时把 `Cli` / `Commands` enum 移到子文件
  - 拆分后只跑代表性 preset 验证
  - commit 中带未解析的 `<<<<<<< HEAD` 冲突标记（commit `1335762` 真实事故）
  - `audit-file-sizes.sh` 失败时忽略
- **目标文件 baseline**（**2026-06-14 v4 @ commit dbe6f35**；v3 baseline @ 9799bf9 已入历史；v2 baseline @ 918192a 已废止；2026-06-12 v1 baseline @ 37bd281 已废止；2026-06-10 立项 baseline 已废止）：
  - `crates/ralph-core/src/event_loop/mod.rs`（**7 171 行**；v3 6 723 / v2 6 501 / v1 6 361 / 立项 5 733；v3→v4 +448 行来自 wave attribution 修复 `e695b6c` + CLI emit policy gate `7225aab`）
  - `crates/ralph-cli/src/loop_runner/tests.rs`（**11 796 行、203 个测试**；v3 11 606 行 + 201 个测试 / v2 11 015 行 + 192 个测试 / v1 10 993 行 + 193 个测试 / 立项 9 891 行 + 153 个测试；v3→v4 +190 行 / +2 个测试，主要来自 wave attribution 修复 `e695b6c` + CLI emit policy gate `7225aab`）
  - `crates/ralph-cli/src/loop_runner/runner.rs`（170 KB，本轮**不拆**）
  - `crates/ralph-core/src/config/ralph_config.rs`（3 660 行，本轮**不拆**）
  - **已存在子模块**：`crates/ralph-core/src/event_loop/review_step_state.rs`（**605 行**，commit 37bd281 引入，v1 512 → v2 605 → v3 605 不变），本计划**不动**
  - **已完成上一轮拆分（6-03-002）的子模块树**：`crates/ralph-cli/src/loop_runner/` 当前 **29 个 .rs**（含 18 个 6-03-002 已拆分子模块 + `tests.rs` + 5 个 wave 子文件 `wave/{mod,acp_mock,dispatcher,io,worker}.rs` + 4 个 hooks 子文件 `hooks/{mod,dispatch,format,mutation,retry,termination}.rs`），本计划**仅动 `tests.rs`，不动这些已拆分子模块**
  - **拆分粒度决策**（adversarial finding #17 补救）：本计划采用"中粒度"——event_loop 9 个子文件 + 1 个嵌套 mod（对应 6 个 impl 域 + 自由函数子模块），tests 19 个 `.rs`（1 mod + 1 common + 17 主题）。**候选对比**：(a) 细粒度（每个 `#[test]` 1 个文件）→ on-boarding 局部性最佳但 import 复杂度爆炸（每个测试需 5-10 个 `use` 跨 17 个子文件），且 `cli-serial` test group 串行运行时 Mutex 持有粒度变成"单 test"（PotterError 风险反而增加）；(b) 粗粒度（5 个主题块，对应 5 个 impl 域 + 5 个测试主题块）→ on-boarding 简单但 review 局部性退化（回到 6-03-002 之前的痛点）；(c) 本计划中粒度（6-03-002 已验证 18 子模块 1 个月内不出现"过度拆"或"拆得不够"反馈）→ 与 6-03-002 保持同等级粒度，便于 reviewer 跨 plan 比对
  - **前提量化**（adversarial finding #23 补救）：Problem Frame 段"阻碍可读性、code review 局部性、新人 onboarding" 三个痛点均为定性，缺少 metric。**U7 完成时本 plan 需追加一个 "Outcome Metrics" 段**（即使粗略）记录：(a) U7 commit 后 7 天内 ralph-cli + ralph-core 包的 PR 平均 review 时间（vs 6-03-002 后同期基线）；(b) 误碰无关代码导致的 revert 次数；(c) 47 个子文件（6-03-002 留下 18 + 本轮 10+19）按 R1 阈值（≤2 000 行）达标率

## Doc Review 衍生补充（adversarial finding 全部落地）

本段汇总 2026-06-12 doc-review 派生的 10 条 manual 项的最终落地位置（每条 P2 已就地修订；3 条 P1 已在 U4.5 / U6 步骤 0-2.5 嵌入）：

| # | finding | 落地位置 | 状态 |
|---|---|---|---|
| 8 (P1) | KTD12 5 域对 process_parse_result 不可证伪 | **U4.5 新增段** + 6 行归属矩阵 | ✓ |
| 9 (P1) | U6 char test 覆盖不足 | **U6 步骤 0 三步扩展**（0a mutation / 0b ≥18 test / 0c insta snapshot） | ✓ |
| 10 (P1) | U6 字节级改写护栏缺失 | **U6 步骤 2.5 硬规则**（签名预锁 / diff 0 差异 / 调用顺序字节级 / anti-pattern 联合校验） | ✓ |
| 12 (P2) | audit-file-sizes 不覆盖 event_loop 根 | R7 校正段 + U7 步骤 10 手动 awk | ✓ |
| 13 (P2) | U7 引用 14+→70+ 严重低估 | KTD9 段 + U7 Files 段 + U7 Verification 段 | ✓（已就地修订 14+→70+） |
| 15 (P2) | nextest.toml 4 Mutex 注释需更新 | **U2 step 列表新增 step 12**（追加到 U2 实施清单）：U2 完成后改 `.config/nextest.toml:2` 注释为 4 Mutex 分账说明（2 tests.rs 内 + 2 wave/acp_mock.rs） | ✓（执行期 step 已增） |
| 16 (P2) | Plan Baseline Refresh 把执行期责任写进 plan | 抽离到 `.ralph/agent/memories.md`（新建 `event-loop-split-baseline-refresh.md`，由 plan author 负责落盘）—— plan 中 8 条 bash + 5 行 awk 段标记为"详细方法论见 memories"，保留 plan 中的快照表与 baseline_head 标注 | ✓（已就地修订，记忆档案落盘为 follow-up 任务） |
| 17 (P2) | 拆分粒度 10/19 缺对比论证 | Sources & Research 末段 3 候选对比 | ✓ |
| 18 (P2) | U2/U7 拆 tests 两次节奏不优 | **本计划显式选择 U2=5 + U7=14**（不合并）的 rationale 记录在本段下：6-03-002 R6 实践表明单个 commit 涉及 >5 个新增子文件 review 面过大；U2 拆 5 个小改动覆盖 wave / hooks / fake_path 三个高复用 helper + mod 文档复制（验证 R4 + R5）；U7 拆 14 个主题子文件无 helper 改动，每子文件机械"grep + move"操作。两次拆分降低 reviewer 单次 review 的 token 消耗。 | ✓ |
| 23 (P2) | 前提量化缺失 | Sources & Research 末段"前提量化" + U7 完成后追加"Outcome Metrics"段 | ✓ |

**未采纳的 deferred questions（保留为 doc-review open issues，由 round 2 review 处理）**：

1. U2 完成后 `tests.rs` 头部的 mutex 文档是否需要在 `README.md` / `CONTRIBUTING.md` 双写？R-Refactor-4 提到"U7 时考虑双写"，但具体写到哪个文件 + 由谁落盘未定
2. R3 第二轮 follow-up 文件清单（runner.rs 170KB / ralph_config.rs 3 660 行 / commands/{run,emit}.rs 1 500-1 700 行 / hard_gate.rs 24KB / preset_lint_gate.rs 20KB）应按"下次先拆哪个"建立优先级排序（按 R1 阈值不达标程度 / 修改频次 / 业务关键性 3 维）
3. `wave/acp_mock.rs` 的 `pub static MOCK_ACP_*` 是否应降级为 `private static` 以统一风格（vs `tests.rs` 内的 `FAKE_PATH_BACKEND_*`）？本计划明确"不动"，但跨文件风格统一是否值得在 R3 第二轮一并处理
4. KTD12 5 域规则在其他 4 个 impl 域（lifecycle / termination / dispatch / diagnostics）的具体可执行性——本计划只对 process_parse_result 1 个方法做矩阵审核，4 个域是否也需要类似 pre-U5 段？

## Plan Baseline Refresh (2026-06-12)

立项后到 baseline 刷新之间（2026-06-10 → 2026-06-12），repo HEAD 推进到 `37bd281`，目标文件与若干契约出现偏差。本段记录**已就地更新的事实**与**未变化的决策**，作为读者快速对照点。

### 数字 / 事实更新（已修订）

| 项目 | 立项 (2026-06-10) | baseline @ 37bd281 v1 (2026-06-12) | **baseline @ 918192a v2 (2026-06-12)** | 影响段落 |
|---|---|---|---|---|
| `event_loop/mod.rs` 行数 | 5 733 | **6 361** (+628) | **6 501** (+140 v1→v2) | Summary, Problem Frame, HTD 图, Sources |
| `loop_runner/tests.rs` 行数 | 9 891 | **10 993** (+1 102) | **11 015** (+22 v1→v2) | Summary, Problem Frame, HTD 图, Sources |
| `loop_runner/tests.rs` 测试数 | 153 | **193** (+40) | **192** (-1 v1→v2) | Summary, Problem Frame, Verification |
| `process_parse_result` 方法行数 | 1 404 | **1 564** (+160) | **1 618** (+54 v1→v2) | KTD6, KTD12, U5, U6, HTD 图 |
| `TerminationReason` 变体数 | "17"（早期错算） | **16** | **16**（不变） | R2, KTD5, R-Refactor-1, Verification |
| `EventLoop` 字段数 | "17"（早期错算，v1 校正 13） | **13** | **14**（+1 v1→v2 校正） | R2, KTD5, R-Refactor-2, Verification |
| tests.rs 内 process-global Mutex 数 | "4"（含 wave 误算） | **2**（`FAKE_PATH_BACKEND_*`；`MOCK_ACP_*` 已迁出到 `wave/acp_mock.rs` 为 `pub static`） | **2**（不变；4 个总拓扑不变） | R5, KTD7, KTD13, U2, R-Refactor-3, Verification |
| `event_loop/tests/` 子文件数 | 30 | **40** (+10) | **40**（不变） | Problem Frame, U2, U4 |
| `lib.rs` config re-export 行号 | `:73-80` | **`:79`** | **`:80`** (+1 v1→v2) | R3, KTD2, Sources |
| `lib.rs` event_loop re-export 行号 | `:104` | **`:103`** | **`:104`** (+1 v1→v2) | R3, KTD2, Sources, Verification |
| **新出现的已存在子模块** | (无) | **`review_step_state.rs` 512 行**（commit 37bd281） | **`review_step_state.rs` 605 行**（+93 v1→v2） | Summary, U1, U5, HTD 图, Verification, Sources |
| **`loop_runner/` 已完成拆分** | 未提及 | **18 个子模块**（6-03-002 U1-U6 完成） | Problem Frame, Sources |

### 未变化的决策（不需要修订）

1. **U1→U7 顺序与 KTD12 边界规则**：completion / KTD12 5 域边界判定原则不变（lifecycle / termination / dispatch / prompt / diagnostics）。
2. **U6 抽 6 个 inline validation 层**：`origin guard / topic format / event policy / state machine / workflow guard / execution contract` 6 个 validation 层依然成立；`37bd281` 引入的 `review_step_tracker.check_semantic_gates` 是 `review_step_state` 模块内部 API，**不属于** U6 抽取的 6 个之一（语义上更接近 hat-lifecycle gate，由 review_step_state 自管）。
3. **R6 零回归原则**：每 U 独立 commit + 全套 nextest 验证；失败 `git revert` 单 commit。
4. **R3 公开 API 锁定**：`lib.rs` 公开 API 列表禁止修改的承诺不变，仅行号范围更新。
5. **Scope Boundaries 范围内 / 范围外清单**：除 baseline 行数修正外，"哪些拆 / 哪些不拆"的决策不变。

### Baseline 刷新方法论

执行步骤可复用于未来的同类基线刷新：

```bash
# 1. 行数 baseline
wc -l crates/ralph-core/src/event_loop/mod.rs crates/ralph-cli/src/loop_runner/tests.rs

# 2. 数据结构 baseline
awk '/^pub enum TerminationReason/{f=1;next} f && /^}/{exit} f && /^    [A-Z]/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 变体数
awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} f && /^    pub[ (]/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 字段数

# 3. 关键方法行号
grep -nE "^impl EventLoop|fn process_parse_result|fn (apply_workflow_guard|apply_event_policy|finding_to_payload_contract|extract_correlation_key|format_duration|termination_status_text)" crates/ralph-core/src/event_loop/mod.rs

# 4. Mutex 拓扑
grep -rnE "^(static|pub static).*LazyLock<Mutex" crates/ralph-cli/src/loop_runner/

# 5. lib.rs re-export 行号
grep -nE "^pub use (config|event_loop)::" crates/ralph-core/src/lib.rs

# 6. event_loop/tests/ 子文件数
ls crates/ralph-core/src/event_loop/tests/ | wc -l

# 7. loop_runner/ 已拆分子模块
ls crates/ralph-cli/src/loop_runner/

# 8. 测试总数
grep -cE "#\[test\]|#\[tokio::test" crates/ralph-cli/src/loop_runner/tests.rs
```

每次执行 U1 之前应重跑上述 8 条命令，并将本表 `baseline @ 37bd281 (2026-06-12)` 列替换为新 baseline（**当前已替换为 v2 baseline @ 918192a，详见下一段**）。

---

## Repo Drift Note

（U3-U6 每 U 完成时追加 Sub-Note；U7 合并为完整表格。模板参考 `docs/plans/2026-06-03-003-refactor-schema-refs-replace-regex-plan.md` 的 "Repo Drift Note" 段。）


## Plan Baseline Refresh v2 (2026-06-12, baseline @ 918192a)

v1 baseline refresh 落地后到本次 v2 之间，repo HEAD 推进到 `918192a`，目标文件与若干契约再次出现偏差。本段记录**v2 已就地更新的事实**与**v1→v2 期间增量 commits 的影响**，与原 "Plan Baseline Refresh (2026-06-12)" 段并列。

### v1→v2 期间增量 commits (37bd281..918192a, 7 commits)

| commit | type | 影响 | 关联 baseline 数字 |
|---|---|---|---|
| `4c244ea` | test(fixtures) | session_b policy fixture 归类到 policy_schemas/ | 无直接影响（fixtures 目录） |
| `4029be3` | docs(plan) | 本 plan v1 baseline refresh 自身 | (本 commit) |
| `0d79743` | docs | 整理 2026-06-12 ce-executor-isolated 闭环缺口 | 文档同步（70+ 引用工作量） |
| `84b8281` / `3d4fc21` | feat(workflow-contract) | 落地 WAC 静态规则 + 运行时握手索引/追踪器 | `mod.rs` +140 行（`EventLoop` 14 字段）、`review_step_state.rs` +93 行（605）、`tests.rs` +22 行（11 015） |
| `bf765c1` | docs(wac) | 003 rollout 完成计划 + tiered gates 方案文档 | 文档同步 |
| `918192a` | chore(ralph-cli) | 清理 WAC 002/003 预留 API 的 dead_code 警告 | `tests.rs` -1 测试（192 个，193→192）|

### v1→v2 数字 / 事实更新（已就地修订）

| 项目 | v1 @ 37bd281 | **v2 @ 918192a** | 漂移 | 影响段落 |
|---|---|---|---|---|
| `event_loop/mod.rs` 行数 | 6 361 | **6 501** | +140 | Summary, Problem Frame, HTD 图, Sources |
| `loop_runner/tests.rs` 行数 | 10 993 | **11 015** | +22 | Summary, Problem Frame, HTD 图, Sources |
| `loop_runner/tests.rs` 测试数 | 193 | **192** | -1 | Summary, Problem Frame, Verification, Sources |
| `process_parse_result` 方法行 | 1 564 | **1 618** | +54 | KTD5, KTD6, KTD12, U5, U6, HTD 图, Sources |
| `process_parse_result` 起始行号 | (未明确) | **mod.rs:4530** | - | U3-U6 引用 |
| `EventLoop` 字段数 | 13（早期错算） | **14** | +1 | R2, KTD5, KTD13, R-Refactor-2, Verification |
| `extract_correlation_key` 行号 | 282 | **288** | +6 | U3, U4 |
| `apply_workflow_guard_validation` 行号 | 365 | **371** | +6 | U3, U4 |
| `apply_event_policy_validation` 行号 | 545 | **551** | +6 | U3, U4 |
| `finding_to_payload_contract_violation` 行号 | 851 | **857** | +6 | U3, U4 |
| `impl EventLoop` 起始行号 | 920 | **926** | +6 | U5, KTD5 |
| `impl EventLoop` 主块结束行 | 6 304 | **6 448** | +144 | U5, HTD 图 |
| `format_duration` 行号 | 6 318 | **6 458** | +140 | U5, HTD 图 |
| `termination_status_text` 行号 | 6 334 | **6 474** | +140 | U5, HTD 图 |
| mod.rs 总行数（end） | 6 361 | **6 501** | +140 | Summary, Sources, R-Refactor-1, HTD 图 |
| `review_step_state.rs` 行数 | 512 | **605** | +93 | Summary, U1, U5, HTD 图, Sources |
| `lib.rs` config re-export 行号 | 79 | **80** | +1 | R3, KTD2, Sources, Verification |
| `lib.rs` event_loop re-export 行号 | 103 | **104** | +1 | R3, KTD2, Sources, Verification |
| types 段 末行 | mod.rs:281 | **mod.rs:287** | +6 | U3 (types 段边界) |
| workflow_guard 段 末行 | mod.rs:544 | **mod.rs:550** | +6 | U4 (workflow_guard 段边界) |
| policy 段 末行 | mod.rs:918 | **mod.rs:925** | +7 | U4 (policy 段边界) |

### v1→v2 期间未变化的契约

1. **Mutex 拓扑（4 个）**：tests.rs 内 2 个 `FAKE_PATH_BACKEND_*` (private `static`) + acp_mock.rs 内 2 个 `MOCK_ACP_*` (`pub static`)，行号 +0（Mutex 段未受 WAC 影响）。
2. **TerminationReason 16 变体顺序**：v1→v2 期间变体集合未变（实测 list 与 v1 完全一致）。
3. **KTD6 U1→U7 风险递增顺序**：不变。
4. **KTD12 5 域边界规则**：不变。
5. **R6 零回归原则**：不变。
6. **6 个 inline validation 层**（origin guard / topic format / event policy / state machine / workflow guard / execution contract）：不变。
7. **Scope Boundaries 范围内 / 范围外清单**：不变。
8. **2 个主文件 R1 阈值（≤ 1 000 行）**：不变。

### v2 baseline refresh 决策树

- **若 U1 启动前 repo 又推进 N commits**：重跑 8 条 baseline 命令（见 v1 段），按 v2 模板追加新一列（v3 / v4 ...）；v2 本段不删（作为历史）。
- **若 v2 之后 `EventLoop` 字段数再次变化**：在 v2 表格中追加行 + 注明增量字段名 + commit hash；不删除旧行。
- **若 v2 之后 `process_parse_result` 行数再次变化**：KTD5 / KTD6 / U5 / U6 / Verification 段同步更新（已在 v2 段就地修订）。
- **若 v2 之后 6 个 inline validation 层有变化**（如新增 / 删除 / 合并）：U4.5 矩阵 + U6 步骤 0 同步重写，并在 v2 段追加"validation 层增量"行。

### v2 重跑命令（与 v1 段 8 条等价，对应更新后的字段数 / 行号）

```bash
# 1. 行数 baseline
wc -l crates/ralph-core/src/event_loop/{mod,review_step_state}.rs crates/ralph-cli/src/loop_runner/tests.rs

# 2. 数据结构 baseline
awk '/^pub enum TerminationReason/{f=1;next} f && /^}/{exit} f && /^    [A-Z]/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 16
awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs        # 14

# 3. 关键方法行号
grep -nE "^impl EventLoop|fn process_parse_result|^fn (extract_correlation_key|apply_workflow_guard|apply_event_policy|finding_to_payload_contract|format_duration|termination_status_text)" crates/ralph-core/src/event_loop/mod.rs

# 4. Mutex 拓扑
grep -nE "^(static|pub static) (FAKE_PATH|MOCK_ACP)" crates/ralph-cli/src/loop_runner/{tests.rs,wave/acp_mock.rs}

# 5. lib.rs re-export 行号
grep -nE "^pub use (config|event_loop)::" crates/ralph-core/src/lib.rs   # 80 / 104

# 6. event_loop/tests/ 子文件数
ls crates/ralph-core/src/event_loop/tests/*.rs | wc -l   # 40

# 7. loop_runner/ 已拆分子模块
ls crates/ralph-cli/src/loop_runner/*.rs crates/ralph-cli/src/loop_runner/*/*.rs   # 29 个 .rs

# 8. 测试总数
awk 'BEGIN{c=0} /^#\[test\]/{c++} /^#\[tokio::test/{c++} END{print c}' crates/ralph-cli/src/loop_runner/tests.rs   # 192
```

## Repo Drift Sub-Note v2 (2026-06-12)

**v1→v2 baseline refresh 阶段无实施 commit**（仅 docs/plans/2026-06-10-003 自身），故本 sub-note 只记录"行号 / 字段数 / 测试数漂移"而无"哪些 commit 落地"。

- `event_loop/mod.rs` 总行数漂移 +140：`cargo wc -l` 显示 6 361 → 6 501，差异主要来自 WAC（workflow-contract）静态规则 + 运行时握手索引/追踪器（commits `84b8281` / `3d4fc21`）+ ce-executor-isolated 闭环增量。**未触及** R3 公开 API / KTD7 Mutex 拓扑 / KTD12 5 域边界。
- `loop_runner/tests.rs` 总行数漂移 +22 / 测试数 -1：WAC `918192a` 清理 dead code 同时移除 1 个失效 test；Mutex 段（605-610 行 `FAKE_PATH_BACKEND_*`）未受影响。
- `process_parse_result` 行数 +54 (1 564 → 1 618)：WAC 新增的 `workflow_contract` 验证逻辑在 process_parse_result 内串联了 6 个 validation 层中的 workflow guard 路径（仍属 U6 待抽的 6 个 inline validation 之一）；不改变 KTD6 风险递增顺序。
- `EventLoop` 字段 +1 (13 → 14)：v1 doc-review 校正时仍写 13 是因为 adversarial reviewer 只对比了 v0→v1 期间（5 733→6 361）的字段增量；v2 实测 `hat_lifecycle_tracker: ActivationLifecycleTracker<SystemTimeClock>` 在 v0→v2 期间被加入（具体 commit hash 待 grep `git log -S"hat_lifecycle_tracker"` 进一步锁定，但与 v1→v2 期间 commit 不直接绑定——可能是更早 commit 的字段被 doc-review 当时误算）。
- `lib.rs:79→80 / 103→104` +1 行偏移：来自 `cargo fmt` 触发的行重排（2026-06-12 期间 WAC 落地可能引入一个 `use` 排序调整）；**不**影响 `pub use config::{...}` / `pub use event_loop::{...}` 列表本身（验证：`git diff v1..v2 -- crates/ralph-core/src/lib.rs | grep -E "^\+.*pub use" | head -10` 仅显示已有列表 +1 行的 use re-order）。
- 漂移引用清单（U7 时再处理）：本阶段 docs 引用本身未受影响（WAC 落地于 `ralph-cli` 包 + `workflow_contract` 子模块，**未触及** `event_loop::*` / `loop_runner::*` 公开 API），故"70+ 引用文件"在 v2 baseline 下仍为 70+（无新增）；仅在 plan 内部行号 / 字段数 / 测试数 / Mutex 段已被就地修订。

**v2 baseline 引用面**（grep 命中数，与 v1 一致或更少）：

```
git grep -nE "event_loop/mod\.rs\b" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null | wc -l   # ~252 (v1 基线)
git grep -nE "loop_runner/tests\.rs\b" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null | wc -l   # ~68 (v1 基线)
```

（U3-U7 实施时如发现实际数字与 v1 不一致，按 U7 step 13 流程补充处理。）

---

## Plan Baseline Refresh v3 (2026-06-13, baseline @ 9799bf9)

v2 baseline refresh 落地后到本次 v3 之间，repo HEAD 推进到 `9799bf9`，目标文件与若干契约再次出现偏差。本段记录**v3 已就地更新的事实**与**v2→v3 期间增量 commits 的影响**，与 v1 / v2 段并列。

### v2→v3 期间增量 commits (918192a..9799bf9, 8 commits)

| commit | type | 影响 | 关联 baseline 数字 |
|---|---|---|---|
| `4ec5ca6` | chore(presets) | 移除已迁移的 4 个内置 preset 模板（code-assist / research / review / pdd-to-code-assist） | 无直接影响（preset 模板） |
| `d13367d` | docs(wave-policy-gate) | 落地 wave 派发与 policy gate 脱节的闭环文档 | 文档同步 |
| `eae5baa` | fix(event-loop) | surface wave policy rejections to runner（U1：event_loop 把 wave 拒绝转 diagnostics + 透传） | `mod.rs` / `tests.rs` 增量基线 |
| `62b9275` | feat(wave-cli) | schema precheck + agent-native JSON errors（`policy_check.rs` 抽离 + wave emit 预检） | 间接影响 `tests.rs`（wave 测试族扩面） |
| `2fbe063` | chore(presets) | strict CLI policy check + depth example | preset 配置层 |
| `a722fe0` | fix(runner) | merge wave policy rejections into candidate_topics（U2：runner 合并拒绝主题到候选） | `tests.rs` +316 行（hard_gate 闭包测试族）；`lib.rs` +1 行 `PolicyRejection` re-export |
| `6eafe79` | fix(event-loop) | pin next hat to gated hat after recovery（U3：next_hat take-semantics 消费 pending_recovery_hat） | `mod.rs` +27 行；`tests.rs` +265 行 |
| `9799bf9` | fix(event-loop) | 增强 wave 策略拒绝与待恢复 hat 的可观测性（batch 聚合 / tracing warn 回落 / `ProcessedEvents::Default` 实现） | `mod.rs` +91 行；`tests.rs` +32 行 |

### v2→v3 数字 / 事实更新（已就地修订）

| 项目 | v2 @ 918192a | **v3 @ 9799bf9** | 漂移 | 影响段落 |
|---|---|---|---|---|
| `event_loop/mod.rs` 行数 | 6 501 | **6 723** | +222 | Summary, Problem Frame, HTD 图, U3-U5, Sources |
| `loop_runner/tests.rs` 行数 | 11 015 | **11 606** | +591 | Summary, Problem Frame, HTD 图, U7, Sources |
| `loop_runner/tests.rs` 测试数 | 192 | **201** | +9 | Summary, Problem Frame, Verification, Sources |
| `EventLoop` 字段数 | 14 | **14** | 0 | R2, KTD5, KTD13, R-Refactor-2, Verification |
| `EventLoop` 起始行号 | 264 | **264**（不变） | 0 | R-Refactor-2 awk 校验 |
| `EventLoop` 字段顺序漂移 | (无) | **0**（不变） | 0 | U5 / R-Refactor-2 |
| `TerminationReason` 变体数 | 16 | **16** | 0 | R2, KTD5, R-Refactor-1, Verification |
| `TerminationReason` 起始行号 | 129 | **129**（不变） | 0 | U3 / R-Refactor-1 |
| `TerminationReason` 末行 | 192 | **192**（不变） | 0 | U3 / types 段边界 |
| `extract_correlation_key` 行号 | 288 | **322** | +34 | U3, U4 |
| `apply_workflow_guard_validation` 行号 | 371 | **405** | +34 | U3, U4 |
| `apply_event_policy_validation` 行号 | 551 | **585** | +34 | U3, U4 |
| `finding_to_payload_contract_violation` 行号 | 857 | **891** | +34 | U3, U4 |
| `workflow_guard` 段末行 | 550 | **584** | +34 | U4 (workflow_guard 段边界) |
| `policy` 段末行 | 925 | **959** | +34 | U4 (policy 段边界) |
| `impl EventLoop` 起始行号 | 926 | **960** | +34 | U5, KTD5 |
| `impl EventLoop` 主块结束行 | 6 448 | **6 666** | +218 | U5, HTD 图 |
| `impl EventLoop` 方法数 | 117 | **118** | +1 | U5, R-Refactor-2, Verification |
| `process_parse_result` 起始行 | 4 530 | **4 715** | +185 | U3-U6 引用 |
| `process_parse_result` 结束行 | 6 147 | **6 332** | +185 | U5, U6 |
| `process_parse_result` 行数 | 1 618 | **1 617** | -1 | KTD5, KTD6, KTD12, U5, U6, HTD 图, Sources |
| `format_duration` 行号 | 6 458 | **6 680** | +222 | U5, HTD 图 |
| `termination_status_text` 行号 | 6 474 | **6 696** | +222 | U5, HTD 图 |
| mod.rs 总行数（end） | 6 501 | **6 723** | +222 | Summary, Sources, R-Refactor-1, HTD 图 |
| `review_step_state.rs` 行数 | 605 | **605** | 0 | Summary, U1, U5, HTD 图, Sources |
| `event_loop/tests/` 子文件数 | 40 | **41** | +1 | Problem Frame, U2, U4, U7 |
| 新增 `event_loop/tests/` 子文件 | (无) | **`wave_policy_rejection.rs`**（commit `9799bf9` 配套，~150 行，覆盖 7 合规 / 7 缺 depth 两组 UAT） | (新) | U2, U7, Sources |
| `loop_runner/` `.rs` 总数 | 29 | **29**（不变） | 0 | Problem Frame, Sources |
| `lib.rs:80` config re-export 行号 | 80 | **80**（不变） | 0 | R3, KTD2, Sources, Verification |
| `lib.rs:104` event_loop re-export 行号 | 104 | **104**（不变） | 0 | R3, KTD2, Sources, Verification |
| `lib.rs` `use` 重排 | (无) | commit `a722fe0` 调整 1 个 `use`（`PolicyRejection` re-export） | (新) | R3 注释（已在 R3 段补充） |

### v2→v3 期间未变化的契约

1. **Mutex 拓扑（4 个）**：tests.rs 内 2 个 `FAKE_PATH_BACKEND_*` (private `static`，行号 605 / 609) + acp_mock.rs 内 2 个 `MOCK_ACP_*` (`pub static`，行号 97 / 102)，行号 +0（Mutex 段未受 wave policy gate 闭环影响）。
2. **`MOCK_ACP_*` Mutex 段 `pub static` 形式不变**：v2→v3 期间 `wave/acp_mock.rs` 0 行变更（`git diff 918192a..9799bf9 -- crates/ralph-cli/src/loop_runner/wave/acp_mock.rs` 实测为空）。
3. **TerminationReason 16 变体顺序**：v2→v3 期间变体集合未变（实测 list 与 v2 完全一致）。
4. **KTD6 U1→U7 风险递增顺序**：不变。
5. **KTD12 5 域边界规则**：不变。
6. **R6 零回归原则**：不变。
7. **6 个 inline validation 层**（origin guard / topic format / event policy / state machine / workflow guard / execution contract）：不变。
8. **Scope Boundaries 范围内 / 范围外清单**：不变。
9. **2 个主文件 R1 阈值（≤ 1 000 行）**：不变。
10. **`event_loop::tests::replay_light_integration.rs` 等 41 个 `tests/` 子文件**：v2→v3 期间除新增 1 个 `wave_policy_rejection.rs` 外其余 40 个**不动**。

### v3 baseline refresh 决策树

- **若 U1 启动前 repo 又推进 N commits**：重跑 8 条 baseline 命令（见 v1 段），按 v2 / v3 模板追加新一列（v4 / v5 ...）；v3 本段不删（作为历史）。
- **若 v3 之后 `EventLoop` 字段数再次变化**：在 v3 表格中追加行 + 注明增量字段名 + commit hash；不删除旧行。
- **若 v3 之后 `process_parse_result` 行数再次变化**：KTD5 / KTD6 / U5 / U6 / Verification 段同步更新（已在 v3 段就地修订）。
- **若 v3 之后 6 个 inline validation 层有变化**（如新增 / 删除 / 合并）：U4.5 矩阵 + U6 步骤 0 同步重写，并在 v3 段追加"validation 层增量"行。
- **若 v3 之后 `lib.rs:80` / `lib.rs:104` 行号漂移**：v3 表格追加行；与 R3 公开 API 列表本身是否变化需在 commit message 单独标注（v3 已发生 1 次 `use` 重排，但列表本身不变）。

### v3 重跑命令（与 v1 / v2 段 8 条等价，对应更新后的字段数 / 行号）

```bash
# 1. 行数 baseline
wc -l crates/ralph-core/src/event_loop/{mod,review_step_state}.rs crates/ralph-cli/src/loop_runner/tests.rs
# v3: mod.rs 6723 / review_step_state 605 / tests.rs 11606

# 2. 数据结构 baseline
awk '/^pub enum TerminationReason/{f=1;next} f && /^}/{exit} f && /^    [A-Z]/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 16
awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 14

# 3. 关键方法行号
grep -nE "^impl EventLoop|fn process_parse_result|^fn (extract_correlation_key|apply_workflow_guard|apply_event_policy|finding_to_payload_contract|format_duration|termination_status_text)" crates/ralph-core/src/event_loop/mod.rs
# v3: 322 / 405 / 585 / 891 / impl 960 / process_parse_result 4715 / format_duration 6680 / termination_status_text 6696

# 4. Mutex 拓扑
grep -nE "^(static|pub static) (FAKE_PATH|MOCK_ACP)" crates/ralph-cli/src/loop_runner/tests.rs crates/ralph-cli/src/loop_runner/wave/acp_mock.rs
# v3: tests.rs:605/609 + acp_mock.rs:97/102 (4 个 Mutex，v2 拓扑未变)

# 5. lib.rs re-export 行号
grep -nE "^pub use (config|event_loop)::" crates/ralph-core/src/lib.rs
# v3: 80 / 104 (不变)

# 6. event_loop/tests/ 子文件数
ls crates/ralph-core/src/event_loop/tests/*.rs | wc -l
# v3: 41 (新增 wave_policy_rejection.rs)

# 7. loop_runner/ 已拆分子模块
ls crates/ralph-cli/src/loop_runner/*.rs crates/ralph-cli/src/loop_runner/*/*.rs | wc -l
# v3: 29 (不变)

# 8. 测试总数
awk 'BEGIN{c=0} /^#\[test\]/{c++} /^#\[tokio::test/{c++} END{print c}' crates/ralph-cli/src/loop_runner/tests.rs
# v3: 201
```

## Repo Drift Sub-Note v3 (2026-06-13)

**v2→v3 baseline refresh 阶段无实施 commit**（仅 docs/plans/2026-06-10-003 自身），故本 sub-note 只记录"行号 / 字段数 / 测试数漂移"而无"哪些 commit 落地"。

- `event_loop/mod.rs` 总行数漂移 +222：实测 6 501 → 6 723，差异主要来自 wave policy gate 闭环 commits（`eae5baa` +11 / `a722fe0` 间接 +0 / `6eafe79` +27 / `9799bf9` +91 等累计），新增逻辑集中在：(a) `next_hat` take-semantics 与 `pending_recovery_hat` 协调（`6eafe79`）；(b) `process_parse_result` 内 wave partition policy 拒绝 batch 聚合 + `ProcessedEvents::Default` 实现（`9799bf9`）；(c) event policy 拒绝诊断信息从单条扩为多拒绝聚合（`9799bf9`）。**未触及** R3 公开 API / KTD7 Mutex 拓扑 / KTD12 5 域边界。
- `loop_runner/tests.rs` 总行数漂移 +591 / 测试数 +9：实测 11 015 → 11 606 / 192 → 201。`a722fe0` +316 / `6eafe79` +265 / `9799bf9` +32 等累计。Mutex 段（605-610 行 `FAKE_PATH_BACKEND_*`）未受影响。
- `process_parse_result` 行数 -1 (1 618 → 1 617)：仅末尾闭合格式微调（行 6 332 结束而非 6 147，闭括号位置因 v3 期间其他函数插入发生 1 行位移），U6 待抽的 6 个 inline validation 层结构与顺序未变；不改变 KTD6 风险递增顺序。
- `EventLoop` 字段 +0 (14 → 14)：v2→v3 期间未新增字段；新增 1 个方法（`impl EventLoop` 方法数 117 → 118），最可能落在 `next_hat` 或 `process_parse_result` 关联的 helper。字段顺序漂移 0（R-Refactor-2 未触发）。
- `lib.rs:80 / 104` 行号不变：commit `a722fe0` 调整了 1 个 `use` 重排（引入 `PolicyRejection` re-export），但 `pub use config::{...}` / `pub use event_loop::{...}` 公开 API 列表本身在 80 / 104 行，位置未漂移（验证：`git diff 918192a..9799bf9 -- crates/ralph-core/src/lib.rs | grep -E "^\+.*pub use" | head -10` 仅显示已有列表 +1 行的 use re-order + `PolicyRejection` 新增项）。
- `event_loop/tests/` 子文件 +1 (40 → 41)：v3 期间新增 `wave_policy_rejection.rs`（commit `9799bf9` 配套，~150 行，覆盖 7 合规 / 7 缺 depth 两组 UAT），其余 40 个子文件**不动**。
- 漂移引用清单（U7 时再处理）：本阶段 docs 引用面维持 70+（实测 `git grep -nE "event_loop/mod\.rs\b" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null | wc -l` ≈ 252，`loop_runner/tests\.rs\b` ≈ 68，与 v2 baseline 持平；v2→v3 期间 8 个 commits 未触及 `event_loop::*` / `loop_runner::*` 公开 API 名变更）。仅在 plan 内部行号 / 字段数 / 测试数 / Mutex 段已被就地修订。

**v3 baseline 引用面**（grep 命中数，与 v2 持平）：

```
git grep -nE "event_loop/mod\.rs\b" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null | wc -l   # ~252 (v2 基线一致)
git grep -nE "loop_runner/tests\.rs\b" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null | wc -l   # ~68 (v2 基线一致)
```

（U3-U7 实施时如发现实际数字与 v3 不一致，按 U7 step 13 流程补充处理。）

## Plan Baseline Refresh v4 (2026-06-14, baseline @ dbe6f35)

v3 baseline refresh 落地后到本次 v4 之间，repo HEAD 推进到 `dbe6f35`，目标文件与若干契约再次出现偏差；同时 **U1 scaffold 已在工作树分支 `ralph/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-lucky-reed` 落地为 commit `464b4d6`**。本段记录**v4 已就地更新的事实**、**v3→v4 期间增量 commits 的影响**，以及 **U1 scaffold 的当前状态**。

### v3→v4 期间增量 commits (9799bf9..dbe6bf9, 3 commits)

| commit | type | 影响 | 关联 baseline 数字 |
|---|---|---|---|
| `e695b6c` | fix(wave-attribution) | 关闭 ce-executor-isolated wave 8/8 完成后 5 个 P0 攻击面；新增 `guidance_dedup.rs` / `incident_fixture.rs` / `recovery_envelope_u7_u8.rs` 三个 event_loop/tests 子文件 | `mod.rs` +470 行 / `tests.rs` +188 行 / `event_loop/tests/` +3 个子文件 |
| `7225aab` | fix(cli) | ralph emit 强制 preset event_policy + loop termination 向父 loop 传播 | `tests.rs` +94 行 / `mod.rs` +4 行；新增 `integration_emit_policy.rs` 等集成测试 |
| `dbe6f35` | fix(review) | 同步 CLAUDE.md/AGENTS.md、补缺失测试、覆盖 CliEmit diagnosis variant | 不直接影响目标文件；触发 AGENTS.md ↔ CLAUDE.md 同步检查 |

### U1 scaffold 状态（已落地，未合并）

- **分支**：`ralph/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-lucky-reed`
- **Commit**：`464b4d6 refactor(event_loop): U1 scaffold — 10 placeholder submodules`
- **内容**：已在 `crates/ralph-core/src/event_loop/` 创建 10 个 placeholder 子文件（`types.rs` / `workflow_guard.rs` / `policy.rs` / `lifecycle.rs` / `termination.rs` / `dispatch.rs` / `prompt.rs` / `diagnostics.rs` / `process.rs` / `wave.rs`），并在 `mod.rs` 顶部追加 10 个 `mod xxx;` 声明 + 相应 `pub use` 占位。
- **与 v4 baseline 的关系**：该 scaffold 基于 `e695b6c` 之前的 HEAD（实际 parent 为 `e695b6c`），尚未吸收 `7225aab` / `dbe6f35` 的增量。U2-U7 实施前需要：
  1. 在分支上 rebase 到 `dbe6f35`；
  2. 按本 plan v4 段更新所有行号 / 方法数 / 测试数；
  3. 重新跑 U1 验证命令（`cargo build -p ralph-core` / `cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast`）。
- **注意**：U1 scaffold 中创建了 `process.rs` 占位文件（与早期 plan "不新增 `process.rs`" 略有调整），文件内已注明 U6 将填充 6 个 `validate_*` 自由函数。U6 实施时可选择保留 `process.rs` 并在其中实现，或把内容迁到 `prompt.rs` 内的嵌套 `pub(crate) mod process`；无论哪种方式，必须保证 `mod.rs` 的 `pub use process::*;` 路径不变。

### v3→v4 数字 / 事实更新（已就地修订）

| 项目 | v3 @ 9799bf9 | **v4 @ dbe6f35** | 漂移 | 影响段落 |
|---|---|---|---|---|
| `event_loop/mod.rs` 行数 | 6 723 | **7 171** | +448 | Summary, Problem Frame, HTD 图, U3-U5, Sources |
| `loop_runner/tests.rs` 行数 | 11 606 | **11 796** | +190 | Summary, Problem Frame, HTD 图, U7, Sources |
| `loop_runner/tests.rs` 测试数 | 201 | **203** | +2 | Summary, Problem Frame, Verification, Sources |
| `EventLoop` 字段数 | 14 | **14** | 0 | R2, KTD5, KTD13, R-Refactor-2, Verification |
| `EventLoop` 起始行号 | 264 | **264**（不变） | 0 | R-Refactor-2 awk 校验 |
| `TerminationReason` 变体数 | 16 | **16** | 0 | R2, KTD5, R-Refactor-1, Verification |
| `TerminationReason` 起始行号 | 129 | **129**（不变） | 0 | U3 / R-Refactor-1 |
| `extract_correlation_key` 行号 | 322 | **324** | +2 | U3, U4 |
| `apply_workflow_guard_validation` 行号 | 405 | **407** | +2 | U3, U4 |
| `apply_event_policy_validation` 行号 | 585 | **587** | +2 | U3, U4 |
| `finding_to_payload_contract_violation` 行号 | 891 | **893** | +2 | U3, U4 |
| `impl EventLoop` 起始行号 | 960 | **962** | +2 | U5, KTD5 |
| `impl EventLoop` 主块结束行 | 6 666 | **7 114** | +448 | U5, HTD 图 |
| `impl EventLoop` 方法数 | 118 | **120** | +2 | U5, R-Refactor-2, Verification |
| `process_parse_result` 起始行 | 4 715 | **4 921** | +206 | U3-U6 引用 |
| `process_parse_result` 结束行 | 6 332 | **6 780** | +448 | U5, U6 |
| `process_parse_result` 行数 | 1 617 | **1 860** | +243 | KTD5, KTD6, KTD12, U5, U6, HTD 图, Sources |
| `format_duration` 行号 | 6 680 | **7 128** | +448 | U5, HTD 图 |
| `termination_status_text` 行号 | 6 696 | **7 144** | +448 | U5, HTD 图 |
| mod.rs 总行数（end） | 6 723 | **7 171** | +448 | Summary, Sources, R-Refactor-1, HTD 图 |
| `review_step_state.rs` 行数 | 605 | **605** | 0 | Summary, U1, U5, HTD 图, Sources |
| `event_loop/tests/` 子文件数 | 41 | **44** | +3 | Problem Frame, U2, U4, U7 |
| 新增 `event_loop/tests/` 子文件 | `wave_policy_rejection.rs` | **`guidance_dedup.rs`** / **`incident_fixture.rs`** / **`recovery_envelope_u7_u8.rs`**（commit `e695b6c` 配套） | (新) | U2, U7, Sources |
| `loop_runner/` `.rs` 总数 | 29 | **29**（不变） | 0 | Problem Frame, Sources |
| `lib.rs:80` config re-export 行号 | 80 | **80**（不变） | 0 | R3, KTD2, Sources, Verification |
| `lib.rs:104` event_loop re-export 行号 | 104 | **104**（不变） | 0 | R3, KTD2, Sources, Verification |
| `lib.rs` `use` 重排 | commit `a722fe0` 调整 1 个 `use`（`PolicyRejection` re-export） | 无新增重排 | 0 | R3 注释 |

### v3→v4 期间未变化的契约

1. **Mutex 拓扑（4 个）**：tests.rs 内 2 个 `FAKE_PATH_BACKEND_*` (private `static`，行号 605 / 609) + acp_mock.rs 内 2 个 `MOCK_ACP_*` (`pub static`，行号 97 / 102)，行号 +0（Mutex 段未受 wave attribution / CLI emit policy 影响）。
2. **`MOCK_ACP_*` Mutex 段 `pub static` 形式不变**：v3→v4 期间 `wave/acp_mock.rs` 0 行变更。
3. **TerminationReason 16 变体顺序**：v3→v4 期间变体集合未变。
4. **EventLoop 14 字段顺序**：v3→v4 期间字段集合与顺序未变。
5. **KTD6 U1→U7 风险递增顺序**：不变；但 U1 已在分支 scaffold 落地，后续 U2-U7 需在 rebased 分支上继续。
6. **KTD12 5 域边界规则**：不变。
7. **R6 零回归原则**：不变。
8. **6 个 inline validation 层**（origin guard / topic format / event policy / state machine / workflow guard / execution contract）：不变；但 `process_parse_result` 总长度 +243 行，U6 抽取范围相应扩大。
9. **Scope Boundaries 范围内 / 范围外清单**：不变。
10. **2 个主文件 R1 阈值（≤ 1 000 行）**：不变。
11. **`event_loop::tests/` 新增 3 个子文件不动**：`guidance_dedup.rs` / `incident_fixture.rs` / `recovery_envelope_u7_u8.rs` 作为已有测试子文件，本轮**只迁移其 `mod.rs` 声明路径（如有必要）**，不修改测试体。

### v4 baseline refresh 决策树

- **若 U1 分支 rebase 前 repo 又推进 N commits**：重跑 8 条 baseline 命令（见 v1 / v2 / v3 段），按 v4 模板追加新一列（v5 / v6 ...）；v4 本段不删（作为历史）。
- **若 v4 之后 `EventLoop` 字段数再次变化**：在 v4 表格中追加行 + 注明增量字段名 + commit hash；不删除旧行。
- **若 v4 之后 `process_parse_result` 行数再次变化**：KTD5 / KTD6 / U5 / U6 / Verification 段同步更新（v4 段已就地修订）。
- **若 v4 之后 6 个 inline validation 层有变化**（如新增 / 删除 / 合并）：U4.5 矩阵 + U6 步骤 0 同步重写，并在 v4 段追加"validation 层增量"行。
- **若 v4 之后 `lib.rs:80` / `lib.rs:104` 行号漂移**：v4 表格追加行；公开 API 列表本身是否变化需在 commit message 单独标注。
- **U1 scaffold 已落地但未合并**：任何 v4 之后的 baseline 刷新都需同时检查主分支 HEAD 与 scaffold 分支 rebased 后的 HEAD，确保两者数字一致后再进入 U2。

### v4 重跑命令（与 v1 / v2 / v3 段 8 条等价，对应更新后的字段数 / 行号）

```bash
# 1. 行数 baseline
wc -l crates/ralph-core/src/event_loop/{mod,review_step_state}.rs crates/ralph-cli/src/loop_runner/tests.rs
# v4: mod.rs 7171 / review_step_state 605 / tests.rs 11796

# 2. 数据结构 baseline
awk '/^pub enum TerminationReason/{f=1;next} f && /^}/{exit} f && /^    [A-Z]/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 16
awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 14

# 3. 关键方法行号
grep -nE "^impl EventLoop|fn process_parse_result|^fn (extract_correlation_key|apply_workflow_guard|apply_event_policy|finding_to_payload_contract|format_duration|termination_status_text)" crates/ralph-core/src/event_loop/mod.rs
# v4: 324 / 407 / 587 / 893 / impl 962 / process_parse_result 4921 / format_duration 7128 / termination_status_text 7144

# 4. Mutex 拓扑
grep -nE "^(static|pub static) (FAKE_PATH|MOCK_ACP)" crates/ralph-cli/src/loop_runner/tests.rs crates/ralph-cli/src/loop_runner/wave/acp_mock.rs
# v4: tests.rs:605/609 + acp_mock.rs:97/102 (4 个 Mutex，v3 拓扑未变)

# 5. lib.rs re-export 行号
grep -nE "^pub use (config|event_loop)::" crates/ralph-core/src/lib.rs
# v4: 80 / 104 (不变)

# 6. event_loop/tests/ 子文件数
ls crates/ralph-core/src/event_loop/tests/*.rs | wc -l
# v4: 44 (新增 guidance_dedup.rs / incident_fixture.rs / recovery_envelope_u7_u8.rs)

# 7. loop_runner/ 已拆分子模块
ls crates/ralph-cli/src/loop_runner/*.rs crates/ralph-cli/src/loop_runner/*/*.rs | wc -l
# v4: 29 (不变)

# 8. 测试总数
awk 'BEGIN{c=0} /^#\[test\]/{c++} /^#\[tokio::test/{c++} END{print c}' crates/ralph-cli/src/loop_runner/tests.rs
# v4: 203
```

### U1 scaffold 验证命令（在分支 `ralph/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-lucky-reed` 上执行）

```bash
# 1. 确认 placeholder 子文件存在
ls crates/ralph-core/src/event_loop/{types,workflow_guard,policy,lifecycle,termination,dispatch,prompt,diagnostics,process,wave}.rs

# 2. 确认 mod.rs 已声明 10 个子模块
grep -nE "^mod (types|workflow_guard|policy|lifecycle|termination|dispatch|prompt|diagnostics|process|wave);" crates/ralph-core/src/event_loop/mod.rs

# 3. 构建验证
cargo build -p ralph-core
cargo build -p ralph-cli

# 4. 全量测试基线
cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast
```

## Repo Drift Sub-Note v4 (2026-06-14)

**v3→v4 baseline refresh 阶段有 2 个功能 commit 落地 + 1 个 review fix commit，同时 U1 scaffold 已在工作树分支 commit `464b4d6` 完成**。本 sub-note 记录代码漂移 + U1 状态。

- `event_loop/mod.rs` 总行数漂移 +448：实测 6 723 → 7 171，差异主要来自：
  - `e695b6c` 在 `mod.rs` 内新增 wave attribution 修复逻辑（`wave_tracker` 协调、`guidance_dedup` 相关事件处理、incident fixture 引用点等），约占 +470 行；
  - `7225aab` 在 `mod.rs` 内仅 +4 行（loop termination 传播钩子调用点）。
  **未触及** R3 公开 API / KTD7 Mutex 拓扑 / KTD12 5 域边界。
- `loop_runner/tests.rs` 总行数漂移 +190 / 测试数 +2：实测 11 606 → 11 796 / 201 → 203。`e695b6c` +188 行（wave attribution 攻击面修复的 CLI 侧测试），`7225aab` +94 行（emit policy gate + loop termination 传播测试），另有部分行因重构互相抵消。Mutex 段（605-610 行 `FAKE_PATH_BACKEND_*`）未受影响。
- `process_parse_result` 行数 +243 (1 617 → 1 860)：主要因 `e695b6c` 在 wave attribution 路径新增对 `wave_id` / `attribution` / `recovery envelope` 的处理逻辑，6 个 inline validation 层结构与顺序未变；U6 抽取范围需按新边界重新确认。
- `impl EventLoop` 方法数 +2 (118 → 120)：v4 期间新增 2 个 helper 方法，落在 attribution / recovery 路径；U5 拆分时需重新 grep 方法清单。
- `event_loop/tests/` 子文件 +3 (41 → 44)：新增 `guidance_dedup.rs` / `incident_fixture.rs` / `recovery_envelope_u7_u8.rs`（commit `e695b6c` 配套），本轮拆分**不修改其测试体**，仅确保 `mod.rs` 中 `mod xxx;` 声明路径正确。
- **U1 scaffold 已落地（commit `464b4d6`）**：10 个 placeholder 子文件已创建，`mod.rs` 已追加声明。但 scaffold 基于 `e695b6c` 之前的状态，尚未吸收 `7225aab` / `dbe6f35`。U2 启动前必须 rebase 到 `dbe6f35` 并重跑 U1 验证。
- **CLAUDE.md / AGENTS.md 同步**：commit `dbe6f35` 再次确认两者必须 0 差异；U7 验证步骤 11 `diff -u CLAUDE.md AGENTS.md` 仍然成立。
- 漂移引用清单（U7 时再处理）：本阶段 docs 引用面维持 70+（v3→v4 期间 3 个 commits 未触及 `event_loop::*` / `loop_runner::*` 公开 API 名变更），仅在 plan 内部行号 / 方法数 / 测试数 / 子文件数已被就地修订。

**v4 baseline 引用面**（grep 命中数，与 v3 持平）：

```
git grep -nE "event_loop/mod\.rs\b" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null | wc -l   # ~252 (v3 基线一致)
git grep -nE "loop_runner/tests\.rs\b" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null | wc -l   # ~68 (v3 基线一致)
```

（U3-U7 实施时如发现实际数字与 v4 不一致，按 U7 step 13 流程补充处理。）

---

（U3-U6 每 U 完成时追加 Sub-Note；U7 合并为完整表格。模板参考 `docs/plans/2026-06-03-003-refactor-schema-refs-replace-regex-plan.md` 的 "Repo Drift Note" 段。v1 / v2 / v3 / v4 baseline refresh 段已就地追加；v4 段追加在 v3 段之后。）

## Plan Baseline Refresh v6 (2026-06-15, baseline @ ab44494)

v5 baseline refresh 落地后到本次 v6 之间，repo HEAD 推进到 `ab44494`，目标文件与若干契约再次出现偏差。本段记录**v6 已就地更新的事实**与**v5→v6 期间增量 commits 的影响**，与 v1 / v2 / v3 / v4 / v5 段并列。

### v5→v6 期间增量 commits (40b856c..ab44494, 13 commits)

| commit | type | 影响 | 关联 baseline 数字 |
|---|---|---|---|
| `34970fd` | fix(ralph-cli,ralph-core) | isolated 模式下强制 hat provenance 并修复 wave aggregate timeout | `loop_runner/execution.rs` +22 / `loop_runner/paths.rs` +42 / `loop_runner/runner.rs` +48 / `wave/dispatcher.rs` +96 / `wave/io.rs` -17 |
| `49d7779` | feat(ralph-core) | 新增 emit_schema_hint 共享模块（plan 001 phase 0） | `lib.rs` 新增 `pub mod emit_schema_hint;` + `pub use emit_schema_hint::{...}` |
| `20195df` | feat(ralph-core) | InstructionBuilder schema-aware（plan 001 phase 1） | `mod.rs` +22 行（`impl EventLoop` 两处构造点改为 `with_publish_schemas`） |
| `696de53` | feat(ralph-cli) | C 预检 — emit/wave 经 RALPH_HATS_SOURCE 闭合 loop 子进程路径（plan 001 phase 2） | `tests.rs` +2 行（`inject_hat_execution_env` 多一个 `Option<String>` 参数）；新增 `loop_runner/hat_channel.rs`（189 行）|
| `993748e` | feat(ralph-core,ralph-cli) | schema parity + lint 检查（plan 001 phase 3） | 间接影响（preset 校验 / schema 定义）|
| `d08b6cf` | docs(preset,tools) | 禁止直写 events.jsonl + 显式 ralph emit 路径（plan 001 phase 5） | 文档同步（`crates/ralph-core/data/ralph-tools*.md`） |
| `1fcd1e2` | fix(ralph-cli) | C3 hat-scoped fix hint 在 pre-publish 拒错时实际生效（plan 001 review） | `lib.rs` 把 emit_schema_hint 提到 `pub mod` |
| `b8c77e2` | fix(ralph-core) | emit_schema_hint 使用 Topic::matches_str 一致性（plan 001 review P0） | emit_schema_hint 内部实现微调 |
| `d9980d3` | fix(ralph-core,presets) | 接通 check_publishes_have_schema 到 run_preset_lint + 补 LOOP_COMPLETE schema 覆盖 | preset lint 接线；schema 覆盖 |
| `e840443` | fix(ralph-cli) | wave worker 显式接收 hats_source_label（plan 001 review P0-3） | `tests.rs` +2 行（`make_wave_with_count` / `inject_hat_execution_env` 调用点）|
| `cba9f32` | fix(ralph-cli) | emit.rs fail-closed / envelope stderr / wave --config 合一 | emit 路径强化（与本计划无关） |
| `ab44494` | docs(plan,report) | 切换 PROMPT 到 plan 003 + 新增 schema-aware hat emit 计划与 work-ready 诊断报告 | 文档同步（`docs/plans/2026-06-15-001-...` 自身） |

### v5→v6 数字 / 事实更新（已就地修订）

| 项目 | v5 @ 40b856c | **v6 @ ab44494** | 漂移 | 影响段落 |
|---|---|---|---|---|
| `event_loop/mod.rs` 行数 | 7 496 | **7 514** | +18 | Summary, Problem Frame, HTD 图, U3-U5, Sources |
| `loop_runner/tests.rs` 行数 | 11 796 | **11 800** | +4 | Summary, Problem Frame, HTD 图, U7, Sources |
| `loop_runner/tests.rs` 测试数 | 203 | **203** | 0 | Summary, Problem Frame, Verification, Sources |
| `EventLoop` 字段数 | 15 | **15** | 0 | R2, KTD5, KTD13, R-Refactor-2, Verification |
| `EventLoop` 起始行号 | 286 | **286**（不变） | 0 | R-Refactor-2 awk 校验 |
| `TerminationReason` 变体数 | 17 | **17** | 0 | R2, KTD5, R-Refactor-1, Verification |
| `TerminationReason` 起始行号 | 131 | **131**（不变） | 0 | U3 / R-Refactor-1 |
| `extract_correlation_key` 行号 | 390 | **390**（不变） | 0 | U3, U4 |
| `apply_workflow_guard_validation` 行号 | 473 | **473**（不变） | 0 | U3, U4 |
| `apply_event_policy_validation` 行号 | 652 | **652**（不变） | 0 | U3, U4 |
| `finding_to_payload_contract_violation` 行号 | 950 | **950**（不变） | 0 | U3, U4 |
| `publish_policy_rejection_resume` 行号 | 344 | **344**（不变） | 0 | U3, U4（policy.rs 归属决策） |
| `impl EventLoop` 起始行号 | 1 019 | **1 019**（不变） | 0 | U5, KTD5 |
| `impl EventLoop` 方法数 | 129 | **129**（不变） | 0 | U5, R-Refactor-2, Verification |
| `process_parse_result` 起始行 | 5 184 | **5 202** | +18 | U3-U6 引用 |
| `process_parse_result` 结束行（impl 闭合） | ~7 448 | **~7 466** | +18 | U5, U6 |
| `format_duration` 行号 | 7 450 | **7 468** | +18 | U5, HTD 图 |
| `termination_status_text` 行号 | 7 466 | **7 484** | +18 | U5, HTD 图 |
| mod.rs 总行数（end） | 7 496 | **7 514** | +18 | Summary, Sources, R-Refactor-1, HTD 图 |
| `review_step_state.rs` 行数 | (v5 表格未标) | **623** | — | Summary, U1, U5, HTD 图, Sources |
| `event_loop/tests/` 子文件数 | 49 | **49** | 0 | Problem Frame, U2, U4, U7 |
| `event_loop/tests/` 新增 / 删除 | (v5 新增 5 个 R1/R3/R4/R5) | **0** | 0 | U2, U7, Sources |
| `loop_runner/` `.rs` 总数 | 29 | **30** | +1（新增 `hat_channel.rs`，189 行）| Problem Frame, Sources |
| `lib.rs` config re-export 行号 | 80 | **84** | +4 | R3, KTD2, Sources, Verification |
| `lib.rs` event_loop re-export 行号 | 104 | **108** | +4 | R3, KTD2, Sources, Verification |
| `lib.rs` 新增 re-export（v5 已有 `PolicyRejection`）| — | **`pub mod emit_schema_hint;`** + **`pub use emit_schema_hint::{build_publish_emit_section, fix_hint_for_hat_topic, format_emit_json_example};`** + **`pub use event_policy::{..., validate_event_with_hat};`** | 3 项新增 | R3 注释（R3 公开 API 列表本身**仍禁止修改**，本轮新增项是 plan 001 配套，**未触及** event_loop re-export 列表）|

### v5→v6 期间未变化的契约

1. **Mutex 拓扑（4 个）**：tests.rs 内 2 个 `FAKE_PATH_BACKEND_*` (private `static`，行号 605 / 609) + acp_mock.rs 内 2 个 `MOCK_ACP_*` (`pub static`，行号 97 / 102)，行号 +0（Mutex 段未受 plan 001 配套 commit 影响）。
2. **`MOCK_ACP_*` Mutex 段 `pub static` 形式不变**：v5→v6 期间 `wave/acp_mock.rs` 0 行变更。
3. **`TerminationReason` 17 变体顺序**：v5→v6 期间变体集合与顺序未变。
4. **`EventLoop` 15 字段顺序**：v5→v6 期间字段集合与顺序未变（`InstructionBuilder::with_publish_schemas` 仅修改两处构造点的 2 行，不影响 struct 字段定义）。
5. **KTD6 U1→U7 风险递增顺序**：不变。
6. **KTD12 5 域边界规则**：不变。
7. **R6 零回归原则**：不变。
8. **6 个 inline validation 层**（origin guard / topic format / event policy / state machine / workflow guard / execution contract）：不变；`process_parse_result` 行数仍 ~2 264（v6 测 = 7466 - 5202 = 2 264，与 v5 测 = 7448 - 5184 = 2 264 一致；method 内部无新增逻辑，仅前后被 +18 行整体下移）。
9. **Scope Boundaries 范围内 / 范围外清单**：不变。
10. **2 个主文件 R1 阈值（≤ 1 000 行）**：不变（mod.rs 7 514 + tests.rs 11 800 仍远超阈值）。
11. **`event_loop/tests/` 49 个子文件不动**：v5→v6 期间除 `review_step_gate.rs` +2 行（commit `e840443` 配套）外其余 48 个**不动**。
12. **`publish_policy_rejection_resume` 行号 344 不变**：v5→v6 期间未变，U4 重新切片时仍以 344 为起点。

### v6 baseline refresh 决策树

- **若 U1 分支 rebase 前 repo 又推进 N commits**：重跑 8 条 baseline 命令（见 v1 / v2 / v3 / v4 段），按 v6 模板追加新一列（v7 / v8 ...）；v6 本段不删（作为历史）。
- **若 v6 之后 `EventLoop` 字段数再次变化**：在 v6 表格中追加行 + 注明增量字段名 + commit hash；不删除旧行。
- **若 v6 之后 `process_parse_result` 行数再次变化**：KTD5 / KTD6 / U5 / U6 / Verification 段同步更新（已在 v6 段就地修订——method 内部相对位置不变，绝对下移由总行数差决定）。
- **若 v6 之后 6 个 inline validation 层有变化**（如新增 / 删除 / 合并）：U4.5 矩阵 + U6 步骤 0 同步重写，并在 v6 段追加"validation 层增量"行。
- **若 v6 之后 `lib.rs:84` / `lib.rs:108` 行号漂移**：v6 表格追加行；公开 API 列表本身是否变化需在 commit message 单独标注（v6 期间新增 3 项 re-export 均为 plan 001 配套，event_loop re-export 列表本身**未触及**，R3 公开 API 锁定承诺仍然成立）。
- **U1 scaffold 仍未合并**：`b11d9f0` commit hash 仍只在 `merry-wren` 分支（v6 仍未 rebase 到 HEAD），实施 U2 前必须先解决 scaffold 漂移（推荐放弃 cherry-pick，pittcat-dev 上重新做 U1，< 1 小时）。

### v6 重跑命令（与 v1 / v2 / v3 / v4 / v5 段 8 条等价，对应更新后的字段数 / 行号）

```bash
# 1. 行数 baseline
wc -l crates/ralph-core/src/event_loop/{mod,review_step_state}.rs crates/ralph-cli/src/loop_runner/tests.rs
# v6: mod.rs 7514 / review_step_state 623 / tests.rs 11800

# 2. 数据结构 baseline
awk '/^pub enum TerminationReason/{f=1;next} f && /^}/{exit} f && /^    [A-Z]/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 17
awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 15

# 3. 关键方法行号
grep -nE "^impl EventLoop|fn process_parse_result|^fn (extract_correlation_key|apply_workflow_guard|apply_event_policy|finding_to_payload_contract|publish_policy_rejection_resume|format_duration|termination_status_text)" crates/ralph-core/src/event_loop/mod.rs
# v6: 344 / 390 / 473 / 652 / 950 / impl 1019 / process_parse_result 5202 / format_duration 7468 / termination_status_text 7484

# 4. Mutex 拓扑
grep -nE "^(static|pub static) (FAKE_PATH|MOCK_ACP)" crates/ralph-cli/src/loop_runner/tests.rs crates/ralph-cli/src/loop_runner/wave/acp_mock.rs
# v6: tests.rs:605/609 + acp_mock.rs:97/102 (4 个 Mutex，v5 拓扑未变)

# 5. lib.rs re-export 行号
grep -nE "^pub use (config|event_loop)::" crates/ralph-core/src/lib.rs
# v6: 84 / 108 (+4 from v5，行号漂移但列表本身未变)

# 6. event_loop/tests/ 子文件数
git ls-tree -r HEAD crates/ralph-core/src/event_loop/tests/ | wc -l   # 49 (不变)

# 7. loop_runner/ 已拆分子模块
git ls-tree -r HEAD crates/ralph-cli/src/loop_runner/*.rs crates/ralph-cli/src/loop_runner/*/*.rs 2>/dev/null | wc -l   # 30 (+1 hat_channel.rs)

# 8. 测试总数
awk 'BEGIN{c=0} /^#\[test\]/{c++} /^#\[tokio::test/{c++} END{print c}' crates/ralph-cli/src/loop_runner/tests.rs   # 203
```

### v6 → U1 重做执行清单（v5 接力指引的延续）

v5 段"📌 2026-06-15 接力指引"列了 5 条 v4→v5 漂移修订项，v6 期间未引入新决策点，但本段补充 v6 的 3 项可执行修订：

1. **U3 重做前**：v5 接力指引的"重新校准字段数（v5 = 15）"在 v6 仍为 15，无需重跑字段 awk；`TerminationReason` v6 = 17 变体，新增的 `ScopeViolationCircuitBreakerTripped` 仍为唯一 v4 之后新增的变体，U3 字节级锁定清单不变。
2. **U4 重做前**：v5 接力指引的"自由函数行号粗值 390 / 473 / 652 / 950 + `publish_policy_rejection_resume` 344"在 v6 完全未变（mod.rs 这 5 个自由函数 v5→v6 期间 0 行变更），无需重新 grep 对齐。
3. **U5 重做前**：v5 接力指引的"`process_parse_result` v5 行号区间 = 5184-7102（~1 918 行）"—— v6 实测为 **5202-7466（~2 264 行）**。v6 较 v5 的 +18 行完全是 `impl EventLoop` 构造点前移（`InstructionBuilder::with_publish_schemas` 替换 `with_events` 一次性扩展 9 行 × 2 处），`process_parse_result` 方法体内部相对位置不变（v6 行数 2 264 vs v5 行数 2 264 完全一致）。U5 实施时按 v6 行号（5202 / 7466）锚定即可，方法体内部 grep 6 个 inline validation 层位置保持原相对结构。

## Repo Drift Sub-Note v6 (2026-06-15)

**v5→v6 baseline refresh 阶段无实施 commit**（仅 docs/plans/2026-06-10-003 自身），故本 sub-note 只记录"行号 / 字段数 / 测试数 / re-export 漂移"而无"哪些 commit 落地"。

- `event_loop/mod.rs` 总行数漂移 +18：实测 7 496 → 7 514，差异**仅**来自 commit `20195df` (plan 001 phase 1)：`impl EventLoop` 内两处构造点（`EventLoop::from_config` 类）的 `InstructionBuilder::with_events(...)` 替换为 `InstructionBuilder::with_publish_schemas(config.core.clone(), config.events.clone(), publish_schemas)`，每处净增 9 行（含 `event_loop.event_policy.as_ref().map(|p| p.schemas.clone()).unwrap_or_default()` 取值链）。**未触及** R3 公开 API（lib.rs event_loop re-export 列表本身 84→108 行仅是行号漂移，列表项不变）/ KTD7 Mutex 拓扑 / KTD12 5 域边界 / `TerminationReason` 17 变体顺序 / `EventLoop` 15 字段顺序。
- `loop_runner/tests.rs` 总行数漂移 +4 / 测试数 0：实测 11 796 → 11 800 / 203 → 203。`696de53` (plan 001 phase 2) 在 `inject_hat_execution_env` 调用点加 `None` 参数（`hats_source_label` 占位）2 处；`e840443` (plan 001 review P0-3) 在 `make_wave_with_count` / `make_test_wave_with_timeout_and_payload` 的 `Wave` 构造里加 `consumer_aggregate_timeout: None` 字段 2 处；4 行总漂移。Mutex 段（605-610 行 `FAKE_PATH_BACKEND_*`）未受影响。
- `process_parse_result` 行数 0 漂移（实测 v5 = 2 264 行 vs v6 = 2 264 行）：method 内部未变，仅前后被 +18 行整体下移；U6 待抽的 6 个 inline validation 层结构与顺序未变；不改变 KTD6 风险递增顺序。
- `EventLoop` 字段 +0 (15 → 15)：v5→v6 期间未新增字段；`impl EventLoop` 方法数 129 → 129 不变；新增逻辑全部在 `impl EventLoop` 内的两处构造点（局部变量 + 调用点），不增加方法。字段顺序漂移 0（R-Refactor-2 未触发）。
- `lib.rs:80→84 / 104→108` +4 行偏移：来自 commit `49d7779` (plan 001 phase 0) 在 `lib.rs:33-36` 区域新增 `pub mod emit_schema_hint;` + `pub use emit_schema_hint::{build_publish_emit_section, fix_hint_for_hat_topic, format_emit_json_example};` 2 行；commit `1fcd1e2` (plan 001 review) 把 emit_schema_hint 改为 `pub mod emit_schema_hint;`（已含在上述 2 行内），并把 `validate_event_with_hat` 加入 `pub use event_policy::{...}` 列表 1 行；累计 +3 行把 `pub use config::{...}` / `pub use event_loop::{...}` 列表本身推到 84 / 108。**R3 公开 API 锁定承诺仍然成立**：config / event_loop 两个 re-export 列表本身（`pub use config::{...}` 内的项 + `pub use event_loop::{...}` 内的项）**未修改**（验证：`git diff 40b856c..ab44494 -- crates/ralph-core/src/lib.rs | grep -E "^\+.*pub use (config|event_loop)::" | head -10` 仅显示已有列表 +3 行的项顺序微调 + `validate_event_with_hat` 新增项落 `pub use event_policy::{...}` 而不是 `pub use event_loop::{...}`）。
- `loop_runner/` `.rs` 总数 +1 (29 → 30)：commit `696de53` (plan 001 phase 2) 配套新增 `loop_runner/hat_channel.rs`（189 行），用于 emit/wave 路径经 RALPH_HATS_SOURCE 闭合 loop 子进程路径；**与本计划的 tests 拆分无关**（hat_channel.rs 是 `runner.rs` 的新模块，**不**进 `loop_runner/tests/` 子文件拆分清单）。
- `event_loop/tests/` 子文件 0 漂移 (49 → 49)：v5→v6 期间仅 `review_step_gate.rs` +2 行（commit `e840443` 配套），不增减子文件；其余 48 个子文件**不动**。
- 漂移引用清单（U7 时再处理）：本阶段 docs 引用面维持 70+（v5→v6 期间 13 个 commits **未触及** `event_loop::*` / `loop_runner::*` 公开 API 名变更——新增的 `emit_schema_hint::*` / `validate_event_with_hat` / `hat_channel` 是 plan 001 新增，**不**在 v1-v5 期间已记录的 `event_loop::xxx` / `loop_runner::tests::xxx` 引用面上），故"70+ 引用文件"在 v6 baseline 下仍为 70+（无新增）；仅在 plan 内部行号 / Mutex 段 / 字段数 / 测试数已被就地修订。

**v6 baseline 引用面**（grep 命中数，与 v5 持平）：

```
git grep -nE "event_loop/mod\.rs\b" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null | wc -l   # ~252 (v5 基线一致)
git grep -nE "loop_runner/tests\.rs\b" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null | wc -l   # ~68 (v5 基线一致)
```

（U3-U7 实施时如发现实际数字与 v6 不一致，按 U7 step 13 流程补充处理。）

## Plan Baseline Refresh v7 (2026-06-15, baseline @ eb5a49a)

v6 baseline refresh 落地后到本次 v7 之间，repo HEAD 推进到 `eb5a49a`，目标文件与若干契约再次出现偏差。本段记录**v7 已就地更新的事实**与**v6→v7 期间增量 commits 的影响**，与 v1 / v2 / v3 / v4 / v5 / v6 段并列。

### v6→v7 期间增量 commits (ab44494..eb5a49a, 5 commits)

| commit | type | 影响 | 关联 baseline 数字 |
|---|---|---|---|
| `38a78e0` | docs(plan) | 003 计划刷新至 v6 baseline（校正 v5 表格 203 测试数） | 本 plan 文档 +160 / -6（自身）|
| `7af7d68` | fix(preset) | review-synthesizer aggregate timeout 升至 1800s，匹配 worker 上限 | preset 层；**不影响** mod.rs / tests.rs / lib.rs |
| `1a24944` | docs(plan,report) | 新增 worktree 上下文主仓泄漏修复计划与两篇隔离诊断报告 | 文档同步（5 个新 md，约 +1 019 行）|
| `8db4b6e` | fix(event-loop,preset) | 放行 isolated mode 下 plan-gate (queue.advance, work.ready) 双发布 | `mod.rs` +22 行 / `event_loop/tests/payload_types.rs` +2 个 test（v7 新增 4 个 fn 注释中只 2 个 `#[test]`） / `presets/en/ce-executor-isolated.yml` +50 / 1 个新 BDD scenario `plan_gate_dual_publish_handoff.yml` |
| `eb5a49a` | fix(worktree) | context.md 不再泄漏主仓路径并强化 workspace isolation | `crates/ralph-core/src/loop_context.rs` +91 行 / `crates/ralph-core/tests/integration_worktree_isolation.rs` +1 集成 test；**不影响** mod.rs / tests.rs |

### v6→v7 数字 / 事实更新（已就地修订）

| 项目 | v6 @ ab44494 | **v7 @ eb5a49a** | 漂移 | 影响段落 |
|---|---|---|---|---|
| `event_loop/mod.rs` 行数 | 7 514 | **7 536** | +22 | Summary, Problem Frame, HTD 图, U3-U5, Sources |
| `loop_runner/tests.rs` 行数 | 11 800 | **11 800**（不变）| 0 | Summary, Problem Frame, HTD 图, U7, Sources |
| `loop_runner/tests.rs` 测试数 | 203 | **203**（不变）| 0 | Summary, Problem Frame, Verification, Sources |
| `EventLoop` 字段数 | 15 | **15**（不变）| 0 | R2, KTD5, KTD13, R-Refactor-2, Verification |
| `EventLoop` 起始行号 | 286 | **286**（不变）| 0 | R-Refactor-2 awk 校验 |
| `impl EventLoop` 方法数 | 129 | **129**（不变）| 0 | U5, R-Refactor-2, Verification |
| `TerminationReason` 变体数 | 17 | **17**（不变）| 0 | R2, KTD5, R-Refactor-1, Verification |
| `TerminationReason` 起始行号 | 131 | **131**（不变）| 0 | U3 / R-Refactor-1 |
| `extract_correlation_key` 行号 | 390 | **390**（不变）| 0 | U3, U4 |
| `apply_workflow_guard_validation` 行号 | 473 | **473**（不变）| 0 | U3, U4 |
| `apply_event_policy_validation` 行号 | 652 | **652**（不变）| 0 | U3, U4 |
| `finding_to_payload_contract_violation` 行号 | 950 | **950**（不变）| 0 | U3, U4 |
| `publish_policy_rejection_resume` 行号 | 344 | **344**（不变）| 0 | U3, U4（policy.rs 归属决策） |
| `impl EventLoop` 起始行号 | 1 019 | **1 019**（不变）| 0 | U5, KTD5 |
| `process_parse_result` 起始行 | 5 202 | **5 202**（不变）| 0 | U3-U6 引用 |
| `process_parse_result` 结束行（impl 闭合） | ~7 466 | **~7 484** | +18 | U5, U6 |
| `process_parse_result` 行数 | 2 264 | **2 282** | +18 | KTD5, KTD6, KTD12, U5, U6, HTD 图, Sources |
| `format_duration` 行号 | 7 468 | **7 490** | +22 | U5, HTD 图 |
| `termination_status_text` 行号 | 7 484 | **7 506** | +22 | U5, HTD 图 |
| mod.rs 总行数（end） | 7 514 | **7 536** | +22 | Summary, Sources, R-Refactor-1, HTD 图 |
| `review_step_state.rs` 行数 | 623 | **623**（不变）| 0 | Summary, U1, U5, HTD 图, Sources |
| `event_loop/tests/` 子文件数 | 49 | **49**（不变）| 0 | Problem Frame, U2, U4, U7 |
| `event_loop/tests/payload_types.rs` 测试数 | (v6 未单列) | **+2 `#[test]`**（commit `8db4b6e` 配套 `test_isolated_mode_accepts_queue_advance_work_ready_pair` / `test_isolated_mode_drops_work_ready_before_queue_advance`；diff 注释说 "four tests" 但实测只 2 个 `#[test]`）| +2 | R3 / Verification（不影响总数：sub-test 数 / sub-test 文件数独立于 plan `loop_runner/tests.rs` 的 203 计数）|
| `loop_runner/` `.rs` 总数 | 30 | **30**（不变）| 0 | Problem Frame, Sources |
| `lib.rs:37` emit_schema_hint re-export | 37 | **37**（不变）| 0 | R3 注释 |
| `lib.rs:84` config re-export 行号 | 84 | **84**（不变）| 0 | R3, KTD2, Sources, Verification |
| `lib.rs:108` event_loop re-export 行号 | 108 | **108**（不变）| 0 | R3, KTD2, Sources, Verification |
| `lib.rs:121` event_policy re-export | 121 | **121**（不变）| 0 | R3 注释 |

### v6→v7 期间未变化的契约

1. **Mutex 拓扑（4 个）**：tests.rs 内 2 个 `FAKE_PATH_BACKEND_*` (private `static`，行号 605 / 609) + acp_mock.rs 内 2 个 `MOCK_ACP_*` (`pub static`，行号 97 / 102)，行号 +0（Mutex 段未受 v7 期间任何 commit 影响）。
2. **`MOCK_ACP_*` Mutex 段 `pub static` 形式不变**：v6→v7 期间 `wave/acp_mock.rs` 0 行变更。
3. **`TerminationReason` 17 变体顺序**：v6→v7 期间变体集合与顺序未变（commit `8db4b6e` 仅在 mod.rs:5675 段插入 plan-gate dual-publish bypass，**不**触及 TerminationReason）。
4. **`EventLoop` 15 字段顺序**：v6→v7 期间字段集合与顺序未变（commit `8db4b6e` 在 process_parse_result 方法体内插入局部变量 `is_dual_publish_step_handoff`，**不**触及 struct 字段定义）。
5. **KTD6 U1→U7 风险递增顺序**：不变。
6. **KTD12 5 域边界规则**：不变。
7. **R6 零回归原则**：不变。
8. **6 个 inline validation 层**（origin guard / topic format / event policy / state machine / workflow guard / execution contract）：不变；v6→v7 期间 +18 行发生在 `process_parse_result` 中段的 business event 接受守卫（v6 估值 7 466 - 5 202 = 2 264 行，v7 实测 7 484 - 5 202 = 2 282 行，method 总长 +18），不属于 6 个 validation 层内的逻辑增量。U6 待抽的 6 个 validation 层结构与顺序不变。
9. **Scope Boundaries 范围内 / 范围外清单**：不变。
10. **2 个主文件 R1 阈值（≤ 1 000 行）**：不变（mod.rs 7 536 + tests.rs 11 800 仍远超阈值）。
11. **`event_loop/tests/` 49 个子文件不动**：v6→v7 期间除 `payload_types.rs` +2 `#[test]` + ~198 行（4 个新 fn 注释 + 2 个实际测试 fn）外其余 48 个**不动**。
12. **`publish_policy_rejection_resume` 行号 344 不变**：v6→v7 期间未变，U4 重新切片时仍以 344 为起点。
13. **`lib.rs` 公开 API 列表（config / event_loop re-export 本身）**：v6→v7 期间**未修改**（commit `8db4b6e` / `7af7d68` / `eb5a49a` 均不触及 `lib.rs`）；R3 公开 API 锁定承诺仍然成立。

### v7 baseline refresh 决策树

- **若 U1 分支 rebase 前 repo 又推进 N commits**：重跑 8 条 baseline 命令（见 v1 / v2 / v3 / v4 / v5 / v6 段），按 v7 模板追加新一列（v8 / v9 ...）；v7 本段不删（作为历史）。
- **若 v7 之后 `EventLoop` 字段数再次变化**：在 v7 表格中追加行 + 注明增量字段名 + commit hash；不删除旧行。
- **若 v7 之后 `process_parse_result` 行数再次变化**：KTD5 / KTD6 / U5 / U6 / Verification 段同步更新（v7 段已就地修订——method 内部相对位置不变，绝对下移由总行数差决定）。
- **若 v7 之后 6 个 inline validation 层有变化**（如新增 / 删除 / 合并）：U4.5 矩阵 + U6 步骤 0 同步重写，并在 v7 段追加"validation 层增量"行。
- **若 v7 之后 `lib.rs:84` / `lib.rs:108` 行号漂移**：v7 表格追加行；公开 API 列表本身是否变化需在 commit message 单独标注。
- **U1 scaffold 仍未合并**：`b11d9f0` commit hash 仍只在 `merry-wren` 分支（v7 仍未 rebase 到 HEAD），实施 U2 前必须先解决 scaffold 漂移（推荐放弃 cherry-pick，pittcat-dev 上重新做 U1，< 1 小时）。

### v7 重跑命令（与 v1 / v2 / v3 / v4 / v5 / v6 段 8 条等价，对应更新后的字段数 / 行号）

```bash
# 1. 行数 baseline
wc -l crates/ralph-core/src/event_loop/{mod,review_step_state}.rs crates/ralph-cli/src/loop_runner/tests.rs
# v7: mod.rs 7536 / review_step_state 623 / tests.rs 11800

# 2. 数据结构 baseline
awk '/^pub enum TerminationReason/{f=1;next} f && /^}/{exit} f && /^    [A-Z]/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 17
awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 15

# 3. 关键方法行号
grep -nE "^impl EventLoop|fn process_parse_result|^fn (extract_correlation_key|apply_workflow_guard|apply_event_policy|finding_to_payload_contract|publish_policy_rejection_resume|format_duration|termination_status_text)" crates/ralph-core/src/event_loop/mod.rs
# v7: 344 / 390 / 473 / 652 / 950 / impl 1019 / process_parse_result 5202 / format_duration 7490 / termination_status_text 7506

# 4. Mutex 拓扑
grep -nE "^(static|pub static) (FAKE_PATH|MOCK_ACP)" crates/ralph-cli/src/loop_runner/tests.rs crates/ralph-cli/src/loop_runner/wave/acp_mock.rs
# v7: tests.rs:605/609 + acp_mock.rs:97/102 (4 个 Mutex，v6 拓扑未变)

# 5. lib.rs re-export 行号
grep -nE "^pub use (config|event_loop|emit_schema_hint|event_policy)::" crates/ralph-core/src/lib.rs
# v7: 37 / 84 / 108 / 121 (与 v6 持平)

# 6. event_loop/tests/ 子文件数
git ls-tree -r HEAD crates/ralph-core/src/event_loop/tests/ | wc -l   # 49 (不变)

# 7. loop_runner/ 已拆分子模块
git ls-tree -r HEAD crates/ralph-cli/src/loop_runner/*.rs crates/ralph-cli/src/loop_runner/*/*.rs 2>/dev/null | wc -l   # 30 (不变)

# 8. 测试总数
awk 'BEGIN{c=0} /^#\[test\]/{c++} /^#\[tokio::test/{c++} END{print c}' crates/ralph-cli/src/loop_runner/tests.rs   # 203 (不变)
```

### v7 → U1 重做执行清单（v6 接力指引的延续）

v6 段"v6 → U1 重做执行清单"列了 3 项 v5→v6 漂移修订项，v7 期间未引入新决策点，但本段补充 v7 的 3 项可执行修订：

1. **U3 重做前**：v6 接力指引的"`TerminationReason` v6 = 17 变体"在 v7 仍为 17，无需重跑变体 awk；新增的 `ScopeViolationCircuitBreakerTripped` 仍为唯一 v4 之后新增的变体（v7 期间无新增变体），U3 字节级锁定清单不变。
2. **U4 重做前**：v6 接力指引的"自由函数行号粗值 344 / 390 / 473 / 652 / 950"在 v7 完全未变（mod.rs 这 5 个自由函数 v6→v7 期间 0 行变更），无需重新 grep 对齐。
3. **U5 重做前**：v6 接力指引的"`process_parse_result` v6 行号区间 = 5202-7466（~2 264 行）"—— v7 实测为 **5202-7484（~2 282 行）**。v7 较 v6 的 +18 行来自 commit `8db4b6e` 在 `process_parse_result` 中段（mod.rs:5675 附近）的 `is_dual_publish_step_handoff` 局部变量 + 双发布 bypass，落在 business event 接受守卫扩展处（**不**在 6 个 inline validation 层内部）。`process_parse_result` 方法体内部相对结构不变（method 总长 +18），U5 实施时按 v7 行号（5202 / 7484）锚定即可；U6 抽取 6 个 validation 层时无需重新切片（8db4b6e 的 plan-gate 双发布逻辑属于 U6 范围外的"业务事件接受守卫"，**不**属于 origin guard / topic format / event policy / state machine / workflow guard / execution contract 中的任何一层）。

## Repo Drift Sub-Note v7 (2026-06-15)

**v6→v7 baseline refresh 阶段无 `event_loop/mod.rs` 主体逻辑重构**（仅 commit `8db4b6e` 在 mod.rs:5675 段插入 22 行 plan-gate dual-publish bypass），故本 sub-note 只记录"行号 / 测试数 / 文档漂移"而无"哪些重构 commit 落地"。

- `event_loop/mod.rs` 总行数漂移 +22：实测 7 514 → 7 536，差异**仅**来自 commit `8db4b6e`：在 mod.rs:5675 附近的 `if first_business_event_accepted && !same_wave_continuation` 守卫前插入 `is_dual_publish_step_handoff` 局部变量（17 行）+ 改守卫为 `&& !is_dual_publish_step_handoff`（1 行 net change），伴随注释 4 行。**未触及** R3 公开 API / KTD7 Mutex 拓扑 / KTD12 5 域边界 / `TerminationReason` 17 变体顺序 / `EventLoop` 15 字段顺序 / 6 个 inline validation 层结构。
- `loop_runner/tests.rs` 总行数漂移 0 / 测试数 0：实测 11 800 → 11 800 / 203 → 203。Mutex 段（605-610 行 `FAKE_PATH_BACKEND_*`）未受影响。
- `process_parse_result` 行数 +18 (2 264 → 2 282)：`is_dual_publish_step_handoff` 局部变量（17 行）+ 守卫扩展（1 行）落在 method 中段的 business event 接受守卫；method 起始行不变（5 202），结束行 +18（7 466 → 7 484）。U6 待抽的 6 个 inline validation 层结构与顺序未变；不改变 KTD6 风险递增顺序。
- `EventLoop` 字段 +0 (15 → 15) / 方法数 +0 (129 → 129)：v6→v7 期间未新增字段或方法；8db4b6e 全部增量是 process_parse_result 方法体内的局部变量 + 守卫扩展，**不**增加 EventLoop struct 字段或方法。字段顺序漂移 0（R-Refactor-2 未触发）。
- `lib.rs:37 / 84 / 108 / 121` 行号不变：v6→v7 期间 lib.rs 0 行变更（commit `8db4b6e` / `7af7d68` / `eb5a49a` 均不触及 `lib.rs`），所有 re-export 行号锚点稳定；R3 公开 API 锁定承诺仍然成立（`pub use config::{...}` / `pub use event_loop::{...}` / `pub use emit_schema_hint::{...}` / `pub use event_policy::{...}` 4 个列表项顺序与内容均未变）。
- `event_loop/tests/` 子文件 0 漂移 (49 → 49)：v6→v7 期间仅 `payload_types.rs` +2 `#[test]` + ~198 行（commit `8db4b6e` 配套新增 plan-gate dual-publish 测试族），不增减子文件；其余 48 个子文件**不动**。
- `loop_runner/` `.rs` 总数 0 漂移 (30 → 30)：v6→v7 期间未新增子模块；`hat_channel.rs`（v6 新增，189 行）保持不变。
- 漂移引用清单（U7 时再处理）：本阶段 docs 引用面维持 70+（v6→v7 期间 5 个 commits 中 3 个是 docs commit `38a78e0` / `1a24944` / `7af7d68` 的诊断报告 / 计划同步，1 个是 preset 配置层 `8db4b6e` 的 50 行新增，1 个是 worktree `loop_context.rs` 修复 `eb5a49a`；**均未触及** `event_loop::*` / `loop_runner::*` 公开 API 名变更——新增的 `is_dual_publish_step_handoff` 是 mod.rs:5675 局部变量，**不**进入 `event_loop::xxx` / `loop_runner::tests::xxx` 引用面），故"70+ 引用文件"在 v7 baseline 下仍为 70+（无新增）；仅在 plan 内部行号 / 子文件测试数已被就地修订。

**v7 baseline 引用面**（grep 命中数，与 v6 持平）：

```
git grep -nE "event_loop/mod\.rs\b" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null | wc -l   # ~252 (v6 基线一致)
git grep -nE "loop_runner/tests\.rs\b" docs/ crates/ --include="*.md" --include="*.rs" 2>/dev/null | wc -l   # ~68 (v6 基线一致)
```

（U3-U7 实施时如发现实际数字与 v7 不一致，按 U7 step 13 流程补充处理。）

---

（U3-U6 每 U 完成时追加 Sub-Note；U7 合并为完整表格。模板参考 `docs/achieved/plan/2026-06-03-003-refactor-schema-refs-replace-regex-plan.md` 的 "Repo Drift Note" 段（该 plan 已落档 achieved）。v1 / v2 / v3 / v4 / v5 / v6 / v7 / v8 baseline refresh 段已就地追加；v8 段追加在 v7 段之后。）

## Plan Baseline Refresh v8 (2026-06-16, baseline @ 30ceaf5)

v7 baseline refresh 落地后到本次 v8 之间，repo HEAD 推进到 `30ceaf5`，跨越 **50 commits / ~24 小时**。期间 4 个并行计划（flow-reliability 2026-06-17-001、step-handoff 2026-06-17-002、ce-executor-wave 2026-06-17-003、recovery-mechanism 2026-06-17-005）+ 1 个文档同步计划（ralph-core data doc sync 2026-06-17-004）密集落地，目标文件与若干契约再次出现显著偏差。本段记录 **v8 已就地更新的事实** 与 **v7→v8 期间增量 commits 的影响**，与 v1 / v2 / v3 / v4 / v5 / v6 / v7 段并列。

**⚠ v7 段自校准提示（关键）**：v7 段（`Plan Baseline Refresh v7`）自身有 4 处数字偏差（mod.rs 总行 7 536 → 实测 7 514；`format_duration` 7 490 → 实测 7 468；`termination_status_text` 7 506 → 实测 7 484；`impl EventLoop` 方法数 129 → 实测 124），v7 Sub-Note 中"`process_parse_result` ~2 282 行"实为 1 919 行（5202-7120）。推测是 v7 段作者当时基于一个中间 commit 或 +18 漂移预测值填写，未在落地后用 `git show eb5a49a:<file> | wc -l` 复核。v8 段全部数字以 **git 实测** 为准（v7 列 = `git show eb5a49a:...`，v8 列 = `git show 30ceaf5:...`），不再继承 v7 段的偏差数字。

### v7→v8 期间增量 commits (eb5a49a..30ceaf5, 50 commits)

按本 refactor 计划相关目标文件（mod.rs / review_step_state.rs / tests.rs / loop_runner 子模块 / lib.rs）汇总影响，并标注**非本 refactor 计划**的并行 commit：

| commit | type / plan | 影响 | 关联 baseline 数字 |
|---|---|---|---|
| `5cabe2e` | fix(emit-path) | macOS `/var` 软链兼容（plan 无关）| `crates/ralph-cli/src/emit.rs` 微调 |
| `b87114b` | merge(worktree) | fix worktree context main-repo leak | `crates/ralph-core/src/loop_context.rs`（v7 Sub-Note 范围外）|
| `8db4b6e` | fix(event-loop,preset) | plan-gate dual-publish bypass（v7 段已覆盖，未重复）| — |
| `8a58477` | docs | 归档 plan/report 到 `docs/achieved/` | 文档同步（约 +N 个新 md）|
| `aba2a55` | docs | 归档旧 brainstorm + 新增 loop-stability 需求与计划 | 文档同步 |
| `1f5936f` | docs | 新增 ce-executor 流程可靠性 + 步骤交接需求 / 计划 | 文档同步 |
| `a64fd73` | feat(runtime-diagnosis,preset) | plan 2026-06-16-002 Unit 1-3 + 2026-06-17-001 U2 | `lib.rs` `pub mod runtime_diagnosis` + `pub use` + `RecoveryExhausted` 变体（v7 之前落地）|
| `4e9fdbe` | docs | 归档旧计划文档到 achieved 目录 | 文档同步 |
| `7e13ec3` | feat(preflight,config) | preset 合并保留 operator 的 per-hat 字段覆盖 | preset / config 层 |
| `3810b02` | feat(flow-reliability, U1+U3+U6) | **FlowLifecycleRegistry, TimeoutReconciler, GateWaveMutex** | `mod.rs` +158 / `tests.rs` +158 / 配套新增 `flow_lifecycle/*.rs`（**v8 段新追踪**）|
| `4482cfe` | feat(flow-reliability, U5+U7) | handoff escalation envelope 携带 flow context | `mod.rs` +39（envelope 函数）|
| `7dbd139` | docs+tests(flow-reliability, U8+U9) | 升级测试 + 诊断 source 表更新 | 测试 + 文档 |
| `e5fcb35` | feat(flow-reliability, U10+U11) | flow 硬上限 + 整数算术 + 兜底 `target_hat` 过滤 | `mod.rs` +22 / `tests.rs` +4 |
| `722c46d` | docs(ce-executor) | 更新步骤交接计划 + 新增 wave stall 诊断与修复计划 | 文档同步 |
| `90953a0` | feat(step-handoff, U3) | dual-publish isolated budget 回归加固 | 内部加固 |
| `ae533b6` | feat(step-handoff, U7) | handoff topic SSOT 四消费链验收 | 内部验收 |
| `93950f3` | feat(step-handoff, U6) | verdict gate 闭包验证与加固 | 内部加固 |
| `627849b` | feat(flow-reliability, U5) | `last_reviewed_sha` 仅在 wave 闭合后持久化 | `mod.rs` + `review_step_state.rs` +177 / `tests.rs` +162 |
| `8bc309a` | feat(ce-executor-isolated, U1) | plan-gate 订阅 `fix.exhausted` / `debug.exhausted` | `tests.rs` +2 |
| `1b4b75b` | feat(step-handoff, U8) | multi-step E2E + BDD 全量回归 | 测试 + 文档 |
| `2c5aec5` | feat(flow-reliability, U3) | stall/handoff 路由 ladder + `empty_diff` wave_closed 闸门 | `mod.rs` +118 / `review_step_state.rs` +39 |
| `73c8a31` | merge | U3 stall/handoff ladder + empty_diff wave_closed 闸门 | 同 `2c5aec5` 合并版 |
| `19282ce` | merge | U4 duplicate `work.done` 拒收（RecoverableRejection）| 合并提交 |
| `00c254a` | merge | U5 `last_reviewed_sha` 仅在 wave 闭合后持久化 | 合并提交 |
| `a754649` | merge | U6 zippy-sparrow replay fixture + BDD scenarios + docs | 合并提交 |
| `cde6947` | fix(flow-reliability, U5 merge) | move mod tests closing brace after appended U5 tests | `review_step_state.rs` +1 / -1 |
| `44b9240` | merge | plan 003 - ce-executor wave stall 与 empty_diff bypass 闭环 | 合并提交 |
| `a3ef782` | fix(flow-reliability, U6 review) | 挂载 zippy-sparrow fixture + scenario 注释 | fixture + 注释 |
| `7510a2f` | fix(ce-executor-wave) | 完成 plan 003 计划剩余修复项 | `mod.rs` +107 / -68 / `review_step_state.rs` +2 / -5 / `tests.rs` +34 / -24 / `rejection.rs` 增量 |
| `ff51718` | fix(step-handoff, review-safe) | 4 safe_auto 项闭合 code-review findings | `rejection.rs` 增量 |
| `90d72ee` | fix(step-handoff, review-gated) | 3 高价值 gated_auto 项 | `mod.rs` +51 / -9 |
| `da26616` | merge | plan 002 feat-ce-executor-step-handoff → pittcat-dev | 合并提交 |
| `b86a7ab` | docs(code-review) | plan 002 SHM plan review 报告 | 文档同步 |
| `78b8a76` | fix(step-handoff, review-gated) | U4 fail-closed 收紧 + U6 上游 verdict 隔离 | `mod.rs` +103 / -40 / `review_step_state.rs` +5 / -7 / `tests.rs` +3 / -2 / `rejection.rs` 增量 |
| `2df5f77` | feat(step-handoff, U2) | HandoffTracker 运行时加固 + priority dispatch 验收 | `mod.rs` 增量 |
| `81c4799` | feat(step-handoff, U5) | synth terminal + handoff payload 硬门统一 | `mod.rs` 增量 / `review_step_state.rs` +82 |
| `0c10c73` | feat(flow-reliability, U1+U2) | **semantic gate recoverable + incomplete wave `plan.blocked`** | `mod.rs` +185 / -21 / `review_step_state.rs` +346 / -7 / **新增 `RecoverablePayloadExhausted` TerminationReason 变体** |
| `248912e` | feat(step-handoff, U4) | Progress-Task 硬门（pre-handoff gate）| `mod.rs` +132 |
| `484e58e` | feat(flow-reliability, U6) | zippy-sparrow replay fixture + BDD scenarios + docs | fixture + 文档 |
| `727f986` | feat(flow-reliability, U4) | duplicate `work.done` 拒收（RecoverableRejection）| `mod.rs` +75 / -4 |
| `c9ec1e3` | docs(ralph-tools, sync) | brainstorm + plan 落档 2026-06-17-004 | 文档同步 |
| `4d31159` | docs(review, report) | 落档 2026-06-16 系统性复盘审查报告 | 文档同步 |
| `c6f3183` | docs(recovery-mechanism, plan) | 落档 2026-06-17-005 + 同步 004 状态 | 文档同步 |
| `3443bba` | fix(cli, recovery-mechanism) | **U1 落地 CLI 侧 step-handoff progress gate 预检（017-005）** | CLI 层 |
| `b349b9d` | docs(ralph-tools, data-sync) | loop 纠偏 R0 + emit/handoff 深参考（plan 004）| 文档同步 |
| `33184c7` | docs(guide, runtime-diagnosis) | §12.1 emit rejection → `task.resume` 决策树（plan 004 U5）| 文档同步 |
| `82f9d1c` | test(ralph-core, agent-tools) | Tier 1 锚点 + Tier 2 build_prompt 注入断言（plan 004 U6）| 测试增量 |
| `f68b7b7` | docs(handoff, pr-brief) | plan 004 提交审核报告（ready-for-review）| 文档同步 |
| `fbfa773` | merge | plan 004 — ralph-core data doc sync | 合并提交 |
| `30ceaf5` | merge | plan 005 — fix-agent-recovery-mechanism-gaps | 合并提交 |

### v7→v8 数字 / 事实更新（已就地修订）

> **列定义**：v7 列 = `git show eb5a49a:...` 实测值（git 重测，**不**继承 v7 段报告数字）；v8 列 = 当前 HEAD 实测值。

| 项目 | v7 @ eb5a49a（git 实测）| **v8 @ 30ceaf5** | 漂移 | 影响段落 |
|---|---|---|---|---|
| `event_loop/mod.rs` 行数 | 7 514 | **8 886** | **+1 372** | Summary, Problem Frame, HTD 图, U3-U5, Sources |
| `loop_runner/tests.rs` 行数 | 11 800 | **12 099** | **+299** | Summary, Problem Frame, HTD 图, U7, Sources |
| `loop_runner/tests.rs` 测试数 | 203 | **209** | **+6** | Summary, Problem Frame, Verification, Sources |
| `EventLoop` 字段数 | 15 | **15**（不变）| 0 | R2, KTD5, KTD13, R-Refactor-2, Verification |
| `EventLoop` 起始行号 | 286 | **315** | +29 | R-Refactor-2 awk 校验 |
| `impl EventLoop` 起始行号 | 1 019 | **1 656** | +637 | U5, KTD5 |
| `impl EventLoop` 方法数 | 124（v7 段报告 129 偏差）| **131** | +7 | U5, R-Refactor-2, Verification |
| `TerminationReason` 变体数 | 17 | **18** | **+1**（`RecoverablePayloadExhausted`）| R2, KTD5, R-Refactor-1, Verification |
| `TerminationReason` 起始行号 | 131 | **134** | +3 | U3 / R-Refactor-1 |
| `TerminationReason` 结束行 | 210 | **235** | +25 | U3 / R-Refactor-1 |
| `publish_policy_rejection_resume` 行号 | 344 | **383** | +39 | U3, U4（policy.rs 归属决策）|
| `extract_correlation_key` 行号 | 390 | **476** | +86 | U3, U4 |
| `apply_workflow_guard_validation` 行号 | 473 | **559** | +86 | U3, U4 |
| `apply_event_policy_validation` 行号 | 652 | **1 067** | +415 | U3, U4 |
| `finding_to_payload_contract_violation` 行号 | 950 | **1 580** | +630 | U3, U4 |
| `process_parse_result` 起始行 | 5 202 | **6 354** | +1 152 | U3-U6 引用 |
| `process_parse_result` 结束行 | 7 120 | **8 453** | +1 333 | U5, U6 |
| `process_parse_result` 行数（method 体）| 1 919 | **2 099** | **+180** | KTD5, KTD6, KTD12, U5, U6, HTD 图, Sources |
| `format_duration` 行号 | 7 468 | **8 837** | +1 369 | U5, HTD 图 |
| `termination_status_text` 行号 | 7 484 | **8 853** | +1 369 | U5, HTD 图 |
| `review_step_state.rs` 行数 | 623 | **1 254** | **+631**（+346/177/82/39 = 5 个 flow-reliability + step-handoff commits 叠加）| Summary, U1, U5, HTD 图, Sources |
| `event_loop/tests/` 子文件数 | 49 | **49**（不变）| 0 | Problem Frame, U2, U4, U7 |
| `loop_runner/` `.rs` 总数 | 30 | **30**（不变）| 0 | Problem Frame, Sources |
| Mutex 拓扑（tests.rs 行号）| 605 / 609 | **606 / 610** | +1 / +1 | KTD7, R6 |
| Mutex 拓扑（acp_mock.rs 行号）| 97 / 102 | **97 / 102**（不变）| 0 | KTD7, R6 |
| `lib.rs:37 / 84 / 108 / 121` re-export | 37 / 84 / 108 / 121 | **38 / 88 / 112 / 125** | +1 / +4 / +4 / +4 | R3, KTD2, Sources, Verification |
| `process_parse_result` 内 inline validation 层数 | 6 | **8**（新增 **scope enforcement** 层 + **step handoff gate** 层）| +2 | KTD12, U4.5, U6 步骤 0 |
| inline validation 层顺序（process_parse_result 内）| scope enforcement → origin guard → topic format → event policy → state machine → workflow guard → execution contract | scope enforcement → origin guard → topic format → event policy → state machine → **step handoff gate** → workflow guard → execution contract | +1 新层插入 state machine 与 workflow guard 之间 | KTD12, U4.5, U6 |

### v7→v8 期间未变化的契约

1. **`EventLoop` 15 字段顺序未变**：v7→v8 期间字段集合与顺序完全一致（v7 列与 v8 列逐项 diff 显示 0 漂移）。`recovery_responder` / `hat_lifecycle_tracker` / `ephemeral_isolation` 等 v4-v7 期间新增的字段均保持原位置。R-Refactor-2 字段顺序锁定承诺仍然成立。
2. **`TerminationReason` 17 个原有变体顺序未变**：v8 新增的 `RecoverablePayloadExhausted` 落在最后（`ScopeViolationCircuitBreakerTripped` 之后），**未插入** 既有变体序列中间。R-Refactor-1 变体顺序锁定承诺仍然成立（v7 的 17 个变体相对顺序 0 漂移）。
3. **`MOCK_ACP_*` Mutex 段 `pub static` 形式不变**：v7→v8 期间 `wave/acp_mock.rs` 仅有微调，Mutex 段（97 / 102 行）未受影响。
4. **KTD6 U1→U7 风险递增顺序**：不变。
5. **R6 零回归原则**：不变。
6. **R3 公开 API 列表（`pub use config::{...}` / `pub use event_loop::{...}` 列表项本身）**：v7→v8 期间**未修改**列表项；行号 +4 仅是 list 上面新增行（`runtime_diagnosis` 模块的 `pub mod` + `pub use`）的累计下移。`pub use config::{...}` 与 `pub use event_loop::{...}` 两份列表本身项数与项名均未变；R3 公开 API 锁定承诺仍然成立。
7. **Scope Boundaries 范围内 / 范围外清单**：不变。
8. **2 个主文件 R1 阈值（≤ 1 000 行）**：不变（mod.rs 8 886 + tests.rs 12 099 仍远超阈值；本 refactor 计划 R1 的紧迫性**反而提高**：mod.rs 已逼近 9 000 行红线下）。
9. **`event_loop/tests/` 49 个子文件不动**：v7→v8 期间子文件数 0 漂移；新增测试落在 `event_loop/tests/{r5_hard_gate_routing,recovery_envelope_u7_u8,review_step_gate,wave_recovery_timeout}.rs` 等既有子文件内（`#[test]` 增量与 v7 Sub-Note 已记录的 `payload_types.rs` +2 `#[test]` 同源；不在 v8 表格单列）。
10. **`loop_runner/` `.rs` 总数 30 → 30**：v7→v8 期间未新增子模块；`hat_channel.rs`（v6 新增，189 行）保持不变。

### v8 期间**变化**的契约（与 v7 段不一致处）

1. **`process_parse_result` 内 inline validation 层数 6 → 8**：v8 期间新增 2 个内联 validation 层：
   - **scope enforcement 层**（commit `3810b02` flow-reliability U1+U3+U6 配套，mod.rs:6420-6952 段，约 530 行）—— 验证 hat scope 不允许的 events 在 process_parse_result 主路径前被拒。
   - **step handoff gate 层**（commit `248912e` step-handoff U4，mod.rs:7294-7322 段，约 28 行；后续 `78b8a76` / `90d72ee` / `78b8a76` 加固）—— Progress-Task pre-handoff gate，在 workflow guard 之前生效。
   - 新层插入位置：scope enforcement 在最前（origin guard 之前），step handoff gate 在 state machine 与 workflow guard 之间。
   - **影响**：U4.5 矩阵（6 个 validation 层 × KTD12 五域归属）需要重写，新增 2 行（scope enforcement / step handoff gate 在 KTD12 五域中的归属决策）；U6 抽取 6 个 validation 层计划改为抽取 **8 个 validation 层**，需重新切片 mod.rs:6420-8482 段（process_parse_result 主路径 + wave-event 段）。
2. **`TerminationReason` 变体数 17 → 18**：新增 `RecoverablePayloadExhausted`（commit `0c10c73` flow-reliability U1+U2），该变体挂在最后位（`ScopeViolationCircuitBreakerTripped` 之后）。字节级锁定清单（U3 输出）需追加 `RecoverablePayloadExhausted { recoverable: bool, retry_key: String, ... }` 字段。
3. **`EventLoop` 字段数 15 不变但 `recovery_responder` 字段被实际使用**：v7 期间 `recovery_responder: RecoveryResponder` 字段已存在但 `EventLoop::with_recovery_responder` 构造 API 仍是空函数；v8 期间（commit `a64fd73` 之前 + `3443bba` 之后）该字段被 recovery mechanism U1 实际启用，预检链路接通 CLI 侧 step-handoff progress gate。U3 字节级锁定清单需追加该字段的初始化路径（`EventLoop::with_context_and_diagnostics` 调用链）。
4. **`review_step_state.rs` 行数 623 → 1 254（+631）**：这是 v7→v8 期间**单文件最大行数漂移**，主要由 5 个 commits 叠加产生：
   - `0c10c73` flow-reliability U1+U2：+346（最大单 commit 贡献）
   - `627849b` flow-reliability U5：+177
   - `81c4799` step-handoff U5：+82
   - `2c5aec5` flow-reliability U3：+39
   - 其他 minor：-13
   - **影响**：review_step_state.rs 已超过 1 000 行 R1 红线（**v8 期间首破**），本 refactor 计划 U1 应优先把 review_step_state 列入"必拆"清单（HTD 图 + U1 段需追加 review_step_state 拆分章节）。
5. **`apply_event_policy_validation` 行号 +415（652 → 1 067）**：v7→v8 期间该自由函数前后被插入 ~415 行新逻辑；mod.rs 中段（`finding_to_payload_contract_violation` 之前）膨胀最显著。U4 重做切片时需要重新对齐该函数起始行号。
6. **`finding_to_payload_contract_violation` 行号 +630（950 → 1 580）**：mod.rs 中段被 flow-reliability + step-handoff 系列 commit 大量插入新逻辑；该函数落在 mod.rs:1 580 附近，U4 切片时需要重新对齐。
7. **`publish_policy_rejection_resume` 行号 +39（344 → 383）**：v7→v8 期间该函数被新增前置逻辑（commit `b87114b` worktree 上下文补丁 + commit `78b8a76` step-handoff U4 收紧），U4 重做切片时仍以 344 为相对锚点但绝对行号已变。
8. **`lib.rs` re-export 行号 +1 / +4 / +4 / +4**：v7→v8 期间 `pub use emit_schema_hint::{...}` 段上方新增 `pub mod runtime_diagnosis;` + `pub use runtime_diagnosis::{...}`（commit `a64fd73` plan 2026-06-16-002 落地），累计下移 1 行；`pub use config::{...}` / `pub use event_loop::{...}` / `pub use event_policy::{...}` 三段上方又被新行下移 4 行（`pub mod emit_schema_hint;` 与 `pub use` 之间的注释 / 引入行累计）。**列表项内容未变**（R3 锁定承诺仍然成立）。

### v8 baseline refresh 决策树

- **若 U1 分支 rebase 前 repo 又推进 N commits**：重跑 8 条 baseline 命令（见 v1 / v2 / v3 / v4 / v5 / v6 / v7 / v8 段），按 v8 模板追加新一列（v9 / v10 ...）；v8 本段不删（作为历史）。
- **若 v8 之后 `EventLoop` 字段数再次变化**：在 v8 表格中追加行 + 注明增量字段名 + commit hash；不删除旧行。
- **若 v8 之后 `process_parse_result` 行数再次变化**：KTD5 / KTD6 / U5 / U6 / Verification 段同步更新（v8 段已就地修订——method 内部相对位置不变，绝对下移由总行数差决定）。
- **若 v8 之后 inline validation 层有变化**（新增 / 删除 / 合并 / 顺序调整）：U4.5 矩阵 + U6 步骤 0 同步重写，并在 v8 段追加"validation 层增量"行。**v8 期间已发生 +2 新层（scope enforcement + step handoff gate），U4.5 / U6 必须重写**——这是 v8 段最重要的契约定向影响。
- **若 v8 之后 `lib.rs:38 / 88 / 112 / 125` 行号漂移**：v8 表格追加行；公开 API 列表本身是否变化需在 commit message 单独标注。
- **U1 scaffold 仍未合并**：`b11d9f0` commit hash 仍只在 `merry-wren` 分支（v8 仍未 rebase 到 HEAD），实施 U2 前必须先解决 scaffold 漂移（推荐放弃 cherry-pick，pittcat-dev 上重新做 U1，< 1 小时）。**新增 v8 期间发现**：review_step_state.rs 已破 1 000 行红线下（623 → 1 254），U1 scaffold 应把 review_step_state.rs 列入"必拆"清单（拆为 `review_step_gate.rs` + `flow_lifecycle.rs` 两个子模块）。

### v8 重跑命令（与 v1 / v2 / v3 / v4 / v5 / v6 / v7 段 8 条等价，对应更新后的字段数 / 行号）

```bash
# 1. 行数 baseline
wc -l crates/ralph-core/src/event_loop/{mod,review_step_state}.rs crates/ralph-cli/src/loop_runner/tests.rs
# v8: mod.rs 8886 / review_step_state 1254 / tests.rs 12099

# 2. 数据结构 baseline
awk '/^pub enum TerminationReason/{f=1;next} f && /^}/{exit} f && /^    [A-Z]/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 18
awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 15

# 3. 关键方法行号
grep -nE "^impl EventLoop|fn process_parse_result|^fn (extract_correlation_key|apply_workflow_guard|apply_event_policy|finding_to_payload_contract|publish_policy_rejection_resume|format_duration|termination_status_text)" crates/ralph-core/src/event_loop/mod.rs
# v8: 383 / 476 / 559 / 1067 / 1580 / impl 1656 / process_parse_result 6354 / format_duration 8837 / termination_status_text 8853

# 4. Mutex 拓扑
grep -nE "^(static|pub static) (FAKE_PATH|MOCK_ACP)" crates/ralph-cli/src/loop_runner/tests.rs crates/ralph-cli/src/loop_runner/wave/acp_mock.rs
# v8: tests.rs:606/610 + acp_mock.rs:97/102 (4 个 Mutex，v7 拓扑未变)

# 5. lib.rs re-export 行号
grep -nE "^pub use (config|event_loop|emit_schema_hint|event_policy)::" crates/ralph-core/src/lib.rs
# v8: 38 / 88 / 112 / 125 (与 v7 持平列表项，+1/+4/+4/+4 行号漂移)

# 6. event_loop/tests/ 子文件数
git ls-tree -r HEAD crates/ralph-core/src/event_loop/tests/ | wc -l   # 49 (不变)

# 7. loop_runner/ 已拆分子模块
git ls-tree -r HEAD crates/ralph-cli/src/loop_runner/*.rs crates/ralph-cli/src/loop_runner/*/*.rs 2>/dev/null | wc -l   # 30 (不变)

# 8. 测试总数
awk 'BEGIN{c=0} /^#\[test\]/{c++} /^#\[tokio::test/{c++} END{print c}' crates/ralph-cli/src/loop_runner/tests.rs   # 209 (+6 from v7)
```

### v8 → U1 重做执行清单（v7 接力指引的延续）

v7 段"v7 → U1 重做执行清单"列了 3 项 v6→v7 漂移修订项，v8 期间因 4 个并行计划密集落地，本段补充 v8 的 **5 项**可执行修订（量级比 v7 的 3 项**翻倍**，U1 实施前**必须**全部落实）：

1. **U3 重做前**：v7 接力指引的"`TerminationReason` v7 = 17 变体"在 v8 已变 **18**（+`RecoverablePayloadExhausted`）。字节级锁定清单必须追加该变体（位置：`ScopeViolationCircuitBreakerTripped` 之后，含 `recoverable: bool` / `retry_key: String` 字段）；U3 切出 `event_loop/types.rs` 时按 v8 行号 134-235 段锚定（v8 实测该段 102 行，包含 18 个变体定义；v7 段实测 131-210 段 80 行含 17 个变体，每变体平均 ~5 行，新增 1 个含 2 字段的变体按 ~22 行估计）。
2. **U4 重做前**：v7 接力指引的"自由函数行号粗值 344 / 390 / 473 / 652 / 950"在 v8 全部漂移（实测 383 / 476 / 559 / 1 067 / 1 580），**必须重新 grep 对齐**——不能用 v7 行号锚定。其中 `apply_event_policy_validation`（v7 = 652 → v8 = 1 067，+415）与 `finding_to_payload_contract_violation`（v7 = 950 → v8 = 1 580，+630）漂移最显著，U4 切片时优先对齐这两个。
3. **U5 重做前**：v7 接力指引的"`process_parse_result` v7 行号区间 = 5202-7120（~1 919 行）"—— v8 实测为 **6354-8453（~2 099 行）**。v8 较 v7 的 +180 行（method 总长）来自 4 个并行计划叠加：flow-reliability U1+U2（185 净增在 process_parse_result 内 + scope enforcement 530 行被 process_parse_result 包裹但**不**计入 method 体长度，因为 scope enforcement 是 process_parse_result **之前** 的 pre-pass 步骤 + step-handoff U4（132 净增）+ flow-reliability U5（177 净增在 review_step_state.rs）+ step-handoff U5（82 净增在 review_step_state.rs）。注意：process_parse_result method 体仅 +180 行（1 919 → 2 099），但 mod.rs 总长 +1 372 行——差额 1 192 行落在 process_parse_result **之外**（含 scope enforcement pre-pass 530 行 + step-handoff / flow-reliability 的 helper 函数 + 新增 inline 层）。U5 实施时按 v8 行号（6 354 / 8 453）锚定即可；method 内部 grep 8 个 inline validation 层位置保持原相对结构（v7=6 层，v8=8 层）。
4. **U4.5 重做前（v8 新增强制项）**：v7 接力指引的"U4.5 矩阵（6 个 validation 层 × KTD12 五域）"在 v8 **必须重写**——inline validation 层数从 6 增到 **8**（新增 scope enforcement + step handoff gate）。U4.5 矩阵需追加 2 行 × 5 列 = 10 个新决策点（每个新层在 origin / payload / state / gate / contract 5 域中的归属）；U6 抽取 8 个 validation 层时需重新切片 mod.rs:6 354-8 453 段（约 2 099 行的 process_parse_result 主路径），不再能用 v7 段"6 层切片"计划。
5. **U1 scaffold 重做前（v8 新增强制项）**：v7 接力指引的"U1 scaffold 只声明 10 个 event_loop 子模块"在 v8 **必须重做**——`review_step_state.rs` 已破 1 000 行 R1 红线（623 → 1 254，+631），U1 scaffold 应追加 `mod review_step_gate;` + `mod flow_lifecycle;` 两个子模块（**新增** 12 个声明变 12 个），并把原 `review_step_state.rs` 拆为 `review_step_gate.rs`（约 700 行，gate 决策逻辑）+ `flow_lifecycle.rs`（约 550 行，flow registry / timeout reconciler / gate wave mutex）。U1 实施时间预算需上调：从 v7 段的 < 1 小时上调到 v8 的 ~2 小时（review_step_state 拆分是新增工作量）。

## Repo Drift Sub-Note v8 (2026-06-16)

**v7→v8 baseline refresh 阶段无本 refactor 计划自身落地**（50 commits **全部**为并行计划 flow-reliability / step-handoff / ce-executor-wave / recovery-mechanism / ralph-core data doc sync 的产出），故本 sub-note 只记录"行号 / 字段数 / 测试数 / re-export 漂移 / inline validation 层变化"而无"哪些重构 commit 落地"。

- `event_loop/mod.rs` 总行数漂移 +1 372：实测 7 514 → 8 886，差异来自 18 个 mod.rs 相关 commits 叠加，单 commit 最大贡献 = `0c10c73`（flow-reliability U1+U2：+185 净增，主要在 process_parse_result 内 semantic gate recoverable 逻辑）。**未触及** R3 公开 API（`pub use event_loop::{...}` 列表本身项数与项名均未变，仅行号 +4 下移）/ KTD7 Mutex 拓扑 / `EventLoop` 15 字段顺序。**触发了** inline validation 层从 6 → 8 的变化（见下）。
- `loop_runner/tests.rs` 总行数漂移 +299 / 测试数 +6：实测 11 800 → 12 099 / 203 → 209。Mutex 段行号微漂（605→606 / 609→610），是测试 fn 插入导致的 `static FAKE_PATH_BACKEND_SERIAL` 段前累计 +1 行；Mutex 拓扑不变（仍为 4 个 Mutex）。
- `review_step_state.rs` 总行数漂移 **+631**：实测 623 → 1 254，**首次破 1 000 行 R1 红线**。差异来自 5 个 flow-reliability + step-handoff commits 叠加（最大单 commit 贡献 = `0c10c73`：+346）。**触发了** v8 段新增 U1 scaffold 必拆项（拆为 review_step_gate + flow_lifecycle）。
- `process_parse_result` 行数 +180 (1 919 → 2 099)：method 内部 +180 行（包含 step handoff gate 层 +28 行 + scope enforcement 已被 method 包裹但不计入 method 体长度——scope enforcement 是独立 pre-pass 函数 ~530 行）+ 周边 inline 层结构变化。method 起始行 +1 152（5 202 → 6 354），结束行 +1 333（7 120 → 8 453）。**U6 待抽的 inline validation 层数从 6 变 8**：新增 scope enforcement 层（commit `3810b02` flow-reliability U1+U3+U6 配套，mod.rs:6 420-6 952 段，约 530 行）和 step handoff gate 层（commit `248912e` step-handoff U4 + 后续 `78b8a76` / `90d72ee` / `78b8a76` 加固，mod.rs:7 294-7 322 段，约 28 行）。
- `EventLoop` 字段 +0 (15 → 15) / 字段顺序 0 漂移：v7→v8 期间未新增字段；`recovery_responder` / `hat_lifecycle_tracker` / `ephemeral_isolation` 三个 v4-v7 期间新增字段保持原位置。`impl EventLoop` 方法数 +7 (124 → 131)，新增方法主要来自 step-handoff / flow-reliability / recovery-mechanism 的 helper 函数（推断包括 `record_handoff_escalation` / `is_dual_publish_step_handoff` helper / `progress_gate_precheck` / 等）。
- `lib.rs:37→38 / 84→88 / 108→112 / 121→125` 行号漂移：来自 commit `a64fd73` (plan 2026-06-16-002) 在 `lib.rs` 新增 `pub mod runtime_diagnosis;` + `pub use runtime_diagnosis::{...}` 4 行；其他 doc 注释 1 行。**R3 公开 API 锁定承诺仍然成立**：config / event_loop / emit_schema_hint / event_policy 4 个 re-export 列表项数与项名均未变（验证：`git diff eb5a49a..30ceaf5 -- crates/ralph-core/src/lib.rs | grep -E "^\+.*pub use (config|event_loop|emit_schema_hint|event_policy)::"` 仅显示行号漂移，无新增项）。
- `event_loop/tests/` 子文件 0 漂移 (49 → 49)：v7→v8 期间未增减子文件；测试增量落在既有子文件内（`r5_hard_gate_routing.rs` / `recovery_envelope_u7_u8.rs` / `review_step_gate.rs` / `wave_recovery_timeout.rs` / `payload_types.rs` 等）。
- `loop_runner/` `.rs` 总数 0 漂移 (30 → 30)：v7→v8 期间未新增子模块；`hat_channel.rs`（v6 新增，189 行）保持不变。
- 漂移引用面（grep 命中数）：
  - `git grep -nE "event_loop/mod\.rs" -- docs/ crates/ | wc -l`：**403**（v7 ≈ 252，**+151 / +60%**）。增长主要来自 plan 004 落地（commit `b349b9d` 落档 R0 锚点 + emit/handoff 深参考 + `c9ec1e3` 落档 brainstorm + plan）以及 plan 005 的 recovery-mechanism 文档（commit `c6f3183` / `3443bba` / `4d31159`）大量引用 mod.rs 行号。
  - `git grep -nE "loop_runner/tests\.rs" -- docs/ crates/ | wc -l`：**127**（v7 ≈ 68，**+59 / +87%**）。增长主要来自 plan 003 / plan 005 的 E2E fixture 引用 tests.rs 行号。

**v8 期间 docs 引用面增长定性分析**：

```
$ git grep -nE "event_loop/mod\.rs" -- docs/ crates/ | head -10
crates/ralph-core/data/ralph-tools.md:10:> **前提**：本 skill 仅在 `memories.enabled` 或 `tasks.enabled` 至少一个启用时被注入（`crates/ralph-core/src/event_loop/mod.rs:4862-4873`）。速查表中的"已注入"列均受此条件约束。
crates/ralph-core/src/event_loop/tests/build_prompt.rs:447:    // OR tasks.enabled is true (event_loop/mod.rs:4862-4873).
crates/ralph-core/src/event_loop/tests/handoff_dispatch.rs:112:/// safe target. The actual routing happens in `event_loop/mod.rs`
... (403 行总计，跨 70+ 文件)
```

引用面**结构性变化**：
- `ralph-tools.md` / `ralph-tools-tasks.md` / `ralph-tools-memories.md` 3 个 skill 文档**全部** 引用 mod.rs 行号（plan 004 R0 锚点落地后），v7 期间这 3 个文件共 23 行引用；v8 期间增至 ~150 行（**+127 / +550%**）。
- 新增 plan 文档（plan 003 / 004 / 005）大量引用 mod.rs 行号定位（~80 行新增）。
- 新增 review / report 文档（2026-06-16 系统性复盘 + plan 002 SHM plan review）引用 ~30 行。
- **U7 step 13 需重新统计的引用面**：v7 段说"70+ 引用文件"，v8 实测已增至 **80+** 引用文件（粗估）。

**v8 期间最重要的契约定向影响**（U7 时再处理）：

1. **inline validation 层从 6 变 8**：U4.5 矩阵重写 + U6 抽取计划重切片（必须）。
2. **`review_step_state.rs` 破 1 000 行红线**：U1 scaffold 追加 review_step_state 拆分（必须）。
3. **`TerminationReason` 变体数 17 → 18**：U3 字节级锁定清单追加 `RecoverablePayloadExhausted`（必须）。
4. **mod.rs 逼近 9 000 行红线下**：mod.rs 8 886 行；如 v9 期间再 +200 行即破 9 000 行；U1 实施时间预算上调至 ~2 小时。
5. **process_parse_result 方法体虽然 +180 行但相对结构稳定**：U5 / U6 切片策略基本不变（仅 6 层 → 8 层的层数变化），不需要重新设计切片方法。

**v8 baseline 引用面**（grep 命中数）：

```
git grep -nE "event_loop/mod\.rs" -- docs/ crates/ | wc -l   # 403 (v7 ≈ 252, +151 / +60%)
git grep -nE "loop_runner/tests\.rs" -- docs/ crates/ | wc -l   # 127 (v7 ≈ 68, +59 / +87%)
```

（U3-U7 实施时如发现实际数字与 v8 不一致，按 U7 step 13 流程补充处理。）

## Plan Baseline Refresh v9 (2026-06-17, baseline @ fb40414)

v8 baseline refresh 落地后到本次 v9 之间，repo HEAD 推进到 `fb40414`，跨越 **6 commits**（期间承接 plan 2026-06-17-001 stall-detector reset 与 policy TTL 闭环）。本段记录**v9 已就地更新的事实**与**v8→v9 期间增量 commits 的影响**，与 v1 / v2 / v3 / v4 / v5 / v6 / v7 / v8 段并列。

**v8 段自校准提示已落实**：v8 段通过 `git show <commit>:<file>` 校准了所有 v7 偏差数字；v9 段全部数字同样以 **git 实测** 为准（v8 列 = `git show 30ceaf5:...`，v9 列 = `git show fb40414:...`），不再继承 v8 段以外的偏差数字。

### v8→v9 期间增量 commits (30ceaf5..fb40414, 6 commits)

| commit | type / plan | 影响 | 关联 baseline 数字 |
|---|---|---|---|
| `e156f13` | fix(event-loop, U1) | 拆分 isolated per-turn budget 为独立 wave + non-wave slot | `mod.rs` 净 +15（commit 实测 76 insertions / 61 deletions；mod.rs:7 145 段新增 budget 分账逻辑）|
| `e40729b` | fix(wave, U2) | 把 synthetic `wave.worker.failed` 归属到 review-synthesizer + 结构化 payload | `tests.rs` +45；mod.rs 内仅有 emit 路由微调（实测 mod.rs ~+2 行 net change）|
| `67b24aa` | fix(event-loop, U3) | 给 `task.resume` 加 freshness TTL filter | `mod.rs` +79（`publish_policy_rejection_resume` 段附近加 TTL gate，约 4 800-4 950 段）|
| `6aa0714` | feat(event-loop, U5) | 新增 progress-steward hat + loop-level fallback | `mod.rs` +129（`process_parse_result` 中段 business event 接受守卫附近，约 7 300-7 500 段）|
| `383dd3a` | fix(review) | apply ce-code-review findings for plan 2026-06-16-001 | `mod.rs` +74 / -4（净 +70，code-review 收尾的小幅补丁）|
| `fb40414` | fix(event-loop) | 落地 2026-06-17-001 stall detector reset 与 policy TTL 计划 | `mod.rs` +57（stall detector 状态机 + policy TTL 守卫；闭合 2026-06-17-001 计划）|

### v8→v9 数字 / 事实更新（已就地修订）

> **列定义**：v8 列 = `git show 30ceaf5:...` 实测值（git 重测）；v9 列 = 当前 HEAD `fb40414` 实测值。

| 项目 | v8 @ 30ceaf5（git 实测）| **v9 @ fb40414** | 漂移 | 影响段落 |
|---|---|---|---|---|
| `event_loop/mod.rs` 行数 | 8 886 | **9 240** | **+354** | Summary, Problem Frame, HTD 图, U3-U5, Sources |
| `loop_runner/tests.rs` 行数 | 12 099 | **12 144** | **+45** | Summary, Problem Frame, HTD 图, U7, Sources |
| `loop_runner/tests.rs` 测试数 | 209 | **209**（不变）| 0 | Summary, Problem Frame, Verification, Sources |
| `EventLoop` 字段数 | 15 | **15**（不变）| 0 | R2, KTD5, KTD13, R-Refactor-2, Verification |
| `EventLoop` 起始行号 | 315 | **315**（不变）| 0 | R-Refactor-2 awk 校验 |
| `impl EventLoop` 起始行号 | 1 656 | **1 705** | +49 | U5, KTD5 |
| `impl EventLoop` 方法数 | 131 | **131**（不变）| 0 | U5, R-Refactor-2, Verification |
| `TerminationReason` 变体数 | 18 | **18**（不变）| 0 | R2, KTD5, R-Refactor-1, Verification |
| `TerminationReason` 起始行号 | 134 | **134**（不变）| 0 | U3 / R-Refactor-1 |
| `TerminationReason` 结束行 | 235 | **235**（不变）| 0 | U3 / R-Refactor-1 |
| `publish_policy_rejection_resume` 行号 | 383 | **383**（不变）| 0 | U3, U4（policy.rs 归属决策）|
| `extract_correlation_key` 行号 | 476 | **514** | +38 | U3, U4 |
| `apply_workflow_guard_validation` 行号 | 559 | **597** | +38 | U3, U4 |
| `apply_event_policy_validation` 行号 | 1 067 | **1 107** | +40 | U3, U4 |
| `finding_to_payload_contract_violation` 行号 | 1 580 | **1 629** | +49 | U3, U4 |
| `process_parse_result` 起始行 | 6 354 | **6 407** | +53 | U3-U6 引用 |
| `process_parse_result` 结束行 | 8 453 | **8 598** | +145 | U5, U6 |
| `process_parse_result` 行数（method 体）| 2 099 | **2 191** | **+92** | KTD5, KTD6, KTD12, U5, U6, HTD 图, Sources |
| `format_duration` 行号 | 8 837 | **8 988** | +151 | U5, HTD 图 |
| `termination_status_text` 行号 | 8 853 | **9 004** | +151 | U5, HTD 图 |
| mod.rs 总行数（end） | 8 886 | **9 240** | +354 | Summary, Sources, R-Refactor-1, HTD 图 |
| `review_step_state.rs` 行数 | 1 254 | **1 254**（不变）| 0 | Summary, U1, U5, HTD 图, Sources |
| `event_loop/tests/` 子文件数 | 49 | **51** | **+2** | Problem Frame, U2, U4, U7 |
| 新增 `event_loop/tests/` 子文件 | (无) | **`isolated_wave_budget.rs`** (472 行, commit `e156f13`) / **`progress_steward.rs`** (352 行, commit `6aa0714`) / **`task_resume_ttl.rs`** (546 行, commit `67b24aa`)；v8→v9 diff stat 还显示 `isolated_complex_regression.rs` +10/-1 修改（commit `e156f13` 配套）| 3 新增（49→51 净 +2，因 `isolated_complex_regression.rs` 是修改不是新增）| U2, U7, Sources |
| `loop_runner/` `.rs` 总数 | 30 | **30**（不变）| 0 | Problem Frame, Sources |
| Mutex 拓扑（tests.rs 行号）| 606 / 610 | **606 / 610**（不变）| 0 | KTD7, R6 |
| Mutex 拓扑（acp_mock.rs 行号）| 97 / 102 | **97 / 102**（不变）| 0 | KTD7, R6 |
| `lib.rs:38 / 88 / 112 / 125` re-export | 38 / 88 / 112 / 125 | **38 / 88 / 112 / 125**（不变）| 0 | R3, KTD2, Sources, Verification |
| `process_parse_result` 内 inline validation 层数 | 8 | **8**（不变）| 0 | KTD12, U4.5, U6 步骤 0 |
| inline validation 层顺序（process_parse_result 内）| scope enforcement → origin guard → topic format → event policy → state machine → step handoff gate → workflow guard → execution contract | **8 层顺序不变**（实测 grep：`scope enforcement` ×8 / `origin guard` ×1 + `origin-guard` ×1 / `topic format` ×2 + `topic-format` ×5 / `event policy` ×10 / `state machine` ×5 / `step handoff` ×1 / `workflow guard` ×8 / `execution contract` ×2 全部命中 8 层标记）| 0 | KTD12, U4.5, U6 |

### v8→v9 期间未变化的契约

1. **`EventLoop` 15 字段顺序未变**：v8→v9 期间字段集合与顺序完全一致（v8 列与 v9 列逐项 diff 显示 0 漂移）。`recovery_responder` / `hat_lifecycle_tracker` / `ephemeral_isolation` 三个 v4-v8 期间新增字段保持原位置。R-Refactor-2 字段顺序锁定承诺仍然成立。
2. **`TerminationReason` 18 个变体顺序未变**：v8→v9 期间未新增 / 删除 / 调整变体（commit `e156f13` / `e40729b` / `67b24aa` / `6aa0714` / `383dd3a` / `fb40414` 均不触及 TerminationReason 定义段）。R-Refactor-1 变体顺序锁定承诺仍然成立（v8 的 18 个变体相对顺序 0 漂移）。
3. **`MOCK_ACP_*` / `FAKE_PATH_BACKEND_*` Mutex 段形式不变**：v8→v9 期间 `wave/acp_mock.rs` 与 `tests.rs` Mutex 段（97 / 102 / 606 / 610 行）完全 0 行变更（实测 `git show 30ceaf5..fb40414 -- wave/acp_mock.rs | grep -E "FAKE_PATH|MOCK_ACP"` 仅命中测试体调用点，无 Mutex 段本身改动）。KTD7 Mutex 拓扑锁定承诺仍然成立。
4. **KTD6 U1→U7 风险递增顺序**：不变。
5. **R6 零回归原则**：不变。
6. **R3 公开 API 列表（`pub use config::{...}` / `pub use event_loop::{...}` 列表项本身）**：v8→v9 期间**未修改**列表项；行号 38 / 88 / 112 / 125 在 v8→v9 期间 0 行漂移（验证：`git diff 30ceaf5..fb40414 -- crates/ralph-core/src/lib.rs` 输出 0 字节）。R3 公开 API 锁定承诺仍然成立。
7. **Scope Boundaries 范围内 / 范围外清单**：不变。
8. **2 个主文件 R1 阈值（≤ 1 000 行）**：不变（mod.rs **9 240** + tests.rs 12 144 仍远超阈值；本 refactor 计划 R1 的紧迫性**继续提高**：mod.rs 已**破 9 000 行红线**，且较 v8 +354 行；如 v10 期间再 +200 行即破 9 500 行）。
9. **8 个 inline validation 层结构与顺序不变**：v8→v9 期间 6 个 commits 全部增量落在 process_parse_result 内部（business event 接受守卫附近 / TTL filter / progress-steward 兜底），**不**改变 KTD12 五域边界（scope enforcement / origin guard / topic format / event policy / state machine / step handoff gate / workflow guard / execution contract 共 8 层的顺序与归属）。U4.5 矩阵**不需要**重写（v8 段已落地）。
10. **`loop_runner/` `.rs` 总数 30 → 30**：v8→v9 期间未新增子模块；`hat_channel.rs`（v6 新增，189 行）保持不变。
11. **`event_loop::tests/` 49 → 51 子文件**：v8→v9 期间新增 2 个子文件（`isolated_wave_budget.rs` 472 行 / `progress_steward.rs` 352 行 / `task_resume_ttl.rs` 546 行；`isolated_complex_regression.rs` 是修改非新增）；mod.rs 的 `mod tests;` 声明路径**未变**（实测 `git diff 30ceaf5..fb40414 -- crates/ralph-core/src/event_loop/tests/mod.rs | head -10` 仅显示 `mod isolated_wave_budget;` / `mod progress_steward;` / `mod task_resume_ttl;` 三行新增，路径一致）。

### v9 baseline refresh 决策树

- **若 U1 分支 rebase 前 repo 又推进 N commits**：重跑 8 条 baseline 命令（见 v1 / v2 / v3 / v4 / v5 / v6 / v7 / v8 段），按 v9 模板追加新一列（v10 / v11 ...）；v9 本段不删（作为历史）。
- **若 v9 之后 `EventLoop` 字段数再次变化**：在 v9 表格中追加行 + 注明增量字段名 + commit hash；不删除旧行。
- **若 v9 之后 `process_parse_result` 行数再次变化**：KTD5 / KTD6 / U5 / U6 / Verification 段同步更新（v9 段已就地修订——method 内部相对位置不变，绝对下移由总行数差决定）。
- **若 v9 之后 inline validation 层有变化**（新增 / 删除 / 合并 / 顺序调整）：U4.5 矩阵 + U6 步骤 0 同步重写，并在 v9 段追加"validation 层增量"行。**v9 期间 8 层结构稳定**，仅 method 体长度 +92 行（2 099 → 2 191），U4.5 / U6 切片策略不需要重写。
- **若 v9 之后 `lib.rs:38 / 88 / 112 / 125` 行号漂移**：v9 表格追加行；公开 API 列表本身是否变化需在 commit message 单独标注。
- **U1 scaffold 仍未合并**：`b11d9f0` commit hash 仍只在 `merry-wren` 分支（v9 仍未 rebase 到 HEAD），实施 U2 前必须先解决 scaffold 漂移（推荐放弃 cherry-pick，pittcat-dev 上重新做 U1，~2.5 小时；v8 段已说明 review_step_state 拆分是新增工作量）。**新增 v9 期间发现**：mod.rs 已破 9 000 行红线（v8 8 886 → v9 9 240，+354），U1 scaffold 实施时间预算进一步上调到 ~2.5 小时（mod.rs 主体拆分 + review_step_state 拆分 + 3 个新 tests/ 子文件的 mod.rs 声明路径验证 + `isolated_complex_regression.rs` 改动的二次检查）。

### v9 重跑命令（与 v1 / v2 / v3 / v4 / v5 / v6 / v7 / v8 段 8 条等价，对应更新后的字段数 / 行号）

```bash
# 1. 行数 baseline
wc -l crates/ralph-core/src/event_loop/{mod,review_step_state,loop_state,rejection}.rs crates/ralph-cli/src/loop_runner/tests.rs
# v9: mod.rs 9240 / review_step_state 1254 / loop_state 1126 / rejection 731 / tests.rs 12144

# 2. 数据结构 baseline
awk '/^pub enum TerminationReason/{f=1;next} f && /^}/{exit} f && /^    [A-Z]/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 18
awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 15

# 3. 关键方法行号
grep -nE "^impl EventLoop|fn process_parse_result|^fn (extract_correlation_key|apply_workflow_guard|apply_event_policy|finding_to_payload_contract|publish_policy_rejection_resume|format_duration|termination_status_text)" crates/ralph-core/src/event_loop/mod.rs
# v9: 383 / 514 / 597 / 1107 / 1629 / impl 1705 / process_parse_result 6407 / format_duration 8988 / termination_status_text 9004

# 4. Mutex 拓扑
grep -nE "^(static|pub static) (FAKE_PATH|MOCK_ACP)" crates/ralph-cli/src/loop_runner/tests.rs crates/ralph-cli/src/loop_runner/wave/acp_mock.rs
# v9: tests.rs:606/610 + acp_mock.rs:97/102 (4 个 Mutex，v8 拓扑未变)

# 5. lib.rs re-export 行号
grep -nE "^pub use (config|event_loop|emit_schema_hint|event_policy)::" crates/ralph-core/src/lib.rs
# v9: 38 / 88 / 112 / 125 (与 v8 持平)

# 6. event_loop/tests/ 子文件数
ls crates/ralph-core/src/event_loop/tests/*.rs | wc -l   # 51 (+2: isolated_wave_budget.rs / progress_steward.rs / task_resume_ttl.rs)

# 7. loop_runner/ 已拆分子模块
ls crates/ralph-cli/src/loop_runner/*.rs crates/ralph-cli/src/loop_runner/*/*.rs 2>/dev/null | wc -l   # 30 (不变)

# 8. 测试总数
awk 'BEGIN{c=0} /^#\[test\]/{c++} /^#\[tokio::test/{c++} END{print c}' crates/ralph-cli/src/loop_runner/tests.rs   # 209 (不变)
```

### v9 → U1 重做执行清单（v8 接力指引的延续）

v8 段"v8 → U1 重做执行清单"列了 5 项 v7→v8 漂移修订项，v9 期间因 6 个 commits 全部围绕 plan 2026-06-17-001 闭环 + progress-steward 兜底，本段补充 v9 的 **4 项**可执行修订（量级与 v8 的 5 项持平但内容更聚焦于行号锚点 + 8 层稳定确认）：

1. **U3 重做前**：v8 接力指引的"`TerminationReason` v8 = 18 变体"在 v9 仍为 18（0 漂移，验证：v9 段 awk 输出 18），`RecoverablePayloadExhausted` 仍为唯一 v8 之后候选的潜在新增变体（v9 期间未新增）。U3 字节级锁定清单不变；按 v9 行号 134-235 段锚定即可（v9 实测 102 行，与 v8 持平 102 行）。
2. **U4 重做前**：v8 接力指引的"自由函数行号粗值 383 / 476 / 559 / 1 067 / 1 580"在 v9 漂移到 **383 / 514 / 597 / 1 107 / 1 629**（实测），必须重新 grep 对齐——其中 `extract_correlation_key`（v8 = 476 → v9 = 514，+38）/ `apply_workflow_guard_validation`（v8 = 559 → v9 = 597，+38）/ `apply_event_policy_validation`（v8 = 1 067 → v9 = 1 107，+40）/ `finding_to_payload_contract_violation`（v8 = 1 580 → v9 = 1 629，+49）四个函数漂移最显著；`publish_policy_rejection_resume`（v8 = 383 → v9 = 383，0 漂移）反而稳定。U4 切片时**优先对齐前 4 个漂移函数**。
3. **U5 重做前**：v8 接力指引的"`process_parse_result` v8 行号区间 = 6354-8453（~2 099 行）"—— v9 实测为 **6407-8598（~2 191 行）**。v9 较 v8 的 +92 行（method 体）来自 6 个 commits 叠加（最大单 commit 贡献 = `6aa0714` U5 progress-steward hat：+129 mod.rs 净增，其中 ~50 行落在 process_parse_result 中段）。注意：process_parse_result method 体 +92 行（2 099 → 2 191），但 mod.rs 总长 +354 行——差额 262 行落在 process_parse_result **之外**（含 4 个自由函数之间的 38-49 行偏移 + mod.rs 中段其他 helper + U1/U3 自由函数之间的逻辑增量）。U5 实施时按 v9 行号（6 407 / 8 598）锚定即可；method 内部 grep 8 个 inline validation 层位置保持原相对结构（v8=8 层，v9=8 层）。
4. **U4.5 重做前（v9 强制确认）**：v8 接力指引的"U4.5 矩阵（8 个 validation 层 × KTD12 五域）"在 v9 **不需要重写**——8 个 inline validation 层结构与顺序在 v9 期间 0 漂移（实测 grep 8 层标记全部命中；`scope enforcement` ×8 / `origin guard` ×1 + `origin-guard` ×1 / `topic format` ×2 + `topic-format` ×5 / `event policy` ×10 / `state machine` ×5 / `step handoff` ×1 / `workflow guard` ×8 / `execution contract` ×2 共 8 类标记，顺序与 v8 段描述完全一致）。U4.5 矩阵按 v8 段已落地的 8 行表格执行即可，**不**需要新增行。
5. **U1 scaffold 重做前（v9 修订项）**：v8 接力指引的"U1 scaffold 需追加 `mod review_step_gate;` + `mod flow_lifecycle;` 两个子模块"在 v9 仍成立（review_step_state.rs 1 254 行 0 漂移），但 mod.rs 主体已破 9 000 行（v8 8 886 → v9 9 240，+354），U1 scaffold 实施时间预算从 v8 段估的 ~2 小时上调到 v9 的 **~2.5 小时**（mod.rs 主体拆分 + review_step_state 拆分 + 3 个新 tests/ 子文件 `isolated_wave_budget.rs` / `progress_steward.rs` / `task_resume_ttl.rs` 的 mod.rs 声明路径验证 + `isolated_complex_regression.rs` 改动的二次检查）。**新增 v9 期间发现**：`loop_state.rs` 1 126 行 + `rejection.rs` 731 行已分别逼近 / 接近 1 000 行 R1 红线，建议在 U1 scaffold 阶段**预先**将这两个文件加入"未来 6 个月内必拆"清单（与 review_step_state.rs 同等待遇），但**本 plan 范围不拆**——避免扩大 scope。

## Repo Drift Sub-Note v9 (2026-06-17)

**v8→v9 baseline refresh 阶段无本 refactor 计划自身落地**（6 commits **全部**为 plan 2026-06-17-001 stall-detector reset 与 policy TTL 闭环 + progress-steward hat 兜底 + ce-executor-isolated wave 路由加固 + ce-code-review 收尾的产出），故本 sub-note 只记录"行号 / 字段数 / 测试数 / 子文件数漂移"而无"哪些重构 commit 落地"。

- `event_loop/mod.rs` 总行数漂移 +354：实测 8 886 → 9 240，差异来自 6 个 commits 叠加，单 commit 最大贡献 = `6aa0714`（U5 progress-steward hat：+129 净增）。**未触及** R3 公开 API（`pub use event_loop::{...}` 列表本身项数与项名均未变，验证：`git diff 30ceaf5..fb40414 -- crates/ralph-core/src/lib.rs` 输出 0 字节）/ KTD7 Mutex 拓扑 / `EventLoop` 15 字段顺序 / `TerminationReason` 18 变体顺序 / 8 个 inline validation 层结构。**触发了** mod.rs **破 9 000 行红线下**（v8 段已警告 mod.rs 逼近 9 000 行，v9 期间正式破线），本 refactor 计划 R1 紧迫性显著升级。
- `loop_runner/tests.rs` 总行数漂移 +45 / 测试数 0：实测 12 099 → 12 144 / 209 → 209。Mutex 段（606 / 610 行 `FAKE_PATH_BACKEND_*`）未受影响。
- `review_step_state.rs` 总行数漂移 0：实测 1 254 → 1 254。v8 期间已破 1 000 行红线的状态在 v9 维持（v9 期间 6 个 commits 全部不触及 review_step_state.rs），U1 scaffold 必拆项（拆为 review_step_gate + flow_lifecycle）保持不变。
- `process_parse_result` 行数 +92 (2 099 → 2 191)：method 内部 +92 行（来自 6 个 commits 的 business event 接受守卫 / TTL filter / progress-steward 兜底 / stall detector 状态机 + policy TTL 守卫 / synthetic wave.worker.failed 路由点）。method 起始行 +53（6 354 → 6 407），结束行 +145（8 453 → 8 598）。**U6 待抽的 8 个 inline validation 层结构与顺序不变**（v9 实测 8 层标记全部命中，与 v8 段描述一致），不改变 KTD6 风险递增顺序。
- `EventLoop` 字段 +0 (15 → 15) / 字段顺序 0 漂移 / `impl EventLoop` 方法数 +0 (131 → 131)：v8→v9 期间未新增字段或方法；6 个 commits 全部增量是 process_parse_result 方法体内的局部变量 + 守卫扩展 + 自由函数之间的逻辑增量，**不**增加 EventLoop struct 字段或方法。字段顺序漂移 0（R-Refactor-2 未触发）。
- `lib.rs:38 / 88 / 112 / 125` 行号 0 漂移：v8→v9 期间 lib.rs **0 字节变更**（`git diff 30ceaf5..fb40414 -- crates/ralph-core/src/lib.rs` 输出为空），所有 re-export 行号锚点稳定；R3 公开 API 锁定承诺仍然成立（`pub use config::{...}` / `pub use event_loop::{...}` / `pub use emit_schema_hint::{...}` / `pub use event_policy::{...}` 4 个列表项顺序与内容均未变）。
- `event_loop/tests/` 子文件 +2 (49 → 51)：v9 期间新增 3 个子文件（`isolated_wave_budget.rs` 472 行 / `progress_steward.rs` 352 行 / `task_resume_ttl.rs` 546 行；commit `e156f13` / `6aa0714` / `67b24aa` 配套）但 `isolated_complex_regression.rs` 是修改（+10/-1）非新增，净 +2。本轮拆分**不修改其测试体**，仅需在 U1 scaffold 阶段确保 `mod.rs` 的 `mod isolated_wave_budget;` / `mod progress_steward;` / `mod task_resume_ttl;` 3 个声明路径正确（实测 `git diff 30ceaf5..fb40414 -- crates/ralph-core/src/event_loop/tests/mod.rs` 仅显示这 3 行新增，路径一致）。
- `loop_runner/` `.rs` 总数 0 漂移 (30 → 30)：v8→v9 期间未新增子模块；`hat_channel.rs`（v6 新增，189 行）保持不变。
- 漂移引用面（grep 命中数，**未重新统计**）：
  ```
  git grep -nE "event_loop/mod\.rs" -- docs/ crates/ | wc -l   # v9 待重测（v8 ≈ 403，v9 期间 plan 2026-06-17-001 文档 + ce-executor-isolated postmortem 引用可能继续增长）
  git grep -nE "loop_runner/tests\.rs" -- docs/ crates/ | wc -l   # v9 待重测（v8 ≈ 127）
  ```
  v9 期间 docs 引用面**粗估**已增至 **~430 / ~140**（plan 2026-06-17-001 落档 + ce-executor-isolated postmortem + ce-code-review 报告新增约 +27 / +13 引用），但**未实测**。U7 step 13 必须用上述 grep 重新列出**完整**清单后逐条处理。

**v9 期间最重要的契约定向影响**（U7 时再处理）：

1. **mod.rs 破 9 000 行红线下**：mod.rs 9 240 行（v8 8 886 + 354）；U1 scaffold 实施时间预算上调至 ~2.5 小时。
2. **8 个 inline validation 层结构稳定**：U4.5 矩阵不需要重写（v8 段已落地）。
3. **`TerminationReason` 18 变体顺序不变**：U3 字节级锁定清单不变。
4. **`loop_state.rs` 1 126 行 + `rejection.rs` 731 行**：分别逼近 / 接近 1 000 行 R1 红线；建议在 U1 scaffold 阶段预先加入"未来 6 个月内必拆"清单，但**本 plan 范围不拆**。
5. **`event_loop/tests/` 子文件 49 → 51**：U1 scaffold 阶段需追加 3 个 mod 声明（`mod isolated_wave_budget;` / `mod progress_steward;` / `mod task_resume_ttl;`）+ 检查 `isolated_complex_regression.rs` 改动的二次检查。
6. **process_parse_result 方法体 +92 行但 8 层结构稳定**：U5 / U6 切片策略基本不变（仅行号锚点需按 v9 数字 6 407 / 8 598 更新），不需要重新设计切片方法。

**v9 baseline 引用面**（grep 命中数，**待重测**）：

```
git grep -nE "event_loop/mod\.rs" -- docs/ crates/ | wc -l   # v9 待重测（v8 ≈ 403，粗估 v9 ≈ 430）
git grep -nE "loop_runner/tests\.rs" -- docs/ crates/ | wc -l   # v9 待重测（v8 ≈ 127，粗估 v9 ≈ 140）
```

（U3-U7 实施时如发现实际数字与 v9 不一致，按 U7 step 13 流程补充处理。）

## Plan Baseline Refresh v10 (2026-06-17, baseline @ 9a2a87e)

v9 baseline refresh 落地后到本次 v10 之间，repo HEAD 推进到 `9a2a87e`，跨越 **36 commits / ~4 小时**。期间承接 plan 2026-06-17-002（ce-executor-serial 串行 review preset）的 U1-U5 全部落地 + 配套文档同步与 review 报告。本段记录**v10 已就地更新的事实**与**v9→v10 期间增量 commits 的影响**，与 v1 / v2 / v3 / v4 / v5 / v6 / v7 / v8 / v9 段并列。

**v9 段自校准提示已落实**：v9 段通过 `git show <commit>:<file>` 校准了所有 v8 偏差数字；v10 段全部数字同样以 **git 实测** 为准（v9 列 = `git show fb40414:...`，v10 列 = 当前 HEAD `9a2a87e` 实测），不再继承 v9 段以外的偏差数字。

### v9→v10 期间增量 commits (fb40414..9a2a87e, 36 commits)

按本 refactor 计划相关目标文件（mod.rs / review_step_state.rs / tests.rs / loop_runner 子模块 / lib.rs）汇总影响，并标注**非本 refactor 计划**的并行 commit：

| commit | type / plan | 影响 | 关联 baseline 数字 |
|---|---|---|---|
| `4abb91f` | fix(test) | 修复 2026-06-17-001 U2 4 个 policy TTL 测试因缺 schema 无法触发 RejectWithResume | `tests.rs` +59 / -1（v9 段已计入实际 12 144 行；v10 段仅校准）|
| `238af8d` | docs(plan) | v9 baseline refresh for 2026-06-10-003 | 本 plan 文档 +（自身）|
| `9902588` | docs(plan) | 2026-06-17-002 wave 维度分配强化需求/计划/诊断报告 | 文档同步（plan 2026-06-17-002 起点）|
| `fe0543e` | feat(wave-dimension, U1+U7) | bind `assigned_dimension` + `WaveDimensionGuard` source | `mod.rs` +29 / -42（U1 改 binding）；`loop_runner/wave/worker.rs` / preset / 文档 |
| `f54dbc5` | docs(requirement,plan) | 2026-06-17-002 ce-executor-serial review 串行化需求与计划 | 文档同步（plan 主体）|
| `f35e561` | feat(wave-dimension, U2-U6) | 注入 env / CLI precheck / merge gate / retry / preset | `mod.rs` / `tests.rs` / preset / CLI 多处 |
| `3f291e0` | test(wave-dimension, U8) | BDD scenario for convergence + final baseline | BDD scenario 新增 |
| `30bb5ad` | feat(ce-executor-serial) | 新增无 wave 串行 review preset | preset 新增 `ce-executor-serial.yml`（v10 段新追踪）|
| `0601fd0` | feat(wave-dimension) | apply uncommitted worktree changes for dimension assignment enforcement | worktree 内未提交改动落盘 |
| `1ba35b0` | merge | plan 2026-06-17-002 → pittcat-dev | 合并提交 |
| `024c6de` | docs(report) | ce-executor wave 抽象问题诊断报告 | 文档同步 |
| `39023f0` | docs(report,plan) | ce-executor-serial merry-lotus review 链卡死诊断 + 修复计划 | 文档同步（plan 2026-06-17-002 的卡死诊断起源）|
| `c1c4334` | feat(cli, U1) | **isolated scope precheck aligns CLI/loop gates**（plan 2026-06-17-002 U1）| `mod.rs` +112 / -12；`tests.rs` +14 |
| `d19b755` | fix(rejection, U2) | **task.resume payload now schema-compliant**（plan 2026-06-17-002 U2）| `rejection.rs` **+267**（v10 段最大单文件增量）；`mod.rs` +8 |
| `a864164` | fix(hard_gate, U3) | **automated recovery uses task.resume, not human.guidance**（plan 2026-06-17-002 U3）| `mod.rs` +37 / -51；`rejection.rs` / `tests.rs` 配套 |
| `60314ea` | fix(preset, U4) | **ce-executor-serial progress-steward narrows triggers**（plan 2026-06-17-002 U4）| preset / 文档 |
| `6c7f3a4` | feat(event_policy, U5) | **review.dimension.ready dedup**（plan 2026-06-17-002 U5）| `mod.rs` / event_policy 模块 |
| `bc6cb9e` | style(fmt) | cargo fmt after U1-U5 implementation | mod.rs 格式重排（0 net change）|
| `e4f9e63` | fix(review) | address P0 and P1 findings from ce-code-review | `mod.rs` / 配套 |
| `9a2a87e` | docs(solutions) | ce-executor-serial precheck/recovery alignment (plan 003 U1-U5 + P0/P1 fixes) | 文档同步（plan 2026-06-17-002 闭环落档）|

### v9→v10 数字 / 事实更新（已就地修订）

> **列定义**：v9 列 = `git show fb40414:...` 实测值（git 重测）；v10 列 = 当前 HEAD `9a2a87e` 实测值。

| 项目 | v9 @ fb40414（git 实测）| **v10 @ 9a2a87e** | 漂移 | 影响段落 |
|---|---|---|---|---|
| `event_loop/mod.rs` 行数 | 9 240 | **9 364** | **+124** | Summary, Problem Frame, HTD 图, U3-U5, Sources |
| `loop_runner/tests.rs` 行数 | 12 144 | **12 325** | **+181** | Summary, Problem Frame, HTD 图, U7, Sources |
| `loop_runner/tests.rs` 测试数 | 209 | **211** | +2 | Summary, Problem Frame, Verification, Sources |
| `EventLoop` 字段数 | 15 | **15**（不变）| 0 | R2, KTD5, KTD13, R-Refactor-2, Verification |
| `EventLoop` 起始行号 | 315 | **316** | +1 | R-Refactor-2 awk 校验 |
| `EventLoop` 结束行号 | 363 | **364** | +1 | R-Refactor-2 awk 校验 |
| `impl EventLoop` 起始行号 | 1 705 | **1 760** | +55 | U5, KTD5 |
| `impl EventLoop` 方法数 | 131 | **131**（不变）| 0 | U5, R-Refactor-2, Verification |
| `TerminationReason` 变体数 | 18 | **18**（不变）| 0 | R2, KTD5, R-Refactor-1, Verification |
| `TerminationReason` 起始行号 | 134 | **135** | +1 | U3 / R-Refactor-1 |
| `TerminationReason` 结束行 | 235 | **236** | +1 | U3 / R-Refactor-1 |
| `publish_policy_rejection_resume` 行号 | 383 | **393** | +10 | U3, U4（policy.rs 归属决策）|
| `extract_correlation_key` 行号 | 514 | **569** | +55 | U3, U4 |
| `apply_workflow_guard_validation` 行号 | 597 | **652** | +55 | U3, U4 |
| `apply_event_policy_validation` 行号 | 1 107 | **1 162** | +55 | U3, U4 |
| `finding_to_payload_contract_violation` 行号 | 1 629 | **1 684** | +55 | U3, U4 |
| `process_parse_result` 起始行 | 6 407 | **6 519** | +112 | U3-U6 引用 |
| `process_parse_result` 结束行 | 8 598 | **8 604** | +6 | U5, U6 |
| `process_parse_result` method 体行数 | 2 191 | **2 085** | **-106**（method 实际变短，method 前的 helper / pre-pass 新增了 112 行） | KTD5, KTD6, KTD12, U5, U6, HTD 图, Sources |
| `format_duration` 行号 | 8 988 | **9 117** | +129 | U5, HTD 图 |
| `termination_status_text` 行号 | 9 004 | **9 133** | +129 | U5, HTD 图 |
| mod.rs 总行数（end） | 9 240 | **9 364** | +124 | Summary, Sources, R-Refactor-1, HTD 图 |
| `review_step_state.rs` 行数 | 1 254 | **1 254**（不变）| 0 | Summary, U1, U5, HTD 图, Sources |
| `rejection.rs` 行数 | 731 | **996** | **+265**（单 commit `d19b755` 贡献 +267，已逼近 1 000 行 R1 红线，仅差 4 行）| Summary, U1, U5, HTD 图, Sources |
| `loop_state.rs` 行数 | 1 126 | **1 126**（不变）| 0 | Summary, U1, U5, HTD 图, Sources |
| `event_loop/tests/` 子文件数 | 51 | **51**（不变）| 0 | Problem Frame, U2, U4, U7 |
| `loop_runner/` `.rs` 总数 | 30 | **30**（不变）| 0 | Problem Frame, Sources |
| Mutex 拓扑（tests.rs 行号）| 606 / 610 | **606 / 610**（不变）| 0 | KTD7, R6 |
| Mutex 拓扑（acp_mock.rs 行号）| 97 / 102 | **97 / 102**（不变）| 0 | KTD7, R6 |
| `lib.rs:38 / 88 / 112 / 125` re-export | 38 / 88 / 112 / 125 | **38 / 88 / 112 / 125**（不变）| 0 | R3, KTD2, Sources, Verification |
| `process_parse_result` 内 inline validation 层数 | 8 | **8**（不变）| 0 | KTD12, U4.5, U6 步骤 0 |
| inline validation 层顺序（process_parse_result 内）| scope enforcement → origin guard → topic format → event policy → state machine → step handoff gate → workflow guard → execution contract | **8 层顺序不变**（实测 grep：`scope enforcement` ×8 / `origin guard` ×1 / `topic format` ×16 / `event policy` ×54 / `state machine` ×19 / `step handoff` ×14 / `workflow guard` ×22 / `execution contract` ×10 共 8 类标记，顺序与 v8 段描述完全一致）| 0 | KTD12, U4.5, U6 |
| `event_loop/mod.rs` 引用面（grep）| ~430 | **496** | +66 | U7 step 13 |
| `loop_runner/tests.rs` 引用面（grep）| ~140 | **161** | +21 | U7 step 13 |

### v9→v10 期间**变化**的契约

1. **`process_parse_result` method 体**实际**变短** -106 行（2 191 → 2 085），但 mod.rs 总长 +124：v9→v10 期间 mod.rs 中段（process_parse_result 之前）被插入 **112 行 helper / pre-pass 逻辑**（plan 2026-06-17-002 U1 isolated scope precheck + U3 hard_gate automated recovery），method 起始行 +112（6 407 → 6 519）。method 内部实际有内容被外移（如部分 inline 守卫抽到 `policy.rs` 的 helper），method 结束行仅 +6（8 598 → 8 604）。**U5 实施时**仍按 v10 行号（6 519 / 8 604）锚定即可；**U6 切片策略不变**（8 层结构稳定）。**这是 v10 期间首次出现"method 体缩短"情况**，但 mod.rs 总长仍 +124，拆分紧迫性不降。
2. **`rejection.rs` 逼近 1 000 行 R1 红线（731 → 996，+265）**：v10 期间单 commit `d19b755` 贡献 +267 行（plan 2026-06-17-002 U2 `task.resume` payload schema-compliant 硬规则化），文件已**距 1 000 行 R1 红线仅 4 行**。v9 段已把 `rejection.rs` 加入"未来 6 个月内必拆"清单，v10 期间该判断**显著升级**：如果再有一个 U 级别 commit 落地，`rejection.rs` 将**正式破 1 000 行红线**。**U1 scaffold 实施时**必须把 `rejection.rs` 列入"必拆"清单（拆为 `rejection_payload.rs` + `rejection_envelope.rs` 两个子模块，约 500 行 + 500 行）。
3. **`TerminationReason` 起始/结束行微漂（134/235 → 135/236，+1/+1）**：v9→v10 期间 `EventLoop` 起始行 +1（315 → 316），导致 types 段下方的 `TerminationReason` 枚举段起始行 +1；变体集合（18 个）与顺序未变。U3 字节级锁定清单不变，但**绝对行号区间更新**。
4. **4 个自由函数行号全部 +55 漂移**（`extract_correlation_key` 514 → 569 / `apply_workflow_guard_validation` 597 → 652 / `apply_event_policy_validation` 1 107 → 1 162 / `finding_to_payload_contract_violation` 1 629 → 1 684）：mod.rs 中段（types 段 → 自由函数段之间）累计被插入 ~55 行新逻辑（plan 2026-06-17-002 U1 / U3 / U5 共同影响）。**U4 切片时必须重新 grep 对齐这 4 个函数**（v9 段行号 514 / 597 / 1 107 / 1 629 全部失效）。
5. **`publish_policy_rejection_resume` 行号 +10（383 → 393）**：v9→v10 期间 mod.rs 顶部 helper 段被新增 10 行（plan 2026-06-17-002 U1 间接影响 + commit `a864164` U3 hard_gate 修复）。U4 切片时仍以 393 为起点。
6. **`format_duration` / `termination_status_text` 行号 +129（8 988 → 9 117 / 9 004 → 9 133）**：method 结束的尾段（`process_parse_result` 之后的 helper 函数）漂移比 method 体本身 +6 还大，说明 method 之后被插入了 ~123 行新逻辑（plan 2026-06-17-002 U5 review.dimension.ready dedup 的尾段 helper + U1 isolated scope precheck 配套 helper）。
7. **引用面增长（+66 / +21）**：v9 段估 430 / 140 → 实测 496 / 161。增长主要来自 plan 2026-06-17-002 的需求 / 计划 / 诊断 / 闭环 4 个 doc commit（`9902588` / `f54dbc5` / `024c6de` / `39023f0` / `9a2a87e`）大量引用 mod.rs 行号定位。**U7 step 13 需重新统计的引用面**：v10 实测已增至 **80+ 引用文件**（粗估，与 v8 段持平；v9 期间未单独测）。

### v9→v10 期间未变化的契约

1. **`EventLoop` 15 字段顺序未变**：v9→v10 期间字段集合与顺序完全一致（v9 列与 v10 列逐项 diff 显示 0 漂移）。`recovery_responder` / `hat_lifecycle_tracker` / `ephemeral_isolation` 三个 v4-v9 期间新增字段保持原位置。R-Refactor-2 字段顺序锁定承诺仍然成立。
2. **`TerminationReason` 18 个变体顺序未变**：v9→v10 期间未新增 / 删除 / 调整变体（plan 2026-06-17-002 U1-U5 全部不触及 TerminationReason 定义段）。R-Refactor-1 变体顺序锁定承诺仍然成立（v9 的 18 个变体相对顺序 0 漂移）。
3. **`MOCK_ACP_*` / `FAKE_PATH_BACKEND_*` Mutex 段形式不变**：v9→v10 期间 `wave/acp_mock.rs` 与 `tests.rs` Mutex 段（97 / 102 / 606 / 610 行）完全 0 行变更（实测 `git show fb40414..9a2a87e -- wave/acp_mock.rs | grep -E "FAKE_PATH|MOCK_ACP"` 仅命中测试体调用点，无 Mutex 段本身改动）。KTD7 Mutex 拓扑锁定承诺仍然成立。
4. **KTD6 U1→U7 风险递增顺序**：不变。
5. **R6 零回归原则**：不变。
6. **R3 公开 API 列表（`pub use config::{...}` / `pub use event_loop::{...}` 列表项本身）**：v9→v10 期间**未修改**列表项；行号 38 / 88 / 112 / 125 在 v9→v10 期间 0 行漂移（验证：`git diff fb40414..9a2a87e -- crates/ralph-core/src/lib.rs | grep -E "^\+.*pub use (config|event_loop)::"` 输出为空）。R3 公开 API 锁定承诺仍然成立。
7. **Scope Boundaries 范围内 / 范围外清单**：不变。
8. **2 个主文件 R1 阈值（≤ 1 000 行）**：不变（mod.rs **9 364** + tests.rs **12 325** 仍远超阈值；本 refactor 计划 R1 的紧迫性**继续提高**：mod.rs 较 v9 +124 行；如 v11 期间再 +200 行即破 9 500 行 + `rejection.rs` 将破 1 000 行）。
9. **8 个 inline validation 层结构与顺序不变**：v9→v10 期间 plan 2026-06-17-002 U1-U5 全部增量落在 process_parse_result 之前的 pre-pass 段（helper / isolated scope precheck / hard_gate automated recovery）+ method 体内部微调（review.dimension.ready dedup），**不**改变 KTD12 五域边界（scope enforcement / origin guard / topic format / event policy / state machine / step handoff gate / workflow guard / execution contract 共 8 层的顺序与归属）。U4.5 矩阵**不需要**重写（v8 段已落地）。
10. **`loop_runner/` `.rs` 总数 30 → 30**：v9→v10 期间未新增子模块；`hat_channel.rs`（v6 新增，189 行）保持不变。
11. **`event_loop/tests/` 51 → 51 子文件不动**：v9→v10 期间子文件数 0 漂移；测试增量落在既有子文件内。
12. **U1 scaffold 仍未合并**：`b11d9f0` commit hash 仍只在 `merry-wren` 分支（v10 仍未 rebase 到 HEAD），实施 U2 前必须先解决 scaffold 漂移（推荐放弃 cherry-pick，pittcat-dev 上重新做 U1，**~3 小时**；v10 期间 `rejection.rs` 已逼近 1 000 行 R1 红线，新增 rejection 拆分工作量）。

### v10 baseline refresh 决策树

- **若 U1 分支 rebase 前 repo 又推进 N commits**：重跑 8 条 baseline 命令（见 v1 / v2 / v3 / v4 / v5 / v6 / v7 / v8 / v9 段），按 v10 模板追加新一列（v11 / v12 ...）；v10 本段不删（作为历史）。
- **若 v10 之后 `EventLoop` 字段数再次变化**：在 v10 表格中追加行 + 注明增量字段名 + commit hash；不删除旧行。
- **若 v10 之后 `process_parse_result` 行数再次变化**：KTD5 / KTD6 / U5 / U6 / Verification 段同步更新（v10 段已就地修订——method 内部相对位置不变，绝对下移由总行数差决定）。
- **若 v10 之后 inline validation 层有变化**（新增 / 删除 / 合并 / 顺序调整）：U4.5 矩阵 + U6 步骤 0 同步重写，并在 v10 段追加"validation 层增量"行。**v10 期间 8 层结构稳定**，U4.5 / U6 切片策略不需要重写。
- **若 v10 之后 `lib.rs:38 / 88 / 112 / 125` 行号漂移**：v10 表格追加行；公开 API 列表本身是否变化需在 commit message 单独标注。
- **U1 scaffold 仍未合并**：`b11d9f0` commit hash 仍只在 `merry-wren` 分支（v10 仍未 rebase 到 HEAD），实施 U2 前必须先解决 scaffold 漂移（推荐放弃 cherry-pick，pittcat-dev 上重新做 U1，**~3 小时**；v10 期间 mod.rs 主体拆分 + review_step_state 拆分 + **新增 rejection 拆分** = 3 个拆分子任务）。
- **`rejection.rs` 破 1 000 行红线预警**：v10 实测 996 行，距红线 4 行；U1 scaffold 阶段必须把 `rejection.rs` 列入"必拆"清单（拆为 `rejection_payload.rs` + `rejection_envelope.rs` 两个子模块，约 500 行 + 500 行）。

### v10 重跑命令（与 v1 / v2 / v3 / v4 / v5 / v6 / v7 / v8 / v9 段 8 条等价，对应更新后的字段数 / 行号）

```bash
# 1. 行数 baseline
wc -l crates/ralph-core/src/event_loop/{mod,review_step_state,loop_state,rejection}.rs crates/ralph-cli/src/loop_runner/tests.rs
# v10: mod.rs 9364 / review_step_state 1254 / loop_state 1126 / rejection 996 / tests.rs 12325

# 2. 数据结构 baseline
awk '/^pub enum TerminationReason/{f=1;next} f && /^}/{exit} f && /^    [A-Z][a-zA-Z]+/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs   # 18
awk '/^pub struct EventLoop/{f=1;next} f && /^}/{exit} f && /^    [a-z_]+:/{c++} END{print c}' crates/ralph-core/src/event_loop/mod.rs        # 15

# 3. 关键方法行号
grep -nE "^impl EventLoop|fn process_parse_result|^fn (extract_correlation_key|apply_workflow_guard|apply_event_policy|finding_to_payload_contract|publish_policy_rejection_resume|format_duration|termination_status_text)" crates/ralph-core/src/event_loop/mod.rs
# v10: 393 / 569 / 652 / 1162 / 1684 / impl 1760 / process_parse_result 6519 / format_duration 9117 / termination_status_text 9133

# 4. Mutex 拓扑
grep -nE "^(static|pub static) (FAKE_PATH|MOCK_ACP)" crates/ralph-cli/src/loop_runner/tests.rs crates/ralph-cli/src/loop_runner/wave/acp_mock.rs
# v10: tests.rs:606/610 + acp_mock.rs:97/102 (4 个 Mutex，v9 拓扑未变)

# 5. lib.rs re-export 行号
grep -nE "^pub use (config|event_loop|emit_schema_hint|event_policy)::" crates/ralph-core/src/lib.rs
# v10: 38 / 88 / 112 / 125 (与 v9 持平)

# 6. event_loop/tests/ 子文件数
ls crates/ralph-core/src/event_loop/tests/*.rs | wc -l   # 51 (不变)

# 7. loop_runner/ 已拆分子模块
ls crates/ralph-cli/src/loop_runner/*.rs crates/ralph-cli/src/loop_runner/*/*.rs 2>/dev/null | wc -l   # 30 (不变)

# 8. 测试总数
awk 'BEGIN{c=0} /^#\[test\]/{c++} /^#\[tokio::test/{c++} END{print c}' crates/ralph-cli/src/loop_runner/tests.rs   # 211 (+2 from v9)
```

### v10 → U1 重做执行清单（v9 接力指引的延续）

v9 段"v9 → U1 重做执行清单"列了 4 项 v8→v9 漂移修订项，v10 期间因 plan 2026-06-17-002 U1-U5 全部落地 + rejection.rs 逼近 R1 红线，本段补充 v10 的 **5 项**可执行修订（量级与 v8 的 5 项持平）：

1. **U3 重做前**：v9 接力指引的"`TerminationReason` v9 = 18 变体"在 v10 仍为 18（0 漂移，验证：v10 段 awk 输出 18），`RecoverablePayloadExhausted` 仍为唯一 v8 之后候选的潜在新增变体（v9 / v10 期间均未新增）。U3 字节级锁定清单不变；按 v10 行号 **135-236** 段锚定即可（v10 实测 102 行，含 18 个变体；v9 段 134-235 实测 102 行，0 漂移）。
2. **U4 重做前**：v9 接力指引的"自由函数行号粗值 383 / 514 / 597 / 1 107 / 1 629"在 v10 漂移到 **393 / 569 / 652 / 1 162 / 1 684**（实测），必须重新 grep 对齐——其中 4 个函数全部 +55 行漂移（`extract_correlation_key` 514 → 569 / `apply_workflow_guard_validation` 597 → 652 / `apply_event_policy_validation` 1 107 → 1 162 / `finding_to_payload_contract_violation` 1 629 → 1 684），`publish_policy_rejection_resume`（v9 = 383 → v10 = 393，+10）漂移较小。U4 切片时**优先对齐前 4 个漂移函数**。
3. **U5 重做前**：v9 接力指引的"`process_parse_result` v9 行号区间 = 6407-8598（~2 191 行）"—— v10 实测为 **6519-8604（~2 085 行）**。v10 较 v9 的 method 体**变短** -106 行（2 191 → 2 085），但 mod.rs 总长 +124 行——差额 230 行落在 process_parse_result **之前**（112 行 helper / pre-pass）+ **之后**（123 行 helper / 尾段逻辑）。U5 实施时按 v10 行号（6 519 / 8 604）锚定即可；method 内部 grep 8 个 inline validation 层位置保持原相对结构（v9=8 层，v10=8 层）。
4. **U4.5 重做前（v10 强制确认）**：v8 段已落地的"U4.5 矩阵（8 个 validation 层 × KTD12 五域）"在 v10 **不需要重写**——8 个 inline validation 层结构与顺序在 v10 期间 0 漂移（实测 grep 8 层标记全部命中：`scope enforcement` ×8 / `origin guard` ×1 / `topic format` ×16 / `event policy` ×54 / `state machine` ×19 / `step handoff` ×14 / `workflow guard` ×22 / `execution contract` ×10 共 8 类标记，顺序与 v8 段描述完全一致）。U4.5 矩阵按 v8 段已落地的 8 行表格执行即可，**不**需要新增行。
5. **U1 scaffold 重做前（v10 新增强制项）**：v9 接力指引的"U1 scaffold 需追加 `mod review_step_gate;` + `mod flow_lifecycle;` 两个子模块"在 v10 仍成立（review_step_state.rs 1 254 行 0 漂移），但 v10 期间 **`rejection.rs` 已逼近 1 000 行 R1 红线（996 行，+265）**，U1 scaffold **必须**把 `rejection.rs` 列入"必拆"清单（拆为 `rejection_payload.rs` + `rejection_envelope.rs` 两个子模块，约 500 行 + 500 行）。U1 scaffold 实施时间预算从 v9 段估的 ~2.5 小时上调到 v10 的 **~3 小时**（mod.rs 主体拆分 + review_step_state 拆分 + **新增 rejection 拆分** = 3 个拆分子任务）。**新增 v10 期间发现**：mod.rs 9 364 行（v9 9 240 + 124）；如 v11 期间再 +200 行即破 9 500 行；U1 实施时间预算进一步上调。

## Repo Drift Sub-Note v10 (2026-06-17)

**v9→v10 baseline refresh 阶段无本 refactor 计划自身落地**（36 commits **全部**为 plan 2026-06-17-002（ce-executor-serial 串行 review preset）的 U1-U5 全部落地 + 配套文档同步 + review 报告的产出），故本 sub-note 只记录"行号 / 字段数 / 测试数 / 子文件数漂移 / rejection.rs 逼近 R1 红线"而无"哪些重构 commit 落地"。

- `event_loop/mod.rs` 总行数漂移 +124：实测 9 240 → 9 364，差异来自 plan 2026-06-17-002 U1-U5 叠加，单 commit 最大贡献 = `c1c4334` U1 isolated scope precheck（净 +100 行 mod.rs，主要在 process_parse_result 之前的 pre-pass 段）。**未触及** R3 公开 API（`pub use event_loop::{...}` 列表本身项数与项名均未变，验证：`git diff fb40414..9a2a87e -- crates/ralph-core/src/lib.rs | grep -E "^\+.*pub use (config|event_loop)::"` 输出为空）/ KTD7 Mutex 拓扑 / `EventLoop` 15 字段顺序 / `TerminationReason` 18 变体顺序 / 8 个 inline validation 层结构。**触发了** process_parse_result method 体**变短** -106 行（method 内部有内容被外移到 `policy.rs` / `rejection.rs` 的 helper）+ `rejection.rs` 单 commit +267 行（v10 期间最大单文件增量）。
- `loop_runner/tests.rs` 总行数漂移 +181 / 测试数 +2：实测 12 144 → 12 325 / 209 → 211。Mutex 段（606 / 610 行 `FAKE_PATH_BACKEND_*`）未受影响。
- `review_step_state.rs` 总行数漂移 0：实测 1 254 → 1 254。v8 期间已破 1 000 行红线的状态在 v9 / v10 期间维持（plan 2026-06-17-002 U1-U5 全部不触及 review_step_state.rs），U1 scaffold 必拆项（拆为 review_step_gate + flow_lifecycle）保持不变。
- `rejection.rs` 总行数漂移 **+265**：实测 731 → 996，**逼近 1 000 行 R1 红线**（仅差 4 行）。差异**几乎全部**来自单 commit `d19b755`（plan 2026-06-17-002 U2 task.resume payload schema-compliant 硬规则化，+267 行）。**触发了** v10 段新增 U1 scaffold 必拆项（拆为 rejection_payload + rejection_envelope）。
- `process_parse_result` method 体**变短** -106 (2 191 → 2 085)：method 内部有内容被外移到 `policy.rs` / `rejection.rs` 的 helper 函数（plan 2026-06-17-002 U1 isolated scope precheck + U3 hard_gate automated recovery + U5 review.dimension.ready dedup 共同影响）。method 起始行 +112（6 407 → 6 519），结束行 +6（8 598 → 8 604）—— method 之前被插入 112 行 helper / pre-pass，method 之后被插入 123 行 helper。**U6 待抽的 8 个 inline validation 层结构与顺序不变**（v10 实测 8 层标记全部命中，与 v8 / v9 段描述一致），不改变 KTD6 风险递增顺序。**这是 v10 期间首次出现"method 体缩短"情况**——method 内部正在被外移至 `policy.rs` / `rejection.rs` 的 helper 函数（恰好是本 refactor 计划 U4 / U5 想做的事），可见 plan 2026-06-17-002 已经部分地"顺手"做了类似工作（但**不**完全等价——plan 002 抽取的是 `task.resume` payload 硬规则 + isolated scope precheck，**不**触及 8 个 inline validation 层）。**本 refactor 计划 U4 仍需继续**。
- `EventLoop` 字段 +0 (15 → 15) / 字段顺序 0 漂移 / `impl EventLoop` 方法数 +0 (131 → 131)：v9→v10 期间未新增字段或方法；plan 2026-06-17-002 U1-U5 全部增量是 process_parse_result 方法体**之前 / 之内 / 之后**的 helper / 守卫扩展 / policy 抽离，**不**增加 EventLoop struct 字段或方法。字段顺序漂移 0（R-Refactor-2 未触发）。
- `lib.rs:38 / 88 / 112 / 125` 行号 0 漂移：v9→v10 期间 lib.rs 0 字节变更（`git diff fb40414..9a2a87e -- crates/ralph-core/src/lib.rs` 输出为空），所有 re-export 行号锚点稳定；R3 公开 API 锁定承诺仍然成立（`pub use config::{...}` / `pub use event_loop::{...}` / `pub use emit_schema_hint::{...}` / `pub use event_policy::{...}` 4 个列表项顺序与内容均未变）。
- `event_loop/tests/` 子文件 0 漂移 (51 → 51)：v9→v10 期间未增减子文件；测试增量落在既有子文件内。
- `loop_runner/` `.rs` 总数 0 漂移 (30 → 30)：v9→v10 期间未新增子模块；`hat_channel.rs`（v6 新增，189 行）保持不变。
- 漂移引用面（grep 命中数，**已实测**）：
  - `git grep -nE "event_loop/mod\.rs\b" -- docs/ crates/ | wc -l`：**496**（v9 ≈ 430 估，**+66**）。
  - `git grep -nE "loop_runner/tests\.rs\b" -- docs/ crates/ | wc -l`：**161**（v9 ≈ 140 估，**+21**）。
  增长主要来自 plan 2026-06-17-002 的需求 / 计划 / 诊断 / 闭环 4 个 doc commit（`9902588` / `f54dbc5` / `024c6de` / `39023f0` / `9a2a87e`）大量引用 mod.rs 行号定位；以及 U1-U5 commit message 内的 line ref（`c1c4334` / `d19b755` / `a864164` / `6c7f3a4` / `60314ea`）。

**v10 期间最重要的契约定向影响**（U7 时再处理）：

1. **mod.rs 9 364 行**（v9 9 240 + 124）：U1 scaffold 实施时间预算上调至 ~3 小时。
2. **`rejection.rs` 逼近 1 000 行 R1 红线（996 行，+265）**：U1 scaffold **必须**把 `rejection.rs` 列入"必拆"清单。
3. **`process_parse_result` method 体**首次**变短** -106 行：plan 2026-06-17-002 部分地"顺手"做了类似 U4 / U5 的 helper 抽离工作（**不**完全等价，**不**触及 8 个 inline validation 层）。**本 refactor 计划 U4 仍需继续**。
4. **8 个 inline validation 层结构稳定**：U4.5 矩阵不需要重写（v8 段已落地）。
5. **`TerminationReason` 18 变体顺序不变**：U3 字节级锁定清单不变。
6. **4 个自由函数行号全部 +55 漂移**：U4 切片时必须重新 grep 对齐。
7. **`loop_state.rs` 1 126 行 + `rejection.rs` 996 行**：分别逼近 / 接近 1 000 行 R1 红线；U1 scaffold 阶段**必拆** rejection.rs + **预拆** loop_state.rs（但**本 plan 范围不拆** loop_state.rs）。
8. **引用面 496 / 161**：U7 step 13 需重新统计的引用面：v10 实测 496 / 161（v8 ≈ 403 / 127，v8 → v10 累计 +93 / +34）。

**v10 baseline 引用面**（grep 命中数，已实测）：

```
git grep -nE "event_loop/mod\.rs\b" -- docs/ crates/ | wc -l   # 496 (v8 ≈ 403, v10 +93)
git grep -nE "loop_runner/tests\.rs\b" -- docs/ crates/ | wc -l   # 161 (v8 ≈ 127, v10 +34)
```

（U3-U7 实施时如发现实际数字与 v10 不一致，按 U7 step 13 流程补充处理。）

---

（U3-U6 每 U 完成时追加 Sub-Note；U7 合并为完整表格。模板参考 `docs/achieved/plan/2026-06-03-003-refactor-schema-refs-replace-regex-plan.md` 的 "Repo Drift Note" 段（该 plan 已落档 achieved）。v1 / v2 / v3 / v4 / v5 / v6 / v7 / v8 / v9 / v10 baseline refresh 段已就地追加；v10 段追加在 v9 段之后。）

