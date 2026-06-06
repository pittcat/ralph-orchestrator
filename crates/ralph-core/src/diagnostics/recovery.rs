//! JSONL writer for `recovery.jsonl`.
//!
//! Each line in `recovery.jsonl` is a single [`RecoveryJournalEntry`]. The
//! writer is a thin `BufWriter<File>` wrapper that mirrors the historical
//! pattern in [`crate::diagnostics::orchestration::OrchestrationLogger`] and
//! [`crate::diagnostics::hook_runs::HookRunLogger`]. U3 is the writer
//! itself; U4 will populate it from existing recovery / gate paths.
//!
//! # Activation
//!
//! The logger is owned by [`crate::diagnostics::DiagnosticsCollector`]
//! and instantiated when either `full_diagnostics` is true or
//! `runtime_diagnosis_artifacts` is true. When the collector is
//! disabled, no `RecoveryLogger` is created and the public
//! [`crate::diagnostics::DiagnosticsCollector::log_recovery`] entry
//! point is a no-op.
//!
//! # Concurrency
//!
//! The logger owns a `BufWriter<File>`. Callers (typically the
//! collector) wrap it in `Arc<Mutex<RecoveryLogger>>` and acquire the
//! lock for the duration of a single `log()` call. The lock is held
//! only while writing one line; long prompts or payloads are not
//! touched here.
//!
//! # Truncation
//!
//! Each note in [`RecoveryJournalEntry::notes`] longer than
//! [`MAX_RECOVERY_NOTE_CHARS`] characters is truncated and suffixed
//! with `\u{2026}` BEFORE serialization, so the JSONL file size is
//! bounded regardless of caller behavior. Truncation is applied to a
//! copy of the entry; the caller's struct is not mutated.
//!
//! # Error handling
//!
//! `log()` returns `io::Result`. The collector wrapper is expected
//! to swallow the result and emit a `tracing::warn!` so that a write
//! failure does not affect the orchestration main path.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::diagnosis::RecoveryJournalEntry;

/// Maximum number of characters a single note may occupy in a
/// `RecoveryJournalEntry::notes` element. Longer notes are truncated
/// to this length and suffixed with `\u{2026}`.
pub const MAX_RECOVERY_NOTE_CHARS: usize = 256;

/// JSONL writer for `recovery.jsonl`.
///
/// The writer is single-threaded; wrap it in `Arc<Mutex<...>>` when
/// sharing with the collector.
pub struct RecoveryLogger {
    writer: BufWriter<File>,
}

impl RecoveryLogger {
    /// Create a new `RecoveryLogger` writing to
    /// `<session_dir>/recovery.jsonl`. The file is created if it does
    /// not exist and is opened in append mode.
    ///
    /// # Errors
    /// Returns the underlying `io::Error` if the session directory is
    /// not writable or the file cannot be opened.
    pub fn new(session_dir: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(session_dir.join("recovery.jsonl"))?;
        Ok(Self {
            writer: BufWriter::new(file),
        })
    }

    /// Serialize a [`RecoveryJournalEntry`] as one line of JSON and
    /// flush it to the underlying file.
    ///
    /// Notes longer than [`MAX_RECOVERY_NOTE_CHARS`] are truncated
    /// before serialization. The caller's entry is not mutated.
    ///
    /// # Errors
    /// Returns the underlying serialization or I/O error. The
    /// collector wrapper is expected to log and swallow.
    pub fn log(&mut self, entry: &RecoveryJournalEntry) -> std::io::Result<()> {
        let mut sanitized = entry.clone();
        sanitized.notes = sanitized
            .notes
            .into_iter()
            .map(|n| truncate_note(&n))
            .collect();
        serde_json::to_writer(&mut self.writer, &sanitized)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

/// Truncate a single note to [`MAX_RECOVERY_NOTE_CHARS`] characters,
/// appending `\u{2026}` when truncation occurred. Notes that fit
/// unchanged are returned as-is.
fn truncate_note(s: &str) -> String {
    let char_count = s.chars().count();
    if char_count <= MAX_RECOVERY_NOTE_CHARS {
        return s.to_string();
    }
    let keep = MAX_RECOVERY_NOTE_CHARS.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnosis::{
        DiagnosisSource, DiagnosisSeverity, RecoveryDiagnosisEnvelope,
    };
    use std::fs;
    use tempfile::TempDir;

    fn sample_entry() -> RecoveryJournalEntry {
        let envelope = RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::MissingEventGate)
            .severity(DiagnosisSeverity::Warning)
            .iteration(3)
            .reason_code("no_emit")
            .message("builder did not emit work.done")
            .source_hat("builder")
            .target_hat("builder")
            .topic("work.done")
            .retry_key("missing_event_gate:builder:work_done:no_emit:*")
            .safe_target(true)
            .build();
        RecoveryJournalEntry::from_envelope(envelope, vec!["short".to_string()])
    }

    #[test]
    fn truncate_note_keeps_short() {
        assert_eq!(truncate_note("hello"), "hello");
    }

    #[test]
    fn truncate_note_handles_long() {
        let long = "a".repeat(MAX_RECOVERY_NOTE_CHARS + 50);
        let out = truncate_note(&long);
        assert_eq!(out.chars().count(), MAX_RECOVERY_NOTE_CHARS);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn log_writes_one_line_per_entry() {
        let temp = TempDir::new().unwrap();
        let mut logger = RecoveryLogger::new(temp.path()).unwrap();

        let entry = sample_entry();
        logger.log(&entry).unwrap();
        logger.log(&entry).unwrap();

        let content = fs::read_to_string(temp.path().join("recovery.jsonl")).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        for line in lines {
            let parsed: RecoveryJournalEntry = serde_json::from_str(line).unwrap();
            assert_eq!(parsed.envelope.reason_code, "no_emit");
        }
    }

    #[test]
    fn log_truncates_long_notes() {
        let temp = TempDir::new().unwrap();
        let mut logger = RecoveryLogger::new(temp.path()).unwrap();

        let long_note = "n".repeat(MAX_RECOVERY_NOTE_CHARS + 100);
        let envelope = RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::DriftMonitor)
            .severity(DiagnosisSeverity::Info)
            .reason_code("r")
            .message("m")
            .build();
        let entry = RecoveryJournalEntry::from_envelope(
            envelope,
            vec![long_note.clone(), "short".to_string()],
        );

        logger.log(&entry).unwrap();
        let content = fs::read_to_string(temp.path().join("recovery.jsonl")).unwrap();
        let line = content.lines().next().unwrap();
        let parsed: RecoveryJournalEntry = serde_json::from_str(line).unwrap();

        assert_eq!(parsed.notes.len(), 2);
        assert_eq!(parsed.notes[0].chars().count(), MAX_RECOVERY_NOTE_CHARS);
        assert!(parsed.notes[0].ends_with('\u{2026}'));
        assert_eq!(parsed.notes[1], "short");
    }

    #[test]
    fn log_does_not_mutate_caller_entry() {
        let temp = TempDir::new().unwrap();
        let mut logger = RecoveryLogger::new(temp.path()).unwrap();

        let long_note = "x".repeat(MAX_RECOVERY_NOTE_CHARS + 50);
        let envelope = RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::StallRecovery)
            .severity(DiagnosisSeverity::Error)
            .reason_code("stall")
            .message("m")
            .build();
        let entry = RecoveryJournalEntry::from_envelope(
            envelope,
            vec![long_note.clone()],
        );
        let original_first_char_count = entry.notes[0].chars().count();

        logger.log(&entry).unwrap();

        // Caller's struct is untouched.
        assert_eq!(entry.notes[0].chars().count(), original_first_char_count);
    }
}
