//! Persistent rejection log — `.ralph/recovery.jsonl` records
//! for the deterministic-correction path (U7a plan
//! 2026-06-21-002).
//!
//! The legacy `RecoveryDiagnosisEnvelope` flow already writes a
//! `recovery.jsonl` file under
//! `.ralph/diagnostics/<session>/recovery.jsonl`.  That file is
//! session-scoped and lives inside the diagnostics collector's
//! directory tree; `ralph diagnose` reads it for the per-session
//! summary.
//!
//! U7a introduces a *second* `recovery.jsonl` at the workspace
//! root (`.ralph/recovery.jsonl`) so:
//!
//!   1. The deterministic-correction path can persist the
//!      per-rejection record alongside the prompt block — even
//!      when the diagnostics collector is disabled
//!      (`RALPH_DIAGNOSTICS` unset).
//!   2. `ralph diagnose` (U8) can prefer the ledger-aligned
//!      log over the legacy session-scoped log for offline
//!      analysis.
//!   3. Bounded-retry bookkeeping survives session restarts:
//!      operators tail the file and see the per-key retry
//!      history.
//!
//! ## File format
//!
//! JSON Lines (one `RejectionRecord` per line).  Fields:
//!
//! | Field | Type | Notes |
//! |-------|------|-------|
//! | `ts` | RFC3339 string | When the rejection was recorded. |
//! | `hat` | string | Source hat (`"unknown"` when missing). |
//! | `topic` | string | Rejected topic. |
//! | `reason_code` | string | Stable code, e.g. `origin:missing_field`. |
//! | `retry_count` | u32 | Per-key counter (R2 + R3). |
//! | `terminal_reason` | Option<string> | Set when the rejection tripped escalation. |
//!
//! File I/O is best-effort: a write failure is logged but does
//! not abort the loop (matches the policy of the legacy
//! `RecoveryDiagnosisEnvelope` logger).

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Relative path of the rejection log file inside the workspace.
pub const RECOVERY_LOG_RELATIVE_PATH: &str = ".ralph/recovery.jsonl";

/// One line in the rejection log.  Serialised as JSON; mirrors
/// the field shape used by [`crate::diagnosis::RecoveryJournalEntry`]
/// for forward-compatibility with `ralph diagnose`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectionRecord {
    /// RFC3339 timestamp the record was written.
    pub ts: String,
    /// Source hat, or `"unknown"` when missing.
    pub hat: String,
    /// Rejected topic.
    pub topic: String,
    /// Stable reason code (`origin:missing_field`, etc.).
    pub reason_code: String,
    /// Retry count for this key at the time of the record.
    pub retry_count: u32,
    /// Optional terminal reason (R11).  `None` for records that
    /// did not trip escalation.
    pub terminal_reason: Option<String>,
}

impl RejectionRecord {
    /// Convenience builder.  `ts` defaults to `now_rfc3339()`.
    pub fn new(
        hat: impl Into<String>,
        topic: impl Into<String>,
        reason_code: impl Into<String>,
        retry_count: u32,
    ) -> Self {
        Self {
            ts: now_rfc3339(),
            hat: hat.into(),
            topic: topic.into(),
            reason_code: reason_code.into(),
            retry_count,
            terminal_reason: None,
        }
    }

    /// Mark the record as terminal (R11 escalation).  Returns
    /// the mutated value.
    pub fn with_terminal_reason(mut self, reason: impl Into<String>) -> Self {
        self.terminal_reason = Some(reason.into());
        self
    }

    /// Stable retry key — `hat+topic+reason_code`.  Mirrors the
    /// shape used by `Rejection::compute_retry_key` minus the
    /// leading `stage:` prefix (the `reason_code` field already
    /// includes the stage).
    pub fn retry_key(&self) -> String {
        format!("{}:{}:{}", self.hat, self.topic, self.reason_code)
    }
}

/// Resolve the workspace-rooted path of the rejection log.
/// Returns `<workspace>/.ralph/recovery.jsonl`.  The directory
/// is created on demand.
pub fn recovery_log_path(workspace: &Path) -> PathBuf {
    workspace.join(RECOVERY_LOG_RELATIVE_PATH)
}

/// Append a single record to the rejection log.  Best-effort:
/// any I/O error is returned so the caller can log it (the loop
/// runner calls this inside `tracing::warn!`).
pub fn append_rejection(workspace: &Path, record: &RejectionRecord) -> std::io::Result<()> {
    let path = recovery_log_path(workspace);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let mut writer = std::io::BufWriter::new(file);
    serde_json::to_writer(&mut writer, record).map_err(std::io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

/// Read every record currently in the rejection log.  Returns
/// an empty `Vec` when the file does not exist or is empty.
/// Malformed lines are skipped (best-effort: the file is meant
/// for `tail -f` first, structured parsing second).
pub fn read_rejection_log(workspace: &Path) -> std::io::Result<Vec<RejectionRecord>> {
    let path = recovery_log_path(workspace);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path)?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<RejectionRecord>(trimmed) {
            records.push(record);
        }
    }
    Ok(records)
}

/// Return the retry count for `retry_key` by counting records
/// with the same `(hat, topic, reason_code)` tuple.  Used by
/// `RecoveryResponder`-adjacent paths to recover the per-key
/// counter across restarts.
pub fn retry_count_for(workspace: &Path, retry_key: &str) -> u32 {
    read_rejection_log(workspace)
        .map(|records| {
            records
                .iter()
                .filter(|r| r.retry_key() == retry_key)
                .count() as u32
        })
        .unwrap_or(0)
}

/// Delete the rejection log.  Test-only helper.
#[cfg(test)]
pub fn reset_rejection_log(workspace: &Path) -> std::io::Result<()> {
    let path = recovery_log_path(workspace);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// RFC3339-ish timestamp using `chrono::Utc::now()`.  Kept
/// private so tests can substitute deterministic clocks in the
/// future without touching the public API.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn append_creates_file_and_dir() {
        let dir = TempDir::new().unwrap();
        let record = RejectionRecord::new("executor", "work.done", "policy:missing_field", 1);
        append_rejection(dir.path(), &record).unwrap();
        let path = recovery_log_path(dir.path());
        assert!(path.exists(), "recovery.jsonl should be created");
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 1);
    }

    #[test]
    fn read_returns_appended_records() {
        let dir = TempDir::new().unwrap();
        let r1 = RejectionRecord::new("a", "t.x", "policy:missing_field", 1);
        let r2 = RejectionRecord::new("b", "t.y", "origin:unknown_hat", 2);
        append_rejection(dir.path(), &r1).unwrap();
        append_rejection(dir.path(), &r2).unwrap();
        let records = read_rejection_log(dir.path()).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].hat, "a");
        assert_eq!(records[1].retry_count, 2);
    }

    #[test]
    fn read_skips_malformed_lines() {
        let dir = TempDir::new().unwrap();
        let path = recovery_log_path(dir.path());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(b"{\"ts\":\"now\",\"hat\":\"a\",\"topic\":\"x\",\"reason_code\":\"r\",\"retry_count\":1,\"terminal_reason\":null}\n").unwrap();
        f.write_all(b"not-json\n").unwrap();
        f.write_all(b"{\"ts\":\"now\",\"hat\":\"b\",\"topic\":\"y\",\"reason_code\":\"r2\",\"retry_count\":2,\"terminal_reason\":null}\n").unwrap();
        drop(f);
        let records = read_rejection_log(dir.path()).unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn read_empty_log_returns_empty_vec() {
        let dir = TempDir::new().unwrap();
        let records = read_rejection_log(dir.path()).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn retry_count_for_filters_by_key() {
        let dir = TempDir::new().unwrap();
        let r1 = RejectionRecord::new("a", "t.x", "policy:missing_field", 1);
        let r2 = RejectionRecord::new("a", "t.x", "policy:missing_field", 2);
        let r3 = RejectionRecord::new("a", "t.y", "policy:missing_field", 1);
        append_rejection(dir.path(), &r1).unwrap();
        append_rejection(dir.path(), &r2).unwrap();
        append_rejection(dir.path(), &r3).unwrap();
        let key_a_x = format!("{}:{}:{}", "a", "t.x", "policy:missing_field");
        assert_eq!(retry_count_for(dir.path(), &key_a_x), 2);
        let key_a_y = format!("{}:{}:{}", "a", "t.y", "policy:missing_field");
        assert_eq!(retry_count_for(dir.path(), &key_a_y), 1);
    }

    #[test]
    fn with_terminal_reason_serialises_field() {
        let r =
            RejectionRecord::new("a", "x", "r", 3).with_terminal_reason("retry budget exhausted");
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("terminal_reason"));
        assert!(s.contains("retry budget exhausted"));
    }

    #[test]
    fn retry_key_shape_matches_rejection() {
        let r = RejectionRecord::new("executor", "work.done", "policy:missing_field", 1);
        assert_eq!(r.retry_key(), "executor:work.done:policy:missing_field");
    }
}
