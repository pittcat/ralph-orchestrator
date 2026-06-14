---
title: ralph run --worktree 支持复用已有 worktree
type: feat
status: active
date: 2026-06-14
origin: docs/brainstorms/2026-06-14-worktree-reuse-requirements.md
---

# `ralph run --worktree` 支持复用已有 worktree

## Overview

为 `ralph run --worktree` 新增 `--reuse-worktree` 选项。启用时，Ralph 会先按当前 prompt/plan 生成的 loop name prefix 查找已完成的 worktree；命中则清理其运行时产物并复用该目录，未命中则回退到现有新建行为。复用保留分支代码状态，只清理 Ralph 运行时中间产物。

---

## Problem Frame

同一 prompt/plan 多次跑 `--worktree` 时，当前行为每次新建 `.worktrees/<loop-id>/`，导致目录膨胀、上次代码改动被丢弃、磁盘/git 开销冗余。用户需要一种可选复用机制，在保留代码状态的同时获得干净的运行时环境。

（详见 origin: `docs/brainstorms/2026-06-14-worktree-reuse-requirements.md`）

---

## Requirements Trace

- R1. 新增 CLI flag `--reuse-worktree`，仅与 `--worktree` 同时生效，与 `--exclusive` 互斥。
- R2. 启用时按 prompt/plan 生成的 loop name prefix 匹配 `loops.json` 中已完成、且 `worktree_path` 非空的 entry；多匹配取时间最近的一条。
- R3. 找不到匹配时自动回退新建 worktree，并提示原因。
- R4. `loops.json` 记录存在但目录被外部删除时视为无匹配，回退新建并警告。
- R5. 复用时不得重新创建 git 分支或重置分支状态。
- R6. 复用成功后清理 worktree-local 的 Ralph 运行时产物。
- R7. 清理不得删除/清空指向主仓库的 symlink 与 `.ralph/agent/context.md`。
- R8. 清理后确保必要目录存在。
- R9. 清理失败必须报错退出，不得进入 loop。
- R10. 命中/清理/回退时输出明确日志。
- R11. 不影响 `--no-auto-merge` 等现有行为。

---

## Scope Boundaries

- 不自动合入上一次 worktree 的改动；合并仍由现有 `--no-auto-merge` / merge queue 机制控制。
- 不复位分支到 base；有复位需求的用户自行使用 git reset/rebase。
- 不清理源码树中的跟踪文件或未跟踪文件；只清理 Ralph 运行时产物。
- 不支持复用运行中（未结束）的 worktree。
- 不改变默认 `--worktree` 行为；复用是显式 opt-in。

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-cli/src/commands/run.rs`：`RunArgs` 定义、`spawn_worktree_loop()`、`run_command()` 中 `--worktree` 与 `--worktree-path` 分支（约第 746–799 行）。
- `crates/ralph-core/src/worktree.rs`：`create_worktree()`、`remove_worktree()`、`list_worktrees()`、`worktree_exists()`；已有 `#[cfg(test)]` 使用 `tempfile::TempDir` + `git init` 测试 worktree 生命周期。
- `crates/ralph-core/src/loop_context.rs`：`LoopContext::worktree()`、各路径方法（`events_path`、`tasks_path`、`scratchpad_path`、`summary_path`、`handoff_path`、`history_path`、`diagnostics_dir`、`current_events_marker`、`context_path` 等）。
- `crates/ralph-core/src/loop_registry.rs`：`LoopEntry`、`LoopRegistry`、`is_alive()`（PID + worktree 目录双重检测）。
- `crates/ralph-core/src/loop_name.rs`：`LoopNameGenerator`、`generate_unique()`、`generate_unique_with_prefix()`、`sanitize_for_git()`，生成 `keywords-adjective-noun` 格式名称。
- `crates/ralph-cli/tests/integration_worktree_isolation.rs`：现有 worktree 集成测试风格（使用 `CARGO_BIN_EXE_ralph` + `tempfile::TempDir`）。

### Institutional Learnings

- `ralph-cli` 包内测试因 `loop_runner/tests.rs` 的 process-global Mutex 与时间敏感测试必须串行执行（`AGENTS.md` HARD RULE 1）。本计划将核心查找/清理逻辑放在 `ralph-core`，在 `ralph-core` 跑并行单元测试；`ralph-cli` 仅保留 flag 与流程接入的最小覆盖。
- `--worktree` 父进程已负责创建 worktree，并通过 `--worktree-path` 将路径传给子进程（U1-U3 修复，见 `crates/ralph-cli/tests/integration_worktree_isolation.rs`）。复用路径必须兼容该机制：父进程找到/清理 worktree 后，子进程仅使用 `--worktree-path` 进入，不再清理。

---

## Key Technical Decisions

- **匹配键：** 按当前 prompt/plan 生成的 loop name prefix 匹配。与 `spawn_worktree_loop` 中 `LoopNameGenerator::generate_unique()` / `generate_unique_with_prefix()` 的输出前缀一致，使用户自然重跑同一 prompt 时最容易命中。
- **匹配范围：** 只匹配 `loops.json` 中 `worktree_path` 非空、目录仍存在、且 PID 已不存活（即已完成）的 entry。避免复用仍在运行的 worktree。
- **找不到时回退新建：** 保持 `--worktree` 的即开即用体验，不阻塞用户。
- **清理范围：** 删除 worktree-local 运行时产物（事件、历史、诊断、任务、scratchpad、summary、handoff、current-events、current-loop-id、urgent-steer 等），保留 symlink 与 `context.md`。
- **清理失败即退出：** 避免在脏状态下启动 loop。
- **子进程不复用/不重复清理：** 父进程完成复用与清理后，子进程通过 `--worktree-path` 进入；子进程分支仅验证路径存在并构造 `LoopContext::worktree()`，与现有行为一致。

---

## Open Questions

### Resolved During Planning

- **哪些 `.ralph` 子文件属于运行时产物？** 基于 `LoopContext` 路径方法、event_logger、diagnostics 模块与 execution_contract 中的 marker 使用，确定清理清单包括：`.ralph/events.jsonl`、`.ralph/events-*.jsonl`、`.ralph/current-events`、`.ralph/history.jsonl`、`.ralph/history-*.jsonl`、`.ralph/diagnostics/`、`.ralph/urgent-steer.json`、`.ralph/current-loop-id`、`.ralph/agent/scratchpad.md`、`.ralph/agent/scratchpad-{loop_id}.md`、`.ralph/agent/tasks.jsonl`、`.ralph/agent/summary.md`、`.ralph/agent/handoff.md`。保留 `.ralph/agent/context.md`、symlink（`memories.md`、`specs/`、`tasks/`）以及 `.ralph/` 和 `.ralph/agent/` 目录本身。
- **与子进程 TUI 路径如何交互？** 复用逻辑只在父进程 `--worktree` 分支执行；父进程清理后将 `worktree_path` 通过 `--worktree-path` 传给子进程，子进程仅使用该路径，不再触发复用或清理。这与现有 `--worktree` → `--worktree-path` 的分工一致。

### Deferred to Implementation

- 最终清理清单中是否包含新增但本次未预见的文件；实现者应在改动 `LoopContext` 新增路径方法时同步更新清理函数。
- `--reuse-worktree` 与 `--continue` 同时使用的精确语义（当前建议：`--continue` 优先，复用仅在新 loop 启动时生效）。

---

## Implementation Units

- [ ] U1. **core: 复用 worktree 查找与匹配**

**Goal:** 提供根据 loop name prefix 查找可复用 worktree 的能力。

**Requirements:** R2, R4, R5

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/worktree.rs`
- Test: `crates/ralph-core/src/worktree.rs` (`#[cfg(test)]`)

**Approach:**
- 新增 `find_reusable_worktree(repo_root, prefix, config) -> Result<Option<Worktree>, WorktreeError>`。
- 读取 `loops.json`（通过 `LoopRegistry::list` 或本地读取），筛选满足以下条件的 entry：
  - `worktree_path` 为 `Some`
  - 对应目录存在
  - `is_alive()` 为 false（已完成）
  - entry 的 `id` 或分支名以生成的 prefix 开头（分支名格式为 `ralph/{loop_id}`）
- 多匹配时按 `started` 取最近一条。
- 复用现有 `list_worktrees()` 做交叉验证，确保 git 也承认该 worktree。

**Patterns to follow:**
- `worktree.rs` 中 `list_worktrees()` / `list_ralph_worktrees()` 的 git porcelain 解析。
- `loop_registry.rs` 中 `LoopEntry::is_alive()` 的完成态检测。

**Test scenarios:**
- Happy path: 给定 prefix `fix-header`，`loops.json` 有一条已完成 worktree entry `fix-header-swift-peacock`，函数返回该 worktree。
- Edge case: 多匹配时返回 `started` 最近的一条。
- Edge case: 运行中 entry（PID 存活）被排除。
- Error path: `loops.json` 记录存在但目录被外部删除，返回 `None`。
- Edge case: 主仓库 entry（`worktree_path: None`）被排除。

**Verification:**
- `cargo nextest run -p ralph-core -- worktree` 通过新增/现有测试。
- 查找函数在各种匹配/非匹配场景下返回正确 `Option`。

---

- [ ] U2. **core: worktree 运行时产物清理**

**Goal:** 在复用 worktree 后清理所有 Ralph 运行时产物，保留共享 symlink 与上下文文件。

**Requirements:** R6, R7, R8, R9

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/src/worktree.rs`
- Test: `crates/ralph-core/src/worktree.rs` (`#[cfg(test)]`)

**Approach:**
- 新增 `clean_worktree_runtime_artifacts(worktree_path) -> Result<(), WorktreeError>`。
- 基于 `LoopContext::worktree(loop_id, worktree_path, repo_root)` 提供的路径方法生成清理清单（不硬编码字符串，避免路径漂移）。
- 清理清单：
  - `.ralph/events.jsonl`
  - `.ralph/events-*.jsonl`
  - `.ralph/current-events`
  - `.ralph/history.jsonl`
  - `.ralph/history-*.jsonl`
  - `.ralph/diagnostics/`（整个目录树）
  - `.ralph/urgent-steer.json`
  - `.ralph/current-loop-id`
  - `.ralph/agent/scratchpad.md`
  - `.ralph/agent/scratchpad-*.md`
  - `.ralph/agent/tasks.jsonl`
  - `.ralph/agent/summary.md`
  - `.ralph/agent/handoff.md`
- 保留并不得删除：
  - `.ralph/agent/context.md`
  - symlink `.ralph/agent/memories.md`、`.ralph/specs/`、`.ralph/tasks/`
- 清理完成后调用 `LoopContext::ensure_directories()` 确保 `.ralph/`、`.ralph/agent/` 存在。
- 任何 IO 错误通过 `anyhow::Context` / `WorktreeError::Io` 向上传播，CLI 层据此退出。

**Patterns to follow:**
- `LoopContext` 路径解析方法。
- `worktree.rs` 中现有 `remove_worktree()` 对目录/文件的安全删除模式。

**Test scenarios:**
- Covers AE3. 复用完成后，`.ralph/events.jsonl`、`.ralph/agent/scratchpad.md`、`.ralph/agent/tasks.jsonl`、`.ralph/agent/summary.md`、`.ralph/agent/handoff.md`、`.ralph/diagnostics/` 不存在或为空；`.ralph/agent/context.md` 仍存在；symlink 仍指向主仓库。
- Edge case: 清理前某些文件本不存在（如 diagnostics 未启用），不报错。
- Edge case: `.ralph/agent/scratchpad-{loop_id}.md`（ephemeral isolation 产物）被删除。
- Error path: 模拟只读目录导致删除失败，函数返回错误。

**Verification:**
- 新增单元测试在临时 worktree 目录上验证清理与保留行为。
- `cargo nextest run -p ralph-core -- worktree` 通过。

---

- [ ] U3. **cli: `--reuse-worktree` flag 与复用路径接入**

**Goal:** 在 CLI 参数和 `run_command` 流程中接入复用逻辑。

**Requirements:** R1, R3, R10, R11

**Dependencies:** U1, U2

**Files:**
- Modify: `crates/ralph-cli/src/commands/run.rs`
- Modify: `crates/ralph-cli/src/commands/mod.rs`（若需导出测试 helper）
- Test: `crates/ralph-cli/tests/integration_worktree_isolation.rs`（新增场景）

**Approach:**
- 在 `RunArgs` 中新增：
  ```rust
  /// Reuse an existing completed worktree for this run instead of creating a new one.
  /// Only valid with --worktree.
  #[arg(long, requires = "worktree", conflicts_with = "exclusive")]
  pub reuse_worktree: bool,
  ```
- 在父进程的 `args.worktree` 分支中：
  1. 计算 `worktree_file_name_prefix`（复用现有逻辑）。
  2. 若 `args.reuse_worktree`：调用 `find_reusable_worktree()`。
  3. 命中时：
     - 输出 `info!("Reusing worktree at {}", path.display())`。
     - 调用 `clean_worktree_runtime_artifacts()`。
     - 输出 `info!("Cleaned runtime artifacts")`。
     - 构造 `LoopContext::worktree(loop_id, path, repo_root)`。
     - 设置 `pending_worktree_registration` 为新的 `LoopEntry::with_id(...)`（复用同一 worktree_path 但新 PID/started）。
  4. 未命中时：
     - 输出 `info!("No reusable worktree found, creating new worktree")`。
     - 走现有 `spawn_worktree_loop()` 新建。
- `--worktree-path` 子进程分支保持不变：仅验证路径存在并构造 context，不复用/不清理。
- `--reuse-worktree` 对 `--no-auto-merge` 无影响：auto_merge_override 仍按现有逻辑计算。

**Patterns to follow:**
- 现有 `spawn_worktree_loop()` 的结构与 `LoopEntry::with_id()` 注册方式。
- 现有 `--worktree` 与 `--worktree-path` 的分工（U1-U3 修复）。

**Test scenarios:**
- Happy path: Covers AE1. 第一次 `ralph run --worktree --reuse-worktree -p "fix header"` 新建 worktree 并在 worktree 内产生 `.ralph/events.jsonl`；第二次同一命令复用同一目录，目录数量不变，`.ralph/events.jsonl` 被清空/重建。
- Covers AE2. 在干净仓库执行 `ralph run --worktree --reuse-worktree`，应新建 worktree 并正常启动。
- Edge case: 运行中的 worktree 不被复用；第二次命令会新建另一个 worktree。
- Integration: `--reuse-worktree` 与 `--no-auto-merge` 同时使用不影响行为。

**Verification:**
- `cargo nextest run -p ralph-cli --test integration_worktree_isolation` 通过新增与现有场景。
- 手动验证两次相同 prompt 的 `--worktree --reuse-worktree` 运行后 `.worktrees/` 目录数不增加。

---

- [ ] U4. **cli: 帮助文本与使用文档更新**

**Goal:** 让用户知道 `--reuse-worktree` 的存在与行为边界。

**Requirements:** R10, R11

**Dependencies:** U3

**Files:**
- Modify: `crates/ralph-cli/src/commands/run.rs`（更新 `--reuse-worktree` 的 doc comment / help text）
- Modify: `docs/guide/parallel-loops.md`（若存在且提及 `--worktree`）
- Modify: `AGENTS.md`（仅当本段 builtin preset 列表或 CLI flag 列表被显式维护时才需同步；若无，则跳过）

**Approach:**
- 在 `RunArgs` 的字段 doc 中说明：
  - 必须与 `--worktree` 一起使用。
  - 找不到匹配时回退新建。
  - 保留分支代码状态，只清理运行时产物。
- 在相关用户文档（如 parallel-loops 指南）中增加 `--reuse-worktree` 示例。

**Test expectation:** none — 纯文档/帮助文本变更，行为由 U3 测试覆盖。

**Verification:**
- `ralph run --help` 中 `--reuse-worktree` 描述准确。
- 文档渲染无 broken link。

---

## System-Wide Impact

- **Interaction graph:** 复用路径影响 `ralph-cli/src/commands/run.rs` 的父进程 `--worktree` 分支、`ralph-core/src/worktree.rs` 的查找/清理函数、`LoopRegistry` 的读取。子进程 `--worktree-path` 分支、TUI 路径、RPC 路径均不受影响。
- **Error propagation:** `find_reusable_worktree` 与 `clean_worktree_runtime_artifacts` 的 IO/git 错误通过 `WorktreeError` / `anyhow::Error` 向上传播；清理失败时 `run_command` 直接返回错误，loop 不启动。
- **State lifecycle risks:** 复用会重写 `loops.json` 中同一 worktree 的 entry（新 PID/started），旧 entry 被替换。这是预期行为，因为同一 worktree 不能同时运行两个 loop。
- **API surface parity:** CLI flag 仅影响 `ralph run`；`ralph loops`、`ralph resume` 等命令暂不提供复用语义。
- **Integration coverage:** 父→子 `--worktree-path` 传递、复用时目录不新增、清理只删运行时产物，需通过 `ralph-cli` 集成测试覆盖。
- **Unchanged invariants:** 默认 `--worktree` 行为不变；未加 `--reuse-worktree` 时仍每次新建。`--exclusive` 与 `--worktree` 的互斥关系不变。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|-----------|
| 清理函数误删用户代码或共享 symlink | 基于 `LoopContext` 路径方法维护清单，单元测试验证保留行为；symlink 删除前检测 `is_symlink()`。 |
| 匹配逻辑误将运行中 worktree 判为可复用 | 使用 `LoopEntry::is_alive()` 双重检测（PID + 目录存在）。 |
| 子进程路径重复清理导致竞态 | 清理仅在父进程 `--worktree` 分支执行；子进程 `--worktree-path` 分支不触发清理。 |
| `loops.json` 与 git worktree list 不一致 | 交叉验证：entry 的 `worktree_path` 必须在 `git worktree list --porcelain` 输出中存在。 |
| 测试依赖真实 git worktree，Windows/非 Unix 环境不稳定 | `ralph-core` 单元测试使用 `tempfile` + `git init`；非 Unix 平台跳过需要 symlink 的断言（延续现有模式）。 |

---

## Documentation / Operational Notes

- 更新 `crates/ralph-cli/src/commands/run.rs` 中 `--reuse-worktree` 的 help text。
- 如 `docs/guide/parallel-loops.md` 存在，补充 `--reuse-worktree` 使用示例与边界说明。
- 无需新增预设或配置项；本功能完全由 CLI flag 控制。

---

## Sources & References

- **Origin document:** [docs/brainstorms/2026-06-14-worktree-reuse-requirements.md](../brainstorms/2026-06-14-worktree-reuse-requirements.md)
- 相关代码：`crates/ralph-cli/src/commands/run.rs`、`crates/ralph-core/src/worktree.rs`、`crates/ralph-core/src/loop_context.rs`、`crates/ralph-core/src/loop_registry.rs`、`crates/ralph-core/src/loop_name.rs`
- 相关测试：`crates/ralph-cli/tests/integration_worktree_isolation.rs`
