//! Tests for the `diagnosis-input.json` manifest writer introduced
//! in plan 2026-08-12-001 Unit 1.
//!
//! These tests run against the real filesystem via `tempfile::TempDir`.
//! The input bundle writes are best-effort by design; fault injection
//! uses real filesystem failures (target path is a directory, etc.),
//! not mocks of serde or atomic-write.

use std::fs;
use std::path::Path;

use ralph_core::diagnostics::{
    ArtifactIntegrity, ArtifactStatus, BoundaryCoverageEntry, BoundaryCoverageStatus,
    CausalBoundary, CodeBaseline, DIAGNOSIS_INPUT_SCHEMA_VERSION, DiagnosisInputBundle,
    DiagnosticsCollector, DiagnosticsOptions, ManifestStatus, RunMetadata,
    write_manifest as write_input_bundle,
};
use tempfile::TempDir;

fn write_pending_bundle(session_dir: &Path) {
    let bundle = DiagnosisInputBundle::new_pending(session_dir);
    let res = write_input_bundle(session_dir, &bundle)
        .expect("write_manifest should succeed for writable session dir");
    assert!(res.is_some(), "manifest should be written to disk");
}

fn manifest_path(session_dir: &Path) -> std::path::PathBuf {
    session_dir.join("diagnosis-input.json")
}

#[test]
fn enabled_run_writes_input_bundle() {
    let tmp = TempDir::new().expect("TempDir");
    let base = tmp.path();
    let opts = DiagnosticsOptions {
        full_diagnostics: false,
        runtime_diagnosis_artifacts: true,
        trace_only: false,
        session_dir: None,
        workspace_root: None,
        // U01b: causal_evidence defaults to `false` so the minimal
        // session here stays byte-equivalent to the pre-U01b
        // `runtime_diagnosis_artifacts=true` shape.
        causal_evidence: false,

        causal_evidence_window_capacity: None,
    };
    let collector = DiagnosticsCollector::with_options(base, &opts).expect("collector constructs");
    let session = collector
        .session_dir()
        .expect("session dir present")
        .to_path_buf();
    // pending manifest was created at collector construction time
    let path = manifest_path(&session);
    assert!(path.is_file(), "diagnosis-input.json should exist");
    let bytes = fs::read(&path).expect("read manifest");
    let bundle: DiagnosisInputBundle =
        serde_json::from_slice(&bytes).expect("manifest is valid JSON");
    assert_eq!(bundle.schema_version, DIAGNOSIS_INPUT_SCHEMA_VERSION);
    assert_eq!(bundle.manifest_status, ManifestStatus::Pending);
    assert!(bundle.run.session_id.is_some(), "session id set");
    assert!(!bundle.run.loop_id.is_some(), "loop id pending");
    assert!(!bundle.write_blocked, "not blocked");

    // complete identity and finalize
    let mut next = bundle.clone();
    next = next.with_completed_identity(
        Some("loop-x".to_string()),
        Some("builtin:ce-executor-pipeline".to_string()),
        Some("ralph.yml".to_string()),
        Some("docs/plans/2026-08-12-001.md".to_string()),
        Some("e0781bf6".to_string()),
        Some("single-chain".to_string()),
        CodeBaseline {
            head_sha: Some("e0781bf6".to_string()),
            worktree: true,
            worktree_path: Some(base.to_string_lossy().to_string()),
        },
    );
    let res = write_input_bundle(&session, &next).expect("write_manifest");
    assert!(res.is_some());
    let bytes = fs::read(&path).expect("read updated manifest");
    let updated: DiagnosisInputBundle =
        serde_json::from_slice(&bytes).expect("updated manifest is valid JSON");
    assert_eq!(updated.manifest_status, ManifestStatus::Present);
    assert_eq!(updated.run.loop_id.as_deref(), Some("loop-x"));
    assert_eq!(
        updated.run.preset_label.as_deref(),
        Some("builtin:ce-executor-pipeline")
    );
    assert!(updated.code_baseline.worktree);

    // finalize
    let artifacts = vec![ArtifactIntegrity {
        path: "recovery.jsonl".to_string(),
        status: ArtifactStatus::Present,
        sha256: Some("deadbeef".to_string()),
        size_bytes: Some(128),
        last_modified: None,
    }];
    let final_bundle = updated.with_finalized(artifacts, vec!["single-chain".to_string()], Vec::new());
    let res = write_input_bundle(&session, &final_bundle).expect("write_manifest");
    assert!(res.is_some());
    let bytes = fs::read(&path).expect("read final manifest");
    let final_read: DiagnosisInputBundle =
        serde_json::from_slice(&bytes).expect("final manifest is valid JSON");
    assert_eq!(final_read.manifest_status, ManifestStatus::Finalized);
    assert_eq!(final_read.artifacts.len(), 1);
    assert_eq!(
        final_read.execution_capabilities,
        vec!["single-chain".to_string()]
    );
}

#[test]
fn disabled_collector_writes_no_new_artifacts() {
    let tmp = TempDir::new().expect("TempDir");
    let base = tmp.path();
    let opts = DiagnosticsOptions::default();
    let collector = DiagnosticsCollector::with_options(base, &opts).expect("disabled collector");
    assert!(!collector.is_enabled());
    assert!(collector.input_bundle_status().is_none());
    assert!(collector.session_dir().is_none());
    // No .ralph directory should have been created.
    let ralph_dir = base.join(".ralph");
    assert!(
        !ralph_dir.exists(),
        "disabled collector must not create session dir"
    );
}

#[test]
fn bundle_write_failure_is_degraded_not_error() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");

    // Write a normal pending manifest first.
    write_pending_bundle(&session);
    let path = manifest_path(&session);
    assert!(path.is_file());

    // Now break the path by replacing it with a directory. The next
    // write must fail (return Ok(None)) and never panic.
    fs::remove_file(&path).expect("remove manifest");
    fs::create_dir_all(&path).expect("create blocking dir at manifest path");

    let bundle = DiagnosisInputBundle::new_pending(&session);
    let res = write_input_bundle(&session, &bundle)
        .expect("write_manifest returns Ok even on write failure");
    assert!(
        res.is_none(),
        "write_manifest returns Ok(None) when target is unwritable"
    );

    // The on-disk state is the broken directory; read_manifest should
    // either succeed via the previous data (not the case here, since
    // we removed the file) or return None.
    let read = ralph_core::diagnostics::read_manifest(&session);
    assert!(
        read.is_none(),
        "read_manifest returns None for missing/garbled data"
    );
}

#[test]
fn serde_round_trip_preserves_metadata() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path();
    let bundle = DiagnosisInputBundle::new_pending(session).with_completed_identity(
        Some("loop-1".to_string()),
        Some("builtin:debug".to_string()),
        Some("ralph.yml".to_string()),
        None,
        Some("abc".to_string()),
        Some("single-chain".to_string()),
        CodeBaseline {
            head_sha: Some("abc".to_string()),
            worktree: false,
            worktree_path: None,
        },
    );
    let json = serde_json::to_string(&bundle).expect("serialize");
    let parsed: DiagnosisInputBundle = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.run.loop_id.as_deref(), Some("loop-1"));
    assert_eq!(parsed.run.preset_label.as_deref(), Some("builtin:debug"));
    assert_eq!(parsed.code_baseline.head_sha.as_deref(), Some("abc"));
    assert_eq!(parsed.run, bundle.run);
    assert_eq!(parsed.code_baseline, bundle.code_baseline);
}

#[test]
fn manifest_artifact_status_serializes_all_four_states() {
    use ralph_core::diagnostics::ArtifactStatus as A;
    let states = [
        A::Present,
        A::Missing,
        A::Degraded,
        A::NotApplicable,
        A::Legacy,
    ];
    for s in states {
        let v = serde_json::to_value(s).expect("serialize");
        let back: A = serde_json::from_value(v).expect("deserialize");
        assert_eq!(back, s);
    }
}

#[test]
fn run_metadata_default_fields_are_none() {
    let m = RunMetadata::default();
    assert!(m.session_id.is_none());
    assert!(m.loop_id.is_none());
    assert!(m.preset_label.is_none());
    assert!(m.config_path.is_none());
    assert!(m.plan_path.is_none());
    assert!(m.baseline_sha.is_none());
    assert!(m.execution_capability.is_none());
}

#[test]
fn collector_update_and_finalize_round_trip() {
    let tmp = TempDir::new().expect("TempDir");
    let base = tmp.path();
    let opts = DiagnosticsOptions {
        full_diagnostics: false,
        runtime_diagnosis_artifacts: true,
        trace_only: false,
        session_dir: None,
        workspace_root: None,
        // U01b: causal_evidence defaults to `false` so the minimal
        // session here stays byte-equivalent to the pre-U01b
        // `runtime_diagnosis_artifacts=true` shape.
        causal_evidence: false,

        causal_evidence_window_capacity: None,
    };
    let collector = DiagnosticsCollector::with_options(base, &opts).expect("collector");
    let session = collector.session_dir().unwrap().to_path_buf();
    collector.update_input_bundle_identity(
        Some("loop-y".to_string()),
        Some("builtin:debug".to_string()),
        Some("ralph.yml".to_string()),
        Some("plan.md".to_string()),
        Some("e0781bf6".to_string()),
        Some("single-chain".to_string()),
        CodeBaseline {
            head_sha: Some("e0781bf6".to_string()),
            worktree: true,
            worktree_path: Some(base.to_string_lossy().to_string()),
        },
    );
    collector.finalize_input_bundle(
        vec![ArtifactIntegrity {
            path: "trace.jsonl".to_string(),
            status: ArtifactStatus::Present,
            sha256: None,
            size_bytes: None,
            last_modified: None,
        }],
        vec!["single-chain".to_string()],
    );

    let bytes = fs::read(manifest_path(&session)).expect("read final manifest");
    let bundle: DiagnosisInputBundle = serde_json::from_slice(&bytes).expect("parse");
    assert_eq!(bundle.manifest_status, ManifestStatus::Finalized);
    assert_eq!(bundle.run.loop_id.as_deref(), Some("loop-y"));
    assert_eq!(
        bundle.execution_capabilities,
        vec!["single-chain".to_string()]
    );
    assert_eq!(bundle.artifacts.len(), 1);
}

// =============================================================================
// Plan 2026-08-26-1104 Unit 7 (U07): 边界覆盖证明 manifest v2 tests.
// 8 boundaries (effective_contract / activation / backend_outcome /
// event_candidate / policy_decision / state_commit / recovery_action /
// termination) are written into `boundary_coverage[]` with expected /
// recorded counters and a status (`covered` when expected == recorded,
// otherwise `gap`). When the collector is degraded mid-run, the gap entries
// carry a reason so the reporter can surface the cause.
// =============================================================================

#[test]
fn v2_manifest_contains_eight_boundary_coverage_entries() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    // Drive the producer-side counters directly so the test is
    // deterministic regardless of whether receipt emitters
    // happen to fire during this session.
    let mut counters = std::collections::BTreeMap::new();
    for boundary in CausalBoundary::all() {
        counters.insert(boundary, ralph_core::diagnostics::BoundaryCounter {
            expected: 1,
            recorded: 1,
        });
    }
    let coverage: Vec<BoundaryCoverageEntry> = counters
        .iter()
        .map(|(boundary, counter)| BoundaryCoverageEntry::new(*boundary, counter, None))
        .collect();
    assert_eq!(coverage.len(), 8, "must enumerate all 8 boundaries");
    let bundle = DiagnosisInputBundle::new_pending(&session)
        .with_finalized(Vec::new(), Vec::new(), coverage);
    let res = write_input_bundle(&session, &bundle).expect("write_manifest");
    assert!(res.is_some());

    let bytes = fs::read(manifest_path(&session)).expect("read manifest");
    let parsed: DiagnosisInputBundle = serde_json::from_slice(&bytes).expect("parse");
    assert_eq!(parsed.schema_version, "run-diagnosis-input/v2");
    assert_eq!(parsed.manifest_status, ManifestStatus::Finalized);
    assert_eq!(
        parsed.boundary_coverage.len(),
        8,
        "v2 manifest must include all 8 boundary entries"
    );
    let names: Vec<&'static str> = parsed
        .boundary_coverage
        .iter()
        .map(|e| e.boundary.as_str())
        .collect();
    assert!(names.contains(&"effective_contract"));
    assert!(names.contains(&"activation"));
    assert!(names.contains(&"backend_outcome"));
    assert!(names.contains(&"event_candidate"));
    assert!(names.contains(&"policy_decision"));
    assert!(names.contains(&"state_commit"));
    assert!(names.contains(&"recovery_action"));
    assert!(names.contains(&"termination"));
    for entry in &parsed.boundary_coverage {
        assert_eq!(entry.expected, 1);
        assert_eq!(entry.recorded, 1);
        assert_eq!(entry.status, BoundaryCoverageStatus::Covered);
        assert!(
            entry.reason.is_none(),
            "covered entries must not carry a reason; got {:?}",
            entry
        );
    }
}

#[test]
fn degraded_logger_produces_structured_gap() {
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    // Mirror what finalize_input_bundle does when the on-disk write
    // fails: take a snapshot, mark the manifest degraded, and stamp
    // the gap entries with a reason. The producer records expected
    // bumps but the recorded counter stops increasing (degraded).
    let reason_text = "logger write failed".to_string();
    let mut coverage = Vec::new();
    for boundary in CausalBoundary::all() {
        let counter = ralph_core::diagnostics::BoundaryCounter {
            expected: 3,
            recorded: 1,
        };
        coverage.push(BoundaryCoverageEntry::new(boundary, &counter, Some(reason_text.clone())));
    }
    let bundle = DiagnosisInputBundle::new_pending(&session)
        .with_finalized(Vec::new(), Vec::new(), coverage)
        .mark_degraded();
    let res = write_input_bundle(&session, &bundle).expect("write_manifest");
    assert!(res.is_some());

    let bytes = fs::read(manifest_path(&session)).expect("read manifest");
    let parsed: DiagnosisInputBundle = serde_json::from_slice(&bytes).expect("parse");
    assert_eq!(parsed.manifest_status, ManifestStatus::Degraded);
    assert_eq!(parsed.boundary_coverage.len(), 8);
    for entry in &parsed.boundary_coverage {
        assert_eq!(entry.expected, 3);
        assert_eq!(entry.recorded, 1);
        assert_eq!(entry.status, BoundaryCoverageStatus::Gap);
        assert_eq!(
            entry.reason.as_deref(),
            Some("logger write failed"),
            "degraded gap entries must carry a reason; got {:?}",
            entry
        );
    }
}

#[test]
fn v1_manifest_without_coverage_serializes_back_as_legacy() {
    // The v1 schema shipped without `boundary_coverage`. The reader
    // must still surface the bundle, and the report's coverage
    // projection must reflect the absent field.
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    let mut bundle = DiagnosisInputBundle::new_pending(&session);
    bundle.schema_version = "run-diagnosis-input/v1".to_string();
    let res = write_input_bundle(&session, &bundle).expect("write_manifest");
    assert!(res.is_some());

    let parsed = ralph_core::diagnostics::read_manifest(&session).expect("read v1 manifest");
    assert_eq!(parsed.schema_version, "run-diagnosis-input/v1");
    assert!(
        parsed.boundary_coverage.is_empty(),
        "v1 manifest must not carry boundary_coverage rows"
    );
}
