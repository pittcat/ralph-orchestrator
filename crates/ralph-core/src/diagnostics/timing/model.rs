//! Timing capture data model (plan 2026-10-29-0104 U1 / KTD2-KTD3).
//!
//! Defines the correlation identity (`ExecutionIdentity`), the
//! interval record (`TimingSpan`), and the per-process clock epoch
//! (`ClockEpoch`) that every `timing.jsonl` row is built from.
//!
//! # Clock epochs (KTD2)
//!
//! Same-process durations use a monotonic clock anchored at a
//! per-process [`ClockEpoch`]. Cross-process ordering uses UTC
//! timestamps. A restarted process creates a new epoch, so monotonic
//! readings from different epochs MUST never be subtracted — the
//! accounting layer enforces this by comparing `clock_epoch` ids.
//!
//! # Interval states (KTD3)
//!
//! A span is written once when it opens (`status=open`, no end) and
//! again when it closes (`completed` / `interrupted` / `invalid`
//! with `ts_end_utc` + `mono_end_ms`). A row that still lacks an end
//! when the session ends is **censored**: readers surface it as an
//! open interval, never back-fill a zero duration.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Schema version for `timing.jsonl`. Bump only on non-additive changes.
pub const TIMING_SCHEMA_VERSION: &str = "run-analysis-timing/v1";

/// Sidecar filename inside the diagnostics session directory.
pub const TIMING_SIDECAR_FILENAME: &str = "timing.jsonl";

/// Version stamped onto the performance-analysis JSON document that
/// `ralph diagnose --performance` emits (the analysis output is
/// versioned independently of the capture schema so the offline
/// pipeline can evolve without forcing re-capture).
pub const PERFORMANCE_ANALYSIS_SCHEMA_VERSION: &str = "run-analysis-performance/v1";

/// Process-wide span id counter. Combined with the clock epoch this
/// makes span ids unique per process without coordination.
static SPAN_SEQ: AtomicU64 = AtomicU64::new(0);

/// Three-state identity value (R1): distinguishes "not applicable"
/// from "missing/unknown" so the report can tell a non-DAG run
/// (`not_applicable`) apart from a DAG run whose task key was not
/// captured (`missing`).
///
/// Wire format: absent field = `Missing`; `"n/a"` = `NotApplicable`;
/// any other string = `Value`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum IdentityValue {
    /// Field was never emitted (writer did not know the value).
    #[default]
    Missing,
    /// The execution shape makes the field inapplicable (e.g.
    /// `plan_key` on a coordinator-mode hat activation).
    NotApplicable,
    /// Captured value.
    Value(String),
}

impl IdentityValue {
    /// `true` when the field should be omitted from the serialized row.
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    /// Convenience constructor for a present value. Empty strings are
    /// normalized to `Missing` so a blank never masquerades as data.
    pub fn value(s: impl Into<String>) -> Self {
        let s = s.into();
        if s.is_empty() {
            Self::Missing
        } else {
            Self::Value(s)
        }
    }

    /// Borrow the captured string, if any.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Value(s) => Some(s),
            _ => None,
        }
    }
}

impl Serialize for IdentityValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Missing => serializer.serialize_none(),
            Self::NotApplicable => serializer.serialize_str("n/a"),
            Self::Value(s) => serializer.serialize_str(s),
        }
    }
}

impl<'de> Deserialize<'de> for IdentityValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let opt = Option::<String>::deserialize(deserializer)?;
        Ok(match opt {
            None => Self::Missing,
            Some(s) if s == "n/a" => Self::NotApplicable,
            Some(s) => Self::Value(s),
        })
    }
}

/// Per-process clock epoch (KTD2).
///
/// `origin` is an [`Instant`] captured at epoch creation; all
/// `mono_*_ms` readings on spans stamped with this epoch id are
/// relative to that origin and are only comparable within the same
/// epoch. `origin_utc` anchors the epoch on the wall clock so the
/// reader can still order spans across epochs.
#[derive(Debug, Clone)]
pub struct ClockEpoch {
    id: String,
    origin: Instant,
    origin_utc: String,
}

impl ClockEpoch {
    /// Create a new epoch for the current process. The id embeds the
    /// pid plus a nanosecond timestamp so a restarted process never
    /// reuses an epoch id.
    pub fn new() -> Self {
        let now = chrono::Utc::now();
        let nanos = now.timestamp_nanos_opt().unwrap_or_default();
        Self {
            id: format!("epoch-{}-{:x}", std::process::id(), nanos),
            origin: Instant::now(),
            origin_utc: now.to_rfc3339(),
        }
    }

    /// Epoch identifier stamped onto every span as `clock_epoch`.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// RFC 3339 UTC timestamp captured at epoch creation.
    pub fn origin_utc(&self) -> &str {
        &self.origin_utc
    }

    /// Milliseconds elapsed since the epoch origin on the process
    /// monotonic clock.
    pub fn mono_ms(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }

    /// Generate a unique span id within this epoch.
    pub fn next_span_id(&self) -> String {
        let seq = SPAN_SEQ.fetch_add(1, Ordering::Relaxed);
        format!("sp-{}-{:06}", &self.id[6..], seq)
    }
}

impl Default for ClockEpoch {
    fn default() -> Self {
        Self::new()
    }
}

/// Correlation identity stamped onto every span (R1).
///
/// `loop_id`/`session_id`/`activation_id`/`job_id`/`attempt`/`stage`/
/// `hat` are plain `Option`s — `None` means "not captured". The
/// plan/unit/task triple uses [`IdentityValue`] so "not applicable"
/// (non-DAG execution) is distinguishable from "applicable but not
/// captured".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Activation instance id — one per hat activation (trigger →
    /// backend completion). Parallel activations of the same hat get
    /// distinct ids (R2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// 1-based attempt index for this job/activation (R2). Retries and
    /// restarts bump the attempt so records never blend identities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// Pipeline stage label (e.g. `execute`, `integrate`, `verify`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// Hat id when the span is bound to a hat activation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hat: Option<String>,
    #[serde(default, skip_serializing_if = "IdentityValue::is_missing")]
    pub plan_key: IdentityValue,
    #[serde(default, skip_serializing_if = "IdentityValue::is_missing")]
    pub unit_key: IdentityValue,
    #[serde(default, skip_serializing_if = "IdentityValue::is_missing")]
    pub task_key: IdentityValue,
}

impl ExecutionIdentity {
    /// Shorthand for a hat-scoped activation identity.
    pub fn for_hat(hat: impl Into<String>) -> Self {
        Self {
            hat: Some(hat.into()),
            ..Self::default()
        }
    }
}

/// Boundary category a span measures (R3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// Ready but blocked on a dependency (event/handoff) resolution.
    DependencyWait,
    /// Ready but blocked on a capacity/admission slot.
    CapacityWait,
    /// Backend process startup (spawn → first observable output).
    Startup,
    /// Agent execution interval (spawn → process exit/session result).
    AgentExec,
    /// Single tool invocation (call → result).
    ToolExec,
    /// Review/verify work performed by a downstream hat.
    ReviewVerify,
    /// Integration / merge work on the runtime side.
    Integration,
    /// Memory selection pass (point or short interval).
    MemorySelection,
    /// Anything that does not fit the categories above.
    Other,
}

impl SpanKind {
    /// Stable snake_case label (matches serde rename).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DependencyWait => "dependency_wait",
            Self::CapacityWait => "capacity_wait",
            Self::Startup => "startup",
            Self::AgentExec => "agent_exec",
            Self::ToolExec => "tool_exec",
            Self::ReviewVerify => "review_verify",
            Self::Integration => "integration",
            Self::MemorySelection => "memory_selection",
            Self::Other => "other",
        }
    }

    /// Categories that count as "work" for cumulative-time accounting
    /// (waits are excluded — they measure blockage, not work).
    pub fn is_work(self) -> bool {
        !matches!(self, Self::DependencyWait | Self::CapacityWait)
    }
}

/// Lifecycle status of a span record (KTD3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    /// Span was opened but no closing row was observed (censored).
    #[default]
    Open,
    /// Span closed normally with an end timestamp.
    Completed,
    /// Span was cut short (interrupted attempt, cancellation).
    Interrupted,
    /// Producer flagged the record as not trustworthy; readers keep it
    /// for coverage accounting but exclude it from durations.
    Invalid,
}

/// Precision of the boundary observation (R3/AE4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationPrecision {
    /// Boundary was directly observed (exact timestamp).
    #[default]
    Exact,
    /// Boundary was inferred from a coarser signal (e.g. poll tick).
    Coarse,
    /// The backend did not expose this boundary — the row exists for
    /// coverage but carries no usable edge.
    Unavailable,
}

/// One `timing.jsonl` row (KTD3).
///
/// A span occupies two rows over its lifetime: the `open` row written
/// at `ts_start_utc` and the closing row that repeats the identity
/// with `status != open` plus `ts_end_utc` / `mono_end_ms`. Readers
/// merge rows by `span_id`; a `span_id` seen only with `open` is a
/// censored interval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimingSpan {
    pub schema_version: String,
    /// Unique span identifier within the process epoch.
    pub span_id: String,
    /// Parent span for containment accounting (tool inside agent exec).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    /// Non-parent links (e.g. the admission span that released this job).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    /// Correlation identity (R1).
    #[serde(default)]
    pub identity: ExecutionIdentity,
    /// Boundary category.
    pub kind: SpanKind,
    /// Lifecycle status.
    pub status: SpanStatus,
    /// UTC RFC 3339 start timestamp.
    pub ts_start_utc: String,
    /// UTC RFC 3339 end timestamp. `None` = censored / still open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts_end_utc: Option<String>,
    /// Monotonic start reading relative to `clock_epoch` (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono_start_ms: Option<u64>,
    /// Monotonic end reading relative to `clock_epoch` (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono_end_ms: Option<u64>,
    /// [`ClockEpoch::id`] of the process that produced this row.
    pub clock_epoch: String,
    /// Observation precision of the recorded boundaries.
    #[serde(default)]
    pub precision: ObservationPrecision,
    /// Bounded JSON object with kind-specific attributes (tool name,
    /// wait reason, backend label). Capped at write time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<serde_json::Value>,
    /// Short reference to the raw artifact backing this row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    /// Monotonic append sequence inside `timing.jsonl` (writer-owned).
    #[serde(default)]
    pub sequence: u64,
}

impl TimingSpan {
    /// Open a new span stamped with `epoch`'s clock identity.
    pub fn open(
        span_id: impl Into<String>,
        identity: ExecutionIdentity,
        kind: SpanKind,
        epoch: &ClockEpoch,
    ) -> Self {
        Self {
            schema_version: TIMING_SCHEMA_VERSION.to_string(),
            span_id: span_id.into(),
            parent_span_id: None,
            links: Vec::new(),
            identity,
            kind,
            status: SpanStatus::Open,
            ts_start_utc: chrono::Utc::now().to_rfc3339(),
            ts_end_utc: None,
            mono_start_ms: Some(epoch.mono_ms()),
            mono_end_ms: None,
            clock_epoch: epoch.id().to_string(),
            precision: ObservationPrecision::Exact,
            attributes: None,
            source_ref: None,
            sequence: 0,
        }
    }

    /// Mark this span completed at `epoch`'s current reading.
    pub fn complete(&mut self, epoch: &ClockEpoch) {
        self.status = SpanStatus::Completed;
        self.ts_end_utc = Some(chrono::Utc::now().to_rfc3339());
        self.mono_end_ms = Some(epoch.mono_ms());
    }

    /// Mark this span interrupted (attempt cut short, restart, cancel).
    pub fn interrupt(&mut self, epoch: &ClockEpoch) {
        self.status = SpanStatus::Interrupted;
        self.ts_end_utc = Some(chrono::Utc::now().to_rfc3339());
        self.mono_end_ms = Some(epoch.mono_ms());
    }

    /// Same-epoch duration in milliseconds. `None` when the span is
    /// censored, marked invalid, or the end precedes the start.
    pub fn duration_ms(&self) -> Option<u64> {
        if !matches!(self.status, SpanStatus::Completed | SpanStatus::Interrupted) {
            return None;
        }
        let start = self.mono_start_ms?;
        let end = self.mono_end_ms?;
        end.checked_sub(start)
    }

    // ---- builder-style field setters ----

    pub fn with_parent(mut self, parent_span_id: impl Into<String>) -> Self {
        self.parent_span_id = Some(parent_span_id.into());
        self
    }

    pub fn with_link(mut self, span_id: impl Into<String>) -> Self {
        self.links.push(span_id.into());
        self
    }

    pub fn with_precision(mut self, precision: ObservationPrecision) -> Self {
        self.precision = precision;
        self
    }

    pub fn with_attributes(mut self, attributes: serde_json::Value) -> Self {
        self.attributes = Some(attributes);
        self
    }

    pub fn with_source_ref(mut self, source_ref: impl Into<String>) -> Self {
        self.source_ref = Some(source_ref.into());
        self
    }
}
