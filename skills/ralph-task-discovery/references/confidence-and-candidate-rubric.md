# Confidence & Candidate Rubric(评分与候选裁决规程 — 冻结 SSOT)

本文件是 ralph-task-discovery 候选方案评分、淘汰与重算的**单一事实源**
(Single Source of Truth)。所有常量与 [../scripts/task_brief.py](../scripts/task_brief.py)
中的冻结常量一一对应,由 `skills/tests/test_task_discovery_contract.py` 的
机器可读块一致性测试锁定;**改这里必须同步改代码,改代码必须同步改这里**。

核心原则:

1. **所有分数可回溯到 Evidence**——任何声明的 confidence / 重算分数都必须能
   被证据台账中的条目支持;
2. **禁止总平均分掩盖单维度**——五个关键维度与候选三门禁全部逐维独立判定;
3. **同一 Evidence 重复引用不得提升分数**——支持度计算按证据 id 去重;
4. **新证据导致可审计的重算**——重算必须留下完整的
   `investigation_attempts` 审计链;
5. **矛盾证据不得被静默覆盖**——只能由用户裁决,不能被新证据冲掉。

## 机器可读冻结块(测试锁定)

<!-- rubric-yaml:start -->
```yaml
schema: confidence-and-candidate-rubric/1
author_ready_threshold: 0.85
reject_threshold: 0.70
score_inflation_tolerance: 0.05
attempt_limit: 3
evidence_levels: [E0, E1, E2, E3, E4]
evidence_level_support:
  E0: 0.05
  E1: 0.15
  E2: 0.25
  E3: 0.40
  E4: 0.55
completion_evidence_levels: [E3, E4]
key_dimensions:
  - goal_clarity
  - project_fact_coverage
  - acceptance_evidence
  - execution_feasibility
  - risk_coverage
candidate_coverage_gates:
  goal_coverage: 0.80
  acceptance_coverage: 0.85
  project_fit: 0.75
candidate_statuses:
  - pending
  - selected
  - rejected_low_confidence
  - rejected_insufficient_coverage
```
<!-- rubric-yaml:end -->

## 1. 证据等级 → 单条证据可用支持度

证据等级语义(与 task-brief-schema.md §5 一致):

| 等级 | 语义 | 单条可用支持度(冻结权重) |
| ---- | ---- | -------------------------- |
| E0 | 用户陈述 / 未验证直觉 | 0.05 |
| E1 | 项目文档 / 配置 / 规则文件 | 0.15 |
| E2 | 源码 / 类型 / 调用链 / 测试入口 | 0.25 |
| E3 | 实际执行的构建 / 测试 / CLI / HTTP / replay 结果 | 0.40 |
| E4 | 独立验收场景 / 真实用户路径 / 可复现回归证据 | 0.55 |

支持度计算(`task_brief.compute_support`,纯函数):

- 输入是**去重后**的证据 id 集合;同一 id 重复引用只计一次——
  **重复引用不提升分数**;
- 每条存在的证据按其等级贡献上表权重;台账中不存在的 id 贡献 0
  (引用完整性由 validator 的 `unreferenced_evidence` 单独审计);
- 结果上限 1.0。
- 权重严格单调递增(E0 < E1 < E2 < E3 < E4):等级更高的证据永远不比
  等级低的证据贡献少。

支持度是**声明 confidence 的可审计上限**:声明分 ≤ 计算支持度总是允许
(保守声明);声明分超出支持度 + `score_inflation_tolerance`(0.05)→
稳定错误 code `score_inflation`(invalid_score 家族)。

## 2. 五个关键维度的硬门禁与分带

五个维度(`key_dimensions`)**逐个独立**过门禁,**禁止用平均值/总分代替**:

- **>= 0.85(`author_ready_threshold`)达标带**:可作为正式决策依据
  (仍须列出证据与未覆盖风险);
- **[0.70, 0.85) 调查带**:停留调查,产生新证据后重算;
  validator 输出 `next_action = rerun_investigation`,不得 author_ready;
- **< 0.70(`reject_threshold`)丢弃带**:`rejected_low_confidence`,
  该维度/候选不得作为正式 Unit 依据。

边界包含语义:0.70 属于调查带,0.85 属于达标带。

## 3. 候选方案三门禁(与 confidence 无关)

候选方案除 confidence 分带外,还有三个**独立覆盖门禁**,confidence 再高
也不能绕过:

| 门禁 | 最小值 |
| ---- | ------ |
| goal_coverage | 0.80 |
| acceptance_coverage | 0.85 |
| project_fit | 0.75 |

**冻结三个,不得新增第四个覆盖门禁。** Candidate 上的 `risk_coverage`
字段仅用于展示/追踪,不参与硬门禁。

未通过任一覆盖门禁的候选 → gate 结论 `rejected_insufficient_coverage`
(failed_gates 列出具体未过门禁)。若该候选还被标 `selected: true`,追加
稳定 code `candidate_coverage_gate_failed`,next_action = `switch_candidate`。

## 4. 低分分类与处置协议

| 候选状态 | 触发条件 | 处置 |
| -------- | -------- | ---- |
| `rejected_low_confidence` | confidence < 0.70 | 淘汰,保留淘汰原因与证据引用;下一步视 attempt 余量决定重查或人工 |
| `rejected_insufficient_coverage` | 任一覆盖门禁未过 | 淘汰(与 confidence 无关);若被标 selected → `switch_candidate` |
| 调查带(pending) | 0.70 ≤ confidence < 0.85 且覆盖达标 | 停留调查带,`next_action = rerun_investigation`,补证据后重算;不得 author_ready |
| `selected` | confidence ≥ 0.85 且三门禁全过 且被标 selected | 进入 author_ready 认证(还须满足 §5 完成证据条件) |

候选 `status` 字段允许值:`pending` / `selected` /
`rejected_low_confidence` / `rejected_insufficient_coverage`。
声明的 status 与门禁结论矛盾 → 稳定 code `candidate_status_inconsistent`。
候选 `id` 必须在候选台账内唯一、决策 `id` 必须在决策台账内唯一:重复 →
稳定 code `duplicate_candidate_id` / `duplicate_decision_id`,validator 拒收
(消除认证侧与消费侧按同 id 匹配时的错位)。

## 5. 完成证据条件(E3/E4)

被标 selected 且通过 confidence 分带与三门禁的候选,其
`supporting_evidence` **必须至少包含一条 E3 或 E4 等级证据**
(`completion_evidence_levels`,实际执行/独立验收级别的完成证据),
否则:

- author_ready 认证被阻断(无条件);
- 若 brief 声明 `author_ready` → 稳定 code
  `selected_candidate_missing_acceptance_evidence`,
  next_action = `rerun_investigation`(去产生真实执行证据)。

## 6. 重算审计(investigation_attempts)

低置信度候选补证据重算时,brief 必须记录 `investigation_attempts` 列表,
每条包含:`round`(轮序号)、`candidate_id`、`added_evidence`(本轮新增
证据 id)、`score_before` / `score_after`(重算前后分数)、`provenance`
(出处/结论)。validator 审计:

1. round 序号从 1 开始连续递增;
2. `candidate_id` 必须指向存在的候选;
3. `added_evidence` 的每个 id 必须存在于证据台账(缺失 →
   `unreferenced_evidence`,进入 missing_evidence 清单);
4. 同一候选的相邻两轮必须链式衔接:本轮 `score_before` == 上轮
   `score_after`;
5. 每个 `score_after` 与该候选当前(去重后)证据的支持度一致:
   `score_after ≤ compute_support + score_inflation_tolerance`,
   超出 → `score_inflation`;
6. 候选当前声明的 confidence 不得显著高于最后一轮 `score_after`
   (超出容差 → `score_inflation`)。

结构违规(round 跳号 / 候选引用缺失 / 链断裂)→ 稳定 code
`investigation_attempt_invalid`。

**审计范围**:支持度审计只作用于有重算历史(出现在
`investigation_attempts` 中)的候选;无重算历史的原始声明沿用既有分带
语义(分带判定,不做证据支持度追溯)。

## 7. 三轮上限(有限轮数调查)

- 每补一轮调查 `attempt_count` +1;
- `attempt_count >= 3`(`attempt_limit`)且仍无达标候选 → 状态必须
  `blocked`,handoff_block_reasons 列出**已尝试候选及其结论**与**需要的
  人工输入**;
- **validator 不得建议第四轮自动调查**——blocked 之后只能
  `needs_user_decision` 或保持 `blocked`(单向转换表)。

## 8. 替代候选选择规则

- 多个候选可以并存,但**至多一个**可以被标 `selected: true` 且通过全部
  硬门禁;
- 两个(及以上)候选同时标 `selected: true` 且都达标(等分或不等分)→
  稳定 code `ambiguous_selected_candidates`,recommended_status 降到
  `needs_user_decision`:必须显式选择其一,或交给用户裁决;
- 一个候选被淘汰(coverage 不足 / 低置信度)而另一个达标 → 达标且被标
  selected 的候选成立,author_ready 不受被淘汰候选影响——**高 confidence
  候选的单维度短板不能被另一个候选的高分或平均分掩盖**。

## 9. 矛盾证据处置

同一主题出现两条及以上互相矛盾的 **E3/E4** 证据时(brief 中用证据
observation 前缀 `conflicting_evidence:<主题>:` 标记):

- **禁止 author_ready**;recommended_status → `needs_user_decision`
  (attempt 耗尽时 → `blocked`);
- **不得用新证据静默覆盖旧事实**——validator 不删除、不改写任何证据;
- 唯一解除路径是**用户裁决**:一个 `resolved: true` 的 blocking 决策,
  其 `supporting_evidence` 同时引用该主题的**全部**冲突证据 id;
- 部分引用(只引用冲突的一侧)不构成裁决,矛盾保持未决。

## 10. 禁止平均分掩盖(显式条款)

任何门禁判定都**不得**使用五个维度的平均值、候选分数的平均值或任何
聚合分来替代单维度判定:

- 五维硬门禁:逐维 >= 0.85;
- 候选三门禁:逐门 >= 最小值;
- 候选淘汰:任一维度/门禁失败即淘汰,其它维度的高分不能补偿;
- 多候选并存:逐个候选独立评估,候选之间的分数不得互相借用或平均。
