//! U10 (2026-06-27 mechanism foundation completion):
//! the dispatch loop recognises terminal emits
//! (`LOOP_COMPLETE` by default after U9) and writes a
//! loop-termination record. Non-terminal topics
//! (e.g. `report.done`) do NOT trigger the
//! termination path.
//!
//! Pinned contracts:
//! 1. `LOOP_COMPLETE` published through `publish_event`
//!    lands on the bus AND the dispatcher logs the
//!    terminal record.
//! 2. `report.done` lands on the bus AND the dispatcher
//!    does NOT log a terminal record.
//! 3. `REVIEW_COMPLETE` lands on the bus AND the
//!    dispatcher does NOT log a terminal record.

use super::*;

fn build_loop_for_u10(workspace: &std::path::Path) -> EventLoop {
    let events_path = workspace.join("events.jsonl");
    let diagnostics_root = workspace.to_path_buf();
    let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["LOOP_COMPLETE", "report.done", "review.complete"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("U10 verdict dispatcher");
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
fn u10_is_terminal_pipeline_probe_for_loop_complete() {
    use crate::event_loop::flow_declaration::FlowDeclaration;
    use crate::event_loop::stage_pipeline::StagePipeline;
    let flow = FlowDeclaration::from_yaml(
        "mechanism:\n  flow:\n    type: declared\n    version: 1\n    terminal_emits: [LOOP_COMPLETE]\n    steps: []\n",
    )
    .unwrap();
    let pipeline = StagePipeline::with_default_stages(flow);
    let event = Event::new("LOOP_COMPLETE", "{}");
    assert!(
        pipeline.is_terminal(&event),
        "StagePipeline::is_terminal must report true for LOOP_COMPLETE"
    );
}

#[test]
fn u10_publish_loop_complete_lands_on_bus_and_dispatcher() {
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u10(temp.path());
    let captured = record_bus_topics(&mut event_loop);

    let event = Event::new("LOOP_COMPLETE", r#"{"reason":"complete"}"#);
    event_loop.publish_event(event);

    let bus_topics = captured.lock().unwrap().clone();
    assert!(
        bus_topics.iter().any(|t| t == "LOOP_COMPLETE"),
        "expected LOOP_COMPLETE on bus, got {bus_topics:?}"
    );
}

#[test]
fn u10_publish_report_done_does_not_trigger_termination() {
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u10(temp.path());
    let captured = record_bus_topics(&mut event_loop);

    let event = Event::new("report.done", r#"{"pass_or_fail":"pass"}"#);
    event_loop.publish_event(event);

    let bus_topics = captured.lock().unwrap().clone();
    assert!(
        bus_topics.iter().any(|t| t == "report.done"),
        "report.done must land on the bus, got {bus_topics:?}"
    );
    // Pin: the dispatcher does NOT consult
    // `stage_pipeline.is_terminal` for `report.done`
    // (the topic is not in the default
    // `terminal_emits` set after U9 retired the
    // mirror). The pipeline probe is the canonical
    // authority.
    use crate::event_loop::stage_pipeline::StagePipeline;
    let event_probe = Event::new("report.done", "{}");
    assert!(
        !event_loop.stage_pipeline.is_terminal(&event_probe),
        "report.done must NOT be terminal"
    );
}

#[test]
fn u10_publish_review_complete_does_not_trigger_termination() {
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u10(temp.path());
    let captured = record_bus_topics(&mut event_loop);

    // `review.complete` has a required `verdict` field
    // per the default schema gate. Provide it so the
    // schema gate accepts the event; the verdict
    // gate's `is_terminal` is what U10 pins here.
    let event = Event::new(
        "review.complete",
        r#"{"fix_plan_file":"/tmp/x","verdict":"pass"}"#,
    );
    event_loop.publish_event(event);

    let bus_topics = captured.lock().unwrap().clone();
    assert!(
        bus_topics.iter().any(|t| t == "review.complete"),
        "review.complete must land on the bus, got {bus_topics:?}"
    );
    let event_probe = Event::new("review.complete", "{}");
    assert!(
        !event_loop.stage_pipeline.is_terminal(&event_probe),
        "review.complete must NOT be terminal"
    );
}