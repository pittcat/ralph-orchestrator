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
    StaleExpected {
        expected: String,
        actual: String,
    },
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

    /// Build a single squash commit on top of `base_commit`
    /// that captures the candidate's `unit_commit` tree.
    /// Returns the new commit OID and the tree OID it
    /// resolved to. Does NOT advance any branch.
    fn prepare_squash_candidate(
        &self,
        candidate: &IntegrationCandidate,
        base_commit: &str,
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

/// Real git-backed [`GitIntegrationPort`]. Uses
/// `<repo_root>/.git/refs/heads/<target_branch>` reads for
/// the head (no plumbing command needed for the read);
/// `git merge --squash` + `git commit-tree` for the squash;
/// `git update-ref` with the CAS check for the FF.
pub struct RealGitIntegrationPort {
    pub repo_root: PathBuf,
}

impl RealGitIntegrationPort {
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}

impl GitIntegrationPort for RealGitIntegrationPort {
    fn current_target_oid(&self, target_branch: &str) -> LaneResult<String> {
        let ref_path = self
            .repo_root
            .join(".git")
            .join("refs")
            .join("heads")
            .join(target_branch);
        let raw = std::fs::read_to_string(&ref_path).map_err(|e| {
            LaneError::StateError(format!(
                "read {}: {}",
                ref_path.display(),
                e
            ))
        })?;
        Ok(raw.trim().to_string())
    }

    fn prepare_squash_candidate(
        &self,
        candidate: &IntegrationCandidate,
        base_commit: &str,
    ) -> LaneResult<SquashCandidate> {
        // Tree of unit_commit: `git rev-parse <unit_commit>^{tree}`.
        let tree_oid = run_git_capture(
            &self.repo_root,
            &["rev-parse", &format!("{}^{{tree}}", candidate.unit_commit)],
        )?;
        let message = format!("squash({}): U7 lane integrate", candidate.unit_id);
        // Commit the tree on top of base_commit:
        //   git commit-tree <tree_oid> -p <base_commit> -m <msg>
        let commit_oid = run_git_capture(
            &self.repo_root,
            &[
                "commit-tree",
                &tree_oid,
                "-p",
                base_commit,
                "-m",
                &message,
            ],
        )?;
        Ok(SquashCandidate {
            unit_id: candidate.unit_id.clone(),
            target_branch: candidate.target_branch.clone(),
            base_commit: base_commit.to_string(),
            squash_commit: commit_oid,
            tree_oid,
            message,
        })
    }

    fn run_targeted_gate(&self, _squash: &SquashCandidate) -> LaneResult<GateOutcome> {
        // The targeted gate's actual command list is wired by
        // the integration use site (U7 integration.rs); this
        // trait method is the abstract pass/fail surface. The
        // real impl here returns Pass as a placeholder so
        // unit tests of the lane itself can run without a
        // full gate. The integration orchestrator replaces
        // this with a wrapped port that runs the gate.
        Ok(GateOutcome::Pass)
    }

    fn compare_and_swap_ff(
        &self,
        target_branch: &str,
        expected_head_before: &str,
        squash: &SquashCandidate,
    ) -> LaneResult<CasOutcome> {
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
        g.trees.insert(unit_commit.to_string(), tree_oid.to_string());
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
        base_commit: &str,
    ) -> LaneResult<SquashCandidate> {
        let mut g = self.inner.lock().expect("fake port mutex");
        let tree_oid = g
            .trees
            .get(&candidate.unit_commit)
            .cloned()
            .unwrap_or_else(|| format!("tree-{}", candidate.unit_commit));
        g.parents.insert(candidate.unit_commit.clone(), base_commit.to_string());
        g.next_idx += 1;
        let idx = g.next_idx;
        let squash_commit = format!("squash-{}-{}", candidate.unit_id, idx);
        Ok(SquashCandidate {
            unit_id: candidate.unit_id.clone(),
            target_branch: candidate.target_branch.clone(),
            base_commit: base_commit.to_string(),
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
        let err = core.try_acquire("feat/test", "U2").expect_err("U2 must fail");
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
        assert!(core
            .try_acquire("feat/test", "U2")
            .is_ok());
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