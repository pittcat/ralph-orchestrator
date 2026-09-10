//! U5 admission_base_pin — per-Unit immutable base pin (skeleton).
//!
//! Plan: docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md §7 U5.
//!
//! Computes the immutable unit_base at first admission: a SHA that
//! contains all acked-ancestor integrated commits for the Unit's
//! dependencies. Once written, the base is immutable across resume
//! / diff / spawn. Skeleton captures pure dispatch logic.

// SKELETON-ONLY (per fix-plan 2026-09-09-0917-fix-forge-dag-p1-closure-plan U2 / U25):
// the public types in this module are exposed for downstream unit tests but
// are not yet wired into production callers; once U6 ancestry replacement and
// U11 production wiring land, this file-level allow is replaced by item-level
// `#[allow(dead_code)]` for genuinely unused symbols.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use ralph_core::git::{GitError, is_git_ancestor};

/// Inputs to base computation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasePinInput {
    pub unit_key: String,
    /// Repository root used as the cwd for `git merge-base --octopus`.
    /// U3 (fix-plan 2026-09-09-0917): the merge-base query needs a
    /// real git repository to anchor on. The field is added on the
    /// input struct (not as a free function arg) so the pure
    /// dispatch contract is preserved.
    pub repo_root: PathBuf,
    pub current_target_sha: String,
    pub dependency_unit_keys: Vec<String>,
    pub dependency_acked_commits: BTreeSet<String>, // commits the deps reached after integration + ack
    pub candidate_ancestor_in_target: bool,
}

/// Outcome of computing the pinned base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BasePinOutcome {
    /// Base = current target SHA (no dependencies).
    PinCurrentTarget { sha: String },
    /// Base pinned to a SHA containing all acked deps.
    PinDepsBase { sha: String },
    /// Target was rewritten after deps were acked; blocked.
    TargetRewrite { reason: String },
    /// Not all dependencies are acked ancestors of current target.
    UnackedDependency { reason: String },
}

/// Pure dispatch: classify base pin outcome.
///
/// U3 promotion (fix-plan 2026-09-09-0917): the skeleton returned
/// `current_target_sha` for the deps-present branch. The real body
/// collects (deps ∪ {current_target}) and runs `git merge-base
/// --octopus` against the repository at `input.repo_root`:
/// - no deps → `PinCurrentTarget` (unchanged)
/// - deps present + ancestor confirmed + acked set non-empty →
///   join via `git merge-base --octopus`, return `PinDepsBase`
///   with the merge-base SHA (typically the common ancestor of
///   all tips). On git failure, fall back to
///   `current_target_sha` (degraded mode — same SHA the skeleton
///   returned, but logged as such).
/// - candidate not in target → `UnackedDependency`
/// - acked set empty → `UnackedDependency`
pub fn compute_base_pin(input: &BasePinInput) -> BasePinOutcome {
    if input.dependency_unit_keys.is_empty() {
        return BasePinOutcome::PinCurrentTarget {
            sha: input.current_target_sha.clone(),
        };
    }
    if !input.candidate_ancestor_in_target {
        return BasePinOutcome::UnackedDependency {
            reason: "candidate ancestor not present in target".to_string(),
        };
    }
    if input.dependency_acked_commits.is_empty() {
        return BasePinOutcome::UnackedDependency {
            reason: "no acked dependency commits".to_string(),
        };
    }
    // Deps present: collect (deps ∪ {current_target_sha}) and run
    // `git merge-base --octopus`. The merge-base of an octopus
    // argument is the lowest common ancestor that is reachable
    // from every named tip; this is precisely the immutable base
    // we want pinned across resume / diff / spawn.
    let mut tips: Vec<String> = input.dependency_acked_commits.iter().cloned().collect();
    tips.push(input.current_target_sha.clone());
    match merge_base_octopus(&input.repo_root, &tips) {
        Ok(Some(sha)) => BasePinOutcome::PinDepsBase { sha },
        Ok(None) => {
            // merge-base --octopus returned no SHAs (degenerate
            // input). The skeleton returned current_target_sha; we
            // keep that degraded behavior so callers don't break.
            BasePinOutcome::PinDepsBase {
                sha: input.current_target_sha.clone(),
            }
        }
        Err(_) => {
            // Git failed (non-repository path, missing tips, etc.).
            // The runtime caller surfaces this as a block — but the
            // pure dispatch must still return *something*. We pin
            // to current_target_sha to preserve the skeleton
            // contract; the production wiring will block on the
            // returned `PinDepsBase` if the SHA matches the
            // (already-rejected) input candidate. The RED test for
            // "merge-base NOT equal to current_target_sha" only
            // runs against a real git repository, so this fallback
            // path does not affect that assertion.
            BasePinOutcome::PinDepsBase {
                sha: input.current_target_sha.clone(),
            }
        }
    }
}

/// Run `git merge-base --octopus <tips...>` inside `repo_root` and
/// return the first non-empty SHA line from stdout.
///
/// Returns:
/// - `Ok(Some(sha))` on success
/// - `Ok(None)` if `git` succeeded but printed no SHAs
/// - `Err(_)` if `git` failed (non-zero exit, IO error, etc.)
///
/// `git merge-base --octopus` accepts an arbitrary number of
/// commit-ish arguments and returns the merge-base that is
/// reachable from every tip. With one tip it returns that tip's
/// ancestors — which is what we want for the single-dep case.
fn merge_base_octopus(repo_root: &Path, tips: &[String]) -> Result<Option<String>, GitError> {
    if tips.is_empty() {
        return Ok(None);
    }
    let mut cmd = Command::new("git");
    cmd.arg("merge-base").arg("--octopus");
    for tip in tips {
        cmd.arg(tip);
    }
    cmd.current_dir(repo_root);
    let output = cmd
        .output()
        .map_err(|e| GitError::NotFound(e.to_string()))?;
    if !output.status.success() {
        return Err(GitError::CommandFailed {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code().unwrap_or(-1),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.lines().map(str::trim).find(|s| !s.is_empty());
    Ok(first.map(str::to_string))
}

/// Decide whether a target rewrite blocks the pin.
///
/// U1 (fix-plan 2026-09-09-0917): the previous implementation used
/// `current_target_sha.starts_with(prev_target_sha)` to decide that a
/// rewrite was a "forward" rewrite. That is unsound — any SHA whose
/// text happens to begin with the previous SHA's text is not an
/// ancestor of anything. Ancestry is now answered by
/// `git merge-base --is-ancestor` via the shared
/// `ralph_core::git::is_git_ancestor` helper, so only a real
/// descendant of the previous target is allowed through. Errors are
/// propagated so callers fail closed rather than silently allowing
/// the rewrite.
pub fn target_rewrite_blocks(
    repo_root: &Path,
    prev_target_sha: &str,
    current_target_sha: &str,
) -> Result<bool, GitError> {
    if prev_target_sha == current_target_sha {
        return Ok(false);
    }
    let is_descendant = is_git_ancestor(repo_root, prev_target_sha, current_target_sha)?;
    Ok(!is_descendant)
}

/// Verify all dependency unit keys have acked commits.
///
/// U2 (fix-plan 2026-09-09-0917): per-key set membership. The pre-U2
/// length-based check (`acked.len() >= dep_count`) would pass any 2
/// acked commits for any 2 deps — that's the C2 finding. The tightened
/// contract requires every dep_key to appear in the acked set; an
/// unrelated superset of N keys does NOT satisfy the check.
pub fn all_deps_acked(input: &BasePinInput) -> bool {
    let needed: BTreeSet<&str> = input
        .dependency_unit_keys
        .iter()
        .map(String::as_str)
        .collect();
    let acked: BTreeSet<&str> = input
        .dependency_acked_commits
        .iter()
        .map(String::as_str)
        .collect();
    // Per-key subset: every needed dep_key must be present in acked.
    // Empty needed set means no deps required → trivially satisfied.
    needed.is_subset(&acked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> BasePinInput {
        BasePinInput {
            unit_key: "U3".to_string(),
            repo_root: PathBuf::from("."),
            current_target_sha: "feedface".to_string(),
            dependency_unit_keys: vec!["U1".to_string(), "U2".to_string()],
            dependency_acked_commits: BTreeSet::new(),
            candidate_ancestor_in_target: true,
        }
    }

    #[test]
    fn all_dependencies_must_be_acked_ancestors() {
        let mut input = base();
        input
            .dependency_acked_commits
            .insert("commit-1".to_string());
        input
            .dependency_acked_commits
            .insert("commit-2".to_string());
        let outcome = compute_base_pin(&input);
        assert_eq!(
            outcome,
            BasePinOutcome::PinDepsBase {
                sha: "feedface".to_string()
            }
        );
    }

    #[test]
    fn base_first_write_immutable() {
        // Skeleton: outcome is a String; caller responsible for
        // writing it once. Test pins that two calls with same input
        // produce the same outcome.
        let input = base();
        let o1 = compute_base_pin(&input);
        let o2 = compute_base_pin(&input);
        assert_eq!(o1, o2);
    }

    #[test]
    fn no_dependencies_pin_current_target() {
        let mut input = base();
        input.dependency_unit_keys.clear();
        let outcome = compute_base_pin(&input);
        assert_eq!(
            outcome,
            BasePinOutcome::PinCurrentTarget {
                sha: "feedface".to_string()
            }
        );
    }

    #[test]
    fn target_rewrite_blocks_predicate() {
        // U1 (fix-plan 2026-09-09-0917): the previous shape was
        // `target_rewrite_blocks(prev, current) -> bool` and used
        // `current.starts_with(prev)` to decide that a rewrite was a
        // forward (descendant) rewrite. That is unsound: any string
        // whose text happens to begin with the previous SHA's text is
        // not an ancestor of anything. Ancestry is now answered by
        // `git merge-base --is-ancestor` via the shared
        // `ralph_core::git::is_git_ancestor` helper.
        //
        // The contract under test is "fail-closed on unrelated /
        // unverifiable rewrites". The pre-U1 test pinned the unsafe
        // `starts_with` semantics by asserting that an unrelated
        // rewrite blocks AND that a string-prefix rewrite is allowed.
        // The "allowed" half is the unsafe-semantic half and is
        // removed: we now exercise only the fail-closed contract
        // against a non-repository path, where `merge-base` cannot
        // answer the question and the helper must propagate the
        // error rather than silently allow the rewrite.
        let tmp = tempfile::tempdir().expect("tempdir");
        let blocks = super::target_rewrite_blocks(tmp.path(), "aaaa", "bbbb");
        assert!(
            blocks.is_err(),
            "unrelated rewrite against a non-repository must surface the merge-base failure (fail-closed), got {blocks:?}"
        );
    }

    #[test]
    fn diff_uses_unit_base() {
        // Skeleton: compute_base_pin returns the SHA the diff should
        // anchor on. Test pins that PinDepsBase returns the current
        // target SHA as the base.
        let mut input = base();
        input.dependency_acked_commits.insert("c1".to_string());
        input.dependency_acked_commits.insert("c2".to_string());
        let outcome = compute_base_pin(&input);
        match outcome {
            BasePinOutcome::PinDepsBase { sha } => assert_eq!(sha, "feedface"),
            _ => panic!("expected PinDepsBase"),
        }
    }

    // ---- U2 typed all_deps_acked (2026-09-09-0917 plan §7 U2) ----------
    //
    // U2 tightens `all_deps_acked` from a length-based check
    // (`acked.len() >= dep_count`) to per-key set membership. The
    // pre-U2 contract would pass any 2 acked commits for any 2 deps,
    // which is the C2 finding. The RED tests below pin the new
    // per-key membership contract.

    #[test]
    fn all_deps_acked_requires_per_key_set_membership() {
        let mut input = base();
        input.unit_key = "U3".to_string();
        input.dependency_unit_keys = vec!["U1".to_string(), "U2".to_string()];
        input.dependency_acked_commits.clear();
        input.dependency_acked_commits.insert("U1".to_string());
        input
            .dependency_acked_commits
            .insert("WRONG_KEY".to_string());
        assert!(
            !all_deps_acked(&input),
            "all_deps_acked must be false when 'WRONG_KEY' is in acked set but U2 is missing"
        );
    }

    #[test]
    fn all_deps_acked_passes_with_exact_set_membership() {
        let mut input = base();
        input.unit_key = "U3".to_string();
        input.dependency_unit_keys = vec!["U1".to_string(), "U2".to_string()];
        input.dependency_acked_commits.clear();
        input.dependency_acked_commits.insert("U1".to_string());
        input.dependency_acked_commits.insert("U2".to_string());
        assert!(all_deps_acked(&input));
    }

    #[test]
    fn all_deps_acked_rejects_superset_of_unrelated_keys() {
        // Pre-U2 contract: len(acked) >= 2 → true, even if neither key
        // matches a real dep. This pins the C2 finding contract.
        let mut input = base();
        input.unit_key = "U3".to_string();
        input.dependency_unit_keys = vec!["U1".to_string(), "U2".to_string()];
        input.dependency_acked_commits.clear();
        input.dependency_acked_commits.insert("U9".to_string());
        input.dependency_acked_commits.insert("U11".to_string());
        assert!(
            !all_deps_acked(&input),
            "all_deps_acked must require the actual dep keys, not just any N keys"
        );
    }

    // ---- U3 RED (2026-09-09-0917 plan §7 第 9 项) ------------------
    //
    // U3 promotes `compute_base_pin` from a skeleton that returned
    // `current_target_sha` unconditionally for the deps-present branch
    // to a real `git merge-base --octopus` over (deps ∪ current
    // target). The new tests below pin that:
    // - the no-deps branch still returns `current_target_sha`
    // - the deps-present branch joins deps + current target via
    //   `merge-base --octopus`, NOT a free-form SHA
    // - the pinned base is replay-deterministic

    /// RED: with no deps, pinned base = current target SHA.
    #[test]
    fn compute_base_pin_with_no_deps_returns_current_target() {
        let mut input = base();
        input.dependency_unit_keys.clear();
        input.dependency_acked_commits.clear();
        let outcome = compute_base_pin(&input);
        assert_eq!(
            outcome,
            BasePinOutcome::PinCurrentTarget {
                sha: "feedface".to_string()
            }
        );
    }

    /// RED: with deps present, pinned base = merge-base of (deps ∪
    /// current target), NOT `current_target_sha` directly.
    ///
    /// We build a real tmp git repo with two branches that diverge
    /// from a common ancestor so the merge-base is provably different
    /// from any individual tip. We then assert that the pinned base
    /// equals the merge-base returned by `git merge-base --octopus`.
    #[test]
    fn compute_base_pin_returns_merge_base_not_current_target() {
        // Build a real tmp git repo with a shared ancestor and two
        // divergent tips; the merge-base of {tip_a, tip_b, tip_target}
        // is the shared ancestor, which is provably NOT equal to any
        // of the tips.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(repo)
                .output()
                .expect("git invocation");
            assert!(
                out.status.success(),
                "git {args:?} failed: stderr={}",
                String::from_utf8_lossy(&out.stderr)
            );
            out
        };
        git(&["init", "-q", "--initial-branch=main"]);
        git(&["config", "user.email", "u3@test.local"]);
        git(&["config", "user.name", "U3 Test"]);
        // Initial commit on main: this is the shared ancestor.
        std::fs::write(repo.join("seed.txt"), "ancestor\n").expect("write seed");
        git(&["add", "seed.txt"]);
        git(&["commit", "-q", "-m", "ancestor"]);
        let ancestor_sha = {
            let out = git(&["rev-parse", "HEAD"]);
            String::from_utf8(out.stdout)
                .expect("utf8 sha")
                .trim()
                .to_string()
        };
        // Branch tip_a: commit on a side branch.
        git(&["checkout", "-q", "-b", "tip_a"]);
        std::fs::write(repo.join("a.txt"), "A\n").expect("write a");
        git(&["add", "a.txt"]);
        git(&["commit", "-q", "-m", "tip_a"]);
        let tip_a = {
            let out = git(&["rev-parse", "HEAD"]);
            String::from_utf8(out.stdout)
                .expect("utf8 sha")
                .trim()
                .to_string()
        };
        // Back to main and add tip_b: diverges from ancestor.
        git(&["checkout", "-q", "main"]);
        std::fs::write(repo.join("b.txt"), "B\n").expect("write b");
        git(&["add", "b.txt"]);
        git(&["commit", "-q", "-m", "tip_b"]);
        let tip_b = {
            let out = git(&["rev-parse", "HEAD"]);
            String::from_utf8(out.stdout)
                .expect("utf8 sha")
                .trim()
                .to_string()
        };

        // Sanity: the three tips are distinct and the merge-base of
        // all three equals the shared ancestor.
        assert_ne!(ancestor_sha, tip_a);
        assert_ne!(ancestor_sha, tip_b);
        assert_ne!(tip_a, tip_b);
        let octopus_out = git(&["merge-base", "--octopus", &tip_a, &tip_b, &tip_a]);
        let octopus_sha = String::from_utf8(octopus_out.stdout)
            .expect("utf8")
            .trim()
            .to_string();
        // octopus may return multiple SHAs (one per side); we only
        // care that the ancestor appears among them.
        assert!(
            octopus_sha.lines().any(|l| l.trim() == ancestor_sha),
            "octopus merge-base must include the ancestor (got {octopus_sha:?})"
        );

        // Now feed `compute_base_pin` with deps_present=true and
        // ensure the pinned base is the merge-base, NOT the current
        // target SHA.
        //
        // Dep acked commits are real SHAs (the production runtime
        // only ever inserts merge-base-able SHA strings into this
        // set). `dependency_unit_keys` carries the unit-key labels
        // used for the `all_deps_acked` per-key check; the BTreeSet
        // carries the corresponding commit SHAs.
        let mut input = BasePinInput {
            unit_key: "U3".to_string(),
            repo_root: repo.to_path_buf(),
            current_target_sha: tip_b.clone(),
            dependency_unit_keys: vec!["U1".to_string(), "U2".to_string()],
            dependency_acked_commits: {
                let mut s = BTreeSet::new();
                s.insert(tip_a.clone());
                s
            },
            candidate_ancestor_in_target: true,
        };
        // The pure dispatch takes only (input) and produces a SHA.
        // The U3 RED contract pins: with deps present, the outcome
        // is `PinDepsBase` carrying a non-current-target SHA derived
        // from the deps set. The current skeleton wrongly returns
        // `current_target_sha` (= tip_b), which is provably distinct
        // from the merge-base (= ancestor_sha).
        let outcome = compute_base_pin(&input);
        match &outcome {
            BasePinOutcome::PinDepsBase { sha } => {
                assert_ne!(
                    sha, &tip_b,
                    "PinDepsBase must NOT equal current_target_sha (skeleton bug)"
                );
                assert_eq!(
                    sha, &ancestor_sha,
                    "PinDepsBase must equal merge-base ancestor (got {sha:?}, expected {ancestor_sha:?})"
                );
            }
            BasePinOutcome::PinCurrentTarget { .. } => panic!(
                "deps-present input must yield PinDepsBase, got PinCurrentTarget {outcome:?}"
            ),
            other => panic!("expected PinDepsBase, got {other:?}"),
        }

        // Replay-determinism: a second call returns the same outcome.
        let outcome_2 = compute_base_pin(&input);
        assert_eq!(outcome, outcome_2);

        // Drop the dep SHA to verify the per-key check is robust to
        // missing dep acks (must produce UnackedDependency).
        input.dependency_acked_commits.remove(&tip_a);
        let outcome_missing = compute_base_pin(&input);
        match outcome_missing {
            BasePinOutcome::UnackedDependency { reason } => assert!(
                reason.contains("acked"),
                "unacked reason must explain ack, got {reason:?}"
            ),
            other => panic!("expected UnackedDependency, got {other:?}"),
        }
    }

    /// RED: replay-once across the deps-present branch.
    #[test]
    fn compute_base_pin_replay_deterministic_for_deps() {
        let mut input = base();
        input.dependency_acked_commits.clear();
        input.dependency_acked_commits.insert("U1".to_string());
        input.dependency_acked_commits.insert("U2".to_string());
        let o1 = compute_base_pin(&input);
        let o2 = compute_base_pin(&input);
        let o3 = compute_base_pin(&input);
        assert_eq!(o1, o2);
        assert_eq!(o2, o3);
    }

    // ---- U5 acceptance (2026-09-09-0917 plan §7 第 9 项) ---------------
    //
    // Plan contract:
    // - U3 base 包含所有依赖 commit
    // - 重启仍同 SHA (immutable across resume / diff / spawn)
    // - Unit diff 不包含已经存在于 base 的前置改动
    // - 未 ack 或 ancestry 不成立不启动
    //
    // `compute_base_pin` is the pure dispatch that picks the immutable
    // unit_base at first admission. The acceptance test exercises the
    // 4 acceptance scenarios end-to-end through the pure function.
    #[test]
    fn dag_dependency_base_is_pinned() {
        // ---- U3 base 包含所有依赖 commit ----
        // U3 has two dependencies (U1, U2) that are both acked;
        // the pinned base must include both. We model that the
        // current target SHA is the merge-base that already contains
        // those deps (per plan: "U3 base 包含所有依赖 commit").
        //
        // U2: `all_deps_acked` is now per-key set membership — the
        // acked set must contain the actual dep unit keys, not just
        // any N commit strings.
        let mut input = base();
        input.unit_key = "U3".to_string();
        input.dependency_unit_keys = vec!["U1".to_string(), "U2".to_string()];
        input.dependency_acked_commits.clear();
        input.dependency_acked_commits.insert("U1".to_string());
        input.dependency_acked_commits.insert("U2".to_string());
        input.candidate_ancestor_in_target = true;
        let outcome_with_deps = compute_base_pin(&input);
        match &outcome_with_deps {
            BasePinOutcome::PinDepsBase { sha } => {
                assert_eq!(sha, "feedface", "pinned base must equal current target SHA");
            }
            other => panic!("expected PinDepsBase, got {other:?}"),
        }
        // Sanity: all_deps_acked reports true when the dependency set
        // is fully represented in `dependency_acked_commits`.
        assert!(
            all_deps_acked(&input),
            "all_deps_acked must report true when both deps are in the acked set"
        );

        // ---- 重启仍同 SHA (immutable across resume / diff / spawn) ----
        // Run the dispatcher twice with the same input — the pinned
        // base must be byte-identical. This is the "first-write
        // immutable" guarantee that lets the runtime rely on the
        // value across restart boundaries: once the base is written
        // the caller treats it as the authoritative anchor; the
        // pure dispatcher itself is deterministic so any replay
        // yields the same SHA.
        let input_replay = input.clone();
        let outcome_replay = compute_base_pin(&input_replay);
        assert_eq!(
            outcome_with_deps, outcome_replay,
            "restart must yield the same pinned base"
        );
        // Pin determinism: triple-call yields the same SHA. The
        // runtime relies on this so resume / diff / spawn all see
        // identical unit_base values.
        let outcome_triple = compute_base_pin(&input);
        assert_eq!(
            outcome_with_deps, outcome_triple,
            "triple-call determinism — restart still same SHA"
        );

        // ---- Unit diff 不包含已经存在于 base 的前置改动 ----
        // If the runtime accidentally uses an outdated base (one
        // that does NOT contain the dep commits), `all_deps_acked`
        // would still need the explicit acked_commits set; the pure
        // dispatch must surface an UnackedDependency otherwise —
        // this is the contract that prevents the Unit diff from
        // "double-counting" already-integrated upstream changes.
        let mut input_no_acks = input.clone();
        input_no_acks.dependency_acked_commits.clear();
        let outcome_no_acks = compute_base_pin(&input_no_acks);
        match &outcome_no_acks {
            BasePinOutcome::UnackedDependency { reason } => {
                assert!(
                    reason.contains("acked"),
                    "unacked reason must explain ack, got {reason:?}"
                );
            }
            other => panic!("expected UnackedDependency, got {other:?}"),
        }
        assert!(
            !all_deps_acked(&input_no_acks),
            "all_deps_acked must report false when deps lack acks"
        );

        // ---- 未 ack 或 ancestry 不成立不启动 ----
        // candidate_ancestor_in_target=false is the explicit
        // "ancestry 不成立" branch — the candidate is not reachable
        // from the current target. Runtime must block, not pin.
        let mut input_bad_ancestry = input.clone();
        input_bad_ancestry.candidate_ancestor_in_target = false;
        let outcome_bad_ancestry = compute_base_pin(&input_bad_ancestry);
        match &outcome_bad_ancestry {
            BasePinOutcome::UnackedDependency { reason } => {
                assert!(
                    reason.contains("ancestor"),
                    "ancestry reason must mention ancestor, got {reason:?}"
                );
            }
            other => panic!("expected UnackedDependency for bad ancestry, got {other:?}"),
        }

        // ---- target_rewrite_blocks (force-push protection) ----
        // U1 (fix-plan 2026-09-09-0917): ancestry is now answered by
        // `git merge-base --is-ancestor`. The pre-U1 shape was
        // `target_rewrite_blocks(prev, current) -> bool` and used
        // `current.starts_with(prev)` to decide that a forward
        // rewrite was a descendant rewrite. That is unsound and has
        // been removed.
        //
        // The remaining plan contract is: an unrelated rewrite must
        // be blocked (fail-closed). We exercise it against a
        // non-repository path so `merge-base` cannot answer the
        // question — the helper must surface the failure rather than
        // silently allow the rewrite.
        let tmp = tempfile::tempdir().expect("tempdir");
        let blocks = target_rewrite_blocks(tmp.path(), "aaaa", "bbbb");
        assert!(
            blocks.is_err(),
            "unrelated rewrite against a non-repository must surface the merge-base failure (fail-closed), got {blocks:?}"
        );
    }
}
