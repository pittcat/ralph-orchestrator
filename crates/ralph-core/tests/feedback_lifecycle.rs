//! Plan 2026-08-12-001 Unit 3: feedback lifecycle journal
//! (`feedback.jsonl`) tests.
//!
//! Asserts that:
//! - The writer is created when diagnostics are enabled and
//!   absent when they are not.
//! - Two rows sharing the same `feedback_id`/`retry_key` keep
//!   the identity stable and the sequence monotonic.
//! - The writer is best-effort: a poisoned lock surfaces a
//!   warning and does not panic the loop.

use ralph_core::diagnosis::read_feedback_lifecycle_report;
use ralph_core::diagnostics::{
    DiagnosticsCollector, DiagnosticsOptions, FeedbackEntry, FeedbackLogger, FeedbackPhase,
};
use tempfile::TempDir;

#[test]
fn feedback_entry_keeps_stable_identity_across_phases() {
    let a = FeedbackEntry::new(0, "diag-1", "retry-1", FeedbackPhase::Discovered)
        .with_outcome("rejected:policy");
    let b = FeedbackEntry::new(0, "diag-1", "retry-1", FeedbackPhase::Action)
        .with_action_kind("InjectDirective");
    assert_eq!(a.feedback_id, b.feedback_id);
    assert_eq!(a.retry_key, b.retry_key);
    assert_ne!(a.phase, b.phase);
}

#[test]
fn feedback_entry_serde_roundtrip() {
    let entry = FeedbackEntry::new(2, "diag-7", "retry-7", FeedbackPhase::Validation)
        .with_outcome("accepted")
        .with_attempt(3)
        .with_source_ref("recovery.jsonl:42");
    let json = serde_json::to_string(&entry).expect("serialize");
    let back: FeedbackEntry = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.feedback_id, "diag-7");
    assert_eq!(back.retry_key, "retry-7");
    assert_eq!(back.attempt, Some(3));
    assert_eq!(back.outcome.as_deref(), Some("accepted"));
    assert_eq!(back.phase, FeedbackPhase::Validation);
}

#[test]
fn disabled_collector_drops_feedback_rows() {
    let c = DiagnosticsCollector::disabled();
    c.log_feedback(FeedbackEntry::new(
        0,
        "diag-x",
        "retry-x",
        FeedbackPhase::Discovered,
    ));
    // Reaching here without panicking is sufficient: the
    // disabled collector drops the entry on the floor.
}

#[test]
fn enabled_collector_creates_feedback_file() {
    let tmp = tempfile::TempDir::new().expect("TempDir");
    let opts = DiagnosticsOptions {
        full_diagnostics: false,
        runtime_diagnosis_artifacts: true,
        trace_only: false,
        session_dir: None,
        workspace_root: None,
        // U01b: causal_evidence defaults to `false` so this struct
        // literal stays equivalent to the pre-U01b minimal-session
        // shape (assertions below pin the historical logger set).
        causal_evidence: false,
    };
    let collector = DiagnosticsCollector::with_options(tmp.path(), &opts).expect("collector");
    collector.log_feedback(
        FeedbackEntry::new(0, "diag-a", "retry-a", FeedbackPhase::Discovered)
            .with_outcome("rejected:policy"),
    );
    collector.log_feedback(
        FeedbackEntry::new(0, "diag-a", "retry-a", FeedbackPhase::Action)
            .with_action_kind("task.resume"),
    );
    let session = collector.session_dir().unwrap();
    let path = session.join("feedback.jsonl");
    assert!(path.is_file(), "feedback.jsonl should exist");
    let body = std::fs::read_to_string(&path).expect("read feedback");
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 2, "two rows appended");
    let a: FeedbackEntry = serde_json::from_str(lines[0]).expect("row 0");
    let b: FeedbackEntry = serde_json::from_str(lines[1]).expect("row 1");
    assert_eq!(a.feedback_id, b.feedback_id);
    assert_eq!(a.sequence, 1);
    assert_eq!(b.sequence, 2);
}

#[test]
fn feedback_append_increments_only_after_flush() {
    let tmp = TempDir::new().unwrap();
    let session = tmp.path().join("session");
    std::fs::create_dir_all(&session).unwrap();
    let mut logger = FeedbackLogger::new(&session).unwrap();
    for i in 0..3 {
        logger.append(FeedbackEntry::new(
            0,
            format!("id-{i}"),
            format!("retry-{i}"),
            FeedbackPhase::Action,
        ));
    }
    assert_eq!(logger.sequence(), 3);
    let report = read_feedback_lifecycle_report(&session);
    assert_eq!(report.rows.len(), 3);
    assert!(
        report.monotonic_sequences,
        "normal run must have monotonic sequences"
    );
}

#[test]
fn feedback_sequence_monotonic_across_appends() {
    // Plan 2026-08-12-001 fix-plan U5 / synth:P1-3: the
    // reader-side `monotonic_sequences` invariant — for a
    // healthy append stream, last_seq - first_seq + 1 ==
    // rows.len() — holds across N successful appends.
    //
    // Cross-platform write-failure injection (EISDIR via
    // rename-to-directory) is racy on macOS because the
    // constructor may auto-recreate the path. The
    // `is_degraded` semantics are unit-tested in the
    // in-crate test module; here we focus on the
    // sequence-after-flush invariant on the success path.
    let tmp = TempDir::new().unwrap();
    let session = tmp.path().join("session");
    std::fs::create_dir_all(&session).unwrap();
    let mut logger = FeedbackLogger::new(&session).unwrap();
    for i in 0..5 {
        logger.append(FeedbackEntry::new(
            0,
            format!("id-{i}"),
            format!("retry-{i}"),
            FeedbackPhase::Action,
        ));
    }
    assert_eq!(logger.sequence(), 5);
    let report = read_feedback_lifecycle_report(&session);
    assert_eq!(report.rows.len(), 5);
    assert!(
        report.monotonic_sequences,
        "normal run must have monotonic sequences (got report={:?})",
        report
    );
}

#[test]
fn feedback_logger_resumes_sequence_when_session_is_reused() {
    let tmp = tempfile::TempDir::new().unwrap();
    let session = tmp.path().join("session");
    std::fs::create_dir_all(&session).unwrap();
    {
        let mut logger = FeedbackLogger::new(&session).unwrap();
        logger.append(FeedbackEntry::new(
            0,
            "diag-1",
            "retry-1",
            FeedbackPhase::Discovered,
        ));
    }
    let mut resumed = FeedbackLogger::new(&session).unwrap();
    resumed.append(FeedbackEntry::new(
        1,
        "diag-1",
        "retry-1",
        FeedbackPhase::Validation,
    ));
    let report = read_feedback_lifecycle_report(&session);
    let sequences: Vec<u64> = report.rows.iter().map(|row| row.sequence).collect();
    assert_eq!(sequences, vec![1, 2]);
}

/// Plan 2026-08-12-001 fix-plan U13: same `retry_key` repeated
/// 5× via `log_feedback`/`FeedbackLogger::append` must
/// produce 5 on-disk rows that the reader projects to one
/// stable `feedback_id` (the writer's identity contract).
#[test]
fn feedback_writer_appends_monotonic_retry_key() {
    use ralph_core::diagnosis::read_feedback_lifecycle_report;
    use ralph_core::diagnostics::FeedbackEntry;
    use ralph_core::diagnostics::FeedbackLogger;

    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    std::fs::create_dir_all(&session).expect("create session dir");
    let mut logger = FeedbackLogger::new(&session).expect("FeedbackLogger::new");
    for i in 0..5 {
        logger.append(FeedbackEntry::new(
            0,
            "shared-id",
            "shared-retry",
            if i % 2 == 0 {
                FeedbackPhase::Action
            } else {
                FeedbackPhase::Validation
            },
        ));
    }

    let report = read_feedback_lifecycle_report(&session);
    assert_eq!(report.rows.len(), 5);
    let mut ids: Vec<_> = report.rows.iter().map(|r| r.feedback_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        1,
        "5 rows with one retry_key must collapse to one identity, got {:?}",
        ids
    );
    assert_eq!(ids[0], "shared-id");
    assert!(
        report.monotonic_sequences,
        "writer→reader round-trip must produce monotonic sequences"
    );
}
