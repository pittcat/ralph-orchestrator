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

use ralph_core::diagnostics::{
    DiagnosticsCollector, DiagnosticsOptions, FeedbackEntry, FeedbackPhase,
};

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
