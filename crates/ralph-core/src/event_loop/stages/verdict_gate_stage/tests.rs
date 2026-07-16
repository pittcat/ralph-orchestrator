use super::*;
use crate::event_loop::flow_declaration::FlowDeclaration;
use crate::event_loop::stage_pipeline::{EmitStage, FlowStep, RepairStateMachine, StageContext};
use ralph_proto::Event;

const FLOW_YAML: &str = r"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        allowed_emits: [work.ready]
";

fn flow() -> FlowDeclaration {
    FlowDeclaration::from_yaml(FLOW_YAML).unwrap()
}

fn ctx() -> StageContext<'static> {
    let repair: &'static mut RepairStateMachine =
        Box::leak(Box::new(RepairStateMachine::default()));
    StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 1, repair)
}

fn ev(topic: &str, payload: &str) -> Event {
    Event::new(topic, payload)
}

#[test]
fn verdict_gate_accepts_terminal_topic() {
    let stage = VerdictGateStage::new(flow());
    let e = ev("LOOP_COMPLETE", "{}");
    assert!(stage.check(&mut ctx(), &e).is_ok());
}

#[test]
fn verdict_gate_accepts_non_terminal_topic() {
    // The verdict gate does not police non-terminal topics.
    let stage = VerdictGateStage::new(flow());
    let e = ev("work.ready", "{}");
    assert!(stage.check(&mut ctx(), &e).is_ok());
}

#[test]
fn verdict_gate_is_terminal_matches_terminal_emits() {
    let stage = VerdictGateStage::new(flow());
    assert!(stage.is_terminal("LOOP_COMPLETE"));
    assert!(!stage.is_terminal("work.ready"));
    assert!(!stage.is_terminal("REPORT_DONE"));
    assert!(!stage.is_terminal("REVIEW_COMPLETE"));
}

#[test]
fn default_terminal_emits_contains_loop_complete_only() {
    assert!(DEFAULT_TERMINAL_EMITS.contains(&"LOOP_COMPLETE"));
    assert!(!DEFAULT_TERMINAL_EMITS.contains(&"REPORT_DONE"));
    assert!(!DEFAULT_TERMINAL_EMITS.contains(&"REVIEW_COMPLETE"));
}
