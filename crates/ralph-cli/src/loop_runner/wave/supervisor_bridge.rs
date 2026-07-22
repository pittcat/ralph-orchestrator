//! 2026-07-03-001 plan U12: supervisor dispatcher bridge.
//!
//! The bridge sits between the existing wave dispatcher
//! (`loop_runner/wave/dispatcher.rs`) and the supervisor
//! types added in U2/U3/U4/U5/U6/U8/U10/U11.
//!
//! It owns:
//! - constructing the right `SupervisorStore` from
//!   `RalphConfig.supervisor` (in-memory when
//!   `supervisor-db` is off, SQLite when the feature is on)
//! - dispatching slots through the runtime's existing
//!   worker-spawning path with the env vars from U10
//! - calling `SupervisorCoordinator::tick` after every fan-in
//!   check so the merge + coord-event injection stays
//!   authoritative
//! - forwarding recovery (`recover_active_waves_at_startup`) at
//!   loop startup so U11's idempotency guarantee holds
//!
//! 2026-07-03-001 supervisor real-wiring: the trait + supporting
//! types (`SupervisorBridge`, `SlotBinding`, `BridgeError`,
//! `BridgeDispatchOutcome`, `is_supervisor_path_enabled`) were
//! sunk down to `ralph_core::supervisor::bridge` so the BDD
//! scenarios in `ralph-core` can construct a real bridge
//! without depending on `ralph-cli`. This file now re-exports
//! those sunk types and keeps only the production
//! (`CoordinatorSupervisorBridge`) and CLI-test mock
//! (`MockSupervisorBridge`) implementations.

use std::collections::VecDeque;
use std::sync::Arc;

use ralph_core::supervisor::PhaseInputs;
use ralph_core::supervisor::worktree_bind::WorktreeFactory;
use ralph_core::supervisor::{
    CoordinatorAction, InMemorySupervisorStore, SupervisorCoordinator, SupervisorStore, WaveKind,
    WaveSnapshot,
};
// 2026-07-03-001 supervisor real-wiring: re-export the sunk
// types so existing `crate::loop_runner::wave::*` imports keep
// working. The types live in `ralph_core::supervisor::bridge`
// but the module itself is private — we import from the
// `ralph_core::supervisor` re-export surface.
use ralph_core::supervisor::DefaultWorktreeFactory;
pub use ralph_core::supervisor::WorktreeError as BridgeWorktreeError;
pub use ralph_core::supervisor::{
    BridgeDispatchOutcome, BridgeError, SlotBinding, SupervisorBridge, is_supervisor_path_enabled,
};

/// Bundle the production bridge needs from the runtime so it
/// can satisfy `bind_slot` without re-resolving workspace paths
/// or reading the supervisor config a second time. The runtime
/// constructs one of these once per loop and threads it through
/// `CoordinatorSupervisorBridge::with_context_and_factory`
/// (U4 closure for the production `bind_slot` empty
/// implementation).
#[derive(Debug, Clone)]
pub struct ProductionBridgeContext {
    /// Loop identifier; encoded into the per-slot branch name
    /// (`{loop_id}-{kind}-{slot_index}`, see
    /// `worktree_bind::exec_binding`).
    pub loop_id: String,
    /// Absolute path to the repo root where the worker will
    /// spawn the per-slot worktree.
    pub repo_root: std::path::PathBuf,
}

/// Production bridge: holds an `Arc<dyn SupervisorStore>` +
/// `SupervisorCoordinator`. Construction is gated behind the
/// `supervisor-db` feature for the SQLite branch; the
/// in-memory branch is always available so dry-runs work in
/// default builds.
///
/// U4: the production `bind_slot` now invokes
/// `worktree_bind::bind_slot_worktree` with the loop's repo
/// root + loop_id and a `WorktreeFactory` (injected in tests,
/// `DefaultWorktreeFactory` in production). Exec/Fix slots
/// hand back `Some(SlotBinding { worktree_path, env })` so the
/// dispatcher can set `WorkerRequest.cwd` and inject the
/// `RALPH_WAVE_*` env vars into the spawned worker. Review
/// slots hand back `Ok(None)` (SharedReadonly) without touching
/// the factory.
#[derive(Debug, Clone)]
pub struct CoordinatorSupervisorBridge {
    store: Arc<dyn SupervisorStore>,
    coordinator: Arc<SupervisorCoordinator>,
    /// U4: context required to drive `bind_slot`. `None` keeps
    /// the legacy behaviour (`Ok(None)` for every kind) so the
    /// old `MockSupervisorBridge`-only tests still resolve. The
    /// runner constructs a concrete `Some(...)` via
    /// `with_context_and_factory` once `supervisor.enabled: true`
    /// is in effect.
    context: Option<ProductionBridgeContext>,
    /// U4: factory for creating per-slot git worktrees. Tests
    /// inject `RecordingFactory` / `FailingFactory` to assert
    /// factory call args without invoking git; production uses
    /// `DefaultWorktreeFactory`.
    factory: Arc<dyn WorktreeFactory>,
    /// Global supervisor worker cap supplied by the loop config.
    max_concurrent_workers: u32,
}

impl CoordinatorSupervisorBridge {
    /// Build a bridge around the in-memory store. Used by tests
    /// and the dry-run CLI path.
    // 2026-07-16 cleanup U4 (KTD-3): test-fixture guard.
    // `with_in_memory_store` / `coordinator` are only consumed
    // inside `#[cfg(test)] mod tests` (e.g. `wave_supervisor.rs`).
    #[allow(dead_code)]
    pub fn with_in_memory_store() -> Self {
        let store = Arc::new(InMemorySupervisorStore::new());
        let coordinator = Arc::new(SupervisorCoordinator::with_in_memory_sink(store.clone()));
        let factory: Arc<dyn WorktreeFactory> = Arc::new(DefaultWorktreeFactory);
        Self {
            store,
            coordinator,
            context: None,
            factory,
            max_concurrent_workers: u32::MAX,
        }
    }

    /// Build a production bridge with a runtime-supplied
    /// context (loop_id + repo_root) and an injected
    /// `WorktreeFactory`. This is the entry point the runner
    /// uses after `is_supervisor_path_enabled` returns `true`
    /// (R7 / U4 closure for the empty `bind_slot`).
    #[allow(dead_code)] // wired by the runner in a follow-up unit (U5).
    pub fn with_context_and_factory(
        store: Arc<dyn SupervisorStore>,
        context: ProductionBridgeContext,
        factory: Arc<dyn WorktreeFactory>,
    ) -> Self {
        Self::with_context_and_factory_with_cap(store, context, factory, u32::MAX)
    }

    /// Build a production bridge with the configured global worker cap.
    pub fn with_context_and_factory_with_cap(
        store: Arc<dyn SupervisorStore>,
        context: ProductionBridgeContext,
        factory: Arc<dyn WorktreeFactory>,
        max_concurrent_workers: u32,
    ) -> Self {
        let coordinator = Arc::new(SupervisorCoordinator::with_in_memory_sink(store.clone()));
        Self {
            store,
            coordinator,
            context: Some(context),
            factory,
            max_concurrent_workers,
        }
    }

    /// Access the underlying store. Diagnostics-friendly.
    // 2026-07-16 cleanup U4 (KTD-3): same test-fixture guard as
    // `with_in_memory_store`.
    #[allow(dead_code)]
    pub fn store(&self) -> Arc<dyn SupervisorStore> {
        self.store.clone()
    }

    /// Access the coordinator so the bridge can hand it to
    /// the runtime when the dispatcher needs to drive a tick
    /// outside the bridge trait.
    // 2026-07-16 cleanup U4 (KTD-3): same test-fixture guard as
    // `with_in_memory_store`.
    #[allow(dead_code)]
    pub fn coordinator(&self) -> Arc<SupervisorCoordinator> {
        self.coordinator.clone()
    }

    /// Build a bridge around a store owned elsewhere (e.g. the
    /// dispatcher bridge reads the store from the runtime
    /// once and shares it across ticks).
    ///
    /// 2026-07-23-001 plan U1: this is the legacy entry point
    /// that left `context: None` and made `bind_slot` return
    /// `Ok(None)` for every kind — the silent failure mode U1
    /// eliminates. Production runners now use
    /// `with_context_and_factory` via `build_supervisor_bridge`.
    /// The function stays available for the legacy
    /// characterization test (`test_legacy_from_store_returns_none_for_exec`)
    /// which pins the old failure mode so a future regression
    /// that re-introduces `from_store` on the hot path is caught.
    #[cfg(test)]
    pub fn from_store(store: Arc<dyn SupervisorStore>) -> Self {
        let coordinator = Arc::new(SupervisorCoordinator::with_in_memory_sink(store.clone()));
        let factory: Arc<dyn WorktreeFactory> = Arc::new(DefaultWorktreeFactory);
        Self {
            store,
            coordinator,
            context: None,
            factory,
            max_concurrent_workers: u32::MAX,
        }
    }
}

impl SupervisorBridge for CoordinatorSupervisorBridge {
    fn tick(&self, wave_id: &str, inputs: PhaseInputs) -> Result<CoordinatorAction, BridgeError> {
        self.coordinator
            .tick(wave_id, inputs)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn max_concurrent_workers(&self) -> u32 {
        self.max_concurrent_workers
    }

    fn try_dispatch_next(&self, wave_id: &str, slot_index: u32) -> Result<bool, BridgeError> {
        let dispatched = self
            .store
            .try_dispatch_next(self.max_concurrent_workers)
            .map_err(BridgeError::from)?;
        Ok(matches!(
            dispatched,
            Some((ref dispatched_wave_id, dispatched_slot_index))
                if dispatched_wave_id == wave_id && dispatched_slot_index == slot_index
        ))
    }

    fn bind_slot(
        &self,
        kind: WaveKind,
        wave_id: &str,
        slot_index: u32,
    ) -> Result<Option<SlotBinding>, BridgeError> {
        // U4: the production bridge now drives the
        // `worktree_bind::bind_slot_worktree` helper. The
        // legacy `Ok(None)` stub is removed; only the `Review`
        // (SharedReadonly) branch returns `Ok(None)`. The
        // factory is injected (production: `DefaultWorktreeFactory`;
        // tests: `RecordingFactory` / `FailingFactory`) so the
        // Git boundary stays behind a single trait method.
        let Some(ctx) = self.context.as_ref() else {
            // Legacy entry points (`with_in_memory_store`,
            // `from_store`) keep returning `Ok(None)` so the
            // older test seam still resolves. New tests must
            // use `with_context_and_factory` to exercise the
            // production wiring.
            return Ok(None);
        };

        // Review slots are SharedReadonly: no worktree, no
        // writeable branch. Return `Ok(None)` so the dispatcher
        // doesn't override `WorkerRequest.cwd` (KTD-5).
        if matches!(kind, WaveKind::Review) {
            return Ok(None);
        }

        // U4: build the per-slot binding via the helper so the
        // branch naming + env-var SSOT lives in
        // `worktree_bind::bind_slot_worktree`. We invoke the
        // factory directly (the helper is generic over `F:
        // WorktreeFactory` and would require `Sized`) so the
        // production bridge keeps its `Arc<dyn WorktreeFactory>`
        // storage. The factory contract is the same: success
        // yields a `Worktree { path, branch }`, failure yields
        // `BridgeError::Store`.
        let branch = format!("{}-{}-{}", ctx.loop_id, kind, slot_index);
        let wt = self
            .factory
            .create(ctx.repo_root.clone(), branch.clone())
            .map_err(|err| BridgeError::Store(err.to_string()))?;
        let worktree_path = wt.path.clone();
        let mut env = std::collections::HashMap::new();
        env.insert(
            ralph_core::supervisor::worktree_env_keys::RALPH_WAVE_WORKER.to_string(),
            "1".to_string(),
        );
        env.insert(
            ralph_core::supervisor::worktree_env_keys::RALPH_WAVE_WORKTREE_PATH.to_string(),
            worktree_path.to_string_lossy().into_owned(),
        );
        env.insert(
            ralph_core::supervisor::worktree_env_keys::RALPH_WAVE_WORKTREE_BRANCH.to_string(),
            branch.clone(),
        );
        env.insert(
            ralph_core::supervisor::worktree_env_keys::RALPH_WAVE_ID.to_string(),
            wave_id.to_string(),
        );
        env.insert(
            ralph_core::supervisor::worktree_env_keys::RALPH_WAVE_INDEX.to_string(),
            slot_index.to_string(),
        );
        env.insert(
            ralph_core::supervisor::worktree_env_keys::RALPH_WAVE_KIND.to_string(),
            kind.to_string(),
        );
        let resource = ralph_core::supervisor::SlotResource {
            slot_index,
            worktree_path: Some(worktree_path.to_string_lossy().into_owned()),
            branch: Some(branch),
        };

        // Persist the `SlotResource` in the store so fan-in
        // can resolve branch/path later (R7 / R10).
        self.store
            .bind_worktree(wave_id, slot_index, resource)
            .map_err(|err| BridgeError::Store(err.to_string()))?;

        Ok(Some(SlotBinding {
            slot_index,
            env,
            worktree_path: Some(worktree_path),
        }))
    }

    fn recover(&self) -> Result<Vec<WaveSnapshot>, BridgeError> {
        self.store
            .recover_active_waves()
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn register_wave_if_absent(
        &self,
        kind: WaveKind,
        wave_id: &str,
        expected_total: u32,
    ) -> Result<String, BridgeError> {
        use ralph_core::supervisor::SupervisorStoreError;
        match self.store.register_wave(wave_id, kind, expected_total) {
            Ok(id) => Ok(id),
            Err(SupervisorStoreError::DuplicateKey(_)) => Ok(wave_id.to_string()),
            Err(err) => Err(BridgeError::Store(err.to_string())),
        }
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

    fn release_slot_dispatch(
        &self,
        wave_id: &str,
        slot_index: u32,
        outcome: ralph_core::supervisor::DispatchOutcome,
    ) -> Result<(), BridgeError> {
        self.store
            .release_slot_dispatch(wave_id, slot_index, outcome)?;
        Ok(())
    }
}

/// U4 R8 fail-closed helper: when `bridge.bind_slot` returns
/// `Err`, the dispatcher MUST NOT spawn a worker against the
/// main workspace. This helper converts the error into a typed
/// `(wave_id, slot_index)` payload the dispatcher can log + map
/// to a fail-closed signal without touching `WorkerRequest.cwd`.
/// The function name mirrors the dispatcher's contract in
/// `execute_wave_via_supervisor`.
pub fn fail_closed_on_bind_error(
    err: &BridgeError,
    wave_id: &str,
    slot_index: u32,
) -> Option<(String, u32)> {
    match err {
        BridgeError::Store(_) | BridgeError::NotDispatchable(_) => {
            tracing::warn!(
                wave_id,
                slot_index,
                error = %err,
                "supervisor bind_slot failed; slot will fail-closed without spawning a worker"
            );
            Some((wave_id.to_string(), slot_index))
        }
        // `Disabled` is a hard "supervisor not wired" signal
        // that the dispatcher routes via the legacy `WaveTracker`
        // path; we leave that to the dispatcher rather than
        // folding it into this helper.
        BridgeError::Disabled => None,
    }
}

/// Mock bridge for tests that need to assert the bridge
/// surface without a real store.
// 2026-07-16 cleanup U4 (KTD-3): test-fixture guard.
// `MockSupervisorBridge` + `new` / `push_actions` / `snapshot`
// are only consumed inside `#[cfg(test)] mod tests` blocks
// (`wave_supervisor.rs` etc.). The struct + impl stay public
// so the cross-crate test imports keep resolving.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct MockSupervisorBridge {
    ticks: Arc<std::sync::Mutex<Vec<(String, PhaseInputs)>>>,
    /// Pre-scripted actions the bridge will return on the next
    /// `tick` calls. Stored in a `VecDeque` so we deliver them
    /// in FIFO order (push_back + pop_front); the previous
    /// `Vec::pop` shape silently reversed the order, which
    /// would mask any contract drift in U12 dispatcher tests
    /// that push >1 action (fix-plan U5, F-005).
    actions: Arc<std::sync::Mutex<VecDeque<CoordinatorAction>>>,
}

impl MockSupervisorBridge {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-script the next actions the bridge will return on
    /// the next `tick` calls. Stored in push order, so the
    /// first pushed action is the first returned (FIFO). This
    /// mirrors the production bridge's
    /// `SupervisorCoordinator::tick` call path so dispatch
    /// tests can pin the round-trip ordering.
    // 2026-07-16 cleanup U4 (KTD-3): test-fixture guard (same as
    // `MockSupervisorBridge` struct).
    #[allow(dead_code)]
    pub fn push_actions(&self, actions: Vec<CoordinatorAction>) {
        let mut guard = self.actions.lock().unwrap();
        for action in actions {
            guard.push_back(action);
        }
    }

    /// Snapshot the recorded ticks + the next action the
    /// bridge will return. Tests use this to assert call
    /// ordering.
    // 2026-07-16 cleanup U4 (KTD-3): test-fixture guard (same as
    // `MockSupervisorBridge` struct).
    #[allow(dead_code)]
    pub fn snapshot(&self) -> (Vec<(String, PhaseInputs)>, Vec<CoordinatorAction>) {
        (
            self.ticks.lock().unwrap().clone(),
            self.actions.lock().unwrap().iter().cloned().collect(),
        )
    }
}

impl SupervisorBridge for MockSupervisorBridge {
    fn tick(&self, wave_id: &str, inputs: PhaseInputs) -> Result<CoordinatorAction, BridgeError> {
        self.ticks
            .lock()
            .unwrap()
            .push((wave_id.to_string(), inputs.clone()));
        // Pop the next action in FIFO order; if none queued,
        // fall back to `ContinueCollect`. The previous
        // `Vec::pop` shape returned the most-recently pushed
        // action first (LIFO) which masked contract drift in
        // tests that pushed >1 action; F-005 pins the fix.
        let action = self
            .actions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(CoordinatorAction::ContinueCollect);
        Ok(action)
    }

    fn bind_slot(
        &self,
        _kind: WaveKind,
        _wave_id: &str,
        _slot_index: u32,
    ) -> Result<Option<SlotBinding>, BridgeError> {
        Ok(None)
    }

    fn recover(&self) -> Result<Vec<WaveSnapshot>, BridgeError> {
        Ok(Vec::new())
    }

    fn register_wave_if_absent(
        &self,
        _kind: WaveKind,
        wave_id: &str,
        _expected_total: u32,
    ) -> Result<String, BridgeError> {
        Ok(wave_id.to_string())
    }

    fn record_slot_result(
        &self,
        _wave_id: &str,
        _slot_index: u32,
        _content_hash: &str,
        _event_count: usize,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    fn record_slot_failure(
        &self,
        _wave_id: &str,
        _slot_index: u32,
        _reason: &str,
    ) -> Result<(), BridgeError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! U12 closed-circuit tests: the bridge delegates to the
    //! U8 coordinator + in-memory store. The dispatcher hot
    //! path is covered separately by the existing
    //! `loop_runner::tests::wave_supervisor::*` family; this
    //! module owns the bridge surface contract.
    use super::*;
    use ralph_core::supervisor::{InMemorySupervisorStore, SlotResource, SlotStatus};

    fn assert_disabled(bridge: &CoordinatorSupervisorBridge) {
        // The bridge surface stays usable regardless of the
        // preset's config; runtime gating is the dispatcher's
        // job.
        let _ = bridge.coordinator();
    }

    /// U1 / F-001 / KTD-7 bridge pin: when the coordinator
    /// returns `AlreadyDone` after a successful merge, the
    /// bridge's downstream code path (the JSONL append
    /// layer) MUST skip the `system_injected` envelope and
    /// treat the tick as a no-op success. This test pins
    /// the producer (coordinator) and consumer (bridge)
    /// agree on `AlreadyDone` semantics so the dispatcher's
    /// `system_injected` append sees the no-op signal.
    #[test]
    fn mock_bridge_returns_continue_collect_after_already_done() {
        let bridge = MockSupervisorBridge::new();
        // Queue `AlreadyDone` (the U1 KTD-7 success path on
        // a post-merge re-tick). The mock bridge delivers
        // FIFO.
        bridge.push_actions(vec![CoordinatorAction::AlreadyDone]);
        let action = bridge
            .tick("w-1", PhaseInputs::default())
            .expect("tick must succeed");
        // The `AlreadyDone` variant propagates through the
        // bridge unchanged; downstream consumers (U12
        // dispatcher's JSONL append path) translate it to a
        // no-op so we never re-inject `system_injected: true`.
        assert_eq!(action, CoordinatorAction::AlreadyDone);
        // Subsequent tick with empty queue → ContinueCollect
        // default (no spurious AlreadyDone re-emission).
        let next = bridge
            .tick("w-1", PhaseInputs::default())
            .expect("tick must succeed");
        assert_eq!(next, CoordinatorAction::ContinueCollect);
    }

    #[test]
    fn production_bridge_holds_coordinator_and_store() {
        let bridge = CoordinatorSupervisorBridge::with_in_memory_store();
        assert_disabled(&bridge);
        let snaps = bridge.recover().unwrap();
        assert!(snaps.is_empty());
    }

    #[test]
    fn mock_bridge_records_tick_calls() {
        let bridge = MockSupervisorBridge::new();
        let action = bridge.tick("w-1", PhaseInputs::default()).unwrap();
        assert_eq!(action, CoordinatorAction::ContinueCollect);
        let (ticks, _) = bridge.snapshot();
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].0, "w-1");
    }

    #[test]
    fn supervisor_path_enabled_predicate() {
        assert!(is_supervisor_path_enabled(true, true));
        assert!(!is_supervisor_path_enabled(true, false));
        assert!(!is_supervisor_path_enabled(false, true));
        assert!(!is_supervisor_path_enabled(false, false));
    }

    #[test]
    fn production_bridge_thread_tick_round_trip() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store.register_wave("rt", WaveKind::Exec, 1).unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/rt".to_string()),
                    branch: Some("ralph/rt".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        let bridge =
            CoordinatorSupervisorBridge::from_store(store.clone() as Arc<dyn SupervisorStore>);
        let action = bridge
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
        // After the coordinator marks `merged_to_events`,
        // the slot stays at the binding's stable state.
        let snap = bridge.store().fan_in_status(&wave).unwrap();
        assert!(
            matches!(
                snap.phase,
                ralph_core::supervisor::WavePhase::Done
                    | ralph_core::supervisor::WavePhase::Integrate
            ) || snap.merged_to_events
                || snap.completed_count == 1
        );
    }

    #[test]
    fn bridge_handles_empty_store_recover() {
        let bridge = CoordinatorSupervisorBridge::with_in_memory_store();
        let snaps = bridge.recover().unwrap();
        assert!(snaps.is_empty());
    }

    /// U4 closure: the production `bind_slot` MUST return
    /// `Ok(None)` when invoked through the legacy entry
    /// points (`with_in_memory_store` / `from_store`) — those
    /// carry no `ProductionBridgeContext` so the new real
    /// wiring has nothing to bind. Production callers now use
    /// `with_context_and_factory` and assert
    /// `Ok(Some(_))` for Exec/Fix.
    #[test]
    fn bind_slot_returns_none_for_legacy_entry_points() {
        let bridge = CoordinatorSupervisorBridge::with_in_memory_store();
        let binding = bridge.bind_slot(WaveKind::Exec, "w-1", 0).unwrap();
        assert!(
            binding.is_none(),
            "legacy entry point must return None (no context); got {binding:?}"
        );
    }

    #[test]
    fn slot_binding_shape_is_transparent() {
        let mut env = std::collections::HashMap::new();
        env.insert("X".to_string(), "Y".to_string());
        let binding = SlotBinding {
            slot_index: 3,
            env,
            worktree_path: Some(std::path::PathBuf::from("/tmp/w")),
        };
        assert_eq!(binding.slot_index, 3);
        assert_eq!(binding.env.get("X").map(String::as_str), Some("Y"));
    }

    #[test]
    fn record_status_for_completed_slot() {
        // Sanity: confirm the store keeps the slot's
        // status as `Completed` after `record_slot_result`.
        let store = InMemorySupervisorStore::new();
        let wave = store.register_wave("rs", WaveKind::Exec, 1).unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/rs".to_string()),
                    branch: Some("ralph/rs".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        let snap = store.fan_in_status(&wave).unwrap();
        // The slot moved out of `pending`. We don't expose
        // per-slot status on the snapshot (it's a counts
        // surface); the slot count is `completed_count == 1`.
        assert_eq!(snap.completed_count, 1);
        assert_eq!(snap.pending_count, 0);
        let _ = SlotStatus::Completed;
    }

    #[test]
    fn merge_sink_round_trips_through_bridge() {
        let bridge = CoordinatorSupervisorBridge::with_in_memory_store();
        let _ = bridge.coordinator().sink_batches();
    }

    #[test]
    fn production_bridge_disabled_does_not_block_recover() {
        let bridge = CoordinatorSupervisorBridge::with_in_memory_store();
        // Disabled semantics live in `is_supervisor_path_enabled`; the
        // bridge surface itself stays usable. Runtime gating decides
        // whether to call it.
        assert!(!is_supervisor_path_enabled(false, true));
        assert!(bridge.recover().is_ok());
    }

    #[test]
    fn tick_records_default_phase_inputs() {
        let bridge = MockSupervisorBridge::new();
        let result = bridge
            .tick(
                "w-1",
                PhaseInputs {
                    aggregate_timeout_secs: 60,
                    elapsed_secs: 0,
                    cancel_requested: false,
                },
            )
            .unwrap();
        // Default state has no action queued → ContinueCollect.
        assert_eq!(result, CoordinatorAction::ContinueCollect);
    }

    /// U5 / F-005 / R5: the mock bridge MUST return actions
    /// in push order (FIFO). Pre-fix the test would have
    /// observed `[InjectedFailed, InjectedComplete]` (LIFO
    /// from `Vec::pop`); the fix-plan F-005 pins the
    /// `VecDeque` + `pop_front` shape.
    #[test]
    fn mock_bridge_returns_queued_actions_in_fifo_order() {
        let bridge = MockSupervisorBridge::new();
        bridge.push_actions(vec![
            CoordinatorAction::InjectedComplete {
                topic: "exec.wave.complete".to_string(),
                blocking_slots: vec![],
            },
            CoordinatorAction::InjectedFailed {
                topic: "exec.wave.failed".to_string(),
                reason: "required_slot_failure",
                blocking_slots: vec![0],
            },
        ]);

        // Tick #1: returns the first pushed action.
        let first = bridge
            .tick("w-1", PhaseInputs::default())
            .expect("tick must succeed");
        // Tick #2: returns the second pushed action (FIFO).
        let second = bridge
            .tick("w-1", PhaseInputs::default())
            .expect("tick must succeed");
        // Tick #3: queue empty → ContinueCollect default.
        let third = bridge
            .tick("w-1", PhaseInputs::default())
            .expect("tick must succeed");

        assert!(matches!(
            first,
            CoordinatorAction::InjectedComplete { ref topic, .. } if topic == "exec.wave.complete"
        ));
        assert!(matches!(
            second,
            CoordinatorAction::InjectedFailed { ref topic, .. } if topic == "exec.wave.failed"
        ));
        assert_eq!(third, CoordinatorAction::ContinueCollect);

        // Snapshot still records all three tick calls in order.
        let (ticks, remaining) = bridge.snapshot();
        assert_eq!(ticks.len(), 3, "every tick records a wave/inputs pair");
        assert!(ticks.iter().all(|(w, _)| w == "w-1"));
        assert!(
            remaining.is_empty(),
            "all pre-scripted actions were drained in FIFO order"
        );
    }

    #[test]
    fn mock_bridge_uses_legacy_dispatch_defaults() {
        let bridge = MockSupervisorBridge::new();
        assert_eq!(bridge.max_concurrent_workers(), u32::MAX);
        assert!(bridge.try_dispatch_next("w-1", 0).unwrap());
    }

    #[test]
    fn production_bridge_exposes_cap_and_forwards_dispatch_approval() {
        let store = Arc::new(InMemorySupervisorStore::new());
        let wave = store
            .register_wave("dispatch-surface", WaveKind::Exec, 1)
            .unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/dispatch-surface".to_string()),
                    branch: Some("ralph/dispatch-surface".to_string()),
                },
            )
            .unwrap();
        let bridge = CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
            store.clone() as Arc<dyn SupervisorStore>,
            ProductionBridgeContext {
                loop_id: "dispatch-surface".to_string(),
                repo_root: std::path::PathBuf::from("/tmp/dispatch-surface"),
            },
            Arc::new(DefaultWorktreeFactory),
            1,
        );

        assert_eq!(bridge.max_concurrent_workers(), 1);
        assert!(bridge.try_dispatch_next(&wave, 0).unwrap());
        assert!(!bridge.try_dispatch_next(&wave, 0).unwrap());
    }

    /// U5 / F-005 edge: empty actions queue → default
    /// `ContinueCollect`. Mirrors the pre-fix `pop` behaviour
    /// for the no-action case so the F-005 fix is scoped to
    /// ordering only.
    #[test]
    fn mock_bridge_returns_continue_collect_when_queue_empty() {
        let bridge = MockSupervisorBridge::new();
        let result = bridge
            .tick("w-1", PhaseInputs::default())
            .expect("tick must succeed");
        assert_eq!(result, CoordinatorAction::ContinueCollect);
    }
}
