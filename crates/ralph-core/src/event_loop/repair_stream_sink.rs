//! U6 (2026-06-27 mechanism foundation completion):
//! `RepairStreamSink` writes repair-stream events to
//! `<workspace>/.ralph/recovery.jsonl` using
//! the same envelope shape as `record_stage_rejection`,
//! so a single consumer (`ralph diagnose`) can read both
//! the stage-rejection signal and the repair-dispatch
//! signal from the same JSONL stream.
//!
//! Why this lives outside `EventLoop`: the sink is a
//! pure file-I/O boundary. It takes an owned `Event`
//! and an absolute workspace path, appends one line,
//! and returns `Result<()>`. No bus, no recovery
//! envelope construction, no orchestration. The
//! orchestration glue (writing the line via the same
//! `record_recovery_envelope` path that the stage
//! pipeline uses) lives in `event_loop::mod` and is
//! wired in U7.
//!
//! Cross-platform / concurrency semantics: standard
//! Rust file I/O. The sink is **not** thread-safe; the
//! caller serialises calls per-loop. The runtime uses
//! `BufWriter` + flush per call so a crash mid-append
//! does not corrupt the JSONL.

use ralph_proto::Event;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::Path;

/// Stable reason code used by every envelope written
/// from this sink. Matches the `DiagnosisSource`
/// classification so `ralph diagnose` can attribute the
/// record to the repair stream.
pub const REPAIR_SINK_REASON_CODE: &str = "repair_dispatch";

/// Sink that appends one envelope per accepted repair
/// event to `recovery.jsonl` (the same file used by
/// `record_stage_rejection`). The sink is intentionally
/// small — the orchestration layer (`event_loop::mod` /
/// U7) decides when to call it.
#[derive(Debug, Clone, Default)]
pub struct RepairStreamSink;

impl RepairStreamSink {
    /// Create a new sink. The sink holds no state; the
    /// constructor exists for symmetry with future
    /// per-loop config.
    pub fn new() -> Self {
        Self
    }

    /// Append a single repair envelope to
    /// `<workspace>/.ralph/recovery.jsonl`. The envelope shape
    /// matches the `RecoveryDiagnosisEnvelope` produced
    /// by `record_stage_rejection` so a single consumer
    /// can read both signal types from the same file.
    ///
    /// Returns `Err` if the workspace directory cannot
    /// be created or the file cannot be opened. The
    /// caller is expected to log the error and continue
    /// — the repair stream is best-effort, the loop
    /// must not crash on a transient FS error.
    pub fn record(&self, event: &Event, workspace: &Path) -> std::io::Result<()> {
        record_repair_event(event, workspace)
    }
}

/// Free function form: same I/O as
/// `RepairStreamSink::record`. Tests use this directly
/// so they don't have to construct a sink.
pub fn record_repair_event(event: &Event, workspace: &Path) -> std::io::Result<()> {
    let dir = workspace.join(".ralph");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("recovery.jsonl");
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let mut writer = BufWriter::new(file);
    let line = serialise_repair_envelope(event);
    writeln!(writer, "{line}")?;
    writer.flush()?;
    Ok(())
}

/// Build the JSON envelope line that the sink writes.
/// Matches the schema of `RecoveryDiagnosisEnvelope` so
/// the existing `ralph diagnose` consumers do not need
/// to special-case the repair signal. The `topic`,
/// `source_hat`, and `payload` mirror the
/// `RecoveryDiagnosisEnvelope` fields.
fn serialise_repair_envelope(event: &Event) -> String {
    let source_hat = event
        .source
        .as_ref()
        .map(|h| h.as_str().to_string())
        .unwrap_or_default();
    serde_json::json!({
        "envelope": {
            "source": "RepairStream",
            "severity": "Info",
            "topic": event.topic.as_str(),
            "source_hat": source_hat,
            "reason_code": REPAIR_SINK_REASON_CODE,
            "message": format!(
                "repair-stream event recorded for topic '{}'",
                event.topic
            ),
            "payload_preview": event.payload.chars().take(200).collect::<String>(),
        },
        "notes": [
            format!(
                "repair_sink: topic={} source_hat={}",
                event.topic, source_hat
            )
        ]
    })
    .to_string()
}

#[cfg(test)]
mod tests;
