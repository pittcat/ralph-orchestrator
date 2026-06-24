//! Unified validation pipeline (U4 / U11).
//!
//! Plan ref: U4 of
//! `docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md`.
//!
//! This module is the single home for validation rules that gate
//! agent-emitted events before they touch [`crate::state::StateLedger`].
//! Rules receive a mutable [`ValidationContext`] so stateful rules
//! (e.g. event-policy dedup) can update runtime state while remaining
//! composable.  Read-only rules use [`ValidationContext::snapshot`].
//! Stage names (`origin`, `publisher`, `required_fields`,
//! `execution_contract`, `step_handoff`,
//! `workflow_guard`, `event_policy`) are preserved as
//! `ValidationRule::name()` values so `reason_code` strings remain
//! stable for downstream tooling (`ralph diagnose`, recovery envelopes,
//! JSONL audits).
//!
//! ## Phase split (KTD-3)
//!
//! Rules declare whether they should run *before* or *after* the
//! speculative commit via [`RulePhase`]:
//!
//! - **PreCommit** rules run with the **current** snapshot. They
//!   answer questions that only depend on configuration + the
//!   event itself (origin guard, publisher, required fields,
//!   step-handoff, event-policy).
//! - **PostCommit** rules run with the **projected** snapshot. They
//!   answer questions that need the post-state (execution
//!   contract, workflow guard).
//!
//! [`ValidationPipeline::validate_with_preview`] runs both phases
//! in the right order without committing the post-commit delta;
//! the caller decides whether to finalize based on the returned
//! [`ValidationReport`].

mod context;
mod pipeline;
mod result;
mod rules_event_policy;
mod rules_execution_contract;
mod rules_origin;
mod rules_publisher;
mod rules_required_fields;
mod rules_step_handoff;
mod rules_workflow_guard;

#[cfg(test)]
mod tests;

pub use context::ValidationContext;
pub use pipeline::{RulePhase, ValidationPipeline, ValidationReport, ValidationRule};
pub use result::{
    ReasonCode, RejectionHint, ValidationResult, ValidationStage, WorkflowGuardRejectionDetail,
};
pub use rules_event_policy::EventPolicyRule;
