//! 2026-07-03-001 plan (supervisor path real wiring): the
//! `SupervisorBridge` trait + supporting types, sunk down from
//! `ralph-cli/src/loop_runner/wave/supervisor_bridge.rs` so the
//! BDD scenarios in `ralph-core` can construct a real bridge
//! without depending on `ralph-cli`.
//!
//! The trait surface is the contract between the wave dispatcher
//! and the supervisor coordinator. The production implementation
//! (`CoordinatorSupervisorBridge`) and the test mock
//! (`MockSupervisorBridge`) live in `ralph-cli`; the BDD-specific
//! `InMemoryCoordinatorBridge` lives in this crate (see below).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::supervisor::{CoordinatorAction, SlotResource, WaveKind, WaveSnapshot};
use crate::supervisor::{PhaseInputs, SupervisorStore, SupervisorStoreError};

/// Outcome of dispatching a single slot through the bridge.
/// The runtime logs the action and forwards it to the
/// `ralph diagnose` aggregator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeDispatchOutcome {
    /// Bridge accepted the slot and either returned a binding
    /// (worker is going to spawn on the worktree) or none
    /// (review shared_readonly → no env, no worker spawn).
    BoundOrShared,
    /// Bridge recorded the binding but the slot is already
    /// in a non-dispatchable state (failed, cancelled, etc.).
    /// The runtime surfaces it as a `task.resume`.
    NotDispatchable(String),
    /// Bridge opened the store and reports it is fully wired.
    Started,
}

/// Lightweight binding bundle for the dispatcher hot path —
/// the trait does NOT hand back a full `SlotResource` so
/// the worker process can construct one from the
/// `DispatchOutcome` events that arrive back through JSONL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotBinding {
    pub slot_index: u32,
    pub env: HashMap<String, String>,
    pub worktree_path: Option<PathBuf>,
}

/// Bridge-side errors (mirrors `SupervisorStoreError`).
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("supervisor store error: {0}")]
    Store(String),
    #[error("dispatch not allowed: {0}")]
    NotDispatchable(String),
    #[error("supervisor bridge is not wired (supervisor.enabled: false)")]
    Disabled,
}

impl From<SupervisorStoreError> for BridgeError {
    fn from(err: SupervisorStoreError) -> Self {
        BridgeError::Store(err.to_string())
    }
}

/// Trait surface so the dispatcher (and BDD scenarios) can
/// substitute a mock for tests without bringing up git. The
/// production implementation lives in `ralph-cli`; the BDD
/// `InMemoryCoordinatorBridge` lives in this crate.
pub trait SupervisorBridge: std::fmt::Debug + Send + Sync {
    /// Shared serialization gate for supervisor fan-in side effects.
    /// Independent waves may execute concurrently, while merge and
    /// coordination commits remain serialized against the main ledger.
    fn fan_in_lock(&self) -> std::sync::Arc<std::sync::Mutex<()>> {
        std::sync::Arc::new(std::sync::Mutex::new(()))
    }

    /// Run one tick of the supervisor fan-in for `wave_id`.
    /// Returns the coordinator action so the dispatcher can
    /// decide whether to merge worker events + persist the
    /// `*.wave.complete` / `*.wave.failed` coordination event.
    fn tick(&self, wave_id: &str, inputs: PhaseInputs) -> Result<CoordinatorAction, BridgeError>;

    /// U6: run one tick of the supervisor fan-in, merging the
    /// per-slot worker business events (`slot_events`, ordered by
    /// slot index and de-duplicated by the caller) through the
    /// coordinator's merge sink on the `Integrate` path. The
    /// production dispatcher's `run_supervisor_fan_in` calls this
    /// so the real fan-in output lands in the main ledger. The
    /// default delegates to [`Self::tick`] (empty batch) so mocks
    /// and the in-memory BDD bridge keep working unchanged.
    fn tick_with_slot_events(
        &self,
        wave_id: &str,
        inputs: PhaseInputs,
        slot_events: Vec<ralph_proto::Event>,
    ) -> Result<CoordinatorAction, BridgeError> {
        let _ = slot_events;
        self.tick(wave_id, inputs)
    }

    /// U6: fetch the per-slot resource bindings for a wave
    /// (`slot_index` + `branch` + `worktree_path`). The
    /// dispatcher's `run_supervisor_fan_in` uses this to build
    /// the `success_slots` payload on the `*.wave.complete`
    /// coordination event. Default: no resources (mocks / bridges
    /// without a store).
    fn slot_resources(&self, _wave_id: &str) -> Result<Vec<SlotResource>, BridgeError> {
        Ok(Vec::new())
    }

    /// Global worker cap exposed to the dispatcher. Bridges that do not
    /// provide store-backed dispatch approval retain the legacy unlimited
    /// behavior.
    fn max_concurrent_workers(&self) -> u32 {
        u32::MAX
    }

    /// 2026-07-28-003 plan U4 (R8 / KTD6): per-slot automatic retry
    /// budget returned to the worker-task attempt loop in
    /// `dispatcher.rs`. The task consults this BEFORE the very
    /// first attempt so the loop count matches the configured
    /// budget.
    ///
    /// **No default implementation** — every `SupervisorBridge`
    /// impl MUST provide a value. Production bridges surface the
    /// operator's `SupervisorConfig.slot_retry_budget` explicitly;
    /// mock / BDD / stub bridges must declare their own
    /// `slot_retry_budget()` (typically `0` to disable auto-retry
    /// for characterization tests). This makes "missing override"
    /// a compile error instead of a silent default of `1`.
    fn slot_retry_budget(&self) -> u32;

    /// 2026-07-23-007 plan U2 (R-W1): return the loop's primary
    /// workspace root when the bridge was constructed with one
    /// (i.e. `with_context_and_factory` in production). The
    /// dispatcher uses this to validate the per-worker events
    /// channel and to inject `RALPH_WORKSPACE_ROOT` into the
    /// spawned worker's env. Default `None` keeps the legacy
    /// mock / BDD bridges compatible — those bridges have no
    /// workspace context to expose.
    fn repo_root(&self) -> Option<&std::path::Path> {
        None
    }

    /// 2026-07-28-002 plan U3 (R3 / S2a): return the
    /// underlying `Arc<dyn SupervisorStore>` when the bridge was
    /// constructed with one. Used by the dispatcher to call
    /// `persist_slot_descriptor` after a successful `bind_slot`.
    /// Default `None` keeps the existing mock contracts working.
    fn store(&self) -> Option<std::sync::Arc<dyn SupervisorStore>> {
        None
    }

    /// 2026-07-23-007 plan U4 (R-W5): return the loop's
    /// `tasks.jsonl` path when the bridge was constructed with
    /// one. The dispatcher projects slot transitions onto the
    /// runtime task ledger; `None` disables the projection
    /// (legacy / mock bridges). Default `None` keeps the
    /// existing mock contracts working.
    fn tasks_path(&self) -> Option<&std::path::Path> {
        None
    }

    /// Ask the bridge whether the requested slot is approved for dispatch.
    /// The default keeps legacy/mock bridges compatible: without a store
    /// approval surface, the caller may proceed with its existing spawn path.
    fn try_dispatch_next(&self, _wave_id: &str, _slot_index: u32) -> Result<bool, BridgeError> {
        Ok(true)
    }

    /// Open a slot dispatch decision: returns the binding (or
    /// `None` for `SharedReadonly`) so the dispatcher knows
    /// whether to spawn a real worker process and with which
    /// env / cwd.
    fn bind_slot(
        &self,
        kind: WaveKind,
        wave_id: &str,
        slot_index: u32,
    ) -> Result<Option<SlotBinding>, BridgeError>;

    /// Snapshot helper for tests and the `ralph diagnose`
    /// surface.
    fn recover(&self) -> Result<Vec<WaveSnapshot>, BridgeError>;

    /// 2026-07-25-004 plan U5 (R6 / AE5): read the current
    /// slot/lifecycle snapshot from the underlying store.
    /// Used by the diagnostics JSON builder in the InjectedFailed
    /// arm. Default: return an error so bridges without a store
    /// don't silently return wrong data; implementations that have
    /// a store override this.
    fn fan_in_status(&self, wave_id: &str) -> Result<WaveSnapshot, BridgeError>;

    /// 2026-07-27-004 plan U5 (R17 / P0): stamp the first
    /// delivery phase (`Pending` → `BusinessProjected`) on the
    /// store after the dispatcher's salvage merge seam lands
    /// the Completed slots' business events on main. The
    /// strict rusqlite `commit_salvage_projection` gate
    /// refuses a `Pending` wave, so every salvage seam must
    /// stamp this BEFORE calling `commit_salvage_projection`.
    /// Default: `Ok(())` so store-less mock bridges keep
    /// compiling; production bridges delegate to the store.
    fn record_business_projection(
        &self,
        wave_id: &str,
        receipt: &crate::supervisor::ProjectionReceiptSummary,
    ) -> Result<(), BridgeError> {
        let _ = (wave_id, receipt);
        Ok(())
    }

    /// 2026-07-27-003 plan U5: bridge-level convenience for the
    /// dispatcher's failed-fan-in salvage commit. Bridges that
    /// own a `SupervisorStore` delegate to it; the default impl
    /// returns `Unsupported` so mock bridges in tests stay
    /// compilable. The dispatcher's failed-path arms call this
    /// AFTER `merge_completed_*_slots_to_main` lands the
    /// Completed-slots business events on main. The receipt is
    /// the SOLE proof the write succeeded; the coordinator's
    /// `commit_coordination_event` refuses to advance
    /// `WaveDeliveryState` without a matching salvage receipt.
    fn commit_salvage_projection(
        &self,
        wave_id: &str,
        receipt: &crate::supervisor::ProjectionReceiptSummary,
    ) -> Result<(), BridgeError> {
        let _ = (wave_id, receipt);
        Err(BridgeError::Store(
            "commit_salvage_projection: bridge does not own a store".to_string(),
        ))
    }

    /// 2026-07-27-003 plan U5: persist a coordination write
    /// receipt on the store side. Bridges delegate to the
    /// underlying store when they own one.
    fn record_coordination_written(
        &self,
        wave_id: &str,
        receipt: &crate::supervisor::CoordinationReceiptSummary,
    ) -> Result<(), BridgeError> {
        let _ = (wave_id, receipt);
        Err(BridgeError::Store(
            "record_coordination_written: bridge does not own a store".to_string(),
        ))
    }

    /// 2026-07-27-003 plan U5: finalise the delivery — set the
    /// wave to its terminal phase and the delivery state to
    /// `CoordinationCommitted`. Bridges delegate to the
    /// underlying store.
    fn commit_coordination_event(
        &self,
        wave_id: &str,
        receipt: &crate::supervisor::CoordinationReceiptSummary,
        terminal_phase: crate::supervisor::WavePhase,
    ) -> Result<(), BridgeError> {
        let _ = (wave_id, receipt, terminal_phase);
        Err(BridgeError::Store(
            "commit_coordination_event: bridge does not own a store".to_string(),
        ))
    }

    /// 2026-07-03-001 supervisor real-wiring: register a wave
    /// in the supervisor store if it is not already present.
    /// Idempotent — re-registering the same `wave_id` is a
    /// no-op. The dispatcher calls this once per detected wave
    /// before binding any slots.
    ///
    /// Returns the wave_id the store actually assigned. The
    /// store implementations allocate a fresh `w-{seq}` id from
    /// the supplied idempotency key; the dispatcher SHOULD use
    /// the returned id for all subsequent `bind_slot` /
    /// `record_slot_result` / `tick` calls. When the wave was
    /// already registered (idempotent re-entry), the existing
    /// wave_id is returned unchanged.
    fn register_wave_if_absent(
        &self,
        kind: WaveKind,
        wave_id: &str,
        expected_total: u32,
        slot_retry_budget: u32,
    ) -> Result<String, BridgeError>;

    /// 2026-07-03-001 supervisor real-wiring: record a slot's
    /// successful completion in the supervisor store so the
    /// coordinator's `tick` can advance the fan-in. Called by
    /// the dispatcher's `run_supervisor_fan_in` for every
    /// entry in `completed.results`.
    fn record_slot_result(
        &self,
        wave_id: &str,
        slot_index: u32,
        content_hash: &str,
        event_count: usize,
    ) -> Result<(), BridgeError>;

    /// 2026-07-26-004 plan U2 (KTD3): attach bounded terminal evidence
    /// to a `Completed` slot via the underlying store. Default: no-op
    /// for mocks / bridges without a store. Store-backed bridges
    /// override this to delegate to [`SupervisorStore::record_slot_terminal_evidence`].
    fn record_slot_terminal_evidence(
        &self,
        _wave_id: &str,
        _slot_index: u32,
        _evidence: &crate::supervisor::TerminalEvidence,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    /// 2026-09-01-001 plan U5 (R5 / D6): record the worker
    /// pid into `dispatch_records.pid` so `ralph diagnose`
    /// surfaces the real OS-level pid. Default: no-op so
    /// mocks / bridges without a store still compile.
    fn record_slot_pid(
        &self,
        _wave_id: &str,
        _slot_index: u32,
        _pid: u32,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    /// 2026-09-01-001 plan U1 (R1 / D1-D3): persist a slot's
    /// accepted event list via the underlying store. Default:
    /// no-op so mocks / bridges without a store still compile.
    /// Store-backed bridges override this to delegate to
    /// [`SupervisorStore::record_slot_event_payloads`].
    fn record_slot_event_payloads(
        &self,
        _wave_id: &str,
        _slot_index: u32,
        _attempt_seq: u32,
        _events: &[crate::Event],
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    /// 2026-09-01-001 plan U2 (R2 / D3): load every persisted
    /// payload for `(wave_id)` so crash recovery can rebuild a
    /// `CompletedWave`-shaped input for the salvage seam.
    /// Default: empty (no persistence layer).
    fn load_slot_event_payloads(
        &self,
        _wave_id: &str,
    ) -> Result<Vec<(u32, u32, Vec<crate::Event>)>, BridgeError> {
        Ok(Vec::new())
    }

    /// 2026-09-01-001 plan U1 (R1 / S1.2): drop every persisted
    /// payload row for `(wave_id)`. Called by `run_supervisor_fan_in`
    /// after the slot events have been merged into the main ledger.
    /// Default: no-op for bridges without persistence.
    fn delete_slot_event_payloads(&self, _wave_id: &str) -> Result<(), BridgeError> {
        Ok(())
    }

    /// 2026-07-26-004 plan U2 (KTD3): read a slot's terminal evidence
    /// from the underlying store. Default: `Ok(None)` (not provably
    /// done) for mocks / bridges without a store.
    fn slot_terminal_evidence(
        &self,
        _wave_id: &str,
        _slot_index: u32,
    ) -> Result<Option<crate::supervisor::TerminalEvidence>, BridgeError> {
        Ok(None)
    }

    /// 2026-07-03-001 supervisor real-wiring: record a slot's
    /// permanent failure. Called by the dispatcher's
    /// `run_supervisor_fan_in` for every entry in
    /// `completed.failures`.
    fn record_slot_failure(
        &self,
        wave_id: &str,
        slot_index: u32,
        reason: &str,
    ) -> Result<(), BridgeError>;

    /// 2026-07-25-004 plan U4 (R4 / R5 / AE4): record
    /// `slot_never_started` for every slot that is still
    /// `Pending` (never reached `Dispatched`/`Running`) when a
    /// wave fails. Idempotent: same-reason replay is a no-op
    /// because `record_slot_failure` already enforces first-terminal-wins.
    ///
    /// 2026-07-25-004 plan U3: this shared default IS the single
    /// authoritative implementation — the store-backed bridges
    /// (`InMemoryCoordinatorBridge`, `CoordinatorSupervisorBridge`)
    /// inherit it instead of duplicating the logic. Per-slot
    /// errors are NOT swallowed: a non-idempotent rejection
    /// (e.g. `AlreadyTerminal` for a slot that reached a terminal
    /// state with a different reason between the snapshot read
    /// and the record) propagates upward so the dispatcher's
    /// warn branch is reachable for real I/O / lock / state
    /// errors.
    fn record_never_started_failures(&self, wave_id: &str) -> Result<(), BridgeError> {
        use crate::supervisor::SlotStatus;
        use crate::supervisor::worker_outcome::REASON_SLOT_NEVER_STARTED;
        let snap = self.fan_in_status(wave_id)?;
        for (slot_index, status) in &snap.slots {
            if *status == SlotStatus::Pending {
                // Fail-fast: same-reason replays already return
                // Ok(()) via the store's idempotency contract, so
                // any Err here is a real non-idempotent rejection
                // that the caller must observe.
                self.record_slot_failure(wave_id, *slot_index, REASON_SLOT_NEVER_STARTED)?;
            }
        }
        Ok(())
    }

    /// 2026-07-25-004 plan U5 (R6 / AE5): read a slot's
    /// recorded failure reason. Returns `None` for non-failed
    /// slots (Completed, Pending, Dispatched, Running) or when
    /// the reason was never recorded. Default: `Ok(None)` so
    /// mocks / bridges without a store stay compiling.
    fn slot_failure_reason(
        &self,
        _wave_id: &str,
        _slot_index: u32,
    ) -> Result<Option<String>, BridgeError> {
        Ok(None)
    }

    /// Release the global dispatch permit after a worker reaches a
    /// terminal state. Bridges without a store retain the legacy no-op.
    fn release_slot_dispatch(
        &self,
        _wave_id: &str,
        _slot_index: u32,
        _outcome: crate::supervisor::DispatchOutcome,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    /// U7 (2026-07-23-002) / KTD8 / R13: idempotent terminal finalizer.
    /// Remove every slot worktree the store recorded for this loop.
    /// `NotFound` is treated as already-cleaned success so restart
    /// recovery can safely re-run the finalizer. Default: no-op for
    /// mocks / bridges without a store.
    fn finalize_terminal_cleanup(&self, _repo_root: &std::path::Path) -> Result<(), BridgeError> {
        Ok(())
    }

    /// 2026-07-22-001 plan U4 (KTD-8): mark `wave_id` cancelled in
    /// the underlying store. The dispatcher calls this on aggregate
    /// timeout / global deadline / spawn failure so subsequent
    /// `tick`s observe the new phase and `inspect` surfaces the
    /// `cancelled` state. Default: no-op so existing mocks keep
    /// compiling. The store-level cancel does NOT itself kill the
    /// spawned worker child; the dispatcher's deadline path owns
    /// the process kill.
    fn cancel_wave(&self, _wave_id: &str) -> Result<(), BridgeError> {
        Ok(())
    }

    /// U1: set the wave phase directly. Used by the terminal
    /// fan-in convergence driver to force the Failed phase when the
    /// coordinator's fail_wave would otherwise be refused or would
    /// leave the wave in ContinueCollect. Default: no-op for mocks.
    fn set_wave_phase(
        &self,
        _wave_id: &str,
        _phase: crate::supervisor::WavePhase,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    /// 2026-07-22-001 plan U6 (KTD-7): enqueue a compensation
    /// job for `wave_id`. Default: no-op (mocks / BDD bridges
    /// without a store).
    fn enqueue_compensation(
        &self,
        _wave_id: &str,
        _kind: crate::supervisor::CompensationKind,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    /// 2026-07-22-001 plan U6: drain pending compensation jobs.
    /// Default: empty.
    fn take_pending_compensations(
        &self,
    ) -> Result<Vec<(String, crate::supervisor::CompensationKind)>, BridgeError> {
        Ok(Vec::new())
    }

    /// 2026-07-22-001 plan U6: mark a drained compensation job
    /// completed. Default: no-op.
    fn complete_compensation(
        &self,
        _wave_id: &str,
        _kind: crate::supervisor::CompensationKind,
        _ok: bool,
    ) -> Result<(), BridgeError> {
        Ok(())
    }
}

/// BDD-specific bridge that wires a `SupervisorCoordinator`
/// directly to an in-memory or rusqlite `SupervisorStore`
/// without pulling in `ralph-cli`'s worker-spawn path. Used by
/// `crates/ralph-core/tests/scenarios.rs` so the
/// `supervisor_minimal` scenario exercises the real
/// coordinator `tick` → `InjectedComplete` →
/// `persist_system_injected_jsonl_event` path instead of
/// faking the `system_injected` envelope via a mock response.
#[derive(Debug, Clone)]
pub struct InMemoryCoordinatorBridge {
    store: std::sync::Arc<dyn SupervisorStore>,
    coordinator: std::sync::Arc<crate::supervisor::SupervisorCoordinator>,
    /// Map from the caller-supplied idempotency key (the
    /// dispatcher's wave_id) to the store-assigned wave_id
    /// (`w-{seq}`). `register_wave_if_absent` populates this on
    /// first registration and returns the stored id on
    /// idempotent re-entry so the dispatcher always sees a
    /// stable wave_id across fan-in ticks.
    registered: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
}

impl InMemoryCoordinatorBridge {
    /// Build a bridge around an existing store. The coordinator
    /// is constructed with an in-memory merge sink that buffers
    /// events for the dispatcher to drain.
    pub fn from_store(store: std::sync::Arc<dyn SupervisorStore>) -> Self {
        let coordinator = std::sync::Arc::new(
            crate::supervisor::SupervisorCoordinator::with_in_memory_sink(store.clone()),
        );
        Self {
            store,
            coordinator,
            registered: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
        }
    }

    /// Access the underlying store (BDD tests assert
    /// `fan_in_status` on it).
    pub fn store(&self) -> std::sync::Arc<dyn SupervisorStore> {
        self.store.clone()
    }
}

impl SupervisorBridge for InMemoryCoordinatorBridge {
    fn store(&self) -> Option<std::sync::Arc<dyn SupervisorStore>> {
        Some(self.store.clone())
    }

    fn tick(&self, wave_id: &str, inputs: PhaseInputs) -> Result<CoordinatorAction, BridgeError> {
        self.coordinator
            .tick(wave_id, inputs)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn tick_with_slot_events(
        &self,
        wave_id: &str,
        inputs: PhaseInputs,
        slot_events: Vec<ralph_proto::Event>,
    ) -> Result<CoordinatorAction, BridgeError> {
        self.coordinator
            .tick_with_slot_events(wave_id, inputs, slot_events)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn slot_resources(&self, wave_id: &str) -> Result<Vec<SlotResource>, BridgeError> {
        self.store
            .list_worktree_paths(wave_id)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    /// BDD / in-memory bridge: return 0 so the BDD scenarios
    /// don't auto-retry — they assert specific failure shapes
    /// and intermediate-attempt counts.
    fn slot_retry_budget(&self) -> u32 {
        0
    }

    fn bind_slot(
        &self,
        _kind: WaveKind,
        _wave_id: &str,
        slot_index: u32,
    ) -> Result<Option<SlotBinding>, BridgeError> {
        // BDD scenarios do not spawn real workers; the bridge
        // returns an empty binding so the dispatcher's supervisor
        // path can still record the slot index without a worktree.
        Ok(Some(SlotBinding {
            slot_index,
            env: HashMap::new(),
            worktree_path: None,
        }))
    }

    fn recover(&self) -> Result<Vec<WaveSnapshot>, BridgeError> {
        self.store
            .recover_active_waves()
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn fan_in_status(&self, wave_id: &str) -> Result<WaveSnapshot, BridgeError> {
        self.store
            .fan_in_status(wave_id)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn commit_salvage_projection(
        &self,
        wave_id: &str,
        receipt: &crate::supervisor::ProjectionReceiptSummary,
    ) -> Result<(), BridgeError> {
        self.store
            .commit_salvage_projection(wave_id, receipt)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn record_coordination_written(
        &self,
        wave_id: &str,
        receipt: &crate::supervisor::CoordinationReceiptSummary,
    ) -> Result<(), BridgeError> {
        self.store
            .record_coordination_written(wave_id, receipt)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn commit_coordination_event(
        &self,
        wave_id: &str,
        receipt: &crate::supervisor::CoordinationReceiptSummary,
        terminal_phase: crate::supervisor::WavePhase,
    ) -> Result<(), BridgeError> {
        self.store
            .commit_coordination_event(wave_id, receipt, terminal_phase)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn register_wave_if_absent(
        &self,
        kind: WaveKind,
        wave_id: &str,
        expected_total: u32,
        slot_retry_budget: u32,
    ) -> Result<String, BridgeError> {
        let mut guard = self.registered.lock().unwrap();
        if let Some(existing) = guard.get(wave_id) {
            return Ok(existing.clone());
        }
        // 2026-07-23-004 plan U2 (R-A2): the store allocates a
        // distinct `w-{seq}` id from the idempotency key on first
        // registration. On a process restart the in-memory map is
        // empty, so a duplicate-key return is expected — but the
        // caller key MUST NOT be returned as the store id. Look
        // the original store id back up via the persistent
        // idempotency_key index. When the lookup misses, surface
        // a `BridgeError::Store` (the wave is gone from disk —
        // cannot resolve the caller's caller-key back to a store
        // row, so refuse to fabricate a fake id).
        let store_id =
            match self
                .store
                .register_wave(wave_id, kind, expected_total, slot_retry_budget)
            {
                Ok(id) => id,
                Err(SupervisorStoreError::DuplicateKey(_)) => {
                    match self.store.wave_id_for_idempotency_key(wave_id) {
                        Ok(Some(resolved)) => resolved,
                        Ok(None) => {
                            return Err(BridgeError::Store(format!(
                                "duplicate idempotency_key={wave_id} but store has no row"
                            )));
                        }
                        Err(err) => return Err(BridgeError::Store(err.to_string())),
                    }
                }
                Err(err) => return Err(BridgeError::Store(err.to_string())),
            };
        guard.insert(wave_id.to_string(), store_id.clone());
        Ok(store_id)
    }

    fn record_slot_result(
        &self,
        wave_id: &str,
        slot_index: u32,
        content_hash: &str,
        event_count: usize,
    ) -> Result<(), BridgeError> {
        self.store
            .record_slot_result(wave_id, slot_index, content_hash, event_count)?;
        Ok(())
    }

    fn record_slot_terminal_evidence(
        &self,
        wave_id: &str,
        slot_index: u32,
        evidence: &crate::supervisor::TerminalEvidence,
    ) -> Result<(), BridgeError> {
        self.store
            .record_slot_terminal_evidence(wave_id, slot_index, evidence)?;
        Ok(())
    }

    fn record_slot_pid(&self, wave_id: &str, slot_index: u32, pid: u32) -> Result<(), BridgeError> {
        self.store.record_slot_pid(wave_id, slot_index, pid)?;
        Ok(())
    }

    fn record_slot_event_payloads(
        &self,
        wave_id: &str,
        slot_index: u32,
        attempt_seq: u32,
        events: &[crate::Event],
    ) -> Result<(), BridgeError> {
        self.store
            .record_slot_event_payloads(wave_id, slot_index, attempt_seq, events)?;
        Ok(())
    }

    fn load_slot_event_payloads(
        &self,
        wave_id: &str,
    ) -> Result<Vec<(u32, u32, Vec<crate::Event>)>, BridgeError> {
        Ok(self.store.load_slot_event_payloads(wave_id)?)
    }

    fn delete_slot_event_payloads(&self, wave_id: &str) -> Result<(), BridgeError> {
        self.store.delete_slot_event_payloads(wave_id)?;
        Ok(())
    }

    fn slot_terminal_evidence(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> Result<Option<crate::supervisor::TerminalEvidence>, BridgeError> {
        Ok(self.store.slot_terminal_evidence(wave_id, slot_index)?)
    }

    fn record_slot_failure(
        &self,
        wave_id: &str,
        slot_index: u32,
        reason: &str,
    ) -> Result<(), BridgeError> {
        self.store
            .record_slot_failure(wave_id, slot_index, reason)?;
        Ok(())
    }

    fn slot_failure_reason(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> Result<Option<String>, BridgeError> {
        self.store
            .slot_failure_reason(wave_id, slot_index)
            .map_err(|e| BridgeError::Store(e.to_string()))
    }

    fn release_slot_dispatch(
        &self,
        wave_id: &str,
        slot_index: u32,
        outcome: crate::supervisor::DispatchOutcome,
    ) -> Result<(), BridgeError> {
        self.store
            .release_slot_dispatch(wave_id, slot_index, outcome)?;
        Ok(())
    }

    fn cancel_wave(&self, wave_id: &str) -> Result<(), BridgeError> {
        // 2026-07-22-001 plan U4 (KTD-8): thread cancel into the
        // store; NotFound is treated as success so the BDD
        // scenarios stay robust to racey cancel/recover orderings.
        match self.store.cancel_wave(wave_id) {
            Ok(()) => Ok(()),
            Err(SupervisorStoreError::UnknownWave(_)) => Ok(()),
            Err(err) => Err(BridgeError::Store(err.to_string())),
        }
    }

    fn enqueue_compensation(
        &self,
        wave_id: &str,
        kind: crate::supervisor::CompensationKind,
    ) -> Result<(), BridgeError> {
        self.store
            .enqueue_compensation(wave_id, kind)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn take_pending_compensations(
        &self,
    ) -> Result<Vec<(String, crate::supervisor::CompensationKind)>, BridgeError> {
        self.store
            .take_pending_compensations()
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn complete_compensation(
        &self,
        wave_id: &str,
        kind: crate::supervisor::CompensationKind,
        ok: bool,
    ) -> Result<(), BridgeError> {
        self.store
            .complete_compensation(wave_id, kind, ok)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }
}

/// Decide whether a loop instance should activate the supervisor
/// bridge: `enabled === true && execution_mode === isolated`.
/// Lives here (not just in `ralph-cli`) so the BDD scenario
/// runner can reuse the same predicate.
pub fn is_supervisor_path_enabled(enabled: bool, execution_mode_isolated: bool) -> bool {
    enabled && execution_mode_isolated
}

#[cfg(test)]
mod tests {
    //! Closed-circuit tests for the sunk-down bridge surface.
    //! The production `CoordinatorSupervisorBridge` and
    //! `MockSupervisorBridge` impls are exercised in
    //! `ralph-cli`'s `wave_supervisor` tests; this module
    //! pins the `InMemoryCoordinatorBridge` BDD contract.
    use super::*;
    use crate::supervisor::{InMemorySupervisorStore, SlotResource, WaveKind};

    #[test]
    fn in_memory_bridge_register_is_idempotent() {
        let store = std::sync::Arc::new(InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(
            store.clone() as std::sync::Arc<dyn SupervisorStore>
        );
        let store_id = bridge
            .register_wave_if_absent(WaveKind::Exec, "bdd-wave", 1, 1)
            .unwrap();
        // Second call must be a no-op (returns the same store id).
        let store_id_again = bridge
            .register_wave_if_absent(WaveKind::Exec, "bdd-wave", 1, 1)
            .unwrap();
        assert_eq!(store_id, store_id_again);
        let snap = store.fan_in_status(&store_id).unwrap();
        assert_eq!(snap.expected_total, 1);
    }

    #[test]
    fn in_memory_bridge_records_slot_result_and_ticks() {
        let store = std::sync::Arc::new(InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(
            store.clone() as std::sync::Arc<dyn SupervisorStore>
        );
        let store_id = bridge
            .register_wave_if_absent(WaveKind::Exec, "bdd-wave", 1, 1)
            .unwrap();
        store
            .bind_worktree(
                &store_id,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/bdd".to_string()),
                    branch: Some("ralph/bdd".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        bridge
            .record_slot_result(&store_id, 0, "bdd-hash", 1)
            .unwrap();
        // Plan 004 R2 / P0-2: Completed slot must carry
        // terminal evidence for the success fan-in path to
        // engage. Without this the coordinator falls into
        // `Failed(IncompleteEvidence)` and the BDD assertion
        // flips.
        store
            .record_slot_terminal_evidence(
                &store_id,
                0,
                &crate::supervisor::TerminalEvidence::from_event(
                    "exec.unit.done",
                    "{\"dimension\":\"default\"}",
                ),
            )
            .unwrap();
        let action = bridge
            .tick(
                &store_id,
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

    #[test]
    fn is_supervisor_path_enabled_predicate() {
        assert!(is_supervisor_path_enabled(true, true));
        assert!(!is_supervisor_path_enabled(true, false));
        assert!(!is_supervisor_path_enabled(false, true));
        assert!(!is_supervisor_path_enabled(false, false));
    }

    // ─────────────────────────────────────────────────────────────────
    // G2: 2026-07-25-004 plan U4 (R4 / R5 / AE4)
    // Store integration: `record_never_started_failures`
    // ─────────────────────────────────────────────────────────────────

    /// G2 T1: register wave (expected_total=3), complete slot 0,
    /// never touch slots 1, 2 → call `record_never_started_failures`
    /// → slots 1, 2 are Failed with reason `slot_never_started`,
    /// slot 0 stays Completed.
    #[test]
    fn g2_record_never_started_marks_pending_slots() {
        use crate::supervisor::SlotStatus;

        let store = std::sync::Arc::new(InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(
            store.clone() as std::sync::Arc<dyn SupervisorStore>
        );

        let store_id = bridge
            .register_wave_if_absent(WaveKind::Exec, "g2-wave", 3, 1)
            .unwrap();

        // Dispatch and complete slot 0.
        store
            .bind_worktree(
                &store_id,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/g2".to_string()),
                    branch: Some("ralph/g2".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(4).unwrap().unwrap();
        store
            .record_slot_result(&store_id, 0, "hash-g2", 1)
            .unwrap();

        // Slots 1 and 2 are still Pending — never dispatched.
        // Call the helper.
        bridge.record_never_started_failures(&store_id).unwrap();

        // Verify slot 0 stayed Completed.
        let snap = store.fan_in_status(&store_id).unwrap();
        assert_eq!(
            snap.slots.iter().find(|(i, _)| *i == 0).map(|(_, s)| *s),
            Some(SlotStatus::Completed),
            "slot 0 must stay Completed"
        );

        // Verify slots 1 and 2 are now Failed with reason `slot_never_started`.
        for slot_index in [1u32, 2] {
            let snap = store.fan_in_status(&store_id).unwrap();
            let (_, status) = snap.slots.iter().find(|(i, _)| *i == slot_index).unwrap();
            assert_eq!(
                status,
                &SlotStatus::Failed,
                "slot {slot_index} must be Failed"
            );
        }

        // Idempotency: second call is a no-op (same-reason replay → Ok).
        bridge.record_never_started_failures(&store_id).unwrap();
        let snap2 = store.fan_in_status(&store_id).unwrap();
        assert_eq!(
            snap2.failed_count, snap.failed_count,
            "second call must not double-count failures"
        );
    }

    /// Test-only bridge that serves a fixed (stale) snapshot from
    /// `fan_in_status` while delegating every other call to a real
    /// `InMemoryCoordinatorBridge`. Simulates the race where the
    /// snapshot was read before the store learned a slot had
    /// already reached a terminal state with a different reason —
    /// the rejection `record_never_started_failures` must
    /// propagate instead of swallowing. Inherits the trait's
    /// shared default `record_never_started_failures`.
    #[derive(Debug)]
    struct StaleSnapshotBridge {
        inner: InMemoryCoordinatorBridge,
        stale: WaveSnapshot,
    }

    impl SupervisorBridge for StaleSnapshotBridge {
        fn tick(
            &self,
            wave_id: &str,
            inputs: PhaseInputs,
        ) -> Result<CoordinatorAction, BridgeError> {
            self.inner.tick(wave_id, inputs)
        }

        fn slot_retry_budget(&self) -> u32 {
            self.inner.slot_retry_budget()
        }

        fn bind_slot(
            &self,
            kind: WaveKind,
            wave_id: &str,
            slot_index: u32,
        ) -> Result<Option<SlotBinding>, BridgeError> {
            self.inner.bind_slot(kind, wave_id, slot_index)
        }

        fn recover(&self) -> Result<Vec<WaveSnapshot>, BridgeError> {
            self.inner.recover()
        }

        fn fan_in_status(&self, _wave_id: &str) -> Result<WaveSnapshot, BridgeError> {
            Ok(self.stale.clone())
        }

        fn register_wave_if_absent(
            &self,
            kind: WaveKind,
            wave_id: &str,
            expected_total: u32,
            slot_retry_budget: u32,
        ) -> Result<String, BridgeError> {
            self.inner
                .register_wave_if_absent(kind, wave_id, expected_total, slot_retry_budget)
        }

        fn record_slot_result(
            &self,
            wave_id: &str,
            slot_index: u32,
            content_hash: &str,
            event_count: usize,
        ) -> Result<(), BridgeError> {
            self.inner
                .record_slot_result(wave_id, slot_index, content_hash, event_count)
        }

        fn record_slot_failure(
            &self,
            wave_id: &str,
            slot_index: u32,
            reason: &str,
        ) -> Result<(), BridgeError> {
            self.inner.record_slot_failure(wave_id, slot_index, reason)
        }
    }

    /// G2 T2: a slot the (stale) snapshot still shows as `Pending`
    /// is already terminally `Failed(worker_timeout)` in the store.
    /// The store's first-terminal-wins contract rejects the
    /// different-reason replay (`AlreadyTerminal`); the helper MUST
    /// propagate that rejection as `Err` instead of swallowing it
    /// with `let _ =`.
    #[test]
    fn g2_record_never_started_propagates_non_idempotent_error() {
        use crate::supervisor::SlotStatus;
        use crate::supervisor::worker_outcome::REASON_WORKER_TIMEOUT;

        let store = std::sync::Arc::new(InMemorySupervisorStore::new());
        let inner = InMemoryCoordinatorBridge::from_store(
            store.clone() as std::sync::Arc<dyn SupervisorStore>
        );

        let store_id = inner
            .register_wave_if_absent(WaveKind::Exec, "g2-prop-wave", 2, 1)
            .unwrap();

        // Slot 1: terminally Failed with a DIFFERENT reason than
        // the `slot_never_started` the helper is about to write.
        inner
            .record_slot_failure(&store_id, 1, REASON_WORKER_TIMEOUT)
            .unwrap();

        // Stale view: the snapshot still shows slot 1 as Pending
        // (read before the store learned about the failure), so
        // the helper attempts `slot_never_started` on it.
        let mut stale = inner.fan_in_status(&store_id).unwrap();
        for (idx, status) in stale.slots.iter_mut() {
            if *idx == 1 {
                *status = SlotStatus::Pending;
            }
        }

        let bridge = StaleSnapshotBridge { inner, stale };
        let result = bridge.record_never_started_failures(&store_id);
        assert!(
            result.is_err(),
            "non-idempotent per-slot rejection must propagate upward; got {result:?}"
        );
    }

    /// G2 T3: `fan_in_status` on an unknown wave returns `Err` —
    /// the helper must propagate it (existing behavior preserved
    /// by the shared implementation).
    #[test]
    fn g2_record_never_started_unknown_wave_errors() {
        let store = std::sync::Arc::new(InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(
            store.clone() as std::sync::Arc<dyn SupervisorStore>
        );

        let result = bridge.record_never_started_failures("no-such-wave");
        assert!(
            result.is_err(),
            "fan_in_status error on an unknown wave must propagate; got {result:?}"
        );
    }
}

#[cfg(test)]
mod dispatch_surface_tests {
    use super::*;

    #[test]
    fn test_trait_exposes_dispatch_surface() {
        let store = std::sync::Arc::new(crate::supervisor::InMemorySupervisorStore::new());
        let bridge =
            InMemoryCoordinatorBridge::from_store(store as std::sync::Arc<dyn SupervisorStore>);

        assert_eq!(bridge.max_concurrent_workers(), u32::MAX);
        assert!(bridge.try_dispatch_next("missing-wave", 0).unwrap());
    }
}
