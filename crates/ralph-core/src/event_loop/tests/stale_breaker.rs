//! Tests for stale_breaker.

use super::common::*;
use super::*;

#[test]
fn test_stale_breaker_first_rejection_injects_resume() {
    // First rejection: should inject task.resume and NOT terminate
    let mut event_loop = setup_loop_with_required_events(vec!["review.done".to_string()]);
    event_loop.initialize("Test");

    // Simulate completion requested without required events
    event_loop.state.completion_requested = true;

    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "First rejection should not terminate");
    assert!(event_loop.has_pending_events(), "Should inject task.resume");
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 1,
        "Should have 1 consecutive rejection"
    );
}

#[test]
fn test_stale_breaker_second_rejection_still_no_terminate() {
    // Second same rejection: should still NOT terminate
    let mut event_loop = setup_loop_with_required_events(vec!["review.done".to_string()]);
    event_loop.initialize("Test");

    // First rejection
    event_loop.state.completion_requested = true;
    let _ = event_loop.check_completion_event();
    // Consume the task.resume event
    event_loop.build_prompt(&HatId::new("ralph"));
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);

    // Second rejection (same signature, no progress)
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Second same rejection should not terminate");
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 2,
        "Should have 2 consecutive rejections"
    );
}

#[test]
fn test_stale_breaker_third_rejection_returns_loop_stale() {
    // Third same rejection with no progress: should return LoopStale
    let mut event_loop = setup_loop_with_required_events(vec!["review.done".to_string()]);
    event_loop.initialize("Test");

    // First rejection
    event_loop.state.completion_requested = true;
    let _ = event_loop.check_completion_event();
    event_loop.build_prompt(&HatId::new("ralph"));

    // Second rejection
    event_loop.state.completion_requested = true;
    let _ = event_loop.check_completion_event();
    event_loop.build_prompt(&HatId::new("ralph"));

    // Third rejection
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::LoopStale),
        "Third same rejection with no progress should return LoopStale"
    );
}

#[test]
fn test_stale_breaker_open_tasks_three_rejections() {
    // Open runtime tasks causing completion rejection 3 times returns LoopStale
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let mut event_loop = setup_loop_with_tasks(temp_dir.path());
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // First rejection
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "First rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);

    // Second rejection
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Second rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 2);

    // Third rejection
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::LoopStale),
        "Third rejection with open tasks should return LoopStale"
    );
}

#[test]
fn test_stale_breaker_workflow_guard_three_rejections() {
    // Workflow guard incomplete 3 times returns LoopStale
    let mut event_loop = setup_loop_with_workflow_guards();
    event_loop.initialize("Test");

    // Start a workflow instance at phase 0 (simulates experiment.planned)
    // We need to advance the workflow progress directly since publishing to bus
    // doesn't go through workflow guard validation.
    event_loop
        .state
        .workflow_progress
        .advance("experiment", None, 0);

    // First rejection
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "First rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);

    // Second rejection
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Second rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 2);

    // Third rejection
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::LoopStale),
        "Third workflow guard rejection should return LoopStale"
    );
}

#[test]
fn test_stale_breaker_business_event_resets_counter() {
    // Two rejections with an accepted business event in between: counter resets
    let mut event_loop = setup_loop_with_required_events(vec!["review.done".to_string()]);
    event_loop.initialize("Test");

    // First rejection
    event_loop.state.completion_requested = true;
    let _ = event_loop.check_completion_event();
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);

    // Simulate accepted business event (adds to seen_topics as a business topic)
    event_loop
        .state
        .seen_topics
        .insert("build.task".to_string());

    // Second rejection (should see progress and reset counter)
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Should not terminate with progress");
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 1,
        "Counter should reset to 1 after business event progress"
    );
}

#[test]
fn test_stale_breaker_task_state_change_resets_counter() {
    // Two rejections with task state change in between: counter resets.
    // After closing the task, completion should be accepted (not rejected).
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let mut event_loop = setup_loop_with_tasks(temp_dir.path());
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // First rejection (task is open)
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "First rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);

    // Close the open task (simulates task state change)
    use crate::task_store::TaskStore;
    let tasks_path = temp_dir.path().join(".ralph/agent/tasks.jsonl");
    let mut store = TaskStore::load(&tasks_path).unwrap();
    let task_id = store.open().first().map(|t| t.id.clone());
    if let Some(id) = task_id.clone() {
        // 2026-06-30-001 P0-4: start the task first; the
        // close guard refuses never-started rows.
        store.start(&id).unwrap();
        store.close(&id);
    }
    store.save().unwrap();

    // Second attempt: task is now closed, completion should be accepted
    // (progress was made, counter reset, and completion is now valid)
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "After task closed, completion should be accepted"
    );
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 0,
        "Counter should be 0 after acceptance"
    );
}

#[test]
fn test_stale_breaker_system_event_does_not_reset() {
    // Two rejections with only system/diagnostic events in between: no reset
    let mut event_loop = setup_loop_with_required_events(vec!["review.done".to_string()]);
    event_loop.initialize("Test");

    // First rejection
    event_loop.state.completion_requested = true;
    let _ = event_loop.check_completion_event();
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);

    // Add only system topics (should NOT count as progress).
    // Use the structured `event.*` diagnostic topics here; the
    // test's pre-2026-06-28 mix included `human.guidance` as a
    // human-only diagnostic, but the topic was removed in
    // plan 2026-06-28-005. `plan.blocked` is NOT a system
    // event — it's the structured terminal orchestrator
    // topic with hat subscriptions — so it does not appear
    // here either.
    event_loop
        .state
        .seen_topics
        .insert("event.malformed".to_string());
    event_loop
        .state
        .seen_topics
        .insert("event.execution_contract.rejected".to_string());
    event_loop
        .state
        .seen_topics
        .insert("task.resume".to_string());

    // Second rejection (should NOT see progress from system topics)
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Should not terminate yet");
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 2,
        "Counter should NOT reset for system topics"
    );
}

#[test]
fn test_stale_breaker_normal_completion_still_works() {
    // Normal accepted completion still returns CompletionPromise
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let agent_dir = temp_dir.path().join(".agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    std::fs::write(
        &scratchpad_path,
        "## Tasks\n- [x] Task 1 done\n- [x] Task 2 done\n",
    )
    .unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Completion event with all tasks done
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Normal completion should still return CompletionPromise"
    );
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 0,
        "Rejection counter should be 0 after acceptance"
    );
}

#[test]
fn test_report_done_satisfies_ce_executor_completion_gate() {
    // Regression: `report.done` must satisfy the ce-executor completion gate.
    // This mirrors the production flow where reporter emits report.done, then
    // LOOP_COMPLETE terminates.
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = setup_loop_with_required_events(vec!["report.done".to_string()]);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // 2026-06-30-001 P0-5: route report.done through the JSONL
    // pipeline so the runtime's admit loop flips
    // `report_done_seen` via `record_event` (or the
    // per-event admit-loop assignment in
    // `process_parse_result`). Direct `seen_topics.insert`
    // bypasses the guard because it does not exercise the
    // accept path.
    write_event_to_jsonl(
        &events_path,
        "report.done",
        r#"{"verdict":"pass","summary":"done"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Write LOOP_COMPLETE to JSONL
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();

    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "report.done + LOOP_COMPLETE should terminate as CompletionPromise"
    );
}

#[test]
fn test_post_completion_business_events_do_not_reset_stale_breaker() {
    // Regression: after completion is accepted, post-completion fake/raw business
    // events must not reset the stale-breaker counter or trigger re-processing.
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = setup_loop_with_required_events(vec!["report.done".to_string()]);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Completion with report.done satisfied
    write_event_to_jsonl(
        &events_path,
        "report.done",
        r#"{"verdict":"pass","summary":"done"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "First completion should succeed"
    );

    // After completion is honored, inject a fake post-completion business event
    // (simulating raw output parsing leaking into the events file)
    write_event_to_jsonl(&events_path, "experiment.planned", r#"{"task_key":"x"}"#);
    let _ = event_loop.process_events_from_jsonl();

    // The stale-breaker counter should remain 0 since completion was already honored
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 0,
        "Post-completion events should not reset stale-breaker after honored completion"
    );
    assert!(
        event_loop.state.completion_honored,
        "completion_honored should remain true after post-completion events"
    );
}

#[test]
fn test_old_bad_required_events_stale_breaks() {
    // Regression: old bad config with mutually exclusive required events
    // (e.g. review.passed + review.complete as required) should stale-break
    // instead of infinitely retrying.
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = setup_loop_with_required_events(vec![
        "review.passed".to_string(),
        "review.complete".to_string(),
    ]);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Simulate: only review.complete appears (not review.passed)
    event_loop
        .state
        .seen_topics
        .insert("review.complete".to_string());

    // First rejection
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "First rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);
    // Consume the task.resume event that was injected
    event_loop.build_prompt(&HatId::new("ralph"));

    // Second rejection (same signature, no progress)
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Second rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 2);

    // Third rejection: stale-break
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::LoopStale),
        "Third rejection with old bad required-events should stale-break"
    );
}
