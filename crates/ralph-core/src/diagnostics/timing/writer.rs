//! On-disk writer for `timing.jsonl` (plan 2026-10-29-0104 U1).
//!
//! Modeled on [`crate::diagnostics::runtime_trace::RuntimeTraceLogger`]:
//! a thin `BufWriter<File>` wrapper with a monotonic `sequence`
//! counter, `degraded` latch, and `MAX_SIDECAR_FIELD_BYTES` field
//! capping. All writes are best-effort — a failure flips the writer
//! into `degraded` and emits a `tracing::warn!`; the orchestration
//! main path is never affected (R4).

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use super::model::{TIMING_SIDECAR_FILENAME, TimingSpan};

/// On-disk writer for `timing.jsonl`.
pub struct TimingWriter {
    writer: BufWriter<File>,
    sequence: u64,
    degraded: bool,
}

impl TimingWriter {
    /// Create a new writer rooted at `session_dir`. Returns
    /// `io::Error` if the file cannot be created. An existing file's
    /// sequence is resumed so continuation runs do not restart at 1.
    pub fn new(session_dir: &Path) -> std::io::Result<Self> {
        let path = session_dir.join(TIMING_SIDECAR_FILENAME);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let sequence = super::super::resume_sidecar_sequence(&path);
        Ok(Self {
            writer: BufWriter::new(file),
            sequence,
            degraded: false,
        })
    }

    /// Current sequence number (rows successfully appended).
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// True when the writer has hit an I/O error and the on-disk
    /// file may be stale. Once degraded, `append` is a no-op.
    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    /// Append a single span row. Sequence is incremented only after
    /// the write and flush succeed. Errors flip the writer into
    /// `degraded` and emit a `tracing::warn!`; subsequent writes are
    /// no-ops (mirrors `RuntimeTraceLogger::append`).
    pub fn append(&mut self, mut entry: TimingSpan) {
        if self.degraded {
            return;
        }
        entry.span_id = super::super::cap_string_field(&entry.span_id, "timing.span_id");
        if let Some(ref parent) = entry.parent_span_id {
            entry.parent_span_id = Some(super::super::cap_string_field(parent, "timing.parent"));
        }
        if let Some(ref source_ref) = entry.source_ref {
            entry.source_ref = Some(super::super::cap_string_field(source_ref, "timing.ref"));
        }
        if let Some(attrs) = entry.attributes.take() {
            entry.attributes = Some(super::super::cap_json_field(attrs, "timing.attributes"));
        }
        let Some(pending) = self.sequence.checked_add(1) else {
            self.degraded = true;
            tracing::warn!(
                target: "ralph_core::diagnostics",
                "timing sequence exhausted; writer marked degraded"
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
                    "failed to serialize timing span; writer marked degraded"
                );
                return;
            }
        };
        if let Err(err) = writeln!(self.writer, "{}", line) {
            self.degraded = true;
            tracing::warn!(
                target: "ralph_core::diagnostics",
                error = %err,
                "failed to write timing span; writer marked degraded"
            );
            return;
        }
        if let Err(err) = self.writer.flush() {
            self.degraded = true;
            tracing::warn!(
                target: "ralph_core::diagnostics",
                error = %err,
                "failed to flush timing writer; writer marked degraded"
            );
            return;
        }
        self.sequence = pending;
    }
}
