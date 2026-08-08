//! U7 (2026-06-27 mechanism foundation completion):
//! the U6 `RepairStreamSink` is wired into both the
//! `publish_event` path (U2) and the `process_parse_result`
//! path (U3). The bus never receives a repair topic.
//!
//! Pinned contracts:
//! 1. `publish_event(task.relocate_legacy)` writes
//!    one `repair_dispatch` envelope to
//!    `<workspace>/recovery.jsonl` AND the bus does
//!    NOT see the topic.
//! 2. `process_parse_result` with a JSONL `task
//!    .relocate_legacy` line writes one envelope via
//!    the same path AND the bus does NOT see the topic.

use super::*;
use crate::event_loop::repair_stream_sink::REPAIR_SINK_REASON_CODE;

fn build_loop_for_u7(workspace: &std::path::Path) -> EventLoop {
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
    event_loop.initialize("U7 repair sink wiring");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop
}

fn record_bus_topics(event_loop: &mut EventLoop) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let cap_clone = captured.clone();
    event_loop.bus.add_observer(move |event| {
        cap_clone.lock().unwrap().push(event.topic.to_string());
    });
    captured
}

#[test]
fn u7_publish_event_repair_topic_writes_envelope_and_skips_bus() {
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u7(temp.path());
    let captured = record_bus_topics(&mut event_loop);

    let event = Event::new("task.relocate_legacy", r#"{"task_key":"legacy-1"}"#);
    event_loop.publish_event(event);

    let bus_topics = captured.lock().unwrap().clone();
    assert!(
        !bus_topics.iter().any(|t| t == "task.relocate_legacy"),
        "bus must not see repair topic, got {bus_topics:?}"
    );

    // Repair envelope written to workspace/.ralph/recovery.jsonl.
    let recovery_path = temp.path().join(".ralph").join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
    assert!(
        content.contains(REPAIR_SINK_REASON_CODE),
        "expected stable reason_code, got: {content}"
    );
    assert!(
        content.contains("task.relocate_legacy"),
        "expected envelope to mention topic, got: {content}"
    );
}

#[test]
fn u7_jsonl_repair_topic_writes_envelope_and_skips_bus() {
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u7(temp.path());
    let captured = record_bus_topics(&mut event_loop);

    let events_path = temp.path().join("events.jsonl");
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .unwrap();
    writeln!(
        f,
        r#"{{"hat":"planner","topic":"task.relocate_legacy","ts":"2026-06-27T00:00:00Z","payload":{{"task_key":"legacy-1"}}}}"#
    )
    .unwrap();

    event_loop
        .process_events_from_jsonl()
        .expect("process events");

    let bus_topics = captured.lock().unwrap().clone();
    assert!(
        !bus_topics.iter().any(|t| t == "task.relocate_legacy"),
        "bus must not see repair topic from JSONL, got {bus_topics:?}"
    );

    let recovery_path = temp.path().join(".ralph").join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
    assert!(
        content.contains(REPAIR_SINK_REASON_CODE),
        "expected stable reason_code, got: {content}"
    );
}
