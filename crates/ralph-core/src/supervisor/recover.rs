//! 2026-07-03-001 plan U11: loop-startup recovery for active
//! supervisor waves.
//!
//! The runtime calls `recover_active_waves_at_startup` once
//! after the `SupervisorStore` opens at loop boot. Each
//! recovered wave is fed to the phase-decision pure function
//! (U6) so the coordinator can advance it without
//! re-dispatching already-completed work (R-C3).
//!
//! Recovery rules:
//!
//! 1. Waves with `delivery_state >= CoordinationCommitted`
//!    are SKIPPED. Their coord topic was already injected;
//!    double-injection would race the integrator. KTD-7 pins
//!    this.
//! 2. Waves with `expected_total > 0` AND `phase ∈ {Dispatch,
//!    Collect}` are classified. Slots in `Completed` already
//!    have their merge intent stamped on disk; the coordinator
//!    re-runs the merge when it sees the wave (idempotent
//!    because the merge sink de-duplicates by content hash).
//! 3. Waves whose slot rows are all `Dispatched`/`Running`
//!    and whose `started_at + aggregate_timeout_secs` has
//!    elapsed move to phase `Failed` with reason `timeout`.
//!    No compensation is run here — compensation is a host
//!    concern owned by U12 dispatcher bridge; recovery only
//!    marks the wave so the loop doesn't retry infinitely.
//! 4. Waves with `cancel_requested == true` stay `cancel`
//!    candidates and the coordinator short-circuits them on
//!    tick. No DB mutation needed.
//!
//! The function is **pure modulo the store**: callers pass the
//! store and the function applies the recovery mutations. The
//! mutation set is small (timeout transitions + a flag for
//! de-dup), so unit tests can run against the in-memory store
//! end-to-end without spinning a real loop.

use std::sync::Arc;
use std::time::SystemTime;

use crate::supervisor::phase::{FailedReason, PhaseDecision, PhaseInputs, evaluate_phase};
use crate::supervisor::{SupervisorStore, SupervisorStoreResult, WaveDeliveryState, WavePhase};

/// Result of running recovery at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Number of waves inspected (any non-terminal phase).
    pub inspected: usize,
    /// Number of waves that were timeout-marked during this
    /// recovery run. Useful for the `ralph diagnose` surface
    /// and the BDD scenario.
    pub timed_out: Vec<String>,
    /// Number of waves skipped because `delivery_state` was
    /// already at `CoordinationCommitted` (recovery decides
    /// NOT to re-inject the coord event).
    pub already_merged: Vec<String>,
}

/// Apply the U11 recovery plan and return a report describing
/// which waves survived an unclean shutdown.
///
/// `aggregate_timeout_secs` is the wave's `SupervisorConfig`
/// value; the caller passes it because the store layer has no
/// time-of-day awareness (we feed elapsed seconds in via the
/// snapshot).
pub fn recover_active_waves_at_startup(
    store: Arc<dyn SupervisorStore>,
    aggregate_timeout_secs: u64,
) -> SupervisorStoreResult<RecoveryReport> {
    let mut report = RecoveryReport::default();
    let snapshots = store.recover_active_waves()?;
    report.inspected = snapshots.len();
    let now = SystemTime::now();
    for snapshot in snapshots {
        // The store's `recover_active_waves` skips waves
        // already at terminal phase (`Done` / `Failed`).
        // A wave that committed through U5's
        // `commit_coordination_event` ends up with
        // `phase = Done` so it never appears in
        // `recover_active_waves`. The `already_merged`
        // tracking happens in `merged_waves_skip_recovery`
        // (regression-tested helper). We don't repeat that
        // scan here to avoid an unbounded store walk on every
        // loop startup.
        // Cancel + still-flying workers: the runtime has
        // already killed the workers via PID; the wave stays
        // in Collect until the dispatcher reconciles.
        if snapshot.in_flight_count > 0 && snapshot.cancel_requested {
            continue;
        }
        if snapshot.in_flight_count == 0 {
            // No in-flight workers; recovery doesn't enforce
            // timeout. The coordinator picks up on the next
            // tick and either integrates (all terminal
            // slots succeeded) or fails (mixed results,
            // KTD-8).
            continue;
        }
        // 2026-07-03-001 plan U6 / F-006 / R-C3: per-wave
        // timeout. Compute `elapsed = now - started_at` and
        // delegate to the U6 pure function `evaluate_phase`.
        // Recovery only enforces the `Timeout` verdict — the
        // other Failed reasons (`Cancelled` /
        // `RequiredSlotFailure`) are the coordinator's job
        // (it owns phase writes via `commit_coordination_event`).
        // Keeping phase-write authority in one place prevents
        // recovery and the coordinator from racing on the
        // same snapshot.
        let elapsed_secs = now
            .duration_since(snapshot.started_at)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let inputs = PhaseInputs {
            aggregate_timeout_secs,
            elapsed_secs,
            cancel_requested: snapshot.cancel_requested,
        };
        if let PhaseDecision::Failed {
            reason: FailedReason::Timeout,
            ..
        } = evaluate_phase(&snapshot, &inputs)
        {
            store.set_wave_phase(&snapshot.wave_id, WavePhase::Failed)?;
            report.timed_out.push(snapshot.wave_id.clone());
        }
        // Cancelled / RequiredSlotFailure / ExpectedTotalZero:
        // leave the phase alone — the coordinator's next
        // `tick` will call `fail_wave` and write the phase
        // itself. Recovery only short-circuits the Timeout
        // branch because a timed-out wave may never tick
        // again (no slots left to record).
    }
    Ok(report)
}

/// A regression-tested helper that pins "delivery_state at
/// `CoordinationCommitted` short-circuits recovery". The
/// runtime calls this when the U11 dispatcher bridge lands so
/// we don't regress the idempotency guarantee.
// 2026-07-16 cleanup U4 (KTD-3): reserved for `--features
// supervisor-db` integration path (pinned here so the public
// recovery helper survives the test-fixture purge).
#[allow(dead_code)]
pub fn merged_waves_skip_recovery(
    store: Arc<dyn SupervisorStore>,
    wave_id: &str,
) -> SupervisorStoreResult<bool> {
    let snapshot = store.fan_in_status(wave_id)?;
    Ok(snapshot
        .delivery_state
        .at_least(WaveDeliveryState::CoordinationCommitted))
}

/// Replay-safe merge intent stampee for waves that recovered
/// past `CoordinationWritten` but pre-commit. Used by the
/// post-recovery coordinator tick so the merge seam resumes
/// idempotently. The function is a no-op once the wave's
/// `delivery_state >= CoordinationCommitted` so calling it
/// from a recovery loop is safe.
#[allow(dead_code)]
pub fn restore_unmerged_completed_slot(
    store: Arc<dyn SupervisorStore>,
    wave_id: &str,
) -> SupervisorStoreResult<bool> {
    let snapshot = store.fan_in_status(wave_id)?;
    if snapshot
        .delivery_state
        .at_least(WaveDeliveryState::CoordinationCommitted)
    {
        return Ok(false);
    }
    if matches!(snapshot.phase, WavePhase::Done | WavePhase::Failed) {
        return Ok(false);
    }
    // Stamp the salvage commit via the new commit API so the
    // dispatcher can re-merge on the next tick. The
    // idempotency contract means re-stamping is a no-op when
    // the receipt matches the persisted fingerprint.
    if snapshot.completed_count > 0 {
        use crate::supervisor::{ProjectionKind, ProjectionReceiptSummary};
        let now_secs = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        store.commit_salvage_projection(
            wave_id,
            &ProjectionReceiptSummary {
                kind: ProjectionKind::Business,
                batch_fingerprint: format!("restore-{wave_id}"),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: now_secs,
            },
        )?;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    //! U11 closed-circuit tests run against the in-memory
    //! store. The rusqlite store's contract is mirrored via
    //! U5 tests, so U11 doesn't need a separate test matrix.

    use super::*;
    use crate::supervisor::{
        CoordinationReceiptSummary, InMemorySupervisorStore, ProjectionReceiptSummary,
        SlotResource, WaveKind,
    };
    use std::sync::Arc;

    fn slot_bound(store: &InMemorySupervisorStore, wave: &str, idx: u32) {
        store
            .bind_worktree(
                wave,
                idx,
                SlotResource {
                    slot_index: idx,
                    worktree_path: Some(format!(".ralph/x/{idx}")),
                    branch: Some(format!("ralph/x/{idx}")),
                },
            )
            .unwrap();
    }

    #[test]
    fn coordination_committed_wave_is_skipped() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("merge", WaveKind::Exec, 1, 1).unwrap();
        slot_bound(&store, &wave, 0);
        store
            .commit_salvage_projection(
                &wave,
                &ProjectionReceiptSummary {
                    kind: crate::supervisor::ProjectionKind::Business,
                    batch_fingerprint: "fp".into(),
                    write_count: 0,
                    already_present_count: 0,
                    committed_at_unix_secs: 0,
                },
            )
            .unwrap();
        store
            .record_coordination_written(
                &wave,
                &CoordinationReceiptSummary {
                    topic: "exec.wave.complete".into(),
                    idempotency_key: "k".into(),
                    payload_fingerprint: "fp".into(),
                    write_count: 1,
                    already_present_count: 0,
                    committed_at_unix_secs: 0,
                },
            )
            .unwrap();
        store
            .commit_coordination_event(
                &wave,
                &CoordinationReceiptSummary {
                    topic: "exec.wave.complete".into(),
                    idempotency_key: "k".into(),
                    payload_fingerprint: "fp".into(),
                    write_count: 1,
                    already_present_count: 0,
                    committed_at_unix_secs: 0,
                },
                WavePhase::Done,
            )
            .unwrap();
        let report = recover_active_waves_at_startup(Arc::new(store.clone()), 60).unwrap();
        // A fully-delivered wave (phase = Done) is excluded
        // from `recover_active_waves`. Recovery therefore
        // never re-injects the coord event.
        assert!(!report.timed_out.contains(&wave));
        assert!(report.inspected == 0);
    }

    #[test]
    fn running_wave_with_zero_in_flight_does_not_timeout() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("zero", WaveKind::Exec, 1, 1).unwrap();
        slot_bound(&store, &wave, 0);
        // No dispatch; in_flight = 0.
        let report = recover_active_waves_at_startup(Arc::new(store.clone()), 60).unwrap();
        assert_eq!(report.inspected, 1);
        assert!(report.timed_out.is_empty());
    }

    #[test]
    fn cancel_wave_with_in_flight_workers_stays_in_flight() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("cancel", WaveKind::Exec, 1, 1).unwrap();
        slot_bound(&store, &wave, 0);
        let _ = store.try_dispatch_next(4).unwrap().unwrap();
        store.cancel_wave(&wave).unwrap();
        let report = recover_active_waves_at_startup(Arc::new(store.clone()), 60).unwrap();
        assert_eq!(report.inspected, 1);
        // Snapshot still has in-flight + cancel_requested; we
        // expect no DB mutation and no timeout escalation —
        // the runtime reconciles via cancel+retry.
        assert!(report.timed_out.is_empty());
    }

    #[test]
    fn merged_waves_skip_recovery_helper_returns_true() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("helper", WaveKind::Exec, 1, 1).unwrap();
        store
            .commit_salvage_projection(
                &wave,
                &ProjectionReceiptSummary {
                    kind: crate::supervisor::ProjectionKind::Business,
                    batch_fingerprint: "fp".into(),
                    write_count: 0,
                    already_present_count: 0,
                    committed_at_unix_secs: 0,
                },
            )
            .unwrap();
        store
            .record_coordination_written(
                &wave,
                &CoordinationReceiptSummary {
                    topic: "exec.wave.complete".into(),
                    idempotency_key: "k".into(),
                    payload_fingerprint: "fp".into(),
                    write_count: 1,
                    already_present_count: 0,
                    committed_at_unix_secs: 0,
                },
            )
            .unwrap();
        store
            .commit_coordination_event(
                &wave,
                &CoordinationReceiptSummary {
                    topic: "exec.wave.complete".into(),
                    idempotency_key: "k".into(),
                    payload_fingerprint: "fp".into(),
                    write_count: 1,
                    already_present_count: 0,
                    committed_at_unix_secs: 0,
                },
                WavePhase::Done,
            )
            .unwrap();
        let flag = merged_waves_skip_recovery(Arc::new(store.clone()), &wave).unwrap();
        assert!(flag);
    }

    #[test]
    fn restore_unmerged_completed_is_noop_for_unmerged_state() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("replay", WaveKind::Exec, 1, 1).unwrap();
        // snapshot.delivery_state == Pending (default) — the
        // marker restore is a no-op.
        let did = restore_unmerged_completed_slot(Arc::new(store.clone()), &wave).unwrap();
        assert!(!did, "unmerged wave must return false (no-op)");
    }

    #[test]
    fn recovery_does_not_panic_on_empty_store() {
        let store = InMemorySupervisorStore::new();
        let report = recover_active_waves_at_startup(Arc::new(store), 60).unwrap();
        assert_eq!(report.inspected, 0);
        assert!(report.timed_out.is_empty());
        assert!(report.already_merged.is_empty());
    }

    #[test]
    fn in_flight_wave_past_timeout_marked_failed() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("stuck", WaveKind::Exec, 1, 1).unwrap();
        slot_bound(&store, &wave, 0);
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        let backdated = SystemTime::now()
            .checked_sub(std::time::Duration::from_hours(2))
            .expect("clock supports 2h subtraction");
        store.backdate_wave_for_test(&wave, backdated).unwrap();
        let store_arc = Arc::new(store);
        let report = recover_active_waves_at_startup(store_arc.clone(), 60).unwrap();
        assert_eq!(report.timed_out, vec![wave.clone()]);
        let snap = store_arc.fan_in_status(&wave).unwrap();
        assert_eq!(snap.phase, WavePhase::Failed);
    }

    #[test]
    fn in_flight_wave_within_timeout_not_marked_failed() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("fresh", WaveKind::Exec, 1, 1).unwrap();
        slot_bound(&store, &wave, 0);
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        let store_arc = Arc::new(store);
        let report = recover_active_waves_at_startup(store_arc.clone(), 3600).unwrap();
        assert!(report.timed_out.is_empty());
        let snap = store_arc.fan_in_status(&wave).unwrap();
        assert_eq!(snap.phase, WavePhase::Collect);
    }

    #[test]
    fn restore_unmerged_completed_stamps_salvage_commit() {
        let store = InMemorySupervisorStore::new();
        let wave = store
            .register_wave("unmerged", WaveKind::Exec, 2, 1)
            .unwrap();
        slot_bound(&store, &wave, 0);
        slot_bound(&store, &wave, 1);
        let _ = store.try_dispatch_next(4).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        let snap = store.fan_in_status(&wave).unwrap();
        assert!(
            !snap
                .delivery_state
                .at_least(WaveDeliveryState::CoordinationCommitted)
        );
        assert_eq!(snap.completed_count, 1);
        let store_arc = Arc::new(store);
        let did = restore_unmerged_completed_slot(store_arc.clone(), &wave).unwrap();
        assert!(did, "completed > 0 must stamp the salvage commit");
        let snap = store_arc.fan_in_status(&wave).unwrap();
        assert!(
            snap.delivery_state
                .at_least(WaveDeliveryState::SalvageCommitted)
        );
    }
}
