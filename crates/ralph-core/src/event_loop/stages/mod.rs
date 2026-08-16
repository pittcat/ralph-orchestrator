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
pub mod coordinator_decision_gate_stage;
pub mod emit_schema_gate_stage;
pub mod flow_step_scope_stage;
// 2026-07-02-006 plan U13: `PhaseAuthorityStage` consults the
// `WorkflowPhaseAuthority` engine and rejects out-of-phase
// topics with a stable `phase_violation` reason code.
pub mod phase_authority_stage;
pub mod repair_dispatch_stage;
pub mod step_close_obligation_stage;
pub mod target_hat_guard_stage;
pub mod terminal_state_guard_stage;
// Plan 2026-08-16-1015 Unit 4: `TerminalTargetGuardStage` enforces
// `EventSchema::required_target_hat` for terminal topics (e.g.
// `report.done → reporter`). Lives in the same module family as
// `TargetHatGuardStage` but covers a different invariant: target
// matches the schema contract, not just non-self-loop.
pub mod terminal_target_guard_stage;
pub mod verdict_gate_stage;
