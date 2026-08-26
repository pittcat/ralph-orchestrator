use super::*;
use crate::event_loop::flow_declaration::FlowDeclaration;
use crate::event_loop::stage_pipeline::{EmitStage, FlowStep, RepairStateMachine, StageContext};
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
fn flow_step_scope_allows_builtin_task_resume_without_preset_declaration() {
    // `task.resume` is runtime recovery transport, not a business emit that
    // each preset must repeat in its step allowlist.
    let stage = FlowStepScopeStage::new(flow());
    let e = ev(
        "task.resume",
        r#"{"reason":"rejected wave boundary","target_hat":"executor","kind":"correction"}"#,
    );
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
    let e = ev_with_source("review.dimensions.complete", "{}", "review-coordinator");
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

// ── 2026-06-29-007 plan U2: transition-topic bypass for review-chain
// aggregate events when source_hat is missing or the step has not
// advanced yet.

#[test]
fn transition_topic_accepts_dimensions_complete_without_source_at_unit_loop() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev("review.dimensions.complete", "{}");
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn transition_topic_accepts_dimensions_complete_with_empty_source_at_unit_loop() {
    let stage = FlowStepScopeStage::new(flow());
    let e = Event::new("review.dimensions.complete", "{}").with_source("");
    assert!(stage.check(&mut ctx_for("unit_loop"), &e).is_ok());
}

#[test]
fn transition_topic_accepts_dimensions_complete_without_source_at_review_walk() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev("review.dimensions.complete", "{}");
    assert!(stage.check(&mut ctx_for("review_walk"), &e).is_ok());
}

#[test]
fn transition_topic_rejects_dimensions_complete_at_unrelated_step() {
    let stage = FlowStepScopeStage::new(flow());
    let e = ev("review.dimensions.complete", "{}");
    let err = stage.check(&mut ctx_for("plan_end"), &e).unwrap_err();
    assert_eq!(err.reason_code, "flow_unknown_emit");
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
    // This test remains a guard for unrelated business topics: the runtime
    // recovery exception is intentionally specific to `task.resume`.
    let stage = FlowStepScopeStage::new(flow());
    let e = ev_with_source("review.complete", "{}", "dimension-reviewer");
    let err = stage.check(&mut ctx_for("unit_loop"), &e).unwrap_err();
    assert_eq!(err.reason_code, "flow_unknown_emit");
}

#[test]
fn u3_bypass_requires_source_hat_for_non_transition_topics() {
    // No source hat — the bypass cannot match, and this topic is
    // NOT in the transition-topic allowlist, so the legacy reject
    // path fires. `review.dimensions.complete` without source is
    // now handled by the U2 transition-topic bypass (see tests
    // above); this test pins the stricter behaviour for other
    // review-chain topics.
    let stage = FlowStepScopeStage::new(flow());
    let e = ev("review.dimension.done", "{}");
    let err = stage.check(&mut ctx_for("unit_loop"), &e).unwrap_err();
    assert_eq!(err.reason_code, "flow_unknown_emit");
}

// 2026-07-24-005 plan U3 (S1 Accept / S2): supervisor
// exec_wave accepts worker `exec.unit.done` after U2 mounts
// the topic on exec_wave.allowed_emits. Conversely, an
// `exec.unit.done` from a worker at `unit_loop` (before the
// task-planner handoff advances the step) must still be
// rejected — that is the S2 product-decision boundary
// (`unit_loop` does NOT double-mount the topic).
//
// The FlowStepScope is the runtime gate that translates the
// `allowed_emits` declaration into Accept/Reject. The
// companion structural pin lives in
// `ce_executor_supervisor_preset_exec_wave_mounts_unit_terminal_topics`
// (ralph-core/src/preset_lint/supervisor_preset_test.rs).

fn supervisor_flow() -> FlowDeclaration {
    // Mirrors the surviving supervisor-enabled builtin
    // `presets/en/parallel-forge.yml` `mechanism.flow.steps`
    // (plan 2026-08-09-001 removed `ce-executor-supervisor`).
    // Kept inline so the test does not depend on a build-script
    // / include_str pipeline.
    const SUPERVISOR_FLOW_YAML: &str = r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        kind: foreach
        allowed_emits: [work.ready, work.done, execution.plan.ready]
      - id: exec_wave
        kind: side_effect
        allowed_emits:
          - exec.wave.complete
          - exec.wave.failed
          - exec.unit.ready
          - exec.unit.done
          - exec.unit.failed
      - id: exec_integrate
        kind: await
        allowed_emits: [plan.complete]
      - id: review_loop
        kind: side_effect
        allowed_emits:
          - review.wave.complete
          - review.wave.failed
          - review.unit.ready
          - review.unit.done
      - id: fix_loop
        kind: side_effect
        allowed_emits:
          - fix.wave.complete
          - fix.wave.failed
          - fix.unit.ready
          - fix.unit.done
          - fix.unit.failed
"#;
    FlowDeclaration::from_yaml(SUPERVISOR_FLOW_YAML).unwrap()
}

#[test]
fn u3_exec_wave_accepts_worker_exec_unit_done() {
    // S1 Accept: worker emits `exec.unit.done` while the
    // plan is on the `exec_wave` step. The
    // FlowStepScope must return Ok — the topic is in
    // `exec_wave.allowed_emits` per U2.
    let stage = FlowStepScopeStage::new(supervisor_flow());
    let e = ev_with_source("exec.unit.done", "{}", "worker");
    assert!(
        stage.check(&mut ctx_for("exec_wave"), &e).is_ok(),
        "exec_wave must accept `exec.unit.done` after U2 (S1 Accept)"
    );
}

#[test]
fn u3_exec_wave_accepts_worker_exec_unit_failed() {
    // S2 / S5: same accept path for the unit-failed
    // terminal companion.
    let stage = FlowStepScopeStage::new(supervisor_flow());
    let e = ev_with_source("exec.unit.failed", "{}", "worker");
    assert!(
        stage.check(&mut ctx_for("exec_wave"), &e).is_ok(),
        "exec_wave must accept `exec.unit.failed` after U2 (S2 / S5)"
    );
}

#[test]
fn u3_unit_loop_rejects_worker_exec_unit_done() {
    // S2 product-decision boundary: `unit_loop` does NOT
    // mount `exec.unit.done`. A worker that emits before
    // task-planner → exec-wave-dispatcher handoff must be
    // rejected with `flow_unknown_emit`, not silently
    // accepted.
    let stage = FlowStepScopeStage::new(supervisor_flow());
    let e = ev_with_source("exec.unit.done", "{}", "worker");
    let err = stage.check(&mut ctx_for("unit_loop"), &e).unwrap_err();
    assert_eq!(
        err.reason_code, "flow_unknown_emit",
        "unit_loop must reject `exec.unit.done` (S2 boundary); got reason={:?}",
        err.reason_code
    );
}

#[test]
fn u3_unit_loop_accepts_execution_plan_ready() {
    // S3 / R4 boundary: `execution.plan.ready` is mounted on
    // `unit_loop.allowed_emits` so task-planner can hand off
    // to exec-wave-dispatcher and the step advances to
    // `exec_wave`.
    let stage = FlowStepScopeStage::new(supervisor_flow());
    let e = ev_with_source("execution.plan.ready", "{}", "task-planner");
    assert!(
        stage.check(&mut ctx_for("unit_loop"), &e).is_ok(),
        "unit_loop must accept `execution.plan.ready` (S3 handoff)"
    );
}

#[test]
fn u3_review_loop_accepts_review_unit_done() {
    let stage = FlowStepScopeStage::new(supervisor_flow());
    let e = ev_with_source("review.unit.done", "{}", "review-batch-worker");
    assert!(
        stage.check(&mut ctx_for("review_loop"), &e).is_ok(),
        "review_loop must accept `review.unit.done` after isomorphic mount"
    );
}

#[test]
fn u3_fix_loop_accepts_fix_unit_done() {
    let stage = FlowStepScopeStage::new(supervisor_flow());
    let e = ev_with_source("fix.unit.done", "{}", "worker");
    assert!(
        stage.check(&mut ctx_for("fix_loop"), &e).is_ok(),
        "fix_loop must accept `fix.unit.done` after isomorphic mount"
    );
}
