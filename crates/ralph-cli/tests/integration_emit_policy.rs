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
