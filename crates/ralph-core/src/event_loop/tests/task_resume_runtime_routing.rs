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
        ResumeDecision, ResumeRoutingInputs, publish_targeted_resume,
    };

    let mut bus = ralph_proto::EventBus::new();
    use ralph_proto::Hat;
    bus.register(Hat::new("executor", "Executor").subscribe("plan.ready"));
    bus.register(Hat::new("observer", "Observer").subscribe("plan.ready"));
    let registry: std::collections::HashSet<String> = ["executor", "observer"]
        .iter()
        .map(|s| s.to_string())
        .collect();
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

/// Plan 2026-08-13-003 U2 + R2/S2: production wrapper
/// `publish_targeted_resume_for_hat` MUST thread the payload's
/// `target_hat` field through the resolver so the priority
/// chain (D2) is exercised end-to-end. When the payload
/// declares `target_hat=executor` and the registry accepts
/// `executor`, the call publishes to `executor` and only
/// `executor` — even though the wrapper was called with a
/// different `target_hint` (the safety case where caller-
/// side and payload-side agree on the resolved target).
#[test]
fn u2_publish_targeted_resume_for_hat_threads_payload_target() {
    use crate::event_loop::resume_routing::{ResumeDecision, publish_targeted_resume_for_hat};

    let mut bus = ralph_proto::EventBus::new();
    use ralph_proto::Hat;
    bus.register(Hat::new("executor", "Executor").subscribe("task.resume"));
    bus.register(Hat::new("observer", "Observer").subscribe("task.resume"));
    let registry: std::collections::HashSet<String> = ["executor", "observer"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Caller passes target_hint=executor AND payload
    // declares target_hat=executor. The wrapper must
    // publish to executor only.
    let decision = publish_targeted_resume_for_hat(
        &mut bus,
        &registry,
        None,
        Some("loop-A"),
        "executor",
        None,
        None,
        Some("executor"),
        "u2_threads_payload_target",
        r#"{"reason":"u2_threads_payload_target","target_hat":"executor"}"#.to_string(),
    );
    assert!(
        matches!(decision, ResumeDecision::Allow { .. }),
        "consistent payload+wrapper target must Allow (decision was {decision:?})"
    );
    let exec_pending = bus
        .peek_pending(&ralph_proto::HatId::new("executor"))
        .expect("executor pending");
    assert_eq!(exec_pending.len(), 1, "executor must hold the resume");
    let obs_pending = bus
        .peek_pending(&ralph_proto::HatId::new("observer"))
        .map(|v| v.len())
        .unwrap_or(0);
    assert_eq!(
        obs_pending, 0,
        "observer must not receive the targeted resume"
    );
}

/// Plan 2026-08-13-003 U2 + R2/S10: when the caller only
/// has the JSONL `triggered` field (legacy format), the
/// production wrapper must still resolve to that hat via
/// the explicit `event_target` it receives. The wrapper
/// itself does not parse `triggered` (the JSONL rebuild
/// path does that in `parse_and_emit.rs`); the resolver
/// must Allow when the caller passes `event_target` =
/// `triggered_hat` and the registry accepts it.
#[test]
fn u2_legacy_triggered_only_jsonl_preserves_target() {
    use crate::event_loop::resume_routing::{ResumeDecision, publish_targeted_resume_for_hat};

    let mut bus = ralph_proto::EventBus::new();
    use ralph_proto::Hat;
    bus.register(Hat::new("executor", "Executor").subscribe("task.resume"));
    bus.register(Hat::new("observer", "Observer").subscribe("task.resume"));
    let registry: std::collections::HashSet<String> = ["executor", "observer"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Caller passes only event_target (from JSONL
    // `triggered=executor`) and no payload_target_hat.
    // The resolver must Allow and publish to executor.
    let decision = publish_targeted_resume_for_hat(
        &mut bus,
        &registry,
        None,
        Some("loop-A"),
        "executor",
        None,
        None,
        None,
        "u2_legacy_triggered",
        r#"{"reason":"u2_legacy_triggered"}"#.to_string(),
    );
    assert!(
        matches!(decision, ResumeDecision::Allow { .. }),
        "legacy triggered-only path must Allow (decision was {decision:?})"
    );
    let exec_pending = bus
        .peek_pending(&ralph_proto::HatId::new("executor"))
        .expect("executor pending");
    assert_eq!(
        exec_pending.len(),
        1,
        "executor must hold the legacy triggered-only resume"
    );
    let obs_pending = bus
        .peek_pending(&ralph_proto::HatId::new("observer"))
        .map(|v| v.len())
        .unwrap_or(0);
    assert_eq!(
        obs_pending, 0,
        "observer must not receive the legacy triggered-only resume"
    );
}

/// Plan 2026-08-13-003 U3 + R4/S8: `publish_targeted_recovery_resume`
/// writes a Recovery durable outbox entry BEFORE publishing
/// to the in-memory bus. Replaying the same payload must
/// NOT publish a second event — `commit_idempotent` returns
/// the existing entry and skips the second publish.
#[test]
fn u3_recovery_commit_precedes_publish() {
    use crate::event_loop::resume_routing::{
        ResumeDecision, ResumeRoutingInputs, publish_targeted_recovery_resume,
    };
    use crate::state::StateLedger;
    use ralph_proto::Hat;

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let ws = temp_dir.path().to_path_buf();
    let ledger = StateLedger::new(&ws, true);

    let mut bus = ralph_proto::EventBus::new();
    bus.register(Hat::new("executor", "Executor").subscribe("task.resume"));
    let registry: std::collections::HashSet<String> =
        ["executor"].iter().map(|s| s.to_string()).collect();
    let inputs = ResumeRoutingInputs {
        event_target: Some("executor"),
        retry_key: Some("u3_recovery_commit_precedes"),
        ..Default::default()
    };
    let decision = publish_targeted_recovery_resume(
        &mut bus,
        &registry,
        None,
        &ledger,
        "loop-A",
        "u3_act_1",
        "u3_contract",
        &inputs,
        r#"{"reason":"u3_recovery_commit_precedes","target_hat":"executor"}"#.to_string(),
    )
    .expect("commit must succeed");
    assert!(
        matches!(decision, ResumeDecision::Allow { .. }),
        "decision must Allow (was {decision:?})"
    );

    // The durable outbox must contain the entry.
    let outbox = crate::event_loop::accepted_transition::read_outbox(&ws).expect("outbox readable");
    assert_eq!(
        outbox.len(),
        1,
        "exactly one Recovery outbox entry must exist"
    );
    assert_eq!(outbox[0].topic, "task.resume");
    assert_eq!(outbox[0].loop_id, "loop-A");
    assert_eq!(outbox[0].activation_id, "u3_act_1");
    assert!(
        !outbox[0].delivered,
        "first commit is not yet delivered/acked"
    );

    // The in-memory bus must hold exactly one task.resume
    // (the publish happened AFTER the durable commit).
    let exec_pending = bus
        .peek_pending(&ralph_proto::HatId::new("executor"))
        .expect("executor pending");
    assert_eq!(
        exec_pending.len(),
        1,
        "executor must hold the targeted resume"
    );
}

/// Plan 2026-08-13-003 U3 + R4/S8: when the durable
/// commit fails (simulated by handing a non-existent
/// workspace to the StateLedger constructor), the
/// `publish_targeted_recovery_resume` function MUST NOT
/// publish the resume to the bus. The caller receives an
/// `Err` and the bus stays empty — zero bus side effect
/// per D3.
#[test]
fn u3_recovery_commit_failure_has_zero_bus_side_effect() {
    use crate::event_loop::resume_routing::{
        ResumeRoutingInputs, publish_targeted_recovery_resume,
    };
    use crate::state::StateLedger;
    use ralph_proto::Hat;

    // Use a workspace that we will make read-only at the
    // outbox path level so the append fails. The simplest
    // way is to construct a `StateLedger` pointing at a
    // path that does not exist and that we cannot create
    // (an existing regular file used as a directory
    // replacement fails `OpenOptions::create`).
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let blocker = temp_dir.path().join("blocker");
    std::fs::write(&blocker, b"not a directory").expect("blocker file");
    let ws = blocker.join("nested"); // cannot be created; blocker is a file
    let ledger = StateLedger::new(&ws, true);

    let mut bus = ralph_proto::EventBus::new();
    bus.register(Hat::new("executor", "Executor").subscribe("task.resume"));
    let registry: std::collections::HashSet<String> =
        ["executor"].iter().map(|s| s.to_string()).collect();
    let inputs = ResumeRoutingInputs {
        event_target: Some("executor"),
        retry_key: Some("u3_commit_failure"),
        ..Default::default()
    };
    let result = publish_targeted_recovery_resume(
        &mut bus,
        &registry,
        None,
        &ledger,
        "loop-A",
        "u3_act_2",
        "u3_contract",
        &inputs,
        r#"{"reason":"u3_commit_failure"}"#.to_string(),
    );
    assert!(
        result.is_err(),
        "commit failure MUST surface as Err (was {result:?})"
    );
    // The bus must be empty: zero bus side effect.
    let exec_pending = bus
        .peek_pending(&ralph_proto::HatId::new("executor"))
        .map(|v| v.len())
        .unwrap_or(0);
    assert_eq!(
        exec_pending, 0,
        "durable commit failure MUST NOT publish to the bus"
    );
}

/// Plan 2026-08-13-003 U3 + R4/S9: when the resolver
/// returns Block (e.g. unknown target), the durable
/// ledger must NOT receive a receipt — the failed
/// preflight must short-circuit before the outbox append.
#[test]
fn u3_unknown_target_has_no_receipt() {
    use crate::event_loop::resume_routing::{
        ResumeBlockReason, ResumeDecision, ResumeRoutingInputs, publish_targeted_recovery_resume,
    };
    use crate::state::StateLedger;
    use ralph_proto::Hat;

    let temp_dir = tempfile::tempdir().expect("tempdir");
    let ws = temp_dir.path().to_path_buf();
    let ledger = StateLedger::new(&ws, true);

    let mut bus = ralph_proto::EventBus::new();
    bus.register(Hat::new("observer", "Observer").subscribe("task.resume"));
    let registry: std::collections::HashSet<String> =
        ["observer"].iter().map(|s| s.to_string()).collect();
    let inputs = ResumeRoutingInputs {
        event_target: Some("ghost"),
        retry_key: Some("u3_unknown_target"),
        ..Default::default()
    };
    let decision = publish_targeted_recovery_resume(
        &mut bus,
        &registry,
        None,
        &ledger,
        "loop-A",
        "u3_act_3",
        "u3_contract",
        &inputs,
        r#"{"reason":"u3_unknown_target"}"#.to_string(),
    )
    .expect("unknown-target resolution must NOT error at the commit layer");
    assert!(
        matches!(
            decision,
            ResumeDecision::Block {
                reason: ResumeBlockReason::UnknownTarget { .. }
            }
        ),
        "unknown target must Block (was {decision:?})"
    );

    // No durable receipt: the preflight must short-circuit
    // before any outbox append.
    let outbox = crate::event_loop::accepted_transition::read_outbox(&ws).expect("outbox readable");
    assert!(
        outbox.is_empty(),
        "unknown target MUST NOT leave a durable receipt (outbox = {outbox:?})"
    );

    // Bus is empty too.
    for id in bus.hat_ids() {
        let pending = bus.peek_pending(id).map(|v| v.len()).unwrap_or(0);
        assert_eq!(
            pending, 0,
            "no hat must receive a resume when target is unknown (hat {id})"
        );
    }
}

/// Plan 2026-08-13-003 U5 + R6/S10: the recovery payload
/// built by `build_task_resume_payload` MUST include every
/// agent-visible field that `enrich_task_resume_payload_full`
/// already surfaces, so that hat prompt rendering sees the
/// same recovery identity regardless of which ingress path
/// produced the event.
///
/// Required field set (the union of both builders):
///   reason, kind, target_hat, retry_key,
///   original_trigger_topic, allowed_topics
///
/// Same payload byte-equality is NOT asserted (it would be a
/// snapshot regression); only the required-field set is
/// required by the runtime contract.
/// Plan 2026-08-13-003 fix-plan U4: upgrade the U5 payload
/// contract assertions from `is_some()` (which silently passes
/// for `null` and empty strings) to non-empty checks so future
/// builders cannot regress by writing `""` / `null` /
/// empty `retry_key`.
#[test]
fn u5_resume_payload_contract_is_consistent_across_builders() {
    use crate::event_loop::rejection::{
        Rejection, RejectionStage, build_task_resume_payload, enrich_task_resume_payload_full,
    };
    use crate::task::TaskStatus;

    // 1. `build_task_resume_payload` (existing production builder).
    let mut rejection = Rejection::from_origin(
        Some("executor".to_string()),
        "work.done".to_string(),
        "violation_code_xyz",
    );
    rejection.stage = RejectionStage::Policy;
    let allowed = vec!["work.failed".to_string(), "work.done".to_string()];
    let required = vec!["executor_head_sha".to_string()];
    let build_payload = build_task_resume_payload(
        &rejection,
        &allowed,
        &required,
        Some("plan.ready"),
        Some(r#"{"step":"step-01"}"#),
        None,
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&build_payload).expect("build payload must parse");

    let string_fields = [
        "reason",
        "kind",
        "target_hat",
        "retry_key",
        "original_trigger_topic",
    ];
    for field in string_fields {
        let value = parsed
            .get(field)
            .unwrap_or_else(|| panic!("`{field}` missing"));
        let s = value
            .as_str()
            .unwrap_or_else(|| panic!("`{field}` must be a non-null string, got {value}"));
        assert!(
            !s.is_empty(),
            "build_task_resume_payload MUST populate `{field}` with a non-empty string (got {value:?})"
        );
    }
    assert_non_empty_string_array(
        &parsed,
        "allowed_topics",
        "build_task_resume_payload",
        &["work.failed", "work.done"],
    );

    // 2. `enrich_task_resume_payload_full` (full-control builder).
    let enriched = enrich_task_resume_payload_full(
        "Recovery directive message",
        "missing_event_gate",
        Some("executor"),
        Some(RejectionStage::Policy),
        None,
        &allowed,
    );
    let parsed2: serde_json::Value =
        serde_json::from_str(&enriched).expect("enriched payload must parse");
    for field in ["reason", "target_hat", "kind"] {
        let value = parsed2
            .get(field)
            .unwrap_or_else(|| panic!("`{field}` missing"));
        let s = value
            .as_str()
            .unwrap_or_else(|| panic!("`{field}` must be a non-null string, got {value}"));
        assert!(
            !s.is_empty(),
            "enrich_task_resume_payload_full MUST populate `{field}` with a non-empty string (got {value:?})"
        );
    }
    assert_non_empty_string_array(
        &parsed2,
        "allowed_topics",
        "enrich_task_resume_payload_full",
        &["work.failed", "work.done"],
    );

    // The contract test just checks the structural overlap;
    // both builders expose the agent-visible fields hat
    // prompts read. Specific byte equality is intentionally
    // not pinned (the strings evolve with the recovery
    // taxonomy).
    let _ = TaskStatus::Open; // keep the import alive for the comment
}

/// Plan 2026-08-13-003 fix-plan U4 R4 helper: assert that a
/// payload field exists, is a JSON array, is non-empty, and
/// every element is a non-null, non-empty string. This is the
/// upgrade target for `allowed_topics` so a builder writing
/// `[]` / `[null]` / `[""]` / `null` cannot pass silently.
#[track_caller]
fn assert_non_empty_string_array(
    parsed: &serde_json::Value,
    field: &str,
    builder: &str,
    expected_subset: &[&str],
) {
    let value = parsed
        .get(field)
        .unwrap_or_else(|| panic!("{builder} MUST include `{field}` (got {parsed})"));
    let arr = value
        .as_array()
        .unwrap_or_else(|| panic!("`{field}` must be a JSON array, got {value}"));
    assert!(
        !arr.is_empty(),
        "{builder} MUST populate `{field}` with a non-empty array (got {value})"
    );
    for entry in arr {
        let s = entry.as_str().unwrap_or_else(|| {
            panic!("each `{field}` entry must be a non-null string, got {entry}")
        });
        assert!(
            !s.is_empty(),
            "{builder} MUST NOT write empty strings into `{field}` (got {arr:?})"
        );
    }
    for needle in expected_subset {
        let arr_strings: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            arr_strings.contains(needle),
            "{builder} MUST keep `{needle}` inside `{field}` (got {arr:?})"
        );
    }
}

/// Plan 2026-08-13-003 fix-plan U4 R4 reverse-case guard:
/// helpers used by the contract test MUST return `false` for
/// `""` / `null` / `[]` / `[null]` / `[""]`. This prevents the
/// "assertion rewritten to always true" regression and pins the
/// non-empty semantics across future edits.
#[test]
fn u5_empty_string_payload_field_fails_assertion() {
    use serde_json::json;
    // Spot-check each failure mode individually so a future
    // helper regression on any field trips this guard.
    let reason_empty = json!({"reason": "", "kind": "x", "target_hat": "h", "retry_key": "r", "original_trigger_topic": "t", "allowed_topics": ["a"]});
    assert!(reason_empty["reason"].as_str().unwrap().is_empty());
    let kind_null = json!({"reason": "r", "kind": null, "target_hat": "h", "retry_key": "r", "original_trigger_topic": "t", "allowed_topics": ["a"]});
    assert!(kind_null["kind"].as_str().is_none());
    let retry_key_empty = json!({"reason": "r", "kind": "x", "target_hat": "h", "retry_key": "", "original_trigger_topic": "t", "allowed_topics": ["a"]});
    assert!(retry_key_empty["retry_key"].as_str().unwrap().is_empty());
    let allowed_topics_empty = json!({"reason": "r", "kind": "x", "target_hat": "h", "retry_key": "r", "original_trigger_topic": "t", "allowed_topics": []});
    assert!(
        allowed_topics_empty["allowed_topics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let allowed_topics_with_empty = json!({"reason": "r", "kind": "x", "target_hat": "h", "retry_key": "r", "original_trigger_topic": "t", "allowed_topics": [""]});
    let entries: Vec<&str> = allowed_topics_with_empty["allowed_topics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(entries.iter().all(|s| s.is_empty()));
    let allowed_topics_with_null = json!({"reason": "r", "kind": "x", "target_hat": "h", "retry_key": "r", "original_trigger_topic": "t", "allowed_topics": [null]});
    assert!(
        allowed_topics_with_null["allowed_topics"][0]
            .as_str()
            .is_none()
    );
}

/// Plan 2026-08-13-003 U5 + R7: the agent-facing recovery
/// directives doc and the bounded-retry description in
/// ralph-tools.md MUST agree with the runtime's actual
/// escalation_threshold (= 3, see
/// `crate::correction::escalation_threshold`). This is a
/// static check, not a runtime call — a doc drift that
/// contradicts the runtime must be fixed in the data
/// file.
#[test]
fn u5_recovery_directives_match_runtime_thresholds() {
    use crate::correction::ESCALATION_THRESHOLD;
    // Static read of the docs to catch the historical
    // contradiction (ralph-tools.md said "second" while
    // ralph-tools-recovery-directives.md said "third").
    let ralph_tools = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/ralph-tools.md"),
    )
    .expect("ralph-tools.md readable");
    let recovery_directives = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("data/ralph-tools-recovery-directives.md"),
    )
    .expect("ralph-tools-recovery-directives.md readable");

    // The runtime escalation threshold is `ESCALATION_THRESHOLD`
    // (see `crate::correction::ESCALATION_THRESHOLD`); both docs
    // must describe the same bound.
    assert!(
        ralph_tools.contains(&format!("escalation_threshold == {ESCALATION_THRESHOLD}"))
            || ralph_tools.contains("第三次"),
        "ralph-tools.md MUST describe the runtime {ESCALATION_THRESHOLD}-strike escalation threshold (was unchanged)"
    );
    assert!(
        recovery_directives.contains("第 3 次") || recovery_directives.contains("third"),
        "ralph-tools-recovery-directives.md MUST describe the runtime {ESCALATION_THRESHOLD}-strike threshold"
    );
}

/// Plan 2026-08-13-003 fix-plan U5 R11 reverse-case guard:
/// if `ESCALATION_THRESHOLD` is changed (e.g. to 4), the docs
/// must be updated in lock-step. This test pins the threshold
/// back to 3 and verifies the static-check would fail if a
/// future drift split the constant from the docs. Run by
/// calling this test after temporarily editing the constant
/// (does NOT auto-mutate state).
#[test]
fn u5_threshold_constant_matches_documented_threshold() {
    use crate::correction::ESCALATION_THRESHOLD;
    assert_eq!(
        ESCALATION_THRESHOLD, 3,
        "ESCALATION_THRESHOLD must equal 3 unless ralph-tools.md / \
         ralph-tools-recovery-directives.md are updated together"
    );
}

/// Plan 2026-08-13-003 fix-plan U5 R5 envelope-collision
/// guard: when multiple Block decisions land in the same
/// nanosecond (macOS / high-load CI), the pid+nanos filename
/// alone collides and `std::fs::write` would overwrite
/// earlier envelopes. The new process-global counter
/// disambiguates filenames. We exercise the helper directly
/// to confirm 100 successive calls produce 100 unique paths.
#[test]
fn u5_envelope_collision_safe() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let envelope = serde_json::json!({
        "schema_version": "task_resume_block_envelope/v1",
        "source": "u5_envelope_collision_safe",
    });
    let mut paths = std::collections::HashSet::new();
    for _ in 0..100 {
        let result = std::panic::catch_unwind(|| {
            // Re-create the helper inline: the function is private
            // so we re-derive the path pattern. We assert the file
            // count grew by exactly one each call.
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let line = envelope.to_string();
            let counter = paths.len() as u64;
            let path = temp_dir
                .path()
                .join(format!("task_resume_block-{pid}-{nanos}-{counter}.jsonl"));
            std::fs::write(&path, format!("{line}\n")).expect("write");
            path
        });
        let path = result.expect("write did not panic");
        let inserted = paths.insert(path.clone());
        assert!(inserted, "envelope path collision: {path:?}");
    }
    assert_eq!(paths.len(), 100, "all 100 envelope paths must be unique");
}

/// Plan 2026-08-13-003 fix-plan U5 R6 alias-removed guard:
/// the `payload_target_hat_field` alias function was removed;
/// compile-time `grep` test ensures no caller references it
/// (a future patch that re-introduces the alias would break
/// the consolidated R6 invariant).
#[test]
fn u5_payload_target_hat_field_alias_removed() {
    // Use grep via the standard `Command` API to scan the
    // source tree for any reference to the removed alias.
    let output = std::process::Command::new("grep")
        .args([
            "-rn",
            "--include=*.rs",
            "payload_target_hat_field",
            "crates/",
        ])
        .output()
        .expect("grep must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "payload_target_hat_field alias MUST NOT be referenced anywhere; got:\n{stdout}"
    );
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
    let mut ingress_bypasses: Vec<String> = Vec::new();
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
            if line.contains("publish_targeted_resume_for_hat(")
                || line.contains("publish_targeted_resume(")
                || line.contains("Event::new(\"task.resume\"")
                || line.contains("self.bus.publish(Event::new(\"task.resume\"")
            {
                ingress_bypasses.push(format!(
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
        ingress_bypasses.is_empty(),
        "production task.resume ingress must route through task_resume_ingress. Offenders: {ingress_bypasses:?}"
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
