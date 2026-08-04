//! Git worktree management for parallel Ralph loops.
//!
//! Provides filesystem isolation for concurrent loops using git worktrees.
//! Each parallel loop gets its own working directory with full filesystem
//! isolation, sharing only `.git` history. Conflicts are resolved at merge time.
//!
//! # Example
//!
//! ```no_run
//! use ralph_core::worktree::{Worktree, WorktreeConfig, create_worktree, remove_worktree, list_worktrees};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = WorktreeConfig::default();
//!
//!     // Create worktree for a parallel loop
//!     let worktree = create_worktree(".", "ralph-20250124-a3f2", &config)?;
//!     println!("Created worktree at: {}", worktree.path.display());
//!
//!     // List all worktrees
//!     let worktrees = list_worktrees(".")?;
//!     for wt in worktrees {
//!         println!("  {}: {}", wt.branch, wt.path.display());
//!     }
//!
//!     // Clean up when done
//!     remove_worktree(".", &worktree.path)?;
//!     Ok(())
//! }
//! ```

use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::LoopEntry;

/// Configuration for worktree operations.
#[derive(Debug, Clone)]
pub struct WorktreeConfig {
    /// Directory where worktrees are created (default: `.worktrees`).
    pub worktree_dir: PathBuf,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            worktree_dir: PathBuf::from(".worktrees"),
        }
    }
}

impl WorktreeConfig {
    /// Create config with custom worktree directory.
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            worktree_dir: dir.into(),
        }
    }

    /// Get the absolute path to worktree directory relative to repo root.
    pub fn worktree_path(&self, repo_root: &Path) -> PathBuf {
        if self.worktree_dir.is_absolute() {
            self.worktree_dir.clone()
        } else {
            repo_root.join(&self.worktree_dir)
        }
    }
}

/// Information about a git worktree.
#[derive(Debug, Clone)]
pub struct Worktree {
    /// Absolute path to the worktree directory.
    pub path: PathBuf,

    /// The branch checked out in this worktree.
    pub branch: String,

    /// Whether this is the main worktree.
    pub is_main: bool,

    /// HEAD commit (if available).
    pub head: Option<String>,
}

/// Statistics about files synced to a worktree.
#[derive(Debug, Default, Clone)]
pub struct SyncStats {
    /// Number of untracked files copied.
    pub untracked_copied: usize,
    /// Number of modified (unstaged) files copied.
    pub modified_copied: usize,
    /// Number of files skipped (e.g., no longer exists).
    pub skipped: usize,
    /// Number of files that failed to copy.
    pub errors: usize,
}

/// Errors that can occur during worktree operations.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// Git command failed.
    #[error("Git command failed: {0}")]
    Git(String),

    /// Worktree already exists.
    #[error("Worktree already exists: {0}")]
    AlreadyExists(String),

    /// Worktree not found.
    #[error("Worktree not found: {0}")]
    NotFound(String),

    /// Not a git repository.
    #[error("Not a git repository: {0}")]
    NotARepo(String),

    /// Branch already exists.
    #[error("Branch already exists: {0}")]
    BranchExists(String),
}

/// Create a new worktree for a parallel Ralph loop.
///
/// Creates a new branch and worktree at `{config.worktree_dir}/{loop_id}`.
/// The branch is created from HEAD of the current branch.
///
/// # Arguments
///
/// * `repo_root` - Root of the git repository
/// * `loop_id` - Unique identifier for the loop (e.g., "ralph-20250124-a3f2")
/// * `config` - Worktree configuration
///
/// # Returns
///
/// Information about the created worktree.
pub fn create_worktree(
    repo_root: impl AsRef<Path>,
    loop_id: &str,
    config: &WorktreeConfig,
) -> Result<Worktree, WorktreeError> {
    let repo_root = repo_root.as_ref();

    // Verify this is a git repository
    if !repo_root.join(".git").exists() && !repo_root.join(".git").is_file() {
        return Err(WorktreeError::NotARepo(
            repo_root.to_string_lossy().to_string(),
        ));
    }

    let worktree_base = config.worktree_path(repo_root);
    let worktree_path = worktree_base.join(loop_id);
    let branch_name = format!("ralph/{loop_id}");

    // Check if worktree already exists
    if worktree_path.exists() {
        return Err(WorktreeError::AlreadyExists(
            worktree_path.to_string_lossy().to_string(),
        ));
    }

    // Ensure worktree directory exists
    fs::create_dir_all(&worktree_base)?;

    // Create worktree with new branch
    // git worktree add -b <branch> <path>
    let output = Command::new("git")
        .args(["worktree", "add", "-b", &branch_name])
        .arg(&worktree_path)
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Check for specific error cases
        if stderr.contains("already exists") {
            if stderr.contains("branch") {
                return Err(WorktreeError::BranchExists(branch_name));
            }
            return Err(WorktreeError::AlreadyExists(
                worktree_path.to_string_lossy().to_string(),
            ));
        }

        return Err(WorktreeError::Git(stderr.to_string()));
    }

    // Sync untracked files and unstaged changes
    let sync_stats = sync_working_directory_to_worktree(repo_root, &worktree_path, config)?;

    if sync_stats.errors > 0 {
        tracing::warn!(
            "Some files failed to sync to worktree: {} errors",
            sync_stats.errors
        );
    }

    // Get the HEAD commit
    let head = get_head_commit(&worktree_path).ok();

    tracing::debug!(
        "Created worktree at {} on branch {} (synced {} untracked, {} modified files)",
        worktree_path.display(),
        branch_name,
        sync_stats.untracked_copied,
        sync_stats.modified_copied
    );

    Ok(Worktree {
        path: worktree_path,
        branch: branch_name,
        is_main: false,
        head,
    })
}

/// Remove a worktree and optionally its branch.
///
/// # Arguments
///
/// * `repo_root` - Root of the git repository
/// * `worktree_path` - Path to the worktree to remove
///
/// # Note
///
/// This also deletes the associated branch if it exists.
pub fn remove_worktree(
    repo_root: impl AsRef<Path>,
    worktree_path: impl AsRef<Path>,
) -> Result<(), WorktreeError> {
    let repo_root = repo_root.as_ref();
    let worktree_path = worktree_path.as_ref();

    if !worktree_path.exists() {
        return Err(WorktreeError::NotFound(
            worktree_path.to_string_lossy().to_string(),
        ));
    }

    // Get the branch name before removing
    let branch = get_worktree_branch(worktree_path);

    // Remove the worktree (--force handles uncommitted changes)
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(worktree_path)
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::Git(stderr.to_string()));
    }

    // Delete the branch if it was a ralph/* branch
    if let Some(branch) = branch
        && branch.starts_with("ralph/")
    {
        let output = Command::new("git")
            .args(["branch", "-D", &branch])
            .current_dir(repo_root)
            .output()?;

        if !output.status.success() {
            // Non-fatal: branch might already be deleted
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::debug!("Failed to delete branch {}: {}", branch, stderr);
        }
    }

    // Prune worktree refs
    let _ = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(repo_root)
        .output();

    tracing::debug!("Removed worktree at {}", worktree_path.display());

    Ok(())
}

/// List all git worktrees in the repository.
///
/// # Arguments
///
/// * `repo_root` - Root of the git repository (can be any worktree)
///
/// # Returns
///
/// List of all worktrees, including the main worktree.
pub fn list_worktrees(repo_root: impl AsRef<Path>) -> Result<Vec<Worktree>, WorktreeError> {
    let repo_root = repo_root.as_ref();

    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::Git(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_worktree_list(&stdout)
}

/// Parse the porcelain output of `git worktree list`.
fn parse_worktree_list(output: &str) -> Result<Vec<Worktree>, WorktreeError> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_head: Option<String> = None;
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;

    for line in output.lines() {
        if line.starts_with("worktree ") {
            // Save previous worktree if any
            if let Some(path) = current_path.take()
                && !is_bare
            {
                worktrees.push(Worktree {
                    path,
                    branch: current_branch
                        .take()
                        .unwrap_or_else(|| "(detached)".to_string()),
                    is_main: worktrees.is_empty(), // First one is main
                    head: current_head.take(),
                });
            }

            current_path = Some(PathBuf::from(line.strip_prefix("worktree ").unwrap()));
            current_head = None;
            current_branch = None;
            is_bare = false;
        } else if line.starts_with("HEAD ") {
            current_head = Some(line.strip_prefix("HEAD ").unwrap().to_string());
        } else if line.starts_with("branch ") {
            // Branch is in format "refs/heads/branch-name"
            let branch_ref = line.strip_prefix("branch ").unwrap();
            current_branch = Some(
                branch_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch_ref)
                    .to_string(),
            );
        } else if line == "bare" {
            is_bare = true;
        }
    }

    // Don't forget the last one
    if let Some(path) = current_path
        && !is_bare
    {
        worktrees.push(Worktree {
            path,
            branch: current_branch.unwrap_or_else(|| "(detached)".to_string()),
            is_main: worktrees.is_empty(),
            head: current_head,
        });
    }

    Ok(worktrees)
}

/// Ensure the worktree directory is in `.gitignore`.
///
/// Appends the pattern to `.gitignore` if not already present.
///
/// # Arguments
///
/// * `repo_root` - Root of the git repository
/// * `worktree_dir` - The worktree directory pattern to ignore (e.g., ".worktrees")
pub fn ensure_gitignore(
    repo_root: impl AsRef<Path>,
    worktree_dir: &str,
) -> Result<(), WorktreeError> {
    let repo_root = repo_root.as_ref();
    let gitignore_path = repo_root.join(".gitignore");

    // Pattern to add (with trailing slash for directory)
    let pattern = if worktree_dir.ends_with('/') {
        worktree_dir.to_string()
    } else {
        format!("{}/", worktree_dir)
    };

    // Check if pattern already exists
    if gitignore_path.exists() {
        let file = File::open(&gitignore_path)?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();

            // Check if this line matches our pattern (with or without trailing slash)
            if trimmed == pattern || trimmed == pattern.trim_end_matches('/') {
                tracing::debug!("Pattern {} already in .gitignore", pattern);
                return Ok(());
            }
        }
    }

    // Append the pattern
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore_path)?;

    // Add newline before if file exists and doesn't end with newline
    if gitignore_path.exists() {
        let contents = fs::read_to_string(&gitignore_path)?;
        if !contents.is_empty() && !contents.ends_with('\n') {
            writeln!(file)?;
        }
    }

    writeln!(file, "{}", pattern)?;

    tracing::debug!("Added {} to .gitignore", pattern);

    Ok(())
}

/// Get the branch name for a worktree.
fn get_worktree_branch(worktree_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch != "HEAD" {
            return Some(branch);
        }
    }
    None
}

/// Get the HEAD commit SHA for a worktree.
fn get_head_commit(worktree_path: &Path) -> Result<String, WorktreeError> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(worktree_path)
        .output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(WorktreeError::Git(stderr.to_string()))
    }
}

/// Get the list of Ralph-specific worktrees (those with `ralph/` branches).
pub fn list_ralph_worktrees(repo_root: impl AsRef<Path>) -> Result<Vec<Worktree>, WorktreeError> {
    let all = list_worktrees(repo_root)?;
    Ok(all
        .into_iter()
        .filter(|wt| wt.branch.starts_with("ralph/"))
        .collect())
}

/// Information about a reusable worktree found via prefix matching.
///
/// Returned by [`find_reusable_worktree_by_name`]. Holds enough information for
/// the caller to construct a `LoopContext::worktree(...)` and register a
/// new loop entry without re-creating the git worktree.
#[derive(Debug, Clone)]
pub struct ReusableWorktree {
    /// Absolute path to the existing worktree directory.
    pub path: PathBuf,
    /// The branch checked out in this worktree (e.g., `ralph/<loop-id>`).
    pub branch: String,
    /// The loop ID from the previous registry entry (also matches the
    /// worktree directory name and the suffix of `branch`).
    pub loop_id: String,
    /// The original `started` timestamp of the registry entry we matched.
    /// Used to break ties when multiple entries match the same prefix:
    /// the entry with the most recent `started` wins.
    pub started: DateTime<Utc>,
    /// HEAD commit recorded at lookup time (may be `None` if the worktree
    /// is in detached state).
    pub head: Option<String>,
}

impl ReusableWorktree {
    /// Convert into a [`Worktree`] view (drops `loop_id` and `started`).
    pub fn as_worktree(&self) -> Worktree {
        Worktree {
            path: self.path.clone(),
            branch: self.branch.clone(),
            is_main: false,
            head: self.head.clone(),
        }
    }
}

pub fn find_reusable_worktree_by_name(
    repo_root: impl AsRef<Path>,
    name: &str,
) -> Result<Option<ReusableWorktree>, WorktreeError> {
    if name.is_empty() {
        return Ok(None);
    }

    let repo_root = repo_root.as_ref();
    let worktree_path = repo_root.join(".worktrees").join(name);

    if !worktree_path.is_dir() {
        return Ok(None);
    }

    // Cross-validate against git's view of worktrees.
    let known_paths: HashSet<PathBuf> = match list_worktrees(repo_root) {
        Ok(list) => list
            .into_iter()
            .map(|wt| canonicalize_for_compare(&wt.path))
            .collect(),
        Err(_) => HashSet::new(),
    };
    let candidate_canonical = canonicalize_for_compare(&worktree_path);
    if !known_paths.is_empty() && !known_paths.contains(&candidate_canonical) {
        tracing::debug!(
            "Skipping reusable worktree {}: not in git worktree list",
            worktree_path.display()
        );
        return Ok(None);
    }

    let registry_path = repo_root.join(".ralph").join("loops.json");
    if registry_path.exists() {
        let entries: Vec<LoopEntry> = read_loop_registry_entries(&registry_path)?;
        for entry in entries {
            let wt_path = match &entry.worktree_path {
                Some(p) => PathBuf::from(p),
                None => continue,
            };

            let loop_id = match wt_path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };

            if loop_id != name && entry.id != name {
                continue;
            }

            if entry.is_alive() {
                return Err(WorktreeError::Git(format!(
                    "Worktree {} is still in use by a running loop (PID {}).",
                    name, entry.pid
                )));
            }

            return Ok(Some(ReusableWorktree {
                path: worktree_path.clone(),
                branch: format!("ralph/{name}"),
                loop_id: name.to_string(),
                started: entry.started,
                head: get_head_commit(&worktree_path).ok(),
            }));
        }
    }

    // Directory exists and is git-known, but there is no registry entry
    // (e.g. a manually created worktree). We still allow reuse.
    let head = get_head_commit(&worktree_path).ok();
    Ok(Some(ReusableWorktree {
        path: worktree_path,
        branch: format!("ralph/{name}"),
        loop_id: name.to_string(),
        started: chrono::Utc::now(),
        head,
    }))
}

/// Canonicalize a path for cross-validation against `git worktree list`
/// output.
///
/// `git worktree list --porcelain` reports paths after resolving all
/// symlinks (e.g. on macOS `/var/folders/...` becomes
/// `/private/var/folders/...`). Registry entries, by contrast, are
/// stored verbatim at registration time. We canonicalize both sides
/// so the lookup is robust against host-specific symlink layouts.
///
/// `fs::canonicalize` requires the path to exist, so we fall back to
/// the input path when the canonicalize call fails (path was deleted
/// between the directory check and this call). In that case the
/// subsequent `is_dir()` filter in the caller would have already
/// skipped the entry, so a non-canonical fallback is harmless.
fn canonicalize_for_compare(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn read_loop_registry_entries(registry_path: &Path) -> Result<Vec<LoopEntry>, WorktreeError> {
    // Read the registry JSON directly. We deliberately bypass
    // `LoopRegistry::list()` because that method takes the registry
    // write-lock and prunes entries whose PID is no longer alive,
    // which would erase the very entries we are trying to find
    // (completed worktree loops). `find_reusable_worktree_by_name` is a
    // read-only lookup; performing its own filtering means the
    // registry can keep its auto-cleanup invariant intact.
    let contents = fs::read_to_string(registry_path).map_err(|e| {
        WorktreeError::Io(io::Error::new(
            e.kind(),
            format!("failed to read {}: {}", registry_path.display(), e),
        ))
    })?;

    if contents.trim().is_empty() {
        return Ok(Vec::new());
    }

    #[derive(serde::Deserialize)]
    struct Wrapper {
        loops: Vec<LoopEntry>,
    }
    let wrapper: Wrapper = serde_json::from_str(&contents).map_err(|e| {
        WorktreeError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to parse loop registry JSON: {e}"),
        ))
    })?;
    Ok(wrapper.loops)
}

fn unique_reuse_archive_dir(ralph_dir: &Path) -> std::io::Result<PathBuf> {
    let archive_root = ralph_dir.join("reuse-history");
    fs::create_dir_all(&archive_root)?;
    let timestamp = Utc::now().format("%Y%m%dT%H%M%S%.9fZ").to_string();
    for suffix in 0..1000_u16 {
        let name = if suffix == 0 {
            timestamp.clone()
        } else {
            format!("{timestamp}-{suffix}")
        };
        let candidate = archive_root.join(name);
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique reuse-history archive directory",
    ))
}

fn archive_if_exists(source: &Path, archive_root: &Path, relative: &Path) -> std::io::Result<bool> {
    match fs::symlink_metadata(source) {
        Ok(_) => {
            let destination = archive_root.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(source, destination)?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn archive_files_matching(
    dir: &Path,
    suffix: &str,
    prefix: &str,
    archive_root: &Path,
    relative_parent: &Path,
) -> std::io::Result<bool> {
    if !dir.is_dir() {
        return Ok(false);
    }
    let mut archived = false;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with(prefix) || !name_str.ends_with(suffix) {
            continue;
        }
        archived |= archive_if_exists(
            &entry.path(),
            archive_root,
            &relative_parent.join(Path::new(&name)),
        )?;
    }
    Ok(archived)
}

/// Clean Ralph runtime artifacts from an existing worktree directory
/// in preparation for reuse.
///
/// `find_reusable_worktree_by_name` matches a worktree that was previously used
/// by a finished loop. The directory still contains that loop's
/// event history, scratchpad, tasks, and diagnostics. We move those
/// records into `.ralph/reuse-history/<timestamp>/` before clearing
/// the live runtime paths, so the new loop starts clean without losing
/// prior-run experience. We *preserve*
/// the worktree's git branch state and the symlinks that point at the
/// main repository (memories, specs, code tasks) — these are the
/// reason the user opted into reuse in the first place.
///
/// # Files removed
///
/// - `.ralph/events.jsonl`, `.ralph/events-*.jsonl`
/// - `.ralph/current-events`
/// - `.ralph/history.jsonl`, `.ralph/history-*.jsonl`
/// - `.ralph/diagnostics/` (entire tree)
/// - `.ralph/urgent-steer.json`
/// - `.ralph/current-loop-id`
/// - `.ralph/agent/scratchpad.md`, `.ralph/agent/scratchpad-*.md`
/// - `.ralph/agent/tasks.jsonl`
/// - `.ralph/agent/summary.md`
/// - `.ralph/agent/handoff.md`
/// - `.ralph/agent/decisions.md`
/// - `.ralph/review/` (entire tree)
///
/// # Files preserved
///
/// - `.ralph/agent/context.md` (worktree metadata)
/// - `.ralph/agent/memories.md` (symlink into main repo)
/// - `.ralph/agent/accepted-transitions.jsonl` (durable acceptance
///   outbox — the ONLY authority for which business events were
///   accepted. Deliberately never archived: it must survive reuse so a
///   crash-window capture can still fall back on its boundary records,
///   and so `commit_idempotent` dedup keeps working across reuses.)
/// - `.ralph/specs/`, `.ralph/tasks/` (symlinks into main repo)
/// - `.ralph/reuse-history/` (prior-run archives)
/// - The `.ralph/` and `.ralph/agent/` directories themselves
/// - The git worktree (branch, history, tracked files)
///
/// # Error propagation
///
/// Any I/O error during cleanup is propagated via `WorktreeError::Io`
/// so the caller can exit before launching the loop. A partial
/// cleanup is worse than a refused start: starting on a stale
/// state would silently corrupt the new run.
///
/// # Resume manifest capture (U1, plan 2026-08-03-004)
///
/// When `resume_inputs` is `Some`, a `parallel-forge-resume-manifest.v1`
/// is captured from the OLD live runtime files BEFORE any file is
/// archived or removed, and written into the archive directory as
/// [`crate::parallel_forge_resume::MANIFEST_FILE_NAME`]. Capture happens
/// first because archiving moves the evidence (events, outbox, task
/// ledger) out of the live paths. The manifest can mark itself
/// incomplete; the start-time validation gate in the CLI is responsible
/// for refusing the loop. When `resume_inputs` is `None`, no manifest
/// is captured (legacy behavior).
///
/// An INCOMPLETE manifest is archived as-is when this cleanup's gate
/// refuses the start (fail-closed first refusal). Later reuses must not
/// be pinned to it: the fallback read
/// ([`crate::parallel_forge_resume::latest_archived_manifest`]) skips
/// incomplete manifests and continues with older archives, so a
/// crash-window capture can never lock the worktree out of reuse
/// permanently (U2-fix, adjudication ①).
///
/// # Cleanup-before-gate ordering (U3-fix, adversarial A1)
///
/// Archiving the live evidence BEFORE the CLI validation gate runs is
/// safe once capture recognizes a terminal tail (last in-log boundary
/// event with no `triggered` hat) as a CLEAN COMPLETION: a normally
/// finished run now captures a COMPLETE manifest, so the gate passes
/// on the archived product instead of refusing it forever. For
/// crash-window shapes the durable outbox is never archived by this
/// cleanup, so a later reuse's re-capture falls back on it (U2-fix)
/// instead of failing closed forever. No re-ordering is needed.
pub fn clean_worktree_runtime_artifacts(
    worktree_path: impl AsRef<Path>,
    resume_inputs: Option<&crate::parallel_forge_resume::CaptureInputs>,
) -> Result<Option<PathBuf>, WorktreeError> {
    let worktree_path = worktree_path.as_ref();
    if !worktree_path.is_dir() {
        return Err(WorktreeError::NotFound(
            worktree_path.to_string_lossy().to_string(),
        ));
    }

    let ralph_dir = worktree_path.join(".ralph");
    let agent_dir = ralph_dir.join("agent");
    fs::create_dir_all(&ralph_dir)?;
    fs::create_dir_all(&agent_dir)?;

    // U1: capture resume evidence BEFORE any live file moves. Reading
    // after the archive would race the very renames we are about to do.
    let manifest = resume_inputs
        .map(|inputs| crate::parallel_forge_resume::capture_manifest(worktree_path, inputs));

    let archive_dir = unique_reuse_archive_dir(&ralph_dir)?;
    let mut archived_any = false;

    // --- .ralph/ top-level artifacts ---
    // events.jsonl
    archived_any |= archive_if_exists(
        &ralph_dir.join("events.jsonl"),
        &archive_dir,
        Path::new("events.jsonl"),
    )?;
    // events-YYYYMMDD-HHMMSS.jsonl
    archived_any |=
        archive_files_matching(&ralph_dir, ".jsonl", "events-", &archive_dir, Path::new(""))?;
    // current-events marker
    archived_any |= archive_if_exists(
        &ralph_dir.join("current-events"),
        &archive_dir,
        Path::new("current-events"),
    )?;
    // history.jsonl
    archived_any |= archive_if_exists(
        &ralph_dir.join("history.jsonl"),
        &archive_dir,
        Path::new("history.jsonl"),
    )?;
    // history-*.jsonl
    archived_any |= archive_files_matching(
        &ralph_dir,
        ".jsonl",
        "history-",
        &archive_dir,
        Path::new(""),
    )?;
    // diagnostics/ (full subtree)
    archived_any |= archive_if_exists(
        &ralph_dir.join("diagnostics"),
        &archive_dir,
        Path::new("diagnostics"),
    )?;
    // urgent-steer.json
    archived_any |= archive_if_exists(
        &ralph_dir.join("urgent-steer.json"),
        &archive_dir,
        Path::new("urgent-steer.json"),
    )?;
    // current-loop-id
    archived_any |= archive_if_exists(
        &ralph_dir.join("current-loop-id"),
        &archive_dir,
        Path::new("current-loop-id"),
    )?;
    archived_any |=
        archive_if_exists(&ralph_dir.join("review"), &archive_dir, Path::new("review"))?;

    // --- .ralph/agent/ artifacts ---
    // scratchpad.md
    archived_any |= archive_if_exists(
        &agent_dir.join("scratchpad.md"),
        &archive_dir,
        Path::new("agent/scratchpad.md"),
    )?;
    // scratchpad-{loop_id}.md (ephemeral isolation artifacts)
    archived_any |= archive_files_matching(
        &agent_dir,
        ".md",
        "scratchpad-",
        &archive_dir,
        Path::new("agent"),
    )?;
    // tasks.jsonl
    archived_any |= archive_if_exists(
        &agent_dir.join("tasks.jsonl"),
        &archive_dir,
        Path::new("agent/tasks.jsonl"),
    )?;
    // summary.md
    archived_any |= archive_if_exists(
        &agent_dir.join("summary.md"),
        &archive_dir,
        Path::new("agent/summary.md"),
    )?;
    // handoff.md
    archived_any |= archive_if_exists(
        &agent_dir.join("handoff.md"),
        &archive_dir,
        Path::new("agent/handoff.md"),
    )?;
    archived_any |= archive_if_exists(
        &agent_dir.join("decisions.md"),
        &archive_dir,
        Path::new("agent/decisions.md"),
    )?;

    // --- Re-create the parent directories so a fresh loop has a
    // clean slate to write into. We deliberately do not create
    // `.ralph/specs/` or `.ralph/tasks/` here — those are symlinks
    // set up by `LoopContext::setup_worktree_symlinks` and pointing
    // them at a non-existent target would be worse than leaving them
    // absent (the next `setup_*_symlink` call will create them
    // idempotently).
    fs::create_dir_all(&ralph_dir)?;
    fs::create_dir_all(&agent_dir)?;

    if !archived_any {
        fs::remove_dir(&archive_dir)?;
        return Ok(None);
    }

    // U1: persist the captured resume manifest into the archive. The
    // manifest was captured from the live paths BEFORE the renames
    // above, so it reflects exactly the run we just archived. An
    // incomplete manifest is written as-is; the CLI validation gate
    // refuses THIS start fail-closed. Later reuses are not locked out
    // by it: the fallback read skips incomplete manifests and continues
    // with older archives (U2-fix, adjudication ①).
    if let Some(manifest) = manifest {
        crate::parallel_forge_resume::write_manifest(&manifest, &archive_dir)?;
    }

    let archive_relative = archive_dir
        .strip_prefix(worktree_path)
        .unwrap_or(&archive_dir)
        .to_string_lossy();
    fs::write(
        agent_dir.join("resume-context.md"),
        format!(
            "# Reused worktree context\n\n\
             Previous runtime archive: `{archive_relative}`\n\n\
             Treat archived records as advisory evidence, not as the current run's verdict. \
             Revalidate against the current plan, Git history, working-tree diff, and tests. \
             Prior failures must inform a new approach but must not consume this run's retry budget.\n"
        ),
    )?;

    tracing::info!(
        "Archived prior runtime artifacts to {} and cleaned live paths in worktree {}",
        archive_dir.display(),
        worktree_path.display(),
    );
    Ok(Some(archive_dir))
}

/// Check if a worktree exists for the given loop ID.
pub fn worktree_exists(
    repo_root: impl AsRef<Path>,
    loop_id: &str,
    config: &WorktreeConfig,
) -> bool {
    let worktree_path = config.worktree_path(repo_root.as_ref()).join(loop_id);
    worktree_path.exists()
}

/// Get list of untracked files in the repository.
///
/// Uses `git ls-files --others --exclude-standard` to get files that are:
/// - Not tracked by git
/// - Not ignored by .gitignore
fn get_untracked_files(repo_root: &Path) -> Result<Vec<PathBuf>, WorktreeError> {
    let output = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::Git(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// Get list of tracked files with unstaged modifications.
///
/// Uses `git diff --name-only` to get files that have been modified
/// but not yet staged for commit.
fn get_unstaged_modified_files(repo_root: &Path) -> Result<Vec<PathBuf>, WorktreeError> {
    let output = Command::new("git")
        .args(["diff", "--name-only"])
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::Git(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// Copy a file from repo to worktree, preserving directory structure.
///
/// Creates parent directories as needed. Handles symlinks on Unix.
/// Returns Ok(false) if the source file no longer exists (race condition).
fn copy_file_with_structure(
    repo_root: &Path,
    worktree_path: &Path,
    relative_path: &Path,
) -> Result<bool, WorktreeError> {
    let source = repo_root.join(relative_path);
    let dest = worktree_path.join(relative_path);

    // Skip if source no longer exists (race condition)
    if !source.exists() && !source.is_symlink() {
        return Ok(false);
    }

    // Create parent directories
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    // Handle symlinks on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs as unix_fs;
        if source.is_symlink() {
            let link_target = fs::read_link(&source)?;
            // Remove existing file/symlink if present
            if dest.exists() || dest.is_symlink() {
                fs::remove_file(&dest)?;
            }
            unix_fs::symlink(&link_target, &dest)?;
            return Ok(true);
        }
    }

    // Copy regular file (handles binary files correctly)
    fs::copy(&source, &dest)?;
    Ok(true)
}

/// Sync untracked and unstaged files from the main repo to a worktree.
///
/// This copies files that are not committed to git, ensuring that WIP files
/// and uncommitted changes are available in the worktree for parallel loops.
///
/// # Exclusions
///
/// - `.git/` directory (never copied)
/// - The worktree directory itself (e.g., `.worktrees/`)
///
/// # Arguments
///
/// * `repo_root` - Root of the git repository
/// * `worktree_path` - Path to the target worktree
/// * `config` - Worktree configuration (for determining exclusion paths)
///
/// # Returns
///
/// Statistics about what was synced.
pub fn sync_working_directory_to_worktree(
    repo_root: &Path,
    worktree_path: &Path,
    config: &WorktreeConfig,
) -> Result<SyncStats, WorktreeError> {
    let mut stats = SyncStats::default();

    // Get the worktree directory name for exclusion
    let worktree_dir = &config.worktree_dir;

    // Helper to check if a path should be excluded
    let should_exclude = |path: &Path| -> bool {
        let path_str = path.to_string_lossy();
        // Exclude .git directory
        if path_str.starts_with(".git/") || path_str == ".git" {
            return true;
        }
        // Exclude the worktree directory itself
        let worktree_dir_str = worktree_dir.to_string_lossy();
        if path_str.starts_with(&*worktree_dir_str)
            || path_str.starts_with(&format!("{}/", worktree_dir_str))
        {
            return true;
        }
        false
    };

    // Get untracked files
    let untracked = get_untracked_files(repo_root)?;
    for file in untracked {
        if should_exclude(&file) {
            stats.skipped += 1;
            continue;
        }
        match copy_file_with_structure(repo_root, worktree_path, &file) {
            Ok(true) => {
                tracing::trace!("Copied untracked file: {}", file.display());
                stats.untracked_copied += 1;
            }
            Ok(false) => {
                stats.skipped += 1;
            }
            Err(e) => {
                tracing::warn!("Failed to copy untracked file {}: {}", file.display(), e);
                stats.errors += 1;
            }
        }
    }

    // Get unstaged modified files
    let modified = get_unstaged_modified_files(repo_root)?;
    for file in modified {
        if should_exclude(&file) {
            stats.skipped += 1;
            continue;
        }
        match copy_file_with_structure(repo_root, worktree_path, &file) {
            Ok(true) => {
                tracing::trace!("Copied modified file: {}", file.display());
                stats.modified_copied += 1;
            }
            Ok(false) => {
                stats.skipped += 1;
            }
            Err(e) => {
                tracing::warn!("Failed to copy modified file {}: {}", file.display(), e);
                stats.errors += 1;
            }
        }
    }

    tracing::debug!(
        "Synced {} untracked and {} modified files to worktree ({} skipped, {} errors)",
        stats.untracked_copied,
        stats.modified_copied,
        stats.skipped,
        stats.errors
    );

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_git_repo(dir: &Path) {
        Command::new("git")
            .args(["init", "--initial-branch=main"])
            .current_dir(dir)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@test.local"])
            .current_dir(dir)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(dir)
            .output()
            .unwrap();

        // Create initial commit (required for worktrees)
        fs::write(dir.join("README.md"), "# Test").unwrap();
        Command::new("git")
            .args(["add", "README.md"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Initial commit"])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn test_worktree_config_default() {
        let config = WorktreeConfig::default();
        assert_eq!(config.worktree_dir, PathBuf::from(".worktrees"));
    }

    #[test]
    fn test_worktree_config_path() {
        let config = WorktreeConfig::default();
        let repo = Path::new("/repo");
        assert_eq!(
            config.worktree_path(repo),
            PathBuf::from("/repo/.worktrees")
        );

        let absolute_config = WorktreeConfig::with_dir("/tmp/worktrees");
        assert_eq!(
            absolute_config.worktree_path(repo),
            PathBuf::from("/tmp/worktrees")
        );
    }

    #[test]
    fn test_create_and_remove_worktree() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let loop_id = "test-loop-123";

        // Create worktree
        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        assert!(worktree.path.exists());
        assert_eq!(worktree.branch, "ralph/test-loop-123");
        assert!(!worktree.is_main);
        assert!(worktree.head.is_some());

        // Verify README was copied
        assert!(worktree.path.join("README.md").exists());

        // Remove worktree
        remove_worktree(temp_dir.path(), &worktree.path).unwrap();
        assert!(!worktree.path.exists());
    }

    #[test]
    fn test_create_worktree_already_exists() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let loop_id = "duplicate";

        // Create first worktree
        let _wt = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Try to create duplicate
        let result = create_worktree(temp_dir.path(), loop_id, &config);
        assert!(matches!(result, Err(WorktreeError::AlreadyExists(_))));
    }

    #[test]
    fn test_list_worktrees() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Initially just the main worktree
        let worktrees = list_worktrees(temp_dir.path()).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].is_main);

        // Add a worktree
        let config = WorktreeConfig::default();
        let _wt = create_worktree(temp_dir.path(), "loop-1", &config).unwrap();

        let worktrees = list_worktrees(temp_dir.path()).unwrap();
        assert_eq!(worktrees.len(), 2);
    }

    #[test]
    fn test_list_ralph_worktrees() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let _wt1 = create_worktree(temp_dir.path(), "loop-1", &config).unwrap();
        let _wt2 = create_worktree(temp_dir.path(), "loop-2", &config).unwrap();

        let ralph_worktrees = list_ralph_worktrees(temp_dir.path()).unwrap();
        assert_eq!(ralph_worktrees.len(), 2);
        assert!(
            ralph_worktrees
                .iter()
                .all(|wt| wt.branch.starts_with("ralph/"))
        );
    }

    #[test]
    fn test_ensure_gitignore_new_file() {
        let temp_dir = TempDir::new().unwrap();
        let gitignore = temp_dir.path().join(".gitignore");

        assert!(!gitignore.exists());

        ensure_gitignore(temp_dir.path(), ".worktrees").unwrap();

        assert!(gitignore.exists());
        let contents = fs::read_to_string(&gitignore).unwrap();
        assert!(contents.contains(".worktrees/"));
    }

    #[test]
    fn test_ensure_gitignore_existing_file() {
        let temp_dir = TempDir::new().unwrap();
        let gitignore = temp_dir.path().join(".gitignore");

        fs::write(&gitignore, "node_modules/\n").unwrap();

        ensure_gitignore(temp_dir.path(), ".worktrees").unwrap();

        let contents = fs::read_to_string(&gitignore).unwrap();
        assert!(contents.contains("node_modules/"));
        assert!(contents.contains(".worktrees/"));
    }

    #[test]
    fn test_ensure_gitignore_already_present() {
        let temp_dir = TempDir::new().unwrap();
        let gitignore = temp_dir.path().join(".gitignore");

        fs::write(&gitignore, ".worktrees/\n").unwrap();

        ensure_gitignore(temp_dir.path(), ".worktrees").unwrap();

        let contents = fs::read_to_string(&gitignore).unwrap();
        // Should only appear once
        assert_eq!(contents.matches(".worktrees/").count(), 1);
    }

    #[test]
    fn test_ensure_gitignore_without_trailing_slash() {
        let temp_dir = TempDir::new().unwrap();
        let gitignore = temp_dir.path().join(".gitignore");

        // Existing pattern without trailing slash
        fs::write(&gitignore, ".worktrees\n").unwrap();

        ensure_gitignore(temp_dir.path(), ".worktrees").unwrap();

        let contents = fs::read_to_string(&gitignore).unwrap();
        // Should not add duplicate
        assert!(!contents.contains(".worktrees/\n.worktrees/"));
    }

    #[test]
    fn test_worktree_exists() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let loop_id = "check-exists";

        assert!(!worktree_exists(temp_dir.path(), loop_id, &config));

        let _wt = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        assert!(worktree_exists(temp_dir.path(), loop_id, &config));
    }

    #[test]
    fn test_not_a_repo() {
        let temp_dir = TempDir::new().unwrap();
        // Don't init git

        let config = WorktreeConfig::default();
        let result = create_worktree(temp_dir.path(), "loop-1", &config);

        assert!(matches!(result, Err(WorktreeError::NotARepo(_))));
    }

    #[test]
    fn test_remove_nonexistent_worktree() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let result = remove_worktree(temp_dir.path(), temp_dir.path().join("nonexistent"));

        assert!(matches!(result, Err(WorktreeError::NotFound(_))));
    }

    #[test]
    fn test_parse_worktree_list() {
        let output = r"worktree /path/to/main
HEAD abc123def
branch refs/heads/main

worktree /path/to/.worktrees/loop-1
HEAD def456ghi
branch refs/heads/ralph/loop-1

";

        let worktrees = parse_worktree_list(output).unwrap();
        assert_eq!(worktrees.len(), 2);

        assert_eq!(worktrees[0].path, PathBuf::from("/path/to/main"));
        assert_eq!(worktrees[0].branch, "main");
        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[0].head, Some("abc123def".to_string()));

        assert_eq!(
            worktrees[1].path,
            PathBuf::from("/path/to/.worktrees/loop-1")
        );
        assert_eq!(worktrees[1].branch, "ralph/loop-1");
        assert!(!worktrees[1].is_main);
    }

    #[test]
    fn test_get_untracked_files() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create untracked files
        fs::write(temp_dir.path().join("untracked1.txt"), "content1").unwrap();
        fs::write(temp_dir.path().join("untracked2.txt"), "content2").unwrap();

        let untracked = get_untracked_files(temp_dir.path()).unwrap();
        assert_eq!(untracked.len(), 2);
        assert!(untracked.contains(&PathBuf::from("untracked1.txt")));
        assert!(untracked.contains(&PathBuf::from("untracked2.txt")));
    }

    #[test]
    fn test_get_unstaged_modified_files() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Modify a tracked file without staging
        fs::write(temp_dir.path().join("README.md"), "# Modified").unwrap();

        let modified = get_unstaged_modified_files(temp_dir.path()).unwrap();
        assert_eq!(modified.len(), 1);
        assert!(modified.contains(&PathBuf::from("README.md")));
    }

    #[test]
    fn test_sync_untracked_files_to_worktree() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create an untracked file
        fs::write(temp_dir.path().join("new_file.txt"), "untracked content").unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-untracked";

        // Create worktree - should sync untracked file
        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Verify untracked file was copied
        let synced_file = worktree.path.join("new_file.txt");
        assert!(synced_file.exists());
        assert_eq!(
            fs::read_to_string(&synced_file).unwrap(),
            "untracked content"
        );
    }

    #[test]
    fn test_sync_unstaged_changes_to_worktree() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Modify a tracked file without staging
        fs::write(temp_dir.path().join("README.md"), "# Modified Content").unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-modified";

        // Create worktree - should sync modified file
        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Verify modified content was copied (overwrote the committed version)
        let synced_file = worktree.path.join("README.md");
        assert!(synced_file.exists());
        assert_eq!(
            fs::read_to_string(&synced_file).unwrap(),
            "# Modified Content"
        );
    }

    #[test]
    fn test_sync_respects_gitignore() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Add a pattern to .gitignore
        fs::write(temp_dir.path().join(".gitignore"), "*.log\n").unwrap();
        Command::new("git")
            .args(["add", ".gitignore"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add gitignore"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();

        // Create an ignored file
        fs::write(temp_dir.path().join("debug.log"), "log content").unwrap();
        // Create a non-ignored file
        fs::write(temp_dir.path().join("valid.txt"), "valid content").unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-gitignore";

        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Ignored file should NOT be copied (git ls-files --others --exclude-standard respects .gitignore)
        assert!(!worktree.path.join("debug.log").exists());
        // Non-ignored file should be copied
        assert!(worktree.path.join("valid.txt").exists());
    }

    #[test]
    fn test_sync_excludes_worktrees_directory() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create an untracked file in the worktrees directory manually
        let worktrees_dir = temp_dir.path().join(".worktrees");
        fs::create_dir_all(&worktrees_dir).unwrap();
        fs::write(worktrees_dir.join("should_not_sync.txt"), "content").unwrap();

        // Create a normal untracked file
        fs::write(temp_dir.path().join("should_sync.txt"), "content").unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-exclude-worktrees";

        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Normal file should be synced
        assert!(worktree.path.join("should_sync.txt").exists());
        // The .worktrees directory should NOT be synced into itself
        // (this would cause recursion issues)
        assert!(
            !worktree
                .path
                .join(".worktrees/should_not_sync.txt")
                .exists()
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_sync_preserves_symlinks() {
        use std::os::unix::fs as unix_fs;

        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create a target file
        fs::write(temp_dir.path().join("target.txt"), "target content").unwrap();
        Command::new("git")
            .args(["add", "target.txt"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Add target"])
            .current_dir(temp_dir.path())
            .output()
            .unwrap();

        // Create an untracked symlink
        unix_fs::symlink("target.txt", temp_dir.path().join("link.txt")).unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-symlinks";

        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Verify symlink was preserved
        let synced_link = worktree.path.join("link.txt");
        assert!(synced_link.is_symlink());
        assert_eq!(
            fs::read_link(&synced_link).unwrap(),
            PathBuf::from("target.txt")
        );
    }

    #[test]
    fn test_sync_handles_binary_files() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create a binary file (PNG header bytes)
        let binary_content: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
        ];
        fs::write(temp_dir.path().join("image.png"), &binary_content).unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-binary";

        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Verify binary file was copied correctly
        let synced_file = worktree.path.join("image.png");
        assert!(synced_file.exists());
        assert_eq!(fs::read(&synced_file).unwrap(), binary_content);
    }

    #[test]
    fn test_sync_handles_nested_directories() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create nested untracked files
        let nested_dir = temp_dir.path().join("src/components/nested");
        fs::create_dir_all(&nested_dir).unwrap();
        fs::write(nested_dir.join("deep.txt"), "deep content").unwrap();

        let config = WorktreeConfig::default();
        let loop_id = "sync-nested";

        let worktree = create_worktree(temp_dir.path(), loop_id, &config).unwrap();

        // Verify nested file was copied with correct directory structure
        let synced_file = worktree.path.join("src/components/nested/deep.txt");
        assert!(synced_file.exists());
        assert_eq!(fs::read_to_string(&synced_file).unwrap(), "deep content");
    }

    #[test]
    fn test_sync_stats_returned() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create untracked files
        fs::write(temp_dir.path().join("untracked1.txt"), "content").unwrap();
        fs::write(temp_dir.path().join("untracked2.txt"), "content").unwrap();

        // Modify a tracked file
        fs::write(temp_dir.path().join("README.md"), "# Modified").unwrap();

        let config = WorktreeConfig::default();

        // Test sync_working_directory_to_worktree directly
        let worktree_path = temp_dir.path().join(".worktrees/stats-test");
        fs::create_dir_all(&worktree_path).unwrap();

        let stats =
            sync_working_directory_to_worktree(temp_dir.path(), &worktree_path, &config).unwrap();

        assert_eq!(stats.untracked_copied, 2);
        assert_eq!(stats.modified_copied, 1);
        assert_eq!(stats.errors, 0);
    }

    /// Write a completed LoopEntry into `.ralph/loops.json` directly,
    /// bypassing `LoopRegistry::register()`'s write-lock + auto-cleanup
    /// that would erase the entry we are trying to stage.
    const DEAD_PID_SENTINEL: u32 = 4_194_305;
    fn register_completed_entry(
        repo_root: &Path,
        loop_id: &str,
        worktree_path: &Path,
        started: DateTime<Utc>,
    ) {
        use crate::loop_registry::LoopEntry;

        let mut entry = LoopEntry::with_id(
            loop_id,
            "test prompt",
            Some(worktree_path.to_string_lossy().to_string()),
            worktree_path.to_string_lossy().to_string(),
        );
        entry.pid = DEAD_PID_SENTINEL;
        entry.started = started;

        let ralph_dir = repo_root.join(".ralph");
        fs::create_dir_all(&ralph_dir).unwrap();
        let registry_path = ralph_dir.join("loops.json");

        #[derive(serde::Serialize)]
        struct Wrapper<'a> {
            loops: &'a [LoopEntry],
        }
        let loops = vec![entry];
        let json = serde_json::to_string_pretty(&Wrapper { loops: &loops }).unwrap();
        fs::write(&registry_path, json).unwrap();
    }

    #[test]
    fn test_find_reusable_worktree_by_name_happy_path() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let worktree = create_worktree(temp_dir.path(), "my-exact-name", &config).unwrap();

        register_completed_entry(
            temp_dir.path(),
            "my-exact-name",
            &worktree.path,
            Utc::now() - chrono::Duration::seconds(60),
        );

        let result = find_reusable_worktree_by_name(temp_dir.path(), "my-exact-name").unwrap();
        let reusable = result.expect("expected a reusable worktree");
        assert_eq!(reusable.loop_id, "my-exact-name");
        assert_eq!(reusable.path, worktree.path);
    }

    #[test]
    fn test_find_reusable_worktree_by_name_live_entry_errors() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let worktree = create_worktree(temp_dir.path(), "my-exact-name", &config).unwrap();

        let registry = crate::loop_registry::LoopRegistry::new(temp_dir.path());
        let entry = crate::loop_registry::LoopEntry::with_id(
            "my-exact-name",
            "running prompt",
            Some(worktree.path.to_string_lossy().to_string()),
            worktree.path.to_string_lossy().to_string(),
        );
        registry.register(entry).unwrap();

        let result = find_reusable_worktree_by_name(temp_dir.path(), "my-exact-name");
        assert!(
            result.is_err(),
            "a still-running worktree must not be reusable by name"
        );
    }

    // -------------------------------------------------------------------------
    // U2: clean_worktree_runtime_artifacts tests
    // -------------------------------------------------------------------------

    /// Set up a worktree with a fully populated `.ralph/` directory
    /// containing both removable artifacts and must-be-preserved
    /// symlinks/files. Returns the worktree path.
    fn setup_worktree_with_artifacts(repo_root: &Path) -> PathBuf {
        let config = WorktreeConfig::default();
        let worktree = create_worktree(repo_root, "clean-test-loop", &config).unwrap();

        let ralph_dir = worktree.path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::create_dir_all(ralph_dir.join("diagnostics")).unwrap();

        // Removable runtime artifacts
        fs::write(ralph_dir.join("events.jsonl"), "{\"x\":1}\n").unwrap();
        fs::write(
            ralph_dir.join("events-20250101-120000.jsonl"),
            "{\"y\":2}\n",
        )
        .unwrap();
        fs::write(ralph_dir.join("current-events"), ".ralph/events.jsonl\n").unwrap();
        fs::write(ralph_dir.join("history.jsonl"), "{\"h\":1}\n").unwrap();
        fs::write(
            ralph_dir.join("history-20250101-120000.jsonl"),
            "{\"h\":2}\n",
        )
        .unwrap();
        fs::write(ralph_dir.join("diagnostics/log.jsonl"), "{\"d\":1}\n").unwrap();
        fs::write(ralph_dir.join("urgent-steer.json"), "{}").unwrap();
        fs::write(ralph_dir.join("current-loop-id"), "clean-test-loop\n").unwrap();
        fs::write(agent_dir.join("scratchpad.md"), "# scratch\n").unwrap();
        fs::write(
            agent_dir.join("scratchpad-clean-test-loop.md"),
            "# scratch-loop\n",
        )
        .unwrap();
        fs::write(agent_dir.join("tasks.jsonl"), "{}\n").unwrap();
        fs::write(agent_dir.join("summary.md"), "# summary\n").unwrap();
        fs::write(agent_dir.join("handoff.md"), "# handoff\n").unwrap();
        fs::write(agent_dir.join("decisions.md"), "# decisions\n").unwrap();
        fs::create_dir_all(ralph_dir.join("review/plan")).unwrap();
        fs::write(ralph_dir.join("review/plan/report.md"), "# report\n").unwrap();

        // Must-be-preserved files
        fs::write(agent_dir.join("context.md"), "# Worktree Context\n").unwrap();

        worktree.path.clone()
    }

    #[test]
    fn test_clean_worktree_runtime_artifacts_archives_runs_state() {
        // Reuse must clear every live runtime path while retaining the
        // prior run under one immutable reuse-history directory.
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let worktree_path = setup_worktree_with_artifacts(temp_dir.path());
        let ralph_dir = worktree_path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");

        let archive = clean_worktree_runtime_artifacts(&worktree_path, None)
            .unwrap()
            .expect("populated prior run should produce an archive");

        // Removables gone
        assert!(!ralph_dir.join("events.jsonl").exists());
        assert!(!ralph_dir.join("events-20250101-120000.jsonl").exists());
        assert!(!ralph_dir.join("current-events").exists());
        assert!(!ralph_dir.join("history.jsonl").exists());
        assert!(!ralph_dir.join("history-20250101-120000.jsonl").exists());
        assert!(!ralph_dir.join("diagnostics").exists());
        assert!(!ralph_dir.join("urgent-steer.json").exists());
        assert!(!ralph_dir.join("current-loop-id").exists());
        assert!(!agent_dir.join("scratchpad.md").exists());
        assert!(!agent_dir.join("scratchpad-clean-test-loop.md").exists());
        assert!(!agent_dir.join("tasks.jsonl").exists());
        assert!(!agent_dir.join("summary.md").exists());
        assert!(!agent_dir.join("handoff.md").exists());
        assert!(!agent_dir.join("decisions.md").exists());
        assert!(!ralph_dir.join("review").exists());

        assert_eq!(
            fs::read_to_string(archive.join("events.jsonl")).unwrap(),
            "{\"x\":1}\n"
        );
        assert_eq!(
            fs::read_to_string(archive.join("agent/summary.md")).unwrap(),
            "# summary\n"
        );
        assert_eq!(
            fs::read_to_string(archive.join("agent/handoff.md")).unwrap(),
            "# handoff\n"
        );
        assert_eq!(
            fs::read_to_string(archive.join("agent/decisions.md")).unwrap(),
            "# decisions\n"
        );
        assert_eq!(
            fs::read_to_string(archive.join("review/plan/report.md")).unwrap(),
            "# report\n"
        );
        let resume_context = fs::read_to_string(agent_dir.join("resume-context.md")).unwrap();
        assert!(resume_context.contains("advisory evidence"));

        // Parent directories still exist (clean slate, not nuked)
        assert!(ralph_dir.is_dir());
        assert!(agent_dir.is_dir());

        // context.md must be preserved
        assert!(agent_dir.join("context.md").exists());
        let ctx = fs::read_to_string(agent_dir.join("context.md")).unwrap();
        assert!(ctx.contains("Worktree Context"));
    }

    #[cfg(unix)]
    #[test]
    fn test_clean_worktree_runtime_artifacts_preserves_symlinks() {
        // The shared symlinks (memories, specs, code tasks) must
        // survive cleanup, because they are the cross-loop bridge
        // that makes worktree reuse valuable.
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let worktree_path = setup_worktree_with_artifacts(temp_dir.path());
        let ralph_dir = worktree_path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");

        // Create main-repo memories/specs/tasks so the symlinks have
        // real targets to point at.
        fs::create_dir_all(temp_dir.path().join(".ralph/agent")).unwrap();
        fs::write(
            temp_dir.path().join(".ralph/agent/memories.md"),
            "# main memories\n",
        )
        .unwrap();
        fs::create_dir_all(temp_dir.path().join(".ralph/specs")).unwrap();
        fs::create_dir_all(temp_dir.path().join(".ralph/tasks")).unwrap();

        // Manually set up the worktree symlinks (the same way
        // LoopContext::setup_worktree_symlinks does, but bypassing
        // the LoopContext API to keep this test focused on the
        // cleanup contract).
        std::os::unix::fs::symlink(
            temp_dir.path().join(".ralph/agent/memories.md"),
            agent_dir.join("memories.md"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            temp_dir.path().join(".ralph/specs"),
            ralph_dir.join("specs"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            temp_dir.path().join(".ralph/tasks"),
            ralph_dir.join("tasks"),
        )
        .unwrap();

        clean_worktree_runtime_artifacts(&worktree_path, None).unwrap();

        // Symlinks must still exist
        assert!(agent_dir.join("memories.md").is_symlink());
        assert!(ralph_dir.join("specs").is_symlink());
        assert!(ralph_dir.join("tasks").is_symlink());
        // And they must still point at the same targets
        assert_eq!(
            fs::read_link(agent_dir.join("memories.md")).unwrap(),
            temp_dir.path().join(".ralph/agent/memories.md")
        );
    }

    #[test]
    fn test_clean_worktree_runtime_artifacts_missing_dir_errors() {
        // Calling cleanup on a non-existent worktree must surface
        // `WorktreeError::NotFound` rather than silently succeed —
        // the caller uses this signal to abort before launching the
        // loop.
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let phantom = temp_dir.path().join(".worktrees/ghost");
        let result = clean_worktree_runtime_artifacts(&phantom, None);
        assert!(matches!(result, Err(WorktreeError::NotFound(_))));
    }

    #[test]
    fn test_clean_worktree_runtime_artifacts_idempotent() {
        // Cleanup must succeed when most of the targeted files do
        // not exist (e.g. diagnostics was never enabled, the
        // scratchpad was never written). This is the
        // "diagnostics-not-enabled" edge case from the spec.
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let worktree = create_worktree(temp_dir.path(), "clean-idempotent", &config).unwrap();

        // No runtime artifacts were ever written; just the worktree
        // directory and its git metadata exist.
        assert!(
            clean_worktree_runtime_artifacts(&worktree.path, None)
                .unwrap()
                .is_none()
        );

        // Second call must also succeed.
        assert!(
            clean_worktree_runtime_artifacts(&worktree.path, None)
                .unwrap()
                .is_none()
        );

        // The worktree's .ralph/ and .ralph/agent/ must now exist as
        // empty directories.
        let ralph_dir = worktree.path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        assert!(ralph_dir.is_dir());
        assert!(agent_dir.is_dir());
    }

    #[test]
    fn test_clean_worktree_runtime_artifacts_preserves_user_code() {
        // Cleanup must not touch tracked source files. A user who
        // reuses a worktree wants to keep the code state.
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let worktree_path = setup_worktree_with_artifacts(temp_dir.path());

        // Add a tracked source file in the worktree.
        let src = worktree_path.join("src/lib.rs");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, "pub fn hello() {}\n").unwrap();

        clean_worktree_runtime_artifacts(&worktree_path, None).unwrap();

        assert!(src.exists(), "user code must not be removed");
        assert_eq!(fs::read_to_string(&src).unwrap(), "pub fn hello() {}\n");
    }

    // -------------------------------------------------------------------------
    // U1 (2026-08-03-004): resume manifest capture during cleanup
    // -------------------------------------------------------------------------

    /// Seed accepted-boundary evidence into the fixture worktree's live
    /// runtime (replaces the generic `{"x":1}` events fixture with a
    /// parseable accepted `forge.plan.ready` boundary).
    fn seed_accepted_boundary(worktree_path: &Path) {
        use crate::parallel_forge_resume::sha256_hex;

        let ralph_dir = worktree_path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        // The generic fixture's rotated channel carries an unparseable
        // line; the boundary fixture replaces it with a clean log.
        let _ = fs::remove_file(ralph_dir.join("events-20250101-120000.jsonl"));
        let payload = "{\"plan_key\":\"pf-wt\"}";
        let event_line = format!(
            "{{\"ts\":\"2026-08-03T00:00:00Z\",\"iteration\":1,\"hat\":\"planner\",\"topic\":\"forge.plan.ready\",\"triggered\":\"guardian\",\"payload\":{}}}\n",
            serde_json::to_string(payload).unwrap()
        );
        fs::write(ralph_dir.join("events.jsonl"), &event_line).unwrap();

        let payload_digest = sha256_hex(payload.as_bytes());
        let transition_id =
            crate::event_loop::accepted_transition::AcceptedTransition::compute_transition_id(
                "old-loop-wt",
                "planner:1",
                "rev-1",
                "forge.plan.ready:planner",
                &payload_digest,
            );
        let outbox_line = serde_json::json!({
            "activation_id": "planner:1",
            "committed_at": "2026-08-03T00:00:01Z",
            "contract_revision": "rev-1",
            "delivered": false,
            "loop_id": "old-loop-wt",
            "payload_digest": payload_digest,
            "topic": "forge.plan.ready",
            "transition_id": transition_id,
        });
        fs::write(
            agent_dir.join("accepted-transitions.jsonl"),
            format!("{outbox_line}\n"),
        )
        .unwrap();
        // The generic fixture tasks.jsonl (`{}`) is malformed; replace
        // it with a valid ledger line so capture stays complete.
        fs::write(
            agent_dir.join("tasks.jsonl"),
            "{\"id\":\"task-1\",\"title\":\"U1\",\"key\":\"forge:pf-wt:U1\",\"status\":\"closed\",\"priority\":1,\"created\":\"2026-08-03T00:00:00Z\"}\n",
        )
        .unwrap();
        fs::write(ralph_dir.join("current-loop-id"), "old-loop-wt\n").unwrap();
    }

    #[test]
    fn test_clean_worktree_runtime_artifacts_captures_resume_manifest() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let worktree_path = setup_worktree_with_artifacts(temp_dir.path());
        seed_accepted_boundary(&worktree_path);
        let ralph_dir = worktree_path.join(".ralph");

        let inputs = crate::parallel_forge_resume::CaptureInputs {
            plan_path: "docs/plans/clean-test-loop.md".to_string(),
            plan_digest: crate::parallel_forge_resume::sha256_hex(b"plan"),
            preset_name: "parallel-forge".to_string(),
            config_digest: crate::parallel_forge_resume::sha256_hex(b"config"),
            worktree_name: "clean-test-loop".to_string(),
        };

        let archive = clean_worktree_runtime_artifacts(&worktree_path, Some(&inputs))
            .unwrap()
            .expect("populated prior run should produce an archive");

        // The manifest lands inside the archive, captured BEFORE the
        // live files moved.
        let manifest_path = archive.join(crate::parallel_forge_resume::MANIFEST_FILE_NAME);
        assert!(
            manifest_path.exists(),
            "resume manifest must be written into the reuse archive"
        );
        let manifest = crate::parallel_forge_resume::read_manifest(&manifest_path)
            .expect("manifest must parse");
        assert!(
            manifest.is_complete(),
            "manifest must be complete: {:?}",
            manifest.incomplete_reasons
        );
        assert_eq!(manifest.boundary.accepted.len(), 1);
        assert_eq!(manifest.boundary.accepted[0].topic, "forge.plan.ready");
        assert_eq!(manifest.boundary.pending_hat.as_deref(), Some("guardian"));
        assert_eq!(manifest.identity.worktree_name, "clean-test-loop");
        assert_eq!(manifest.identity.loop_id, "old-loop-wt");
        assert_eq!(manifest.tasks.len(), 1);
        assert_eq!(manifest.tasks[0].task_key, "forge:pf-wt:U1");

        // Existing cleanup semantics unchanged: live event log archived.
        assert!(!ralph_dir.join("events.jsonl").exists());
        assert!(
            fs::read_to_string(archive.join("events.jsonl"))
                .unwrap()
                .contains("forge.plan.ready")
        );
    }

    #[test]
    fn test_clean_worktree_runtime_artifacts_no_inputs_keeps_legacy_semantics() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let worktree_path = setup_worktree_with_artifacts(temp_dir.path());

        let archive = clean_worktree_runtime_artifacts(&worktree_path, None)
            .unwrap()
            .expect("populated prior run should produce an archive");

        // No capture inputs → no manifest (legacy behavior preserved).
        assert!(
            !archive
                .join(crate::parallel_forge_resume::MANIFEST_FILE_NAME)
                .exists()
        );
    }
}
