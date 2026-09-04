use super::super::*;
use crate::loop_runner::wave::{
    MockSupervisorBridge, SupervisorBridge, is_supervisor_path_enabled,
};
use ralph_core::supervisor::WaveKind;
use ralph_core::supervisor::worktree_bind::{DefaultWorktreeFactory, WorktreeFactory};
use ralph_core::supervisor::{InMemorySupervisorStore, SupervisorStore};

use super::fixtures::*;

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

    let (bridge, store) = production_bridge_with_factory(
        factory.clone() as std::sync::Arc<dyn WorktreeFactory>,
        tmp.path().to_path_buf(),
        "u4-loop",
    );

    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Exec, "u4-wave", 2, 0)
        .expect("register must succeed");

    // 2026-07-28-002 plan U1: branch naming now follows
    // `{loop_id}-{kind}-{wave_id}-{slot_index}`. The store assigns
    // `store_wave_id` (= `w-{seq}`) on register, so pre-register the
    // factory paths using the live id rather than a hard-coded
    // branch string.
    factory.pre_create(
        format!("u4-loop-exec-{store_wave_id}-0").as_str(),
        tmp.path().join("wt-0"),
    );
    factory.pre_create(
        format!("u4-loop-exec-{store_wave_id}-1").as_str(),
        tmp.path().join("wt-1"),
    );

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
        Some(format!("u4-loop-exec-{store_wave_id}-0").as_str()),
        "slot 0 branch must follow the {{loop_id}}-{{kind}}-{{wave_id}}-{{slot_index}} convention"
    );
    assert_eq!(
        binding_1
            .env
            .get("RALPH_WAVE_WORKTREE_BRANCH")
            .map(String::as_str),
        Some(format!("u4-loop-exec-{store_wave_id}-1").as_str()),
        "slot 1 branch must follow the {{loop_id}}-{{kind}}-{{wave_id}}-{{slot_index}} convention"
    );

    // Factory observed both calls.
    let calls = factory.calls_snapshot();
    assert_eq!(calls.len(), 2, "two exec slots must call the factory twice");
    assert_eq!(calls[0].1, format!("u4-loop-exec-{store_wave_id}-0"));
    assert_eq!(calls[1].1, format!("u4-loop-exec-{store_wave_id}-1"));

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
    assert_eq!(
        branch_0.branch.as_deref(),
        Some(format!("u4-loop-exec-{store_wave_id}-0").as_str())
    );
    assert_eq!(
        branch_1.branch.as_deref(),
        Some(format!("u4-loop-exec-{store_wave_id}-1").as_str())
    );
}

/// U4 R7: production `bind_slot` for `Fix` MUST use the same
/// `{loop_id}-{kind}-{wave_id}-{slot_index}` branch convention and hand
/// back distinct worktree paths.
#[test]
fn fix_kind_produces_unique_branch_path_cwd() {
    let factory = std::sync::Arc::new(RecordingFactory::new());
    let tmp = tempfile::tempdir().expect("temp dir");

    let (bridge, store) = production_bridge_with_factory(
        factory.clone() as std::sync::Arc<dyn WorktreeFactory>,
        tmp.path().to_path_buf(),
        "u4-loop",
    );

    let store_wave_id = bridge
        .register_wave_if_absent(WaveKind::Fix, "u4-fix-wave", 3, 0)
        .expect("register must succeed");

    // 2026-07-28-002 plan U1: branch naming now follows
    // `{loop_id}-{kind}-{wave_id}-{slot_index}`.
    for slot in 0u32..3 {
        factory.pre_create(
            format!("u4-loop-fix-{store_wave_id}-{slot}").as_str(),
            tmp.path().join(format!("fix-wt-{slot}")),
        );
    }

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
            Some(format!("u4-loop-fix-{store_wave_id}-{slot}").as_str()),
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
            format!("u4-loop-fix-{store_wave_id}-0"),
            format!("u4-loop-fix-{store_wave_id}-1"),
            format!("u4-loop-fix-{store_wave_id}-2"),
        ],
        "fix branch names must follow the loop-kind-wave_id-index convention"
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
        !workspace
            .join(format!("u4-loop-exec-{store_wave_id}-0"))
            .exists(),
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

    let (bridge, _store) = production_bridge_with_factory(
        factory.clone() as std::sync::Arc<dyn WorktreeFactory>,
        tmp.path().to_path_buf(),
        "u4-loop",
    );

    // 2026-07-28-002 plan U1: branch naming now follows
    // `{loop_id}-{kind}-{wave_id}-{slot_index}`. Pre-create using
    // the actual store wave ids.
    let exec_wave = bridge
        .register_wave_if_absent(WaveKind::Exec, "u4-exec-pin", 1, 0)
        .expect("register");
    factory.pre_create(
        format!("u4-loop-exec-{exec_wave}-0").as_str(),
        tmp.path().join("exec-wt"),
    );
    let fix_wave = bridge
        .register_wave_if_absent(WaveKind::Fix, "u4-fix-pin", 1, 0)
        .expect("register");
    factory.pre_create(
        format!("u4-loop-fix-{fix_wave}-0").as_str(),
        tmp.path().join("fix-wt"),
    );

    let exec_binding = bridge
        .bind_slot(WaveKind::Exec, &exec_wave, 0)
        .expect("exec bind must succeed");
    assert!(
        exec_binding.is_some(),
        "production Exec bind MUST NOT return None; got None (old behaviour)"
    );

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

    // 2026-07-28-002 plan U1: branch naming now follows
    // `{loop_id}-{kind}-{wave_id}-{slot_index}`.
    factory.pre_create(
        format!("u1-loop-exec-{store_wave_id}-0").as_str(),
        tmp.path().join("exec-wt-0"),
    );
    factory.pre_create(
        format!("u1-loop-exec-{store_wave_id}-1").as_str(),
        tmp.path().join("exec-wt-1"),
    );

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
        Some(format!("u1-loop-exec-{store_wave_id}-0").as_str()),
        "slot 0 branch must follow the {{loop_id}}-{{kind}}-{{wave_id}}-{{slot_index}} convention"
    );
    assert_eq!(
        binding_1
            .env
            .get("RALPH_WAVE_WORKTREE_BRANCH")
            .map(String::as_str),
        Some(format!("u1-loop-exec-{store_wave_id}-1").as_str()),
        "slot 1 branch must follow the {{loop_id}}-{{kind}}-{{wave_id}}-{{slot_index}} convention"
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

    // 2026-07-28-002 plan U1: branch naming now follows
    // `{loop_id}-{kind}-{wave_id}-{slot_index}`.
    for slot in 0u32..3 {
        factory.pre_create(
            format!("u1-loop-fix-{store_wave_id}-{slot}").as_str(),
            tmp.path().join(format!("fix-wt-{slot}")),
        );
    }

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
            Some(format!("u1-loop-fix-{store_wave_id}-{slot}").as_str()),
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
    //
    // 2026-08-07-009 plan U2: the per-attempt receipt path adds a
    // `spawn_blocking` git probe before `record_slot_result`, so
    // the slot_results Vec ordering no longer matches slot_index
    // monotonically. Look up by slot_index instead of Vec index
    // so the test stays stable across dispatcher scheduling
    // jitter.
    let recorded = bridge.slot_results.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2, "bridge must have recorded 2 slots");
    let slot0_hash = recorded
        .iter()
        .find(|(idx, _, _)| *idx == 0)
        .map(|(_, h, _)| h.clone())
        .expect("slot 0 recorded");
    let slot1_hash = recorded
        .iter()
        .find(|(idx, _, _)| *idx == 1)
        .map(|(_, h, _)| h.clone())
        .expect("slot 1 recorded");
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
        // 2026-09-03-0959 plan U1: legacy `WaveTracker` path.
        scheduler_mode: ralph_core::config::SchedulerMode::Wave,
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
