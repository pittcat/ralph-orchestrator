//! U2 (2026-06-27 mechanism foundation completion):
//! `EventLoop::publish_event` routes through the
//! `evaluate_emit_gate` facade introduced in U1.
//!
//! These tests pin the U2 contracts:
//!
//! 1. AcceptMainBus → `bus.publish(event)` runs (the event
//!    is observable on the bus).
//! 2. Reject → no bus event, recovery envelope written
//!    with `missing_required_fields` reason code.
//! 3. AcceptRepairStream → bus NEVER receives the topic;
//!    U2 placeholder counter records the early return
//!    (U6 will replace the counter with the real repair
//!    sink).

use super::*;

fn build_loop_for_u2(workspace: &std::path::Path) -> EventLoop {
    let events_path = workspace.join("events.jsonl");
    let diagnostics_root = workspace.to_path_buf();
    let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
hats:
  planner:
    name: "Planner"
    triggers: ["work.start"]
    publishes: ["plan.blocked", "work.done", "task.relocate_legacy"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("U2 publish_event facade");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop
}

/// Shared helper: register an observer on the bus that
/// records every published event. Tests then assert the
/// presence/absence of topics on the bus.
fn record_bus_topics(event_loop: &mut EventLoop) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
    let captured: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let cap_clone = captured.clone();
    event_loop.bus.add_observer(move |event| {
        cap_clone.lock().unwrap().push(event.topic.to_string());
    });
    captured
}

#[test]
fn u2_publish_event_full_plan_blocked_routes_to_bus() {
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u2(temp.path());
    let captured = record_bus_topics(&mut event_loop);

    let event = Event::new("plan.blocked", r#"{"reason":"blocked by unit fail"}"#);
    event_loop.publish_event(event);

    let bus_topics = captured.lock().unwrap().clone();
    assert!(
        bus_topics.iter().any(|t| t == "plan.blocked"),
        "expected plan.blocked on bus, got {bus_topics:?}"
    );
}

#[test]
fn u2_publish_event_empty_plan_blocked_rejected_to_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u2(temp.path());
    let captured = record_bus_topics(&mut event_loop);

    // Pin: `plan.blocked` with NO `reason` field is
    // rejected by the schema gate; bus does NOT see the
    // event; recovery envelope contains the
    // stage-rejection signature.
    let event = Event::new("plan.blocked", r#"{}"#);
    event_loop.publish_event(event);

    let bus_topics = captured.lock().unwrap().clone();
    assert!(
        !bus_topics.iter().any(|t| t == "plan.blocked"),
        "plan.blocked should have been rejected, but bus has {bus_topics:?}"
    );

    let session_dir = event_loop
        .diagnostics
        .session_dir()
        .expect("session dir")
        .to_path_buf();
    let recovery_path = session_dir.join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
    assert!(
        content.contains("missing_required_fields"),
        "expected missing_required_fields in recovery.jsonl, got: {content}"
    );
}

#[test]
fn u2_publish_event_full_work_done_routes_to_bus() {
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u2(temp.path());
    let captured = record_bus_topics(&mut event_loop);

    let event = Event::new("work.done", r#"{"task_id":"task-u2-ok"}"#);
    event_loop.publish_event(event);

    let bus_topics = captured.lock().unwrap().clone();
    assert!(
        bus_topics.iter().any(|t| t == "work.done"),
        "expected work.done on bus, got {bus_topics:?}"
    );
}

#[test]
fn u2_publish_event_repair_topic_routes_to_placeholder_not_bus() {
    // U2 placeholder: repair topics are routed to a
    // counter / log line instead of the bus. The
    // contract pinned here is:
    //   (a) the bus does NOT see the topic
    //   (b) `repair_stream_pending` counter is >= 1
    //   (c) recovery.jsonl is empty for the repair topic
    //       (U6 will write the real repair sink).
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u2(temp.path());
    let captured = record_bus_topics(&mut event_loop);

    let event = Event::new("task.relocate_legacy", r#"{"task_key":"legacy-1"}"#);
    event_loop.publish_event(event);

    let bus_topics = captured.lock().unwrap().clone();
    assert!(
        !bus_topics.iter().any(|t| t == "task.relocate_legacy"),
        "repair topic must not reach the main bus, but bus has {bus_topics:?}"
    );

    // U7 (2026-06-27-002 plan completion): the
    // `AcceptRepairStream` path writes to
    // `<workspace>/recovery.jsonl` (U6 sink), not the
    // session_dir-scoped file the U6 stage-rejection path
    // uses. Both paths share the `REPAIR_SINK_REASON_CODE`
    // constant so the contract is identical at the
    // reason_code level.
    let workspace_recovery = temp.path().join("recovery.jsonl");
    let content = std::fs::read_to_string(&workspace_recovery)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", workspace_recovery.display()));
    assert!(
        content.contains(crate::event_loop::repair_stream_sink::REPAIR_SINK_REASON_CODE),
        "U7: repair stream should write a stable reason_code to recovery.jsonl, got: {content}"
    );

    // U2 placeholder counter was retired in U7. The
    // U7 sink (recovery.jsonl) is the new contract.
    let _ = event_loop.repair_stream_pending; // field kept for diagnostics
}