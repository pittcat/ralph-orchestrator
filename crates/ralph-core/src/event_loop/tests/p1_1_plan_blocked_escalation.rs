//! P1-1 (2026-06-28 review): when the `RepairDispatchStage`
//! rejects an event because the per-task budget is exhausted,
//! `EventLoop::record_stage_rejection` must escalate by
//! publishing a synthesised `plan.blocked` on the main bus.
//! Without this escalation the loop silently accumulates
//! rejection envelopes and the operator sees no `plan.blocked`
//! until `max_iterations` finally fires — the exact behaviour
//! that produced the 2026-06-26 4/8 incident.
//!
//! These tests drive `record_stage_rejection` directly
//! (it is `pub(crate)` precisely so this path is testable
//! without round-tripping through JSONL ingest) and assert
//! the bus observed a `plan.blocked` carrying the budget
//! exhaustion reason.

use super::*;
use crate::event_loop::stage_pipeline::StageReject;
use ralph_proto::HatId;

fn empty_event_loop() -> EventLoop {
    let config = RalphConfig::default();
    EventLoop::with_diagnostics(config, crate::diagnostics::DiagnosticsCollector::disabled())
}

fn drain_plan_blocked(event_loop: &mut EventLoop, hat: &HatId) -> Vec<String> {
    // Ensure the target hat is registered AND subscribed to
    // `plan.blocked`. The bus routes events to hats that
    // have a matching subscription (see
    // `EventBus::publish`); without an explicit subscribe
    // the synthesised event is silently dropped because
    // no hat has `plan.blocked` in its subscription list.
    let hat_typed = ralph_proto::Hat::new(hat.clone(), "observer").subscribe("plan.blocked");
    event_loop.bus.register(hat_typed);
    let pending = event_loop.bus.take_pending(hat);
    pending
        .into_iter()
        .filter(|e| e.topic.as_str() == "plan.blocked")
        .map(|e| e.payload.as_str().to_string())
        .collect()
}

#[test]
fn p1_1_budget_exhaustion_rejection_publishes_plan_blocked() {
    let mut event_loop = empty_event_loop();
    let hat = HatId::new("observer");
    // Subscribe BEFORE calling `record_stage_rejection` so
    // the synthesised event lands in the pending queue
    // when `bus.publish` is invoked from inside the
    // rejection path.
    event_loop
        .bus
        .register(ralph_proto::Hat::new(hat.clone(), "observer").subscribe("plan.blocked"));
    let event = ralph_proto::Event::new(
        "task.resume",
        r#"{"task_key":"alpha","reason":"stall_no_events"}"#,
    );
    let reject = StageReject::new("RepairDispatch", "repair_unrecoverable_after_3_retries");

    event_loop.record_stage_rejection(&event, &reject);

    let payloads = drain_plan_blocked(&mut event_loop, &hat);
    assert_eq!(
        payloads.len(),
        1,
        "P1-1: exactly one plan.blocked must be published on the main bus after budget exhaustion"
    );
    let payload: serde_json::Value =
        serde_json::from_str(&payloads[0]).expect("blocked payload must be JSON");
    assert_eq!(
        payload.get("reason").and_then(|v| v.as_str()),
        Some("repair_unrecoverable_after_3_retries"),
        "plan.blocked payload must carry the budget exhaustion reason code"
    );
    assert_eq!(
        payload.get("topic").and_then(|v| v.as_str()),
        Some("task.resume"),
        "plan.blocked payload must reference the rejected topic"
    );
    assert_eq!(
        payload.get("stage").and_then(|v| v.as_str()),
        Some("RepairDispatch"),
        "plan.blocked payload must name the rejecting stage"
    );
}

#[test]
fn p1_1_non_budget_rejection_does_not_publish_plan_blocked() {
    // A regular stage rejection (e.g. schema gate
    // missing-field) must NOT trigger the P1-1
    // escalation — only budget exhaustion does. The
    // pre-P1-1 behaviour (a recovery envelope alone) is
    // preserved for every other reason code.
    let mut event_loop = empty_event_loop();
    let hat = HatId::new("observer");
    let event = ralph_proto::Event::new("work.ready", r#"{"task_id":"t1"}"#);
    let reject = StageReject::new("EmitSchemaGate", "missing_required_fields");

    event_loop.record_stage_rejection(&event, &reject);

    let payloads = drain_plan_blocked(&mut event_loop, &hat);
    assert!(
        payloads.is_empty(),
        "non-budget rejections must NOT publish plan.blocked (P1-1 only escalates repair budget exhaustion)"
    );
}
