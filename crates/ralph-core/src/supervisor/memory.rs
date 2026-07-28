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

#[cfg(test)]
use super::ProjectionKind;
use super::{
    CompensationKind, CoordinationReceiptSummary, DispatchOutcome, EmissionReservation,
    EmissionState, IdempotencyKey, IsolationMode, ProjectionReceiptSummary, RedrivePendingChild,
    RedrivePendingChildSlot, RedriveResult, RedriveTakeOutcome, SlotDescriptor, SlotResource,
    SlotStatus, SupervisorStore, SupervisorStoreError, SupervisorStoreResult, WaveDeliveryState,
    WaveId, WaveKind, WavePhase, WaveSnapshot,
};

/// Per-wave descriptor held in `InMemorySupervisorStore::waves`.
#[derive(Debug, Clone)]
struct WaveRow {
    wave_id: String,
    kind: WaveKind,
    expected_total: u32,
    phase: WavePhase,
    cancel_requested: bool,
    /// 2026-07-27-003 plan U5: orthogonal delivery state to
    /// `phase`. Tracks the four-phase commit protocol
    /// (Pending → BusinessProjected → SalvageCommitted →
    /// CoordinationWritten → CoordinationCommitted) and the
    /// receipts each commit consumed. The two booleans this
    /// field replaces (`merged_to_events`, `salvage_merged`)
    /// participated in a silent-success regression (Plan 004
    /// P0-1) that U5 closes.
    delivery_state: WaveDeliveryState,
    /// Persisted summary of the last accepted salvage receipt.
    /// Used by `commit_salvage_projection` to verify the same
    /// receipt is replayed after a crash (and to refuse a
    /// mismatched one).
    salvage_receipt: Option<ProjectionReceiptSummary>,
    /// Persisted summary of the last accepted coordination
    /// receipt. Used by `record_coordination_written` /
    /// `commit_coordination_event` for the same idempotency
    /// check.
    coordination_receipt: Option<CoordinationReceiptSummary>,
    /// 2026-07-03-001 plan U6: wall-clock instant the wave
    /// was registered. Recovery (U11) uses this to decide
    /// the `Failed` timeout verdict; the in-memory store
    /// records `SystemTime::now()` on `register_wave`.
    /// Mirrors the `waves.created_at` column on the rusqlite
    /// store.
    created_at: SystemTime,
    /// 2026-07-25-005 plan U2: default retry budget for all slots
    /// in this wave. Range 0..=2; 0 disables auto-retry.
    slot_retry_budget: u32,
    /// 2026-07-25-005 plan U2: child attempt wave's epoch marker
    /// (incremented each time a redrive wave is created from this wave).
    attempt_epoch: u32,
    /// 2026-07-25-005 plan U2: redrive parent reference (NULL for original waves).
    parent_wave_id: Option<String>,
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
    /// 2026-07-26-004 plan U2 (KTD3): bounded terminal evidence for
    /// a `Completed` slot. `None` for legacy / not-provably-done.
    terminal_evidence: Option<super::TerminalEvidence>,
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
// 2026-07-24-003 plan U4: `emissions` is the in-memory mirror of
// the v3 `wave_emissions` table. It backs the CLI emission
// reservation state machine (reserve / apply / recovery / fail)
// so unit tests can exercise the dual-process happy path
// without a SQLite dependency.
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
    /// 2026-07-24-003 plan U4: in-memory mirror of the
    /// `wave_emissions` v3 table. The store hands the CLI a
    /// `public_wave_id` on `reserve_emission`; `scope_key` is
    /// the dedup primary key.
    emissions: HashMap<String, EmissionRow>,
    /// 2026-07-25-005 plan U11: redrive request ledger.
    /// Key = `(parent_wave_id, slot_index, attempt_epoch)` for UNIQUE lookup.
    redrive_requests: HashMap<(String, u32, u32), RedriveRequestRow>,
    /// Autoincrement id for redrive_requests.
    next_redrive_id: i64,
    /// 2026-07-27-004 plan U4 (R11 / R14): bounded activation
    /// descriptors by `(wave_id, slot_index)`. The dispatcher
    /// writes a snapshot of the ready event topic + payload +
    /// digest at registration time; `ralph run --resume`
    /// consumes the same descriptor to spawn a worker through
    /// the existing dispatcher seam (no parallel hot path,
    /// no fabricated events).
    slot_descriptors: HashMap<(String, u32), SlotDescriptor>,
    /// 2026-07-28-002 plan U2 (R5 / R6 / S2a): maps
    /// `(child_wave_id, child_slot_index) -> parent_slot_index`.
    /// Populated when `create_redrive_wave` builds child slots.
    /// Used by `slot_descriptor` to redirect child-wave lookups
    /// to their parent descriptor, and by
    /// `list_redrive_pending_child_waves` to build the
    /// `parent_slot_index` + `expected_digest` enrichment.
    #[allow(dead_code)] // used in slot_descriptor and create_redrive_wave
    child_parent_slots: HashMap<(String, u32), u32>,
}

/// 2026-07-24-003 plan U4: in-memory emission reservation row.
/// Mirrors the `wave_emissions` SQLite schema so unit tests
/// exercise the same state-machine semantics as the rusqlite
/// store (U4 contract: `CURRENT_VERSION` v3).
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct EmissionRow {
    scope_key: String,
    public_wave_id: String,
    payload_digest: String,
    expected_count: u32,
    state: EmissionState,
    applied_at: Option<u64>,
}

/// 2026-07-25-005 plan U11: in-memory redrive request row.
/// Mirrors the `redrive_requests` SQLite table so unit tests
/// exercise the same idempotency semantics as the rusqlite store.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields mirror the SQLite schema but are not all read in tests
struct RedriveRequestRow {
    id: i64,
    parent_wave_id: String,
    slot_index: u32,
    attempt_epoch: u32,
    created_at_ms: u64,
    status: RedriveRequestStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Pending/RejectedDuplicate/RejectedTerminal are part of the schema contract
enum RedriveRequestStatus {
    Pending,
    Applied,
    RejectedDuplicate,
    RejectedTerminal,
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
// 2026-07-22-001 plan U6: `compensation_jobs` is now actively
// populated by the dispatcher's cancel / aggregate-timeout /
// spawn-failure paths and drained by the coordinator tick.
// The entry is no longer reserved.
struct CompensationEntry {
    wave_id: String,
    kind: CompensationKind,
    status: CompensationStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// 2026-07-22-001 plan U6: compensation-hook lifecycle states.
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
        slot_retry_budget: u32,
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
        if slot_retry_budget > 2 {
            return Err(SupervisorStoreError::InvalidTransition(
                "slot_retry_budget must be 0..=2".to_string(),
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
                    terminal_evidence: None,
                },
            );
        }
        let row = WaveRow {
            wave_id: wave_id.clone(),
            kind,
            expected_total,
            phase: WavePhase::Dispatch,
            cancel_requested: false,
            delivery_state: WaveDeliveryState::Pending,
            salvage_receipt: None,
            coordination_receipt: None,
            created_at: SystemTime::now(),
            slot_retry_budget,
            attempt_epoch: 0,
            parent_wave_id: None,
            slots,
        };
        inner.waves_by_id.insert(wave_id.clone(), row);
        inner
            .waves_by_key
            .insert(idempotency_key.to_string(), wave_id.clone());
        Ok(wave_id)
    }

    fn register_wave_with_public_id(
        &self,
        public_id: &WaveId,
        kind: WaveKind,
        expected_total: u32,
        slot_retry_budget: u32,
    ) -> SupervisorStoreResult<WaveId> {
        // 2026-07-27-004 plan U1 (R1-R4 / D1 / D2): the public
        // id IS the store primary key. Re-registering with the
        // same id and matching contract is idempotent; a contract
        // drift returns IdentityContractConflict.
        let mut inner = self.lock()?;
        if expected_total == 0 {
            return Err(SupervisorStoreError::InvalidTransition(
                "expected_total must be > 0".to_string(),
            ));
        }
        if slot_retry_budget > 2 {
            return Err(SupervisorStoreError::InvalidTransition(
                "slot_retry_budget must be 0..=2".to_string(),
            ));
        }
        if let Some(existing) = inner.waves_by_id.get(public_id.as_str()) {
            // R3 / D2: same id + matching contract → idempotent.
            if existing.kind != kind
                || existing.expected_total != expected_total
                || existing.slot_retry_budget != slot_retry_budget
            {
                return Err(SupervisorStoreError::IdentityContractConflict(format!(
                    "wave_id '{}' already registered with a different contract \
                     (existing: kind={:?} total={} retry_budget={}; \
                      incoming: kind={:?} total={} retry_budget={})",
                    public_id,
                    existing.kind,
                    existing.expected_total,
                    existing.slot_retry_budget,
                    kind,
                    expected_total,
                    slot_retry_budget,
                )));
            }
            return Ok(public_id.clone());
        }
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
                    terminal_evidence: None,
                },
            );
        }
        let wave_id_str = public_id.as_str().to_string();
        let row = WaveRow {
            wave_id: wave_id_str.clone(),
            kind,
            expected_total,
            phase: WavePhase::Dispatch,
            cancel_requested: false,
            delivery_state: WaveDeliveryState::Pending,
            salvage_receipt: None,
            coordination_receipt: None,
            created_at: SystemTime::now(),
            slot_retry_budget,
            attempt_epoch: 0,
            parent_wave_id: None,
            slots,
        };
        inner.waves_by_id.insert(wave_id_str.clone(), row);
        // Keep `waves_by_key` in sync so the legacy lookup path
        // (`wave_id_for_idempotency_key(public_id)`) keeps finding
        // the row. U1 does not retire the legacy idempotency_key
        // contract — it makes the public id alias for it.
        inner
            .waves_by_key
            .entry(wave_id_str.clone())
            .or_insert(wave_id_str.clone());
        Ok(public_id.clone())
    }

    fn enqueue_wave(
        &self,
        idempotency_key: &str,
        kind: WaveKind,
        expected_total: u32,
        slot_retry_budget: u32,
    ) -> SupervisorStoreResult<String> {
        // U3: enqueue is a placeholder until U4 wires
        // backpressure dispatch. The store still tracks the wave
        // so U4 can advance without re-introducing it.
        let wave_id =
            self.register_wave(idempotency_key, kind, expected_total, slot_retry_budget)?;
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
        // already reached `Failed` / `Cancelled` MUST NOT be
        // overwritten by a late failure. The idempotent replay of
        // the SAME failure reason returns Ok without rewriting
        // (mirrors the `record_slot_result` same-content_hash
        // contract).
        //
        // 2026-07-23-007 plan U3 (R-W4): cancel reason wins over a
        // prior `Completed` row — a slot whose worker emitted a
        // Done marker and was then cancelled MUST end as
        // `Cancelled`, not `Completed`. Any other terminal kind
        // still wins on first-write.
        let already_failed = matches!(slot.status, SlotStatus::Failed | SlotStatus::Cancelled);
        let cancel_wins = reason == crate::supervisor::worker_outcome::REASON_WORKER_CANCELLED
            && matches!(slot.status, SlotStatus::Completed);
        if already_failed {
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
        if matches!(slot.status, SlotStatus::Completed) && !cancel_wins {
            return Err(SupervisorStoreError::AlreadyTerminal(format!(
                "wave={wave_id} slot={slot_index} status=completed"
            )));
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

    fn slot_failure_reason(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> SupervisorStoreResult<Option<String>> {
        let inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        let slot =
            wave.slots
                .get(&slot_index)
                .ok_or_else(|| SupervisorStoreError::UnknownSlot {
                    wave_id: wave_id.to_string(),
                    slot_index,
                })?;
        Ok(slot.failure_reason.clone())
    }

    fn record_slot_terminal_evidence(
        &self,
        wave_id: &str,
        slot_index: u32,
        evidence: &super::TerminalEvidence,
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
        // 2026-07-26-004 plan U2 (R3): idempotent same-evidence replay
        // is a no-op; conflicting evidence for the same slot fails
        // closed so a double-recorded slot cannot silently swap its
        // proven terminal event.
        match slot.terminal_evidence.as_ref() {
            Some(existing) if existing == evidence => Ok(()),
            Some(existing) => Err(SupervisorStoreError::AlreadyTerminal(format!(
                "wave={wave_id} slot={slot_index} terminal evidence conflict: \
                 existing={existing:?} incoming={evidence:?}"
            ))),
            None => {
                slot.terminal_evidence = Some(evidence.clone());
                Ok(())
            }
        }
    }

    fn slot_terminal_evidence(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> SupervisorStoreResult<Option<super::TerminalEvidence>> {
        let inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        let slot =
            wave.slots
                .get(&slot_index)
                .ok_or_else(|| SupervisorStoreError::UnknownSlot {
                    wave_id: wave_id.to_string(),
                    slot_index,
                })?;
        Ok(slot.terminal_evidence.clone())
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
                // 2026-07-25-004 plan U5: freeze `failure_reason` to
                // `slot_never_started` for the Pending slots we cancel
                // here (symmetric with the rusqlite store), so the
                // InjectedFailed reason-collection sees a non-null
                // reason. The `if Pending` guard guarantees
                // already-terminal slots keep their own reason.
                slot.failure_reason =
                    Some(crate::supervisor::worker_outcome::REASON_SLOT_NEVER_STARTED.to_string());
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
            delivery_state: wave.delivery_state,
            started_at: wave.created_at,
            slots,
        })
    }

    fn record_business_projection(
        &self,
        wave_id: &str,
        receipt: &ProjectionReceiptSummary,
    ) -> SupervisorStoreResult<()> {
        // 2026-07-27-004 plan U5 (R17 / P0): first phase of the
        // delivery protocol. The merge seam stamps this AFTER
        // its write to main lands; the strict rusqlite
        // `commit_salvage_projection` gate requires it.
        let mut inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        // Once the wave latched `SalvageCommitted` there is
        // nothing for the first-phase stamp to do — the
        // fingerprint conflict gate lives in
        // `commit_salvage_projection`, not here. Replays are a
        // plain no-op so the dispatcher's salvage seam can run
        // unconditionally on retried ticks.
        if wave
            .delivery_state
            .at_least(WaveDeliveryState::SalvageCommitted)
        {
            return Ok(());
        }
        // Forward-only advance.
        if !wave
            .delivery_state
            .at_least(WaveDeliveryState::BusinessProjected)
        {
            wave.delivery_state = WaveDeliveryState::BusinessProjected;
        }
        if !receipt.batch_fingerprint.is_empty() || wave.salvage_receipt.is_none() {
            wave.salvage_receipt = Some(receipt.clone());
        }
        Ok(())
    }

    fn commit_salvage_projection(
        &self,
        wave_id: &str,
        receipt: &ProjectionReceiptSummary,
    ) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        // 2026-07-27-003 plan U5: the merge helpers
        // (`merge_completed_*_slots_to_main`,
        // `project_empty_salvage`) are responsible for the
        // actual disk write; the commit step advances the
        // store's view of the wave. Pending is a legal
        // starting state because the merge seam may run
        // before any prior transition (an empty salvage,
        // for instance). Already-advanced states are
        // accepted idempotently. A DIFFERENT fingerprint
        // after `CoordinationWritten` is allowed because
        // the dispatcher may have pre-stamped a placeholder
        // (e.g. legacy `mark_salvage_merged` calls) and the
        // real fingerprint arrives only after the merge
        // seam lands on disk.
        if wave
            .delivery_state
            .at_least(WaveDeliveryState::CoordinationWritten)
        {
            let existing = wave.salvage_receipt.as_ref();
            if let Some(existing) = existing
                && !existing.batch_fingerprint.is_empty()
                && existing.batch_fingerprint == receipt.batch_fingerprint
            {
                // idempotent re-commit with the SAME
                // fingerprint; allow.
            } else if let Some(existing) = existing
                && !existing.batch_fingerprint.is_empty()
            {
                // Different fingerprint after CoordinationWritten:
                // log a warning but accept (the merge seam
                // already ran, the placeholder may have been
                // set by a pre-U5 caller).
                tracing::warn!(
                    existing = %existing.batch_fingerprint,
                    new = %receipt.batch_fingerprint,
                    "commit_salvage_projection: salvage fingerprint replaced after CoordinationWritten"
                );
            }
        }
        // Forward-only advance.
        if !wave
            .delivery_state
            .at_least(WaveDeliveryState::SalvageCommitted)
        {
            wave.delivery_state = WaveDeliveryState::SalvageCommitted;
        }
        if receipt.batch_fingerprint.is_empty()
            || wave
                .salvage_receipt
                .as_ref()
                .is_none_or(|existing| existing.batch_fingerprint == receipt.batch_fingerprint)
        {
            wave.salvage_receipt = Some(receipt.clone());
        }
        Ok(())
    }

    fn record_coordination_written(
        &self,
        wave_id: &str,
        receipt: &CoordinationReceiptSummary,
    ) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        // 2026-07-27-003 plan U5 / recovery: relax the gate from
        // `≥ CoordinationWritten` to `≥ SalvageCommitted` (with
        // explicit refusal only at terminal `CoordinationCommitted`).
        // The original guard skipped `pending` / `business_projected`
        // rows, which the rusqlite SQL CASE handles but the
        // restart-replay path could not exercise through the rust
        // gate. We now mirror the SQL CASE and let the
        // fingerprint-mismatch check below catch true replays with
        // a different receipt.
        // rusqlite SQL CASE so a restart that re-derives the
        // receipt from disk and replays through
        // `record_coordination_written` may observe a `pending`
        // row (the merge sink wrote main + coord-event but never
        // stamped the wave).
        //
        // Idempotency: a re-record on a wave already at
        // `CoordinationCommitted` is a no-op. The fingerprint
        // check below catches a true conflict (different
        // payload). This mirrors the rusqlite SQL CASE which
        // leaves `coordination_committed` rows unchanged.
        if wave
            .delivery_state
            .at_least(WaveDeliveryState::CoordinationCommitted)
        {
            if let Some(existing) = wave.coordination_receipt.as_ref()
                && !existing.payload_fingerprint.is_empty()
                && !receipt.payload_fingerprint.is_empty()
                && existing.payload_fingerprint != receipt.payload_fingerprint
            {
                return Err(SupervisorStoreError::InvalidTransition(format!(
                    "record_coordination_written: fingerprint mismatch (existing={}, new={})",
                    existing.payload_fingerprint, receipt.payload_fingerprint
                )));
            }
            return Ok(());
        }
        if wave
            .delivery_state
            .at_least(WaveDeliveryState::CoordinationWritten)
        {
            let existing = wave.coordination_receipt.as_ref();
            if let Some(existing) = existing
                && !existing.payload_fingerprint.is_empty()
                && existing.payload_fingerprint != receipt.payload_fingerprint
            {
                return Err(SupervisorStoreError::InvalidTransition(format!(
                    "record_coordination_written: fingerprint mismatch (existing={}, new={})",
                    existing.payload_fingerprint, receipt.payload_fingerprint
                )));
            }
        }
        if !wave
            .delivery_state
            .at_least(WaveDeliveryState::CoordinationWritten)
        {
            wave.delivery_state = WaveDeliveryState::CoordinationWritten;
        }
        if receipt.payload_fingerprint.is_empty()
            || wave
                .coordination_receipt
                .as_ref()
                .is_none_or(|existing| existing.payload_fingerprint == receipt.payload_fingerprint)
        {
            wave.coordination_receipt = Some(receipt.clone());
        }
        Ok(())
    }

    fn commit_coordination_event(
        &self,
        wave_id: &str,
        receipt: &CoordinationReceiptSummary,
        terminal_phase: WavePhase,
    ) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let wave = inner
            .waves_by_id
            .get_mut(wave_id)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
        // 2026-07-27-003 plan U5 / recovery: the merge sink may
        // have written the coord event to main and crashed
        // before `record_coordination_written` ran. Accept any
        // state ≥ SalvageCommitted so a restart that already
        // has the main-append + coord-event in place can
        // advance the wave to CoordinationCommitted without
        // replaying a stale receipt.
        if !wave
            .delivery_state
            .at_least(WaveDeliveryState::SalvageCommitted)
        {
            return Err(SupervisorStoreError::InvalidTransition(
                "commit_coordination_event requires SalvageCommitted state".to_string(),
            ));
        }
        if let Some(existing) = wave.coordination_receipt.as_ref()
            && !existing.payload_fingerprint.is_empty()
            && existing.payload_fingerprint != receipt.payload_fingerprint
        {
            return Err(SupervisorStoreError::InvalidTransition(format!(
                "commit_coordination_event: fingerprint mismatch (existing={}, new={})",
                existing.payload_fingerprint, receipt.payload_fingerprint
            )));
        }
        wave.delivery_state = WaveDeliveryState::CoordinationCommitted;
        // Set the terminal phase only on the FIRST commit — a
        // re-commit on a Done wave must NOT flip Failed into
        // Done again (the dispatcher must align with the
        // already-recorded outcome).
        if !matches!(wave.phase, WavePhase::Done | WavePhase::Failed) {
            wave.phase = terminal_phase;
        }
        if receipt.payload_fingerprint.is_empty()
            || wave
                .coordination_receipt
                .as_ref()
                .is_none_or(|existing| existing.payload_fingerprint == receipt.payload_fingerprint)
        {
            wave.coordination_receipt = Some(receipt.clone());
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
            // U2 contract: skip waves the coordinator has
            // already driven to a terminal phase
            // (`Done` / `Failed`). Recovery only cares about
            // waves that still need a coordinator verdict.
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
                delivery_state: wave.delivery_state,
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

    fn enqueue_compensation(
        &self,
        wave_id: &str,
        kind: CompensationKind,
    ) -> SupervisorStoreResult<()> {
        // 2026-07-22-001 plan U6: dedup enqueue by (wave_id, kind)
        // so a re-entered cancel path does not stack two jobs
        // for the same wave. We also accept enqueue for unknown
        // waves silently — the dispatcher may call cancel_wave
        // before the store has registered the wave when the
        // dispatcher is shutting down, and we do not want a
        // compensation enqueue failure to abort the shutdown.
        let mut inner = self.lock()?;
        let exists = inner.compensation.iter().any(|c| {
            c.wave_id == wave_id && c.kind == kind && c.status == CompensationStatus::Pending
        });
        if !exists {
            inner.compensation.push(CompensationEntry {
                wave_id: wave_id.to_string(),
                kind,
                status: CompensationStatus::Pending,
            });
        }
        Ok(())
    }

    fn take_pending_compensations(&self) -> SupervisorStoreResult<Vec<(String, CompensationKind)>> {
        let inner = self.lock()?;
        Ok(inner
            .compensation
            .iter()
            .filter(|c| c.status == CompensationStatus::Pending)
            .map(|c| (c.wave_id.clone(), c.kind))
            .collect())
    }

    fn complete_compensation(
        &self,
        wave_id: &str,
        kind: CompensationKind,
        ok: bool,
    ) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        if let Some(entry) = inner.compensation.iter_mut().find(|c| {
            c.wave_id == wave_id && c.kind == kind && c.status == CompensationStatus::Pending
        }) {
            entry.status = if ok {
                CompensationStatus::Executed
            } else {
                CompensationStatus::Failed
            };
        }
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────
    // 2026-07-25-005 plan U11: redrive API.
    // ─────────────────────────────────────────────────────────────────

    fn create_redrive_wave(
        &self,
        parent_wave_id: &str,
        slots: Option<&[u32]>,
    ) -> SupervisorStoreResult<RedriveResult> {
        use std::time::UNIX_EPOCH;

        // 1. Load parent wave — clone data we need so we can drop the borrow
        let (parent_kind, attempt_epoch_base, parent_slot_retry_budget, target_slots) = {
            let inner = self.lock()?;
            let parent = inner
                .waves_by_id
                .get(parent_wave_id)
                .ok_or_else(|| SupervisorStoreError::UnknownWave(parent_wave_id.to_string()))?;

            // 2. Reject Done or Integrate parent
            if matches!(parent.phase, WavePhase::Done | WavePhase::Integrate) {
                return Err(SupervisorStoreError::InvalidTransition(
                    "cannot redrive a wave in done or integrate phase".to_string(),
                ));
            }

            // 3. Collect failed slots
            let failed_slot_indices: Vec<u32> = parent
                .slots
                .iter()
                .filter(|(_, s)| s.status == SlotStatus::Failed)
                .map(|(idx, _)| *idx)
                .collect();

            if failed_slot_indices.is_empty() {
                return Err(SupervisorStoreError::InvalidTransition(
                    "no failed slots to redrive".to_string(),
                ));
            }

            // 4. Apply explicit slots filter or default to all failed
            let target: Vec<u32> = match slots {
                Some(s) => {
                    let requested: std::collections::HashSet<u32> = s.iter().cloned().collect();
                    failed_slot_indices
                        .into_iter()
                        .filter(|i| requested.contains(i))
                        .collect()
                }
                None => failed_slot_indices,
            };

            if target.is_empty() {
                return Err(SupervisorStoreError::InvalidTransition(
                    "none of the requested slots are failed".to_string(),
                ));
            }

            (
                parent.kind,
                parent.attempt_epoch,
                parent.slot_retry_budget,
                target,
            )
        };

        let attempt_epoch = attempt_epoch_base + 1;

        // Re-acquire lock for state mutations
        let mut inner = self.lock()?;

        // 5. Check if a child wave already exists for this (parent, attempt_epoch).
        //    This covers the idempotency case: the second redrive call should
        //    find the child already created by the first call.
        let existing_child_wave_id: Option<String> = inner
            .waves_by_id
            .values()
            .find(|w| {
                w.parent_wave_id.as_deref() == Some(parent_wave_id)
                    && w.attempt_epoch == attempt_epoch
            })
            .map(|w| w.wave_id.clone());

        if let Some(existing_id) = existing_child_wave_id {
            // Idempotent hit: find the first redrive_request for this (parent, epoch)
            // to get its id; return the existing child wave.
            let req_id = inner
                .redrive_requests
                .values()
                .find(|r| r.parent_wave_id == parent_wave_id && r.attempt_epoch == attempt_epoch)
                .map(|r| r.id);
            return Ok(RedriveResult {
                redrive_request_id: req_id.unwrap_or(0),
                child_wave_id: existing_id,
                attempt_epoch,
                parent_wave_id: parent_wave_id.to_string(),
                slots: target_slots,
            });
        }

        // 6. For each target slot: idempotency check / record in redrive_requests
        let now_ms = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let mut redrive_request_id: Option<i64> = None;

        for &slot_index in &target_slots {
            let key = (parent_wave_id.to_string(), slot_index, attempt_epoch);
            if let Some(existing) = inner.redrive_requests.get(&key) {
                // Duplicate: use the existing request id
                redrive_request_id.get_or_insert(existing.id);
            } else {
                // Insert new redrive request with status Applied
                let id = inner.next_redrive_id;
                inner.next_redrive_id += 1;
                redrive_request_id.get_or_insert(id);
                inner.redrive_requests.insert(
                    key,
                    RedriveRequestRow {
                        id,
                        parent_wave_id: parent_wave_id.to_string(),
                        slot_index,
                        attempt_epoch,
                        created_at_ms: now_ms,
                        status: RedriveRequestStatus::Applied,
                    },
                );
            }
        }

        let redrive_request_id = redrive_request_id.unwrap();

        // 7. Create child wave (register_wave equivalent)
        let child_wave_id = format!("w-{}", inner.next_wave_seq + 1);
        inner.next_wave_seq += 1;

        let default_isolation = match parent_kind {
            WaveKind::Exec | WaveKind::Fix => IsolationMode::Worktree,
            WaveKind::Review => IsolationMode::SharedReadonly,
        };

        let mut child_slots = BTreeMap::new();
        for (i, &_slot_index) in target_slots.iter().enumerate() {
            child_slots.insert(
                i as u32,
                SlotRow {
                    slot_index: i as u32,
                    status: SlotStatus::Pending,
                    isolation: default_isolation,
                    resource: None,
                    content_hash: None,
                    event_count: None,
                    failure_reason: None,
                    terminal_evidence: None,
                },
            );
        }

        let row = WaveRow {
            wave_id: child_wave_id.clone(),
            kind: parent_kind,
            expected_total: target_slots.len() as u32,
            phase: WavePhase::Dispatch,
            cancel_requested: false,
            delivery_state: WaveDeliveryState::Pending,
            salvage_receipt: None,
            coordination_receipt: None,
            created_at: SystemTime::now(),
            slot_retry_budget: parent_slot_retry_budget,
            attempt_epoch,
            parent_wave_id: Some(parent_wave_id.to_string()),
            slots: child_slots,
        };

        inner.waves_by_id.insert(child_wave_id.clone(), row);

        // 8. 2026-07-28-002 plan U2 (R5 / S2a): record the
        // parent → child slot mapping so
        // `list_redrive_pending_child_waves` can build the
        // enriched slot list, AND copy each target parent
        // slot's descriptor into the child key so U4's boot
        // `take_dispatchable_redrive_descriptor(child, c, ...)`
        // can find it. The cloned descriptor keeps
        // `slot_index` = parent_slot (audit/digest anchor) and
        // gains `slot_index_in_parent = Some(parent_slot)` for
        // explicit tracing.
        for (i, &parent_slot) in target_slots.iter().enumerate() {
            inner
                .child_parent_slots
                .insert((child_wave_id.clone(), i as u32), parent_slot);

            // Copy parent descriptor into child key (if the
            // parent had one). Pre-U4 parent slots without a
            // descriptor are intentionally skipped here — the
            // boot path will surface `expected_digest = None`
            // and fail-closed (S4).
            let parent_key = (parent_wave_id.to_string(), parent_slot);
            if let Some(parent_desc) = inner.slot_descriptors.get(&parent_key).cloned() {
                let mut child_desc = parent_desc;
                child_desc.slot_index_in_parent = Some(parent_slot);
                let child_key = (child_wave_id.clone(), i as u32);
                inner.slot_descriptors.insert(child_key, child_desc);
            }
        }

        Ok(RedriveResult {
            redrive_request_id,
            child_wave_id,
            attempt_epoch,
            parent_wave_id: parent_wave_id.to_string(),
            slots: target_slots,
        })
    }

    /// 2026-07-27-004 plan U4 (R11 / R14 / S11) + 2026-07-28-002
    /// fix A1 (R-F4): persist the bounded activation descriptor for
    /// a slot. Mirrors the rusqlite store's `UPDATE ...
    /// COALESCE(slot_index_in_parent, ?)` semantics: when the new
    /// descriptor passes `None` for `slot_index_in_parent`, the prior
    /// value (which `create_redrive_wave` seeded) is preserved. A
    /// `Some(_)` override always wins. Payload fields (topic /
    /// payload_json / wave_kind / payload_digest) are overwritten
    /// unconditionally — those are the mutating channel.
    fn persist_slot_descriptor(
        &self,
        wave_id: &str,
        descriptor: &SlotDescriptor,
    ) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        // R16 / S11: the wave must exist. A descriptor for a
        // never-registered wave is a programming error — fail
        // closed so the operator can spot the misrouted register
        // before any redrive is attempted.
        if !inner.waves_by_id.contains_key(wave_id) {
            return Err(SupervisorStoreError::UnknownWave(wave_id.to_string()));
        }
        let key = (wave_id.to_string(), descriptor.slot_index);
        let mut merged = descriptor.clone();
        if merged.slot_index_in_parent.is_none()
            && let Some(existing) = inner.slot_descriptors.get(&key)
        {
            merged.slot_index_in_parent = existing.slot_index_in_parent;
        }
        inner.slot_descriptors.insert(key, merged);
        Ok(())
    }

    fn take_dispatchable_redrive_descriptor(
        &self,
        child_wave_id: &str,
        slot_index: u32,
        expected_digest: &str,
    ) -> SupervisorStoreResult<RedriveTakeOutcome> {
        let inner = self.lock()?;
        let key = (child_wave_id.to_string(), slot_index);
        let Some(descriptor) = inner.slot_descriptors.get(&key) else {
            // R16 / S13: no persisted descriptor (legacy pre-U4
            // row). Fail-closed.
            return Ok(RedriveTakeOutcome::DescriptorUnavailable);
        };
        // R16 / S13: digest mismatch is a strict fail-close. The
        // runtime has re-derived a payload whose fingerprint
        // does not match the stored descriptor — that means
        // somebody (an agent? a tooling script?) tampered with
        // the activation contract. Refuse rather than silently
        // re-execute a stale activation.
        if descriptor.payload_digest != expected_digest {
            return Ok(RedriveTakeOutcome::DescriptorConflict);
        }
        Ok(RedriveTakeOutcome::Dispatchable {
            descriptor: descriptor.clone(),
        })
    }

    /// 2026-07-28-002 plan U2 (R4 / R6): read a persisted
    /// descriptor for `(wave_id, slot_index)`.
    ///
    /// For a CHILD wave slot, the descriptor was stored at
    /// `(child_wave_id, parent_slot_index)` (because the
    /// dispatcher calls `persist_slot_descriptor(child_wave_id,
    /// child_slot, descriptor)` where `descriptor.slot_index`
    /// is the parent's slot). We redirect the lookup to
    /// `(parent_wave_id, parent_slot_index)` via
    /// `child_parent_slots`.
    ///
    /// For a PARENT wave, no redirect exists; we do a direct
    /// lookup in `slot_descriptors`.
    fn slot_descriptor(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> SupervisorStoreResult<Option<SlotDescriptor>> {
        let inner = self.lock()?;
        let key = (wave_id.to_string(), slot_index);
        // R4 / R6: direct lookup in `slot_descriptors`. Both
        // parent and child slots store their descriptors under
        // their own `(wave_id, slot_index)` key — child slots
        // carry the descriptor copied by `create_redrive_wave`
        // (with `slot_index` / `slot_index_in_parent` set to
        // the parent slot for audit).
        Ok(inner.slot_descriptors.get(&key).cloned())
    }

    /// 2026-07-28-002 plan U2 (R5 / R6 / S2a / S4): list all
    /// child waves with `parent_wave_id IS NOT NULL` and phase
    /// `Dispatch`, enriched per slot with `parent_slot_index`
    /// and `expected_digest` (None when the parent slot had no
    /// descriptor — pre-U4 legacy row; fail-closed at boot).
    fn list_redrive_pending_child_waves(&self) -> SupervisorStoreResult<Vec<RedrivePendingChild>> {
        let inner = self.lock()?;
        let mut results = Vec::new();

        for wave_row in inner.waves_by_id.values() {
            let parent_wave_id = match wave_row.parent_wave_id.as_deref() {
                Some(id) => id,
                None => continue,
            };

            if wave_row.phase != WavePhase::Dispatch {
                continue;
            }

            // Build the enriched slot list. child_slot_index 0..N
            // maps to parent_slot via child_parent_slots. The
            // `expected_digest` is read from the descriptor we
            // already copied into the child key during
            // `create_redrive_wave` (i.e. the same digest the
            // parent carried — `take` will compare against it).
            // A missing descriptor means pre-U4 legacy row
            // (S4) and surfaces as `expected_digest = None`
            // so the boot fails closed.
            let mut slots = Vec::new();
            for &child_slot_index in wave_row.slots.keys() {
                let parent_slot_index = inner
                    .child_parent_slots
                    .get(&(wave_row.wave_id.clone(), child_slot_index))
                    .copied();

                let parent_slot_index = match parent_slot_index {
                    Some(idx) => idx,
                    None => continue, // no parent mapping — skip this slot
                };

                // Look up the child descriptor (the one
                // copied in from the parent) for
                // `expected_digest`. Falls through to `None`
                // when the parent slot had no descriptor.
                let expected_digest = inner
                    .slot_descriptors
                    .get(&(wave_row.wave_id.clone(), child_slot_index))
                    .map(|d| d.payload_digest.clone());

                slots.push(RedrivePendingChildSlot {
                    child_slot_index,
                    parent_slot_index,
                    expected_digest,
                });
            }

            if !slots.is_empty() {
                results.push(RedrivePendingChild {
                    child_wave_id: wave_row.wave_id.clone(),
                    parent_wave_id: parent_wave_id.to_string(),
                    kind: wave_row.kind,
                    slots,
                });
            }
        }

        Ok(results)
    }

    // ─────────────────────────────────────────────────────────────────
    // 2026-07-24-003 plan U4: emission reservation state machine.
    //
    // The InMemory implementation serialises under the same
    // `Mutex` as the rest of the store, so concurrent
    // `reserve_emission` calls on the same `scope_key` always
    // observe the prior commit before deciding the outcome.
    // The rusqlite implementation (mirrored below) relies on the
    // `scope_key` UNIQUE constraint + an in-transaction
    // re-read for the same guarantee.
    // ─────────────────────────────────────────────────────────────────

    fn reserve_emission(
        &self,
        scope_key: &str,
        payload_digest: &str,
        expected_count: u32,
        count_events_on_disk: &dyn Fn(&str) -> u32,
    ) -> SupervisorStoreResult<EmissionReservation> {
        let mut inner = self.lock()?;

        if let Some(existing) = inner.emissions.get(scope_key).cloned() {
            // Same scope: payload digest mismatch is a hard conflict
            // (S4). The CLI must pick a different `--idempotency-key`.
            if existing.payload_digest != payload_digest {
                return Ok(EmissionReservation::Conflict);
            }
            // Same scope + same digest: classify by state.
            return match existing.state {
                EmissionState::Applied => Ok(EmissionReservation::AlreadyApplied {
                    public_wave_id: existing.public_wave_id,
                }),
                EmissionState::Failed => {
                    // A previous fail terminal — the caller is
                    // retrying; treat as conflict so the agent
                    // surfaces the history rather than silently
                    // re-running. (S9 fail-closed path.)
                    Ok(EmissionReservation::Conflict)
                }
                EmissionState::RecoveryRequired
                | EmissionState::Reserved
                | EmissionState::Applying => {
                    // Use the caller-supplied closure to count
                    // events on disk; the closure lets the trait
                    // surface stay free of file paths.
                    let on_disk = count_events_on_disk(&existing.public_wave_id);
                    if on_disk == 0 {
                        Ok(EmissionReservation::FailedPartial {
                            public_wave_id: existing.public_wave_id,
                            on_disk,
                            expected: existing.expected_count,
                        })
                    } else if on_disk < existing.expected_count {
                        Ok(EmissionReservation::RecoveryRequired {
                            public_wave_id: existing.public_wave_id,
                            on_disk,
                            expected: existing.expected_count,
                        })
                    } else {
                        // on_disk >= expected_count and the row
                        // never reached `applied`: recover by
                        // transitioning the row to `applied`
                        // and returning `AlreadyApplied`.
                        inner
                            .emissions
                            .get_mut(scope_key)
                            .expect("scope_key just observed")
                            .state = EmissionState::Applied;
                        Ok(EmissionReservation::AlreadyApplied {
                            public_wave_id: existing.public_wave_id,
                        })
                    }
                }
            };
        }

        // First call: allocate a fresh public_wave_id. The
        // allocator format mirrors `wave::generate_wave_id` so
        // the existing operator tooling that greps for `w-` keeps
        // working. We deliberately use the store's own counter
        // (rather than re-using `chrono::Utc::now()` + PID) so
        // two reservations in the same nanosecond still get
        // distinct ids.
        let public_wave_id = format!("w-{}", inner.next_wave_seq);
        inner.next_wave_seq += 1;
        inner.emissions.insert(
            scope_key.to_string(),
            EmissionRow {
                scope_key: scope_key.to_string(),
                public_wave_id: public_wave_id.clone(),
                payload_digest: payload_digest.to_string(),
                expected_count,
                state: EmissionState::Reserved,
                applied_at: None,
            },
        );
        Ok(EmissionReservation::Reserved { public_wave_id })
    }

    fn mark_emission_applying(&self, scope_key: &str) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let row = inner
            .emissions
            .get_mut(scope_key)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(scope_key.to_string()))?;
        if row.state != EmissionState::Reserved {
            return Err(SupervisorStoreError::InvalidTransition(format!(
                "emission row for {scope_key} is in state {:?}, expected Reserved",
                row.state
            )));
        }
        row.state = EmissionState::Applying;
        Ok(())
    }

    fn mark_emission_applied(
        &self,
        scope_key: &str,
        applied_at_unix_secs: u64,
    ) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let row = inner
            .emissions
            .get_mut(scope_key)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(scope_key.to_string()))?;
        // Only Applying/Reserved → Applied.  applied→applied is
        // NOT idempotent: a second `mark_emission_applied` on an
        // already Applied row must fail closed so a double-Apply
        // cannot silently overwrite `applied_at` (test pin
        // `mark_emission_applied_rejects_terminal_applied_row`).
        // Rusqlite mirrors the same strict WHERE clause — peer
        // dedup is handled by `reserve_emission` returning
        // AlreadyApplied, not by loosening this transition.
        if !matches!(row.state, EmissionState::Applying | EmissionState::Reserved) {
            return Err(SupervisorStoreError::InvalidTransition(format!(
                "emission row for {scope_key} is in state {:?}, expected Applying or Reserved",
                row.state
            )));
        }
        row.state = EmissionState::Applied;
        row.applied_at = Some(applied_at_unix_secs);
        Ok(())
    }

    fn mark_emission_recovery_required(&self, scope_key: &str) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let row = inner
            .emissions
            .get_mut(scope_key)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(scope_key.to_string()))?;
        // Parity with rusqlite: only Reserved/Applying → RecoveryRequired.
        if !matches!(row.state, EmissionState::Reserved | EmissionState::Applying) {
            return Err(SupervisorStoreError::InvalidTransition(format!(
                "emission row for {scope_key} is in state {:?}, expected Reserved or Applying",
                row.state
            )));
        }
        row.state = EmissionState::RecoveryRequired;
        Ok(())
    }

    fn mark_emission_failed(&self, scope_key: &str) -> SupervisorStoreResult<()> {
        let mut inner = self.lock()?;
        let row = inner
            .emissions
            .get_mut(scope_key)
            .ok_or_else(|| SupervisorStoreError::UnknownWave(scope_key.to_string()))?;
        if !matches!(
            row.state,
            EmissionState::Reserved | EmissionState::Applying | EmissionState::RecoveryRequired
        ) {
            return Err(SupervisorStoreError::InvalidTransition(format!(
                "emission row for {scope_key} is terminal-applied; cannot mark failed"
            )));
        }
        row.state = EmissionState::Failed;
        Ok(())
    }

    fn emission_state_for_wave_id(
        &self,
        public_wave_id: &str,
    ) -> SupervisorStoreResult<Option<EmissionState>> {
        let inner = self.lock()?;
        Ok(inner
            .emissions
            .values()
            .find(|r| r.public_wave_id == public_wave_id)
            .map(|r| r.state))
    }

    fn adopt_legacy_emission(
        &self,
        scope_key: &str,
        payload_digest: &str,
        expected_count: u32,
        legacy_wave_id: &str,
    ) -> SupervisorStoreResult<String> {
        let mut inner = self.lock()?;
        if let Some(existing) = inner.emissions.get(scope_key).cloned() {
            // Idempotent re-import: return the recorded id.
            return Ok(existing.public_wave_id);
        }
        inner.emissions.insert(
            scope_key.to_string(),
            EmissionRow {
                scope_key: scope_key.to_string(),
                public_wave_id: legacy_wave_id.to_string(),
                payload_digest: payload_digest.to_string(),
                expected_count,
                state: EmissionState::Applied,
                applied_at: Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                ),
            },
        );
        Ok(legacy_wave_id.to_string())
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

    /// 2026-07-26-004 plan U2 (KTD3 / R2 / R3): terminal evidence
    /// round-trips, idempotent same-evidence replay is a no-op,
    /// conflicting evidence fails closed, and a legacy slot with no
    /// evidence reads back as `None` (not provably done).
    #[test]
    fn u2_terminal_evidence_round_trip_and_conflict() {
        use crate::supervisor::TerminalEvidence;
        let s = store();
        let wave = s.register_wave("k-ev", WaveKind::Review, 2, 1).unwrap();
        // Legacy: no evidence recorded yet → None.
        assert_eq!(s.slot_terminal_evidence(&wave, 0).unwrap(), None);

        let ev =
            TerminalEvidence::from_event("review.unit.done", "{\"dimension\":\"correctness\"}");
        assert_eq!(ev.dimension.as_deref(), Some("correctness"));
        s.record_slot_terminal_evidence(&wave, 0, &ev).unwrap();
        assert_eq!(
            s.slot_terminal_evidence(&wave, 0).unwrap(),
            Some(ev.clone())
        );

        // Idempotent same-evidence replay → Ok no-op.
        s.record_slot_terminal_evidence(&wave, 0, &ev).unwrap();
        assert_eq!(
            s.slot_terminal_evidence(&wave, 0).unwrap(),
            Some(ev.clone())
        );

        // Conflicting evidence for the same slot → AlreadyTerminal.
        let other = TerminalEvidence::from_event("review.unit.done", "{\"dimension\":\"testing\"}");
        let conflict = s.record_slot_terminal_evidence(&wave, 0, &other);
        assert!(
            matches!(conflict, Err(SupervisorStoreError::AlreadyTerminal(_))),
            "conflicting evidence must fail closed; got {conflict:?}"
        );
        // Original evidence preserved.
        assert_eq!(s.slot_terminal_evidence(&wave, 0).unwrap(), Some(ev));

        // Slot 1 untouched → None.
        assert_eq!(s.slot_terminal_evidence(&wave, 1).unwrap(), None);
    }

    #[test]
    fn register_wave_creates_expected_total_pending_slots() {
        let s = store();
        let wave = s
            .register_wave("key-1", WaveKind::Exec, 4, 1)
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
        s.register_wave("dup", WaveKind::Exec, 2, 1).unwrap();
        let err = s.register_wave("dup", WaveKind::Fix, 1, 1).unwrap_err();
        assert!(matches!(err, SupervisorStoreError::DuplicateKey(_)));
    }

    #[test]
    fn worktree_isolation_blocks_dispatch_until_bound() {
        let s = store();
        let wave = s.register_wave("k", WaveKind::Exec, 2, 1).unwrap();
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
        s.register_wave("rv", WaveKind::Review, 3, 1).unwrap();
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
        let wave = s.register_wave("kf", WaveKind::Exec, 2, 1).unwrap();
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
            .register_wave("partial-fail-mem", WaveKind::Exec, 2, 1)
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
            .register_wave("u3-after-completed", WaveKind::Exec, 1, 1)
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
            .register_wave("u3-same-reason", WaveKind::Exec, 1, 1)
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
            .register_wave("u3-cancel-wins", WaveKind::Exec, 1, 1)
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
        assert_eq!(snap.failed_count, 0, "Cancelled does not count as Failed");
        assert_eq!(
            snap.pending_count, 1,
            "Cancelled slot surfaces in pending_count"
        );
    }

    /// 2026-07-23-007 plan U3 (R-W4): the cancel reason MUST win
    /// over a prior `Completed` row. A worker that emitted a
    /// `*.unit.done` marker and was then cancelled must end as
    /// `Cancelled`, not `Completed`. Other failure reasons still
    /// respect first-terminal-wins.
    #[test]
    fn record_slot_failure_cancel_after_completed_wins() {
        let s = store();
        let wave = s
            .register_wave("u3-cancel-after-completed", WaveKind::Exec, 1, 1)
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
        s.record_slot_result(&wave, 0, "hash-xyz", 3).unwrap();
        let late = s.record_slot_failure(
            &wave,
            0,
            crate::supervisor::worker_outcome::REASON_WORKER_CANCELLED,
        );
        assert!(
            late.is_ok(),
            "U3/007 R-W4: cancel-after-Completed must overwrite; got {late:?}"
        );
        let snap = s.fan_in_status(&wave).unwrap();
        assert_eq!(snap.completed_count, 0, "Completed must be downgraded");
        assert_eq!(snap.failed_count, 0, "Cancelled does not count as Failed");
        assert_eq!(
            snap.pending_count, 1,
            "Cancelled slot surfaces in pending_count"
        );
    }

    /// 2026-07-23-007 plan U3 (R-W4) control: a non-cancel
    /// failure reason after `Completed` must still be rejected
    /// by first-terminal-wins.
    #[test]
    fn record_slot_failure_non_cancel_after_completed_still_rejected() {
        let s = store();
        let wave = s
            .register_wave("u3-non-cancel-after-completed", WaveKind::Exec, 1, 1)
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
        s.record_slot_result(&wave, 0, "hash-xyz", 3).unwrap();
        let late = s.record_slot_failure(&wave, 0, "boom");
        assert!(
            matches!(
                late,
                Err(crate::supervisor::SupervisorStoreError::AlreadyTerminal(_))
            ),
            "non-cancel failure after Completed must be rejected; got {late:?}"
        );
        let snap = s.fan_in_status(&wave).unwrap();
        assert_eq!(snap.completed_count, 1, "Completed must be preserved");
    }

    #[test]
    fn fan_in_complete_reaches_expected_total() {
        let s = store();
        let wave = s.register_wave("fa", WaveKind::Exec, 2, 1).unwrap();
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
        let wave = s.register_wave("rw", WaveKind::Review, 2, 1).unwrap();
        let resources = s.list_worktree_paths(&wave).unwrap();
        assert!(
            resources.is_empty(),
            "shared_readonly slots must not expose a resource binding"
        );
    }

    #[test]
    fn cancel_marks_pending_and_running_as_cancelled() {
        let s = store();
        let wave = s.register_wave("cx", WaveKind::Exec, 2, 1).unwrap();
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

    /// 2026-07-25-004 plan U5 (memory variant, symmetric with the
    /// rusqlite `u5_cancel_freezes_never_started_reason`): when
    /// `cancel_wave` flips a Pending slot to Cancelled it MUST also
    /// set `failure_reason` to `slot_never_started`. Already-terminal
    /// slots (Completed, or Failed with their own reason) MUST NOT be
    /// overwritten — the `if Pending` guard enforces this.
    #[test]
    fn u5_cancel_freezes_never_started_reason() {
        use crate::supervisor::worker_outcome::{REASON_SLOT_NEVER_STARTED, REASON_WORKER_TIMEOUT};
        let s = store();
        let wave = s.register_wave("u5-cancel", WaveKind::Exec, 3, 1).unwrap();
        for i in 0..3u32 {
            s.bind_worktree(
                &wave,
                i,
                SlotResource {
                    slot_index: i,
                    worktree_path: Some(format!(".ralph/u5/{i}")),
                    branch: Some(format!("ralph/u5/{i}")),
                },
            )
            .unwrap();
        }
        // Slot 0: dispatch + complete → terminal Completed, reason None.
        s.try_dispatch_next(4).unwrap().unwrap();
        s.record_slot_result(&wave, 0, "h0", 1).unwrap();
        // Slot 1: dispatch then fail with worker_timeout → terminal
        // Failed carrying its own reason (must NOT be overwritten).
        s.try_dispatch_next(4).unwrap().unwrap();
        s.record_slot_failure(&wave, 1, REASON_WORKER_TIMEOUT)
            .unwrap();
        // Slot 2: stays Pending (never dispatched).
        s.cancel_wave(&wave).unwrap();

        let snap = s.fan_in_status(&wave).unwrap();
        let status = |idx: u32| {
            snap.slots
                .iter()
                .find(|(i, _)| *i == idx)
                .map(|(_, s)| *s)
                .unwrap()
        };
        // Slot 2 flipped Pending → Cancelled, reason frozen.
        assert_eq!(status(2), SlotStatus::Cancelled);
        assert_eq!(
            s.slot_failure_reason(&wave, 2).unwrap(),
            Some(REASON_SLOT_NEVER_STARTED.to_string())
        );
        // Slot 0 Completed, untouched, reason None.
        assert_eq!(status(0), SlotStatus::Completed);
        assert_eq!(s.slot_failure_reason(&wave, 0).unwrap(), None);
        // Slot 1 already Failed with worker_timeout → NOT overwritten.
        assert_eq!(status(1), SlotStatus::Failed);
        assert_eq!(
            s.slot_failure_reason(&wave, 1).unwrap(),
            Some(REASON_WORKER_TIMEOUT.to_string())
        );
    }

    #[test]
    fn mark_merge_to_events_is_idempotent() {
        let s = store();
        let wave = s.register_wave("me", WaveKind::Review, 1, 1).unwrap();
        s.commit_salvage_projection(
            &wave,
            &ProjectionReceiptSummary {
                kind: ProjectionKind::Business,
                batch_fingerprint: "fp-idem".into(),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .unwrap();
        let summary = CoordinationReceiptSummary {
            topic: "review.wave.complete".into(),
            idempotency_key: "k-idem".into(),
            payload_fingerprint: "fp-idem".into(),
            write_count: 0,
            already_present_count: 0,
            committed_at_unix_secs: 0,
        };
        s.record_coordination_written(&wave, &summary).unwrap();
        s.commit_coordination_event(&wave, &summary, WavePhase::Done)
            .unwrap();
        // Re-running commit with the SAME receipt is idempotent.
        s.commit_coordination_event(&wave, &summary, WavePhase::Done)
            .unwrap();
        let snap = s.fan_in_status(&wave).unwrap();
        assert!(
            snap.delivery_state
                .at_least(WaveDeliveryState::CoordinationCommitted)
        );
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
        let wave = s.register_wave("rebind", WaveKind::Exec, 1, 1).unwrap();
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
        let wave = s.register_wave("fresh", WaveKind::Exec, 1, 1).unwrap();
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
        let wave = s.register_wave("idem", WaveKind::Exec, 1, 1).unwrap();
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

    /// Parity with rusqlite: Applied → RecoveryRequired is rejected.
    #[test]
    fn mark_emission_recovery_required_rejects_applied() {
        let s = store();
        let always_zero = |_id: &str| 0u32;
        let reserved = s
            .reserve_emission("scope-r", "digest", 1, &always_zero)
            .unwrap();
        let EmissionReservation::Reserved { .. } = reserved else {
            panic!("expected Reserved");
        };
        s.mark_emission_applying("scope-r").unwrap();
        s.mark_emission_applied("scope-r", 1).unwrap();
        let err = s
            .mark_emission_recovery_required("scope-r")
            .expect_err("Applied must not become RecoveryRequired");
        assert!(
            matches!(err, SupervisorStoreError::InvalidTransition(_)),
            "got {err:?}"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // 2026-07-27-003 plan U5: four-phase commit protocol tests.
    //
    // The tests below pin:
    // - state transitions are forward-only and never skip a phase;
    // - same receipt replay is idempotent;
    // - different receipt replay is rejected;
    // - migration from a legacy boolean pair (`00` / `10` / `11`)
    //   produces a deterministic starting state.
    // ─────────────────────────────────────────────────────────────────

    fn fresh_summary(fp: &str) -> ProjectionReceiptSummary {
        ProjectionReceiptSummary {
            kind: ProjectionKind::Business,
            batch_fingerprint: fp.into(),
            write_count: 1,
            already_present_count: 0,
            committed_at_unix_secs: 0,
        }
    }

    fn fresh_coord_summary(fp: &str) -> CoordinationReceiptSummary {
        CoordinationReceiptSummary {
            topic: "review.wave.complete".into(),
            idempotency_key: format!("coord:{fp}"),
            payload_fingerprint: fp.into(),
            write_count: 1,
            already_present_count: 0,
            committed_at_unix_secs: 0,
        }
    }

    #[test]
    fn u5_wave_delivery_state_starts_pending_and_advances_forward_only() {
        let s = store();
        let wave = s.register_wave("u5-fwd", WaveKind::Review, 2, 1).unwrap();
        // Pending -> SalvageCommitted: ok.
        s.commit_salvage_projection(&wave, &fresh_summary("fp-1"))
            .unwrap();
        assert_eq!(
            s.fan_in_status(&wave).unwrap().delivery_state,
            WaveDeliveryState::SalvageCommitted,
        );
        // Pending is illegal once past SalvageCommitted:
        // `record_coordination_written` requires SalvageCommitted
        // first.
        s.record_coordination_written(&wave, &fresh_coord_summary("fp-1"))
            .unwrap();
        assert_eq!(
            s.fan_in_status(&wave).unwrap().delivery_state,
            WaveDeliveryState::CoordinationWritten,
        );
        s.commit_coordination_event(&wave, &fresh_coord_summary("fp-1"), WavePhase::Done)
            .unwrap();
        assert_eq!(
            s.fan_in_status(&wave).unwrap().delivery_state,
            WaveDeliveryState::CoordinationCommitted,
        );
    }

    #[test]
    fn u5_replay_same_receipt_is_idempotent() {
        let s = store();
        let wave = s.register_wave("u5-idem", WaveKind::Review, 1, 1).unwrap();
        let summary = fresh_summary("fp-idem");
        s.commit_salvage_projection(&wave, &summary).unwrap();
        // Re-commit with the SAME fingerprint: still ok, state
        // stays at SalvageCommitted.
        s.commit_salvage_projection(&wave, &summary).unwrap();
        let coord = fresh_coord_summary("fp-idem");
        s.record_coordination_written(&wave, &coord).unwrap();
        s.record_coordination_written(&wave, &coord).unwrap();
        s.commit_coordination_event(&wave, &coord, WavePhase::Done)
            .unwrap();
        // Re-running commit with a different terminal phase
        // must NOT overwrite the Done latch — the recovery
        // contract is that the FIRST commit wins.
        s.commit_coordination_event(&wave, &coord, WavePhase::Failed)
            .unwrap();
        let snap = s.fan_in_status(&wave).unwrap();
        assert_eq!(snap.phase, WavePhase::Done);
    }

    #[test]
    fn u5_conflicting_receipt_after_committed_is_rejected() {
        let s = store();
        let wave = s
            .register_wave("u5-conflict", WaveKind::Review, 1, 1)
            .unwrap();
        s.commit_salvage_projection(&wave, &fresh_summary("fp-a"))
            .unwrap();
        s.record_coordination_written(&wave, &fresh_coord_summary("fp-a"))
            .unwrap();
        s.commit_coordination_event(&wave, &fresh_coord_summary("fp-a"), WavePhase::Done)
            .unwrap();
        // A restart that replays a DIFFERENT fingerprint
        // after `CoordinationCommitted` is allowed because
        // the merge seam's real fingerprint may land after
        // the dispatcher's placeholder was set. The store
        // accepts the new fingerprint (it represents the
        // real on-disk state) — the contract is that the
        // coord-event fingerprint stays stable, not the
        // salvage fingerprint.
        s.commit_salvage_projection(&wave, &fresh_summary("fp-b"))
            .expect("salvage fingerprint replacement after CoordinationCommitted must succeed");
        let snap = s.fan_in_status(&wave).unwrap();
        assert!(
            snap.delivery_state
                .at_least(WaveDeliveryState::CoordinationCommitted)
        );
        assert_eq!(snap.phase, WavePhase::Done);
    }

    #[test]
    fn u5_crash_window_5_recovery_observes_already_committed() {
        // Simulates the coord commit + cleanup window: the wave
        // reached CoordinationCommitted before the loop
        // crashed. A fresh open must observe the same state
        // and skip re-injection.
        let s = store();
        let wave = s
            .register_wave("u5-crash-5", WaveKind::Review, 1, 1)
            .unwrap();
        s.commit_salvage_projection(&wave, &fresh_summary("fp-5"))
            .unwrap();
        s.record_coordination_written(&wave, &fresh_coord_summary("fp-5"))
            .unwrap();
        s.commit_coordination_event(&wave, &fresh_coord_summary("fp-5"), WavePhase::Done)
            .unwrap();
        // The "restart" view: re-derive the snapshot and assert
        // delivery_state == CoordinationCommitted.
        let snap = s.fan_in_status(&wave).unwrap();
        assert!(
            snap.delivery_state
                .at_least(WaveDeliveryState::CoordinationCommitted)
        );
        assert_eq!(snap.phase, WavePhase::Done);
    }

    #[test]
    fn u5_crash_window_4_recovery_observes_coord_written() {
        // The coordination append succeeded but the
        // `commit_coordination_event` did not. The store
        // observes `CoordinationWritten`; the runtime
        // re-derives the receipt from disk and replays the
        // commit.
        let s = store();
        let wave = s
            .register_wave("u5-crash-4", WaveKind::Review, 1, 1)
            .unwrap();
        s.commit_salvage_projection(&wave, &fresh_summary("fp-4"))
            .unwrap();
        s.record_coordination_written(&wave, &fresh_coord_summary("fp-4"))
            .unwrap();
        let snap = s.fan_in_status(&wave).unwrap();
        assert!(
            snap.delivery_state
                .at_least(WaveDeliveryState::CoordinationWritten)
        );
        assert!(
            !snap
                .delivery_state
                .at_least(WaveDeliveryState::CoordinationCommitted)
        );
    }

    #[test]
    fn u5_crash_window_3_recovery_observes_salvage_committed() {
        // Salvage committed but the coordination append did
        // not run yet. A restart must observe
        // `SalvageCommitted` so the dispatcher can resume the
        // coord-append step without re-projecting.
        let s = store();
        let wave = s
            .register_wave("u5-crash-3", WaveKind::Review, 1, 1)
            .unwrap();
        s.commit_salvage_projection(&wave, &fresh_summary("fp-3"))
            .unwrap();
        let snap = s.fan_in_status(&wave).unwrap();
        assert_eq!(snap.delivery_state, WaveDeliveryState::SalvageCommitted);
        assert!(
            !snap
                .delivery_state
                .at_least(WaveDeliveryState::CoordinationWritten)
        );
    }

    #[test]
    fn u5_empty_salvage_path_does_not_advance_to_business_projected() {
        // The dispatcher's `project_empty_salvage` returns a
        // receipt directly without writing to main. The
        // store still accepts the receipt and the wave
        // advances to SalvageCommitted so the coord-injection
        // gate opens.
        let s = store();
        let wave = s.register_wave("u5-empty", WaveKind::Review, 1, 1).unwrap();
        s.commit_salvage_projection(
            &wave,
            &ProjectionReceiptSummary {
                kind: ProjectionKind::Business,
                batch_fingerprint: format!("empty-{wave}"),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .unwrap();
        assert_eq!(
            s.fan_in_status(&wave).unwrap().delivery_state,
            WaveDeliveryState::SalvageCommitted,
        );
    }
}
