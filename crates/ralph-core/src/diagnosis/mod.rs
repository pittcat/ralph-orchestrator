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

pub(crate) use envelope::normalize_part;
pub use envelope::{
    DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, EvidenceKind, EvidenceRef,
    RecoveryDiagnosisEnvelope, RecoveryDiagnosisEnvelopeBuilder,
};
pub use journal::{DriftJournalEntry, DriftMetric, RecoveryJournalEntry};
pub use reporter::{
    DIAGNOSIS_LEDGER_SCHEMA_VERSION, DIAGNOSE_JSON_SCHEMA_VERSION, DiagnosisReport,
    LedgerReportError, LedgerSummary, RankedFinding, RejectionSummary, Report, ReporterError,
    RootCause, SessionData, SessionSelector, build_report, load_session, read_rejection_records,
    render_diagnosis_report_json, render_diagnosis_report_markdown, render_json, render_markdown,
    report_from_ledger, resolve_session,
};
pub use responder::{
    AcceptedEventEvidence, EscalationDecision, EscalationLevel, RUNTIME_DIAGNOSIS_ALERT_HEADER,
    RecoveryAction, RecoveryResponder, TerminationHint,
};

/// Map the unified validation pipeline stage (U4a/U4b/U4c — plan
/// 2026-06-21-002) to the legacy `DiagnosisSource` string used by
/// the `ralph diagnose` report.  Each stage in the unified
/// `validate_event` pipeline corresponds to a single source so
/// the reporter can group rejection records consistently with
/// the existing `top_findings` table.
///
/// The mapping is intentionally small and exhaustive: every
/// stage in the unified validation pipeline maps to one source
/// string.  Adding a new validation stage requires a new match
/// arm (the `non_exhaustive` signature leaves room for
/// future stages without breaking downstream callers).
///
/// - `origin` → `origin_guard` (was `execution_contract`'s hat
///   boundary check in the legacy U3 path; the unified pipeline
///   promotes it to its own stage).
/// - `publisher` → `event_policy` (the legacy `payload_policy` /
///   `event_policy.validate_event` reorg consolidated the
///   publisher field into one stage).
/// - `policy` → `event_policy` (the U7a legacy stage name;
///   kept for forward-compat with rejection records written
///   before U4a renamed it to `publisher`).
/// - `required_fields` → `engine_required` (the engine gate's
///   `required_fields` check; previously bucketed as
///   `payload_contract`).
/// - `execution_contract` → `execution_contract` (unchanged).
/// - `workflow_guard` → `workflow_guard` (unchanged).
/// - `step_handoff` → `step_handoff_gate` (U5 macro-edge gate).
/// - `hat_handoff` → `hat_handoff_gate` (U5 macro-edge gate).
/// - any other stage → `unknown` (caller should still produce
///   the report, but flag the source as `unknown` so the
///   operator can spot a missing mapping).
#[must_use]
pub fn validation_stage_to_source(stage: &str) -> &'static str {
    match stage {
        "origin" => "origin_guard",
        "publisher" | "policy" => "event_policy",
        "required_fields" => "engine_required",
        "execution_contract" => "execution_contract",
        "workflow_guard" => "workflow_guard",
        "step_handoff" => "step_handoff_gate",
        "hat_handoff" => "hat_handoff_gate",
        _ => "unknown",
    }
}
