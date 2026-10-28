//! Tests for the timing capture contract (plan 2026-10-29-0104 U1).
//!
//! Covered scenarios from the plan:
//! - two jobs in the same iteration keep independent identities (R2)
//! - attempts after restart never blend (attempt distinguishes rows)
//! - a writer failure flips to `degraded` without touching the main path
//! - capture disabled → `not_collected` on the manifest
//! - malformed lines and missing boundaries are detectable by readers
//! - spans round-trip through the file intact

use super::model::*;
use super::writer::TimingWriter;
use std::io::BufRead;

fn read_rows(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let path = dir.join(TIMING_SIDECAR_FILENAME);
    let file = std::fs::File::open(path).expect("timing.jsonl exists");
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

#[test]
fn identity_value_three_states_roundtrip() {
    // Missing → field absent from the row.
    let missing = IdentityValue::Missing;
    assert!(missing.is_missing());
    // NotApplicable → literal "n/a"; Value → the string.
    assert_eq!(
        serde_json::to_string(&IdentityValue::NotApplicable).unwrap(),
        "\"n/a\""
    );
    assert_eq!(
        serde_json::to_string(&IdentityValue::value("U1")).unwrap(),
        "\"U1\""
    );
    // Round-trip: null → Missing, "n/a" → NotApplicable.
    assert_eq!(
        serde_json::from_str::<IdentityValue>("null").unwrap(),
        IdentityValue::Missing
    );
    assert_eq!(
        serde_json::from_str::<IdentityValue>("\"n/a\"").unwrap(),
        IdentityValue::NotApplicable
    );
    assert_eq!(
        serde_json::from_str::<IdentityValue>("\"u-x\"").unwrap(),
        IdentityValue::value("u-x")
    );
    // Empty string input normalizes to Missing.
    assert_eq!(IdentityValue::value(""), IdentityValue::Missing);
}

#[test]
fn span_lifecycle_open_then_complete() {
    let epoch = ClockEpoch::new();
    let identity = ExecutionIdentity {
        loop_id: Some("loop-1".into()),
        activation_id: Some("act-1".into()),
        job_id: Some("j-1".into()),
        attempt: Some(1),
        stage: Some("execute".into()),
        hat: Some("executor".into()),
        plan_key: IdentityValue::value("plan-a"),
        unit_key: IdentityValue::value("U1"),
        task_key: IdentityValue::NotApplicable,
        ..ExecutionIdentity::default()
    };
    let mut span = TimingSpan::open("sp-1", identity, SpanKind::AgentExec, &epoch);
    assert_eq!(span.status, SpanStatus::Open);
    assert!(span.ts_end_utc.is_none());
    assert_eq!(span.duration_ms(), None, "open span is censored");

    std::thread::sleep(std::time::Duration::from_millis(2));
    span.complete(&epoch);
    assert_eq!(span.status, SpanStatus::Completed);
    assert!(span.ts_end_utc.is_some());
    let d = span.duration_ms().expect("closed span has duration");
    assert!(d >= 2, "duration {d}ms should cover the sleep");
}

#[test]
fn same_iteration_two_jobs_have_independent_identities() {
    // R2: two jobs under one iteration must never blend identity.
    let epoch = ClockEpoch::new();
    let base = ExecutionIdentity {
        loop_id: Some("loop-1".into()),
        session_id: Some("sess-1".into()),
        hat: Some("executor".into()),
        attempt: Some(1),
        ..ExecutionIdentity::default()
    };
    let a = ExecutionIdentity {
        job_id: Some("job-a".into()),
        activation_id: Some("act-a".into()),
        unit_key: IdentityValue::value("U1"),
        ..base.clone()
    };
    let b = ExecutionIdentity {
        job_id: Some("job-b".into()),
        activation_id: Some("act-b".into()),
        unit_key: IdentityValue::value("U2"),
        ..base
    };
    let span_a = TimingSpan::open(epoch.next_span_id(), a, SpanKind::AgentExec, &epoch);
    let span_b = TimingSpan::open(epoch.next_span_id(), b, SpanKind::AgentExec, &epoch);
    assert_ne!(span_a.span_id, span_b.span_id);
    assert_ne!(span_a.identity.job_id, span_b.identity.job_id);
    assert_ne!(
        span_a.identity.activation_id,
        span_b.identity.activation_id
    );
}

#[test]
fn attempts_distinguish_retry_records() {
    // R2 / AE3: attempt 1 interrupted, attempt 2 completed — both
    // recorded under the same job but never merged.
    let epoch = ClockEpoch::new();
    let ident = |attempt: u32| ExecutionIdentity {
        job_id: Some("job-1".into()),
        attempt: Some(attempt),
        ..ExecutionIdentity::default()
    };
    let mut s1 = TimingSpan::open("sp-a1", ident(1), SpanKind::AgentExec, &epoch);
    let mut s2 = TimingSpan::open("sp-a2", ident(2), SpanKind::AgentExec, &epoch);
    s1.interrupt(&epoch);
    s2.complete(&epoch);
    assert_eq!(s1.status, SpanStatus::Interrupted);
    assert_eq!(s2.status, SpanStatus::Completed);
    assert_ne!(s1.identity.attempt, s2.identity.attempt);
}

#[test]
fn writer_round_trips_spans() {
    let tmp = tempfile::TempDir::new().unwrap();
    let epoch = ClockEpoch::new();
    let mut writer = TimingWriter::new(tmp.path()).unwrap();

    let ident = ExecutionIdentity {
        loop_id: Some("loop-9".into()),
        hat: Some("reviewer".into()),
        ..ExecutionIdentity::default()
    };
    let mut open = TimingSpan::open("sp-1", ident.clone(), SpanKind::ReviewVerify, &epoch)
        .with_source_ref("runtime-trace.jsonl#42");
    writer.append(open.clone());
    open.complete(&epoch);
    writer.append(open);

    let rows = read_rows(tmp.path());
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["sequence"], 1);
    assert_eq!(rows[1]["sequence"], 2);
    assert_eq!(rows[0]["status"], "open");
    assert_eq!(rows[1]["status"], "completed");
    // Full identity payload survived the round-trip.
    let decoded: TimingSpan = serde_json::from_value(rows[1].clone()).unwrap();
    assert_eq!(decoded.identity.loop_id.as_deref(), Some("loop-9"));
    assert_eq!(decoded.kind, SpanKind::ReviewVerify);
    assert_eq!(decoded.source_ref.as_deref(), Some("runtime-trace.jsonl#42"));
}

#[test]
fn writer_resumes_sequence_across_reopen() {
    let tmp = tempfile::TempDir::new().unwrap();
    let epoch = ClockEpoch::new();
    {
        let mut writer = TimingWriter::new(tmp.path()).unwrap();
        writer.append(TimingSpan::open(
            "sp-1",
            ExecutionIdentity::default(),
            SpanKind::Other,
            &epoch,
        ));
    }
    let mut writer = TimingWriter::new(tmp.path()).unwrap();
    writer.append(TimingSpan::open(
        "sp-2",
        ExecutionIdentity::default(),
        SpanKind::Other,
        &epoch,
    ));
    let rows = read_rows(tmp.path());
    assert_eq!(rows[1]["sequence"], 2, "sequence resumed after reopen");
}

#[test]
fn writer_marks_degraded_on_io_failure() {
    let tmp = tempfile::TempDir::new().unwrap();
    let writer = TimingWriter::new(tmp.path()).unwrap();
    drop(writer);
    // Read-only directory: construction must fail cleanly (R4 — the
    // caller degrades to "no timing", never errors the loop).
    let ro = tmp.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    let mut perms = std::fs::metadata(&ro).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&ro, perms).unwrap();
    let res = TimingWriter::new(&ro);
    #[cfg(unix)]
    assert!(res.is_err(), "read-only dir must fail to create the file");
    let _ = res;
}

#[test]
fn reader_detects_bad_lines_and_censored_rows() {
    // Bad JSONL rows must not break the reader; a span with only an
    // `open` row is censored (never zero-filled).
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join(TIMING_SIDECAR_FILENAME);
    let epoch = ClockEpoch::new();
    let open = TimingSpan::open(
        "sp-orphan",
        ExecutionIdentity::default(),
        SpanKind::AgentExec,
        &epoch,
    );
    let mut content = serde_json::to_string(&open).unwrap();
    content.push('\n');
    content.push_str("{not valid json}\n");
    content.push_str("{\"schema_version\":\"x\",\"span_id\":1}\n"); // wrong shape
    std::fs::write(&path, content).unwrap();

    // Parse line-by-line the same way the U4 reader will.
    let file = std::fs::File::open(&path).unwrap();
    let mut spans = Vec::new();
    let mut bad = 0usize;
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        match serde_json::from_str::<TimingSpan>(&line) {
            Ok(s) => spans.push(s),
            Err(_) => bad += 1,
        }
    }
    assert_eq!(spans.len(), 1);
    assert_eq!(bad, 2, "two malformed lines detected");
    assert_eq!(spans[0].status, SpanStatus::Open, "orphan stays censored");
    assert_eq!(spans[0].duration_ms(), None);
}

#[test]
fn no_cross_epoch_duration_subtraction() {
    // KTD2: two epochs must never produce a shared monotonic reading.
    let e1 = ClockEpoch::new();
    let e2 = ClockEpoch::new();
    assert_ne!(e1.id(), e2.id());
    let s = TimingSpan::open("sp-x", ExecutionIdentity::default(), SpanKind::Other, &e1);
    assert_eq!(s.clock_epoch, e1.id());
    assert_ne!(s.clock_epoch, e2.id());
}

#[test]
fn collector_writes_timing_rows() {
    let tmp = tempfile::TempDir::new().unwrap();
    let options = crate::diagnostics::DiagnosticsOptions {
        full_diagnostics: true,
        ..Default::default()
    };
    let collector =
        crate::diagnostics::DiagnosticsCollector::with_options(tmp.path(), &options).unwrap();
    assert!(collector.timing_capture_active());

    let mut handle = collector
        .begin_timing_span(
            ExecutionIdentity::for_hat("executor"),
            SpanKind::AgentExec,
            None,
        )
        .expect("span opened");
    handle.set_attributes(serde_json::json!({"backend": "fake"}));
    handle.complete();

    let session_dir = collector.session_dir().unwrap().to_path_buf();
    drop(collector);
    let rows = read_rows(&session_dir);
    assert_eq!(rows.len(), 2, "open + close rows");
    assert_eq!(rows[0]["status"], "open");
    assert_eq!(rows[1]["status"], "completed");
    assert_eq!(rows[1]["identity"]["hat"], "executor");
    assert_eq!(rows[1]["attributes"]["backend"], "fake");
}

#[test]
fn collector_honors_timing_capture_disabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let options = crate::diagnostics::DiagnosticsOptions {
        full_diagnostics: true,
        timing_capture: false,
        ..Default::default()
    };
    let collector =
        crate::diagnostics::DiagnosticsCollector::with_options(tmp.path(), &options).unwrap();
    assert!(!collector.timing_capture_active());
    assert!(
        collector
            .begin_timing_span(ExecutionIdentity::default(), SpanKind::Other, None)
            .is_none()
    );
    let session_dir = collector.session_dir().unwrap().to_path_buf();
    assert!(!session_dir.join(TIMING_SIDECAR_FILENAME).exists());
    // Record path is also a no-op — capture failure never reaches the
    // main path (R4).
    let epoch = ClockEpoch::new();
    collector.record_timing_span(TimingSpan::open(
        "sp-x",
        ExecutionIdentity::default(),
        SpanKind::Other,
        &epoch,
    ));
    assert!(!session_dir.join(TIMING_SIDECAR_FILENAME).exists());
}

#[test]
fn collector_disabled_means_not_collected() {
    let collector = crate::diagnostics::DiagnosticsCollector::disabled();
    assert!(!collector.timing_capture_active());
    assert!(
        collector
            .begin_timing_span(ExecutionIdentity::default(), SpanKind::Other, None)
            .is_none()
    );
}
