---
title: 证据驱动编排缺口需求
type: requirements
date: 2026-08-12
topic: evidence-driven-orchestration-state
artifact_contract: ce-unified-plan/v1
artifact_readiness: requirements-only
product_contract_source: ce-brainstorm
execution: code
---

# 证据驱动编排缺口需求

## Goal Capsule

- **目标**：记录当前 Ralph 复杂编排相对于 Evidence-driven State Machine 的已确认缺口，并为后续规划提供优先级、源码证据和需求边界。
- **文档性质**：这是缺口审计与需求记录，不是实现计划，不规定模块拆分、代码顺序、数据结构或具体迁移方案。
- **产品权威**：当前源码中已经接纳的状态、事件、投影、恢复和证据机制是现状事实；Agent 的自然语言声明不能被当作系统事实。

## Summary

Ralph 已经具备 Accepted Transition、Execution Contract、Recovery Intent、typed resume routing、diagnosis evidence 和部分 preset 级收敛门禁。

主要问题不是“没有任何门禁”，而是这些能力仍按局部机制分散存在，尚未形成统一的 Gap → Evidence → Decision → Route → Retry → Convergence 闭环。

## Priority Meaning

| 级别 | 含义 |
|---|---|
| P0 | 可能造成错误状态推进、错误完成或不可恢复的不一致，必须先解决。 |
| P1 | 会让路由、验证或重试失真，导致复杂编排成本上升或重复失败。 |
| P2 | 已有局部能力但缺少跨 Hat、跨 preset 或跨 worktree 的通用语义。 |
| P3 | 影响预算、观测、校准和长期演进，不是第一阶段正确性的阻断点。 |

## Gap Register

### P0 — 状态正确性与接受边界

#### GAP-01：没有统一的编排认知状态

- **当前状态**：`RuntimeStateSnapshot` 主要记录 plan、step、task、wave、git、fix 和 review 等运行状态；`LedgerSnapshot` 也以 workflow、counter、task、policy 和事件状态为主。
- **源码证据**：`crates/ralph-core/src/runtime_state.rs:35`；`crates/ralph-core/src/state/snapshot.rs:53`。
- **缺少什么**：没有统一的一等状态来保存 claim、evidence、hypothesis、assumption、unknown、verified、falsified、decision 和 route reason。
- **风险**：Ralph 知道某个 Hat 发出了什么事件，但不能稳定回答“这个结论由什么证据证明”“哪些假设已经被排除”“哪些未知阻止接受”。每次新 activation 只能重新从 prompt、artifact 或事件摘要推断。
- **需求记录**：建立统一的、可恢复的编排认知状态；Prompt 只能是相关状态的压缩投影，不能成为跨 activation 的事实源。
- **边界**：本缺口只要求统一语义和权威性，不在此文档决定存储方式或序列化格式。

#### GAP-02：状态机推进与最终接纳不是同一个原子边界

- **当前状态**：StateMachine 在 `validate_event` 中可以直接应用 business transition；之后事件还要经过 state projection、pre-commit 和 accepted transition 相关处理。
- **源码证据**：`crates/ralph-core/src/state_machine.rs:421`；`crates/ralph-core/src/event_loop/parse_and_emit.rs:1495`；`crates/ralph-core/src/event_loop/parse_and_emit.rs:1557`。
- **已有能力**：`AcceptedTransition` 已提供 durable outbox、commit receipt 和崩溃恢复语义，见 `crates/ralph-core/src/accepted_transition.rs:9`。
- **缺少什么**：StateMachine 的语义状态推进没有被明确纳入所有下游检查和 durable commit 的同一事务边界。
- **风险**：事件可能已经让状态机前进，随后却被 projection 或 pre-commit 拒绝，造成“事件未接受但状态已推进”的不一致。
- **需求记录**：任何业务状态转换必须在所有接受条件通过后才生效；拒绝、提交失败和进程重启不能留下半提交的语义状态。
- **边界**：不要求替换 AcceptedTransition，只要求明确其与 StateMachine、Projection 和 replay 的一致性关系。

### P1 — 验证、路由与重试

#### GAP-03：完成证明仍主要依赖 Producer 自报

- **当前状态**：Execution Contract 可以检查 payload 字段、task、git 和 test evidence 义务，但 test evidence 可以只是 payload 中存在声明字段。
- **源码证据**：`crates/ralph-core/src/execution_contract/mod.rs:571`；`crates/ralph-core/src/execution_contract/mod.rs:1253`。
- **已有能力**：部分 preset 已经配置 Reviewer、Verifier 和 evidence gate；`parallel-forge` 也有较强的局部证据约束。
- **缺少什么**：没有跨 preset 的 claim → evidence → independent evaluator → system gate 统一契约，也没有强制保存 worker confidence、evaluator confidence、confidence gap、evidence strength 和 evidence coverage。
- **风险**：Agent 说“已完成”与系统有可复核证据之间仍存在断层；高置信度自评可能绕过覆盖不足或 critical unknown。
- **需求记录**：重要状态转换必须基于独立验证或确定性证据；Worker 可以报告 confidence，但不能批准自己的完成声明。
- **边界**：不在此文档规定 Evaluator 使用的模型、提示词或具体测试命令。

#### GAP-04：路由主要由静态拓扑决定

- **当前状态**：`HatRegistry` 依据 topic subscription、phase 和优先级选择订阅 Hat；Resume routing 主要解决确定性的恢复目标解析。
- **源码证据**：`crates/ralph-core/src/hat_registry.rs:320`；`crates/ralph-core/src/event_loop/resume_routing.rs:150`。
- **缺少什么**：没有统一的 metric-driven route decision，根据 failure class、coverage、uncertainty、consistency、risk 或 confidence gap 选择 Reproducer、Verifier、Investigator、Arbiter 或 Fixer。
- **风险**：相同失败只能沿 preset 预先写好的 topic 链路前进；路由无法随证据变化，容易把低证据问题直接交给错误角色。
- **需求记录**：静态拓扑只能定义候选范围；最终路由必须能依据当前证据和决策指标选择下一责任 Hat，并记录理由和未解决项。
- **边界**：不要求取消 topic subscription；静态拓扑仍然是合法路由候选的约束来源。

#### GAP-05：Retry 有预算，但不证明策略改变或信息增加

- **当前状态**：RecoveryIntent、rejection retry 和 hard gate 已经提供 attempt count、retry key、预算和 exhausted 语义。
- **源码证据**：`crates/ralph-core/src/recovery_intent.rs:42`；`crates/ralph-core/src/loop_runner/hard_gate.rs:240`；`crates/ralph-core/src/loop_runner/hard_gate.rs:321`。
- **缺少什么**：Retry 没有统一记录 previous strategy、hypothesis、rejected hypotheses、new strategy、expected information gain、actual information gain 和 same-strategy 重复判断。
- **风险**：失败可能只触发“再跑一次”，消耗预算却没有新增信息；连续重复实验不能被系统识别为低收益重试。
- **需求记录**：Retry 必须改变策略、假设或证据状态之一；系统必须记录并评估 information gain，并阻止无新增信息的重复重试。
- **边界**：已有针对特定 supervisor slot 或 recovery reason 的预算不视为本缺口的完整解决方案。

### P2 — 复用、隔离与多方收敛

#### GAP-06：Artifact 可以交接，但 Evidence 没有通用失效与复用语义

- **当前状态**：artifact-first handoff、payload digest 和 Parallel Forge 的 execution plan digest 已经支持部分产物复用。
- **源码证据**：`docs/explanation/execution-contract-design.md:66`；`docs/explanation/execution-contract-design.md:137`。
- **缺少什么**：没有通用 Evidence Registry 来记录 producer、command、commit/config/environment fingerprint、有效条件和 invalidation rule。
- **风险**：下游可以复用文件或摘要，却无法可靠判断证据是否仍适用于当前代码、配置、环境和依赖。
- **需求记录**：Evidence 和已验证 Decision 必须可跨 Hat 复用，但输入指纹变化时必须失效或要求重新验证。
- **边界**：不在这里决定 Registry 的物理存储或跨 run 保留策略。

#### GAP-07：Convergence 主要存在于特定 preset，尚未成为通用接受语义

- **当前状态**：Parallel Forge 和 Post-Merge Converge 已经实现较强的局部 merge、reconcile、verification 和 regression 语义。
- **源码证据**：`presets/en/parallel-forge.yml:749`；`presets/en/parallel-forge.yml:960`；`docs/plans/2026-08-08-004-feat-multi-plan-scope-resolution-and-convergence-gates-plan.md`。
- **缺少什么**：通用 runtime 没有统一的 convergence receipt，证明任意多 Hat 或多 worktree 流程在 merge 后完成接口、行为、配置、依赖、回归和未知项检查。
- **风险**：某个 preset 中 merge 成功可能仍被错误理解为系统完成；换一个 preset 就重新依赖 prompt 和局部约定。
- **需求记录**：Merge 必须只是中间状态；最终接受必须有可审计的系统级收敛证明。
- **边界**：不要求把所有 preset 立即改成同一套拓扑，只要求最终接受语义可统一表达。

#### GAP-08：隔离主要是路径契约和事后扫描，不是统一硬边界

- **当前状态**：`ephemeral_isolation` 扫描已知临时文件并进行搬迁；事件权限、worktree contract 和 allowed paths 提供额外约束。
- **源码证据**：`crates/ralph-core/src/ephemeral_isolation.rs:24`；`crates/ralph-core/src/ephemeral_isolation.rs:112`。
- **缺少什么**：没有看到统一的、由子进程继承的文件系统 deny boundary 来阻止 symlink escape、绝对路径越界写入和访问其他 worktree。
- **风险**：Agent 可能绕过事件协议直接写入不属于当前 workspace 的内容；事后扫描无法阻止已经发生的破坏性修改。
- **需求记录**：高风险写操作必须有可执行的 workspace 权限边界；无法提供硬边界时必须显式降级并阻止不允许的风险级别。
- **边界**：不在这里指定操作系统沙箱、容器或具体权限实现。

### P3 — 预算与长期校准

#### GAP-09：预算是多套局部计数，没有信息收益和风险维度

- **当前状态**：Ledger、activation、recovery、retry、timeout 和 cost 各自维护局部预算或计数。
- **源码证据**：`crates/ralph-core/src/state/snapshot.rs:53`；`crates/ralph-core/src/recovery_intent.rs:180`；`crates/ralph-cli/src/loop_runner/hard_gate.rs:240`。
- **缺少什么**：没有统一比较 attempts、time、cost、tokens、tool calls、evidence collection 和 evaluator capacity 的预算模型。
- **风险**：系统无法判断“继续收集证据的成本是否值得”，也无法基于风险动态提高或降低调查预算。
- **需求记录**：预算必须可审计，并能与 risk、information gain 和最终决策结果关联。
- **边界**：不在本需求中固定成本模型或具体阈值。

#### GAP-10：指标没有运行校准闭环

- **当前状态**：diagnosis responder 已经记录部分 accepted event evidence 和 metric-specific recovery 结果，但这些指标主要用于局部运行恢复。
- **源码证据**：`crates/ralph-core/src/diagnosis/responder.rs:62`；`crates/ralph-core/src/diagnosis/responder.rs:630`。
- **缺少什么**：没有跨 run 保存 decision metric 的实际结果，用于分析 false-pass、false-block、无效 retry、worker/evaluator 偏差和不同 risk threshold 的表现。
- **风险**：阈值会长期依赖经验或 prompt 文案，无法知道系统是在过早接受还是过度升级。
- **需求记录**：决策指标和阈值结果必须可观测、可回放、可统计，并支持后续校准；指标不能只作为报告展示字段。
- **边界**：校准数据用于改进阈值，不允许反向覆盖单次运行中的确定性硬门。

## Gap Priority Summary

| 优先级 | Gap | 核心问题 |
|---|---|---|
| P0 | GAP-01、GAP-02 | 没有统一决策状态，且状态推进可能与最终接纳脱离原子边界。 |
| P1 | GAP-03、GAP-04、GAP-05 | 验证、路由和重试还不能由证据与信息增益闭环驱动。 |
| P2 | GAP-06、GAP-07、GAP-08 | 复用、收敛和隔离存在局部实现，但缺少通用系统语义。 |
| P3 | GAP-09、GAP-10 | 预算、指标和阈值缺少长期校准能力。 |

## Existing Strengths Not to Reclassify as Gaps

- `AcceptedTransition` 已经提供业务事件的 durable commit、outbox 和 crash recovery 基础。
- `Execution Contract` 已经提供 payload、task、git 和部分 test evidence 的完成义务。
- `Recovery Intent` 和 `resume_routing` 已经提供 typed recovery target、retry key、预算和 fail-closed 路由。
- `diagnosis` 已经具备 accepted event evidence、recovery metrics 和局部自愈判断。
- `parallel-forge`、`post-merge-converge` 已经包含局部的 evidence gate 和 merge 后验证模式。

这些能力是后续补 Gap 时应复用的现有基础，不应被重新描述成从零建设。

## Requirements Boundary

### 本文要记录的内容

- 当前实现与目标编排模型之间的差距。
- 每个 Gap 的源码事实、风险、优先级和需求方向。
- 哪些已有机制可以作为补 Gap 的基础。
- 后续规划必须覆盖的重启/replay、拒绝原子性、独立验证、动态路由、Retry information gain、证据失效和 merge 后收敛问题。

### 本文不记录的内容

- 具体 Rust 模块拆分、数据库或序列化方案。
- 具体 CLI 参数、preset YAML 结构和事件字段设计。
- 具体实施 Unit、TDD 顺序、迁移批次或提交拆分。
- 把所有 Gap 一次性实现的承诺。

## Evidence Basis

本记录基于当前源码审计，重点依据 `crates/ralph-core/src/runtime_state.rs`、`crates/ralph-core/src/state_machine.rs`、`crates/ralph-core/src/event_loop/parse_and_emit.rs`、`crates/ralph-core/src/accepted_transition.rs`、`crates/ralph-core/src/execution_contract`、`crates/ralph-core/src/recovery_intent.rs`、`crates/ralph-core/src/event_loop/resume_routing.rs`、`crates/ralph-core/src/diagnosis/responder.rs`、`crates/ralph-core/src/ephemeral_isolation.rs`、`docs/explanation/execution-contract-design.md` 和 `presets/en/parallel-forge.yml`。

其中 GAP-02 的核心风险应在后续规划阶段增加 restart/replay 和下游拒绝场景验证；本需求文档不把尚未通过该验证的推断写成已修复事实。
