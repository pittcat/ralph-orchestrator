//! Tests for termination.

use super::common::*;
use super::*;

#[test]
fn test_termination_max_iterations() {
    let yaml = r"
event_loop:
  max_iterations: 2
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.state.iteration = 2;

    assert_eq!(
        event_loop.check_termination(),
        Some(TerminationReason::MaxIterations)
    );
}

#[test]
fn test_hard_gate_terminates_after_max() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    // Below threshold — should not terminate
    event_loop.state.consecutive_hard_gates = 2;
    assert_eq!(event_loop.check_termination(), None);

    // At threshold — should terminate with Stopped
    event_loop.state.consecutive_hard_gates = 3;
    assert_eq!(
        event_loop.check_termination(),
        Some(TerminationReason::Stopped)
    );
}

#[test]
fn test_hard_gate_count_methods() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    assert_eq!(event_loop.state().consecutive_hard_gates, 0);

    event_loop.increment_hard_gate_count();
    assert_eq!(event_loop.state().consecutive_hard_gates, 1);

    event_loop.increment_hard_gate_count();
    assert_eq!(event_loop.state().consecutive_hard_gates, 2);

    event_loop.reset_hard_gate_count();
    assert_eq!(event_loop.state().consecutive_hard_gates, 0);
}

#[test]
fn test_completion_promise_detection() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    // Create scratchpad with all tasks completed (use absolute path, no set_current_dir)
    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(
        &scratchpad_path,
        "## Tasks\n- [x] Task 1 done\n- [x] Task 2 done\n",
    )
    .unwrap();

    // Configure event loop to use temp directory scratchpad
    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // LOOP_COMPLETE event with all tasks done - should terminate immediately
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Should terminate immediately when LOOP_COMPLETE + tasks verified"
    );
}

#[test]
fn test_completion_promise_with_open_tasks_in_scratchpad_still_terminates() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    // Create scratchpad with PENDING tasks ([ ] markers)
    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(
        &scratchpad_path,
        "## Tasks\n- [x] Task 1 done\n- [ ] Task 2 still pending\n",
    )
    .unwrap();

    // Configure event loop to use temp directory scratchpad
    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Scratchpad mode still trusts the agent's completion signal even with open checklist items.
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Scratchpad mode should still trust the agent's decision"
    );
}

#[test]
fn test_completion_promise_with_pending_tasks_in_task_store_is_rejected() {
    use crate::loop_context::LoopContext;
    use crate::task::{Task, TaskStatus};
    use crate::task_store::TaskStore;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let tasks_path = temp_dir.path().join(".ralph/agent/tasks.jsonl");

    // Create task store with one open and one closed task
    let mut store = TaskStore::load(&tasks_path).unwrap();
    let mut task1 = Task::new("Completed task".to_string(), 1);
    task1.status = TaskStatus::Closed;
    store.add(task1);

    let task2 = Task::new("Still open task".to_string(), 2);
    store.add(task2);
    store.save().unwrap();

    // Configure event loop with memories enabled and pointing to temp dir
    let mut config = RalphConfig::default();
    config.memories.enabled = true;
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let loop_context = LoopContext::primary(temp_dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, loop_context);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Runtime tasks are the canonical queue in memories/tasks mode, so completion should be rejected.
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "Should reject completion while runtime tasks remain pending"
    );
    assert!(
        event_loop.has_pending_events(),
        "Rejecting completion should inject task.resume so the loop continues"
    );
}

#[test]
fn test_completion_promise_accepted_even_when_not_last_event() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Completion is now accepted regardless of position in batch (U5).
    // Events after it in the same batch are protected by completion guard.
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    write_event_to_jsonl(&events_path, "task.resume", "Continue");
    let result = event_loop.process_events_from_jsonl().unwrap();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Completion should be accepted even when not the last event"
    );
    // task.resume after LOOP_COMPLETE in same batch should still be published
    // (task.resume is not a business/terminal topic, so completion guard lets it through)
    assert!(
        result.had_events,
        "Non-business events after completion should still be published"
    );
}

#[test]
fn test_builder_cannot_terminate_loop() {
    // Per spec: completion requires an emitted event; output-only tokens are ignored
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    // Builder output containing completion promise - should be IGNORED
    let hat_id = HatId::new("builder");
    let reason = event_loop.process_output(&hat_id, "Done!\nLOOP_COMPLETE", true);

    // Builder cannot terminate, so no termination reason
    assert_eq!(reason, None);

    // Completion event should still terminate
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let completion = event_loop.check_completion_event();
    assert_eq!(completion, Some(TerminationReason::CompletionPromise));
}

#[test]
fn test_exit_codes_per_spec() {
    // Per spec "Loop Termination" section:
    // - 0: Completion promise detected (success)
    // - 1: Consecutive failures or unrecoverable error (failure)
    // - 2: Max iterations, max runtime, or max cost exceeded (limit)
    // - 130: User interrupt (SIGINT = 128 + 2)
    assert_eq!(TerminationReason::CompletionPromise.exit_code(), 0);
    assert_eq!(TerminationReason::ConsecutiveFailures.exit_code(), 1);
    assert_eq!(TerminationReason::LoopThrashing.exit_code(), 1);
    assert_eq!(TerminationReason::Stopped.exit_code(), 1);
    assert_eq!(TerminationReason::MaxIterations.exit_code(), 2);
    assert_eq!(TerminationReason::MaxRuntime.exit_code(), 2);
    assert_eq!(TerminationReason::MaxCost.exit_code(), 2);
    assert_eq!(TerminationReason::Interrupted.exit_code(), 130);
}

#[test]
fn test_termination_reason_mappings() {
    let cases = vec![
        (TerminationReason::CompletionPromise, "completed", 0, true),
        (TerminationReason::MaxIterations, "max_iterations", 2, false),
        (TerminationReason::MaxRuntime, "max_runtime", 2, false),
        (TerminationReason::MaxCost, "max_cost", 2, false),
        (
            TerminationReason::ConsecutiveFailures,
            "consecutive_failures",
            1,
            false,
        ),
        (TerminationReason::LoopThrashing, "loop_thrashing", 1, false),
        (
            TerminationReason::ValidationFailure,
            "validation_failure",
            1,
            false,
        ),
        (TerminationReason::Stopped, "stopped", 1, false),
        (TerminationReason::Interrupted, "interrupted", 130, false),
        (
            TerminationReason::RestartRequested,
            "restart_requested",
            3,
            false,
        ),
    ];

    for (reason, expected_str, expected_code, is_success) in cases {
        assert_eq!(reason.as_str(), expected_str);
        assert_eq!(reason.exit_code(), expected_code);
        assert_eq!(reason.is_success(), is_success);
    }
}

#[test]
fn test_termination_status_texts() {
    let cases = vec![
        (
            TerminationReason::CompletionPromise,
            "All tasks completed successfully.",
        ),
        (
            TerminationReason::MaxIterations,
            "Stopped at iteration limit.",
        ),
        (TerminationReason::MaxRuntime, "Stopped at runtime limit."),
        (TerminationReason::MaxCost, "Stopped at cost limit."),
        (
            TerminationReason::ConsecutiveFailures,
            "Too many consecutive failures.",
        ),
        (
            TerminationReason::LoopThrashing,
            "Loop thrashing detected - same hat repeatedly blocked.",
        ),
        (
            TerminationReason::ValidationFailure,
            "Too many consecutive malformed JSONL events.",
        ),
        (TerminationReason::Stopped, "Manually stopped."),
        (TerminationReason::Interrupted, "Interrupted by signal."),
        (
            TerminationReason::RestartRequested,
            "Restarting by human request.",
        ),
    ];

    for (reason, expected) in cases {
        assert_eq!(termination_status_text(&reason), expected);
    }
}

#[test]
fn test_format_duration_variants() {
    use std::time::Duration;

    assert_eq!(format_duration(Duration::from_secs(45)), "45s");
    assert_eq!(format_duration(Duration::from_secs(61)), "1m 1s");
    assert_eq!(format_duration(Duration::from_hours(1)), "1h 0m 0s");
    assert_eq!(format_duration(Duration::from_secs(3661)), "1h 1m 1s");
}

#[test]
fn test_extract_task_id_first_line_and_default() {
    assert_eq!(
        EventLoop::extract_task_id(" task-123 \nMore details"),
        "task-123"
    );
    assert_eq!(EventLoop::extract_task_id(""), "unknown");
}

#[test]
fn test_mutation_warning_reason_variants() {
    let fail = MutationEvidence {
        status: MutationStatus::Fail,
        score_percent: Some(12.5),
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&fail, Some(80.0)).unwrap(),
        "mutation testing failed"
    );

    let warn = MutationEvidence {
        status: MutationStatus::Warn,
        score_percent: Some(65.5),
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&warn, Some(80.0)).unwrap(),
        "mutation score below threshold (65.50%)"
    );

    let unknown = MutationEvidence {
        status: MutationStatus::Unknown,
        score_percent: None,
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&unknown, Some(80.0)).unwrap(),
        "mutation testing status unknown"
    );

    let pass_low = MutationEvidence {
        status: MutationStatus::Pass,
        score_percent: Some(70.0),
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&pass_low, Some(80.0)).unwrap(),
        "mutation score 70.00% below threshold 80.00%"
    );

    let pass_missing = MutationEvidence {
        status: MutationStatus::Pass,
        score_percent: None,
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&pass_missing, Some(80.0)).unwrap(),
        "mutation score missing (threshold 80.00%)"
    );

    let pass_high = MutationEvidence {
        status: MutationStatus::Pass,
        score_percent: Some(95.0),
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&pass_high, Some(80.0)),
        None
    );

    let pass_no_threshold = MutationEvidence {
        status: MutationStatus::Pass,
        score_percent: Some(10.0),
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&pass_no_threshold, None),
        None
    );
}

#[test]
fn test_termination_reason_exit_codes() {
    let cases = [
        (TerminationReason::CompletionPromise, 0),
        (TerminationReason::ConsecutiveFailures, 1),
        (TerminationReason::LoopThrashing, 1),
        (TerminationReason::ValidationFailure, 1),
        (TerminationReason::Stopped, 1),
        (TerminationReason::MaxIterations, 2),
        (TerminationReason::MaxRuntime, 2),
        (TerminationReason::MaxCost, 2),
        (TerminationReason::Interrupted, 130),
        (TerminationReason::RestartRequested, 3),
    ];

    for (reason, code) in cases {
        assert_eq!(reason.exit_code(), code, "{reason:?} exit code mismatch");
    }
}

/// P0-C (2026-06-10): fail-path auto-termination kicks in when the
/// verdict gate has observed a failing verdict on the LAST configured
/// topic. The verdict gate's purpose is to forbid `LOOP_COMPLETE` on
/// fail, but until this fix there was no other exit signal — the
/// loop would burn iterations forever. The new check in
/// `check_termination()` returns `TerminationReason::ReviewFailed`
/// with the topic that carried the final fail mirror.
#[test]
fn test_review_failed_termination_exit_code() {
    // ReviewFailed is a 1 (failure) per spec: workflow reached its
    // terminus but the verdict was fail, not the pass path.
    let reason = TerminationReason::ReviewFailed {
        topic: "report.done".to_string(),
    };
    assert_eq!(reason.exit_code(), 1);
    assert_eq!(reason.as_str(), "review_failed");
}

/// P0-C: when the verdict gate is configured, recording a fail
/// verdict on the LAST mirror topic should make the next
/// `check_termination()` return `ReviewFailed` (with that topic).
#[test]
fn test_review_failed_triggers_when_verdict_propagates_to_last_mirror() {
    use crate::config::VerdictGateConfig;

    let mut config = RalphConfig::default();
    // Mirror the ce-executor gate: REVIEW_COMPLETE is upstream,
    // report.done is the final downstream mirror.
    config.event_loop.verdict_gate = Some(VerdictGateConfig {
        topic: "REVIEW_COMPLETE".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
        additional_topics: vec!["report.done".to_string()],
    });

    let mut event_loop = EventLoop::new(config);
    // Drive the fail verdict to the LAST mirror (report.done).
    event_loop.state_mut().last_verdict_topic = Some("report.done".to_string());
    event_loop.state_mut().last_verdict_payload =
        Some(r#"{"pass_or_fail":"fail","verdict":"fail"}"#.to_string());

    let reason = event_loop.check_termination();
    match reason {
        Some(TerminationReason::ReviewFailed { topic }) => {
            assert_eq!(topic, "report.done");
        }
        other => panic!("expected ReviewFailed, got {other:?}"),
    }
}

/// P0-C: a fail verdict on an UPSTREAM topic (REVIEW_COMPLETE) must
/// NOT auto-terminate — the workflow still has to propagate the
/// verdict to the final mirror. The fix's correctness depends on
/// waiting for the verdict chain to drain.
#[test]
fn test_review_failed_does_not_trigger_on_upstream_only() {
    use crate::config::VerdictGateConfig;

    let mut config = RalphConfig::default();
    config.event_loop.verdict_gate = Some(VerdictGateConfig {
        topic: "REVIEW_COMPLETE".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
        additional_topics: vec!["report.done".to_string()],
    });

    let mut event_loop = EventLoop::new(config);
    // Only the upstream REVIEW_COMPLETE has fired.
    event_loop.state_mut().last_verdict_topic = Some("REVIEW_COMPLETE".to_string());
    event_loop.state_mut().last_verdict_payload = Some(r#"{"pass_or_fail":"fail"}"#.to_string());

    let reason = event_loop.check_termination();
    assert!(
        !matches!(reason, Some(TerminationReason::ReviewFailed { .. })),
        "ReviewFailed should not fire on upstream-only verdict, got {reason:?}"
    );
}

/// P0-C: a PASS verdict on the final mirror must not auto-terminate.
/// The verdict gate's job is to reject LOOP_COMPLETE only on FAIL;
/// a pass verdict is the happy path and the normal completion
/// machinery still applies.
#[test]
fn test_review_failed_does_not_trigger_on_pass_verdict() {
    use crate::config::VerdictGateConfig;

    let mut config = RalphConfig::default();
    config.event_loop.verdict_gate = Some(VerdictGateConfig {
        topic: "REVIEW_COMPLETE".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
        additional_topics: vec!["report.done".to_string()],
    });

    let mut event_loop = EventLoop::new(config);
    event_loop.state_mut().last_verdict_topic = Some("report.done".to_string());
    event_loop.state_mut().last_verdict_payload = Some(r#"{"pass_or_fail":"pass"}"#.to_string());

    let reason = event_loop.check_termination();
    assert!(
        !matches!(reason, Some(TerminationReason::ReviewFailed { .. })),
        "ReviewFailed must not fire on a pass verdict, got {reason:?}"
    );
}

#[test]
fn test_termination_reason_strings_and_flags() {
    let cases = [
        (TerminationReason::CompletionPromise, "completed", true),
        (TerminationReason::MaxIterations, "max_iterations", false),
        (TerminationReason::MaxRuntime, "max_runtime", false),
        (TerminationReason::MaxCost, "max_cost", false),
        (
            TerminationReason::ConsecutiveFailures,
            "consecutive_failures",
            false,
        ),
        (TerminationReason::LoopThrashing, "loop_thrashing", false),
        (
            TerminationReason::ValidationFailure,
            "validation_failure",
            false,
        ),
        (TerminationReason::Stopped, "stopped", false),
        (TerminationReason::Interrupted, "interrupted", false),
        (
            TerminationReason::RestartRequested,
            "restart_requested",
            false,
        ),
    ];

    for (reason, expected_str, is_success) in cases {
        assert_eq!(reason.as_str(), expected_str, "{reason:?} as_str mismatch");
        assert_eq!(
            reason.is_success(),
            is_success,
            "{reason:?} success mismatch"
        );
    }
}
