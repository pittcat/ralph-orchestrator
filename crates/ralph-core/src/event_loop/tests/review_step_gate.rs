//! Integration tests for review step gate + Session B policy violations.

use super::common::*;
use super::*;

#[test]
fn session_b_fixture_lines_rejected_by_policy() {
    use crate::config::{
        EventPolicyConfig, EventPolicyMode, EventSchema, PayloadType, ViolationAction,
    };
    use crate::event_policy::{PolicyDecision, PolicyRuntimeState, validate_event};
    use std::collections::HashMap;
    use std::io::Read;

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/policy_schemas/ce_executor_session_b_policy_violations.jsonl"
    );
    let mut file = std::fs::File::open(path).expect("fixture must exist");
    let mut content = String::new();
    file.read_to_string(&mut content).unwrap();

    let mut config = EventPolicyConfig {
        enabled: true,
        mode: EventPolicyMode::Enforce,
        on_violation: ViolationAction::RejectWithResume,
        schemas: HashMap::new(),
        ..Default::default()
    };
    config.schemas.insert(
        "review.passed".to_string(),
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![
                "plan_name".into(),
                "task_id".into(),
                "task_key".into(),
                "step".into(),
                "findings_count".into(),
                "fix_round".into(),
                "verdict".into(),
                "skip_reason".into(),
            ],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
        },
    );
    config.schemas.insert(
        "review.failed".to_string(),
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![
                "plan_name".into(),
                "fix_round".into(),
                "safe_auto_count".into(),
                "gated_manual_count".into(),
                "findings_summary".into(),
                "task_id".into(),
                "task_key".into(),
                "step".into(),
            ],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
        },
    );

    let mut state = PolicyRuntimeState::default();
    for line in content.lines().filter(|l| !l.trim().is_empty()) {
        let event: crate::event_reader::Event = serde_json::from_str(line).unwrap();
        let decision = validate_event(&event.topic, event.payload.as_deref(), &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "fixture line for {} must be rejected, got {:?}",
            event.topic,
            decision
        );
    }
}

#[test]
fn plan_complete_rejected_without_prior_synth_terminal() {
    let yaml = r#"
hats:
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["review.dimension.done"]
    publishes: ["review.passed", "review.failed", "review.complete"]
    instructions: "SYNTH"
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed", "review.complete"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    instructions: "GATE"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      review.passed:
        payload: json_object
        required_fields: [plan_name, task_id, task_key, step, findings_count, fix_round, verdict, skip_reason]
      plan.complete:
        payload: json_object
        required_fields: [plan_name, completed_steps, task_id, task_key, verdict]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    write_event_to_jsonl(
        &events_path,
        "plan.complete",
        r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","verdict":"pass"}"#,
    );
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(result.had_rejected_events);
    assert!(result.accepted_events.is_empty());
}

#[test]
fn legal_synth_passed_then_plan_complete_accepted() {
    let yaml = r#"
hats:
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["review.dimension.done"]
    publishes: ["review.passed", "review.failed", "review.complete"]
    instructions: "SYNTH"
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed", "review.complete"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    instructions: "GATE"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      review.passed:
        payload: json_object
        required_fields: [plan_name, task_id, task_key, step, findings_count, fix_round, verdict, skip_reason]
      plan.complete:
        payload: json_object
        required_fields: [plan_name, completed_steps, task_id, task_key, verdict]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    write_event_to_jsonl(
        &events_path,
        "review.passed",
        r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
    );
    write_event_to_jsonl(
        &events_path,
        "plan.complete",
        r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","verdict":"pass"}"#,
    );
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(result.had_events);
    assert_eq!(result.accepted_events.len(), 2);
}
