//! U6 (2026-06-27 mechanism foundation) — `StagePipeline`
//! wiring into `EventLoop::publish_event`.
//!
//! These tests pin the two U6 contracts:
//!
//! 1. Every hat-emitted event flows through the locked default
//!    stage pipeline before reaching the `EventBus`. A
//!    well-formed `work.done` (with all required fields) is
//!    accepted and routed to the bus without writing a
//!    recovery envelope.
//! 2. A malformed event (e.g. `plan.blocked` missing the
//!    `reason` field) is rejected by the `EmitSchemaGate`
//!    stage and the rejection is surfaced via
//!    `record_recovery_envelope` into `recovery.jsonl`.

use super::*;

/// U6 happy path: a complete `work.done` payload (with the
/// schema gate's required `task_id` field) passes through
/// every stage and reaches the bus.
#[test]
fn u6_publish_event_runs_through_stage_pipeline() {
    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();

    let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
hats:
  planner:
    name: "Planner"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("U6 wiring happy path");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Publish a complete `work.done` with a `task_id` field.
    let event = Event::new("work.done", r#"{"task_id":"task-u6-ok","note":"complete"}"#);
    event_loop.publish_event(event);
}

/// U6 rejection path: `plan.blocked` with no `reason` field
/// is rejected by `EmitSchemaGate`. The rejection must
/// surface as a `RecoveryDiagnosisEnvelope` written to
/// `recovery.jsonl` with `source = CliEmit`.
#[test]
fn u6_publish_event_rejects_missing_required_field() {
    use crate::diagnosis::DiagnosisSource;

    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();

    let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
hats:
  planner:
    name: "Planner"
    triggers: ["work.start"]
    publishes: ["plan.blocked"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let session_dir = diagnostics.session_dir().unwrap().to_path_buf();
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("U6 wiring missing required field");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Empty payload — missing the `reason` field that the
    // default schema gate requires on `plan.blocked`.
    let event = Event::new("plan.blocked", r#"{}"#);
    event_loop.publish_event(event);

    // Pin: a `RecoveryDiagnosisEnvelope` was written with
    // `source = CliEmit` (the U6 reused source bucket) and a
    // stage-pipeline prefix in the message.
    let recovery_path = session_dir.join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
    let entries: Vec<crate::diagnosis::RecoveryJournalEntry> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let u6_env = entries
        .iter()
        .find(|e| {
            e.envelope.source == DiagnosisSource::CliEmit
                && e.envelope.message.starts_with("stage '")
        })
        .map(|e| &e.envelope)
        .unwrap_or_else(|| {
            panic!(
                "U6: stage_pipeline rejection envelope not found; got entries: {:?}",
                entries
                    .iter()
                    .map(|e| (
                        e.envelope.source,
                        e.envelope.reason_code.clone(),
                        e.envelope.message.clone()
                    ))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(u6_env.topic.as_deref(), Some("plan.blocked"));
    assert_eq!(
        u6_env.reason_code, "missing_required_fields",
        "U6: schema gate reason code for missing-field rejection"
    );
}
