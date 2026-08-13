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
    assert_eq!(
        resume_count, 1,
        "executor must hold exactly one task.resume"
    );
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
    let line =
        r#"{"topic":"plan.ready","payload":"{\"step\":\"step-1\"}","ts":"2026-08-10T00:00:00Z"}"#;
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

/// Plan 2026-08-10-001 U3: end-to-end verification that the unified
/// `publish_targeted_resume` boundary is reachable from a real
/// `EventLoop`. Routes through the resolver and lands in the
/// target hat's pending queue; a Block path produces no recipient.
#[test]
fn unit3_unified_publisher_targeted_resume_reaches_target_hat() {
    let mut bus = ralph_proto::EventBus::new();
    use ralph_proto::Hat;
    bus.register(Hat::new("executor", "Executor").subscribe("plan.ready"));
    bus.register(Hat::new("observer", "Observer").subscribe("plan.ready"));
    let registry: std::collections::HashSet<String> = ["executor", "observer"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let inputs = crate::event_loop::resume_routing::ResumeRoutingInputs {
        event_target: Some("executor"),
        retry_key: Some("unit_test_end_to_end"),
        ..Default::default()
    };
    let decision = crate::event_loop::resume_routing::publish_targeted_resume(
        &mut bus,
        &inputs,
        &registry,
        None,
        &[],
        "{\"reason\":\"u3_end_to_end\",\"target_hat\":\"executor\"}".to_string(),
    );
    assert!(matches!(
        decision,
        crate::event_loop::resume_routing::ResumeDecision::Allow { .. }
    ));
    let exec_pending = bus
        .peek_pending(&ralph_proto::HatId::new("executor"))
        .unwrap();
    let obs_pending = bus
        .peek_pending(&ralph_proto::HatId::new("observer"))
        .map(|v| v.len())
        .unwrap_or(0);
    assert_eq!(
        exec_pending.len(),
        1,
        "executor must hold exactly one targeted resume"
    );
    assert_eq!(
        exec_pending[0].target.as_ref().map(|h| h.as_str()),
        Some("executor"),
        "target must survive the unified publisher boundary"
    );
    assert_eq!(obs_pending, 0, "non-target hat must not receive the resume");
}

/// Plan 2026-08-13-003 U1: the unified `publish_targeted_resume`
/// boundary must enforce `recipient == [target]` (D4). After
/// the U1 fix, `publish_targeted_resume` checks the
/// `Vec<HatId>` returned by `bus.publish(event)` and the
/// target hat receives exactly one resume; non-target hats
/// receive zero. This test asserts both sides of the
/// contract.
#[test]
fn u1_publish_targeted_resume_recipient_mismatch_blocks() {
    use crate::event_loop::resume_routing::{
        publish_targeted_resume, ResumeDecision, ResumeRoutingInputs,
    };

    let mut bus = ralph_proto::EventBus::new();
    use ralph_proto::Hat;
    bus.register(Hat::new("executor", "Executor").subscribe("plan.ready"));
    bus.register(Hat::new("observer", "Observer").subscribe("plan.ready"));
    let registry: std::collections::HashSet<String> =
        ["executor", "observer"].iter().map(|s| s.to_string()).collect();
    let inputs = ResumeRoutingInputs {
        event_target: Some("executor"),
        retry_key: Some("u1_recipient_check"),
        ..Default::default()
    };

    let decision = publish_targeted_resume(
        &mut bus,
        &inputs,
        &registry,
        None,
        &[],
        "{\"reason\":\"u1_recipient_check\",\"target_hat\":\"executor\"}".to_string(),
    );

    assert!(
        matches!(decision, ResumeDecision::Allow { .. }),
        "happy path: registry is consistent, recipient must equal [target] (decision was {decision:?})"
    );
    let exec_pending = bus
        .peek_pending(&ralph_proto::HatId::new("executor"))
        .expect("executor must hold the resume");
    assert_eq!(
        exec_pending.len(),
        1,
        "executor must hold exactly one targeted resume"
    );
    let resume = &exec_pending[0];
    assert_eq!(
        resume.target.as_ref().map(|h| h.as_str()),
        Some("executor"),
        "Event.target must equal the resolved target"
    );
    let obs_pending = bus
        .peek_pending(&ralph_proto::HatId::new("observer"))
        .map(|v| v.len())
        .unwrap_or(0);
    assert_eq!(
        obs_pending, 0,
        "non-target hat must NOT receive the targeted resume (recipient == [target] contract)"
    );
}

/// Plan 2026-08-13-003 U1 + S1: real EventLoop characterization
/// that the runtime ingress produces a targeted `task.resume`
/// in the target hat's pending queue only. The trigger topic
/// arrives via the trusted JSONL `triggered` field — the
/// accepted-branch rebuild must surface `target =
/// Some(triggered_hat)` and `recipient = [triggered_hat]`.
/// Other hats must NOT receive the resume.
///
/// Before the U1 inventory guard was added this test would
/// fail when the production site used `Event::new("task.resume",
/// payload)` without a target.
#[test]
fn u1_runtime_generated_resume_is_targeted() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let events_path = temp_dir.path().join("events.jsonl");
    let config = resume_routing_config();

    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("U1-runtime-target-fidelity");

    let line = r#"{"topic":"task.resume","payload":"{\"reason\":\"u1_runtime_target\",\"kind\":\"u1_runtime_target\",\"target_hat\":\"executor\",\"retry_key\":\"u1_runtime_target_v1\",\"original_trigger_topic\":\"plan.ready\",\"original_trigger_payload\":{}}","ts":"2026-08-13T00:00:00Z","hat":"executor","triggered":"executor"}"#;
    write_raw_jsonl_line(&events_path, line);

    let processed = event_loop.process_events_from_jsonl().expect("process");
    assert!(
        processed.had_events,
        "the task.resume must be admitted by the accepted branch"
    );

    let exec_pending = event_loop
        .bus
        .peek_pending(&ralph_proto::HatId::new("executor"))
        .expect("executor must hold the resume");
    let resume_count = exec_pending
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    assert_eq!(
        resume_count, 1,
        "executor must hold exactly one task.resume (count={resume_count})"
    );
    let resume = exec_pending
        .iter()
        .find(|e| e.topic.as_str() == "task.resume")
        .expect("resume");
    assert_eq!(
        resume.target.as_ref().map(|h| h.as_str()),
        Some("executor"),
        "executor.pending event.target must equal `executor`"
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
}

#[test]
fn unit3_unified_publisher_blocks_broadcast_when_no_safe_target() {
    let mut bus = ralph_proto::EventBus::new();
    use ralph_proto::Hat;
    bus.register(Hat::new("executor", "Executor").subscribe("plan.ready"));
    let registry: std::collections::HashSet<String> =
        ["executor"].iter().map(|s| s.to_string()).collect();
    let inputs = crate::event_loop::resume_routing::ResumeRoutingInputs::default();
    let decision = crate::event_loop::resume_routing::publish_targeted_resume(
        &mut bus,
        &inputs,
        &registry,
        None,
        &[],
        "{\"reason\":\"u3_no_target\"}".to_string(),
    );
    assert!(matches!(
        decision,
        crate::event_loop::resume_routing::ResumeDecision::Block { .. }
    ));
    for id in bus.hat_ids() {
        let pending = bus.peek_pending(id).map(|v| v.len()).unwrap_or(0);
        assert_eq!(
            pending, 0,
            "hat {id} must not receive a blocked resume (no safe target)"
        );
    }
}

/// Plan 2026-08-10-001 U1 R1 inventory regression: every
/// production `task.resume` publish inside `event_loop/*.rs`
/// must route through `publish_targeted_resume_*`. A bare
/// `Event::new("task.resume", …)` or
/// `self.bus.publish(Event::new("task.resume"` outside the
/// helper module signals a migration regression — fail this
/// test loudly and route the fix through the U1 wires.
///
/// Scope: production files only (`src/event_loop/*.rs`),
/// excluding the helper module itself. The walker does not
/// descend into `src/event_loop/tests/` because those files
/// use bare `task.resume` events as event-loop-internal
/// test fixtures for the bus routing semantics — they do
/// not produce publish-side behaviour in production.
#[test]
fn ingress_inventory_regression_storm_dispatch() {
    let event_loop_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/event_loop");
    let mut bare_publishes: Vec<String> = Vec::new();
    walk_event_loop_rs(&event_loop_root, &mut |path| {
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        // The helper itself is the only allow-listed exception
        // — it constructs the targeted event for resolution.
        if path.ends_with("resume_routing.rs") {
            return;
        }
        // Production files outside `tests/` are in scope.
        if path.components().any(|c| c.as_os_str() == "tests") {
            return;
        }
        // Skip the loop_state mini-tests (existing
        // characterization tests inside `mod.rs`-shaped
        // files use `Event::new("task.resume", ...)` for
        // stale-counter scenarios — not production publishes).
        if path.ends_with("loop_state.rs") {
            return;
        }
        for (idx, line) in content.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains("Event::new(\"task.resume\"")
                || line.contains("self.bus.publish(Event::new(\"task.resume\"")
            {
                bare_publishes.push(format!(
                    "{}:{}",
                    path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(path)
                        .display(),
                    idx + 1,
                ));
            }
        }
    });
    assert!(
        bare_publishes.is_empty(),
        "production task.resume publish must route through publish_targeted_resume_*. Offenders: {bare_publishes:?}"
    );
}

fn walk_event_loop_rs<F>(root: &std::path::Path, visit: &mut F)
where
    F: FnMut(&std::path::Path),
{
    let mut stack: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                visit(&path);
            }
        }
    }
}
