//! Unified validation pipeline (U4).
//!
//! Plan ref: U4 of
//! `docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md`.
//!
//! This module is the single home for the *stateless* validation rules
//! that gate agent-emitted events before they touch
//! [`crate::state::StateLedger`]. The rules are pure functions:
//!
//! ```text
//! (ProtocolView, LedgerSnapshot, Event) -> ValidationResult
//! ```
//!
//! They share no mutable state — every field they need comes from
//! `ProtocolView` (config SSOT) and `LedgerSnapshot` (state SSOT).
//! Stage names (`origin`, `publisher`, `required_fields`,
//! `execution_contract`, `step_handoff`, `hat_handoff`,
//! `workflow_guard`) are preserved as `ValidationRule::name()`
//! values so `reason_code` strings remain stable for downstream
//! tooling (`ralph diagnose`, recovery envelopes, JSONL audits).
//!
//! ## Phase split (KTD-3)
//!
//! Rules declare whether they should run *before* or *after* the
//! speculative commit via [`RulePhase`]:
//!
//! - **PreCommit** rules run with the **current** snapshot. They
//!   answer questions that only depend on configuration + the
//!   event itself (origin guard, publisher, required fields,
//!   hat-handoff, step-handoff).
//! - **PostCommit** rules run with the **projected** snapshot. They
//!   answer questions that need the post-state (execution
//!   contract, workflow guard).
//!
//! [`ValidationPipeline::validate_with_preview`] runs both phases
//! in the right order without committing the post-commit delta;
//! the caller decides whether to finalize based on the returned
//! [`ValidationReport`].
//!
//! ## Feature flag (KTD-8)
//!
//! The pipeline is **opt-in**. The runtime checks the
//! `UNIFIED_VALIDATION` env var at construction time and falls
//! back to the legacy gate stack when the flag is off. See
//! [`ValidationPipeline::from_config`] for the resolution rule.

mod pipeline;
mod result;
mod rules_execution_contract;
mod rules_hat_handoff;
mod rules_origin;
mod rules_publisher;
mod rules_required_fields;
mod rules_step_handoff;
mod rules_workflow_guard;

#[cfg(test)]
mod tests;

pub use pipeline::{RulePhase, ValidationPipeline, ValidationReport, ValidationRule};
pub use result::{ReasonCode, RejectionHint, ValidationResult, ValidationStage};
