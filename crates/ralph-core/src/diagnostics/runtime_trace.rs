//! Structured runtime trace writer for `runtime-trace.jsonl`.
//!
//! Plan 2026-08-12-001 Unit 2: emits a sidecar JSONL of lifecycle
//! facts (activation, batch, accepted, rejected, commit, watchdog
//! timeout, termination) without changing the bus/ledger order or
//! the event acceptance / rejection semantics. The writer is a thin
//! `BufWriter<File>` wrapper modeled on [`recovery::RecoveryLogger`]
//! and [`orchestration::OrchestrationLogger`]. The trace is
//! independent of `trace.jsonl` (which is owned by
//! [`crate::diagnostics::trace_layer::DiagnosticTraceLayer`]) so
//! the two writers do not race on the same file handle.
//!
//! # Activation
//!
//! The logger is created by [`crate::diagnostics::DiagnosticsCollector`]
//! when `full_diagnostics`, `runtime_diagnosis_artifacts`, or
//! `causal_evidence` is active. The collector wraps it in
//! `Arc<Mutex<RuntimeTraceLogger>>`.
//!
//! # Error handling
//!
//! `append()` is best-effort. A failure emits a `tracing::warn!` and
//! the in-memory state flips to `degraded`; the orchestration main
//! path is never affected.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Schema version for the runtime-trace file. Bump only on
/// non-additive changes.
pub const RUNTIME_TRACE_SCHEMA_VERSION: &str = "run-diagnosis-trace/v1";

/// Lifecycle phase the entry records. Stays small so consumers can
/// index quickly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTracePhase {
    Activation,
    Batch,
    Accepted,
    Rejected,
    Commit,
    /// Plan 2026-08-26-1104 Unit 2 (`Causal Identity + Contract
    /// Receipt`). Decision-side rows stamped with `kind=contract_receipt`
    /// (and, in later units, `policy_receipt` / `commit_receipt` /
    /// `recovery_receipt`) all live under this phase so consumers can
    /// pull the receipt stream with a single phase filter without
    /// scanning the lifecycle phases.
    Decision,
    WatchdogTimeout,
    Termination,
}

/// Stable per-iteration correlation identity stamped onto every
/// runtime-trace row by [`crate::diagnostics::DiagnosticsCollector`]
/// once the loop's identity has been resolved (U02, plan
/// 2026-08-26-1104). `loop_id` is the canonical loop identifier
/// produced by `loop_runner`; `iteration` is the 0-based loop
/// iteration counter that matches `RuntimeTraceEntry::iteration`
/// and `LoopState::iteration`. The pair is the join key that the
/// attribution engine (U8) uses to slice the receipt stream per
/// run, and the per-iteration coherence check the diagnostic
/// reader enforces (S2.1).
///
/// BTreeMap / canonical JSON ordering on the wire is the
/// collector's responsibility; this struct is plain serde so a
/// downstream reader can decode it without a custom deserializer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CausalContext {
    pub loop_id: String,
    pub iteration: u64,
}

/// Single JSONL row. The set of populated fields depends on
/// `phase`/`kind`; unused fields are serialized only when they
/// carry data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTraceEntry {
    pub schema_version: String,
    /// UTC RFC 3339 timestamp the event was observed.
    pub ts: String,
    /// Loop iteration counter (0-based).
    pub iteration: u64,
    /// Monotonic per-session sequence number. Existing rows are scanned when
    /// a session is reused so continuation runs do not restart at 1.
    pub sequence: u64,
    pub phase: RuntimeTracePhase,
    /// Stable event kind used by bundle consumers. It is kept separate from
    /// `phase` because one phase can contain several runtime facts.
    #[serde(default = "default_trace_kind")]
    pub kind: String,
    /// Hat id, if the phase is bound to a hat (e.g. activation,
    /// accepted/rejected). Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hat: Option<String>,
    /// Topic, for accepted/rejected entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Workspace-relative path or other short ref pointing at the
    /// underlying raw artifact (e.g. the event log, the recovery
    /// journal line).
    #[serde(default, skip_serializing)]
    pub source_ref: Option<String>,
    /// Fixed-shape reference field for report consumers. `source_ref` is
    /// retained as a compatibility alias for existing Rust callers.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Human-readable status string (`accepted`, `rejected:<code>`,
    /// `commit`, `watchdog_timeout`, `termination:<reason>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Plan 2026-08-26-1104 Unit 2: per-row correlation identity
    /// (`loop_id` + `iteration`) used by the attribution engine to
    /// slice the receipt stream per run and by readers to enforce
    /// per-iteration coherence. `None` when the collector has not yet
    /// been told the loop identity, or when a test deliberately omits
    /// the field. `skip_serializing_if = Option::is_none` keeps the
    /// on-disk shape backwards-compatible: pre-U02 reader code
    /// continues to parse rows that do not carry the field (S2.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causal: Option<CausalContext>,
    /// Bounded JSON object with extra context (e.g. attempt count,
    /// payload_violation kind, watchdog reason). Field count is
    /// small on purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Value>,
}

impl RuntimeTraceEntry {
    /// Build a new entry with the current UTC timestamp.
    pub fn new(iteration: u64, sequence: u64, phase: RuntimeTracePhase) -> Self {
        Self {
            schema_version: RUNTIME_TRACE_SCHEMA_VERSION.to_string(),
            ts: chrono::Utc::now().to_rfc3339(),
            iteration,
            sequence,
            phase,
            kind: phase_kind(phase).to_string(),
            hat: None,
            topic: None,
            source_ref: None,
            reference: None,
            status: None,
            causal: None,
            fields: None,
        }
    }

    /// Set the `hat` field. Builder-style; returns the modified entry.
    pub fn with_hat(mut self, hat: impl Into<String>) -> Self {
        self.hat = Some(hat.into());
        self
    }

    /// Override the `causal` correlation identity. The collector
    /// auto-stamps this when the caller omits it; this builder is
    /// for tests that want to pin the value without going through
    /// the collector (S2.1, S2.4).
    pub fn with_causal(mut self, causal: CausalContext) -> Self {
        self.causal = Some(causal);
        self
    }

    /// Set the `topic` field.
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// Set the `source_ref` field.
    pub fn with_source_ref(mut self, source_ref: impl Into<String>) -> Self {
        let source_ref = source_ref.into();
        self.source_ref = Some(source_ref.clone());
        self.reference = Some(source_ref);
        self
    }

    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = kind.into();
        self
    }

    /// Set the `status` field.
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    /// Set the `fields` JSON object.
    pub fn with_fields(mut self, fields: serde_json::Value) -> Self {
        self.fields = Some(fields);
        self
    }
}

fn phase_kind(phase: RuntimeTracePhase) -> &'static str {
    match phase {
        RuntimeTracePhase::Activation => "activation",
        RuntimeTracePhase::Batch => "batch",
        RuntimeTracePhase::Accepted => "accepted",
        RuntimeTracePhase::Rejected => "rejected",
        RuntimeTracePhase::Commit => "commit",
        RuntimeTracePhase::Decision => "decision",
        RuntimeTracePhase::WatchdogTimeout => "watchdog_timeout",
        RuntimeTracePhase::Termination => "termination",
    }
}

fn default_trace_kind() -> String {
    "unknown".to_string()
}

/// On-disk writer for `runtime-trace.jsonl`. Holds a `BufWriter<File>`
/// and a monotonic sequence counter.
pub struct RuntimeTraceLogger {
    writer: BufWriter<File>,
    sequence: u64,
    degraded: bool,
}

impl RuntimeTraceLogger {
    /// Create a new logger rooted at `session_dir`. Returns
    /// `io::Error` if the file cannot be created.
    pub fn new(session_dir: &Path) -> std::io::Result<Self> {
        let path = session_dir.join("runtime-trace.jsonl");
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let sequence = super::resume_sidecar_sequence(&path);
        Ok(Self {
            writer: BufWriter::new(file),
            sequence,
            degraded: false,
        })
    }

    /// Current sequence number, used by callers to keep their own
    /// counters in sync.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// True when the writer has hit an I/O error and the on-disk
    /// file may be stale.
    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    /// Append a single entry. Sequence is incremented only after
    /// the write and flush succeed. Errors flip the logger into
    /// `degraded` and emit a `tracing::warn!`; subsequent writes
    /// are no-ops.
    ///
    /// Plan 2026-08-12-001 fix-plan U9: oversized string and JSON
    /// fields are truncated to `MAX_SIDECAR_FIELD_BYTES` at the
    /// writer boundary, with one `tracing::warn!` per offending
    /// field.
    pub fn append(&mut self, mut entry: RuntimeTraceEntry) {
        if self.degraded {
            return;
        }
        if let Some(hat) = entry.hat.as_ref() {
            entry.hat = Some(super::cap_string_field(hat, "runtime_trace.hat"));
        }
        if let Some(topic) = entry.topic.as_ref() {
            entry.topic = Some(super::cap_string_field(topic, "runtime_trace.topic"));
        }
        if let Some(status) = entry.status.as_ref() {
            entry.status = Some(super::cap_string_field(status, "runtime_trace.status"));
        }
        entry.kind = super::cap_string_field(&entry.kind, "runtime_trace.kind");
        // Plan 2026-08-12-001 fix-plan U9: cap per-field bytes
        // before serializing. The `source_ref` and JSON `fields`
        // blob are the only non-scalar inputs from upstream.
        if let Some(ref source_ref) = entry.source_ref {
            let capped = super::cap_string_field(source_ref, "runtime_trace.source_ref");
            entry.source_ref = Some(capped.clone());
            entry.reference = Some(capped);
        } else if let Some(reference) = entry.reference.as_ref() {
            entry.reference = Some(super::cap_string_field(reference, "runtime_trace.ref"));
        }
        if let Some(fields) = entry.fields.take() {
            entry.fields = Some(super::cap_json_field(fields, "runtime_trace.fields"));
        }
        let Some(pending) = self.sequence.checked_add(1) else {
            self.degraded = true;
            tracing::warn!(
                target: "ralph_core::diagnostics",
                "runtime trace sequence exhausted; logger marked degraded"
            );
            return;
        };
        entry.sequence = pending;
        let line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(err) => {
                self.degraded = true;
                tracing::warn!(
                    target: "ralph_core::diagnostics",
                    error = %err,
                    "failed to serialize runtime-trace entry; logger marked degraded"
                );
                return;
            }
        };
        if let Err(err) = writeln!(self.writer, "{}", line) {
            self.degraded = true;
            tracing::warn!(
                target: "ralph_core::diagnostics",
                error = %err,
                "failed to write runtime-trace entry; logger marked degraded"
            );
            return;
        }
        if let Err(err) = self.writer.flush() {
            self.degraded = true;
            tracing::warn!(
                target: "ralph_core::diagnostics",
                error = %err,
                "failed to flush runtime-trace writer; logger marked degraded"
            );
            return;
        }
        // Write and flush succeeded — commit the sequence.
        self.sequence = pending;
    }
}
