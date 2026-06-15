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

// 2026-06-15-003 fix U1: plan-gate Path A dual-publish
// (`queue.advance` + `work.ready`) is the only legitimate two-business-event
// pair in isolated mode. The following four tests pin the budget carve-out:
//   - happy: ordered pair both accepted
//   - reverse: only first accepted
//   - third: third business event still dropped
//   - non-pair: second business event still dropped when pair does not match
//
// Reference: docs/plans/2026-06-15-003-fix-plan-gate-dual-publish-isolated-budget-plan.md

#[test]
fn test_isolated_mode_accepts_queue_advance_work_ready_pair() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  plan-gate:
    name: "Plan Gate"
    description: "Advances plan steps"
    triggers: ["work.failed"]
    publishes: ["queue.advance", "work.ready"]
  executor:
    name: "Executor"
    description: "Executes a step"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    event_loop.process_output(&HatId::new("plan-gate"), "output", true);

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    write_event_with_hat_to_jsonl(&events_path, "queue.advance", "step-02", "plan-gate");
    write_event_with_hat_to_jsonl(
        &events_path,
        "work.ready",
        r#"{"task_id":"task-real-001"}"#,
        "plan-gate",
    );

    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    assert!(
        event_loop
            .state()
            .seen_topics
            .contains("queue.advance"),
        "queue.advance (first business event) should be accepted"
    );
    assert!(
        event_loop.state().seen_topics.contains("work.ready"),
        "work.ready (second business event in dual-publish pair) should be accepted"
    );
}

#[test]
fn test_isolated_mode_drops_work_ready_before_queue_advance() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  plan-gate:
    name: "Plan Gate"
    description: "Advances plan steps"
    triggers: ["work.failed"]
    publishes: ["queue.advance", "work.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    event_loop.process_output(&HatId::new("plan-gate"), "output", true);

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // Reverse order: work.ready first, then queue.advance.
    // Only the first business event is accepted; queue.advance is dropped.
    write_event_with_hat_to_jsonl(
        &events_path,
        "work.ready",
        r#"{"task_id":"task-real-002"}"#,
        "plan-gate",
    );
    write_event_with_hat_to_jsonl(&events_path, "queue.advance", "step-02", "plan-gate");

    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let _ = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        event_loop.state().seen_topics.contains("work.ready"),
        "First business event (work.ready) should be accepted"
    );
    assert!(
        !event_loop.state().seen_topics.contains("queue.advance"),
        "Second business event (queue.advance) should be dropped — pair order matters"
    );
}

#[test]
fn test_isolated_mode_drops_third_business_event_after_dual_publish_pair() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  plan-gate:
    name: "Plan Gate"
    description: "Advances plan steps"
    triggers: ["work.failed"]
    publishes: ["queue.advance", "work.ready", "experiment.planned"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    event_loop.process_output(&HatId::new("plan-gate"), "output", true);

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    write_event_with_hat_to_jsonl(&events_path, "queue.advance", "step-02", "plan-gate");
    write_event_with_hat_to_jsonl(
        &events_path,
        "work.ready",
        r#"{"task_id":"task-real-003"}"#,
        "plan-gate",
    );
    write_event_with_hat_to_jsonl(&events_path, "experiment.planned", "noise", "plan-gate");

    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let _ = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        event_loop.state().seen_topics.contains("queue.advance"),
        "queue.advance should be accepted"
    );
    assert!(
        event_loop.state().seen_topics.contains("work.ready"),
        "work.ready (second in pair) should be accepted"
    );
    assert!(
        !event_loop
            .state()
            .seen_topics
            .contains("experiment.planned"),
        "Third business event must still be dropped — pair exception is exactly 2 events"
    );
}

#[test]
fn test_isolated_mode_drops_non_pair_second_business_event() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  plan-gate:
    name: "Plan Gate"
    description: "Advances plan steps"
    triggers: ["work.failed"]
    publishes: ["queue.advance", "work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    event_loop.process_output(&HatId::new("plan-gate"), "output", true);

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // queue.advance → work.done is NOT a white-listed pair. work.done is dropped.
    write_event_with_hat_to_jsonl(&events_path, "queue.advance", "step-02", "plan-gate");
    write_event_with_hat_to_jsonl(
        &events_path,
        "work.done",
        r#"{"ok":true}"#,
        "plan-gate",
    );

    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let _ = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        event_loop.state().seen_topics.contains("queue.advance"),
        "queue.advance should be accepted"
    );
    assert!(
        !event_loop.state().seen_topics.contains("work.done"),
        "work.done (not the white-listed second topic) must be dropped"
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
