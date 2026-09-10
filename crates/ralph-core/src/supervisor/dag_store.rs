//! 2026-09-03-0959 plan U3 (R2 / R17 / D4 / D17 / D18 / E5 / E7 / E9 / E16):
//! durable DAG store contract + bounded registration receipt.
//!
//! The trait surface here is the **minimum** that lets the runtime
//! execute the U3 acceptance contract (register → activate →
//! reopen-equivalent) without depending on the future rusqlite
//! implementation. Memory and rusqlite implementations share the
//! same `DagSchedulerStore` trait; the in-memory variant lands
//! here, the rusqlite variant lands in `dag_store_rusqlite.rs`
//! in a future Unit.
//!
//! `DagPlanReceiptRegistry` is a separate bounded pre-write
//! receipt surface — the runtime writes the receipt BEFORE
//! `ensure_task_projection` / `ack` so a crash in the projection
//! window can be reconstructed on resume without losing the
//! plan identity. The receipt itself is bounded (plan key / path
//! / digest / target identity only); raw payload never appears
//! here.

// `result_large_err` is allowed at file scope (more granular than
// crate level) per U2 / F17: keep DagStoreError variants human-readable
// for fail-closed evidence while suppressing function-level
// `result_large_err` errors on every method returning
// `Result<_, DagStoreError>`.
#![allow(clippy::result_large_err)]

use std::fmt;
use thiserror::Error;

/// Failure modes a `DagSchedulerStore` implementation can
/// surface. Implementations MUST return `DigestConflict` when the
/// same `plan_key` is re-registered with a DIFFERENT
/// `artifact_digest` — fail closed so a stale event cannot
/// silently overwrite an already-registered canonical plan.
/// `TargetMismatch` is returned when `activate_plan` is called
/// with a `target_branch` that does not match the registered one
/// (in BOTH `Pending` and `Active` states — R10/R17 fail-closed).
/// `InvalidTransition` is returned when `activate_plan` is called
/// on a `Closed` plan (no valid transition out of `Closed`).
// `result_large_err` is allowed at item level per the
// dag_scheduler dead-code policy (`mod.rs:60`): keep variants
// human-readable; do not box for ergonomic reasons. Suppressed
// here to keep clippy green while preserving fail-closed error
// surfaces for dag_store consumers. Per
// 2026-09-09-0917-fix-forge-dag-p1-closure-plan U2 / F17.
#[allow(clippy::result_large_err)]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DagStoreError {
    #[error("plan key already registered with a different artifact digest: {0}")]
    DuplicatePlan(String),
    #[error("plan key not found: {0}")]
    UnknownPlan(String),
    #[error("artifact digest conflict for plan key {plan_key}: expected {expected}, got {actual}")]
    DigestConflict {
        plan_key: String,
        expected: String,
        actual: String,
    },
    #[error(
        "activate_plan target_branch mismatch for plan key {plan_key}: expected {expected}, got {actual}"
    )]
    TargetMismatch {
        plan_key: String,
        expected: String,
        actual: String,
    },
    #[error("invalid plan transition for plan key {plan_key}: expected {expected}, got {actual}")]
    InvalidTransition {
        plan_key: String,
        expected: String,
        actual: String,
    },
    /// 2026-09-09-0917 plan U3 (per-Unit base commit pin): a
    /// `pin_unit_base` call for an already-pinned `(plan_key,
    /// unit_key)` carried a DIFFERENT base commit than the one
    /// already on disk. The first pin is authoritative; any
    /// re-pin with a different base fails closed so the executor's
    /// accepted-output hand-off (reviewer / verifier / fixer)
    /// cannot silently land on top of an unrelated base.
    #[error(
        "dag_unit_bases base_commit drift for {plan_key}/{unit_key}: persisted {persisted} != candidate {candidate}"
    )]
    UnitBaseDrift {
        plan_key: String,
        unit_key: String,
        persisted: String,
        candidate: String,
    },
    /// 2026-09-09-0917 plan U3 (per-stage accepted evidence ledger):
    /// an attempt to record `dag_stage_evidence` for an already-
    /// recorded `(plan_key, unit_key, stage, attempt)` tuple
    /// carried a different `accepted_commit` / `base_commit` /
    /// `evidence_token` / `evidence_fingerprint` than the row
    /// already on disk. Fail closed: the executor's accepted
    /// output for an attempt is immutable once persisted.
    #[error(
        "dag_stage_evidence drift for {plan_key}/{unit_key}/{stage}#{attempt}: {field} persisted {persisted} != candidate {candidate}"
    )]
    StageEvidenceDrift {
        plan_key: String,
        unit_key: String,
        stage: String,
        attempt: u32,
        field: &'static str,
        persisted: String,
        candidate: String,
    },
    #[error("DAG store IO error: {0}")]
    IoError(String),
    /// 2026-09-09-0917 plan F2 / U17 (fail-closed integer decode):
    /// a SQLite column read returned an `i64` that cannot be
    /// losslessly narrowed to its target type (`u64` /
    /// `u32`) — either negative or above the target's `MAX`.
    /// Silent `unwrap_or(0)` / `as u64` would corrupt persisted
    /// timestamps or attempt counters; the rusqlite stores must
    /// surface this so the caller can refuse to resume from a
    /// poisoned row.
    #[error("integer overflow in {field}: expected {expected}, got {actual}")]
    IntegerOverflow {
        field: String,
        expected: String,
        actual: i64,
    },
}

/// Lifecycle status of a registered canonical plan. The
/// transition rule is `Pending → Active → Closed`. Re-registering
/// a `Pending` plan with the same digest is idempotent
/// (regression-R10 / R17); re-activating an already-`Active`
/// plan is a no-op (`activate_plan` returns `Ok(())`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlanStatus {
    Pending,
    Active,
    Closed,
}

impl fmt::Display for PlanStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanStatus::Pending => write!(f, "pending"),
            PlanStatus::Active => write!(f, "active"),
            PlanStatus::Closed => write!(f, "closed"),
        }
    }
}

/// Input record handed to `DagSchedulerStore::register_plan`.
/// Only the bounded identity (plan key, digest, target branch,
/// unit ids, created-at epoch-ms) — the runtime never copies the
/// raw canonical artifact bytes into the store (E9 / E16
/// receipt-content rules).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalPlanRecord {
    pub plan_key: String,
    pub artifact_digest: String,
    pub target_branch: String,
    pub unit_ids: Vec<String>,
    pub created_at_ms: u64,
}

/// Persisted registration row returned by
/// `DagSchedulerStore::register_plan`. The store allocates `id`
/// so a SQL primary key round-trips for the rusqlite variant; the
/// memory variant uses a monotonic counter. `id` is opaque to
/// callers — public callers always identify a plan by
/// `(plan_key, artifact_digest)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRegistration {
    pub id: i64,
    pub plan_key: String,
    pub artifact_digest: String,
    pub target_branch: String,
    pub status: PlanStatus,
    pub unit_ids: Vec<String>,
    pub created_at_ms: u64,
}

/// Result alias for the `DagSchedulerStore` trait surface.
pub type DagStoreResult<T> = Result<T, DagStoreError>;

/// 2026-09-03-0959 plan U3 (R2 / R17 / D17 / E5 / E7 / E9 / E16):
/// durable DAG store contract.
///
/// The contract is intentionally small: a registered plan, an
/// activation transition, and a read API. The runtime MAY call
/// `register_plan` from the `forge.plan.ready` accepted
/// boundary BEFORE `ensure_task_projection` / `ack` — the receipt
/// round-trip lets a crash in the projection window resume from
/// the durable registration rather than re-fanning the work.
///
/// Idempotency contract (R2 / R17):
/// - Same `(plan_key, artifact_digest)` → `Ok(existing)`; no
///   duplicate row, no error. This is the dual-process / restart
///   happy path.
/// - Same `plan_key` with a DIFFERENT `artifact_digest` →
///   `Err(DigestConflict)` (fail-closed; the agent MUST pick a
///   new plan_key or stop).
/// - `activate_plan` is a `Pending → Active` transition;
///   re-activating an already-Active plan is `Ok(())`.
pub trait DagSchedulerStore: Send + Sync {
    /// Register a canonical plan. Idempotent on
    /// `(plan_key, artifact_digest)`; fail-closed on
    /// `plan_key` digest drift.
    fn register_plan(&self, plan: &CanonicalPlanRecord) -> DagStoreResult<PlanRegistration>;

    /// Transition a registered plan from `Pending` to `Active`.
    /// Re-activating an already-Active plan is `Ok(())`; an
    /// unknown plan returns `UnknownPlan`.
    fn activate_plan(&self, plan_key: &str, target_branch: &str) -> DagStoreResult<()>;

    /// Read a registered plan by `plan_key`. Returns `Ok(None)`
    /// when no plan was ever registered under that key.
    fn get_plan(&self, plan_key: &str) -> DagStoreResult<Option<PlanRegistration>>;

    /// List every registered plan whose status is `Active`. Used
    /// by recovery to rebuild the in-memory plan set after a
    /// process restart (R10).
    fn list_active_plans(&self) -> DagStoreResult<Vec<PlanRegistration>>;

    // ─────────────────────────────────────────────────────────────────
    // 2026-09-09-0917 plan U3 (per-Unit base commit pin + per-stage
    // accepted evidence ledger): the executor's accepted-output
    // hand-off needs a durable base commit that survives crashes.
    //
    // `dag_unit_bases` pins the FIRST admission's base (one row
    // per `(plan_key, unit_key)`); subsequent stages build on
    // the prior stage's accepted commit, not on plan HEAD.
    //
    // `dag_stage_evidence` records the accepted commit head, the
    // base it descended from, and an `evidence_token` /
    // `evidence_fingerprint` binding so a resume of the next
    // stage can verify identity before continuing.
    //
    // The default impls are no-op so legacy callers (pre-U3 store
    // mocks) keep compiling. The in-memory and rusqlite production
    // stores MUST override every method — the runtime must never
    // be tricked into resuming from plan HEAD when the durable
    // store has authoritative per-stage evidence.
    // ─────────────────────────────────────────────────────────────────

    /// Pin the per-Unit base commit. Idempotent on
    /// `(plan_key, unit_key, base_commit)`; a re-pin with the SAME
    /// base is a no-op (`Ok(())`), with a DIFFERENT base returns
    /// [`DagStoreError::UnitBaseDrift`] so the executor's
    /// hand-off cannot silently switch bases mid-plan. `pinned_at_ms`
    /// is the caller's injected-clock epoch-ms (no SQL clock
    /// functions so tests stay deterministic).
    fn pin_unit_base(
        &self,
        _plan_key: &str,
        _unit_key: &str,
        _base_commit: &str,
        _pinned_at_ms: u64,
    ) -> DagStoreResult<()> {
        Ok(())
    }

    /// Read the pinned base commit for `(plan_key, unit_key)`,
    /// or `Ok(None)` when the Unit has never been admitted (the
    /// runtime uses plan-level `verified_base_commit` until the
    /// first admission pins the per-Unit base).
    fn get_unit_base(
        &self,
        _plan_key: &str,
        _unit_key: &str,
    ) -> DagStoreResult<Option<UnitBaseRecord>> {
        Ok(None)
    }

    /// Persist the per-stage accepted evidence for
    /// `(plan_key, unit_key, stage, attempt)`. Idempotent on the
    /// exact tuple + fields (replay returns `Ok(())`); a re-record
    /// with a DIFFERENT `accepted_commit` / `base_commit` /
    /// `evidence_token` / `evidence_fingerprint` returns
    /// [`DagStoreError::StageEvidenceDrift`] so a stale replay
    /// cannot rewrite history.
    fn record_stage_evidence(
        &self,
        _plan_key: &str,
        _unit_key: &str,
        _stage: &str,
        _attempt: u32,
        _evidence: &StageEvidenceRecord,
    ) -> DagStoreResult<()> {
        Ok(())
    }

    /// Read the latest persisted stage evidence for
    /// `(plan_key, unit_key, stage)` (highest `attempt`). Returns
    /// `Ok(None)` when the stage has never reached an accepted
    /// terminal for the unit — the runtime treats `None` as
    /// "no prior stage evidence, fall back to plan-level
    /// verified_base_commit".
    fn latest_stage_evidence(
        &self,
        _plan_key: &str,
        _unit_key: &str,
        _stage: &str,
    ) -> DagStoreResult<Option<StageEvidenceRecord>> {
        Ok(None)
    }
}

/// 2026-09-09-0917 plan U3: persisted row for the
/// `dag_unit_bases` table. Records the base commit the Unit's
/// first admission took from the operator worktree; subsequent
/// stages build on the prior stage's accepted commit, never on
/// plan HEAD. The PRIMARY KEY `(plan_key, unit_key)` keeps the
/// pin idempotent at the schema level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitBaseRecord {
    pub plan_key: String,
    pub unit_key: String,
    pub base_commit: String,
    pub pinned_at_ms: u64,
}

/// 2026-09-09-0917 plan U3: persisted row for the
/// `dag_stage_evidence` table. Records the accepted commit head,
/// the base it descended from, and an `evidence_token` /
/// `evidence_fingerprint` binding so a resume of the next stage
/// can verify identity before continuing. The PRIMARY KEY
/// `(plan_key, unit_key, stage, attempt)` keeps the evidence
/// idempotent at the schema level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageEvidenceRecord {
    pub plan_key: String,
    pub unit_key: String,
    pub stage: String,
    pub attempt: u32,
    pub accepted_commit: String,
    pub base_commit: String,
    pub evidence_token: String,
    pub evidence_fingerprint: String,
    pub accepted_at_ms: u64,
}

// The bounded registration receipt type and its in-memory
// registry live in `dag_plan_receipt.rs`; this module only owns
// the store trait + the plan record / registration types that
// the trait hands back to callers.

#[cfg(test)]
pub(crate) mod contract_tests {
    //! 2026-09-09-0917 plan U3 contract helpers.
    //!
    //! The trait defaults on `DagSchedulerStore` are no-op so
    //! legacy callers keep compiling, but EVERY production store
    //! (memory + rusqlite) MUST override every default so the
    //! executor's accepted-output hand-off can resume from the
    //! just-accepted commit rather than plan HEAD.
    //!
    //! Each helper here is a single contract assertion; the
    //! in-memory and rusqlite impls run them from their own
    //! `#[cfg(test)] mod tests` blocks. Keeping the contract
    //! checks centralised means a new store gets the full U3
    //! suite by re-exporting these helpers.

    use super::{DagSchedulerStore, DagStoreError, StageEvidenceRecord, UnitBaseRecord};

    /// Build an evidence record with deterministic defaults; tests
    /// override only the fields they exercise.
    pub(crate) fn evidence(
        plan_key: &str,
        unit_key: &str,
        stage: &str,
        attempt: u32,
        accepted: &str,
        base: &str,
    ) -> StageEvidenceRecord {
        StageEvidenceRecord {
            plan_key: plan_key.to_string(),
            unit_key: unit_key.to_string(),
            stage: stage.to_string(),
            attempt,
            accepted_commit: accepted.to_string(),
            base_commit: base.to_string(),
            evidence_token: format!("tok-{stage}-{attempt}"),
            evidence_fingerprint: format!("fp-{accepted}-{stage}-{attempt}"),
            accepted_at_ms: 1_700_000_000_000 + u64::from(attempt),
        }
    }

    /// Pin once, read back identical record (the round-trip).
    pub(crate) fn assert_pin_roundtrip(store: &dyn DagSchedulerStore) {
        store
            .pin_unit_base("p", "U1", "base-a", 1)
            .expect("first pin");
        let got = store.get_unit_base("p", "U1").expect("read").expect("set");
        assert_eq!(
            got,
            UnitBaseRecord {
                plan_key: "p".into(),
                unit_key: "U1".into(),
                base_commit: "base-a".into(),
                pinned_at_ms: 1,
            }
        );
    }

    /// Pinning twice with the SAME base is idempotent — no error,
    /// no second row, first-pin `pinned_at_ms` wins.
    pub(crate) fn assert_pin_is_idempotent_on_same_base(store: &dyn DagSchedulerStore) {
        store.pin_unit_base("p", "U1", "base-a", 1).expect("first");
        store
            .pin_unit_base("p", "U1", "base-a", 2)
            .expect("idempotent same base");
        let got = store.get_unit_base("p", "U1").expect("read").expect("set");
        assert_eq!(got.pinned_at_ms, 1);
        assert_eq!(got.base_commit, "base-a");
    }

    /// Pinning twice with a DIFFERENT base fails closed (the
    /// first pin is authoritative).
    pub(crate) fn assert_pin_fails_closed_on_base_drift(store: &dyn DagSchedulerStore) {
        store.pin_unit_base("p", "U1", "base-a", 1).expect("first");
        let err = store
            .pin_unit_base("p", "U1", "base-b", 2)
            .expect_err("drift must fail closed");
        assert!(
            matches!(err, DagStoreError::UnitBaseDrift { .. }),
            "expected UnitBaseDrift, got {err:?}"
        );
        // Stored pin unchanged.
        let got = store.get_unit_base("p", "U1").expect("read").expect("set");
        assert_eq!(got.base_commit, "base-a");
    }

    /// `get_unit_base` for an unpinned unit returns `Ok(None)`,
    /// not an error — the runtime treats `None` as "fall back to
    /// plan level verified_base_commit".
    pub(crate) fn assert_get_returns_none_when_unpinned(store: &dyn DagSchedulerStore) {
        let got = store
            .get_unit_base("p", "U_MISSING")
            .expect("missing must not error");
        assert!(got.is_none());
    }

    /// Two units under the same plan are pinned independently —
    /// one unit's base does not collide with another.
    pub(crate) fn assert_units_are_pinned_independently(store: &dyn DagSchedulerStore) {
        store.pin_unit_base("p", "U1", "base-a", 1).expect("U1");
        store.pin_unit_base("p", "U2", "base-b", 2).expect("U2");
        let u1 = store.get_unit_base("p", "U1").expect("u1").expect("u1 set");
        let u2 = store.get_unit_base("p", "U2").expect("u2").expect("u2 set");
        assert_eq!(u1.base_commit, "base-a");
        assert_eq!(u2.base_commit, "base-b");
    }

    /// Recording evidence once round-trips identically.
    pub(crate) fn assert_evidence_roundtrip(store: &dyn DagSchedulerStore) {
        let ev = evidence("p", "U1", "execute", 1, "acc-a", "base-a");
        store
            .record_stage_evidence("p", "U1", "execute", 1, &ev)
            .expect("record");
        let got = store
            .latest_stage_evidence("p", "U1", "execute")
            .expect("read")
            .expect("set");
        assert_eq!(got, ev);
    }

    /// Recording the same evidence twice is idempotent (no error).
    pub(crate) fn assert_evidence_is_idempotent_on_same_record(store: &dyn DagSchedulerStore) {
        let ev = evidence("p", "U1", "execute", 1, "acc-a", "base-a");
        store
            .record_stage_evidence("p", "U1", "execute", 1, &ev)
            .expect("first");
        store
            .record_stage_evidence("p", "U1", "execute", 1, &ev)
            .expect("idempotent");
        let got = store
            .latest_stage_evidence("p", "U1", "execute")
            .expect("read")
            .expect("set");
        assert_eq!(got, ev);
    }

    /// Recording evidence for the SAME (plan, unit, stage) at a
    /// HIGHER attempt returns the highest attempt as latest.
    pub(crate) fn assert_latest_stage_evidence_picks_highest_attempt(
        store: &dyn DagSchedulerStore,
    ) {
        let e1 = evidence("p", "U1", "review", 1, "acc-r1", "base-a");
        let e2 = evidence("p", "U1", "review", 2, "acc-r2", "base-a");
        store
            .record_stage_evidence("p", "U1", "review", 1, &e1)
            .expect("record attempt 1");
        store
            .record_stage_evidence("p", "U1", "review", 2, &e2)
            .expect("record attempt 2");
        let got = store
            .latest_stage_evidence("p", "U1", "review")
            .expect("read")
            .expect("set");
        assert_eq!(got.attempt, 2);
        assert_eq!(got.accepted_commit, "acc-r2");
    }

    /// A re-record at the same attempt with a DIFFERENT field
    /// fails closed — `accepted_commit`, `base_commit`,
    /// `evidence_token`, `evidence_fingerprint` are all
    /// immutable per attempt.
    pub(crate) fn assert_evidence_fails_closed_on_field_drift(store: &dyn DagSchedulerStore) {
        let ev = evidence("p", "U1", "review", 1, "acc-r1", "base-a");
        store
            .record_stage_evidence("p", "U1", "review", 1, &ev)
            .expect("first");
        // Drift accepted_commit.
        let mut drifted = ev.clone();
        drifted.accepted_commit = "acc-NEW".into();
        let err = store
            .record_stage_evidence("p", "U1", "review", 1, &drifted)
            .expect_err("accepted_commit drift must fail closed");
        match err {
            DagStoreError::StageEvidenceDrift { field, .. } => {
                assert_eq!(field, "accepted_commit");
            }
            other => panic!("expected StageEvidenceDrift, got {other:?}"),
        }
        // Drift base_commit.
        let mut drifted = ev.clone();
        drifted.base_commit = "base-NEW".into();
        let err = store
            .record_stage_evidence("p", "U1", "review", 1, &drifted)
            .expect_err("base_commit drift must fail closed");
        match err {
            DagStoreError::StageEvidenceDrift { field, .. } => {
                assert_eq!(field, "base_commit");
            }
            other => panic!("expected StageEvidenceDrift, got {other:?}"),
        }
        // Drift evidence_token.
        let mut drifted = ev.clone();
        drifted.evidence_token = "tok-NEW".into();
        let err = store
            .record_stage_evidence("p", "U1", "review", 1, &drifted)
            .expect_err("evidence_token drift must fail closed");
        match err {
            DagStoreError::StageEvidenceDrift { field, .. } => {
                assert_eq!(field, "evidence_token");
            }
            other => panic!("expected StageEvidenceDrift, got {other:?}"),
        }
        // Drift evidence_fingerprint.
        let mut drifted = ev.clone();
        drifted.evidence_fingerprint = "fp-NEW".into();
        let err = store
            .record_stage_evidence("p", "U1", "review", 1, &drifted)
            .expect_err("evidence_fingerprint drift must fail closed");
        match err {
            DagStoreError::StageEvidenceDrift { field, .. } => {
                assert_eq!(field, "evidence_fingerprint");
            }
            other => panic!("expected StageEvidenceDrift, got {other:?}"),
        }
        // Stored row unchanged — the drift was rejected, not
        // silently accepted.
        let got = store
            .latest_stage_evidence("p", "U1", "review")
            .expect("read")
            .expect("set");
        assert_eq!(got, ev);
    }

    /// Different stages under the same unit are tracked
    /// independently — the `latest_stage_evidence` lookup never
    /// crosses stages.
    pub(crate) fn assert_stages_are_tracked_independently(store: &dyn DagSchedulerStore) {
        let exec = evidence("p", "U1", "execute", 1, "acc-exec", "base-a");
        let rev = evidence("p", "U1", "review", 1, "acc-rev", "base-a");
        store
            .record_stage_evidence("p", "U1", "execute", 1, &exec)
            .expect("exec");
        store
            .record_stage_evidence("p", "U1", "review", 1, &rev)
            .expect("rev");
        assert_eq!(
            store
                .latest_stage_evidence("p", "U1", "execute")
                .expect("exec read")
                .expect("exec set")
                .accepted_commit,
            "acc-exec"
        );
        assert_eq!(
            store
                .latest_stage_evidence("p", "U1", "review")
                .expect("rev read")
                .expect("rev set")
                .accepted_commit,
            "acc-rev"
        );
        // Unknown stage returns Ok(None), not an error.
        assert!(
            store
                .latest_stage_evidence("p", "U1", "verify")
                .expect("verify read")
                .is_none()
        );
    }
}

// The bounded registration receipt type and its in-memory
// registry live in `dag_plan_receipt.rs`; this module only owns
// the store trait + the plan record / registration types that
// the trait hands back to callers.
