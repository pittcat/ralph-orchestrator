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

---

## 7. Timeout 语义（worktree / RPC / autonomous 路径）

> **新增（Unit 4 of plan 2026-06-06-001）**：worktree / RPC 路径必须区分
> interactive 与 autonomous 的 timeout 行为，否则会在长任务期间被误杀或
> 静默卡死。本节把现有约定写清楚，避免用户继续把 “interactive timeout”
> 误解成 “所有 PTY 路径的 timeout”。

### 7.1 两条 timeout 链路，互不复用

| 路径 | 字段 | 默认值 | 0 的含义 | 谁在用 |
|------|------|--------|---------|--------|
| **interactive / 手动 PTY 会话** | `cli.idle_timeout_secs` | `30` 秒 | 禁用 interactive idle timeout | `ralph run` 的 TTY 交互模式 |
| **autonomous / RPC / worktree** | `cli.autonomous_idle_timeout_secs` | `None` → 回退到 `adapters.<backend>.timeout`（通常 `300` 秒） | 显式禁用 autonomous watchdog | `ralph run --no-tui`、`--rpc`、`--worktree`、TUI observation 模式下的后端调用 |

- 这两条链路是 **独立字段、独立 resolver、独立 executor 分支**，不能
  把 interactive 的 30 秒默认值直接套到 autonomous / RPC / worktree 上。
  30 秒的默认值在 autonomous 路径上会误杀任何 >30 秒的长任务（长
  工具调用、模型 thinking、网络阻塞操作）。
- 详见 `crates/ralph-core/src/config/cli.rs::CliConfig.autonomous_idle_timeout_secs`
  文档注释，以及 plan `2026-06-06-001-fix-autonomous-pty-timeout-plan.md` 的
  R6 / R8 约束。

### 7.2 `--autonomous-idle-timeout` CLI flag

`ralph run` 提供 `--autonomous-idle-timeout <SECONDS>`，作用范围与
`autonomous_idle_timeout_secs` 字段相同，但只对本次调用生效：

```bash
# 显式覆盖：1500 秒
ralph run -H builtin:ce-executor --worktree --autonomous-idle-timeout 1500 \
  -p "docs/plans/my-plan.md"

# 显式禁用：0（仅在用户明确知道要等多久时使用；循环会无限等待）
ralph run -H builtin:ce-executor --worktree --autonomous-idle-timeout 0 \
  -p "docs/plans/my-plan.md"
```

CLI 优先级高于 YAML / TOML 配置文件。配置解析与默认值详见
`crates/ralph-cli/src/commands/run.rs::RunArgs.autonomous_idle_timeout` 和
`crates/ralph-core/src/config/ralph_config.rs::RalphConfig::autonomous_idle_timeout_secs(backend)`。

### 7.3 超时之后会发生什么

`ce-executor` 在 autonomous / RPC / worktree 路径上，**超时不是 loop
终止**：

1. PTY watchdog 触发后，runner 用 SIGTERM 终止当前 backend 子进程，保留
   已经收集到的输出和事件文件。
2. `convert_termination_type(IdleTimeout, autonomous)` 返回 `None`——
   **不会** 误把整个 loop 标记为 `Stopped`。runner 继续走
   `process_output` / `process_events_from_jsonl` 把已有事件交给后续
   hat。
3. 如果事件流中已经有 `work.done` / `work.failed`，orchestration 继续走
   现有 review/fix 链路。
4. 如果事件流为空，进入 missing-event hard gate / fallback，由
   `RalphConfig` 的 hard-gate 决定下一步（通常打回 executor 重试，或
   在 `max_iterations` 用尽后明确失败）。
5. 失败原因（`watchdog_timeout=true`）在 runner 的 `warn!` 日志中
   显式记录，便于诊断。

详细代码路径：`crates/ralph-cli/src/loop_runner/runner.rs::if outcome.watchdog_timeout`、
`crates/ralph-cli/src/loop_runner/execution.rs::ExecutionOutcome::watchdog_timeout` 字段文档。

### 7.4 与 ce-executor preset 的关系

- `presets/en/ce-executor.yml` 和 `presets/zh/ce-executor-zh.yml` 不应
  包含 hard-coded `cli.autonomous_idle_timeout_secs`——它属于执行层
  配置，由用户或 operator 在调用时显式控制。如果 preset 强制覆盖，
  会绕过 plan 层的可调性。
- 如果某个 ce-executor 子场景（例如超长 `pytest`、批量模型推理）
  真的需要 30 分钟以上的 watchdog，正确做法是用户在 `ralph.yml` 中
  设置 `cli.autonomous_idle_timeout_secs: 1800` 或在 CLI 上传
  `--autonomous-idle-timeout 1800`，**不要** 修改 preset。

### 7.5 不变量

- R2：interactive 模式现有行为保持不变——`cli.idle_timeout_secs` 默认
  30 秒，0 禁用，单元测试覆盖。
- R6：autonomous 路径 **不** 复用 interactive 默认 30 秒；用 300 秒
  默认（或 per-adapter timeout），避免误杀。
- R8：`0` 的语义在两条链路上都是“显式禁用”，不是“仍然启用默认 watchdog”。
  CLI help / 配置文档 / 测试三者口径必须一致。
- R7：超时前已产生的有效事件必须继续可见、被处理；不能因为
  backend idle timeout 丢失 partial events。
