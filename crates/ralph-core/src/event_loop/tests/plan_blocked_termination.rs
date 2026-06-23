//! 2026-06-23-005 U3 (R4+R8): typed dead-letter termination path.
//!
//! Verifies that when `CoordinatorDispatcher::dispatch` returns
//! `PlanBlocked` for a task.resume dead-letter, the typed
//! `TerminationTrigger::DeadLetter` surface maps to a typed
//! `TerminationReason::PayloadContractViolation` (the closest typed
//! variant available today). The plan's full typed dead-letter
//! termination reason (per AE-3) is tracked as follow-up work in the
//! plan's `Deferred to Follow-Up Work` section — this U3 PR establishes
//! the typed enum SSOT without requiring a breaking `TerminationReason`
//! variant addition.
//!
//! Reference: `docs/plans/2026-06-23-005-fix-ce-executor-serial-hard-gate-half-edge-recovery-plan.md`
//! U3 / R4 / R8 / AE-3.

use crate::event_loop::rejection::{CoordinatorAction, CoordinatorDispatcher};
use crate::event_loop::termination::{
    DeadLetterSource, TerminationTrigger, trigger_to_reason,
};
use crate::preset::engine::gates::RejectionKind;
use crate::TerminationReason;

#[test]
fn missing_event_gate_dead_letter_routes_to_dead_letter_trigger() {
    // Threshold (3) → PlanBlocked in CoordinatorDispatcher
    let action = CoordinatorDispatcher::dispatch(RejectionKind::MissingEventGate, 3);
    assert!(matches!(action, CoordinatorAction::PlanBlocked { .. }));

    // Surface as typed DeadLetter trigger
    let trigger = TerminationTrigger::DeadLetter {
        kind: RejectionKind::MissingEventGate,
        source: DeadLetterSource::HardGate,
    };
    let reason = trigger_to_reason(trigger);
    // AE-3: surface MUST be a typed (non-string-concatenated) reason.
    // Today we route through PayloadContractViolation as the closest
    // typed variant; full typed variant is U8 follow-up.
    assert_eq!(reason, TerminationReason::PayloadContractViolation);
}

#[test]
fn stall_no_events_dead_letter_at_threshold_routes_to_dead_letter_trigger() {
    let action = CoordinatorDispatcher::dispatch(RejectionKind::StallNoEvents, 3);
    assert!(matches!(action, CoordinatorAction::PlanBlocked { .. }));

    let trigger = TerminationTrigger::DeadLetter {
        kind: RejectionKind::StallNoEvents,
        source: DeadLetterSource::StallRecovery,
    };
    let reason = trigger_to_reason(trigger);
    assert_eq!(reason, TerminationReason::PayloadContractViolation);
}

#[test]
fn contract_violation_dead_letter_at_threshold_routes_to_dead_letter_trigger() {
    let action = CoordinatorDispatcher::dispatch(RejectionKind::ContractViolation, 3);
    assert!(matches!(action, CoordinatorAction::PlanBlocked { .. }));

    let trigger = TerminationTrigger::DeadLetter {
        kind: RejectionKind::ContractViolation,
        source: DeadLetterSource::PayloadContract,
    };
    let reason = trigger_to_reason(trigger);
    assert_eq!(reason, TerminationReason::PayloadContractViolation);
}

#[test]
fn all_three_dead_letter_sources_serialize_to_typed_reason() {
    // R14: typed enum serialization — no string concatenation.
    let triggers = [
        (
            RejectionKind::MissingEventGate,
            DeadLetterSource::HardGate,
        ),
        (
            RejectionKind::StallNoEvents,
            DeadLetterSource::StallRecovery,
        ),
        (
            RejectionKind::ContractViolation,
            DeadLetterSource::PayloadContract,
        ),
    ];
    for (kind, source) in triggers {
        let reason = trigger_to_reason(TerminationTrigger::DeadLetter { kind, source });
        assert_eq!(
            reason,
            TerminationReason::PayloadContractViolation,
            "kind={kind:?} source={source:?} must map to typed reason"
        );
    }
}
