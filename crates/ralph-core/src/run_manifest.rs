//! Per-run evidence manifest (`.ralph/runs/<run_id>.manifest.json`).
//!
//! feat(monitor) foundation plan (2026-10-28-1949) Unit 1: every
//! `ralph run` / `ralph resume` / `ralph collection run` incarnation
//! writes a small JSON manifest at start and updates it once with
//! terminal evidence when the loop runner exits. The manifest is the
//! stable observation contract consumed by `ralph inspect run
//! --format json` and by the monitor service's collector — readers
//! never parse runtime internals (loop.lock, loops.json, merge queue,
//! state files) to discover run identity.
//!
//! Write protocol:
//!
//! - [`write_start`]: upsert on `run_id`. The first incarnation sets
//!   `started_at`; a later incarnation (e.g. `ralph run --continue`
//!   re-using a loop id) preserves the original `started_at`, refreshes
//!   `pid`, and clears `terminal` so the run is observable as alive
//!   again.
//! - [`record_terminal`]: first-write-wins. A manifest that already
//!   has `terminal` set is left untouched so a late second writer
//!   cannot overwrite the authoritative terminal evidence.
//! - Both writes bump `revision` (monotonic counter starting at 1)
//!   so optimistic-concurrency readers can detect a stale snapshot.
//! - Every write goes through a tmp file in the same directory plus
//!   `rename`, so readers never observe a partially written manifest.
//!
//! The manifest lives in the loop's own workspace `.ralph/runs/` —
//! for a worktree loop that means the worktree directory, which is
//! removed when the worktree is cleaned up. The monitor keeps its
//! own SQLite projection, so projection state survives that deletion.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::text::truncate_with_ellipsis;

/// Schema tag for the manifest file itself (distinct from the
/// `run-observation/v1` envelope emitted by `ralph inspect run`).
pub const MANIFEST_SCHEMA_VERSION: &str = "run-manifest/v1";

/// Directory (relative to a workspace root) holding run manifests.
pub const RUNS_DIR: &str = ".ralph/runs";

/// File-name suffix appended to `<run_id>` inside the runs directory.
pub const MANIFEST_SUFFIX: &str = ".manifest.json";

/// Character budget for `prompt_preview`.
const PROMPT_PREVIEW_LEN: usize = 160;

/// Whether the run is the primary loop (main workspace, holds
/// `loop.lock`) or a worktree loop (isolated checkout).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    /// Primary loop running in the main workspace.
    Primary,
    /// Worktree loop running in an isolated git worktree.
    Worktree,
}

impl RunKind {
    /// Stable string form for JSON consumers and diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Worktree => "worktree",
        }
    }
}

/// Terminal evidence recorded exactly once when the loop runner exits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInfo {
    /// Terminal reason in the `TerminationReason::as_str()` vocabulary
    /// (`"completed"`, `"max_iterations"`, `"error"`, `"sigterm"`, ...).
    pub reason: String,
    /// Wall-clock time the terminal record was written.
    pub at: DateTime<Utc>,
    /// Merge commit produced in-loop (merge-loop runs with inline
    /// merge). `None` for queued/discarded/manual-merge outcomes —
    /// monitors should correlate `merge_state` evidence instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_commit: Option<String>,
}

/// The run manifest document (v1).
///
/// Missing fields from older manifests deserialize as `None` /
/// defaults, so readers must tolerate sparse documents; new optional
/// fields stay backward compatible for the same reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunManifest {
    /// Always [`MANIFEST_SCHEMA_VERSION`] for files written by this
    /// crate; unknown values should be surfaced by readers, not parsed
    /// silently.
    pub schema_version: String,
    /// Stable run identity (`primary-<ts>` or `loop-<ts>-<hex>`).
    pub run_id: String,
    /// Primary vs worktree classification.
    pub kind: RunKind,
    /// PID of the `ralph` process that owns this run.
    pub pid: u32,
    /// Process working directory at start.
    pub cwd: PathBuf,
    /// Workspace root this run operates on (worktree dir for worktree
    /// loops; same as `cwd` in practice).
    pub workspace: PathBuf,
    /// Main repository root (differs from `workspace` for worktrees).
    pub repo_root: PathBuf,
    /// Worktree checkout path when `kind == "worktree"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<PathBuf>,
    /// First ~160 chars of the resolved prompt.
    pub prompt_preview: String,
    /// `sha256` hex digest of the full resolved prompt.
    pub prompt_sha256: String,
    /// Preset / hats source label (`builtin:<name>` or a file path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Plan / prompt-file path that seeded the run, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_path: Option<PathBuf>,
    /// Client-supplied idempotency id for monitor-launched runs
    /// (`ralph run --client-request-id`); `None` for operator runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
    /// When this incarnation started (preserved across `--continue`).
    pub started_at: DateTime<Utc>,
    /// Events JSONL file the run writes to (absolute path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events_file: Option<PathBuf>,
    /// Terminal evidence; `None` while the run has not converged.
    #[serde(default)]
    pub terminal: Option<TerminalInfo>,
    /// Monotonic write counter; bumped on every manifest write.
    #[serde(default)]
    pub revision: u64,
}

/// Inputs for [`write_start`]. Grouped so the call site stays legible.
pub struct WriteStartArgs<'a> {
    /// Loop workspace root (worktree dir for worktree loops).
    pub workspace_root: &'a Path,
    /// Main repository root.
    pub repo_root: &'a Path,
    /// Worktree checkout path; `None` for the primary loop.
    pub worktree_path: Option<&'a Path>,
    /// Resolved run identity.
    pub run_id: &'a str,
    /// Owning `ralph` process id.
    pub pid: u32,
    /// Fully resolved prompt text (preview + hash are derived here).
    pub prompt: &'a str,
    /// Preset / hats source label.
    pub preset: Option<String>,
    /// Plan / prompt file that seeded the run.
    pub plan_path: Option<PathBuf>,
    /// Monitor-supplied idempotency id.
    pub client_request_id: Option<String>,
    /// Events JSONL file the loop writes to.
    pub events_file: Option<PathBuf>,
}

/// Absolute path of the manifest for `run_id` under `workspace_root`.
///
/// Returns `Err(InvalidInput)` when `run_id` contains a path
/// separator or `..` so a hostile id cannot escape the runs dir.
pub fn manifest_path(workspace_root: &Path, run_id: &str) -> io::Result<PathBuf> {
    if run_id.is_empty() || run_id.contains('/') || run_id.contains('\\') || run_id.contains("..") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid run_id for manifest path: {run_id:?}"),
        ));
    }
    Ok(workspace_root
        .join(RUNS_DIR)
        .join(format!("{run_id}{MANIFEST_SUFFIX}")))
}

/// The `.ralph/runs/` directory for a workspace.
pub fn runs_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(RUNS_DIR)
}

/// Read a manifest from disk. IO and JSON errors surface to the caller.
pub fn read(path: &Path) -> Result<RunManifest, io::Error> {
    let raw = fs::read_to_string(path)?;
    serde_json::from_str(&raw)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e}")))
}

/// A manifest discovered on disk. `manifest` is `None` when the file
/// exists but failed to parse (corrupt), which readers must surface
/// as evidence rather than skip silently.
#[derive(Debug)]
pub struct DiscoveredManifest {
    /// Path the file was loaded from.
    pub path: PathBuf,
    /// Run id taken from the file name (`<run_id>.manifest.json`) so
    /// corrupt files still contribute identity.
    pub run_id: String,
    /// Parsed document; `None` when `corrupt`.
    pub manifest: Option<RunManifest>,
    /// `true` when the file failed JSON/schema parse.
    pub corrupt: bool,
}

/// List every manifest under `runs_dir` (typically `.ralph/runs/`).
///
/// Missing directory yields an empty list — not an error, since a
/// workspace with no runs legitimately has no runs dir. Files that
/// fail to parse are returned with `corrupt = true`.
pub fn discover_all(runs_dir: &Path) -> Vec<DiscoveredManifest> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(runs_dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(stem) = name.strip_suffix(MANIFEST_SUFFIX) else {
            continue;
        };
        let parsed = read(&path);
        match parsed {
            Ok(manifest) => out.push(DiscoveredManifest {
                path,
                run_id: stem.to_string(),
                manifest: Some(manifest),
                corrupt: false,
            }),
            Err(_) => out.push(DiscoveredManifest {
                path,
                run_id: stem.to_string(),
                manifest: None,
                corrupt: true,
            }),
        }
    }
    out.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    out
}

/// Write (or refresh) the start evidence for a run.
///
/// When a manifest for `run_id` already exists (resume re-using the
/// loop id), the original `started_at` is preserved, `terminal` is
/// cleared, and `revision` is incremented. Otherwise a fresh manifest
/// at `revision = 1` is created.
///
/// Returns the manifest path on success.
pub fn write_start(args: &WriteStartArgs<'_>) -> io::Result<PathBuf> {
    let dir = runs_dir(args.workspace_root);
    fs::create_dir_all(&dir)?;
    let path = manifest_path(args.workspace_root, args.run_id)?;

    let mut sha = Sha256::new();
    sha.update(args.prompt.as_bytes());
    let prompt_sha256 = hex::encode(sha.finalize());

    let (started_at, revision) = match read(&path) {
        Ok(existing) => (existing.started_at, existing.revision.saturating_add(1)),
        Err(_) => (Utc::now(), 1),
    };

    let manifest = RunManifest {
        schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
        run_id: args.run_id.to_string(),
        kind: if args.worktree_path.is_some() {
            RunKind::Worktree
        } else {
            RunKind::Primary
        },
        pid: args.pid,
        cwd: args.workspace_root.to_path_buf(),
        workspace: args.workspace_root.to_path_buf(),
        repo_root: args.repo_root.to_path_buf(),
        worktree_path: args.worktree_path.map(Path::to_path_buf),
        prompt_preview: truncate_with_ellipsis(args.prompt.trim(), PROMPT_PREVIEW_LEN),
        prompt_sha256,
        preset: args.preset.clone(),
        plan_path: args.plan_path.clone(),
        client_request_id: args.client_request_id.clone(),
        started_at,
        events_file: args.events_file.clone(),
        terminal: None,
        revision,
    };

    write_atomic(&path, &manifest)?;
    Ok(path)
}

/// Record terminal evidence for a run. First-write-wins: when the
/// manifest already carries `terminal`, the call is a no-op and
/// returns `Ok(false)`. A missing or corrupt manifest also returns
/// `Ok(false)` — terminal evidence is never synthesized for a run
/// this process did not observe starting.
///
/// Returns `Ok(true)` when the terminal record was written.
pub fn record_terminal(
    workspace_root: &Path,
    run_id: &str,
    reason: &str,
    merge_commit: Option<String>,
) -> io::Result<bool> {
    let path = manifest_path(workspace_root, run_id)?;
    if !path.exists() {
        return Ok(false);
    }
    let mut manifest = match read(&path) {
        Ok(m) => m,
        Err(_) => return Ok(false),
    };
    if manifest.terminal.is_some() {
        return Ok(false);
    }
    manifest.terminal = Some(TerminalInfo {
        reason: reason.to_string(),
        at: Utc::now(),
        merge_commit,
    });
    manifest.revision = manifest.revision.saturating_add(1);
    write_atomic(&path, &manifest)?;
    Ok(true)
}

/// `sha256` hex digest helper for callers that need the prompt hash
/// without writing a manifest.
pub fn prompt_sha256_hex(prompt: &str) -> String {
    let mut sha = Sha256::new();
    sha.update(prompt.as_bytes());
    hex::encode(sha.finalize())
}

/// Whether a process `pid` exists. Mirrors `LoopEntry::is_pid_alive`
/// semantics (POSIX `kill(pid, 0)`); non-Unix platforms assume alive.
#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    kill(Pid::from_raw(pid as i32), None).is_ok()
}

/// Non-Unix fallback: assume alive (platforms without signal 0 cannot
/// distinguish and must not claim death).
#[cfg(not(unix))]
pub fn pid_alive(_pid: u32) -> bool {
    true
}

/// Write `manifest` to `path` via a same-dir temp file + rename so
/// concurrent readers only ever see complete documents.
fn write_atomic(path: &Path, manifest: &RunManifest) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("manifest path has no parent: {}", path.display()),
        )
    })?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e}")))?;
    use std::io::Write;
    tmp.write_all(json.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn args<'a>(ws: &'a Path, run_id: &'a str, prompt: &'a str) -> WriteStartArgs<'a> {
        WriteStartArgs {
            workspace_root: ws,
            repo_root: ws,
            worktree_path: None,
            run_id,
            pid: std::process::id(),
            prompt,
            preset: None,
            plan_path: None,
            client_request_id: None,
            events_file: None,
        }
    }

    #[test]
    fn write_start_creates_manifest_and_record_terminal_is_first_write_wins() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();

        let path = write_start(&args(ws, "primary-20261028-120000", "hello prompt")).unwrap();
        assert!(path.exists());
        let m = read(&path).unwrap();
        assert_eq!(m.schema_version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(m.run_id, "primary-20261028-120000");
        assert_eq!(m.kind, RunKind::Primary);
        assert_eq!(m.prompt_preview, "hello prompt");
        assert_eq!(m.prompt_sha256, prompt_sha256_hex("hello prompt"));
        assert_eq!(m.revision, 1);
        assert!(m.terminal.is_none());

        // First terminal write lands.
        let wrote = record_terminal(ws, "primary-20261028-120000", "completed", None).unwrap();
        assert!(wrote);
        let m = read(&path).unwrap();
        let t = m.terminal.as_ref().unwrap();
        assert_eq!(t.reason, "completed");
        assert!(t.merge_commit.is_none());
        assert_eq!(m.revision, 2);

        // Second terminal write is ignored (first-write-wins).
        let wrote =
            record_terminal(ws, "primary-20261028-120000", "sigterm", Some("abc".into())).unwrap();
        assert!(!wrote);
        let m = read(&path).unwrap();
        assert_eq!(m.terminal.as_ref().unwrap().reason, "completed");
        assert_eq!(m.revision, 2);
    }

    #[test]
    fn write_start_resume_preserves_started_at_and_clears_terminal() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();

        write_start(&args(ws, "primary-1", "p")).unwrap();
        record_terminal(ws, "primary-1", "completed", None).unwrap();
        let before = read(&manifest_path(ws, "primary-1").unwrap()).unwrap();

        write_start(&args(ws, "primary-1", "p2")).unwrap();
        let after = read(&manifest_path(ws, "primary-1").unwrap()).unwrap();
        assert_eq!(after.started_at, before.started_at);
        assert!(after.terminal.is_none());
        assert_eq!(after.revision, 3);
    }

    #[test]
    fn discover_all_skips_corrupt_and_reports_flag() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let dir = runs_dir(ws);
        fs::create_dir_all(&dir).unwrap();

        write_start(&args(ws, "primary-1", "p")).unwrap();
        fs::write(dir.join("broken.manifest.json"), "{not json").unwrap();
        fs::write(dir.join("not-a-manifest.json"), "{}").unwrap();

        let found = discover_all(&dir);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].run_id, "broken");
        assert!(found[0].corrupt);
        assert_eq!(found[1].run_id, "primary-1");
        assert!(!found[1].corrupt);
    }

    #[test]
    fn discover_all_missing_dir_is_empty() {
        let tmp = TempDir::new().unwrap();
        assert!(discover_all(&runs_dir(tmp.path())).is_empty());
    }

    #[test]
    fn manifest_path_rejects_path_traversal() {
        let tmp = TempDir::new().unwrap();
        assert!(manifest_path(tmp.path(), "../evil").is_err());
        assert!(manifest_path(tmp.path(), "a/b").is_err());
        assert!(manifest_path(tmp.path(), "").is_err());
    }

    #[test]
    fn record_terminal_missing_manifest_is_noop() {
        let tmp = TempDir::new().unwrap();
        assert!(!record_terminal(tmp.path(), "nope", "completed", None).unwrap());
    }

    #[test]
    fn worktree_args_write_worktree_kind() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let wt = tmp.path().join("wt");
        let mut a = args(&wt, "loop-1", "p");
        a.repo_root = ws;
        a.worktree_path = Some(&wt);
        // simulate write into the worktree workspace
        fs::create_dir_all(&wt).unwrap();
        write_start(&a).unwrap();
        let m = read(&manifest_path(&wt, "loop-1").unwrap()).unwrap();
        assert_eq!(m.kind, RunKind::Worktree);
        assert_eq!(m.worktree_path.as_deref(), Some(wt.as_path()));
        assert_eq!(m.repo_root, ws);
    }
}
