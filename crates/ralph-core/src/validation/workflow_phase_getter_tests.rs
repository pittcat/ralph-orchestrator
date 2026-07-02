//! 2026-07-02-006 plan U18: `ValidationContext::workflow_phase` getter.
//!
//! Test entry point:
//! `cargo nextest run -p ralph-core -- validation_context_workflow_phase`.

use crate::event_loop::phase_authority::snapshot::PhaseSnapshot;
use crate::state::LedgerSnapshot;
use crate::validation::ValidationContext;

#[test]
fn getter_returns_none_when_engine_disabled() {
    let mut snap = LedgerSnapshot::default();
    let ctx = ValidationContext::new(&mut snap);
    assert!(ctx.workflow_phase().is_none());
}

#[test]
fn getter_returns_some_when_snapshot_has_workflow_phase() {
    let mut snap = LedgerSnapshot::default();
    snap.workflow_phase = Some(PhaseSnapshot::with_phase_id("unit_loop"));
    let ctx = ValidationContext::new(&mut snap);
    let phase = ctx.workflow_phase().expect("workflow_phase");
    assert_eq!(phase.phase_id, "unit_loop");
}

#[test]
fn getter_survives_snapshot_field_assignment_after_construction() {
    let mut snap = LedgerSnapshot::default();
    {
        let ctx = ValidationContext::new(&mut snap);
        assert!(ctx.workflow_phase().is_none());
    }
    snap.workflow_phase = Some(PhaseSnapshot::with_phase_id("review"));
    let ctx = ValidationContext::new(&mut snap);
    assert_eq!(ctx.workflow_phase().unwrap().phase_id, "review");
}