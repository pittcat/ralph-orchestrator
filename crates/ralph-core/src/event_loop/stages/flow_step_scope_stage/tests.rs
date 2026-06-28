use super::*;
use crate::event_loop::flow_declaration::FlowDeclaration;
use crate::event_loop::stage_pipeline::{
    EmitStage, FlowStep, RepairStateMachine, StageContext,
};
use ralph_proto::Event;

const FLOW_YAML: &str = r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        kind: foreach
        allowed_emits: [work.ready, work.done, work.failed]
        terminal_when: all_done
      - id: review_walk
        kind: sequence
        allowed_emits: [review.start, review.complete]
      - id: plan_end
        kind: branch
        allowed_emits: [plan.complete, plan.blocked]
        terminal_when: partial_units_done
        on_partial:
          all_done: plan.complete
          any_failed: plan.blocked(reason="unit_failed")
          partial_units_done: plan.blocked(reason="4_of_8_partial")
"#;

fn flow() -> FlowDeclaration {
    FlowDeclaration::from_yaml(FLOW_YAML).unwrap()
}

fn ctx_for(step_id: &str) -> StageContext<'static> {
    // U4 (2026-06-27-002 plan completion): real
    // `repair_flow::RepairStateMachine` (re-exported
    // as `stage_pipeline::RepairStateMachine`).
    let repair: &'static mut RepairStateMachine =
        Box::leak(Box::new(RepairStateMachine::default()));
    StageContext::for_test_machine(FlowStep::new(step_id), "loop-1", 1, repair)
}

fn ev(topic: &str, payload: &str) -> Event {
    Event::new(topic, payload)
}

#[test]
fn flow_step_scope_accepts_event_in_allowed_emits() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev("work.ready", "{}");
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn flow_step_scope_rejects_event_outside_allowed_emits() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev("plan.complete", "{}");
    let err = stage.check(&mut ctx_for("unit_loop"), &e).unwrap_err();
    assert_eq!(err.reason_code, "flow_unknown_emit");
}

#[test]
fn flow_step_scope_allows_terminal_topic_through_to_verdict_gate() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev("LOOP_COMPLETE", "{}");
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn flow_step_scope_accepts_partial_state_event_with_matching_reason() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev(
        "plan.blocked",
        r#"{"reason":"4_of_8_partial_continue_to_review"}"#,
    );
    assert!(stage.check(&mut ctx_for("plan_end"), &e).is_ok());
}

#[test]
fn flow_step_scope_rejects_partial_state_event_with_empty_reason() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev("plan.blocked", r#"{"reason":""}"#);
    let err = stage.check(&mut ctx_for("plan_end"), &e).unwrap_err();
    assert_eq!(err.reason_code, "flow_partial_state_undeclared");
}

#[test]
fn flow_step_scope_rejects_partial_state_event_with_wrong_reason_pattern() {
    let stage = FlowStepScopeStage::new(flow());
    // `partial_units_done` branch requires `partial` substring.
    let e = ev("plan.blocked", r#"{"reason":"i_give_up"}"#);
    let err = stage.check(&mut ctx_for("plan_end"), &e).unwrap_err();
    assert_eq!(err.reason_code, "reason_pattern_mismatch");
}

#[test]
fn flow_step_scope_skips_partial_check_for_non_plan_topics() {
    let stage = FlowStepScopeStage::new(flow());
    // `work.done` is in allowed_emits and the step has a
    // partial-state terminal_when — but work.done is not a
    // plan.* topic, so the partial pattern check does not
    // fire.
    let e = ev("work.done", r#"{"task_id":"abc"}"#);
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn flow_step_scope_rejects_check_when_step_id_not_in_flow() {
    // U11 (2026-06-27-002 plan completion) takes the
    // partial-fail-closed path: when the `current_step`
    // is not in the flow, the stage falls through to
    // the legacy fail-open behaviour (the
    // `flow_declaration_missing` lint is the operator
    // signal). The fail-closed check fires only when
    // the step IS declared but the topic is NOT in
    // its `allowed_emits` set; that contract is pinned
    // by `flow_step_scope_rejects_event_outside_allowed_emits`
    // (line 60-63). When a future plan iteration wants
    // the strict fail-closed behaviour (reject when
    // `current_step` is undeclared), this test must
    // be flipped alongside the `step = self.flow.step`
    // arm above.
    let stage = FlowStepScopeStage::new(flow());
    let e = ev("plan.complete", "{}");
    // P1-6 (2026-06-27 adversarial review): the
    // stage is now fail-closed for undeclared
    // steps — `ctx_for("does_not_exist")` is
    // not in the flow, so the stage returns
    // `flow_step_undeclared` and the event is
    // rejected. The legacy fail-open behaviour
    // is gone.
    let reject = stage
        .check(&mut ctx_for("does_not_exist"), &e)
        .expect_err("undeclared step must be rejected (P1-6)");
    assert_eq!(reject.reason_code, "flow_step_undeclared");
}

#[test]
fn reason_pattern_partial_units_done_requires_partial_substring() {
    assert!(reason_matches_partial_pattern(
        "partial_units_done",
        "4_of_8_partial"
    ));
    assert!(reason_matches_partial_pattern(
        "partial_units_done",
        "PARTIAL_UNITS_DONE"
    ));
    assert!(!reason_matches_partial_pattern(
        "partial_units_done",
        "all_done"
    ));
}

#[test]
fn reason_pattern_any_failed_requires_unit_failed_or_any_failed() {
    assert!(reason_matches_partial_pattern("any_failed", "unit_failed"));
    assert!(reason_matches_partial_pattern("any_failed", "any_failed"));
    assert!(!reason_matches_partial_pattern("any_failed", "partial"));
}

#[test]
fn extract_reason_returns_empty_string_for_null() {
    let r = extract_reason(r#"{"reason":null}"#);
    assert_eq!(r, Some(String::new()));
}

#[test]
fn extract_reason_returns_none_for_non_object() {
    assert_eq!(extract_reason("\"hi\""), None);
    assert_eq!(extract_reason("not-json"), None);
}

// ── 2026-06-28 plan U3: defensive bypass for review-chain hats ─────

fn ev_with_source(topic: &str, payload: &str, source: &str) -> Event {
    Event::new(topic, payload).with_source(source)
}

#[test]
fn u3_bypass_accepts_coordinator_review_start() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev_with_source("review.start", "{}", "coordinator");
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn u3_bypass_accepts_coordinator_plan_complete() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev_with_source("plan.complete", "{}", "coordinator");
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn u3_bypass_accepts_review_coordinator_dimensions_complete() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev_with_source(
        "review.dimensions.complete",
        "{}",
        "review-coordinator",
    );
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn u3_bypass_accepts_review_synthesizer_review_complete() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev_with_source("review.complete", "{}", "review-synthesizer");
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn u3_bypass_accepts_dimension_reviewer_dimension_done() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev_with_source("review.dimension.done", "{}", "dimension-reviewer");
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn u3_bypass_accepts_shipper_review_complete() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev_with_source("REVIEW_COMPLETE", "{}", "shipper");
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn u3_bypass_accepts_ralph_plan_blocked() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev_with_source("plan.blocked", r#"{"reason":"x"}"#, "ralph");
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn u3_bypass_accepts_ralph_loop_complete() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev_with_source("LOOP_COMPLETE", r#"{"success":false}"#, "ralph");
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn u3_bypass_accepts_coordinator_loop_complete() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev_with_source("LOOP_COMPLETE", r#"{"success":false}"#, "coordinator");
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn u3_bypass_rejects_unrelated_hat() {
    // executor emitting review.* — bypass list does NOT include
    // executor, so the topic falls through to the legacy
    // flow_unknown_emit reject path.
    let stage = FlowStepScopeStage::new(flow());
    let e = ev_with_source("review.dimensions.complete", "{}", "executor");
    let err = stage.check(&mut ctx_for("unit_loop"), &e).unwrap_err();
    assert_eq!(err.reason_code, "flow_unknown_emit");
}

#[test]
fn u3_bypass_rejects_wrong_topic_for_bypassed_hat() {
    // dimension-reviewer is on the bypass for review.dimension.done
    // but emitting `task.resume` from `unit_loop` is NOT on the
    // bypass list, AND `task.resume` is not in unit_loop's
    // allowed_emits — so the legacy reject path fires.
    let stage = FlowStepScopeStage::new(flow());
    let e = ev_with_source("task.resume", "{}", "dimension-reviewer");
    let err = stage.check(&mut ctx_for("unit_loop"), &e).unwrap_err();
    assert_eq!(err.reason_code, "flow_unknown_emit");
}

#[test]
fn u3_bypass_requires_source_hat() {
    // No source hat — the bypass cannot match, and `unit_loop`
    // does not allow `review.dimensions.complete`, so the
    // legacy reject path fires.
    let stage = FlowStepScopeStage::new(flow());
    let e = ev("review.dimensions.complete", "{}");
    let err = stage.check(&mut ctx_for("unit_loop"), &e).unwrap_err();
    assert_eq!(err.reason_code, "flow_unknown_emit");
}