//! Doctor health check for `agent_doc_sync`.
//!
//! Reads the snapshot written by [`super::persist::write_snapshot`] and
//! reports the outcome of the most recent sync. This is a fast O(1) check
//! — no file I/O beyond reading the small JSON snapshot file.
//!
//! # Mapping
//!
//! The snapshot stores counts (`synced`, `skipped`, `failed`) rather than
//! a single `SyncOutcome` enum. The mapping is:
//!
//! - `failed > 0` → "Failed" (errors blocked the sync)
//! - `synced > 0` → "Completed" (at least one block was written)
//! - `synced == 0 && failed == 0 && skipped > 0` → "UpToDate" (nothing to do)
//! - snapshot file missing → never run (warn)
//! - `schema_version > 1` → unknown future format (warn)
//!
//! This module is read-only: it never writes to the snapshot or recovery
//! files.

use std::fs;
use std::path::Path;

use crate::preflight::CheckResult;

use super::persist::AgentDocSyncSnapshot;

/// Doctor check name used in the report.
pub const CHECK_NAME: &str = "agent_doc_sync";

/// Current snapshot schema version that this reader understands.
const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// Run the `agent_doc_sync` health check.
///
/// Reads `<diagnostics_dir>/agent_doc_sync.json` and returns a
/// [`CheckResult`] indicating the most recent sync outcome:
///
/// - **file missing** → `Warn` ("agent_doc_sync 未运行过")
/// - **`failed > 0`** → `Fail` (sync blocked by errors)
/// - **`synced > 0`** → `Pass` (at least one block written)
/// - **`synced == 0 && failed == 0`** → `Pass` (up to date)
/// - **schema too new** → `Warn` (reader is older than writer)
/// - **I/O / parse error** → `Warn` (snapshot exists but unreadable)
pub fn check_agent_doc_sync_health(diagnostics_dir: &Path) -> CheckResult {
    let path = diagnostics_dir.join("agent_doc_sync.json");

    if !path.exists() {
        return CheckResult::warn(
            CHECK_NAME,
            "agent_doc_sync has not run yet",
            "Snapshot not found — confirm `agent_doc_sync` is enabled in ralph.yml",
        );
    }

    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(err) => {
            return CheckResult::warn(
                CHECK_NAME,
                "agent_doc_sync snapshot unreadable",
                format!("Failed to read {}: {err}", path.display()),
            );
        }
    };

    let snapshot: AgentDocSyncSnapshot = match serde_json::from_str(&raw) {
        Ok(s) => s,
        Err(err) => {
            return CheckResult::warn(
                CHECK_NAME,
                "agent_doc_sync snapshot malformed",
                format!("Failed to parse {}: {err}", path.display()),
            );
        }
    };

    if snapshot.schema_version > SUPPORTED_SCHEMA_VERSION {
        return CheckResult::warn(
            CHECK_NAME,
            "agent_doc_sync snapshot schema too new",
            format!(
                "Snapshot schema_version={} is newer than supported ({}); upgrade ralph-cli",
                snapshot.schema_version, SUPPORTED_SCHEMA_VERSION
            ),
        );
    }

    let last_outcome = classify_outcome(&snapshot);
    match last_outcome {
        SyncOutcome::Failed => CheckResult::fail(
            CHECK_NAME,
            "agent_doc_sync last run failed",
            format!(
                "synced={}, skipped={}, failed={}; see .ralph/diagnostics/recovery.jsonl for details",
                snapshot.synced, snapshot.skipped, snapshot.failed
            ),
        ),
        SyncOutcome::Completed | SyncOutcome::UpToDate => {
            CheckResult::pass(CHECK_NAME, "agent_doc_sync last run succeeded")
        }
    }
}

/// Outcome classification for a single snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncOutcome {
    /// At least one block was synced.
    Completed,
    /// Nothing changed; all blocks up to date.
    UpToDate,
    /// At least one block failed.
    Failed,
}

fn classify_outcome(snapshot: &AgentDocSyncSnapshot) -> SyncOutcome {
    if snapshot.failed > 0 {
        SyncOutcome::Failed
    } else if snapshot.synced > 0 {
        SyncOutcome::Completed
    } else {
        SyncOutcome::UpToDate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preflight::CheckStatus;
    use tempfile::TempDir;

    /// Write a snapshot under `<dir>/.ralph/diagnostics/agent_doc_sync.json`
    /// and return the diagnostics dir so the caller can pass it to the
    /// health check.
    fn write_snapshot(
        workspace: &Path,
        schema_version: u32,
        synced: usize,
        skipped: usize,
        failed: usize,
    ) -> std::path::PathBuf {
        let diag = workspace.join(".ralph").join("diagnostics");
        fs::create_dir_all(&diag).unwrap();
        let snapshot = AgentDocSyncSnapshot {
            schema_version,
            synced,
            skipped,
            failed,
            last_success_at: None,
        };
        let path = diag.join("agent_doc_sync.json");
        let raw = serde_json::to_string_pretty(&snapshot).unwrap();
        fs::write(&path, raw).unwrap();
        diag
    }

    #[test]
    fn missing_snapshot_returns_warn() {
        let dir = TempDir::new().unwrap();
        let diag = dir.path().join(".ralph").join("diagnostics");
        fs::create_dir_all(&diag).unwrap();
        let check = check_agent_doc_sync_health(&diag);
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.message.as_deref().unwrap_or("").contains("not found"));
    }

    #[test]
    fn completed_snapshot_returns_pass() {
        let dir = TempDir::new().unwrap();
        let diag = write_snapshot(dir.path(), 1, 2, 0, 0);
        let check = check_agent_doc_sync_health(&diag);
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn up_to_date_snapshot_returns_pass() {
        let dir = TempDir::new().unwrap();
        // synced=0, skipped=3, failed=0
        let diag = write_snapshot(dir.path(), 1, 0, 3, 0);
        let check = check_agent_doc_sync_health(&diag);
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn failed_snapshot_returns_fail() {
        let dir = TempDir::new().unwrap();
        let diag = write_snapshot(dir.path(), 1, 0, 0, 1);
        let check = check_agent_doc_sync_health(&diag);
        assert_eq!(check.status, CheckStatus::Fail);
        let msg = check.message.as_deref().unwrap_or("");
        assert!(msg.contains("recovery.jsonl"));
    }

    #[test]
    fn unsupported_schema_version_returns_warn() {
        let dir = TempDir::new().unwrap();
        let diag = write_snapshot(dir.path(), 99, 1, 0, 0);
        let check = check_agent_doc_sync_health(&diag);
        assert_eq!(check.status, CheckStatus::Warn);
        let msg = check.message.as_deref().unwrap_or("");
        assert!(msg.contains("schema_version=99"));
    }

    #[test]
    fn malformed_json_returns_warn() {
        let dir = TempDir::new().unwrap();
        let diag = dir.path().join(".ralph").join("diagnostics");
        fs::create_dir_all(&diag).unwrap();
        fs::write(diag.join("agent_doc_sync.json"), b"not valid json").unwrap();
        let check = check_agent_doc_sync_health(&diag);
        assert_eq!(check.status, CheckStatus::Warn);
    }

    #[test]
    fn unreadable_path_returns_warn() {
        // Path exists but is a directory — reading will fail
        let dir = TempDir::new().unwrap();
        let diag = dir.path().join(".ralph").join("diagnostics");
        fs::create_dir_all(&diag).unwrap();
        fs::create_dir(diag.join("agent_doc_sync.json")).unwrap();
        let check = check_agent_doc_sync_health(&diag);
        assert_eq!(check.status, CheckStatus::Warn);
    }
}
