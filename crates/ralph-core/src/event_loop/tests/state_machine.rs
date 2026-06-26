//! Tests for state_machine.

use super::common::*;
use super::*;

#[test]
fn test_state_machine_terminal_rejected_by_open_tasks_does_not_honor_terminal() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  required_events: [never.seen]
  state_machine:
    enabled: true
    instance_key:
      from_payload: task_key
      required_for: [experiment.planned]
    terminal_topics: [LOOP_COMPLETE]
    business_topics: [experiment.planned]
    terminal_guard:
      require_no_open_instances: false
    transitions:
      - topic: experiment.planned
        from: [idle]
        to: planned
        opens_instance: true
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.planned"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();

    assert_eq!(reason, None);
    assert!(
        !event_loop
            .state
            .state_machine_runtime_state
            .as_ref()
            .unwrap()
            .is_terminal_honored(),
        "state machine terminal should not be honored until loop completion is honored"
    );

    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(&events_path2, "experiment.planned", r#"{"task_key":"t1"}"#);
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        result.had_events,
        "business event should still be accepted after completion was rejected by open runtime tasks"
    );
}

#[test]
fn test_state_machine_branch_close_runs_before_workflow_guard() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.evaluated
        correlation:
          from_payload: task_key
  state_machine:
    enabled: true
    instance_key:
      from_payload: task_key
      required_for: [experiment.planned, experiment.blocked]
    terminal_topics: [LOOP_COMPLETE]
    business_topics: [experiment.planned, experiment.blocked]
    transitions:
      - topic: experiment.planned
        from: [idle]
        to: planned
        opens_instance: true
      - topic: experiment.blocked
        from: [planned]
        to: blocked
        closes_instance: true
hats:
  observer:
    name: "Observer"
    triggers: ["experiment.blocked"]
    publishes: ["LOOP_COMPLETE"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "experiment.planned", r#"{"task_key":"t1"}"#);
    write_event_to_jsonl(&events_path, "experiment.blocked", r#"{"task_key":"t1"}"#);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        result.had_events,
        "state machine branch-close event should be accepted instead of rejected by linear workflow guards"
    );
    assert!(
        event_loop
            .state
            .state_machine_runtime_state
            .as_ref()
            .unwrap()
            .closed_instances_snapshot()
            .contains_key("t1"),
        "blocked should close the instance"
    );
}

#[test]
fn test_state_machine_processed_events_reports_only_accepted_events() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  state_machine:
    enabled: true
    instance_key:
      from_payload: task_key
      required_for: [experiment.planned, experiment.ready, experiment.blocked]
    terminal_topics: [LOOP_COMPLETE]
    business_topics: [experiment.planned, experiment.ready, experiment.blocked]
    transitions:
      - topic: experiment.planned
        from: [idle]
        to: planned
        opens_instance: true
      - topic: experiment.blocked
        from: [planned]
        to: blocked
        closes_instance: true
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "experiment.ready", r#"{"task_key":"bad"}"#);
    write_event_to_jsonl(&events_path, "experiment.planned", r#"{"task_key":"t1"}"#);
    write_event_to_jsonl(&events_path, "experiment.blocked", r#"{"task_key":"t1"}"#);
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");

    let result = event_loop.process_events_from_jsonl().unwrap();
    let topics: Vec<_> = result
        .accepted_events
        .iter()
        .map(|event| event.topic.as_str())
        .collect();

    assert_eq!(
        topics,
        vec!["experiment.planned", "experiment.blocked", "LOOP_COMPLETE"],
        "accepted event summary should omit rejected candidates and include accepted terminal"
    );
}

#[test]
fn test_verdict_gate_rejects_loop_complete_when_payload_is_fail() {
    use crate::config::VerdictGateConfig;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut config = RalphConfig::default();
    config.event_loop.verdict_gate = Some(VerdictGateConfig {
        topic: "review.complete".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
        additional_topics: Vec::new(),
    verdict_field: None,
    residual_count_field: None,
    });
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(
        &events_path,
        "review.complete",
        r#"{"pass_or_fail":"fail","verdict":"fail","final_findings_count":2}"#,
    );
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "verdict gate should reject LOOP_COMPLETE when review.complete carries pass_or_fail=fail"
    );
    // P0-2 (2026-06-23-003 plan): completion rejection now routes
    // through the deterministic-correction path. The rejection
    // signal lives in `state.prompt_context.correction_blocks`
    // (rendered into the next prompt as `## ORCHESTRATOR CORRECTION`)
    // instead of being published on the EventBus as `task.resume`.
    assert!(
        !event_loop
            .state()
            .prompt_context
            .correction_blocks
            .is_empty(),
        "completion rejection must inject a CorrectionContext into state.prompt_context (P0-2)"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// 2026-06-26 plan U5: typed Verdict path — `verdict_field` configured
// opts the gate into the typed Pass / PassWithResiduals / Fail model.
// `pass_with_residuals` is no longer mis-classified as fail.
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn test_typed_verdict_pass_with_residuals_below_threshold_passes() {
    use crate::config::VerdictGateConfig;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut config = RalphConfig::default();
    config.event_loop.verdict_gate = Some(VerdictGateConfig {
        topic: "review.complete".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
        additional_topics: Vec::new(),
        // Opt into the typed Verdict path.
        verdict_field: Some("verdict".to_string()),
        residual_count_field: Some("final_findings_count".to_string()),
    });
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // `pass_with_residuals` with 5 findings (max=8) must be
    // promoted to Pass — the gate MUST NOT reject LOOP_COMPLETE.
    // This is the regression test for the binary-match bug.
    write_event_to_jsonl(
        &events_path,
        "review.complete",
        r#"{"verdict":"pass_with_residuals","final_findings_count":5}"#,
    );
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");

    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "typed verdict `pass_with_residuals` (5 <= max=8) must promote to Pass; got {reason:?}"
    );
}

#[test]
fn test_typed_verdict_pass_with_residuals_above_threshold_fails() {
    use crate::config::VerdictGateConfig;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut config = RalphConfig::default();
    config.event_loop.verdict_gate = Some(VerdictGateConfig {
        topic: "review.complete".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
        additional_topics: Vec::new(),
        verdict_field: Some("verdict".to_string()),
        residual_count_field: Some("final_findings_count".to_string()),
    });
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // 12 findings > max=8 must downgrade to Fail and the gate
    // must reject LOOP_COMPLETE (reason is `None` while the
    // gate is open).
    write_event_to_jsonl(
        &events_path,
        "review.complete",
        r#"{"verdict":"pass_with_residuals","final_findings_count":12}"#,
    );
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");

    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "typed verdict `pass_with_residuals` (12 > max=8) must downgrade to Fail; gate must reject (reason=None), got {reason:?}"
    );
    assert!(
        !event_loop
            .state()
            .prompt_context
            .correction_blocks
            .is_empty(),
        "completion rejection must inject a CorrectionContext"
    );
}

#[test]
fn test_typed_verdict_explicit_fail_rejects() {
    use crate::config::VerdictGateConfig;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut config = RalphConfig::default();
    config.event_loop.verdict_gate = Some(VerdictGateConfig {
        topic: "review.complete".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
        additional_topics: Vec::new(),
        verdict_field: Some("verdict".to_string()),
        residual_count_field: Some("final_findings_count".to_string()),
    });
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(
        &events_path,
        "review.complete",
        r#"{"verdict":"fail","reason":"tests broke"}"#,
    );
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");

    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "typed verdict `fail` must reject LOOP_COMPLETE (reason=None); got {reason:?}"
    );
}

#[test]
fn test_legacy_binary_match_preserved_when_verdict_field_none() {
    // Pre-U5 behaviour: when `verdict_field` is `None`, the gate
    // keeps the binary `pass_or_fail == fail` match. The new
    // `verdict` field is ignored. This test guards against
    // accidentally flipping every preset into the typed path.
    use crate::config::VerdictGateConfig;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut config = RalphConfig::default();
    config.event_loop.verdict_gate = Some(VerdictGateConfig {
        topic: "review.complete".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
        additional_topics: Vec::new(),
        verdict_field: None,
        residual_count_field: None,
    });
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Payload has `verdict=pass_with_residuals` AND
    // `pass_or_fail=pass` — the legacy binary path must look at
    // `pass_or_fail` only and accept.
    write_event_to_jsonl(
        &events_path,
        "review.complete",
        r#"{"pass_or_fail":"pass","verdict":"pass_with_residuals","final_findings_count":12}"#,
    );
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");

    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "legacy binary match (pass_or_fail=pass) must accept; got {reason:?}"
    );
}

#[test]
fn test_verdict_gate_accepts_loop_complete_when_payload_is_pass() {
    use crate::config::VerdictGateConfig;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut config = RalphConfig::default();
    config.event_loop.verdict_gate = Some(VerdictGateConfig {
        topic: "review.complete".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
        additional_topics: Vec::new(),
    verdict_field: None,
    residual_count_field: None,
    });
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(
        &events_path,
        "review.complete",
        r#"{"pass_or_fail":"pass","verdict":"pass_with_residuals","final_findings_count":2}"#,
    );
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "verdict gate should accept LOOP_COMPLETE when review.complete carries pass_or_fail=pass"
    );
}

#[test]
fn test_no_verdict_gate_config_preserves_completion_behavior() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let config = RalphConfig::default();
    assert!(
        config.event_loop.verdict_gate.is_none(),
        "verdict_gate must default to None for backward compatibility"
    );
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Even if a review.complete event with pass_or_fail=fail is published, no verdict_gate
    // means the loop ignores it and accepts LOOP_COMPLETE (backward-compatible default).
    write_event_to_jsonl(
        &events_path,
        "review.complete",
        r#"{"pass_or_fail":"fail"}"#,
    );
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "without verdict_gate configured, LOOP_COMPLETE should be honored as before"
    );
}

/// Verifies the ce-executor-serial gate: REVIEW_COMPLETE + additional_topics: ["report.done"].
/// When `report.done` carries pass_or_fail="fail", the verdict gate must block LOOP_COMPLETE
/// even if the upstream REVIEW_COMPLETE topic itself would match.
#[test]
fn test_verdict_gate_additional_topic_blocks_loop_complete_on_fail() {
    use crate::config::VerdictGateConfig;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut config = RalphConfig::default();
    // Mirror ce-executor-serial preset: REVIEW_COMPLETE is upstream,
    // report.done is the final downstream mirror.
    config.event_loop.verdict_gate = Some(VerdictGateConfig {
        topic: "REVIEW_COMPLETE".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
        additional_topics: vec!["report.done".to_string()],
    verdict_field: None,
    residual_count_field: None,
    });
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Upstream REVIEW_COMPLETE first (establishes propagation chain).
    write_event_to_jsonl(
        &events_path,
        "REVIEW_COMPLETE",
        r#"{"pass_or_fail":"fail","verdict":"fail","final_findings_count":2}"#,
    );
    // Final mirror: report.done with pass_or_fail=fail.
    write_event_to_jsonl(
        &events_path,
        "report.done",
        r#"{"pass_or_fail":"fail","verdict":"fail"}"#,
    );
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");

    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "verdict gate should reject LOOP_COMPLETE when report.done carries pass_or_fail=fail"
    );
    // P0-2 (2026-06-23-003 plan): completion rejection now routes
    // through the deterministic-correction path. The rejection
    // signal lives in `state.prompt_context.correction_blocks`
    // (rendered into the next prompt as `## ORCHESTRATOR CORRECTION`)
    // instead of being published on the EventBus as `task.resume`.
    assert!(
        !event_loop
            .state()
            .prompt_context
            .correction_blocks
            .is_empty(),
        "completion rejection must inject a CorrectionContext into state.prompt_context (P0-2)"
    );
}

