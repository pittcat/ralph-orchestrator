//! Plan 2026-08-10-001 Unit 1: JSONL `task.resume` target/metadata fidelity.
//!
//! Real EventLoop + EventReader + EventBus routing characterization.
//! A JSONL event with `triggered=executor` must drive the next
//! activation to `executor` even though the preset has no
//! `task.resume` trigger declared on executor; source/target/wave/
//! system_injected metadata must survive the accepted-branch
//! rebuild path in `process_parse_result`.
//!
//! Per D2 + E5/E13 the accepted branch rebuilds accepted events
//! with `Event::new(...)`, which previously dropped the metadata
//! that `From<Event> for ralph_proto::Event` (event_reader.rs:182)
//! already preserves. These tests pin the contract that rebuilds
//! must not drop that metadata.

use super::*;

fn resume_routing_config() -> crate::config::RalphConfig {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  executor:
    name: "Executor"
    triggers: ["plan.ready"]
    publishes: ["work.done"]
  observer:
    name: "Observer"
    triggers: ["plan.ready"]
    publishes: ["work.done"]
"#;
    serde_yaml::from_str(yaml).expect("resume_routing_config YAML must parse")
}

fn write_raw_jsonl_line(path: &std::path::Path, raw_line: &str) {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open events.jsonl");
    writeln!(f, "{raw_line}").expect("write JSONL line");
}

/// R1 + S1: `triggered=executor` JSONL `task.resume` lands in
/// `executor`'s pending queue with `target == Some(executor)`,
/// and `next_hat` returns `executor`. Other hats must not receive
/// the resume.
#[test]
fn jsonl_task_resume_preserves_target_and_activates_original_hat() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let events_path = temp_dir.path().join("events.jsonl");
    let config = resume_routing_config();

    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("U1-target-fidelity");

    let line = r#"{"topic":"task.resume","payload":"{\"reason\":\"u1_test_target_fidelity\",\"kind\":\"u1_test_target_fidelity\",\"target_hat\":\"executor\",\"original_trigger_topic\":\"plan.ready\",\"original_trigger_payload\":{}}","ts":"2026-08-10T00:00:00Z","hat":"executor","triggered":"executor"}"#;
    write_raw_jsonl_line(&events_path, line);

    let processed = event_loop.process_events_from_jsonl().expect("process");
    assert!(
        processed.had_events,
        "the task.resume must be admitted by the accepted branch"
    );

    let executor_pending = event_loop
        .bus
        .peek_pending(&ralph_proto::HatId::new("executor"))
        .expect("executor must hold the resume");
    let resume_count = executor_pending
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    assert_eq!(resume_count, 1, "executor must hold exactly one task.resume");
    let resume = executor_pending
        .iter()
        .find(|e| e.topic.as_str() == "task.resume")
        .expect("resume");
    assert_eq!(
        resume.target.as_ref().map(|h| h.as_str()),
        Some("executor"),
        "executor.pending event.target must equal `executor` (was lost in accepted rebuild)"
    );

    for other in ["observer", "ralph"] {
        let pending = event_loop
            .bus
            .peek_pending(&ralph_proto::HatId::new(other))
            .map(|events| {
                events
                    .iter()
                    .filter(|e| e.topic.as_str() == "task.resume")
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(
            pending, 0,
            "hat `{other}` must not receive the targeted resume"
        );
    }

    let next = event_loop
        .next_hat()
        .expect("next_hat must select a hat with pending work")
        .clone();
    assert_eq!(
        next.as_str(),
        "executor",
        "next_hat must return the targeted hat"
    );
}

/// R1 + characterization: `source` and `target` survive the
/// rebuild path that the JSONL-event-rebuild helper must use.
#[test]
fn metadata_copy_preserves_source_and_target() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let events_path = temp_dir.path().join("events.jsonl");
    let config = resume_routing_config();

    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("U1-metadata-copy");

    // `hat=executor` and `triggered=executor`: source AND target
    // are both set; the rebuild must not drop either.
    let line = r#"{"topic":"task.resume","payload":"{\"reason\":\"u1_test_metadata_copy\",\"kind\":\"u1_test_metadata_copy\",\"target_hat\":\"executor\",\"original_trigger_topic\":\"plan.ready\",\"original_trigger_payload\":{}}","ts":"2026-08-10T00:00:00Z","hat":"executor","triggered":"executor"}"#;
    write_raw_jsonl_line(&events_path, line);

    let _ = event_loop.process_events_from_jsonl().expect("process");

    let pending = event_loop
        .bus
        .peek_pending(&ralph_proto::HatId::new("executor"))
        .expect("executor pending");
    let resume = pending
        .iter()
        .find(|e| e.topic.as_str() == "task.resume")
        .expect("resume event");
    assert_eq!(
        resume.source.as_ref().map(|h| h.as_str()),
        Some("executor"),
        "source must survive the accepted-branch rebuild"
    );
    assert_eq!(
        resume.target.as_ref().map(|h| h.as_str()),
        Some("executor"),
        "target must survive the accepted-branch rebuild"
    );
}

/// S1 isolation: targeted event must reach the target hat only.
#[test]
fn targeted_task_resume_only_reaches_target_hat() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let events_path = temp_dir.path().join("events.jsonl");
    let config = resume_routing_config();

    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("U1-targeted-only");

    let line = r#"{"topic":"task.resume","payload":"{\"reason\":\"u1_test_targeted_only\",\"kind\":\"u1_test_targeted_only\",\"target_hat\":\"executor\",\"original_trigger_topic\":\"plan.ready\",\"original_trigger_payload\":{}}","ts":"2026-08-10T00:00:00Z","hat":"executor","triggered":"executor"}"#;
    write_raw_jsonl_line(&events_path, line);

    let _ = event_loop.process_events_from_jsonl().expect("process");

    let observer_pending = event_loop
        .bus
        .peek_pending(&ralph_proto::HatId::new("observer"))
        .map(|events| {
            events
                .iter()
                .filter(|e| e.topic.as_str() == "task.resume")
                .count()
        })
        .unwrap_or(0);
    assert_eq!(
        observer_pending, 0,
        "observer must not receive a resume that targets executor"
    );
}

/// R8 + regression: ordinary business event without `triggered`
/// must still flow through subscription routing.
#[test]
fn ordinary_event_without_target_keeps_subscription_routing() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let events_path = temp_dir.path().join("events.jsonl");
    let config = resume_routing_config();

    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("U1-ordinary-event");

    // No `triggered`, no `target`. Both executor and observer
    // subscribe to `plan.ready`; the broadcast lands in both.
    let line = r#"{"topic":"plan.ready","payload":"{\"step\":\"step-1\"}","ts":"2026-08-10T00:00:00Z"}"#;
    write_raw_jsonl_line(&events_path, line);

    let _ = event_loop.process_events_from_jsonl().expect("process");

    let exec = event_loop
        .bus
        .peek_pending(&ralph_proto::HatId::new("executor"))
        .map(|events| {
            events
                .iter()
                .filter(|e| e.topic.as_str() == "plan.ready")
                .count()
        })
        .unwrap_or(0);
    let obs = event_loop
        .bus
        .peek_pending(&ralph_proto::HatId::new("observer"))
        .map(|events| {
            events
                .iter()
                .filter(|e| e.topic.as_str() == "plan.ready")
                .count()
        })
        .unwrap_or(0);
    assert!(
        exec >= 1 && obs >= 1,
        "subscription routing must still deliver plan.ready to both subscribed hats (exec={exec}, obs={obs})"
    );
}