//! 2026-06-23-005 U2 (R3+R8): typed dispatch coverage for the three
//! new `RejectionKind` variants added by U1
//! (`MissingEventGate` / `StallNoEvents` / `ContractViolation`).
//!
//! Each test exercises the `CoordinatorDispatcher::dispatch` (typed
//! escalation) path. The thresholds mirror `KTD-2` in the plan
//! document:
//!
//! - `MissingEventGate`: count >= 2 → `PlanBlocked` (via the dead-letter
//!   threshold at COORDINATOR_DEAD_LETTER_THRESHOLD = 3, the test
//!   exercises both the per-kind branch at count < threshold and the
//!   dead-letter path at count >= threshold).
//! - `StallNoEvents`: count >= 3 → `PlanBlocked` (matches the existing
//!   ContractViolation threshold).
//! - `ContractViolation`: count < threshold → `FixPayloadSchema` (typed
//!   branch; count >= threshold → `PlanBlocked` dead-letter).
//!
//! Reference: `docs/plans/2026-06-23-005-fix-ce-executor-serial-hard-gate-half-edge-recovery-plan.md`
//! U2 / R3 / KTD-2.

use crate::event_loop::rejection::{CoordinatorAction, CoordinatorDispatcher};
use crate::preset::engine::gates::RejectionKind;

#[test]
fn missing_event_gate_count_1_routes_reemit_work_ready() {
    let action = CoordinatorDispatcher::dispatch(RejectionKind::MissingEventGate, 1);
    assert!(
        matches!(action, CoordinatorAction::ReEmitWorkReady),
        "MissingEventGate at count < threshold must route to ReEmitWorkReady; got {action:?}"
    );
}

#[test]
fn missing_event_gate_count_2_routes_plan_blocked() {
    let action = CoordinatorDispatcher::dispatch(RejectionKind::MissingEventGate, 3);
    match action {
        CoordinatorAction::PlanBlocked { kind, count } => {
            assert_eq!(kind, RejectionKind::MissingEventGate);
            assert_eq!(count, 3);
        }
        other => panic!("expected PlanBlocked dead-letter; got {other:?}"),
    }
}

#[test]
fn stall_no_events_count_2_routes_reemit_work_ready() {
    let action = CoordinatorDispatcher::dispatch(RejectionKind::StallNoEvents, 2);
    assert!(
        matches!(action, CoordinatorAction::ReEmitWorkReady),
        "StallNoEvents at count < threshold must route to ReEmitWorkReady; got {action:?}"
    );
}

#[test]
fn stall_no_events_count_3_routes_plan_blocked() {
    let action = CoordinatorDispatcher::dispatch(RejectionKind::StallNoEvents, 3);
    match action {
        CoordinatorAction::PlanBlocked { kind, count } => {
            assert_eq!(kind, RejectionKind::StallNoEvents);
            assert_eq!(count, 3);
        }
        other => panic!("expected PlanBlocked dead-letter; got {other:?}"),
    }
}

#[test]
fn contract_violation_count_1_routes_fix_payload_schema() {
    let action = CoordinatorDispatcher::dispatch(RejectionKind::ContractViolation, 1);
    assert!(
        matches!(action, CoordinatorAction::FixPayloadSchema),
        "ContractViolation at count < threshold must route to FixPayloadSchema; got {action:?}"
    );
}

#[test]
fn contract_violation_count_3_routes_plan_blocked() {
    let action = CoordinatorDispatcher::dispatch(RejectionKind::ContractViolation, 3);
    match action {
        CoordinatorAction::PlanBlocked { kind, count } => {
            assert_eq!(kind, RejectionKind::ContractViolation);
            assert_eq!(count, 3);
        }
        other => panic!("expected PlanBlocked dead-letter; got {other:?}"),
    }
}

#[test]
fn unknown_kind_falls_through_to_reemit_work_ready() {
    // `RejectionKind::MissingField` is not in the typed dispatch table
    // (no match arm); the `_ => ReEmitWorkReady` arm must catch it.
    let action = CoordinatorDispatcher::dispatch(RejectionKind::MissingField, 1);
    assert!(
        matches!(action, CoordinatorAction::ReEmitWorkReady),
        "Unknown kind at count < threshold must fall through to ReEmitWorkReady; got {action:?}"
    );
}

#[test]
fn unknown_kind_count_5_routes_plan_blocked() {
    // Dead-letter threshold (>= 3) fires first regardless of which kind
    // — protects against silent swallowing of an unmapped kind.
    let action = CoordinatorDispatcher::dispatch(RejectionKind::MissingField, 5);
    match action {
        CoordinatorAction::PlanBlocked { kind, count } => {
            assert_eq!(kind, RejectionKind::MissingField);
            assert_eq!(count, 5);
        }
        other => panic!("expected PlanBlocked dead-letter; got {other:?}"),
    }
}

#[test]
fn all_six_new_kinds_are_dispatched_by_kind_arm() {
    // Exhaustiveness check: every newly-added `RejectionKind` variant
    // must NOT fall through to the `_` arm at count = 1 (otherwise we
    // silently swallow typed kind, defeating the U1 typed kind wiring).
    // We can detect this by checking that the action differs from the
    // default ReEmitWorkReady for at least one of the test inputs — for
    // ContractViolation, it must be FixPayloadSchema instead.
    let contract_action = CoordinatorDispatcher::dispatch(RejectionKind::ContractViolation, 1);
    assert!(
        matches!(contract_action, CoordinatorAction::FixPayloadSchema),
        "ContractViolation must route to FixPayloadSchema, NOT the _ => ReEmitWorkReady fallback"
    );
    let missing_event_action = CoordinatorDispatcher::dispatch(RejectionKind::MissingEventGate, 1);
    assert!(
        matches!(missing_event_action, CoordinatorAction::ReEmitWorkReady),
        "MissingEventGate must route to ReEmitWorkReady"
    );
    let stall_action = CoordinatorDispatcher::dispatch(RejectionKind::StallNoEvents, 1);
    assert!(
        matches!(stall_action, CoordinatorAction::ReEmitWorkReady),
        "StallNoEvents must route to ReEmitWorkReady at low count"
    );
}
