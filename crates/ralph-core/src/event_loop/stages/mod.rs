//! Pipeline stages wiring (U6+). Each U-* wiring unit lives in
//! its own submodule:
//!
//! - U6: `emit_schema_gate_stage` (required-fields check).
//! - U7: `repair_dispatch_stage` (repair-topic early return).
//! - U9: `flow_step_scope_stage` (allowed_emits check).
//! - U9.5: `verdict_gate_stage` (terminal alignment).
//! - U11: `archive_version_stage` (loop-start hook).
//! - P1-4 (2026-06-27 adversarial review):
//!   `step_close_obligation_stage` (U12 partial-state
//!   obligation enforcement).
//!
//! The list is registered in `stage_pipeline::with_default_stages`
//! in the locked order documented in
//! `docs/plans/2026-06-27-001-feat-ralph-orchestrator-mechanism-foundation-plan.md`
//! §"Stage pipeline 锁定".

pub mod archive_version_stage;
pub mod emit_schema_gate_stage;
pub mod flow_step_scope_stage;
pub mod repair_dispatch_stage;
pub mod step_close_obligation_stage;
pub mod verdict_gate_stage;
