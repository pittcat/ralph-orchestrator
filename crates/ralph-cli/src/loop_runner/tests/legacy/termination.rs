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
use crate::test_support::CwdGuard;
use ralph_core::HatRegistry;
use ralph_core::planning_session::{ConversationEntry, ConversationType};
use ralph_proto::{Hat, Topic};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::sync::{Arc, Mutex};
use super::super::common::*;
use super::super::fake_path::*;
use super::helpers::*;

// Test: test_pty_only_enabled_for_tui_rpc_or_interactive
#[test]
fn test_pty_only_enabled_for_tui_rpc_or_interactive() {
    let should_use_pty = |enable_tui: bool, enable_rpc: bool, user_interactive: bool| -> bool {
        enable_tui || enable_rpc || user_interactive
    };

    assert!(!should_use_pty(false, false, false));
    assert!(should_use_pty(true, false, false));
    assert!(should_use_pty(false, true, false));
    assert!(should_use_pty(false, false, true));
}

// Test: test_user_interactive_mode_determination
#[test]
fn test_user_interactive_mode_determination() {
    // user_interactive is determined by default_mode setting, not PTY.
    // PTY handles output streaming; user_interactive handles input forwarding.

    // Autonomous mode: no user input forwarding
    let autonomous_interactive = false;
    assert!(
        !autonomous_interactive,
        "Autonomous mode should not forward user input"
    );

    // Interactive mode with TTY: forward user input
    let interactive_with_tty = true;
    assert!(
        interactive_with_tty,
        "Interactive mode with TTY should forward user input"
    );
}

// Test: test_prepare_tui_iteration_seeds_max_iterations
#[test]
fn test_prepare_tui_iteration_seeds_max_iterations() {
    let state = Arc::new(Mutex::new(ralph_tui::TuiState::new()));

    let lines = prepare_tui_iteration(&state, "Planner".to_string(), "claude".to_string(), 42);

    assert!(lines.is_some(), "should return a lines handle");
    let state = state.lock().expect("state lock");
    assert_eq!(state.max_iterations, Some(42));
    assert_eq!(state.total_iterations(), 1);
}

// Test: test_fail_if_blocking_loop_termination_outcomes_allows_non_blocking_dispositions
#[test]
fn test_fail_if_blocking_loop_termination_outcomes_allows_non_blocking_dispositions() {
    let outcomes = vec![
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PreLoopComplete,
            hook_name: "warn-hook".to_string(),
            disposition: HookDisposition::Warn,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: Some(HookDispatchFailure::HookRunFailed {
                exit_code: Some(9),
                timed_out: false,
            }),

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PostLoopError,
            hook_name: "pass-hook".to_string(),
            disposition: HookDisposition::Pass,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: None,

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
    ];

    assert!(fail_if_blocking_loop_termination_outcomes(&outcomes).is_ok());
}

// Test: test_fail_if_blocking_loop_termination_outcomes_surfaces_failure_context
#[test]
fn test_fail_if_blocking_loop_termination_outcomes_surfaces_failure_context() {
    let blocked_timeout_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PostLoopError,
        hook_name: "block-timeout-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookRunFailed {
            exit_code: None,
            timed_out: true,
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_timeout_error =
        fail_if_blocking_loop_termination_outcomes(&blocked_timeout_outcomes)
            .expect_err("block disposition should fail loop termination boundary");
    let blocked_timeout_message = blocked_timeout_error.to_string();
    assert!(blocked_timeout_message.contains("block-timeout-hook"));
    assert!(blocked_timeout_message.contains("post.loop.error"));
    assert!(blocked_timeout_message.contains("hook timed out"));

    let blocked_exec_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PreLoopComplete,
        hook_name: "block-exec-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookExecutionError {
            message: "spawn failed".to_string(),
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_exec_error = fail_if_blocking_loop_termination_outcomes(&blocked_exec_outcomes)
        .expect_err("block disposition should fail loop termination boundary");
    let blocked_exec_message = blocked_exec_error.to_string();
    assert!(blocked_exec_message.contains("block-exec-hook"));
    assert!(blocked_exec_message.contains("pre.loop.complete"));
    assert!(blocked_exec_message.contains("hook execution failed: spawn failed"));
}

// Test: test_wait_for_resume_if_suspended_is_noop_without_suspend_dispositions
#[test]
fn test_wait_for_resume_if_suspended_is_noop_without_suspend_dispositions() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PreLoopStart,
        hook_name: "warn-hook".to_string(),
        disposition: HookDisposition::Warn,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(7),
            timed_out: false,
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    assert_eq!(wait_result, None);
    assert!(!suspend_state_store.suspend_state_path().exists());
    assert!(!suspend_state_store.resume_requested_path().exists());
}

// Test: test_wait_for_resume_if_suspended_resumes_and_clears_suspend_artifacts
#[test]
fn test_wait_for_resume_if_suspended_resumes_and_clears_suspend_artifacts() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());
    suspend_state_store
        .write_resume_requested()
        .expect("write resume signal");

    let outcomes = vec![suspend_outcome(
        HookPhaseEvent::PreLoopStart,
        "suspend-hook",
    )];

    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    assert_eq!(wait_result, None);
    assert!(!suspend_state_store.suspend_state_path().exists());
    assert!(!suspend_state_store.resume_requested_path().exists());
}

// Test: test_wait_for_resume_if_suspended_prioritizes_stop_over_resume
#[test]
fn test_wait_for_resume_if_suspended_prioritizes_stop_over_resume() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    std::fs::create_dir_all(temp_dir.path().join(".ralph")).expect("create .ralph");
    std::fs::write(temp_dir.path().join(".ralph/stop-requested"), "").expect("write stop signal");
    suspend_state_store
        .write_resume_requested()
        .expect("write resume signal");

    let outcomes = vec![suspend_outcome(
        HookPhaseEvent::PreIterationStart,
        "suspend-hook",
    )];

    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    assert_eq!(wait_result, Some(TerminationReason::Stopped));
    assert!(!temp_dir.path().join(".ralph/stop-requested").exists());
    assert!(!suspend_state_store.suspend_state_path().exists());
    assert!(!suspend_state_store.resume_requested_path().exists());
}

// Test: test_wait_for_resume_if_suspended_prioritizes_restart_over_resume
#[test]
fn test_wait_for_resume_if_suspended_prioritizes_restart_over_resume() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    std::fs::create_dir_all(temp_dir.path().join(".ralph")).expect("create .ralph");
    std::fs::write(temp_dir.path().join(".ralph/restart-requested"), "")
        .expect("write restart signal");
    suspend_state_store
        .write_resume_requested()
        .expect("write resume signal");

    let outcomes = vec![suspend_outcome(
        HookPhaseEvent::PostIterationStart,
        "suspend-hook",
    )];

    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    assert_eq!(wait_result, Some(TerminationReason::RestartRequested));
    assert!(temp_dir.path().join(".ralph/restart-requested").exists());
    assert!(!suspend_state_store.suspend_state_path().exists());
    assert!(!suspend_state_store.resume_requested_path().exists());
}

// Test: test_convert_termination_idle_timeout_autonomous_is_none_characterization
#[test]
fn test_convert_termination_idle_timeout_autonomous_is_none_characterization() {
    // Given: the autonomous / RPC / worktree path that Unit 2's watchdog fires on.
    let termination_type = ralph_adapters::TerminationType::IdleTimeout;
    let interactive = false;

    // When/Then: Unit 3 remaps this to None so the runner can still process
    // any partial events the agent emitted before the watchdog killed the
    // backend. The "watchdog fired" cause is preserved via
    // `ExecutionOutcome.watchdog_timeout` (see execution.rs) and logged as a
    // warn! line in runner.rs — both compose to satisfy R3 without falsely
    // declaring success and without bypassing event parsing.
    let result = convert_termination_type(termination_type, interactive);

    assert!(
        result.is_none(),
        "Characterization (post Unit 3): autonomous IdleTimeout maps to None \
         so the runner continues to event parsing / hard-gate fallback. The \
         legacy `Some(TerminationReason::Stopped)` mapping short-circuited \
         the partial-event pipeline (R7 violation) and is no longer correct."
    );
}

// Test: test_convert_termination_idle_timeout_interactive_is_none_characterization
#[test]
fn test_convert_termination_idle_timeout_interactive_is_none_characterization() {
    // Given: interactive mode
    let termination_type = ralph_adapters::TerminationType::IdleTimeout;
    let interactive = true;

    // When/Then: interactive IdleTimeout has always mapped to None (the
    // event loop continues, output is processed for events). R2 requires
    // this semantic to be preserved by Units 2/3; Unit 3 keeps it intact
    // and now matches the autonomous mapping above.
    let result = convert_termination_type(termination_type, interactive);

    assert!(
        result.is_none(),
        "Characterization: interactive IdleTimeout maps to None (iteration \
         continues, output is processed for events). R2 requires this \
         semantic to be preserved by Units 2/3."
    );
}

// Test: test_natural_termination_always_continues
#[test]
fn test_natural_termination_always_continues() {
    // Given: Natural termination in any mode
    let termination_type = ralph_adapters::TerminationType::Natural;

    // When/Then: should return None regardless of mode
    assert!(
        convert_termination_type(termination_type.clone(), true).is_none(),
        "Natural termination should continue in interactive mode"
    );
    assert!(
        convert_termination_type(termination_type, false).is_none(),
        "Natural termination should continue in autonomous mode"
    );
}

// Test: test_user_interrupt_always_terminates
#[test]
fn test_user_interrupt_always_terminates() {
    // Given: UserInterrupt termination in any mode
    let termination_type = ralph_adapters::TerminationType::UserInterrupt;

    // When/Then: should return Interrupted regardless of mode
    assert_eq!(
        convert_termination_type(termination_type.clone(), true),
        Some(TerminationReason::Interrupted),
        "UserInterrupt should terminate in interactive mode"
    );
    assert_eq!(
        convert_termination_type(termination_type, false),
        Some(TerminationReason::Interrupted),
        "UserInterrupt should terminate in autonomous mode"
    );
}

// Test: test_force_kill_always_terminates
#[test]
fn test_force_kill_always_terminates() {
    // Given: ForceKill termination in any mode
    let termination_type = ralph_adapters::TerminationType::ForceKill;

    // When/Then: should return Interrupted regardless of mode
    assert_eq!(
        convert_termination_type(termination_type.clone(), true),
        Some(TerminationReason::Interrupted),
        "ForceKill should terminate in interactive mode"
    );
    assert_eq!(
        convert_termination_type(termination_type, false),
        Some(TerminationReason::Interrupted),
        "ForceKill should terminate in autonomous mode"
    );
}

// Test: test_autonomous_watchdog_timeout_does_not_force_stop_loop
#[test]
fn test_autonomous_watchdog_timeout_does_not_force_stop_loop() {
    let result = convert_termination_type(
        ralph_adapters::TerminationType::IdleTimeout,
        false, // autonomous / RPC / worktree path
    );

    assert!(
        result.is_none(),
        "Unit 3: autonomous IdleTimeout must NOT map to a TerminationReason \
         (the legacy `Stopped` mapping short-circuited event parsing). The \
         runner needs `None` here so it falls through to `process_output` / \
         `process_events_from_jsonl` and partial events become visible."
    );
}

// Test: test_watchdog_timeout_keeps_termination_none_so_event_pipeline_runs
#[test]
fn test_watchdog_timeout_keeps_termination_none_so_event_pipeline_runs() {
    // Simulate the ExecutionOutcome produced by `execute_pty` for an
    // autonomous watchdog fire.
    let outcome = ExecutionOutcome {
        output: String::new(),
        success: false,
        termination: convert_termination_type(ralph_adapters::TerminationType::IdleTimeout, false),
        watchdog_timeout: true,
        total_cost_usd: 0.0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    };

    assert!(
        outcome.termination.is_none(),
        "Watchdog timeout MUST leave `termination = None` so the runner's \
         `if let Some(reason) = outcome.termination` short-circuit is \
         skipped. Without this, `process_output` and \
         `process_events_from_jsonl` never run and the missing-event hard \
         gate / fallback path cannot recover."
    );
    assert!(
        outcome.watchdog_timeout,
        "Diagnostic flag must be true so the runner can log the cause"
    );
}

// Test: test_execution_outcome_watchdog_flag_is_set_for_idle_timeout
#[test]
fn test_execution_outcome_watchdog_flag_is_set_for_idle_timeout() {
    let cases = [
        (ralph_adapters::TerminationType::IdleTimeout, true, true),
        (ralph_adapters::TerminationType::IdleTimeout, false, true),
        (ralph_adapters::TerminationType::Natural, true, false),
        (ralph_adapters::TerminationType::Natural, false, false),
        (ralph_adapters::TerminationType::UserInterrupt, false, false),
        (ralph_adapters::TerminationType::ForceKill, false, false),
    ];
    for (kind, interactive, expected) in cases {
        // Mirror the assignment `execute_pty` performs.
        let watchdog = matches!(kind, ralph_adapters::TerminationType::IdleTimeout);
        assert_eq!(
            watchdog, expected,
            "watchdog_timeout flag for {:?} interactive={} should be {}",
            kind, interactive, expected
        );
    }
}

// Test: test_autonomous_watchdog_timeout_never_maps_to_loop_terminate
#[test]
fn test_autonomous_watchdog_timeout_never_maps_to_loop_terminate() {
    let result = convert_termination_type(ralph_adapters::TerminationType::IdleTimeout, false);

    // The only acceptable mapping per Unit 3 is `None`. Spell out the
    // forbidden mappings so future edits explain themselves.
    if let Some(reason) = result {
        panic!(
            "Autonomous IdleTimeout must NOT terminate the loop. Mapping \
             it to {:?} would bypass event parsing and let a watchdog \
             fire fake-pass plan-gate / review / hard-gate. See Unit 3 \
             of plan 2026-06-06-001.",
            reason
        );
    }
}

// Test: test_non_timeout_terminations_unchanged_by_unit_3
#[test]
fn test_non_timeout_terminations_unchanged_by_unit_3() {
    // Natural: always None (let runner drain events normally).
    assert_eq!(
        convert_termination_type(ralph_adapters::TerminationType::Natural, true),
        None,
    );
    assert_eq!(
        convert_termination_type(ralph_adapters::TerminationType::Natural, false),
        None,
    );
    // UserInterrupt / ForceKill: always Interrupted (operator action).
    assert_eq!(
        convert_termination_type(ralph_adapters::TerminationType::UserInterrupt, true),
        Some(TerminationReason::Interrupted),
    );
    assert_eq!(
        convert_termination_type(ralph_adapters::TerminationType::UserInterrupt, false),
        Some(TerminationReason::Interrupted),
    );
    assert_eq!(
        convert_termination_type(ralph_adapters::TerminationType::ForceKill, true),
        Some(TerminationReason::Interrupted),
    );
    assert_eq!(
        convert_termination_type(ralph_adapters::TerminationType::ForceKill, false),
        Some(TerminationReason::Interrupted),
    );
}

// Test: test_convert_termination_autonomous_idle_timeout_emits_no_warn
#[test]
fn test_convert_termination_autonomous_idle_timeout_emits_no_warn() {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    /// Shared buffer the `MakeWriter` impl drains into.
    #[derive(Clone, Default)]
    struct VecWriter(Arc<Mutex<Vec<u8>>>);
    impl Write for VecWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> MakeWriter<'a> for VecWriter {
        type Writer = VecWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    let writer = VecWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_target(false)
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        // Control: a hand-rolled warn emitted inside the same scope MUST be
        // captured. If the buffer stays empty, the subscriber wiring is broken
        // and the negative assertion below would be a false pass.
        tracing::warn!("CONTROL_PROBE: capture layer is wired up");

        // The function under test — must remain silent.
        let result = convert_termination_type(
            ralph_adapters::TerminationType::IdleTimeout,
            false, // autonomous / RPC / worktree path
        );
        assert!(
            result.is_none(),
            "convert_termination_type(IdleTimeout, autonomous) must still return None \
             (regression check: Unit 3 contract preserved by I-1). Got: {:?}",
            result
        );
    });

    let captured = String::from_utf8(writer.0.lock().unwrap().clone()).expect("utf-8");

    // Control assertion first: if this fails, the capture layer itself is
    // broken and the negative assertion below would be unreliable.
    assert!(
        captured.contains("CONTROL_PROBE"),
        "Capture layer is not wired up — the test would silently pass on regressions. \
         Captured logs were:\n{}",
        captured
    );

    // Pin the specific message we deleted in I-1. We match on the unique
    // phrase so a future unrelated warn from this file (e.g. a new
    // characterization test) does not break the test.
    assert!(
        !captured.contains("Autonomous PTY watchdog timeout reached"),
        "convert_termination_type(IdleTimeout, autonomous) must NOT emit a warn. \
         The 'backend watchdog timeout' diagnostic is the runner's sole \
         responsibility; emitting it here would duplicate the warn and break \
         the PTY vs CliExecutor diagnostic parity that I-1 restored. \
         Captured logs were:\n{}",
        captured
    );
}
