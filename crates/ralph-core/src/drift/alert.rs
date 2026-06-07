//! Drift alert conversion and EventBus observer wiring.
//!
//! Three responsibilities live here:
//!
//! 1. [`finding_to_envelope`] — convert a [`DriftFinding`] into a
//!    [`RecoveryDiagnosisEnvelope`] with `source = DriftMonitor`.
//! 2. [`finding_to_journal_entry`] — convert a [`DriftFinding`] into
//!    a [`DriftJournalEntry`] (the JSONL record type U3 writes to
//!    `drift.jsonl`).
//! 3. [`finding_to_orchestration_event`] — convert a [`DriftFinding`]
//!    into the high-level [`OrchestrationEvent::DriftDetected`] audit
//!    event U3 writes to `orchestration.jsonl`.
//! 4. [`DriftObserver`] — a panic-safe, non-blocking, bounded
//!    EventBus observer that converts each accepted event to an
//!    [`EventSnapshot`] and forwards it to a bounded channel for
//!    the consumer (loop side) to drain.
//!
//! # Why a separate module
//!
//! The drift module is split into three submodules to keep the
//! dependency direction clear: `window` and `detector` are pure
//! data + pure compute; `alert` is the only place that knows about
//! the diagnostic envelope / journal types and the EventBus
//! observer wiring. U6 (the responder) imports this module to
//! build its escalation strategy; the rest of the crate does not.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ralph_proto::Event;

use crate::diagnosis::{
    DiagnosisSeverity, DiagnosisSource, DriftJournalEntry, DriftMetric, EvidenceKind, EvidenceRef,
    RecoveryDiagnosisEnvelope,
};
use crate::diagnostics::OrchestrationEvent;

use super::detector::DriftFinding;
use super::window::EventSnapshot;

/// Convert a [`DriftFinding`] into a [`RecoveryDiagnosisEnvelope`].
///
/// The envelope's `source` is always [`DiagnosisSource::DriftMonitor`]
/// and `safe_target` is always `false` — the drift monitor never
/// picks a target hat; the responder (U6) does.
///
/// `session_id` is plumbed through to the envelope for cross-referencing
/// the recovery journal line with the diagnostics session.
#[must_use]
pub fn finding_to_envelope(
    finding: &DriftFinding,
    session_id: Option<String>,
) -> RecoveryDiagnosisEnvelope {
    let reason_code = finding.metric.reason_code();
    let expected_action = "investigate payload or workflow drift; consider runtime guidance";
    let mut builder = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::DriftMonitor)
        .severity(finding.severity)
        .iteration(finding.iteration)
        .reason_code(reason_code)
        .message(&finding.message)
        .expected_action(expected_action)
        .safe_target(false)
        .outcome(crate::diagnosis::DiagnosisOutcome::Pending);
    if let Some(session) = session_id {
        builder = builder.session_id(session);
    }
    if let Some(topic) = &finding.topic {
        builder = builder.topic(topic.clone());
    }
    if let Some(field) = &finding.field {
        builder = builder.evidence(EvidenceRef {
            kind: EvidenceKind::Field,
            ref_path: field.clone(),
            snippet: Some(format!(
                "observed={:.3}, threshold={:.3}",
                finding.observed_value, finding.threshold
            )),
        });
    } else {
        // The dedup key for drift findings without a field still
        // needs an evidence ref so the retry_key derivation picks
        // the topic/from/to slot, not a field slot. Use the topic
        // (or the from/to pair) as a Topic evidence ref.
        let (ref_path, kind) = topic_or_edge_ref(finding);
        if !ref_path.is_empty() {
            builder = builder.evidence(EvidenceRef {
                kind,
                ref_path,
                snippet: Some(format!(
                    "observed={:.3}, threshold={:.3}",
                    finding.observed_value, finding.threshold
                )),
            });
        }
    }
    builder = builder.retry_key(make_retry_key(finding));
    builder.build()
}

/// Convert a [`DriftFinding`] into a [`DriftJournalEntry`] ready to
/// be appended to `drift.jsonl`.
#[must_use]
pub fn finding_to_journal_entry(finding: &DriftFinding) -> DriftJournalEntry {
    let mut builder = DriftJournalEntry::builder()
        .finding_id(&finding.finding_id)
        .metric(finding.metric)
        .observed_value(finding.observed_value)
        .threshold(finding.threshold)
        .severity(finding.severity)
        .window_iterations(finding.iteration)
        .iteration(finding.iteration)
        .message(&finding.message);
    if let Some(topic) = &finding.topic {
        builder = builder.topic(topic.clone());
    }
    if let Some(field) = &finding.field {
        builder = builder.field(field.clone());
    }
    if let Some(from) = &finding.from_topic {
        builder = builder.from_topic(from.clone());
    }
    if let Some(to) = &finding.to_topic {
        builder = builder.to_topic(to.clone());
    }
    builder.build()
}

/// Convert a [`DriftFinding`] into the high-level
/// [`OrchestrationEvent::DriftDetected`] audit event.
#[must_use]
pub fn finding_to_orchestration_event(finding: &DriftFinding) -> OrchestrationEvent {
    OrchestrationEvent::DriftDetected {
        finding_id: finding.finding_id.clone(),
        metric: finding.metric.as_str().to_string(),
        topic: finding.topic.clone(),
        field: finding.field.clone(),
        severity: finding.severity.as_str().to_string(),
    }
}

/// Build the drift `retry_key` string from a finding.
///
/// Format: `drift_monitor:<metric>:<topic_or_*>:<field_or_*>:<from_or_*>:<to_or_*>`.
/// The trailing fields mirror the dedup tuple so that the same
/// `(metric, topic, field, from_topic, to_topic)` collapses to one
/// retry key — which the responder can then aggregate across
/// iterations.
fn make_retry_key(finding: &DriftFinding) -> String {
    let topic = finding.topic.as_deref().unwrap_or("*");
    let field = finding.field.as_deref().unwrap_or("*");
    let from = finding.from_topic.as_deref().unwrap_or("*");
    let to = finding.to_topic.as_deref().unwrap_or("*");
    format!(
        "drift_monitor:{}:{}:{}:{}:{}",
        finding.metric.reason_code(),
        topic,
        field,
        from,
        to
    )
}

/// Pick the topic-or-edge evidence ref kind for a finding that has
/// no `field`.
fn topic_or_edge_ref(finding: &DriftFinding) -> (String, EvidenceKind) {
    if let Some(from) = &finding.from_topic {
        if let Some(to) = &finding.to_topic {
            return (format!("{from}->{to}"), EvidenceKind::Topic);
        }
    }
    if let Some(topic) = &finding.topic {
        return (topic.clone(), EvidenceKind::Topic);
    }
    (String::new(), EvidenceKind::Other)
}

// ── EventBus observer wiring ───────────────────────────────────────

/// Bounded, non-blocking, panic-safe observer for `EventBus` that
/// converts each accepted event into an [`EventSnapshot`] and
/// forwards it over a bounded channel.
///
/// # Why this struct lives here
///
/// The drift plan calls out a critical non-regression invariant:
///
/// > EventBus observer is sync and has no panic isolation — drift
/// > observer blocking or panic would break the event routing main
/// > path.
///
/// [`DriftObserver`] satisfies the invariant in three ways:
///
/// 1. **Bounded + non-blocking:** the underlying channel is a
///    `crossbeam`-style bounded queue. When full, the observer
///    drops the event and increments `dropped_events` instead of
///    blocking the publish path. We use `std::sync::mpsc` (which is
///    in `core`) and wrap it in a tiny bounded adapter. The channel
///    capacity is configurable; the default is 256.
/// 2. **Panic-safe:** the public closure installed on
///    `EventBus::add_observer` catches any panic the projection
///    code might raise and silently increments `panicked`. The
///    publish path is never affected.
/// 3. **Rejected events are not observed:** the
///    `EventOriginGuard`-rejected events are dropped by
///    `EventBus::publish` *before* the observer closure runs. See
///    `EventBus::publish` for the source-guard check. We rely on
///    that fact; the observer never inspects the rejection path.
pub struct DriftObserver {
    /// Sender side of the bounded channel.
    sender: std::sync::mpsc::SyncSender<EventSnapshot>,
    /// Receiver side, owned by the consumer.
    receiver: std::sync::mpsc::Receiver<EventSnapshot>,
    /// Number of events dropped because the channel was full.
    dropped: Arc<AtomicU64>,
    /// Number of times the projection code panicked.
    panicked: Arc<AtomicU64>,
}

impl DriftObserver {
    /// Create a new observer with a bounded channel of `capacity`
    /// snapshots. Capacity must be greater than zero; `0` is
    /// silently bumped to 1 to keep the channel constructor happy.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        let (tx, rx) = std::sync::mpsc::sync_channel(cap);
        Self {
            sender: tx,
            receiver: rx,
            dropped: Arc::new(AtomicU64::new(0)),
            panicked: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Build the closure that the caller should hand to
    /// `EventBus::add_observer`. The closure is `Fn + Send +
    /// 'static`, matching `event_bus::add_observer`'s bounds.
    ///
    /// The closure is panic-safe: any panic from the projection
    /// logic is caught with `std::panic::catch_unwind` and
    /// translated into an atomic counter bump on `panicked`. The
    /// `EventBus::publish` path therefore never sees a panic from
    /// the drift observer.
    ///
    /// `iteration_fn` returns the current loop iteration. The
    /// observer stamps every snapshot with it.
    pub fn observer_closure<F>(&self, iteration_fn: F) -> impl Fn(&Event) + Send + 'static
    where
        F: Fn() -> u32 + Send + Sync + 'static,
    {
        let sender = self.sender.clone();
        let dropped = Arc::clone(&self.dropped);
        let panicked = Arc::clone(&self.panicked);
        let iteration_fn = Arc::new(iteration_fn);
        move |event: &Event| {
            // The drift observer closure is allowed to fail. We
            // catch the panic at the boundary so the publish path
            // never sees one. `AssertUnwindSafe` is sound here
            // because we don't expose any half-built state back to
            // the caller; we only mutate atomics.
            let snapshot: EventSnapshot =
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    project_event_to_snapshot(event, iteration_fn())
                })) {
                    Ok(snap) => snap,
                    Err(_) => {
                        panicked.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                };
            if sender.try_send(snapshot).is_err() {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Read the number of dropped events. Updated atomically; safe
    /// to read from any thread.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Read the number of projection panics. Updated atomically;
    /// safe to read from any thread.
    #[must_use]
    pub fn panicked(&self) -> u64 {
        self.panicked.load(Ordering::Relaxed)
    }

    /// Drain at most `max` pending snapshots from the consumer
    /// side. Returns the snapshots in FIFO order.
    pub fn drain(&self, max: usize) -> Vec<EventSnapshot> {
        let mut out = Vec::with_capacity(max);
        while out.len() < max {
            match self.receiver.try_recv() {
                Ok(snap) => out.push(snap),
                Err(_) => break,
            }
        }
        out
    }

    /// Non-blocking receive of a single snapshot, if available.
    pub fn try_recv(&self) -> Option<EventSnapshot> {
        self.receiver.try_recv().ok()
    }
}

/// Project a raw `Event` into the lightweight `EventSnapshot` the
/// detector consumes.
///
/// The projection is intentionally lossy:
///
/// - `topic` is taken verbatim.
/// - `source_hat` is the event source's `as_str()`.
/// - `fields` is the top-level field names of a JSON-object
///   payload. Non-JSON payloads yield an empty set; the detector
///   treats those as "no field evidence" rather than panicking.
/// - `wave_id` carries through when the event is part of a wave.
/// - `iteration` and `timestamp` come from the caller.
fn project_event_to_snapshot(event: &Event, iteration: u32) -> EventSnapshot {
    let mut snap = EventSnapshot::new(event.topic.as_str(), iteration, chrono::Utc::now());
    if let Some(source) = &event.source {
        snap = snap.with_source_hat(source.as_str());
    }
    if let Some(wid) = &event.wave_id {
        snap = snap.with_wave_id(wid.clone());
    }
    // Project payload field names. The detector only needs the
    // top-level field set; nested structure is ignored to keep the
    // window cheap. We tolerate any payload that is not a JSON
    // object: the resulting `fields` is empty, and the
    // field_completeness metric becomes a no-op for that event.
    let fields = parse_json_fields(&event.payload);
    if !fields.is_empty() {
        snap = snap.with_fields(fields);
    }
    snap
}

/// Parse a payload string as a JSON object and return the set of
/// top-level field names. Returns an empty set for any non-object
/// payload or parse failure.
fn parse_json_fields(payload: &str) -> std::collections::BTreeSet<String> {
    use std::collections::BTreeSet;
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return BTreeSet::new();
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(serde_json::Value::Object(map)) => map.keys().cloned().collect(),
        // String-encoded JSON: try once more to unwrap a quoted
        // JSON object, e.g. `"{\"a\":1}"`.
        Ok(serde_json::Value::String(s)) => parse_json_fields(&s),
        _ => BTreeSet::new(),
    }
}

// Severity label used by U3's `OrchestrationEvent::DriftDetected`.
#[allow(dead_code)]
fn severity_label(s: DiagnosisSeverity) -> &'static str {
    s.as_str()
}

// Drift metric label used by the conversion helpers; kept here so
// the alert module owns the metric → string conversion path
// alongside the retry_key builder.
#[allow(dead_code)]
fn metric_label(m: DriftMetric) -> &'static str {
    m.as_str()
}
