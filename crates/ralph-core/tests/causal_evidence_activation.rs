//! Plan 2026-08-26-1104 U01b acceptance tests: causal evidence
//! diagnostics activation. Verifies the activation-matrix row
//! driven by `DiagnosticsOptions::causal_evidence = true`:
//!
//! - `causal_evidence=true` and no other flags ⇒ minimal
//!   session created, `runtime-trace.jsonl` lazy-writable, no
//!   historical full-diagnostics files (`agent-output.jsonl`,
//!   `prompt-log.md`, `orchestration.jsonl`).
//! - `causal_evidence=false` and no other flags ⇒ no session
//!   directory at all.
//! - `full_diagnostics=true` and `causal_evidence=true` ⇒ the
//!   full diagnostics mode subsumes the causal row; the existing
//!   historical loggers are created and no second session is
//!   opened.
//!
//! These tests deliberately bypass the telemetry bridge (that
//! lives in `crates/ralph-core/src/config/telemetry.rs` and is
//! the U01a scope) — they construct `DiagnosticsOptions`
//! directly so the matrix contract can be locked down
//! independently of the YAML→options plumbing. U01a's commit
//! adds the `CausalEvidenceConfig` field and fills
//! `options.causal_evidence` from
//! `telemetry.causal_evidence.enabled`; the integration test
//! for that bridge lives in `crates/ralph-core/src/config/telemetry.rs`.

use ralph_core::diagnostics::{DiagnosticsCollector, DiagnosticsOptions};
use tempfile::TempDir;

fn session_dir_for(temp: &TempDir) -> std::path::PathBuf {
    let diag_root = temp.path().join(".ralph").join("diagnostics");
    assert!(
        diag_root.is_dir(),
        "expected diagnostics dir at {}",
        diag_root.display()
    );
    let entries: Vec<_> = std::fs::read_dir(&diag_root)
        .expect("read diagnostics dir")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one session dir under {}",
        diag_root.display()
    );
    entries[0].path()
}

#[test]
fn causal_evidence_creates_minimal_session_with_runtime_trace_logger() {
    let temp = TempDir::new().expect("TempDir");
    let opts = DiagnosticsOptions {
        full_diagnostics: false,
        runtime_diagnosis_artifacts: false,
        trace_only: false,
        session_dir: None,
        workspace_root: None,
        causal_evidence: true,
    };
    let collector = DiagnosticsCollector::with_options(temp.path(), &opts).expect("collector");

    assert!(
        collector.is_enabled(),
        "causal_evidence=true alone must satisfy is_enabled()"
    );
    let session = collector
        .session_dir()
        .expect("session dir must be created when causal_evidence=true");

    // Write one runtime-trace entry through the production logger API so we
    // verify the logger was actually wired into the collector — not just
    // that the session dir exists.
    use ralph_core::diagnostics::{RuntimeTraceEntry, RuntimeTracePhase};
    collector.log_runtime_trace(
        RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Activation).with_hat("executor"),
    );

    let runtime_trace = session.join("runtime-trace.jsonl");
    assert!(
        runtime_trace.is_file(),
        "minimal causal session must create runtime-trace.jsonl at {}",
        runtime_trace.display()
    );

    // Historical full-diagnostics files MUST NOT exist under minimal session
    // (mirrors `test_minimal_runtime_diagnosis_creates_recovery_logger`).
    assert!(!session.join("agent-output.jsonl").exists());
    assert!(!session.join("prompt-log.md").exists());
    assert!(!session.join("orchestration.jsonl").exists());

    // session_dir pointer round-trip
    assert_eq!(session, session_dir_for(&temp));
}

#[test]
fn causal_evidence_disabled_with_no_other_flags_creates_no_session() {
    let temp = TempDir::new().expect("TempDir");
    let opts = DiagnosticsOptions {
        full_diagnostics: false,
        runtime_diagnosis_artifacts: false,
        trace_only: false,
        session_dir: None,
        workspace_root: None,
        causal_evidence: false,
    };
    let collector = DiagnosticsCollector::with_options(temp.path(), &opts).expect("collector");

    assert!(
        !collector.is_enabled(),
        "all-false options must report is_enabled() = false"
    );
    assert!(
        collector.session_dir().is_none(),
        "all-false options must not create a session dir"
    );
    assert!(
        !temp.path().join(".ralph").join("diagnostics").exists(),
        ".ralph/diagnostics must not be created when nothing is enabled"
    );
}

#[test]
fn full_diagnostics_subsumes_causal_evidence_into_single_session() {
    let temp = TempDir::new().expect("TempDir");
    let opts = DiagnosticsOptions {
        full_diagnostics: true,
        runtime_diagnosis_artifacts: false,
        trace_only: false,
        session_dir: None,
        workspace_root: None,
        causal_evidence: true,
    };
    let collector = DiagnosticsCollector::with_options(temp.path(), &opts).expect("collector");

    let session = collector
        .session_dir()
        .expect("session dir must exist when full_diagnostics=true");

    assert!(
        collector.is_full_diagnostics(),
        "full_diagnostics=true must report is_full_diagnostics() = true"
    );
    assert!(
        !collector.has_runtime_diagnosis_artifacts(),
        "full must NOT also flip runtime_diagnosis_artifacts (subsumption)"
    );

    // Full diagnostics lazily creates the historical files on first log.
    use ralph_core::diagnosis::{DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope};
    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::DriftMonitor)
        .severity(DiagnosisSeverity::Info)
        .reason_code("causal-evidence-full-test")
        .message("full + causal_evidence creates single session")
        .build();
    collector.log_recovery(ralph_core::diagnosis::RecoveryJournalEntry::from_envelope(
        envelope,
        Vec::new(),
    ));

    // Full set: recovery.jsonl + the runtime-trace sidecar must coexist
    // with the historical loggers.
    assert!(session.join("recovery.jsonl").is_file());
    assert!(session.join("orchestration.jsonl").is_file());

    // Exactly one session dir — full and causal must NOT open a second one.
    let diag_root = temp.path().join(".ralph").join("diagnostics");
    let entries: Vec<_> = std::fs::read_dir(&diag_root)
        .expect("read diagnostics dir")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "full + causal_evidence must produce exactly one session dir, got {}",
        entries.len()
    );
}
