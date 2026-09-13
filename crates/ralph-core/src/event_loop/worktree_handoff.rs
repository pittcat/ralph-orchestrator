//! Activation-scoped worktree snapshots used by handoff guards and audits.

use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

/// SHA-256 of a file's bytes at the moment the snapshot was taken.
/// Computed by `WorkspaceMutationGuard::sha256_hex`; the field is
/// typed as the lowercase hex string so two snapshots stay
/// comparable with no need for byte-vs-hex ambiguity.
pub(crate) type FileContentHash = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeSnapshot {
    pub(crate) head_sha: String,
    pub(crate) dirty_fingerprint: u64,
    pub(crate) dirty_paths: Vec<String>,
    /// 2026-09-13-001-fix-forge-dag-artifact-handoff-plan U2 (C1+M3):
    /// per-path SHA-256 hex digest of the dirty foreign file at the
    /// moment the snapshot was taken. `dirty_paths_content_equal`
    /// compares this between `before` and `current` so a content
    /// edit mid-handoff is detected even though both snapshots
    /// share the same path set. An absent entry means the path was
    /// not a regular file at capture time (directory / symlink /
    /// unreadable); the comparison helper treats those as
    /// non-comparable and returns `Err` so the caller can decide.
    pub(crate) content_hashes: BTreeMap<PathBuf, FileContentHash>,
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
        let mut content_hashes: BTreeMap<PathBuf, FileContentHash> = BTreeMap::new();
        for entry in &entries {
            let path = status_entry_path(entry);
            path.hash(&mut hasher);
            let full_path = workspace.join(path);
            // 2026-09-13-001-fix-forge-dag-artifact-handoff-plan U2
            // (C1+M3): capture the SHA-256 of the file's bytes at
            // snapshot time. `dirty_paths_content_equal` uses these
            // to detect content edits between `before` and
            // `current` without having to re-read the live disk
            // twice (the previous impl double-read the same on-disk
            // state, so any mid-handoff edit was invisible).
            match hash_worktree_path(workspace, path, &mut hasher) {
                HashOutcome::File => {
                    if let Ok(bytes) = std::fs::read(&full_path) {
                        // Key by the relative `path` (the same
                        // shape `dirty_paths` carries) so the
                        // comparison helper can look up the digest
                        // directly from the relative entry rather
                        // than re-joining against the workspace.
                        content_hashes.insert(
                            PathBuf::from(path),
                            crate::workspace_mutation_guard::sha256_hex(&bytes),
                        );
                    }
                }
                HashOutcome::NonFile | HashOutcome::Missing => {
                    // Non-regular files (directories, symlinks) and
                    // entries that disappeared between `git status`
                    // and our `read` are intentionally absent from
                    // the map. `dirty_paths_content_equal` treats
                    // those as non-comparable.
                }
                HashOutcome::IoError(err) => return Err(err),
            }
        }
        Ok(Self {
            head_sha,
            dirty_fingerprint: hasher.finish(),
            dirty_paths: entries,
            content_hashes,
        })
    }

    pub(crate) fn changed_since(&self, before: &Self) -> bool {
        self.head_sha != before.head_sha || self.dirty_fingerprint != before.dirty_fingerprint
    }
}

/// Outcome of `hash_worktree_path`: distinguishes a captured file
/// (whose bytes we want to also record in `content_hashes`) from
/// non-files and IO errors so `capture` can branch without a
/// second stat.
enum HashOutcome {
    File,
    NonFile,
    Missing,
    IoError(std::io::Error),
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
) -> HashOutcome {
    let full_path = workspace.join(path);
    match std::fs::metadata(&full_path) {
        Ok(metadata) if metadata.is_file() => {
            b"file".hash(hasher);
            match std::fs::read(full_path) {
                Ok(bytes) => {
                    bytes.hash(hasher);
                    HashOutcome::File
                }
                Err(error) => HashOutcome::IoError(error),
            }
        }
        Ok(metadata) => {
            b"non-file".hash(hasher);
            metadata.file_type().is_dir().hash(hasher);
            HashOutcome::NonFile
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            b"missing".hash(hasher);
            HashOutcome::Missing
        }
        Err(error) => HashOutcome::IoError(error),
    }
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

/// Compares the dirty foreign paths of two snapshots by reading
/// the SHA-256 hex digests captured at snapshot time (NOT by
/// re-reading the live disk twice).  The previous implementation
/// silently double-read the same on-disk state through an unused
/// `_current` parameter, so a content edit between `before` and
/// `current` was invisible (C1+M3 in the
/// 2026-09-13-001-fix-forge-dag-artifact-handoff-plan review).
///
/// `before` and `current` carry independent `content_hashes`
/// captured at the moments their respective snapshots were taken,
/// so we can detect a mid-handoff edit even if both calls happen
/// to read the same current disk state after the edit is gone.
///
/// Returns `Ok(true)` only when every path that exists in both
/// snapshots has a matching digest; `Ok(false)` if any path
/// disagrees (the prior `_current`-unused code wrongly returned
/// `Ok(true)` in that case).  An absent entry in either side
/// (non-file / missing / unreadable) is treated as a non-comparable
/// path and surfaces as `Err` so the caller can decide whether to
/// bail; a path only present in `current` is a `dirty → dirty-with-
/// new-path` change and also surfaces as `Err` since the path-set
/// guard has already matched by the time we run.
fn dirty_paths_content_equal(
    workspace: &Path,
    before: &WorktreeSnapshot,
    current: &WorktreeSnapshot,
) -> Result<bool, String> {
    for raw_entry in &before.dirty_paths {
        // `dirty_paths` carries the raw `git status` entries (e.g.
        // ` M foo.txt`), but `content_hashes` is keyed by the
        // path extracted by `status_entry_path`. Apply the same
        // extraction here so the lookup matches.
        let path = status_entry_path(raw_entry);
        let before_digest = before.content_hashes.get(&PathBuf::from(path)).ok_or_else(|| {
            format!(
                "baseline dirty path {path} has no captured content hash \
                 (non-regular file at capture time; cannot compare)"
            )
        })?;
        let current_digest = current
            .content_hashes
            .get(&PathBuf::from(path))
            .ok_or_else(|| {
                format!(
                    "current dirty path {path} has no captured content hash \
                     (non-regular file at capture time; cannot compare)"
                )
            })?;
        if before_digest != current_digest {
            return Ok(false);
        }
    }
    // `before.dirty_paths` already covered every shared entry (the
    // call site guarantees `before.dirty_paths == current.dirty_paths`),
    // so reaching this line means every path matched. The previous
    // implementation re-walked `current.dirty_paths` for the same
    // lookup, which double-read the same on-disk state and silently
    // accepted mid-handoff edits; we removed that walk.
    let _ = workspace;
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
        WorktreeSnapshot, dirty_paths_content_equal, is_ralph_path,
        validate_stabilization_handoff, validate_work_done_handoff,
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

    /// Regression for plan 2026-09-13-001 U2 / C1+M3: the previous
    /// `dirty_paths_content_equal` double-read the same on-disk
    /// state via an unused `_current` parameter, so a content edit
    /// between `before` and `current` was invisible — the function
    /// returned `Ok(true)` even though the bytes actually changed.
    /// After the fix, `before` and `current` carry independent
    /// `content_hashes` captured at their respective snapshot
    /// moments, so the comparison surfaces a mid-handoff edit.
    #[test]
    fn dirty_paths_content_equal_detects_content_edit_between_snapshots() {
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
        std::fs::write(temp.path().join("tracked.txt"), "baseline\n").expect("write baseline");
        git(&["add", "tracked.txt"]);
        git(&["commit", "--quiet", "-m", "init"]);

        // Pre-existing dirty foreign file at activation.
        std::fs::write(temp.path().join("tracked.txt"), "v1\n").expect("pre-dirty");
        let before = WorktreeSnapshot::capture(temp.path()).expect("capture before");

        // Edit the dirty file mid-handoff (this is the bug the old
        // implementation silently accepted).
        std::fs::write(temp.path().join("tracked.txt"), "v2\n").expect("mid-edit");
        let current = WorktreeSnapshot::capture(temp.path()).expect("capture current");

        // Same path set on both sides — the helper MUST see the
        // content drift and return Ok(false). The previous
        // implementation returned Ok(true) here (the bug).
        assert!(
            before.dirty_paths == current.dirty_paths,
            "preconditions: path sets must match"
        );
        let equal = dirty_paths_content_equal(temp.path(), &before, &current)
            .expect("comparison must run cleanly");
        assert!(
            !equal,
            "dirty→dirty-with-content-edit must NOT be reported as equal"
        );
    }

    /// Negative control: same dirty file, no edit between
    /// snapshots → the helper MUST return Ok(true) (legitimate
    /// "dirt was untouched" hand-off).
    #[test]
    fn dirty_paths_content_equal_returns_true_when_content_unchanged() {
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
        std::fs::write(temp.path().join("tracked.txt"), "baseline\n").expect("write baseline");
        git(&["add", "tracked.txt"]);
        git(&["commit", "--quiet", "-m", "init"]);

        std::fs::write(temp.path().join("tracked.txt"), "stable\n").expect("pre-dirty");
        let before = WorktreeSnapshot::capture(temp.path()).expect("capture before");
        // No edit between snapshots.
        let current = WorktreeSnapshot::capture(temp.path()).expect("capture current");

        assert!(
            dirty_paths_content_equal(temp.path(), &before, &current)
                .expect("comparison must run cleanly"),
            "dirty→dirty-without-edit must report equal"
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
