// U7's bin-side public surface is consumed by U8 (correction
// wiring) and U10 (preset cutover). Until then, the
// orchestrator is reachable from tests but not from the
// runtime driver, which is the expected transitional state.
#![allow(dead_code)]

//! 2026-09-03-0959 plan U7 (R7; S8-S11; D7-D9; E10-E12):
//! per-Unit integration orchestrator.
//!
//! This module is the U7 glue between:
//!   - [`super::worktree::UnitWorktree`] (per-Unit trusted
//!     worktree bound to a verified base commit), and
//!   - the lane + git port types in
//!     `ralph_core::supervisor::integration_lane`
//!     (`IntegrationLane`, `RealGitIntegrationPort`,
//!     `FakeGitIntegrationPort`), and
//!   - `ralph_core::supervisor::changed_path_guard::ChangedPathSet`
//!     (the second authorisation check the lane demands).
//!
//! # End-to-end flow (orchestrator, not the lane)
//!
//! ```text
//! candidate_input (unit_id, target_branch, base_commit,
//!                  unit_commit, integration_order,
//!                  authorised_paths)
//!        │
//!        ▼
//! 1. select_eligible   — sort (integration_order, unit_id)
//!        │
//!        ▼
//! 2. gate_candidate    — second changed-path check using the
//!                        SAME allowlist the reviewer used
//!        │
//!        ▼
//! 3. lane.try_acquire  — refuse if target busy
//!        │
//!        ▼
//! 4. port.current_target_oid
//! 5. port.prepare_squash_candidate(parent = lane-time head,
//!                        unit diff = base_commit → unit_commit)
//! 6. port.run_targeted_gate
//!        │   if Fail → drop guard → return LaneGateFailed
//!        ▼
//! 7. port.compare_and_swap_ff(expected_head_before)
//!        │   StaleExpected → drop guard → retry (caller
//!        │   responsibility); Refused → fail candidate
//!        ▼
//! 8. integration_store.record_integrated (idempotent)
//! 9. lane_guard.release() (or Drop)
//! ```
//!
//! The orchestrator owns no git state — every git call goes
//! through the port. The Fake variant of the port is what
//! the orchestrator tests use to simulate the hostile-agent
//! race (target moves between read and CAS).

use std::path::PathBuf;
use std::sync::Arc;

use ralph_core::supervisor::changed_path_guard::{
    ChangedPathError, ChangedPathRejection, ChangedPathSet, DiffPathEntry,
};
// `FORBIDDEN_TOP_LEVEL_PREFIXES` is referenced only from the
// `tests` module below (bin compilation has no other use), so
// pull it in there directly to avoid an `unused_imports` warning
// at the bin target.
use ralph_core::supervisor::dag_integration::{
    IntegrationInput, IntegrationRecord, IntegrationStore,
};
#[cfg(feature = "supervisor-db")]
use ralph_core::supervisor::dag_store_rusqlite::RusqliteIntegrationStore;
use ralph_core::supervisor::integration_lane::{
    CasOutcome, GateCommandSpec, GateOutcome, GitIntegrationPort, IntegrationCandidate,
    IntegrationLane, LaneCore, LaneError, LaneGuard, RealGitIntegrationPort, select_eligible,
};

#[allow(unused_imports)]
use super::worktree::UnitWorktree;

/// What the orchestrator was given as input. The lane expects
/// its own [`IntegrationCandidate`]; this struct adds the
/// fields the lane doesn't know about (changed-path shape
/// metadata, the orchestrator's clock for `created_at_ms`).
#[derive(Debug, Clone)]
pub struct IntegrationRequest {
    pub unit_id: String,
    pub integration_order: u32,
    pub target_branch: String,
    pub base_commit: String,
    pub unit_commit: String,
    /// Diff entries from the integrator's `git diff-tree`
    /// (path + change status + symlink/submodule flags).
    /// Re-checked by the orchestrator against the lane
    /// allowlist at lock time; rename entries authorise BOTH
    /// source and target paths.
    pub changed_paths: Vec<DiffPathEntry>,
    /// Lane allowlist (e.g. `["crates/ralph-core",
    /// "crates/ralph-cli", ...]`). Re-checked at lock time.
    pub allowlist: Vec<PathBuf>,
    /// U8 (R18/D23/S18): the job's declared changed-path set.
    /// Every actual changed path must be `⊆ declared_paths`
    /// (bidirectional authorisation) as well as `⊆ allowlist`.
    /// Sourced from the job's declared changed-set. Re-checked
    /// at lock time alongside the allowlist.
    pub declared_paths: Vec<PathBuf>,
    pub created_at_ms: i64,
}

/// Final outcome returned to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationOutcome {
    /// Lane FF'd; integration record persisted.
    Integrated {
        record: IntegrationRecord,
        target_branch: String,
        new_head: String,
    },
    /// Targeted gate failed; lane refused to FF.
    GateFailed { reason: String },
    /// CAS refused because the target moved under us.
    StaleExpected { expected: String, actual: String },
    /// CAS refused for a non-racy reason (non-FF, dirty,
    /// plumbing error). The candidate is rejected.
    CasRefused { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntegrationError {
    #[error("changed-path parse error: {0}")]
    ChangedPathParse(#[from] ChangedPathError),
    #[error("second changed-path authorisation failed: {0}")]
    ChangedPathRejected(#[from] ChangedPathRejection),
    #[error("lane error: {0}")]
    Lane(#[from] LaneError),
    #[error("integration store error: {0}")]
    Store(#[from] ralph_core::supervisor::dag_integration::IntegrationStoreError),
}

/// Per-target integration orchestrator. Composes the lane +
/// port + integration store. Generic over the lane's repo
/// marker (Real/Fake) and the git port. The store is the
/// `IntegrationStore` TRAIT object so the durable (rusqlite)
/// and in-memory variants are interchangeable; `real_orchestrator`
/// wires the durable variant (PMI-006).
pub struct IntegrationOrchestrator<R, P>
where
    R: 'static,
    P: GitIntegrationPort + 'static,
{
    pub lane: Arc<IntegrationLane<R, P>>,
    pub store: Arc<dyn IntegrationStore>,
}

impl<R, P> IntegrationOrchestrator<R, P>
where
    R: 'static,
    P: GitIntegrationPort + 'static,
{
    pub fn new(lane: Arc<IntegrationLane<R, P>>, store: Arc<dyn IntegrationStore>) -> Self {
        Self { lane, store }
    }

    /// Sort candidates in stable `(integration_order,
    /// unit_id)` order. Exposed for callers that want to
    /// preview the eligibility order before locking any
    /// lane.
    pub fn select_eligible<'a>(
        &self,
        candidates: &'a [IntegrationCandidate],
    ) -> Vec<&'a IntegrationCandidate> {
        select_eligible(candidates)
    }

    /// Drive one Unit's integration end-to-end. See module
    /// docs for the 9-step flow.
    pub fn integrate(
        &self,
        req: IntegrationRequest,
    ) -> Result<IntegrationOutcome, IntegrationError> {
        // Step 2: re-authorise the changed-path set against
        // the same allowlist the reviewer used. The guard is
        // fail-closed: any forbidden top-level prefix,
        // symlink, submodule, or out-of-allowlist path
        // rejects the candidate.
        let set = ChangedPathSet::from_diff_entries(req.changed_paths.clone())?;
        let _authorised = set.is_clean_within(&req.allowlist, &req.declared_paths, &req.unit_id)?;

        // Step 3: build the lane's `IntegrationCandidate` and
        // acquire the per-target lease.
        let candidate = IntegrationCandidate {
            unit_id: req.unit_id.clone(),
            integration_order: req.integration_order,
            target_branch: req.target_branch.clone(),
            base_commit: req.base_commit.clone(),
            unit_commit: req.unit_commit.clone(),
            authorised_paths: req.allowlist.clone(),
        };
        let guard: LaneGuard<'_> = self
            .lane
            .core
            .try_acquire(&req.target_branch, &req.unit_id)?;

        // Step 4: read the current target HEAD. Capture it
        // for the CAS check below.
        let expected_head_before = self.lane.port.current_target_oid(&req.target_branch)?;

        // Step 5: build the squash candidate on top of the
        // LANE-TIME head (`expected_head_before`), not the
        // admission-time verified base (D19/S19). The port applies
        // the unit's diff (base_commit → unit_commit) onto the
        // lane-time head, so the squash is a descendant of it BY
        // CONSTRUCTION — siblings of the same target integrate
        // sequentially instead of CAS-refusing forever once the
        // first Unit moves the head. The verified base stays the
        // unit-diff anchor and is what `UnitWorktree::acquire`
        // still pins the worktree to (untrusted-base → Blocked).
        let squash = self
            .lane
            .port
            .prepare_squash_candidate(&candidate, &expected_head_before)?;

        // Step 6: targeted gate against the squash tree.
        let gate = self.lane.port.run_targeted_gate(&squash)?;
        if let GateOutcome::Fail { reason } = gate {
            // Drop guard, then return.
            guard.release();
            return Ok(IntegrationOutcome::GateFailed { reason });
        }

        // Step 6b (2026-09-03-0959 plan Step E2): persist the TESTED
        // candidate BEFORE moving the target ref. The intent row lets
        // recovery distinguish a pending CAS from a completed CAS
        // whose record write was lost; a replayed prepare for the
        // same unit/target with a different candidate fails closed in
        // the store layer.
        self.store.prepare_intent(
            &ralph_core::supervisor::dag_integration::IntegrationIntent {
                input: IntegrationInput {
                    unit_id: req.unit_id.clone(),
                    target_branch: req.target_branch.clone(),
                    base_commit: req.base_commit.clone(),
                    integrated_commit: squash.squash_commit.clone(),
                    expected_head_before: expected_head_before.clone(),
                    created_at_ms: req.created_at_ms,
                },
                unit_commit: req.unit_commit.clone(),
                tree_oid: squash.tree_oid.clone(),
            },
        )?;

        // Step 7: CAS FF. The lane refuses to advance the
        // target if the head moved between read and CAS.
        let cas = self.lane.port.compare_and_swap_ff(
            &req.target_branch,
            &expected_head_before,
            &squash,
        )?;
        let new_head = match cas {
            CasOutcome::Advanced { new_head } => new_head,
            CasOutcome::StaleExpected { expected, actual } => {
                guard.release();
                return Ok(IntegrationOutcome::StaleExpected { expected, actual });
            }
            CasOutcome::Refused { reason } => {
                guard.release();
                return Ok(IntegrationOutcome::CasRefused { reason });
            }
        };

        // Step 8: persist the integration record. The
        // store's natural-key tuple makes this idempotent.
        let record = self.store.record_integrated(&IntegrationInput {
            unit_id: req.unit_id.clone(),
            target_branch: req.target_branch.clone(),
            base_commit: req.base_commit.clone(),
            integrated_commit: squash.squash_commit.clone(),
            expected_head_before: expected_head_before.clone(),
            created_at_ms: req.created_at_ms,
        })?;

        // Step 9: release the lane lease. Drop would do
        // this too; we call it explicitly so the test code
        // can assert the lane is free before returning.
        guard.release();

        Ok(IntegrationOutcome::Integrated {
            record,
            target_branch: req.target_branch,
            new_head,
        })
    }
}

/// Convenience constructor for the live, git-backed
/// orchestrator. The `repo_root` is what every git call
/// resolves against.
///
/// PMI-006: the integration store is the DURABLE rusqlite
/// variant persisted at `<repo_root>/.ralph/dag.db` (override
/// with `RALPH_DAG_STORE_PATH`). Integration records survive
/// process restarts; a reopened orchestrator replays the same
/// rows (idempotent natural-key semantics) instead of cold
/// starting. Store-open failure fails closed — the caller sees
/// the typed error, never a silent fallback to the in-memory
/// variant (that would resurrect the process-local store
/// PMI-006 closed).
///
/// PMI-004①: `gate_commands` is the per-target gate command set
/// the real port executes against the squash tree. **Empty list
/// fails closed**: the orchestrator a caller gets from this
/// constructor never carries the unconditional-Pass placeholder
/// — an unconfigured gate rejects candidates at Step 6 instead
/// of clearing them to FF. Declare a real command set (e.g. the
/// project's targeted test subset) here.
pub fn real_orchestrator(
    repo_root: PathBuf,
    gate_commands: Vec<GateCommandSpec>,
) -> Result<Arc<IntegrationOrchestrator<RealRepo, RealGitIntegrationPort>>, DagStoreOpenError> {
    let core = shared_lane_core(&repo_root);
    let port =
        Arc::new(RealGitIntegrationPort::new(repo_root.clone()).with_gate_commands(gate_commands));
    let lane = Arc::new(IntegrationLane::<RealRepo, _>::new(core, port));
    let db_path = dag_store_path(&repo_root);
    #[cfg(feature = "supervisor-db")]
    let store: Arc<dyn IntegrationStore> = {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| DagStoreOpenError {
                path: db_path.clone(),
                source_msg: format!("create parent dir: {err}"),
            })?;
        }
        Arc::new(
            RusqliteIntegrationStore::open(&db_path).map_err(|err| DagStoreOpenError {
                path: db_path.clone(),
                source_msg: err.to_string(),
            })?,
        )
    };
    #[cfg(not(feature = "supervisor-db"))]
    let store: Arc<dyn IntegrationStore> = {
        // Fail closed, mirroring build_supervisor_bridge: a build
        // without the `supervisor-db` feature must not silently
        // degrade the durable promise to the process-local store.
        let _ = db_path;
        return Err(DagStoreOpenError {
            path: db_path,
            source_msg: "supervisor-db cargo feature is off in this build; rebuild \
                     ralph-cli with --features supervisor-db (or the default \
                     features) to use the durable DAG integration store"
                .to_string(),
        });
    };
    Ok(Arc::new(IntegrationOrchestrator::new(lane, store)))
}

/// Typed error for a failed durable DAG store open. Fail-closed
/// surface: the caller decides how to surface it; no silent
/// fallback.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("durable DAG store unavailable at {path}: {source_msg}")]
pub struct DagStoreOpenError {
    pub path: PathBuf,
    pub source_msg: String,
}

/// Process-wide lane registry (P1, 2026-09-07): the `LaneCore` is
/// shared per repo_root across every `real_orchestrator` instance
/// in this process, so two orchestrators targeting the same branch
/// of the same repo serialise on the lease instead of racing.
/// `LaneCore` keys its holders per target branch internally, which
/// makes the effective exclusion domain (repo_root, target_branch).
///
/// Cross-PROCESS mutual exclusion is intentionally out of scope
/// here: the CAS on `expected_head_before` is the fail-closed
/// backstop for a second OS process advancing the same target
/// (tracked as PMI-018). This registry closes only the in-process
/// hole where each `real_orchestrator()` call used to mint a fresh
/// `LaneCore`.
fn shared_lane_core(repo_root: &std::path::Path) -> Arc<LaneCore> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<LaneCore>>>> = OnceLock::new();
    // Canonicalise so `repo/` and `./repo` resolve to one lane;
    // fall back to the raw path when the dir does not exist yet.
    let key = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
    let mut map = REGISTRY
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.entry(key)
        .or_insert_with(|| Arc::new(LaneCore::new()))
        .clone()
}

/// Resolve the durable DAG store path for `repo_root`:
/// `RALPH_DAG_STORE_PATH` (absolute) when set, else
/// `<repo_root>/.ralph/dag.db`. The DAG state family deliberately
/// keeps its own file: the wave supervisor store
/// (`.ralph/supervisor.db`) stays the single wave authority
/// (04 audit); the DAG scheduler owns its tables in its own
/// database so neither store's migrations or lock window wedges
/// the other.
pub fn dag_store_path(repo_root: &std::path::Path) -> PathBuf {
    std::env::var("RALPH_DAG_STORE_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root.join(".ralph").join("dag.db"))
}

/// Real-repo marker for the orchestrator's generic param.
/// (`ralph_core::supervisor::integration_lane::RealRepo`
/// is re-exported here for convenience — the orchestrator's
/// public surface hides the supervisor module path.)
pub use ralph_core::supervisor::integration_lane::RealRepo;

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use ralph_core::supervisor::changed_path_guard::{
        DiffPathEntry, DiffStatus, FORBIDDEN_TOP_LEVEL_PREFIXES,
    };
    use ralph_core::supervisor::dag_integration::InMemoryIntegrationStore;
    use ralph_core::supervisor::integration_lane::{
        CasOutcome, FakeGitIntegrationPort, FakeRepo, GateOutcome, IntegrationLane,
    };

    fn entry(path: &str) -> DiffPathEntry {
        DiffPathEntry {
            path: PathBuf::from(path),
            status: DiffStatus::Modified,
            is_symlink: false,
            is_submodule: false,
        }
    }

    fn orchestrator_with_fake(
        port: &Arc<FakeGitIntegrationPort>,
    ) -> Arc<IntegrationOrchestrator<FakeRepo, FakeGitIntegrationPort>> {
        let core = Arc::new(LaneCore::new());
        let lane = Arc::new(IntegrationLane::<FakeRepo, _>::new(core, port.clone()));
        let store: Arc<dyn IntegrationStore> = Arc::new(InMemoryIntegrationStore::new());
        Arc::new(IntegrationOrchestrator::new(lane, store))
    }

    fn base_request() -> IntegrationRequest {
        IntegrationRequest {
            unit_id: "U1".to_string(),
            integration_order: 1,
            target_branch: "feat/integration".to_string(),
            base_commit: "BASE_OID".to_string(),
            unit_commit: "UNIT_OID".to_string(),
            changed_paths: vec![entry("src/a.rs"), entry("src/b.rs")],
            allowlist: vec![PathBuf::from("src")],
            // U8: declared changed-set authorises everything the base
            // request actually changes (`src/a.rs`, `src/b.rs`); tests
            // that mutate `changed_paths` outside the declared set
            // (forbidden prefix, symlink) short-circuit at earlier
            // checks before the declared-set gate runs.
            declared_paths: vec![PathBuf::from("src")],
            created_at_ms: 1_700_000_000_000,
        }
    }

    /// U7 contract: a candidate whose changed paths fall
    /// inside the allowlist + pass every check integrates,
    /// the lane CAS advances, and the integration store
    /// records the row.
    #[test]
    fn orchestrator_integrates_clean_candidate_end_to_end() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        port.set_head("feat/integration", "BASE_OID");
        port.set_unit_tree("UNIT_OID", "TREE_OID");
        let orch = orchestrator_with_fake(&port);

        let outcome = orch.integrate(base_request()).expect("integrate");
        match outcome {
            IntegrationOutcome::Integrated {
                record,
                target_branch,
                new_head,
            } => {
                assert_eq!(record.unit_id, "U1");
                assert_eq!(target_branch, "feat/integration");
                // Fake port's squash_commit format is
                // "squash-<unit_id>-<idx>" with idx
                // incrementing per call.
                assert_eq!(record.integrated_commit, "squash-U1-1");
                assert_eq!(new_head, "squash-U1-1");
            }
            other => panic!("expected Integrated, got {other:?}"),
        }
        // The lane is released.
        assert!(
            orch.lane
                .core
                .current_holder("feat/integration")
                .unwrap()
                .is_none()
        );
    }

    /// Step E2: the tested candidate is persisted as an intent
    /// BEFORE the CAS moves the target ref, and the intent matches
    /// the integration record the CAS produced.
    #[test]
    fn integrate_persists_intent_before_cas() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        port.set_head("feat/integration", "BASE_OID");
        port.set_unit_tree("UNIT_OID", "TREE_OID");
        let core = Arc::new(LaneCore::new());
        let lane = Arc::new(IntegrationLane::<FakeRepo, _>::new(core, port.clone()));
        let store: Arc<dyn IntegrationStore> = Arc::new(InMemoryIntegrationStore::new());
        let orch = Arc::new(IntegrationOrchestrator::new(lane, store.clone()));

        let outcome = orch.integrate(base_request()).expect("integrate");
        let IntegrationOutcome::Integrated { record, .. } = outcome else {
            panic!("expected Integrated, got {outcome:?}");
        };
        let intent = store
            .get_intent("U1", "feat/integration")
            .expect("get_intent")
            .expect("intent persisted before CAS");
        assert_eq!(intent.unit_commit, "UNIT_OID");
        assert_eq!(intent.input.integrated_commit, record.integrated_commit);
        assert_eq!(intent.input.expected_head_before, record.expected_head_before);
    }

    /// U7 contract: a forbidden top-level prefix is rejected
    /// on the SECOND check (after the reviewer already
    /// approved). The lane never acquires, the store never
    /// sees a row.
    #[test]
    fn orchestrator_rejects_forbidden_path_on_second_check() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        port.set_head("feat/integration", "BASE_OID");
        port.set_unit_tree("UNIT_OID", "TREE_OID");
        let orch = orchestrator_with_fake(&port);

        let mut req = base_request();
        // Sneak in a `.git/HEAD` change between reviewer
        // approval and lock acquire — the orchestrator must
        // refuse even though the allowlist is `["src"]` and
        // the reviewer already said "go".
        req.changed_paths = vec![entry("src/a.rs"), entry(".git/HEAD")];
        let err = orch.integrate(req).expect_err("must reject");
        assert!(matches!(
            err,
            IntegrationError::ChangedPathRejected(ChangedPathRejection::ForbiddenPath(_))
        ));
        // Lane must still be free (we never acquired).
        assert!(
            orch.lane
                .core
                .current_holder("feat/integration")
                .unwrap()
                .is_none()
        );
    }

    /// U7 contract: a symlink change is rejected on the
    /// second check. (Defence against an agent that swaps
    /// a regular file for a symlink after review approval.)
    #[test]
    fn orchestrator_rejects_symlink_on_second_check() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        port.set_head("feat/integration", "BASE_OID");
        port.set_unit_tree("UNIT_OID", "TREE_OID");
        let orch = orchestrator_with_fake(&port);

        let mut req = base_request();
        req.changed_paths = vec![DiffPathEntry {
            path: PathBuf::from("src/link"),
            status: DiffStatus::Modified,
            is_symlink: true,
            is_submodule: false,
        }];
        let err = orch.integrate(req).expect_err("must reject");
        assert!(matches!(
            err,
            IntegrationError::ChangedPathRejected(ChangedPathRejection::SymlinkPath(_))
        ));
    }

    /// U7 contract: when the targeted gate fails, the
    /// orchestrator returns GateFailed, releases the lane,
    /// and writes nothing to the store.
    #[test]
    fn orchestrator_gate_fail_releases_lane_and_writes_no_record() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        port.set_head("feat/integration", "BASE_OID");
        port.set_unit_tree("UNIT_OID", "TREE_OID");
        port.force_gate(GateOutcome::Fail {
            reason: "compilation error".to_string(),
        });
        let orch = orchestrator_with_fake(&port);

        let outcome = orch.integrate(base_request()).expect("integrate");
        match outcome {
            IntegrationOutcome::GateFailed { reason } => {
                assert!(reason.contains("compilation"));
            }
            other => panic!("expected GateFailed, got {other:?}"),
        }
        assert!(
            orch.lane
                .core
                .current_holder("feat/integration")
                .unwrap()
                .is_none()
        );
        assert!(
            orch.store
                .list_for_unit("U1")
                .expect("store readable after gate failure")
                .is_empty(),
            "gate failure must write no integration record"
        );
    }

    /// U7 contract: when the target moves between read and
    /// CAS (hostile-agent race), the orchestrator returns
    /// StaleExpected, releases the lane, and writes nothing
    /// to the store.
    #[test]
    fn orchestrator_stale_expected_releases_lane_and_writes_no_record() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        port.set_head("feat/integration", "BASE_OID");
        port.set_unit_tree("UNIT_OID", "TREE_OID");
        // CAS will refuse because the head will be reported
        // as `STALE_OID`, not `BASE_OID`.
        port.override_cas(
            "feat/integration",
            CasOutcome::StaleExpected {
                expected: "BASE_OID".to_string(),
                actual: "STALE_OID".to_string(),
            },
        );
        let orch = orchestrator_with_fake(&port);

        let outcome = orch.integrate(base_request()).expect("integrate");
        match outcome {
            IntegrationOutcome::StaleExpected { expected, actual } => {
                assert_eq!(expected, "BASE_OID");
                assert_eq!(actual, "STALE_OID");
            }
            other => panic!("expected StaleExpected, got {other:?}"),
        }
        assert!(
            orch.lane
                .core
                .current_holder("feat/integration")
                .unwrap()
                .is_none()
        );
        assert!(
            orch.store
                .list_for_unit("U1")
                .expect("store readable after stale CAS")
                .is_empty(),
            "stale CAS must write no integration record"
        );
    }

    /// U7 contract: integration records are idempotent on the
    /// natural-key tuple. The fake port's `prepare_squash_*`
    /// increments a counter per call, so calling
    /// `integrate()` twice on the same `(unit_id,
    /// target_branch)` would produce DIFFERENT squash
    /// commits and trigger DuplicateUnitForTarget. To
    /// exercise the true idempotency contract (same tuple →
    /// same record, no duplicate row), we drive the store
    /// directly with the SAME `IntegrationInput` twice.
    #[test]
    fn orchestrator_integration_is_idempotent_on_same_unit_target() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        port.set_head("feat/integration", "BASE_OID");
        port.set_unit_tree("UNIT_OID", "TREE_OID");
        let orch = orchestrator_with_fake(&port);

        // First integration: produce the canonical record.
        let first = orch.integrate(base_request()).expect("first");
        let (record_1, expected_head, squash_commit, base_commit) = match first {
            IntegrationOutcome::Integrated { record, .. } => {
                let sq = record.integrated_commit.clone();
                let b = record.base_commit.clone();
                let e = record.expected_head_before.clone();
                (record, e, sq, b)
            }
            other => panic!("expected Integrated, got {other:?}"),
        };
        // Now drive the store directly with the SAME tuple.
        let again = orch
            .store
            .record_integrated(&IntegrationInput {
                unit_id: "U1".to_string(),
                target_branch: "feat/integration".to_string(),
                base_commit,
                integrated_commit: squash_commit,
                expected_head_before: expected_head,
                created_at_ms: 1_700_000_000_000,
            })
            .expect("idempotent");
        assert_eq!(again.id, record_1.id);
        assert_eq!(again.commit_fingerprint, record_1.commit_fingerprint);
        assert_eq!(
            orch.store
                .list_for_unit("U1")
                .expect("store readable")
                .len(),
            1,
            "idempotent replay must not create a second row"
        );
    }

    /// U7 contract: select_eligible returns candidates in
    /// stable `(integration_order, unit_id)` order even when
    /// the input is jumbled.
    #[test]
    fn orchestrator_select_eligible_is_stable() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        let orch = orchestrator_with_fake(&port);
        let candidates = vec![
            IntegrationCandidate {
                unit_id: "U3".to_string(),
                integration_order: 2,
                target_branch: "feat/x".to_string(),
                base_commit: "B".to_string(),
                unit_commit: "C".to_string(),
                authorised_paths: vec![],
            },
            IntegrationCandidate {
                unit_id: "U1".to_string(),
                integration_order: 1,
                target_branch: "feat/x".to_string(),
                base_commit: "B".to_string(),
                unit_commit: "C".to_string(),
                authorised_paths: vec![],
            },
            IntegrationCandidate {
                unit_id: "U2".to_string(),
                integration_order: 1,
                target_branch: "feat/x".to_string(),
                base_commit: "B".to_string(),
                unit_commit: "C".to_string(),
                authorised_paths: vec![],
            },
        ];
        let order: Vec<&str> = orch
            .select_eligible(&candidates)
            .iter()
            .map(|c| c.unit_id.as_str())
            .collect();
        assert_eq!(order, vec!["U1", "U2", "U3"]);
    }

    /// U7 contract: two siblings racing for the same target
    /// serialise — the second `try_acquire` returns
    /// `LaneError::TargetBusy`. The orchestrator surfaces
    /// this as `IntegrationError::Lane(LaneError::TargetBusy)`.
    #[test]
    fn orchestrator_two_siblings_serialise_on_same_target() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        port.set_head("feat/integration", "BASE_OID");
        port.set_unit_tree("UNIT_OID", "TREE_OID");
        let orch = orchestrator_with_fake(&port);

        // First acquire: grab the lease manually so the
        // second integrate call sees the target as busy.
        let _first_guard = orch
            .lane
            .core
            .try_acquire("feat/integration", "U-other")
            .expect("first guard");

        let err = orch.integrate(base_request()).expect_err("must reject");
        match err {
            IntegrationError::Lane(LaneError::TargetBusy) => {}
            other => panic!("expected TargetBusy, got {other:?}"),
        }
    }

    /// U7 contract: re-running with a DIFFERENT base for the
    /// same (unit, target) is rejected at the store layer
    /// (DuplicateUnitForTarget) — the lane had already
    /// accepted the first integration.
    #[test]
    fn orchestrator_same_unit_different_base_is_rejected_by_store() {
        let port = Arc::new(FakeGitIntegrationPort::new());
        port.set_head("feat/integration", "BASE_OID");
        port.set_unit_tree("UNIT_OID", "TREE_OID");
        let orch = orchestrator_with_fake(&port);

        let first = orch.integrate(base_request()).expect("first");
        assert!(matches!(first, IntegrationOutcome::Integrated { .. }));

        // Second: same unit, same target, but a different
        // base. The lane CAS will succeed (the fake port
        // returns the same squash commit anyway), but the
        // store refuses because (unit_id, target_branch)
        // already has a row with a different base_commit.
        let mut second_req = base_request();
        second_req.base_commit = "DIFFERENT_BASE_OID".to_string();
        let err = orch.integrate(second_req).expect_err("store must reject");
        assert!(matches!(
            err,
            IntegrationError::Store(
                ralph_core::supervisor::dag_integration::IntegrationStoreError::DuplicateUnitForTarget { .. }
            )
        ));
    }

    /// U7 contract: the forbidden top-level prefix list is
    /// the one the changed_path_guard module exports —
    /// defence-in-depth so a typo in the orchestrator
    /// doesn't drift from the canonical list.
    #[test]
    fn orchestrator_uses_canonical_forbidden_prefixes() {
        // Sanity: the list contains .git, target,
        // node_modules. The orchestrator doesn't re-export
        // the list (it goes through ChangedPathSet), but
        // let's make sure the canonical list still exists
        // at the expected path.
        assert!(FORBIDDEN_TOP_LEVEL_PREFIXES.contains(&".git"));
        assert!(FORBIDDEN_TOP_LEVEL_PREFIXES.contains(&"target"));
        assert!(FORBIDDEN_TOP_LEVEL_PREFIXES.contains(&".ralph"));
    }

    // ===================================================================
    // TG-S04 (PMI-006, P1, post-merge-converge): durable DAG store
    // ——持久化断言(原过渡 pin 于 durable store 落地后翻转,fixer
    // activation 2026-09-05)。Invariant: exactly-once / crash-window
    // recovery 承诺以持久化状态为前提;`real_orchestrator` 必须把
    // integration 记录写进 rusqlite durable 变体,「进程重启」(第二
    // 个实例)经同 repo_root 的 dag store reopen 读到 A 的记录——
    // 重放语义而非冷启动语义。
    // ===================================================================

    /// TG-S04 步骤 1+2: 实例 A 对 `(unit-u1, feat/target-a)` 执行
    /// `record_integrated`（幂等自然键）;实例 B（同一 repo_root、模拟
    /// 进程重启）查询同 tuple → 断言**读回 A 的记录**（重放语义,
    /// 非冷启动空态）。
    #[test]
    fn tg_s04_real_orchestrator_store_survives_process_restart() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo_root = tmp.path().to_path_buf();

        // 实例 A: 同一 repo_root 构造两个独立 orchestrator,
        // 模拟「进程 A 运行 → 退出 → 进程 B 重启后查询」。
        // (PMI-004① 后 real_orchestrator 需要显式 gate 命令集;
        //  本测试只动 store,直接给空的 gate spec——空集对 store
        //  语义无影响,fail-closed 也不触发 integrate 路径。)
        let orch_a = real_orchestrator(repo_root.clone(), Vec::new())
            .expect("instance A opens the durable DAG store");
        let input_a = IntegrationInput {
            unit_id: "unit-u1".to_string(),
            target_branch: "feat/target-a".to_string(),
            base_commit: "BASE_A".to_string(),
            integrated_commit: "SQUASH_A".to_string(),
            expected_head_before: "HEAD_A".to_string(),
            created_at_ms: 1_700_000_000_000,
        };
        let recorded_a = orch_a
            .store
            .record_integrated(&input_a)
            .expect("instance A records the integration");
        assert_eq!(recorded_a.unit_id, "unit-u1");
        assert_eq!(recorded_a.target_branch, "feat/target-a");
        // 幂等自然键: 同一实例内同 tuple 重放返回同一行。
        let replay_a = orch_a
            .store
            .record_integrated(&input_a)
            .expect("same-instance replay is idempotent");
        assert_eq!(replay_a.id, recorded_a.id);

        // 实例 B（同 repo_root,模拟进程重启）: 同 tuple 查询。
        // durable store 落地后,实例 B 必须读回 A 的记录(重放语义)。
        let orch_b = real_orchestrator(repo_root.clone(), Vec::new())
            .expect("instance B reopens the durable DAG store");
        let rows_b = orch_b
            .store
            .list_for_unit("unit-u1")
            .expect("instance B queries the same unit tuple");
        assert_eq!(
            rows_b.len(),
            1,
            "TG-S04 durable assertion: instance B (same repo_root, simulated \
             process restart) MUST see instance A's integration record for \
             (unit-u1, feat/target-a) — got {} row(s): {rows_b:?}. \
             If this assertion FAILS, real_orchestrator has regressed to a \
             process-local store (half-wiring / durability loss): restore \
             the rusqlite-backed store in real_orchestrator per PMI-006.",
            rows_b.len()
        );
        assert_eq!(rows_b[0].unit_id, "unit-u1");
        assert_eq!(rows_b[0].target_branch, "feat/target-a");
        assert_eq!(rows_b[0].base_commit, "BASE_A");
        assert_eq!(rows_b[0].integrated_commit, "SQUASH_A");
        assert_eq!(rows_b[0].expected_head_before, "HEAD_A");
        // 同 tuple 幂等重放(跨进程): B 以 A 的原始输入重放,必须
        // 返回同一行(fingerprint 一致,零 DuplicateUnitForTarget)。
        let replay_b = orch_b
            .store
            .record_integrated(&input_a)
            .expect("cross-instance replay is idempotent");
        assert_eq!(replay_b.id, recorded_a.id);
        assert_eq!(replay_b.commit_fingerprint, recorded_a.commit_fingerprint);
    }

    /// TG-S04 步骤 3: `crates/ralph-core/src/supervisor/dag_store_rusqlite.rs`
    /// 存在、`migrations/` v13 覆盖 DAG 表——durable 层与 real_orchestrator
    /// 改线必须同 PR 落地(防半接线)。原「无 durable 层」过渡 pin 于
    /// durable store 落地时翻转为本断言。
    #[test]
    fn tg_s04_durable_dag_store_layer_exists_and_migrations_cover_dag_tables() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        // CARGO_MANIFEST_DIR = .../crates/ralph-cli → repo root
        // 两级父目录（与 crates/ralph-cli/tests/repro_pmi_005.rs 的
        // repo_root() 同一推导）。
        let repo_root = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .expect("CARGO_MANIFEST_DIR must be at crates/ralph-cli")
            .to_path_buf();

        let supervisor_dir = repo_root.join("crates/ralph-core/src/supervisor");
        let rusqlite_variant = supervisor_dir.join("dag_store_rusqlite.rs");
        assert!(
            rusqlite_variant.exists(),
            "TG-S04 durable assertion: {rusqlite_variant:?} no longer exists — \
             the durable DAG store layer was removed. Per PMI-006, \
             real_orchestrator depends on the rusqlite DAG store; removing \
             it requires a deliberate revert decision, not silent deletion."
        );

        // migrations/ 覆盖 DAG 表: v13 DDL 必须命中 dag_plans /
        // dag_integrations 两张表(与 real_orchestrator 的 durable store
        // 同 PR 落地,防「store 在、schema 漂移」)。
        let migrations_dir = supervisor_dir.join("migrations");
        let migration_files: Vec<PathBuf> = std::fs::read_dir(&migrations_dir)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", migrations_dir.display()))
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        assert!(
            !migration_files.is_empty(),
            "supervisor migrations dir unexpectedly empty — the SQL \
             set must be present for this pin to mean anything"
        );
        let mut combined_ddl = String::new();
        for path in &migration_files {
            let body = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            combined_ddl.push_str(&body);
        }
        let ddl_lower = combined_ddl.to_ascii_lowercase();
        for table in ["dag_plans", "dag_integrations"] {
            assert!(
                ddl_lower.contains(table),
                "TG-S04 durable assertion: migration DDL no longer covers the \
                 `{table}` table — the DAG schema drifted from the rusqlite \
                 store. Restore the table (v13+) or update this pin \
                 alongside a deliberate schema decision (PMI-006)."
            );
        }
    }

    // ===================================================================
    // TG-S12 (PMI-006, P1, promote-time 全量恢复验证): durable DAG
    // store 落地后的 crash-window / exactly-once 验收矩阵。
    // 场景(09-test-gap-plan §TG-S12):
    //   1. 进程 A 注册 plan + record_integrated(unit, target) → 退出
    //      (kill -9 由「实例 drop 后重开」模拟——SQLite 的 WAL 保证
    //      已 commit 的行跨进程存活,这正是被验证的机制本体)。
    //   2. 进程 B(同 repo)reopen store → 断言 plan 行/integration
    //      记录可读。
    //   3. recovery planner 对同 tuple 输入 → Idempotent(而非
    //      Commit)——重放零不可逆副作用(不重复 FF/squash)。
    //   4. 异 tuple → DuplicateUnitForTarget fail-closed 不变。
    // invariant: 崩溃后 reopen 走「重放/幂等」而非「冷启动」。
    // ===================================================================
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn tg_s12_crash_restart_replays_idempotent_not_cold_start() {
        use ralph_core::supervisor::dag_store::{
            CanonicalPlanRecord, DagSchedulerStore, PlanStatus,
        };
        use ralph_core::supervisor::dag_store_rusqlite::RusqliteDagSchedulerStore;

        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("dag.db");
        let plan_record = CanonicalPlanRecord {
            plan_key: "plan-key-x".to_string(),
            artifact_digest: "digest-x".to_string(),
            target_branch: "feat/target-a".to_string(),
            unit_ids: vec!["unit-u1".to_string(), "unit-u2".to_string()],
            created_at_ms: 1_700_000_000_000,
        };
        let input = IntegrationInput {
            unit_id: "unit-u1".to_string(),
            target_branch: "feat/target-a".to_string(),
            base_commit: "BASE_A".to_string(),
            integrated_commit: "SQUASH_A".to_string(),
            expected_head_before: "HEAD_A".to_string(),
            created_at_ms: 1_700_000_000_000,
        };

        // 步骤 1: 进程 A 注册 plan + activate + record_integrated,
        // 然后整实例 drop(模拟 kill -9:连接关闭,WAL 落盘)。
        {
            let plans_a =
                RusqliteDagSchedulerStore::open(&db_path).expect("process A opens plan store");
            let reg = plans_a
                .register_plan(&plan_record)
                .expect("process A registers the plan");
            assert_eq!(reg.status, PlanStatus::Pending);
            plans_a
                .activate_plan("plan-key-x", "feat/target-a")
                .expect("process A activates the plan");
            let integration_a = plans_a.shared_with_integration();
            integration_a
                .record_integrated(&input)
                .expect("process A records the integration");
            // 实例 A 到此结束——不再持有任何 handle。
        }

        // 步骤 2: 进程 B(同 repo)reopen store → plan 行 + integration
        // 记录可读(重放语义,非冷启动)。
        let plans_b =
            RusqliteDagSchedulerStore::open(&db_path).expect("process B reopens the store");
        let reg_b = plans_b
            .get_plan("plan-key-x")
            .expect("process B reads the plan")
            .expect("plan row survives the restart");
        assert_eq!(reg_b.artifact_digest, "digest-x");
        assert_eq!(reg_b.target_branch, "feat/target-a");
        assert_eq!(reg_b.status, PlanStatus::Active, "activation survives");
        assert_eq!(
            reg_b.unit_ids,
            vec!["unit-u1".to_string(), "unit-u2".to_string()]
        );
        let integration_b = plans_b.shared_with_integration();
        let rows = integration_b
            .list_for_unit("unit-u1")
            .expect("process B lists the unit");
        assert_eq!(rows.len(), 1, "integration record survives the restart");
        assert_eq!(rows[0].integrated_commit, "SQUASH_A");
        assert!(!rows[0].acked, "ack state round-trips as false");
        // 重放读出的记录与 recovery planner 的 persisted 输入对齐。
        let persisted = super::super::recovery::MergeIntentFingerprint {
            unit_id: rows[0].unit_id.clone(),
            base_commit: rows[0].base_commit.clone(),
            integrated_commit: rows[0].integrated_commit.clone(),
            expected_head_before: rows[0].expected_head_before.clone(),
        };

        // 步骤 3: recovery planner 对同 tuple 输入 → Idempotent
        // (而非 Commit)——重放零不可逆副作用。
        let candidate = super::super::recovery::MergeIntentFingerprint {
            unit_id: input.unit_id.clone(),
            base_commit: input.base_commit.clone(),
            integrated_commit: input.integrated_commit.clone(),
            expected_head_before: input.expected_head_before.clone(),
        };
        use super::super::recovery::{IntegrationRecordDecision, plan_integration_record};
        assert_eq!(
            plan_integration_record(Some(&persisted), &candidate),
            IntegrationRecordDecision::Idempotent,
            "TG-S12 step 3: same-tuple replay after crash must plan as \
             Idempotent, not Commit — otherwise recovery would re-FF/re-squash"
        );

        // 步骤 4: 异 tuple → DuplicateUnitForTarget fail-closed 不变。
        let drift_candidate = super::super::recovery::MergeIntentFingerprint {
            unit_id: input.unit_id.clone(),
            base_commit: "DIFFERENT_BASE".to_string(),
            integrated_commit: input.integrated_commit.clone(),
            expected_head_before: input.expected_head_before.clone(),
        };
        assert!(
            matches!(
                plan_integration_record(Some(&persisted), &drift_candidate),
                IntegrationRecordDecision::DuplicateUnitForTarget { .. }
            ),
            "TG-S12 step 4: different tuple for the same unit must stay \
             fail-closed (DuplicateUnitForTarget)"
        );
        // 同一语义在 store 层直接验证(持久化行存在,replay 路径)。
        let mut drifted = input.clone();
        drifted.base_commit = "DIFFERENT_BASE".to_string();
        let err = integration_b
            .record_integrated(&drifted)
            .expect_err("different tuple must fail closed");
        assert!(matches!(
            err,
            ralph_core::supervisor::dag_integration::IntegrationStoreError::DuplicateUnitForTarget { .. }
        ));
        // 同 tuple 幂等重放(跨进程): plan 侧同样幂等。
        let reg_replay = plans_b
            .register_plan(&plan_record)
            .expect("cross-process plan re-register is idempotent");
        assert_eq!(reg_replay.id, reg_b.id);
        let replay = integration_b
            .record_integrated(&input)
            .expect("cross-process replay is idempotent");
        assert_eq!(replay.id, rows[0].id);
        assert_eq!(replay.commit_fingerprint, rows[0].commit_fingerprint);
    }

    // ===================================================================
    // TG-S07 (PMI-004①, P2, post-merge-converge) — FLIPPED per the
    // pin's own built-in upgrade guidance (TG-S13): the real gate
    // landed. This family now asserts the flip semantics the pin
    // demanded — gate fail → `IntegrationOutcome::GateFailed` +
    // lane released + store zero rows (Fake 语义参照:
    // orchestrator_gate_fail_releases_lane_and_writes_no_record),
    // plus the empty-gate-spec fail-closed contract and the
    // passing-gate integration path.
    // ===================================================================

    /// TG-S07 orchestrator 级（翻转后）: gate-hostile squash + 真实
    /// gate 命令集 → `IntegrationOutcome::GateFailed`,main HEAD 未
    /// 移动、store 零行、lane 释放。端到端决策流级实证(非仅 port
    /// 方法): 恒 Pass 假绿(P0 级危害——未过门禁的 squash 被 FF 进
    /// target)已不可达。
    ///
    /// fixture: main 带 `base.txt`;unit 分支删掉 `base.txt`(
    /// gate-hostile 形态);gate 命令集 = `sh -c 'test -f base.txt'`。
    #[test]
    fn tg_s07_real_orchestrator_rejects_gate_hostile_squash() {
        use std::process::Command as StdCommand;

        fn git(root: &std::path::Path, args: &[&str]) -> String {
            let out = StdCommand::new("git")
                .arg("-C")
                .arg(root)
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

        // Build the throwaway repo (mirrors ralph-core's
        // tests_real_port::fixture_repo shape, local copy because the
        // core fixture helper is not exported across crates).
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        git(root, &["init", "-q", "--initial-branch=main"]);
        git(root, &["config", "user.email", "test@example.com"]);
        git(root, &["config", "user.name", "Test"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git(root, &["add", "base.txt"]);
        git(root, &["commit", "-q", "-m", "base"]);
        let main_oid = git(root, &["rev-parse", "HEAD"]);

        // Gate-hostile unit branch: DROP base.txt. The real gate
        // command below (`test -f base.txt`) fails this squash.
        git(root, &["checkout", "-q", "-b", "feat/u1"]);
        std::fs::remove_file(root.join("base.txt")).unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", "drop base.txt"]);
        let unit_commit = git(root, &["rev-parse", "HEAD"]);
        git(root, &["checkout", "-q", "main"]);

        let orch = real_orchestrator(
            root.to_path_buf(),
            vec![GateCommandSpec {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), "test -f base.txt".to_string()],
            }],
        )
        .expect("TG-S07 opens the durable DAG store");
        let outcome = orch
            .integrate(IntegrationRequest {
                unit_id: "U1".to_string(),
                integration_order: 1,
                target_branch: "main".to_string(),
                base_commit: main_oid.clone(),
                unit_commit,
                changed_paths: vec![DiffPathEntry {
                    path: PathBuf::from("base.txt"),
                    status: DiffStatus::Modified,
                    is_symlink: false,
                    is_submodule: false,
                }],
                allowlist: vec![PathBuf::from("base.txt")],
                declared_paths: vec![PathBuf::from("base.txt")],
                created_at_ms: 1_700_000_000_000,
            })
            .expect("integrate runs the decision flow");

        match outcome {
            IntegrationOutcome::GateFailed { reason } => {
                // Reason carries the failing command (bounded stderr tail).
                assert!(
                    reason.contains("base.txt") || reason.contains("exited"),
                    "GateFailed reason must carry the failing gate command, got: {reason}"
                );
            }
            IntegrationOutcome::Integrated { .. } => panic!(
                "TG-S07 flipped pin: gate-hostile squash was FF'd into main — the \
                 unconditional-Pass placeholder has regressed (PMI-004①). See \
                 .ralph/post-merge/09-test-gap-plan.md §TG-S07/TG-S13."
            ),
            other => panic!("TG-S07 flipped pin: expected GateFailed, got {other:?}"),
        }

        // main HEAD 未移动 (git 断言,非仅退出码)。
        assert_eq!(
            git(root, &["rev-parse", "refs/heads/main"]),
            main_oid,
            "gate failure must leave the target branch unmoved"
        );
        // store 零行 (TG-S13 fail-injection 语义)。
        let rows = orch
            .store
            .list_for_unit("U1")
            .expect("store readable after gate failure");
        assert!(
            rows.is_empty(),
            "gate failure must write no integration record, got {} row(s)",
            rows.len()
        );
        // lane 释放 (lane 可重新 acquire)。
        assert!(
            orch.lane
                .core
                .current_holder("main")
                .expect("lane readable")
                .is_none(),
            "gate failure must release the lane lease"
        );
    }

    /// TG-S07 orchestrator 级（翻转后）——对照路径: gate 通过 → 正常
    /// Integrated。gate 命令集真实执行(exit 0)且 CAS FF 落地,证明
    /// 翻转没有把 real 路径推向「恒拒绝」的反向退化。
    #[test]
    fn tg_s07_real_orchestrator_integrates_when_gate_passes() {
        use std::process::Command as StdCommand;

        fn git(root: &std::path::Path, args: &[&str]) -> String {
            let out = StdCommand::new("git")
                .arg("-C")
                .arg(root)
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

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        git(root, &["init", "-q", "--initial-branch=main"]);
        git(root, &["config", "user.email", "test@example.com"]);
        git(root, &["config", "user.name", "Test"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git(root, &["add", "base.txt"]);
        git(root, &["commit", "-q", "-m", "base"]);
        let main_oid = git(root, &["rev-parse", "HEAD"]);

        // Benign unit branch: keeps base.txt, adds u1.txt.
        git(root, &["checkout", "-q", "-b", "feat/u1"]);
        std::fs::write(root.join("u1.txt"), "unit-1\n").unwrap();
        git(root, &["add", "u1.txt"]);
        git(root, &["commit", "-q", "-m", "add u1.txt"]);
        let unit_commit = git(root, &["rev-parse", "HEAD"]);
        git(root, &["checkout", "-q", "main"]);

        let orch = real_orchestrator(
            root.to_path_buf(),
            vec![GateCommandSpec {
                program: "sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    "test -f base.txt && test -f u1.txt".to_string(),
                ],
            }],
        )
        .expect("TG-S07 opens the durable DAG store");
        let outcome = orch
            .integrate(IntegrationRequest {
                unit_id: "U1".to_string(),
                integration_order: 1,
                target_branch: "main".to_string(),
                base_commit: main_oid.clone(),
                unit_commit,
                changed_paths: vec![DiffPathEntry {
                    path: PathBuf::from("u1.txt"),
                    status: DiffStatus::Modified,
                    is_symlink: false,
                    is_submodule: false,
                }],
                allowlist: vec![PathBuf::from("u1.txt")],
                declared_paths: vec![PathBuf::from("u1.txt")],
                created_at_ms: 1_700_000_000_000,
            })
            .expect("integrate must succeed for a passing gate");

        match outcome {
            IntegrationOutcome::Integrated { new_head, .. } => {
                let tip = git(root, &["rev-parse", "refs/heads/main"]);
                assert_eq!(tip, new_head, "passing gate → FF into main");
                let tree_files = git(root, &["ls-tree", "--name-only", "HEAD"]);
                assert!(
                    tree_files.lines().any(|f| f == "base.txt")
                        && tree_files.lines().any(|f| f == "u1.txt"),
                    "integrated tree must carry both files"
                );
            }
            IntegrationOutcome::GateFailed { reason } => panic!(
                "TG-S07 flipped pin (contrast path): a PASSING gate command set must \
                 not reject the candidate, got GateFailed: {reason}"
            ),
            other => panic!("TG-S07 flipped pin: expected Integrated, got {other:?}"),
        }
        // Store carries exactly one row (integration recorded).
        let rows = orch.store.list_for_unit("U1").expect("store readable");
        assert_eq!(rows.len(), 1, "integrated record persisted exactly once");
    }

    /// TG-S07 orchestrator 级（翻转后）——fail-closed 路径: 空的 gate
    /// 命令集 = 无门禁配置,`real_orchestrator` 的整合流水线对任何
    /// candidate 都拒绝(GateFailed),绝不回到「恒 Pass 无门禁」。
    /// 这是 PMI-004① 关闭后防止半接线复活的常驻断言。
    #[test]
    fn tg_s07_real_orchestrator_empty_gate_spec_fails_closed() {
        use std::process::Command as StdCommand;

        fn git(root: &std::path::Path, args: &[&str]) -> String {
            let out = StdCommand::new("git")
                .arg("-C")
                .arg(root)
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

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        git(root, &["init", "-q", "--initial-branch=main"]);
        git(root, &["config", "user.email", "test@example.com"]);
        git(root, &["config", "user.name", "Test"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git(root, &["add", "base.txt"]);
        git(root, &["commit", "-q", "-m", "base"]);
        let main_oid = git(root, &["rev-parse", "HEAD"]);
        git(root, &["checkout", "-q", "-b", "feat/u1"]);
        std::fs::write(root.join("u1.txt"), "unit-1\n").unwrap();
        git(root, &["add", "u1.txt"]);
        git(root, &["commit", "-q", "-m", "add u1.txt"]);
        let unit_commit = git(root, &["rev-parse", "HEAD"]);
        git(root, &["checkout", "-q", "main"]);

        // Empty gate command set — the constructor contract says
        // this FAILS CLOSED at Step 6.
        let orch = real_orchestrator(root.to_path_buf(), Vec::new())
            .expect("orchestrator still constructs (store path is independent)");
        let outcome = orch
            .integrate(IntegrationRequest {
                unit_id: "U1".to_string(),
                integration_order: 1,
                target_branch: "main".to_string(),
                base_commit: main_oid.clone(),
                unit_commit,
                changed_paths: vec![DiffPathEntry {
                    path: PathBuf::from("u1.txt"),
                    status: DiffStatus::Modified,
                    is_symlink: false,
                    is_submodule: false,
                }],
                allowlist: vec![PathBuf::from("u1.txt")],
                declared_paths: vec![PathBuf::from("u1.txt")],
                created_at_ms: 1_700_000_000_000,
            })
            .expect("integrate runs the decision flow");
        match outcome {
            IntegrationOutcome::GateFailed { reason } => {
                assert!(
                    reason.contains("fail-closed") || reason.contains("no commands"),
                    "empty gate spec must explain the fail-closed refusal, got: {reason}"
                );
            }
            IntegrationOutcome::Integrated { .. } => panic!(
                "TG-S07 flipped pin: empty gate spec must FAIL CLOSED — an \
                 unconfigured gate clearing a candidate to FF is the fake-green \
                 gate PMI-004① closed."
            ),
            other => panic!("TG-S07 flipped pin: expected GateFailed, got {other:?}"),
        }
        // Branch + store both untouched.
        assert_eq!(
            git(root, &["rev-parse", "refs/heads/main"]),
            main_oid,
            "fail-closed refusal must leave the target branch unmoved"
        );
        let rows = orch.store.list_for_unit("U1").expect("store readable");
        assert!(rows.is_empty(), "fail-closed refusal must write no rows");
    }

    // ===================================================================
    // S19 (P0-1, 2026-09-07) closed loop: two sibling Units admitted at
    // the SAME base integrate SEQUENTIALLY into the same target. Before
    // the fix the second Unit's squash was parented on the stale
    // admission base and the lane's CAS ancestor check refused it
    // forever. After the fix the squash is parented on the lane-time
    // head with the unit diff applied onto it.
    // ===================================================================

    /// Two same-base siblings on one target: U1 lands (head moves),
    /// U2 still squashes onto the moved head, passes the gate, and
    /// CAS-advances. The object id the CAS advances MUST be the object
    /// id the targeted gate ran against.
    #[test]
    fn s19_two_same_base_siblings_integrate_sequentially() {
        use std::process::Command as StdCommand;

        fn git(root: &std::path::Path, args: &[&str]) -> String {
            let out = StdCommand::new("git")
                .arg("-C")
                .arg(root)
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

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        git(root, &["init", "-q", "--initial-branch=main"]);
        git(root, &["config", "user.email", "test@example.com"]);
        git(root, &["config", "user.name", "Test"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git(root, &["add", "base.txt"]);
        git(root, &["commit", "-q", "-m", "base"]);
        let base_oid = git(root, &["rev-parse", "HEAD"]);

        // Two sibling unit branches from the SAME base, disjoint files.
        git(root, &["checkout", "-q", "-b", "feat/u1"]);
        std::fs::write(root.join("u1.txt"), "unit-1\n").unwrap();
        git(root, &["add", "u1.txt"]);
        git(root, &["commit", "-q", "-m", "unit-1"]);
        let u1_commit = git(root, &["rev-parse", "HEAD"]);
        git(root, &["checkout", "-q", "main"]);
        git(root, &["checkout", "-q", "-b", "feat/u2"]);
        std::fs::write(root.join("u2.txt"), "unit-2\n").unwrap();
        git(root, &["add", "u2.txt"]);
        git(root, &["commit", "-q", "-m", "unit-2"]);
        let u2_commit = git(root, &["rev-parse", "HEAD"]);
        git(root, &["checkout", "-q", "main"]);

        let orch = real_orchestrator(
            root.to_path_buf(),
            vec![GateCommandSpec {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), "test -f base.txt".to_string()],
            }],
        )
        .expect("S19 opens the durable DAG store");

        let request =
            |unit_id: &str, order: u32, unit_commit: &str, file: &str| IntegrationRequest {
                unit_id: unit_id.to_string(),
                integration_order: order,
                target_branch: "main".to_string(),
                base_commit: base_oid.clone(),
                unit_commit: unit_commit.to_string(),
                changed_paths: vec![DiffPathEntry {
                    path: PathBuf::from(file),
                    status: DiffStatus::Added,
                    is_symlink: false,
                    is_submodule: false,
                }],
                allowlist: vec![PathBuf::from(file)],
                declared_paths: vec![PathBuf::from(file)],
                created_at_ms: 1_700_000_000_000,
            };

        // U1 integrates; main advances to the first squash.
        let head_1 = match orch
            .integrate(request("U1", 1, &u1_commit, "u1.txt"))
            .expect("U1 integrate")
        {
            IntegrationOutcome::Integrated { new_head, .. } => new_head,
            other => panic!("S19: U1 expected Integrated, got {other:?}"),
        };
        assert_eq!(git(root, &["rev-parse", "refs/heads/main"]), head_1);
        assert_ne!(head_1, base_oid, "precondition: U1 moved the head");

        // U2 admitted at the SAME (now stale) base must still
        // integrate: squash parented on the lane-time head.
        let (record_2, head_2) = match orch
            .integrate(request("U2", 2, &u2_commit, "u2.txt"))
            .expect("U2 integrate")
        {
            IntegrationOutcome::Integrated {
                record, new_head, ..
            } => (record, new_head),
            other => panic!(
                "S19 regression: second same-base sibling was rejected ({other:?}) — \
                 the squash must be built on the lane-time head, not the \
                 admission-time base (P0-1 / D19 / S19)"
            ),
        };
        assert_eq!(git(root, &["rev-parse", "refs/heads/main"]), head_2);
        // The object the CAS advanced is exactly the object the
        // targeted gate ran against (the squash commit recorded by
        // the orchestrator).
        assert_eq!(
            record_2.integrated_commit, head_2,
            "CAS-advanced object id must equal the gate-tested candidate object id"
        );
        // The squash is a descendant of the moved head BY
        // CONSTRUCTION: its parent is U1's squash.
        assert_eq!(
            git(root, &["rev-parse", &format!("{head_2}^")]),
            head_1,
            "U2 squash parent must be the lane-time head (U1's squash)"
        );
        // The final tree carries base + BOTH siblings' files — the
        // unit diff was APPLIED onto the head, not wholesale-replaced
        // (which would have reverted u1.txt).
        let files = git(root, &["ls-tree", "-r", "--name-only", "HEAD"]);
        for f in ["base.txt", "u1.txt", "u2.txt"] {
            assert!(
                files.lines().any(|line| line == f),
                "final tree must carry {f}, got: {files}"
            );
        }
        // Both records persisted; U2's record keeps the ORIGINAL
        // admission base as its base_commit provenance.
        let rows = orch.store.list_for_unit("U2").expect("store readable");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].base_commit, base_oid);
        assert_eq!(rows[0].expected_head_before, head_1);
    }

    /// S19 conflict path: when a sibling changed the SAME lines, the
    /// unit diff no longer applies onto the lane-time head. The
    /// orchestrator surfaces the typed `ApplyConflict`, the target is
    /// untouched, the lane is released, and no record is written.
    #[test]
    fn s19_overlapping_sibling_diff_is_typed_conflict_not_clobber() {
        use std::process::Command as StdCommand;

        fn git(root: &std::path::Path, args: &[&str]) -> String {
            let out = StdCommand::new("git")
                .arg("-C")
                .arg(root)
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

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        git(root, &["init", "-q", "--initial-branch=main"]);
        git(root, &["config", "user.email", "test@example.com"]);
        git(root, &["config", "user.name", "Test"]);
        git(root, &["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git(root, &["add", "base.txt"]);
        git(root, &["commit", "-q", "-m", "base"]);
        let base_oid = git(root, &["rev-parse", "HEAD"]);

        // Both siblings edit the SAME file differently.
        git(root, &["checkout", "-q", "-b", "feat/u1"]);
        std::fs::write(root.join("base.txt"), "unit-1\n").unwrap();
        git(root, &["add", "base.txt"]);
        git(root, &["commit", "-q", "-m", "unit-1"]);
        let u1_commit = git(root, &["rev-parse", "HEAD"]);
        git(root, &["checkout", "-q", "main"]);
        git(root, &["checkout", "-q", "-b", "feat/u2"]);
        std::fs::write(root.join("base.txt"), "unit-2\n").unwrap();
        git(root, &["add", "base.txt"]);
        git(root, &["commit", "-q", "-m", "unit-2"]);
        let u2_commit = git(root, &["rev-parse", "HEAD"]);
        git(root, &["checkout", "-q", "main"]);

        let orch = real_orchestrator(
            root.to_path_buf(),
            vec![GateCommandSpec {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), "exit 0".to_string()],
            }],
        )
        .expect("opens the durable DAG store");

        let request = |unit_id: &str, order: u32, unit_commit: &str| IntegrationRequest {
            unit_id: unit_id.to_string(),
            integration_order: order,
            target_branch: "main".to_string(),
            base_commit: base_oid.clone(),
            unit_commit: unit_commit.to_string(),
            changed_paths: vec![DiffPathEntry {
                path: PathBuf::from("base.txt"),
                status: DiffStatus::Modified,
                is_symlink: false,
                is_submodule: false,
            }],
            allowlist: vec![PathBuf::from("base.txt")],
            declared_paths: vec![PathBuf::from("base.txt")],
            created_at_ms: 1_700_000_000_000,
        };

        let head_1 = match orch
            .integrate(request("U1", 1, &u1_commit))
            .expect("U1 integrate")
        {
            IntegrationOutcome::Integrated { new_head, .. } => new_head,
            other => panic!("U1 expected Integrated, got {other:?}"),
        };

        // U2's diff (base → "unit-2") no longer applies: main now
        // carries "unit-1". Typed conflict, never a silent 3-way.
        let err = orch
            .integrate(request("U2", 2, &u2_commit))
            .expect_err("overlapping sibling diff must be rejected");
        assert!(
            matches!(err, IntegrationError::Lane(LaneError::ApplyConflict { .. })),
            "expected typed ApplyConflict, got {err:?}"
        );
        assert_eq!(
            git(root, &["rev-parse", "refs/heads/main"]),
            head_1,
            "conflict must leave the target branch unmoved"
        );
        assert!(
            orch.store
                .list_for_unit("U2")
                .expect("store readable")
                .is_empty(),
            "conflict must write no integration record"
        );
        assert!(
            orch.lane
                .core
                .current_holder("main")
                .expect("lane readable")
                .is_none(),
            "conflict must release the lane lease"
        );
    }

    // ===================================================================
    // P1 (2026-09-07): the lane lease is process-global per repo — two
    // `real_orchestrator` instances over the same repo_root share one
    // LaneCore, so the same target serialises across instances while
    // different targets stay parallel. Cross-PROCESS exclusion remains
    // the CAS backstop's job (PMI-018).
    // ===================================================================

    #[test]
    fn real_orchestrators_share_lane_core_per_repo() {
        let tmp_a = tempfile::tempdir().expect("tempdir a");
        let tmp_b = tempfile::tempdir().expect("tempdir b");
        let orch_a1 = real_orchestrator(tmp_a.path().to_path_buf(), Vec::new()).expect("a1");
        let orch_a2 = real_orchestrator(tmp_a.path().to_path_buf(), Vec::new()).expect("a2");
        let orch_b = real_orchestrator(tmp_b.path().to_path_buf(), Vec::new()).expect("b");
        assert!(
            Arc::ptr_eq(&orch_a1.lane.core, &orch_a2.lane.core),
            "same repo_root must share one LaneCore"
        );
        assert!(
            !Arc::ptr_eq(&orch_a1.lane.core, &orch_b.lane.core),
            "different repo_root must get an independent LaneCore"
        );
    }

    #[test]
    fn real_orchestrator_lease_serialises_same_target_across_instances() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let orch_a = real_orchestrator(tmp.path().to_path_buf(), Vec::new()).expect("a");
        let orch_b = real_orchestrator(tmp.path().to_path_buf(), Vec::new()).expect("b");

        // Instance A holds the lease for `main`...
        let _guard = orch_a
            .lane
            .core
            .try_acquire("main", "U-holder")
            .expect("A takes main");

        // ...instance B integrating the SAME target sees TargetBusy
        // (Step 2's changed-path guard passes; the lease is what
        // refuses). Fails before any git call, so synthetic ids are
        // fine here.
        let err = orch_b
            .integrate(IntegrationRequest {
                unit_id: "U1".to_string(),
                integration_order: 1,
                target_branch: "main".to_string(),
                base_commit: "BASE".to_string(),
                unit_commit: "UNIT".to_string(),
                changed_paths: vec![entry("src/a.rs")],
                allowlist: vec![PathBuf::from("src")],
                declared_paths: vec![PathBuf::from("src")],
                created_at_ms: 1_700_000_000_000,
            })
            .expect_err("same target across instances must serialise");
        assert!(
            matches!(err, IntegrationError::Lane(LaneError::TargetBusy)),
            "expected TargetBusy, got {err:?}"
        );

        // A DIFFERENT target stays parallel while `main` is held.
        let other = orch_b
            .lane
            .core
            .try_acquire("release/other", "U2")
            .expect("different target stays parallel");
        other.release();
    }
}
