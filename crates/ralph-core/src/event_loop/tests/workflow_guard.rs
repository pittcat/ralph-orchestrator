//! Tests for workflow_guard.

use super::common::*;
use super::*;

#[test]
fn test_workflow_guard_rejects_evaluated_before_scored() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // Configure workflow guard for AutoResearch experiment chain
    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Chain: planned -> ready -> measured
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Now try to skip scoring and go directly to evaluated - should be rejected
    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // evaluated should NOT be recorded as seen in workflow progress
    // Get phase AFTER processing to avoid borrow conflict
    // No correlation config → global instance (None key)
    let phase = event_loop
        .state
        .workflow_progress
        .get_phase("experiment", None);
    assert_eq!(
        phase,
        Some(2), // Still at measured (phase 2), not advanced to evaluated (phase 4)
        "experiment.evaluated before experiment.scored should not advance workflow"
    );
}

#[test]
fn test_workflow_guard_accepts_evaluated_after_scored() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Full chain in order
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.scored",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // After scoring, evaluated should advance the workflow
    // No correlation config → global instance (None key)
    let progress = &event_loop.state.workflow_progress;
    assert_eq!(
        progress.get_phase("experiment", None),
        Some(4), // Reached evaluated (phase 4)
        "experiment.evaluated after experiment.scored should advance workflow"
    );
}

#[test]
fn test_workflow_guard_periodic_review_does_not_advance_chain() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Chain: planned -> ready -> measured
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Interleave periodic.review - this should NOT advance the experiment chain
    write_event_to_jsonl(&events_path, "periodic.review", r#"{"status": "progress"}"#);
    let _ = event_loop.process_events_from_jsonl();

    // Now try to evaluate before scoring - still rejected
    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // periodic.review is not in the experiment chain, so workflow should still be at measured
    // evaluated was rejected because scored was never emitted
    // Get phase AFTER processing to avoid borrow conflict
    // No correlation config → global instance (None key)
    let phase = event_loop
        .state
        .workflow_progress
        .get_phase("experiment", None);
    assert_eq!(
        phase,
        Some(2), // Still at measured (phase 2) - evaluated was rejected
        "evaluated should still be rejected after periodic.review"
    );
}

#[test]
fn test_workflow_guard_completion_rejected_when_chain_incomplete() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Chain: planned -> ready -> measured (missing scored and evaluated)
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Try LOOP_COMPLETE before chain is complete
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();

    assert_eq!(
        reason, None,
        "LOOP_COMPLETE should be rejected when experiment chain is incomplete"
    );
}

#[test]
fn test_workflow_guard_completion_accepted_when_chain_complete() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Complete chain: planned -> ready -> measured -> scored -> evaluated
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.scored",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // LOOP_COMPLETE should now be accepted
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();

    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "LOOP_COMPLETE should be accepted when experiment chain is complete"
    );
}

#[test]
fn test_workflow_guard_instance_isolation_two_experiments() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
        correlation:
          from_payload: experiment_id
          from_topic: experiment.planned
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Experiment 1: fully complete
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.scored",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Experiment 2: only at measured
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-2"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-2"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-2"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    let progress = &event_loop.state.workflow_progress;

    // exp-1 should be at phase 4 (complete)
    assert_eq!(
        progress.get_phase("experiment", Some("exp-1")),
        Some(4),
        "exp-1 should be complete"
    );

    // exp-2 should be at phase 2 (measured)
    assert_eq!(
        progress.get_phase("experiment", Some("exp-2")),
        Some(2),
        "exp-2 should be at measured"
    );

    // Cannot evaluate exp-2 yet (needs scored first)
    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-2"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Get phase AFTER processing to verify evaluated was rejected
    let exp2_phase = event_loop
        .state
        .workflow_progress
        .get_phase("experiment", Some("exp-2"));
    assert_eq!(
        exp2_phase,
        Some(2), // Still at measured - evaluated was rejected until exp-2 is scored
        "exp-2 evaluated should be rejected until exp-2 is scored"
    );
}

#[test]
fn test_workflow_guard_rejection_publishes_task_resume_with_context() {
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Capture all published events via observer
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
        });
    });

    // Advance to phase 2 (measured)
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Now try to skip scoring and go directly to evaluated - should be rejected
    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Verify that a task.resume event was published with actionable context
    let events = captured_events.lock().unwrap();
    let resume_events: Vec<&Event> = events
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .collect();

    assert!(
        !resume_events.is_empty(),
        "A task.resume event should be published when a workflow guard rejects an event"
    );

    let last_resume = resume_events.last().unwrap();
    assert!(
        last_resume.payload.contains("WORKFLOW_GUARD_REJECTED"),
        "task.resume payload should indicate workflow guard rejection"
    );
    assert!(
        last_resume.payload.contains("next expected="),
        "task.resume payload should contain actionable next-expected context"
    );
    assert!(
        last_resume.payload.contains("experiment.scored"),
        "task.resume payload should mention the next expected topic"
    );
}

#[test]
fn test_workflow_guard_advisory_mode_accepts_out_of_order() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: advisory
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Skip ahead to evaluated without scoring — advisory should accept it
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // evaluated should be recorded as seen (in seen_topics)
    assert!(
        event_loop
            .state
            .seen_topics
            .contains("experiment.evaluated"),
        "Advisory mode should accept out-of-order events and record them as seen"
    );

    // Workflow progress should NOT advance for the skipped phase (advisory only advances valid phases)
    let phase = event_loop
        .state
        .workflow_progress
        .get_phase("experiment", None);
    assert_eq!(
        phase,
        Some(0),
        "Advisory mode should not advance progress for out-of-order events"
    );

    // LOOP_COMPLETE should NOT be blocked by advisory chains
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "LOOP_COMPLETE should be accepted when only advisory chains are incomplete"
    );
}

#[test]
fn test_workflow_guard_correlation_extraction_failure_rejects_event() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.scored
          - experiment.evaluated
        mode: strict
        correlation:
          from_payload: experiment_id
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Count task.resume events via lightweight observer
    let resume_count = Arc::new(AtomicUsize::new(0));
    let count = resume_count.clone();
    event_loop.bus.add_observer(move |event: &Event| {
        if event.topic.as_str() == "task.resume" {
            count.fetch_add(1, Ordering::SeqCst);
        }
    });

    // Valid first event with correlation key
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", Some("exp-1")),
        Some(0)
    );

    // Event with malformed JSON payload should be rejected
    write_event_to_jsonl(&events_path, "experiment.scored", r"not-json-at-all");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", Some("exp-1")),
        Some(0),
        "Event with malformed JSON should not advance workflow"
    );

    // Event with missing correlation key should be rejected
    write_event_to_jsonl(
        &events_path,
        "experiment.scored",
        r#"{"other_field": "value"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", Some("exp-1")),
        Some(0),
        "Event with missing correlation key should not advance workflow"
    );

    // Event with non-string correlation value should be rejected
    write_event_to_jsonl(
        &events_path,
        "experiment.scored",
        r#"{"experiment_id": 123}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", Some("exp-1")),
        Some(0),
        "Event with non-string correlation value should not advance workflow"
    );

    // Verify recovery events were published for each rejection
    let observed = resume_count.load(Ordering::SeqCst);
    assert!(
        observed >= 3,
        "Should have published task.resume for each correlation extraction failure, got {}",
        observed
    );

    // Finally, a valid event should be accepted and allow recovery
    write_event_to_jsonl(
        &events_path,
        "experiment.scored",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", Some("exp-1")),
        Some(1),
        "Valid event after rejections should advance workflow"
    );
}

#[test]
fn test_workflow_guard_recovery_after_rejection() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Advance normally to measured
    write_event_to_jsonl(&events_path, "experiment.planned", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(&events_path, "experiment.ready", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(&events_path, "experiment.measured", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(2)
    );

    // Try to skip scoring — rejected
    write_event_to_jsonl(&events_path, "experiment.evaluated", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(2),
        "Progress should remain at measured after rejected evaluated"
    );
    assert!(
        !event_loop
            .state
            .seen_topics
            .contains("experiment.evaluated"),
        "Rejected event should not be recorded as seen"
    );

    // Recovery: emit the correct next event (scored)
    write_event_to_jsonl(&events_path, "experiment.scored", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(3),
        "Scored should advance progress after recovery"
    );

    // Now evaluated should be accepted
    write_event_to_jsonl(&events_path, "experiment.evaluated", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(4),
        "Evaluated should be accepted after scoring"
    );

    // LOOP_COMPLETE should now succeed
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "LOOP_COMPLETE should succeed after recovery and full chain"
    );
}

#[test]
fn test_workflow_guard_rejects_old_phase_after_advance() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Advance to ready (phase 1)
    write_event_to_jsonl(&events_path, "experiment.planned", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(&events_path, "experiment.ready", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(1)
    );

    // Re-emit planned (phase 0) — should be accepted idempotently, no regression
    write_event_to_jsonl(&events_path, "experiment.planned", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(1),
        "Re-emitting old phase should not regress progress"
    );
}

// ──────────────────────────────────────────────────────────────────────
// 2026-06-04 plan U4: workflow guard envelope wiring
// ──────────────────────────────────────────────────────────────────────
//
// These tests pin the existing workflow-guard behavior (so a future
// refactor of `apply_workflow_guard_validation` cannot silently break
// the rejection contract) and assert the new recovery envelope write
// that U4 introduces.

#[test]
fn test_workflow_guard_rejection_writes_recovery_envelope() {
    // U4: an out-of-order workflow event rejected by the guard must
    //   (a) not enter the accepted events list (existing behavior);
    //   (b) not advance workflow progress (existing behavior);
    //   (c) emit a `task.resume` recovery event (existing behavior);
    //   (d) NEW: write a `RecoveryJournalEntry` to `recovery.jsonl`
    //       with `source = WorkflowGuard` and the next expected
    //       topic as evidence.
    use crate::diagnosis::{DiagnosisSource, EvidenceKind};
    use ralph_proto::Event;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.scored
        mode: strict
";
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Count task.resume events via lightweight observer to assert the
    // existing recovery signal is still published.
    let resume_count = Arc::new(AtomicUsize::new(0));
    let count = resume_count.clone();
    event_loop.bus.add_observer(move |event: &Event| {
        if event.topic.as_str() == "task.resume" {
            count.fetch_add(1, Ordering::SeqCst);
        }
    });

    // Phase 0 (planned) is allowed.
    write_event_to_jsonl(&events_path, "experiment.planned", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    // Skip ready and try scored — strict guard rejects.
    write_event_to_jsonl(&events_path, "experiment.scored", r"{}");
    let _ = event_loop.process_events_from_jsonl();

    // Characterization: rejected event did NOT advance progress
    // and did NOT enter the bus as accepted.
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(0),
        "out-of-order scored must not advance workflow progress"
    );
    assert!(
        !event_loop.state.seen_topics.contains("experiment.scored"),
        "rejected event must not be recorded as seen"
    );
    // Characterization: task.resume was published.
    assert!(
        resume_count.load(Ordering::SeqCst) >= 1,
        "rejection must publish a task.resume recovery event"
    );

    // U4: a recovery journal entry was written.
    let mut session_dirs: Vec<_> = std::fs::read_dir(diagnostics_root.join(".ralph/diagnostics"))
        .expect("read diagnostics dir")
        .filter_map(Result::ok)
        .collect();
    session_dirs.sort_by_key(|entry| entry.path());
    let session_path = session_dirs
        .last()
        .expect("at least one diagnostics session")
        .path();
    let recovery_path = session_path.join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
    let entries: Vec<crate::diagnosis::RecoveryJournalEntry> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse recovery entry"))
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one recovery entry, got: {:?}",
        entries
            .iter()
            .map(|e| &e.envelope.reason_code)
            .collect::<Vec<_>>()
    );
    let env = &entries[0].envelope;
    assert_eq!(env.source, DiagnosisSource::WorkflowGuard);
    assert_eq!(env.reason_code, "out_of_order_phase");
    assert!(
        !env.safe_target,
        "workflow guard has no registered safe target"
    );
    assert_eq!(env.topic.as_deref(), Some("experiment.scored"));
    assert!(
        env.evidence
            .iter()
            .any(|e| e.kind == EvidenceKind::Topic && e.ref_path == "experiment.ready"),
        "evidence must carry the next expected topic"
    );
}

#[test]
fn test_workflow_guard_recovery_publishes_recovery_diagnosis_audit() {
    // U4: in addition to the journal entry, the rejection writes a
    // high-level `OrchestrationEvent::RecoveryDiagnosed` audit line
    // to `orchestration.jsonl` so the audit timeline can show the
    // failure without re-parsing the journal.
    use crate::diagnosis::DiagnosisSource;
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.scored
        mode: strict
";
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Advance to phase 0, then attempt an out-of-order event.
    write_event_to_jsonl(&events_path, "experiment.planned", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(&events_path, "experiment.scored", r"{}");
    let _ = event_loop.process_events_from_jsonl();

    // Find the recovery.jsonl and orchestration.jsonl entries.
    let mut session_dirs: Vec<_> = std::fs::read_dir(diagnostics_root.join(".ralph/diagnostics"))
        .expect("read diagnostics dir")
        .filter_map(Result::ok)
        .collect();
    session_dirs.sort_by_key(|entry| entry.path());
    let session_path = session_dirs.last().unwrap().path();
    let recovery_path = session_path.join("recovery.jsonl");
    let orch_path = session_path.join("orchestration.jsonl");
    let recovery_content = std::fs::read_to_string(&recovery_path).unwrap();
    let recovery_entry: crate::diagnosis::RecoveryJournalEntry = recovery_content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .next()
        .map(|line| serde_json::from_str(line).unwrap())
        .expect("expected a recovery journal entry");
    assert_eq!(
        recovery_entry.envelope.source,
        DiagnosisSource::WorkflowGuard
    );

    let orch_content = std::fs::read_to_string(&orch_path).unwrap();
    assert!(
        orch_content.contains("\"type\":\"recovery_diagnosed\""),
        "orchestration.jsonl must include a RecoveryDiagnosed audit line for workflow guard rejections"
    );
    assert!(
        orch_content.contains(&recovery_entry.envelope.diagnosis_id),
        "orchestration.jsonl audit must reference the envelope's diagnosis_id"
    );
}
