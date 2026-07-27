# Unit Completion Report — <UNIT_ID> <UNIT_TITLE>

> **模板来源**：parallel-dev-preset.md §13.4。Executor 复制到
> `.ralph/forge/<plan-key>/units/<unit-id>-completion.md` 并填满后 emit `exec.unit.done`。

## 元数据

| 字段 | 值 |
|---|---|
| unit_id | |
| task_id | （live，来自 ralph tools task list） |
| task_key | |
| worktree_path | |
| branch | |
| commit_sha | |

## 修改摘要

<!-- 本 Unit 做了什么 -->

## 修改文件列表

| 路径 | 变更类型 | 说明 |
|---|---|---|

## RED 证据

| 字段 | 内容 |
|---|---|
| RED command | |
| RED result | |
| Expected failure | |
| Actual failure | |

## GREEN 证据

<!-- 最小实现后哪些测试通过 -->

## REFACTOR 说明

<!-- 在测试保护下做了哪些整理；是否扩大 Unit 范围：否 -->

## 回归测试结果

| 命令 | 结果 | 说明 |
|---|---|---|

## 验收条件映射

| acceptance_criteria（来自 execution-plan unit） | 结果 | 证据 |
|---|---|---|

## 当前 Commit

- Commit SHA：
- Commit Message：
- final_commit_count：必须为 1（交付 Integrator 前）

## 已知风险

## 共享接口变更

- 是否修改共享接口：是 / 否
- 若「是」，列出接口与影响：

## 后续 Unit 影响

- 是否需要调整后续 Unit：是 / 否
- 说明：
