//! Evidence-based bottleneck analysis (plan 2026-10-29-0104 U5, R7/R8).
//!
//! Turns the deterministic [`PerformanceReport`] facts into candidate
//! optimization directions. Every candidate obeys a hard contract:
//!
//! - it names concrete [`EvidenceRef`]s, OR
//! - it carries an explicit `gap` explaining which observation is
//!   missing — a candidate never asserts a cause it cannot show.
//!
//! Candidates are *directions*, not verified root causes; the causal
//! report owns attribution. Recommendations state their assumption
//! inline ("assumes X") so time savings are never fabricated by
//! summing overlapping intervals.

use serde::Serialize;

use super::evidence::{EvidenceRef, any_tool_observations, tool_breakdown_exists};
use super::{PerformanceReport, SpanView};
use crate::diagnostics::{SpanKind, TimingCaptureStatus};

/// Candidate bottleneck categories (closed set — the report renders
/// kind-based colors from these).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// A completed work span that dominates the wall clock.
    SlowStage,
    /// A wait span (dependency/capacity) holding ready work back —
    /// the scheduling candidate when idle capacity existed.
    WaitRelease,
    /// Capture gap: the report itself cannot answer a question the
    /// operator asked because the observation is missing.
    CoverageGap,
}

/// One bottleneck candidate with its evidence or explicit gap.
#[derive(Debug, Clone, Serialize)]
pub struct BottleneckCandidate {
    pub kind: CandidateKind,
    /// Span this candidate is about (`None` for coverage-only gaps).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    /// One-line factual finding, e.g. "tool_exec t-9 took 42% of wall clock".
    pub finding: String,
    /// Share of the wall-clock window (percent, integer).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall_clock_share_pct: Option<u8>,
    pub evidence: Vec<EvidenceRef>,
    /// Optimization direction with its assumption stated inline.
    /// `None` when the evidence does not support any direction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommendation: Option<String>,
    /// Explicit missing-observation note. Present exactly when the
    /// candidate cannot back a claim with evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap: Option<String>,
}

/// Minimum share of wall clock before a span becomes a candidate —
/// below this, a "slow" span is noise for optimization purposes.
const MIN_SHARE_PCT: u64 = 10;
/// Minimum absolute duration; tiny spans are never candidates.
const MIN_DURATION_MS: u64 = 500;

/// Derive bottleneck candidates from the report facts. Deterministic:
/// same report → same candidates, same order (evidence order stable).
pub fn analyze_bottlenecks(report: &PerformanceReport) -> Vec<BottleneckCandidate> {
    let mut out = Vec::new();

    // 1. Coverage gaps first — if capture itself is incomplete, the
    //    headline fact is "we cannot fully answer", before any
    //    span-level claim.
    if report.capture.status != TimingCaptureStatus::Collected {
        out.push(BottleneckCandidate {
            kind: CandidateKind::CoverageGap,
            span_id: None,
            finding: format!(
                "timing capture status is {:?}; span data is incomplete",
                report.capture.status
            ),
            wall_clock_share_pct: None,
            evidence: vec![EvidenceRef::manifest(
                "manifest timing_capture field reports the capture state",
            )],
            recommendation: Some(
                "enable diagnostics timing capture for the next run; assumes the missing \
                 intervals cover the stages this report cannot see"
                    .to_string(),
            ),
            gap: Some("timing sidecar not collected or degraded".to_string()),
        });
    }
    if report.capture.censored_spans > 0 {
        out.push(BottleneckCandidate {
            kind: CandidateKind::CoverageGap,
            span_id: None,
            finding: format!(
                "{} span(s) have no closing record; their durations are unknown",
                report.capture.censored_spans
            ),
            wall_clock_share_pct: None,
            evidence: vec![EvidenceRef::sidecar(
                "censored open rows in timing.jsonl (process exited or span dropped mid-flight)",
            )],
            recommendation: Some(
                "treat affected stage totals as lower bounds; assumes the process exited \
                 normally rather than spans being dropped"
                    .to_string(),
            ),
            gap: Some("censored intervals excluded from duration facts".to_string()),
        });
    }

    // 2. Span-level candidates ordered by duration.
    let wall = report.wall_clock_ms.unwrap_or(0);
    for span in &report.slowest_spans {
        let Some(dur) = span.duration_ms else {
            continue;
        };
        if dur < MIN_DURATION_MS {
            continue;
        }
        let share = dur.saturating_mul(100).checked_div(wall).unwrap_or(0);
        if share < MIN_SHARE_PCT {
            continue;
        }
        out.push(span_candidate(report, span, dur, share));
        if out.len() >= 8 {
            break; // bounded: headline candidates only
        }
    }
    out
}

fn span_candidate(
    report: &PerformanceReport,
    span: &SpanView,
    dur: u64,
    share: u64,
) -> BottleneckCandidate {
    let share_u8 = u8::try_from(share).unwrap_or(u8::MAX);
    let who = span
        .identity
        .hat
        .as_deref()
        .or(span.identity.loop_id.as_deref())
        .unwrap_or("unknown-scope");

    match span.kind {
        SpanKind::DependencyWait | SpanKind::CapacityWait => BottleneckCandidate {
            kind: CandidateKind::WaitRelease,
            span_id: Some(span.span_id.clone()),
            finding: format!(
                "{} waited {}ms ({}% of wall clock) in {}",
                span.kind.as_str(),
                dur,
                share,
                who
            ),
            wall_clock_share_pct: Some(share_u8),
            evidence: vec![
                EvidenceRef::span(span, "wait interval bounded by open/close rows"),
                EvidenceRef::sidecar("work spans completing concurrently while this wait was open"),
            ],
            recommendation: Some(
                "scheduling candidate: revisit admission/dependency release ordering; \
                 assumes capacity was idle while this job waited (verify against the \
                 admission ledger)"
                    .to_string(),
            ),
            gap: None,
        },
        SpanKind::AgentExec => {
            let has_tools = tool_breakdown_exists(report, &span.span_id);
            let tools_seen = any_tool_observations(report);
            let (evidence, recommendation, gap) = if has_tools {
                (
                    vec![
                        EvidenceRef::span(span, "exec interval"),
                        EvidenceRef::sidecar("tool_exec children bound to this span"),
                    ],
                    Some(
                        "inspect the child tool_exec spans for the dominant tool; assumes \
                         tool time dominates model time inside this exec window"
                            .to_string(),
                    ),
                    None,
                )
            } else if tools_seen {
                (
                    vec![EvidenceRef::span(span, "exec interval")],
                    None,
                    Some(
                        "no tool_exec children under this exec span — cannot split model \
                         vs tool time here"
                            .to_string(),
                    ),
                )
            } else {
                (
                    vec![
                        EvidenceRef::span(span, "exec interval"),
                        EvidenceRef::manifest("no tool_exec observations in this session at all"),
                    ],
                    None,
                    Some(
                        "backend/tool timing was not captured for this session — a slow \
                         agent span stays unattributed (model vs tool unknown)"
                            .to_string(),
                    ),
                )
            };
            BottleneckCandidate {
                kind: CandidateKind::SlowStage,
                span_id: Some(span.span_id.clone()),
                finding: format!(
                    "agent execution took {}ms ({}% of wall clock) in {}",
                    dur, share, who
                ),
                wall_clock_share_pct: Some(share_u8),
                evidence,
                recommendation,
                gap,
            }
        }
        SpanKind::ToolExec => {
            let tool = span
                .attributes
                .as_ref()
                .and_then(|a| a.get("tool"))
                .and_then(|v| v.as_str())
                .unwrap_or("tool");
            BottleneckCandidate {
                kind: CandidateKind::SlowStage,
                span_id: Some(span.span_id.clone()),
                finding: format!("tool `{tool}` took {}ms ({}% of wall clock)", dur, share),
                wall_clock_share_pct: Some(share_u8),
                evidence: vec![EvidenceRef::span(
                    span,
                    "tool call→result interval observed by the stream handler",
                )],
                recommendation: Some(
                    "check whether this tool's input/output size or retry path is the \
                     driver; assumes the measured call→result interval is the tool's \
                     cost, not transport"
                        .to_string(),
                ),
                gap: None,
            }
        }
        other => BottleneckCandidate {
            kind: CandidateKind::SlowStage,
            span_id: Some(span.span_id.clone()),
            finding: format!(
                "{} stage took {}ms ({}% of wall clock) in {}",
                other.as_str(),
                dur,
                share,
                who
            ),
            wall_clock_share_pct: Some(share_u8),
            evidence: vec![EvidenceRef::span(span, "stage interval")],
            recommendation: Some(
                "inspect this stage's inputs for a structural driver (assume stage \
                 boundaries, not internal steps, dominate the interval)"
                    .to_string(),
            ),
            gap: None,
        },
    }
}
