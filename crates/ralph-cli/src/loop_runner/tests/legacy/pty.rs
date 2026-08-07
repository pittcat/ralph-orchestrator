// Auto-extracted from the legacy loop-runner regression suite. Tests in this
// module remain part of the loop_runner::tests::legacy surface; only the file
// layout changed (mechanical split per plan 2026-08-07-005). Behavior,
// assertions, fixtures, and process environment semantics are unchanged.
//
// The full original `legacy.rs` import set is reproduced verbatim per bucket so
// that every existing test compiles without rewriting call sites. Splits may
// leave some imports unused in a given bucket; this is a mechanical artifact,
// not dead code (the same items remain used by sibling buckets).

#![allow(unused_imports)]

use super::super::super::*;
use super::super::common::*;
use super::super::fake_path::*;
use super::helpers::*;
use crate::test_support::CwdGuard;
use ralph_core::HatRegistry;
use ralph_core::planning_session::{ConversationEntry, ConversationType};
use ralph_proto::{Hat, Topic};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::sync::{Arc, Mutex};

// Test: test_main_pty_watchdog_aligns_with_wave_worker_partial_events_semantics
#[test]
fn test_main_pty_watchdog_aligns_with_wave_worker_partial_events_semantics() {
    // Wave worker invariant: on `timed_out=true`, partial events are
    // preserved and surfaced via `Ok((events, ..))`, not converted into a
    // hard "stop the loop" terminate. See wave/worker.rs:462-484 for the
    // mirrored logic.
    //
    // Main PTY invariant after Unit 3: `convert_termination_type` returns
    // `None`, leaving the runner free to drain partial events through the
    // same JSONL pipeline.
    let main_pty_outcome_termination =
        convert_termination_type(ralph_adapters::TerminationType::IdleTimeout, false);

    assert!(
        main_pty_outcome_termination.is_none(),
        "Main PTY path must mirror the wave worker partial-timeout-visible-events \
         contract: backend watchdog timeout is a backend-call end, NOT a loop \
         terminate. Wave worker returns `Ok((events, ..))` to keep partial \
         events flowing; the main PTY path mirrors that by leaving \
         `outcome.termination = None`. See wave/worker.rs:447-484 and \
         test_execute_wave_keeps_text_partial_timeout_events_visible."
    );
}

// Test: test_execute_pty_autonomous_watchdog_fires_for_ce_executor_worktree_rpc
#[cfg(unix)]
#[tokio::test]
async fn test_execute_pty_autonomous_watchdog_fires_for_ce_executor_worktree_rpc() {
    use crate::cli::Verbosity;
    use ralph_adapters::{OutputFormat as CliOutputFormat, PromptMode};
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;
    use tempfile::TempDir;

    // Spin up a fake shell backend in a temp dir. `sleep 60` mimics a
    // Claude-spawned long-running command: it produces NO stdout and does
    // NOT exit. Without the watchdog fix, the runner would block on this
    // for the full minute and the test would elapse its wallclock budget.
    let temp_dir = TempDir::new().expect("temp dir");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let worker_path = bin_dir.join("fake-claude");
    std::fs::write(&worker_path, "#!/bin/sh\nexec sleep 60\n").expect("write script");
    let mut perms = std::fs::metadata(&worker_path)
        .expect("metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&worker_path, perms).expect("chmod");

    let backend = ralph_adapters::CliBackend {
        command: worker_path.display().to_string(),
        args: vec![],
        prompt_mode: PromptMode::Stdin,
        prompt_flag: None,
        output_format: CliOutputFormat::StreamJson,
        env_vars: vec![],
    };

    // Real `RalphConfig` with the new autonomous watchdog pinned to 1s.
    // The `None` -> per-adapter timeout fallback (default 300s) would make
    // the test slow and unreliable across CI environments, so we override
    // to 1s explicitly. This is the same knob `ralph run
    // --autonomous-idle-timeout 1` would set; the test exercises the same
    // resolver path the CLI uses (see ralph_config.rs::autonomous_idle_timeout_secs).
    let yaml = r#"
core:
  scratchpad: ".ralph/agent/scratchpad.md"
  specs_dir: "./specs"
event_loop:
  starting_event: "work.ready"
  completion_promise: "LOOP_COMPLETE"
  max_iterations: 5
cli:
  backend: claude
  default_mode: autonomous
  autonomous_idle_timeout_secs: 1
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).expect("parse config");

    let (_interrupt_tx, interrupt_rx) = tokio::sync::watch::channel(false);

    // Wallclock budget: 1s watchdog + 4s slack (PTY spawn + kill + cleanup).
    // A regression that re-disables the autonomous watchdog would make
    // `execute_pty` block on `sleep 60` and the outer timeout would fire,
    // which the `expect` below turns into a clear failure with the right
    // diagnostic.
    let wallclock = Duration::from_secs(5);
    let outcome = tokio::time::timeout(
        wallclock,
        execute_pty(
            None, // No pre-built executor → execute_pty constructs one from config
            &backend,
            &config,
            "ignored",
            false, // interactive=false (autonomous / RPC / worktree path)
            interrupt_rx,
            Verbosity::Quiet,
            None, // No TUI lines
            None, // No RPC stdout
            0,    // iteration
            "executor",
            "claude",
        ),
    )
    .await
    .expect(
        "autonomous watchdog must fire well within wallclock budget — otherwise the outer \
         `ralph run` loop would hang forever on a silent, non-exiting backend (R1 / R5 \
         violation). This is the exact regression that motivated plan 2026-06-06-001.",
    )
    .expect("PTY observe must not return an io error");

    // R1 / R5: the autonomous / RPC / worktree path must surface
    // `watchdog_timeout = true` so the runner can log the cause without
    // falsely declaring success.
    assert!(
        outcome.watchdog_timeout,
        "R1 / R5: autonomous / RPC / worktree path MUST set `watchdog_timeout = true` \
         when the backend is killed by inactivity. Got watchdog_timeout=false. A \
         regression that re-disables the autonomous watchdog would let this assertion \
         pass only because the wallclock budget above would have panicked first — but \
         the explicit flag is what the runner actually checks at runner.rs::if outcome.watchdog_timeout."
    );

    // R3 / R7 (Unit 3): watchdog timeout must leave `termination = None` so
    // the runner continues to event parsing / hard-gate fallback. The
    // legacy `Some(TerminationReason::Stopped)` mapping short-circuited
    // the partial-event pipeline; that regression must not return.
    assert!(
        outcome.termination.is_none(),
        "R3 / R7: watchdog timeout must leave termination=None so the runner's \
         `if let Some(reason) = outcome.termination` short-circuit is skipped, \
         letting partial events surface and the missing-event hard gate / fallback \
         take over on the next iteration. The legacy Some(Stopped) mapping would \
         silently drop partial events and is no longer correct."
    );
}

// Test: test_execute_pty_reused_executor_refreshes_autonomous_watchdog_timeout
#[cfg(unix)]
#[tokio::test]
async fn test_execute_pty_reused_executor_refreshes_autonomous_watchdog_timeout() {
    use crate::cli::Verbosity;
    use ralph_adapters::{OutputFormat as CliOutputFormat, PromptMode, PtyConfig, PtyExecutor};
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().expect("temp dir");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let worker_path = bin_dir.join("fake-hat-backend");
    std::fs::write(&worker_path, "#!/bin/sh\nexec sleep 60\n").expect("write script");
    let mut perms = std::fs::metadata(&worker_path)
        .expect("metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&worker_path, perms).expect("chmod");

    let backend = ralph_adapters::CliBackend {
        command: worker_path.display().to_string(),
        args: vec![],
        prompt_mode: PromptMode::Stdin,
        prompt_flag: None,
        output_format: CliOutputFormat::StreamJson,
        env_vars: vec![],
    };

    let yaml = r#"
core:
  scratchpad: ".ralph/agent/scratchpad.md"
  specs_dir: "./specs"
cli:
  backend: claude
  default_mode: autonomous
  autonomous_idle_timeout_secs: 1
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).expect("parse config");

    let pty_config = PtyConfig {
        interactive: false,
        idle_timeout_secs: 0,
        cols: 32768,
        rows: 24,
        workspace_root: temp_dir.path().to_path_buf(),
    };
    let mut executor = PtyExecutor::new(backend.clone(), pty_config);
    let (_interrupt_tx, interrupt_rx) = tokio::sync::watch::channel(false);

    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        execute_pty(
            Some(&mut executor),
            &backend,
            &config,
            "ignored",
            false,
            interrupt_rx,
            Verbosity::Quiet,
            None,
            None,
            0,
            "executor",
            "claude",
        ),
    )
    .await
    .expect(
        "reused PTY executor must refresh its idle timeout from the current backend; \
         otherwise TUI/RPC mode keeps the stale 0 timeout and hangs",
    )
    .expect("PTY observe must not return an io error");

    assert!(
        outcome.watchdog_timeout,
        "reused PTY executor must fire the refreshed autonomous watchdog"
    );
}

// Test: test_adapter_timeout_zero_maps_to_no_cli_timeout
#[test]
fn test_adapter_timeout_zero_maps_to_no_cli_timeout() {
    use std::time::Duration;

    assert!(
        runner::adapter_timeout_duration(0).is_none(),
        "adapter timeout 0 is the disabled sentinel; headless CliExecutor must receive None"
    );
    assert_eq!(
        runner::adapter_timeout_duration(5),
        Some(Duration::from_secs(5)),
        "positive adapter timeout values must still enable the inactivity watchdog"
    );
}

// Test: test_execute_pty_autonomous_watchdog_zero_means_disabled_under_real_runner
#[cfg(unix)]
#[tokio::test]
async fn test_execute_pty_autonomous_watchdog_zero_means_disabled_under_real_runner() {
    use crate::cli::Verbosity;
    use ralph_adapters::{OutputFormat as CliOutputFormat, PromptMode};
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;
    use tempfile::TempDir;

    // Same fake backend as the other test, but emits ONE line of stdout
    // after a delay, then exits cleanly. The watchdog is disabled (0),
    // so the test must run to natural completion, not be killed.
    let temp_dir = TempDir::new().expect("temp dir");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let worker_path = bin_dir.join("fake-claude-quiet");
    std::fs::write(
        &worker_path,
        "#!/bin/sh\necho 'natural completion marker'\nsleep 0.2\nexit 0\n",
    )
    .expect("write script");
    let mut perms = std::fs::metadata(&worker_path)
        .expect("metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&worker_path, perms).expect("chmod");

    let backend = ralph_adapters::CliBackend {
        command: worker_path.display().to_string(),
        args: vec![],
        prompt_mode: PromptMode::Stdin,
        prompt_flag: None,
        output_format: CliOutputFormat::StreamJson,
        env_vars: vec![],
    };

    // `autonomous_idle_timeout_secs: 0` is the explicit-disable sentinel
    // (R8 of the plan). The resolver at
    // `RalphConfig::autonomous_idle_timeout_secs(backend)` must NOT
    // silently swap `0` for the per-adapter 300s default — that would
    // make "0 disables" a lie. We assert that the call returns `0`
    // here so the watchdog-disable contract is locked in at the config
    // boundary the runner uses.
    let yaml = r#"
core:
  scratchpad: ".ralph/agent/scratchpad.md"
  specs_dir: "./specs"
event_loop:
  starting_event: "work.ready"
  completion_promise: "LOOP_COMPLETE"
  max_iterations: 5
cli:
  backend: claude
  default_mode: autonomous
  autonomous_idle_timeout_secs: 0
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).expect("parse config");
    assert_eq!(
        config.autonomous_idle_timeout_secs("claude"),
        0,
        "R8: explicit `autonomous_idle_timeout_secs: 0` must round-trip to 0 \
         (the disabled sentinel), not be silently replaced by the per-adapter \
         300s default. A regression here would make the doc / help text claim \
         `0 = disabled` while the runner still fires a 300s watchdog."
    );

    // Drive a real `execute_pty` call end-to-end with watchdog disabled.
    // The backend emits a short stdout line and exits naturally; the
    // disabled watchdog must NOT fire (would set `watchdog_timeout=true`).
    let (_interrupt_tx, interrupt_rx) = tokio::sync::watch::channel(false);
    let wallclock = Duration::from_secs(8);
    let outcome = tokio::time::timeout(
        wallclock,
        execute_pty(
            None,
            &backend,
            &config,
            "ignored",
            false, // autonomous path
            interrupt_rx,
            Verbosity::Quiet,
            None,
            None,
            0,
            "executor",
            "claude",
        ),
    )
    .await
    .expect(
        "with watchdog disabled, a natural-exit backend must complete without the \
         wallclock budget elapsing. If this times out, the resolver is firing a \
         watchdog on `autonomous_idle_timeout_secs: 0` (R8 regression).",
    )
    .expect("PTY observe must not return an io error");

    assert!(
        !outcome.watchdog_timeout,
        "R8: explicit `autonomous_idle_timeout_secs: 0` means the watchdog is \
         disabled; a backend that exits naturally must NOT be reported as a \
         watchdog fire. Got watchdog_timeout=true."
    );
}
