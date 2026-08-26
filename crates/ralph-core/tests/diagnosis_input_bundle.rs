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
    ArtifactIntegrity, ArtifactStatus, CodeBaseline, DIAGNOSIS_INPUT_SCHEMA_VERSION,
    DiagnosisInputBundle, DiagnosticsCollector, DiagnosticsOptions, ManifestStatus, RunMetadata,
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
    let final_bundle = updated.with_finalized(artifacts, vec!["single-chain".to_string()]);
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
