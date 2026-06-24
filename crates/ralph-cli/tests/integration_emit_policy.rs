//! Integration tests for `ralph emit` policy precheck (Units U1, U2, U5).
//!
//! These tests verify the strict validation mechanisms themselves:
//! - no-policy skip info
//! - isolated mode hat mismatch rejection
//! - ralph pseudo-hat authority (blocklist + allowlist)
//! - payload schema validation (string vs json_object)
//! - malformed RALPH_HATS_SOURCE fail-closed
//! - rejection hint security (no leak of unauthorised topic example)
//! - control topic exemption (loop.cancel, task.resume)
//! - source field default rules (business vs control topic)
//!
//! 2026-06-24: preset-text-specific tests (hardcoded hat/topic/handoff
//! assertions like review-synthesizer+review.passed, executor+work.done,
//! coordinator+review.passed+skip_reason) were removed. The preset only
//! needs to pass strict validation; per-hat/per-topic ownership is
//! covered by the preset_lint suite and the SSOT merge tests.

use std::process::Command;
use tempfile::TempDir;

fn ralph_emit(temp_path: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args(args)
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command")
}

/// Edge path: no preset and no event_policy logs that the policy check is skipped.
#[test]
fn test_emit_without_policy_logs_skip_info() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = ralph_emit(temp_path, &["emit", "debug.step", "task_id=demo"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "emit should stay lenient without policy: stderr={}",
        stderr
    );

    let combined = format!("{} {}", stdout, stderr);
    assert!(
        combined.contains("cli emit policy check skipped: no event_policy in resolved config"),
        "expected skip info log in stdout/stderr; stdout={} stderr={}",
        stdout,
        stderr
    );
}

/// Phase 2: in isolated mode, an agent that tries to override its hat via
/// `--hat` while `RALPH_CURRENT_HAT` points to a different hat is rejected.
#[test]
fn test_emit_isolated_mode_rejects_conflicting_hat_override() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "debug.step",
            "task_id=demo",
            "--hat",
            "review-coordinator",
        ])
        .env("RALPH_CURRENT_HAT", "review-synthesizer")
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "emit should reject conflicting --hat in isolated mode: stderr={}",
        stderr
    );
    assert!(
        stderr.contains("Isolated mode hat mismatch"),
        "expected isolated-mode mismatch error, got: {}",
        stderr
    );

    // The event must not be written anywhere.
    let events_file = temp_path.join(".ralph/events.jsonl");
    assert!(
        !events_file.exists()
            || std::fs::read_to_string(&events_file)
                .unwrap()
                .trim()
                .is_empty(),
        "rejected event must not be written"
    );
}

/// ralph pseudo-hat must NOT be allowed to emit business topics. Allowing it
/// would let a worktree loop's loop runner impersonate downstream hats and
/// advance the workflow as `ralph` — the same impersonation attack the P0
/// origin guard rejects at JSONL read time. The CLI-side guard rejects this
/// at the write boundary so the agent gets immediate backpressure.
///
/// `review.passed` here is an arbitrary business topic sample; the test
/// pins the blocklist side of the ralph pseudo-hat authority guard.
#[test]
fn test_emit_ralph_pseudo_hat_cannot_emit_review_passed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "review.passed",
            "--json",
            r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass"}"#,
            "--hat",
            "ralph",
        ])
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "ralph pseudo-hat + review.passed (business topic) must be rejected, got stderr={stderr}"
    );
    assert!(
        stderr.contains("ralph")
            && (stderr.contains("business topic") || stderr.contains("control topics")),
        "expected ralph pseudo-hat rejection message, got stderr={stderr}"
    );

    // Critical: the event must NOT have landed in events.jsonl.
    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap_or_default();
    assert!(
        !events.contains("review.passed"),
        "rejected event must not be written to events.jsonl, got: {events}"
    );
}

/// Companion of `test_emit_ralph_pseudo_hat_cannot_emit_review_passed`:
/// control topics (`loop.cancel`, `task.resume`, `human.*`) MUST still
/// be accepted when emitted by `ralph`, because the loop / runtime
/// pseudo-hat is the legitimate producer. This pins the allowlist side
/// of the same guard so a future tightening does not break the runner.
#[test]
fn test_emit_ralph_pseudo_hat_can_emit_loop_cancel() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "loop.cancel",
            "--json",
            r#"{"reason":"manual cancel for test"}"#,
            "--hat",
            "ralph",
        ])
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "ralph pseudo-hat + loop.cancel (control topic) must succeed: stdout={stdout} stderr={stderr}"
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap();
    assert!(
        events.contains("loop.cancel") && events.contains("\"hat\":\"ralph\""),
        "expected loop.cancel with hat=ralph in events.jsonl, got: {events}"
    );
}

// -------------------------------------------------------------------------
// Plan 001 §4.3 C1/C4/C5: RALPH_HATS_SOURCE env routes pre-publish check.
// -------------------------------------------------------------------------

/// Run `ralph emit` with an explicit env var, no `-H` flag. The CLI must
/// pick up the preset advertised by `RALPH_HATS_SOURCE`, enforce its
/// `event_policy.schemas`, and refuse to write a string payload for a
/// topic that requires a JSON object (AC-2).
#[test]
fn test_emit_with_env_hats_source_rejects_string_payload_for_work_ready() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "emit",
            "work.ready",
            "long string payload that is not json",
            "--hat",
            "coordinator",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-serial")
        .env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit");

    assert!(
        !output.status.success(),
        "string payload must be rejected when RALPH_HATS_SOURCE preset requires json_object"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("rejected") || stderr.contains("policy") || stderr.contains("required"),
        "stderr should explain rejection: {}",
        stderr
    );
    assert!(
        stderr.contains("Event rejected by")
            && stderr.contains("work.ready")
            && (stderr.contains("ralph emit work.ready --json")
                || stderr.contains("requires payload")
                || stderr.contains("event_policy:payload_type_mismatch")),
        "stderr should expose a hat-aware or schema-aware rejection: {}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    if events_file.exists() {
        let events = std::fs::read_to_string(&events_file).unwrap_or_default();
        assert!(
            !events.contains("long string payload"),
            "rejected payload MUST NOT land on disk: {}",
            events
        );
    }
}

/// AC-8: malformed RALPH_HATS_SOURCE (no such preset) with a workspace
/// config present must fail closed — never silently Skip.
#[test]
fn test_emit_with_malformed_hats_source_fails_closed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();
    // Create a workspace ralph.yml so fail-closed engages.
    std::fs::write(temp_path.join("ralph.yml"), "agent: claude\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args(["emit", "work.ready", "--json", "{}", "--hat", "coordinator"])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:not-a-real-preset")
        .env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit");

    assert!(
        !output.status.success(),
        "malformed RALPH_HATS_SOURCE + workspace config must fail closed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not-a-real-preset") || stderr.contains("Pre-publish"),
        "stderr should explain the fail-closed reason: {}",
        stderr
    );
}

/// AC-4: when a hat tries to emit a topic it has no authority for, the
/// rejection hint must NOT leak the unauthorised payload example. This is
/// the hat-scoping rule from plan 001 §4.1. `work.ready` + `executor` are
/// samples — the test pins the rejection-hint security mechanism, not the
/// specific hat/topic ownership.
#[test]
fn test_emit_rejection_hint_excludes_unauthorised_topics() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "emit",
            "work.ready",
            "garbage",
            "--hat",
            // executor is not in the work.ready authorised set
            "executor",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-serial")
        .env("RALPH_CURRENT_HAT", "executor")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "executor must not be allowed to emit work.ready: stderr={}",
        stderr
    );
    // When fix_hint_for_hat_topic returns None (the hat cannot publish
    // the topic), the rejection message must not leak the example
    // `ralph emit work.ready --json '{...}'` form — that would teach
    // the agent to bypass provenance.
    assert!(
        !stderr.contains("ralph emit work.ready --json"),
        "fix hint must not surface an unauthorised-topic payload example: {}",
        stderr
    );
}

// -------------------------------------------------------------------------
// 2026-06-17-004 plan U1 (R1, R2): CLI provenance fail-closed tests.
// -------------------------------------------------------------------------

/// T1.4 Edge: isolated + `hat=ralph` + `loop.cancel` → allowed (control topic
/// exemption for the runtime pseudo-hat).
#[test]
fn test_emit_t1_4_isolated_ralph_hat_loop_cancel_allowed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "loop.cancel",
            "--json",
            r#"{"reason":"demo"}"#,
            "--hat",
            "ralph",
        ])
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "isolated+ralph+loop.cancel must succeed (T1.4 control topic exemption): \
         stdout={stdout} stderr={stderr}"
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap();
    assert!(
        events.contains("loop.cancel"),
        "loop.cancel must be written (T1.4): {}",
        events
    );
}

/// T1.7 Edge: isolated + no hat + `task.resume` → allowed (control topic
/// exemption for the loop's recovery signal).
#[test]
fn test_emit_t1_7_isolated_no_hat_task_resume_allowed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-serial",
            "emit",
            "task.resume",
            "--json",
            r#"{"target_hat":"executor","reason":"missing_event_gate","kind":"missing_field","original_trigger_topic":"review.dimension.ready"}"#,
        ])
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "isolated+no-hat+task.resume must succeed (T1.7 control topic exemption): \
         stdout={stdout} stderr={stderr}"
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap();
    assert!(
        events.contains("task.resume"),
        "task.resume must be written (T1.7): {}",
        events
    );
}

/// U7 (R7): control topics must NOT get a hat-derived source default
/// (they are orchestrator-internal events; source field is left empty or absent).
#[test]
fn test_u7_control_topic_source_unchanged() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    // Without a preset (-H), isolated scope check is skipped, so any hat can emit.
    // ce-executor-serial is isolated-mode but without -H the CLI runs without
    // isolated scope enforcement — still valid for the "control topic bypasses
    // hat-default source" assertion.
    let output = ralph_emit(
        temp_path,
        &[
            "emit",
            "task.resume",
            "--json",
            r#"{"reason":"recovery","target_hat":"coordinator"}"#,
            "--hat",
            "coordinator",
        ],
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "control topic emit must succeed: stderr={}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).expect("events file should exist");
    // task.resume is in RALPH_CONTROL_TOPICS — source must NOT be defaulted to hat
    assert!(
        !events.contains("\"source\":\"coordinator\""),
        "U7 R7: control topic must NOT get hat-default source. Got: {}",
        events
    );
}
