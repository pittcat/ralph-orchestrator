# ce-executor Worktree 隔离模式需求文档

## 1. 背景与动机

当前 `ce-executor` preset 的 Executor hat 会在当前工作目录创建 feature branch（如 `feat/plan-name`），所有代码改动直接发生在主工作区。这带来几个问题：

- **污染主工作区**：即使计划简单，也会创建一个可能长期存在的 branch
- **无法并行**：两个 ce-executor 任务不能同时运行，因为共享同一个 working directory
- **缺乏隔离**：master/main 分支上的未提交改动可能与 ce-executor 的改动冲突

Ralph 已经具备成熟的 worktree 基础设施（`crates/ralph-core/src/worktree.rs`、parallel loops、`merge-loop` preset），但当前仅在锁竞争时自动 spawn worktree。用户希望 ce-executor 能够**显式、默认地**利用 worktree 进行隔离执行。

---

## 2. 核心需求

### 2.1 显式 Worktree 触发

| ID | 需求描述 | 优先级 |
|---|---|---|
| WM-01 | `ralph run -H builtin:ce-executor --worktree` 时，自动创建 git worktree 并在其中执行全部工作 | P0 |
| WM-02 | 不传 `--worktree` 时，ce-executor 不再自动创建 feature branch，直接在当前 checkout 的分支上工作 | P0 |
| WM-03 | Worktree 命名规则：`ce-executor/<plan-name>` branch + `.worktrees/ce-executor-<plan-name>-<timestamp>/` 目录 | P1 |

### 2.2 Worktree 环境初始化

| ID | 需求描述 | 优先级 |
|---|---|---|
| WM-04 | Worktree 创建后，自动设置 `.ralph/` 目录结构（隔离的 events、tasks） | P0 |
| WM-05 | Memories（`.ralph/agent/memories.md`）通过 symlink 共享主仓库，保证跨 worktree 学习积累 | P0 |
| WM-06 | `.agents/scratchpad/` 在 worktree 中独立存在（ce-executor 的运行时工作目录） | P1 |
| WM-07 | 未追踪文件（untracked）和未暂存修改（unstaged）按需同步到 worktree（复用 `sync_working_directory_to_worktree`） | P1 |

### 2.3 执行与完成行为

| ID | 需求描述 | 优先级 |
|---|---|---|
| WM-08 | 所有代码实现、测试、commit 均在 worktree 中进行，主工作区完全不受影响 | P0 |
| WM-09 | LOOP_COMPLETE 后，worktree 和 branch **保留**，不触发 auto-merge 或自动清理 | P0 |
| WM-10 | 用户可通过 `ralph loops` 命令族查看、attach、diff、手动 merge 或 discard 该 worktree | P1 |

### 2.4 Preset 行为调整

| ID | 需求描述 | 优先级 |
|---|---|---|
| WM-11 | Coordinator hat：移除"不主动创建分支（Executor 负责）"的注释，环境检查仅记录当前 branch 状态 | P0 |
| WM-12 | Executor hat：移除"如果不在 feature branch 上，创建一个"的指令 | P0 |
| WM-13 | Shipper hat：commit 行为保持不变（仍在 worktree 中执行），不 push | P0 |

---

## 3. 当前状态分析

### 3.1 已有能力

| 组件 | 位置 | 能力 |
|---|---|---|
| Worktree 管理 | `crates/ralph-core/src/worktree.rs` | create/remove/list/sync，已成熟 |
| Parallel loops | `crates/ralph-cli/src/loop_runner.rs` | 锁竞争时自动 spawn worktree |
| Loop registry | `crates/ralph-core/src/loop_registry.rs` | 追踪所有 loop 状态 |
| Merge-loop preset | `crates/ralph-cli/presets/merge-loop.yml` | worktree 合并回主分支 |
| Loops CLI | `crates/ralph-cli/src/loops.rs` | list/attach/diff/merge/discard 等 |

### 3.2 需要修改的组件

| 组件 | 修改内容 |
|---|---|
| `ralph run` CLI | 添加 `--worktree` flag，传递到 loop runner |
| `loop_runner.rs` | 支持显式 worktree 模式（不依赖锁竞争触发） |
| `ce-executor.yml` | 移除 branch 创建逻辑，适配 worktree 环境 |
| `ce-executor-zh.yml` | 同步中文版本修改 |

---

## 4. 方案概述

### 推荐方案：复用 Parallel Loop 基础设施，添加显式触发

1. **CLI 层**：`ralph run` 添加 `--worktree` flag
2. **Loop Runner 层**：若 `--worktree=true`，在启动事件循环前调用 `worktree::create_worktree()`，然后切换到 worktree 目录执行
3. **Worktree 初始化**：创建 `.ralph/`（隔离）、symlink `memories`（共享）
4. **Preset 层**：移除 branch 创建指令，改为依赖 worktree 提供的隔离环境

**优势**：
- 复用现有成熟基础设施，改动面小
- 与 parallel loops 语义一致，`ralph loops` 命令族无需修改即可管理
- 完成后 worktree 保留，用户完全掌控 merge 时机

---

## 5. 非目标

- **不改动现有 parallel loop 的自动 spawn 逻辑**（锁竞争时仍自动进 worktree）
- **不提供 auto-merge**（用户手动处理）
- **不修改其他 preset**（仅 ce-executor 受益）
- **不修改 merge-loop preset**（保留给 parallel loops 使用）

---

## 6. 成功标准

1. `ralph run -H builtin:ce-executor --worktree -p "docs/plans/my-plan.md"` 成功创建 worktree 并执行
2. 主工作区 `git status` 显示干净（worktree 完全隔离）
3. 不传 `--worktree` 时，ce-executor 不再创建 feature branch
4. 完成后 `ralph loops list` 能看到该 worktree，且 branch 存在
5. `cargo test` 全量通过
