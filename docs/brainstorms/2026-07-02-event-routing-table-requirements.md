---
date: 2026-07-02
topic: event-routing-table
---

# 事件路由表(Event Routing Table)

> 面向 `ce-plan` 的需求文档。这是一次技术/架构型 brainstorm,因此包含机制层细节;但不锁定具体文件格式、字段 schema、函数签名等实现细节(留给规划阶段)。

## Problem Frame

当前 orchestrator 的"下一跳"(交接给哪个 hat)是**隐式算出来的**:由「事件订阅关系 + isolated 轮询 + WAC-U5 优先级抢占」三套启发式拼接而成,没有一个显式、可判定、单一权威的地方回答"当前在哪一步、发生了什么事件、接下来该谁上、什么条件下走哪条岔路"。

这套隐式选路已经反复出问题——最近一次(2026-07-01 e2e 跑 `ce-executor-serial`):validator 验收 step-01 后,本该 coordinator 派 step-02,却因为 executor 队列里一条无关的残留 `task.resume` 骗过了优先级抢占,executor 被跳着选中、自己建 task 自己干,coordinator 被跳过;同时 `mechanism.flow` 在配置合并时被悄悄丢弃,越界护栏(`FlowStepScopeStage`)空转,无人拦截。根因分类为机制缺陷(编排 preset 本身正确)。

拓扑其实**早已在 preset 里声明清楚**(谁连谁、什么算结束、怎么进行下一跳),只是散落在多处、用得不一致。本需求要把"下一跳"从"猜"变成"照一张从 preset 生成的路由表走",并在交接前后用同一张表做导航与校验。

受影响面:所有多 hat preset 的运行可靠性;preset 作者的可观测性与调试体验;以及"配置被悄悄丢失还无人察觉"这一类静默退化。

---

## Actors

- A1. Runtime 调度器:在每次交接时决定"下一跳"、并在 emit 时校验合法性。**路由的最终权威。**
- A2. Hat / Agent:干活并产出一个终态事件(outcome)。**只决定"产出哪个事件",不决定"下一跳是谁"。**
- A3. Preset(SSOT):事件拓扑与流程守卫的单一事实源(`presets/schemas/<name>.yml` + `presets/en/<name>.yml`)。
- A4. 路由表生成器:从 preset 派生出人类可读的路由表产物,并提供一致性校验。

---

## Key Flows

- F1. 生成路由表
  - **Trigger:** 作者改完 preset,运行生成命令(如 `ralph route build`)。
  - **Actors:** A3, A4
  - **Steps:** 读 preset SSOT → 派生边集合(起点 = step+from_hat+终态事件;下一跳 = 订阅该事件的 consumer;守卫 = flow 的 terminal_when/on_partial/emit_when;终态 = terminal_emits)→ 写出可读、可 diff 的路由表产物。
  - **Outcome:** 仓库里有一份与 preset 一致、可 review 的路由表文件。
  - **Covered by:** R1, R2, R3

- F2. 运行时照表交接 + 校验
  - **Trigger:** 一个 hat 干完、emit 一个终态事件。
  - **Actors:** A1, A2
  - **Steps:** 执行前加载路由表 → agent 产出终态事件 → runtime 查表:(a) 该 emit 在当前 step 是否合法(校验),(b) 下一跳是谁(选路,可能多个)→ 激活下一跳。
  - **Outcome:** 交接确定、可解释;越界 emit 被拦。
  - **Covered by:** R5, R6, R7, R8

- F3. 影子先行 → 稳了接管
  - **Trigger:** 路由表机制就绪,但尚未成为运行时权威。
  - **Actors:** A1
  - **Steps:** 影子期——表照常加载并算出"下一跳该谁",但**不作数**,真正选路仍由现有逻辑执行,系统记录两边答案并比对分歧 → 真实运行验证表判得对 → 翻转开关,表正式接管选路,旧启发式退休。
  - **Outcome:** 在不影响线上的前提下,用真实数据证明表可靠后再夺权;可回滚。
  - **Covered by:** R11, R12

- F4. 漂移检测
  - **Trigger:** CI / preset_lint 运行。
  - **Actors:** A4
  - **Steps:** 从当前 preset 重新生成一遍路由表 → 与仓库里 committed 的产物比对 → 不一致即报错。
  - **Outcome:** 路由表永远不会和 preset 分叉(不产生"第二份需手工同步的事实源")。
  - **Covered by:** R3, R4

---

## Requirements

**生成与一致性(路由表 = 派生产物,非手写源)**
- R1. 路由表必须**从 preset 自动生成**,不得手工编写;作者只维护 preset。
- R2. 生成产物必须是**人类可读、可 diff、可 review** 的文件,能一眼看清该 preset 的完整流转拓扑。
- R3. 必须提供**一致性校验**:重新从 preset 生成与 committed 产物比对,不一致即失败(接入 CI / preset_lint)。
- R4. 生成应基于**生效后的合并配置**(preset + 用户 `ralph.yml` merge 之后),以便"配置在合并中被丢弃"(如本次 `mechanism.flow` 被丢)会表现为路由表缺边/异常,当场暴露而非静默退化。

**运行时选路与校验(runtime 权威)**
- R5. 执行前必须**加载**路由表。
- R6. 交接时的"下一跳"必须由 runtime **照表确定性判定**,取代现有轮询 + 优先级抢占的启发式(对确定性/唯一消费者的边)。
- R7. emit 时必须用**同一张表校验**该终态事件在当前 step 是否合法(承接被关掉的 `FlowStepScopeStage` 职责)。
- R8. agent 产出的终态事件若**不在表内合法出口**中,必须走 backpressure(拒绝 + 反馈 / 恢复路径),不得让未知事件卡死或静默错路。

**Agent 导航(agent 只导航,不掌舵)**
- R9. 表可向 agent 注入**导航信息**("你现在是 hat X @ step S,合法出口 = {…},各自路由到谁"),但 agent **只决定产出哪个终态事件**,下一跳由 runtime 决定。

**Fan-out(并发)**
- R10. 路由表必须支持**一对多下一跳**(wave / 并行 review 维度)及其 **join 条件**(如 N 个维度都 done 才进入下一步)。对真正并发的 fan-out 保留轮询作为回退。

**上线切换(影子先行)**
- R11. 必须支持**影子模式**:表加载并计算决策但不作数,记录"表的答案 vs 现有逻辑的答案"的分歧。
- R12. 必须能从影子**安全翻转为权威**(开关/opt-in,可按 preset 逐个放开,可回滚)。

**可观测**
- R13. 每次选路/校验决策必须**留痕**(命中哪条边、下一跳、守卫结果),便于事后复盘,避免再次靠考古 events 文件定位。

---

## Acceptance Examples

- AE1. **Covers R6, R8.** 给定 executor 队列里有一条残留 `task.resume`、coordinator 队列有 `test.passed(step-01)`,当 runtime 照表选路时,下一跳必须是 coordinator(发 `work.ready(step-02)`),而非被残留事件带偏选中 executor。
- AE2. **Covers R3, R4.** 给定 preset 的 `mechanism.flow` 在合并路径被丢弃,当加载/生成路由表时,必须表现为缺边/校验失败并报警,而不是静默退化为"全放行"。
- AE3. **Covers R3.** 给定作者改了 preset 的 hat 拓扑但没重新生成路由表,当跑 CI/lint 时,一致性校验必须失败。
- AE4. **Covers R8.** 给定 agent 在某 step emit 了一个该 step 不允许的终态事件,当 runtime 校验时,必须拒绝并给出 backpressure 反馈,而非放行或死锁。
- AE5. **Covers R11.** 给定影子模式开启,当表的下一跳判定与现有逻辑不一致时,必须记录一条分歧日志,且**实际交接仍由现有逻辑执行**。

---

## Success Criteria

- 人的判断:同类"下一步走错/被跳过"的事故不再复现;交接可解释,走错一步当场可见(表上没有这条边)。
- 可观测:preset 作者能打开路由表文件直接读懂流转;运行日志能还原每次选路决策。
- 一致性:路由表与 preset 永不分叉(漂移即 CI 失败),不新增"手工同步的第二份真相"。
- 下游交接:`ce-plan` 拿到本文档即可规划,无需再发明产品行为、范围或成败标准。

---

## Scope Boundaries

- 不手写路由表源文件(违背 R1);路由表始终是生成产物。
- 不让 agent/LLM 决定"下一跳是谁"(违背 R9);agent 只决定产出哪个事件。
- 不改 preset 的编排语义;本功能只改"如何依据 preset 选路/校验",不改拓扑本身。
- 不承诺替换所有轮询:真正并发的 fan-out 仍可保留轮询回退(见 R10)。
- 不在本功能内治理"恢复类事件在业务队列的生命周期(TTL)"——记为独立后续(见 Dependencies)。

### Deferred to Follow-Up Work

- 恢复类事件(如 `task.resume`)的 TTL / "激活即消费或过期"语义:另行处理(P3)。
- 把现有 `state_machine.rs`(校验)、`handoff_index`(邻接)、`mechanism.flow`(守卫)在代码层完全收敛为单一路由抽象:可分阶段,不必一次到位。

---

## Key Decisions

- 路由表是**生成物 + 一致性门**,不是手写源(类比 `Cargo.lock`/编译产物):既给作者一个"看得见、能加载"的文件,又不背上手工对账的债。
- **Runtime 权威,agent 只导航**:agent proposes outcome(发事件),runtime disposes route(照表选路 + 校验)。把路由权交给不确定的 LLM 正是本次事故的同类风险,明确排除。
- **一张表,两个消费点**:执行前(注入 agent 导航)+ emit 时(校验 + 选路)查同一张表,统一现在割裂的 `FlowStepScopeStage`(emit 校验)与 `next_hat`/轮询(选路)。
- **从生效配置生成**:顺带把"配置在合并中被丢弃"变成显式可见的故障(呼应本次 `mechanism.flow` 被丢的 bug)。
- **影子先行(方案 B)**:选路是全局命门,先用真实运行数据证明表判得对,再翻转为权威;可按 preset 逐个放开、可回滚。

---

## Dependencies / Assumptions

- 依赖已定的战术修复先行落地(`docs/plans/2026-07-02-001-fix-hat-routing-next-hop-plan.md`):Fix A(选路谓词)止血、Fix C(配置保真)恢复护栏。路由表是这两者的**战略替代/收敛**——Fix A 最终会被"照表选路"取代。
- 可复用的现有件:`mechanism.flow`(守卫来源)、hat `triggers/publishes` + handoff index(邻接来源)、`state_machine.rs`(已在做终态/实例生命周期校验,可扩展承接 R7)、`flow_lifecycle.rs`(wave 生命周期/deadline,可支撑 R10 的 join)。
- 假设(需规划阶段核实):`ce-executor-serial` 的"每个事件唯一订阅者"约束成立 → 确定性单一下一跳可行;fan-out 场景(wave / 并行 review)是需要显式支持的少数分支。

---

## Outstanding Questions

### Resolve Before Planning

- (无)核心产品决策已在本次讨论锁定。

### Deferred to Planning

- [Affects R2][Technical] 路由表产物的**格式与落盘位置**(committed 于 `presets/<name>.routes.*` 作为 golden,是否同时在 `.ralph/` dump 一份"本次生效版"用于对照)。
- [Affects R1][Technical] 从 preset 到边集合的**精确派生规则**(step×from_hat×topic 如何枚举,守卫如何从 `terminal_when`/`on_partial`/`emit_when` 映射为边上条件)。
- [Affects R10][Technical] **fan-out 与 join** 的表达与运行时判定(集合下一跳 + "全部完成才前进"的聚合条件)。
- [Affects R6][Needs research] 照表选路如何与现有 `next_hat`/`EventBus` 轮询 + 游标共存(确定性边走表、并发边走轮询的边界)。
- [Affects R11][Technical] 影子模式**分歧如何记录与比对**(记录点、比对口径、落到哪个诊断流)。
- [Affects R8][Technical] 未知/越界 emit 的 **backpressure 具体策略**(拒绝 + task.resume vs 路由到恢复 hat)。

---

## Next Steps

- -> `/ce-plan` 做结构化实施规划(建议分阶段:先生成器 + 一致性门 + 影子;再翻转权威 + emit 校验收敛)。
