//! Plan-level git baseline persistence.
//!
//! The review diff base for a plan should be anchored to the git HEAD at the
//! moment the plan started, not the HEAD of an arbitrary rerun. This module
//! provides a small helper to read/write that baseline to disk so it survives
//! `--reuse-worktree` and `--continue`.
//!
//! Baseline file layout:
//! - Worktree mode (plan scoped by worktree):
//!   `{workspace}/.ralph/agent/plan-baseline.sha`
//! - Non-worktree mode with an identifiable plan:
//!   `{workspace}/.ralph/agent/plan-baseline-{plan_id}.sha`
//! - Non-worktree mode without a plan identifier: not persisted.

use std::path::{Path, PathBuf};

use crate::git_ops::get_head_sha;

/// Derive a plan identifier from the prompt file or explicit plan path.
///
/// Mirrors the naming logic used for worktree prefixes so that baseline
/// files line up with the plan the user thinks they are running.
pub fn derive_plan_id(prompt_file: &str, plan_path: Option<&Path>) -> Option<String> {
    // Explicit --plan takes precedence.
    if let Some(plan) = plan_path {
        if let Some(stem) = plan
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.trim().is_empty())
        {
            return Some(stem.to_string());
        }
    }

    // Fallback: non-default prompt file stem.
    if prompt_file.is_empty() {
        return None;
    }
    let prompt_path = Path::new(prompt_file);
    prompt_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .filter(|stem| stem.to_ascii_lowercase() != "prompt")
        .map(|stem| stem.to_string())
}

/// Path to the persisted plan baseline file.
pub fn plan_baseline_path(workspace: &Path, plan_id: Option<&str>) -> PathBuf {
    let base = workspace.join(".ralph").join("agent");
    match plan_id {
        Some(id) => base.join(format!("plan-baseline-{id}.sha")),
        None => base.join("plan-baseline.sha"),
    }
}

/// Read a persisted plan baseline SHA if it exists and looks like a valid SHA.
pub fn read_plan_baseline(workspace: &Path, plan_id: Option<&str>) -> Option<String> {
    let path = plan_baseline_path(workspace, plan_id);
    let content = std::fs::read_to_string(&path).ok()?;
    let sha = content.trim();
    if is_valid_sha(sha) {
        Some(sha.to_string())
    } else {
        None
    }
}

/// Ensure the plan baseline file exists with the given SHA.
///
/// If the file already exists, it is **not** overwritten. This preserves the
/// original plan baseline across `--reuse-worktree` and `--continue`.
pub fn ensure_plan_baseline(
    workspace: &Path,
    plan_id: Option<&str>,
    sha: &str,
) -> std::io::Result<()> {
    if !is_valid_sha(sha) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid plan baseline SHA: {sha}"),
        ));
    }

    let path = plan_baseline_path(workspace, plan_id);
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, format!("{sha}\n"))
}

/// Convenience: record the current HEAD as the plan baseline if one does not
/// already exist.
pub fn ensure_plan_baseline_from_head(
    workspace: &Path,
    plan_id: Option<&str>,
) -> Result<(), PlanBaselineError> {
    if read_plan_baseline(workspace, plan_id).is_some() {
        return Ok(());
    }
    let sha = get_head_sha(workspace).map_err(PlanBaselineError::Git)?;
    ensure_plan_baseline(workspace, plan_id, &sha).map_err(PlanBaselineError::Io)
}

/// Force the current HEAD as the plan baseline, overwriting any existing file.
pub fn write_plan_baseline_from_head(
    workspace: &Path,
    plan_id: Option<&str>,
) -> Result<(), PlanBaselineError> {
    let sha = get_head_sha(workspace).map_err(PlanBaselineError::Git)?;
    let path = plan_baseline_path(workspace, plan_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, format!("{sha}\n")).map_err(PlanBaselineError::Io)
}

#[derive(Debug, thiserror::Error)]
pub enum PlanBaselineError {
    #[error("git error: {0}")]
    Git(#[from] crate::git_ops::GitOpsError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

fn is_valid_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn derive_plan_id_prefers_explicit_plan() {
        let plan = Path::new("docs/plans/2026-06-25-foo-plan.md");
        assert_eq!(
            derive_plan_id("PROMPT.md", Some(plan)),
            Some("2026-06-25-foo-plan".to_string())
        );
    }

    #[test]
    fn derive_plan_id_falls_back_to_prompt_file() {
        assert_eq!(
            derive_plan_id("docs/plans/bar-plan.md", None),
            Some("bar-plan".to_string())
        );
    }

    #[test]
    fn derive_plan_id_ignores_default_prompt() {
        assert_eq!(derive_plan_id("PROMPT.md", None), None);
    }

    #[test]
    fn read_plan_baseline_filters_invalid_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = plan_baseline_path(tmp.path(), None);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "not-a-sha").unwrap();
        assert!(read_plan_baseline(tmp.path(), None).is_none());
    }

    #[test]
    fn ensure_plan_baseline_does_not_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let sha_a = "a".repeat(40);
        let sha_b = "b".repeat(40);
        ensure_plan_baseline(tmp.path(), None, &sha_a).unwrap();
        ensure_plan_baseline(tmp.path(), None, &sha_b).unwrap();
        assert_eq!(read_plan_baseline(tmp.path(), None), Some(sha_a));
    }
}
