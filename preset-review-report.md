# ce-executor-pipeline 聚焦审查报告

## Executive Summary

- 审查范围：仅 `plan-reviewer` 与 `executor` 两个 hat，以及直接决定 `--reuse-worktree` 语义的运行时代码。
- 未审查：其余 hats、完整拓扑、注入 skill 文档、mechanical lint。
- `agent_skill_audit: skipped`
- 结论：preset 已能恢复“已经提交且写入 checkpoint 的 Unit”，但没有充分处理“复用 worktree 时遗留的未提交半成品”。该缺口位于 `executor` 的入口，而不是 `plan-reviewer`。

## 已确认的运行时语义

`ralph run --worktree --reuse-worktree` 会复用原 worktree 的 Git 分支与文件状态。启动时只清理旧事件、scratchpad、tasks、diagnostics 等运行态文件；不会 reset、stash、提交或删除工作区中的未提交代码。`.ralph/agent/decisions.md` 也未列入清理集合，因此可继续作为恢复证据。

这意味着遗留修改会原样进入新一轮 `plan-reviewer → executor`。

## Findings

| 严重度 | confidence | finding | 证据 | 影响 |
|---|---:|---|---|---|
| P0 | 95 | `executor` 缺少复用 worktree 的入口脏树接管协议 | `Re-entry continuation` 只读取 `decisions.md` checkpoint/failure brief 并交叉检查已提交的 `git log`；没有在 dispatch 前读取 staged/unstaged/untracked diff，也没有把遗留 diff 归属到某个 U-ID。脏树检查直到最终 emit 前的 `Final Git Handoff Precheck` 才执行。 | 新 subagent、baseline verification 和 Unit 验收可能在上次半成品之上运行；修改来源混合后，executor 可能重复实现、错误归因、误提交，或一直到末尾才发现无法安全收口。 |
| P1 | 90 | `plan-reviewer` 没有声明如何看待复用 worktree 中的遗留代码修改 | 该 hat 只允许修改 plan，并在修改 plan 后提交；Git Flow Audit 仅通过 commits 判断 Unit 是否已落地，不检查未提交 implementation diff。 | 它通常会把该场景继续路由为 `first_run`。这不是由 plan-reviewer 修复半成品的职责，但应避免它把脏树误解为计划不可用或已完成，并在 handoff 中明确提示 executor 需要先接管残留。 |

## 建议契约

### plan-reviewer

保持只读代码、不清理残留。增加一个只读入口检查：

1. 执行 `git status --porcelain --untracked-files=all`，忽略 `.ralph/`。
2. 若存在遗留代码修改，不提交、不回滚、不 stash。
3. `flow_audit` 仍按 commits 判定；未提交修改不能算已完成 Unit。
4. 在 `review_summary` 中明确写入“检测到复用 worktree 的未提交残留，executor 必须先执行 entry reconciliation”，最好增加结构化 artifact/path 字段，而不是只依赖自由文本。

### executor

把现有 `Re-entry continuation` 扩展成 dispatch 前的强制 `Entry Reconciliation`：

1. 在 baseline-verifier 和任何 U subagent 之前，采集 staged、unstaged、untracked 状态及完整 diff。
2. 使用 plan 的 allowed files、测试、U-ID scope，加上 `decisions.md` 的旧 attempt/checkpoint，逐路径归属。
3. 对每份遗留修改只允许四种处置：
   - **可归属且可验证完成**：运行该 U 的验证；绿色后由主 executor 按该 U-ID 提交，并补写 checkpoint。
   - **可归属但未完成**：把现有 diff 作为该 U 的恢复起点，派发该 U 的 subagent 继续完成；不能从零重复实现。
   - **可归属但验证失败/不安全**：保留证据，计入该 U 已消耗的 attempt；按 retry budget 继续或 settle failed。
   - **无法归属或跨 Unit 混杂**：禁止自动提交、禁止静默回滚、禁止 stash 后忽略；写入 decisions artifact，并在无法安全拆分时 fail/partial settlement。
4. reconciliation 完成后重新建立 baseline snapshot；否则所谓 baseline 已被旧半成品污染。
5. 只有工作区已经被明确接管、且每个 dirty path 都有 U-ID 与 disposition，才允许 dispatch 新 Unit。

## 最终判断

当前 preset **有讲复跑恢复，但只讲清楚了已提交进度，没有讲清楚未提交半成品**。末尾的 clean-worktree gate 只能防止带脏树交接，不能防止执行过程建立在来源不明的旧修改之上。

正确方向不是自动 reset 或自动 stash。复用 worktree 的目的本来就是保留工作成果；应由 `executor` 在入口先做“识别、归属、验证、继续或阻塞”，而 `plan-reviewer` 只负责发现并传递这个事实。
