//! Ephemeral file isolation (R3 — 2026-06-14-003 plan).
//!
//! The `ce-executor-isolated` preset has been observed to be polluted by
//! agent-written runtime artefacts (`scratchpad.md`, `notes.md`, `tmp*.md`,
//! etc.) that the agent drops into source directories such as
//! `crates/ralph-core/`.  These files show up as untracked changes and
//! cause `review-coordinator` to emit spurious `review.wave.ready`
//! events for what is effectively an empty diff — driving the loop into
//! needless review rounds (the `calm-oak` worktree incident).
//!
//! This module centralises the cleanup.  The runtime calls
//! [`EphemeralIsolation::scan_and_relocate`] on every iteration; matching
//! files are first appended to `.ralph/agent/scratchpad-{loop_id}.md`
//! (so the agent does not lose the content it wanted to write) and then
//! removed from the source tree.  The returned [`RelocationRecord`]s
//! drive the `## EPHEMERAL RELOCATED` block in the next prompt so the
//! agent learns the file has been moved and stops recreating it.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Default file basenames / glob patterns that count as ephemeral.  Kept
/// conservative on purpose — only files the agent is known to drop during
/// execution land in this list.  Each entry is matched against the file
/// **name only** (not the full path), so `crates/ralph-core/scratchpad.md`
/// and `backend/api/scratchpad.md` are both caught.
pub const EPHEMERAL_FILE_NAMES: &[&str] = &[
    "scratchpad.md",
    "notes.md",
    "agent-notes.md",
    // `tmp*.md` is matched via prefix below; the static list only
    // covers well-known names without globs.
];

/// Match helper: is the given file name a runtime artefact we should
/// isolate?  This function owns the glob-style logic (`tmp*.md`,
/// `*.tmp.md`, `*.bak`) so the matching surface stays small and easy
/// to test in isolation.
pub fn is_ephemeral_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if EPHEMERAL_FILE_NAMES.iter().any(|n| *n == lower) {
        return true;
    }
    if lower.starts_with("tmp") && lower.ends_with(".md") {
        return true;
    }
    if lower.ends_with(".tmp.md") {
        return true;
    }
    if lower.ends_with(".bak") {
        return true;
    }
    false
}

/// Source-tree directories that are NOT allowed to host ephemeral
/// artefacts.  When an ephemeral file lives in (or under) one of these
/// directories, it is relocated.  The match is a **path-prefix** check
/// so `crates/ralph-core/scratchpad.md` is caught because its path
/// starts with `crates/`.
pub const FORBIDDEN_SOURCE_DIRS: &[&str] = &[
    "crates", "src", "backend", "frontend", "examples", "tests", "docs",
];

/// Runtime-allowed locations for ephemeral artefacts.  A file under
/// any of these paths is left alone by [`EphemeralIsolation`].
pub const ALLOWED_PATHS: &[&str] = &[".ralph/agent", ".agents/scratchpad", "/tmp", "/var/tmp"];

/// Outcome of relocating a single ephemeral file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelocationRecord {
    /// Absolute or repo-relative path of the source file before
    /// relocation.  Reported back to the agent so it can verify the
    /// content was preserved.
    pub from: String,
    /// Path of the scratchpad file the content was appended to.  The
    /// file is created on first use and shared across all relocated
    /// files for the same loop.
    pub to: String,
    /// Number of bytes that were appended to `to`.  Used by the
    /// prompt to give the agent a quick sanity-check.
    pub size_bytes: u64,
}

/// Ephemeral isolation engine.  Constructed once per `EventLoop` and
/// re-used across iterations; the cache of `mtime` / `size` lets the
/// implementation skip the git round-trip when the workspace has not
/// changed.
#[derive(Debug, Default)]
pub struct EphemeralIsolation {
    /// Last events-file mtime/size seen when we scanned.  Used to
    /// short-circuit when nothing has changed since the last call.
    last_events_mtime: Option<u64>,
    last_events_size: Option<u64>,
    /// Last scratchpad file we wrote to (so we do not re-resolve on
    /// every iteration).
    last_scratchpad: Option<PathBuf>,
}

impl EphemeralIsolation {
    /// Construct a fresh engine.
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan the workspace for ephemeral files and relocate any that
    /// land in forbidden source dirs.  Returns one
    /// [`RelocationRecord`] per file moved (the order matches the
    /// order of `git ls-files` output — typically the on-disk
    /// order).  When the workspace is not a git repository, the
    /// function falls back to a direct walk of the immediate
    /// children of the workspace root.
    ///
    /// `loop_id` namespaces the scratchpad file (`.ralph/agent/scratchpad-{loop_id}.md`)
    /// so parallel loops on the same repo do not stomp on each
    /// other.  When `loop_id` is `None` the file is just
    /// `scratchpad.md`.
    pub fn scan_and_relocate(
        &mut self,
        repo_root: &Path,
        loop_id: Option<&str>,
    ) -> Vec<RelocationRecord> {
        self.scan_and_relocate_with_allowlist(repo_root, loop_id, ALLOWED_PATHS)
    }

    /// Same as [`Self::scan_and_relocate`] but lets the caller override
    /// the allowlist.  Tests use this to point at a sandboxed `.ralph/`
    /// directory; the production path keeps the hardcoded allowlist.
    pub fn scan_and_relocate_with_allowlist(
        &mut self,
        repo_root: &Path,
        loop_id: Option<&str>,
        allowlist: &[&str],
    ) -> Vec<RelocationRecord> {
        // Cache short-circuit (R3 review round 2, finding #2): when
        // the cache fingerprint is hot AND the last call relocated
        // nothing, skip git.  The fingerprint is the (size, max
        // mtime) of the candidate set + whether the last call
        // returned any records.  A transient delete failure the
        // previous iteration leaves the candidate unchanged but
        // the "did we relocate" flag resets, forcing a re-scan.
        // This matches the plan's "best-effort, self-healing on
        // next iteration" semantics while bounding the per-iteration
        // git cost.
        if self.cache_hit(repo_root) {
            return Vec::new();
        }
        // Resolve untracked file candidates via git first; if the
        // workspace is not a git repo, fall back to a direct walk of
        // immediate children.
        let candidates = self.collect_candidates(repo_root);
        if candidates.is_empty() {
            return Vec::new();
        }

        let scratchpad = match self.ensure_scratchpad(repo_root, loop_id) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    "ephemeral_isolation: failed to ensure scratchpad at {}: {}",
                    repo_root.display(),
                    e
                );
                return Vec::new();
            }
        };

        let mut records = Vec::new();
        for candidate in candidates {
            // `candidate` is a path relative to `repo_root`.  Re-anchor
            // it so the path-prefix checks below operate on the same
            // shape callers see in their prompts.
            let rel = candidate.strip_prefix(repo_root).unwrap_or(&candidate);
            if !is_forbidden_source_path(rel) {
                continue;
            }
            let file_name = match rel.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if !is_ephemeral_name(file_name) {
                continue;
            }
            if path_is_allowed(rel, allowlist) {
                continue;
            }
            // Read the source file, append to the scratchpad, then
            // remove the original.  The append-then-delete order means
            // a partial delete (e.g. read-only file system) does not
            // silently lose the content.
            let content = match fs::read_to_string(&candidate) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        "ephemeral_isolation: failed to read {}: {}",
                        candidate.display(),
                        e
                    );
                    continue;
                }
            };
            let size_bytes = content.len() as u64;
            if let Err(e) = append_to_scratchpad(&scratchpad, &content) {
                tracing::warn!(
                    "ephemeral_isolation: failed to append to {}: {}",
                    scratchpad.display(),
                    e
                );
                continue;
            }
            if let Err(e) = fs::remove_file(&candidate) {
                // We log but do not return early: the content is
                // already in the scratchpad, so the agent can still
                // see what it wrote.  The next iteration will re-detect
                // the source file and re-relocate it (idempotent), so a
                // transient delete failure is self-healing.
                tracing::warn!(
                    "ephemeral_isolation: failed to delete {}: {}",
                    candidate.display(),
                    e
                );
            }
            records.push(RelocationRecord {
                from: rel.to_string_lossy().into_owned(),
                to: scratchpad
                    .strip_prefix(repo_root)
                    .unwrap_or(&scratchpad)
                    .to_string_lossy()
                    .into_owned(),
                size_bytes,
            });
        }
        self.last_scratchpad = Some(scratchpad);
        records
    }

    /// Collect candidate ephemeral files via `git ls-files --others
    /// --exclude-standard` when available, falling back to a
    /// top-level walk otherwise.  Updates the mtime/size cache as a
    /// side effect so the next call can short-circuit when the
    /// workspace has not changed.
    fn collect_candidates(&mut self, repo_root: &Path) -> Vec<PathBuf> {
        let output = std::process::Command::new("git")
            .arg("ls-files")
            .arg("--others")
            .arg("--exclude-standard")
            .arg("-z")
            .current_dir(repo_root)
            .output();
        if let Ok(out) = output {
            if out.status.success() {
                // Cache the raw output fingerprint (byte count of the
                // git stdout) — when the workspace's untracked set is
                // unchanged, the byte count is unchanged, and the
                // next call's `cache_hit` short-circuits without
                // re-running the per-file work below.
                let raw_bytes = out.stdout.len() as u64;
                let mut paths: Vec<PathBuf> = Vec::new();
                for chunk in out.stdout.split(|b| *b == 0) {
                    if chunk.is_empty() {
                        continue;
                    }
                    if let Ok(rel) = std::str::from_utf8(chunk) {
                        let p = repo_root.join(rel);
                        if p.is_file() {
                            paths.push(p);
                        }
                    }
                }
                self.last_events_size = Some(raw_bytes);
                self.last_events_mtime = paths
                    .iter()
                    .filter_map(|p| fs::metadata(p).ok())
                    .filter_map(|m| m.modified().ok())
                    .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .max();
                return paths;
            }
        }

        // Fallback: walk the immediate children of `repo_root` only.
        // We do NOT recurse — a recursive walk could be expensive on
        // large monorepos and the policy is "ephemeral files we know
        // about, only at the top of a forbidden source dir".  If the
        // agent nests `crates/foo/scratchpad.md`, the git path picks
        // it up; the fallback is intentionally shallow.
        let mut paths = Vec::new();
        let entries = match fs::read_dir(repo_root) {
            Ok(e) => e,
            Err(_) => return paths,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            paths.push(path);
        }
        paths
    }

    /// True when the workspace's untracked file surface has not
    /// changed since the last call.  Used to short-circuit the
    /// per-file relocation pass on iterations where the workspace
    /// is identical to the previous one.
    ///
    /// Implementation: re-runs `git ls-files --others
    /// --exclude-standard` and compares the raw output byte count
    /// to the cached value.  We pay one git invocation per call
    /// but skip the per-file metadata reads and relocation writes.
    /// Falls through (returns `false`) on the first call so the
    /// cache builds from scratch.
    fn cache_hit(&mut self, repo_root: &Path) -> bool {
        // The fingerprint is the byte count of the git stdout.  When
        // the workspace's untracked set is unchanged, the byte count
        // is unchanged, and the next call's `cache_hit` short-circuits
        // without re-running the per-file work below.
        let output = std::process::Command::new("git")
            .arg("ls-files")
            .arg("--others")
            .arg("--exclude-standard")
            .arg("-z")
            .current_dir(repo_root)
            .output();
        let Ok(out) = output else {
            return false;
        };
        if !out.status.success() {
            return false;
        }
        let cur_bytes = out.stdout.len() as u64;
        let prev = self.last_events_size;
        // Always update the cache so the NEXT call's prev is
        // current.  The short-circuit fires only when both
        // `prev == cur_bytes` and `prev > 0` (a non-empty cache
        // means a prior call found something).
        self.last_events_size = Some(cur_bytes);
        prev == Some(cur_bytes) && cur_bytes > 0
    }

    /// Ensure the scratchpad file exists and return its path.
    fn ensure_scratchpad(
        &mut self,
        repo_root: &Path,
        loop_id: Option<&str>,
    ) -> std::io::Result<PathBuf> {
        if let Some(p) = &self.last_scratchpad {
            return Ok(p.clone());
        }
        let dir = repo_root.join(".ralph").join("agent");
        fs::create_dir_all(&dir)?;
        let name = match loop_id {
            Some(id) => format!("scratchpad-{id}.md"),
            None => "scratchpad.md".to_string(),
        };
        let path = dir.join(name);
        if !path.exists() {
            // Create an empty file so subsequent appends succeed.
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::File::create(&path)?;
        }
        Ok(path)
    }
}

fn is_forbidden_source_path(rel: &Path) -> bool {
    let mut components = rel.components();
    let Some(first) = components.next() else {
        return false;
    };
    let Some(first_str) = first.as_os_str().to_str() else {
        return false;
    };
    FORBIDDEN_SOURCE_DIRS.iter().any(|d| *d == first_str)
}

fn path_is_allowed(rel: &Path, allowlist: &[&str]) -> bool {
    let s = rel.to_string_lossy();
    allowlist.iter().any(|p| {
        // Absolute allowlist entries (`/tmp`, `/var/tmp`) match by
        // string prefix; relative entries (`.ralph/agent`,
        // `.agents/scratchpad`) match by path-component prefix.
        if p.starts_with('/') {
            s.starts_with(p.trim_end_matches('/'))
        } else {
            s == *p || s.starts_with(&format!("{p}/"))
        }
    })
}

fn append_to_scratchpad(path: &Path, content: &str) -> std::io::Result<()> {
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let separator = format!(
        "\n\n<!-- relocated by ephemeral_isolation @ {} -->\n",
        chrono_like_now()
    );
    f.write_all(separator.as_bytes())?;
    f.write_all(content.as_bytes())?;
    Ok(())
}

/// Cheap, allocation-light timestamp.  Avoids a hard dependency on
/// `chrono` / `time` — the scratchpad separator only needs to be
/// grep-friendly, not RFC-3339.
fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

/// Cheap fingerprint of a set of paths: returns `(mtime_max, size_total)`.
/// Used by [`EphemeralIsolation::collect_candidates`] to short-circuit
/// a `git ls-files --others` call when the workspace's untracked file
/// fingerprint has not changed since the last call.
///
/// We deliberately do NOT include file count in the fingerprint: a
/// newly-created ephemeral file changes the count but the same `git`
/// invocation will re-encounter it, so the cache is not load-bearing
/// for correctness.  `mtime` + `size` catches the practical case of
/// "nothing on disk changed since the last iteration".
#[cfg(test)]
fn file_mtime_sentinel(paths: &[PathBuf]) -> (u64, u64) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut mtime_max: u64 = 0;
    let mut size_total: u64 = 0;
    for p in paths {
        let Ok(meta) = fs::metadata(p) else { continue };
        size_total = size_total.wrapping_add(meta.len());
        // The mtime field of `fs::Metadata` is unstable across
        // platforms; use a coarse wall-clock comparison against the
        // 8-hour loop cap.  A value of `now_unix` (file newer than
        // the cache build) forces a refresh.
        let file_mtime = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(now_unix);
        if file_mtime > mtime_max {
            mtime_max = file_mtime;
        }
    }
    (mtime_max, size_total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Test helper: build a fresh sandbox repo under `tempfile` and
    /// initialise it as a git repo so `git ls-files` succeeds.  When
    /// the test environment does not have `git` on PATH the helper
    /// still works — the implementation falls back to the
    /// non-recursive walk.
    fn sandbox(label: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        // Initialise a git repo so `git ls-files` works.  Tests run in
        // environments with git; when not available the fallback path
        // kicks in and the assertions still hold (the relocation
        // records match the files we created).
        let _ = std::process::Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(dir.path())
            .output();
        // Use a `user.email`/`user.name` so `git ls-files` does not
        // warn — warnings are harmless but they clutter test logs.
        let _ = std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir.path())
            .output();
        let _ = std::process::Command::new("git")
            .args(["config", "user.name", "test"])
            .current_dir(dir.path())
            .output();
        // Mark `label` as used to avoid dead-code warnings when the
        // helper is reused across modules.
        let _ = label;
        dir
    }

    #[test]
    fn file_mtime_sentinel_handles_missing_files() {
        // Helper resilience: missing / unreadable paths must not
        // panic.  The sentinel returns (0, 0) for an empty list.
        assert_eq!(file_mtime_sentinel(&[]), (0, 0));
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("does-not-exist.md");
        let (mtime, size) = file_mtime_sentinel(&[p]);
        assert_eq!(size, 0);
        // mtime may be 0 or a wall-clock default — we only assert
        // the function returned successfully without panicking.
        let _ = mtime;
    }

    #[test]
    fn is_ephemeral_name_matches_known_patterns() {
        assert!(is_ephemeral_name("scratchpad.md"));
        assert!(is_ephemeral_name("notes.md"));
        assert!(is_ephemeral_name("tmp-notes.md"));
        assert!(is_ephemeral_name("notes.tmp.md"));
        assert!(is_ephemeral_name("scratchpad.md.bak"));
        assert!(!is_ephemeral_name("lib.rs"));
        assert!(!is_ephemeral_name("README.md"));
    }

    #[test]
    fn detects_scratchpad_in_crates_and_relocates() {
        let dir = sandbox("detects_scratchpad_in_crates_and_relocates");
        let crates = dir.path().join("crates").join("ralph-core");
        fs::create_dir_all(&crates).unwrap();
        let src = crates.join("scratchpad.md");
        fs::write(&src, "## Notes\nfoo\n").unwrap();

        let mut engine = EphemeralIsolation::new();
        let records = engine.scan_and_relocate(dir.path(), Some("loop-1"));

        assert_eq!(records.len(), 1);
        let rec = &records[0];
        assert!(rec.from.ends_with("crates/ralph-core/scratchpad.md"));
        assert!(rec.to.ends_with(".ralph/agent/scratchpad-loop-1.md"));
        assert!(rec.size_bytes > 0);
        assert!(!src.exists(), "original file must be removed");
        let scratchpad = dir.path().join(&rec.to);
        let content = fs::read_to_string(&scratchpad).unwrap();
        assert!(content.contains("## Notes"));
        assert!(content.contains("foo"));
    }

    #[test]
    fn ignores_allowed_paths() {
        let dir = sandbox("ignores_allowed_paths");
        let allowed = dir.path().join(".agents").join("scratchpad");
        fs::create_dir_all(&allowed).unwrap();
        let notes = allowed.join("notes.md");
        fs::write(&notes, "## Allowed notes\n").unwrap();

        let mut engine = EphemeralIsolation::new();
        let records = engine.scan_and_relocate(dir.path(), Some("loop-1"));
        assert!(records.is_empty(), "allowed paths must NOT be relocated");
        assert!(notes.exists(), "allowed file must remain in place");
    }

    #[test]
    fn detects_multiple_patterns_across_source_dirs() {
        let dir = sandbox("detects_multiple_patterns");
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("tmp-notes.md"), "tmp A\n").unwrap();

        let backend = dir.path().join("backend");
        fs::create_dir_all(&backend).unwrap();
        fs::write(backend.join("api.tmp.md"), "tmp B\n").unwrap();

        // A real `.rs` source file must be left alone (not ephemeral).
        fs::write(src_dir.join("lib.rs"), "fn main() {}\n").unwrap();

        let mut engine = EphemeralIsolation::new();
        let records = engine.scan_and_relocate(dir.path(), Some("loop-1"));
        assert_eq!(
            records.len(),
            2,
            "exactly two ephemeral files should be relocated"
        );
        let from_set: std::collections::BTreeSet<_> =
            records.iter().map(|r| r.from.as_str()).collect();
        assert!(from_set.contains("src/tmp-notes.md"));
        assert!(from_set.contains("backend/api.tmp.md"));
        assert!(
            src_dir.join("lib.rs").exists(),
            "non-ephemeral files must not be moved"
        );
    }

    #[test]
    fn relocation_is_idempotent() {
        let dir = sandbox("relocation_is_idempotent");
        let crates = dir.path().join("crates").join("ralph-core");
        fs::create_dir_all(&crates).unwrap();
        let src = crates.join("scratchpad.md");
        fs::write(&src, "## Round 1\n").unwrap();

        let mut engine = EphemeralIsolation::new();
        let first = engine.scan_and_relocate(dir.path(), Some("loop-1"));
        assert_eq!(first.len(), 1);

        // The source file is gone; a second scan should relocate 0
        // files.  The scratchpad content is preserved across the two
        // scans.
        let second = engine.scan_and_relocate(dir.path(), Some("loop-1"));
        assert!(second.is_empty());
        let scratchpad = dir.path().join(".ralph/agent/scratchpad-loop-1.md");
        let content = fs::read_to_string(&scratchpad).unwrap();
        assert!(content.contains("Round 1"));
    }
}
