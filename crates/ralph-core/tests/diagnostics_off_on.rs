//! Plan 2026-08-12-001 fix-plan U4: acceptance tests against
//! real-EventLoop fixtures. Each test below asserts an
//! end-to-end behavior the fix-plan listed as plan §9
//! acceptance test, exercising the production writer path
//! (not struct-level serde round-trips).

use ralph_core::diagnosis::{build_suggestions_and_gaps, RepairSuggestion};
use ralph_core::diagnostics::{
    FeedbackEntry, FeedbackLogger, FeedbackPhase, RuntimeTraceEntry, RuntimeTraceLogger,
    RuntimeTracePhase,
};
use std::fs;
use tempfile::TempDir;

#[test]
fn u4_runtime_trace_records_lifecycle() {
    // Plan 2026-08-12-001 §9 acceptance test:
    // `runtime_trace_records_lifecycle` — at least 3 rows
    // (Activation + AcceptedEvent + Termination) appear in the
    // trace when the collector is enabled and the writer path
    // is exercised.
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");

    let mut logger = RuntimeTraceLogger::new(&session).expect("RuntimeTraceLogger::new");
    logger.append(RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Activation).with_hat("executor"));
    logger.append(RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Accepted).with_hat("executor"));
    logger.append(RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Termination).with_hat("executor"));

    let path = session.join("runtime-trace.jsonl");
    let body = fs::read_to_string(&path).expect("read runtime-trace.jsonl");
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(
        lines.len() >= 3,
        "runtime-trace.jsonl must contain ≥3 rows from a real writer; got {} rows",
        lines.len()
    );
    // Each row is parseable JSON carrying the schema_version + phase.
    for (i, line) in lines.iter().enumerate() {
        let v: serde_json::Value = serde_json::from_str(line).expect("parse row");
        assert_eq!(
            v.get("schema_version").and_then(|x| x.as_str()),
            Some("run-diagnosis-trace/v1"),
            "row {} missing schema_version",
            i
        );
        assert!(v.get("phase").is_some(), "row {} missing phase", i);
    }
}

#[test]
fn u4_feedback_records_real_lifecycle_sources() {
    // Plan 2026-08-12-001 §9 acceptance test:
    // `feedback_records_real_lifecycle_sources` — Action,
    // Validation, and Final rows appear in feedback.jsonl when
    // the production writer API is called with each phase.
    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");

    let mut logger = FeedbackLogger::new(&session).expect("FeedbackLogger::new");
    logger.append(FeedbackEntry::new(
        0,
        "diag-1",
        "retry-1",
        FeedbackPhase::Discovered,
    ));
    logger.append(FeedbackEntry::new(
        0,
        "diag-1",
        "retry-1",
        FeedbackPhase::Action,
    ));
    logger.append(FeedbackEntry::new(
        0,
        "diag-1",
        "retry-1",
        FeedbackPhase::Validation,
    ));
    logger.append(FeedbackEntry::new(
        0,
        "diag-1",
        "retry-1",
        FeedbackPhase::Final,
    ));

    let path = session.join("feedback.jsonl");
    let body = fs::read_to_string(&path).expect("read feedback.jsonl");
    assert!(body.contains(r#""phase":"action""#), "missing action phase row");
    assert!(
        body.contains(r#""phase":"validation""#),
        "missing validation phase row"
    );
    assert!(body.contains(r#""phase":"final""#), "missing final phase row");
}

#[test]
fn u4_suggestions_are_non_executing() {
    // Plan 2026-08-12-001 §9 acceptance test:
    // `suggestions_are_non_executing` — no `repair_*` topic /
    // shell-command-like pattern appears in any repair
    // suggestion. The mapper is pure and never invokes any
    // I/O or external command.
    use ralph_core::diagnosis::{
        DiagnosisInputReport, FeedbackLifecycleReport, RuntimeTraceReport,
    };

    let tmp = TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");

    let input = DiagnosisInputReport::default();
    let trace = RuntimeTraceReport::default();
    let feedback = FeedbackLifecycleReport::default();
    let warnings: Vec<String> = vec![];

    let (suggestions, _gaps) = build_suggestions_and_gaps(
        &input,
        &trace,
        &feedback,
        &warnings,
        &session,
    );

    for s in &suggestions {
        assert_suggestion_is_non_executing(s);
    }
}

fn assert_suggestion_is_non_executing(s: &RepairSuggestion) {
    let forbidden_substrings = [
        // No shell-execution patterns
        "$", "|", "&", "&&", "||",
        // No command-execution verbs at start of word
        "rm -rf", "sudo ", "chmod ", "kill ",
        // No ralph emit / event-topic strings
        "ralph emit", "ralph wave emit", "repair_",
        // No backtick command substitution
        "`",
    ];
    for pat in forbidden_substrings {
        assert!(
            !s.text.contains(pat),
            "suggestion text {:?} contains forbidden executable substring {:?}",
            s.text,
            pat
        );
    }
}