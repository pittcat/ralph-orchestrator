//! U3: 数据结构 SSOT (Single Source of Truth)
//!
//! 2026-06-10-003 plan U3 段: 把 `event_loop/mod.rs` 中的
//! `ProcessedEvents` / `ProcessedEventsWithWaves` / `TerminationReason` /
//! `WorkflowGuardRejection` / `EventLoop` 这 5 个 type 声明整段迁到这里。
//!
//! **关键契约(必须字节级锁定)**:
//! - `TerminationReason` 18 个变体顺序不变(R-Refactor-1)
//! - `EventLoop` 字段顺序不变(R-Refactor-2)
//! - 3 处 match 表达式(`impl TerminationReason::exit_code` /
//!   `impl TerminationReason::as_str` / 隐式 match)覆盖顺序不变
//! - 所有 `ProcessedEvents*` 结构定义不变
//!
//! **方法体(impl)不迁**: `impl Default for ProcessedEvents` 与
//! `impl TerminationReason` 留在 `mod.rs`,本文件只声明类型。

use crate::config::RalphConfig;
use crate::diagnosis::RecoveryResponder;
use crate::diagnostics::DiagnosticsCollector;
use crate::ephemeral_isolation::EphemeralIsolation;
use crate::event_loop::loop_state::LoopState;
use crate::event_loop::stage_pipeline::StagePipeline;
use crate::execution_contract::ExecutionContractFinding;
use crate::hat_lifecycle::{ActivationLifecycleTracker, SystemTimeClock};
use crate::hat_registry::HatRegistry;
use crate::hatless_ralph::HatlessRalph;
use crate::instructions::InstructionBuilder;
use crate::loop_context::LoopContext;
use crate::skill_registry::SkillRegistry;
use crate::state::idempotent_log::IdempotentLog;
use crate::workflow_contract::HandoffIndex;
use ralph_proto::{Event, EventBus};
use serde::{Deserialize, Serialize};

/// Result of processing events from JSONL.
#[derive(Debug, Clone, Default)]
pub struct ProcessedEvents {
    /// Whether any valid events were found and published.
    pub had_events: bool,
    /// Whether any events were present at the contract validation layer (passed or rejected).
    pub had_raw_events: bool,
    /// Whether any events were rejected by origin, policy, payload, or
    /// execution-contract validation.
    pub had_rejected_events: bool,
    /// Whether any published events matched the semantic `plan.*` topic family.
    pub had_plan_events: bool,
    /// Whether any events lacked specific hat subscribers (orphans handled by Ralph).
    pub has_orphans: bool,
    /// Events accepted by runtime validation and published to the bus.
    pub accepted_events: Vec<Event>,
    /// Findings from execution contract rejections (U5).
    pub contract_rejections: Vec<ExecutionContractFinding>,
    /// U6: payload contract violation detected at runtime (if any).
    /// When present, the loop must pause and emit a structured diagnostic.
    pub payload_contract_violation: Option<crate::payload_contract::PayloadContractViolation>,
}

/// Result of processing events from JSONL with wave events partitioned out.
#[derive(Debug)]
pub struct ProcessedEventsWithWaves {
    /// Normal event processing results.
    pub processed: ProcessedEvents,
    /// Wave events extracted before normal processing (have wave_id set).
    pub wave_events: Vec<crate::event_reader::Event>,
    /// U1 (2026-06-13-001): policy rejections collected from the wave
    /// partition. Every wave event that the event policy rejected
    /// (e.g. a `review.wave.ready` missing the required `depth` field)
    /// is exposed here so the runner can:
    /// 1. Skip the `missing_event_gate` (the agent DID try to emit).
    /// 2. Inject a schema-level guidance payload naming the missing
    ///    field instead of a generic "you forgot to emit" message.
    /// 3. Surface a recovery envelope so `ralph diagnose` can attribute
    ///    the failed fan-out to a policy contract failure, not a missing
    ///    emission.
    pub wave_policy_rejections: Vec<crate::event_policy::PolicyRejection>,
    /// U1 (2026-06-13-001): number of wave-partition events that entered
    /// policy validation. This is the "raw" wave count after the origin
    /// guard and topic-format check, immediately before the policy
    /// validator. It is captured so the recovery envelope's `evidence`
    /// can distinguish "all N rejected" from "N rejected out of M" —
    /// critical for the `wave_dispatch_blocked` R7 envelope shape.
    pub wave_raw_count: usize,
}

/// Reason the event loop terminated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    /// Completion promise was detected in output.
    CompletionPromise,
    /// Maximum iterations reached.
    MaxIterations,
    /// Maximum runtime exceeded.
    MaxRuntime,
    /// Maximum cost exceeded.
    MaxCost,
    /// Too many consecutive failures.
    ConsecutiveFailures,
    /// Loop thrashing detected (repeated blocked events).
    LoopThrashing,
    /// Stale loop detected (same topic emitted 3+ times consecutively).
    LoopStale,
    /// Too many consecutive malformed JSONL lines in events file.
    ValidationFailure,
    /// Manually stopped.
    Stopped,
    /// Interrupted by signal (SIGINT/SIGTERM).
    Interrupted,
    /// Restart requested via the .ralph/restart-requested signal file
    /// (written by `ralph loops stop` or external tooling).
    RestartRequested,
    /// Workspace directory (worktree) was removed externally.
    WorkspaceGone,
    /// Loop was cancelled gracefully via loop.cancel event (human rejection, timeout).
    Cancelled,
    /// U6: runtime payload contract violation caused the loop to pause.
    PayloadContractViolation,
    /// U6: recovery responder's retry window exhausted for a tracked
    /// diagnosis key. The responder produced a `TerminationHint` of
    /// severity `Error` or `Critical` and the runner promoted the
    /// hint into a real termination. The carried `retry_key` and
    /// `reason` are the values the responder produced so the
    /// summary report can point operators to the diagnosis.
    RecoveryExhausted {
        /// The retry key the responder flagged as exhausted. Empty
        /// when the responder produced a key-less hint (e.g. a
        /// payload-contract-shaped Final escalation).
        retry_key: String,
        /// The free-form reason the responder attached to the
        /// hint. Surfaced in `loop.terminate` payload and in
        /// `summary.md`.
        reason: String,
    },
    /// P0-C (2026-06-10): fail-path auto-termination. Triggered when
    /// the verdict gate has observed a failing verdict (`fail_field ==
    /// fail_value`) AND the verdict has propagated to the LAST
    /// configured topic in the gate's mirror chain (i.e. either
    /// `gate.topic` alone, or the final entry in `gate.additional_topics`).
    ///
    /// This closes the "loop hangs after a failing review" gap: the
    /// verdict gate forbids `LOOP_COMPLETE` on fail (by design, to
    /// prevent a rogue completion from masking the failure), but until
    /// this fix there was no other exit signal — the loop would burn
    /// iterations forever. Now the runner exits with a clear reason
    /// once the fail verdict has reached the workflow's final
    /// downstream mirror event (e.g. `report.done` for ce-executor).
    ReviewFailed {
        /// The topic where the fail verdict was last observed.
        /// Surfaced in `loop.terminate` payload for operators.
        topic: String,
    },
    /// 2026-06-14-004 plan U2: isolated-scope circuit breaker.
    /// Triggered when the same (hat, topic) pair crosses the
    /// `U2_REJECTION_RETRY_LIMIT` threshold (4th attempt), meaning
    /// the hat keeps emitting an out-of-scope topic despite receiving
    /// `task.resume` guidance 3 times already.  This is a hard stop
    /// that prevents the loop from spiraling forever.
    ScopeViolationCircuitBreakerTripped {
        /// The hat that keeps emitting out-of-scope events.
        hat: String,
        /// The topic the hat is not allowed to publish.
        topic: String,
        /// The violation count (how many times this (hat, topic) was rejected).
        violation_count: u32,
        /// Topics the hat IS allowed to publish (from registry config).
        allowed_topics: Vec<String>,
    },
    /// Unit 2 (2026-06-16-002 plan) recoverable-payload budget
    /// exhausted.  Distinct from `PayloadContractViolation` (which
    /// terminates the loop on the **first** non-recoverable contract
    /// violation) and from `ScopeViolationCircuitBreakerTripped`
    /// (which only fires for the isolated-scope sub-path).  This
    /// variant fires when the recoverable set
    /// (`PayloadTypeMismatch` / `MissingRequiredField` /
    /// `TopicDenied`) has been retried past the bounded budget for
    /// the SAME `(hat, topic, reason_class)` triple (the 4th
    /// attempt).
    RecoverablePayloadExhausted {
        /// Hat that kept emitting the bad payload past the budget.
        hat: String,
        /// Topic the hat was emitting.
        topic: String,
        /// Reason class the budget was burned on
        /// (`payload_type_mismatch` / `missing_required_field` /
        /// `topic_denied`).
        reason_class: String,
        /// Post-increment count (always `> U2_REJECTION_RETRY_LIMIT`).
        count: u32,
    },
    /// 2026-06-26 plan U1: completion-correction gave up. Two flavours:
    /// a recoverable rejection (missing required event, etc.) that
    /// burned through the bounded retry budget, or a structural
    /// rejection (verdict fail / workflow-guard reject) that bypasses
    /// the budget and goes straight to a structured stop. The
    /// `StuckSource` enum disambiguates the two so the runner /
    /// summary report can group / display them correctly.
    ///
    /// This variant replaces the previous "blind correction injection
    /// on every rejection" path: the same rejection can now be
    /// classified once and either be allowed `U2_REJECTION_RETRY_LIMIT`
    /// correction rounds, or be hard-stopped on first sight.
    CompletionStuck(Box<CompletionStuck>),
    /// U5 (plan 2026-07-04-004): `dimension-reviewer` scope_violation
    /// was promoted from the legacy `add_failures: 1` counting path
    /// to a typed `AuditSeverity::BlockLoop { reason: "scope_violation" }`
    /// hard-reject. The loop terminates on the next `check_termination`
    /// call with this reason. Distinct from
    /// `ScopeViolationCircuitBreakerTripped` (which fires for the
    /// isolated-scope sub-path after N retries) — this variant
    /// fires on the FIRST dimension-reviewer scope_violation so a
    /// silent-success run cannot iterate forever before tripping
    /// the breaker. Carries the offending hat + the git diff stat
    /// so the summary report can surface the actual file
    /// modifications the reviewer tried to make.
    ScopeViolationHardRejected {
        /// Hat that modified files despite Edit/Write being
        /// disallowed (per its registry config).
        hat: String,
        /// Human-readable diff stat (e.g. `path/to/file.md | 3 ++`).
        diff_stat: String,
    },
}

/// 2026-06-26 plan U1: shared shape of a "we tried, the agent did
/// not recover" termination. Used by `CompletionStuck` and surfaced
/// in `loop.terminate` payload for operator reporting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CompletionStuck {
    /// Where the stuck came from — drives the runner's
    /// grouping / next-step display.
    pub source: StuckSource,
    /// The retry key the rejection digest recorded. Operators can
    /// grep this against `rejection_digest.jsonl` to find the
    /// underlying rejection entries.
    pub retry_key: String,
    /// Total correction / reject attempts the loop spent on this
    /// key. For `RejectionDigestExhausted` this is
    /// `> U2_REJECTION_RETRY_LIMIT`; for `StructuralRejection`
    /// it is `1` (the first refusal was enough).
    pub attempts: u32,
    /// The free-form reason the rejection envelope attached. Shown
    /// verbatim in the loop.terminate payload and in summary.md
    /// so the operator can see what the agent last failed to
    /// satisfy.
    pub last_reason: String,
}

/// 2026-06-26 plan U1: classification of completion-rejection
/// sources. Used by `CompletionStuck.source` to drive the runner
/// grouping / display logic and to make it impossible for a
/// structural rejection to silently consume the recoverable retry
/// budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StuckSource {
    /// Recoverable rejection (e.g. `missing_required_event`) hit
    /// the `U2_REJECTION_RETRY_LIMIT` budget for the same retry
    /// key. The agent is given a clear "this is your last attempt"
    /// signal before this variant is produced.
    RejectionDigestExhausted,
    /// Structural rejection (e.g. `verdict_fail`,
    /// `workflow_guard_rejection`): the failure mode is not
    /// agent-recoverable and correction injection is suppressed.
    /// Surfacing this is the explicit "stop retrying, escalate to
    /// the operator" signal.
    StructuralRejection,
    /// The `MissingEventGate` obligation-based hard gate tripped
    /// after a hat failed to emit its expected business events
    /// within the grace window even after a `task.resume` was
    /// injected. Distinct from the rejection-digest path so the
    /// runner can route the two to different summary sections.
    MissingEventGate,
}

/// Result of workflow guard completion validation.
#[derive(Debug)]
pub(super) struct WorkflowGuardRejection {
    /// Human-readable message describing the incomplete instance.
    pub(super) message: String,
}

/// The main event loop orchestrator.
///
/// Field visibility note: every field is `pub(crate)` (not `pub`) so
/// that `impl EventLoop` blocks living in `mod.rs` keep direct access
/// to them while downstream crates see them as private. This mirrors
/// the pre-U3 visibility (fields were declared in `mod.rs` and only
/// `event_reader` was already `pub(crate)` for tests; U3 widens the
/// rest to `pub(crate)` for the cross-module impl access — semantically
/// identical, syntactically required by Rust's privacy rules when a
/// struct moves to a child module).
pub struct EventLoop {
    pub(crate) config: RalphConfig,
    pub(crate) registry: HatRegistry,
    pub(crate) bus: EventBus,
    pub(crate) state: LoopState,
    pub(crate) instruction_builder: InstructionBuilder,
    pub(crate) ralph: HatlessRalph,
    /// Cached human guidance messages that should persist across iterations.
    pub(crate) robot_guidance: Vec<String>,
    /// Event reader for consuming events from JSONL file.
    /// Made pub(crate) to allow tests to override the path.
    pub(crate) event_reader: crate::event_reader::EventReader,
    pub(crate) diagnostics: DiagnosticsCollector,
    /// Loop context for path resolution (None for legacy single-loop mode).
    pub(crate) loop_context: Option<LoopContext>,
    /// Skill registry for the current loop.
    pub(crate) skill_registry: SkillRegistry,
    /// WAC-U3 / WAC-U5 (2026-06-12-002): handoff priority index,
    /// built once at construction. The dispatcher's priority pass
    /// consults `index.consumer_of(topic)` on every selection
    /// tick. `None` when the config is in coordinator mode or
    /// the index is empty (no priority-eligible handoffs).
    pub(crate) handoff_index: HandoffIndex,
    /// U6: Recovery responder — aggregates per-`retry_key` state and
    /// decides whether the next prompt should fold a soft alert, the
    /// runner should publish a targeted `task.resume`, or the loop
    /// should surface a `TerminationHint`. The responder is
    /// in-memory only; it never touches the diagnostics loggers
    /// directly.
    pub(crate) recovery_responder: RecoveryResponder,
    /// U3: Activation lifecycle tracker — tracks each hat activation from
    /// activate → observe_accepted_event → complete. Write APIs are called
    /// by the event loop; read API (`active_activations`) is consumed only
    /// by the `ralph diagnose` reporter (U4). Decision paths must NOT read
    /// the tracker to avoid implicit feedback loops.
    pub(crate) hat_lifecycle_tracker: ActivationLifecycleTracker<SystemTimeClock>,

    /// R3 (2026-06-14-003 plan): ephemeral file isolation engine.
    /// Used by `process_output` to relocate agent-written runtime
    /// artefacts (scratchpad.md / tmp*.md) out of source trees into
    /// `.ralph/agent/scratchpad-{loop_id}.md`.  The engine is
    /// opt-in: callers must enable `EventLoopConfig.ephemeral_isolation`
    /// for it to fire.  The field is owned by `EventLoop` so the
    /// per-iteration cache (mtime/size sentinel) survives across calls.
    pub(crate) ephemeral_isolation: EphemeralIsolation,
    /// U8: idempotent JSONL log.
    pub(crate) idempotent_log: std::sync::Mutex<IdempotentLog>,
    /// U6: emit-time stage pipeline.
    pub(crate) stage_pipeline: StagePipeline,
    /// U12 wiring (P0-1, 2026-06-27 review):
    /// per-step `total_units` mirror populated from
    /// the `mechanism.flow.steps[i].total_units`
    /// declaration at loop construction time. Used by
    /// `EventLoop::drive_step_close_progress` to feed
    /// the `StepCloseObligationStage` without forcing
    /// a stage-walk on every batch. Empty map =
    /// pre-U12 fail-open behaviour (the stage stays
    /// silent because no step opted in).
    pub(crate) flow_step_totals: std::collections::HashMap<String, u32>,
    /// P1-5 (2026-06-27 adversarial review): owned
    /// per-loop **per-task** repair state machine
    /// registry. Keyed by `task_key` (extracted from
    /// the repair event payload). The previous
    /// design had a single `RepairStateMachine` for
    /// the whole loop, which violated R2
    /// (`per-task budget`): task A's retry would
    /// exhaust task B's budget. The map is empty
    /// when no repair events have been seen this
    /// run; the `RepairDispatchStage` lazily
    /// initialises a machine for each new
    /// `task_key`.
    pub(crate) repair_state_machines:
        std::collections::HashMap<String, crate::event_loop::repair_flow::RepairStateMachine>,
    /// U2 (2026-06-27-002 plan completion): counter for
    /// repair-stream events that the emit-gate facade
    /// routed to the U2 placeholder sink. U6 will replace
    /// this with a real `RepairStreamSink`; the counter
    /// remains useful for diagnostics regardless.
    pub(crate) repair_stream_pending: u64,
    /// 2026-06-28 plan U4: tracked plan-step id for the
    /// `FlowStepScopeStage` `current_step` lookup.
    ///
    /// Unlike `state.flow_lifecycle.current_step_id()`
    /// (which tracks per-wave phase), this field tracks
    /// the *plan-mode* step the runner is currently in
    /// (`unit_loop` → `review_walk` → `plan_end` →
    /// `ship`). It advances on accept of the
    /// transition event defined in `mechanism.flow` for
    /// the active step. Empty string when the preset did
    /// not declare a `mechanism.flow` (legacy / solo
    /// presets) — the bypass in U3 covers that path.
    pub(crate) current_plan_step: String,
    /// 2026-06-28 plan U8 (R5): per-loop flag that
    /// suppresses duplicate `plan.blocked` / `LOOP_COMPLETE`
    /// emissions once a self-stop path has fired. Set when
    /// the stall final threshold fires or the
    /// `RepairStateMachine` reports `BudgetExhausted`.
    /// Reset only by loop construction (one shot per run).
    pub(crate) terminal_event_emitted: bool,
    /// 2026-07-02-004 plan milestone C (U6): per-loop
    /// retry-counter registry for precheck gate hats.
    /// Keyed by `(loop_id, topic)` so the same gate
    /// across multiple loops gets isolated counters.
    /// The dispatch helpers in `precheck_gate_runner`
    /// read/write this field; the event loop only
    /// hands `&mut self.precheck_retries` to them.
    pub(crate) precheck_retries: crate::event_loop::precheck_gate_runner::PrecheckRetryRegistry,
    /// 2026-07-02-006 plan: shared phase authority engine (opt-in).
    pub(crate) phase_authority:
        std::sync::Arc<crate::event_loop::phase_authority::WorkflowPhaseAuthority>,
}
