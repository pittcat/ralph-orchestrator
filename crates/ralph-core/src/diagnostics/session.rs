//! Small helpers shared by the diagnostics writer modules.
//!
//! The input bundle (`diagnosis-input.json`) and the planned
//! runtime trace / feedback writers all need a consistent way to
//! check whether a target directory is safe to write to. Centralising
//! the check here means each writer can stay focused on its own
//! schema.

use std::path::Path;

/// Returns `true` when `dir` exists, is a directory, and the
/// current process can create or open a file inside it.
///
/// This is a best-effort capability check used by the input bundle
/// writer to decide whether to fall back to `manifest_status=missing`
/// without attempting the write. The check is intentionally cheap:
/// it does not lock the directory or reserve a file name. Real
/// write failures are still surfaced through the writer's normal
/// error path.
pub fn is_session_dir_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    // Try to create a uniquely-named temp file inside the directory.
    // We never persist it; if creation succeeds the directory is at
    // least writable by us. If it fails (permission denied, EROFS,
    // EXDEV, etc.) we return false and let the writer log a warning.
    match tempfile::Builder::new()
        .prefix(".ralph-dx-writeprobe-")
        .tempfile_in(dir)
    {
        Ok(p) => {
            // Drop closes the file. We do not need to remove it; the
            // session directory is allowed to accumulate probe files
            // (and they are tiny).
            let _ = p.keep();
            true
        }
        Err(_) => false,
    }
}
