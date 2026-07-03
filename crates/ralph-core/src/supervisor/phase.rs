//! Wave phase decision pure function (U6).
//!
//! Given a `WaveSnapshot` plus cancel / timeout flags, decide
//! whether the supervisor should advance the wave to `Integrate`,
//! transition it to `Failed`, or stay in `Collect` for another
//! tick. The function is **pure**: no I/O, no store calls, no
//! `SystemTime` reads beyond what the caller passes. The
//! coordinator (U8) wires the function into the fan-in loop.

use crate::supervisor::{SlotStatus, WavePhase, WaveSnapshot};

/// Inputs that supplement `WaveSnapshot` to make the phase
/// decision. `cancel_requested` is duplicated on the snapshot
/// itself, but we keep it explicit here so the pure function's
/// signature documents every input that influences the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseInputs {
    /// Wall-clock budget supplied by `SupervisorConfig::aggregate_timeout_secs`.
    pub aggregate_timeout_secs: u64,
    /// Optional `Instant`-equivalent elapsed seconds; the runtime
    /// computes `now - started_at` and passes it in so the pure
    /// function has no clock dependency.
    pub elapsed_secs: u64,
    /// Cancel flag (mirrors `WaveSnapshot::cancel_requested` so
    /// callers don't have to dig into the snapshot).
    pub cancel_requested: bool,
}

impl Default for PhaseInputs {
    fn default() -> Self {
        // The default budget matches `SupervisorConfig`'s default of
        // 600s; `elapsed_secs == 0` means "no timeout elapsed" and
        // `cancel_requested == false` keeps the wave ticking. U8
        // ticks always overwrite the elapsed field before evaluating,
        // so the default is safe for tests / dry-runs.
        Self {
            aggregate_timeout_secs: 600,
            elapsed_secs: 0,
            cancel_requested: false,
        }
    }
}

/// Per-tick fan-in decision returned by `evaluate_phase`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseDecision {
    /// Stay in `Collect`; more slots still need to finish
    /// (or `pending > 0`, or `in_flight > 0`).
    ContinueCollect,
    /// All required slots reached `Completed`. Advance to
    /// `Integrate`. The coordinator (U8) owns the merge gate.
    Integrate,
    /// Wave cannot complete. Includes the human-readable
    /// reason used in the `task.resume` payload and the slots
    /// whose `Failed` / `Cancelled` status drove the verdict.
    Failed {
        reason: FailedReason,
        /// Slot indices whose status is `Failed` or `Cancelled`;
        /// included so the integrator (U12) can name them in
        /// diagnostics.
        blocking_slots: Vec<u32>,
    },
}

/// Why the wave ended in `Failed`. The string form is what
/// eventually shows up in `*.wave.failed` payloads and
/// `ralph diagnose` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailedReason {
    /// `cancel_requested == true`. Required slot ended up
    /// `Cancelled` or the runtime killed running workers via
    /// PID.
    Cancelled,
    /// `elapsed_secs > aggregate_timeout_secs`. No further
    /// polling — the wave ran out of time.
    Timeout,
    /// At least one required slot is permanently `Failed` —
    /// the rest never caught up. KTD-8 forbids silent partial
    /// completes so this is a real `Failed` (SC2).
    RequiredSlotFailure,
    /// Internal safety net: `expected_total == 0`. Should
    /// never trigger because `register_wave` rejects zero
    /// sizes, but the function stays defensive.
    ExpectedTotalZero,
}

impl FailedReason {
    /// Stable string for logs + `ralph diagnose` reports.
    pub fn as_str(&self) -> &'static str {
        match self {
            FailedReason::Cancelled => "cancelled",
            FailedReason::Timeout => "timeout",
            FailedReason::RequiredSlotFailure => "required_slot_failure",
            FailedReason::ExpectedTotalZero => "expected_total_zero",
        }
    }
}

/// Evaluate a `WaveSnapshot` against the supplied inputs and
/// return the next phase decision. This is the U6 pure
/// function: it does not call the store, does not mutate
/// anything, and is therefore safe to invoke from any test
/// (including those that exercise only the U8 coordinator).
pub fn evaluate_phase(snapshot: &WaveSnapshot, inputs: &PhaseInputs) -> PhaseDecision {
    // Pre-condition guards.
    if snapshot.expected_total == 0 {
        return PhaseDecision::Failed {
            reason: FailedReason::ExpectedTotalZero,
            blocking_slots: Vec::new(),
        };
    }
    // Cancel wins before any other terminal decision. Even if
    // some slots `Completed`, a cancel-requested wave MUST
    // not advance to Integrate — the runtime is tearing down
    // workers (R-B3).
    if inputs.cancel_requested || snapshot.cancel_requested {
        return PhaseDecision::Failed {
            reason: FailedReason::Cancelled,
            blocking_slots: collect_blocking(&snapshot.blocking_slot_indices()),
        };
    }
    // Timeout: the wave ran past its budget. Mark it failed so
    // the coordinator injects `*.wave.failed(reason=timeout)`.
    if inputs.elapsed_secs > inputs.aggregate_timeout_secs {
        return PhaseDecision::Failed {
            reason: FailedReason::Timeout,
            blocking_slots: collect_blocking(&snapshot.blocking_slot_indices()),
        };
    }
    // Fan-in: every slot has reached a terminal state.
    if snapshot.pending_count == 0 && snapshot.in_flight_count == 0 {
        if snapshot.completed_count >= snapshot.expected_total {
            return PhaseDecision::Integrate;
        }
        if snapshot.failed_count > 0 {
            return PhaseDecision::Failed {
                reason: FailedReason::RequiredSlotFailure,
                blocking_slots: collect_blocking(&snapshot.blocking_slot_indices()),
            };
        }
    }
    // Some slots still pending or in flight.
    PhaseDecision::ContinueCollect
}

/// Helper that turns a snapshot-blocking-slice snapshot into
/// owned `Vec<u32>`. The snapshot helper stores a small
/// window; collecting into `Vec<u32>` is the cheapest way to
/// produce an owned payload for the runtime.
fn collect_blocking(iter: &[u32]) -> Vec<u32> {
    iter.to_vec()
}

/// Optional convenience: extension on `WaveSnapshot` so the
/// coordinator doesn't have to re-derive the blocking slice
/// inline. The implementation is hidden behind this module to
/// avoid leaking phase-decision details into the public
/// `supervisor` API.
pub trait WaveSnapshotExt {
    /// Indices of slots whose status is `Failed` or
    /// `Cancelled`. Used by the phase decision to populate
    /// the `blocking_slots` payload.
    fn blocking_slot_indices(&self) -> Vec<u32>;
}

impl WaveSnapshotExt for WaveSnapshot {
    fn blocking_slot_indices(&self) -> Vec<u32> {
        // U3 / F-003: filter the per-slot status list kept on
        // the snapshot (populated by both stores via JOIN in
        // `fan_in_status`). The pre-fix code fabricated a range
        // from `expected_total - completed_count .. expected_total`,
        // which mis-classified legitimately completed slots as
        // blocking. The new contract reads real `Failed` and
        // `Cancelled` statuses from `slots`.
        self.slots
            .iter()
            .filter_map(|(idx, status)| {
                if matches!(status, SlotStatus::Failed | SlotStatus::Cancelled) {
                    Some(*idx)
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::{SlotStatus, WavePhase};

    fn snap(
        expected_total: u32,
        completed: u32,
        failed: u32,
        in_flight: u32,
        pending: u32,
        cancel: bool,
    ) -> (WaveSnapshot, PhaseInputs) {
        let s = WaveSnapshot {
            wave_id: "w-1".into(),
            kind: crate::supervisor::WaveKind::Exec,
            phase: WavePhase::Collect,
            expected_total,
            completed_count: completed,
            failed_count: failed,
            pending_count: pending,
            in_flight_count: in_flight,
            cancel_requested: cancel,
            merged_to_events: false,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            // U3 / F-003: snap helper builds a slots vec that
            // mirrors the count aggregate so the
            // pre-fix-range-based blocking_slots call site
            // (the `blocking_slots_index_helper_is_stable`
            // test) keeps its semantic anchor. The new
            // U3 tests build their own `slots` explicitly.
            slots: Vec::new(),
        };
        let i = PhaseInputs {
            aggregate_timeout_secs: 60,
            elapsed_secs: 0,
            cancel_requested: cancel,
        };
        (s, i)
    }

    #[test]
    fn all_completed_advances_to_integrate() {
        let (s, i) = snap(3, 3, 0, 0, 0, false);
        assert_eq!(evaluate_phase(&s, &i), PhaseDecision::Integrate);
    }

    #[test]
    fn one_failed_with_rest_completed_yields_failed() {
        let (s, i) = snap(4, 3, 1, 0, 0, false);
        let decision = evaluate_phase(&s, &i);
        match decision {
            PhaseDecision::Failed {
                reason: FailedReason::RequiredSlotFailure,
                blocking_slots,
            } => {
                assert!(!blocking_slots.is_empty(), "blocking_slots must list the failed slot");
            }
            other => panic!("expected RequiredSlotFailure, got {other:?}"),
        }
    }

    #[test]
    fn partial_complete_stays_in_collect() {
        let (s, i) = snap(4, 2, 0, 1, 1, false);
        assert_eq!(evaluate_phase(&s, &i), PhaseDecision::ContinueCollect);
    }

    #[test]
    fn cancel_requested_short_circuits_to_failed() {
        let (s, mut i) = snap(4, 4, 0, 0, 0, true);
        i.cancel_requested = true;
        match evaluate_phase(&s, &i) {
            PhaseDecision::Failed {
                reason: FailedReason::Cancelled,
                ..
            } => {}
            other => panic!("expected Cancelled, got {other:?}"),
        }
    }

    #[test]
    fn timeout_yields_failed_with_timeout_reason() {
        let (s, mut i) = snap(4, 1, 0, 1, 2, false);
        i.elapsed_secs = 61;
        match evaluate_phase(&s, &i) {
            PhaseDecision::Failed {
                reason: FailedReason::Timeout,
                ..
            } => {}
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn two_of_four_complete_one_failed_one_in_flight_is_partial_failure() {
        let (s, i) = snap(4, 2, 1, 1, 0, false);
        // pending_count and failed_count both > 0, but there
        // are still in-flight slots; the function must wait
        // until every slot is terminal. KTD-8 keeps the wave
        // pending until in_flight drains, so this is NOT a
        // partial complete.
        assert_eq!(evaluate_phase(&s, &i), PhaseDecision::ContinueCollect);
    }

    #[test]
    fn expected_total_zero_yields_internal_failure() {
        let (s, i) = snap(0, 0, 0, 0, 0, false);
        match evaluate_phase(&s, &i) {
            PhaseDecision::Failed {
                reason: FailedReason::ExpectedTotalZero,
                ..
            } => {}
            other => panic!("expected ExpectedTotalZero, got {other:?}"),
        }
    }

    /// U3 / F-003 / KTD-8 contract pin: `blocking_slot_indices`
    /// MUST read per-slot status, not fabricate a range from
    /// `expected_total - completed_count`. The snapshot's
    /// `slots` field carries real slot status; the helper
    /// filters for `Failed` (and `Cancelled`, per F-003 / U3).
    #[test]
    fn blocking_slot_indices_reads_real_status() {
        // total=4, completed=2, failed=1, in_flight=0,
        // pending=1. The failed slot is index 1 (a real
        // value carried on the snapshot, not a range).
        let slot_index_of_failed = 1u32;
        let snap = WaveSnapshot {
            wave_id: "w-real".into(),
            kind: crate::supervisor::WaveKind::Exec,
            phase: WavePhase::Collect,
            expected_total: 4,
            completed_count: 2,
            failed_count: 1,
            pending_count: 1,
            in_flight_count: 0,
            cancel_requested: false,
            merged_to_events: false,
            // U3: pop the slot list. The failed slot is
            // index 1; the completed + pending ones do NOT
            // appear in `blocking_slot_indices`.
            started_at: std::time::SystemTime::UNIX_EPOCH,
            slots: vec![
                (0, SlotStatus::Completed),
                (1, SlotStatus::Failed),
                (2, SlotStatus::Completed),
                (3, SlotStatus::Pending),
            ],
        };
        let blocking = snap.blocking_slot_indices();
        assert_eq!(
            blocking,
            vec![slot_index_of_failed],
            "blocking slots must list only the failed slot, got {blocking:?}"
        );
    }

    /// U3 / F-003 edge: every slot in `Failed` → blocking
    /// returns all indices. The pre-fix range fabrication
    /// would have produced a different (wrong) answer.
    #[test]
    fn blocking_slot_indices_all_failed_returns_all_indices() {
        let snap = WaveSnapshot {
            wave_id: "w-all-failed".into(),
            kind: crate::supervisor::WaveKind::Exec,
            phase: WavePhase::Collect,
            expected_total: 3,
            completed_count: 0,
            failed_count: 3,
            pending_count: 0,
            in_flight_count: 0,
            cancel_requested: false,
            merged_to_events: false,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            slots: vec![
                (0, SlotStatus::Failed),
                (1, SlotStatus::Failed),
                (2, SlotStatus::Failed),
            ],
        };
        let blocking = snap.blocking_slot_indices();
        assert_eq!(blocking, vec![0, 1, 2]);
    }

    /// U3 / F-003 negative: no `Failed` slots → empty
    /// blocking list (regardless of `pending_count`).
    #[test]
    fn blocking_slot_indices_no_failures_is_empty() {
        let snap = WaveSnapshot {
            wave_id: "w-all-completed".into(),
            kind: crate::supervisor::WaveKind::Exec,
            phase: WavePhase::Collect,
            expected_total: 2,
            completed_count: 2,
            failed_count: 0,
            pending_count: 0,
            in_flight_count: 0,
            cancel_requested: false,
            merged_to_events: false,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            slots: vec![
                (0, SlotStatus::Completed),
                (1, SlotStatus::Completed),
            ],
        };
        let blocking = snap.blocking_slot_indices();
        assert!(blocking.is_empty(), "no failures → no blocking slots: {blocking:?}");
    }

    #[test]
    fn failed_reason_strings_match_required_topics() {
        // `ralph diagnose` greps for these strings; if any
        // change, the BDD scenario (U13) needs to follow.
        assert_eq!(FailedReason::Cancelled.as_str(), "cancelled");
        assert_eq!(FailedReason::Timeout.as_str(), "timeout");
        assert_eq!(
            FailedReason::RequiredSlotFailure.as_str(),
            "required_slot_failure"
        );
        assert_eq!(
            FailedReason::ExpectedTotalZero.as_str(),
            "expected_total_zero"
        );
    }

    #[test]
    fn no_decision_loses_information() {
        // Defensive: a snapshot that has *all* completed but
        // also has failed slots (impossible under the in-mem
        // store's lifecycle, but possible after manual SQL
        // edits) MUST still resolve to the failed path rather
        // than silently integrate (KTD-8 partial = fail).
        let (s, i) = snap(4, 3, 1, 0, 0, false);
        let decision = evaluate_phase(&s, &i);
        assert!(matches!(
            decision,
            PhaseDecision::Failed {
                reason: FailedReason::RequiredSlotFailure,
                ..
            }
        ));
    }

    #[test]
    fn slot_status_round_trip_does_not_collide() {
        // U3/U4 added `in_flight` to the snapshot. Make sure
        // the U6 evaluator never confuses `pending_count`
        // and `in_flight_count`. A snapshot with both > 0
        // must stay ContinueCollect.
        let snap_struct = WaveSnapshot {
            wave_id: "w-mix".into(),
            kind: crate::supervisor::WaveKind::Exec,
            phase: WavePhase::Collect,
            expected_total: 5,
            completed_count: 1,
            failed_count: 0,
            pending_count: 2,
            in_flight_count: 2,
            cancel_requested: false,
            merged_to_events: false,
            started_at: std::time::SystemTime::UNIX_EPOCH,
            slots: Vec::new(),
        };
        let inputs = PhaseInputs {
            aggregate_timeout_secs: 60,
            elapsed_secs: 0,
            cancel_requested: false,
        };
        assert_eq!(
            evaluate_phase(&snap_struct, &inputs),
            PhaseDecision::ContinueCollect
        );
        // And when both pending + in_flight drain, integrate.
        let mut finalize = snap_struct.clone();
        finalize.pending_count = 0;
        finalize.in_flight_count = 0;
        finalize.completed_count = 5;
        assert_eq!(
            evaluate_phase(&finalize, &inputs),
            PhaseDecision::Integrate
        );
        let _ = SlotStatus::Pending; // ensures the import is used in tests.
    }
}
