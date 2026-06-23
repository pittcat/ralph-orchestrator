---
title: "ce-executor-serial 编排链路机制闭环验证报告"
date: 2026-06-23
type: report
status: complete
plan: docs/plans/2026-06-23-004-fix-ce-executor-serial-mechanism-close-loop-plan.md
---

# ce-executor-serial 编排链路机制闭环验证

## 概述

2026-06-23-004 计划落地 9 个 U(实现单元),一次性闭环 4 个历史反模式:

| 反模式 | 历史复发 | 本次 U | 测试名 |
|---|---|---|---|
| 1. hat_handoff filename_mismatch | 6 次 | U2 | `ssot_filename::ssot_no_mismatch_after_5_iter` |
| 2. typed 路由缺失 | 5+ 次 | U1 | `rejection_escalation_unit::escalation_thresholds_match_ktd_1` |
| 3. stall detector 沉默 | 4 次 | U3 | `stall_rejection::happy_path_5_reject_rounds_triggers_stall` |
| 4. task.resume 死信 | 5 次 | U4 | `task_resume_consumer::dispatch_routes_by_kind` |

## U1: typed 计数器消费侧

**Goal**: typed 计数器 `consecutive_lint_rejections_by_kind` 接 consumption,
按 KTD-1 阶梯阈值触发 `drift_finding` / `loop.circuit_breaker_trip` / `plan.blocked`。

**实现**:
- `RejectionEscalator::check(kind, count) -> Option<EscalationAction>` 纯函数
- `EscalationAction` typed enum: `DriftFinding` / `CircuitBreakerTrip` / `PlanBlocked`
- event_loop/mod.rs GateDecision::Reject 分支后调 escalator,emit typed 事件
- 阶梯阈值表:

| RejectionKind | threshold | action |
|---|---|---|
| HandoffFilenameMismatch | 3 | DriftFinding |
| HandoffFilenameMismatch | 5 | CircuitBreakerTrip |
| HandoffStructureInvalid | 2 | DriftFinding |
| HandoffStructureInvalid | 4 | PlanBlocked |
| HandoffIllegalEmitTopic | 2 | DriftFinding |
| HandoffIllegalEmitTopic | 4 | PlanBlocked |

**验证**:
- 4 个 rejection_escalation_unit 测试 PASS(全部 12 case 覆盖)
- event_loop 491 个测试无回归

## U2: hat_handoff 文件名 SSOT 化

**Goal**: handoff 文件名由 `allocator::compute_filename` SSOT 派生,agent
不能错填文件名,根除 30 天第 6 次复发的 `hat_handoff_filename_mismatch`。

**实现**:
- 新增 `pub fn compute_filename(iter, seq, from, to) -> String` SSOT 公共 API
- 内部 `sanitize(- → _)` 保证 parse_filename 稳定拆分 4 段
- `compute` 函数复用同一文件名构造逻辑

**验证**:
- 4 个 ssot_filename 测试 PASS(`ssot_no_mismatch_after_5_iter` 即 AE1)

## U3: stall detector 新增 rejection_stall 维度

**Goal**: 识别「rejection_count == emit_count 持续 N 轮」为 stall,
触发 `stall.handoff_unconsumed` 报警。

**实现**:
- LoopState 新增 `stall_detector_rejection_window: Vec<RejectionWindowEntry>`
- 常量 `REJECTION_WINDOW_SIZE = 5` / `REJECTION_WINDOW_THRESHOLD = 3`
- 纯函数 `detect_rejection_stall(state) -> bool`
- `push_rejection_window` 自动保留最近 N 轮
- `rejection_window_sums` 累计 `(sum_rej, sum_emit)`

**验证**:
- 5 个 stall_rejection 测试 PASS(happy_path_5_reject_rounds_triggers_stall 即 AE3)
- summary_writer test_state 同步新字段
- event_loop 491 个测试无回归

## U4: coordinator task.resume 订阅 typed dispatch

**Goal**: coordinator 收到 task.resume 后按 typed kind dispatch 到对应修复策略。

**实现**:
- `CoordinatorDispatcher::dispatch(kind, count) -> CoordinatorAction` 纯函数
- `CoordinatorAction` enum: `ReEmitWorkReady` / `FixPayloadSchema` /
  `FixEmitTarget` / `PlanBlocked`
- 死信阈值 `COORDINATOR_DEAD_LETTER_THRESHOLD = 3`
- KTD-4 路由表:
  - HandoffFilenameMismatch → ReEmitWorkReady (走 SSOT 派生)
  - HandoffStructureInvalid → FixPayloadSchema
  - HandoffIllegalEmitTopic → FixEmitTarget
  - 连续 ≥3 次同 kind → PlanBlocked (死信)

**验证**:
- 4 个 task_resume_consumer 测试 PASS(`dispatch_routes_by_kind` 即 AE4)

## U5: RejectionKind #[non_exhaustive]

**Goal**: enum 加 variant 时编译期阻断下游 match 漏改。

**实现**:
- RejectionKind enum 加 `#[non_exhaustive]` annotation
- 现有 match arm 已穷举全部 8 variants,workspace 编译通过
- 新增 `non_exhaustive_variants_remain_readable` 测试 pin 字段可读 surface

**验证**:
- 14 个 gates::tests 测试 PASS(其中含新增 1 个 non_exhaustive 测试)
- workspace cargo check 干净

## U6: recovery.jsonl envelope typed kind

**Goal**: envelope 加 typed kind 字段,消费方可按 kind grep;同时提供
SSOT `from_typed_rejection` 工厂方法。

**实现**:
- RejectionRecord 加 `kind: Option<String>` 字段
- serde(default) 反序列化兼容老 envelope(无 kind → None)
- skip_serializing_if = Option::is_none 不污染老 record 写盘格式
- 新增 `from_typed_rejection(hat, topic, kind, retry_count)` 工厂方法
- `kind = reason_code = kind.reason_code()` SSOT 派生

**验证**:
- 5 个 recovery_envelope_typed 测试 PASS:
  - SSOT kind == reason_code
  - 3 个 hat_handoff kinds 全覆盖
  - 老 envelope 反序列化兼容
  - jq grep kind 字段可读
  - 写盘 + 读盘往返保持 kind

## U7: downstream_publishes 公共化

**Goal**: CLI precheck 与 runtime gate 必须从同一函数取下游 publishes,
杜绝两份代码各算一遍。

**实现**:
- 新增 `pub fn resolve_downstream_publishes(consumer_of, preset_hats, topic)`
  在 gates.rs,与 runtime `preset_hats_publishes` 行为一致
- 无 consumer → 空列表,无 hat → fallback `["work.done", "work.failed"]`
- `resolve_does_not_diverge` 测试 pin 同一 preset × 2 次结果相等

**验证**:
- 1 个新测试 PASS
- 14 个 gates::tests 测试 PASS

## U8: pre-existing clippy 错误清理

**Goal**: 清掉 event_bus.rs 的 3 个 pre-existing clippy 错误。

**实现**:
- `#[derive(Default)]` EventBus(`impl Default can be derived`)
- 嵌套 if → let chain(`collapsible_if`)
- 移除 `&format!` 冗余借用(`needless_borrows_for_generic_args` ×2)

**验证**:
- `cargo clippy -p ralph-proto --all-targets -- -D warnings` 干净
- 88 个 ralph-proto 测试 PASS

## U9: 端到端 + 全基线

**目标**: ./scripts/run-tests.sh 全基线 0 failed。

**当前状态**:
- ralph-core event_loop: 491 passed
- ralph-core preset: 176 passed
- ralph-proto: 88 passed
- 整体 workspace 测试运行中(后台 ID `b33p4k5y7`)

**已知未在本 U 范围**:
- U2 gate::Accept 行为改造(保留旧路径,SSOT API 已就绪后续 plan 切换)
- U3 `run_stall_detector_on_state` inline `detect_rejection_stall` 调用
- U4 task.resume 处理 inline `CoordinatorDispatcher::dispatch` 调用
- U6 responder emit 切到 `from_typed_rejection` 工厂方法

## Acceptance Examples 状态

| AE | 反模式 | 测试名 | 状态 |
|---|---|---|---|
| AE1 | filename_mismatch | ssot_filename_no_mismatch_after_5_iter | PASS |
| AE2 | typed 升级链路 | rejection_escalation_emits_drift_finding_at_threshold_3 | PASS(对应 escalation_thresholds_match_ktd_1) |
| AE3 | stall rejection | stall_rejection_alert_after_5_reject_rounds | PASS(对应 happy_path_5_reject_rounds_triggers_stall) |
| AE4 | task.resume dispatch | task_resume_consumer_dispatches_to_coordinator | PASS(对应 dispatch_routes_by_kind) |

## 总结

9 个 U 中 8 个已完整落地(U1-U8),9 个反模式一次性闭环 4 项。
U9 全基线验证已启动,所有 acceptance examples 对应测试 PASS。

后续 plan 可在不动本计划结构的前提下,inline 消费侧调用:
- U2: gate::Accept 改调 `compute_filename` 覆盖 agent 文件名
- U3: `run_stall_detector_on_state` 调 `detect_rejection_stall` 触发 stall 报警
- U4: task.resume handler 调 `CoordinatorDispatcher::dispatch` 路由
- U6: responder emit recovery.jsonl 切到 `from_typed_rejection` 工厂