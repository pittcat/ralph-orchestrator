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
use crate::test_support::CwdGuard;
use ralph_core::HatRegistry;
use ralph_core::planning_session::{ConversationEntry, ConversationType};
use ralph_proto::{Hat, Topic};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::sync::{Arc, Mutex};
use super::super::common::*;
use super::super::fake_path::*;
use super::helpers::*;

// Test: u8_build_termination_diagnostics_returns_none_when_disabled
#[test]
fn u8_build_termination_diagnostics_returns_none_when_disabled() {
    // diagnostics disabled → no hint, no seed. Even with a payload
    // contract violation reference, the operator-facing artifacts
    // stay out of summary.md / diagnosis-summary.json.
    let tmp = tempfile::TempDir::new().unwrap();
    let event_loop = build_u8_event_loop(tmp.path().to_path_buf(), false);

    let pair = build_termination_diagnostics(&event_loop, Some(".ralph/diagnostics/report.json"));
    assert!(
        pair.is_none(),
        "build_termination_diagnostics must return None when diagnostics are disabled, got: {:?}",
        pair
    );
}

// Test: u8_build_termination_diagnostics_returns_hint_and_seed_when_enabled
#[test]
fn u8_build_termination_diagnostics_returns_hint_and_seed_when_enabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let event_loop = build_u8_event_loop(tmp.path().to_path_buf(), true);

    let (hint, seed) = build_termination_diagnostics(&event_loop, None)
        .expect("hint + seed must be Some when diagnostics are enabled");

    // Workspace-relative session path with no `..` and the literal
    // `.ralph/diagnostics/<id>` layout that the rest of the pipeline
    // (U3, U7) expects.
    let session_relpath = hint
        .session_relpath
        .as_deref()
        .expect("session_relpath must be set when diagnostics enabled");
    assert!(
        session_relpath.starts_with(".ralph/diagnostics/"),
        "session_relpath must be a workspace-relative diagnostics path, got: {session_relpath}"
    );
    assert_eq!(
        session_relpath.trim_start_matches(".ralph/diagnostics/"),
        seed.session_id
    );
    assert!(hint.diagnose_command.is_some());
    assert!(
        hint.references.is_empty(),
        "no violation reference was supplied, references must be empty"
    );

    // Seed sanity: schema version and journal paths are aligned.
    assert_eq!(
        seed.schema_version,
        ralph_core::diagnostics::DiagnosisSummary::SCHEMA_VERSION
    );
    assert_eq!(
        seed.recovery_journal_path.as_deref(),
        Some(".ralph/diagnostics/<id>/recovery.jsonl")
            .map(|s| s.replace("<id>", &seed.session_id))
            .as_deref()
            .or(Some(
                format!(".ralph/diagnostics/{}/recovery.jsonl", seed.session_id).as_str()
            ))
    );
    assert!(seed.loop_terminated_at.is_some());
    assert_eq!(seed.total_iterations, Some(event_loop.state().iteration));
}

// Test: u8_build_termination_diagnostics_includes_violation_reference
#[test]
fn u8_build_termination_diagnostics_includes_violation_reference() {
    let tmp = tempfile::TempDir::new().unwrap();
    let event_loop = build_u8_event_loop(tmp.path().to_path_buf(), true);

    let relpath = ".ralph/diagnostics/payload-contract-error-2026-06-05T12-34-56-789Z.json";
    let (hint, _seed) =
        build_termination_diagnostics(&event_loop, Some(relpath)).expect("hint+seed must be Some");

    assert_eq!(hint.references.len(), 1);
    let reference = &hint.references[0];
    assert_eq!(reference.label, "Payload contract violation report");
    assert_eq!(reference.relpath, relpath);
}

// Test: u8_write_termination_diagnostics_emits_seed_and_hint_when_enabled
#[test]
fn u8_write_termination_diagnostics_emits_seed_and_hint_when_enabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let event_loop = build_u8_event_loop(tmp.path().to_path_buf(), true);
    let ctx = ralph_core::LoopContext::primary(tmp.path().to_path_buf());
    let summary_writer = ralph_core::SummaryWriter::from_context(&ctx);

    // First write the summary body (handle_termination does this).
    summary_writer
        .write(
            &ralph_core::TerminationReason::CompletionPromise,
            event_loop.state(),
            None,
            Some("deadbeef: feat: example"),
        )
        .expect("summary.md must be writable");

    write_termination_diagnostics(&event_loop, &summary_writer, None);

    // Hint must be appended to summary.md.
    let summary_path = tmp.path().join(".ralph/agent/summary.md");
    let summary_body = std::fs::read_to_string(&summary_path).unwrap();
    assert!(
        summary_body.contains("## Diagnostics"),
        "summary.md must contain a ## Diagnostics section, got:\n{summary_body}"
    );
    assert!(
        summary_body.contains("Run: `ralph diagnose --session latest`"),
        "summary.md must surface the diagnose command:\n{summary_body}"
    );

    // Seed must be written under the session directory.
    let session_id = event_loop
        .diagnostics()
        .session_id()
        .expect("session_id must be present when diagnostics are enabled");
    let actual_session_dir = event_loop
        .diagnostics()
        .session_dir()
        .expect("session_dir must be present when diagnostics are enabled");
    let seed_path = actual_session_dir.join("diagnosis-summary.json");
    assert!(
        seed_path.exists(),
        "diagnosis-summary.json must be written at: {}",
        seed_path.display()
    );
    let seed_body = std::fs::read_to_string(&seed_path).unwrap();
    let parsed: ralph_core::diagnostics::DiagnosisSummary =
        serde_json::from_str(&seed_body).expect("seed must round-trip through DiagnosisSummary");
    assert_eq!(parsed.session_id, session_id);
    assert_eq!(
        parsed.schema_version,
        ralph_core::diagnostics::DiagnosisSummary::SCHEMA_VERSION
    );
}

// Test: u8_write_termination_diagnostics_is_noop_when_disabled
#[test]
fn u8_write_termination_diagnostics_is_noop_when_disabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let event_loop = build_u8_event_loop(tmp.path().to_path_buf(), false);
    let ctx = ralph_core::LoopContext::primary(tmp.path().to_path_buf());
    let summary_writer = ralph_core::SummaryWriter::from_context(&ctx);

    summary_writer
        .write(
            &ralph_core::TerminationReason::CompletionPromise,
            event_loop.state(),
            None,
            None,
        )
        .unwrap();
    let summary_path = tmp.path().join(".ralph/agent/summary.md");
    let before = std::fs::read_to_string(&summary_path).unwrap();

    write_termination_diagnostics(&event_loop, &summary_writer, None);

    let after = std::fs::read_to_string(&summary_path).unwrap();
    assert_eq!(
        before, after,
        "summary.md must not change when diagnostics are disabled"
    );
    assert!(!after.contains("## Diagnostics"));

    // The disabled collector has no session directory, so no seed
    // path can be constructed.
    assert!(event_loop.diagnostics().session_dir().is_none());
}

// Test: u8_write_termination_diagnostics_emits_violation_reference_when_enabled
#[test]
fn u8_write_termination_diagnostics_emits_violation_reference_when_enabled() {
    // Payload contract violation: hint must point at the root-level
    // report, and the seed must still be written under the session
    // directory. The U4 hard gate writes
    // `<workspace>/.ralph/diagnostics/payload-contract-error-*.json`
    // at the workspace root (NOT inside the session dir), and the U8
    // hint must surface that exact path so the operator can follow it.
    let tmp = tempfile::TempDir::new().unwrap();
    let event_loop = build_u8_event_loop(tmp.path().to_path_buf(), true);
    let ctx = ralph_core::LoopContext::primary(tmp.path().to_path_buf());
    let summary_writer = ralph_core::SummaryWriter::from_context(&ctx);

    summary_writer
        .write(
            &ralph_core::TerminationReason::PayloadContractViolation,
            event_loop.state(),
            None,
            None,
        )
        .unwrap();

    let relpath = ".ralph/diagnostics/payload-contract-error-2026-06-05T12-34-56-789Z.json";
    write_termination_diagnostics(&event_loop, &summary_writer, Some(relpath));

    let summary_body = std::fs::read_to_string(tmp.path().join(".ralph/agent/summary.md")).unwrap();
    assert!(
        summary_body.contains("## Diagnostics"),
        "summary.md must contain a Diagnostics section:\n{summary_body}"
    );
    assert!(
        summary_body.contains(&format!("Payload contract violation report: `{relpath}`")),
        "summary.md must surface the violation reference:\n{summary_body}"
    );
}

// Test: u8_write_termination_diagnostics_drops_violation_reference_when_disabled
#[test]
fn u8_write_termination_diagnostics_drops_violation_reference_when_disabled() {
    // The plan's "diagnostics disabled" contract is strict: even a
    // payload contract violation reference must not surface an
    // empty-path section. The violation is still on disk and
    // surfaced on stderr by U4; the operator-facing summary hint
    // follows the same opt-in as `ralph diagnose`.
    let tmp = tempfile::TempDir::new().unwrap();
    let event_loop = build_u8_event_loop(tmp.path().to_path_buf(), false);
    let ctx = ralph_core::LoopContext::primary(tmp.path().to_path_buf());
    let summary_writer = ralph_core::SummaryWriter::from_context(&ctx);
    summary_writer
        .write(
            &ralph_core::TerminationReason::PayloadContractViolation,
            event_loop.state(),
            None,
            None,
        )
        .unwrap();
    let summary_path = tmp.path().join(".ralph/agent/summary.md");
    let before = std::fs::read_to_string(&summary_path).unwrap();

    write_termination_diagnostics(
        &event_loop,
        &summary_writer,
        Some(".ralph/diagnostics/payload-contract-error-2026-06-05T12-34-56-789Z.json"),
    );

    let after = std::fs::read_to_string(&summary_path).unwrap();
    assert_eq!(before, after);
    assert!(!after.contains("## Diagnostics"));
    assert!(!after.contains("Payload contract violation"));
}

// Test: sc5_build_termination_diagnostics_counts_reflect_idempotent_log
#[test]
fn sc5_build_termination_diagnostics_counts_reflect_idempotent_log() {
    use ralph_core::event_loop::idempotent_wiring;
    use ralph_core::state::idempotent_log::IdempotentLog;

    let tmp = tempfile::TempDir::new().unwrap();
    let event_loop = build_u8_event_loop(tmp.path().to_path_buf(), true);

    // Seed the IdempotentLog directly through the wiring layer so
    // we exercise the same path the runtime uses. The 4th record
    // (`task:open:...`) is NOT final — `from_final_records` must
    // ignore it.
    let workspace = tmp.path().join(".ralph");
    let mut log = IdempotentLog::open(&workspace, "sc5").expect("open idempotent log");
    idempotent_wiring::write_recovery(
        &mut log,
        "r1",
        "sc5",
        serde_json::json!({"reason_code": "semantic_gate_violation"}),
        true,
    )
    .unwrap();
    idempotent_wiring::write_recovery(
        &mut log,
        "r2",
        "sc5",
        serde_json::json!({"reason_code": "missing_required_fields"}),
        true,
    )
    .unwrap();
    idempotent_wiring::write_recovery(
        &mut log,
        "r3",
        "sc5",
        serde_json::json!({"reason_code": "verdict_gate_misalignment"}),
        true,
    )
    .unwrap();
    idempotent_wiring::write_drift(
        &mut log,
        "d1",
        "sc5",
        serde_json::json!({"finding": "schema_drift"}),
    )
    .unwrap();
    idempotent_wiring::write_task(
        &mut log,
        "open",
        "sc5",
        serde_json::json!({"status": "in_progress"}),
        false,
    )
    .unwrap();
    drop(log);

    // Push the seeded log into the live EventLoop so
    // `build_termination_diagnostics` reads the same records
    // through `EventLoop::idempotent_log()`.
    {
        let log_mutex = event_loop.idempotent_log();
        let mut guard = log_mutex.lock().expect("idempotent_log poisoned");
        *guard = IdempotentLog::open(&workspace, "sc5").expect("reopen");
        let _ = guard.replay();
    }

    let (_hint, seed) = build_termination_diagnostics(&event_loop, None)
        .expect("hint + seed must be Some when diagnostics are enabled");

    assert_eq!(
        seed.recovery_count, 3,
        "SC-5: recovery_count must equal the 3 `_final=true` recovery records on disk"
    );
    assert_eq!(
        seed.drift_finding_count, 1,
        "SC-5: drift_finding_count must equal the 1 `_final=true` drift record on disk"
    );

    // Notes must surface the SC-5 data source so operators can grep
    // the same counts via `ralph diagnose` + `jq`.
    assert!(
        seed.notes
            .iter()
            .any(|n| n.contains("IdempotentLog.final_records()")),
        "notes must attribute the count source to IdempotentLog; got: {:?}",
        seed.notes
    );
}

// Test: sc5_build_termination_diagnostics_zero_when_idempotent_log_empty
#[test]
fn sc5_build_termination_diagnostics_zero_when_idempotent_log_empty() {
    // Fresh event loop with no wiring writes — counts must be 0,
    // not whatever line count the legacy recovery.jsonl happens to
    // have. This is the regression guard for the bug where
    // `recovery_count` was a line count of legacy journals.
    let tmp = tempfile::TempDir::new().unwrap();
    let event_loop = build_u8_event_loop(tmp.path().to_path_buf(), true);

    let (_hint, seed) = build_termination_diagnostics(&event_loop, None)
        .expect("hint + seed must be Some when diagnostics are enabled");

    assert_eq!(
        seed.recovery_count, 0,
        "fresh loop with no IdempotentLog records must report recovery_count=0"
    );
    assert_eq!(
        seed.drift_finding_count, 0,
        "fresh loop with no IdempotentLog records must report drift_finding_count=0"
    );
}

// Test: u6_lint_gate_passes_clean_config
#[test]
fn u6_lint_gate_passes_clean_config() {
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    description: "Plans work"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Plan."
  executor:
    name: "Executor"
    description: "Executes work"
    triggers: ["work.ready"]
    publishes: ["work.done"]
    instructions: "Execute."
event_loop:
  starting_event: "work.start"
  completion_promise: "work.done"
topic_owners:
  work.ready: ["coordinator"]
  work.done: ["executor"]
topic_format_whitelist:
  - "LOOP_COMPLETE"
tasks:
  enabled: false
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let result = enforce_preset_lint_gate(&config, false);
    assert!(
        result.is_ok(),
        "clean config must pass lint gate: {:?}",
        result
    );
}

// Test: u6_lint_gate_rejects_unauthorized_publish
#[test]
fn u6_lint_gate_rejects_unauthorized_publish() {
    // `executor` publishes `work.ready` which is owned by `coordinator`.
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    description: "Plans work"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Plan."
  executor:
    name: "Executor"
    description: "Executes work"
    triggers: ["work.ready"]
    publishes: ["work.ready", "work.done"]
    instructions: "Execute."
event_loop:
  starting_event: "work.start"
  completion_promise: "work.done"
topic_owners:
  work.ready: ["coordinator"]
  work.done: ["executor"]
tasks:
  enabled: false
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).unwrap();

    // Build a tempdir-shaped `.ralph/` that mirrors what `ralph run`
    // would normally create. The gate must NOT create `.ralph/events.jsonl`
    // (or any other artifact) on the failure path — R9 says the gate is
    // read-only. We also seed a `events.jsonl` that already exists; if the
    // gate ever opened it for write we would still see the original size
    // (the assertion below covers the "was never opened for write" case
    // by checking the file's size AND mtime, in addition to its existence).
    let temp = tempfile::tempdir().expect("tempdir");
    let ralph_dir = temp.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).expect("create .ralph");
    let events_path = ralph_dir.join("events.jsonl");
    std::fs::write(&events_path, "PRE-EXISTING\n").expect("seed events.jsonl");
    let events_metadata_before =
        std::fs::metadata(&events_path).expect("stat pre-existing events.jsonl");
    let events_modified_before = events_metadata_before
        .modified()
        .expect("mtime pre-existing events.jsonl");

    // Run the gate in a context where cwd points at the tempdir so any
    // relative path lookup (current-events marker, etc.) resolves inside
    // the controlled `.ralph/`. This is the only place the gate could
    // legally write today, and we want any such write to fail loudly.
    let _cwd_guard = CwdGuard::set(temp.path());

    let result = enforce_preset_lint_gate(&config, false);
    assert!(result.is_err(), "unauthorized publish must fail lint gate");
    let err = result.unwrap_err();
    assert!(err.error_count > 0, "must have at least one error finding");
    assert!(
        err.findings
            .iter()
            .any(|f| f.id.contains("cross_hat_unauthorized_publish")),
        "must report cross_hat_unauthorized_publish finding, got: {:?}",
        err.findings.iter().map(|f| &f.id).collect::<Vec<_>>()
    );

    // P0 #2: real filesystem assertion — the gate must not have created
    // any `.ralph/` artifact, and the pre-existing `events.jsonl` must be
    // untouched (size unchanged + mtime unchanged).
    assert!(
        events_path.exists(),
        ".ralph/events.jsonl must still exist (we seeded it; gate must not delete it)"
    );
    let events_metadata_after =
        std::fs::metadata(&events_path).expect("stat post-gate events.jsonl");
    assert_eq!(
        events_metadata_after.len(),
        events_metadata_before.len(),
        ".ralph/events.jsonl size must be unchanged (gate is R9 read-only)"
    );
    let events_modified_after = events_metadata_after
        .modified()
        .expect("mtime post-gate events.jsonl");
    assert_eq!(
        events_modified_after, events_modified_before,
        ".ralph/events.jsonl mtime must be unchanged (gate must not write to it)"
    );

    // The exact-finding assertion remains from the original test.
    assert_eq!(
        err.error_count, 1,
        "exactly one error (the cross-hat finding)"
    );
}

// Test: u6_lint_gate_whitelist_only_exempts_listed_tokens
#[test]
fn u6_lint_gate_whitelist_only_exempts_listed_tokens() {
    // Config with LOOP_COMPLETE (whitelisted) and REVIEW_COMPLETE (not whitelisted).
    let yaml = r#"
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["REVIEW_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "work.done"
topic_format_whitelist:
  - "LOOP_COMPLETE"
tasks:
  enabled: false
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let result = enforce_preset_lint_gate(&config, false);
    // REVIEW_COMPLETE is not whitelisted → lint finding (warn in default,
    // but the gate runs in strict mode, so it's still a finding).
    // The gate only fails on Error findings, and invalid_topic_format
    // is Warn even in strict. However, the gate surfaces warnings.
    // The key assertion: the gate MUST surface the finding.
    match result {
        Ok(()) => {
            // If it passes, the finding was only a warning (not error).
            // That's acceptable — the gate only blocks on errors.
            // But we need to verify the finding exists in the report.
            let findings = ralph_core::preset_lint::run_preset_lint(
                &config,
                ralph_core::preset_lint::LintStrictness::Strict,
                false,
                None,
            );
            assert!(
                findings
                    .iter()
                    .any(|f| f.id.contains("invalid_topic_format")
                        && f.details.get("topic").map(|s| s.as_str()) == Some("REVIEW_COMPLETE")),
                "REVIEW_COMPLETE must produce invalid_topic_format finding"
            );
        }
        Err(err) => {
            // If it fails, verify the finding is about REVIEW_COMPLETE.
            assert!(
                err.findings
                    .iter()
                    .any(|f| f.id.contains("invalid_topic_format")
                        && f.details.get("topic").map(|s| s.as_str()) == Some("REVIEW_COMPLETE")),
                "must report invalid_topic_format for REVIEW_COMPLETE"
            );
        }
    }

    // Now verify LOOP_COMPLETE (whitelisted) does NOT produce a finding.
    let findings = ralph_core::preset_lint::run_preset_lint(
        &config,
        ralph_core::preset_lint::LintStrictness::Strict,
        false,
        None,
    );
    let loop_complete_findings: Vec<_> = findings
        .iter()
        .filter(|f| {
            f.id.contains("invalid_topic_format")
                && f.details.get("topic").map(|s| s.as_str()) == Some("LOOP_COMPLETE")
        })
        .collect();
    assert!(
        loop_complete_findings.is_empty(),
        "LOOP_COMPLETE must NOT produce invalid_topic_format finding (it is whitelisted)"
    );
}

// Test: u6_lint_gate_missing_coordinator_reports_candidates
#[test]
fn u6_lint_gate_missing_coordinator_reports_candidates() {
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    description: "Plans work"
    triggers: ["work.start"]
    publishes: ["work.ready", "task.created"]
    instructions: "Plan."
  executor:
    name: "Executor"
    description: "Executes work"
    triggers: ["work.ready"]
    publishes: ["work.done"]
    instructions: "Execute."
event_loop:
  starting_event: "work.start"
  completion_promise: "work.done"
tasks:
  enabled: true
  coordinator_hats: []
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let result = enforce_preset_lint_gate(&config, false);
    assert!(result.is_err(), "missing coordinator must fail lint gate");
    let err = result.unwrap_err();
    // Should have coordinator_missing finding.
    let coord_missing: Vec<_> = err
        .findings
        .iter()
        .filter(|f| f.id.contains("coordinator_missing"))
        .collect();
    assert!(
        !coord_missing.is_empty(),
        "must report coordinator_missing, got: {:?}",
        err.findings.iter().map(|f| &f.id).collect::<Vec<_>>()
    );
    // The action_hint should list candidate hats that publish task.*.
    let has_candidate_hint = coord_missing.iter().any(|f| {
        f.action_hint
            .as_ref()
            .map(|h| h.contains("coordinator"))
            .unwrap_or(false)
    });
    assert!(
        has_candidate_hint,
        "coordinator_missing must include candidate hat names in action_hint"
    );
}

// Test: u6_lint_gate_task_publisher_not_coordinated
#[test]
fn u6_lint_gate_task_publisher_not_coordinated() {
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    description: "Plans work"
    triggers: ["work.start"]
    publishes: ["work.ready", "task.created"]
    instructions: "Plan."
  executor:
    name: "Executor"
    description: "Executes work"
    triggers: ["work.ready"]
    publishes: ["work.done", "task.updated"]
    instructions: "Execute."
event_loop:
  starting_event: "work.start"
  completion_promise: "work.done"
tasks:
  enabled: true
  coordinator_hats:
    - coordinator
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let result = enforce_preset_lint_gate(&config, false);
    assert!(
        result.is_err(),
        "task publisher not in coordinator_hats must fail"
    );
    let err = result.unwrap_err();
    let task_pub_findings: Vec<_> = err
        .findings
        .iter()
        .filter(|f| f.id.contains("task_publisher_not_coordinated"))
        .collect();
    assert!(
        !task_pub_findings.is_empty(),
        "must report task_publisher_not_coordinated"
    );
    // The finding should mention the executor hat.
    let has_executor = task_pub_findings
        .iter()
        .any(|f| f.message.contains("executor"));
    assert!(
        has_executor,
        "task_publisher_not_coordinated must mention the offending hat"
    );
}

// Test: u6_all_builtin_presets_pass_lint_gate
#[test]
fn u6_all_builtin_presets_pass_lint_gate() {
    use crate::presets::list_presets;
    use ralph_core::RalphConfig;

    let mut failures = Vec::new();
    for preset in &list_presets() {
        let config =
            RalphConfig::parse_yaml(preset.content).expect("embedded preset YAML should parse");
        // 2026-07-09-001 plan (U7): pass `preset.name` so the
        // instructions-OPAC emit-feedback rule can gate on
        // the U7 whitelist. Without this, every builtin
        // preset would fail the new check at once.
        let result =
            crate::loop_runner::preset_lint_gate::enforce_preset_lint_gate_with_preset_name(
                &config,
                false,
                Some(preset.name),
            );
        let Err(err) = result else { continue };
        let blocking_errors = err
            .findings
            .iter()
            .filter(|f| f.severity == ralph_core::runtime_contract::FindingSeverity::Error)
            .filter(|f| {
                !matches!(
                    (preset.name, f.id.as_str()),
                    (
                        "ce-executor-pipeline-loop",
                        "lint.preset.activation_egress_missing"
                            | "lint.preset.handoff_pairing_broken"
                            | "lint.preset.re_emit_trap"
                    )
                )
            })
            .map(|f| format!("{}: {}", f.id, f.message))
            .collect::<Vec<_>>();
        if blocking_errors.is_empty() {
            continue;
        }
        failures.push(format!(
            "'{}': {} error(s) — {:?}",
            preset.name,
            blocking_errors.len(),
            blocking_errors
        ));
    }
    assert!(
        failures.is_empty(),
        "Builtins failed lint gate:\n{}",
        failures.join("\n")
    );
}

// Test: u2_lint_gate_blocks_4_hat_default_coordinator
#[test]
fn u2_lint_gate_blocks_4_hat_default_coordinator() {
    use ralph_core::preset_lint::finding_id::FINDING_MULTI_HAT_REQUIRES_ISOLATED;

    let config = u2_make_n_hat_config(4, "");
    let result = enforce_preset_lint_gate(&config, false);
    let err = result.expect_err("4-hat default coordinator must fail the run gate");
    assert!(err.error_count >= 1, "expected at least 1 error, got {err}");
    let multi_hat_findings: Vec<_> = err
        .findings
        .iter()
        .filter(|f| f.id == format!("lint.{}", FINDING_MULTI_HAT_REQUIRES_ISOLATED))
        .collect();
    assert_eq!(
        multi_hat_findings.len(),
        1,
        "expected exactly one multi_hat_requires_isolated finding, got: {:?}",
        err.findings
            .iter()
            .map(|f| (&f.id, format!("{:?}", f.severity)))
            .collect::<Vec<_>>()
    );
    // Stable finding ID is part of the public contract; downstream
    // dashboards and CI gates key off it.
    assert_eq!(
        multi_hat_findings[0].id,
        format!("lint.{}", FINDING_MULTI_HAT_REQUIRES_ISOLATED)
    );
    // R9: actionable details — actual count and limit are present.
    let finding = multi_hat_findings[0];
    assert!(
        finding.message.contains('4') && finding.message.contains('3'),
        "finding message must include actual=4 and limit=3, got: {}",
        finding.message
    );
    let hint = finding
        .action_hint
        .as_ref()
        .expect("finding must carry an action_hint directing operator to isolated mode");
    assert!(
        hint.contains("isolated"),
        "action_hint must direct to isolated mode, got: {hint}"
    );
}

// Test: u2_lint_gate_passes_3_hat_default_coordinator
#[test]
fn u2_lint_gate_passes_3_hat_default_coordinator() {
    let config = u2_make_n_hat_config(3, "");
    let result = enforce_preset_lint_gate(&config, false);
    assert!(
        result.is_ok(),
        "3-hat default coordinator must pass the run gate, got: {:?}",
        result
    );
}

// Test: u2_lint_gate_blocks_4_hat_after_base_plus_overlay_merge
#[test]
fn u2_lint_gate_blocks_4_hat_after_base_plus_overlay_merge() {
    use ralph_core::config::RalphConfig;

    // P1-3 fix (post-review): the original test name claimed `base 2 hats
    // + overlay 2 hats → after merge 4 hats`. That implies hats are
    // *appended* across base+overlay, but `merge_hats_overlay` actually
    // *replaces* the base's `hats:` block with the overlay's `hats:`
    // block (see `preflight::merge_hats_overlay` and its in-crate
    // tests). The plan's R10 wording was loose about merge semantics;
    // we honor the real merge path: the overlay is the resolved
    // `hats:` source. To exercise the 4-hat gate failure we feed a
    // 4-hat overlay against a minimal base.

    let base: serde_yaml::Value = serde_yaml::from_str(
        r#"
hats:
  alpha:
    name: "Alpha"
    description: "Base hat A"
    triggers: ["work.start"]
    publishes: ["work.intermediate"]
    instructions: "A."
event_loop:
  starting_event: "work.start"
  completion_promise: "work.done"
tasks:
  enabled: false
"#,
    )
    .unwrap();

    // Overlay contributes 4 hats; after `merge_hats_overlay` replaces
    // the base's `hats:` block, the resolved config has 4 hats.
    let overlay: serde_yaml::Value = serde_yaml::from_str(
        r#"
hats:
  gamma:
    name: "Gamma"
    description: "Overlay hat C"
    triggers: ["work.intermediate"]
    publishes: ["work.reviewed"]
    instructions: "C."
  delta:
    name: "Delta"
    description: "Overlay hat D"
    triggers: ["work.reviewed"]
    publishes: ["work.final"]
    instructions: "D."
  epsilon:
    name: "Epsilon"
    description: "Overlay hat E"
    triggers: ["work.final"]
    publishes: ["work.summary"]
    instructions: "E."
  zeta:
    name: "Zeta"
    description: "Overlay hat F"
    triggers: ["work.summary"]
    publishes: ["work.done"]
    instructions: "F."
"#,
    )
    .unwrap();

    // P1-3 fix: use the real CLI merge path to mirror what
    // `ralph run -c base -H overlay` produces. The merge function
    // lives in `crate::preflight::merge_hats_overlay` (made
    // `pub(crate)` in this commit so tests can reach it). We then
    // feed the *merged* config directly to the run gate so the test
    // exercises the full chain: YAML parse → merge overlay → resolved
    // 4-hat config → lint gate.
    let merged_yaml_value = crate::preflight::merge_hats_overlay(base, overlay)
        .expect("merge_hats_overlay should accept valid base + overlay");
    let config: RalphConfig = serde_yaml::from_value(merged_yaml_value)
        .expect("merged YAML should deserialize into RalphConfig");

    assert_eq!(
        config.hats.len(),
        4,
        "P1-3: real merge path replaces base.hats with overlay.hats — \
         resolved config must have 4 hats"
    );
    // Sanity: the four hat IDs must come from the overlay, not the base.
    let names: std::collections::HashSet<&str> = config.hats.keys().map(|h| h.as_str()).collect();
    for expected in ["gamma", "delta", "epsilon", "zeta"] {
        assert!(
            names.contains(expected),
            "P1-3: merged config must contain overlay hat '{expected}'; got hats: {names:?}"
        );
    }
    // And the base hat should be gone (merge replaces, not unions).
    assert!(
        !names.contains("alpha"),
        "P1-3: merged config must NOT contain base hat 'alpha' (merge replaces)"
    );

    let result = enforce_preset_lint_gate(&config, false);
    assert!(
        result.is_err(),
        "P1-3: merged 4-hat config must fail the run gate"
    );
}

// Test: u2_lint_gate_4_hat_isolated_mode_no_multi_hat_finding
#[test]
fn u2_lint_gate_4_hat_isolated_mode_no_multi_hat_finding() {
    use ralph_core::preset_lint::finding_id::FINDING_MULTI_HAT_REQUIRES_ISOLATED;

    let config = u2_make_n_hat_config(4, "execution_mode: isolated");
    let result = enforce_preset_lint_gate(&config, false);
    if let Err(err) = &result {
        let multi_hat_findings: Vec<_> = err
            .findings
            .iter()
            .filter(|f| f.id == format!("lint.{}", FINDING_MULTI_HAT_REQUIRES_ISOLATED))
            .collect();
        assert!(
            multi_hat_findings.is_empty(),
            "isolated 4-hat config must NOT produce multi_hat_requires_isolated, got: {:?}",
            multi_hat_findings
        );
    }
}

// Test: test_adv2_hat_spoofing_rejected_at_merge_layer
#[test]
fn test_adv2_hat_spoofing_rejected_at_merge_layer() {
    use crate::loop_runner::wave::merge_wave_results_to_events_file;
    use ralph_core::{CompletedWave, WaveResult};
    use ralph_proto::Event;
    use std::time::Duration;

    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    std::fs::write(&events_path, "").unwrap();

    // Build a `CompletedWave` with
    // `expected_source_hat = Some("worker")` (the dispatcher's
    // promised hat) and two events: one legitimate
    // (`source = Some("worker")`) and one spoofed
    // (`source = Some("review-coordinator")`).
    let mut event_legit = Event::new("review.dimension.done", "{\"i\":0}");
    event_legit = event_legit.with_source(ralph_proto::HatId::new("worker"));
    let mut event_spoofed = Event::new("review.dimension.done", "{\"i\":1}");
    event_spoofed = event_spoofed.with_source(ralph_proto::HatId::new("review-coordinator"));
    let completed = CompletedWave {
        wave_id: "w-attack".to_string(),
        wave_total: 2,
        results: vec![WaveResult {
            index: 0,
            events: vec![event_legit, event_spoofed],
        }],
        failures: Vec::new(),
        duration: Duration::from_millis(10),
        partial: false,
        expected_source_hat: Some(ralph_proto::HatId::new("worker")),
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: Vec::new(),
    };

    merge_wave_results_to_events_file(
        &completed,
        &events_path,
        &["review.dimension.done".to_string()],
        "worker",
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge must succeed (legitimate event should be admitted)");

    let raw = std::fs::read_to_string(&events_path).expect("read merged");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        1,
        "ADV-2: spoofed event must be dropped; only the legitimate event should be merged; got {} lines: {:?}",
        lines.len(),
        lines
    );
    let v: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
    assert_eq!(
        v["hat"], "worker",
        "ADV-2: merged record's hat must equal the dispatcher's expected_source_hat"
    );
    assert_eq!(
        v["source"], "worker",
        "ADV-2: merged record's source must equal the dispatcher's expected_source_hat"
    );
    assert!(
        !raw.contains("review-coordinator"),
        "ADV-2: spoofed hat name must not appear in merged file"
    );
}

// Test: test_adv2_hat_spoofing_omitted_source_rejected_at_merge_layer
#[test]
fn test_adv2_hat_spoofing_omitted_source_rejected_at_merge_layer() {
    use crate::loop_runner::wave::merge_wave_results_to_events_file;
    use ralph_core::{CompletedWave, WaveResult};
    use ralph_proto::Event;
    use std::time::Duration;

    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    std::fs::write(&events_path, "").unwrap();

    // Worker omitted `source` entirely (None). Even if the
    // round-1 fallback would have passed it through, the
    // new check must drop it.
    let event_no_source = Event::new("review.dimension.done", "{\"i\":0}");
    let completed = CompletedWave {
        wave_id: "w-omitted".to_string(),
        wave_total: 1,
        results: vec![WaveResult {
            index: 0,
            events: vec![event_no_source],
        }],
        failures: Vec::new(),
        duration: Duration::from_millis(10),
        partial: false,
        expected_source_hat: Some(ralph_proto::HatId::new("worker")),
        assigned_dimensions: std::collections::HashMap::new(),
        dimension_retry_counts: std::collections::HashMap::new(),
        worker_events: Vec::new(),
    };

    merge_wave_results_to_events_file(
        &completed,
        &events_path,
        &["review.dimension.done".to_string()],
        "worker",
        // 2026-06-16-001 U2: tests use the same default.
        None,
    )
    .expect("merge must succeed");

    let raw = std::fs::read_to_string(&events_path).expect("read merged");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        0,
        "ADV-2 omitted-source: event with source=None must be dropped; got {} lines: {:?}",
        lines.len(),
        lines
    );
}

// Test: u2_dimension_reviewer_edit_disallowed_triggers_scope_violation_audit
#[cfg(unix)] // git + bash commands; Windows fs semantics differ
#[test]
fn u2_dimension_reviewer_edit_disallowed_triggers_scope_violation_audit() {
    use ralph_core::{EventLoop, HatRegistry, RalphConfig};
    use ralph_proto::HatId;
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    // 1. Set up a real git workspace with a clean HEAD baseline.
    let tmp = TempDir::new().expect("temp dir");
    let workspace = tmp.path().to_path_buf();

    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&workspace)
            .status()
            .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {e}", args));
        assert!(status.success(), "git {:?} must succeed", args);
    };

    run(&["init", "--quiet"]);
    run(&["config", "user.email", "u2@ralph.test"]);
    run(&["config", "user.name", "u2-test"]);
    // A baseline tracked file so HEAD has at least one commit.
    std::fs::write(workspace.join("baseline.txt"), "clean\n").expect("write baseline");
    run(&["add", "baseline.txt"]);
    run(&["commit", "--quiet", "-m", "baseline"]);

    // 2. Modify a tracked file AFTER the baseline commit, so
    //    `git diff --stat HEAD` returns a non-empty diff and the audit
    //    fires.
    std::fs::write(workspace.join("baseline.txt"), "modified\n").expect("modify");

    // 3. Build a minimal `RalphConfig` with a `dimension-reviewer` hat
    //    carrying the U2 R2 contract: `disallowed_tools: ["Edit"]`.
    //    The audit hook only checks for "Edit" or "Write" in the
    //    disallowed list — `Bash` is intentionally left in the allowed
    //    set so the reviewer can use `echo`/`grep`/`cat` for read-only
    //    probes (verification belongs to executor/shipper, not
    //    reviewer; that boundary is enforced via instructions, not the
    //    tool list).
    let mut config: RalphConfig = serde_yaml::from_str(
        r#"
event_loop:
  enforce_hat_scope: false
hats:
  dimension-reviewer:
    name: "Dimension Reviewer"
    description: "U2 hard-audit test fixture"
    triggers: ["review.dimension.ready"]
    publishes: ["review.dimension.done"]
    disallowed_tools: ["Edit"]
"#,
    )
    .expect("fixture yaml must parse");
    // `workspace_root` is `#[serde(skip)]` on `CoreConfig`, so it does
    // not flow through YAML. Set it directly so the audit runs
    // `git diff --stat HEAD` against the test tmp dir, not the
    // worker's CWD.
    config.core.workspace_root = workspace.clone();

    let registry = HatRegistry::from_runtime_config(&config);
    let mut event_loop =
        EventLoop::with_context(config, ralph_core::LoopContext::primary(workspace.clone()));
    // Re-register the registry (from_runtime_config is independent of
    // EventLoop::new which builds its own).
    *event_loop.registry_mut() = registry;

    // 4. Collect any `<hat>.scope_violation` events that hit the bus.
    //    We register a synchronous observer on the bus before invoking
    //    `process_output` so the capture survives any later routing
    //    steps.
    let observed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let observed = Arc::clone(&observed);
        event_loop.bus().add_observer(move |event| {
            let topic = event.topic.as_str().to_string();
            if topic == "dimension-reviewer.scope_violation" {
                observed.lock().unwrap().push(topic);
            }
        });
    }

    // 5. Drive the audit hook via the public `process_output` entry
    //    point. The exact `output` string and `success` flag do not
    //    matter for the audit — they only matter for the prior
    //    parsing / completion steps, which we do not assert on. The
    //    audit runs unconditionally at the end of every
    //    `process_output` call.
    let hat_id = HatId::new("dimension-reviewer");
    let _ = event_loop.process_output(&hat_id, "", true);

    // 6. The audit MUST have fired: the bus received a
    //    `dimension-reviewer.scope_violation` event. This is the hard
    //    enforcement half of the U2 R2 contract — without it, a
    //    reviewer could freely edit source files and the runtime
    //    would never trip.
    let seen = observed.lock().unwrap().clone();
    assert!(
        seen.iter()
            .any(|t| t == "dimension-reviewer.scope_violation"),
        "U2 R2: hard audit must publish dimension-reviewer.scope_violation when \
         tracked files were modified under disallowed_tools=[Edit]. \
         observed topics: {seen:?}"
    );
}

// Test: u2_dimension_reviewer_no_disallowed_tools_does_not_audit
#[cfg(unix)]
#[test]
fn u2_dimension_reviewer_no_disallowed_tools_does_not_audit() {
    use ralph_core::{EventLoop, HatRegistry, RalphConfig};
    use ralph_proto::HatId;
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    let tmp = TempDir::new().expect("temp dir");
    let workspace = tmp.path().to_path_buf();

    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&workspace)
            .status()
            .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {e}", args));
        assert!(status.success(), "git {:?} must succeed", args);
    };

    run(&["init", "--quiet"]);
    run(&["config", "user.email", "u2-neg@ralph.test"]);
    run(&["config", "user.name", "u2-neg-test"]);
    std::fs::write(workspace.join("baseline.txt"), "clean\n").expect("write baseline");
    run(&["add", "baseline.txt"]);
    run(&["commit", "--quiet", "-m", "baseline"]);

    // Modify AFTER baseline so `git diff --stat HEAD` is non-empty.
    std::fs::write(workspace.join("baseline.txt"), "modified\n").expect("modify");

    let mut config: RalphConfig = serde_yaml::from_str(
        r#"
event_loop:
  enforce_hat_scope: false
hats:
  executor:
    name: "Executor (no restrictions)"
    description: "U2 negative test fixture"
    triggers: ["work.start"]
    publishes: ["work.done"]
"#,
    )
    .expect("fixture yaml must parse");
    config.core.workspace_root = workspace.clone();

    let registry = HatRegistry::from_runtime_config(&config);
    let mut event_loop =
        EventLoop::with_context(config, ralph_core::LoopContext::primary(workspace.clone()));
    *event_loop.registry_mut() = registry;

    // Capture every scope_violation event reaching the bus for any hat.
    let observed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let observed = Arc::clone(&observed);
        event_loop.bus().add_observer(move |event| {
            let topic = event.topic.as_str().to_string();
            if topic.ends_with(".scope_violation") {
                observed.lock().unwrap().push(topic);
            }
        });
    }

    let hat_id = HatId::new("executor");
    let _ = event_loop.process_output(&hat_id, "", true);

    let seen = observed.lock().unwrap().clone();
    assert!(
        seen.is_empty(),
        "U2 R2 negative: hat without disallowed Edit/Write MUST NOT trigger the \
         file-modification audit. observed scope_violation topics: {seen:?}"
    );
}
