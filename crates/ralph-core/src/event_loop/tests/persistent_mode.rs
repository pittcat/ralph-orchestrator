//! Tests for persistent_mode.

use super::common::*;
use super::*;

#[test]
fn test_persistent_mode_suppresses_loop_complete() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(&scratchpad_path, "## Tasks\n- [x] All done\n").unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    config.event_loop.persistent = true;
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // LOOP_COMPLETE should NOT terminate in persistent mode
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "Persistent mode should suppress LOOP_COMPLETE termination"
    );

    // Verify a task.resume event was injected so the loop continues
    let ralph_id = HatId::new("ralph");
    let pending = event_loop.bus.peek_pending(&ralph_id);
    assert!(
        pending.is_some_and(|events| events
            .iter()
            .any(|e| e.topic.as_str() == "task.resume" && e.payload.contains("Persistent mode"))),
        "A task.resume event should be injected after suppressed LOOP_COMPLETE"
    );
}

#[test]
fn test_non_persistent_mode_terminates_on_loop_complete() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(&scratchpad_path, "## Tasks\n- [x] All done\n").unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    // persistent defaults to false, but be explicit
    config.event_loop.persistent = false;
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // LOOP_COMPLETE should terminate normally when not persistent
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Non-persistent mode should terminate on LOOP_COMPLETE"
    );
}

#[test]
fn test_persistent_mode_still_respects_hard_limits() {
    let yaml = r"
event_loop:
  max_iterations: 2
  persistent: true
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.state.iteration = 2;

    // Hard limits should still terminate even in persistent mode
    assert_eq!(
        event_loop.check_termination(),
        Some(TerminationReason::MaxIterations),
        "Persistent mode should still respect max_iterations"
    );
}
