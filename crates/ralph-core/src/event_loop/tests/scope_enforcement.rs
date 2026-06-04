//! Tests for scope_enforcement.

use super::common::*;
use super::*;

#[test]
fn test_scope_enforcement_drops_unauthorized_event() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  enforce_hat_scope: true
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Set builder as the active hat
    event_loop.state.last_active_hat_ids = vec![HatId::new("builder")];

    // Builder tries to emit LOOP_COMPLETE (not in its publishes)
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();

    // completion_requested should be false — the event was dropped by scope enforcement
    assert!(
        !event_loop.state.completion_requested,
        "LOOP_COMPLETE should be dropped when builder hat is active (not in publishes)"
    );
}

#[test]
fn test_scope_enforcement_allows_authorized_event() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  enforce_hat_scope: true
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done", "build.blocked"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Set builder as the active hat
    event_loop.state.last_active_hat_ids = vec![HatId::new("builder")];

    // Builder emits build.done (in its publishes) — should pass through
    write_event_to_jsonl(
        &events_path,
        "build.done",
        "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass",
    );
    let _ = event_loop.process_events_from_jsonl();

    // The event should have been published to the bus (not dropped)
    assert!(
        event_loop.has_pending_events(),
        "build.done should pass scope enforcement when builder is active"
    );
}

#[test]
fn test_scope_enforcement_allows_only_control_when_no_active_hats() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  enforce_hat_scope: true
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // No active hats (Ralph coordinating)
    event_loop.state.last_active_hat_ids = vec![];

    // LOOP_COMPLETE should pass through when Ralph is coordinating
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();

    assert!(
        event_loop.state.completion_requested,
        "LOOP_COMPLETE should be accepted when no active hats (Ralph coordinating)"
    );

    write_event_to_jsonl(&events_path, "build.done", "fake business event");
    let processed = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !processed
            .accepted_events
            .iter()
            .any(|event| event.topic.as_str() == "build.done"),
        "business events without an active hat should be rejected in hat-based mode"
    );
}

#[test]
fn test_scope_violation_event_published() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  enforce_hat_scope: true
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Set builder as the active hat
    event_loop.state.last_active_hat_ids = vec![HatId::new("builder")];

    // Builder tries to emit plan.approved (not in its publishes)
    write_event_to_jsonl(&events_path, "plan.approved", "Auto-approved");
    let _ = event_loop.process_events_from_jsonl();

    // A scope_violation event should have been published to the bus
    assert!(
        event_loop.has_pending_events(),
        "Scope violation event should be published to the bus"
    );
}

#[test]
fn test_ce_executor_shipper_cannot_publish_build_done() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  enforce_hat_scope: true
  completion_promise: LOOP_COMPLETE
hats:
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked", "fix.exhausted"]
    publishes: ["REVIEW_COMPLETE"]
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    event_loop.state.last_active_hat_ids = vec![HatId::new("shipper")];

    write_event_with_hat_to_jsonl(&events_path, "build.done", r#"{"ok":true}"#, "shipper");
    let processed = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        !processed
            .accepted_events
            .iter()
            .any(|event| event.topic.as_str() == "build.done"),
        "shipper must not be allowed to publish build.done"
    );
    assert!(
        event_loop.has_pending_events(),
        "scope violation should be published for rejected shipper build.done"
    );
}
