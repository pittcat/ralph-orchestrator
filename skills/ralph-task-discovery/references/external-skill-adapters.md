# 外部 skill adapter 规程(ralph-task-discovery)

本文档是 discovery 工作流对外部 skill corpus 的**适配规程**:每个外部
skill 一行/一节,定义触发条件、输入、输出、绑定的证据等级、不可用时的
最小 fallback、停止条件与 **provenance(外部文件路径)**。

**Corpus 来源(只读)**:`/Users/pittcat/Dev/agent_tools/skills/skills/`。
调用分类与 corpus README 的 `disable-model-invocation` 名单一致:

- **model-invoked**(可直接吸收执行):`grilling` / `domain-modeling` /
  `diagnosing-bugs` / `codebase-design`;
- **user-invoked**(**只吸收规则,禁止静默子调用**):`triage` / `wayfinder` /
  `grill-with-docs` / `to-spec`——它们依赖外部 issue tracker 与
  human-in-loop 语义,discovery 不实际执行这些流程本身。

**证据规则**:外部 skill 结果写入 brief 证据台账时必须带 provenance
(schema §5.3):corpus 可用且方法被应用 → observation 以
`external_skill_applied:<name>` 开头,source 记录 corpus 出处;corpus
不可用 → `external_skill_unavailable:` + `fallback:` provenance,绝不伪造
已执行。无法从外部文件确认的规则标 `unverified`,证据降为 E0。

## 机器可读 adapter 表

<!-- adapters-yaml:start -->
```yaml
adapters:
  - skill: grilling
    invocation_mode: model_invoked
    sub_invocation_policy: invoke-allowed
    trigger: "存在待确认的业务决策主题(goal/scope/completion_evidence/failure_boundaries)"
    inputs: "已知项目事实清单 + 开放决策主题"
    outputs: "逐题问答记录(E0 证据)+ resolved 决策;每问附推荐项"
    bound_evidence_level: "规则出处 E1;用户回答 E0"
    fallback_if_unavailable: "内置四主题逐题确认问题列表(goal/scope/completion_evidence/failure_boundaries)"
    stop_condition: "用户拒绝回答或要求停止;用户确认前不得行动"
    provenance: "/Users/pittcat/Dev/agent_tools/skills/skills/productivity/grilling/SKILL.md"
  - skill: domain-modeling
    invocation_mode: model_invoked
    sub_invocation_policy: invoke-allowed
    trigger: "用户陈述与 glossary(CONTEXT.md)或代码行为出现术语/边界差异"
    inputs: "glossary 条目 + 对应代码路径 + 用户陈述"
    outputs: "terminology_conflict: 证据(E2)+ 未决 blocking 决策"
    bound_evidence_level: "规则出处 E1;冲突证据 E2(与代码交叉核对)"
    fallback_if_unavailable: "内置 glossary/代码交叉核对清单(冲突记录为 terminology_conflict 证据)"
    stop_condition: "冲突未获用户裁决前不得覆盖 glossary 或代码任何一侧"
    provenance: "/Users/pittcat/Dev/agent_tools/skills/skills/engineering/domain-modeling/SKILL.md"
  - skill: diagnosing-bugs
    invocation_mode: model_invoked
    sub_invocation_policy: invoke-allowed
    trigger: "task_type=bug:用户报告损坏/报错/变慢等症状"
    inputs: "症状描述 + 代码入口事实"
    outputs: "red-capable 反馈回路事实(E3);无回路时产生 unknowns 清单"
    bound_evidence_level: "回路命令必须 E3(真实运行过且能针对症状变红)"
    fallback_if_unavailable: "内置 red-capable 反馈回路判据清单(无回路不确认根因)"
    stop_condition: "无 red-capable 命令 → 不确认根因、不产生执行方案;确实构造不出回路时停下并向用户要环境/产物/授权"
    provenance: "/Users/pittcat/Dev/agent_tools/skills/skills/engineering/diagnosing-bugs/SKILL.md"
  - skill: codebase-design
    invocation_mode: model_invoked
    sub_invocation_policy: invoke-allowed
    trigger: "feature 任务进入候选方案设计,需要比较接口/接缝/深度边界"
    inputs: "候选方案 + 现有模块/接口事实(E2)"
    outputs: "候选方案的 summary 与覆盖度评估依据"
    bound_evidence_level: "规则出处 E1;设计比较依据 E2(源码阅读)"
    fallback_if_unavailable: "内置 module/interface/seam/depth 词汇对照表"
    stop_condition: "比较结论必须回落到证据台账;不得以词汇替代证据"
    provenance: "/Users/pittcat/Dev/agent_tools/skills/skills/engineering/codebase-design/SKILL.md"
  - skill: triage
    invocation_mode: user_invoked
    sub_invocation_policy: absorb-rules-only
    trigger: "收到外部任务请求(issue/PR 式请求)需要甄别时"
    inputs: "请求描述 + 项目代码树"
    outputs: "规则吸收结果:重复实现检查与历史拒绝检查的结论证据(E1/E2);bug 类请求的验证结论"
    bound_evidence_level: "查重/历史拒绝 E1/E2;bug 声明验证(复现)为 E3"
    fallback_if_unavailable: "内置重复实现/历史拒绝两项前置检查"
    stop_condition: "只吸收规则,不执行 triage 状态机、不写 issue tracker;验证不了的请求按 needs_investigation 处理"
    provenance: "/Users/pittcat/Dev/agent_tools/skills/skills/engineering/triage/SKILL.md"
  - skill: wayfinder
    invocation_mode: user_invoked
    sub_invocation_policy: absorb-rules-only
    trigger: "目标过大/路径不明,出现无法立即具体化的 fog(未知区域)"
    inputs: "目标请求 + 已知事实缺口"
    outputs: "规则吸收结果:unknowns 清单(fog 保留)、HITL/AFK 区分、决策依赖顺序"
    bound_evidence_level: "fog/unknowns 不构成达标证据,只进 unknowns 清单(未验证陈述按 E0 对待)"
    fallback_if_unavailable: "内置 unknowns 清单(fog 保留,不强行拆问题)"
    stop_condition: "只吸收规则,不创建 tracker map/ticket;需要用户拍板的决策(HITL)绝不自行代答"
    provenance: "/Users/pittcat/Dev/agent_tools/skills/skills/engineering/wayfinder/SKILL.md"
  - skill: grill-with-docs
    invocation_mode: user_invoked
    sub_invocation_policy: absorb-rules-only
    trigger: "逐题确认的同时需要同步锐化术语(问答 + 术语记录组合)"
    inputs: "开放决策主题 + glossary 现状"
    outputs: "规则吸收结果:逐题确认与术语冲突记录合并执行的顺序"
    bound_evidence_level: "同 grilling(E0)+ domain-modeling(E2)两行"
    fallback_if_unavailable: "内置逐题确认 + 术语记录组合流程"
    stop_condition: "只吸收组合规则,不静默子调用 grilling/domain-modeling 之外的流程;术语更新留待用户裁决后"
    provenance: "/Users/pittcat/Dev/agent_tools/skills/skills/engineering/grill-with-docs/SKILL.md"
  - skill: to-spec
    invocation_mode: user_invoked
    sub_invocation_policy: absorb-rules-only
    trigger: "发现完成、需要把已讨论内容综合为交接物时"
    inputs: "author_ready 的 task brief + 已确认决策"
    outputs: "规则吸收结果:综合(而非再次面谈)产出交接材料的纪律;seam 偏好(复用现有、尽量最高、数量最少)"
    bound_evidence_level: "综合产物引用 brief 内既有证据(E0–E3),不新增未验证陈述"
    fallback_if_unavailable: "内置 brief 综合(author_ready 输出作为交接材料,不再面谈)"
    stop_condition: "只吸收综合规则,不发布到 issue tracker、不打 triage 标签;综合不得引入新的未确认决策"
    provenance: "/Users/pittcat/Dev/agent_tools/skills/skills/engineering/to-spec/SKILL.md"
```
<!-- adapters-yaml:end -->

## grilling(model-invoked)

吸收的规则(provenance:`productivity/grilling/SKILL.md`):

- 一次只问一个问题,等用户回答后再继续;同时抛多个问题会造成混乱。
- 每个问题附推荐答案。
- 沿决策树逐分支推进,逐个解决决策之间的依赖。
- **事实能从环境(文件系统、工具等)查到的,先查证,不要问用户;决策属于用户,逐题交给用户等待回答。**
- 用户确认达成共同理解之前不得行动。

discovery 绑定:问答结论记为 E0 证据与决策记录;开放问题清单与路由
(environment vs user)由 `scripts/discovery_transcript.py` 的
`open_questions()` 实现。

## domain-modeling(model-invoked)

吸收的规则(provenance:`engineering/domain-modeling/SKILL.md`):

- 用户使用的术语与 glossary(CONTEXT.md)冲突时,立即指出。
- 模糊/过载术语要锐化为精确的规范术语。
- 讨论域关系时用具体边界场景压测。
- 用户陈述与代码交叉核对;发现矛盾要显式提出。
- 术语解决后即时更新 CONTEXT.md(在 discovery 阶段**降级为记录**:
  冲突写入证据台账,实际修改留给用户裁决之后——discovery 不改目标项目文档)。
- ADR 仅在「难以逆转 + 无背景会意外 + 真实权衡」三条全真时才建议。

discovery 绑定:冲突记为 `terminology_conflict:` 证据(E2,与代码交叉
核对)+ 未决 blocking 决策;裁决前不覆盖任何一侧。

## diagnosing-bugs(model-invoked)

吸收的规则(provenance:`engineering/diagnosing-bugs/SKILL.md`):

- Phase 1 是构建反馈回路:没有针对该 bug 变红的 tight pass/fail 信号,
  任何盯代码都无济于事。
- 构造方式按序尝试:失败测试 → curl/HTTP → CLI fixture → 无头浏览器 →
  replay 捕获 trace → 一次性 harness → property/fuzz → bisect → 差分回路 →
  HITL 脚本(最后手段)。
- **完成判据**:能说出一个**已真实运行过至少一次**的命令,它 red-capable
  (针对用户真实症状、能红能绿)、确定性、秒级、agent 可无人值守运行。
- 没有 red-capable 命令就没有 Phase 2:禁止先读代码建理论/提假设。
- 确实构造不出回路时:停下,列出尝试过的,向用户要复现环境/捕获产物/
  临时埋点授权,**不进入假设阶段**。
- 假设阶段产生 3–5 个可证伪假设(先于任何测试)。

discovery 绑定:red-capable 回路作为 E3 事实;缺失 → `needs_investigation`,
不产生根因决策与执行方案。

## codebase-design(model-invoked)

吸收的规则(provenance:`engineering/codebase-design/SKILL.md`):

- 使用固定词汇:Module / Interface / Implementation / Depth / Seam /
  Adapter / Leverage / Locality;不用 component/service/API/boundary 替代。
- 深模块 = 小接口 + 大量实现;接口是测试面。
- 一个 adapter 是假想的接缝,两个 adapter 才是真实接缝。
- 设计面向可测性:接受依赖而非创建依赖;返回结果而非产生副作用。

discovery 绑定:候选方案设计比较使用该词汇,但比较结论必须回落到证据
台账(E2 源码事实),不得以词汇替代证据。

## triage(user-invoked,禁止静默子调用)

只吸收规则,不执行其 issue-tracker 状态机(provenance:
`engineering/triage/SKILL.md`):

- 收集上下文时做两项检查:(a)**重复实现**——按域概念(而非请求措辞)
  搜索已有实现并报告搜索位置;发现即"已实现"结论。(b)**历史拒绝**——
  读取拒绝记录库,浮出相似的历史拒绝。
- 先给出分类与状态建议和理由,**等待方向**再行动。
- **验证声明**:bug 要按报告者步骤复现;验证不充分的请求是强烈的
  needs-info 信号。
- 需要细化时才进入 grilling/domain-modeling,产出 agent-ready brief。

discovery 绑定:查重/历史拒绝结论作为 E1/E2 证据;bug 声明验证(复现)
为 E3;无法验证 → `needs_investigation`。

## wayfinder(user-invoked,禁止静默子调用)

只吸收规则,不创建 tracker map/ticket(provenance:
`engineering/wayfinder/SKILL.md`):

- **plan, don't do**:每个 ticket 解决一个决策;路径清晰即完成,
  "想直接动手"通常说明已到地图边缘该交接。
- **fog of war**:无法现在精确表述的问题**不强行拆成 ticket**,记入
  "Not yet specified";fog 或 ticket 的判据是"现在能否精确表述问题",
  而不是"现在能否回答"。
- **HITL vs AFK**:需要用户拍板的决策只能由用户回答,agent 绝不代答。
- **blocking edge**:决策之间的依赖显式连线,frontier = 开放、未阻塞、
  未被认领的部分。

discovery 绑定:fog → unknowns 清单(`unknown_topics()`);HITL → 用户
路由问题;blocking → 决策顺序(goal 先于 scope 等)。

## grill-with-docs(user-invoked,禁止静默子调用)

吸收的规则(provenance:`engineering/grill-with-docs/SKILL.md`):
该流程 = 一次 grilling 会话 + 同时使用 domain-modeling。discovery 借用
其组合纪律:逐题确认与术语冲突记录**合并推进**,但术语更新仍受
"裁决前不覆盖"约束,也不静默子调用其它流程。

## to-spec(user-invoked,禁止静默子调用)

只吸收规则,不发布 spec 到 issue tracker(provenance:
`engineering/to-spec/SKILL.md`):

- **不再面谈,只综合**已有讨论与代码理解。
- 全程使用项目 glossary 词汇,尊重相关 ADR。
- seam 偏好:复用现有接缝优于新建;尽可能最高层;数量越少越好,理想为 1。
- 交接物结构:问题陈述 / 方案 / 用户故事 / 实现决策 / 测试决策 /
  范围外 / 补充说明;不含易过期的具体文件路径与代码片段。

discovery 绑定:author_ready 的 brief 是综合输入;交接时不再引入新的
未确认决策。
