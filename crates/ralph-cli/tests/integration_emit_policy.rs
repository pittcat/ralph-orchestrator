//! Integration tests for `ralph emit` policy precheck (Units U1, U2, U5).
//!
//! These tests verify that:
//! - `ralph emit` honors builtin preset `event_policy` when `-H` is provided.
//! - Payload contract violations are rejected at the CLI entry and leave a
//!   recovery envelope in `.ralph/recovery.jsonl`.
//! - When no preset and no `event_policy` is configured, the CLI logs that the
//!   policy check was skipped.

use std::process::Command;
use tempfile::TempDir;

fn ralph_emit(temp_path: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args(args)
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command")
}

/// Happy path: a conforming `work.done` payload is accepted and written.
#[test]
fn test_emit_with_builtin_preset_accepts_valid_work_done() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = ralph_emit(
        temp_path,
        &[
            "-H",
            "builtin:ce-executor-isolated",
            "emit",
            "work.done",
            "--json",
            r#"{"plan_name":"x","plan_path":"y","task_id":"z","task_key":"k","step":"s","commit_count":1,"changed_lines":10}"#,
            "--hat",
            "executor",
        ],
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "emit should succeed for valid work.done payload: stderr={}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).expect("events file should exist");
    assert!(
        events.contains("\"topic\":\"work.done\""),
        "events: {}",
        events
    );
    assert!(
        events.contains("\"plan_name\":\"x\""),
        "events should contain payload: {}",
        events
    );
}

/// Error path: a JSON object missing required fields for `work.done` is
/// rejected and leaves a recovery envelope.
#[test]
fn test_emit_with_builtin_preset_rejects_missing_required_fields() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = ralph_emit(
        temp_path,
        &[
            "-H",
            "builtin:ce-executor-isolated",
            "emit",
            "work.done",
            "--json",
            r#"{"ok":true}"#,
            "--hat",
            "executor",
        ],
    );

    assert!(
        !output.status.success(),
        "emit should fail for work.done payload missing required fields"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing") || stderr.contains("required"),
        "stderr should explain missing required fields: {}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    assert!(
        !events_file.exists()
            || std::fs::read_to_string(&events_file)
                .unwrap()
                .trim()
                .is_empty(),
        "rejected event must not be written to events file"
    );

    let recovery_file = temp_path.join(".ralph/recovery.jsonl");
    assert!(recovery_file.exists(), "recovery.jsonl should be written");
    let recovery = std::fs::read_to_string(&recovery_file).unwrap();
    let entry: serde_json::Value = recovery
        .lines()
        .next()
        .expect("recovery.jsonl should have at least one line")
        .parse()
        .expect("recovery line should be valid JSON");
    assert_eq!(entry["envelope"]["source"], "cli_emit");
    assert_eq!(entry["envelope"]["topic"], "work.done");
    assert_eq!(entry["envelope"]["source_hat"], "executor");
}

/// Error path: a string payload for `work.done` is rejected and leaves a
/// recovery envelope.
#[test]
fn test_emit_with_builtin_preset_rejects_string_payload() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = ralph_emit(
        temp_path,
        &[
            "-H",
            "builtin:ce-executor-isolated",
            "emit",
            "work.done",
            "free text",
            "--hat",
            "executor",
        ],
    );

    assert!(
        !output.status.success(),
        "emit should fail for string work.done payload"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Payload is not valid JSON") || stderr.contains("payload type mismatch"),
        "stderr should explain payload rejection: {}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    assert!(
        !events_file.exists()
            || std::fs::read_to_string(&events_file)
                .unwrap()
                .trim()
                .is_empty(),
        "rejected event must not be written to events file"
    );

    let recovery_file = temp_path.join(".ralph/recovery.jsonl");
    assert!(recovery_file.exists(), "recovery.jsonl should be written");
    let recovery = std::fs::read_to_string(&recovery_file).unwrap();
    let entry: serde_json::Value = recovery
        .lines()
        .next()
        .expect("recovery.jsonl should have at least one line")
        .parse()
        .expect("recovery line should be valid JSON");
    assert_eq!(entry["envelope"]["source"], "cli_emit");
    assert_eq!(
        entry["envelope"]["reason_code"],
        "payload_contract_violation"
    );
    assert_eq!(entry["envelope"]["topic"], "work.done");
    assert_eq!(entry["envelope"]["outcome"], "not_retriable");
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
            "builtin:ce-executor-isolated",
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

/// Phase 3: hat-aware allowed values reject review-coordinator emitting
/// review.passed(skip_reason=aggregate_timeout) before it reaches jsonl.
#[test]
fn test_emit_isolated_mode_rejects_coordinator_aggregate_timeout() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-isolated",
            "emit",
            "review.passed",
            "--json",
            r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"aggregate_timeout"}"#,
            "--hat",
            "review-coordinator",
        ])
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "emit should reject review-coordinator + aggregate_timeout: stderr={}",
        stderr
    );
    assert!(
        stderr.contains("review-coordinator") && stderr.contains("aggregate_timeout"),
        "expected hat-aware rejection message, got: {}",
        stderr
    );

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

/// Phase 2: in isolated mode, when --hat agrees with RALPH_CURRENT_HAT the
/// emit proceeds normally (provided the topic is in the hat's `publishes`).
///
/// 2026-06-17-003 plan U1: this test previously emitted `debug.step` from
/// `review-synthesizer` — a hat that does not own `debug.step`. The test
/// was capturing the pre-U1 behaviour where isolated-scope was only
/// enforced at loop runtime (events would land in events.jsonl and be
/// dropped silently). U1 closes that precheck gap; the test now emits a
/// topic the hat actually owns (`review.passed`) and asserts the
/// provenance-override path still works.
#[test]
fn test_emit_isolated_mode_allows_matching_hat() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-isolated",
            "emit",
            "review.passed",
            r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"aggregate_timeout"}"#,
            "--hat",
            "review-synthesizer",
        ])
        .env("RALPH_CURRENT_HAT", "review-synthesizer")
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "emit should succeed when --hat matches env: stderr={}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    let events = std::fs::read_to_string(&events_file).unwrap();
    assert!(events.contains("\"hat\":\"review-synthesizer\""));
    assert!(events.contains("review.passed"));
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
            "中文长字符串 payload",
            "--hat",
            "coordinator",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-isolated")
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
    // Plan 001 AC-4 / §4.3 C3: stderr should expose a copy-pasteable
    // `ralph emit work.ready --json ...` example restricted to topics
    // the active hat may publish. The legacy bare rejection would NOT
    // contain the example line, so this assertion also catches drift.
    assert!(
        stderr.contains("ralph emit work.ready --json"),
        "stderr should expose the schema-aware fix hint: {}",
        stderr
    );

    let events_file = temp_path.join(".ralph/events.jsonl");
    if events_file.exists() {
        let events = std::fs::read_to_string(&events_file).unwrap_or_default();
        assert!(
            !events.contains("中文长字符串"),
            "rejected payload MUST NOT land on disk: {}",
            events
        );
    }
}

/// AC-3: with RALPH_HATS_SOURCE, a properly-formed JSON payload is
/// accepted and written to the events file.
#[test]
fn test_emit_with_env_hats_source_accepts_valid_json_payload() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "emit",
            "work.ready",
            "--json",
            r#"{"plan_name":"p","plan_path":"/tmp/p","task_id":"t","task_key":"k","step":"s","complexity":3}"#,
            "--hat",
            "coordinator",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-isolated")
        .env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "valid payload must succeed when RALPH_HATS_SOURCE preset is honoured: stderr={}",
        stderr
    );
    let events = std::fs::read_to_string(temp_path.join(".ralph/events.jsonl")).unwrap();
    assert!(events.contains("\"topic\":\"work.ready\""));
    assert!(events.contains("\"plan_name\":\"p\""));
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

/// AC-7: `ralph wave emit` honours RALPH_HATS_SOURCE — a batch with a
/// missing required field is rejected and no candidate events are
/// written.
#[test]
fn test_wave_emit_with_env_hats_source_rejects_missing_required_field() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads",
            r#"{"dim":"d1"}"#,
            r#"{"dim":"d2"}"#,
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-wave")
        .env("RALPH_CURRENT_HAT", "dimension-reviewer")
        .env("RALPH_WAVE_WORKER", "1")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph wave emit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "missing required field in batch must reject: stderr={} stdout={}",
        stderr,
        stdout
    );

    let candidate = temp_path.join(".ralph/candidate-events.jsonl");
    if candidate.exists() {
        let contents = std::fs::read_to_string(&candidate).unwrap_or_default();
        assert!(
            contents.trim().is_empty(),
            "rejected batch must NOT write candidate-events: {}",
            contents
        );
    }
}

/// AC-4: when an executor-hat child process tries to emit a topic it has
/// no authority for (e.g. `work.ready`, which only `coordinator`/`plan-gate`
/// may publish), the rejection hint must NOT leak the unauthorised
/// payload example. This is the hat-scoping rule from plan 001 §4.1.
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
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-isolated")
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
