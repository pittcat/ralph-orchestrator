//! Tests for payload_types.

use super::common::*;
use super::*;

#[test]
fn test_next_hat_isolated_returns_concrete_hat() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  strategist:
    name: "Strategist"
    description: "Plans"
    triggers: ["task.start"]
    publishes: ["experiment.planned"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    // After initialize, task.start is pending for strategist
    let next = event_loop.next_hat();
    assert!(next.is_some());
    assert_eq!(next.unwrap().as_str(), "strategist");
}

#[test]
fn test_next_hat_coordinator_returns_ralph() {
    let yaml = r#"
hats:
  strategist:
    name: "Strategist"
    description: "Plans"
    triggers: ["task.start"]
    publishes: ["experiment.planned"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    let next = event_loop.next_hat();
    assert!(next.is_some());
    assert_eq!(next.unwrap().as_str(), "ralph");
}

#[test]
fn test_isolated_prompt_contains_only_target_hat_instructions() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  implementer:
    name: "Implementer"
    description: "Implements"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
    instructions: "You are the implementer."
  reviewer:
    name: "Reviewer"
    description: "Reviews"
    triggers: ["experiment.ready"]
    publishes: ["review.done"]
    instructions: "You are the reviewer."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop
        .bus
        .publish(Event::new("experiment.planned", "Plan ready"));

    let next = event_loop.next_hat().unwrap().clone();
    assert_eq!(next.as_str(), "implementer");

    let prompt = event_loop.build_prompt(&next).unwrap();
    assert!(
        prompt.contains("You are the implementer."),
        "Prompt should contain implementer instructions"
    );
    assert!(
        !prompt.contains("You are the reviewer."),
        "Prompt should NOT contain reviewer instructions"
    );
    assert!(
        !prompt.contains("## HATS"),
        "Isolated prompt should not contain HATS section"
    );
}

#[test]
fn test_isolated_mode_accepts_only_first_business_event() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  strategist:
    name: "Strategist"
    description: "Plans"
    triggers: ["task.start"]
    publishes: ["experiment.planned", "experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    // Simulate process_output to set current_isolated_hat
    event_loop.process_output(&HatId::new("strategist"), "output", true);

    // Simulate strategist emitting two business events
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    write_event_with_hat_to_jsonl(&events_path, "experiment.planned", "plan1", "strategist");
    write_event_with_hat_to_jsonl(&events_path, "experiment.ready", "ready1", "strategist");

    // Replace event_reader with our test file
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    // Only experiment.planned should be in seen_topics
    assert!(
        event_loop
            .state()
            .seen_topics
            .contains("experiment.planned"),
        "First business event should be accepted"
    );
    // experiment.ready should NOT be accepted
    assert!(
        !event_loop.state().seen_topics.contains("experiment.ready"),
        "Second business event should be dropped in isolated mode"
    );
}

#[test]
fn test_string_payload_events_pass_through_normally() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    write_event_to_jsonl(&events_path, "work.start", "Begin task");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    assert!(
        event_loop.state().seen_topics.contains("work.start"),
        "String payload event should pass through normally"
    );
}

#[test]
fn test_object_payload_events_from_jsonl_converted_to_string() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let object_payload = serde_json::json!({"status": "ok", "count": 42});
    write_object_event_to_jsonl(&events_path, "task.done", object_payload);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    assert!(
        event_loop.state().seen_topics.contains("task.done"),
        "Object payload event should be accepted after conversion"
    );

    // Verify the event on the bus has a string payload (serialized JSON)
    let ralph_id = HatId::new("ralph");
    let pending = event_loop.bus.peek_pending(&ralph_id);
    assert!(pending.is_some(), "Event should be on the bus");
    let events = pending.unwrap();
    let event = events.iter().find(|e| e.topic.as_str() == "task.done");
    assert!(event.is_some(), "build.done event should exist on bus");
    let payload = &event.unwrap().payload;
    assert!(
        payload.contains("status") && payload.contains("ok"),
        "Object payload should be converted to JSON string, got: {}",
        payload
    );
}
