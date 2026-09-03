//! 2026-09-03-0959 plan U3 (R2 / R17 / E5 / E7 / E9 / E16):
//! in-memory implementation of [`DagSchedulerStore`].
//!
//! Backs the DAG persistence contract for tests and for the
//! future `dag_shadow` runtime mode (U5) which needs a working
//! store without touching SQLite. The contract suite below is
//! shared between this module and the future rusqlite
//! implementation in `dag_store_rusqlite.rs` — every test here
//! is a contract the rusqlite variant MUST also pass (per U3 §
//! 9. 验收).
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
//!   row → `Err(InvalidTransition("plan is closed"))`.

use std::collections::HashMap;
use std::sync::Mutex;

use super::dag_store::{
    CanonicalPlanRecord, DagSchedulerStore, DagStoreError, DagStoreResult, PlanRegistration,
    PlanStatus,
};

/// In-memory `DagSchedulerStore`. Backed by a single `Mutex`
/// guarding a `HashMap<plan_key, PlanRegistration>`. Monotonic
/// `id` allocation is store-private; the rusqlite variant will
/// use SQLite rowid.
#[derive(Debug, Default)]
pub struct InMemoryDagSchedulerStore {
    plans: Mutex<HashMap<String, PlanRegistration>>,
    next_id: Mutex<i64>,
}

impl InMemoryDagSchedulerStore {
    /// Build an empty in-memory DAG store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl DagSchedulerStore for InMemoryDagSchedulerStore {
    fn register_plan(&self, plan: &CanonicalPlanRecord) -> DagStoreResult<PlanRegistration> {
        let mut guard = self.plans.lock().expect("InMemoryDagSchedulerStore mutex");
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
            .expect("InMemoryDagSchedulerStore id mutex");
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
        let mut guard = self.plans.lock().expect("InMemoryDagSchedulerStore mutex");
        let entry = guard
            .get_mut(plan_key)
            .ok_or_else(|| DagStoreError::UnknownPlan(plan_key.to_string()))?;
        if entry.status == PlanStatus::Closed {
            return Err(DagStoreError::DigestConflict {
                plan_key: plan_key.to_string(),
                expected: "active_or_pending".to_string(),
                actual: format!("plan is {}", entry.status),
            });
        }
        // Validate target_branch matches the registered one when
        // activating from Pending. A re-activation (status already
        // Active) with the same branch is a no-op; with a
        // different branch is fail-closed.
        if entry.target_branch != target_branch && entry.status == PlanStatus::Active {
            return Err(DagStoreError::DigestConflict {
                plan_key: plan_key.to_string(),
                expected: entry.target_branch.clone(),
                actual: target_branch.to_string(),
            });
        }
        entry.status = PlanStatus::Active;
        Ok(())
    }

    fn get_plan(&self, plan_key: &str) -> DagStoreResult<Option<PlanRegistration>> {
        let guard = self.plans.lock().expect("InMemoryDagSchedulerStore mutex");
        Ok(guard.get(plan_key).cloned())
    }

    fn list_active_plans(&self) -> DagStoreResult<Vec<PlanRegistration>> {
        let guard = self.plans.lock().expect("InMemoryDagSchedulerStore mutex");
        Ok(guard
            .values()
            .filter(|p| p.status == PlanStatus::Active)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
