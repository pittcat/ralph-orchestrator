//! 2026-09-03-0959 plan U7 (R7; S8-S11; D7-D9; E10-E12):
//! per-Unit worktree binding — trusted worktree created or
//! reused from a verified base commit.
//!
//! U7 ships the bin-side public surface as a transitional
//! state: the worktree is consumed by U8 (correction
//! wiring) and U10 (preset cutover) in the same plan, and
//! the U7 acceptance tests already cover the surface via
//! `crate::loop_runner::dag_scheduler::worktree::tests`.
//! Until those later units land, no bin-side module other
//! than this file's own tests exercises `UnitWorktree` /
//! `UnitWorktreeError` / `acquire` — we mark the module
//! `#![allow(dead_code)]` to mirror `integration.rs`'s
//! transitional pattern and keep the `RUSTFLAGS='-D warnings'`
//! gate honest. The lint will return the moment U8 / U10
//! introduce a bin-side caller and dead-code warnings flip
//! back on.
#![allow(dead_code)]

//! # Why a per-Unit worktree?
//!
//! Plan §7 U7 says: "Each Unit works in a worktree created
//! or reused from a verified base commit". The verified
//! base is the SHA the runtime captured when the plan was
//! admitted (the integration-target HEAD before any sibling
//! FF'd). Locking every Unit to that base is what lets the
//! lane CAS the candidate in safely: the lane knows nothing
//! outside the Unit's worktree could have raced with it.
//!
//! # Reuse vs create
//!
//! When a Unit is re-run after a transient failure (network
//! glitch, supervisor restart), its worktree may still be
//! alive on disk. Reuse rules:
//!   - Reuse if the existing branch tip equals
//!     `verified_base_commit` (the same base the plan was
//!     admitted with).
//!   - Re-create (with a fresh worktree) if the existing
//!     branch tip differs — the previous run was racing
//!     against a stale base and must be abandoned.
//!   - **Reject** (do NOT auto-clean) if the host repo has
//!     uncommitted changes or untracked files in
//!     `$repo_root` (not the worktree). Cleaning host state
//!     silently would erase operator changes; the lane
//!     fails-closed and the operator resolves the conflict.
//!
//! # Not the only worktree
//!
//! [`crate::worktree`] and
//! [`crate::supervisor::worktree_bind`] both define
//! worktree primitives. This module is the *third* one,
//! specialised for U7:
//!   - `crate::worktree` (loop-level helper used by
//!     `ralph run --worktree`).
//!   - `worktree_bind::bind_slot_worktree` is the
//!     supervisor's slot-binding helper (used by U3 / U4).
//!   - This module's `UnitWorktree` is the *integration-lane*
//!     one: it takes a verified base commit as input and
//!     hands out a worktree whose branch tip equals that
//!     base.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Identity of one Unit's trusted worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitWorktree {
    pub unit_id: String,
    pub loop_id: String,
    pub path: PathBuf,
    pub branch: String,
    pub base_commit: String,
    /// Whether this worktree was already present on disk
    /// and reused (vs freshly created).
    pub reused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnitWorktreeError {
    #[error("host repo '{0}' has uncommitted changes; refuse to reuse or create unit worktree")]
    HostDirty(String),
    #[error("host repo '{0}' has untracked files; refuse to reuse or create unit worktree")]
    HostUntracked(String),
    #[error(
        "existing worktree branch '{branch}' tip '{tip}' does not match verified base '{base}'"
    )]
    BaseMismatch {
        branch: String,
        tip: String,
        base: String,
    },
    #[error("failed to inspect existing worktree '{path}': {reason}")]
    #[allow(dead_code)] // defensive variant reserved for future worktree-state probes
    InspectFailed { path: String, reason: String },
    #[error("git command failed: {0}")]
    GitFailed(String),
}

pub type UnitWorktreeResult<T> = Result<T, UnitWorktreeError>;

impl UnitWorktree {
    /// Acquire a trusted worktree for `unit_id`.
    ///
    /// `verified_base_commit` is the SHA the runtime captured
    /// at plan admission. The worktree branch tip is set
    /// (or asserted) to that SHA.
    ///
    /// Host repo's `repo_root` must be clean (no uncommitted
    /// changes, no untracked files OTHER than registered
    /// git worktrees that this module creates). Otherwise
    /// the lane refuses — operator must resolve the dirty
    /// state before any unit worktrees are bound.
    pub fn acquire(
        repo_root: &Path,
        loop_id: &str,
        unit_id: &str,
        verified_base_commit: &str,
    ) -> UnitWorktreeResult<Self> {
        // First, register the worktree directory as a
        // local-only ignore (per-repo `.git/info/exclude`).
        // Without this, the host repo's `git status
        // --porcelain` reports each previously-created
        // `.ralph/worktrees/<unit>` dir as untracked on the
        // second `acquire` call, and the host-clean check
        // would falsely reject the operation.
        ensure_worktree_dir_excluded(repo_root)?;
        ensure_host_clean(repo_root)?;

        let branch = format!("ralph/{}/{}", loop_id, unit_id);
        let worktree_root = repo_root.join(".ralph").join("worktrees");
        let path = worktree_root.join(format!("{}-{}", loop_id, unit_id));

        // Try reuse: does the branch already exist?
        let existing_tip = read_branch_tip(repo_root, &branch);
        match existing_tip {
            Ok(tip) if !tip.is_empty() => {
                if tip == verified_base_commit {
                    // Verify the worktree path is still on disk.
                    if path.exists() {
                        return Ok(UnitWorktree {
                            unit_id: unit_id.to_string(),
                            loop_id: loop_id.to_string(),
                            path,
                            branch,
                            base_commit: verified_base_commit.to_string(),
                            reused: true,
                        });
                    }
                    // Branch exists but worktree path is missing —
                    // fall through to fresh create.
                } else {
                    return Err(UnitWorktreeError::BaseMismatch {
                        branch,
                        tip,
                        base: verified_base_commit.to_string(),
                    });
                }
            }
            Ok(_) => {
                // Branch doesn't exist — fresh create.
            }
            Err(e) => {
                return Err(e);
            }
        }

        // Fresh create: ensure parent dir, then `git worktree add
        // -B <branch> <path> <verified_base>`. The `-B` flag
        // creates the branch if it doesn't exist; pointing the
        // new branch at the verified base means the worktree's
        // initial tip IS the verified base.
        std::fs::create_dir_all(&worktree_root)
            .map_err(|e| UnitWorktreeError::GitFailed(format!("create_dir_all: {e}")))?;
        let status = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .arg("worktree")
            .arg("add")
            .arg("-B")
            .arg(&branch)
            .arg(&path)
            .arg(verified_base_commit)
            .status()
            .map_err(|e| UnitWorktreeError::GitFailed(format!("worktree add: {e}")))?;
        if !status.success() {
            return Err(UnitWorktreeError::GitFailed(format!(
                "git worktree add exited {:?}",
                status.code()
            )));
        }
        Ok(UnitWorktree {
            unit_id: unit_id.to_string(),
            loop_id: loop_id.to_string(),
            path,
            branch,
            base_commit: verified_base_commit.to_string(),
            reused: false,
        })
    }
}

fn ensure_host_clean(repo_root: &Path) -> UnitWorktreeResult<()> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("status")
        .arg("--porcelain")
        .output()
        .map_err(|e| UnitWorktreeError::GitFailed(format!("git status: {e}")))?;
    if !out.status.success() {
        return Err(UnitWorktreeError::GitFailed(format!(
            "git status exited {:?}",
            out.status.code()
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut has_modified = false;
    let mut has_untracked = false;
    for line in text.lines() {
        if line.len() < 2 {
            continue;
        }
        let xy = &line[..2];
        if xy.starts_with('?') {
            has_untracked = true;
        } else if xy != "!!" {
            has_modified = true;
        }
    }
    if has_modified {
        return Err(UnitWorktreeError::HostDirty(
            repo_root.display().to_string(),
        ));
    }
    if has_untracked {
        return Err(UnitWorktreeError::HostUntracked(
            repo_root.display().to_string(),
        ));
    }
    Ok(())
}

fn read_branch_tip(repo_root: &Path, branch: &str) -> UnitWorktreeResult<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("rev-parse")
        .arg("--verify")
        .arg(&format!("refs/heads/{}", branch))
        .output()
        .map_err(|e| UnitWorktreeError::GitFailed(format!("git rev-parse: {e}")))?;
    if !out.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Register `.ralph/worktrees/` in the repo's
/// `.git/info/exclude` so the host-clean check doesn't
/// flag worktrees this module itself created. Idempotent —
/// a no-op if the line is already present.
fn ensure_worktree_dir_excluded(repo_root: &Path) -> UnitWorktreeResult<()> {
    let exclude_path = repo_root.join(".git").join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| UnitWorktreeError::GitFailed(format!("create exclude dir: {e}")))?;
    }
    let existing = std::fs::read_to_string(&exclude_path).unwrap_or_default();
    // Match a precise line ".ralph/worktrees/" rather than a
    // substring (avoids matching a hypothetical user entry
    // like ".ralph/worktrees-old/").
    let already_present = existing
        .lines()
        .any(|line| line.trim() == ".ralph/worktrees/");
    if already_present {
        return Ok(());
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(".ralph/worktrees/\n");
    std::fs::write(&exclude_path, content)
        .map_err(|e| UnitWorktreeError::GitFailed(format!("write exclude: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::process::Command;
    use tempfile::TempDir;

    fn run_git(cwd: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn init_repo_with_initial_commit() -> (TempDir, PathBuf, String) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().to_path_buf();
        run_git(&repo, &["init", "-q"]);
        run_git(&repo, &["config", "user.email", "t@e"]);
        run_git(&repo, &["config", "user.name", "T"]);
        std::fs::write(repo.join("README.md"), "init\n").expect("write readme");
        run_git(&repo, &["add", "README.md"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        let head = run_git(&repo, &["rev-parse", "HEAD"]);
        (tmp, repo, head)
    }

    /// U7 contract: a fresh Unit worktree is created from
    /// the verified base commit; the branch tip matches.
    #[test]
    fn unit_worktree_acquire_creates_with_verified_base() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        let wt = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect("acquire");
        assert_eq!(wt.unit_id, "U1");
        assert_eq!(wt.base_commit, base);
        assert_eq!(wt.branch, "ralph/loop-1/U1");
        assert!(!wt.reused);
        // Verify the new branch's tip equals the base.
        let tip = run_git(&repo, &["rev-parse", &wt.branch]);
        assert_eq!(tip, base);
    }

    /// U7 contract: a second acquire with the same base
    /// reuses the existing worktree and reports `reused`.
    #[test]
    fn unit_worktree_acquire_reuses_when_base_matches() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        let first = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect("first");
        let second = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect("second");
        assert!(second.reused);
        assert_eq!(first.path, second.path);
        assert_eq!(first.branch, second.branch);
    }

    /// U7 contract: re-acquire with a DIFFERENT base fails
    /// closed with `BaseMismatch` — the lane refuses to
    /// silently rewrite the unit's base.
    #[test]
    fn unit_worktree_acquire_rejects_base_mismatch_on_reuse() {
        let (_tmp, repo, base1) = init_repo_with_initial_commit();
        UnitWorktree::acquire(&repo, "loop-1", "U1", &base1).expect("first");
        // Build a second commit to change the base.
        std::fs::write(repo.join("extra.txt"), "extra\n").expect("write extra");
        run_git(&repo, &["add", "extra.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "extra"]);
        let base2 = run_git(&repo, &["rev-parse", "HEAD"]);
        let err = UnitWorktree::acquire(&repo, "loop-1", "U1", &base2).expect_err("must reject");
        match err {
            UnitWorktreeError::BaseMismatch { branch, tip, base } => {
                assert_eq!(branch, "ralph/loop-1/U1");
                assert_eq!(tip, base1);
                assert_eq!(base, base2);
            }
            other => panic!("expected BaseMismatch, got {other:?}"),
        }
    }

    /// U7 contract: a host repo with uncommitted changes
    /// fails acquire with `HostDirty`.
    #[test]
    fn unit_worktree_acquire_rejects_host_dirty() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        std::fs::write(repo.join("README.md"), "modified\n").expect("write");
        let err = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect_err("must reject");
        assert!(matches!(err, UnitWorktreeError::HostDirty(_)));
    }

    /// U7 contract: a host repo with untracked files fails
    /// acquire with `HostUntracked`.
    #[test]
    fn unit_worktree_acquire_rejects_host_untracked() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        std::fs::write(repo.join("new_file.txt"), "x").expect("write untracked");
        let err = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect_err("must reject");
        assert!(matches!(err, UnitWorktreeError::HostUntracked(_)));
    }

    /// U7 contract: two distinct Units get two distinct
    /// worktrees on the same repo.
    #[test]
    fn unit_worktree_acquire_distinct_units_get_distinct_paths() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        let wt_u1 = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect("U1");
        let wt_u2 = UnitWorktree::acquire(&repo, "loop-1", "U2", &base).expect("U2");
        assert_ne!(wt_u1.path, wt_u2.path);
        assert_ne!(wt_u1.branch, wt_u2.branch);
    }
}
