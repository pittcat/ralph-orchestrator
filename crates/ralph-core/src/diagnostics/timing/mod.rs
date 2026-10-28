//! Timing capture sidecar (`timing.jsonl`) — plan 2026-10-29-0104 U1.
//!
//! Records structured execution intervals (waits, startup, agent/tool
//! execution, review, integration) with a correlation identity so the
//! offline analysis layer (`crate::diagnosis::performance`) can answer
//! "where did the time go" without guessing.
//!
//! # Lifecycle
//!
//! - [`TimingSpan::open`] builds an `open` row; the collector writes it
//!   immediately so a crashed process still leaves a censored record.
//! - [`TimingSpanHandle::complete`] / [`TimingSpanHandle::interrupt`]
//!   write the closing row with `ts_end_utc` + `mono_end_ms`.
//! - Dropping a handle without an explicit close records `interrupted`
//!   (same-process early return); a crashed process leaves the `open`
//!   row alone — readers never fabricate an end for it (KTD3).

pub mod model;
#[cfg(test)]
mod tests;
pub mod writer;

pub use model::{
    ClockEpoch, ExecutionIdentity, IdentityValue, ObservationPrecision,
    PERFORMANCE_ANALYSIS_SCHEMA_VERSION, SpanKind, SpanStatus, TimingSpan,
    TIMING_SCHEMA_VERSION, TIMING_SIDECAR_FILENAME,
};
pub use writer::TimingWriter;

/// In-flight timing span bound to a [`crate::diagnostics::DiagnosticsCollector`].
///
/// Created by `DiagnosticsCollector::begin_timing_span`, which already
/// persisted the `open` row. The handle owns the span record until one
/// of the closing methods consumes it.
pub struct TimingSpanHandle {
    span: Option<TimingSpan>,
    collector: crate::diagnostics::DiagnosticsCollector,
    closed: bool,
}

impl TimingSpanHandle {
    pub(crate) fn new(
        span: TimingSpan,
        collector: crate::diagnostics::DiagnosticsCollector,
    ) -> Self {
        Self {
            span: Some(span),
            collector,
            closed: false,
        }
    }

    /// Span id for linking children / log lines.
    pub fn span_id(&self) -> &str {
        self.span.as_ref().map(|s| s.span_id.as_str()).unwrap_or("")
    }

    /// Attach kind-specific attributes (bounded JSON). Only effective
    /// before the closing row is written.
    pub fn set_attributes(&mut self, attributes: serde_json::Value) {
        if let Some(span) = self.span.as_mut() {
            span.attributes = Some(attributes);
        }
    }

    /// Add a non-parent link to another span.
    pub fn add_link(&mut self, span_id: impl Into<String>) {
        if let Some(span) = self.span.as_mut() {
            span.links.push(span_id.into());
        }
    }

    /// Attach a source reference to the raw artifact backing this span.
    pub fn set_source_ref(&mut self, source_ref: impl Into<String>) {
        if let Some(span) = self.span.as_mut() {
            span.source_ref = Some(source_ref.into());
        }
    }

    /// Close the span as `completed` and persist the closing row.
    pub fn complete(mut self) {
        self.finish(SpanStatus::Completed);
    }

    /// Close the span as `interrupted` (attempt cut short, cancel).
    pub fn interrupt(mut self) {
        self.finish(SpanStatus::Interrupted);
    }

    /// Close the span as `invalid` — recorded for coverage but
    /// excluded from duration math by readers.
    pub fn invalidate(mut self) {
        self.finish(SpanStatus::Invalid);
    }

    fn finish(&mut self, status: SpanStatus) {
        let Some(mut span) = self.span.take() else {
            return;
        };
        let epoch = self.collector.clock_epoch();
        match status {
            SpanStatus::Completed => span.complete(epoch),
            SpanStatus::Interrupted => span.interrupt(epoch),
            other => {
                span.status = other;
                span.ts_end_utc = Some(chrono::Utc::now().to_rfc3339());
                span.mono_end_ms = Some(epoch.mono_ms());
            }
        }
        self.collector.record_timing_span(span);
        self.closed = true;
    }
}

impl Drop for TimingSpanHandle {
    /// A handle dropped without an explicit close is recorded as
    /// `interrupted` — same-process early returns get an honest end,
    /// while a crashed process leaves the `open` row censored.
    fn drop(&mut self) {
        if !self.closed {
            self.finish(SpanStatus::Interrupted);
        }
    }
}
