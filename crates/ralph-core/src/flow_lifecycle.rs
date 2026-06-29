//! Flow Lifecycle Registry — 2026-06-17-001 Unit 1 & Unit 3
//!
//! ## Unit 1
//!
//! Provides observable lifecycle state for parallel flow units
//! (first-version: `wave_id`). Each registered record tracks
//! phase transitions; transitions write a `flow_lifecycle`
//! `RecoveryDiagnosisEnvelope` so the existing recovery journal
//! and `ralph diagnose` reporter pick them up without any new
//! observability layer.
//!
//! The registry is intentionally topic-agnostic — it only cares
//! that a flow unit has a string id and a phase. Future plan-driven
//! parallel units (Unit 1 reserved `flow_unit_id` for that) reuse
//! the same API.
//!
//! ## Unit 3
//!
//! [`WaveDeadlines`] and [`effective_wave_deadlines`] form the
//! single deadline computation entry point used by the wave
//! dispatcher. The reconciler ([`reconcile_wave_timeouts`]) then
//! compares the configured deadlines against the actual time the
//! dispatcher waited and writes a `flow_lifecycle` envelope with
//! `reason_code: wave_timeout_drift` when the actual wait is more
//! than 10% over budget.
//!
//! Both halves share the `flow_lifecycle` recovery envelope
//! channel — Unit 1 owns the state machine, Unit 3 owns the
//! deadline math, and both feed the same reporter.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::diagnosis::{
    DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, EvidenceKind, EvidenceRef,
    RecoveryDiagnosisEnvelope, RecoveryDiagnosisEnvelopeBuilder,
};
use crate::wave_detection::DetectedWave;

/// Hard cap on `wave_total` stored in a [`FlowLifecycleRecord`]. Set to
/// 65 536 so the registry can never be the source of an OOM if a
/// dispatcher emits a wave with a `u32::MAX` total. The cap is applied
/// in [`FlowLifecycleRecord::new`].
pub const MAX_FLOW_RECORDS: u32 = 65_536;

/// Hard cap on the length of `flow_unit_id` stored in a
/// [`FlowLifecycleRecord`]. Anything longer is truncated to this many
/// Unicode scalar values.
pub const MAX_FLOW_UNIT_ID_CHARS: usize = 256;

/// Lifecycle phase of one parallel flow unit.
///
/// Transitions follow the documented state machine in the
/// 2026-06-17-001 plan:
///
/// ```text
/// Detected -> Spawning -> WorkersActive -> {Aggregating -> Closed | PartialClosed -> Degraded -> Closed | Failed -> Degraded -> Closed}
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowPhase {
    /// Wave events passed detector and queued for dispatch.
    Detected,
    /// Dispatcher is forking worker processes.
    Spawning,
    /// At least one worker is running.
    WorkersActive,
    /// All workers reported (or partial threshold reached) and the
    /// aggregator hat is being activated.
    Aggregating,
    /// Terminal: wave closed cleanly with all expected results.
    Closed,
    /// Terminal: aggregate timeout reached first; partial results
    /// may have been merged.
    PartialClosed,
    /// Terminal: spawn or runtime failure prevented any usable
    /// progress.
    Failed,
    /// Terminal: the FlowLifecycleRegistry emitted a degraded
    /// completion (e.g. `review.failed` with `skip_reason=
    /// aggregate_timeout`) on the flow's behalf.
    Degraded,
}

impl FlowPhase {
    /// Stable snake_case label used in JSON output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Detected => "detected",
            Self::Spawning => "spawning",
            Self::WorkersActive => "workers_active",
            Self::Aggregating => "aggregating",
            Self::Closed => "closed",
            Self::PartialClosed => "partial_closed",
            Self::Failed => "failed",
            Self::Degraded => "degraded",
        }
    }

    /// True if this phase is a terminal state. `PartialClosed`
    /// and `Failed` are intermediate "we did not close cleanly but
    /// the flow is finished from the dispatcher's perspective";
    /// they are still legal to follow up with `Degraded` (the
    /// registry's controlled-completion emit).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Closed | Self::Degraded)
    }
}

/// One in-flight flow record.
#[derive(Debug, Clone)]
pub struct FlowLifecycleRecord {
    /// Flow unit id. First-version: equal to `wave_id`.
    pub flow_unit_id: String,
    /// Hat that owns the worker side of the flow.
    pub target_hat: String,
    /// Topic that the wave is being dispatched on
    /// (e.g. `review.wave.ready`).
    pub source_topic: String,
    /// Total expected events / workers in this flow.
    pub wave_total: u32,
    /// Number of `*.dimension.done` (or equivalent) results
    /// received so far.
    pub received_count: u32,
    /// Indices that have not reported back yet.
    pub missing_indices: Vec<u32>,
    /// Configured aggregate timeout in seconds (0 if not set).
    pub configured_aggregate_secs: u64,
    /// Configured per-worker timeout in seconds (0 if not set).
    pub configured_worker_secs: u64,
    /// Wall-clock when the flow was first detected.
    pub started_at: Instant,
    /// Wall-clock of the most recent transition.
    pub last_transition_at: Instant,
    /// Current phase.
    pub phase: FlowPhase,
    /// Source hat for the most recent transition (used as the
    /// `source_hat` on the recovery envelope).
    pub last_source_hat: Option<String>,
    /// Optional reason code supplied by the dispatcher when
    /// the transition is non-happy.
    pub last_reason_code: Option<String>,
}

impl FlowLifecycleRecord {
    /// Build a new record in [`FlowPhase::Detected`].
    ///
    /// `wave_total` is capped to [`MAX_FLOW_RECORDS`] to keep the
    /// registry bounded. `flow_unit_id` is truncated to
    /// [`MAX_FLOW_UNIT_ID_CHARS`] characters. A warning is logged
    /// when either cap fires.
    #[must_use]
    pub fn new(
        flow_unit_id: impl Into<String>,
        target_hat: impl Into<String>,
        source_topic: impl Into<String>,
        wave_total: u32,
    ) -> Self {
        let now = Instant::now();
        let flow_unit_id = {
            let raw = flow_unit_id.into();
            let truncated: String = raw.chars().take(MAX_FLOW_UNIT_ID_CHARS).collect();
            if raw.chars().count() > MAX_FLOW_UNIT_ID_CHARS {
                warn!(
                    original_len = raw.chars().count(),
                    cap = MAX_FLOW_UNIT_ID_CHARS,
                    "flow_unit_id exceeded MAX_FLOW_UNIT_ID_CHARS; truncating"
                );
            }
            truncated
        };
        let wave_total = if wave_total > MAX_FLOW_RECORDS {
            warn!(
                original = wave_total,
                cap = MAX_FLOW_RECORDS,
                "wave_total exceeded MAX_FLOW_RECORDS; capping"
            );
            MAX_FLOW_RECORDS
        } else {
            wave_total
        };
        Self {
            flow_unit_id,
            target_hat: target_hat.into(),
            source_topic: source_topic.into(),
            wave_total,
            received_count: 0,
            missing_indices: if wave_total == 0 {
                Vec::new()
            } else {
                (0..wave_total).collect()
            },
            configured_aggregate_secs: 0,
            configured_worker_secs: 0,
            started_at: now,
            last_transition_at: now,
            phase: FlowPhase::Detected,
            last_source_hat: None,
            last_reason_code: None,
        }
    }

    /// Configure timeout values (call once after detection
    /// has resolved the actual deadlines).
    #[must_use]
    pub fn with_timeouts(mut self, worker_secs: u64, aggregate_secs: u64) -> Self {
        self.configured_worker_secs = worker_secs;
        self.configured_aggregate_secs = aggregate_secs;
        self
    }
}

/// In-memory registry of flow records.
#[derive(Debug, Default, Clone)]
pub struct FlowLifecycleRegistry {
    records: HashMap<String, FlowLifecycleRecord>,
    /// Most-recent transition envelope ready to be appended to
    /// `recovery.jsonl` by the caller. The registry does not own
    /// file I/O — the loop runner / dispatcher drains this queue
    /// after each `transition()` call so envelope writing stays on
    /// the caller's preferred path.
    pending_envelopes: Vec<RecoveryDiagnosisEnvelope>,
    /// 2026-06-29-007 plan U1a: explicit `current_step` field.
    /// Tracks the plan-mode step id (`unit_loop` / `review_walk` /
    /// `plan_end` / `ship`) directly instead of inferring it from
    /// the active record's `source_topic`. Set on
    /// [`Self::register`] from the caller-supplied step, updated
    /// by [`Self::advance_to`] (U1b), and read by
    /// [`Self::current_step_id`]. The legacy records-iteration
    /// fallback is preserved for existing fixtures/tests that
    /// relied on the inferred behaviour — see the `#[deprecated]`
    /// note on [`Self::current_step_id_fallback`].
    current_step: String,
}

impl FlowLifecycleRegistry {
    /// Build an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new flow unit in [`FlowPhase::Detected`].
    /// If a record with the same id already exists, the call is a
    /// no-op (the existing record keeps its current phase). This
    /// matches the dispatcher's idempotency story: a re-issued
    /// wave batch with the same `wave_id` must not blow away the
    /// in-flight record.
    ///
    /// When the new record has the same `flow_unit_id` as an
    /// existing one but differs in `wave_total` or `source_topic`,
    /// a `tracing::warn!` is emitted so operators can detect
    /// dispatcher bugs that re-register a wave under a different
    /// shape. The existing record is always preserved.
    ///
    /// Returns a reference to the inserted (or existing) record
    /// so the caller can chain the [`Self::transition`] calls
    /// without touching the map twice.
    pub fn register(&mut self, record: FlowLifecycleRecord) -> &FlowLifecycleRecord {
        let id = record.flow_unit_id.clone();
        let new_wave_total = record.wave_total;
        let new_source_topic = record.source_topic.clone();
        let entry = self.records.entry(id.clone()).or_insert(record);
        if entry.flow_unit_id == id
            && (entry.wave_total != new_wave_total || entry.source_topic != new_source_topic)
        {
            warn!(
                flow_unit_id = %id,
                existing_wave_total = entry.wave_total,
                new_wave_total,
                existing_source_topic = %entry.source_topic,
                new_source_topic = %new_source_topic,
                "register() called with same flow_unit_id but different shape; keeping existing record"
            );
        }
        self.records
            .get(id.as_str())
            .expect("record was just inserted")
    }

    /// Drive a state transition. Returns the new phase, or
    /// `Err(message)` if the transition is illegal.
    ///
    /// On a successful transition a `flow_lifecycle` recovery
    /// envelope is queued; callers should call
    /// [`Self::drain_pending_envelopes`] after the dispatch
    /// step completes.
    pub fn transition(
        &mut self,
        flow_unit_id: &str,
        next: FlowPhase,
        iteration: u32,
        reason_code: Option<&str>,
        source_hat: Option<&str>,
    ) -> Result<FlowPhase, String> {
        let now = Instant::now();
        let (current, target_hat, source_topic, configured_aggregate_secs) = {
            let record = self
                .records
                .get_mut(flow_unit_id)
                .ok_or_else(|| format!("flow unit '{flow_unit_id}' not registered"))?;
            let current = record.phase;
            if !is_legal_transition(current, next) {
                let legal: Vec<String> = legal_successors(current)
                    .iter()
                    .map(|p| p.as_str().to_string())
                    .collect();
                return Err(format!(
                    "illegal transition for '{flow_unit_id}': {} -> {}; legal successors: [{}]",
                    current.as_str(),
                    next.as_str(),
                    legal.join(", "),
                ));
            }
            record.phase = next;
            record.last_transition_at = now;
            if let Some(code) = reason_code {
                record.last_reason_code = Some(code.to_string());
            }
            if let Some(hat) = source_hat {
                record.last_source_hat = Some(hat.to_string());
            }
            (
                current,
                record.target_hat.clone(),
                record.source_topic.clone(),
                record.configured_aggregate_secs,
            )
        };

        let envelope = build_transition_envelope(
            flow_unit_id,
            current,
            next,
            &target_hat,
            &source_topic,
            configured_aggregate_secs,
            iteration,
            reason_code,
            source_hat,
        );
        self.pending_envelopes.push(envelope);
        Ok(next)
    }

    /// Update the `received_count` and `missing_indices` for an
    /// in-flight flow. Does **not** write a transition envelope —
    /// progress updates are bulk-appended to the existing record
    /// and only surface as envelope fields when the next
    /// transition fires.
    ///
    /// **Behavior:** does NOT enqueue transition envelopes —
    /// progress updates are bulk-appended to the existing record
    /// and surface as envelope fields only when the next
    /// transition fires.
    pub fn record_progress(
        &mut self,
        flow_unit_id: &str,
        received_count: u32,
        missing_indices: Vec<u32>,
    ) -> Result<(), String> {
        let record = self
            .records
            .get_mut(flow_unit_id)
            .ok_or_else(|| format!("flow unit '{flow_unit_id}' not registered"))?;
        record.received_count = received_count;
        record.missing_indices = missing_indices;
        Ok(())
    }

    /// Look up a record by id.
    #[must_use]
    pub fn get(&self, flow_unit_id: &str) -> Option<&FlowLifecycleRecord> {
        self.records.get(flow_unit_id)
    }

    /// All non-terminal records, in insertion order. Used by
    /// Unit 6 (`GateWaveMutex`) to decide whether a
    /// `missing_event_gate` should be suppressed.
    #[must_use]
    pub fn active_records(&self) -> impl Iterator<Item = &FlowLifecycleRecord> {
        self.records.values().filter(|r| !r.phase.is_terminal())
    }

    /// Whether the registry currently tracks a non-terminal record
    /// for `flow_unit_id`.
    #[must_use]
    pub fn is_obligation_pending(&self, flow_unit_id: &str) -> bool {
        self.records
            .get(flow_unit_id)
            .is_some_and(|r| !r.phase.is_terminal())
    }

    /// True when at least one in-flight (non-terminal) record is
    /// associated with `target_hat`. Used by [`crate::event_loop`]
    /// `should_gate_missing_events` (Unit 6, GateWaveMutex) to
    /// suppress the gate while a hat is legitimately waiting on
    /// wave workers to report back.
    ///
    /// The optional `trigger_topics` filter is a back-stop: when
    /// provided, only records whose `source_topic` matches one
    /// of the supplied trigger topics count. The original Unit 6
    /// spec expected the source topic to align with the
    /// obligation's `on_trigger` (e.g. `review.wave.ready`); in
    /// practice the dispatcher registers the wave under the
    /// `target_hat` *hat config* topic, which may differ. Callers
    /// should pass an empty slice when they want the pure
    /// "any-active-wave-for-this-hat" semantics.
    #[must_use]
    pub fn is_obligation_pending_for_hat(&self, target_hat: &str, trigger_topics: &[&str]) -> bool {
        self.records.values().any(|r| {
            if r.phase.is_terminal() || r.target_hat != target_hat {
                return false;
            }
            if trigger_topics.is_empty() {
                true
            } else {
                trigger_topics.iter().any(|t| *t == r.source_topic)
            }
        })
    }

    /// Drain queued transition envelopes. Caller writes them to
    /// `recovery.jsonl`.
    pub fn drain_pending_envelopes(&mut self) -> Vec<RecoveryDiagnosisEnvelope> {
        std::mem::take(&mut self.pending_envelopes)
    }

    /// Number of tracked records (terminal or not).
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// True when the registry holds no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Remove terminal records older than the given age. The
    /// dispatcher calls this at the end of every `execute_wave`
    /// to keep the registry bounded.
    pub fn prune_terminal_older_than(&mut self, max_age: Duration) {
        let now = Instant::now();
        self.records.retain(|_, r| {
            if r.phase.is_terminal() {
                now.duration_since(r.last_transition_at) <= max_age
            } else {
                true
            }
        });
    }

    /// 2026-06-29-007 plan U1a: explicit accessor for the
    /// plan-mode `current_step`. Returns the value of the
    /// dedicated [`Self::current_step`] field. Defaults to
    /// `"unit_loop"` when the field is empty (legacy callers
    /// that never called `register`/`advance_to`).
    pub fn current_step_id(&self) -> &str {
        if !self.current_step.is_empty() {
            return self.current_step.as_str();
        }
        self.current_step_id_fallback()
    }

    /// 2026-06-29-007 plan U1a: deprecated records-iteration
    /// fallback. Preserved so the existing fixtures (003-005
    /// plans landed pre-U1a) keep passing without a mass
    /// rewrite. U1b will add explicit `advance_to` calls in
    /// the event loop; U1a only swaps the accessor to prefer
    /// the dedicated field when set.
    #[deprecated(
        note = "Pre-U1a records-iteration fallback. New code should set current_step via register/advance_to and call current_step_id() directly."
    )]
    pub fn current_step_id_fallback(&self) -> &str {
        for record in self.records.values() {
            if !record.phase.is_terminal() {
                return record.source_topic.as_str();
            }
        }
        "unit_loop"
    }

    /// 2026-06-29-007 plan U1a: explicit setter for the
    /// `current_step` field. Validates that `step` is a
    /// non-empty string; empty strings are rejected with a
    /// typed error so the field can never drift to the
    /// empty state via this path (the empty default is only
    /// reachable through [`Self::default`] / unset state).
    pub fn set_current_step(&mut self, step: &str) -> Result<(), FlowError> {
        if step.is_empty() {
            return Err(FlowError::UnknownStep {
                step: step.to_string(),
            });
        }
        self.current_step = step.to_string();
        Ok(())
    }

    /// 2026-06-29-007 plan U1b: advance the `current_step`
    /// field to `target`. Refuses to advance if the target
    /// equals the current value (idempotent no-op). The
    /// U1b-emitted `flow.transition` event handling is the
    /// caller's responsibility; this helper is a pure
    /// mutator. U1a's set_current_step stays for tests.
    pub fn advance_to(&mut self, target: &str) -> Result<&str, FlowError> {
        if target.is_empty() {
            return Err(FlowError::PrematureTransition {
                from: self.current_step.clone(),
                to: target.to_string(),
            });
        }
        if self.current_step == target {
            return Ok(self.current_step.as_str());
        }
        let prev = std::mem::replace(&mut self.current_step, target.to_string());
        tracing::debug!(
            from = %prev,
            to = %self.current_step,
            "FlowLifecycleRegistry::advance_to"
        );
        Ok(self.current_step.as_str())
    }
}

/// Typed errors for flow step manipulation. U1a adds the
/// two variants used by `set_current_step` /
/// `advance_to`; future Units may add more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowError {
    /// The supplied step id is not declared in the
    /// `mechanism.flow` declaration. Currently we only
    /// guard against empty strings; full StepId enum
    /// validation lands with the follow-up type-state
    /// refactor (see plan §Deferred to Follow-Up Work).
    UnknownStep { step: String },
    /// Caller attempted to advance before reaching the
    /// `unit_loop.all_done` trigger condition. Returned
    /// by [`FlowLifecycleRegistry::advance_to`] when the
    /// target is empty (the only premature condition we
    /// detect pre-U1b; full validation lives in the
    /// event-loop caller).
    PrematureTransition { from: String, to: String },
}

impl std::fmt::Display for FlowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownStep { step } => write!(f, "unknown step id: {step:?}"),
            Self::PrematureTransition { from, to } => {
                write!(f, "premature transition from {from:?} to {to:?}")
            }
        }
    }
}

impl std::error::Error for FlowError {}

/// Returns the legal successor phases for `current`.
///
/// Used to surface a remediation hint in [`FlowLifecycleRegistry::transition`]
/// error messages. Terminal phases return an empty slice.
#[must_use]
pub fn legal_successors(phase: FlowPhase) -> &'static [FlowPhase] {
    use FlowPhase::{
        Aggregating, Closed, Degraded, Detected, Failed, PartialClosed, Spawning, WorkersActive,
    };
    match phase {
        Detected => &[Spawning, Failed],
        Spawning => &[WorkersActive, Failed],
        WorkersActive => &[Aggregating, PartialClosed, Failed],
        Aggregating => &[Closed, Degraded],
        PartialClosed => &[Degraded],
        Failed => &[Degraded],
        Closed | Degraded => &[],
    }
}

/// Returns true if `current -> next` is a legal transition.
///
/// Legal:
/// - `Detected -> Spawning | Failed`
/// - `Spawning -> WorkersActive | Failed`
/// - `WorkersActive -> Aggregating | PartialClosed | Failed`
/// - `Aggregating -> Closed | Degraded`
/// - `PartialClosed -> Degraded` (the flow is partial-closed and
///   the registry needs to issue a degraded completion)
/// - `Failed -> Degraded` (spawn failures escalate to degraded
///   completion)
/// - All terminal phases are absorbing (no further transitions).
fn is_legal_transition(current: FlowPhase, next: FlowPhase) -> bool {
    use FlowPhase::{
        Aggregating, Closed, Degraded, Detected, Failed, PartialClosed, Spawning, WorkersActive,
    };
    if current.is_terminal() {
        return false;
    }
    match (current, next) {
        (Detected, Spawning | Failed) => true,
        (Spawning, WorkersActive | Failed) => true,
        (WorkersActive, Aggregating | PartialClosed | Failed) => true,
        (Aggregating, Closed | Degraded) => true,
        (PartialClosed, Degraded) => true,
        (Failed, Degraded) => true,
        _ => false,
    }
}

fn build_transition_envelope(
    flow_unit_id: &str,
    from: FlowPhase,
    to: FlowPhase,
    target_hat: &str,
    source_topic: &str,
    configured_aggregate_secs: u64,
    iteration: u32,
    reason_code: Option<&str>,
    source_hat: Option<&str>,
) -> RecoveryDiagnosisEnvelope {
    let severity = match to {
        FlowPhase::Failed | FlowPhase::Degraded => DiagnosisSeverity::Error,
        FlowPhase::PartialClosed => DiagnosisSeverity::Warning,
        _ => DiagnosisSeverity::Info,
    };
    let mut builder = RecoveryDiagnosisEnvelopeBuilder::new(DiagnosisSource::FlowLifecycle, severity)
        .iteration(iteration)
        .topic(source_topic.to_string())
        .target_hat(target_hat.to_string())
        .reason_code(reason_code.unwrap_or("phase_transition"))
        .message(format!(
            "flow '{}' phase: {} -> {}",
            flow_unit_id,
            from.as_str(),
            to.as_str()
        ))
        .retry_key(format!("flow_lifecycle:{flow_unit_id}:{}", to.as_str()))
        .outcome(DiagnosisOutcome::Pending)
        .safe_target(false)
        .expected_action(format!("Flow unit '{flow_unit_id}' is now in phase '{}'.", to.as_str()))
        .evidence(EvidenceRef {
            kind: EvidenceKind::Field,
            ref_path: format!("flow.phase.{}", to.as_str()),
            snippet: Some(format!(
                "{{\"from\":\"{}\",\"to\":\"{}\",\"configured_aggregate_secs\":{configured_aggregate_secs}}}",
                from.as_str(),
                to.as_str()
            )),
        });
    if let Some(hat) = source_hat {
        builder = builder.source_hat(hat.to_string());
    }
    builder.build()
}

/// Reasons a wave's `actual_wait_ms` may diverge from its
/// `configured_aggregate_secs`. The reconciler writes a
/// `flow_lifecycle` envelope with `reason_code` set to one of
/// these stable strings.
pub mod timeout_reasons {
    /// `actual_wait_ms` exceeded `configured_aggregate_ms * 1.1`
    /// (10 % tolerance). Operator-actionable but not yet a
    /// terminal escalation.
    pub const WAVE_TIMEOUT_DRIFT: &str = "wave_timeout_drift";
    /// The dispatcher waited less than 50 % of the configured
    /// aggregate budget before declaring `AggregateDeadlineExceeded`
    /// (defensive: the deadline should never fire early).
    pub const WAVE_TIMEOUT_EARLY: &str = "wave_timeout_early";
}

/// Tolerance applied to `actual_wait_ms` when comparing to
/// `configured_aggregate_ms`. Unit 3 keeps this conservative so
/// genuine clock drift does not generate spurious `flow_lifecycle`
/// envelopes; operators only see envelopes when the divergence is
/// large enough to matter.
pub const WAVE_TIMEOUT_DRIFT_TOLERANCE: f64 = 1.10;

/// Unified deadline structure returned by [`effective_wave_deadlines`].
///
/// The dispatcher is required to consume both fields and pass them
/// to the worker spawn loop / aggregate wait. Reading either field
/// from `DetectedWave` directly is now considered a layering
/// violation; all callers go through this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaveDeadlines {
    /// Per-worker timeout in seconds.
    pub per_worker: u64,
    /// Wave-level aggregate timeout in seconds.
    pub aggregate: u64,
}

impl WaveDeadlines {
    /// Build deadlines from raw values. Used by tests and the
    /// dispatcher override path.
    #[must_use]
    pub const fn new(per_worker: u64, aggregate: u64) -> Self {
        Self {
            per_worker,
            aggregate,
        }
    }

    /// Per-worker deadline as [`Duration`].
    #[must_use]
    pub fn per_worker_duration(&self) -> Duration {
        Duration::from_secs(self.per_worker)
    }

    /// Aggregate deadline as [`Duration`].
    #[must_use]
    pub fn aggregate_duration(&self) -> Duration {
        Duration::from_secs(self.aggregate)
    }
}

/// Single entry point for wave deadline computation. Wraps
/// [`DetectedWave::per_worker_timeout_secs`] and
/// [`DetectedWave::aggregate_timeout_secs`] so the dispatcher
/// never has to remember the priority chain.
///
/// The returned deadlines are the same numbers the existing
/// dispatcher would compute inline — this function is purely a
/// shim that gives Unit 3 a stable surface to read against and
/// gives Unit 4+ a place to add caching / observability later
/// without touching every caller.
#[must_use]
pub fn effective_wave_deadlines(detected: &DetectedWave) -> WaveDeadlines {
    WaveDeadlines::new(
        detected.per_worker_timeout_secs(),
        detected.aggregate_timeout_secs(),
    )
}

/// Reconciler outcome. The dispatcher appends the envelope to
/// `recovery.jsonl` when `drift_envelope.is_some()`. The
/// `escalated` field tells the responder whether the divergence
/// crossed the escalation threshold (currently identical to
/// "drift_envelope is Some" — split out for forward
/// compatibility with Unit 8 stall escalation).
#[derive(Debug, Clone)]
pub struct TimeoutReconciliation {
    /// Stable identifier for this wave (`wave_id`).
    pub flow_unit_id: String,
    /// Configured aggregate timeout in milliseconds.
    pub configured_aggregate_ms: u64,
    /// Actual time the dispatcher waited in milliseconds.
    pub actual_wait_ms: u64,
    /// `actual_wait_ms - configured_aggregate_ms`. Signed to
    /// expose both over- and under-shoots.
    pub delta_ms: i64,
    /// Recovery envelope to append to `recovery.jsonl` if a
    /// divergence was detected. `None` when the actual wait
    /// was within tolerance.
    pub drift_envelope: Option<RecoveryDiagnosisEnvelope>,
    /// `true` when the divergence crossed the escalation
    /// threshold. Dispatcher does not terminate the loop on
    /// `escalated = true` — Unit 8 owns the stall escalation
    /// policy.
    pub escalated: bool,
}

/// Reconcile configured vs actual wait time for one wave.
///
/// Called by the wave dispatcher after a `Completed`,
/// `Partial`, or `AggregateDeadlineExceeded` outcome. The
/// `actual_wait` should be measured from `Detected -> terminal`
/// by the dispatcher (`Instant::now() - dispatch_start`).
///
/// Tolerance rule (per Unit 3 spec):
/// - `actual_wait_ms > configured_aggregate_ms * 1.10` →
///   `drift_envelope` Some, `reason_code: wave_timeout_drift`,
///   `severity: Warning`, `outcome: Escalated`.
/// - `actual_wait_ms < configured_aggregate_ms / 2` (with
///   `configured_aggregate_ms > 0`) →
///   `drift_envelope` Some, `reason_code: wave_timeout_early`,
///   `severity: Warning`. The dispatcher should not be firing
///   `AggregateDeadlineExceeded` early — that was a known
///   archive bug ("1464s no degrade").
/// - Otherwise: no envelope.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn reconcile_wave_timeouts(
    flow_unit_id: &str,
    source_topic: &str,
    target_hat: &str,
    configured: &WaveDeadlines,
    actual_wait: Duration,
    iteration: u32,
) -> TimeoutReconciliation {
    let configured_aggregate_ms = configured.aggregate.saturating_mul(1000);
    let actual_wait_ms = actual_wait.as_millis() as u64;
    let delta_ms = actual_wait_ms as i64 - configured_aggregate_ms as i64;
    let mut drift_envelope: Option<RecoveryDiagnosisEnvelope> = None;
    let mut escalated = false;

    if configured.aggregate > 0 {
        // Integer arithmetic to avoid `u32::try_from` truncation for
        // large aggregate values. Compute (aggregate * 110 / 100) * 1000
        // using u128 to hold the intermediate without overflow.
        let threshold_ms = ((configured.aggregate as u128)
            .saturating_mul(110)
            .saturating_div(100)
            .saturating_mul(1000)
            .min(u64::MAX as u128)) as u64;
        if actual_wait_ms > threshold_ms {
            drift_envelope = Some(build_drift_envelope(
                flow_unit_id,
                source_topic,
                target_hat,
                configured_aggregate_ms,
                actual_wait_ms,
                delta_ms,
                iteration,
                timeout_reasons::WAVE_TIMEOUT_DRIFT,
            ));
            escalated = true;
        } else if actual_wait_ms < configured_aggregate_ms / 2 {
            drift_envelope = Some(build_drift_envelope(
                flow_unit_id,
                source_topic,
                target_hat,
                configured_aggregate_ms,
                actual_wait_ms,
                delta_ms,
                iteration,
                timeout_reasons::WAVE_TIMEOUT_EARLY,
            ));
        }
    }

    TimeoutReconciliation {
        flow_unit_id: flow_unit_id.to_string(),
        configured_aggregate_ms,
        actual_wait_ms,
        delta_ms,
        drift_envelope,
        escalated,
    }
}

fn build_drift_envelope(
    flow_unit_id: &str,
    source_topic: &str,
    target_hat: &str,
    configured_ms: u64,
    actual_ms: u64,
    delta_ms: i64,
    iteration: u32,
    reason_code: &str,
) -> RecoveryDiagnosisEnvelope {
    RecoveryDiagnosisEnvelopeBuilder::new(DiagnosisSource::FlowLifecycle, DiagnosisSeverity::Warning)
        .iteration(iteration)
        .topic(source_topic.to_string())
        .target_hat(target_hat.to_string())
        .reason_code(reason_code)
        .message(format!(
            "wave '{flow_unit_id}' actual wait {actual_ms}ms vs configured {configured_ms}ms \
             (delta {delta_ms}ms)"
        ))
        .retry_key(format!("flow_lifecycle:{flow_unit_id}:timeout_drift"))
        .outcome(DiagnosisOutcome::Pending)
        .safe_target(false)
        .expected_action(match reason_code {
            timeout_reasons::WAVE_TIMEOUT_EARLY => {
                "Aggregate deadline fired before the configured budget. Inspect the dispatcher deadline path; this is a defensive alarm, not a worker fault."
                    .to_string()
            }
            timeout_reasons::WAVE_TIMEOUT_DRIFT => {
                "Wave actual wait exceeded configured budget. Increase `aggregate.timeout` or reduce per-worker count for the next wave."
                    .to_string()
            }
            _ => "Inspect dispatcher deadline path: actual wait diverged from configured budget."
                .to_string(),
        })
        .evidence(EvidenceRef {
            kind: EvidenceKind::Field,
            ref_path: "flow.timeout".to_string(),
            snippet: Some(format!(
                "{{\"configured_aggregate_ms\":{configured_ms},\"actual_wait_ms\":{actual_ms},\
                 \"delta_ms\":{delta_ms},\"tolerance\":\"1.10\"}}"
            )),
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> FlowLifecycleRecord {
        FlowLifecycleRecord::new("wf-1", "review-coordinator", "review.wave.ready", 7)
            .with_timeouts(60, 1800)
    }

    #[test]
    fn happy_path_transitions() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.register(rec());
        let seq = [
            FlowPhase::Spawning,
            FlowPhase::WorkersActive,
            FlowPhase::Aggregating,
            FlowPhase::Closed,
        ];
        let mut current = FlowPhase::Detected;
        for next in seq {
            let got = reg
                .transition("wf-1", next, 1, None, Some("review-coordinator"))
                .unwrap();
            current = got;
        }
        assert_eq!(current, FlowPhase::Closed);
        assert!(reg.is_obligation_pending("wf-1") == false);
    }

    #[test]
    fn partial_path_transitions() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.register(rec());
        reg.transition("wf-1", FlowPhase::Spawning, 1, None, None)
            .unwrap();
        reg.transition("wf-1", FlowPhase::WorkersActive, 1, None, None)
            .unwrap();
        reg.transition("wf-1", FlowPhase::PartialClosed, 1, None, None)
            .unwrap();
        // PartialClosed -> Degraded is the only legal post-terminal
        // transition, and Degraded itself is terminal so this
        // returns Ok(Degraded) but no further transitions are
        // allowed.
        reg.transition(
            "wf-1",
            FlowPhase::Degraded,
            1,
            Some("aggregate_timeout"),
            None,
        )
        .unwrap();
        let err = reg.transition("wf-1", FlowPhase::Closed, 1, None, None);
        assert!(err.is_err(), "Closed from Degraded must be rejected");
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.register(rec());
        // Detected -> Aggregating is not legal.
        let err = reg.transition("wf-1", FlowPhase::Aggregating, 1, None, None);
        assert!(err.is_err());
    }

    #[test]
    fn transition_error_message_includes_legal_successors() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.register(rec());
        // Detected -> Aggregating is illegal. Legal successors from
        // Detected are [Spawning, Failed].
        let err = reg
            .transition("wf-1", FlowPhase::Aggregating, 1, None, None)
            .expect_err("must be rejected");
        assert!(
            err.contains("legal successors:"),
            "error must include remediation hint: {err}"
        );
        assert!(
            err.contains("spawning") && err.contains("failed"),
            "error must list legal successors: {err}"
        );
    }

    #[test]
    fn unknown_flow_unit_rejected() {
        let mut reg = FlowLifecycleRegistry::new();
        let err = reg.transition("nope", FlowPhase::Spawning, 1, None, None);
        assert!(err.is_err());
    }

    #[test]
    fn progress_updates_do_not_emit_envelopes() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.register(rec());
        reg.transition("wf-1", FlowPhase::Spawning, 1, None, None)
            .unwrap();
        // drain the spawn transition envelope
        let _spawn_env = reg.drain_pending_envelopes();
        // record_progress must NOT queue any envelope
        reg.record_progress("wf-1", 5, vec![5, 6]).unwrap();
        let pending_after = reg.drain_pending_envelopes().len();
        assert_eq!(
            pending_after, 0,
            "record_progress must not enqueue envelopes"
        );
        let record = reg.get("wf-1").unwrap();
        assert_eq!(record.received_count, 5);
        assert_eq!(record.missing_indices, vec![5, 6]);
    }

    #[test]
    fn obligation_pending_only_for_active() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.register(rec());
        assert!(reg.is_obligation_pending("wf-1"));
        reg.transition("wf-1", FlowPhase::Spawning, 1, None, None)
            .unwrap();
        assert!(reg.is_obligation_pending("wf-1"));
        reg.transition("wf-1", FlowPhase::WorkersActive, 1, None, None)
            .unwrap();
        // Failed is intermediate (not terminal): the registry may
        // still need to escalate to `Degraded`. The gate-suppression
        // path treats "intermediate" as "active", so the obligation
        // is still pending.
        reg.transition("wf-1", FlowPhase::Failed, 1, Some("spawn_error"), None)
            .unwrap();
        assert!(reg.is_obligation_pending("wf-1"));
        reg.transition("wf-1", FlowPhase::Degraded, 1, None, None)
            .unwrap();
        assert!(!reg.is_obligation_pending("wf-1"));
    }

    #[test]
    fn prune_terminal_keeps_active() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.register(rec());
        reg.transition("wf-1", FlowPhase::Spawning, 1, None, None)
            .unwrap();
        reg.transition("wf-1", FlowPhase::Failed, 1, Some("x"), None)
            .unwrap();
        let r = reg.get("wf-1").unwrap();
        let now = r.last_transition_at;
        // Pretend 10s passed.
        let too_old = Duration::from_secs(10);
        // Manually rewind `last_transition_at` so the prune sees an
        // old record.
        // SAFETY: We do not expose direct mutation, so this test
        // would normally rely on a real clock. To keep the test
        // self-contained we just verify the API no-ops for fresh
        // records and accepts `max_age = 0` for instant pruning
        // (separate test).
        let _ = now;
        reg.prune_terminal_older_than(too_old);
        assert!(reg.get("wf-1").is_some());
    }

    #[test]
    fn phase_serde_uses_snake_case() {
        let p = FlowPhase::PartialClosed;
        let s = serde_json::to_string(&p).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v.as_str().unwrap(), "partial_closed");
    }

    #[test]
    fn flow_lifecycle_record_caps_wave_total_and_id_length() {
        // Cap wave_total
        let big_wave = FlowLifecycleRecord::new("wf-cap", "h", "t", 100_000_000);
        assert_eq!(big_wave.wave_total, MAX_FLOW_RECORDS);
        assert_eq!(big_wave.missing_indices.len(), MAX_FLOW_RECORDS as usize);

        // Cap flow_unit_id (10_000 char string -> truncated to 256)
        let long_id: String = "x".repeat(10_000);
        let rec = FlowLifecycleRecord::new(long_id.as_str(), "h", "t", 3);
        assert_eq!(rec.flow_unit_id.chars().count(), MAX_FLOW_UNIT_ID_CHARS);
    }

    #[test]
    fn transition_to_same_phase_is_rejected() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.register(rec());
        reg.transition("wf-1", FlowPhase::Spawning, 1, None, None)
            .unwrap();
        reg.transition("wf-1", FlowPhase::WorkersActive, 1, None, None)
            .unwrap();
        // Now try transitioning WorkersActive -> WorkersActive.
        let err = reg.transition("wf-1", FlowPhase::WorkersActive, 2, None, None);
        assert!(err.is_err(), "same-phase transition must be rejected");
    }

    #[test]
    fn is_obligation_pending_for_hat_topic_filter_branches() {
        let mut reg = FlowLifecycleRegistry::new();
        // Two records on the same hat with different source_topics.
        let rec_a = FlowLifecycleRecord::new("wf-a", "review-coordinator", "review.wave.ready", 3);
        let rec_b = FlowLifecycleRecord::new("wf-b", "review-coordinator", "other.topic", 3);
        reg.register(rec_a);
        reg.register(rec_b);

        // Empty filter -> "any active wave for this hat" -> true.
        assert!(reg.is_obligation_pending_for_hat("review-coordinator", &[]));

        // Filter that matches rec_a.source_topic -> true.
        assert!(reg.is_obligation_pending_for_hat("review-coordinator", &[&"review.wave.ready"],));

        // Filter for an unrelated topic -> false.
        assert!(!reg.is_obligation_pending_for_hat("review-coordinator", &[&"unrelated.topic"],));
    }

    #[test]
    fn register_warns_on_duplicate_with_different_fields() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.register(FlowLifecycleRecord::new("wf-dup", "h1", "t1", 5));
        // Same id, different wave_total. The original record must
        // be kept; the warn is emitted but we do not assert on it
        // (no tracing subscriber installed).
        reg.register(FlowLifecycleRecord::new("wf-dup", "h1", "t1", 99));
        let r = reg.get("wf-dup").unwrap();
        assert_eq!(r.wave_total, 5, "existing record must be preserved");
        assert_eq!(r.phase, FlowPhase::Detected);
    }

    // -----------------------------------------------------------------
    // U1a (2026-06-29-007 plan): explicit `current_step` field
    // -----------------------------------------------------------------

    #[test]
    fn current_step_defaults_to_unit_loop() {
        let reg = FlowLifecycleRegistry::new();
        assert_eq!(reg.current_step_id(), "unit_loop");
    }

    #[test]
    fn current_step_setter_updates_field() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.set_current_step("review_walk").unwrap();
        assert_eq!(reg.current_step_id(), "review_walk");
        reg.set_current_step("plan_end").unwrap();
        assert_eq!(reg.current_step_id(), "plan_end");
        reg.set_current_step("ship").unwrap();
        assert_eq!(reg.current_step_id(), "ship");
    }

    #[test]
    fn current_step_setter_rejects_empty() {
        let mut reg = FlowLifecycleRegistry::new();
        let err = reg.set_current_step("").unwrap_err();
        assert!(matches!(err, FlowError::UnknownStep { .. }));
    }

    #[test]
    fn current_step_advance_to_updates_field() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.set_current_step("unit_loop").unwrap();
        let step = reg.advance_to("review_walk").unwrap();
        assert_eq!(step, "review_walk");
        assert_eq!(reg.current_step_id(), "review_walk");
        let step = reg.advance_to("plan_end").unwrap();
        assert_eq!(step, "plan_end");
    }

    #[test]
    fn current_step_advance_to_is_idempotent() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.set_current_step("unit_loop").unwrap();
        reg.advance_to("review_walk").unwrap();
        let step = reg.advance_to("review_walk").unwrap();
        assert_eq!(step, "review_walk");
        assert_eq!(reg.current_step_id(), "review_walk");
    }

    #[test]
    fn current_step_advance_to_rejects_empty_target() {
        let mut reg = FlowLifecycleRegistry::new();
        reg.set_current_step("unit_loop").unwrap();
        let err = reg.advance_to("").unwrap_err();
        assert!(matches!(err, FlowError::PrematureTransition { .. }));
    }

    #[test]
    fn current_step_field_takes_precedence_over_records_fallback() {
        // 2026-06-29-007 plan U1a: even when records are
        // registered (which would have triggered the legacy
        // records-iteration fallback), the dedicated field
        // wins. This is the contract change U1a introduces:
        // setter/advance_to always beat the inferred value.
        let mut reg = FlowLifecycleRegistry::new();
        reg.register(FlowLifecycleRecord::new(
            "wf-1",
            "review-coordinator",
            "review.wave.ready",
            3,
        ));
        // Field is still empty → records fallback fires.
        assert_eq!(reg.current_step_id(), "review.wave.ready");
        // Setter takes precedence.
        reg.set_current_step("review_walk").unwrap();
        assert_eq!(reg.current_step_id(), "review_walk");
    }

    // -----------------------------------------------------------------
    // TimeoutReconciler (Unit 3)
    // -----------------------------------------------------------------

    #[test]
    fn timeout_reconciler_within_tolerance_writes_no_envelope() {
        let deadlines = WaveDeadlines::new(60, 1800);
        let actual = Duration::from_millis(1_750_000); // 1750s, within 10% of 1800s
        let r = reconcile_wave_timeouts(
            "wf-1",
            "review.wave.ready",
            "review-coordinator",
            &deadlines,
            actual,
            7,
        );
        assert_eq!(r.configured_aggregate_ms, 1_800_000);
        assert_eq!(r.actual_wait_ms, 1_750_000);
        assert_eq!(r.delta_ms, -50_000);
        assert!(r.drift_envelope.is_none());
        assert!(!r.escalated);
    }

    #[test]
    fn timeout_reconciler_over_budget_writes_drift_envelope() {
        let deadlines = WaveDeadlines::new(60, 1800);
        let actual = Duration::from_millis(1_990_000); // 1990s > 1800*1.10=1980s
        let r = reconcile_wave_timeouts(
            "wf-1",
            "review.wave.ready",
            "review-coordinator",
            &deadlines,
            actual,
            7,
        );
        assert!(r.drift_envelope.is_some());
        let env = r.drift_envelope.unwrap();
        assert_eq!(env.reason_code, timeout_reasons::WAVE_TIMEOUT_DRIFT);
        assert!(r.escalated);
    }

    #[test]
    fn timeout_reconciler_early_firing_writes_envelope() {
        let deadlines = WaveDeadlines::new(60, 1800);
        let actual = Duration::from_millis(100_000); // 100s, well under 900s half-budget
        let r = reconcile_wave_timeouts(
            "wf-1",
            "review.wave.ready",
            "review-coordinator",
            &deadlines,
            actual,
            7,
        );
        assert!(r.drift_envelope.is_some());
        let env = r.drift_envelope.unwrap();
        assert_eq!(env.reason_code, timeout_reasons::WAVE_TIMEOUT_EARLY);
        assert!(!r.escalated);
    }

    #[test]
    fn timeout_reconciler_zero_configured_aggregate_skips_reconciliation() {
        // Defensive: when the consumer hat did not configure an
        // aggregate timeout the dispatcher should still be able to
        // call us without panicking.
        let deadlines = WaveDeadlines::new(60, 0);
        let actual = Duration::from_millis(99_999_999);
        let r = reconcile_wave_timeouts(
            "wf-1",
            "review.wave.ready",
            "review-coordinator",
            &deadlines,
            actual,
            7,
        );
        assert_eq!(r.configured_aggregate_ms, 0);
        assert!(r.drift_envelope.is_none());
    }

    #[test]
    fn reconcile_wave_timeouts_handles_large_aggregate_without_overflow() {
        // u32::MAX as the configured aggregate. The old f64 path
        // truncated this to u32::MAX anyway, so the new integer
        // path must agree: configured_aggregate_ms == u32::MAX * 1000.
        let deadlines = WaveDeadlines::new(60, u32::MAX as u64);
        let actual = Duration::from_millis(0);
        let r = reconcile_wave_timeouts(
            "wf-1",
            "review.wave.ready",
            "review-coordinator",
            &deadlines,
            actual,
            1,
        );
        assert_eq!(r.configured_aggregate_ms, u32::MAX as u64 * 1000);
        // With actual_wait=0 and configured huge, the early-firing
        // branch (actual < configured/2) should fire.
        assert!(r.drift_envelope.is_some());
    }

    #[test]
    fn effective_wave_deadlines_matches_existing_priority_chain() {
        // The shim is supposed to be a no-op layering-wise; spot
        // check the priority chain behaves identically to the
        // helper on `DetectedWave`.
        use crate::config::{AggregateConfig, HatConfig};
        let wave = DetectedWave {
            wave_id: "w-1".into(),
            target_hat: "dimension-reviewer".into(),
            events: vec![],
            total: 7,
            hat_config: HatConfig {
                timeout: Some(45),
                aggregate: Some(AggregateConfig {
                    mode: crate::config::AggregateMode::WaitForAll,
                    timeout: 120,
                }),
                ..HatConfig::default()
            },
            consumer_aggregate_timeout: Some(300),
            partial: false,
        };
        let deadlines = effective_wave_deadlines(&wave);
        // explicit aggregate wins over consumer
        assert_eq!(deadlines.aggregate, 120);
        assert_eq!(deadlines.per_worker, 45);
    }
}

// U2 (2026-06-17-003 plan): incomplete wave gate — when a review
// wave stalls below its `wave_total` for longer than 80% of
// `aggregate_timeout_secs` past the **last dimension progress**,
// the mechanism emits a `plan.blocked` event on behalf of
// `review-synthesizer` (which has been observed to never fire
// when no `dimension.done` arrives). The target is `shipper` per
// the plan's routing decision (`plan-gate.triggers` does NOT
// include `plan.blocked`).
//
// This module is intentionally a submodule of `flow_lifecycle`
// (per plan §Files — "扩展现有 `flow_lifecycle.rs` 模块，非新目录")
// so the existing observability channel stays coherent. The
// helper [`IncompleteWaveGate::evaluate`] returns the
// `plan.blocked` payload as JSON; the caller is responsible
// for publishing it through `Event::with_target("shipper")` and
// closing the tracker wave so the gate does not re-fire.
pub mod incomplete_wave_gate {
    use super::{FlowLifecycleRegistry, FlowPhase};
    use serde::Serialize;

    /// Configuration knobs for the U2 incomplete-wave gate.
    ///
    /// `enabled` defaults to `false` globally and `true` for
    /// `ce-executor-serial` (per plan §U2). The caller
    /// (`EventLoop::maybe_emit_incomplete_wave_blocked`) reads
    /// `workflow_contract.incomplete_wave_gate.enabled` from
    /// `RalphConfig` and falls back to the global default.
    #[derive(Debug, Clone)]
    pub struct IncompleteWaveGateConfig {
        pub enabled: bool,
        /// Multiplier on `aggregate_timeout_secs` that defines
        /// the staleness window. Plan §U2: 0.8 (80%).
        pub staleness_ratio: f64,
    }

    impl Default for IncompleteWaveGateConfig {
        fn default() -> Self {
            Self {
                enabled: false,
                staleness_ratio: 0.8,
            }
        }
    }

    /// Payload published as `plan.blocked` when the gate fires.
    ///
    /// Field names match the plan §U2 "Payload" spec. We
    /// intentionally **exclude** the parsed-but-unused
    /// `missing_dimensions` set from the JSON when it's empty
    /// (the tracker cannot know per-dimension labels for waves
    /// where `dimension.done` never arrives), so the audit
    /// payload does not carry a misleading empty array.
    #[derive(Debug, Clone, Serialize)]
    pub struct PlanBlockedPayload {
        pub reason: &'static str,
        pub wave_id: String,
        pub plan_name: String,
        pub task_id: String,
        pub step: String,
        pub expected: u32,
        pub received: u32,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        pub missing_dimensions: Vec<String>,
        pub staleness_secs: u64,
        pub aggregate_timeout_secs: u64,
    }

    impl PlanBlockedPayload {
        /// Stable reason string used in the audit trail. The
        /// plan pins this as `dimension_reviewers_failed_to_converge`
        /// so operators can grep on it across runs.
        pub const REASON: &'static str = "dimension_reviewers_failed_to_converge";
    }

    /// U2 gate evaluator. Pure function over the inputs —
    /// construction is cheap; `evaluate` does no I/O.
    pub struct IncompleteWaveGate {
        pub config: IncompleteWaveGateConfig,
    }

    impl IncompleteWaveGate {
        pub const fn new(config: IncompleteWaveGateConfig) -> Self {
            Self { config }
        }

        /// Compute the absolute staleness threshold from the
        /// configured ratio and the supplied aggregate timeout.
        /// Uses saturating arithmetic to avoid panics on
        /// pathological inputs (ratio = 0, aggregate = 0).
        pub fn staleness_secs(&self, aggregate_timeout_secs: u64) -> u64 {
            if aggregate_timeout_secs == 0 {
                return 0;
            }
            let ratio_milli = (self.config.staleness_ratio * 1000.0).round() as u64;
            (aggregate_timeout_secs.saturating_mul(ratio_milli)) / 1000
        }

        /// Decide whether to emit `plan.blocked` for an
        /// incomplete wave. The gate fires **only** when all of:
        ///
        /// 1. `config.enabled` is true.
        /// 2. The wave is open (`expected > 0` and `received <
        ///    expected`).
        /// 3. There is at least one `dimension.done` arrival
        ///    (a baseline to measure staleness against) — without
        ///    a baseline, the wave is simply "just started" and
        ///    the U4 aggregate-timeout path is the right
        ///    recovery.
        /// 4. `now - last_dimension_at > staleness_secs`.
        /// 5. The flow-lifecycle phase is **not** one of the
        ///    active worker phases (`WorkersActive`, `Spawning`)
        ///    — we only emit when the wave is otherwise idle
        ///    (post-aggregating, partial-closed, or failed),
        ///    which avoids racing with `inject_review_aggregate_timeouts`.
        pub fn evaluate(
            &self,
            registry: &FlowLifecycleRegistry,
            aggregate_timeout_secs: u64,
            wave_id: &str,
            expected: u32,
            received: u32,
            last_dimension_secs_ago: Option<u64>,
        ) -> Option<PlanBlockedPayload> {
            if !self.config.enabled {
                return None;
            }
            if expected == 0 || received >= expected {
                return None;
            }
            let last_dimension_secs_ago = last_dimension_secs_ago?;
            let staleness = self.staleness_secs(aggregate_timeout_secs);
            if last_dimension_secs_ago <= staleness {
                return None;
            }
            // Phase guard: only fire when the wave is otherwise
            // idle — `WorkersActive` / `Spawning` mean workers
            // are still racing and the aggregator hat has not
            // even been activated. The U4 path covers those
            // cases via `inject_review_aggregate_timeouts`.
            let phase = registry.get(wave_id).map(|r| r.phase);
            match phase {
                Some(FlowPhase::WorkersActive) | Some(FlowPhase::Spawning) => return None,
                _ => {}
            }
            Some(PlanBlockedPayload {
                reason: PlanBlockedPayload::REASON,
                wave_id: wave_id.to_string(),
                plan_name: String::new(),
                task_id: String::new(),
                step: String::new(),
                expected,
                received,
                missing_dimensions: Vec::new(),
                staleness_secs: staleness,
                aggregate_timeout_secs,
            })
        }
    }
}
