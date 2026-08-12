//! Small helpers shared by the diagnostics writer modules.
//!
//! The input bundle (`diagnosis-input.json`) and the planned
//! runtime trace / feedback writers all need a consistent way to
//! check whether a target directory is safe to write to. Centralising
//! the check here means each writer can stay focused on its own
//! schema.

use std::fs;
use std::io::Write;
use std::path::Path;

/// Returns `true` when `dir` exists, is a directory, and the
/// current process can create, write to, and clean up a file
/// inside it.
///
/// Plan 2026-08-12-001 fix-plan U7 / synth:P1-5: the function
/// has a deliberate **side effect** — it creates and removes a
/// probe file. The old name `is_session_dir_writable` implied a
/// read-only predicate and the implementation called `.keep()`
/// on the `tempfile::Builder` handle, leaking an empty
/// `.ralph-dx-writeprobe-*` file into the session dir on every
/// invocation (and across the three call sites). The new name
/// `probe_session_dir_writable` reflects the side effect, and
/// the implementation explicitly unlinks the probe after the
/// write succeeds so the session dir stays clean.
pub fn probe_session_dir_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    // Create a uniquely-named temp file inside the directory,
    // write one byte, flush, then explicitly unlink it. This
    // exercises the create + write + flush path the writers
    // will hit without leaving artifacts behind.
    match tempfile::NamedTempFile::new_in(dir) {
        Ok(mut f) => {
            let write_ok = f.write_all(b"x").and_then(|()| f.flush()).is_ok();
            let path = f.path().to_path_buf();
            // NamedTempFile's Drop would clean up too, but we
            // want the path removed BEFORE returning so the
            // session dir is byte-clean immediately. Best-effort:
            // a leaked probe file on a chmod 000 dir is
            // self-recovering (Drop runs on scope exit).
            let _ = fs::remove_file(&path);
            write_ok
        }
        Err(_) => false,
    }
}

/// Backwards-compatible alias used by callers that haven't been
/// renamed yet. The name was wrong (implied a read-only
/// predicate) and the implementation leaked files. Retained so
/// a `cargo fix` pass can mechanically rename callers — every
/// call site should migrate to [`probe_session_dir_writable`].
#[deprecated(note = "renamed to probe_session_dir_writable (fix-plan U7)")]
pub fn is_session_dir_writable(dir: &Path) -> bool {
    probe_session_dir_writable(dir)
}
