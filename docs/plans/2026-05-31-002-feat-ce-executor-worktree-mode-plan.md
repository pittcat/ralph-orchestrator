---
title: ce-executor Worktree 隔离模式
type: feat
status: completed
date: 2026-05-31
origin: docs/brainstorms/ce-executor-worktree-mode-requirements.md
commit: 52f3311
---

# ce-executor Worktree 隔离模式

## Overview

为 `ralph run` 添加 `--worktree` 显式参数，使 ce-executor（及其他 preset）能够在 git worktree 中隔离执行。当用户传入 `--worktree` 时，ralph 在当前目录之外创建一个独立的 worktree，所有代码改动、commit、测试均在 worktree 中进行，主工作区完全不受影响。完成后 worktree 和 branch 保留，由用户手动决定是否 merge。

同时修改 ce-executor preset，移除其默认的 feature branch 创建行为——不传 `--worktree` 时，ce-executor 直接在当前分支工作。

为避免 ce-executor 在原地执行时把当前分支已有改动纳入审查，Coordinator 在启动时记录 `start_sha`，Review Coordinator 优先基于 `start_sha..HEAD` 计算 diff。`start_sha` 不可用时才回退到现有 base 检测链。

---

## Problem Frame

当前 ce-executor preset 的 Executor hat 会在当前目录创建 feature branch（如 `feat/plan-name`），这带来三个问题：

1. **污染主工作区**：即使计划简单，也会留下一个可能长期存在的 branch
2. **无法并行**：多个 ce-executor 任务不能同时运行
3. **缺乏隔离**：master 上的未提交改动可能与 ce-executor 的改动冲突

Ralph 已有成熟的 worktree 基础设施（`worktree.rs`、parallel loops、`merge-loop` preset），但仅在锁竞争时自动 spawn worktree。用户希望**显式控制**何时使用 worktree 隔离。

---

## Requirements Trace

- R1. `ralph run --worktree` 自动创建 git worktree 并在其中执行（对应 origin WM-01）
- R2. 不传 `--worktree` 时 ce-executor 不再创建 feature branch（对应 origin WM-02）
- R3. Worktree 完成后保留，不触发 auto-merge（对应 origin WM-09）
- R4. Worktree 中的 `.ralph/` 状态隔离，memories 共享（对应 origin WM-04, WM-05）
- R5. 主工作区完全不受 worktree 内 commit/branch 操作影响（对应 origin WM-08）
- R6. `--worktree` 与 `--exclusive` 是冲突参数，同时传入时报错
- R7. ce-executor 每次运行记录 `start_sha`，review 阶段优先审查 `start_sha..HEAD` 的本次改动

**Origin actors:** 终端用户（运行 ce-executor 的开发者）
**Origin flows:** F1 显式 worktree 执行流程，F2 默认原地执行流程

---

## Scope Boundaries

- 不修改现有 parallel loop 的自动 spawn 逻辑（锁竞争时仍自动进 worktree）
- 不提供 worktree 的自动 merge 或清理功能（用户手动处理）
- 不修改其他 preset 的默认行为（仅 ce-executor 移除 branch 创建）
- 不新增 web API 或 TUI 界面（`ralph loops` 命令族已足够）

### Deferred to Follow-Up Work

- worktree 命名基于 plan 文件内容（当前复用现有 `LoopNameGenerator` 和 `ralph/<loop-id>` branch 命名）
- `--worktree` 支持与其他 preset 的默认 worktree 行为配置（当前仅 CLI flag 触发）

---

## Context & Research

### Relevant Code and Patterns

- **Worktree 创建**：`crates/ralph-cli/src/main.rs` 中 `handle_active_lock()`（第 1434 行）已实现完整的 worktree 创建流程：生成 loop ID → `ensure_gitignore` → `create_worktree` → `LoopContext::worktree` → `setup_worktree_symlinks` → `generate_context_file` → `LoopEntry` 注册
- **LoopContext worktree 支持**：`crates/ralph-core/src/loop_context.rs` 已提供 `worktree()` 构造函数、symlink 设置、`ensure_directories()` 等完整路径管理
- **Lock 获取逻辑**：`crates/ralph-cli/src/main.rs` 第 1706-1829 行，`run_command()` 中的锁检测/获取/降级到 worktree 流程
- **run_loop_impl 签名**：`crates/ralph-cli/src/loop_runner.rs` 第 107 行，接受 `auto_merge_override: Option<bool>` 参数
- **Auto-merge 触发**：`run_command` 中 `args.no_auto_merge` → `auto_merge_override = Some(false)`
- **Subprocess TUI 模式**：`run_subprocess_tui()` 当前由 parent 构造 `ralph run --rpc ...` 子进程；默认 TUI 下真正执行 loop 的是 child，因此 `--worktree` 必须转发给 child，由 child 创建 worktree
- **Loop 列表展示**：`loop_runner` 完成时会从 `LoopRegistry` deregister；`ralph loops list` 之后通过扫描 `ralph/*` worktree 展示保留的 worktree，当前状态表现为 `orphan`

### Existing Patterns to Follow

- `handle_active_lock` 中的 worktree 初始化流程（创建 → symlink → context file → registry）
- `RunArgs` 中 `--exclusive` 和 `--no-auto-merge` 的 flag 定义模式
- `LoopContext::worktree` + `setup_worktree_symlinks` 的组合使用

---

## Key Technical Decisions

- **在 non-TUI/RPC 路径的 lock 获取前拦截 `--worktree`**：`run_command()` 中在锁检测逻辑之前检查 `args.worktree`，若启用且当前进程是真正执行 loop 的进程，则直接创建 worktree 并跳过整个 lock 获取流程。这比修改 `handle_active_lock` 更干净，因为 `--worktree` 的语义是"用户显式要求 worktree"，与"锁竞争导致的 worktree"是不同场景。
- **复用 `handle_active_lock` 中的 worktree 创建逻辑**：将 worktree 创建代码（生成 loop ID、create_worktree、LoopContext、symlink、context file）提取为独立函数 `spawn_worktree_loop()`，供 `handle_active_lock` 和 `--worktree` 路径共同调用。避免代码重复。
- **默认 TUI 下由 child 创建 worktree**：当 `use_subprocess_tui` 为 true 时，parent 不创建 worktree，只把 `--worktree` 转发给 `ralph run --rpc` child。这样 worktree 创建、config workspace 更新、lock 跳过、preflight、registry 注册都发生在实际执行 loop 的进程中，避免 parent/child cwd 不一致。
- **`--worktree` 与 `--exclusive` 冲突**：`--worktree` 表示立即进入 worktree 隔离执行，`--exclusive` 表示等待主工作区 lock 后原地独占执行。两者同时传入没有一致语义，应在 clap 层声明冲突并报错。
- **worktree 模式下禁用 auto-merge**：`--worktree` 自动设置 `auto_merge_override = Some(false)`，与 `--no-auto-merge` 效果叠加。确保 worktree 完成后不会自动进入 merge queue。
- **完成后保留为手动处理 worktree**：显式 `--worktree` 完成后不进 merge queue；`LoopRegistry` 退出时仍 deregister。`ralph loops list` 可以通过 worktree 扫描看到保留目录，当前显示为 `orphan` 是可接受的手动处理状态。
- **ce-executor preset 移除 branch 创建，并记录 `start_sha`**：Coordinator 和 Executor hat 的 branch 创建指令删除；Coordinator 初始化时写入 `start_sha = git rev-parse HEAD`；Review Coordinator 优先用 `start_sha..HEAD` 生成 diff 和文件列表。

---

## Open Questions

### Resolved During Planning

- **worktree 命名**：复用现有 `LoopNameGenerator`，生成 `ralph/<loop-id>` 风格的 branch 名。worktree 目录为 `.worktrees/<loop-id>/`。（决议：复用现有机制，不基于 plan 文件名，避免冲突和复杂度）
- **是否获取主 lock**：不获取。worktree 有独立的 `.ralph/` 目录，不需要主 lock 协调。（决议：worktree 模式不持有 `.ralph/loop.lock`）
- **memories symlink 失败怎么办**：复用现有行为——非 Unix 平台不创建 symlink，worktree 有独立的 memories 文件。当前代码已有 `#[cfg(not(unix))]` stub。（决议：保持现有跨平台行为）
- **subprocess TUI 如何兼容 `--worktree`**：parent 只转发 `--worktree`，child `--rpc` 进程创建 worktree。（决议：避免 parent 创建后 child 仍在原 cwd 执行）
- **完成后 loops 状态**：接受当前 `orphan` 展示语义，表示保留待用户手动 attach/diff/discard/merge 的 worktree。（决议：不新增 completed/manual 状态模型）
- **`--worktree` + `--exclusive`**：参数冲突，直接报错。（决议：不做优先级猜测）

### Deferred to Implementation

- `spawn_worktree_loop` 提取后，`handle_active_lock` 的签名是否需要调整（如是否继续接受 `pending_worktree_registration` 参数）
- ce-executor preset 中 `start_sha` 不可用时的 fallback 行为：保留现有 diff base 检测链（`origin/main` → `origin/master` → `main` → `master` → `HEAD~1`）

---

## Implementation Units

- [ ] U1. **CLI 添加 `--worktree` flag**

**Goal:** 在 `ralph run` 命令中添加 `--worktree` 参数

**Requirements:** R1, R6

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-cli/src/main.rs`（`RunArgs` struct）
- Modify: `crates/ralph-cli/src/main.rs`（`run_command` 中 default `RunArgs` 构造，第 1239 行附近）

**Approach:**
- 在 `RunArgs` 的 "Multi-Loop Concurrency Options" 区块中添加 `#[arg(long, conflicts_with = "exclusive")] worktree: bool`
- 在 `None => { let args = RunArgs { ... } }` 默认构造中添加 `worktree: false`
- 在 `SubprocessTuiArgs` 中添加 `worktree: bool` 字段，确保默认 TUI parent 能把该 flag 转发给 child

**Test scenarios:**
- Happy path: `ralph run --help` 输出包含 `--worktree` flag 说明
- Edge case: `--worktree` 与 `--exclusive` 同时传入时报 clap 参数冲突错误
- TUI forwarding: 默认 TUI 模式下 parent 构造的 child args 包含 `--worktree`

**Verification:**
- `cargo build` 编译通过
- `ralph run --help | grep worktree` 显示 flag

---

- [ ] U2. **提取并复用 worktree 创建逻辑**

**Goal:** 将 `handle_active_lock` 中的 worktree 创建代码提取为独立函数，供显式 `--worktree` 路径复用

**Requirements:** R1, R4

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-cli/src/main.rs`
- Test: `crates/ralph-cli/src/main.rs`（现有测试）

**Approach:**
- 在 `handle_active_lock` 上方（或同一文件底部）创建 `spawn_worktree_loop()` 函数，接收 `workspace_root`, `prompt_summary`, `loop_naming`, `pending_worktree_registration`，返回 `(LoopContext, Option<LockGuard>)`
- 将 `handle_active_lock` 中 `else` 分支（第 1463-1521 行）的 worktree 创建逻辑整体移入新函数
- `handle_active_lock` 中改为调用 `spawn_worktree_loop(...)`
- `run_command` 的 non-TUI/RPC 执行路径在 lock 获取逻辑之前检查 `args.worktree`：
  - 若为 true，调用 `spawn_worktree_loop(...)` 获取 `loop_context`
  - 跳过整个 `LoopLock::inspect` / `try_acquire` 分支
  - `lock_guard = None`（worktree 不持有主 lock）
  - 默认 TUI parent 不在此处创建 worktree；该路径由 child `--rpc --worktree` 执行

**Patterns to follow:**
- `handle_active_lock` 现有逻辑（loop ID 生成、gitignore、create_worktree、symlink、context file、registry entry）

**Test scenarios:**
- Happy path: `--worktree` 传入时，`.worktrees/` 下创建新目录，branch 为 `ralph/<loop-id>`
- Integration: worktree 创建后，`loop_context.is_primary()` 返回 false，`loop_context.workspace()` 指向 worktree 路径
- Error path: worktree 已存在时，`create_worktree` 返回 `AlreadyExists` 错误，正确传播给用户

**Verification:**
- `cargo test` 通过（包括现有 parallel loop 测试）
- 手动验证：`ralph run --worktree -p "test"` 后 `git worktree list` 显示新 worktree

---

- [ ] U3. **run_command 集成显式 worktree 模式**

**Goal:** 在 `run_command` 中连接 `--worktree` flag 与 worktree 创建逻辑，并禁用 auto-merge

**Requirements:** R1, R3

**Dependencies:** U2

**Files:**
- Modify: `crates/ralph-cli/src/main.rs`（`run_command` 函数，第 1706-1829 行 lock 获取区块）

**Approach:**
- 在 lock 获取逻辑（`let (loop_context, _lock_guard) = if use_subprocess_tui { ... } else { ... }`）中区分两种路径：
  - 若 `use_subprocess_tui` 为 true：parent 仍跳过 lock 获取并使用 primary context；不要创建 worktree。`run_subprocess_tui()` 会把 `--worktree` 转发给 child，由 child 执行下面的 non-TUI/RPC 路径。
  - 若 `use_subprocess_tui` 为 false 且 `args.worktree` 为 true：调用 `spawn_worktree_loop(...)` 获取 worktree `loop_context`，跳过 `LoopLock::inspect` / `try_acquire` 分支，`lock_guard = None`。
- `auto_merge_override` 计算改为：`if args.worktree || args.no_auto_merge { Some(false) } else { None }`
- 更新 `run_subprocess_tui()`：若 `args.worktree` 为 true，向 child args 添加 `--worktree`。不要用 parent 的 `current_dir(worktree)` 方案。

**Technical design:**
> *This illustrates the intended approach and is directional guidance for review, not implementation specification.*
>
> ```
> run_command(args):
>   if use_subprocess_tui:
>       loop_context = LoopContext::primary(workspace_root)
>       // child receives --worktree and creates worktree itself
>   else if args.worktree:
>       (loop_context, _lock_guard) = spawn_worktree_loop(...)
>   else:
>       // existing lock acquisition logic
>       (loop_context, _lock_guard) = ...
>   auto_merge_override = if args.worktree || args.no_auto_merge { Some(false) } else { None }
>   // rest of run_command unchanged
> ```

**Test scenarios:**
- Happy path: `ralph run --worktree -p "docs/plans/my-plan.md"` 成功在 worktree 中启动 loop
- Integration: worktree 模式下 `loop_context.is_primary()` 为 false，`config.core.workspace_root` 更新为 worktree 路径
- TUI integration: 默认 TUI 模式下 parent 不创建 worktree，child `--rpc --worktree` 创建 worktree；不会在原 cwd 重新获取 primary lock
- Error path: 非 git 仓库中传入 `--worktree` 时，`create_worktree` 返回 `NotARepo` 错误

**Verification:**
- `cargo test -p ralph-cli` 通过
- 手动验证：运行中 `ralph loops list` 显示该 worktree，状态为 running；完成后保留 worktree 并可通过 orphan/manual worktree 入口 attach/diff/discard

---

- [ ] U4. **修改 ce-executor preset（英文版）**

**Goal:** 移除 ce-executor 预设中创建 feature branch 的指令，并记录本次运行的 `start_sha`

**Requirements:** R2, R7

**Dependencies:** None（可与 U1-U3 并行）

**Files:**
- Modify: `presets/ce-executor.yml`

**Approach:**
- **Coordinator hat**：删除 "Environment Check" 区块中的 "If not on a feature branch, note the suggested branch name in context.md" 和 "Do not create branches (Executor handles that)"
- **Coordinator hat**：在创建 `context.md` 时记录：
  - `branch = git branch --show-current`
  - `start_sha = git rev-parse HEAD`
  - 如果 `git rev-parse HEAD` 失败，记录 `start_sha: unavailable` 和失败原因，供 review fallback 使用
- **Executor hat**：
  - 删除 "Environment Setup" 区块（"Check current branch / If not on a feature branch, create one"）
  - 删除 "Step Advancement" 中 `git diff --stat $(git branch --show-current)` 的 branch 引用（改为 `git diff --stat HEAD` 或保持原样——在 worktree 中 `git branch --show-current` 也能正常工作）
  - 保留 commit 逻辑（`git add <files>` + `git commit`）
- **Review Coordinator hat**：
  - 从 `.agents/scratchpad/ce-executor/{plan_name}/context.md` 读取 `start_sha`
  - 若 `start_sha` 是有效 SHA，使用 `git diff -U10 <start_sha>..HEAD` 和 `git diff --name-only <start_sha>..HEAD`
  - `git log` intent extraction 使用 `git log --oneline <start_sha>..HEAD`
  - 若 `start_sha` 不存在或无效，回退到现有 base 检测链
  - Empty diff handling 基于最终选定的 diff base 执行

**Test scenarios:**
- Happy path: 预设 YAML 语法有效，`cargo test -p ralph-cli test_preset_content_is_valid_yaml` 通过
- Integration: `ralph run -H builtin:ce-executor` 不再自动创建 `feat/*` branch
- Review scope: 当前分支已有旧提交时，review diff 只包含 `start_sha..HEAD` 的本次 ce-executor 改动

**Verification:**
- `cargo test -p ralph-cli test_public_presets_have_completion_path` 通过
- `cargo test -p ralph-cli test_public_presets_have_required_events` 通过

---

- [ ] U5. **同步修改 ce-executor-zh preset（中文版）**

**Goal:** 将 U4 的修改同步到中文版本

**Requirements:** R2, R7

**Dependencies:** U4

**Files:**
- Modify: `presets/ce-executor-zh.yml`

**Approach:**
- 镜像 U4 的所有修改到中文版对应位置
- 协调器：删除 "环境检查" 中的 branch 建议注释
- 协调器：在 `context.md` 中记录当前 branch 和 `start_sha = git rev-parse HEAD`
- 执行器：删除 "环境设置" 区块，保留 commit 逻辑
- Review Coordinator：优先使用 `start_sha..HEAD`，不可用时回退到现有 base 检测链

**Verification:**
- `cargo test -p ralph-cli test_preset_content_is_valid_yaml` 通过
- 中文与英文版本在 branch 创建逻辑上保持一致
- 中文与英文版本在 `start_sha` 记录和 diff base 逻辑上保持一致

---

- [ ] U6. **更新 zsh 补全脚本（如新增 public flag）**

**Goal:** 确保 `ralph run --worktree` 的补全可用

**Requirements:** R1

**Dependencies:** U1

**Files:**
- Modify: `scripts/ralph-zsh-plugin.zsh`

**Approach:**
- 检查 `scripts/ralph-zsh-plugin.zsh` 中 `ralph run` 的 flag 补全列表
- 添加 `--worktree` 到 `run` 子命令的 flag 列表中
- 按照 AGENTS.md 要求：`cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`

**Verification:**
- `scripts/ralph-zsh-plugin.zsh` 语法正确（zsh 可加载）
- `--worktree` 出现在补全列表中

---

- [ ] U7. **更新用户文档**

**Goal:** 让 CLI 文档和 preset 文档说明新的 worktree 模式、参数冲突和完成后处理方式

**Requirements:** R1, R3, R6, R7

**Dependencies:** U1-U5

**Files:**
- Modify: `docs/guide/cli-reference.md`
- Modify: `docs/guide/presets.md`

**Approach:**
- 在 `ralph run` flag 列表中添加 `--worktree`
- 说明 `--worktree` 与 `--exclusive` 冲突：
  - `--worktree`：立即创建 worktree 隔离运行
  - `--exclusive`：等待主工作区 lock 后原地运行
- 在 ce-executor 文档中说明：
  - 默认不再自动创建 feature branch
  - 推荐需要隔离时使用 `ralph run -H builtin:ce-executor --worktree ...`
  - worktree 完成后保留，由用户通过 `ralph loops attach/diff/discard/merge` 或 git 命令手动处理
  - review 默认基于本次运行记录的 `start_sha` 审查，不把当前分支旧提交纳入本次 diff

**Verification:**
- 文档中的命令与实际 clap flag 一致
- 文档不承诺自动 merge、自动 cleanup 或 completed registry 状态

---

## Success Criteria

1. `ralph run --worktree -p "test"` 在非 TUI/RPC 模式下创建 `ralph/<loop-id>` branch 和 `.worktrees/<loop-id>/`，并在 worktree 中运行 loop。
2. 默认 TUI 模式下，parent 不创建 worktree；child `ralph run --rpc --worktree ...` 创建 worktree 并在其中运行。
3. `ralph run --worktree --exclusive ...` 报参数冲突错误。
4. `ralph run -H builtin:ce-executor ...` 不传 `--worktree` 时不再自动创建 `feat/*` branch，直接在当前 checkout 上工作。
5. ce-executor 的 `context.md` 记录 `start_sha`；review 阶段优先使用 `start_sha..HEAD`，不会把当前分支的旧提交纳入本次 review diff。
6. 显式 worktree loop 完成后不进入 auto-merge queue，worktree 和 branch 保留；`ralph loops list` 能通过 worktree 扫描看到它，当前可显示为 `orphan`。
7. `cargo test` 全量通过；涉及 preset、CLI 参数、subprocess TUI forwarding 的 focused tests 通过。

---

## System-Wide Impact

- **Interaction graph:** non-TUI/RPC 的 `--worktree` 路径绕过主 lock 获取，与 `LoopLock` 无交互；默认 TUI parent 只转发 `--worktree`，child 走 non-TUI/RPC worktree 创建路径；`LoopRegistry` 在运行期间注册 worktree loop，退出后按现有逻辑 deregister
- **Error propagation:** worktree 创建失败（磁盘满、非 git 仓库、branch 已存在）通过 `anyhow::Context` 向上传播，与现有 parallel loop 错误处理一致
- **State lifecycle risks:** worktree 保留意味着磁盘占用累积；完成后不进入 merge queue，用户需通过 `ralph loops attach/diff/discard/merge` 或 `git worktree remove` 手动处理。当前 `ralph loops list` 可将其显示为 `orphan`
- **API surface parity:** `--worktree` 仅需添加到 `RunArgs`；`ResumeArgs` 无需添加（resume 场景下 loop 已在 worktree 中，通过 `--loop-id` 即可定位）
- **Review scope:** ce-executor 默认原地执行后，review 范围由 `start_sha` 锚定到本次运行，降低旧提交/旧改动混入 review 的风险
- **Unchanged invariants:**
  - 现有 parallel loop 自动 spawn 逻辑完全不变
  - `--exclusive` 语义不变，但与 `--worktree` 显式冲突
  - `--no-auto-merge` 语义不变（与 `--worktree` 叠加时效果相同）
  - `merge-loop` preset 不变
  - 其他 preset 默认行为不变

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| `spawn_worktree_loop` 提取后引入回归（parallel loop 自动 spawn 行为改变） | U2 的修改保持 `handle_active_lock` 调用语义不变，仅内部委托；现有 parallel loop 测试覆盖回归 |
| 默认 TUI 下 parent 和 child 工作目录不一致，导致 child 没有进入 worktree | U3 明确 parent 不创建 worktree，只转发 `--worktree`；child 作为实际 loop 进程创建 worktree并更新自己的 config workspace |
| ce-executor preset 删除 branch 创建指令后，用户依赖旧行为的脚本失效 | 这是一个 breaking change，但 scope 明确（仅 ce-executor）；用户可通过 `--worktree` 获得更好的隔离 |
| ce-executor 原地执行时 review 混入当前分支旧提交 | U4/U5 记录 `start_sha`，Review Coordinator 优先用 `start_sha..HEAD` |
| `start_sha` 记录失败或被用户手动改坏 | Review Coordinator fallback 到现有 base 检测链，并在 context/findings 中记录 fallback 原因 |
| worktree 中 `.agents/scratchpad/` 路径问题 | `specs_dir` 配置为 `.agents/scratchpad/`，worktree 有独立路径；Coordinator 创建 `.agents/scratchpad/ce-executor/{plan_name}/`。若主工作区已有 untracked scratchpad 内容被同步，当前接受为初始上下文，不作为共享运行时目录 |
| 完成后 `ralph loops list` 显示 `orphan` 容易被误解为错误 | U7 文档说明显式 worktree 完成后 `orphan` 表示保留待手动处理；不承诺 completed/manual registry 状态 |

---

## Documentation / Operational Notes

- U7 更新 `docs/guide/cli-reference.md` 中 `ralph run` 的 flag 列表，添加 `--worktree`
- U7 更新 `docs/guide/presets.md` 中 ce-executor 的使用说明，提及 `--worktree` 选项、默认不建分支、`start_sha` review 范围和完成后手动处理方式
- 无需更新 `docs/advanced/parallel-loops.md`（显式 worktree 与自动 parallel loop 是不同概念）

---

## Sources & References

- **Origin document:** [docs/brainstorms/ce-executor-worktree-mode-requirements.md](docs/brainstorms/ce-executor-worktree-mode-requirements.md)
- Worktree 管理: `crates/ralph-core/src/worktree.rs`
- Loop 上下文: `crates/ralph-core/src/loop_context.rs`
- CLI 入口: `crates/ralph-cli/src/main.rs`
- Loop runner: `crates/ralph-cli/src/loop_runner.rs`
- ce-executor preset: `presets/ce-executor.yml`
- 嵌入式 preset 测试: `crates/ralph-cli/src/presets.rs`
