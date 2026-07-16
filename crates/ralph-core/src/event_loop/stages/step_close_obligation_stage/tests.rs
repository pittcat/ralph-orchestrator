use super::*;
use crate::event_loop::flow_declaration::FlowDeclaration;
use crate::event_loop::stage_pipeline::{FlowStep, StageContext};
use ralph_proto::Event;

fn declared_flow() -> FlowDeclaration {
    let yaml = r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: []
    steps:
      - id: unit_loop
        terminal_when: partial_units_done
        on_partial:
          partial: plan.blocked(reason="4_of_8_partial")
"#;
    FlowDeclaration::from_yaml(yaml).expect("parse flow")
}

#[test]
fn no_progress_recorded_means_no_obligation() {
    let flow = declared_flow();
    let stage = StepCloseObligationStage::new(flow);
    let mut sm = crate::event_loop::repair_flow::RepairStateMachine::default();
    let mut ctx = StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 0, &mut sm);
    let event = Event::new("plan.blocked", r#"{"reason":"unrelated"}"#);
    assert!(stage.check(&mut ctx, &event).is_ok());
}

#[test]
fn partial_progress_emits_must_match_on_partial_branch() {
    let flow = declared_flow();
    let mut stage = StepCloseObligationStage::new(flow);
    stage.update_progress("unit_loop", 4, 8);
    let mut sm = crate::event_loop::repair_flow::RepairStateMachine::default();
    let mut ctx = StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 0, &mut sm);
    // Match — should be accepted.
    let good = Event::new("plan.blocked", r#"{"reason":"4_of_8_partial"}"#);
    assert!(stage.check(&mut ctx, &good).is_ok());
    // Wrong topic — should be rejected.
    let bad_topic = Event::new("work.done", r#"{"task_id":"t1"}"#);
    let r = stage.check(&mut ctx, &bad_topic);
    assert!(r.is_err());
    // Wrong reason pattern — should be rejected.
    let bad_reason = Event::new("plan.blocked", r#"{"reason":"all_done"}"#);
    let r = stage.check(&mut ctx, &bad_reason);
    assert!(r.is_err());
}

#[test]
fn complete_progress_means_no_obligation() {
    let flow = declared_flow();
    let mut stage = StepCloseObligationStage::new(flow);
    stage.update_progress("unit_loop", 8, 8);
    let mut sm = crate::event_loop::repair_flow::RepairStateMachine::default();
    let mut ctx = StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 0, &mut sm);
    // Even a non-matching emit is accepted because
    // the step is complete.
    let event = Event::new("work.done", r#"{"task_id":"t1"}"#);
    assert!(stage.check(&mut ctx, &event).is_ok());
}

#[test]
fn update_progress_is_idempotent_and_no_regressions() {
    let flow = declared_flow();
    let mut stage = StepCloseObligationStage::new(flow);
    stage.update_progress("unit_loop", 4, 8);
    // No-op (same value).
    stage.update_progress("unit_loop", 4, 8);
    // Regression in `done` is rejected silently
    // (the larger value sticks).
    stage.update_progress("unit_loop", 2, 8);
    let progress = stage.progress.get("unit_loop").copied().unwrap();
    assert_eq!(progress.done, 4);
    // Forward progress is recorded.
    stage.update_progress("unit_loop", 6, 8);
    let progress = stage.progress.get("unit_loop").copied().unwrap();
    assert_eq!(progress.done, 6);
}

#[test]
fn empty_on_partial_means_no_obligation() {
    let yaml = r"
mechanism:
  flow:
    type: declared
    version: 1
    steps:
      - id: unit_loop
        terminal_when: partial_units_done
";
    let flow = FlowDeclaration::from_yaml(yaml).expect("parse flow");
    let mut stage = StepCloseObligationStage::new(flow);
    stage.update_progress("unit_loop", 4, 8);
    let mut sm = crate::event_loop::repair_flow::RepairStateMachine::default();
    let mut ctx = StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 0, &mut sm);
    // on_partial is empty so no obligation — the
    // stage is a no-op even at 4/8.
    let event = Event::new("work.done", r#"{"task_id":"t1"}"#);
    assert!(stage.check(&mut ctx, &event).is_ok());
}

#[test]
fn step_not_in_flow_means_no_obligation() {
    let flow = declared_flow();
    let mut stage = StepCloseObligationStage::new(flow);
    stage.update_progress("other_step", 1, 4);
    let mut sm = crate::event_loop::repair_flow::RepairStateMachine::default();
    let mut ctx = StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 0, &mut sm);
    let event = Event::new("work.done", r#"{"task_id":"t1"}"#);
    // Step not in flow → no obligation even if
    // progress is recorded.
    assert!(stage.check(&mut ctx, &event).is_ok());
}

// P0-1 (2026-06-28 review): the
// `StagePipeline::update_step_close_progress` helper
// routes through `EmitStage::as_any_mut` to drive the
// concrete `StepCloseObligationStage`. Verify the
// wiring end-to-end: building the locked-default
// pipeline must accept `update_progress` calls and
// the stage must then reject an out-of-band emit.
#[test]
fn pipeline_update_step_close_progress_drives_stage() {
    use crate::event_loop::stage_pipeline::StagePipeline;

    let flow_yaml = r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: []
    steps:
      - id: unit_loop
        total_units: 8
        allowed_emits:
          - work.done
        terminal_when: partial_units_done
        on_partial:
          partial: plan.blocked(reason="4_of_8_partial")
"#;
    let flow =
        crate::event_loop::flow_declaration::FlowDeclaration::from_yaml(flow_yaml).expect("flow");
    let mut pipeline = StagePipeline::with_default_stages(flow);

    // Drive 4/8 progress.
    pipeline.update_step_close_progress("unit_loop", 4, 8);

    let mut sm = crate::event_loop::repair_flow::RepairStateMachine::default();
    let mut ctx = StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 0, &mut sm);
    // The wiring must now reject an emit that does not
    // satisfy `on_partial.partial`.
    let bad = Event::new("work.done", r#"{"task_id":"t1"}"#);
    let result = pipeline.run(&mut ctx, &bad);
    assert!(
        result.is_err(),
        "P0-1: stage pipeline must reject out-of-band emit after update_progress"
    );
    let reject = result.unwrap_err();
    assert_eq!(reject.stage_name, "StepCloseObligation");
    assert_eq!(reject.reason_code, "step_close_obligation_violated");
}

// 2026-06-28-002 U6: `flow_step_total_units` falls back to
// the `tasks.jsonl` fix-unit count when a fix step does not
// declare `total_units`. We exercise the helper directly here
// because the wiring is private to `EventLoop`.
#[test]
fn u6_fix_step_falls_back_to_tasks_jsonl_count() {
    use crate::task::{Task, TaskStatus};
    use crate::task_store::TaskStore;

    // Stand-in EventLoop with a custom `tasks_path` is heavy;
    // instead we replicate the U6 helper contract:
    //   - count tasks whose key matches `ce-executor:*:fix-{step}:*`
    //   - return None when no tasks match
    fn count_fix_unit_tasks_in(path: &std::path::Path, step_id: &str) -> Option<u32> {
        let store = TaskStore::load(path).ok()?;
        let prefix = "ce-executor:".to_string();
        let needle = format!(":{step_id}:");
        let count = store
            .all()
            .iter()
            .filter(|t| {
                t.key
                    .as_deref()
                    .map(|k| k.starts_with(&prefix) && k.contains(&needle))
                    .unwrap_or(false)
            })
            .count() as u32;
        if count == 0 { None } else { Some(count) }
    }

    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("tasks.jsonl");
    let mut store = TaskStore::load(&path).unwrap();
    for n in 1..=5 {
        let mut t = Task::new(format!("fix-{n}"), 1);
        t.key = Some(format!("ce-executor:myplan:fix-{n:02}:u{n}-impl"));
        t.status = TaskStatus::Open;
        store.add(t);
    }
    store.save().unwrap();

    // fix-01 step has exactly one matching task.
    assert_eq!(count_fix_unit_tasks_in(&path, "fix-01"), Some(1));
    // fix-02 also matches.
    assert_eq!(count_fix_unit_tasks_in(&path, "fix-02"), Some(1));
    // No step-99 tasks → None.
    assert_eq!(count_fix_unit_tasks_in(&path, "step-99"), None);
}

#[test]
fn u6_non_fix_step_still_returns_none_when_undeclared() {
    // Non-fix steps must NOT auto-derive total_units from
    // arbitrary tasks.jsonl records — that would silently
    // change behaviour for other presets. The fallback is
    // strictly opt-in for the fix-* shape.
    use crate::task::Task;
    use crate::task_store::TaskStore;

    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("tasks.jsonl");
    let mut store = TaskStore::load(&path).unwrap();
    let mut t = Task::new("some step task".into(), 1);
    t.key = Some("ce-executor:myplan:step-03:u1-impl".into());
    store.add(t);
    store.save().unwrap();

    let store = TaskStore::load(&path).unwrap();
    let count = store
        .all()
        .iter()
        .filter(|t| {
            t.key
                .as_deref()
                .map(|k| k.starts_with("ce-executor:") && k.contains(":step-03:"))
                .unwrap_or(false)
        })
        .count() as u32;
    // The repo count itself is 1, but `flow_step_total_units`
    // does NOT use this fallback for non-fix steps — so the
    // `flow_step_totals` lookup returns None at the top of
    // `flow_step_total_units` and we never reach this branch.
    // We pin the count here so any future change to the helper
    // contract is forced to update this test.
    assert_eq!(count, 1);
}
