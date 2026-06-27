use crate::event_loop::stage_pipeline::{
    EmitStage, FlowStep, RepairStateMachine, StageContext, StagePipeline, StageReject,
};
use ralph_proto::Event;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

    fn check(&self, _ctx: &StageContext, _event: &Event) -> Result<(), StageReject> {
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

    fn check(&self, _ctx: &StageContext, _event: &Event) -> Result<(), StageReject> {
        Err(StageReject::new(self.name, "always_reject"))
    }
}

fn dummy_event() -> Event {
    Event::new("work.ready", "{}")
}

fn dummy_ctx(repair: &RepairStateMachine) -> StageContext<'_> {
    StageContext::new(FlowStep::new("unit_loop"), "loop-1", 1, repair)
}

#[test]
fn stage_pipeline_skeleton_empty_accepts_everything() {
    let repair = RepairStateMachine;
    let pipeline = StagePipeline::default();
    let event = dummy_event();
    assert!(pipeline.run(&dummy_ctx(&repair), &event).is_ok());
}

#[test]
fn stage_pipeline_skeleton_three_counters_run_in_order() {
    let repair = RepairStateMachine;
    let a = Arc::new(AtomicUsize::new(0));
    let b = Arc::new(AtomicUsize::new(0));
    let c = Arc::new(AtomicUsize::new(0));

    let pipeline = StagePipeline::new(vec![
        Box::new(AcceptCounter::new("Alpha", a.clone())),
        Box::new(AcceptCounter::new("Beta", b.clone())),
        Box::new(AcceptCounter::new("Gamma", c.clone())),
    ]);

    let event = dummy_event();
    assert!(pipeline.run(&dummy_ctx(&repair), &event).is_ok());

    assert_eq!(a.load(Ordering::SeqCst), 1);
    assert_eq!(b.load(Ordering::SeqCst), 1);
    assert_eq!(c.load(Ordering::SeqCst), 1);
}

#[test]
fn stage_pipeline_skeleton_reject_short_circuits() {
    let repair = RepairStateMachine;
    let a = Arc::new(AtomicUsize::new(0));
    let b = Arc::new(AtomicUsize::new(0));
    let c = Arc::new(AtomicUsize::new(0));

    let pipeline = StagePipeline::new(vec![
        Box::new(AcceptCounter::new("Alpha", a.clone())),
        Box::new(AlwaysReject { name: "Beta" }),
        Box::new(AcceptCounter::new("Gamma", c.clone())),
    ]);

    let event = dummy_event();
    let err = pipeline.run(&dummy_ctx(&repair), &event).unwrap_err();

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
    fn check(&self, _ctx: &StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

impl EmitStage for RepairDispatchStage {
    fn name(&self) -> &'static str {
        "RepairDispatch"
    }
    fn check(&self, _ctx: &StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

impl EmitStage for EmitSchemaGateStage {
    fn name(&self) -> &'static str {
        "EmitSchemaGate"
    }
    fn check(&self, _ctx: &StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

impl EmitStage for FlowStepScopeStage {
    fn name(&self) -> &'static str {
        "FlowStepScope"
    }
    fn check(&self, _ctx: &StageContext, _event: &Event) -> Result<(), StageReject> {
        Ok(())
    }
}

impl EmitStage for VerdictGateStage {
    fn name(&self) -> &'static str {
        "VerdictGate"
    }
    fn check(&self, _ctx: &StageContext, _event: &Event) -> Result<(), StageReject> {
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
        Box::new(VerdictGateStage),
    ]);

    crate::assert_stage_order!(pipeline, [ArchiveVersion, RepairDispatch, EmitSchemaGate, FlowStepScope, VerdictGate]);
}

#[test]
fn stage_pipeline_skeleton_wrong_order_fails_at_runtime() {
    let pipeline = StagePipeline::new(vec![
        Box::new(RepairDispatchStage),
        Box::new(ArchiveVersionStage),
    ]);

    let actual = pipeline.names();
    let expected: &[&str] = &["ArchiveVersion", "RepairDispatch"];
    assert_ne!(actual, expected, "deliberately wrong order to test assertion utility");
}
