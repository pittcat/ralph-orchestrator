---
date: 2026-06-14
topic: worktree-reuse
---

# `ralph run --worktree` 支持复用已有 worktree

## Problem Frame

使用 `ralph run --worktree` 跑同一 prompt/plan 的多次迭代时，当前行为是每次都新建一个 `.worktrees/<loop-id>/` 目录。这会导致：

- worktree 目录不断膨胀，用户需要手动 `ralph loops discard` 清理；
- 上一次 run 留在 worktree 分支里的代码改动、中间状态被丢弃，无法自然延续；
- 重复跑相似任务时磁盘和 git 开销无意义。

因此需要一种可选的复用机制：当存在与当前 run 匹配的已完成 worktree 时，Ralph 清掉 worktree 内的运行时中间产物，然后在该目录里重新跑，而不是新建目录。

---

## Actors

- A1. **用户 / 开发者**：触发 `ralph run` 并期望复用或新建 worktree。
- A2. **Ralph CLI**：根据命令参数决定复用已有 worktree 还是新建，并执行清理。

---

## Key Flows

- F1. **复用已有 worktree**
  - **Trigger：** 用户执行 `ralph run --worktree --reuse-worktree -p "..."`（或带 plan 文件）。
  - **Actors：** A1, A2
  - **Steps：**
    1. CLI 按当前 prompt/plan 生成 loop name prefix（与现有 `--worktree` 生成 loop_id 的规则一致）。
    2. 查询 `loops.json` + `git worktree list`，找出 `worktree_path` 非空且状态为已完成（非运行中）的匹配 entry；多个匹配时取时间最近的一条。
    3. 验证该 worktree 目录仍存在。
    4. 清理 worktree 内的 Ralph 运行时产物（`.ralph/events.jsonl`、`.ralph/agent/scratchpad.md` 等）。
    5. 构造 `LoopContext::worktree` 指向该已有目录，启动 loop。
  - **Outcome：** 同一 worktree 被复用，运行时状态干净，代码/分支状态保留。
  - **Covered by：** R1, R2, R4, R5, R6, R8

- F2. **找不到匹配时回退新建**
  - **Trigger：** 无匹配的已完成 worktree，或记录存在但目录已被外部删除。
  - **Actors：** A2
  - **Steps：**
    1. 在日志中提示“未找到可复用 worktree，将新建”。
    2. 走现有 `--worktree` 的创建流程。
  - **Outcome：** 用户不会因复用失败而被阻塞。
  - **Covered by：** R3, R4

- F3. **清理运行时中间产物**
  - **Trigger：** 复用 worktree 成功后、loop 启动前。
  - **Actors：** A2
  - **Steps：**
    1. 删除/清空 worktree-local 的 `.ralph/events.jsonl`、`.ralph/current-events`、`.ralph/history.jsonl`、`.ralph/diagnostics/`、`.ralph/agent/scratchpad.md`、`.ralph/agent/tasks.jsonl`、`.ralph/agent/summary.md`、`.ralph/agent/handoff.md` 等。
    2. 保留 `.ralph/agent/context.md`、指向主仓库的 symlink（`memories.md`、`specs/`、`tasks/`）。
    3. 确保 `.ralph/agent/` 等必要目录仍存在。
  - **Outcome：** 新 loop 面对干净的运行时状态，但共享记忆、spec、code task 不变。
  - **Covered by：** R6, R7, R8

---

## Requirements

**匹配与复用**

- R1. 新增 CLI flag `--reuse-worktree`，仅与 `--worktree` 同时生效；与 `--exclusive` 互斥。
- R2. 启用 `--reuse-worktree` 时，Ralph 先按当前 prompt/plan 生成的 loop name prefix 匹配 `loops.json` 中已完成（非运行中）且 `worktree_path` 非空的 entry；多个匹配时取时间最近的一条。
- R3. 若找不到匹配，自动回退到现有 `--worktree` 行为新建 worktree，并在日志中说明回退原因。
- R4. 若 `loops.json` 记录存在但对应的 worktree 目录已被外部删除，视为无匹配，回退新建并给出警告。
- R5. 复用 worktree 时不得重新创建 git 分支或重置分支；保留原有的 commit 和 working tree 状态。

**清理中间产物**

- R6. 复用成功后、loop 启动前，必须清理 worktree 内的 Ralph 运行时产物，包括但不限于 `.ralph/events.jsonl`、`.ralph/current-events`、`.ralph/history.jsonl`、`.ralph/diagnostics/`、`.ralph/agent/scratchpad.md`、`.ralph/agent/tasks.jsonl`、`.ralph/agent/summary.md`、`.ralph/agent/handoff.md`。
- R7. 清理不得删除或清空指向主仓库的 symlink（`.ralph/agent/memories.md`、`.ralph/specs/`、`.ralph/tasks/`）以及 `.ralph/agent/context.md`。
- R8. 清理完成后需确保 `.ralph/`、`.ralph/agent/` 等必要目录存在，保证后续 loop 能正常写入。
- R9. 若清理失败，CLI 必须报错并退出，不得进入 loop。

**用户体验**

- R10. 复用命中时日志输出 `Reusing worktree at <path>`；清理后输出 `Cleaned runtime artifacts`；回退新建时输出 `No reusable worktree found, creating new worktree`。
- R11. `--reuse-worktree` 对 `--no-auto-merge` 等现有行为无影响。

---

## Acceptance Examples

- AE1. **Covers R1, R2, R5, R6.** 给定上一次 run 生成了 `.worktrees/loop-fix-header/` 且已完成，当用户执行 `ralph run --worktree --reuse-worktree -p "fix header"` 时，Ralph 复用该目录，清理 `.ralph/events.jsonl`、`.ralph/agent/scratchpad.md` 等运行时产物，保留 `ralph/loop-fix-header` 分支上的代码改动，并在该 worktree 内启动新 loop。
- AE2. **Covers R3.** 给定当前没有任何匹配的已完成 worktree，当用户执行 `ralph run --worktree --reuse-worktree -p "fix header"` 时，Ralph 按现有逻辑新建 `.worktrees/loop-fix-header-2/`（或类似唯一名）并正常启动。
- AE3. **Covers R6, R7.** 复用完成后，`.ralph/agent/scratchpad.md` 为空或不存在，`.ralph/events.jsonl` 不存在；但 `.ralph/agent/memories.md` 仍是指向主仓库的 symlink，内容未变；`.ralph/agent/context.md` 仍存在。

---

## Success Criteria

- 用户可以用 `--reuse-worktree` 多次跑同一 prompt/plan，而不会产生大量重复 worktree 目录。
- 被复用的 worktree 在每次新 run 开始时拥有干净的运行时状态。
- 默认保留上一次 run 的代码改动和分支状态，除非用户显式用 git 操作重置。
- 找不到可复用 worktree 时行为与现有 `--worktree` 完全一致，不会阻塞用户。
- 测试覆盖匹配逻辑、清理逻辑、回退新建逻辑。

---

## Scope Boundaries

- 不自动合入上一次 worktree 的改动；合并仍由现有 `--no-auto-merge` / merge queue 机制控制。
- 不复位分支到 base；有复位需求的用户应自行使用 git reset/rebase。
- 不清理源码树中的跟踪文件或未跟踪文件；只清理 Ralph 运行时产物。
- 不支持复用运行中（未结束）的 worktree，避免状态冲突。
- 不改变默认 `--worktree` 行为；复用是显式 opt-in。

---

## Key Decisions

- **复用键：** 按当前 prompt/plan 生成的 loop name prefix 匹配，而不是要求用户记忆 loop ID，使自然重跑最方便。
- **找不到时回退新建：** 避免用户因匹配失败而被卡住，保持 `--worktree` 的即开即用体验。
- **默认保留分支状态：** 复用的核心收益之一就是延续上次代码改动；清理仅针对 Ralph 运行时产物。
- **CLI flag 触发：** 比解析 prompt 关键词更可靠、可脚本化、行为可预测。

---

## Dependencies / Assumptions

- `loops.json` 可靠地记录了已完成 worktree loop 的 `worktree_path` 和最后更新时间。
- 现有 loop name 生成逻辑（`LoopNameGenerator`）和 worktree 创建逻辑可以被复用/暴露用于匹配。
- Worktree 目录被外部删除是小概率事件，通过回退新建处理，无需复杂恢复流程。

---

## Outstanding Questions

### Resolve Before Planning

无。

### Deferred to Planning

- [Needs research] 精确列出当前代码中哪些 `.ralph` 子文件/目录属于“运行时产物”且安全可清理；是否存在随功能演进新增的文件需要纳入清理清单。
- [Technical] `--reuse-worktree` 与 subprocess TUI / RPC 路径的交互是否需要额外处理（如父进程已创建 worktree 后如何向子进程传递复用路径）。

---

## Next Steps

`-> /ce-plan` 进行结构化实现规划。
