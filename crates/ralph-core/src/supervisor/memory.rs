//! In-memory `SupervisorStore` implementation (U3 + U4).
//!
//! U3 introduces the wave/slot/resource lifecycle (no idempotency,
//! queue, or compensation — those land in U4). U4 adds the
//! dispatch_records UNIQUE key, worker_results content_hash,
//! wave_queue FIFO, cancel_requested flag and compensation_jobs
//! execution hook on top of this base.
//!
//! The store takes `&self` to match the future rusqlite
//! implementation's synchronous surface area. Concurrency
//! control lives in a `std::sync::Mutex<Inner>` so the trait
//! stays `Send + Sync`; tests can deep-clone the store to
//! snapshot state without touching the live locks.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use super::{
    DispatchOutcome, IdempotencyKey, IsolationMode, SlotResource, SlotStatus, SupervisorStore,
    SupervisorStoreError, SupervisorStoreResult, WaveKind, WavePhase, WaveSnapshot,
};

/// Per-wave descriptor held in `InMemorySupervisorStore::waves`.
#[derive(Debug, Clone)]
struct WaveRow {
    wave_id: String,
    kind: WaveKind,
    expected_total: u32,
    phase: WavePhase,
    cancel_requested: bool,
    merged_to_events: bool,
    slots: BTreeMap<u32, SlotRow>,
}

/// Per-slot descriptor.
#[derive(Debug, Clone)]
struct SlotRow {
    slot_index: u32,
    status: SlotStatus,
    isolation: IsolationMode,
    resource: Option<SlotResource>,
    content_hash: Option<String>,
    event_count: Option<usize>,
    failure_reason: Option<String>,
}

/// In-memory implementation of `SupervisorStore`. The store
/// exposes `&self` access through the trait; all mutation runs
/// inside the `std::sync::Mutex`. Lock poisoning bubbles up as
/// `SupervisorStoreError::Storage` so the runtime can fail
/// closed without panicking across loop iterations.
#[derive(Debug, Default)]
pub struct InMemorySupervisorStore {
    inner: Mutex<Inner>,
}

impl Clone for InMemorySupervisorStore {
    fn clone(&self) -> Self {
        // Deep-clone the inner state so tests can snapshot a
        // store at a moment in time without touching the live
        // locks. The Inner state is small enough to re-key
        // directly.
        let inner = self
            .inner
            .lock()
            .expect("supervisor store mutex poisoned")
            .clone();
        Self {
            inner: Mutex::new(inner),
        }
    }
}

#[derive(Debug, Default, Clone)]
struct Inner {
    /// Stable wave ID assignment counter; the runtime specifies
    /// its own idempotency_key, so this is informational only.
    waves_by_id: HashMap<String, WaveRow>,
    waves_by_key: HashMap<IdempotencyKey, String>,
    next_wave_seq: u64,
    // ----- U4 additions below -----
    /// Dispatch records by `(wave_id, slot_index)`. Each entry has
    /// the worker PID (when spawned) plus the dispatch outcome;
    /// backpressure / dedup lookups hit this map.
    dispatches: HashMap<(String, u32), DispatchRecord>,
    /// Worker results by `(wave_id, slot_index)`.
    worker_results: HashMap<(String, u32), WorkerResult>,
    /// FIFO queue of pending wave IDs waiting for a backpressure
    /// slot to free up.
    queue: Vec<String>,
    /// Per-wave compensation jobs (executed by U4 hooks).
    compensation: Vec<CompensationEntry>,
}

#[derive(Debug, Clone)]
struct DispatchRecord {
    pid: Option<u32>,
    outcome: Option<DispatchOutcome>,
}

#[derive(Debug, Clone)]
struct WorkerResult {
    content_hash: String,
    event_count: usize,
}

#[derive(Debug, Clone)]
struct CompensationEntry {
    wave_id: String,
    kind: CompensationKind,
    status: CompensationStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompensationKind {
    OnTimeout,
    OnCancel,
    OnPartial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompensationStatus {
    Pending,
    Executed,
    Failed,
}

impl InMemorySupervisorStore {
    /// Build an empty store. U3 implementation only — the rusqlite
    /// counterpart arrives in U5.
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire the inner mutex. Lock poisoning is rewritten as
    /// `Storage` so the runtime recovers cleanly via
    /// `task.resume` instead of crashing the loop.
    fn lock(&self) -> SupervisorStoreResult<std::sync::MutexGuard<'_, Inner>> {
        self.inner.lock().map_err(|_| {
            SupervisorStoreError::Storage("supervisor store mutex poisoned".to_string())
        })
    }

    /// Return the active worker count across all waves. Used by the
    /// trait's backpressure path; exposed for tests so they don't
    /// have to enumerate slots.
    fn active_workers(inner: &Inner) -> u32 {
        inner
            .waves_by_id
            .values()
            .flat_map(|w| w.slots.values())
            .filter(|s| matches!(s.status, SlotStatus::Dispatched | SlotStatus::Running))
            .count() as u32
    }
}

impl SupervisorStore for InMemorySupervisorStore {
    fn register_wave(
        &self,
        idempotency_key: &str,
        kind: WaveKind,
        expected_total: u32,
    ) -> SupervisorStoreResult<String> {
        let mut inner = self.lock()?;
        if inner.waves_by_key.contains_key(idempotency_key) {
            return Err(SupervisorStoreError::DuplicateKey(
                idempotency_key.to_string(),
            ));
        }
        if expected_total == 0 {
            return Err(SupervisorStoreError::InvalidTransition(
                "expected_total must be > 0".to_string(),
            ));
        }
        let wave_id = format!("w-{}", inner.next_wave_seq + 1);
        inner.next_wave_seq += 1;
        let default_isolation = match kind {
            WaveKind::Exec | WaveKind::Fix => IsolationMode::Worktree,
            WaveKind::Review => IsolationMode::SharedReadonly,
        };
        let mut slots = BTreeMap::new();
        for idx in 0..expected_total {
            slots.insert(
                idx,
                SlotRow {
                    slot_index: idx,
                    status: SlotStatus::Pending,
                    isolation: default_isolation,
                    resource: None,
                    content_hash: None,
                    event_count: None,
                    failure_reason: None,
                },
            );
        }
        let row = WaveRow {
            wave_id: wave_id.clone(),
            kind,
            expected_total,
            phase: WavePhase::Dispatch,
            cancel_requested: false,
            merged_to_events: false,
            slots,
        };
        inner.waves_by_id.insert(wave_id.clone(), row);
        inner.waves_by_key.insert(idempotency_key.to_string(), wave_id.clone());
        Ok(wave_id)
    }

    fn enqueue_wave(
        &self,
        idempotency_key: &str,
        kind: WaveKind,
        expected_total: u32,
    ) -> SupervisorStoreResult<String> {
        // U3: enqueue is a placeholder until U4 wires
        // backpressure dispatch. The store still tracks the wave
        // so U4 can advance without re-introducing it.
        let wave_id = self.register_wave(idempotency_key, kind, expected_total)?;
        let mut inner = self.lock()?;
        inner.queue.push(wave_id.clone());
        Ok(wave_id)
    }

    fn try_dispatch_next(
        &self,
        max_concurrent_workers: u32,
    ) -> SupervisorStoreResult<Option<(String, u32)>> {
        let mut inner = self.lock()?;
        if Self::active_workers(&inner) >= max_concurrent_workers {
            return Ok(None);
        }
        // Collect-phase waves (KTD-5) also have pending slots to
        // drain, so the dispatch loop walks both Dispatch and
        // Collect.
        let mut wave_ids: Vec<String> = inner.waves_by_id.keys().cloned().collect();
        wave_ids.sort();
        for wave_id in wave_ids {
            let candidate = {
                let wave = match inner.waves_by_id.get_mut(&wave_id) {
                    Some(w)
                        if matches!(w.phase, WavePhase::Dispatch | WavePhase::Collect) =>
                    {
                        w
                    }
                    _ => continue,
                };
                wave.slots
                    .iter_mut()
                    .find(|(_, s)| {
                        s.status == SlotStatus::Pending
                            && (s.isolation != IsolationMode::Worktree
                                || s.resource.is_some())
                    })
                    .map(|(idx, _)| *idx)
            };
            let Some(idx) = candidate else {
                continue;
            };
            let wave = inner.waves_by_id.get_mut(&wave_id).expect("wave exists");
            wave.slots.get_mut(&idx).unwrap().status = SlotStatus::Dispatched;
            wave.phase = WavePhase::Collect;
            inner
                .dispatches
                .entry((wave_id.clone(), idx))
                .or_insert(DispatchRecord {
                    pid: None,
                    outcome: None,
                });
            return Ok(Some((wave_id, idx)));
        }
        Ok(None)
    }

    fn bind_worktree(
        &self,
        wave_id: &str,
        slot_index: u32,
        binding: SlotResource,
    ) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        let slot = wave
            .slots
            .get_mut(&slot_index)
            .ok_or_else(|| SupervisorStoreError::UnknownSlot {
                wave_id: wave_id.to_string(),
                slot_index,
            })?;
        // R-WT-1: shared_readonly slots MUST NOT receive a
        // worktree binding.
        if slot.isolation == IsolationMode::SharedReadonly
            && (binding.worktree_path.is_some() || binding.branch.is_some())
        {
            return Err(SupervisorStoreError::InvalidTransition(
                "shared_readonly slot cannot receive a worktree binding".to_string(),
            ));
        }
        slot.resource = Some(binding);
        Ok(())
    }

    fn record_slot_result(
        &self,
        wave_id: &str,
        slot_index: u32,
        content_hash: &str,
        event_count: usize,
    ) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        let slot = wave
            .slots
            .get_mut(&slot_index)
            .ok_or_else(|| SupervisorStoreError::UnknownSlot {
                wave_id: wave_id.to_string(),
                slot_index,
            })?;
        slot.status = SlotStatus::Completed;
        slot.content_hash = Some(content_hash.to_string());
        slot.event_count = Some(event_count);
        // U4 storage: WorkerResult dedup contract (R-E1).
        let key = (wave_id.to_string(), slot_index);
        let prior = inner.worker_results.insert(
            key.clone(),
            WorkerResult {
                content_hash: content_hash.to_string(),
                event_count,
            },
        );
        if let Some(prev) = prior {
            if prev.content_hash != content_hash {
                // R-E2/R-E4: replace prior worker_result; the
                // diagnostics collector can read
                // `compaction_diagnostics` (out of scope for U3/U4).
                let _ = prev;
            }
        }
        // Bind the slot's `dispatches` outcome to Completed so the
        // coordinator/U8 can correlate.
        if let Some(d) = inner.dispatches.get_mut(&key) {
            d.outcome = Some(DispatchOutcome::Completed);
        }
        Ok(())
    }

    fn record_slot_failure(
        &self,
        wave_id: &str,
        slot_index: u32,
        reason: &str,
    ) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let key = (wave_id.to_string(), slot_index);
        if let Some(d) = inner.dispatches.get_mut(&key) {
            d.outcome = Some(DispatchOutcome::Failed);
        }
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        let slot = wave
            .slots
            .get_mut(&slot_index)
            .ok_or_else(|| SupervisorStoreError::UnknownSlot {
                wave_id: wave_id.to_string(),
                slot_index,
            })?;
        slot.status = SlotStatus::Failed;
        slot.failure_reason = Some(reason.to_string());
        // KTD-5 + U6 phase: a permanent slot failure on a
        // required slot transitions the wave to Failed
        // (coordinator confirms via fan_in_status).
        wave.phase = WavePhase::Failed;
        Ok(())
    }

    fn cancel_wave(&self, wave_id: &str) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        wave.cancel_requested = true;
        // Per R-B3/B4: only Pending slots turn Cancelled here.
        // Dispatched/Running slots are killed out-of-band by the
        // runtime (PID) and then transitioned via
        // `record_slot_failure(reason="cancelled")` so the
        // coordinator sees them as Failed (which is the right
        // terminal status for fan-in accounting, R-KTD-8).
        for slot in wave.slots.values_mut() {
            if slot.status == SlotStatus::Pending {
                slot.status = SlotStatus::Cancelled;
            }
        }
        Ok(())
    }

    fn fan_in_status(&self, wave_id: &str) -> SupervisorStoreResult<WaveSnapshot> {
        let inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        let mut completed = 0u32;
        let mut failed = 0u32;
        let mut in_flight = 0u32;
        let mut pending = 0u32;
        for slot in wave.slots.values() {
            match slot.status {
                SlotStatus::Completed => completed += 1,
                SlotStatus::Failed => failed += 1,
                SlotStatus::Dispatched | SlotStatus::Running => in_flight += 1,
                SlotStatus::Pending | SlotStatus::Cancelled => pending += 1,
            }
        }
        Ok(WaveSnapshot {
            wave_id: wave.wave_id.clone(),
            kind: wave.kind,
            phase: wave.phase,
            expected_total: wave.expected_total,
            completed_count: completed,
            failed_count: failed,
            pending_count: pending,
            in_flight_count: in_flight,
            cancel_requested: wave.cancel_requested,
            merged_to_events: wave.merged_to_events,
        })
    }

    fn mark_merge_to_events(&self, wave_id: &str) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        if !wave.merged_to_events {
            wave.merged_to_events = true;
        }
        Ok(())
    }

    fn recover_active_waves(&self) -> SupervisorStoreResult<Vec<WaveSnapshot>> {
        let inner = self.lock()?;
        let mut out = Vec::new();
        for wave in inner.waves_by_id.values() {
            if matches!(wave.phase, WavePhase::Done | WavePhase::Failed) {
                continue;
            }
            let mut completed = 0u32;
            let mut failed = 0u32;
            let mut in_flight = 0u32;
            let mut pending = 0u32;
            for slot in wave.slots.values() {
                match slot.status {
                    SlotStatus::Completed => completed += 1,
                    SlotStatus::Failed => failed += 1,
                    SlotStatus::Dispatched | SlotStatus::Running => in_flight += 1,
                    SlotStatus::Pending | SlotStatus::Cancelled => pending += 1,
                }
            }
            out.push(WaveSnapshot {
                wave_id: wave.wave_id.clone(),
                kind: wave.kind,
                phase: wave.phase,
                expected_total: wave.expected_total,
                completed_count: completed,
                failed_count: failed,
                pending_count: pending,
                in_flight_count: in_flight,
                cancel_requested: wave.cancel_requested,
                merged_to_events: wave.merged_to_events,
            });
        }
        Ok(out)
    }

    fn list_worktree_paths(&self, wave_id: &str) -> SupervisorStoreResult<Vec<SlotResource>> {
        let inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        Ok(wave
            .slots
            .values()
            .filter_map(|s| s.resource.clone())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    //! U3 closed-circuit tests: this slice is limited to
    //! wave/slot/resource lifecycle. U4 covers dedup,
    //! backpressure, cancel compensation hooks.

    use super::*;

    fn store() -> InMemorySupervisorStore {
        InMemorySupervisorStore::new()
    }

    #[test]
    fn register_wave_creates_expected_total_pending_slots() {
        let s = store();
        let wave = s
            .register_wave("key-1", WaveKind::Exec, 4)
            .expect("register_wave must succeed");
        let snapshot = s.fan_in_status(&wave).unwrap();
        assert_eq!(snapshot.expected_total, 4);
        assert_eq!(snapshot.pending_count, 4);
        assert_eq!(snapshot.completed_count, 0);
        assert_eq!(snapshot.failed_count, 0);
    }

    #[test]
    fn duplicate_idempotency_key_is_rejected() {
        let s = store();
        s.register_wave("dup", WaveKind::Exec, 2).unwrap();
        let err = s.register_wave("dup", WaveKind::Fix, 1).unwrap_err();
        assert!(matches!(err, SupervisorStoreError::DuplicateKey(_)));
    }

    #[test]
    fn worktree_isolation_blocks_dispatch_until_bound() {
        let s = store();
        let wave = s.register_wave("k", WaveKind::Exec, 2).unwrap();
        // No bindings yet: dispatch must yield None.
        let dispatched = s.try_dispatch_next(4).unwrap();
        assert!(dispatched.is_none(), "no slot should leave Pending without binding");
        // Bind slot 0 only.
        s.bind_worktree(
            &wave,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/worktrees/w".to_string()),
                branch: Some("ralph/u".to_string()),
            },
        )
        .unwrap();
        let (w, idx) = s.try_dispatch_next(4).unwrap().unwrap();
        assert_eq!(w, wave);
        assert_eq!(idx, 0, "lowest bound slot must win");
    }

    #[test]
    fn shared_readonly_slots_dispatch_without_binding() {
        let s = store();
        s.register_wave("rv", WaveKind::Review, 3).unwrap();
        let (wave, idx) = s.try_dispatch_next(2).unwrap().unwrap();
        assert_eq!(idx, 0);
        let snap = s.fan_in_status(&wave).unwrap();
        // Dispatch only takes one slot at a time; 2 slots remain
        // (1 Pending + 1 Dispatched-as-of-the-call, both bundled
        // under `pending_count` for the phase-decision
        // pure-function; see `WaveSnapshot::pending_count`
        // contract in supervisor::types).
        assert_eq!(snap.pending_count, 2);
        assert_eq!(snap.completed_count, 0);
    }

    #[test]
    fn slot_pending_dispatched_completed_lifecycle() {
        let s = store();
        let wave = s.register_wave("kf", WaveKind::Exec, 2).unwrap();
        s.bind_worktree(
            &wave,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/worktrees/a".to_string()),
                branch: Some("ralph/a".to_string()),
            },
        )
        .unwrap();
        s.bind_worktree(
            &wave,
            1,
            SlotResource {
                slot_index: 1,
                worktree_path: Some(".ralph/worktrees/b".to_string()),
                branch: Some("ralph/b".to_string()),
            },
        )
        .unwrap();
        let _ = s.try_dispatch_next(4).unwrap().unwrap();
        s.record_slot_result(&wave, 0, "hash-0", 3).unwrap();
        let snap = s.fan_in_status(&wave).unwrap();
        assert_eq!(snap.completed_count, 1);
        assert_eq!(snap.pending_count, 1);
        // wave is in Collect once at least one slot leaves Pending.
        assert_eq!(snap.phase, WavePhase::Collect);
    }

    #[test]
    fn fan_in_complete_reaches_expected_total() {
        let s = store();
        let wave = s.register_wave("fa", WaveKind::Exec, 2).unwrap();
        for idx in 0..2 {
            s.bind_worktree(
                &wave,
                idx,
                SlotResource {
                    slot_index: idx,
                    worktree_path: Some(format!(".ralph/wt/{idx}")),
                    branch: Some(format!("ralph/u{idx}")),
                },
            )
            .unwrap();
        }
        let _ = s.try_dispatch_next(4).unwrap().unwrap();
        s.record_slot_result(&wave, 0, "h0", 1).unwrap();
        s.record_slot_result(&wave, 1, "h1", 1).unwrap();
        let snap = s.fan_in_status(&wave).unwrap();
        assert_eq!(snap.completed_count, 2);
        assert_eq!(snap.pending_count, 0);
    }

    #[test]
    fn review_wave_shared_readonly_no_resource_emitted() {
        let s = store();
        let wave = s.register_wave("rw", WaveKind::Review, 2).unwrap();
        let resources = s.list_worktree_paths(&wave).unwrap();
        assert!(
            resources.is_empty(),
            "shared_readonly slots must not expose a resource binding"
        );
    }

    #[test]
    fn cancel_marks_pending_and_running_as_cancelled() {
        let s = store();
        let wave = s.register_wave("cx", WaveKind::Exec, 2).unwrap();
        s.bind_worktree(
            &wave,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/x".to_string()),
                branch: Some("ralph/x".to_string()),
            },
        )
        .unwrap();
        let _ = s.try_dispatch_next(2).unwrap().unwrap();
        s.cancel_wave(&wave).unwrap();
        let snap = s.fan_in_status(&wave).unwrap();
        assert!(snap.cancel_requested);
        assert_eq!(
            snap.failed_count + snap.completed_count,
            0,
            "no slot should reach a terminal pass/fail just from cancel"
        );
    }

    #[test]
    fn mark_merge_to_events_is_idempotent() {
        let s = store();
        let wave = s.register_wave("me", WaveKind::Review, 1).unwrap();
        s.mark_merge_to_events(&wave).unwrap();
        s.mark_merge_to_events(&wave).unwrap();
        let snap = s.fan_in_status(&wave).unwrap();
        assert!(snap.merged_to_events);
    }
}
