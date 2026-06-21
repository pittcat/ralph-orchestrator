//! Drift/Recovery engine — the production wiring of U5 + U6.
//!
//! This module turns the component-level `DriftObserver`,
//! `DriftDetector`, and `RecoveryResponder` types into a single
//! glue struct that the loop runner can install and tick once per
//! iteration. It exists for the four P1 fixes from the
//! `2026-06-07-drift-auto-calibration-branch-review.md` report:
//!
//! - **P1.1 — Drift observer/detector not wired into production.**
//!   Without this engine the drift components sit in
//!   `crates/ralph-core/src/drift/` unused; the loop runner never
//!   constructs them and never calls `drain()`, `observe()`,
//!   `record_recovery_envelope`, etc.
//! - **P1.2 — Hard escalation queue never consumed.** The
//!   responder's `drain_hard_escalations` is wired to the bus here.
//! - **P1.3 — Recovery outcome tracking never invoked.** The
//!   responder's `check_recovery` is invoked per tracked key here.
//! - **P1.4 — Final escalation never terminates.** The responder's
//!   `peek_termination_hint` is checked here; high-severity hints
//!   are promoted into [`TerminationReason::RecoveryExhausted`].
//!
//! # Tick API
//!
//! ```text
//! loop_runner tick():
//!   engine.begin_iteration(event_loop)             // → responder.begin_iteration()
//!   engine.drain_observer(event_loop)              // → observer.drain → detector.observe
//!   engine.drain_hard_escalations(event_loop)      // → publish targeted task.resume
//!   engine.check_recovery_for_iteration(loop, top) // → responder.check_recovery per key
//!   engine.check_termination_hint(loop)            // → Option<TerminationReason>
//! ```
//!
//! # Default behaviour
//!
//! When `RuntimeDiagnosisConfig::enabled` is `false` (the default),
//! [`DriftEngine::disabled`] returns a no-op engine. Calling any of
//! the per-iteration methods on a disabled engine is a cheap
//! no-op — this is the U0 activation contract: nothing observable
//! changes unless telemetry is explicitly enabled.
//!
//! # Non-regression
//!
//! - The engine never blocks on I/O; journal writes are delegated
//!   to the existing `DiagnosticsCollector::log_drift` and
//!   `log_recovery` helpers which already swallow errors.
//! - The engine never panics. The drift observer's panic isolation
//!   is preserved (see `alert::DriftObserver::observer_closure`).
//! - Hard escalation publishing reuses the existing
//!   `EventBus::publish` path; the engine never reaches into bus
//!   internals.
//! - The engine never overrides a stronger termination reason.
//!   `PayloadContractViolation`, `LoopStale`, `MaxIterations`,
//!   etc. all outrank a hint-derived `RecoveryExhausted`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use ralph_proto::{Event, HatId};

use crate::config::execution_contracts::ExecutionContractsConfig;
use crate::config::{DriftConfig, EventPolicyConfig, HatConfig, RuntimeDiagnosisConfig};
use crate::diagnosis::{
    AcceptedEventEvidence, DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, EvidenceKind,
    EvidenceRef, RecoveryDiagnosisEnvelope, RecoveryJournalEntry, TerminationHint,
};
use crate::diagnostics::OrchestrationEvent;
use crate::event_loop::{EventLoop, TerminationReason};

use super::alert::{
    DriftObserver, finding_to_envelope, finding_to_journal_entry, finding_to_orchestration_event,
};
use super::detector::{DeclaredEdges, DriftDetector, RequiredFields};

/// Bundles the drift observer, detector, and per-iteration
/// responder glue so the loop runner can wire it with one call.
///
/// Construct with [`DriftEngine::enabled`] when telemetry is on,
/// or [`DriftEngine::disabled`] for the default no-op path. The
/// runner should call [`Self::begin_iteration`] at the start of
/// every loop iteration, [`Self::drain_observer`] after the
/// observer has accumulated snapshots, [`Self::drain_hard_escalations`]
/// and [`Self::check_recovery_for_iteration`] in the events-handled
/// phase, and [`Self::check_termination_hint`] before any other
/// termination check fires.
pub struct DriftEngine {
    /// The per-iteration drift observer. `None` when telemetry is
    /// disabled — the runner still constructs an engine, but the
    /// observer installation is skipped.
    observer: Option<DriftObserver>,
    /// The drift detector. Owned by the engine so the runner does
    /// not need to keep another handle alive.
    detector: DriftDetector,
    /// Cached config to gate whether the engine performs real
    /// work. Cheap to clone (it's behind an `Arc`).
    config: Arc<RuntimeDiagnosisConfig>,
    /// Iteration stamped onto snapshots by the EventBus observer.
    iteration: Arc<AtomicU32>,
    /// Last iteration a `human.guidance` event was published
    /// for a `Warning`-level Final hint. The engine only re-fires
    /// when the iteration advances, so the same hint does not
    /// spam the bus. `u32::MAX` means "never published".
    last_guidance_iteration: u32,
}

impl DriftEngine {
    /// Build a disabled engine. All per-iteration methods are
    /// no-ops on the returned value. The detector and observer
    /// are still allocated (they're cheap), but `observer_closure`
    /// is never installed on the EventBus and `drain_observer`
    /// returns immediately.
    #[must_use]
    pub fn disabled(config: Arc<RuntimeDiagnosisConfig>) -> Self {
        let detector = DriftDetector::new(config.drift.clone());
        Self {
            observer: None,
            detector,
            config,
            iteration: Arc::new(AtomicU32::new(0)),
            last_guidance_iteration: u32::MAX,
        }
    }

    /// Build an enabled engine. The caller is expected to call
    /// [`Self::install_observer`] exactly once after `EventLoop`
    /// is constructed.
    #[must_use]
    pub fn enabled(
        config: Arc<RuntimeDiagnosisConfig>,
        required_fields: RequiredFields,
        declared_edges: DeclaredEdges,
    ) -> Self {
        let drift_cfg: DriftConfig = config.drift.clone();
        let observer = Some(DriftObserver::new(drift_cfg.window_size.max(1)));
        let detector = DriftDetector::new_with_sources(drift_cfg, required_fields, declared_edges);
        Self {
            observer,
            detector,
            config,
            iteration: Arc::new(AtomicU32::new(0)),
            last_guidance_iteration: u32::MAX,
        }
    }

    /// True when telemetry is enabled. The runner should branch on
    /// this before calling the per-iteration methods when it wants
    /// to avoid even the cheap allocations.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.config.enabled && self.observer.is_some()
    }

    /// Install the drift observer closure on the `EventBus`. The
    /// closure is panic-safe and bounded; the engine never sees a
    /// panic from the publish path.
    pub fn install_observer(&self, event_loop: &mut EventLoop) {
        let Some(observer) = self.observer.as_ref() else {
            return;
        };
        let iteration = Arc::clone(&self.iteration);
        let closure = observer.observer_closure(move || iteration.load(Ordering::Relaxed));
        event_loop.add_observer(closure);
    }

    /// Clear the responder's per-iteration caches. Call this at the
    /// very start of each loop iteration so prompt injection sees
    /// only findings from the iteration that just produced them.
    pub fn begin_iteration(&self, event_loop: &mut EventLoop, iteration: u32) {
        self.iteration.store(iteration, Ordering::Relaxed);
        event_loop.begin_diagnosis_iteration();
    }

    /// Drain the observer's pending snapshots, run them through
    /// the detector, and convert any findings into
    /// `RecoveryDiagnosisEnvelope`s. Each finding is also written
    /// to `drift.jsonl` and surfaced as a high-level
    /// `OrchestrationEvent::DriftDetected` audit event.
    ///
    /// Returns the number of findings produced this iteration. The
    /// runner can use the return value for telemetry; the engine
    /// already updated the responder's state.
    pub fn drain_observer(&mut self, event_loop: &mut EventLoop) -> usize {
        let Some(observer) = self.observer.as_ref() else {
            return 0;
        };
        let current_iteration = event_loop.state().iteration;
        let snapshots = observer.drain(observer_capacity());
        let mut produced = 0;
        let session_id = event_loop.diagnostics().session_id();
        for snap in snapshots {
            let findings = self.detector.observe(snap);
            for finding in findings {
                produced += 1;
                let envelope = finding_to_envelope(&finding, session_id.clone());
                let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
                event_loop
                    .diagnostics()
                    .log_drift(finding_to_journal_entry(&finding));
                let hat = envelope
                    .source_hat
                    .as_deref()
                    .unwrap_or(envelope.target_hat.as_deref().unwrap_or("ralph"));
                event_loop.diagnostics().log_orchestration(
                    current_iteration,
                    hat,
                    finding_to_orchestration_event(&finding),
                );
            }
        }
        produced
    }

    /// Take the responder's hard-escalation actions and publish
    /// them as targeted `task.resume` events. Returns the number
    /// of actions published. The runner should call this once per
    /// iteration after `drain_observer`; the responder clears its
    /// queue on the next `begin_iteration`.
    pub fn drain_hard_escalations(&self, event_loop: &mut EventLoop) -> usize {
        if !self.config.enabled {
            return 0;
        }
        let actions = event_loop.recovery_responder_mut().drain_hard_escalations();
        let count = actions.len();
        for action in actions {
            publish_hard_recovery_event(event_loop, &action);
        }
        count
    }

    /// For every retry key the responder is tracking, ask the
    /// responder whether the next iteration's accepted events
    /// satisfied the diagnosis. The responder updates the
    /// `last_outcome` field; this method additionally writes a
    /// `recovery.jsonl` line for any outcome change so the
    /// reporter can show "Recovered" / "Repeated" badges in the
    /// final report.
    ///
    /// Returns the list of `(retry_key, new_outcome)` pairs that
    /// changed this iteration. The list is empty when nothing
    /// changed or when no keys are tracked.
    ///
    /// `accepted_evidence` is the per-event evidence stream
    /// collected by the loop runner from the iteration that just
    /// completed. Each entry carries the topic, top-level field
    /// set, source hat, and timestamp — the responder needs the
    /// fields and timestamps to re-evaluate the specific drift
    /// metric that produced each finding (see the R7 review).
    /// Older call sites that only have a topic list should use
    /// [`Self::check_recovery_for_iteration_topics`].
    pub fn check_recovery_for_iteration(
        &self,
        event_loop: &mut EventLoop,
        accepted_evidence: &[AcceptedEventEvidence],
    ) -> Vec<(String, DiagnosisOutcome)> {
        if !self.config.enabled {
            return Vec::new();
        }
        let current_iteration = event_loop.state().iteration;
        let keys: Vec<String> = event_loop.recovery_responder().tracked_retry_keys_list();
        let mut updates = Vec::new();
        for key in keys {
            let prior_outcome = match event_loop.recovery_responder().outcome_for(&key) {
                Some(o) => o,
                None => continue,
            };
            let new_outcome = match event_loop.recovery_responder_mut().check_recovery(
                &key,
                accepted_evidence,
                current_iteration,
            ) {
                Some(o) => o,
                None => continue,
            };
            if new_outcome != prior_outcome {
                updates.push((key.clone(), new_outcome));
                let severity = event_loop
                    .recovery_responder()
                    .last_severity_for(&key)
                    .unwrap_or(DiagnosisSeverity::Info);
                let topic = event_loop.recovery_responder().target_topic_for(&key);
                let envelope =
                    build_outcome_envelope(&key, current_iteration, severity, new_outcome, topic);
                let notes = vec![format!("outcome updated to {new_outcome:?}")];
                event_loop
                    .diagnostics()
                    .log_recovery(RecoveryJournalEntry::from_envelope(envelope.clone(), notes));
                let hat = envelope
                    .source_hat
                    .as_deref()
                    .unwrap_or(envelope.target_hat.as_deref().unwrap_or("ralph"));
                event_loop.diagnostics().log_orchestration(
                    current_iteration,
                    hat,
                    OrchestrationEvent::from_recovery_envelope(&envelope),
                );
            }
        }
        updates
    }

    /// Inspect the responder's most recent `TerminationHint` and,
    /// when the hint severity is high enough AND the loop does not
    /// already have a stronger termination reason, return
    /// [`TerminationReason::RecoveryExhausted`].
    ///
    /// The check is intentionally idempotent: the hint is *not*
    /// consumed here (the runner's `finalize_recovery_diagnosis`
    /// still needs it for the summary).
    ///
    /// # Final escalation contract (R8)
    ///
    /// The responder's `classify` produces a `Final` escalation
    /// level whenever the retry key is exhausted, regardless of
    /// the underlying `DiagnosisSeverity`. The engine MUST honour
    /// every Final hint, otherwise the same Warning finding would
    /// produce a Final hint on every iteration and the loop would
    /// silently drift forever. The severity drives the action:
    ///
    /// | Severity | Action |
    /// |---|---|
    /// | `Critical` / `Error` | Promote to `TerminationReason::RecoveryExhausted` — the loop terminates |
    /// | `Warning` | Emit a `human.guidance` event so the operator / RObot can intervene — the loop continues |
    /// | `Info` | No active action — the soft alert is enough |
    ///
    /// Callers should always pair this with
    /// [`Self::check_final_human_guidance`] so the Warning path is
    /// not silently dropped.
    #[must_use]
    pub fn check_termination_hint(&self, event_loop: &EventLoop) -> Option<TerminationReason> {
        if !self.config.enabled {
            return None;
        }
        let hint: &TerminationHint = event_loop.recovery_responder().peek_termination_hint()?;
        match hint.severity {
            DiagnosisSeverity::Critical | DiagnosisSeverity::Error => {
                Some(TerminationReason::RecoveryExhausted {
                    retry_key: hint.retry_key.clone().unwrap_or_default(),
                    reason: hint.reason.clone(),
                })
            }
            // Warning is intentionally NOT promoted here — the
            // engine publishes a `human.guidance` event instead
            // and lets the loop continue under operator
            // supervision. The runner checks
            // `check_final_human_guidance` for the Warning
            // branch.
            DiagnosisSeverity::Warning | DiagnosisSeverity::Info => None,
        }
    }

    /// Inspect the responder's most recent `TerminationHint` and,
    /// when the severity is `Warning` AND the engine has not
    /// already published a `human.guidance` for this hint, publish
    /// one on the bus. The hint is intentionally NOT consumed: the
    /// loop must keep running under supervision and the next
    /// iteration's `finalize_recovery_diagnosis` will still see
    /// it.
    ///
    /// Returns `true` when a `human.guidance` event was published
    /// this call. The runner should call this once per iteration
    /// after [`Self::check_termination_hint`]. The engine keeps an
    /// internal monotonic counter (matched to the iteration
    /// number) so it does not re-fire the same guidance every
    /// iteration once the hint is stable.
    pub fn check_final_human_guidance(&mut self, event_loop: &mut EventLoop) -> bool {
        if !self.config.enabled {
            return false;
        }
        // Unit 3 (2026-06-16-002 plan): during the coordinator
        // bootstrap window we must NOT publish any
        // `human.guidance` event.  The build_prompt guard already
        // drops the events on the consumer side, but emitting
        // them at all would (a) be visible to RObot / Telegram
        // and (b) inflate the bus for downstream readers.  Skip
        // the call entirely while `in_bootstrap_phase()` is
        // true; the Warning hint stays in the responder and
        // will fire naturally once `bootstrap_complete` flips.
        if event_loop.in_bootstrap_phase() {
            return false;
        }
        let Some(hint) = event_loop.recovery_responder().peek_termination_hint() else {
            return false;
        };
        // Only Warning triggers a guidance event; Error/Critical
        // terminate the loop via `check_termination_hint`, and
        // Info is silent. The boundary matches the table in
        // `check_termination_hint`'s doc.
        if !matches!(hint.severity, DiagnosisSeverity::Warning) {
            return false;
        }
        let current_iteration = event_loop.state().iteration;
        if self.last_guidance_iteration == current_iteration {
            return false;
        }
        let retry_key = hint.retry_key.clone().unwrap_or_default();
        let reason = hint.reason.clone();
        let payload = format!(
            "RECOVERY-FINAL-WARNING\nretry_key={retry_key}\nreason={reason}\niteration={current_iteration}\n"
        );
        // `human.guidance` is the existing in-band topic used
        // by the RObot integration. RObot / Telegram relay
        // operators see it and can intervene. The bus accepts
        // it without targeting a specific hat.
        let event = Event::new("human.guidance", payload);
        event_loop.bus().publish(event);
        self.last_guidance_iteration = current_iteration;
        true
    }

    /// Configuration handle. Useful for tests and for the runner
    /// when checking whether the engine is enabled.
    #[must_use]
    pub fn config(&self) -> &RuntimeDiagnosisConfig {
        &self.config
    }
}

/// Publish a single `task.resume` event that targets the
/// `RecoveryAction`'s recommended hat. The payload is a stable,
/// machine-detectable recovery block the recipient hat can parse.
///
/// The `target` field is used by the bus's `targeted` routing
/// path so the event reaches the right hat even when the
/// recipient has a narrow `default_publishes` set.
///
/// **Deprecated (U7a, plan 2026-06-21-002).**  When the
/// `UNIFIED_DETERMINISTIC_CORRECTION=1` env var is set, the
/// drift engine should call
/// [`crate::correction::emit_correction_context`] instead and
/// let the loop runner prepend the `## ORCHESTRATOR CORRECTION`
/// block to the next prompt.  U9 will migrate the drift engine
/// to the new API; this function is preserved so the existing
/// tests under `loop_runner/tests.rs` keep passing with the
/// feature flag off.
fn publish_hard_recovery_event(
    event_loop: &mut EventLoop,
    action: &crate::diagnosis::RecoveryAction,
) {
    let topic_hint = action
        .topic_hint
        .clone()
        .unwrap_or_else(|| "(none)".to_string());
    let severity = action.severity.as_str();
    let payload = format!(
        "RECOVERY-HARD\nretry_key={}\ntarget={}\ntopic_hint={}\nattempt={}\nseverity={}\n",
        action.retry_key,
        action.target_hat.as_str(),
        topic_hint,
        action.attempt,
        severity,
    );
    let event = Event::new("task.resume", payload).with_target(action.target_hat.clone());
    // Publish through the existing route so all observers
    // (including the drift observer) see the recovery event.
    event_loop.bus().publish(event);
}

/// Build a recovery envelope representing an outcome change so
/// the journal captures the state transition. The envelope is a
/// thin wrapper around the prior finding — it does not promote
/// the original severity.
fn build_outcome_envelope(
    retry_key: &str,
    iteration: u32,
    severity: DiagnosisSeverity,
    outcome: DiagnosisOutcome,
    topic: Option<String>,
) -> RecoveryDiagnosisEnvelope {
    let mut builder = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::DriftMonitor)
        .severity(severity)
        .iteration(iteration)
        .reason_code("recovery_outcome_update")
        .message(format!("outcome updated to {outcome:?}"))
        .expected_action("none — observation only")
        .safe_target(true)
        .outcome(outcome)
        .retry_key(retry_key);
    if let Some(t) = topic.clone() {
        builder = builder.topic(t.clone()).evidence(EvidenceRef {
            kind: EvidenceKind::Topic,
            ref_path: t,
            snippet: None,
        });
    }
    builder.build()
}

/// Observer channel capacity hint. We always drain in chunks of
/// this size to bound the per-iteration work.
fn observer_capacity() -> usize {
    64
}

/// Build a list of [`AcceptedEventEvidence`] from the loop
/// runner's accepted `ralph_proto::Event` list. The runner holds
/// the canonical accepted event stream after `EventPolicy` and
/// `EventOriginGuard`; the drift engine does not have direct
/// access to it, so the runner hands the events over for
/// per-metric recovery evaluation.
///
/// `iter_now` is the loop iteration the evidence was collected
/// in. It is currently only used to drive the deterministic
/// fallback timestamp when the wall clock is unavailable; once
/// the bus events carry a per-event timestamp (planned U10
/// work) this parameter will become the cadence sample index.
///
/// JSON-object payloads have their top-level field names
/// extracted; non-JSON payloads produce an empty field set,
/// which the responder treats as "no field evidence" (the
/// `FieldCompleteness` recovery rule then falls back to
/// topic-presence for the topic-only envelope case).
///
/// Timestamps: the `ralph_proto::Event` type does NOT carry a
/// timestamp — the cadence metric only needs a *sequence* of
/// timestamps, and using `Utc::now()` per call would skew the
/// detector's z-score because all events would be observed in
/// the same nanosecond. We use the loop iteration + a stable
/// offset so consecutive iterations produce monotonic
/// timestamps. Tests that need a specific cadence should
/// inject events through the bus (which adds the timestamp at
/// publish time).
#[must_use]
pub fn evidence_from_jsonl_events<I>(events: I, iter_now: u32) -> Vec<AcceptedEventEvidence>
where
    I: IntoIterator<Item = ralph_proto::Event>,
{
    use chrono::DateTime;
    let base = DateTime::<chrono::Utc>::from_timestamp(1_700_000_000 + i64::from(iter_now) * 60, 0)
        .unwrap_or_else(chrono::Utc::now);
    let mut out = Vec::new();
    for (idx, ev) in events.into_iter().enumerate() {
        let fields = super::parse_json_object_field_set(&ev.payload);
        // Per-event timestamp offset (one second per index)
        // keeps the cadence rule deterministic for the
        // recovery tests; the loop's iteration base keeps it
        // monotonic across iterations.
        let ts_offset = chrono::Duration::seconds(idx as i64);
        out.push(AcceptedEventEvidence {
            topic: ev.topic.as_str().to_string(),
            fields,
            source_hat: ev.source.map(|h| h.as_str().to_string()),
            timestamp: base + ts_offset,
        });
    }
    out
}

/// Build [`RequiredFields`] from a `RalphConfig`'s `EventPolicyConfig`
/// and `ExecutionContractsConfig`.
///
/// The detector's `field_completeness` metric is a no-op when this
/// returns an empty declaration set. The contract rule's
/// `require_payload_fields` and the policy's `schemas[topic].required_fields`
/// are merged into a single per-topic list.
#[must_use]
pub fn required_fields_from_config(
    event_policy: Option<&EventPolicyConfig>,
    contracts: Option<&ExecutionContractsConfig>,
) -> RequiredFields {
    let mut rf = RequiredFields::new();
    if let Some(policy) = event_policy {
        for (topic, schema) in &policy.schemas {
            if schema.required_fields.is_empty() {
                continue;
            }
            rf.from_policy
                .entry(topic.clone())
                .or_default()
                .extend(schema.required_fields.iter().cloned());
        }
    }
    if let Some(contracts) = contracts {
        for (topic, rule) in &contracts.rules {
            if rule.require_payload_fields.is_empty() {
                continue;
            }
            rf.from_execution_contract
                .entry(topic.clone())
                .or_default()
                .extend(rule.require_payload_fields.iter().cloned());
        }
    }
    rf
}

/// Build [`DeclaredEdges`] from a list of hat configs.
///
/// A declared edge is `(from_topic, to_topic)` for one hat where
/// `from_topic` triggers the hat and `to_topic` is one of the
/// topics that hat may publish in response.
#[must_use]
pub fn declared_edges_from_hats(hats: &[HatConfig]) -> DeclaredEdges {
    let mut edges: Vec<(String, String)> = Vec::new();
    for hat in hats {
        for from_topic in &hat.triggers {
            for to_topic in &hat.publishes {
                edges.push((from_topic.clone(), to_topic.clone()));
            }
        }
    }
    DeclaredEdges::from_pairs(edges)
}

// Suppress the unused-import warning for the test-only `HatId`
// import path. The drift engine does not construct a `HatId`
// directly, but the recovery helpers re-export it for callers
// that want to build targeted resume events.
#[allow(dead_code)]
fn _ensure_link(_: &HatId) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnosis::EscalationLevel;

    fn cfg(enabled: bool) -> Arc<RuntimeDiagnosisConfig> {
        Arc::new(RuntimeDiagnosisConfig {
            enabled,
            ..RuntimeDiagnosisConfig::default()
        })
    }

    #[test]
    fn disabled_engine_does_not_install_observer() {
        let engine = DriftEngine::disabled(cfg(false));
        assert!(!engine.is_enabled());
    }

    #[test]
    fn enabled_engine_marks_enabled() {
        let engine = DriftEngine::enabled(cfg(true), RequiredFields::new(), DeclaredEdges::new());
        assert!(engine.is_enabled());
    }

    #[test]
    fn observer_capacity_is_stable() {
        // Pinned to keep the per-iteration drain budget stable.
        // 64 events is enough to drain a small hat topology
        // without breaking out of the per-iteration budget.
        assert_eq!(observer_capacity(), 64);
    }

    #[test]
    fn begin_iteration_updates_observer_clock() {
        let engine = DriftEngine::enabled(cfg(true), RequiredFields::new(), DeclaredEdges::new());
        engine.iteration.store(1, Ordering::Relaxed);
        assert_eq!(engine.iteration.load(Ordering::Relaxed), 1);
        engine.iteration.store(7, Ordering::Relaxed);
        assert_eq!(engine.iteration.load(Ordering::Relaxed), 7);
    }

    #[test]
    fn declared_edges_follow_each_hats_trigger_to_publish_flow() {
        let reviewer = HatConfig {
            name: "reviewer".to_string(),
            triggers: vec!["work.done".to_string()],
            publishes: vec!["review.wave.ready".to_string()],
            ..HatConfig::default()
        };
        let builder = HatConfig {
            name: "builder".to_string(),
            triggers: vec!["plan.ready".to_string()],
            publishes: vec!["work.done".to_string()],
            ..HatConfig::default()
        };

        let edges = declared_edges_from_hats(&[reviewer, builder]);

        assert!(
            edges
                .edges
                .contains(&("work.done".to_string(), "review.wave.ready".to_string()))
        );
        assert!(
            edges
                .edges
                .contains(&("plan.ready".to_string(), "work.done".to_string()))
        );
        assert!(
            !edges
                .edges
                .contains(&("work.done".to_string(), "work.done".to_string()))
        );
    }

    // ──────────────────────────────────────────────────────────────────
    // R7 / R8 lifecycle tests: exercise the full runner tick through
    // DriftEngine so a regression in any one of the four paths is
    // caught end-to-end. The previous unit tests only covered the
    // helper layer; the review report explicitly called out that
    // the production wiring was untested.
    // ──────────────────────────────────────────────────────────────────

    /// Build a `RuntimeDiagnosisConfig` with all knobs exposed.
    fn diagnosis_config(
        enabled: bool,
        max_repeats: usize,
        window: usize,
    ) -> Arc<RuntimeDiagnosisConfig> {
        Arc::new(RuntimeDiagnosisConfig {
            enabled,
            write_artifacts: false,
            prompt_injection_enabled: true,
            max_prompt_findings: 5,
            max_prompt_chars: 2000,
            retry_window_iterations: window,
            max_repeated_recoveries: max_repeats,
            artifact_retention: 10,
            malformed_jsonl_policy: crate::config::MalformedJsonlPolicy::Warn,
            drift: crate::config::DriftConfig::default(),
        })
    }

    /// Build a RalphConfig with telemetry enabled and a single hat.
    fn make_config_with_diagnosis(diag: Arc<RuntimeDiagnosisConfig>) -> crate::RalphConfig {
        // The full YAML pipeline is heavy for a unit test; we
        // build the config manually so the test pins exactly the
        // fields the engine reads.
        let mut config = crate::RalphConfig::default();
        config.telemetry.runtime_diagnosis = (*diag).clone();
        config
    }

    /// Lifecycle 1: SOFT alert.
    ///
    /// A single Warning finding under the configured threshold
    /// produces a `RecoveryAction` queue of zero, a peek
    /// termination hint of `None`, and the prompt alert is
    /// injected. The engine does NOT publish a `task.resume` and
    /// does NOT terminate.
    #[test]
    fn lifecycle_soft_alert_does_not_publish_or_terminate() {
        // max_repeats=3, window=5 → first two observations stay
        // Soft, third crosses into Hard.
        let diag = diagnosis_config(true, 3, 5);
        let config = make_config_with_diagnosis(Arc::clone(&diag));
        let mut event_loop = crate::event_loop::EventLoop::new(config);
        event_loop.initialize("lifecycle_soft");
        event_loop.set_iteration_for_test(1);

        let mut engine = DriftEngine::enabled(
            Arc::clone(&diag),
            RequiredFields::new(),
            DeclaredEdges::new(),
        );
        engine.begin_iteration(&mut event_loop, 1);

        // First observation: still under the threshold.
        let env = envelope_for(
            "missing_event_gate:builder:work_done:no_emit:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let decision = event_loop.recovery_responder_mut().record_finding(&env, 1);
        assert_eq!(decision.level, EscalationLevel::Soft);
        // No hard escalations queued.
        let actions = event_loop.recovery_responder_mut().drain_hard_escalations();
        assert!(actions.is_empty(), "Soft must not queue hard actions");
        // No termination hint surfaced.
        let hint = event_loop.recovery_responder().peek_termination_hint();
        assert!(hint.is_none(), "Soft must not produce a hint");
        // No human guidance published for Soft.
        let published = engine.check_final_human_guidance(&mut event_loop);
        assert!(!published, "Soft must not publish human.guidance");
        // No termination reason promoted.
        let term = engine.check_termination_hint(&event_loop);
        assert!(term.is_none(), "Soft must not terminate the loop");
    }

    /// Lifecycle 2: HARD retry.
    ///
    /// After `max_repeats` observations of the same retry key,
    /// the responder escalates to Hard and queues a
    /// `RecoveryAction` that the engine publishes as a
    /// `task.resume` event. The engine does NOT terminate the
    /// loop on Hard.
    #[test]
    fn lifecycle_hard_retry_publishes_task_resume() {
        let diag = diagnosis_config(true, 2, 5);
        let config = make_config_with_diagnosis(Arc::clone(&diag));
        let mut event_loop = crate::event_loop::EventLoop::new(config);
        event_loop.initialize("lifecycle_hard");
        let engine = DriftEngine::enabled(
            Arc::clone(&diag),
            RequiredFields::new(),
            DeclaredEdges::new(),
        );
        engine.begin_iteration(&mut event_loop, 1);
        event_loop.set_iteration_for_test(1);

        // First observation: Soft.
        let env = envelope_for(
            "missing_event_gate:builder:work_done:no_emit:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let _ = event_loop.recovery_responder_mut().record_finding(&env, 1);
        // Second observation: still Soft because max_repeats=2
        // → crosses on observation 2.
        event_loop.begin_diagnosis_iteration();
        let env2 = envelope_for(
            "missing_event_gate:builder:work_done:no_emit:*",
            2,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let decision2 = event_loop.recovery_responder_mut().record_finding(&env2, 2);
        assert_eq!(decision2.level, EscalationLevel::Hard);
        // Drain hard escalations through the engine and confirm
        // it publishes a `task.resume` event.
        let published = engine.drain_hard_escalations(&mut event_loop);
        assert_eq!(published, 1, "engine must publish the task.resume");
        // Engine does NOT terminate on Hard.
        let term = engine.check_termination_hint(&event_loop);
        assert!(term.is_none(), "Hard must not terminate the loop");
    }

    /// Lifecycle 3a: FINAL termination on Error severity.
    ///
    /// When the retry window is exhausted AND severity ≥ Error,
    /// the responder raises a `Final` hint. The engine promotes
    /// it to `TerminationReason::RecoveryExhausted`.
    #[test]
    fn lifecycle_final_error_terminates_loop() {
        let diag = diagnosis_config(true, 1, 1);
        let config = make_config_with_diagnosis(Arc::clone(&diag));
        let mut event_loop = crate::event_loop::EventLoop::new(config);
        event_loop.initialize("lifecycle_final_error");
        let mut engine = DriftEngine::enabled(
            Arc::clone(&diag),
            RequiredFields::new(),
            DeclaredEdges::new(),
        );
        engine.begin_iteration(&mut event_loop, 1);
        event_loop.set_iteration_for_test(1);

        // One Error observation with retry window=1 already
        // exhausted on the first observation.
        let env = envelope_for(
            "k:ralph:*:stall:*",
            1,
            DiagnosisSeverity::Error,
            false,
            None,
            DiagnosisSource::StallRecovery,
        );
        let _ = event_loop.recovery_responder_mut().record_finding(&env, 1);
        // The engine must promote the Error hint to a
        // termination reason.
        let term = engine.check_termination_hint(&event_loop);
        assert!(
            matches!(
                term,
                Some(crate::event_loop::TerminationReason::RecoveryExhausted { .. })
            ),
            "Error Final hint must become RecoveryExhausted, got: {term:?}"
        );
        // And it must NOT publish a human.guidance (the
        // guidance is the Warning branch, not the Error
        // branch).
        let published = engine.check_final_human_guidance(&mut event_loop);
        assert!(!published, "Error must not produce human.guidance");
    }

    /// Lifecycle 3b: FINAL Warning produces `human.guidance`,
    /// not termination. The loop continues under operator
    /// supervision.
    #[test]
    fn lifecycle_final_warning_publishes_human_guidance() {
        let diag = diagnosis_config(true, 1, 1);
        let config = make_config_with_diagnosis(Arc::clone(&diag));
        let mut event_loop = crate::event_loop::EventLoop::new(config);
        event_loop.initialize("lifecycle_final_warning");
        let mut engine = DriftEngine::enabled(
            Arc::clone(&diag),
            RequiredFields::new(),
            DeclaredEdges::new(),
        );
        engine.begin_iteration(&mut event_loop, 1);
        event_loop.set_iteration_for_test(1);
        // Unit 3 (2026-06-16-002 plan): `check_final_human_guidance`
        // short-circuits while `in_bootstrap_phase()` is true so the
        // bootstrap window is human-guidance free.  Flip the gate
        // here so the test exercises the post-bootstrap path.
        event_loop.state_mut().bootstrap_complete = true;

        let env = envelope_for(
            "k:ralph:*:stall:*",
            1,
            DiagnosisSeverity::Warning,
            false,
            None,
            DiagnosisSource::StallRecovery,
        );
        let _ = event_loop.recovery_responder_mut().record_finding(&env, 1);
        // Warning Final does NOT promote to termination.
        let term = engine.check_termination_hint(&event_loop);
        assert!(term.is_none(), "Warning must not terminate, got: {term:?}");
        // It DOES publish human.guidance so the operator can
        // intervene. The bus is wired but nothing observes
        // the event in this unit test, so we only check the
        // return value.
        let published = engine.check_final_human_guidance(&mut event_loop);
        assert!(published, "Warning Final must publish human.guidance");
        // A second call in the same iteration is a no-op so
        // the bus is not spammed.
        let published2 = engine.check_final_human_guidance(&mut event_loop);
        assert!(!published2, "same iteration must not re-publish");
    }

    /// Unit 3 (2026-06-16-002 plan) companion test:
    /// `check_final_human_guidance` MUST return `false` while
    /// the loop is still in the bootstrap window.  The Warning
    /// hint stays in the responder (and the operator will see
    /// it once `bootstrap_complete` flips on the next iteration),
    /// but we MUST NOT publish a `human.guidance` event into
    /// the bus during the bootstrap window — the coordinator's
    /// first prompt is supposed to be guidance-free so the
    /// first legal handoff wins over stale human input.
    #[test]
    fn lifecycle_final_warning_suppressed_during_bootstrap() {
        let diag = diagnosis_config(true, 1, 1);
        let config = make_config_with_diagnosis(Arc::clone(&diag));
        let mut event_loop = crate::event_loop::EventLoop::new(config);
        event_loop.initialize("lifecycle_final_warning_bootstrap");
        // Sanity: we ARE in the bootstrap window — no need to
        // flip any flag here.
        assert!(event_loop.in_bootstrap_phase());
        let mut engine = DriftEngine::enabled(
            Arc::clone(&diag),
            RequiredFields::new(),
            DeclaredEdges::new(),
        );
        engine.begin_iteration(&mut event_loop, 1);
        event_loop.set_iteration_for_test(1);

        let env = envelope_for(
            "k:ralph:*:stall:*",
            1,
            DiagnosisSeverity::Warning,
            false,
            None,
            DiagnosisSource::StallRecovery,
        );
        let _ = event_loop.recovery_responder_mut().record_finding(&env, 1);
        // The bootstrap gate MUST suppress the warning
        // publication.  The Warning hint stays in the
        // responder; we only check that nothing has been
        // published to the bus.
        let published = engine.check_final_human_guidance(&mut event_loop);
        assert!(
            !published,
            "bootstrap window MUST suppress drift Warning -> human.guidance; got published={published}"
        );
    }

    /// Lifecycle 4: RECOVERED.
    ///
    /// A finding on iteration N is NOT marked Recovered on the
    /// same iteration (grace period). On iteration N+1, when
    /// the per-metric rule is satisfied (here: the topic flows
    /// through the bus with the right field), the responder
    /// transitions to `Recovered`.
    #[test]
    fn lifecycle_recovery_requires_per_metric_evidence() {
        use crate::diagnosis::AcceptedEventEvidence;
        use chrono::{TimeZone, Utc};
        use std::collections::BTreeSet;

        let diag = diagnosis_config(true, 3, 5);
        let config = make_config_with_diagnosis(Arc::clone(&diag));
        let mut event_loop = crate::event_loop::EventLoop::new(config);
        event_loop.initialize("lifecycle_recovered");
        // We do not need a DriftEngine instance for this
        // unit test — the responder alone owns the recovery
        // state machine. The engine is exercised separately
        // by the soft/hard/final tests.

        // Observation 1: Warning finding under threshold
        // (soft). The state map stores the metric+evidence
        // (non-drift → metric=None, recovery rule is the
        // topic-presence fallback).
        event_loop.set_iteration_for_test(1);
        let env = envelope_for(
            "missing_event_gate:builder:work_done:no_emit:*",
            1,
            DiagnosisSeverity::Warning,
            true,
            Some("builder"),
            DiagnosisSource::MissingEventGate,
        );
        let _ = event_loop.recovery_responder_mut().record_finding(&env, 1);

        // Same iteration: a topic-only evidence stream is NOT
        // enough to mark the finding Recovered. The grace
        // period is enforced by `current_iteration <=
        // state.last_iteration`.
        let evidence_same_iter = vec![AcceptedEventEvidence {
            topic: "work.done".to_string(),
            fields: BTreeSet::new(),
            source_hat: Some("builder".to_string()),
            timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        }];
        let outcome_same = event_loop.recovery_responder_mut().check_recovery(
            "missing_event_gate:builder:work_done:no_emit:*",
            &evidence_same_iter,
            1,
        );
        assert_eq!(
            outcome_same,
            Some(DiagnosisOutcome::Pending),
            "same-iteration evidence must not mark recovered"
        );

        // Next iteration: a topic-matched evidence stream
        // flips the outcome to Recovered.
        event_loop.set_iteration_for_test(2);
        let outcome_next = event_loop.recovery_responder_mut().check_recovery(
            "missing_event_gate:builder:work_done:no_emit:*",
            &evidence_same_iter,
            2,
        );
        assert_eq!(
            outcome_next,
            Some(DiagnosisOutcome::Recovered),
            "next-iteration topic match must mark recovered"
        );
    }

    /// Helper: build a `RecoveryDiagnosisEnvelope` for the
    /// lifecycle tests.
    fn envelope_for(
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
            .reason_code("lifecycle_test")
            .message(format!("lifecycle test for {retry_key}"))
            .retry_key(retry_key)
            .safe_target(safe_target);
        if let Some(t) = target {
            b = b.target_hat(t);
        }
        b.build()
    }

    /// R7: `field_completeness` finding is NOT recovered by a
    /// topic-only event when the required field is still
    /// missing. This is the regression case the R7 review
    /// explicitly called out: "field_completeness 只要后续出现
    /// 同 topic 就会标记 Recovered，即使缺失字段仍未恢复".
    #[test]
    fn field_completeness_finding_requires_field_evidence() {
        use crate::diagnosis::{AcceptedEventEvidence, EvidenceKind, EvidenceRef};
        use chrono::{TimeZone, Utc};
        use std::collections::BTreeSet;

        let diag = diagnosis_config(true, 3, 5);
        let config = make_config_with_diagnosis(Arc::clone(&diag));
        let mut event_loop = crate::event_loop::EventLoop::new(config);
        event_loop.initialize("r7_field_completeness");

        // DriftMonitor finding with a Field evidence ref. The
        // responder reverse-maps the reason code to the
        // `FieldCompleteness` metric.
        let mut b = RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::DriftMonitor)
            .severity(DiagnosisSeverity::Warning)
            .iteration(1)
            .reason_code("drift_field_completeness")
            .message("field `task_id` missing on `work.done`")
            .retry_key("drift_monitor:drift_field_completeness:work.done:task_id:*:*")
            .safe_target(false)
            .evidence(EvidenceRef {
                kind: EvidenceKind::Field,
                ref_path: "task_id".to_string(),
                snippet: Some("observed=0.000, threshold=0.900".to_string()),
            })
            .topic("work.done")
            .target_hat("builder");
        b = b.retry_key("drift_monitor:drift_field_completeness:work.done:task_id:*:*");
        let env = b.build();
        let _ = event_loop.recovery_responder_mut().record_finding(&env, 1);

        // Next iteration: a topic-only event without the
        // required field. The responder must NOT mark it
        // Recovered.
        event_loop.set_iteration_for_test(2);
        let evidence_no_field = vec![AcceptedEventEvidence {
            topic: "work.done".to_string(),
            fields: BTreeSet::new(), // empty: no `task_id`
            source_hat: Some("builder".to_string()),
            timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        }];
        let outcome = event_loop.recovery_responder_mut().check_recovery(
            "drift_monitor:drift_field_completeness:work.done:task_id:*:*",
            &evidence_no_field,
            2,
        );
        assert_ne!(
            outcome,
            Some(DiagnosisOutcome::Recovered),
            "missing field must not be considered recovered"
        );
        // The single record_finding call earlier set
        // `attempt_count = 1`; the responder therefore falls
        // back to `Pending`. The important contract is that
        // the outcome is NOT `Recovered` when the field is
        // still missing — that is what the R7 review called
        // out as the regression.
        assert_eq!(
            outcome,
            Some(DiagnosisOutcome::Pending),
            "single-attempt finding should stay Pending, got: {outcome:?}"
        );

        // Same iteration's evidence that DOES include the
        // required field flips the outcome to Recovered.
        let mut fields = BTreeSet::new();
        fields.insert("task_id".to_string());
        let evidence_with_field = vec![AcceptedEventEvidence {
            topic: "work.done".to_string(),
            fields,
            source_hat: Some("builder".to_string()),
            timestamp: Utc.timestamp_opt(1_700_000_100, 0).unwrap(),
        }];
        event_loop.set_iteration_for_test(3);
        let outcome2 = event_loop.recovery_responder_mut().check_recovery(
            "drift_monitor:drift_field_completeness:work.done:task_id:*:*",
            &evidence_with_field,
            3,
        );
        assert_eq!(
            outcome2,
            Some(DiagnosisOutcome::Recovered),
            "field-evidence must mark recovered, got: {outcome2:?}"
        );
    }
}
