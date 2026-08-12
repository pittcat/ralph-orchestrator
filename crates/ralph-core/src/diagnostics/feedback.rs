//! Plan 2026-08-12-001 Unit 3: feedback lifecycle sidecar
//! `feedback.jsonl`. Each row records one phase of a recovery
//! envelope (discovered, evidence, action, validation, final).
//! Identity groups rows by `feedback_id == diagnosis_id` (or
//! `retry_key` when no `diagnosis_id` is present), so repeated
//! envelopes for the same `retry_key` do not create a second
//! identity. The writer is best-effort, modeled on
//! [`crate::diagnostics::recovery::RecoveryLogger`].
//!
//! # Activation
//!
//! Created by [`crate::diagnostics::DiagnosticsCollector`] when
//! `full_diagnostics` or `runtime_diagnosis_artifacts` is active.
//! Failure to create the writer at startup flips the collector's
//! sidecar to None; subsequent `log_feedback` calls are no-ops.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Schema version for the feedback file. Bump only on
/// non-additive changes.
pub const FEEDBACK_SCHEMA_VERSION: &str = "run-diagnosis-feedback/v1";

/// Lifecycle phase recorded by a single feedback row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackPhase {
    /// The recovery envelope was observed in `record_recovery_envelope`.
    Discovered,
    /// Evidence was attached to the envelope (e.g. a follow-up
    /// accepted event).
    Evidence,
    /// A recovery action was requested by the runtime
    /// (`apply_runtime_recovery_actions`, `drain_hard_escalations`).
    Action,
    /// The accepted-evidence outcome check ran
    /// (`check_recovery_for_iteration`).
    Validation,
    /// The run terminated; the final state of this feedback
    /// identity is recorded.
    Final,
}

/// Single JSONL row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackEntry {
    pub schema_version: String,
    /// UTC RFC 3339 timestamp the row was appended.
    pub ts: String,
    /// Loop iteration counter (0-based).
    pub iteration: u64,
    /// Monotonic per-session sequence.
    pub sequence: u64,
    /// Stable feedback identity. Reused across rows for the same
    /// retry_key / diagnosis_id.
    pub feedback_id: String,
    /// The retry_key this row is bound to. Stays constant for
    /// every row sharing the same `feedback_id`.
    pub retry_key: String,
    pub phase: FeedbackPhase,
    /// Optional action kind (`InjectDirective`, `ForcePlanBlocked`,
    /// `DedupeEnvelope`, `task.resume`, `correction`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_kind: Option<String>,
    /// Outcome / status string from the upstream record
    /// (`accepted`, `rejected:<code>`, `exhausted`, `recovered`,
    /// `escalated`, `unresolved`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Attempt count carried by the upstream record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// Workspace-relative path or short ref to the upstream
    /// record (e.g. recovery journal line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    /// Bounded JSON object with extra context. Field count is
    /// small on purpose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<serde_json::Value>,
}

impl FeedbackEntry {
    /// New row at the current UTC timestamp. `sequence` and
    /// `schema_version` are filled in by the logger; the
    /// `feedback_id` and `retry_key` are caller-supplied.
    pub fn new(iteration: u64, feedback_id: impl Into<String>, retry_key: impl Into<String>, phase: FeedbackPhase) -> Self {
        Self {
            schema_version: FEEDBACK_SCHEMA_VERSION.to_string(),
            ts: chrono::Utc::now().to_rfc3339(),
            iteration,
            sequence: 0,
            feedback_id: feedback_id.into(),
            retry_key: retry_key.into(),
            phase,
            action_kind: None,
            outcome: None,
            attempt: None,
            source_ref: None,
            fields: None,
        }
    }

    pub fn with_action_kind(mut self, kind: impl Into<String>) -> Self {
        self.action_kind = Some(kind.into());
        self
    }

    pub fn with_outcome(mut self, outcome: impl Into<String>) -> Self {
        self.outcome = Some(outcome.into());
        self
    }

    pub fn with_attempt(mut self, attempt: u32) -> Self {
        self.attempt = Some(attempt);
        self
    }

    pub fn with_source_ref(mut self, source_ref: impl Into<String>) -> Self {
        self.source_ref = Some(source_ref.into());
        self
    }

    pub fn with_fields(mut self, fields: serde_json::Value) -> Self {
        self.fields = Some(fields);
        self
    }
}

/// On-disk writer for `feedback.jsonl`.
pub struct FeedbackLogger {
    writer: BufWriter<File>,
    sequence: u64,
    degraded: bool,
}

impl FeedbackLogger {
    /// Create a new logger rooted at `session_dir`. Returns
    /// `io::Error` if the file cannot be created.
    pub fn new(session_dir: &Path) -> std::io::Result<Self> {
        let path = session_dir.join("feedback.jsonl");
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            sequence: 0,
            degraded: false,
        })
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    /// Append a single row. Sequence is incremented only after the
    /// write and flush succeed. Errors flip the logger into
    /// `degraded` and emit a warning; subsequent writes are no-ops.
    pub fn append(&mut self, mut entry: FeedbackEntry) {
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
                    "failed to serialize feedback entry; logger marked degraded"
                );
                return;
            }
        };
        if let Err(err) = writeln!(self.writer, "{}", line) {
            self.degraded = true;
            tracing::warn!(
                target: "ralph_core::diagnostics",
                error = %err,
                "failed to write feedback entry; logger marked degraded"
            );
            return;
        }
        if let Err(err) = self.writer.flush() {
            self.degraded = true;
            tracing::warn!(
                target: "ralph_core::diagnostics",
                error = %err,
                "failed to flush feedback writer; logger marked degraded"
            );
            return;
        }
        // Write and flush succeeded — commit the sequence.
        self.sequence = pending;
    }
}
