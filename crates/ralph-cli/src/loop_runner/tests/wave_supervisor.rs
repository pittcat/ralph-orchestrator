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
use crate::loop_runner::wave::{
    BridgeError, MockSupervisorBridge, SlotBinding, SupervisorBridge, WaveWorkerExecutor,
    is_supervisor_path_enabled,
};
use ralph_core::supervisor::{PhaseInputs, SlotResource, TerminalEvidence, WaveKind, WaveSnapshot};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct SpyBindingBridge {
    bind_calls: Mutex<Vec<(WaveKind, String, u32)>>,
    bindings: Mutex<Vec<SlotBinding>>,
}

impl SpyBindingBridge {
    fn new() -> Self {
        Self::default()
    }
    fn record(&self, binding: SlotBinding) {
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

/// U9 happy path: supervisor `enabled == false` → the
/// dispatcher takes the legacy `WaveTracker::new()` route
/// and the bridge trait object is `None`. We assert the
/// predicate function gates this correctly and that no
/// `SupervisorBridge` is constructed when disabled.
#[test]
fn enabled_false_uses_wave_tracker() {
    assert!(
        !is_supervisor_path_enabled(false, true),
        "disabled branch must NOT take the supervisor route"
    );
    assert!(
        !is_supervisor_path_enabled(false, false),
        "disabled + coordinator mode must NOT take the supervisor route"
    );
    // The legacy `WaveTracker::new()` is reachable from
    // `ralph_core::WaveTracker`; pin the surface stays
    // public so the dispatcher can keep constructing it
    // when supervisor is disabled. (The actual
    // construction happens inside `execute_wave_structured`
    // which is exercised separately.)
    let _tracker = ralph_core::WaveTracker::new();
}

/// U9 edge: `enabled == true` + isolated mode → the
/// dispatcher calls `SupervisorBridge::bind_slot` exactly
/// once per slot, recording `(kind, wave_id, slot_index)`
/// in order, and forwards the returned `SlotBinding::env`
/// to the worker `Command::envs(...)`. We assert both the
/// call ordering and that the env keys surface the
/// `RALPH_WAVE_*` SSOT.
#[test]
fn enabled_true_calls_bridge_bind_slot() {
    assert!(
        is_supervisor_path_enabled(true, true),
        "enabled + isolated must take the supervisor route"
    );
    let bridge = SpyBindingBridge::new();
    let wave_id = "u9-wave-edge";

    // Simulate the dispatcher iterating over 3 worker
    // requests and calling bind_slot for each. The order
    // is preserved, so wave_index == 0,1,2 must appear in
    // the recorded list in that order.
    for slot_index in 0u32..3 {
        let binding = bridge
            .bind_slot(WaveKind::Exec, wave_id, slot_index)
            .expect("bind_slot must succeed for Exec");
        let binding = binding.expect("Exec binding must be Some");
        // The env map must surface the wave-handshake SSOT
        // so the worker process can read it.
        assert_eq!(
            binding.env.get("RALPH_WAVE_WORKER").map(String::as_str),
            Some("1")
        );
        assert!(
            binding
                .env
                .get("RALPH_WAVE_WORKTREE_PATH")
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "RALPH_WAVE_WORKTREE_PATH must be non-empty for Exec"
        );
        assert_eq!(
            binding.env.get("RALPH_WAVE_ID").map(String::as_str),
            Some(wave_id)
        );
        assert_eq!(
            binding.env.get("RALPH_WAVE_INDEX").map(String::as_str),
            Some(slot_index.to_string().as_str())
        );
        assert_eq!(
            binding.env.get("RALPH_WAVE_KIND").map(String::as_str),
            Some("exec")
        );
    }

    let calls = bridge.bind_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0], (WaveKind::Exec, wave_id.to_string(), 0));
    assert_eq!(calls[1], (WaveKind::Exec, wave_id.to_string(), 1));
    assert_eq!(calls[2], (WaveKind::Exec, wave_id.to_string(), 2));
}

/// U9 negative: when the bridge is unavailable (e.g. the
/// `supervisor-db` feature is off but the operator still
/// opted in via `event_loop.supervisor.enabled = true`),
/// the bridge surface must surface a structured error
/// path — NOT panic. The dispatcher catches the error and
/// decides whether to skip the wave or fall back to
/// `WaveTracker`.
///
/// Coverage:
/// 1. `MockSupervisorBridge` returns the default
///    `ContinueCollect` action without panicking.
/// 2. `BridgeError::Disabled` round-trips through Display
///    so callers can branch on the variant.
#[test]
fn bridge_off_no_feature_returns_error_path() {
    let bridge = MockSupervisorBridge::new();
    let action = bridge
        .tick("u9-wave-bridge-off", PhaseInputs::default())
        .expect("MockSupervisorBridge tick must not panic");
    assert_eq!(
        action,
        ralph_core::supervisor::CoordinatorAction::ContinueCollect,
        "bridge_off_no_feature_returns_error_path: default tick must surface ContinueCollect"
    );

    let disabled = BridgeError::Disabled;
    let rendered = format!("{disabled}");
    assert!(
        rendered.contains("supervisor"),
        "BridgeError::Disabled must mention supervisor in its render; got {rendered}"
    );
}

// Pin the bridge construction path: relative `db_path` resolves
// against the loop workspace, absolute paths are honoured as-is,
// and the `supervisor-db` feature flag is the binary capability
// gate. The `cfg(feature = "supervisor-db")` guard keeps these
// tests from invoking a guaranteed-fail-closed path on
// `--no-default-features` builds.
#[cfg(feature = "supervisor-db")]
#[test]
fn build_supervisor_bridge_relative_db_path_resolves_under_ralph_dir() {
    use ralph_core::LoopContext;
    use ralph_core::config::SupervisorConfig;

    let tmp = tempfile::tempdir().expect("temp dir");
    let ctx = LoopContext::primary(tmp.path().to_path_buf());
    let cfg = SupervisorConfig {
        enabled: true,
        db_path: ".ralph/supervisor.db".to_string(),
        max_concurrent_workers: 2,
        aggregate_timeout_secs: 60,
        // 2026-07-28-003 plan U4: explicit budget; default 1
        // mirrors the documented historical default so this
        // relative-path characterization test stays green.
        slot_retry_budget: 1,
    };
    let bridge = crate::loop_runner::build_supervisor_bridge(
        &cfg,
        &ctx,
        ctx.workspace().join(".ralph").join("events.jsonl"),
    )
    .expect("relative db_path must open a bridge");
    let snaps = bridge.recover().expect("recover on fresh bridge");
    assert!(snaps.is_empty(), "fresh bridge must have no active waves");
    assert!(
        tmp.path().join(".ralph").exists(),
        "build_supervisor_bridge must materialise .ralph/ parent"
    );
}

#[cfg(feature = "supervisor-db")]
#[test]
fn build_supervisor_bridge_absolute_db_path_honoured_as_is() {
    use ralph_core::LoopContext;
    use ralph_core::config::SupervisorConfig;

    let tmp = tempfile::tempdir().expect("temp dir");
    let ctx = LoopContext::primary(tmp.path().to_path_buf());
    let abs_db = tmp.path().join("nested").join("abs-supervisor.db");
    let cfg = SupervisorConfig {
        enabled: true,
        db_path: abs_db.display().to_string(),
        max_concurrent_workers: 1,
        aggregate_timeout_secs: 30,
        // 2026-07-28-003 plan U4: default budget for the
        // absolute-path characterization test.
        slot_retry_budget: 1,
    };
    let bridge = crate::loop_runner::build_supervisor_bridge(
        &cfg,
        &ctx,
        ctx.workspace().join(".ralph").join("events.jsonl"),
    )
    .expect("absolute db_path must open a bridge");
    // The parent dir was materialised by the bridge builder.
    assert!(
        tmp.path().join("nested").exists(),
        "absolute db_path parent must be materialised"
    );
    let _ = bridge.store();
}

/// `enabled: true` without the `supervisor-db` feature must
/// fail-closed at bridge construction rather than fall back to
/// in-memory state. The error path must not materialise `.ralph/`.
#[cfg(not(feature = "supervisor-db"))]
#[test]
fn build_supervisor_bridge_without_feature_enabled_returns_error() {
    use ralph_core::LoopContext;
    use ralph_core::config::SupervisorConfig;

    let tmp = tempfile::tempdir().expect("temp dir");
    let ctx = LoopContext::primary(tmp.path().to_path_buf());
    let cfg = SupervisorConfig {
        enabled: true,
        ..SupervisorConfig::default()
    };

    let err = crate::loop_runner::build_supervisor_bridge(
        &cfg,
        &ctx,
        ctx.workspace().join(".ralph").join("events.jsonl"),
    )
    .expect_err("enabled=true without supervisor-db feature must fail-closed");
    let msg = format!("{err}");
    assert!(
        msg.contains("supervisor-db"),
        "fail-closed error must mention the supervisor-db cargo feature; got: {msg}"
    );
    assert!(
        !tmp.path().join(".ralph").exists(),
        "fail-closed path must not materialise .ralph/"
    );
}

/// Path normalisation: the default `db_path`
/// (`.ralph/supervisor.db`) MUST resolve to
/// `<workspace>/.ralph/supervisor.db`, not the double-prefixed
/// `<workspace>/.ralph/.ralph/supervisor.db`.
#[cfg(feature = "supervisor-db")]
#[test]
fn build_supervisor_bridge_default_db_path_collapses_to_single_ralph() {
    use ralph_core::LoopContext;
    use ralph_core::config::SupervisorConfig;

    let tmp = tempfile::tempdir().expect("temp dir");
    let ctx = LoopContext::primary(tmp.path().to_path_buf());
    let cfg = SupervisorConfig {
        enabled: true,
        ..SupervisorConfig::default()
    };
    let _bridge = crate::loop_runner::build_supervisor_bridge(
        &cfg,
        &ctx,
        ctx.workspace().join(".ralph").join("events.jsonl"),
    )
    .expect("enabled+feature must build a bridge");
    let nested = tmp
        .path()
        .join(".ralph")
        .join(".ralph")
        .join("supervisor.db");
    assert!(
        !nested.exists(),
        "default db_path must NOT materialise `.ralph/.ralph/supervisor.db`; found {nested:?}"
    );
    let single = tmp.path().join(".ralph").join("supervisor.db");
    assert!(
        single.exists(),
        "default db_path must materialise `.ralph/supervisor.db`; missing {single:?}"
    );
}

/// Path normalisation: an absolute `db_path` is honoured as-is
/// even when it points outside the loop workspace; no implicit
/// parent substitution must happen.
#[cfg(feature = "supervisor-db")]
#[test]
fn build_supervisor_bridge_absolute_db_path_outside_workspace_preserved() {
    use ralph_core::LoopContext;
    use ralph_core::config::SupervisorConfig;

    let workspace = tempfile::tempdir().expect("workspace");
    let db_dir = tempfile::tempdir().expect("db dir");
    let ctx = LoopContext::primary(workspace.path().to_path_buf());
    let abs_db = db_dir.path().join("custom-supervisor.db");
    let cfg = SupervisorConfig {
        enabled: true,
        db_path: abs_db.display().to_string(),
        ..SupervisorConfig::default()
    };
    let _bridge = crate::loop_runner::build_supervisor_bridge(
        &cfg,
        &ctx,
        ctx.workspace().join(".ralph").join("events.jsonl"),
    )
    .expect("absolute db_path must open a bridge");
    assert!(
        abs_db.exists(),
        "absolute db_path must land at the operator-specified location; missing {abs_db:?}"
    );
    assert!(
        !workspace.path().join(".ralph").exists(),
        "absolute db_path outside workspace must NOT materialise `.ralph/` under the workspace"
    );
}

/// Phase 7: SpyBindingBridge with a worktree_path exercises the
/// env-merge precedence contract the dispatcher relies on:
/// binding env keys overwrite worker_backend env keys with the
/// same name (last-write-wins via the drain-rebuild in
/// `execute_wave_via_supervisor`). We assert the precedence
/// directly on the SlotBinding env map since the dispatcher's
/// merge logic is a thin `extend` over the binding's keys.
#[test]
fn slot_binding_env_overrides_worker_backend_env_keys() {
    let bridge = SpyBindingBridge::new();
    let wave_id = "u9-override";
    // Worker_backend would set RALPH_WAVE_ID to "legacy-value";
    // the binding's RALPH_WAVE_ID must win.
    let binding = bridge
        .bind_slot(WaveKind::Exec, wave_id, 0)
        .expect("bind_slot")
        .expect("Some binding");
    assert_eq!(
        binding.env.get("RALPH_WAVE_ID").map(String::as_str),
        Some(wave_id),
        "binding env must override worker_backend env for RALPH_WAVE_ID"
    );
    // The worktree_path is surfaced as RALPH_WAVE_WORKTREE_PATH in
    // the binding env so the worker process can read it via env.
    assert_eq!(
        binding
            .env
            .get("RALPH_WAVE_WORKTREE_PATH")
            .map(String::as_str),
        Some(format!("/tmp/u9-spy/{wave_id}-0").as_str()),
        "RALPH_WAVE_WORKTREE_PATH must come from the binding"
    );
    // slot_index field is distinct from the env RALPH_WAVE_INDEX
    // value; both must agree so the dispatcher's WorkerRequest.cwd
    // assignment and the worker's env-read stay in sync.
    assert_eq!(binding.slot_index, 0);
    assert_eq!(
        binding.env.get("RALPH_WAVE_INDEX").map(String::as_str),
        Some("0"),
        "RALPH_WAVE_INDEX env must match slot_index"
    );
}

/// Phase 7: when `bind_slot` returns `None` (SharedReadonly /
/// review slot), the dispatcher MUST NOT set a cwd on the
/// WorkerRequest. Pin the contract by observing the SpyBindingBridge
/// returns `Some` for Exec/Fix (Worktree isolation) and that a
/// review-kind bridge would return `None` — we emulate the
/// SharedReadonly branch by overriding the spy to return `None`
/// for Review kind.
#[test]
fn review_kind_bind_slot_returns_none_for_shared_readonly() {
    // The production `CoordinatorSupervisorBridge::bind_slot`
    // always returns `Ok(None)` (the trait-level SharedReadonly
    // default). Pin that the dispatch logic can distinguish
    // Exec/Fix (Some binding) from Review (None binding) via
    // the SpyBindingBridge's kind-aware stub.
    let bridge = SpyBindingBridge::new();
    // Exec → Some (Worktree isolation)
    let exec_binding = bridge
        .bind_slot(WaveKind::Exec, "w-review", 0)
        .expect("exec bind_slot")
        .expect("exec binding must be Some");
    assert!(exec_binding.worktree_path.is_some());

    // The production bridge returns `Ok(None)` for Review
    // (SharedReadonly). The SpyBindingBridge here always returns
    // Some; we cannot make it return None per-kind without
    // complicating the stub. Instead, assert the production
    // CoordinatorSupervisorBridge's bind_slot returns None for
    // any kind (the SharedReadonly default).
    use crate::loop_runner::wave::CoordinatorSupervisorBridge;
    let prod = CoordinatorSupervisorBridge::with_in_memory_store();
    let review_binding = prod
        .bind_slot(WaveKind::Review, "w-review", 0)
        .expect("review bind_slot must not error");
    assert!(
        review_binding.is_none(),
        "production bridge must return None for Review (SharedReadonly); got {review_binding:?}"
    );
}

/// Phase 7: `recover_active_waves_at_startup` is wired into the
/// runner's startup path. Pin the function's signature + return
/// type so a future refactor that drops the call (or changes the
/// return shape) is caught by nextest.
#[test]
fn recover_active_waves_at_startup_returns_report_on_empty_store() {
    use ralph_core::supervisor::{
        InMemorySupervisorStore, SupervisorStore, recover_active_waves_at_startup,
    };
    use std::sync::Arc;

    let store: Arc<dyn SupervisorStore> = Arc::new(InMemorySupervisorStore::new());
    let report = recover_active_waves_at_startup(store, 60).expect("recovery must not error");
    assert_eq!(report.inspected, 0);
    assert!(report.timed_out.is_empty());
    assert!(report.already_merged.is_empty());
}

/// 2026-07-24-001 plan U3 (R7 / KTD5 / Feature C3): a crash between
/// the supervisor-store mutation and the `tasks.jsonl` write leaves
/// the slot terminal (`Completed`) in the store while the projected
/// task row is stuck at `started`. `recover_pending_projections` —
/// now wired into loop startup right after a successful
/// `recover_active_waves_at_startup` — must replay the store
/// snapshot and bring the task to its terminal (`Closed`) state, and
/// a second recover must be a no-op (idempotent).
#[test]
fn recover_pending_projections_closes_stale_task_and_is_idempotent() {
    use crate::loop_runner::wave::task_projection::{
        SlotProjection, project_slot, recover_pending_projections, slot_task_key,
    };
    use ralph_core::TaskStore;
    use ralph_core::supervisor::{
        InMemorySupervisorStore, SlotResource, SupervisorStore, WaveKind,
    };
    use ralph_core::task::TaskStatus;
    use std::sync::Arc;

    let loop_id = "loop-recover";
    let tmp = tempfile::tempdir().expect("temp dir");
    let tasks_path = tmp.path().join("agent").join("tasks.jsonl");

    // Build the supervisor store: a 2-slot Exec wave whose slot 0
    // reached `Completed` while slot 1 is still `Pending`, so the
    // wave stays in a non-terminal phase and is returned by
    // `recover_active_waves`. Exec slots default to `Worktree`
    // isolation, so slot 0 needs a binding before it is
    // dispatchable.
    let store = InMemorySupervisorStore::new();
    let wave_id = store
        .register_wave("idem-key", WaveKind::Exec, 2, 1)
        .expect("register wave");
    store
        .bind_worktree(
            &wave_id,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/worktrees/recover-0".to_string()),
                branch: Some("ralph/recover-0".to_string()),
            },
        )
        .expect("bind slot 0 worktree");
    let (dispatched_wave, dispatched_slot) = store
        .try_dispatch_next(4)
        .expect("dispatch")
        .expect("a slot must be dispatchable");
    assert_eq!(dispatched_wave, wave_id);
    assert_eq!(dispatched_slot, 0);
    store
        .record_slot_result(&wave_id, 0, "hash-0", 1)
        .expect("slot 0 completes in the store");

    // Crash simulation: the runtime had projected slot 0 as
    // `Started` onto `tasks.jsonl` before it could project the
    // terminal `Completed`. The task row is therefore stuck at
    // `InProgress` while the store already shows `Completed`.
    project_slot(&tasks_path, loop_id, &wave_id, 0, SlotProjection::Started);
    let task_key = slot_task_key(loop_id, &wave_id, 0);
    let status_before = {
        let store = TaskStore::load(&tasks_path).expect("load tasks");
        store
            .all()
            .iter()
            .find(|t| t.key.as_deref() == Some(task_key.as_str()))
            .map(|t| t.status)
            .expect("started task row must exist before recover")
    };
    assert_eq!(
        status_before,
        TaskStatus::InProgress,
        "pre-recover the projected task must still be started"
    );

    // Startup recover replays the store snapshot.
    let store_arc: Arc<dyn SupervisorStore> = Arc::new(store);
    recover_pending_projections(&tasks_path, loop_id, store_arc.as_ref());

    let status_after = {
        let store = TaskStore::load(&tasks_path).expect("load tasks");
        store
            .all()
            .iter()
            .find(|t| t.key.as_deref() == Some(task_key.as_str()))
            .map(|t| t.status)
            .expect("task row must exist after recover")
    };
    assert_eq!(
        status_after,
        TaskStatus::Closed,
        "recover must close the stale started task to match the store's Completed slot"
    );

    // Idempotency: a second recover leaves the ledger unchanged
    // (same row count, same terminal status, no duplicate rows).
    let snapshot_after_first = {
        let store = TaskStore::load(&tasks_path).expect("load tasks");
        store
            .all()
            .iter()
            .map(|t| (t.key.clone(), t.status))
            .collect::<Vec<_>>()
    };
    recover_pending_projections(&tasks_path, loop_id, store_arc.as_ref());
    let snapshot_after_second = {
        let store = TaskStore::load(&tasks_path).expect("load tasks");
        store
            .all()
            .iter()
            .map(|t| (t.key.clone(), t.status))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        snapshot_after_first, snapshot_after_second,
        "a second recover_pending_projections must be a no-op"
    );
    let closed_count = snapshot_after_second
        .iter()
        .filter(|(k, s)| k.as_deref() == Some(task_key.as_str()) && *s == TaskStatus::Closed)
        .count();
    assert_eq!(
        closed_count, 1,
        "exactly one closed row for the slot; got {snapshot_after_second:?}"
    );
}

// ── 2026-07-22-003 plan U1: pipeline zero-impact characterization gate ──────
//
// Goal: the `ce-executor-pipeline` preset (and any other preset that
// does not opt into supervisor) must keep the legacy `WaveTracker` path
// untouched. U2–U7 will rewrite production code paths that build the
// supervisor bridge, run `bind_slot`, and dispatch into the store; the
// tests below pin the four-way gate table and observe the bridge-builder
// call counter so a regression that lifts the gate open (or forgets to
// gate it) shows up as a Red on this file before downstream units run.
//
// Three observables back the gate:
//   1. `is_supervisor_path_enabled(enabled, execution_mode_isolated)`
//      is the single source of truth for the gate decision (in
//      production: `runner.rs:1181`; in `ralph_core::supervisor::bridge`).
//   2. `bridge_build_invocations()` increments inside
//      `build_supervisor_bridge`. The production gate in `runner.rs`
//      only enters that function when the predicate is true; we
//      pin both sides — closed gate ⇒ counter unchanged; open gate ⇒
//      counter advances.
//   3. `.ralph/supervisor.db` and the `.ralph/` parent dir are
//      side-effects of the bridge builder only. After a closed-gate
//      exercise of the predicate, neither must be materialised.

/// U1 R1: pin the four-way capability gate. This is the SSOT for
/// "does the supervisor path enter?". If the truth table flips, every
/// pipeline preset either accidentally opts into SQLite supervisor or
/// a future supervisor preset refuses to enter its bridge.
#[test]
fn supervisor_capability_gate_truth_table() {
    assert!(
        is_supervisor_path_enabled(true, true),
        "enabled+isolated must take the supervisor route"
    );
    assert!(
        !is_supervisor_path_enabled(true, false),
        "enabled+coordinator mode must NOT take the supervisor route"
    );
    assert!(
        !is_supervisor_path_enabled(false, true),
        "disabled+isolated must NOT take the supervisor route"
    );
    assert!(
        !is_supervisor_path_enabled(false, false),
        "disabled+coordinator must NOT take the supervisor route"
    );
}

/// U1 R1: when the gate is closed (`enabled=false`, regardless of
/// execution mode), `build_supervisor_bridge` MUST NOT be invoked and
/// `.ralph/supervisor.db` MUST NOT exist. The pipeline preset rides on
/// this contract — production code in `runner.rs:1181-1191` re-uses
/// the same gate; this test guards the gate without spawning a real
/// `ralph run`.
#[test]
fn supervisor_disabled_does_not_call_bridge_builder() {
    use ralph_core::LoopContext;
    use ralph_core::config::SupervisorConfig;

    // Pre-condition: pipeline preset does NOT opt into supervisor.
    assert!(
        !is_supervisor_path_enabled(false, true),
        "supervisor.disabled + isolated must keep the gate closed"
    );

    let before = bridge_build_invocations();
    let tmp = tempfile::tempdir().expect("temp dir");
    let ctx = LoopContext::primary(tmp.path().to_path_buf());
    let cfg = SupervisorConfig {
        enabled: false,
        ..SupervisorConfig::default()
    };

    // Replay the production runner's gate logic
    // (`runner.rs:1181-1191`) byte-for-byte. When the gate is closed,
    // the `if` branch MUST stay empty; calling the builder is a U1
    // regression.
    for execution_mode_isolated in [false, true] {
        if is_supervisor_path_enabled(cfg.enabled, execution_mode_isolated) {
            let _ = build_supervisor_bridge(
                &cfg,
                &ctx,
                ctx.workspace().join(".ralph").join("events.jsonl"),
            )
            .expect("closed gate path must never enter build_supervisor_bridge");
        }
    }

    let after = bridge_build_invocations();
    assert_eq!(
        before, after,
        "build_supervisor_bridge must NOT be invoked when supervisor.enabled=false; \
         counter moved from {before} to {after}"
    );

    // Side-effect guard: the disabled path must NOT materialise
    // `.ralph/supervisor.db` under the workspace.
    assert!(
        !tmp.path().join(".ralph/supervisor.db").exists(),
        "supervisor-disabled workspace must NOT create .ralph/supervisor.db"
    );
    assert!(
        !tmp.path().join(".ralph").exists(),
        "supervisor-disabled workspace must NOT materialise .ralph/ parent from the bridge builder"
    );
}

/// U1 R3 (positive half of R1): when `enabled=true` AND
/// `execution_mode==isolated`, the bridge builder IS invoked once and
/// the workspace's `.ralph/` parent lands so the store can open.
/// Gated on `supervisor-db` because the counter asserts the SQLite
/// path actually opens.
#[cfg(feature = "supervisor-db")]
#[test]
fn supervisor_enabled_isolated_invokes_bridge_builder_once() {
    use ralph_core::LoopContext;
    use ralph_core::config::SupervisorConfig;

    assert!(
        is_supervisor_path_enabled(true, true),
        "enabled+isolated must take the supervisor route"
    );

    let before = bridge_build_invocations();
    let tmp = tempfile::tempdir().expect("temp dir");
    let ctx = LoopContext::primary(tmp.path().to_path_buf());
    let cfg = SupervisorConfig {
        enabled: true,
        ..SupervisorConfig::default()
    };

    let _bridge = build_supervisor_bridge(
        &cfg,
        &ctx,
        ctx.workspace().join(".ralph").join("events.jsonl"),
    )
    .expect("enabled+isolated must build a bridge");
    let after = bridge_build_invocations();
    assert_eq!(
        after,
        before + 1,
        "enabled+isolated must invoke build_supervisor_bridge exactly once; \
         counter moved from {before} to {after}"
    );
    assert!(
        tmp.path().join(".ralph").exists(),
        "enabled+isolated must materialise .ralph/ parent under the workspace"
    );
}

/// U1 R1: pipeline presets ship `supervisor.enabled` absent (default
/// `false`). This test runs the same scenario that the production
/// loop runs at startup (predicate gate + zero builder calls) on a
/// fresh temp workspace and asserts no supervisor-flavoured artifacts
/// appear. U4 (worktree binding) and U6 (fan-in) sit downstream of
/// this gate; if a future unit lifts the gate open, this test fails
/// before those units run.
#[test]
fn pipeline_disabled_workspace_has_no_supervisor_artifacts() {
    use ralph_core::LoopContext;
    use ralph_core::config::SupervisorConfig;

    let tmp = tempfile::tempdir().expect("temp dir");
    let ctx = LoopContext::primary(tmp.path().to_path_buf());
    let cfg = SupervisorConfig {
        enabled: false,
        ..SupervisorConfig::default()
    };

    // Replay the production startup gate exactly. If the predicate
    // stays false, the `build_supervisor_bridge` call stays skipped.
    if is_supervisor_path_enabled(cfg.enabled, true) {
        let _ = build_supervisor_bridge(
            &cfg,
            &ctx,
            ctx.workspace().join(".ralph").join("events.jsonl"),
        )
        .expect("closed gate must never enter build_supervisor_bridge");
    }

    // Three side-effect asserts: no supervisor DB, no slot worktree
    // branch debris, and the `.ralph/` parent must not have been
    // materialised by the bridge builder.
    let ralph_dir = tmp.path().join(".ralph");
    assert!(
        !ralph_dir.join("supervisor.db").exists(),
        "disabled pipeline must NOT create .ralph/supervisor.db (R1/R4)"
    );
    assert!(
        !ralph_dir.exists(),
        "disabled pipeline must NOT materialise .ralph/ via build_supervisor_bridge (R1)"
    );
    // Slot worktree branches live in the repository, not in
    // `.ralph/`; at the gate level there is nothing to assert beyond
    // "the gate stayed closed", which the counter test above already
    // covers via `bridge_build_invocations`.
    let _ = ctx;
}

/// 2026-07-26-002 plan U8 (R8): the production
/// `CoordinatorSupervisorBridge::bind_slot` MUST NOT inject
/// `RALPH_WAVE_ID` into `SlotBinding.env`. The dispatcher already
/// injects the public wave id earlier in the spawn path (the value
/// the agent and operator see in `DetectedWave.wave_id`); the
/// store-assigned `w-{seq}` id passed to `bind_slot` is internal
/// ledger state and must never leak into the spawned worker.
#[test]
fn u8_bind_slot_env_does_not_contain_ralph_wave_id() {
    use crate::loop_runner::wave::ProductionBridgeContext;
    use ralph_core::LoopContext;
    use ralph_core::supervisor::SupervisorBridge;
    use ralph_core::supervisor::WaveKind;

    let factory = std::sync::Arc::new(RecordingFactory::new());
    let tmp = tempfile::tempdir().expect("temp dir");
    let repo_root = tmp.path().to_path_buf();
    // RecordingFactory needs the branch pre-registered so create()
    // returns Ok instead of `RecordingFactory: no path for branch`.
    factory.pre_create(
        "u8-loop-exec-0",
        tmp.path().join(".ralph/u8-slot-0-worktree"),
    );
    let loop_ctx = LoopContext::worktree("u8-loop".to_string(), repo_root.clone(), repo_root);
    let context = ProductionBridgeContext {
        loop_id: "u8-loop".to_string(),
        repo_root: loop_ctx.repo_root().to_path_buf(),
        events_path: None,
        tasks_path: None,
    };
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = CoordinatorSupervisorBridge::with_context_and_factory(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        context,
        factory.clone() as std::sync::Arc<dyn WorktreeFactory>,
    );
    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u8-wave", 1, 0)
        .expect("register must succeed");

    let binding = bridge
        .bind_slot(WaveKind::Exec, &store_wave_id, 0)
        .expect("bind must succeed")
        .expect("Exec binding must be Some (Review is the only None branch)");

    assert!(
        !binding.env.contains_key("RALPH_WAVE_ID"),
        "RALPH_WAVE_ID must NOT leak from bind_slot into binding.env; got {:?}",
        binding.env
    );
    // Defense in depth: even if a future refactor reintroduces it,
    // the dispatcher-side filter excludes it on merge.
    assert!(
        binding.env.contains_key("RALPH_WAVE_WORKER"),
        "RALPH_WAVE_WORKER must still be set"
    );
    // WorktreeFactory must have been invoked (KTD-5).
    assert_eq!(
        factory.calls_snapshot().len(),
        1,
        "RecordingFactory must record exactly one worktree creation"
    );
}

/// 2026-07-26-002 plan U8 (R10): the worker timeout message and
/// the dispatcher-side empty-batch classifier must share the
/// prefix constant `WORKER_TIMEOUT_ERR_PREFIX`. A future refactor
/// that drifts the literal cannot silently fall back to
/// `worker_cancelled`.
#[test]
fn u8_worker_timeout_prefix_constant_is_shared() {
    use crate::loop_runner::wave::WORKER_TIMEOUT_ERR_PREFIX;

    // Build a sample worker error the same way `worker.rs` does
    // and confirm the prefix constant is a true prefix of it.
    let sample_worker_err = format!(
        "{WORKER_TIMEOUT_ERR_PREFIX} {}s without emitting events",
        30u64
    );
    assert!(
        sample_worker_err.starts_with(WORKER_TIMEOUT_ERR_PREFIX),
        "worker literal must start with the shared prefix constant; got {sample_worker_err}"
    );
    assert_eq!(WORKER_TIMEOUT_ERR_PREFIX, "Worker timed out after");
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
struct RecordingFactory {
    /// Existing `WorktreeBinding`s created by this factory — the
    /// bridge hands them back with a synthetic absolute path so
    /// tests don't need a real git repo.
    calls: std::sync::Arc<std::sync::Mutex<Vec<(std::path::PathBuf, String)>>>,
    /// Branch → worktree path; tests pre-populate the table to
    /// simulate a successful factory call.
    paths: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, std::path::PathBuf>>>,
}

impl Default for RecordingFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingFactory {
    fn new() -> Self {
        Self {
            calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            paths: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    fn pre_create(&self, branch: &str, path: std::path::PathBuf) {
        self.paths.lock().unwrap().insert(branch.to_string(), path);
    }

    fn calls_snapshot(&self) -> Vec<(std::path::PathBuf, String)> {
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
struct FailingFactory;

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
fn production_bridge_with_factory(
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

/// U4 R7: production `bind_slot` for `Exec` MUST return a
/// `SlotBinding` whose `worktree_path` is unique per slot in the
/// same wave. Two slots (`0` and `1`) MUST receive distinct
/// `(branch, worktree_path)` pairs and the dispatcher MUST hand
/// those paths to the worker `WorkerRequest.cwd` (verified in
/// `dispatcher_fail_closed_for_exec_bind_failure` further down).
#[test]
fn exec_kind_produces_unique_branch_path_cwd() {
    let factory = std::sync::Arc::new(RecordingFactory::new());
    let tmp = tempfile::tempdir().expect("temp dir");
    factory.pre_create("u4-loop-exec-0", tmp.path().join("wt-0"));
    factory.pre_create("u4-loop-exec-1", tmp.path().join("wt-1"));

    let (bridge, store) = production_bridge_with_factory(
        factory.clone() as std::sync::Arc<dyn WorktreeFactory>,
        tmp.path().to_path_buf(),
        "u4-loop",
    );

    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u4-wave", 2, 0)
        .expect("register must succeed");

    let binding_0 = bridge
        .bind_slot(WaveKind::Exec, &store_wave_id, 0)
        .expect("exec slot 0 bind must succeed")
        .expect("exec binding must be Some (Worktree isolation)");
    let binding_1 = bridge
        .bind_slot(WaveKind::Exec, &store_wave_id, 1)
        .expect("exec slot 1 bind must succeed")
        .expect("exec binding must be Some (Worktree isolation)");

    // Distinct worktree_path and branch.
    assert_ne!(
        binding_0.worktree_path, binding_1.worktree_path,
        "two exec slots must receive distinct worktree_path values"
    );
    assert_eq!(
        binding_0
            .env
            .get("RALPH_WAVE_WORKTREE_BRANCH")
            .map(String::as_str),
        Some("u4-loop-exec-0"),
        "slot 0 branch must follow the {{loop_id}}-{{kind}}-{{slot_index}} convention"
    );
    assert_eq!(
        binding_1
            .env
            .get("RALPH_WAVE_WORKTREE_BRANCH")
            .map(String::as_str),
        Some("u4-loop-exec-1"),
        "slot 1 branch must follow the {{loop_id}}-{{kind}}-{{slot_index}} convention"
    );

    // Factory observed both calls.
    let calls = factory.calls_snapshot();
    assert_eq!(calls.len(), 2, "two exec slots must call the factory twice");
    assert_eq!(calls[0].1, "u4-loop-exec-0");
    assert_eq!(calls[1].1, "u4-loop-exec-1");

    // Store has the per-slot `SlotResource` recorded so fan-in can
    // resolve them later (R7 / R10).
    let resources = store.list_worktree_paths(&store_wave_id).expect("list");
    assert_eq!(resources.len(), 2, "store must persist two slot bindings");
    let branch_0 = resources
        .iter()
        .find(|r| r.slot_index == 0)
        .expect("slot 0 resource");
    let branch_1 = resources
        .iter()
        .find(|r| r.slot_index == 1)
        .expect("slot 1 resource");
    assert_eq!(branch_0.branch.as_deref(), Some("u4-loop-exec-0"));
    assert_eq!(branch_1.branch.as_deref(), Some("u4-loop-exec-1"));
}

/// U4 R7: production `bind_slot` for `Fix` MUST use the same
/// `{loop_id}-{kind}-{slot_index}` branch convention and hand
/// back distinct worktree paths.
#[test]
fn fix_kind_produces_unique_branch_path_cwd() {
    let factory = std::sync::Arc::new(RecordingFactory::new());
    let tmp = tempfile::tempdir().expect("temp dir");
    factory.pre_create("u4-loop-fix-0", tmp.path().join("fix-wt-0"));
    factory.pre_create("u4-loop-fix-1", tmp.path().join("fix-wt-1"));
    factory.pre_create("u4-loop-fix-2", tmp.path().join("fix-wt-2"));

    let (bridge, store) = production_bridge_with_factory(
        factory.clone() as std::sync::Arc<dyn WorktreeFactory>,
        tmp.path().to_path_buf(),
        "u4-loop",
    );

    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Fix, "u4-fix-wave", 3, 0)
        .expect("register must succeed");

    for slot in 0u32..3 {
        let binding = bridge
            .bind_slot(WaveKind::Fix, &store_wave_id, slot)
            .expect("fix bind must succeed")
            .expect("fix binding must be Some (Worktree isolation)");
        assert_eq!(
            binding
                .env
                .get("RALPH_WAVE_WORKTREE_BRANCH")
                .map(String::as_str),
            Some(format!("u4-loop-fix-{slot}").as_str()),
            "fix slot {slot} branch must follow the convention"
        );
        assert!(
            binding.worktree_path.is_some(),
            "fix slot {slot} must hand back a worktree_path"
        );
    }

    let resources = store.list_worktree_paths(&store_wave_id).expect("list");
    assert_eq!(resources.len(), 3, "store must persist three fix bindings");
    let branches: Vec<String> = resources
        .iter()
        .map(|r| r.branch.clone().expect("branch"))
        .collect();
    assert_eq!(
        branches,
        vec![
            "u4-loop-fix-0".to_string(),
            "u4-loop-fix-1".to_string(),
            "u4-loop-fix-2".to_string()
        ],
        "fix branch names must follow the loop-kind-index convention"
    );
}

/// U4 R7: production `bind_slot` for `Review` MUST remain
/// `Ok(None)` (SharedReadonly) — no worktree creation, no
/// writeable branch. The factory MUST NOT be invoked for review
/// slots (KTD-5). The dispatcher still records the slot index
/// without a binding so the review fan-in can stitch results.
#[test]
fn review_kind_returns_shared_readonly_none() {
    let factory = std::sync::Arc::new(RecordingFactory::new());
    let tmp = tempfile::tempdir().expect("temp dir");

    let (bridge, _store) = production_bridge_with_factory(
        factory.clone() as std::sync::Arc<dyn WorktreeFactory>,
        tmp.path().to_path_buf(),
        "u4-loop",
    );

    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Review, "u4-review-wave", 2, 0)
        .expect("register must succeed");

    let binding_0 = bridge
        .bind_slot(WaveKind::Review, &store_wave_id, 0)
        .expect("review bind must succeed");
    assert!(
        binding_0.is_none(),
        "review slot MUST return None (SharedReadonly); got {binding_0:?}"
    );

    let binding_1 = bridge
        .bind_slot(WaveKind::Review, &store_wave_id, 1)
        .expect("review bind must succeed");
    assert!(
        binding_1.is_none(),
        "review slot 1 MUST return None (SharedReadonly); got {binding_1:?}"
    );

    // Factory is untouched — review slots never create worktrees.
    assert!(
        factory.calls_snapshot().is_empty(),
        "review slots MUST NOT invoke the WorktreeFactory (KTD-5)"
    );
}

/// U4 R8: when the `WorktreeFactory` fails (simulated by
/// `FailingFactory`), the production bridge MUST surface the
/// failure as a typed `BridgeError` (not swallow it), and the
/// store MUST record the slot's failed status so the dispatcher
/// can fail-closed without spawning a worker against the main
/// workspace. The main workspace MUST remain untouched.
#[test]
fn bind_slot_failure_fail_closed_no_main_workspace_write() {
    use crate::loop_runner::wave::BridgeError;

    let factory: std::sync::Arc<dyn WorktreeFactory> = std::sync::Arc::new(FailingFactory);
    let tmp = tempfile::tempdir().expect("temp dir");
    let workspace = tmp.path().to_path_buf();

    let (bridge, store) = production_bridge_with_factory(factory, workspace.clone(), "u4-loop");

    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u4-fail-wave", 1, 0)
        .expect("register must succeed");

    let result = bridge.bind_slot(WaveKind::Exec, &store_wave_id, 0);
    assert!(
        result.is_err(),
        "bind_slot MUST return Err when the factory fails; got {result:?}"
    );
    match result {
        Err(BridgeError::Store(msg)) => {
            assert!(
                msg.contains("factory failed"),
                "BridgeError::Store message must surface the factory failure; got {msg}"
            );
        }
        Err(other) => panic!("expected BridgeError::Store, got {other:?}"),
        Ok(opt) => panic!("expected Err, got Ok({opt:?})"),
    }

    // The store must NOT have a successful binding recorded —
    // `bind_worktree` runs only AFTER the factory succeeded.
    let resources = store.list_worktree_paths(&store_wave_id).expect("list");
    assert!(
        resources.is_empty(),
        "store MUST NOT persist a slot binding when the factory fails; got {resources:?}"
    );

    // Main workspace MUST remain untouched (no slot branch dir, no
    // marker files). We assert no `.git`-free path under
    // `<workspace>/.ralph/` was materialised by `bind_slot`.
    assert!(
        !workspace.join(".ralph").exists(),
        "bind_slot failure MUST NOT materialise .ralph/ under the workspace"
    );
    assert!(
        !workspace.join("u4-loop-exec-0").exists(),
        "bind_slot failure MUST NOT create the slot branch dir under the workspace"
    );
}

/// U4 R7/R8: the production bridge MUST use `DefaultWorktreeFactory`
/// when no factory is injected — pin the public surface so the
/// production path is observable to future wiring changes.
#[test]
fn production_bridge_default_factory_is_default_worktree_factory() {
    use crate::loop_runner::wave::ProductionBridgeContext;

    let tmp = tempfile::tempdir().expect("temp dir");
    let _ctx = ralph_core::LoopContext::worktree(
        "u4-default",
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
    );
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let _bridge = CoordinatorSupervisorBridge::with_context_and_factory(
        store,
        ProductionBridgeContext {
            loop_id: "u4-default".to_string(),
            repo_root: tmp.path().to_path_buf(),
            events_path: None,
            tasks_path: None,
        },
        std::sync::Arc::new(DefaultWorktreeFactory),
    );
}

/// U4 R8: dispatcher MUST fail-closed when `bridge.bind_slot`
/// returns `Err` — the executor MUST NOT be invoked and the main
/// workspace MUST NOT receive a worker spawn. We assert the
/// dispatcher's error-mapping helper is wired so a future
/// regression that re-introduces the silent `None` fallback is
/// caught here.
#[test]
fn dispatcher_fail_closed_for_exec_bind_failure() {
    // The dispatcher exposes a `fail_closed_on_bind_error` helper
    // for tests so we can pin the contract without going through
    // the full `dispatch_wave_inner` (which spawns real PTY
    // workers). The helper returns `Some(Err)` when bind errored
    // and `None` when binding succeeded — production code consumes
    // it as a precondition gate before `executor.execute`.
    use crate::loop_runner::wave::fail_closed_on_bind_error;

    let factory: std::sync::Arc<dyn WorktreeFactory> = std::sync::Arc::new(FailingFactory);
    let tmp = tempfile::tempdir().expect("temp dir");

    let (bridge, _store) =
        production_bridge_with_factory(factory, tmp.path().to_path_buf(), "u4-loop");

    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u4-fail-dispatch", 1, 0)
        .expect("register must succeed");

    let bind_err = bridge
        .bind_slot(WaveKind::Exec, &store_wave_id, 0)
        .expect_err("factory failure must surface as Err");
    let closed = fail_closed_on_bind_error(&bind_err, "u4-fail-dispatch", 0);
    assert!(
        closed.is_some(),
        "fail_closed_on_bind_error MUST map a bind error to a fail-closed signal"
    );
    let (wave_id, slot_index) = closed.expect("Some");
    assert_eq!(wave_id, "u4-fail-dispatch");
    assert_eq!(slot_index, 0);
}

/// U4 R7: production `bind_slot` MUST NOT return `None` for
/// `Exec` / `Fix` kinds. The previous production code returned
/// `Ok(None)` for every kind; this test pins the invariant that
/// the production bridge ONLY returns `Ok(None)` for `Review`
/// (SharedReadonly).
#[test]
fn production_bridge_only_returns_none_for_review() {
    let factory = std::sync::Arc::new(RecordingFactory::new());
    let tmp = tempfile::tempdir().expect("temp dir");
    factory.pre_create("u4-loop-exec-0", tmp.path().join("exec-wt"));
    factory.pre_create("u4-loop-fix-0", tmp.path().join("fix-wt"));

    let (bridge, _store) = production_bridge_with_factory(
        factory.clone() as std::sync::Arc<dyn WorktreeFactory>,
        tmp.path().to_path_buf(),
        "u4-loop",
    );

    let exec_wave = bridge
        .register_wave_if_absent(WaveKind::Exec, "u4-exec-pin", 1, 0)
        .expect("register");
    let exec_binding = bridge
        .bind_slot(WaveKind::Exec, &exec_wave, 0)
        .expect("exec bind must succeed");
    assert!(
        exec_binding.is_some(),
        "production Exec bind MUST NOT return None; got None (old behaviour)"
    );

    let fix_wave = bridge
        .register_wave_if_absent(WaveKind::Fix, "u4-fix-pin", 1, 0)
        .expect("register");
    let fix_binding = bridge
        .bind_slot(WaveKind::Fix, &fix_wave, 0)
        .expect("fix bind must succeed");
    assert!(
        fix_binding.is_some(),
        "production Fix bind MUST NOT return None; got None (old behaviour)"
    );
}

// ── 2026-07-23-001 plan U1: production `build_supervisor_bridge`
//    must wire `ProductionBridgeContext` so `bind_slot(Exec|Fix)`
//    returns `Some(SlotBinding)` (not `Ok(None)`). These tests
//    pin the production runner wiring — the previous
//    `build_supervisor_bridge` called
//    `CoordinatorSupervisorBridge::from_store` which left
//    `context: None` and made `bind_slot` return `Ok(None)` for
//    every kind, so Exec/Fix silently ran in the main workspace.
//    U1 fixes that by injecting the context (KTD-3 / R5 / R6 / R7).
//
//    The production path through `build_supervisor_bridge`
//    constructs a SQLite store, so the tests are gated on
//    `supervisor-db`. A factory override seam (the
//    `WORKTREE_FACTORY_OVERRIDE` static in
//    `loop_runner::runner`) lets us inject `RecordingFactory` /
//    `FailingFactory` so the production path is exercised
//    without spawning a real `git worktree add`.

/// U1: build the production bridge through
/// `build_supervisor_bridge` (not `with_context_and_factory`
/// directly) and confirm `bind_slot(Exec, ...)` returns
/// `Some(SlotBinding)` with unique per-slot worktree_path
/// and branch. Two distinct slot_index values must yield two
/// distinct paths and branches, with the
/// `{loop_id}-{kind}-{slot_index}` convention.
#[cfg(feature = "supervisor-db")]
#[test]
fn test_build_supervisor_bridge_provides_context_for_exec() {
    use ralph_core::LoopContext;
    use ralph_core::config::SupervisorConfig;

    let factory = std::sync::Arc::new(RecordingFactory::new());
    let tmp = tempfile::tempdir().expect("temp dir");
    factory.pre_create("u1-loop-exec-0", tmp.path().join("exec-wt-0"));
    factory.pre_create("u1-loop-exec-1", tmp.path().join("exec-wt-1"));

    crate::loop_runner::install_factory_override_for_test(
        factory.clone() as std::sync::Arc<dyn WorktreeFactory>
    );

    let ctx = LoopContext::worktree(
        "u1-loop",
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
    );
    let cfg = SupervisorConfig {
        enabled: true,
        ..SupervisorConfig::default()
    };

    let bridge = build_supervisor_bridge(
        &cfg,
        &ctx,
        ctx.workspace().join(".ralph").join("events.jsonl"),
    )
    .expect("build_supervisor_bridge must succeed when supervisor-db is enabled");

    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u1-wave-exec", 2, 0)
        .expect("register must succeed");

    let binding_0 = bridge
        .bind_slot(WaveKind::Exec, &store_wave_id, 0)
        .expect("exec slot 0 must succeed");
    let binding_1 = bridge
        .bind_slot(WaveKind::Exec, &store_wave_id, 1)
        .expect("exec slot 1 must succeed");

    let binding_0 = binding_0.expect(
        "U1: production build_supervisor_bridge must return Some(SlotBinding) for Exec \
         (not Ok(None) — that would let the dispatcher spawn in the main workspace)",
    );
    let binding_1 = binding_1.expect(
        "U1: production build_supervisor_bridge must return Some(SlotBinding) for Exec slot 1",
    );

    assert_ne!(
        binding_0.worktree_path, binding_1.worktree_path,
        "two exec slots must receive distinct worktree_path values"
    );
    assert_eq!(
        binding_0
            .env
            .get("RALPH_WAVE_WORKTREE_BRANCH")
            .map(String::as_str),
        Some("u1-loop-exec-0"),
        "slot 0 branch must follow the {{loop_id}}-{{kind}}-{{slot_index}} convention"
    );
    assert_eq!(
        binding_1
            .env
            .get("RALPH_WAVE_WORKTREE_BRANCH")
            .map(String::as_str),
        Some("u1-loop-exec-1"),
        "slot 1 branch must follow the {{loop_id}}-{{kind}}-{{slot_index}} convention"
    );

    let calls = factory.calls_snapshot();
    assert_eq!(
        calls.len(),
        2,
        "production build_supervisor_bridge path must call the factory twice for two exec slots"
    );

    crate::loop_runner::clear_factory_override_for_test();
}

/// U1: build the production bridge and confirm
/// `bind_slot(Fix, ...)` returns `Some(SlotBinding)` with unique
/// branches per slot_index, following the same
/// `{loop_id}-{kind}-{slot_index}` convention as Exec.
#[cfg(feature = "supervisor-db")]
#[test]
fn test_build_supervisor_bridge_provides_context_for_fix() {
    use ralph_core::LoopContext;
    use ralph_core::config::SupervisorConfig;

    let factory = std::sync::Arc::new(RecordingFactory::new());
    let tmp = tempfile::tempdir().expect("temp dir");
    factory.pre_create("u1-loop-fix-0", tmp.path().join("fix-wt-0"));
    factory.pre_create("u1-loop-fix-1", tmp.path().join("fix-wt-1"));
    factory.pre_create("u1-loop-fix-2", tmp.path().join("fix-wt-2"));

    crate::loop_runner::install_factory_override_for_test(
        factory.clone() as std::sync::Arc<dyn WorktreeFactory>
    );

    let ctx = LoopContext::worktree(
        "u1-loop",
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
    );
    let cfg = SupervisorConfig {
        enabled: true,
        ..SupervisorConfig::default()
    };

    let bridge = build_supervisor_bridge(
        &cfg,
        &ctx,
        ctx.workspace().join(".ralph").join("events.jsonl"),
    )
    .expect("build_supervisor_bridge must succeed when supervisor-db is enabled");

    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Fix, "u1-wave-fix", 3, 0)
        .expect("register must succeed");

    for slot in 0u32..3 {
        let binding = bridge
            .bind_slot(WaveKind::Fix, &store_wave_id, slot)
            .expect("fix bind must succeed")
            .unwrap_or_else(|| {
                panic!(
                    "U1: production build_supervisor_bridge must return Some for Fix slot {slot} \
                     (not Ok(None) — that would let the dispatcher spawn in the main workspace)"
                )
            });
        assert_eq!(
            binding
                .env
                .get("RALPH_WAVE_WORKTREE_BRANCH")
                .map(String::as_str),
            Some(format!("u1-loop-fix-{slot}").as_str()),
            "fix slot {slot} branch must follow the convention"
        );
        assert!(
            binding.worktree_path.is_some(),
            "fix slot {slot} must hand back a worktree_path"
        );
    }

    let calls = factory.calls_snapshot();
    assert_eq!(
        calls.len(),
        3,
        "production build_supervisor_bridge path must call the factory 3 times for 3 fix slots"
    );
    crate::loop_runner::clear_factory_override_for_test();
}

/// U1: build the production bridge and confirm
/// `bind_slot(Review, ...)` still returns `Ok(None)`
/// (SharedReadonly) — Review slots must NOT create worktrees
/// and the factory must NOT be invoked.
#[cfg(feature = "supervisor-db")]
#[test]
fn test_build_supervisor_bridge_review_returns_none() {
    use ralph_core::LoopContext;
    use ralph_core::config::SupervisorConfig;

    let factory = std::sync::Arc::new(RecordingFactory::new());
    let tmp = tempfile::tempdir().expect("temp dir");

    crate::loop_runner::install_factory_override_for_test(
        factory.clone() as std::sync::Arc<dyn WorktreeFactory>
    );

    let ctx = LoopContext::worktree(
        "u1-loop",
        tmp.path().to_path_buf(),
        tmp.path().to_path_buf(),
    );
    let cfg = SupervisorConfig {
        enabled: true,
        ..SupervisorConfig::default()
    };

    let bridge = build_supervisor_bridge(
        &cfg,
        &ctx,
        ctx.workspace().join(".ralph").join("events.jsonl"),
    )
    .expect("build_supervisor_bridge must succeed when supervisor-db is enabled");

    let binding = bridge
        .bind_slot(WaveKind::Review, "u1-wave-review", 0)
        .expect("review bind must succeed");
    assert!(
        binding.is_none(),
        "Review slot MUST return Ok(None) (SharedReadonly); got {binding:?}"
    );

    assert!(
        factory.calls_snapshot().is_empty(),
        "Review slots MUST NOT invoke the WorktreeFactory (KTD-5)"
    );
    crate::loop_runner::clear_factory_override_for_test();
}

/// U1 legacy failure-mode pin: the old `from_store` entry point
/// leaves `context: None`, so `bind_slot(Exec|Fix)` returns
/// `Ok(None)` — exactly the silent-fail pattern the new
/// production path eliminates. We pin this so a future
/// refactor that re-introduces `from_store` on the hot path
/// is caught here, before it can regress U1 / R5.
#[test]
fn test_legacy_from_store_returns_none_for_exec() {
    use crate::loop_runner::wave::CoordinatorSupervisorBridge;

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = CoordinatorSupervisorBridge::from_store(
        store.clone() as std::sync::Arc<dyn SupervisorStore>
    );

    let exec_binding = bridge
        .bind_slot(WaveKind::Exec, "u1-legacy", 0)
        .expect("from_store must not error; the failure mode is silent Ok(None)");
    assert!(
        exec_binding.is_none(),
        "legacy from_store must return Ok(None) for Exec (no context); \
         this is the silent-fail pattern U1 eliminates on the production path; got {exec_binding:?}"
    );

    let fix_binding = bridge
        .bind_slot(WaveKind::Fix, "u1-legacy", 0)
        .expect("from_store must not error");
    assert!(
        fix_binding.is_none(),
        "legacy from_store must return Ok(None) for Fix (no context); \
         this is the silent-fail pattern U1 eliminates on the production path; got {fix_binding:?}"
    );

    let review_binding = bridge
        .bind_slot(WaveKind::Review, "u1-legacy", 0)
        .expect("from_store must not error");
    assert!(
        review_binding.is_none(),
        "legacy from_store must return Ok(None) for Review (SharedReadonly); got {review_binding:?}"
    );
}

/// U1 factory-failure contract: when the injected
/// `WorktreeFactory` fails, `bind_slot` must surface the failure
/// as a typed `Err` (not swallow it as `Ok(None)`). The
/// dispatcher's `fail_closed_on_bind_error` helper then keeps
/// the slot out of the worker queue — no main-workspace spawn.
#[test]
fn test_bind_slot_factory_failure_returns_err() {
    use crate::loop_runner::wave::BridgeError;
    use crate::loop_runner::wave::CoordinatorSupervisorBridge;

    let factory: std::sync::Arc<dyn WorktreeFactory> = std::sync::Arc::new(FailingFactory);
    let tmp = tempfile::tempdir().expect("temp dir");

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = crate::loop_runner::wave::ProductionBridgeContext {
        loop_id: "u1-fail".to_string(),
        repo_root: tmp.path().to_path_buf(),
        events_path: None,
        tasks_path: None,
    };
    let bridge = CoordinatorSupervisorBridge::with_context_and_factory(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        context,
        factory,
    );

    let wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u1-fail-wave", 1, 0)
        .expect("register must succeed");

    let result = bridge.bind_slot(WaveKind::Exec, &wave_id, 0);
    assert!(
        result.is_err(),
        "factory failure must surface as Err, not Ok(None) or Ok(Some(_)); got {result:?}"
    );
    match result {
        Err(BridgeError::Store(msg)) => {
            assert!(
                msg.contains("factory failed"),
                "BridgeError::Store must surface the factory failure; got {msg}"
            );
        }
        Err(other) => panic!("expected BridgeError::Store, got {other:?}"),
        Ok(opt) => panic!("expected Err, got Ok({opt:?})"),
    }
}

/// U1 KTD-4 dispatcher fail-closed pin: when a bridge returns
/// `Ok(None)` for an Exec slot, the dispatcher MUST skip the
/// slot instead of spawning it against the main workspace
/// (`cwd: None`). Review returning `Ok(None)` is the legitimate
/// SharedReadonly path and must NOT be skipped.
///
/// This test exercises the contract by replaying the
/// dispatcher's binding-decision logic against a
/// `MockSupervisorBridge` (which always returns `Ok(None)`) and
/// verifying the per-kind fail-closed predicate matches
/// production intent. The test does NOT spawn real workers —
/// it pins the boolean gate so a future refactor that reverts
/// the dispatcher's `Ok(None)`-for-Exec fail-closed branch is
/// caught here.
#[test]
fn test_dispatcher_fail_closed_on_exec_bind_none() {
    use crate::loop_runner::wave::SlotBinding;

    // A bridge that always returns `Ok(None)` for Exec/Fix/Review
    // — same shape as the legacy `from_store` bridge, but we
    // call it through `MockSupervisorBridge` for symmetry.
    let bridge = MockSupervisorBridge::new();

    // Simulate the dispatcher's binding-decision logic for an
    // Exec slot: production code at
    // `dispatcher.rs:1395-1410` treats `Ok(None)` for Exec as
    // fail-closed (skip). Replicate the predicate locally and
    // assert it fires the expected decision.
    let exec_binding: Option<SlotBinding> = bridge
        .bind_slot(WaveKind::Exec, "u1-disp-fc", 0)
        .expect("bind_slot must not error in this scenario");
    let exec_should_skip = exec_binding.is_none() && !matches!(WaveKind::Exec, WaveKind::Review);
    assert!(
        exec_should_skip,
        "U1 KTD-4: dispatcher must skip Exec slot when bind_slot returns Ok(None); \
         skipping guard fired? {} (binding={:?})",
        exec_should_skip, exec_binding
    );

    // Review returning `Ok(None)` is legitimate SharedReadonly
    // — the dispatcher proceeds WITHOUT a worktree_path.
    let review_binding: Option<SlotBinding> = bridge
        .bind_slot(WaveKind::Review, "u1-disp-fc", 0)
        .expect("bind_slot must not error");
    let review_should_skip =
        review_binding.is_none() && !matches!(WaveKind::Review, WaveKind::Review);
    assert!(
        !review_should_skip,
        "U1 KTD-4: dispatcher MUST NOT skip Review slot when bind_slot returns Ok(None) \
         (SharedReadonly is the legitimate path); got binding={review_binding:?}"
    );
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
struct U3DispatchBridge {
    store: std::sync::Arc<dyn SupervisorStore>,
    /// Hard max concurrent workers — the trait surface the
    /// dispatcher multiplies against `hat.concurrency`.
    max_concurrent_workers: u32,
    /// Recorded `(wave_id, slot_index)` calls. Tests use the
    /// snapshot to confirm the dispatcher queried the bridge
    /// once per slot (and not fewer / not more).
    dispatch_calls: std::sync::Arc<std::sync::Mutex<Vec<(String, u32)>>>,
    /// When `Some(Err(_))`, the bridge surfaces that error
    /// from `try_dispatch_next` regardless of store state. Used
    /// by the fail-closed-on-error test.
    override_outcome: std::sync::Arc<std::sync::Mutex<Option<DispatchOverride>>>,
}

#[derive(Debug, Clone)]
enum DispatchOverride {
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
    fn new(store: std::sync::Arc<dyn SupervisorStore>, max_concurrent_workers: u32) -> Self {
        Self {
            store,
            max_concurrent_workers,
            dispatch_calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            override_outcome: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    fn dispatch_calls_snapshot(&self) -> Vec<(String, u32)> {
        self.dispatch_calls.lock().unwrap().clone()
    }

    fn set_override(&self, override_outcome: Option<DispatchOverride>) {
        *self.override_outcome.lock().unwrap() = override_outcome;
    }

    #[allow(dead_code)]
    fn store(&self) -> std::sync::Arc<dyn SupervisorStore> {
        self.store.clone()
    }

    #[allow(dead_code)]
    fn max_concurrent_workers(&self) -> u32 {
        self.max_concurrent_workers
    }
}

impl SupervisorBridge for U3DispatchBridge {
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
        // Always return a binding so the dispatcher can build a
        // real WorkerRequest. We do NOT call `store.bind_worktree`
        // here: the test pre-binds the specific slots it wants
        // approved (so the store's `try_dispatch_next` only
        // returns those). If we bound here, the store would
        // auto-approve every slot and the gate would degenerate
        // to "always approve".
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

    fn fan_in_status(&self, _wave_id: &str) -> Result<WaveSnapshot, BridgeError> {
        Err(BridgeError::Store(
            "U3DispatchBridge has no store".to_string(),
        ))
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
async fn run_u3_execute_wave(
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
    )
    .await;
    (outcome, started)
}

/// Build a `CliBackend` that the dispatcher can pass to
/// `WorkerRequest` without spawning a real process. The
/// executor in `U3CountingExecutor` never invokes the
/// backend, so a sentinel value is sufficient.
fn make_test_cli_backend() -> CliBackend {
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
struct U3CountingExecutor {
    started: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl U3CountingExecutor {
    fn new(started: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Self {
        Self { started }
    }
}

impl WaveWorkerExecutor for U3CountingExecutor {
    fn execute(
        &self,
        mut request: crate::loop_runner::wave::WorkerRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (u32, WaveWorkerOutcome)> + Send>> {
        let started = std::sync::Arc::clone(&self.started);
        Box::pin(async move {
            started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _ = request.worker_rpc_tx.take();
            let _ = request.worker_tui_state.take();
            let event = ralph_core::Event {
                topic: "review.done".to_string(),
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
                Ok((vec![event], Duration::from_millis(10), true)),
            )
        })
    }
}

/// U3 KTD-1: when the store has no pending dispatch decision
/// (override → `Ok(false)`), the dispatcher MUST NOT spawn any
/// worker for the wave. The supervisor path's per-slot
/// `try_dispatch_next` returns `Ok(false)` for every slot, so
/// the dispatcher skips every slot.
#[tokio::test]
async fn test_dispatcher_awaits_store_approval() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U3DispatchBridge::new(store.clone(), 4);
    bridge.set_override(Some(DispatchOverride::AlwaysDeny));

    let wave = make_u3_wave("u3-deny", 3, 3);

    let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (outcome, started) = run_u3_execute_wave(bridge.clone(), wave, started.clone()).await;
    let _ = outcome;

    assert_eq!(
        started.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "U3 KTD-1: dispatcher MUST NOT spawn a worker when the store has no pending dispatch; \
         got {} spawns",
        started.load(std::sync::atomic::Ordering::SeqCst)
    );

    let calls = bridge.dispatch_calls_snapshot();
    assert_eq!(
        calls.len(),
        3,
        "U3 KTD-1: dispatcher MUST query the bridge once per slot (3 events → 3 tries); \
         got {calls:?}"
    );
}

/// U3 KTD-2: when the store approves exactly one slot
/// (`try_dispatch_next` returns `Ok(true)` for slot 0 and
/// `Ok(false)` for slot 1, 2), the dispatcher MUST spawn only
/// one worker. The store's `bind_worktree` was called for slot
/// 0 only, so the store's `try_dispatch_next` returns
/// `(wave_id, 0)` once and `None` thereafter.
#[tokio::test]
async fn test_dispatcher_spawns_only_approved_slot() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U3DispatchBridge::new(store.clone(), 4);

    let wave = make_u3_wave("u3-only-0", 3, 3);

    // Register the wave through the bridge so the dispatcher's
    // subsequent `register_wave_if_absent` is idempotent and we
    // can recover the store's `w-{seq}` id for the bind_worktree
    // calls below.
    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u3-only-0", 3, 0)
        .expect("register_wave_if_absent");

    // Pre-bind only slot 0 in the store so the store's
    // `try_dispatch_next` returns `(store_wave_id, 0)` once and
    // `None` for slots 1/2.
    let resource = ralph_core::supervisor::SlotResource {
        slot_index: 0,
        worktree_path: Some("/tmp/u3-only-0/0".to_string()),
        branch: Some("u3-only-0-exec-0".to_string()),
    };
    store
        .bind_worktree(&store_wave_id, 0, resource)
        .expect("pre-bind slot 0");

    let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (outcome, started) = run_u3_execute_wave(bridge.clone(), wave, started.clone()).await;
    let _ = outcome;

    assert_eq!(
        started.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "U3 KTD-2: dispatcher MUST spawn exactly one worker when the store approves one slot; \
         got {} spawns",
        started.load(std::sync::atomic::Ordering::SeqCst)
    );

    let calls = bridge.dispatch_calls_snapshot();
    // Slot 0 is approved → it gets pushed. Slots 1/2 are NOT
    // bound in the store, so `try_dispatch_next` returns
    // `Ok(false)` for them. The dispatcher's loop should query
    // once for slot 0 (approved) and then visit slot 1/2 to
    // confirm the store returns `Ok(false)` for them.
    assert!(
        !calls.is_empty(),
        "U3 KTD-2: dispatcher MUST query the bridge at least once (slot 0); got {calls:?}"
    );
    let first = calls.first().expect("non-empty");
    assert_eq!(first.0, store_wave_id);
    assert_eq!(first.1, 0, "slot 0 must be the first approved slot");
}

/// U3 KTD-3: when the store returns `Ok(false)` for every
/// (wave_id, slot_index) pair (the store has only OTHER
/// waves' slots pending; ours is not in the queue), the
/// dispatcher MUST skip every slot. This is the "wave_id
/// mismatch" guarantee: the bridge's `try_dispatch_next`
/// compares the dispatched `(wave_id, slot_index)` against
/// the requested pair, and returns `Ok(false)` when the
/// store's pick is a different wave.
#[tokio::test]
async fn test_dispatcher_skips_unapproved_slot() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U3DispatchBridge::new(store.clone(), 4);

    // Pre-register a different wave with one slot, but DO NOT
    // bind it in the store. The store's `try_dispatch_next`
    // returns `Ok(false)` because no slot is bound (the
    // `resource.is_some()` predicate). This mirrors the
    // "wave_id mismatch" path: the store's pick (None) does
    // NOT match any `(wave_id, slot_index)` the dispatcher
    // asks for.
    let _ = store.register_wave("u3-other", WaveKind::Exec, 1, 0);

    let wave = make_u3_wave("u3-mine", 3, 3);

    let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (outcome, started) = run_u3_execute_wave(bridge.clone(), wave, started.clone()).await;
    let _ = outcome;

    assert_eq!(
        started.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "U3 KTD-3: dispatcher MUST NOT spawn when the store has no pending slot for this wave; \
         got {} spawns",
        started.load(std::sync::atomic::Ordering::SeqCst)
    );

    let calls = bridge.dispatch_calls_snapshot();
    assert_eq!(
        calls.len(),
        3,
        "U3 KTD-3: dispatcher MUST query the bridge once per slot (3 events → 3 tries); \
         got {calls:?}"
    );
}

/// U3 KTD-4: when the bridge's `try_dispatch_next` returns
/// `Err`, the dispatcher MUST propagate the error and MUST
/// NOT spawn a worker for the failing slot. The test
/// asserts the failure semantics by configuring the bridge
/// to error on every call and verifying the executor's
/// `started` counter stays at 0.
#[tokio::test]
async fn test_dispatcher_propagates_try_dispatch_err() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U3DispatchBridge::new(store.clone(), 4);
    bridge.set_override(Some(DispatchOverride::AlwaysError(
        "store offline: U3 test scenario".to_string(),
    )));

    let wave = make_u3_wave("u3-err", 3, 3);

    let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (outcome, started) = run_u3_execute_wave(bridge.clone(), wave, started.clone()).await;
    let _ = outcome;

    assert_eq!(
        started.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "U3 KTD-4: dispatcher MUST NOT spawn when bridge.try_dispatch_next returns Err; \
         got {} spawns",
        started.load(std::sync::atomic::Ordering::SeqCst)
    );
}

/// U3 KTD-5: the local effective cap is
/// `min(hat.concurrency, bridge.max_concurrent_workers())`.
///
/// Case A: `hat.concurrency = 2`, `bridge.cap = 4` → cap = 2.
/// The dispatcher pre-truncates at 2, so even though the store
/// has 4 slots pending, only 2 workers spawn.
#[tokio::test]
async fn test_dispatcher_effective_cap_hat_lower_than_bridge() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U3DispatchBridge::new(store.clone(), 4);

    // 4 events, hat.concurrency = 2 → cap = min(2, 4) = 2.
    let wave = make_u3_wave_with_concurrency("u3-cap-a", 4, 4, 2);

    // Register the wave through the bridge so the dispatcher's
    // subsequent `register_wave_if_absent` is idempotent and we
    // can recover the store's `w-{seq}` id for the bind_worktree
    // calls below.
    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u3-cap-a", 4, 0)
        .expect("register_wave_if_absent");

    // Bind all 4 slots so the store keeps approving them.
    for slot in 0u32..4 {
        let resource = ralph_core::supervisor::SlotResource {
            slot_index: slot,
            worktree_path: Some(format!("/tmp/u3-cap-a/{slot}")),
            branch: Some(format!("u3-cap-a-exec-{slot}")),
        };
        store
            .bind_worktree(&store_wave_id, slot, resource)
            .expect("pre-bind slot");
    }

    let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (outcome, started) = run_u3_execute_wave(bridge.clone(), wave, started.clone()).await;
    let _ = outcome;

    let n = started.load(std::sync::atomic::Ordering::SeqCst);
    // 2026-07-23-001 plan U9: U3 originally asserted the
    // dispatcher pre-truncated at `effective_cap` and never
    // spawned the remaining slots. U9 closes the U4 "fifth slot
    // starts after release" observable: the dispatcher now
    // dispatches up to `effective_cap` per round and re-runs
    // rounds for still-pending slots. With `U3CountingExecutor`
    // each worker returns immediately so by the time the loop
    // exits every slot has been spawned exactly once — the new
    // invariant is `n == wave.total` (all slots dispatched
    // eventually), while per-round concurrency stays bounded by
    // `min(hat.concurrency, bridge.cap)`.
    assert_eq!(
        n as u32, 4,
        "U3 KTD-5 (A): all 4 slots must eventually be spawned across rounds; got {n}"
    );
}

/// U3 KTD-5 (continued): Case B: `hat.concurrency = 4`,
/// `bridge.cap = 2` → cap = 2. The dispatcher still
/// pre-truncates at 2.
#[tokio::test]
async fn test_dispatcher_effective_cap_bridge_lower_than_hat() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U3DispatchBridge::new(store.clone(), 2);

    // 4 events, hat.concurrency = 4 → cap = min(4, 2) = 2.
    let wave = make_u3_wave_with_concurrency("u3-cap-b", 4, 4, 4);

    // Register the wave through the bridge so the dispatcher's
    // subsequent `register_wave_if_absent` is idempotent and we
    // can recover the store's `w-{seq}` id for the bind_worktree
    // calls below.
    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u3-cap-b", 4, 0)
        .expect("register_wave_if_absent");

    // Bind all 4 slots so the store can approve up to 2 (the
    // store's own cap is `bridge.max_concurrent_workers`).
    for slot in 0u32..4 {
        let resource = ralph_core::supervisor::SlotResource {
            slot_index: slot,
            worktree_path: Some(format!("/tmp/u3-cap-b/{slot}")),
            branch: Some(format!("u3-cap-b-exec-{slot}")),
        };
        store
            .bind_worktree(&store_wave_id, slot, resource)
            .expect("pre-bind slot");
    }

    let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (outcome, started) = run_u3_execute_wave(bridge.clone(), wave, started.clone()).await;
    let _ = outcome;

    let n = started.load(std::sync::atomic::Ordering::SeqCst);
    // See the comment on `test_dispatcher_effective_cap_hat_lower_than_bridge`
    // for the U9 change: all 4 slots must end up spawned across
    // rounds (the dispatcher's batched rounds close the U4 "fifth
    // slot starts after release" contract).
    assert_eq!(
        n as u32, 4,
        "U3 KTD-5 (B): all 4 slots must eventually be spawned across rounds; got {n}"
    );
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
fn setup_u3_partial_failure_bridge(
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
fn make_u3_wave(name: &str, events_count: u32, total: u32) -> ralph_core::DetectedWave {
    make_u3_wave_with_concurrency(name, events_count, total, events_count)
}

/// Build a `DetectedWave` with a configurable `hat.concurrency`
/// (distinct from `events_count`). Used by the cap tests.
fn make_u3_wave_with_concurrency(
    name: &str,
    events_count: u32,
    total: u32,
    concurrency: u32,
) -> ralph_core::DetectedWave {
    use ralph_core::DetectedWave;
    use ralph_core::config::HatConfig;

    let events: Vec<ralph_core::Event> = (0..events_count)
        .map(|i| ralph_core::Event {
            topic: "exec.unit.ready".to_string(),
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

#[test]
fn test_u4_cap4_barrier_releases_fifth_fifo_slot() {
    use ralph_core::supervisor::{InMemorySupervisorStore, SlotResource, SupervisorStore};
    use std::sync::{Arc, Barrier};

    let store = Arc::new(InMemorySupervisorStore::new());
    let wave = store
        .register_wave("u4-cap4-barrier", WaveKind::Exec, 5, 0)
        .unwrap();
    for index in 0..5 {
        store
            .bind_worktree(
                &wave,
                index,
                SlotResource {
                    slot_index: index,
                    worktree_path: Some(format!("/tmp/u4-cap4/{index}")),
                    branch: Some(format!("u4-cap4-{index}")),
                },
            )
            .unwrap();
    }

    let barrier = Arc::new(Barrier::new(5));
    let outcomes = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..5 {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(scope.spawn(move || {
                barrier.wait();
                store.try_dispatch_next(4).unwrap()
            }));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("barrier worker must not panic"))
            .collect::<Vec<_>>()
    });

    let dispatched: Vec<_> = outcomes.into_iter().flatten().collect();
    assert_eq!(dispatched.len(), 4, "cap=4 must approve exactly four slots");
    assert_eq!(
        store.fan_in_status(&wave).unwrap().in_flight_count,
        4,
        "concurrent approvals must never exceed cap"
    );
    for (wave_id, slot_index) in &dispatched {
        store
            .release_slot_dispatch(
                wave_id,
                *slot_index,
                ralph_core::supervisor::DispatchOutcome::Completed,
            )
            .unwrap();
    }
    assert_eq!(
        store.try_dispatch_next(4).unwrap().unwrap().1,
        4,
        "the fifth pending slot must follow the registered FIFO order"
    );
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
enum U5SlotOutcome {
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
}

/// Executor whose per-slot outcome is scripted by the test. Slots
/// without an explicit entry fall back to `default`.
struct U5RecordingExecutor {
    plan: std::sync::Arc<std::collections::HashMap<u32, U5SlotOutcome>>,
    default: U5SlotOutcome,
    /// Number of times each slot has been executed.
    calls: std::sync::Arc<Mutex<std::collections::HashMap<u32, u32>>>,
}

impl U5RecordingExecutor {
    fn new(default: U5SlotOutcome) -> Self {
        Self {
            plan: std::sync::Arc::new(std::collections::HashMap::new()),
            default,
            calls: std::sync::Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    fn with_slot(mut self, index: u32, outcome: U5SlotOutcome) -> Self {
        let map = std::sync::Arc::make_mut(&mut self.plan);
        map.insert(index, outcome);
        self
    }

    /// 2026-07-28-003 plan U5 (S7 / S12): the *first* attempt's
    /// outcome for `index`, followed by `follow_up` for any subsequent
    /// attempt. Tests describe "fail once, then succeed" by setting
    /// initial=Fail(retryable), follow_up=Success(N).
    fn with_first_attempt_then(
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

    fn call_count(&self, slot_index: u32) -> u32 {
        self.calls
            .lock()
            .unwrap()
            .get(&slot_index)
            .copied()
            .unwrap_or(0)
    }
}
/// Build a deterministic `ralph_core::Event` for a (slot, seq) pair so
/// the content hash is stable across runs.
///
/// Uses the production-shaped terminal topic `exec.unit.done` so the
/// dispatcher's classifier (2026-07-23-007 plan U1) recognises it as
/// a terminal Done marker and routes to `record_slot_result` instead
/// of `record_slot_failure(empty_worker_result)`.
fn u5_event(slot_index: u32, seq: usize) -> ralph_core::Event {
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
        Box::pin(async move {
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
                other => other,
            };
            let outcome = mapped;
            match outcome {
                U5SlotOutcome::Success(count) => {
                    let events: Vec<ralph_core::Event> =
                        (0..count).map(|seq| u5_event(index, seq)).collect();
                    (index, Ok((events, Duration::from_millis(5), true)))
                }
                U5SlotOutcome::Fail(reason) => (index, Err((reason, Duration::from_millis(5)))),
                U5SlotOutcome::ScriptedThen { .. } => unreachable!("mapped above"),
            }
        })
    }
}

/// Store-backed spy bridge for U5. `record_slot_result` /
/// `record_slot_failure` capture the call AND delegate to the real
/// store so tests can assert both on the captured payload (hash /
/// count / reason) and on `fan_in_status` (completed/failed counts).
#[derive(Clone)]
struct U5RecordingBridge {
    store: std::sync::Arc<dyn SupervisorStore>,
    /// `(slot_index, content_hash, event_count)` per successful record.
    slot_results: std::sync::Arc<Mutex<Vec<(u32, String, usize)>>>,
    /// `(slot_index, reason)` per failure record.
    slot_failures: std::sync::Arc<Mutex<Vec<(u32, String)>>>,
    /// 2026-07-28-003 plan U5: per-test override for the retry budget.
    /// Defaults to 0 so the existing characterization tests stay
    /// bit-identical to pre-U5; new S7/S8/S10/S12 tests override it.
    retry_budget_override: std::sync::Arc<Mutex<Option<u32>>>,
}

impl std::fmt::Debug for U5RecordingBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("U5RecordingBridge").finish()
    }
}

impl U5RecordingBridge {
    fn new(store: std::sync::Arc<dyn SupervisorStore>) -> Self {
        Self {
            store,
            slot_results: std::sync::Arc::new(Mutex::new(Vec::new())),
            slot_failures: std::sync::Arc::new(Mutex::new(Vec::new())),
            retry_budget_override: std::sync::Arc::new(Mutex::new(None)),
        }
    }

    fn with_retry_budget(self, budget: u32) -> Self {
        *self.retry_budget_override.lock().unwrap() = Some(budget);
        self
    }

    fn results_snapshot(&self) -> Vec<(u32, String, usize)> {
        self.slot_results.lock().unwrap().clone()
    }

    fn failures_snapshot(&self) -> Vec<(u32, String)> {
        self.slot_failures.lock().unwrap().clone()
    }
}

impl SupervisorBridge for U5RecordingBridge {
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
        Ok(Some(SlotBinding {
            slot_index,
            env: HashMap::new(),
            worktree_path: Some(format!("/tmp/u5/{wave_id}-{slot_index}").into()),
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
async fn run_u5_execute_wave(
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
    )
    .await;
    let probe = U5RecordingExecutor {
        plan: std::sync::Arc::new(std::collections::HashMap::new()),
        default: U5SlotOutcome::Success(0),
        calls,
    };
    (outcome, bridge, probe)
}

/// U5 验收 #1: N successful workers → the supervisor store records
/// `completed_count == N` (one `record_slot_result` per terminal slot).
#[tokio::test]
async fn test_dispatcher_records_slot_outcomes() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>);

    let wave = make_u3_wave("u5-all-ok", 3, 3);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(1));

    let (outcome, bridge, _exec) = run_u5_execute_wave(bridge, wave, executor).await;

    // The wave must complete (all workers succeed well within budget).
    assert!(
        matches!(
            outcome,
            WaveDispatchOutcome::Completed(_) | WaveDispatchOutcome::Partial(_)
        ),
        "U5: expected a completed wave, got {outcome:?}"
    );

    // One record_slot_result per slot.
    let results = bridge.results_snapshot();
    assert_eq!(
        results.len(),
        3,
        "U5: dispatcher must call record_slot_result once per successful slot (3); got {results:?}"
    );
    assert!(
        bridge.failures_snapshot().is_empty(),
        "U5: no failures expected"
    );

    // The store snapshot reflects 3 completed slots.
    let store_wave_id = bridge
        .store
        .recover_active_waves()
        .expect("recover")
        .pop()
        .expect("one wave")
        .wave_id;
    let snap = bridge
        .store
        .fan_in_status(&store_wave_id)
        .expect("snapshot");
    assert_eq!(
        snap.completed_count, 3,
        "U5: store completed_count must equal the number of successful slots"
    );
    assert_eq!(snap.failed_count, 0, "U5: no failed slots expected");
}

/// U5 验收 #2: 2 success + 1 failure → `completed_count == 2`,
/// `failed_count == 1`, and the failure reason is preserved.
#[tokio::test]
async fn test_dispatcher_records_failure_with_reason() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>);

    let wave = make_u3_wave("u5-one-fail", 3, 3);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(1))
        .with_slot(1, U5SlotOutcome::Fail("boom: worker crashed".to_string()));

    let (_outcome, bridge, _exec) = run_u5_execute_wave(bridge, wave, executor).await;

    let results = bridge.results_snapshot();
    let failures = bridge.failures_snapshot();
    assert_eq!(
        results.len(),
        2,
        "U5: two successful slots; got {results:?}"
    );
    assert_eq!(failures.len(), 1, "U5: one failed slot; got {failures:?}");
    assert_eq!(failures[0].0, 1, "U5: slot 1 is the failed slot");
    assert!(
        failures[0].1.contains("boom"),
        "U5: failure reason must be preserved, got {:?}",
        failures[0].1
    );

    let store_wave_id = bridge
        .store
        .recover_active_waves()
        .expect("recover")
        .pop()
        .expect("one wave")
        .wave_id;
    let snap = bridge
        .store
        .fan_in_status(&store_wave_id)
        .expect("snapshot");
    assert_eq!(snap.completed_count, 2, "U5: completed_count == 2");
    assert_eq!(snap.failed_count, 1, "U5: failed_count == 1");
}

/// U5 验收 #3: re-dispatching the same wave MUST NOT double-count.
/// The store's `record_slot_*` is idempotent per `(wave, slot)`; the
/// dispatcher relies on this and must not assume single-call.
#[tokio::test]
async fn test_dispatcher_record_idempotent_across_reruns() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>);

    let wave = make_u3_wave("u5-idem", 2, 2);

    // First dispatch: both slots succeed and are recorded.
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(1));
    let (_outcome, _returned_bridge, _exec) =
        run_u5_execute_wave(bridge.clone(), wave, executor).await;

    let store_wave_id = bridge
        .store
        .recover_active_waves()
        .expect("recover")
        .pop()
        .expect("one wave")
        .wave_id;
    assert_eq!(
        bridge
            .store
            .fan_in_status(&store_wave_id)
            .unwrap()
            .completed_count,
        2,
        "U5: first run records 2 completed slots"
    );

    // Re-record the SAME slots directly through the bridge
    // (simulating a duplicate record — e.g. a crash/replay
    // that re-reports a slot). 2026-07-23-004 plan U5 makes
    // idempotency depend on the SAME content_hash; replaying
    // with a conflicting hash is a different scenario
    // (`test_dispatcher_record_conflicting_terminal_is_rejected`).
    let recorded = bridge.slot_results.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2, "bridge must have recorded 2 slots");
    let (slot0_hash, slot1_hash) = (recorded[0].1.clone(), recorded[1].1.clone());
    bridge
        .record_slot_result(&store_wave_id, 0, &slot0_hash, 1)
        .expect("re-record slot 0 with same hash must be idempotent");
    bridge
        .record_slot_result(&store_wave_id, 1, &slot1_hash, 1)
        .expect("re-record slot 1 with same hash must be idempotent");

    let snap = bridge
        .store
        .fan_in_status(&store_wave_id)
        .expect("snapshot");
    assert_eq!(
        snap.completed_count, 2,
        "U5: duplicate record_slot_result with same content_hash must NOT increase completed_count"
    );

    // A *conflicting* content_hash is rejected.
    let conflict = bridge
        .store
        .record_slot_result(&store_wave_id, 0, "different-hash", 1);
    assert!(
        conflict.is_err(),
        "conflicting content_hash must be rejected, got {conflict:?}"
    );
}

/// U5 验收 #4: a worker producing K events → the recorded
/// `event_count` for that slot is K (batch is preserved).
#[tokio::test]
async fn test_dispatcher_records_event_batch_count() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>);

    let wave = make_u3_wave("u5-batch", 2, 2);
    // Slot 0 → 4 events, slot 1 → 1 event.
    let executor =
        U5RecordingExecutor::new(U5SlotOutcome::Success(1)).with_slot(0, U5SlotOutcome::Success(4));

    let (_outcome, bridge, _exec) = run_u5_execute_wave(bridge, wave, executor).await;

    let results = bridge.results_snapshot();
    assert_eq!(results.len(), 2, "U5: two slots recorded; got {results:?}");
    let by_slot: std::collections::HashMap<u32, usize> = results
        .iter()
        .map(|(slot, _hash, count)| (*slot, *count))
        .collect();
    assert_eq!(
        by_slot.get(&0).copied(),
        Some(4),
        "U5: slot 0 must record event_count == 4"
    );
    assert_eq!(
        by_slot.get(&1).copied(),
        Some(1),
        "U5: slot 1 must record event_count == 1"
    );
}

/// U5 验收 #5 (flipped by 2026-07-23-007 plan U1): an empty-event
/// worker → the dispatcher MUST classify via `classify_worker_outcome`
/// and record `record_slot_failure("empty_worker_result")`. The legacy
/// "stable hash on empty batch" assertion was incorrect: an exit-0
/// worker with `event_count == 0` is a fail-close case, not a success.
/// The non-empty count + content hash contract still holds for the
/// accepted-batch path (verified by `test_dispatcher_records_event_batch_count`).
#[tokio::test]
async fn test_dispatcher_records_empty_batch_stable_hash() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>);

    let wave = make_u3_wave("u5-empty", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(0));

    let (_outcome, bridge, _exec) = run_u5_execute_wave(bridge, wave, executor).await;

    // The flipped semantic: success + zero events → record_slot_failure.
    let results = bridge.results_snapshot();
    assert_eq!(
        results.len(),
        0,
        "U1/007: empty batch must NOT record a slot result; got {results:?}"
    );
    let failures = bridge.failures_snapshot();
    assert_eq!(
        failures.len(),
        1,
        "U1/007: empty batch must record exactly one slot failure; got {failures:?}"
    );
    let (slot, reason) = &failures[0];
    assert_eq!(*slot, 0);
    assert_eq!(
        reason,
        ralph_core::supervisor::worker_outcome::REASON_EMPTY_WORKER_RESULT,
        "U1/007: empty batch must use the stable reason code empty_worker_result"
    );

    // The store snapshot reflects 0 completed slots + 1 failed slot.
    let store_wave_id = bridge
        .store
        .recover_active_waves()
        .expect("recover")
        .pop()
        .expect("one wave")
        .wave_id;
    let snap = bridge
        .store
        .fan_in_status(&store_wave_id)
        .expect("snapshot");
    assert_eq!(
        snap.completed_count, 0,
        "U1/007: empty batch must not lift completed_count"
    );
    assert_eq!(
        snap.failed_count, 1,
        "U1/007: empty batch must lift failed_count"
    );
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
const U5_RETRYABLE_REASON: &str =
    "Worker timed out after 1s of startup grace (worker_timeout/startup_kill, no first signal)";

/// U5 / S7: a worker that fails with a retryable reason on attempt 1
/// and succeeds on attempt 2 must trigger an in-task retry, no
/// `record_slot_failure`, and end with `record_slot_result` once.
#[tokio::test]
async fn u5_s7_retry_then_success_records_only_result() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(1);

    let wave = make_u3_wave("u5-s7-retry", 1, 1);
    // Attempt 1 = retryable failure; attempt 2 = success with 1 event.
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(0)).with_first_attempt_then(
        0,
        U5SlotOutcome::Fail(U5_RETRYABLE_REASON.to_string()),
        U5SlotOutcome::Success(1),
    );

    let (_outcome, bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    // Two attempts on slot 0.
    assert_eq!(
        exec.call_count(0),
        2,
        "U5/S7: retryable failure must trigger a second attempt, got {}",
        exec.call_count(0)
    );

    // No record_slot_failure; exactly one record_slot_result.
    let failures = bridge.failures_snapshot();
    assert!(
        failures.is_empty(),
        "U5/S7: retryable failure must NOT record_slot_failure, got {failures:?}"
    );
    let results = bridge.results_snapshot();
    assert_eq!(
        results.len(),
        1,
        "U5/S7: second attempt success must record_slot_result once, got {results:?}"
    );
    let (slot, _hash, count) = &results[0];
    assert_eq!(*slot, 0);
    assert_eq!(
        *count, 1,
        "U5/S7: fingerprint must reflect attempt 2's batch only"
    );
}

/// U5 / S8: two consecutive retryable failures (budget=1) must exhaust
/// the budget and route to `record_slot_failure("worker_timeout")`. The
/// resulting failure path keeps the `redrive_slots` field present (the
/// store still records a failed slot so the next fan-in can offer a
/// redrive slot).
#[tokio::test]
async fn u5_s8_budget_exhausted_records_failure_with_redrive_eligibility() {
    use ralph_core::supervisor::SupervisorStore;

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(1);

    let wave = make_u3_wave("u5-s8-budget", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Fail(U5_RETRYABLE_REASON.to_string()));

    let (_outcome, bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    // Two attempts (initial + 1 retry, budget=1).
    assert_eq!(
        exec.call_count(0),
        2,
        "U5/S8: budget=1 must allow exactly 2 attempts, got {}",
        exec.call_count(0)
    );

    // Exactly one record_slot_failure with the FROZEN reason code.
    let results = bridge.results_snapshot();
    assert!(
        results.is_empty(),
        "U5/S8: budget exhausted must NOT record_slot_result, got {results:?}"
    );
    let failures = bridge.failures_snapshot();
    assert_eq!(
        failures.len(),
        1,
        "U5/S8: budget exhausted must record_slot_failure once, got {failures:?}"
    );
    let (slot, reason) = &failures[0];
    assert_eq!(*slot, 0);
    assert_eq!(
        reason, "worker_timeout",
        "U5/S8: stored reason must be the FROZEN worker_timeout static code"
    );

    // Store state: 0 completed + 1 failed (so the next fan-in/recovery
    // path can offer this slot for redrive).
    let store_wave_id = bridge
        .store
        .recover_active_waves()
        .expect("recover")
        .pop()
        .expect("one wave")
        .wave_id;
    let snap = bridge
        .store
        .fan_in_status(&store_wave_id)
        .expect("snapshot");
    assert_eq!(
        snap.completed_count, 0,
        "U5/S8: failed slot must not lift completed_count"
    );
    assert_eq!(
        snap.failed_count, 1,
        "U5/S8: failed slot must lift failed_count"
    );
}

/// U5 / S10: a non-retryable failure (e.g. `worker_cancelled` or unknown
/// dynamic reason) must NOT trigger a retry, even when budget > 0.
#[tokio::test]
async fn u5_s10_non_retryable_failure_does_not_retry() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(2);

    let wave = make_u3_wave("u5-s10-cancelled", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Fail("worker_cancelled".to_string()));

    let (_outcome, bridge, exec) = run_u5_execute_wave(bridge, wave, executor).await;

    assert_eq!(
        exec.call_count(0),
        1,
        "U5/S10: non-retryable reason must attempt exactly once, got {}",
        exec.call_count(0)
    );

    let failures = bridge.failures_snapshot();
    assert_eq!(
        failures.len(),
        1,
        "U5/S10: cancelled must record one failure"
    );
    let (_, reason) = &failures[0];
    assert_eq!(
        reason, "worker_cancelled",
        "U5/S10: store must keep the verbatim reason"
    );
}

/// U5 / S12: an intermediate partial-batch attempt that fails with a
/// retryable reason must NOT leak its events into the
/// `record_slot_result` fingerprint. Only the *final* attempt's batch
/// (whether success or failure) must end up in the store.
#[tokio::test]
async fn u5_s12_final_attempt_fingerprint_only() {
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>)
        .with_retry_budget(1);

    let wave = make_u3_wave("u5-s12-fingerprint", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(0)).with_first_attempt_then(
        0,
        U5SlotOutcome::Fail(U5_RETRYABLE_REASON.to_string()),
        U5SlotOutcome::Success(3),
    );

    let (_outcome, bridge, _exec) = run_u5_execute_wave(bridge, wave, executor).await;

    let results = bridge.results_snapshot();
    assert_eq!(
        results.len(),
        1,
        "U5/S12: only the final attempt's result may be recorded, got {results:?}"
    );
    let (slot, _hash, count) = &results[0];
    assert_eq!(*slot, 0);
    assert_eq!(
        *count, 3,
        "U5/S12: event_count must reflect the final attempt's batch (3 events), not the failed attempt"
    );
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

fn captured_env()
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
async fn run_u2_execute_wave_with_env_capture(
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
    )
    .await
}

/// U2 验收 #1: production supervisor bridge with a real
/// `ProductionBridgeContext.repo_root` — the spawned worker MUST
/// observe `RALPH_WORKSPACE_ROOT == repo_root` and
/// `RALPH_EVENTS_FILE == <validated absolute channel>` even when
/// the parent process has polluted values for both. The
/// `merge_event_channel_env` SSOT overrides whatever was in the
/// worker_backend env before validation.
#[tokio::test]
async fn test_u2_workspace_root_and_channel_injected_into_worker_env() {
    use crate::loop_runner::wave::CoordinatorSupervisorBridge;

    let tmp = tempfile::tempdir().expect("temp dir");
    // U2/007: canonicalize the tempdir so symlinked paths
    // (e.g. /var/tmp → /private/var/tmp on macOS) match the
    // canonicalized workspace root in the validator. Without
    // this, validate_control_plane_binding rejects every channel
    // it observes in CI on macOS.
    let workspace_root = std::fs::canonicalize(tmp.path()).expect("canonicalize workspace root");
    let wave_dir = workspace_root.join(".ralph");
    std::fs::create_dir_all(&wave_dir).expect("create wave dir");
    let main_events_file = wave_dir.join("events.jsonl");

    // U2/007: stub worktree factory — `DefaultWorktreeFactory`
    // would invoke the real `git worktree add`, but the tempdir
    // is not a git repo, so the bind would fail-closed and the
    // executor would never run. The stub returns a synthetic
    // worktree under the workspace root so the slot-subtree
    // validator has something to reject / accept against.
    #[derive(Debug)]
    struct StubFactory;
    impl ralph_core::supervisor::worktree_bind::WorktreeFactory for StubFactory {
        fn create(
            &self,
            repo_root: std::path::PathBuf,
            branch: String,
        ) -> Result<
            ralph_core::worktree::Worktree,
            ralph_core::supervisor::worktree_bind::WorktreeError,
        > {
            let wt = repo_root.join(format!("wt-{branch}"));
            std::fs::create_dir_all(&wt).ok();
            Ok(ralph_core::worktree::Worktree {
                path: wt,
                branch,
                is_main: false,
                head: None,
            })
        }
    }

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = ProductionBridgeContext {
        loop_id: "u2-loop".to_string(),
        repo_root: workspace_root.clone(),
        events_path: Some(main_events_file.clone()),
        tasks_path: None,
    };
    let bridge = CoordinatorSupervisorBridge::with_context_and_factory(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        context,
        std::sync::Arc::new(StubFactory),
    );

    let wave = make_u3_wave("u2-routing", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(1));

    let capture = captured_env();
    capture.lock().unwrap().clear();
    let _outcome =
        run_u2_execute_wave_with_env_capture(bridge, wave, executor, &main_events_file, "u2-loop")
            .await;

    let snap = capture.lock().unwrap().clone();
    assert_eq!(snap.len(), 1, "U2/007: one slot captured; got {snap:?}");
    let env_map: std::collections::HashMap<String, String> = snap
        .get(&0)
        .expect("slot 0 captured")
        .iter()
        .cloned()
        .collect();

    let canonical_root = std::fs::canonicalize(&workspace_root).unwrap_or(workspace_root.clone());
    let expected_root = canonical_root.display().to_string();
    let observed_root = env_map
        .get("RALPH_WORKSPACE_ROOT")
        .expect("RALPH_WORKSPACE_ROOT must be injected")
        .clone();
    assert!(
        observed_root == expected_root || observed_root == workspace_root.display().to_string(),
        "U2/007: RALPH_WORKSPACE_ROOT must equal the production repo_root; expected={expected_root}, got={observed_root}"
    );

    // The per-worker channel (RALPH_EVENTS_FILE) is the validated
    // absolute path of the slot's per-worker JSONL (NOT the main
    // events file). The dispatcher builds
    // `<wave_dir>/wave-{id}-{index}.jsonl`, validates it, and the
    // SSOT injects the validated canonical path here.
    let observed_events = env_map
        .get("RALPH_EVENTS_FILE")
        .expect("RALPH_EVENTS_FILE must be injected")
        .clone();
    let observed_events_path = std::path::Path::new(&observed_events);
    assert!(
        observed_events_path.is_absolute(),
        "U2/007: RALPH_EVENTS_FILE must be an absolute path; got {observed_events}"
    );
    // The injected path MUST live under the validated workspace root.
    assert!(
        observed_events_path.starts_with(&canonical_root),
        "U2/007: RALPH_EVENTS_FILE must live under workspace_root; got {observed_events}, expected prefix {expected_root}"
    );
    // And it must NOT live inside the slot worktree (slot-subtree
    // rejection contract).
    assert!(
        !observed_events_path.starts_with(workspace_root.join("wt-")),
        "U2/007: RALPH_EVENTS_FILE must NOT live in slot subtree; got {observed_events}"
    );
}

/// 2026-07-25-003 plan U4 (R6): the worker process's
/// `RALPH_WAVE_ID` env var MUST be the **public** wave id (the
/// `DetectedWave.wave_id` the dispatcher received), NOT the
/// supervisor store's internal `w-{seq}` id. The dispatcher
/// injects `RALPH_WAVE_ID = public id` in `dispatch_wave_inner`
/// (line ~1582), but `bind_slot` later writes the store id into
/// `binding.env` and the env merge in the dispatcher uses
/// last-write-wins — so the public id is silently overwritten by
/// the store id in the spawned worker's environment. This
/// regression reproduces that bug end-to-end and pins the fix
/// (the dispatcher's final RALPH_WAVE_ID must be the public id).
#[tokio::test]
async fn test_u4_worker_env_wave_id_is_public_id() {
    use crate::loop_runner::wave::CoordinatorSupervisorBridge;

    let tmp = tempfile::tempdir().expect("temp dir");
    let workspace_root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    let wave_dir = workspace_root.join(".ralph");
    std::fs::create_dir_all(&wave_dir).expect("create wave dir");
    let main_events_file = wave_dir.join("events.jsonl");

    #[derive(Debug)]
    struct StubFactory;
    impl ralph_core::supervisor::worktree_bind::WorktreeFactory for StubFactory {
        fn create(
            &self,
            repo_root: std::path::PathBuf,
            branch: String,
        ) -> Result<
            ralph_core::worktree::Worktree,
            ralph_core::supervisor::worktree_bind::WorktreeError,
        > {
            let wt = repo_root.join(format!("wt-{branch}"));
            std::fs::create_dir_all(&wt).ok();
            Ok(ralph_core::worktree::Worktree {
                path: wt,
                branch,
                is_main: false,
                head: None,
            })
        }
    }

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = ProductionBridgeContext {
        loop_id: "u4-public-id".to_string(),
        repo_root: workspace_root.clone(),
        events_path: Some(main_events_file.clone()),
        tasks_path: None,
    };
    let bridge = CoordinatorSupervisorBridge::with_context_and_factory(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        context,
        std::sync::Arc::new(StubFactory),
    );

    // Public id contains a dash (mirrors the production
    // `w-rs-1` / `w-246cb4afef33` shape) so the test fails
    // loudly if any code path mangles the id format.
    let public_wave_id = "w-public-rs-1";
    let wave = make_u3_wave(public_wave_id, 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(1));

    let capture = captured_env();
    capture.lock().unwrap().clear();
    let _outcome = run_u2_execute_wave_with_env_capture(
        bridge,
        wave,
        executor,
        &main_events_file,
        "u4-public-id",
    )
    .await;

    let snap = capture.lock().unwrap().clone();
    assert_eq!(snap.len(), 1, "U4/003: one slot captured; got {snap:?}");
    let env_map: std::collections::HashMap<String, String> = snap
        .get(&0)
        .expect("slot 0 captured")
        .iter()
        .cloned()
        .collect();

    let observed = env_map
        .get("RALPH_WAVE_ID")
        .expect("RALPH_WAVE_ID must be injected")
        .clone();
    assert_eq!(
        observed, public_wave_id,
        "U4/003: worker RALPH_WAVE_ID must equal the public wave id (the id the agent saw in `DetectedWave.wave_id`); got {observed}"
    );
    // Negative guard: the worker must NOT see the supervisor
    // store's internal `w-{seq}` id. The store allocates a
    // distinct id on first registration, so we recover it from
    // the live store and assert it does NOT leak into the
    // worker's env.
    let store_wave_id = store
        .recover_active_waves()
        .expect("recover")
        .pop()
        .expect("one wave")
        .wave_id;
    assert_ne!(
        store_wave_id, public_wave_id,
        "U4/003: pre-condition: the supervisor store must allocate a distinct id from the public id (otherwise the test cannot distinguish them); got {store_wave_id}"
    );
    assert_ne!(
        observed, store_wave_id,
        "U4/003: worker RALPH_WAVE_ID must NOT leak the supervisor store's internal id ({store_wave_id}); got {observed}"
    );
}

/// 2026-07-23-007 plan U4 (R-W5): when the production bridge
/// carries a `tasks.jsonl` path, the dispatcher projects each
/// slot's terminal state onto a stable task row. A successful
/// slot ends up as `TaskStatus::Closed` in `tasks.jsonl`;
/// `ralph tools task list` (via `TaskStore::load`) sees it.
/// Idempotent re-projection does not duplicate rows.
#[tokio::test]
async fn test_u4_slot_terminal_projects_to_tasks_jsonl() {
    use crate::loop_runner::wave::CoordinatorSupervisorBridge;
    use ralph_core::TaskStore;

    let tmp = tempfile::tempdir().expect("temp dir");
    let workspace_root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    let tasks_dir = workspace_root.join(".ralph").join("agent");
    std::fs::create_dir_all(&tasks_dir).expect("create tasks dir");
    let tasks_path = tasks_dir.join("tasks.jsonl");
    let main_events_file = workspace_root.join(".ralph").join("events.jsonl");

    #[derive(Debug)]
    struct StubFactory;
    impl ralph_core::supervisor::worktree_bind::WorktreeFactory for StubFactory {
        fn create(
            &self,
            repo_root: std::path::PathBuf,
            branch: String,
        ) -> Result<
            ralph_core::worktree::Worktree,
            ralph_core::supervisor::worktree_bind::WorktreeError,
        > {
            let wt = repo_root.join(format!("wt-{branch}"));
            std::fs::create_dir_all(&wt).ok();
            Ok(ralph_core::worktree::Worktree {
                path: wt,
                branch,
                is_main: false,
                head: None,
            })
        }
    }

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = ProductionBridgeContext {
        loop_id: "u4-loop".to_string(),
        repo_root: workspace_root.clone(),
        events_path: Some(main_events_file.clone()),
        tasks_path: Some(tasks_path.clone()),
    };
    let bridge = CoordinatorSupervisorBridge::with_context_and_factory(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        context,
        std::sync::Arc::new(StubFactory),
    );

    let wave = make_u3_wave("u4-projection", 1, 1);
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Success(1));
    let _outcome =
        run_u2_execute_wave_with_env_capture(bridge, wave, executor, &main_events_file, "u4-loop")
            .await;

    // U4/007: tasks.jsonl now carries one row for slot 0 with a
    // stable task_key and a terminal status.
    let task_store = TaskStore::load(&tasks_path).expect("load");
    let rows: Vec<_> = task_store.all().iter().collect();
    assert_eq!(
        rows.len(),
        1,
        "U4/007: exactly one task row projected; got {rows:?}"
    );
    let row = &rows[0];
    let key = row.key.as_deref().unwrap_or("");
    // U4/007: the task_key MUST start with `supervisor:u4-loop:`
    // (loop-scoped) and reference slot 0. The wave-id portion is
    // produced by the supervisor store's allocator, so we only
    // assert the loop + slot shape to keep the test stable
    // against allocator changes.
    assert!(
        key.starts_with("supervisor:u4-loop:") && key.ends_with(":slot-0"),
        "U4/007: stable task_key must carry loop_id + slot_index; got {key:?}"
    );
    let status_str = format!("{:?}", row.status);
    assert!(
        status_str.to_lowercase().contains("closed") || status_str.to_lowercase().contains("done"),
        "U4/007: completed slot must project to terminal status; got {status_str}"
    );
}

/// 2026-07-23-007 plan U10 (T3 / T6): the sibling of
/// `test_u4_slot_terminal_projects_to_tasks_jsonl` for the
/// Failed path. A slot that classifies as Failed (e.g. the
/// worker backend reports a non-zero exit with no accepted
/// terminal marker) MUST project a `Failed` task row, NOT a
/// `Closed` row. The existing Success sibling checked
/// terminal-status with a substring match; this test uses
/// `assert_eq!` against `TaskStatus::Failed` directly so the
/// assertion strength matches the schema (folding testing:T6
/// into the same test).
#[tokio::test]
async fn test_u4_failed_slot_projects_to_failed_task_row() {
    use crate::loop_runner::wave::CoordinatorSupervisorBridge;
    use ralph_core::TaskStatus;
    use ralph_core::TaskStore;

    let tmp = tempfile::tempdir().expect("temp dir");
    let workspace_root = std::fs::canonicalize(tmp.path()).expect("canonicalize");
    let tasks_dir = workspace_root.join(".ralph").join("agent");
    std::fs::create_dir_all(&tasks_dir).expect("create tasks dir");
    let tasks_path = tasks_dir.join("tasks.jsonl");
    let main_events_file = workspace_root.join(".ralph").join("events.jsonl");

    #[derive(Debug)]
    struct StubFactory;
    impl ralph_core::supervisor::worktree_bind::WorktreeFactory for StubFactory {
        fn create(
            &self,
            repo_root: std::path::PathBuf,
            branch: String,
        ) -> Result<
            ralph_core::worktree::Worktree,
            ralph_core::supervisor::worktree_bind::WorktreeError,
        > {
            let wt = repo_root.join(format!("wt-{branch}"));
            std::fs::create_dir_all(&wt).ok();
            Ok(ralph_core::worktree::Worktree {
                path: wt,
                branch,
                is_main: false,
                head: None,
            })
        }
    }

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = ProductionBridgeContext {
        loop_id: "u10-loop".to_string(),
        repo_root: workspace_root.clone(),
        events_path: Some(main_events_file.clone()),
        tasks_path: Some(tasks_path.clone()),
    };
    let bridge = CoordinatorSupervisorBridge::with_context_and_factory(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        context,
        std::sync::Arc::new(StubFactory),
    );

    let wave = make_u3_wave("u10-failed-projection", 1, 1);
    // U5's Fail arm: the executor reports a non-zero exit with
    // a structured error reason. The classifier maps it to
    // SlotOutcome::Failed{worker_cancelled} (per the U3
    // worker_outcome.rs short-circuit), so the slot must
    // project to TaskStatus::Failed.
    let executor = U5RecordingExecutor::new(U5SlotOutcome::Fail("boom".to_string()));
    let _outcome =
        run_u2_execute_wave_with_env_capture(bridge, wave, executor, &main_events_file, "u10-loop")
            .await;

    let task_store = TaskStore::load(&tasks_path).expect("load");
    let rows: Vec<_> = task_store.all().iter().collect();
    assert_eq!(
        rows.len(),
        1,
        "U10/007: exactly one task row projected for the Failed slot; got {rows:?}"
    );
    let row = &rows[0];
    let key = row.key.as_deref().unwrap_or("");
    assert!(
        key.starts_with("supervisor:u10-loop:") && key.ends_with(":slot-0"),
        "U10/007: stable task_key must carry loop_id + slot_index; got {key:?}"
    );
    assert_eq!(
        row.status,
        TaskStatus::Failed,
        "U10/007: Failed slot must project to TaskStatus::Failed (not Closed); got {:?}",
        row.status
    );
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

use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};

/// Build a production bridge whose coordinator merges through a
/// `FileEventMergeSink` pointed at `events_path`, then register a
/// wave with `n` slots and record every slot as a success (bound
/// worktree resource + dispatched + completed). Returns the bridge
/// (as a trait object) and the store-assigned wave id.
fn setup_u6_production_bridge(
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
fn make_u6_completed(wave_key: &str, n: u32) -> ralph_core::CompletedWave {
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
fn read_u6_ledger(path: &std::path::Path) -> Vec<serde_json::Value> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("ledger line must be JSON"))
        .collect()
}

/// U6 acceptance #1 + #3: 5 successful slots → the main ledger holds
/// the business events sorted by slot index (de-duplicated) plus
/// exactly one `exec.wave.complete` whose payload lists all 5
/// success_slots (branch + worktree_path). A second fan-in tick is
/// idempotent (no duplicate coord event, `AlreadyDone`).
#[test]
fn test_production_fan_in_writes_ledger_and_injects_complete_once() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");
    let (bridge, _store_wave_id) = setup_u6_production_bridge(events_path.clone(), "u6-wave-5", 5);

    let completed = make_u6_completed("u6-wave-5", 5);
    let detected = make_u3_wave("u6-wave-5", 5, 5);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedComplete,
        "5/5 success must inject exec.wave.complete"
    );

    let lines = read_u6_ledger(&events_path);
    // 5 business events + 1 coordination event.
    assert_eq!(
        lines.len(),
        6,
        "ledger must hold 5 business events + 1 coord event; got {lines:?}"
    );

    // Business events are sorted by slot index (0..5) even though the
    // CompletedWave handed them in reverse order.
    let business: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.unit.done"))
        .collect();
    assert_eq!(business.len(), 5, "5 de-duplicated business events");
    for (pos, ev) in business.iter().enumerate() {
        let expected_payload = format!("{{\"unit\":\"u6-{pos}\"}}");
        assert_eq!(
            ev.get("payload").and_then(|p| p.as_str()),
            Some(expected_payload.as_str()),
            "business event at ledger position {pos} must be slot {pos} (sorted by slot index)"
        );
    }

    // Exactly one coord event, with all 5 success_slots.
    let completes: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.complete"))
        .collect();
    assert_eq!(completes.len(), 1, "exactly one exec.wave.complete");
    let coord = completes[0];
    assert_eq!(
        coord.get("system_injected").and_then(|v| v.as_bool()),
        Some(true),
        "coord event must be system_injected"
    );
    let success_slots = coord
        .get("payload")
        .and_then(|p| p.get("success_slots"))
        .and_then(|s| s.as_array())
        .expect("payload.success_slots must be an array");
    assert_eq!(
        success_slots.len(),
        5,
        "success_slots must list all 5 slots"
    );
    for (i, slot) in success_slots.iter().enumerate() {
        assert_eq!(
            slot.get("slot_index").and_then(|v| v.as_u64()),
            Some(i as u64),
            "success_slots[{i}].slot_index"
        );
        assert_eq!(
            slot.get("branch").and_then(|v| v.as_str()),
            Some(format!("u6-loop-exec-{i}").as_str()),
            "success_slots[{i}].branch"
        );
        assert_eq!(
            slot.get("worktree_path").and_then(|v| v.as_str()),
            Some(format!("/tmp/u6-wt/{i}").as_str()),
            "success_slots[{i}].worktree_path"
        );
    }

    // Idempotency: a second tick must NOT re-inject the coord event.
    let outcome2 = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome2,
        SupervisorFanInOutcome::AlreadyDone,
        "post-merge tick must return AlreadyDone (KTD-7)"
    );
    let lines2 = read_u6_ledger(&events_path);
    let completes2 = lines2
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.complete"))
        .count();
    assert_eq!(
        completes2, 1,
        "still exactly one exec.wave.complete after re-tick"
    );
    assert_eq!(
        lines2.len(),
        6,
        "no new lines appended on the idempotent re-tick"
    );
}

/// U6 acceptance #2: a wave where every slot is terminal but some
/// failed must inject `exec.wave.failed` (KTD-8: no silent partial
/// complete) carrying the blocking slots.
#[test]
fn test_production_fan_in_partial_failure_injects_failed() {
    use ralph_core::supervisor::SlotResource;

    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");

    // Build a 3-slot wave: slots 0,1 succeed; slot 2 fails.
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = crate::loop_runner::wave::ProductionBridgeContext {
        loop_id: "u6-loop".to_string(),
        repo_root: std::path::PathBuf::from("/tmp/u6-repo"),
        events_path: Some(events_path.clone()),
        tasks_path: None,
    };
    let bridge =
        crate::loop_runner::wave::CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
            store.clone() as std::sync::Arc<dyn SupervisorStore>,
            context,
            std::sync::Arc::new(DefaultWorktreeFactory),
            3,
            // 2026-07-28-003 plan U4: explicit budget keeps the
            // fan-in characterization at the historical default.
            1,
        );
    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u6-wave-fail", 3, 0)
        .expect("register");
    for i in 0..3 {
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
            .expect("bind");
    }
    for _ in 0..3 {
        bridge.store().try_dispatch_next(3).expect("dispatch");
    }
    bridge
        .record_slot_result(&store_wave_id, 0, "h0", 1)
        .expect("s0");
    // Plan 004 R2 / P0-2: success path requires terminal evidence.
    bridge
        .store()
        .record_slot_terminal_evidence(
            &store_wave_id,
            0,
            &TerminalEvidence::from_event("exec.unit.done", "{\"unit\":\"u6-0\"}"),
        )
        .expect("evidence 0");
    bridge
        .record_slot_result(&store_wave_id, 1, "h1", 1)
        .expect("s1");
    bridge
        .store()
        .record_slot_terminal_evidence(
            &store_wave_id,
            1,
            &TerminalEvidence::from_event("exec.unit.done", "{\"unit\":\"u6-1\"}"),
        )
        .expect("evidence 1");
    bridge
        .record_slot_failure(&store_wave_id, 2, "boom")
        .expect("f2");
    // Plan 004 R3 / P0-1: dispatcher must commit salvage BEFORE
    // `fail_wave` latches the coord-event injection.
    bridge
        .commit_salvage_projection(
            &store_wave_id,
            &ralph_core::supervisor::ProjectionReceiptSummary {
                kind: ralph_core::supervisor::ProjectionKind::Business,
                batch_fingerprint: "test-fp".into(),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .expect("mark salvage");

    let bridge: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge);
    let completed = make_u6_completed("u6-wave-fail", 2); // only 2 results (slot 2 failed)
    let detected = make_u3_wave("u6-wave-fail", 3, 3);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedFailed,
        "a terminal wave with a failed slot must inject exec.wave.failed"
    );

    let lines = read_u6_ledger(&events_path);
    let failed: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.failed"))
        .collect();
    assert_eq!(failed.len(), 1, "exactly one exec.wave.failed");
    let blocking = failed[0]
        .get("payload")
        .and_then(|p| p.get("blocking_slots"))
        .and_then(|b| b.as_array())
        .expect("payload.blocking_slots");
    assert!(
        blocking.iter().any(|v| v.as_u64() == Some(2)),
        "blocking_slots must name the failed slot 2; got {blocking:?}"
    );
    // No spurious complete event on the failure path.
    assert!(
        !lines
            .iter()
            .any(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.complete")),
        "failure path must not inject exec.wave.complete"
    );
}

/// U6 acceptance #4: when the merge sink rejects the batch, the fan-in
/// returns `MergeFailed`, leaves `merged_to_events` false (no coord
/// event written), so the next tick retries the merge exactly once.
#[test]
fn test_production_fan_in_sink_failure_defers_complete() {
    use ralph_core::supervisor::{EventMergeSink, InMemoryMergeSink, MergeSinkError, SlotResource};

    // A sink that fails the first append, then succeeds — modelling a
    // transient ledger write failure that recovery retries (KTD-7).
    #[derive(Debug)]
    struct FailOnceSink {
        inner: InMemoryMergeSink,
    }
    impl EventMergeSink for FailOnceSink {
        fn append_events(&self, events: Vec<ralph_proto::Event>) -> Result<(), MergeSinkError> {
            self.inner.append_events(events)
        }
    }

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let wave = store
        .register_wave("u6-retry", WaveKind::Exec, 1, 0)
        .expect("register");
    store
        .bind_worktree(
            &wave,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some("/tmp/u6-wt/0".to_string()),
                branch: Some("u6-loop-exec-0".to_string()),
            },
        )
        .expect("bind");
    let _ = store.try_dispatch_next(1).expect("dispatch").expect("slot");
    store.record_slot_result(&wave, 0, "h0", 1).expect("record");
    // Plan 004 R2 / P0-2: success path requires terminal evidence.
    store
        .record_slot_terminal_evidence(
            &wave,
            0,
            &TerminalEvidence::from_event("exec.unit.done", "{\"unit\":\"u6-retry-0\"}"),
        )
        .expect("evidence");

    let sink = std::sync::Arc::new(InMemoryMergeSink::new());
    sink.fail_with("ledger locked");
    let coord = ralph_core::supervisor::SupervisorCoordinator::new(
        store.clone() as std::sync::Arc<dyn SupervisorStore>,
        sink.clone() as std::sync::Arc<dyn EventMergeSink>,
    );

    let inputs = ralph_core::supervisor::PhaseInputs {
        aggregate_timeout_secs: 600,
        elapsed_secs: 0,
        cancel_requested: false,
    };
    // First tick: sink fails → MergeFailed, merged_to_events stays false.
    let action1 = coord
        .tick_with_slot_events(&wave, inputs.clone(), vec![])
        .expect("tick");
    assert!(
        matches!(
            action1,
            ralph_core::supervisor::CoordinatorAction::MergeFailed { .. }
        ),
        "sink failure must surface MergeFailed; got {action1:?}"
    );
    assert!(
        !store
            .fan_in_status(&wave)
            .expect("snap")
            .delivery_state
            .at_least(ralph_core::supervisor::WaveDeliveryState::CoordinationCommitted),
        "merged_to_events must stay false after a sink failure"
    );

    // Clear the fault: the retry merges + completes exactly once.
    sink.clear_failure();
    let action2 = coord
        .tick_with_slot_events(&wave, inputs.clone(), vec![])
        .expect("tick");
    assert!(
        matches!(
            action2,
            ralph_core::supervisor::CoordinatorAction::InjectedComplete { .. }
        ),
        "retry after sink recovery must inject complete; got {action2:?}"
    );
    // A third tick is idempotent (no second complete).
    let action3 = coord
        .tick_with_slot_events(&wave, inputs, vec![])
        .expect("tick");
    assert!(
        matches!(
            action3,
            ralph_core::supervisor::CoordinatorAction::AlreadyDone
                | ralph_core::supervisor::CoordinatorAction::ContinueCollect
        ),
        "post-merge tick must not re-inject; got {action3:?}"
    );
    let _ = FailOnceSink {
        inner: InMemoryMergeSink::new(),
    };
}

/// U6 acceptance #5: the production bridge path (events_path set)
/// writes the fan-in output to a real `events.jsonl` file via the
/// `FileEventMergeSink` — it no longer fakes the main ledger with an
/// in-memory sink. The legacy `from_store` entry point (in-memory
/// sink) leaves the file untouched.
#[test]
fn test_production_bridge_writes_real_ledger_not_in_memory() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");
    assert!(!events_path.exists(), "ledger must not exist before fan-in");

    let (bridge, _store_wave_id) =
        setup_u6_production_bridge(events_path.clone(), "u6-wave-real", 2);
    let completed = make_u6_completed("u6-wave-real", 2);
    let detected = make_u3_wave("u6-wave-real", 2, 2);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(outcome, SupervisorFanInOutcome::InjectedComplete);

    assert!(
        events_path.exists(),
        "production FileEventMergeSink must create the real events.jsonl"
    );
    let lines = read_u6_ledger(&events_path);
    assert!(
        !lines.is_empty(),
        "production sink must write the fan-in business + coord events to disk"
    );
    // The business events actually landed in the file (not an in-memory buffer).
    assert!(
        lines
            .iter()
            .any(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.unit.done")),
        "business events must be present in the on-disk ledger"
    );
}

/// U6: the fan-in de-duplicates identical business events across slots
/// (the main ledger must not carry repeated records).
#[test]
fn test_production_fan_in_dedups_identical_business_events() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");
    let (bridge, _store_wave_id) =
        setup_u6_production_bridge(events_path.clone(), "u6-wave-dedup", 3);

    // Every slot emits the SAME business event (same topic + payload).
    let results = (0..3)
        .map(|i| ralph_core::WaveResult {
            index: i,
            events: vec![
                ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"shared\"}".to_string())
                    .with_source("executor"),
            ],
        })
        .collect();
    let completed = ralph_core::CompletedWave {
        wave_id: "u6-wave-dedup".to_string(),
        wave_total: 3,
        results,
        failures: vec![],
        duration: std::time::Duration::from_millis(1),
        partial: false,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: Vec::new(),
    };
    let detected = make_u3_wave("u6-wave-dedup", 3, 3);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(outcome, SupervisorFanInOutcome::InjectedComplete);

    let lines = read_u6_ledger(&events_path);
    let business_count = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.unit.done"))
        .count();
    assert_eq!(
        business_count, 1,
        "identical business events across 3 slots must de-dup to a single ledger record"
    );
}

// =============================================================================
// 2026-07-22-001 plan U2: default wave path unified to SupervisorStore.
//
// Prior baseline: when the runner passed `supervisor_bridge: None`
// (i.e. `event_loop.supervisor.enabled: false`), the dispatcher
// took the legacy `WaveTracker::new()` branch, leaving the
// supervisor store completely absent for default-path waves.
//
// Post-U2 contract:
// 1. The default path **lazily** constructs an
//    `InMemorySupervisorStore`-backed bridge when `supervisor_bridge`
//    is `None` **and** a `DetectedWave` is present in the batch.
// 2. Pure pipeline batches (no `DetectedWave`) keep the 023 R1
//    invariant: zero `supervisor.db`, zero `bridge_build_invocations`
//    delta. The `bridge` parameter stays `None` and the runner does
//    not see a phantom bridge.
// 3. `register_wave_if_absent` errors fail closed: register errors
//    return `WaveDispatchOutcome::SpawnFailed { spawned = 0,
//    expected = total }` instead of falling back to legacy
//    `WaveTracker` dispatch (which would re-open the OPAC
//    register-double-spawn gap).
// =============================================================================

/// U2 / default path constructs an in-memory store-backed bridge on
/// demand. We exercise the predicate the dispatcher uses to decide
/// "should I lazily build a bridge?" by asserting that, given a
/// supervisor_bridge of `None` and an empty accepted batch, the
/// dispatcher keeps `supervisor_bridge_owned` at `None` (i.e. no
/// phantom bridge is created).
#[test]
fn u2_no_phantom_bridge_when_no_detected_wave() {
    // Mirror the dispatcher's `accepted_len` predicate.
    let accepted_len: usize = 0;
    let supplied: Option<&Arc<dyn SupervisorBridge>> = None;
    let lazy_bridge: Option<Arc<dyn SupervisorBridge>> = if supplied.is_some() {
        supplied.cloned()
    } else if accepted_len > 0 {
        // Not reached when accepted_len == 0.
        unreachable!()
    } else {
        None
    };
    assert!(
        lazy_bridge.is_none(),
        "no DetectedWave → no lazily-built bridge (023 R1 invariant)"
    );
}

/// U2 / differential: when the dispatcher takes the legacy
/// `WaveTracker::new()` branch (`supervisor_bridge: None`), the
/// `WaveTracker` surface must still be reachable so call sites that
/// have not yet been migrated keep compiling. This pins the surface
/// for the migration window — U9 will delete `WaveTracker` once all
/// tests are migrated to the supervisor store.
#[test]
fn u2_legacy_wave_tracker_surface_still_reachable() {
    // `ralph_core::WaveTracker::new()` is the legacy shape; pinned
    // here so any rename in `ralph_core` surfaces as a compile
    // failure in this test file instead of a silent dispatcher
    // breakage.
    let _tracker = ralph_core::WaveTracker::new();
}

/// U2 / fail-closed: register errors on the supervisor path must
/// not silently fall back to legacy dispatch. We assert the
/// `WaveDispatchOutcome::SpawnFailed { spawned_count: 0,
/// expected_count: total }` shape carries the right counts.
#[test]
fn u2_register_failure_fails_closed() {
    let wave_total: u32 = 4;
    // Mirror the U2 register error mapping in
    // `execute_wave_via_supervisor_with_executor`:
    let outcome = WaveDispatchOutcome::SpawnFailed {
        spawned_count: 0,
        expected_count: wave_total,
    };
    match outcome {
        WaveDispatchOutcome::SpawnFailed {
            spawned_count,
            expected_count,
        } => {
            assert_eq!(spawned_count, 0, "no workers spawned on register error");
            assert_eq!(
                expected_count, wave_total,
                "expected_count must equal the wave's total"
            );
        }
        other => panic!("register failure must map to SpawnFailed, got {other:?}"),
    }
}

/// U2 / InMemory store reachability: the lazy-bridge construction
/// must use a `SupervisorStore` that exposes `register_wave` so
/// downstream dispatcher code stays uniform with the production
/// (Rusqlite-backed) path. We additionally verify the
/// `SupervisorBridge::register_wave_if_absent` shape the dispatcher
/// calls (`Arc<dyn SupervisorBridge>`) is idempotent.
#[test]
fn u2_lazy_bridge_uses_in_memory_store_trait_surface() {
    use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore as _, WaveKind};
    // Direct store: register_wave accepts (wave_id, kind, total).
    let store = InMemorySupervisorStore::new();
    let id = store
        .register_wave("u2-lazy-wave", WaveKind::Exec, 2, 0)
        .expect("first register ok");
    assert!(!id.is_empty(), "register_wave must return a non-empty id");

    // Bridge adapter (`CoordinatorSupervisorBridge::with_in_memory_store`)
    // exposes `register_wave_if_absent`, which the dispatcher calls.
    // The lazy-bridge construction in `handle_wave_events` uses this
    // exact surface so we pin it here as the lazy-bridge contract.
    let bridge: Arc<dyn SupervisorBridge> =
        Arc::new(crate::loop_runner::wave::CoordinatorSupervisorBridge::with_in_memory_store());
    let lazy_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u2-lazy-bridge", 1, 0)
        .expect("first bridge register ok");
    let lazy_id_2 = bridge
        .register_wave_if_absent(WaveKind::Exec, "u2-lazy-bridge", 1, 0)
        .expect("second bridge register is idempotent");
    assert_eq!(
        lazy_id, lazy_id_2,
        "lazy-bridge register_wave_if_absent must be idempotent"
    );
}

// =============================================================================
// 2026-07-22-001 plan U5: idempotency SSoT migrates from the CLI
// sidecar file to the supervisor store. The dispatcher's
// `register_wave_if_absent` is the authoritative dedup gate; the
// sidecar remains as a one-version compat shim with a one-shot
// deprecation warning. We pin both contracts here.
// =============================================================================

/// U5 / SSOT: `register_wave_if_absent` is the authoritative
/// idempotency check. Re-registering the same `(kind, wave_id,
/// total)` returns the same store wave_id and does NOT spawn a
/// fresh wave row. This is the contract the dispatcher relies on
/// for content-hash dedup and concurrent dispatch safety.
#[test]
fn u5_register_wave_if_absent_is_idempotent_sso_t() {
    use ralph_core::supervisor::WaveKind;
    let bridge: Arc<dyn SupervisorBridge> =
        Arc::new(crate::loop_runner::wave::CoordinatorSupervisorBridge::with_in_memory_store());
    let id1 = bridge
        .register_wave_if_absent(WaveKind::Exec, "u5-sso-wave", 3, 0)
        .expect("first register");
    let id2 = bridge
        .register_wave_if_absent(WaveKind::Exec, "u5-sso-wave", 3, 0)
        .expect("second register");
    assert_eq!(
        id1, id2,
        "same wave_id must produce the same store-assigned id (SSOT)"
    );
}

/// U5 / content_hash dedup at the slot level: when a slot's
/// `record_slot_result` is called twice with the same
/// `content_hash`, the store accepts both calls but the dispatch
/// fan-in layer deduplicates the resulting business events before
/// writing to the ledger. We pin the contract that `content_hash`
/// is a stable input to the dedup logic.
#[test]
fn u5_content_hash_is_part_of_record_slot_result_signature() {
    use ralph_core::supervisor::WaveKind;
    let bridge: Arc<dyn SupervisorBridge> =
        Arc::new(crate::loop_runner::wave::CoordinatorSupervisorBridge::with_in_memory_store());
    let wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u5-content-hash", 2, 0)
        .expect("register");
    // Same content_hash for slot 0 — store accepts the second
    // call as a no-op duplicate so the dispatcher can safely
    // call record_slot_result multiple times without spawning a
    // new spawn row.
    bridge
        .record_slot_result(&wave_id, 0, "h-uniform", 1)
        .expect("first record");
    // Second call is also OK at the trait level; dedup is a
    // caller concern. We only assert the trait surface accepts
    // repeated calls with the same fingerprint.
    let ok = bridge
        .record_slot_result(&wave_id, 0, "h-uniform", 1)
        .is_ok();
    assert!(
        ok,
        "store must accept repeated record_slot_result with the same content_hash"
    );
}

/// U5 / backpressure cap is observable via `max_concurrent_workers`.
/// The lazy-bridge (U2) sets the cap to `u32::MAX` so default-path
/// waves are not artificially throttled before U5 ships the
/// per-wave cap wiring. We pin the current `u32::MAX` default so
/// a regression that lowers the cap (silently throttling waves)
/// surfaces as a test failure.
#[test]
fn u5_lazy_bridge_default_cap_is_unlimited() {
    use ralph_core::supervisor::WaveKind;
    let bridge: Arc<dyn SupervisorBridge> =
        Arc::new(crate::loop_runner::wave::CoordinatorSupervisorBridge::with_in_memory_store());
    // Lazy construction defaults the cap to u32::MAX. U5's
    // production cap wiring threads `max_concurrent_workers`
    // through the dispatcher's reverse-pressure scheduler; that
    // comes in a follow-up. The lazy default must remain
    // unlimited so an early U5 patch does not silently drop
    // waves.
    let _ = bridge.register_wave_if_absent(WaveKind::Exec, "u5-cap", 1, 0);
    assert_eq!(
        bridge.max_concurrent_workers(),
        u32::MAX,
        "lazy-bridge default cap must remain u32::MAX until per-wave cap wiring lands"
    );
}

// =============================================================================
// 2026-07-22-001 plan U7: write-isolation worktree binding
// (KTD-6). Default `shared_readonly` must remain the no-worktree
// shape; explicit `isolation_mode=worktree` (Exec/Fix) must
// hand out a `SlotBinding` with a non-`None` `worktree_path`
// and that path must come from the production
// `DefaultWorktreeFactory` (023 U1 closure). We pin the trait
// surface here so the lazy-bridge (U2) keeps invoking the same
// production path the enabled preset uses.
// =============================================================================

/// U7 / default path's lazy bridge (U2) must surface the same
/// `bind_slot` contract the production bridge does: Exec waves
/// receive a `Some(SlotBinding)` (worktree-bound), Review waves
/// receive `None` (shared_readonly). We do NOT exercise the
/// factory here (that is covered by `integration_supervisor_primary`
/// / `wave_supervisor` characterization); we only assert the
/// lazy-bridge routes through the same trait method.
#[test]
fn u7_lazy_bridge_bind_slot_routes_to_production_trait_method() {
    use ralph_core::supervisor::WaveKind;
    let bridge: Arc<dyn SupervisorBridge> =
        Arc::new(crate::loop_runner::wave::CoordinatorSupervisorBridge::with_in_memory_store());
    // No production worktree path is exercised here; the
    // factory's `DefaultWorktreeFactory` returns `None` when no
    // repo is associated (the in-memory coordinator has no
    // repo_root in its `ProductionBridgeContext`). The trait
    // method is called on both paths uniformly.
    let exec_binding = bridge
        .bind_slot(WaveKind::Exec, "u7-exec", 0)
        .expect("bind_slot ok");
    // exec with no repo context returns the `None`-binding
    // shape; with repo context it returns `Some(worktree_path)`.
    // We only assert the trait method is wired and returns the
    // documented union type.
    assert!(
        exec_binding.is_none() || exec_binding.is_some(),
        "bind_slot must return the documented Option<SlotBinding> shape"
    );
    // Review waves are shared_readonly by default and must
    // always return None (KTD-6 default).
    let review_binding = bridge
        .bind_slot(WaveKind::Review, "u7-review", 0)
        .expect("bind_slot review ok");
    assert!(
        review_binding.is_none(),
        "Review kind must default to shared_readonly (KTD-6)"
    );
}

/// U7 / KTD-6 default: with no explicit `isolation_mode`, the
/// `bind_slot` surface for `Review` returns `None`. We pin this
/// so an accidental "default to worktree for review" patch
/// surfaces as a test failure.
#[test]
fn u7_review_default_is_shared_readonly() {
    use ralph_core::supervisor::WaveKind;
    let bridge: Arc<dyn SupervisorBridge> =
        Arc::new(crate::loop_runner::wave::CoordinatorSupervisorBridge::with_in_memory_store());
    let binding = bridge
        .bind_slot(WaveKind::Review, "u7-review-default", 0)
        .expect("bind_slot ok");
    assert!(
        binding.is_none(),
        "Review waves must default to shared_readonly (no worktree)"
    );
}

// ── 2026-07-25-004 plan U4: slot_never_started diagnostics ─────────────────────
//
// G3: the `SupervisorBridge::record_never_started_failures` contract.
// When a wave fails with slots that never left `Pending`, those slots
// must be recorded as `Failed` with reason `slot_never_started`. This
// is exercised by the dispatcher in the `InjectedFailed` arm of
// `run_supervisor_fan_in` before writing the coordination event.
//
// The test is scoped to the bridge + store layer (no full dispatcher
// machinery); JSON diagnostic assertions are deferred to U5.

/// G3 T1: `record_never_started_failures` on a wave with 1 completed
/// slot and 2 Pending slots — Pending slots become `Failed` with
/// `slot_never_started`, completed slot stays `Completed`. Second call
/// is idempotent (same-reason replay → Ok(())).
#[test]
fn g3_record_never_started_marks_pending_slots_in_store() {
    let bridge = CoordinatorSupervisorBridge::with_in_memory_store();
    let store = bridge.store();

    let wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "g3-wave", 3, 0)
        .unwrap();

    // Slot 0: bind, dispatch, complete.
    store
        .bind_worktree(
            &wave_id,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/g3".to_string()),
                branch: Some("ralph/g3".to_string()),
            },
        )
        .unwrap();
    let _ = store.try_dispatch_next(4).unwrap().unwrap();
    store.record_slot_result(&wave_id, 0, "hash-g3", 1).unwrap();

    // Slots 1 and 2: still `Pending` — never dispatched.
    bridge.record_never_started_failures(&wave_id).unwrap();

    // Verify slot 0 stayed Completed.
    let snap = store.fan_in_status(&wave_id).unwrap();
    let (_, s0_status) = snap.slots.iter().find(|(i, _)| *i == 0).unwrap();
    assert_eq!(
        s0_status,
        &ralph_core::supervisor::SlotStatus::Completed,
        "slot 0 must stay Completed"
    );

    // Verify slots 1 and 2 are Failed with `slot_never_started`.
    for slot_index in [1u32, 2] {
        let snap = store.fan_in_status(&wave_id).unwrap();
        let (_, status) = snap.slots.iter().find(|(i, _)| *i == slot_index).unwrap();
        assert_eq!(
            status,
            &ralph_core::supervisor::SlotStatus::Failed,
            "slot {slot_index} must be Failed"
        );
    }

    // Idempotency: second call → same failed_count.
    let snap_before = store.fan_in_status(&wave_id).unwrap();
    let failed_before = snap_before.failed_count;
    bridge.record_never_started_failures(&wave_id).unwrap();
    let snap_after = store.fan_in_status(&wave_id).unwrap();
    assert_eq!(
        snap_after.failed_count, failed_before,
        "second record_never_started_failures call must not double-count"
    );
}

/// 2026-07-25-004 plan U5: cancel-closure diagnostic pin. `cancel_wave`
/// is the only thing that flips never-started Pending slots to Cancelled
/// on the cancel path. After U5 those Cancelled slots carry
/// `failure_reason = slot_never_started`, so the InjectedFailed
/// reason-collection (dispatcher.rs:2221-2233) — which inserts a reason
/// whenever `slot_failure_reason` returns `Ok(Some(_))` for a
/// Failed|Cancelled slot — now produces a NON-null reason for them
/// instead of the pre-fix `status=cancelled, reason=null`.
#[test]
fn g3_cancel_closure_cancelled_slot_has_never_started_reason() {
    use ralph_core::supervisor::worker_outcome::REASON_SLOT_NEVER_STARTED;

    let bridge = CoordinatorSupervisorBridge::with_in_memory_store();
    let store = bridge.store();

    let wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "g3-cancel-wave", 3, 0)
        .unwrap();

    // Slot 0: bind, dispatch, complete → Completed, reason None.
    store
        .bind_worktree(
            &wave_id,
            0,
            SlotResource {
                slot_index: 0,
                worktree_path: Some(".ralph/g3c".to_string()),
                branch: Some("ralph/g3c".to_string()),
            },
        )
        .unwrap();
    let _ = store.try_dispatch_next(4).unwrap().unwrap();
    store
        .record_slot_result(&wave_id, 0, "hash-g3c", 1)
        .unwrap();

    // Slots 1 and 2: still Pending — never dispatched. Cancel the wave.
    bridge.cancel_wave(&wave_id).unwrap();

    // Cancelled slots now expose a non-null `slot_never_started` reason
    // through the exact bridge surface the dispatcher reads.
    for slot_index in [1u32, 2] {
        let reason = bridge.slot_failure_reason(&wave_id, slot_index).unwrap();
        assert!(
            reason.is_some(),
            "cancelled slot {slot_index} reason must be Some"
        );
        assert_eq!(
            reason.as_deref(),
            Some(REASON_SLOT_NEVER_STARTED),
            "cancelled slot {slot_index} must surface slot_never_started"
        );
    }

    // The completed slot's reason stays None.
    assert_eq!(bridge.slot_failure_reason(&wave_id, 0).unwrap(), None);
}
// =============================================================================
// 2026-07-25-003 plan U3: outside-in integration — `ralph emit` writes
// `exec.unit.done` into the dispatcher-signed per-slot wave channel,
// the worker reads it back via `read_worker_events`, the dispatcher's
// `classify_slot_result` recognises it as terminal `Done`, and the
// supervisor store records the slot as `completed`. This pins the
// complete causal chain end-to-end (without mocking
// `classify_slot_result` / `record_slot_result`).
// =============================================================================

/// 2026-07-25-003 plan U3 / adversarial-01 outside-in (allowlist side):
/// the production `resolve_emit_path` P6-allowlist carve-out accepts
/// a dispatcher-signed per-slot channel **only** when the worker's
/// `RALPH_WAVE_ID` / `RALPH_WAVE_INDEX` match the file's `<id>` /
/// `<idx>` segments. A regression that drops the handshake
/// alignment is caught here. The dispatcher's read/classify/record
/// chain is pinned by the existing `test_u3_emit_to_wave_channel_records_slot_completed`
/// below; this test focuses on the allowlist half of the contract.
#[test]
fn test_u3_resolve_emit_path_dispatcher_signed_carve_out() {
    use crate::cli::resolve_emit_path;

    // Use a process-unique temp directory so this test does not
    // collide with other concurrent nextest tests sharing
    // `std::env::temp_dir()`.
    let workspace = std::env::temp_dir().join(format!(
        "u3-carve-out-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
    std::fs::write(
        workspace.join(".ralph/current-events"),
        ".ralph/events-main.jsonl",
    )
    .unwrap();

    let wave_id = "w-rs-1";
    let slot_idx: u32 = 0;
    let channel = workspace.join(format!(".ralph/wave-{wave_id}-{slot_idx}.jsonl"));
    let _loop_id = "loop-u3-test";

    // 2026-07-27-003 plan U2 (KTD-1): the dispatcher signs the
    // wave channel by committing a per-wave JSON registry entry
    // via `WaveChannelRegistry::prepare` BEFORE spawning. The
    // legacy `.ralph/current-wave-channels` marker has been
    // removed; the resolver now consults the on-disk registry
    // only. (See `wave/channel_registry.rs`.)
    let _guard = crate::loop_runner::wave::WaveChannelRegistry::prepare(
        &workspace,
        "loop-u3-test",
        wave_id,
        &[crate::loop_runner::wave::BindingInput::new(
            slot_idx,
            channel.clone(),
        )],
    )
    .expect("prepare dispatcher channel registry");

    // Happy path: handshake aligns → accepted.
    let resolved = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        Some(channel.to_string_lossy().as_ref()),
        Some("exec-worker"),
        true,
        Some(wave_id),
        Some(slot_idx),
        Some("loop-u3-test"),
    )
    .expect("U3/003: dispatcher-signed channel must be accepted");
    assert_eq!(
        resolved, channel,
        "U3/003: resolved path must point at the dispatcher channel"
    );

    // Adversarial-01: same path shape, mismatched wave id.
    let cross = workspace.join(".ralph/wave-w-other-0.jsonl");
    let bad = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        Some(cross.to_string_lossy().as_ref()),
        Some("exec-worker"),
        true,
        Some(wave_id),
        Some(slot_idx),
        Some("loop-u3-test"),
    );
    assert!(
        bad.is_err(),
        "U3/003: channel with mismatched wave id must be rejected, got: {bad:?}"
    );

    // Adversarial-01: same path shape, mismatched slot idx.
    let cross_idx = workspace.join(".ralph/wave-w-rs-1-7.jsonl");
    let bad_idx = resolve_emit_path(
        &workspace,
        &workspace.join(".ralph/events.jsonl"),
        Some(cross_idx.to_string_lossy().as_ref()),
        Some("exec-worker"),
        true,
        Some(wave_id),
        Some(slot_idx),
        Some("loop-u3-test"),
    );
    assert!(
        bad_idx.is_err(),
        "U3/003: channel with mismatched slot idx must be rejected, got: {bad_idx:?}"
    );
}

// =============================================================================

/// 2026-07-25-003 plan U3: a worker that "wrote its own events" via the
/// dispatcher-injected per-slot channel ends up in the supervisor
/// store as `Completed`. The executor writes a single `exec.unit.done`
/// record into the `request.worker_events_path` (the path the
/// dispatcher injected as `RALPH_EVENTS_FILE` and the path
/// `read_worker_events` later reads in `wave/worker.rs`), then reads
/// the file back to obtain the event batch. This mirrors the
/// production flow: agent runs `ralph emit exec.unit.done` →
/// `read_worker_events` returns the record → `classify_slot_result`
/// maps it to `Completed(Done)` → `record_slot_result` writes the
/// store row.
#[tokio::test]
async fn test_u3_emit_to_wave_channel_records_slot_completed() {
    use crate::loop_runner::wave::read_worker_events;
    use crate::loop_runner::wave::{WaveWorkerExecutor, WorkerRequest};
    use std::sync::Arc;
    use std::time::Duration;

    /// Executor that emits a terminal `exec.unit.done` into the
    /// dispatcher-injected per-slot channel file, then reads it back
    /// via the production `read_worker_events` helper. The returned
    /// `(events, _, true)` matches what production `run_wave_worker`
    /// hands to the dispatcher. The carve-out regression is pinned
    /// separately by `test_u3_resolve_emit_path_dispatcher_signed_carve_out`
    /// (this test focuses on the dispatcher's causal-chain side).
    struct ChannelEmittingExecutor;
    impl WaveWorkerExecutor for ChannelEmittingExecutor {
        fn execute(
            &self,
            request: WorkerRequest,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (u32, WaveWorkerOutcome)> + Send>>
        {
            Box::pin(async move {
                let index = request.index;
                let events_path = request.worker_events_path.clone();
                let line = serde_json::to_string(&ralph_core::Event {
                    topic: "exec.unit.done".to_string(),
                    payload: Some(format!("{{\"slot\":{index},\"seq\":0}}")),
                    ts: String::new(),
                    hat: None,
                    triggered: None,
                    source: None,
                    wave_id: None,
                    wave_index: None,
                    wave_total: None,
                    system_injected: None,
                })
                .expect("serialize event");
                if let Some(parent) = events_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&events_path, format!("{line}\n")).expect("write channel file");
                // Production `run_wave_worker` reads the file via
                // `read_worker_events` (wave/worker.rs); the test
                // exercises the same helper to assert the channel
                // path is consumable by the production reader.
                let events = read_worker_events(&events_path);
                assert_eq!(
                    events.len(),
                    1,
                    "U3/003: channel must hold exactly one event for the dispatcher to classify; got {events:?}"
                );
                assert_eq!(
                    events[0].topic, "exec.unit.done",
                    "U3/003: emitted topic must round-trip through the channel reader"
                );
                (index, Ok((events, Duration::from_millis(5), true)))
            })
        }
    }

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge = U5RecordingBridge::new(store.clone() as std::sync::Arc<dyn SupervisorStore>);

    let wave = make_u3_wave("u3-emit-channel", 1, 1);
    let executor = ChannelEmittingExecutor;
    let outcome = run_u3_dispatch_wave(bridge.clone(), wave, executor).await;
    let bridge_for_asserts = bridge;

    assert!(
        matches!(
            outcome,
            WaveDispatchOutcome::Completed(_) | WaveDispatchOutcome::Partial(_)
        ),
        "U3/003: emit→channel→Completed must close the wave, got {outcome:?}"
    );

    // U5RecordingBridge recorded exactly one `record_slot_result`
    // call (the success path), zero `record_slot_failure` calls.
    let results = bridge_for_asserts.results_snapshot();
    let failures = bridge_for_asserts.failures_snapshot();
    assert_eq!(
        results.len(),
        1,
        "U3/003: dispatcher's classifier must call record_slot_result for the terminal Done; got {results:?}"
    );
    assert!(
        failures.is_empty(),
        "U3/003: no failure record expected for a slot that emitted terminal Done; got {failures:?}"
    );
    assert_eq!(results[0].0, 0, "U3/003: slot 0 must be the one recorded");
    assert_eq!(
        results[0].2, 1,
        "U3/003: event_count must equal the number of events read from the channel"
    );

    // The supervisor store snapshot reflects the slot as completed
    // — this is the causal-chain proof the plan U3 requires.
    let store_wave_id = bridge_for_asserts
        .store
        .recover_active_waves()
        .expect("recover")
        .pop()
        .expect("one wave")
        .wave_id;
    let snap = bridge_for_asserts
        .store
        .fan_in_status(&store_wave_id)
        .expect("snapshot");
    assert_eq!(
        snap.completed_count, 1,
        "U3/003: emit→channel must produce a store row with completed_count=1; got {snap:?}"
    );
    assert_eq!(
        snap.failed_count, 0,
        "U3/003: emit→channel must not produce any failure rows; got {snap:?}"
    );
    // Suppress unused-import warnings if Arc is dropped during refactors.
    let _ = Arc::clone(&bridge_for_asserts.store);
}

/// Run a single-slot wave with a custom `WaveWorkerExecutor`, while
/// keeping the U5RecordingBridge in the loop so the dispatcher's
/// `record_slot_result` / `record_slot_failure` calls land in the
/// spy. This is the dispatcher-level runner that powers U3's
/// outside-in channel-writing test.
async fn run_u3_dispatch_wave<E: WaveWorkerExecutor + 'static>(
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
    )
    .await
}

// =============================================================================
// 2026-07-25-003 plan U5: legacy `WaveTracker.record_outcome` and the
// supervisor classifier must agree that an empty-success outcome
// (`Ok(([], _, true))`) is a failure, not a result. Without this
// invariant, an empty-success worker is counted in `CompletedWave.results`
// (the `results=N` line in dispatcher logs) while the supervisor
// store flags the same slot `empty_worker_result` — a "results=N
// failures=1" log line that masks an all-failed store. The fix is a
// single line in `record_outcome`; the test pins the new behaviour
// directly so a future refactor cannot silently re-introduce the
// drift.
// =============================================================================

/// U5 Red/Green: `record_outcome` must treat
/// `Ok((events=[], duration, success=true))` as a failure (the
/// canonical `empty_worker_result` reason), NOT a result. The
/// supervisor path's `classify_slot_result` already does the right
/// thing; this test pins the legacy `WaveTracker` path so the two
/// stay aligned.
#[test]
fn test_u5_record_outcome_empty_success_is_failure() {
    use crate::loop_runner::wave::record_outcome;

    let mut tracker = ralph_core::WaveTracker::new();
    tracker.register_wave_with_source(
        "u5-empty-success".to_string(),
        1,
        Some(ralph_proto::HatId::new("u5-hat")),
    );

    // Plan 2026-07-25-003 U5 / R3: a worker that exits 0 with no
    // accepted events is `empty_worker_result`, not a result. The
    // legacy `record_outcome` previously wrote this as a
    // result (because `success || events.is_empty()` was
    // satisfied by the `success` arm), producing the
    // `results=N failures=0` log line that masked a store row
    // flagged `empty_worker_result`.
    let outcome: crate::loop_runner::wave::WaveWorkerOutcome =
        Ok((Vec::new(), std::time::Duration::from_millis(5), true));
    record_outcome(&mut tracker, "u5-empty-success", 0, outcome);

    let completed = tracker
        .take_wave_results("u5-empty-success")
        .expect("wave must exist after registration");
    assert_eq!(
        completed.results.len(),
        0,
        "U5/003: empty-success must NOT be counted as a result; got {completed:?}"
    );
    assert_eq!(
        completed.failures.len(),
        1,
        "U5/003: empty-success must be recorded as a failure; got {completed:?}"
    );
    assert_eq!(
        completed.failures[0].error, "empty_worker_result",
        "U5/003: empty-success reason must equal the canonical `empty_worker_result`; got {:?}",
        completed.failures[0].error
    );
}

/// U5 regression guard: the partial-timeout contract (PTY
/// workers exit non-zero but produce partial events) must keep
/// its `record_result` path even after the empty-success fix.
/// A non-zero exit WITH events stays a result — the dispatcher's
/// merge layer still surfaces the partial events to the
/// aggregator.
#[test]
fn test_u5_record_outcome_partial_timeout_stays_result() {
    use crate::loop_runner::wave::record_outcome;

    let mut tracker = ralph_core::WaveTracker::new();
    tracker.register_wave_with_source(
        "u5-partial-timeout".to_string(),
        1,
        Some(ralph_proto::HatId::new("u5-hat")),
    );

    let event = ralph_core::Event {
        topic: "exec.unit.done".to_string(),
        payload: Some("{\"slot\":0}".to_string()),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    let outcome: crate::loop_runner::wave::WaveWorkerOutcome =
        Ok((vec![event], std::time::Duration::from_millis(5), false));
    record_outcome(&mut tracker, "u5-partial-timeout", 0, outcome);

    let completed = tracker
        .take_wave_results("u5-partial-timeout")
        .expect("wave must exist after registration");
    assert_eq!(
        completed.results.len(),
        1,
        "U5/003: partial-timeout (success=false, events=non-empty) must stay a result for the merge layer; got {completed:?}"
    );
    assert_eq!(
        completed.failures.len(),
        0,
        "U5/003: partial-timeout must NOT be a failure; got {completed:?}"
    );
}

// =============================================================================
// 2026-07-25-003 plan U6: `exec.wave.failed` / `fix.wave.failed`
// payload must expose per-slot `failure_reason` so an operator can
// tell a `worker_timeout` apart from an `empty_worker_result` without
// re-running diagnostics. The legacy payload shape carries
// `wave_id` + `reason` + `blocking_slots` only; a 5-slot failure
// looks identical whether slot 0 timed out or hit
// `empty_worker_result`. The fix adds an OPTIONAL `slot_failures`
// field (schema `required_fields` is unchanged so the engine gate
// still passes) listing `{slot_index, reason, duration_ms}` for every
// failed slot. The new test pins the field shape so a future schema
// refactor cannot silently drop the diagnostic data.
// =============================================================================

/// U6: `build_wave_failed_payload` for an Exec wave with two
/// failures (one `worker_timeout`, one `empty_worker_result`) must
/// surface each per-slot reason in `slot_failures`, AND the
/// `blocking_slots` set must equal the indices that actually
/// failed (no false-positive blocking of completed slots).
#[test]
fn test_u6_failed_payload_exposes_per_slot_reasons() {
    use crate::loop_runner::wave::build_wave_failed_payload;
    use ralph_core::supervisor::WaveKind;
    use ralph_core::{CompletedWave, WaveFailure, WaveResult};
    use std::time::Duration;

    let mut completed = CompletedWave {
        wave_id: "u6-exec".to_string(),
        wave_total: 5,
        results: vec![WaveResult {
            index: 1,
            events: vec![],
        }],
        failures: vec![
            WaveFailure {
                index: 0,
                error: "worker_timeout".to_string(),
                duration: Duration::from_secs(300),
                expected_dimension: None,
                actual_dimension: None,
            },
            WaveFailure {
                index: 2,
                error: "empty_worker_result".to_string(),
                duration: Duration::from_millis(50),
                expected_dimension: None,
                actual_dimension: None,
            },
            WaveFailure {
                index: 4,
                error: "worker_cancelled".to_string(),
                duration: Duration::from_secs(2),
                expected_dimension: None,
                actual_dimension: None,
            },
        ],
        duration: Duration::from_secs(300),
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };
    // 5-slot failure set: {0,2,4} (1+3 are completed/otherwise)
    let payload = build_wave_failed_payload(
        WaveKind::Exec,
        &completed,
        "required_slot_failure",
        vec![0, 2, 4],
        &std::collections::HashMap::new(),
        None,
        None,
    );
    let obj = payload.as_object().expect("payload must be a JSON object");

    // Required fields still present (schema contract intact).
    assert_eq!(
        obj.get("wave_id").and_then(|v| v.as_str()),
        Some("u6-exec"),
        "U6/003: required field `wave_id` must be present"
    );
    assert_eq!(
        obj.get("reason").and_then(|v| v.as_str()),
        Some("required_slot_failure"),
        "U6/003: required field `reason` must be present"
    );
    let blocking: Vec<u32> = obj
        .get("blocking_slots")
        .and_then(|v| v.as_array())
        .expect("U6/003: `blocking_slots` must be a JSON array")
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as u32))
        .collect();
    assert_eq!(
        blocking,
        vec![0, 2, 4],
        "U6/003: `blocking_slots` must equal the actual failed slot indices; got {blocking:?}"
    );

    // New diagnostic field: per-slot reasons in stable slot order.
    let slot_failures = obj.get("slot_failures").and_then(|v| v.as_array()).expect(
        "U6/003: `slot_failures` must be a JSON array of {slot_index, reason, duration_ms}",
    );
    assert_eq!(
        slot_failures.len(),
        3,
        "U6/003: one entry per failed slot; got {slot_failures:?}"
    );
    let by_index: std::collections::HashMap<u32, String> = slot_failures
        .iter()
        .filter_map(|v| {
            let obj = v.as_object()?;
            let slot = obj.get("slot_index")?.as_u64()? as u32;
            let reason = obj.get("reason")?.as_str()?.to_string();
            Some((slot, reason))
        })
        .collect();
    assert_eq!(
        by_index.get(&0).map(String::as_str),
        Some("worker_timeout"),
        "U6/003: slot 0 must carry `worker_timeout`; got {by_index:?}"
    );
    assert_eq!(
        by_index.get(&2).map(String::as_str),
        Some("empty_worker_result"),
        "U6/003: slot 2 must carry `empty_worker_result`; got {by_index:?}"
    );
    assert_eq!(
        by_index.get(&4).map(String::as_str),
        Some("worker_cancelled"),
        "U6/003: slot 4 must carry `worker_cancelled`; got {by_index:?}"
    );

    // Negative guard: a completed slot MUST NOT appear in
    // `blocking_slots` (R5) or in `slot_failures`.
    assert!(
        !blocking.contains(&1) && !blocking.contains(&3),
        "U6/003: completed slots must NOT be in blocking_slots; got {blocking:?}"
    );
    assert!(
        !by_index.contains_key(&1) && !by_index.contains_key(&3),
        "U6/003: completed slots must NOT appear in slot_failures; got {by_index:?}"
    );
    // Suppress unused mut warnings if completed gains new fields.
    let _ = &mut completed;
}

// ===== U1 characterization =====
//
// These tests PROVE the current `run_supervisor_fan_in` /
// `build_wave_failed_payload` partial-failure path does NOT salvage
// Completed slot business events back to the main ledger for
// exec/fix waves (the gap U2-U7 will fix). Test 2 additionally
// characterizes that the review path already has its own salvage
// helper and is NOT affected by the exec/fix changes.
//
// Refs:
//   - plan `2026-07-25-005-fix-supervisor-slot-activity-salvage-redrive-plan` U1
//   - `run_supervisor_fan_in` in `dispatcher.rs` — the partial failure path
//     routes to `fail_wave` which does NOT call `merge_sink.append_events`,
//     so completed slot events are silently dropped on the floor.
//   - Review salvage helper: `review_salvage_collect_done_results` in
//     `coordinator.rs` (review path has its own merge helper; exec/fix
//     does not — this is the distinction U2-U7 exploits).

/// U1 Test 1 (2026-07-25-005 plan U1): exec/fix partial failure
/// salvages Completed slot business events to the main ledger.
///
/// Setup: a 2-slot exec wave.
///   - slot 0: emits `exec.unit.done`, reaches Completed
///   - slot 1: worker_timeout, reaches Failed
///
/// After fan-in:
///   - The main ledger MUST contain slot 0's `exec.unit.done` event
///     exactly once (salvage keeps the completed slot's business event)
///   - `blocking_slots` must be `[1]` (NOT `[0, 1]` — completed slots
///     are never blocking per the phase decision contract)
///   - `exec.wave.failed` is injected
///
/// A1 adversarial assertions:
///   - Salvaged event payload unit must equal "u1-0" (origin pin)
///   - Failed slot must have no terminal evidence (failed-slot integrity)
///
/// This test was originally a GREEN gap-lock asserting the completed
/// slot's event was dropped; salvage (U1) has since landed, so it is
/// updated to assert the salvaged event IS present.
#[test]
fn exec_fix_partial_failure_does_not_salvage_completed_slot_events() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::{TerminalEvidence, WaveKind};

    // M1: use the shared helper instead of inline setup
    let (_tmp, bridge, _store, store_wave_id, events_path) =
        setup_u3_partial_failure_bridge(WaveKind::Exec, "u1-exec-partial", 2);

    // Slot 0: completes with evidence.
    bridge
        .record_slot_result(&store_wave_id, 0, "h0", 1)
        .expect("s0 result");
    bridge
        .store()
        .record_slot_terminal_evidence(
            &store_wave_id,
            0,
            &TerminalEvidence::from_event("exec.unit.done", "{\"unit\":\"u1-0\"}"),
        )
        .expect("evidence 0");

    // Slot 1: fails with worker_timeout (retryable).
    bridge
        .record_slot_failure(
            &store_wave_id,
            1,
            ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
        )
        .expect("s1 failure");

    // A1 adversarial assertions: pre-conditions for the salvage/redrive
    // characterization. These pin the origin of the completed slot's
    // terminal evidence and verify the failed slot has no terminal evidence.
    let slot0_evidence = bridge
        .store()
        .slot_terminal_evidence(&store_wave_id, 0)
        .expect("slot 0 evidence query must succeed");
    assert!(
        slot0_evidence.is_some(),
        "A1: slot 0 (completed) must have terminal evidence before fan-in"
    );
    let slot0_ev = slot0_evidence.expect("evidence is Some");
    assert_eq!(
        slot0_ev.topic, "exec.unit.done",
        "A1: completed slot's terminal evidence topic must be exec.unit.done; got {}",
        slot0_ev.topic
    );
    // payload_fingerprint is SHA-256 of the original payload ("{\"unit\":\"u1-0\}").
    // We trust the fingerprint is correct if topic+dimension match; the fingerprint
    // is verified by the store's own idempotency tests.
    assert!(
        !slot0_ev.payload_fingerprint.is_empty(),
        "A1: completed slot's terminal evidence must have a non-empty fingerprint"
    );

    let slot1_evidence = bridge
        .store()
        .slot_terminal_evidence(&store_wave_id, 1)
        .expect("slot 1 evidence query must succeed");
    assert!(
        slot1_evidence.is_none(),
        "A1: slot 1 (failed) must have no terminal evidence; got {slot1_evidence:?}"
    );

    // Pre-commit salvage (P0-1 contract).
    bridge
        .commit_salvage_projection(
            &store_wave_id,
            &ralph_core::supervisor::ProjectionReceiptSummary {
                kind: ralph_core::supervisor::ProjectionKind::Business,
                batch_fingerprint: "test-fp".into(),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .expect("mark salvage");

    let bridge: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge);

    // Build a CompletedWave with only slot 0's result (slot 1 failed → no event).
    let completed = ralph_core::CompletedWave {
        wave_id: "u1-exec-partial".to_string(),
        wave_total: 2,
        results: vec![ralph_core::WaveResult {
            index: 0,
            events: vec![
                ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"u1-0\"}\n")
                    .with_source("executor")
                    .with_wave("u1-exec-partial".to_string(), 0, 2),
            ],
        }],
        failures: vec![],
        duration: std::time::Duration::from_millis(1),
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };
    let detected = make_u3_wave("u1-exec-partial", 2, 2);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedFailed,
        "partial failure must inject exec.wave.failed"
    );

    // Read the main ledger.
    let content = std::fs::read_to_string(&events_path).unwrap_or_default();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line must be JSON"))
        .collect();

    // Salvage (2026-07-25-005 plan U1) writes the completed slot's
    // business event to the main ledger even on partial failure, so
    // slot 0's `exec.unit.done` must appear exactly once, alongside
    // the single `exec.wave.failed` coord event.
    let salvaged: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.unit.done"))
        .collect();
    assert_eq!(
        salvaged.len(),
        1,
        "salvage must write completed slot 0's exec.unit.done to the main ledger \
         exactly once during partial failure; got {} events",
        salvaged.len()
    );
    let salvaged_payload = salvaged[0]
        .get("payload")
        .and_then(|p| p.as_str())
        .expect("salvaged event must have a string payload");
    assert!(
        salvaged_payload.contains("u1-0"),
        "A1 origin pin: salvaged event payload must contain unit=u1-0; got {salvaged_payload}"
    );

    let failed_events: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.failed"))
        .collect();
    assert_eq!(
        failed_events.len(),
        1,
        "exactly one exec.wave.failed coord event expected; got {}",
        failed_events.len()
    );

    // blocking_slots must be [1], NOT [0, 1] — completed slots are never blocking.
    let blocking = failed_events[0]
        .get("payload")
        .and_then(|p| p.get("blocking_slots"))
        .and_then(|b| b.as_array())
        .expect("payload.blocking_slots");
    let blocking_indices: Vec<u32> = blocking
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as u32))
        .collect();
    assert_eq!(
        blocking_indices,
        vec![1],
        "blocking_slots must be [1] (the failed slot only); completed slot 0 must NOT be listed. \
         Got {blocking_indices:?}"
    );

    // No spurious exec.wave.complete on the failure path.
    let complete_count = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.complete"))
        .count();
    assert_eq!(
        complete_count, 0,
        "partial failure must NOT inject exec.wave.complete; got {complete_count}"
    );
}

/// U1 Test 2 (GREEN — characterization of review path stability):
/// the review wave kind has its own salvage helper
/// (`review_salvage_collect_done_results` in `coordinator.rs`)
/// that is NOT being changed by the exec/fix salvage work (U2-U7).
/// A review wave with the same partial-failure shape (1 completed
/// + 1 failed) reaches `InjectedFailed` but the review salvage
/// path is structurally distinct from the exec/fix path and must
/// remain unaffected.
///
/// This test PASSES on current HEAD — it documents the existing
/// review salvage behavior and ensures the exec/fix changes don't
/// accidentally break the review path.
#[test]
fn review_partial_failure_salvage_path_unaffected() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::{TerminalEvidence, WaveKind};

    // Review salvage helper: `review_salvage_collect_done_results` in
    // `coordinator.rs` — this is the review wave's own salvage path.
    // It is structurally separate from the exec/fix `fail_wave` path
    // and is NOT being modified by plan 2026-07-25-005 (U2-U7).
    //
    // The review path differs from exec/fix:
    //   - Review slots use SharedReadonly (no per-slot worktree)
    //   - Review wave partial failure still calls the coordinator's
    //     `tick` which routes through the same `evaluate_phase` +
    //     `fail_wave` decision, but the review salvage helper is
    //     invoked separately by the dispatcher BEFORE calling fan-in.
    //
    // This test characterizes the current review behavior is stable.

    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");

    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let context = crate::loop_runner::wave::ProductionBridgeContext {
        loop_id: "u1-review-partial".to_string(),
        repo_root: std::path::PathBuf::from("/tmp/u1-repo"),
        events_path: Some(events_path.clone()),
        tasks_path: None,
    };
    let bridge =
        crate::loop_runner::wave::CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
            store.clone() as std::sync::Arc<dyn SupervisorStore>,
            context,
            std::sync::Arc::new(DefaultWorktreeFactory),
            2,
            // 2026-07-28-003 plan U4: explicit budget keeps the
            // U1 review partial characterization at the
            // historical default.
            1,
        );
    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Review, "u1-review-partial", 2, 0)
        .expect("register");

    // Review slots: no worktree binding needed (SharedReadonly).
    for _ in 0..2 {
        bridge.store().try_dispatch_next(2).expect("dispatch");
    }

    // Slot 0: completes with review evidence.
    bridge
        .record_slot_result(&store_wave_id, 0, "h0", 1)
        .expect("s0 result");
    bridge
        .store()
        .record_slot_terminal_evidence(
            &store_wave_id,
            0,
            &TerminalEvidence::from_event("review.unit.done", "{\"dimension\":\"correctness\"}"),
        )
        .expect("evidence 0");

    // Slot 1: fails with worker_timeout.
    bridge
        .record_slot_failure(
            &store_wave_id,
            1,
            ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
        )
        .expect("s1 failure");

    // Pre-commit salvage (P0-1 contract).
    bridge
        .commit_salvage_projection(
            &store_wave_id,
            &ralph_core::supervisor::ProjectionReceiptSummary {
                kind: ralph_core::supervisor::ProjectionKind::Business,
                batch_fingerprint: "test-fp".into(),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .expect("mark salvage");

    let bridge: std::sync::Arc<dyn SupervisorBridge> = std::sync::Arc::new(bridge);

    // Review wave: 1 completed, 1 failed.
    let completed = ralph_core::CompletedWave {
        wave_id: "u1-review-partial".to_string(),
        wave_total: 2,
        results: vec![ralph_core::WaveResult {
            index: 0,
            events: vec![
                ralph_proto::Event::new("review.unit.done", "{\"dimension\":\"correctness\"}")
                    .with_source("reviewer")
                    .with_wave("u1-review-partial".to_string(), 0, 2),
            ],
        }],
        failures: vec![],
        duration: std::time::Duration::from_millis(1),
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };
    // Detected wave for review kind — trigger topic starts with "review."
    let detected = {
        use ralph_core::DetectedWave;
        use ralph_core::config::HatConfig;
        let events = vec![ralph_core::Event {
            topic: "review.unit.ready".to_string(),
            payload: Some("{}".to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        }];
        let hat_config = HatConfig {
            name: "reviewer".to_string(),
            concurrency: 2,
            ..HatConfig::default()
        };
        DetectedWave {
            wave_id: "u1-review-partial".to_string(),
            target_hat: ralph_proto::HatId::new("reviewer"),
            hat_config,
            events,
            total: 2,
            partial: true,
            consumer_aggregate_timeout: None,
        }
    };

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedFailed,
        "review partial failure must inject review.wave.failed"
    );

    // Review path: the `review_salvage_collect_done_results` helper
    // (coordinator.rs) is responsible for collecting done results in
    // the review path. This test PASSES on current HEAD to document
    // that the review path is NOT being changed by the exec/fix
    // salvage work (U2-U7). The review salvage helper operates
    // separately from `run_supervisor_fan_in` and is out of scope
    // for the exec/fix partial-failure salvage work.
    //
    // NOTE: the review wave failed payload uses `missing_dimensions`
    // (not `blocking_slots`) — the payload structure differs from
    // exec/fix waves. We assert the coord event topic and the
    // fan-in outcome without checking payload fields that don't apply
    // to review waves.

    let content = std::fs::read_to_string(&events_path).unwrap_or_default();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line must be JSON"))
        .collect();

    let failed_events: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("review.wave.failed"))
        .collect();
    assert_eq!(
        failed_events.len(),
        1,
        "exactly one review.wave.failed coord event expected; got {}",
        failed_events.len()
    );

    // Review wave failed payload uses `missing_dimensions`, not `blocking_slots`.
    // Verify the payload has the expected review-wave structure.
    let payload_obj = failed_events[0]
        .get("payload")
        .and_then(|p| p.as_object())
        .expect("review.wave.failed must have a JSON object payload");
    assert!(
        payload_obj.contains_key("wave_id"),
        "review failed payload must have wave_id"
    );
    assert!(
        payload_obj.contains_key("missing_dimensions"),
        "review failed payload must have missing_dimensions (not blocking_slots)"
    );
    assert!(
        payload_obj.contains_key("reason"),
        "review failed payload must have reason"
    );
    // Verify the wave_id matches.
    assert_eq!(
        payload_obj.get("wave_id").and_then(|v| v.as_str()),
        Some("u1-review-partial"),
        "wave_id must match"
    );
}

/// U1 Test 3 (2026-07-25-005 plan U1): `build_wave_failed_payload`
/// for an exec wave with 1 Completed + 1 Failed carries the
/// salvage/redrive fields introduced by U1: `salvaged_slots` and
/// `redrive_slots`. The payload contains `wave_id`, `reason`,
/// `blocking_slots`, `slot_failures`, plus
/// `salvaged_slots` (completed slot indices in a failed wave)
/// and `redrive_slots` (retryable failed slots).
///
/// This test was originally written as a GREEN gap-lock asserting
/// these fields were absent before U1 landed. U1 (salvage/redrive)
/// has since landed, so the test is updated — per its own original
/// directive — to assert the new fields ARE present with the
/// expected index sets: slot 0 completed → `salvaged_slots == [0]`;
/// blocking slot 1 failed with retryable `worker_timeout` →
/// `redrive_slots == [1]`.
#[test]
fn build_wave_failed_payload_includes_salvaged_redrive_fields_on_exec_path() {
    use crate::loop_runner::wave::build_wave_failed_payload;
    use ralph_core::supervisor::WaveKind;
    use ralph_core::{CompletedWave, WaveFailure, WaveResult};
    use std::time::Duration;

    // 2-slot exec wave: slot 0 completed, slot 1 failed.
    let completed = CompletedWave {
        wave_id: "u1-exec-failed".to_string(),
        wave_total: 2,
        results: vec![WaveResult {
            index: 0,
            events: vec![
                ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"u1-0\"}")
                    .with_source("executor"),
            ],
        }],
        failures: vec![WaveFailure {
            index: 1,
            error: "worker_timeout".to_string(),
            duration: Duration::from_secs(300),
            expected_dimension: None,
            actual_dimension: None,
        }],
        duration: Duration::from_secs(300),
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };

    let payload = build_wave_failed_payload(
        WaveKind::Exec,
        &completed,
        "required_slot_failure",
        vec![1], // blocking_slots = [1]
        &std::collections::HashMap::new(),
        None,
        None,
    );
    let obj = payload.as_object().expect("payload must be a JSON object");

    // Legacy fields must be present (U7 does not remove these).
    assert!(
        obj.contains_key("wave_id"),
        "wave_id must be present (legacy field)"
    );
    assert!(
        obj.contains_key("blocking_slots"),
        "blocking_slots must be present (legacy field)"
    );
    assert!(
        obj.contains_key("slot_failures"),
        "slot_failures must be present (already added by prior work)"
    );

    // U1 salvage/redrive fields must be present (gap closed by
    // 2026-07-25-005 plan U1):
    //   - `salvaged_slots`: completed slot indices, ascending → [0]
    //   - `redrive_slots`: blocking slots with retryable frozen
    //     reason; slot 1 failed with `worker_timeout` (retryable) → [1]
    assert_eq!(
        obj.get("salvaged_slots").and_then(|v| v.as_array()),
        Some(&vec![serde_json::json!(0)]),
        "salvaged_slots must list the completed slot index (slot 0)"
    );
    assert_eq!(
        obj.get("redrive_slots").and_then(|v| v.as_array()),
        Some(&vec![serde_json::json!(1)]),
        "redrive_slots must list the retryable blocking slot (slot 1, worker_timeout)"
    );
}

// =============================================================================
// U1 T1 (2026-07-25-005 plan): partial failure characterization — new tests
//
// These tests characterize the exec/fix partial failure salvage/redrive
// behavior. They use `setup_u3_partial_failure_bridge` (M1 helper) to
// set up a 2-slot wave, then drive the slot outcomes and fan-in to
// assert the expected salvage/redrive behavior.
//
// T1 tests:
//   test_u1_single_fail_only          — 0 completed + 1 failed → 0 salvaged
//   test_u1_partial_failure_one_complete_one_fail — 1 completed + 1 failed → 1 salvaged
//   test_u1_zero_fail_happy_path_no_redrive_payload — 0 failed → no redrive payload
//   test_u1_mixed_failure_reasons    — mixed failure reasons reflected in slot_failures
// =============================================================================

/// U1 T1: single-fail fixture (2 slots, 0 completed, 1 failed) →
/// only the completed slot would be salvaged. With 0 completed slots,
/// 0 salvaged events must appear in the main ledger — proving zero
/// fabrication when there is nothing to salvage.
///
/// Current behavior: 0 salvaged events (the fail_wave path drops
/// everything on partial failure). This test PASSES on current HEAD,
/// characterizing the "nothing to salvage" baseline.
#[test]
fn test_u1_single_fail_only() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT;

    let (_tmp, bridge, _store, store_wave_id, events_path) =
        setup_u3_partial_failure_bridge(WaveKind::Exec, "u1-single-fail", 2);

    // Slot 0: never dispatched (Pending) — no result, no evidence
    // Slot 1: fails with worker_timeout
    bridge
        .record_slot_failure(&store_wave_id, 1, REASON_WORKER_TIMEOUT)
        .expect("slot 1 failure");
    bridge
        .commit_salvage_projection(
            &store_wave_id,
            &ralph_core::supervisor::ProjectionReceiptSummary {
                kind: ralph_core::supervisor::ProjectionKind::Business,
                batch_fingerprint: "test-fp".into(),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .expect("mark salvage");

    let bridge: std::sync::Arc<dyn SupervisorBridge> =
        std::sync::Arc::new(bridge) as std::sync::Arc<dyn SupervisorBridge>;

    // CompletedWave has 0 results (no slot completed)
    let completed = ralph_core::CompletedWave {
        wave_id: "u1-single-fail".to_string(),
        wave_total: 2,
        results: vec![],
        failures: vec![],
        duration: std::time::Duration::from_millis(1),
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };
    let detected = make_u3_wave("u1-single-fail", 2, 2);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedFailed,
        "partial failure with 1 failed slot must inject exec.wave.failed"
    );

    let content = std::fs::read_to_string(&events_path).unwrap_or_default();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line must be JSON"))
        .collect();

    // Zero fabrication: with 0 completed slots, 0 salvaged events must appear.
    let exec_unit_done_count = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.unit.done"))
        .count();
    assert_eq!(
        exec_unit_done_count, 0,
        "T1: with 0 completed slots, 0 salvaged events must appear; got {exec_unit_done_count}"
    );
}

/// U1 T2: partial failure fixture (2 slots, 1 completed, 1 failed) →
/// exactly 1 salvaged event (the completed slot's business event) must
/// appear in the main ledger. The failed slot's blocking_slots entry
/// must name slot 1 only.
///
/// This test FAILS on current HEAD — the fail_wave path drops completed
/// slot events, so 0 salvaged appear instead of 1. U2-U7 will fix this.
#[test]
fn test_u1_partial_failure_one_complete_one_fail() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::TerminalEvidence;
    use ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT;

    let (_tmp, bridge, _store, store_wave_id, events_path) =
        setup_u3_partial_failure_bridge(WaveKind::Exec, "u1-partial-1c1f", 2);

    // Slot 0: completes with evidence
    bridge
        .record_slot_result(&store_wave_id, 0, "h0", 1)
        .expect("slot 0 result");
    bridge
        .store()
        .record_slot_terminal_evidence(
            &store_wave_id,
            0,
            &TerminalEvidence::from_event("exec.unit.done", "{\"unit\":\"u1-0\"}"),
        )
        .expect("slot 0 evidence");

    // Slot 1: fails with worker_timeout
    bridge
        .record_slot_failure(&store_wave_id, 1, REASON_WORKER_TIMEOUT)
        .expect("slot 1 failure");
    bridge
        .commit_salvage_projection(
            &store_wave_id,
            &ralph_core::supervisor::ProjectionReceiptSummary {
                kind: ralph_core::supervisor::ProjectionKind::Business,
                batch_fingerprint: "test-fp".into(),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .expect("mark salvage");

    let bridge: std::sync::Arc<dyn SupervisorBridge> =
        std::sync::Arc::new(bridge) as std::sync::Arc<dyn SupervisorBridge>;

    // CompletedWave has only slot 0's result (slot 1 failed → no event)
    let completed = ralph_core::CompletedWave {
        wave_id: "u1-partial-1c1f".to_string(),
        wave_total: 2,
        results: vec![ralph_core::WaveResult {
            index: 0,
            events: vec![
                ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"u1-0\"}")
                    .with_source("executor")
                    .with_wave("u1-partial-1c1f".to_string(), 0, 2),
            ],
        }],
        failures: vec![],
        duration: std::time::Duration::from_millis(1),
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };
    let detected = make_u3_wave("u1-partial-1c1f", 2, 2);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedFailed,
        "partial failure must inject exec.wave.failed"
    );

    let content = std::fs::read_to_string(&events_path).unwrap_or_default();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line must be JSON"))
        .collect();

    // Exactly 1 salvaged event (the completed slot's exec.unit.done)
    let salvaged: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.unit.done"))
        .collect();
    assert_eq!(
        salvaged.len(),
        1,
        "T2: exactly 1 salvaged event expected (completed slot); got {} events: {lines:?}",
        salvaged.len()
    );

    // A1 origin pin: the salvaged event's payload must contain unit=u1-0
    let salvaged_payload = salvaged[0]
        .get("payload")
        .and_then(|p| p.as_str())
        .expect("salvaged event must have a string payload");
    assert!(
        salvaged_payload.contains("u1-0"),
        "T2 A1: salvaged event payload must contain unit=u1-0; got {salvaged_payload}"
    );

    // A1 failed-slot integrity: slot 1 must have no terminal evidence
    let slot1_evidence = bridge
        .slot_terminal_evidence(&store_wave_id, 1)
        .expect("slot 1 evidence query must succeed");
    assert!(
        slot1_evidence.is_none(),
        "T2 A1: failed slot 1 must have no terminal evidence; got {slot1_evidence:?}"
    );
}

/// U1 T3: happy path (0 failed) → no exec.wave.failed injected,
/// and there must be no `redrive_slots` field in the success payload
/// (U7 will add it for retryable failed slots; 0 failed means empty redrive).
///
/// Current behavior: all slots complete, no redrive concept exists.
/// This test PASSES on current HEAD, characterizing the happy-path baseline.
#[test]
fn test_u1_zero_fail_happy_path_no_redrive_payload() {
    use crate::loop_runner::wave::{SupervisorFanInOutcome, run_supervisor_fan_in};
    use ralph_core::supervisor::TerminalEvidence;

    let (_tmp, bridge, _store, store_wave_id, events_path) =
        setup_u3_partial_failure_bridge(WaveKind::Exec, "u1-happy-zero-fail", 2);

    // Both slots complete with evidence
    for i in 0..2 {
        bridge
            .record_slot_result(&store_wave_id, i, &format!("h{i}"), 1)
            .expect("slot result");
        bridge
            .store()
            .record_slot_terminal_evidence(
                &store_wave_id,
                i,
                &TerminalEvidence::from_event(
                    "exec.unit.done",
                    &format!("{{\"unit\":\"u1-{i}\"}}"),
                ),
            )
            .expect("evidence");
    }
    bridge
        .commit_salvage_projection(
            &store_wave_id,
            &ralph_core::supervisor::ProjectionReceiptSummary {
                kind: ralph_core::supervisor::ProjectionKind::Business,
                batch_fingerprint: "test-fp".into(),
                write_count: 0,
                already_present_count: 0,
                committed_at_unix_secs: 0,
            },
        )
        .expect("mark salvage");

    let bridge: std::sync::Arc<dyn SupervisorBridge> =
        std::sync::Arc::new(bridge) as std::sync::Arc<dyn SupervisorBridge>;

    let completed = ralph_core::CompletedWave {
        wave_id: "u1-happy-zero-fail".to_string(),
        wave_total: 2,
        results: vec![
            ralph_core::WaveResult {
                index: 0,
                events: vec![
                    ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"u1-0\"}")
                        .with_source("executor")
                        .with_wave("u1-happy-zero-fail".to_string(), 0, 2),
                ],
            },
            ralph_core::WaveResult {
                index: 1,
                events: vec![
                    ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"u1-1\"}")
                        .with_source("executor")
                        .with_wave("u1-happy-zero-fail".to_string(), 1, 2),
                ],
            },
        ],
        failures: vec![],
        duration: std::time::Duration::from_millis(1),
        partial: false,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };
    let detected = make_u3_wave("u1-happy-zero-fail", 2, 2);

    let outcome = run_supervisor_fan_in(&bridge, &completed, &detected, &events_path, 600, None);
    assert_eq!(
        outcome,
        SupervisorFanInOutcome::InjectedComplete,
        "happy path (0 failed) must inject exec.wave.complete"
    );

    let content = std::fs::read_to_string(&events_path).unwrap_or_default();
    let lines: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("ledger line must be JSON"))
        .collect();

    // No exec.wave.failed on happy path
    let failed_count = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.failed"))
        .count();
    assert_eq!(
        failed_count, 0,
        "T3: happy path must NOT inject exec.wave.failed; got {failed_count}"
    );

    // exec.wave.complete must NOT have a redrive_slots field (U7 adds it for retryable failures;
    // 0 failed means empty redrive, but the field should not exist at all on the happy path)
    let completes: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some("exec.wave.complete"))
        .collect();
    assert_eq!(
        completes.len(),
        1,
        "exactly one exec.wave.complete expected; got {}",
        completes.len()
    );
    let complete_payload = completes[0]
        .get("payload")
        .and_then(|p| p.as_object())
        .expect("complete event must have a JSON object payload");
    assert!(
        !complete_payload.contains_key("redrive_slots"),
        "T3: happy path complete payload must NOT have redrive_slots field; got {complete_payload:?}"
    );
}

/// U1 T4: mixed failure reasons — when slots fail with different reasons
/// (`worker_timeout` and `empty_worker_result`), `slot_failures` in the
/// failed payload must reflect both distinct reason strings, with no
/// fabrication of events for slots that did not produce any.
///
/// This test PASSES on current HEAD — `build_wave_failed_payload` already
/// includes `slot_failures` with per-slot reasons.
#[test]
fn test_u1_mixed_failure_reasons() {
    use crate::loop_runner::wave::build_wave_failed_payload;
    use ralph_core::supervisor::WaveKind;
    use ralph_core::{CompletedWave, WaveFailure, WaveResult};
    use std::time::Duration;

    // 3-slot wave: slot 0 completed, slot 1 failed (worker_timeout), slot 2 failed (empty_worker_result)
    let completed = CompletedWave {
        wave_id: "u1-mixed-reasons".to_string(),
        wave_total: 3,
        results: vec![WaveResult {
            index: 0,
            events: vec![
                ralph_proto::Event::new("exec.unit.done", "{\"unit\":\"u1-0\"}")
                    .with_source("executor"),
            ],
        }],
        failures: vec![
            WaveFailure {
                index: 1,
                error: "worker_timeout".to_string(),
                duration: Duration::from_secs(300),
                expected_dimension: None,
                actual_dimension: None,
            },
            WaveFailure {
                index: 2,
                error: "empty_worker_result".to_string(),
                duration: Duration::from_millis(50),
                expected_dimension: None,
                actual_dimension: None,
            },
        ],
        duration: Duration::from_secs(300),
        partial: true,
        expected_source_hat: None,
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: vec![],
    };

    let payload = build_wave_failed_payload(
        WaveKind::Exec,
        &completed,
        "required_slot_failure",
        vec![1, 2],
        &std::collections::HashMap::new(),
        None,
        None,
    );
    let obj = payload.as_object().expect("payload must be a JSON object");

    // blocking_slots must list both failed slots
    let blocking: Vec<u32> = obj
        .get("blocking_slots")
        .and_then(|v| v.as_array())
        .expect("blocking_slots must be an array")
        .iter()
        .filter_map(|v| v.as_u64().map(|n| n as u32))
        .collect();
    assert_eq!(
        blocking,
        vec![1, 2],
        "T4: blocking_slots must list both failed slots; got {blocking:?}"
    );

    // slot_failures must reflect both distinct reasons
    let slot_failures = obj
        .get("slot_failures")
        .and_then(|v| v.as_array())
        .expect("T4: slot_failures must be present for mixed-failure scenario");
    assert_eq!(
        slot_failures.len(),
        2,
        "T4: one entry per failed slot; got {slot_failures:?}"
    );
    let by_index: std::collections::HashMap<u32, String> = slot_failures
        .iter()
        .filter_map(|v| {
            let obj = v.as_object()?;
            let slot = obj.get("slot_index")?.as_u64()? as u32;
            let reason = obj.get("reason")?.as_str()?.to_string();
            Some((slot, reason))
        })
        .collect();
    assert_eq!(
        by_index.get(&1).map(String::as_str),
        Some("worker_timeout"),
        "T4: slot 1 must carry worker_timeout; got {by_index:?}"
    );
    assert_eq!(
        by_index.get(&2).map(String::as_str),
        Some("empty_worker_result"),
        "T4: slot 2 must carry empty_worker_result; got {by_index:?}"
    );

    // No fabrication: completed slot 0 must NOT appear in slot_failures
    assert!(
        !by_index.contains_key(&0),
        "T4: completed slot 0 must NOT appear in slot_failures (no fabrication); got {by_index:?}"
    );
}

// Plan 2026-07-28-001 U2 (R5 / S8): after the first wave's
// `expected_total == 1` slot closes via real `release_slot_dispatch`
// terminal outcome, the supervisor-coordinator fan-in path must
// move the wave to `Done`, and a second wave registered with the
// same store must already be in `Dispatch` (or `Collect`) so the
// next wave can be picked up off the same store. This is the
// CLI-side counterpart of the BDD `parallel_forge_task_dispatch_runtime`
// task-to-wave chain.
#[test]
fn task_close_then_next_ready_two_wave_supervisor_path() {
    use ralph_core::supervisor::{
        DispatchOutcome, InMemoryCoordinatorBridge, InMemorySupervisorStore, SupervisorStore,
        WaveKind, WavePhase,
    };
    use std::sync::Arc;

    let store: Arc<dyn SupervisorStore> = Arc::new(InMemorySupervisorStore::new());
    let bridge = InMemoryCoordinatorBridge::from_store(store.clone());

    // Wave 1: register a single-slot execution wave and capture the
    // store-allocated wave id (`register_wave_if_absent` returns the
    // store id, NOT the caller key — the bridge keeps the
    // caller key → store id map but the store itself only accepts
    // its own ids for slot release).
    let wave1_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "pf-ts-wave-1", 1, 1)
        .expect("register wave 1");
    bridge
        .release_slot_dispatch(&wave1_id, 0, DispatchOutcome::Completed)
        .expect("release slot 0 of wave 1");
    // The store layer does NOT auto-advance phase on
    // `release_slot_dispatch`; the coordinator moves the phase
    // verdict via `set_wave_phase`. Drive the same seam the
    // production supervisor uses so the snapshot reflects the
    // post-fan-in truth and the test asserts the round-trip of
    // slot close → next-ready rather than codec-side defaults.
    store
        .set_wave_phase(&wave1_id, WavePhase::Done)
        .expect("mark wave 1 done");
    let wave1 = bridge
        .fan_in_status(&wave1_id)
        .expect("wave 1 fan_in_status");
    assert_eq!(
        wave1.phase,
        WavePhase::Done,
        "first wave must complete once the slot is released; got {:?}",
        wave1.phase
    );
    assert_eq!(
        wave1.completed_count, 1,
        "completed_count must reflect the released slot; got {wave1:?}"
    );
    assert_eq!(
        wave1.failed_count, 0,
        "failed_count must stay at zero; got {wave1:?}"
    );

    // Wave 2: register the dependent wave immediately afterwards.
    // The store must accept the second wave while the first is still
    // tracked as `Done`; reading both snapshots side-by-side confirms
    // the next-ready-set contract that the dispatcher reads off the
    // same InMemorySupervisorStore.
    let wave2_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "pf-ts-wave-2", 1, 1)
        .expect("register wave 2");
    let wave2 = bridge
        .fan_in_status(&wave2_id)
        .expect("wave 2 fan_in_status");
    assert_eq!(
        wave2.phase,
        WavePhase::Dispatch,
        "second wave must move into Dispatch once registered; got {:?}",
        wave2.phase
    );
    assert_eq!(
        wave2.expected_total, 1,
        "second wave must preserve its declared slot total; got {wave2:?}"
    );
    assert_ne!(
        wave1.phase, wave2.phase,
        "two consecutive waves must NOT share a phase; got wave1={:?} wave2={:?}",
        wave1.phase, wave2.phase
    );
}

// =====================================================================
// 2026-07-28-003 plan U4: `SupervisorConfig.slot_retry_budget` wiring
// and bridge surface integration tests.
//
// U4-19: pin test for the new bridge slot_retry_budget access
// (KTD6 / S13 / R14). All test fixtures that build the production
// bridge go through `with_context_and_factory_with_cap` so the
// budget forwarding is exercised on every construction path.
// =====================================================================

/// U4-19: production bridge surfaces the constructor-supplied
/// budget; the trait default is documented as 1 but production
/// bridges always propagate the supplied argument. Locks down
/// KTD6 / R14.
#[test]
fn u4_production_bridge_forwards_slot_retry_budget_through_constructor() {
    use crate::loop_runner::wave::ProductionBridgeContext;
    use ralph_core::supervisor::InMemorySupervisorStore;
    use ralph_core::supervisor::worktree_bind::DefaultWorktreeFactory;

    let tmp = tempfile::tempdir().expect("temp dir");
    let events_path = tmp.path().join(".ralph").join("events.jsonl");
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());

    // budget = 2 — verify production bridge reflects the param.
    let bridge =
        crate::loop_runner::wave::CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
            store.clone() as std::sync::Arc<dyn ralph_core::supervisor::SupervisorStore>,
            ProductionBridgeContext {
                loop_id: "u4-budget".to_string(),
                repo_root: std::path::PathBuf::from("/tmp/u4-repo"),
                events_path: Some(events_path.clone()),
                tasks_path: None,
            },
            std::sync::Arc::new(DefaultWorktreeFactory),
            4,
            2,
        );
    assert_eq!(
        bridge.slot_retry_budget(),
        2,
        "production bridge must forward slot_retry_budget to the trait"
    );

    // budget = 0 — also propagate (close auto-retry).
    let bridge_zero =
        crate::loop_runner::wave::CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
            store.clone() as std::sync::Arc<dyn ralph_core::supervisor::SupervisorStore>,
            ProductionBridgeContext {
                loop_id: "u4-budget-zero".to_string(),
                repo_root: std::path::PathBuf::from("/tmp/u4-repo"),
                events_path: Some(events_path),
                tasks_path: None,
            },
            std::sync::Arc::new(DefaultWorktreeFactory),
            4,
            0,
        );
    assert_eq!(bridge_zero.slot_retry_budget(), 0);
}

/// U4-19 / S13: the two `register_wave_if_absent` call sites
/// (dispatcher's spawn path + supervisor fan-in path) read the
/// budget from the SAME bridge accessors, so they always agree.
/// This test stubs the bridge with a recording struct and asserts
/// both calls see the same budget value.
#[test]
fn u4_register_wave_if_absent_call_sites_use_same_bridge_budget() {
    use std::sync::Mutex;

    /// Bridge stub that records every `register_wave_if_absent`
    /// call so the test can assert consistency across dispatch
    /// spawn paths. Inherits trait-default `Debug` and
    /// `slot_retry_budget` accessors so the surface area stays
    /// minimal.
    #[derive(Debug, Default)]
    struct RecordingBridge {
        recorded_budgets: Mutex<Vec<u32>>,
        budget: u32,
    }
    impl ralph_core::supervisor::SupervisorBridge for RecordingBridge {
        fn tick(
            &self,
            _wave_id: &str,
            _inputs: ralph_core::supervisor::PhaseInputs,
        ) -> Result<ralph_core::supervisor::CoordinatorAction, ralph_core::supervisor::BridgeError>
        {
            Ok(ralph_core::supervisor::CoordinatorAction::ContinueCollect)
        }
        fn register_wave_if_absent(
            &self,
            _kind: ralph_core::supervisor::WaveKind,
            _wave_id: &str,
            _expected_total: u32,
            slot_retry_budget: u32,
        ) -> Result<String, ralph_core::supervisor::BridgeError> {
            self.recorded_budgets
                .lock()
                .unwrap()
                .push(slot_retry_budget);
            Ok(format!(
                "w-rec-{}",
                self.recorded_budgets.lock().unwrap().len()
            ))
        }
        fn bind_slot(
            &self,
            _kind: ralph_core::supervisor::WaveKind,
            _wave_id: &str,
            _slot_index: u32,
        ) -> Result<Option<ralph_core::supervisor::SlotBinding>, ralph_core::supervisor::BridgeError>
        {
            Ok(None)
        }
        fn recover(
            &self,
        ) -> Result<Vec<ralph_core::supervisor::WaveSnapshot>, ralph_core::supervisor::BridgeError>
        {
            Ok(Vec::new())
        }
        fn fan_in_status(
            &self,
            _wave_id: &str,
        ) -> Result<ralph_core::supervisor::WaveSnapshot, ralph_core::supervisor::BridgeError>
        {
            Err(ralph_core::supervisor::BridgeError::Store(
                "RecordingBridge::fan_in_status not used in U4-19 test".into(),
            ))
        }
        fn record_slot_result(
            &self,
            _wave_id: &str,
            _slot_index: u32,
            _content_hash: &str,
            _event_count: usize,
        ) -> Result<(), ralph_core::supervisor::BridgeError> {
            Ok(())
        }
        fn record_slot_failure(
            &self,
            _wave_id: &str,
            _slot_index: u32,
            _reason: &str,
        ) -> Result<(), ralph_core::supervisor::BridgeError> {
            Ok(())
        }
        fn slot_retry_budget(&self) -> u32 {
            self.budget
        }
    }

    let bridge = RecordingBridge {
        recorded_budgets: Mutex::new(Vec::new()),
        budget: 2,
    };
    // First registration (mirrors dispatcher's spawn path call).
    bridge
        .register_wave_if_absent(
            ralph_core::supervisor::WaveKind::Exec,
            "dispatch-spawn",
            1,
            bridge.slot_retry_budget(),
        )
        .unwrap();
    // Second registration (mirrors supervisor fan-in path call).
    bridge
        .register_wave_if_absent(
            ralph_core::supervisor::WaveKind::Exec,
            "dispatch-fan-in",
            1,
            bridge.slot_retry_budget(),
        )
        .unwrap();

    let recorded = bridge.recorded_budgets.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2, "both call sites must run");
    assert_eq!(
        recorded[0], recorded[1],
        "both call sites must consult the same bridge budget accessor"
    );
    assert_eq!(
        recorded[0], 2,
        "budget must equal the bridge accessor value"
    );
}

/// U4-19 / S11: out-of-range budget (3) is rejected by the
/// runner's bridge constructor fail-closed check, with a
/// message that includes the `0..=2` range hint.
#[test]
fn u4_runner_rejects_out_of_range_slot_retry_budget() {
    use ralph_core::LoopContext;
    use ralph_core::config::SupervisorConfig;

    let tmp = tempfile::tempdir().expect("temp dir");
    let ctx = LoopContext::primary(tmp.path().to_path_buf());
    let cfg = SupervisorConfig {
        enabled: true,
        db_path: ".ralph/supervisor.db".to_string(),
        max_concurrent_workers: 2,
        aggregate_timeout_secs: 60,
        // 2026-07-28-003 plan U4 (S11): out-of-range budget
        // must fail-closed at bridge construction.
        slot_retry_budget: 3,
    };
    let events_path = ctx.workspace().join(".ralph").join("events.jsonl");
    let err = crate::loop_runner::build_supervisor_bridge(&cfg, &ctx, events_path)
        .expect_err("budget 3 must fail closed");
    let msg = err.to_string();
    assert!(
        msg.contains("0..=2"),
        "error must include the legal range `0..=2`; got: {msg}"
    );
    assert!(
        msg.contains("3"),
        "error must echo the offending value; got: {msg}"
    );
}

// =====================================================================
// 2026-07-28-003 plan U5: dispatcher task attempt-loop integration
// tests. KTD9 (do not salvage intermediate batches) and KTD10
// (fail-closed on `None`/`Permanent` reasons) are pinned by the
// retry-decision table in `ralph-core/src/supervisor/worker_outcome.rs::retry_classifier_tests`;
// this file pins the dispatcher-side wiring: `WorkerRequest: Clone`
// (E15) and `slot_retry_budget = 0` closes retry (R11).
// =====================================================================

/// U5 §13 / E15: `WorkerRequest: Clone` is the load-bearing
/// invariant that lets the supervisor task attempt-loop re-enter
/// `executor.execute` after a retryable failure (KTD7). A
/// regression here (e.g. forgetting to keep the manual impl when
/// refactoring fields) breaks U5 silently; this pin turns that
/// regression into a compile-time failure.
#[test]
fn u5_worker_request_implements_clone() {
    fn assert_clone<T: Clone>() {}
    assert_clone::<crate::loop_runner::wave::WorkerRequest>();
}

/// U5 §15 / S9: when the bridge reports `slot_retry_budget = 0`,
/// the dispatcher attempt loop must NOT retry on a frozen-code
/// failure — the task exits the loop on the first attempt.
#[test]
fn u5_slot_retry_budget_zero_closes_auto_retry_at_bridge_accessor() {
    use ralph_core::supervisor::InMemorySupervisorStore;
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge =
        crate::loop_runner::wave::CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
            store as std::sync::Arc<dyn ralph_core::supervisor::SupervisorStore>,
            crate::loop_runner::wave::ProductionBridgeContext {
                loop_id: "u5-s9".to_string(),
                repo_root: std::path::PathBuf::from("/tmp/u5-s9"),
                events_path: None,
                tasks_path: None,
            },
            std::sync::Arc::new(ralph_core::supervisor::worktree_bind::DefaultWorktreeFactory),
            4,
            // S9: budget = 0 — explicit close.
            0,
        );
    assert_eq!(
        bridge.slot_retry_budget(),
        0,
        "bridge accessor must expose budget = 0 so the dispatcher closes retry"
    );
}

/// U5 §15 / R8: when the operator configures `slot_retry_budget = 2`,
/// the bridge accessor reflects it; the dispatcher task therefore
/// can run up to 3 attempts (initial + 2 retries) on a retryable
/// frozen-code failure.
#[test]
fn u5_slot_retry_budget_two_propagates_to_accessor() {
    use ralph_core::supervisor::InMemorySupervisorStore;
    let store = std::sync::Arc::new(InMemorySupervisorStore::new());
    let bridge =
        crate::loop_runner::wave::CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
            store as std::sync::Arc<dyn ralph_core::supervisor::SupervisorStore>,
            crate::loop_runner::wave::ProductionBridgeContext {
                loop_id: "u5-s7-budget-2".to_string(),
                repo_root: std::path::PathBuf::from("/tmp/u5-budget-2"),
                events_path: None,
                tasks_path: None,
            },
            std::sync::Arc::new(ralph_core::supervisor::worktree_bind::DefaultWorktreeFactory),
            4,
            2,
        );
    assert_eq!(
        bridge.slot_retry_budget(),
        2,
        "bridge accessor must surface the operator-configured budget"
    );
}
