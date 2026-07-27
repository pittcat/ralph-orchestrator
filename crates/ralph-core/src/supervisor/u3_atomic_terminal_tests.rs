//! 2026-07-27-004 plan U3 (R8-R10 / D4): atomic slot terminal
//! commit becomes the single source of truth for
//! `Completed` / `Failed` / `Cancelled` slot state. Tests
//! verify:
//! - `commit_slot_terminal` advances the slot to its terminal
//!   state in one shot (no half-write visible to `fan_in_status`).
//! - Idempotent same-evidence replay returns `Idempotent` and
//!   does not rewrite state.
//! - Conflicting terminal record returns `AlreadyTerminal`
//!   (fail-closed conflict).
//!
//! Existing trait methods (`record_slot_result` / etc.) stay
//! compiling for backward compatibility; production code paths
//! that need atomic semantics call `commit_slot_terminal`. The
//! fallback default impl is documented to NOT be atomic — U3's
//! production stores override the method for true atomicity.

#[cfg(test)]
mod tests {
    use crate::supervisor::{
        DispatchOutcome, InMemorySupervisorStore, SlotStatus, SlotTerminalOutcome,
        SlotTerminalRecord, SupervisorStore, SupervisorStoreError, TerminalEvidence, WaveKind,
    };

    fn fresh_store() -> InMemorySupervisorStore {
        InMemorySupervisorStore::new()
    }

    fn bind_worktree_2slots(store: &InMemorySupervisorStore, wave: &str) {
        use crate::supervisor::SlotResource;
        store
            .bind_worktree(
                wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/u3/0".to_string()),
                    branch: Some("ralph/u3-0".to_string()),
                },
            )
            .unwrap();
        store
            .bind_worktree(
                wave,
                1,
                SlotResource {
                    slot_index: 1,
                    worktree_path: Some(".ralph/u3/1".to_string()),
                    branch: Some("ralph/u3-1".to_string()),
                },
            )
            .unwrap();
    }

    /// R8 / S7: a single commit_slot_terminal advances the slot
    /// to Completed with terminal evidence + content_hash atomically
    /// visible in fan_in_status.
    #[test]
    fn u3_atomic_completed_advances_slot_and_evidence_together() {
        let store = fresh_store();
        let wave = store.register_wave("u3-s7", WaveKind::Exec, 2, 1).unwrap();
        bind_worktree_2slots(&store, &wave);
        // dispatch both slots
        let _ = store.try_dispatch_next(4).unwrap().unwrap();
        let _ = store.try_dispatch_next(4).unwrap().unwrap();

        let ev = TerminalEvidence::from_event("exec.unit.done", r#"{"content_hash":"hash-u3-s7"}"#);
        let record = SlotTerminalRecord::Completed {
            slot_index: 0,
            content_hash: "hash-u3-s7".to_string(),
            event_count: 1,
            terminal_evidence: ev.clone(),
        };
        let outcome = store
            .commit_slot_terminal(&wave, &record)
            .expect("commit must succeed");
        assert_eq!(outcome, SlotTerminalOutcome::Committed);

        // fan_in_status sees the completed status WITH evidence
        // — the atomic-commit guarantee.
        let snap = store.fan_in_status(&wave).unwrap();
        let slot0 = snap
            .slots
            .iter()
            .find(|(i, _)| *i == 0)
            .map(|(_, s)| *s)
            .unwrap();
        assert_eq!(slot0, SlotStatus::Completed);
        assert_eq!(snap.completed_count, 1);
        assert_eq!(store.slot_terminal_evidence(&wave, 0).unwrap(), Some(ev));
    }

    /// R9 / S9 (idempotent replay): a Commit with the IDENTICAL
    /// content_hash + evidence on an already-terminal slot
    /// returns `Idempotent` and does NOT rewrite state.
    #[test]
    fn u3_idempotent_replay_returns_idempotent_outcome() {
        let store = fresh_store();
        let wave = store
            .register_wave("u3-idem", WaveKind::Exec, 1, 1)
            .unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                crate::supervisor::SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/u3-idem/0".to_string()),
                    branch: Some("ralph/u3-idem-0".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(4).unwrap().unwrap();

        let ev = TerminalEvidence::from_event("exec.unit.done", r#"{"content_hash":"h"}"#);
        let record = SlotTerminalRecord::Completed {
            slot_index: 0,
            content_hash: "h".to_string(),
            event_count: 1,
            terminal_evidence: ev.clone(),
        };
        assert_eq!(
            store.commit_slot_terminal(&wave, &record).unwrap(),
            SlotTerminalOutcome::Committed
        );
        // Identical replay → Idempotent.
        assert_eq!(
            store.commit_slot_terminal(&wave, &record).unwrap(),
            SlotTerminalOutcome::Idempotent
        );
        // Evidence is still the original.
        assert_eq!(store.slot_terminal_evidence(&wave, 0).unwrap(), Some(ev));
    }

    /// R9 / S9 (conflict): a Committed terminal evidence + a
    /// different terminal record on the same slot returns
    /// `AlreadyTerminal`. The legacy `record_slot_result` /
    /// `record_slot_terminal_evidence` first-terminal-wins
    /// contract is preserved by the default impl.
    #[test]
    fn u3_conflicting_terminal_record_is_rejected() {
        let store = fresh_store();
        let wave = store
            .register_wave("u3-conflict", WaveKind::Exec, 1, 1)
            .unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                crate::supervisor::SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/u3-c/0".to_string()),
                    branch: Some("ralph/u3-c-0".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(4).unwrap().unwrap();

        let first = SlotTerminalRecord::Completed {
            slot_index: 0,
            content_hash: "h-1".to_string(),
            event_count: 1,
            terminal_evidence: TerminalEvidence::from_event(
                "exec.unit.done",
                r#"{"content_hash":"h-1"}"#,
            ),
        };
        store
            .commit_slot_terminal(&wave, &first)
            .expect("first commit succeeds");

        // Different content_hash + different evidence → conflict.
        let second = SlotTerminalRecord::Completed {
            slot_index: 0,
            content_hash: "h-2".to_string(),
            event_count: 1,
            terminal_evidence: TerminalEvidence::from_event(
                "exec.unit.done",
                r#"{"content_hash":"h-2"}"#,
            ),
        };
        let conflict = store.commit_slot_terminal(&wave, &second);
        assert!(
            matches!(conflict, Err(SupervisorStoreError::AlreadyTerminal(_))),
            "conflicting terminal record must fail closed; got {conflict:?}"
        );
    }

    /// R8: a `Failed` record commits the slot to Failed with the
    /// reason; `record_slot_failure` first-terminal-wins prevents
    /// a second Failed record from overwriting.
    #[test]
    fn u3_failed_terminal_record_then_idempotent_replay() {
        let store = fresh_store();
        let wave = store
            .register_wave("u3-failed", WaveKind::Exec, 1, 1)
            .unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                crate::supervisor::SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/u3-f/0".to_string()),
                    branch: Some("ralph/u3-f-0".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(4).unwrap().unwrap();

        let record = SlotTerminalRecord::Failed {
            slot_index: 0,
            reason: "worker_timeout".to_string(),
            terminal_evidence: None,
        };
        let outcome = store.commit_slot_terminal(&wave, &record).unwrap();
        assert_eq!(outcome, SlotTerminalOutcome::Committed);

        // Same reason replay → Idempotent (the default impl
        // detects the matching status + reason and short-circuits).
        let replay = store.commit_slot_terminal(&wave, &record).unwrap();
        assert_eq!(replay, SlotTerminalOutcome::Idempotent);

        let snap = store.fan_in_status(&wave).unwrap();
        let status = snap.slots.iter().find(|(i, _)| *i == 0).map(|(_, s)| *s);
        assert_eq!(status, Some(SlotStatus::Failed));
        assert_eq!(
            store.slot_failure_reason(&wave, 0).unwrap(),
            Some("worker_timeout".to_string())
        );
    }

    /// R8: Cancelled record commits; the dispatcher treats the
    /// cancel reason as a permanent failure-with-cancel
    /// distinction.
    #[test]
    fn u3_cancelled_terminal_record_routes_to_failure_path() {
        let store = fresh_store();
        let wave = store
            .register_wave("u3-cancel", WaveKind::Exec, 1, 1)
            .unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                crate::supervisor::SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/u3-ca/0".to_string()),
                    branch: Some("ralph/u3-ca-0".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(4).unwrap().unwrap();

        let record = SlotTerminalRecord::Cancelled {
            slot_index: 0,
            reason: crate::supervisor::worker_outcome::REASON_WORKER_CANCELLED.to_string(),
        };
        let outcome = store.commit_slot_terminal(&wave, &record).unwrap();
        assert_eq!(outcome, SlotTerminalOutcome::Committed);

        // The cancellation path records the cancel reason via
        // `record_slot_failure` so the existing first-terminal-wins
        // + Cancelled-status mapping still applies.
        assert_eq!(
            store.slot_failure_reason(&wave, 0).unwrap(),
            Some(crate::supervisor::worker_outcome::REASON_WORKER_CANCELLED.to_string())
        );
    }

    /// S10 / R10: a slot mid-Running remains Running after a
    /// different slot is Completed. The atomic terminal
    /// commit only mutates the target slot — sibling slots and
    /// dispatch capacity stay untouched.
    #[test]
    fn u3_completed_terminal_does_not_mutate_sibling_running_slot() {
        let store = fresh_store();
        let wave = store
            .register_wave("u3-sibling", WaveKind::Exec, 2, 1)
            .unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                crate::supervisor::SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/u3-s/0".to_string()),
                    branch: Some("ralph/u3-s-0".to_string()),
                },
            )
            .unwrap();
        store
            .bind_worktree(
                &wave,
                1,
                crate::supervisor::SlotResource {
                    slot_index: 1,
                    worktree_path: Some(".ralph/u3-s/1".to_string()),
                    branch: Some("ralph/u3-s-1".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(4).unwrap().unwrap();
        let _ = store.try_dispatch_next(4).unwrap().unwrap();

        let record = SlotTerminalRecord::Completed {
            slot_index: 0,
            content_hash: "h".to_string(),
            event_count: 1,
            terminal_evidence: TerminalEvidence::from_event(
                "exec.unit.done",
                r#"{"content_hash":"h"}"#,
            ),
        };
        store.commit_slot_terminal(&wave, &record).unwrap();

        let snap = store.fan_in_status(&wave).unwrap();
        let slot1 = snap
            .slots
            .iter()
            .find(|(i, _)| *i == 1)
            .map(|(_, s)| *s)
            .unwrap();
        assert_eq!(slot1, SlotStatus::Dispatched);
        assert_eq!(snap.completed_count, 1);
        assert_eq!(snap.in_flight_count, 1);
        assert_eq!(snap.pending_count, 0);
    }

    /// U3 doc pin: legacy dispatch releases are still callable
    /// in isolation. The new atomic API is additive, not
    /// destructive. Existing unit tests in `memory.rs` cover
    /// the per-method contracts.
    #[test]
    fn u3_legacy_release_slot_dispatch_is_still_callable() {
        let store = fresh_store();
        let wave = store
            .register_wave("u3-legacy-rel", WaveKind::Exec, 1, 1)
            .unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                crate::supervisor::SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/u3-lr/0".to_string()),
                    branch: Some("ralph/u3-lr-0".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(4).unwrap().unwrap();
        store
            .release_slot_dispatch(&wave, 0, DispatchOutcome::Completed)
            .unwrap();
        let snap = store.fan_in_status(&wave).unwrap();
        let status = snap.slots.iter().find(|(i, _)| *i == 0).map(|(_, s)| *s);
        assert_eq!(status, Some(SlotStatus::Completed));
    }
}
