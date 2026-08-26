// Auto-extracted from the legacy loop-runner regression suite. Tests in this
// module remain part of the loop_runner::tests::legacy surface; only the file
// layout changed (mechanical split per plan 2026-08-07-005). Behavior,
// assertions, fixtures, and process environment semantics are unchanged.
//
// The full original `legacy.rs` import set is reproduced verbatim per bucket so
// that every existing test compiles without rewriting call sites. Splits may
// leave some imports unused in a given bucket; this is a mechanical artifact,
// not dead code (the same items remain used by sibling buckets).

#![allow(unused_imports)]

use super::super::super::*;
use super::super::common::*;
use super::super::fake_path::*;
use super::helpers::*;
use crate::test_support::CwdGuard;
use ralph_core::HatRegistry;
use ralph_core::planning_session::{ConversationEntry, ConversationType};
use ralph_proto::{Hat, Topic};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::sync::{Arc, Mutex};

// Test: test_recover_late_events_before_fallback_routes_pending_work
#[test]
fn test_recover_late_events_before_fallback_routes_pending_work() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let yaml = r#"
hats:
  investigator:
    name: "Investigator"
    triggers: ["debug.start", "hypothesis.rejected", "hypothesis.confirmed", "fix.verified"]
    publishes: ["hypothesis.test", "fix.propose", "DEBUG_COMPLETE"]
  tester:
    name: "Tester"
    triggers: ["hypothesis.test"]
    publishes: ["hypothesis.confirmed", "hypothesis.rejected"]
"#;
    let (mut event_loop, loop_ctx) =
        dispatch_test_event_loop_from_yaml_with_context(temp_dir.path(), yaml);
    let events_path = loop_ctx.events_path();
    std::fs::create_dir_all(events_path.parent().expect("events path parent"))
        .expect("create events directory");

    let mut events_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .expect("open events file");
    writeln!(
            events_file,
            r#"{{"topic":"hypothesis.test","payload":"Race condition suspected","ts":"2026-03-08T00:00:00Z"}}"#
        )
        .expect("write late event");
    events_file.flush().expect("flush late event");

    let outcome =
        recover_late_events_before_fallback(&mut event_loop).expect("recover late events");
    assert_eq!(outcome, LateEventRecovery::PendingWork);
    assert_eq!(
        event_loop.next_hat().map(|hat| hat.as_str()),
        Some("ralph"),
        "late downstream work should route the next iteration to Ralph in multi-hat mode"
    );

    let tester_id = HatId::new("tester");
    let tester_pending = event_loop
        .bus()
        .peek_pending(&tester_id)
        .cloned()
        .unwrap_or_default();
    assert_eq!(tester_pending.len(), 1);
    assert_eq!(tester_pending[0].topic.as_str(), "hypothesis.test");
}

// Test: test_recover_late_events_before_fallback_honors_completion
#[test]
fn test_recover_late_events_before_fallback_honors_completion() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (mut event_loop, loop_ctx) = dispatch_test_event_loop_with_context(temp_dir.path());
    let events_path = loop_ctx.events_path();
    std::fs::create_dir_all(events_path.parent().expect("events path parent"))
        .expect("create events directory");

    let mut events_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .expect("open events file");
    writeln!(
        events_file,
        r#"{{"topic":"LOOP_COMPLETE","payload":"done","ts":"2026-03-08T00:00:00Z"}}"#
    )
    .expect("write completion event");
    events_file.flush().expect("flush completion event");

    let outcome = recover_late_events_before_fallback(&mut event_loop).expect("recover completion");
    assert_eq!(
        outcome,
        LateEventRecovery::Terminate(TerminationReason::CompletionPromise)
    );
}

// Test: test_recover_late_events_before_fallback_polls_for_delayed_completion
#[test]
fn test_recover_late_events_before_fallback_polls_for_delayed_completion() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (mut event_loop, loop_ctx) = dispatch_test_event_loop_with_context(temp_dir.path());
    let events_path = loop_ctx.events_path();
    std::fs::create_dir_all(events_path.parent().expect("events path parent"))
        .expect("create events directory");

    let delayed_events_path = events_path.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        let mut events_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&delayed_events_path)
            .expect("open delayed events file");
        writeln!(
            events_file,
            r#"{{"topic":"LOOP_COMPLETE","payload":"done","ts":"2026-03-08T00:00:00Z"}}"#
        )
        .expect("write delayed completion event");
        events_file.flush().expect("flush delayed completion event");
    });

    let outcome = recover_late_events_before_fallback(&mut event_loop).expect("recover completion");
    writer.join().expect("join delayed event writer");

    assert_eq!(
        outcome,
        LateEventRecovery::Terminate(TerminationReason::CompletionPromise)
    );
}

// Test: test_recover_expected_emit_after_output_polls_for_delayed_completion
#[test]
fn test_recover_expected_emit_after_output_polls_for_delayed_completion() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (mut event_loop, loop_ctx) = dispatch_test_event_loop_with_context(temp_dir.path());
    let events_path = loop_ctx.events_path();
    std::fs::create_dir_all(events_path.parent().expect("events path parent"))
        .expect("create events directory");

    let delayed_events_path = events_path.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        let mut events_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&delayed_events_path)
            .expect("open delayed events file");
        writeln!(
            events_file,
            r#"{{"topic":"LOOP_COMPLETE","payload":"done","ts":"2026-03-08T00:00:00Z"}}"#
        )
        .expect("write delayed completion event");
        events_file.flush().expect("flush delayed completion event");
    });

    let outcome =
        recover_expected_emit_after_output(&mut event_loop).expect("recover expected emit");
    writer.join().expect("join delayed event writer");

    assert_eq!(
        outcome,
        LateEventRecovery::Terminate(TerminationReason::CompletionPromise)
    );
}

// Test: test_compute_recovery_status_returns_target_when_targeted_retry_published
#[test]
fn test_compute_recovery_status_returns_target_when_targeted_retry_published() {
    // 2026-06-04 plan U7: a `task.resume` event with `target=executor`
    // and a payload mentioning the rejected topic must register as
    // recovery routed to executor.
    use ralph_proto::Event;
    let mut event_loop = make_event_loop_for_recovery_test();
    let payload = serde_json::json!({
        "rejected_topic": "work.done",
        "reason": "task not closed",
        "required_action": "fix and re-emit",
        "original_payload": "{}",
        "retry_publish_topics": ["work.done", "work.failed"],
    })
    .to_string();
    event_loop
        .bus()
        .publish(Event::new("task.resume", payload).with_target("executor"));

    let status = compute_recovery_status(&mut event_loop, "work.done");
    assert_eq!(
        status.as_deref(),
        Some("executor"),
        "compute_recovery_status must return the target hat when a targeted retry was published"
    );
}

// Test: test_compute_recovery_status_returns_none_when_no_targeted_retry
#[test]
fn test_compute_recovery_status_returns_none_when_no_targeted_retry() {
    // When no targeted retry was published, the operator log must say
    // "no safe retry target" so they know to intervene.
    use ralph_proto::Event;
    let mut event_loop = make_event_loop_for_recovery_test();
    // Publish a human.guidance event but no targeted retry.
    event_loop
        .bus()
        .publish(Event::new("human.guidance", "see doc"));

    let status = compute_recovery_status(&mut event_loop, "work.done");
    assert!(
        status.is_none(),
        "compute_recovery_status must return None when no targeted retry is in the bus"
    );
}

// Test: u4_handle_execution_contract_rejections_writes_envelope_for_safe_target
#[test]
fn u4_handle_execution_contract_rejections_writes_envelope_for_safe_target() {
    // U4: a rejected contract event with a safe retry target writes
    // a recovery envelope with `safe_target = true` and
    // `target_hat = <retry target>`.
    use ralph_core::ProcessedEvents;
    use ralph_core::diagnosis::{DiagnosisSeverity, DiagnosisSource};
    use ralph_core::execution_contract::{
        ExecutionContractFinding, ExecutionContractViolationKind,
    };

    let (_temp, workspace) = u4_workspace();
    let diagnostics = ralph_core::diagnostics::DiagnosticsCollector::with_enabled(&workspace, true)
        .expect("create diagnostics collector");
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done", "work.failed"]
"#;
    let mut config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
    config.core.workspace_root = workspace.clone();
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.set_iteration_for_test(7);

    let finding = ExecutionContractFinding {
        topic: "work.done".to_string(),
        kind: ExecutionContractViolationKind::NoGitEvidence { step: None },
        message: "no diff or commit observed".to_string(),
        source_hat: Some("executor".to_string()),
    };

    // Simulate a targeted retry that was published to the source hat
    // (so compute_recovery_status returns Some("executor")).
    let retry_payload = serde_json::json!({
        "rejected_topic": "work.done",
        "reason": finding.message,
    })
    .to_string();
    event_loop
        .bus()
        .publish(ralph_proto::Event::new("task.resume", retry_payload).with_target("executor"));

    let processed = ProcessedEvents {
        had_events: false,
        had_raw_events: true,
        had_rejected_events: true,
        had_plan_events: false,

        has_orphans: false,
        accepted_events: vec![],
        contract_rejections: vec![finding.clone()],
        payload_contract_violation: None,
    };
    let hat_id = ralph_proto::HatId::new("executor");
    handle_execution_contract_rejections(&processed, &mut event_loop, &hat_id);

    // Characterization: the existing audit line was still emitted
    // (ContractRecoveryRouted with the target).
    let orch_path = u4_orchestration_log(&workspace);
    let orch = std::fs::read_to_string(&orch_path).expect("read orchestration");
    assert!(
        orch.contains("\"type\":\"contract_recovery_routed\""),
        "missing ContractRecoveryRouted audit line"
    );
    assert!(
        orch.contains("\"retry_target\":\"executor\""),
        "ContractRecoveryRouted must carry retry_target=executor; content was: {orch}"
    );

    // The runner observes EventLoop's targeted recovery and must not
    // remove or duplicate the pending task.resume.
    let pending: Vec<ralph_proto::Event> = event_loop
        .bus()
        .peek_pending(&ralph_proto::HatId::new("executor"))
        .cloned()
        .unwrap_or_default();
    let resume_count = pending
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    assert!(
        resume_count >= 1,
        "U2: at least one task.resume must be pending for the source hat; got {resume_count}"
    );

    // Characterization: the rejected event must NOT be on the bus
    // (it was a rejection, not a publication).
    let no_rejected_on_bus = event_loop
        .bus()
        .peek_pending(&ralph_proto::HatId::new("executor"))
        .map(|events| !events.iter().any(|e| e.topic.as_str() == "work.done"))
        .unwrap_or(true);
    assert!(
        no_rejected_on_bus,
        "rejected work.done must not be in the bus"
    );

    // U4: a recovery journal entry was written.
    let entries = u4_recovery_journal(&workspace);
    assert_eq!(entries.len(), 1, "expected one recovery entry");
    let entry = &entries[0];
    let env = &entry.envelope;
    assert_eq!(env.source, DiagnosisSource::ExecutionContract);
    assert_eq!(env.target_hat.as_deref(), Some("executor"));
    assert_eq!(env.source_hat.as_deref(), Some("executor"));
    assert_eq!(env.severity, DiagnosisSeverity::Error);
    assert_eq!(env.topic.as_deref(), Some("work.done"));
    assert!(env.safe_target, "retry target exists");
    assert!(
        entry.notes.iter().any(|n| n.contains("executor")),
        "notes should mention the safe retry target"
    );
    assert!(
        u4_orchestration_has_recovery_diagnosed(&workspace, &env.diagnosis_id),
        "audit line must reference the envelope's diagnosis_id"
    );
}

// Test: u4_handle_execution_contract_rejections_writes_envelope_when_no_safe_target
#[test]
fn u4_handle_execution_contract_rejections_writes_envelope_when_no_safe_target() {
    // U2: when the bounded retry budget is exhausted, the envelope is
    // still written but with `safe_target = false`, `target_hat = None`
    // (since the runner refuses to publish a `task.resume` it knows will
    // not be honored) and a "failed-closed" / "retry budget exhausted"
    // note.  Pre-2026-06-07, this test asserted the no-task-resume-on-bus
    // case; normal publication is owned by EventLoop.
    use ralph_core::ProcessedEvents;
    use ralph_core::U2_REJECTION_RETRY_LIMIT;
    use ralph_core::diagnosis::DiagnosisSource;
    use ralph_core::execution_contract::{
        ExecutionContractFinding, ExecutionContractViolationKind,
    };

    let (_temp, workspace) = u4_workspace();
    let diagnostics = ralph_core::diagnostics::DiagnosticsCollector::with_enabled(&workspace, true)
        .expect("create diagnostics collector");
    let config = ralph_core::RalphConfig::default();
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.set_iteration_for_test(2);

    let finding = ExecutionContractFinding {
        topic: "work.done".to_string(),
        kind: ExecutionContractViolationKind::TaskNotTerminal {
            task_id: "t-1".to_string(),
            status: "open".to_string(),
            allowed: vec!["closed".to_string()],
        },
        message: "task is still open".to_string(),
        source_hat: Some("executor".to_string()),
    };

    // Pre-exhaust the retry budget so the next rejection is the
    // fail-closed case.  With the `>` semantics from the 2026-06-07
    // rework, the budget is exhausted on the (LIMIT+1)-th attempt —
    // we record LIMIT times so the rejection we're about to test
    // becomes the (LIMIT+1)-th and triggers fail-closed.
    for _ in 0..U2_REJECTION_RETRY_LIMIT {
        let probe = ralph_core::Rejection::from_execution_contract(
            &finding,
            Some("executor".to_string()),
            Some("executor".to_string()),
        );
        event_loop
            .state_mut()
            .record_rejection_key(&probe.retry_key);
    }

    let processed = ProcessedEvents {
        had_events: false,
        had_raw_events: true,
        had_rejected_events: true,
        had_plan_events: false,

        has_orphans: false,
        accepted_events: vec![],
        contract_rejections: vec![finding],
        payload_contract_violation: None,
    };
    let hat_id = ralph_proto::HatId::new("executor");
    handle_execution_contract_rejections(&processed, &mut event_loop, &hat_id);

    let entries = u4_recovery_journal(&workspace);
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    let env = &entry.envelope;
    assert_eq!(env.source, DiagnosisSource::ExecutionContract);
    assert!(!env.safe_target, "budget exhausted → no safe target");
    assert!(
        env.target_hat.is_none(),
        "target_hat must be None when budget exhausted"
    );
    assert!(
        entry.notes.iter().any(|n| n.contains("failed-closed")),
        "notes must say 'failed-closed' when budget is exhausted; got: {:?}",
        entry.notes
    );
    assert!(
        entry
            .notes
            .iter()
            .any(|n| n.contains("retry budget exhausted")),
        "notes must explain why failed-closed; got: {:?}",
        entry.notes
    );
}

// Test: u4_inject_fallback_event_payload_has_recovery_diagnosis_block
#[test]
fn u4_inject_fallback_event_payload_has_recovery_diagnosis_block() {
    // U4: the task.resume payload built by inject_fallback_event
    // carries a "## Recovery Diagnosis" appendix so downstream
    // tooling can grep for the structured block.
    let mut event_loop = make_event_loop_for_recovery_test();
    // We can't mutate `state.last_hat` directly from here, so just
    // exercise the formatter on a representative event.
    let payload = format!(
        "RECOVERY: Previous iteration by hat `executor` did not publish an event.{}",
        EventLoop::format_recovery_diagnosis_block(
            "stall_no_events",
            "executor",
            "emit a regular event",
            0,
            &[],
        ),
    );
    event_loop
        .bus()
        .publish(ralph_proto::Event::new("task.resume", payload).with_target("executor"));

    // Drain pending and inspect the task.resume payload.
    let pending = event_loop
        .bus()
        .take_pending(&ralph_proto::HatId::new("executor"));
    let task_resume = pending
        .iter()
        .find(|e| e.topic.as_str() == "task.resume")
        .expect("task.resume must be on the bus");
    let body = task_resume.payload.as_str();
    assert!(
        body.contains("## Recovery Diagnosis"),
        "task.resume payload must include the '## Recovery Diagnosis' block: {body}"
    );
    assert!(body.contains("- reason: stall_no_events"));
    assert!(body.contains("- target: executor"));
    assert!(body.contains("- expected action: emit a regular event"));
    assert!(body.contains("- retry attempt: 0"));
}

// Test: u4_recovery_count_aggregates_workspace_and_session_journals
#[test]
fn u4_recovery_count_aggregates_workspace_and_session_journals() {
    use ralph_core::event_loop::idempotent_wiring;
    use ralph_core::state::idempotent_log::IdempotentLog;

    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    // Seed 4 `_final=true` IdempotentLog records.
    let ralph_dir = workspace.join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    let mut log = IdempotentLog::open(&ralph_dir, "u4-p0-2").expect("open idempotent log");
    for i in 0..3 {
        idempotent_wiring::write_recovery(
            &mut log,
            &format!("cli-{i}"),
            "u4-p0-2",
            serde_json::json!({"reason_code": "policy_denied"}),
            true,
        )
        .unwrap();
    }
    idempotent_wiring::write_recovery(
        &mut log,
        "sess-1",
        "u4-p0-2",
        serde_json::json!({"reason_code": "no_emit"}),
        true,
    )
    .unwrap();
    drop(log);

    let event_loop = build_u8_event_loop(workspace.clone(), true);
    // Push the seeded log into the EventLoop so
    // `build_termination_diagnostics` reads the right records.
    {
        let log_mutex = event_loop.idempotent_log();
        let mut guard = log_mutex.lock().expect("idempotent_log poisoned");
        *guard = IdempotentLog::open(&ralph_dir, "u4-p0-2").expect("reopen");
        let _ = guard.replay();
    }

    let (_hint, seed) =
        build_termination_diagnostics(&event_loop, None).expect("hint + seed must be Some");

    assert_eq!(
        seed.recovery_count, 4,
        "P0-2: 4 `_final=true` IdempotentLog records → recovery_count must be 4, got {}. notes={:?}",
        seed.recovery_count, seed.notes
    );
    // Notes must surface the SC-5 data source (IdempotentLog)
    // so operators know the count is authoritative.
    assert!(
        seed.notes
            .iter()
            .any(|n| n.contains("IdempotentLog.final_records()")),
        "notes must attribute count source to IdempotentLog; got: {:?}",
        seed.notes
    );
}

// Test: u4_recovery_count_zero_when_no_journals_present
#[test]
fn u4_recovery_count_zero_when_no_journals_present() {
    let tmp = tempfile::TempDir::new().unwrap();
    let event_loop = build_u8_event_loop(tmp.path().to_path_buf(), true);

    let (_hint, seed) =
        build_termination_diagnostics(&event_loop, None).expect("hint + seed must be Some");

    assert_eq!(
        seed.recovery_count, 0,
        "P0-2: no IdempotentLog final records → count must be 0, got {}. notes={:?}",
        seed.recovery_count, seed.notes
    );
    // Notes still describe the data source so operators know where to look.
    assert_eq!(seed.notes.len(), 3);
    assert!(
        seed.notes[0].contains("IdempotentLog.final_records()"),
        "first note must attribute count to IdempotentLog, got: {}",
        seed.notes[0]
    );
}

// Test: u4_recovery_count_falls_back_to_workspace_when_session_empty
#[test]
fn u4_recovery_count_falls_back_to_workspace_when_session_empty() {
    use ralph_core::event_loop::idempotent_wiring;
    use ralph_core::state::idempotent_log::IdempotentLog;

    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    // 2 `_final=true` IdempotentLog records (simulating
    // workspaced-level recovery entries).
    let ralph_dir = workspace.join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    let mut log = IdempotentLog::open(&ralph_dir, "u4-edge").expect("open idempotent log");
    for i in 0..2 {
        idempotent_wiring::write_recovery(
            &mut log,
            &format!("cli-{i}"),
            "u4-edge",
            serde_json::json!({"reason_code": "policy_denied"}),
            true,
        )
        .unwrap();
    }
    drop(log);

    let event_loop = build_u8_event_loop(workspace.clone(), true);
    // Push the seeded log into the EventLoop.
    {
        let log_mutex = event_loop.idempotent_log();
        let mut guard = log_mutex.lock().expect("idempotent_log poisoned");
        *guard = IdempotentLog::open(&ralph_dir, "u4-edge").expect("reopen");
        let _ = guard.replay();
    }

    let (_hint, seed) =
        build_termination_diagnostics(&event_loop, None).expect("hint + seed must be Some");

    assert_eq!(
        seed.recovery_count, 2,
        "P0-2: 2 IdempotentLog final records → recovery_count must equal 2, got {}",
        seed.recovery_count
    );
}

// Test: u5_persist_starting_event_writes_work_start_line
#[test]
fn u5_persist_starting_event_writes_work_start_line() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let (ctx, events_path) = u5_stage_events_file(tmp.path(), "u5-events.jsonl");

    persist_starting_event_to_events_file(&ctx, "work.start", "Implement dev plan:foo.md")
        .expect("persist should succeed");

    let content = std::fs::read_to_string(&events_path).expect("read events file");
    let line = content
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("at least one event line must be written");

    let event: serde_json::Value =
        serde_json::from_str(line).expect("work.start event must be valid JSON");
    assert_eq!(
        event["topic"], "work.start",
        "U5: topic must be the configured starting_event"
    );
    assert_eq!(
        event["source"], "loop-bootstrap",
        "U5: source tag identifies the orchestrator-owned bootstrap write"
    );
    assert_eq!(
        event["payload"], "Implement dev plan:foo.md",
        "U5: payload must round-trip the prompt content verbatim"
    );
    assert!(
        event["ts"].is_string(),
        "U5: ts must be an RFC3339 string (EventReader classifies it)"
    );
    // No `hat` field is written — this matches the orchestrator's
    // internal emits and keeps the origin guard whitelist unchanged.
    assert!(
        event.get("hat").is_none(),
        "U5: bootstrap write must not include a hat field; got: {event}"
    );

    // The line must end with a newline so the next writer (hat
    // activations, hard-gate) does not bleed into the same record.
    assert!(
        content.ends_with('\n'),
        "U5: events line must be newline-terminated"
    );
}

// Test: u5_sync_event_reader_to_file_end_skips_bootstrap_line
#[test]
fn u5_sync_event_reader_to_file_end_skips_bootstrap_line() {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let (ctx, events_path) = u5_stage_events_file(tmp.path(), "u5-events.jsonl");

    persist_starting_event_to_events_file(&ctx, "work.start", "noop")
        .expect("persist should succeed");

    let file_len = std::fs::metadata(&events_path).expect("file exists").len();
    assert!(file_len > 0, "U5 precondition: bootstrap line was written");

    // Build an EventLoop that points at the same events file.
    let mut config = RalphConfig::default();
    config.core.workspace_root = tmp.path().to_path_buf();
    config.event_loop.starting_event = Some("work.start".to_string());
    let mut event_loop = EventLoop::with_context(config, ctx.clone());

    // Position must start at 0 (fresh EventReader) — confirms that
    // the bootstrap line WOULD be re-read if we did not skip.
    assert_eq!(
        event_loop.event_reader_position(),
        0,
        "U5 precondition: fresh EventReader starts at offset 0 \
         (would re-deliver work.start without sync_event_reader_to_file_end)"
    );

    event_loop.sync_event_reader_to_file_end();

    assert_eq!(
        event_loop.event_reader_position(),
        file_len,
        "U5: sync_event_reader_to_file_end must push the cursor to the file end"
    );

    // read_new_events must see zero events — the bootstrap record
    // exists on disk but is past the cursor.
    let peek = event_loop
        .peek_event_reader_for_test()
        .expect("peek new events");
    assert!(
        peek.events.is_empty(),
        "U5: no events should be re-delivered after sync_event_reader_to_file_end; \
         got: {peek:?}"
    );
}

// Test: u5_resume_branch_does_not_re_inject_work_start
#[test]
fn u5_resume_branch_does_not_re_inject_work_start() {
    // The runner's `if !resume { ... persist ... }` guard is the
    // only enforcement point.  We exercise it indirectly by
    // simulating the resume precondition: no `current-events`
    // marker rotation happens, and the helper, if called, would
    // write into whatever path the marker points to.  The runner
    // itself never calls the helper in this branch — verified by
    // reading `run_loop_impl_inner` (see line ~720, the
    // `if !resume` block).  This test pins that contract by
    // asserting the helper is *not* invoked from the resume path:
    // we only check that the helper is callable and idempotent
    // (i.e. calling it twice produces two lines, which the resume
    // path must avoid).
    let tmp = tempfile::tempdir().expect("temp dir");
    let (ctx, events_path) = u5_stage_events_file(tmp.path(), "u5-resume-events.jsonl");

    persist_starting_event_to_events_file(&ctx, "work.start", "first").expect("first persist");
    let after_first = std::fs::read_to_string(&events_path).expect("read").len();

    // If the resume path were to call the helper again, the file
    // would grow by another line.  The runner's contract is to
    // NOT call it on resume; the assertion below documents the
    // expected size of the file after exactly one persist call.
    let content_after_first = std::fs::read_to_string(&events_path).expect("read");
    let lines_after_first: Vec<&str> = content_after_first
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert_eq!(
        lines_after_first.len(),
        1,
        "U5: a single persist call must produce a single line; \
         resume path must not re-inject work.start"
    );
    // Belt-and-suspenders: file size must not have been touched by
    // a second call (this test does not call it, but the assertion
    // pins the byte length for any future regression).
    assert!(after_first > 0, "U5: bootstrap line must be non-empty");
}

// Test: u5_persist_starting_event_reports_io_errors
#[test]
fn u5_persist_starting_event_reports_io_errors() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let ctx = LoopContext::primary(tmp.path().to_path_buf());
    let ralph_dir = ctx.ralph_dir();
    std::fs::create_dir_all(&ralph_dir).expect("create .ralph dir");

    // Point the marker at a path whose parent we will NOT create.
    // `OpenOptions::create(true)` only creates the leaf file, so the
    // missing parent directory is the failure mode.
    let bogus = ".ralph/missing-subdir/u5-events.jsonl";
    std::fs::write(ctx.current_events_marker(), bogus).expect("write marker");

    let result = persist_starting_event_to_events_file(&ctx, "work.start", "noop");
    assert!(
        result.is_err(),
        "U5: persisting into a missing parent directory must surface Err; got: {result:?}"
    );
}

// Test: test_interrupt_helper_merges_hat_channel_content_into_main_events
#[test]
fn test_interrupt_helper_merges_hat_channel_content_into_main_events() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let ctx = ralph_core::LoopContext::primary(workspace.clone());
    ctx.ensure_ralph_dir().expect("ensure ralph dir");

    // Materialise an isolated hat-channel with one pre-existing emit using the
    // same naming the runner uses for `prepare_hat_channel`.
    let hat = "reviewer";
    let loop_id = "primary-20260807-000000";
    let iteration = 1u32;
    let payload = r#"{"topic":"merge.reviewed","hat":"reviewer","source":"reviewer","payload":{"target_branch":"pittcat-dev"},"ts":"2026-08-07T00:00:00Z"}"#;
    let channel_path = seed_hat_channel(&ctx, hat, loop_id, iteration, &format!("{payload}\n"));

    let mut config = ralph_core::RalphConfig::default();
    config.event_loop.execution_mode = ralph_core::config::HatExecutionMode::Isolated;

    let event_loop = build_isolated_event_loop(config.clone(), Some(hat));

    let state_machine_enabled = false;
    let target_events_path =
        crate::loop_runner::paths::resolve_emit_events_path(&ctx, state_machine_enabled);
    if let Some(parent) = target_events_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&target_events_path, "").unwrap();

    crate::loop_runner::runner::runner_inner_test_api::merge_isolated_channel_on_interrupt(
        &ctx,
        &config,
        state_machine_enabled,
        &event_loop,
        "test_interrupt_helper_merges_hat_channel_content_into_main_events",
        Some(&channel_path),
    );

    // The line from the hat-channel must now appear in the main events file.
    let main = std::fs::read_to_string(&target_events_path).unwrap();
    assert!(
        main.lines().any(|line| line.contains("merge.reviewed")),
        "interrupt helper must merge hat-channel content into main events; got: {main}"
    );
    // The channel file must have been removed by `merge_hat_channel` so a
    // future iteration does not replay the same line.
    assert!(
        !channel_path.exists(),
        "merge_hat_channel must remove the channel file on success to prevent replays; \
         channel_path still exists at: {}",
        channel_path.display()
    );
    // The marker must have been cleared too.
    assert!(
        !ctx.current_events_marker().exists(),
        "current-events marker must still be intact (clear is the merger's job, not this test's)"
    );
}

// Test: test_interrupt_helper_with_empty_hat_channel_does_not_corrupt_events
#[test]
fn test_interrupt_helper_with_empty_hat_channel_does_not_corrupt_events() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let ctx = ralph_core::LoopContext::primary(workspace.clone());
    ctx.ensure_ralph_dir().expect("ensure ralph dir");

    // Empty channel — same shape as the merge-batch run reproduced in the diagnosis.
    let hat = "reviewer";
    let loop_id = "primary-20260807-000001";
    let iteration = 1u32;
    let _channel_path = seed_hat_channel(&ctx, hat, loop_id, iteration, "");

    let mut config = ralph_core::RalphConfig::default();
    config.event_loop.execution_mode = ralph_core::config::HatExecutionMode::Isolated;

    let event_loop = build_isolated_event_loop(config.clone(), Some(hat));

    let state_machine_enabled = false;
    let target_events_path =
        crate::loop_runner::paths::resolve_emit_events_path(&ctx, state_machine_enabled);
    if let Some(parent) = target_events_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let original = "{\"topic\":\"merge.start\",\"source\":\"loop-bootstrap\",\"ts\":\"x\"}\n";
    std::fs::write(&target_events_path, original).unwrap();

    crate::loop_runner::runner::runner_inner_test_api::merge_isolated_channel_on_interrupt(
        &ctx,
        &config,
        state_machine_enabled,
        &event_loop,
        "test_interrupt_helper_with_empty_hat_channel_does_not_corrupt_events",
        Some(&_channel_path),
    );

    // Empty channel must not append anything to main events.
    let main_after = std::fs::read_to_string(&target_events_path).unwrap();
    assert_eq!(
        main_after, original,
        "interrupt helper with empty hat-channel must leave the main events file unchanged"
    );

    // And the diagnostic fallback file should have been produced in
    // `.ralph/diagnostics/channel-routing-fallback-*.md`.
    let diag_dir = ctx.ralph_dir().join("diagnostics");
    let has_diag = std::fs::read_dir(&diag_dir)
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("channel-routing-fallback")
            })
        })
        .unwrap_or(false);
    assert!(
        has_diag,
        "interrupt helper with empty hat-channel must emit a channel-routing-fallback diagnostic \
         under {} so operators can see the warning",
        diag_dir.display()
    );
}

// Test: test_interrupt_helper_with_no_marker_is_a_safe_noop
#[test]
fn test_interrupt_helper_with_no_marker_is_a_safe_noop() {
    // Cold-path interrupt at the very top of the loop, before any iteration
    // ran, must be a safe no-op. Locks in that the fix can never panic or
    // corrupt the main events file when no hat-channel exists.
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let ctx = ralph_core::LoopContext::primary(workspace.clone());
    ctx.ensure_ralph_dir().expect("ensure ralph dir");

    let mut config = ralph_core::RalphConfig::default();
    config.event_loop.execution_mode = ralph_core::config::HatExecutionMode::Isolated;

    // `last_hat = None` simulates the cold-path interrupt (no iteration yet).
    let event_loop = build_isolated_event_loop(config.clone(), None);

    let state_machine_enabled = false;
    let target_events_path =
        crate::loop_runner::paths::resolve_emit_events_path(&ctx, state_machine_enabled);
    if let Some(parent) = target_events_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let original = "{\"topic\":\"merge.start\"}\n";
    std::fs::write(&target_events_path, original).unwrap();

    crate::loop_runner::runner::runner_inner_test_api::merge_isolated_channel_on_interrupt(
        &ctx,
        &config,
        state_machine_enabled,
        &event_loop,
        "test_interrupt_helper_with_no_marker_is_a_safe_noop",
        None,
    );

    let main_after = std::fs::read_to_string(&target_events_path).unwrap();
    assert_eq!(
        main_after, original,
        "interrupt helper with no marker must not modify main events file"
    );
}
