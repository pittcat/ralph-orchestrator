//! Plan 2026-08-12-001 fix-plan U5 / synth:P1-3: sequence
//! increment must happen AFTER write+flush; reader surfaces
//! `monotonic_sequences: bool`.
use ralph_core::diagnostics::{RuntimeTraceEntry, RuntimeTraceLogger, RuntimeTracePhase};
use ralph_core::diagnosis::read_runtime_trace_report;
use tempfile::TempDir;

#[test]
fn append_increments_only_after_flush() {
    let tmp = TempDir::new().unwrap();
    let session = tmp.path().join("session");
    std::fs::create_dir_all(&session).unwrap();
    let mut logger = RuntimeTraceLogger::new(&session).unwrap();
    for i in 0..3 {
        logger.append(
            RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Activation)
                .with_hat(format!("hat-{i}")),
        );
    }
    // After 3 successful appends, sequence must be 3 and reader must see 3 rows.
    assert_eq!(logger.sequence(), 3);
    let report = read_runtime_trace_report(&session);
    assert_eq!(report.record_count, 3);
    assert_eq!(report.first_sequence, Some(1));
    assert_eq!(report.last_sequence, Some(3));
    assert!(
        report.monotonic_sequences,
        "normal run must have monotonic sequences"
    );
}

#[test]
fn sequence_monotonic_across_appends() {
    // Plan 2026-08-12-001 fix-plan U5 / synth:P1-3: the
    // reader-side `monotonic_sequences` invariant — for a
    // healthy append stream, last_seq - first_seq + 1 ==
    // record_count — holds across N successful appends.
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
    let mut logger = RuntimeTraceLogger::new(&session).unwrap();
    for i in 0..5 {
        logger.append(
            RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Activation)
                .with_hat(format!("hat-{i}")),
        );
    }
    assert_eq!(logger.sequence(), 5);
    let report = read_runtime_trace_report(&session);
    assert_eq!(report.record_count, 5);
    assert!(
        report.monotonic_sequences,
        "normal run must have monotonic sequences (got report={:?})",
        report
    );
}

#[test]
fn runtime_trace_logger_resumes_sequence_when_session_is_reused() {
    let tmp = tempfile::TempDir::new().unwrap();
    let session = tmp.path().join("session");
    std::fs::create_dir_all(&session).unwrap();
    {
        let mut logger = RuntimeTraceLogger::new(&session).unwrap();
        logger.append(RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Batch));
    }
    let mut resumed = RuntimeTraceLogger::new(&session).unwrap();
    resumed.append(RuntimeTraceEntry::new(1, 0, RuntimeTracePhase::Commit));
    let report = read_runtime_trace_report(&session);
    assert_eq!(report.first_sequence, Some(1));
    assert_eq!(report.last_sequence, Some(2));
    assert!(report.monotonic_sequences);
}
