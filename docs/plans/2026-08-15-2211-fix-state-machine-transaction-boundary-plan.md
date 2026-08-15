---
type: fix
title: "StateMachine live、ledger 与 replay 的事务边界"
date: 2026-08-15
origin: docs/brainstorms/2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-brainstorm
execution: code
---

# StateMachine live、ledger 与 replay 的事务边界：开发计划

## Goal Capsule

### 0. 计划状态

- 状态：READY。
- 基线：`d0e53e75e0ea078ea9b43afdf8b16adeaee15d87`，分支 `pittcat-dev`，工作树干净。
- 目标：StateMachine 只有在事件真正通过所有 gate 且 durable projection 成功后才改变 live state；重启 replay 与 live 结果一致；失败不能留下“内存已前进、ledger 未提交”的半状态。
- 调查范围：candidate stage、pending-publish survivor 过滤、`StateMachineRuntimeState` validation/projection、`AcceptedTransition` projection commit、outbox repair、snapshot replay、现有 GAP-02 tests/history。
- 已执行验证命令：`rg --files`、`rg` 调用链搜索、`nl` 读取当前实现、`wc -l`、`git log/show` 检查 GAP-02 与 PMI-011 相关提交。
- 尚未执行验证：计划阶段不跑测试、不改生产代码；Executor 必须按第 9 节执行 nextest/build/clippy/full gate。
- 阻塞项：无。关键决策均由当前实现和已有 fault-injection/replay 测试支持，置信度 0.90 以上。

## Product Contract

### 1. 功能目标

- 业务目标：StateMachine 的业务状态、durable ledger/outbox 和重启 replay 必须表达同一组已接受事件。
- 调用方：启用 `event_loop.state_machine` 的 EventLoop；普通 preset、关闭 StateMachine 的 preset、diagnostic/control event 也必须继续可用。
- 当前行为：candidate stage 在 clone 上验证，但 `apply_state_machine_decisions` 会在 projection-aware durable commit 前直接修改 live runtime；`AcceptedTransition` 的 materialize rollback closure 当前为 no-op。另有 survivor 过滤后不重新按最终 surviving batch 做因果验证、projection 从 live flags 而不是 candidate snapshot 取 terminal flags 的风险。
- 目标行为：候选只描述“可能接受”的决策；最终 survivors 经过最终顺序验证后才生成 delta；delta 成功写入 ledger 后再发布并提交 live，ledger 失败时 live 与之前完全相同，outbox repair 仍可恢复一次 durable projection。
- 行为差异：StateMachine enabled 且发生下游拒绝/ledger 故障时，不再把未 durable 的状态暴露给后续事件；disabled/no-projection 路径保持原始 U6/U7/U8 行为。
- 范围：候选因果、terminal observed/honored snapshot、live commit rollback、outbox repair/replay/idempotency 的真实闭环。
- 非目标：不重新设计 StateMachine 配置 DSL；不改变 transition identity wire format，除非测试证明当前 source/semantic identity 直接造成该 P0；不修改普通业务 projector、EventBus API、preset 拓扑；不把所有 EventLoop 状态一次性事务化。
- 输入：JSONL events、StateMachineConfig、当前 runtime snapshot、最终 gate survivors、StateLedger/outbox。
- 输出：`StateMachineDecision`/`StateMachineTransitionDelta`、ledger commit/outbox receipt、bus publish 或确定性错误。
- 状态：只有最终 accepted + durable 的 delta 改变 live/replay state；失败时 live rollback，outbox projection 可留待 restart repair。
- 错误：ledger/outbox commit failure 必须 fail-closed，不 publish success；replay repair 保持现有 idempotent 语义；拒绝事件保留 diagnostic event，不改变 StateMachine live maps。
- 兼容：StateMachine disabled/None、`projection=None`、旧 ledger 无 StateMachine delta、旧 transition identity 必须继续通过现有测试。
- 性能：仍以 semantic delta 持久化，不保存完整 runtime snapshot；candidate clone 只在启用路径使用。
- 安全/权限：无外部权限变化；不得通过恢复/重放绕过 acceptance gate。
- 已确认假设：StateMachine projection 生产路径是 `run_state_machine_candidate_stage` → `apply_state_machine_decisions` → `publish_synthetic_with_state_machine_projection` → `AcceptedTransition`。
- 待验证假设：是否能在不改变 `AcceptedTransition` 公共签名的情况下把 live mutation 延后或提供可逆 snapshot。进入 U3 先用现有 `StateMachineRuntimeState: Clone`、`StateLedger::set_bypass_active_for_test` 和当前 helper 测试确认；若不能，必须在 U3 停止并更新决策，不得临时引入第二套事务框架。

### 4. BDD 行为规格

```gherkin
Feature: StateMachine 的接受、提交与 replay 保持一致

  Background:
    Given StateMachine 已启用并配置 business/terminal topics
    And EventLoop 使用真实 StateLedger 与 EventBus

  Scenario: 下游拒绝不会让后续事件继承被拒绝事件的状态
    Given batch 中先出现 A、再出现 B、再出现 C
    And A 会使 C 可接受，但 A 在下游 gate 被拒绝
    When EventLoop 提交最终 survivors
    Then C 不得以 A 的候选状态被接受
    And live runtime 与 accepted ledger 只包含真正 survivors

  Scenario: terminal observed 与 honored 使用候选后的快照
    Given terminal event 在 candidate stage 被接受
    When 该 delta 被 durable commit 并 replay
    Then live 与 replay 都保留 terminal_observed
    And 只有既有 honor 条件成立时才保留 terminal_honored

  Scenario: ledger projection 失败不污染 live runtime
    Given outbox write 成功但 StateLedger projection commit 被 fault injection 拒绝
    When EventLoop 提交 StateMachine transition
    Then 不发布业务成功事件
    And live runtime 与提交前完全相同
    And outbox 保留 projection 供 repair

  Scenario: restart repair 只应用一次并与 live 等价
    Given 上一进程留下 outbox-only projection
    When 新 ledger repair 后 replay
    Then transition 只应用一次
    And open/closed maps、terminal flags、count 与 durable delta 相同

  Scenario: StateMachine 关闭或无 projection 的旧路径不变
    Given StateMachine disabled 或 projection=None
    When 普通业务、diagnostic、control event 通过 EventLoop
    Then 继续使用既有 outbox/direct channel 语义
    And 不新增 StateMachineTransition commit
```

## Planning Contract

### 2. 代码库现状与证据

#### 2.1 当前实现入口与调用链

- candidate 入口：`crates/ralph-core/src/event_loop/state_machine_stage.rs::run_state_machine_candidate_stage`。它以 clone 累积 validation，并将 accepted candidate 放到 `pending_state_machine_candidates`。
- 最终应用：同文件 `apply_state_machine_decisions`。当前通过 `get_or_insert_with` 取得 live runtime，调用 `project_transition_delta`，随后立即 `live.apply_transition_delta(&delta)`。
- durable 路由：`crates/ralph-core/src/event_loop/parse_and_emit/legacy.rs` 先从 pending candidates 过滤 `survivors`，调用 apply，再把 delta 按 topic/payload 送入 `disposition::publish_synthetic_with_state_machine_projection`。
- durable helper：`crates/ralph-core/src/event_loop/accepted_transition.rs::commit_idempotent_with_state_machine_projection` 顺序为 materialize → outbox → ledger StateMachineTransition → bus publish；ledger 失败时执行 caller rollback closure，但当前 disposition 传入 `|| Ok(Box::new(|| {}))`。
- replay：`crates/ralph-core/src/state/commit.rs::CommitDelta::StateMachineTransition` 与 `crates/ralph-core/src/state/snapshot.rs::LedgerSnapshot::apply_delta` 通过 `StateMachineRuntimeState::apply_transition_delta` 物化并 dedupe。
- 纯状态逻辑：`crates/ralph-core/src/state_machine.rs::validate_event` 会修改 validator receiver；`project_transition_delta` 当前读取 `self.terminal_observed/honored`，而不是接收 candidate snapshot。

#### 2.2 Evidence Ledger

| ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `crates/ralph-core/src/event_loop/state_machine_stage.rs:80-135` | candidate 用 clone 累积验证；但 `survivors` 在 `legacy.rs:3914-3928` 只做 topic/payload 过滤，未对最终 survivor 顺序重新验证 | 必须先修复“最终 survivors 才是验证输入”的因果边界 | 高 |
| E2 | `state_machine_stage.rs:157-197` | `apply_state_machine_decisions` 先拿 live、生成 delta、立即 `live.apply_transition_delta` | live mutation 发生在 durable ledger commit 前，构成 P0 半提交窗口 | 高 |
| E3 | `state_machine_stage.rs:102-110`、`state_machine.rs:258-259,633-672` | candidate 记录 `accepted_at_terminal_observed/honored`，但 `project_transition_delta` 仍读取 live flags；这些字段目前未被 apply 消费 | terminal observed/honored 可能在 live/replay 中丢失；必须把 candidate snapshot 作为 delta 输入 | 高 |
| E4 | `disposition.rs:187-239` | projection path 的 materialize rollback closure 是 no-op | PMI-011 只保护了 ledger/outbox/bus，不会恢复已在 EventLoop live state 中完成的 mutation | 高 |
| E5 | `accepted_transition.rs:527-552` | ledger projection commit 失败时 outbox 保留、rollback、无 bus publish | 该 durable repair 语义应保留；新增 rollback 必须只撤销 live provisional state，不删除 outbox repair 记录 | 高 |
| E6 | `accepted_transition.rs:1941-2055` | 现有 PMI-011 fault test 断言 fail-closed、outbox 保留、repair 成功、第二次 repair 幂等 | U3 必须增强 live runtime 断言，而不是替换已有测试断言 | 高 |
| E7 | `state_machine.rs:693-746`、`state/snapshot.rs` StateMachineTransition 分支 | replay delta 有 transition id/fingerprint dedupe、open/close、terminal flags | 不能改成完整 snapshot；修复必须继续使用 semantic delta 与幂等 replay | 高 |
| E8 | `event_loop/tests/state_machine.rs` U2/U4 测试、`state/tests.rs:2154-2586` | 已有 candidate rejection、disabled path、hydration、terminal delta、legacy ledger、dedupe 测试，但缺最终 survivor 因果和 live rollback 断言 | 新测试应落在现有测试模块，不另起 fake runtime | 高 |
| E9 | Git `ef16fcbe`、`9fef85ef`、`96491cbf`、`1be0eff9`、`55d46dd8` | GAP-02 已连续落地 candidate、projection、repair、fail-closed、幂等修复；当前缺口是这些机制之间的事务边界 | 计划必须是增量修复，不回滚/重写已合并 GAP-02 | 高 |
| E10 | `crates/ralph-core/src/event_loop/accepted_transition.rs` 2058 行、`state_machine_stage.rs` 257 行、`state_machine.rs` 1297 行 | 相关模块均未超过 5000 行 | 可以局部修改；不得把事务逻辑塞进大文件造成新结构债务 | 高 |
| E11 | `event_loop/tests/state_machine.rs` disabled/no-candidate 测试、`accepted_transition.rs` projection=None 测试 | no-projection 路径已有明确“不新增 StateMachineTransition”的断言 | 回归必须保护默认 preset 和非 StateMachine 功能 | 高 |

#### 2.3 受影响范围

- 生产模块：`state_machine_stage.rs`、`state_machine.rs`、`parse_and_emit/legacy.rs`、`disposition.rs`、`accepted_transition.rs`；必要时 `state/snapshot.rs` 只做 delta 接口适配。
- 测试模块：`event_loop/tests/state_machine.rs`、`accepted_transition.rs` 内 tests、`state/tests.rs`、真实 `tests/scenarios.rs`（若现有 scenario 能启用 StateMachine）。
- 配置：仅使用现有 `event_loop.state_machine`，不改 preset 配置。
- 数据：`CommitDelta::StateMachineTransition`、outbox projection、ledger replay；不改 wire shape。
- API：优先保持 `AcceptedTransition` 公共签名；若必须改，只允许 crate 内部增加 rollback/materialization 接口，并对调用方做完整回归。
- 其他功能：`projection=None`、diagnostic/control direct channel、StateMachine disabled、旧 ledger replay。
- 构建：`ralph-core` 首先，随后 workspace/CLI 全量。

### 3. 决策记录与置信度

| ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | 最终 survivors 如何验证？ | 保持累计 candidate；下游过滤后直接复用 decision；对最终 survivor 从 live snapshot 按顺序重新 candidate-validate | 保留早期 candidate 仅作候选，最终 survivor batch 在最终顺序上重新验证并生成 delta；被拒绝项不进入后续状态 | E1、E3、E8 | 直接复用会让被丢弃 A 影响 C；全量重写 parser 超出范围 | 0.93 |
| D2 | terminal flags 从哪里进入 delta？ | 继续读 live；使用 candidate capture；重构 validator 返回完整 immutable delta | 使用已有 `accepted_at_terminal_observed/honored` capture 传入 projection helper | E3、E7、E8 | 已有字段就是为该边界保留；重构 validator 会扩大 API/回归 | 0.95 |
| D3 | durable 失败如何保证 live 回滚？ | 让 ledger commit 后才 apply live；保存 runtime clone 后在失败时恢复；新增事务对象 | 在当前调用链中保存 apply 前 runtime snapshot，projection commit 失败执行精确 restore；outbox 保留 repair | E2、E4、E5、E6 | 先 commit ledger 需解决 ledger delta 与 live 同一结果但会扩大 AcceptedTransition API；新事务对象无现有模式 | 0.90 |
| D4 | 是否改变 projection identity/source？ | 顺便改 identity；保持现状，仅新增测试 | 本计划不改变 identity/source；若 U1/U2 Red 明确证明 source collision 才停止并另立决策 | E9、E10、用户限制“不引入回归” | 当前证据支持事务/因果缺口，不足以把 identity migration 绑定进 P0 修复 | 0.92 |
| D5 | 如何保护普通 preset？ | 把所有事件都走 StateMachine transaction；只在 projection Some 且 enabled 时走新路径 | 只修 StateMachine projection Some 路径；None/disabled/direct channel byte-for-byte regression | E11 | 全局改动会影响默认 preset 和诊断事件 | 0.98 |

没有低于 0.85 的关键决策。D3 的实现边界必须在 U3 Red 前用当前 clone/rollback API 验证；若失败，Unit 3 停止，不得让 Executor 自行换架构。

### 8. Unit 串行依赖图

```text
Unit 1：最终 survivor 的因果一致性
  ↓
Unit 2：candidate terminal snapshot 的 durable projection
  ↓
Unit 3：ledger 失败时 live runtime 回滚与 outbox repair
  ↓
Unit 4：restart/replay/idempotency 及非 StateMachine 回归
```

- U2 必须使用 U1 的最终 survivor 输入，否则 terminal delta 仍可能从错误候选产生。
- U3 必须使用 U2 的完整 delta（含 terminal flags），否则 rollback 后 replay 对比没有完整状态。
- U4 只验证此前三项在重启、重复提交、disabled/no-projection 下的组合行为，不提前加入新语义。

## Implementation Units

### 7. Unit 1：最终 survivor 不再继承被下游拒绝事件的状态

1. Unit 目标：下游 gate 丢弃某个候选后，后续事件必须从“最终已接受状态”重新验证，不能使用被丢弃事件造成的候选状态。
2. 对应：R1、S1、D1、E1/E8。
3. 外部结果：最终 accepted events、live maps、ledger delta 不包含被丢弃事件带来的隐式前置状态。
4. 当前基线：`legacy.rs` 用 topic/payload 过滤 survivors 后直接调用 `apply_state_machine_decisions`；没有最终顺序 revalidation。先写 mixed batch Red。
5. 输入输出：A/B/C batch、一个可使 A 被下游拒绝的 gate；输出只接受真实 survivors；拒绝保持 diagnostic，不污染 live。
6. 修改位置：`state_machine_stage.rs::run_state_machine_candidate_stage/apply_state_machine_decisions`、`parse_and_emit/legacy.rs` survivor→apply 边界、`event_loop/tests/state_machine.rs`。不改 StateMachineConfig DSL、EventBus。
7. 依赖：已有 clone candidate、pending list、真实 gate outcomes、StateMachineDecision。
8. 禁止：不得通过把所有候选都接受来绕过；不得删除 diagnostic reject；不得提前处理 durable rollback。
9. 验收：构造 A accepted candidate、A downstream rejected、C dependent event；断言 C 不以 A 状态通过，live/ledger 仅含最终顺序接受者。
10. Acceptance Red：先跑 mixed survivor test，预期当前实现会让 C 看到 A 的状态或生成错误 delta；若 C 本来就被独立规则拒绝，fixture 无效，必须修正前置状态。
11. 单测：最终 survivor 重新验证；无 survivor；全部 survivors；reject/ignore/diagnostic 不改变 live；disabled passthrough。
12. 顺序：mixed Red→最终 survivor revalidation Green；reject side-effect Red→保持 diagnostic/不改 live Green；全部既有 candidate tests→Refactor。
13. 最小实现：在最终 survivors 边界建立从 live snapshot 开始的顺序 candidate；只把最终 Accept 的 decision/delta 交给 apply。
14. 集成：真实 `process_parse_result` + EventBus + ledger；只 fake gate outcome/fixture，不能 fake StateMachine validator。
15. 风险：State-machine/state-machine test 是 state-machine test；重点覆盖 rejected predecessor、multiple same-key transitions、batch ordering。
16. 回归：`cargo nextest run -p ralph-core -- state_machine`、parse/event policy targeted、disabled/no-candidate、全量 core。
17. 预期文件：`state_machine_stage.rs`、`parse_and_emit/legacy.rs`、`event_loop/tests/state_machine.rs`；不改 preset。
18. 完成：Red→Green→Refactor→integration→regression 全通过，单独可提交。
19. 停止：实际 gate 顺序与证据不符、需要改 parser 公共 API、Red 不是因果缺陷、回归扩大到无 SM path。
20. 风险：重新验证可能改变重复事件的 identity 顺序；检测现有 dedupe 测试，缓解是保持 semantic identity 不变。

### Unit 2：terminal observed/honored 从 candidate 快照进入 durable delta

1. Unit 目标：terminal event 的 observed/honored 状态在 live projection 与 replay 中一致。
2. 对应：R2、S2、D2、E3/E7。
3. 外部结果：accepted terminal event 后 live runtime 和新 ledger replay 都显示相同 terminal flags。
4. 基线：candidate 已保存 `accepted_at_terminal_observed/honored`，但 `project_transition_delta` 从 live 读取；现有手写 delta replay 测试没有覆盖 production candidate path。
5. 输入输出：terminal candidate decision + 两个 flags；输出 delta flags 与 candidate snapshot 相同；拒绝 terminal 不产生 delta。
6. 修改位置：`state_machine.rs::project_transition_delta` 或其调用边界、`state_machine_stage.rs::apply_state_machine_decisions`、`event_loop/tests/state_machine.rs`、`state/tests.rs` 如需补 replay assertion。不改 CommitDelta wire shape。
7. 依赖：U1 的最终 accepted decision 列表。
8. 禁止：不得把 terminal_honored 无条件设 true；不得修改 completion gate 的 honor 条件；不得把 full runtime 放进 delta。
9. 验收：真实 terminal candidate 后检查 generated delta flags、live summary、fresh ledger replay summary。
10. Acceptance Red：现有 production-path regression test 预期看到 delta terminal_observed=false 而 candidate=true；若测试绕过 candidate 直接手写 delta，不算有效 Red。
11. 单测：observed only、observed+honored、neither、rejected terminal、legacy delta default false。
12. 顺序：production candidate flag Red→传递 snapshot Green；replay roundtrip Red→Green；既有 manual delta tests→Refactor。
13. 最小实现：复用已存在两个 capture 字段，将它们明确作为 projection 输入；保持 `StateMachineTransitionDelta` 字段和 serde 形状不变。
14. 集成：真实 EventLoop candidate + StateLedger commit + `StateLedger::new` replay；不 mock projection method。
15. 风险：Differential/live-vs-replay test；因为当前 bug 正是两个路径结果不一致。
16. 回归：`cargo nextest run -p ralph-core -- state_machine state_machine_runtime_hydrates terminal_honored`、state tests、full core。
17. 预期文件：`state_machine.rs`、`state_machine_stage.rs`、现有 state-machine/state tests。
18. 完成：flags、maps、count 的 live/replay 断言均通过。
19. 停止：需要变更 delta serde 字段、旧 ledger 解析失败或 completion 语义被迫改变。
20. 风险：terminal flags 可能由后续 `mark_terminal_honored` 产生；测试必须区分 accepted observed 与 later honored 两个时刻。

### Unit 3：StateLedger projection 失败时回滚 live runtime

1. Unit 目标：outbox 已写但 StateLedger projection commit 失败时，EventLoop live StateMachine 恢复到提交前，且不 publish。
2. 对应：R3、S3、D3、E2/E4/E5/E6。
3. 外部结果：调用返回 commit failure；bus 没有成功事件；live map/flags/count 与 fault 前完全相同；outbox 保留 projection。
4. 基线：`AcceptedTransition` 已 fail-closed，但 disposition 传入 no-op rollback；`apply_state_machine_decisions` 已提前 mutate live。
5. 输入输出：有效 projection + bypass-active ledger；输出 CommitFailed；live unchanged；repair later succeeds once。
6. 修改位置：`disposition.rs::publish_synthetic_with_state_machine_projection`、`parse_and_emit/legacy.rs` apply/commit boundary、`accepted_transition.rs` only if rollback closure contract must carry StateMachine restore；tests in accepted_transition and event_loop state_machine. 不改 outbox retention。
7. 依赖：U1/U2 已验证的最终 candidates 和完整 delta；现有 `set_bypass_active_for_test` fault injection。
8. 禁止：不得删除 PMI-011 断言；不得在失败时删除 outbox；不得 publish 后再补偿；不得用全局重建 EventLoop 作为唯一 rollback。
9. 验收：fault commit 前后比较 runtime summary、open/closed maps、terminal flags、applied IDs、count；检查 bus=0、ledger 无 projection commit、outbox=1；repair 后恰好一次。
10. Acceptance Red：扩展现有 `pmi011_ledger_commit_failure_fails_closed_without_publish`，先断言 runtime unchanged；当前代码会失败，因为 no-op rollback 不恢复 live mutation。若 outbox/bypass 失败先于 apply，则 fault fixture 无效。
11. 单测：rollback on ledger failure；rollback on outbox failure；successful commit keeps live; no projection path unchanged; repair after failure.
12. 顺序：live rollback Red→保存/恢复 snapshot Green；outbox failure Red→不误删/不污染 Green；successful commit Red/Green；repair/idempotency→Refactor。
13. 最小实现：为 projection path 建立 apply 前可恢复的 StateMachine runtime snapshot，并把恢复 closure 真正交给 AcceptedTransition failure path；成功时丢弃 snapshot；不改变 `projection=None`。
14. 集成：真实 ledger fault flag、outbox file、EventBus observer、runtime summary；不得只测 rollback closure 被调用。
15. 风险：Fault Injection 必测，因为问题只在 outbox/ledger split window 触发；同时保留 PMI-011 repair test。
16. 回归：`cargo nextest run -p ralph-core -- accepted_transition state_machine`、state commit tests、full core/CLI。
17. 预期文件：`disposition.rs`、可能的 `accepted_transition.rs`/`legacy.rs` 边界、现有 tests；不新增依赖。
18. 完成：失败 live 不变、outbox repair 一次、成功 publish 一次、无 projection 旧语义全通过。
19. 停止：无法在现有 borrow/ownership 下形成可验证 restore、必须改变 public API、或需要把 ledger commit 改成不可逆多步事务。
20. 风险：rollback snapshot 可能覆盖并发期间无关状态；检测：commit helper 的 exclusive lock/单线程 EventLoop 调用链；缓解：snapshot 只在本次 projection boundary 捕获并按 transition id 精确恢复，不做全 LoopState 替换。

### Unit 4：restart/replay/idempotency 与普通 preset 回归

1. Unit 目标：U1-U3 修复后，重新启动、重复 repair、disabled/no-projection 和 diagnostic/control 路径不回归。
2. 对应：R4、S4-S5、D4/D5、E7/E8/E11。
3. 外部结果：同一 transition 只计数一次；旧 ledger 可打开；无 StateMachine 的 preset 不产生 StateMachineTransition；direct channel 不改变。
4. 基线：已有 U4 hydration/legacy/dedupe 与 projection=None tests；本 Unit 先执行 characterization。
5. 输入输出：旧/新 delta、重复 outbox、disabled config、diagnostic event；输出与既有断言相同。
6. 修改位置：`event_loop/tests/state_machine.rs`、`accepted_transition.rs` tests、`state/tests.rs`；只在实际失败时修改生产代码，禁止借回归 Unit 顺手重构。
7. 依赖：U3 成功后的 rollback/durable boundary。
8. 禁止：不得改变 transition identity、preset schema、EventBus public contract；不得删除老 ledger compatibility tests。
9. 验收：fresh ledger replay 与 live summary 等价；第二次 repair=0；projection None 无 SM commit；disabled path passthrough；diagnostic 无 outbox。
10. Acceptance Red：若 U1-U3 改动破坏旧路径，已有 characterization 应真实 Red；若全量 unrelated failure，记录并停下，不更新 golden。
11. 单测：duplicate transition id/fingerprint、legacy delta default、projection None、disabled、diagnostic/control。
12. 顺序：hydration/dedupe Red→Green；legacy Red→Green；disabled/no-projection Red→Green；full regression→Refactor。
13. 最小实现：只修复由前三 Unit 直接造成的兼容问题；不扩展 StateMachine 功能。
14. 集成：真实 `StateLedger::new` replay、EventLoop disabled config、AcceptedTransition projection None。
15. 风险：Differential test/live-vs-replay；state-machine enabled/disabled matrix；必要时 concurrency 不扩展为新并发模型，只验证现有 lock 不被绕过。
16. 回归：`state/tests.rs`、`accepted_transition`、`event_loop/tests/state_machine`、所有 `ralph-core`/`ralph-cli`、preset lint、全 workspace。
17. 预期文件：只增/改测试，除非由真实 Red 指向前三 Unit 的生产缺陷；不改 presets/manifest/zsh。
18. 完成：所有 old/new/disabled/no-projection/replay 质量门禁通过，可独立提交。
19. 停止：发现 identity migration、公开调用方、preset topology 或 unrelated package 行为变化。
20. 风险：当前部分 StateMachine 只在 opt-in preset 实际启用；必须保留 direct unit/integration coverage，不能以“builtin 没启用”代替生产路径证明。

## Verification Contract

### 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
|---|---|---|---|---|---|
| S1 | rejected predecessor 不影响 dependent survivor | `event_loop/tests/state_machine.rs` | integration/state-machine | causal batch | 否 |
| S2 | terminal flags live/replay 一致 | state-machine + `state/tests.rs` | unit + integration | differential | 否 |
| S3 | ledger fault 后 live 不变、bus=0、outbox retained | `accepted_transition.rs` + EventLoop test | fault-injection integration | rollback | 否 |
| S4 | repair/replay exactly once | `accepted_transition.rs`、`state/tests.rs` | integration | idempotency | 否 |
| S5 | disabled/None/direct legacy 不变 | existing state/disposition tests | characterization/regression | default-path matrix | 否 |

不能只断言错误返回；S3 必须比较提交前后 runtime，S4 必须比较 live/replay summary，S5 必须断言 ledger commit 类型和 bus/outbox 副作用。

### 6. 需求—测试追踪矩阵

| Requirement | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | 最终 survivors 因果一致 | S1 | mixed survivor test | candidate stage | real EventLoop | 否 | E1/E8 |
| R2 | terminal flags durable | S2 | production candidate roundtrip | projection helper | ledger replay | 否 | E3/E7 |
| R3 | durable failure 不污染 live | S3 | PMI-011 extension | rollback snapshot | outbox/ledger fault | 否 | E2/E4-E6 |
| R4 | replay/idempotency/旧路径兼容 | S4-S5 | existing+new characterization | dedupe | StateLedger/EventLoop | 否 | E7/E8/E11 |

## Definition of Done

### 9. 执行命令清单

- U1 Red/Green：`cargo nextest run -p ralph-core -- state_machine`。
- U2：`cargo nextest run -p ralph-core -- state_machine terminal_honored state_machine_runtime_hydrates`；名称过滤若与 nextest 当前版本不匹配，使用已确认的模块/测试过滤，不改裸 cargo test。
- U3：`cargo nextest run -p ralph-core -- accepted_transition state_machine`。
- U4 state replay：`cargo nextest run -p ralph-core -- state`；不得使用裸 `cargo test`。
- 真实 BDD：对确实启用 StateMachine 的 `crates/ralph-core/tests/scenarios/*.yml` 使用 `cargo nextest run -p ralph-core --test scenarios`；若当前 scenarios 没有该 opt-in，必须在现有 harness 中新增最小真实 scenario，不得用 stub。
- CLI 回归：`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`、`cargo nextest run -p ralph-cli --test integration_emit_policy`。
- Build/typecheck：`cargo build --workspace`、`cargo check --workspace`。
- Lint：`cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 最终全量：`./scripts/run-tests.sh`；不能手动 `cargo nextest run --workspace` 替代仓库两阶段入口。

### 10. 最终质量门禁

S1-S5 全通过；PMI-011 旧断言保留且增加 live unchanged；无新增 skip/only、无削弱断言、无无解释 snapshot；StateMachine delta wire shape/旧 ledger/identity 未被无证据改变；disabled/None/diagnostic/direct channel 和所有其他 preset 通过；build/check/clippy/nextest/full scenario 全通过；每个 Unit 按顺序完成完整 Red→Green→Refactor→Integration→Regression→Close；所有决策置信度仍 ≥0.85。

### 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap | 是 | 具体到调用链、现有函数、Red 和 fault 断言 |
| Executor 是否仍需做关键设计决策 | 否 | D1-D5 已给候选、选择和停止条件 |
| 所有文件和接口是否有代码库证据 | 是 | E1-E11；条件新增模块明确标记 |
| 所有关键决策置信度是否 ≥0.85 | 是 | D1-D5 为 0.90-0.98 |
| 是否存在未处理的低置信度假设 | 否 | D3 的实现可行性设为 U3 Red 前验证门，不让后续猜测 |
| 每个 Unit 是否只有一个可观察行为 | 是 | 因果、flags、rollback、兼容分别拆分 |
| 每个 Unit 是否可以独立验证 | 是 | 每 Unit 有 acceptance、unit、integration、regression |
| 每个 Unit 是否有真实 Red | 是 | 绑定当前具体缺陷/已有 fault fixture |
| 每个 Unit 是否包含回归范围 | 是 | 第 16 节逐 Unit 定义 |
| 是否存在未来 Unit 依赖 | 否 | 只有前置 Unit 已验证能力 |
| 是否存在泛化任务描述 | 否 | 文件/函数/输入/断言/命令明确 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 第 5、6 节 |
| 所有关键决策是否有 Evidence | 是 | 第 2.2、3 节 |
| 计划是否可以严格串行执行 | 是 | 第 8 节 |

