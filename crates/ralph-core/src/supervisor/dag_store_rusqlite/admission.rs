//! PMI-014 / 2026-09-09-0917 plan U2 (R2 / S2): durable
//! resource capacity oversubscription prevention.
//!
//! A Unit from first `reserve_job` to its final release must
//! hold typed (resource_key, permits) leases for the entire
//! pipeline span (execute → review → verify → fix →
//! integration). No two Units can ever push the held sum of any
//! resource past its declared capacity, even across
//! process / connection boundaries, because every
//! read-decide-write cycle runs inside one
//! `TransactionBehavior::Immediate` SQLite transaction. A claim
//! failure rolls back its own lease inserts AND any
//! co-committed `dag_jobs` / `dag_units` writes from the same
//! transaction, so an aborted Unit never leaks a half-applied
//! state into the durable journal (R2/S2: same-transaction
//! atomicity is the single source of truth).
//!
//! Three external entry points (plus a private in-tx helper):
//!
//! - [`claim_resources`] opens its own IMMEDIATE transaction
//!   and is the right shape for callers that have no other
//!   work to commit alongside the leases.
//! - [`held_permits`] and [`release_resources_for_unit`] are
//!   cheap single-statement reads/writes against
//!   `dag_resource_leases`.
//! - [`claim_resources_in_tx`] is the same logic as
//!   [`claim_resources`] but operates on a `Transaction` the
//!   caller already owns; `reserve_job` uses it so the lease
//!   writes share atomicity with the `dag_jobs` / `dag_units`
//!   inserts it performs.
//!
//! Idempotent re-claim for the same `(unit_key, resource_key)`
//! is encoded as `ON CONFLICT(unit_key,resource_key) DO
//! UPDATE`: a replay replaces `permits` and refreshes
//! `acquired_at_ms` instead of stacking a second row. The
//! held-sum check subtracts the unit's own current
//! contribution before comparing `other_held + permits` against
//! `capacity`, so an idempotent replay never trips the
//! oversubscribe gate on its own prior row.

use rusqlite::{TransactionBehavior, params};

use super::jobs::JobIdentity;
use super::{DagStoreError, DagStoreResult, RusqliteDagSchedulerStore, plan_io_err};

/// Bridge a lease-layer error into the unified store error
/// enum. The "resource oversubscribed" wording is part of the
/// U2 contract — the spawn path / DAG runtime match on this
/// prefix to convert capacity failures into admission rejection
/// without leaking storage internals.
fn admission_err(reason: impl Into<String>) -> DagStoreError {
    DagStoreError::IoError(format!("resource oversubscribed: {}", reason.into()))
}

impl RusqliteDagSchedulerStore {
    /// Same logic as [`Self::claim_resources`] but executed
    /// inside an existing `Transaction` so callers (currently
    /// `reserve_job`) can fold the lease writes into the same
    /// commit boundary as their own `dag_jobs` / `dag_units`
    /// inserts. The held-sum / capacity check uses the SAME
    /// transaction snapshot, so a concurrent committer between
    /// the SELECT and the INSERT can never race in: the
    /// surrounding `TransactionBehavior::Immediate` upgrade
    /// blocks it until we commit or roll back.
    pub(crate) fn claim_resources_in_tx(
        tx: &rusqlite::Transaction<'_>,
        identity: &JobIdentity,
        claims: &[(String, u32)],
        capacities: &[(String, u32)],
        now_ms: i64,
    ) -> DagStoreResult<()> {
        // No claims → nothing to do. The helper stays cheap
        // for callers that haven't wired capacity plumbing yet
        // (the default path: reserve_job keeps passing None).
        if claims.is_empty() {
            return Ok(());
        }
        // Build the capacity lookup once; reserve_job gets a
        // parallel slice pair, the standalone `claim_resources`
        // gets its own.
        let cap_map: std::collections::BTreeMap<&str, u32> = capacities
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        let unit_key = identity.unit_key();
        for (resource_key, permits) in claims {
            // Capacity must be declared for EVERY claimed
            // resource. A claim against an unknown key is
            // fail-closed: an undeclared resource has no bound
            // to constrain it, so letting it through would let a
            // plan silently inflate past supervisor ceilings.
            let capacity = cap_map
                .get(resource_key.as_str())
                .copied()
                .ok_or_else(|| admission_err(resource_key.as_str()))?;
            // Held sum across ALL units (we will subtract this
            // unit's own contribution in a moment).
            let held_total: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(permits), 0) FROM dag_resource_leases \
                     WHERE resource_key = ?1 AND status = 'held'",
                    params![resource_key.as_str()],
                    |row| row.get(0),
                )
                .map_err(plan_io_err)?;
            // Own contribution: an idempotent re-acquire
            // replaces this unit's held permits rather than
            // stacking, so we must subtract the unit's current
            // row before re-comparing against capacity.
            let own_held: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(permits), 0) FROM dag_resource_leases \
                     WHERE resource_key = ?1 AND unit_key = ?2 AND status = 'held'",
                    params![resource_key.as_str(), &unit_key],
                    |row| row.get(0),
                )
                .map_err(plan_io_err)?;
            let other_held = held_total.saturating_sub(own_held);
            // Same-transaction comparison. `permits` is u32,
            // `capacity` is u32, so widening to u64 is exact.
            let needed = other_held as u64 + *permits as u64;
            if needed > capacity as u64 {
                return Err(admission_err(resource_key.as_str()));
            }
            // Idempotent lease write. The UNIQUE constraint on
            // (unit_key, resource_key) lets `ON CONFLICT ...
            // DO UPDATE` replace this unit's contribution in
            // place; a first-time claim inserts a fresh row.
            tx.execute(
                "INSERT INTO dag_resource_leases \
                    (plan_key, unit_key, resource_key, permits, status, acquired_at_ms) \
                 VALUES (?1, ?2, ?3, ?4, 'held', ?5) \
                 ON CONFLICT(unit_key, resource_key) DO UPDATE SET \
                    permits = excluded.permits, \
                    acquired_at_ms = excluded.acquired_at_ms",
                params![
                    identity.plan_key,
                    unit_key,
                    resource_key.as_str(),
                    *permits as i64,
                    now_ms,
                ],
            )
            .map_err(plan_io_err)?;
        }
        Ok(())
    }

    /// Open an IMMEDIATE transaction, claim the listed
    /// resources for `identity`, and commit. Callers that need
    /// to fold the lease writes into a larger transaction
    /// (e.g. `reserve_job`) MUST use
    /// [`Self::claim_resources_in_tx`] instead; this public
    /// entry point is the standalone shape (single-claim,
    /// no co-commit).
    ///
    /// The capacity lookup is the caller's responsibility:
    /// capacities come from the plan's declared metadata, not
    /// from the durable schema. A claim against a key without a
    /// declared capacity fails closed — passing empty
    /// `capacities` while passing non-empty `claims` rejects
    /// every claim with `resource oversubscribed: <key>`.
    pub fn claim_resources(
        &self,
        identity: &JobIdentity,
        claims: &[(String, u32)],
        capacities: &[(String, u32)],
        now_ms: i64,
    ) -> DagStoreResult<()> {
        let mut conn = self
            .inner
            .conn
            .lock()
            .map_err(|_| DagStoreError::IoError("dag store mutex poisoned".into()))?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_io_err)?;
        Self::claim_resources_in_tx(&tx, identity, claims, capacities, now_ms)?;
        tx.commit().map_err(plan_io_err)
    }

    /// Mark every still-held lease for `unit_key` as
    /// `released` and stamp `released_at_ms`. Idempotent:
    /// releasing a unit whose leases are already released is a
    /// no-op (the second call updates zero rows). Lease rows
    /// are NOT deleted so recovery / audit reads can still see
    /// the full held-then-released timeline.
    pub fn release_resources_for_unit(
        &self,
        unit_key: &str,
        now_ms: i64,
    ) -> DagStoreResult<()> {
        let conn = self
            .inner
            .conn
            .lock()
            .map_err(|_| DagStoreError::IoError("dag store mutex poisoned".into()))?;
        conn.execute(
            "UPDATE dag_resource_leases \
             SET status = 'released', released_at_ms = ?1 \
             WHERE unit_key = ?2 AND status = 'held'",
            params![now_ms, unit_key],
        )
        .map_err(plan_io_err)?;
        Ok(())
    }

    /// Sum of held permits across all Units for
    /// `resource_key`. Released leases do NOT contribute. This
    /// is the diagnostic / admission-precheck probe; it is
    /// not load-bearing for correctness (admission uses the
    /// same statement under a transaction).
    pub fn held_permits(&self, resource_key: &str) -> DagStoreResult<i64> {
        let conn = self
            .inner
            .conn
            .lock()
            .map_err(|_| DagStoreError::IoError("dag store mutex poisoned".into()))?;
        conn.query_row(
            "SELECT COALESCE(SUM(permits), 0) FROM dag_resource_leases \
             WHERE resource_key = ?1 AND status = 'held'",
            params![resource_key],
            |row| row.get(0),
        )
        .map_err(plan_io_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::dag_store::{CanonicalPlanRecord, DagSchedulerStore};

    fn open_pair(dir: &tempfile::TempDir) -> (RusqliteDagSchedulerStore, RusqliteDagSchedulerStore) {
        // Two store handles, two SQLite connections, one file
        // — this is the cheapest faithful model of "two
        // supervisor processes racing the same DB" the schema
        // has to defend against.
        let primary = RusqliteDagSchedulerStore::open(dir.path().join("dag.db")).unwrap();
        let peer = RusqliteDagSchedulerStore::open(dir.path().join("dag.db")).unwrap();
        (primary, peer)
    }

    fn register(
        store: &RusqliteDagSchedulerStore,
        plan_key: &str,
        unit_ids: &[&str],
    ) -> JobIdentity {
        store
            .register_plan(&CanonicalPlanRecord {
                plan_key: plan_key.into(),
                artifact_digest: "digest".into(),
                target_branch: "main".into(),
                unit_ids: unit_ids.iter().map(|s| s.to_string()).collect(),
                created_at_ms: 0,
            })
            .unwrap();
        store.activate_plan(plan_key, "main").unwrap();
        JobIdentity {
            plan_key: plan_key.into(),
            unit_id: unit_ids[0].into(),
            job_id: format!("{}-job", unit_ids[0]),
            hat: "executor".into(),
            stage: "execute".into(),
            attempt: 0,
            token: format!("{}-nonce", unit_ids[0]),
        }
    }

    // -----------------------------------------------------------------
    // Step 1 acceptance: held sum survives across connections,
    // and a release on connection A is visible from connection B.
    // -----------------------------------------------------------------
    #[test]
    fn dag_resource_leases_survive_stages() {
        let dir = tempfile::tempdir().unwrap();
        let (store_a, store_b) = open_pair(&dir);
        let id = register(&store_a, "plan", &["U1"]);
        // Connection A claims; connection B reads.
        store_a
            .claim_resources(&id, &[("gpu".into(), 3u32)], &[("gpu".into(), 10u32)], 1)
            .unwrap();
        assert_eq!(store_b.held_permits("gpu").unwrap(), 3);
        // Connection B releases; connection A reads.
        store_b
            .release_resources_for_unit(&id.unit_key(), 2)
            .unwrap();
        assert_eq!(store_a.held_permits("gpu").unwrap(), 0);
        // Released leases do not count even after a reopen
        // (the held-sum filter is `status='held'`).
        drop(store_a);
        drop(store_b);
        let (_, reopened) = open_pair(&dir);
        assert_eq!(reopened.held_permits("gpu").unwrap(), 0);
    }

    // -----------------------------------------------------------------
    // Minimal happy path: two resources claimed in one call both
    // land, and held_permits returns the expected sum for each.
    // -----------------------------------------------------------------
    #[test]
    fn claim_resources_atomic_two_resources() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open_pair(&dir);
        let id = register(&store, "plan", &["U1"]);
        store
            .claim_resources(
                &id,
                &[("gpu".into(), 4u32), ("cpu".into(), 2u32)],
                &[("gpu".into(), 10u32), ("cpu".into(), 8u32)],
                1,
            )
            .unwrap();
        assert_eq!(store.held_permits("gpu").unwrap(), 4);
        assert_eq!(store.held_permits("cpu").unwrap(), 2);
    }

    // -----------------------------------------------------------------
    // Oversubscribe: the second claim pushes the held sum past
    // the declared capacity and is rejected. The first claim
    // stays intact (the rejection is its own short-circuited
    // transaction; nothing has been partially applied).
    // -----------------------------------------------------------------
    #[test]
    fn lease_oversubscribe_rejected_held_sum() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open_pair(&dir);
        let u1 = register(&store, "plan", &["U1"]);
        let mut u2 = u1.clone();
        u2.unit_id = "U2".into();
        u2.job_id = "U2-job".into();
        u2.token = "U2-nonce".into();
        store
            .claim_resources(&u1, &[("gpu".into(), 6u32)], &[("gpu".into(), 10u32)], 1)
            .unwrap();
        // U2 tries to take 5 of a 10-capacity resource with
        // 6 already held by U1 → must reject and leave U1's
        // lease untouched.
        let err = store
            .claim_resources(&u2, &[("gpu".into(), 5u32)], &[("gpu".into(), 10u32)], 2)
            .unwrap_err();
        assert!(
            format!("{err}").contains("resource oversubscribed"),
            "expected oversubscribed error, got {err:?}"
        );
        assert_eq!(store.held_permits("gpu").unwrap(), 6);
    }

    // -----------------------------------------------------------------
    // Release path: a held lease becomes released, the held sum
    // drops by exactly that many permits, and a follow-up claim
    // that would have been rejected before is now accepted.
    // -----------------------------------------------------------------
    #[test]
    fn lease_release_frees_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open_pair(&dir);
        let u1 = register(&store, "plan", &["U1"]);
        let mut u2 = u1.clone();
        u2.unit_id = "U2".into();
        u2.job_id = "U2-job".into();
        u2.token = "U2-nonce".into();
        store
            .claim_resources(&u1, &[("gpu".into(), 7u32)], &[("gpu".into(), 10u32)], 1)
            .unwrap();
        // U2 cannot enter while U1 holds 7 of 10.
        assert!(
            store
                .claim_resources(&u2, &[("gpu".into(), 4u32)], &[("gpu".into(), 10u32)], 2)
                .is_err()
        );
        store.release_resources_for_unit(&u1.unit_key(), 3).unwrap();
        assert_eq!(store.held_permits("gpu").unwrap(), 0);
        // Same claim now succeeds.
        store
            .claim_resources(&u2, &[("gpu".into(), 4u32)], &[("gpu".into(), 10u32)], 4)
            .unwrap();
        assert_eq!(store.held_permits("gpu").unwrap(), 4);
    }

    // -----------------------------------------------------------------
    // Idempotent re-claim: the same unit re-asserting the same
    // (resource_key, permits) replaces, never stacks. Held sum
    // reflects the latest permits value, and the same-transaction
    // check does not false-positive on the unit's own row.
    // -----------------------------------------------------------------
    #[test]
    fn lease_replay_no_double_permits() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open_pair(&dir);
        let id = register(&store, "plan", &["U1"]);
        store
            .claim_resources(&id, &[("gpu".into(), 5u32)], &[("gpu".into(), 10u32)], 1)
            .unwrap();
        assert_eq!(store.held_permits("gpu").unwrap(), 5);
        // Replay with the SAME permits: must not double-count.
        store
            .claim_resources(&id, &[("gpu".into(), 5u32)], &[("gpu".into(), 10u32)], 2)
            .unwrap();
        assert_eq!(store.held_permits("gpu").unwrap(), 5);
        // Replay with DIFFERENT permits: the row is REPLACED,
        // not summed, so held reflects the latest value.
        store
            .claim_resources(&id, &[("gpu".into(), 3u32)], &[("gpu".into(), 10u32)], 3)
            .unwrap();
        assert_eq!(store.held_permits("gpu").unwrap(), 3);
        // Replay with DIFFERENT permits within own-budget: the
        // row is REPLACED, not summed, so held reflects the
        // latest value. Subtracting the unit's own contribution
        // before comparing means an idempotent re-claim whose
        // new permits fit within capacity must succeed — even
        // when adding the new permits to the prior row would
        // have oversubscribed. The oversubscribe guard is
        // exercised separately by
        // `lease_oversubscribe_rejected_held_sum`.
        store
            .claim_resources(&id, &[("gpu".into(), 8u32)], &[("gpu".into(), 10u32)], 4)
            .unwrap();
        assert_eq!(store.held_permits("gpu").unwrap(), 8);
        // But if a different unit is already holding permits,
        // subtracting THIS unit's own row no longer hides the
        // other unit's contribution, and an oversubscribe is
        // correctly rejected.
        let id2 = register(&store, "plan", &["U2"]);
        store
            .claim_resources(&id2, &[("gpu".into(), 5u32)], &[("gpu".into(), 10u32)], 5)
            .unwrap_err();
    }

    // -----------------------------------------------------------------
    // Undeclared-capacity guard: a claim against a resource_key
    // that has no entry in the declared capacities map fails
    // closed. The DAG runtime cannot accept a claim that the
    // plan never bounded — otherwise a malicious / drifted
    // plan could silently inflate resources past supervisor
    // ceilings.
    // -----------------------------------------------------------------
    #[test]
    fn lease_unknown_capacity_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open_pair(&dir);
        let id = register(&store, "plan", &["U1"]);
        let err = store
            .claim_resources(
                &id,
                &[("unbounded".into(), 1u32)],
                &[("gpu".into(), 10u32)],
                1,
            )
            .unwrap_err();
        assert!(
            format!("{err}").contains("resource oversubscribed"),
            "expected oversubscribed error for undeclared capacity, got {err:?}"
        );
        assert_eq!(store.held_permits("gpu").unwrap(), 0);
        assert_eq!(store.held_permits("unbounded").unwrap(), 0);
    }
}
