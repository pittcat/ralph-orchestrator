//! Evidence references for performance findings (plan 2026-10-29-0104
//! U5, R7/R8).
//!
//! An [`EvidenceRef`] points at a concrete artifact backing a
//! bottleneck claim: a `timing.jsonl` span id, the session manifest,
//! or the causal report. A candidate that cannot name its evidence
//! must instead carry an explicit `gap` string — missing data never
//! produces a confident root cause.

use serde::Serialize;

use super::{PerformanceReport, SpanView};
use crate::diagnostics::TIMING_SIDECAR_FILENAME;

/// Artifact kinds evidence can point at. Free-form strings would let
/// candidates fabricate provenance, so the space is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceArtifact {
    /// `timing.jsonl` span rows (optionally narrowed to `span_id`).
    TimingSidecar,
    /// The session manifest (`diagnosis-input.json`) — coverage
    /// claims such as "capture was degraded".
    SessionManifest,
    /// The causal-attribution report — a reference link, never a
    /// merged conclusion (the causal report keeps its own domain).
    CausalReport,
}

/// One evidence pointer attached to a bottleneck candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceRef {
    pub artifact: EvidenceArtifact,
    /// Span id within `timing.jsonl` when the evidence is a span.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    /// What the artifact shows for this candidate.
    pub note: String,
}

impl EvidenceRef {
    /// Reference a merged span row in `timing.jsonl`.
    pub fn span(span: &SpanView, note: impl Into<String>) -> Self {
        Self {
            artifact: EvidenceArtifact::TimingSidecar,
            span_id: Some(span.span_id.clone()),
            note: note.into(),
        }
    }

    /// Reference the whole timing sidecar (aggregate facts).
    pub fn sidecar(note: impl Into<String>) -> Self {
        Self {
            artifact: EvidenceArtifact::TimingSidecar,
            span_id: None,
            note: note.into(),
        }
    }

    /// Reference the session manifest (coverage status).
    pub fn manifest(note: impl Into<String>) -> Self {
        Self {
            artifact: EvidenceArtifact::SessionManifest,
            span_id: None,
            note: note.into(),
        }
    }

    /// Cross-reference the causal report without absorbing its
    /// semantics — the causal domain stays authoritative for
    /// attribution, the performance report only *points* at it.
    pub fn causal(note: impl Into<String>) -> Self {
        Self {
            artifact: EvidenceArtifact::CausalReport,
            span_id: None,
            note: note.into(),
        }
    }
}

/// Whether the report has tool-level observations under a span —
/// a `ToolExec` span is "under" `parent_span_id` when it declares it
/// as parent, or when its `observed_by == stream` attribute marks it
/// as adapter-captured for the same activation (the adapter-side
/// parent id is the runner's exec span).
pub fn tool_breakdown_exists(report: &PerformanceReport, parent_span_id: &str) -> bool {
    report.spans.iter().any(|s| {
        s.kind == crate::diagnostics::SpanKind::ToolExec
            && s.parent_span_id.as_deref() == Some(parent_span_id)
    })
}

/// Whether any `ToolExec` span exists at all — distinguishes "the
/// backend never gave us tool boundaries" from "this exec span just
/// had no children recorded".
pub fn any_tool_observations(report: &PerformanceReport) -> bool {
    report
        .spans
        .iter()
        .any(|s| s.kind == crate::diagnostics::SpanKind::ToolExec)
}

/// Display name of the timing sidecar for human-readable notes.
pub fn sidecar_name() -> &'static str {
    TIMING_SIDECAR_FILENAME
}
