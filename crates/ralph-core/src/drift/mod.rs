//! Runtime drift monitor — a *signal source* for the diagnosis layer.
//!
//! The drift module turns the accepted, published event stream into
//! structured [`DriftFinding`]s. It does **not** manipulate the loop,
//! inject `task.resume`, or escalate. The recovery responder (U6) is
//! the only place that converts findings into action.
//!
//! # Three metrics
//!
//! | Metric | Question it answers | Default threshold |
//! |---|---|---|
//! | `field_completeness` | How often is a required field missing on a topic's events? | 0.9 |
//! | `coord_join_rate` | How often is a `from_topic` event followed by its declared `to_topic` event? | 0.6 |
//! | `emit_cadence` | Is the inter-emit interval of a topic drifting above the rolling baseline? | 2σ |
//!
//! All thresholds come from
//! [`crate::config::telemetry::DriftConfig`]. `field_completeness`
//! requires both an `EventPolicyConfig` and an
//! `ExecutionContractsConfig` source to be wired in via
//! [`RequiredFields`] — otherwise the metric is a silent no-op.
//! `coord_join_rate` only inspects declared edges ([`DeclaredEdges`])
//! to avoid inferring topology.
//!
//! # Architecture
//!
//! ```text
//! EventBus::publish
//!       │
//!       ▼
//! DriftObserver (alert.rs)
//!   - panic-safe, non-blocking, bounded try_send
//!       │
//!       ▼
//! std::sync::mpsc channel
//!       │
//!       ▼
//! loop_runner drains channel between iterations
//!       │
//!       ▼
//! DriftDetector::observe(snapshot) -> Vec<DriftFinding>
//!       │
//!       ▼
//! alert::finding_to_envelope / finding_to_journal_entry
//!       │
//!       ▼
//! DiagnosticsCollector::log_drift() / log_recovery() (U3)
//! ```
//!
//! # Rejected events
//!
//! Events rejected by `EventOriginGuard` (unknown source) are
//! dropped by `EventBus::publish` **before** observers run. The
//! drift observer therefore only sees accepted, published events.
//! `event_origin.rs` is the source of truth for what counts as
//! "rejected"; we deliberately do not duplicate that logic here.
//!
//! # Non-regression
//!
//! - The detector is constructed with default `DriftConfig`; with
//!   the default config (`enabled = false`) the detector is
//!   constructible but no observer is registered, so the loop runs
//!   unchanged.
//! - `RALPH_DIAGNOSTICS=1` does **not** auto-enable drift; drift is
//!   config-driven via `telemetry.runtime_diagnosis.enabled`.
//! - The observer must never block `EventBus::publish()`. The
//!   observer uses a bounded `try_send` and an atomic drop counter.
//! - The observer must never panic. `std::panic::catch_unwind`
//!   isolates the projection logic.

pub mod alert;
pub mod detector;
pub mod engine;
pub mod window;

#[cfg(test)]
mod tests;

pub use alert::{
    DriftObserver, finding_to_envelope, finding_to_journal_entry, finding_to_orchestration_event,
};
pub use detector::{
    DeclaredEdges, DriftDetector, DriftFinding, EMIT_CADENCE_MIN_SAMPLES, RequiredFields,
};
pub use engine::{DriftEngine, evidence_from_jsonl_events};
pub use window::{DriftWindow, EventSnapshot};

/// Parse a payload string as a JSON object and return the set of
/// top-level field names.
///
/// Shared by the EventBus observer projection (`alert.rs`) and the
/// loop-runner evidence builder (`engine.rs`) so both layers compute
/// the **same** field set for the `field_completeness` metric. Before
/// this was unified the observer path saw a 0% completeness false
/// positive on string-encoded payloads (e.g. `review.wave.ready`).
///
/// Behaviour:
///
/// - empty / whitespace-only payloads → empty set;
/// - JSON object payload → its top-level keys;
/// - JSON-string payload → re-parse the decoded string once, which
///   unwraps the double-encoded payloads agents emit through the wave
///   path (e.g. `"{\"dimension\":\"x\"}"`);
/// - anything else (prose, numbers, arrays, parse failure) → empty
///   set rather than panicking.
pub(crate) fn parse_json_object_field_set(payload: &str) -> std::collections::BTreeSet<String> {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return std::collections::BTreeSet::new();
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(serde_json::Value::Object(map)) => map.keys().cloned().collect(),
        Ok(serde_json::Value::String(s)) => parse_json_object_field_set(&s),
        _ => std::collections::BTreeSet::new(),
    }
}
