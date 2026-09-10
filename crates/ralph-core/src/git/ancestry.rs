//! Boundary-correct git ancestry check via `git merge-base --is-ancestor`.
//!
//! Commit OIDs must never be compared with `starts_with`: a short or
//! attacker-chosen prefix of a descendant SHA is a string prefix of
//! that SHA without being an ancestor of anything. This module asks
//! git itself, and surfaces stdout, stderr and the exit code for
//! every answer git cannot give (missing object, not a repository,
//! bad configuration).

use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

/// Failures raised while probing ancestry.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GitError {
    /// `git merge-base --is-ancestor` exited with a code other than
    /// `0` (ancestor) or `1` (not an ancestor). Git uses `128` for
    /// missing objects / not-a-repository and `129` for usage
    /// errors. Both output streams are captured so callers can
    /// journal a complete diagnostic.
    #[error("git command failed (exit={code}): stderr={stderr}, stdout={stdout}")]
    CommandFailed {
        stdout: String,
        stderr: String,
        code: i32,
    },
    /// The `git` binary could not be spawned at all.
    #[error("git command not found or not spawnable: {0}")]
    NotFound(String),
    /// The supplied path is not inside a git working tree.
    #[error("not a git repository: {}", .path.display())]
    NotARepository { path: PathBuf },
}

/// Return `Ok(true)` when `ancestor` is a (transitive) ancestor of
/// `descendant` inside `repo_root`, `Ok(false)` when it is not, and
/// `Err(GitError::CommandFailed)` for every exit code git uses to
/// report that it cannot answer the question (fail-closed).
pub fn is_git_ancestor(
    repo_root: &Path,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .map_err(|e| GitError::NotFound(e.to_string()))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        other => Err(GitError::CommandFailed {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: other.unwrap_or(-1),
        }),
    }
}
