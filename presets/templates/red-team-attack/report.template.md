# Red Team Report Template
#
# Copy to `.ralph/red-team/REPORT.md` and fill every section.
# This is the operator-facing deliverable. Write for a decision-maker,
# not for the agent that ran the loop.
#
# Artifact lifecycle:
#   owner: reporter (writes this file)
#   readers: operator (final decision), independent-reviewer (audit trail)
#   retention: operator-owned; agents must not delete

# Red Team Attack Report

## 结论
PLAN_READY | PLAN_REJECTED

## 一句话摘要
<one sentence: what was attacked, what was found, what is recommended>

## 输入
- 开发计划列表: <plans>
- 目标分支: <branch>
- 锁定 HEAD: <head_sha>
- 锁定 tree: <tree_sha>

## 执行摘要
- 计划解析: X/Y plans resolved (commit match ≥85)
- Patch 重建: attribution coverage Z%, critical claim traceability W%
- 攻击面识别: N surfaces
- 实验执行: M experiments (K passed gates, L rejected after retry)
- 正式 Finding: F findings (P0: a, P1: b, P2: c, P3: d)

## 正式 Finding 列表
| ID | Severity | Title | Confidence | Evidence | Verifiability | Impact | Status |
|---|---|---|---|---|---|---|---|
| RTF-001 | P1 | ... | 92 | 88 | 91 | 87 | IMPACT_QUALIFIED |

## 被拒绝候选（Retry 耗尽后仍不达标）
| Candidate | Failed Metric | Final Score | Threshold | Retry Attempts | Rejection Reason |
|---|---|---|---|---|---|
| RTE-003 | verifiability | 82 | 90 | 3 | REJECTED_AFTER_RETRY_LOW_VERIFIABILITY |

## 修复计划（PLAN.md 摘要）
- Unit 0: 锁定修复前基线
- Unit 1: RTF-001 — Red → Green → Refactor → Regression
- ...

## 零回归约束检查清单
- [ ] 先锁定现有行为
- [ ] 先生成失败测试
- [ ] 限制最小修复范围
- [ ] 禁止无关重构
- [ ] 逐 Finding 回归
- [ ] 跨计划集成回归
- [ ] 全量测试
- [ ] 静态检查
- [ ] 干净环境重建
- [ ] 独立最终审查

## 需要人工确认的问题（QUESTIONS.md）
- [ ] 问题 1: ...
- [ ] 问题 2: ...

## 证据索引
- 实验证据: `.ralph/red-team/evidence/RTE-*/**`
- 原始日志: `.ralph/red-team/logs/**`
- Patch 存档: `.ralph/red-team/patches/**`
- 独立审查: `.ralph/red-team/10-independent-review.md`

## 交付声明
- DELIVERABLE_PATH: `.ralph/red-team/PLAN.md`
- REPORT_PATH: `.ralph/red-team/REPORT.md`
- QUESTIONS_PATH: `.ralph/red-team/QUESTIONS.md`
- EXECUTION_AUTHORIZED: false
- CONFIRMATION_REQUIRED: true

## 下一步
等待操作者明确确认后，方可将 PLAN.md 交给 Coding Agent 执行。
