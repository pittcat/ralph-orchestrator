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
//! when `full_diagnostics` or `runtime_diagnosis_artifacts` is
//! active. The collector wraps it in `Arc<Mutex<RuntimeTraceLogger>>`.
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
    WatchdogTimeout,
    Termination,
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
    /// Monotonic per-session sequence number. Resets to 0 when the
    /// logger is created.
    pub sequence: u64,
    pub phase: RuntimeTracePhase,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    /// Human-readable status string (`accepted`, `rejected:<code>`,
    /// `commit`, `watchdog_timeout`, `termination:<reason>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
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
            hat: None,
            topic: None,
            source_ref: None,
            status: None,
            fields: None,
        }
    }

    /// Set the `hat` field. Builder-style; returns the modified entry.
    pub fn with_hat(mut self, hat: impl Into<String>) -> Self {
        self.hat = Some(hat.into());
        self
    }

    /// Set the `topic` field.
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// Set the `source_ref` field.
    pub fn with_source_ref(mut self, source_ref: impl Into<String>) -> Self {
        self.source_ref = Some(source_ref.into());
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
        Ok(Self {
            writer: BufWriter::new(file),
            sequence: 0,
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
    pub fn append(&mut self, mut entry: RuntimeTraceEntry) {
        if self.degraded {
            return;
        }
        let pending = self.sequence + 1;
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
