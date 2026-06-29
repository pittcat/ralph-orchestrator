//! U1 (2026-06-27 mechanism foundation completion): tests
//! for the `evaluate_emit_gate` facade. These six tests
//! pin the routing rules declared in the `emit_gate`
//! module-level documentation:
//!
//! 1. Happy path: full payload + non-repair topic
//!    → `AcceptMainBus`.
//! 2. Happy path: `task.relocate_legacy` + pipeline Ok
//!    → `AcceptRepairStream`.
//! 3. Error path: missing required field
//!    → `Reject(missing_required_fields)`.
//! 4. Error path: empty pipeline → any event
//!    → `AcceptMainBus`.
//! 5. Edge case: repair topic + pipeline Reject → `Reject`
//!    (the repair-topic hint never overrides a schema
//!    rejection).
//! 6. Edge case: `LOOP_COMPLETE` accepted by the pipeline
//!    → `AcceptMainBus`.

use super::{EmitGateOutcome, evaluate_emit_gate};
use crate::event_loop::flow_declaration::FlowDeclaration;
use crate::event_loop::repair_flow::RepairStateMachine;
use crate::event_loop::stage_pipeline::{FlowStep, StageContext, StagePipeline};
use ralph_proto::Event;

/// Empty flow declaration — `with_default_stages` does not
/// require any steps, just a valid FlowDeclaration value.
fn minimal_flow() -> FlowDeclaration {
    FlowDeclaration::from_yaml(
        "mechanism:\n  flow:\n    type: declared\n    version: 1\n    steps: []\n",
    )
    .expect("parse minimal flow")
}

/// Flow with steps covering every topic the U1 tests emit
/// against — `unit_loop` for the `work.*` topics and
/// repair-stream topics (`task.relocate_legacy`,
/// `task.relocate`, `repair.budget.exhausted`, `repair.close`),
/// `plan_end` for `plan.blocked`. `LOOP_COMPLETE` is in
/// `terminal_emits` so the VerdictGate accepts it.
/// U11 (2026-06-27-002 plan) made `FlowStepScopeStage`
/// fail-closed, so tests that previously relied on the
/// empty flow + `LOOP_COMPLETE` short-circuit must now
/// declare the step the emit belongs to. Repair topics
/// must also be enumerated on the emitting step — the
/// facade only routes to `AcceptRepairStream` when the
/// pipeline accepts, and `FlowStepScope` does not
/// short-circuit on repair topics (the U1 facade contract
/// is "pipeline reject wins over the repair hint").
fn flow_for_facade_tests() -> FlowDeclaration {
    FlowDeclaration::from_yaml(
        "mechanism:\n  flow:\n    type: declared\n    version: 1\n    terminal_emits: [LOOP_COMPLETE]\n    steps:\n      - id: unit_loop\n        allowed_emits: [work.done, work.ready, task.relocate_legacy, task.relocate, repair.budget.exhausted, repair.close]\n      - id: plan_end\n        allowed_emits: [plan.blocked]\n",
    )
    .expect("parse facade-test flow")
}

fn default_pipeline() -> StagePipeline {
    StagePipeline::with_default_stages(flow_for_facade_tests())
}

fn empty_pipeline() -> StagePipeline {
    StagePipeline::default()
}

/// Build a `StageContext` that carries the pipeline —
/// mirrors what `EventLoop::build_stage_context_for`
/// produces at runtime. The `RepairStateMachine` is
/// owned by the caller (typically the test body) so the
/// returned context's lifetime is tied to that caller.
///
/// `step_id` is the declared flow step the hat is in
/// (looked up against `flow.steps` by `FlowStepScopeStage`);
/// `topic` is the topic the hat is emitting. After U11
/// fail-closed, every emit must belong to a step whose
/// `allowed_emits` contains the topic — passing the same
/// string for both is a common foot-gun for tests that
/// pre-date U11.
/// P1-5 (2026-06-27 adversarial review): the helper
/// now takes `&mut HashMap<String, RepairStateMachine>`
/// instead of a single machine. Tests that don't care
/// about per-task isolation use the one-element
/// `for_test_machine` shim via `StageContext::with_pipeline`
/// — they pre-populate the registry with a single
/// `_loop_default` machine. The legacy `sm: &mut
/// RepairStateMachine` shape is preserved at the call
/// site by wrapping the `sm` in a one-element `HashMap`
/// in each test.
fn ctx_with_pipeline<'a>(
    pipeline: &'a StagePipeline,
    step_id: &'static str,
    states: &'a mut std::collections::HashMap<String, RepairStateMachine>,
) -> StageContext<'a> {
    StageContext::with_pipeline(FlowStep::new(step_id), "u1-test", 0, states, pipeline)
}

#[test]
fn u1_facade_accept_main_bus_for_complete_work_done() {
    let mut pipeline = default_pipeline();
    let mut sm: std::collections::HashMap<String, RepairStateMachine> =
        std::collections::HashMap::new();
    let mut ctx = ctx_with_pipeline(&mut pipeline, "unit_loop", &mut sm);
    let event = Event::new("work.done", r#"{"task_id":"t-1"}"#);
    let outcome = evaluate_emit_gate(&mut ctx, &event);
    assert_eq!(outcome, EmitGateOutcome::AcceptMainBus);
}

#[test]
fn u1_facade_accept_repair_stream_for_relocate_legacy() {
    let mut pipeline = default_pipeline();
    let mut sm: std::collections::HashMap<String, RepairStateMachine> =
        std::collections::HashMap::new();
    // `task.relocate_legacy` is a repair topic — the
    // FlowStepScopeStage short-circuits repair topics
    // BEFORE the step lookup (verdict_gate_topics), so
    // the step id here is arbitrary.
    let mut ctx = ctx_with_pipeline(&mut pipeline, "unit_loop", &mut sm);
    let event = Event::new("task.relocate_legacy", r#"{"task_key":"legacy-1"}"#);
    let outcome = evaluate_emit_gate(&mut ctx, &event);
    assert_eq!(outcome, EmitGateOutcome::AcceptRepairStream);
}

#[test]
fn u1_facade_reject_when_required_field_missing() {
    let mut pipeline = default_pipeline();
    let mut sm: std::collections::HashMap<String, RepairStateMachine> =
        std::collections::HashMap::new();
    let mut ctx = ctx_with_pipeline(&mut pipeline, "plan_end", &mut sm);
    // `plan.blocked` requires `reason` per the default
    // schema gate. Empty payload → Reject.
    let event = Event::new("plan.blocked", r#"{}"#);
    let outcome = evaluate_emit_gate(&mut ctx, &event);
    match outcome {
        EmitGateOutcome::Reject(reject) => {
            assert_eq!(reject.reason_code, "missing_required_fields");
            assert!(reject.missing_fields.contains(&"reason".to_string()));
        }
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[test]
fn u1_facade_empty_pipeline_accepts_every_event() {
    let mut pipeline = empty_pipeline();
    let mut sm: std::collections::HashMap<String, RepairStateMachine> =
        std::collections::HashMap::new();
    let mut ctx = ctx_with_pipeline(&mut pipeline, "plan_end", &mut sm);
    // Even a malformed event is accepted when there is
    // no stage to reject it.
    let event = Event::new("plan.blocked", r#"{}"#);
    let outcome = evaluate_emit_gate(&mut ctx, &event);
    assert_eq!(outcome, EmitGateOutcome::AcceptMainBus);
}

#[test]
fn u1_facade_repair_topic_with_pipeline_reject_yields_reject() {
    use crate::event_loop::stages::emit_schema_gate_stage::EmitSchemaGateStage;
    let mut required = std::collections::HashMap::new();
    required.insert(
        "task.relocate_legacy".to_string(),
        vec!["task_key".to_string()],
    );
    let mut schema_only = StagePipeline::new(vec![Box::new(EmitSchemaGateStage::new(required))]);
    let mut sm: std::collections::HashMap<String, RepairStateMachine> =
        std::collections::HashMap::new();
    let mut ctx = ctx_with_pipeline(&mut schema_only, "unit_loop", &mut sm);
    let event = Event::new("task.relocate_legacy", r#"{}"#);
    let outcome = evaluate_emit_gate(&mut ctx, &event);
    match outcome {
        EmitGateOutcome::Reject(reject) => {
            assert_eq!(reject.reason_code, "missing_required_fields");
        }
        other => panic!("expected Reject, got {other:?}"),
    }
}

#[test]
fn u1_facade_loop_complete_topic_passes_to_main_bus() {
    let mut pipeline = default_pipeline();
    let mut sm: std::collections::HashMap<String, RepairStateMachine> =
        std::collections::HashMap::new();
    let mut ctx = ctx_with_pipeline(&mut pipeline, "ship", &mut sm);
    // `LOOP_COMPLETE` is in `terminal_emits`; the
    // FlowStepScopeStage and VerdictGate short-circuit
    // terminal topics before any step lookup, so the
    // step id here is arbitrary.
    let event = Event::new("LOOP_COMPLETE", r#"{}"#);
    let outcome = evaluate_emit_gate(&mut ctx, &event);
    assert_eq!(outcome, EmitGateOutcome::AcceptMainBus);
}
