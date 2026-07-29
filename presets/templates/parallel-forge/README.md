# Parallel Forge artifact templates

Builtin preset `parallel-forge` 的 **artifact SSOT**（源自 `parallel-dev-preset.md`）：

| 文件 | MD 章节 | 生产者 | 产出路径 |
|---|---|---|---|
| `development-plan.template.md` | §10.14 | planner | `.ralph/forge/<plan-key>/development-plan.md` |
| `unit.template.yml` | §4 | planner（写入 execution-plan） | `execution-plan.yml` 内 `units[]` |
| `execution-plan.template.yml` | §7–§11 机器可读 | planner | `.ralph/forge/<plan-key>/execution-plan.yml` |
| `unit-completion.template.md` | §13.4 | executor | `.ralph/forge/<plan-key>/units/<unit-id>-completion.md` |
| `manager-report.template.md` | §21–§23 | reporter | `docs/reports/<YYYY-MM-DD>-<task>-manager-report.md` |
| `wave-settlement.template.md` | wave settlement | integrator | `.ralph/forge/<plan-key>/waves/<wave_id>/settlement.md` |
| `wave-failure.template.md` | `*.failed` 证据 | forge-failure-handler / integrator / verifier | `.ralph/forge/<plan-key>/waves/<wave_id>/failure-<round>.md` |
| `merge-conflict.template.md` | integrator 冲突 | integrator | `.ralph/forge/<plan-key>/waves/<wave_id>/merge-conflicts/<unit-id>.md` |
| `correction.template.md` | correction round | wave-fixer | `.ralph/forge/<plan-key>/waves/<wave_id>/corrections/round-<n>/report.md` |

**用法**：各 hat 在 activation 内 **先 materialize 内置模板，再复制到产出路径，按模板内章节/字段填满**；禁止跳过模板直接写自由格式文档。

**二进制安装**（无需仓库源码）：

```bash
ralph preset materialize-artifacts parallel-forge --plan-key <plan-key>
```

默认写出到：`.ralph/forge/<plan-key>/templates/`

开发仓库内源文件路径（编译时嵌入二进制）：`presets/templates/parallel-forge/`
