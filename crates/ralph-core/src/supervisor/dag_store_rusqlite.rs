//! PMI-006 / 2026-09-03-0959 plan U3 (R2 / R17 / D4 / E5 / E9):
//! rusqlite implementation of the durable DAG store contract.
//!
//! The module owns THREE trait implementations over a single SQLite
//! connection (the v13 schema in `migrations/v13.sql` plus the v14
//! receipt / unit / lease / job tables in `migrations/v14.sql`):
//! - [`RusqliteDagSchedulerStore`] implements
//!   [`super::dag_store::DagSchedulerStore`] (plan registrations),
//! - [`RusqliteIntegrationStore`] implements
//!   [`super::dag_integration::IntegrationStore`] (integration
//!   records),
//! - [`RusqliteDagPlanReceiptStore`] implements
//!   [`super::dag_plan_receipt::DagPlanReceiptStore`] (durable
//!   registration receipts — PMI-013 / Step 0).
//!
//! Contract parity with the in-memory variants
//! (`dag_store_memory.rs` / `dag_integration.rs`) is enforced by
//! the shared contract suites in this file and by the memory
//! variants' own tests — per plan U3 §14, the same contract runs
//! against both adapters.
//!
//! Concurrency: a single `Mutex<Connection>` serialises every
//! statement within the process, mirroring
//! `RusqliteSupervisorStore`. Cross-process safety does NOT rely
//! on that mutex: every read-decide-write cycle runs inside one
//! SQLite transaction, with `INSERT ... ON CONFLICT DO NOTHING`
//! and status-guarded conditional UPDATEs, so two processes
//! racing on the same `plan_key` cannot interleave a
//! read-decide-write cycle or turn a UNIQUE collision into a
//! spurious IO error.
//!
//! Migration versioning: the store runs the supervisor migration
//! ledger (v1..=14) on `open`, so a `supervisor.db` and a DAG
//! store opened against the same file agree on the schema. The
//! DAG tables are additive to the wave tables — the wave store
//! keeps its single authority; this module only adds the DAG
//! family.

#[cfg(feature = "supervisor-db")]
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::dag_integration::{
    IntegrationInput, IntegrationIntent, IntegrationRecord, IntegrationStore,
    IntegrationStoreError, IntegrationStoreResult, compute_integration_fingerprint,
    validate_intent_replay,
};
use super::dag_plan_receipt::{DagPlanReceipt, DagPlanReceiptStore, parse_receipt_status};
use super::dag_store::{
    CanonicalPlanRecord, DagSchedulerStore, DagStoreError, DagStoreResult, PlanRegistration,
    PlanStatus,
};

#[cfg(feature = "supervisor-db")]
use super::migrations;
#[cfg(feature = "supervisor-db")]
use rusqlite::OptionalExtension;

pub mod jobs;

// ---------------------------------------------------------------------------
// Shared connection wrapper.
// ---------------------------------------------------------------------------

/// `PRAGMA busy_timeout` value for the DAG store connection —
/// same 5-second bound and rationale as the supervisor store's
/// constant (two racing openers of a fresh database must not
/// wedge; a wedged peer must not block a loop indefinitely).
#[cfg(feature = "supervisor-db")]
const DAG_BUSY_TIMEOUT_MS: u32 = 5_000;

/// SQLite-backed connection handle shared by both DAG store
/// implementations. `open` runs migrations so every construction
/// path sees the v13 schema.
struct DagConnection {
    conn: Mutex<rusqlite::Connection>,
}

#[cfg(feature = "supervisor-db")]
impl DagConnection {
    /// Open (creating if needed) the database at `path` and run
    /// the migration ledger. Mirrors `RusqliteSupervisorStore::
    /// open`'s busy-retry so two racing openers of a fresh
    /// database do not collide on the WAL header switch.
    fn open(path: impl AsRef<Path>) -> Result<Self, DagStoreError> {
        let path = path.as_ref();
        let conn = rusqlite::Connection::open(path)
            .map_err(|err| DagStoreError::IoError(format!("{}: {err}", path.display())))?;
        conn.pragma_update(None, "busy_timeout", DAG_BUSY_TIMEOUT_MS)
            .map_err(|err| {
                DagStoreError::IoError(format!(
                    "failed to set busy_timeout on {}: {err}",
                    path.display()
                ))
            })?;
        // Same SQLITE_BUSY retry contract as the supervisor store:
        // a fresh database's WAL sidecar creation can race a
        // second opener at the filesystem level (below the busy
        // handler). 5 attempts × linear backoff.
        const MIGRATION_RETRIES: u32 = 5;
        let mut last_busy: Option<rusqlite::Error> = None;
        for attempt in 0..MIGRATION_RETRIES {
            match migrations::run(&conn) {
                Ok(()) => {
                    last_busy = None;
                    break;
                }
                Err(err) if is_sqlite_busy(&err) => {
                    last_busy = Some(err);
                    std::thread::sleep(std::time::Duration::from_millis(50 * (attempt as u64 + 1)));
                }
                Err(err) => {
                    return Err(DagStoreError::IoError(format!(
                        "migration failed on {}: {err}",
                        path.display()
                    )));
                }
            }
        }
        if let Some(err) = last_busy {
            return Err(DagStoreError::IoError(format!(
                "migration failed on {} after {MIGRATION_RETRIES} retries: {err}",
                path.display()
            )));
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

/// `true` when the rusqlite error carries `SQLITE_BUSY`. Shared
/// with the supervisor store's retry semantics.
#[cfg(feature = "supervisor-db")]
fn is_sqlite_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::DatabaseBusy,
                ..
            },
            _,
        )
    )
}

/// Bridge a `rusqlite::Error` into the plan-store error enum.
#[cfg(feature = "supervisor-db")]
fn plan_io_err(err: rusqlite::Error) -> DagStoreError {
    DagStoreError::IoError(err.to_string())
}

/// Bridge a `rusqlite::Error` into the integration-store error
/// enum.
#[cfg(feature = "supervisor-db")]
fn integration_io_err(err: rusqlite::Error) -> IntegrationStoreError {
    IntegrationStoreError::StorageIo(err.to_string())
}

/// Serialise the bounded unit-id list as a JSON array. The list is
/// plan identity (never raw artifact bytes, E9/E16); empty lists
/// round-trip as `[]`.
#[cfg(feature = "supervisor-db")]
fn unit_ids_to_json(unit_ids: &[String]) -> String {
    serde_json::to_string(unit_ids).unwrap_or_else(|_| "[]".to_string())
}

/// Parse the `unit_ids` JSON column back into a `Vec<String>`. A
/// corrupt / non-array column is a storage error (fail closed —
/// never silently downgrade to an empty list, which would make a
/// registered plan look unit-less).
#[cfg(feature = "supervisor-db")]
fn unit_ids_from_json(raw: &str) -> DagStoreResult<Vec<String>> {
    serde_json::from_str(raw)
        .map_err(|err| DagStoreError::IoError(format!("corrupt dag_plans.unit_ids column: {err}")))
}

#[cfg(feature = "supervisor-db")]
fn parse_plan_status(s: &str) -> DagStoreResult<PlanStatus> {
    match s {
        "pending" => Ok(PlanStatus::Pending),
        "active" => Ok(PlanStatus::Active),
        "closed" => Ok(PlanStatus::Closed),
        other => Err(DagStoreError::IoError(format!(
            "corrupt dag_plans.status column: unknown status {other:?}"
        ))),
    }
}

#[cfg(feature = "supervisor-db")]
#[allow(dead_code)] // pinned by the reopen contract tests (status round-trip)
fn plan_status_to_str(status: PlanStatus) -> &'static str {
    match status {
        PlanStatus::Pending => "pending",
        PlanStatus::Active => "active",
        PlanStatus::Closed => "closed",
    }
}

/// Read one `dag_plans` row into a [`PlanRegistration`]. A
/// corrupt `status` or `unit_ids` column surfaces as a storage
/// IO error via the FromSqlConversionFailure path — fail closed
/// rather than guessing a status.
#[cfg(feature = "supervisor-db")]
fn row_to_plan_registration(row: &rusqlite::Row<'_>) -> Result<PlanRegistration, rusqlite::Error> {
    let unit_ids_raw: String = row.get("unit_ids")?;
    let status_raw: String = row.get("status")?;
    let status = parse_plan_status(&status_raw).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(err))
    })?;
    let unit_ids = unit_ids_from_json(&unit_ids_raw).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok(PlanRegistration {
        id: row.get("id")?,
        plan_key: row.get("plan_key")?,
        artifact_digest: row.get("artifact_digest")?,
        target_branch: row.get("target_branch")?,
        status,
        unit_ids,
        created_at_ms: row.get::<_, i64>("created_at_ms")? as u64,
    })
}

// ---------------------------------------------------------------------------
// Durable DAG plan store.
// ---------------------------------------------------------------------------

/// SQLite-backed [`DagSchedulerStore`]. One row per registered
/// canonical plan in `dag_plans`. Idempotent on
/// `(plan_key, artifact_digest)`; digest drift fails closed
/// (R2 / R17).
#[derive(Clone)]
pub struct RusqliteDagSchedulerStore {
    inner: Arc<DagConnection>,
}

#[cfg(feature = "supervisor-db")]
impl RusqliteDagSchedulerStore {
    /// Open (creating if needed) the durable DAG plan store at
    /// `path`. Migrations run on every open; already-current
    /// databases are a no-op.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DagStoreError> {
        Ok(Self {
            inner: Arc::new(DagConnection::open(path)?),
        })
    }

    /// Share the same connection as the integration store so the
    /// two DAG state families live in one database file.
    pub fn shared_with_integration(&self) -> RusqliteIntegrationStore {
        RusqliteIntegrationStore {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Share the same connection as the receipt store so the
    /// registration receipts live in the same database file as
    /// the plan registrations they precede (R17 / D18).
    pub fn shared_with_receipts(&self) -> RusqliteDagPlanReceiptStore {
        RusqliteDagPlanReceiptStore {
            inner: Arc::clone(&self.inner),
        }
    }

    /// S15 exactly-once fence for runtime-emitted terminal
    /// coordination events (dag mode). `INSERT OR IGNORE` against
    /// the `(plan_key, topic)` PRIMARY KEY: the first caller wins
    /// the emit permit (`Ok(true)`); every replay loses
    /// (`Ok(false)`). A conflicting key payload for an already-
    /// fenced pair fails closed with `IoError` — a replay must
    /// carry the identical idempotency key.
    pub fn try_record_terminal_emit(
        &self,
        plan_key: &str,
        topic: &str,
        idempotency_key: &str,
        created_at_ms: i64,
    ) -> Result<bool, DagStoreError> {
        let conn = self
            .inner
            .conn
            .lock()
            .map_err(|_| DagStoreError::IoError("dag store mutex poisoned".into()))?;
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO dag_terminal_emits \
                 (plan_key, topic, idempotency_key, created_at_ms) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![plan_key, topic, idempotency_key, created_at_ms],
            )
            .map_err(|err| DagStoreError::IoError(format!("terminal-emit fence insert: {err}")))?;
        if inserted == 1 {
            return Ok(true);
        }
        let existing: String = conn
            .query_row(
                "SELECT idempotency_key FROM dag_terminal_emits \
                 WHERE plan_key = ?1 AND topic = ?2",
                rusqlite::params![plan_key, topic],
                |row| row.get(0),
            )
            .map_err(|err| DagStoreError::IoError(format!("terminal-emit fence read: {err}")))?;
        if existing != idempotency_key {
            return Err(DagStoreError::IoError(format!(
                "terminal-emit fence conflict for ({plan_key}, {topic}): \
                 persisted key {existing} != candidate {idempotency_key}"
            )));
        }
        Ok(false)
    }
}

impl std::fmt::Debug for RusqliteDagSchedulerStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RusqliteDagSchedulerStore").finish()
    }
}

impl DagSchedulerStore for RusqliteDagSchedulerStore {
    fn register_plan(&self, plan: &CanonicalPlanRecord) -> DagStoreResult<PlanRegistration> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = plan;
            Err(DagStoreError::IoError(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let mut conn = self
                .inner
                .conn
                .lock()
                .map_err(|_| DagStoreError::IoError("dag plan store mutex poisoned".into()))?;
            // Single transaction spanning the write and the
            // deciding read: the in-process mutex alone cannot
            // stop a SECOND PROCESS from interleaving its own
            // INSERT between our SELECT and INSERT (cross-process
            // TOCTOU). `INSERT ... ON CONFLICT(plan_key) DO
            // NOTHING` makes the insert atomic against the UNIQUE
            // constraint — a losing racer does nothing instead of
            // surfacing a raw constraint violation — and the
            // follow-up SELECT (same transaction snapshot)
            // distinguishes the two existing-row cases:
            //   same digest  → idempotent Ok(existing)
            //   other digest → DigestConflict (fail closed)
            let tx = conn.transaction().map_err(plan_io_err)?;
            tx.execute(
                "INSERT INTO dag_plans \
                 (plan_key, artifact_digest, target_branch, unit_ids, status, created_at_ms) \
                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5) \
                 ON CONFLICT(plan_key) DO NOTHING",
                rusqlite::params![
                    plan.plan_key,
                    plan.artifact_digest,
                    plan.target_branch,
                    unit_ids_to_json(&plan.unit_ids),
                    plan.created_at_ms as i64,
                ],
            )
            .map_err(plan_io_err)?;
            let row: PlanRegistration = tx
                .query_row(
                    "SELECT id, plan_key, artifact_digest, target_branch, unit_ids, status, \
                     created_at_ms FROM dag_plans WHERE plan_key = ?1",
                    [&plan.plan_key],
                    row_to_plan_registration,
                )
                .map_err(plan_io_err)?;
            if row.artifact_digest != plan.artifact_digest {
                // Digest drift: roll back (tx drop) and fail
                // closed. When we lost the insert race this also
                // leaves the winner's row untouched.
                return Err(DagStoreError::DigestConflict {
                    plan_key: plan.plan_key.clone(),
                    expected: row.artifact_digest,
                    actual: plan.artifact_digest.clone(),
                });
            }
            tx.commit().map_err(plan_io_err)?;
            Ok(row)
        }
    }

    fn activate_plan(&self, plan_key: &str, target_branch: &str) -> DagStoreResult<()> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = (plan_key, target_branch);
            Err(DagStoreError::IoError(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let mut conn = self
                .inner
                .conn
                .lock()
                .map_err(|_| DagStoreError::IoError("dag plan store mutex poisoned".into()))?;
            // Conditional UPDATE first, inside one transaction:
            // the status guard in the WHERE clause makes the
            // Pending → Active transition atomic across processes
            // (a second-process racer cannot slip between a
            // check-SELECT and a write). `affected == 1` means we
            // won the transition; `affected == 0` needs one
            // disambiguating read in the same snapshot to tell
            // apart unknown key / already-active (idempotent Ok)
            // / closed (InvalidTransition) / target mismatch
            // (fail closed, R10/R17).
            let tx = conn.transaction().map_err(plan_io_err)?;
            let affected = tx
                .execute(
                    "UPDATE dag_plans SET status = 'active' \
                     WHERE plan_key = ?1 AND status = 'pending' AND target_branch = ?2",
                    rusqlite::params![plan_key, target_branch],
                )
                .map_err(plan_io_err)?;
            if affected == 0 {
                let row: Option<(String, String)> = tx
                    .query_row(
                        "SELECT target_branch, status FROM dag_plans WHERE plan_key = ?1",
                        [plan_key],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()
                    .map_err(plan_io_err)?;
                let Some((registered_branch, status)) = row else {
                    return Err(DagStoreError::UnknownPlan(plan_key.to_string()));
                };
                let status = parse_plan_status(&status)
                    .map_err(|err| DagStoreError::IoError(err.to_string()))?;
                if status == PlanStatus::Closed {
                    return Err(DagStoreError::InvalidTransition {
                        plan_key: plan_key.to_string(),
                        expected: "active_or_pending".to_string(),
                        actual: "plan is closed".to_string(),
                    });
                }
                if registered_branch != target_branch {
                    return Err(DagStoreError::TargetMismatch {
                        plan_key: plan_key.to_string(),
                        expected: registered_branch,
                        actual: target_branch.to_string(),
                    });
                }
                debug_assert_eq!(status, PlanStatus::Active);
                // Already Active with the same target: idempotent
                // no-op. Nothing was written; commit is a no-op
                // read-transaction close.
            }
            tx.commit().map_err(plan_io_err)?;
            Ok(())
        }
    }

    fn get_plan(&self, plan_key: &str) -> DagStoreResult<Option<PlanRegistration>> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = plan_key;
            Err(DagStoreError::IoError(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let conn = self
                .inner
                .conn
                .lock()
                .map_err(|_| DagStoreError::IoError("dag plan store mutex poisoned".into()))?;
            conn.query_row(
                "SELECT id, plan_key, artifact_digest, target_branch, unit_ids, status, \
                 created_at_ms FROM dag_plans WHERE plan_key = ?1",
                [plan_key],
                row_to_plan_registration,
            )
            .optional()
            .map_err(plan_io_err)
        }
    }

    fn list_active_plans(&self) -> DagStoreResult<Vec<PlanRegistration>> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            Err(DagStoreError::IoError(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let conn = self
                .inner
                .conn
                .lock()
                .map_err(|_| DagStoreError::IoError("dag plan store mutex poisoned".into()))?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, plan_key, artifact_digest, target_branch, unit_ids, status, \
                     created_at_ms FROM dag_plans WHERE status = 'active' ORDER BY id",
                )
                .map_err(plan_io_err)?;
            let rows = stmt
                .query_map([], row_to_plan_registration)
                .map_err(plan_io_err)?;
            let mut plans = Vec::new();
            for row in rows {
                plans.push(row.map_err(plan_io_err)?);
            }
            Ok(plans)
        }
    }
}

// ---------------------------------------------------------------------------
// Durable registration receipt store.
// ---------------------------------------------------------------------------

/// Read one `dag_plan_receipts` row into a [`DagPlanReceipt`]. A
/// corrupt `status` column surfaces as a storage error via the
/// FromSqlConversionFailure path — fail closed rather than
/// guessing a status.
#[cfg(feature = "supervisor-db")]
fn row_to_receipt(row: &rusqlite::Row<'_>) -> Result<DagPlanReceipt, rusqlite::Error> {
    let status_raw: String = row.get("status")?;
    let status = parse_receipt_status(&status_raw).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok(DagPlanReceipt {
        plan_key: row.get("plan_key")?,
        artifact_path: row.get("artifact_path")?,
        artifact_digest: row.get("artifact_digest")?,
        target_branch: row.get("target_branch")?,
        status,
        created_at_ms: row.get::<_, i64>("created_at_ms")? as u64,
        activated_at_ms: row
            .get::<_, Option<i64>>("activated_at_ms")?
            .map(|v| v as u64),
        consumed_at_ms: row
            .get::<_, Option<i64>>("consumed_at_ms")?
            .map(|v| v as u64),
    })
}

/// SQLite-backed [`DagPlanReceiptStore`]. One row per registration
/// receipt in `dag_plan_receipts` (migration v14), keyed by
/// `plan_key`. Idempotent on `(plan_key, artifact_digest)`; digest
/// drift fails closed (R17 / D18). PMI-013 / Step 0: this is the
/// durable authority behind
/// [`super::dag_plan_receipt::DagPlanReceiptRegistry`].
#[derive(Clone)]
pub struct RusqliteDagPlanReceiptStore {
    inner: Arc<DagConnection>,
}

#[cfg(feature = "supervisor-db")]
impl RusqliteDagPlanReceiptStore {
    /// Open (creating if needed) the durable receipt store at
    /// `path`. Migrations run on every open; already-current
    /// databases are a no-op.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DagStoreError> {
        Ok(Self {
            inner: Arc::new(DagConnection::open(path)?),
        })
    }

    /// Share the same connection as the plan store.
    pub fn shared_with_plans(&self) -> RusqliteDagSchedulerStore {
        RusqliteDagSchedulerStore {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Share the same connection as the integration store.
    pub fn shared_with_integration(&self) -> RusqliteIntegrationStore {
        RusqliteIntegrationStore {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl std::fmt::Debug for RusqliteDagPlanReceiptStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RusqliteDagPlanReceiptStore").finish()
    }
}

impl DagPlanReceiptStore for RusqliteDagPlanReceiptStore {
    fn record_receipt(&self, receipt: &DagPlanReceipt) -> DagStoreResult<bool> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = receipt;
            Err(DagStoreError::IoError(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let mut conn =
                self.inner.conn.lock().map_err(|_| {
                    DagStoreError::IoError("dag receipt store mutex poisoned".into())
                })?;
            // Same single-transaction + ON CONFLICT shape as
            // `register_plan`: the insert is atomic against the
            // PRIMARY KEY, and the follow-up SELECT (same snapshot)
            // distinguishes idempotent replay from digest drift
            // (fail closed).
            let tx = conn.transaction().map_err(plan_io_err)?;
            let inserted = tx
                .execute(
                    "INSERT INTO dag_plan_receipts \
                     (plan_key, artifact_path, artifact_digest, target_branch, status, \
                      created_at_ms, activated_at_ms, consumed_at_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                     ON CONFLICT(plan_key) DO NOTHING",
                    rusqlite::params![
                        receipt.plan_key,
                        receipt.artifact_path,
                        receipt.artifact_digest,
                        receipt.target_branch,
                        receipt.status.as_str(),
                        receipt.created_at_ms as i64,
                        receipt.activated_at_ms.map(|v| v as i64),
                        receipt.consumed_at_ms.map(|v| v as i64),
                    ],
                )
                .map_err(plan_io_err)?;
            let row: DagPlanReceipt = tx
                .query_row(
                    "SELECT plan_key, artifact_path, artifact_digest, target_branch, status, \
                     created_at_ms, activated_at_ms, consumed_at_ms \
                     FROM dag_plan_receipts WHERE plan_key = ?1",
                    [&receipt.plan_key],
                    row_to_receipt,
                )
                .map_err(plan_io_err)?;
            if row.artifact_digest != receipt.artifact_digest {
                // Digest drift: roll back (tx drop) and fail
                // closed, leaving the winner's row untouched.
                return Err(DagStoreError::DigestConflict {
                    plan_key: receipt.plan_key.clone(),
                    expected: row.artifact_digest,
                    actual: receipt.artifact_digest.clone(),
                });
            }
            tx.commit().map_err(plan_io_err)?;
            Ok(inserted > 0)
        }
    }

    fn get_receipt(&self, plan_key: &str) -> DagStoreResult<Option<DagPlanReceipt>> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = plan_key;
            Err(DagStoreError::IoError(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let conn =
                self.inner.conn.lock().map_err(|_| {
                    DagStoreError::IoError("dag receipt store mutex poisoned".into())
                })?;
            conn.query_row(
                "SELECT plan_key, artifact_path, artifact_digest, target_branch, status, \
                 created_at_ms, activated_at_ms, consumed_at_ms \
                 FROM dag_plan_receipts WHERE plan_key = ?1",
                [plan_key],
                row_to_receipt,
            )
            .optional()
            .map_err(plan_io_err)
        }
    }

    fn list_receipts(&self) -> DagStoreResult<Vec<DagPlanReceipt>> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            Err(DagStoreError::IoError(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let conn =
                self.inner.conn.lock().map_err(|_| {
                    DagStoreError::IoError("dag receipt store mutex poisoned".into())
                })?;
            let mut stmt = conn
                .prepare(
                    "SELECT plan_key, artifact_path, artifact_digest, target_branch, status, \
                     created_at_ms, activated_at_ms, consumed_at_ms \
                     FROM dag_plan_receipts ORDER BY created_at_ms, plan_key",
                )
                .map_err(plan_io_err)?;
            let rows = stmt.query_map([], row_to_receipt).map_err(plan_io_err)?;
            let mut receipts = Vec::new();
            for row in rows {
                receipts.push(row.map_err(plan_io_err)?);
            }
            Ok(receipts)
        }
    }

    fn activate_receipt(
        &self,
        plan_key: &str,
        activated_at_ms: u64,
    ) -> DagStoreResult<DagPlanReceipt> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = (plan_key, activated_at_ms);
            Err(DagStoreError::IoError(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let mut conn =
                self.inner.conn.lock().map_err(|_| {
                    DagStoreError::IoError("dag receipt store mutex poisoned".into())
                })?;
            // Status-guarded conditional UPDATE first (same shape as
            // `activate_plan`): `affected == 1` means we won the
            // Pending → Active transition atomically; `affected == 0`
            // needs one disambiguating read in the same snapshot to
            // tell apart unknown key / already-active (idempotent
            // no-op) / consumed (InvalidTransition, fail closed).
            let tx = conn.transaction().map_err(plan_io_err)?;
            let affected = tx
                .execute(
                    "UPDATE dag_plan_receipts SET status = 'active', activated_at_ms = ?2 \
                     WHERE plan_key = ?1 AND status = 'pending'",
                    rusqlite::params![plan_key, activated_at_ms as i64],
                )
                .map_err(plan_io_err)?;
            if affected == 0 {
                let row: Option<DagPlanReceipt> = tx
                    .query_row(
                        "SELECT plan_key, artifact_path, artifact_digest, target_branch, status, \
                         created_at_ms, activated_at_ms, consumed_at_ms \
                         FROM dag_plan_receipts WHERE plan_key = ?1",
                        [plan_key],
                        row_to_receipt,
                    )
                    .optional()
                    .map_err(plan_io_err)?;
                let Some(row) = row else {
                    return Err(DagStoreError::UnknownPlan(plan_key.to_string()));
                };
                match row.status {
                    super::dag_plan_receipt::ReceiptStatus::Consumed => {
                        return Err(DagStoreError::InvalidTransition {
                            plan_key: plan_key.to_string(),
                            expected: "pending_or_active".to_string(),
                            actual: "receipt is consumed".to_string(),
                        });
                    }
                    super::dag_plan_receipt::ReceiptStatus::Active => {
                        // Already Active: idempotent no-op; the
                        // original activation stamp is untouched.
                        tx.commit().map_err(plan_io_err)?;
                        return Ok(row);
                    }
                    super::dag_plan_receipt::ReceiptStatus::Pending => {
                        // The guarded UPDATE would have matched a
                        // Pending row; reaching this arm is a store
                        // bug.
                        debug_assert!(false, "pending receipt must have been updated");
                    }
                }
            }
            // Re-read so the caller observes the post-transition row
            // exactly as a fresh reopen would.
            let row: DagPlanReceipt = tx
                .query_row(
                    "SELECT plan_key, artifact_path, artifact_digest, target_branch, status, \
                     created_at_ms, activated_at_ms, consumed_at_ms \
                     FROM dag_plan_receipts WHERE plan_key = ?1",
                    [plan_key],
                    row_to_receipt,
                )
                .map_err(plan_io_err)?;
            tx.commit().map_err(plan_io_err)?;
            Ok(row)
        }
    }

    fn consume_receipt(
        &self,
        plan_key: &str,
        consumed_at_ms: u64,
    ) -> DagStoreResult<DagPlanReceipt> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = (plan_key, consumed_at_ms);
            Err(DagStoreError::IoError(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let mut conn =
                self.inner.conn.lock().map_err(|_| {
                    DagStoreError::IoError("dag receipt store mutex poisoned".into())
                })?;
            let tx = conn.transaction().map_err(plan_io_err)?;
            let affected = tx
                .execute(
                    "UPDATE dag_plan_receipts SET status = 'consumed', consumed_at_ms = ?2 \
                     WHERE plan_key = ?1 AND status IN ('pending', 'active')",
                    rusqlite::params![plan_key, consumed_at_ms as i64],
                )
                .map_err(plan_io_err)?;
            if affected == 0 {
                let row: Option<DagPlanReceipt> = tx
                    .query_row(
                        "SELECT plan_key, artifact_path, artifact_digest, target_branch, status, \
                         created_at_ms, activated_at_ms, consumed_at_ms \
                         FROM dag_plan_receipts WHERE plan_key = ?1",
                        [plan_key],
                        row_to_receipt,
                    )
                    .optional()
                    .map_err(plan_io_err)?;
                let Some(row) = row else {
                    return Err(DagStoreError::UnknownPlan(plan_key.to_string()));
                };
                // Already Consumed: idempotent no-op; the original
                // consume stamp is untouched.
                tx.commit().map_err(plan_io_err)?;
                return Ok(row);
            }
            let row: DagPlanReceipt = tx
                .query_row(
                    "SELECT plan_key, artifact_path, artifact_digest, target_branch, status, \
                     created_at_ms, activated_at_ms, consumed_at_ms \
                     FROM dag_plan_receipts WHERE plan_key = ?1",
                    [plan_key],
                    row_to_receipt,
                )
                .map_err(plan_io_err)?;
            tx.commit().map_err(plan_io_err)?;
            Ok(row)
        }
    }
}

// ---------------------------------------------------------------------------
// Durable integration store.
// ---------------------------------------------------------------------------

/// SQLite-backed [`IntegrationStore`]. One row per
/// `(unit_id, target_branch)` in `dag_integrations`. Idempotent
/// on the natural-key tuple; `DuplicateUnitForTarget` /
/// `FingerprintDrift` fail closed (U7 contract).
#[derive(Clone)]
pub struct RusqliteIntegrationStore {
    inner: Arc<DagConnection>,
}

#[cfg(feature = "supervisor-db")]
impl RusqliteIntegrationStore {
    /// Open (creating if needed) the durable integration store at
    /// `path`. Migrations run on every open.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, IntegrationStoreError> {
        Ok(Self {
            inner: Arc::new(
                DagConnection::open(path)
                    .map_err(|err| IntegrationStoreError::StorageIo(err.to_string()))?,
            ),
        })
    }

    /// Share the same connection as the plan store.
    pub fn shared_with_plans(&self) -> RusqliteDagSchedulerStore {
        RusqliteDagSchedulerStore {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl std::fmt::Debug for RusqliteIntegrationStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RusqliteIntegrationStore").finish()
    }
}

impl IntegrationStore for RusqliteIntegrationStore {
    fn prepare_intent(
        &self,
        intent: &IntegrationIntent,
    ) -> IntegrationStoreResult<IntegrationIntent> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = intent;
            Err(IntegrationStoreError::StorageIo(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let conn = self.inner.conn.lock().map_err(|_| {
                IntegrationStoreError::StorageIo("dag integration store mutex poisoned".into())
            })?;
            let existing: Option<IntegrationIntent> = conn
                .query_row(
                    "SELECT unit_id, target_branch, base_commit, unit_commit, \
                     expected_head_before, integrated_commit, tree_oid, created_at_ms \
                     FROM dag_integration_intents WHERE unit_id = ?1 AND target_branch = ?2",
                    [&intent.input.unit_id, &intent.input.target_branch],
                    row_to_integration_intent,
                )
                .optional()
                .map_err(integration_io_err)?;
            if let Some(existing) = existing {
                validate_intent_replay(&existing, intent)?;
                return Ok(existing);
            }
            conn.execute(
                "INSERT INTO dag_integration_intents \
                 (unit_id, target_branch, base_commit, unit_commit, \
                  expected_head_before, integrated_commit, tree_oid, created_at_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    intent.input.unit_id,
                    intent.input.target_branch,
                    intent.input.base_commit,
                    intent.unit_commit,
                    intent.input.expected_head_before,
                    intent.input.integrated_commit,
                    intent.tree_oid,
                    intent.input.created_at_ms,
                ],
            )
            .map_err(integration_io_err)?;
            Ok(intent.clone())
        }
    }

    fn get_intent(
        &self,
        unit_id: &str,
        target_branch: &str,
    ) -> IntegrationStoreResult<Option<IntegrationIntent>> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = (unit_id, target_branch);
            Err(IntegrationStoreError::StorageIo(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let conn = self.inner.conn.lock().map_err(|_| {
                IntegrationStoreError::StorageIo("dag integration store mutex poisoned".into())
            })?;
            conn.query_row(
                "SELECT unit_id, target_branch, base_commit, unit_commit, \
                 expected_head_before, integrated_commit, tree_oid, created_at_ms \
                 FROM dag_integration_intents WHERE unit_id = ?1 AND target_branch = ?2",
                [unit_id, target_branch],
                row_to_integration_intent,
            )
            .optional()
            .map_err(integration_io_err)
        }
    }

    fn record_integrated(
        &self,
        input: &IntegrationInput,
    ) -> IntegrationStoreResult<IntegrationRecord> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = input;
            Err(IntegrationStoreError::StorageIo(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let conn = self.inner.conn.lock().map_err(|_| {
                IntegrationStoreError::StorageIo("dag integration store mutex poisoned".into())
            })?;
            let fingerprint = compute_integration_fingerprint(input);
            let existing: Option<IntegrationRecord> = conn
                .query_row(
                    "SELECT id, unit_id, target_branch, base_commit, integrated_commit, \
                     expected_head_before, commit_fingerprint, acked, created_at_ms \
                     FROM dag_integrations WHERE unit_id = ?1 AND target_branch = ?2",
                    [&input.unit_id, &input.target_branch],
                    row_to_integration_record,
                )
                .optional()
                .map_err(integration_io_err)?;
            if let Some(existing) = existing {
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
                        expected: existing.commit_fingerprint,
                        actual: fingerprint,
                    });
                }
                return Ok(existing);
            }
            conn.execute(
                "INSERT INTO dag_integrations \
                 (unit_id, target_branch, base_commit, integrated_commit, \
                  expected_head_before, commit_fingerprint, acked, created_at_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
                rusqlite::params![
                    input.unit_id,
                    input.target_branch,
                    input.base_commit,
                    input.integrated_commit,
                    input.expected_head_before,
                    fingerprint,
                    input.created_at_ms,
                ],
            )
            .map_err(integration_io_err)?;
            conn.query_row(
                "SELECT id, unit_id, target_branch, base_commit, integrated_commit, \
                 expected_head_before, commit_fingerprint, acked, created_at_ms \
                 FROM dag_integrations WHERE unit_id = ?1 AND target_branch = ?2",
                [&input.unit_id, &input.target_branch],
                row_to_integration_record,
            )
            .map_err(integration_io_err)
        }
    }

    fn ack(&self, unit_id: &str, target_branch: &str) -> IntegrationStoreResult<IntegrationRecord> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = (unit_id, target_branch);
            Err(IntegrationStoreError::StorageIo(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let conn = self.inner.conn.lock().map_err(|_| {
                IntegrationStoreError::StorageIo("dag integration store mutex poisoned".into())
            })?;
            let existing: Option<IntegrationRecord> = conn
                .query_row(
                    "SELECT id, unit_id, target_branch, base_commit, integrated_commit, \
                     expected_head_before, commit_fingerprint, acked, created_at_ms \
                     FROM dag_integrations WHERE unit_id = ?1 AND target_branch = ?2",
                    [unit_id, target_branch],
                    row_to_integration_record,
                )
                .optional()
                .map_err(integration_io_err)?;
            let Some(record) = existing else {
                return Err(IntegrationStoreError::NotYetRecorded {
                    unit_id: unit_id.to_string(),
                    target_branch: target_branch.to_string(),
                });
            };
            if !record.acked {
                conn.execute(
                    "UPDATE dag_integrations SET acked = 1 WHERE unit_id = ?1 AND \
                     target_branch = ?2",
                    [unit_id, target_branch],
                )
                .map_err(integration_io_err)?;
            }
            // Re-read so the caller observes the post-update row
            // exactly as a fresh reopen would.
            conn.query_row(
                "SELECT id, unit_id, target_branch, base_commit, integrated_commit, \
                 expected_head_before, commit_fingerprint, acked, created_at_ms \
                 FROM dag_integrations WHERE unit_id = ?1 AND target_branch = ?2",
                [unit_id, target_branch],
                row_to_integration_record,
            )
            .map_err(integration_io_err)
        }
    }

    fn list_for_unit(&self, unit_id: &str) -> IntegrationStoreResult<Vec<IntegrationRecord>> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = unit_id;
            Err(IntegrationStoreError::StorageIo(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let conn = self.inner.conn.lock().map_err(|_| {
                IntegrationStoreError::StorageIo("dag integration store mutex poisoned".into())
            })?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, unit_id, target_branch, base_commit, integrated_commit, \
                     expected_head_before, commit_fingerprint, acked, created_at_ms \
                     FROM dag_integrations WHERE unit_id = ?1 ORDER BY id",
                )
                .map_err(integration_io_err)?;
            let rows = stmt
                .query_map([unit_id], row_to_integration_record)
                .map_err(integration_io_err)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(integration_io_err)?);
            }
            Ok(records)
        }
    }

    fn list_unacked_for_target(
        &self,
        target_branch: &str,
    ) -> IntegrationStoreResult<Vec<IntegrationRecord>> {
        #[cfg(not(feature = "supervisor-db"))]
        {
            let _ = target_branch;
            Err(IntegrationStoreError::StorageIo(
                "supervisor-db cargo feature is off in this build".to_string(),
            ))
        }
        #[cfg(feature = "supervisor-db")]
        {
            let conn = self.inner.conn.lock().map_err(|_| {
                IntegrationStoreError::StorageIo("dag integration store mutex poisoned".into())
            })?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, unit_id, target_branch, base_commit, integrated_commit, \
                     expected_head_before, commit_fingerprint, acked, created_at_ms \
                     FROM dag_integrations WHERE target_branch = ?1 AND acked = 0 \
                     ORDER BY id",
                )
                .map_err(integration_io_err)?;
            let rows = stmt
                .query_map([target_branch], row_to_integration_record)
                .map_err(integration_io_err)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(integration_io_err)?);
            }
            Ok(records)
        }
    }
}

/// Read one `dag_integrations` row into an [`IntegrationRecord`].
#[cfg(feature = "supervisor-db")]
fn row_to_integration_record(
    row: &rusqlite::Row<'_>,
) -> Result<IntegrationRecord, rusqlite::Error> {
    Ok(IntegrationRecord {
        id: row.get("id")?,
        unit_id: row.get("unit_id")?,
        target_branch: row.get("target_branch")?,
        base_commit: row.get("base_commit")?,
        integrated_commit: row.get("integrated_commit")?,
        expected_head_before: row.get("expected_head_before")?,
        commit_fingerprint: row.get("commit_fingerprint")?,
        acked: row.get::<_, i64>("acked")? != 0,
        created_at_ms: row.get("created_at_ms")?,
    })
}

/// Read one `dag_integration_intents` row into an [`IntegrationIntent`].
#[cfg(feature = "supervisor-db")]
fn row_to_integration_intent(
    row: &rusqlite::Row<'_>,
) -> Result<IntegrationIntent, rusqlite::Error> {
    Ok(IntegrationIntent {
        input: IntegrationInput {
            unit_id: row.get("unit_id")?,
            target_branch: row.get("target_branch")?,
            base_commit: row.get("base_commit")?,
            integrated_commit: row.get("integrated_commit")?,
            expected_head_before: row.get("expected_head_before")?,
            created_at_ms: row.get("created_at_ms")?,
        },
        unit_commit: row.get("unit_commit")?,
        tree_oid: row.get("tree_oid")?,
    })
}

// Keep the unused-symbol surface honest: these helpers only serve
// the `supervisor-db` compilation branch.
#[allow(dead_code)]
fn _feature_marker() {}

// ===========================================================================
// Tests. The plan-store half mirrors the `dag_store_memory.rs`
// contract suite (per B2 plan U3 §14: the same contract runs
// against memory and rusqlite); the integration-store half
// mirrors `dag_integration.rs`. The rusqlite variants additionally
// pin the REOPEN semantics the memory variants cannot express
// (PMI-006: crash-window recovery / exactly-once projection).
// ===========================================================================
#[cfg(all(test, feature = "supervisor-db"))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn plan(key: &str, digest: &str) -> CanonicalPlanRecord {
        CanonicalPlanRecord {
            plan_key: key.to_string(),
            artifact_digest: digest.to_string(),
            target_branch: "feat/test".to_string(),
            unit_ids: vec!["U1".to_string(), "U2".to_string()],
            created_at_ms: 1_700_000_000_000,
        }
    }

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

    fn fresh_plan_store() -> (TempDir, RusqliteDagSchedulerStore) {
        let dir = TempDir::new().expect("tempdir");
        let store = RusqliteDagSchedulerStore::open(dir.path().join("dag.db"))
            .expect("open fresh plan store");
        (dir, store)
    }

    /// S15 fence: first insert wins the emit permit, same-key replay
    /// loses quietly, conflicting key fails closed, and the fence
    /// survives a reopen.
    #[test]
    fn terminal_emit_fence_is_exactly_once_and_durable() {
        let (dir, store) = fresh_plan_store();
        assert!(
            store
                .try_record_terminal_emit("pf", "forge.exec.development.done", "k1", 100)
                .expect("first insert")
        );
        assert!(
            !store
                .try_record_terminal_emit("pf", "forge.exec.development.done", "k1", 200)
                .expect("same-key replay"),
            "replay must not win the emit permit"
        );
        assert!(
            store
                .try_record_terminal_emit("pf", "forge.exec.development.done", "k2", 300)
                .is_err(),
            "conflicting idempotency key fails closed"
        );
        // A different topic for the same plan is an independent fence.
        assert!(
            store
                .try_record_terminal_emit("pf", "forge.plan.complete", "k9", 400)
                .expect("independent topic")
        );
        drop(store);
        let reopened = RusqliteDagSchedulerStore::open(dir.path().join("dag.db"))
            .expect("reopen");
        assert!(
            !reopened
                .try_record_terminal_emit("pf", "forge.exec.development.done", "k1", 500)
                .expect("replay after reopen"),
            "the fence survives process restart"
        );
    }

    // -----------------------------------------------------------------
    // Plan store contract (mirrors dag_store_memory.rs).
    // -----------------------------------------------------------------

    #[test]
    fn register_plan_creates_pending_row() {
        let (_dir, store) = fresh_plan_store();
        let reg = store.register_plan(&plan("p1", "d1")).expect("register");
        assert_eq!(reg.status, PlanStatus::Pending);
        assert_eq!(reg.plan_key, "p1");
        assert_eq!(reg.artifact_digest, "d1");
        assert!(reg.id > 0);
        assert_eq!(reg.unit_ids, vec!["U1".to_string(), "U2".to_string()]);
    }

    #[test]
    fn register_plan_is_idempotent_on_same_digest() {
        let (_dir, store) = fresh_plan_store();
        let first = store.register_plan(&plan("p1", "d1")).expect("first");
        let second = store.register_plan(&plan("p1", "d1")).expect("idempotent");
        assert_eq!(first.id, second.id);
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.id, first.id);
    }

    #[test]
    fn register_plan_fails_closed_on_digest_conflict() {
        let (_dir, store) = fresh_plan_store();
        store.register_plan(&plan("p1", "d1")).expect("first");
        let err = store
            .register_plan(&plan("p1", "d2"))
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
    fn activate_plan_transitions_pending_to_active() {
        let (_dir, store) = fresh_plan_store();
        store.register_plan(&plan("p1", "d1")).expect("register");
        store.activate_plan("p1", "feat/test").expect("activate");
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.status, PlanStatus::Active);
    }

    #[test]
    fn activate_plan_is_idempotent_on_active() {
        let (_dir, store) = fresh_plan_store();
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
        let (_dir, store) = fresh_plan_store();
        let err = store
            .activate_plan("missing", "feat/test")
            .expect_err("err");
        assert!(matches!(err, DagStoreError::UnknownPlan(_)));
    }

    #[test]
    fn get_plan_returns_none_for_missing_key() {
        let (_dir, store) = fresh_plan_store();
        let fetched = store.get_plan("missing").expect("get");
        assert!(fetched.is_none());
    }

    #[test]
    fn list_active_plans_filters_by_active_only() {
        let (_dir, store) = fresh_plan_store();
        store.register_plan(&plan("p1", "d1")).expect("register p1");
        store.register_plan(&plan("p2", "d2")).expect("register p2");
        store.activate_plan("p1", "feat/test").expect("activate p1");
        let actives = store.list_active_plans().expect("list");
        assert_eq!(actives.len(), 1);
        assert_eq!(actives[0].plan_key, "p1");
    }

    #[test]
    fn activate_plan_fails_closed_on_pending_target_mismatch() {
        let (_dir, store) = fresh_plan_store();
        store.register_plan(&plan("p1", "d1")).expect("register");
        let err = store
            .activate_plan("p1", "feat/OTHER")
            .expect_err("mismatch must fail closed");
        assert!(matches!(err, DagStoreError::TargetMismatch { .. }));
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.status, PlanStatus::Pending);
        assert_eq!(fetched.target_branch, "feat/test");
    }

    #[test]
    fn activate_plan_fails_closed_on_active_target_mismatch() {
        let (_dir, store) = fresh_plan_store();
        store.register_plan(&plan("p1", "d1")).expect("register");
        store
            .activate_plan("p1", "feat/test")
            .expect("first activate");
        let err = store
            .activate_plan("p1", "feat/OTHER")
            .expect_err("active mismatch must fail closed");
        assert!(matches!(err, DagStoreError::TargetMismatch { .. }));
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.status, PlanStatus::Active);
        assert_eq!(fetched.target_branch, "feat/test");
    }

    #[test]
    fn activate_plan_on_closed_returns_invalid_transition() {
        let (_dir, store) = fresh_plan_store();
        store.register_plan(&plan("p1", "d1")).expect("register");
        // Close the row directly (Closed is terminal; no public
        // close API on the trait).
        {
            let conn = store.inner.conn.lock().expect("conn");
            conn.execute(
                "UPDATE dag_plans SET status = 'closed' WHERE plan_key = 'p1'",
                [],
            )
            .expect("close row");
        }
        let err = store
            .activate_plan("p1", "feat/test")
            .expect_err("closed must reject activation");
        assert!(matches!(err, DagStoreError::InvalidTransition { .. }));
    }

    // -----------------------------------------------------------------
    // Receipt store contract (mirrors the memory suite in
    // dag_plan_receipt.rs) + reopen semantics (PMI-013 / S12 / S20 —
    // the memory variant cannot express reopen).
    // -----------------------------------------------------------------

    use crate::supervisor::dag_plan_receipt::{DagPlanReceiptRegistry, ReceiptStatus};

    fn receipt(key: &str, digest: &str) -> DagPlanReceipt {
        DagPlanReceipt::new(
            key,
            format!("/tmp/{key}.yaml"),
            digest,
            "feat/test",
            1_700_000_000_000,
        )
    }

    fn fresh_receipt_store() -> (TempDir, RusqliteDagPlanReceiptStore) {
        let dir = TempDir::new().expect("tempdir");
        let store = RusqliteDagPlanReceiptStore::open(dir.path().join("dag.db"))
            .expect("open fresh receipt store");
        (dir, store)
    }

    #[test]
    fn record_receipt_creates_pending_row() {
        let (_dir, store) = fresh_receipt_store();
        let recorded = store.record_receipt(&receipt("p1", "d1")).expect("record");
        assert!(recorded);
        let fetched = store.get_receipt("p1").expect("get").expect("exists");
        assert_eq!(fetched.status, ReceiptStatus::Pending);
        assert_eq!(fetched.artifact_path, "/tmp/p1.yaml");
        assert_eq!(fetched.target_branch, "feat/test");
        assert_eq!(fetched.activated_at_ms, None);
        assert_eq!(fetched.consumed_at_ms, None);
    }

    #[test]
    fn record_receipt_is_idempotent_on_same_digest() {
        let (_dir, store) = fresh_receipt_store();
        assert!(store.record_receipt(&receipt("p1", "d1")).expect("first"));
        assert!(
            !store
                .record_receipt(&receipt("p1", "d1"))
                .expect("idempotent")
        );
        // A replay with a different created_at_ms must not overwrite
        // the durable row.
        let mut replayed = receipt("p1", "d1");
        replayed.created_at_ms = 42;
        assert!(!store.record_receipt(&replayed).expect("idempotent"));
        let fetched = store.get_receipt("p1").expect("get").expect("exists");
        assert_eq!(fetched.created_at_ms, 1_700_000_000_000);
        assert_eq!(store.list_receipts().expect("list").len(), 1);
    }

    #[test]
    fn record_receipt_fails_closed_on_digest_conflict() {
        let (_dir, store) = fresh_receipt_store();
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
        // The pre-existing row is untouched by the rejected write.
        let fetched = store.get_receipt("p1").expect("get").expect("exists");
        assert_eq!(fetched.artifact_digest, "d1");
        assert_eq!(fetched.status, ReceiptStatus::Pending);
    }

    #[test]
    fn get_receipt_returns_none_for_missing_key() {
        let (_dir, store) = fresh_receipt_store();
        assert!(store.get_receipt("missing").expect("get").is_none());
    }

    #[test]
    fn list_receipts_returns_every_recorded_receipt() {
        let (_dir, store) = fresh_receipt_store();
        store.record_receipt(&receipt("p1", "d1")).expect("p1");
        store.record_receipt(&receipt("p2", "d2")).expect("p2");
        let all = store.list_receipts().expect("list");
        assert_eq!(all.len(), 2);
        let mut keys: Vec<&str> = all.iter().map(|r| r.plan_key.as_str()).collect();
        keys.sort();
        assert_eq!(keys, vec!["p1", "p2"]);
    }

    #[test]
    fn activate_receipt_transitions_pending_to_active() {
        let (_dir, store) = fresh_receipt_store();
        store.record_receipt(&receipt("p1", "d1")).expect("record");
        let activated = store
            .activate_receipt("p1", 1_700_000_001_000)
            .expect("activate");
        assert_eq!(activated.status, ReceiptStatus::Active);
        assert_eq!(activated.activated_at_ms, Some(1_700_000_001_000));
    }

    #[test]
    fn activate_receipt_is_idempotent_on_active() {
        let (_dir, store) = fresh_receipt_store();
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
    fn activate_receipt_unknown_key_returns_error() {
        let (_dir, store) = fresh_receipt_store();
        let err = store
            .activate_receipt("missing", 1_700_000_001_000)
            .expect_err("err");
        assert!(matches!(err, DagStoreError::UnknownPlan(_)));
    }

    #[test]
    fn activate_receipt_consumed_returns_invalid_transition() {
        let (_dir, store) = fresh_receipt_store();
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
    fn consume_receipt_transitions_and_keeps_activation_stamp() {
        let (_dir, store) = fresh_receipt_store();
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
    fn consume_receipt_is_idempotent_on_consumed() {
        let (_dir, store) = fresh_receipt_store();
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
    fn consume_receipt_unknown_key_returns_error() {
        let (_dir, store) = fresh_receipt_store();
        let err = store
            .consume_receipt("missing", 1_700_000_001_000)
            .expect_err("err");
        assert!(matches!(err, DagStoreError::UnknownPlan(_)));
    }

    /// PMI-013 / S12 / S20: receipts survive a close/reopen cycle —
    /// after a crash the store can list the recorded receipt,
    /// activate it, and consume it, each in a separate process
    /// lifetime.
    #[test]
    fn receipt_survives_reopen_list_activate_consume() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("dag.db");

        // Lifetime 1: record at the accepted boundary, then crash.
        {
            let store = RusqliteDagPlanReceiptStore::open(&path).expect("open 1");
            store.record_receipt(&receipt("p1", "d1")).expect("record");
        }

        // Lifetime 2: recovery lists the receipt and activates it.
        {
            let store = RusqliteDagPlanReceiptStore::open(&path).expect("open 2");
            let all = store.list_receipts().expect("list");
            assert_eq!(all.len(), 1);
            assert_eq!(all[0].plan_key, "p1");
            assert_eq!(all[0].status, ReceiptStatus::Pending);
            let activated = store
                .activate_receipt("p1", 1_700_000_001_000)
                .expect("activate");
            assert_eq!(activated.status, ReceiptStatus::Active);
        }

        // Lifetime 3: consume the activated receipt.
        {
            let store = RusqliteDagPlanReceiptStore::open(&path).expect("open 3");
            let consumed = store
                .consume_receipt("p1", 1_700_000_002_000)
                .expect("consume");
            assert_eq!(consumed.status, ReceiptStatus::Consumed);
        }

        // Lifetime 4: the terminal row round-trips with both stamps.
        {
            let store = RusqliteDagPlanReceiptStore::open(&path).expect("open 4");
            let fetched = store.get_receipt("p1").expect("get").expect("exists");
            assert_eq!(fetched.status, ReceiptStatus::Consumed);
            assert_eq!(fetched.activated_at_ms, Some(1_700_000_001_000));
            assert_eq!(fetched.consumed_at_ms, Some(1_700_000_002_000));
            assert_eq!(fetched.artifact_digest, "d1");
        }
    }

    /// The runtime-facing registry is durable: `open` on the same
    /// path recovers the receipt, and activate/consume work after
    /// the reopen (the in-process cache is rebuilt from the DB
    /// authority, not trusted across processes).
    #[test]
    fn registry_open_recovers_receipt_across_reopen() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("dag.db");

        {
            let reg = DagPlanReceiptRegistry::open(&path).expect("open 1");
            assert!(reg.record(receipt("p1", "d1")).expect("record"));
            assert_eq!(
                reg.cached("p1").expect("cached").status,
                ReceiptStatus::Pending
            );
        }

        {
            let reg = DagPlanReceiptRegistry::open(&path).expect("open 2");
            // Fresh process: the cache starts empty but the store
            // (authority) still has the receipt.
            assert!(reg.cached("p1").is_none());
            let fetched = reg.get("p1").expect("get").expect("exists");
            assert_eq!(fetched.status, ReceiptStatus::Pending);
            // Reads refresh the cache.
            assert!(reg.cached("p1").is_some());
            let activated = reg.activate("p1", 1_700_000_001_000).expect("activate");
            assert_eq!(activated.status, ReceiptStatus::Active);
            let consumed = reg.consume("p1", 1_700_000_002_000).expect("consume");
            assert_eq!(consumed.status, ReceiptStatus::Consumed);
        }

        {
            let reg = DagPlanReceiptRegistry::open(&path).expect("open 3");
            let fetched = reg.get("p1").expect("get").expect("exists");
            assert_eq!(fetched.status, ReceiptStatus::Consumed);
            // Idempotent re-record after reopen: same digest → no-op.
            assert!(!reg.record(receipt("p1", "d1")).expect("idempotent"));
            // Digest drift after reopen still fails closed.
            let err = reg.record(receipt("p1", "d2")).expect_err("conflict");
            assert!(matches!(err, DagStoreError::DigestConflict { .. }));
        }
    }

    /// The plan store can hand off a receipt store sharing the same
    /// connection / file, so receipts and plan registrations live in
    /// one database.
    #[test]
    fn plan_store_shares_connection_with_receipt_store() {
        let (_dir, plans) = fresh_plan_store();
        let receipts = plans.shared_with_receipts();
        receipts
            .record_receipt(&receipt("p1", "d1"))
            .expect("record");
        plans.register_plan(&plan("p1", "d1")).expect("register");
        let fetched = receipts.get_receipt("p1").expect("get").expect("exists");
        assert_eq!(fetched.status, ReceiptStatus::Pending);
    }

    // -----------------------------------------------------------------
    // Integration store contract (mirrors dag_integration.rs).
    // -----------------------------------------------------------------

    fn fresh_integration_store() -> (TempDir, RusqliteIntegrationStore) {
        let dir = TempDir::new().expect("tempdir");
        let store = RusqliteIntegrationStore::open(dir.path().join("dag.db"))
            .expect("open fresh integration store");
        (dir, store)
    }

    #[test]
    fn record_integrated_creates_new_row() {
        let (_dir, store) = fresh_integration_store();
        let rec = store
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("record");
        assert_eq!(rec.unit_id, "U1");
        assert_eq!(rec.target_branch, "feat/test");
        assert!(!rec.acked);
    }

    #[test]
    fn record_integrated_is_idempotent_on_same_tuple() {
        let (_dir, store) = fresh_integration_store();
        let a = store
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("first");
        let b = store
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("idempotent");
        assert_eq!(a.id, b.id);
        assert_eq!(a.commit_fingerprint, b.commit_fingerprint);
        let rows = store.list_for_unit("U1").expect("list");
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn record_integrated_rejects_duplicate_unit_for_target() {
        let (_dir, store) = fresh_integration_store();
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

    #[test]
    fn ack_flips_acked_bit_and_is_idempotent() {
        let (_dir, store) = fresh_integration_store();
        let rec = store
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("record");
        let acked = store.ack("U1", "feat/test").expect("ack");
        assert_eq!(acked.id, rec.id);
        assert!(acked.acked);
        let acked_again = store.ack("U1", "feat/test").expect("ack again");
        assert_eq!(acked_again, acked);
        // Unacked filter: after ack, list_unacked is empty.
        let unacked = store
            .list_unacked_for_target("feat/test")
            .expect("list unacked");
        assert!(unacked.is_empty());
    }

    #[test]
    fn ack_returns_not_yet_recorded_for_ghost_unit() {
        let (_dir, store) = fresh_integration_store();
        let err = store
            .ack("ghost-unit", "feat/test")
            .expect_err("ghost unit was never recorded");
        assert!(matches!(err, IntegrationStoreError::NotYetRecorded { .. }));
    }

    #[test]
    fn list_unacked_filters_by_target_and_acked_flag() {
        let (_dir, store) = fresh_integration_store();
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
        assert!(unacked.is_empty(), "U1 was acked, so none unacked");
        let unacked_other = store
            .list_unacked_for_target("feat/other")
            .expect("list unacked other");
        assert_eq!(unacked_other.len(), 1);
        assert_eq!(unacked_other[0].unit_id, "U2");
    }

    #[test]
    fn list_for_unit_returns_all_targets() {
        let (_dir, store) = fresh_integration_store();
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
    }

    // -----------------------------------------------------------------
    // REOPEN semantics (the rusqlite-only half — PMI-006's
    // crash-window recovery core; the memory variants cannot
    // express these).
    // -----------------------------------------------------------------

    #[test]
    fn reopen_preserves_plan_rows_and_activation() {
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("dag.db");
        {
            let store = RusqliteDagSchedulerStore::open(&db).expect("open A");
            store.register_plan(&plan("p1", "d1")).expect("register");
            store.activate_plan("p1", "feat/test").expect("activate");
        }
        let store_b = RusqliteDagSchedulerStore::open(&db).expect("reopen B");
        let fetched = store_b.get_plan("p1").expect("get").expect("survives");
        assert_eq!(fetched.status, PlanStatus::Active);
        assert_eq!(fetched.artifact_digest, "d1");
        // Re-registration after reopen is idempotent (R2/R17
        // restart happy path).
        let replay = store_b.register_plan(&plan("p1", "d1")).expect("replay");
        assert_eq!(replay.id, fetched.id);
    }

    #[test]
    fn reopen_preserves_integration_records_and_ack_state() {
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("dag.db");
        {
            let store = RusqliteIntegrationStore::open(&db).expect("open A");
            store
                .record_integrated(&input("U1", "b1", "i1", "h1"))
                .expect("record");
            store.ack("U1", "feat/test").expect("ack");
        }
        let store_b = RusqliteIntegrationStore::open(&db).expect("reopen B");
        let rows = store_b.list_for_unit("U1").expect("list");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].acked, "ack state survives the reopen");
        // Same-tuple replay across reopen is idempotent.
        let replay = store_b
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("replay");
        assert_eq!(replay.id, rows[0].id);
        assert_eq!(replay.commit_fingerprint, rows[0].commit_fingerprint);
        // Drifted tuple still fails closed after reopen.
        let mut drifted = input("U1", "b1", "i1", "h1");
        drifted.base_commit = "b2".to_string();
        let err = store_b
            .record_integrated(&drifted)
            .expect_err("drift must fail closed");
        assert!(matches!(
            err,
            IntegrationStoreError::DuplicateUnitForTarget { .. }
        ));
    }

    #[test]
    fn plan_and_integration_stores_share_one_database() {
        // The two state families share one connection/file via
        // `shared_with_*` (single-authority, one file — 04 audit).
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("dag.db");
        let plans = RusqliteDagSchedulerStore::open(&db).expect("open plans");
        let integrations = plans.shared_with_integration();
        plans.register_plan(&plan("p1", "d1")).expect("register");
        integrations
            .record_integrated(&input("U1", "b1", "i1", "h1"))
            .expect("record");
        // Both families reopen together.
        let plans_b = RusqliteDagSchedulerStore::open(&db).expect("reopen plans");
        let integrations_b = plans_b.shared_with_integration();
        assert!(plans_b.get_plan("p1").expect("get").is_some());
        assert_eq!(integrations_b.list_for_unit("U1").expect("list").len(), 1);
    }

    #[test]
    fn open_failure_is_typed_not_panicking() {
        // A path that cannot be a database (a directory) fails
        // closed with the typed IO error, not a panic.
        let dir = TempDir::new().expect("tempdir");
        let not_a_db = dir.path().join("not-a-db");
        std::fs::create_dir_all(&not_a_db).expect("mkdir");
        let err =
            RusqliteDagSchedulerStore::open(&not_a_db).expect_err("opening a directory must fail");
        assert!(matches!(err, DagStoreError::IoError(_)));
        let err_i =
            RusqliteIntegrationStore::open(&not_a_db).expect_err("opening a directory must fail");
        assert!(matches!(err_i, IntegrationStoreError::StorageIo(_)));
    }

    // -----------------------------------------------------------------
    // Cross-process concurrency (P1 fix pins). Each thread opens
    // its OWN connection to the same file — the same race shape
    // as two OS processes sharing one database, which the
    // in-process mutex cannot serialise. A barrier maximises the
    // overlap of the two writers.
    // -----------------------------------------------------------------

    use std::sync::Barrier;

    /// Run `f` concurrently from two threads, each with its own
    /// store connection on `db`, released simultaneously by a
    /// barrier. Returns both results in spawn order.
    fn race_two_handles<T: Send + 'static>(
        db: &std::path::Path,
        f: impl Fn(RusqliteDagSchedulerStore) -> DagStoreResult<T> + Send + Sync + 'static,
    ) -> [DagStoreResult<T>; 2] {
        let barrier = Arc::new(Barrier::new(2));
        let f = Arc::new(f);
        let mut handles = Vec::new();
        for _ in 0..2 {
            let db = db.to_path_buf();
            let barrier = Arc::clone(&barrier);
            let f = Arc::clone(&f);
            handles.push(std::thread::spawn(move || {
                let store = RusqliteDagSchedulerStore::open(&db).expect("open racing handle");
                barrier.wait();
                f(store)
            }));
        }
        let mut results = handles
            .into_iter()
            .map(|h| h.join().expect("racing thread panicked"));
        [
            results.next().expect("two results"),
            results.next().expect("two results"),
        ]
    }

    #[test]
    fn concurrent_register_same_key_same_digest_both_ok_idempotent() {
        // Two racing handles register the same (plan_key, digest):
        // BOTH must observe Ok with the SAME row id (contract
        // idempotency), never a raw UNIQUE-constraint IO error.
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("dag.db");
        let [a, b] = race_two_handles(&db, |store| store.register_plan(&plan("p1", "d1")));
        let a = a.expect("first racer must be Ok");
        let b = b.expect("second racer must be Ok");
        assert_eq!(a.id, b.id, "both racers must observe the one row");
        let store = RusqliteDagSchedulerStore::open(&db).expect("reopen");
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.id, a.id);
        assert_eq!(fetched.status, PlanStatus::Pending);
    }

    #[test]
    fn concurrent_register_same_key_different_digest_exactly_one_conflict() {
        // Digest drift under race: EXACTLY ONE side wins the
        // insert; the loser MUST get DigestConflict (fail closed),
        // never a spurious IO error from the UNIQUE constraint.
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("dag.db");
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for digest in ["d1", "d2"] {
            let db = db.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let store = RusqliteDagSchedulerStore::open(&db).expect("open racing handle");
                barrier.wait();
                store.register_plan(&plan("p1", digest))
            }));
        }
        let results: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().expect("racing thread panicked"))
            .collect();
        let oks = results.iter().filter(|r| r.is_ok()).count();
        let conflicts = results
            .iter()
            .filter(|r| matches!(r, Err(DagStoreError::DigestConflict { .. })))
            .count();
        assert_eq!(oks, 1, "exactly one racer wins: {results:?}");
        assert_eq!(conflicts, 1, "exactly one racer conflicts: {results:?}");
        // Whichever digest won is the single persisted row.
        let store = RusqliteDagSchedulerStore::open(&db).expect("reopen");
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        let winner = results
            .iter()
            .find_map(|r| r.as_ref().ok())
            .expect("one Ok");
        assert_eq!(fetched.id, winner.id);
        assert_eq!(fetched.artifact_digest, winner.artifact_digest);
    }

    #[test]
    fn concurrent_activate_exactly_one_pending_to_active_transition() {
        // Two racing handles activate the same Pending plan: both
        // observe Ok (activation is idempotent on Active), the
        // conditional UPDATE guarantees at most one performs the
        // Pending → Active write, and the final state is Active.
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("dag.db");
        {
            let store = RusqliteDagSchedulerStore::open(&db).expect("open");
            store.register_plan(&plan("p1", "d1")).expect("register");
        }
        let [a, b] = race_two_handles(&db, |store| store.activate_plan("p1", "feat/test"));
        a.expect("first activation must be Ok");
        b.expect("second activation must be Ok (idempotent on active)");
        let store = RusqliteDagSchedulerStore::open(&db).expect("reopen");
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.status, PlanStatus::Active);
    }

    #[test]
    fn concurrent_activate_vs_mismatch_fails_closed_without_transition() {
        // One racer activates with the registered branch; the
        // other with a WRONG branch. The mismatch racer MUST get
        // TargetMismatch and MUST NOT flip the status itself —
        // the row ends Active only via the legitimate racer.
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("dag.db");
        {
            let store = RusqliteDagSchedulerStore::open(&db).expect("open");
            store.register_plan(&plan("p1", "d1")).expect("register");
        }
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for branch in ["feat/test", "feat/OTHER"] {
            let db = db.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let store = RusqliteDagSchedulerStore::open(&db).expect("open racing handle");
                barrier.wait();
                store.activate_plan("p1", branch)
            }));
        }
        let results: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().expect("racing thread panicked"))
            .collect();
        let oks = results.iter().filter(|r| r.is_ok()).count();
        let mismatches = results
            .iter()
            .filter(|r| matches!(r, Err(DagStoreError::TargetMismatch { .. })))
            .count();
        assert_eq!(oks, 1, "legit racer activates: {results:?}");
        assert_eq!(
            mismatches, 1,
            "wrong-branch racer fails closed: {results:?}"
        );
        let store = RusqliteDagSchedulerStore::open(&db).expect("reopen");
        let fetched = store.get_plan("p1").expect("get").expect("exists");
        assert_eq!(fetched.status, PlanStatus::Active);
        assert_eq!(fetched.target_branch, "feat/test");
    }
}
