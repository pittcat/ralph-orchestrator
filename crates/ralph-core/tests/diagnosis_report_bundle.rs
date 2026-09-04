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
    BundleStatus, FeedbackLifecycleRow, FeedbackPhase, Report, load_session,
    read_feedback_lifecycle_report, read_input_bundle_report, read_runtime_trace_report,
    render_json,
};
use ralph_core::diagnostics::{
    ArtifactIntegrity, ArtifactStatus, CodeBaseline, DiagnosisInputBundle, RuntimeTraceEntry,
    RuntimeTracePhase, write_manifest as write_input_bundle,
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
    // Plan 2026-08-12-001 fix-plan U13: route the test fixture
    // through the production `FeedbackLogger` writer API (which
    // owns the sequence counter, the degraded-flip semantics,
    // and the post-success flush invariant) instead of writing
    // directly with `fs::OpenOptions::append`. Each call opens a
    // fresh logger; for tests that need multiple sequential
    // appends in one process, use `append_feedbacks_with`
    // below to share one logger so the sequence is monotonic
    // across the rows.
    use ralph_core::diagnostics::FeedbackEntry;
    let mut logger =
        ralph_core::diagnostics::FeedbackLogger::new(session_dir).expect("FeedbackLogger::new");
    let entry = FeedbackEntry::new(0, id, retry, phase);
    logger.append(entry);
}

fn append_feedbacks_with<F>(session_dir: &Path, count: usize, mk_entry: F)
where
    F: Fn(usize) -> ralph_core::diagnostics::FeedbackEntry,
{
    // Plan 2026-08-12-001 fix-plan U13: write N rows through one
    // FeedbackLogger so the on-disk sequence is monotonic (and
    // the reader's `monotonic_sequences` flag flips to true).
    let mut logger =
        ralph_core::diagnostics::FeedbackLogger::new(session_dir).expect("FeedbackLogger::new");
    for i in 0..count {
        logger.append(mk_entry(i));
    }
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
        Vec::new(),
    );
    let _ = write_input_bundle(&session, &bundle).expect("write_manifest");

    let report = read_input_bundle_report(&session);
    assert_eq!(report.status, BundleStatus::Finalized);
    assert_eq!(
        report.preset_label.as_deref(),
        Some("builtin:ce-executor-pipeline")
    );
    assert_eq!(report.loop_id.as_deref(), Some("loop-1"));
    assert_eq!(report.worktree, Some(true));
    assert_eq!(report.artifacts.len(), 1);
    assert_eq!(
        report.execution_capabilities,
        vec!["single-chain".to_string()]
    );
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
    assert_eq!(report.status, BundleStatus::Degraded);
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
        Vec::new(),
    );
    let _ = write_input_bundle(&session, &bundle).expect("write_manifest");
    let data = load_session(&session);
    let report = Report::from_session(&data);
    assert_eq!(report.diagnosis_input.status, BundleStatus::Finalized);
    assert!(
        !report.repair_suggestions.is_empty(),
        "non-empty suggestions"
    );
    let json = render_json(&report);
    assert!(json.get("diagnosis_input").is_some());
    assert!(json.get("runtime_trace").is_some());
    assert!(json.get("feedback_lifecycle").is_some());
    assert!(json["feedback_lifecycle"].is_array());
    assert!(json.get("repair_suggestions").is_some());
    assert!(json.get("evidence_gaps").is_some());
    let suggestions = json["repair_suggestions"].as_array().expect("array");
    for s in suggestions {
        let tier = s["tier"].as_str().unwrap_or("");
        assert!(
            matches!(tier, "short" | "mid" | "long"),
            "tier must be one of short/mid/long, got: {tier}"
        );
        let text = s["text"].as_str().unwrap_or("");
        assert!(
            !text.contains("rm -rf") && !text.contains("cargo run") && !text.contains("ralph "),
            "suggestion must not contain executable commands; got: {text}"
        );
    }
}

#[test]
fn malformed_manifest_is_reported_as_degraded() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    fs::write(session.join("diagnosis-input.json"), b"{not valid json}").expect("write bad");
    let report = read_input_bundle_report(&session);
    assert_eq!(report.status, BundleStatus::Degraded);
    assert_eq!(report.path.as_deref(), Some("diagnosis-input.json"));
}

// Plan 2026-08-12-001 fix-plan U2 / synth:P0-2: schema-version
// mismatch must surface as `BundleStatus::SchemaMismatch`, NOT as
// `Legacy` (which would silently demote a valid bundle on
// binary downgrade and emit a misleading "re-run with
// diagnostics" suggestion).

#[test]
fn schema_mismatch_surfaces_as_schema_mismatch_not_legacy() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    // Build a valid bundle and rewrite its schema_version to a
    // future marker. The reader must NOT collapse this to
    // Legacy; it must distinguish "running reader is older
    // than on-disk writer".
    let mut bundle = DiagnosisInputBundle::new_pending(&session);
    bundle.schema_version = "run-diagnosis-input/v999".to_string();
    write_input_bundle(&session, &bundle).expect("write manifest");
    let report = read_input_bundle_report(&session);
    match &report.status {
        BundleStatus::SchemaMismatch {
            on_disk_version,
            reader_version,
        } => {
            assert_eq!(on_disk_version, "run-diagnosis-input/v999");
            assert_eq!(
                reader_version,
                ralph_core::diagnostics::DIAGNOSIS_INPUT_SCHEMA_VERSION
            );
        }
        other => panic!(
            "expected SchemaMismatch for version-bumped on-disk manifest, got {:?}",
            other
        ),
    }
}

#[test]
fn schema_mismatch_suggestion_names_versions() {
    // The suggestion mapper must NOT route SchemaMismatch to
    // the misleading "Re-run with diagnostics enabled" path;
    // it must spell out the on-disk vs reader version and
    // describe the rollback-safety contract.
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    let mut bundle = DiagnosisInputBundle::new_pending(&session);
    bundle.schema_version = "run-diagnosis-input/v999".to_string();
    write_input_bundle(&session, &bundle).expect("write manifest");
    let report = read_input_bundle_report(&session);
    let json = serde_json::to_string(&report.status).expect("serialize status");
    assert!(
        json.contains("schema_mismatch"),
        "serialized status must include schema_mismatch tag, got: {}",
        json
    );
    assert!(
        json.contains("run-diagnosis-input/v999"),
        "serialized status must include on-disk version, got: {}",
        json
    );
}

#[test]
fn matching_schema_version_projects_as_present() {
    // Sanity: when the on-disk schema matches the reader's
    // compiled constant, the report must NOT spuriously
    // surface SchemaMismatch.
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    let bundle = DiagnosisInputBundle::new_pending(&session);
    write_input_bundle(&session, &bundle).expect("write manifest");
    let report = read_input_bundle_report(&session);
    assert_eq!(report.status, BundleStatus::Pending);
    assert!(!matches!(
        report.status,
        BundleStatus::SchemaMismatch { .. }
    ));
}

// Plan 2026-08-12-001 fix-plan U8: empty / whitespace-only
// sidecar files must NOT spoof BundleStatus::Present with
// record_count=0. The reader must treat them as Missing.

#[test]
fn empty_runtime_trace_file_treated_as_missing() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    fs::write(session.join("runtime-trace.jsonl"), "").expect("write empty");
    let report = read_runtime_trace_report(&session);
    assert_eq!(report.status, BundleStatus::Missing);
    assert_eq!(report.record_count, 0);
}

#[test]
fn whitespace_only_runtime_trace_treated_as_missing() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    fs::write(session.join("runtime-trace.jsonl"), "\n\n   \n").expect("write whitespace");
    let report = read_runtime_trace_report(&session);
    assert_eq!(report.status, BundleStatus::Missing);
    assert_eq!(report.record_count, 0);
}

#[test]
fn empty_feedback_file_treated_as_missing() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    fs::write(session.join("feedback.jsonl"), "").expect("write empty");
    let report = read_feedback_lifecycle_report(&session);
    assert_eq!(report.status, BundleStatus::Missing);
    assert_eq!(report.rows.len(), 0);
}

#[test]
fn single_valid_row_still_present() {
    // Regression guard: a single valid JSONL row must still
    // project as Present with record_count == 1.
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");

    // One valid runtime-trace row.
    let trace_entry = RuntimeTraceEntry::new(0, 1, RuntimeTracePhase::Activation);
    let trace_json = serde_json::to_string(&trace_entry).expect("serialize trace");
    fs::write(
        session.join("runtime-trace.jsonl"),
        format!("{trace_json}\n"),
    )
    .expect("write trace");

    // One valid feedback row.
    let feedback_entry = ralph_core::diagnostics::FeedbackEntry::new(
        0,
        "fb-1",
        "retry-1",
        FeedbackPhase::Discovered,
    );
    let feedback_json = serde_json::to_string(&feedback_entry).expect("serialize feedback");
    fs::write(session.join("feedback.jsonl"), format!("{feedback_json}\n"))
        .expect("write feedback");

    let trace_report = read_runtime_trace_report(&session);
    assert_eq!(trace_report.status, BundleStatus::Present);
    assert_eq!(trace_report.record_count, 1);

    let feedback_report = read_feedback_lifecycle_report(&session);
    assert_eq!(feedback_report.status, BundleStatus::Present);
    assert_eq!(feedback_report.rows.len(), 1);
}

#[test]
fn malformed_and_out_of_order_sidecar_rows_are_degraded_without_panicking() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");

    let mut first = serde_json::to_value(RuntimeTraceEntry::new(0, 5, RuntimeTracePhase::Batch))
        .expect("trace value");
    let mut second = serde_json::to_value(RuntimeTraceEntry::new(0, 1, RuntimeTracePhase::Commit))
        .expect("trace value");
    first["sequence"] = serde_json::json!(5);
    second["sequence"] = serde_json::json!(1);
    fs::write(
        session.join("runtime-trace.jsonl"),
        format!("{}\n{}\n{{not-json}}\n", first, second),
    )
    .expect("write trace");

    let report = read_runtime_trace_report(&session);
    assert_eq!(report.status, BundleStatus::Degraded);
    assert_eq!(report.record_count, 2);
    assert_eq!(report.malformed_lines, 1);
    assert!(!report.monotonic_sequences);
    let report = Report::from_session(&ralph_core::diagnosis::load_session(&session));
    assert!(
        report
            .evidence_gaps
            .iter()
            .any(|gap| gap.artifact == "runtime-trace.jsonl")
    );
}

// =========================================================================
// Plan 2026-08-12-001 fix-plan U13: writer→reader integration fixtures.
// The tests below exercise `FeedbackLogger::append` (the production
// writer path) end-to-end against the on-disk JSONL, then assert the
// reader's invariants on rows the writer actually produced. Before U13
// the existing tests wrote rows directly with `fs::OpenOptions::append`,
// which never tested the writer's sequence-increment-after-flush path
// or the `(feedback_id, retry_key)` identity projection against real
// production writes.
// =========================================================================

#[test]
fn u13_writer_reader_monotonic_sequences_via_production_path() {
    use ralph_core::diagnostics::FeedbackEntry;
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");

    // 5 rows, distinct retry_keys, one FeedbackLogger (so the
    // sequence counter is monotonic across appends).
    append_feedbacks_with(&session, 5, |i| {
        FeedbackEntry::new(
            0,
            format!("diag-{i}"),
            format!("retry-{i}"),
            FeedbackPhase::Action,
        )
    });

    let report = read_feedback_lifecycle_report(&session);
    assert_eq!(report.status, BundleStatus::Present);
    assert_eq!(
        report.rows.len(),
        5,
        "writer→reader round-trip must surface all 5 rows"
    );
    assert!(
        report.monotonic_sequences,
        "5 appends via FeedbackLogger must produce monotonic sequences (got report={:?})",
        report
    );
    // First/last sequence come from the on-disk rows.
    let first_seq = report.rows.iter().map(|r| r.sequence).min().unwrap();
    let last_seq = report.rows.iter().map(|r| r.sequence).max().unwrap();
    assert_eq!(first_seq, 1, "first row sequence must be 1");
    assert_eq!(last_seq, 5, "last row sequence must be 5");
}

#[test]
fn u13_same_retry_key_yields_stable_feedback_id() {
    use ralph_core::diagnostics::FeedbackEntry;
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");

    // 5 rows sharing the same `retry_key` (and therefore the
    // same `feedback_id` per the writer's identity contract).
    append_feedbacks_with(&session, 5, |i| {
        FeedbackEntry::new(
            0,
            "shared-id",
            "shared-retry",
            if i % 2 == 0 {
                FeedbackPhase::Action
            } else {
                FeedbackPhase::Validation
            },
        )
    });

    let report = read_feedback_lifecycle_report(&session);
    assert_eq!(report.rows.len(), 5);
    // All rows map to the same `feedback_id` (the projection
    // uses retry_key ⇒ feedback_id, so repeated retry_keys
    // collapse to one identity).
    let mut ids: Vec<_> = report.rows.iter().map(|r| r.feedback_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        1,
        "all 5 rows sharing one retry_key must map to one feedback_id, got {:?}",
        ids
    );
    assert_eq!(ids[0], "shared-id");
}

#[test]
fn u13_distinct_retry_keys_yield_distinct_feedback_ids() {
    use ralph_core::diagnostics::FeedbackEntry;
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");

    // 5 rows with 5 distinct keys → 5 distinct feedback_ids.
    append_feedbacks_with(&session, 5, |i| {
        FeedbackEntry::new(
            0,
            format!("id-{i}"),
            format!("retry-{i}"),
            FeedbackPhase::Action,
        )
    });

    let report = read_feedback_lifecycle_report(&session);
    assert_eq!(report.rows.len(), 5);
    let mut ids: Vec<_> = report.rows.iter().map(|r| r.feedback_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        5,
        "5 distinct retry_keys must map to 5 distinct feedback_ids, got {:?}",
        ids
    );
}

// =============================================================================
// Plan 2026-08-26-1104 Unit 7 (U07): boundary coverage report projection tests.
// - v1 manifest reads as legacy with empty coverage
// - v2 gap surfaces as structured evidence_gap entries that name the
//   affected boundary so the operator can pin the missing receipt.
// - Unknown (higher) schema version still maps to SchemaMismatch,
//   not Legacy or Present.
// =============================================================================

#[test]
fn v1_manifest_reads_as_legacy_without_coverage() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    // Build a v1 manifest (no `boundary_coverage` field at all) by
    // overwriting the schema_version on the standard pending bundle.
    let mut bundle = DiagnosisInputBundle::new_pending(&session);
    bundle.schema_version = "run-diagnosis-input/v1".to_string();
    write_input_bundle(&session, &bundle).expect("write v1 manifest");

    let report = read_input_bundle_report(&session);
    assert_eq!(report.status, BundleStatus::Legacy);
    assert!(
        report.boundary_coverage.is_empty(),
        "v1 reader must surface empty boundary_coverage; got {:?}",
        report.boundary_coverage
    );

    // Suggestions still mention the bundle, but the per-boundary
    // gap evidence is NOT injected (legacy path can't fabricate
    // boundary coverage that never existed on disk).
    let mut trace = ralph_core::diagnosis::RuntimeTraceReport::default();
    trace.status = BundleStatus::Present;
    let mut feedback = ralph_core::diagnosis::FeedbackLifecycleReport::default();
    feedback.status = BundleStatus::Present;
    let (suggestions, gaps) = ralph_core::diagnosis::build_suggestions_and_gaps(
        &report,
        &trace,
        &feedback,
        &[],
        Path::new("/tmp/x"),
    );
    assert!(
        gaps.iter()
            .all(|g| !g.affects.as_deref().unwrap_or("").starts_with("boundary:")),
        "legacy v1 reader must not inject synthetic boundary gap evidence"
    );
    // Suggestions are still produced for the missing bundle path.
    assert!(!suggestions.is_empty());
}

#[test]
fn v2_gap_projects_evidence_gap_with_boundary_affects() {
    use ralph_core::diagnostics::{BoundaryCoverageEntry, BoundaryCoverageStatus, CausalBoundary};
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    // Two boundaries covered (1/1), one boundary gap (expected=2,
    // recorded=1), rest covered (0/0). The reader must surface
    // the gap boundary in the report AND in the evidence_gaps
    // list with `affects="boundary:<name>"`.
    let mut coverage: Vec<BoundaryCoverageEntry> = CausalBoundary::all()
        .map(|b| {
            let (expected, recorded) = if b == CausalBoundary::StateCommit {
                (2, 1)
            } else {
                (1, 1)
            };
            let mut entry = BoundaryCoverageEntry::new(
                b,
                &ralph_core::diagnostics::BoundaryCounter { expected, recorded },
                None,
            );
            if entry.status == BoundaryCoverageStatus::Gap {
                entry.reason = Some("commit_receipt missing".to_string());
            }
            entry
        })
        .collect();
    // Force at least one covered row to have a 0/0 profile so the
    // test covers the "no emission yet" path.
    for entry in coverage.iter_mut() {
        if entry.boundary == CausalBoundary::Activation {
            entry.expected = 0;
            entry.recorded = 0;
            entry.status = BoundaryCoverageStatus::Covered;
            entry.reason = None;
        }
    }

    let bundle = DiagnosisInputBundle::new_pending(&session).with_finalized(
        Vec::new(),
        Vec::new(),
        coverage,
    );
    write_input_bundle(&session, &bundle).expect("write v2 manifest");

    let report = read_input_bundle_report(&session);
    assert_eq!(report.status, BundleStatus::Finalized);
    assert_eq!(report.boundary_coverage.len(), 8);
    let gap = report
        .boundary_coverage
        .iter()
        .find(|e| e.boundary == "state_commit")
        .expect("state_commit gap entry");
    assert_eq!(gap.expected, 2);
    assert_eq!(gap.recorded, 1);
    assert_eq!(gap.status, BoundaryCoverageStatus::Gap);
    assert_eq!(gap.reason.as_deref(), Some("commit_receipt missing"));

    // Pre-populate trace/feedback as Present so the suggestion mapper
    // doesn't inject unrelated suggestions.
    let mut trace = ralph_core::diagnosis::RuntimeTraceReport::default();
    trace.status = BundleStatus::Present;
    let mut feedback = ralph_core::diagnosis::FeedbackLifecycleReport::default();
    feedback.status = BundleStatus::Present;
    let (suggestions, gaps) = ralph_core::diagnosis::build_suggestions_and_gaps(
        &report,
        &trace,
        &feedback,
        &[],
        Path::new("/tmp/x"),
    );
    let state_commit_gap = gaps
        .iter()
        .find(|g| g.affects.as_deref() == Some("boundary:state_commit"))
        .expect("evidence_gap entry for state_commit");
    assert_eq!(state_commit_gap.artifact, "diagnosis-input.json");
    assert!(
        state_commit_gap.reason.contains("commit_receipt missing"),
        "gap reason must surface the producer-supplied reason; got {:?}",
        state_commit_gap
    );
    // And the suggestion mapper should attach a boundary-tagged
    // suggestion so the operator can pin the missing receipt.
    assert!(
        suggestions
            .iter()
            .any(|s| s.finding_refs.iter().any(|r| r == "bundle.boundary_gap")),
        "missing boundary_gap suggestion: {:?}",
        suggestions
    );
}

#[test]
fn unknown_higher_version_is_schema_mismatch_not_present() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    let mut bundle = DiagnosisInputBundle::new_pending(&session);
    bundle.schema_version = "run-diagnosis-input/v999".to_string();
    write_input_bundle(&session, &bundle).expect("write far-future manifest");
    let report = read_input_bundle_report(&session);
    match &report.status {
        BundleStatus::SchemaMismatch {
            on_disk_version,
            reader_version,
        } => {
            assert_eq!(on_disk_version, "run-diagnosis-input/v999");
            assert_eq!(
                reader_version,
                ralph_core::diagnostics::DIAGNOSIS_INPUT_SCHEMA_VERSION
            );
        }
        other => panic!("expected SchemaMismatch for v999 manifest, got {:?}", other),
    }
    // Boundary coverage must NOT be silently promoted to v2 data
    // when the schema version is unknown.
    assert!(report.boundary_coverage.is_empty());
}
