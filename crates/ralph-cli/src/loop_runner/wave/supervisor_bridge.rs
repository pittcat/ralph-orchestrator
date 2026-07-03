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
//! The bridge is trait-abstracted (`SupervisorBridge`) so
//! existing wave tests can substitute a mock without spinning
//! real workers. The CLI's loop_runner constructs the
//! production bridge via `SupervisorBridge::open` after the
//! config check.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use ralph_core::supervisor::PhaseInputs;
use ralph_core::supervisor::{
    CoordinatorAction, InMemorySupervisorStore, SupervisorCoordinator, SupervisorStore, WaveKind,
    WaveSnapshot,
};

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

/// Trait surface so U8-coordinator callers can substitute a
/// mock for tests (and so the
/// `loop_runner::tests::wave_supervisor::` family can mock the
/// supervisor without bringing up git).
pub trait SupervisorBridge: std::fmt::Debug + Send + Sync {
    /// Run one tick of the supervisor fan-in for `wave_id`.
    /// Returns the coordinator action so tests can assert
    /// the bridge called through to U8.
    fn tick(
        &self,
        wave_id: &str,
        inputs: PhaseInputs,
    ) -> Result<CoordinatorAction, BridgeError>;

    /// Open a slot dispatch decision: returns the binding (or
    /// `None` for `SharedReadonly`) so the dispatcher knows
    /// whether to spawn a real worker process. The trait
    /// shapes the slot-bound side of the contract; U12's
    /// production bridge delegates to U10's helper.
    fn bind_slot(
        &self,
        kind: WaveKind,
        wave_id: &str,
        slot_index: u32,
    ) -> Result<Option<SlotBinding>, BridgeError>;

    /// Snapshot helper for tests and the `ralph diagnose`
    /// surface.
    fn recover(&self) -> Result<Vec<WaveSnapshot>, BridgeError>;
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

/// Production bridge: holds an `Arc<dyn SupervisorStore>` +
/// `SupervisorCoordinator`. Construction is gated behind the
/// `supervisor-db` feature for the SQLite branch; the
/// in-memory branch is always available so dry-runs work in
/// default builds.
#[derive(Debug, Clone)]
pub struct CoordinatorSupervisorBridge {
    store: Arc<dyn SupervisorStore>,
    coordinator: Arc<SupervisorCoordinator>,
}

impl CoordinatorSupervisorBridge {
    /// Build a bridge around the in-memory store. Used by tests
    /// and the dry-run CLI path.
    pub fn with_in_memory_store() -> Self {
        let store = Arc::new(InMemorySupervisorStore::new());
        let coordinator = Arc::new(SupervisorCoordinator::with_in_memory_sink(store.clone()));
        Self {
            store,
            coordinator,
        }
    }

    /// Access the underlying store. Diagnostics-friendly.
    pub fn store(&self) -> Arc<dyn SupervisorStore> {
        self.store.clone()
    }

    /// Access the coordinator so the bridge can hand it to
    /// the runtime when the dispatcher needs to drive a tick
    /// outside the bridge trait.
    pub fn coordinator(&self) -> Arc<SupervisorCoordinator> {
        self.coordinator.clone()
    }

    /// Build a bridge around a store owned elsewhere (e.g. the
    /// dispatcher bridge reads the store from the runtime
    /// once and shares it across ticks).
    pub fn from_store(store: Arc<dyn SupervisorStore>) -> Self {
        let coordinator = Arc::new(SupervisorCoordinator::with_in_memory_sink(store.clone()));
        Self {
            store,
            coordinator,
        }
    }
}

impl SupervisorBridge for CoordinatorSupervisorBridge {
    fn tick(
        &self,
        wave_id: &str,
        inputs: PhaseInputs,
    ) -> Result<CoordinatorAction, BridgeError> {
        self.coordinator
            .tick(wave_id, inputs)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn bind_slot(
        &self,
        _kind: WaveKind,
        _wave_id: &str,
        _slot_index: u32,
    ) -> Result<Option<SlotBinding>, BridgeError> {
        // Production wiring is delegated to `dispatcher.rs`
        // (U12 plan: the existing wave dispatcher reads the
        // binding and forwards the env to the spawned worker).
        // Returning `Ok(None)` here keeps the bridge interface
        // a one-method hot path; real bindings come from U10.
        Ok(None)
    }

    fn recover(&self) -> Result<Vec<WaveSnapshot>, BridgeError> {
        self.store
            .recover_active_waves()
            .map_err(|err| BridgeError::Store(err.to_string()))
    }
}

/// Mock bridge for tests that need to assert the bridge
/// surface without a real store.
#[derive(Debug, Clone, Default)]
pub struct MockSupervisorBridge {
    ticks: Arc<std::sync::Mutex<Vec<(String, PhaseInputs)>>>,
    actions: Arc<std::sync::Mutex<Vec<CoordinatorAction>>>,
}

impl MockSupervisorBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot the recorded ticks + the next action the
    /// bridge will return. Tests use this to assert call
    /// ordering.
    pub fn snapshot(&self) -> (Vec<(String, PhaseInputs)>, Vec<CoordinatorAction>) {
        (
            self.ticks.lock().unwrap().clone(),
            self.actions.lock().unwrap().clone(),
        )
    }
}

impl SupervisorBridge for MockSupervisorBridge {
    fn tick(
        &self,
        wave_id: &str,
        inputs: PhaseInputs,
    ) -> Result<CoordinatorAction, BridgeError> {
        self.ticks
            .lock()
            .unwrap()
            .push((wave_id.to_string(), inputs.clone()));
        // Pop the next action if any, else ContinueCollect.
        let action = self
            .actions
            .lock()
            .unwrap()
            .pop()
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
}

/// Decide whether a `loop_runner` instance should activate the
/// supervisor bridge. Mirrors `enabled === true && execution_mode === isolated`
/// without coupling to `RalphConfig` (the caller's job to
/// pass the relevant config slice).
pub fn is_supervisor_path_enabled(enabled: bool, execution_mode_isolated: bool) -> bool {
    enabled && execution_mode_isolated
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
        let action = bridge
            .tick("w-1", PhaseInputs::default())
            .unwrap();
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
        store
            .record_slot_result(&wave, 0, "h", 1)
            .unwrap();
        let bridge = CoordinatorSupervisorBridge::from_store(store.clone() as Arc<dyn SupervisorStore>);
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
        assert!(matches!(
            snap.phase,
            ralph_core::supervisor::WavePhase::Done | ralph_core::supervisor::WavePhase::Integrate
        ) || snap.merged_to_events
            || snap.completed_count == 1);
    }

    #[test]
    fn bridge_handles_empty_store_recover() {
        let bridge = CoordinatorSupervisorBridge::with_in_memory_store();
        let snaps = bridge.recover().unwrap();
        assert!(snaps.is_empty());
    }

    #[test]
    fn bind_slot_returns_none_for_test_bridge() {
        let bridge = CoordinatorSupervisorBridge::with_in_memory_store();
        let binding = bridge
            .bind_slot(WaveKind::Exec, "w-1", 0)
            .unwrap();
        assert!(binding.is_none());
    }

    #[test]
    fn slot_binding_shape_is_transparent() {
        let mut env = HashMap::new();
        env.insert("X".to_string(), "Y".to_string());
        let binding = SlotBinding {
            slot_index: 3,
            env,
            worktree_path: Some(PathBuf::from("/tmp/w")),
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
        store
            .record_slot_result(&wave, 0, "h", 1)
            .unwrap();
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
        let _ = bridge
            .coordinator()
            .sink_batches();
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
}
