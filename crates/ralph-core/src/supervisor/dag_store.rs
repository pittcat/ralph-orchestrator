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

use std::fmt;
use thiserror::Error;

/// Failure modes a `DagSchedulerStore` implementation can
/// surface. Implementations MUST return `DigestConflict` when the
/// same `plan_key` is re-registered with a DIFFERENT
/// `artifact_digest` — fail closed so a stale event cannot
/// silently overwrite an already-registered canonical plan.
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
    #[error("DAG store IO error: {0}")]
    IoError(String),
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
}

// The bounded registration receipt type and its in-memory
// registry live in `dag_plan_receipt.rs`; this module only owns
// the store trait + the plan record / registration types that
// the trait hands back to callers.
