//! U5 bottleneck-analysis contract tests (plan 2026-10-29-0104).
//!
//! Every candidate must carry concrete evidence refs OR an explicit
//! gap — a missing observation can never produce a confident cause.

use ralph_core::diagnosis::performance::analyze;
use ralph_core::diagnosis::performance::bottleneck::CandidateKind;
use ralph_core::diagnostics::{
    ClockEpoch, ExecutionIdentity, IdentityValue, SpanKind, TimingSpan, TimingWriter,
};

fn write_rows(dir: &tempfile::TempDir, rows: &[TimingSpan]) {
    let mut w = TimingWriter::new(dir.path()).unwrap();
    for r in rows {
        w.append(r.clone());
    }
}

fn ident(hat: &str) -> ExecutionIdentity {
    ExecutionIdentity {
        loop_id: Some("l1".into()),
        hat: Some(hat.into()),
        unit_key: IdentityValue::value("U1"),
        ..Default::default()
    }
}

/// Open+close a span separated by a controlled delta on BOTH clocks —
/// the monotonic delta drives `duration_ms`, the UTC delta drives the
/// report's wall-clock window. Without the UTC shift a synthetic test
/// produces a ~0ms window and share computations collapse to 0.
fn completed_span(
    id: &str,
    identity: ExecutionIdentity,
    kind: SpanKind,
    epoch: &ClockEpoch,
) -> [TimingSpan; 2] {
    let mut span = TimingSpan::open(id, identity, kind, epoch);
    span.complete(epoch);
    span.mono_end_ms = Some(span.mono_end_ms.unwrap() + 5_000);
    let end = chrono::DateTime::parse_from_rfc3339(&span.ts_start_utc).unwrap()
        + chrono::Duration::milliseconds(5_000);
    span.ts_end_utc = Some(end.to_rfc3339());
    let open = TimingSpan {
        status: ralph_core::diagnostics::SpanStatus::Open,
        ts_end_utc: None,
        mono_end_ms: None,
        ..span.clone()
    };
    [open, span]
}

#[test]
fn slow_tool_exec_produces_evidence_backed_candidate() {
    let tmp = tempfile::TempDir::new().unwrap();
    let epoch = ClockEpoch::new();
    let exec = completed_span("sp-exec", ident("executor"), SpanKind::AgentExec, &epoch);
    let mut tool = TimingSpan::open("sp-tool", ident("executor"), SpanKind::ToolExec, &epoch)
        .with_parent("sp-exec")
        .with_attributes(serde_json::json!({"tool": "Bash"}));
    tool.complete(&epoch);
    tool.mono_end_ms = Some(tool.mono_end_ms.unwrap() + 6_000);
    let end = chrono::DateTime::parse_from_rfc3339(&tool.ts_start_utc).unwrap()
        + chrono::Duration::milliseconds(6_000);
    tool.ts_end_utc = Some(end.to_rfc3339());
    let tool_open = TimingSpan {
        status: ralph_core::diagnostics::SpanStatus::Open,
        ts_end_utc: None,
        mono_end_ms: None,
        ..tool.clone()
    };
    write_rows(&tmp, &[exec[0].clone(), exec[1].clone(), tool_open, tool]);

    let report = analyze(tmp.path());
    assert!(!report.bottlenecks.is_empty());
    // Every candidate satisfies the evidence-or-gap contract.
    for b in &report.bottlenecks {
        assert!(
            !b.evidence.is_empty() || b.gap.is_some(),
            "candidate without evidence must carry a gap: {:?}",
            b.finding
        );
    }
    // The tool span's candidate names its evidence.
    let tool_cand = report
        .bottlenecks
        .iter()
        .find(|b| b.span_id.as_deref() == Some("sp-tool"))
        .expect("tool span should be a candidate");
    assert_eq!(tool_cand.kind, CandidateKind::SlowStage);
    assert!(!tool_cand.evidence.is_empty());
    assert!(tool_cand.recommendation.is_some());
}

#[test]
fn exec_without_tool_breakdown_reports_gap_not_cause() {
    let tmp = tempfile::TempDir::new().unwrap();
    let epoch = ClockEpoch::new();
    // Slow exec span, NO tool spans anywhere in the session.
    let rows = completed_span("sp-exec", ident("executor"), SpanKind::AgentExec, &epoch);
    write_rows(&tmp, &rows);

    let report = analyze(tmp.path());
    let cand = report
        .bottlenecks
        .iter()
        .find(|b| b.span_id.as_deref() == Some("sp-exec"))
        .expect("exec span should be a candidate");
    // No tool evidence exists → the candidate must expose a gap and
    // must NOT emit an optimization recommendation (R8).
    assert!(cand.gap.is_some());
    assert!(cand.recommendation.is_none());
}

#[test]
fn wait_span_yields_wait_release_candidate() {
    let tmp = tempfile::TempDir::new().unwrap();
    let epoch = ClockEpoch::new();
    let rows = completed_span("sp-wait", ident("executor"), SpanKind::CapacityWait, &epoch);
    write_rows(&tmp, &rows);

    let report = analyze(tmp.path());
    let cand = report
        .bottlenecks
        .iter()
        .find(|b| b.span_id.as_deref() == Some("sp-wait"))
        .expect("wait span should be a candidate");
    assert_eq!(cand.kind, CandidateKind::WaitRelease);
    assert!(
        cand.recommendation
            .as_deref()
            .unwrap_or("")
            .contains("scheduling")
    );
}

#[test]
fn not_collected_capture_surfaces_gap_candidate() {
    let tmp = tempfile::TempDir::new().unwrap();
    let report = analyze(tmp.path());
    assert!(
        report
            .bottlenecks
            .iter()
            .any(|b| b.kind == CandidateKind::CoverageGap && b.gap.is_some())
    );
}
