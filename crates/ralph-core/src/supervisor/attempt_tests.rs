//! 2026-08-07-009 plan U1 (R1-R8 / KTD1-KTD4 / KTD11): shared
//! parity tests for the per-slot attempt receipt contract. The
//! same vector runs against the in-memory and rusqlite adapters so
//! `attempt_seq` monotonicity, `finish` transition semantics, and
//! `list` ordering stay identical across adapters.
//!
//! Tests live in `mod attempt_tests` and gate on the
//! `supervisor-db` feature for the rusqlite-backed half; the
//! memory-backed half always runs.

#![cfg(test)]

#[cfg(feature = "supervisor-db")]
use super::RusqliteSupervisorStore;
use super::{AttemptStatus, GitCheckpoint, InMemorySupervisorStore, SupervisorStore, WaveKind};

/// Register a single fresh wave with `slot_count` slots and
/// return the assigned wave id.
fn register_wave_with_slots(store: &dyn SupervisorStore, slot_count: u32) -> String {
    let wave_id = store
        .register_wave(
            &format!("idem-{}", slot_count),
            WaveKind::Exec,
            slot_count,
            1,
        )
        .expect("register_wave must succeed");
    wave_id
}

/// Allocate a fresh store backed by a temporary SQLite file so
/// `Open` exercises the same code path as production.
#[cfg(feature = "supervisor-db")]
fn fresh_rusqlite_store() -> (tempfile::TempDir, RusqliteSupervisorStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("supervisor.db");
    let store = RusqliteSupervisorStore::open(&path).expect("open");
    (dir, store)
}

/// Convenience: build a checkpoint with a known HEAD and dirty bit.
fn checkpoint(head: &str, dirty: bool) -> Option<GitCheckpoint> {
    Some(GitCheckpoint {
        head_sha: Some(head.to_string()),
        dirty: Some(dirty),
    })
}

/// Memory adapter parity vector — runs once for `InMemorySupervisorStore`.
mod memory_contract {
    use super::*;

    #[test]
    fn attempt_contract_begin_finish_list_round_trips() {
        let store = InMemorySupervisorStore::new();
        let wave_id = register_wave_with_slots(&store, 2);

        let begin = store
            .begin_slot_attempt(&wave_id, 0, checkpoint("aaaaaaa", false), 1000)
            .expect("begin slot 0 attempt 1");
        assert_eq!(begin.attempt_seq, 1);
        assert_eq!(begin.status, AttemptStatus::Running);
        assert_eq!(begin.finished_at_unix_ms, 0);

        let finished = store
            .finish_slot_attempt(
                &wave_id,
                0,
                1,
                AttemptStatus::Succeeded,
                checkpoint("bbbbbbb", false),
                None,
                1500,
            )
            .expect("finish attempt 1 as succeeded");
        assert_eq!(finished.status, AttemptStatus::Succeeded);
        assert_eq!(finished.finished_at_unix_ms, 1500);
        assert!(finished.failure_code.is_none());

        let list = store
            .list_slot_attempts(&wave_id, 0, None)
            .expect("list slot 0 attempts");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].attempt_seq, 1);

        // Second attempt on the same slot increments seq to 2.
        let begin2 = store
            .begin_slot_attempt(&wave_id, 0, checkpoint("ccccccc", true), 2000)
            .expect("begin slot 0 attempt 2");
        assert_eq!(begin2.attempt_seq, 2);
        let failed = store
            .finish_slot_attempt(
                &wave_id,
                0,
                2,
                AttemptStatus::Failed,
                None,
                Some("executor_reported_failure"),
                2500,
            )
            .expect("finish attempt 2 as failed");
        assert_eq!(
            failed.failure_code.as_deref(),
            Some("executor_reported_failure")
        );

        // Listing returns both rows in ascending seq order.
        let list = store
            .list_slot_attempts(&wave_id, 0, None)
            .expect("list after two attempts");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].attempt_seq, 1);
        assert_eq!(list[1].attempt_seq, 2);

        // `limit` bounds the trailing slice; asc ordering preserved.
        let recent = store
            .list_slot_attempts(&wave_id, 0, Some(1))
            .expect("list with limit=1");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].attempt_seq, 2);

        // limit=0 returns empty vec (caller can probe cheaply).
        let none = store
            .list_slot_attempts(&wave_id, 0, Some(0))
            .expect("list with limit=0");
        assert!(none.is_empty());
    }

    #[test]
    fn attempt_contract_finish_is_idempotent_but_conflict_is_rejected() {
        let store = InMemorySupervisorStore::new();
        let wave_id = register_wave_with_slots(&store, 1);
        let _ = store
            .begin_slot_attempt(&wave_id, 0, None, 1000)
            .expect("begin");
        let _ = store
            .finish_slot_attempt(&wave_id, 0, 1, AttemptStatus::Succeeded, None, None, 1500)
            .expect("finish as succeeded");

        // Same terminal status + same fingerprint → idempotent ok.
        let again = store
            .finish_slot_attempt(&wave_id, 0, 1, AttemptStatus::Succeeded, None, None, 1500)
            .expect("idempotent finish");
        assert_eq!(again.status, AttemptStatus::Succeeded);

        // Different terminal status → conflict rejected.
        let conflict = store.finish_slot_attempt(
            &wave_id,
            0,
            1,
            AttemptStatus::Failed,
            None,
            Some("executor_reported_failure"),
            2000,
        );
        assert!(conflict.is_err(), "conflicting terminal must be rejected");

        // Unknown attempt_seq → InvalidTransition.
        let err =
            store.finish_slot_attempt(&wave_id, 0, 99, AttemptStatus::Succeeded, None, None, 3000);
        assert!(err.is_err());

        // Status invariants: failed without code is rejected.
        let _ = store
            .begin_slot_attempt(&wave_id, 0, None, 4000)
            .expect("second begin");
        let err =
            store.finish_slot_attempt(&wave_id, 0, 2, AttemptStatus::Failed, None, None, 4500);
        assert!(err.is_err(), "failed must carry a failure_code");

        // Status invariants: succeeded with code is rejected.
        let err = store.finish_slot_attempt(
            &wave_id,
            0,
            2,
            AttemptStatus::Succeeded,
            None,
            Some("never"),
            4600,
        );
        assert!(err.is_err(), "succeeded must not carry a failure_code");
    }

    #[test]
    fn attempt_contract_concurrent_begin_allocates_unique_sequence() {
        use std::sync::Arc;
        use std::thread;

        let store = Arc::new(InMemorySupervisorStore::new());
        let wave_id = register_wave_with_slots(&*store, 1);

        let barrier = Arc::new(std::sync::Barrier::new(8));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let store = Arc::clone(&store);
            let wave_id = wave_id.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                store
                    .begin_slot_attempt(&wave_id, 0, None, 0)
                    .expect("concurrent begin must succeed")
            }));
        }
        let mut seqs: Vec<u32> = handles
            .into_iter()
            .map(|h| h.join().unwrap().attempt_seq)
            .collect();
        seqs.sort_unstable();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6, 7, 8]);

        // Listing returns all 8 rows in ascending seq order.
        let list = store
            .list_slot_attempts(&wave_id, 0, None)
            .expect("list after concurrent begins");
        assert_eq!(list.len(), 8);
        for (i, receipt) in list.iter().enumerate() {
            assert_eq!(receipt.attempt_seq, (i as u32) + 1);
        }
    }

    #[test]
    fn attempt_contract_unknown_wave_or_slot_returns_error() {
        let store = InMemorySupervisorStore::new();
        let err = store.begin_slot_attempt("does-not-exist", 0, None, 0);
        assert!(matches!(err, Err(_)));

        // Register a wave with one slot then probe slot 99 — the
        // store has no `slot 99` row, so begin must surface
        // UnknownSlot.
        let wave_id = register_wave_with_slots(&store, 1);
        let err = store.begin_slot_attempt(&wave_id, 99, None, 0);
        assert!(matches!(err, Err(_)));
    }

    #[test]
    fn parent_slot_attempts_returns_empty_for_unrelated_child() {
        // A child wave without a `(child → parent)` slot mapping
        // must NOT fabricate parent history; the dispatcher relies
        // on this to render "no history" honestly.
        let store = InMemorySupervisorStore::new();
        let child = store
            .register_wave("child-no-parent", WaveKind::Exec, 1, 1)
            .expect("register child wave");
        let history = store
            .parent_slot_attempts(&child, 0, None)
            .expect("parent slot attempts");
        assert!(history.attempts.is_empty());
    }

    #[test]
    fn parent_slot_resource_is_unbound_for_never_bound_parent() {
        let store = InMemorySupervisorStore::new();
        let parent = store
            .register_wave("parent-w", WaveKind::Exec, 2, 1)
            .expect("register parent wave");
        // Mark a slot Failed so create_redrive_wave accepts it.
        store
            .record_slot_failure(&parent, 0, "frozen")
            .expect("fail slot 0");
        let result = store
            .create_redrive_wave(&parent, None)
            .expect("create redrive child");
        let child = result.child_wave_id;

        // Parent slot was never bound to a worktree — parent
        // resource resolver must surface `Unbound` so the
        // dispatcher falls back to the factory.
        let err = store
            .parent_slot_resource(&child, 0)
            .expect_err("unbound parent must surface Unbound");
        assert!(matches!(err, super::super::ParentResourceError::Unbound));
    }
}

/// Rusqlite adapter parity vector — mirrors the memory contract
/// on a real SQLite file. Gated on `supervisor-db` so
/// no-feature builds skip these tests cleanly.
#[cfg(feature = "supervisor-db")]
mod rusqlite_contract {
    use super::*;
    use crate::supervisor::SupervisorStoreError;

    #[test]
    fn rusqlite_attempt_receipts_survive_reopen() {
        let (_dir, store) = fresh_rusqlite_store();
        let wave_id = register_wave_with_slots(&store, 1);
        let begin = store
            .begin_slot_attempt(&wave_id, 0, checkpoint("aaaaaaa", false), 1000)
            .expect("begin");
        store
            .finish_slot_attempt(
                &wave_id,
                0,
                begin.attempt_seq,
                AttemptStatus::Succeeded,
                checkpoint("bbbbbbb", false),
                None,
                1500,
            )
            .expect("finish succeeded");
        store
            .begin_slot_attempt(&wave_id, 0, checkpoint("ccccccc", true), 2000)
            .expect("begin second");
        store
            .finish_slot_attempt(
                &wave_id,
                0,
                2,
                AttemptStatus::Failed,
                None,
                Some("executor_reported_failure"),
                2500,
            )
            .expect("finish failed");

        // Close and reopen; the receipts persist.
        drop(store);
        let path = _dir.path().join("supervisor.db");
        let store = RusqliteSupervisorStore::open(&path).expect("reopen");
        let list = store
            .list_slot_attempts(&wave_id, 0, None)
            .expect("list after reopen");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].attempt_seq, 1);
        assert_eq!(list[0].status, AttemptStatus::Succeeded);
        assert_eq!(list[1].attempt_seq, 2);
        assert_eq!(list[1].status, AttemptStatus::Failed);
        assert_eq!(
            list[1].failure_code.as_deref(),
            Some("executor_reported_failure")
        );

        // After reopen the next attempt_seq is monotonic (3).
        let begin3 = store
            .begin_slot_attempt(&wave_id, 0, None, 3000)
            .expect("begin after reopen");
        assert_eq!(begin3.attempt_seq, 3);
    }

    #[test]
    fn rusqlite_concurrent_begin_allocates_unique_sequence() {
        use std::sync::Arc;
        use std::thread;

        let (_dir, store) = fresh_rusqlite_store();
        let store = Arc::new(store);
        let wave_id = register_wave_with_slots(&*store, 1);

        let barrier = Arc::new(std::sync::Barrier::new(4));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let store = Arc::clone(&store);
            let wave_id = wave_id.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                store
                    .begin_slot_attempt(&wave_id, 0, None, 0)
                    .expect("begin")
            }));
        }
        let mut seqs: Vec<u32> = handles
            .into_iter()
            .map(|h| h.join().unwrap().attempt_seq)
            .collect();
        seqs.sort_unstable();
        assert_eq!(seqs, vec![1, 2, 3, 4]);
    }

    #[test]
    fn rusqlite_finish_unknown_attempt_is_rejected() {
        let (_dir, store) = fresh_rusqlite_store();
        let wave_id = register_wave_with_slots(&store, 1);
        let err =
            store.finish_slot_attempt(&wave_id, 0, 1, AttemptStatus::Succeeded, None, None, 0);
        assert!(matches!(
            err,
            Err(SupervisorStoreError::InvalidTransition(_))
        ));

        let err = store.begin_slot_attempt("missing-wave", 0, None, 0);
        assert!(matches!(err, Err(SupervisorStoreError::UnknownWave(_))));
    }
}
