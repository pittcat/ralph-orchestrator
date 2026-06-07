//! Runtime diagnosis data model.
//!
//! Defines the shared [`RecoveryDiagnosisEnvelope`] and journal record types
//! that every recovery / drift / report path funnels into. U3 onwards will
//! serialize these types to `.ralph/diagnostics/<session>/recovery.jsonl`
//! and `drift.jsonl`; U7's `ralph diagnose` will read them back to produce
//! operator-facing reports.
//!
//! # Stability rules
//!
//! - The JSON field names produced by these types are part of the public
//!   contract — renaming or repurposing them is a breaking change for
//!   downstream `ralph diagnose` consumers.
//! - All fields are forward-compatible: every optional field carries
//!   `#[serde(default)]`, so older readers can deserialize newer envelopes
//!   without crashing.
//! - The types in this module are pure data. They MUST NOT depend on
//!   `EventBus`, `HatRegistry`, or other runtime types.
//!
//! # Layers
//!
//! - [`envelope`]: `RecoveryDiagnosisEnvelope`, `DiagnosisSource`,
//!   `DiagnosisSeverity`, `DiagnosisOutcome`, `EvidenceRef`, and the
//!   builder.
//! - [`journal`]: `RecoveryJournalEntry` and `DriftJournalEntry` JSONL
//!   record types.
//! - [`responder`]: U6 `RecoveryResponder` — soft alerts, targeted
//!   `task.resume`, and Final escalation. The only place that turns
//!   diagnoses into runtime action.
//!
//! See the 2026-06-04 "Runtime Diagnosis & Recovery Intelligence" plan
//! for context.

mod envelope;
mod journal;
mod reporter;
mod responder;

#[cfg(test)]
mod tests;

pub use envelope::{
    DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, EvidenceKind, EvidenceRef,
    RecoveryDiagnosisEnvelope, RecoveryDiagnosisEnvelopeBuilder,
};
pub use journal::{DriftJournalEntry, DriftMetric, RecoveryJournalEntry};
pub use reporter::{
    DIAGNOSE_JSON_SCHEMA_VERSION, RankedFinding, Report, ReporterError, SessionData,
    SessionSelector, build_report, load_session, render_json, render_markdown, resolve_session,
};
pub use responder::{
    AcceptedEventEvidence, EscalationDecision, EscalationLevel, RUNTIME_DIAGNOSIS_ALERT_HEADER,
    RecoveryAction, RecoveryResponder, TerminationHint,
};
