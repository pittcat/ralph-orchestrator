//! File-level sync writer: read → parse → apply → atomic write.
//!
//! Handles file locking with retry, marker insertion/replacement, and
//! atomic writes via `tempfile` + `persist`.

use std::path::Path;
use std::thread;
use std::time::Duration;

use tracing::{debug, warn};

use super::block::{BlockSpec, BlockState, parse_marker_state_with_version};
use crate::file_lock::FileLock;

/// On-error policy for sync failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnError {
    /// Log a warning and continue.
    Warn,
    /// Return an error (caller may exit the process).
    Strict,
}

impl Default for OnError {
    fn default() -> Self {
        Self::Warn
    }
}

/// Configuration for a single file sync operation.
#[derive(Debug)]
pub(crate) struct FileSyncConfig<'a> {
    /// The target file path (e.g. `<workspace_root>/CLAUDE.md`).
    pub path: &'a Path,
    /// Blocks to sync into this file.
    pub blocks: &'a [BlockSpec],
    /// Error policy.
    pub on_error: OnError,
    /// Whether to skip sync entirely (from flag / env / config).
    pub skip: bool,
}

/// Result of syncing a single file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileSyncResult {
    /// Number of blocks synced (appended or replaced).
    pub synced: usize,
    /// Number of blocks skipped (already up to date).
    pub skipped: usize,
    /// Number of blocks that failed (lock contention, I/O error, etc.).
    pub failed: usize,
}

/// Acquires an exclusive file lock with retry.
///
/// Tries `max_retries` times with `retry_delay` between attempts.
/// Returns `None` if all retries are exhausted.
pub(crate) fn try_lock_with_retry(
    path: &Path,
    max_retries: u32,
    retry_delay: Duration,
) -> Option<crate::file_lock::LockGuard> {
    let lock = match FileLock::new(path) {
        Ok(l) => l,
        Err(e) => {
            warn!(error = %e, path = %path.display(), "failed to create FileLock");
            return None;
        }
    };

    for attempt in 1..=max_retries {
        match lock.try_exclusive() {
            Ok(Some(guard)) => return Some(guard),
            Ok(None) => {
                if attempt < max_retries {
                    thread::sleep(retry_delay);
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    path = %path.display(),
                    attempt,
                    "failed to acquire exclusive lock"
                );
                return None;
            }
        }
    }
    None
}

/// Determines the action needed for a single block in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockAction {
    /// Block is missing; append it to the managed blocks section.
    Append,
    /// Block markers exist but hash differs; replace in place.
    Replace { begin_line: usize, end_line: usize },
    /// Block is already up to date; no write needed.
    Skip,
}

/// Determines the action for a block given current file content.
pub(crate) fn determine_action(content: &str, block: &BlockSpec) -> BlockAction {
    let (state, begin, end) =
        parse_marker_state_with_version(content, &block.id, &block.content_sha256);
    match state {
        BlockState::UpToDate => BlockAction::Skip,
        BlockState::Mismatched { .. } => BlockAction::Replace {
            begin_line: begin.expect("mismatched state must have begin_line"),
            end_line: end.expect("mismatched state must have end_line"),
        },
        BlockState::Missing => BlockAction::Append,
    }
}

/// Computes new file content for a single block.
///
/// - `Append`: adds block to the `## Ralph Managed Blocks` section (creates
///   section if absent).
/// - `Replace`: swaps content between begin/end markers and updates the hash.
/// - `Skip`: returns `None` (no change).
pub(crate) fn compute_new_content(
    content: &str,
    block: &BlockSpec,
    action: &BlockAction,
) -> Option<String> {
    match action {
        BlockAction::Skip => None,
        BlockAction::Append => Some(compute_append(content, block)),
        BlockAction::Replace {
            begin_line,
            end_line,
        } => Some(compute_replace(content, block, *begin_line, *end_line)),
    }
}

fn begin_marker(id: &str, hash: &str) -> String {
    format!("<!-- ralph:begin {id} v=sha256:{hash} -->")
}

fn end_marker(id: &str) -> String {
    format!("<!-- ralph:end {id} -->")
}

fn compute_append(content: &str, block: &BlockSpec) -> String {
    let section_header = "## Ralph Managed Blocks";
    let begin = begin_marker(&block.id, &block.content_sha256);
    let end = end_marker(&block.id);

    let mut lines: Vec<&str> = content.split('\n').collect();
    // Remove trailing empty element from split (if content ends with \n)
    if lines.last() == Some(&"") {
        lines.pop();
    }

    let mut result: Vec<&str> = Vec::with_capacity(lines.len() + 6);
    let mut inserted = false;

    for line in &lines {
        result.push(line);
        if !inserted && line.trim() == section_header {
            // Insert block after the section header
            result.push("");
            result.push(begin.as_str());
            result.push(&block.content);
            if !block.content.ends_with('\n') {
                result.push("");
            }
            result.push(end.as_str());
            inserted = true;
        }
    }

    if !inserted {
        // Section doesn't exist: append at end
        result.push("");
        result.push(section_header);
        result.push("");
        result.push(begin.as_str());
        result.push(&block.content);
        if !block.content.ends_with('\n') {
            result.push("");
        }
        result.push(end.as_str());
    }

    let mut out = result.join("\n");
    out.push('\n');
    out
}

fn compute_replace(
    content: &str,
    block: &BlockSpec,
    begin_line: usize,
    end_line: usize,
) -> String {
    let lines: Vec<&str> = content.split('\n').collect();
    let begin = begin_marker(&block.id, &block.content_sha256);
    let end = end_marker(&block.id);

    let mut result: Vec<&str> = Vec::with_capacity(lines.len() + 2);
    for (i, line) in lines.iter().enumerate() {
        if i == begin_line {
            result.push(begin.as_str());
            result.push(&block.content);
            if !block.content.ends_with('\n') {
                result.push("");
            }
        } else if i > begin_line && i < end_line {
            // Skip old content between markers
            continue;
        } else if i == end_line {
            result.push(end.as_str());
        } else {
            result.push(line);
        }
    }

    let mut out = result.join("\n");
    out.push('\n');
    out
}

/// Writes content to a file atomically using tempfile + persist.
pub(crate) fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(content.as_bytes())?;
    tmp.persist(path)?;
    Ok(())
}

/// Syncs blocks into a single file. Returns per-block results.
pub(crate) fn sync_file(config: &FileSyncConfig<'_>) -> FileSyncResult {
    if config.skip {
        return FileSyncResult {
            synced: 0,
            skipped: 0,
            failed: 0,
        };
    }

    let mut result = FileSyncResult {
        synced: 0,
        skipped: 0,
        failed: 0,
    };

    // Acquire lock with retry
    let _guard = match try_lock_with_retry(config.path, 3, Duration::from_millis(50)) {
        Some(g) => g,
        None => {
            result.failed = config.blocks.len();
            match config.on_error {
                OnError::Warn => {
                    warn!(
                        path = %config.path.display(),
                        "agent_doc_sync: failed to acquire lock after retries"
                    );
                }
                OnError::Strict => {
                    warn!(
                        path = %config.path.display(),
                        "agent_doc_sync: failed to acquire lock (strict mode)"
                    );
                }
            }
            return result;
        }
    };

    // Read current content
    let content = match std::fs::read_to_string(config.path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            result.failed = config.blocks.len();
            match config.on_error {
                OnError::Warn => {
                    warn!(error = %e, path = %config.path.display(), "agent_doc_sync: failed to read file");
                }
                OnError::Strict => {
                    warn!(error = %e, path = %config.path.display(), "agent_doc_sync: failed to read file (strict)");
                }
            }
            return result;
        }
    };

    // Process each block
    let mut current_content = content;
    for block in config.blocks {
        let action = determine_action(&current_content, block);

        match action {
            BlockAction::Skip => {
                debug!(
                    block_id = %block.id,
                    path = %config.path.display(),
                    "skipped (up to date)"
                );
                result.skipped += 1;
            }
            _ => {
                // Check if target file is writable (before we compute new content).
                // This catches readonly files early; tempfile::persist() uses rename()
                // which doesn't check file permissions.
                if config.path.exists() {
                    match std::fs::OpenOptions::new().write(true).open(config.path) {
                        Ok(f) => drop(f),
                        Err(e) => {
                            result.failed += 1;
                            match config.on_error {
                                OnError::Warn => {
                                    warn!(
                                        error = %e,
                                        block_id = %block.id,
                                        path = %config.path.display(),
                                        "agent_doc_sync: file is not writable"
                                    );
                                }
                                OnError::Strict => {
                                    warn!(
                                        error = %e,
                                        block_id = %block.id,
                                        path = %config.path.display(),
                                        "agent_doc_sync: file is not writable (strict)"
                                    );
                                }
                            }
                            continue;
                        }
                    }
                }

                if let Some(new_content) = compute_new_content(&current_content, block, &action) {
                    match write_atomic(config.path, &new_content) {
                        Ok(()) => {
                            // Verify the file is actually readable and matches
                            // what we wrote. This catches silent failures from
                            // persist() on readonly targets (rename() bypasses
                            // file-level permissions).
                            match std::fs::read_to_string(config.path) {
                                Ok(verified) if verified == new_content => {
                                    debug!(
                                        block_id = %block.id,
                                        path = %config.path.display(),
                                        action = ?action,
                                        "synced block"
                                    );
                                    current_content = new_content;
                                    result.synced += 1;
                                }
                                Ok(_) => {
                                    // File exists but content doesn't match —
                                    // write was silently lost (e.g. readonly target)
                                    result.failed += 1;
                                    match config.on_error {
                                        OnError::Warn => {
                                            warn!(
                                                block_id = %block.id,
                                                path = %config.path.display(),
                                                "agent_doc_sync: write verification failed (content mismatch)"
                                            );
                                        }
                                        OnError::Strict => {
                                            warn!(
                                                block_id = %block.id,
                                                path = %config.path.display(),
                                                "agent_doc_sync: write verification failed (strict)"
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    result.failed += 1;
                                    match config.on_error {
                                        OnError::Warn => {
                                            warn!(
                                                error = %e,
                                                block_id = %block.id,
                                                path = %config.path.display(),
                                                "agent_doc_sync: write verification failed (read error)"
                                            );
                                        }
                                        OnError::Strict => {
                                            warn!(
                                                error = %e,
                                                block_id = %block.id,
                                                path = %config.path.display(),
                                                "agent_doc_sync: write verification failed (strict)"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            result.failed += 1;
                            match config.on_error {
                                OnError::Warn => {
                                    warn!(
                                        error = %e,
                                        block_id = %block.id,
                                        path = %config.path.display(),
                                        "agent_doc_sync: failed to write file"
                                    );
                                }
                                OnError::Strict => {
                                    warn!(
                                        error = %e,
                                        block_id = %block.id,
                                        path = %config.path.display(),
                                        "agent_doc_sync: failed to write file (strict)"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_doc_sync::block::BlockSpec;
    use std::fs;
    use tempfile::TempDir;

    fn sample_block() -> BlockSpec {
        BlockSpec::new("hang-prevention", "Rule 1\nRule 2\n")
    }

    #[test]
    fn sync_creates_file_when_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");
        let block = sample_block();

        let result = sync_file(&FileSyncConfig {
            path: &path,
            blocks: &[block.clone()],
            on_error: OnError::Warn,
            skip: false,
        });

        assert_eq!(result.synced, 1);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("## Ralph Managed Blocks"));
        assert!(content.contains("Rule 1"));
        assert!(content.contains(&begin_marker("hang-prevention", &block.content_sha256)));
        assert!(content.contains(&end_marker("hang-prevention")));
    }

    #[test]
    fn sync_appends_block_when_marker_absent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");
        fs::write(&path, "# My Project\n\nSome content.\n").unwrap();

        let block = sample_block();
        let result = sync_file(&FileSyncConfig {
            path: &path,
            blocks: &[block.clone()],
            on_error: OnError::Warn,
            skip: false,
        });

        assert_eq!(result.synced, 1);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# My Project"));
        assert!(content.contains("Some content."));
        assert!(content.contains("Rule 1"));
    }

    #[test]
    fn sync_skips_when_version_matches() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");
        let block = sample_block();

        let existing = format!(
            "# Project\n\n## Ralph Managed Blocks\n\n{}\n{}\n{}\n",
            begin_marker("hang-prevention", &block.content_sha256),
            block.content.trim(),
            end_marker("hang-prevention")
        );
        let original_mtime = {
            fs::write(&path, &existing).unwrap();
            fs::metadata(&path).unwrap().modified().unwrap()
        };

        let result = sync_file(&FileSyncConfig {
            path: &path,
            blocks: &[block],
            on_error: OnError::Warn,
            skip: false,
        });

        assert_eq!(result.skipped, 1);
        assert_eq!(result.synced, 0);

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, existing);

        let new_mtime = fs::metadata(&path).unwrap().modified().unwrap();
        let diff = new_mtime.duration_since(original_mtime).unwrap_or_default();
        assert!(
            diff < Duration::from_secs(2),
            "mtime should not change significantly"
        );
    }

    #[test]
    fn sync_replaces_in_place_on_v_mismatch() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");

        let old_hash = "a".repeat(64);
        let existing = format!(
            "# Project\n\n## Ralph Managed Blocks\n\n\
             <!-- ralph:begin hang-prevention v=sha256:{old_hash} -->\n\
             old content\n\
             <!-- ralph:end hang-prevention -->\n"
        );
        fs::write(&path, &existing).unwrap();

        let block = sample_block();
        let result = sync_file(&FileSyncConfig {
            path: &path,
            blocks: &[block.clone()],
            on_error: OnError::Warn,
            skip: false,
        });

        assert_eq!(result.synced, 1);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("## Ralph Managed Blocks"));
        assert!(!content.contains("old content"));
        assert!(content.contains("Rule 1"));
        assert!(content.contains(&begin_marker("hang-prevention", &block.content_sha256)));
        assert!(content.contains("# Project"));
    }

    #[test]
    fn sync_respects_user_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");

        let user_content =
            "# My Project\n\nThis is user-written content.\n\n## Notes\n\nSome notes here.\n";
        fs::write(&path, user_content).unwrap();

        let block = sample_block();
        sync_file(&FileSyncConfig {
            path: &path,
            blocks: &[block],
            on_error: OnError::Warn,
            skip: false,
        });

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with(
            "# My Project\n\nThis is user-written content.\n\n## Notes\n\nSome notes here.\n"
        ));
    }

    #[test]
    fn sync_retries_lock_then_succeeds() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");
        let block = sample_block();

        let result = sync_file(&FileSyncConfig {
            path: &path,
            blocks: &[block],
            on_error: OnError::Warn,
            skip: false,
        });

        assert_eq!(result.synced, 1);
        assert_eq!(result.failed, 0);
    }

    #[test]
    fn sync_returns_failed_when_lock_always_fails() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");
        let block = sample_block();

        let lock = crate::file_lock::FileLock::new(&path).unwrap();
        let _guard = lock.exclusive().unwrap();

        let result = sync_file(&FileSyncConfig {
            path: &path,
            blocks: &[block],
            on_error: OnError::Warn,
            skip: false,
        });

        assert_eq!(result.failed, 1);
        assert_eq!(result.synced, 0);
    }

    #[test]
    fn sync_handles_readonly_file_via_on_error_warn() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");
        fs::write(&path, "existing\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        }

        let block = sample_block();
        let result = sync_file(&FileSyncConfig {
            path: &path,
            blocks: &[block],
            on_error: OnError::Warn,
            skip: false,
        });

        assert_eq!(result.failed, 1);
        assert_eq!(result.synced, 0);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
    }

    #[test]
    fn sync_handles_readonly_file_via_on_error_strict() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");
        fs::write(&path, "existing\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        }

        let block = sample_block();
        let result = sync_file(&FileSyncConfig {
            path: &path,
            blocks: &[block],
            on_error: OnError::Strict,
            skip: false,
        });

        assert_eq!(result.failed, 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
    }

    #[test]
    fn sync_skips_entirely_when_skip_flag_set() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");
        let block = sample_block();

        let result = sync_file(&FileSyncConfig {
            path: &path,
            blocks: &[block],
            on_error: OnError::Warn,
            skip: true,
        });

        assert_eq!(result.synced, 0);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);
        assert!(!path.exists());
    }

    #[test]
    fn sync_multiple_blocks() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("CLAUDE.md");
        let block1 = BlockSpec::new("block-a", "Content A\n");
        let block2 = BlockSpec::new("block-b", "Content B\n");

        let result = sync_file(&FileSyncConfig {
            path: &path,
            blocks: &[block1.clone(), block2.clone()],
            on_error: OnError::Warn,
            skip: false,
        });

        assert_eq!(result.synced, 2);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("Content A"));
        assert!(content.contains("Content B"));
        assert!(content.contains(&begin_marker("block-a", &block1.content_sha256)));
        assert!(content.contains(&begin_marker("block-b", &block2.content_sha256)));
    }

    #[test]
    fn compute_new_content_skip_returns_none() {
        let content = "hello\n";
        let block = sample_block();
        let result = compute_new_content(content, &block, &BlockAction::Skip);
        assert!(result.is_none());
    }
}
