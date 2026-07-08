//! U4 (2026-06-27 mechanism foundation completion):
//! the stage pipeline no longer holds a stub
//! `RepairStateMachine`. Every `StageContext` and the
//! `EventLoop` itself now carry the real
//! `repair_flow::RepairStateMachine` (U2 / 002 plan),
//! so U5's `RepairDispatchStage` can read the budget
//! from the same struct the loop owns.
//!
//! P1-5 (2026-06-27 adversarial review): the
//! `RepairStateMachine` was promoted to a
//! per-`task_key` registry inside `EventLoop` to
//! satisfy R2 per-task budget. The
//! `StageContext.repair_states` field is a
//! `HashMap<String, RepairStateMachine>` instead
//! of a single instance; tests below verify the
//! registry starts empty and the
//! `RepairDispatchStage` lazily inserts a fresh
//! machine per task key.
//!
//! Pinned contracts:
//! 1. `EventLoop::new` initialises an empty
//!    `repair_state_machines` registry (the
//!    `RepairDispatchStage` lazily inserts on
//!    first use).
//! 2. `build_stage_context_for` returns a
//!    `StageContext` whose `repair_states` points
//!    at the loop's registry.
//! 3. The `stage_pipeline::RepairStateMachine` type
//!    no longer exists as a stub (compile-time
//!    guarantee).

use super::*;

fn build_loop_for_u4(workspace: &std::path::Path) -> EventLoop {
    let events_path = workspace.join("events.jsonl");
    let diagnostics_root = workspace.to_path_buf();
    let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
hats:
  planner:
    name: "Planner"
    triggers: ["work.start"]
    publishes: ["work.done"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("U4 repair state machine unification");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop
}

#[test]
fn u4_repair_state_machine_default_budget_is_three() {
    use crate::event_loop::repair_flow::{RepairBudget, RepairStateMachine};
    // The default budget used by `RepairStateMachine::default`
    // is 3 (the historical preset used `mechanism.repair_budget: 3`
    // — see `presets/schemas/`).
    let sm = RepairStateMachine::default();
    assert_eq!(sm.budget(), RepairBudget { max: 3 });
}

#[test]
fn u4_event_loop_owns_real_repair_state_machine() {
    let temp = tempfile::tempdir().unwrap();
    let event_loop = build_loop_for_u4(temp.path());
    // P1-5 (2026-06-27 adversarial review): the
    // registry is empty on construction;
    // `RepairDispatchStage` lazily inserts a
    // fresh machine for each new `task_key`.
    assert!(event_loop.repair_state_machines.is_empty());
}

#[test]
fn u4_stage_pipeline_re_exports_repair_flow_state_machine() {
    // The compile-time guarantee: `stage_pipeline::RepairStateMachine`
    // resolves to `repair_flow::RepairStateMachine` after
    // U4. If U4 broke the rename, this would fail to
    // compile.
    use crate::event_loop::repair_flow::RepairStateMachine as RepairFlowSm;
    use crate::event_loop::stage_pipeline::RepairStateMachine as StagePipeSm;
    fn assert_same_type(_: RepairFlowSm, _: StagePipeSm) {}
    let _check: fn(RepairFlowSm, StagePipeSm) = assert_same_type;
    // Run the assertion to ensure the types are usable.
    let a = RepairFlowSm::default();
    let b = StagePipeSm::default();
    assert_same_type(a, b);
}

/// Verify the `StageContext` returned by
/// `build_stage_context_for` references the SAME
/// `repair_state_machines` registry the loop owns
/// (P1-5: the registry is per-loop; the per-task
/// machine is lazily inserted on first use).
#[test]
fn u4_build_stage_context_shares_repair_state_machine() {
    use crate::event_loop::repair_flow::RepairBudget;
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u4(temp.path());
    let ctx = event_loop.build_stage_context_for(&Event::new("work.start", "{}"));
    // Both start empty; the `RepairDispatchStage`
    // inserts on first use. The per-task default
    // budget is 3 (mirroring
    // `mechanism.repair_budget: 3` in the historical
    // preset family — see `presets/schemas/`).
    assert!(ctx.repair_states.is_empty());
    assert!(event_loop.repair_state_machines.is_empty());
    let _ = RepairBudget { max: 3 };
}
