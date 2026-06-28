use super::*;
use crate::event_loop::stage_pipeline::{
    EmitStage, FlowStep, RepairStateMachine, StageContext,
};
use ralph_proto::Event;

fn ctx() -> StageContext<'static> {
    // U4 (2026-06-27-002 plan completion): the
    // `RepairStateMachine` is now a real
    // `repair_flow::RepairStateMachine` (re-exported
    // from `stage_pipeline`). The lifetime trick with
    // `Box::leak` keeps the borrow checker happy
    // without changing the public API.
    let repair: &'static mut RepairStateMachine = Box::leak(Box::new(RepairStateMachine::default()));
    StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 1, repair)
}

fn ev(topic: &str, payload: &str) -> Event {
    Event::new(topic, payload)
}

#[test]
fn emit_schema_gate_stage_accepts_event_with_all_required_fields() {
    let stage = EmitSchemaGateStage::with_defaults();
    let e = ev("plan.blocked", r#"{"reason":"unit_failed"}"#);
    assert!(stage.check(&mut ctx(), &e).is_ok());
}

#[test]
fn emit_schema_gate_stage_accepts_plan_blocked_with_empty_reason() {
    // The schema gate is *type* level, not value level. An
    // empty `reason` is still a string, so it is present.
    // Empty values are caught by U9's reason-pattern check
    // (and by the operator reading the plan output).
    let stage = EmitSchemaGateStage::with_defaults();
    let e = ev("plan.blocked", r#"{"reason":""}"#);
    assert!(stage.check(&mut ctx(), &e).is_ok());
}

#[test]
fn emit_schema_gate_stage_rejects_plan_blocked_with_null_reason() {
    let stage = EmitSchemaGateStage::with_defaults();
    let e = ev("plan.blocked", r#"{"reason":null}"#);
    let err = stage.check(&mut ctx(), &e).unwrap_err();
    assert_eq!(err.reason_code, "missing_required_fields");
    assert_eq!(err.missing_fields, vec!["reason".to_string()]);
}

#[test]
fn emit_schema_gate_stage_rejects_task_resume_with_missing_kind() {
    let stage = EmitSchemaGateStage::with_defaults();
    let e = ev(
        "task.resume",
        r#"{"reason":"retry","target_hat":"coordinator"}"#,
    );
    let err = stage.check(&mut ctx(), &e).unwrap_err();
    assert_eq!(err.missing_fields, vec!["kind".to_string()]);
}

#[test]
fn emit_schema_gate_stage_rejects_malformed_json_payload() {
    let stage = EmitSchemaGateStage::with_defaults();
    let e = ev("plan.blocked", "not json");
    let err = stage.check(&mut ctx(), &e).unwrap_err();
    assert_eq!(err.reason_code, "missing_required_fields");
    assert_eq!(err.missing_fields, vec!["reason".to_string()]);
}

#[test]
fn emit_schema_gate_stage_accepts_topics_outside_schema() {
    let stage = EmitSchemaGateStage::with_defaults();
    // `work.ready` is not in the default schema — the gate
    // must not block it just because the operator forgot to
    // declare a schema.
    let e = ev("work.ready", "{}");
    assert!(stage.check(&mut ctx(), &e).is_ok());
}

#[test]
fn required_fields_from_loop_config_gates_preset_review_complete() {
    use crate::config::RalphConfig;
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    schemas:
      review.complete:
        required_fields: [fix_plan_file, verdict, plan_name]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let required = super::required_fields_from_loop_config(&config.event_loop);
    let stage = EmitSchemaGateStage::new(required);

    let err = stage
        .check(&mut ctx(), &ev("review.complete", "{}"))
        .unwrap_err();
    assert_eq!(err.reason_code, "missing_required_fields");
    assert!(err.missing_fields.contains(&"fix_plan_file".to_string()));
    assert!(err.missing_fields.contains(&"verdict".to_string()));

    // Baseline topics remain gated after the merge.
    let err = stage
        .check(&mut ctx(), &ev("plan.blocked", r#"{"reason":null}"#))
        .unwrap_err();
    assert_eq!(err.reason_code, "missing_required_fields");
}

#[test]
fn emit_schema_gate_stage_custom_required_overrides_defaults() {
    let mut required = HashMap::new();
    required.insert("custom.event".to_string(), vec!["x".to_string()]);
    let stage = EmitSchemaGateStage::new(required);

    // Default-schema topics are no longer gated because we
    // built the stage from a custom table.
    let e = ev("plan.blocked", "{}");
    assert!(stage.check(&mut ctx(), &e).is_ok(), "custom stage must not fall back to defaults");

    // Custom event with missing field is gated.
    let e = ev("custom.event", "{}");
    let err = stage.check(&mut ctx(), &e).unwrap_err();
    assert_eq!(err.missing_fields, vec!["x".to_string()]);
}