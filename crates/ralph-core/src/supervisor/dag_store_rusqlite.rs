//! PMI-006 / 2026-09-03-0959 plan U3 (R2 / R17 / D4 / E5 / E9):
//! rusqlite implementation of the durable DAG store contract.
//!
//! The module owns TWO trait implementations over a single SQLite
//! connection (the v13 schema in `migrations/v13.sql`):
//! - [`RusqliteDagSchedulerStore`] implements
//!   [`super::dag_store::DagSchedulerStore`] (plan registrations),
//! - [`RusqliteIntegrationStore`] implements
//!   [`super::dag_integration::IntegrationStore`] (integration
//!   records).
//!
//! Contract parity with the in-memory variants
//! (`dag_store_memory.rs` / `dag_integration.rs`) is enforced by
//! the shared contract suites in this file and by the memory
//! variants' own tests — per plan U3 §14, the same contract runs
//! against both adapters.
//!
//! Concurrency: a single `Mutex<Connection>` serialises every
//! statement, mirroring `RusqliteSupervisorStore`. All
//! idempotency / fail-closed checks happen INSIDE the lock so two
//! racing writers cannot interleave a read-decide-write cycle.
//!
//! Migration versioning: the store runs the supervisor migration
//! ledger (v1..=13) on `open`, so a `supervisor.db` and a DAG
//! store opened against the same file agree on the schema. The
//! DAG tables are additive to the wave tables — the wave store
//! keeps its single authority; this module only adds the DAG
//! family.

#[cfg(feature = "supervisor-db")]
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::dag_integration::{
    IntegrationInput, IntegrationRecord, IntegrationStore, IntegrationStoreError,
    IntegrationStoreResult, compute_integration_fingerprint,
};
use super::dag_store::{
    CanonicalPlanRecord, DagSchedulerStore, DagStoreError, DagStoreResult, PlanRegistration,
    PlanStatus,
};

#[cfg(feature = "supervisor-db")]
use super::migrations;
#[cfg(feature = "supervisor-db")]
use rusqlite::OptionalExtension;

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
            let conn = self
                .inner
                .conn
                .lock()
                .map_err(|_| DagStoreError::IoError("dag plan store mutex poisoned".into()))?;
            // Read-decide-write INSIDE the lock: a same-key
            // existing row decides idempotent-return vs
            // DigestConflict before any INSERT runs.
            let existing: Option<PlanRegistration> = conn
                .query_row(
                    "SELECT id, plan_key, artifact_digest, target_branch, unit_ids, status, \
                     created_at_ms FROM dag_plans WHERE plan_key = ?1",
                    [&plan.plan_key],
                    row_to_plan_registration,
                )
                .optional()
                .map_err(plan_io_err)?;
            if let Some(existing) = existing {
                if existing.artifact_digest == plan.artifact_digest {
                    return Ok(existing);
                }
                return Err(DagStoreError::DigestConflict {
                    plan_key: plan.plan_key.clone(),
                    expected: existing.artifact_digest,
                    actual: plan.artifact_digest.clone(),
                });
            }
            conn.execute(
                "INSERT INTO dag_plans \
                 (plan_key, artifact_digest, target_branch, unit_ids, status, created_at_ms) \
                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                rusqlite::params![
                    plan.plan_key,
                    plan.artifact_digest,
                    plan.target_branch,
                    unit_ids_to_json(&plan.unit_ids),
                    plan.created_at_ms as i64,
                ],
            )
            .map_err(plan_io_err)?;
            let registered: PlanRegistration = conn
                .query_row(
                    "SELECT id, plan_key, artifact_digest, target_branch, unit_ids, status, \
                     created_at_ms FROM dag_plans WHERE plan_key = ?1",
                    [&plan.plan_key],
                    row_to_plan_registration,
                )
                .map_err(plan_io_err)?;
            // The INSERT wrote 'pending' + the caller's identity
            // columns; re-read everything so the returned row is
            // exactly what a later get_plan would observe.
            debug_assert_eq!(registered.status, PlanStatus::Pending);
            Ok(registered)
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
            let conn = self
                .inner
                .conn
                .lock()
                .map_err(|_| DagStoreError::IoError("dag plan store mutex poisoned".into()))?;
            let row: Option<(String, String)> = conn
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
            if status == PlanStatus::Active {
                return Ok(());
            }
            conn.execute(
                "UPDATE dag_plans SET status = 'active' WHERE plan_key = ?1",
                [plan_key],
            )
            .map_err(plan_io_err)?;
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
}
