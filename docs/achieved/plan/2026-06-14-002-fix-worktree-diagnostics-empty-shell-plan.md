---
title: Fix worktree diagnostics empty shell leak in subprocess TUI mode
type: fix
status: active
date: 2026-06-14
---

# Fix worktree diagnostics empty shell leak in subprocess TUI mode

## Overview

修复 `ralph run --worktree` 在默认 subprocess TUI 模式下，主仓库 `.ralph/diagnostics/` 出现空 `recovery.jsonl` / `drift.jsonl` 等「空壳产物」的问题。核心思路是：让 TUI 父进程只保留 trace/logging 所需的轻量 session，真正跑 `EventLoop` 的 RPC 子进程才创建包含 recovery/drift 的完整 diagnostics session。同时补齐 loop 结束后 `ralph diagnose` 定位 worktree diagnostics 的能力，避免用户拿到空报告。

---

## Problem Frame

当前 Ralph CLI 在 TTY 下默认走 subprocess TUI 模式：

1. **父进程**跑在主仓库 cwd，负责启动 TUI 并把子进程 stderr 重定向到日志文件。
2. **子进程**通过 `--rpc` 启动，并被 `run_subprocess_tui` 显式 `current_dir` 到 worktree，真正执行 `EventLoop`。

问题出在 `crates/ralph-cli/src/main.rs`：父进程在还不知道自己是「TUI wrapper」还是「真正跑 loop」之前，就先创建了一个完整的 `DiagnosticsCollector`（包含 `RecoveryLogger` / `DriftLogger` 等）。由于创建时 cwd 还是主仓库，session dir 落在主仓库 `.ralph/diagnostics/<timestamp>/`。父进程随后只跑 TUI、不跑 `EventLoop`，导致这些文件永远是空的（`recovery.jsonl` / `drift.jsonl` 0 字节）。

实际跑 loop 的子进程在 chdir 到 worktree 后重新进入 `main.rs`，创建自己的完整 session，因此 worktree 里有一份真实数据。最终同一 run 出现两套同名 timestamp session：主仓库是空壳，worktree 是真实数据。

这带来三个后果：

1. **违反隔离契约**：`RunArgs` 文档明确说明 `--worktree` 时 loop 的 `.ralph/`（含 diagnostics）应建在 worktree 里。
2. **干扰排查**：用户容易把主仓库的空 recovery/drift 当成真实数据。
3. **`ralph diagnose` 可能拿到空报告**：loop 结束后 PID 死掉，`loops.json` 按 `is_pid_alive()` 清理，`ralph diagnose` 会 fallback 到主仓库 `.ralph/diagnostics/`，看到空文件。

---

## Requirements Trace

- **R1. 隔离性**：subprocess TUI 父进程不应在主仓库产生 recovery/drift/agent-output/orchestration 等 loop 级 diagnostics 文件。
- **R2. 无回归**：非 subprocess TUI 模式（`--no-tui`、`--legacy-tui`、`--rpc` 子进程本身、autonomous 模式）的 diagnostics 行为保持完全不变。
- **R3. 完整 session 仍在正确位置**：worktree 子进程仍需在 worktree 内创建完整 diagnostics session（`recovery.jsonl`、`drift.jsonl`、`diagnosis-summary.json` 等）。
- **R4. TUI 父进程 trace 能力保留**：父进程仍应能写 `trace.jsonl` 与 TUI stderr log，用于排查 TUI 本身的问题。
- **R5. diagnose 可定位已结束 worktree loop**：loop 结束后，`ralph diagnose` 仍能解析到 worktree 的真实 diagnostics session，而不是主仓库空壳。

---

## Scope Boundaries

- **本计划内**：subprocess TUI 父进程的 diagnostics 创建策略；`DiagnosticsCollector` 的 trace-only 模式；`ralph diagnose` 对 worktree session 的 fallback 解析。
- **本计划外（Deferred to Follow-Up Work）**：
  - 修改 `LoopRegistry` 长期保留已结束 worktree entry 的语义（本计划选择更轻量的 pointer 文件方案）。
  - 重构整个 diagnostics 初始化时序（例如让父进程完全不创建 session）。当前方案只引入 `trace_only` 模式，改动最小。

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/diagnostics/mod.rs`
  - `DiagnosticsOptions`：控制 `full_diagnostics`、`runtime_diagnosis_artifacts`、`session_dir`。
  - `DiagnosticsCollector::with_options`：根据 `DiagnosticsOptions` 创建 session dir 并实例化各类 logger。
  - 现有 activation matrix 已区分 full / minimal / disabled 模式，新增 trace-only 模式是自然的第四态。
- `crates/ralph-cli/src/main.rs`
  - 计算 `tui_enabled`、`rpc_enabled`、`diagnostics_enabled`。
  - 创建 `authoritative_diagnostics` 并传给 `commands::run::run_command` / `commands::resume::resume_command`。
  - TUI 分支用 `ralph_core::diagnostics::create_log_file` + `DiagnosticTraceLayer` 写日志。
- `crates/ralph-cli/src/commands/run.rs`
  - `run_subprocess_tui`：父进程 spawn `--rpc` 子进程，设置 `.current_dir(&args.workspace)`。
  - 子进程 `--worktree_path` 路径直接复用父进程创建的 worktree，不会二次创建。
- `crates/ralph-cli/src/commands/diagnose.rs`
  - `resolve_diagnostics_root_via_loops`：读 `loops.json`，按 `is_pid_alive()` 取最新 alive loop 的 `workspace`。
  - loop 结束后 alive entry 被清理，导致 fallback 到主仓库 diagnostics。
- `crates/ralph-core/src/loop_context.rs`
  - `LoopContext::worktree` 持有 `workspace`（worktree 路径）和 `repo_root`（主仓库路径）。
  - 子进程可通过 `repo_root()` 写 pointer 文件到主仓库 `.ralph/`。

### Institutional Learnings

- `docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md`：CLI 测试必须用 `cargo nextest run`，禁止裸跑 `cargo test -p ralph-cli`。本计划涉及 `crates/ralph-cli/src/main.rs` 修改，验证时必须遵守。
- `AGENTS.md` / `CLAUDE.md`：所有中文输出规则、测试入口规则、CLAUDE.md 与 AGENTS.md 同步规则。

### External References

- 无需外部研究。本修复完全基于现有 Rust 代码与内部 diagnostics 模式。

---

## Key Technical Decisions

1. **新增 `trace_only` 模式而非取消父进程 session**
   - 父进程仍需 session dir 来承载 `DiagnosticTraceLayer` 与 TUI stderr log。
   - `trace_only` 让父进程保留 session dir，但跳过所有 loop 级 logger（recovery/drift/orchestration/performance/errors/hook-runs/agent-output/prompt-log）。

2. **父进程在 `main.rs` 层决定模式，不传到 `run_command` 再判断**
   - `main.rs` 已经知道命令类型和 TUI/RPC/autonomous 标志，在这里计算 `use_subprocess_tui` 最自然，也避免 `run_command` 接口过度膨胀。

3. **使用 pointer 文件解决 loop 结束后的 diagnose 定位**
   - 不修改 `LoopRegistry` 清理语义（避免 loops.json 膨胀和语义变化）。
   - 子进程在创建完整 diagnostics session 后，向主仓库 `.ralph/diagnostics-session-pointer.json` 写入该 session 的绝对路径。
   - `ralph diagnose` 无 alive loop 时，优先使用 pointer 文件指向的 worktree diagnostics root。

4. **pointer 文件采用「最后写入者胜」语义**
   - 多个 worktree loop 并发运行时，pointer 指向最后一个创建 session 的 worktree。
   - 这与 `ralph diagnose --session latest` 的「latest」语义一致。

---

## Open Questions

### Resolved During Planning

- **Q1**: 是否让父进程完全不创建 diagnostics session？
  - **Resolution**: 否。父进程需要 session dir 承载 `trace.jsonl` 与 TUI stderr log。改为 `trace_only` 模式即可消除空壳。
- **Q2**: 如何区分 default subprocess TUI 与 `--legacy-tui`？
  - **Resolution**: 在 `main.rs` 判断 `args.legacy_tui`，仅当 `tui_enabled && !legacy_tui && is_tty` 时才使用 `trace_only`。`--legacy-tui` 在父进程内跑 `EventLoop`，需要完整 session。
- **Q3**: `ralph diagnose` 是否必须改？
  - **Resolution**: 必须。即使修复空壳后，loop 结束后 diagnose 仍可能 fallback 到主仓库（此时为空）。通过 pointer 文件确保用户能拿到 worktree 真实报告。

### Deferred to Implementation

- **D1**: `DiagnosticsCollector` 的 `Debug` 输出中如何表示 `trace_only`？
  - 实现时决定，计划不约束具体字段名与 `Debug` 格式。
- **D2**: pointer 文件是否需要在 loop 正常结束时清理？
  - 实现时评估：保留 pointer 对离线 diagnose 有价值；若用户手动删除 worktree，diagnose 会 graceful fallback，无需清理。

---

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

### 模式选择矩阵

| 运行模式 | 父进程 cwd | 父进程 diagnostics | 子进程 cwd | 子进程 diagnostics |
|---|---|---|---|---|
| `ralph run` (TTY, default TUI) | 主仓库 | `trace_only` session | worktree | 完整 session |
| `ralph run --no-tui` | 主仓库 | 完整 session（主仓库） | 无 | 无 |
| `ralph run --legacy-tui` | 主仓库 | 完整 session（主仓库） | 无 | 无 |
| `ralph run --rpc`（子进程自身） | worktree | 完整 session（worktree） | 无 | 无 |
| `ralph run --worktree --no-tui` | worktree | 完整 session（worktree） | 无 | 无 |

### 数据流

```
[main.rs 父进程]
    │
    ├─ 计算 use_subprocess_tui
    ├─ 若为 true：创建 trace_only DiagnosticsCollector
    │            （session dir 在主仓库，仅 trace/log）
    │
    ├─ 启动 TUI / spawn 子进程 --rpc --worktree-path <wt>
    │
    └─ 子进程 current_dir = worktree
            │
            └─ [子进程 main.rs]
                 │
                 ├─ --rpc 分支：创建完整 DiagnosticsCollector
                 │            （session dir 在 worktree）
                 ├─ 跑 EventLoop → 写 recovery.jsonl / drift.jsonl / ...
                 └─ 写 pointer 文件到主仓库 .ralph/
```

---

## Implementation Units

- [ ] U1. **给 `DiagnosticsCollector` 增加 `trace_only` 模式**

**Goal:** 让 subprocess TUI 父进程可以只创建 session dir 与 trace 支持，不实例化任何 loop 级 logger。

**Requirements:** R1, R4

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/diagnostics/mod.rs`
- Test: `crates/ralph-core/src/diagnostics/mod.rs`（扩展现有 tests 模块）

**Approach:**
- 在 `DiagnosticsOptions` 增加 `pub trace_only: bool`，默认 `false`。
- 在 `DiagnosticsCollector::with_options` 中处理 `trace_only`：
  - 仍创建/复用 `session_dir`。
  - `full_diagnostics` 与 `runtime_diagnosis_artifacts` 的 logger 均不实例化。
  - 不创建 `recovery.jsonl`、`drift.jsonl`、`orchestration.jsonl`、`performance.jsonl`、`errors.jsonl`、`hook-runs.jsonl`、`agent-output.jsonl`、`prompt-log.md`。
  - `is_enabled()` 对 `trace_only` 返回 `true`（因为 session dir 存在）。
  - 新增 `is_trace_only()` 查询方法。
- 明确优先级：`full_diagnostics=true` 时忽略 `trace_only`（保持完整 session）；`trace_only=true` 且 `full_diagnostics=false` 且 `runtime_diagnosis_artifacts=false` 时才是 trace-only。

**Patterns to follow:**
- 沿用现有 `DiagnosticsOptions::from_env`、`from_env_with_telemetry` 的构造风格。
- 沿用现有 activation matrix 测试模式（参考 `test_activation_matrix_*` 系列）。

**Test scenarios:**
- Happy path：`trace_only=true` 时 `session_dir()` 返回存在的目录，且目录下没有 `recovery.jsonl`、`drift.jsonl`、`orchestration.jsonl`。
- Edge case：`trace_only=true` 但 `full_diagnostics=true` 时，仍创建完整 logger 集合（full 优先）。
- Edge case：`trace_only=true` 时 `is_trace_only()` 返回 true，`is_full_diagnostics()` 返回 false，`has_runtime_diagnosis_artifacts()` 返回 false。
- Regression：默认 `DiagnosticsOptions`（全 false）仍返回 disabled collector。
- Regression：`full_diagnostics=true` 与 `runtime_diagnosis_artifacts=true` 的现有行为不变。

**Verification:**
- 新增单元测试通过。
- `cargo nextest run -p ralph-core diagnostics::mod::tests` 全部通过。

---

- [ ] U2. **`main.rs` 在 subprocess TUI 父进程使用 `trace_only` 模式**

**Goal:** 消除父进程在主仓库创建空 recovery/drift 文件的行为。

**Requirements:** R1, R2, R4

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-cli/src/main.rs`
- Test: `crates/ralph-cli/src/main.rs`（扩展现有 tests 模块）

**Approach:**
- 在 `main()` 中、创建 `authoritative_diagnostics` 之前，计算 `use_subprocess_tui`：
  - 需要 `is_tty`（`stdin`/`stdout` 都是 terminal）。
  - 对 `Commands::Run(args)` / `Commands::Resume(args)` / `None`（默认 run）：`tui_enabled && !args.legacy_tui && !args.rpc`。
  - 注意：`None` 默认等价于 `ralph run`  interactive。
- 当 `use_subprocess_tui=true` 时，在构造 `DiagnosticsOptions` 时设置 `trace_only: true`。
- 其余情况（`--no-tui`、`--legacy-tui`、父进程自己就是 `--rpc` 子进程、autonomous、非 Run/Resume 命令）保持现有逻辑。

**Patterns to follow:**
- 与 `commands/run.rs` 中 `use_subprocess_tui` 计算逻辑保持一致，避免两份逻辑漂移。
- 保留 `read_telemetry_write_artifacts` 与 `RALPH_DIAGNOSTICS=1` 的现有语义。

**Test scenarios:**
- Happy path：`ralph run`（TTY，无额外 flag）→ `authoritative_diagnostics` 为 `trace_only`。
- Edge case：`ralph run --legacy-tui` → 非 trace_only（父进程跑 EventLoop）。
- Edge case：`ralph run --no-tui` → 非 trace_only（父进程跑 EventLoop）。
- Edge case：`ralph run --autonomous` → 非 trace_only（父进程跑 EventLoop）。
- Edge case：`ralph run --rpc`（手动启动子进程）→ 非 trace_only（该进程本身就是 loop 进程）。
- Edge case：`ralph resume`（TTY）→ trace_only。
- Edge case：非 TTY → 非 trace_only（不会 spawn subprocess TUI）。
- Regression：`is_diagnostics_eligible_command` 的现有测试不变。

**Verification:**
- 新增/修改单元测试通过。
- `cargo nextest run -p ralph-cli --bin ralph main` 相关测试通过（注意 ralph-cli 包串行跑 nextest）。

---

- [ ] U3. **确保子 RPC 进程在 worktree 创建完整 diagnostics session**

**Goal:** 验证并保障实际跑 `EventLoop` 的子进程仍在 worktree 内生成真实 recovery/drift 等文件。

**Requirements:** R3

**Dependencies:** U2

**Files:**
- Modify：无需新代码，主要依赖 U2 后子进程重新进入 `main.rs` 的路径
- Test：`crates/ralph-cli/src/loop_runner/tests.rs` 或新增 worktree diagnostics 集成测试

**Approach:**
- 子进程被 `run_subprocess_tui` 以 `current_dir(&args.workspace)` spawn，且 `--rpc` flag 为 true。
- 子进程进入 `main.rs` 后，`rpc_enabled=true`，`use_subprocess_tui=false`，因此会创建完整 diagnostics session（非 trace_only）。
- 由于 cwd 已被设为 worktree，`DiagnosticsCollector::with_options(..., base_path=".")` 会在 worktree 内创建 `.ralph/diagnostics/<timestamp>/`。
- `EventLoop::with_context` 会复用该 collector（通过 `LoopContext::with_prebuilt_diagnostics`）。

**Patterns to follow:**
- 沿用现有 `loop_runner/tests.rs` 中 worktree 相关测试的 TempDir + git init 模式。

**Test scenarios:**
- Integration（mock）：
  - 设置 TempDir git repo，创建 worktree，模拟子进程 cwd=worktree + `--rpc` + `RALPH_DIAGNOSTICS=1`。
  - 断言 worktree 下 `.ralph/diagnostics/<session>/recovery.jsonl` 与 `drift.jsonl` 最终被创建并写入内容。
  - 断言主仓库 `.ralph/diagnostics/` 下不存在同名空 `recovery.jsonl` / `drift.jsonl`。
- Integration：`telemetry.runtime_diagnosis.write_artifacts=true` 配置下，子进程仍创建完整 worktree session。
- Regression：非 worktree 的 `ralph run`（TTY default TUI）→ 子进程在主仓库创建完整 session，主仓库的 recovery/drift 有真实内容。
- Regression：`ralph run --no-tui` 直接在主仓库创建完整 session。

**Verification:**
- 新增集成测试通过。
- `./scripts/run-tests.sh` 或 `cargo nextest run --workspace --exclude ralph-e2e` 全绿。

---

- [ ] U4. **`ralph diagnose` 支持定位已结束的 worktree loop session**

**Goal:** loop 结束后，`ralph diagnose` 不再 fallback 到主仓库空壳，而是能解析到 worktree 真实 diagnostics。

**Requirements:** R5

**Dependencies:** U3

**Files:**
- Modify: `crates/ralph-core/src/diagnostics/mod.rs`（新增写 pointer 文件方法）
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`（在 loop 启动后调用 pointer 写入）
- Modify: `crates/ralph-cli/src/commands/diagnose.rs`（新增 pointer fallback）
- Test: `crates/ralph-cli/src/commands/diagnose.rs`（扩展现有 tests 模块）

**Approach:**
1. 在 `DiagnosticsCollector` 新增方法（例如 `write_session_pointer(&self, repo_root: &Path)`）：
   - 仅当 `session_dir` 是 worktree 内路径时写入 pointer（通过比较 `session_dir` 与 `repo_root` 判断是否位于 worktree）。
   - 写入主仓库 `.ralph/diagnostics-session-pointer.json`，内容为 `{ "session_path": "<abs-path>", "written_at": "..." }`。
   - 使用 atomic write（`NamedTempFile::persist`）保证读不到半写文件。
2. 在 `run_loop_impl` 中，当确认是 worktree loop（`!loop_context.is_primary()`）且 diagnostics session 创建成功后，调用 `collector.write_session_pointer(loop_context.repo_root())`。
3. 在 `ralph diagnose` 的 `resolve_diagnostics_root_via_loops` 中：
   - 保持现有逻辑：优先按 alive loop 的 `workspace` 解析。
   - 若无 alive loop 或 alive loop 的 diagnostics root 不存在，则读取主仓库 `.ralph/diagnostics-session-pointer.json`。
   - 若 pointer 存在且指向有效目录，使用该目录；否则 fallback 到主仓库 `.ralph/diagnostics`。

**Patterns to follow:**
- atomic write 模式已在 `write_diagnosis_summary_seed` / `write_active_activations` 中使用，直接复用。
- diagnose 解析的现有 fallback 逻辑保持不变，pointer 只是新增一层 fallback。

**Test scenarios:**
- Happy path：worktree loop 结束后，主仓库 `.ralph/diagnostics-session-pointer.json` 存在且指向 worktree diagnostics root；`ralph diagnose` 能渲染该 session 的 recovery 内容。
- Edge case：pointer 文件存在但指向已删除路径 → diagnose fallback 到主仓库 `.ralph/diagnostics`，返回 `NoSession`。
- Edge case：显式 `--diagnostics-root` 覆盖 pointer，不使用 pointer。
- Edge case：主仓库 loop（非 worktree）不写 pointer 文件。
- Edge case：多个 worktree loop 先后运行，pointer 指向最后一个 session。
- Regression：无 pointer 文件且 loops.json 为空时，行为与现在一致（fallback 主仓库，NoSession）。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph commands::diagnose` 测试通过。
- `./scripts/run-tests.sh` 全绿。

---

## System-Wide Impact

- **Interaction graph：**
  - `main.rs` → `DiagnosticsCollector`：新增 `trace_only` 控制路径。
  - `run_loop_impl` → `DiagnosticsCollector`：新增 pointer 文件写入。
  - `ralph diagnose` → pointer 文件：新增解析路径。
- **Error propagation：**
  - pointer 写入失败只发 `tracing::warn!`，不阻塞 loop（与现有 `write_diagnosis_summary_seed` 一致）。
  - pointer 读取失败 graceful fallback 到现有主仓库路径。
- **State lifecycle risks：**
  - pointer 文件是「最后 session」缓存，不会自动清理；若用户删除 worktree，diagnose 会检测到目录不存在并 fallback。
  - 主仓库空 recovery/drift 文件不再产生，消除了 diagnose 误解析风险。
- **API surface parity：**
  - `DiagnosticsOptions` 新增 public 字段 `trace_only`，属于 additive change，不影响现有调用方。
- **Integration coverage：**
  - subprocess TUI + worktree 的端到端路径（父 trace-only + 子完整 session + pointer 写入）必须通过集成测试验证，单元测试无法覆盖跨进程 cwd 与 session 创建。
- **Unchanged invariants：**
  - `RALPH_DIAGNOSTICS=1` 的完整 diagnostics 内容不变，只是位置从主仓库空壳转移到 worktree 真实 session。
  - `--no-tui`、`--legacy-tui`、`--autonomous` 等模式的 diagnostics 行为不变。
  - `loops.json` 的写入/清理语义不变。

---

## Risks & Dependencies

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `trace_only` 模式误用于真正跑 loop 的进程，导致 recovery/drift 丢失 | 低 | 高 | 严格限制 `use_subprocess_tui` 计算：只有确认会 spawn `--rpc` 子进程的父进程才用 `trace_only`；`--rpc` 子进程自身不用。新增单元测试覆盖所有命令组合。 |
| `main.rs` 与 `commands/run.rs` 的 subprocess TUI 判断逻辑漂移 | 中 | 中 | 在计划注释与测试中明确两份逻辑必须同步；必要时提取共享 helper 到 `crates/ralph-cli/src/cli/`。 |
| pointer 文件导致 `ralph diagnose` 指向 stale session | 低 | 中 | pointer 只作为无 alive loop 时的 fallback；显式 `--diagnostics-root` 与 alive loop 均优先于 pointer；实现时检查目录存在性。 |
| 现有 diagnostics 集成测试依赖主仓库 session 路径 | 低 | 中 | 这些测试不经过 subprocess TUI，不受 `trace_only` 影响；U3 的集成测试专门覆盖 worktree 路径。 |

---

## Documentation / Operational Notes

- 若用户之前已习惯在主仓库 `.ralph/diagnostics/` 查看 worktree loop 的诊断，需更新为查看 worktree 路径。
- `ralph diagnose` 在 loop 结束后仍可自动找到 worktree session，无需手动 `--diagnostics-root`。
- `AGENTS.md` / `CLAUDE.md` 中「Worktree Loops」与「Diagnostics」段若提到 diagnostics 位置，需在实现后同步更新（但本计划范围只到代码修复，文档同步作为 follow-up）。

---

## Sources & References

- 相关代码：
  - `crates/ralph-core/src/diagnostics/mod.rs`
  - `crates/ralph-cli/src/main.rs`
  - `crates/ralph-cli/src/commands/run.rs`
  - `crates/ralph-cli/src/commands/diagnose.rs`
  - `crates/ralph-core/src/loop_context.rs`
- 测试入口规则：`docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md`
- 项目规范：`AGENTS.md` / `CLAUDE.md`
