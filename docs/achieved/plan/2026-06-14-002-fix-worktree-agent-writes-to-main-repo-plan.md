---
title: 修复 worktree 模式下 Agent 写入主仓库而非 worktree 的 bug
type: fix
status: active
date: 2026-06-14
---

# 修复 worktree 模式下 Agent 写入主仓库而非 worktree 的 bug

## Overview

当用户执行 `ralph -H builtin:ce-executor-isolated run --worktree` 时，Ralph 会创建 git worktree 并在其中启动子进程运行 Agent。当前实现中，子进程的**实际 CWD 已正确切换为 worktree**，但继承的 `PWD` 环境变量仍指向主仓库，且 prompt file 被以主仓库绝对路径转发。Agent/Claude 的部分路径解析依赖 `PWD`/prompt 文件位置，导致代码修改落到了主分支。

本计划通过最小化修改，修复 worktree 子进程的环境隔离，确保 Agent 的文件写入落在 worktree。

---

## Problem Frame

- `ralph run --worktree` 会创建 worktree，父进程通过 `run_subprocess_tui` 在 worktree 路径下 spawn `--rpc` 子进程。
- 子进程实际 `cwd` 是 worktree，但环境变量 `PWD` 继承自主进程（主仓库路径）。
- `forward_prompt_args` 在 worktree 模式下把默认 `PROMPT.md` 以主仓库绝对路径转发给子进程（`-P /主仓库/PROMPT.md`）。
- Agent 在解析相对路径或选择项目根目录时，可能以 `PWD` 或 prompt file 所在目录为准，从而把 `crates/ralph-core/src/worktree.rs` 等文件写到了主仓库。

---

## Requirements Trace

- R1. worktree 子进程的环境变量 `PWD` 必须与其真实 CWD 一致，即指向 worktree 路径。
- R2. worktree 子进程读取的 prompt file 不应把 Agent 上下文锚定到主仓库；优先让 Agent 在 worktree 内解析相对路径。
- R3. `CliExecutor` 不应依赖可能漂移的 `std::env::current_dir()`，而应显式使用 Ralph 配置的 workspace root。
- R4. 必须添加回归测试，防止 worktree 隔离在后续改动中再次被破坏。
- R5. 修改后须通过 `cargo nextest run`（项目指定测试入口）。

---

## Scope Boundaries

- **In scope:** worktree 模式下的子进程环境隔离、prompt file 转发、`CliExecutor` 的 CWD 选择、回归测试。
- **Out of scope:** 重新设计 worktree 创建流程、修改 git worktree 同步逻辑、改动 preset 的事件/schema、改动 Agent 提示词内容。
- **Deferred to follow-up work:** 若发现 PTY executor 或其他 backend adapter 存在类似 `current_dir()` 依赖，可在后续统一审计（本计划只做 cli_executor 的最小加固）。

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-cli/src/commands/run.rs`
  - `run_subprocess_tui` 使用 `.current_dir(&args.workspace)` 设置子进程 CWD（已正确）。
  - `forward_prompt_args` 在 worktree 模式下把 `PROMPT.md` 以主仓库绝对路径转发。
  - `SubprocessTuiArgs` 携带 `workspace`（worktree 路径）和 `worktree_path`。
- `crates/ralph-adapters/src/cli_executor.rs`
  - `execute` 方法使用 `std::env::current_dir()` 设置 agent command 的 `current_dir`。
  - `inject_ralph_runtime_env` 已正确注入 `RALPH_WORKSPACE_ROOT`。
- `crates/ralph-core/src/worktree.rs` / `crates/ralph-core/src/loop_context.rs`
  - worktree 创建、路径解析、symlink 设置。

### Institutional Learnings

- `docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md`：测试入口必须用 `cargo nextest run`，`ralph-cli` 包测试需要串行。
- `AGENTS.md`：默认走并发，`ralph-cli` 走串行（`.config/nextest.toml`）。

---

## Key Technical Decisions

- **决策 1：优先修复 `PWD` 环境变量。** 这是最直接根因。在 `run_subprocess_tui` spawn 子进程时，显式把 `PWD` 环境变量设为 `args.workspace`。
  - 理由：不改变现有路径转发逻辑，风险最小，能立即消除 Agent 因 `PWD` 漂移而写错目录的问题。
- **决策 2：worktree 模式下把 prompt file 复制/同步到 worktree，子进程读取 worktree 内的副本。**
  - 理由：当前 `forward_prompt_args` 以主仓库绝对路径转发，会把 Agent 的上下文锚定到主仓库。改为在 worktree 内提供 prompt file，并让子进程用 worktree-relative 路径读取，可进一步消除歧义。
  - 实现方式：在创建 worktree 后，把主仓库的 prompt file 复制到 worktree 根目录（`PROMPT.md`），然后 `forward_prompt_args` 对该 worktree 转发相对路径 `PROMPT.md`。
- **决策 3：`CliExecutor` 优先使用 `RALPH_WORKSPACE_ROOT` 作为 agent CWD。**
  - 理由：`std::env::current_dir()` 在子进程中被正确设为 worktree，但为了防御未来任何 CWD 漂移，显式使用 Ralph 注入的 workspace root 更可靠。

---

## Open Questions

### Resolved During Planning

- **Q1:** 是否必须修改 `forward_prompt_args`？只修 `PWD` 不够吗？
  - **A1:** 只修 `PWD` 能解决当前观察到的症状，但 prompt file 仍以主仓库绝对路径存在，Agent 的 project-root heuristics 仍可能被误导。为最小化回归风险，把 prompt file 同步到 worktree 并用相对路径转发是推荐的 defense-in-depth。
- **Q2:** 修改 `CliExecutor` 会不会影响非 worktree 模式？
  - **A2:** 不会。`RALPH_WORKSPACE_ROOT` 在 primary 模式下等于 `current_dir()`，行为保持一致。

### Deferred to Implementation

- **D1:** 是否需要同步 `.ralph/agent/` 下的其他文件（如 `context.md`）到 worktree？已在 `LoopContext::setup_worktree_symlinks` 处理，无需额外改动。
- **D2:** PTY executor 是否也有类似 `current_dir()` 风险？ deferred 到后续审计，本计划不处理。

---

## Implementation Units

- [ ] U1. **同步 worktree 子进程的 `PWD` 环境变量**

**Goal:** 消除子进程真实 CWD 与 `PWD` 环境变量不一致导致的 Agent 路径解析错误。

**Requirements:** R1

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-cli/src/commands/run.rs`

**Approach:**
- 在 `run_subprocess_tui` 中构造 `Command` 时，除了 `.current_dir(&args.workspace)`，再显式设置环境变量 `PWD` 为 `args.workspace` 的字符串形式。
- 确保仅在 worktree 模式（`args.worktree_path.is_some()`）或 `args.workspace` 不等于父进程 CWD 时覆盖 `PWD`，避免影响 primary 模式。

**Patterns to follow:**
- 现有代码已在 `crates/ralph-adapters/src/cli_executor.rs` 和 `crates/ralph-adapters/src/pty_executor.rs` 中使用 `.env("RALPH_WORKSPACE_ROOT", workspace_root)` 注入环境变量，可参考其风格。

**Test scenarios:**
- Happy path: 在 worktree 模式下 spawn 子进程后，读取子进程环境变量 `PWD`，应等于 worktree 绝对路径。
- Edge case: primary 模式下 `PWD` 应保持不变（不强制覆盖为 workspace）。
- Integration: 子进程内调用 `std::env::var("PWD")` 与 `std::env::current_dir()` 返回一致。

**Verification:**
- 新增/扩展的单元测试通过。
- 手动检查：worktree 运行时，子进程 `PWD` 环境变量与实际 CWD 一致。

---

- [ ] U2. **把 prompt file 同步到 worktree 并用相对路径转发**

**Goal:** 避免 Agent 的 prompt 上下文被锚定到主仓库。

**Requirements:** R2

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-cli/src/commands/run.rs`

**Approach:**
- 在父进程创建 worktree 后、spawn 子进程前，如果主仓库存在默认 `PROMPT.md`，将其复制到 worktree 根目录（`worktree_path.join("PROMPT.md")`）。
- 修改 `forward_prompt_args`：当 `args.worktree_path.is_some()` 且 prompt file 是相对路径（或默认未指定）时，转发相对路径 `PROMPT.md`，不再构造主仓库绝对路径。
- 保持现有测试行为不变：非 worktree 模式下 `forward_prompt_args` 逻辑不变。

**Patterns to follow:**
- worktree 文件同步已有 `sync_working_directory_to_worktree`，但那是创建时复制 untracked/modified 文件。这里只需显式复制 prompt file，逻辑更简单。
- `spawn_worktree_loop` 中已调用 `context.generate_context_file`，可在其后或附近添加 prompt file 复制逻辑。

**Test scenarios:**
- Happy path: worktree 模式下默认 `PROMPT.md` 被复制到 worktree，子进程 argv 包含 `-P PROMPT.md`（相对路径）。
- Edge case: 主仓库没有 `PROMPT.md` 时，不复制，也不转发 `-P`。
- Error path: 复制失败应记录 warning，不应阻塞启动（与现有 sync 错误处理一致）。
- Integration: 子进程启动后能正常读取 worktree 内的 `PROMPT.md`。

**Verification:**
- `forward_prompt_args_tests` 中新增 worktree 模式相对路径测试。
- worktree 运行时，子进程命令行不再出现主仓库绝对路径的 `-P`。

---

- [ ] U3. **加固 `CliExecutor` 使用 `RALPH_WORKSPACE_ROOT` 作为 agent CWD**

**Goal:** 防御未来 CWD 漂移，确保 agent 始终在 Ralph 配置的 workspace 下运行。

**Requirements:** R3

**Dependencies:** None（可与 U1 并行）

**Files:**
- Modify: `crates/ralph-adapters/src/cli_executor.rs`

**Approach:**
- 在 `CliExecutor::execute` 中，不再使用 `std::env::current_dir()` 作为 `cwd`。
- 优先使用 `self.workspace_root`（`AcpExecutor` 已持有）作为 agent command 的 `current_dir`。
- `inject_ralph_runtime_env` 继续使用该 `cwd`/`workspace_root`。
- 保留 fallback：若 `workspace_root` 为空或无效，再回退到 `std::env::current_dir()`。

**Patterns to follow:**
- `AcpExecutor::new(backend, workspace_root)` 已接收 workspace root；`CliExecutor` 应直接使用该字段。
- `crates/ralph-adapters/src/pty_executor.rs` 已使用 `self.config.workspace_root` 设置 `cmd_builder.cwd()`，可参考。

**Test scenarios:**
- Happy path: `CliExecutor` spawn 的 command 的 `current_dir` 等于 `workspace_root`。
- Edge case: `workspace_root` 为空时回退到 `current_dir()`，行为不变。
- Integration: 通过 mock backend 验证收到的工作目录参数。

**Verification:**
- 修改 `crates/ralph-adapters/src/cli_executor.rs` 的现有测试或新增断言，验证 command 的 cwd。
- 全量测试通过。

---

- [ ] U4. **新增回归测试覆盖 worktree 环境隔离**

**Goal:** 防止 worktree 子进程的 `PWD`、prompt file 路径、`CliExecutor` cwd 在以后被改回有问题的状态。

**Requirements:** R4

**Dependencies:** U1, U2, U3

**Files:**
- Modify: `crates/ralph-cli/src/commands/run.rs`（`forward_prompt_args_tests` 模块）
- Modify: `crates/ralph-adapters/src/cli_executor.rs`（现有测试模块）
- 可选新增：若现有测试模块难以覆盖，可在 `crates/ralph-cli/src/commands/run.rs` 或 `crates/ralph-cli/src/loop_runner/tests.rs` 新增集成测试。

**Approach:**
- 在 `forward_prompt_args_tests` 中新增：
  - worktree 模式下转发相对路径 `PROMPT.md`。
  - worktree 模式下不再转发主仓库绝对路径。
  - 自定义 `-P` 相对路径在 worktree 模式下仍转发为相对路径（因为子进程 cwd 已是 worktree）。
- 在 `cli_executor` 测试中新增/修改断言，验证 command 的 cwd 等于 `workspace_root`。
- 若条件允许，新增一个集成测试：创建临时 git repo + worktree，spawn 一个简单子进程，验证其 `PWD` 环境变量等于 worktree 路径。

**Test scenarios:**
- Happy path: `forward_prompt_args` 在 worktree 模式下返回 `[-P, PROMPT.md]`。
- Edge case: worktree 模式下用户显式 `-P docs/plans/foo.md`，转发相对路径不变。
- Error path: worktree 模式下主仓库无 `PROMPT.md`，`forward_prompt_args` 不注入 `-P`。
- Integration: worktree 子进程 `PWD` == worktree path。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- forward_prompt_args` 通过。
- `cargo nextest run -p ralph-adapters` 通过。

---

- [ ] U5. **运行完整测试套件验证**

**Goal:** 确保修改没有破坏现有功能。

**Requirements:** R5

**Dependencies:** U1, U2, U3, U4

**Files:**
- 无需修改文件，仅运行测试。

**Approach:**
- 运行 `./scripts/run-tests.sh`（项目推荐入口）。
- 若该脚本不可用，运行：
  - `cargo nextest run --workspace --exclude ralph-e2e`
  - `cargo test --workspace --exclude ralph-e2e --doc`
- 重点关注 `ralph-cli` 包（串行）和 `ralph-adapters` 包。

**Verification:**
- 所有测试通过，无新增 clippy/fmt 警告。

---

## System-Wide Impact

- **Interaction graph:**
  - `commands/run.rs` 的 `run_subprocess_tui` → 子进程 `ralph run --rpc` → `CliExecutor` → agent backend。
  - 修改后，整条链上的 CWD/prompt 根目录信号一致指向 worktree。
- **Error propagation:**
  - prompt file 复制失败为 warning，不阻塞启动，避免引入新的失败模式。
- **State lifecycle risks:**
  - worktree 内的 `PROMPT.md` 是副本，不影响主仓库文件。
  - 删除 worktree 时会一并清理该副本。
- **API surface parity:**
  - CLI 参数和行为不变；仅内部转发逻辑和子进程环境变量调整。
- **Unchanged invariants:**
  - `LoopContext::worktree` 的路径语义不变。
  - primary 模式（非 worktree）下 `forward_prompt_args` 行为不变。
  - ` CliExecutor` 在 primary 模式下 cwd 仍等于当前进程 CWD（因为 `workspace_root == current_dir()`）。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|-----------|
| 修改 `PWD` 后，某些 shell/bash tool 行为改变 | 这是预期行为；worktree 模式本就应让 Agent 以为自己在 worktree。 |
| prompt file 复制到 worktree 会轻微增加启动开销 | 仅复制一个文件，开销可忽略；失败也不阻塞。 |
| `CliExecutor` 改用 `workspace_root` 影响非 worktree 模式 | `workspace_root` 在 primary 模式下等于当前目录，行为一致；保留 fallback。 |
| 回归测试在临时目录中创建 git repo 较复杂 | 已有 `worktree.rs` 测试使用 `init_git_repo` helper，可复用。 |

---

## Documentation / Operational Notes

- 无需更新用户文档；CLI 行为对用户透明。
- 可在 `docs/solutions/developer-experience/` 下补充一篇简短 note，记录 worktree 隔离必须保持 `PWD` 同步，但这不是本计划强制的。

---

## Sources & References

- 相关代码：
  - `crates/ralph-cli/src/commands/run.rs`
  - `crates/ralph-adapters/src/cli_executor.rs`
  - `crates/ralph-core/src/worktree.rs`
  - `crates/ralph-core/src/loop_context.rs`
- 项目规范：`AGENTS.md`（测试入口、并发/串行规则）
