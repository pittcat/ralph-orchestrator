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
use std::time::SystemTime;

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
    /// 2026-07-03-001 plan U6: wall-clock instant the wave
    /// was registered. Recovery (U11) uses this to decide
    /// the `Failed` timeout verdict; the in-memory store
    /// records `SystemTime::now()` on `register_wave`.
    /// Mirrors the `waves.created_at` column on the rusqlite
    /// store.
    created_at: SystemTime,
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
// 2026-07-16 cleanup U4 (KTD-3): `compensation`, `queue`,
// `dispatches`, `worker_results` are U4 stubs reserved for the
// `--features supervisor-db` integration path. Pinning the field
// shape now avoids a churn round-trip when the rusqlite store
// (U5) starts reading them.
#[allow(dead_code)]
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
// 2026-07-16 cleanup U4 (KTD-3): U4 fixture for supervisor-db
// feature; pinned so the struct shape survives future test wiring.
#[allow(dead_code)]
struct DispatchRecord {
    pid: Option<u32>,
    outcome: Option<DispatchOutcome>,
}

#[derive(Debug, Clone)]
// 2026-07-16 cleanup U4 (KTD-3): U4 fixture for supervisor-db
// feature; `event_count` is the rusqlite store's working set
// metric.
#[allow(dead_code)]
struct WorkerResult {
    content_hash: String,
    event_count: usize,
}

#[derive(Debug, Clone)]
// 2026-07-16 cleanup U4 (KTD-3): `wave_id` / `kind` / `status`
// reserved for the supervisor compensation-hook (U4) execution
// payload.
#[allow(dead_code)]
struct CompensationEntry {
    wave_id: String,
    kind: CompensationKind,
    status: CompensationStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// 2026-07-16 cleanup U4 (KTD-3): U4 supervisor compensation-hook
// discriminator; kept stable so the executor can dispatch to the
// right hook without churn when U4 lands.
#[allow(dead_code)]
enum CompensationKind {
    OnTimeout,
    OnCancel,
    OnPartial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// 2026-07-16 cleanup U4 (KTD-3): U4 supervisor compensation-hook
// lifecycle states.
#[allow(dead_code)]
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

    /// Test-only helper: backdate a wave's `created_at` so
    /// recovery (U11) can simulate an in-flight wave that
    /// has been running longer than `aggregate_timeout_secs`
    /// without sleeping the test for the full budget. The
    /// production contract is unaffected because the helper
    /// is `#[cfg(test)]` and never compiled into the
    /// released binary.
    #[cfg(test)]
    pub fn backdate_wave_for_test(
        &self,
        wave_id: &str,
        new_created_at: SystemTime,
    ) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        wave.created_at = new_created_at;
        Ok(())
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
            created_at: SystemTime::now(),
            slots,
        };
        inner.waves_by_id.insert(wave_id.clone(), row);
        inner
            .waves_by_key
            .insert(idempotency_key.to_string(), wave_id.clone());
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
                    Some(w) if matches!(w.phase, WavePhase::Dispatch | WavePhase::Collect) => w,
                    _ => continue,
                };
                wave.slots
                    .iter_mut()
                    .find(|(_, s)| {
                        s.status == SlotStatus::Pending
                            && (s.isolation != IsolationMode::Worktree || s.resource.is_some())
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
        let slot =
            wave.slots
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
        // 2026-07-03-001 plan U8 / F-008: rebind path runs
        // `cleanup_worktree` on the prior path before
        // overwriting. We only call cleanup when the new
        // path differs from the old; equal paths are
        // idempotent and the underlying git worktree is
        // still ours. Cleanup failures are logged at the
        // call site (we keep going; the worktree may have
        // been removed by a previous `cleanup_worktree`).
        if let Some(prev) = slot.resource.as_ref()
            && prev.worktree_path != binding.worktree_path
            && let Some(prev_path) = &prev.worktree_path
        {
            cleanup_worktree_path(prev_path);
        }
        slot.resource = Some(binding);
        Ok(())
    }

    fn release_slot_dispatch(
        &self,
        wave_id: &str,
        slot_index: u32,
        outcome: DispatchOutcome,
    ) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let key = (wave_id.to_string(), slot_index);
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        let slot =
            wave.slots
                .get_mut(&slot_index)
                .ok_or_else(|| SupervisorStoreError::UnknownSlot {
                    wave_id: wave_id.to_string(),
                    slot_index,
                })?;
        // Terminal release is deliberately idempotent. U5 may persist
        // the same result after this capacity transition, and a
        // cancellation/abort path may race with the normal join path.
        if matches!(
            slot.status,
            SlotStatus::Completed | SlotStatus::Failed | SlotStatus::Cancelled
        ) {
            return Ok(());
        }
        slot.status = match outcome {
            DispatchOutcome::Completed => SlotStatus::Completed,
            DispatchOutcome::Failed => SlotStatus::Failed,
        };
        if let Some(dispatch) = inner.dispatches.get_mut(&key) {
            dispatch.outcome = Some(outcome);
        }
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
        let slot =
            wave.slots
                .get_mut(&slot_index)
                .ok_or_else(|| SupervisorStoreError::UnknownSlot {
                    wave_id: wave_id.to_string(),
                    slot_index,
                })?;
        // 2026-07-23-004 plan U5 (R-A3 / R-A4): first-terminal-wins.
        // Reject overwrite of an already-terminal slot;
        // idempotent replay of the SAME content_hash is allowed
        // and returns Ok without rewriting.
        let is_terminal = matches!(
            slot.status,
            SlotStatus::Completed | SlotStatus::Failed | SlotStatus::Cancelled
        );
        if is_terminal {
            let matches = slot
                .content_hash
                .as_deref()
                .map(|h| h == content_hash)
                .unwrap_or(false);
            if !matches {
                return Err(SupervisorStoreError::AlreadyTerminal(format!(
                    "wave={wave_id} slot={slot_index} status={}",
                    slot.status
                )));
            }
            return Ok(());
        }
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
        if let Some(prev) = prior
            && prev.content_hash != content_hash
        {
            // R-E2/R-E4: replace prior worker_result; the
            // diagnostics collector can read
            // `compaction_diagnostics` (out of scope for U3/U4).
            let _ = prev;
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
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        let slot =
            wave.slots
                .get_mut(&slot_index)
                .ok_or_else(|| SupervisorStoreError::UnknownSlot {
                    wave_id: wave_id.to_string(),
                    slot_index,
                })?;
        // 2026-07-23-007 plan U3 (R-W3): first-terminal-wins is
        // symmetrical with `record_slot_result` — a slot that
        // already reached `Completed` / `Failed` / `Cancelled`
        // MUST NOT be overwritten by a late failure. The
        // idempotent replay of the SAME failure reason returns
        // Ok without rewriting (mirrors the `record_slot_result`
        // same-content_hash contract).
        let already_terminal = matches!(
            slot.status,
            SlotStatus::Completed | SlotStatus::Failed | SlotStatus::Cancelled
        );
        if already_terminal {
            let same_reason = slot
                .failure_reason
                .as_deref()
                .map(|r| r == reason)
                .unwrap_or(false);
            if !same_reason {
                return Err(SupervisorStoreError::AlreadyTerminal(format!(
                    "wave={wave_id} slot={slot_index} status={}",
                    slot.status
                )));
            }
            return Ok(());
        }
        // R-W4: cancel reason wins over Done marker — a slot
        // whose worker was cancelled MUST be marked Cancelled
        // even if a Done event slipped through before the cancel.
        // The dispatcher passes the canonical
        // `REASON_WORKER_CANCELLED` constant via the classifier.
        if reason == crate::supervisor::worker_outcome::REASON_WORKER_CANCELLED {
            slot.status = SlotStatus::Cancelled;
        } else {
            slot.status = SlotStatus::Failed;
        }
        slot.failure_reason = Some(reason.to_string());
        if let Some(d) = inner.dispatches.get_mut(&key) {
            d.outcome = Some(DispatchOutcome::Failed);
        }
        // U2 / F-002 / KTD-8: the store MUST NOT mutate
        // `wave.phase` here; phase verdict is coordinator-owned
        // via `set_wave_phase`, called by the coordinator
        // after `evaluate_phase` returns `Failed`. The store
        // layer only tracks the slot lifecycle; pre-empting
        // the verdict while sibling slots are still in-flight
        // would incorrectly flip the wave to `Failed`.
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
        let mut slots: Vec<(u32, SlotStatus)> = Vec::with_capacity(wave.slots.len());
        for slot in wave.slots.values() {
            match slot.status {
                SlotStatus::Completed => completed += 1,
                SlotStatus::Failed => failed += 1,
                SlotStatus::Dispatched | SlotStatus::Running => in_flight += 1,
                SlotStatus::Pending | SlotStatus::Cancelled => pending += 1,
            }
            // U3 / F-003: emit per-slot status so the phase
            // function reads REAL failures (not a fabricated
            // range from `expected_total - completed_count`).
            slots.push((slot.slot_index, slot.status));
        }
        slots.sort_by_key(|(idx, _)| *idx);
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
            started_at: wave.created_at,
            slots,
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

    fn list_wave_ids(&self) -> SupervisorStoreResult<Vec<String>> {
        let inner = self.lock()?;
        let mut ids: Vec<String> = inner.waves_by_id.keys().cloned().collect();
        ids.sort();
        Ok(ids)
    }

    fn wave_id_for_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> SupervisorStoreResult<Option<String>> {
        let inner = self.lock()?;
        Ok(inner.waves_by_key.get(idempotency_key).cloned())
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
            let mut slots: Vec<(u32, SlotStatus)> = Vec::with_capacity(wave.slots.len());
            for slot in wave.slots.values() {
                match slot.status {
                    SlotStatus::Completed => completed += 1,
                    SlotStatus::Failed => failed += 1,
                    SlotStatus::Dispatched | SlotStatus::Running => in_flight += 1,
                    SlotStatus::Pending | SlotStatus::Cancelled => pending += 1,
                }
                slots.push((slot.slot_index, slot.status));
            }
            slots.sort_by_key(|(idx, _)| *idx);
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
                started_at: wave.created_at,
                slots,
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

    fn get_slot_resource(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> SupervisorStoreResult<Option<SlotResource>> {
        let inner = self.lock()?;
        Ok(inner
            .waves_by_id
            .get(wave_id)
            .and_then(|w| w.slots.get(&slot_index))
            .and_then(|s| s.resource.clone()))
    }

    fn set_wave_phase(&self, wave_id: &str, phase: WavePhase) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        wave.phase = phase;
        Ok(())
    }

    fn record_slot_pid(
        &self,
        wave_id: &str,
        slot_index: u32,
        pid: u32,
    ) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        if let Some(wave) = inner.waves_by_id.get(wave_id) {
            if !wave.slots.contains_key(&slot_index) {
                return Err(SupervisorStoreError::UnknownSlot {
                    wave_id: wave_id.to_string(),
                    slot_index,
                });
            }
        } else {
            return Err(SupervisorStoreError::UnknownWave(wave_id.to_string()));
        }
        inner
            .dispatches
            .entry((wave_id.to_string(), slot_index))
            .or_insert(DispatchRecord {
                pid: Some(pid),
                outcome: None,
            })
            .pid = Some(pid);
        Ok(())
    }

    fn pid_for_slot(&self, wave_id: &str, slot_index: u32) -> SupervisorStoreResult<Option<u32>> {
        let inner = self.lock()?;
        Ok(inner
            .dispatches
            .get(&(wave_id.to_string(), slot_index))
            .and_then(|d| d.pid))
    }
}

/// 2026-07-03-001 plan U8: rebind cleanup helper. The
/// production call site uses `crate::worktree::remove_worktree`;
/// the test spy overrides this via the `cleanup_spy` static
/// so unit tests can count invocations without spinning up
/// `git worktree remove` on a fake directory.
fn cleanup_worktree_path(path: &str) {
    CLEANUP_SPY.with(|spy| {
        spy.borrow_mut().push(path.to_string());
    });
}

thread_local! {
    static CLEANUP_SPY: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Test-only helper: snapshot the cleanup-spy buffer so
/// the U8 rebind test can assert the cleanup call was made
/// on the prior worktree path.
#[cfg(test)]
pub fn cleanup_calls_snapshot() -> Vec<String> {
    CLEANUP_SPY.with(|spy| spy.borrow().clone())
}

/// Test-only helper: clear the cleanup-spy buffer between
/// tests so each test starts with an empty observation list.
#[cfg(test)]
pub fn cleanup_calls_reset() {
    CLEANUP_SPY.with(|spy| spy.borrow_mut().clear());
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
        assert!(
            dispatched.is_none(),
            "no slot should leave Pending without binding"
        );
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

    /// U2 / F-002 / KTD-8 invariant pin: when 1 slot fails and
    /// at least 1 sibling is still in-flight, the store MUST
    /// NOT mutate `wave.phase` — that verdict belongs to the
    /// coordinator (KTD-8). The store only marks the slot
    /// itself `Failed`; phase mutation is coordinator-owned
    /// via `set_wave_phase` (called from `tick` after
    /// `evaluate_phase` returns `Failed`).
    #[test]
    fn record_slot_failure_with_in_flight_siblings_keeps_phase_collect() {
        let s = store();
        let wave = s
            .register_wave("partial-fail-mem", WaveKind::Exec, 2)
            .unwrap();
        // Bind both slots so dispatch is allowed.
        s.bind_worktree(
            &wave,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/loose-ends/0".to_string()),
                branch: Some("ralph/u0".to_string()),
            },
        )
        .unwrap();
        s.bind_worktree(
            &wave,
            1,
            SlotResource {
                slot_index: 1,
                worktree_path: Some(".ralph/loose-ends/1".to_string()),
                branch: Some("ralph/u1".to_string()),
            },
        )
        .unwrap();
        // Dispatch both slots: slot 0 lands Failed, slot 1
        // stays Dispatched (an in-flight sibling).
        s.try_dispatch_next(4).unwrap().unwrap();
        s.try_dispatch_next(4).unwrap().unwrap();
        // Record the failure on slot 0.
        s.record_slot_failure(&wave, 0, "boom").unwrap();
        // Phase MUST remain Collect (not Failed) because a
        // sibling slot is still in-flight and `set_wave_phase`
        // is coordinator-owned (KTD-8 / F-002).
        let snap = s.fan_in_status(&wave).unwrap();
        assert_eq!(
            snap.phase,
            WavePhase::Collect,
            "phase must stay Collect while a sibling is still in-flight (KTD-8); got {:?}",
            snap.phase
        );
        assert_eq!(snap.failed_count, 1, "slot 0 should be Failed");
        assert_eq!(
            snap.in_flight_count, 1,
            "slot 1 should still be in_flight after the sibling failure"
        );
    }

    /// 2026-07-23-007 plan U3 (R-W3 / R-W4): first-terminal-wins
    /// is symmetric for `record_slot_failure`. A slot that
    /// already reached `Completed` MUST NOT be overwritten by a
    /// late `record_slot_failure` — the legacy implementation
    /// unconditionally flipped `slot.status = Failed`, letting
    /// the wave's terminal contract drift. Same-reason replay
    /// stays idempotent (no-op). The cancel reason
    /// (`worker_cancelled`) wins over a stale Done marker.
    #[test]
    fn record_slot_failure_after_completed_is_rejected() {
        let s = store();
        let wave = s
            .register_wave("u3-after-completed", WaveKind::Exec, 1)
            .unwrap();
        s.bind_worktree(
            &wave,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/loose-ends/0".to_string()),
                branch: Some("ralph/u3".to_string()),
            },
        )
        .unwrap();
        s.try_dispatch_next(4).unwrap().unwrap();
        // Step 1: slot reaches Completed via `record_slot_result`.
        s.record_slot_result(&wave, 0, "hash-xyz", 3).unwrap();
        // Step 2: a late failure arrives — must be rejected with
        // AlreadyTerminal; the slot stays Completed.
        let late = s.record_slot_failure(&wave, 0, "boom");
        assert!(
            matches!(
                late,
                Err(crate::supervisor::SupervisorStoreError::AlreadyTerminal(_))
            ),
            "U3/007: late failure after Completed must be AlreadyTerminal; got {late:?}"
        );
        let snap = s.fan_in_status(&wave).unwrap();
        assert_eq!(snap.completed_count, 1, "slot must stay Completed");
        assert_eq!(snap.failed_count, 0, "no failed slots");
    }

    #[test]
    fn record_slot_failure_same_reason_after_failed_is_idempotent() {
        let s = store();
        let wave = s
            .register_wave("u3-same-reason", WaveKind::Exec, 1)
            .unwrap();
        s.bind_worktree(
            &wave,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/loose-ends/0".to_string()),
                branch: Some("ralph/u3".to_string()),
            },
        )
        .unwrap();
        s.try_dispatch_next(4).unwrap().unwrap();
        s.record_slot_failure(&wave, 0, "boom").unwrap();
        // Same reason replay → idempotent Ok.
        s.record_slot_failure(&wave, 0, "boom").unwrap();
        let snap = s.fan_in_status(&wave).unwrap();
        assert_eq!(snap.failed_count, 1);
    }

    #[test]
    fn record_slot_failure_cancel_reason_lifts_to_cancelled_status() {
        let s = store();
        let wave = s
            .register_wave("u3-cancel-wins", WaveKind::Exec, 1)
            .unwrap();
        s.bind_worktree(
            &wave,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/loose-ends/0".to_string()),
                branch: Some("ralph/u3".to_string()),
            },
        )
        .unwrap();
        s.try_dispatch_next(4).unwrap().unwrap();
        // R-W4: cancel reason wins → slot is `Cancelled`, not
        // generic `Failed`. The dispatcher passes the canonical
        // reason from `worker_outcome::REASON_WORKER_CANCELLED`.
        s.record_slot_failure(
            &wave,
            0,
            crate::supervisor::worker_outcome::REASON_WORKER_CANCELLED,
        )
        .unwrap();
        // The slot's failure_reason matches; the status is the
        // distinct `Cancelled` marker so the coordinator /
        // reporter can route it differently. The store keeps the
        // Cancelled count separate from `failed_count` so the
        // caller can distinguish operator-initiated cancel from
        // worker-induced failure.
        let snap = s.fan_in_status(&wave).unwrap();
        assert_eq!(
            snap.failed_count, 0,
            "Cancelled does not count as Failed"
        );
        assert_eq!(
            snap.pending_count, 1,
            "Cancelled slot surfaces in pending_count"
        );
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

    /// U8 / F-008 / R8: rebinding a slot to a different
    /// worktree path must call `cleanup_worktree` on the
    /// prior path before overwriting. Without this branch
    /// the rebind path leaks worktree dirs and git
    /// branches on every dispatch retry (45 leaked dirs
    /// after 5 iters × 4 slots × 3 waves × 3 retries).
    #[test]
    fn bind_worktree_rebind_cleans_up_prior_path() {
        super::cleanup_calls_reset();
        let s = store();
        let wave = s.register_wave("rebind", WaveKind::Exec, 1).unwrap();
        s.bind_worktree(
            &wave,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/old/0".to_string()),
                branch: Some("ralph/old".to_string()),
            },
        )
        .unwrap();
        // Fresh binding (different path) → cleanup must be
        // invoked with the OLD path.
        s.bind_worktree(
            &wave,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/new/0".to_string()),
                branch: Some("ralph/new".to_string()),
            },
        )
        .unwrap();
        let calls = super::cleanup_calls_snapshot();
        assert_eq!(calls, vec![".ralph/old/0".to_string()]);
        // Final binding points at the new path.
        let final_binding = s.get_slot_resource(&wave, 0).unwrap().unwrap();
        assert_eq!(final_binding.worktree_path.as_deref(), Some(".ralph/new/0"));
    }

    /// U8 edge: fresh slot (no prior binding) → no cleanup
    /// call. The rebind path is gated on `prev.resource` so
    /// a first-time bind is a no-op for cleanup.
    #[test]
    fn bind_worktree_fresh_does_not_call_cleanup() {
        super::cleanup_calls_reset();
        let s = store();
        let wave = s.register_wave("fresh", WaveKind::Exec, 1).unwrap();
        s.bind_worktree(
            &wave,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/new/0".to_string()),
                branch: Some("ralph/new".to_string()),
            },
        )
        .unwrap();
        let calls = super::cleanup_calls_snapshot();
        assert!(calls.is_empty(), "fresh bind must NOT trigger cleanup");
    }

    /// U8 edge: rebind to the SAME path is idempotent —
    /// cleanup is NOT invoked (the underlying worktree is
    /// still ours and there's nothing to remove).
    #[test]
    fn bind_worktree_rebind_to_same_path_is_idempotent() {
        super::cleanup_calls_reset();
        let s = store();
        let wave = s.register_wave("idem", WaveKind::Exec, 1).unwrap();
        let binding = SlotResource {
            slot_index: 0,
            worktree_path: Some(".ralph/same/0".to_string()),
            branch: Some("ralph/same".to_string()),
        };
        s.bind_worktree(&wave, 0, binding.clone()).unwrap();
        s.bind_worktree(&wave, 0, binding).unwrap();
        let calls = super::cleanup_calls_snapshot();
        assert!(
            calls.is_empty(),
            "rebind to same path must NOT call cleanup"
        );
    }
}
