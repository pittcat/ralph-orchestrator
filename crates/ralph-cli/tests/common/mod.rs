//! Shared helpers for ralph-cli integration tests.
//!
//! When a hat runs `cargo nextest` / `./scripts/run-tests.sh` under
//! `ralph run`, the process inherits agent-context env vars from
//! `inject_hat_execution_env`. Tests that spawn the `ralph` binary and
//! assume human-CLI semantics must scrub those keys first; otherwise
//! ACL / emit allowlist / skill visibility checks treat the fixture as
//! an in-loop agent and fail.

use std::process::Command;

/// Keys that mark agent-owned CLI context (plus hat-execution overlays).
///
/// `RALPH_WORKSPACE_ROOT` and `RALPH_LOOP_ITERATION` are loop-runtime
/// keys: `RALPH_WORKSPACE_ROOT` would otherwise let CwdGuard resolve
/// back to the real repo root when a test sets `current_dir(temp_path)`
/// (see mem-1784744041-fd32), so a `ralph emit` writes to the active
/// loop's events file instead of the test's temp one and the test's
/// read returns empty. `RALPH_LOOP_ITERATION` is not consulted by the
/// spawned CLI but is stripped for consistency so the spawned process
/// sees a uniform "no-loop" environment.
pub const AGENT_CONTEXT_ENV_KEYS: &[&str] = &[
    "RALPH_CURRENT_HAT",
    "RALPH_CURRENT_LOOP_ID",
    "RALPH_EVENTS_FILE",
    "RALPH_WAVE_WORKER",
    "RALPH_TRIGGERED_HAT",
    "RALPH_HATS_SOURCE",
    "RALPH_CONFIG",
    "RALPH_WORKSPACE_ROOT",
    "RALPH_CURRENT_BRANCH",
    "RALPH_LOOP_ITERATION",
];

/// Drop agent-context env so the spawned `ralph` sees a human CLI.
///
/// Call this before any intentional `.env(...)` overlays that simulate
/// a specific agent scenario.
pub fn scrub_agent_runtime_env(cmd: &mut Command) {
    for key in AGENT_CONTEXT_ENV_KEYS {
        cmd.env_remove(*key);
    }
}

/// `ralph` binary command with agent-context env scrubbed.
pub fn ralph_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ralph"));
    scrub_agent_runtime_env(&mut cmd);
    cmd
}

/// Create a tempdir prefixed with `ralph-verify-ws-` for tests that
/// spawn the `ralph` binary. The returned `tempfile::TempDir` keeps the
/// directory alive until dropped — most tests should bind it to a
/// variable in their test body so the cleanup runs on test exit.
#[allow(dead_code)] // Not all integration test binaries use this helper.
pub fn make_scenario_workspace() -> std::io::Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix("ralph-verify-ws-")
        .tempdir()
}
