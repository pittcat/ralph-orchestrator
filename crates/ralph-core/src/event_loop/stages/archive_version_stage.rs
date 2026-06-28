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
/// Called from `EventLoop::new` (via `with_context_and_diagnostics`)
/// exactly once at loop start, **before** `IdempotentLog::open` writes
/// the new `loop-version.json`. Best-effort: callers warn on error
/// and continue (the loop must not panic on archive failure).
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
        // 2026-06-28-002 U4: first run used to return `Ok(None)`
        // and never wrote `loop-version.json`. That made U11 a
        // permanent no-op on fresh workspaces because
        // `IdempotentLog::open` was the only other writer, and
        // it was gated behind `state_idempotency: required`.
        // We now write the initial `{"loop_id": ..., "version": 1}`
        // marker here so downstream stages see the canonical
        // version file regardless of whether idempotent log
        // is enabled.
        // Ensure the parent directory exists — a fresh worktree
        // does not have `.ralph/` until the loop starts.
        if let Some(parent) = version_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let initial = serde_json::json!({
            "loop_id": current_loop_id,
            "version": 1,
        });
        fs::write(&version_path, serde_json::to_string_pretty(&initial).map_err(|e| {
            ArchiveError::Io(io::Error::other(format!(
                "failed to serialise initial loop-version.json: {e}"
            )))
        })?)?;
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
        let file_name = path
            .file_name()
            .ok_or_else(|| ArchiveError::Io(io::Error::other("path has no file name")))?;
        // P1-8 (2026-06-27 adversarial review):
        // skip the archive directory itself so a
        // re-archive on the same workspace does not
        // re-archive archived files.
        if path.is_dir() {
            if file_name == ARCHIVE_DIR {
                continue;
            }
            // Recurse into subdirectories so
            // `.ralph/agent/*.jsonl` (and any
            // other JSONL living in a subdir) is
            // archived too. The relative path is
            // preserved under the archive
            // directory so the structure mirrors
            // the source workspace.
            archive_dir_recursive(&path, &archive_dir, file_name)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        if ext != "jsonl" {
            continue;
        }
        let dest = archive_dir.join(file_name);
        fs::rename(&path, &dest)?;
    }

    // loop-version.json itself is left in place —
    // `IdempotentLog::open` will overwrite it with the new
    // version and loop_id after archive completes.

    Ok(Some(archive_dir))
}

/// P1-8 (2026-06-27 adversarial review): recursive
/// helper that mirrors a source directory subtree
/// into the archive directory. The relative path
/// is preserved so the archive mirrors the source
/// structure (`workspace/sub/a.jsonl` →
/// `workspace/archive/<id>/sub/a.jsonl`).
fn archive_dir_recursive(
    src: &Path,
    dest_root: &Path,
    relative_name: &std::ffi::OsStr,
) -> Result<(), ArchiveError> {
    let dest = dest_root.join(relative_name);
    fs::create_dir_all(&dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = path
            .file_name()
            .ok_or_else(|| ArchiveError::Io(io::Error::other("path has no file name")))?;
        if path.is_dir() {
            // Bound the recursion: skip the
            // archive directory itself to avoid
            // re-archiving archived files when
            // the archive directory is a
            // subdirectory of the workspace.
            if file_name == ARCHIVE_DIR {
                continue;
            }
            archive_dir_recursive(&path, &dest, file_name)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        if ext != "jsonl" {
            continue;
        }
        let target = dest.join(file_name);
        fs::rename(&path, &target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;