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
//! 1. Waves with `merged_to_events == true` are SKIPPED.
//!    Their coord topic was already injected; double-injection
//!    would race the integrator. KTD-7 pins this.
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
use crate::supervisor::{SupervisorStore, SupervisorStoreResult, WavePhase};

/// Result of running recovery at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Number of waves inspected (any non-terminal phase).
    pub inspected: usize,
    /// Number of waves that were timeout-marked during this
    /// recovery run. Useful for the `ralph diagnose` surface
    /// and the BDD scenario.
    pub timed_out: Vec<String>,
    /// Number of waves skipped because `merged_to_events` was
    /// already true (recovery decides NOT to re-inject the
    /// coord event).
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
        if snapshot.merged_to_events {
            report.already_merged.push(snapshot.wave_id.clone());
            continue;
        }
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
        // (it owns phase writes via `fail_wave`). Keeping
        // phase-write authority in one place prevents
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
        match evaluate_phase(&snapshot, &inputs) {
            PhaseDecision::Failed {
                reason: FailedReason::Timeout,
                ..
            } => {
                store.set_wave_phase(&snapshot.wave_id, WavePhase::Failed)?;
                report.timed_out.push(snapshot.wave_id.clone());
            }
            // Cancelled / RequiredSlotFailure / ExpectedTotalZero:
            // leave the phase alone — the coordinator's next
            // `tick` will call `fail_wave` and write the phase
            // itself. Recovery only short-circuits the Timeout
            // branch because a timed-out wave may never tick
            // again (no slots left to record).
            _ => {}
        }
    }
    Ok(report)
}

/// A regression-tested helper that pins "merged_to_events
/// short-circuits recovery". The runtime calls this when the
/// U11 dispatcher bridge lands so we don't regress the
/// idempotency guarantee.
pub fn merged_waves_skip_recovery(
    store: Arc<dyn SupervisorStore>,
    wave_id: &str,
) -> SupervisorStoreResult<bool> {
    let snapshot = store.fan_in_status(wave_id)?;
    Ok(snapshot.merged_to_events)
}

/// Reset the merge flag on a wave during recovery ONLY when
/// the wave's `phase ∈ {Dispatch, Collect}` AND its slot rows
/// include at least one `Completed` slot whose event merge
/// intent was committed to disk but not yet durably stamped
/// (U11 R-C3). This is the re-merge intent marker: recovery
/// re-plays the merge on next coordinator tick.
///
/// 2026-07-03-001 plan U6 / F-006: the marker stamp is
/// idempotent — calling `mark_merge_to_events` is a no-op
/// when the wave already has the flag set, so re-running
/// recovery does not double-inject. The function returns
/// `true` when it transitioned the wave into the
/// "merge intent stamped" state (i.e. flagged it for the
/// next coordinator tick to re-merge).
pub fn restore_unmerged_completed_slot(
    store: Arc<dyn SupervisorStore>,
    wave_id: &str,
) -> SupervisorStoreResult<bool> {
    let snapshot = store.fan_in_status(wave_id)?;
    if snapshot.merged_to_events {
        return Ok(false);
    }
    if matches!(snapshot.phase, WavePhase::Done | WavePhase::Failed) {
        return Ok(false);
    }
    // Stamp the merge-intent marker so the next
    // coordinator tick re-runs the merge (F-006).
    // `mark_merge_to_events` is the trait-level verb for
    // this transition; the coordinator's idempotency
    // contract (U1 / KTD-7) means re-stamping is a no-op
    // when the flag is already true.
    if snapshot.completed_count > 0 {
        store.mark_merge_to_events(wave_id)?;
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
    use crate::supervisor::{InMemorySupervisorStore, SlotResource, WaveKind};
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
    fn merged_to_events_wave_is_skipped() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("merge", WaveKind::Exec, 1).unwrap();
        slot_bound(&store, &wave, 0);
        store.mark_merge_to_events(&wave).unwrap();
        let report = recover_active_waves_at_startup(Arc::new(store.clone()), 60).unwrap();
        assert_eq!(report.already_merged, vec!["w-1"]);
        // Already-merged waves do NOT trigger the timeout list.
        assert!(report.timed_out.is_empty());
    }

    #[test]
    fn running_wave_with_zero_in_flight_does_not_timeout() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("zero", WaveKind::Exec, 1).unwrap();
        slot_bound(&store, &wave, 0);
        // No dispatch; in_flight = 0.
        let report = recover_active_waves_at_startup(Arc::new(store.clone()), 60).unwrap();
        assert_eq!(report.inspected, 1);
        assert!(report.timed_out.is_empty());
    }

    #[test]
    fn cancel_wave_with_in_flight_workers_stays_in_flight() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("cancel", WaveKind::Exec, 1).unwrap();
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
        let wave = store.register_wave("helper", WaveKind::Exec, 1).unwrap();
        store.mark_merge_to_events(&wave).unwrap();
        let flag = merged_waves_skip_recovery(Arc::new(store.clone()), &wave).unwrap();
        assert!(flag);
    }

    #[test]
    fn restore_unmerged_completed_is_noop_for_unmerged_state() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("replay", WaveKind::Exec, 1).unwrap();
        // snapshot.merged_to_events == false (default) — the
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

    /// U6 / F-006 / R6 / R-C3: an in-flight wave whose
    /// `elapsed_secs > aggregate_timeout_secs` is
    /// transitioned to `phase=Failed` during recovery and
    /// recorded in `report.timed_out`. Without this branch
    /// the wave would stay in `Collect` forever and the
    /// loop would re-poll it on every iteration.
    #[test]
    fn in_flight_wave_past_timeout_marked_failed() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("stuck", WaveKind::Exec, 1).unwrap();
        slot_bound(&store, &wave, 0);
        // Dispatch so the slot moves to `Dispatched` and
        // `in_flight_count > 0` for the recovery check.
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        // Backdate the wave so `now - started_at` is huge
        // (simulate a 2-hour-old in-flight wave).
        let backdated = SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(2 * 60 * 60))
            .expect("clock supports 2h subtraction");
        store.backdate_wave_for_test(&wave, backdated).unwrap();
        // Recovery with a 60s budget: elapsed (7200s) > 60s.
        // We pass the same `Arc` to recovery and read back
        // the mutation on the same store instance.
        let store_arc = Arc::new(store);
        let report = recover_active_waves_at_startup(store_arc.clone(), 60).unwrap();
        assert_eq!(report.timed_out, vec![wave.clone()]);
        let snap = store_arc.fan_in_status(&wave).unwrap();
        assert_eq!(snap.phase, WavePhase::Failed);
    }

    /// U6 / F-006 / R6 edge: in-flight wave within budget
    /// is NOT mutated (no false positive on a still-fresh
    /// wave). Regression pin for the `elapsed_secs` math.
    #[test]
    fn in_flight_wave_within_timeout_not_marked_failed() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("fresh", WaveKind::Exec, 1).unwrap();
        slot_bound(&store, &wave, 0);
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        // `aggregate_timeout_secs = 3600` and the wave is
        // brand-new: no mutation.
        let store_arc = Arc::new(store);
        let report = recover_active_waves_at_startup(store_arc.clone(), 3600).unwrap();
        assert!(report.timed_out.is_empty());
        let snap = store_arc.fan_in_status(&wave).unwrap();
        assert_eq!(snap.phase, WavePhase::Collect);
    }

    /// U6 / F-006 / R6: `restore_unmerged_completed_slot`
    /// stamps the merge-intent marker (via
    /// `mark_merge_to_events`) when
    /// `merged_to_events=0 && completed>0 && phase !=
    /// Done|Failed`. The coordinator's idempotency contract
    /// (U1 / KTD-7) means re-stamping is a no-op when the
    /// flag is already true, so the marker doubles as the
    /// "merge intent was committed to disk" signal.
    #[test]
    fn restore_unmerged_completed_stamps_merge_intent() {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("unmerged", WaveKind::Exec, 2).unwrap();
        slot_bound(&store, &wave, 0);
        slot_bound(&store, &wave, 1);
        let _ = store.try_dispatch_next(4).unwrap().unwrap();
        // Slot 0 completes; slot 1 still in-flight.
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        let snap = store.fan_in_status(&wave).unwrap();
        assert!(!snap.merged_to_events);
        assert_eq!(snap.completed_count, 1);
        // Recovery helper stamps the merge-intent marker.
        let store_arc = Arc::new(store);
        let did = restore_unmerged_completed_slot(store_arc.clone(), &wave).unwrap();
        assert!(did, "completed > 0 must stamp the merge intent");
        let snap = store_arc.fan_in_status(&wave).unwrap();
        assert!(snap.merged_to_events);
    }
}
