//! Tests for initialization.

use super::common::*;
use super::*;

#[test]
fn test_initialization_routes_to_ralph_in_multihat_mode() {
    // Per "Hatless Ralph" architecture: When custom hats are defined,
    // Ralph is always the executor. Custom hats define topology only.
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.start", "build.done", "build.blocked"]
    publishes: ["build.task"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.initialize("Test prompt");

    // Per spec: In multi-hat mode, Ralph handles all iterations
    let next = event_loop.next_hat();
    assert!(next.is_some());
    assert_eq!(
        next.unwrap().as_str(),
        "ralph",
        "Multi-hat mode should route to Ralph"
    );

    // Verify Ralph's prompt includes the event
    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        prompt.contains("task.start"),
        "Ralph's prompt should include the event"
    );
}

#[test]
fn test_loop_thrashing_detection() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");
    install_admitting_flow(&mut event_loop, &["build.blocked"]);

    // Builder blocks on "Fix bug" three times (should emit build.task.abandoned)
    write_event_to_jsonl(&events_path, "build.blocked", "Fix bug\nCan't compile");
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "build.blocked",
        "Fix bug\nStill can't compile",
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(&events_path, "build.blocked", "Fix bug\nReally stuck");
    let _ = event_loop.process_events_from_jsonl();

    // Task should be abandoned
    assert!(
        event_loop
            .state
            .abandoned_tasks
            .contains(&"Fix bug".to_string()),
        "Task should be abandoned after 3 blocks"
    );
}

#[test]
fn test_thrashing_counter_increments_on_blocked_events() {
    // Events now come from JSONL file via `ralph emit`, not from text output.
    // Per-hat tracking is removed since events don't carry hat context.
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    // Two blocked events should increment counter
    write_event_to_jsonl(&events_path, "build.blocked", "Stuck");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.consecutive_blocked, 1);

    write_event_to_jsonl(&events_path, "build.blocked", "Still stuck");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.consecutive_blocked, 2);
}

#[test]
fn test_thrashing_counter_resets_on_non_blocked_event() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    // Two blocked events
    write_event_to_jsonl(&events_path, "build.blocked", "Stuck");
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(&events_path, "build.blocked", "Still stuck");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.consecutive_blocked, 2);

    // Non-blocked event should reset counter
    write_event_to_jsonl(&events_path, "build.task", "Working now");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.consecutive_blocked, 0);
}

#[test]
fn test_task_cancellation_with_tilde_marker() {
    // Test that tasks marked with [~] are recognized as cancelled
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test task");

    let ralph_id = HatId::new("ralph");

    // Simulate Ralph output with cancelled task
    let output = r"
## Tasks
- [x] Task 1 completed
- [~] Task 2 cancelled (too complex for current scope)
- [ ] Task 3 pending
";

    // Process output - should not terminate since there are still pending tasks
    let reason = event_loop.process_output(&ralph_id, output, true);
    assert_eq!(reason, None, "Should not terminate with pending tasks");
}

#[test]
fn test_partial_completion_with_cancelled_tasks() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    // Create scratchpad with completed and cancelled tasks (use absolute path, no set_current_dir)
    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    let scratchpad_content = r"## Tasks
- [x] Core feature implemented
- [x] Tests added
- [~] Documentation update (cancelled: out of scope)
- [~] Performance optimization (cancelled: not needed)
";
    fs::write(&scratchpad_path, scratchpad_content).unwrap();

    // Test that cancelled tasks don't block completion when all other tasks are done
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    publishes: ["build.done"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test task");

    // Simulate completion with some cancelled tasks - should complete immediately
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Should complete immediately with partial completion (cancelled tasks ok)"
    );
}

#[test]
fn test_planner_auto_cancellation_after_three_blocks() {
    // Test that task is abandoned after 3 build.blocked events for same task
    // Events now come from JSONL via `ralph emit`.
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test task");

    // First blocked event for "Task X" - should not abandon
    write_event_to_jsonl(&events_path, "build.blocked", "Task X\nmissing dependency");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.task_block_counts.get("Task X"), Some(&1));

    // Second blocked event for "Task X" - should not abandon
    write_event_to_jsonl(
        &events_path,
        "build.blocked",
        "Task X\ndependency issue persists",
    );
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.task_block_counts.get("Task X"), Some(&2));

    // Third blocked event for "Task X" - should emit build.task.abandoned
    write_event_to_jsonl(
        &events_path,
        "build.blocked",
        "Task X\nsame dependency issue",
    );
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.task_block_counts.get("Task X"), Some(&3));
    assert!(
        event_loop
            .state
            .abandoned_tasks
            .contains(&"Task X".to_string()),
        "Task X should be abandoned"
    );
}
