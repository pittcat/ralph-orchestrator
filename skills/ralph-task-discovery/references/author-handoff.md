# Author Handoff:task brief 交接与消费规程

本文档是 `ralph-task-discovery` → `ralph-preset-author` 交接的**书面协议**:
discovery 侧产出通过硬门禁的 task brief,author 侧按本文档复核并消费。
术语与门禁语义见 [task-brief-schema.md](task-brief-schema.md),评分与候选
规则见 [confidence-and-candidate-rubric.md](confidence-and-candidate-rubric.md);
author 侧校验序列的确定性参考实现是
[scripts/author_handoff.py](../scripts/author_handoff.py)
(`evaluate_task_brief(brief_path, target_project_root)`,返回
`author_handoff_ok` / `task_brief_invalid` 裁定)。

**交接原则:** handoff 只传 brief 路径(repo-relative),不复制长文本进
prompt;author 自己读取文件并独立复核。brief 的 `author_ready` 标志
**不是**跳过 author 既有门禁的许可——它只提供已确认输入,Discovery /
Intent Confirmation / AAF / Payload Contract / prompt visibility /
pre-review gate / review handoff 全部照常执行。

## 消费顺序(author 侧强制)

author 收到 `task_brief_path` 后,必须按以下顺序读取与校验;任一步失败
立即输出 `task_brief_invalid` + 对应错误(code/path),停在 Discovery gate,
**不生成任何 preset**,也不消费 brief 的任何字段:

1. **文件存在**:brief 路径可读。缺失 → `task_brief_invalid` +
   `task_brief_file_not_found`(author 侧 code)。
2. **YAML 可解析**:运行 `brief_validator.validate_brief_text`
   (真实 `yaml.safe_load`,格式错误无法绕过)。解析失败 →
   `task_brief_invalid` + validator code `invalid_yaml`。
3. **`schema_version` 受支持**:当前唯一支持 `"1.0"`(validator 判定)。
   不受支持 → `task_brief_invalid` + `schema_version_invalid`。
4. **`project_root` 与当前目标项目根一致**(author 侧匹配,validator
   只校验该字段非空):规范化比较(尾随斜杠等价)。不一致 →
   `task_brief_invalid` + `task_brief_root_mismatch`;字段缺失时不重复
   报错,由 validator 的 `root_provenance_missing` 覆盖。
5. **运行完整 validator**:五维置信度、证据台账、决策、候选三门禁、
   重算审计、author_ready 充要条件全部复核。任一错误 →
   `task_brief_invalid` + 对应 validator code/path(如
   `author_ready_gate_violation` / `candidate_coverage_gate_failed`)。
6. **`status` / `author_ready` 复核**:只有 validator 认证
   `author_ready == true` 且 `next_action == ready_for_handoff` 的 brief
   才可消费。brief 内部一致但未认证(诚实声明 blocked /
   needs_user_decision / needs_investigation)→ `task_brief_invalid` +
   `task_brief_not_author_ready`(author 侧 code,message 携带 validator
   的 `handoff_block_reasons`)。
7. **字段消费**:仅在第 1–6 步全部通过后,按下一节字段清单消费已确认事实。

错误汇报顺序与上述读取顺序一致:validator 错误按其自身顺序透传,
author 侧 root mismatch 在其后,`task_brief_not_author_ready` 仅在无其它
错误时补充。

## 最小可见字段清单(可消费事实)

author 只消费以下字段;任何清单外的 brief 内容都不得当作已确认输入:

| brief 字段 | author 消费位置 |
|---|---|
| `goal` | Preset Intent Confirmation 的目标 |
| `confidence`(五维) | 置信度背景参考(逐维读取,禁止平均值理解) |
| `evidence`(台账) | Intent Confirmation 的 Evidence refs;证据等级语义见 schema §5 |
| `decisions` | 已确认决策背景;resolved blocking 决策的结论可直接引用 |
| `candidates`(含 `status` / `selected` / `rejection_reason`) | 方案输入;**selected 只认 validator `candidate_gates` 结论为 `selected` 的候选**,被 rejected 的候选不得被当作 selected 使用 |
| `user_confirmations`(goal / scope / completion_evidence / failure_boundaries) | Intent Confirmation 的成功条件(acceptance)、阻塞条件(failure boundaries)、scope 与目标确认 |
| `investigation_attempts` | 重算审计背景(哪些证据是后补的、分数如何收敛) |

## stale brief 判据

满足任一条件即判定 brief 对当前 authoring 任务**陈旧(stale)**,
输出 `task_brief_invalid`,不得消费:

1. **root mismatch**:brief `project_root` 与当前目标项目根不一致
   (`task_brief_root_mismatch`)——brief 属于另一个代码树;
2. **provenance 错误**:validator 报 `schema_version_invalid` 或
   `root_provenance_missing`——brief 的出处声明本身不可信;
3. **目标漂移**:author 读取 `goal` 后与当前 authoring 请求交叉核对,
   目标/范围明显不符——按 stale 处理,回到 Discovery 重新确认,不得
   凭旧 brief 起草。

## 停止条件与错误输出汇总

| 情形 | verdict | 错误 code | 来源 |
|---|---|---|---|
| brief 文件缺失 | `task_brief_invalid` | `task_brief_file_not_found` | author 侧 |
| YAML 解析失败 | `task_brief_invalid` | `invalid_yaml` | validator |
| schema_version 不受支持 | `task_brief_invalid` | `schema_version_invalid` | validator |
| project_root 与当前目标根不一致 | `task_brief_invalid` | `task_brief_root_mismatch` | author 侧 |
| 门禁/完整性错误(含 author_ready 虚假声明) | `task_brief_invalid` | 对应 validator code(如 `author_ready_gate_violation`) | validator |
| valid 但未认证(blocked / needs_user_decision / needs_investigation) | `task_brief_invalid` | `task_brief_not_author_ready` | author 侧(携带 `handoff_block_reasons`) |
| 全部通过 | `author_handoff_ok` | — | — |

所有 `task_brief_invalid` 情形的作者侧行为一致:停在 Discovery gate,
向调用方报告 verdict + 错误清单(code/path/message),不生成任何 preset
YAML,不消费 brief 的任何字段。

## 端到端证据/错误映射(U6 预留结构)

下表是 discovery 产出 → author 消费/停止的端到端映射,作为后续 e2e
测试(U6)的结构化基础:

| 环节 | discovery 侧输出 | author 侧消费 / 停止 |
|---|---|---|
| 交接物 | brief 文件路径(`author_ready` + `ready_for_handoff`) | Workflow 0 接收 `task_brief_path`,只传路径,不复制长文本 |
| 文件 | brief YAML 落盘 | 存在性检查;缺失 → `task_brief_file_not_found` |
| 格式 | YAML 文本 | `validate_brief_text` 真实解析;失败 → `invalid_yaml` |
| provenance | `schema_version` / `project_root` | 版本支持 + 目标根匹配;失败 → `schema_version_invalid` / `root_provenance_missing` / `task_brief_root_mismatch` |
| 门禁 | validator 裁定(`valid` / `author_ready` / `errors`) | 复核认证;失败 → 透传 validator code |
| 状态 | `status` / `recommended_status` / `handoff_block_reasons` | 仅接受认证通过的 brief;未认证 → `task_brief_not_author_ready` |
| 事实 | goal / confirmations / candidates / evidence | Intent Confirmation 引用;selected 仅取 `candidate_gates` 结论为 selected 的候选 |
| 门禁不豁免 | — | brief 通过后,author 既有 Discovery / AAF / Payload Contract / review handoff 照常执行 |
