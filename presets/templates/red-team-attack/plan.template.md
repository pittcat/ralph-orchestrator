# Red Team PLAN Template
#
# Copy to `.ralph/red-team/PLAN.md` and fill every section.
# This is the repair plan deliverable. It is NOT authorization to modify code.
#
# Artifact lifecycle:
#   owner: repair-planner (writes this file)
#   readers: independent-reviewer (audits), operator (confirms), coding-agent (executes after confirmation)
#   retention: operator-owned; do not delete until plan is executed or abandoned

# Red Team Repair Plan

## 元信息
- 生成时间: <ISO8601>
- 目标分支: <branch>
- 锁定 HEAD: <head_sha>
- 计划来源: Red Team Attack Preset
- 执行授权: false（必须人工确认）

## 输入 Finding 汇总
| Finding | Severity | Confidence | Evidence | Verifiability | Impact |
|---|---|---|---|---|---|
| RTF-001 | P1 | 92 | 88 | 91 | 87 |

## 修复 Unit 列表
# 严格串行。前一个 Unit 的 Red → Green → Refactor → Regression 全部完成
# 后，才能开始下一个 Unit。

### Unit 0: 锁定修复前基线
- 目标: 记录修复前全量测试状态、静态检查状态、关键行为快照
- 命令: <verification_command>
- 预期: 全部通过（或记录已知失败作为基线）
- 产物: `.ralph/red-team/baseline/`

### Unit 1: RTF-001 — <title>
- source:
  finding_id: RTF-001
  experiment_id: RTE-001
  plan_ids: []
  commits: []
  patch_hunks: []
- metrics:
  confidence: 92
  evidence_coverage: 88
  verifiability: 91
  impact_certainty: 87
- broken_invariant: "<invariant>"
- minimal_fix_locus: "<file:line>"
- allowed_files: []
- forbidden_scope: []
- existing_behavior_to_preserve: []
- red_test_source: "<how to convert reproducer to formal failing test>"
- regression_matrix:
  - component: "<module>"
    command: "<test command>"
    expected: pass
  - integration: "<module>"
    command: "<test command>"
    expected: pass
  - cross_plan: "<module>"
    command: "<test command>"
    expected: pass
- rollback_condition: "<when to revert>"

#### 执行顺序
1. 读取原始实验 `.ralph/red-team/experiments/RTE-001.md`
2. 将临时 reproducer 转为正式失败测试（Red）
3. 确认测试失败（Red）
4. 最小生产修复（Green）
5. 确认测试通过（Green）
6. 局部重构（如有必要）
7. 组件回归
8. 集成回归
9. 跨计划回归
10. 全量回归

### Unit 2: RTF-002 — <title>
...（同上结构）

## Final Regression
- 全量测试: <command>
- 静态检查: <command>
- 干净环境重建: <command>

## Clean Environment
- 新 clone / container 中执行 Full Regression
- 预期: 全部通过

## Independent Review
- 审查人: <independent-reviewer hat>
- 审查产物: `.ralph/red-team/10-independent-review.md`
- 结论: PLAN_READY | PLAN_REJECTED

## 禁止事项
- 禁止无关重构
- 禁止顺手修复其他 Finding
- 禁止删除失败测试
- 禁止弱化失败断言
- 禁止 catch-and-ignore
- 禁止返回假成功
- 禁止改变公共行为来规避问题
- 禁止扩大公共接口
- 禁止通过默认关闭功能掩盖缺陷
- 禁止使用不达标 Finding 生成 Unit

## 人工确认
- [ ] 操作者已阅读 REPORT.md
- [ ] 操作者已阅读 QUESTIONS.md 并回答所有问题
- [ ] 操作者明确授权执行本计划
- 确认签名: _______________
- 确认时间: _______________
