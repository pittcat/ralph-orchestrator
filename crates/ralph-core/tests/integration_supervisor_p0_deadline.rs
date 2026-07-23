//! 2026-07-23-004 plan U6 (R-A5): deadline arbitration
//! parity test between the in-memory and rusqlite stores.
//!
//! The plan establishes that:
//! - `now == deadline` is the boundary: terminal events that
//!   commit at the boundary are still accepted; only
//!   `now > deadline` allows the timeout to fire.
//! - The aggregate epoch starts at the durable registration
//!   commit. The store MUST report `started_at` consistently
//!   between Memory and SQLite for the same wave.
//! - Per-worker epoch starts at the running transition commit
//!   (not from dispatch / queue). The store records it the
//!   same way both backends.

#[cfg(feature = "supervisor-db")]
use ralph_core::supervisor::RusqliteSupervisorStore;
use ralph_core::supervisor::SupervisorStore;
use ralph_core::supervisor::{InMemorySupervisorStore, WaveKind};

use tempfile::TempDir;

/// Boundary check: `evaluate_phase` must NOT fire
/// `aggregate_timeout` when `elapsed_secs == aggregate_timeout_secs`.
/// The deadline race R-A5 requires the terminal to win at
/// `now == deadline` (strict `>` semantics).
#[test]
fn evaluate_phase_now_equals_deadline_still_allows_integrate() {
    use ralph_core::supervisor::phase::{PhaseInputs, evaluate_phase};
    use ralph_core::supervisor::{SlotStatus, WaveSnapshot};

    let mut wave = WaveSnapshot {
        wave_id: "w-deadline-boundary".into(),
        kind: WaveKind::Exec,
        phase: ralph_core::supervisor::WavePhase::Dispatch,
        expected_total: 1,
        pending_count: 0,
        in_flight_count: 0,
        completed_count: 1,
        failed_count: 0,
        slots: vec![(0, SlotStatus::Completed)].into_iter().collect(),
        cancel_requested: false,
        merged_to_events: false,
        started_at: std::time::SystemTime::now(),
    };

    let decision = evaluate_phase(
        &wave,
        &PhaseInputs {
            aggregate_timeout_secs: 60,
            elapsed_secs: 60,
            cancel_requested: false,
        },
    );
    assert!(
        matches!(
            decision,
            ralph_core::supervisor::phase::PhaseDecision::Integrate
        ),
        "elapsed_secs == aggregate_timeout_secs must still allow Integrate (no timeout fire), got {decision:?}"
    );

    // One second past the deadline does fire Timeout.
    wave.completed_count = 0;
    wave.failed_count = 1;
    wave.slots = vec![(0, SlotStatus::Failed)].into_iter().collect();
    let decision2 = evaluate_phase(
        &wave,
        &PhaseInputs {
            aggregate_timeout_secs: 60,
            elapsed_secs: 61,
            cancel_requested: false,
        },
    );
    assert!(
        matches!(
            decision2,
            ralph_core::supervisor::phase::PhaseDecision::Failed {
                reason: ralph_core::supervisor::phase::FailedReason::Timeout,
                ..
            }
        ),
        "elapsed_secs > aggregate_timeout_secs must fire Timeout, got {decision2:?}"
    );
}

/// Both backends record the same wave shape. The rusqlite
/// branch is gated on the feature so the test still passes
/// without the feature.
#[cfg(feature = "supervisor-db")]
#[test]
fn memory_and_rusqlite_record_consistent_wave_shape() {
    let mem = InMemorySupervisorStore::new();
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join(".ralph/supervisor.db");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let sql = RusqliteSupervisorStore::open(&path).unwrap();

    let mem_id = mem.register_wave("parity", WaveKind::Exec, 3).unwrap();
    let sql_id = sql.register_wave("parity", WaveKind::Exec, 3).unwrap();

    // 2026-07-23-004 U6: both stores allocate distinct
    // `w-{seq}` ids; we do NOT require identical values (the
    // sqlite store uses AUTOINCREMENT and the memory store
    // uses an in-process counter). What we require is that
    // both stores successfully resolve the SAME public
    // idempotency key back to THEIR respective store ids.
    assert_eq!(mem_id, "w-1");
    assert!(!sql_id.is_empty());

    let mem_resolved = mem.wave_id_for_idempotency_key("parity").unwrap();
    let sql_resolved = sql.wave_id_for_idempotency_key("parity").unwrap();
    assert_eq!(mem_resolved.as_deref(), Some(mem_id.as_str()));
    assert_eq!(sql_resolved.as_deref(), Some(sql_id.as_str()));

    // Both stores produce a snapshot with the same expected_total.
    let mem_snap = mem.fan_in_status(&mem_id).unwrap();
    let sql_snap = sql.fan_in_status(&sql_id).unwrap();
    assert_eq!(mem_snap.expected_total, sql_snap.expected_total);
    assert_eq!(mem_snap.expected_total, 3);
}

/// `cancel_requested` short-circuits any timeout race:
/// the cancel flag wins before the deadline is checked.
#[test]
fn cancel_before_timeout_fires_cancelled_not_timeout() {
    use ralph_core::supervisor::WaveSnapshot;
    use ralph_core::supervisor::phase::{PhaseInputs, evaluate_phase};

    let wave = WaveSnapshot {
        wave_id: "w-cancel-wins".into(),
        kind: WaveKind::Exec,
        phase: ralph_core::supervisor::WavePhase::Failed,
        expected_total: 1,
        pending_count: 0,
        in_flight_count: 0,
        completed_count: 0,
        failed_count: 1,
        slots: vec![(0, ralph_core::supervisor::SlotStatus::Cancelled)]
            .into_iter()
            .collect(),
        cancel_requested: true,
        merged_to_events: false,
        started_at: std::time::SystemTime::now(),
    };

    let decision = evaluate_phase(
        &wave,
        &PhaseInputs {
            aggregate_timeout_secs: 30,
            elapsed_secs: 999, // past timeout
            cancel_requested: true,
        },
    );
    assert!(
        matches!(
            decision,
            ralph_core::supervisor::phase::PhaseDecision::Failed {
                reason: ralph_core::supervisor::phase::FailedReason::Cancelled,
                ..
            }
        ),
        "cancel_requested must preempt timeout race, got {decision:?}"
    );
}
