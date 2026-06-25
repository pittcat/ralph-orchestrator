//! 2026-06-23-005 U1 (R1+R2+R8): typed `kind` SSOT wiring for the
//! `enrich_task_resume_payload[_with_stage]` helper functions.
//!
//! The `enrich_*` helpers are the dual path of `build_task_resume_payload`
//! (see `crates/ralph-core/data/doppelganger-functions.md` for the full
//! SSOT). U1 closed the half-edge fix by adding a typed `kind: Option<RejectionKind>`
//! parameter so every caller can carry the typed kind SSOT downstream.
//!
//! Reference: `docs/plans/2026-06-23-005-fix-ce-executor-serial-hard-gate-half-edge-recovery-plan.md`
//! U1 / R1 / R2 / R8 / KTD-1.

use crate::event_loop::rejection::{
    RejectionStage, enrich_task_resume_payload, enrich_task_resume_payload_with_stage,
    task_resume_payload_has_required_fields,
};
use crate::preset::engine::gates::RejectionKind;

#[test]
fn enrich_task_resume_payload_carries_typed_kind_when_provided() {
    let payload = enrich_task_resume_payload(
        "hard_gate missing event",
        "hard_gate_missing_event",
        Some("dimension-reviewer"),
        Some(RejectionKind::MissingEventGate),
    );
    assert!(task_resume_payload_has_required_fields(&payload));
    let v: serde_json::Value = serde_json::from_str(&payload).expect("valid JSON");
    assert_eq!(v["kind"], "missing_event_gate");
    assert_eq!(v["target_hat"], "dimension-reviewer");
    assert_eq!(v["reason"], "missing_field");
}

#[test]
fn enrich_task_resume_payload_kind_none_falls_back_to_reason() {
    let payload = enrich_task_resume_payload("out-of-scope", "out-of-scope", Some("ralph"), None);
    let v: serde_json::Value = serde_json::from_str(&payload).expect("valid JSON");
    // Fallback mirrors reason_code (legacy behaviour preserved).
    assert_eq!(v["kind"], "out_of_scope");
    assert_eq!(v["reason"], "out_of_scope");
}

#[test]
fn enrich_with_stage_helper_carries_both_stage_and_typed_kind() {
    let payload = enrich_task_resume_payload_with_stage(
        "missing event",
        "hard_gate_missing_event",
        Some("dimension-reviewer"),
        Some(RejectionStage::MissingEvent),
        Some(RejectionKind::MissingEventGate),
    );
    let v: serde_json::Value = serde_json::from_str(&payload).expect("valid JSON");
    assert_eq!(v["stage"], "missing_event");
    assert_eq!(v["kind"], "missing_event_gate");
    assert_eq!(v["target_hat"], "dimension-reviewer");
}

#[test]
fn enrich_with_stage_helper_omits_stage_when_none_but_keeps_kind() {
    let payload = enrich_task_resume_payload_with_stage(
        "stall recovery",
        "stall_no_events",
        Some("executor"),
        None,
        Some(RejectionKind::StallNoEvents),
    );
    let v: serde_json::Value = serde_json::from_str(&payload).expect("valid JSON");
    // Stage None: no `stage` field (legacy behaviour).
    assert!(v.get("stage").is_none(), "stage=None must omit field");
    // Kind Some: typed kind MUST still be present.
    assert_eq!(v["kind"], "stall_no_events");
}

#[test]
fn enrich_three_new_kinds_round_trip_via_reason_code() {
    // Verifies that each of the three new `RejectionKind` variants
    // serialises to a stable reason_code (R8: kind 覆盖率 100%).
    for (kind, expected_reason) in [
        (RejectionKind::MissingEventGate, "missing_event_gate"),
        (RejectionKind::StallNoEvents, "stall_no_events"),
        (RejectionKind::ContractViolation, "contract_violation"),
    ] {
        let payload = enrich_task_resume_payload("hint", "hint", Some("h"), Some(kind));
        let v: serde_json::Value = serde_json::from_str(&payload).expect("valid JSON");
        assert_eq!(
            v["kind"], expected_reason,
            "kind {kind:?} must serialise to {expected_reason}"
        );
    }
}

/// 2026-06-23-005 F2: the two completion-signal rejection paths
/// (persistent mode at `event_loop::mod.rs:1757`, open tasks at
/// `event_loop::mod.rs:1801`) MUST also carry a typed kind so the
/// R2 SSOT holds for every caller (no `None` fallback for
/// recovery.injected task.resume payloads).
#[test]
fn enrich_completion_rejection_kinds_round_trip() {
    for (kind, expected_reason) in [
        (
            RejectionKind::PersistentLoopActive,
            "persistent_loop_active",
        ),
        (RejectionKind::OpenTasksBlocking, "open_tasks_blocking"),
    ] {
        let payload = enrich_task_resume_payload("hint", "hint", Some("h"), Some(kind));
        let v: serde_json::Value = serde_json::from_str(&payload).expect("valid JSON");
        assert_eq!(
            v["kind"], expected_reason,
            "completion-rejection kind {kind:?} must serialise to {expected_reason}"
        );
    }
}

/// 2026-06-23-005 F2: every `enrich_task_resume_payload` caller in
/// the runtime must carry a typed `Some(RejectionKind)`. This test
/// is a *static* guard — it cannot grep the source tree, so it
/// asserts the documented SSOT contract by re-invoking each
/// completion-rejection caller with the same payload the loop
/// produces and verifying `kind` is non-empty.
#[test]
fn completion_rejection_payloads_have_typed_kind_field() {
    // mod.rs:1757 — persistent mode path
    let persistent_payload = enrich_task_resume_payload(
        "Persistent mode: loop staying alive after completion signal.",
        "persistent mode",
        None,
        Some(RejectionKind::PersistentLoopActive),
    );
    let v: serde_json::Value = serde_json::from_str(&persistent_payload).expect("valid JSON");
    assert_eq!(v["kind"], "persistent_loop_active");

    // mod.rs:1801 — open tasks remain path
    let open_tasks_payload = enrich_task_resume_payload(
        "Completion rejected: runtime tasks remain open: [t1].",
        "open tasks remain",
        None,
        Some(RejectionKind::OpenTasksBlocking),
    );
    let v2: serde_json::Value = serde_json::from_str(&open_tasks_payload).expect("valid JSON");
    assert_eq!(v2["kind"], "open_tasks_blocking");
}
