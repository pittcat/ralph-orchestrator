//! U6: Recovery Responder — soft alerts, targeted retry, escalation.
//!
//! This module is the *only* place in the orchestrator that converts a
//! [`RecoveryDiagnosisEnvelope`] into a runtime action. The drift detector
//! (U5) is a pure signal source; U4 write paths funnel envelopes into
//! `recovery.jsonl`; U6 is the policy layer that decides what to do with
//! the diagnosis.
//!
//! # Three escalation levels
//!
//! | Level | Action | Preconditions | Regression guard |
//! |---|---|---|---|
//! | Soft | prompt alert only | new finding or attempt < `max_repeated_recoveries` | does not publish new events, does not change termination |
//! | Hard | targeted `task.resume` | `safe_target == true` and same `retry_key` already escalated | target must be a registered hat, must not re-fire on the source hat |
//! | Final | pause/terminate with report hint | no safe target OR retry window exhausted | never replaces an existing [`crate::event_loop::TerminationReason::PayloadContractViolation`] |
//!
//! The thresholds come from
//! [`crate::config::RuntimeDiagnosisConfig`]:
//!
//! - `max_repeated_recoveries` controls the Soft → Hard transition.
//! - `retry_window_iterations` controls the *forget-after-N-iterations*
//!   policy so old findings do not haunt a long-running loop forever.
//!
//! # Non-regression
//!
//! - The responder never panics and never blocks on I/O.
//! - The responder does **not** write to `recovery.jsonl` or
//!   `orchestration.jsonl` directly. The caller is expected to keep
//!   using [`crate::diagnostics::DiagnosticsCollector::log_recovery`]
//!   and `log_orchestration`; the responder is a pure in-memory
//!   aggregator.
//! - The responder does **not** create a new parallel termination
//!   system. Its `TerminationHint` is *advisory*: the loop runner is
//!   free to ignore it when an existing reason already explains the
//!   loop end (e.g. `PayloadContractViolation`).
//!
//! [`crate::event_loop::TerminationReason::PayloadContractViolation`]:
//!     crate::event_loop::TerminationReason::PayloadContractViolation

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use ralph_proto::HatId;
use serde::Serialize;

use super::envelope::{
    DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
};
use super::journal::DriftMetric;
use crate::config::RuntimeDiagnosisConfig;
use crate::event_loop::rejection::extract_reason_code;

/// Minimum number of accepted-event samples the responder needs
/// to re-evaluate an `EmitCadence` finding. Mirrors
/// [`crate::drift::EMIT_CADENCE_MIN_SAMPLES`]; the constant is
/// duplicated here so the responder does not need to import the
/// drift module (which would create a `drift -> diagnosis -> drift`
/// cycle — `drift::alert` already depends on `diagnosis`).
pub const EMIT_CADENCE_RECOVERY_MIN_SAMPLES: usize = 5;

/// Per-event evidence handed to
/// [`RecoveryResponder::check_recovery`]. This is the responder's
/// view of an accepted event: enough metadata to re-evaluate the
/// specific drift metric that produced the original finding
/// (`field_completeness` needs the field set, `coord_join_rate`
/// needs `(from, to, ts)`, `emit_cadence` needs the timestamp
/// sequence). Source-hats are kept for the prompt-injection filter.
///
/// A plain topic list is no longer enough — see the R7 review
/// finding: "field_completeness 只要后续出现同 topic 就会标记
/// Recovered，即使缺失字段仍未恢复". The responder now re-derives
/// the metric from the accepted evidence instead of trusting
/// "topic == state.topic".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedEventEvidence {
    /// The accepted event's topic.
    pub topic: String,
    /// Top-level field names of a JSON-object payload. Empty for
    /// non-JSON payloads. The field-completeness metric uses this
    /// to verify the required field is actually present.
    pub fields: BTreeSet<String>,
    /// Hat that published the event, when known.
    pub source_hat: Option<String>,
    /// Wall-clock time the event was observed at. The coord-join
    /// and cadence metrics use this to compute inter-event
    /// intervals.
    pub timestamp: DateTime<Utc>,
}

/// Header for the prompt-injection block. Stable, machine-detectable,
/// distinct from `## ROBOT GUIDANCE` so the existing guidance detector
/// does not double-count it.
pub const RUNTIME_DIAGNOSIS_ALERT_HEADER: &str = "## Runtime Diagnosis Alert";

/// Maximum number of findings the responder will surface per prompt
/// even when the config allows more — a hard cap that protects the
/// prompt from a runaway drift storm. Configured via
/// [`RuntimeDiagnosisConfig::max_prompt_findings`]; this constant is
/// the *upper* bound (it must remain an internal sanity check, not a
/// user-facing knob).
const HARD_MAX_FINDINGS: usize = 32;

/// Capacity of the per-retry-key outcome history ring. The runtime-
/// recovery detector's flapping rule (plan 2026-06-28-003 §Defense 2 /
/// function 2) reads the last `FLAP_WINDOW=8` entries, so we keep
/// exactly that many to bound memory while making the snapshot
/// available without copying.
pub(super) const OUTCOME_HISTORY_CAP: usize = 8;

/// Push an outcome onto a bounded ring buffer, evicting the oldest
/// entry when at capacity. Centralised here so `observe()` does not
/// repeat the bounded-push logic.
fn push_outcome_history(buffer: &mut Vec<DiagnosisOutcome>, outcome: DiagnosisOutcome) {
    if buffer.len() >= OUTCOME_HISTORY_CAP {
        buffer.remove(0);
    }
    buffer.push(outcome);
}

/// What level of escalation a single finding triggered this iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationLevel {
    /// No new escalation — either a brand-new finding or a repeated
    /// finding still under the configured threshold. The caller
    /// should fold it into the prompt alert.
    Soft,
    /// Same `retry_key` was seen more than `max_repeated_recoveries`
    /// times in the retry window AND the envelope has a safe target
    /// hat. The caller should synthesize a targeted `task.resume`.
    Hard,
    /// No safe target OR the retry window has been exhausted. The
    /// caller should surface a `TerminationHint` so the loop runner
    /// can pause / report / escalate to human guidance.
    Final,
}

/// Per-iteration decision returned by [`RecoveryResponder::record_finding`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscalationDecision {
    /// The level this finding escalated to.
    pub level: EscalationLevel,
    /// The retry key the decision applies to.
    pub retry_key: String,
    /// Total attempt count for the same retry key (1-based, includes
    /// the current observation).
    pub attempt: u32,
    /// Recommended target hat for Hard escalation, when known.
    /// `None` for Soft and Final.
    pub target_hat: Option<String>,
    /// Reason string for Final escalation — used as a short
    /// `no_retry_reason` hint by the caller. `None` for Soft and Hard.
    pub reason: Option<String>,
}

/// The action a `Hard` escalation asks the caller to perform.
///
/// We keep this struct small and POD: the responder never touches the
/// `EventBus` directly. The runner takes the action and either
/// publishes it through `bus.publish` or feeds it into the existing
/// hard-gate path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryAction {
    /// The retry key the action targets.
    pub retry_key: String,
    /// Hat to route a `task.resume` event to.
    pub target_hat: HatId,
    /// Topic hint to include in the recovery event payload.
    pub topic_hint: Option<String>,
    /// The current attempt counter (1-based).
    pub attempt: u32,
    /// The current severity bucket.
    pub severity: DiagnosisSeverity,
}

impl RecoveryAction {
    /// U7a (plan 2026-06-21-002): convert a [`RecoveryAction`]
    /// into a [`crate::correction::CorrectionContext`].  Used
    /// by the drift engine's `drain_hard_escalations` path
    /// when `UNIFIED_DETERMINISTIC_CORRECTION=1` — the
    /// resulting context goes into
    /// `LoopState::prompt_context` instead of triggering a
    /// `task.resume` event on the bus.
    ///
    /// The `attempt` counter maps to `retry_count`; the
    /// R11 tripwire (`needs_escalation`) flips at the
    /// default 3-attempt threshold.
    pub fn to_correction_context(&self) -> crate::correction::CorrectionContext {
        let reason_code = format!("recovery:{}", extract_reason_code(&self.retry_key));
        let escalation_threshold = 3;
        crate::correction::CorrectionContext {
            reason_code,
            stage: "drift".to_string(),
            topic: self.topic_hint.clone().unwrap_or_default(),
            source_hat: Some(self.target_hat.as_str().to_string()),
            retry_key: self.retry_key.clone(),
            retry_count: self.attempt,
            escalation_threshold,
            needs_escalation: self.attempt >= escalation_threshold,
            last_message: format!("drift hard escalation: retry_key={}", self.retry_key),
            expected_payload_template: String::new(),
            allowed_topics: Vec::new(),
            required_fields: Vec::new(),
        }
    }
}

/// Advisory hint for the loop runner. The runner is free to ignore
/// this when an existing termination reason already explains the
/// outcome — in particular, it must NOT replace
/// `TerminationReason::PayloadContractViolation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminationHint {
    /// Why the responder suggests pausing / terminating.
    pub reason: String,
    /// The retry key that triggered the hint, when applicable.
    pub retry_key: Option<String>,
    /// Severity that triggered the hint. The runner may use this to
    /// weight the hint (e.g. only final-escalate on Critical).
    pub severity: DiagnosisSeverity,
    /// 2026-06-28-002 U2: escalation level that produced this hint.
    /// When `level == EscalationLevel::Final`, the hint is the
    /// terminal signal regardless of severity — a Warning-severity
    /// Final hint means the retry window was exhausted with no safe
    /// target and the loop MUST terminate.
    pub level: EscalationLevel,
}

/// Per-retry-key state.
#[derive(Debug, Clone)]
struct RetryState {
    /// Number of times this retry key was observed. Includes the
    /// current observation.
    attempt_count: u32,
    /// Loop iteration the key was first observed at.
    first_iteration: Option<u32>,
    /// Loop iteration the key was last observed at. Used for the
    /// R7 "grace period" rule: a retry key CANNOT be marked
    /// `Recovered` on the same iteration it was just produced —
    /// the original event in the snapshot must come from a
    /// later iteration's accepted event stream.
    last_iteration: Option<u32>,
    /// Most recent severity seen.
    last_severity: DiagnosisSeverity,
    /// Most recent outcome recorded.
    last_outcome: DiagnosisOutcome,
    /// Bounded ring of recent outcomes for flapping detection. Capped
    /// at [`OUTCOME_HISTORY_CAP`] entries so memory stays bounded in
    /// long loops; the runtime-recovery detector (plan 2026-06-28-003
    /// §Defense 2 / function 2) reads this snapshot to force a
    /// `plan.blocked` when a retry key flips among
    /// `Pending ↔ Recovered ↔ Repeated` too many times within the
    /// recent window.
    outcome_history: Vec<DiagnosisOutcome>,
    /// Optional target hat from the envelope, used for Hard
    /// escalation routing.
    target_hat: Option<String>,
    /// Optional topic hint for the recovery action payload.
    topic: Option<String>,
    /// Most recent source subsystem, used for the prompt alert and the
    /// orchestration audit event.
    source: DiagnosisSource,
    /// Whether the envelope had a safe target when last seen.
    safe_target: bool,
    /// Whether the responder has already escalated this key to Hard
    /// or Final in a previous iteration. Used to avoid re-emitting a
    /// `task.resume` every iteration once the threshold is crossed.
    escalated: bool,
    /// Drift metric that produced the original finding, when the
    /// source is [`DiagnosisSource::DriftMonitor`]. The responder
    /// uses this to pick the correct recovery criterion:
    /// `FieldCompleteness` → field set check,
    /// `CoordJoinRate` → declared-edge follow check,
    /// `EmitCadence` → inter-emit z-score check.
    /// `None` for non-drift findings; the responder falls back to
    /// the topic-presence rule.
    metric: Option<DriftMetric>,
    /// Required field name, when the metric is
    /// [`DriftMetric::FieldCompleteness`]. Pulled from
    /// [`RecoveryDiagnosisEnvelope::evidence`]'s last
    /// [`super::envelope::EvidenceKind::Field`] entry at observation
    /// time.
    required_field: Option<String>,
    /// Coord-join source topic, when the metric is
    /// [`DriftMetric::CoordJoinRate`]. The recovery criterion
    /// requires an accepted event on `from_topic` followed by an
    /// accepted event on `to_topic` (the state's `topic`).
    from_topic: Option<String>,
    /// Coord-join target topic. Mirrors `state.topic` for
    /// `CoordJoinRate` findings, kept separate so the responder
    /// can reason about edge cases (e.g. the from/to pair is
    /// declared in the detector's `DeclaredEdges`).
    to_topic: Option<String>,
}

impl RetryState {
    fn from_envelope(envelope: &RecoveryDiagnosisEnvelope) -> Self {
        // Drift findings carry a `metric` inside their `reason_code`
        // (the detector stamps `drift_field_completeness` /
        // `drift_coord_join_rate` / `drift_emit_cadence`). We
        // reverse that mapping here so the responder can pick
        // the right recovery rule without re-parsing the reason
        // string at every check.
        let metric = if matches!(envelope.source, DiagnosisSource::DriftMonitor) {
            match envelope.reason_code.as_str() {
                "drift_field_completeness" => Some(DriftMetric::FieldCompleteness),
                "drift_coord_join_rate" => Some(DriftMetric::CoordJoinRate),
                "drift_emit_cadence" => Some(DriftMetric::EmitCadence),
                _ => None,
            }
        } else {
            None
        };
        // The last Field-kind evidence ref carries the required
        // field name. Drift findings always stamp one.
        let required_field = envelope
            .evidence
            .iter()
            .rev()
            .find(|e| matches!(e.kind, super::envelope::EvidenceKind::Field))
            .map(|e| e.ref_path.clone());
        // Coord-join findings have neither `field` nor `topic`;
        // the from/to pair is encoded as a single Topic ref of
        // the form `from->to`. We split it back here so the
        // responder can compare cleanly.
        let (from_topic, to_topic) = envelope
            .evidence
            .iter()
            .rev()
            .find(|e| matches!(e.kind, super::envelope::EvidenceKind::Topic))
            .and_then(|e| {
                let mut parts = e.ref_path.splitn(2, "->");
                let from = parts.next()?.to_string();
                let to = parts.next()?.to_string();
                if from.is_empty() || to.is_empty() {
                    None
                } else {
                    Some((Some(from), Some(to)))
                }
            })
            .unwrap_or((None, None));
        Self {
            attempt_count: 1,
            first_iteration: envelope.iteration,
            last_iteration: envelope.iteration,
            last_severity: envelope.severity,
            last_outcome: envelope.outcome,
            outcome_history: vec![envelope.outcome],
            target_hat: envelope.target_hat.clone(),
            topic: envelope.topic.clone(),
            source: envelope.source,
            safe_target: envelope.safe_target,
            escalated: false,
            metric,
            required_field,
            from_topic,
            to_topic,
        }
    }
}

/// Soft, Hard, and Final escalation policy for runtime diagnosis
/// findings. Owned by [`crate::event_loop::EventLoop`]; the loop
/// runner feeds new envelopes into it via
/// [`crate::event_loop::EventLoop::record_recovery_envelope`].
#[derive(Debug)]
pub struct RecoveryResponder {
    /// Effective config (cloned per responder so config mutation
    /// during the run does not race the responder).
    config: Arc<RuntimeDiagnosisConfig>,
    /// Per-retry-key state.
    state: HashMap<String, RetryState>,
    /// Findings observed this iteration, retained for prompt
    /// injection. Cleared at the start of each iteration by
    /// [`Self::begin_iteration`].
    pending_findings: Vec<RecoveryDiagnosisEnvelope>,
    /// Retry keys that escalated to Hard in the most recent
    /// `record_finding` batch. The runner reads this to publish
    /// targeted `task.resume` events and clear the bit at the end of
    /// the iteration.
    last_hard_escalations: Vec<RecoveryAction>,
    /// `TerminationHint` produced by the most recent
    /// `record_finding` batch, if any. Cleared at the start of each
    /// iteration.
    last_termination_hint: Option<TerminationHint>,
}

impl RecoveryResponder {
    /// Construct a new responder. The config is shared via `Arc` so
    /// the rest of the loop can keep the same handle.
    #[must_use]
    pub fn new(config: Arc<RuntimeDiagnosisConfig>) -> Self {
        Self {
            config,
            state: HashMap::new(),
            pending_findings: Vec::new(),
            last_hard_escalations: Vec::new(),
            last_termination_hint: None,
        }
    }

    /// Read-only access to the effective config. Useful for tests
    /// and for the prompt builder.
    #[must_use]
    pub fn config(&self) -> &RuntimeDiagnosisConfig {
        &self.config
    }

    /// Open a new iteration. Clears the per-iteration `pending_findings`,
    /// `last_hard_escalations`, and `last_termination_hint` caches.
    /// The `state` map is preserved so cross-iteration aggregation
    /// (retry counters, recovery tracking) survives.
    pub fn begin_iteration(&mut self) {
        self.pending_findings.clear();
        self.last_hard_escalations.clear();
        self.last_termination_hint = None;
    }

    /// Number of distinct retry keys currently being tracked. Useful
    /// for unit tests and for the orchestration audit.
    #[must_use]
    pub fn tracked_retry_keys(&self) -> usize {
        self.state.len()
    }

    /// True when the responder has findings to fold into a prompt
    /// alert. Used by `apply_runtime_diagnosis_prompt` to skip the
    /// injection path entirely when the cache is empty.
    #[must_use]
    pub fn has_pending_findings(&self) -> bool {
        !self.pending_findings.is_empty()
    }

    /// Number of pending findings awaiting prompt injection.
    #[must_use]
    pub fn pending_finding_count(&self) -> usize {
        self.pending_findings.len()
    }

    /// Read-only access to the pending findings collected this
    /// iteration. Used by the runtime-recovery engine to inspect
    /// envelopes without re-parsing recovery.jsonl.
    #[must_use]
    pub fn pending_findings(&self) -> &[RecoveryDiagnosisEnvelope] {
        &self.pending_findings
    }

    /// Take the most recent hard-escalation actions. The runner
    /// publishes them as `task.resume` events and calls this again at
    /// the end of the iteration to clear the queue.
    pub fn drain_hard_escalations(&mut self) -> Vec<RecoveryAction> {
        std::mem::take(&mut self.last_hard_escalations)
    }

    /// Take the most recent termination hint, if any. Cleared by
    /// [`Self::begin_iteration`].
    pub fn take_termination_hint(&mut self) -> Option<TerminationHint> {
        self.last_termination_hint.take()
    }

    /// Read-only access to the most recent termination hint, if
    /// any. Unlike [`Self::take_termination_hint`], the hint is
    /// NOT removed; the loop runner's `finalize_recovery_diagnosis`
    /// can still consume it for the operator summary.
    ///
    /// This is the API the drift engine uses to decide whether to
    /// promote the hint into a [`crate::event_loop::TerminationReason`]
    /// — it needs to peek without destroying the value.
    #[must_use]
    pub fn peek_termination_hint(&self) -> Option<&TerminationHint> {
        self.last_termination_hint.as_ref()
    }

    /// Snapshot of every retry key the responder is currently
    /// tracking. Used by the drift engine to call
    /// [`Self::check_recovery`] for every tracked key after the
    /// runner has collected the iteration's accepted topics.
    #[must_use]
    pub fn tracked_retry_keys_list(&self) -> Vec<String> {
        self.state.keys().cloned().collect()
    }

    /// Look up the current outcome for `retry_key`. Returns
    /// `None` when the key is not tracked.
    ///
    /// Used by the drift engine to detect outcome transitions
    /// (Pending → Recovered, Pending → Repeated, ...) so it can
    /// write a `recovery.jsonl` line for the reporter to surface.
    #[must_use]
    pub fn outcome_for(&self, retry_key: &str) -> Option<DiagnosisOutcome> {
        self.state.get(retry_key).map(|s| s.last_outcome)
    }

    /// Snapshot the recent outcome history (newest last) for the
    /// runtime-recovery flapping detector. Returns an empty `Vec`
    /// when the retry key is not tracked.
    ///
    /// The buffer is capped at [`OUTCOME_HISTORY_CAP`] entries; the
    /// returned slice never exceeds that cap.
    #[must_use]
    pub fn outcome_history_snapshot(&self, retry_key: &str) -> Vec<DiagnosisOutcome> {
        self.state
            .get(retry_key)
            .map(|s| s.outcome_history.clone())
            .unwrap_or_default()
    }

    /// Most recent severity the responder recorded for
    /// `retry_key`, or `None` when the key is not tracked.
    #[must_use]
    pub fn last_severity_for(&self, retry_key: &str) -> Option<DiagnosisSeverity> {
        self.state.get(retry_key).map(|s| s.last_severity)
    }

    /// Topic the responder recorded for `retry_key`, or `None`
    /// when the key is not tracked or had no topic.
    #[must_use]
    pub fn target_topic_for(&self, retry_key: &str) -> Option<String> {
        self.state.get(retry_key).and_then(|s| s.topic.clone())
    }

    /// Record a new envelope and compute the escalation level for the
    /// current iteration. Pure in-memory operation: the caller is
    /// responsible for persisting the envelope via
    /// `DiagnosticsCollector::log_recovery` and emitting the audit
    /// event via `log_orchestration`.
    pub fn record_finding(
        &mut self,
        envelope: &RecoveryDiagnosisEnvelope,
        current_iteration: u32,
    ) -> EscalationDecision {
        let retry_key = envelope.retry_key.clone();
        let target_hat = envelope.target_hat.clone();
        let topic = envelope.topic.clone();
        let source = envelope.source;
        let safe_target = envelope.safe_target;
        let severity = envelope.severity;
        let attempt = self.observe(retry_key.clone(), envelope, current_iteration);
        // Stash the envelope for prompt injection this iteration.
        self.pending_findings.push(envelope.clone());

        let level = self.classify(&retry_key, current_iteration, safe_target);
        let mut decision = EscalationDecision {
            level,
            retry_key: retry_key.clone(),
            attempt,
            target_hat: None,
            reason: None,
        };
        match level {
            EscalationLevel::Soft => {}
            EscalationLevel::Hard => {
                if let Some(hat) = target_hat.clone() {
                    let action = RecoveryAction {
                        retry_key: retry_key.clone(),
                        target_hat: HatId::new(hat.clone()),
                        topic_hint: topic.clone(),
                        attempt,
                        severity,
                    };
                    self.last_hard_escalations.push(action);
                    decision.target_hat = Some(hat);
                }
            }
            EscalationLevel::Final => {
                let reason = if safe_target {
                    format!(
                        "retry window exhausted for retry_key={retry_key} (>= {attempts} attempts within {window} iterations)",
                        attempts = self.config.max_repeated_recoveries,
                        window = self.config.retry_window_iterations,
                    )
                } else {
                    format!("no safe retry target for retry_key={retry_key}")
                };
                self.last_termination_hint = Some(TerminationHint {
                    reason: reason.clone(),
                    retry_key: Some(retry_key.clone()),
                    severity,
                    // 2026-06-28-002 U2: stamp the Final level
                    // so `check_termination_hint` can promote
                    // Warning-severity Final hints to
                    // `RecoveryExhausted`.
                    level: EscalationLevel::Final,
                });
                decision.reason = Some(reason);
                decision.target_hat = target_hat;
                let _ = source; // Reserved for future audit fields.
            }
        }
        decision
    }

    /// Returns the most recent attempt counter for `retry_key` (1-based),
    /// or 0 when the key has not been observed.
    #[must_use]
    pub fn attempt_count(&self, retry_key: &str) -> u32 {
        self.state.get(retry_key).map_or(0, |s| s.attempt_count)
    }

    /// True when the responder has a safe target hat for `retry_key`.
    /// Used by the runner before publishing a `task.resume` to avoid
    /// targeting a hat that is not registered.
    #[must_use]
    pub fn has_safe_target(&self, retry_key: &str) -> bool {
        self.state
            .get(retry_key)
            .is_some_and(|s| s.safe_target && s.target_hat.is_some())
    }

    /// Look up the recommended target hat for a retry key, when one
    /// is known. `None` means "no safe target" — the caller must NOT
    /// synthesize a fake target.
    #[must_use]
    pub fn target_hat_for_retry(&self, retry_key: &str) -> Option<String> {
        self.state
            .get(retry_key)
            .and_then(|s| s.target_hat.clone())
            .filter(|_| self.has_safe_target(retry_key))
    }

    /// Mark a `retry_key` as recovered when the next iteration's
    /// accepted events include the expected `topic`. Returns the
    /// resulting outcome when state was updated, or `None` when the
    /// Mark a `retry_key` as recovered when the iteration that just
    /// completed satisfies the **specific** recovery criterion for
    /// the finding's drift metric. Returns the resulting outcome
    /// when state was updated, or `None` when the key is not
    /// tracked.
    ///
    /// # Per-metric recovery rules
    ///
    /// | Metric | Recovery condition |
    /// |---|---|
    /// | `FieldCompleteness` | At least one accepted event on `topic` includes the `required_field` |
    /// | `CoordJoinRate` | At least one accepted event on `to_topic` is timestamped after an accepted event on `from_topic` |
    /// | `EmitCadence` | At least [`EMIT_CADENCE_RECOVERY_MIN_SAMPLES`] accepted events on `topic` form a stable cadence (worst positive z-score < `2.0`) |
    /// | non-drift / unknown | Backward-compatible: at least one accepted event on `state.topic`, OR (topic-less) any accepted event on the target hat |
    ///
    /// # Grace period (R7)
    ///
    /// A retry key CANNOT be marked `Recovered` on the same
    /// iteration it was just recorded. The original event is part
    /// of the same iteration's evidence stream, so a finding
    /// produced in iteration `N` and an "evidence" event also in
    /// iteration `N` would be self-referential and would mask
    /// genuine recovery regressions. We require `current_iteration
    /// > state.last_iteration` (the iteration the finding was
    /// > produced at) for the recovery path to fire.
    ///
    /// `accepted_evidence` should be the **accepted** event stream
    /// of the iteration that just completed (i.e. the events that
    /// passed through `EventPolicy` AND were not rejected by the
    /// `EventOriginGuard`). The per-event `fields` and `timestamp`
    /// fields are required for the metric-specific rules; callers
    /// that only have a topic list should look at
    /// [`Self::check_recovery_topics`] instead.
    pub fn check_recovery(
        &mut self,
        retry_key: &str,
        accepted_evidence: &[AcceptedEventEvidence],
        current_iteration: u32,
    ) -> Option<DiagnosisOutcome> {
        let state = self.state.get_mut(retry_key)?;
        // R7: a finding cannot self-heal in the iteration it was
        // recorded. The drift snapshot and the next iteration's
        // accepted events must come from different iterations.
        if state
            .last_iteration
            .is_some_and(|li| current_iteration <= li)
        {
            // We still need to set the outcome so the engine's
            // transition detector does not flap. Pending is the
            // most informative default — the finding has not yet
            // had a chance to recover.
            state.last_outcome = DiagnosisOutcome::Pending;
            state.last_iteration = Some(current_iteration);
            return Some(DiagnosisOutcome::Pending);
        }
        // First, try the metric-specific rule. Drift findings
        // carry a `metric`; non-drift findings fall through to the
        // topic-presence rule.
        let metric_recovered = match state.metric {
            Some(DriftMetric::FieldCompleteness) => {
                check_field_completeness_recovered(state, accepted_evidence)
            }
            Some(DriftMetric::CoordJoinRate) => {
                check_coord_join_rate_recovered(state, accepted_evidence)
            }
            Some(DriftMetric::EmitCadence) => {
                check_emit_cadence_recovered(state, accepted_evidence)
            }
            None => false,
        };
        // R7: when the finding has a metric, the topic-presence
        // rule MUST NOT be used as a fallback. Otherwise a
        // `field_completeness` finding that still has the field
        // missing would recover as soon as a topic-matched
        // event flows — which is exactly the regression the
        // R7 review called out. The topic rule is reserved for
        // non-drift findings (`MissingEventGate`,
        // `WorkflowGuard`, ...) where the topic-presence
        // condition is the genuine recovery signal.
        let topic_recovered = if state.metric.is_some() {
            false
        } else {
            check_topic_recovered(state, accepted_evidence)
        };
        let recovered = metric_recovered || topic_recovered;
        if recovered {
            state.last_outcome = DiagnosisOutcome::Recovered;
        } else if state.attempt_count > 1 {
            state.last_outcome = DiagnosisOutcome::Repeated;
        } else {
            state.last_outcome = DiagnosisOutcome::Pending;
        }
        state.last_iteration = Some(current_iteration);
        Some(state.last_outcome)
    }

    /// Backward-compatible recovery check that takes only the
    /// topic list. Useful for callers that have not yet wired
    /// [`AcceptedEventEvidence`] (e.g. older tests). The non-drift
    /// recovery rule is applied: at least one accepted topic must
    /// match `state.topic` (or, when topic-less, any non-empty
    /// topic must match the target hat). The R7 grace period
    /// still applies.
    ///
    /// New callers should prefer [`Self::check_recovery`] so the
    /// metric-specific rule is honoured for drift findings.
    pub fn check_recovery_topics(
        &mut self,
        retry_key: &str,
        accepted_topics: &[String],
        current_iteration: u32,
    ) -> Option<DiagnosisOutcome> {
        let evidence: Vec<AcceptedEventEvidence> = accepted_topics
            .iter()
            .filter(|t| !t.is_empty())
            .map(|t| AcceptedEventEvidence {
                topic: t.clone(),
                fields: BTreeSet::new(),
                source_hat: None,
                timestamp: DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_else(chrono::Utc::now),
            })
            .collect();
        self.check_recovery(retry_key, &evidence, current_iteration)
    }

    /// Build the prompt-injection block. Returns a new prompt string
    /// with the alert appended (or unchanged when no findings apply).
    ///
    /// `hat_id` is the hat the prompt is being built for. In
    /// coordinator / solo mode the responder injects every finding;
    /// the helper caller passes `None` for those paths. In isolated
    /// mode, only findings whose `target_hat` (or `source_hat` when
    /// target is `None`) matches the given hat are surfaced — the
    /// plan requires "isolated hat mode 下 alert 只注入目标 hat".
    ///
    /// `current_iteration` is the loop iteration the prompt is being
    /// built for. The responder never injects a finding that was
    /// already marked [`DiagnosisOutcome::Recovered`].
    #[must_use]
    pub fn inject_prompt_alert(
        &self,
        prompt: &str,
        hat_id: Option<&HatId>,
        current_iteration: u32,
    ) -> String {
        if !self.config.enabled || !self.config.prompt_injection_enabled {
            return prompt.to_string();
        }
        if self.pending_findings.is_empty() {
            return prompt.to_string();
        }

        let max_chars = self.config.max_prompt_chars.max(1);
        let max_findings = self.config.max_prompt_findings.clamp(1, HARD_MAX_FINDINGS);

        // Filter findings for the current hat and recover status.
        // The state map is the source of truth for recovery: an
        // envelope whose state is `Recovered` is dropped even if the
        // original envelope in `pending_findings` still carries
        // `Pending`. The state map is updated by `check_recovery`.
        let mut eligible: Vec<&RecoveryDiagnosisEnvelope> = self
            .pending_findings
            .iter()
            .filter(|env| {
                self.state
                    .get(&env.retry_key)
                    .map(|s| s.last_outcome != DiagnosisOutcome::Recovered)
                    .unwrap_or(true)
            })
            .filter(|env| match hat_id {
                None => true, // coordinator / solo: surface all
                Some(hat) => {
                    let hat_str = hat.as_str();
                    env.target_hat.as_deref() == Some(hat_str)
                        || env.source_hat.as_deref() == Some(hat_str)
                }
            })
            .collect();
        // Stable order: by iteration then by retry_key.
        eligible.sort_by(|a, b| {
            a.iteration
                .cmp(&b.iteration)
                .then_with(|| a.retry_key.cmp(&b.retry_key))
        });
        eligible.truncate(max_findings);

        if eligible.is_empty() {
            return prompt.to_string();
        }

        let mut body = String::from(RUNTIME_DIAGNOSIS_ALERT_HEADER);
        body.push_str("\n\nThe runtime diagnosis layer observed the following issues in the recent loop. Address them in priority order before producing new work.\n\n");

        for env in &eligible {
            let line = format_finding_line(env, current_iteration);
            body.push_str("- ");
            body.push_str(&line);
            body.push('\n');
            if let Some(action) = &env.expected_action {
                body.push_str("  expected action: ");
                body.push_str(action);
                body.push('\n');
            }
        }

        body.push_str(
            "\nFull details in `.ralph/diagnostics/<session>/recovery.jsonl`. \
             Do NOT re-emit the same payload without addressing the diagnosis.\n",
        );

        let truncated = truncate_to_chars(&body, max_chars);
        if truncated.is_empty() {
            return prompt.to_string();
        }

        // Append the alert to the prompt. The recommended order (per
        // the plan) is: skills prefix → base prompt → phase section
        // → diagnosis alert. The caller is responsible for the
        // skills prefix; the alert goes after the phase section,
        // which is already inside `prompt` at this point.
        let mut out = String::with_capacity(prompt.len() + truncated.len() + 2);
        out.push_str(prompt);
        if !prompt.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        out.push_str(&truncated);
        out
    }

    /// Mark the responder as escalated for `retry_key` (Hard or
    /// Final). Prevents repeated emission of the same Hard action on
    /// the next `record_finding` call.
    fn mark_escalated(&mut self, retry_key: &str) {
        if let Some(state) = self.state.get_mut(retry_key) {
            state.escalated = true;
        }
    }

    /// Update the in-memory state for `retry_key` and return the new
    /// attempt counter.
    fn observe(
        &mut self,
        retry_key: String,
        envelope: &RecoveryDiagnosisEnvelope,
        current_iteration: u32,
    ) -> u32 {
        let window = self.config.retry_window_iterations.max(1) as u32;
        // We pre-compute the metric-derived fields outside the
        // `get_mut` borrow so the second-arm mutation does not
        // double-borrow `self`.
        let incoming = RetryState::from_envelope(envelope);
        
        match self.state.get_mut(&retry_key) {
            None => {
                // First observation: seed the state. We do the
                // seeding here instead of via `Entry::or_insert_with`
                // so the `attempt_count` field can be set explicitly
                // (the constructor in `from_envelope` is shared with
                // callers that do not want to bump the counter).
                let mut state = incoming;
                state.first_iteration = envelope.iteration;
                state.last_iteration = Some(current_iteration);
                state.attempt_count = 1;
                state.outcome_history = vec![envelope.outcome];
                self.state.insert(retry_key.clone(), state);
                1
            }
            Some(entry) => {
                // Stale-window reset: when the gap between current
                // and first observation exceeds the configured
                // window AND the key has not already escalated, treat
                // the new observation as a fresh start. This keeps
                // the state map bounded in long loops and prevents
                // old findings from haunting the responder.
                if current_iteration.saturating_sub(entry.first_iteration.unwrap_or(0)) > window
                    && !entry.escalated
                {
                    entry.attempt_count = 1;
                    entry.first_iteration = envelope.iteration;
                    entry.outcome_history.clear();
                } else {
                    entry.attempt_count = entry.attempt_count.saturating_add(1);
                }
                entry.last_iteration = Some(current_iteration);
                entry.last_severity = envelope.severity;
                entry.target_hat = envelope.target_hat.clone();
                entry.topic = envelope.topic.clone();
                entry.source = envelope.source;
                entry.safe_target = envelope.safe_target;
                // Refresh the metric-derived fields so a re-fired
                // finding (e.g. operator retried a different
                // schema) re-uses the latest evidence rather than
                // the stale values from the first observation.
                entry.metric = incoming.metric;
                entry.required_field = incoming.required_field;
                entry.from_topic = incoming.from_topic;
                entry.to_topic = incoming.to_topic;
                entry.last_outcome = envelope.outcome;
                push_outcome_history(&mut entry.outcome_history, envelope.outcome);
                entry.attempt_count
            }
        }
    }

    /// Classify the current observation into a Soft / Hard / Final
    /// escalation level.
    fn classify(
        &mut self,
        retry_key: &str,
        current_iteration: u32,
        safe_target: bool,
    ) -> EscalationLevel {
        let max_repeats = self.config.max_repeated_recoveries.max(1) as u32;
        let window = self.config.retry_window_iterations.max(1) as u32;
        // P0-1 (plan 2026-06-29-006): when classifying
        // `missing_event_gate:*` or `stall_recovery:*` retry
        // keys, also fold in the attempt_count of the *sibling*
        // key on the same (hat, topic) — the two paths diagnose
        // the same root cause in the handoff path and the
        // P1-1 dedup guard (`hard_gate.rs`) catches the in-flight
        // case, but if a duplicate envelope slips through (e.g.
        // across iterations) the two attempt counters must not
        // accumulate independently. See
        // 2026-06-29-ce-executor-serial-primary-172725 §F1/F2.
        let merged_attempts = self.merge_sibling_attempts(retry_key);
        let merged_first_iteration = self.merge_sibling_first_iteration(retry_key);
        let (attempts, escalated) = self
            .state
            .get(retry_key)
            .map_or((0_u32, false), |s| (s.attempt_count, s.escalated));
        let attempts = attempts.max(merged_attempts);
        let over_threshold = attempts >= max_repeats;
        let over_window = current_iteration.saturating_sub(
            self.state
                .get(retry_key)
                .map_or(merged_first_iteration, |s| {
                    s.first_iteration.unwrap_or(merged_first_iteration)
                }),
        ) >= window;
        if !over_threshold {
            return EscalationLevel::Soft;
        }
        if !safe_target {
            // No registered hat to route to. Pause / report / human
            // guidance instead of synthesizing a fake `task.resume`.
            self.mark_escalated(retry_key);
            return EscalationLevel::Final;
        }
        if over_window {
            self.mark_escalated(retry_key);
            return EscalationLevel::Final;
        }
        if escalated {
            // Already escalated; re-firing the same `task.resume`
            // every iteration would spam the bus. Stay at Soft so
            // the prompt alert still surfaces the finding.
            return EscalationLevel::Soft;
        }
        self.mark_escalated(retry_key);
        EscalationLevel::Hard
    }

    /// P0-1 (plan 2026-06-29-006): for `stall_recovery` and
    /// `missing_event_gate` retry keys, look up the sibling key
    /// on the same (hat, topic) and return the larger
    /// `attempt_count`. Returns 0 when no sibling exists.
    fn merge_sibling_attempts(&self, retry_key: &str) -> u32 {
        let Some((sibling_prefix, suffix)) = self.sibling_lookup(retry_key) else {
            return 0;
        };
        self.state
            .keys()
            .filter(|k| k.starts_with(&sibling_prefix) && k.ends_with(&suffix))
            .filter_map(|k| self.state.get(k).map(|s| s.attempt_count))
            .max()
            .unwrap_or(0)
    }

    /// Same idea as [`Self::merge_sibling_attempts`] but for
    /// `first_iteration`: return the earliest first_iteration
    /// across the key and its sibling. The window check uses
    /// the earliest observation so the merged retry class
    /// cannot outlive either side's window.
    fn merge_sibling_first_iteration(&self, retry_key: &str) -> u32 {
        let Some((sibling_prefix, suffix)) = self.sibling_lookup(retry_key) else {
            return 0;
        };
        self.state
            .keys()
            .filter(|k| k.starts_with(&sibling_prefix) && k.ends_with(&suffix))
            .filter_map(|k| self.state.get(k).and_then(|s| s.first_iteration))
            .min()
            .unwrap_or(0)
    }

    /// Map a `stall_recovery` retry key to its
    /// `missing_event_gate` sibling (and vice versa). Both keys
    /// have the form
    /// `<source>:<hat>:<topic>:<reason>:<field>` — the
    /// `reason` segment is the differentiator. Returns
    /// `(sibling_prefix, field_suffix)`:
    /// - `sibling_prefix` matches the keys' shared leading
    ///   three segments + the alternative source + the
    ///   alternative reason;
    /// - `field_suffix` is the trailing `<field>` so we only
    ///   match keys with the same `field` slot (`*` for both
    ///   call sites in this fix).
    fn sibling_lookup(&self, retry_key: &str) -> Option<(String, String)> {
        // The retry_key format is
        // `<source>:<hat>:<topic>:<reason>:<field>` with five
        // colon-separated parts. We split on `:` to inspect
        // the source and reason.
        let parts: Vec<&str> = retry_key.split(':').collect();
        if parts.len() != 5 {
            return None;
        }
        let (source, hat, topic, reason, field) =
            (parts[0], parts[1], parts[2], parts[3], parts[4]);
        let (sibling_source, sibling_reason) = match (source, reason) {
            ("stall_recovery", _) => ("missing_event_gate", "missing_event"),
            ("missing_event_gate", _) => ("stall_recovery", "handoff_dispatch_timeout"),
            _ => return None,
        };
        Some((
            format!("{sibling_source}:{hat}:{topic}:{sibling_reason}:"),
            field.to_string(),
        ))
    }
}

/// Format a single finding line for the prompt alert.
fn format_finding_line(env: &RecoveryDiagnosisEnvelope, current_iteration: u32) -> String {
    let retry_attempt = env.retry_attempt.max(1);
    // Repeated findings keep the original attempt counter; the
    // caller already passed it in via `retry_attempt`.
    let attempt_for_state = retry_attempt;
    let topic = env.topic.as_deref().unwrap_or("*");
    let target = env.target_hat.as_deref().unwrap_or("*");
    let source_hat = env.source_hat.as_deref().unwrap_or("*");
    let severity = env.severity.as_str();
    format!(
        "[{severity}] source={source} target={target} topic={topic} hat={source_hat} attempt={n} iter={iter} — {msg}",
        severity = severity,
        source = env.source.as_str(),
        target = target,
        topic = topic,
        source_hat = source_hat,
        n = attempt_for_state,
        iter = current_iteration,
        msg = env.message,
    )
}

/// R7: `FieldCompleteness` recovery rule.
///
/// The finding is recovered when at least one accepted event on
/// `state.topic` carries the `required_field` in its top-level
/// field set. A bare topic-presence match (e.g. "an event of
/// topic `work.done` flowed through the bus") is NOT enough: the
/// review explicitly rejected that as self-recovery because
/// missing fields do not magically reappear.
fn check_field_completeness_recovered(
    state: &RetryState,
    evidence: &[AcceptedEventEvidence],
) -> bool {
    let Some(topic) = state.topic.as_deref() else {
        return false;
    };
    let Some(field) = state.required_field.as_deref() else {
        // Field-completeness findings always carry a field.
        // Without one we cannot decide, so we treat the finding
        // as NOT recovered.
        return false;
    };
    evidence
        .iter()
        .filter(|e| e.topic == topic)
        .any(|e| e.fields.contains(field))
}

/// R7: `CoordJoinRate` recovery rule.
///
/// The finding is recovered when the accepted event stream
/// contains at least one `(from_topic, to_topic)` pair where
/// the `to_topic` event is timestamped at or after the
/// `from_topic` event — i.e. the join actually happened in the
/// observed window. We do not require the gap to be tight; the
/// detector's threshold rule still applies. We just need to
/// see the join.
fn check_coord_join_rate_recovered(state: &RetryState, evidence: &[AcceptedEventEvidence]) -> bool {
    let (Some(from_topic), Some(to_topic)) = (
        state.from_topic.as_deref(),
        state.to_topic.as_deref().or(state.topic.as_deref()),
    ) else {
        return false;
    };
    // Find the latest from-topic timestamp we have observed in
    // the accepted evidence, and check whether any to-topic
    // event was emitted at or after it.
    let latest_from = evidence
        .iter()
        .filter(|e| e.topic == from_topic)
        .map(|e| e.timestamp)
        .max();
    let Some(latest_from) = latest_from else {
        return false;
    };
    evidence
        .iter()
        .any(|e| e.topic == to_topic && e.timestamp >= latest_from)
}

/// R7: `EmitCadence` recovery rule.
///
/// The finding is recovered when the accepted event stream on
/// `state.topic` has at least [`EMIT_CADENCE_MIN_SAMPLES`]
/// events that form a stable cadence. We define "stable" as
/// "the worst positive z-score of the inter-emit intervals
/// is below 2σ" — the same threshold the detector uses to
/// raise a finding in the first place. A finding that was
/// produced by a z-score > 2 is recovered only when the new
/// z-score drops below 2.
///
/// We do not import the detector to keep the responder free
/// of drift-internal types; the formula is small enough to
/// re-implement here.
fn check_emit_cadence_recovered(state: &RetryState, evidence: &[AcceptedEventEvidence]) -> bool {
    let Some(topic) = state.topic.as_deref() else {
        return false;
    };
    let timestamps: Vec<DateTime<Utc>> = evidence
        .iter()
        .filter(|e| e.topic == topic)
        .map(|e| e.timestamp)
        .collect();
    if timestamps.len() < EMIT_CADENCE_RECOVERY_MIN_SAMPLES {
        return false;
    }
    // Inter-emit intervals in seconds, sorted by timestamp
    // (the input is already in accepted order but we re-sort
    // to be safe).
    let mut sorted = timestamps;
    sorted.sort();
    let intervals: Vec<f64> = sorted
        .windows(2)
        .map(|w| (w[1] - w[0]).num_milliseconds() as f64 / 1000.0)
        .filter(|iv| *iv > 0.0)
        .collect();
    if intervals.len() < 2 {
        // All events share the same timestamp (e.g. wave): the
        // detector itself marks this as a silent path. We do
        // the same — the cadence is not stable, it is undefined.
        return false;
    }
    let avg = intervals.iter().sum::<f64>() / intervals.len() as f64;
    let var = intervals.iter().map(|iv| (iv - avg).powi(2)).sum::<f64>() / intervals.len() as f64;
    let stddev = var.sqrt();
    let worst_z = if stddev <= 0.0 {
        0.0
    } else {
        intervals
            .iter()
            .map(|iv| (iv - avg).max(0.0) / stddev)
            .fold(0.0_f64, f64::max)
    };
    // The detector uses `emit_cadence_sigma` from
    // `DriftConfig::emit_cadence_sigma` (default 2.0). The
    // responder mirrors the same default. If the runtime
    // overrides it, the detector's threshold and the
    // responder's recovery threshold should be tuned together;
    // see `DriftConfig::emit_cadence_sigma` for the source of
    // truth.
    worst_z < 2.0
}

/// Fallback recovery rule for findings without a metric
/// (`MissingEventGate`, `WorkflowGuard`, ...) and topic-less
/// envelopes (e.g. some `StallRecovery` cases). Returns `true`
/// when the accepted evidence contains a topic that matches
/// `state.topic` (or, when topic-less, contains a non-empty
/// event whose source matches `state.target_hat`).
fn check_topic_recovered(state: &RetryState, evidence: &[AcceptedEventEvidence]) -> bool {
    if let Some(topic) = state.topic.as_deref() {
        return evidence.iter().any(|e| e.topic == topic);
    }
    if let Some(target) = state.target_hat.as_deref() {
        return evidence
            .iter()
            .any(|e| e.source_hat.as_deref() == Some(target));
    }
    false
}

/// Truncate a string to at most `max_chars` characters, appending the
/// Unicode horizontal ellipsis when truncation happens. Returns the
/// input unchanged when it already fits.
fn truncate_to_chars(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DriftConfig, MalformedJsonlPolicy};

    fn cfg_with(max_repeats: usize, window: usize, prompt: bool) -> Arc<RuntimeDiagnosisConfig> {
        Arc::new(RuntimeDiagnosisConfig {
            enabled: true,
            write_artifacts: false,
            prompt_injection_enabled: prompt,
            max_prompt_findings: 5,
            max_prompt_chars: 2000,
            retry_window_iterations: window,
            max_repeated_recoveries: max_repeats,
            artifact_retention: 10,
            malformed_jsonl_policy: MalformedJsonlPolicy::Warn,
            drift: DriftConfig::default(),
        })
    }

    fn envelope(
        retry_key: &str,
        iteration: u32,
        severity: DiagnosisSeverity,
        safe_target: bool,
        target: Option<&str>,
        source: DiagnosisSource,
    ) -> RecoveryDiagnosisEnvelope {
        let mut b = RecoveryDiagnosisEnvelope::builder()
            .source(source)
            .severity(severity)
            .iteration(iteration)
            .reason_code("test")
            .message("m")
            .retry_key(retry_key)
            .safe_target(safe_target);
        if let Some(t) = target {
            b = b.target_hat(t);
        }
        b.build()
    }

    #[test]
    fn first_finding_is_soft() {
        let mut r = RecoveryResponder::new(cfg_with(3, 5, true));
        r.begin_iteration();
        let env = envelope(
            "k:builder:work_done:r:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let d = r.record_finding(&env, 1);
        assert_eq!(d.level, EscalationLevel::Soft);
        assert_eq!(d.attempt, 1);
        assert!(r.has_pending_findings());
    }

    #[test]
    fn three_repeats_stay_soft_below_threshold() {
        let mut r = RecoveryResponder::new(cfg_with(3, 5, true));
        for i in 1..=2 {
            r.begin_iteration();
            let env = envelope(
                "k:builder:work_done:r:*",
                i,
                DiagnosisSeverity::Warning,
                true,
                Some("builder"),
                DiagnosisSource::MissingEventGate,
            );
            let d = r.record_finding(&env, i);
            assert_eq!(d.level, EscalationLevel::Soft, "iter {i}");
        }
        r.begin_iteration();
        let env = envelope(
            "k:builder:work_done:r:*",
            3,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let d = r.record_finding(&env, 3);
        assert_eq!(d.level, EscalationLevel::Hard, "iter 3 should be Hard");
        assert_eq!(d.attempt, 3);
        assert_eq!(d.target_hat.as_deref(), Some("builder"));
        let actions = r.drain_hard_escalations();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].target_hat.as_str(), "builder");
    }

    #[test]
    fn no_safe_target_skips_hard_and_surfaces_final() {
        let mut r = RecoveryResponder::new(cfg_with(2, 5, true));
        for i in 1..=2 {
            r.begin_iteration();
            let env = envelope(
                "k:ralph:*:stall:*",
                i,
                DiagnosisSeverity::Error,
                false,
                None,
                DiagnosisSource::StallRecovery,
            );
            let d = r.record_finding(&env, i);
            if i < 2 {
                assert_eq!(d.level, EscalationLevel::Soft);
            } else {
                assert_eq!(d.level, EscalationLevel::Final);
            }
        }
        let actions = r.drain_hard_escalations();
        assert!(actions.is_empty());
        let hint = r.take_termination_hint();
        assert!(hint.is_some());
        assert!(!hint.unwrap().reason.is_empty());
    }

    #[test]
    fn retry_window_exhaustion_raises_final_even_with_target() {
        let mut r = RecoveryResponder::new(cfg_with(3, 2, true));
        // Three iterations with the same key, the window of 2 means
        // the 3rd observation is far past the window.
        for i in 1..=3 {
            r.begin_iteration();
            let env = envelope(
                "k:builder:*:r:*",
                i,
                DiagnosisSeverity::Error,
                true,
                Some("builder"),
                DiagnosisSource::MissingEventGate,
            );
            let d = r.record_finding(&env, i);
            if i == 3 {
                assert_eq!(d.level, EscalationLevel::Final);
            } else {
                assert_eq!(d.level, EscalationLevel::Soft);
            }
        }
    }

    #[test]
    fn recovery_marks_outcome_and_drops_finding_from_prompt() {
        let mut r = RecoveryResponder::new(cfg_with(3, 5, true));
        r.begin_iteration();
        let env = envelope(
            "k:builder:work_done:r:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let _ = r.record_finding(&env, 1);

        // Pretend the next iteration accepted the topic the envelope
        // was complaining about. `check_recovery` is the source of
        // truth for "this finding no longer needs a prompt alert".
        let evidence = vec![AcceptedEventEvidence {
            topic: "work.done".to_string(),
            fields: BTreeSet::new(),
            source_hat: Some("builder".to_string()),
            timestamp: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        }];
        let outcome = r.check_recovery("k:builder:work_done:r:*", &evidence, 2);
        assert_eq!(outcome, Some(DiagnosisOutcome::Recovered));
        // The inject filter consults the state map, not the
        // envelope's outcome field, so the alert is dropped without
        // any extra `pending_findings.retain(...)` plumbing.
        let hat = HatId::new("builder");
        let prompt = r.inject_prompt_alert("base", Some(&hat), 2);
        assert!(!prompt.contains("Runtime Diagnosis Alert"));
    }

    #[test]
    fn isolated_hat_prompt_filters_unrelated_findings() {
        let mut r = RecoveryResponder::new(cfg_with(3, 5, true));
        r.begin_iteration();
        let builder_env = envelope(
            "k:builder:work_done:r:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let planner_env = envelope(
            "k:planner:plan.x:r:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("planner"),
            DiagnosisSource::MissingEventGate,
        );
        let _ = r.record_finding(&builder_env, 1);
        let _ = r.record_finding(&planner_env, 1);
        let builder_hat = HatId::new("builder");
        let prompt = r.inject_prompt_alert("base", Some(&builder_hat), 1);
        assert!(prompt.contains("builder"));
        assert!(!prompt.contains("plan.x"));
    }

    #[test]
    fn prompt_injection_disabled_skips_alert() {
        let mut r = RecoveryResponder::new(cfg_with(3, 5, false));
        r.begin_iteration();
        let env = envelope(
            "k:builder:work_done:r:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let _ = r.record_finding(&env, 1);
        let hat = HatId::new("builder");
        let prompt = r.inject_prompt_alert("base", Some(&hat), 1);
        assert_eq!(prompt, "base");
    }

    #[test]
    fn prompt_alert_truncated_to_max_chars() {
        let mut r = RecoveryResponder::new(Arc::new(RuntimeDiagnosisConfig {
            enabled: true,
            write_artifacts: false,
            prompt_injection_enabled: true,
            max_prompt_findings: 50,
            max_prompt_chars: 80,
            retry_window_iterations: 5,
            max_repeated_recoveries: 3,
            artifact_retention: 10,
            malformed_jsonl_policy: MalformedJsonlPolicy::Warn,
            drift: DriftConfig::default(),
        }));
        r.begin_iteration();
        for i in 0..5 {
            let env = envelope(
                &format!("k:builder:work_done:long_long_message:{i}"),
                1,
                DiagnosisSeverity::Warning,
                true,
                Some("builder"),
                DiagnosisSource::MissingEventGate,
            );
            let _ = r.record_finding(&env, 1);
        }
        let hat = HatId::new("builder");
        let prompt = r.inject_prompt_alert("base", Some(&hat), 1);
        // The alert body must be at most `max_prompt_chars` chars;
        // the helper adds a small separator (one or two newlines)
        // between the original prompt and the alert.
        let added = &prompt["base".len()..];
        // Find the start of the alert header to skip the separator.
        let alert_start = added
            .find(RUNTIME_DIAGNOSIS_ALERT_HEADER)
            .expect("alert header should be present");
        let alert_body = &added[alert_start..];
        assert!(
            alert_body.chars().count() <= 80,
            "alert body len = {}",
            alert_body.chars().count()
        );
        // Sanity: the body must end with the truncation ellipsis.
        assert!(alert_body.ends_with('\u{2026}'));
    }

    #[test]
    fn hard_escalation_does_not_re_fire_after_first_escalation() {
        let mut r = RecoveryResponder::new(cfg_with(2, 5, true));
        for i in 1..=3 {
            r.begin_iteration();
            let env = envelope(
                "k:builder:work_done:r:*",
                i,
                DiagnosisSeverity::Error,
                true,
                Some("builder"),
                DiagnosisSource::MissingEventGate,
            );
            let d = r.record_finding(&env, i);
            if i == 2 {
                assert_eq!(d.level, EscalationLevel::Hard);
            } else {
                // The 3rd iteration observes the same already-escalated
                // key. The responder stays at Soft because the
                // `task.resume` was already published.
                assert_eq!(d.level, EscalationLevel::Soft);
            }
        }
    }

    #[test]
    fn final_hint_does_not_include_payload_contract_violation_reason() {
        // The U6 plan explicitly forbids overwriting
        // `TerminationReason::PayloadContractViolation`. The
        // responder surfaces a *hint* (advisory) that the runner can
        // ignore. This test asserts the hint has no
        // payload-contract-specific reason; the runner contract
        // enforces the no-overwrite rule.
        let mut r = RecoveryResponder::new(cfg_with(1, 1, true));
        r.begin_iteration();
        let env = envelope(
            "k:ralph:*:r:*",
            1,
            DiagnosisSeverity::Error,
            false,
            None,
            DiagnosisSource::StallRecovery,
        );
        let _ = r.record_finding(&env, 1);
        let hint = r.take_termination_hint();
        assert!(hint.is_some());
        let hint = hint.unwrap();
        assert!(
            !hint.reason.contains("payload_contract"),
            "hint reason must not introduce a new termination reason: {}",
            hint.reason
        );
    }

    #[test]
    fn target_hat_for_retry_returns_none_when_no_safe_target() {
        let mut r = RecoveryResponder::new(cfg_with(2, 5, true));
        r.begin_iteration();
        let env = envelope(
            "k:ralph:*:r:*",
            1,
            DiagnosisSeverity::Error,
            false,
            Some("ralph"),
            DiagnosisSource::StallRecovery,
        );
        let _ = r.record_finding(&env, 1);
        assert!(r.target_hat_for_retry("k:ralph:*:r:*").is_none());
    }

    // === P0-1 (plan 2026-06-29-006) attempt-count merging ===
    //
    // `stall_recovery` and `missing_event_gate` on the same
    // (hat, topic) diagnose the same root cause (consumer
    // did not emit within window). Before U6, the two retry
    // keys had independent attempt counters — each could
    // cross the threshold alone, prematurely triggering
    // `EscalationLevel::Final`. The P0-1 fix folds the
    // sibling attempt_count into `classify` so the two paths
    // cannot blow past the threshold independently.

    #[test]
    fn p0_1_sibling_lookup_maps_stall_to_missing_event() {
        let r = RecoveryResponder::new(cfg_with(3, 5, true));
        let (prefix, suffix) = r
            .sibling_lookup("stall_recovery:executor:work.done:handoff_dispatch_timeout:*")
            .expect("stall_recovery must have a sibling key shape");
        assert_eq!(
            prefix,
            "missing_event_gate:executor:work.done:missing_event:"
        );
        assert_eq!(suffix, "*");
    }

    #[test]
    fn p0_1_sibling_lookup_maps_missing_event_to_stall() {
        let r = RecoveryResponder::new(cfg_with(3, 5, true));
        let (prefix, suffix) = r
            .sibling_lookup("missing_event_gate:executor:work.done:missing_event:*")
            .expect("missing_event_gate must have a sibling key shape");
        assert_eq!(
            prefix,
            "stall_recovery:executor:work.done:handoff_dispatch_timeout:"
        );
        assert_eq!(suffix, "*");
    }

    #[test]
    fn p0_1_sibling_lookup_returns_none_for_unrelated_keys() {
        let r = RecoveryResponder::new(cfg_with(3, 5, true));
        // A non-stall/non-missing key must not be folded.
        assert!(
            r.sibling_lookup("execution_contract:executor:r:*")
                .is_none()
        );
        assert!(r.sibling_lookup("drift_monitor:ralph:r:*").is_none());
    }

    #[test]
    fn p0_1_classify_takes_max_of_self_and_sibling_attempts() {
        // Setup: 2 attempts on `stall_recovery` and 1 on
        // `missing_event_gate`; threshold = 2; window = 5.
        // Without P0-1 the `missing_event_gate` classify would
        // see attempts=1 (under threshold → Soft). With P0-1
        // the sibling stall_recovery attempts=2 are folded in,
        // so `missing_event_gate` classify now also sees
        // attempts=2 and crosses the threshold.
        let mut r = RecoveryResponder::new(cfg_with(2, 5, true));
        r.begin_iteration();
        let stall = envelope(
            "stall_recovery:executor:work.done:handoff_dispatch_timeout:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("executor"),
            DiagnosisSource::StallRecovery,
        );
        // The first record_finding runs classify on the stall
        // key with attempts=1 (under threshold → Soft) — no
        // state mutation that would affect the miss key.
        let _ = r.record_finding(&stall, 1);
        r.begin_iteration();
        // Second record on stall: attempts=2, threshold=2,
        // classify returns Hard and marks the stall key
        // escalated. The miss key has not been touched yet.
        let _ = r.record_finding(&stall, 2);

        // Now drive a missing_event_gate envelope on a fresh
        // iteration so the internal classify gets called on
        // the miss key.
        r.begin_iteration();
        let miss = envelope(
            "missing_event_gate:executor:work.done:missing_event:*",
            4,
            DiagnosisSeverity::Warning,
            true,
            Some("executor"),
            DiagnosisSource::MissingEventGate,
        );
        // `record_finding` invokes classify internally — the
        // P0-1 sibling fold must make it return Hard (the miss
        // key itself has attempts=1, but the stall sibling has
        // attempts=2, merged=2 ≥ threshold=2).
        let decision = r.record_finding(&miss, 4);
        assert_eq!(
            decision.level,
            EscalationLevel::Hard,
            "P0-1: missing_event_gate must inherit stall_recovery attempts and escalate"
        );
    }

    #[test]
    fn p0_1_classify_does_not_inherit_when_sibling_absent() {
        // Without any stall_recovery entry, the missing_event_gate
        // attempt counter is independent — verifies the merge is
        // a true sibling fold (not a global multiplier).
        let mut r = RecoveryResponder::new(cfg_with(2, 5, true));
        r.begin_iteration();
        let miss = envelope(
            "missing_event_gate:executor:work.done:missing_event:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("executor"),
            DiagnosisSource::MissingEventGate,
        );
        let decision = r.record_finding(&miss, 1);
        // Only 1 attempt on the key itself, threshold = 2, so
        // classify must return Soft regardless of the merge.
        assert_eq!(
            decision.level,
            EscalationLevel::Soft,
            "P0-1: without a sibling, attempts must not be over-merged"
        );
    }

    #[test]
    fn p0_1_merge_sibling_attempts_returns_max() {
        // Pinned contract: `merge_sibling_attempts` must return
        // the maximum attempt_count across the key and its
        // sibling. This is the underlying primitive the
        // `classify` change relies on.
        let mut r = RecoveryResponder::new(cfg_with(2, 5, true));
        r.begin_iteration();
        let stall = envelope(
            "stall_recovery:executor:work.done:handoff_dispatch_timeout:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("executor"),
            DiagnosisSource::StallRecovery,
        );
        let _ = r.record_finding(&stall, 1);
        r.begin_iteration();
        let _ = r.record_finding(&stall, 2);
        // Now ask the responder directly: from the
        // missing_event_gate side, what is the merged attempt
        // count? Must be 2 (the stall sibling).
        let merged =
            r.merge_sibling_attempts("missing_event_gate:executor:work.done:missing_event:*");
        assert_eq!(
            merged, 2,
            "P0-1: merge_sibling_attempts must return the stall_recovery attempt_count of 2"
        );
    }

    #[test]
    fn outcome_history_caps_at_window_and_clears_on_reset() {
        // Drive observe() past OUTCOME_HISTORY_CAP and assert the
        // oldest entries drop off — the runtime-recovery flapping
        // detector relies on this bound to stay memory-safe in
        // long loops.
        let mut r = RecoveryResponder::new(cfg_with(20, 50, false));
        // 9 observations within the retry window: history should hold
        // exactly the last 8.
        for i in 1..=9_u32 {
            r.begin_iteration();
            let env = envelope(
                "k:builder:work_done:r:*",
                i,
                DiagnosisSeverity::Warning,
                true,
                Some("builder"),
                DiagnosisSource::MissingEventGate,
            );
            let _ = r.record_finding(&env, i);
        }
        let snap = r.outcome_history_snapshot("k:builder:work_done:r:*");
        assert_eq!(snap.len(), OUTCOME_HISTORY_CAP);
        // Stale-window reset: when the gap between first observation
        // and current exceeds the configured window, the next
        // observation must clear the history.
        let mut r2 = RecoveryResponder::new(cfg_with(20, 5, false));
        for i in 1..=4_u32 {
            r2.begin_iteration();
            let env = envelope(
                "k:builder:work_done:r:*",
                i,
                DiagnosisSeverity::Warning,
                true,
                Some("builder"),
                DiagnosisSource::MissingEventGate,
            );
            let _ = r2.record_finding(&env, i);
        }
        assert_eq!(
            r2.outcome_history_snapshot("k:builder:work_done:r:*").len(),
            4
        );
        // 6 iterations later (window=5) the entry must reset.
        r2.begin_iteration();
        let env = envelope(
            "k:builder:work_done:r:*",
            10,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let _ = r2.record_finding(&env, 10);
        let snap2 = r2.outcome_history_snapshot("k:builder:work_done:r:*");
        assert_eq!(
            snap2.len(),
            1,
            "stale-window reset must clear the history before pushing the new outcome"
        );
    }

    #[test]
    fn outcome_history_drives_runtime_recovery_flapping_detector() {
        // End-to-end: drive the responder through a flip-flapping
        // pattern (Pending → Recovered → Repeated → Recovered →
        // Pending) and assert that the runtime-recovery detector
        // returns ForcePlanBlocked. This is the regression test for
        // the primary-172725 cascade — without outcome_history,
        // finalize_recovery_outcome_on_flapping never fires.
        let mut r = RecoveryResponder::new(cfg_with(20, 50, false));
        let outcomes = [
            DiagnosisOutcome::Pending,
            DiagnosisOutcome::Recovered,
            DiagnosisOutcome::Repeated,
            DiagnosisOutcome::Recovered,
            DiagnosisOutcome::Pending,
        ];
        for (i, outcome) in outcomes.iter().enumerate() {
            r.begin_iteration();
            let mut env = envelope(
                "k:executor:work_done:missing_event:*",
                i as u32 + 1,
                DiagnosisSeverity::Warning,
                true,
                Some("executor"),
                DiagnosisSource::MissingEventGate,
            );
            env.outcome = *outcome;
            let _ = r.record_finding(&env, i as u32 + 1);
        }
        // Build the RuntimeContext the way runtime_recovery_context
        // does — using the snapshot, not a single element.
        let history_strings: Vec<String> = r
            .outcome_history_snapshot("k:executor:work_done:missing_event:*")
            .into_iter()
            .map(|o| format!("{o:?}"))
            .collect();
        let ctx = crate::recovery_runtime::RuntimeContext {
            current_iteration: 5,
            retry_key_states: vec![crate::recovery_runtime::RetryKeyState {
                retry_key: "k:executor:work_done:missing_event:*".to_string(),
                last_outcome: history_strings.last().cloned().unwrap_or_default(),
                outcome_history: history_strings,
                attempt_count: 5,
            }],
            ..Default::default()
        };
        let actions = crate::recovery_runtime::dispatch(&ctx);
        assert_eq!(
            actions.len(),
            1,
            "flapping detector must fire on real history"
        );
        assert!(
            matches!(&actions[0], crate::recovery_runtime::RecoveryAction::ForcePlanBlocked { reason, .. } if reason.contains("outcome_flapping")),
            "action must be ForcePlanBlocked with outcome_flapping reason, got: {:?}",
            actions[0]
        );
    }
}
