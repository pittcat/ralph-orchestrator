use super::*;
use crate::event_loop::stage_pipeline::{EmitStage, FlowStep, StageContext};

/// Build a fresh `(ctx, machine)` pair. The
/// `&'static mut RepairStateMachine` lives in
/// leaked heap memory for the test's lifetime so the
/// caller can thread the same machine across multiple
/// `stage.check` calls and observe state evolution.
/// Returns a `Box` so the caller never accidentally
/// shares the leak across threads.
fn fresh_ctx() -> StageContext<'static> {
    let repair: &'static mut RepairStateMachine =
        Box::leak(Box::new(RepairStateMachine::default()));
    StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 1, repair)
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
    let e = ev(
        "task.relocate_legacy",
        r#"{"task_key":"abc","target_loop_id":"loop-x","reason":"legacy"}"#,
    );
    assert!(
        stage.check(&mut fresh_ctx(), &e).is_ok(),
        "repair events must not be rejected by the stage"
    );
}

#[test]
fn repair_dispatch_stage_accepts_non_repair_events() {
    let stage = RepairDispatchStage;
    let e = ev("work.ready", "{}");
    assert!(
        stage.check(&mut fresh_ctx(), &e).is_ok(),
        "non-repair events must pass through"
    );
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

// P1-4 (2026-06-28 review): `task.resume` is a
// budget-tracked topic — it stays on the main bus but
// advances the per-task `RepairStateMachine`. After the
// default 3 retries are consumed, the next `task.resume`
// must be rejected with `repair_unrecoverable_after_*`
// so the budget exhaustion path (P1-1) can synthesise
// `plan.blocked`.
#[test]
fn task_resume_consumes_repair_budget_then_rejects() {
    use std::collections::HashMap;

    let stage = RepairDispatchStage;
    let payload = r#"{"task_key":"alpha","reason":"stall_no_events"}"#;

    // Build a single persistent `HashMap` and leak it.
    // The `StageContext` borrows `&mut` from this map,
    // so every `check` call observes the previous
    // transitions on the same `_loop_default` machine.
    let states: &'static mut HashMap<String, RepairStateMachine> =
        Box::leak(Box::new(HashMap::new()));

    // Helper macro: each call constructs a fresh
    // `StageContext` borrowing the leaked map.
    macro_rules! ctx {
        () => {
            StageContext::new(FlowStep::new("unit_loop"), "loop-1", 1, states)
        };
    }

    // First emit transitions `_loop_default` from
    // `Detected` → `Diagnosing` (no budget consumed).
    let result = stage.check(&mut ctx!(), &ev("task.resume", payload));
    assert!(
        result.is_ok(),
        "first task.resume (Detected → Diagnosing) must be accepted: {:?}",
        result
    );

    // Three more emits are `Retry` actions. With the
    // default `repair_budget: 3`, three retries fit
    // inside the budget.
    for i in 0..3 {
        let result = stage.check(&mut ctx!(), &ev("task.resume", payload));
        assert!(
            result.is_ok(),
            "task.resume retry #{i} must be accepted within budget: {:?}",
            result
        );
    }

    // Fifth emit exhausts the budget — rejected with
    // `repair_unrecoverable_after_*` (the P1-1
    // escalation marker).
    let result = stage.check(&mut ctx!(), &ev("task.resume", payload));
    let reject = result.expect_err("fifth task.resume must be rejected");
    assert!(
        reject
            .reason_code
            .starts_with("repair_unrecoverable_after_"),
        "P1-4: reject reason_code must be the P1-1 escalation marker, got: {}",
        reject.reason_code
    );
}

#[test]
fn task_resume_is_not_a_repair_topic_so_main_bus_path_preserved() {
    // The dispatcher routes events whose topic is in
    // REPAIR_TOPICS to the repair sink. `task.resume`
    // MUST stay off that list (it carries payload the
    // targeted hat consumes via `take_pending`).
    assert!(
        !is_repair_topic("task.resume"),
        "task.resume must NOT be routed to the repair sink"
    );
    assert!(
        is_budget_tracked_topic("task.resume"),
        "task.resume must remain on the budget-tracked list (P1-4)"
    );
}
