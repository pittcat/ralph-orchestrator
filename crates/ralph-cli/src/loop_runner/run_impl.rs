//! Supervisor bridge construction (run_impl helpers).
//!
//! This module owns the production `CoordinatorSupervisorBridge`
//! builder plus the test-only counter and factory-override seam
//! that the bridge uses. It is the home of:
//!
//! - [`BRIDGE_BUILD_INVOCATIONS`] + [`bridge_build_invocations`]
//!   (counter test seam; U1 characterization in
//!   `loop_runner::tests::wave_supervisor`).
//! - [`WORKTREE_FACTORY_OVERRIDE`] + [`install_factory_override_for_test`]
//!   + [`clear_factory_override_for_test`] (test-only factory seam;
//!   2026-07-23-001 plan U1).
//! - [`build_supervisor_bridge`] (production bridge builder).
//! - [`resolve_supervisor_db_path`] (helper).

/// 2026-07-22-003 plan U1: a strictly-additive counter incremented
/// each time `build_supervisor_bridge` enters. The counter exists
/// solely so U1 characterization tests in
/// `loop_runner::tests::wave_supervisor` can read it and assert that
/// the production gate (`is_supervisor_path_enabled`) keeps the
/// bridge builder un-invoked for `ce-executor-pipeline` (and any
/// other preset that does not opt into `supervisor.enabled`).
///
/// This counter is a read-only test seam; it never alters the
/// behaviour of `build_supervisor_bridge` or the runner that calls
/// it. U2+ keep the counter in place so subsequent units can reuse
/// it for their own gate assertions.
pub(crate) static BRIDGE_BUILD_INVOCATIONS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// 2026-07-22-003 plan U1: read the bridge builder counter so tests
/// can take before/after snapshots and assert the production gate
/// does not call `build_supervisor_bridge` when `supervisor.enabled`
/// is `false`. See `BRIDGE_BUILD_INVOCATIONS` for the rationale.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn bridge_build_invocations() -> u64 {
    BRIDGE_BUILD_INVOCATIONS.load(std::sync::atomic::Ordering::SeqCst)
}

/// 2026-07-23-001 plan U1: factory override seam for tests that
/// exercise the production `build_supervisor_bridge` path
/// without invoking `git worktree add`. Tests install a
/// `RecordingFactory` (or `FailingFactory`) and verify
/// `bind_slot(Exec|Fix)` returns `Some(SlotBinding)` (or
/// `Err` on factory failure). Production code never installs
/// anything — the override stays `None` and the bridge uses
/// `DefaultWorktreeFactory` (which calls
/// `worktree::create_worktree`).
#[cfg(feature = "supervisor-db")]
#[cfg(test)]
pub(crate) static WORKTREE_FACTORY_OVERRIDE: std::sync::Mutex<
    Option<std::sync::Arc<dyn ralph_core::supervisor::worktree_bind::WorktreeFactory>>,
> = std::sync::Mutex::new(None);

/// 2026-07-23-001 plan U1: install a factory override so the
/// production `build_supervisor_bridge` path uses the supplied
/// factory instead of `DefaultWorktreeFactory`. See
/// `WORKTREE_FACTORY_OVERRIDE` for the rationale.
#[cfg(feature = "supervisor-db")]
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn install_factory_override_for_test(
    factory: std::sync::Arc<dyn ralph_core::supervisor::worktree_bind::WorktreeFactory>,
) {
    *WORKTREE_FACTORY_OVERRIDE.lock().unwrap() = Some(factory);
}

/// 2026-07-23-001 plan U1: clear the factory override installed
/// by `install_factory_override_for_test`. Production code does
/// not call this — tests use it to clean up between assertions.
#[cfg(feature = "supervisor-db")]
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn clear_factory_override_for_test() {
    *WORKTREE_FACTORY_OVERRIDE.lock().unwrap() = None;
}

/// Build the production `CoordinatorSupervisorBridge` from `SupervisorConfig`.
///
/// Resolves `db_path` relative to the loop workspace when it is not
/// absolute; absolute paths are honoured as-is. Without the
/// `supervisor-db` cargo feature this returns an error so a
/// `supervisor.enabled: true` preset fails fast at bridge
/// construction rather than silently losing wave state across
/// restarts.
///
/// 2026-07-23-001 plan U1: the bridge is constructed via
/// `CoordinatorSupervisorBridge::with_context_and_factory` so
/// `bind_slot(Exec|Fix)` hands back `Some(SlotBinding)` against
/// per-slot worktrees (U5 / R5 / R7 / KTD-3). The legacy
/// `from_store` path left `context: None` and made every
/// `bind_slot` return `Ok(None)`, so the dispatcher spawned
/// Exec/Fix workers against the main workspace — the silent
/// failure mode U1 closes. `loop_id` resolves from
/// `ctx.loop_id()`; primary loops fall back to `"primary"`.
/// The factory is `DefaultWorktreeFactory` (which calls
/// `worktree::create_worktree`); tests inject a recording/
/// failing factory via `install_factory_override_for_test`.
///
/// Errors surface as `anyhow` so the caller can fail-closed
/// (R-C4) without leaking `SupervisorStoreError` across module
/// boundaries.
///
/// The counter increment is a read-only test seam kept for the
/// gate assertions in `loop_runner::tests::wave_supervisor`; it
/// does not alter the build path.
pub(crate) fn build_supervisor_bridge(
    cfg: &ralph_core::config::SupervisorConfig,
    ctx: &ralph_core::LoopContext,
    events_path: std::path::PathBuf,
) -> std::result::Result<crate::loop_runner::wave::CoordinatorSupervisorBridge, anyhow::Error> {
    BRIDGE_BUILD_INVOCATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    #[cfg(feature = "supervisor-db")]
    let resolved_db_path = resolve_supervisor_db_path(cfg, ctx);
    #[cfg(not(feature = "supervisor-db"))]
    let _ = resolve_supervisor_db_path(cfg, ctx);

    #[cfg(not(feature = "supervisor-db"))]
    {
        return Err(anyhow::anyhow!(
            "supervisor-db cargo feature is off in this build; \
             rebuild ralph-cli with --features supervisor-db (or rely on \
             the default features) to enable event_loop.supervisor.enabled: true"
        ));
    }

    #[cfg(feature = "supervisor-db")]
    {
        use crate::loop_runner::wave::{CoordinatorSupervisorBridge, ProductionBridgeContext};
        use ralph_core::supervisor::worktree_bind::DefaultWorktreeFactory;

        // 2026-07-28-003 plan U4 (KTD10 / S11): the slot retry
        // budget MUST be in `0..=2`. Out-of-range values fail
        // closed here, before the bridge exists, so an operator
        // who typed `3` (or `99`) gets a startup error pointing
        // at the field rather than a silent runtime invariant
        // violation. The store layer's `register_wave` check
        // (`memory.rs:313-315`) remains as a defensive second
        // gate.
        if cfg.slot_retry_budget > 2 {
            anyhow::bail!(
                "supervisor.slot_retry_budget must be in 0..=2; got {}. Update preset {}",
                cfg.slot_retry_budget,
                "<event_loop.supervisor.slot_retry_budget>",
            );
        }

        if let Some(parent) = resolved_db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let store = ralph_core::supervisor::RusqliteSupervisorStore::open(&resolved_db_path)
            .map_err(|err| {
                anyhow::anyhow!(
                    "failed to open supervisor db at {}: {err}",
                    resolved_db_path.display()
                )
            })?;
        let store: std::sync::Arc<dyn ralph_core::supervisor::SupervisorStore> =
            std::sync::Arc::new(store);

        // U1: derive the ProductionBridgeContext from the runtime
        // `LoopContext`. `ctx.loop_id()` is `Option<&str>` —
        // primary loops return `None`; fall back to `"primary"`
        // so the branch naming still produces a stable, unique
        // per-slot key. `ctx.repo_root()` is the absolute repo
        // root that per-slot worktrees branch off.
        let loop_id = ctx.loop_id().unwrap_or("primary").to_string();
        let repo_root = ctx.repo_root().to_path_buf();
        // U6: hand the coordinator the loop's main ledger path so
        // the production `FileEventMergeSink` writes the fan-in
        // business events to the same `events.jsonl` the
        // dispatcher merges into (KTD-6).
        // U4 (2026-07-23-007): hand the bridge the loop's
        // `tasks.jsonl` path so the supervisor path can project
        // slot transitions onto the runtime task ledger. The path
        // is derived from the events file's parent directory
        // (canonical `.ralph/agent/tasks.jsonl`).
        let tasks_path = events_path
            .parent()
            .map(|p| p.join("agent").join("tasks.jsonl"))
            .unwrap_or_else(|| std::path::PathBuf::from(".ralph/agent/tasks.jsonl"));
        let context = ProductionBridgeContext {
            loop_id: loop_id.clone(),
            repo_root,
            events_path: Some(events_path),
            tasks_path: Some(tasks_path),
        };

        // U1: factory resolution — production uses the default
        // (real git worktree); tests inject a recording/failing
        // factory via the `WORKTREE_FACTORY_OVERRIDE` seam so
        // they can assert the production wiring without spawning
        // a real `git worktree add`.
        let factory: std::sync::Arc<dyn ralph_core::supervisor::worktree_bind::WorktreeFactory> = {
            #[cfg(test)]
            {
                if let Some(override_factory) = WORKTREE_FACTORY_OVERRIDE.lock().unwrap().clone() {
                    override_factory
                } else {
                    std::sync::Arc::new(DefaultWorktreeFactory)
                }
            }
            #[cfg(not(test))]
            std::sync::Arc::new(DefaultWorktreeFactory)
        };

        Ok(
            CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
                store,
                context,
                factory,
                cfg.max_concurrent_workers,
                // 2026-07-28-003 plan U4 (KTD6): forward the
                // operator-configured slot retry budget to the
                // bridge so the dispatcher attempt loop and the
                // two `register_wave_if_absent` calls see the
                // exact same value.
                cfg.slot_retry_budget,
            ),
        )
    }
}

/// Resolve the supervisor SQLite store path. Absolute
/// `cfg.db_path` values are honoured as-is; relative values
/// resolve against the loop workspace so the default
/// `SupervisorConfig::db_path` (`.ralph/supervisor.db`) lands at
/// `<workspace>/.ralph/supervisor.db` exactly once — a bare
/// `supervisor.db` and `.ralph/supervisor.db` both produce the
/// same target without a double `.ralph` segment. The caller
/// decides whether to create the parent dir.
fn resolve_supervisor_db_path(
    cfg: &ralph_core::config::SupervisorConfig,
    ctx: &ralph_core::LoopContext,
) -> std::path::PathBuf {
    let db_path = std::path::Path::new(&cfg.db_path);
    if db_path.is_absolute() {
        db_path.to_path_buf()
    } else {
        ctx.workspace().join(db_path)
    }
}
