//! 2026-09-03-0959 plan U3 (R17 / D4 / D17 / D18 / E5 / E7): bounded
//! registration receipt — durable since PMI-013 / Step 0.
//!
//! The runtime writes a `DagPlanReceipt` BEFORE
//! `ensure_task_projection` / `ack` so the projection window has
//! a recorded plan identity to key off. The receipt is bounded
//! (plan key / path / digest / target identity only) — it NEVER
//! carries the raw canonical artifact bytes (E9 / E16
//! receipt-content rule).
//!
//! **Durability (PMI-013 / Step 0):** receipts are durable through
//! the `dag_plan_receipts` table (migration v14). The
//! [`DagPlanReceiptStore`] trait is the contract; the rusqlite
//! implementation (`dag_store_rusqlite.rs`) is the authority, and
//! the in-memory implementation below keeps the same contract for
//! `dag_shadow` / non-`supervisor-db` builds. [`DagPlanReceiptRegistry`]
//! is the runtime-facing handle: it wraps any
//! `Arc<dyn DagPlanReceiptStore>`, keeps a write-through in-process
//! read cache for hot snapshots, and treats the store as the sole
//! authority — every read goes to the store and refreshes the
//! cache, so a reopen after a crash lists, activates, and consumes
//! the same receipts (S12 / S20).
//!
//! Lifecycle: `Pending → Active → Consumed`.
//! - `record` at the `forge.plan.ready` accepted boundary → `Pending`.
//! - `activate` when a later accepted approval activates the plan
//!   → `Active` (idempotent on `Active`).
//! - `consume` when recovery / projection has used the receipt →
//!   `Consumed` (idempotent on `Consumed`; terminal — activation of
//!   a consumed receipt fails closed).
//!
//! Idempotency (same semantics as `DagSchedulerStore`):
//! - `record(receipt)` with the same `(plan_key, artifact_digest)`
//!   returns `Ok(false)` (already recorded — no observable change).
//! - `record(receipt)` with the same `plan_key` but a DIFFERENT
//!   `artifact_digest` returns `Err(DigestConflict)` — fail closed
//!   so a stale event cannot silently overwrite an
//!   already-recorded plan identity.

// `result_large_err` is allowed at file scope (more granular than
// crate level) per U2 / F17: keep DagStoreError variants human-readable
// for fail-closed evidence while suppressing function-level
// `result_large_err` errors on every method returning
// `Result<_, DagStoreError>`.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use super::dag_store::{DagStoreError, DagStoreResult};

/// Lifecycle status of a registration receipt. Persisted as the
/// lowercase string in `dag_plan_receipts.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReceiptStatus {
    /// Recorded at the accepted boundary; not yet activated.
    Pending,
    /// A later accepted approval activated the receipt.
    Active,
    /// Recovery / projection consumed the receipt; terminal.
    Consumed,
}

impl ReceiptStatus {
    /// Stable lowercase string used in the SQLite `status` column.
    pub fn as_str(self) -> &'static str {
        match self {
            ReceiptStatus::Pending => "pending",
            ReceiptStatus::Active => "active",
            ReceiptStatus::Consumed => "consumed",
        }
    }
}

impl fmt::Display for ReceiptStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parse a `dag_plan_receipts.status` column value. Unknown values
/// fail closed as a storage error — never guess a status.
pub fn parse_receipt_status(raw: &str) -> DagStoreResult<ReceiptStatus> {
    match raw {
        "pending" => Ok(ReceiptStatus::Pending),
        "active" => Ok(ReceiptStatus::Active),
        "consumed" => Ok(ReceiptStatus::Consumed),
        other => Err(DagStoreError::IoError(format!(
            "corrupt dag_plan_receipts.status column: unknown status {other:?}"
        ))),
    }
}

/// Bounded registration receipt. Carries the minimum identity a
/// `forge.plan.ready` accepted boundary must record before
/// projecting tasks / acking the runtime. Durable in
/// `dag_plan_receipts` (migration v14); the DB row is the
/// authority and survives process restarts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagPlanReceipt {
    pub plan_key: String,
    pub artifact_path: String,
    pub artifact_digest: String,
    pub target_branch: String,
    pub status: ReceiptStatus,
    pub created_at_ms: u64,
    /// Set when the receipt transitioned to `Active`.
    pub activated_at_ms: Option<u64>,
    /// Set when the receipt transitioned to `Consumed`.
    pub consumed_at_ms: Option<u64>,
}

impl DagPlanReceipt {
    /// Build a fresh `Pending` receipt for the accepted boundary.
    pub fn new(
        plan_key: impl Into<String>,
        artifact_path: impl Into<String>,
        artifact_digest: impl Into<String>,
        target_branch: impl Into<String>,
        created_at_ms: u64,
    ) -> Self {
        Self {
            plan_key: plan_key.into(),
            artifact_path: artifact_path.into(),
            artifact_digest: artifact_digest.into(),
            target_branch: target_branch.into(),
            status: ReceiptStatus::Pending,
            created_at_ms,
            activated_at_ms: None,
            consumed_at_ms: None,
        }
    }
}

/// Durable registration receipt store contract. Memory and
/// rusqlite implementations share this trait; the same contract
/// test suite runs against both (mirroring the
/// `DagSchedulerStore` memory/rusqlite parity rule).
pub trait DagPlanReceiptStore: Send + Sync {
    /// Record a receipt. Returns `Ok(true)` when this is the first
    /// record for `plan_key`; `Ok(false)` when the same
    /// `(plan_key, artifact_digest)` was already recorded (no-op —
    /// the existing row is left untouched); `Err(DigestConflict)`
    /// when the same `plan_key` was recorded with a DIFFERENT
    /// digest (fail closed).
    fn record_receipt(&self, receipt: &DagPlanReceipt) -> DagStoreResult<bool>;

    /// Read a receipt by `plan_key`. Returns `Ok(None)` when
    /// nothing was recorded under that key.
    fn get_receipt(&self, plan_key: &str) -> DagStoreResult<Option<DagPlanReceipt>>;

    /// List every recorded receipt. Order is unspecified; callers
    /// that need deterministic ordering must sort.
    fn list_receipts(&self) -> DagStoreResult<Vec<DagPlanReceipt>>;

    /// Transition `Pending → Active` and stamp `activated_at_ms`.
    /// Re-activating an already-`Active` receipt is an idempotent
    /// no-op (returns the existing receipt). `UnknownPlan` when the
    /// key was never recorded; `InvalidTransition` when the receipt
    /// is already `Consumed` (no transition out of `Consumed`).
    fn activate_receipt(
        &self,
        plan_key: &str,
        activated_at_ms: u64,
    ) -> DagStoreResult<DagPlanReceipt>;

    /// Transition `Pending | Active → Consumed` and stamp
    /// `consumed_at_ms` (a receipt recorded at the accepted
    /// boundary may be consumed without a separate activation when
    /// recovery replays the projection directly). Idempotent on an
    /// already-`Consumed` receipt. `UnknownPlan` when the key was
    /// never recorded.
    fn consume_receipt(
        &self,
        plan_key: &str,
        consumed_at_ms: u64,
    ) -> DagStoreResult<DagPlanReceipt>;
}

/// In-memory [`DagPlanReceiptStore`]. Backs the contract for tests
/// and for the `dag_shadow` runtime mode, which needs a working
/// store without touching SQLite. A single `Mutex` serialises every
/// read-decide-write cycle.
#[derive(Debug, Default)]
pub struct InMemoryDagPlanReceiptStore {
    receipts: Mutex<HashMap<String, DagPlanReceipt>>,
}

impl InMemoryDagPlanReceiptStore {
    /// Build an empty in-memory receipt store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl DagPlanReceiptStore for InMemoryDagPlanReceiptStore {
    fn record_receipt(&self, receipt: &DagPlanReceipt) -> DagStoreResult<bool> {
        let mut guard = self
            .receipts
            .lock()
            .expect("InMemoryDagPlanReceiptStore mutex");
        if let Some(existing) = guard.get(&receipt.plan_key) {
            if existing.artifact_digest == receipt.artifact_digest {
                return Ok(false);
            }
            return Err(DagStoreError::DigestConflict {
                plan_key: receipt.plan_key.clone(),
                expected: existing.artifact_digest.clone(),
                actual: receipt.artifact_digest.clone(),
            });
        }
        guard.insert(receipt.plan_key.clone(), receipt.clone());
        Ok(true)
    }

    fn get_receipt(&self, plan_key: &str) -> DagStoreResult<Option<DagPlanReceipt>> {
        let guard = self
            .receipts
            .lock()
            .expect("InMemoryDagPlanReceiptStore mutex");
        Ok(guard.get(plan_key).cloned())
    }

    fn list_receipts(&self) -> DagStoreResult<Vec<DagPlanReceipt>> {
        let guard = self
            .receipts
            .lock()
            .expect("InMemoryDagPlanReceiptStore mutex");
        Ok(guard.values().cloned().collect())
    }

    fn activate_receipt(
        &self,
        plan_key: &str,
        activated_at_ms: u64,
    ) -> DagStoreResult<DagPlanReceipt> {
        let mut guard = self
            .receipts
            .lock()
            .expect("InMemoryDagPlanReceiptStore mutex");
        let entry = guard
            .get_mut(plan_key)
            .ok_or_else(|| DagStoreError::UnknownPlan(plan_key.to_string()))?;
        match entry.status {
            ReceiptStatus::Consumed => Err(DagStoreError::InvalidTransition {
                plan_key: plan_key.to_string(),
                expected: "pending_or_active".to_string(),
                actual: "receipt is consumed".to_string(),
            }),
            ReceiptStatus::Active => Ok(entry.clone()),
            ReceiptStatus::Pending => {
                entry.status = ReceiptStatus::Active;
                entry.activated_at_ms = Some(activated_at_ms);
                Ok(entry.clone())
            }
        }
    }

    fn consume_receipt(
        &self,
        plan_key: &str,
        consumed_at_ms: u64,
    ) -> DagStoreResult<DagPlanReceipt> {
        let mut guard = self
            .receipts
            .lock()
            .expect("InMemoryDagPlanReceiptStore mutex");
        let entry = guard
            .get_mut(plan_key)
            .ok_or_else(|| DagStoreError::UnknownPlan(plan_key.to_string()))?;
        match entry.status {
            ReceiptStatus::Consumed => Ok(entry.clone()),
            ReceiptStatus::Pending | ReceiptStatus::Active => {
                entry.status = ReceiptStatus::Consumed;
                entry.consumed_at_ms = Some(consumed_at_ms);
                Ok(entry.clone())
            }
        }
    }
}

/// Runtime-facing registration receipt registry. Wraps any
/// [`DagPlanReceiptStore`] backend: with the rusqlite backend the
/// receipts are durable (the SQLite row is the sole authority);
/// the in-process cache is a write-through read mirror refreshed on
/// every operation, for hot snapshots that must not hit the store.
///
/// Cross-process note: the cache only observes writes made through
/// THIS registry. `get` / `list_all` always consult the store
/// (the authority) and refresh the cache, so they never serve
/// stale entries; [`DagPlanReceiptRegistry::cached`] is the cheap
/// in-process mirror and may lag writes from other processes.
pub struct DagPlanReceiptRegistry {
    store: Arc<dyn DagPlanReceiptStore>,
    cache: Mutex<HashMap<String, DagPlanReceipt>>,
}

impl fmt::Debug for DagPlanReceiptRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DagPlanReceiptRegistry").finish()
    }
}

impl DagPlanReceiptRegistry {
    /// Build a registry over an arbitrary receipt store backend.
    pub fn new(store: Arc<dyn DagPlanReceiptStore>) -> Self {
        Self {
            store,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Open (creating if needed) a durable registry backed by the
    /// `dag_plan_receipts` table at `path`. Migrations run on every
    /// open; already-current databases are a no-op.
    #[cfg(feature = "supervisor-db")]
    pub fn open(path: impl AsRef<std::path::Path>) -> DagStoreResult<Self> {
        Ok(Self::new(Arc::new(
            super::dag_store_rusqlite::RusqliteDagPlanReceiptStore::open(path)?,
        )))
    }

    /// Refresh the in-process cache entry for one receipt.
    fn cache_put(&self, receipt: DagPlanReceipt) {
        let mut guard = self
            .cache
            .lock()
            .expect("DagPlanReceiptRegistry cache mutex");
        guard.insert(receipt.plan_key.clone(), receipt);
    }

    /// Record a receipt at the accepted boundary. See
    /// [`DagPlanReceiptStore::record_receipt`] for the idempotency
    /// contract. The store is the authority; the returned bool and
    /// the cache entry reflect the durable row.
    pub fn record(&self, receipt: DagPlanReceipt) -> DagStoreResult<bool> {
        let recorded = self.store.record_receipt(&receipt)?;
        if let Some(row) = self.store.get_receipt(&receipt.plan_key)? {
            self.cache_put(row);
        }
        Ok(recorded)
    }

    /// Read a receipt by `plan_key` from the store (authority) and
    /// refresh the cache. Returns `Ok(None)` when nothing was
    /// recorded under that key.
    pub fn get(&self, plan_key: &str) -> DagStoreResult<Option<DagPlanReceipt>> {
        let row = self.store.get_receipt(plan_key)?;
        if let Some(row) = &row {
            self.cache_put(row.clone());
        }
        Ok(row)
    }

    /// List every recorded receipt from the store (authority) and
    /// refresh the cache. Order is unspecified; callers that need
    /// deterministic ordering must sort.
    pub fn list_all(&self) -> DagStoreResult<Vec<DagPlanReceipt>> {
        let rows = self.store.list_receipts()?;
        for row in &rows {
            self.cache_put(row.clone());
        }
        Ok(rows)
    }

    /// Transition `Pending → Active`. See
    /// [`DagPlanReceiptStore::activate_receipt`] for the transition
    /// contract. Returns the post-transition receipt.
    pub fn activate(&self, plan_key: &str, activated_at_ms: u64) -> DagStoreResult<DagPlanReceipt> {
        let row = self.store.activate_receipt(plan_key, activated_at_ms)?;
        self.cache_put(row.clone());
        Ok(row)
    }

    /// Transition to `Consumed` (terminal). See
    /// [`DagPlanReceiptStore::consume_receipt`] for the transition
    /// contract. Returns the post-transition receipt.
    pub fn consume(&self, plan_key: &str, consumed_at_ms: u64) -> DagStoreResult<DagPlanReceipt> {
        let row = self.store.consume_receipt(plan_key, consumed_at_ms)?;
        self.cache_put(row.clone());
        Ok(row)
    }

    /// In-process cache mirror: the last receipt this registry
    /// observed for `plan_key`, without touching the store. May lag
    /// writes from OTHER processes — use [`DagPlanReceiptRegistry::get`]
    /// when the authoritative row matters.
    pub fn cached(&self, plan_key: &str) -> Option<DagPlanReceipt> {
        let guard = self
            .cache
            .lock()
            .expect("DagPlanReceiptRegistry cache mutex");
        guard.get(plan_key).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(key: &str, digest: &str) -> DagPlanReceipt {
        DagPlanReceipt::new(
            key,
            format!("/tmp/{key}.yaml"),
            digest,
            "feat/test",
            1_700_000_000_000,
        )
    }

    fn memory_store() -> InMemoryDagPlanReceiptStore {
        InMemoryDagPlanReceiptStore::new()
    }

    // -----------------------------------------------------------------
    // Receipt store contract (mirrored by the rusqlite suite in
    // dag_store_rusqlite.rs — the same contract runs against both).
    // -----------------------------------------------------------------

    #[test]
    fn receipt_new_starts_pending() {
        let r = receipt("p1", "d1");
        assert_eq!(r.status, ReceiptStatus::Pending);
        assert_eq!(r.activated_at_ms, None);
        assert_eq!(r.consumed_at_ms, None);
    }

    #[test]
    fn record_returns_true_for_first_record() {
        let store = memory_store();
        assert!(store.record_receipt(&receipt("p1", "d1")).expect("record"));
    }

    #[test]
    fn record_is_idempotent_on_same_payload() {
        let store = memory_store();
        assert!(store.record_receipt(&receipt("p1", "d1")).expect("first"));
        assert!(
            !store
                .record_receipt(&receipt("p1", "d1"))
                .expect("idempotent")
        );
        // The existing row is left untouched — a replay with a
        // different created_at_ms must not overwrite it.
        let mut replayed = receipt("p1", "d1");
        replayed.created_at_ms = 42;
        assert!(!store.record_receipt(&replayed).expect("idempotent"));
        let fetched = store.get_receipt("p1").expect("get").expect("exists");
        assert_eq!(fetched.created_at_ms, 1_700_000_000_000);
    }

    #[test]
    fn record_fails_closed_on_digest_conflict() {
        let store = memory_store();
        store.record_receipt(&receipt("p1", "d1")).expect("first");
        let err = store
            .record_receipt(&receipt("p1", "d2"))
            .expect_err("conflict");
        assert!(matches!(
            err,
            DagStoreError::DigestConflict {
                plan_key,
                expected,
                actual,
            } if plan_key == "p1" && expected == "d1" && actual == "d2"
        ));
    }

    #[test]
    fn get_returns_recorded_receipt() {
        let store = memory_store();
        let r = receipt("p1", "d1");
        store.record_receipt(&r).expect("record");
        let fetched = store.get_receipt("p1").expect("get").expect("exists");
        assert_eq!(fetched, r);
    }

    #[test]
    fn get_returns_none_for_missing_receipt() {
        let store = memory_store();
        assert!(store.get_receipt("missing").expect("get").is_none());
    }

    #[test]
    fn list_all_returns_every_recorded_receipt() {
        let store = memory_store();
        store.record_receipt(&receipt("p1", "d1")).expect("p1");
        store.record_receipt(&receipt("p2", "d2")).expect("p2");
        let all = store.list_receipts().expect("list");
        assert_eq!(all.len(), 2);
        let mut keys: Vec<&str> = all.iter().map(|r| r.plan_key.as_str()).collect();
        keys.sort();
        assert_eq!(keys, vec!["p1", "p2"]);
    }

    #[test]
    fn activate_transitions_pending_to_active() {
        let store = memory_store();
        store.record_receipt(&receipt("p1", "d1")).expect("record");
        let activated = store
            .activate_receipt("p1", 1_700_000_001_000)
            .expect("activate");
        assert_eq!(activated.status, ReceiptStatus::Active);
        assert_eq!(activated.activated_at_ms, Some(1_700_000_001_000));
    }

    #[test]
    fn activate_is_idempotent_on_active() {
        let store = memory_store();
        store.record_receipt(&receipt("p1", "d1")).expect("record");
        let first = store
            .activate_receipt("p1", 1_700_000_001_000)
            .expect("activate");
        let second = store
            .activate_receipt("p1", 1_700_000_002_000)
            .expect("no-op second activate");
        assert_eq!(second, first, "re-activation must not move the stamp");
    }

    #[test]
    fn activate_unknown_key_returns_error() {
        let store = memory_store();
        let err = store
            .activate_receipt("missing", 1_700_000_001_000)
            .expect_err("err");
        assert!(matches!(err, DagStoreError::UnknownPlan(_)));
    }

    #[test]
    fn activate_consumed_returns_invalid_transition() {
        let store = memory_store();
        store.record_receipt(&receipt("p1", "d1")).expect("record");
        store
            .consume_receipt("p1", 1_700_000_001_000)
            .expect("consume");
        let err = store
            .activate_receipt("p1", 1_700_000_002_000)
            .expect_err("consumed must reject activation");
        assert!(matches!(err, DagStoreError::InvalidTransition { .. }));
        let fetched = store.get_receipt("p1").expect("get").expect("exists");
        assert_eq!(fetched.status, ReceiptStatus::Consumed);
    }

    #[test]
    fn consume_transitions_pending_to_consumed() {
        let store = memory_store();
        store.record_receipt(&receipt("p1", "d1")).expect("record");
        let consumed = store
            .consume_receipt("p1", 1_700_000_001_000)
            .expect("consume");
        assert_eq!(consumed.status, ReceiptStatus::Consumed);
        assert_eq!(consumed.consumed_at_ms, Some(1_700_000_001_000));
        assert_eq!(consumed.activated_at_ms, None);
    }

    #[test]
    fn consume_from_active_keeps_activation_stamp() {
        let store = memory_store();
        store.record_receipt(&receipt("p1", "d1")).expect("record");
        store
            .activate_receipt("p1", 1_700_000_001_000)
            .expect("activate");
        let consumed = store
            .consume_receipt("p1", 1_700_000_002_000)
            .expect("consume");
        assert_eq!(consumed.status, ReceiptStatus::Consumed);
        assert_eq!(consumed.activated_at_ms, Some(1_700_000_001_000));
        assert_eq!(consumed.consumed_at_ms, Some(1_700_000_002_000));
    }

    #[test]
    fn consume_is_idempotent_on_consumed() {
        let store = memory_store();
        store.record_receipt(&receipt("p1", "d1")).expect("record");
        let first = store
            .consume_receipt("p1", 1_700_000_001_000)
            .expect("consume");
        let second = store
            .consume_receipt("p1", 1_700_000_002_000)
            .expect("no-op second consume");
        assert_eq!(second, first, "re-consume must not move the stamp");
    }

    #[test]
    fn consume_unknown_key_returns_error() {
        let store = memory_store();
        let err = store
            .consume_receipt("missing", 1_700_000_001_000)
            .expect_err("err");
        assert!(matches!(err, DagStoreError::UnknownPlan(_)));
    }

    // -----------------------------------------------------------------
    // Registry (backend-agnostic handle over Arc<dyn DagPlanReceiptStore>).
    // -----------------------------------------------------------------

    fn memory_registry() -> DagPlanReceiptRegistry {
        DagPlanReceiptRegistry::new(Arc::new(InMemoryDagPlanReceiptStore::new()))
    }

    #[test]
    fn registry_record_get_list_roundtrip_through_store() {
        let reg = memory_registry();
        assert!(reg.record(receipt("p1", "d1")).expect("record"));
        assert!(!reg.record(receipt("p1", "d1")).expect("idempotent"));
        let fetched = reg.get("p1").expect("get").expect("exists");
        assert_eq!(fetched, receipt("p1", "d1"));
        assert_eq!(reg.list_all().expect("list").len(), 1);
        // The in-process cache mirrors the store after each op.
        assert_eq!(reg.cached("p1"), Some(receipt("p1", "d1")));
        assert!(reg.cached("missing").is_none());
    }

    #[test]
    fn registry_delegates_digest_conflict_fail_closed() {
        let reg = memory_registry();
        reg.record(receipt("p1", "d1")).expect("record");
        let err = reg.record(receipt("p1", "d2")).expect_err("conflict");
        assert!(matches!(err, DagStoreError::DigestConflict { .. }));
    }

    #[test]
    fn registry_activate_consume_flow_updates_cache() {
        let reg = memory_registry();
        reg.record(receipt("p1", "d1")).expect("record");
        let activated = reg.activate("p1", 1_700_000_001_000).expect("activate");
        assert_eq!(activated.status, ReceiptStatus::Active);
        assert_eq!(
            reg.cached("p1").expect("cached").status,
            ReceiptStatus::Active
        );
        let consumed = reg.consume("p1", 1_700_000_002_000).expect("consume");
        assert_eq!(consumed.status, ReceiptStatus::Consumed);
        assert_eq!(
            reg.cached("p1").expect("cached").status,
            ReceiptStatus::Consumed
        );
        // Consumed is terminal: activation fails closed.
        let err = reg.activate("p1", 1_700_000_003_000).expect_err("err");
        assert!(matches!(err, DagStoreError::InvalidTransition { .. }));
    }
}
