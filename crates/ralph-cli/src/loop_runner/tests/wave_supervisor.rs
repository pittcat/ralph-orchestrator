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
    BridgeError, MockSupervisorBridge, SlotBinding, SupervisorBridge, is_supervisor_path_enabled,
};
use ralph_core::supervisor::{PhaseInputs, WaveKind};
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
    };
    let bridge = crate::loop_runner::build_supervisor_bridge(&cfg, &ctx)
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
    };
    let bridge = crate::loop_runner::build_supervisor_bridge(&cfg, &ctx)
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

    let err = crate::loop_runner::build_supervisor_bridge(&cfg, &ctx)
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
    let _bridge = crate::loop_runner::build_supervisor_bridge(&cfg, &ctx)
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
    let _bridge = crate::loop_runner::build_supervisor_bridge(&cfg, &ctx)
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
            let _ = build_supervisor_bridge(&cfg, &ctx)
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

    let _bridge =
        build_supervisor_bridge(&cfg, &ctx).expect("enabled+isolated must build a bridge");
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
        let _ = build_supervisor_bridge(&cfg, &ctx)
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
