//! Integration tests for `ralph diagnose` (U7).
//!
//! These tests spawn the compiled `ralph` binary against a
//! hand-built `.ralph/diagnostics/<session>/` tree. They exercise
//! the CLI surface (exit codes, stdout / stderr discipline,
//! `--format json`, `--output`) end-to-end so the contract from
//! the U7 plan is enforced at the binary boundary, not just at the
//! reporter API.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn ralph_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ralph"))
}

fn write_recovery_entry(dir: &Path) {
    let entry = "{\"schema_version\":1,\"envelope\":{\"schema_version\":1,\"diagnosis_id\":\"d1\",\"iteration\":1,\"source\":\"missing_event_gate\",\"severity\":\"error\",\"reason_code\":\"no_emit\",\"message\":\"builder did not emit work.done\",\"source_hat\":\"builder\",\"target_hat\":\"builder\",\"topic\":\"work.done\",\"retry_key\":\"missing_event_gate:builder:work_done:no_emit:*\",\"retry_attempt\":0,\"safe_target\":true,\"outcome\":\"pending\",\"timestamp\":\"2026-06-05T10:20:30Z\"},\"iteration\":1,\"timestamp\":\"2026-06-05T10:20:30Z\"}\n";
    fs::write(dir.join("recovery.jsonl"), entry).unwrap();
}

fn write_drift_entry(dir: &Path) {
    let entry = "{\"schema_version\":1,\"finding_id\":\"f1\",\"metric\":\"field_completeness\",\"observed_value\":0.4,\"threshold\":0.9,\"severity\":\"warning\",\"topic\":\"work.done\",\"field\":\"plan_name\",\"window_iterations\":20,\"message\":\"plan_name missing 60% of the time\",\"timestamp\":\"2026-06-05T10:20:30Z\",\"iteration\":3}\n";
    fs::write(dir.join("drift.jsonl"), entry).unwrap();
}

fn write_summary(dir: &Path) {
    let summary = r#"{
  "schema_version": 1,
  "session_id": "2026-06-05T10-20-30",
  "generated_at": "2026-06-05T10:20:30Z",
  "loop_started_at": "2026-06-05T10:20:00Z",
  "loop_terminated_at": "2026-06-05T10:20:40Z",
  "total_iterations": 12,
  "termination_reason": "completion_promise",
  "recovery_journal_path": "recovery.jsonl",
  "drift_journal_path": "drift.jsonl",
  "orchestration_log_path": "orchestration.jsonl",
  "errors_log_path": "errors.jsonl",
  "recovery_count": 1,
  "drift_finding_count": 1,
  "notes": []
}"#;
    fs::write(dir.join("diagnosis-summary.json"), summary).unwrap();
}

fn write_orchestration(dir: &Path) {
    let entry = r#"{"timestamp":"2026-06-05T10:20:01Z","iteration":1,"hat":"builder","event":{"type":"hat_selected","hat":"builder","reason":"tasks_ready"}}"#;
    fs::write(dir.join("orchestration.jsonl"), format!("{entry}\n")).unwrap();
}

fn write_errors(dir: &Path) {
    let entry = r#"{"ts":"2026-06-05T10:20:02Z","iteration":1,"hat":"builder","error_type":"parse_error","message":"bad json","context":{}}"#;
    fs::write(dir.join("errors.jsonl"), format!("{entry}\n")).unwrap();
}

fn fresh_session(tmp: &TempDir, name: &str) -> std::path::PathBuf {
    let dir = tmp.path().join(".ralph").join("diagnostics").join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn no_sessions_exits_non_zero() {
    let tmp = TempDir::new().unwrap();
    let output = ralph_bin()
        .arg("diagnose")
        .arg("--diagnostics-root")
        .arg(tmp.path().join(".ralph/diagnostics"))
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(!output.status.success(), "exit code must be non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no diagnostics sessions"),
        "stderr should hint at missing sessions, got: {stderr}"
    );
    assert!(
        stderr.contains("RALPH_DIAGNOSTICS=1") || stderr.contains("telemetry.runtime_diagnosis"),
        "stderr should re-run hint, got: {stderr}"
    );
}

#[test]
fn default_session_renders_markdown_to_stdout() {
    let tmp = TempDir::new().unwrap();
    let diag = tmp.path().join(".ralph/diagnostics");
    fs::create_dir_all(&diag).unwrap();
    // Older session must be ignored.
    let _old = fresh_session(&tmp, "2026-05-01T00-00-00");
    // Latest session with full content.
    let session = fresh_session(&tmp, "2026-06-05T10-20-30");
    write_recovery_entry(&session);
    write_drift_entry(&session);
    write_summary(&session);
    write_orchestration(&session);
    write_errors(&session);

    let output = ralph_bin()
        .arg("diagnose")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "exit code should be 0, got {:?}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# Ralph Diagnose Report"));
    assert!(stdout.contains("## Top findings"));
    assert!(stdout.contains("## Recovery timeline"));
    assert!(stdout.contains("## Drift findings"));
    assert!(stdout.contains("missing_event_gate:builder:work_done:no_emit:*"));
}

#[test]
fn latest_ignores_logs_and_payload_contract_files() {
    let tmp = TempDir::new().unwrap();
    let diag = tmp.path().join(".ralph/diagnostics");
    fs::create_dir_all(diag.join("logs")).unwrap();
    // Root-level violation report must be ignored.
    fs::write(diag.join("payload-contract-error-2026-06-05.json"), "{}").unwrap();
    let _old = fresh_session(&tmp, "2026-05-01T00-00-00");
    let _latest = fresh_session(&tmp, "2026-06-05T10-20-30");

    let output = ralph_bin()
        .arg("diagnose")
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["schema_version"], "1");
    assert_eq!(
        value["session_path"]
            .as_str()
            .unwrap()
            .ends_with("2026-06-05T10-20-30"),
        true
    );
}

#[test]
fn json_format_does_not_emit_markdown_headings() {
    let tmp = TempDir::new().unwrap();
    let session = fresh_session(&tmp, "2026-06-05T10-20-30");
    write_recovery_entry(&session);
    write_summary(&session);

    let output = ralph_bin()
        .arg("diagnose")
        .arg("--format")
        .arg("json")
        .arg("--diagnostics-root")
        .arg(tmp.path().join(".ralph/diagnostics"))
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("## "));
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["schema_version"], "1");
    assert!(!value["top_findings"].as_array().unwrap().is_empty());
}

#[test]
fn output_flag_writes_file_and_keeps_stdout_short() {
    let tmp = TempDir::new().unwrap();
    let session = fresh_session(&tmp, "2026-06-05T10-20-30");
    write_recovery_entry(&session);
    let out = tmp.path().join("report.md");

    let output = ralph_bin()
        .arg("diagnose")
        .arg("--output")
        .arg(&out)
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // stdout must NOT contain the full report; it should only mention the path.
    assert!(
        !stdout.contains("# Ralph Diagnose Report"),
        "stdout should be short summary, got: {stdout}"
    );
    assert!(out.exists(), "report file should be created");
    let body = fs::read_to_string(&out).unwrap();
    assert!(body.contains("# Ralph Diagnose Report"));
}

#[test]
fn missing_recovery_journal_does_not_fail() {
    let tmp = TempDir::new().unwrap();
    // Only the orchestration log exists; no recovery journal.
    let session = fresh_session(&tmp, "2026-06-05T10-20-30");
    write_orchestration(&session);
    write_summary(&session);

    let output = ralph_bin()
        .arg("diagnose")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "missing recovery.jsonl must not fail the report (stderr: {})",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("无 recovery journal"));
    assert!(stdout.contains("Warnings"));
    assert!(stdout.contains("recovery.jsonl"));
}

#[test]
fn malformed_jsonl_is_reported_as_warning_not_failure() {
    let tmp = TempDir::new().unwrap();
    let session = fresh_session(&tmp, "2026-06-05T10-20-30");
    // Two lines: one malformed, one valid.
    fs::write(
        session.join("recovery.jsonl"),
        "not json\n{\"unrelated\":true}\n",
    )
    .unwrap();
    let output = ralph_bin()
        .arg("diagnose")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "malformed line should not fail the report (stderr: {})",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("malformed recovery.jsonl"));
}

#[test]
fn explicit_session_id_under_root() {
    let tmp = TempDir::new().unwrap();
    let session = fresh_session(&tmp, "2026-06-05T10-20-30");
    write_recovery_entry(&session);

    let output = ralph_bin()
        .arg("diagnose")
        .arg("--session")
        .arg("2026-06-05T10-20-30")
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["schema_version"], "1");
    assert!(!value["top_findings"].as_array().unwrap().is_empty());
}

#[test]
fn invalid_session_exits_3() {
    let tmp = TempDir::new().unwrap();
    let diag = tmp.path().join(".ralph/diagnostics");
    fs::create_dir_all(&diag).unwrap();
    let output = ralph_bin()
        .arg("diagnose")
        .arg("--session")
        .arg("not-a-timestamp")
        .arg("--diagnostics-root")
        .arg(&diag)
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a valid diagnostics session"),
        "stderr should explain invalid session, got: {stderr}"
    );
}
