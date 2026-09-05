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
//! 5. port.prepare_squash_candidate
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
    InMemoryIntegrationStore, IntegrationInput, IntegrationRecord, IntegrationStore,
};
use ralph_core::supervisor::integration_lane::{
    CasOutcome, GateOutcome, GitIntegrationPort, IntegrationCandidate, IntegrationLane, LaneCore,
    LaneError, LaneGuard, RealGitIntegrationPort, select_eligible,
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
    /// (path + symlink/submodule flags). Re-checked by the
    /// orchestrator against the lane allowlist at lock time.
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
/// marker (Real/Fake) and the git port.
pub struct IntegrationOrchestrator<R, P>
where
    R: 'static,
    P: GitIntegrationPort + 'static,
{
    pub lane: Arc<IntegrationLane<R, P>>,
    pub store: Arc<InMemoryIntegrationStore>,
}

impl<R, P> IntegrationOrchestrator<R, P>
where
    R: 'static,
    P: GitIntegrationPort + 'static,
{
    pub fn new(lane: Arc<IntegrationLane<R, P>>, store: Arc<InMemoryIntegrationStore>) -> Self {
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
        // base (NOT on top of `expected_head_before` — the
        // squash is purely a tree-on-base commit; the lane
        // expects it to be a fast-forward descendant of
        // `expected_head_before`).
        let squash = self
            .lane
            .port
            .prepare_squash_candidate(&candidate, &req.base_commit)?;

        // Step 6: targeted gate against the squash tree.
        let gate = self.lane.port.run_targeted_gate(&squash)?;
        if let GateOutcome::Fail { reason } = gate {
            // Drop guard, then return.
            guard.release();
            return Ok(IntegrationOutcome::GateFailed { reason });
        }

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
pub fn real_orchestrator(
    repo_root: PathBuf,
) -> Arc<IntegrationOrchestrator<RealRepo, RealGitIntegrationPort>> {
    let core = Arc::new(LaneCore::new());
    let port = Arc::new(RealGitIntegrationPort::new(repo_root));
    let lane = Arc::new(IntegrationLane::<RealRepo, _>::new(core, port));
    let store = Arc::new(InMemoryIntegrationStore::new());
    Arc::new(IntegrationOrchestrator::new(lane, store))
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

    use ralph_core::supervisor::changed_path_guard::{DiffPathEntry, FORBIDDEN_TOP_LEVEL_PREFIXES};
    use ralph_core::supervisor::integration_lane::{
        CasOutcome, FakeGitIntegrationPort, FakeRepo, GateOutcome, IntegrationLane,
    };

    fn entry(path: &str) -> DiffPathEntry {
        DiffPathEntry {
            path: PathBuf::from(path),
            is_symlink: false,
            is_submodule: false,
        }
    }

    fn orchestrator_with_fake(
        port: &Arc<FakeGitIntegrationPort>,
    ) -> Arc<IntegrationOrchestrator<FakeRepo, FakeGitIntegrationPort>> {
        let core = Arc::new(LaneCore::new());
        let lane = Arc::new(IntegrationLane::<FakeRepo, _>::new(core, port.clone()));
        let store = Arc::new(InMemoryIntegrationStore::new());
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
        assert_eq!(orch.store.len(), 0);
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
        assert_eq!(orch.store.len(), 0);
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
        assert_eq!(orch.store.len(), 1);
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
    }

    // ===================================================================
    // TG-S04 (PMI-006, P1, post-merge-converge): DAG 状态族无持久权威
    // ——现状 pin。Invariant: exactly-once / crash-window recovery 承诺
    // 以持久化状态为前提;`real_orchestrator` 每次调用
    // `InMemoryIntegrationStore::new()`（integration.rs 的构造函数体）,
    // 跨「进程重启」（第二个实例）集成记录即蒸发。
    //
    // 本组测试是**过渡 pin**（expected GREEN at HEAD）:
    //   - 步骤 2 变绿 = durable store 已落地 → 按 TG-S12 升级全量验证;
    //   - 步骤 3 变红 = 有人在没改 `real_orchestrator` 的情况下加了
    //     rusqlite 层（半接线）→ 本 pin 强制同步改 `real_orchestrator`。
    // ===================================================================

    /// TG-S04 步骤 1+2: 实例 A 对 `(unit-u1, feat/target-a)` 执行
    /// `record_integrated`（幂等自然键）;实例 B（同一 repo_root、模拟
    /// 进程重启）查询同 tuple → 断言**查不到**（返回空/不存在,而非
    /// 返回 A 的记录）——内存 store 跨进程即丢,行为如实反映现状。
    #[test]
    fn tg_s04_real_orchestrator_store_is_process_local_transitional_pin() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo_root = tmp.path().to_path_buf();

        // 实例 A: 同一 repo_root 构造两个独立 orchestrator,
        // 模拟「进程 A 运行 → 退出 → 进程 B 重启后查询」。
        let orch_a = real_orchestrator(repo_root.clone());
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
        let orch_b = real_orchestrator(repo_root.clone());
        let rows_b = orch_b
            .store
            .list_for_unit("unit-u1")
            .expect("instance B queries the same unit tuple");
        assert!(
            rows_b.is_empty(),
            "TG-S04 transitional pin: instance B (same repo_root, simulated \
             process restart) must NOT see instance A's integration records \
             for (unit-u1, feat/target-a) — got {} row(s): {rows_b:?}. \
             If this assertion FAILS, the durable DAG integration store has \
             landed (good news): follow TG-S12 in \
             .ralph/post-merge/09-test-gap-plan.md to upgrade to the full \
             crash-window / exactly-once verification matrix, and update \
             this pin to assert persistence instead.",
            rows_b.len()
        );
        // 步骤 2 的另一半: store 整体为空（len()=0）,连「行存在但
        // 状态漂移」都不存在——冷启动语义而非重放语义。
        assert_eq!(
            orch_b.store.len(),
            0,
            "instance B's store must be cold-empty (len=0), not carry \
             over instance A's rows"
        );
    }

    /// TG-S04 步骤 3: `crates/ralph-core/src/supervisor/` 不存在
    /// `dag_store_rusqlite.rs`、`migrations/` 无 dag 表——过渡 pin;
    /// promote 落 rusqlite 时本断言必红,强制同步改 `real_orchestrator`
    /// （防止「加了 durable 层但 real 路径仍用内存 store」的半接线）。
    #[test]
    fn tg_s04_no_durable_dag_store_layer_transitional_pin_delete_when_durable_store_lands() {
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
            !rusqlite_variant.exists(),
            "TG-S04 transitional pin: {rusqlite_variant:?} now exists. A durable \
             DAG store layer landed WITHOUT rewiring `real_orchestrator` to \
             use it (or this pin was not updated alongside the cutover). \
             Per PMI-006 / TG-S04 步骤 3: promote 落 rusqlite 时必须同步改 \
             real_orchestrator（integration.rs real_orchestrator 构造的 \
             InMemoryIntegrationStore 换成 durable 变体）,并按 TG-S12 执行 \
             全量 crash-window / exactly-once 验证;删除本 pin 与 \
             tg_s04_real_orchestrator_store_is_process_local_transitional_pin。"
        );

        // migrations/ 无 dag 表: 现有 v1–v12 的 SQL DDL 全文零命中
        // `dag`（当前 grep 实测为 0 命中;迁移文件命名与 DDL 表名都会
        // 携带 dag 字样,直接全文扫描足够钉住「无 DAG 表」）。
        let migrations_dir = supervisor_dir.join("migrations");
        let migration_files: Vec<PathBuf> = std::fs::read_dir(&migrations_dir)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", migrations_dir.display()))
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        assert!(
            !migration_files.is_empty(),
            "supervisor migrations dir unexpectedly empty — the v1–v12 SQL \
             set must be present for this pin to mean anything"
        );
        for path in &migration_files {
            let body = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            assert!(
                !body.to_ascii_lowercase().contains("dag"),
                "TG-S04 transitional pin: migration {} mentions `dag` — a DAG \
                 table landed in migrations/ while real_orchestrator still \
                 constructs InMemoryIntegrationStore. Half-wiring detected; \
                 see PMI-006 / TG-S04 步骤 3 for the promote checklist.",
                path.display()
            );
        }
    }
}
