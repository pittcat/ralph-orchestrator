//! Persistence layer for agent_doc_sync.
//!
//! Two independent write paths, both called at the end of `sync_all`:
//!
//! 1. **Snapshot** (`write_snapshot`): Atomic write of `agent_doc_sync.json`
//!    for `ralph doctor` to read. Contains `{synced, skipped, failed,
//!    last_success_at, blocks: [...]}`.
//!
//! 2. **Recovery envelope** (`append_recovery_envelope`): Appends a
//!    `RecoveryJournalEntry` to `recovery.jsonl` for `ralph diagnose`.
//!    Uses `DiagnosisSource::AgentDocSync`.
//!
//! The two paths are **independent** — a failure in one does not affect
//! the other (KTD-8).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::diagnosis::{
    DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
    RecoveryJournalEntry,
};

use super::SyncReport;

/// Snapshot of agent_doc_sync state, written atomically to
/// `<workspace_root>/.ralph/diagnostics/agent_doc_sync.json`.
///
/// Read by `ralph doctor` for a fast O(1) health check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDocSyncSnapshot {
    /// Number of blocks synced (appended or replaced).
    pub synced: usize,
    /// Number of blocks skipped (already up to date).
    pub skipped: usize,
    /// Number of blocks that failed.
    pub failed: usize,
    /// Wall-clock time of the last successful sync (any block).
    pub last_success_at: Option<DateTime<Utc>>,
}

impl AgentDocSyncSnapshot {
    /// Build a snapshot from a [`SyncReport`].
    pub fn from_report(report: &SyncReport) -> Self {
        let last_success_at = if report.synced > 0 {
            Some(Utc::now())
        } else {
            None
        };
        Self {
            synced: report.synced,
            skipped: report.skipped,
            failed: report.failed,
            last_success_at,
        }
    }
}

/// Write `agent_doc_sync.json` atomically to the diagnostics directory.
///
/// Creates the diagnostics directory if it does not exist. Uses
/// `tempfile::NamedTempFile` + `persist` for atomic replacement so a
/// crash mid-write never corrupts the snapshot.
///
/// # Errors
/// Returns `io::Error` if the file cannot be written.
pub fn write_snapshot(workspace_root: &Path, report: &SyncReport) -> std::io::Result<()> {
    let diag_dir = workspace_root.join(".ralph").join("diagnostics");
    fs::create_dir_all(&diag_dir)?;

    let snapshot = AgentDocSyncSnapshot::from_report(report);
    let path = diag_dir.join("agent_doc_sync.json");

    // Atomic write: temp file + rename
    let temp = tempfile::NamedTempFile::new_in(&diag_dir)?;
    serde_json::to_writer_pretty(std::io::BufWriter::new(temp.reopen()?), &snapshot)?;
    temp.persist(&path)?;

    Ok(())
}

/// Append a recovery envelope to `recovery.jsonl` in the given session
/// directory.
///
/// The envelope uses [`DiagnosisSource::AgentDocSync`] and carries the
/// sync report counts. If no `session_dir` is provided, this is a
/// no-op (sync ran without diagnostics enabled).
///
/// # Errors
/// Returns `io::Error` if the file cannot be opened or written.
pub fn append_recovery_envelope(
    session_dir: Option<&Path>,
    report: &SyncReport,
) -> std::io::Result<()> {
    let Some(session_dir) = session_dir else {
        return Ok(());
    };

    let outcome = if report.failed > 0 {
        DiagnosisOutcome::Failed
    } else {
        DiagnosisOutcome::Recovered
    };

    let severity = if report.failed > 0 {
        DiagnosisSeverity::Warning
    } else {
        DiagnosisSeverity::Info
    };

    let reason_code = if report.failed > 0 {
        "sync_failed"
    } else if report.synced > 0 {
        "sync_completed"
    } else {
        "sync_up_to_date"
    };

    let message = format!(
        "agent_doc_sync: synced={}, skipped={}, failed={}",
        report.synced, report.skipped, report.failed
    );

    let retry_key = format!(
        "agent_doc_sync:loop:agent_doc_sync:{}",
        if report.failed > 0 {
            "failed"
        } else if report.synced > 0 {
            "synced"
        } else {
            "skipped"
        }
    );

    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::AgentDocSync)
        .severity(severity)
        .reason_code(reason_code)
        .message(message)
        .retry_key(retry_key)
        .outcome(outcome)
        .safe_target(false)
        .build();

    let entry = RecoveryJournalEntry::from_envelope(envelope, vec![]);

    // Append to recovery.jsonl using the same pattern as RecoveryLogger.
    let recovery_path = session_dir.join("recovery.jsonl");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&recovery_path)?;

    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, &entry)?;
    writer.write_all(b"\n")?;
    writer.flush()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_report() -> SyncReport {
        SyncReport {
            synced: 1,
            skipped: 2,
            failed: 0,
            block_results: vec![],
        }
    }

    #[test]
    fn write_snapshot_creates_file_with_expected_shape() {
        let dir = TempDir::new().unwrap();
        let report = sample_report();

        write_snapshot(dir.path(), &report).unwrap();

        let path = dir.path()
            .join(".ralph")
            .join("diagnostics")
            .join("agent_doc_sync.json");
        assert!(path.exists());

        let content = fs::read_to_string(&path).unwrap();
        let snapshot: AgentDocSyncSnapshot = serde_json::from_str(&content).unwrap();
        assert_eq!(snapshot.synced, 1);
        assert_eq!(snapshot.skipped, 2);
        assert_eq!(snapshot.failed, 0);
        assert!(snapshot.last_success_at.is_some());
    }

    #[test]
    fn write_snapshot_is_atomic() {
        let dir = TempDir::new().unwrap();
        let report = sample_report();

        // First write
        write_snapshot(dir.path(), &report).unwrap();

        let path = dir.path()
            .join(".ralph")
            .join("diagnostics")
            .join("agent_doc_sync.json");
        let first_content = fs::read_to_string(&path).unwrap();

        // Second write
        let mut report2 = report.clone();
        report2.synced = 3;
        write_snapshot(dir.path(), &report2).unwrap();

        let second_content = fs::read_to_string(&path).unwrap();
        assert_ne!(first_content, second_content);

        // Verify the second write succeeded
        let snapshot: AgentDocSyncSnapshot = serde_json::from_str(&second_content).unwrap();
        assert_eq!(snapshot.synced, 3);
    }

    #[test]
    fn write_snapshot_failed_not_zero_still_writes() {
        let dir = TempDir::new().unwrap();
        let report = SyncReport {
            synced: 0,
            skipped: 0,
            failed: 1,
            block_results: vec![],
        };

        write_snapshot(dir.path(), &report).unwrap();

        let path = dir.path()
            .join(".ralph")
            .join("diagnostics")
            .join("agent_doc_sync.json");
        let snapshot: AgentDocSyncSnapshot =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(snapshot.failed, 1);
        assert!(snapshot.last_success_at.is_none());
    }

    #[test]
    fn append_recovery_envelope_no_session_dir_is_noop() {
        let report = sample_report();
        // Should not error
        append_recovery_envelope(None, &report).unwrap();
    }

    #[test]
    fn append_recovery_envelope_uses_existing_logger_path() {
        let dir = TempDir::new().unwrap();
        let report = sample_report();

        append_recovery_envelope(Some(dir.path()), &report).unwrap();

        let recovery_path = dir.path().join("recovery.jsonl");
        assert!(recovery_path.exists());

        let content = fs::read_to_string(&recovery_path).unwrap();
        let line = content.lines().next().unwrap();
        let entry: RecoveryJournalEntry = serde_json::from_str(line).unwrap();
        assert_eq!(entry.envelope.source, DiagnosisSource::AgentDocSync);
    }

    #[test]
    fn append_recovery_envelope_outcome_on_failure() {
        let dir = TempDir::new().unwrap();
        let report = SyncReport {
            synced: 0,
            skipped: 0,
            failed: 1,
            block_results: vec![],
        };

        append_recovery_envelope(Some(dir.path()), &report).unwrap();

        let content = fs::read_to_string(dir.path().join("recovery.jsonl")).unwrap();
        let entry: RecoveryJournalEntry =
            serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(entry.envelope.outcome, DiagnosisOutcome::Failed);
        assert_eq!(entry.envelope.severity, DiagnosisSeverity::Warning);
    }

    #[test]
    fn append_recovery_envelope_outcome_on_success() {
        let dir = TempDir::new().unwrap();
        let mut report = sample_report();
        report.failed = 0;

        append_recovery_envelope(Some(dir.path()), &report).unwrap();

        let content = fs::read_to_string(dir.path().join("recovery.jsonl")).unwrap();
        let entry: RecoveryJournalEntry =
            serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(entry.envelope.outcome, DiagnosisOutcome::Recovered);
        assert_eq!(entry.envelope.severity, DiagnosisSeverity::Info);
    }

    #[test]
    fn append_recovery_envelope_outcome_on_up_to_date() {
        let dir = TempDir::new().unwrap();
        let report = SyncReport {
            synced: 0,
            skipped: 3,
            failed: 0,
            block_results: vec![],
        };

        append_recovery_envelope(Some(dir.path()), &report).unwrap();

        let content = fs::read_to_string(dir.path().join("recovery.jsonl")).unwrap();
        let entry: RecoveryJournalEntry =
            serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(entry.envelope.outcome, DiagnosisOutcome::Recovered);
        assert!(entry.envelope.reason_code.contains("up_to_date"));
    }

    #[test]
    fn dual_writes_are_independent() {
        let dir = TempDir::new().unwrap();
        let report = sample_report();

        // Write both
        write_snapshot(dir.path(), &report).unwrap();
        append_recovery_envelope(Some(dir.path()), &report).unwrap();

        // Verify both exist and are independent
        let snapshot_path = dir.path()
            .join(".ralph")
            .join("diagnostics")
            .join("agent_doc_sync.json");
        let recovery_path = dir.path().join("recovery.jsonl");
        assert!(snapshot_path.exists());
        assert!(recovery_path.exists());

        // Snapshot file is valid JSON
        let _: AgentDocSyncSnapshot =
            serde_json::from_str(&fs::read_to_string(&snapshot_path).unwrap()).unwrap();
        // Recovery file is valid JSONL
        let entry: RecoveryJournalEntry =
            serde_json::from_str(&fs::read_to_string(&recovery_path).unwrap().lines().next().unwrap())
                .unwrap();
        assert_eq!(entry.envelope.source, DiagnosisSource::AgentDocSync);
    }
}
