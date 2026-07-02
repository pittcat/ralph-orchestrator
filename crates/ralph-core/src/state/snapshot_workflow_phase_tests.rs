//! 2026-07-02-006 plan U12: `LedgerSnapshot::workflow_phase` field.
//!
//! Test entry point:
//! `cargo nextest run -p ralph-core -- ledger_snapshot_workflow_phase`.
//!
//! The unit's contract is "仅增字段;不改 gate 行为". The
//! round-trip and the cold-start default cover the shape; gate
//! behaviour is exercised by the existing event-loop tests
//! that read `LedgerSnapshot`.

use crate::event_loop::phase_authority::snapshot::{
    PhaseSnapshot, ViolationKind,
};
use crate::state::snapshot::LedgerSnapshot;

#[test]
fn cold_start_defaults_workflow_phase_to_none() {
    let snap = LedgerSnapshot::default();
    assert!(
        snap.workflow_phase.is_none(),
        "workflow_phase must be None when the engine is disabled (cold start)"
    );
}

#[test]
fn workflow_phase_field_round_trips_through_clone() {
    let mut snap = LedgerSnapshot::default();
    let phase = PhaseSnapshot::with_phase_id("unit_loop")
        .with_entered_at_seq(7)
        .bump_violation("coordinator", ViolationKind::PhaseViolation);
    snap.workflow_phase = Some(phase.clone());

    let cloned = snap.clone();
    let restored = cloned
        .workflow_phase
        .expect("workflow_phase must survive clone");

    assert_eq!(restored.phase_id, "unit_loop");
    assert_eq!(restored.entered_at_seq, 7);
    assert_eq!(
        restored
            .violation_counts
            .get(&("coordinator".to_string(), ViolationKind::PhaseViolation)),
        Some(&1)
    );
}

#[test]
fn workflow_phase_field_does_not_change_default_of_other_fields() {
    // Regression guard: adding `workflow_phase` must not shift
    // any of the existing counters or collections away from
    // their `Default` zero state. The test reads a handful of
    // representative fields and asserts their pre-006 baseline
    // values are preserved.
    let snap = LedgerSnapshot::default();
    assert_eq!(snap.iteration, 0);
    assert_eq!(snap.consecutive_failures, 0);
    assert_eq!(snap.cumulative_cost, 0.0);
    assert!(snap.flow_lifecycle_log.is_empty());
    assert!(snap.workflow_phase.is_none());
}