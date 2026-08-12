//! Plan 2026-08-12-001 Unit 4: bundle-first reporter tests.
//!
//! Asserts that:
//! - `load_session` populates the additive bundle/trace/feedback
//!   status fields from the on-disk sidecars.
//! - Legacy sessions (no manifest) still produce a report with
//!   `status=legacy` and empty arrays, and old fields are
//!   unchanged.
//! - Malformed sidecars increment `malformed_lines` and never
//!   panic.
//! - Suggestions are non-executing: no command strings, no
//!   side-effects.

use std::fs;
use std::path::Path;

use ralph_core::diagnosis::{
    load_session, read_feedback_lifecycle_report, read_input_bundle_report,
    read_runtime_trace_report, render_json, BundleStatus, FeedbackLifecycleRow, FeedbackPhase,
    Report,
};
use ralph_core::diagnostics::{
    write_manifest as write_input_bundle, ArtifactIntegrity, ArtifactStatus, CodeBaseline,
    DiagnosisInputBundle, RuntimeTraceEntry, RuntimeTracePhase,
};
use tempfile::TempDir;

fn write_pending_bundle(session_dir: &Path) {
    let bundle = DiagnosisInputBundle::new_pending(session_dir);
    let _ = write_input_bundle(session_dir, &bundle).expect("write_manifest");
}

fn append_runtime_trace(session_dir: &Path, hat: &str, phase: RuntimeTracePhase, sequence: u64) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(session_dir.join("runtime-trace.jsonl"))
        .expect("open runtime-trace");
    let entry = RuntimeTraceEntry::new(0, sequence, phase).with_hat(hat);
    let json = serde_json::to_string(&entry).expect("serialize");
    writeln!(file, "{}", json).expect("append");
}

fn append_feedback(session_dir: &Path, id: &str, retry: &str, phase: FeedbackPhase) {
    use ralph_core::diagnostics::FeedbackEntry;
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(session_dir.join("feedback.jsonl"))
        .expect("open feedback");
    let entry = FeedbackEntry::new(0, id, retry, phase);
    let json = serde_json::to_string(&entry).expect("serialize");
    writeln!(file, "{}", json).expect("append");
}

#[test]
fn legacy_session_loads_with_legacy_status() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    // No sidecars at all.
    let data = load_session(&session);
    assert_eq!(data.diagnosis_input.status, BundleStatus::Legacy);
    assert_eq!(data.runtime_trace.status, BundleStatus::Missing);
    assert_eq!(data.feedback_lifecycle.status, BundleStatus::Missing);
}

#[test]
fn present_bundle_projects_to_report() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    let bundle = DiagnosisInputBundle::new_pending(&session).with_completed_identity(
        Some("loop-1".to_string()),
        Some("builtin:ce-executor-pipeline".to_string()),
        Some("ralph.yml".to_string()),
        Some("plan.md".to_string()),
        Some("e0781bf6".to_string()),
        Some("single-chain".to_string()),
        CodeBaseline {
            head_sha: Some("e0781bf6".to_string()),
            worktree: true,
            worktree_path: None,
        },
    );
    let bundle = bundle.with_finalized(
        vec![ArtifactIntegrity {
            path: "trace.jsonl".to_string(),
            status: ArtifactStatus::Present,
            sha256: Some("abcd".to_string()),
            size_bytes: Some(1024),
            last_modified: None,
        }],
        vec!["single-chain".to_string()],
    );
    let _ = write_input_bundle(&session, &bundle).expect("write_manifest");

    let report = read_input_bundle_report(&session);
    assert_eq!(report.status, BundleStatus::Finalized);
    assert_eq!(report.preset_label.as_deref(), Some("builtin:ce-executor-pipeline"));
    assert_eq!(report.loop_id.as_deref(), Some("loop-1"));
    assert_eq!(report.worktree, Some(true));
    assert_eq!(report.artifacts.len(), 1);
    assert_eq!(report.execution_capabilities, vec!["single-chain".to_string()]);
}

#[test]
fn runtime_trace_reader_counts_records() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    append_runtime_trace(&session, "executor", RuntimeTracePhase::Activation, 1);
    append_runtime_trace(&session, "executor", RuntimeTracePhase::Batch, 2);
    append_runtime_trace(&session, "executor", RuntimeTracePhase::Accepted, 3);
    use std::io::Write as _;
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(session.join("runtime-trace.jsonl"))
        .expect("open for append");
    f.write_all(b"\nnot json\n").expect("append bad line");
    let report = read_runtime_trace_report(&session);
    assert_eq!(report.status, BundleStatus::Present);
    assert_eq!(report.record_count, 3);
    assert_eq!(report.malformed_lines, 1);
    assert_eq!(report.first_sequence, Some(1));
    assert_eq!(report.last_sequence, Some(3));
}

#[test]
fn feedback_reader_groups_rows_by_id() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    append_feedback(&session, "diag-1", "retry-1", FeedbackPhase::Discovered);
    append_feedback(&session, "diag-1", "retry-1", FeedbackPhase::Action);
    append_feedback(&session, "diag-2", "retry-2", FeedbackPhase::Discovered);
    let report = read_feedback_lifecycle_report(&session);
    assert_eq!(report.status, BundleStatus::Present);
    assert_eq!(report.rows.len(), 3);
    let diag1: Vec<&FeedbackLifecycleRow> = report
        .rows
        .iter()
        .filter(|r| r.feedback_id == "diag-1")
        .collect();
    assert_eq!(diag1.len(), 2);
}

#[test]
fn report_from_session_includes_bundle_fields() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    write_pending_bundle(&session);
    let bundle = DiagnosisInputBundle::new_pending(&session).with_finalized(
        vec![],
        vec!["single-chain".to_string()],
    );
    let _ = write_input_bundle(&session, &bundle).expect("write_manifest");
    let data = load_session(&session);
    let report = Report::from_session(&data);
    assert_eq!(report.diagnosis_input.status, BundleStatus::Finalized);
    assert!(!report.repair_suggestions.is_empty(), "non-empty suggestions");
    let json = render_json(&report);
    assert!(json.get("diagnosis_input").is_some());
    assert!(json.get("runtime_trace").is_some());
    assert!(json.get("feedback_lifecycle").is_some());
    assert!(json.get("repair_suggestions").is_some());
    assert!(json.get("evidence_gaps").is_some());
    let suggestions = json["repair_suggestions"].as_array().expect("array");
    for s in suggestions {
        let tier = s["tier"].as_str().unwrap_or("");
        assert!(matches!(tier, "short" | "mid" | "long"), "tier must be one of short/mid/long, got: {tier}");
        let text = s["text"].as_str().unwrap_or("");
        assert!(
            !text.contains("rm -rf") && !text.contains("cargo run") && !text.contains("ralph "),
            "suggestion must not contain executable commands; got: {text}"
        );
    }
}

#[test]
fn malformed_manifest_falls_back_to_legacy() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    fs::write(session.join("diagnosis-input.json"), b"{not valid json}").expect("write bad");
    let report = read_input_bundle_report(&session);
    assert_eq!(report.status, BundleStatus::Legacy);
}
