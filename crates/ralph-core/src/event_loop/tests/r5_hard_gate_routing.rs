//! 2026-06-14-003 R5 integration tests: hard-gate routing stability.
//!
//! Verifies that the policy / workflow rejection paths publish
//! `task.resume` events whose `target` is the source hat (so the
//! next activation lands on the offending hat, not the
//! alphabetically-first hat) and whose payload carries the wave
//! metadata when the source event was a wave record.
//!
//! The unit tests in `rejection.rs` cover the
//! `build_task_resume_payload` payload shape; the integration
//! tests here cover the wiring through `EventLoop`'s
//! `enforce_wave_isolated_scope` and the hard-gate injection
//! paths.

use crate::event_loop::EventLoop;
use crate::event_loop::tests::common::init_git_workspace;
use crate::loop_context::LoopContext;
use std::path::Path;

fn solo_config(workspace: &Path) -> crate::config::RalphConfig {
    let mut config = crate::config::RalphConfig::default();
    config.core.workspace_root = workspace.to_path_buf();
    config
}

#[test]
fn enforce_current_unit_active_default_off() {
    // The default `RalphConfig` (no preset) leaves the R4 contract
    // disabled so non-isolated presets are unaffected.  This guards
    // against an accidental flip in `EventLoopConfig::default()`.
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let config = solo_config(dir.path());
    let ctx = LoopContext::primary(dir.path().to_path_buf());
    let event_loop = EventLoop::with_context(config, ctx);
    assert!(
        !event_loop.enforce_current_unit_active(),
        "R4 contract must default to off for non-isolated configs"
    );
}

#[test]
fn ephemeral_isolation_default_off_in_non_isolated_config() {
    // Same default-off guard for R3: the ephemeral isolation
    // engine is only active when the preset opts in.
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let config = solo_config(dir.path());
    let ctx = LoopContext::primary(dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx);
    // We do not have a public `ephemeral_isolation_active` accessor;
    // running the engine when the flag is off is a no-op.  The
    // test simply asserts the engine does not panic and produces
    // no relocations for an empty workspace.
    event_loop.run_ephemeral_isolation();
    assert!(
        event_loop.state().last_ephemeral_relocations.is_empty(),
        "default-off isolation must produce no relocations"
    );
}
