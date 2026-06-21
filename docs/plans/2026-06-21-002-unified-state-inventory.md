# U0 Inventory — Unified Orchestrator State（盘点文档）

> 仅盘点,不修改任何代码。本文档配合计划 `docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md` 的 Phase 0 单元（U0）执行,产出 LoopState → LedgerSnapshot 字段映射、分散内存状态源清单、task.resume 引用基线、fixture 路径与 process_parse_result 阶段拓扑,作为后续 U1-U10 重构的事实依据。

- 盘点日期: 2026-06-22
- 盘点执行: U0 agent（只读,无 `cargo build`/`cargo test`）
- 范围: `crates/ralph-core/src/event_loop/loop_state.rs` + 相关分散状态源 + task.resume 引用 + BDD/scenarios/fixtures + process_parse_result 阶段

---

## 1. LoopState 字段映射表

> 表 1.1: `LoopState`（`crates/ralph-core/src/event_loop/loop_state.rs:97-480`）的 75 个 `pub` 字段到目标 `LedgerSnapshot`（U1 待实现）子结构的映射。
>
> 列定义:
> - **进入 ledger**:该字段值需写入 `.ralph/ledger.jsonl` commit log(U1 引入)。
> - **只读外部参数**:loop 启动时的 frozen 配置参数,无需持久化,仅作为 `LedgerSnapshot::config` 子结构传给派生视图。
> - **删除/合并**:进入 ledger 之前就地清理(可降级、合并到更少字段、或彻底删除)。
> - **类别**:6 选 1 — `iter`/`runtime counter`/`runtime cache`/`runtime gate`/`flow control`/`external param`。

| # | 字段名 | 类型 | 定义位置 | 类别 | 进入 ledger | 只读外部参数 | 删除/合并 | 备注 |
|---|--------|------|----------|------|-------------|--------------|-----------|------|
| 1 | `iteration` | `u32` | `loop_state.rs:99` | iter | ✅ | — | — | 唯一 commit 序列号生成器;LedgerSnapshot 顶层字段 |
| 2 | `hat_handoff_seq` | `u32` | `loop_state.rs:106` | runtime counter | ✅ | — | — | handoff 序号,U1 归到 `handoff` 子结构 |
| 3 | `consecutive_failures` | `u32` | `loop_state.rs:108` | runtime counter | ✅ | — | — | termination 触发依据 |
| 4 | `cumulative_cost` | `f64` | `loop_state.rs:110` | runtime counter | ✅ | — | — | USD 累计,只增,`cost_usd` |
| 5 | `started_at` | `Instant` | `loop_state.rs:112` | external param | ❌ | ✅ | — | loop 启动时 Instant,跨 session 不可序列化,派生视图读时使用 session_start_ts |
| 6 | `last_hat` | `Option<HatId>` | `loop_state.rs:114` | runtime cache | ✅ | — | — | 最近 active hat |
| 7 | `consecutive_blocked` | `u32` | `loop_state.rs:116` | runtime counter | ✅ | — | — | 同一 hat 连续 blocked 计数 |
| 8 | `last_blocked_hat` | `Option<HatId>` | `loop_state.rs:118` | runtime cache | ✅ | — | — | — |
| 9 | `task_block_counts` | `HashMap<String, u32>` | `loop_state.rs:120` | runtime cache | ✅ | — | — | per-task thrash 计数 |
| 10 | `abandoned_tasks` | `Vec<String>` | `loop_state.rs:122` | runtime cache | ✅ | — | — | 已放弃 task 列表 |
| 11 | `abandoned_task_redispatches` | `u32` | `loop_state.rs:124` | runtime counter | ✅ | — | — | planner 重发放弃 task 的次数 |
| 12 | `consecutive_malformed_events` | `u32` | `loop_state.rs:126` | runtime counter | ✅ | — | — | 终止 backstop |
| 13 | `consecutive_hard_gates` | `u32` | `loop_state.rs:128` | runtime counter | ✅ | — | — | hard gate 累计 |
| 14 | `completion_requested` | `bool` | `loop_state.rs:130` | flow control | ✅ | — | — | completion 已观察到 |
| 15 | `completion_honored` | `bool` | `loop_state.rs:132` | flow control | ✅ | — | — | completion 已处理(防重入) |
| 16 | `isolated_turn_business_event_accepted` | `bool` | `loop_state.rs:141` | flow control | ✅ | — | — | 隔离模式每 turn 业务事件预算 |
| 17 | `hat_activation_counts` | `HashMap<HatId, u32>` | `loop_state.rs:144` | runtime counter | ✅ | — | — | max_activations 强制 |
| 18 | `exhausted_hats` | `HashSet<HatId>` | `loop_state.rs:147` | flow control | ✅ | — | — | 已发 `*.exhausted` 的 hats |
| 19 | `last_checkin_at` | `Option<Instant>` | `loop_state.rs:151` | external param | ❌ | — | ✅ 删除 | 进程内时间戳,重启后无意义;U1 删除,Telegram 路径由 `last_checkin_ts: Option<String>` 取代 |
| 20 | `last_active_hat_ids` | `Vec<HatId>` | `loop_state.rs:155` | runtime cache | ✅ | — | — | default_publishes 注入依据 |
| 21 | `last_activation_events` | `Vec<Event>` | `loop_state.rs:158` | runtime cache | ✅ | — | — | 最近一次激活的 trigger 事件快照(obligation replay 用) |
| 22 | `seen_topics` | `HashSet<String>` | `loop_state.rs:161` | runtime cache | ✅ | — | — | 整个 loop 周期内所有出现 topic |
| 23 | `last_emitted_signature` | `Option<EventSignature>` | `loop_state.rs:164` | runtime cache | ✅ | — | — | stale loop 检测 |
| 24 | `rejection_retry_counts` | `HashMap<String, u32>` | `loop_state.rs:173` | runtime counter | ✅ | — | — | U2 拒收重试预算;key 形如 `stage:hat:topic:violation` |
| 25 | `scope_violation_circuit_breaker_tripped` | `Option<TerminationReason>` | `loop_state.rs:180` | flow control | ✅ | — | — | 隔离模式 scope 断路器状态 |
| 26 | `rejection_last_iteration` | `HashMap<String, u32>` | `loop_state.rs:185` | runtime cache | ✅ | — | — | 同 key 上次出现 iter(responder dedup) |
| 27 | `recent_rejection_digest` | `BTreeMap<String, RejectionDigestEntry>` | `loop_state.rs:194` | runtime cache | ✅ | — | — | U6 digest,prompt 注入用 |
| 28 | `consecutive_same_signature` | `u32` | `loop_state.rs:197` | runtime counter | ✅ | — | — | stale loop 累计 |
| 29 | `cancellation_requested` | `bool` | `loop_state.rs:200` | flow control | ✅ | — | — | loop.cancel 已收到 |
| 30 | `current_isolated_hat` | `Option<HatId>` | `loop_state.rs:204` | flow control | ✅ | — | — | 隔离模式当前 hat |
| 31 | `workflow_progress` | `WorkflowProgress` | `loop_state.rs:207` | runtime cache | ✅ | — | — | guarded chain 进度,见 §2 |
| 32 | `policy_runtime_state` | `Option<PolicyRuntimeState>` | `loop_state.rs:210` | runtime cache | ✅ | — | — | 事件 policy 状态,见 §2 |
| 33 | `state_machine_runtime_state` | `Option<StateMachineRuntimeState>` | `loop_state.rs:213` | runtime cache | ✅ | — | — | 状态机实例,见 §2 |
| 34 | `state_projection` | `Option<StateProjector>` | `loop_state.rs:220` | runtime cache | ✅ | — | — | 含 tasks_cache + progress_cache,见 §2 |
| 35 | `last_verdict_payload` | `Option<String>` | `loop_state.rs:226` | flow control | ✅ | — | — | 最近 verdict topic payload |
| 36 | `last_verdict_topic` | `Option<String>` | `loop_state.rs:234` | flow control | ✅ | — | — | verdict mirror topic 跟踪 |
| 37 | `last_upstream_verdict_payload` | `Option<String>` | `loop_state.rs:242` | flow control | ✅ | — | — | 上游 verdict,防 mirror 覆盖 |
| 38 | `completion_rejection_signature` | `Option<String>` | `loop_state.rs:245` | runtime counter | ✅ | — | — | completion 拒收签名 |
| 39 | `consecutive_completion_rejections` | `u32` | `loop_state.rs:248` | runtime counter | ✅ | — | — | — |
| 40 | `consecutive_no_progress_turns` | `u32` | `loop_state.rs:256` | runtime counter | ✅ | — | — | U5 stall |
| 41 | `consecutive_steward_activations` | `u32` | `loop_state.rs:265` | runtime counter | ✅ | — | — | U5 progress-steward 累活 |
| 42 | `steward_woken_this_turn` | `bool` | `loop_state.rs:273` | flow control | ✅ | — | — | 防递归 steward |
| 43 | `stall_detector_had_events` | `bool` | `loop_state.rs:282` | flow control | ✅ | — | — | 每 turn 旗标 |
| 44 | `last_rejection_fingerprint` | `u64` | `loop_state.rs:286` | runtime counter | ✅ | — | — | stale-breaker fingerprint |
| 45 | `invariant_violation_count` | `u32` | `loop_state.rs:290` | runtime counter | ✅ | — | — | 不变量违反次数 |
| 46 | `last_invariant_violation` | `Option<String>` | `loop_state.rs:293` | runtime cache | ✅ | — | — | 最近 rule id |
| 47 | `loop_start_sha` | `Option<String>` | `loop_state.rs:302` | external param | ❌ | ✅ | — | 启动时 git HEAD,execution_contract 用 |
| 48 | `review_step_tracker` | `ReviewStepTracker` | `loop_state.rs:305` | runtime cache | ✅ | — | — | U1 per-step review,见 §2 |
| 49 | `handoff_tracker` | `HandoffTracker` | `loop_state.rs:319` | runtime cache | ✅ | — | — | WRC-U4,见 §2 |
| 50 | `flow_lifecycle` | `FlowLifecycleRegistry` | `loop_state.rs:329` | runtime cache | ✅ | — | — | U6 flow phase 跟踪,见 §2 |
| 51 | `stall_recovery_counts` | `HashMap<String, u32>` | `loop_state.rs:332` | runtime counter | ✅ | — | — | stall_no_events 累活 |
| 52 | `pending_recovery_hat` | `Option<HatId>` | `loop_state.rs:350` | flow control | ✅ | — | — | 下次 hard gate/wave recovery 钉死 hat |
| 53 | `pending_synthesizer_timeout` | `Option<String>` | `loop_state.rs:364` | flow control | ✅ | — | — | R1 一次性 wave_id 钉子 |
| 54 | `last_ephemeral_relocations` | `Vec<RelocationRecord>` | `loop_state.rs:372` | runtime cache | ✅ | — | — | R3 ephemeral 隔离 |
| 55 | `bootstrap_complete` | `bool` | `loop_state.rs:382` | flow control | ✅ | — | — | coordinator bootstrap 一次信号 |
| 56 | `bootstrap_failed` | `bool` | `loop_state.rs:393` | flow control | ✅ | — | — | coordinator bootstrap 失败信号 |
| 57 | `recoverable_exhaustion_buffer` | `Vec<RecoverableExhaustion>` | `loop_state.rs:400` | flow control | ✅ | — | — | U2 recoverable 桶溢出 |
| 58 | `work_done_seen_tasks` | `HashSet<String>` | `loop_state.rs:410` | runtime cache | ✅ | — | — | U4 work.done dedup |
| 59 | `hat_activation_at` | `HashMap<HatId, Instant>` | `loop_state.rs:428` | external param | ❌ | — | ✅ 转换 | Instant 不可序列化;U1 改为 `hat_activation_at: HashMap<HatId, DateTime<Utc>>`,入 ledger |
| 60 | `pending_obligation_triggers` | `Vec<Event>` | `loop_state.rs:443` | runtime cache | ✅ | — | — | R4+R5 trigger replay 快照 |
| 61 | `pending_lint_resume` | `Option<LintResumeHint>` | `loop_state.rs:455` | runtime cache | ✅ | — | — | U4b lint resume hint,见 §2 |
| 62 | `consecutive_engine_gate_rejections` | `u32` | `loop_state.rs:469` | runtime counter | ✅ | — | — | 引擎 gate 累活(2026-06-20-001 KTD-7) |
| 63 | `lint_circuit_breaker_tripped` | `bool` | `loop_state.rs:479` | flow control | ✅ | — | — | 引擎 gate 断路器 latch |

> 字段 #5 (`started_at`) 与 #19 (`last_checkin_at`) 均为 `Instant`,与 #59 (`hat_activation_at` HashMap<HatId, Instant>) 在 U1 迁移时统一转换为 RFC3339 字符串持久化,运行时视图可选用 `DateTime<Utc>`/`Instant` 内部表示。
> 字段 #47 (`loop_start_sha`) 与启动时其他 git metadata 一起归入 `LedgerSnapshot::startup` 子结构,无需单条 commit,只在 `bootstrap` 阶段写入。
> 字段 #34 (`state_projection`) 内部包含 `tasks_cache: Vec<Task>` + `progress_cache: ProgressSnapshot`(见 `crates/ralph-core/src/state_projector/mod.rs:87-115`),这两项是 U1-U2 目标派生视图的原始数据源,需要在 ledger commit log 中显式建模 `task` 与 `progress` 事件类型(与现 `tasks.jsonl` / `progress.md` 同形)。
> 字段 #1 (`iteration`) 是 commit log 的隐式序号;LedgerSnapshot 的 `version` 字段应等于其值。
> 字段 #20-21 (`last_active_hat_ids` / `last_activation_events`) 是 obligation replay 的关键,U1 持久化后供 U7a/U7b 的 `replay_obligation_triggers_to_activation_state` 在恢复时重建。

---

## 2. 分散内存状态源清单

> 下列子结构分别独立定义在 `ralph-core` 多个模块中,目前通过 `LoopState` 字段持有;U1 阶段应整体进入 `LedgerSnapshot` 的对应子结构,不再由 `LoopState` 直接 `pub` 持有。

| 状态子结构 | struct 定义位置 | 当前 LoopState 字段 | 主要写入路径 | 主要读取路径 | U1 ledger 子结构建议 |
|-----------|----------------|---------------------|---------------|---------------|---------------------|
| **tasks_cache** | `state_projector/mod.rs:111` (字段在 `ProjectionContext`) | 间接通过 `state_projection` | `state_projector/task.rs::project`、`StateProjector::bootstrap_from_disk`(`mod.rs:122-145`) | `state_projector/progress.rs:39,46-51,57`、`runtime_state.rs:91-103` | `LedgerSnapshot::task::Ledger` (直接对应现 `tasks.jsonl`) |
| **progress_cache** | `state_projector/mod.rs:114` (字段在 `ProjectionContext`,类型 `ProgressSnapshot`) | 间接通过 `state_projection` | `state_projector/progress.rs::push_completed / set_current` | `state_projector/progress.rs`、`runtime_state.rs:102-103` | `LedgerSnapshot::progress::Ledger` (对应现 `progress.md`) |
| **WorkflowProgress** | `loop_state.rs:601-605` | `loop_state.workflow_progress` (`loop_state.rs:207`) | `event_loop/mod.rs:8716` 通过 `apply_workflow_guard_validation` 写入 | `event_loop/mod.rs:7076,1009-1010,8718,8740`、`loop_state.rs::compute_progress_fingerprint` | `LedgerSnapshot::workflow::Ledger { chains: HashMap<ChainName, Vec<WorkflowInstanceProgress>> }` |
| **ReviewStepTracker** | `event_loop/review_step_state.rs:58` (字段 `steps: HashMap<StepKey, StepReviewState>`) | `loop_state.review_step_tracker` (`loop_state.rs:305`) | `event_loop/mod.rs:8153, 8174, 8512, 9982, 10005`、`apply_step_handoff_gate`(`mod.rs:8680`) | `apply_workflow_guard_validation`(`mod.rs:8718`)、`publish_policy_rejection_resume`(`mod.rs:402`)、`StepKey::plan_gate_step_gate` | `LedgerSnapshot::review::Ledger { steps: HashMap<StepKey, StepReviewState> }` |
| **StepKey** | `event_loop/review_step_state.rs:21-29` | (作为 ReviewStepTracker key) | `step_key_from_event`(`review_step_state.rs:87-124`) | 同上 | 跟随 ReviewStepTracker,作为子 key |
| **HandoffTracker** | `workflow_contract/handoff_tracker.rs:97` (字段 `pending: HashMap<event_id, PendingHandoff>` + `escalations: Vec<HandoffEscalation>`) | `loop_state.handoff_tracker` (`loop_state.rs:319`) | `event_loop/mod.rs:1952, 2092, 3578, 4129, 8231, 8506` (`on_handoff_accepted` / `on_hat_activated` / `cancel_pending`) | `event_loop/mod.rs:6573-6589` (`harness hard_gate::route_targeted_task_resume`)、`tests/recovery_envelope_u7_u8.rs:284, 389, 407` | `LedgerSnapshot::handoff::Ledger { pending: ..., escalations: ... }` |
| **PolicyRuntimeState** | `event_policy.rs:255-275+` (字段 `terminal_observed`、`observed_topics`、`completion_honored`、`current_plan_name`、`work_done_seen_topics`) | `loop_state.policy_runtime_state` (`loop_state.rs:210`) | `event_loop/mod.rs:2992-3001, 8106, 8175, 8310, 8740-8746, 9974, 10006` (`apply_event_policy_validation`) | `event_policy.rs::validate_event`、`apply_event_policy_validation`(`mod.rs:1169`) | `LedgerSnapshot::policy::Ledger` |
| **StateMachineRuntimeState** | `state_machine.rs:59-73` (字段 `open_instances`、`closed_instances`、`terminal_observed`、`terminal_honored`、`last_terminal_rejection`、`accepted_transition_count`) | `loop_state.state_machine_runtime_state` (`loop_state.rs:213`) | `event_loop/mod.rs:8369-8372, 8422` (`sm_state.validate_event`) | `state_machine::validate_event`、`compute_progress_fingerprint`(`loop_state.rs:1013-1017`) | `LedgerSnapshot::state_machine::Ledger` |
| **StateProjector (含 ProjectionContext)** | `state_projector/mod.rs:195` (字段 `ctx: ProjectionContext`);`ProjectionContext` 在 `mod.rs:87-115` | `loop_state.state_projection` (`loop_state.rs:220`) | `event_loop/mod.rs:8587-8603` (`get_or_insert_with` 初始化 + `bootstrap_from_disk`)、`state_projector::apply` | `event_loop/mod.rs:5775` (`build_orchestrator_context` 读取)、`runtime_state.rs:91-103` (`ctx.tasks_cache` / `ctx.progress_cache`) | 自身作为子结构被 tasks_cache/progress_cache 取代,U1 不再需要 `ProjectionContext` 包装层 |
| **FlowLifecycleRegistry** | `flow_lifecycle.rs:220` (字段 `records: HashMap<EventId, FlowLifecycleRecord>`) | `loop_state.flow_lifecycle` (`loop_state.rs:329`) | `event_loop/mod.rs:3418, 3466`(incomplete wave gate 读) | `event_loop/mod.rs:6573-6589`(harness hard_gate 取 record)、`hard_gate::should_gate_missing_events` | `LedgerSnapshot::flow::Ledger { records: HashMap<EventId, FlowLifecycleRecord> }` |
| **RejectionDigestEntry** | `loop_state.rs:88-93` | 间接通过 `recent_rejection_digest: BTreeMap<...>` (`loop_state.rs:194`) | `loop_state::record_rejection_digest`(`loop_state.rs:734-760`) | `loop_state::format_rejection_digest_block`(`loop_state.rs:764-786`)、`build_prompt::prepend_rejection_digest`(`mod.rs:5746`) | `LedgerSnapshot::rejection::Digest { entries: BTreeMap<...> }` |
| **LintResumeHint** (经 `pending_lint_resume`) | `preset/engine/hint.rs`(间接,被 `loop_state.pending_lint_resume` 持有) | `loop_state.pending_lint_resume` (`loop_state.rs:455`) | `event_loop/mod.rs:6086` (`engine_required_field_gate` 写入)、`commands/emit.rs:288, 301` (`write_pending_lint_resume`) | `event_loop/mod.rs:5880-5925` (`inject_pending_lint_resume` 一次性消费) | `LedgerSnapshot::lint::Resume { hint: Option<LintResumeHint> }`(与 R9 兼容:不写入 recovery.jsonl,但写入 ledger) |
| **RecoveryResponder (内部 `last_hard_escalations`、`pending_findings`、`tracked_keys`)** | `diagnosis/responder.rs` | 由 `EventLoop::recovery_responder` 持有(`mod.rs:5188-5195`),**非 LoopState 字段** | `record_finding`(`responder.rs:441`)、`begin_iteration` | `drain_hard_escalations`(`responder.rs:379`)、`peek_termination_hint`、`take_termination_hint` | U1 应在 `LedgerSnapshot` 新增 `recovery::Ledger` 子结构,迁出 `RecoveryResponder` 的 in-memory 状态 |
| **DriftEngine (内部 `last_actions: VecDeque<RecoveryAction>`、各种 metric observer)** | `drift/engine.rs`、`drift/detector.rs` | 由 `EventLoop::drift_engine` 持有(非 LoopState 字段) | `DriftEngine::drain_observer` | `drain_hard_escalations`(`drift/engine.rs:217`) | U1 同样迁出至 `LedgerSnapshot::drift::Ledger` |
| **DriftResponder (内部 `pending_findings: Vec<Envelope>`、`last_hard_escalations`、`tracked_keys`)** | `diagnosis/responder.rs` | 由 `EventLoop::recovery_responder` 持有 | `record_finding`、`classify` | `drain_hard_escalations` | 同上 |
| **DiagnosticsCollector 内部** | `diagnostics/mod.rs` | 由 `EventLoop::diagnostics` 持有(非 LoopState 字段) | `record_recovery_envelope`(`mod.rs:5156-5175`)、`log_orchestration`、`log_recovery` | `ralph diagnose` CLI、`tests/diagnostics/*` | 独立保留;LedgerSnapshot 持有 `last_session_id` 关联即可 |
| **Harness Extension `runtime_state`** | `state_projector/mod.rs` 配合 `runtime_state.rs` | 由 `EventLoop::runtime_state` 持有(经 `RuntimeContext` 装配) | `bootstrap_from_disk`、`RuntimeContext::apply_event` | `state_projector/progress.rs`、`runtime_state.rs::RuntimeState::apply` | U1 迁出后由 `LedgerSnapshot::task::Ledger` + `LedgerSnapshot::progress::Ledger` 替代 |
| **ExhaustedHats / HatActivationCounts** | (仅 `HashSet<HatId>` / `HashMap<HatId, u32>` 原语) | `loop_state.exhausted_hats`、`hat_activation_counts` | `hat_handoff/gate.rs` (Reject 路径)、`process_parse_result` 中 `event.malformed` 处理 | `event_loop/hat_handoff` 校验、`max_activations` 强制 | `LedgerSnapshot::hat_activation::Ledger` |

> 表 2.1 关键观察:
> 1. `tasks_cache` / `progress_cache` 实际并不在 `LoopState` 字段中,而是嵌在 `StateProjector` 内部的 `ProjectionContext`(`state_projector/mod.rs:111-114`);只有 `state_projection: Option<StateProjector>` 才是 `LoopState` 字段。U1 应把它们提升为 ledger 一等公民。
> 2. `RecoveryResponder` / `DriftEngine` / `DiagnosticsCollector` 当前都不在 `LoopState` 字段中,各自由 `EventLoop` 字段持有;U1 设计 `LedgerSnapshot` 时仍需将它们的"持久化相关状态"迁出(详细子结构见上表最右列),不能漏。
> 3. `LintResumeHint`(经 `pending_lint_resume`)在原 plan 中明确不写入 `recovery.jsonl`(R9),但作为 `LoopState` 字段需要入 ledger —— U1 ledger 写入策略要保留"不进 recovery journal"的语义,可通过 ledger 文件名/前缀区分。

---

## 3. task.resume 生产调用点清单

> 范围:本仓库生产代码路径(非测试、非 BDD scenarios)内所有"**实际**产生一个会被 bus publish 到 events.jsonl 的 `task.resume` 事件"的代码位置。
> 排除:仅在 doc 注释、字符串字面量、配置字段中出现的引用(那些由 §4 的测试覆盖)。

| # | file:line | 函数/上下文 | 触发条件 | 替代策略建议(U7a/U7b) |
|---|-----------|-------------|----------|------------------------|
| 1 | `crates/ralph-core/src/event_loop/mod.rs:489` | `publish_policy_rejection_resume` 内部 | 被 U1 拒收路径调用 8 处(见下) | 迁出,改为在 `StateLedger::commit(Rejection)` 后由 responder 统一发布;payload 由 ledger commit 派生,不再重复构建 |
| 2 | `crates/ralph-core/src/event_loop/mod.rs:2801` | `check_completion_event` —— missing required events 拒收 | LOOP_COMPLETE 时 event chain 仍有未完成 topic | 由 `TerminationReason::RecoverablePayloadExhausted` 转换(同 plan U6/U7 提议);不直接 publish task.resume,改由 ledger 记录 + 一次性 human.guidance |
| 3 | `crates/ralph-core/src/event_loop/mod.rs:2856` | `check_completion_event` —— verdict gate 失败 | gate 期望 fail 字段,但最近 verdict payload 是 pass | 同 #2 |
| 4 | `crates/ralph-core/src/event_loop/mod.rs:2890` | `check_completion_event` —— workflow guard incomplete | workflow guard chain 仍有 open instance | 同 #2 |
| 5 | `crates/ralph-core/src/event_loop/mod.rs:2918` | `check_completion_event` —— persistent mode | 持久模式抑制 LOOP_COMPLETE | 改为:publish `task.continue` 或 `idle.prompt` 一次(语义更清晰),不进 task.resume 桶 |
| 6 | `crates/ralph-core/src/event_loop/mod.rs:2966` | `check_completion_event` —— open tasks 拒收 | runtime tasks 仍有 open | 由 `state.tasks_cache` 派生判定;**确定性 correction**(U7a 提议)取代 retry |
| 7 | `crates/ralph-core/src/event_loop/mod.rs:3586` | `inject_review_aggregate_timeouts` —— review-synthesizer 超时 | wave 维度超时未聚合完成 | U5 ladder 已分 3 阶;U7a 建议 3 阶末端转 `loop.stalled` 终止,不 retry |
| 8 | `crates/ralph-core/src/event_loop/mod.rs:3712` | `inject_fallback_event` —— HARD stall (3+ 阶) | stall 计数 ≥ 3 | 转 `TerminationReason::StallRecovered` |
| 9 | `crates/ralph-core/src/event_loop/mod.rs:3762` | `inject_fallback_event` —— 上一 hat stall | `state.last_hat` 非 ralph | 改为 `loop.resume(last_hat)` 命令,U7a |
| 10 | `crates/ralph-core/src/event_loop/mod.rs:3786` | `inject_fallback_event` —— ralph fallback | `state.last_hat == None/ralph` | 同 #8 |
| 11 | `crates/ralph-core/src/event_loop/mod.rs:6545` | handoff_tracker 周期滴答 —— dispatch timeout | 周期滴答发现 handoff 过期 | 合并到 #8;U7a deterministic correction 路径 |
| 12 | `crates/ralph-core/src/event_loop/mod.rs:7462` | `process_parse_result` —— isolated_anonymous_business_topic | ralph pseudo-hat 无 provenance | 由 `state_projection` 拒收后一次性 human.guidance 取代(U4 review P0-1/P0-2 已修) |
| 13 | `crates/ralph-core/src/event_loop/mod.rs:7822` | `process_parse_result` —— isolated scope_violation | isolated 模式 hat 超范围 publish | 合并到 responder 统一发,U7a |
| 14 | `crates/ralph-core/src/event_loop/mod.rs:8896` | `process_parse_result` —— execution_contract 拒收(contract_rejections) | contract rule reject | 合并到 responder 统一发 |
| 15 | `crates/ralph-core/src/drift/engine.rs:441` | `publish_hard_recovery_event` | `RecoveryAction` hard escalation,被 `runner.rs:3582` 调 | U7a 改为在 ledger `recovery::HardEscalation` commit 后由 responder 派生一次 |
| 16 | `crates/ralph-core/src/event_loop/mod.rs:8410-8570` 范围 | `apply_event_policy_validation` 路径下游(`publish_policy_rejection_resume` 在 771/1296/1319/1405/1427/1477/1515/1617/1651 调用,8 个分支) | policy gate reject(U1, U3, U4, R5, monotonicity 等) | 同 #1,迁至 responder 统一 |

> 表 3.1 关键观察:
> - 16 个生产调用点,全部在 `event_loop/mod.rs` + `drift/engine.rs` 两个文件中。
> - 主导模式有 3 类:
>   (a) 拒收重试(`publish_policy_rejection_resume` 家族,8 个分支 + #15 + #16)— 计划 U2 已落,正向"bounded retry"收敛;
>   (b) stall/timeout/fallback(`inject_fallback_event` 家族,3 个分支 + #7 + #11)— 计划 U5 已有 ladder,U7a 拟终结;
>   (c) completion 拒收(`check_completion_event` 家族,5 个分支)— U7a 拟转为 deterministic correction。
> - 没有 `crates/ralph-cli/src/loop_runner/runner.rs` 直接 publish `task.resume` 的生产路径(只有 envelope + drift engine 间接驱动)。

---

## 4. task.resume 测试/BDD 引用清单

> 范围:所有测试文件 + BDD scenarios YAML 中对 `task.resume` 的引用,按文件分类。

| # | file:line | 类型 | 断言内容 |
|---|-----------|------|---------|
| 1 | `crates/ralph-core/src/event_loop/loop_state.rs:1102-1104` | 单元测试 | 反复 `task.resume` 触发 stale_loop 累计 |
| 2 | `crates/ralph-core/src/event_loop/tests/workflow_guard.rs:545,549,554,560,564,568,672,676,744,924,959,964,990,993` | 单元测试 | workflow guard 拒收时 `task.resume` 出现次数、payload 包含 next-expected topic |
| 3 | `crates/ralph-core/src/event_loop/tests/r5_hard_gate_routing.rs:4` | 单元测试(doc) | hard gate routing 描述 |
| 4 | `crates/ralph-core/src/event_loop/tests/text_fallback.rs:62,96` | 单元测试 | completion 拒收注入 task.resume |
| 5 | `crates/ralph-core/src/event_loop/tests/replay_light_integration.rs:487` | 单元测试(doc) | targeted retry 描述 |
| 6 | `crates/ralph-core/src/event_loop/tests/chain_validation.rs:144,147` | 单元测试 | 拒收时 task.resume 已 publish |
| 7 | `crates/ralph-core/src/event_loop/tests/handoff_dispatch.rs:9,91,111,117,119,161` | 单元测试 + doc | handoff_dispatch_timeout payload 构造 |
| 8 | `crates/ralph-core/src/event_loop/tests/event_policy.rs:166,171,177,178,274,280,281,295,351,371,381,386,387,391,443,444,455,486,495,514,578,605,610,614,618,628,689,693,823` | 单元测试 | U1/U2/R5/monotonicity 拒收时 task.resume routing 与次数 |
| 9 | `crates/ralph-core/src/event_loop/tests/stale_breaker.rs:8,17,33,242,393` | 单元测试 + doc | stale_breaker 注入 task.resume |
| 10 | `crates/ralph-core/src/event_loop/tests/recovery_envelope_u7_u8.rs:535,613,696` | 单元测试(doc) | U7/U8 envelope + 软路径 |
| 11 | `crates/ralph-core/src/event_loop/tests/persistent_mode.rs:36,42,43` | 单元测试 | persistent 模式注入 task.resume |
| 12 | `crates/ralph-core/src/event_loop/tests/build_prompt.rs:391,413,422,423,467` | 单元测试 | "收到 task.resume 时" R0 anchor |
| 13 | `crates/ralph-core/src/event_loop/tests/termination.rs:172,192,200,201,591,636,706` | 单元测试 | completion 拒收注入 task.resume、task.resume 在 LOOP_COMPLETE 后仍能 publish |
| 14 | `crates/ralph-core/src/event_loop/tests/origin_guard.rs:425,669,698,701` | 单元测试 + doc | orchestrator control topic 含 task.resume |
| 15 | `crates/ralph-core/src/event_loop/tests/isolated_complex_regression.rs:10,140,519,545,580,594,944` | 单元测试 | isolated 模式 targeted task.resume 路由 |
| 16 | `crates/ralph-core/src/event_loop/tests/topic_format_recovery.rs:118,120,157,168,184` | 单元测试 | R10 topic-format non-retryable 不发 task.resume |
| 17 | `crates/ralph-core/src/event_loop/tests/progress_steward.rs:88` | 单元测试 | steward hat publishes 列表含 task.resume |
| 18 | `crates/ralph-core/src/event_loop/tests/loop_context.rs:123,138,172` | 单元测试 | triggers=["task.resume"] pending 检查 |
| 19 | `crates/ralph-core/src/event_loop/tests/state_machine.rs:214,331` | 单元测试 | completion 拒收注入 task.resume |
| 20 | `crates/ralph-core/src/event_loop/tests/task_resume_ttl.rs:1,5,8,69,80,89,91,102,111,117,141,153,189,193,223,227,257,261,271,278,340,344,349,405,417,427,480,484,490,550,559` | 单元测试(30 处) | TTL 300s 默认、stale/fresh 过滤、未来 ts 拒收 |
| 21 | `crates/ralph-core/src/event_loop/tests/active_hat.rs:448,458,467,483,493,496,503` | 单元测试 + doc | active hat 选择忽略 fallback task.resume |
| 22 | `crates/ralph-core/src/event_loop/tests/serial_lint.rs:57,88,90,102,128,129,134,135,149,175,176,182,193,229,239,240` | 单元测试 | U4b pending_lint_resume 注入与 consume 行为 |
| 23 | `crates/ralph-core/tests/scenarios.rs:831,1588` | BDD runner (doc) | dispatcher task.resume + recovery wiring 描述 |
| 24 | `crates/ralph-core/tests/smoke_runner.rs:184,221` | 集成测试 | smoke chain 描述、task.resume 标记 |
| 25 | `crates/ralph-cli/src/loop_runner/tests.rs:7752,7762,8270,8413,9852,9867,10243,10247,10322,10337,10344,10345,10358,10359,10362,10364,10383,10421,10441,10442,10454,10455,10458,10460,10487,10511,10524,10527,10534,10535,10581,10611,10619,10623,10663,10749,10767,10769,10775,10776,10780,13200,13214,13542` | 集成测试(45+ 处) | U3 swap、hard gate 写 task.resume、U2 retry、U4 recovery 块等 |
| 26 | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:543,548,572,607,620,644,652,1090,2889,2892,2902,2919,3015,3020,3022,3046,3067,3166,3169,3174,3178,3214` | 集成测试(20+ 处) | U5/R5 dimension retry budget、pre-rendered task.resume |
| 27 | `crates/ralph-cli/tests/ce_executor_recovery.rs:332,390` | 集成测试 | no target → no task.resume; payload 携带 violation + allowed |
| 28 | `crates/ralph-cli/tests/integration_resume.rs:10,187,196,197,260,278,286,293,294` | 集成测试 | `--continue` 发 task.resume、事件文件含 task.resume |
| 29 | `crates/ralph-cli/tests/integration_agent_reference.rs:318,326,327` | 集成测试 | ralph-tools 文档 R0 anchor |
| 30 | `crates/ralph-cli/tests/integration_emit_policy.rs:588,633,1206,1219,1231,1238,1239,1682,1806,1823` | 集成测试 | T1.7 control topic 允许、isolated+no-hat+task.resume |

> 表 4.1 关键观察:
> - 总计 ~30 个独立测试文件,涵盖 ~150+ 处 `task.resume` 引用。
> - 主要测试集中地:
>   - `event_loop/tests/event_policy.rs`(30+ 处,U1/U2/R5 policy gate)
>   - `event_loop/tests/task_resume_ttl.rs`(30+ 处,U3 TTL)
>   - `loop_runner/tests.rs`(45+ 处,hard gate、U3 swap、U4 block)
>   - `loop_runner/wave/dispatcher.rs`(20+ 处,dimension retry)
>   - `loop_runner/wave/io.rs`(3 处 pre-rendered JSONL)
>   - `loop_runner/wave/dispatcher.rs::tests` 子模块内 6 个测试函数
> - 测试 BDD 场景引用集中于 §5 列出的 ce_executor_* 文件。
> - `ralph-tui/src/state.rs:451` 有 1 处 `task.resume` 字符串分支(TUI 渲染层,不算生产 publish,但属消费侧)。

---

## 5. 基线 fixture 路径

> 现有 smoke replay / BDD scenario 路径,作为 U0 录制基线的来源。`ce-executor-serial` / `ce-executor-isolated` / `ce-executor-wave` 三个 preset 的覆盖情况一览。

### 5.1 现有 fixture 目录树

```
crates/ralph-core/tests/
├── scenarios.rs                          # BDD scenario runner
├── smoke_runner.rs                       # smoke replay runner
├── fixtures/
│   ├── basic_session.jsonl
│   ├── claude_complex_session.jsonl
│   ├── flow_reliability/
│   │   └── zippy-sparrow-4of11-stall.jsonl
│   ├── kiro/                             # adapter 特定
│   ├── kiro-acp/
│   ├── noble-peacock-review-stall/
│   │   ├── replay.jsonl
│   │   └── topology.yml
│   ├── policy_schemas/
│   │   └── ce_executor_session_b_policy_violations.jsonl
│   ├── recovery/
│   │   └── ce-executor-rejected-event-recovery.jsonl
│   ├── rpc-v1/
│   ├── skills/
│   └── wave-isolated-dimension-done/
│       ├── 8-dimension-done.jsonl
│       └── topology.yml
└── scenarios/
    ├── autoresearch_guard.yml
    ├── ce_executor_bootstrap_recovery.yml
    ├── ce_executor_recovery.yml
    ├── ce_executor_serial_fix_applied_rereview.yml
    ├── ce_executor_serial_review.yml
    ├── ce_executor_serial_review_silent_reviewer_recovers.yml
    ├── ce-executor-worktree-isolation.yml
    ├── default_publishes.yml
    ├── flow_reliability/
    │   ├── incomplete_wave_plan_blocked.yml
    │   ├── review_passed_while_wave_open.yml
    │   └── wave_dimension_mismatch_retry.yml
    ├── four-p0-guards/
    │   ├── u1-partial-wave-dispatch.yml
    │   ├── u2-ralph-pseudo-hat-rejection.yml
    │   ├── u3-topic-deny-rule.yml
    │   └── u4-plan-name-equality.yml
    ├── hat_handoff/
    │   ├── disabled_passthrough.yml
    │   ├── dual_publish_work_ready_only.yml
    │   ├── macro_handoff_inject.yml
    │   ├── micro_edge_exempt.yml
    │   ├── next_rejected.yml
    │   └── work_done_rejected_blocks_projection.yml
    ├── hat_lifecycle_contract.yml
    ├── isolated_boundary_violation.yml
    ├── isolated_multi_hat.yml
    ├── isolated_with_event_projection.yml
    ├── mixed_backends.yml
    ├── multi_hat_isolation_lint.yml
    ├── multi_hat.yml
    ├── orphaned_events.yml
    ├── plan_gate_dual_publish_handoff.yml
    ├── plan_gate_dual_publish_inverse_rejected.yml
    ├── plan_gate_dual_publish_third_blocked.yml
    ├── preset_static_lint.yml
    ├── serial_lint/
    │   ├── assert_state_harness_smoke.yaml
    │   ├── serial_lint_1_internal_source_bypass.yaml
    │   ├── serial_lint_10_circuit_breaker.yaml
    │   ├── serial_lint_11_isolated_unaffected.yaml
    │   ├── serial_lint_2_rejection_digest.yaml
    │   ├── serial_lint_3_steward_guidance_exempt.yaml
    │   ├── serial_lint_4_resume_hint_consumed.yaml
    │   ├── serial_lint_5_fix_applied_dedup.yaml
    │   ├── serial_lint_6_handoff_auto_prepare.yaml
    │   ├── serial_lint_7_handoff_seeds_coverage.yaml
    │   ├── serial_lint_8_step_chain_replay.yaml
    │   └── serial_lint_9_timeout_fail_closed.yaml
    ├── step_handoff/
    │   ├── debug_exhausted_reaches_plan_gate.yml
    │   ├── fix_exhausted_reaches_plan_gate.yml
    │   ├── progress_task_mismatch.yml
    │   ├── state_projection_work_done_updates_progress.yml
    │   └── step_advance_u1_to_u2.yml
    ├── u6_coordinator_build_done_deny.yml
    └── verdict_gate_fail_keeps_loop_open.yml
```

### 5.2 三 preset BDD 路径对照

| Preset | YAML 路径 | 关键 scenario 描述 | 备注 |
|--------|-----------|-------------------|------|
| **ce-executor-serial** | `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml`<br>`crates/ralph-core/tests/scenarios/ce_executor_serial_review_silent_reviewer_recovers.yml`<br>`crates/ralph-core/tests/scenarios/ce_executor_serial_fix_applied_rereview.yml` | 4 维度串行 review chain(无 wave dispatcher) | 由 `scenarios.rs:1435, 1593, 1615` 三个测试函数驱动 |
| **ce-executor-isolated** | `crates/ralph-core/tests/scenarios/ce_executor_bootstrap_recovery.yml`<br>`crates/ralph-core/tests/scenarios/ce_executor_recovery.yml` | isolated 模式 handoff、bootstrap、recovery | 由 `scenarios.rs:1406, 1423` 驱动;`scenarios.rs:988-988` 描述 `ce-executor-isolated` handoff |
| **ce-executor-wave** | (无独立 YAML scenario) | 仅 fixture 覆盖:`tests/fixtures/wave-isolated-dimension-done/8-dimension-done.jsonl` + `topology.yml`;`tests/fixtures/flow_reliability/zippy-sparrow-4of11-stall.jsonl` 关联 flow_reliability scenarios | `presets/en/ce-executor-wave.yml` 存在,但 BDD 没有专门 scenario;`memory:ce-executor-isolated-wave-deprecation` 标注 wave preset 后续会被舍弃 |
| **ce-executor-serial (serial_lint 套件)** | `tests/scenarios/serial_lint/*.yaml`(11 个) | U2-U4b 引擎 gate 行为、断路器、rejection digest、steward exempt、resume hint、fix dedup、handoff auto-prepare、coverage、step chain replay、timeout fail-closed、isolated unaffected | 由 `scenarios.rs::test_*` 中关于 serial_lint 的 case 驱动;`assert_state_harness_smoke.yaml` 是 test harness 自身 |

> 表 5.1 关键观察:
> 1. `ce-executor-serial` BDD 覆盖最完整(3 个核心 + 11 个 serial_lint 子场景)。
> 2. `ce-executor-isolated` BDD 覆盖中等(2 个核心 recovery)。
> 3. `ce-executor-wave` BDD 覆盖最弱(0 个独立 YAML);用户 memory `ce-executor-isolated-wave-deprecation` 明确 wave preset 将被舍弃,U0 录制时可不为此 preset 单独建基线,但需保留现有 `wave-isolated-dimension-done` fixture 的回归覆盖。
> 4. U0 plan §"Approach" 要求"运行三个 preset 的 BDD/scenarios 捕获 `recovery.jsonl` / `tasks.jsonl` / `progress.md` / `events.jsonl` / prompt snapshot 作为 golden fixtures";`tests/fixtures/` 目前只有 `replay.jsonl`(noble-peacock) + `8-dimension-done.jsonl`(wave-isolated) + `ce-executor-rejected-event-recovery.jsonl` + `ce_executor_session_b_policy_violations.jsonl` 4 个基础 fixture,U1 需要为 `ce-executor-serial` + `ce-executor-isolated` 各补一套新的 `unified-state-baseline/` 目录(plan 217 行已声明新目录 `crates/ralph-core/tests/fixtures/unified-state-baseline/`)。

---

## 6. process_parse_result 阶段清单

> 范围:`crates/ralph-core/src/event_loop/mod.rs::process_parse_result`(`mod.rs:7193-9855`,约 2660 行)按调用顺序串接的每个 stage。
> 标注:
> - **类型**:gate(拒收/降级)/transform(改写事件)/passthrough(仅记录)/projection(派生 state)。
> - **是否 publish `task.resume`**:是 → 列出,否 → 标注"—"。
> - **U4 ledger 迁入**:该阶段输入是否要进入 ledger commit log。

| 序号 | 阶段名 | 类型 | file:line | 职责 | publish task.resume? | U4 迁入 |
|------|--------|------|-----------|------|----------------------|---------|
| 0 | 入口/调试日志 | passthrough | `mod.rs:7197-7215` | `tracing::debug!` 入口日志 + 前 5 个 event 详情 | — | — |
| 1 | payload_contract_violation 局部变量初始化 | passthrough | `mod.rs:7217-7222` | 局部 `Option<PayloadContractViolation>` | — | — |
| 2 | **引擎 required-field gate (2026-06-20-001 R15/KTD-10)** | gate + transform | `mod.rs:7267-7271` (`apply_engine_required_field_gate` 实现 `mod.rs:6012-...`) | engine 通过 `ProtocolView` 做 required-field fail-fast;**review P0 #1 后已前置到 malformed-loop 之前**;review P0 #4 同时 seed `state.pending_lint_resume` | ❌(只 seed lint hint) | 拒收事件以 `Rejection` 形式 commit |
| 3 | malformed 行 backpressure | gate | `mod.rs:7279-7297` | 遍历 `result.malformed`,publish `event.malformed`,累加 `consecutive_malformed_events` | ❌(publish `event.malformed`) | 同上 |
| 4 | 空 turn stall 探测 | gate | `mod.rs:7299-7321` | 早返回 + `run_stall_detector_on_state` | — | — |
| 5 | **scope enforcement (isolated mode) 准备** | passthrough | `mod.rs:7334-7336` | 拷贝 `current_isolated_hat` 与 `cancellation_promise` 避免借用冲突 | — | — |
| 6 | **isolated mode event loop** (8 子阶段) | gate | `mod.rs:7337-7830+`(约 500 行) | 见 §6.1 子表 | 是(子阶段 6.4, 6.7, 6.8) | 各 sub-rejection 各自 commit |
| 6.1 | orchestrator control/diagnostic topic 旁路 | transform | `mod.rs:7382-7393` | `is_orchestrator_internal` 接受,不入业务预算 | — | — |
| 6.2 | ralph pseudo-hat 业务 topic 拒收 | gate | `mod.rs:7399-7421` | P1-12 prefix match,publish `event.isolation.boundary_violation` | ❌ | R6 拒收入 ledger |
| 6.3 | isolated anonymous business topic 拒收(U5) | gate | `mod.rs:7428-7464` | 无 hat/source/triggered provenance 时 fail-closed,publish `event.isolation.boundary_violation` + **task.resume(target=ralph)** | ✅ | 拒收入 ledger |
| 6.4 | isolated scope 边界检查 + 拒收 | gate | `mod.rs:7482-7830` | `isolated_publish_allowed` 失败时发 `event.isolation.boundary_violation` + 写 recovery envelope + **build_task_resume_payload → task.resume**;`enforce_wave_isolated_scope` 协同 | ✅(via `publish_policy_rejection_resume`) | R6 拒收入 ledger |
| 7 | coordinator mode event loop(后续 §6.2) | gate | `mod.rs:7830+` 之后 | 协调模式主路径(§6.2 表) | — | — |
| 8 | **wave pre-partition** | passthrough | `mod.rs:8090-8190+` | `apply_event_policy_validation` 前置 wave 相关状态读取 + 启动 `state_projection` | — | — |
| 9 | **event policy validation (U1, U2, U3, U4, R5)** | gate | `mod.rs:8154` (`apply_event_policy_validation` 实现 `mod.rs:1169-...`) | 8 个 reject 分支,每个调用 `publish_policy_rejection_resume`(771/1296/1319/1405/1427/1477/1515/1617/1651) | ✅(每分支) | 拒收入 ledger |
| 10 | policy hold artifact 写入 | transform | `mod.rs:8350-8355` | `write_hold_artifact` 持久化 hold reason | — | — |
| 11 | payload_contract_violation 收集 | passthrough | `mod.rs:8356-8360` | 收第一条 violation 给 runner | — | — |
| 12 | **state machine validation** | gate | `mod.rs:8364-8423` | `sm_state.validate_event` 四态决策(Accept/Reject/Ignore/DiagnosticOnly) | ❌(publish `event.state_machine.{rejected,ignored,diagnostic}`) | SM decision commit |
| 13 | **hat-handoff gate (2026-06-18-002 U5, KTD-4)** | gate | `mod.rs:8426-8576` | isolated 模式下 macro-edge 验证;Reject 时 publish `event.step_handoff.gate_rejected` + **task.resume** | ✅(Reject 分支 `mod.rs:8562`) | R-KTD-4 拒收入 ledger |
| 14 | **state projection (U1 2026-06-17-003)** | projection | `mod.rs:8578-8664` | `StateProjector::apply`;reject 时 publish `event.state_projection.rejected` + 累加 consecutive 计数 | ❌(publish diag) | projection commit |
| 15 | **step handoff gate (U4 2026-06-17-002)** | gate | `mod.rs:8666-8695` | `apply_step_handoff_gate` 验 progress.md ↔ tasks.jsonl 一致性;reject 时 publish `event.step_handoff.gate_rejected` + 写 recovery envelope | ❌(envelope) | gate reject commit |
| 16 | **workflow guard validation** | gate | `mod.rs:8697-8731` | `apply_workflow_guard_validation` 检 linear guard chain;reject 时 `Self::log_workflow_guard_rejection` 写 envelope,**经 `publish_policy_rejection_resume` 发 task.resume** | ✅ | R-WG 拒收入 ledger |
| 17 | **policy runtime state 更新** | projection | `mod.rs:8733-8746` | `policy_state.terminal_observed = true` 当 `event.topic ∈ terminal_topics` | — | — |
| 18 | **execution contract validation (U5 2026-06-04)** | gate | `mod.rs:8748-...` | `validate_execution_contract` 检 work.done schema + git evidence;reject 时 publish diag + **targeted task.resume(source_hat)**(`mod.rs:8896`) | ✅ | contract_rejections commit |
| 19 | record_event 循环(accepted events) | transform | `mod.rs:9637-9734` | `state.record_event` + `record_verdict_if_match` + hat_lifecycle tracker + handoff_tracker.on_hat_activated + `clear_rejection_keys_for_hat` | — | ✅ 每个 accepted event 是 ledger commit |
| 20 | hat_lifecycle complete/observe_accepted_event | transform | `mod.rs:9655-9706` | 终端事件 `complete`,非终端 `observe_accepted_event`;ralph 跳过 handoff clear | — | — |
| 21 | **invariant assertion checks (U3)** | gate | `mod.rs:9780-9823` | INV-1: ralph 不可发 business topic;累加 `invariant_violation_count` 与 `last_invariant_violation` | — | INV violation commit |
| 22 | **stall detection (U5 收尾)** | gate | `mod.rs:9825-9836` | `run_stall_detector_on_state` 末次跑,post-validation stall 计数 | — | — |
| 23 | 返回 `ProcessedEvents` | passthrough | `mod.rs:9838-...` | 组装返回结构(含 `contract_rejections`、`payload_contract_violation`、`human_interact_context` 等) | — | — |

### 6.1 isolated mode 子阶段(序号 6 的展开)

> 范围:`mod.rs:7337-7830` 内 `if self.config.event_loop.execution_mode == HatExecutionMode::Isolated && let Some(ref isolated_hat) = isolated_hat_owned` 分支。

| 子序 | 子阶段名 | 类型 | file:line | 职责 | publish task.resume? |
|------|----------|------|-----------|------|----------------------|
| 6.1.1 | `non_wave_business_event_accepted` 与 `accepted_wave_id` 局部状态初始化 | passthrough | `mod.rs:7346-7379` | per-turn 业务事件预算 + wave group 接纳窗口 + `envelopes_written_this_turn` dedup | — |
| 6.1.2 | `is_orchestrator_control/diagnostic_topic` allowlist 旁路 | transform | `mod.rs:7382-7393` | 旁路,continue | — |
| 6.1.3 | ralph pseudo-hat 业务拒收(R6/U2) | gate | `mod.rs:7395-7421` | P1-12 prefix;publish `event.isolation.boundary_violation` | ❌ |
| 6.1.4 | isolated anonymous business topic 拒收(U5) | gate | `mod.rs:7423-7464` | publish `event.isolation.boundary_violation` + **task.resume(target=ralph, reason=isolated_anonymous_business_topic)** | ✅ |
| 6.1.5 | scope_hat 选择(P0-1) | transform | `mod.rs:7466-7481` | 优先 event.hat,fallback 到 isolated_hat | — |
| 6.1.6 | `isolated_publish_allowed` 边界检查 | gate | `mod.rs:7482-7820+` | 失败时 publish `event.isolation.boundary_violation` + 写 recovery envelope + **build_task_resume_payload → task.resume**(via `publish_policy_rejection_resume` 被 `enforce_wave_isolated_scope` 与 `publish_isolated_wave_violation` 间接调用) | ✅(via `publish_policy_rejection_resume`) |
| 6.1.7 | scope_violation 断路器累计 + trip | gate | `mod.rs:7700-7725` | `state.scope_violation_circuit_breaker_tripped = Some(reason)` when `rejection_key_is_exhausted` | — |
| 6.1.8 | `cancellation` 字符串 + wave-context-for-resume 准备 | passthrough | `mod.rs:7378-7380, 7808-7822` | 给 `build_task_resume_payload` 准备 `WaveContextForResume` | — |

### 6.2 coordinator mode 主路径关键调用

> 范围:`mod.rs:7830+` 之后,`if` 分支外的主路径(`is_orchestrator_control_topic` 旁路 + 各 gate + final publish)。

| 子序 | 子阶段名 | file:line | 备注 |
|------|----------|-----------|------|
| 6.2.1 | human.interact 同步轮询(robot service) | `mod.rs:9450-9619` | `RobotService::wait_for_response`;成功 → `human.response`,超时 → `human.timeout` |
| 6.2.2 | restart_requested 检查 | `mod.rs:9621-9627` | `mark_restart_requested` if `is_restart_request_event` |
| 6.2.3 | hat_activation_tracker 更新 + verdict_record + handoff_tracker.on_hat_activated + clear_rejection_keys | `mod.rs:9635-9713` | 见 6 #19-20 |
| 6.2.4 | event_projection 同步调用 | `mod.rs:9736-9747` | `crate::event_projection::apply_projection`(独立于 state_projection 字段的 projection 通道) |
| 6.2.5 | bus.publish 循环 | `mod.rs:9749-9755` | 全部 accepted events 上 bus |
| 6.2.6 | human.response 单独处理 | `mod.rs:9757-9778` | 若 robot 返回 response_event,单独 publish |
| 6.2.7 | invariant 收尾 | `mod.rs:9780-9823` | 同 §6 #21 |
| 6.2.8 | stall detector 收尾 | `mod.rs:9825-9836` | 同 §6 #22 |

### 6.3 关键表

- **进入 ledger 的数据**:已 accepted events(序号 19)、各 gate 的 reject decision(2, 6.1.3, 6.1.4, 6.1.6, 9, 12, 13, 14, 15, 16, 18, 21)。
- **publish task.resume 的阶段**:6.1.4(anonymous)、6.1.6(scope_violation)、9(event_policy, 8 分支)、13(hat_handoff reject)、16(workflow guard reject)、18(execution contract reject)。共 6 个主要 stage,覆盖 §3 表的 13 个生产调用点。
- **stage 上下游关系**:
  - engine gate(2)→ malformed(3)→ empty(4)→ isolated(6) or coordinator(7)→ wave pre-partition(8)→ event policy(9)→ state machine(12)→ hat-handoff gate(13)→ state projection(14)→ step handoff gate(15)→ workflow guard(16)→ execution contract(18)→ record+publish(19-20, 6.2.5)→ invariants(21)→ stall(22)。
  - 注意 stage 12-18 在 isolated 与 coordinator 两个分支中共享(在 `events` 变量上下文中运行)。

---

## 7. 意外发现与备注

1. **`tasks_cache` / `progress_cache` 不在 `LoopState`**:盘点时发现 `state_projection: Option<StateProjector>` 才是字段,但内部持有的 `ProjectionContext::tasks_cache` / `progress_cache`(`state_projector/mod.rs:111,114`)才是真正的 ledger 数据源。U1 ledger 的 `task` 与 `progress` 子结构应该直接由 `ProjectionContext` 提取,不再经 `StateProjector` 包装。
2. **`RecoveryResponder` / `DriftEngine` / `DiagnosticsCollector` 都不在 `LoopState`**:目前由 `EventLoop` 字段独立持有(`mod.rs:5188-5195, 3574-3582`)。U1 ledger 设计需要明确是否将其内部状态也迁入 ledger,否则 §3 #15 (drift engine publish task.resume) 仍会绕过 `StateLedger::commit`。
3. **`LintResumeHint` 的双写**:CLI `commands/emit.rs:288,301` 仍调用 `write_pending_lint_resume`(写 `.ralph/pending_lint_resume.json`),而 `state.pending_lint_resume` 才是 SSOT(2026-06-20-001 review P0-4 修复);plan R9 要求"lint failure 不进 recovery.jsonl",U1 ledger 设计需要保留这个区分。
4. **`Instant` 字段共 3 个**:`started_at`、`last_checkin_at`(已建议删)、`hat_activation_at`。U1 持久化方案需要统一转为 RFC3339 + 进程内重建为 `Instant`/`DateTime<Utc>`。
5. **`publish_policy_rejection_resume` 在 `apply_event_policy_validation` 中调用 8 次**,但函数定义在 `mod.rs:398`;U7a 建议统一迁到 `StateLedger::commit(Rejection)` 后由 responder 一次性发,8 个分支可全部折叠为 1 个。
6. **BDD scenario 对 `ce-executor-wave` 无独立 YAML**:`ce-executor-wave.yml` 存在但 `tests/scenarios/` 下没有任何 `ce_executor_wave*` 文件;既有 wave 路径仅由 `wave-isolated-dimension-done/8-dimension-done.jsonl` + `flow_reliability/zippy-sparrow-4of11-stall.jsonl` 两个 fixture 覆盖。Plan U0 不强制 wave preset 基线(配合 wave deprecation 决策)。
7. **`loop_runner/wave/dispatcher.rs::tests` 子模块**(`mod.rs:3015, 3020, 3022, 3046, 3067, 3166, 3169, 3174, 3178, 3214`)是 wave 模块的内嵌测试,不是顶层测试文件;U9 迁移测试时需要单独处理这些 inline 测试。
8. **`ralph-tui/src/state.rs:451` 出现 1 处 `task.resume` 字符串分支**:TUI 渲染消费侧,生产 publish 路径不涉及,迁移时只需保持事件 topic 字符串不变即可。
9. **`apply_engine_required_field_gate` 的 seed lint resume**(`mod.rs:6086`):这是 U2 在 `process_parse_result` 入口前的副作用,需要在 U1 的 `StateLedger` 路径中显式建模"非 event-derived 状态的写入"(目前 `pending_lint_resume` 是 `LoopState` 字段,理论上可以走 `ledger.commit(Rejection)` 衍生)。
10. **`event.state_projection.rejected` 与 `event.step_handoff.gate_rejected` 两条诊断 topic** 在 stage 13/14 中 publish,目前在 ledger 设计中未在 §1 字段表里出现 —— 它们的发布在 stage 内部,对应 ledger 的 `diagnostic` 事件类型(应由 `LedgerSnapshot` 的 commit type 显式枚举)。
11. **本盘点未运行任何 cargo 命令**:`cargo build`、`cargo test`、`cargo nextest` 均未执行,所有行号/字段计数均通过 `Read` + `rg` + `awk` 静态检索得出,与运行时无耦合。
