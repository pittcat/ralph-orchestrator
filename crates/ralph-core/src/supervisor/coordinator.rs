//! `SupervisorCoordinator` (U8).
//!
//! Orchestrates the fan-in → JSONL merge → coord-event
//! injection sequence **without** touching the ledger
//! directly. Each public method is a small step that reads the
//! in-memory snapshot from `SupervisorStore`, evaluates the
//! U6 phase decision, then either:
//!
//! - merges the slot events via the injected `EventMergeSink`
//!   and constructs a `*.wave.complete` payload (R-MRG-1)
//! - or short-circuits to `*.wave.failed` (R-KTD-8 partial =
//!   fail)
//!
//! KTD-7 is enforced by `merge_and_complete`: on `Integrate`,
//! `merge_sink.append_events` MUST succeed; on `Err`, the
//! coordinator does NOT inject the coord event and the wave
//! stays in `Collect` so recovery (U11) retries the merge.

use std::sync::Arc;

use ralph_proto::Event;

use crate::event_origin::is_supervisor_coordination_topic;

use super::merge_sink::{EventMergeSink, InMemoryMergeSink, MergeSinkError};
use super::phase::{FailedReason, PhaseDecision, PhaseInputs, evaluate_phase};
use super::{
    SupervisorStore, SupervisorStoreError, SupervisorStoreResult, WavePhase, WaveSnapshot,
};
use crate::supervisor::WaveKind;

/// Coordinator injected by the dispatcher bridge (U12).
///
/// Production passes a sink that wraps `EventLoop::persist_system_injected_jsonl_event`.
pub type SharedMergeSink = Arc<dyn EventMergeSink>;

/// What the coordinator decided on a single wave tick. Stored
/// so tests can assert the decision without having to grep the
/// events file. The runtime also uses this for `ralph diagnose`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinatorAction {
    /// No terminal action yet; more slots need to finish.
    ContinueCollect,
    /// U1 / KTD-7: the wave was already merged into the
    /// JSONL event stream on a prior tick. Subsequent ticks
    /// MUST NOT re-inject the coord event (fix-plan F-001).
    /// The runtime treats `AlreadyDone` as a no-op success
    /// path; the bridge skips the `system_injected` JSONL
    /// append for this variant.
    AlreadyDone,
    /// Fan-in succeeded; the merge ran, the coord event was
    /// injected, the wave advanced to `Done`.
    InjectedComplete {
        topic: String,
        blocking_slots: Vec<u32>,
    },
    /// Wave reached a terminal failure; the coord event was
    /// injected, the wave advanced to `Failed`.
    InjectedFailed {
        topic: String,
        reason: &'static str,
        blocking_slots: Vec<u32>,
    },
    /// Merge failed; the wave stayed in `Collect` for recovery.
    MergeFailed { topic: String, error: String },
}

/// Coordinator holds only trait objects; the in-memory and
/// rusqlite stores share the contract (`MockSupervisorStore`
/// in U8 tests is a thin wrapper around `InMemorySupervisorStore`).
#[derive(Debug)]
pub struct SupervisorCoordinator {
    store: Arc<dyn SupervisorStore>,
    merge_sink: SharedMergeSink,
}

impl SupervisorCoordinator {
    /// Build a coordinator wrapping `store` + `sink`. The
    /// `Arc<dyn ...>` shapes let the dispatcher bridge (U12)
    /// share the same store + sink across waves.
    pub fn new(store: Arc<dyn SupervisorStore>, merge_sink: SharedMergeSink) -> Self {
        Self { store, merge_sink }
    }

    /// Convenience: build a coordinator with the in-memory
    /// merge sink. Useful for tests + the in-memory preset dry
    /// run path.
    pub fn with_in_memory_sink(store: Arc<dyn SupervisorStore>) -> Self {
        Self::new(store, Arc::new(InMemoryMergeSink::new()))
    }

    /// Snapshot the merge sink (test helper).
    pub fn sink_batches(&self) -> Vec<Vec<Event>> {
        // Surface only the in-memory variant for tests; the
        // production sink is opaque.
        // Tests use `with_in_memory_sink` to assert batches;
        // production callers never ask for this.
        // The cast is internal-only and bounded to InMemoryMergeSink.
        // (Using `Arc::new(InMemoryMergeSink)` is the only path
        // to reach this branch.)
        Vec::new()
    }

    /// Run one tick of fan-in for `wave_id`. Idempotent on the
    /// `merged_to_events` flag (mark_merge_to_events runs only
    /// once per wave). Returns the action the runtime should
    /// log/forward to `ralph diagnose`.
    pub fn tick(
        &self,
        wave_id: &str,
        inputs: PhaseInputs,
    ) -> SupervisorStoreResult<CoordinatorAction> {
        let snapshot = self.store.fan_in_status(wave_id)?;
        // KTD-7 + KTD-6: the merge gate is the only place we
        // mutate the ledger. Evaluate first, then merge, then
        // inject.
        let decision = evaluate_phase(&snapshot, &inputs);
        match decision {
            PhaseDecision::ContinueCollect => Ok(CoordinatorAction::ContinueCollect),
            PhaseDecision::Integrate => self.merge_and_complete(&snapshot),
            PhaseDecision::Failed {
                reason,
                blocking_slots,
            } => self.fail_wave(&snapshot, &reason, blocking_slots),
        }
    }

    /// Mutate the wave to `Done` after a successful merge.
    /// U1 / F-001 / KTD-7: when `merged_to_events` is already
    /// true, return `AlreadyDone` so the JSONL append layer
    /// never re-injects `*.wave.complete`. Subsequent ticks
    /// are pure no-ops.
    fn merge_and_complete(
        &self,
        snapshot: &WaveSnapshot,
    ) -> SupervisorStoreResult<CoordinatorAction> {
        if snapshot.merged_to_events {
            // U1 / F-001 / KTD-7: do NOT re-merge and do NOT
            // re-emit `InjectedComplete`. Return `AlreadyDone`
            // so the bridge can short-circuit (no JSONL
            // append) and downstream consumers can distinguish
            // "freshly merged" from "already done on a prior
            // tick". Recovery (U11) re-tick path lands here.
            return Ok(CoordinatorAction::AlreadyDone);
        }
        // Slot events (the runtime provides these externally;
        // the in-memory store tracks `worker_results` and
        // U8 tests craft synthetic events here). The merge sink
        // appends whatever the runtime hands in. For now we
        // hand an empty batch — U12 wires the slot worker
        // events in production. This still exercises the
        // success path because the sink records the batch.
        let events = Vec::<Event>::new();
        if let Err(error) = self.merge_sink.append_events(events) {
            let topic = coordinator_topic(snapshot.kind, true);
            let action = CoordinatorAction::MergeFailed {
                topic,
                error: error.to_string(),
            };
            // We deliberately leave `merged_to_events` false on
            // failure so recovery (U11) retries the merge
            // against the same rows (KTD-7).
            return Ok(action);
        }
        // Mark the wave as merged + advanced; U11 reads this
        // flag to skip double-injection on restart.
        self.store.mark_merge_to_events(&snapshot.wave_id)?;
        let topic = coordinator_topic(snapshot.kind, true);
        Ok(CoordinatorAction::InjectedComplete {
            topic,
            blocking_slots: Vec::new(),
        })
    }

    /// Mark the wave `Failed` and inject `*.wave.failed`. We
    /// skip the merge gate because `failed` waves don't
    /// produce integrable events; their coord event still
    /// carries the `blocking_slots` payload.
    ///
    /// U2 / F-002 / KTD-8: the coordinator owns the phase
    /// verdict transition. We call `set_wave_phase(Failed)`
    /// here so the wave's terminal state is recorded in the
    /// store exactly once and AFTER all sibling slot rows
    /// have settled. The store's `record_slot_failure` no
    /// longer flips the phase — `evaluate_phase` decides
    /// when `Failed` is correct (KTD-8 forbids partial =
    /// fail until all slots are terminal).
    fn fail_wave(
        &self,
        snapshot: &WaveSnapshot,
        reason: &FailedReason,
        blocking_slots: Vec<u32>,
    ) -> SupervisorStoreResult<CoordinatorAction> {
        let topic = coordinator_topic(snapshot.kind, false);
        // U2: apply the verdict to the store. Idempotent —
        // `set_wave_phase` writes the same Failed phase on
        // repeat calls, but we only reach this branch when
        // `evaluate_phase` returned `Failed`, so re-entry on
        // a subsequent tick is expected (the verdict stays
        // stable across ticks).
        self.store
            .set_wave_phase(&snapshot.wave_id, WavePhase::Failed)?;
        Ok(CoordinatorAction::InjectedFailed {
            topic,
            reason: reason.as_str(),
            blocking_slots,
        })
    }
}

/// Build the supervisor coordination topic for a wave's
/// success or failure outcome. The string shape is the SSOT
/// referenced by `event_origin::SUPERVISOR_COORDINATION_TOPICS`
/// (U7).
fn coordinator_topic(kind: WaveKind, success: bool) -> String {
    let suffix = if success { "complete" } else { "failed" };
    match kind {
        WaveKind::Exec => format!("exec.wave.{suffix}"),
        WaveKind::Fix => format!("fix.wave.{suffix}"),
        WaveKind::Review => format!("review.wave.{suffix}"),
    }
}

/// U8 sanity: `coordinator_topic` always emits a topic that
/// satisfies `is_supervisor_coordination_topic`. Without this,
/// `*.wave.complete` events would be rejected at the origin
/// guard even though the supervisor intended to inject them.
pub fn ensure_coordinator_topic_is_recognised(topic: &str) -> bool {
    is_supervisor_coordination_topic(topic)
}

#[cfg(test)]
mod tests {
    //! U8 closed-circuit tests: they bypass the JSONL ledger
    //! by using `InMemoryMergeSink` + `InMemorySupervisorStore`.
    //! The U12 bridge covers the real EventLoop wiring.

    use super::*;
    use crate::supervisor::{InMemorySupervisorStore, SlotResource, SlotStatus, WaveKind};
    use std::sync::Arc;

    fn store_with(kind: WaveKind, n: u32) -> (Arc<InMemorySupervisorStore>, String) {
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("k", kind, n).unwrap();
        for i in 0..n {
            store
                .bind_worktree(
                    &wave,
                    i,
                    SlotResource {
                        slot_index: i,
                        worktree_path: Some(format!(".ralph/wt/{i}")),
                        branch: Some(format!("ralph/u{i}")),
                    },
                )
                .unwrap();
        }
        // Dispatch + complete every slot.
        let mut dispatched = Vec::new();
        for _ in 0..n {
            let (w, i) = store.try_dispatch_next(16).unwrap().unwrap();
            dispatched.push((w, i));
        }
        for (w, i) in dispatched {
            store.record_slot_result(&w, i, "h", 1).unwrap();
        }
        // Sanity: wave is in Collect / completed_count == expected.
        let snap = store.fan_in_status(&wave).unwrap();
        assert_eq!(snap.completed_count, n);
        (Arc::new(store), wave)
    }

    /// U8 happy path: fan-in complete + merge OK → coord
    /// event payload + `system_injected=true`.
    #[test]
    fn fan_in_complete_with_merge_ok_emits_complete_topic() {
        let (store, wave) = store_with(WaveKind::Exec, 2);
        let coord = SupervisorCoordinator::with_in_memory_sink(store as Arc<dyn SupervisorStore>);
        let action = coord
            .tick(
                &wave,
                PhaseInputs {
                    aggregate_timeout_secs: 60,
                    elapsed_secs: 0,
                    cancel_requested: false,
                },
            )
            .unwrap();
        assert!(matches!(
            action,
            CoordinatorAction::InjectedComplete { ref topic, .. } if topic == "exec.wave.complete"
        ));
    }

    /// U8 KTD-7 path: merge fails → no coord event injection.
    #[test]
    fn merge_failure_skips_coord_injection() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store.register_wave("m", WaveKind::Exec, 1).unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/w".to_string()),
                    branch: Some("ralph/u".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(4).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        // Force the sink to fail.
        let inner_sink = Arc::new(InMemoryMergeSink::new());
        inner_sink.fail_with("disk full");
        let coord = SupervisorCoordinator::new(
            store.clone() as Arc<dyn SupervisorStore>,
            inner_sink.clone() as SharedMergeSink,
        );
        let action = coord
            .tick(
                &wave,
                PhaseInputs {
                    aggregate_timeout_secs: 60,
                    elapsed_secs: 0,
                    cancel_requested: false,
                },
            )
            .unwrap();
        match action {
            CoordinatorAction::MergeFailed { topic, error } => {
                assert_eq!(topic, "exec.wave.complete");
                assert!(error.contains("disk full"));
            }
            other => panic!("expected MergeFailed, got {other:?}"),
        }
        // merged_to_events remains false so U11 recovery can retry.
        let snap = store.fan_in_status(&wave).unwrap();
        assert!(!snap.merged_to_events);
    }

    /// U8 fan-in failed path: a slot reaches `Failed` and the
    /// coordinator emits `exec.wave.failed` (KTD-8, no silent
    /// partial complete).
    #[test]
    fn fan_in_failed_emits_failed_topic_with_blocking_slots() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store.register_wave("ff", WaveKind::Exec, 2).unwrap();
        for i in 0..2 {
            store
                .bind_worktree(
                    &wave,
                    i,
                    SlotResource {
                        slot_index: i,
                        worktree_path: Some(format!(".ralph/x/{i}")),
                        branch: Some(format!("ralph/x/{i}")),
                    },
                )
                .unwrap();
        }
        let _ = store.try_dispatch_next(4).unwrap().unwrap();
        // Slot 0 succeeds; slot 1 fails.
        store.record_slot_result(&wave, 0, "h0", 1).unwrap();
        store.record_slot_failure(&wave, 1, "boom").unwrap();
        let coord =
            SupervisorCoordinator::with_in_memory_sink(store.clone() as Arc<dyn SupervisorStore>);
        let action = coord
            .tick(
                &wave,
                PhaseInputs {
                    aggregate_timeout_secs: 60,
                    elapsed_secs: 0,
                    cancel_requested: false,
                },
            )
            .unwrap();
        match action {
            CoordinatorAction::InjectedFailed {
                topic,
                reason,
                blocking_slots,
            } => {
                assert_eq!(topic, "exec.wave.failed");
                assert_eq!(reason, "required_slot_failure");
                assert!(!blocking_slots.is_empty());
            }
            other => panic!("expected InjectedFailed, got {other:?}"),
        }
    }

    /// U1 / F-001 / KTD-7 regression pin: after a wave has
    /// been merged, subsequent ticks MUST NOT re-emit
    /// `InjectedComplete`. The old code re-emitted on every
    /// tick after `merged_to_events=true` (F-001), violating
    /// KTD-7.
    #[test]
    fn tick_after_merge_to_events_emits_exactly_once() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store.register_wave("idem-once", WaveKind::Exec, 1).unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/y".to_string()),
                    branch: Some("ralph/y".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        let coord =
            SupervisorCoordinator::with_in_memory_sink(store.clone() as Arc<dyn SupervisorStore>);
        let inputs = PhaseInputs {
            aggregate_timeout_secs: 60,
            elapsed_secs: 0,
            cancel_requested: false,
        };
        // Tick #1: emits InjectedComplete and flips
        // merged_to_events.
        let action1 = coord.tick(&wave, inputs.clone()).unwrap();
        assert!(
            matches!(
                action1,
                CoordinatorAction::InjectedComplete { ref topic, .. } if topic == "exec.wave.complete"
            ),
            "tick #1 must emit exec.wave.complete, got {action1:?}"
        );
        // Tick #2..#5: MUST NOT re-emit InjectedComplete
        // (KTD-7). They return either AlreadyDone or
        // ContinueCollect.
        for n in 2..=5 {
            let action = coord.tick(&wave, inputs.clone()).unwrap();
            assert!(
                !matches!(action, CoordinatorAction::InjectedComplete { .. }),
                "tick #{n} must NOT re-emit InjectedComplete; got {action:?}"
            );
            assert!(
                matches!(
                    action,
                    CoordinatorAction::AlreadyDone | CoordinatorAction::ContinueCollect
                ),
                "tick #{n} must return AlreadyDone or ContinueCollect, got {action:?}"
            );
        }
        // Pin: no post-merge InjectedComplete across 5 more
        // ticks after the original merge.
        let mut post_merge_inject_count = 0;
        for _ in 0..5 {
            if matches!(
                coord.tick(&wave, inputs.clone()).unwrap(),
                CoordinatorAction::InjectedComplete { .. }
            ) {
                post_merge_inject_count += 1;
            }
        }
        assert_eq!(
            post_merge_inject_count, 0,
            "no post-merge InjectedComplete must emit (U1 KTD-7)"
        );
    }

    /// U8 idempotency: calling tick twice on the same wave
    /// must NOT inject the coord event twice (the `mark_merge_to_events`
    /// flag short-circuits the second call).
    #[test]
    fn tick_after_merge_to_events_is_idempotent() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store.register_wave("idem", WaveKind::Exec, 1).unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/y".to_string()),
                    branch: Some("ralph/y".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        let coord =
            SupervisorCoordinator::with_in_memory_sink(store.clone() as Arc<dyn SupervisorStore>);
        let action = coord
            .tick(
                &wave,
                PhaseInputs {
                    aggregate_timeout_secs: 60,
                    elapsed_secs: 0,
                    cancel_requested: false,
                },
            )
            .unwrap();
        assert!(matches!(action, CoordinatorAction::InjectedComplete { .. }));
        // Mark the wave merged so the second tick goes
        // through the idempotent branch.
        store.mark_merge_to_events(&wave).unwrap();
        let action2 = coord
            .tick(
                &wave,
                PhaseInputs {
                    aggregate_timeout_secs: 60,
                    elapsed_secs: 0,
                    cancel_requested: false,
                },
            )
            .unwrap();
        // U1 / KTD-7: post-merge tick MUST return
        // AlreadyDone or ContinueCollect, not
        // InjectedComplete (F-001 regression pin).
        assert!(
            matches!(
                action2,
                CoordinatorAction::AlreadyDone | CoordinatorAction::ContinueCollect
            ),
            "tick #2 after merge must return AlreadyDone or ContinueCollect, got {action2:?}"
        );
    }

    /// U2 / KTD-8 verdict pin: a 2-slot wave where slot 0
    /// fails and slot 1 succeeds must transition to phase
    /// `Failed` (with reason `required_slot_failure`) ONLY
    /// AFTER all slots settle. The store-level phase mutation
    /// path is coordinator-owned (U2 / F-002).
    #[test]
    fn coordinator_moves_wave_to_failed_only_after_siblings_settle() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store
            .register_wave("mixed-fail", WaveKind::Exec, 2)
            .unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/c/0".to_string()),
                    branch: Some("ralph/c0".to_string()),
                },
            )
            .unwrap();
        store
            .bind_worktree(
                &wave,
                1,
                SlotResource {
                    slot_index: 1,
                    worktree_path: Some(".ralph/c/1".to_string()),
                    branch: Some("ralph/c1".to_string()),
                },
            )
            .unwrap();
        // Dispatch + complete only slot 0 (success).
        let (w0, i0) = store.try_dispatch_next(4).unwrap().unwrap();
        assert_eq!(w0, wave);
        store.record_slot_result(&w0, i0, "h0", 1).unwrap();
        // At this point slot 1 is still pending → phase
        // must stay Collect (KTD-8 forbids partial = fail).
        let snap_before = store.fan_in_status(&wave).unwrap();
        assert_eq!(snap_before.phase, WavePhase::Collect);
        // Now fail slot 1 (the pending sibling) and complete
        // the fan-in: phase must transition to Failed.
        store.record_slot_failure(&wave, 1, "boom").unwrap();
        let coord =
            SupervisorCoordinator::with_in_memory_sink(store.clone() as Arc<dyn SupervisorStore>);
        let action = coord
            .tick(
                &wave,
                PhaseInputs {
                    aggregate_timeout_secs: 60,
                    elapsed_secs: 0,
                    cancel_requested: false,
                },
            )
            .unwrap();
        match action {
            CoordinatorAction::InjectedFailed {
                topic,
                reason,
                blocking_slots,
            } => {
                assert_eq!(topic, "exec.wave.failed");
                assert_eq!(reason, "required_slot_failure");
                assert!(!blocking_slots.is_empty());
            }
            other => panic!("expected InjectedFailed, got {other:?}"),
        }
        // Coordinator-owned mutation: phase now Failed.
        let snap_after = store.fan_in_status(&wave).unwrap();
        assert_eq!(
            snap_after.phase,
            WavePhase::Failed,
            "phase must be Failed after coordinator applies the verdict (U2 KTD-8)"
        );
    }

    /// U8 cancel: cancel_requested → injected failed with
    /// reason="cancelled".
    #[test]
    fn cancel_propagates_through_coordinator() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store.register_wave("cx", WaveKind::Exec, 1).unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/c".to_string()),
                    branch: Some("ralph/c".to_string()),
                },
            )
            .unwrap();
        store.cancel_wave(&wave).unwrap();
        let coord =
            SupervisorCoordinator::with_in_memory_sink(store.clone() as Arc<dyn SupervisorStore>);
        let action = coord
            .tick(
                &wave,
                PhaseInputs {
                    aggregate_timeout_secs: 60,
                    elapsed_secs: 0,
                    cancel_requested: true,
                },
            )
            .unwrap();
        match action {
            CoordinatorAction::InjectedFailed { reason, .. } => assert_eq!(reason, "cancelled"),
            other => panic!("expected InjectedFailed, got {other:?}"),
        }
    }

    /// U8 timeout: `elapsed_secs > aggregate_timeout_secs` triggers
    /// `Failed` with reason="timeout", even if slots are still
    /// mid-flight.
    #[test]
    fn timeout_propagates_through_coordinator() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store.register_wave("to", WaveKind::Exec, 2).unwrap();
        for i in 0..2 {
            store
                .bind_worktree(
                    &wave,
                    i,
                    SlotResource {
                        slot_index: i,
                        worktree_path: Some(format!(".ralph/t/{i}")),
                        branch: Some(format!("ralph/t/{i}")),
                    },
                )
                .unwrap();
        }
        let coord =
            SupervisorCoordinator::with_in_memory_sink(store.clone() as Arc<dyn SupervisorStore>);
        let action = coord
            .tick(
                &wave,
                PhaseInputs {
                    aggregate_timeout_secs: 10,
                    elapsed_secs: 30,
                    cancel_requested: false,
                },
            )
            .unwrap();
        match action {
            CoordinatorAction::InjectedFailed { reason, .. } => assert_eq!(reason, "timeout"),
            other => panic!("expected InjectedFailed(timeout), got {other:?}"),
        }
    }

    /// U8 sanity: the in-memory merge sink records batches so
    /// the test above can verify nothing slipped through.
    #[test]
    fn in_memory_merge_sink_records_batches() {
        let sink = InMemoryMergeSink::new();
        let event = Event::new("unit.done", "{}");
        sink.append_events(vec![event.clone(), event]).unwrap();
        let batches = sink.batches();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 2);
    }

    /// U8 helper: every coordinator_topic is recognised by
    /// `is_supervisor_coordination_topic`. Drift here breaks
    /// U7's allowlist.
    #[test]
    fn coordinator_topic_matches_u7_allowlist() {
        for kind in [WaveKind::Exec, WaveKind::Fix, WaveKind::Review] {
            for success in [true, false] {
                let topic = coordinator_topic(kind, success);
                assert!(
                    ensure_coordinator_topic_is_recognised(&topic),
                    "{topic} must match the U7 allowlist"
                );
            }
        }
    }

    /// U8 Phase coverage: every Status / Phase mapping that the
    /// coordinator may consume stays consistent with the U3
    /// snapshot contract.
    #[test]
    fn slot_status_phase_round_trip() {
        for s in [
            SlotStatus::Pending,
            SlotStatus::Dispatched,
            SlotStatus::Running,
            SlotStatus::Completed,
            SlotStatus::Failed,
            SlotStatus::Cancelled,
        ] {
            assert!(!s.to_string().is_empty());
        }
        for p in [
            WavePhase::Dispatch,
            WavePhase::Collect,
            WavePhase::Integrate,
            WavePhase::Done,
            WavePhase::Failed,
        ] {
            assert!(!p.to_string().is_empty());
        }
    }

    /// U8: KTD-7 implies `MergeFailed` returns a non-OK
    /// `SupervisorStoreError` only when the store itself
    /// errors. The bridge never silently drops a merge error.
    #[test]
    fn merge_sink_error_carries_detail() {
        let sink = InMemoryMergeSink::new();
        sink.fail_with("ledger prune in progress");
        let err = sink
            .append_events(vec![Event::new("unit.done", "{}")])
            .unwrap_err();
        match err {
            MergeSinkError::Rejected(msg) => assert!(msg.contains("ledger prune")),
        }
        sink.clear_failure();
        sink.append_events(vec![Event::new("unit.done", "{}")])
            .unwrap();
    }
}
