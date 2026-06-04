//! Tests for human_timeout.

use super::common::*;
use super::*;

#[test]
fn test_human_timeout_injects_timeout_event() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.set_robot_service(Box::new(MockRobotService {
        timeout: 5,
        should_timeout: true,
    }));

    // Write a human.interact event
    write_event_to_jsonl(&events_path, "human.interact", "Please review this plan");
    let _ = event_loop.process_events_from_jsonl();

    // The bus should have a human.timeout event (from the mock timeout)
    assert!(
        event_loop.has_pending_events(),
        "human.timeout event should be published on timeout"
    );
}

#[test]
fn test_human_response_still_works() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.set_robot_service(Box::new(MockRobotService {
        timeout: 5,
        should_timeout: false,
    }));

    // Write a human.interact event — mock returns "approved"
    write_event_to_jsonl(&events_path, "human.interact", "Please review this plan");
    let _ = event_loop.process_events_from_jsonl();

    // The bus should have a human.response event
    assert!(
        event_loop.has_pending_events(),
        "human.response event should be published when response received"
    );
}

#[test]
fn test_user_prompt_restart_request_creates_restart_signal_file() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "user.prompt", "Please restart yourself");
    let _ = event_loop.process_events_from_jsonl();

    assert!(
        temp_dir.path().join(".ralph/restart-requested").exists(),
        "user.prompt restart request should create restart signal file"
    );
}

#[test]
fn test_human_response_restart_request_creates_restart_signal_file() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.set_robot_service(Box::new(RestartRequestRobotService));

    write_event_to_jsonl(&events_path, "human.interact", "Need approval");
    let _ = event_loop.process_events_from_jsonl();

    assert!(
        temp_dir.path().join(".ralph/restart-requested").exists(),
        "human.response restart request should create restart signal file"
    );
}
