//! 2026-09-03-0959 plan U3 (R17 / D4 / D17 / E5 / E7): bounded
//! registration receipt.
//!
//! The runtime writes a `DagPlanReceipt` BEFORE
//! `ensure_task_projection` / `ack` so a crash in the projection
//! window can be reconstructed on resume without losing the
//! plan identity. The receipt is bounded (plan key / path /
//! digest / target identity only) — it NEVER carries the raw
//! canonical artifact bytes (E9 / E16 receipt-content rule).
//!
//! Activation lives on `DagSchedulerStore`; this registry is the
//! pre-write log. Idempotency:
//! - `record(receipt)` with the same `(plan_key, artifact_digest)`
//!   returns `false` (already recorded).
//! - `get(plan_key, artifact_digest)` returns `None` when nothing
//!   was recorded under that key.

use std::collections::HashMap;
use std::sync::Mutex;

/// Bounded registration receipt. Carries the minimum identity a
/// `forge.plan.ready` accepted boundary must durably record
/// before projecting tasks / acking the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagPlanReceipt {
    pub plan_key: String,
    pub artifact_path: String,
    pub artifact_digest: String,
    pub target_branch: String,
    pub created_at_ms: u64,
}

/// In-memory receipt registry. Records `(plan_key,
/// artifact_digest) → DagPlanReceipt`. Backs the bounded
/// pre-write log; the runtime writes a receipt here at the
/// accepted boundary BEFORE `ensure_task_projection` so a crash in
/// the projection window can be reconstructed on resume.
#[derive(Debug, Default)]
pub struct DagPlanReceiptRegistry {
    inner: Mutex<HashMap<(String, String), DagPlanReceipt>>,
}

impl DagPlanReceiptRegistry {
    /// Build an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a receipt. Returns `true` when this is the first
    /// record for `(plan_key, artifact_digest)`; `false`
    /// otherwise (idempotent — the same payload is silently
    /// overwritten with itself, so two identical `record` calls
    /// produce no observable difference).
    pub fn record(&self, receipt: DagPlanReceipt) -> bool {
        let key = (receipt.plan_key.clone(), receipt.artifact_digest.clone());
        let mut guard = self.inner.lock().expect("DagPlanReceiptRegistry mutex");
        guard.insert(key, receipt).is_none()
    }

    /// Read a previously-recorded receipt by `(plan_key,
    /// artifact_digest)`. Returns `None` when nothing was
    /// recorded under that key.
    pub fn get(&self, plan_key: &str, artifact_digest: &str) -> Option<DagPlanReceipt> {
        let guard = self.inner.lock().expect("DagPlanReceiptRegistry mutex");
        guard
            .get(&(plan_key.to_string(), artifact_digest.to_string()))
            .cloned()
    }

    /// List every recorded receipt. Order is unspecified; callers
    /// that need deterministic ordering must sort.
    pub fn list_all(&self) -> Vec<DagPlanReceipt> {
        let guard = self.inner.lock().expect("DagPlanReceiptRegistry mutex");
        guard.values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(key: &str, digest: &str) -> DagPlanReceipt {
        DagPlanReceipt {
            plan_key: key.to_string(),
            artifact_path: format!("/tmp/{key}.yaml"),
            artifact_digest: digest.to_string(),
            target_branch: "feat/test".to_string(),
            created_at_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn record_returns_true_for_first_record() {
        let reg = DagPlanReceiptRegistry::new();
        assert!(reg.record(receipt("p1", "d1")));
    }

    #[test]
    fn record_is_idempotent_on_same_payload() {
        let reg = DagPlanReceiptRegistry::new();
        assert!(reg.record(receipt("p1", "d1")));
        assert!(!reg.record(receipt("p1", "d1")));
    }

    #[test]
    fn get_returns_recorded_receipt() {
        let reg = DagPlanReceiptRegistry::new();
        let r = receipt("p1", "d1");
        reg.record(r.clone());
        let fetched = reg.get("p1", "d1").expect("exists");
        assert_eq!(fetched, r);
    }

    #[test]
    fn get_returns_none_for_missing_receipt() {
        let reg = DagPlanReceiptRegistry::new();
        assert!(reg.get("missing", "d1").is_none());
    }

    #[test]
    fn get_returns_none_when_digest_differs() {
        let reg = DagPlanReceiptRegistry::new();
        reg.record(receipt("p1", "d1"));
        assert!(reg.get("p1", "d2").is_none());
    }

    #[test]
    fn list_all_returns_every_recorded_receipt() {
        let reg = DagPlanReceiptRegistry::new();
        reg.record(receipt("p1", "d1"));
        reg.record(receipt("p2", "d2"));
        let all = reg.list_all();
        assert_eq!(all.len(), 2);
        let mut keys: Vec<&str> = all.iter().map(|r| r.plan_key.as_str()).collect();
        keys.sort();
        assert_eq!(keys, vec!["p1", "p2"]);
    }
}
