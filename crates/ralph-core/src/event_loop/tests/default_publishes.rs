//! Tests for default_publishes.

use super::*;

#[test]
fn test_default_publishes_injects_when_no_events() {
    use std::collections::HashMap;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    let mut hats = HashMap::new();
    hats.insert(
        "test-hat".to_string(),
        crate::config::HatConfig {
            name: "test-hat".to_string(),
            description: Some("Test hat for default publishes".to_string()),
            triggers: vec!["task.start".to_string()],
            publishes: vec!["task.done".to_string()],
            instructions: "Test hat".to_string(),
            extra_instructions: vec![],
            backend_args: None,
            backend: None,
            default_publishes: Some("task.done".to_string()),
            max_activations: None,
            scratchpad: None,
            disallowed_tools: vec![],
            timeout: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            phase_triggers: None,
            ..Default::default()
        },
    );
    config.hats = hats;

    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    let hat_id = HatId::new("test-hat");

    // Agent wrote no events — process_events_from_jsonl would return had_events: false
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(!result.had_events, "No events should be found");

    // check_default_publishes should inject the default
    event_loop.check_default_publishes(&hat_id);

    assert!(
        event_loop.has_pending_events(),
        "Default event should be injected"
    );

    // The default_publishes topic should be recorded in seen_topics
    assert!(
        event_loop.state.seen_topics.contains("task.done"),
        "default_publishes should record topic in seen_topics for chain validation"
    );
}

#[test]
fn test_default_publishes_not_injected_when_events_written() {
    use std::collections::HashMap;
    use std::io::Write;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    let mut hats = HashMap::new();
    hats.insert(
        "test-hat".to_string(),
        crate::config::HatConfig {
            name: "test-hat".to_string(),
            description: Some("Test hat for default publishes".to_string()),
            triggers: vec!["task.start".to_string()],
            publishes: vec!["task.done".to_string()],
            instructions: "Test hat".to_string(),
            extra_instructions: vec![],
            backend_args: None,
            backend: None,
            default_publishes: Some("task.done".to_string()),
            max_activations: None,
            scratchpad: None,
            disallowed_tools: vec![],
            timeout: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            phase_triggers: None,
            ..Default::default()
        },
    );
    config.hats = hats;

    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    let _hat_id = HatId::new("test-hat");

    // Agent writes an event to the JSONL file
    let mut file = std::fs::File::create(&events_path).unwrap();
    writeln!(
        file,
        r#"{{"topic":"task.done","ts":"2024-01-01T00:00:00Z"}}"#
    )
    .unwrap();
    file.flush().unwrap();

    // process_events_from_jsonl reads them — caller should NOT call check_default_publishes
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(result.had_events, "Events should be found from JSONL");

    // Verify: even if someone mistakenly calls check_default_publishes, the
    // call site guards with `if !agent_wrote_events`, so defaults won't fire.
    // But we assert the guard condition here:
    assert!(
        result.had_events,
        "Caller should skip check_default_publishes when agent wrote events"
    );
}

#[test]
fn test_has_pending_plan_events_in_jsonl_peeks_without_consuming() {
    use std::io::Write;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut event_loop = EventLoop::new(RalphConfig::default());
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let mut file = std::fs::File::create(&events_path).unwrap();
    writeln!(
        file,
        r#"{{"topic":"plan.created","payload":"ready","ts":"2024-01-01T00:00:00Z"}}"#
    )
    .unwrap();
    file.flush().unwrap();

    assert!(
        event_loop
            .has_pending_plan_events_in_jsonl()
            .expect("peek should succeed"),
        "peek should report unread plan.* topics"
    );

    let processed = event_loop.process_events_from_jsonl().unwrap();
    assert!(processed.had_events);
    assert!(
        processed.had_plan_events,
        "processed metadata should preserve semantic plan.* detection"
    );
    assert!(
        processed.human_interact_context.is_none(),
        "plan-only batches should not synthesize human.interact metadata"
    );

    assert!(
        !event_loop
            .has_pending_plan_events_in_jsonl()
            .expect("peek after consume should succeed"),
        "peek should return false after unread events are consumed"
    );
}

#[test]
fn test_pending_human_interact_context_in_jsonl_peeks_without_consuming() {
    use std::io::Write;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut event_loop = EventLoop::new(RalphConfig::default());
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let mut file = std::fs::File::create(&events_path).unwrap();
    writeln!(
        file,
        r#"{{"topic":"human.interact","payload":"Need approval?","ts":"2024-01-01T00:00:00Z"}}"#
    )
    .unwrap();
    file.flush().unwrap();

    let pending_context = event_loop
        .pending_human_interact_context_in_jsonl()
        .expect("peek should succeed")
        .expect("peek should include pending human.interact context");
    assert_eq!(
        pending_context["question"],
        serde_json::json!("Need approval?")
    );
    assert!(
        pending_context.get("outcome").is_none(),
        "pre-dispatch context should not include outcome metadata"
    );

    let processed = event_loop.process_events_from_jsonl().unwrap();
    assert!(processed.had_events);
    let processed_context = processed
        .human_interact_context
        .expect("processed metadata should include human.interact context");
    assert_eq!(
        processed_context["question"],
        serde_json::json!("Need approval?")
    );
    assert_eq!(
        processed_context["outcome"],
        serde_json::json!("no_robot_service")
    );

    assert!(
        event_loop
            .pending_human_interact_context_in_jsonl()
            .expect("peek after consume should succeed")
            .is_none(),
        "peek should return no pending human.interact events after consume"
    );
}

#[test]
fn test_process_events_from_jsonl_reports_when_plan_topics_absent() {
    use std::io::Write;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut event_loop = EventLoop::new(RalphConfig::default());
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let mut file = std::fs::File::create(&events_path).unwrap();
    writeln!(
        file,
        r#"{{"topic":"task.start","payload":"start","ts":"2024-01-01T00:00:00Z"}}"#
    )
    .unwrap();
    file.flush().unwrap();

    let processed = event_loop.process_events_from_jsonl().unwrap();
    assert!(processed.had_events);
    assert!(
        !processed.had_plan_events,
        "semantic plan.* flag should remain false when no plan topics were published"
    );
    assert!(
        processed.human_interact_context.is_none(),
        "non-human batches should not expose human.interact metadata"
    );
}

#[test]
fn test_default_publishes_skipped_when_non_orphan_event_written() {
    use std::collections::HashMap;
    use std::io::Write;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    let mut hats = HashMap::new();
    // hat-a triggers on task.start → task.start is NOT an orphan
    hats.insert(
        "hat-a".to_string(),
        crate::config::HatConfig {
            name: "hat-a".to_string(),
            description: Some("Hat triggered by task.start".to_string()),
            triggers: vec!["task.start".to_string()],
            publishes: vec!["task.done".to_string()],
            instructions: "Do the task".to_string(),
            extra_instructions: vec![],
            backend_args: None,
            backend: None,
            default_publishes: Some("task.done".to_string()),
            max_activations: None,
            scratchpad: None,
            disallowed_tools: vec![],
            timeout: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            phase_triggers: None,
            ..Default::default()
        },
    );
    config.hats = hats;

    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    let hat_id = HatId::new("hat-a");

    // Consume the initial event from initialize so pending state starts clean
    let _ = event_loop.build_prompt(&hat_id);

    // Agent writes a non-orphan event (task.start → triggers hat-a)
    let mut file = std::fs::File::create(&events_path).unwrap();
    writeln!(
        file,
        r#"{{"topic":"task.start","ts":"2024-01-01T00:00:00Z","payload":"starting work"}}"#
    )
    .unwrap();
    file.flush().unwrap();

    // Process events — this is what the event loop calls
    let result = event_loop.process_events_from_jsonl().unwrap();

    // The caller in loop_runner.rs uses `had_events` to decide whether to inject defaults:
    //   let agent_wrote_events = result.had_events;
    //   if !agent_wrote_events { check_default_publishes(...) }
    //
    // Before the fix, the return was a single bool (= has_orphans). For a non-orphan
    // event like task.start, has_orphans=false, so the caller would see
    // agent_wrote_events=false and incorrectly inject default_publishes.
    assert!(
        result.had_events,
        "had_events must be true when agent wrote events (even non-orphan ones)"
    );
    // Also verify has_orphans is false — this was the old return value that got conflated
    assert!(
        !result.has_orphans,
        "has_orphans should be false for non-orphan events"
    );
}

#[test]
fn test_default_publishes_not_injected_when_not_configured() {
    use std::collections::HashMap;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    let mut hats = HashMap::new();
    hats.insert(
        "test-hat".to_string(),
        crate::config::HatConfig {
            name: "test-hat".to_string(),
            description: Some("Test hat for default publishes".to_string()),
            triggers: vec!["task.start".to_string()],
            publishes: vec!["task.done".to_string()],
            instructions: "Test hat".to_string(),
            extra_instructions: vec![],
            backend_args: None,
            backend: None,
            default_publishes: None, // No default configured
            max_activations: None,
            scratchpad: None,
            disallowed_tools: vec![],
            timeout: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            phase_triggers: None,
            ..Default::default()
        },
    );
    config.hats = hats;

    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    let hat_id = HatId::new("test-hat");

    // Consume the initial event from initialize
    let _ = event_loop.build_prompt(&hat_id);

    // Agent wrote no events
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(!result.had_events);

    // check_default_publishes should NOT inject since not configured
    event_loop.check_default_publishes(&hat_id);

    assert!(
        !event_loop.has_pending_events(),
        "No default should be injected"
    );
}
