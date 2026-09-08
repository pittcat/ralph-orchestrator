//! Read-only recovery of a unit worktree at a persisted stage boundary.

use super::{
    Command, Path, PathBuf, UnitWorktree, UnitWorktreeError, UnitWorktreeResult,
    validate_commit_oid, validate_component_id,
};

fn inspect_error(path: &Path, reason: impl Into<String>) -> UnitWorktreeError {
    UnitWorktreeError::InspectFailed {
        path: path.display().to_string(),
        reason: reason.into(),
    }
}

fn git(path: &Path, args: &[&str]) -> UnitWorktreeResult<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(path).args(args);
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    cmd.env("GIT_NO_REPLACE_OBJECTS", "1");
    for name in [
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        cmd.env_remove(name);
    }
    let output = cmd
        .output()
        .map_err(|e| inspect_error(path, e.to_string()))?;
    if !output.status.success() {
        return Err(inspect_error(
            path,
            format!("git {} failed", args.first().unwrap_or(&"inspect")),
        ));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_string())
        .map_err(|_| inspect_error(path, "non-UTF8 git identity"))
}

fn canonical(path: &Path) -> UnitWorktreeResult<PathBuf> {
    path.canonicalize()
        .map_err(|e| inspect_error(path, e.to_string()))
}

impl UnitWorktree {
    /// Restore an existing, clean unit worktree. `expected_commit` comes from
    /// the durable result accepted for the preceding stage, never from a
    /// worker's unvalidated path claim. No branch, file, or index is changed.
    ///
    /// The caller must first fence unresolved processes using the launch
    /// journal. An unknown process must block recovery before this method.
    pub fn resume(
        repo_root: &Path,
        loop_id: &str,
        unit_id: &str,
        verified_base_commit: &str,
        expected_commit: &str,
    ) -> UnitWorktreeResult<Self> {
        validate_component_id("loop_id", loop_id)?;
        validate_component_id("unit_id", unit_id)?;
        validate_commit_oid("verified_base_commit", verified_base_commit)?;
        validate_commit_oid("expected_commit", expected_commit)?;
        let repo = canonical(repo_root)?;
        let mut path = repo.clone();
        for component in [
            ".ralph".to_string(),
            "worktrees".to_string(),
            format!("{loop_id}-{unit_id}"),
        ] {
            path.push(component);
            let meta = std::fs::symlink_metadata(&path)
                .map_err(|e| inspect_error(&path, e.to_string()))?;
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(inspect_error(
                    &path,
                    "worktree path must contain only real directories",
                ));
            }
        }
        let path = canonical(&path)?;
        let top = canonical(Path::new(&git(&path, &["rev-parse", "--show-toplevel"])?))?;
        if top != path {
            return Err(inspect_error(&path, "path is not a worktree root"));
        }
        let common = canonical(Path::new(&git(
            &path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?))?;
        let host_common = canonical(Path::new(&git(
            &repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?))?;
        if common != host_common {
            return Err(inspect_error(
                &path,
                "worktree belongs to another repository",
            ));
        }
        let branch = format!("ralph/{loop_id}/{unit_id}");
        if git(&path, &["symbolic-ref", "--quiet", "HEAD"])? != format!("refs/heads/{branch}") {
            return Err(inspect_error(
                &path,
                "worktree branch does not match recorded unit",
            ));
        }
        let head = git(&path, &["rev-parse", "--verify", "HEAD^{commit}"])?;
        if !head.eq_ignore_ascii_case(expected_commit) {
            return Err(UnitWorktreeError::BaseMismatch {
                branch,
                tip: head,
                base: expected_commit.to_string(),
            });
        }
        git(
            &path,
            &[
                "merge-base",
                "--is-ancestor",
                verified_base_commit,
                expected_commit,
            ],
        )?;
        if !git(
            &path,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        )?
        .is_empty()
        {
            return Err(inspect_error(
                &path,
                "worktree has uncommitted or untracked changes",
            ));
        }
        Ok(Self {
            unit_id: unit_id.into(),
            loop_id: loop_id.into(),
            path,
            branch,
            base_commit: verified_base_commit.into(),
            reused: true,
        })
    }
}
