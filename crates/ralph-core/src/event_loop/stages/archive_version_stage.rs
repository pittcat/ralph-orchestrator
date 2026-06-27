//! Archive state on worktree reuse (U11).
//!
//! Why this stage exists as a loop-start hook rather than an
//! emit stage: it moves old `.ralph/*.jsonl` files into an
//! archive directory when the same worktree is reused with a
//! new `loop_id`. Without this, a new loop's `TaskWrongLoop`
//! checks would still see old tasks whose `loop_id` differs
//! from the current loop, and the diagnosis summary counts
//! would mix old and new records.
//!
//! Cross-platform / concurrency semantics: macOS / Linux
//! use `std::fs::rename`, which is atomic for same-filesystem
//! moves. Windows requires an extra `fsync(parent_dir)` before
//! rename. Because archive is performed once at loop start,
//! there is no concurrent-write risk; the OS file-lock story
//! in `state::idempotent_log` covers the concurrent-write case.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

const ARCHIVE_DIR: &str = "archive";

/// Error surfaced when archive cannot complete. The caller
/// should abort loop start so the operator can diagnose the
/// workspace state.
#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("workspace path is not absolute: {0}")]
    NotAbsolute(PathBuf),
}

/// Move every `.jsonl` file in `.ralph/` into
/// `.ralph/archive/{old_loop_id}.{ISO8601}/` when the
/// persisted `loop-version.json` says the previous run used a
/// different `loop_id`. Returns the archive directory path.
///
/// If this is the first run (no `loop-version.json`), no
/// archive is created. If the persisted `loop_id` equals the
/// current one, the function is a no-op (resume case).
///
/// The caller is expected to call `IdempotentLog::open` after
/// this, which will write the new `loop-version.json` with the
/// bumped version.
pub fn archive_state_for_loop(
    workspace: &Path,
    current_loop_id: &str,
) -> Result<Option<PathBuf>, ArchiveError> {
    if !workspace.is_absolute() {
        return Err(ArchiveError::NotAbsolute(workspace.to_path_buf()));
    }

    let version_path = workspace.join("loop-version.json");
    if !version_path.exists() {
        // First run in this workspace — nothing to archive.
        return Ok(None);
    }

    let persisted: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&version_path)?)
            .map_err(|e| ArchiveError::Io(io::Error::other(format!("bad loop-version.json: {e}"))))?;
    let old_loop_id = persisted
        .get("loop_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    if old_loop_id == current_loop_id {
        // Resume on the same loop — do not archive.
        return Ok(None);
    }

    let timestamp = chrono::Utc::now().to_rfc3339_opts(
        chrono::SecondsFormat::Micros,
        true,
    );
    let archive_name = format!("{}.{}", old_loop_id, timestamp);
    let archive_dir = workspace.join(ARCHIVE_DIR).join(&archive_name);
    fs::create_dir_all(&archive_dir)?;

    for entry in fs::read_dir(workspace)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        if ext != "jsonl" {
            continue;
        }
        let file_name = path
            .file_name()
            .ok_or_else(|| ArchiveError::Io(io::Error::other("path has no file name")))?;
        let dest = archive_dir.join(file_name);
        fs::rename(&path, &dest)?;
    }

    // loop-version.json itself is left in place —
    // `IdempotentLog::open` will overwrite it with the new
    // version and loop_id after archive completes.

    Ok(Some(archive_dir))
}

#[cfg(test)]
mod tests;