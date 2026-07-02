//! 2026-07-02-006 plan U14: tests for
//! `StagePipeline::with_phase_authority_stages_for_loop_config`.
//!
//! Test entry point:
//! `cargo nextest run -p ralph-core -- with_phase_authority_stages`.
//!
//! Per KTD9 the phase stage must run **before** the flow-scope
//! stage. The unit checks that `names()` returns the expected
//! ordering; it does NOT exercise `build_stage_pipeline` (U15).

use std::sync::Arc;

use crate::event_loop::flow_declaration::FlowDeclaration;
use crate::event_loop::phase_authority::WorkflowPhaseAuthority;
use crate::event_loop::stage_pipeline::StagePipeline;

fn minimal_flow() -> FlowDeclaration {
    FlowDeclaration::from_yaml(
        r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        allowed_emits: [work.ready, work.done]
"#,
    )
    .expect("minimal flow must parse")
}

#[test]
fn phase_authority_pipeline_includes_phase_authority_stage() {
    let flow = minimal_flow();
    let engine = Arc::new(WorkflowPhaseAuthority::disabled());
    let pipeline = StagePipeline::with_phase_authority_stages_for_loop_config(
        flow,
        None,
        engine,
    );
    let names = pipeline.names();
    assert!(
        names.contains(&"PhaseAuthority"),
        "phase-authority pipeline must include the PhaseAuthority stage; got {:?}",
        names
    );
}

#[test]
fn phase_authority_stage_runs_before_flow_step_scope() {
    let flow = minimal_flow();
    let engine = Arc::new(WorkflowPhaseAuthority::disabled());
    let pipeline = StagePipeline::with_phase_authority_stages_for_loop_config(
        flow,
        None,
        engine,
    );
    let names = pipeline.names();
    let pa = names
        .iter()
        .position(|n| *n == "PhaseAuthority")
        .expect("PhaseAuthority must be present");
    let fss = names
        .iter()
        .position(|n| *n == "FlowStepScope")
        .expect("FlowStepScope must be present");
    assert!(
        pa < fss,
        "PhaseAuthority must run before FlowStepScope per KTD9; got {:?}",
        names
    );
}

#[test]
fn phase_authority_pipeline_preserves_other_stages() {
    let flow = minimal_flow();
    let engine = Arc::new(WorkflowPhaseAuthority::disabled());
    let pipeline = StagePipeline::with_phase_authority_stages_for_loop_config(
        flow,
        None,
        engine,
    );
    let names = pipeline.names();
    for required in [
        "RepairDispatch",
        "EmitSchemaGate",
        "PhaseAuthority",
        "FlowStepScope",
        "StepCloseObligation",
        "VerdictGate",
    ] {
        assert!(
            names.contains(&required),
            "phase-authority pipeline missing {}; got {:?}",
            required,
            names
        );
    }
}