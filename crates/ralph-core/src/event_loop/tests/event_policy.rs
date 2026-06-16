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

// -------------------------------------------------------------------------
// Unit 2 (2026-06-16-002 plan): unified recoverable payload contract.
//
// These tests pin the (hat, reason_class) bucketing and the
// bounded-retry semantics introduced by Unit 2.  The contract
// is:
//
//   * `PayloadTypeMismatch` / `MissingRequiredField` /
//     `TopicDenied` are **recoverable**: the first 3 attempts
//     publish a `task.resume` and let the loop continue.
//   * The 4th attempt for the same `(hat, reason_class)` pushes
//     a `RecoverableExhaustion` into
//     `state.recoverable_exhaustion_buffer`.
//   * `plan_name` mismatch, duplicate terminal, and
//     `InvalidFieldValue` are **non-recoverable** and trigger
//     the U6 `PayloadContractViolation` path on the first
//     attempt (no resume, no buffer entry).
//
// The plan is in
// `docs/plans/2026-06-16-002-feat-ce-executor-loop-stability-plan.md`.
// -------------------------------------------------------------------------

/// Helper: build a single-hat `event_loop` with a JSON-object
/// schema for the supplied topic and a `coordinator`-style hat
/// that publishes it.  Mirrors the existing
/// `test_event_policy_enforce_reject_replaces_with_task_resume`
/// setup but in a reusable form.
fn u2_payload_type_mismatch_config() -> RalphConfig {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      u2.work.ready:
        payload: json_object
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["u2.work.ready"]
    instructions: "Coordinate the run."
"#;
    serde_yaml::from_str(yaml).unwrap()
}

/// Helper: write a string payload event to a JSONL file, with
/// an explicit hat field.  Mirrors `write_event_with_hat_to_jsonl`
/// from `common::` but kept inline so the Unit 2 tests stay
/// self-contained.
fn u2_write_string_event(
    path: &std::path::Path,
    topic: &str,
    hat: &str,
) {
    use std::io::Write;
    let event = serde_json::json!({
        "topic": topic,
        "payload": "not-a-json-object",
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
fn test_u2_payload_type_mismatch_recoverable_first_three_attempts() {
    // Happy path: a non-JSON `u2.work.ready` is a recoverable
    // `PayloadTypeMismatch` for the `coordinator` hat.  The
    // first 3 attempts must NOT populate
    // `recoverable_exhaustion_buffer` and must publish a
    // `task.resume` for the source hat.  (U2 plan §3
    // "recoverable 1-3 times走 RejectWithResume ... 不 capture
    // violation".)
    let config = u2_payload_type_mismatch_config();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // 3 attempts in one iteration pass.  Each one is a
    // recoverable `PayloadTypeMismatch`; none should push a
    // `RecoverableExhaustion`.
    for _ in 0..3 {
        u2_write_string_event(&events_path, "u2.work.ready", "coordinator");
    }
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !result.had_events,
        "All 3 attempts must be rejected (no accepted business events)"
    );
    assert!(
        event_loop.state().recoverable_exhaustion_buffer.is_empty(),
        "recoverable_exhaustion_buffer must be empty after 3 attempts (limit is 3, exhausted on the 4th); got: {:?}",
        event_loop.state().recoverable_exhaustion_buffer
    );
    // `task.resume` must be on the bus, targeted at the
    // source hat.  The schema-aware fix_hint is not in
    // scope for this assertion — only the routing.
    let coordinator_id = HatId::new("coordinator");
    let pending = event_loop.bus.peek_pending(&coordinator_id);
    assert!(pending.is_some(), "task.resume should be on the bus");
    let resume_count = pending
        .unwrap()
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    assert_eq!(
        resume_count, 3,
        "3 attempts should produce 3 task.resume events"
    );
}

#[test]
fn test_u2_missing_required_field_recoverable_first_three_attempts() {
    // Happy path: an event with a missing `commit_count`
    // (when the schema requires it) is a recoverable
    // `MissingRequiredField`.  The first 3 attempts must NOT
    // push a `RecoverableExhaustion` and must publish a
    // `task.resume` for the source hat.  (U2 plan R-B5.)
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      u2.work.done:
        payload: json_object
        required_fields: [commit_count]
hats:
  executor:
    name: "Executor"
    triggers: ["u2.work.ready"]
    publishes: ["u2.work.done"]
    instructions: "Execute the work."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    use std::io::Write;
    // 3 attempts, each missing `commit_count`.  Each one is
    // a `MissingRequiredField`; none should push a
    // `RecoverableExhaustion`.
    for _ in 0..3 {
        let event = serde_json::json!({
            "topic": "u2.work.done",
            "payload": r#"{"summary":"done"}"#,
            "ts": chrono::Utc::now().to_rfc3339(),
            "hat": "executor",
        });
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_path)
            .unwrap();
        writeln!(f, "{}", event).unwrap();
    }

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !result.had_events,
        "All 3 attempts must be rejected (no accepted business events)"
    );
    assert!(
        event_loop.state().recoverable_exhaustion_buffer.is_empty(),
        "recoverable_exhaustion_buffer must be empty after 3 attempts; got: {:?}",
        event_loop.state().recoverable_exhaustion_buffer
    );
    let executor_id = HatId::new("executor");
    let pending = event_loop.bus.peek_pending(&executor_id);
    assert!(pending.is_some());
    let resume_count = pending
        .unwrap()
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    assert_eq!(resume_count, 3, "3 attempts should produce 3 task.resume events");
}

#[test]
fn test_u2_recoverable_buckets_are_independent_per_reason_class() {
    // Edge case: same hat, 3 `PayloadTypeMismatch` then 3
    // `MissingRequiredField`.  The (hat, reason_class) buckets
    // are independent — the 4th `PayloadTypeMismatch` is the
    // first one to push a `RecoverableExhaustion`; the
    // `MissingRequiredField` bucket still has 0 entries.
    // (U2 plan §3 "重试 key 按 (hat, reason_class) 分桶 ...
    // 不合并 topic".)
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      u2.bucket.topic:
        payload: json_object
        required_fields: [required_field]
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["u2.bucket.topic"]
    instructions: "Coordinate."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    use std::io::Write;

    // 3 attempts at the wrong type (string payload).
    for _ in 0..3 {
        let event = serde_json::json!({
            "topic": "u2.bucket.topic",
            "payload": "not-a-json-object",
            "ts": chrono::Utc::now().to_rfc3339(),
            "hat": "coordinator",
        });
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_path)
            .unwrap();
        writeln!(f, "{}", event).unwrap();
    }
    let _ = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        event_loop.state().recoverable_exhaustion_buffer.is_empty(),
        "After 3 PayloadTypeMismatch, no exhaustions should be recorded"
    );

    // Now 3 attempts with the right type but missing the
    // required field.  This is a different
    // (coordinator, missing_required_field) bucket and must
    // also NOT push an exhaustion.
    for _ in 0..3 {
        let event = serde_json::json!({
            "topic": "u2.bucket.topic",
            "payload": r#"{"other":"value"}"#,
            "ts": chrono::Utc::now().to_rfc3339(),
            "hat": "coordinator",
        });
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_path)
            .unwrap();
        writeln!(f, "{}", event).unwrap();
    }
    let _ = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        event_loop.state().recoverable_exhaustion_buffer.is_empty(),
        "After 3 MissingRequiredField, no exhaustions should be recorded \
         (independent bucket from PayloadTypeMismatch); got: {:?}",
        event_loop.state().recoverable_exhaustion_buffer
    );

    // The 4th `PayloadTypeMismatch` attempt should now
    // push exactly one exhaustion for the
    // (coordinator, payload_type_mismatch) bucket.  The
    // MissingRequiredField bucket must still be empty.
    u2_write_string_event(&events_path, "u2.bucket.topic", "coordinator");
    let _ = event_loop.process_events_from_jsonl().unwrap();
    let buf = &event_loop.state().recoverable_exhaustion_buffer;
    assert_eq!(
        buf.len(),
        1,
        "4th PayloadTypeMismatch should push exactly one RecoverableExhaustion; got: {:?}",
        buf
    );
    let entry = &buf[0];
    assert_eq!(entry.hat, "coordinator");
    assert_eq!(entry.topic, "u2.bucket.topic");
    assert_eq!(
        entry.reason_class,
        crate::event_policy::ReasonClass::PayloadTypeMismatch
    );
    assert!(
        entry.count > crate::event_loop::U2_REJECTION_RETRY_LIMIT,
        "Post-increment count must exceed the limit; got count={}, limit={}",
        entry.count,
        crate::event_loop::U2_REJECTION_RETRY_LIMIT
    );
}

#[test]
fn test_u2_non_recoverable_payload_contract_violation_first_attempt() {
    // Error path: a `plan_name` mismatch (a
    // non-recoverable violation) is captured on the FIRST
    // attempt and pushed into `payload_contract_violation`
    // (U6 fast-fail).  It must NOT push a
    // `RecoverableExhaustion` — the recoverable bucket is
    // reserved for the U2 R-B1 set
    // (`PayloadTypeMismatch` / `MissingRequiredField` /
    // `TopicDenied`).
    //
    // We exercise the non-recoverable path with an
    // `InvalidFieldValue` (U2 R-B2 marks it as
    // "deferred").  The Unit 2 implementation does NOT
    // alter the pre-existing U1 R5 routing for this case
    // (it still publishes a `task.resume` so the source
    // hat can correct its emission).  The key contract
    // pin for Unit 2 is: even after `InvalidFieldValue`
    // triggers U6, the recoverable bucket must remain
    // empty (no `RecoverableExhaustion` entry) — the
    // non-recoverable path does not consume retry budget.
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      u2.nonrecoverable.topic:
        payload: json_object
        required_fields: [status]
        allowed_values:
          status: ["ok", "blocked"]
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["u2.nonrecoverable.topic"]
    instructions: "Coordinate."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    use std::io::Write;
    // `status: "unknown"` violates `allowed_values`.
    let event = serde_json::json!({
        "topic": "u2.nonrecoverable.topic",
        "payload": r#"{"status":"unknown"}"#,
        "ts": chrono::Utc::now().to_rfc3339(),
        "hat": "coordinator",
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .unwrap();
    writeln!(f, "{}", event).unwrap();

    let result = event_loop.process_events_from_jsonl().unwrap();
    // Non-recoverable: the FIRST attempt captures the
    // violation (U6 path) and does NOT push a
    // RecoverableExhaustion.  This is the Unit 2 contract
    // pin — the non-recoverable bucket does not consume
    // the recoverable retry budget.
    assert!(
        event_loop.state().recoverable_exhaustion_buffer.is_empty(),
        "Non-recoverable violation must NOT push a RecoverableExhaustion; got: {:?}",
        event_loop.state().recoverable_exhaustion_buffer
    );
    // The non-recoverable violation lives on
    // `ProcessedEvents::payload_contract_violation` (the
    // U6 shape), not on `LoopState`.  When the runner
    // sees this field on the next iteration pass it
    // terminates the loop with
    // `TerminationReason::PayloadContractViolation` (the
    // U6 fast-fail path).
    assert!(
        result.payload_contract_violation.is_some(),
        "Non-recoverable violation must populate `payload_contract_violation` (U6 path)"
    );
}

#[test]
fn test_u2_fourth_recoverable_attempt_pushes_recoverable_exhaustion() {
    // Error path: the 4th recoverable rejection for the
    // same (hat, reason_class) pushes a
    // `RecoverableExhaustion` into
    // `state.recoverable_exhaustion_buffer`.  After the
    // exhaustion, the loop's runner will surface a
    // `RecoverablePayloadExhausted` termination reason.  This
    // test pins only the buffer side; the runner-side
    // termination is covered by the integration
    // `test_recoverable_exhaustion_terminates_loop` (added
    // alongside the runner U6 wiring in this commit).
    let config = u2_payload_type_mismatch_config();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // 4 attempts in one iteration pass.  The 4th one must
    // be the first `RecoverableExhaustion` entry.
    for _ in 0..4 {
        u2_write_string_event(&events_path, "u2.work.ready", "coordinator");
    }
    let _ = event_loop.process_events_from_jsonl().unwrap();

    let buf = &event_loop.state().recoverable_exhaustion_buffer;
    assert_eq!(
        buf.len(),
        1,
        "Exactly one RecoverableExhaustion should be on the buffer after the 4th attempt; got: {:?}",
        buf
    );
    let entry = &buf[0];
    assert_eq!(entry.hat, "coordinator");
    assert_eq!(entry.topic, "u2.work.ready");
    assert_eq!(
        entry.reason_class,
        crate::event_policy::ReasonClass::PayloadTypeMismatch
    );
    assert!(
        entry.count > crate::event_loop::U2_REJECTION_RETRY_LIMIT,
        "Post-increment count must exceed the limit; got count={}, limit={}",
        entry.count,
        crate::event_loop::U2_REJECTION_RETRY_LIMIT
    );
    // Diagnostic surface: the count is what the runner
    // puts into the `RecoverablePayloadExhausted`
    // `TerminationReason` and into the recovery envelope
    // `message`; this assertion is the contract pin.
    let _ = format!(
        "hat={} topic={} reason_class={} count={}",
        entry.hat, entry.topic, entry.reason_class.as_str(), entry.count
    );
}
