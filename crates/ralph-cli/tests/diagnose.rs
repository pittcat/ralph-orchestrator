//! Integration tests for `ralph diagnose` (U7).
//!
//! These tests spawn the compiled `ralph` binary against a
//! hand-built `.ralph/diagnostics/<session>/` tree. They exercise
//! the CLI surface (exit codes, stdout / stderr discipline,
//! `--format json`, `--output`) end-to-end so the contract from
//! the U7 plan is enforced at the binary boundary, not just at the
//! reporter API.

mod common;

use std::fs;
use std::path::Path;

use tempfile::TempDir;

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
    let output = common::ralph_bin()
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

    let output = common::ralph_bin()
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

    let output = common::ralph_bin()
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
    assert!(
        value["session_path"]
            .as_str()
            .unwrap()
            .ends_with("2026-06-05T10-20-30")
    );
}

#[test]
fn json_format_does_not_emit_markdown_headings() {
    let tmp = TempDir::new().unwrap();
    let session = fresh_session(&tmp, "2026-06-05T10-20-30");
    write_recovery_entry(&session);
    write_summary(&session);

    let output = common::ralph_bin()
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

    let output = common::ralph_bin()
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

    let output = common::ralph_bin()
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
    let output = common::ralph_bin()
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

    let output = common::ralph_bin()
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
    let output = common::ralph_bin()
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

// -------------------------------------------------------------------------
// U4 (2026-06-17-004): `ralph diagnose` sees workspace-level
// `recovery.jsonl` (cli_emit rejects) when the session journal is
// empty. Without the fallback, the 26 cli_emit rejects from the
// noble-peacock incident would be invisible to operators running
// `ralph diagnose` on a stale or fresh session.

/// T4.3 (Integration): `ralph diagnose --format json` reports a
/// non-zero `recovery_count` when the session-level `recovery.jsonl`
/// is empty but the workspace `.ralph/recovery.jsonl` carries 3
/// cli_emit envelopes.
#[test]
fn u4_diagnose_falls_back_to_workspace_recovery_journal() {
    let tmp = TempDir::new().unwrap();
    let session = fresh_session(&tmp, "2026-06-05T10-20-30");
    // Intentionally do NOT write session recovery.jsonl.
    write_summary(&session);
    write_orchestration(&session);

    // 3 cli_emit entries in workspace-level journal.
    let workspace_journal = tmp.path().join(".ralph").join("recovery.jsonl");
    fs::create_dir_all(workspace_journal.parent().unwrap()).unwrap();
    let cli_emit_entry = "{\"schema_version\":1,\"envelope\":{\"schema_version\":1,\"diagnosis_id\":\"cli-1\",\"iteration\":1,\"source\":\"cli_emit\",\"severity\":\"error\",\"reason_code\":\"policy_denied\",\"message\":\"reject\",\"source_hat\":\"ralph\",\"target_hat\":\"executor\",\"topic\":\"work.done\",\"retry_key\":\"cli_emit:executor:work_done:policy_denied:*\",\"retry_attempt\":0,\"safe_target\":false,\"outcome\":\"failed\",\"timestamp\":\"2026-06-05T10:20:30Z\"},\"iteration\":1,\"timestamp\":\"2026-06-05T10:20:30Z\"}\n";
    fs::write(&workspace_journal, cli_emit_entry.repeat(3)).unwrap();

    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "diagnose should succeed (stderr: {})",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["schema_version"], "1");
    let findings = value["top_findings"]
        .as_array()
        .expect("top_findings array");
    assert_eq!(
        findings.len(),
        1,
        "U4: workspace fallback should surface 1 grouped finding (same retry_key), got {findings:?}"
    );
    assert_eq!(
        findings[0]["occurrences"], 3,
        "U4: 3 cli_emit envelopes must aggregate to occurrences=3"
    );
    assert_eq!(
        findings[0]["source"], "cli_emit",
        "U4: source must reflect workspace journal provenance"
    );
}

/// T4.2 (Edge, CLI side): no workspace recovery, no session recovery
/// → top_findings is empty, exit code 0.
#[test]
fn u4_diagnose_no_journal_no_findings_no_panic() {
    let tmp = TempDir::new().unwrap();
    let session = fresh_session(&tmp, "2026-06-05T10-20-30");
    write_summary(&session);
    write_orchestration(&session);

    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "missing recovery journals must not fail (stderr: {})",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let findings = value["top_findings"]
        .as_array()
        .expect("top_findings array");
    assert!(
        findings.is_empty(),
        "U4: no journals → top_findings must be empty, got {findings:?}"
    );
}

/// T4.4 (Edge, CLI side): session has its own recovery.jsonl with 1
/// entry → workspace fallback must NOT double-count. Session entries
/// take precedence (per KTD-6: dual-path indexing, not merging).
#[test]
fn u4_diagnose_session_takes_precedence_over_workspace() {
    let tmp = TempDir::new().unwrap();
    let session = fresh_session(&tmp, "2026-06-05T10-20-30");
    write_recovery_entry(&session);
    write_summary(&session);
    write_orchestration(&session);

    // Also populate workspace journal — must be ignored when session
    // journal is non-empty.
    let workspace_journal = tmp.path().join(".ralph").join("recovery.jsonl");
    fs::create_dir_all(workspace_journal.parent().unwrap()).unwrap();
    let cli_emit_entry = "{\"schema_version\":1,\"envelope\":{\"schema_version\":1,\"diagnosis_id\":\"cli-1\",\"iteration\":1,\"source\":\"cli_emit\",\"severity\":\"error\",\"reason_code\":\"policy_denied\",\"message\":\"reject\",\"source_hat\":\"ralph\",\"target_hat\":\"executor\",\"topic\":\"work.done\",\"retry_key\":\"cli_emit:executor:work_done:policy_denied:*\",\"retry_attempt\":0,\"safe_target\":false,\"outcome\":\"failed\",\"timestamp\":\"2026-06-05T10:20:30Z\"},\"iteration\":1,\"timestamp\":\"2026-06-05T10:20:30Z\"}\n";
    fs::write(&workspace_journal, cli_emit_entry).unwrap();

    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let findings = value["top_findings"]
        .as_array()
        .expect("top_findings array");
    assert_eq!(
        findings.len(),
        1,
        "U4: session journal takes precedence; workspace entry ignored, got {findings:?}"
    );
    assert_eq!(
        findings[0]["source"], "missing_event_gate",
        "U4: source must come from session journal"
    );
}

// -------------------------------------------------------------------------
// U8 (2026-06-21-002 plan): `--from-ledger` / `--legacy` flag tests.
// These exercise the binary-level CLI surface for the U8 view switch.

/// Helper: write a single ledger-shaped rejection record into the
/// workspace-level `.ralph/recovery.jsonl`.  The record uses the
/// U7a schema (`{ts, hat, topic, reason_code, retry_count,
/// terminal_reason}`) so the U8 reporter can parse it.
fn write_workspace_rejection(workspace: &Path, line: &str) {
    let path = workspace.join(".ralph").join("recovery.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("{line}\n")).unwrap();
}

/// T8.1 (Happy path, CLI side): `--from-ledger` against a workspace
/// that carries a U7a rejection record emits the U8 schema
/// (`u8-1`) and renders a root-cause row.
#[test]
fn u8_diagnose_from_ledger_renders_ledger_schema() {
    let tmp = TempDir::new().unwrap();
    let ralph_dir = tmp.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).unwrap();
    let record = r#"{"ts":"2026-06-22T01:00:00Z","hat":"executor","topic":"work.done","reason_code":"execution_contract:missing_field","retry_count":1,"terminal_reason":null}"#;
    write_workspace_rejection(tmp.path(), record);

    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--from-ledger")
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "--from-ledger should succeed (stderr: {})",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    // U8 schema (distinct from the session-level "1").
    assert_eq!(value["schema_version"], "u8-1");
    let causes = value["root_causes"].as_array().expect("root_causes array");
    assert_eq!(causes.len(), 1);
    assert_eq!(causes[0]["reason_code"], "execution_contract:missing_field");
    assert_eq!(causes[0]["frequency"], 1);
    assert_eq!(causes[0]["source"], "execution_contract");
    assert!(value["used_ledger"].as_bool().unwrap_or(false));
    assert_eq!(value["rejection_summary"]["record_count"], 1);
}

/// T8.2 (Edge, CLI side): no rejection log + no commit log → NoSession
/// exit code (2).  The CLI must not silently fall back to the legacy
/// view when the operator explicitly asked for the ledger view.
#[test]
fn u8_diagnose_from_ledger_with_empty_workspace_returns_no_session() {
    let tmp = TempDir::new().unwrap();
    let ralph_dir = tmp.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).unwrap();
    // Empty `.ralph/` directory; no rejection.jsonl, no ledger.jsonl.
    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--from-ledger")
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "--from-ledger with no data should exit 2 (NoSession), stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("rejection log") || stderr.contains("commit log"),
        "stderr should hint at missing log files, got: {stderr}"
    );
}

/// T8.3 (Default fallback, CLI side): the default mode prefers the
/// ledger view when the workspace has a rejection log.  The CLI
/// must emit the U8 schema (`u8-1`) and surface the
/// `root_causes` array, NOT the legacy `top_findings`.
#[test]
fn u8_diagnose_default_prefers_ledger_view() {
    let tmp = TempDir::new().unwrap();
    let ralph_dir = tmp.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).unwrap();
    let record = r#"{"ts":"2026-06-22T02:00:00Z","hat":"reviewer","topic":"review.passed","reason_code":"policy:missing_field","retry_count":2,"terminal_reason":null}"#;
    write_workspace_rejection(tmp.path(), record);

    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "default mode should render the ledger view when present (stderr: {})",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    // U8 default: ledger view wins when the workspace has a
    // rejection log.
    assert_eq!(value["schema_version"], "u8-1");
    let causes = value["root_causes"].as_array().expect("root_causes array");
    assert_eq!(causes.len(), 1);
    assert_eq!(causes[0]["reason_code"], "policy:missing_field");
    // `source` comes from the validation_stage_to_source mapping:
    // `policy` → `event_policy`.
    assert_eq!(causes[0]["source"], "event_policy");
    assert!(value["used_ledger"].as_bool().unwrap_or(false));
}

/// T8.4 (CLI side): `--legacy` forces the session view even when
/// the workspace has a rejection log.  The result is the legacy
/// schema (v1) with `top_findings` rather than `root_causes`.
#[test]
fn u8_diagnose_legacy_flag_uses_session_view() {
    let tmp = TempDir::new().unwrap();
    let ralph_dir = tmp.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).unwrap();
    // Workspace rejection log present, BUT also populate a
    // session-level recovery.jsonl so the legacy view has
    // something to render.
    let record = r#"{"ts":"2026-06-22T03:00:00Z","hat":"executor","topic":"work.done","reason_code":"execution_contract:missing_field","retry_count":1,"terminal_reason":null}"#;
    write_workspace_rejection(tmp.path(), record);
    let session = fresh_session(&tmp, "2026-06-05T10-20-30");
    write_recovery_entry(&session);

    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--legacy")
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "--legacy should succeed (stderr: {})",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    // Legacy view: schema v1 + top_findings populated from the
    // session journal (the workspace rejection log is ignored
    // because of --legacy).
    assert_eq!(value["schema_version"], "1");
    let findings = value["top_findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "legacy session view: 1 finding");
    assert_eq!(findings[0]["source"], "missing_event_gate");
}
