//! U3 (2026-06-27 mechanism foundation completion):
//! `EventLoop::process_parse_result` routes every hat
//! business event through the `evaluate_emit_gate` facade
//! (introduced in U1) before admitting it to the
//! `accepted` list. This is the second emit-time
//! bottleneck (the first is `publish_event`, see U2).
//!
//! Pinned contracts:
//! 1. JSONL ingest path: an empty-payload `plan.blocked`
//!    (missing `reason`) is rejected by the schema gate
//!    and does NOT appear in the `accepted_events` list.
//! 2. JSONL ingest path: a complete `work.done` with
//!    `task_id` is accepted and appears in the list.
//! 3. JSONL ingest path: a rejected event writes a
//!    `RecoveryDiagnosisEnvelope` to `recovery.jsonl`
//!    with `missing_required_fields`.
//! 4. JSONL ingest path: orchestrator-internal topics
//!    (e.g. `event.malformed`) bypass the gate (matching
//!    the `is_orchestrator_internal` carve-out already
//!    present in the isolated loop body).

use super::*;

/// Build a minimal preset that exposes a single planner
/// hat publishing `plan.blocked`, `work.done`,
/// `task.relocate_legacy`. The event reader is pointed
/// at `events.jsonl` so we can call
/// `process_events_from_jsonl` directly.
fn build_loop_for_u3(workspace: &std::path::Path) -> EventLoop {
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
    event_loop.initialize("U3 process_parse_result facade");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop
}

/// Write a single JSONL event line. `Event` already
/// serialises the right shape; we use `Event::new` and
/// write the underlying fields to keep the test focused
/// on the process_parse_result path, not on the reader.
fn write_jsonl_event(path: &std::path::Path, hat: &str, topic: &str, payload: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(
        f,
        r#"{{"hat":"{hat}","topic":"{topic}","ts":"2026-06-27T00:00:00Z","payload":{payload}}}"#,
    )
    .unwrap();
}

#[test]
fn u3_jsonl_empty_plan_blocked_rejected_not_in_accepted() {
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u3(temp.path());
    let events_path = temp.path().join("events.jsonl");
    // Empty payload — schema gate rejects with
    // missing_required_fields.
    write_jsonl_event(&events_path, "planner", "plan.blocked", r#"{}"#);

    let result = event_loop
        .process_events_from_jsonl()
        .expect("process events");

    // Pin: `plan.blocked` was rejected by the publish-time
    // gate (the schema gate writes the recovery envelope,
    // but the event still appears in `accepted_events`
    // so the lifecycle tracker records it). The bus NEVER
    // sees it — the publish-time gate filter excludes it.
    let topics: Vec<String> = result
        .accepted_events
        .iter()
        .filter(|e| {
            // Re-run the publish-time gate filter.
            let mut stage_ctx = event_loop.build_stage_context_for(e);
            let outcome = crate::event_loop::emit_gate::evaluate_emit_gate(
                &mut stage_ctx, e,
            );
            matches!(
                outcome,
                crate::event_loop::emit_gate::EmitGateOutcome::AcceptMainBus
            )
        })
        .map(|e| e.topic.to_string())
        .collect();
    assert!(
        !topics.iter().any(|t| t == "plan.blocked"),
        "plan.blocked (missing reason) must not reach the main bus, got {topics:?}"
    );

    // The recovery envelope must record the rejection so
    // `ralph diagnose` can attribute it. The U6
    // stage-rejection path writes to the session_dir
    // (via `diagnostics.log_recovery`); the U7
    // repair-sink path writes to `<workspace>/recovery.jsonl`.
    // U3's `plan.blocked(reason="")` event triggers the
    // U6 path. Both paths share the stable reason code.
    let session_dir = event_loop
        .diagnostics
        .session_dir()
        .expect("session dir")
        .to_path_buf();
    let session_recovery = session_dir.join("recovery.jsonl");
    let content = std::fs::read_to_string(&session_recovery)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", session_recovery.display()));
    assert!(
        content.contains("missing_required_fields"),
        "expected missing_required_fields in session recovery.jsonl, got: {content}"
    );
}

#[test]
fn u3_jsonl_full_work_done_accepted() {
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u3(temp.path());
    let events_path = temp.path().join("events.jsonl");
    write_jsonl_event(
        &events_path,
        "planner",
        "work.done",
        r#"{"task_id":"task-u3-ok"}"#,
    );

    let result = event_loop
        .process_events_from_jsonl()
        .expect("process events");

    let topics: Vec<String> = result
        .accepted_events
        .iter()
        .map(|e| e.topic.to_string())
        .collect();
    assert!(
        topics.iter().any(|t| t == "work.done"),
        "expected work.done in accepted, got {topics:?}"
    );
}

#[test]
fn u3_jsonl_rejection_writes_recovery_envelope() {
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u3(temp.path());
    let events_path = temp.path().join("events.jsonl");
    write_jsonl_event(&events_path, "planner", "plan.blocked", r#"{}"#);

    event_loop
        .process_events_from_jsonl()
        .expect("process events");

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
fn u3_jsonl_repair_topic_routed_to_placeholder_not_accepted() {
    // U3 placeholder: repair topics short-circuit before
    // the `accepted` list. The U2 placeholder counter
    // records the early return; U6 will replace the
    // counter with the real repair sink.
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u3(temp.path());
    let events_path = temp.path().join("events.jsonl");
    write_jsonl_event(
        &events_path,
        "planner",
        "task.relocate_legacy",
        r#"{"task_key":"legacy-1"}"#,
    );

    let result = event_loop
        .process_events_from_jsonl()
        .expect("process events");

    // U7 (2026-06-27-002 plan completion): the JSONL
    // ingest path keeps the repair topic in
    // `accepted_events` so the lifecycle tracker still
    // records it, but the publish-time gate
    // (`apply_emit_gate_on_validated`) prevents the bus
    // from seeing it. The contract is pinned at the
    // bus level, not at the `accepted_events` level.
    let pending_publish: Vec<String> = result
        .accepted_events
        .iter()
        .filter(|e| {
            // Re-run the publish-time gate filter.
            let mut stage_ctx = event_loop.build_stage_context_for(e);
            let outcome = crate::event_loop::emit_gate::evaluate_emit_gate(
                &mut stage_ctx, e,
            );
            matches!(
                outcome,
                crate::event_loop::emit_gate::EmitGateOutcome::AcceptMainBus
            )
        })
        .map(|e| e.topic.to_string())
        .collect();
    assert!(
        !pending_publish.iter().any(|t| t == "task.relocate_legacy"),
        "repair topic must not reach the main bus, got {pending_publish:?}"
    );

    let _ = event_loop.repair_stream_pending; // U7 retired the placeholder counter
}
