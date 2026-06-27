use super::*;
use crate::event_loop::stage_pipeline::{
    EmitStage, FlowStep, RepairStateMachine, StageContext,
};

fn ctx() -> StageContext<'static> {
    let repair: &'static RepairStateMachine = Box::leak(Box::new(RepairStateMachine));
    StageContext::new(FlowStep::new("unit_loop"), "loop-1", 1, repair)
}

fn ev(topic: &str, payload: &str) -> ralph_proto::Event {
    ralph_proto::Event::new(topic, payload)
}

#[test]
fn repair_dispatch_recognises_all_repair_topics() {
    for topic in REPAIR_TOPICS {
        assert!(is_repair_topic(topic), "{topic} must be a repair topic");
    }
}

#[test]
fn repair_dispatch_does_not_recognise_normal_topics() {
    assert!(!is_repair_topic("work.ready"));
    assert!(!is_repair_topic("plan.blocked"));
    assert!(!is_repair_topic("review.start"));
    assert!(!is_repair_topic(""));
}

#[test]
fn repair_dispatch_stage_accepts_repair_events_without_error() {
    let stage = RepairDispatchStage;
    let e = ev("task.relocate_legacy", r#"{"task_key":"abc","target_loop_id":"loop-x","reason":"legacy"}"#);
    assert!(stage.check(&ctx(), &e).is_ok(), "repair events must not be rejected by the stage");
}

#[test]
fn repair_dispatch_stage_accepts_non_repair_events() {
    let stage = RepairDispatchStage;
    let e = ev("work.ready", "{}");
    assert!(stage.check(&ctx(), &e).is_ok(), "non-repair events must pass through");
}

#[test]
fn extract_task_key_reads_task_key_field() {
    let e = ev("task.relocate_legacy", r#"{"task_key":"abc","other":1}"#);
    assert_eq!(extract_task_key(&e).as_deref(), Some("abc"));
}

#[test]
fn extract_task_key_returns_none_when_missing() {
    let e = ev("task.relocate_legacy", r#"{"other":1}"#);
    assert_eq!(extract_task_key(&e), None);
}

#[test]
fn extract_task_key_returns_none_on_malformed_json() {
    let e = ev("task.relocate_legacy", "not-json");
    assert_eq!(extract_task_key(&e), None);
}

#[test]
fn extract_task_key_returns_none_on_non_string_value() {
    let e = ev("task.relocate_legacy", r#"{"task_key":42}"#);
    assert_eq!(extract_task_key(&e), None);
}