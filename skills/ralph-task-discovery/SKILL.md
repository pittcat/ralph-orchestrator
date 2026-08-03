---
name: ralph-task-discovery
description: >-
  Structured task discovery before formal planning: investigate project facts
  from the environment (never re-ask the user for facts), confirm business
  decisions one question at a time with recommendations, record
  terminology/boundary conflicts explicitly, require a red-capable feedback
  loop before bug root-causing, and converge a hard-gated task brief
  (Evidence/Decision/Candidate) that hands off to ralph-preset-author only at
  author_ready. Use when a task needs discovery before a plan; not for preset
  authoring/review, loop ops, or post-run diagnosis.
---

# Ralph Task Discovery

在进入正式计划(plan/Unit)之前完成一轮结构化任务发现:事实由项目调查获得,
决策与用户逐题确认,最终产出一份通过硬门禁的 **task brief(任务简报)**,
且只在 `author_ready` 状态交接给计划作者。

**契约速查**:字段与门禁语义 → [references/task-brief-schema.md](references/task-brief-schema.md);
外部 skill 方法规程 → [references/external-skill-adapters.md](references/external-skill-adapters.md)。
本文档不复述这两份契约的细节,只定义调用顺序、状态与停止条件。

## 边界

- **输入。** 调用方提供:项目根目录(cwd)+ 一句话目标请求。
- **输出。** task brief YAML 路径 + validator 裁定结果
  (`recommended_status` / `next_action` / `handoff_block_reasons`)。
  交接物**只有 brief 路径**。
- **不生成 preset。** preset / hat 起草属于 `ralph-preset-author`;
  本 skill 到 `author_ready` 为止。
- **不修改目标项目。** 发现阶段不写代码、不改 glossary、不写目标项目文档;
  冲突只记录,裁决与修改留给用户决策之后。
- **不调用 Ralph 状态命令。** 发现阶段不执行 `ralph emit` /
  `ralph tools` 等任何状态变更命令。
- **事实查环境,不反问用户。** 凡能从项目调查获得的事实
  (包结构、测试入口、CI、glossary、代码行为)一律自己查证,
  不得作为问题抛给用户。
- **决策属于用户。** 业务决策一次一问、每问附推荐项;
  agent 不得替用户确认。

## 工作流

调用顺序(每步都写入 brief;停止条件随时生效):

1. **解析输入。** 确认 cwd 为项目根;把目标请求记为 `goal` 初始陈述;
   brief 初始为 `draft`。
2. **项目事实调查(environment 路由)。** 按
   [scripts/discovery_transcript.py](scripts/discovery_transcript.py) 的
   `unknown_topics()` / `open_questions()` 列出的缺口,补齐该任务类型的必需事实:
   包结构、可真实运行的测试命令、CI 门禁;bug 任务额外要求代码入口与
   **red-capable 反馈回路命令**(必须已真实运行过一次、能针对真实症状变红)。
   每条事实写入证据台账(等级 E1–E3,见 schema §5)。
   事实类问题永远路由环境,不进入用户问题清单。
3. **逐题用户决策(user 路由)。** 按 `goal` → `scope` → `completion_evidence` →
   `failure_boundaries` 顺序逐题确认(schema §8)。一次只问一个问题,
   每个问题附推荐项。显式回答 → 记为 resolved 决策 + E0 证据;
   模糊回答 → 产生**恰好一个**带推荐项的澄清问题,对应决策保持未决;
   未获用户确认前不得进入下一题,也不得把推荐项直接当成结论。
4. **领域澄清。** 用项目 glossary 与代码交叉核对用户陈述
   (方法边界见 adapters 的 domain-modeling 行)。术语或边界冲突 →
   记为 `terminology_conflict:` 证据 + 未决 blocking 决策,等待用户裁决;
   **不自动覆盖 glossary 或代码任何一侧**。
5. **任务类型分流。** `feature`:进入候选方案设计,可用
   module/interface/seam/depth 词汇比较设计边界(adapters 的 codebase-design 行)。
   `bug`:无 red-capable 回路 → 停留 `needs_investigation`,
   不产生已确认根因、不产生执行方案(adapters 的 diagnosing-bugs 行)。
6. **Evidence/Decision 写入。** 证据、决策、候选、用户确认按 schema §5–§8
   写入;外部 skill 结果必须带 source/provenance(schema §5.3)。
7. **状态收敛。** 跑一次 validator(运行方式见 schema §14),以
   `recommended_status` 为准:与声明状态不一致时先修 brief;按 `next_action`
   补调查 / 逐题问用户 / 换候选 / 输出 blocked。每补一轮调查
   `attempt_count` +1;三轮仍不达标 → `blocked`,不得开第四轮自动调查。
8. **交接。** 仅当 `author_ready` 且 `next_action == ready_for_handoff` 时,
   把 **brief 路径**交接给 `ralph-preset-author`,发现阶段结束。

上述规则的确定性参考实现在 [scripts/discovery_transcript.py](scripts/discovery_transcript.py)
(transcript → brief 映射,纯函数);deterministic transcript 样例见
[fixtures/transcripts/](fixtures/transcripts/)。

## 状态与停止条件

五状态与单向转换表见 schema §3;author_ready 充要条件见 schema §9;
attempt 耗尽规则见 schema §10。遇到以下任一情况**立即停止**并按
`next_action` 行动:

- 任一硬门禁不满足(五维任一 < 0.85、候选未过硬门禁、用户确认缺失、
  blocking 决策未决)→ 不得交接;
- bug 任务无 red-capable 反馈回路 → 不确认根因、不产生执行方案;
- glossary/代码冲突未裁决 → 不覆盖任何一侧;
- 外部 skill corpus 不可用 → 按 adapters 的 fallback 显式记录
  `external_skill_unavailable:` + `fallback:` provenance,不伪造已执行;
- 无法从外部 skill 文件确认某条规则 → 标 `unverified` 并降低证据等级,
  不得编造;
- `blocked` → 只等人工输入,不得自动开启新一轮调查。

## 外部 skill 方法边界

完整规程(触发条件 / 输入 / 输出 / 证据等级 / fallback / 停止条件 /
provenance)在 [references/external-skill-adapters.md](references/external-skill-adapters.md),此处只给调用边界:

- **model-invoked 方法**(`grilling` / `domain-modeling` / `diagnosing-bugs` /
  `codebase-design`):发现过程中可直接吸收执行;方法产出按 adapters
  绑定的证据等级写入证据台账。
- **user-invoked 流程**(`triage` / `wayfinder` / `grill-with-docs` /
  `to-spec`):**只吸收规则,禁止静默子调用**——它们依赖外部 issue tracker
  与 human-in-loop 语义;discovery 只借用其规则(查重与历史拒绝、
  请求验证、fog 保留、HITL/AFK 区分、blocking 依赖、综合交接),
  不实际执行这些流程本身。
- **不可用处理:** corpus 不可用 → 证据记 `external_skill_unavailable:` +
  `fallback:` provenance;此时绝不写 `external_skill_applied:`。

## 交接边界

- 交接条件:`status == author_ready`,validator `author_ready == true`,
  `next_action == ready_for_handoff`。
- 交接物:**仅 task brief 路径**(YAML);下游 `ralph-preset-author`
  自行读取 brief 与 validator 结果。
- 其余状态(`draft` / `needs_investigation` / `needs_user_decision` /
  `blocked`)一律不得交接。

## 参考

- [references/task-brief-schema.md](references/task-brief-schema.md) — task brief 数据契约与硬门禁 validator 使用手册
- [references/external-skill-adapters.md](references/external-skill-adapters.md) — 外部 skill adapter 规程(provenance / fallback / 停止条件)
- [scripts/task_brief.py](scripts/task_brief.py) — 数据契约(纯数据结构)
- [scripts/brief_validator.py](scripts/brief_validator.py) — 硬门禁 validator
- [scripts/discovery_transcript.py](scripts/discovery_transcript.py) — 工作流规则的确定性实现
- [fixtures/](fixtures/) — brief fixtures;[fixtures/transcripts/](fixtures/transcripts/) — discovery transcript fixtures
