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
        event_loop.state().seen_topics.contains("queue.advance"),
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
    write_event_with_hat_to_jsonl(&events_path, "work.done", r#"{"ok":true}"#, "plan-gate");

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
fn test_isolated_mode_drops_queue_advance_after_work_ready() {
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
        "Second business event (queue.advance) should be dropped — ordered pair required"
    );
}

#[test]
fn test_isolated_mode_drops_third_event_after_dual_publish_pair() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  plan-gate:
    name: "Plan Gate"
    description: "Advances plan steps"
    triggers: ["work.failed"]
    publishes: ["queue.advance", "work.ready", "work.done"]
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
    write_event_with_hat_to_jsonl(&events_path, "work.done", r#"{"ok":true}"#, "plan-gate");

    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let _ = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        event_loop.state().seen_topics.contains("queue.advance"),
        "queue.advance (first) should be accepted"
    );
    assert!(
        event_loop.state().seen_topics.contains("work.ready"),
        "work.ready (second in pair) should be accepted"
    );
    assert!(
        !event_loop.state().seen_topics.contains("work.done"),
        "Third business event (work.done) must be dropped — budget is exactly 2 events"
    );
}

#[test]
fn test_isolated_mode_dual_publish_not_cross_hat() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  coordinator:
    name: "Coordinator"
    description: "Coordinates"
    triggers: ["task.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    description: "Executes"
    triggers: ["work.ready"]
    publishes: ["queue.advance", "work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    // Establish an isolated turn so the per-turn business-event budget is enforced.
    // The active hat is arbitrary for this test; event.hat is authoritative for scope.
    event_loop.process_output(&HatId::new("coordinator"), "output", true);

    // Simulate: executor emits queue.advance first, then coordinator emits work.ready
    // in the same turn. The coordinator's work.ready should NOT be豁免 by executor's
    // queue.advance (different hat), so it gets dropped as the second business event.
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // executor: queue.advance first (business event #1 — accepted)
    write_event_with_hat_to_jsonl(&events_path, "queue.advance", "step-02", "executor");
    // coordinator: work.ready second (business event #2 — should be dropped,
    // not豁免 by executor's queue.advance which is a different hat)
    write_event_with_hat_to_jsonl(
        &events_path,
        "work.ready",
        r#"{"task_id":"task-real-001"}"#,
        "coordinator",
    );

    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let _ = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        event_loop.state().seen_topics.contains("queue.advance"),
        "queue.advance from executor (first business event) should be accepted"
    );
    assert!(
        !event_loop.state().seen_topics.contains("work.ready"),
        "work.ready from coordinator must be dropped — different hat does not豁免"
    );
}

#[test]
fn test_isolated_dual_publish_handoff_required_event_to_completion() {
    let yaml = r#"
event_loop:
  completion_promise: LOOP_COMPLETE
  required_events: ["report.done", "align.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    let mk = |topic: &str, hat: &str| crate::event_reader::Event {
        topic: topic.to_string(),
        payload: None,
        ts: "t".to_string(),
        hat: Some(hat.to_string()),
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };

    let report_pair = vec![mk("report.done", "reporter")];
    assert!(
        event_loop.isolated_dual_publish_handoff(
            "LOOP_COMPLETE",
            "reporter",
            "reporter",
            &report_pair
        ),
        "report.done → LOOP_COMPLETE handoff must be allowed"
    );

    let align_pair = vec![mk("align.done", "alignment")];
    assert!(
        event_loop.isolated_dual_publish_handoff(
            "LOOP_COMPLETE",
            "alignment",
            "alignment",
            &align_pair
        ),
        "align.done → LOOP_COMPLETE handoff must be allowed"
    );

    let wrong_hat = vec![mk("report.done", "reporter")];
    assert!(
        !event_loop.isolated_dual_publish_handoff("LOOP_COMPLETE", "ralph", "reporter", &wrong_hat),
        "cross-hat handoff must be rejected"
    );

    // BDD scenarios often omit `hat` on JSONL lines; both sides inherit
    // `isolated_hat` and the queue.advance → work.ready pair must still work.
    let no_hat_pair = vec![crate::event_reader::Event {
        topic: "queue.advance".to_string(),
        payload: None,
        ts: "t".to_string(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    }];
    assert!(
        event_loop.isolated_dual_publish_handoff(
            "work.ready",
            "plan-gate",
            "plan-gate",
            &no_hat_pair
        ),
        "queue.advance → work.ready handoff must work without hat provenance"
    );
}

#[test]
fn test_isolated_required_event_then_completion_same_turn_report_done() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
  required_events: ["report.done"]
hats:
  reporter:
    name: "Reporter"
    description: "Final report"
    triggers: ["align.done"]
    publishes: ["report.done", "LOOP_COMPLETE"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.process_output(&HatId::new("reporter"), "output", true);
    event_loop.state.current_isolated_hat = Some(HatId::new("reporter"));

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    write_event_with_hat_to_jsonl(
        &events_path,
        "report.done",
        r#"{"verdict":"pass"}"#,
        "reporter",
    );
    write_event_with_hat_to_jsonl(
        &events_path,
        "LOOP_COMPLETE",
        r#"{"reason":"done"}"#,
        "reporter",
    );

    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();
    let accepted_topics: Vec<_> = result
        .accepted_events
        .iter()
        .map(|e| e.topic.as_str())
        .collect();

    assert!(
        event_loop.state().seen_topics.contains("report.done"),
        "report.done must survive isolated per-turn budgeting"
    );
    assert!(
        accepted_topics.contains(&"LOOP_COMPLETE"),
        "LOOP_COMPLETE must be admitted as required-event handoff; got {accepted_topics:?}"
    );
    assert!(
        event_loop.state.completion_requested,
        "same-turn required event + completion must set completion_requested"
    );
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "completion must be honored once required event was observed"
    );
}

#[test]
fn test_isolated_required_event_then_completion_same_turn_generic_topic() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
  required_events: ["align.done"]
hats:
  alignment:
    name: "Alignment"
    description: "Align residuals"
    triggers: ["fix.done"]
    publishes: ["align.done", "LOOP_COMPLETE"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.process_output(&HatId::new("alignment"), "output", true);
    event_loop.state.current_isolated_hat = Some(HatId::new("alignment"));

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    write_event_with_hat_to_jsonl(
        &events_path,
        "align.done",
        r#"{"residuals_count":0}"#,
        "alignment",
    );
    write_event_with_hat_to_jsonl(
        &events_path,
        "LOOP_COMPLETE",
        r#"{"reason":"done"}"#,
        "alignment",
    );

    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();
    let accepted_topics: Vec<_> = result
        .accepted_events
        .iter()
        .map(|e| e.topic.as_str())
        .collect();

    assert!(event_loop.state().seen_topics.contains("align.done"));
    assert!(
        accepted_topics.contains(&"LOOP_COMPLETE"),
        "LOOP_COMPLETE handoff must succeed for generic required topic; got {accepted_topics:?}"
    );
    assert!(event_loop.state.completion_requested);
    assert_eq!(
        event_loop.check_completion_event(),
        Some(TerminationReason::CompletionPromise)
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
    assert!(event.is_some(), "task.done event should exist on bus");
    let payload = &event.unwrap().payload;
    assert!(
        payload.contains("status") && payload.contains("ok"),
        "Object payload should be converted to JSON string, got: {}",
        payload
    );
}

// required-event-to-completion 合法配对的反例覆盖：第二个事件必须是配置的
// completion_promise，第一个事件必须属于 required_events，且两个事件必须
// 来自当前 isolated hat；第三个业务事件仍会被丢弃。

fn required_completion_event_loop() -> EventLoop {
    let config: RalphConfig = serde_yaml::from_str(
        r#"
event_loop:
  completion_promise: LOOP_COMPLETE
  required_events: ["report.done"]
"#,
    )
    .unwrap();
    EventLoop::new(config)
}

fn event_from_hat(topic: &str, hat: &str) -> crate::event_reader::Event {
    crate::event_reader::Event {
        topic: topic.to_string(),
        payload: None,
        ts: "t".to_string(),
        hat: Some(hat.to_string()),
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    }
}

#[test]
fn test_isolated_required_to_completion_pair_rejects_non_required_first_topic() {
    let event_loop = required_completion_event_loop();
    let accepted = vec![event_from_hat("work.done", "reporter")];
    assert!(
        !event_loop.isolated_dual_publish_handoff(
            "LOOP_COMPLETE",
            "reporter",
            "reporter",
            &accepted
        ),
        "non-required first topic must not qualify as handoff to completion"
    );
}

#[test]
fn test_isolated_required_to_completion_pair_rejects_wrong_completion_text() {
    let event_loop = required_completion_event_loop();
    let accepted = vec![event_from_hat("report.done", "reporter")];
    assert!(
        !event_loop.isolated_dual_publish_handoff(
            "LOOP_CANCELED",
            "reporter",
            "reporter",
            &accepted
        ),
        "second topic must be exactly the configured completion_promise"
    );

    assert!(
        !event_loop.isolated_dual_publish_handoff("align.done", "reporter", "reporter", &accepted),
        "second topic other than completion_promise cannot ride the handoff"
    );
}

#[test]
fn test_isolated_required_to_completion_pair_rejects_cross_hat_handoff() {
    let event_loop = required_completion_event_loop();
    let accepted = vec![event_from_hat("report.done", "reporter")];
    assert!(
        !event_loop.isolated_dual_publish_handoff(
            "LOOP_COMPLETE",
            "alignment",
            "reporter",
            &accepted
        ),
        "cross-hat handoff must be rejected even when pair shape is correct"
    );
}
