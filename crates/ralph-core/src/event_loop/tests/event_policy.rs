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

// -------------------------------------------------------------------------
// U1 (2026-06-11-002): trivial_step semantic gate — integration tests
//
// These exercise the full event-loop partition path: a review.passed
// event with skip_reason=trivial_step AND a non-trivial diff is fed
// through the loop and the loop must (a) NOT put the event on the bus,
// (b) publish a task.resume targeting the source hat with a recovery
// payload that names the observed changed_lines / findings_count.
// -------------------------------------------------------------------------

fn u1_review_passed_config() -> RalphConfig {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    trivial_step_max_changed_lines: 50
    schemas:
      review.passed:
        payload: json_object
        required_fields: [plan_name, task_id, task_key, step, findings_count, fix_round, verdict, skip_reason]
        allowed_values:
          skip_reason: ["empty_diff", "trivial_step", "aggregate_timeout"]
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.wave.ready"]
    publishes: ["review.passed", "review.complete", "review.failed"]
    instructions: "Review code."
"#;
    serde_yaml::from_str(yaml).unwrap()
}

fn write_review_passed_event(
    path: &std::path::Path,
    hat: &str,
    skip_reason: &str,
    findings_count: u64,
    changed_lines: u64,
) {
    use std::io::Write;
    let event = serde_json::json!({
        "topic": "review.passed",
        "payload": format!(
            r#"{{"plan_name":"plan-x","task_id":"t1","task_key":"k1","step":"step-01","findings_count":{},"fix_round":0,"verdict":"pass","skip_reason":"{}","changed_lines":{}}}"#,
            findings_count, skip_reason, changed_lines
        ),
        "ts": chrono::Utc::now().to_rfc3339(),
        "hat": hat,
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(f, "{}", event).unwrap();
}

#[test]
fn test_u1_trivial_step_bypass_rejected_with_task_resume() {
    // U1: review.passed with skip_reason=trivial_step + non-trivial
    // diff is rejected and replaced with task.resume targeting the
    // source hat.
    let config = u1_review_passed_config();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_review_passed_event(&events_path, "reviewer", "trivial_step", 5, 80);

    let result = event_loop.process_events_from_jsonl().unwrap();

    // Rejected → validated_events is empty → no business progress
    assert!(
        !result.had_events,
        "trivial_step bypass with non-trivial diff should be rejected, got had_events=true"
    );

    // R5 (2026-06-14-003 plan): task.resume is targeted at the
    // source hat (`reviewer`), not `ralph` — the source hat is
    // the one that needs to fix the bypass attempt.  Pre-R5 the
    // resume was published without a target so it fell through
    // to `ralph`; the test previously asserted that path.  R5
    // makes the routing explicit and load-bearing.
    let reviewer_id = HatId::new("reviewer");
    let pending = event_loop.bus.peek_pending(&reviewer_id);
    assert!(
        pending.is_some(),
        "task.resume should be on the bus for reviewer"
    );
    let events = pending.unwrap();
    let resume = events
        .iter()
        .find(|e| e.topic.as_str() == "task.resume")
        .expect("task.resume must be present for U1 violation");
    assert_eq!(
        resume.target.as_ref().map(|h| h.as_str()),
        Some("reviewer"),
        "R5 must route task.resume to the source hat"
    );
    let payload = &resume.payload;
    assert!(
        payload.contains("invalid_trivial_step_bypass"),
        "recovery payload must name the reason code, got: {payload}"
    );
    assert!(
        payload.contains("findings_count=5"),
        "recovery payload must include observed findings_count, got: {payload}"
    );
    assert!(
        payload.contains("changed_lines=80"),
        "recovery payload must include observed changed_lines, got: {payload}"
    );

    // The bad event itself is NOT on the bus
    assert!(
        !events.iter().any(|e| e.topic.as_str() == "review.passed"),
        "rejected review.passed must not be on the bus"
    );
}

#[test]
fn test_u1_trivial_step_legitimate_passes_through() {
    // U1 negative case: legitimate trivial step (small diff + 0
    // findings) goes all the way through to the bus.
    let config = u1_review_passed_config();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_review_passed_event(&events_path, "reviewer", "trivial_step", 0, 5);

    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        result.had_events,
        "legitimate trivial_step (small diff + 0 findings) must be accepted"
    );

    // The event must reach the synthesizer (next hat in the
    // review-synthesizer chain). The test loop is a minimal preset
    // with no synthesizer; the ralph hat is the loop runner's
    // catch-all. The key assertion: the event was NOT rejected.
    let ralph_id = HatId::new("ralph");
    let pending = event_loop.bus.peek_pending(&ralph_id);
    if let Some(events) = pending {
        assert!(
            !events.iter().any(|e| e.topic.as_str() == "task.resume"),
            "no task.resume should be emitted for legitimate trivial_step"
        );
    }
}

#[test]
fn test_u1_trivial_step_gate_disabled_by_zero_threshold() {
    // U1 escape hatch: trivial_step_max_changed_lines=0 disables the
    // gate (kept for operators who want to opt out). Verify the
    // configuration is honored: a review.passed event with
    // skip_reason=trivial_step AND a non-trivial diff is accepted
    // by the policy layer (no task.resume generated by U1).
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    trivial_step_max_changed_lines: 0
    schemas:
      review.passed:
        payload: json_object
        required_fields: [plan_name, task_id, task_key, step, findings_count, fix_round, verdict, skip_reason]
        allowed_values:
          skip_reason: ["empty_diff", "trivial_step", "aggregate_timeout"]
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.wave.ready"]
    publishes: ["review.passed", "review.complete", "review.failed"]
    instructions: "Review code."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Pathological case: trivial_step + 1000 findings + 5000 lines.
    // With the gate disabled, this must still pass through with no
    // task.resume being generated by the U1 gate.
    write_review_passed_event(&events_path, "reviewer", "trivial_step", 1000, 5000);

    let _ = event_loop.process_events_from_jsonl().unwrap();

    let ralph_id = HatId::new("ralph");
    if let Some(events) = event_loop.bus.peek_pending(&ralph_id) {
        let u1_resume = events
            .iter()
            .filter(|e| e.topic.as_str() == "task.resume")
            .find(|e| e.payload.contains("invalid_trivial_step_bypass"));
        assert!(
            u1_resume.is_none(),
            "U1 gate must be silent when trivial_step_max_changed_lines=0; found: {:?}",
            u1_resume.map(|e| &e.payload)
        );
    }
}
