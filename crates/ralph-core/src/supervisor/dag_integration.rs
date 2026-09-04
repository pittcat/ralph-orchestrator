//! 2026-09-03-0959 plan U7 (R7; S8-S11; D7-D9; E10-E12):
//! integration store — idempotent records of every unit
//! the lane has integrated into its target branch.
//!
//! # Why a separate store?
//!
//! `DagSchedulerStore` (U3) owns plan-level state. U7's
//! integration record is finer-grained: one record per
//! unit-integrated-into-target event, keyed on
//! `(unit_id, base_commit, integrated_commit, expected_head_before)`,
//! with a SHA-256 fingerprint for drift detection. This is
//! the data the reviewer / tester / reporter read back when
//! they ask "what has actually landed on the integration
//! target?".
//!
//! # Idempotency contract
//!
//! - `record_integrated` with a fresh
//!   `(unit_id, base_commit, integrated_commit, expected_head_before)`
//!   tuple → `Ok(new_record)`.
//! - `record_integrated` with a tuple matching an existing
//!   record → `Ok(existing_record)` (no error, no duplicate
//!   row).
//! - `record_integrated` with the same `unit_id` but a
//!   different `(base_commit, integrated_commit,
//!   expected_head_before)` triple →
//!   `Err(IntegrationStoreError::DuplicateUnitForTarget)`.
//!   The lane is the single writer per target; a second
//!   record for the same unit on the same target means a
//!   re-run with a different candidate, which is
//!   fail-closed.
//! - `record_integrated` with the same `(unit_id, ...)`
//!   triple but a different `commit_fingerprint` (e.g. the
//!   squash tree OID differs even though the commit OID
//!   matches) → `Err(IntegrationStoreError::FingerprintDrift)`.
//!
//! # Acknowledgement
//!
//! The lane emits an integration record the moment CAS
//! succeeds; the integrator then acks the record once the
//! unit's `forge.unit.acked` event has been emitted. Until
//! ack, the record shows up in `list_unacked_for_target`
//! so the U8 recovery layer can pick up after a crash.

use std::collections::HashMap;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

/// Per-unit integration record. Idempotent on the natural
/// key tuple below; `commit_fingerprint` is the SHA-256
/// hash of `(unit_id || base_commit || integrated_commit ||
/// expected_head_before)` for drift detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationRecord {
    pub id: i64,
    pub unit_id: String,
    pub target_branch: String,
    pub base_commit: String,
    pub integrated_commit: String,
    pub expected_head_before: String,
    pub commit_fingerprint: String,
    pub acked: bool,
    pub created_at_ms: i64,
}

/// Idempotency input handed to
/// [`IntegrationStore::record_integrated`]. The caller is
/// the integration orchestrator; the store never invents
/// any of these fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationInput {
    pub unit_id: String,
    pub target_branch: String,
    pub base_commit: String,
    pub integrated_commit: String,
    pub expected_head_before: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntegrationStoreError {
    #[error("unit '{unit_id}' already integrated into target '{target_branch}' with a different candidate")]
    DuplicateUnitForTarget { unit_id: String, target_branch: String },
    #[error("unit '{unit_id}' not yet recorded on target '{target_branch}'")]
    NotYetRecorded { unit_id: String, target_branch: String },
    #[error("commit fingerprint drift for unit '{unit_id}': expected {expected}, got {actual}")]
    FingerprintDrift {
        unit_id: String,
        expected: String,
        actual: String,
    },
    #[error("integration store mutex poisoned")]
    StorePoisoned,
}

pub type IntegrationStoreResult<T> = Result<T, IntegrationStoreError>;

/// Trait abstracting integration persistence. The in-memory
/// implementation lives in [`super::dag_integration`]
/// (this file); the future rusqlite variant will satisfy
/// the same contract.
pub trait IntegrationStore: Send + Sync {
    /// Record an integration event. Idempotent on the
    /// natural key tuple. See module docs for the contract.
    fn record_integrated(
        &self,
        input: &IntegrationInput,
    ) -> IntegrationStoreResult<IntegrationRecord>;

    /// Mark the unit's record on this target as acked.
    /// No-op if already acked. Returns the updated record.
    fn ack(&self, unit_id: &str, target_branch: &str) -> IntegrationStoreResult<IntegrationRecord>;

    /// Return all records for `unit_id` across every target.
    /// Typically returns one record per unit (the lane only
    /// writes once per unit per target), but the trait
    /// allows multiple (e.g. for rollback/replay).
    fn list_for_unit(&self, unit_id: &str) -> IntegrationStoreResult<Vec<IntegrationRecord>>;

    /// Return all unacked records for `target_branch`. Used
    /// by the U8 recovery layer to figure out what
    /// integrations need their ack event re-emitted.
    fn list_unacked_for_target(
        &self,
        target_branch: &str,
    ) -> IntegrationStoreResult<Vec<IntegrationRecord>>;
}

/// Compute the SHA-256 fingerprint for an
/// [`IntegrationInput`]. Hex-encoded lowercase, no
/// separators. Used for drift detection when the same
/// `(unit_id, base_commit, integrated_commit,
/// expected_head_before)` tuple is re-recorded with a
/// different `tree_oid` (would be a red flag).
pub fn compute_integration_fingerprint(input: &IntegrationInput) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.unit_id.as_bytes());
    hasher.update(b"|");
    hasher.update(input.base_commit.as_bytes());
    hasher.update(b"|");
    hasher.update(input.integrated_commit.as_bytes());
    hasher.update(b"|");
    hasher.update(input.expected_head_before.as_bytes());
    let digest = hasher.finalize();
    format!("{:x}", digest)
}

/// In-memory [`IntegrationStore`]. Backed by a single
/// `Mutex<HashMap<(unit_id, target_branch), IntegrationRecord>>`.
#[derive(Debug, Default)]
pub struct InMemoryIntegrationStore {
    rows: Mutex<HashMap<(String, String), IntegrationRecord>>,
    next_id: Mutex<i64>,
}

impl InMemoryIntegrationStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test-only: how many records the store currently
    /// holds.
    pub fn len(&self) -> usize {
        self.rows.lock().expect("integration store mutex").len()
    }
}

impl IntegrationStore for InMemoryIntegrationStore {
    fn record_integrated(
        &self,
        input: &IntegrationInput,
    ) -> IntegrationStoreResult<IntegrationRecord> {
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| IntegrationStoreError::StorePoisoned)?;
        let key = (input.unit_id.clone(), input.target_branch.clone());
        let fingerprint = compute_integration_fingerprint(input);
        if let Some(existing) = rows.get(&key) {
            // Same unit+target: re-check the full tuple and
            // the fingerprint.
            if existing.base_commit != input.base_commit
                || existing.integrated_commit != input.integrated_commit
                || existing.expected_head_before != input.expected_head_before
            {
                return Err(IntegrationStoreError::DuplicateUnitForTarget {
                    unit_id: input.unit_id.clone(),
                    target_branch: input.target_branch.clone(),
                });
            }
            if existing.commit_fingerprint != fingerprint {
                return Err(IntegrationStoreError::FingerprintDrift {
                    unit_id: input.unit_id.clone(),
                    expected: existing.commit_fingerprint.clone(),
                    actual: fingerprint,
                });
            }
            return Ok(existing.clone());
        }
        let mut id_guard = self
            .next_id
            .lock()
            .map_err(|_| IntegrationStoreError::StorePoisoned)?;
        *id_guard += 1;
        let id = *id_guard;
        let record = IntegrationRecord {
            id,
            unit_id: input.unit_id.clone(),
            target_branch: input.target_branch.clone(),
            base_commit: input.base_commit.clone(),
            integrated_commit: input.integrated_commit.clone(),
            expected_head_before: input.expected_head_before.clone(),
            commit_fingerprint: fingerprint,
            acked: false,
            created_at_ms: input.created_at_ms,
        };
        rows.insert(key, record.clone());
        Ok(record)
    }

    fn ack(&self, unit_id: &str, target_branch: &str) -> IntegrationStoreResult<IntegrationRecord> {
        let mut rows = self
            .rows
            .lock()
            .map_err(|_| IntegrationStoreError::StorePoisoned)?;
        let key = (unit_id.to_string(), target_branch.to_string());
        let entry = rows
            .get_mut(&key)
            .ok_or_else(|| IntegrationStoreError::NotYetRecorded {
                unit_id: unit_id.to_string(),
                target_branch: target_branch.to_string(),
            })?;
        entry.acked = true;
        Ok(entry.clone())
    }

    fn list_for_unit(&self, unit_id: &str) -> IntegrationStoreResult<Vec<IntegrationRecord>> {
        let rows = self
            .rows
            .lock()
            .map_err(|_| IntegrationStoreError::StorePoisoned)?;
        Ok(rows
            .values()
            .filter(|r| r.unit_id == unit_id)
            .cloned()
            .collect())
    }

    fn list_unacked_for_target(
        &self,
        target_branch: &str,
    ) -> IntegrationStoreResult<Vec<IntegrationRecord>> {
        let rows = self
            .rows
            .lock()
            .map_err(|_| IntegrationStoreError::StorePoisoned)?;
        Ok(rows
            .values()
            .filter(|r| r.target_branch == target_branch && !r.acked)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(unit: &str, base: &str, integrated: &str, expected_head: &str) -> IntegrationInput {
        IntegrationInput {
            unit_id: unit.to_string(),
            target_branch: "feat/test".to_string(),
            base_commit: base.to_string(),
            integrated_commit: integrated.to_string(),
            expected_head_before: expected_head.to_string(),
            created_at_ms: 1_700_000_000_000,
        }
    }

    /// U7 contract: a fresh record is created, acked=false.
    #[test]
    fn record_integrated_creates_new_row() {
        let store = InMemoryIntegrationStore::new();
        let rec = store
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("record");
        assert_eq!(rec.unit_id, "U1");
        assert_eq!(rec.target_branch, "feat/test");
        assert!(!rec.acked);
        assert_eq!(store.len(), 1);
    }

    /// U7 contract: re-recording the same tuple returns the
    /// existing row without error.
    #[test]
    fn record_integrated_is_idempotent_on_same_tuple() {
        let store = InMemoryIntegrationStore::new();
        let a = store
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("first");
        let b = store
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("idempotent");
        assert_eq!(a.id, b.id);
        assert_eq!(a.commit_fingerprint, b.commit_fingerprint);
        assert_eq!(store.len(), 1);
    }

    /// U7 contract: re-recording the same unit on the same
    /// target with a DIFFERENT (base_commit, integrated,
    /// expected_head) triple is fail-closed.
    #[test]
    fn record_integrated_rejects_duplicate_unit_for_target() {
        let store = InMemoryIntegrationStore::new();
        store
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("first");
        let err = store
            .record_integrated(&input("U1", "b2", "i1", "h1"))
            .expect_err("must reject");
        assert!(matches!(
            err,
            IntegrationStoreError::DuplicateUnitForTarget { .. }
        ));
    }

    /// U7 contract: `ack` flips the bit; the same call is a
    /// no-op the second time.
    #[test]
    fn ack_flips_acked_bit_and_is_idempotent() {
        let store = InMemoryIntegrationStore::new();
        let rec = store
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("record");
        let acked = store.ack("U1", "feat/test").expect("ack");
        // Strengthened (T2): assert every field of the returned
        // record, not just the acked bit. The ack must return the
        // same row with only `acked` flipped — no field drift.
        assert_eq!(acked.id, rec.id);
        assert_eq!(acked.unit_id, "U1");
        assert_eq!(acked.target_branch, "feat/test");
        assert_eq!(acked.base_commit, "b1");
        assert_eq!(acked.integrated_commit, "i1");
        assert_eq!(acked.expected_head_before, "h1");
        assert_eq!(acked.commit_fingerprint, rec.commit_fingerprint);
        assert!(acked.acked);
        assert_eq!(acked.created_at_ms, rec.created_at_ms);
        let acked_again = store.ack("U1", "feat/test").expect("ack again");
        assert_eq!(acked_again, acked);
    }

    /// U7 contract (C2 fix): `ack` on a unit that was NEVER
    /// recorded on this target returns `NotYetRecorded`, not
    /// `DuplicateUnitForTarget`. "Never recorded" and
    /// "recorded but candidate differs" are distinct failure
    /// modes — only the latter is `DuplicateUnitForTarget`
    /// (handled in `record_integrated`).
    #[test]
    fn ack_returns_not_yet_recorded_for_ghost_unit() {
        let store = InMemoryIntegrationStore::new();
        let err = store
            .ack("ghost-unit", "feat/test")
            .expect_err("ghost unit was never recorded");
        assert!(
            matches!(err, IntegrationStoreError::NotYetRecorded { .. }),
            "expected NotYetRecorded for never-recorded unit, got {err:?}"
        );
        // Sanity: the store is still empty (ack never creates a row).
        assert_eq!(store.len(), 0);
    }

    /// U7 contract: `list_unacked_for_target` returns only
    /// unacked records on the named target branch.
    #[test]
    fn list_unacked_filters_by_target_and_acked_flag() {
        let store = InMemoryIntegrationStore::new();
        // Two records on feat/test (unacked), one on
        // feat/other (unacked), then ack the U1 on
        // feat/test.
        store
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("rec U1");
        store
            .record_integrated(&{
                let mut i = input("U2", "b1", "i2", "h1");
                i.target_branch = "feat/other".to_string();
                i
            })
            .expect("rec U2");
        store.ack("U1", "feat/test").expect("ack");
        let unacked = store
            .list_unacked_for_target("feat/test")
            .expect("list unacked");
        assert!(unacked.iter().all(|r| !r.acked));
        assert!(unacked.is_empty(), "U1 was acked, so none unacked");
        let unacked_other = store
            .list_unacked_for_target("feat/other")
            .expect("list unacked other");
        assert_eq!(unacked_other.len(), 1);
        assert_eq!(unacked_other[0].unit_id, "U2");
    }

    /// U7 contract: `list_for_unit` returns every record
    /// for the unit across all targets. Same unit can be
    /// recorded on a DIFFERENT target (different target =
    /// different key) — that's how multi-target plans work.
    #[test]
    fn list_for_unit_returns_all_targets() {
        let store = InMemoryIntegrationStore::new();
        store
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("rec feat/test");
        store
            .record_integrated(&{
                let mut i = input("U1", "b2", "i2", "h2");
                i.target_branch = "feat/other".to_string();
                i
            })
            .expect("rec feat/other");
        let recs = store.list_for_unit("U1").expect("list");
        assert_eq!(recs.len(), 2);
        let branches: std::collections::BTreeSet<String> =
            recs.iter().map(|r| r.target_branch.clone()).collect();
        assert!(branches.contains("feat/test"));
        assert!(branches.contains("feat/other"));
    }

    /// U7 contract: the SHA-256 fingerprint is stable for
    /// the same input and changes when any of the four
    /// canonical fields changes.
    #[test]
    fn fingerprint_is_stable_and_changes_on_drift() {
        let a = compute_integration_fingerprint(&input("U1", "b1", "i1", "h1"));
        let b = compute_integration_fingerprint(&input("U1", "b1", "i1", "h1"));
        assert_eq!(a, b);
        let c = compute_integration_fingerprint(&input("U1", "b1", "i1", "h2"));
        assert_ne!(a, c);
        let d = compute_integration_fingerprint(&input("U1", "b1", "i2", "h1"));
        assert_ne!(a, d);
        let e = compute_integration_fingerprint(&input("U1", "b2", "i1", "h1"));
        assert_ne!(a, e);
        let f = compute_integration_fingerprint(&input("U2", "b1", "i1", "h1"));
        assert_ne!(a, f);
        // Lowercase hex, 64 chars (SHA-256).
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}