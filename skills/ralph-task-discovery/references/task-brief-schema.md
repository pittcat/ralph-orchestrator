# Task Brief 数据契约与硬门禁(validator 使用手册)

本文档是 `ralph-task-discovery` skill 的 agent-facing 契约说明:一份结构化
task brief(任务简报)应该长什么样、每个字段从哪里取值、什么时候必须停下来。

## 1. 这份契约解决什么问题

在进入正式计划(plan)与 Unit(实施单元)之前,必须先完成一轮任务发现:
确认目标、收集证据、做出关键决策、挑选候选方案。**task brief** 就是这轮
发现结果的结构化载体(YAML)。**validator(硬门禁校验器)** 机器化地判定
这份 brief 是否达到可交接标准——它只认字段、数值与引用关系,不认"感觉差不多"。

关键术语:

- **task brief(任务简报)**:一次任务发现结果的结构化 YAML 描述。
- **handoff(交接)**:把验证通过的 brief 交给计划作者去写正式计划。
- **author_ready**:brief 的一种状态,表示"可以交接给计划作者"。
- **硬门禁**:不可商量、不可用平均分绕过的单条判定规则。
- **attempt(调查轮次)**:为补齐证据/确认而重复进行的一轮调查,记入 `attempt_count`。

## 2. 顶层字段总表

| 字段 | 必填 | 来源 | 填充规则 | 缺失/非法时 validator 行为 |
|---|---|---|---|---|
| `schema_version` | 是 | 本契约版本 | 目前唯一合法值:字符串 `"1.0"`(必须带引号,否则 YAML 会解析成浮点数) | `schema_version_invalid` |
| `project_root` | 是 | brief 所属项目的根目录路径 | 非空字符串,记录 provenance(出处),供后续计划定位代码树 | `root_provenance_missing` |
| `status` | 是 | 本轮验证结论 | 五个枚举值之一,见 §3 | 缺失 `missing_required_field`;非法值 `unknown_status` |
| `previous_status` | 否 | 上一轮验证时记录的 status | 五个枚举值之一;用于验证单向状态转换,见 §3 | 非法值/非法转换 `state_transition_invalid` |
| `attempt_count` | 否(默认 1) | 本任务已进行的调查轮次计数 | >= 1 的整数;每做完一轮补齐调查就 +1 | 非法值 `invalid_score` |
| `goal` | 是 | 与用户确认后的任务目标 | 一句话描述,非空字符串 | `missing_required_field` |
| `confidence` | 是 | 本轮发现后的评估 | 五个关键维度各一个 [0,1] 浮点数,见 §4 | 缺段/缺维度 `missing_required_field`;越界/非数值 `invalid_score` |
| `evidence` | 是(可为空列表) | 调查发现的事实 | 证据台账,见 §5 | 条目字段问题见 §5 |
| `decisions` | 是(可为空列表) | 发现中做出的关键决策 | 决策记录列表,见 §6 | 条目字段问题见 §6 |
| `candidates` | 是(可为空列表) | 设计出的可行方案 | 候选方案列表,见 §7 | 条目字段问题见 §7 |
| `user_confirmations` | author_ready 时必填 | 与用户逐项确认的记录 | 四项确认,见 §8 | author_ready 条件不满足 → `author_ready_gate_violation` |

## 3. 状态机:五种状态与单向转换

`status` 只允许(全部小写):

| 状态 | 含义 |
|---|---|
| `draft` | 初始草稿,尚未系统调查 |
| `needs_investigation` | 还差证据,需要再调查一轮后重算 |
| `needs_user_decision` | 还差用户拍板(逐项确认或决策) |
| `blocked` | 三轮调查耗尽仍不达标,等待人工输入 |
| `author_ready` | 通过全部硬门禁,可交接给计划作者 |

**单向转换表**(`previous_status` → 允许的 `status`):

| 上一轮 | 允许进入 |
|---|---|
| `draft` | `needs_investigation` / `needs_user_decision` / `blocked` |
| `needs_investigation` | 自身 / `needs_user_decision` / `blocked` / `author_ready` |
| `needs_user_decision` | 自身 / `needs_investigation` / `blocked` / `author_ready` |
| `blocked` | 自身 / `needs_user_decision`(blocked 之后**只能**等人工输入,不得自动重新调查) |
| `author_ready` | 自身 / `blocked` |

要点:

- `draft` **不能**直接跳到 `author_ready`——没经过调查与确认的 brief 不可能合法达标。
- 违反转换表 → `state_transition_invalid`,必须停下修正。
- validator 会独立推导一个**状态建议**(`recommended_status`);声明状态与
  门禁结论冲突时同样报错。特别是:`attempt_count >= 3` 且仍不达标时,
  状态**必须**声明 `blocked`,否则报 `state_transition_invalid`,
  next_action 为 `emit_blocked`(输出 blocked)。

## 4. 五个关键置信度维度

`confidence` 下五个维度,**每个独立判定,禁止用平均值代替**:

| 维度 | 语义 |
|---|---|
| `goal_clarity` | 目标、范围、非目标、用户决策都已确认 |
| `project_fact_coverage` | 入口、调用链、现有模式、验证命令、影响面都有证据 |
| `acceptance_evidence` | 每个重要结果都有可执行或可观察的完成证据 |
| `execution_feasibility` | 至少一个候选方案能在项目能力与约束下执行 |
| `risk_coverage` | 关键失败、兼容、权限、外部依赖、恢复风险已处理 |

分带规则(对任何分数——维度、决策 confidence、候选 confidence 通用):

| 分数 | 带 | 后果 |
|---|---|---|
| `< 0.70` | 丢弃带 | 对应维度/决策/候选立即丢弃,标 `rejected_low_confidence`,不得进入正式 Unit;next action = 重新调查或逐题向用户确认 |
| `0.70 <= 分数 < 0.85` | 调查带 | 只能 `needs_investigation`,必须产生新证据后重算 |
| `>= 0.85` | 达标带 | 可作为正式决策,但仍必须列出证据与未覆盖风险 |

**边界包含语义**:0.70 属于调查带(不被丢弃),0.85 属于达标带(达标)。

author_ready 要求**五个维度全部** `>= 0.85`;任何一个维度不达标,
无论其它维度多高、平均多高,都不能交接。

## 5. Evidence(证据台账)

每条证据是一个 mapping:

| 字段 | 必填 | 规则 |
|---|---|---|
| `id` | 是 | 台账内唯一,如 `E1`、`E2`;决策/候选用它引用。缺失 → `missing_required_field`;重复 → `duplicate_evidence_id` |
| `source` | 是 | 证据出处(文件路径、命令输出、对话记录等)。缺失 → `missing_required_field` |
| `observation` | 是 | 观察到的事实陈述。缺失 → `missing_required_field` |
| `level` | 是 | 证据等级,见下表。非法值 → `invalid_evidence_level` |

证据等级(从弱到强):

| 等级 | 含义 |
|---|---|
| `E0` | 用户陈述 / 未验证直觉 |
| `E1` | 项目文档 / 配置 / 规则文件 |
| `E2` | 源码 / 类型 / 调用链 / 测试入口 |
| `E3` | 实际执行的构建 / 测试 / CLI / HTTP / replay 结果 |
| `E4` | 独立验收场景 / 真实用户路径 / 可复现回归证据 |

**引用完整性**:Decision 与 Candidate 的 `supporting_evidence` 里每个 id
必须存在于证据台账,且列表不得为空。违反 → `unreferenced_evidence`,
缺失的 id 会进入验证结果的 `missing_evidence` 清单。

## 6. Decision Record(决策记录)

每条决策是一个 mapping:

| 字段 | 必填 | 规则 |
|---|---|---|
| `id` | 是 | 如 `D1` |
| `question` | 是 | 决策对应的问题 |
| `confidence` | 是 | [0,1] 浮点数,按 §4 分带 |
| `supporting_evidence` | 是 | 证据 id 列表,不得为空,必须可解析(§5) |
| `blocking` | 是 | 布尔。`true` = author-blocking 决策:计划作者必须依赖它才能写计划 |
| `resolved` | 是 | 布尔。`false` = 未决,author_ready 前必须解决 |
| `resolution` | 建议 | 结论陈述 |
| `uncovered_risks` | blocking 且达标时必填 | 未覆盖风险列表(可为空列表,但必须显式写出)——置信度 `>= 0.85` 的 blocking 正式决策必须列出 |

author-blocking 决策的额外硬门禁:

- 每条 author-blocking 决策的 `confidence` 必须**单独** `>= 0.85`才能支撑 author_ready;
- blocking 决策 `confidence < 0.70` → 立即丢弃(`rejected_low_confidence`);
- 存在未决(blocking 且 `resolved: false`)决策 → 不能 author_ready。

## 7. Candidate(候选方案)

每条候选是一个 mapping:

| 字段 | 必填 | 规则 |
|---|---|---|
| `id` | 是 | 如 `C1` |
| `summary` | 是 | 方案一句话描述 |
| `confidence` | 是 | [0,1] 浮点数,按 §4 分带 |
| `goal_coverage` | 是 | 方案对目标的覆盖度,[0,1] |
| `acceptance_coverage` | 是 | 方案对验收/完成证据的覆盖度,[0,1] |
| `project_fit` | 是 | 方案与项目现有能力、约束、模式的契合度,[0,1] |
| `supporting_evidence` | 是 | 证据 id 列表,不得为空,必须可解析(§5) |
| `selected` | 是 | 布尔。是否为当前选定方案 |

**独立覆盖门禁**(与 confidence 完全无关,confidence 再高也不能绕过):

| 门禁 | 阈值 |
|---|---|
| `goal_coverage` | `>= 0.80` |
| `acceptance_coverage` | `>= 0.85` |
| `project_fit` | `>= 0.75` |

判定结果(`candidate_gates`):

- 覆盖门禁任一不达标 → 标 `rejected_insufficient_coverage`;若该候选还被标
  `selected`,则额外报 `candidate_coverage_gate_failed`,next action = 换候选;
- `confidence < 0.70` → 标 `rejected_low_confidence`;
- 全部通过且 `confidence >= 0.85`:被标 `selected` → 结果 `selected`,否则 `viable`;
- 覆盖通过但 confidence 落在调查带 → 结果 `needs_investigation`。

author_ready 要求**至少一个**候选通过全部硬门禁并被标 `selected`。

## 8. user_confirmations(用户确认记录)

author_ready 前,必须存在四项确认,且每项 `confirmed: true`:

| 键 | 确认内容 |
|---|---|
| `goal` | 目标 |
| `scope` | 范围 |
| `completion_evidence` | 完成证据(什么算做完) |
| `failure_boundaries` | 关键失败边界(哪些失败必须停下来) |

每项含 `confirmed`(布尔)与 `note`(确认摘要)。缺段、缺项或未确认 →
author_ready 被拒,状态建议降为 `needs_user_decision`,next action = 逐题问用户。

## 9. author_ready 的充要条件(全部满足才可交接)

1. 五个关键置信度维度**均** `>= 0.85`;
2. 至少一个候选方案通过全部硬门禁(覆盖门禁 + confidence `>= 0.85`)并被标 `selected`;
3. 用户确认记录四项齐全且全部 `confirmed: true`;
4. 无未决的 author-blocking 决策,且每条 author-blocking 决策 confidence `>= 0.85`;
5. `schema_version` 与 `project_root` provenance 字段有效。

任何一条不满足:声明 author_ready 会被判 `author_ready_gate_violation`
(valid=false),验证结果给出门禁推导的状态建议与**禁止 handoff 原因清单**
(`handoff_block_reasons`)。

## 10. attempt 耗尽规则

`attempt_count >= 3` 且 brief 仍不达标(存在错误或任一 author_ready 条件未满足):

- 状态建议强制为 `blocked`;声明其它状态 → `state_transition_invalid`(next action = `emit_blocked`);
- validator **不会**建议第四轮自动调查;整体 next action 指向人工输入
  (`confirm_with_user`),所有丢弃建议的 next action 也随之指向人工输入。

## 11. 稳定错误码与 next_action 词表

每个错误含四元组:`code`(稳定码)、`path`(JSON-path 位置,如
`$.confidence.goal_clarity`、`$.decisions[0].supporting_evidence`)、
`message`(人类可读说明)、`next_action`。

| code | 含义 |
|---|---|
| `unknown_status` | status 不在允许枚举中 |
| `missing_required_field` | 必填字段缺失或类型错误 |
| `invalid_score` | 数值字段越界或非数值(分数必须在 [0,1];attempt_count 必须是 >= 1 整数) |
| `unreferenced_evidence` | 引用了台账中不存在的证据 id,或决策/候选完全没有证据引用 |
| `invalid_evidence_level` | 证据等级不是 E0–E4 |
| `author_ready_gate_violation` | 声明 author_ready 但充要条件未满足 |
| `candidate_coverage_gate_failed` | 被标 selected 的候选未通过独立覆盖门禁 |
| `state_transition_invalid` | 状态转换违反单向转换表,或耗尽后未声明 blocked |
| `schema_version_invalid` | schema_version 缺失或不是 `"1.0"` |
| `root_provenance_missing` | project_root 缺失或为空 |
| `duplicate_evidence_id` | 证据 id 在台账中重复 |
| `invalid_yaml` | YAML 文本本身解析失败 |

next_action 词表(机器可读):

| token | 含义 |
|---|---|
| `ready_for_handoff` | 可交接给计划作者 |
| `rerun_investigation` | 补调查、产生新证据后重算 |
| `confirm_with_user` | 逐题问用户 / 等待人工输入 |
| `switch_candidate` | 换候选方案 |
| `emit_blocked` | 输出 blocked 状态 |

## 12. 验证结果字段(ValidationResult)

| 字段 | 含义 |
|---|---|
| `valid` | brief 内部一致、无任何错误 |
| `author_ready` | 门禁认证结论:§9 充要条件全部满足 |
| `recommended_status` | 门禁推导的状态建议 |
| `next_action` | 整体下一步(§11 词表) |
| `errors` | 错误四元组列表 |
| `rejections` | 被丢弃的维度/决策/候选(kind / id / reason / next_action) |
| `candidate_gates` | 每个候选的门禁结论(candidate_id / outcome / failed_gates) |
| `handoff_block_reasons` | 禁止 handoff 的原因清单(可交接时为空) |
| `missing_evidence` | 被引用但台账中不存在的证据 id 清单 |

结果可序列化为 dict / JSON / YAML(`to_dict()` / `to_json()` / `to_yaml()`),
便于作为 artifact 落盘传递。

## 13. 失败停止条件(agent 必读)

遇到以下任一情况,**必须停止 handoff**,按 next_action 行动:

1. `valid=false`:先修复错误清单中的全部条目,再重新验证;
2. `author_ready=false`:按 `recommended_status` 与 `next_action` 行动
   (补调查 / 逐题问用户 / 换候选 / 输出 blocked),**不要**试图通过改写
   声明状态或调高分数绕过——validator 按字段与引用关系独立判定;
3. 任何 `rejected_low_confidence` 丢弃:对应维度/决策/候选不得进入正式 Unit;
4. `blocked`:只能等待人工输入,不得自动开启新一轮调查;
5. YAML 解析失败(`invalid_yaml`):先修复格式,绝不能用"看起来对"的
   内存对象替代真实解析。

## 14. 如何运行验证

```bash
# 从仓库根目录运行(使用 skills 目录的虚拟环境)
skills/.venv/bin/python - <<'PY'
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path("skills/ralph-task-discovery/scripts")))
import brief_validator

text = pathlib.Path("path/to/your-brief.yml").read_text(encoding="utf-8")
result = brief_validator.validate_brief_text(text)
print(result.to_json())
PY
```

- YAML 文本入口:`validate_brief_text(text)`(真实 `yaml.safe_load`,格式错误无法绕过);
- 已解析 mapping 入口:`validate_brief_data(data)`;
- 测试用 fixtures 位于 `skills/ralph-task-discovery/fixtures/`:
  `valid.yml`(达标样例)与五个负例(`missing-evidence` / `medium-confidence` /
  `low-confidence` / `coverage-fail` / `blocked`),可作为撰写 brief 的参照。
