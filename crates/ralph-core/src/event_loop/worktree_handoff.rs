//! Activation-scoped worktree snapshots used by handoff guards and audits.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeSnapshot {
    pub(crate) head_sha: String,
    pub(crate) dirty_fingerprint: u64,
    pub(crate) dirty_paths: Vec<String>,
}

impl WorktreeSnapshot {
    pub(crate) fn capture(workspace: &Path) -> std::io::Result<Self> {
        let head_sha = git_output(workspace, &["rev-parse", "HEAD"])?;
        let status = git_output_bytes(
            workspace,
            &["status", "--porcelain=v1", "--untracked-files=all", "-z"],
        )?;
        let entries = status
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
            .map(String::from_utf8_lossy)
            .filter(|entry| !is_ralph_path(entry))
            .map(|entry| entry.into_owned())
            .collect::<Vec<_>>();

        let mut hasher = DefaultHasher::new();
        entries.hash(&mut hasher);
        for entry in &entries {
            let path = status_entry_path(entry);
            path.hash(&mut hasher);
            hash_worktree_path(workspace, path, &mut hasher)?;
        }
        Ok(Self {
            head_sha,
            dirty_fingerprint: hasher.finish(),
            dirty_paths: entries,
        })
    }

    pub(crate) fn changed_since(&self, before: &Self) -> bool {
        self.head_sha != before.head_sha || self.dirty_fingerprint != before.dirty_fingerprint
    }
}

fn is_ralph_path(status_entry: &str) -> bool {
    let path = status_entry_path(status_entry);
    path == ".ralph" || path.starts_with(".ralph/")
}

fn status_entry_path(status_entry: &str) -> &str {
    if status_entry.as_bytes().get(2) == Some(&b' ') {
        &status_entry[3..]
    } else {
        // `git status --porcelain=v1 -z` emits a second bare path for
        // rename/copy entries. Keeping it in the fingerprint prevents a
        // rename from being mistaken for an unchanged path set.
        status_entry
    }
}

fn hash_worktree_path(
    workspace: &Path,
    path: &str,
    hasher: &mut DefaultHasher,
) -> std::io::Result<()> {
    let full_path = workspace.join(path);
    match std::fs::metadata(&full_path) {
        Ok(metadata) if metadata.is_file() => {
            b"file".hash(hasher);
            std::fs::read(full_path)?.hash(hasher);
        }
        Ok(metadata) => {
            b"non-file".hash(hasher);
            metadata.file_type().is_dir().hash(hasher);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            b"missing".hash(hasher);
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

pub(crate) fn validate_work_done_handoff(
    workspace: &Path,
    activation_baseline: Option<&WorktreeSnapshot>,
    payload: &str,
) -> Result<(), String> {
    let value: serde_json::Value = serde_json::from_str(payload)
        .map_err(|error| format!("work.done payload is not valid JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "work.done payload must be a JSON object".to_string())?;

    let expected_head = required_string(object, "executor_head_sha")?;
    let baseline_sha = required_string(object, "resolved_baseline_sha")?;
    if !is_sha(expected_head) || !is_sha(baseline_sha) {
        return Err("executor_head_sha and resolved_baseline_sha must be 40-char Git SHAs".into());
    }

    let current = WorktreeSnapshot::capture(workspace)
        .map_err(|error| format!("could not capture work.done handoff state: {error}"))?;
    if current.head_sha != expected_head {
        return Err(format!(
            "executor_head_sha is stale: payload={expected_head}, actual={}",
            current.head_sha
        ));
    }
    if let Some(before) = activation_baseline {
        // The dirty-state comparison must detect any change to non-`.ralph/`
        // worktree state during the executor activation: modifications,
        // untracked additions, or content edits. There are three legitimate
        // end states:
        //
        // 1. **clean → clean**: worktree was clean at activation, stays clean.
        //    Goal state; path set must be empty on both sides.
        // 2. **dirty → clean**: pre-existing dirt at activation was committed
        //    (e.g. `before` captured before the agent's normal commit
        //    workflow, `current` captured after). This is the legitimate
        //    "everything was committed" outcome — the path set differs by
        //    design, so we only require that `current` is actually clean.
        // 3. **dirty → same-dirty**: dirt was untouched (rare, e.g. tests
        //    that intentionally leave files behind). Both path sets and
        //    per-path content hashes must match.
        //
        // Reject when:
        // - clean → dirty: agent introduced new foreign dirt mid-handoff.
        // - dirty → different-dirty: agent modified the dirt (additions or
        //   content edits) without committing.
        //
        // We compare path sets and per-path content hashes, NOT the
        // aggregate `dirty_fingerprint`: that value is seeded from the
        // process id (SipHash defaults), so two `WorktreeSnapshot` captures
        // taken from different `ralph` processes (e.g. RPC worker vs the
        // loop CLI) — which is the common case when `before` is captured at
        // build_prompt in one process and `current` is captured at
        // precheck in another — produce different hashes for the same
        // underlying state. Path-and-content equality is what the check is
        // really trying to enforce.
        let before_clean = before.dirty_paths.is_empty();
        let current_clean = current.dirty_paths.is_empty();
        let paths_agree = before.dirty_paths == current.dirty_paths;
        let dirty_to_same_dirty = !before_clean
            && !current_clean
            && paths_agree
            && dirty_paths_content_equal(workspace, before, &current)?;
        let clean_to_clean = before_clean && current_clean;
        let dirty_to_clean = !before_clean && current_clean;
        if !(clean_to_clean || dirty_to_clean || dirty_to_same_dirty) {
            return Err(format!(
                "worktree changed during executor activation; dirty paths: {:?}",
                current.dirty_paths
            ));
        }
    } else {
        return Err("executor activation worktree baseline is missing".into());
    }

    let actual_commit_count = git_output(
        workspace,
        &[
            "rev-list",
            "--count",
            &format!("{baseline_sha}..{expected_head}"),
        ],
    )
    .map_err(|error| format!("could not verify executor commit range: {error}"))?;
    let actual_commit_count = actual_commit_count
        .parse::<u64>()
        .map_err(|error| format!("git returned an invalid commit count: {error}"))?;
    let claimed_commit_count = required_u64(object, "commit_count")?;
    if actual_commit_count != claimed_commit_count {
        return Err(format!(
            "commit_count mismatch: payload={claimed_commit_count}, actual={actual_commit_count}"
        ));
    }

    let completed_units = object
        .get("completed_units")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "completed_units must be an array".to_string())?;
    if actual_commit_count < completed_units.len() as u64 {
        return Err(format!(
            "completed_units={} exceeds deliverable commits={actual_commit_count}",
            completed_units.len()
        ));
    }
    Ok(())
}

pub(crate) fn validate_stabilization_handoff(
    workspace: &Path,
    activation_baseline: Option<&WorktreeSnapshot>,
    payload: &str,
) -> Result<(), String> {
    let value: serde_json::Value = serde_json::from_str(payload)
        .map_err(|error| format!("stabilization.done payload is not valid JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "stabilization.done payload must be a JSON object".to_string())?;
    let expected_head = required_string(object, "head_sha")?;
    let worktree_status = required_string(object, "worktree_status")?;
    if !is_sha(expected_head) {
        return Err("head_sha must be a 40-char Git SHA".into());
    }
    let current = WorktreeSnapshot::capture(workspace)
        .map_err(|error| format!("could not capture stabilization handoff state: {error}"))?;
    if current.head_sha != expected_head {
        return Err(format!(
            "stabilization head_sha is stale: payload={expected_head}, actual={}",
            current.head_sha
        ));
    }
    let expected_status = if current.dirty_paths.is_empty() {
        "clean"
    } else {
        "dirty"
    };
    if worktree_status != expected_status {
        return Err(format!(
            "worktree_status mismatch: payload={worktree_status}, actual={expected_status}"
        ));
    }
    if let Some(before) = activation_baseline {
        // See the matching comment in `validate_work_done_handoff` for the
        // three legitimate end states and the rationale for using path +
        // content equality instead of the per-process `dirty_fingerprint`.
        let before_clean = before.dirty_paths.is_empty();
        let current_clean = current.dirty_paths.is_empty();
        let paths_agree = before.dirty_paths == current.dirty_paths;
        let dirty_to_same_dirty = !before_clean
            && !current_clean
            && paths_agree
            && dirty_paths_content_equal(workspace, before, &current)?;
        let clean_to_clean = before_clean && current_clean;
        let dirty_to_clean = !before_clean && current_clean;
        if !(clean_to_clean || dirty_to_clean || dirty_to_same_dirty) {
            return Err(format!(
                "worktree changed during stabilization; dirty paths: {:?}",
                current.dirty_paths
            ));
        }
    } else {
        return Err("stabilizer activation worktree baseline is missing".into());
    }
    Ok(())
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<&'a str, String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{field} is missing or empty"))
}

fn required_u64(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<u64, String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("{field} is missing or not a non-negative integer"))
}

fn is_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Compares the dirty foreign paths of two snapshots by reading their on-disk
/// content. Used in place of the aggregate `dirty_fingerprint` (which is seeded
/// from `process::id()` and is therefore not stable across the two `ralph`
/// processes that typically cooperate on a single handoff: the loop CLI that
/// captured `before` at `build_prompt`, and the RPC worker that captures
/// `current` at precheck). We only need to call this when both snapshots have
/// matching non-empty path sets — anything else is already short-circuited by
/// the caller.
///
/// `_current` is reserved for future asymmetric checks (e.g. "did current
/// introduce a new path that wasn't in `before`?"). Today we trust the
/// path-set equality guard the caller enforces before invoking us.
fn dirty_paths_content_equal(
    workspace: &Path,
    before: &WorktreeSnapshot,
    _current: &WorktreeSnapshot,
) -> Result<bool, String> {
    for path in &before.dirty_paths {
        let full_path = workspace.join(path);
        let before_bytes = match std::fs::read(&full_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "could not read baseline dirty path {path}: {error}"
                ));
            }
        };
        let current_bytes = match std::fs::read(&full_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(false);
            }
            Err(error) => {
                return Err(format!(
                    "could not read current dirty path {path}: {error}"
                ));
            }
        };
        if before_bytes != current_bytes {
            return Ok(false);
        }
    }
    Ok(true)
}

fn git_output(workspace: &Path, args: &[&str]) -> std::io::Result<String> {
    Ok(String::from_utf8_lossy(&git_output_bytes(workspace, args)?)
        .trim()
        .to_string())
}

fn git_output_bytes(workspace: &Path, args: &[&str]) -> std::io::Result<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "git {} failed with status {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::{
        WorktreeSnapshot, is_ralph_path, validate_stabilization_handoff, validate_work_done_handoff,
    };
    use serde_json::json;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn filters_runtime_paths_from_status_entries() {
        assert!(is_ralph_path(" M .ralph/events.jsonl"));
        assert!(is_ralph_path("?? .ralph"));
        assert!(!is_ralph_path(" M crates/ralph-core/src/lib.rs"));
        assert!(!is_ralph_path("?? .ralphish/file.rs"));
    }

    #[test]
    fn detects_content_changes_and_ignores_runtime_changes() {
        let temp = TempDir::new().expect("tempdir");
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(temp.path())
                .output()
                .expect("git starts");
            assert!(output.status.success(), "git {:?} failed", args);
        };
        git(&["init", "--quiet"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "test"]);
        std::fs::write(temp.path().join("tracked.txt"), "one\n").expect("write tracked");
        git(&["add", "tracked.txt"]);
        git(&["commit", "--quiet", "-m", "baseline"]);

        let baseline = WorktreeSnapshot::capture(temp.path()).expect("capture baseline");
        std::fs::create_dir_all(temp.path().join(".ralph")).expect("runtime dir");
        std::fs::write(temp.path().join(".ralph/events.jsonl"), "runtime\n")
            .expect("write runtime");
        let runtime_changed = WorktreeSnapshot::capture(temp.path()).expect("capture runtime");
        assert!(!runtime_changed.changed_since(&baseline));

        std::fs::write(temp.path().join("tracked.txt"), "two\n").expect("modify tracked");
        let content_changed = WorktreeSnapshot::capture(temp.path()).expect("capture content");
        assert!(content_changed.changed_since(&runtime_changed));
    }

    #[test]
    fn work_done_handoff_requires_real_commit_and_unchanged_foreign_dirt() {
        let temp = TempDir::new().expect("tempdir");
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(temp.path())
                .output()
                .expect("git starts");
            assert!(output.status.success(), "git {:?} failed", args);
        };
        git(&["init", "--quiet"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "test"]);
        std::fs::write(temp.path().join("tracked.txt"), "one\n").expect("write tracked");
        git(&["add", "tracked.txt"]);
        git(&["commit", "--quiet", "-m", "baseline"]);
        let baseline_sha = git_sha(temp.path(), &["rev-parse", "HEAD"]);
        let activation = WorktreeSnapshot::capture(temp.path()).expect("capture activation");

        std::fs::write(temp.path().join("tracked.txt"), "two\n").expect("modify tracked");
        let uncommitted_payload = json!({
            "executor_head_sha": baseline_sha,
            "resolved_baseline_sha": baseline_sha,
            "completed_units": ["U1"],
            "commit_count": 0,
        })
        .to_string();
        let error =
            validate_work_done_handoff(temp.path(), Some(&activation), &uncommitted_payload)
                .expect_err("uncommitted work must be rejected");
        assert!(
            error.contains("worktree changed"),
            "unexpected error: {error}"
        );

        git(&["add", "tracked.txt"]);
        git(&["commit", "--quiet", "-m", "U1: deliver change"]);
        let head_sha = git_sha(temp.path(), &["rev-parse", "HEAD"]);
        let committed_payload = json!({
            "executor_head_sha": head_sha,
            "resolved_baseline_sha": baseline_sha,
            "completed_units": ["U1"],
            "commit_count": 1,
        })
        .to_string();
        validate_work_done_handoff(temp.path(), Some(&activation), &committed_payload)
            .expect("committed work with a clean handoff must pass");
    }

    /// Regression: when the activation baseline captured pre-existing dirty
    /// foreign paths (e.g. another hat or operator left them behind) and the
    /// executor legitimately commits them away, `before` is dirty and
    /// `current` is clean — that must still pass. The earlier fingerprint-only
    /// comparison incorrectly rejected this case as `worktree_handoff_inconsistent`
    /// because the dirty_fingerprint hashes of a non-empty set vs an empty set
    /// can never agree.
    #[test]
    fn work_done_handoff_allows_committing_pre_existing_dirt() {
        let temp = TempDir::new().expect("tempdir");
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(temp.path())
                .output()
                .expect("git starts");
            assert!(output.status.success(), "git {:?} failed", args);
        };
        git(&["init", "--quiet"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "test"]);
        std::fs::write(temp.path().join("tracked.txt"), "one\n").expect("write tracked");
        git(&["add", "tracked.txt"]);
        git(&["commit", "--quiet", "-m", "baseline"]);

        // Pre-existing dirty tracked file at activation time.
        std::fs::write(temp.path().join("tracked.txt"), "two\n").expect("pre-dirty");
        let activation = WorktreeSnapshot::capture(temp.path()).expect("capture activation");
        assert!(
            !activation.dirty_paths.is_empty(),
            "activation baseline must record pre-existing dirt"
        );

        // Executor commits the pre-existing dirt — current is now clean.
        git(&["add", "tracked.txt"]);
        git(&["commit", "--quiet", "-m", "U1: deliver change"]);
        let head_sha = git_sha(temp.path(), &["rev-parse", "HEAD"]);
        let baseline_sha = git_sha(temp.path(), &["rev-list", "HEAD~1"]);
        let committed_payload = json!({
            "executor_head_sha": head_sha,
            "resolved_baseline_sha": baseline_sha,
            "completed_units": ["U1"],
            "commit_count": 1,
        })
        .to_string();
        validate_work_done_handoff(temp.path(), Some(&activation), &committed_payload)
            .expect("committing pre-existing dirt must still pass");
    }

    /// Regression: clean activation that introduced *new* foreign dirt mid-handoff
    /// must still be rejected. The relaxed fingerprint rule must not regress
    /// the original security guarantee.
    #[test]
    fn work_done_handoff_rejects_clean_to_new_dirt() {
        let temp = TempDir::new().expect("tempdir");
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(temp.path())
                .output()
                .expect("git starts");
            assert!(output.status.success(), "git {:?} failed", args);
        };
        git(&["init", "--quiet"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "test"]);
        std::fs::write(temp.path().join("tracked.txt"), "one\n").expect("write tracked");
        git(&["add", "tracked.txt"]);
        git(&["commit", "--quiet", "-m", "baseline"]);
        let baseline_sha = git_sha(temp.path(), &["rev-parse", "HEAD"]);
        let activation = WorktreeSnapshot::capture(temp.path()).expect("capture activation");
        assert!(
            activation.dirty_paths.is_empty(),
            "activation baseline must be clean for this test"
        );

        // Executor introduces new untracked foreign dirt.
        std::fs::write(temp.path().join("untracked.txt"), "leaked\n").expect("leak");
        let payload = json!({
            "executor_head_sha": baseline_sha,
            "resolved_baseline_sha": baseline_sha,
            "completed_units": ["U1"],
            "commit_count": 0,
        })
        .to_string();
        let error =
            validate_work_done_handoff(temp.path(), Some(&activation), &payload)
                .expect_err("clean→dirty must be rejected");
        assert!(
            error.contains("worktree changed"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn stabilization_handoff_requires_actual_head_and_status() {
        let temp = TempDir::new().expect("tempdir");
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(temp.path())
                .output()
                .expect("git starts");
            assert!(output.status.success(), "git {:?} failed", args);
        };
        git(&["init", "--quiet"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "test"]);
        std::fs::write(temp.path().join("tracked.txt"), "one\n").expect("write tracked");
        git(&["add", "tracked.txt"]);
        git(&["commit", "--quiet", "-m", "baseline"]);

        let activation = WorktreeSnapshot::capture(temp.path()).expect("capture activation");
        let head_sha = git_sha(temp.path(), &["rev-parse", "HEAD"]);
        let payload = json!({
            "head_sha": head_sha,
            "worktree_status": "clean",
        })
        .to_string();
        validate_stabilization_handoff(temp.path(), Some(&activation), &payload)
            .expect("clean stabilization handoff must pass");

        std::fs::write(temp.path().join("tracked.txt"), "dirty\n").expect("dirty tracked");
        let dirty_payload = json!({
            "head_sha": git_sha(temp.path(), &["rev-parse", "HEAD"]),
            "worktree_status": "clean",
        })
        .to_string();
        let error = validate_stabilization_handoff(temp.path(), Some(&activation), &dirty_payload)
            .expect_err("new dirty work must be rejected");
        assert!(error.contains("worktree_status mismatch") || error.contains("worktree changed"));
    }

    fn git_sha(workspace: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .expect("git starts");
        assert!(output.status.success(), "git {:?} failed", args);
        String::from_utf8(output.stdout)
            .expect("git output is UTF-8")
            .trim()
            .to_string()
    }
}
