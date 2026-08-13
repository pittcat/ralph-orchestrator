---
type: fix
title: "补齐 StateMachine 最终接纳与 Ledger replay 的记账一致性"
status: ready
date: 2026-08-13
origin: docs/brainstorms/2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
---

# GAP-02：StateMachine 最终接纳与 Ledger replay 的记账一致性

本文是可直接交给 Coding Agent 执行的实现计划，不是 Roadmap，也不包含生产代码。严格执行 `Unit 1 → Unit 2 → Unit 3 → Unit 4`，前一个 Unit 未完成完整的 Red → Green → Refactor → Integration → Regression → Close，不得进入下一个 Unit。

## 0. 计划状态

- **状态：READY**
- **当前基线**：分支 `pittcat-dev`，HEAD `43af71ca`（2026-08-13）；该提交已包含 GAP-01、accepted-transition fail-close 以及最新 handoff precheck/worktree baseline 变更。
- **工作树约束**：工作树当前有用户未提交修改，包含 GAP-01、diagnostics、state ledger 等文件。Executor 不得清理、回滚或覆盖这些修改；只在本计划列出的边界内增量修改。
- **调查范围**：StateMachine validator、EventLoop 接纳流水线、state projection、execution contract、AcceptedTransition/outbox、StateLedger/CommitDelta/LedgerSnapshot、LoopState 生命周期初始化、已有 StateMachine/ledger/replay 测试、builtin preset/config/data skill。
- **已执行的只读调查**：在当前 HEAD 重新执行 `rg`/`sed`/`git log`/`git status`/`git diff --check`；再次确认 `EventBus::publish` 的真实签名、`AcceptedTransition` 的 outbox 路径，以及 builtin preset/schema 中没有 `state_machine` 配置。
- **已有可执行证据**：此前在同一分支运行过 `cargo nextest run -p ralph-core -- state_machine`（31 个测试通过）和 `cargo nextest run -p ralph-core -- replay_from_disk`（3 个测试通过）。本计划执行阶段必须在当前修改集合上重新运行相关测试；计划阶段不把未重跑的结果伪装成最终验收。
- **计划阶段未执行**：未修改生产代码，未运行完整 build/lint/全量测试；这些是 Executor 的 Unit 级和最终质量门禁。
- **阻塞项**：无。所有实施关键决策均有当前代码证据，并在下方 Decision Record 中达到 `≥ 0.85`。如果执行时发现 `EventBus::publish`、`AcceptedTransition` 或 `StateLedger` 真实签名与本计划冲突，必须按 Unit 停止条件暂停，不得自行换架构。

### 0.1 2026-08-13 仓库同步校准

- 当前 `acceptance_and_lifecycle.rs` 已将 `StateLedger` 接入为始终启用的 loop state 组件；因此本计划的“显式启用 StateMachine”约束只适用于 StateMachine validator，不能再表述为 StateLedger opt-in。
- `AcceptedTransition` 当前仍只保证 accepted-transitions outbox 写入先于 bus publish；`OutboxEntry` 尚未携带 StateMachine projection，`CommitDelta`/`LedgerSnapshot::apply_delta` 也尚未承载 StateMachine delta。这些仍是 Unit 1/3 的目标，不是现状。
- 最新 `parse_and_emit.rs` 增加了 runtime handoff precheck：`work.done`/`stabilization.done` 在进入最终接纳前可能被 `worktree_handoff.rs` 拒绝。StateMachine projection 必须只从 precheck、workflow、policy、execution contract 等全部 gate 之后的最终 accepted 集合产生；precheck rejection 不得写 StateMachine accepted delta。
- 最新 `event_processing.rs` 在 hat activation 时捕获 worktree baseline，属于前置验收证据，不改变本计划的 ledger 设计；Unit 2/4 的集成测试应覆盖“handoff precheck 拒绝不会污染 StateMachine ledger”。

## 1. 功能目标

### 1.1 业务目标

对于显式启用 `event_loop.state_machine.enabled: true` 的流程，把一次状态机转换的“记账”从当前的进程内直接改写，收敛为一个可审计的结果：

1. 状态机先产生候选状态，不立即污染 live state。
2. 后续 state projection、统一 validation、workflow/emit gate 和 execution contract 仍然有机会拒绝该事件。
3. 只有最终被 EventLoop 接纳的转换才更新 live StateMachine state，并写入 `StateLedger`。
4. AcceptedTransition 的 durable acceptance boundary 失败时，不发布事件，不留下 live StateMachine 半状态；如果 outbox receipt 已经落盘但 ledger projection 尚未完成，该 receipt 明确作为“待补账”凭证，重启时先补齐 ledger 再 hydration，不把它误判成已发布。
5. 新进程从 Ledger replay 后，得到与正常进程最终 accepted state 等价的 StateMachine state。

这解决的是“状态机已经算过，但后面是否真的接受、是否写账、重启后是否恢复没有一个统一记账结果”的问题，不重复建设 OPAC，也不改变 GAP-01 的 `Observation + Unverified` 认知记录。

### 1.2 用户或调用方

- 直接调用方：启用了 StateMachine 的 `EventLoop::process_parse_result` / `process_events_from_jsonl`。
- 持久化调用方：`StateLedger::commit`、`StateLedger::replay_from_disk`。
- 发布调用方：已有 `AcceptedTransition::commit_idempotent` / `commit_idempotent_with_rollback` 和 `disposition::publish_synthetic`。
- 重启调用方：`acceptance_and_lifecycle` 中构造 `StateLedger` 的 loop 初始化路径。
- 非调用方：Agent prompt、OPAC task/wave/emit 命令、GAP-01 cognitive observation、builtin preset 拓扑。

### 1.3 当前行为

- `StateMachineRuntimeState::validate_event(&mut self, ...)` 在 `state_machine.rs` 中直接写 `open_instances`、`closed_instances`、`terminal_observed` 和 `accepted_transition_count`。
- `parse_and_emit.rs` 在后续 workflow guard、record/publish 之前调用它，因此下游拒绝可能已经看到一个被状态机推进过的 live state。
- `LedgerSnapshot` 已有 `state_machine_runtime: Option<StateMachineRuntimeState>` 字段，但 `CommitDelta` 没有 StateMachine 分支，`LedgerSnapshot::apply_delta` 也没有对应分支。
- `LoopState::new` 将 `state_machine_runtime_state` 设为 `None`；生命周期中虽然会构造 `StateLedger`，但没有把 replay 后的 StateMachine snapshot hydration 到 LoopState。
- `StateLedger::new` 会 replay `.ralph/ledger.jsonl`；因此普通 ledger state 能恢复，StateMachine runtime 目前不能从这个账本恢复。
- `AcceptedTransition` 的真实保证是：outbox 写入成功后才调用 `EventBus::publish`；`EventBus::publish` 返回 `Vec<HatId>`，没有 `Result`，所以“发布失败”在当前代码中只能通过 durable outbox 写入失败来故障注入，不能编造一个不存在的 bus publish error。
- builtin preset 和 `presets/schemas` 当前没有 `state_machine` 配置；本缺口是显式启用 StateMachine 的条件性增强，不改变默认流程。

### 1.4 目标行为

- 状态机预检查只修改候选副本；live `LoopState.state_machine_runtime_state` 在最终接纳前保持原值。
- 下游拒绝的事件不产生 StateMachine commit，不改变 live state，不进入 accepted transition outbox，也不发布业务事件。
- 最终接纳的业务/恢复事件产生可 replay 的 `CommitDelta`，并通过现有 AcceptedTransition acceptance boundary 关联 durable receipt；LoopControl/legacy direct path 仍走当前 direct publish 路径，但在 publish 前完成 ledger commit。
- ledger 写入失败时，StateMachine live state 回到原状态，EventBus 不收到该业务事件；错误沿现有 `Result<std::io::Error>` 路径返回，不能静默当成功。
- 重启时先恢复 ledger snapshot，再补齐 durable outbox 中已有但 ledger 尚未 materialize 的 StateMachine acceptance projection，最后 hydration LoopState；同一个 transition 不重复计数、不重复打开实例。
- `terminal_observed` 只在最终接受的 terminal event 上持久化；`terminal_honored` 只在现有 completion checks 通过并执行 `mark_terminal_honored` 后持久化。

### 1.5 行为差异

| 场景 | 当前行为 | 目标行为 |
|---|---|---|
| StateMachine 被接受、后续 gate 拒绝 | 可能已经推进 live state | 不推进 live state，不落 StateMachine commit |
| StateMachine 被接受、Ledger 写失败 | 当前没有 StateMachine ledger 语义 | live state 不变，bus 不发布，明确返回错误 |
| AcceptedTransition outbox 写失败 | 无业务事件发布，但 StateMachine 可能已在前面推进 | 无业务事件发布，StateMachine 不留下 live/durable 半状态 |
| 进程重启 | LoopState 的 StateMachine runtime 从空状态开始 | 从 Ledger/outbox acceptance evidence 恢复等价状态 |
| StateMachine 未启用 | 现有事件流 | 保持现有事件流，不新增文件、commit 或 validation |

### 1.6 本次范围

- StateMachine runtime 的最小可 replay delta。
- StateMachine 候选状态与最终接纳边界的分离。
- StateLedger snapshot/apply/replay 对 StateMachine 的 wiring。
- AcceptedTransition acceptance receipt 与 StateMachine projection 的幂等关联。
- LoopState 启动 hydration、terminal honored 的持久化。
- 失败恢复、重复提交、重启 replay、禁用路径回归。

### 1.7 非目标

- 不重做 OPAC verify/apply/confirm。
- 不改变 task/wave/emit CLI、命令参数或 Agent skill。
- 不把 GAP-01 `KnowledgeRecord` 的 `Observation` 自动变为 `Verified`。
- 不替换 `AcceptedTransition`，只在其已有 durable boundary 上增加 StateMachine projection 关联。
- 不改 builtin preset、preset schema、hat trigger/publish 拓扑。
- 不把 EventBus 改造成可失败 API；当前 `publish` 没有错误返回，这是已确认约束。
- 不把 `last_terminal_rejection` 这类拒绝诊断指纹伪装成 accepted business state。它继续是非权威的运行诊断字段；replay 等价性只比较 accepted transition state 和 terminal honor state。
- 不引入数据库迁移、新 crate 依赖或新的 CLI 命令。

### 1.8 输入、输出与状态变化

- 输入：`Event` 的 topic/payload/source、已启用的 `StateMachineConfig`、当前 StateMachine runtime、后续验证/emit gate 结果、AcceptedTransition identity。
- 输出：原有 `ProcessedEvents` 结果、原有 bus/outbox 行为，以及新增的 StateMachine ledger commit/receipt projection。
- live 状态：只在最终接纳后更新 `LoopState.state_machine_runtime_state`。
- durable 状态：`CommitDelta` 写入 `.ralph/ledger.jsonl`；若事件走 AcceptedTransition，outbox entry 追加可重建该 projection 的可选字段。
- 诊断：原有 `event.state_machine.rejected` 保留；它不作为业务 acceptance 记录。
- 不变量：同一 transition identity 重放不重复增加 `accepted_transition_count`，不重复改变 open/closed instance；reject/commit failure 不产生 accepted StateMachine delta。

### 1.9 错误、兼容、性能、安全与约束

- 错误语义：StateMachine validation reject 仍按当前 finding/diagnostic 语义处理；Ledger/outbox durable failure 进入现有 error/backpressure 路径，不能吞掉；replay repair 失败时必须 fail closed 并给出明确 warning/error，不得假装 live state 已恢复。
- 兼容性：已有 `CommitDelta` JSONL 必须继续 replay；新增字段使用 serde default；旧的 `OutboxEntry` 没有新字段时按“无 StateMachine projection”处理。
- 性能：不在每个普通事件上复制完整 `LedgerSnapshot`；只复制小型 `StateMachineRuntimeState` 候选，并按 batch/transition 产生最小 delta；未启用 StateMachine 路径不得增加 ledger 操作。
- 安全/权限：不扩大 Agent 可见文件或 CLI 权限；Agent 不直接读取新增内部账本字段。
- 已知约束：`StateLedger::commit` 已提供本地 snapshot 回滚；`AcceptedTransition` 已保证 outbox 先于 bus publish；跨文件 crash window 必须依靠“outbox 先写入携带 projection 的 receipt → ledger projection → bus publish → 启动时 repair”收敛，不能声称存在不存在的跨文件事务。Executor 不得改变这个顺序。

### 1.10 已确认与待验证假设

已确认假设：

- A1：StateMachine 仅由显式配置启用；当前 builtin preset 不启用。证据 E7/E8。
- A2：`EventBus::publish` 无失败返回；可测试的 AcceptedTransition 发布前失败是 outbox durable write failure。证据 E5/E6。
- A3：StateLedger 是已有的 replay source，`LedgerSnapshot.state_machine_runtime` 是预留字段，不需要新建第二套 ledger。证据 E2/E3/E4。
- A4：现有 StateMachine 单元调用方依赖 `validate_event` 的返回结构；不改变 `StateMachineDecision` enum 形状，新增 staging 辅助 API。证据 E1/E9。

待验证假设及进入 Unit 前动作：

- H1：AcceptedTransition outbox entry 增加可选 StateMachine projection 字段，不会破坏现有 outbox 消费者。验证：搜索所有 `OutboxEntry` 构造/反序列化消费者，执行 `cargo nextest run -p ralph-core -- accepted_transition` 与 `cargo nextest run -p ralph-cli --test integration_resume`；若存在精确 JSON shape consumer，改为新增可选字段并补 contract test。失败影响：Unit 3 停止并重新比较“outbox 承载 projection”与“ledger-only compensation”方案。
- H2：loop 初始化可以在现有 StateLedger 构造之后安全 hydration，而不改变 policy/task/projector bootstrap 顺序。验证：沿 `acceptance_and_lifecycle.rs` 初始化顺序添加 characterization assertion，运行 `cargo nextest run -p ralph-core -- state_machine` 与 `cargo nextest run -p ralph-core --test replay_light_integration`。失败影响：Unit 4 必须拆出独立 bootstrap adapter，不得在 `LoopState::new` 中临时读取文件。

## 2. 代码库现状与证据

### 2.1 当前实现入口

外部事件入口是 `EventLoop::process_parse_result` / `process_events_from_jsonl`，主状态机检查位于 `crates/ralph-core/src/event_loop/parse_and_emit.rs`。调用链为：

`EventReader/ParseResult` → policy/origin validation → StateMachine `validate_event` → workflow/state projection/unified validation → `validated_events` → final emit gate 形成 `pending_publish` → AcceptedTransition 或 direct `EventBus::publish` → phase authority/ledger tail。

StateMachine 领域实现位于 `crates/ralph-core/src/state_machine.rs`，Ledger 持久化位于 `crates/ralph-core/src/state/commit.rs`、`ledger.rs`、`snapshot.rs`，loop 生命周期初始化位于 `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs` 和 `lifecycle.rs`。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `crates/ralph-core/src/state_machine.rs::StateMachineRuntimeState::validate_event` | 方法接收 `&mut self`，业务转换在 validator 内直接修改 runtime；`StateMachineDecision::Accept` 只返回 instance key/new state | 必须增加候选/staging 入口；不得在 event loop 中继续直接改 live state | 高 |
| E2 | `crates/ralph-core/src/state/commit.rs::CommitDelta` | 当前枚举没有 StateMachine 专用 variant；文件注释要求新状态通过 delta 进入 replay | 新增最小 semantic delta，不新增第二套持久化机制 | 高 |
| E3 | `crates/ralph-core/src/state/snapshot.rs::LedgerSnapshot::state_machine_runtime` 与 `apply_delta` | snapshot 已预留 StateMachine runtime 字段，但 exhaustive `apply_delta` 没有对应分支 | 该字段是现成目标位置，Unit 1 补齐 apply/replay | 高 |
| E4 | `crates/ralph-core/src/state/ledger.rs::StateLedger::new/commit/replay_from_disk` | new 会 replay ledger；commit 失败会恢复 snapshot/commit log；replay 遇损坏返回错误并由上层 cold-start warning | 复用现有原子 ledger 写入与失败回滚，不能另写 StateMachine 文件 | 高 |
| E5 | `crates/ralph-core/src/event_loop/accepted_transition.rs::commit_idempotent_with_rollback` | 已有 validate → materialize → durable outbox → publish 顺序和 rollback closure；重复 transition 不调用 materialize | Unit 3 应扩展现有入口，并保留旧调用者语义 | 高 |
| E6 | `crates/ralph-proto/src/event_bus.rs::EventBus::publish` | 返回 `Vec<HatId>`，不是 `Result`；未知 source 直接返回空 recipients | 不能编造 bus publish error；“发布失败”测试必须注入 outbox durable failure，并断言 bus 零事件 | 高 |
| E7 | `rg -n "state_machine|state-machine" presets/en presets/schemas crates/ralph-cli/preset-templates` | builtin preset/schema/template 没有 StateMachine 配置 | 默认路径不增加行为；不修改 preset/schema/manifest | 高 |
| E8 | `crates/ralph-core/src/config/loop_config.rs`、`config/state_machine.rs` | StateMachine 是 `LoopConfig.state_machine: Option<StateMachineConfig>`，默认 None，enabled 由用户显式设置 | 范围限定为显式启用流程；兼容测试必须覆盖 None/disabled | 高 |
| E9 | `crates/ralph-core/src/event_loop/parse_and_emit.rs` StateMachine stage | StateMachine stage 在 workflow guard、record_event、publish 之前运行；后面仍有 `validated_events`/`pending_publish` 两次接受过滤 | StateMachine 只能产生 candidate，最终状态必须延后到 `pending_publish`/AcceptedTransition 边界 | 高 |
| E10 | `crates/ralph-core/src/event_loop/tests/state_machine.rs` | 已有 terminal rejected、branch close、accepted event summary 测试；现有测试只验证部分 live runtime，不验证 Ledger replay | 先保留 characterization，再扩展 downstream rejection/replay 断言 | 高 |
| E11 | `crates/ralph-core/src/state/tests.rs` | 已有 commit failure rollback、replay、process restart、exhaustive apply_delta 测试 | Unit 1 直接扩展现有 state test，避免创建第二种测试框架 | 高 |
| E12 | `crates/ralph-core/src/event_loop/wave_scope.rs` completion path | `check_completion_event` 通过后先提交 `CompletionHonored`，再调用 `sm_state.mark_terminal_honored()`；当前 mark 未进入 Ledger | Unit 4 必须把 terminal honored 作为单独 committed state，保持“观察到 terminal”与“最终 honor”区分 | 高 |
| E13 | `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs`、`lifecycle.rs` | runtime 初始化会构造 StateLedger，但 `LoopState.state_machine_runtime_state` 仍从 None 开始 | Unit 4 在现有 lifecycle wiring 中 hydration，不在 agent/CLI 层新增入口 | 高 |
| E14 | `crates/ralph-core/src/event_loop/disposition.rs`、`parse_and_emit.rs` | compiled execution contract 下 Business/Recovery 走 AcceptedTransition；Diagnostic/LoopControl direct publish；无 contract 的 legacy path direct publish | 设计必须覆盖三类发布路径，并明确只有 Business/Recovery 使用 outbox projection | 高 |
| E15 | `crates/ralph-core/src/state/knowledge.rs`、GAP-01 计划 | GAP-01 只从 accepted event 产生 bounded `KnowledgeObserved`，状态是 Observation/Unverified，不替代 business acceptance | 不修改 GAP-01 认知语义，不把 StateMachine commit 混进 knowledge observation | 高 |
| E16 | `crates/ralph-core/data/ralph-tools-opac.md`、其他 `data/*.md` | Agent-facing skill 只描述公开 Observe/Precheck/Apply/Confirm；没有 StateMachine runtime 内部操作 | 本计划不改变 agent 下一步动作，不改 data skill；只做 stale-reference 检查 | 高 |
| E17 | `git log --oneline -- state_machine.rs state/commit.rs event_loop/accepted_transition.rs` | StateMachine、Ledger、AcceptedTransition 是已有演进模块，已有 rollback/replay 先例 | 采用增量 wiring，不做跨模块重写 | 中 |

### 2.3 受影响范围

已确认的生产模块：

- `crates/ralph-core/src/state_machine.rs`
- `crates/ralph-core/src/state/commit.rs`
- `crates/ralph-core/src/state/snapshot.rs`
- `crates/ralph-core/src/state/ledger.rs`（只在需要暴露已有 receipt 查询/repair 能力时修改；优先不动核心 persist 算法）
- `crates/ralph-core/src/event_loop/parse_and_emit.rs`
- `crates/ralph-core/src/event_loop/accepted_transition.rs`
- `crates/ralph-core/src/event_loop/disposition.rs`
- `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs`
- `crates/ralph-core/src/event_loop/lifecycle.rs`
- `crates/ralph-core/src/event_loop/wave_scope.rs`
- `crates/ralph-core/src/event_loop/loop_state.rs`（仅当 hydration 需要新增明确的构造辅助函数）

已确认的测试模块：

- `crates/ralph-core/src/state/tests.rs`
- `crates/ralph-core/src/state_machine.rs` 内 unit tests
- `crates/ralph-core/src/event_loop/tests/state_machine.rs`
- `crates/ralph-core/src/event_loop/tests/replay_light_integration.rs`
- `crates/ralph-core/src/event_loop/accepted_transition.rs` 内 tests
- `crates/ralph-core/src/event_loop/disposition.rs` 内 tests
- `crates/ralph-core/tests/scenarios.rs`（只在真实 workflow scenario 必要时扩展）

明确不受影响：CLI 命令、preset YAML/schema、manifest/index、zsh completion、Agent data skill 内容、OPAC 行为、GAP-01 knowledge schema。构建目标为 `ralph-core` 及 workspace；最终按仓库硬规则运行 `./scripts/run-tests.sh`。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | StateMachine durable state 放在哪里 | 新文件；events.jsonl 重放；现有 LedgerSnapshot/CommitDelta | 复用 `LedgerSnapshot.state_machine_runtime` + 新 `CommitDelta` | E2/E3/E4 | 新文件会产生第二权威；events.jsonl 不是统一 ledger replay source | 0.97 |
| D2 | 是否修改 `StateMachineDecision` enum | 改 enum 携带完整 delta；保持 enum，新增 candidate/delta 辅助类型 | 保持 enum 公开形状，新增 staging 和 semantic delta API | E1/E9/E10 | 改 enum 会扩大现有 unit/serde/调用方回归面；返回值不足可由 config+candidate diff 补齐 | 0.91 |
| D3 | live state 何时更新 | StateMachine validate 后立即更新；state projection 后；最终 `pending_publish`/AcceptedTransition 后 | 只在最终接纳边界更新；validator 使用 candidate | E1/E9/E14 | 前两者正是当前 gap；最终边界覆盖下游拒绝 | 0.95 |
| D4 | AcceptedTransition 如何关联 StateMachine projection | 完全绕过 AcceptedTransition；在 bus publish 后再 commit；在现有 acceptance receipt 上附带可选 projection 并启动 repair | 固定顺序为：计算 transition identity → 在 outbox entry 中写入可选 StateMachine projection receipt → durable outbox 成功后提交 Ledger projection → Ledger 成功后才 publish；若进程在两步之间退出，启动时按 receipt identity 幂等补账。重复 identity 先检查 Ledger 是否已有对应 projection，再决定是否 publish/返回 | E5/E6/E14/E17 | 绕过 outbox 会破坏已有 acceptance authority；publish 后 commit 有 crash window；新文件会创建第二 receipt；不固定顺序会让 Executor 自行选择错误边界 | 0.92 |
| D5 | “发布失败”如何测试 | 虚构 EventBus error；修改 EventBus API；注入 outbox durable write failure | 使用已有 `CommitFailed` outbox failure，断言无 bus publish/无 live state；不修改 EventBus API | E5/E6、现有 `u6_commit_failure_no_publish` | EventBus 没有 Result；改 API 与本增强无关且回归面大 | 0.99 |
| D6 | terminal observed 与 terminal honored 是否同一 delta | 一次写入；继续区分两个阶段 | accepted terminal 产生 observed delta；completion check 通过后单独产生 honored delta；rejected terminal 两者都不写 | E10/E12 | 现有行为明确区分“终端已看到”和“终端被 honor”；合并或提前写入会破坏 open-task/verdict gate 语义 | 0.96 |
| D7 | disabled/default path 是否使用新账本 | 所有事件都走新逻辑；仅 enabled path | 只有 `state_machine.enabled` 才 staging/commit；None/false 完全保持现有路径 | E7/E8/E14 | builtin preset 未启用；默认路径增加 ledger I/O 会带来无意义回归 | 0.98 |
| D8 | Agent skill/preset 是否需要适配 | 修改 data skill/preset；不修改，只做 drift audit | 不修改；若实现引入新的 agent-facing action 才停止并新增决策 | E7/E16 | 本功能不新增命令/字段/agent action；内部记账对 Agent 不可见 | 0.94 |
| D9 | projection-aware acceptance 如何取得可写 Ledger | 修改现有所有 `&StateLedger` API；使用全局可变状态；新增 helper 接收 `&mut StateLedger` 并在调用点显式取出再放回 | 保留旧 `commit`/`commit_idempotent`/`publish_synthetic` 签名；新增 projection-aware helper 接收 `&mut StateLedger`，EventLoop 发布边界使用现有 `std::mem::take(&mut self.state.state_ledger)` 完成 borrow boundary 后恢复 | E4/E5/E14，当前 `StateLedger::commit(&mut self)` 与已有 `std::mem::take` wiring | 全局状态会扩大并发/回归面；修改旧签名会波及所有 outbox consumers；隐式 interior mutability 会掩盖记账边界 | 0.91 |

所有决策均达到 `0.85`。D4/D9 是相对风险最高的决策，Unit 3 的 Acceptance Red 和 outbox consumer 回归必须验证其关键假设；若 H1 失败，D4 必须回退为 BLOCKED，不得让 Executor 临时改 API。

## 4. BDD 行为规格

### Feature: 显式启用 StateMachine 的转换必须按最终接纳结果记账

  Background:

    Given EventLoop 使用 `event_loop.state_machine.enabled: true`
    And StateLedger 以测试 workspace 为根目录
    And StateMachine 配置包含可打开、推进、关闭实例的 transition

  Scenario: 最终接纳的业务转换同时更新 live state 和 Ledger

    Given 实例 `t1` 当前为 `idle`
    When 输入合法的业务事件并通过所有 downstream gates
    Then 事件进入原有 accepted event 结果和 bus
    And `t1` 的 live state 变为配置指定的新状态
    And Ledger 中存在一个 StateMachine transition commit
    And commit 的 replay 结果与 live state 相等

  Scenario: StateMachine 接受但后续 completion gate 拒绝时不推进终态

    Given 当前有未满足的 required event 或 open task
    When 输入 `LOOP_COMPLETE` 且 StateMachine validator 预检查通过
    And 现有 completion check 拒绝终态
    Then `terminal_honored` 保持 false
    And StateMachine 不留下 accepted terminal transition
    And 后续合法业务事件仍按拒绝终态前的状态验证

  Scenario: 下游拒绝不产生 StateMachine 账

    Given 一个业务事件能通过 StateMachine，但会被后续确定性 gate 拒绝
    When EventLoop 完成该批次处理
    Then 该业务事件不在 accepted event 结果中
    And live StateMachine 与处理前完全相同
    And Ledger 不新增该 transition 的 commit
    And bus 不收到该业务事件

  Scenario: Ledger durable write 失败时不发布且恢复原状态

    Given Ledger 文件路径被故意设置为不可写目录
    When 输入本来会被最终接纳的 StateMachine 业务事件
    Then EventLoop 返回现有 I/O/commit failure
    And live StateMachine 保持处理前状态
    And bus 不收到该业务事件
    And 不存在成功的 StateMachine commit

  Scenario: AcceptedTransition outbox 写入失败时不留下状态机半提交

    Given 业务事件走 compiled execution contract 的 AcceptedTransition 路径
    And outbox 路径被故意设置为目录以触发 `CommitFailed`
    When 提交该业务事件
    Then AcceptedTransition 返回 `CommitFailed`
    And bus 收到零个该业务事件
    And live StateMachine 不推进
    And Ledger 不出现该 transition 的 accepted projection

  Scenario: 重启后 replay 恢复与 live 等价的 StateMachine state

    Given 第一个 EventLoop 接纳一组打开、推进、关闭实例的事件
    And completion honored 状态也已被接受并记账
    When 第一个 EventLoop 被丢弃并用同一 workspace 创建第二个 EventLoop
    Then 第二个 EventLoop 的 StateMachine open/closed instances 与第一个最终状态相同
    And accepted transition count 不重复增加
    And terminal honored 状态保持一致

  Scenario: 相同 compiled-contract accepted transition 重放不会重复应用

    Given compiled-contract durable outbox 和 Ledger 已记录某个 transition identity
    When 以相同 loop/activation/contract/event identity 再次提交
    Then outbox 不新增重复 acceptance
    And StateMachine 不重复打开/关闭实例
    And bus 不发生重复发布

  Scenario: 未启用 StateMachine 的默认路径保持原行为

    Given `state_machine` 配置缺失或 `enabled: false`
    When 输入现有 builtin workflow 使用的事件
    Then EventLoop 的 accepted/rejected/publish 结果与改动前一致
    And 不新增 StateMachine commit
    And 不新增 StateMachine outbox projection

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
|---|---|---|---|---|---|
| S1 | 断言 live open/closed state、Ledger commit delta、replay snapshot 三者相等；bus 有且仅有一次 | `crates/ralph-core/src/event_loop/tests/state_machine.rs` | EventLoop 集成测试 | StateLedger replay characterization | 否 |
| S2 | 断言 terminal honored=false、无 accepted terminal delta、后续业务事件仍可按旧状态处理 | 已存在 `test_state_machine_terminal_rejected_by_open_tasks_does_not_honor_terminal` 扩展 | EventLoop 集成测试 | State-machine state-machine test | 否 |
| S3 | 断言 downstream reject 后 `state_machine_runtime_state`、ledger commit count、bus recipients 均不变 | `event_loop/tests/state_machine.rs` 新增真实 runtime 场景 | EventLoop 集成测试 | Characterization + state transition diff | 否 |
| S4 | 断言 Ledger error 返回、snapshot 等于 before、bus 零事件 | `state/tests.rs` + `event_loop/tests/state_machine.rs` | 单元 + 集成 | Fault Injection | 否 |
| S5 | 断言 `CommitFailed`、outbox failure、bus zero、StateMachine projection zero | `accepted_transition.rs` 现有 `u6_commit_failure_no_publish` 扩展 | AcceptedTransition 集成测试 | Fault Injection + idempotency | 否 |
| S6 | 两个 EventLoop 实例在同一 workspace 的 StateMachine snapshot 字段完全一致 | `event_loop/tests/replay_light_integration.rs` 或已确认的 state_machine integration module | 进程生命周期集成测试 | Differential replay | 否 |
| S7 | 相同 transition identity 不重复写 outbox、不重复 materialize、不重复 publish | `accepted_transition.rs` 现有 U7 tests 扩展 | 单元/集成 | Idempotency | 否 |
| S8 | None/false 配置下既有 StateMachine tests、scenario、builtin smoke 结果不变 | `event_loop/tests/state_machine.rs` + `./scripts/run-tests.sh` | Regression | Differential disabled-path characterization | 否；全量门禁覆盖 |

每个测试必须同时断言：外部结果、bus/outbox 副作用、Ledger commit 变化、live/replay 不变量。不得只断言 `StateMachineDecision::Accept`，因为那正是当前 gap 未覆盖的层级。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | accepted StateMachine transition 必须可持久化/replay | S1/S6 | `state_machine_accept_commits_and_replays` | `apply_state_machine_delta` | EventLoop restart integration | 否 | E2/E3/E4 |
| R2 | downstream reject 不得推进 StateMachine | S2/S3 | `rejected_terminal_does_not_commit_state_machine` | candidate-vs-live diff | state_machine EventLoop integration | 否 | E1/E9/E10 |
| R3 | Ledger write failure 必须 rollback 且不发布 | S4 | `state_machine_commit_failure_rolls_back` | existing `failed_commit_preserves_snapshot` 扩展 | EventLoop failure integration | 否 | E4/E6 |
| R4 | AcceptedTransition durable boundary 失败不得发布/半提交 | S5 | `accepted_transition_failure_does_not_materialize_state_machine` | existing `u6_commit_failure_no_publish` 扩展 | disposition/AcceptedTransition integration | 否 | E5/E6/E14 |
| R5 | restart hydration 必须等价 | S6 | `state_machine_runtime_hydrates_from_ledger` | `replay_state_machine_runtime` | replay-light lifecycle test | 否 | E4/E13 |
| R6 | repeated acceptance 必须幂等 | S7 | `state_machine_projection_is_idempotent` | delta identity guard | AcceptedTransition integration | 否 | E5/E14 |
| R7 | disabled/default path 不变 | S8 | existing StateMachine disabled tests + regression | `test_disabled_state_machine_accepts_all` | full ralph-core/workspace test | 必要时 mock E2E，仅最终门禁 |
| R8 | terminal observed/honored 语义保持分离 | S2/S6 | `terminal_honored_replays_only_after_completion_gate` | terminal delta apply | EventLoop completion integration | 否 | E10/E12 |

## 7. 严格串行开发单元

执行顺序固定如下：

```text
Unit 1：StateMachine Ledger delta 与 replay
  ↓ 完成 Acceptance Red、Unit Red、Green、Refactor、Integration、Regression、Close
Unit 2：候选状态与最终 EventLoop 接纳边界
  ↓ 完成 Acceptance Red、Unit Red、Green、Refactor、Integration、Regression、Close
Unit 3：AcceptedTransition/outbox 与 StateMachine projection 的幂等接纳
  ↓ 完成 Acceptance Red、Unit Red、Green、Refactor、Integration、Regression、Close
Unit 4：重启 hydration、terminal honored 与全链路回归
```

### Unit 1：StateMachine Ledger delta 与 replay

#### 1. Unit 目标

让一个已经明确被接受的 StateMachine transition 能通过现有 `StateLedger` 写入 `LedgerSnapshot.state_machine_runtime`，并在 `replay_from_disk` 后恢复同一 accepted state；本 Unit 不改变 EventLoop 的接纳时机。

#### 2. 对应需求与 Scenario

- Requirement：R1、R5、R8
- Scenario：S1、S6、S7、S8 的 ledger 子集
- Decision：D1、D2、D6、D7
- Evidence：E1、E2、E3、E4、E11

#### 3. 外部可观察结果

调用方可通过 `StateLedger::snapshot()` 看到 StateMachine runtime；丢弃 ledger 后用同一 workspace 重新构造，snapshot 的 accepted instances/count/terminal flags 相等。没有任何 EventLoop 路径改变。

#### 4. 当前行为基线

当前 `LedgerSnapshot` 虽有 `state_machine_runtime` 字段，但 `CommitDelta` 没有 StateMachine 分支，`apply_delta` 不能重建它；当前 StateMachine unit tests 只验证内存对象。先运行现有 `cargo nextest run -p ralph-core -- state_machine` 和 `cargo nextest run -p ralph-core -- replay_from_disk`，固定旧行为通过。

#### 5. 输入与输出

- 输入：accepted transition 的 instance key、new state、topic、open/close 语义、terminal observed/honored、transition identity。
- 输出：新增 semantic StateMachine delta；`LedgerSnapshot.state_machine_runtime` 发生同等投影；旧 Commit JSONL 仍可反序列化。
- 错误：Ledger 持久化错误沿 `LedgerError` 返回，snapshot/commit log 不改变。
- 状态变化：只改变 Ledger snapshot，不改变 LoopState。
- 副作用：成功时只写 `.ralph/ledger.jsonl`；失败时无成功 commit。
- 不变量：delta replay 是幂等的；`LedgerSnapshot` 额外维护已应用的 StateMachine transition identities；同一 identity 不重复计数。

#### 6. 修改位置

- `crates/ralph-core/src/state_machine.rs`：增加可序列化、可比较的最小 accepted transition projection/delta 辅助类型或构造方法；保留 `StateMachineDecision` 结构和原有 `validate_event` 行为。不修改 terminal rejection 文本。
- `crates/ralph-core/src/state/commit.rs`：在 `CommitDelta` 中新增 StateMachine semantic variant，字段只包含 replay 所需增量和 transition identity，不保存整个 `LedgerSnapshot`。
- `crates/ralph-core/src/state/snapshot.rs`：在 `apply_delta` exhaustive match 中处理新 variant，写入已有 `state_machine_runtime`，并维护一个仅用于 replay 幂等判断的 applied transition identity 集合；保证旧 delta 分支不改。
- `crates/ralph-core/src/state/tests.rs`：在现有 commit/replay/failure/exhaustive tests 位置新增测试。
- `crates/ralph-core/src/state_machine.rs` tests：补充 projection/delta 计算的纯单元测试。

不修改 `EventLoop`、AcceptedTransition、config、preset、data skill。

#### 7. 可依赖能力

- 现有 `StateMachineRuntimeState: Clone + Serialize + Deserialize + PartialEq`。
- 现有 `StateLedger::commit` 的 snapshot apply、原子写入、失败 rollback、replay。
- 现有 `CommitDelta` exhaustive apply test。

#### 8. 禁止依赖的未来能力

- 不依赖 Unit 2 的 candidate staging。
- 不提前把新 delta 接入 EventLoop。
- 不提前修改 AcceptedTransition/outbox wire format。
- 不修改 `StateMachineDecision` 对外枚举，避免把 Unit 3 的 receipt 设计提前混入。

#### 9. 验收测试

- 测试名称：`state_machine_delta_commit_replays_to_same_runtime`。
- 层级：`ralph-core` state integration/unit。
- 前置：fresh temp workspace + `StateLedger::new(..., true)`。
- 输入：打开 `t1`、推进 `t1`、关闭 `t1`、terminal observed、terminal honored 的 semantic delta 序列。
- 动作：commit delta；读取 live snapshot；创建新的 ledger replay。
- 断言：open/closed maps、accepted count、terminal flags 全部相等；commit log 每个 sequence 单调递增。
- 副作用：`.ralph/ledger.jsonl` 可读；不存在重复 transition identity。
- 不变量：旧字段（tasks、progress、knowledge、completion）仍保持默认值。
- 命令：`cargo nextest run -p ralph-core -- state_machine_delta_commit_replays_to_same_runtime`；再运行 `cargo nextest run -p ralph-core -- replay_from_disk`。

#### 10. Acceptance Red

首先运行新增 acceptance test。未实现时必须因 `CommitDelta` 没有 StateMachine projection 或 `apply_delta` 后 replay runtime 仍为 `None` 而失败；这是有效 Red，因为它穿过真实 `StateLedger::commit` 与 `replay_from_disk`。

以下不算有效 Red：测试编译错误、temp workspace 未创建、测试过滤器没有执行目标、旧 knowledge/GAP-01 dirty code 的无关失败、或通过删除断言让测试通过。

#### 11. 单元测试拆分

1. `apply_state_machine_transition_delta`：输入 open transition，断言 open map/state/topic/count，并记录 transition identity。
2. `apply_state_machine_close_delta`：输入 close transition，断言从 open 移到 closed，重复 apply 不重复计数且不重复写 applied identity。
3. `apply_terminal_observed_delta`：断言 observed=true、honored=false。
4. `apply_terminal_honored_delta`：只在 observed 后设置 honored；未 observed 不得伪造 honored。
5. `replay_old_commit_log_without_state_machine_delta`：旧 log replay 不报错，runtime 保持 default。
6. `failed_state_machine_commit_restores_snapshot`：ledger path 为目录时，snapshot/commit log 不变。

Fake/Stub：只使用 tempdir 和真实 StateLedger；不 mock `LedgerSnapshot::apply_delta`、`replay_from_disk` 或 serde。

#### 12. Red → Green → Refactor 顺序

1. Test 1 Red：`state_machine_delta_commit_replays_to_same_runtime` 失败，因为无 delta/apply 分支。
2. 最小实现：新增 semantic delta 和 `apply_delta` 分支；Test 1 Green。
3. Test 2 Red：重复 delta 断言失败，因为 identity/idempotency 尚未实现。
4. 最小实现：按 transition identity 在 snapshot projection 中忽略已应用 identity，或采用与当前 Commit 顺序一致的幂等语义；Test 2 Green。
5. Test 3 Red：terminal observed/honored 分离断言失败；最小实现两种 delta 分支；Test 3 Green。
6. Test 4 Red：ledger write failure snapshot rollback 断言失败；修正 delta apply 与现有 commit rollback 的边界；Test 4 Green。
7. Refactor：只抽取 state-machine delta apply 私有 helper，保持 `apply_delta` exhaustive 可读。
8. 运行 Unit 1 全部测试、state tests、replay tests 后 Close。

#### 13. 最小实现范围

- 必须：semantic delta、snapshot apply、serde compatibility、replay、idempotency、failure rollback tests。
- 必须保持：旧 CommitDelta 的 JSON 形状、旧 StateMachineDecision、旧 Ledger failure semantics。
- 必须处理：空 instance key、closed instance、terminal flags、重复 identity。
- 不实现：EventLoop staging、outbox receipt、startup hydration、Agent skill。

#### 14. 集成验证

联合真实模块：`CommitDelta` → `LedgerSnapshot::apply_delta` → `StateLedger::commit` → `replay_from_disk`。可 fake 的只有 filesystem failure fixture（把 ledger path 设为目录）；不得 fake replay。

命令：`cargo nextest run -p ralph-core -- state_machine_delta`、`cargo nextest run -p ralph-core -- replay_from_disk`、`cargo nextest run -p ralph-core -- state`。任一失败不得进入 Unit 2。

#### 15. 风险驱动测试

- Characterization：现有 `state/tests.rs` replay/failure tests，防止 Ledger 核心行为回归。
- Property-like round trip：构造多 transition 序列，比较 live/replay snapshot。
- Idempotency：重复相同 transition identity；原因是 AcceptedTransition 本来定义了幂等边界。
- Fault Injection：ledger path directory；原因是 commit rollback 是本 Gap 的关键。

#### 16. 回归范围

- 直接：`cargo nextest run -p ralph-core -- state_machine`、`-- replay_from_disk`、`-- state`。
- 相邻：`cargo nextest run -p ralph-core -- u11_unified_pipeline`，因为 Ledger snapshot 被统一 pipeline 读取。
- 旧数据：旧 `.ralph/ledger.jsonl` 无新 delta 必须正常 replay。
- 默认关闭：现有 disabled StateMachine test。
- 构建/lint：Unit Close 前 `cargo fmt --check`、`cargo build`、`cargo clippy`。
- 全量：最终 Unit 4 执行 `./scripts/run-tests.sh`。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/state_machine.rs` | 修改现有生产文件 + unit tests | 定义最小 accepted projection/delta helper | E1 |
| `crates/ralph-core/src/state/commit.rs` | 修改现有生产文件 | 新增 StateMachine CommitDelta | E2 |
| `crates/ralph-core/src/state/snapshot.rs` | 修改现有生产文件 | apply delta/replay | E3 |
| `crates/ralph-core/src/state/tests.rs` | 新增测试 | commit/replay/failure/idempotency | E4/E11 |

#### 18. 完成标准

Acceptance/Unit/Integration/Regression 全通过；build/lint 通过；无 skip/only/弱化断言；旧 ledger replay 通过；未提前修改 EventLoop/outbox；Evidence/Decision 更新；Unit 可独立提交。

#### 19. 停止条件

新增 delta 无法保持旧 JSONL replay、需要改 `StateMachineDecision`、Ledger failure 无法恢复 snapshot、或单文件接近 5000 行时停止。记录新证据并重新决策，不进入 Unit 2。

#### 20. 风险与注意事项

- 风险：把完整 runtime snapshot 塞进每条 commit，导致 ledger 膨胀。检测：review delta 字段和 serialized line；缓解：只记录 transition semantic delta。
- 风险：重复 delta 使 count 漂移。检测：idempotency test；缓解：使用稳定 identity/已有序列语义。
- 剩余风险：拒绝指纹仍是非权威 runtime 诊断，重启后可能重新生成诊断；本 Unit 不把它伪装为 accepted state。

### Unit 2：候选状态与最终 EventLoop 接纳边界

#### 1. Unit 目标

让 EventLoop 的 StateMachine validation 使用候选 runtime，并只为最终进入 `pending_publish` 的事件生成 Unit 1 定义的 StateMachine projection；本 Unit 只生成 projection plan，不写 Ledger、不调用 AcceptedTransition，确保下游拒绝不能改变 live state。

#### 2. 对应需求与 Scenario

- Requirement：R2、R7、R8
- Scenario：S1、S2、S3、S8
- Decision：D2、D3、D7
- Evidence：E1、E9、E10、E14

#### 3. 外部可观察结果

StateMachine 被 validator 接受但被 completion/workflow/emit gate 拒绝时，`ProcessedEvents` 和 live runtime 都不包含该 transition；projection plan 也不包含该事件。最终接受的事件仍按原 topology 处理，并把一个待交给 Unit 3 的 projection plan 传出。

#### 4. 当前行为基线

`parse_and_emit.rs` 当前在 `1517` 附近取得 `&mut self.state.state_machine_runtime_state` 并直接调用 `validate_event`；现有 `test_state_machine_terminal_rejected_by_open_tasks_does_not_honor_terminal` 只保证 `terminal_honored=false`，没有保证 terminal observed/transition 不提前落账。先运行该测试固定基线。

#### 5. 输入与输出

- 输入：同一批 `Event`、当前 live StateMachine、StateMachineConfig、后续最终 `pending_publish`。
- 输出：accepted/rejected events 与现有结果保持一致；仅最终 accepted events 生成 StateMachine projection plan，供 Unit 3 作为唯一 durable input。
- 错误：下游 reject 是现有 reject/diagnostic 语义，不写 accepted state；Ledger failure 返回错误。
- 状态变化：只更新局部 candidate；本 Unit 结束时 live StateMachine、Ledger、outbox、bus 均不因 projection plan 发生变化。
- 副作用：reject 不写 StateMachine ledger、不发布业务 event；accepted projection 也不在本 Unit 提前落账。
- 不变量：现有 state machine transition selection、finding reason、accepted event order 不改变。

#### 6. 修改位置

- `crates/ralph-core/src/state_machine.rs`：新增基于 clone/candidate 的辅助调用；不得修改现有 `validate_event` 的结果语义。
- `crates/ralph-core/src/event_loop/parse_and_emit.rs`：将 StateMachine stage 从 live mutation 改为 candidate；在 `pending_publish` 形成后计算最终 projection；只接入 Unit 1 的 delta API。
- `crates/ralph-core/src/event_loop/tests/state_machine.rs`：扩展已有 terminal rejection/branch close/accepted summary tests，新增 downstream rejection 场景。

不修改 workflow guard 规则、policy、execution contract、EventBus routing。

#### 7. 可依赖能力

- Unit 1 的 CommitDelta/apply/replay。
- 现有 `validated_events`、`pending_publish` 流程。
- 现有 terminal completion check 和 diagnostics。

#### 8. 禁止依赖的未来能力

- 不提前修改 AcceptedTransition outbox schema（Unit 3）。
- 不提前做 restart hydration（Unit 4）。
- 不改变 `EventBus::publish` 签名。

#### 9. 验收测试

- `state_machine_rejected_terminal_does_not_commit_runtime`：使用现有 open-task/required-event completion rejection fixture；断言处理后 live runtime 与 before 相等，后续 business event 可接受。
- `state_machine_final_acceptance_builds_projection_plan`：真实 EventLoop 处理合法 business event；断言 accepted event 顺序不变、projection plan 只包含最终 accepted event，且本 Unit 不写 bus/ledger/live state。
- `state_machine_downstream_rejection_keeps_live_runtime`：使用现有确定性 downstream gate fixture；不得用 mock 绕过 EventLoop。
- 命令：`cargo nextest run -p ralph-core -- state_machine_rejected_terminal`、`cargo nextest run -p ralph-core -- state_machine_final_acceptance`、`cargo nextest run -p ralph-core -- state_machine_downstream_rejection`。

#### 10. Acceptance Red

首先运行 terminal rejection acceptance。目标 Red 是当前实现会在 `validate_event` 后留下 terminal observed/accepted count 变化，导致 before/after runtime diff 失败；该失败证明测试执行到了真实 StateMachine stage 和后续 completion gate。

如果测试只失败在 fixture 没有 open task、required event 或 gate 没触发，则不是有效 Red；先修 fixture，不得修改生产代码迎合错误 fixture。

#### 11. 单元测试拆分

1. `preview_event_does_not_mutate_source`：clone candidate，source runtime 完全相等。
2. `preview_accept_returns_candidate_transition`：candidate 包含新 state，source 不变。
3. `preview_reject_returns_original_candidate`：reject 不改变 candidate/source。
4. `final_projection_uses_only_pending_publish_events`：validated 但被 final gate drop 的 event 不出现在 delta。
5. `disabled_state_machine_skips_candidate_and_delta`：None/false 配置不创建 projection。

Fake/Stub：只 fake deterministic filesystem failure；EventLoop gate、StateMachine、Ledger、EventBus 必须真实执行。

#### 12. Red → Green → Refactor 顺序

1. Test 1 Red：terminal rejection 后 runtime diff 不相等。
2. 最小实现：StateMachine stage 使用 candidate clone；Test 1 Green。
3. Test 2 Red：最终 accepted event 没有 StateMachine projection plan。
4. 最小实现：在 final accepted list 计算 projection plan，但不提交 delta；Test 2 Green。
5. Test 3 Red：downstream rejected event 仍进入 projection；最小实现只从 `pending_publish` 生成 projection；Test 3 Green。
6. Test 4 Red：disabled path 出现多余 delta；加 enabled guard；Test 4 Green。
7. Refactor：抽取小型 staging/finalization helper，禁止把更多 gate 逻辑塞进 StateMachine module。

#### 13. 最小实现范围

- 必须：candidate source、final accepted event filtering、projection plan、terminal rejection no-op。
- 必须保持：accepted event 顺序、finding、diagnostic topic、workflow guard、direct publish behavior。
- 必须处理：同批多 event 顺序、open→close、terminal observed/honored 分离、disabled path。
- 不实现：outbox crash repair、startup hydration、new CLI/data skill。

#### 14. 集成验证

真实联合：EventReader/ParseResult → EventLoop policy/state machine → existing downstream gate → `pending_publish` → projection plan。只真实验证 accepted/rejected stream 与 candidate 不变；StateLedger/AcceptedTransition durable commit 留到 Unit 3，避免本 Unit 提前实现未来行为。

命令：`cargo nextest run -p ralph-core -- state_machine`、`cargo nextest run -p ralph-core --test replay_light_integration`。任何既有 StateMachine test 失败都不能进入 Unit 3。

#### 15. 风险驱动测试

- Characterization：已有 terminal rejected、branch close、processed events tests。
- State-machine test：多事件 ordered transitions，原因是 candidate 与 final list 的顺序会决定状态。
- Differential disabled path：enabled=false 与原行为的 accepted topics/bus 结果比较。
- Fault Injection：在 final ledger commit 失败时比较 before/after runtime。

#### 16. 回归范围

- 直接：`cargo nextest run -p ralph-core -- state_machine`。
- 相邻：`cargo nextest run -p ralph-core --test replay_light_integration`、`cargo nextest run -p ralph-core --test scenarios`。
- 公共消费者：`ProcessedEvents.accepted_events`、EventBus observers、phase authority input。
- 默认/旧配置：无 state_machine、enabled=false、已有 StateMachine YAML fixture。
- 构建/lint：`cargo build`、`cargo clippy`、`cargo fmt --check`。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/state_machine.rs` | 修改现有生产文件 | candidate preview/final projection helper | E1/E9 |
| `crates/ralph-core/src/event_loop/parse_and_emit.rs` | 修改现有生产文件 | 延后 live mutation 到最终接纳 | E9 |
| `crates/ralph-core/src/event_loop/tests/state_machine.rs` | 新增/修改测试 | downstream rejection/final acceptance | E10 |

#### 18. 完成标准

S1/S2/S3/S8 的 candidate/projection 子断言通过；现有 StateMachine tests 全绿；accepted event order 和 diagnostics 不变；projection plan 只来自 final accepted list，且本 Unit 没有 Ledger/outbox/bus/live side effect；build/lint 通过；Unit 可独立提交。

#### 19. 停止条件

如果无法在不改变 accepted/rejected event 结果的情况下实现 candidate；如果 `pending_publish` 不是最终 acceptance list；如果需要改 EventBus API；如果新的公开调用方出现；停止并更新 D3，不进入 Unit 3。

#### 20. 风险与注意事项

- 风险：用 `validated_events` 而不是 `pending_publish` 生成 delta，会重新引入下游拒绝半状态。检测：S3；缓解：代码 review 强制 projection 输入只来自 final list。
- 风险：同批事件 candidate 顺序与最终 publish 顺序不同。检测：ordered multi-event test；缓解：复用现有 `pending_publish` 顺序。
- 剩余风险：Unit 2 结束时 contract outbox 仍未承载 StateMachine receipt；这是 Unit 3 的明确未完成边界，不得宣称 Gap 完成。

### Unit 3：AcceptedTransition/outbox 与 StateMachine projection 的幂等接纳

#### 1. Unit 目标

把 Unit 2 产生的 accepted StateMachine projection 与现有 AcceptedTransition durable receipt 绑定：outbox durable boundary 失败时不发布、不 materialize live state；重复 receipt 不重复应用。

#### 2. 对应需求与 Scenario

- Requirement：R3、R4、R6
- Scenario：S4、S5、S7
- Decision：D4、D5
- Evidence：E5、E6、E14、E17

#### 3. 外部可观察结果

Business/Recovery accepted transition 仍由 AcceptedTransition 负责 outbox→publish；新增 StateMachine projection 在同一 transition identity 下可恢复、幂等。outbox 写失败时 bus 零事件，live runtime 不变。

#### 4. 当前行为基线

当前 `AcceptedTransition::commit_idempotent_with_rollback` 已有 validate/materialize/rollback，但 `disposition::publish_synthetic` 传入的 materialize closure 是空操作；StateMachine state 在更早阶段已可能改变。现有 `u6_commit_failure_no_publish` 已证明 outbox directory 会返回 `CommitFailed` 且 bus zero，但不检查 StateMachine。

#### 5. 输入与输出

- 输入：Event、loop/activation/contract identity、StateMachine projection delta。
- 输出：现有 OutboxEntry 增加 serde-default 的可选 StateMachine projection receipt；成功时 ledger projection/live state 与 receipt identity 对齐。
- 固定顺序：先计算 transition identity 和 projection；再在同一 outbox lock 下写入包含 projection 的 receipt；receipt durable 成功后调用 `StateLedger::commit`；ledger 成功后才更新 LoopState live projection 并调用已有 bus publish。不得改成 publish 后写 ledger。
- 错误：outbox durable failure 返回 `TransitionError::CommitFailed`，不调用 ledger projection、不更新 live、不发布；receipt 已写但 ledger commit 失败时保留该 receipt 作为待补账凭证，当前调用仍返回错误且不发布，启动 repair 通过 transition identity 补齐；projection materialize failure 不得伪装成 bus 成功。
- 状态变化：成功 acceptance 才应用 live projection；失败调用内存没有 accepted StateMachine state，磁盘只可能存在带有明确 identity 的 pending receipt。
- 副作用：成功写 outbox、ledger、bus；失败 bus zero；重复提交先检查 receipt/ledger identity，已完整 materialize 的 transition 不再写 projection、不再重复 publish。
- 不变量：旧 outbox line 无 projection 字段仍可读取；现有 non-StateMachine transition 行为不变。

#### 6. 修改位置

- `crates/ralph-core/src/event_loop/accepted_transition.rs`：新增 projection-aware acceptance helper，签名中的 ledger 参数固定为 `&mut StateLedger`；保持现有 `commit`/`commit_idempotent` 的旧调用签名和语义，避免把所有调用方强制改造。
  新增 receipt 的字段固定为：`transition_id`、event `topic`、可选 `instance_key`、`new_state`、`opens_instance`、`closes_instance`、`terminal_observed`；不得把完整 `LedgerSnapshot` 或原始 payload 塞入 outbox。Ledger 的 `CommitDelta` 必须携带同一个 `transition_id`，启动 repair 只按该字段判断是否已经落账。
- `crates/ralph-core/src/event_loop/disposition.rs`：只在 Business/Recovery 且 StateMachine enabled 时传递 projection；Diagnostic/LoopControl 不写 StateMachine outbox projection。
- `crates/ralph-core/src/event_loop/parse_and_emit.rs`：把 Unit 2 的 final projection 传给 acceptance helper，不重排现有 event publish loop。
- `crates/ralph-core/src/event_loop/accepted_transition.rs` tests：扩展现有 outbox failure/idempotency tests。
- `crates/ralph-core/src/event_loop/disposition.rs` tests：验证 Business/Recovery 与 Diagnostic/LoopControl 分流。

不得修改 `EventBus::publish` 签名，不得把 direct diagnostic/control 变成 outbox transition。

#### 7. 可依赖能力

- Unit 1 semantic delta/apply/replay。
- Unit 2 final accepted projection。
- 现有 outbox lock、atomic write、transition_id、idempotent commit、rollback closure。

#### 8. 禁止依赖的未来能力

- 不提前实现 startup hydration/repair（Unit 4；本 Unit 可提供可调用 helper，但不接生命周期）。
- 不修改 parallel forge resume manifest 语义。
- 不新增 CLI/data skill。

#### 9. 验收测试

- `accepted_transition_failure_does_not_materialize_state_machine`：复用现有 `u6_commit_failure_no_publish` fixture，把 outbox path 设为目录；断言 CommitFailed、bus zero、live/ledger StateMachine unchanged。
- `accepted_transition_projection_is_idempotent`：同一 event identity 两次提交；断言一个 outbox、一次 ledger projection、一次 bus publish。
- `legacy_outbox_entry_without_projection_remains_readable`：手写现有字段 JSONL，`read_outbox` 成功，projection optional 为 None。
- `business_projection_uses_outbox_but_diagnostic_does_not`：扩展 disposition tests。

命令：`cargo nextest run -p ralph-core -- accepted_transition`、`cargo nextest run -p ralph-core -- disposition`。

#### 10. Acceptance Red

首先运行扩展的 outbox failure test。当前代码会因为 StateMachine 已在 Unit 2 final path materialize 或 projection 参数不存在而失败；正确 Red 必须同时看到 `CommitFailed` 与 StateMachine state mismatch/未覆盖断言，且 bus zero 的原有断言仍执行。

若失败来自 outbox fixture 路径未成为目录、`read_outbox` 解析测试本身错误、或 EventBus source guard 提前吞掉事件，则不是有效 Red。

#### 11. 单元测试拆分

1. `projection_receipt_round_trips_with_outbox_entry`：新可选字段 serde round trip。
2. `old_outbox_entry_defaults_to_no_projection`：兼容旧 JSON。
3. `commit_failure_runs_projection_rollback`：durable write failure 后 live state unchanged。
4. `duplicate_transition_skips_projection_and_publish`：现有 idempotent branch 不重复 materialize/publish。
5. `diagnostic_and_loop_control_skip_projection`：非 advancing disposition 不新增 projection。
6. `projection_identity_matches_outbox_transition_identity`：receipt 与 commit delta identity 一致。

不允许 mock `AcceptedTransition::find_committed`、`append_outbox_unlocked` 或 `EventBus::publish` 来伪造成功；故障测试使用真实 filesystem boundary。

#### 12. Red → Green → Refactor 顺序

1. Test 1 Red：OutboxEntry 无 projection 字段。
2. 最小实现：加 optional serde-default receipt；Test 1 Green。
3. Test 2 Red：outbox failure 后 StateMachine 仍推进或缺少 rollback。
4. 最小实现：projection-aware acceptance helper 使用已有 rollback contract；Test 2 Green。
5. Test 3 Red：重复提交仍 materialize/publish；最小实现接入同一 transition_id dedup branch；Test 3 Green。
6. Test 4 Red：Diagnostic/LoopControl 错误地产生 projection；修正 disposition 分支；Test 4 Green。
7. Test 5 Red：旧 outbox JSON 无法反序列化；补 serde defaults；Test 5 Green。
8. Refactor：保持旧 API wrapper，新增 helper 命名清楚；不把 receipt 逻辑散落到 parse loop。

#### 13. 最小实现范围

- 必须：optional receipt、projection-aware acceptance、固定的 outbox → ledger → live → publish 顺序、pending receipt repair identity、no publish on failure、idempotent replay、old outbox compatibility。
- 必须保持：transition_id 算法、outbox lock/fsync/atomic rewrite、diagnostic/control direct route。
- 必须处理：outbox write failure、existing committed entry、missing optional field、contract disabled direct path。
- 不实现：EventLoop startup repair 的调用、全量 outbox redrive、Agent-facing docs。

#### 14. 集成验证

真实联合：`disposition::publish_synthetic` → `AcceptedTransition` → `StateLedger` → `EventBus`。Business/Recovery、Diagnostic、LoopControl 各跑一次；legacy no-contract path 不得错误套用 outbox。对于无 compiled contract 的 StateMachine direct path，沿用 `pending_publish` 作为最终 acceptance list，但顺序固定为 `StateLedger::commit` 成功后才调用 direct `EventBus::publish`；Ledger 失败时直接返回、不发布。

命令：`cargo nextest run -p ralph-core -- accepted_transition`、`cargo nextest run -p ralph-core -- disposition`、`cargo nextest run -p ralph-core -- state_machine`。失败不得进入 Unit 4。

#### 15. 风险驱动测试

- Fault Injection：outbox path directory；已有真实失败模式，不能改成 mock。
- Idempotency：已有 U7 materialize tests，新增 StateMachine projection 断言。
- Contract compatibility：`integration_resume` 读取旧 outbox JSON，确认 optional 字段不破坏。
- Differential：StateMachine disabled 的 `publish_synthetic` 输出与修改前一致。

#### 16. 回归范围

- 直接：accepted_transition/disposition/state_machine nextest。
- 公共接口消费者：`parallel_forge_resume.rs`、CLI integration resume 中的 outbox parse/manifest path；运行 `cargo nextest run -p ralph-cli --test integration_resume`。
- 旧数据：旧 OutboxEntry JSONL 无新增字段。
- 默认关闭：所有非-StateMachine business/recovery/diagnostic/control 路径。
- 构建/lint：`cargo build`、`cargo clippy`、`cargo fmt --check`。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/accepted_transition.rs` | 修改现有生产文件 + 测试 | receipt/projection/rollback/idempotency | E5/E6 |
| `crates/ralph-core/src/event_loop/disposition.rs` | 修改现有生产文件 + 测试 | 按 disposition 传递 projection | E14 |
| `crates/ralph-core/src/event_loop/parse_and_emit.rs` | 修改现有生产文件 | 接入 final projection | E9/E14 |
| `crates/ralph-cli/tests/integration_resume.rs` | 仅在 H1 验证需要时新增兼容断言 | 旧 outbox consumer contract | H1/E14 |

#### 18. 完成标准

S4/S5/S7 通过；现有 outbox tests 全绿；旧 outbox consumer 全绿；没有 EventBus API 改动；失败不发布；重复不 materialize；Unit 可独立提交。

#### 19. 停止条件

发现任何 outbox consumer 依赖 byte-equal JSON、无法保留旧字段兼容、projection rollback 不能证明 live state 不变、或需要新增外部依赖时停止。D4 下降到 0.85 以下时标记 BLOCKED，不进入 Unit 4。

#### 20. 风险与注意事项

- 风险：把 optional projection 当成旧 outbox 的必填字段，导致 resume 失败。检测：integration_resume；缓解：serde default + old fixture。
- 风险：AcceptedTransition 已写 receipt 但 ledger projection 尚未 materialize 的 crash window。检测：Unit 4 startup repair；缓解：receipt 携带完整可重建 projection，启动先 repair 再 hydration。
- 剩余风险：EventBus 没有 delivery acknowledgement；本计划只保证 acceptance/outbox/publish 顺序，不新增 exactly-once bus delivery 语义。

### Unit 4：重启 hydration、terminal honored 与全链路回归

#### 1. Unit 目标

在 loop 生命周期入口把 replayed/repair 后的 StateMachine snapshot 注入 LoopState，并把 terminal honored 纳入同一 Ledger 记账语义，证明正常运行与重启后的 StateMachine acceptance state 等价。

#### 2. 对应需求与 Scenario

- Requirement：R1、R5、R8、R7
- Scenario：S2、S6、S8
- Decision：D1、D6、D7、D8
- Evidence：E4、E7、E8、E12、E13、E16

#### 3. 外部可观察结果

同一 workspace 重启 loop 后，StateMachine 不从空状态开始；open/closed instances、accepted count、terminal honored 与第一进程最终状态一致。builtin/default path 结果不变。

#### 4. 当前行为基线

`StateLedger::new` 已 replay 普通 ledger，但 `LoopState::new` 默认 StateMachine runtime None；`acceptance_and_lifecycle` 只 wiring `state_ledger`；`wave_scope` 直接调用 `sm_state.mark_terminal_honored()`，未提交专用 delta。现有 replay tests 不覆盖 StateMachine hydration。

#### 5. 输入与输出

- 输入：已有 `.ralph/ledger.jsonl`、可能存在 StateMachine projection-bearing outbox、StateMachine config、completion state。
- 输出：hydrated `LoopState.state_machine_runtime_state`；repair 后 ledger 与 outbox identity 对齐。
- 错误：repair/replay 失败不得静默构造“已完成” state；遵循现有 ledger warning/cold-start policy，并输出诊断。
- 状态变化：startup 只从 durable source 构造 runtime；terminal honored 通过 CommitDelta 持久化后再更新 live。
- 副作用：可能补写缺失的 ledger projection；不新增 CLI 或 Agent-visible artifact。
- 不变量：首次运行和重启运行的 accepted event decisions 相同；旧 loop 没有 StateMachine delta 仍正常启动。

#### 6. 修改位置

- `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs`：在现有 StateLedger 构造后调用明确的 StateMachine hydration/repair helper；不改变 policy/task/projector 顺序。
- `crates/ralph-core/src/event_loop/lifecycle.rs`：如需 helper，放在已有 StateLedger lifecycle wiring 附近；不新增第二个 ledger。
- `crates/ralph-core/src/event_loop/wave_scope.rs`：在现有 completion honored 成功点提交 StateMachine terminal-honored delta，再更新 live flag；保留原有 `CompletionHonored` commit。
- `crates/ralph-core/src/event_loop/tests/state_machine.rs`：增加 process restart、terminal honored replay、outbox repair 测试。
- `crates/ralph-core/src/event_loop/tests/replay_light_integration.rs`：只在现有真实 lifecycle helper 合适时扩展 restart test。

不修改 data skill；Unit Close 必须执行 data skill drift 检查，确认没有新 agent action。

#### 7. 可依赖能力

- Unit 1 的 replayable StateMachine delta。
- Unit 2 的 final acceptance semantics。
- Unit 3 的 projection-bearing outbox 和 identity。
- 现有 `StateLedger::new`、`acceptance_and_lifecycle`、completion gate。

#### 8. 禁止依赖的未来能力

- 不新增 outbox redrive CLI。
- 不改变 Agent prompt、OPAC、preset instructions。
- 不把 knowledge observation hydration 当成 StateMachine hydration。

#### 9. 验收测试

- `state_machine_runtime_hydrates_after_restart`：第一 EventLoop 接纳 planned/blocked/terminal；丢弃；第二 EventLoop 用同 workspace 初始化；比较 runtime snapshot。
- `state_machine_outbox_projection_repairs_before_hydration`：模拟 outbox 已写、ledger projection 缺失的 crash window；启动后先 repair，再断言 hydrated state 与 receipt 一致且 count 不重复。
- `terminal_honored_is_replayable`：completion check 成功后写 terminal honored；重启后 `is_terminal_honored()` 为 true。
- `legacy_workspace_without_state_machine_delta_starts_cleanly`：只有旧 ledger/outbox，启动不 panic、不伪造 runtime。
- 命令：`cargo nextest run -p ralph-core -- state_machine_runtime_hydrates`、`cargo nextest run -p ralph-core -- replay`、`cargo nextest run -p ralph-core --test replay_light_integration`。

#### 10. Acceptance Red

首先运行 `state_machine_runtime_hydrates_after_restart`。当前第二个 EventLoop 的 `state_machine_runtime_state` 为 None/空状态，或 terminal honored 未恢复；这是有效 Red，因为测试经过真实 lifecycle + StateLedger replay，而非只构造一个内存 struct。

如果失败来自 test workspace 没有真实 `.ralph/ledger.jsonl`、StateMachine config 未启用、或测试未走 `acceptance_and_lifecycle`，先修测试入口，不算实现 Red。

#### 11. 单元测试拆分

1. `hydrate_state_machine_from_ledger_snapshot`：已有 snapshot 有 runtime 时，LoopState 获得等价 clone。
2. `repair_outbox_projection_is_idempotent`：同一 receipt repair 两次只产生一个 ledger projection。
3. `repair_ignores_legacy_outbox_without_projection`：旧 receipt 不伪造 StateMachine state。
4. `terminal_honored_commit_updates_snapshot`：completion accepted 后 delta replay 设置 honored。
5. `completion_rejected_does_not_commit_terminal_honored`：open task/required event reject 时不写 honored delta。
6. `disabled_state_machine_does_not_hydrate_runtime`：None/false 保持 None/旧行为。

不允许 mock lifecycle 的 StateLedger 构造或直接写 `LoopState.state_machine_runtime_state` 代替 restart test。

#### 12. Red → Green → Refactor 顺序

1. Test 1 Red：第二个 EventLoop runtime 为空。
2. 最小实现：lifecycle hydration from ledger snapshot；Test 1 Green。
3. Test 2 Red：outbox-only crash window 启动后 ledger/runtime 不一致；最小实现 startup repair；Test 2 Green。
4. Test 3 Red：terminal honored live true 但 replay false；最小实现 terminal-honored delta at existing completion success point；Test 3 Green。
5. Test 4 Red：旧 outbox/ledger fixture 被错误当成 StateMachine state；加 optional projection/feature guard；Test 4 Green。
6. Test 5 Red：completion rejection 也写 honored；调整提交点只在 `check_completion_event` success branch；Test 5 Green。
7. Refactor：把 hydration/repair 保持为小的 lifecycle helper，注释 durable source/ordering；不扩展到其他 runtime fields。

#### 13. 最小实现范围

- 必须：startup repair/hydration、terminal honored durable delta、restart equivalence、legacy workspace compatibility、disabled path。
- 必须保持：completion gate 顺序、CompletionHonored 原有 delta、policy/task/projector hydration、旧 warning/cold-start policy。
- 必须处理：outbox-only crash window、重复 repair、旧 outbox 无 projection、terminal rejected/accepted 两条路径。
- 不实现：通用 outbox redrive、EventBus acknowledgement、Agent skill/preset 变更。

#### 14. 集成验证

真实联合：`acceptance_and_lifecycle` → `StateLedger::new/replay` → outbox repair → LoopState hydration → `check_completion_event`/`wave_scope` terminal commit。至少运行同一 workspace 两次初始化。

命令：`cargo nextest run -p ralph-core -- state_machine`、`cargo nextest run -p ralph-core -- replay`、`cargo nextest run -p ralph-core --test replay_light_integration`、`cargo nextest run -p ralph-core --test scenarios`。

#### 15. 风险驱动测试

- Differential replay：第一进程 snapshot 与第二进程 snapshot 字段逐项比较，原因是本 Gap 的核心就是 live/replay drift。
- Fault Injection：outbox-only and ledger-only crash window fixtures，原因是跨文件持久化顺序。
- Idempotency：repair twice/restart twice，原因是 outbox/ledger 都是可重复扫描边界。
- Characterization：现有 terminal rejected test，确保不把“observed”误写成“honored”。

#### 16. 回归范围

- 直接：所有 state_machine、state、accepted_transition、disposition、replay-light tests。
- 相邻：`cargo nextest run -p ralph-core --test scenarios`、`cargo nextest run -p ralph-core --features recording --test smoke_runner`。
- CLI consumers：`cargo nextest run -p ralph-cli --test integration_resume`，因为 outbox 是 CLI resume boundary evidence。
- 旧配置/数据：builtin preset、无 state_machine config、旧 ledger/outbox JSONL。
- 默认关闭：所有 state_machine None/false 流程。
- 构建/lint/typecheck：`cargo build`、`cargo clippy`、`cargo fmt --check`；Rust workspace 无单独 TypeScript typecheck 入口，本计划不编造命令。
- 最终全量：`./scripts/run-tests.sh`；必要时 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 只作为真实 race flake 兜底。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs` | 修改现有生产文件 | startup hydration/repair wiring | E13 |
| `crates/ralph-core/src/event_loop/lifecycle.rs` | 修改现有生产文件（如 helper 必要） | 生命周期边界复用 | E4/E13 |
| `crates/ralph-core/src/event_loop/wave_scope.rs` | 修改现有生产文件 | terminal honored durable delta | E12 |
| `crates/ralph-core/src/event_loop/tests/state_machine.rs` | 新增测试 | restart/terminal/repair | E10/E12 |
| `crates/ralph-core/src/event_loop/tests/replay_light_integration.rs` | 仅在真实 lifecycle 入口需要时新增测试 | process restart evidence | E13 |
| `crates/ralph-core/data/*.md` | 预期不修改；执行 drift audit | 无 agent-facing action 变化 | E16 |

#### 18. 完成标准

S2/S6/S8 通过；restart/live/replay 等价；outbox repair 幂等；terminal honored 只有成功 completion 才落账；旧配置/旧数据通过；相关回归、build、lint、全量测试通过；没有新增 skill/preset 变更；所有 Evidence/Decision 更新；Unit 可独立提交。

#### 19. 停止条件

hydration 顺序会改变已有 policy/task/projector state、旧 outbox 无法兼容、repair 需要新增外部服务、terminal rejected 语义无法保持、或全量回归显示默认路径改变时停止；不得通过关闭 StateMachine 或放宽断言让测试通过。

#### 20. 风险与注意事项

- 风险：startup repair 在 hydration 后执行，导致第一次 prompt 看到半状态。检测：repair-before-hydration integration test；缓解：生命周期顺序固定为 ledger replay → outbox repair → StateMachine hydration → prompt/processing。
- 风险：terminal honored commit 与已有 CompletionHonored commit 顺序不一致。检测：replay snapshot comparison；缓解：两者都在现有 successful completion branch 按固定顺序写入，rejected branch不写。
- 风险：全量 workspace dirty changes 与本计划交叠。检测：每个 Unit 开始/结束记录 `git diff --name-only`；缓解：只增量修改计划文件列出的符号，不使用 reset/checkout。
- 剩余风险：EventBus delivery 本身无 ack/Result；本计划不声称解决 delivery exactly-once，只解决 acceptance/state/replay 记账一致性。

## 8. Unit 串行依赖图

```text
Unit 1
  ↓
Unit 2
  ↓
Unit 3
  ↓
Unit 4
```

- Unit 2 使用 Unit 1 已验证的 semantic delta/apply/replay；不能先改 event loop，否则没有可验证的 durable target。
- Unit 3 使用 Unit 2 已验证的 final accepted projection；不能提前接 outbox，否则会把 pre-gate candidate 当 acceptance。
- Unit 4 使用 Unit 3 已验证的 receipt identity/projection；不能先做 hydration，否则 restart 只能猜测 outbox 与 ledger 的关系。
- 每个 Unit 都禁止实现后续 Unit 的行为；例如 Unit 2 不写 outbox projection，Unit 3 不调用 startup repair，Unit 4 不新增 CLI redrive。

## 9. 执行命令清单

以下命令均来自仓库现有配置；命令失败时不得进入下一步。

| 命令 | 时机 | 目的 | 预期 | 失败处理 |
|---|---|---|---|---|
| `cargo nextest run -p ralph-core -- state_machine` | 每个 Unit Red 前后 | StateMachine 直接回归 | 全部通过 | 停止当前 Unit |
| `cargo nextest run -p ralph-core -- replay_from_disk` | Unit 1 | Ledger replay 回归 | 通过 | 停止 Unit 1 |
| `cargo nextest run -p ralph-core -- state` | Unit 1/2 | unified state commit/apply | 通过 | 停止当前 Unit |
| `cargo nextest run -p ralph-core --test replay_light_integration` | Unit 2/4 | 真实 loop/replay path | 通过 | 停止当前 Unit |
| `cargo nextest run -p ralph-core -- accepted_transition` | Unit 3 | outbox/rollback/idempotency | 通过 | 停止 Unit 3 |
| `cargo nextest run -p ralph-core -- disposition` | Unit 3 | Business/Recovery vs diagnostic/control | 通过 | 停止 Unit 3 |
| `cargo nextest run -p ralph-cli --test integration_resume` | Unit 3/4 | 旧 outbox/resume consumer compatibility | 通过 | 停止当前 Unit，复查 H1 |
| `cargo nextest run -p ralph-core --test scenarios` | Unit 2/4 | BDD real runtime paths | 通过 | 停止当前 Unit |
| `cargo nextest run -p ralph-core --features recording --test smoke_runner` | Unit 4 | replay smoke | 通过 | 停止 Unit 4 |
| `cargo fmt --check` | 每个 Unit Close | 格式 | 无 diff | 修复格式后重跑 |
| `cargo build` | 每个 Unit Close | 编译 | 成功 | 停止当前 Unit |
| `cargo clippy` | 每个 Unit Close | lint/type-level regression | 成功 | 停止当前 Unit |
| `./scripts/check-cli-doc-drift.sh` | 确认未改 CLI/data skill 后做 drift audit | 确认命令文档无漂移 | 通过 | 若发现漂移，补文档并重跑 |
| `./scripts/run-tests.sh` | Unit 4 final gate | workspace 全量 nextest + doctest 基线 | 通过 | 修复真实失败；只有确认 race flake 才用 serial fallback |
| `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | 仅全量并发出现已确认时序 flake | 最后兜底 | 通过 | 仍失败则视为真实失败 |

不得使用裸 `cargo test -p ralph-cli`。不得用局部测试替代最终 `./scripts/run-tests.sh`。

## 10. 最终质量门禁

- 所有 S1–S8 有真实可执行测试并通过。
- 所有 R1–R8 至少关联一个 Scenario、一个测试和一个 Unit。
- StateMachine unit、Ledger replay、AcceptedTransition、disposition、EventLoop replay、BDD scenario 全通过。
- 旧 ledger/outbox 数据可读取；新增字段可选；disabled/default path 没有新增 commit、publish 或文件。
- downstream reject、Ledger write failure、AcceptedTransition outbox failure、restart repair、terminal rejected/honored、duplicate transition 均覆盖。
- `cargo fmt --check`、`cargo build`、`cargo clippy` 和 `./scripts/run-tests.sh` 通过。
- 无新增 skipped/ignored/only 测试，无删除/削弱断言，无无解释 snapshot/golden 更新。
- 没有修改 EventBus API，没有引入新依赖，没有修改 OPAC/data skill/preset 拓扑。
- `crates/ralph-core/data/*.md` 已做反向检查；由于没有新增 Agent-facing action，预期不修改；若实现实际新增命令/事件字段，必须停止并补充 skill 计划。
- `skills/ralph-preset-author`、`skills/ralph-preset-review` 影响评估完成；由于不改 preset/config/event schema，预期不修改；若实现改变 event contract，必须补同步。
- 所有 Unit 严格串行关闭，且每个 Unit 都有真实 Acceptance Red、Unit Red、Green、Refactor、Integration、Regression、Close。
- 所有 Decision confidence 最终仍不低于 0.85；D4/D9 若因 H1 或 borrow-boundary 验证下降，计划必须回到 BLOCKED。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每个 Unit 指定真实文件、入口、Red、最小实现边界、命令和完成标准 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D9 已固定方案；D4/D9 有明确 H1、borrow boundary 验证和停止条件 |
| 所有文件和接口是否有代码库证据 | 是 | E1–E17；新增对象明确标为计划新增 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1–D9 为 0.91–0.99；D4/D9 绑定 Unit 3 验证 |
| 是否存在未处理的低置信度假设 | 否 | H1/H2 有验证动作、失败影响和阻塞规则 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 replay、U2 final acceptance、U3 durable receipt、U4 restart hydration |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 有独立测试、命令、回归和 Close |
| 每个 Unit 是否有真实 Red | 是 | 每个 Unit 规定目标缺失能力导致的失败，不接受 fixture/命令错误 |
| 每个 Unit 是否包含回归范围 | 是 | 每个 Unit 第 16 节列出直接、相邻、旧数据、默认路径和构建门禁 |
| 是否存在未来 Unit 依赖 | 否 | 每个 Unit 的“禁止依赖未来能力”已明确；整体只按线性顺序执行 |
| 是否存在泛化任务描述 | 否 | 修改位置、符号、输入输出、断言和停止条件具体化 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 第 5、6、7 节完整映射 |
| 所有关键决策是否有 Evidence | 是 | D1–D9 均引用 E 编号 |
| 计划是否可以严格串行执行 | 是 | 第 8 节固定 Unit 1→2→3→4 |
