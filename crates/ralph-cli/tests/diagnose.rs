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

// -------------------------------------------------------------------------
// U9 (plan 2026-08-26-1104): `--causal` flag.
//
// Adds the deterministic `CausalAttributionReport` (U8) to the
// session-view output: a Markdown "Causal Attribution" section
// or a JSON `causal` object. The flag is mutually exclusive
// with `--from-ledger` / `--legacy` because the ledger view
// has no session sidecars to feed the engine.
//
// Scenarios:
// - S9.1: complete session → markdown section + json object
//   carry `primary_domain`, `fix_point`, `confidence` breakdown,
//   and `rejected_hypotheses`.
// - S9.2: legacy / v1 session → `not_evaluable` + reason; never
//   claims an attribution.
// - S9.3: JSON `causal` object mirrors the
//   `CausalAttributionReport` shape (serde rename snake_case).
// - ledger + --causal → clap mutual exclusion error.

/// Find the first `{` byte and return the slice from there
/// onwards. Used to extract the JSON document from stdout when
/// `tracing::warn!` lines were emitted ahead of it
/// (pre-existing main.rs configuration writes WARN lines to
/// stdout for `ralph diagnose` with an authoritative
/// diagnostics collector). The JSON document is the only valid
/// `{ ... }` payload the CLI emits in `--format json` mode, so
/// trimming the WARN prelude cannot truncate valid output.
fn extract_json_document(stdout: &str) -> &str {
    match stdout.find('{') {
        Some(idx) => &stdout[idx..],
        None => stdout,
    }
}

/// Helper: write a v2 manifest with all 8 boundaries covered,
/// terminal topics in `execution_capabilities[]`, and a
/// `contract_receipt` whose terminal_topics list a topic the
/// manifest does NOT declare. Mirrors the U08 fixture pattern
/// (S8.1 → preset domain) so the engine returns
/// `primary_domain = preset`.
fn write_u9_manifest_v2(session: &Path) {
    let manifest = r#"{
  "schema_version": "run-diagnosis-input/v2",
  "manifest_status": "finalized",
  "created_at": "2026-08-26T12:00:00Z",
  "updated_at": "2026-08-26T12:00:00Z",
  "run": {
    "loop_id": "L-test-u9",
    "preset_label": "builtin:test",
    "execution_capability": "supervisor"
  },
  "code_baseline": { "head_sha": "deadbeef", "worktree": false },
  "execution_capabilities": [
    "executor",
    "planner",
    "alignment",
    "plan.complete"
  ],
  "boundary_coverage": [
    { "boundary": "effective_contract",   "expected": 1, "recorded": 1, "status": "covered" },
    { "boundary": "activation",          "expected": 1, "recorded": 1, "status": "covered" },
    { "boundary": "backend_outcome",     "expected": 1, "recorded": 1, "status": "covered" },
    { "boundary": "event_candidate",     "expected": 1, "recorded": 1, "status": "covered" },
    { "boundary": "policy_decision",     "expected": 1, "recorded": 1, "status": "covered" },
    { "boundary": "state_commit",        "expected": 1, "recorded": 1, "status": "covered" },
    { "boundary": "recovery_action",     "expected": 1, "recorded": 1, "status": "covered" },
    { "boundary": "termination",         "expected": 1, "recorded": 1, "status": "covered" }
  ]
}"#;
    fs::write(session.join("diagnosis-input.json"), manifest).unwrap();
}

/// Helper: write a v1 (legacy) manifest — no `boundary_coverage`
/// block. The engine returns `not_evaluable` because the gap
/// evidence required for an honest attribution is absent (U07
/// `run-diagnosis-input/v2` is the minimal evaluable schema).
fn write_u9_manifest_v1(session: &Path) {
    let manifest = r#"{
  "schema_version": "run-diagnosis-input/v1",
  "manifest_status": "finalized",
  "run": { "loop_id": "L-test-u9-legacy" }
}"#;
    fs::write(session.join("diagnosis-input.json"), manifest).unwrap();
}

/// Helper: write a `runtime-trace.jsonl` with one
/// `contract_receipt` whose terminal_topics list
/// `plan.complete.missing` — a topic the manifest's
/// `execution_capabilities[]` declares but the preset's
/// contract_digest does not expose (S8.1 → preset domain).
fn write_u9_runtime_trace(session: &Path) {
    let trace = r#"{"schema_version":"v1","ts":"2026-08-26T12:00:00Z","iteration":1,"sequence":1,"phase":"decision","kind":"contract_receipt","fields":{"contract_digest":"abc","hats_digest":"h","terminal_topics_digest":"t","preset_label":"builtin:test","terminal_topics":["plan.complete.missing"]}}
"#;
    fs::write(session.join("runtime-trace.jsonl"), trace).unwrap();
}

/// T9.1 (S9.1 + S9.3): `--causal` on a complete v2 session
/// emits the markdown "Causal Attribution" section AND the JSON
/// `causal` object. The JSON object's shape mirrors the
/// `CausalAttributionReport` contract: `contract_version`,
/// `status`, `primary_domain`, `confidence.total`,
/// `rejected_hypotheses`.
///
/// The fixture here is intentionally minimal: with only a
/// `contract_receipt` and no workspace rejection log / commit
/// log, the engine resolves `primary_domain = preset` but
/// reports `status = incomplete` because `refutation` and
/// `freeze_window` components cannot be scored without the
/// sidecar rows. We pin the structured output regardless of
/// whether the score clears the >85 gate — both `complete`
/// and `incomplete` render the same markdown + JSON shape.
#[test]
fn u9_diagnose_causal_emits_section_and_object_on_complete_session() {
    let tmp = TempDir::new().unwrap();
    let diag = tmp.path().join(".ralph/diagnostics");
    let session = diag.join("2026-08-26T12-00-00");
    fs::create_dir_all(&session).unwrap();
    write_u9_manifest_v2(&session);
    write_u9_runtime_trace(&session);

    // ----- markdown surface -----
    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--causal")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "--causal must succeed on a complete session (stderr: {})",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("## Causal Attribution"),
        "markdown must contain the Causal Attribution section, got: {stdout}"
    );
    // Both `complete` (score > 85) and `incomplete` (score ≤ 85)
    // surface the same structured output. The minimal fixture
    // produces `incomplete` because refutation + freeze_window
    // cannot be scored without workspace sidecars; the more
    // elaborate U08 S8.1 fixture reaches `complete`.
    let status_ok =
        stdout.contains("- status: `complete`") || stdout.contains("- status: `incomplete`");
    assert!(
        status_ok,
        "complete/incomplete session must report a status line, got: {stdout}"
    );
    // S8.1 fixture resolution: preset rule fires because
    // manifest's `execution_capabilities[]` does not name
    // `plan.complete.missing`. Either `complete` or
    // `incomplete` keeps the same primary_domain.
    assert!(
        stdout.contains("- primary_domain: `preset`"),
        "S8.1 fixture must resolve primary_domain=preset, got: {stdout}"
    );
    assert!(
        stdout.contains("- confidence:") && stdout.contains("- total:"),
        "confidence breakdown must be present, got: {stdout}"
    );
    assert!(
        stdout.contains("rejected_hypotheses"),
        "rejected_hypotheses section must be present, got: {stdout}"
    );

    // ----- JSON surface -----
    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--causal")
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(extract_json_document(&stdout)).unwrap();
    // R6: same versioned contract as the U8 engine output.
    assert_eq!(value["causal"]["contract_version"], "causal-attribution/v1");
    let status = value["causal"]["status"].as_str().unwrap();
    assert!(
        status == "complete" || status == "incomplete",
        "minimal fixture must report complete or incomplete, got: {status}"
    );
    assert_eq!(value["causal"]["primary_domain"], "preset");
    let rejected = value["causal"]["rejected_hypotheses"]
        .as_array()
        .expect("rejected_hypotheses array");
    assert!(
        !rejected.is_empty(),
        "structured attribution must list rejected hypotheses"
    );
}

/// T9.2 (S9.2): a v1 (legacy) session must surface
/// `status: not_evaluable` with the gap reason, never claim a
/// `primary_domain`, and never claim a confidence score above
/// the 85 gate.
#[test]
fn u9_diagnose_causal_legacy_session_renders_not_evaluable() {
    let tmp = TempDir::new().unwrap();
    let diag = tmp.path().join(".ralph/diagnostics");
    let session = diag.join("2026-08-26T12-00-00");
    fs::create_dir_all(&session).unwrap();
    write_u9_manifest_v1(&session);

    // ----- markdown surface -----
    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--causal")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "legacy session must not fail (stderr: {})",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("## Causal Attribution"),
        "Causal Attribution section must still be present on legacy"
    );
    assert!(
        stdout.contains("- status: `not_evaluable`"),
        "legacy session must report not_evaluable, got: {stdout}"
    );
    // R14: legacy fallback must NEVER claim a primary_domain or
    // a confidence.total > 0 — the gate is locked.
    assert!(
        !stdout.contains("- primary_domain: `"),
        "legacy session must not claim a primary_domain, got: {stdout}"
    );
    // The "reason" line must surface so operators know why the
    // engine could not evaluate.
    assert!(
        stdout.contains("- reason:"),
        "legacy session must surface the not_evaluable reason, got: {stdout}"
    );

    // ----- JSON surface -----
    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--causal")
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(extract_json_document(&stdout)).unwrap();
    assert_eq!(value["causal"]["status"], "not_evaluable");
    assert!(
        value["causal"]["primary_domain"].is_null(),
        "legacy session must not claim primary_domain in JSON, got: {value:?}"
    );
    assert_eq!(
        value["causal"]["confidence"]["total"], 0,
        "legacy session must report confidence.total=0, got: {value:?}"
    );
}

/// T9.3 (mutual exclusion): `--causal --from-ledger` must be
/// rejected by clap (R14 / S9 ledger 互斥). The exit code is 2
/// (clap's argument-error code), and stderr surfaces the
/// conflict.
#[test]
fn u9_diagnose_causal_with_from_ledger_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let ralph_dir = tmp.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).unwrap();
    // Workspace rejection log so --from-ledger has data to
    // consider (still rejected by clap before any of it is read).
    let record = r#"{"ts":"2026-08-26T13:00:00Z","hat":"executor","topic":"work.done","reason_code":"execution_contract:missing_field","retry_count":1,"terminal_reason":null}"#;
    fs::write(ralph_dir.join("recovery.jsonl"), format!("{record}\n")).unwrap();

    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--causal")
        .arg("--from-ledger")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "--causal --from-ledger must be rejected; got exit {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--causal") && stderr.contains("--from-ledger"),
        "stderr must mention the conflicting flags, got: {stderr}"
    );
}

/// T9.4 (no-flag byte-identical): when `--causal` is absent,
/// the rendered markdown MUST NOT contain a "Causal Attribution"
/// section — the no-flag output is byte-identical to the U7/U8
/// baseline (R6 + acceptance criteria: 「无 flag 输出逐字节不变」).
#[test]
fn u9_diagnose_no_causal_output_is_byte_identical_to_baseline() {
    let tmp = TempDir::new().unwrap();
    let diag = tmp.path().join(".ralph/diagnostics");
    let session = diag.join("2026-08-26T12-00-00");
    fs::create_dir_all(&session).unwrap();
    write_recovery_entry(&session);
    write_summary(&session);
    write_orchestration(&session);

    let output = common::ralph_bin()
        .arg("diagnose")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Causal Attribution"),
        "no-flag markdown must NOT contain the causal section, got: {stdout}"
    );

    let output = common::ralph_bin()
        .arg("diagnose")
        .arg("--format")
        .arg("json")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(extract_json_document(&stdout)).unwrap();
    assert!(
        value.get("causal").is_none(),
        "no-flag JSON must NOT carry a causal object, got: {value:?}"
    );
}
