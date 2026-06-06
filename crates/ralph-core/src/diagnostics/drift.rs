//! JSONL writer for `drift.jsonl`.
//!
//! Each line in `drift.jsonl` is a single [`DriftJournalEntry`]. The
//! writer mirrors [`crate::diagnostics::orchestration::OrchestrationLogger`]
//! and [`crate::diagnostics::hook_runs::HookRunLogger`]; the collector
//! wraps it in `Arc<Mutex<DriftLogger>>` and exposes it via
//! [`crate::diagnostics::DiagnosticsCollector::log_drift`].
//!
//! # Activation
//!
//! Instantiated when either `full_diagnostics` or
//! `runtime_diagnosis_artifacts` is true. When the collector is
//! disabled, no logger is created and the public entry point is a
//! no-op.
//!
//! # Truncation
//!
//! [`DriftJournalEntry::message`] is bounded by
//! [`MAX_DRIFT_MESSAGE_CHARS`]. Longer messages are truncated and
//! suffixed with `\u{2026}` BEFORE serialization. Truncation is
//! applied to a copy of the entry; the caller's struct is not
//! mutated.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::diagnosis::DriftJournalEntry;

/// Maximum number of characters [`DriftJournalEntry::message`] may
/// occupy in the serialized JSONL. Longer messages are truncated
/// and suffixed with `\u{2026}`.
pub const MAX_DRIFT_MESSAGE_CHARS: usize = 1024;

/// JSONL writer for `drift.jsonl`.
pub struct DriftLogger {
    writer: BufWriter<File>,
}

impl DriftLogger {
    /// Create a new `DriftLogger` writing to
    /// `<session_dir>/drift.jsonl`. The file is created if it does
    /// not exist and is opened in append mode.
    ///
    /// # Errors
    /// Returns the underlying `io::Error` if the session directory is
    /// not writable or the file cannot be opened.
    pub fn new(session_dir: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(session_dir.join("drift.jsonl"))?;
        Ok(Self {
            writer: BufWriter::new(file),
        })
    }

    /// Serialize a [`DriftJournalEntry`] as one line of JSON and
    /// flush it to the underlying file.
    ///
    /// The `message` field is truncated to
    /// [`MAX_DRIFT_MESSAGE_CHARS`] characters before serialization.
    /// The caller's entry is not mutated.
    ///
    /// # Errors
    /// Returns the underlying serialization or I/O error. The
    /// collector wrapper is expected to log and swallow.
    pub fn log(&mut self, entry: &DriftJournalEntry) -> std::io::Result<()> {
        let mut sanitized = entry.clone();
        sanitized.message = truncate_message(&sanitized.message);
        serde_json::to_writer(&mut self.writer, &sanitized)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

/// Truncate a single message to [`MAX_DRIFT_MESSAGE_CHARS`]
/// characters, appending `\u{2026}` when truncation occurred.
fn truncate_message(s: &str) -> String {
    let char_count = s.chars().count();
    if char_count <= MAX_DRIFT_MESSAGE_CHARS {
        return s.to_string();
    }
    let keep = MAX_DRIFT_MESSAGE_CHARS.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnosis::{DiagnosisSeverity, DriftJournalEntry, DriftMetric};
    use std::fs;
    use tempfile::TempDir;

    fn sample_entry() -> DriftJournalEntry {
        DriftJournalEntry::builder()
            .metric(DriftMetric::FieldCompleteness)
            .observed_value(0.4)
            .threshold(0.9)
            .severity(DiagnosisSeverity::Warning)
            .topic("work.done")
            .field("plan_name")
            .window_iterations(20)
            .iteration(7)
            .message("plan_name missing in 60% of events")
            .build()
    }

    #[test]
    fn truncate_message_keeps_short() {
        assert_eq!(truncate_message("hello"), "hello");
    }

    #[test]
    fn truncate_message_handles_long() {
        let long = "a".repeat(MAX_DRIFT_MESSAGE_CHARS + 50);
        let out = truncate_message(&long);
        assert_eq!(out.chars().count(), MAX_DRIFT_MESSAGE_CHARS);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn log_writes_one_line_per_entry() {
        let temp = TempDir::new().unwrap();
        let mut logger = DriftLogger::new(temp.path()).unwrap();

        let entry = sample_entry();
        logger.log(&entry).unwrap();
        logger.log(&entry).unwrap();

        let content = fs::read_to_string(temp.path().join("drift.jsonl")).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        for line in lines {
            let parsed: DriftJournalEntry = serde_json::from_str(line).unwrap();
            assert_eq!(parsed.metric, DriftMetric::FieldCompleteness);
            assert_eq!(parsed.field.as_deref(), Some("plan_name"));
        }
    }

    #[test]
    fn log_truncates_long_message() {
        let temp = TempDir::new().unwrap();
        let mut logger = DriftLogger::new(temp.path()).unwrap();

        let long_msg = "m".repeat(MAX_DRIFT_MESSAGE_CHARS + 100);
        let entry = DriftJournalEntry::builder()
            .metric(DriftMetric::CoordJoinRate)
            .observed_value(0.3)
            .threshold(0.8)
            .severity(DiagnosisSeverity::Error)
            .window_iterations(10)
            .iteration(2)
            .message(long_msg.clone())
            .build();

        logger.log(&entry).unwrap();
        let content = fs::read_to_string(temp.path().join("drift.jsonl")).unwrap();
        let line = content.lines().next().unwrap();
        let parsed: DriftJournalEntry = serde_json::from_str(line).unwrap();

        assert_eq!(parsed.message.chars().count(), MAX_DRIFT_MESSAGE_CHARS);
        assert!(parsed.message.ends_with('\u{2026}'));
    }

    #[test]
    fn log_does_not_mutate_caller_entry() {
        let temp = TempDir::new().unwrap();
        let mut logger = DriftLogger::new(temp.path()).unwrap();

        let long_msg = "z".repeat(MAX_DRIFT_MESSAGE_CHARS + 50);
        let entry = DriftJournalEntry::builder()
            .metric(DriftMetric::EmitCadence)
            .observed_value(0.1)
            .threshold(0.5)
            .severity(DiagnosisSeverity::Info)
            .window_iterations(5)
            .iteration(1)
            .message(long_msg.clone())
            .build();
        let original_len = entry.message.chars().count();

        logger.log(&entry).unwrap();

        assert_eq!(entry.message.chars().count(), original_len);
    }
}
