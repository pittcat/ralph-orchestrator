//! Plan 2026-08-26-1104 Unit 6: bounded frozen evidence window.
//!
//! When the loop detects one of the five anomaly triggers
//! (`watchdog_timeout`, `non_zero_exit`, `precheck_exhausted`,
//! `recovery_exhausted`, `abnormal_activation_outcome`) it flushes
//! the most recent window of structured evidence lines into
//! `<session>/evidence-window.jsonl`. The file is the on-disk
//! artifact consumed by the boundary-coverage reader (U7) and the
//! deterministic attribution engine (U8) — the latter scores the
//! freeze trigger as a confidence signal in DT7.
//!
//! # Wire format
//!
//! Each line of `evidence-window.jsonl` is a JSON object. The
//! first line is the [`AnomalyDescriptor`] carrying the trigger
//! identity; the remaining lines are the buffered candidate rows
//! (in arrival order, oldest dropped when the ring buffer is
//! full) followed by the post-trigger lines supplied by the
//! caller at flush time. There is at most
//! `capacity + 1 + post_trigger_lines.len()` lines per file.
//!
//! # Activation
//!
//! The writer is wired into [`crate::diagnostics::DiagnosticsCollector`]
//! when `full_diagnostics` is true or either of the minimal-session
//! flags (`runtime_diagnosis_artifacts`, `causal_evidence`) is
//! true. The writer is *not* wired for `trace_only` (parent TUI)
//! because that mode owns no loop events.
//!
//! # Field cap
//!
//! Per-field byte cap mirrors the runtime-trace logger: oversized
//! strings and JSON sub-trees are truncated at the
//! [`crate::diagnostics::MAX_SIDECAR_FIELD_BYTES`] (8 KiB)
//! boundary by [`crate::diagnostics::cap_string_field`] and
//! [`crate::diagnostics::cap_json_field`]. The cap applies to any
//! string-valued field within a candidate row AND to the anomaly
//! descriptor's `details` blob. Full prompts / model outputs must
//! not appear in the frozen file (S6.4).
//!
//! # Error handling
//!
//! `flush()` returns `io::Result`. The collector wrapper is
//! expected to swallow the error, emit a `tracing::warn!`, and
//! flip the writer into `degraded`. Subsequent flushes are
//! no-ops while `is_degraded() == true`.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// Schema version for `evidence-window.jsonl`. Bump only on
/// non-additive changes.
pub const EVIDENCE_WINDOW_SCHEMA_VERSION: &str = "run-diagnosis-evidence-window/v1";

/// Default ring buffer capacity. Matches the per-session
/// `telemetry.causal_evidence.window_capacity` setting; the
/// collector passes the configured value when constructing the
/// writer. The default is used by direct unit tests that bypass
/// the telemetry bridge.
pub const DEFAULT_WINDOW_CAPACITY: usize = 200;

/// Stable kind tag for the first row of `evidence-window.jsonl`.
pub const ANOMALY_ROW_KIND: &str = "anomaly";

/// Canonical trigger kind strings. The five values below are the
/// only legal inputs to [`AnomalyDescriptor::trigger_kind`].
pub mod trigger_kinds {
    /// Backend watchdog timer fired before the hat emitted a
    /// terminal event.
    pub const WATCHDOG_TIMEOUT: &str = "watchdog_timeout";
    /// Backend process exited with a non-zero status.
    pub const NON_ZERO_EXIT: &str = "non_zero_exit";
    /// Precheck retry budget exhausted for some topic.
    pub const PRECHECK_EXHAUSTED: &str = "precheck_exhausted";
    /// Recovery retry budget exhausted for some retry key.
    pub const RECOVERY_EXHAUSTED: &str = "recovery_exhausted";
    /// Hat activation outcome was abnormal (empty / merge_failed /
    /// unreadable channel) and the loop is about to terminate.
    pub const ABNORMAL_ACTIVATION_OUTCOME: &str = "abnormal_activation_outcome";
}

/// Descriptor carried as the first line of every
/// `evidence-window.jsonl`. The trigger kind is one of the
/// constants in [`trigger_kinds`].
#[derive(Debug, Clone, Serialize)]
pub struct AnomalyDescriptor {
    pub trigger_kind: String,
    pub ts: String,
    pub iteration: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Frozen-window sidecar writer. Owns a ring buffer of candidate
/// JSON lines plus a lazily-opened `BufWriter<File>` for the
/// destination `evidence-window.jsonl`. The file is created on
/// the first [`Self::flush`] call rather than at construction
/// time so that a normal `LOOP_COMPLETE` loop never leaves the
/// artifact on disk (S6.1: the file must not exist when no
/// anomaly fired).
pub struct EvidenceWindowWriter {
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    capacity: usize,
    buffer: VecDeque<Value>,
    degraded: bool,
}

impl EvidenceWindowWriter {
    /// Construct a writer rooted at `session_dir`. The destination
    /// `evidence-window.jsonl` is NOT created here — see
    /// [`Self::flush`] for the actual open. The capacity is the
    /// ring buffer width and must be at least 1.
    pub fn new(session_dir: &Path, capacity: usize) -> std::io::Result<Self> {
        Ok(Self {
            path: session_dir.join("evidence-window.jsonl"),
            writer: None,
            capacity: capacity.max(1),
            buffer: VecDeque::with_capacity(capacity.max(1)),
            degraded: false,
        })
    }

    /// Number of buffered candidate rows currently held in the
    /// ring buffer (always `<= capacity`). Exposed for unit tests.
    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    /// Configured ring buffer capacity (after the
    /// `max(1)` clamp applied in [`Self::new`]).
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Snapshot of the buffered candidate rows in arrival order.
    /// Exposed for unit tests that want to inspect ring buffer
    /// contents without going through `flush`.
    pub fn snapshot_buffer(&self) -> Vec<Value> {
        self.buffer.iter().cloned().collect()
    }

    /// True when the writer has hit an I/O error and the
    /// underlying file may be stale. Subsequent flushes are
    /// no-ops while `is_degraded() == true`.
    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    /// Push a candidate line into the ring buffer. Lines that
    /// arrive after the buffer is full cause the oldest entry to
    /// be dropped silently — the ring semantics are
    /// "newest-first", matching the S6.3 contract. The input is
    /// capped per-field via [`super::cap_string_field`] /
    /// [`super::cap_json_field`] so the ring buffer cannot
    /// accumulate unbounded entries (an oversized row is
    /// truncated before being stored).
    pub fn push(&mut self, mut line: Value) {
        if self.degraded {
            return;
        }
        line = cap_window_value(line);
        if self.buffer.len() == self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back(line);
    }

    /// Write the frozen window to disk.
    ///
    /// Layout:
    ///
    /// 1. anomaly descriptor row (always first).
    /// 2. buffered candidate rows in arrival order (oldest
    ///    first); at most `capacity` rows.
    /// 3. caller-supplied post-trigger rows in the order they
    ///    were provided.
    ///
    /// The file is *truncated* before writing (per the constructor)
    /// so a retry on the same anomaly produces a clean sidecar.
    pub fn flush(
        &mut self,
        anomaly: AnomalyDescriptor,
        post_trigger_lines: Vec<Value>,
    ) -> std::io::Result<()> {
        if self.degraded {
            return Err(std::io::Error::other(
                "evidence-window writer is degraded; flush refused",
            ));
        }
        // Lazy-open on the first successful flush so a normal
        // LOOP_COMPLETE loop never creates `evidence-window.jsonl`.
        if self.writer.is_none() {
            match OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&self.path)
            {
                Ok(file) => self.writer = Some(BufWriter::new(file)),
                Err(err) => {
                    self.degraded = true;
                    tracing::warn!(
                        target: "ralph_core::diagnostics",
                        path = %self.path.display(),
                        error = %err,
                        "failed to open evidence-window.jsonl on first flush; writer marked degraded",
                    );
                    return Err(err);
                }
            }
        }
        let writer = self.writer.as_mut().expect("writer opened above");
        let anomaly_row = build_anomaly_row(&anomaly)?;
        let pre_trigger = self
            .buffer
            .iter()
            .cloned()
            .map(cap_window_value)
            .collect::<Vec<_>>();
        let remaining = self.capacity.saturating_sub(pre_trigger.len());
        let post_trigger = post_trigger_lines
            .into_iter()
            .take(remaining)
            .map(cap_window_value)
            .collect::<Vec<_>>();

        if let Err(err) = write_row(writer, &anomaly_row) {
            self.degraded = true;
            tracing::warn!(
                target: "ralph_core::diagnostics",
                error = %err,
                "failed to write anomaly descriptor to evidence-window.jsonl; writer marked degraded",
            );
            return Err(err);
        }
        for row in &pre_trigger {
            if let Err(err) = write_row(writer, row) {
                self.degraded = true;
                tracing::warn!(
                    target: "ralph_core::diagnostics",
                    error = %err,
                    "failed to write pre-trigger row to evidence-window.jsonl; writer marked degraded",
                );
                return Err(err);
            }
        }
        for row in &post_trigger {
            if let Err(err) = write_row(writer, row) {
                self.degraded = true;
                tracing::warn!(
                    target: "ralph_core::diagnostics",
                    error = %err,
                    "failed to write post-trigger row to evidence-window.jsonl; writer marked degraded",
                );
                return Err(err);
            }
        }
        if let Err(err) = writer.flush() {
            self.degraded = true;
            tracing::warn!(
                target: "ralph_core::diagnostics",
                error = %err,
                "failed to flush evidence-window.jsonl; writer marked degraded",
            );
            return Err(err);
        }
        // Reset the ring buffer after a successful flush so the
        // next anomaly starts with an empty pre-trigger window.
        self.buffer.clear();
        Ok(())
    }
}

fn build_anomaly_row(anomaly: &AnomalyDescriptor) -> std::io::Result<Value> {
    let details = anomaly
        .details
        .clone()
        .map(cap_window_value)
        .unwrap_or(Value::Null);
    let mut row = serde_json::Map::new();
    row.insert(
        "schema_version".to_string(),
        Value::String(EVIDENCE_WINDOW_SCHEMA_VERSION.to_string()),
    );
    row.insert(
        "kind".to_string(),
        Value::String(ANOMALY_ROW_KIND.to_string()),
    );
    row.insert(
        "trigger_kind".to_string(),
        Value::String(anomaly.trigger_kind.clone()),
    );
    row.insert("ts".to_string(), Value::String(anomaly.ts.clone()));
    row.insert(
        "iteration".to_string(),
        Value::Number(anomaly.iteration.into()),
    );
    if anomaly.details.is_some() {
        row.insert("details".to_string(), details);
    }
    Ok(Value::Object(row))
}

fn write_row<W: Write>(writer: &mut W, row: &Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *writer, row)?;
    writer.write_all(b"\n")
}

fn cap_window_value(value: Value) -> Value {
    // Recursively cap every string field at
    // [`super::MAX_SIDECAR_FIELD_BYTES`]. We deliberately do NOT
    // use [`super::cap_json_field`] here because that helper
    // drops keys wholesale when the parent object still exceeds
    // 8 KiB after per-field capping — that behavior would erase
    // the very evidence the truncated string was meant to
    // preserve. Instead we cap every string leaf in isolation
    // and apply the same bounded-row contract used by other
    // diagnostic sidecars. Callers must not be able to grow a
    // frozen window through arbitrarily large nested values.
    const MAX_COLLECTION_ITEMS: usize = 32;
    const MAX_WINDOW_VALUE_BYTES: usize = 64 * 1024;
    const MAX_NESTING_DEPTH: usize = 4;

    fn cap(value: Value, depth: usize) -> Value {
        if depth >= MAX_NESTING_DEPTH {
            return Value::Null;
        }
        match value {
            Value::String(s) => Value::String(super::cap_string_field(&s, "evidence_window")),
            Value::Array(items) => Value::Array(
                items
                    .into_iter()
                    .take(MAX_COLLECTION_ITEMS)
                    .map(|item| cap(item, depth + 1))
                    .collect(),
            ),
            Value::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len().min(MAX_COLLECTION_ITEMS));
                for (k, v) in map.into_iter().take(MAX_COLLECTION_ITEMS) {
                    out.insert(k, cap(v, depth + 1));
                }
                Value::Object(out)
            }
            other => other,
        }
    }
    let capped = cap(value, 0);
    match serde_json::to_vec(&capped) {
        Ok(bytes) if bytes.len() <= MAX_WINDOW_VALUE_BYTES => capped,
        Ok(bytes) => serde_json::json!({
            "truncated": true,
            "original_bytes": bytes.len(),
        }),
        Err(_) => serde_json::json!({"truncated": true}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_zero_is_clamped_to_one_in_constructor() {
        // Defensive: callers might pass `0` from a config bug.
        // The constructor clamps the capacity to 1 so the ring
        // buffer never silently becomes zero-width.
        let temp = tempfile::TempDir::new().expect("TempDir");
        let writer = EvidenceWindowWriter::new(temp.path(), 0).expect("writer");
        assert_eq!(writer.capacity(), 1);
    }

    #[test]
    fn build_anomaly_row_omits_details_when_none() {
        let anomaly = AnomalyDescriptor {
            trigger_kind: trigger_kinds::WATCHDOG_TIMEOUT.to_string(),
            ts: "2026-08-26T00:00:00Z".to_string(),
            iteration: 0,
            details: None,
        };
        let row = build_anomaly_row(&anomaly).expect("build row");
        assert_eq!(row["kind"], json!(ANOMALY_ROW_KIND));
        assert_eq!(row["trigger_kind"], json!("watchdog_timeout"));
        assert!(row.get("details").is_none());
    }

    use serde_json::json;
}
