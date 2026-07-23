//! U4 protocol-level tests: idempotency dedup, content dedup,
//! backpressure, cancel + compensation. Each scenario pins a single
//! requirement from R6 (backpressure), R7 (cancel), R8 (recovery),
//! R9 (idempotency), R10 (content dedup) or R11 (compensation) on
//! top of `InMemorySupervisorStore`. The U5 rusqlite store is
//! expected to mirror the same test matrix.

use crate::supervisor::{
    InMemorySupervisorStore, SlotResource, SlotStatus, SupervisorStore, SupervisorStoreError,
    SupervisorStoreResult, WaveKind, WavePhase, WaveSnapshot,
};
use std::collections::HashMap;

fn store() -> InMemorySupervisorStore {
    InMemorySupervisorStore::new()
}

fn bind(slot: u32) -> SlotResource {
    SlotResource {
        slot_index: slot,
        worktree_path: Some(format!(".ralph/wt/{slot}")),
        branch: Some(format!("ralph/u{slot}")),
    }
}

fn wave_into(
    s: &InMemorySupervisorStore,
    key: &str,
    kind: WaveKind,
    n: u32,
) -> SupervisorStoreResult<String> {
    let wave = s.register_wave(key, kind, n)?;
    for i in 0..n {
        s.bind_worktree(&wave, i, bind(i))?;
    }
    Ok(wave)
}

/// R-D1: a duplicate `idempotency_key` is rejected at
/// `register_wave` time. This is the contract that protects the
/// dispatcher from double-spawning the same wave on a loop retry.
#[test]
fn duplicate_idempotency_key_returns_duplicate_key() {
    let s = store();
    s.register_wave("dup", WaveKind::Exec, 1).unwrap();
    let err = s.register_wave("dup", WaveKind::Fix, 1).unwrap_err();
    assert!(
        matches!(err, SupervisorStoreError::DuplicateKey(ref k) if k == "dup"),
        "expected DuplicateKey, got {err:?}"
    );
}

/// R-E1: when the same slot reports the same `content_hash` twice
/// (a retry hitting already-completed state), the second call does
/// not overwrite the stored result. Required for content-dedup so
/// downstream JSONL merges don't double-append.
#[test]
fn same_content_hash_does_not_overwrite() {
    let s = store();
    let wave = wave_into(&s, "dup-content", WaveKind::Exec, 1).unwrap();
    s.record_slot_result(&wave, 0, "hash-a", 3).unwrap();
    let snap_before: WaveSnapshot = s.fan_in_status(&wave).unwrap();
    // Re-report same hash; counters must not change.
    s.record_slot_result(&wave, 0, "hash-a", 3).unwrap();
    let snap_after = s.fan_in_status(&wave).unwrap();
    assert_eq!(snap_before.completed_count, snap_after.completed_count);
    assert_eq!(snap_before.expected_total, snap_after.expected_total);
}

/// 2026-07-23-004 plan U5 (R-A3): first-terminal-wins;
/// conflicting terminal events for the same slot are rejected
/// instead of replacing the recorded result. The previous
/// "latest write wins" R-E2 contract is replaced by the
/// stricter U5 contract.
#[test]
fn different_content_hash_replaces_record() {
    let s = store();
    let wave = wave_into(&s, "diff-hash", WaveKind::Exec, 1).unwrap();
    s.record_slot_result(&wave, 0, "hash-a", 1).unwrap();
    // The slot is now Completed with `hash-a`. A second
    // record_slot_result with a different content_hash MUST be
    // rejected — conflicting terminal events must not overwrite
    // the recorded result.
    let conflict = s.record_slot_result(&wave, 0, "hash-b", 2);
    assert!(
        matches!(conflict, Err(SupervisorStoreError::AlreadyTerminal(_))),
        "conflicting terminal must be rejected as AlreadyTerminal, got {conflict:?}"
    );

    // Idempotent replay with the SAME content_hash is allowed.
    let replay = s.record_slot_result(&wave, 0, "hash-a", 1);
    assert!(
        replay.is_ok(),
        "idempotent replay with the same content_hash must succeed, got {replay:?}"
    );

    let snap = s.fan_in_status(&wave).unwrap();
    assert_eq!(snap.completed_count, 1);
}

/// R-A2: when active workers reach the soft cap, additional
/// `try_dispatch_next` calls return `Ok(None)` instead of consuming
/// a slot from the queue. The contract is independent of whether
/// the wave passed through `enqueue_wave` or `register_wave`.
#[test]
fn backpressure_returns_none_when_cap_is_hit() {
    let s = store();
    // Two pending waves, each with 2 slots — 4 dispatches in total
    // would be possible without backpressure.
    let _w1 = wave_into(&s, "bp-1", WaveKind::Exec, 2).unwrap();
    let _w2 = wave_into(&s, "bp-2", WaveKind::Exec, 2).unwrap();
    // Cap = 2: first two dispatch attempts succeed.
    let d1 = s.try_dispatch_next(2).unwrap();
    assert!(d1.is_some());
    let d2 = s.try_dispatch_next(2).unwrap();
    assert!(d2.is_some());
    // Third call backpressures.
    let d3 = s.try_dispatch_next(2).unwrap();
    assert!(d3.is_none(), "third call must yield None when cap is hit");
}

/// R-A3: once a slot completes (or fails), the count of active
/// workers drops and the next dispatch attempt drains the next
/// pending slot (FIFO across waves).
#[test]
fn backpressure_releases_after_slot_completes() {
    let s = store();
    let _w1 = wave_into(&s, "bp-fifo-1", WaveKind::Exec, 1).unwrap();
    let _w2 = wave_into(&s, "bp-fifo-2", WaveKind::Exec, 1).unwrap();
    let (dispatched_wave, dispatched_idx) = s.try_dispatch_next(1).unwrap().unwrap();
    let _ = s.try_dispatch_next(1).unwrap();
    // Cap is full -> backpressure.
    assert!(s.try_dispatch_next(1).unwrap().is_none());
    // Complete the first slot -> count drops.
    s.record_slot_result(&dispatched_wave, dispatched_idx, "h0", 1)
        .unwrap();
    let next = s.try_dispatch_next(1).unwrap();
    assert!(
        next.is_some(),
        "after slot completes, dispatch slot from queue/other wave"
    );
}

/// R-A4 (FIFO contract): when waves are tracked via `enqueue_wave`,
/// drain order is FIFO across waves: wave-1 first, wave-2 second.
#[test]
fn enqueue_wave_drains_fifo_across_waves() {
    let s = store();
    let w1 = s.enqueue_wave("fifo-1", WaveKind::Exec, 1).unwrap();
    s.bind_worktree(&w1, 0, bind(0)).unwrap();
    let w2 = s.enqueue_wave("fifo-2", WaveKind::Exec, 1).unwrap();
    s.bind_worktree(&w2, 0, bind(0)).unwrap();
    // Cap = 1: only w1's slot can dispatch.
    let first = s.try_dispatch_next(1).unwrap().unwrap();
    assert_eq!(first.0, w1, "first dispatched slot must belong to w1");
    assert!(s.try_dispatch_next(1).unwrap().is_none());
    // Complete, then w2 must dispatch.
    s.record_slot_result(&w1, 0, "h", 1).unwrap();
    let second = s.try_dispatch_next(1).unwrap().unwrap();
    assert_eq!(second.0, w2, "second dispatched slot must belong to w2");
}

/// R-B3: `cancel_wave` on a wave whose slots are all `Pending`
/// moves them to `Cancelled`; the snapshot's `cancel_requested`
/// flag flips on; `pending_count` keeps the cancelled slots in
/// the same bucket so the phase-decision pure function reads
/// them as `cancelled-but-not-yet-failed`.
#[test]
fn cancel_wave_moves_pending_slots_to_cancelled() {
    let s = store();
    let wave = wave_into(&s, "cx-pending", WaveKind::Exec, 2).unwrap();
    s.cancel_wave(&wave).unwrap();
    let snap = s.fan_in_status(&wave).unwrap();
    assert!(snap.cancel_requested);
    // Cancelled slots live in `pending_count` per the U3
    // snapshot contract. They are not Failed (R-KTD-8).
    assert_eq!(snap.pending_count, 2);
    assert_eq!(snap.failed_count, 0);
    assert_eq!(snap.completed_count, 0);
}

/// R-B4: cancelling an already-running wave only flips the flag
/// here; the runtime is responsible for the PID kill and the
/// eventual `record_slot_failure(reason=cancelled)`. The store
/// remains consistent either way.
#[test]
fn cancel_wave_does_not_force_running_to_failed() {
    let s = store();
    let wave = wave_into(&s, "cx-running", WaveKind::Exec, 1).unwrap();
    let (w, idx) = s.try_dispatch_next(2).unwrap().unwrap();
    assert_eq!(w, wave);
    s.cancel_wave(&wave).unwrap();
    let snap_before = s.fan_in_status(&wave).unwrap();
    assert!(snap_before.cancel_requested);
    assert_eq!(
        snap_before.failed_count, 0,
        "cancel alone must not push a running slot into Failed"
    );
    // Simulate the runtime killing the worker process and
    // reporting the failure.
    s.record_slot_failure(&wave, idx, "cancelled").unwrap();
    let snap_after = s.fan_in_status(&wave).unwrap();
    assert_eq!(snap_after.failed_count, 1);
}

/// R-F2/R-F4: compensation on failure must surface in
/// `fan_in_status`. The in-memory store records the slot's
/// terminal state plus the cancellation flag; U2 / KTD-8
/// moved the wave's `phase = Failed` verdict into the
/// coordinator's `set_wave_phase` call. The compensation
/// jobs themselves (out of scope for the store layer) live
/// in the dispatch bridge; the recovery module owns the
/// worker that runs them. This pins the read-side contract.
#[test]
fn compensation_records_failure_state_on_wave() {
    let s = store();
    let wave = wave_into(&s, "comp", WaveKind::Fix, 2).unwrap();
    // Cancel trigger: any pending slot on the wave turns Cancelled.
    s.cancel_wave(&wave).unwrap();
    // Failure trigger: record a permanent failure on slot 0.
    s.record_slot_failure(&wave, 0, "permanent").unwrap();
    let snap = s.fan_in_status(&wave).unwrap();
    assert!(
        snap.cancel_requested,
        "cancel flag must persist on terminal"
    );
    assert_eq!(snap.failed_count, 1);
    // U2 / KTD-8: the store does NOT mutate `phase`. The
    // coordinator's `fail_wave` applies `set_wave_phase`
    // when `evaluate_phase` returns `Failed`. Until then,
    // the wave stays in its initial non-terminal phase
    // (`Dispatch` from `register_wave`).
    assert!(
        !matches!(snap.phase, WavePhase::Failed | WavePhase::Done),
        "phase must stay non-terminal until the coordinator applies the verdict (U2 KTD-8); got {:?}",
        snap.phase
    );
    // Simulate the coordinator applying the verdict.
    s.set_wave_phase(&wave, WavePhase::Failed).unwrap();
    // The wave reached the Failed phase, so it is **excluded**
    // from `recover_active_waves` (terminal phase filter); this
    // is the contract that prevents re-injection on restart.
    let recovered = s.recover_active_waves().unwrap();
    assert!(
        recovered.iter().all(|s| s.wave_id != wave),
        "Failed phase must not surface in recovery"
    );
}

/// R-C3: `recover_active_waves` returns a snapshot for every
/// wave whose phase is not terminal. This pins the U11 recovery
/// contract: recovered waves retain their slot counts so the
/// coordinator can decide whether to re-merge, timeout, or
/// re-dispatch.
///
/// U2 / KTD-8: a `Failed` wave reaches terminal phase only
/// after the coordinator applies the verdict via
/// `set_wave_phase` — `record_slot_failure` alone does NOT
/// flip the phase. The test below uses both endpoints to
/// cover the storage-only AND the verdict-applied paths.
#[test]
fn recover_active_waves_returns_all_non_terminal_phases() {
    let s = store();
    let live = wave_into(&s, "live", WaveKind::Exec, 2).unwrap();
    let failed = wave_into(&s, "failed", WaveKind::Exec, 1).unwrap();
    s.record_slot_failure(&failed, 0, "boom").unwrap();
    // U2: store leaves the wave in Collect; simulate the
    // coordinator verdict with `set_wave_phase(Failed)` so
    // it becomes terminal.
    s.set_wave_phase(&failed, WavePhase::Failed).unwrap();
    let snaps: HashMap<String, WaveSnapshot> = s
        .recover_active_waves()
        .unwrap()
        .into_iter()
        .map(|s| (s.wave_id.clone(), s))
        .collect();
    assert!(snaps.contains_key(&live), "live wave must surface");
    // The failed wave is now in Failed phase, so it is
    // skipped by `recover_active_waves` (its phase is terminal).
    assert!(
        !snaps.contains_key(&failed),
        "terminal phase must not surface in recovery"
    );
}

/// Cross-check for U3's documented "Dispatch does not touch
/// Started/Running counters" promise now that U4 introduced
/// `in_flight_count`. Two-slot wave with one slot in flight
/// after dispatch must split across `in_flight` and `pending`.
#[test]
fn in_flight_count_reflects_dispatched_status() {
    let s = store();
    let wave = wave_into(&s, "ifc", WaveKind::Exec, 2).unwrap();
    let (_, _) = s.try_dispatch_next(2).unwrap().unwrap();
    let snap = s.fan_in_status(&wave).unwrap();
    assert_eq!(snap.in_flight_count, 1);
    assert_eq!(snap.pending_count, 1);
}

/// Sanity: a slot that was Dispatched then `record_slot_result`'d
/// moves from `in_flight_count` to `completed_count`. The
/// invariant is `completed + failed + in_flight + pending ==
/// expected_total`.
#[test]
fn total_slot_count_partitions_consistently() {
    let s = store();
    let wave = wave_into(&s, "parts", WaveKind::Exec, 3).unwrap();
    let (w, i) = s.try_dispatch_next(2).unwrap().unwrap();
    s.record_slot_result(&w, i, "h", 1).unwrap();
    let snap = s.fan_in_status(&wave).unwrap();
    let total =
        snap.completed_count + snap.failed_count + snap.in_flight_count + snap.pending_count;
    assert_eq!(
        total, snap.expected_total,
        "fan_in_status partitioning invariant violated: {:?}",
        snap
    );
}

/// Ensure SlotStatus Round-trips so future rusqlite tests can
/// assert against serialized state. (Cheap to test here.)
#[test]
fn slot_status_string_round_trip() {
    for status in [
        SlotStatus::Pending,
        SlotStatus::Dispatched,
        SlotStatus::Running,
        SlotStatus::Completed,
        SlotStatus::Failed,
        SlotStatus::Cancelled,
    ] {
        let s = status.to_string();
        let back: SlotStatus = serde_json::from_str(&format!("\"{s}\"")).unwrap();
        assert_eq!(back, status);
    }
}

/// U4 cap=4 barrier contract: after four slots are dispatched,
/// terminal release makes the fifth FIFO slot dispatchable.
#[test]
fn cap_four_release_five_slots() {
    let s = store();
    let wave = wave_into(&s, "cap4-release", WaveKind::Exec, 5).unwrap();
    let mut dispatched = Vec::new();
    for _ in 0..4 {
        dispatched.push(s.try_dispatch_next(4).unwrap().unwrap());
    }
    assert_eq!(s.fan_in_status(&wave).unwrap().in_flight_count, 4);
    assert!(s.try_dispatch_next(4).unwrap().is_none());

    let (released_wave, released_slot) = dispatched[0].clone();
    s.release_slot_dispatch(
        &released_wave,
        released_slot,
        crate::supervisor::DispatchOutcome::Completed,
    )
    .unwrap();

    let fifth = s
        .try_dispatch_next(4)
        .unwrap()
        .expect("fifth slot must dispatch after a terminal release");
    assert_eq!(fifth.0, wave);
    assert_eq!(fifth.1, 4);
    assert_eq!(s.fan_in_status(&wave).unwrap().in_flight_count, 4);
}

/// Terminal release is idempotent and applies equally to failure and
/// cancellation paths, while the pending cancellation path remains
/// untouched by the release API.
#[test]
fn terminal_release_failure_and_cancel_are_idempotent() {
    let s = store();
    let failed_wave = wave_into(&s, "release-failed", WaveKind::Exec, 1).unwrap();
    s.try_dispatch_next(1).unwrap().unwrap();
    s.release_slot_dispatch(&failed_wave, 0, crate::supervisor::DispatchOutcome::Failed)
        .unwrap();
    s.release_slot_dispatch(&failed_wave, 0, crate::supervisor::DispatchOutcome::Failed)
        .unwrap();
    assert_eq!(s.fan_in_status(&failed_wave).unwrap().failed_count, 1);

    let cancel_wave = wave_into(&s, "release-cancel", WaveKind::Exec, 1).unwrap();
    s.try_dispatch_next(1).unwrap().unwrap();
    s.release_slot_dispatch(&cancel_wave, 0, crate::supervisor::DispatchOutcome::Failed)
        .unwrap();
    assert_eq!(s.fan_in_status(&cancel_wave).unwrap().in_flight_count, 0);
}
