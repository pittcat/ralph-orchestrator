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
/// Returned by [`find_reusable_worktree`]. Holds enough information for
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

/// Find a reusable worktree for reuse mode (`--reuse-worktree`).
///
/// Scans `.ralph/loops.json` for completed worktree entries whose loop ID
/// (or the suffix of the `ralph/<id>` branch) starts with `prefix` and
/// returns the most recent match. Cross-validates against
/// `git worktree list --porcelain` to ensure git also knows the worktree.
///
/// # Semantics
///
/// - Only entries with `worktree_path == Some(_)` are considered (primary
///   loops running in the main workspace are excluded).
/// - Only entries whose PID is no longer alive (the loop has finished or
///   crashed) are considered. The combined `is_alive()` check (PID +
///   directory existence) is the canonical "completed" detector.
/// - If the recorded `worktree_path` no longer exists on disk, the entry
///   is silently skipped — these are zombie records and we want to fall
///   through to "create new worktree" rather than fail.
/// - The worktree must also appear in `git worktree list` output, so we
///   never reuse a path that git has forgotten about.
/// - When multiple entries match the same prefix, the one with the most
///   recent `started` timestamp wins.
///
/// # Arguments
///
/// * `repo_root` - Root of the git repository (used to locate
///   `.ralph/loops.json` and to invoke `git worktree list`).
/// * `prefix` - Loop name prefix to match against (the same prefix used
///   by `LoopNameGenerator::generate_unique_with_prefix`).
///
/// # Returns
///
/// - `Ok(Some(_))` if a completed, git-known worktree matches.
/// - `Ok(None)` if no entry matches, all matches have stale directories,
///   or the registry is missing/empty.
/// - `Err(_)` only on I/O errors reading the registry or running git.
pub fn find_reusable_worktree(
    repo_root: impl AsRef<Path>,
    prefix: &str,
) -> Result<Option<ReusableWorktree>, WorktreeError> {
    if prefix.is_empty() {
        return Ok(None);
    }

    let repo_root = repo_root.as_ref();
    let registry_path = repo_root.join(".ralph").join("loops.json");

    // Missing registry ⇒ nothing to reuse, but not an error.
    if !registry_path.exists() {
        return Ok(None);
    }

    let entries: Vec<LoopEntry> = read_loop_registry_entries(&registry_path)?;
    if entries.is_empty() {
        return Ok(None);
    }

    // Pre-compute the set of git-known worktree paths for cross-validation.
    // `git worktree list` reports canonicalized paths (e.g. on macOS the
    // `/var` symlink resolves to `/private/var`), but registry entries
    // store whatever absolute path the original loop happened to be
    // running with. Canonicalize both sides so symlinked prefixes don't
    // cause spurious mismatches.
    let known_paths: HashSet<PathBuf> = match list_worktrees(repo_root) {
        Ok(list) => list
            .into_iter()
            .map(|wt| canonicalize_for_compare(&wt.path))
            .collect(),
        Err(_) => HashSet::new(),
    };

    // Iterate once; the last-write-wins in chronological order is fine
    // because we walk `entries` in the order they were registered, and
    // tie-break by comparing `started` timestamps explicitly.
    let mut best: Option<(DateTime<Utc>, ReusableWorktree)> = None;
    for entry in entries {
        let wt_path = match &entry.worktree_path {
            Some(p) => PathBuf::from(p),
            None => continue, // skip primary (non-worktree) loops
        };

        // Extract the loop ID component (worktree dir name).
        let loop_id = match wt_path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        // Match by loop ID (the most common case). The branch name
        // `ralph/<id>` is derivable from the directory name so we do
        // not need a separate prefix match.
        if !loop_id.starts_with(prefix) && !entry.id.starts_with(prefix) {
            continue;
        }

        // Reuse is opt-in for completed worktrees only. `is_alive()` is
        // the canonical "is this loop still running?" check used
        // throughout the registry; for worktree entries it combines
        // PID liveness with directory existence.
        if entry.is_alive() {
            continue;
        }

        // The recorded worktree_path must still exist on disk.
        if !wt_path.is_dir() {
            tracing::debug!(
                "Skipping reusable candidate {}: directory no longer exists",
                wt_path.display()
            );
            continue;
        }

        // Cross-validate against git's view of the worktrees. If git
        // has pruned the worktree, the branch ref is unreliable. We
        // canonicalize the candidate path so symlinked prefixes (the
        // classic example being `/var` ↔ `/private/var` on macOS) do
        // not produce false negatives.
        let candidate_canonical = canonicalize_for_compare(&wt_path);
        if !known_paths.is_empty() && !known_paths.contains(&candidate_canonical) {
            tracing::debug!(
                "Skipping reusable candidate {}: not in git worktree list",
                wt_path.display()
            );
            continue;
        }

        let candidate = ReusableWorktree {
            path: wt_path.clone(),
            branch: format!("ralph/{loop_id}"),
            loop_id: loop_id.clone(),
            started: entry.started,
            head: get_head_commit(&wt_path).ok(),
        };

        match &best {
            Some((existing_started, _)) if *existing_started >= candidate.started => {
                // keep existing
            }
            _ => best = Some((entry.started, candidate)),
        }
    }

    Ok(best.map(|(_, w)| w))
}

/// Find a reusable worktree by its exact loop/worktree name.
///
/// This is the precise-match counterpart to [`find_reusable_worktree`].
/// It is used when the operator passes `--worktree-name <name>` together
/// with `--reuse-worktree`: we look for a registry entry whose loop ID
/// or worktree directory name equals `name`, verify the loop is no longer
/// alive, and cross-check the directory against `git worktree list`.
///
/// Returns `Ok(Some(_))` if a matching, reusable worktree is found;
/// `Ok(None)` if no such worktree exists or the directory is gone;
/// `Err(_)` if the worktree is still in use by a live loop.
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
    // (completed worktree loops). `find_reusable_worktree` is a
    // read-only lookup; performing its own filtering means the
    // registry can keep its auto-cleanup invariant intact.
    // `LoopRegistry::list()` because that method takes the registry
    // write-lock and prunes entries whose PID is no longer alive,
    // which would erase the very entries we are trying to find
    // (completed worktree loops). `find_reusable_worktree` is a
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

/// Remove a runtime artifact from a worktree if it exists.
///
/// We deliberately use `fs::remove_file` / `fs::remove_dir_all` and treat
/// `NotFound` as a no-op so the cleanup is idempotent. The caller does
/// not need to know in advance whether the file was created by a
/// previous run.
fn remove_if_exists(path: &Path) -> std::io::Result<()> {
    match fs::metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Remove files matching a glob pattern under a directory, leaving
/// non-matches alone.
///
/// We use a lightweight manual filter instead of pulling in the
/// `glob` crate to keep `ralph-core`'s dependency footprint
/// unchanged. The match is on `file_name()` only, so paths like
/// `.ralph/events-20250101-120000.jsonl` (which the spec needs to
/// support) are picked up while the parent directory is left intact.
fn remove_files_matching(dir: &Path, suffix: &str, prefix: &str) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with(prefix) || !name_str.ends_with(suffix) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// Clean Ralph runtime artifacts from an existing worktree directory
/// in preparation for reuse.
///
/// `find_reusable_worktree` finds a worktree that was previously used
/// by a finished loop. The directory still contains that loop's
/// event history, scratchpad, tasks, and diagnostics, which we want
/// to discard so the new loop starts on a clean slate. We *preserve*
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
///
/// # Files preserved
///
/// - `.ralph/agent/context.md` (worktree metadata)
/// - `.ralph/agent/memories.md` (symlink into main repo)
/// - `.ralph/specs/`, `.ralph/tasks/` (symlinks into main repo)
/// - The `.ralph/` and `.ralph/agent/` directories themselves
/// - The git worktree (branch, history, tracked files)
///
/// # Error propagation
///
/// Any I/O error during cleanup is propagated via `WorktreeError::Io`
/// so the caller can exit before launching the loop. A partial
/// cleanup is worse than a refused start: starting on a stale
/// state would silently corrupt the new run.
pub fn clean_worktree_runtime_artifacts(
    worktree_path: impl AsRef<Path>,
) -> Result<(), WorktreeError> {
    let worktree_path = worktree_path.as_ref();
    if !worktree_path.is_dir() {
        return Err(WorktreeError::NotFound(
            worktree_path.to_string_lossy().to_string(),
        ));
    }

    let ralph_dir = worktree_path.join(".ralph");
    let agent_dir = ralph_dir.join("agent");

    // --- .ralph/ top-level artifacts ---
    // events.jsonl
    remove_if_exists(&ralph_dir.join("events.jsonl"))?;
    // events-YYYYMMDD-HHMMSS.jsonl
    remove_files_matching(&ralph_dir, ".jsonl", "events-")?;
    // current-events marker
    remove_if_exists(&ralph_dir.join("current-events"))?;
    // history.jsonl
    remove_if_exists(&ralph_dir.join("history.jsonl"))?;
    // history-*.jsonl
    remove_files_matching(&ralph_dir, ".jsonl", "history-")?;
    // diagnostics/ (full subtree)
    let diagnostics_dir = ralph_dir.join("diagnostics");
    if diagnostics_dir.is_dir() {
        fs::remove_dir_all(&diagnostics_dir)?;
    }
    // urgent-steer.json
    remove_if_exists(&ralph_dir.join("urgent-steer.json"))?;
    // current-loop-id
    remove_if_exists(&ralph_dir.join("current-loop-id"))?;

    // --- .ralph/agent/ artifacts ---
    // scratchpad.md
    remove_if_exists(&agent_dir.join("scratchpad.md"))?;
    // scratchpad-{loop_id}.md (ephemeral isolation artifacts)
    remove_files_matching(&agent_dir, ".md", "scratchpad-")?;
    // tasks.jsonl
    remove_if_exists(&agent_dir.join("tasks.jsonl"))?;
    // summary.md
    remove_if_exists(&agent_dir.join("summary.md"))?;
    // handoff.md
    remove_if_exists(&agent_dir.join("handoff.md"))?;

    // --- Re-create the parent directories so a fresh loop has a
    // clean slate to write into. We deliberately do not create
    // `.ralph/specs/` or `.ralph/tasks/` here — those are symlinks
    // set up by `LoopContext::setup_worktree_symlinks` and pointing
    // them at a non-existent target would be worse than leaving them
    // absent (the next `setup_*_symlink` call will create them
    // idempotently).
    fs::create_dir_all(&ralph_dir)?;
    fs::create_dir_all(&agent_dir)?;

    tracing::info!(
        "Cleaned runtime artifacts in worktree {}",
        worktree_path.display()
    );
    Ok(())
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

    // -------------------------------------------------------------------------
    // U1: find_reusable_worktree tests
    // -------------------------------------------------------------------------

    /// Helper: write a completed LoopEntry into `.ralph/loops.json` for a
    /// real, on-disk worktree directory.
    ///
    /// We bypass `LoopRegistry::register()` on purpose: that method takes
    /// the registry write-lock and prunes dead-PID entries inside
    /// `with_lock`, which would erase the very entry we are trying to
    /// stage. A read-only caller (`find_reusable_worktree`) must be
    /// able to see completed entries without first triggering the
    /// registry's auto-cleanup. The test therefore writes the JSON file
    /// directly using the same on-disk shape the registry uses, so the
    /// lookup code path is exercised end-to-end.
    ///
    /// The PID is set to a sentinel that is not running on any test
    /// machine, so the entry behaves as "completed" (`is_alive() ==
    /// false`) without us having to wait for a real process to exit.
    /// We use a value above Linux's `PID_MAX_LIMIT` (typically 4_194_304
    /// on 64-bit systems, much lower on 32-bit) so that `kill(pid,
    /// None)` returns ESRCH and `is_alive()` reports the process as
    /// dead. A value with the high bit set (e.g. `0x7fff_ffff`) would
    /// wrap into a negative `i32` and be interpreted by `kill(2)` as
    /// "send to process group -1", which falsely succeeds.
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
    fn test_find_reusable_worktree_happy_path() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // Create a real worktree (git knows about it)
        let config = WorktreeConfig::default();
        let worktree =
            create_worktree(temp_dir.path(), "fix-header-swift-peacock", &config).unwrap();

        // Register a completed entry pointing at that worktree
        register_completed_entry(
            temp_dir.path(),
            "fix-header-swift-peacock",
            &worktree.path,
            Utc::now() - chrono::Duration::seconds(60),
        );

        // Look up by prefix that matches the loop_id
        let result = find_reusable_worktree(temp_dir.path(), "fix-header").unwrap();
        assert!(result.is_some(), "expected a reusable worktree");
        let reusable = result.unwrap();
        assert_eq!(reusable.loop_id, "fix-header-swift-peacock");
        assert_eq!(reusable.path, worktree.path);
        assert_eq!(reusable.branch, "ralph/fix-header-swift-peacock");
    }

    #[test]
    fn test_find_reusable_worktree_no_match_returns_none() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // No registry, no worktree — should be a clean None.
        let result = find_reusable_worktree(temp_dir.path(), "does-not-exist").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_find_reusable_worktree_picks_most_recent() {
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let older = create_worktree(temp_dir.path(), "fix-header-swift-peacock", &config).unwrap();
        let newer = create_worktree(temp_dir.path(), "fix-header-bright-falcon", &config).unwrap();

        let older_started = Utc::now() - chrono::Duration::seconds(120);
        let newer_started = Utc::now() - chrono::Duration::seconds(10);

        register_completed_entry(
            temp_dir.path(),
            "fix-header-swift-peacock",
            &older.path,
            older_started,
        );
        register_completed_entry(
            temp_dir.path(),
            "fix-header-bright-falcon",
            &newer.path,
            newer_started,
        );

        let result = find_reusable_worktree(temp_dir.path(), "fix-header").unwrap();
        let reusable = result.expect("expected a reusable worktree");
        assert_eq!(
            reusable.loop_id, "fix-header-bright-falcon",
            "the more recently started worktree should win"
        );
    }

    #[test]
    fn test_find_reusable_worktree_excludes_alive_entry() {
        // An entry whose PID is still alive must NOT be considered for
        // reuse. The test mirrors the live-LoopEntry contract by writing
        // a registry entry with the current PID, which is always alive.
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let config = WorktreeConfig::default();
        let worktree =
            create_worktree(temp_dir.path(), "fix-header-swift-peacock", &config).unwrap();

        let registry = crate::loop_registry::LoopRegistry::new(temp_dir.path());
        let entry = crate::loop_registry::LoopEntry::with_id(
            "fix-header-swift-peacock",
            "running prompt",
            Some(worktree.path.to_string_lossy().to_string()),
            worktree.path.to_string_lossy().to_string(),
        );
        registry.register(entry).unwrap();

        // Same prefix, but the live entry must be filtered out.
        let result = find_reusable_worktree(temp_dir.path(), "fix-header").unwrap();
        assert!(
            result.is_none(),
            "a still-running worktree should not be reusable"
        );
    }

    #[test]
    fn test_find_reusable_worktree_skips_missing_directory() {
        // R4: a registry entry that points at a deleted worktree
        // directory must be treated as "no match" rather than a hard
        // error, so the caller can fall through to "create new".
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let phantom = temp_dir.path().join(".worktrees/fix-header-swift-peacock");
        // Note: the directory is *not* created.

        register_completed_entry(
            temp_dir.path(),
            "fix-header-swift-peacock",
            &phantom,
            Utc::now() - chrono::Duration::seconds(60),
        );

        let result = find_reusable_worktree(temp_dir.path(), "fix-header").unwrap();
        assert!(
            result.is_none(),
            "a missing worktree directory should be silently skipped"
        );
    }

    #[test]
    fn test_find_reusable_worktree_excludes_primary_entry() {
        // Primary loops have worktree_path == None and must not be
        // treated as reusable worktrees.
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        // No worktree; register a primary entry.
        let registry = crate::loop_registry::LoopRegistry::new(temp_dir.path());
        let mut entry = crate::loop_registry::LoopEntry::with_id(
            "fix-header-primary",
            "primary prompt",
            None::<String>,
            temp_dir.path().to_string_lossy().to_string(),
        );
        entry.pid = 0x7fff_ffff;
        registry.register(entry).unwrap();

        let result = find_reusable_worktree(temp_dir.path(), "fix-header").unwrap();
        assert!(
            result.is_none(),
            "primary (non-worktree) entries must not be reused"
        );
    }

    #[test]
    fn test_find_reusable_worktree_empty_prefix() {
        // An empty prefix would match every worktree; treat as "no match"
        // to keep the contract explicit at the call site.
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let result = find_reusable_worktree(temp_dir.path(), "").unwrap();
        assert!(result.is_none());
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

        // Must-be-preserved files
        fs::write(agent_dir.join("context.md"), "# Worktree Context\n").unwrap();

        worktree.path.clone()
    }

    #[test]
    fn test_clean_worktree_runtime_artifacts_removes_runs_state() {
        // The cleanup must delete every runtime artifact listed in the
        // spec, including the event/history rotation files and the
        // diagnostics directory tree.
        let temp_dir = TempDir::new().unwrap();
        init_git_repo(temp_dir.path());

        let worktree_path = setup_worktree_with_artifacts(temp_dir.path());
        let ralph_dir = worktree_path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");

        clean_worktree_runtime_artifacts(&worktree_path).unwrap();

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

        clean_worktree_runtime_artifacts(&worktree_path).unwrap();

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
        let result = clean_worktree_runtime_artifacts(&phantom);
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
        clean_worktree_runtime_artifacts(&worktree.path).unwrap();

        // Second call must also succeed.
        clean_worktree_runtime_artifacts(&worktree.path).unwrap();

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

        clean_worktree_runtime_artifacts(&worktree_path).unwrap();

        assert!(src.exists(), "user code must not be removed");
        assert_eq!(fs::read_to_string(&src).unwrap(), "pub fn hello() {}\n");
    }
}
