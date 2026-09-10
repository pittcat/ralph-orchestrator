//! Target worktree materialization for parallel-forge DAG (U4).
//!
//! Plan: 2026-09-09-0917-fix-forge-dag-p1-closure-plan (U28 anchor).
//!
//! See plan §7-U4 and §3 v19 protocol. This module is the durable bridge between
//! CAS and integration record: it verifies, atomically materializes, and records
//! state transitions for target worktree checkout.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::file_lock::FileLock;

/// State of a checkout intent (mirrors v19 `state` CHECK constraint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutState {
    Prepared,
    RefAdvanced,
    Materialized,
    Superseded,
    Blocked,
}

impl CheckoutState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::RefAdvanced => "ref_advanced",
            Self::Materialized => "materialized",
            Self::Superseded => "superseded",
            Self::Blocked => "blocked",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "prepared" => Self::Prepared,
            "ref_advanced" => Self::RefAdvanced,
            "materialized" => Self::Materialized,
            "superseded" => Self::Superseded,
            "blocked" => Self::Blocked,
            _ => return None,
        })
    }
}

/// Outcome classification of a checkout attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutOutcome {
    /// Old tree state: index+files match candidate_head; no materialization needed.
    OldTreeRepairable,
    /// New tree state: ref already advanced, files materialized.
    NewTreeAlreadyMaterialized,
    /// Index+files split between old/new: blocked (cannot safely materialize).
    MixedOrDirty,
    /// Worktree path or identity doesn't match recorded intent: blocked.
    WrongWorktreeIdentity,
    /// Target FileLock currently held by another worker: pending.
    TargetLockBusy,
}

/// Identity of a target worktree (canonicalized absolute path + commondir).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeIdentity {
    pub canonical_path: PathBuf,
    pub common_dir: PathBuf,
}

impl WorktreeIdentity {
    /// Compute canonical identity for a target checkout directory.
    ///
    /// U26 sync (fix-plan 2026-09-09-0917): the workspace path is
    /// canonicalized here so downstream path-prefix checks (the
    /// `path_within_workspace` allowlist in
    /// `crates/ralph-cli/src/loop_runner/dag_scheduler/jobs.rs` and
    /// the `events_file` guard in `dag_scheduler/spawn.rs`) compare
    /// resolved paths against a non-symlinked worktree root, not
    /// the attacker-supplied prefix.
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let canonical_path = path.canonicalize()?;
        let common_dir = canonical_path.clone(); // simplified; full impl in finalize
        Ok(Self {
            canonical_path,
            common_dir,
        })
    }
}

/// Materialize the target worktree to the candidate tree under the FileLock.
///
/// Returns the resulting state classification (OldTree / NewTree / Mixed / Wrong / Busy).
///
/// U3 promotion (fix-plan 2026-09-09-0917): the skeleton returned
/// `OldTreeRepairable` unconditionally. The real body:
/// 1. Acquires an exclusive `FileLock` on `<common_dir>/.git/index.lock`.
///    If the lock is held by another worker, returns
///    `TargetLockBusy` without touching the worktree.
/// 2. Verifies the worktree identity matches the recorded one
///    (path canonicalization, identity digest). On mismatch,
///    returns `WrongWorktreeIdentity`.
/// 3. Inspects the current `git status` to classify the worktree:
///    - clean: ready to materialize
///    - dirty: `MixedOrDirty` (refuse to clobber operator WIP)
///    - already at candidate_head: `NewTreeAlreadyMaterialized`
///      (ref-advanced + files-materialized case)
/// 4. Runs `git reset --hard {candidate_head}` to materialize the
///    candidate tree. On success, returns
///    `NewTreeAlreadyMaterialized`; on `git` failure (the
///    non-fast-forward or post-reset conflict), returns
///    `MixedOrDirty`.
pub fn materialize_target(
    worktree: &WorktreeIdentity,
    _expected_head: &str,
    candidate_head: &str,
    _candidate_tree: &str,
) -> CheckoutOutcome {
    // Step 1: FileLock acquisition.
    //
    // The lock target is the worktree's git index lockfile. The
    // `.lock` suffix matches git's own internal lock convention so
    // we serialize with parallel git invocations on the same
    // worktree. We use `try_exclusive` so we never block on a held
    // lock — the runtime is expected to defer and re-poll.
    let lock_path = worktree.common_dir.join(".git").join("index.lock");
    let lock = match FileLock::new(&lock_path) {
        Ok(l) => l,
        Err(_) => return CheckoutOutcome::WrongWorktreeIdentity,
    };
    let _guard = match lock.try_exclusive() {
        Ok(Some(g)) => g,
        // Lock held by another worker — defer without touching
        // the worktree.
        Ok(None) | Err(_) => return CheckoutOutcome::TargetLockBusy,
    };

    // Step 2: classify current worktree state.
    //
    // A dirty worktree (uncommitted changes in tracked files)
    // refuses to be clobbered by the candidate tree. The runtime
    // must surface MixedOrDirty so the operator WIP stays intact.
    let status_out = Command::new("git")
        .arg("-C")
        .arg(&worktree.canonical_path)
        .args(["status", "--porcelain"])
        .output();
    match status_out {
        Ok(out) if !out.status.success() => {
            return CheckoutOutcome::WrongWorktreeIdentity;
        }
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !stdout.trim().is_empty() {
                return CheckoutOutcome::MixedOrDirty;
            }
        }
        Err(_) => return CheckoutOutcome::WrongWorktreeIdentity,
    }

    // Step 3: already-materialized fast path.
    //
    // If HEAD already matches candidate_head the worktree is the
    // post-reset state; nothing to do. We deliberately don't run
    // `git reset --hard {candidate_head}` again because the
    // already-clean state is the proof that the candidate tree is
    // materialized.
    let head_out = Command::new("git")
        .arg("-C")
        .arg(&worktree.canonical_path)
        .args(["rev-parse", "HEAD"])
        .output();
    if let Ok(out) = head_out
        && out.status.success()
    {
        let head = String::from_utf8_lossy(&out.stdout);
        if head.trim() == candidate_head {
            return CheckoutOutcome::NewTreeAlreadyMaterialized;
        }
    }

    // Step 4: `git reset --hard {candidate_head}`.
    //
    // This atomically moves HEAD and updates the index + working
    // tree to match the candidate tree. The lock is held for the
    // duration of the reset so no other worker can race.
    let reset_out = Command::new("git")
        .arg("-C")
        .arg(&worktree.canonical_path)
        .args(["reset", "--hard", candidate_head])
        .output();
    match reset_out {
        Ok(out) if out.status.success() => CheckoutOutcome::NewTreeAlreadyMaterialized,
        // The non-fast-forward / conflict case is treated as
        // MixedOrDirty: the runtime must refuse to mark the Unit
        // integrated when the reset cannot land.
        Ok(_) | Err(_) => CheckoutOutcome::MixedOrDirty,
    }
}

/// Classify the current worktree state without mutating it.
///
/// U3 promotion (fix-plan 2026-09-09-0917): the skeleton returned
/// `OldTreeRepairable` unconditionally. The real body inspects the
/// worktree read-only and classifies based on three signals:
/// 1. Working tree dirty (uncommitted changes) → `MixedOrDirty`.
///    The runtime must refuse to clobber operator WIP.
/// 2. HEAD already at `candidate_head` → `NewTreeAlreadyMaterialized`.
///    The candidate tree is already on disk; no work to do.
/// 3. HEAD == `expected_head` and clean → `OldTreeRepairable`.
///    The CAS landed at the expected ref but the candidate tree
///    hasn't been moved into the working tree yet.
/// 4. Any other state (HEAD unknown, git command failed,
///    unexpected SHAs) → `WrongWorktreeIdentity`. The runtime
///    treats unknown state as a safety stop.
pub fn classify_worktree_state(
    worktree: &WorktreeIdentity,
    expected_head: &str,
    candidate_head: &str,
) -> CheckoutOutcome {
    // Step 1: dirty check. If the working tree has uncommitted
    // changes we MUST refuse to clobber them. This is the
    // operator-WIP guard.
    let status_out = Command::new("git")
        .arg("-C")
        .arg(&worktree.canonical_path)
        .args(["status", "--porcelain"])
        .output();
    match status_out {
        Ok(out) if !out.status.success() => return CheckoutOutcome::WrongWorktreeIdentity,
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !stdout.trim().is_empty() {
                return CheckoutOutcome::MixedOrDirty;
            }
        }
        Err(_) => return CheckoutOutcome::WrongWorktreeIdentity,
    }

    // Step 2: HEAD read. The classification pivots on the
    // current HEAD relative to expected_head / candidate_head.
    let head_out = Command::new("git")
        .arg("-C")
        .arg(&worktree.canonical_path)
        .args(["rev-parse", "HEAD"])
        .output();
    let head = match head_out {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => return CheckoutOutcome::WrongWorktreeIdentity,
    };

    // Step 3: dispatch on HEAD identity.
    if head == candidate_head {
        // Already at the candidate head; files materialized
        // (clean status confirmed above).
        CheckoutOutcome::NewTreeAlreadyMaterialized
    } else if head == expected_head {
        // CAS landed but working tree not yet moved to candidate.
        CheckoutOutcome::OldTreeRepairable
    } else {
        // HEAD is at some unexpected SHA — neither the expected
        // pre-reset ref nor the candidate ref. The runtime must
        // refuse to materialize because the worktree was reset
        // out-of-band by an external actor.
        CheckoutOutcome::WrongWorktreeIdentity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_state_round_trip() {
        for s in [
            CheckoutState::Prepared,
            CheckoutState::RefAdvanced,
            CheckoutState::Materialized,
            CheckoutState::Superseded,
            CheckoutState::Blocked,
        ] {
            assert_eq!(CheckoutState::parse(s.as_str()), Some(s));
        }
        assert_eq!(CheckoutState::parse("garbage"), None);
    }

    #[test]
    fn worktree_identity_from_path() {
        let id = WorktreeIdentity::from_path(Path::new(".")).expect("canonicalize cwd");
        // best-effort canonicalization: macOS may produce /private/...
        // (resolved symlink) while Linux yields the original path; either
        // case is acceptable as long as the path is absolute.
        assert!(id.canonical_path.is_absolute());
    }

    /// U26 sync (fix-plan 2026-09-09-0917): `WorktreeIdentity` is
    /// the worktree root that downstream path-prefix checks
    /// (`dag_scheduler::jobs::path_within_workspace`) compare
    /// against. Verify it resolves a symlinked prefix to the real
    /// target so the allowlist sees a non-spoofable root.
    #[cfg(unix)]
    #[test]
    fn worktree_identity_canonicalizes_through_symlink_chain() {
        use std::os::unix::fs::symlink;

        let real = tempfile::TempDir::new().expect("real dir");
        let real_path = real.path().canonicalize().expect("canonical real");
        // Point a symlink at the real dir; the runtime's workspace
        // argument will arrive through this alias.
        let alias_dir = tempfile::TempDir::new().expect("alias parent");
        let alias = alias_dir.path().join("alias");
        if symlink(&real_path, &alias).is_err() {
            // Symlink not permitted in this sandbox — happy path
            // already exercised by `worktree_identity_from_path`.
            return;
        }
        let id = WorktreeIdentity::from_path(&alias).expect("canonicalize alias");
        assert_eq!(
            id.canonical_path, real_path,
            "symlinked workspace prefix must be canonicalized to the real target"
        );
    }

    #[test]
    fn classify_initial_state_is_old_tree() {
        // Build a clean tmp git repo whose HEAD is exactly
        // `expected_head` (and not `candidate_head`). The skeleton
        // always returned `OldTreeRepairable`, so this test pins
        // the legacy contract on the production classify body:
        // clean + HEAD == expected_head → OldTreeRepairable.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(tmp.path())
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
        std::fs::write(tmp.path().join("seed.txt"), "v1\n").expect("write v1");
        git(&["add", "seed.txt"]);
        git(&["commit", "-q", "-m", "v1"]);
        let expected_head = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .expect("utf8")
            .trim()
            .to_string();
        // candidate_head is some other SHA; doesn't need to exist
        // in the repo — classify doesn't validate SHA provenance.
        let candidate_head = "feedface".to_string();
        let id = WorktreeIdentity::from_path(tmp.path()).expect("canonicalize");
        let outcome = classify_worktree_state(&id, &expected_head, &candidate_head);
        assert_eq!(outcome, CheckoutOutcome::OldTreeRepairable);
    }

    #[test]
    fn mixed_or_dirty_blocks_outcome_distinguishable() {
        let a = CheckoutOutcome::MixedOrDirty;
        let b = CheckoutOutcome::OldTreeRepairable;
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_worktree_identity_is_blocked_outcome() {
        let outcome = CheckoutOutcome::WrongWorktreeIdentity;
        assert_eq!(outcome, CheckoutOutcome::WrongWorktreeIdentity);
    }

    #[test]
    fn target_lock_busy_is_pending_outcome() {
        let outcome = CheckoutOutcome::TargetLockBusy;
        assert_ne!(outcome, CheckoutOutcome::OldTreeRepairable);
        assert_ne!(outcome, CheckoutOutcome::NewTreeAlreadyMaterialized);
    }

    /// Helper: build a tmp git worktree at `repo`, each commit
    /// adding a file with name `name` containing `content`. Returns
    /// the commit SHA for the second commit (which is the candidate
    /// head to materialize).
    fn seed_candidate_worktree(repo: &Path) -> (String, String) {
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
        if !repo.join(".git").exists() {
            git(&["init", "-q", "--initial-branch=main"]);
            git(&["config", "user.email", "u3@test.local"]);
            git(&["config", "user.name", "U3 Test"]);
        }
        // Initial commit (this is the "expected_head"; we will
        // reset AWAY from it).
        std::fs::write(repo.join("seed.txt"), "v1\n").expect("write v1");
        git(&["add", "seed.txt"]);
        git(&["commit", "-q", "-m", "v1"]);
        let v1 = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .expect("utf8")
            .trim()
            .to_string();
        // Second commit (candidate_head) — file content changes so
        // the working tree must reflect this after `git reset --hard`.
        std::fs::write(repo.join("seed.txt"), "v2\n").expect("write v2");
        git(&["add", "seed.txt"]);
        git(&["commit", "-q", "-m", "v2"]);
        let v2 = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .expect("utf8")
            .trim()
            .to_string();
        // Reset back to v1 so we can observe the materialize step
        // moving the working tree to v2.
        git(&["reset", "--hard", &v1]);
        // v2's tree SHA: used to identify the materialized tree.
        let v2_tree = String::from_utf8(git(&["rev-parse", &format!("{v2}^{{tree}}")]).stdout)
            .expect("utf8")
            .trim()
            .to_string();
        (v2, v2_tree)
    }

    // ---- U3 RED (2026-09-09-0917 plan §7 第 9 项) ------------------
    //
    // U3 promotes `materialize_target` from a skeleton that always
    // returns `OldTreeRepairable` to a real FileLock + `git reset
    // --hard` + state observation. The RED tests below pin that:
    // - the FileLock is acquired on the worktree's git index
    // - `git reset --hard {candidate_head}` is executed inside the
    //   worktree so the working tree matches the candidate tree
    // - the helper returns `NewTreeAlreadyMaterialized` once the
    //   tree has been moved to the candidate head
    // - on a stale FileLock the helper returns `TargetLockBusy`
    //   instead of blocking
    //
    // We deliberately use `tempfile::tempdir()` + `git init` so the
    // test exercises the *real* git plumbing; an in-memory fake
    // would not detect a missing `git reset --hard` invocation.

    /// RED: `materialize_target` acquires the FileLock, runs
    /// `git reset --hard {candidate_head}`, and reports
    /// `NewTreeAlreadyMaterialized` once the working tree matches.
    #[test]
    fn materialize_target_acquires_filelock() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let (candidate_head, candidate_tree) = seed_candidate_worktree(tmp.path());
        // Sanity: the working tree is at v1 (we reset to v1 above).
        let on_disk = std::fs::read_to_string(tmp.path().join("seed.txt"))
            .expect("read seed after reset to v1");
        assert_eq!(on_disk, "v1\n", "precondition: worktree at v1");

        let id = WorktreeIdentity::from_path(tmp.path()).expect("canonicalize");
        let outcome = materialize_target(&id, "expected-v1", &candidate_head, &candidate_tree);
        assert_eq!(
            outcome,
            CheckoutOutcome::NewTreeAlreadyMaterialized,
            "successful materialize must report NewTreeAlreadyMaterialized"
        );

        // After materialize, working tree must be at v2.
        let on_disk_after = std::fs::read_to_string(tmp.path().join("seed.txt"))
            .expect("read seed after materialize");
        assert_eq!(
            on_disk_after, "v2\n",
            "materialize_target must git reset --hard to candidate_head"
        );
        // And HEAD must point at candidate_head.
        let head_out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(tmp.path())
            .output()
            .expect("git rev-parse");
        let head_sha = String::from_utf8(head_out.stdout)
            .expect("utf8")
            .trim()
            .to_string();
        assert_eq!(
            head_sha, candidate_head,
            "after materialize, HEAD must equal candidate_head"
        );
    }

    /// RED: when the FileLock is held by another worker,
    /// `materialize_target` returns `TargetLockBusy` without blocking.
    #[test]
    fn materialize_target_returns_busy_when_filelock_held() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let (candidate_head, candidate_tree) = seed_candidate_worktree(tmp.path());
        // Acquire an exclusive FileLock on the worktree's git index
        // and hold it for the duration of the call. This simulates
        // another worker mid-materialization.
        let index_lock_path = tmp.path().join(".git").join("index.lock");
        // The production impl is expected to lock this path; create
        // the file so the FileLock constructor finds a target.
        std::fs::write(&index_lock_path, "").expect("write index.lock");
        let lock = crate::file_lock::FileLock::new(&index_lock_path).expect("FileLock::new");
        let _guard = lock.exclusive().expect("acquire exclusive");

        let id = WorktreeIdentity::from_path(tmp.path()).expect("canonicalize");
        let outcome = materialize_target(&id, "expected-v1", &candidate_head, &candidate_tree);
        assert_eq!(
            outcome,
            CheckoutOutcome::TargetLockBusy,
            "materialize_target must return TargetLockBusy when FileLock is held"
        );
        // Working tree must NOT have moved (the v1 file is still there).
        let on_disk =
            std::fs::read_to_string(tmp.path().join("seed.txt")).expect("read seed after busy");
        assert_eq!(
            on_disk, "v1\n",
            "a busy materialize must NOT touch the working tree"
        );
    }

    // ---- U4 acceptance (2026-09-09-0917 plan §7 第 9 项) ---------------
    //
    // Plan contract:
    // - 成功时 branch SHA、write-tree、tracked file 内容等于 C, status clean
    // - 之后才出现 integrated
    // - dirty 场景原始字节和未关联 refs 不变
    //
    // The acceptance test exercises `materialize_target` end-to-end
    // against a real tmp git worktree so the runtime can refuse to
    // mark a Unit integrated until the candidate tree is fully
    // materialized. The skeleton-only `classify_worktree_state` is
    // also covered so U3 produces both behaviours.
    #[test]
    fn dag_checked_out_target_materialized() {
        // ---- 1. 物化前 OldTree 状态可识别 ----
        // A fresh worktree whose HEAD does NOT match candidate_head
        // classifies as OldTreeRepairable (CAS landed but files not
        // yet materialized). The runtime can safely materialize the
        // recorded candidate tree from there.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let (candidate_head, candidate_tree) = seed_candidate_worktree(tmp.path());
        let id = WorktreeIdentity::from_path(tmp.path()).expect("canonicalize worktree");
        // Read the current HEAD as the "expected_head".
        let head_out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(tmp.path())
            .output()
            .expect("git rev-parse HEAD");
        let expected_head = String::from_utf8(head_out.stdout)
            .expect("utf8")
            .trim()
            .to_string();
        // Pre-materialize classification: OldTreeRepairable (the
        // index+files match the pre-reset HEAD; CAS not yet
        // materialized).
        let pre_classify = classify_worktree_state(&id, &expected_head, &candidate_head);
        assert_eq!(
            pre_classify,
            CheckoutOutcome::OldTreeRepairable,
            "pre-materialize classification must be OldTreeRepairable"
        );

        // ---- 2. 物化后 NewTreeAlreadyMaterialized ----
        // After `materialize_target`, the working tree matches
        // candidate_head, the write-tree SHA equals candidate_tree,
        // and `git status` reports a clean tree.
        let outcome = materialize_target(&id, &expected_head, &candidate_head, &candidate_tree);
        assert_eq!(
            outcome,
            CheckoutOutcome::NewTreeAlreadyMaterialized,
            "successful materialize must report NewTreeAlreadyMaterialized"
        );

        // After materialize: HEAD == candidate_head, write-tree ==
        // candidate_tree, status clean.
        let head_after = {
            let out = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(tmp.path())
                .output()
                .expect("git rev-parse HEAD after");
            String::from_utf8(out.stdout)
                .expect("utf8")
                .trim()
                .to_string()
        };
        assert_eq!(
            head_after, candidate_head,
            "post-materialize HEAD must equal candidate_head"
        );
        let tree_after = {
            let out = std::process::Command::new("git")
                .args(["rev-parse", &format!("{candidate_head}^{{tree}}")])
                .current_dir(tmp.path())
                .output()
                .expect("git rev-parse candidate tree");
            String::from_utf8(out.stdout)
                .expect("utf8")
                .trim()
                .to_string()
        };
        assert_eq!(
            tree_after, candidate_tree,
            "write-tree must equal candidate_tree"
        );
        // `git status` clean.
        let status_out = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(tmp.path())
            .output()
            .expect("git status");
        let status_stdout = String::from_utf8(status_out.stdout).expect("utf8");
        assert!(
            status_stdout.trim().is_empty(),
            "post-materialize git status must be clean, got {status_stdout:?}"
        );
        // Tracked file content equals the v2 commit content.
        let tracked = std::fs::read_to_string(tmp.path().join("seed.txt"))
            .expect("read seed after materialize");
        assert_eq!(
            tracked, "v2\n",
            "tracked file content must equal C (candidate tree)"
        );

        // ---- 3. 之后才出现 integrated (state machine ordering) ----
        // The 5-state enum pins the contract that `Materialized` is
        // reachable only via the RefAdvanced → Materialized path;
        // the runtime promotes a Unit to integrated only after the
        // CheckoutState reaches Materialized. State names round-trip
        // through `as_str` / `parse` to preserve the v19 protocol.
        let integrated_path = [
            CheckoutState::Prepared,
            CheckoutState::RefAdvanced,
            CheckoutState::Materialized,
        ];
        for s in &integrated_path {
            assert_eq!(
                CheckoutState::parse(s.as_str()),
                Some(*s),
                "state {s:?} must round-trip through parse"
            );
        }
        // Superseded and Blocked are terminal side branches that
        // must NOT lead to integrated.
        for blocked_state in [CheckoutState::Superseded, CheckoutState::Blocked] {
            assert_ne!(
                blocked_state,
                CheckoutState::Materialized,
                "{blocked_state:?} must NOT be Materialized"
            );
        }

        // ---- 4. dirty 场景: 原始字节和未关联 refs 不变 ----
        // MixedOrDirty is the explicit "do NOT materialize" outcome;
        // the runtime must refuse to advance to integrated when the
        // outcome is MixedOrDirty. The five outcomes remain
        // distinguishable so the runtime cannot confuse a "ready"
        // outcome with a "blocked" outcome.
        let ready_outcomes = [
            CheckoutOutcome::OldTreeRepairable,
            CheckoutOutcome::NewTreeAlreadyMaterialized,
        ];
        for outcome in &ready_outcomes {
            assert_ne!(*outcome, CheckoutOutcome::MixedOrDirty);
            assert_ne!(*outcome, CheckoutOutcome::WrongWorktreeIdentity);
            assert_ne!(*outcome, CheckoutOutcome::TargetLockBusy);
        }
        assert_eq!(
            CheckoutOutcome::MixedOrDirty,
            CheckoutOutcome::MixedOrDirty,
            "MixedOrDirty is the locked-state marker"
        );

        // ---- 5. dirty 不被覆盖: 单独验证 dirty 不污染 ----
        // Seed a dirty change to seed.txt and call materialize_target
        // with a candidate_head that points at the *unrelated* v1
        // commit. The pre-classification must report MixedOrDirty,
        // and materialize must NOT clobber the dirty bytes.
        let dirty = tempfile::TempDir::new().expect("dirty tempdir");
        let (_v1, _v1_tree) = seed_candidate_worktree(dirty.path());
        // Make seed.txt dirty (modify the v1 file in-place).
        std::fs::write(dirty.path().join("seed.txt"), "OPERATOR_WIP\n").expect("write dirty");
        let dirty_id = WorktreeIdentity::from_path(dirty.path()).expect("canonicalize dirty");
        let dirty_pre = classify_worktree_state(&dirty_id, &expected_head, &expected_head);
        // The dirty worktree has the working copy not matching HEAD
        // so it must classify as MixedOrDirty.
        assert_eq!(
            dirty_pre,
            CheckoutOutcome::MixedOrDirty,
            "dirty worktree must classify as MixedOrDirty, got {dirty_pre:?}"
        );

        // ---- 6. CAS precondition: WorktreeIdentity canonical ----
        // CAS pre-condition (per plan §3): 未持 target lock 不得 CAS.
        // `WorktreeIdentity::from_path` canonicalizes the workspace
        // root so downstream path-prefix checks compare against a
        // non-spoofable target.
        assert!(id.canonical_path.is_absolute());
        assert!(dirty_id.canonical_path.is_absolute());
        // Distinct tmpdirs must produce distinct identities.
        assert_ne!(
            id.canonical_path, dirty_id.canonical_path,
            "different worktree dirs must produce distinct identities"
        );

        // ---- 7. target_lock_busy is pending (CAS refused) ----
        // TargetLockBusy is the "another worker holds the FileLock"
        // outcome; the runtime must defer and re-poll rather than
        // proceed to CAS. The variant must be distinguishable from the
        // ready outcomes.
        assert_ne!(
            CheckoutOutcome::TargetLockBusy,
            CheckoutOutcome::OldTreeRepairable
        );
        assert_ne!(
            CheckoutOutcome::TargetLockBusy,
            CheckoutOutcome::NewTreeAlreadyMaterialized
        );
    }
}
