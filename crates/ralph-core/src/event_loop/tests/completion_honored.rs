//! Tests for completion_honored.

use super::common::*;
use super::*;

#[test]
fn test_check_completion_event_is_idempotent() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let ralph_id = HatId::new("ralph");

    // Set up completion request
    event_loop.state.completion_requested = true;

    // First call should succeed and set completion_honored
    let result1 = event_loop.check_completion_event();
    assert_eq!(result1, Some(TerminationReason::CompletionPromise));
    assert!(event_loop.state.completion_honored);

    // Capture bus state after first call
    let pending_after_first = event_loop
        .bus
        .peek_pending(&ralph_id)
        .map(|v| v.len())
        .unwrap_or(0);

    // Second call should return the same conclusion without side effects
    let result2 = event_loop.check_completion_event();
    assert_eq!(result2, Some(TerminationReason::CompletionPromise));

    // Bus event count should not have increased
    let pending_after_second = event_loop
        .bus
        .peek_pending(&ralph_id)
        .map(|v| v.len())
        .unwrap_or(0);
    assert_eq!(
        pending_after_second, pending_after_first,
        "Second check_completion_event call should not publish extra bus events"
    );
}

#[test]
fn test_request_completion_fallback_ignored_when_already_handled() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    // Simulate that completion was already handled
    event_loop.state.completion_honored = true;
    event_loop.state.completion_requested = false;

    // Text fallback request should be ignored
    event_loop.request_completion_from_text_fallback();
    assert!(
        !event_loop.state.completion_requested,
        "completion_requested should remain false when completion is already handled"
    );
}

#[test]
fn test_parsed_completion_event_ignored_when_already_handled() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Simulate that completion was already handled
    event_loop.state.completion_honored = true;
    event_loop.state.completion_requested = false;

    // Write a completion event to JSONL
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");

    // Process events — the parsed completion event should be ignored
    let _ = event_loop.process_events_from_jsonl();
    assert!(
        !event_loop.state.completion_requested,
        "Parsed completion event should be ignored when completion_honored is true"
    );
}

#[test]
fn test_completion_honored_same_batch_duplicate_terminal_does_not_route() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: ignore
      business_after_completion: ignore
      write_diagnostic_event: false
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Batch: LOOP_COMPLETE, LOOP_COMPLETE
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Retry");

    let result = event_loop.process_events_from_jsonl().unwrap();
    // First LOOP_COMPLETE sets completion_requested, second is filtered by
    // same-batch completion guard (ignore) and does not produce had_events.
    assert!(
        event_loop.state.completion_requested,
        "First LOOP_COMPLETE should request completion"
    );

    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Completion should be accepted"
    );

    // No business events should have been published
    assert!(
        !result.had_plan_events,
        "Duplicate terminal should not produce plan events"
    );
}

#[test]
fn test_completion_honored_same_batch_business_event_does_not_route() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: ignore
      business_after_completion: ignore
      write_diagnostic_event: false
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Batch: LOOP_COMPLETE, experiment.planned
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    write_event_to_jsonl(&events_path, "experiment.planned", "{\"task_key\":\"a\"}");

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        event_loop.state.completion_requested,
        "LOOP_COMPLETE should request completion"
    );
    // experiment.planned is filtered by same-batch completion guard
    assert!(
        !result.had_plan_events,
        "Business event after completion in same batch should not trigger plan events"
    );
    assert!(
        !result.had_events,
        "Business event after completion in same batch should not produce any events"
    );

    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Completion should be accepted"
    );

    // Strategist should not have pending events from experiment.planned
    let strategist_pending = event_loop.bus.peek_pending(&HatId::new("strategist"));
    assert!(
        strategist_pending.map(|v| v.is_empty()).unwrap_or(true),
        "Business event after completion should not trigger strategist"
    );
}

#[test]
fn test_completion_honored_next_iteration_business_event_blocked() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: ignore
      business_after_completion: ignore
      write_diagnostic_event: false
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // First iteration: LOOP_COMPLETE
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, Some(TerminationReason::CompletionPromise));
    assert!(event_loop.state.completion_honored);

    // Second iteration: experiment.planned after completion honored
    // Need to reset event reader position and write new events
    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(&events_path2, "experiment.planned", "{\"task_key\":\"b\"}");

    let result = event_loop.process_events_from_jsonl().unwrap();
    // Business event should be blocked by completion-honored guard
    assert!(
        !result.had_events,
        "Business event after completion honored should not be published"
    );

    let strategist_pending = event_loop.bus.peek_pending(&HatId::new("strategist"));
    assert!(
        strategist_pending.map(|v| v.is_empty()).unwrap_or(true),
        "Business event after completion honored should not trigger strategist"
    );
}

#[test]
fn test_completion_honored_next_iteration_duplicate_terminal_blocked() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: ignore
      business_after_completion: ignore
      write_diagnostic_event: false
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // First iteration: LOOP_COMPLETE
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, Some(TerminationReason::CompletionPromise));
    assert!(event_loop.state.completion_honored);

    // Second iteration: duplicate LOOP_COMPLETE after completion honored
    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(&events_path2, "LOOP_COMPLETE", "Retry");

    let result = event_loop.process_events_from_jsonl().unwrap();
    // Duplicate terminal should be blocked by completion-honored guard
    assert!(
        !result.had_events,
        "Duplicate terminal after completion honored should not be published"
    );
    assert!(
        !result.had_plan_events,
        "Duplicate terminal after completion honored should not trigger plan events"
    );

    let strategist_pending = event_loop.bus.peek_pending(&HatId::new("strategist"));
    assert!(
        strategist_pending.map(|v| v.is_empty()).unwrap_or(true),
        "Duplicate terminal after completion honored should not trigger strategist"
    );
}

#[test]
fn test_completion_honored_loop_exit_reason_is_completion() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: ignore
      business_after_completion: ignore
      write_diagnostic_event: false
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Accept completion
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason1 = event_loop.check_completion_event();
    assert_eq!(reason1, Some(TerminationReason::CompletionPromise));

    // Even if more events arrive after completion is honored, termination reason
    // should still be CompletionPromise.
    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(&events_path2, "experiment.planned", "{\"task_key\":\"c\"}");
    let _ = event_loop.process_events_from_jsonl();

    let reason2 = event_loop.check_completion_event();
    assert_eq!(
        reason2,
        Some(TerminationReason::CompletionPromise),
        "Termination reason must remain CompletionPromise after honored completion"
    );
}

#[test]
fn test_completion_honored_old_config_without_completion_guard_behaves_as_before() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // Config with event_policy enabled but NO completion_after_terminal specified
    // (uses defaults: warn/warn/false)
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(&mut event_loop, &["LOOP_COMPLETE", "experiment.planned"]);

    // Old behavior: LOOP_COMPLETE should still work normally
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Old config should still accept completion normally"
    );

    // Old config default is Warn: business events after completion should still route
    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(
        &events_path2,
        "experiment.planned",
        "{\"task_key\":\"post-completion\"}",
    );

    let result = event_loop.process_events_from_jsonl().unwrap();
    eprintln!(
        "DEBUG: had_events={}, had_plan_events={}, completion_honored={}",
        result.had_events, result.had_plan_events, event_loop.state.completion_honored
    );
    eprintln!(
        "DEBUG: bus hats: {:?}",
        event_loop.bus.hat_ids().collect::<Vec<_>>()
    );
    assert!(
        result.had_events,
        "Old config with Warn default should allow business events after completion"
    );

    let strategist_pending = event_loop.bus.peek_pending(&HatId::new("strategist"));
    eprintln!("DEBUG: strategist_pending={:?}", strategist_pending);
    assert!(
        strategist_pending.map(|v| !v.is_empty()).unwrap_or(false),
        "Old config should trigger strategist for business events after completion"
    );
}

#[test]
fn test_completion_honored_reject_action_blocks_events() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: reject
      business_after_completion: reject
      write_diagnostic_event: true
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    use std::sync::Mutex;
    // Capture diagnostic events via observer
    let captured_events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = captured_events.clone();
    event_loop.bus.add_observer(move |event: &Event| {
        captured.lock().unwrap().push(Event {
            topic: event.topic.clone(),
            payload: event.payload.clone(),
            source: event.source.clone(),
            target: event.target.clone(),
            wave_id: event.wave_id.clone(),
            wave_index: event.wave_index,
            wave_total: event.wave_total,
            system_injected: event.system_injected,
        });
    });

    // First accept completion
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let _ = event_loop.check_completion_event();
    assert!(event_loop.state.completion_honored);

    // Next batch: business event after completion
    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(&events_path2, "experiment.planned", "{\"task_key\":\"d\"}");

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !result.had_events,
        "Reject action should block business event after completion"
    );

    // Diagnostic event should be published when write_diagnostic_event is true
    let has_diagnostic = captured_events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e.topic.as_str() == "event.completion.blocked");
    assert!(
        has_diagnostic,
        "Reject action with write_diagnostic_event=true should publish event.completion.blocked"
    );
}

#[test]
fn test_completion_honored_warn_action_allows_event_with_warning() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: warn
      business_after_completion: warn
      write_diagnostic_event: false
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(&mut event_loop, &["LOOP_COMPLETE", "experiment.planned"]);

    // First accept completion
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let _ = event_loop.check_completion_event();
    assert!(event_loop.state.completion_honored);

    // Next batch: business event after completion with warn action
    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(&events_path2, "experiment.planned", "{\"task_key\":\"e\"}");

    let result = event_loop.process_events_from_jsonl().unwrap();
    // Warn action allows the event through (with a warning diagnostic)
    assert!(
        result.had_events,
        "Warn action should allow business event after completion"
    );
}
