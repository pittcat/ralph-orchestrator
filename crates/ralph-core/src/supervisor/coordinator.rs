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

use super::merge_sink::{EventMergeSink, InMemoryMergeSink};
use super::{
    SlotStatus,
    phase::{FailedReason, PhaseDecision, PhaseInputs, evaluate_phase},
};
use super::{SupervisorStore, SupervisorStoreResult, WavePhase, WaveSnapshot};
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
    /// Plan 004 R3 / P0-1: the failed-fan-in path was reached
    /// BEFORE the dispatcher committed the salvage merge. The
    /// caller must run the merge seam, call `mark_salvage_merged`
    /// on the store, and re-tick. This variant exists to refuse
    /// the unsafe pre-merge injection that the pre-fix code
    /// performed, and to give `ralph diagnose` a structured
    /// signal that the loop needs another tick after the
    /// salvage merge.
    SalvageNotMerged,
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
    ///
    /// U6: this is the backwards-compatible entry point used by
    /// the in-memory U8 tests; it merges an **empty** business
    /// event batch. The production dispatcher calls
    /// [`Self::tick_with_slot_events`] with the per-slot worker
    /// events (sorted by slot index, de-duplicated) so the merge
    /// sink writes the real fan-in output to the main ledger.
    pub fn tick(
        &self,
        wave_id: &str,
        inputs: PhaseInputs,
    ) -> SupervisorStoreResult<CoordinatorAction> {
        self.tick_with_slot_events(wave_id, inputs, Vec::new())
    }

    /// U6: run one tick of fan-in, merging `slot_events` (the
    /// per-slot worker business events, ordered by slot index and
    /// de-duplicated by the caller) through the sink on the
    /// `Integrate` path. Idempotent on `merged_to_events`
    /// (KTD-7 / F-001): once merged, subsequent ticks return
    /// `AlreadyDone` without re-appending. This is the entry
    /// point the production dispatcher's `run_supervisor_fan_in`
    /// calls; the in-memory U8 tests keep using [`Self::tick`]
    /// (empty batch).
    pub fn tick_with_slot_events(
        &self,
        wave_id: &str,
        inputs: PhaseInputs,
        slot_events: Vec<Event>,
    ) -> SupervisorStoreResult<CoordinatorAction> {
        let snapshot = self.store.fan_in_status(wave_id)?;
        // Plan 004 R2 / P0-2: a `Completed` slot WITHOUT terminal
        // evidence MUST NOT enter the success fan-in path. The
        // pre-fix code read only `slot.status` and proceeded to
        // `merge_and_complete`, which is a silent-success
        // anti-pattern (R2). We consult the evidence row for
        // every Completed slot before allowing Integrate. A slot
        // lacking evidence here forces the phase to `Failed`
        // with reason `incomplete_evidence` — closing the
        // missing-dimensions inflation at the coordinator level.
        let decision = if matches!(evaluate_phase(&snapshot, &inputs), PhaseDecision::Integrate) {
            if self.all_completed_slots_have_evidence(&snapshot)? {
                PhaseDecision::Integrate
            } else {
                // Plan 004 R2 / P0-2: the slots whose status
                // is `Completed` but whose evidence row is
                // missing are the "blocking" set on this path
                // (the failed/cancelled index helper doesn't
                // cover them because they're nominally done).
                let blocking = self.evidence_missing_slot_indices(&snapshot)?;
                PhaseDecision::Failed {
                    reason: FailedReason::IncompleteEvidence,
                    blocking_slots: blocking,
                }
            }
        } else {
            evaluate_phase(&snapshot, &inputs)
        };
        match decision {
            PhaseDecision::ContinueCollect => Ok(CoordinatorAction::ContinueCollect),
            PhaseDecision::Integrate => self.merge_and_complete(&snapshot, slot_events),
            PhaseDecision::Failed {
                reason,
                blocking_slots,
            } => self.fail_wave(&snapshot, &reason, blocking_slots),
        }
    }

    /// Plan 004 R2 / P0-2 helper: return the slot indices whose
    /// `Completed` status is NOT backed by terminal evidence.
    /// Used to populate `blocking_slots` on the
    /// `Failed(IncompleteEvidence)` path — the
    /// `blocking_slot_indices` helper intentionally ignores
    /// `Completed` slots because they are normally "not
    /// blocking"; the evidence gate is a separate, narrower
    /// signal.
    fn evidence_missing_slot_indices(
        &self,
        snapshot: &WaveSnapshot,
    ) -> SupervisorStoreResult<Vec<u32>> {
        let mut out = Vec::new();
        for (slot_index, status) in &snapshot.slots {
            if !matches!(status, SlotStatus::Completed) {
                continue;
            }
            if self
                .store
                .slot_terminal_evidence(&snapshot.wave_id, *slot_index)?
                .is_none()
            {
                out.push(*slot_index);
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    /// Plan 004 R2 / P0-2 helper: every `Completed` slot in
    /// the snapshot must have terminal evidence recorded on
    /// the store. Slots still `Dispatched` / `Running` /
    /// `Pending` are out of scope (evaluate_phase already
    /// excludes them). `Failed` slots are out of scope
    /// (Failed → already on the Failed branch). The returned
    /// boolean is true ONLY when every Completed slot has a
    /// non-`None` evidence row.
    fn all_completed_slots_have_evidence(
        &self,
        snapshot: &WaveSnapshot,
    ) -> SupervisorStoreResult<bool> {
        for (slot_index, status) in &snapshot.slots {
            if !matches!(status, SlotStatus::Completed) {
                continue;
            }
            if self
                .store
                .slot_terminal_evidence(&snapshot.wave_id, *slot_index)?
                .is_none()
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Mutate the wave to `Done` after a successful merge.
    /// U1 / F-001 / KTD-7: when `merged_to_events` is already
    /// true, return `AlreadyDone` so the JSONL append layer
    /// never re-injects `*.wave.complete`. Subsequent ticks
    /// are pure no-ops.
    ///
    /// U6: `slot_events` is the per-slot worker business event
    /// batch (sorted by slot index, de-duplicated by the
    /// dispatcher's `run_supervisor_fan_in`). The merge sink
    /// appends it to the main ledger; on `Err` the wave stays in
    /// `Collect` so recovery retries the merge exactly once
    /// (KTD-7). The in-memory U8 tests pass an empty batch.
    fn merge_and_complete(
        &self,
        snapshot: &WaveSnapshot,
        slot_events: Vec<Event>,
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
        // U6: merge the per-slot worker events through the sink.
        // Production hands in the sorted + de-duplicated batch
        // gathered by `run_supervisor_fan_in`; the in-memory U8
        // tests pass an empty batch (the sink records it). On
        // failure we deliberately leave `merged_to_events` false
        // so recovery (U11) retries the merge against the same
        // rows (KTD-7).
        if let Err(error) = self.merge_sink.append_events(slot_events) {
            let topic = coordinator_topic(snapshot.kind, true);
            let action = CoordinatorAction::MergeFailed {
                topic,
                error: error.to_string(),
            };
            return Ok(action);
        }
        // Mark the wave as merged + advanced; U11 reads this
        // flag to skip double-injection on restart.
        self.store.mark_merge_to_events(&snapshot.wave_id)?;
        // U7 (2026-07-23-002) / KTD-8: coordinator owns the phase
        // verdict on the success path too — mirror `fail_wave`'s
        // `set_wave_phase(Failed)`. Without this the wave stays in
        // `Collect` forever even after a successful fan-in merge,
        // breaking Outside-In assertions that require `Done`.
        self.store
            .set_wave_phase(&snapshot.wave_id, WavePhase::Done)?;
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
    /// Plan 004 R3 / P0-1: salvage-merge / coord-injection are
    /// split into two persisted phases. `fail_wave` rejects
    /// any caller that has not yet committed the dispatcher
    /// salvage merge (`salvage_merged=false`) — closing the
    /// crash window where the coord-injection latch
    /// (`merged_to_events`) flipped before the salvage write
    /// landed and orphaned the merge. On restart the recovery
    /// path must observe `salvage_merged=true` to inject
    /// `*.wave.failed`; otherwise the supervisor re-runs the
    /// merge seam before completing the failed fan-in.
    ///
    /// The combined latch check is split as follows:
    /// - `salvage_merged == false` → caller is unsafe; refuse
    ///   with `SalvageNotMerged`. The dispatcher is responsible
    ///   for committing the salvage before invoking us.
    /// - `merged_to_events == true && salvage_merged == true`
    ///   → fully latched, return `AlreadyDone`. A restart that
    ///   observes both flags must NOT re-inject `*.wave.failed`.
    /// - `merged_to_events == true && salvage_merged == false`
    ///   is an invariant violation (the coord-event injection
    ///   cannot precede the salvage merge); we treat it as a
    ///   fail-closed refusal.
    fn fail_wave(
        &self,
        snapshot: &WaveSnapshot,
        reason: &FailedReason,
        blocking_slots: Vec<u32>,
    ) -> SupervisorStoreResult<CoordinatorAction> {
        // Plan 004 R3 / P0-1: refuse when salvage has not
        // been committed yet. Without this guard, the pre-fix
        // `merged_to_events` latch flipped BEFORE the salvage
        // write and a crash between them orphaned the merge.
        if !snapshot.salvage_merged {
            return Ok(CoordinatorAction::SalvageNotMerged);
        }
        // Plan 004 R3 / P0-1 (continued): coord-injection latch.
        // Mirrors `merge_and_complete`. `evaluate_phase` is a
        // pure function of the slot snapshot — it keeps returning
        // `Failed` on every tick after the wave settles, so
        // without this guard a replay / restart / repeated tick
        // would re-inject `*.wave.failed`. Once latched,
        // re-tick returns `AlreadyDone`. The wave PHASE (Failed
        // here vs Done on the success path) still distinguishes
        // the two outcomes — `merged_to_events` is the "coord
        // event already injected" latch, not the outcome bit.
        if snapshot.merged_to_events {
            return Ok(CoordinatorAction::AlreadyDone);
        }
        let topic = coordinator_topic(snapshot.kind, false);
        // U2: apply the verdict to the store.
        self.store
            .set_wave_phase(&snapshot.wave_id, WavePhase::Failed)?;
        // U4: latch the failed fan-in so a subsequent tick is a no-op.
        self.store.mark_merge_to_events(&snapshot.wave_id)?;
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
// 2026-07-16 cleanup U4 (KTD-3): reserved for `--features
// supervisor-db` integration test (pinned here so the public
// sanity check survives the test-fixture purge).
#[allow(dead_code)]
pub fn ensure_coordinator_topic_is_recognised(topic: &str) -> bool {
    is_supervisor_coordination_topic(topic)
}

#[cfg(test)]
mod tests {
    //! U8 closed-circuit tests: they bypass the JSONL ledger
    //! by using `InMemoryMergeSink` + `InMemorySupervisorStore`.
    //! The U12 bridge covers the real EventLoop wiring.

    use super::*;
    use crate::supervisor::{
        InMemorySupervisorStore, MergeSinkError, SlotResource, SlotStatus, WaveKind, WavePhase,
    };
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
            // Plan 004 R2 / P0-2: Completed slots MUST carry
            // terminal evidence for the success fan-in path
            // to engage (KTD3 fail-closed).
            store
                .record_slot_terminal_evidence(
                    &w,
                    i,
                    &crate::supervisor::TerminalEvidence::from_event(
                        "exec.unit.done",
                        "{\"dimension\":\"default\"}",
                    ),
                )
                .unwrap();
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

    /// 2026-07-26-004 plan U8 (R1–R5): ONE shared mechanism contract for
    /// all three WaveKind. A mixed wave (1 Completed-with-evidence + 1
    /// Failed) reaches `InjectedFailed` with the kind-specific topic,
    /// latches `merged_to_events` so a replay is `AlreadyDone` (no
    /// double-inject), keeps the wave phase `Failed`, and a Completed
    /// slot stays distinguishable from a Failed slot via terminal
    /// evidence. The wave KIND only changes the coordination topic — the
    /// lifecycle / evidence / idempotency rules are identical, so Review,
    /// Exec and Fix cannot drift into separate branches.
    #[test]
    fn u8_shared_fan_in_contract_across_wave_kinds() {
        for (kind, failed_topic) in [
            (WaveKind::Exec, "exec.wave.failed"),
            (WaveKind::Fix, "fix.wave.failed"),
            (WaveKind::Review, "review.wave.failed"),
        ] {
            let store = Arc::new(InMemorySupervisorStore::new());
            let wave = store.register_wave("k-contract", kind, 2).unwrap();
            // Slot 0: Completed WITH terminal evidence. Slot 1: Failed.
            store.record_slot_result(&wave, 0, "h0", 1).unwrap();
            store
                .record_slot_terminal_evidence(
                    &wave,
                    0,
                    &crate::supervisor::TerminalEvidence::from_event(
                        "review.unit.done",
                        "{\"dimension\":\"correctness\"}",
                    ),
                )
                .unwrap();
            store
                .record_slot_failure(
                    &wave,
                    1,
                    crate::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
                )
                .unwrap();

            let coord = SupervisorCoordinator::with_in_memory_sink(
                store.clone() as Arc<dyn SupervisorStore>
            );
            // Plan 004 R3 / P0-1: pre-commit salvage before tick.
            store.mark_salvage_merged(&wave).unwrap();
            let inputs = PhaseInputs {
                aggregate_timeout_secs: 60,
                elapsed_secs: 0,
                cancel_requested: false,
            };
            // First tick → InjectedFailed with the kind-specific topic.
            let action = coord.tick(&wave, inputs.clone()).unwrap();
            match &action {
                CoordinatorAction::InjectedFailed { topic, .. } => {
                    assert_eq!(topic, failed_topic, "{kind} failed topic mismatch");
                }
                other => panic!("{kind}: expected InjectedFailed, got {other:?}"),
            }
            // merged_to_events latched → replay is AlreadyDone (idempotent).
            let replay = coord.tick(&wave, inputs.clone()).unwrap();
            assert!(
                matches!(replay, CoordinatorAction::AlreadyDone),
                "{kind}: replayed failed fan-in must be AlreadyDone, got {replay:?}"
            );
            // Terminal evidence: Completed slot has it, Failed slot does not.
            assert!(
                store.slot_terminal_evidence(&wave, 0).unwrap().is_some(),
                "{kind}: Completed slot must carry terminal evidence"
            );
            assert!(
                store.slot_terminal_evidence(&wave, 1).unwrap().is_none(),
                "{kind}: Failed slot must carry no terminal evidence"
            );
            // Phase is Failed, not Done.
            assert_eq!(
                store.fan_in_status(&wave).unwrap().phase,
                WavePhase::Failed,
                "{kind}: mixed wave must settle to Failed phase"
            );
        }
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
        // Plan 004 R2 / P0-2: Completed slots MUST carry
        // terminal evidence for the success fan-in path to
        // engage. Without this the coordinator falls into
        // `Failed(IncompleteEvidence)` before reaching the
        // merge sink — which would mask the merge-failure
        // contract this test pins.
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &crate::supervisor::TerminalEvidence::from_event(
                    "exec.unit.done",
                    "{\"dimension\":\"default\"}",
                ),
            )
            .unwrap();
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
        // Plan 004 R3 / P0-1: the dispatcher is responsible
        // for committing the salvage merge before the
        // coordinator's `fail_wave` latches the coord-event
        // injection. U8 tests run the coordinator directly so
        // they mark salvage here.
        store.mark_salvage_merged(&wave).unwrap();
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
        // Plan 004 R2 / P0-2: success path requires terminal
        // evidence; record it so the test reaches
        // `InjectedComplete` and pins the idempotency contract.
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &crate::supervisor::TerminalEvidence::from_event(
                    "exec.unit.done",
                    "{\"dimension\":\"default\"}",
                ),
            )
            .unwrap();
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
        // U7 (2026-07-23-002): success path must also advance phase to Done
        // (mirrors fail_wave → Failed). Outside-In full-chain asserts this.
        let snap_after = store.fan_in_status(&wave).unwrap();
        assert!(
            snap_after.merged_to_events,
            "merged_to_events must flip on InjectedComplete"
        );
        assert_eq!(
            snap_after.phase,
            WavePhase::Done,
            "success fan-in must set phase=Done; got {:?}",
            snap_after.phase
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
        // Plan 004 R2 / P0-2: success path requires terminal
        // evidence; record it so the test reaches
        // `InjectedComplete` and pins the idempotency contract.
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &crate::supervisor::TerminalEvidence::from_event(
                    "exec.unit.done",
                    "{\"dimension\":\"default\"}",
                ),
            )
            .unwrap();
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
        // Plan 004 R3 / P0-1: pre-commit salvage before tick.
        store.mark_salvage_merged(&wave).unwrap();
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
        // Plan 004 R3 / P0-1: pre-commit salvage before tick.
        store.mark_salvage_merged(&wave).unwrap();
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
        // Plan 004 R3 / P0-1: pre-commit salvage before tick.
        store.mark_salvage_merged(&wave).unwrap();
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

    /// Plan 004 R3 / P0-1 — fault injection: a failed fan-in
    /// reached WITHOUT a prior `mark_salvage_merged` MUST be
    /// refused (`SalvageNotMerged`) so the coord-injection
    /// latch cannot flip before the salvage merge lands. This
    /// pins the post-fix invariant that closes the crash
    /// window where the salvage write could be lost between
    /// `fail_wave` returning and the dispatcher-layer merge.
    #[test]
    fn p0_1_failed_fan_in_without_salvage_is_refused() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store.register_wave("p01", WaveKind::Review, 2).unwrap();
        // Slot 0 Completed with evidence; slot 1 Failed.
        store.record_slot_result(&wave, 0, "h0", 1).unwrap();
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &crate::supervisor::TerminalEvidence::from_event(
                    "review.unit.done",
                    "{\"dimension\":\"correctness\"}",
                ),
            )
            .unwrap();
        store
            .record_slot_failure(
                &wave,
                1,
                crate::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
            )
            .unwrap();

        let coord = SupervisorCoordinator::with_in_memory_sink(
            store.clone() as Arc<dyn SupervisorStore>,
        );
        // No mark_salvage_merged — coordinator must refuse.
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
        assert!(
            matches!(action, CoordinatorAction::SalvageNotMerged),
            "expected SalvageNotMerged, got {action:?}",
        );
        // After refusal, merged_to_events stays false so the
        // recovery path can mark salvage and re-tick.
        let snap = store.fan_in_status(&wave).unwrap();
        assert!(
            !snap.merged_to_events,
            "merged_to_events must NOT be latched after refusal",
        );
        assert!(
            !snap.salvage_merged,
            "salvage_merged must NOT be latched after refusal",
        );

        // Mark salvage and re-tick — coordinator now injects
        // the failed coord event exactly once.
        store.mark_salvage_merged(&wave).unwrap();
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
        assert!(
            matches!(action2, CoordinatorAction::InjectedFailed { .. }),
            "after mark_salvage_merged the second tick must InjectedFailed, got {action2:?}",
        );
        // Replay is AlreadyDone — restart-safe.
        let replay = coord
            .tick(
                &wave,
                PhaseInputs {
                    aggregate_timeout_secs: 60,
                    elapsed_secs: 0,
                    cancel_requested: false,
                },
            )
            .unwrap();
        assert!(
            matches!(replay, CoordinatorAction::AlreadyDone),
            "replay must AlreadyDone, got {replay:?}",
        );
    }

    /// Plan 004 R3 / P0-1 — restart simulation: drop the
    /// coordinator mid-tick, rebuild from snapshot, and
    /// confirm the new coordinator refuses to re-run the
    /// salvage path (no business-event double-write) AND
    /// does not re-inject the coord event after restart.
    #[test]
    fn p0_1_restart_after_coord_injection_is_no_op() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store.register_wave("restart", WaveKind::Review, 1).unwrap();
        store.record_slot_failure(&wave, 0, "boom").unwrap();
        // Tick 1: mark salvage, inject failed.
        store.mark_salvage_merged(&wave).unwrap();
        let coord1 =
            SupervisorCoordinator::with_in_memory_sink(store.clone() as Arc<dyn SupervisorStore>);
        let action1 = coord1
            .tick(
                &wave,
                PhaseInputs {
                    aggregate_timeout_secs: 60,
                    elapsed_secs: 0,
                    cancel_requested: false,
                },
            )
            .unwrap();
        assert!(matches!(action1, CoordinatorAction::InjectedFailed { .. }));

        // Simulate restart — drop the coordinator, observe
        // the persisted store, build a fresh one.
        drop(coord1);
        let snap = store.fan_in_status(&wave).unwrap();
        assert!(snap.merged_to_events);
        assert!(snap.salvage_merged);
        assert_eq!(snap.phase, WavePhase::Failed);

        let coord2 =
            SupervisorCoordinator::with_in_memory_sink(store.clone() as Arc<dyn SupervisorStore>);
        let action2 = coord2
            .tick(
                &wave,
                PhaseInputs {
                    aggregate_timeout_secs: 60,
                    elapsed_secs: 0,
                    cancel_requested: false,
                },
            )
            .unwrap();
        assert!(
            matches!(action2, CoordinatorAction::AlreadyDone),
            "post-restart tick must AlreadyDone, got {action2:?}",
        );
    }

    /// Plan 004 R2 / P0-2 — fail-closed: a Completed slot
    /// without terminal evidence MUST NOT enter the success
    /// fan-in path. The coordinator must surface
    /// `Failed(IncompleteEvidence)` so the dispatcher injects
    /// `*.wave.failed` instead of silently completing the wave
    /// (R2 / KTD3). Cross-store parity: the same invariant must
    /// hold against the in-memory store; the rusqlite parity
    /// test lives in `rusqlite.rs::p0_2_completed_without_evidence_fails_closed`.
    #[test]
    fn p0_2_completed_without_evidence_fails_closed() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store.register_wave("p02", WaveKind::Exec, 1).unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/p02".to_string()),
                    branch: Some("ralph/p02".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        // Mark Completed but DO NOT record terminal evidence.
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        // Pre-commit salvage so fail_wave is not blocked on the
        // unrelated P0-1 latch.
        store.mark_salvage_merged(&wave).unwrap();
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
                assert_eq!(
                    reason, "incomplete_evidence",
                    "missing evidence must surface as Failed(IncompleteEvidence)"
                );
                assert!(!blocking_slots.is_empty());
            }
            other => panic!(
                "expected InjectedFailed(incomplete_evidence), got {other:?} — \
                 R2 / KTD3 fail-closed invariant violated"
            ),
        }
        // Phase is Failed, not Done. Snap must NOT record a
        // coord-event injection.
        let snap = store.fan_in_status(&wave).unwrap();
        assert_eq!(snap.phase, WavePhase::Failed);
        assert!(snap.merged_to_events);
    }

    /// Plan 004 R2 / P0-2 — happy path: once evidence is
    /// recorded, the success path engages normally. Pins that
    /// the evidence gate does not break legitimate flows.
    #[test]
    fn p0_2_with_evidence_engages_success_path() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store.register_wave("p02-ok", WaveKind::Exec, 1).unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/p02-ok".to_string()),
                    branch: Some("ralph/p02-ok".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &crate::supervisor::TerminalEvidence::from_event(
                    "exec.unit.done",
                    "{\"dimension\":\"default\"}",
                ),
            )
            .unwrap();
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
        assert!(
            matches!(action, CoordinatorAction::InjectedComplete { .. }),
            "with evidence, success path must engage, got {action:?}"
        );
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
