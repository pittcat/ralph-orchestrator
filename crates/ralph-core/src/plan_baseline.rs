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
//! - Non-worktree mode with an identifiable plan/prompt source:
//!   `{workspace}/.ralph/agent/plan-baseline-{key}.sha`
//!   The key is `{parent_dir}-{stem}` for explicit plan/prompt paths, or a
//!   content hash for inline prompt text / the default `PROMPT.md`. This
//!   prevents unrelated plans that happen to share a filename stem from
//!   sharing a baseline.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::git_ops::get_head_sha;

/// Derive a stable baseline key from the prompt source.
///
/// Priority:
/// 1. Explicit `--plan` path.
/// 2. Non-default prompt file path.
/// 3. Inline prompt text (`-p/--prompt`).
/// 4. Default prompt file content (read from `workspace` if relative).
///
/// The returned key is safe to use in a filename. Returns `None` only when no
/// plan/prompt source can be identified.
pub fn derive_baseline_key(
    prompt_file: &str,
    plan_path: Option<&Path>,
    prompt_text: Option<&str>,
    workspace: Option<&Path>,
) -> Option<String> {
    if let Some(id) = derive_plan_id(prompt_file, plan_path) {
        return Some(id);
    }
    if let Some(text) = prompt_text.filter(|t| !t.trim().is_empty()) {
        return Some(format!("prompt-{}", short_hash(text.as_bytes())));
    }
    if prompt_file.is_empty() {
        return None;
    }
    let resolved = workspace
        .map(|w| w.join(prompt_file))
        .unwrap_or_else(|| PathBuf::from(prompt_file));
    if let Ok(content) = std::fs::read_to_string(&resolved)
        && !content.trim().is_empty() {
            return Some(format!("prompt-{}", short_hash(content.as_bytes())));
        }
    None
}

/// Derive a plan identifier from the prompt file or explicit plan path.
///
/// The identifier is `{parent_dir}-{stem}` so that plans in different
/// directories with the same stem do not collide. `PROMPT.md` (the default
/// prompt file) intentionally returns `None` so that the content-addressed
/// fallback in [`derive_baseline_key`] can be used.
pub fn derive_plan_id(prompt_file: &str, plan_path: Option<&Path>) -> Option<String> {
    // Explicit --plan takes precedence.
    if let Some(plan) = plan_path {
        return path_based_plan_id(plan);
    }

    // Fallback: non-default prompt file path.
    if prompt_file.is_empty() {
        return None;
    }
    let prompt_path = Path::new(prompt_file);
    let is_default_prompt = prompt_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        == Some("prompt".to_string());
    if is_default_prompt {
        return None;
    }
    path_based_plan_id(prompt_path)
}

fn path_based_plan_id(path: &Path) -> Option<String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.trim().is_empty())?;
    let parent = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty());
    match parent {
        Some(parent) => Some(format!("{parent}-{stem}")),
        None => Some(stem.to_string()),
    }
}

fn short_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())[..16].to_string()
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
/// original plan baseline across `--reuse-worktree` and `--continue`. The
/// creation is atomic (`O_EXCL`) so concurrent loop starts cannot accidentally
/// overwrite an existing baseline.
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            file.write_all(format!("{sha}\n").as_bytes())?;
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
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
        let id = derive_plan_id("PROMPT.md", Some(plan)).unwrap();
        assert_eq!(id, "plans-2026-06-25-foo-plan");
    }

    #[test]
    fn derive_plan_id_falls_back_to_prompt_file() {
        let id = derive_plan_id("docs/plans/bar-plan.md", None).unwrap();
        assert_eq!(id, "plans-bar-plan");
    }

    #[test]
    fn derive_plan_id_ignores_default_prompt() {
        assert_eq!(derive_plan_id("PROMPT.md", None), None);
    }

    #[test]
    fn derive_baseline_key_uses_content_hash_for_default_prompt_file() {
        let tmp = tempfile::tempdir().unwrap();
        let prompt = tmp.path().join("PROMPT.md");
        std::fs::write(&prompt, "implement the feature").unwrap();
        let id = derive_baseline_key(prompt.to_str().unwrap(), None, None, None).unwrap();
        assert!(id.starts_with("prompt-"), "expected prompt hash, got: {id}");
        assert_eq!(id.len(), "prompt-".len() + 16);
    }

    #[test]
    fn derive_baseline_key_uses_text_hash_when_prompt_file_empty() {
        let id = derive_baseline_key("", None, Some("fix the bug"), None).unwrap();
        assert!(id.starts_with("prompt-"), "expected prompt hash, got: {id}");
    }

    #[test]
    fn derive_baseline_key_resolves_relative_prompt_file_against_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let prompt = tmp.path().join("PROMPT.md");
        std::fs::write(&prompt, "relative prompt content").unwrap();
        let id = derive_baseline_key("PROMPT.md", None, None, Some(tmp.path())).unwrap();
        assert!(id.starts_with("prompt-"), "expected prompt hash, got: {id}");
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
