use crate::event_loop::flow_declaration::FlowDeclaration;
use crate::event_loop::stage_pipeline::{
    EmitStage, FlowStep, RepairStateMachine, StageContext, StagePipeline, StageReject,
};
use ralph_proto::Event;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counter stage that accepts every event and increments a counter.
struct AcceptCounter {
    name: &'static str,
    counter: Arc<AtomicUsize>,
}

impl AcceptCounter {
    fn new(name: &'static str, counter: Arc<AtomicUsize>) -> Self {
        Self { name, counter }
    }
}

impl EmitStage for AcceptCounter {
    fn name(&self) -> &'static str {
        self.name
    }

    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        self.counter.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Stage that rejects every event with a fixed reason.
struct AlwaysReject {
    name: &'static str,
}

impl EmitStage for AlwaysReject {
    fn name(&self) -> &'static str {
        self.name
    }

    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        Err(StageReject::new(self.name, "always_reject"))
    }
}

fn dummy_event() -> Event {
    Event::new("work.ready", "{}")
}

fn dummy_ctx(repair: &mut RepairStateMachine) -> StageContext<'_> {
    StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 1, repair)
}

#[test]
fn stage_pipeline_skeleton_empty_accepts_everything() {
    let mut repair = RepairStateMachine::default();
    let pipeline = StagePipeline::default();
    let event = dummy_event();
    assert!(pipeline.run(&mut dummy_ctx(&mut repair), &event).is_ok());
}

#[test]
fn stage_pipeline_skeleton_three_counters_run_in_order() {
    let mut repair = RepairStateMachine::default();
    let a = Arc::new(AtomicUsize::new(0));
    let b = Arc::new(AtomicUsize::new(0));
    let c = Arc::new(AtomicUsize::new(0));

    let pipeline = StagePipeline::new(vec![
        Box::new(AcceptCounter::new("Alpha", a.clone())),
        Box::new(AcceptCounter::new("Beta", b.clone())),
        Box::new(AcceptCounter::new("Gamma", c.clone())),
    ]);

    let event = dummy_event();
    assert!(pipeline.run(&mut dummy_ctx(&mut repair), &event).is_ok());

    assert_eq!(a.load(Ordering::SeqCst), 1);
    assert_eq!(b.load(Ordering::SeqCst), 1);
    assert_eq!(c.load(Ordering::SeqCst), 1);
}

#[test]
fn stage_pipeline_skeleton_reject_short_circuits() {
    let mut repair = RepairStateMachine::default();
    let a = Arc::new(AtomicUsize::new(0));
    let b = Arc::new(AtomicUsize::new(0));
    let c = Arc::new(AtomicUsize::new(0));

    let pipeline = StagePipeline::new(vec![
        Box::new(AcceptCounter::new("Alpha", a.clone())),
        Box::new(AlwaysReject { name: "Beta" }),
        Box::new(AcceptCounter::new("Gamma", c.clone())),
    ]);

    let event = dummy_event();
    let err = pipeline
        .run(&mut dummy_ctx(&mut repair), &event)
        .unwrap_err();

    assert_eq!(err.stage_name, "Beta");
    assert_eq!(err.reason_code, "always_reject");
    assert_eq!(a.load(Ordering::SeqCst), 1);
    assert_eq!(b.load(Ordering::SeqCst), 0);
    assert_eq!(c.load(Ordering::SeqCst), 0);
}

// Dummy named stages used only for order-assertion compile-time tests.
struct ArchiveVersionStage;
struct RepairDispatchStage;
struct EmitSchemaGateStage;
struct FlowStepScopeStage;
struct VerdictGateStage;

impl EmitStage for ArchiveVersionStage {
    fn name(&self) -> &'static str {
        "ArchiveVersion"
    }
    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

impl EmitStage for RepairDispatchStage {
    fn name(&self) -> &'static str {
        "RepairDispatch"
    }
    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

impl EmitStage for EmitSchemaGateStage {
    fn name(&self) -> &'static str {
        "EmitSchemaGate"
    }
    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

impl EmitStage for FlowStepScopeStage {
    fn name(&self) -> &'static str {
        "FlowStepScope"
    }
    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

impl EmitStage for VerdictGateStage {
    fn name(&self) -> &'static str {
        "VerdictGate"
    }
    fn check(&self, _ctx: &mut StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

#[test]
fn stage_pipeline_skeleton_locked_order_matches() {
    let pipeline = StagePipeline::new(vec![
        Box::new(ArchiveVersionStage),
        Box::new(RepairDispatchStage),
        Box::new(EmitSchemaGateStage),
        Box::new(FlowStepScopeStage),
        // P1-4 (2026-06-27 adversarial review):
        // the locked emit order now also
        // includes `StepCloseObligation`
        // between `FlowStepScope` and
        // `VerdictGate`.
        Box::new(
            crate::event_loop::stages::step_close_obligation_stage::StepCloseObligationStage::new(
                FlowDeclaration::from_yaml(
                    "mechanism:\n  flow:\n    type: declared\n    version: 1\n    steps: []\n",
                )
                .unwrap(),
            ),
        ),
        Box::new(VerdictGateStage),
    ]);

    // P1-4 (2026-06-27 adversarial review): the
    // locked emit order now also includes
    // `StepCloseObligation` between
    // `FlowStepScope` and `VerdictGate`. The
    // `ArchiveVersion` stage is a loop-start hook,
    // not an emit stage, so it does not appear in
    // the runtime pipeline.
    crate::assert_stage_order!(
        pipeline,
        [
            ArchiveVersion,
            RepairDispatch,
            EmitSchemaGate,
            FlowStepScope,
            StepCloseObligation,
            VerdictGate
        ]
    );
}

#[test]
fn stage_pipeline_order_default_matches_locked_emit_order() {
    use crate::event_loop::flow_declaration::FlowDeclaration;

    let flow = FlowDeclaration::from_yaml(
        r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        allowed_emits: [work.ready]
"#,
    )
    .unwrap();
    let pipeline = StagePipeline::with_default_stages(flow);
    assert_eq!(
        pipeline.names(),
        // P1-4 (2026-06-27 adversarial review):
        // `StepCloseObligation` is now part of
        // the locked emit order (between
        // `FlowStepScope` and `VerdictGate`).
        vec![
            "RepairDispatch",
            "EmitSchemaGate",
            "FlowStepScope",
            "StepCloseObligation",
            "VerdictGate"
        ]
    );
}

#[test]
fn hat_only_pipeline_omits_flow_step_scope_and_accepts_plan_ready() {
    use crate::event_loop::emit_gate::{EmitGateOutcome, evaluate_emit_gate};
    use crate::event_loop::repair_flow::RepairStateMachine;
    use ralph_proto::Event;

    let mut pipeline = StagePipeline::with_hat_only_stages_for_loop_config(None);
    assert_eq!(
        pipeline.names(),
        vec!["RepairDispatch", "EmitSchemaGate", "VerdictGate"]
    );

    let mut sm: std::collections::HashMap<String, RepairStateMachine> =
        std::collections::HashMap::new();
    let mut ctx =
        StageContext::with_pipeline(FlowStep::new("unit_loop"), "loop-1", 1, &mut sm, &pipeline);
    let event = Event::new(
        "plan.ready",
        r#"{"plan_name":"p","plan_path":"docs/plans/p.md","plan_revised":false,"review_summary":"ok"}"#,
    );
    let outcome = evaluate_emit_gate(&mut ctx, &event);
    assert!(
        matches!(outcome, EmitGateOutcome::AcceptMainBus),
        "hat-only pipeline must not reject plan.ready via FlowStepScope: {outcome:?}"
    );
}

#[test]
fn stage_pipeline_skeleton_wrong_order_fails_at_runtime() {
    let pipeline = StagePipeline::new(vec![
        Box::new(RepairDispatchStage),
        Box::new(ArchiveVersionStage),
    ]);

    let actual = pipeline.names();
    let expected: &[&str] = &["ArchiveVersion", "RepairDispatch"];
    assert_ne!(
        actual, expected,
        "deliberately wrong order to test assertion utility"
    );
}
