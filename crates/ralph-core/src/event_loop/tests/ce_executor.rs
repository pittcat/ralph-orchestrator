//! Tests for ce_executor.

use super::*;

#[test]
fn test_ce_executor_review_passed_routes_to_plan_gate_not_shipper() {
    // R11 regression: After review.passed, plan-gate must be the active hat.
    // Shipper must NOT activate on review.passed. Reporter must NOT activate.
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready", "queue.advance"]
    publishes: ["work.done"]
    instructions: "EXECUTOR MODE — Implement the task."
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["work.done"]
    publishes: ["review.passed", "review.complete"]
    instructions: "SYNTHESIZER MODE — Merge findings."
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed", "review.complete"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    default_publishes: "plan.blocked"
    instructions: "PLAN GATE MODE — Decide queue.advance vs plan.complete."
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked", "fix.exhausted"]
    publishes: ["REVIEW_COMPLETE"]
    default_publishes: "REVIEW_COMPLETE"
    instructions: "SHIPPER MODE — Final validation and commit."
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"
    instructions: "REPORTER MODE — Generate manager report."
event_loop:
  starting_event: "work.ready"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Simulate review.passed arriving at the bus (as if review-synthesizer emitted it)
    event_loop.bus.publish(Event::new(
        "review.passed",
        r#"{"plan_name":"test","task_id":"t1","task_key":"k1","step":"1"}"#,
    ));

    // next_hat should return ralph (the constant coordinator)
    assert_eq!(
        event_loop.next_hat().unwrap().as_str(),
        "ralph",
        "ralph should coordinate after review.passed"
    );

    // build_prompt should activate plan-gate, NOT shipper or reporter
    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        prompt.contains("PLAN GATE MODE"),
        "review.passed should route to plan-gate. prompt did not contain 'PLAN GATE MODE'"
    );
    assert!(
        !prompt.contains("SHIPPER MODE"),
        "review.passed should NOT route to shipper. prompt contained 'SHIPPER MODE'"
    );
    assert!(
        !prompt.contains("REPORTER MODE"),
        "review.passed should NOT route to reporter. prompt contained 'REPORTER MODE'"
    );
}

#[test]
fn test_ce_executor_queue_advance_routes_to_executor_not_reporter() {
    // R11 regression: After plan-gate emits queue.advance, executor must activate.
    // Reporter/shipper must NOT activate on queue.advance.
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready", "queue.advance"]
    publishes: ["work.done"]
    instructions: "EXECUTOR MODE — Implement the task."
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    default_publishes: "plan.blocked"
    instructions: "PLAN GATE MODE — Decide queue.advance vs plan.complete."
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked"]
    publishes: ["REVIEW_COMPLETE"]
    default_publishes: "REVIEW_COMPLETE"
    instructions: "SHIPPER MODE — Final validation and commit."
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"
    instructions: "REPORTER MODE — Generate manager report."
event_loop:
  starting_event: "work.ready"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Simulate plan-gate emitting queue.advance
    event_loop.bus.publish(Event::new(
        "queue.advance",
        r#"{"plan_name":"test","next_step":"2","reviewed_task_id":"t1","reviewed_task_key":"k1"}"#,
    ));

    assert_eq!(
        event_loop.next_hat().unwrap().as_str(),
        "ralph",
        "ralph should coordinate after queue.advance"
    );

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        prompt.contains("EXECUTOR MODE"),
        "queue.advance should route to executor. prompt did not contain 'EXECUTOR MODE'"
    );
    assert!(
        !prompt.contains("SHIPPER MODE"),
        "queue.advance should NOT route to shipper. prompt contained 'SHIPPER MODE'"
    );
    assert!(
        !prompt.contains("REPORTER MODE"),
        "queue.advance should NOT route to reporter. prompt contained 'REPORTER MODE'"
    );
}

#[test]
fn test_ce_executor_review_complete_fail_routes_to_plan_gate_not_shipper() {
    // R11 regression: review.complete with verdict=fail must route to plan-gate,
    // NOT directly to shipper. Shipper only activates on plan.blocked/plan.complete.
    let yaml = r#"
hats:
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["work.done"]
    publishes: ["review.passed", "review.failed", "review.complete"]
    instructions: "SYNTHESIZER MODE — Merge findings."
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed", "review.complete"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    default_publishes: "plan.blocked"
    instructions: "PLAN GATE MODE — Decide queue.advance vs plan.complete."
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked", "fix.exhausted"]
    publishes: ["REVIEW_COMPLETE"]
    default_publishes: "REVIEW_COMPLETE"
    instructions: "SHIPPER MODE — Final validation and commit."
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"
    instructions: "REPORTER MODE — Generate manager report."
event_loop:
  starting_event: "work.ready"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.bus.publish(Event::new(
        "review.complete",
        r#"{"plan_name":"test","verdict":"fail","task_id":"t1","task_key":"k1","step":"1"}"#,
    ));

    assert_eq!(
        event_loop.next_hat().unwrap().as_str(),
        "ralph",
        "ralph should coordinate after review.complete"
    );

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        prompt.contains("PLAN GATE MODE"),
        "review.complete(verdict=fail) should route to plan-gate. prompt did not contain 'PLAN GATE MODE'"
    );
    assert!(
        !prompt.contains("SHIPPER MODE"),
        "review.complete should NOT route directly to shipper. prompt contained 'SHIPPER MODE'"
    );
    assert!(
        !prompt.contains("REPORTER MODE"),
        "review.complete should NOT route to reporter. prompt contained 'REPORTER MODE'"
    );
}

#[test]
fn test_ce_executor_plan_blocked_routes_to_shipper_not_reporter() {
    // R11 regression: plan.blocked must route to shipper, which publishes REVIEW_COMPLETE.
    // Reporter must NOT activate on plan.blocked directly.
    let yaml = r#"
hats:
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.complete"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    default_publishes: "plan.blocked"
    instructions: "PLAN GATE MODE — Decide queue.advance vs plan.complete."
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked", "fix.exhausted"]
    publishes: ["REVIEW_COMPLETE"]
    default_publishes: "REVIEW_COMPLETE"
    instructions: "SHIPPER MODE — Final validation and commit."
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"
    instructions: "REPORTER MODE — Generate manager report."
event_loop:
  starting_event: "work.ready"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.bus.publish(Event::new(
        "plan.blocked",
        r#"{"plan_name":"test","reason":"state mismatch","task_id":"t1","task_key":"k1"}"#,
    ));

    assert_eq!(
        event_loop.next_hat().unwrap().as_str(),
        "ralph",
        "ralph should coordinate after plan.blocked"
    );

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        prompt.contains("SHIPPER MODE"),
        "plan.blocked should route to shipper. prompt did not contain 'SHIPPER MODE'"
    );
    assert!(
        !prompt.contains("REPORTER MODE"),
        "plan.blocked should NOT route directly to reporter. prompt contained 'REPORTER MODE'"
    );
}
