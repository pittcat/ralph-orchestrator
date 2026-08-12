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

Ralph 已经具备 Accepted Transition、Execution Contract、Recovery Intent、typed resume routing、diagnosis evidence、task verify-then-apply、Hat ACL、typed Verdict、failure class 和部分 preset 级收敛门禁。

对抗性审查后的结论是：初版十项缺口不完整，而且对“隔离”和“验证”的描述曾经过于绝对。当前真正的问题不是“没有任何门禁”，而是这些能力仍按任务、事件、recovery、preset 或单个阶段分散存在，尚未形成跨动作、跨 Hat、跨 preset 的 Gap → Evidence → Decision → Route → Retry → Convergence 闭环。

## Priority Meaning

| 级别 | 含义 |
|---|---|
| P0 | 可能造成错误状态推进、错误完成或不可恢复的不一致，必须先解决。 |
| P1 | 会让路由、验证或重试失真，导致复杂编排成本上升或重复失败。 |
| P2 | 已有局部能力但缺少跨 Hat、跨 preset 或跨 worktree 的通用语义。 |
| P3 | 影响预算、观测、校准和长期演进，不是第一阶段正确性的阻断点。 |

## Gap Register

### P0 — 状态、证据与接受边界

#### GAP-01：没有统一的编排认知状态、来源权威与新鲜度语义

- **当前状态**：`RuntimeStateSnapshot` 主要记录 plan、step、task、wave、git、fix 和 review 等运行状态；`LedgerSnapshot` 也以 workflow、counter、task、policy 和事件状态为主。Runtime snapshot 还是窄投影，且 state projection 可以关闭。
- **源码证据**：`crates/ralph-core/src/runtime_state.rs:35`、`crates/ralph-core/src/runtime_state.rs:56`、`crates/ralph-core/src/state/snapshot.rs:53`、`crates/ralph-core/src/config/state_projection.rs:33`。
- **缺少什么**：没有统一的一等状态来保存 claim、evidence、hypothesis、assumption、unknown、verified、falsified、decision、route reason、producer、输入指纹和证据新鲜度，也没有规定 Ledger、LoopState、任务投影、recovery journal 与 prompt 投影之间的权威顺序。
- **风险**：Ralph 知道某个 Hat 发出了什么事件，但不能稳定回答“这个结论由什么证据证明”“哪些假设已经被排除”“哪些未知阻止接受”“当前证据是否仍适用于这棵代码树”。不同来源或过期 prompt 可能被当成同等权威。
- **需求记录**：建立统一的、可恢复的编排认知状态；Prompt 只能是相关状态的压缩投影，不能成为跨 activation 的事实源。
- **边界**：本缺口只要求统一语义和权威性，不在此文档决定存储方式或序列化格式。

#### GAP-02：状态机推进、Ledger replay 与最终接纳没有被证明为同一个原子边界

- **当前状态**：StateMachine 在 `validate_event` 中会直接修改运行态；事件随后还要经过 state projection、pre-commit 和 AcceptedTransition 相关处理。`LedgerSnapshot` 虽声明包含 state-machine runtime，但 `CommitDelta` 的变体和 `apply_delta` 分支中没有对应的 state-machine 专用持久化变更。
- **源码证据**：`crates/ralph-core/src/state_machine.rs:421`；`crates/ralph-core/src/event_loop/parse_and_emit.rs:1495`；`crates/ralph-core/src/event_loop/parse_and_emit.rs:1557`；`crates/ralph-core/src/state/commit.rs:76`；`crates/ralph-core/src/state/snapshot.rs:335`。
- **已有能力**：`AcceptedTransition` 已提供 durable outbox、commit receipt 和崩溃恢复语义，见 `crates/ralph-core/src/event_loop/accepted_transition.rs:9`。
- **缺少什么**：StateMachine 的语义状态推进没有被证明纳入所有下游检查、durable commit 和 replay 的同一事务边界；尤其缺少“拒绝不改变状态、重启重放后与 live state 相同”的系统级证明。
- **风险**：事件可能已经让状态机前进，随后却被 projection 或 pre-commit 拒绝；或者 live state 与 ledger replay state 分叉，造成“事件未接受但状态已推进”或恢复后错误路由。
- **需求记录**：任何业务状态转换必须在所有接受条件通过后才生效，并且必须可由同一 durable 记录重建；拒绝、提交失败和进程重启不能留下半提交的语义状态。
- **边界**：不要求替换 AcceptedTransition，只要求明确其与 StateMachine、Projection 和 replay 的一致性关系。

#### GAP-03：终态接受没有跨流程统一的硬证据与证据可信度边界

- **当前状态**：Execution Contract 可以检查 payload 字段、task、git 和 test evidence 义务，但 test evidence 的一种模式只是检查 payload 中存在声明字段；`Verdict`、coordinator gate 和 Parallel Forge 的 confidence/coverage 门禁是局部机制。
- **源码证据**：`crates/ralph-core/src/execution_contract/mod.rs:571`、`crates/ralph-core/src/execution_contract/mod.rs:1253`、`crates/ralph-core/src/event_loop/verdict.rs:1`、`crates/ralph-core/src/event_loop/stages/coordinator_decision_gate_stage.rs:1`、`presets/en/parallel-forge.yml:749`。
- **缺少什么**：没有跨 preset 的 claim → evidence → independent evaluator → system gate 统一契约，也没有统一规定哪些 evidence 属于确定性证据、哪些只是 producer assertion；worker confidence、evaluator confidence、confidence gap、evidence strength、coverage、reproducibility 和 critical unknown 也没有成为系统接受条件。
- **风险**：Agent 说“已完成”与系统有可复核证据之间仍存在断层；高置信度自评、存在字段的 payload 或局部 verdict 可能绕过覆盖不足、证据伪造或 critical unknown。
- **需求记录**：重要状态转换必须基于独立验证或确定性证据；自报内容只能作为待评估输入，不能单独推动终态接受。
- **边界**：不在此文档规定 Evaluator 使用的模型、提示词或具体测试命令。

### P1 — 决策、动作、评估、路由与重试

#### GAP-04：没有统一的 Decision Contract 与系统级 Decision Gate

- **当前状态**：代码已有 typed `Verdict`、coordinator decision gate、hard gate、payload consistency 和 preset 局部 confidence gate，但它们的输入、阈值、结果和终态语义不统一。
- **源码证据**：`crates/ralph-core/src/event_loop/verdict.rs:1`；`crates/ralph-core/src/event_loop/stages/coordinator_decision_gate_stage.rs:1`；`crates/ralph-cli/src/loop_runner/hard_gate.rs:240`；`presets/en/parallel-forge.yml:749`。
- **缺少什么**：没有统一输出 `PASS / RETRY / INVESTIGATE / ESCALATE / ABORT` 的 Decision Contract，也没有系统拥有的 failed metrics、critical unknown、route reason 和 threshold profile。
- **风险**：不同 preset 的 `pass`、`fail`、`pass_with_residuals`、hard gate 和 recovery exhausted 可能代表不同的接受语义；系统无法稳定解释一次决定为什么进入下一状态。
- **需求记录**：所有影响状态推进的决定必须能表达结论、指标、失败门槛、未知项、证据引用和下一路由；局部 gate 不能绕过系统级接受规则。
- **边界**：不要求消灭 preset-specific gate；局部 gate 可以作为系统 gate 的前置证据，但不能替代最终决策契约。

#### GAP-05：没有跨动作的统一 Action Contract

- **当前状态**：task add/ensure 已有 verify-then-apply ticket、fingerprint、原子 claim/consume；HatCommandPolicy 也对 coordinator-only task 和 wave dispatcher 做 ACL。
- **源码证据**：`crates/ralph-cli/src/task_verify_gate.rs:1`；`crates/ralph-cli/src/hat_command_policy.rs:204`。
- **缺少什么**：上述 Action Contract 只覆盖部分 task CLI 和 wave 命令，没有统一描述所有动作的 actor、workspace、目标范围、前置条件、预期副作用、幂等键、确认事件和失败补偿。
- **风险**：任务 mutation 有 OPAC 保护，但文件修改、事件 emit、merge、环境操作和其他编排动作可能仍使用不同的授权、幂等和确认语义；Route 决定“谁来做”却不能同时约束“允许做什么”。
- **需求记录**：所有会改变状态、代码树、事件流或外部环境的动作都必须有可审计的 intent、scope、precondition、apply 和 confirmation 语义。
- **边界**：不把现有 task verify gate 视为无效；它是通用 Action Contract 的已验证局部实现。

#### GAP-06：Independent Evaluator 的独立性、输入盲区和责任链没有被系统强制

- **当前状态**：多个 preset 已有 reviewer、verifier、dimension reviewer 和 auditor，但独立性主要依赖拓扑和 prompt 约定。
- **源码证据**：`presets/en/parallel-forge.yml:1036`；`presets/en/post-merge-converge.yml:1`；`crates/ralph-core/src/diagnosis/responder.rs:630`。
- **缺少什么**：没有统一记录 evaluator 是否读取 producer 的自评、是否使用同一 evidence、是否具备独立执行证据的能力，以及最终结论由哪个 evaluator/规则负责。
- **风险**：两个高 confidence 的 Agent 可能只是复述同一份未经验证的输入；系统把重复意见误当成一致性，把 confirmation bias 误当成独立验证。
- **需求记录**：重要状态转换必须能够审计 evaluator 的输入集合、执行行为、独立性边界和结论责任；高风险决策必须触发真正独立的验证路径。

#### GAP-07：Failure Class 与 Route Reason 没有跨流程统一语义

- **当前状态**：supervisor 已有 `failure_class` 映射，resume routing 也有 typed recovery reason；这些主要服务于 wave slot 或恢复路径。
- **源码证据**：`crates/ralph-core/src/supervisor/worker_outcome.rs:126`；`crates/ralph-core/src/event_loop/resume_routing.rs:150`。
- **缺少什么**：没有统一覆盖 CODE_BUG、TEST_BUG、SPEC_GAP、LOW_EVIDENCE、LOW_COVERAGE、HIGH_UNCERTAINTY、AGENT_DISAGREEMENT、ENV_FAILURE 等编排级分类，并规定每类可去往哪些 Hat。
- **风险**：同一类问题在不同 preset 中使用不同 reason 字符串，指标无法横向比较，动态路由也无法可靠选择 Reproducer、Verifier、Investigator、Arbiter 或 Fixer。
- **需求记录**：Failure Class、Route Reason、允许目标和终态影响必须是可校验的系统语义，而不是自由文本或 preset 私有约定。

#### GAP-08：路由缺少冲突、循环和振荡收敛控制

- **当前状态**：`HatRegistry` 依据 topic subscription、phase 和优先级选择订阅 Hat；Resume routing 主要解决确定性的恢复目标解析。部分 supervisor/preset 有自己的重试或 confidence 退出规则。
- **源码证据**：`crates/ralph-core/src/hat_registry.rs:320`；`crates/ralph-core/src/event_loop/resume_routing.rs:150`。
- **缺少什么**：没有统一检测 route cycle、same-strategy oscillation、A/B evaluator 冲突后的无限往返，以及“路由虽然变化但信息状态没有变化”的假收敛。
- **风险**：相同失败可能在 reviewer/fixer/investigator 之间反复流转；重试预算耗尽前系统没有识别出路由本身无法收敛。
- **需求记录**：静态拓扑只能定义候选范围；最终路由必须依据当前证据和指标选择下一责任 Hat，并对循环、冲突和无进展路由设置可审计的停止或升级条件。
- **边界**：不要求取消 topic subscription；静态拓扑仍然是合法路由候选的约束来源。

#### GAP-09：Retry 有预算，但不证明策略改变或信息增加

- **当前状态**：RecoveryIntent、rejection retry 和 hard gate 已经提供 attempt count、retry key、预算和 exhausted 语义。
- **源码证据**：`crates/ralph-core/src/recovery_intent.rs:42`；`crates/ralph-core/src/loop_runner/hard_gate.rs:240`；`crates/ralph-core/src/loop_runner/hard_gate.rs:321`。
- **缺少什么**：Retry 没有统一记录 previous strategy、hypothesis、rejected hypotheses、new strategy、expected information gain、actual information gain 和 same-strategy 重复判断。
- **风险**：失败可能只触发“再跑一次”，消耗预算却没有新增信息；连续重复实验不能被系统识别为低收益重试。
- **需求记录**：Retry 必须改变策略、假设或证据状态之一；系统必须记录并评估 information gain，并阻止无新增信息的重复重试。
- **边界**：已有针对特定 supervisor slot 或 recovery reason 的预算不视为本缺口的完整解决方案。

### P2 — 复用、覆盖、隔离与多方收敛

#### GAP-10：Artifact 可以交接，但 Evidence 没有通用失效与复用语义

- **当前状态**：artifact-first handoff、payload digest 和 Parallel Forge 的 execution plan digest 已经支持部分产物复用。
- **源码证据**：`docs/explanation/execution-contract-design.md:66`；`docs/explanation/execution-contract-design.md:137`。
- **缺少什么**：没有通用 Evidence Registry 来记录 producer、command、commit/config/environment fingerprint、有效条件和 invalidation rule。
- **风险**：下游可以复用文件或摘要，却无法可靠判断证据是否仍适用于当前代码、配置、环境和依赖。
- **需求记录**：Evidence 和已验证 Decision 必须可跨 Hat 复用，但输入指纹变化时必须失效或要求重新验证。
- **边界**：不在这里决定 Registry 的物理存储或跨 run 保留策略。

#### GAP-11：Convergence 主要存在于特定 preset，尚未成为通用接受语义

- **当前状态**：Parallel Forge 和 Post-Merge Converge 已经实现较强的局部 merge、reconcile、verification 和 regression 语义。
- **源码证据**：`presets/en/parallel-forge.yml:749`；`presets/en/parallel-forge.yml:960`；`docs/plans/2026-08-08-004-feat-multi-plan-scope-resolution-and-convergence-gates-plan.md`。
- **缺少什么**：通用 runtime 没有统一的 convergence receipt，证明任意多 Hat 或多 worktree 流程在 merge 后完成接口、行为、配置、依赖、回归和未知项检查。
- **风险**：某个 preset 中 merge 成功可能仍被错误理解为系统完成；换一个 preset 就重新依赖 prompt 和局部约定。
- **需求记录**：Merge 必须只是中间状态；最终接受必须有可审计的系统级收敛证明。
- **边界**：不要求把所有 preset 立即改成同一套拓扑，只要求最终接受语义可统一表达。

#### GAP-12：隔离主要是路径契约和事后扫描，不是统一硬边界

- **当前状态**：`ephemeral_isolation` 扫描已知临时文件并进行搬迁；事件权限、worktree contract、allowed paths、Hat ACL 和 task OPAC 提供额外约束。
- **源码证据**：`crates/ralph-core/src/ephemeral_isolation.rs:24`；`crates/ralph-core/src/ephemeral_isolation.rs:112`；`crates/ralph-cli/src/operation_guard.rs:1`；`crates/ralph-cli/src/task_verify_gate.rs:1`。
- **缺少什么**：没有看到覆盖所有 Agent 子进程与所有写入动作的统一、继承式文件系统 deny boundary，能够阻止 symlink escape、绝对路径越界写入和访问其他 worktree。当前强保护集中在特定命令和事后扫描，不能等同于全局 sandbox。
- **风险**：Agent 可能绕过事件协议直接写入不属于当前 workspace 的内容；事后扫描无法阻止已经发生的破坏性修改。
- **需求记录**：高风险写操作必须有可执行的 workspace 权限边界；无法提供硬边界时必须显式降级并阻止不允许的风险级别。
- **边界**：不在这里指定操作系统沙箱、容器或具体权限实现。

#### GAP-13：Evidence/Decision Contract 的 preset 覆盖率与启用状态不可作为系统接受前提

- **当前状态**：Execution Contract 默认 `enabled: false`；contract completeness 在未启用时是 vacuous；局部门禁是否启用由 preset 和配置决定。
- **源码证据**：`crates/ralph-core/src/config/execution_contracts.rs:1`；`crates/ralph-core/src/contract_completeness.rs:1`；`crates/ralph-core/src/config/loop_config.rs:460`。
- **缺少什么**：没有全局覆盖清单回答“当前 workflow 的哪些 producer、terminal event、merge boundary 和 verifier 已受统一 contract 保护”，也没有把关键风险级别与 gate 未启用绑定。
- **风险**：同一仓库中某个 preset 的证据门禁很强，另一个 preset 却可以 passthrough；用户看到“系统支持 contract”并不等于当前运行真的受 contract 保护。
- **需求记录**：运行前和终态接受时都必须能审计 contract coverage、enabled/passthrough 状态与 risk profile；关键风险下缺少 gate 必须阻断或显式升级。

#### GAP-14：Merge 边界没有统一的证据失效与 fan-in 责任语义

- **当前状态**：Parallel Forge 和 Post-Merge Converge 已经有局部的 merge、reconcile、verification 和 regression 流程。
- **源码证据**：`presets/en/parallel-forge.yml:749`；`presets/en/parallel-forge.yml:960`；`docs/plans/2026-08-08-004-feat-multi-plan-scope-resolution-and-convergence-gates-plan.md`。
- **缺少什么**：没有通用规定 merge 后哪些 pre-merge evidence 自动失效、哪些可以复用、fan-in 是否已收齐、由谁签发 convergence receipt，以及 merge 后冲突如何回溯到责任 Hat。
- **风险**：局部 worktree 的绿色测试或 review 结论可能被错误地当成最终代码树的系统证据；merge 改变输入后仍沿用旧结论。
- **需求记录**：合并必须触发证据适用性重算；最终接受必须绑定最终代码树、完整 fan-in、integration verification、regression 与 unresolved unknowns。

### P3 — 预算、可观测性与长期校准

#### GAP-15：预算是多套局部计数，没有信息收益和风险维度

- **当前状态**：Ledger、activation、recovery、retry、timeout 和 cost 各自维护局部预算或计数。
- **源码证据**：`crates/ralph-core/src/state/snapshot.rs:53`；`crates/ralph-core/src/recovery_intent.rs:180`；`crates/ralph-cli/src/loop_runner/hard_gate.rs:240`。
- **缺少什么**：没有统一比较 attempts、time、cost、tokens、tool calls、evidence collection 和 evaluator capacity 的预算模型。
- **风险**：系统无法判断“继续收集证据的成本是否值得”，也无法基于风险动态提高或降低调查预算。
- **需求记录**：预算必须可审计，并能与 risk、information gain 和最终决策结果关联。
- **边界**：不在本需求中固定成本模型或具体阈值。

#### GAP-16：指标没有运行校准闭环

- **当前状态**：diagnosis responder 已经记录部分 accepted event evidence 和 metric-specific recovery 结果，但这些指标主要用于局部运行恢复。
- **源码证据**：`crates/ralph-core/src/diagnosis/responder.rs:62`；`crates/ralph-core/src/diagnosis/responder.rs:630`。
- **缺少什么**：没有跨 run 保存 decision metric 的实际结果，用于分析 false-pass、false-block、无效 retry、worker/evaluator 偏差和不同 risk threshold 的表现。
- **风险**：阈值会长期依赖经验或 prompt 文案，无法知道系统是在过早接受还是过度升级。
- **需求记录**：决策指标和阈值结果必须可观测、可回放、可统计，并支持后续校准；指标不能只作为报告展示字段。
- **边界**：校准数据用于改进阈值，不允许反向覆盖单次运行中的确定性硬门。

## Gap Priority Summary

| 优先级 | Gap | 核心问题 |
|---|---|---|
| P0 | GAP-01、GAP-02、GAP-03 | 认知状态、证据可信度和最终接受边界尚未统一，可能造成错误推进或错误完成。 |
| P1 | GAP-04～GAP-09 | Decision、Action、Evaluator、Route 和 Retry 不能跨流程形成证据驱动闭环。 |
| P2 | GAP-10～GAP-14 | 复用、覆盖、隔离和收敛存在局部实现，但缺少通用系统语义。 |
| P3 | GAP-15、GAP-16 | 预算、指标和阈值缺少风险关联与长期校准能力。 |

## Existing Strengths Not to Reclassify as Gaps

- `AcceptedTransition` 已经提供业务事件的 durable commit、outbox 和 crash recovery 基础。
- `Execution Contract` 已经提供 payload、task、git 和部分 test evidence 的完成义务。
- `Recovery Intent` 和 `resume_routing` 已经提供 typed recovery target、retry key、预算和 fail-closed 路由。
- `task_verify_gate` 已经为 task add/ensure 提供 verify-then-apply、payload fingerprint 和原子 claim/consume；`HatCommandPolicy` 已经提供部分 role ACL。
- `Verdict`、coordinator decision gate、failure class、hard gate、Parallel Forge 和 Post-Merge Converge 已经提供局部的决策、分类、覆盖和收敛能力。
- `diagnosis` 已经具备 accepted event evidence、recovery metrics 和局部自愈判断。
- `parallel-forge`、`post-merge-converge` 已经包含局部的 evidence gate 和 merge 后验证模式，但不能替代跨 preset 的统一契约。

这些能力是后续补 Gap 时应复用的现有基础，不应被重新描述成从零建设。

## Requirements Boundary

### 本文要记录的内容

- 当前实现与目标编排模型之间的差距。
- 每个 Gap 的源码事实、风险、优先级和需求方向。
- 哪些已有机制可以作为补 Gap 的基础。
- 后续规划必须覆盖的重启/replay、拒绝原子性、独立验证、动态路由、Retry information gain、证据失效和 merge 后收敛问题。
- 后续规划必须额外覆盖的 Action Contract、Decision Contract、failure taxonomy、route cycle、gate coverage 和 evaluator independence 问题。

### 本文不记录的内容

- 具体 Rust 模块拆分、数据库或序列化方案。
- 具体 CLI 参数、preset YAML 结构和事件字段设计。
- 具体实施 Unit、TDD 顺序、迁移批次或提交拆分。
- 把所有 Gap 一次性实现的承诺。

## Evidence Basis

本记录基于当前源码审计，重点依据 `crates/ralph-core/src/runtime_state.rs`、`crates/ralph-core/src/state_machine.rs`、`crates/ralph-core/src/state/commit.rs`、`crates/ralph-core/src/state/snapshot.rs`、`crates/ralph-core/src/event_loop/parse_and_emit.rs`、`crates/ralph-core/src/event_loop/accepted_transition.rs`、`crates/ralph-core/src/execution_contract`、`crates/ralph-core/src/contract_completeness.rs`、`crates/ralph-core/src/recovery_intent.rs`、`crates/ralph-core/src/event_loop/resume_routing.rs`、`crates/ralph-core/src/event_loop/verdict.rs`、`crates/ralph-core/src/diagnosis/responder.rs`、`crates/ralph-core/src/ephemeral_isolation.rs`、`crates/ralph-cli/src/task_verify_gate.rs`、`crates/ralph-cli/src/hat_command_policy.rs`、`docs/explanation/execution-contract-design.md` 和 `presets/en/parallel-forge.yml`。

其中 GAP-02 的核心风险应在后续规划阶段增加 restart/replay 和下游拒绝场景验证；本需求文档不把尚未通过该验证的推断写成已修复事实。
