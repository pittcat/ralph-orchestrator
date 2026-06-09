//! Synchronizes managed agent doc blocks into `CLAUDE.md` / `AGENTS.md`.
//!
//! This module implements the sync engine that runs **synchronously**
//! (not via `tokio::spawn`) before backend spawn in `ralph run`. It
//! injects curated constraint blocks (such as "Command Hang Prevention
//! Rules") into agent-facing markdown files, delimited by versioned
//! HTML-comment markers for idempotent, upgradable, escapable operation.
//!
//! # Architecture
//!
//! ```text
//! sync_all(workspace_root, blocks, config)
//!   ├── for each file (CLAUDE.md, AGENTS.md):
//!   │     ├── try_lock_with_retry (FileLock::try_exclusive, 3×50ms)
//!   │     ├── read_to_string (or empty if missing)
//!   │     ├── for each block:
//!   │     │     ├── determine_action (Missing / Mismatched / UpToDate)
//!   │     │     └── compute_new_content (append / replace / skip)
//!   │     └── write_atomic (tempfile + persist)
//!   └── return SyncReport
//! ```
//!
//! # Marker format
//!
//! ```text
//! <!-- ralph:begin <block_id> v=sha256:<64hex> -->
//! <content>
//! <!-- ralph:end <block_id> -->
//! ```

pub mod block;
pub mod builtin;
pub mod writer;

use std::path::Path;

pub use block::BlockSpec;
pub use writer::{OnError, SyncError};
use writer::FileSyncConfig;

use tracing::{debug, info};

/// Aggregated result of syncing all target files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Total blocks synced (appended or replaced) across all files.
    pub synced: usize,
    /// Total blocks skipped (already up to date) across all files.
    pub skipped: usize,
    /// Total blocks that failed across all files.
    pub failed: usize,
    /// Per-block results (one entry per (file, block) pair).
    pub block_results: Vec<BlockResult>,
}

/// Result of syncing a single block into a single file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockResult {
    /// Block identifier.
    pub block_id: String,
    /// Target file path.
    pub file: String,
    /// Outcome of the sync operation.
    pub outcome: SyncOutcome,
}

/// Outcome of syncing a single block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Block was appended or replaced successfully.
    Synced,
    /// Block was already up to date (hash matched).
    Skipped,
    /// Sync failed (lock contention, I/O error, etc.).
    Failed,
}

/// Configuration for `sync_all`.
#[derive(Debug)]
pub struct SyncConfig<'a> {
    /// Whether to skip sync entirely.
    pub skip: bool,
    /// Error policy.
    pub on_error: OnError,
    /// Target files to sync into (typically `["CLAUDE.md", "AGENTS.md"]`).
    pub target_files: &'a [&'a str],
    /// Blocks to inject.
    pub blocks: &'a [BlockSpec],
}

/// Synchronously injects managed blocks into agent doc files.
///
/// This function performs blocking I/O and must complete before the
/// backend is spawned. It is **not** suitable for use inside an async
/// runtime without `spawn_blocking`.
///
/// # Arguments
///
/// * `workspace_root` — Root directory of the workspace (may be a worktree).
/// * `config` — Sync configuration (skip flag, error policy, targets, blocks).
///
/// # Returns
///
/// A [`SyncReport`] summarizing the outcome of all sync operations.
/// In `OnError::Strict` mode, returns `Err` if any file sync fails.
pub fn sync_all(
    workspace_root: &Path,
    config: &SyncConfig<'_>,
) -> Result<SyncReport, writer::SyncError> {
    if config.skip {
        debug!("agent_doc_sync: skipped (disabled via flag/env/config)");
        return Ok(SyncReport {
            synced: 0,
            skipped: 0,
            failed: 0,
            block_results: Vec::new(),
        });
    }

    let mut report = SyncReport {
        synced: 0,
        skipped: 0,
        failed: 0,
        block_results: Vec::new(),
    };

    for file_name in config.target_files {
        let path = workspace_root.join(file_name);
        let file_result = writer::sync_file(&FileSyncConfig {
            path: &path,
            blocks: config.blocks,
            on_error: config.on_error,
            skip: false,
        })?;

        report.synced += file_result.synced;
        report.skipped += file_result.skipped;
        report.failed += file_result.failed;

        for block_outcome in &file_result.block_results {
            report.block_results.push(BlockResult {
                block_id: block_outcome.block_id.clone(),
                file: file_name.to_string(),
                outcome: block_outcome.outcome.clone(),
            });
        }

        debug!(
            file = %file_name,
            synced = file_result.synced,
            skipped = file_result.skipped,
            failed = file_result.failed,
            "agent_doc_sync: file sync complete"
        );
    }

    if report.synced > 0 {
        info!(
            synced = report.synced,
            skipped = report.skipped,
            failed = report.failed,
            "agent_doc_sync: complete"
        );
    } else if report.skipped > 0 {
        debug!(
            skipped = report.skipped,
            "agent_doc_sync: all blocks up to date"
        );
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn sample_block() -> BlockSpec {
        BlockSpec::new("hang-prevention", "Rule 1\nRule 2\nRule 3\nRule 4\nRule 5\n")
    }

    #[test]
    fn sync_creates_section_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let block = sample_block();

        let report = sync_all(
            dir.path(),
            &SyncConfig {
                skip: false,
                on_error: OnError::Warn,
                target_files: &["CLAUDE.md"],
                blocks: &[block.clone()],
            },
        )
        .unwrap();

        assert_eq!(report.synced, 1);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.failed, 0);

        let path = dir.path().join("CLAUDE.md");
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("## Ralph Managed Blocks"));
        assert!(content.contains("Rule 1"));
    }

    #[test]
    fn sync_appends_block_when_marker_absent() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("CLAUDE.md"), "# Project\n").unwrap();

        let block = sample_block();
        let report = sync_all(
            dir.path(),
            &SyncConfig {
                skip: false,
                on_error: OnError::Warn,
                target_files: &["CLAUDE.md"],
                blocks: &[block],
            },
        )
        .unwrap();

        assert_eq!(report.synced, 1);
        let content = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert!(content.contains("# Project"));
        assert!(content.contains("Rule 1"));
    }

    #[test]
    fn sync_skips_when_v_matches() {
        let dir = TempDir::new().unwrap();
        let block = sample_block();

        let existing = format!(
            "# Project\n\n## Ralph Managed Blocks\n\n\
             <!-- ralph:begin hang-prevention v=sha256:{} -->\n\
             Rule 1\nRule 2\nRule 3\nRule 4\nRule 5\n\
             <!-- ralph:end hang-prevention -->\n",
            block.content_sha256
        );
        fs::write(dir.path().join("CLAUDE.md"), &existing).unwrap();

        let report = sync_all(
            dir.path(),
            &SyncConfig {
                skip: false,
                on_error: OnError::Warn,
                target_files: &["CLAUDE.md"],
                blocks: &[block],
            },
        )
        .unwrap();

        assert_eq!(report.skipped, 1);
        assert_eq!(report.synced, 0);
    }

    #[test]
    fn sync_replaces_in_place_on_v_mismatch() {
        let dir = TempDir::new().unwrap();
        let old_hash = "a".repeat(64);

        let existing = format!(
            "# Project\n\n## Ralph Managed Blocks\n\n\
             <!-- ralph:begin hang-prevention v=sha256:{old_hash} -->\n\
             old\n\
             <!-- ralph:end hang-prevention -->\n"
        );
        fs::write(dir.path().join("CLAUDE.md"), &existing).unwrap();

        let block = sample_block();
        let report = sync_all(
            dir.path(),
            &SyncConfig {
                skip: false,
                on_error: OnError::Warn,
                target_files: &["CLAUDE.md"],
                blocks: &[block],
            },
        )
        .unwrap();

        assert_eq!(report.synced, 1);
        let content = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert!(content.contains("## Ralph Managed Blocks"));
        assert!(!content.contains("old\n"));
        assert!(content.contains("Rule 1"));
        assert!(content.contains("# Project"));
    }

    #[test]
    fn sync_respects_user_content() {
        let dir = TempDir::new().unwrap();
        let user_content = "# My Project\n\nUser notes.\n\n## Custom Section\n\nDetails.\n";
        fs::write(dir.path().join("CLAUDE.md"), user_content).unwrap();

        let block = sample_block();
        sync_all(
            dir.path(),
            &SyncConfig {
                skip: false,
                on_error: OnError::Warn,
                target_files: &["CLAUDE.md"],
                blocks: &[block],
            },
        )
        .unwrap();

        let content = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        // User content at the start is preserved
        assert!(content.starts_with("# My Project\n\nUser notes.\n\n## Custom Section\n\nDetails.\n"));
    }

    #[test]
    fn sync_handles_both_files() {
        let dir = TempDir::new().unwrap();
        let block = sample_block();

        let report = sync_all(
            dir.path(),
            &SyncConfig {
                skip: false,
                on_error: OnError::Warn,
                target_files: &["CLAUDE.md", "AGENTS.md"],
                blocks: &[block],
            },
        )
        .unwrap();

        assert_eq!(report.synced, 2);
        assert!(dir.path().join("CLAUDE.md").exists());
        assert!(dir.path().join("AGENTS.md").exists());
    }

    #[test]
    fn sync_skips_when_skip_flag_set() {
        let dir = TempDir::new().unwrap();
        let block = sample_block();

        let report = sync_all(
            dir.path(),
            &SyncConfig {
                skip: true,
                on_error: OnError::Warn,
                target_files: &["CLAUDE.md"],
                blocks: &[block],
            },
        )
        .unwrap();

        assert_eq!(report.synced, 0);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.failed, 0);
        assert!(!dir.path().join("CLAUDE.md").exists());
    }

    #[test]
    fn sync_returns_failed_after_3_lock_retries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");
        let block = sample_block();

        // Hold exclusive lock
        let lock = crate::file_lock::FileLock::new(&path).unwrap();
        let _guard = lock.exclusive().unwrap();

        let report = sync_all(
            dir.path(),
            &SyncConfig {
                skip: false,
                on_error: OnError::Warn,
                target_files: &["CLAUDE.md"],
                blocks: &[block],
            },
        )
        .unwrap();

        assert_eq!(report.failed, 1);
    }

    #[test]
    fn sync_strict_mode_propagates_lock_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");
        let block = sample_block();

        // Hold exclusive lock
        let lock = crate::file_lock::FileLock::new(&path).unwrap();
        let _guard = lock.exclusive().unwrap();

        let err = sync_all(
            dir.path(),
            &SyncConfig {
                skip: false,
                on_error: OnError::Strict,
                target_files: &["CLAUDE.md"],
                blocks: &[block],
            },
        )
        .unwrap_err();

        assert!(
            matches!(err, writer::SyncError::LockFailed { .. }),
            "expected LockFailed, got: {err:?}"
        );
    }

    #[test]
    fn block_result_structure() {
        let dir = TempDir::new().unwrap();
        let block = sample_block();

        let report = sync_all(
            dir.path(),
            &SyncConfig {
                skip: false,
                on_error: OnError::Warn,
                target_files: &["CLAUDE.md"],
                blocks: &[block],
            },
        )
        .unwrap();

        assert_eq!(report.block_results.len(), 1);
        assert_eq!(report.block_results[0].block_id, "hang-prevention");
        assert_eq!(report.block_results[0].file, "CLAUDE.md");
    }

    #[test]
    fn block_result_skipped_when_up_to_date() {
        let dir = TempDir::new().unwrap();
        let block = sample_block();

        // Pre-populate with an up-to-date block
        let existing = format!(
            "# Project\n\n## Ralph Managed Blocks\n\n\
             <!-- ralph:begin hang-prevention v=sha256:{} -->\n\
             Rule 1\nRule 2\nRule 3\nRule 4\nRule 5\n\
             <!-- ralph:end hang-prevention -->\n",
            block.content_sha256
        );
        fs::write(dir.path().join("CLAUDE.md"), &existing).unwrap();

        let report = sync_all(
            dir.path(),
            &SyncConfig {
                skip: false,
                on_error: OnError::Warn,
                target_files: &["CLAUDE.md"],
                blocks: &[block],
            },
        )
        .unwrap();

        assert_eq!(report.skipped, 1);
        assert_eq!(report.synced, 0);
        assert_eq!(report.block_results.len(), 1);
        assert_eq!(report.block_results[0].block_id, "hang-prevention");
        assert_eq!(report.block_results[0].file, "CLAUDE.md");
        assert_eq!(report.block_results[0].outcome, SyncOutcome::Skipped);
    }

    #[test]
    fn sync_replaces_orphan_begin_marker_no_duplication() {
        let dir = TempDir::new().unwrap();
        let block = sample_block();

        // File has an orphan begin marker (no matching end marker)
        let orphan_hash = "b".repeat(64);
        let existing = format!(
            "# Project\n\n## Ralph Managed Blocks\n\n\
             <!-- ralph:begin hang-prevention v=sha256:{orphan_hash} -->\n\
             stale orphan content\n"
        );
        fs::write(dir.path().join("CLAUDE.md"), &existing).unwrap();

        let report = sync_all(
            dir.path(),
            &SyncConfig {
                skip: false,
                on_error: OnError::Warn,
                target_files: &["CLAUDE.md"],
                blocks: &[block.clone()],
            },
        )
        .unwrap();

        assert_eq!(report.synced, 1);
        let content = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();

        // Should contain exactly ONE begin marker
        let begin_count = content.matches("<!-- ralph:begin hang-prevention").count();
        assert_eq!(begin_count, 1, "expected exactly 1 begin marker, found {begin_count}");

        // Should have proper markers
        assert!(content.contains("Rule 1"));
        assert!(content.contains("## Ralph Managed Blocks"));
    }

    #[test]
    fn sync_replaces_orphan_begin_with_matching_hash_and_preserves_user_content() {
        let dir = TempDir::new().unwrap();
        let block = sample_block();

        // File has orphan begin marker with CORRECT hash + user content after
        let existing = format!(
            "# Project\n\n## Ralph Managed Blocks\n\n\
             <!-- ralph:begin hang-prevention v=sha256:{} -->\n\
             stale orphan content\n\
             ## Notes\n\nUser-written notes.\n",
            block.content_sha256
        );
        fs::write(dir.path().join("CLAUDE.md"), &existing).unwrap();

        let report = sync_all(
            dir.path(),
            &SyncConfig {
                skip: false,
                on_error: OnError::Warn,
                target_files: &["CLAUDE.md"],
                blocks: &[block.clone()],
            },
        )
        .unwrap();

        assert_eq!(report.synced, 1);
        let content = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();

        // User content after orphan should be preserved
        assert!(content.contains("## Notes"));
        assert!(content.contains("User-written notes."));
        // New block should be properly placed
        assert!(content.contains("Rule 1"));
        assert!(content.contains("## Ralph Managed Blocks"));
        let begin_count = content.matches("<!-- ralph:begin hang-prevention").count();
        assert_eq!(begin_count, 1, "expected exactly 1 begin marker, found {begin_count}");
    }
}
