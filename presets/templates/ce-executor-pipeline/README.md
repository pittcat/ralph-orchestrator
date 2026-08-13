# ce-executor-pipeline artifact templates

Builtin preset `ce-executor-pipeline` 的 **fail 门禁证据模板 SSOT**（源自 plan `2026-07-29-001-feat-ce-pipeline-fail-gate-and-reuse-plan`）：

| 文件 | 内容 | 生产者 | 产出路径 |
|---|---|---|---|
| `fail-confidence-rubric.template.md` | 阈值化 fail 自评/他评 rubric（§1 四维度评分 / §2 阈值表 / §3 coverage 规则 / §4 六类 failed_checks 命名） | executor / fixer / precheck gate 三方共用 | （仅参考，不直接落盘为业务 artifact） |
| `review-findings-contract.md` | 六个维度 reviewer 共用的 finding 字段、dimension 一致性与 emit 前自检契约 | 六个 `dim:*` reviewer / review-synthesizer | （仅参考，不直接落盘为业务 artifact） |
| `settlement-evidence.template.md` | 证据文件格式（每 failed/blocked Unit 一节 + 自评 + coverage 缺口说明） | executor / fixer | executor → `.ralph/review/<plan>/dead-end-evidence.md`；fixer → `.ralph/review/<plan>/fix-settlement-evidence.md` |

**用法**：需要填写上述模板的 hat 在 activation 内 **先 materialize 内置模板，再按模板章节填写**；禁止跳过模板直接写自由格式证据文件。

**二进制安装**（无需仓库源码）：

```bash
ralph preset materialize-artifacts ce-executor-pipeline --plan-key <plan-key>
```

默认写出到：`.ralph/forge/<plan-key>/templates/`

开发仓库内源文件路径（编译时嵌入二进制）：`presets/templates/ce-executor-pipeline/`
