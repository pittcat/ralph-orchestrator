//! Tests for event_policy.

use super::common::*;
use super::*;

#[test]
fn test_workflow_guards_absent_means_no_chain_validation() {
    let yaml = r#"
event_loop:
  workflow_guards:
    chains: []
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // Emit out-of-order events that would violate a chain if one existed
    write_event_to_jsonl(&events_path, "experiment.evaluated", "done");
    write_event_to_jsonl(&events_path, "experiment.planned", "plan");

    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    assert!(
        event_loop
            .state()
            .seen_topics
            .contains("experiment.evaluated"),
        "Events should pass through when workflow_guards has empty chains"
    );
    assert!(
        event_loop
            .state()
            .seen_topics
            .contains("experiment.planned"),
        "Events should pass through when workflow_guards has empty chains"
    );
}

#[test]
fn test_empty_required_events_allows_completion() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    // Create scratchpad with all tasks completed
    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(&scratchpad_path, "## Tasks\n- [x] Task 1 done\n").unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    // required_events is empty by default
    assert!(config.event_loop.required_events.is_empty());

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Empty required_events should allow completion"
    );
}

#[test]
fn test_completion_promise_behavior_unchanged_without_event_policy() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(&scratchpad_path, "## Tasks\n- [x] All done\n").unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    // Ensure no event_policy is configured (default)
    assert!(config.event_loop.event_policy.is_none());

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Finished");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "completion_promise behavior should be unchanged when event_policy is absent"
    );
}

#[test]
fn test_event_policy_observe_mode_allows_bad_events_with_diagnostics() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: observe
    on_violation: warn
    schemas:
      test.topic:
        payload: json_object
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // String payload when JSON object required
    write_event_to_jsonl(&events_path, "test.topic", "plain string");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    // Event should still be on bus (observe mode)
    let ralph_id = HatId::new("ralph");
    let pending = event_loop.bus.peek_pending(&ralph_id);
    assert!(pending.is_some());
}

#[test]
fn test_event_policy_enforce_reject_replaces_with_task_resume() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      test.topic:
        payload: json_object
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    write_event_to_jsonl(&events_path, "test.topic", "plain string");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    // When event is rejected, validated_events is empty, so had_events is false
    // But task.resume is published directly to bus during policy validation
    assert!(
        !result.had_events,
        "Rejected events should not count as had_events"
    );
    // Bad event should NOT be on bus, but task.resume should be
    let ralph_id = HatId::new("ralph");
    let pending = event_loop.bus.peek_pending(&ralph_id);
    assert!(pending.is_some());
    let events = pending.unwrap();
    assert!(
        events.iter().any(|e| e.topic.as_str() == "task.resume"),
        "task.resume should be published for policy rejection"
    );
    assert!(
        !events.iter().any(|e| e.topic.as_str() == "test.topic"),
        "Bad event should NOT be on bus"
    );
}

#[test]
fn test_no_event_policy_skips_validation() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // Use a plain topic (not build.done which has special backpressure handling)
    write_event_to_jsonl(&events_path, "test.event", "Test payload");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    // Without event_policy, behavior should be unchanged
    assert!(event_loop.state().seen_topics.contains("test.event"));
}

#[test]
fn test_wave_events_update_terminal_observed_for_policy() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "review.file"
    business_topics:
      - "task.update"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 2
    instructions: "Review code."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Write a wave event with a terminal topic
    {
        use std::io::Write;
        let event = serde_json::json!({
            "topic": "review.file",
            "payload": "src/main.rs",
            "ts": chrono::Utc::now().to_rfc3339(),
            "wave_id": "w-1",
            "wave_index": 0,
            "wave_total": 1,
        });
        writeln!(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path)
                .unwrap(),
            "{}",
            event
        )
        .unwrap();
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();
    assert_eq!(
        result.wave_events.len(),
        1,
        "Wave event should be partitioned"
    );
    assert_eq!(result.wave_events[0].topic, "review.file");

    // Write a business event after the terminal topic
    write_event_to_jsonl(&events_path, "task.update", "update");

    let result = event_loop.process_events_from_jsonl().unwrap();
    // Business event after terminal should be rejected, so had_events is false
    assert!(
        !result.had_events,
        "Business event after terminal should be rejected"
    );

    // task.resume should be on the bus due to monotonicity violation
    let ralph_id = HatId::new("ralph");
    let pending = event_loop.bus.peek_pending(&ralph_id);
    assert!(pending.is_some());
    let events = pending.unwrap();
    assert!(
        events.iter().any(|e| e.topic.as_str() == "task.resume"),
        "task.resume should be published for terminal monotonicity violation"
    );
    assert!(
        events.iter().any(|e| e.payload.contains("monotonicity")),
        "Violation message should mention monotonicity"
    );
}
