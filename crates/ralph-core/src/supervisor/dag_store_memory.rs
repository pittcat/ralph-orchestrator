//! 2026-09-03-0959 plan U3 (R2 / R17 / E5 / E7 / E9 / E16):
//! in-memory implementation of [`DagSchedulerStore`].
//!
//! Plan: 2026-09-09-0917-fix-forge-dag-p1-closure-plan U2 / F17 / U13.
//!
//! 2026-09-09-0917 plan U3 (per-Unit base commit pin + per-stage
//! accepted evidence ledger): in-memory backing for
//! `dag_unit_bases` / `dag_stage_evidence`. The bounded maps
//! match the schema-level uniqueness rules so the contract
//! parallels the rusqlite variant byte-for-byte. The full U3
//! contract suite (defined in `contract_tests`)
//! runs against this impl below.
//!
//! Concurrency: a single `Mutex` covers the registration map.
//! Registration, activation, and reads all serialize through the
//! mutex; the lock is held only for the duration of the
//! bounded in-memory mutations so contention is bounded by the
//! duration of a `HashMap::insert` / `HashMap::get`.
//!
//! Idempotency contract:
//! - `register_plan` with a fresh `plan_key` → `Ok(new_row)`.
//! - `register_plan` with `(plan_key, digest)` matching an
//!   existing row → `Ok(existing_row)` (no error, no duplicate
//!   row).
//! - `register_plan` with `plan_key` matching an existing row
//!   but a DIFFERENT `artifact_digest` → `Err(DigestConflict)`.
//! - `activate_plan` on a `Pending` row → `Active`. On an
//!   already-`Active` row → no-op (`Ok(())`). On an unknown
//!   `plan_key` → `Err(UnknownPlan)`. On an already-`Closed`
//!   row → `Err(InvalidTransition { ... "plan is closed" })`.
//!   On a `Pending` OR `Active` row whose registered
//!   `target_branch` does not match the request →
//!   `Err(TargetMismatch)` and the status is left untouched
//!   (R10/R17 fail-closed).
//! - `pin_unit_base` with a fresh `(plan_key, unit_key)` →
//!   `Ok(())`. Re-pin with the SAME base → `Ok(())` (idempotent,
//!   first `pinned_at_ms` wins). Re-pin with a DIFFERENT base →
//!   `Err(UnitBaseDrift)` (fail closed, plan U3 R3/S3).
//! - `record_stage_evidence` with a fresh `(plan_key, unit_key,
//!   stage, attempt)` → `Ok(())`. Re-record with the SAME fields
//!   → `Ok(())` (idempotent). Re-record with any field drift →
//!   `Err(StageEvidenceDrift)` (fail closed, plan U3 R3/S3).

// `result_large_err` is allowed at file scope (more granular than
// crate level) per U2 / F17: keep DagStoreError variants human-readable
// for fail-closed evidence while suppressing function-level
// `result_large_err` errors on every method returning
// `Result<_, DagStoreError>`.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::sync::Mutex;

use super::dag_store::{
    CanonicalPlanRecord, DagSchedulerStore, DagStoreError, DagStoreResult, PlanRegistration,
    PlanStatus, StageEvidenceRecord, UnitBaseRecord,
};

/// In-memory `DagSchedulerStore`. Backed by a single `Mutex`
/// guarding the per-feature maps. Monotonic `id` allocation is
/// store-private; the rusqlite variant will use SQLite rowid.
#[derive(Debug, Default)]
pub struct InMemoryDagSchedulerStore {
    plans: Mutex<HashMap<String, PlanRegistration>>,
    next_id: Mutex<i64>,
    /// U3 (2026-09-09-0917 plan): per-(plan_key, unit_key) base
    /// commit pin. Keyed by `"{plan_key}::{unit_key}"` so a single
    /// map mirrors the schema-level PRIMARY KEY without growing
    /// nested maps.
    unit_bases: Mutex<HashMap<String, UnitBaseRecord>>,
    /// U3 (2026-09-09-0917 plan): per-(plan_key, unit_key, stage,
    /// attempt) accepted evidence ledger. Keyed by
    /// `"{plan_key}::{unit_key}::{stage}::{attempt}"`. A
    /// `latest_stage_evidence` lookup scans the unit's entries
    /// (in practice O(1)–O(4) — execute/review/verify/fix).
    stage_evidence: Mutex<HashMap<String, StageEvidenceRecord>>,
}

impl InMemoryDagSchedulerStore {
    /// Build an empty in-memory DAG store.
    pub fn new() -> Self {
        Self::default()
    }

    fn base_key(plan_key: &str, unit_key: &str) -> String {
        format!("{plan_key}::{unit_key}")
    }

    fn evidence_key(plan_key: &str, unit_key: &str, stage: &str, attempt: u32) -> String {
        format!("{plan_key}::{unit_key}::{stage}::{attempt}")
    }
}

impl DagSchedulerStore for InMemoryDagSchedulerStore {
    fn register_plan(&self, plan: &CanonicalPlanRecord) -> DagStoreResult<PlanRegistration> {
        let mut guard = self
            .plans
            .lock()
            .map_err(|e| DagStoreError::IoError(format!("InMemoryDagSchedulerStore mutex poisoned: {e}")))?;
        if let Some(existing) = guard.get(&plan.plan_key) {
            // Same plan_key: idempotent on identical digest,
            // fail-closed on digest drift.
            if existing.artifact_digest == plan.artifact_digest {
                return Ok(existing.clone());
            }
            return Err(DagStoreError::DigestConflict {
                plan_key: plan.plan_key.clone(),
                expected: existing.artifact_digest.clone(),
                actual: plan.artifact_digest.clone(),
            });
        }
        let mut id_guard = self
            .next_id
            .lock()
            .map_err(|e| DagStoreError::IoError(format!("InMemoryDagSchedulerStore mutex poisoned: {e}")))?;
        *id_guard += 1;
        let id = *id_guard;
        let registration = PlanRegistration {
            id,
            plan_key: plan.plan_key.clone(),
            artifact_digest: plan.artifact_digest.clone(),
            target_branch: plan.target_branch.clone(),
            status: PlanStatus::Pending,
            unit_ids: plan.unit_ids.clone(),
            created_at_ms: plan.created_at_ms,
        };
        guard.insert(plan.plan_key.clone(), registration.clone());
        Ok(registration)
    }

    fn activate_plan(&self, plan_key: &str, target_branch: &str) -> DagStoreResult<()> {
        let mut guard = self
            .plans
            .lock()
            .map_err(|e| DagStoreError::IoError(format!("InMemoryDagSchedulerStore mutex poisoned: {e}")))?;
        let entry = guard
            .get_mut(plan_key)
            .ok_or_else(|| DagStoreError::UnknownPlan(plan_key.to_string()))?;
        if entry.status == PlanStatus::Closed {
            return Err(DagStoreError::InvalidTransition {
                plan_key: plan_key.to_string(),
                expected: "active_or_pending".to_string(),
                actual: format!("plan is {}", entry.status),
            });
        }
        // Validate target_branch matches the registered one in BOTH
        // Pending and Active states (R10/R17 fail-closed). A re-activation
        // of an already-Active plan with the same branch is a no-op;
        // a mismatched branch in either state fails closed and leaves
        // the status untouched.
        if entry.target_branch != target_branch {
            return Err(DagStoreError::TargetMismatch {
                plan_key: plan_key.to_string(),
                expected: entry.target_branch.clone(),
                actual: target_branch.to_string(),
            });
        }
        entry.status = PlanStatus::Active;
        Ok(())
    }

    fn get_plan(&self, plan_key: &str) -> DagStoreResult<Option<PlanRegistration>> {
        let guard = self
            .plans
            .lock()
            .map_err(|e| DagStoreError::IoError(format!("InMemoryDagSchedulerStore mutex poisoned: {e}")))?;
        Ok(guard.get(plan_key).cloned())
    }

    fn list_active_plans(&self) -> DagStoreResult<Vec<PlanRegistration>> {
        let guard = self
            .plans
            .lock()
            .map_err(|e| DagStoreError::IoError(format!("InMemoryDagSchedulerStore mutex poisoned: {e}")))?;
        Ok(guard
            .values()
            .filter(|p| p.status == PlanStatus::Active)
            .cloned()
            .collect())
    }

    // ─────────────────────────────────────────────────────────────────
    // 2026-09-09-0917 plan U3: per-Unit base commit pin +
    // per-stage accepted evidence ledger.
    // ─────────────────────────────────────────────────────────────────

    fn pin_unit_base(
        &self,
        plan_key: &str,
        unit_key: &str,
        base_commit: &str,
        pinned_at_ms: u64,
    ) -> DagStoreResult<()> {
        let key = Self::base_key(plan_key, unit_key);
        let mut guard = self
            .unit_bases
            .lock()
            .map_err(|e| DagStoreError::IoError(format!("InMemoryDagSchedulerStore mutex poisoned: {e}")))?;
        if let Some(existing) = guard.get(&key) {
            if existing.base_commit == base_commit {
                // Idempotent: first pin's `pinned_at_ms` wins, the
                // stored row stays exactly as written.
                return Ok(());
            }
            return Err(DagStoreError::UnitBaseDrift {
                plan_key: plan_key.to_string(),
                unit_key: unit_key.to_string(),
                persisted: existing.base_commit.clone(),
                candidate: base_commit.to_string(),
            });
        }
        guard.insert(
            key,
            UnitBaseRecord {
                plan_key: plan_key.to_string(),
                unit_key: unit_key.to_string(),
                base_commit: base_commit.to_string(),
                pinned_at_ms,
            },
        );
        Ok(())
    }

    fn get_unit_base(
        &self,
        plan_key: &str,
        unit_key: &str,
    ) -> DagStoreResult<Option<UnitBaseRecord>> {
        let guard = self
            .unit_bases
            .lock()
            .map_err(|e| DagStoreError::IoError(format!("InMemoryDagSchedulerStore mutex poisoned: {e}")))?;
        Ok(guard.get(&Self::base_key(plan_key, unit_key)).cloned())
    }

    fn record_stage_evidence(
        &self,
        plan_key: &str,
        unit_key: &str,
        stage: &str,
        attempt: u32,
        evidence: &StageEvidenceRecord,
    ) -> DagStoreResult<()> {
        let key = Self::evidence_key(plan_key, unit_key, stage, attempt);
        let mut guard = self
            .stage_evidence
            .lock()
            .map_err(|e| DagStoreError::IoError(format!("InMemoryDagSchedulerStore mutex poisoned: {e}")))?;
        if let Some(existing) = guard.get(&key) {
            // Compare every immutable field; a single drift fails
            // closed. Mirrors the rusqlite variant's per-field
            // UPDATE-guarded-by-WHERE check.
            let drifted = |field: &'static str, persisted: &str, candidate: &str| {
                if persisted == candidate {
                    None
                } else {
                    Some(DagStoreError::StageEvidenceDrift {
                        plan_key: plan_key.to_string(),
                        unit_key: unit_key.to_string(),
                        stage: stage.to_string(),
                        attempt,
                        field,
                        persisted: persisted.to_string(),
                        candidate: candidate.to_string(),
                    })
                }
            };
            if let Some(err) = drifted(
                "accepted_commit",
                &existing.accepted_commit,
                &evidence.accepted_commit,
            ) {
                return Err(err);
            }
            if let Some(err) = drifted("base_commit", &existing.base_commit, &evidence.base_commit)
            {
                return Err(err);
            }
            if let Some(err) = drifted(
                "evidence_token",
                &existing.evidence_token,
                &evidence.evidence_token,
            ) {
                return Err(err);
            }
            if let Some(err) = drifted(
                "evidence_fingerprint",
                &existing.evidence_fingerprint,
                &evidence.evidence_fingerprint,
            ) {
                return Err(err);
            }
            // Idempotent on identical fields; first write wins
            // (the existing row's `accepted_at_ms` is not
            // rewritten on replay).
            return Ok(());
        }
        guard.insert(key, evidence.clone());
        Ok(())
    }

    fn latest_stage_evidence(
        &self,
        plan_key: &str,
        unit_key: &str,
        stage: &str,
    ) -> DagStoreResult<Option<StageEvidenceRecord>> {
        let guard = self
            .stage_evidence
            .lock()
            .map_err(|e| DagStoreError::IoError(format!("InMemoryDagSchedulerStore mutex poisoned: {e}")))?;
        let prefix = format!("{plan_key}::{unit_key}::{stage}::");
        let mut best: Option<&StageEvidenceRecord> = None;
        for (key, value) in guard.iter() {
            if !key.starts_with(&prefix) {
                continue;
            }
            let matches = match best {
                None => true,
                Some(current) => value.attempt > current.attempt,
            };
            if matches {
                best = Some(value);
            }
        }
        Ok(best.cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::supervisor::dag_store::contract_tests;

    fn plan(key: &str, digest: &str) -> CanonicalPlanRecord {
        CanonicalPlanRecord {
            plan_key: key.to_string(),
            artifact_digest: digest.to_string(),
            target_branch: "feat/test".to_string(),
            unit_ids: vec!["U1".to_string(), "U2".to_string()],
            created_at_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn register_plan_creates_pending_row() {
        let store = InMemoryDagSchedulerStore::new();
        let reg = store.register_plan(&plan("p1", "d1")).expect("register");
        assert_eq!(reg.status, PlanStatus::Pending);
        assert_eq!(reg.plan_key, "p1");
        assert_eq!(reg.artifact_digest, "d1");
        assert!(reg.id > 0);
    }

    #[test]
    fn register_plan_is_idempotent_on_same_digest() {
        let store = InMemoryDagSchedulerStore::new();
        let first = store.register_plan(&plan("p1", "d1")).expect("first");
        let second = store.register_plan(&plan("p1", "d1")).expect("idempotent");
        assert_eq!(first.id, second.id);
        // No duplicate row — get_plan returns the same row.
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.id, first.id);
    }

    #[test]
    fn register_plan_fails_closed_on_digest_conflict() {
        let store = InMemoryDagSchedulerStore::new();
        store.register_plan(&plan("p1", "d1")).expect("first");
        let err = store
            .register_plan(&plan("p1", "d2"))
            .expect_err("conflict");
        match err {
            DagStoreError::DigestConflict {
                plan_key,
                expected,
                actual,
            } => {
                assert_eq!(plan_key, "p1");
                assert_eq!(expected, "d1");
                assert_eq!(actual, "d2");
            }
            other => panic!("expected DigestConflict, got {other:?}"),
        }
    }

    #[test]
    fn activate_plan_transitions_pending_to_active() {
        let store = InMemoryDagSchedulerStore::new();
        store.register_plan(&plan("p1", "d1")).expect("register");
        store.activate_plan("p1", "feat/test").expect("activate");
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.status, PlanStatus::Active);
    }

    #[test]
    fn activate_plan_is_idempotent_on_active() {
        let store = InMemoryDagSchedulerStore::new();
        store.register_plan(&plan("p1", "d1")).expect("register");
        store
            .activate_plan("p1", "feat/test")
            .expect("first activate");
        store
            .activate_plan("p1", "feat/test")
            .expect("no-op second activate");
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.status, PlanStatus::Active);
    }

    #[test]
    fn activate_plan_unknown_key_returns_error() {
        let store = InMemoryDagSchedulerStore::new();
        let err = store
            .activate_plan("missing", "feat/test")
            .expect_err("err");
        assert!(matches!(err, DagStoreError::UnknownPlan(_)));
    }

    #[test]
    fn get_plan_returns_none_for_missing_key() {
        let store = InMemoryDagSchedulerStore::new();
        let fetched = store.get_plan("missing").expect("get");
        assert!(fetched.is_none());
    }

    #[test]
    fn list_active_plans_filters_by_active_only() {
        let store = InMemoryDagSchedulerStore::new();
        store.register_plan(&plan("p1", "d1")).expect("register p1");
        store.register_plan(&plan("p2", "d2")).expect("register p2");
        store.activate_plan("p1", "feat/test").expect("activate p1");
        let actives = store.list_active_plans().expect("list");
        assert_eq!(actives.len(), 1);
        assert_eq!(actives[0].plan_key, "p1");
    }

    #[test]
    fn reopen_equivalent_re_register_returns_same_row() {
        // Simulates S12 (close/reopen SQLite preserves plan/unit/job/lease/lane
        // consistency). The in-memory store has no real "close",
        // but re-registering the same (key, digest) returns the
        // same row, which is the in-memory analog of the contract.
        let store = InMemoryDagSchedulerStore::new();
        let first = store.register_plan(&plan("p1", "d1")).expect("first");
        // Drop the in-memory `reg` handle and re-fetch by key.
        let second = store.register_plan(&plan("p1", "d1")).expect("re-register");
        assert_eq!(first.id, second.id);
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.unit_ids, vec!["U1".to_string(), "U2".to_string()]);
        assert_eq!(fetched.created_at_ms, 1_700_000_000_000);
    }

    #[test]
    fn activate_plan_fails_closed_on_pending_target_mismatch() {
        // C3 + T3: a Pending entry activated with a mismatched
        // target_branch MUST fail closed (TargetMismatch) and leave
        // the status untouched (Pending), not silently flip to
        // Active while keeping the stale registered branch.
        let store = InMemoryDagSchedulerStore::new();
        store
            .register_plan(&CanonicalPlanRecord {
                plan_key: "p1".to_string(),
                artifact_digest: "d1".to_string(),
                target_branch: "feat/test".to_string(),
                unit_ids: vec!["U1".to_string()],
                created_at_ms: 1_700_000_000_000,
            })
            .expect("register");
        let err = store
            .activate_plan("p1", "feat/OTHER")
            .expect_err("mismatch must fail closed");
        match err {
            DagStoreError::TargetMismatch {
                plan_key,
                expected,
                actual,
            } => {
                assert_eq!(plan_key, "p1");
                assert_eq!(expected, "feat/test");
                assert_eq!(actual, "feat/OTHER");
            }
            other => panic!("expected TargetMismatch, got {other:?}"),
        }
        // Status MUST stay Pending — not flipped to Active by the
        // failed activation.
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(
            fetched.status,
            PlanStatus::Pending,
            "status must stay Pending after fail-closed mismatch"
        );
        // And the registered target_branch MUST be unchanged.
        assert_eq!(fetched.target_branch, "feat/test");
    }

    #[test]
    fn activate_plan_fails_closed_on_active_target_mismatch() {
        // C3: the mismatch check applies to Active entries too — a
        // re-activation with a different branch must fail closed,
        // not silently no-op with the stale branch.
        let store = InMemoryDagSchedulerStore::new();
        store.register_plan(&plan("p1", "d1")).expect("register");
        store
            .activate_plan("p1", "feat/test")
            .expect("first activate");
        let err = store
            .activate_plan("p1", "feat/OTHER")
            .expect_err("active mismatch must fail closed");
        assert!(matches!(
            err,
            DagStoreError::TargetMismatch {
                expected: ref e,
                actual: ref a,
                ..
            } if e == "feat/test" && a == "feat/OTHER"
        ));
        // Status stays Active (the original activation is not
        // rolled back), branch unchanged.
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.status, PlanStatus::Active);
        assert_eq!(fetched.target_branch, "feat/test");
    }

    #[test]
    fn activate_plan_on_closed_returns_invalid_transition() {
        // C4: a Closed plan has no valid transition out of Closed;
        // activate_plan must return InvalidTransition (not the
        // semantically-wrong DigestConflict).
        let store = InMemoryDagSchedulerStore::new();
        store.register_plan(&plan("p1", "d1")).expect("register");
        // Manually close the entry by mutating through the
        // internal map — there is no public close API on the
        // in-memory store (per contract, Closed is terminal).
        {
            let mut guard = store.plans.lock().expect("InMemoryDagSchedulerStore mutex");
            guard.get_mut("p1").expect("entry").status = PlanStatus::Closed;
        }
        let err = store
            .activate_plan("p1", "feat/test")
            .expect_err("closed must reject activation");
        match err {
            DagStoreError::InvalidTransition {
                plan_key,
                expected,
                actual,
            } => {
                assert_eq!(plan_key, "p1");
                assert_eq!(expected, "active_or_pending");
                assert_eq!(actual, "plan is closed");
            }
            other => panic!("expected InvalidTransition, got {other:?}"),
        }
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.status, PlanStatus::Closed);
    }

    // ─────────────────────────────────────────────────────────────────
    // 2026-09-09-0917 plan U3: per-Unit base commit pin +
    // per-stage accepted evidence ledger contract suite. The
    // helpers below are mirrored byte-for-byte against the
    // rusqlite variant so a contract regression on either side
    // surfaces the same test name.
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn u3_pin_roundtrip() {
        let store = InMemoryDagSchedulerStore::new();
        contract_tests::assert_pin_roundtrip(&store);
    }

    #[test]
    fn u3_pin_is_idempotent_on_same_base() {
        let store = InMemoryDagSchedulerStore::new();
        contract_tests::assert_pin_is_idempotent_on_same_base(&store);
    }

    #[test]
    fn u3_pin_fails_closed_on_base_drift() {
        let store = InMemoryDagSchedulerStore::new();
        contract_tests::assert_pin_fails_closed_on_base_drift(&store);
    }

    #[test]
    fn u3_get_returns_none_when_unpinned() {
        let store = InMemoryDagSchedulerStore::new();
        contract_tests::assert_get_returns_none_when_unpinned(&store);
    }

    #[test]
    fn u3_units_are_pinned_independently() {
        let store = InMemoryDagSchedulerStore::new();
        contract_tests::assert_units_are_pinned_independently(&store);
    }

    #[test]
    fn u3_evidence_roundtrip() {
        let store = InMemoryDagSchedulerStore::new();
        contract_tests::assert_evidence_roundtrip(&store);
    }

    #[test]
    fn u3_evidence_is_idempotent_on_same_record() {
        let store = InMemoryDagSchedulerStore::new();
        contract_tests::assert_evidence_is_idempotent_on_same_record(&store);
    }

    #[test]
    fn u3_latest_stage_evidence_picks_highest_attempt() {
        let store = InMemoryDagSchedulerStore::new();
        contract_tests::assert_latest_stage_evidence_picks_highest_attempt(&store);
    }

    #[test]
    fn u3_evidence_fails_closed_on_field_drift() {
        let store = InMemoryDagSchedulerStore::new();
        contract_tests::assert_evidence_fails_closed_on_field_drift(&store);
    }

    #[test]
    fn u3_stages_are_tracked_independently() {
        let store = InMemoryDagSchedulerStore::new();
        contract_tests::assert_stages_are_tracked_independently(&store);
    }
}
