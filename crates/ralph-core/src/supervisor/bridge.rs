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

use crate::supervisor::{CoordinatorAction, WaveKind, WaveSnapshot};
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
    /// Run one tick of the supervisor fan-in for `wave_id`.
    /// Returns the coordinator action so the dispatcher can
    /// decide whether to merge worker events + persist the
    /// `*.wave.complete` / `*.wave.failed` coordination event.
    fn tick(&self, wave_id: &str, inputs: PhaseInputs) -> Result<CoordinatorAction, BridgeError>;

    /// Global worker cap exposed to the dispatcher. Bridges that do not
    /// provide store-backed dispatch approval retain the legacy unlimited
    /// behavior.
    fn max_concurrent_workers(&self) -> u32 {
        u32::MAX
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
}

/// BDD-specific bridge that wires a `SupervisorCoordinator`
/// directly to an in-memory or rusqlite `SupervisorStore`
/// without pulling in `ralph-cli`'s worker-spawn path. Used by
/// `crates/ralph-core/tests/scenarios.rs` so the
/// `ce_executor_supervisor_minimal` scenario exercises the real
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
    fn tick(&self, wave_id: &str, inputs: PhaseInputs) -> Result<CoordinatorAction, BridgeError> {
        self.coordinator
            .tick(wave_id, inputs)
            .map_err(|err| BridgeError::Store(err.to_string()))
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

    fn register_wave_if_absent(
        &self,
        kind: WaveKind,
        wave_id: &str,
        expected_total: u32,
    ) -> Result<String, BridgeError> {
        let mut guard = self.registered.lock().unwrap();
        if let Some(existing) = guard.get(wave_id) {
            return Ok(existing.clone());
        }
        // The store returns `DuplicateKey` if the wave was
        // already registered (e.g. by a previous loop
        // iteration); treat that as the idempotent success
        // the trait name promises. We cannot recover the
        // store-assigned id from the DuplicateKey error, so
        // the dispatcher MUST reuse the same idempotency key
        // across ticks (which it does — it's the wave_id).
        let store_id = match self.store.register_wave(wave_id, kind, expected_total) {
            Ok(id) => id,
            Err(SupervisorStoreError::DuplicateKey(_)) => wave_id.to_string(),
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
        outcome: crate::supervisor::DispatchOutcome,
    ) -> Result<(), BridgeError> {
        self.store
            .release_slot_dispatch(wave_id, slot_index, outcome)?;
        Ok(())
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
            .register_wave_if_absent(WaveKind::Exec, "bdd-wave", 1)
            .unwrap();
        // Second call must be a no-op (returns the same store id).
        let store_id_again = bridge
            .register_wave_if_absent(WaveKind::Exec, "bdd-wave", 1)
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
            .register_wave_if_absent(WaveKind::Exec, "bdd-wave", 1)
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
