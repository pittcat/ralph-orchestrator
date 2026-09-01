//! U9 / fix-plan U9: `tests/wave_supervisor.rs` — pin the
//! supervisor bridge hot-path contract at the loop_runner
//! test integration level.
//!
//! Why this file exists (fix-plan F-009 / U12 delivery side):
//! the previous supervisor plan wired the bridge types but
//! never connected them to the wave dispatcher. This file
//! locks in three named invariants so a future regression
//! (e.g. accidental reversion of the dispatcher branch or
//! dropping the bridge trait object on the floor) is caught
//! by nextest:
//!
//! - `enabled_false_uses_wave_tracker` — when the operator
//!   omits the `event_loop.supervisor` block (or sets
//!   `enabled: false`), the dispatcher path must take the
//!   legacy `WaveTracker` shape and the bridge trait object
//!   must remain `None`.
//! - `enabled_true_calls_bridge_bind_slot` — when the
//!   operator opts in (`event_loop.supervisor.enabled = true`
//!   + `execution_mode: isolated`), the dispatcher must
//!   invoke `SupervisorBridge::bind_slot` once per slot and
//!   forward the `SlotBinding::env` map to the worker
//!   `Command::envs(...)`. This test asserts the call
//!   ordering + env keys via a `MockSupervisorBridge` spy
//!   that records the bound slots.
//! - `bridge_off_no_feature_returns_error_path` — when the
//!   `supervisor-db` feature is off and the operator still
//!   opts in (`event_loop.supervisor.enabled = true`), the
//!   bridge must surface `BridgeError::Disabled` (NOT panic)
//!   so callers can decide to fall back to `WaveTracker`.
//!
//! The tests are intentionally architected around
//! `MockSupervisorBridge` + the existing public bridge
//! surface (`bind_slot`, `tick`, `recover`) so they exercise
//! the production trait without spawning `git worktree add`
//! or a real `RusqliteSupervisorStore`. The
//! `bridge_off_no_feature_returns_error_path` scenario uses
//! the production `CoordinatorSupervisorBridge` with an
//! in-memory store (which compiles cleanly without the
//! `supervisor-db` feature gate).

use super::super::*;
use crate::loop_runner::wave::{BridgeError, SlotBinding, SupervisorBridge, WaveWorkerExecutor};
use ralph_core::supervisor::{PhaseInputs, WaveKind, WaveSnapshot};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub(super) struct SpyBindingBridge {
    pub(super) bind_calls: Mutex<Vec<(WaveKind, String, u32)>>,
    pub(super) bindings: Mutex<Vec<SlotBinding>>,
}

impl SpyBindingBridge {
    pub(super) fn new() -> Self {
        Self::default()
    }
    pub(super) fn record(&self, binding: SlotBinding) {
        self.bindings.lock().unwrap().push(binding);
    }
}

impl std::fmt::Debug for SpyBindingBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpyBindingBridge").finish()
    }
}

impl SupervisorBridge for SpyBindingBridge {
    fn tick(
        &self,
        _wave_id: &str,
        _inputs: PhaseInputs,
    ) -> Result<ralph_core::supervisor::CoordinatorAction, BridgeError> {
        Ok(ralph_core::supervisor::CoordinatorAction::ContinueCollect)
    }

    fn bind_slot(
        &self,
        kind: WaveKind,
        wave_id: &str,
        slot_index: u32,
    ) -> Result<Option<SlotBinding>, BridgeError> {
        self.bind_calls
            .lock()
            .unwrap()
            .push((kind, wave_id.to_string(), slot_index));
        let mut env = HashMap::new();
        env.insert("RALPH_WAVE_WORKER".to_string(), "1".to_string());
        env.insert(
            "RALPH_WAVE_WORKTREE_PATH".to_string(),
            format!("/tmp/u9-spy/{wave_id}-{slot_index}"),
        );
        env.insert("RALPH_WAVE_ID".to_string(), wave_id.to_string());
        env.insert("RALPH_WAVE_INDEX".to_string(), slot_index.to_string());
        env.insert("RALPH_WAVE_KIND".to_string(), kind.to_string());
        let binding = SlotBinding {
            slot_index,
            env,
            worktree_path: Some(format!("/tmp/u9-spy/{wave_id}-{slot_index}").into()),
        };
        self.record(binding.clone());
        Ok(Some(binding))
    }

    fn recover(&self) -> Result<Vec<ralph_core::supervisor::WaveSnapshot>, BridgeError> {
        Ok(Vec::new())
    }

    fn fan_in_status(&self, _wave_id: &str) -> Result<WaveSnapshot, BridgeError> {
        Err(BridgeError::Store(
            "SpyBindingBridge has no store".to_string(),
        ))
    }

    // 2026-07-03-001 supervisor real-wiring: the trait gained
    // three new methods. The spy records nothing for them —
    // existing tests only assert `bind_slot` + `tick`. New
    // tests (see supervisor integration tests in this file)
    // exercise the real path through `InMemorySupervisorStore`.
    fn register_wave_if_absent(
        &self,
        _kind: WaveKind,
        wave_id: &str,
        _expected_total: u32,
        _slot_retry_budget: u32,
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

    fn release_slot_dispatch(
        &self,
        _wave_id: &str,
        _slot_index: u32,
        _outcome: ralph_core::supervisor::DispatchOutcome,
    ) -> Result<(), BridgeError> {
        Ok(())
    }

    /// Spy: 0 so the spy doesn't auto-retry; tests that
    /// exercise the retry path use other bridges.
    fn slot_retry_budget(&self) -> u32 {
        0
    }
}

// ── 2026-07-22-003 plan U4: production per-slot worktree binding ─────────────
//
// Goal: the production `CoordinatorSupervisorBridge::bind_slot` MUST actually
// invoke the worktree helper, persist the `SlotResource` to the store, and
// return a `SlotBinding` with the per-slot cwd/env. Exec/Fix bindings
// MUST NOT return `None` — `None` is reserved for the `Review`
// (SharedReadonly) branch only. `bind_slot` failures MUST be
// surfaced as typed `BridgeError` (not silently swallowed) and the
// dispatcher MUST translate them into fail-closed behaviour (no
// worker spawned against the main workspace).
//
// The tests below pin four contracts:
//   1. `exec_kind_produces_unique_branch_path_cwd`: two exec
//      slots through the same wave produce two distinct
//      `(branch, worktree_path)` pairs and the `WorkerRequest`
//      `cwd` matches the binding.
//   2. `fix_kind_produces_unique_branch_path_cwd`: same for
//      `WaveKind::Fix`.
//   3. `review_kind_returns_shared_readonly_none`: `Review`
//      bindings remain `None` (no worktree, no writeable branch)
//      and the binding's `worktree_path` is `None`.
//   4. `bind_slot_failure_fail_closed_no_main_workspace_write`:
//      when the `WorktreeFactory` returns an error, the bridge
//      records the failure in the store and returns `Err`, so
//      the dispatcher's fail-closed branch keeps the slot out
//      of the worktree queue and the main workspace never
//      receives a worker spawn with the loop's cwd.
//
// All tests use a `RecordingFactory` (records every call into a
// shared `Vec`) — the production bridge constructs a
// `DefaultWorktreeFactory` for real workers and only the tests
// inject the recording factory.

use ralph_core::supervisor::worktree_bind::{DefaultWorktreeFactory, WorktreeFactory};
use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore};
use ralph_core::worktree::Worktree;

#[derive(Debug, Clone)]
pub(super) struct RecordingFactory {
    /// Existing `WorktreeBinding`s created by this factory — the
    /// bridge hands them back with a synthetic absolute path so
    /// tests don't need a real git repo.
    pub(super) calls: std::sync::Arc<std::sync::Mutex<Vec<(std::path::PathBuf, String)>>>,
    /// Branch → worktree path; tests pre-populate the table to
    /// simulate a successful factory call.
    pub(super) paths:
        std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, std::path::PathBuf>>>,
}

impl Default for RecordingFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingFactory {
    pub(super) fn new() -> Self {
        Self {
            calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            paths: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub(super) fn pre_create(&self, branch: &str, path: std::path::PathBuf) {
        self.paths.lock().unwrap().insert(branch.to_string(), path);
    }

    pub(super) fn calls_snapshot(&self) -> Vec<(std::path::PathBuf, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl WorktreeFactory for RecordingFactory {
    fn create(
        &self,
        repo_root: std::path::PathBuf,
        branch: String,
    ) -> Result<Worktree, ralph_core::supervisor::worktree_bind::WorktreeError> {
        self.calls.lock().unwrap().push((repo_root, branch.clone()));
        let path = self
            .paths
            .lock()
            .unwrap()
            .get(&branch)
            .cloned()
            .ok_or_else(|| {
                ralph_core::supervisor::worktree_bind::WorktreeError::CreateFailed(format!(
                    "RecordingFactory: no path for branch {branch}"
                ))
            })?;
        Ok(Worktree {
            path,
            branch,
            is_main: false,
            head: None,
        })
    }
}

#[derive(Debug)]
pub(super) struct FailingFactory;

impl WorktreeFactory for FailingFactory {
    fn create(
        &self,
        _repo_root: std::path::PathBuf,
        _branch: String,
    ) -> Result<Worktree, ralph_core::supervisor::worktree_bind::WorktreeError> {
        Err(
            ralph_core::supervisor::worktree_bind::WorktreeError::CreateFailed(
                "factory failed: simulating U4 worktree creation failure".to_string(),
            ),
        )
    }
}

/// Build a `CoordinatorSupervisorBridge` whose `bind_slot` calls
/// run through `factory` instead of the production
/// `DefaultWorktreeFactory`. The bridge still owns the in-memory
/// store + coordinator so the `bind_worktree` / `record_slot_*`
/// paths stay live for assertions.
pub(super) fn production_bridge_with_factory(
    factory: std::sync::Arc<dyn WorktreeFactory>,
    repo_root: std::path::PathBuf,
    loop_id: &str,
) -> (
    crate::loop_runner::wave::CoordinatorSupervisorBridge,
    std::sync::Arc<dyn SupervisorStore>,
) {
    use crate::loop_runner::wave::ProductionBridgeContext;
    use ralph_core::LoopContext;

    let loop_ctx = LoopContext::worktree(loop_id.to_string(), repo_root.clone(), repo_root);
    let context = ProductionBridgeContext {
        loop_id: loop_id.to_string(),
        repo_root: loop_ctx.repo_root().to_path_buf(),
        events_path: None,
        tasks_path: None,
    };
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = CoordinatorSupervisorBridge::with_context_and_factory(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        context,
        factory,
    );
    (bridge, store)
}

// =============================================================================
// U3 KTD-1..KTD-5: dispatcher awaits store approval before spawn.
//
// The supervisor path (execute_wave_via_supervisor) gates every
// slot on `bridge.try_dispatch_next(wave_id, slot_index)`. The
// store may approve or withhold each call; the dispatcher MUST
// only push a WorkerRequest for the slot when the bridge returns
// `Ok(true)`. When the bridge returns `Ok(false)` the slot is
// skipped (no spawn); when `Err` the dispatcher fails closed and
// does not spawn.
//
// The local effective cap is `min(hat.concurrency,
// bridge.max_concurrent_workers())` — the dispatcher tracks how
// many slots it has approved in this wave and stops pushing
// WorkerRequests once the cap is reached. The store-side cap
// still applies (any subsequent `try_dispatch_next` returns
// `Ok(false)` naturally), but the dispatcher pre-truncates so
// the test signal is deterministic.
//
// The tests below use a `U3DispatchBridge` (spy) that owns an
// `InMemorySupervisorStore` and decides which slots are
// approved via a scripted `Outcome` set. The bridge surface
// mirrors `SupervisorBridge::try_dispatch_next` exactly: when
// the store approves `(wave_id, slot_index)` (i.e. the store's
// `try_dispatch_next(max_workers)` returns that exact pair),
// the bridge returns `Ok(true)`; otherwise `Ok(false)`. When
// the spy is configured with `dispatch_outcome = Err(...)`
// the bridge surfaces the error; the dispatcher MUST propagate
// it without spawning a worker for the failing slot.
//
// We deliberately drive the supervisor path directly through
// `production_bridge_with_factory` rather than the
// `MockSupervisorBridge` (which always returns `Ok(None)` from
// `bind_slot`). The supervisor path's per-slot `bind_slot`
// must succeed so the WorkerRequest.cwd is non-None and the
// spawn attempt is real; only the dispatch-approval gate is
// under the test's control.
// =============================================================================

/// Spy bridge that exposes a controllable `try_dispatch_next`
/// outcome while delegating `bind_slot` to a real
/// `CoordinatorSupervisorBridge`-style path against an
/// `InMemorySupervisorStore` + `RecordingFactory`. The bridge
/// records every `try_dispatch_next` call so tests can assert
/// the dispatcher's call ordering.
#[derive(Debug, Clone)]
pub(super) struct U3DispatchBridge {
    pub(super) store: std::sync::Arc<dyn SupervisorStore>,
    /// Hard max concurrent workers — the trait surface the
    /// dispatcher multiplies against `hat.concurrency`.
    pub(super) max_concurrent_workers: u32,
    /// Recorded `(wave_id, slot_index)` calls. Tests use the
    /// snapshot to confirm the dispatcher queried the bridge
    /// once per slot (and not fewer / not more).
    pub(super) dispatch_calls: std::sync::Arc<std::sync::Mutex<Vec<(String, u32)>>>,
    /// When `Some(Err(_))`, the bridge surfaces that error
    /// from `try_dispatch_next` regardless of store state. Used
    /// by the fail-closed-on-error test.
    pub(super) override_outcome: std::sync::Arc<std::sync::Mutex<Option<DispatchOverride>>>,
}

#[derive(Debug, Clone)]
pub(super) enum DispatchOverride {
    /// Force every `try_dispatch_next` call to return `Ok(false)`
    /// (the store has nothing pending). The dispatcher MUST skip
    /// every slot.
    AlwaysDeny,
    /// Force every `try_dispatch_next` call to return
    /// `Err(BridgeError::Store(_))`. The dispatcher MUST fail
    /// closed.
    AlwaysError(String),
}

impl U3DispatchBridge {
    pub(super) fn new(
        store: std::sync::Arc<dyn SupervisorStore>,
        max_concurrent_workers: u32,
    ) -> Self {
        Self {
            store,
            max_concurrent_workers,
            dispatch_calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            override_outcome: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub(super) fn dispatch_calls_snapshot(&self) -> Vec<(String, u32)> {
        self.dispatch_calls.lock().unwrap().clone()
    }

    pub(super) fn set_override(&self, override_outcome: Option<DispatchOverride>) {
        *self.override_outcome.lock().unwrap() = override_outcome;
    }

    /// 2026-07-28-002 plan U3 (S2a): pre-bind specific slots in the
    /// store so `try_dispatch_next` returns them as dispatchable.
    /// Needed when the dispatcher must bind slots that the test
    /// pre-approves (S2a happy path).
    #[allow(dead_code)]
    pub(super) fn pre_bind_slots(&self, wave_id: &str, slots: &[u32]) {
        use ralph_core::supervisor::SlotResource;
        for &slot_index in slots {
            let resource = SlotResource {
                slot_index,
                worktree_path: Some(format!("/tmp/u3-spy/{wave_id}-{slot_index}")),
                branch: Some(format!("u3-{wave_id}-{slot_index}")),
            };
            // Best-effort: ignore errors (slot might not exist yet).
            let _ = self.store.bind_worktree(wave_id, slot_index, resource);
        }
    }

    #[allow(dead_code)]
    pub(super) fn store(&self) -> std::sync::Arc<dyn SupervisorStore> {
        self.store.clone()
    }

    #[allow(dead_code)]
    pub(super) fn max_concurrent_workers(&self) -> u32 {
        self.max_concurrent_workers
    }
}

impl SupervisorBridge for U3DispatchBridge {
    fn store(&self) -> Option<std::sync::Arc<dyn SupervisorStore>> {
        Some(self.store.clone())
    }

    fn tick(
        &self,
        _wave_id: &str,
        _inputs: PhaseInputs,
    ) -> Result<ralph_core::supervisor::CoordinatorAction, BridgeError> {
        Ok(ralph_core::supervisor::CoordinatorAction::ContinueCollect)
    }

    fn max_concurrent_workers(&self) -> u32 {
        self.max_concurrent_workers
    }

    // 2026-07-28-003 plan U5 (R11): the U3 characterization
    // helpers preserve the pre-U5 "no retry" semantics by
    // overriding the trait default budget (`1`) to `0`. This
    // keeps `test_dispatcher_effective_cap_*` green without
    // having to special-case the spawn counter.
    fn slot_retry_budget(&self) -> u32 {
        0
    }

    fn try_dispatch_next(&self, wave_id: &str, slot_index: u32) -> Result<bool, BridgeError> {
        self.dispatch_calls
            .lock()
            .unwrap()
            .push((wave_id.to_string(), slot_index));
        if let Some(override_outcome) = self.override_outcome.lock().unwrap().clone() {
            return match override_outcome {
                DispatchOverride::AlwaysDeny => Ok(false),
                DispatchOverride::AlwaysError(msg) => Err(BridgeError::Store(msg)),
            };
        }
        // Drive the store's own dispatch approval. The store's
        // `try_dispatch_next(max)` returns the next pending slot
        // (if any), bumping `active_workers` and the slot's
        // status to `Dispatched`. We then check whether the
        // returned `(wave_id, slot_index)` matches what the
        // dispatcher requested.
        let dispatched = self
            .store
            .try_dispatch_next(self.max_concurrent_workers)
            .map_err(|err| BridgeError::Store(err.to_string()))?;
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
        // Return a real binding so the dispatcher can build a
        // WorkerRequest. The U3 scenarios below control the store's
        // "approvable slot set" explicitly — either by pre-binding
        // the slots they want approved (KTD-2) or by registering
        // another wave whose bound slots sort before ours (KTD-3).
        // The spy must NOT call `store.bind_worktree` here: doing so
        // makes the dispatcher's own per-slot bind auto-populate the
        // store, which collapses every scenario into "all slots
        // dispatchable" and defeats the approval gate under test.
        //
        // The production `CoordinatorSupervisorBridge::bind_slot`
        // DOES persist the binding (fan-in / R7 rely on it); tests
        // that need production persistence use that bridge directly.
        let mut env = HashMap::new();
        env.insert("RALPH_WAVE_WORKER".to_string(), "1".to_string());
        env.insert(
            "RALPH_WAVE_WORKTREE_PATH".to_string(),
            format!("/tmp/u3-spy/{wave_id}-{slot_index}"),
        );
        env.insert("RALPH_WAVE_ID".to_string(), wave_id.to_string());
        env.insert("RALPH_WAVE_INDEX".to_string(), slot_index.to_string());
        env.insert("RALPH_WAVE_KIND".to_string(), format!("{kind:?}"));
        Ok(Some(SlotBinding {
            slot_index,
            env,
            worktree_path: Some(format!("/tmp/u3-spy/{wave_id}-{slot_index}").into()),
        }))
    }

    fn recover(&self) -> Result<Vec<ralph_core::supervisor::WaveSnapshot>, BridgeError> {
        Ok(Vec::new())
    }

    fn fan_in_status(&self, wave_id: &str) -> Result<WaveSnapshot, BridgeError> {
        // 2026-07-28-002 plan U4 (S6): the pre-registered redrive path
        // verifies the wave row via `fan_in_status`; delegate to the
        // real store so pre-registered child waves pass the existence
        // check instead of failing closed.
        self.store
            .fan_in_status(wave_id)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn register_wave_if_absent(
        &self,
        kind: WaveKind,
        wave_id: &str,
        expected_total: u32,
        slot_retry_budget: u32,
    ) -> Result<String, BridgeError> {
        // Register the wave in the store so subsequent
        // `bind_worktree` calls succeed. Return the STORE's
        // allocated id (`w-{seq}`) so the dispatcher's
        // subsequent `bind_slot(wave_id, ...)` calls line up
        // with the store's `waves_by_id` keys.
        use ralph_core::supervisor::SupervisorStoreError;
        match self
            .store
            .register_wave(wave_id, kind, expected_total, slot_retry_budget)
        {
            Ok(store_wave_id) => Ok(store_wave_id),
            Err(SupervisorStoreError::DuplicateKey(_)) => {
                // Idempotent re-entry: the wave is already
                // registered under the caller's idempotency
                // key. Recover the store's id by walking active
                // waves; for the test scenarios we only have
                // one wave, so the first snapshot is the right
                // one.
                let mut snapshots = self
                    .store
                    .recover_active_waves()
                    .map_err(|err| BridgeError::Store(err.to_string()))?;
                let snapshot = snapshots.pop().ok_or_else(|| {
                    BridgeError::Store(format!(
                        "register_wave_if_absent: duplicate key {wave_id} but no recovered wave"
                    ))
                })?;
                Ok(snapshot.wave_id)
            }
            Err(err) => Err(BridgeError::Store(err.to_string())),
        }
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

    fn release_slot_dispatch(
        &self,
        wave_id: &str,
        slot_index: u32,
        outcome: ralph_core::supervisor::DispatchOutcome,
    ) -> Result<(), BridgeError> {
        self.store
            .release_slot_dispatch(wave_id, slot_index, outcome)
            .map_err(|error| BridgeError::Store(error.to_string()))
    }
}

/// 2026-07-28-002 plan U3 (S2a): like `run_u3_execute_wave` but
/// pre-binds the given slots in the store before dispatch, so
/// `try_dispatch_next` returns them as dispatchable.
pub(super) async fn run_u3_execute_wave_with_prebound_slots(
    bridge: &U3DispatchBridge,
    wave: &ralph_core::DetectedWave,
    prebound_slots: &[u32],
    started: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> (
    WaveDispatchOutcome,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    use crate::loop_runner::wave::execute_wave_via_supervisor_with_executor;

    // Pre-register the wave so we get the store's wave_id,
    // then pre-bind slots using that id.
    let store_wave_id = {
        let registered = bridge
            .register_wave_if_absent(
                WaveKind::Exec,
                &wave.wave_id,
                wave.total,
                0, // slot_retry_budget
            )
            .expect("register_wave_if_absent must succeed");
        bridge.pre_bind_slots(&registered, prebound_slots);
        registered
    };
    let _ = store_wave_id; // suppress unused warning

    let wave_dir =
        std::env::temp_dir().join(format!("u3-disp-{}-{}", wave.wave_id, std::process::id()));
    let _ = std::fs::remove_dir_all(&wave_dir);
    let _ = std::fs::create_dir_all(&wave_dir);
    let main_events_file = wave_dir.join("events.jsonl");
    let _ = std::fs::File::create(&main_events_file);

    let executor = std::sync::Arc::new(U3CountingExecutor::new(started.clone()));
    let executor_dyn: std::sync::Arc<dyn WaveWorkerExecutor> = executor as _;
    let bridge_arc: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge.clone());
    let outcome = execute_wave_via_supervisor_with_executor(
        wave,
        &make_test_cli_backend(),
        &main_events_file,
        false,
        false,
        None,
        None,
        "u3-test-loop",
        WaveDispatchLimits::default(),
        None,
        None,
        &bridge_arc,
        executor_dyn,
        None, // pre_registered_id: not pre-registered in test path
        None, // slot_index_override: test path uses events-array position
    )
    .await;
    (outcome, started)
}

/// Drive `execute_wave_via_supervisor_with_executor` with a
/// `U3DispatchBridge` and an injected `U3CountingExecutor`. The
/// helper returns the dispatch outcome and the `started`
/// counter so the test can assert how many slots actually
/// spawned a worker.
///
/// The test goes through the SUPERVISOR path's hot loop (the
/// one inside `execute_wave_via_supervisor_with_executor`),
/// not the legacy `WaveTracker` path, so it pins the gate under
/// test rather than a replica.
pub(super) async fn run_u3_execute_wave(
    bridge: U3DispatchBridge,
    wave: ralph_core::DetectedWave,
    started: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> (
    WaveDispatchOutcome,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    use crate::loop_runner::wave::execute_wave_via_supervisor_with_executor;

    let wave_dir =
        std::env::temp_dir().join(format!("u3-disp-{}-{}", wave.wave_id, std::process::id()));
    let _ = std::fs::remove_dir_all(&wave_dir);
    let _ = std::fs::create_dir_all(&wave_dir);
    let main_events_file = wave_dir.join("events.jsonl");
    let _ = std::fs::File::create(&main_events_file);

    let executor = std::sync::Arc::new(U3CountingExecutor::new(started.clone()));
    let executor_dyn: std::sync::Arc<dyn WaveWorkerExecutor> = executor as _;
    let bridge_arc: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge);
    let outcome = execute_wave_via_supervisor_with_executor(
        &wave,
        &make_test_cli_backend(),
        &main_events_file,
        false,
        false,
        None,
        None,
        "u3-test-loop",
        WaveDispatchLimits::default(),
        None,
        None,
        &bridge_arc,
        executor_dyn,
        None, // pre_registered_id: not pre-registered in test path
        None, // slot_index_override: test path uses events-array position
    )
    .await;
    (outcome, started)
}

/// Build a `CliBackend` that the dispatcher can pass to
/// `WorkerRequest` without spawning a real process. The
/// executor in `U3CountingExecutor` never invokes the
/// backend, so a sentinel value is sufficient.
pub(super) fn make_test_cli_backend() -> CliBackend {
    CliBackend {
        command: "echo".to_string(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format: ralph_adapters::OutputFormat::Text,
        env_vars: vec![],
    }
}

/// Minimal executor that increments a `started` counter on
/// every `execute()` call, then sleeps briefly and returns a
/// successful outcome. Success decouples the test from
/// `dispatch_wave_inner`'s deadline / abort logic so the
/// `started` count is stable.
pub(super) struct U3CountingExecutor {
    pub(super) started: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Topic of the single event this executor emits. Defaults to
    /// `review.done` (NO `.unit.done` / `.wave.done` terminal marker)
    /// to preserve the historical U3 fixture shape. Tests that need
    /// the slot to classify as `Completed` — e.g. the S3 redrive
    /// boot-scan test, whose "exactly one worker" assertion must not
    /// be inflated by the 2026-07-28-003 plan U5 slot auto-retry loop
    /// (a missing terminal classifies as the retryable
    /// `missing_worker_terminal` frozen code) — override this with a
    /// `*.unit.done` topic via `with_topic`.
    pub(super) topic: String,
}

impl U3CountingExecutor {
    pub(super) fn new(started: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Self {
        Self {
            started,
            topic: "review.done".to_string(),
        }
    }

    pub(super) fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = topic.into();
        self
    }
}

impl WaveWorkerExecutor for U3CountingExecutor {
    fn execute(
        &self,
        mut request: crate::loop_runner::wave::WorkerRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (u32, WaveWorkerOutcome)> + Send>> {
        let started = std::sync::Arc::clone(&self.started);
        let topic = self.topic.clone();
        Box::pin(async move {
            started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = request.worker_rpc_tx.take();
            let _ = request.worker_tui_state.take();
            let event = ralph_core::Event {
                topic,
                payload: Some("ok".to_string()),
                ts: String::new(),
                hat: None,
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            };
            (
                request.index,
                Ok((vec![event], Duration::from_millis(10), true, None)),
            )
        })
    }
}

// =============================================================================
// U3 test helpers (subordinate to the U3 tests above).
// =============================================================================

// U1 M1: partial failure bridge setup helper used by the U1
// characterization tests (Test 1, 2, 3 and the new T1-T4 tests).
// Follows the same pattern as `make_u3_wave` / `make_u3_wave_with_concurrency`
// to stay consistent with the existing helper style.

/// Build a production `CoordinatorSupervisorBridge` for a partial-failure
/// scenario with `slot_count` slots, all bound and dispatched but NOT
/// yet recorded (callers record completed/failed per slot before fan-in).
///
/// Returns `(tmp, bridge, store_wave_id, events_path)` where:
///   - `tmp` is the temp directory (kept alive for the duration of the test)
///   - `bridge` is a production bridge ready for `record_slot_result` /
///     `record_slot_failure` calls
///   - `store_wave_id` is the store-assigned wave id for subsequent calls
///   - `events_path` is the ledger path for `run_supervisor_fan_in`
///
/// Usage:
///   let (tmp, bridge, store_wave_id, events_path) =
///       setup_u3_partial_failure_bridge(WaveKind::Exec, "u1-test", 2);
///   // Record slot 0 as completed, slot 1 as failed
///   bridge.record_slot_result(&store_wave_id, 0, "h0", 1).unwrap();
///   bridge.store().record_slot_terminal_evidence(...).unwrap();
///   bridge.record_slot_failure(&store_wave_id, 1, REASON_WORKER_TIMEOUT).unwrap();
///   bridge.commit_salvage_projection(
///       &store_wave_id,
///       &ralph_core::supervisor::ProjectionReceiptSummary {
///           kind: ralph_core::supervisor::ProjectionKind::Business,
///           batch_fingerprint: "test-fp".into(),
///           write_count: 0,
///           already_present_count: 0,
///           committed_at_unix_secs: 0,
///       },
///   ).unwrap();
///   // Now run fan-in
pub(super) fn setup_u3_partial_failure_bridge(
    kind: WaveKind,
    loop_id: &str,
    slot_count: u32,
) -> (
    tempfile::TempDir,
    crate::loop_runner::wave::CoordinatorSupervisorBridge,
    std::sync::Arc<dyn SupervisorStore>,
    String,
    std::path::PathBuf,
) {
    use crate::loop_runner::wave::{CoordinatorSupervisorBridge, ProductionBridgeContext};
    use ralph_core::supervisor::SlotResource;

    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");
    std::fs::create_dir_all(events_path.parent().unwrap()).ok();

    let store: std::sync::Arc<dyn SupervisorStore> =
        std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = ProductionBridgeContext {
        loop_id: loop_id.to_string(),
        repo_root: std::path::PathBuf::from("/tmp/u1-repo"),
        events_path: Some(events_path.clone()),
        tasks_path: None,
    };
    let bridge = CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
        store.clone(),
        context,
        std::sync::Arc::new(DefaultWorktreeFactory),
        slot_count.max(1),
        // 2026-07-28-003 plan U4: explicit budget keeps the
        // characterization test seam at the historical default.
        1,
    );
    let store_wave_id = bridge
        .register_wave_if_absent(kind, loop_id, slot_count, 0)
        .expect("register wave must succeed");

    for i in 0..slot_count {
        bridge
            .store()
            .bind_worktree(
                &store_wave_id,
                i,
                SlotResource {
                    slot_index: i,
                    worktree_path: Some(format!("/tmp/u1-wt/{i}")),
                    branch: Some(format!("u1-{loop_id}-{i}")),
                },
            )
            .expect("bind worktree must succeed");
    }
    for _ in 0..slot_count {
        bridge
            .store()
            .try_dispatch_next(slot_count.max(1))
            .expect("dispatch must succeed")
            .expect("a slot must be dispatchable");
    }

    (tmp, bridge, store, store_wave_id, events_path)
}

/// Build a `DetectedWave` with a single trigger topic and a
/// fixed `(events_count, total, concurrency)`. The wave_id is
/// the caller's `name` so test output is grep-friendly.
pub(super) fn make_u3_wave(name: &str, events_count: u32, total: u32) -> ralph_core::DetectedWave {
    make_u3_wave_with_concurrency(name, events_count, total, events_count)
}

/// Build a `DetectedWave` with a configurable `hat.concurrency`
/// (distinct from `events_count`). Used by the cap tests.
pub(super) fn make_u3_wave_with_concurrency(
    name: &str,
    events_count: u32,
    total: u32,
    concurrency: u32,
) -> ralph_core::DetectedWave {
    make_u3_wave_with_topic(name, events_count, total, concurrency, "exec.unit.ready")
}

/// 2026-07-30-001 plan U1: same as [`make_u3_wave_with_concurrency`]
/// but with an explicit trigger topic, so a test can drive the
/// Review / Fix wave kinds through the same supervisor path.
pub(super) fn make_u3_wave_with_topic(
    name: &str,
    events_count: u32,
    total: u32,
    concurrency: u32,
    trigger_topic: &str,
) -> ralph_core::DetectedWave {
    use ralph_core::DetectedWave;
    use ralph_core::config::HatConfig;

    let events: Vec<ralph_core::Event> = (0..events_count)
        .map(|i| ralph_core::Event {
            topic: trigger_topic.to_string(),
            payload: Some(format!("{{\"unit_id\":\"u3-{name}-{i}\"}}")),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        })
        .collect();
    let hat_config = HatConfig {
        name: format!("u3-hat-{name}"),
        concurrency,
        ..HatConfig::default()
    };
    DetectedWave {
        wave_id: name.to_string(),
        target_hat: ralph_proto::HatId::new(format!("u3-hat-{name}")),
        hat_config,
        events,
        total,
        partial: events_count < total,
        consumer_aggregate_timeout: None,
    }
}

// =============================================================================
// U5 (2026-07-23-001 plan): 登记 slot 成功/失败到 SupervisorStore
//
// The production dispatcher MUST call `bridge.record_slot_result`
// (success) / `bridge.record_slot_failure` (failure) at the structured
// worker-outcome boundary so the supervisor store's `completed_count` /
// `failed_count` reflect the wave's terminal slots (R8). These tests
// drive the real supervisor path (`execute_wave_via_supervisor_with_executor`)
// with a store-backed spy bridge and assert on the store snapshot +
// the captured record calls.
// =============================================================================

/// Per-slot script for the U5 test executor.
#[derive(Clone, Debug)]
pub(super) enum U5SlotOutcome {
    /// Worker succeeds, producing `usize` events.
    Success(usize),
    /// Worker fails with the given reason.
    Fail(String),
    /// 2026-07-28-003 plan U5 (S7 / S12): first attempt follows
    /// `initial`; any subsequent attempt follows `follow_up`.
    ScriptedThen {
        initial: Box<U5SlotOutcome>,
        follow_up: Box<U5SlotOutcome>,
    },
    /// 2026-07-30-001 plan U1: the worker accepted the request and
    /// actively emitted a `<kind>.unit.failed` terminal carrying the
    /// given `reason` detail. The process itself exits successfully —
    /// this is a business failure, not a process failure.
    ReportedFailure {
        terminal_topic: &'static str,
        reason: String,
    },
    /// 2026-07-30-001 plan U1: per-attempt script. Attempt `n` uses
    /// `steps[n - 1]`; attempts beyond the script reuse the last step.
    PerAttempt(Vec<U5SlotOutcome>),
}

/// Executor whose per-slot outcome is scripted by the test. Slots
/// without an explicit entry fall back to `default`.
pub(super) struct U5RecordingExecutor {
    pub(super) plan: std::sync::Arc<std::collections::HashMap<u32, U5SlotOutcome>>,
    pub(super) default: U5SlotOutcome,
    /// Number of times each slot has been executed.
    pub(super) calls: std::sync::Arc<Mutex<std::collections::HashMap<u32, u32>>>,
    /// 2026-07-30-001 plan U2: every prompt each slot was handed, in
    /// attempt order, so a test can assert what a retry was told.
    pub(super) prompts: std::sync::Arc<Mutex<std::collections::HashMap<u32, Vec<String>>>>,
    /// 2026-07-30-001 plan U3: how long each attempt occupies its slot.
    /// Under `start_paused` this consumes wave budget without wall time.
    pub(super) delay: Duration,
}

impl U5RecordingExecutor {
    pub(super) fn new(default: U5SlotOutcome) -> Self {
        Self {
            plan: std::sync::Arc::new(std::collections::HashMap::new()),
            default,
            calls: std::sync::Arc::new(Mutex::new(std::collections::HashMap::new())),
            prompts: std::sync::Arc::new(Mutex::new(std::collections::HashMap::new())),
            delay: Duration::ZERO,
        }
    }

    /// 2026-07-30-001 plan U3: make every attempt take `delay` of wave
    /// budget.
    pub(super) fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub(super) fn with_slot(mut self, index: u32, outcome: U5SlotOutcome) -> Self {
        let map = std::sync::Arc::make_mut(&mut self.plan);
        map.insert(index, outcome);
        self
    }

    /// 2026-07-28-003 plan U5 (S7 / S12): the *first* attempt's
    /// outcome for `index`, followed by `follow_up` for any subsequent
    /// attempt. Tests describe "fail once, then succeed" by setting
    /// initial=Fail(retryable), follow_up=Success(N).
    pub(super) fn with_first_attempt_then(
        mut self,
        index: u32,
        initial: U5SlotOutcome,
        follow_up: U5SlotOutcome,
    ) -> Self {
        let map = std::sync::Arc::make_mut(&mut self.plan);
        // Use entry list semantics by encoding both in a tuple; the
        // execute() path below picks the right one based on call count.
        map.insert(
            index,
            U5SlotOutcome::ScriptedThen {
                initial: Box::new(initial),
                follow_up: Box::new(follow_up),
            },
        );
        self
    }

    /// 2026-07-30-001 plan U1: script one outcome per attempt.
    pub(super) fn with_attempts(mut self, index: u32, steps: Vec<U5SlotOutcome>) -> Self {
        let map = std::sync::Arc::make_mut(&mut self.plan);
        map.insert(index, U5SlotOutcome::PerAttempt(steps));
        self
    }

    pub(super) fn call_count(&self, slot_index: u32) -> u32 {
        self.calls
            .lock()
            .unwrap()
            .get(&slot_index)
            .copied()
            .unwrap_or(0)
    }

    /// 2026-07-30-001 plan U2: the prompts this slot received, in
    /// attempt order.
    pub(super) fn prompts_for(&self, slot_index: u32) -> Vec<String> {
        self.prompts
            .lock()
            .unwrap()
            .get(&slot_index)
            .cloned()
            .unwrap_or_default()
    }
}

/// 2026-07-30-001 plan U1: an actively-reported terminal failure event
/// (`exec.unit.failed` / `review.unit.failed`) carrying a `reason`.
pub(super) fn u5_failed_event(terminal_topic: &'static str, reason: &str) -> ralph_core::Event {
    ralph_core::Event {
        topic: terminal_topic.to_string(),
        payload: Some(format!(
            "{}",
            serde_json::json!({ "reason": reason, "unit_id": "u1" })
        )),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    }
}
/// Build a deterministic `ralph_core::Event` for a (slot, seq) pair so
/// the content hash is stable across runs.
///
/// Uses the production-shaped terminal topic `exec.unit.done` so the
/// dispatcher's classifier (2026-07-23-007 plan U1) recognises it as
/// a terminal Done marker and routes to `record_slot_result` instead
/// of `record_slot_failure(empty_worker_result)`.
pub(super) fn u5_event(slot_index: u32, seq: usize) -> ralph_core::Event {
    ralph_core::Event {
        topic: "exec.unit.done".to_string(),
        payload: Some(format!("{{\"slot\":{slot_index},\"seq\":{seq}}}")),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    }
}

impl WaveWorkerExecutor for U5RecordingExecutor {
    fn execute(
        &self,
        mut request: crate::loop_runner::wave::WorkerRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (u32, WaveWorkerOutcome)> + Send>> {
        let plan = std::sync::Arc::clone(&self.plan);
        let default = self.default.clone();
        let calls = std::sync::Arc::clone(&self.calls);
        let prompts = std::sync::Arc::clone(&self.prompts);
        let delay = self.delay;
        Box::pin(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            prompts
                .lock()
                .unwrap()
                .entry(request.index)
                .or_default()
                .push(request.prompt.clone());
            // U2/007: capture the per-slot env map so the test
            // surface can assert on RALPH_WORKSPACE_ROOT and
            // RALPH_EVENTS_FILE. The executor runs in the dispatcher's
            // task, so writes go through the recording Mutex.
            if let Some(captured) = CAPTURED_ENV.get() {
                let mut guard = captured.lock().unwrap();
                guard.insert(request.index, request.backend.env_vars.clone());
            }
            let _ = request.worker_rpc_tx.take();
            let _ = request.worker_tui_state.take();
            let index = request.index;
            // 2026-07-28-003 plan U5 (S7 / S8 / S12): count attempts
            // per slot so the test surface can assert the budget loop.
            let attempt_number = {
                let mut guard = calls.lock().unwrap();
                let n = guard.entry(index).or_insert(0);
                *n += 1;
                *n
            };
            let mapped = match plan.get(&index).cloned().unwrap_or(default.clone()) {
                U5SlotOutcome::ScriptedThen { initial, follow_up } => {
                    if attempt_number == 1 {
                        (*initial).clone()
                    } else {
                        (*follow_up).clone()
                    }
                }
                U5SlotOutcome::PerAttempt(steps) => {
                    let idx = (attempt_number as usize).saturating_sub(1).min(
                        steps
                            .len()
                            .checked_sub(1)
                            .expect("PerAttempt script must not be empty"),
                    );
                    steps[idx].clone()
                }
                other => other,
            };
            let outcome = mapped;
            match outcome {
                U5SlotOutcome::Success(count) => {
                    let events: Vec<ralph_core::Event> =
                        (0..count).map(|seq| u5_event(index, seq)).collect();
                    (index, Ok((events, Duration::from_millis(5), true, None)))
                }
                U5SlotOutcome::Fail(reason) => (index, Err((reason, Duration::from_millis(5)))),
                U5SlotOutcome::ReportedFailure {
                    terminal_topic,
                    reason,
                } => (
                    index,
                    Ok((
                        vec![u5_failed_event(terminal_topic, &reason)],
                        Duration::from_millis(5),
                        true,
                        None,
                    )),
                ),
                U5SlotOutcome::ScriptedThen { .. } | U5SlotOutcome::PerAttempt(_) => {
                    unreachable!("mapped above")
                }
            }
        })
    }
}

/// Store-backed spy bridge for U5. `record_slot_result` /
/// `record_slot_failure` capture the call AND delegate to the real
/// store so tests can assert both on the captured payload (hash /
/// count / reason) and on `fan_in_status` (completed/failed counts).
#[derive(Clone)]
pub(super) struct U5RecordingBridge {
    pub(super) store: std::sync::Arc<dyn SupervisorStore>,
    /// `(slot_index, content_hash, event_count)` per successful record.
    pub(super) slot_results: std::sync::Arc<Mutex<Vec<(u32, String, usize)>>>,
    /// `(slot_index, reason)` per failure record.
    pub(super) slot_failures: std::sync::Arc<Mutex<Vec<(u32, String)>>>,
    /// 2026-07-28-003 plan U5: per-test override for the retry budget.
    /// Defaults to 0 so the existing characterization tests stay
    /// bit-identical to pre-U5; new S7/S8/S10/S12 tests override it.
    pub(super) retry_budget_override: std::sync::Arc<Mutex<Option<u32>>>,
    /// 2026-07-30-001 plan U2: per-test override for the slot's
    /// worktree path. The fresh-process test needs a directory that
    /// really exists, because the spawned worker actually chdirs into
    /// it.
    pub(super) worktree_override: std::sync::Arc<Mutex<Option<std::path::PathBuf>>>,
}

impl std::fmt::Debug for U5RecordingBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("U5RecordingBridge").finish()
    }
}

impl U5RecordingBridge {
    pub(super) fn new(store: std::sync::Arc<dyn SupervisorStore>) -> Self {
        Self {
            store,
            slot_results: std::sync::Arc::new(Mutex::new(Vec::new())),
            slot_failures: std::sync::Arc::new(Mutex::new(Vec::new())),
            retry_budget_override: std::sync::Arc::new(Mutex::new(None)),
            worktree_override: std::sync::Arc::new(Mutex::new(None)),
        }
    }

    pub(super) fn with_retry_budget(self, budget: u32) -> Self {
        *self.retry_budget_override.lock().unwrap() = Some(budget);
        self
    }

    /// 2026-07-30-001 plan U2: pin every slot to a real directory so a
    /// spawned worker can chdir into it.
    #[cfg(unix)]
    pub(super) fn with_worktree(self, path: std::path::PathBuf) -> Self {
        *self.worktree_override.lock().unwrap() = Some(path);
        self
    }

    pub(super) fn results_snapshot(&self) -> Vec<(u32, String, usize)> {
        self.slot_results.lock().unwrap().clone()
    }

    pub(super) fn failures_snapshot(&self) -> Vec<(u32, String)> {
        self.slot_failures.lock().unwrap().clone()
    }
}

impl SupervisorBridge for U5RecordingBridge {
    // 2026-08-07-009 plan U2 (R1 / KTD5): expose the store so the
    // dispatcher's per-attempt begin/finish path can write
    // receipts. Tests that do not override this get the trait
    // default (None) which keeps receipt writes disabled —
    // matching the pre-U2 contract.
    fn store(&self) -> Option<std::sync::Arc<dyn SupervisorStore>> {
        Some(self.store.clone())
    }

    fn tick(
        &self,
        _wave_id: &str,
        _inputs: PhaseInputs,
    ) -> Result<ralph_core::supervisor::CoordinatorAction, BridgeError> {
        // U5 does NOT drive the coordinator fan-in (that is U6).
        Ok(ralph_core::supervisor::CoordinatorAction::ContinueCollect)
    }

    fn bind_slot(
        &self,
        _kind: WaveKind,
        wave_id: &str,
        slot_index: u32,
    ) -> Result<Option<SlotBinding>, BridgeError> {
        // Return a real binding (worktree_path = Some) so Exec/Fix
        // slots pass the U1 KTD-4 fail-closed gate.
        let worktree_path = self
            .worktree_override
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| format!("/tmp/u5/{wave_id}-{slot_index}").into());
        Ok(Some(SlotBinding {
            slot_index,
            env: HashMap::new(),
            worktree_path: Some(worktree_path),
        }))
    }

    fn slot_retry_budget(&self) -> u32 {
        self.retry_budget_override.lock().unwrap().unwrap_or(0)
    }

    fn recover(&self) -> Result<Vec<ralph_core::supervisor::WaveSnapshot>, BridgeError> {
        self.store
            .recover_active_waves()
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn fan_in_status(&self, wave_id: &str) -> Result<WaveSnapshot, BridgeError> {
        self.store
            .fan_in_status(wave_id)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn register_wave_if_absent(
        &self,
        kind: WaveKind,
        wave_id: &str,
        expected_total: u32,
        _slot_retry_budget: u32,
    ) -> Result<String, BridgeError> {
        use ralph_core::supervisor::SupervisorStoreError;
        match self.store.register_wave(wave_id, kind, expected_total, 0) {
            Ok(store_wave_id) => Ok(store_wave_id),
            Err(SupervisorStoreError::DuplicateKey(_)) => {
                let mut snapshots = self
                    .store
                    .recover_active_waves()
                    .map_err(|err| BridgeError::Store(err.to_string()))?;
                let snapshot = snapshots.pop().ok_or_else(|| {
                    BridgeError::Store(format!(
                        "register_wave_if_absent: duplicate key {wave_id} but no recovered wave"
                    ))
                })?;
                Ok(snapshot.wave_id)
            }
            Err(err) => Err(BridgeError::Store(err.to_string())),
        }
    }

    fn try_dispatch_next(&self, _wave_id: &str, _slot_index: u32) -> Result<bool, BridgeError> {
        // Approve every slot the dispatcher asks about.
        Ok(true)
    }

    fn record_slot_result(
        &self,
        _wave_id: &str,
        slot_index: u32,
        content_hash: &str,
        event_count: usize,
    ) -> Result<(), BridgeError> {
        self.slot_results
            .lock()
            .unwrap()
            .push((slot_index, content_hash.to_string(), event_count));
        self.store
            .record_slot_result(_wave_id, slot_index, content_hash, event_count)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn record_slot_failure(
        &self,
        _wave_id: &str,
        slot_index: u32,
        reason: &str,
    ) -> Result<(), BridgeError> {
        self.slot_failures
            .lock()
            .unwrap()
            .push((slot_index, reason.to_string()));
        self.store
            .record_slot_failure(_wave_id, slot_index, reason)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }

    fn release_slot_dispatch(
        &self,
        wave_id: &str,
        slot_index: u32,
        outcome: ralph_core::supervisor::DispatchOutcome,
    ) -> Result<(), BridgeError> {
        self.store
            .release_slot_dispatch(wave_id, slot_index, outcome)
            .map_err(|err| BridgeError::Store(err.to_string()))
    }
}

/// Drive `execute_wave_via_supervisor_with_executor` with a
/// `U5RecordingBridge` and the scripted `U5RecordingExecutor`.
/// Returns the dispatch outcome and the bridge (for spy assertions).
pub(super) async fn run_u5_execute_wave(
    bridge: U5RecordingBridge,
    wave: ralph_core::DetectedWave,
    executor: U5RecordingExecutor,
) -> (WaveDispatchOutcome, U5RecordingBridge, U5RecordingExecutor) {
    use crate::loop_runner::wave::execute_wave_via_supervisor_with_executor;

    let workspace_root = std::env::temp_dir().join(format!("u5-disp-{}", wave.wave_id));
    let ralph_dir = workspace_root.join(".ralph");
    std::fs::create_dir_all(&ralph_dir).expect("create test .ralph directory");
    let main_events_file = ralph_dir.join("events.jsonl");
    std::fs::File::create(&main_events_file).expect("create test events file");

    // 2026-07-28-003 plan U5 (S7 / S8 / S12): share the executor's
    // call counter between the dispatcher-side Arc<dyn..> and the
    // returned probe so the test surface can assert attempt counts.
    let calls = std::sync::Arc::clone(&executor.calls);
    let prompts = std::sync::Arc::clone(&executor.prompts);
    let executor_dyn: std::sync::Arc<dyn WaveWorkerExecutor> = std::sync::Arc::new(executor) as _;
    let bridge_arc: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge.clone());
    let outcome = execute_wave_via_supervisor_with_executor(
        &wave,
        &make_test_cli_backend(),
        &main_events_file,
        false,
        false,
        None,
        None,
        "u5-test-loop",
        WaveDispatchLimits::default(),
        None,
        None,
        &bridge_arc,
        executor_dyn,
        None, // pre_registered_id: not pre-registered in test path
        None, // slot_index_override: test path uses events-array position
    )
    .await;
    let probe = U5RecordingExecutor {
        plan: std::sync::Arc::new(std::collections::HashMap::new()),
        default: U5SlotOutcome::Success(0),
        calls,
        prompts,
        delay: Duration::ZERO,
    };
    (outcome, bridge, probe)
}

// =============================================================================
// 2026-07-28-003 plan U5 (R9 / R10 / R11 / R12 / R13): dispatcher task 内
// 自动重试闭环 — S7 / S8 / S10 / S12 真实集成覆盖（注入 retryable 失败）
//
// Existing characterization tests only exercise the bridgeAccessor /
// worker_timeout classifier surface. These four tests drive the real
// `execute_wave_via_supervisor_with_executor` with a `U5RecordingExecutor`
// that returns `Err((reason, duration))` on attempt 1 and `Ok(...)` on
// attempt 2, then assert on attempt count, `record_slot_result` /
// `record_slot_failure` invocations, and the fingerprint contract.
// =============================================================================

/// Sanitized retryable reason (matches `dispatcher.rs:5359-5366` first
/// arm classification when the worker's `Err` reason starts with the
/// `WORKER_TIMEOUT_ERR_PREFIX`).
pub(super) const U5_RETRYABLE_REASON: &str =
    "Worker timed out after 1s of startup grace (worker_timeout/startup_kill, no first signal)";

// =============================================================================
// 2026-07-30-001 plan U1: an Exec worker that actively emits
// `exec.unit.failed` is a retryable failed ATTEMPT, not a Completed
// slot. These tests drive the real supervisor dispatch path and assert
// on executor call counts, the store's record_slot_* calls and the
// event batch that escapes to the tracker.
// =============================================================================

/// Convenience: the reported-failure script for an Exec slot.
pub(super) fn exec_reported_failure(reason: &str) -> U5SlotOutcome {
    U5SlotOutcome::ReportedFailure {
        terminal_topic: "exec.unit.failed",
        reason: reason.to_string(),
    }
}

pub(super) fn completed_wave_of(outcome: &WaveDispatchOutcome) -> &ralph_core::CompletedWave {
    match outcome {
        WaveDispatchOutcome::Completed(w)
        | WaveDispatchOutcome::Partial(w)
        | WaveDispatchOutcome::AggregateDeadlineExceeded(w) => w,
        other => panic!("expected a completed/partial wave, got {other:?}"),
    }
}

// =============================================================================
// 2026-07-30-001 plan U2: a retry is a NEW backend process in the SAME
// worktree, and it is told what the previous attempts hit.
//
// These tests spawn a real fake backend through `ProductionExecutor`, so
// they prove the second invocation is a distinct OS process (distinct
// PID) that chdir'd into the same directory and received a prompt
// carrying the retry block. A recording executor could not prove any of
// that.
// =============================================================================

/// One captured backend invocation.
#[cfg(unix)]
pub(super) struct U2AttemptRecord {
    pub(super) pid: String,
    pub(super) cwd: std::path::PathBuf,
    pub(super) prompt: String,
}

/// Body of the fake backend used by the U2 fresh-process tests.
///
/// Each invocation appends one record file to `$U2_RECORD_DIR` holding
/// its own PID, its working directory, and everything it was handed as
/// a prompt (inline argument or prompt temp file — the loop covers both
/// delivery shapes). It then writes the terminal event scripted for
/// that attempt number.
#[cfg(unix)]
pub(super) const U2_FRESH_PROCESS_BACKEND: &str = r#"
n=$(ls "$U2_RECORD_DIR" | wc -l | tr -d ' ')
n=$((n + 1))
rec="$U2_RECORD_DIR/attempt-$n"
{
  echo "pid=$$"
  echo "cwd=$(pwd)"
  echo "--PROMPT--"
  for arg in "$@"; do
    if [ -f "$arg" ]; then cat "$arg"; else printf '%s\n' "$arg"; fi
  done
} > "$rec"
if [ "$n" -eq 1 ]; then
  cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"exec.unit.failed","payload":"{\"reason\":\"u2 first attempt left the unit tests red\"}","ts":"2026-01-01T00:00:00Z"}
EOF
else
  cat <<'EOF' > "$RALPH_EVENTS_FILE"
{"topic":"exec.unit.done","payload":"{\"unit_id\":\"u2\"}","ts":"2026-01-01T00:00:00Z"}
EOF
fi
"#;

/// Read every record the fake backend wrote, in attempt order.
#[cfg(unix)]
pub(super) fn u2_read_attempts(record_dir: &std::path::Path) -> Vec<U2AttemptRecord> {
    let mut names: Vec<_> = std::fs::read_dir(record_dir)
        .expect("record dir must exist")
        .map(|entry| entry.expect("record entry").path())
        .collect();
    names.sort();
    names
        .iter()
        .map(|path| {
            let raw = std::fs::read_to_string(path).expect("record must be readable");
            let (head, prompt) = raw
                .split_once("--PROMPT--\n")
                .expect("record must carry a prompt section");
            let mut pid = String::new();
            let mut cwd = std::path::PathBuf::new();
            for line in head.lines() {
                if let Some(v) = line.strip_prefix("pid=") {
                    pid = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("cwd=") {
                    cwd = std::path::PathBuf::from(v.trim());
                }
            }
            U2AttemptRecord {
                pid,
                cwd,
                prompt: prompt.to_string(),
            }
        })
        .collect()
}

/// Drive the real supervisor path with `ProductionExecutor` and a fake
/// backend script, so every attempt is an actual spawned process.
///
/// Returns the captured per-attempt records plus the spy bridge.
#[cfg(unix)]
pub(super) async fn run_u2_fresh_process_wave(
    wave_name: &str,
    retry_budget: u32,
    prepare_worktree: impl FnOnce(&std::path::Path),
) -> (
    Vec<U2AttemptRecord>,
    U5RecordingBridge,
    std::path::PathBuf,
    // Returned so the caller keeps the worktree alive while asserting.
    tempfile::TempDir,
) {
    use crate::loop_runner::wave::execute_wave_via_supervisor_with_executor;

    let tmp = tempfile::tempdir().expect("temp dir");
    let ralph_dir = tmp.path().join(".ralph");
    let worktree = tmp.path().join("worktree");
    let record_dir = tmp.path().join("records");
    let bin_dir = tmp.path().join("bin");
    for dir in [&ralph_dir, &worktree, &record_dir, &bin_dir] {
        std::fs::create_dir_all(dir).expect("create test dir");
    }
    prepare_worktree(&worktree);

    let main_events_file = ralph_dir.join("events.jsonl");
    std::fs::File::create(&main_events_file).expect("create events file");
    let script = super::fake_path::write_fake_executable(
        &bin_dir,
        "u2-fresh-backend",
        U2_FRESH_PROCESS_BACKEND,
    );

    let backend = CliBackend {
        command: script.display().to_string(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format: ralph_adapters::OutputFormat::Text,
        env_vars: vec![(
            "U2_RECORD_DIR".to_string(),
            record_dir.display().to_string(),
        )],
    };

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(retry_budget)
        .with_worktree(worktree.clone());
    let bridge_arc: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge.clone());

    let _outcome = execute_wave_via_supervisor_with_executor(
        &make_u3_wave(wave_name, 1, 1),
        &backend,
        &main_events_file,
        false,
        false,
        None,
        None,
        "u2-test-loop",
        WaveDispatchLimits::default(),
        None,
        None,
        &bridge_arc,
        std::sync::Arc::new(crate::loop_runner::wave::ProductionExecutor),
        None,
        None,
    )
    .await;

    (u2_read_attempts(&record_dir), bridge, worktree, tmp)
}

// =============================================================================
// 2026-07-23-007 plan U2 (R-W1): control-plane binding + workspace root
// injection in the production supervisor path.
//
// These tests drive `execute_wave_via_supervisor_with_executor` with the
// real production bridge (`CoordinatorSupervisorBridge` +
// `ProductionBridgeContext`) and a controllable executor that captures
// the per-slot env map so the test surface can assert on
// `RALPH_WORKSPACE_ROOT` / `RALPH_EVENTS_FILE`. The capture is
// opt-in via a thread-local so the existing U5 tests stay untouched.
// =============================================================================

pub(crate) static CAPTURED_ENV: std::sync::OnceLock<
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u32, Vec<(String, String)>>>>,
> = std::sync::OnceLock::new();

pub(super) fn captured_env()
-> std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u32, Vec<(String, String)>>>> {
    CAPTURED_ENV
        .get_or_init(|| {
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))
        })
        .clone()
}

/// Run a wave through the production supervisor path with env capture
/// enabled. Returns the dispatch outcome and the captured per-slot
/// env map. Uses an isolated tempdir for the wave's per-worker
/// channels so the validator's parent-creatable check passes.
pub(super) async fn run_u2_execute_wave_with_env_capture(
    bridge: CoordinatorSupervisorBridge,
    wave: ralph_core::DetectedWave,
    executor: U5RecordingExecutor,
    main_events_file: &std::path::Path,
    loop_id: &str,
) -> WaveDispatchOutcome {
    use crate::loop_runner::wave::execute_wave_via_supervisor_with_executor;

    let bridge_arc: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge);
    let executor_dyn: std::sync::Arc<dyn WaveWorkerExecutor> = std::sync::Arc::new(executor);

    execute_wave_via_supervisor_with_executor(
        &wave,
        &ralph_adapters::CliBackend::claude(),
        main_events_file,
        false,
        false,
        None,
        None,
        loop_id,
        WaveDispatchLimits::default(),
        None,
        None,
        &bridge_arc,
        executor_dyn,
        None, // pre_registered_id: not pre-registered in test path
        None, // slot_index_override: test path uses events-array position
    )
    .await
}

// =============================================================================
// 2026-07-23-001 plan U6: production ledger sink + unique coordination event.
//
// These tests exercise the real production fan-in path:
// `run_supervisor_fan_in` drives the coordinator's
// `tick_with_slot_events`, which merges the per-slot business events
// through the production `FileEventMergeSink` into `events.jsonl` and
// injects the unique `*.wave.complete` / `*.wave.failed` coordination
// event (with the `success_slots` branch / worktree_path payload).
// =============================================================================

/// Build a production bridge whose coordinator merges through a
/// `FileEventMergeSink` pointed at `events_path`, then register a
/// wave with `n` slots and record every slot as a success (bound
/// worktree resource + dispatched + completed). Returns the bridge
/// (as a trait object) and the store-assigned wave id.
pub(super) fn setup_u6_production_bridge(
    events_path: std::path::PathBuf,
    wave_key: &str,
    n: u32,
) -> (std::sync::Arc<dyn SupervisorBridge>, String) {
    use crate::loop_runner::wave::{CoordinatorSupervisorBridge, ProductionBridgeContext};
    use ralph_core::supervisor::SlotResource;

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = ProductionBridgeContext {
        loop_id: "u6-loop".to_string(),
        repo_root: std::path::PathBuf::from("/tmp/u6-repo"),
        events_path: Some(events_path),
        tasks_path: None,
    };
    let bridge = CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        context,
        std::sync::Arc::new(DefaultWorktreeFactory),
        n.max(1),
        // 2026-07-28-003 plan U4: explicit budget keeps the U6
        // characterization helpers at the historical default.
        1,
    );
    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, wave_key, n, 0)
        .expect("register wave must succeed");
    for i in 0..n {
        bridge
            .store()
            .bind_worktree(
                &store_wave_id,
                i,
                SlotResource {
                    slot_index: i,
                    worktree_path: Some(format!("/tmp/u6-wt/{i}")),
                    branch: Some(format!("u6-loop-exec-{i}")),
                },
            )
            .expect("bind worktree must succeed");
    }
    for _ in 0..n {
        bridge
            .store()
            .try_dispatch_next(n.max(1))
            .expect("dispatch must succeed")
            .expect("a slot must be dispatchable");
    }
    for i in 0..n {
        bridge
            .record_slot_result(&store_wave_id, i, &format!("hash-{i}"), 1)
            .expect("record slot result must succeed");
        // Plan 004 R2 / P0-2: the production success path requires
        // terminal evidence per slot (KTD3 fail-closed). Without
        // this the coordinator falls into
        // `Failed(IncompleteEvidence)` and the wave never reaches
        // `InjectedComplete`.
        bridge
            .store()
            .record_slot_terminal_evidence(
                &store_wave_id,
                i,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "exec.unit.done",
                    &format!("{{\"unit\":\"u6-{i}\"}}"),
                ),
            )
            .expect("record terminal evidence must succeed");
    }
    (std::sync::Arc::new(bridge), store_wave_id)
}

/// Build a `CompletedWave` carrying one distinct `exec.unit.done`
/// business event per slot. The `results` are emitted in REVERSE
/// slot order so the test can assert the fan-in re-sorts them by
/// slot index before writing to the ledger.
pub(super) fn make_u6_completed(wave_key: &str, n: u32) -> ralph_core::CompletedWave {
    let results = (0..n)
        .rev()
        .map(|i| ralph_core::WaveResult {
            index: i,
            events: vec![
                ralph_proto::Event::new("exec.unit.done", format!("{{\"unit\":\"u6-{i}\"}}"))
                    .with_source("executor")
                    .with_wave(wave_key.to_string(), i, n),
            ],
        })
        .collect();
    ralph_core::CompletedWave {
        wave_id: wave_key.to_string(),
        wave_total: n,
        results,
        failures: vec![],
        duration: std::time::Duration::from_millis(1),
        partial: false,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: Vec::new(),
    }
}

/// Read the ledger file and parse each non-empty line as JSON.
pub(super) fn read_u6_ledger(path: &std::path::Path) -> Vec<serde_json::Value> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("ledger line must be JSON"))
        .collect()
}

/// Run a single-slot wave with a custom `WaveWorkerExecutor`, while
/// keeping the U5RecordingBridge in the loop so the dispatcher's
/// `record_slot_result` / `record_slot_failure` calls land in the
/// spy. This is the dispatcher-level runner that powers U3's
/// outside-in channel-writing test.
pub(super) async fn run_u3_dispatch_wave<E: WaveWorkerExecutor + 'static>(
    bridge: U5RecordingBridge,
    wave: ralph_core::DetectedWave,
    executor: E,
) -> WaveDispatchOutcome {
    use crate::loop_runner::wave::execute_wave_via_supervisor_with_executor;

    let wave_dir =
        std::env::temp_dir().join(format!("u3-disp-{}-{}", wave.wave_id, std::process::id()));
    let _ = std::fs::remove_dir_all(&wave_dir);
    let _ = std::fs::create_dir_all(&wave_dir);
    let main_events_file = wave_dir.join("events.jsonl");
    let _ = std::fs::File::create(&main_events_file);

    let executor_dyn: std::sync::Arc<dyn WaveWorkerExecutor> = std::sync::Arc::new(executor);
    let bridge_arc: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge);
    execute_wave_via_supervisor_with_executor(
        &wave,
        &make_test_cli_backend(),
        &main_events_file,
        false,
        false,
        None,
        None,
        "u3-test-loop",
        WaveDispatchLimits::default(),
        None,
        None,
        &bridge_arc,
        executor_dyn,
        None, // pre_registered_id: not pre-registered in test path
        None, // slot_index_override: test path uses events-array position
    )
    .await
}

// ── 2026-07-28-002 plan U4: boot redrive scanner tests ─────────────────────────────────
//
// S3: `dispatch_pending_redrive_waves` → `dispatch_redrive_child_wave` →
//      worker spawned exactly once (in-memory store).
// S3 (rusqlite-backed): same flow with real SQLite store.
// S4: `expected_digest = None` → fail-closed (no descriptor persisted).
// S5: digest conflict → fail-closed.
// S6: no pending children → executor not called.

// Helper: build a HatRegistry with one hat subscribing to "exec.unit.ready".
pub(super) fn make_test_hat_registry() -> ralph_core::HatRegistry {
    let yaml = r#"
hats:
  test-exec:
    name: TestExec
    triggers:
      - "exec.unit.ready"
    publishes:
      - "exec.unit.done"
    timeout: 300
    concurrency: 4
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    ralph_core::HatRegistry::from_config(&config)
}

// The fail-closed variant of S2a (persist failure → no spawn) is
// implemented below via `PersistFailingSupervisorStore`
// (`test_s2a_persist_failure_fails_closed_no_spawn`) — the follow-up
// that the original S2a landing deferred.

// =============================================================================
// 2026-07-28-002 plan U3 follow-up (S2a fail-closed): persist fault injection.
//
// The dispatcher persists a `SlotDescriptor` after `bind_slot` and before
// spawning the worker (dispatcher.rs, U3 wiring). When that persist fails,
// the slot MUST be skipped (no spawn) and the failure MUST be recorded on
// the bridge. `PersistFailingSupervisorStore` delegates every store method
// to a real `InMemorySupervisorStore` except `persist_slot_descriptor`,
// which always errors — proving the fail-closed branch end to end.
// =============================================================================

/// Fault-injecting store wrapper: all methods delegate to `inner`,
/// except `persist_slot_descriptor`, which always fails.
#[derive(Debug)]
pub(super) struct PersistFailingSupervisorStore {
    pub(super) inner: std::sync::Arc<ralph_core::supervisor::InMemorySupervisorStore>,
}

impl ralph_core::supervisor::SupervisorStore for PersistFailingSupervisorStore {
    fn persist_slot_descriptor(
        &self,
        _wave_id: &str,
        _descriptor: &ralph_core::supervisor::SlotDescriptor,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        Err(ralph_core::supervisor::SupervisorStoreError::Storage(
            "synthetic persist failure (fault injection)".to_string(),
        ))
    }

    fn register_wave(
        &self,
        idempotency_key: &str,
        kind: ralph_core::supervisor::WaveKind,
        expected_total: u32,
        slot_retry_budget: u32,
    ) -> ralph_core::supervisor::SupervisorStoreResult<String> {
        self.inner
            .register_wave(idempotency_key, kind, expected_total, slot_retry_budget)
    }

    fn enqueue_wave(
        &self,
        idempotency_key: &str,
        kind: ralph_core::supervisor::WaveKind,
        expected_total: u32,
        slot_retry_budget: u32,
    ) -> ralph_core::supervisor::SupervisorStoreResult<String> {
        self.inner
            .enqueue_wave(idempotency_key, kind, expected_total, slot_retry_budget)
    }

    fn try_dispatch_next(
        &self,
        max_concurrent_workers: u32,
    ) -> ralph_core::supervisor::SupervisorStoreResult<Option<(String, u32)>> {
        self.inner.try_dispatch_next(max_concurrent_workers)
    }

    fn release_slot_dispatch(
        &self,
        wave_id: &str,
        slot_index: u32,
        outcome: ralph_core::supervisor::DispatchOutcome,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner
            .release_slot_dispatch(wave_id, slot_index, outcome)
    }

    fn bind_worktree(
        &self,
        wave_id: &str,
        slot_index: u32,
        binding: ralph_core::supervisor::SlotResource,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner.bind_worktree(wave_id, slot_index, binding)
    }

    fn record_slot_result(
        &self,
        wave_id: &str,
        slot_index: u32,
        content_hash: &str,
        event_count: usize,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner
            .record_slot_result(wave_id, slot_index, content_hash, event_count)
    }

    fn record_slot_failure(
        &self,
        wave_id: &str,
        slot_index: u32,
        reason: &str,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner.record_slot_failure(wave_id, slot_index, reason)
    }

    fn slot_failure_reason(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> ralph_core::supervisor::SupervisorStoreResult<Option<String>> {
        self.inner.slot_failure_reason(wave_id, slot_index)
    }

    fn cancel_wave(&self, wave_id: &str) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner.cancel_wave(wave_id)
    }

    fn record_slot_pid(
        &self,
        wave_id: &str,
        slot_index: u32,
        pid: u32,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner.record_slot_pid(wave_id, slot_index, pid)
    }

    fn pid_for_slot(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> ralph_core::supervisor::SupervisorStoreResult<Option<u32>> {
        self.inner.pid_for_slot(wave_id, slot_index)
    }

    fn fan_in_status(
        &self,
        wave_id: &str,
    ) -> ralph_core::supervisor::SupervisorStoreResult<ralph_core::supervisor::WaveSnapshot> {
        self.inner.fan_in_status(wave_id)
    }

    fn commit_salvage_projection(
        &self,
        wave_id: &str,
        receipt: &ralph_core::supervisor::ProjectionReceiptSummary,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner.commit_salvage_projection(wave_id, receipt)
    }

    fn record_coordination_written(
        &self,
        wave_id: &str,
        receipt: &ralph_core::supervisor::CoordinationReceiptSummary,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner.record_coordination_written(wave_id, receipt)
    }

    fn commit_coordination_event(
        &self,
        wave_id: &str,
        receipt: &ralph_core::supervisor::CoordinationReceiptSummary,
        terminal_phase: ralph_core::supervisor::WavePhase,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner
            .commit_coordination_event(wave_id, receipt, terminal_phase)
    }

    fn list_wave_ids(&self) -> ralph_core::supervisor::SupervisorStoreResult<Vec<String>> {
        self.inner.list_wave_ids()
    }

    fn wave_id_for_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> ralph_core::supervisor::SupervisorStoreResult<Option<String>> {
        self.inner.wave_id_for_idempotency_key(idempotency_key)
    }

    fn recover_active_waves(
        &self,
    ) -> ralph_core::supervisor::SupervisorStoreResult<Vec<ralph_core::supervisor::WaveSnapshot>>
    {
        self.inner.recover_active_waves()
    }

    fn list_worktree_paths(
        &self,
        wave_id: &str,
    ) -> ralph_core::supervisor::SupervisorStoreResult<Vec<ralph_core::supervisor::SlotResource>>
    {
        self.inner.list_worktree_paths(wave_id)
    }

    fn get_slot_resource(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> ralph_core::supervisor::SupervisorStoreResult<Option<ralph_core::supervisor::SlotResource>>
    {
        self.inner.get_slot_resource(wave_id, slot_index)
    }

    fn set_wave_phase(
        &self,
        wave_id: &str,
        phase: ralph_core::supervisor::WavePhase,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner.set_wave_phase(wave_id, phase)
    }

    fn enqueue_compensation(
        &self,
        wave_id: &str,
        kind: ralph_core::supervisor::CompensationKind,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner.enqueue_compensation(wave_id, kind)
    }

    fn take_pending_compensations(
        &self,
    ) -> ralph_core::supervisor::SupervisorStoreResult<
        Vec<(String, ralph_core::supervisor::CompensationKind)>,
    > {
        self.inner.take_pending_compensations()
    }

    fn complete_compensation(
        &self,
        wave_id: &str,
        kind: ralph_core::supervisor::CompensationKind,
        ok: bool,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner.complete_compensation(wave_id, kind, ok)
    }

    fn create_redrive_wave(
        &self,
        parent_wave_id: &str,
        slots: Option<&[u32]>,
    ) -> ralph_core::supervisor::SupervisorStoreResult<ralph_core::supervisor::RedriveResult> {
        self.inner.create_redrive_wave(parent_wave_id, slots)
    }

    fn reserve_emission(
        &self,
        scope_key: &str,
        payload_digest: &str,
        expected_count: u32,
        count_events_on_disk: &dyn Fn(&str) -> u32,
    ) -> ralph_core::supervisor::SupervisorStoreResult<ralph_core::supervisor::EmissionReservation>
    {
        self.inner.reserve_emission(
            scope_key,
            payload_digest,
            expected_count,
            count_events_on_disk,
        )
    }

    fn mark_emission_applying(
        &self,
        scope_key: &str,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner.mark_emission_applying(scope_key)
    }

    fn mark_emission_applied(
        &self,
        scope_key: &str,
        applied_at_unix_secs: u64,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner
            .mark_emission_applied(scope_key, applied_at_unix_secs)
    }

    fn mark_emission_recovery_required(
        &self,
        scope_key: &str,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner.mark_emission_recovery_required(scope_key)
    }

    fn mark_emission_failed(
        &self,
        scope_key: &str,
    ) -> ralph_core::supervisor::SupervisorStoreResult<()> {
        self.inner.mark_emission_failed(scope_key)
    }

    fn emission_state_for_wave_id(
        &self,
        public_wave_id: &str,
    ) -> ralph_core::supervisor::SupervisorStoreResult<Option<ralph_core::supervisor::EmissionState>>
    {
        self.inner.emission_state_for_wave_id(public_wave_id)
    }

    fn adopt_legacy_emission(
        &self,
        scope_key: &str,
        payload_digest: &str,
        expected_count: u32,
        legacy_wave_id: &str,
    ) -> ralph_core::supervisor::SupervisorStoreResult<String> {
        self.inner
            .adopt_legacy_emission(scope_key, payload_digest, expected_count, legacy_wave_id)
    }

    // 2026-08-07-009 plan U1 / U3: per-slot attempt receipt +
    // parent resolver API. `PersistFailingSupervisorStore` only
    // fails on `persist_slot_descriptor`; the new methods
    // delegate so U1/U3 integration tests can exercise the
    // same fault seam without inventing a fourth mock store.
    fn begin_slot_attempt(
        &self,
        wave_id: &str,
        slot_index: u32,
        start_checkpoint: Option<ralph_core::supervisor::GitCheckpoint>,
        started_at_unix_ms: u64,
    ) -> ralph_core::supervisor::SupervisorStoreResult<ralph_core::supervisor::SlotAttemptReceipt>
    {
        self.inner
            .begin_slot_attempt(wave_id, slot_index, start_checkpoint, started_at_unix_ms)
    }

    fn finish_slot_attempt(
        &self,
        wave_id: &str,
        slot_index: u32,
        attempt_seq: u32,
        status: ralph_core::supervisor::AttemptStatus,
        end_checkpoint: Option<ralph_core::supervisor::GitCheckpoint>,
        failure_code: Option<&str>,
        finished_at_unix_ms: u64,
    ) -> ralph_core::supervisor::SupervisorStoreResult<ralph_core::supervisor::SlotAttemptReceipt>
    {
        self.inner.finish_slot_attempt(
            wave_id,
            slot_index,
            attempt_seq,
            status,
            end_checkpoint,
            failure_code,
            finished_at_unix_ms,
        )
    }

    fn list_slot_attempts(
        &self,
        wave_id: &str,
        slot_index: u32,
        limit: Option<u32>,
    ) -> ralph_core::supervisor::SupervisorStoreResult<
        Vec<ralph_core::supervisor::SlotAttemptReceipt>,
    > {
        self.inner.list_slot_attempts(wave_id, slot_index, limit)
    }

    fn parent_slot_attempts(
        &self,
        child_wave_id: &str,
        child_slot_index: u32,
        limit: Option<u32>,
    ) -> ralph_core::supervisor::SupervisorStoreResult<ralph_core::supervisor::SlotAttemptHistory>
    {
        self.inner
            .parent_slot_attempts(child_wave_id, child_slot_index, limit)
    }

    fn parent_slot_resource(
        &self,
        child_wave_id: &str,
        child_slot_index: u32,
    ) -> ralph_core::supervisor::ParentResourceResult<Option<ralph_core::supervisor::SlotResource>>
    {
        self.inner
            .parent_slot_resource(child_wave_id, child_slot_index)
    }
}

// =============================================================================
// 2026-07-28-002 plan U4 (G1 / R-F1 / S3-S6): boot redrive dispatch.
//
// `dispatch_pending_redrive_waves` scans the supervisor store for redrive
// child waves created by a previous loop (`create_redrive_wave`) but never
// dispatched (crash between create and spawn), takes each slot's descriptor
// (fail-closed on unavailable / digest conflict), and dispatches each slot
// as a single-slot wave through the supervisor path with the TRUE child
// slot index (C3: multi-slot child waves must bind 0/1/2, not 0/0/0).
// =============================================================================

/// Executor that records the slot index and wave_total (from the
/// worker prompt `worker **i/N**` line) of every spawn request.
#[derive(Clone, Default)]
pub(super) struct U4SlotRecordingExecutor {
    pub(super) indices: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
    pub(super) totals: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
}

impl WaveWorkerExecutor for U4SlotRecordingExecutor {
    fn execute(
        &self,
        mut request: crate::loop_runner::wave::WorkerRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (u32, WaveWorkerOutcome)> + Send>> {
        let indices = std::sync::Arc::clone(&self.indices);
        let totals = std::sync::Arc::clone(&self.totals);
        Box::pin(async move {
            indices.lock().unwrap().push(request.index);
            // Prompt line: "You are worker **{i}/{total}** in wave ..."
            let total = request
                .prompt
                .lines()
                .find_map(|line| {
                    let start = line.find("worker **")?;
                    let rest = &line[start + "worker **".len()..];
                    let (_idx, after_slash) = rest.split_once('/')?;
                    let (total_s, _) = after_slash.split_once("**")?;
                    total_s.parse::<u32>().ok()
                })
                .unwrap_or(0);
            totals.lock().unwrap().push(total);
            let _ = request.worker_rpc_tx.take();
            let _ = request.worker_tui_state.take();
            let event = ralph_core::Event {
                topic: "exec.done".to_string(),
                payload: Some("ok".to_string()),
                ts: String::new(),
                hat: None,
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            };
            (
                request.index,
                Ok((vec![event], Duration::from_millis(5), true, None)),
            )
        })
    }
}

/// A hat registry with one hat subscribed to `exec.unit.ready` so
/// `dispatch_redrive_child_wave` can resolve the descriptor topic.
pub(super) fn redrive_test_registry() -> ralph_core::HatRegistry {
    let mut registry = ralph_core::HatRegistry::new();
    let hat = ralph_proto::Hat {
        id: ralph_proto::HatId::new("redrive-worker"),
        name: "Redrive Worker".to_string(),
        description: "test hat for redrive boot dispatch".to_string(),
        subscriptions: vec![ralph_proto::Topic::new("exec.unit.ready")],
        publishes: vec![ralph_proto::Topic::new("exec.done")],
        instructions: String::new(),
    };
    registry.register_with_config(hat, ralph_core::config::HatConfig::default());
    registry
}

/// Register a parent wave with `slots` failed slots, optionally
/// persisting a descriptor per slot first (the U3 spawn-time record).
pub(super) fn make_redrive_parent_with_descriptors(
    store: &dyn ralph_core::supervisor::SupervisorStore,
    key: &str,
    slots: u32,
    persist_descriptors: bool,
) -> String {
    use ralph_core::supervisor::{SlotDescriptor, SlotResource, WaveKind};
    let wave = store.register_wave(key, WaveKind::Exec, slots, 1).unwrap();
    for i in 0..slots {
        store
            .bind_worktree(
                &wave,
                i,
                SlotResource {
                    slot_index: i,
                    worktree_path: Some(format!("/tmp/u4-redrive/{key}-{i}")),
                    branch: Some(format!("u4-{key}-{i}")),
                },
            )
            .unwrap();
        if persist_descriptors {
            let payload = format!(r#"{{"unit":"u{i}"}}"#);
            store
                .persist_slot_descriptor(
                    &wave,
                    &SlotDescriptor {
                        slot_index: i,
                        topic: "exec.unit.ready".to_string(),
                        payload_json: payload.clone(),
                        wave_kind: WaveKind::Exec,
                        payload_digest: SlotDescriptor::digest_of(&payload),
                        slot_index_in_parent: None,
                    },
                )
                .unwrap();
        }
        store
            .record_slot_failure(&wave, i, "synthetic parent failure")
            .unwrap();
    }
    wave
}
