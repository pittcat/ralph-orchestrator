//! Loop state tracking for the event loop.
//!
//! This module contains the `LoopState` struct that tracks the current
//! state of the orchestration loop including iteration count, failures,
//! timing, and hat activation tracking.

use ralph_proto::{Event, HatId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, Instant};

use super::TerminationReason;
use super::termination::{TRIGGER_QUEUE_CAPACITY, TerminationTrigger};
use crate::flow_lifecycle::FlowLifecycleRegistry;

/// Maximum number of times the same rejection key may be retried
/// before the runner stops attempting targeted `task.resume` and
/// escalates to a fail-closed terminal reason.  Chosen to match
/// the historical `consecutive_hard_gates` ceiling so operators see
/// a single, consistent retry budget across failure modes.
pub const U2_REJECTION_RETRY_LIMIT: u32 = 3;

/// Plan 2026-06-20-001 KTD-7 / RISK-6: when the engine gate
/// rejects this many *consecutive* iterations, the linter
/// auto-disables itself for the rest of the run. d623c09's
/// runtime gates keep running, and the existing
/// `consecutive_malformed_events >= 3` termination check
/// remains as the final backstop. The breaker is *strictly
/// less than* the termination threshold (3) so it trips
/// *before* the loop dies, giving the runtime gates one
/// iteration to record the rejection before the existing
/// safety net fires.
///
/// We use a *separate* counter (`consecutive_engine_gate_rejections`)
/// rather than `consecutive_malformed_events` because the
/// Production default for the lint circuit breaker. The breaker
/// trips after this many consecutive engine-gate rejections and
/// disables the gate for the remainder of the run (the d623c09
/// runtime gates stay active).
///
/// Override per-process with the `RALPH_LINT_CIRCUIT_BREAKER_LIMIT`
/// env var. The override exists so the U9 3-stage R11 escalation
/// scenario (`correction_three_escalation_scenario`) can verify
/// the full retry→escalate flow without relaxing the production
/// default. RISK-6 (2026-06-20-001 KTD-7) keeps the production
/// limit at 2: trip on threshold 2 so the breaker fires *before*
/// the termination check at 3, giving the runtime gates one
/// iteration to record the rejection before the loop dies.
pub const LINT_CIRCUIT_BREAKER_LIMIT: u32 = 2;

/// Test-only override for the lint circuit breaker limit. Mirrors
/// `correction::set_correction_enabled_for_test`: a `OnceLock` so
/// tests can opt into a relaxed limit without calling
/// `std::env::set_var` (which is `unsafe` under Rust 1.81+ and
/// would conflict with the workspace's `forbid(unsafe_code)`).
/// Production code paths never call this — the override is read
/// at trip time in `apply_engine_required_field_gate`.
///
/// Returns `None` when the override was never set; the caller
/// falls back to [`LINT_CIRCUIT_BREAKER_LIMIT`] or the
/// `RALPH_LINT_CIRCUIT_BREAKER_LIMIT` env var.
pub fn lint_circuit_breaker_limit_for_test() -> Option<u32> {
    TEST_LINT_BREAKER_LIMIT
        .get()
        .map(|cell| cell.load(std::sync::atomic::Ordering::Relaxed))
}

/// Install a test override for the circuit breaker limit.
/// No-op when the override was already set; tests that need to
/// change the value across calls should pair this with
/// [`reset_lint_circuit_breaker_limit_for_test`].
pub fn set_lint_circuit_breaker_limit_for_test(limit: u32) {
    let cell = TEST_LINT_BREAKER_LIMIT
        .get_or_init(|| std::sync::atomic::AtomicU32::new(LINT_CIRCUIT_BREAKER_LIMIT));
    cell.store(limit, std::sync::atomic::Ordering::Relaxed);
}

/// Reset the test override. After this call the production
/// default + env-var path takes over again.
pub fn reset_lint_circuit_breaker_limit_for_test() {
    if let Some(cell) = TEST_LINT_BREAKER_LIMIT.get() {
        cell.store(
            LINT_CIRCUIT_BREAKER_LIMIT,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

static TEST_LINT_BREAKER_LIMIT: std::sync::OnceLock<std::sync::atomic::AtomicU32> =
    std::sync::OnceLock::new();

/// Fingerprint of the last emitted event for stale loop detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSignature {
    pub topic: String,
    pub source: Option<HatId>,
    pub payload_fingerprint: u64,
}

/// Composite progress marker for the stale-breaker mechanism.
///
/// Captures all forms of meaningful progress: accepted business events,
/// task state changes, workflow advancement, and state machine transitions.
/// Compared between consecutive completion rejections to determine whether
/// real work has occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressFingerprint {
    /// Count of distinct business topics accepted (excludes system/diagnostic topics).
    pub accepted_business_count: usize,
    /// Task store snapshot: (open_count, closed_count) at fingerprint time.
    pub task_snapshot: (usize, usize),
    /// Total workflow instances tracked across all chains.
    pub workflow_instances: usize,
    /// Sum of highest phases across all workflow instances.
    pub workflow_phase_sum: usize,
    /// State machine accepted transition count (0 when SM disabled).
    pub sm_transition_count: u32,
}

impl ProgressFingerprint {
    /// Computes a stable u64 hash from this fingerprint for quick comparison.
    pub fn hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.accepted_business_count.hash(&mut hasher);
        self.task_snapshot.hash(&mut hasher);
        self.workflow_instances.hash(&mut hasher);
        self.workflow_phase_sum.hash(&mut hasher);
        self.sm_transition_count.hash(&mut hasher);
        hasher.finish()
    }
}

/// 2026-06-18-001 plan U6: 单条 runtime 拒收摘要,记一次拒收事件。
///
/// - `count`: 同一 reason_code 的累计拒收次数
/// - `last_message`: 最近一次拒收 message(给 agent 看的)
/// - `last_ts`: 最近一次拒收的事件 ts
/// - `last_topic`: 拒收时事件 topic(供 agent 定位 payload 字段)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectionDigestEntry {
    pub count: u32,
    pub last_message: String,
    pub last_ts: String,
    pub last_topic: String,
}

/// Current state of the event loop.
#[derive(Debug)]
pub struct LoopState {
    /// Current iteration counter (0-indexed, starting at 0; first
    /// `complete_iteration` call advances to 1). Reflected verbatim
    /// to the `RALPH_LOOP_ITERATION` env var injected into the
    /// backend subprocess by `loop_runner::runner`. Kept 0-indexed
    /// to match the runtime gate's `expects iter=…` error message
    /// and the `compute` allocator's filename derivation.
    pub iteration: u32,
    /// Number of consecutive failures.
    pub consecutive_failures: u32,
    /// Cumulative cost in USD (if tracked).
    pub cumulative_cost: f64,
    /// When the loop started.
    pub started_at: Instant,
    /// The last hat that executed.
    pub last_hat: Option<HatId>,
    /// Consecutive blocked events from the same hat.
    pub consecutive_blocked: u32,
    /// Hat that emitted the last blocked event.
    pub last_blocked_hat: Option<HatId>,
    /// Per-task block counts for task-level thrashing detection.
    pub task_block_counts: HashMap<String, u32>,
    /// Tasks that have been abandoned after 3+ blocks.
    pub abandoned_tasks: Vec<String>,
    /// Count of times planner dispatched an already-abandoned task.
    pub abandoned_task_redispatches: u32,
    /// Consecutive malformed JSONL lines encountered (for validation backpressure).
    pub consecutive_malformed_events: u32,
    /// Consecutive hard-gate triggers when agent claims emit but writes no event.
    pub consecutive_hard_gates: u32,
    /// Whether a completion event has been observed in JSONL.
    pub completion_requested: bool,
    /// Whether the completion event has already been honored (prevents duplicate side effects).
    pub completion_honored: bool,

    /// U3 P0 fix (post-review): sticky flag tracking whether a business
    /// event has been accepted in the current isolated-mode turn.
    /// Reset at the start of each new turn (in `process_output`).
    /// Read by `check_default_publishes` so a `default_publishes` injection
    /// cannot co-exist with a JSONL business event in the same turn, and
    /// written by `check_default_publishes` so a later JSONL business event
    /// in the same turn still hits the boundary_violation gate.
    pub isolated_turn_business_event_accepted: bool,

    /// Per-hat activation counts (used for max_activations).
    pub hat_activation_counts: HashMap<HatId, u32>,

    /// Hats for which `<hat_id>.exhausted` has been emitted.
    pub exhausted_hats: HashSet<HatId>,

    /// When the last Telegram check-in message was sent.
    /// `None` means no check-in has been sent yet.
    pub last_checkin_at: Option<Instant>,

    /// Hat IDs that were active in the last iteration.
    /// Used to inject `default_publishes` when agent writes no events.
    pub last_active_hat_ids: Vec<HatId>,

    /// Events that activated `last_active_hat_ids` for the current execution.
    pub last_activation_events: Vec<Event>,

    /// Topics seen during the loop's lifetime (for event chain validation).
    pub seen_topics: HashSet<String>,

    /// The last event signature emitted (for stale loop detection).
    pub last_emitted_signature: Option<EventSignature>,

    /// Per-rejection-key retry counts (2026-06-07 plan U2).  The key is
    /// the stable string returned by `Rejection::compute_retry_key`
    /// (`stage:source_hat:topic:violation_class`).  Increments each
    /// time the runner sees the same key in a fresh batch.  When a
    /// key crosses `U2_REJECTION_RETRY_LIMIT` the next attempt
    /// emits a fail-closed `NonRetryableReason::RetryBudgetExhausted`
    /// so the runner can terminate or escalate.
    pub rejection_retry_counts: HashMap<String, u32>,

    /// 2026-06-14-004 U2: when the isolated-scope circuit breaker trips,
    /// the original (non-normalized) termination reason is stored here so
    /// `check_termination()` can return it with clear diagnostics.
    /// Set in the isolated-scope rejection branch when
    /// `rejection_key_is_exhausted` becomes true; consumed by the runner.
    pub scope_violation_circuit_breaker_tripped: Option<TerminationReason>,

    /// Iteration at which each rejection key was last seen, used by
    /// the recovery responder (U6) to de-duplicate recovery envelopes
    /// written for the same key across adjacent iterations.
    pub rejection_last_iteration: HashMap<String, u32>,

    /// 2026-06-18-001 plan U6: runtime 拒收摘要,按 reason_code 聚合,
    /// 注入到当前 hat prompt 的 `## RECENT REJECTIONS` 块。
    ///
    /// 每次 origin guard / policy check 拒收一个
    /// business 事件时,**仅** 累加 (count, last_ts, last_message);
    /// 保留最近 5 个不同 reason_code。recovery topic 自身
    /// (`task.resume` / `human.guidance`) 不生成摘要,避免循环。
    pub recent_rejection_digest: BTreeMap<String, RejectionDigestEntry>,

    /// Consecutive times the same event signature was emitted (for stale loop detection).
    pub consecutive_same_signature: u32,

    /// Set to true when a loop.cancel event is detected.
    pub cancellation_requested: bool,

    /// The hat currently selected for isolated execution.
    /// Set in isolated mode so `process_parse_result` knows which hat's scope to enforce.
    pub current_isolated_hat: Option<HatId>,

    /// Workflow progress tracking for guarded chains (chain name -> instance key -> phase).
    pub workflow_progress: WorkflowProgress,

    /// Event policy runtime state (opt-in, None when policy is disabled).
    pub policy_runtime_state: Option<crate::event_policy::PolicyRuntimeState>,

    /// State machine runtime state (opt-in, None when state machine is disabled).
    pub state_machine_runtime_state: Option<crate::state_machine::StateMachineRuntimeState>,

    /// 2026-06-17-003 U1: state-projection context. Held on the
    /// loop state so the projector survives across iterations and
    /// `bootstrap_from_disk` can re-populate the in-memory cache
    /// once on loop resume (Unit 6). `None` until the first
    /// projector-enabled iteration runs.
    pub state_projection: Option<crate::state_projector::StateProjector>,

    /// Payload of the most recent event whose topic matches the configured
    /// verdict gate topic. Used to enforce that the latest review verdict was
    /// a pass before the loop can terminate. `None` when no such event has
    /// been observed (or no verdict gate is configured).
    pub last_verdict_payload: Option<String>,

    /// Topic of the most recent event whose topic matched the configured
    /// verdict gate. Tracked alongside `last_verdict_payload` so the
    /// fail-path auto-termination check (P0-C, 2026-06-10) can detect
    /// when the verdict has fully propagated to the gate's last
    /// downstream mirror topic (e.g. `report.done` for ce-executor).
    /// `None` when no verdict has been observed.
    pub last_verdict_topic: Option<String>,

    /// Payload of the most recent **upstream** verdict event (the
    /// `gate.topic` itself, e.g. `REVIEW_COMPLETE`). This is kept
    /// separate from `last_verdict_payload` so that downstream mirror
    /// events (`report.done`) cannot overwrite a failing upstream
    /// verdict with a fake pass. `None` until the upstream topic is
    /// seen.
    pub last_upstream_verdict_payload: Option<String>,

    /// Signature of the most recent completion rejection (for stale-breaker).
    pub completion_rejection_signature: Option<String>,

    /// Count of consecutive completion rejections with the same signature.
    pub consecutive_completion_rejections: u32,

    /// 2026-06-16-001 U5: number of consecutive turns with no
    /// accepted business event. Reset to 0 the first time a
    /// business event is admitted; incremented on each
    /// no-progress turn. When the counter reaches
    /// `progress_steward.max_steward_iterations`, the loop emits
    /// `loop.stalled` and wakes the steward hat.
    pub consecutive_no_progress_turns: u32,

    /// 2026-06-16-001 U5: number of consecutive times the
    /// `progress-steward` hat has been auto-woken without
    /// producing a forwarded business event. When this counter
    /// reaches `progress_steward.max_steward_iterations`, the
    /// loop emits
    /// `plan.blocked(reason=loop_stalled_max_iterations)` and
    /// terminates cleanly.
    pub consecutive_steward_activations: u32,

    /// 2026-06-16-001 U5: when the loop auto-woke the steward in
    /// the current turn. Used to suppress recursive
    /// steward-wakes when the steward's own emit is rejected by
    /// the origin guard. The flag is set in
    /// `process_parse_result` and cleared at the start of the
    /// next turn.
    pub steward_woken_this_turn: bool,

    /// 2026-06-16-001 U5: per-turn flag passed to the
    /// stall-detector helper so it knows whether to reset the
    /// counters. Set at the start of each
    /// `process_events_from_jsonl` call to false; flipped to
    /// true if any business event is admitted; consulted by
    /// `run_stall_detector_on_state` after the post-validation
    /// tail.
    pub stall_detector_had_events: bool,

    /// Progress fingerprint hash at the time of the last completion rejection.
    /// Used to detect whether real progress has occurred between rejections.
    pub last_rejection_fingerprint: u64,

    /// Count of invariant assertion violations detected (U3).
    /// Incremented each time an invariant check fails.
    pub invariant_violation_count: u32,

    /// The most recent violation rule ID (e.g. "INV-1") for diagnostic display.
    pub last_invariant_violation: Option<String>,

    /// The git HEAD SHA at the moment the loop was started.
    ///
    /// Used by execution contract git-evidence validation to distinguish
    /// "this loop iteration produced new commits" from "the repository
    /// merely has commits from prior history". `None` when the SHA was
    /// not recorded (e.g., the workspace is not a git repository, or the
    /// loop runner could not resolve HEAD at startup).
    pub loop_start_sha: Option<String>,

    /// Per-step review terminal tracker for plan-gate hard enforcement (U1).
    pub review_step_tracker: super::review_step_state::ReviewStepTracker,

    /// WRC-U4 (2026-06-12-003): per-loop handoff deadline tracker.
    /// Records accepted handoff events for unique-consumer topics
    /// and surfaces escalations when a consumer hat fails to
    /// activate within `handoff_dispatch_timeout_seconds`. The
    /// tracker is wired into the main loop's three hook points
    /// (policy-accept, hat-activation, iteration tick) and is
    /// no-op for coordinator mode (the consumer-of-`*-only` path
    /// never runs in coordinator mode because the priority pass
    /// is disabled there). `Default::default()` builds a tracker
    /// with the documented 30s timeout (replaced from
    /// `WorkflowContractConfig` at construction time in both
    /// `EventLoop::with_context` and `EventLoop::with_diagnostics`).
    pub handoff_tracker: crate::workflow_contract::HandoffTracker,

    /// Unit 6 (2026-06-17-001 plan): FlowLifecycleRegistry owned
    /// by the loop. The wave dispatcher registers each new wave
    /// here on `Detected` and drives it through
    /// `Spawning -> WorkersActive -> Closed` / `PartialClosed ->
    /// Degraded` / `Failed -> Degraded`. Read by
    /// `hard_gate::should_gate_missing_events` (Unit 6,
    /// GateWaveMutex) to suppress the gate while a hat is
    /// legitimately waiting on wave workers.
    pub flow_lifecycle: FlowLifecycleRegistry,

    /// Count of consecutive stall_no_events recoveries (U5).
    pub stall_recovery_counts: HashMap<String, u32>,

    /// U3 (2026-06-13-001 plan): hat pinning target for the next
    /// iteration after a hard gate or schema-level wave recovery
    /// fires.  When set, [`EventLoop::next_hat`] overrides the
    /// round-robin / coordinator selection and activates this hat
    /// instead, then clears the value via `take()` (consume-on-use).
    /// The hat stays pinned for exactly **one** activation so the
    /// loop cannot get stuck on a single hat when the obligation
    /// is actually satisfied. The field is consumed by `next_hat`
    /// and is NOT cleared at any other turn boundary — if the loop
    /// exits before `next_hat` runs (deadline, panic, MaxIterations),
    /// the pin persists in-memory for the lifetime of the session.
    ///
    /// Populated by:
    ///   - `inject_missing_event_hard_gate_guidance` (hard gate)
    ///   - `inject_wave_policy_rejection_guidance` (schema-level
    ///     wave recovery)
    pub pending_recovery_hat: Option<HatId>,

    /// R1 (2026-06-14-003 plan): when `review-synthesizer` is woken up
    /// by an aggregate timeout (`inject_review_aggregate_timeouts`),
    /// the loop pins the wave_id here so the next `build_prompt` can
    /// mark the injected `WaveContext` with `AGGREGATE_TIMEOUT: true`.
    ///
    /// The pin is read with `.take()` on first read so the
    /// `AGGREGATE_TIMEOUT` signal does not leak across waves: a
    /// wave-1 timeout must not mark wave-2's synthesizer activation
    /// as timed-out.  After the first read, the pin is `None` until
    /// a new aggregate timeout sets it.  This matches the prior
    /// round's fix for the adversarial S9 scenario (stale wave
    /// context bleeding across waves).
    pub pending_synthesizer_timeout: Option<String>,

    /// R3 (2026-06-14-003 plan): ephemeral files relocated by
    /// `EphemeralIsolation::scan_and_relocate` during the most recent
    /// `process_output` iteration.  Surfaced as a `## EPHEMERAL RELOCATED`
    /// block in the next `build_prompt` so the agent learns the file
    /// has been moved and stops recreating it.  Cleared on `build_prompt`
    /// (consume-on-read).
    pub last_ephemeral_relocations: Vec<crate::ephemeral_isolation::RelocationRecord>,

    /// Unit 3 (2026-06-16-002 plan): `true` once the coordinator has
    /// emitted its first legal bootstrap `work.ready` event
    /// (no `reviewed_task_id` correlation).  While `false`, the
    /// `build_prompt` paths skip injecting `human.guidance` into the
    /// coordinator's prompt and `prepend_scratchpad` strips any
    /// `### HUMAN GUIDANCE` blocks from the scratchpad snapshot.
    /// Reset to `false` whenever `work.start` / `task.start` is
    /// published, mirroring the RObot guidance lifecycle.
    pub bootstrap_complete: bool,

    /// Unit 3 (2026-06-16-002 plan): `true` once the coordinator has
    /// emitted a terminal `work.failed` event after bootstrap.  This
    /// is a *distinct* signal from `bootstrap_complete`: the loop
    /// is no longer waiting on a coordinator handoff, but the
    /// bootstrap explicitly failed rather than succeeding.  The
    /// runner should use this flag to surface an explicit failure
    /// reason instead of letting the loop hang on a missing
    /// `work.ready`.  Reset to `false` together with
    /// `bootstrap_complete` on `work.start` / `task.start`.
    pub bootstrap_failed: bool,

    /// Unit 2 (2026-06-16-002 plan) recoverable-bucket budget
    /// exhaustions produced by the most recent policy validation
    /// pass.  The runner drains the buffer at the U6 guard and
    /// promotes the first entry into a
    /// `TerminationReason::RecoverablePayloadExhausted`.
    pub recoverable_exhaustion_buffer: Vec<crate::event_loop::RecoverableExhaustion>,

    /// U4 (2026-06-17-003 plan): dedup set for `work.done` events.
    /// Key format: `{plan_name}::{step}::{task_id}`. The set is
    /// populated when a `work.done` event is accepted by policy
    /// validation, and pruned on `queue.advance` / `review.failed`
    /// / `fix.applied` / step-close events. Fix-rounds within the
    /// same step are allowed to re-send `work.done` legitimately
    /// (the pruning fires on step boundaries, not on every
    /// `review.failed`).
    pub work_done_seen_tasks: HashSet<String>,

    /// 2026-06-24 P1-2: per-(plan, step, task) fix-round counter.
    /// Hard cap is `FIX_ROUND_HARD_CAP` (10). When the counter
    /// reaches the cap, subsequent `fix.applied` events for the
    /// same key are rejected and `fix.exhausted` is emitted to the
    /// bus. This is the Rust-side enforcement of the fixer
    /// instructions' "max 10 fix rounds" contract — the agent-side
    /// limit is advisory; this is the hard gate.
    pub fix_round_counts: HashMap<String, u32>,

    /// 2026-06-17-004 U2 (R3): per-hat first-activation timestamp
    /// for the current loop lifetime.  The `HatActivationClock` is
    /// written/refreshed when a hat is selected to execute an agent
    /// (see `event_loop/mod.rs::process_output` L4228 area).  The
    /// missing-event hard gate consults the clock to defer itself
    /// for a per-hat `missing_event_grace_secs` window — long-running
    /// hats like `dimension-reviewer` (per-worker timeout 1800s) must
    /// not be mis-fired during the first ~30-60s of model warm-up
    /// just because no event has appeared on the bus yet.
    ///
    /// `Instant::now()` is recorded the first time a hat activates;
    /// subsequent activations REFRESH the timestamp so repeated
    /// short activations do not accumulate a stale clock.  The
    /// missing-event gate fires only when `now - activation_at >=
    /// grace_secs`; within the grace window the gate is suppressed
    /// (returns `false`) regardless of the obligation/legacy path.
    pub hat_activation_at: HashMap<HatId, Instant>,

    /// 2026-06-17-004 U3 (R4+R5): snapshot of the trigger events
    /// that activated the most recent hat.  Populated when the
    /// gate is about to inject a `task.resume` for a hat that
    /// forgot to emit — the gate's `inject_missing_event_hard_gate_guidance`
    /// helper reads this snapshot to embed the original trigger
    /// topic + payload into the resume JSON (via
    /// `original_trigger_topic` / `original_trigger_payload`).
    /// The snapshot is then drained by the runner's
    /// `replay_obligation_triggers_to_activation_state` helper so
    /// the next `last_activation_events` snapshot includes the
    /// triggers (the resume's first activation sees its own
    /// `task.resume` event, not the original `review.dimension.ready`
    /// that woke the original hat).
    pub pending_obligation_triggers: Vec<Event>,

    /// U4b (plan 2026-06-20-001, R13 / KTD-8): in-memory lint
    /// failure hint consumed by the next `build_prompt`. Populated
    /// by `EventLoop::record_lint_failure` (which the CLI emit
    /// path also seeds via `.ralph/pending_lint_resume.json`),
    /// drained by `build_prompt` so the hint is delivered exactly
    /// once. The hint carries the failing topic, the
    /// `LintFailureClass`, the target hat, and the reason — enough
    /// for the prompt builder to inject `## LINT MIRROR` +
    /// `## LINT RESUME REQUIRED`. Stays in-memory only; never
    /// written to recovery.jsonl (R9).
    pub pending_lint_resume: Option<crate::preset::engine::LintResumeHint>,

    /// Plan 2026-06-20-001 KTD-7 / RISK-6: count of *consecutive*
    /// iterations in which the engine gate rejected every event.
    /// Reset to 0 on any iteration where the gate accepted at
    /// least one event. When this reaches
    /// [`LINT_CIRCUIT_BREAKER_LIMIT`] the engine gate
    /// auto-disables for the remainder of the run (set
    /// [`Self::lint_circuit_breaker_tripped`]).
    ///
    /// Distinct from `consecutive_malformed_events` (which
    /// tracks JSONL parse failures and feeds the
    /// `ValidationFailure` termination check); conflating the
    /// two would couple lint backoff to loop termination.
    pub consecutive_engine_gate_rejections: u32,

    /// Plan 2026-06-20-001 KTD-7 / RISK-6: latch set when
    /// [`Self::consecutive_engine_gate_rejections`] reaches
    /// [`LINT_CIRCUIT_BREAKER_LIMIT`]. While `true`, the
    /// engine gate short-circuits in `should_run_engine_gate`
    /// and d623c09's runtime gates keep running. Operators
    /// can override with `RALPH_SERIAL_LINT_MODE=off` (also
    /// disables the gate via a different code path) or by
    /// restarting the loop.
    pub lint_circuit_breaker_tripped: bool,

    /// 2026-06-23 fix plan (mechanism review layer 2,
    /// P0-B): typed per-kind consecutive lint-rejection
    /// counters. The key is `RejectionKind::reason_code()`
    /// (stable string SSOT for `recovery.jsonl` greps), the
    /// value is the consecutive count for that exact kind.
    /// Distinct from
    /// [`Self::consecutive_engine_gate_rejections`] (which
    /// counts *iterations* with all-reject batches) and from
    /// [`Self::rejection_retry_counts`] (which is keyed by
    /// arbitrary caller strings). The typed map enables:
    ///   1. circuit-breaker logic that picks a different
    ///      escalation per kind (e.g. filename mismatch →
    ///      drift_finding; structure invalid → plan.blocked).
    ///   2. follow-up plans (2026-06-21-001 U4) to add
    ///      `consecutive_lint_rejections:{kind}` counters
    ///      without re-instrumenting the gate.
    /// Seed: empty; populated by
    /// [`Self::record_typed_lint_rejection`].
    pub consecutive_lint_rejections_by_kind: HashMap<String, u32>,

    /// U3 (plan 2026-06-23-004, anti-pattern 3): rejection stall 检测窗口。
    ///
    /// 最近 N 轮(`REJECTION_WINDOW_SIZE`)的 `(rejection_count, emit_count)` 计数。
    /// 当 sum(rejection_count) ≥ 3 && sum(emit_count) == 0 → stall,
    /// emit `stall.handoff_unconsumed` typed 事件。
    ///
    /// 复用现有 progressive_failures 窗口,避免新增独立 timer。
    pub stall_detector_rejection_window: Vec<RejectionWindowEntry>,

    /// U1 (plan 2026-06-21-002): unified state ledger.
    /// `None` until the loop constructor wires it in.
    pub state_ledger: Option<crate::state::StateLedger>,

    /// Deterministic correction queue.  The loop runner writes a
    /// [`CorrectionContext`] here whenever a recoverable
    /// rejection fires; the next `build_prompt` reads the
    /// queue and prepends the `## ORCHESTRATOR CORRECTION`
    /// block to the prompt.
    pub prompt_context: crate::correction::PromptContext,

    /// 2026-06-23-005 F4: typed `TerminationTrigger` queue, the
    /// SSOT shape for future termination conditions
    /// (`plan_complete` / `dead_letter` / typed `block_loop`).
    ///
    /// **Status (F4)**: infrastructure-only. The queue is
    /// exposed via `push_termination_trigger` /
    /// `pop_termination_trigger` methods on `LoopState` so call
    /// sites that want to enqueue typed triggers have a typed
    /// API. `process_output` still consumes the legacy
    /// `consecutive_failures >= 5` termination path; the full
    /// migration to single-match `TerminationTrigger` dispatch
    /// is deferred until `LoopState` gains a persistence path
    /// (plan R15 — the `pending_dead_letter` field the original
    /// 005 plan assumed exists does NOT exist in the current
    /// codebase; wiring `process_output` to consume this queue
    /// without the persistence story would silently drop
    /// triggers across process restarts).
    ///
    /// Capacity: `TRIGGER_QUEUE_CAPACITY` (16). `push` returns
    /// `Err` on overflow and the caller can decide whether to
    /// force-terminate or drop the trigger.
    pub termination_triggers: VecDeque<TerminationTrigger>,
}

/// U3 (plan 2026-06-23-004): 单轮 rejection stall 检测窗口的条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RejectionWindowEntry {
    /// 本轮 typed rejection 累计计数(来自 typed_lint_rejection_count)。
    pub rejection_count: u32,
    /// 本轮 emit 的合法 business event 数(work.done / report.done 等)。
    pub emit_count: u32,
}

/// U3 (plan 2026-06-23-004): rejection stall 检测窗口大小。N 轮全 reject 触发 stall。
pub const REJECTION_WINDOW_SIZE: usize = 5;

/// U3 (plan 2026-06-23-004): rejection stall 阈值——窗口内累计拒绝次数 ≥ 此值触发 stall。
pub const REJECTION_WINDOW_THRESHOLD: u32 = 3;
impl Default for LoopState {
    fn default() -> Self {
        Self {
            iteration: 0,
            consecutive_failures: 0,
            cumulative_cost: 0.0,
            started_at: Instant::now(),
            last_hat: None,
            consecutive_blocked: 0,
            last_blocked_hat: None,
            task_block_counts: HashMap::new(),
            abandoned_tasks: Vec::new(),
            abandoned_task_redispatches: 0,
            consecutive_malformed_events: 0,
            consecutive_hard_gates: 0,
            completion_requested: false,
            completion_honored: false,
            isolated_turn_business_event_accepted: false,
            hat_activation_counts: HashMap::new(),
            exhausted_hats: HashSet::new(),
            last_checkin_at: None,
            last_active_hat_ids: Vec::new(),
            last_activation_events: Vec::new(),
            seen_topics: HashSet::new(),
            last_emitted_signature: None,
            consecutive_same_signature: 0,
            cancellation_requested: false,
            current_isolated_hat: None,
            workflow_progress: WorkflowProgress::new(),
            policy_runtime_state: None,
            state_machine_runtime_state: None,
            last_verdict_payload: None,
            last_verdict_topic: None,
            last_upstream_verdict_payload: None,
            completion_rejection_signature: None,
            consecutive_completion_rejections: 0,
            // 2026-06-16-001 U5: stall counter starts at 0. The
            // first turn that admits a business event resets it
            // (see process_parse_result).
            consecutive_no_progress_turns: 0,
            consecutive_steward_activations: 0,
            steward_woken_this_turn: false,
            // 2026-06-16-001 U5: per-turn stall-detector flag.
            // Set to false at the start of each
            // `process_events_from_jsonl` call; the helper
            // consults it after the post-validation tail.
            stall_detector_had_events: false,
            last_rejection_fingerprint: 0,
            loop_start_sha: None,
            rejection_retry_counts: HashMap::new(),
            scope_violation_circuit_breaker_tripped: None,
            rejection_last_iteration: HashMap::new(),
            // 2026-06-18-001 plan U6: 空 digest,运行时累积。
            recent_rejection_digest: BTreeMap::new(),
            invariant_violation_count: 0,
            last_invariant_violation: None,
            review_step_tracker: super::review_step_state::ReviewStepTracker::default(),
            // WRC-U4: default 30s timeout; the runtime replaces
            // this from `WorkflowContractConfig` in
            // `EventLoop::with_diagnostics` so the per-loop config
            // value (clamped to 120s) takes effect.
            handoff_tracker: crate::workflow_contract::HandoffTracker::new(),
            flow_lifecycle: FlowLifecycleRegistry::new(),
            stall_recovery_counts: HashMap::new(),
            pending_recovery_hat: None,
            pending_synthesizer_timeout: None,
            last_ephemeral_relocations: Vec::new(),
            bootstrap_complete: false,
            bootstrap_failed: false,
            recoverable_exhaustion_buffer: Vec::new(),
            work_done_seen_tasks: HashSet::new(),
            // 2026-06-24 P1-2: fix-round counter starts empty.
            fix_round_counts: HashMap::new(),
            // 2026-06-17-003 U1: state projector is lazily
            // initialised by the first enabled iteration; the
            // cache is empty until then.
            state_projection: None,
            // 2026-06-17-004 U2 (R3): per-hat activation clock for
            // missing-event gate grace window. Empty by default;
            // the loop populates entries as hats are activated.
            hat_activation_at: HashMap::new(),
            // 2026-06-17-004 U3 (R4+R5): empty obligation-trigger
            // snapshot. Populated by the missing-event hard gate
            // before injecting the resume JSON; drained by the
            // runner's `replay_obligation_triggers_to_activation_state`
            // helper after `pending_recovery_hat` is pinned.
            pending_obligation_triggers: Vec::new(),
            // U4b (plan 2026-06-20-001, R13): no lint hint on cold
            // start. The CLI emit path seeds this file when a
            // rejection happens during `ralph emit`; loop_runner
            // loads it into `pending_lint_resume` on the next
            // iteration so the agent sees the resume block on the
            // prompt that immediately follows the rejection.
            pending_lint_resume: None,
            // Plan 2026-06-20-001 KTD-7: cold start with a fresh
            // circuit breaker counter; no trip on iteration 1.
            consecutive_engine_gate_rejections: 0,
            lint_circuit_breaker_tripped: false,
            // 2026-06-23 fix: typed per-kind counters start empty;
            // the first rejection seeds a new bucket.
            consecutive_lint_rejections_by_kind: HashMap::new(),
            // U3 (plan 2026-06-23-004): rejection stall 检测窗口。
            stall_detector_rejection_window: Vec::new(),
            // U1 (plan 2026-06-21-002): unified state ledger.
            // `None` until the loop constructor wires it in.
            state_ledger: None,
            // U7a: deterministic correction queue.
            prompt_context: crate::correction::PromptContext::default(),
            // 2026-06-23-005 F4: typed TerminationTrigger queue
            // starts empty. Callers enqueue via
            // `push_termination_trigger`; `process_output` does
            // NOT consume this field in F4 (the queue is
            // infrastructure-only until the process_output
            // migration lands).
            termination_triggers: VecDeque::new(),
        }
    }
}

/// Progress tracking for a single workflow instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowInstanceProgress {
    /// The chain this instance belongs to.
    pub chain_name: String,
    /// The instance key (e.g., experiment_id) or None for global instances.
    pub instance_key: Option<String>,
    /// The highest phase index reached (0-indexed into the chain's topics).
    pub highest_phase: usize,
}

/// Tracks workflow progress for guarded chains.
///
/// Maps chain_name -> instance_key -> WorkflowInstanceProgress.
/// When a chain has no correlation key, instance_key is None and a single
/// global instance is tracked.
#[derive(Debug, Default)]
pub struct WorkflowProgress {
    /// Per-chain, per-instance progress. The outer HashMap key is chain_name.
    instances: HashMap<String, HashMap<Option<String>, WorkflowInstanceProgress>>,
}

impl WorkflowProgress {
    /// Creates a new empty workflow progress tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the highest phase reached for a given chain and instance.
    pub fn get_phase(&self, chain_name: &str, instance_key: Option<&str>) -> Option<usize> {
        self.instances
            .get(chain_name)?
            .get(&instance_key.map(String::from))
            .map(|p| p.highest_phase)
    }

    /// Returns a reference to the progress for a specific chain/instance.
    pub fn get(
        &self,
        chain_name: &str,
        instance_key: Option<&str>,
    ) -> Option<&WorkflowInstanceProgress> {
        self.instances
            .get(chain_name)?
            .get(&instance_key.map(String::from))
    }

    /// Returns the next valid phase index for a given chain.
    ///
    /// Returns 0 if no progress exists yet. Otherwise returns `highest_phase + 1`.
    pub fn next_phase(&self, chain_name: &str, instance_key: Option<&str>) -> usize {
        self.get_phase(chain_name, instance_key)
            .map(|p| p + 1)
            .unwrap_or(0)
    }

    /// Returns true if the given phase is the next valid one to advance to.
    ///
    /// A phase is valid for advancement if:
    /// - No progress exists yet and phase is 0 (chain start)
    /// - phase equals current highest_phase + 1 (sequential advancement)
    /// - phase equals current highest_phase (idempotent re-emission of same phase)
    pub fn is_phase_valid(
        &self,
        chain_name: &str,
        instance_key: Option<&str>,
        phase: usize,
    ) -> bool {
        let current_highest = self.get_phase(chain_name, instance_key);
        match current_highest {
            None => phase == 0,
            Some(highest) => phase == highest || phase == highest + 1,
        }
    }

    /// Records advancement to a new phase for a chain instance.
    ///
    /// If the given phase is not valid (skipping ahead), this is a no-op.
    /// If the phase is <= current highest, this is idempotent (no update).
    pub fn advance(&mut self, chain_name: &str, instance_key: Option<&str>, phase: usize) {
        if !self.is_phase_valid(chain_name, instance_key, phase) {
            return;
        }

        let current_highest = self.get_phase(chain_name, instance_key);
        if current_highest.is_some_and(|h| phase <= h) {
            // Idempotent: already at or past this phase
            return;
        }

        let instances = self.instances.entry(chain_name.to_string()).or_default();
        let progress = WorkflowInstanceProgress {
            chain_name: chain_name.to_string(),
            instance_key: instance_key.map(String::from),
            highest_phase: phase,
        };
        instances.insert(instance_key.map(String::from), progress);
    }

    /// Returns the total number of tracked instances across all chains.
    pub fn instance_count(&self) -> usize {
        self.instances.values().map(|m| m.len()).sum()
    }

    /// Returns the sum of highest phases across all tracked instances.
    ///
    /// Used as part of the progress fingerprint to detect workflow advancement.
    /// A phase advancement increases this sum, indicating real progress.
    pub fn phase_sum(&self) -> usize {
        self.instances
            .values()
            .flat_map(|m| m.values())
            .map(|p| p.highest_phase)
            .sum()
    }

    /// Returns all tracked instance keys for a given chain.
    pub fn instance_keys(&self, chain_name: &str) -> Vec<Option<String>> {
        self.instances
            .get(chain_name)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }
}

impl LoopState {
    /// Creates a new loop state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the elapsed time since the loop started.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// 把 `started_at: Instant`（进程内 monotonic clock）映射为
    /// `LedgerSnapshot::started_at_ts` 用的 RFC3339 wall-clock 字符串。
    ///
    /// `LoopState::started_at` 是 monotonic 的，跨进程不可序列化；
    /// ledger 的 `started_at_ts` 字段是它的可序列化对照。两次取值
    /// （一次 monotonic 一次 wall-clock）需要在同一调用点抓取，
    /// 否则转换就会失真。这个 helper 在被调用时返回当前
    /// wall-clock（`chrono::Utc::now().to_rfc3339()`），U2 在
    /// `with_context_and_diagnostics` 处一次性塞进 snapshot 即可。
    pub fn started_at_wall_clock(&self) -> String {
        chrono::Utc::now().to_rfc3339()
    }

    /// Increment the per-rejection-key retry counter and return the
    /// post-increment value.  When the result exceeds
    /// [`U2_REJECTION_RETRY_LIMIT`] the caller must mark the rejection
    /// 2026-06-18-001 plan U6: 累积一次 runtime 拒收到 digest。
    ///
    /// - `reason_code` 用于聚合键
    /// - `message` 与 `topic` 给 agent 看的最近一次上下文
    /// - `ts` 用事件 timestamp
    /// - 最多保留 5 个不同 reason_code,超限后淘汰最旧(按 insertion order 用 BTreeMap 不直接 FIFO,
    ///   因此采用"超限清空"——拒绝数本来就不应密集,5 条覆盖常见场景即可)。
    ///
    /// recovery topic (`task.resume` / `human.guidance` / control topics)
    /// 不应进入 digest,调用方负责过滤。
    pub fn record_rejection_digest(
        &mut self,
        reason_code: &str,
        message: &str,
        topic: &str,
        ts: &str,
    ) {
        const MAX_DIGEST_ENTRIES: usize = 5;
        let entry = self
            .recent_rejection_digest
            .entry(reason_code.to_string())
            .or_insert_with(|| RejectionDigestEntry {
                count: 0,
                last_message: String::new(),
                last_ts: String::new(),
                last_topic: String::new(),
            });
        entry.count = entry.count.saturating_add(1);
        entry.last_message = message.to_string();
        entry.last_ts = ts.to_string();
        entry.last_topic = topic.to_string();
        if self.recent_rejection_digest.len() > MAX_DIGEST_ENTRIES {
            // 超限清空(BTreeMap 不直接给 FIFO,简单起见全清,重新累积)
            // 这种极端场景只在 agent 反复跨多类错误时出现,清空让 agent 重读恢复信号
            self.recent_rejection_digest.clear();
        }
    }

    /// 2026-06-18-001 plan U6: 把 digest 格式化成 markdown 注入块。
    /// 空 digest 时返回空字符串。
    pub fn format_rejection_digest_block(&self) -> String {
        if self.recent_rejection_digest.is_empty() {
            return String::new();
        }
        let mut out = String::from("## RECENT REJECTIONS\n\n");
        out.push_str(
            "Your recent emits have been rejected by the runtime. Read each reason and adjust:\n\n",
        );
        for (code, entry) in &self.recent_rejection_digest {
            out.push_str(&format!(
                "- `{code}` × {count} (last at {ts}, topic `{topic}`): {msg}\n",
                code = code,
                count = entry.count,
                ts = entry.last_ts,
                topic = entry.last_topic,
                msg = entry.last_message,
            ));
        }
        out.push_str(
            "\nDo NOT retry the same payload. Read `recovery.jsonl` for full structured reason_code, fix the payload, then re-emit.\n",
        );
        out
    }

    /// as fail-closed (R2: bounded retry to prevent infinite loops).
    pub fn record_rejection_key(&mut self, key: &str) -> u32 {
        let entry = self
            .rejection_retry_counts
            .entry(key.to_string())
            .or_insert(0);
        *entry = entry.saturating_add(1);
        *entry
    }

    /// 2026-06-23 fix plan (mechanism review layer 2, P0-B):
    /// typed variant of [`Self::record_rejection_key`]. The kind
    /// is mapped to its stable `reason_code()` string so the
    /// typed counters and the legacy string-keyed counters stay
    /// aligned (no parallel SSOT). Returns the **post-increment**
    /// count for that exact kind so callers can branch on
    /// `count >= U2_REJECTION_RETRY_LIMIT` to trigger their own
    /// per-kind escalation (drift_finding, circuit_breaker, etc.).
    ///
    /// The follow-up plan `2026-06-21-001 U4` consumes this
    /// signal to:
    ///   - kind `MissingField` × 2 → `drift_finding`
    ///   - kind `*` × 3 → `loop.circuit_breaker_trip`
    ///   - kind `*` × 4 → `plan.blocked(reason=...)`.
    ///
    /// Until that follow-up lands, the typed counter is recorded
    /// but no caller consumes it; this is intentional so the
    /// landing block is the typed call site, not a string match.
    pub fn record_typed_lint_rejection(
        &mut self,
        kind: crate::preset::engine::gates::RejectionKind,
    ) -> u32 {
        let key = kind.reason_code();
        let entry = self
            .consecutive_lint_rejections_by_kind
            .entry(key.to_string())
            .or_insert(0);
        *entry = entry.saturating_add(1);
        *entry
    }

    /// 2026-06-23 fix (anti-pattern 3): current typed count for a
    /// given `RejectionKind`. Returns 0 when no rejection of that
    /// kind has been recorded.
    pub fn typed_lint_rejection_count(
        &self,
        kind: crate::preset::engine::gates::RejectionKind,
    ) -> u32 {
        self.consecutive_lint_rejections_by_kind
            .get(kind.reason_code())
            .copied()
            .unwrap_or(0)
    }

    /// 2026-06-23 fix (anti-pattern 3): clear the typed counter
    /// for a single kind. Called when a downstream hat successfully
    /// publishes a legal event so a stale count does not trigger
    /// a premature fuse for an unrelated later violation. Mirrors
    /// the legacy [`Self::clear_rejection_keys_for_hat`] but
    /// operates on the typed per-kind map.
    pub fn clear_typed_lint_rejection_count(
        &mut self,
        kind: crate::preset::engine::gates::RejectionKind,
    ) {
        self.consecutive_lint_rejections_by_kind
            .remove(kind.reason_code());
    }

    /// Unit 2 (2026-06-16-002 plan) recoverable-payload variant of
    /// [`Self::record_rejection_key`].  The key shape is fixed to
    /// `"policy:{hat}:{topic}:{reason_class}"` so the bucket is the
    /// last segment — two distinct reason classes on the same
    /// `(hat, topic)` keep **independent** counters.
    ///
    /// Returns `(count, exhausted)`.  `count` is the post-increment
    /// value; `exhausted` is `true` iff `count > U2_REJECTION_RETRY_LIMIT`.
    pub fn record_recoverable_rejection_key(
        &mut self,
        hat: &str,
        topic: &str,
        reason_class: &str,
    ) -> (u32, bool) {
        let key = format!("policy:{hat}:{topic}:{reason_class}");
        let count = self.record_rejection_key(&key);
        (count, self.rejection_key_is_exhausted(&key))
    }

    /// Current count of retries observed for a given rejection key.
    /// Returns 0 when the key has never been recorded.
    pub fn rejection_retry_count(&self, key: &str) -> u32 {
        self.rejection_retry_counts.get(key).copied().unwrap_or(0)
    }

    /// Returns `true` if a rejection key has crossed the bounded-retry
    /// threshold.  Used by the runner to decide between
    /// `task.resume` (retryable) and `TerminationReason::Recovered`
    /// (fail-closed escalation).
    ///
    /// Semantics: the budget allows `U2_REJECTION_RETRY_LIMIT` retries.
    /// When the *post-increment* count strictly **exceeds** the limit
    /// the budget is exhausted and the next attempt must terminate.
    /// This means counts 1..=LIMIT all permit a `task.resume`; the
    /// (LIMIT+1)-th attempt is the first one marked exhausted.  The
    /// `>` comparison is intentional — `>=` would silently drop the
    /// last retry attempt.
    pub fn rejection_key_is_exhausted(&self, key: &str) -> bool {
        self.rejection_retry_count(key) > U2_REJECTION_RETRY_LIMIT
    }

    /// 2026-06-14-004 U2: clear all rejection retry counts whose key
    /// belongs to `hat`.  Called when the hat successfully publishes a
    /// legal event, so a single recovery does not leave stale counts
    /// that trigger a premature fuse on a later, unrelated violation.
    pub fn clear_rejection_keys_for_hat(&mut self, hat: &str) {
        let normalized = crate::diagnosis::normalize_part(hat);
        self.rejection_retry_counts
            .retain(|key, _| key.split(':').nth(1) != Some(&normalized));
    }

    /// Build a U4 dedup key from a (plan_name, step, task_id) triple.
    /// Used for `work.done` duplicate detection in event policy.
    pub fn work_done_dedup_key(plan_name: &str, step: &str, task_id: &str) -> String {
        format!("{plan_name}::{step}::{task_id}")
    }

    // ── 2026-06-24 P1-2: fix-round hard cap (Rust-side enforcement) ──

    /// Hard cap on fix rounds per (plan, step, task). Matches the
    /// fixer instructions' "max 10 fix rounds" advisory limit.
    /// This is the runtime hard gate — the agent-side limit is
    /// advisory and can be exceeded by a misbehaving agent.
    pub const FIX_ROUND_HARD_CAP: u32 = 10;

    /// Build a fix-round counter key from a (plan, step, task_id) triple.
    pub fn fix_round_key(plan_name: &str, step: &str, task_id: &str) -> String {
        format!("{plan_name}::{step}::{task_id}")
    }

    /// Current fix-round count for a (plan, step, task). Starts at 0.
    pub fn fix_round_count(&self, plan_name: &str, step: &str, task_id: &str) -> u32 {
        let key = Self::fix_round_key(plan_name, step, task_id);
        self.fix_round_counts.get(&key).copied().unwrap_or(0)
    }

    /// Increment the fix-round counter for a (plan, step, task).
    /// Returns the new count after increment.
    pub fn increment_fix_round(&mut self, plan_name: &str, step: &str, task_id: &str) -> u32 {
        let key = Self::fix_round_key(plan_name, step, task_id);
        let count = self.fix_round_counts.entry(key).or_insert(0);
        *count += 1;
        *count
    }

    /// Returns true when the fix-round counter has reached the hard cap.
    pub fn fix_round_exhausted(&self, plan_name: &str, step: &str, task_id: &str) -> bool {
        self.fix_round_count(plan_name, step, task_id) >= Self::FIX_ROUND_HARD_CAP
    }

    /// Prune the fix-round counter for a (plan, step) bucket.
    /// Called on step-close to free memory.
    pub fn prune_fix_round_bucket(&mut self, plan_name: &str, step: &str) {
        let prefix = format!("{plan_name}::{step}::");
        self.fix_round_counts
            .retain(|key, _| !key.starts_with(&prefix));
    }

    /// U4 (2026-06-17-003 plan): prune the dedup-set entries that
    /// belong to a given `(plan_name, step)` bucket. Called on
    /// `queue.advance` (step close), `review.failed` (fix-round
    /// re-emit window opens), and `fix.applied` (fix-round
    /// completed). After pruning, a new `work.done` for the same
    /// `(plan_name, step, task_id)` can be accepted by policy.
    pub fn prune_work_done_bucket(&mut self, plan_name: &str, step: &str) {
        let prefix = format!("{plan_name}::{step}::");
        self.work_done_seen_tasks
            .retain(|key| !key.starts_with(&prefix));
    }

    /// 2026-06-17-004 U2 (R3): record/refresh the per-hat
    /// activation clock.  Called from
    /// `event_loop/mod.rs::process_output` whenever a hat is
    /// selected to execute an agent.  Subsequent activations
    /// REPLACE the timestamp so a hat that loops through several
    /// short activations (e.g. executor retrying on a transient
    /// contract failure) does not accumulate a stale "first
    /// activation" that suppresses the gate across many turns.
    /// The default zero-duration return means "no clock
    /// recorded" — the missing-event gate does not consult
    /// `hat_activation_at` at all when the hat has never been
    /// activated (e.g. fresh deployment, tests that bypass
    /// `process_output`).
    pub fn record_hat_activation(&mut self, hat_id: &HatId) {
        self.hat_activation_at
            .insert(hat_id.clone(), Instant::now());
    }

    /// 2026-06-17-004 U2 (R3): how long since the hat was last
    /// activated, or `None` when no activation has been recorded.
    /// Used by [`crate::event_loop::hard_gate::should_gate_missing_events`]
    /// to defer the missing-event gate during the per-hat grace
    /// window.
    pub fn hat_activation_elapsed(&self, hat_id: &HatId) -> Option<Duration> {
        self.hat_activation_at
            .get(hat_id)
            .map(|when| when.elapsed())
    }

    /// 2026-06-17-004 U3 (R4+R5): drain the obligation-trigger
    /// snapshot into `last_activation_events` so the next hat
    /// activation (the one woken by the recovery `task.resume`)
    /// sees the original trigger topic — typically
    /// `review.dimension.ready` for `dimension-reviewer`.  Without
    /// the replay, the next activation's `last_activation_events`
    /// would be empty (only the resume itself is in flight), and
    /// the obligation check in `should_gate_missing_events` would
    /// not know which trigger event the hat is responding to.
    ///
    /// Called by the runner after `pending_recovery_hat` is pinned
    /// and before the next `process_output` runs.  Returns the
    /// drained events for callers that want to log them; the
    /// in-place side effect (mutating `last_activation_events`) is
    /// the load-bearing piece.
    pub fn replay_obligation_triggers_to_activation_state(&mut self) -> Vec<Event> {
        let drained = std::mem::take(&mut self.pending_obligation_triggers);
        if !drained.is_empty() {
            self.last_activation_events = drained.clone();
        }
        drained
    }

    fn event_counts_toward_stale_loop(event: &Event) -> bool {
        !matches!(event.topic.as_str(), "task.complete")
    }

    /// Record that an event has been seen during this loop run.
    ///
    /// Also tracks consecutive same-signature emissions for stale loop detection.
    pub fn record_event(&mut self, event: &Event) {
        self.seen_topics.insert(event.topic.to_string());

        if !Self::event_counts_toward_stale_loop(event) {
            self.consecutive_same_signature = 0;
            self.last_emitted_signature = Some(EventSignature::from_event(event));
            return;
        }

        let signature = EventSignature::from_event(event);
        if self.last_emitted_signature.as_ref() == Some(&signature) {
            self.consecutive_same_signature += 1;
        } else {
            self.consecutive_same_signature = 1;
            self.last_emitted_signature = Some(signature);
        }
    }

    /// Check if all required topics have been seen.
    pub fn missing_required_events<'a>(&self, required: &'a [String]) -> Vec<&'a String> {
        required
            .iter()
            .filter(|topic| !self.seen_topics.contains(topic.as_str()))
            .collect()
    }

    /// Records the payload of an event if its topic matches the configured verdict gate.
    ///
    /// Called alongside `record_event` at every site. The most recent matching
    /// event's payload is retained so `check_completion_event` can read the
    /// verdict without re-scanning event history. No-op when `verdict_topics`
    /// is `None` / empty or the event topic does not match any entry.
    ///
    /// 2026-06-09 fix: now accepts a slice of topics so a single
    /// gate can cover both the upstream verdict topic (e.g.
    /// `REVIEW_COMPLETE`) and downstream events that mirror the
    /// verdict payload (e.g. `report.done`).  When ANY of the
    /// listed topics carries a failing verdict, the gate fires.
    pub fn record_verdict_if_match(&mut self, event: &Event, verdict_topics: Option<&[String]>) {
        let Some(topics) = verdict_topics else {
            return;
        };
        if topics.is_empty() {
            return;
        }
        if topics.iter().any(|t| t == event.topic.as_str()) {
            // 2026-06-10 P0-C fix: track the topic alongside the payload
            // so the fail-path auto-termination check can detect when
            // the verdict has propagated to the LAST configured mirror
            // topic (e.g. `report.done`).
            self.last_verdict_topic = Some(event.topic.to_string());
            self.last_verdict_payload = Some(event.payload.clone());

            // 2026-06-17-002 U6: keep the upstream verdict payload
            // separate from downstream mirrors. A fake pass on a mirror
            // must not erase an upstream fail verdict.
            if let Some(upstream) = topics.first()
                && upstream == event.topic.as_str()
            {
                self.last_upstream_verdict_payload = Some(event.payload.clone());
            }
        }
    }

    /// Computes a composite progress fingerprint capturing all meaningful progress signals.
    ///
    /// The fingerprint includes:
    /// - Count of accepted business topics (excludes system/diagnostic topics)
    /// - Task store snapshot (open/closed counts)
    /// - Workflow instance count and phase sum
    /// - State machine transition count
    ///
    /// This replaces the naive `seen_topics.len()` check which could be fooled by
    /// irrelevant topics (e.g., `event.malformed`, `human.guidance`).
    pub fn compute_progress_fingerprint(&self) -> ProgressFingerprint {
        // Count only business topics (exclude system/diagnostic/recovery topics)
        let accepted_business_count = self
            .seen_topics
            .iter()
            .filter(|t| !Self::is_system_topic(t))
            .count();

        // Workflow progress: instance count and sum of all highest phases
        let workflow_instances = self.workflow_progress.instance_count();
        let workflow_phase_sum = self.workflow_progress.phase_sum();

        // SM transition count (0 when disabled)
        let sm_transition_count = self
            .state_machine_runtime_state
            .as_ref()
            .map(|sm| sm.accepted_transition_count())
            .unwrap_or(0);

        ProgressFingerprint {
            accepted_business_count,
            task_snapshot: (0, 0), // Caller must fill in task counts
            workflow_instances,
            workflow_phase_sum,
            sm_transition_count,
        }
    }

    /// Returns true if the given topic is a system/diagnostic topic that should
    /// not count as business progress.
    fn is_system_topic(topic: &str) -> bool {
        matches!(
            topic,
            "task.resume"
                | "task.start"
                | "event.malformed"
                | "event.scope_violation"
                | "event.workflow_guard_rejected"
                | "event.state_machine.rejected"
                | "event.state_machine.ignored"
                | "event.state_machine.diagnostic"
                | "event.policy_warning"
                | "event.completion.blocked"
                | "event.completion.ignored"
                | "event.isolation.boundary_violation"
                | "event.step_handoff.gate_rejected"
                | "human.interact"
                | "human.response"
                | "human.guidance"
                | "human.timeout"
                | "loop.cancel"
                | "build.task.abandoned"
        ) || topic.ends_with(".exhausted")
            || topic.ends_with(".scope_violation")
    }

    /// 2026-06-23-005 F4: enqueue a typed termination trigger.
    /// Returns `Err` when the queue is at
    /// [`TRIGGER_QUEUE_CAPACITY`]; the caller decides whether
    /// to force-terminate (e.g. by translating the overflow
    /// into a `TerminationReason::QueueOverflow`) or drop the
    /// trigger.
    ///
    /// **Status (F4)**: infrastructure-only. `process_output`
    /// does not consume this queue yet. Future plan (R15
    /// follow-up) wires the single-match dispatch.
    pub fn push_termination_trigger(
        &mut self,
        trigger: TerminationTrigger,
    ) -> Result<(), &'static str> {
        if self.termination_triggers.len() >= TRIGGER_QUEUE_CAPACITY {
            return Err("TerminationTrigger queue at capacity");
        }
        self.termination_triggers.push_back(trigger);
        Ok(())
    }

    /// 2026-06-23-005 F4: FIFO-pop the next typed termination
    /// trigger. Returns `None` when the queue is empty.
    pub fn pop_termination_trigger(&mut self) -> Option<TerminationTrigger> {
        self.termination_triggers.pop_front()
    }

    /// 2026-06-23-005 F4: number of pending triggers in the
    /// queue. Useful for diagnostics and for the future
    /// `process_output` migration to decide between
    /// dispatching one trigger vs. all triggers.
    pub fn termination_trigger_queue_len(&self) -> usize {
        self.termination_triggers.len()
    }

    /// U3 (plan 2026-06-23-004): 推入一轮窗口条目,保留最近 N=5 轮。
    ///
    /// `rejection_count` 通常是 `typed_lint_rejection_count` 累计值,
    /// `emit_count` 是本轮通过 gate 的合法 business event 数。
    pub fn push_rejection_window(&mut self, entry: RejectionWindowEntry) {
        self.stall_detector_rejection_window.push(entry);
        // 保留最近 N 轮,旧数据自动出队。
        let len = self.stall_detector_rejection_window.len();
        if len > REJECTION_WINDOW_SIZE {
            let drop_n = len - REJECTION_WINDOW_SIZE;
            self.stall_detector_rejection_window.drain(0..drop_n);
        }
    }

    /// U3 (plan 2026-06-23-004): 当前窗口内 reject / emit 累计求和。
    ///
    /// 返回 `(sum_rejections, sum_emits)`。stall 触发条件:
    /// `sum_rejections >= REJECTION_WINDOW_THRESHOLD && sum_emits == 0`。
    pub fn rejection_window_sums(&self) -> (u32, u32) {
        let mut rej = 0u32;
        let mut emit = 0u32;
        for entry in &self.stall_detector_rejection_window {
            rej = rej.saturating_add(entry.rejection_count);
            emit = emit.saturating_add(entry.emit_count);
        }
        (rej, emit)
    }
}

impl EventSignature {
    pub fn from_event(event: &Event) -> Self {
        Self {
            topic: event.topic.to_string(),
            source: event.source.clone(),
            payload_fingerprint: fingerprint_payload(&event.payload),
        }
    }
}

fn fingerprint_payload(payload: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    payload.hash(&mut hasher);
    hasher.finish()
}

/// U3 (plan 2026-06-23-004, anti-pattern 3): rejection stall 检查。
///
/// 纯函数:输入当前 `LoopState`,返回 `Some(())` 表示检测到 stall(应 emit
/// `stall.handoff_unconsumed` 报警);`None` 表示正常。
///
/// 阈值复用 `REJECTION_WINDOW_SIZE` × `REJECTION_WINDOW_THRESHOLD`:
/// - 窗口大小 = 5 轮(最近 N 轮)
/// - 累计 rejection ≥ 3 && emit == 0
pub fn detect_rejection_stall(state: &LoopState) -> bool {
    let (sum_rej, sum_emit) = state.rejection_window_sums();
    sum_rej >= REJECTION_WINDOW_THRESHOLD && sum_emit == 0
}

/// 2026-06-23 fix plan U3 (CB-6): typed rejection-stall
/// detector. Walks the per-kind counter map and returns the
/// first kind whose count meets or exceeds
/// `REJECTION_WINDOW_THRESHOLD`. Used by the loop to emit
/// `stall.handoff_unconsumed` with the specific kind
/// (the bool-only `detect_rejection_stall` cannot tell which
/// kind tripped). Order: Handoff* kinds first
/// (the ones with no remediation in the executor) then the
/// others, so the diagnostic carries the most actionable
/// kind.
pub fn detect_rejection_stall_kind(
    state: &LoopState,
) -> Option<crate::preset::engine::gates::RejectionKind> {
    use crate::preset::engine::gates::RejectionKind;
    let order = [
        RejectionKind::MissingField,
        RejectionKind::TopicOwnership,
        RejectionKind::UpstreamState,
        RejectionKind::PreCheck,
    ];
    for kind in order {
        if state.typed_lint_rejection_count(kind) >= REJECTION_WINDOW_THRESHOLD {
            return Some(kind);
        }
    }
    None
}

#[cfg(test)]
mod detect_rejection_stall_kind_tests {
    //! 2026-06-23 fix plan U3 (CB-6): the typed `detect_rejection_stall_kind`
    //! helper MUST surface the first kind whose count meets the
    //! typed threshold so the runtime can emit
    //! `stall.handoff_unconsumed` with the actionable kind.

    use super::*;
    use crate::preset::engine::gates::RejectionKind;

    #[test]
    fn no_rejections_returns_none() {
        let state = LoopState::new();
        assert!(detect_rejection_stall_kind(&state).is_none());
    }

    #[test]
    fn below_threshold_returns_none() {
        let mut state = LoopState::new();
        for _ in 0..(REJECTION_WINDOW_THRESHOLD - 1) {
            state.record_typed_lint_rejection(RejectionKind::MissingField);
        }
        assert_eq!(
            state.typed_lint_rejection_count(RejectionKind::MissingField),
            REJECTION_WINDOW_THRESHOLD - 1
        );
        assert!(detect_rejection_stall_kind(&state).is_none());
    }

    #[test]
    fn at_threshold_returns_first_handoff_kind() {
        let mut state = LoopState::new();
        for _ in 0..REJECTION_WINDOW_THRESHOLD {
            state.record_typed_lint_rejection(RejectionKind::MissingField);
        }
        assert_eq!(
            detect_rejection_stall_kind(&state),
            Some(RejectionKind::MissingField)
        );
    }

    /// 2026-06-23 fix plan U3 (CB-6): the helper must trip on the
    /// exact threshold (3 in `REJECTION_WINDOW_THRESHOLD`), not on
    /// a count strictly greater — used by
    /// `run_stall_detector_on_state` to emit
    /// `stall.handoff_unconsumed`.
    #[test]
    fn stall_at_5_reject_rounds_triggers_stall() {
        let mut state = LoopState::new();
        // 5 rounds (well past the threshold) — the primary-20260622-182705
        // case was filename_mismatch × 6.
        for _ in 0..5 {
            state.record_typed_lint_rejection(RejectionKind::MissingField);
        }
        assert_eq!(
            state.typed_lint_rejection_count(RejectionKind::MissingField),
            5
        );
        assert_eq!(
            detect_rejection_stall_kind(&state),
            Some(RejectionKind::MissingField),
            "kind helper surfaces the actionable Handoff* kind"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{LoopState, U2_REJECTION_RETRY_LIMIT, WorkflowProgress};
    use ralph_proto::Event;

    #[test]
    fn repeated_task_complete_does_not_accumulate_stale_loop_count() {
        let mut state = LoopState::new();

        state.record_event(&Event::new("task.complete", "task 1 complete"));
        assert_eq!(state.consecutive_same_signature, 0);

        state.record_event(&Event::new("task.complete", "task 2 complete"));
        state.record_event(&Event::new("task.complete", "task 3 complete"));

        assert_eq!(state.consecutive_same_signature, 0);
        assert_eq!(
            state
                .last_emitted_signature
                .as_ref()
                .map(|s| s.topic.as_str()),
            Some("task.complete")
        );
    }

    #[test]
    fn repeated_non_progress_topics_still_accumulate_stale_loop_count() {
        let mut state = LoopState::new();

        state.record_event(&Event::new("task.resume", "same payload"));
        state.record_event(&Event::new("task.resume", "same payload"));
        state.record_event(&Event::new("task.resume", "same payload"));

        assert_eq!(state.consecutive_same_signature, 3);
        assert_eq!(
            state
                .last_emitted_signature
                .as_ref()
                .map(|s| s.topic.as_str()),
            Some("task.resume")
        );
    }

    // -------------------------------------------------------------------------
    // WorkflowProgress tests
    // -------------------------------------------------------------------------

    #[test]
    fn workflow_progress_single_instance_sequential_phases() {
        // Test: one experiment progresses through all configured topics in order.
        let mut progress = WorkflowProgress::new();
        let chain = "experiment";
        let instance: Option<&str> = None; // global instance

        // Phase 0: experiment.planned
        progress.advance(chain, instance, 0);
        assert_eq!(progress.get_phase(chain, instance), Some(0));

        // Phase 1: experiment.ready
        progress.advance(chain, instance, 1);
        assert_eq!(progress.get_phase(chain, instance), Some(1));

        // Phase 2: experiment.measured
        progress.advance(chain, instance, 2);
        assert_eq!(progress.get_phase(chain, instance), Some(2));

        // Phase 3: experiment.scored
        progress.advance(chain, instance, 3);
        assert_eq!(progress.get_phase(chain, instance), Some(3));

        // Phase 4: experiment.evaluated
        progress.advance(chain, instance, 4);
        assert_eq!(progress.get_phase(chain, instance), Some(4));

        assert_eq!(progress.instance_count(), 1);
    }

    #[test]
    fn workflow_progress_two_instances_independent() {
        // Test: two experiment IDs progress independently.
        let mut progress = WorkflowProgress::new();
        let chain = "experiment";

        // Experiment 1: scored (phase 3)
        progress.advance(chain, Some("exp-1"), 0);
        progress.advance(chain, Some("exp-1"), 1);
        progress.advance(chain, Some("exp-1"), 2);
        progress.advance(chain, Some("exp-1"), 3);

        // Experiment 2: only at measured (phase 2)
        progress.advance(chain, Some("exp-2"), 0);
        progress.advance(chain, Some("exp-2"), 1);
        progress.advance(chain, Some("exp-2"), 2);

        assert_eq!(progress.get_phase(chain, Some("exp-1")), Some(3));
        assert_eq!(progress.get_phase(chain, Some("exp-2")), Some(2));

        // exp-1's scored should NOT advance exp-2
        progress.advance(chain, Some("exp-2"), 3); // This should work since exp-2 is at phase 2, and 3 == 2+1
        assert_eq!(progress.get_phase(chain, Some("exp-2")), Some(3));
    }

    #[test]
    fn workflow_progress_instance_isolation() {
        // Test: experiment.scored for experiment 1 does not allow
        // experiment.evaluated for experiment 2 (instance isolation).
        let mut progress = WorkflowProgress::new();
        let chain = "experiment";

        // Experiment 1 is at scored (phase 3)
        progress.advance(chain, Some("exp-1"), 0);
        progress.advance(chain, Some("exp-1"), 1);
        progress.advance(chain, Some("exp-1"), 2);
        progress.advance(chain, Some("exp-1"), 3);
        assert_eq!(progress.get_phase(chain, Some("exp-1")), Some(3));

        // Experiment 2 is only at measured (phase 2) - cannot skip to evaluated (phase 4)
        progress.advance(chain, Some("exp-2"), 0);
        progress.advance(chain, Some("exp-2"), 1);
        progress.advance(chain, Some("exp-2"), 2);
        assert_eq!(progress.get_phase(chain, Some("exp-2")), Some(2));

        // Attempt to advance exp-2 to evaluated (phase 4) should be rejected
        // because current highest is 2, and 4 > 2 + 1
        progress.advance(chain, Some("exp-2"), 4);
        assert_eq!(
            progress.get_phase(chain, Some("exp-2")),
            Some(2),
            "exp-2 should remain at phase 2 - cannot skip to evaluated"
        );

        // But exp-2 can advance to scored (phase 3)
        progress.advance(chain, Some("exp-2"), 3);
        assert_eq!(progress.get_phase(chain, Some("exp-2")), Some(3));

        // Now exp-2 can advance to evaluated (phase 4)
        progress.advance(chain, Some("exp-2"), 4);
        assert_eq!(progress.get_phase(chain, Some("exp-2")), Some(4));
    }

    #[test]
    fn workflow_progress_idempotent_same_phase() {
        // Test: repeated same-phase event is handled idempotently.
        let mut progress = WorkflowProgress::new();
        let chain = "experiment";
        let instance = Some("exp-1");

        // Phase 0
        progress.advance(chain, instance, 0);
        assert_eq!(progress.get_phase(chain, instance), Some(0));

        // Re-emit same phase 0 - should be idempotent (no change)
        progress.advance(chain, instance, 0);
        assert_eq!(progress.get_phase(chain, instance), Some(0));

        // Advance to phase 1
        progress.advance(chain, instance, 1);
        assert_eq!(progress.get_phase(chain, instance), Some(1));

        // Re-emit phase 0 again - should still be idempotent
        progress.advance(chain, instance, 0);
        assert_eq!(progress.get_phase(chain, instance), Some(1));

        // Re-emit phase 1 - should be idempotent
        progress.advance(chain, instance, 1);
        assert_eq!(progress.get_phase(chain, instance), Some(1));
    }

    #[test]
    fn workflow_progress_global_vs_per_instance() {
        // Test: chains with no correlation key share a global instance (None key).
        let mut progress = WorkflowProgress::new();
        let chain = "experiment";

        // Global instance advances
        progress.advance(chain, None, 0);
        progress.advance(chain, None, 1);
        assert_eq!(progress.get_phase(chain, None), Some(1));

        // Per-instance tracking is independent
        progress.advance(chain, Some("exp-1"), 0);
        assert_eq!(progress.get_phase(chain, Some("exp-1")), Some(0));
        assert_eq!(progress.get_phase(chain, None), Some(1));

        // Global and per-instance are separate entries
        assert_eq!(progress.instance_count(), 2);
        let global_keys = progress.instance_keys(chain);
        assert!(global_keys.contains(&None));
        assert!(global_keys.contains(&Some("exp-1".to_string())));
    }

    #[test]
    fn workflow_progress_is_phase_valid() {
        let mut progress = WorkflowProgress::new();
        let chain = "experiment";
        let instance = Some("exp-1");

        // No progress yet: only phase 0 is valid
        assert!(progress.is_phase_valid(chain, instance, 0));
        assert!(!progress.is_phase_valid(chain, instance, 1)); // skipping
        assert!(!progress.is_phase_valid(chain, instance, 4)); // way ahead

        // At phase 2: can accept 2 (idempotent re-emit), 3 (next)
        progress.advance(chain, instance, 0);
        progress.advance(chain, instance, 1);
        progress.advance(chain, instance, 2);
        assert_eq!(progress.get_phase(chain, instance), Some(2));

        assert!(!progress.is_phase_valid(chain, instance, 0)); // old phase — no longer accepted
        assert!(!progress.is_phase_valid(chain, instance, 1)); // old phase — no longer accepted
        assert!(progress.is_phase_valid(chain, instance, 2)); // idempotent re-emit
        assert!(progress.is_phase_valid(chain, instance, 3)); // next
        assert!(!progress.is_phase_valid(chain, instance, 4)); // skip
    }

    #[test]
    fn u2_rejection_retry_counter_increments_and_saturates() {
        // 2026-06-07 plan U2: per-key retry counter must increment
        // monotonically and surface exhaustion at the configured limit.
        // Semantics: budget allows U2_REJECTION_RETRY_LIMIT retries
        // (counts 1..=LIMIT all permit a task.resume); the (LIMIT+1)-th
        // attempt is the first one marked exhausted.
        let mut state = LoopState::new();
        let key = "execution_contract:executor:work.done:missing_field";

        assert_eq!(state.rejection_retry_count(key), 0);
        assert!(!state.rejection_key_is_exhausted(key));

        // First LIMIT attempts must all be marked *not* exhausted so the
        // runner actually issues task.resume for each.
        for i in 1..=U2_REJECTION_RETRY_LIMIT {
            let n = state.record_rejection_key(key);
            assert_eq!(n, i, "post-increment count must match attempt {i}");
            assert!(
                !state.rejection_key_is_exhausted(key),
                "attempt {i} (count={i}) must NOT be exhausted; \
                 the budget is `LIMIT` retries, not `LIMIT-1`"
            );
        }

        // The (LIMIT+1)-th attempt is the first exhausted one.
        let n = state.record_rejection_key(key);
        assert_eq!(n, U2_REJECTION_RETRY_LIMIT + 1);
        assert!(
            state.rejection_key_is_exhausted(key),
            "attempt {} (count={}) MUST be exhausted",
            U2_REJECTION_RETRY_LIMIT + 1,
            U2_REJECTION_RETRY_LIMIT + 1
        );

        // Further attempts are still allowed to increment (saturating) —
        // the runner is the one that reads `is_exhausted` and stops
        // publishing `task.resume`.  We must not panic on overflow.
        let n = state.record_rejection_key(key);
        assert_eq!(n, U2_REJECTION_RETRY_LIMIT + 2);
        assert!(state.rejection_key_is_exhausted(key));
    }

    #[test]
    fn u2_rejection_retry_keys_are_independent() {
        // Two distinct rejection keys must not share counts.
        let mut state = LoopState::new();
        let k1 = "execution_contract:executor:work.done:missing_field";
        let k2 = "execution_contract:executor:work.done:type_mismatch";
        state.record_rejection_key(k1);
        state.record_rejection_key(k1);
        state.record_rejection_key(k2);
        assert_eq!(state.rejection_retry_count(k1), 2);
        assert_eq!(state.rejection_retry_count(k2), 1);
        assert!(!state.rejection_key_is_exhausted(k1));
        assert!(!state.rejection_key_is_exhausted(k2));
    }

    #[test]
    fn u2_rejection_retry_counter_starts_at_zero_for_unknown_key() {
        let state = LoopState::new();
        assert_eq!(state.rejection_retry_count("nonexistent"), 0);
        assert!(!state.rejection_key_is_exhausted("nonexistent"));
    }

    // -------------------------------------------------------------------------
    // U4 (2026-06-17-003 plan): work_done dedup key + bucket pruning
    // -------------------------------------------------------------------------

    #[test]
    fn u4_work_done_dedup_key_format() {
        // Key format is `plan_name::step::task_id` to match the
        // event-policy dedup check.
        assert_eq!(
            LoopState::work_done_dedup_key("p1", "step-01", "t1"),
            "p1::step-01::t1"
        );
    }

    #[test]
    fn u4_prune_work_done_bucket_removes_step_entries() {
        let mut state = LoopState::new();
        // Two entries in the (p1, step-01) bucket
        state
            .work_done_seen_tasks
            .insert(LoopState::work_done_dedup_key("p1", "step-01", "t1"));
        state
            .work_done_seen_tasks
            .insert(LoopState::work_done_dedup_key("p1", "step-01", "t2"));
        // One entry in a different step bucket
        state
            .work_done_seen_tasks
            .insert(LoopState::work_done_dedup_key("p1", "step-02", "t1"));

        state.prune_work_done_bucket("p1", "step-01");

        // step-01 bucket is gone
        assert!(!state.work_done_seen_tasks.contains("p1::step-01::t1"));
        assert!(!state.work_done_seen_tasks.contains("p1::step-01::t2"));
        // step-02 bucket is preserved (different step key)
        assert!(state.work_done_seen_tasks.contains("p1::step-02::t1"));
    }

    #[test]
    fn u4_prune_work_done_bucket_empty_state_is_noop() {
        let mut state = LoopState::new();
        state.prune_work_done_bucket("p1", "step-01");
        assert!(state.work_done_seen_tasks.is_empty());
    }

    // ── 2026-06-24 P1-2: fix-round hard cap tests ──

    #[test]
    fn fix_round_hard_cap_is_ten() {
        assert_eq!(LoopState::FIX_ROUND_HARD_CAP, 10);
    }

    #[test]
    fn fix_round_count_starts_at_zero() {
        let state = LoopState::new();
        assert_eq!(state.fix_round_count("p1", "step-01", "t1"), 0);
        assert!(!state.fix_round_exhausted("p1", "step-01", "t1"));
    }

    #[test]
    fn fix_round_increment_advances_counter() {
        let mut state = LoopState::new();
        assert_eq!(state.increment_fix_round("p1", "step-01", "t1"), 1);
        assert_eq!(state.increment_fix_round("p1", "step-01", "t1"), 2);
        assert_eq!(state.fix_round_count("p1", "step-01", "t1"), 2);
    }

    #[test]
    fn fix_round_exhausted_at_cap() {
        let mut state = LoopState::new();
        for _ in 0..LoopState::FIX_ROUND_HARD_CAP {
            state.increment_fix_round("p1", "step-01", "t1");
        }
        assert!(state.fix_round_exhausted("p1", "step-01", "t1"));
        // A different task in the same step is NOT exhausted
        assert!(!state.fix_round_exhausted("p1", "step-01", "t2"));
    }

    #[test]
    fn fix_round_prune_clears_step() {
        let mut state = LoopState::new();
        state.increment_fix_round("p1", "step-01", "t1");
        state.increment_fix_round("p1", "step-01", "t2");
        state.increment_fix_round("p1", "step-02", "t1");

        state.prune_fix_round_bucket("p1", "step-01");

        // step-01 entries are gone
        assert_eq!(state.fix_round_count("p1", "step-01", "t1"), 0);
        assert_eq!(state.fix_round_count("p1", "step-01", "t2"), 0);
        // step-02 entry is preserved
        assert_eq!(state.fix_round_count("p1", "step-02", "t1"), 1);
    }

    #[test]
    fn fix_round_key_is_namespaced_per_plan_step_task() {
        // Different plans / steps / tasks have independent counters
        let mut state = LoopState::new();
        state.increment_fix_round("p1", "step-01", "t1");
        state.increment_fix_round("p1", "step-01", "t1");
        state.increment_fix_round("p2", "step-01", "t1");

        assert_eq!(state.fix_round_count("p1", "step-01", "t1"), 2);
        assert_eq!(state.fix_round_count("p2", "step-01", "t1"), 1);
        assert_eq!(state.fix_round_count("p1", "step-02", "t1"), 0);
    }

    // ── 2026-06-18-001 plan U6: rejection digest ──────────

    #[test]
    fn u6_record_rejection_digest_increments_count() {
        let mut state = LoopState::new();
        state.record_rejection_digest(
            "missing_payload_field",
            "no field in payload",
            "work.ready",
            "t1",
        );
        state.record_rejection_digest(
            "missing_payload_field",
            "still no field",
            "work.ready",
            "t2",
        );
        let entry = state
            .recent_rejection_digest
            .get("missing_payload_field")
            .unwrap();
        assert_eq!(entry.count, 2);
        assert_eq!(entry.last_ts, "t2");
        assert_eq!(entry.last_topic, "work.ready");
    }

    #[test]
    fn u6_record_rejection_digest_different_codes_kept() {
        let mut state = LoopState::new();
        state.record_rejection_digest("code_a", "msg_a", "topic_a", "t1");
        state.record_rejection_digest("code_b", "msg_b", "topic_b", "t2");
        assert_eq!(state.recent_rejection_digest.len(), 2);
    }

    #[test]
    fn u6_format_block_empty_digest_returns_empty() {
        let state = LoopState::new();
        assert!(state.format_rejection_digest_block().is_empty());
    }

    #[test]
    fn u6_format_block_includes_recent_rejections_header() {
        let mut state = LoopState::new();
        state.record_rejection_digest(
            "isolated_scope_violation",
            "executor cannot publish review.passed",
            "review.passed",
            "t1",
        );
        let block = state.format_rejection_digest_block();
        assert!(block.contains("## RECENT REJECTIONS"));
        assert!(block.contains("isolated_scope_violation"));
        assert!(block.contains("review.passed"));
        assert!(block.contains("recovery.jsonl"));
    }

    #[test]
    fn u6_record_rejection_digest_caps_at_max_entries() {
        let mut state = LoopState::new();
        // MAX_DIGEST_ENTRIES=5, 第 6 个不同 code 后触发清空
        for i in 0..6 {
            state.record_rejection_digest(&format!("code_{i}"), "msg", "topic", "t");
        }
        // 第 6 个 code 写入后 len=6 > 5,触发清空
        assert!(
            state.recent_rejection_digest.is_empty(),
            "digest 应在第 6 个不同 code 时清空,避免无限增长"
        );
    }

    // ── 2026-06-23-005 F4: typed TerminationTrigger queue API.
    // Infrastructure-only — `process_output` does not consume
    // these triggers yet (see the module-level docs on
    // `event_loop::termination` for the rationale).

    #[test]
    fn f4_termination_trigger_queue_starts_empty() {
        let mut state = LoopState::new();
        assert_eq!(state.termination_trigger_queue_len(), 0);
        assert!(state.pop_termination_trigger().is_none());
    }

    #[test]
    fn f4_termination_trigger_queue_push_then_pop_fifo() {
        use crate::event_loop::termination::TerminationTrigger;
        let mut state = LoopState::new();
        assert!(
            state
                .push_termination_trigger(TerminationTrigger::PlanComplete {
                    plan_id: "p1".to_string()
                })
                .is_ok()
        );
        assert!(
            state
                .push_termination_trigger(TerminationTrigger::PlanComplete {
                    plan_id: "p2".to_string()
                })
                .is_ok()
        );
        assert_eq!(state.termination_trigger_queue_len(), 2);

        // FIFO order: first pushed is first popped.
        let first = state.pop_termination_trigger();
        assert!(matches!(
            first,
            Some(TerminationTrigger::PlanComplete { ref plan_id }) if plan_id == "p1"
        ));
        let second = state.pop_termination_trigger();
        assert!(matches!(
            second,
            Some(TerminationTrigger::PlanComplete { ref plan_id }) if plan_id == "p2"
        ));
        assert!(state.pop_termination_trigger().is_none());
    }

    #[test]
    fn f4_termination_trigger_queue_overflow_returns_err() {
        use crate::event_loop::termination::{TRIGGER_QUEUE_CAPACITY, TerminationTrigger};
        let mut state = LoopState::new();
        for i in 0..TRIGGER_QUEUE_CAPACITY {
            let result = state.push_termination_trigger(TerminationTrigger::PlanComplete {
                plan_id: format!("p{i}"),
            });
            assert!(result.is_ok(), "push #{i} should succeed");
        }
        // The (TRIGGER_QUEUE_CAPACITY + 1)-th push returns Err.
        let overflow = state.push_termination_trigger(TerminationTrigger::PlanComplete {
            plan_id: "overflow".to_string(),
        });
        assert!(
            overflow.is_err(),
            "push beyond TRIGGER_QUEUE_CAPACITY must return Err, not panic"
        );
        assert_eq!(
            state.termination_trigger_queue_len(),
            TRIGGER_QUEUE_CAPACITY
        );
    }

    // ── 2026-06-23 fix (mechanism review layer 2 P0-B): typed
    // per-kind rejection counters. The 4 historical recovery.jsonl
    // entries from primary-20260622-182705 were all `outcome=failed`
    // because the same retry_key + same kind were never aggregated
    // across iterations. The typed counter is the new SSOT. ──

    use crate::preset::engine::gates::RejectionKind;

    #[test]
    fn typed_lint_rejection_count_buckets_per_kind() {
        let mut state = LoopState::new();
        // Same kind twice — counter accumulates in ONE bucket.
        let n1 = state.record_typed_lint_rejection(RejectionKind::MissingField);
        let n2 = state.record_typed_lint_rejection(RejectionKind::MissingField);
        assert_eq!(n1, 1);
        assert_eq!(n2, 2);
        assert_eq!(
            state.typed_lint_rejection_count(RejectionKind::MissingField),
            2,
            "second rejection of the same kind MUST land in the same bucket"
        );
        // Different kind — independent bucket.
        state.record_typed_lint_rejection(RejectionKind::TopicOwnership);
        assert_eq!(
            state.typed_lint_rejection_count(RejectionKind::TopicOwnership),
            1,
            "different kind MUST land in its own bucket; do NOT collapse across kinds"
        );
        assert_eq!(
            state.typed_lint_rejection_count(RejectionKind::MissingField),
            2,
            "first bucket MUST stay independent of the second"
        );
    }

    #[test]
    fn typed_lint_rejection_clear_isolated_per_kind() {
        let mut state = LoopState::new();
        state.record_typed_lint_rejection(RejectionKind::MissingField);
        state.record_typed_lint_rejection(RejectionKind::TopicOwnership);
        state.record_typed_lint_rejection(RejectionKind::ContractViolation);
        assert_eq!(
            state.typed_lint_rejection_count(RejectionKind::MissingField),
            1
        );
        state.clear_typed_lint_rejection_count(RejectionKind::MissingField);
        assert_eq!(
            state.typed_lint_rejection_count(RejectionKind::MissingField),
            0
        );
        assert_eq!(
            state.typed_lint_rejection_count(RejectionKind::TopicOwnership),
            1
        );
        assert_eq!(
            state.typed_lint_rejection_count(RejectionKind::ContractViolation),
            1,
            "clearing one bucket MUST NOT clear the others"
        );
    }

    #[test]
    fn typed_lint_rejection_reason_code_keys_match_legacy_ssot() {
        // The typed counter MUST bucket by `reason_code()` so
        // operators grepping `.ralph/recovery.jsonl` see the same
        // string. This guards against a future drift between the
        // typed kind enum and the legacy reason_code strings.
        let mut state = LoopState::new();
        state.record_typed_lint_rejection(RejectionKind::MissingField);
        state.record_typed_lint_rejection(RejectionKind::TopicOwnership);
        state.record_typed_lint_rejection(RejectionKind::ContractViolation);
        assert!(
            state
                .consecutive_lint_rejections_by_kind
                .contains_key(RejectionKind::MissingField.reason_code()),
            "typed counter MUST key on RejectionKind::reason_code() for legacy SSOT compatibility"
        );
    }

    // ── 2026-06-23 fix (mechanism review layer 3, anti-pattern 3):
    // pending handoff artifact dead-letter detector tests removed
    // (plan 2026-06-23-006 U3). ──

    // U3 (plan 2026-06-23-004): rejection stall 检测测试。
    mod stall_rejection {
        use super::*;
        use crate::event_loop::loop_state::{
            REJECTION_WINDOW_SIZE, REJECTION_WINDOW_THRESHOLD, RejectionWindowEntry,
            detect_rejection_stall,
        };

        #[test]
        fn happy_path_5_reject_rounds_triggers_stall() {
            // AE3 (反模式 3): 5 轮全 reject → emit stall.handoff_unconsumed
            let mut state = LoopState::default();
            for _ in 0..REJECTION_WINDOW_SIZE {
                state.push_rejection_window(RejectionWindowEntry {
                    rejection_count: 1,
                    emit_count: 0,
                });
            }
            assert!(detect_rejection_stall(&state));
        }

        #[test]
        fn negative_one_emit_breaks_stall() {
            let mut state = LoopState::default();
            // 4 轮 reject + 1 轮有 emit → stall 不触发
            for _ in 0..4 {
                state.push_rejection_window(RejectionWindowEntry {
                    rejection_count: 1,
                    emit_count: 0,
                });
            }
            state.push_rejection_window(RejectionWindowEntry {
                rejection_count: 0,
                emit_count: 1,
            });
            assert!(!detect_rejection_stall(&state));
        }

        #[test]
        fn threshold_boundary_3_rejects_below_window_still_triggers() {
            // 累计 reject 3 次即触发 stall,不需要等到 5 轮窗口填满。
            let mut state = LoopState::default();
            state.push_rejection_window(RejectionWindowEntry {
                rejection_count: 1,
                emit_count: 0,
            });
            state.push_rejection_window(RejectionWindowEntry {
                rejection_count: 1,
                emit_count: 0,
            });
            state.push_rejection_window(RejectionWindowEntry {
                rejection_count: 1,
                emit_count: 0,
            });
            assert_eq!(REJECTION_WINDOW_THRESHOLD, 3);
            assert!(detect_rejection_stall(&state));
        }

        #[test]
        fn window_rolls_after_size_exceeded() {
            // 旧 reject 出队后,新 emit 抵消 stall。
            let mut state = LoopState::default();
            // 填 5 轮全 reject
            for _ in 0..REJECTION_WINDOW_SIZE {
                state.push_rejection_window(RejectionWindowEntry {
                    rejection_count: 1,
                    emit_count: 0,
                });
            }
            assert!(detect_rejection_stall(&state));
            // 推入 5 轮全 emit,旧 reject 出队
            for _ in 0..REJECTION_WINDOW_SIZE {
                state.push_rejection_window(RejectionWindowEntry {
                    rejection_count: 0,
                    emit_count: 1,
                });
            }
            assert!(!detect_rejection_stall(&state));
        }

        #[test]
        fn empty_window_does_not_stall() {
            let state = LoopState::default();
            assert!(!detect_rejection_stall(&state));
        }
    }
}
