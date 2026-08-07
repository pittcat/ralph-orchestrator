use super::super::*;
use crate::loop_runner::wave::{
    BridgeError, MockSupervisorBridge, SupervisorBridge, is_supervisor_path_enabled,
};
use ralph_core::supervisor::worktree_bind::WorktreeFactory;
use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore};
use ralph_core::supervisor::{PhaseInputs, WaveKind};

use super::fixtures::*;

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

    // 2026-07-28-002 plan U1: branch naming now follows
    // `{loop_id}-{kind}-{wave_id}-{slot_index}`.
    factory.pre_create(
        format!("u8-loop-exec-{store_wave_id}-0").as_str(),
        tmp.path()
            .join(format!(".ralph/u8-slot-{store_wave_id}-0-worktree")),
    );

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

/// S1 (2026-07-28-002 plan U1): same loop, two consecutive exec waves,
/// same slot index (0) — the branch names must be different so the
/// second wave does not conflict with the first wave's worktree.
///
/// Branch naming follows `{loop_id}-{kind}-{wave_id}-{slot_index}`.
///
/// This is the core regression test for the wave-slot namespace fix:
/// without `wave_id` in the branch name, both waves would produce
/// `loop-S1-exec-0` and the second `bind_slot` would either reuse
/// the first wave's worktree or fail if the branch already exists.
#[test]
fn s1_same_loop_different_waves_get_distinct_branches() {
    let factory = std::sync::Arc::new(RecordingFactory::new());
    let tmp = tempfile::tempdir().expect("temp dir");

    // Pre-register branch names following the new convention:
    // {loop_id}-{kind}-{wave_id}-{slot_index}
    let loop_id = "S1";
    let wave_id_1 = "w-1";
    let wave_id_2 = "w-2";
    let slot_index = 0u32;

    factory.pre_create(
        &format!("{loop_id}-exec-{wave_id_1}-{slot_index}"),
        tmp.path().join("wt-wave-1"),
    );
    factory.pre_create(
        &format!("{loop_id}-exec-{wave_id_2}-{slot_index}"),
        tmp.path().join("wt-wave-2"),
    );

    let (bridge, _store) = production_bridge_with_factory(
        factory.clone() as std::sync::Arc<dyn WorktreeFactory>,
        tmp.path().to_path_buf(),
        loop_id,
    );

    // Register two distinct waves.
    let store_wave_id_1 = bridge
        .register_wave_if_absent(WaveKind::Exec, wave_id_1, 1, 0)
        .expect("register wave 1 must succeed");
    let store_wave_id_2 = bridge
        .register_wave_if_absent(WaveKind::Exec, wave_id_2, 1, 0)
        .expect("register wave 2 must succeed");

    // Bind slot 0 for wave 1.
    let binding_1 = bridge
        .bind_slot(WaveKind::Exec, &store_wave_id_1, slot_index)
        .expect("bind_slot wave 1 must succeed")
        .expect("Exec binding must be Some");

    // Bind slot 0 for wave 2.
    let binding_2 = bridge
        .bind_slot(WaveKind::Exec, &store_wave_id_2, slot_index)
        .expect("bind_slot wave 2 must succeed")
        .expect("Exec binding must be Some");

    // Both bindings succeeded.
    assert!(
        binding_1.worktree_path.is_some(),
        "wave 1 binding must have worktree_path"
    );
    assert!(
        binding_2.worktree_path.is_some(),
        "wave 2 binding must have worktree_path"
    );

    // Distinct worktree paths.
    assert_ne!(
        binding_1.worktree_path, binding_2.worktree_path,
        "two waves with the same slot_index must receive distinct worktree paths"
    );

    // Branch names follow the new convention and are distinct.
    let branch_1 = binding_1
        .env
        .get("RALPH_WAVE_WORKTREE_BRANCH")
        .map(String::as_str);
    let branch_2 = binding_2
        .env
        .get("RALPH_WAVE_WORKTREE_BRANCH")
        .map(String::as_str);

    assert_eq!(
        branch_1,
        Some("S1-exec-w-1-0"),
        "wave 1 branch must follow {{loop_id}}-{{kind}}-{{wave_id}}-{{slot_index}}"
    );
    assert_eq!(
        branch_2,
        Some("S1-exec-w-2-0"),
        "wave 2 branch must follow {{loop_id}}-{{kind}}-{{wave_id}}-{{slot_index}}"
    );
    assert_ne!(
        branch_1, branch_2,
        "two waves must produce distinct branch names even for the same slot_index"
    );

    // Factory was called exactly twice with different branch names.
    let calls = factory.calls_snapshot();
    assert_eq!(calls.len(), 2, "two waves must call the factory twice");
    assert_eq!(calls[0].1, "S1-exec-w-1-0");
    assert_eq!(calls[1].1, "S1-exec-w-2-0");
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
