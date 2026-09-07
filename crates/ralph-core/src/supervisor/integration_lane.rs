//! 2026-09-03-0959 plan U7 (R7; S8-S11; D7-D9; E10-E12):
//! per-target integration lease + the compare-and-swap
//! fast-forward pipeline.
//!
//! # Invariants
//!
//! 1. **One lease per target.** A lane refuses `try_acquire`
//!    while another guard is alive; the second caller fails
//!    with [`LaneError::TargetBusy`].
//! 2. **Eligibility is deterministic.** When multiple Units
//!    are ready for the same target, [`select_eligible`]
//!    returns them in stable
//!    `(integration_order, unit_id)` order. Same input →
//!    same output (replay determinism).
//! 3. **CAS on the lane's expected head.** A candidate only
//!    lands when `git` reports `current_target_oid` ==
//!    `expected_head_before`. If a sibling unit raced the FF
//!    in between `prepare_squash_candidate` and
//!    `compare_and_swap_ff`, the CAS refuses and the caller
//!    must retry — never silently overwrite.
//! 4. **Lane guard is RAII.** Dropping a [`LaneGuard`]
//!    releases the lease. There is no `release()` API — the
//!    borrow checker is the lock.
//!
//! # Trait split
//!
//! The lane is generic over a `Repo` parameter (a marker
//! struct: `RealRepo` or `FakeRepo`) and an associated
//! [`GitIntegrationPort`] impl. The Real variant spawns
//! `git`; the Fake variant is a small state machine the
//! tests drive directly. Both implement the same trait so
//! the lane code is identical.
//!
//! The integration *use site* (U7's `integration.rs`)
//! owns the lane, not the trait impl — this module exposes
//! lane + ports, the use site composes them.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Marker for the live, real-world git-backed lane.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealRepo;

/// Marker for the in-memory test lane. The companion
/// [`FakeGitIntegrationPort`] is what tests construct
/// directly.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeRepo;

/// Per-target integration candidate assembled by the
/// integrator from a Unit's reviewed diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationCandidate {
    pub unit_id: String,
    pub integration_order: u32,
    pub target_branch: String,
    pub base_commit: String,
    pub unit_commit: String,
    /// Authorised changed paths (second check, at lock time).
    pub authorised_paths: Vec<PathBuf>,
}

/// Squash merge candidate produced by
/// [`GitIntegrationPort::prepare_squash_candidate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SquashCandidate {
    pub unit_id: String,
    pub target_branch: String,
    pub base_commit: String,
    pub squash_commit: String,
    pub tree_oid: String,
    pub message: String,
}

/// Outcome of running the targeted gate against the
/// squash candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// Targeted tests pass; lane is cleared to advance.
    Pass,
    /// Targeted tests fail; lane refuses to FF.
    Fail { reason: String },
}

/// Outcome of compare-and-swap fast-forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CasOutcome {
    /// Lane advanced; the new target HEAD is `new_head`.
    Advanced { new_head: String },
    /// Target moved under us; CAS refused; caller must
    /// re-read the head and retry.
    StaleExpected { expected: String, actual: String },
    /// Lane refused to FF (e.g. non-FF ahead, dirty index,
    /// git plumbing error). Caller fails the candidate.
    Refused { reason: String },
}

/// Lane error surface. The lane is fail-closed: every
/// error here is a reason to reject the candidate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaneError {
    #[error("target branch is busy with another live lease")]
    TargetBusy,
    #[error("candidate is missing required field: {0}")]
    MissingField(&'static str),
    #[error("base commit '{0}' not found in target lane")]
    UnknownBase(String),
    #[error("unit '{0}' has no eligible lane entry (not in admission set)")]
    IneligibleUnit(String),
    /// P0-3: an externally supplied commit-ish / branch name failed
    /// the shape whitelist before git ever saw it.
    #[error("invalid {field} shape: '{value}'")]
    InvalidInput { field: &'static str, value: String },
    /// D19/S19: the unit's diff (verified base → unit commit) did
    /// not apply cleanly onto the lane-time head (a sibling changed
    /// overlapping lines). The candidate is rejected; the target is
    /// untouched.
    #[error("unit diff does not apply onto the lane-time head for unit '{unit_id}': {reason}")]
    ApplyConflict { unit_id: String, reason: String },
    #[error("lane state error: {0}")]
    StateError(String),
}

pub type LaneResult<T> = Result<T, LaneError>;

/// Abstract port over `git` plumbing. The Real impl spawns
/// `git`; the Fake impl is a pure state machine the tests
/// control. The lane depends ONLY on this trait.
pub trait GitIntegrationPort: Send + Sync {
    /// Current HEAD of `target_branch` as reported by git.
    fn current_target_oid(&self, target_branch: &str) -> LaneResult<String>;

    /// Build a single squash commit on top of `parent_commit`
    /// (the lane-time target head, D19/S19) whose tree is
    /// `parent_commit`'s tree PLUS the candidate's unit diff
    /// (`candidate.base_commit` → `candidate.unit_commit`) applied
    /// onto it. Returns the new commit OID and the tree OID it
    /// resolved to. Does NOT advance any branch. A diff that does
    /// not apply cleanly onto `parent_commit` fails with
    /// [`LaneError::ApplyConflict`] — the target stays untouched.
    fn prepare_squash_candidate(
        &self,
        candidate: &IntegrationCandidate,
        parent_commit: &str,
    ) -> LaneResult<SquashCandidate>;

    /// Run the targeted gate on `squash.tree_oid`. The
    /// gate must be read-only against the workspace — it
    /// must NOT advance the target.
    fn run_targeted_gate(&self, squash: &SquashCandidate) -> LaneResult<GateOutcome>;

    /// Compare-and-swap fast-forward:
    ///   IF `target_branch` is currently at `expected_head_before`
    ///   THEN move it to `squash.squash_commit` (FF only)
    ///   AND return `CasOutcome::Advanced { new_head }`.
    ///   ELSE return `CasOutcome::StaleExpected { expected, actual }`.
    ///   ELSE (non-FF / dirty / plumbing error)
    ///   return `CasOutcome::Refused { reason }`.
    fn compare_and_swap_ff(
        &self,
        target_branch: &str,
        expected_head_before: &str,
        squash: &SquashCandidate,
    ) -> LaneResult<CasOutcome>;
}

/// Per-target lane state. Shared by all callers; the
/// `Mutex` makes `try_acquire` serialised.
#[derive(Debug, Default)]
pub struct LaneCore {
    /// Branch → current lock holder's unit_id.
    holders: Mutex<BTreeMap<String, String>>,
}

impl LaneCore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Atomically: if `target_branch` is free, mark it held
    /// by `unit_id` and return a [`LaneGuard`]. Otherwise
    /// return [`LaneError::TargetBusy`].
    pub fn try_acquire(&self, target_branch: &str, unit_id: &str) -> LaneResult<LaneGuard<'_>> {
        let mut holders = self
            .holders
            .lock()
            .map_err(|_| LaneError::StateError("holders mutex poisoned".into()))?;
        if holders.contains_key(target_branch) {
            return Err(LaneError::TargetBusy);
        }
        holders.insert(target_branch.to_string(), unit_id.to_string());
        Ok(LaneGuard {
            holders: &self.holders,
            target_branch: target_branch.to_string(),
            released: false,
        })
    }

    /// Read-only: returns the current holder of
    /// `target_branch` (if any).
    pub fn current_holder(&self, target_branch: &str) -> LaneResult<Option<String>> {
        let holders = self
            .holders
            .lock()
            .map_err(|_| LaneError::StateError("holders mutex poisoned".into()))?;
        Ok(holders.get(target_branch).cloned())
    }
}

impl LaneError {
    /// Borrow the unit-id hint when relevant. Currently a
    /// no-op because [`LaneError`] is a flat enum and the
    /// caller formats the holder from
    /// [`LaneCore::current_holder`] directly; kept as a
    /// convenience method so the API stays stable if we
    /// later add a structured `TargetBusy { branch, holder }`
    /// variant.
    pub fn hint(_branch: &str, _holder: &str) -> &'static str {
        "target_branch is held by another unit"
    }
}

/// RAII guard returned by [`LaneCore::try_acquire`].
/// Dropping releases the lease.
pub struct LaneGuard<'a> {
    holders: &'a Mutex<BTreeMap<String, String>>,
    target_branch: String,
    released: bool,
}

impl std::fmt::Debug for LaneGuard<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaneGuard")
            .field("target_branch", &self.target_branch)
            .field("released", &self.released)
            .finish()
    }
}

impl<'a> LaneGuard<'a> {
    /// Branch the guard owns.
    pub fn target_branch(&self) -> &str {
        &self.target_branch
    }

    /// Explicit early release; Drop also releases, this is
    /// just for symmetry / readability.
    pub fn release(mut self) {
        self.do_release();
        // Mark released so the subsequent `Drop` no-ops.
        self.released = true;
    }

    fn do_release(&mut self) {
        if self.released {
            return;
        }
        if let Ok(mut holders) = self.holders.lock() {
            holders.remove(&self.target_branch);
            self.released = true;
        }
    }
}

impl<'a> Drop for LaneGuard<'a> {
    fn drop(&mut self) {
        self.do_release();
    }
}

/// Top-level lane. Owns a shared [`LaneCore`] (lock map)
/// and a shared [`GitIntegrationPort`].
pub struct IntegrationLane<R: 'static, P: GitIntegrationPort + 'static> {
    pub core: Arc<LaneCore>,
    pub port: Arc<P>,
    _phantom: PhantomData<R>,
}

impl<R, P> IntegrationLane<R, P>
where
    P: GitIntegrationPort + 'static,
{
    pub fn new(core: Arc<LaneCore>, port: Arc<P>) -> Self {
        Self {
            core,
            port,
            _phantom: PhantomData,
        }
    }
}

/// Eligible-unit selector. Stable order on
/// `(integration_order, unit_id)` so two snapshots with
/// the same input yield the same output.
pub fn select_eligible(candidates: &[IntegrationCandidate]) -> Vec<&IntegrationCandidate> {
    let mut sorted: Vec<&IntegrationCandidate> = candidates.iter().collect();
    sorted.sort_by(|a, b| {
        a.integration_order
            .cmp(&b.integration_order)
            .then_with(|| a.unit_id.cmp(&b.unit_id))
    });
    sorted
}

// ===========================================================================
// Real git-backed port. Spawns the real `git` binary.
// ===========================================================================

/// One command in the targeted gate's command set (PMI-004①).
/// The real port executes these against a throwaway worktree
/// checked out at the squash commit; the FIRST non-zero exit
/// fails the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateCommandSpec {
    /// Program to execute (resolved via PATH of the lane's
    /// process, run with the gate worktree as CWD).
    pub program: String,
    /// Verbatim arguments. No shell interpolation — the spec is
    /// an argv vector, never a shell string.
    pub args: Vec<String>,
}

/// Real git-backed [`GitIntegrationPort`]. Uses
/// `git rev-parse --verify refs/heads/<target_branch>` for
/// the head (resolves both loose refs and `.git/packed-refs`
/// after a `git gc` / `git pack-refs --all`);
/// `git merge --squash` + `git commit-tree` for the squash;
/// `git update-ref` with the CAS check for the FF.
pub struct RealGitIntegrationPort {
    pub repo_root: PathBuf,
    /// Per-target gate command set (PMI-004①). Empty set is
    /// FAIL-CLOSED in `run_targeted_gate` — the placeholder
    /// unconditional Pass is gone.
    gate_commands: Vec<GateCommandSpec>,
}

impl RealGitIntegrationPort {
    /// Construct the port with NO gate commands. Note this is
    /// NOT the old placeholder behaviour: with an empty command
    /// set `run_targeted_gate` returns `GateOutcome::Fail`
    /// (fail-closed), so lane-level unit tests that want a
    /// passing gate must declare a trivially-passing command
    /// via [`Self::with_gate_commands`].
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            repo_root,
            gate_commands: Vec::new(),
        }
    }

    /// Replace the gate command set. Builder-style: consumes and
    /// returns the port so `real_orchestrator` can compose it
    /// with the caller's per-target spec in one expression.
    pub fn with_gate_commands(mut self, commands: Vec<GateCommandSpec>) -> Self {
        self.gate_commands = commands;
        self
    }

    /// Read-only view of the gate command set.
    pub fn gate_commands(&self) -> &[GateCommandSpec] {
        &self.gate_commands
    }

    /// Execute the gate command spec inside the throwaway
    /// worktree at `worktree_path`. Every command runs with the
    /// worktree as CWD; the FIRST non-zero exit FAILS the gate
    /// (short-circuit). Stdout/stderr of the failing command is
    /// truncated into the Fail reason (bounded, no env echo).
    fn run_gate_commands_in(&self, worktree_path: &str, unit_id: &str) -> LaneResult<GateOutcome> {
        for spec in &self.gate_commands {
            let mut cmd = std::process::Command::new(&spec.program);
            cmd.args(&spec.args);
            cmd.current_dir(worktree_path);
            // Read-only hygiene: strip inherited env so the gate
            // observes only the squash tree, not host secrets or
            // host git config (plan U7 #14 controlled-environment
            // rule). PATH is re-seeded from the lane's own process
            // so the gate commands resolve their programs.
            cmd.env_clear();
            cmd.env("PATH", std::env::var("PATH").unwrap_or_default());
            let out = cmd
                .output()
                .map_err(|e| LaneError::StateError(format!("gate {}: spawn: {e}", spec.program)))?;
            if !out.status.success() {
                let stderr_tail = {
                    let s = String::from_utf8_lossy(&out.stderr);
                    // Char-boundary-safe tail: `floor_char_boundary`
                    // avoids slicing inside a multi-byte character
                    // (same fix family as PMI-001).
                    let floor = crate::text::floor_char_boundary(&s, s.len().saturating_sub(500));
                    s[floor..].to_string()
                };
                return Ok(GateOutcome::Fail {
                    reason: format!(
                        "targeted gate command {:?} (unit {}) exited {:?}: {}",
                        spec.program,
                        unit_id,
                        out.status.code(),
                        stderr_tail
                    ),
                });
            }
        }
        Ok(GateOutcome::Pass)
    }
}

impl GitIntegrationPort for RealGitIntegrationPort {
    fn current_target_oid(&self, target_branch: &str) -> LaneResult<String> {
        // P0-3: the branch name is interpolated into the ref below;
        // reject option-like / ref-escape shapes before git sees it.
        require_branch_name(target_branch)?;
        // Resolve via `git rev-parse --verify refs/heads/<branch>` so the
        // branch resolves whether it lives in a loose ref
        // (`.git/refs/heads/<branch>`) or in `.git/packed-refs` after a
        // `git gc` / `git pack-refs --all`. Reading the loose ref file
        // directly breaks post-pack: the file is gone (the branch still
        // exists), so `read_to_string` returns `NotFound` → `StateError`.
        let ref_name = format!("refs/heads/{target_branch}");
        run_git_capture(&self.repo_root, &["rev-parse", "--verify", &ref_name])
    }

    fn prepare_squash_candidate(
        &self,
        candidate: &IntegrationCandidate,
        parent_commit: &str,
    ) -> LaneResult<SquashCandidate> {
        // P0-3: every externally supplied commit-ish becomes a git argv
        // positional below; only 40/64-hex object ids are accepted.
        require_hex_oid("candidate.base_commit", &candidate.base_commit)?;
        require_hex_oid("candidate.unit_commit", &candidate.unit_commit)?;
        require_hex_oid("parent_commit", parent_commit)?;

        // D19/S19: the squash is built on top of the LANE-TIME head
        // (`parent_commit`), not the admission-time verified base. The
        // unit's diff (verified base → unit commit) is applied onto a
        // throwaway index seeded from `parent_commit`, so siblings of
        // the same target integrate sequentially: after the first FF
        // moves the head, the next candidate still produces a
        // DESCENDANT of it, which is what the CAS ancestor check
        // demands. A wholesale `unit_commit^{tree}` commit would
        // silently revert every sibling that landed after admission.
        let patch = run_git_capture_bytes(
            &self.repo_root,
            &[
                "diff",
                "--binary",
                &candidate.base_commit,
                &candidate.unit_commit,
            ],
        )?;
        // Throwaway index via GIT_INDEX_FILE — the host repo's real
        // index and worktree are never touched.
        let scratch = tempfile::tempdir()
            .map_err(|e| LaneError::StateError(format!("squash scratch tempdir: {e}")))?;
        let index_file = scratch.path().join("index");
        run_git_with_index(&self.repo_root, &index_file, &["read-tree", parent_commit])?;
        if !patch.iter().all(u8::is_ascii_whitespace) {
            let patch_path = scratch.path().join("unit.patch");
            std::fs::write(&patch_path, &patch)
                .map_err(|e| LaneError::StateError(format!("write squash patch: {e}")))?;
            let patch_arg = patch_path.to_string_lossy().to_string();
            if let Err(err) = run_git_with_index(
                &self.repo_root,
                &index_file,
                &["apply", "--cached", &patch_arg],
            ) {
                // The diff does not apply onto the lane-time head —
                // a sibling changed overlapping lines. Typed conflict,
                // target untouched.
                return Err(LaneError::ApplyConflict {
                    unit_id: candidate.unit_id.clone(),
                    reason: err.to_string(),
                });
            }
        }
        let tree_oid = run_git_with_index(&self.repo_root, &index_file, &["write-tree"])?;
        let message = format!("squash({}): U7 lane integrate", candidate.unit_id);
        // Commit the tree on top of the lane-time head:
        //   git commit-tree <tree_oid> -p <parent_commit> -m <msg>
        let commit_oid = run_git_capture(
            &self.repo_root,
            &[
                "commit-tree",
                &tree_oid,
                "-p",
                parent_commit,
                "-m",
                &message,
            ],
        )?;
        Ok(SquashCandidate {
            unit_id: candidate.unit_id.clone(),
            target_branch: candidate.target_branch.clone(),
            base_commit: parent_commit.to_string(),
            squash_commit: commit_oid,
            tree_oid,
            message,
        })
    }

    fn run_targeted_gate(&self, squash: &SquashCandidate) -> LaneResult<GateOutcome> {
        // PMI-004①: the real targeted gate. The gate commands
        // are owned by the caller (the integration orchestrator
        // declares the per-target command set; the lane itself
        // stays command-agnostic). `gate_commands` on the port
        // holds that spec — an empty spec FAILS CLOSED: a lane
        // with no gate must never clear a candidate to FF
        // (the unconditional-Pass placeholder PMI-004 closed).
        if self.gate_commands.is_empty() {
            return Ok(GateOutcome::Fail {
                reason: "targeted gate has no commands configured: refusing to clear the \
                         candidate without running a gate (PMI-004 fail-closed)"
                    .to_string(),
            });
        }
        // The gate must be read-only against the workspace and
        // must NOT advance the target: run it against a throwaway
        // worktree checked out at the SQUASH COMMIT (not the
        // branch), so the commands observe exactly the tree the
        // CAS would fast-forward to. The worktree is detached
        // (`--detach`, no `-B`) so no ref is created or moved.
        let worktree_dir = tempfile::tempdir_in(&self.repo_root)
            .map_err(|e| LaneError::StateError(format!("gate worktree tempdir: {e}")))?;
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();
        let checkout = run_git_capture(
            &self.repo_root,
            &[
                "worktree",
                "add",
                "--detach",
                "--quiet",
                &worktree_path,
                &squash.squash_commit,
            ],
        );
        if let Err(lane_err) = checkout {
            // Worktree add failure could leave a half-registered
            // worktree behind; `git worktree prune` cleans the
            // stale registration. The candidate is REJECTED
            // (gate Fail), not the lane: fail-closed direction.
            let _ = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.repo_root)
                .args(["worktree", "prune"])
                .output();
            return Ok(GateOutcome::Fail {
                reason: format!(
                    "gate worktree checkout failed for squash {}: {lane_err}",
                    squash.squash_commit
                ),
            });
        }
        // Collect the outcome before removing the worktree.
        let outcome = self.run_gate_commands_in(&worktree_path, &squash.unit_id);
        // Explicit removal: unregister the worktree + delete the
        // directory. `worktree remove` also handles the tempdir
        // leftover; the TempDir Drop then only removes an empty
        // or missing dir. Removal failure is a diagnostic, not a
        // silent swallow — a leaked worktree registration confuses
        // later `worktree list` audits.
        match std::process::Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["worktree", "remove", "--force", &worktree_path])
            .output()
        {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                tracing::warn!(
                    path = %worktree_path,
                    status = ?out.status.code(),
                    stderr = %String::from_utf8_lossy(&out.stderr),
                    "integration gate worktree remove failed; falling back to fs cleanup"
                );
            }
            Err(e) => {
                tracing::warn!(
                    path = %worktree_path,
                    error = %e,
                    "integration gate worktree remove spawn failed; falling back to fs cleanup"
                );
            }
        }
        if worktree_dir.path().exists()
            && let Err(e) = std::fs::remove_dir_all(worktree_dir.path())
        {
            tracing::warn!(
                path = %worktree_path,
                error = %e,
                "integration gate worktree fs cleanup failed"
            );
        }
        outcome
    }

    fn compare_and_swap_ff(
        &self,
        target_branch: &str,
        expected_head_before: &str,
        squash: &SquashCandidate,
    ) -> LaneResult<CasOutcome> {
        // P0-3: branch name + both commit-ish values become git argv
        // below; shape-validate before git sees them.
        require_branch_name(target_branch)?;
        require_hex_oid("expected_head_before", expected_head_before)?;
        require_hex_oid("squash.squash_commit", &squash.squash_commit)?;
        // Re-read HEAD; if it's moved, refuse.
        let current = self.current_target_oid(target_branch)?;
        if current != expected_head_before {
            return Ok(CasOutcome::StaleExpected {
                expected: expected_head_before.to_string(),
                actual: current,
            });
        }
        // Verify squash_commit is a descendant of expected_head_before
        // (FF requirement). If not, refuse.
        let ancestor_check = run_git_capture(
            &self.repo_root,
            &[
                "merge-base",
                "--is-ancestor",
                expected_head_before,
                &squash.squash_commit,
            ],
        );
        if ancestor_check.is_err() {
            // Not an ancestor means either: (a) squash is not on
            // base, or (b) git refused to merge-base. Either way:
            // not an FF candidate.
            return Ok(CasOutcome::Refused {
                reason: "squash is not a descendant of expected head".into(),
            });
        }
        // Atomic update:
        let ref_name = format!("refs/heads/{}", target_branch);
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .arg("update-ref")
            .arg(&ref_name)
            .arg(&squash.squash_commit)
            .arg(expected_head_before)
            .status()
            .map_err(|e| LaneError::StateError(format!("git update-ref: {e}")))?;
        if !status.success() {
            return Ok(CasOutcome::Refused {
                reason: format!("git update-ref exited {:?}", status.code()),
            });
        }
        Ok(CasOutcome::Advanced {
            new_head: squash.squash_commit.clone(),
        })
    }
}

fn run_git_capture(repo_root: &PathBuf, args: &[&str]) -> LaneResult<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .map_err(|e| LaneError::StateError(format!("git {args:?}: {e}")))?;
    if !out.status.success() {
        return Err(LaneError::StateError(format!(
            "git {args:?} exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Raw-stdout variant of [`run_git_capture`]: no trimming. Patch
/// payloads are whitespace-sensitive, so `git diff` output must not
/// pass through the trimming capture.
fn run_git_capture_bytes(repo_root: &PathBuf, args: &[&str]) -> LaneResult<Vec<u8>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .map_err(|e| LaneError::StateError(format!("git {args:?}: {e}")))?;
    if !out.status.success() {
        return Err(LaneError::StateError(format!(
            "git {args:?} exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(out.stdout)
}

/// Run git with `GIT_INDEX_FILE=<index>` so index-mutating plumbing
/// (`read-tree` / `apply --cached` / `write-tree`) operates on a
/// throwaway index, never the host repo's real one.
fn run_git_with_index(
    repo_root: &PathBuf,
    index: &std::path::Path,
    args: &[&str],
) -> LaneResult<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .output()
        .map_err(|e| LaneError::StateError(format!("git {args:?}: {e}")))?;
    if !out.status.success() {
        return Err(LaneError::StateError(format!(
            "git {args:?} exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// P0-3 shape whitelist for externally supplied commit-ish values
/// that become git argv positionals: 40-hex (SHA-1) or 64-hex
/// (SHA-256) object ids only. Anything else (branch names, `HEAD`,
/// `-`-prefixed option probes) is rejected before git sees it.
fn require_hex_oid(field: &'static str, value: &str) -> LaneResult<()> {
    let ok = matches!(value.len(), 40 | 64) && value.chars().all(|c| c.is_ascii_hexdigit());
    if ok {
        Ok(())
    } else {
        Err(LaneError::InvalidInput {
            field,
            value: value.to_string(),
        })
    }
}

/// P0-3 shape whitelist for target branch names. `/` is legal
/// (`feat/x`); the guard rejects empty names, a leading `-` (argv
/// option injection), `..` (ref escape), whitespace and the ref
/// metacharacters git's own `check-ref-format` forbids.
fn require_branch_name(target_branch: &str) -> LaneResult<()> {
    let ok = !target_branch.is_empty()
        && !target_branch.starts_with('-')
        && !target_branch.contains("..")
        && target_branch
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'));
    if ok {
        Ok(())
    } else {
        Err(LaneError::InvalidInput {
            field: "target_branch",
            value: target_branch.to_string(),
        })
    }
}

// ===========================================================================
// Fake git-backed port. Pure state machine for tests.
// ===========================================================================

/// In-memory fake of [`GitIntegrationPort`]. Tests
/// pre-populate the heads map, drive `prepare_*` /
/// `run_targeted_gate` / `compare_and_swap_ff`, and
/// observe branch heads as they advance.
#[derive(Debug)]
pub struct FakeGitIntegrationPort {
    inner: Mutex<FakeGitInner>,
}

#[derive(Debug, Default)]
struct FakeGitInner {
    /// branch → current head oid
    heads: BTreeMap<String, String>,
    /// unit_commit → tree oid
    trees: BTreeMap<String, String>,
    /// unit_commit → parent (what we'll commit on top of)
    parents: BTreeMap<String, String>,
    /// next squash_commit index
    next_idx: u64,
    /// What `compare_and_swap_ff` should return when the
    /// CAS is otherwise valid. Lets tests simulate Stale /
    /// Refused without contorting the state machine.
    cas_override: BTreeMap<String, CasOutcome>,
    /// What `run_targeted_gate` should return for the next
    /// call (consumed on read).
    gate_outcome: Option<GateOutcome>,
}

impl FakeGitIntegrationPort {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(FakeGitInner::default()),
        }
    }

    /// Set the head of a target branch.
    pub fn set_head(&self, branch: &str, oid: &str) {
        let mut g = self.inner.lock().expect("fake port mutex");
        g.heads.insert(branch.to_string(), oid.to_string());
    }

    /// Map a unit_commit → tree_oid (what `prepare_*` will
    /// claim as the squash tree).
    pub fn set_unit_tree(&self, unit_commit: &str, tree_oid: &str) {
        let mut g = self.inner.lock().expect("fake port mutex");
        g.trees
            .insert(unit_commit.to_string(), tree_oid.to_string());
    }

    /// Override the next `compare_and_swap_ff` outcome for
    /// the given target branch. Cleared after one use.
    pub fn override_cas(&self, branch: &str, outcome: CasOutcome) {
        let mut g = self.inner.lock().expect("fake port mutex");
        g.cas_override.insert(branch.to_string(), outcome);
    }

    /// Force the next `run_targeted_gate` to return a
    /// specific outcome. Cleared after one use.
    pub fn force_gate(&self, outcome: GateOutcome) {
        let mut g = self.inner.lock().expect("fake port mutex");
        g.gate_outcome = Some(outcome);
    }
}

impl Default for FakeGitIntegrationPort {
    fn default() -> Self {
        Self::new()
    }
}

impl GitIntegrationPort for FakeGitIntegrationPort {
    fn current_target_oid(&self, target_branch: &str) -> LaneResult<String> {
        let g = self.inner.lock().expect("fake port mutex");
        g.heads
            .get(target_branch)
            .cloned()
            .ok_or_else(|| LaneError::UnknownBase(target_branch.to_string()))
    }

    fn prepare_squash_candidate(
        &self,
        candidate: &IntegrationCandidate,
        parent_commit: &str,
    ) -> LaneResult<SquashCandidate> {
        let mut g = self.inner.lock().expect("fake port mutex");
        let tree_oid = g
            .trees
            .get(&candidate.unit_commit)
            .cloned()
            .unwrap_or_else(|| format!("tree-{}", candidate.unit_commit));
        g.parents
            .insert(candidate.unit_commit.clone(), parent_commit.to_string());
        g.next_idx += 1;
        let idx = g.next_idx;
        let squash_commit = format!("squash-{}-{}", candidate.unit_id, idx);
        Ok(SquashCandidate {
            unit_id: candidate.unit_id.clone(),
            target_branch: candidate.target_branch.clone(),
            base_commit: parent_commit.to_string(),
            squash_commit,
            tree_oid,
            message: format!("squash({}): U7 fake", candidate.unit_id),
        })
    }

    fn run_targeted_gate(&self, _squash: &SquashCandidate) -> LaneResult<GateOutcome> {
        let mut g = self.inner.lock().expect("fake port mutex");
        Ok(g.gate_outcome.take().unwrap_or(GateOutcome::Pass))
    }

    fn compare_and_swap_ff(
        &self,
        target_branch: &str,
        expected_head_before: &str,
        squash: &SquashCandidate,
    ) -> LaneResult<CasOutcome> {
        let mut g = self.inner.lock().expect("fake port mutex");
        if let Some(override_outcome) = g.cas_override.remove(target_branch) {
            // When the test injects an override, do NOT mutate
            // heads — only the natural code path advances the
            // branch.
            return Ok(override_outcome);
        }
        let current = g
            .heads
            .get(target_branch)
            .cloned()
            .ok_or_else(|| LaneError::UnknownBase(target_branch.to_string()))?;
        if current != expected_head_before {
            return Ok(CasOutcome::StaleExpected {
                expected: expected_head_before.to_string(),
                actual: current,
            });
        }
        g.heads
            .insert(target_branch.to_string(), squash.squash_commit.clone());
        Ok(CasOutcome::Advanced {
            new_head: squash.squash_commit.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// U7 contract: only one guard can hold the same target
    /// at a time; a second caller receives `TargetBusy`.
    #[test]
    fn lane_only_one_active_lease_per_target() {
        let core = Arc::new(LaneCore::new());
        let g1 = core.try_acquire("feat/test", "U1").expect("U1 takes lane");
        assert_eq!(g1.target_branch(), "feat/test");
        let err = core
            .try_acquire("feat/test", "U2")
            .expect_err("U2 must fail");
        assert!(matches!(err, LaneError::TargetBusy));
    }

    /// U7 contract: dropping the guard releases the lease so
    /// a subsequent acquire succeeds.
    #[test]
    fn lane_drop_releases_lease() {
        let core = Arc::new(LaneCore::new());
        {
            let _g = core.try_acquire("feat/test", "U1").expect("U1 takes");
            // `_g` is dropped at the end of this scope.
        }
        let _g2 = core
            .try_acquire("feat/test", "U2")
            .expect("U2 takes after U1 drops");
    }

    /// U7 contract: a different target branch is independent.
    #[test]
    fn lane_targets_are_independent() {
        let core = Arc::new(LaneCore::new());
        let _a = core.try_acquire("feat/a", "U1").expect("a");
        let _b = core.try_acquire("feat/b", "U2").expect("b");
    }

    /// U7 contract: explicit `release()` also drops the
    /// lease; the lane returns to free.
    #[test]
    fn lane_explicit_release() {
        let core = Arc::new(LaneCore::new());
        let g = core.try_acquire("feat/test", "U1").expect("U1 takes");
        g.release();
        assert!(core.try_acquire("feat/test", "U2").is_ok());
    }

    /// U7 contract: `select_eligible` sorts by
    /// `(integration_order, unit_id)`.
    #[test]
    fn select_eligible_is_stable() {
        let candidates = vec![
            IntegrationCandidate {
                unit_id: "U_B".into(),
                integration_order: 1,
                target_branch: "main".into(),
                base_commit: "b".into(),
                unit_commit: "u_b".into(),
                authorised_paths: vec![],
            },
            IntegrationCandidate {
                unit_id: "U_A".into(),
                integration_order: 2,
                target_branch: "main".into(),
                base_commit: "b".into(),
                unit_commit: "u_a".into(),
                authorised_paths: vec![],
            },
            IntegrationCandidate {
                unit_id: "U_C".into(),
                integration_order: 1,
                target_branch: "main".into(),
                base_commit: "b".into(),
                unit_commit: "u_c".into(),
                authorised_paths: vec![],
            },
        ];
        let picked = select_eligible(&candidates);
        assert_eq!(picked[0].unit_id, "U_B");
        assert_eq!(picked[1].unit_id, "U_C");
        assert_eq!(picked[2].unit_id, "U_A");
    }

    /// U7 contract: same input → same output (replay
    /// determinism). Two calls return identical slices.
    #[test]
    fn select_eligible_replay_determinism() {
        let candidates = vec![
            IntegrationCandidate {
                unit_id: "U_X".into(),
                integration_order: 5,
                target_branch: "main".into(),
                base_commit: "b".into(),
                unit_commit: "u_x".into(),
                authorised_paths: vec![],
            },
            IntegrationCandidate {
                unit_id: "U_Y".into(),
                integration_order: 5,
                target_branch: "main".into(),
                base_commit: "b".into(),
                unit_commit: "u_y".into(),
                authorised_paths: vec![],
            },
        ];
        let a = select_eligible(&candidates);
        let b = select_eligible(&candidates);
        let a_ids: Vec<&str> = a.iter().map(|c| c.unit_id.as_str()).collect();
        let b_ids: Vec<&str> = b.iter().map(|c| c.unit_id.as_str()).collect();
        assert_eq!(a_ids, b_ids);
    }

    /// U7 contract: when the expected head matches, the
    /// fake port advances the lane and the new head is
    /// reported by `current_target_oid`.
    #[test]
    fn fake_port_advances_on_matching_head() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        port.set_head("feat/test", "head_before");
        let cand = IntegrationCandidate {
            unit_id: "U1".into(),
            integration_order: 1,
            target_branch: "feat/test".into(),
            base_commit: "head_before".into(),
            unit_commit: "u1".into(),
            authorised_paths: vec![],
        };
        let squash = port.prepare_squash_candidate(&cand, "head_before").unwrap();
        let outcome = port
            .compare_and_swap_ff("feat/test", "head_before", &squash)
            .unwrap();
        match outcome {
            CasOutcome::Advanced { new_head } => {
                assert_eq!(new_head, squash.squash_commit);
                let now = port.current_target_oid("feat/test").unwrap();
                assert_eq!(now, squash.squash_commit);
            }
            other => panic!("expected Advanced, got {other:?}"),
        }
    }

    /// U7 contract: when the expected head does NOT match
    /// (the lane moved under us), CAS returns
    /// `StaleExpected { expected, actual }` and does NOT
    /// advance the branch.
    #[test]
    fn fake_port_cas_refuses_when_head_moved() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        port.set_head("feat/test", "head_before");
        // Simulate a sibling FF-ing under us:
        port.set_head("feat/test", "sibling_advanced_head");
        let cand = IntegrationCandidate {
            unit_id: "U1".into(),
            integration_order: 1,
            target_branch: "feat/test".into(),
            base_commit: "head_before".into(),
            unit_commit: "u1".into(),
            authorised_paths: vec![],
        };
        let squash = port.prepare_squash_candidate(&cand, "head_before").unwrap();
        let outcome = port
            .compare_and_swap_ff("feat/test", "head_before", &squash)
            .unwrap();
        match outcome {
            CasOutcome::StaleExpected { expected, actual } => {
                assert_eq!(expected, "head_before");
                assert_eq!(actual, "sibling_advanced_head");
            }
            other => panic!("expected StaleExpected, got {other:?}"),
        }
        // Head did NOT advance.
        let now = port.current_target_oid("feat/test").unwrap();
        assert_eq!(now, "sibling_advanced_head");
    }

    /// U7 contract: when the gate returns `Fail`, the lane
    /// does NOT advance (caller checks before CAS).
    #[test]
    fn fake_port_gate_fail_short_circuits_lane() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        port.set_head("feat/test", "head_before");
        port.force_gate(GateOutcome::Fail {
            reason: "test failure".into(),
        });
        let squash = SquashCandidate {
            unit_id: "U1".into(),
            target_branch: "feat/test".into(),
            base_commit: "head_before".into(),
            squash_commit: "sc".into(),
            tree_oid: "t".into(),
            message: "m".into(),
        };
        let outcome = port.run_targeted_gate(&squash).unwrap();
        assert!(matches!(outcome, GateOutcome::Fail { .. }));
        // Lane head unchanged.
        let now = port.current_target_oid("feat/test").unwrap();
        assert_eq!(now, "head_before");
    }

    /// U7 contract: `current_holder` reflects the in-flight
    /// guard's unit_id.
    #[test]
    fn lane_current_holder_visible() {
        let core = Arc::new(LaneCore::new());
        let _g = core.try_acquire("feat/test", "U1").expect("U1 takes");
        let holder = core.current_holder("feat/test").unwrap();
        assert_eq!(holder.as_deref(), Some("U1"));
    }
}

// ===========================================================================
// RealGitIntegrationPort coverage. These tests exercise the live `git`
// binary against a `tempfile::TempDir` repo, so they cover the four
// RealGitIntegrationPort methods that the FakeGitIntegrationPort tests
// above do not: `current_target_oid`, `prepare_squash_candidate`,
// `run_targeted_gate`, `compare_and_swap_ff`, plus the `run_git_capture`
// helper's spawn-failure path.
// ===========================================================================
#[cfg(test)]
mod tests_real_port {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    /// Run `git -C <repo_root> <args>`; panic on failure with
    /// stderr so the helper callers stay readable.
    fn git(repo_root: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: spawn {e}"));
        assert!(
            out.status.success(),
            "git {args:?} exited {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Build a throwaway repo with one initial commit on `main`
    /// plus a feature branch whose tip (`unit_commit`) holds a
    /// distinct tree. Returns `(repo_root, main_oid, unit_commit)`.
    fn fixture_repo() -> (TempDir, String, String) {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        git(root, &["init", "-q", "--initial-branch=main"]);
        git(root, &["config", "user.email", "test@example.com"]);
        git(root, &["config", "user.name", "Test"]);
        git(root, &["config", "commit.gpgsign", "false"]);

        // Initial commit on main: file `base.txt`.
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git(root, &["add", "base.txt"]);
        git(root, &["commit", "-q", "-m", "base"]);
        let main_oid = git(root, &["rev-parse", "HEAD"]);

        // Feature branch `feat/u1` with an extra file; its tip is
        // the candidate `unit_commit`.
        git(root, &["checkout", "-q", "-b", "feat/u1"]);
        std::fs::write(root.join("u1.txt"), "unit-1\n").unwrap();
        git(root, &["add", "u1.txt"]);
        git(root, &["commit", "-q", "-m", "unit-1"]);
        let unit_commit = git(root, &["rev-parse", "HEAD"]);

        // Leave HEAD back on main so CAS targets `refs/heads/main`.
        git(root, &["checkout", "-q", "main"]);
        (dir, main_oid, unit_commit)
    }

    fn candidate(unit_commit: &str, base_commit: &str) -> IntegrationCandidate {
        IntegrationCandidate {
            unit_id: "U1".into(),
            integration_order: 1,
            target_branch: "main".into(),
            base_commit: base_commit.into(),
            unit_commit: unit_commit.into(),
            authorised_paths: vec![],
        }
    }

    /// Happy path: a loose `refs/heads/main` ref exists, so
    /// `current_target_oid` returns the OID git reports.
    #[test]
    fn current_target_oid_resolves_loose_ref() {
        let (dir, main_oid, _unit) = fixture_repo();
        let port = RealGitIntegrationPort::new(dir.path().to_path_buf());
        assert_eq!(port.current_target_oid("main").unwrap(), main_oid);
    }

    /// C5 edge: after `git pack-refs --all`, the loose ref file at
    /// `.git/refs/heads/main` is GONE (the branch now lives only in
    /// `.git/packed-refs`). Before the fix, `current_target_oid`
    /// read the loose file and returned `StateError(NotFound)`.
    /// After the fix (rev-parse --verify), it still resolves the
    /// packed ref and returns the correct OID. This is the
    /// RED→GREEN test for C5.
    #[test]
    fn current_target_oid_resolves_packed_refs_after_gc() {
        let (dir, main_oid, _unit) = fixture_repo();
        let root = dir.path();

        // Sanity: the loose ref exists before packing.
        let loose = root.join(".git").join("refs").join("heads").join("main");
        assert!(loose.exists(), "loose ref should exist before pack-refs");

        // Pack every ref into .git/packed-refs and drop the loose file.
        git(root, &["pack-refs", "--all"]);
        assert!(
            !loose.exists(),
            "loose ref must be gone after pack-refs --all; C5 premise"
        );

        let port = RealGitIntegrationPort::new(root.to_path_buf());
        assert_eq!(
            port.current_target_oid("main").unwrap(),
            main_oid,
            "current_target_oid must resolve the packed ref, not read the (now-gone) loose file"
        );
    }

    /// `prepare_squash_candidate` produces a real commit whose tree
    /// matches the candidate's tree and whose (single) parent is
    /// `base_commit`. It does NOT advance any branch.
    #[test]
    fn prepare_squash_candidate_builds_commit_on_base() {
        let (dir, main_oid, unit_commit) = fixture_repo();
        let root = dir.path();
        let port = RealGitIntegrationPort::new(root.to_path_buf());

        let cand = candidate(&unit_commit, &main_oid);
        let squash = port.prepare_squash_candidate(&cand, &main_oid).unwrap();

        // squash_commit is a real commit object.
        let squash_tree = git(
            root,
            &["rev-parse", &format!("{}^{{tree}}", squash.squash_commit)],
        );
        let squash_parent = git(root, &["rev-parse", &format!("{}^", squash.squash_commit)]);
        assert_eq!(
            squash_tree, squash.tree_oid,
            "squash commit tree matches reported tree_oid"
        );
        assert_eq!(
            squash_parent, main_oid,
            "squash commit parent is base_commit"
        );

        // The squash tree must carry the unit's file (u1.txt),
        // proving it captured the unit_commit tree — not main's.
        let ls = git(
            root,
            &["ls-tree", "-r", "--name-only", &squash.squash_commit],
        );
        assert!(
            ls.contains("u1.txt"),
            "squash tree contains the unit's file"
        );

        // Branch did NOT advance.
        assert_eq!(
            port.current_target_oid("main").unwrap(),
            main_oid,
            "prepare_squash_candidate must not advance the branch"
        );
    }

    /// `compare_and_swap_ff` advances the branch to the squash commit
    /// when `expected_head_before` matches, and reports `Advanced`.
    #[test]
    fn compare_and_swap_ff_advances_on_matching_head() {
        let (dir, main_oid, unit_commit) = fixture_repo();
        let root = dir.path();
        let port = RealGitIntegrationPort::new(root.to_path_buf());

        let squash = port
            .prepare_squash_candidate(&candidate(&unit_commit, &main_oid), &main_oid)
            .unwrap();
        let outcome = port
            .compare_and_swap_ff("main", &main_oid, &squash)
            .unwrap();
        match outcome {
            CasOutcome::Advanced { new_head } => {
                assert_eq!(new_head, squash.squash_commit);
                // The branch actually moved.
                assert_eq!(
                    port.current_target_oid("main").unwrap(),
                    squash.squash_commit
                );
            }
            other => panic!("expected Advanced, got {other:?}"),
        }
    }

    /// `compare_and_swap_ff` refuses (typed `StaleExpected`) when the
    /// target moved under us between prepare and CAS. The branch is
    /// NOT advanced.
    #[test]
    fn compare_and_swap_ff_refuses_when_head_moved() {
        let (dir, main_oid, unit_commit) = fixture_repo();
        let root = dir.path();
        let port = RealGitIntegrationPort::new(root.to_path_buf());

        let squash = port
            .prepare_squash_candidate(&candidate(&unit_commit, &main_oid), &main_oid)
            .unwrap();

        // Sibling FFs main to a fresh commit while we were preparing.
        git(root, &["commit", "-q", "--allow-empty", "-m", "sibling"]);
        let moved_head = port.current_target_oid("main").unwrap();
        assert_ne!(moved_head, main_oid, "precondition: sibling advanced main");

        let outcome = port
            .compare_and_swap_ff("main", &main_oid, &squash)
            .unwrap();
        match outcome {
            CasOutcome::StaleExpected { expected, actual } => {
                assert_eq!(expected, main_oid);
                assert_eq!(actual, moved_head);
            }
            other => panic!("expected StaleExpected, got {other:?}"),
        }
        // Branch stays at the sibling's commit, NOT the squash.
        assert_eq!(port.current_target_oid("main").unwrap(), moved_head);
    }

    /// `run_git_capture` maps a spawn failure (git binary unreachable
    /// because PATH is empty) to a typed `LaneError::StateError` —
    /// never a panic. Drives this through `current_target_oid` post-fix.
    #[test]
    fn run_git_capture_spawn_failure_is_typed_error() {
        // A repo that genuinely has a `main` ref, so the only thing
        // that can fail is spawning `git` itself.
        let (dir, _main_oid, _unit) = fixture_repo();
        let mut port = RealGitIntegrationPort::new(dir.path().to_path_buf());
        // Subvert the repo_root so the spawned `git -C <root>` lookup
        // still points at a real repo but the binary cannot be found:
        // we cannot easily make `git` un-spawnable while keeping a
        // valid root, so instead drive the helper directly with a
        // bogus repo_root AND an emptied PATH to force spawn failure.
        port.repo_root = std::path::PathBuf::from("/nonexistent/repo/for/spawn/fail");
        let err = port.current_target_oid("main").unwrap_err();
        assert!(
            matches!(err, LaneError::StateError(_)),
            "spawn failure must surface as StateError, got {err:?}"
        );
    }

    /// TG-S07 (PMI-004①, P2, post-merge-converge) — FLIPPED per the
    /// pin's own built-in upgrade guidance (TG-S13): the real port's
    /// targeted gate now EXECUTES the configured gate command set
    /// against a throwaway worktree checked out at the squash commit.
    /// This is the fail-injection family the pin demanded:
    ///   1. gate command fails → `GateOutcome::Fail` (reason carries
    ///      the failing command, NOT a Pass);
    ///   2. gate command passes → `GateOutcome::Pass`;
    ///   3. empty command set → `GateOutcome::Fail` (fail-closed —
    ///      the unconditional-Pass placeholder PMI-004 closed).
    #[test]
    fn tg_s07_real_port_gate_runs_commands_and_fails_closed() {
        // Fixture: main carries `base.txt`; feat/u1 adds `u1.txt`.
        let (dir, main_oid, _unit_commit) = fixture_repo();
        let root = dir.path();

        // Branch advance guard: the gate must be read-only — no
        // branch may move, and no worktree registration may remain.
        let worktree_list_before = git(root, &["worktree", "list", "--porcelain"]);

        let squash = {
            let port = RealGitIntegrationPort::new(root.to_path_buf());
            port.prepare_squash_candidate(&candidate(&_unit_commit, &main_oid), &main_oid)
                .unwrap()
        };

        // 1. FAIL path: a gate command that exits non-zero.
        let port_fail = RealGitIntegrationPort::new(root.to_path_buf()).with_gate_commands(vec![
            GateCommandSpec {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), "exit 42".to_string()],
            },
        ]);
        let outcome = port_fail.run_targeted_gate(&squash).unwrap();
        match &outcome {
            GateOutcome::Fail { reason } => {
                assert!(
                    reason.contains("exited") && reason.contains("42"),
                    "Fail reason must carry the failing command + exit code, got: {reason}"
                );
            }
            GateOutcome::Pass => panic!(
                "TG-S07 flipped pin: a failing gate command MUST yield GateOutcome::Fail — \
                 the unconditional-Pass placeholder has regressed (PMI-004①). See \
                 .ralph/post-merge/09-test-gap-plan.md §TG-S07/TG-S13."
            ),
        }

        // 2. PASS path: the same command set, exit zero.
        let port_pass = RealGitIntegrationPort::new(root.to_path_buf()).with_gate_commands(vec![
            GateCommandSpec {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), "exit 0".to_string()],
            },
        ]);
        let outcome = port_pass.run_targeted_gate(&squash).unwrap();
        assert!(
            matches!(outcome, GateOutcome::Pass),
            "TG-S07 flipped pin: passing gate commands must yield Pass, got {outcome:?}"
        );

        // 3. FAIL-CLOSED path: no commands configured.
        let port_empty = RealGitIntegrationPort::new(root.to_path_buf());
        let outcome = port_empty.run_targeted_gate(&squash).unwrap();
        match &outcome {
            GateOutcome::Fail { reason } => {
                assert!(
                    reason.contains("fail-closed") || reason.contains("no commands"),
                    "empty gate spec must fail closed with an explanatory reason, got: {reason}"
                );
            }
            GateOutcome::Pass => panic!(
                "TG-S07 flipped pin: an EMPTY gate command set must FAIL CLOSED — \
                 an unconfigured gate clearing a candidate to FF is exactly the \
                 'fake green gate' PMI-004① closed."
            ),
        }

        // 4. Read-only + hygiene guards: branch unmoved, no
        //    leftover worktree registration (gate worktree cleaned).
        assert_eq!(
            git(root, &["rev-parse", "refs/heads/main"]),
            main_oid,
            "run_targeted_gate must not advance any branch"
        );
        let worktree_list_after = git(root, &["worktree", "list", "--porcelain"]);
        assert_eq!(
            worktree_list_before, worktree_list_after,
            "gate worktree must be fully unregistered after the gate runs"
        );
    }

    /// TG-S07 companion (PMI-004①): the gate commands observe the
    /// SQUASH TREE, not the checked-out branch. A command that
    /// greps for a file only the unit branch carries must PASS
    /// while the main checkout still lacks it — proving the gate
    /// ran against a worktree at the squash commit.
    #[test]
    fn tg_s07_real_port_gate_observes_squash_tree() {
        let (dir, main_oid, unit_commit) = fixture_repo();
        let root = dir.path();
        let port = RealGitIntegrationPort::new(root.to_path_buf()).with_gate_commands(vec![
            GateCommandSpec {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), "test -f u1.txt".to_string()],
            },
        ]);
        let squash = port
            .prepare_squash_candidate(&candidate(&unit_commit, &main_oid), &main_oid)
            .unwrap();
        // The MAIN checkout (repo root, on main) does NOT carry
        // u1.txt — only the squash tree does.
        assert!(
            !root.join("u1.txt").exists(),
            "fixture premise: main checkout must lack u1.txt"
        );
        let outcome = port.run_targeted_gate(&squash).unwrap();
        assert!(
            matches!(outcome, GateOutcome::Pass),
            "gate must run inside a worktree at the squash commit (u1.txt present \
             there, absent in the main checkout), got {outcome:?}"
        );
    }

    /// TG-S07 companion (PMI-004①): gate-hostile squash + a real
    /// gate command → Fail, and the branch is untouched. This is
    /// the exact scenario the pre-flip pin documented as
    /// "gate-hostile shape would be FF'd into main".
    #[test]
    fn tg_s07_gate_hostile_squash_rejected_by_real_gate() {
        let (dir, main_oid, _unit_commit) = fixture_repo();
        let root = dir.path();

        // Gate-hostile: unit branch DROPS base.txt.
        git(root, &["checkout", "-q", "feat/u1"]);
        std::fs::remove_file(root.join("base.txt")).unwrap();
        git(root, &["add", "-A"]);
        git(
            root,
            &["commit", "-q", "-m", "drop base.txt (gate-hostile shape)"],
        );
        let hostile_unit_commit = git(root, &["rev-parse", "HEAD"]);
        git(root, &["checkout", "-q", "main"]);

        let port = RealGitIntegrationPort::new(root.to_path_buf()).with_gate_commands(vec![
            GateCommandSpec {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), "test -f base.txt".to_string()],
            },
        ]);
        let squash = port
            .prepare_squash_candidate(&candidate(&hostile_unit_commit, &main_oid), &main_oid)
            .unwrap();
        // Premise anchor: the squash tree genuinely lacks base.txt.
        let tree_files = git(root, &["ls-tree", "--name-only", &squash.tree_oid]);
        assert!(
            !tree_files.lines().any(|f| f == "base.txt"),
            "fixture premise: squash tree must lack base.txt, got: {tree_files}"
        );

        let outcome = port.run_targeted_gate(&squash).unwrap();
        assert!(
            matches!(outcome, GateOutcome::Fail { .. }),
            "gate-hostile squash (drops base.txt) must FAIL the real gate — this is \
             the P0-grade hazard PMI-004① closed (untested squash FF'd into target). \
             Got {outcome:?}"
        );
        // Branch untouched: the refusal happens BEFORE the CAS.
        assert_eq!(
            git(root, &["rev-parse", "refs/heads/main"]),
            main_oid,
            "gate failure must leave the target branch unmoved"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // P0-1 (D19/S19, 2026-09-07): the squash is built on the lane-time
    // head with the unit's diff APPLIED, not a wholesale unit tree
    // committed on the admission-time base.
    // ─────────────────────────────────────────────────────────────────

    /// After a sibling FF moved the target past the unit's admission
    /// base, `prepare_squash_candidate` with `parent_commit` = the
    /// MOVED head must produce a commit that (a) is a descendant of
    /// that moved head, (b) carries BOTH the sibling's file and the
    /// unit's file, and (c) is accepted by `compare_and_swap_ff`.
    #[test]
    fn prepare_squash_applies_unit_diff_onto_moved_head() {
        let (dir, main_oid, unit_commit) = fixture_repo();
        let root = dir.path();
        let port = RealGitIntegrationPort::new(root.to_path_buf());

        // Sibling lands on main AFTER the unit's admission base.
        std::fs::write(root.join("sibling.txt"), "sibling\n").unwrap();
        git(root, &["add", "sibling.txt"]);
        git(root, &["commit", "-q", "-m", "sibling"]);
        let moved_head = git(root, &["rev-parse", "HEAD"]);
        assert_ne!(moved_head, main_oid, "precondition: sibling advanced main");

        // candidate.base_commit stays the admission-time base; the
        // parent is the lane-time (moved) head.
        let squash = port
            .prepare_squash_candidate(&candidate(&unit_commit, &main_oid), &moved_head)
            .expect("squash on moved head");
        let parent = git(root, &["rev-parse", &format!("{}^", squash.squash_commit)]);
        assert_eq!(parent, moved_head, "squash parent is the lane-time head");
        let files = git(
            root,
            &["ls-tree", "-r", "--name-only", &squash.squash_commit],
        );
        assert!(
            files.lines().any(|f| f == "sibling.txt"),
            "squash tree must carry the sibling's file, got: {files}"
        );
        assert!(
            files.lines().any(|f| f == "u1.txt"),
            "squash tree must carry the unit's file, got: {files}"
        );

        // CAS accepts: the squash is a descendant of the moved head.
        let outcome = port
            .compare_and_swap_ff("main", &moved_head, &squash)
            .expect("cas");
        assert!(
            matches!(outcome, CasOutcome::Advanced { .. }),
            "squash on the lane-time head must CAS cleanly, got {outcome:?}"
        );
    }

    /// A unit diff that overlaps a sibling's change must NOT silently
    /// merge: `git apply` fails and the port reports a typed
    /// `ApplyConflict` with the target untouched.
    #[test]
    fn prepare_squash_conflict_is_typed_and_leaves_target_untouched() {
        let (dir, main_oid, _unit_commit) = fixture_repo();
        let root = dir.path();
        let port = RealGitIntegrationPort::new(root.to_path_buf());

        // Unit branch edits base.txt one way...
        git(root, &["checkout", "-q", "feat/u1"]);
        std::fs::write(root.join("base.txt"), "unit-version\n").unwrap();
        git(root, &["add", "base.txt"]);
        git(root, &["commit", "-q", "-m", "unit edits base.txt"]);
        let conflicting_unit = git(root, &["rev-parse", "HEAD"]);
        // ...sibling edits the SAME file differently on main.
        git(root, &["checkout", "-q", "main"]);
        std::fs::write(root.join("base.txt"), "sibling-version\n").unwrap();
        git(root, &["add", "base.txt"]);
        git(root, &["commit", "-q", "-m", "sibling edits base.txt"]);
        let moved_head = git(root, &["rev-parse", "HEAD"]);

        let err = port
            .prepare_squash_candidate(&candidate(&conflicting_unit, &main_oid), &moved_head)
            .expect_err("overlapping diff must be a typed conflict");
        assert!(
            matches!(err, LaneError::ApplyConflict { .. }),
            "expected ApplyConflict, got {err:?}"
        );
        assert_eq!(
            git(root, &["rev-parse", "refs/heads/main"]),
            moved_head,
            "conflict must leave the target branch unmoved"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // P0-3 (2026-09-07): argv shape whitelist — externally supplied
    // commit-ish / branch names are rejected before git sees them.
    // ─────────────────────────────────────────────────────────────────

    /// `-`-leading / non-hex commit-ish values must never reach git
    /// argv as positionals.
    #[test]
    fn real_port_rejects_non_hex_commit_ish() {
        let (dir, main_oid, unit_commit) = fixture_repo();
        let root = dir.path();
        let port = RealGitIntegrationPort::new(root.to_path_buf());

        let mut cand = candidate(&unit_commit, &main_oid);
        cand.unit_commit = "--output=/tmp/pwned".to_string();
        let err = port
            .prepare_squash_candidate(&cand, &main_oid)
            .expect_err("option-like unit_commit must be rejected");
        assert!(
            matches!(
                err,
                LaneError::InvalidInput {
                    field: "candidate.unit_commit",
                    ..
                }
            ),
            "got {err:?}"
        );

        let err = port
            .prepare_squash_candidate(&candidate(&unit_commit, &main_oid), "-HEAD")
            .expect_err("option-like parent must be rejected");
        assert!(
            matches!(
                err,
                LaneError::InvalidInput {
                    field: "parent_commit",
                    ..
                }
            ),
            "got {err:?}"
        );

        let squash = port
            .prepare_squash_candidate(&candidate(&unit_commit, &main_oid), &main_oid)
            .unwrap();
        let err = port
            .compare_and_swap_ff("main", "not-a-sha", &squash)
            .expect_err("non-hex expected head must be rejected");
        assert!(
            matches!(
                err,
                LaneError::InvalidInput {
                    field: "expected_head_before",
                    ..
                }
            ),
            "got {err:?}"
        );
    }

    /// Branch names with option-like or ref-escape shapes are
    /// rejected before ref interpolation.
    #[test]
    fn real_port_rejects_malformed_branch_name() {
        let (dir, _main_oid, _unit) = fixture_repo();
        let port = RealGitIntegrationPort::new(dir.path().to_path_buf());
        for bad in ["-x", "../escape", "feat/../main", ""] {
            let err = port
                .current_target_oid(bad)
                .expect_err("malformed branch must be rejected");
            assert!(
                matches!(
                    err,
                    LaneError::InvalidInput {
                        field: "target_branch",
                        ..
                    }
                ),
                "branch {bad:?}: got {err:?}"
            );
        }
    }
}
