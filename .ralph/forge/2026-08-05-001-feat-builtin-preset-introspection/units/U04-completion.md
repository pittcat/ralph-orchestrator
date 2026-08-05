# Unit Completion Report — U04 Sync operator docs and completion for builtin introspection

## 元数据

| 字段 | 值 |
|---|---|
| unit_id | U04 |
| task_id | task-1785901327-62fa |
| task_key | forge:2026-08-05-001-feat-builtin-preset-introspection:U04 |
| worktree_path | /Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/primary-exec-w-4-0 |
| branch | ralph/primary-exec-w-4-0 |
| commit_sha | efd20804 |

## 修改摘要

同步 project-bootstrap operator skill、CLI reference、preset guide 与 zsh preset 子命令补全，使 builtin list/show 与 template list/show 数据源边界明确。

## 修改文件列表

| 路径 | 变更类型 | 说明 |
|---|---|---|
| skills/ralph-project-bootstrap/SKILL.md | 修改 | 使用 `ralph preset builtin list/show` 解析运行时 builtin。 |
| docs/guide/cli-reference.md | 修改 | 增加 builtin 子命令及示例。 |
| docs/guide/presets.md | 修改 | 说明 builtin 与 template 边界。 |
| scripts/ralph-zsh-plugin.zsh | 修改 | 增加 builtin/list/show 补全。 |

## RED 证据

| 字段 | 内容 |
|---|---|
| RED command | `cargo nextest run -p ralph-cli --test integration_preset_builtin -- help` |
| RED result | FAIL：当前分支未包含该集成测试目标。 |
| Expected failure | 预期应运行 builtin help 集成测试。 |
| Actual failure | cargo 报告 no test target named integration_preset_builtin。 |

## GREEN 证据

`zsh -n scripts/ralph-zsh-plugin.zsh` 通过；`scripts/check-cli-doc-drift.sh --strict` 通过。

## REFACTOR 说明

仅按 Unit allowed_paths 做最小文档与补全同步，未扩大范围。

## 回归测试结果

| 命令 | 结果 | 说明 |
|---|---|---|
| `zsh -n scripts/ralph-zsh-plugin.zsh` | PASS | zsh 语法有效。 |
| `scripts/check-cli-doc-drift.sh --strict` | PASS | 无新的 CLI 文档漂移。 |
| `cargo nextest run -p ralph-cli --test integration_preset_builtin -- help` | FAIL | 当前分支在测试目标发现前失败；Integrator 应在合并 U01-U03 后重跑。 |

## 验收条件映射

| acceptance_criteria（来自 execution-plan unit） | 结果 | 证据 |
|---|---|---|
| CLI/operator 文档描述 builtin list/show | PASS | cli-reference、presets、SKILL 已更新。 |
| zsh 补全语法通过 | PASS | `zsh -n`。 |
| source/install 补全同步 | PASS | 已复制到本地 oh-my-zsh 插件并通过 `zsh -n`。 |
| 全量回归 | NOT RUN | 本 Unit 未运行全量门禁。 |

## 当前 Commit

- Commit SHA：efd20804
- Commit Message：feat(unit-u04): sync operator docs and completion for builtin introspection
- final_commit_count：1（U04 变更提交；上游 U01-U03 为依赖提交）

## 已知风险

集成分支合并 U01-U03 后需重跑 builtin help 集成测试与最终全量门禁。

## 共享接口变更

- 是否修改共享接口：否

## 后续 Unit 影响

- 是否需要调整后续 Unit：否
- 说明：文档已与 U01-U03 的 builtin CLI 表面同步；全量验证由 Integrator 执行。
