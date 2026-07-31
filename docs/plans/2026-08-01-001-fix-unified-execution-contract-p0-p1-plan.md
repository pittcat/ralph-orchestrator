---
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
title: "fix: 收敛 unified execution contract 的真实 P0/P1 缺陷"
date: 2026-08-01
type: fix
origin: .ralph/review/2026-07-30-004-refactor-unified-execution-contract-plan/fix-plan.md
baseline: e323f48e
---

# fix: 收敛 unified execution contract 的真实 P0/P1 缺陷

## 0. 计划状态

**READY**。当前基线为 `e323f48e`（`pittcat-dev`）。调查覆盖原 fix-plan 的 1 个 P0 与 6 个 P1 finding、其父提交范围 `57b2e80..cfaf03dc`、以及当前基线在其后的 outbox 修复提交。

已执行的只读验证：源码/测试路径检索、`git diff --stat 57b2e80..cfaf03dc`、相关调用点检索、`git log -S` 对 FU-02 的后续修复追踪、配置与 preset schema 读取。

未执行测试、构建或 lint；本次 `ce-plan` 只负责调查与计划，不把未执行命令伪装成验证结果。

原 fix-plan 中 FU-02（`correctness-01`）不再是当前缺陷：`e323f48e` 已在 `AcceptedTransition::ack` 中把同一 outbox lock 覆盖到 read-modify-write-rename 全过程。因此它被明确放弃，不进入本计划。FU-07～FU-14 是 P2，也不进入本计划。FU-04 的 P1 测试缺口需要修复仍存在的 fail-open ingress 根因；该根因作为 U3 的实现范围纳入，而不是留下一个无法独立验收的 fixture-only 单元。

---

## 1. 功能目标

业务目标是让 parallel-forge 的执行计划、task/wave 派生、agent task 能力、事件策略和 Accepted Transition 使用单一且真实的运行时契约。调用方包括 planner/agent 发出的 `forge.plan.ready`、ralph CLI 的 agent-context task/wave/emit 命令、EventLoop synthetic ingress，以及 replay/outbox 维护路径。

当前行为是：`forge.plan.ready` schema 仍把 `unit_tasks` 作为 planner payload；CLI 仍执行 `check_forge_plan_ready_disk_consistency`；task projector 仍从 payload specs 投影；canonicalizer 与 handoff verifier 仅有模块内测试而无生产调用方。task prompt 以 `owner_hat_id` 推导 actionable，contract compiler 没有三语义 TaskCapability，task/wave CLI 也没有消费同一能力评估器。fail-close scenario 仍断言 `forge.plan.blocked` 不进入 accepted authority。identity tuple 在两个提交路径重复推导。compiler 与旧 event policy 对 deny/glob/Observe 语义分叉。

目标行为是：

- `forge.plan.ready` 只携带受验证的 artifact reference/identity/digest；runtime 从该 artifact 原子派生 task/wave DAG。
- agent-context 的 task lifecycle、execution ownership、actionable-now 由同一 execution contract 派生，prompt、task CLI、wave CLI 不再各自判断。
- synthetic `forge.plan.blocked` 和业务 ingress 在 contract 已启用时只能通过 Accepted Transition；durable commit 失败时不再 direct publish，loop 停止且 accepted ledger 不产生半条记录。
- accepted transition identity 只有一个推导入口；emit deny 规则只有一个语义来源，并保留当前 event policy 的 glob、Observe 与 violation-action 行为。
- fail-close BDD 能在真实 EventLoop 与 accepted ledger 中观察到恰好一次的 blocked transition；artifact-first BDD 能观察到非空 canonical task/wave。

输入包括 `forge.plan.ready` JSON、workspace 内 execution-plan artifact、hat/context、event policy 与 task store。输出包括 accepted transition/outbox 记录、TaskStore 中的 task/wave、CLI authorization result、prompt actionability 和明确的拒绝/停止错误。状态变化必须保持同一 loop、activation revision/digest、transition identity 与 task ledger 的一致性；失败时不得发布未持久化的业务事件。

兼容要求：保留非-parallel-forge 的 legacy state projection 行为，保留当前 human CLI 与 agent-context 的隔离，保留 event policy 的 `debug.*` glob、Observe warning 和 Enforce `on_violation` 语义；不保留 planner 双写 derived `unit_tasks` 的兼容路径。性能要求为不增加每个 event 的第二次 artifact 读取或第二套 policy 解析。安全要求是 artifact path 必须受 workspace 边界和 digest 校验保护，agent 不得通过 coordinator ownership 获得 execution capability。

本次范围是五个真实 P0/P1 根因：artifact-first handoff、task capability contract、Accepted Transition fail-close 及其 BDD、identity 推导单一化、emit policy 单一真相。非目标是 P2 的 E2E durable-state 扩展、migration matrix、raw EventLoop constructor 收口、ActivationRegistry 重构、命名重构和 hat identity legacy 清理。

已确认假设：`e323f48e` 是当前执行基线；FU-02 已修复；`parallel_forge_handoff` 和 `artifact_canonicalizer` 已存在且可复用。待验证假设仅限执行阶段的精确 helper 名称和现有 scenario harness 参数；进入对应 Unit 前必须以源码与编译错误确认，不能改变上述边界。

---

## 2. 代码库现状与证据

### 2.1 当前实现入口

`crates/ralph-cli/src/commands/emit.rs` 在 emit 入口调用 `check_forge_plan_ready_disk_consistency`；该函数位于 `crates/ralph-cli/src/policy_check.rs`，读取 payload 的 `unit_tasks` 并与磁盘计划比较。`crates/ralph-core/src/state_projector/task.rs` 的 `ensure_task_batch` 解析 `BatchSpec` 并以 projector action 的 payload 创建 task。`crates/ralph-core/src/parallel_forge_handoff.rs` 暴露 artifact path/digest 校验，但当前检索只发现模块自身测试调用；`crates/ralph-core/src/lib.rs` 仅导出模块。

`crates/ralph-core/src/event_loop/mod.rs::prepend_ready_tasks` 以 `task.owner_hat_id` 判断 prompt actionable。`execution_contract/compiler.rs::EffectiveExecutionContract::emit_decision` 是新 emit 判定入口；`event_policy.rs::check_topic_deny_rules` 仍是旧运行时策略入口，二者使用不同的匹配与 mode 语义。`AcceptedTransition::commit_idempotent_with_rollback` 和 `commit_unlocked` 各自构造 payload digest、event identity、transition id。

BDD scenario 入口由 `crates/ralph-core/tests/scenarios.rs` 提供，仓库规则要求使用真实 `run_workflow_guard_scenario`；相关 fixture 是 `crates/ralph-core/tests/scenarios/parallel_forge_task_dispatch_runtime.yml` 与 `parallel_forge_fail_close_runtime.yml`。CLI agent-context 入口位于 `task_cli.rs`、`wave.rs` 和 `hat_command_policy.rs`。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `fix-plan.md`、`synthesized-review.md` | P0 明确指出 schema、CLI 特例、projector 仍接受 payload `unit_tasks`，canonicalizer/handoff 无生产调用方。 | U1 必须把 artifact 校验接到真实 ingress，并删除双写路径。 | 高 |
| E2 | `presets/schemas/parallel-forge.yml` | `unit_tasks.source` 仍是 Planner payload，字段要求仍要求完整 task specs。 | schema 是 U1 的外部契约修改点。 | 高 |
| E3 | `crates/ralph-cli/src/commands/emit.rs`、`policy_check.rs` | `forge.plan.ready` 仍调用 `check_forge_plan_ready_disk_consistency`，函数读取 payload `unit_tasks`。 | U1 必须删除该特例及其测试，而不是保留第二个校验源。 | 高 |
| E4 | `crates/ralph-core/src/state_projector/task.rs` | task projector 仍把 action payload specs 转成 `BatchSpec`，并校验 payload schedule。 | U1 必须让 forge handoff 的 canonical artifact 成为唯一 specs 来源，同时保留非 PF action。 | 高 |
| E5 | `crates/ralph-core/src/parallel_forge_handoff.rs`、`artifact_canonicalizer.rs`、`lib.rs` | 两模块存在并有单元测试，但当前生产调用检索未找到。 | U1 复用现有边界，不新建第三套 canonicalizer。 | 高 |
| E6 | `crates/ralph-core/src/event_loop/mod.rs` | prompt actionable 是 `owner_hat_id == caller`，不是 execution contract capability。 | U2 必须改 prompt projection 的来源。 | 高 |
| E7 | `execution_contract/activation.rs`、`compiler.rs`、`task_cli.rs`、`wave.rs`、`hat_command_policy.rs` | 未发现 administration/execution-owner/actionable-now 的统一 TaskCapability API；task/wave CLI 未消费它。 | U2 必须新增共享能力评估边界并接入三类调用方。 | 高 |
| E8 | `crates/ralph-core/src/event_loop/mod.rs` synthetic ingress 分支 | contract/ledger 缺失时 direct publish；commit error 时记录 fallback 并继续 publish。 | U3 必须移除 direct-publish escape hatch，并使 durable failure 停止 loop。 | 高 |
| E9 | `parallel_forge_fail_close_runtime.yml` | `expected.events` 排除 `forge.plan.blocked`，注释固定旧 direct-bus 语义。 | U3 的真实 BDD 必须反转为 accepted authority 恰好一次。 | 高 |
| E10 | `accepted_transition.rs` | `commit_unlocked` 与 `commit_idempotent_with_rollback` 都重复计算 digest、identity、transition id；当前 `ack` 已有 W2 lock 修复。 | U4 只做 identity helper 收敛；不重新处理已修复的 ack。 | 高 |
| E11 | `execution_contract/compiler.rs` | compiler 对 deny 使用精确 pair set，并无 glob/mode 处理。 | U5 必须复用旧 event policy 的匹配/决策语义或抽出唯一解析器。 | 高 |
| E12 | `event_policy.rs` | `check_topic_deny_rules` 支持 `debug.*`、Observe→Warn 与 Enforce action。 | U5 的兼容性验收基线。 | 高 |
| E13 | `e323f48e`、`git log -S'hold the exclusive outbox lock'` | FU-02 的 ack lost-update 修复已在当前 HEAD。 | FU-02 从范围中排除，避免重复实现。 | 高 |

### 2.3 受影响范围

生产模块：`parallel_forge_handoff.rs`、`artifact_canonicalizer.rs`、`state_projector/task.rs`、`event_loop/mod.rs`、`event_loop/accepted_transition.rs`、`execution_contract/{compiler.rs,activation.rs}`、`event_policy.rs`、CLI 的 `commands/emit.rs`、`policy_check.rs`、`task_cli.rs`、`wave.rs`、`hat_command_policy.rs`。

测试模块：两个 parallel-forge BDD YAML、`crates/ralph-core/tests/scenarios.rs`、accepted transition 单元测试、compiler/event policy 单元测试、现有 CLI task/wave/emit 测试位置（进入 Unit 时按符号检索确认）。

配置和数据：`presets/schemas/parallel-forge.yml`、parallel-forge preset 中 `forge.plan.ready` schema、workspace 内 execution-plan artifact、TaskStore、accepted transition outbox。未确认的新增文件不作为计划事实。

构建目标：`ralph-core`、`ralph-cli`，以及由最终 workspace gate 覆盖的其它 crates。未发现需要新增依赖、数据库 migration 或外部服务。

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | artifact-first 应扩展现有校验还是保留 payload 双写？ | 保留双写；复用现有 handoff/canonicalizer 并由 runtime 派生 | 删除 `unit_tasks` payload 契约，接线现有 handoff 到 PF ingress，runtime 从 artifact 派生 | E1–E5 | 双写与 P0 目标相反，且存在两个 authority | 0.98 |
| D2 | task capability 放在哪里？ | 各 CLI 独立判断；新增 execution-contract capability 并由各调用方复用 | 在 resolved execution contract 中表达三种 capability，并暴露一个共享 evaluator | E6–E7 | 独立判断无法满足 R6/R7，ownership 不等于 execution | 0.95 |
| D3 | commit 失败如何处理？ | direct publish fallback；返回失败并停止 loop | fail-closed：不 publish 后续业务事件，记录终止原因并停止推进 | E8–E9 | fallback 违反 accepted authority；fixture 已固定相反旧语义 | 0.96 |
| D4 | identity tuple 如何去重？ | 保留重复代码；抽单一私有 helper | 单一 `derive_identity`/等价现有命名 helper，commit 两条路径共用 | E10 | 重复逻辑已是幂等核心；不需改变数据格式 | 0.97 |
| D5 | deny 语义以谁为真相？ | compiler 新语义；event_policy 旧语义；抽共享 resolver 保留旧可观察行为 | 抽共享 resolver，compiler 与 runtime policy 都消费它，保留 glob/Observe/Enforce | E11–E12 | 选 compiler 会破坏已有 glob/mode 测试；保留双实现继续漂移 | 0.93 |
| D6 | FU-02 是否纳入？ | 重复修复；排除 | 排除，当前基线已完成 W2 修复；仅保留回归验证 | E13 | 当前源码不再满足 finding 前提 | 0.99 |

所有执行关键决策均达到 0.85；没有低置信度实施决策。若执行时发现 D1/D3 的真实 ingress 不同于 E3/E8，必须停止并更新证据，不得在 Unit 内自行另造入口。

---

## 4. BDD 行为规格

### Feature: artifact-first parallel-forge plan handoff

  Background:
    Given workspace contains a valid execution-plan artifact
    And the artifact has a canonical identity and digest

  Scenario: planner handoff derives a non-empty task DAG
    Given `forge.plan.ready` contains only the artifact reference and digest
    When the real EventLoop accepts the handoff
    Then the task ledger contains the artifact's canonical task keys and waves
    And a second identical handoff does not create duplicate task identities

  Scenario: planner cannot submit derived task specs in the handoff
    Given `forge.plan.ready` contains `unit_tasks`
    When the agent-context emit path validates it
    Then the handoff is rejected with a non-zero policy result
    And no task or wave is written

### Feature: contract-derived task capabilities

  Scenario: coordinator lifecycle authority does not grant execution ownership
    Given a coordinator hat may manage task lifecycle but does not own the task
    When it evaluates task execution capability
    Then lifecycle administration is allowed and execution ownership is denied

  Scenario: owner sees only actionable-now tasks
    Given a task is owned by the current hat and is ready under the current loop
    When prompt injection and `ralph tools task` evaluate it
    Then both expose the same actionable-now result

  Scenario: human context is not granted agent primitive capabilities
    Given task/wave commands run without agent runtime context
    When the command is evaluated
    Then it follows human CLI policy and does not consume agent-only capability

### Feature: fail-closed accepted ingress

  Scenario: synthetic forge plan blocked is durably accepted once
    Given the contract and state ledger are available
    When the fail-close path emits `forge.plan.blocked`
    Then the accepted ledger contains exactly one transition
    And the EventBus observes the event only after durable commit

  Scenario: durable commit failure stops publication
    Given accepted transition persistence fails
    When the EventLoop handles the synthetic event
    Then no direct EventBus publication occurs
    And the loop stops without partial task advancement

### Feature: deterministic transition identity

  Scenario: first commit and replay derive the same identity
    Given identical loop, activation, revision, source/topic and payload
    When the event is committed twice
    Then both paths use the same transition identity
    And replay creates no second outbox entry or publication

### Feature: one event-policy deny resolver

  Scenario: glob deny and Observe mode preserve existing policy behavior
    Given a `debug.*` deny rule in Observe mode
    When the compiler and runtime policy evaluate `debug.step`
    Then both return the same warning-level decision

  Scenario: Enforce violation action remains authoritative
    Given an exact deny rule and Enforce mode
    When the configured violation action is Block or RejectWithResume
    Then both compiler-facing validation and runtime validation agree on the selected denial

---

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口/层级 | 风险补充 | E2E |
|---|---|---|---|---|
| artifact handoff derives DAG | fixture 断言非空 canonical task/wave、accepted event 与无重复副作用 | `crates/ralph-core/tests/scenarios.rs` + PF fixture，真实 BDD | Characterization 保留非 PF projector | 否 |
| payload derived tasks rejected | `unit_tasks` 被拒且 TaskStore 不变 | CLI emit/policy 集成测试 | 断言无 ledger/task 副作用 | 否 |
| task capabilities | admin/execution/actionable 三结果在 prompt、task、wave 一致 | ralph-core 单元 + ralph-cli 集成 | 带 `RALPH_CURRENT_HAT` 等 env 的污染验收 | 否 |
| fail-close | accepted ledger 恰好一条，commit error 后 bus 无事件 | EventLoop 单元 + BDD | Fault injection：不可写 outbox | 否 |
| identity | 首次提交/replay transition_id 相同且仅一条 outbox | accepted_transition 单元 | Differential characterization：重构前后字段结果一致 | 否 |
| policy resolver | glob、Observe、Enforce action 在两调用方相同 | event_policy/compiler 单元与 CLI emit 集成 | Property table 覆盖 exact/glob/non-match | 否 |

测试必须先形成真实 Red，再实现最小行为。仓库规定的运行命令见第 9 节；不得用裸 `cargo test -p ralph-cli`。

---

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | PF handoff 的 artifact 是唯一 derived-task authority | S1,S2 | task dispatch BDD | handoff/parser | CLI emit + EventLoop | — | E1–E5 |
| R2 | task 三语义由 contract 派生 | S3–S5 | capability integration | evaluator cases | task/wave CLI | — | E6–E7 |
| R3 | Accepted Transition fail-closed | S6,S7 | fail-close BDD | commit error cases | EventLoop ledger/bus | — | E8–E9 |
| R4 | identity 推导唯一且幂等 | S8 | replay test | helper cases | outbox integration | — | E10 |
| R5 | deny policy 单一真相且兼容旧行为 | S9,S10 | policy contract tests | resolver matrix | compiler + CLI emit | — | E11–E12 |
| R6 | 不重复修复已完成 FU-02 | — | existing ack regression | ack lock case | current core suite | — | E13 |

---

## 7. 严格串行开发单元

执行顺序固定：U1 → U2 → U3 → U4 → U5。

### U1：完成 artifact-first forge.plan.ready handoff

1. **Unit 目标**：`forge.plan.ready` 只接受 artifact identity/digest/reference，并由真实 runtime 从 artifact 派生非空 canonical task/wave DAG。
2. **对应需求与 Scenario**：R1；S1、S2；D1；E1–E5。
3. **外部可观察结果**：planner 不再能通过 payload 双写 task；合法 artifact handoff 产生非空 task keys 和 wave；重复 handoff 幂等。
4. **当前行为基线**：schema 要求 `unit_tasks`，CLI 特例读取它，projector 消费它，BDD 断言 `ready_task_keys: []`（E2–E4）。
5. **输入与输出**：输入为 PF artifact reference/identity/digest；输出为 canonical TaskStore/task wave；错误为 path/digest/schema/size 不合法时拒绝；副作用是不合法输入不改 TaskStore/outbox；不变量是 artifact digest、task keys、wave order 一致。
6. **修改位置**：`presets/schemas/parallel-forge.yml` 修改 `forge.plan.ready` 字段契约；`crates/ralph-cli/src/commands/emit.rs` 和 `policy_check.rs` 删除 PF 双写特例及其测试；`crates/ralph-core/src/parallel_forge_handoff.rs`、`artifact_canonicalizer.rs` 接入真实 handoff；`state_projector/task.rs` 增加 PF artifact-to-BatchSpec 的边界并保留普通 action；两个 PF scenario/相关 projector 测试更新。不得修改非 PF task projection 语义。
7. **可依赖能力**：现有 canonicalizer、handoff path boundary、TaskStore lock、真实 workflow scenario harness。
8. **禁止依赖的未来能力**：不得等待 U2 的 task capability；不得实现 E2E durable revision（P2）；不得顺手修 TOCTOU/raw digest P3 residual。
9. **验收测试**：先让 task-dispatch BDD 断言非空 task/wave；再增加 payload `unit_tasks` 拒绝和重复 handoff 幂等。运行 `cargo nextest run -p ralph-core --test scenarios` 与受影响 CLI nextest。
10. **Acceptance Red**：先运行现有 task-dispatch scenario，应看到 `ready_task_keys` 仍为空且缺 artifact 字段；这是目标缺失的有效 Red。schema/CLI 单测若因 fixture 结构错误失败，不算有效 Red，必须先修测试输入。
11. **单元测试拆分**：artifact payload 解码；digest/path rejection；artifact units→BatchSpec；重复 handoff 不重复；payload derived specs 被拒。不得 mock canonicalizer 或 TaskStore 的真实投影。
12. **Red → Green → Refactor**：BDD 非空断言 Red → 接通 artifact 读取/派生 → BDD Green → payload rejection Red → 删除双写路径 → Green → 抽取只供 PF 使用的转换边界 → integration/regression。
13. **最小实现范围**：只改 PF handoff authority、schema、真实投影入口和测试；不新增第二个 artifact format，不兼容 planner `unit_tasks` 双写。
14. **集成验证**：真实 EventLoop、handoff verifier、TaskStore 必须真实；artifact 文件可用临时 workspace；CLI policy parser 可使用稳定 fixture。
15. **风险驱动测试**：Characterization 保留普通 `state_projection`；idempotency 覆盖重复 handoff；fault path 覆盖 digest/path rejection。
16. **回归范围**：PF preset/schema lint、state projector、CLI emit/policy、scenarios、默认非 PF projector；原因是 schema、CLI 入口与 projection source 同时变化。
17. **预期文件变更**：上述已确认生产文件、PF schema、两个已确认 scenario fixture 和现有测试位置；不得新增未调查的模块。
18. **完成标准**：S1/S2、单元/集成/相关回归、schema parity 与 lint 通过；无 derived payload 兼容；Evidence 更新；可独立提交。
19. **停止条件**：发现 PF ingress 不在 `commands/emit.rs`、artifact 不含计划所需字段、真实 harness 不是 `run_workflow_guard_scenario` 或需要新增依赖时停止并重做 D1。
20. **风险与注意事项**：风险是 schema 改动触发 manifest/preset parity；检测为 preset lint/presets 检查；缓解是同步 schema 与所有受影响 fixture；剩余风险是 P3 TOCTOU 不在本单元。

### U2：统一 task capability 并接入 prompt、task、wave

1. **Unit 目标**：同一 resolved execution contract evaluator 同时给出 lifecycle administration、execution ownership、actionable-now，并被 prompt/task/wave 使用。
2. **对应需求与 Scenario**：R2；S3–S5；D2；E6–E7。
3. **外部可观察结果**：coordinator 可管理但不能执行非自有 task；task prompt、task CLI、wave CLI 对同一 context 给出一致结果；human context 不获得 agent primitive。
4. **当前行为基线**：prompt 只比较 owner；未发现 TaskCapability API，task/wave CLI 未接入 contract（E6–E7）。
5. **输入与输出**：输入为 hat context、task owner/status/loop、compiled contract；输出为三个 capability decision；错误为缺失 agent contract 时按 human/legacy policy 处理，不静默授权；副作用为拒绝命令不改变 task/wave。
6. **修改位置**：`execution_contract/activation.rs`/`compiler.rs` 定义 capability 数据与 evaluator；`event_loop/mod.rs` 用 evaluator 投影 prompt；`task_cli.rs`、`wave.rs`、`hat_command_policy.rs` 使用同一 evaluator；对应既有 core/CLI 测试位置。不得改 unrelated ACL、任务持久化格式或 U1 artifact authority。
7. **可依赖能力**：U1 已验证的 PF task shape、现有 agent runtime env/context、现有 task lifecycle ACL。
8. **禁止依赖的未来能力**：不得在本单元收 raw EventLoop constructor；不得改变 emit policy resolver（U5）。
9. **验收测试**：分别验证 coordinator-admin/non-owner、owner-ready、human-context；带 `RALPH_CURRENT_HAT` 等环境变量运行 CLI 集成测试。
10. **Acceptance Red**：先运行新增 capability contract test，应因不存在统一 capability/evaluator 或 prompt ownership 结果与预期不符而失败。若失败来自 env fixture 未 scrub，先按仓库 common helper 修正，不算有效 Red。
11. **单元测试拆分**：三个 capability truth-table；status/loop boundary；coordinator admin≠execution；prompt projection；CLI task/wave parity。不得 mock evaluator 本身。
12. **Red → Green → Refactor**：truth-table Red → compiler capability representation → Green；prompt Red → evaluator projection → Green；CLI parity Red → shared call sites → Green；抽共享只读 evaluator → regression。
13. **最小实现范围**：只表达并消费三 capability；错误和权限沿现有 policy error 形式返回；不引入新配置字段或持久化迁移。
14. **集成验证**：真实 contract compile、prompt construction、task/wave CLI policy；外部 agent env 只作为输入 fixture。
15. **风险驱动测试**：state-machine/truth-table 覆盖 owner/status/loop；contract test 覆盖 human vs agent；permission negative cases。
16. **回归范围**：execution_contract、event_loop prompt、task CLI、wave CLI、hat policy、agent env scrub 集成测试；原因是同一 evaluator 横跨这些调用方。
17. **预期文件变更**：`activation.rs`、`compiler.rs`、`event_loop/mod.rs`、`task_cli.rs`、`wave.rs`、`hat_command_policy.rs` 与已确认测试位置。
18. **完成标准**：S3–S5、相关 nextest、clippy/build 通过；无 coordinator execution 越权；可独立提交。
19. **停止条件**：发现 task/wave 实际 authorization 入口不在列出的文件、需要公开 API 兼容层或 capability 缺少 contract 输入时停止。
20. **风险与注意事项**：风险是 human/agent 语义被混合；检测是污染 env 集成测试；缓解是复用 `scrub_agent_runtime_env` 并显式注入 agent context；剩余风险是 P2 raw constructor bypass。

### U3：Accepted Transition fail-close 与真实 BDD

1. **Unit 目标**：移除 synthetic ingress 的 direct-publish fallback，durable commit 失败时停止 loop，并让 fail-close scenario 观察 accepted ledger 恰好一次。
2. **对应需求与 Scenario**：R3；S6、S7；D3；E8–E9。
3. **外部可观察结果**：`forge.plan.blocked` 只有在 durable accepted transition 后可见；outbox 失败时 bus 不收到业务事件，loop 不继续部分推进。
4. **当前行为基线**：`event_loop/mod.rs` 在 contract/ledger 缺失或 commit error 时 direct publish；fixture 明确排除 accepted `forge.plan.blocked`（E8–E9）。
5. **输入与输出**：输入为 synthetic event、contract、ledger；输出为 accepted ledger/outbox 与 bus；错误为 commit failure；状态变化是 loop stopped/terminal failure；不变量是 no publish-before-durable、no duplicate accepted transition。
6. **修改位置**：`crates/ralph-core/src/event_loop/mod.rs` ingress branch；`crates/ralph-core/src/event_loop/disposition.rs`/accepted transition call path only if compiler confirms it owns the error mapping；`parallel_forge_fail_close_runtime.yml` 和真实 scenarios tests。不得重写 ack（E13 已修复），不得把 failure 变成 retry loop。
7. **可依赖能力**：U1 的真实 PF event shape、现有 AcceptedTransition/outbox、scenario runner。
8. **禁止依赖的未来能力**：不得等待 U4 identity helper；不得纳入 P2 E2E。
9. **验收测试**：fail-close fixture 断言 accepted events/ledger 恰好一次；fault-injected commit failure 断言 no bus publish、loop stopped。
10. **Acceptance Red**：现有 fixture 在目标断言下应因 `forge.plan.blocked` 缺席而失败；commit-failure test 应观察当前 fallback publish。非目标的 harness panic 不算有效 Red。
11. **单元测试拆分**：contract-present success；missing durable dependency; commit error; no direct publish; ledger exactly-once. 使用真实 EventBus/ledger，只有 filesystem failure 使用受控 temp path/permission boundary。
12. **Red → Green → Refactor**：BDD authority Red → route synthetic through accepted transition → Green；fault Red → remove fallback/stop loop → Green；抽错误终止路径 → integration/regression。
13. **最小实现范围**：只收敛 ingress 和失败语义；不改变其它 loop termination reason 的命名，不增加 retry。
14. **集成验证**：真实 EventLoop、AcceptedTransition、StateLedger、EventBus；不可写 workspace 作为 fault injection。
15. **风险驱动测试**：Fault Injection、state-machine stop-after-failure、idempotency ledger count。
16. **回归范围**：accepted_transition、event_loop termination、PF fail-close BDD、其它 synthetic ingress scenario；原因是 direct publish 分支是共享入口。
17. **预期文件变更**：`event_loop/mod.rs`、必要的已确认 disposition 文件、PF fail-close fixture、相关测试。
18. **完成标准**：S6/S7 和相关回归通过；无 direct publish fallback；可独立提交。
19. **停止条件**：发现 stop signal 由另一个 state machine 拥有、测试无法观察 bus/ledger 或需要改变持久化格式时停止。
20. **风险与注意事项**：风险是把诊断事件误当业务事件停止；检测为事件分类/回归失败；缓解是只修改 synthetic business ingress 分支；剩余风险是 P2 constructor bypass。

### U4：收敛 accepted transition identity 推导

1. **Unit 目标**：让 commit 与 idempotent commit 共用一个 identity derivation helper，行为和字段格式不变。
2. **对应需求与 Scenario**：R4；S8；D4；E10。
3. **外部可观察结果**：首提交与 replay 的 transition_id 相同，重复提交仍只有一条 outbox 和一次 publish。
4. **当前行为基线**：两个路径重复计算 payload digest、event identity、transition id；当前 ack lock 修复不变。
5. **输入与输出**：输入是五个 identity 字段及 Event；输出是现有 tuple/transition_id；错误语义和持久化格式不变。
6. **修改位置**：`event_loop/accepted_transition.rs` 的 `commit_unlocked` 与 `commit_idempotent_with_rollback` 及同模块测试。不得修改 `compute_transition_id` 的编码格式或 ack。
7. **可依赖能力**：U3 的 accepted ingress；现有 replay tests。
8. **禁止依赖的未来能力**：不得做 outbox schema migration 或 policy 变更。
9. **验收测试**：首提交/replay identity equality、不同 payload/topic/source 不碰撞、outbox count unchanged on replay。
10. **Acceptance Red**：先用结构化 coverage/temporary duplicate guard 确认两个路径未共用 helper；若现有行为测试本已全绿，不以全绿冒充 Red，必须以新 helper-call boundary test 或 mutation-equivalent 证明缺口。
11. **单元测试拆分**：字段边界、same tuple、different field、replay dedup；不 mock hash/ledger。
12. **Red → Green → Refactor**：identity helper boundary Red → 抽 helper并替换两路径 → Green → 删除重复局部 hash → Green → fmt/clippy/regression。
13. **最小实现范围**：纯内部重构，保持所有现有 outputs、errors、locks、timestamps 语义。
14. **集成验证**：accepted_transition tests 与 outbox replay integration。
15. **风险驱动测试**：Differential characterization，比较重构前 documented tuple 与新 helper 的结果；idempotency。
16. **回归范围**：ralph-core accepted_transition、event_loop replay/termination；原因是 identity 被 replay 使用。
17. **预期文件变更**：`crates/ralph-core/src/event_loop/accepted_transition.rs` 与其现有测试位置。
18. **完成标准**：identity outputs 不变、重复代码消失、相关 nextest/clippy 通过。
19. **停止条件**：发现第三个生产 identity builder、字段来源不一致或 helper 会改变 serialized ID 时停止。
20. **风险与注意事项**：风险是微小字段顺序改变导致 replay incompatibility；检测为 differential test；缓解是锁定现有 `compute_transition_id` 输入顺序；剩余风险无新数据迁移。

### U5：统一 emit deny policy resolver

1. **Unit 目标**：compiler 与 runtime event policy 使用一个 resolver，保留旧 glob、Observe、Enforce action 行为，并让 contract emit decision 与 CLI validation 一致。
2. **对应需求与 Scenario**：R5；S9、S10；D5；E11–E12。
3. **外部可观察结果**：相同 hat/topic/policy 在 compiler、EventLoop validation、CLI emit 得到同一允许/拒绝/警告结果。
4. **当前行为基线**：compiler 只把 literal pair 放进 deny set；event policy 支持 glob 和 mode/action，已形成语义分叉。
5. **输入与输出**：输入 `EventPolicyConfig`、hat、topic；输出 shared decision；错误/警告沿现有 `PolicyDecision` 语义；不改变 config schema。
6. **修改位置**：`execution_contract/compiler.rs`、`event_policy.rs`，以及调用 resolver 的 CLI emit/validation 测试。不得删除现有 public policy API，不得改变 unrelated topic whitelist。
7. **可依赖能力**：U2 已验证的 compiled contract consumer；现有 event_policy tests。
8. **禁止依赖的未来能力**：不得纳入 P2 naming cleanup 或 E2E。
9. **验收测试**：exact match、`debug.*` match/non-match、Observe Warn、Enforce Block/RejectWithResume、deny-wins overlap；compiler/runtime/CLI parity。
10. **Acceptance Red**：新增 parity table test 先应显示 compiler literal Deny 与 runtime glob/Observe decision 不一致；若某 case 恰好一致，必须覆盖能暴露现有分叉的 glob+Observe 输入。
11. **单元测试拆分**：resolver matching；mode/action mapping；deny-wins; compiler projection; runtime adapter. 不 mock resolver。
12. **Red → Green → Refactor**：parity Red → shared resolver → Green；删除 compiler duplicate set logic → Green；整理 adapter/test fixtures → regression。
13. **最小实现范围**：只统一 topic deny resolution；保持 terminal/system-control exceptions和旧 policy tests。
14. **集成验证**：compiler compile、EventLoop validation、CLI emit policy check 真实联通；policy fixtures 可固定 config。
15. **风险驱动测试**：Property/table-driven matching for exact/glob/non-match；contract parity。
16. **回归范围**：event_policy 全部 deny tests、execution_contract compiler、CLI emit policy、preset lint；原因是共享 resolver 影响三层。
17. **预期文件变更**：`event_policy.rs`、`execution_contract/compiler.rs`、已确认 CLI emit 测试位置。
18. **完成标准**：S9/S10 parity、旧 policy regression、clippy/build 通过；无第二套 deny parser。
19. **停止条件**：发现 Observe 只适用于一个调用方、shared resolver 需要新公开配置或不能保留 control-topic exception 时停止。
20. **风险与注意事项**：风险是把 runner control topics 纳入 deny；检测为已有 control-topic tests；缓解是保留 `is_system_control_topic` 例外；剩余风险是 P2 contract naming duplication。

---

## 8. Unit 串行依赖图

U1 → U2 → U3 → U4 → U5。

U2 使用 U1 已验证的 canonical task shape；不能交换，因为 capability evaluator 的 actionable-now 输入必须先稳定。U3 使用 U1 的真实 PF event shape；不能交换，因为 BDD 的 accepted event 必须先从新 handoff 进入。U4 在 U3 后执行以避免同时调试 ingress 和 identity；U5 最后执行，因为 U2 的 compiled contract consumer 已稳定，且 policy resolver 会横跨 compiler/runtime/CLI。每个 Unit 只实现自身 Scenario，不提前实现后续行为。

---

## 9. 执行命令清单

以下命令按 Unit 严格串行执行；命令失败不得进入下一步。

| 时机 | 命令 | 目的 | 预期 |
|---|---|---|---|
| U1 | `cargo nextest run -p ralph-core --test scenarios` | 真实 BDD handoff/task projection | PF scenario 与其它 scenarios 通过 |
| U1 | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | schema/preset lint | 无新增 finding |
| U1 | `cargo nextest run -p ralph-core -- preset_lint` | core preset lint | 通过 |
| U1 | `cargo nextest run -p ralph-cli --bin ralph -- presets` | embedded/root parity | 通过 |
| U2 | `cargo nextest run -p ralph-core -- execution_contract` | capability/compiler | 通过 |
| U2 | `cargo nextest run -p ralph-cli -- task` | task CLI | 通过 |
| U2 | `cargo nextest run -p ralph-cli -- wave` | wave CLI | 通过 |
| U3 | `cargo nextest run -p ralph-core -- accepted_transition` | fail-close/outbox | 通过 |
| U3 | `cargo nextest run -p ralph-core --test scenarios` | accepted authority BDD | 通过 |
| U4 | `cargo nextest run -p ralph-core -- accepted_transition` | identity/replay | 通过 |
| U5 | `cargo nextest run -p ralph-core -- emit_decision` | compiler policy | 通过 |
| U5 | `cargo nextest run -p ralph-core -- topic_deny` | legacy policy | 通过 |
| 每 Unit | `cargo clippy --workspace --all-targets -- -D warnings` | lint/type-level regression | 通过 |
| 最终 | `./scripts/run-tests.sh` | 仓库规定的完整门禁 | phase 1/2、doctest 等全部通过 |

涉及 CLI 语法时额外运行对应 `ralph <cmd> --help`；若修改 `crates/ralph-core/data/*.md`，必须运行 `scripts/check-cli-doc-drift.sh`。本计划预期不修改 injected skill guide。

---

## 10. 最终质量门禁

所有 R1–R5 均有 Scenario、验收测试和 U-ID；S1–S10 必须通过；accepted transition、PF BDD、task/wave/emit CLI、policy parity、Characterization 和 fault-injection 覆盖必须通过。必须通过 workspace build、clippy、nextest 规定入口和最终 `./scripts/run-tests.sh`；不得新增 skip/only、削弱断言或无解释更新 snapshot。FU-02 仅验证现有 W2 回归，不重新实现。不得存在未处理 BLOCKED 决策，实际变更不得超出 U1–U5 文件边界。

---

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每个 U-ID 指定行为、入口、Red、最小边界、回归和停止条件 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D6 已决策且均 ≥0.85 |
| 所有文件和接口是否有代码库证据 | 是 | E1–E13；未知位置要求进入 Unit 前检索确认 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1 0.98、D2 0.95、D3 0.96、D4 0.97、D5 0.93、D6 0.99 |
| 是否存在未处理的低置信度假设 | 否 | 仅保留执行时符号确认，不作为架构决策 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 handoff、U2 capability、U3 fail-close、U4 identity、U5 policy |
| 每个 Unit 是否可以独立验证 | 是 | 各自有 Red、测试入口、集成与回归；依赖仅使用前置已验证能力 |
| 每个 Unit 是否有真实 Red | 是 | 明确了现有断言/调用链的预期失败；纯重构 U4 使用 boundary/differential Red |
| 每个 Unit 是否包含回归范围 | 是 | U1–U5 第 16 项分别列出 |
| 是否存在未来 Unit 依赖 | 否 | 只依赖前置 Unit，不提前实现后续能力 |
| 是否存在泛化任务描述 | 否 | 每项均落到真实文件、符号、输入、输出和命令 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | R/S/测试矩阵及 U1–U5 |
| 所有关键决策是否有 Evidence | 是 | D1–D6 均引用 E |
| 计划是否可以严格串行执行 | 是 | 第 8 节固定 U1→U5 |

**Product Contract preservation**：本计划是基于用户指定的 review fix-plan 重新核验后生成的开发计划；未引入新的产品目标，仅删除已修复 FU-02、排除 P2，并把 FU-04 的必要 fail-open 根因纳入 U3。
