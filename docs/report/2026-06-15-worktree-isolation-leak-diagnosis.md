# Worktree 隔离泄漏诊断报告

**日期**: 2026-06-15
**问题**: `ralph -H builtin:ce-executor-isolated run --worktree --reuse-worktree` 执行时，agent 在主分支 `pittcat-dev` 中修改了代码
**Loop ID**: `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-lucky-peacock`

---

## 结论摘要

**确认问题存在**：agent 在执行过程中同时修改了 worktree 和主仓库的 `crates/ralph-core/src/event_loop/mod.rs`。Worktree 的修改被正确提交到 worktree 分支（`e070d5d`），但主仓库 `pittcat-dev` 分支上也留下了相同的未暂存修改。

**根因**：Agent 上下文（`context.md`）同时暴露了 workspace 路径和 main repo 路径，agent 在文件操作中混淆了两个路径，导致修改泄漏到主仓库。

**性质**：Agent 行为问题（非 orchestrator 基础设施缺陷）。PtyExecutor 正确设置了 `cwd` 为 worktree 路径，事件也正确写入 worktree 的 events 文件。但 agent 在工具调用中使用了主仓库的绝对路径。

---

## 1. 流程还原

### 1.1 预期执行链路

```
ralph run --worktree
  → 创建/复用 git worktree 于 .worktrees/<loop-id>/
  → 切换到 worktree 分支 ralph/<loop-id>
  → PtyExecutor 以 worktree 路径为 cwd 启动 claude 后端
  → Agent 在 worktree 中修改代码、提交
  → 事件写入 worktree/.ralph/events-*.jsonl
  → 主仓库不受影响
```

### 1.2 实际执行链路

```
12:35:00  Loop 启动，worktree 路径正确
12:38:47  coordinator 发出 build.done（被 isolated scope 丢弃）
12:39:04  coordinator 再次发出 build.done（被丢弃）
12:39:07  worktree mod.rs 被修改（mtime）
12:39:35  主仓库 mod.rs 被修改（mtime，比 worktree 晚 28 秒）⚠️
12:41:10  coordinator 发出 work.ready（step-01, task-u1-scaffold-placeholder-modules）
12:45:06  executor 发出 work.done（commit_count=1, changed_lines=77）
12:45:13  execution contract 拒绝 work.done（TaskNotFound）
```

### 1.3 关键时间线对比

| 时间 (UTC) | 事件 | 位置 |
|---|---|---|
| 12:35:00 | Loop 启动 | Worktree |
| 12:39:07 | `mod.rs` 修改 | **Worktree** |
| 12:39:35 | `mod.rs` 修改 | **主仓库** ⚠️ |
| 12:41:10 | `work.ready` 发出 | Worktree events |
| 12:45:06 | `work.done` 发出 | Worktree events |

**主仓库的 `mod.rs` 修改时间（12:39:35）比 worktree（12:39:07）晚 28 秒**，说明 agent 先修改了 worktree 的文件，随后又修改了主仓库的同名文件。

---

## 2. 对账分析

### 2.1 文件修改对账

**Worktree 分支** (`ralph/2026-06-10-003-...`)：
```
git log --oneline:
e070d5d refactor(event-loop): U1 scaffold - add 10 placeholder submodule files
7af7d68 fix(preset): review-synthesizer aggregate timeout 升至 1800s

git status:
A  crates/ralph-core/src/event_loop/diagnostics.rs
A  crates/ralph-core/src/event_loop/dispatch.rs
A  crates/ralph-core/src/event_loop/lifecycle.rs
M  crates/ralph-core/src/event_loop/mod.rs
A  crates/ralph-core/src/event_loop/policy.rs
A  crates/ralph-core/src/event_loop/process.rs
A  crates/ralph-core/src/event_loop/prompt.rs
A  crates/ralph-core/src/event_loop/termination.rs
A  crates/ralph-core/src/event_loop/types.rs
A  crates/ralph-core/src/event_loop/wave.rs
A  crates/ralph-core/src/event_loop/workflow_guard.rs
```
→ 10 个新文件 + 1 个修改，已暂存，已提交。**符合预期**。

**主仓库** (`pittcat-dev` 分支)：
```
git status:
M crates/ralph-core/src/event_loop/mod.rs
```
→ 1 个未暂存修改。**不符合预期**。

**Diff 内容对比**：两处的 `mod.rs` diff **完全一致**（25 行新增，均为 U1 scaffold 的 `mod` 声明和 `pub use` 重导出）。

### 2.2 事件对账

**Worktree events** (`events-20260615-123500.jsonl`)：
```jsonl
{"hat":"coordinator","topic":"build.done","ts":"12:38:47"}
{"hat":"coordinator","topic":"build.done","ts":"12:39:04"}
{"hat":"coordinator","topic":"work.ready","ts":"12:41:10"}
{"hat":"executor","topic":"work.done","ts":"12:45:06","triggered":"ralph"}
```

**主仓库 events** (`events-history-20260615-025112.jsonl`)：
```jsonl
{"hat":"loop","topic":"work.start","ts":"02:51:12"}
```
→ 仅有一条历史记录，来自更早的 loop。当前 loop 的事件**未泄漏到主仓库**。

### 2.3 诊断日志对账

```
workspace_root="/home/.../ralph-orchestrator/.worktrees/2026-06-10-003-...-lucky-peacock"
```
→ PtyExecutor 的 `cwd` 正确设置为 worktree 路径。

```
Isolated mode: event out of hat scope — dropping hat=coordinator topic=build.done
```
→ `build.done` 不在 coordinator 的 publishes 范围内，被正确丢弃。这是预期行为。

```
Execution contract rejected event topic=work.done violation=TaskNotFound
```
→ `work.done` 中的 `task_id` 在 task store 中找不到，被 execution contract 拒绝。这是预期行为（task 未通过 `ralph tools task ensure` 创建）。

### 2.4 符号链接对账

Worktree 的 `.ralph/` 下存在指向主仓库的符号链接：
```
.ralph/specs -> /home/.../ralph-orchestrator/.ralph/specs
.ralph/tasks -> /home/.../ralph-orchestrator/.ralph/tasks
.ralph/agent/memories.md -> /home/.../ralph-orchestrator/.ralph/agent/memories.md
```

这些符号链接是**设计如此**（见 `context.md` 和 `AGENTS.md` 的 Parallel Loops 文档），用于共享 specs、tasks、memories。它们**不是**泄漏原因——它们只影响 `.ralph/` 下的元数据，不影响源码树。

---

## 3. 问题归因

### 3.1 根因：Agent 上下文暴露主仓库路径

`context.md` 内容：
```markdown
- **Workspace**: /home/.../ralph-orchestrator/.worktrees/2026-06-10-003-...-lucky-peacock
- **Main Repo**: /home/chaowen/Dev/agent_tools/ralph-orchestrator
```

Agent 同时知道两个路径。在文件操作中，agent 可能：
1. 使用主仓库路径（`/home/chaowen/Dev/agent_tools/ralph-orchestrator/crates/...`）而非 workspace 路径
2. 在两个位置都进行了修改

**证据**：主仓库 `mod.rs` 的 mtime 比 worktree 晚 28 秒，且 diff 内容完全一致。

### 3.2 归因表

| 因素 | 类别 | 是否根因 | 说明 |
|---|---|---|---|
| Agent 混淆 workspace/main-repo 路径 | Agent 行为 | **是** | context.md 暴露了两个路径，agent 在两个位置都修改了文件 |
| `context.md` 暴露主仓库路径 | Preset/配置 | 促成 | 主仓库路径对 agent 不必要，但当前设计如此 |
| 符号链接（specs/tasks/memories） | 基础设施 | 否 | 设计如此，只影响 .ralph/ 元数据 |
| PtyExecutor cwd 设置 | 基础设施 | 否 | 正确设置为 worktree 路径 |
| `build.done` 被丢弃 | Preset 配置 | 否 | isolated scope 正确行为 |
| `work.done` 被 execution contract 拒绝 | Preset 配置 | 否 | task 未 ensure，正确拒绝 |

### 3.3 非根因排除

- **PtyExecutor 工作目录**：日志确认 `workspace_root` 正确指向 worktree，`cmd_builder.cwd()` 设置正确
- **事件泄漏**：主仓库的 events 文件没有收到当前 loop 的事件
- **符号链接**：只涉及 `.ralph/` 下的元数据文件，不涉及源码树
- **Git worktree 机制**：worktree 本身工作正常，修改被正确提交到 worktree 分支

---

## 4. 修复建议

### 4.1 短期（推荐立即实施）

**方案 A：从 context.md 中移除 Main Repo 路径**

修改生成 `context.md` 的代码，不再向 agent 暴露主仓库路径。Agent 只需要知道 workspace 路径。

位置：`crates/ralph-core/src/worktree.rs` 或生成 context.md 的相关代码。

**方案 B：在 context.md 中增加强警告**

在 `context.md` 中明确警告 agent 不要使用 Main Repo 路径进行文件操作：
```markdown
## CRITICAL
All file modifications MUST use the Workspace path only.
The Main Repo path is for reference only. NEVER write files to the Main Repo.
```

### 4.2 中期（建议规划）

**方案 C：Agent prompt 中注入路径隔离指令**

在 executor hat 的 prompt 中注入明确的工作目录约束，要求所有文件操作使用 `$RALPH_WORKSPACE_ROOT` 环境变量或相对路径。

### 4.3 长期（架构改进）

**方案 D：文件系统级隔离**

在 agent 子进程层面，通过容器/沙箱/cgroup 等方式限制 agent 只能写入 worktree 路径，无法访问主仓库路径。这是最彻底的解决方案，但实现成本最高。

---

## 5. 证据清单

| 编号 | 证据 | 路径 |
|---|---|---|
| E1 | Worktree git status（10 新文件 + 1 修改，已暂存） | `.worktrees/.../` git status |
| E2 | 主仓库 git status（1 未暂存修改） | 主仓库 git status |
| E3 | 两处 diff 内容一致 | `git diff` 对比 |
| E4 | Worktree mod.rs mtime: 12:39:07 UTC | `stat` 输出 |
| E5 | 主仓库 mod.rs mtime: 12:39:35 UTC（晚 28s） | `stat` 输出 |
| E6 | Worktree events 正确记录 | `events-20260615-123500.jsonl` |
| E7 | 主仓库 events 无泄漏 | `events-history-20260615-025112.jsonl` |
| E8 | 诊断日志确认 workspace_root 正确 | `ralph-2026-06-15T20-35-00-530-342101.log` |
| E9 | context.md 暴露两个路径 | `context.md` |
| E10 | PtyExecutor cwd 设置代码 | `pty_executor.rs:326` |
| E11 | Worktree 提交记录 | `git log e070d5d` |
| E12 | 符号链接列表 | `ls -la .ralph/` |
