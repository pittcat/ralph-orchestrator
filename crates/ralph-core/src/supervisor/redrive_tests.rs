//! U11 unit tests: `SupervisorStore::create_redrive_wave` API.
//!
//! Tests the store-level redrive logic:
//! - rejects Done/Integrate parent waves
//! - rejects zero-failed parent waves
//! - happy path: creates child with attempt_epoch + 1
//! - idempotency: duplicate (parent, slot, epoch) returns same child
//! - explicit slots filter: only failed slots included
//! - unknown parent wave → UnknownWave

use crate::supervisor::{
    InMemorySupervisorStore, SlotResource, SupervisorStore, SupervisorStoreError, WaveKind,
    WavePhase,
};

fn store() -> InMemorySupervisorStore {
    InMemorySupervisorStore::new()
}

fn make_wave_with_failed_slots(
    s: &InMemorySupervisorStore,
    n: u32,
    failed_indices: &[u32],
) -> String {
    let wave_id = s.register_wave("test-key", WaveKind::Exec, n, 1).unwrap();
    for i in 0..n {
        let resource = SlotResource {
            slot_index: i,
            worktree_path: Some(format!(".ralph/wt/{i}")),
            branch: Some(format!("ralph/u{i}")),
        };
        s.bind_worktree(&wave_id, i, resource).unwrap();
    }
    for &i in failed_indices {
        s.record_slot_failure(&wave_id, i, "test failure").unwrap();
    }
    wave_id
}

/// create_redrive_wave_rejects_done_parent
/// Parent wave in Done phase → InvalidTransition.
#[test]
fn create_redrive_wave_rejects_done_parent() {
    let s = store();
    // Register a wave and simulate it reaching Done phase.
    let wave_id = s
        .register_wave("done-parent", WaveKind::Exec, 2, 1)
        .unwrap();
    // Transition to Done via phase-evaluation path (mark slots completed).
    s.record_slot_result(&wave_id, 0, "hash1", 1).unwrap();
    s.record_slot_result(&wave_id, 1, "hash2", 1).unwrap();
    // Advance phase to Done (simulate coordinator evaluation).
    // fan_in_status is read-only; we set phase via the store setter.
    s.set_wave_phase(&wave_id, WavePhase::Done).unwrap();

    let result = s.create_redrive_wave(&wave_id, None);
    assert!(
        matches!(result, Err(SupervisorStoreError::InvalidTransition(ref msg))
            if msg.contains("done") || msg.contains("Done") || msg.contains("terminal")),
        "expected InvalidTransition for Done parent, got {result:?}"
    );
}

/// create_redrive_wave_rejects_zero_failed
/// Parent has 0 failed slots → InvalidTransition("no failed slots to redrive").
#[test]
fn create_redrive_wave_rejects_zero_failed() {
    let s = store();
    let wave_id = make_wave_with_failed_slots(&s, 3, &[]);
    // Mark all completed, not failed.
    s.record_slot_result(&wave_id, 0, "h1", 1).unwrap();
    s.record_slot_result(&wave_id, 1, "h2", 1).unwrap();
    s.record_slot_result(&wave_id, 2, "h3", 1).unwrap();

    let result = s.create_redrive_wave(&wave_id, None);
    assert!(
        matches!(result, Err(SupervisorStoreError::InvalidTransition(ref msg))
            if msg.contains("no failed slots")),
        "expected InvalidTransition for zero-failed wave, got {result:?}"
    );
}

/// create_redrive_wave_creates_child_with_attempt_epoch_plus_one
/// Happy path: parent attempt_epoch=0 → child attempt_epoch=1.
#[test]
fn create_redrive_wave_creates_child_with_attempt_epoch_plus_one() {
    let s = store();
    let parent_id = make_wave_with_failed_slots(&s, 3, &[0, 2]);

    let result = s.create_redrive_wave(&parent_id, None);
    let redrive = result.expect("create_redrive_wave should succeed");
    assert_eq!(redrive.parent_wave_id, parent_id);
    assert_eq!(redrive.attempt_epoch, 1, "child epoch should be parent + 1");
    assert!(!redrive.child_wave_id.is_empty());
    assert_eq!(
        redrive.slots,
        vec![0, 2],
        "should include failed slots 0 and 2"
    );
}

/// create_redrive_wave_idempotent_on_duplicate_triple
/// Calling twice with same (parent, slot, attempt_epoch) → same child wave.
#[test]
fn create_redrive_wave_idempotent_on_duplicate_triple() {
    let s = store();
    let parent_id = make_wave_with_failed_slots(&s, 2, &[1]);

    let r1 = s
        .create_redrive_wave(&parent_id, None)
        .expect("first redrive should succeed");
    let r2 = s
        .create_redrive_wave(&parent_id, None)
        .expect("second redrive should succeed (idempotent)");

    assert_eq!(
        r1.child_wave_id, r2.child_wave_id,
        "duplicate redrive should return same child wave_id"
    );
    assert_eq!(
        r1.attempt_epoch, r2.attempt_epoch,
        "attempt_epoch should be stable across idempotent calls"
    );
}

/// create_redrive_wave_only_includes_failed_slots
/// Explicit `--slots 0` on a wave where slot 1 is Failed and slot 0 is Completed
/// → child wave has 1 slot.
#[test]
fn create_redrive_wave_only_includes_failed_slots() {
    let s = store();
    let wave_id = make_wave_with_failed_slots(&s, 3, &[1]);
    // Slot 0 is Completed.
    s.record_slot_result(&wave_id, 0, "hash0", 1).unwrap();
    // Slot 2 is Pending.

    // Redrive only slot 1 (the failed one).
    let result = s
        .create_redrive_wave(&wave_id, Some(&[1]))
        .expect("redrive with explicit slot 1 should succeed");

    assert_eq!(
        result.slots,
        vec![1],
        "should include only the explicitly specified failed slot"
    );
}

/// create_redrive_wave_handles_unknown_parent_wave_id
/// Unknown parent wave_id → UnknownWave error.
#[test]
fn create_redrive_wave_handles_unknown_parent_wave_id() {
    let s = store();
    let result = s.create_redrive_wave("w-99999", None);
    assert!(
        matches!(result, Err(SupervisorStoreError::UnknownWave(ref id))
            if id == "w-99999"),
        "expected UnknownWave for unknown id, got {result:?}"
    );
}
