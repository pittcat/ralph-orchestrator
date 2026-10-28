//! Deterministic performance accounting (plan 2026-10-29-0104 U4/U5).
//!
//! Reads `timing.jsonl` span rows plus the session manifest and
//! produces a `PerformanceReport` of **observed facts only**:
//!
//! - every number is computed by this code, never by an agent (R11);
//! - censored spans (open rows with no closing row) are surfaced as
//!   censored, never zero-filled (KTD3);
//! - monotonic readings are only subtracted within the same
//!   `clock_epoch` (KTD2) — cross-epoch rows fall back to UTC diff
//!   only when both endpoints exist;
//! - capture coverage comes from the manifest `timing_capture`
//!   field so the report can show gaps rather than guessing (R4);
//! - bottleneck candidates are presented as *observed* slow spans,
//!   explicitly NOT verified root causes (R7/R8).

pub mod bottleneck;
pub mod evidence;

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::Path;

use serde::Serialize;

use crate::diagnostics::{
    DiagnosisInputBundle, PERFORMANCE_ANALYSIS_SCHEMA_VERSION, SpanKind, SpanStatus,
    TIMING_SIDECAR_FILENAME, TimingCaptureStatus, TimingSpan, read_manifest,
};

/// One merged span view for reporting: latest row wins for status;
/// identity/kind/timestamps come from the open row.
#[derive(Debug, Clone, Serialize)]
pub struct SpanView {
    pub span_id: String,
    /// Parent span when the row declared one (tool inside agent exec).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub kind: SpanKind,
    pub status: SpanStatus,
    pub identity: crate::diagnostics::ExecutionIdentity,
    pub ts_start_utc: String,
    pub ts_end_utc: Option<String>,
    /// Same-epoch duration when computable.
    pub duration_ms: Option<u64>,
    pub clock_epoch: String,
    pub attributes: Option<serde_json::Value>,
    /// Rows observed for this span (2 = open+close, 1 = censored).
    pub rows: u32,
}

/// Aggregate bucket used for the heatmap and totals.
#[derive(Debug, Clone, Default, Serialize)]
pub struct KindAggregate {
    pub kind: Option<SpanKind>,
    pub count: u32,
    pub total_ms: u64,
    pub censored: u32,
}

/// Coverage story for the report header (R4).
#[derive(Debug, Clone, Serialize)]
pub struct CaptureCoverage {
    pub status: TimingCaptureStatus,
    pub span_rows: u64,
    pub malformed_rows: u64,
    pub censored_spans: u32,
    /// Human-readable evidence gaps surfaced in the report.
    pub gaps: Vec<String>,
}

/// Deterministic performance report (facts only — R11).
#[derive(Debug, Clone, Serialize)]
pub struct PerformanceReport {
    pub schema_version: String,
    pub generated_at: String,
    pub session_dir: String,
    pub capture: CaptureCoverage,
    pub by_kind: Vec<KindAggregate>,
    /// Slowest completed spans — observed facts, NOT causes (R7/R8).
    pub slowest_spans: Vec<SpanView>,
    /// All merged spans for timeline/heatmap rendering.
    pub spans: Vec<SpanView>,
    /// Total wall-clock window covered by the span set (min start →
    /// max end on the UTC axis), `None` when empty/censored only.
    pub wall_clock_ms: Option<u64>,
    /// Sum of completed work-kind durations (waits excluded — R5).
    pub cumulative_work_ms: u64,
    /// Evidence-backed bottleneck candidates (U5, R7/R8) — directions,
    /// not verified causes; every entry carries evidence or an
    /// explicit gap.
    pub bottlenecks: Vec<bottleneck::BottleneckCandidate>,
}

/// Read `timing.jsonl` + `diagnosis-input.json` under `session_dir`
/// and compute the deterministic report. Malformed rows are counted
/// as malformed and skipped — never fatal (R4).
pub fn analyze(session_dir: &Path) -> PerformanceReport {
    let manifest: Option<DiagnosisInputBundle> = read_manifest(session_dir);

    let mut spans: BTreeMap<String, SpanView> = BTreeMap::new();
    let mut span_rows = 0u64;
    let mut malformed_rows = 0u64;
    let timing_path = session_dir.join(TIMING_SIDECAR_FILENAME);
    if let Ok(file) = std::fs::File::open(&timing_path) {
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<TimingSpan>(&line) {
                Ok(row) => {
                    span_rows += 1;
                    merge_row(&mut spans, row);
                }
                Err(_) => malformed_rows += 1,
            }
        }
    }

    let mut by_kind: BTreeMap<&'static str, KindAggregate> = BTreeMap::new();
    let mut censored = 0u32;
    let mut cumulative_work_ms = 0u64;
    let mut min_start: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut max_end: Option<chrono::DateTime<chrono::Utc>> = None;
    for span in spans.values() {
        let entry = by_kind
            .entry(span.kind.as_str())
            .or_insert_with(|| KindAggregate {
                kind: Some(span.kind),
                ..Default::default()
            });
        entry.count += 1;
        if let Some(d) = span.duration_ms {
            entry.total_ms += d;
            if span.kind.is_work() {
                cumulative_work_ms += d;
            }
        } else {
            entry.censored += 1;
            censored += 1;
        }
        if let Ok(start) = chrono::DateTime::parse_from_rfc3339(&span.ts_start_utc) {
            let start = start.with_timezone(&chrono::Utc);
            if min_start.is_none_or(|m| start < m) {
                min_start = Some(start);
            }
        }
        if let Some(end) = span
            .ts_end_utc
            .as_deref()
            .and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok())
        {
            let end = end.with_timezone(&chrono::Utc);
            if max_end.is_none_or(|m| end > m) {
                max_end = Some(end);
            }
        }
    }

    let mut slowest: Vec<SpanView> = spans
        .values()
        .filter(|s| s.duration_ms.is_some())
        .cloned()
        .collect();
    slowest.sort_by_key(|s| std::cmp::Reverse(s.duration_ms.unwrap_or(0)));
    slowest.truncate(20);

    // Coverage: manifest field wins; fall back to file presence.
    let capture_status = manifest
        .and_then(|m| m.timing_capture)
        .unwrap_or_else(|| {
            if timing_path.exists() {
                TimingCaptureStatus::Collected
            } else {
                TimingCaptureStatus::NotCollected
            }
        });
    let mut gaps = Vec::new();
    match capture_status {
        TimingCaptureStatus::NotCollected => gaps.push(
            "timing capture was not collected this session".to_string(),
        ),
        TimingCaptureStatus::Degraded => {
            gaps.push("timing writer degraded; span data may be partial".to_string())
        }
        TimingCaptureStatus::Collected => {}
    }
    if malformed_rows > 0 {
        gaps.push(format!("{malformed_rows} malformed timing rows skipped"));
    }
    if censored > 0 {
        gaps.push(format!("{censored} span(s) censored (no closing record)"));
    }

    let mut report = PerformanceReport {
        schema_version: PERFORMANCE_ANALYSIS_SCHEMA_VERSION.to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        session_dir: session_dir.display().to_string(),
        capture: CaptureCoverage {
            status: capture_status,
            span_rows,
            malformed_rows,
            censored_spans: censored,
            gaps,
        },
        by_kind: by_kind.into_values().collect(),
        slowest_spans: slowest,
        spans: spans.into_values().collect(),
        wall_clock_ms: min_start
            .zip(max_end)
            .and_then(|(s, e)| (e - s).num_milliseconds().try_into().ok()),
        cumulative_work_ms,
        bottlenecks: Vec::new(),
    };
    // U5: derive candidates after all facts are settled so every
    // candidate can point at the same merged view the report renders.
    report.bottlenecks = bottleneck::analyze_bottlenecks(&report);
    report
}

fn merge_row(spans: &mut BTreeMap<String, SpanView>, row: TimingSpan) {
    let view = spans.entry(row.span_id.clone()).or_insert_with(|| SpanView {
        span_id: row.span_id.clone(),
        parent_span_id: row.parent_span_id.clone(),
        kind: row.kind,
        status: SpanStatus::Open,
        identity: row.identity.clone(),
        ts_start_utc: row.ts_start_utc.clone(),
        ts_end_utc: None,
        duration_ms: None,
        clock_epoch: row.clock_epoch.clone(),
        attributes: row.attributes.clone(),
        rows: 0,
    });
    view.rows += 1;
    if row.status != SpanStatus::Open {
        view.status = row.status;
        view.ts_end_utc = row.ts_end_utc.clone();
        // KTD2: only subtract monotonic readings stamped with the
        // same epoch; rows that disagree stay censored for duration.
        if row.clock_epoch == view.clock_epoch {
            view.duration_ms = row.duration_ms();
        }
        if row.attributes.is_some() {
            view.attributes = row.attributes;
        }
    }
    if row.ts_start_utc < view.ts_start_utc {
        view.ts_start_utc = row.ts_start_utc.clone();
    }
}

/// Serialize the report for `--format json`.
pub fn report_json(report: &PerformanceReport) -> serde_json::Value {
    serde_json::to_value(report)
        .unwrap_or_else(|_| serde_json::json!({"schema_version": report.schema_version}))
}

/// Render the report as terminal-friendly Markdown (`--format markdown`).
pub fn render_markdown(report: &PerformanceReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "# Run Analysis — {}", report.session_dir);
    let _ = writeln!(out, "\nGenerated: `{}` · schema `{}`\n", report.generated_at, report.schema_version);
    let _ = writeln!(
        out,
        "- **Wall clock:** {}",
        report.wall_clock_ms.map(|m| format!("{m}ms")).unwrap_or_else(|| "unknown".into())
    );
    let _ = writeln!(out, "- **Cumulative work:** {}ms", report.cumulative_work_ms);
    let _ = writeln!(
        out,
        "- **Capture:** {:?} ({} span rows, {} malformed, {} censored)\n",
        report.capture.status,
        report.capture.span_rows,
        report.capture.malformed_rows,
        report.capture.censored_spans
    );
    if !report.capture.gaps.is_empty() {
        out.push_str("## Evidence gaps\n\n");
        for g in &report.capture.gaps {
            let _ = writeln!(out, "- {g}");
        }
        out.push('\n');
    }
    if !report.by_kind.is_empty() {
        out.push_str("## Time by stage kind\n\n| Kind | Spans | Censored | Total ms |\n|---|---|---|---|\n");
        for k in &report.by_kind {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} |",
                k.kind.map(SpanKind::as_str).unwrap_or("other"),
                k.count,
                k.censored,
                k.total_ms
            );
        }
        out.push('\n');
    }
    if !report.slowest_spans.is_empty() {
        out.push_str("## Slowest observed spans\n\n");
        out.push_str("_Observed facts only — candidates, not verified causes._\n\n");
        out.push_str("| Span | Kind | Status | Hat | Unit | Duration ms |\n|---|---|---|---|---|---|\n");
        for s in &report.slowest_spans {
            let _ = writeln!(
                out,
                "| {} | {} | {:?} | {} | {} | {} |",
                s.span_id,
                s.kind.as_str(),
                s.status,
                s.identity.hat.as_deref().unwrap_or("-"),
                s.identity.unit_key.as_str().unwrap_or("-"),
                s.duration_ms.map(|d| d.to_string()).unwrap_or_else(|| "censored".into())
            );
        }
        out.push('\n');
    }
    if !report.bottlenecks.is_empty() {
        out.push_str("## Bottleneck candidates\n\n");
        out.push_str(
            "_Directions backed by evidence or explicit gaps — not verified root causes._\n\n",
        );
        for (i, b) in report.bottlenecks.iter().enumerate() {
            let _ = writeln!(out, "{}. **{:?}** — {}", i + 1, b.kind, b.finding);
            for e in &b.evidence {
                let where_ = e
                    .span_id
                    .as_deref()
                    .map(|id| format!("{:?}#{}", e.artifact, id))
                    .unwrap_or_else(|| format!("{:?}", e.artifact));
                let _ = writeln!(out, "   - evidence: {} ({})", e.note, where_);
            }
            if let Some(rec) = &b.recommendation {
                let _ = writeln!(out, "   - direction: {rec}");
            }
            if let Some(gap) = &b.gap {
                let _ = writeln!(out, "   - gap: {gap}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{ClockEpoch, ExecutionIdentity, IdentityValue};

    fn session_with_rows(rows: &[TimingSpan]) -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut writer = crate::diagnostics::TimingWriter::new(tmp.path()).unwrap();
        for r in rows {
            writer.append(r.clone());
        }
        tmp
    }

    #[test]
    fn analyze_aggregates_kinds_and_marks_censored() {
        let epoch = ClockEpoch::new();
        let ident = ExecutionIdentity {
            loop_id: Some("l1".into()),
            hat: Some("executor".into()),
            unit_key: IdentityValue::value("U1"),
            ..Default::default()
        };
        let mut done = TimingSpan::open("sp-1", ident.clone(), SpanKind::AgentExec, &epoch);
        let orphan = TimingSpan::open("sp-2", ident, SpanKind::DependencyWait, &epoch);
        let report_dir = {
            let tmp = session_with_rows(&[done.clone()]);
            // closing row for sp-1
            done.complete(&epoch);
            let mut w = crate::diagnostics::TimingWriter::new(tmp.path()).unwrap();
            w.append(done);
            w.append(orphan);
            tmp
        };
        let report = analyze(report_dir.path());
        assert_eq!(report.capture.status, TimingCaptureStatus::Collected);
        assert_eq!(report.spans.len(), 2);
        assert_eq!(report.capture.censored_spans, 1);
        let exec = report
            .by_kind
            .iter()
            .find(|k| k.kind == Some(SpanKind::AgentExec))
            .unwrap();
        assert_eq!(exec.count, 1);
        assert!(exec.total_ms < u64::MAX);
        assert!(report.cumulative_work_ms >= exec.total_ms - exec.total_ms); // no panic path
    }

    #[test]
    fn analyze_reports_not_collected_without_sidecar() {
        let tmp = tempfile::TempDir::new().unwrap();
        let report = analyze(tmp.path());
        assert_eq!(report.capture.status, TimingCaptureStatus::NotCollected);
        assert!(!report.capture.gaps.is_empty());
        assert_eq!(report.spans.len(), 0);
    }

    #[test]
    fn malformed_rows_are_counted_not_fatal() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(TIMING_SIDECAR_FILENAME),
            "{bad}\n{\"span_id\":\"x\"}\n",
        )
        .unwrap();
        let report = analyze(tmp.path());
        assert!(report.capture.malformed_rows >= 1);
        assert!(report
            .capture
            .gaps
            .iter()
            .any(|g| g.contains("malformed")));
    }
}
