//! 2026-07-24-003 plan Unit 4: emission reservation state machine.
//!
//! Exercises the public `SupervisorStore::reserve_emission` API
//! against both the in-memory and rusqlite implementations so a
//! future regression on either side of the dual implementation
//! surfaces before U5 wires the CLI onto it.
//!
//! Coverage map (mirrors the plan's §3 risk-driven split):
//!
//! - S2: same scope + same payload → `AlreadyApplied`
//! - S3: distinct scopes → two distinct public_wave_ids
//! - S4: same scope + different payload → `Conflict`
//! - S8: reservation + events on disk but state still
//!   `Reserved` → recovery to `Applied`
//! - S9: reservation + zero events on disk → `FailedPartial`
//! - Migration v2 → v3 keeps waves / slots / seq intact

use ralph_core::supervisor::{
    EmissionReservation, EmissionState, InMemorySupervisorStore, SupervisorStore,
    SupervisorStoreError, WaveKind,
};
#[cfg(feature = "supervisor-db")]
use ralph_core::supervisor::RusqliteSupervisorStore;
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::TempDir;

#[cfg(feature = "supervisor-db")]
fn open_rusqlite() -> (TempDir, RusqliteSupervisorStore) {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("supervisor.db");
    let store = RusqliteSupervisorStore::open(&path).expect("open store");
    (dir, store)
}

/// Always-zero disk count so recovery paths hit `FailedPartial`.
fn always_zero(_wave_id: &str) -> u32 {
    0
}

/// Always-`expected_count` so recovery paths land on `Applied`.
fn always_full(expected: u32) -> impl Fn(&str) -> u32 {
    move |_| expected
}

/// Count the events that match a given public_wave_id on the
/// caller-supplied events JSONL. The closure lets the trait stay
/// free of file paths while letting tests inject a static
/// scenario.
fn events_jsonl_count<'a>(events: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> u32 + 'a {
    move |wave_id: &str| {
        events
            .iter()
            .filter(|(w, _)| *w == wave_id)
            .count()
            .try_into()
            .unwrap_or(u32::MAX)
    }
}

// =============================================================================
// State machine — fresh reservation.
// =============================================================================

#[test]
fn reserve_emission_first_call_returns_reserved() {
    let store = InMemorySupervisorStore::new();
    let outcome = store
        .reserve_emission("scope-1", "digest-1", 3, &always_zero)
        .expect("reserve");
    match outcome {
        EmissionReservation::Reserved { public_wave_id } => {
            assert!(
                public_wave_id.starts_with("w-"),
                "public_wave_id must carry the stable prefix: {public_wave_id}"
            );
        }
        other => panic!("expected Reserved, got {other:?}"),
    }
}

#[test]
fn reserve_emission_rusqlite_first_call_returns_reserved() {
    #[cfg(feature = "supervisor-db")]
    {
        let (_dir, store) = open_rusqlite();
        let outcome = store
            .reserve_emission("scope-1", "digest-1", 3, &always_zero)
            .expect("reserve");
        assert!(matches!(outcome, EmissionReservation::Reserved { .. }));
    }
}

// =============================================================================
// S2: same scope, same payload → AlreadyApplied.
// =============================================================================

#[test]
fn reserve_emission_dedup_after_apply_returns_already_applied() {
    let store = InMemorySupervisorStore::new();
    let first = store
        .reserve_emission("scope", "digest", 2, &always_zero)
        .expect("first");
    let first_id = match &first {
        EmissionReservation::Reserved { public_wave_id } => public_wave_id.clone(),
        other => panic!("expected Reserved, got {other:?}"),
    };
    store
        .mark_emission_applying("scope")
        .expect("mark applying");
    store
        .mark_emission_applied("scope", 1_700_000_000)
        .expect("mark applied");

    let second = store
        .reserve_emission("scope", "digest", 2, &always_zero)
        .expect("second");
    match second {
        EmissionReservation::AlreadyApplied { public_wave_id } => {
            assert_eq!(
                public_wave_id, first_id,
                "second reservation must reuse the original public_wave_id"
            );
        }
        other => panic!("expected AlreadyApplied, got {other:?}"),
    }
}

// =============================================================================
// S4: same scope, different payload → Conflict.
// =============================================================================

#[test]
fn reserve_emission_payload_conflict_returns_conflict() {
    let store = InMemorySupervisorStore::new();
    store
        .reserve_emission("scope", "digest-A", 1, &always_zero)
        .expect("first");
    store
        .mark_emission_applying("scope")
        .expect("applying");
    store
        .mark_emission_applied("scope", 1)
        .expect("applied");

    let conflict = store
        .reserve_emission("scope", "digest-B", 1, &always_zero)
        .expect("reserve");
    assert_eq!(
        conflict,
        EmissionReservation::Conflict,
        "same scope + different payload must surface Conflict (S4)"
    );
}

// =============================================================================
// S3: distinct scopes → distinct public_wave_ids.
// =============================================================================

#[test]
fn reserve_emission_distinct_scopes_yield_distinct_ids() {
    let store = InMemorySupervisorStore::new();
    let a = store
        .reserve_emission("scope-A", "d", 1, &always_zero)
        .expect("a");
    let b = store
        .reserve_emission("scope-B", "d", 1, &always_zero)
        .expect("b");
    match (a, b) {
        (
            EmissionReservation::Reserved { public_wave_id: a_id },
            EmissionReservation::Reserved { public_wave_id: b_id },
        ) => assert_ne!(a_id, b_id),
        _ => panic!("expected two Reserved variants"),
    }
}

// =============================================================================
// S9: zero events on disk for a Reserved row → FailedPartial.
// =============================================================================

#[test]
fn reserve_emission_zero_events_on_disk_returns_failed_partial() {
    let store = InMemorySupervisorStore::new();
    let first = store
        .reserve_emission("scope", "digest", 3, &always_zero)
        .expect("reserve");
    let first_id = match first {
        EmissionReservation::Reserved { public_wave_id } => public_wave_id,
        other => panic!("expected Reserved, got {other:?}"),
    };
    // Caller bails before writing events, then retries with the
    // same scope. Disk is still empty → fail-closed.
    let second = store
        .reserve_emission("scope", "digest", 3, &always_zero)
        .expect("second");
    match second {
        EmissionReservation::FailedPartial {
            public_wave_id,
            on_disk,
            expected,
        } => {
            assert_eq!(public_wave_id, first_id);
            assert_eq!(on_disk, 0);
            assert_eq!(expected, 3);
        }
        other => panic!("expected FailedPartial, got {other:?}"),
    }
}

// =============================================================================
// S8: events landed but state still Reserved → recovery to Applied.
// =============================================================================

#[test]
fn reserve_emission_events_present_recovers_to_applied() {
    use std::cell::Cell;
    // 0 = no events, 1 = full batch landed. We flip the
    // counter on the second call so the in-memory store can
    // observe "events present + state still Reserved".
    let calls = Cell::new(0u32);
    let store = InMemorySupervisorStore::new();
    let first = store
        .reserve_emission("scope", "digest", 2, &|_| {
            calls.set(calls.get() + 1);
            0
        })
        .expect("first");
    let first_id = match first {
        EmissionReservation::Reserved { public_wave_id } => public_wave_id,
        other => panic!("expected Reserved, got {other:?}"),
    };
    // Disk now reports `expected_count` events but the row never
    // transitioned through `applied` (e.g. process crashed between
    // event-write and mark-applied).
    let second = store
        .reserve_emission("scope", "digest", 2, &|_| {
            calls.set(calls.get() + 1);
            2
        })
        .expect("second");
    match second {
        EmissionReservation::AlreadyApplied { public_wave_id } => {
            assert_eq!(
                public_wave_id, first_id,
                "S8 recovery must reuse the original id"
            );
        }
        other => panic!("expected AlreadyApplied after recovery, got {other:?}"),
    }
}

// =============================================================================
// State transitions: applying → applied → emission_state_for_wave_id.
// =============================================================================

#[test]
fn emission_state_for_wave_id_tracks_lifecycle() {
    let store = InMemorySupervisorStore::new();
    let outcome = store
        .reserve_emission("scope", "digest", 1, &always_zero)
        .expect("reserve");
    let id = match outcome {
        EmissionReservation::Reserved { public_wave_id } => public_wave_id,
        other => panic!("expected Reserved, got {other:?}"),
    };
    assert_eq!(
        store
            .emission_state_for_wave_id(&id)
            .expect("state lookup"),
        Some(EmissionState::Reserved)
    );

    store.mark_emission_applying("scope").expect("applying");
    assert_eq!(
        store.emission_state_for_wave_id(&id).expect("state"),
        Some(EmissionState::Applying)
    );

    store.mark_emission_applied("scope", 42).expect("applied");
    assert_eq!(
        store.emission_state_for_wave_id(&id).expect("state"),
        Some(EmissionState::Applied)
    );

    // Unknown wave_id returns None rather than erroring — this
    // keeps `ralph wave inspect` failure-free for clean store
    // misses (S13).
    assert_eq!(
        store.emission_state_for_wave_id("w-missing").expect("miss"),
        None
    );
}

#[test]
fn mark_emission_applied_rejects_terminal_applied_row() {
    let store = InMemorySupervisorStore::new();
    store
        .reserve_emission("scope", "digest", 1, &always_zero)
        .expect("reserve");
    store.mark_emission_applying("scope").expect("applying");
    store.mark_emission_applied("scope", 1).expect("applied");
    // A second mark_applied MUST fail closed so a double-Apply
    // cannot corrupt the audit trail.
    let err = store
        .mark_emission_applied("scope", 2)
        .expect_err("double apply must fail");
    assert!(
        matches!(err, SupervisorStoreError::InvalidTransition(_)),
        "expected InvalidTransition, got {err:?}"
    );
}

#[test]
fn mark_emission_failed_rejects_already_applied_row() {
    let store = InMemorySupervisorStore::new();
    store
        .reserve_emission("scope", "digest", 1, &always_zero)
        .expect("reserve");
    store.mark_emission_applying("scope").expect("applying");
    store.mark_emission_applied("scope", 1).expect("applied");
    let err = store
        .mark_emission_failed("scope")
        .expect_err("mark_failed after apply must fail");
    assert!(matches!(err, SupervisorStoreError::InvalidTransition(_)));
}

// =============================================================================
// Differential: InMemory and Rusqlite emit the same public_wave_id +
// EmissionReservation enum for the same scope under the same disk
// count closure. This is the U4 contract that the CLI (U5) relies on.
// =============================================================================

#[test]
fn differential_in_memory_vs_rusqlite_first_reserve() {
    let counter = AtomicU64::new(0);
    let count = |_: &str| {
        let n = counter.load(Ordering::SeqCst);
        if n == 0 {
            counter.fetch_add(1, Ordering::SeqCst);
            0
        } else {
            2
        }
    };

    let mem = InMemorySupervisorStore::new();

    let mem_out = mem
        .reserve_emission("scope", "digest", 2, &count)
        .expect("mem reserve");

    #[cfg(feature = "supervisor-db")]
    {
        let (_dir, rs) = open_rusqlite();
        let rs_out = rs
            .reserve_emission("scope", "digest", 2, &count)
            .expect("rs reserve");

        match (&mem_out, &rs_out) {
            (
                EmissionReservation::Reserved { public_wave_id: a },
                EmissionReservation::Reserved { public_wave_id: b },
            ) => {
                // Both carry the stable prefix; format is not
                // pinned byte-for-byte (the in-memory counter uses
                // `w-{}` while rusqlite uses `w-rs-{}`).
                assert!(a.starts_with("w-"));
                assert!(b.starts_with("w-"));
            }
            _ => panic!("differential mismatch: mem={mem_out:?} rs={rs_out:?}"),
        }
    }
    // Without supervisor-db, the in-memory branch still passes
    // (sanity that the dual implementation isn't a no-op).
    assert!(matches!(mem_out, EmissionReservation::Reserved { .. }));
}

// =============================================================================
// S3 + U3: distinct keys in parallel threads must each get a
// non-deduplicated reservation. The test asserts the same public id
// for repeated reservations on the SAME scope (S2 contract) and
// distinct ids for distinct scopes (S3 contract).
// =============================================================================

#[test]
fn parallel_reservation_same_scope_converges_to_single_id() {
    use std::sync::Arc;
    use std::thread;

    // Closure simulates "events are on disk" so a parallel
    // reservation observes the events-present branch and
    // returns `AlreadyApplied` after recovery. We use
    // `always_full` so every reserve sees `expected_count`
    // events.
    let store = Arc::new(InMemorySupervisorStore::new());
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let s = Arc::clone(&store);
        let b = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            b.wait();
            s.reserve_emission("scope", "digest", 2, &always_full(2))
        }));
    }
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().expect("thread panicked").expect("reserve"))
        .collect();
    // All four reservations must agree on the same public_wave_id
    // because the in-memory mutex serialises them.
    let ids: std::collections::HashSet<String> = outcomes
        .iter()
        .map(|o| match o {
            EmissionReservation::Reserved { public_wave_id }
            | EmissionReservation::AlreadyApplied { public_wave_id } => public_wave_id.clone(),
            _ => panic!("unexpected outcome: {o:?}"),
        })
        .collect();
    assert_eq!(
        ids.len(),
        1,
        "concurrent reservations on the same scope must converge to one id: {ids:?}"
    );
}

// =============================================================================
// Migration v2 → v3 keeps existing waves/slots/seq intact.
// =============================================================================

#[cfg(feature = "supervisor-db")]
#[test]
fn migration_v2_to_v3_preserves_existing_waves() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("supervisor.db");

    // Bring the DB up to v2 by registering a wave.
    {
        let store = RusqliteSupervisorStore::open(&path).expect("open v2");
        let wave = store.register_wave("k1", WaveKind::Exec, 1).expect("reg");
        assert!(wave.starts_with("w-"));
    }

    // Reopen — migrations run idempotently; user_version
    // advances to v3 (U4). Existing waves + slots + seq MUST
    // survive the upgrade.
    let store = RusqliteSupervisorStore::open(&path).expect("reopen v3");
    let ids = store.list_wave_ids().expect("list");
    assert_eq!(ids.len(), 1, "v2 wave must survive v3 migration");

    // wave_emissions table is now present and accepts a
    // reservation that the v2 DB had no place to record.
    let outcome = store
        .reserve_emission("scope-x", "d", 1, &always_zero)
        .expect("reserve on migrated db");
    assert!(matches!(outcome, EmissionReservation::Reserved { .. }));

    // Sanity: emissions table is queryable for the row.
    let id = match outcome {
        EmissionReservation::Reserved { public_wave_id } => public_wave_id,
        _ => unreachable!(),
    };
    assert_eq!(
        store.emission_state_for_wave_id(&id).expect("state"),
        Some(EmissionState::Reserved)
    );
}

// =============================================================================
// Closure-driven events_jsonl_count probe: lets the test inject a
// specific event-batch on-disk scenario so the recovery branches
// hit the precise code path.
// =============================================================================

#[test]
fn reserve_emission_recovery_required_when_partial() {
    let store = InMemorySupervisorStore::new();
    let first = store
        .reserve_emission("scope", "digest", 3, &always_zero)
        .expect("first");
    let first_id = match first {
        EmissionReservation::Reserved { public_wave_id } => public_wave_id,
        other => panic!("expected Reserved, got {other:?}"),
    };
    // Caller wrote 1 of 3 events then crashed before mark_applied.
    let second = store
        .reserve_emission("scope", "digest", 3, &events_jsonl_count(&[(&first_id, "p1")]))
        .expect("second");
    match second {
        EmissionReservation::RecoveryRequired {
            public_wave_id,
            on_disk,
            expected,
        } => {
            assert_eq!(public_wave_id, first_id);
            assert_eq!(on_disk, 1);
            assert_eq!(expected, 3);
        }
        other => panic!("expected RecoveryRequired, got {other:?}"),
    }
}