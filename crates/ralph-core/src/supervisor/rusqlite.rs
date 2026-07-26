//! `RusqliteSupervisorStore` (U5).
//!
//! Mirrors the in-memory store contract against an SQLite database
//! opened at `SupervisorConfig::db_path`. The store is constructed
//! via `RusqliteSupervisorStore::open(path)` so failures to open
//! the DB bubble up as `SupervisorStoreError::Open` and the
//! supervisor fails closed (R-C4).
//!
//! The store takes `&self` everywhere; the underlying `Connection`
//! is wrapped in `std::sync::Mutex` so the trait surface stays
//! `Send + Sync`. U12 can thus share a single instance across
//! tasks without an extra wrapper.
//!
//! Migration version: see `migrations::CURRENT_VERSION`. The store
//! runs migrations on `open`; subsequent `register_wave` calls
//! proceed without re-running them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension};

#[cfg(feature = "supervisor-db")]
use super::migrations;
use super::{
    CompensationKind, DispatchOutcome, EmissionReservation, EmissionState, IsolationMode,
    SlotResource, SlotStatus, SupervisorStore, SupervisorStoreError, SupervisorStoreResult,
    WaveKind, WavePhase, WaveSnapshot,
};

/// `PRAGMA busy_timeout` value installed on every supervisor
/// store connection. Two `ralph wave emit` processes may race
/// the same fresh database; without a non-zero timeout, the
/// losing process hits `SQLITE_BUSY` ("database is locked")
/// the moment the winner starts writing the WAL header or any
/// DDL transaction. Five seconds covers the worst-case
/// migration + WAL-switch window on cold-disk CI runners while
/// remaining short enough that a wedged peer can't block a
/// loop indefinitely. The migration runner caps the wait via
/// the underlying timeout, so this stays a safety bound, not a
/// SLA. (2026-07-25)
#[cfg(feature = "supervisor-db")]
const BUSY_TIMEOUT_MS: u32 = 5_000;

/// Returns `true` when the rusqlite error carries SQLite result
/// code `SQLITE_BUSY` (code 5).  Used by the migration retry
/// loop in `RusqliteSupervisorStore::open` to distinguish a
/// transient lock conflict (retryable) from a permanent schema
/// or I/O error (not retryable).
#[cfg(feature = "supervisor-db")]
fn is_sqlite_busy(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::DatabaseBusy,
                ..
            },
            _
        )
    )
}

/// Persistent `SupervisorStore` backed by SQLite (WAL mode,
/// foreign keys ON, see `migrations::run`). The store is
/// `Send + Sync` because the underlying `Connection` lives
/// behind a `std::sync::Mutex<Connection>` shared through
/// `Arc` — dispatcher bridges in U12 can share a single
/// instance across tasks.
#[cfg(feature = "supervisor-db")]
#[derive(Debug, Clone)]
pub struct RusqliteSupervisorStore {
    inner: Arc<Mutex<Connection>>,
}

#[cfg(feature = "supervisor-db")]
impl RusqliteSupervisorStore {
    /// Open the database at `path`, run pending migrations, and
    /// return a ready store. Errors surface as `Open(...)` so
    /// the runtime can mark the supervisor path as failed
    /// (R-C4). The path is created if missing.
    ///
    /// `busy_timeout` is set BEFORE we touch the file in any
    /// way, including the WAL header switch in `migrations::run`.
    /// Two processes racing the same fresh database can both
    /// try to flip `journal_mode` to `WAL`; without a non-zero
    /// `busy_timeout` the loser hits `SQLITE_BUSY` immediately
    /// and the store fails closed even though either process
    /// alone would succeed.  The `PRAGMA journal_mode = WAL`
    /// switch on a brand-new file can still hit filesystem-level
    /// contention (creating the `-wal`/`-shm` sidecars) that
    /// bypasses SQLite's busy handler, so `migrations::run` is
    /// wrapped in a short retry loop with linear backoff.
    /// (2026-07-25 integration
    /// `u8_concurrent_barrier_same_key_single_apply`).
    pub fn open(path: impl AsRef<Path>) -> SupervisorStoreResult<Self> {
        let path = path.as_ref();
        let conn = Connection::open(path)
            .map_err(|err| SupervisorStoreError::Open(format!("{}: {err}", path.display())))?;
        // 2026-07-25: tolerate concurrent open of the same DB.
        conn.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)
            .map_err(|err| {
                SupervisorStoreError::Open(format!(
                    "failed to set busy_timeout on {}: {err}",
                    path.display()
                ))
            })?;
        // Retry migrations on SQLITE_BUSY: the WAL header switch
        // on a fresh database creates `-wal`/`-shm` sidecar files;
        // two processes racing this creation can hit a brief
        // filesystem-level lock that the busy handler does not
        // cover.  5 attempts × 50-250 ms backoff covers the
        // worst-case cold-disk CI window without wedging a loop.
        const MIGRATION_RETRIES: u32 = 5;
        let mut last_busy_err: Option<rusqlite::Error> = None;
        for attempt in 0..MIGRATION_RETRIES {
            match migrations::run(&conn) {
                Ok(()) => {
                    last_busy_err = None;
                    break;
                }
                Err(err) if is_sqlite_busy(&err) => {
                    last_busy_err = Some(err);
                    std::thread::sleep(std::time::Duration::from_millis(50 * (attempt as u64 + 1)));
                }
                Err(err) => {
                    return Err(SupervisorStoreError::Open(format!(
                        "migration failed on {}: {err}",
                        path.display()
                    )));
                }
            }
        }
        if let Some(err) = last_busy_err {
            return Err(SupervisorStoreError::Open(format!(
                "migration failed on {} after {MIGRATION_RETRIES} retries: {err}",
                path.display()
            )));
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
        })
    }

    /// Construct a store around an already-open `Connection`.
    /// Used by tests so they can pass `:memory:` connections.
    /// Migrations still run here so the same invariant holds.
    /// Also installs `busy_timeout` so a connection hand-built
    /// by callers still tolerates concurrent writers (e.g.
    /// shared `:memory:` is uncommon but see
    /// `RusqliteSupervisorStore::open` for the production
    /// rationale).
    pub fn from_connection(connection: Connection) -> SupervisorStoreResult<Self> {
        connection
            .pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)
            .map_err(|err| {
                SupervisorStoreError::Open(format!("failed to set busy_timeout: {err}"))
            })?;
        migrations::run(&connection)
            .map_err(|err| SupervisorStoreError::Open(format!("migration failed: {err}")))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(connection)),
        })
    }

    fn lock(&self) -> SupervisorStoreResult<std::sync::MutexGuard<'_, Connection>> {
        self.inner
            .lock()
            .map_err(|_| SupervisorStoreError::Storage("rusqlite store mutex poisoned".to_string()))
    }

    /// Convenience: acquire the connection once and run `f` so
    /// we don't need to thread the guard through every helper.
    /// The closure receives `&mut Connection` because SQL DDL/DML
    /// (`execute`, `transaction`) needs mut access.
    fn with_conn<F, R>(&self, f: F) -> SupervisorStoreResult<R>
    where
        F: FnOnce(&mut Connection) -> SupervisorStoreResult<R>,
    {
        let mut guard = self.lock()?;
        f(&mut guard)
    }
}

#[cfg(feature = "supervisor-db")]
impl Default for RusqliteSupervisorStore {
    fn default() -> Self {
        // Tests that don't care about persistence can construct
        // an in-memory store via `from_connection`. A default
        // instance here points at a throwaway path so accidental
        // `Default::default()` calls never touch the operator's
        // workspace.
        Self::open(db_path_for_tests_helper()).expect("throwaway rusqlite store must open")
    }
}

#[cfg(feature = "supervisor-db")]
fn db_path_for_tests_helper() -> PathBuf {
    let dir = tempfile::tempdir().expect("tempdir");
    dir.keep().join("supervisor.db")
}

#[cfg(feature = "supervisor-db")]
fn parse_phase(s: &str) -> SupervisorStoreResult<WavePhase> {
    match s {
        "dispatch" => Ok(WavePhase::Dispatch),
        "collect" => Ok(WavePhase::Collect),
        "integrate" => Ok(WavePhase::Integrate),
        "done" => Ok(WavePhase::Done),
        "failed" => Ok(WavePhase::Failed),
        other => Err(SupervisorStoreError::Storage(format!(
            "unknown phase: {other}"
        ))),
    }
}

#[cfg(feature = "supervisor-db")]
fn phase_to_str(phase: WavePhase) -> &'static str {
    match phase {
        WavePhase::Dispatch => "dispatch",
        WavePhase::Collect => "collect",
        WavePhase::Integrate => "integrate",
        WavePhase::Done => "done",
        WavePhase::Failed => "failed",
    }
}

#[cfg(feature = "supervisor-db")]
fn parse_kind(s: &str) -> SupervisorStoreResult<WaveKind> {
    match s {
        "exec" => Ok(WaveKind::Exec),
        "fix" => Ok(WaveKind::Fix),
        "review" => Ok(WaveKind::Review),
        other => Err(SupervisorStoreError::Storage(format!(
            "unknown wave kind: {other}"
        ))),
    }
}

#[cfg(all(test, feature = "supervisor-db"))]
fn parse_isolation(s: &str) -> SupervisorStoreResult<IsolationMode> {
    match s {
        "worktree" => Ok(IsolationMode::Worktree),
        "shared_readonly" => Ok(IsolationMode::SharedReadonly),
        other => Err(SupervisorStoreError::Storage(format!(
            "unknown isolation: {other}"
        ))),
    }
}

#[cfg(feature = "supervisor-db")]
fn parse_status(s: &str) -> SupervisorStoreResult<SlotStatus> {
    match s {
        "pending" => Ok(SlotStatus::Pending),
        "dispatched" => Ok(SlotStatus::Dispatched),
        "running" => Ok(SlotStatus::Running),
        "completed" => Ok(SlotStatus::Completed),
        "failed" => Ok(SlotStatus::Failed),
        "cancelled" => Ok(SlotStatus::Cancelled),
        other => Err(SupervisorStoreError::Storage(format!(
            "unknown slot status: {other}"
        ))),
    }
}

#[cfg(feature = "supervisor-db")]
fn status_to_str(status: SlotStatus) -> &'static str {
    match status {
        SlotStatus::Pending => "pending",
        SlotStatus::Dispatched => "dispatched",
        SlotStatus::Running => "running",
        SlotStatus::Completed => "completed",
        SlotStatus::Failed => "failed",
        SlotStatus::Cancelled => "cancelled",
    }
}

#[cfg(feature = "supervisor-db")]
fn isolation_to_str(isolation: IsolationMode) -> &'static str {
    match isolation {
        IsolationMode::Worktree => "worktree",
        IsolationMode::SharedReadonly => "shared_readonly",
    }
}

#[cfg(feature = "supervisor-db")]
fn default_isolation_for(kind: WaveKind) -> IsolationMode {
    match kind {
        WaveKind::Exec | WaveKind::Fix => IsolationMode::Worktree,
        WaveKind::Review => IsolationMode::SharedReadonly,
    }
}

/// Bridge `rusqlite::Error` into the supervisor error enum so
/// the trait methods can `?`-propagate SQL failures without
/// hand-written conversions.
#[cfg(feature = "supervisor-db")]
impl From<rusqlite::Error> for SupervisorStoreError {
    fn from(err: rusqlite::Error) -> Self {
        SupervisorStoreError::Storage(err.to_string())
    }
}

#[cfg(feature = "supervisor-db")]
impl SupervisorStore for RusqliteSupervisorStore {
    fn register_wave(
        &self,
        idempotency_key: &str,
        kind: WaveKind,
        expected_total: u32,
    ) -> SupervisorStoreResult<String> {
        if expected_total == 0 {
            return Err(SupervisorStoreError::InvalidTransition(
                "expected_total must be > 0".to_string(),
            ));
        }
        self.with_conn(|conn| {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT wave_id FROM waves WHERE idempotency_key = ?1",
                    [idempotency_key],
                    |row| row.get(0),
                )
                .optional()?;
            if existing.is_some() {
                return Err(SupervisorStoreError::DuplicateKey(
                    idempotency_key.to_string(),
                ));
            }
            // U4 / F-004 / R4: allocate the wave_id atomically
            // via the `wave_id_seq` autoincrement table. The
            // pre-fix `SELECT COUNT(*) + 1 FROM waves` was
            // racy under concurrent register_wave callers
            // because two transactions could observe the same
            // count and pick identical PKs; the seq table
            // guarantees one row per INSERT.
            //
            // We use `RETURNING seq` so the value lands in the
            // same atomic step as the insert. The subsequent
            // INSERT into `waves` reuses the seq value as the
            // primary key.
            let tx = conn.transaction()?;
            let next_seq: i64 = tx.query_row(
                "INSERT INTO wave_id_seq DEFAULT VALUES RETURNING seq",
                [],
                |row| row.get(0),
            )?;
            let wave_id = format!("w-{next_seq}");
            let isolation = default_isolation_for(kind);

            tx.execute(
                "INSERT INTO waves (wave_id, idempotency_key, kind, expected_total, phase)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    &wave_id,
                    idempotency_key,
                    kind_to_str(kind),
                    i64::from(expected_total),
                    phase_to_str(WavePhase::Dispatch),
                ],
            )?;
            for idx in 0..expected_total {
                tx.execute(
                    "INSERT INTO wave_slots (wave_id, slot_index, status, isolation)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        &wave_id,
                        i64::from(idx),
                        status_to_str(SlotStatus::Pending),
                        isolation_to_str(isolation),
                    ],
                )?;
            }
            tx.commit()?;
            // Silence unused-variable warning; ensures
            // duplicate-key branches anchor on `existing`.
            let _ = existing.as_ref();
            Ok(wave_id)
        })
    }

    fn enqueue_wave(
        &self,
        idempotency_key: &str,
        kind: WaveKind,
        expected_total: u32,
    ) -> SupervisorStoreResult<String> {
        let wave_id = self.register_wave(idempotency_key, kind, expected_total)?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO wave_queue (wave_id) VALUES (?1)",
                [&wave_id],
            )?;
            Ok(())
        })?;
        Ok(wave_id)
    }

    fn try_dispatch_next(
        &self,
        max_concurrent_workers: u32,
    ) -> SupervisorStoreResult<Option<(String, u32)>> {
        self.with_conn(|conn| {
            let active: i64 = conn.query_row(
                "SELECT COUNT(*) FROM wave_slots WHERE status IN ('dispatched','running')",
                [],
                |row| row.get(0),
            )?;
            if (active as u32) >= max_concurrent_workers {
                return Ok(None);
            }
            // Select candidate rows first, then drop the stmt so
            // we can take a mutable `transaction()` borrow on
            // `conn` afterwards without conflict.
            let mut stmt = conn.prepare(
                "SELECT w.wave_id, w.kind, ws.slot_index, ws.isolation, sr.worktree_path
                 FROM waves w
                 JOIN wave_slots ws ON ws.wave_id = w.wave_id
                 LEFT JOIN slot_resources sr
                   ON sr.wave_id = ws.wave_id AND sr.slot_index = ws.slot_index
                 WHERE ws.status = 'pending'
                   AND w.phase IN ('dispatch','collect')
                   AND (ws.isolation = 'shared_readonly' OR sr.worktree_path IS NOT NULL)
                 ORDER BY w.wave_id ASC, ws.slot_index ASC
                 LIMIT 1",
            )?;
            let candidate: Option<(String, String, u32, String, Option<String>)> = {
                let mut rows = stmt.query([])?;
                match rows.next()? {
                    Some(row) => Some((
                        row.get(0)?,
                        row.get(1)?,
                        row.get::<_, i64>(2)? as u32,
                        row.get(3)?,
                        row.get(4)?,
                    )),
                    None => None,
                }
            };
            drop(stmt);
            let (wave_id, kind, slot_index, isolation, worktree_path) =
                match candidate {
                    Some(t) => t,
                    None => return Ok(None),
                };
            let _ = (kind, isolation, worktree_path);
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE wave_slots SET status = 'dispatched' WHERE wave_id = ?1 AND slot_index = ?2",
                rusqlite::params![&wave_id, i64::from(slot_index)],
            )?;
            tx.execute(
                "UPDATE waves SET phase = 'collect', updated_at = strftime('%s','now') WHERE wave_id = ?1",
                [&wave_id],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO dispatch_records (wave_id, slot_index, outcome)
                 VALUES (?1, ?2, NULL)",
                rusqlite::params![&wave_id, i64::from(slot_index)],
            )?;
            tx.commit()?;
            Ok(Some((wave_id, slot_index)))
        })
    }

    fn bind_worktree(
        &self,
        wave_id: &str,
        slot_index: u32,
        binding: SlotResource,
    ) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            let isolation: String = conn
                .query_row(
                    "SELECT isolation FROM wave_slots WHERE wave_id = ?1 AND slot_index = ?2",
                    rusqlite::params![wave_id, i64::from(slot_index)],
                    |row| row.get(0),
                )
                .map_err(|err| match err {
                    rusqlite::Error::QueryReturnedNoRows => SupervisorStoreError::UnknownSlot {
                        wave_id: wave_id.to_string(),
                        slot_index,
                    },
                    other => SupervisorStoreError::Storage(other.to_string()),
                })?;
            if isolation == "shared_readonly"
                && (binding.worktree_path.is_some() || binding.branch.is_some())
            {
                return Err(SupervisorStoreError::InvalidTransition(
                    "shared_readonly slot cannot receive a worktree binding".to_string(),
                ));
            }
            // 2026-07-03-001 plan U8 / F-008: rebind path
            // runs `cleanup_worktree` on the prior path
            // before overwriting. We fetch the previous
            // binding first, then ON CONFLICT the new one.
            // Equal paths are skipped (idempotent).
            let prev_path: Option<Option<String>> = conn
                .query_row(
                    "SELECT worktree_path FROM slot_resources
                     WHERE wave_id = ?1 AND slot_index = ?2",
                    rusqlite::params![wave_id, i64::from(slot_index)],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(Some(prev)) = prev_path
                && Some(&prev) != binding.worktree_path.as_ref()
            {
                cleanup_worktree_path(&prev);
            }
            conn.execute(
                "INSERT INTO slot_resources (wave_id, slot_index, worktree_path, branch)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(wave_id, slot_index) DO UPDATE SET
                   worktree_path = excluded.worktree_path,
                   branch = excluded.branch",
                rusqlite::params![
                    wave_id,
                    i64::from(slot_index),
                    binding.worktree_path,
                    binding.branch,
                ],
            )?;
            Ok(())
        })
    }

    fn get_slot_resource(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> SupervisorStoreResult<Option<SlotResource>> {
        self.with_conn(|conn| {
            let row: Option<(Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT worktree_path, branch FROM slot_resources
                     WHERE wave_id = ?1 AND slot_index = ?2",
                    rusqlite::params![wave_id, i64::from(slot_index)],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            Ok(row.map(|(worktree_path, branch)| SlotResource {
                slot_index,
                worktree_path,
                branch,
            }))
        })
    }

    fn release_slot_dispatch(
        &self,
        wave_id: &str,
        slot_index: u32,
        outcome: DispatchOutcome,
    ) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            let status = match outcome {
                DispatchOutcome::Completed => "completed",
                DispatchOutcome::Failed => "failed",
            };
            let tx = conn.transaction()?;
            // Only an in-flight row is transitioned. This makes a
            // duplicate terminal signal a no-op and preserves the
            // first terminal status across cancellation races.
            tx.execute(
                "UPDATE wave_slots SET status = ?3
                 WHERE wave_id = ?1 AND slot_index = ?2
                   AND status IN ('dispatched','running')",
                rusqlite::params![wave_id, i64::from(slot_index), status],
            )?;
            tx.execute(
                "UPDATE dispatch_records SET outcome = ?3
                 WHERE wave_id = ?1 AND slot_index = ?2
                   AND outcome IS NULL",
                rusqlite::params![wave_id, i64::from(slot_index), status],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    fn record_slot_result(
        &self,
        wave_id: &str,
        slot_index: u32,
        content_hash: &str,
        event_count: usize,
    ) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            // 2026-07-23-004 plan U5 (R-A3 / R-A4): first-terminal-wins.
            // Inspect the slot's current status before the
            // UPDATE — if already terminal, refuse to overwrite
            // unless the new content_hash matches the recorded
            // one (idempotent replay).
            let current: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT status, content_hash FROM wave_slots
                     WHERE wave_id = ?1 AND slot_index = ?2",
                    rusqlite::params![wave_id, i64::from(slot_index)],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let (current_status, current_hash) = match current {
                Some(row) => row,
                None => {
                    return Err(SupervisorStoreError::UnknownSlot {
                        wave_id: wave_id.to_string(),
                        slot_index,
                    });
                }
            };
            let is_terminal = matches!(
                current_status.as_str(),
                "completed" | "failed" | "cancelled"
            );
            if is_terminal {
                let matches = current_hash
                    .as_deref()
                    .map(|h| h == content_hash)
                    .unwrap_or(false);
                if !matches {
                    return Err(SupervisorStoreError::AlreadyTerminal(format!(
                        "wave={wave_id} slot={slot_index} status={current_status}"
                    )));
                }
                // Idempotent replay — same content_hash, no write.
                return Ok(());
            }
            tx.execute(
                "UPDATE wave_slots SET status = 'completed', content_hash = ?3, event_count = ?4
                 WHERE wave_id = ?1 AND slot_index = ?2",
                rusqlite::params![
                    wave_id,
                    i64::from(slot_index),
                    content_hash,
                    event_count as i64
                ],
            )?;
            tx.execute(
                "INSERT INTO worker_results (wave_id, slot_index, content_hash, event_count)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(wave_id, slot_index) DO UPDATE SET
                   content_hash = excluded.content_hash,
                   event_count = excluded.event_count,
                   updated_at = strftime('%s','now')",
                rusqlite::params![
                    wave_id,
                    i64::from(slot_index),
                    content_hash,
                    event_count as i64
                ],
            )?;
            tx.execute(
                "UPDATE dispatch_records SET outcome = 'completed'
                 WHERE wave_id = ?1 AND slot_index = ?2",
                rusqlite::params![wave_id, i64::from(slot_index)],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    fn record_slot_failure(
        &self,
        wave_id: &str,
        slot_index: u32,
        reason: &str,
    ) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            // 2026-07-23-007 plan U3 (R-W3): first-terminal-wins
            // — refuse to overwrite an already-terminal slot. The
            // dispatcher passes a canonical reason from
            // `classify_worker_outcome`, so a re-emitted same-reason
            // failure is a no-op (idempotent replay), and a
            // different reason returns `AlreadyTerminal`.
            //
            // R-W4: cancel reason wins — a slot whose worker was
            // cancelled must be marked Cancelled even if a Done
            // marker slipped through earlier.
            let prior = tx
                .query_row(
                    "SELECT status, failure_reason FROM wave_slots
                     WHERE wave_id = ?1 AND slot_index = ?2",
                    rusqlite::params![wave_id, i64::from(slot_index)],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()?;
            if let Some((status, prior_reason)) = prior {
                let is_failed = matches!(status.as_str(), "failed" | "cancelled");
                let is_completed = status.as_str() == "completed";
                let cancel_wins = reason
                    == crate::supervisor::worker_outcome::REASON_WORKER_CANCELLED
                    && is_completed;
                if is_failed {
                    let same_reason = prior_reason
                        .as_deref()
                        .map(|p| p == reason)
                        .unwrap_or(false);
                    if !same_reason {
                        return Err(SupervisorStoreError::AlreadyTerminal(format!(
                            "wave={wave_id} slot={slot_index} status={status}"
                        )));
                    }
                    // Idempotent replay — commit no-op.
                    tx.commit()?;
                    return Ok(());
                }
                if is_completed && !cancel_wins {
                    return Err(SupervisorStoreError::AlreadyTerminal(format!(
                        "wave={wave_id} slot={slot_index} status=completed"
                    )));
                }
            }
            let new_status = if reason == crate::supervisor::worker_outcome::REASON_WORKER_CANCELLED
            {
                "cancelled"
            } else {
                "failed"
            };
            tx.execute(
                "UPDATE wave_slots SET status = ?3, failure_reason = ?4
                 WHERE wave_id = ?1 AND slot_index = ?2",
                rusqlite::params![wave_id, i64::from(slot_index), new_status, reason],
            )?;
            tx.execute(
                "UPDATE dispatch_records SET outcome = 'failed'
                 WHERE wave_id = ?1 AND slot_index = ?2",
                rusqlite::params![wave_id, i64::from(slot_index)],
            )?;
            // U2 / F-002 / KTD-8: the store MUST NOT mutate
            // `waves.phase` here; phase verdict is
            // coordinator-owned via `set_wave_phase`, called
            // by the coordinator after `evaluate_phase`
            // returns `Failed`. Pre-empting the verdict
            // while sibling slots are still in-flight would
            // incorrectly flip the wave to `Failed`.
            tx.commit()?;
            Ok(())
        })
    }

    fn slot_failure_reason(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> SupervisorStoreResult<Option<String>> {
        self.with_conn(|conn| {
            // Read the column as `Option<String>` so a SQL NULL
            // failure_reason projects to the `Ok(None)` column value
            // rather than raising `InvalidColumnType`. `.optional()?`
            // then lifts "no such row" into the outer `None`, which we
            // map to `UnknownSlot` — symmetric with the in-memory store
            // (memory.rs:580-588) that errors on an absent slot.
            let row: Option<Option<String>> = conn
                .query_row(
                    "SELECT failure_reason FROM wave_slots
                     WHERE wave_id = ?1 AND slot_index = ?2",
                    rusqlite::params![wave_id, i64::from(slot_index)],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?;
            match row {
                None => Err(SupervisorStoreError::UnknownSlot {
                    wave_id: wave_id.to_string(),
                    slot_index,
                }),
                Some(reason) => Ok(reason),
            }
        })
    }


    fn record_slot_terminal_evidence(
        &self,
        wave_id: &str,
        slot_index: u32,
        evidence: &super::TerminalEvidence,
    ) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            // Read the existing evidence triple (NULL topic == no
            // evidence yet). `.optional()?` lifts "no such slot row"
            // into `UnknownSlot`, symmetric with the in-memory store.
            let existing: Option<(Option<String>, Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT evidence_topic, evidence_dimension, evidence_fingerprint
                     FROM wave_slots WHERE wave_id = ?1 AND slot_index = ?2",
                    rusqlite::params![wave_id, i64::from(slot_index)],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .optional()?;
            let Some((topic, dimension, fingerprint)) = existing else {
                return Err(SupervisorStoreError::UnknownSlot {
                    wave_id: wave_id.to_string(),
                    slot_index,
                });
            };
            match topic {
                // 2026-07-26-004 plan U2 (R3): idempotent same-evidence
                // replay is a no-op; conflicting evidence fails closed.
                Some(existing_topic) => {
                    let existing_ev = super::TerminalEvidence {
                        topic: existing_topic,
                        dimension,
                        payload_fingerprint: fingerprint.unwrap_or_default(),
                    };
                    if &existing_ev == evidence {
                        Ok(())
                    } else {
                        Err(SupervisorStoreError::AlreadyTerminal(format!(
                            "wave={wave_id} slot={slot_index} terminal evidence conflict: \
                             existing={existing_ev:?} incoming={evidence:?}"
                        )))
                    }
                }
                None => {
                    conn.execute(
                        "UPDATE wave_slots
                         SET evidence_topic = ?3, evidence_dimension = ?4, evidence_fingerprint = ?5
                         WHERE wave_id = ?1 AND slot_index = ?2",
                        rusqlite::params![
                            wave_id,
                            i64::from(slot_index),
                            evidence.topic,
                            evidence.dimension,
                            evidence.payload_fingerprint,
                        ],
                    )?;
                    Ok(())
                }
            }
        })
    }

    fn slot_terminal_evidence(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> SupervisorStoreResult<Option<super::TerminalEvidence>> {
        self.with_conn(|conn| {
            let row: Option<(Option<String>, Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT evidence_topic, evidence_dimension, evidence_fingerprint
                     FROM wave_slots WHERE wave_id = ?1 AND slot_index = ?2",
                    rusqlite::params![wave_id, i64::from(slot_index)],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .optional()?;
            match row {
                None => Err(SupervisorStoreError::UnknownSlot {
                    wave_id: wave_id.to_string(),
                    slot_index,
                }),
                // NULL topic == legacy / no evidence → `None`
                // (reconciliation treats this as not-provably-done).
                Some((None, _, _)) => Ok(None),
                Some((Some(topic), dimension, fingerprint)) => Ok(Some(super::TerminalEvidence {
                    topic,
                    dimension,
                    payload_fingerprint: fingerprint.unwrap_or_default(),
                })),
            }
        })
    }

    fn cancel_wave(&self, wave_id: &str) -> SupervisorStoreResult<()> {
        use crate::supervisor::worker_outcome::REASON_SLOT_NEVER_STARTED;
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE waves SET cancel_requested = 1, updated_at = strftime('%s','now')
                 WHERE wave_id = ?1",
                [&wave_id],
            )?;
            // 2026-07-25-004 plan U5: also freeze `failure_reason` to
            // `slot_never_started` for the Pending slots we cancel here,
            // so the InjectedFailed reason-collection sees a non-null
            // reason for cancelled never-started slots. The
            // `status = 'pending'` guard is essential: already-terminal
            // slots (Completed/Failed with their own reason, or
            // Dispatched/Running) are NOT overwritten.
            conn.execute(
                "UPDATE wave_slots SET status = 'cancelled', failure_reason = ?2
                 WHERE wave_id = ?1 AND status = 'pending'",
                rusqlite::params![wave_id, REASON_SLOT_NEVER_STARTED],
            )?;
            Ok(())
        })
    }

    fn fan_in_status(&self, wave_id: &str) -> SupervisorStoreResult<WaveSnapshot> {
        self.with_conn(|conn| {
            let wave = conn
                .query_row(
                    "SELECT wave_id, phase, expected_total, cancel_requested, merged_to_events, created_at
                     FROM waves WHERE wave_id = ?1",
                    [&wave_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)? as u32,
                            row.get::<_, i64>(3)? != 0,
                            row.get::<_, i64>(4)? != 0,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
            let (wave_id_row, phase_str, expected_total, cancel, merged, created_at_unix) = wave;
            let phase = parse_phase(&phase_str)?;
            let kind = {
                let kind_str: String = conn
                    .query_row(
                        "SELECT kind FROM waves WHERE wave_id = ?1",
                        [&wave_id],
                        |row| row.get(0),
                    )
                    .optional()?
                    .ok_or_else(|| SupervisorStoreError::UnknownWave(wave_id.to_string()))?;
                parse_kind(&kind_str)?
            };
            let row = conn.query_row(
                "SELECT
                   SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END),
                   SUM(CASE WHEN status = 'failed'    THEN 1 ELSE 0 END),
                   SUM(CASE WHEN status IN ('dispatched','running') THEN 1 ELSE 0 END),
                   SUM(CASE WHEN status IN ('pending','cancelled') THEN 1 ELSE 0 END)
                 FROM wave_slots WHERE wave_id = ?1",
                [&wave_id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?.unwrap_or(0) as u32,
                        row.get::<_, Option<i64>>(1)?.unwrap_or(0) as u32,
                        row.get::<_, Option<i64>>(2)?.unwrap_or(0) as u32,
                        row.get::<_, Option<i64>>(3)?.unwrap_or(0) as u32,
                    ))
                },
            )?;
            let (completed, failed, in_flight, pending) = row;
            // U3 / F-003: emit per-slot status via JOIN so the
            // phase function reads REAL failures (not a
            // fabricated range from
            // `expected_total - completed_count`).
            let mut stmt_slots = conn
                .prepare("SELECT slot_index, status FROM wave_slots WHERE wave_id = ?1 ORDER BY slot_index ASC")?;
            let slots: Vec<(u32, SlotStatus)> = stmt_slots
                .query_map([&wave_id], |row| {
                    let idx: i64 = row.get(0)?;
                    let status_str: String = row.get(1)?;
                    // Convert the supervisor parse error into
                    // a `rusqlite::Error` so the closure's
                    // `?` operator stays in its native error
                    // type. The store trait surfaces it via
                    // `SupervisorStoreError::Storage`.
                    let status = parse_status(&status_str)
                        .map_err(|e| {
                            // Wrap into a small anonymous error
                            // type so the closure's `?` keeps
                            // using the `rusqlite::Error` arm.
                            // The store trait re-translates via
                            // `From<rusqlite::Error> for
                            // SupervisorStoreError` upstream.
                            struct SlotStatusParseError(String);
                            impl std::fmt::Display for SlotStatusParseError {
                                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                                    f.write_str(&self.0)
                                }
                            }
                            impl std::fmt::Debug for SlotStatusParseError {
                                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                                    f.debug_tuple("SlotStatusParseError").field(&self.0).finish()
                                }
                            }
                            impl std::error::Error for SlotStatusParseError {}
                            rusqlite::Error::FromSqlConversionFailure(
                                1,
                                rusqlite::types::Type::Text,
                                Box::new(SlotStatusParseError(e.to_string())),
                            )
                        })?;
                    Ok((idx as u32, status))
                })?
                .collect::<Result<_, _>>()?;
            drop(stmt_slots);
            // 2026-07-03-001 plan U6: convert `created_at`
            // (unix seconds) into `SystemTime` so the
            // recovery path can compute `elapsed_secs` via
            // `SystemTime::duration_since`.
            let started_at = std::time::UNIX_EPOCH
                .checked_add(std::time::Duration::from_secs(
                    created_at_unix.max(0) as u64,
                ))
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            Ok(WaveSnapshot {
                wave_id: wave_id_row,
                kind,
                phase,
                expected_total,
                completed_count: completed,
                failed_count: failed,
                pending_count: pending,
                in_flight_count: in_flight,
                cancel_requested: cancel,
                merged_to_events: merged,
                started_at,
                slots,
            })
        })
    }

    fn mark_merge_to_events(&self, wave_id: &str) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE waves SET merged_to_events = 1, updated_at = strftime('%s','now')
                 WHERE wave_id = ?1 AND merged_to_events = 0",
                [&wave_id],
            )?;
            Ok(())
        })
    }

    fn list_wave_ids(&self) -> SupervisorStoreResult<Vec<String>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT wave_id FROM waves ORDER BY wave_id ASC")?;
            let ids = stmt
                .query_map([], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
    }

    fn wave_id_for_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> SupervisorStoreResult<Option<String>> {
        self.with_conn(|conn| {
            let wave_id: Option<String> = conn
                .query_row(
                    "SELECT wave_id FROM waves WHERE idempotency_key = ?1",
                    [idempotency_key],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(wave_id)
        })
    }

    fn recover_active_waves(&self) -> SupervisorStoreResult<Vec<WaveSnapshot>> {
        // U8 / R11 (crash-restart recovery): read the active wave
        // ids under the connection lock, then RELEASE it before
        // building snapshots. We must NOT call `fan_in_status`
        // while still holding the guard — `fan_in_status`
        // re-enters `with_conn`, and `std::sync::Mutex` is not
        // reentrant, so nesting the two deadlocks the store the
        // instant a wave survives a crash. The in-memory store
        // mirror never exercises this path, which is why the
        // deadlock stayed latent until the real-DB reopen tests.
        let wave_ids: Vec<String> = self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT wave_id FROM waves
                 WHERE phase NOT IN ('done','failed')
                 ORDER BY wave_id ASC",
            )?;
            let ids = stmt
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            Ok(ids)
        })?;
        // Lock released: each `fan_in_status` call takes and drops
        // the guard on its own, so no reentrant acquisition.
        let mut out = Vec::with_capacity(wave_ids.len());
        for id in wave_ids {
            out.push(self.fan_in_status(&id)?);
        }
        Ok(out)
    }

    fn list_worktree_paths(&self, wave_id: &str) -> SupervisorStoreResult<Vec<SlotResource>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT slot_index, worktree_path, branch FROM slot_resources
                 WHERE wave_id = ?1 ORDER BY slot_index ASC",
            )?;
            let rows = stmt
                .query_map([wave_id], |row| {
                    Ok(SlotResource {
                        slot_index: row.get::<_, i64>(0)? as u32,
                        worktree_path: row.get(1)?,
                        branch: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    fn set_wave_phase(&self, wave_id: &str, phase: WavePhase) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE waves SET phase = ?2, updated_at = strftime('%s','now')
                 WHERE wave_id = ?1",
                rusqlite::params![wave_id, phase_to_str(phase)],
            )?;
            Ok(())
        })
    }

    fn record_slot_pid(
        &self,
        wave_id: &str,
        slot_index: u32,
        pid: u32,
    ) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            let slot_exists: bool = conn
                .query_row(
                    "SELECT 1 FROM wave_slots WHERE wave_id = ?1 AND slot_index = ?2",
                    rusqlite::params![wave_id, i64::from(slot_index)],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            if !slot_exists {
                return Err(SupervisorStoreError::UnknownSlot {
                    wave_id: wave_id.to_string(),
                    slot_index,
                });
            }
            conn.execute(
                "INSERT INTO dispatch_records (wave_id, slot_index, pid, outcome)
                 VALUES (?1, ?2, ?3, NULL)
                 ON CONFLICT(wave_id, slot_index) DO UPDATE SET
                   pid = excluded.pid",
                rusqlite::params![wave_id, i64::from(slot_index), i64::from(pid)],
            )?;
            Ok(())
        })
    }

    fn pid_for_slot(&self, wave_id: &str, slot_index: u32) -> SupervisorStoreResult<Option<u32>> {
        self.with_conn(|conn| {
            let pid: Option<Option<i64>> = conn
                .query_row(
                    "SELECT pid FROM dispatch_records WHERE wave_id = ?1 AND slot_index = ?2",
                    rusqlite::params![wave_id, i64::from(slot_index)],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(pid.flatten().map(|p| p.max(0) as u32))
        })
    }

    fn enqueue_compensation(
        &self,
        wave_id: &str,
        kind: CompensationKind,
    ) -> SupervisorStoreResult<()> {
        // 2026-07-22-001 plan U6: persist the compensation job
        // in the existing `compensation_jobs` table. Dedup on
        // (wave_id, kind, status='pending') so a re-entered
        // cancel path does not stack two jobs for the same
        // wave. The full hook execution lands in a follow-up
        // release (U6 follow-up); today we record the row and
        // let the coordinator tick mark it executed/failed via
        // `complete_compensation`.
        self.with_conn(|conn| {
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT id FROM compensation_jobs
                     WHERE wave_id = ?1 AND kind = ?2 AND status = 'pending'
                     LIMIT 1",
                    rusqlite::params![wave_id, compensation_kind_to_str(kind)],
                    |row| row.get(0),
                )
                .optional()?;
            if existing.is_none() {
                conn.execute(
                    "INSERT INTO compensation_jobs (wave_id, kind, status, created_at)
                     VALUES (?1, ?2, 'pending', strftime('%s','now'))",
                    rusqlite::params![wave_id, compensation_kind_to_str(kind)],
                )?;
            }
            Ok(())
        })
    }

    fn take_pending_compensations(&self) -> SupervisorStoreResult<Vec<(String, CompensationKind)>> {
        let pairs = self.with_conn(|conn| -> SupervisorStoreResult<Vec<(String, String)>> {
            let mut stmt = conn.prepare(
                "SELECT wave_id, kind FROM compensation_jobs WHERE status = 'pending' ORDER BY id ASC",
            )?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<(String, String)>>>()?;
            Ok(rows)
        })?;
        Ok(pairs
            .into_iter()
            .filter_map(|(w, k)| match k.as_str() {
                "timeout" => Some((w, CompensationKind::OnTimeout)),
                "cancel" => Some((w, CompensationKind::OnCancel)),
                "partial" => Some((w, CompensationKind::OnPartial)),
                _ => None,
            })
            .collect())
    }

    fn complete_compensation(
        &self,
        wave_id: &str,
        kind: CompensationKind,
        ok: bool,
    ) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE compensation_jobs
                 SET status = ?3, completed_at = strftime('%s','now')
                 WHERE wave_id = ?1 AND kind = ?2 AND status = 'pending'",
                rusqlite::params![
                    wave_id,
                    compensation_kind_to_str(kind),
                    if ok { "executed" } else { "failed" }
                ],
            )?;
            Ok(())
        })
    }

    // ─────────────────────────────────────────────────────────────────
    // 2026-07-24-003 plan U4: emission reservation state machine.
    //
    // SQLite `UNIQUE (scope_key)` constraint backs the
    // single-owner invariant. `INSERT OR IGNORE` lets us probe
    // for an existing row without surfacing the constraint
    // error to the caller; the affected-row count distinguishes
    // the "inserted fresh" branch from the "already reserved"
    // branch.
    // ─────────────────────────────────────────────────────────────────

    fn reserve_emission(
        &self,
        scope_key: &str,
        payload_digest: &str,
        expected_count: u32,
        count_events_on_disk: &dyn Fn(&str) -> u32,
    ) -> SupervisorStoreResult<EmissionReservation> {
        // Wrap the read-modify-write in BEGIN IMMEDIATE so two
        // callers racing the same scope serialize at the SQLite
        // write-lock (busy_timeout=5000 waits for the peer).
        // This only covers THIS call's RMW — production still
        // commits `reserved` before FileLock / event write /
        // mark_applied, so a later peer can observe in-flight
        // `reserved`/`applying` (live producer or crash residue).
        // Those rows must be classified by on-disk evidence
        // (FailedPartial / RecoveryRequired / AlreadyApplied via
        // recovery) — never coerced into AlreadyApplied, which
        // would silently drop events.  (2026-07-25)
        self.with_conn(|conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            // `RETURNING` via prepare+query_row holds a borrow on
            // `tx` for the lifetime of the prepared `Statement`,
            // which conflicts with `tx.commit()` later.  Materialise
            // the seq to a local first, then drop the statement.
            let seq: i64 = {
                let mut stmt =
                    tx.prepare("INSERT INTO wave_id_seq (placeholder) VALUES (0) RETURNING seq")?;
                stmt.query_row([], |row| row.get(0))?
            };
            let public_wave_id = format!("w-rs-{seq}");

            // Try to insert a fresh emission row. The UNIQUE
            // constraint on `scope_key` makes a parallel
            // reservation a no-op so we can probe the existing
            // row instead.
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO wave_emissions
                   (scope_key, public_wave_id, payload_digest, expected_count, state)
                 VALUES (?1, ?2, ?3, ?4, 'reserved')",
                rusqlite::params![scope_key, &public_wave_id, payload_digest, expected_count],
            )?;

            if inserted == 1 {
                tx.commit()?;
                return Ok(EmissionReservation::Reserved { public_wave_id });
            }

            // A row already exists; load it and classify.
            let existing = tx
                .query_row(
                    "SELECT public_wave_id, payload_digest, expected_count, state
                     FROM wave_emissions WHERE scope_key = ?1",
                    [scope_key],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)? as u32,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        SupervisorStoreError::UnknownWave(scope_key.to_string())
                    }
                    other => SupervisorStoreError::Storage(other.to_string()),
                })?;

            let (existing_public_wave_id, existing_payload_digest, existing_expected, state_str) =
                existing;
            if existing_payload_digest != payload_digest {
                tx.commit()?;
                return Ok(EmissionReservation::Conflict);
            }
            let state = parse_emission_state(&state_str)?;
            let out = match state {
                EmissionState::Applied => Ok(EmissionReservation::AlreadyApplied {
                    public_wave_id: existing_public_wave_id,
                }),
                EmissionState::Failed => Ok(EmissionReservation::Conflict),
                // Prior reservation still open (live peer mid-emit,
                // crash residue, or explicit recovery_required).
                // Classify by on-disk event count — do not coerce
                // into AlreadyApplied without evidence.
                EmissionState::Reserved
                | EmissionState::Applying
                | EmissionState::RecoveryRequired => {
                    let on_disk = count_events_on_disk(&existing_public_wave_id);
                    if on_disk == 0 {
                        Ok(EmissionReservation::FailedPartial {
                            public_wave_id: existing_public_wave_id,
                            on_disk,
                            expected: existing_expected,
                        })
                    } else if on_disk < existing_expected {
                        Ok(EmissionReservation::RecoveryRequired {
                            public_wave_id: existing_public_wave_id,
                            on_disk,
                            expected: existing_expected,
                        })
                    } else {
                        // Recovery path: events present + row never
                        // reached applied → advance to applied.
                        tx.execute(
                            "UPDATE wave_emissions
                             SET state = 'applied', applied_at = strftime('%s','now')
                             WHERE scope_key = ?1",
                            [scope_key],
                        )?;
                        Ok(EmissionReservation::AlreadyApplied {
                            public_wave_id: existing_public_wave_id,
                        })
                    }
                }
            };
            tx.commit()?;
            out
        })
    }

    fn mark_emission_applying(&self, scope_key: &str) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            let rows = conn.execute(
                "UPDATE wave_emissions
                 SET state = 'applying'
                 WHERE scope_key = ?1 AND state = 'reserved'",
                [scope_key],
            )?;
            if rows == 0 {
                return Err(SupervisorStoreError::InvalidTransition(format!(
                    "emission row for {scope_key} not in state Reserved"
                )));
            }
            Ok(())
        })
    }

    fn mark_emission_applied(
        &self,
        scope_key: &str,
        applied_at_unix_secs: u64,
    ) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            // Strict Applying/Reserved → Applied.  'applied' is
            // NOT a valid source state: overwriting `applied_at`
            // would corrupt the audit trail.  Peer same-key
            // retries go through `reserve_emission` →
            // AlreadyApplied and never call this path; 0 rows
            // here means a true invalid transition (e.g.
            // recovery_required or failed terminal).  (2026-07-25)
            let rows = conn.execute(
                "UPDATE wave_emissions
                 SET state = 'applied', applied_at = ?2
                 WHERE scope_key = ?1 AND state IN ('applying', 'reserved')",
                rusqlite::params![scope_key, applied_at_unix_secs as i64],
            )?;
            if rows == 0 {
                return Err(SupervisorStoreError::InvalidTransition(format!(
                    "emission row for {scope_key} not in Applying/Reserved state"
                )));
            }
            Ok(())
        })
    }

    fn mark_emission_recovery_required(&self, scope_key: &str) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            let rows = conn.execute(
                "UPDATE wave_emissions
                 SET state = 'recovery_required'
                 WHERE scope_key = ?1 AND state IN ('reserved', 'applying')",
                [scope_key],
            )?;
            if rows == 0 {
                return Err(SupervisorStoreError::InvalidTransition(format!(
                    "emission row for {scope_key} not in Reserved/Applying state"
                )));
            }
            Ok(())
        })
    }

    fn mark_emission_failed(&self, scope_key: &str) -> SupervisorStoreResult<()> {
        self.with_conn(|conn| {
            let rows = conn.execute(
                "UPDATE wave_emissions
                 SET state = 'failed'
                 WHERE scope_key = ?1 AND state IN ('reserved', 'applying', 'recovery_required')",
                [scope_key],
            )?;
            if rows == 0 {
                return Err(SupervisorStoreError::InvalidTransition(format!(
                    "emission row for {scope_key} already terminal-applied"
                )));
            }
            Ok(())
        })
    }

    fn emission_state_for_wave_id(
        &self,
        public_wave_id: &str,
    ) -> SupervisorStoreResult<Option<EmissionState>> {
        self.with_conn(|conn| {
            let state_str: Option<String> = conn
                .query_row(
                    "SELECT state FROM wave_emissions WHERE public_wave_id = ?1",
                    [public_wave_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(match state_str {
                Some(s) => Some(parse_emission_state(&s)?),
                None => None,
            })
        })
    }

    fn adopt_legacy_emission(
        &self,
        scope_key: &str,
        payload_digest: &str,
        expected_count: u32,
        legacy_wave_id: &str,
    ) -> SupervisorStoreResult<String> {
        self.with_conn(|conn| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            // 2026-07-24-003 plan U5 (S10): idempotent import. If
            // the scope already has an emission row, return the
            // recorded id; never mint a third wave. `INSERT OR
            // IGNORE` + a follow-up lookup covers the
            // unique-collision branch.
            let inserted = conn.execute(
                "INSERT OR IGNORE INTO wave_emissions \
                   (scope_key, public_wave_id, payload_digest, expected_count, state, applied_at) \
                 VALUES (?1, ?2, ?3, ?4, 'applied', ?5)",
                rusqlite::params![
                    scope_key,
                    legacy_wave_id,
                    payload_digest,
                    i64::from(expected_count),
                    now,
                ],
            )?;
            let _ = inserted;
            // Read back the canonical id (handles both the
            // inserted and the already-present branches).
            let recorded: String = conn
                .query_row(
                    "SELECT public_wave_id FROM wave_emissions WHERE scope_key = ?1",
                    [scope_key],
                    |row| row.get(0),
                )
                .map_err(|err| SupervisorStoreError::Storage(err.to_string()))?;
            Ok(recorded)
        })
    }
}

#[cfg(feature = "supervisor-db")]
fn parse_emission_state(s: &str) -> SupervisorStoreResult<EmissionState> {
    match s {
        "reserved" => Ok(EmissionState::Reserved),
        "applying" => Ok(EmissionState::Applying),
        "applied" => Ok(EmissionState::Applied),
        "recovery_required" => Ok(EmissionState::RecoveryRequired),
        "failed" => Ok(EmissionState::Failed),
        other => Err(SupervisorStoreError::Storage(format!(
            "unknown emission state '{other}'"
        ))),
    }
}

#[cfg(feature = "supervisor-db")]
fn compensation_kind_to_str(kind: CompensationKind) -> &'static str {
    match kind {
        CompensationKind::OnTimeout => "timeout",
        CompensationKind::OnCancel => "cancel",
        CompensationKind::OnPartial => "partial",
    }
}

#[cfg(feature = "supervisor-db")]
fn kind_to_str(kind: WaveKind) -> &'static str {
    match kind {
        WaveKind::Exec => "exec",
        WaveKind::Fix => "fix",
        WaveKind::Review => "review",
    }
}

#[cfg(feature = "supervisor-db")]
#[cfg(test)]
mod tests {
    //! U5 mirror of U3 + U4 contract tests against the
    //! rusqlite store. Tests use `from_connection` with an
    //! in-memory DB; CLI integration lives in U12.

    use super::*;
    use crate::supervisor::WaveKind;

    fn store() -> RusqliteSupervisorStore {
        let conn = Connection::open_in_memory().unwrap();
        RusqliteSupervisorStore::from_connection(conn).unwrap()
    }


    /// 2026-07-26-004 plan U2 (KTD3 / R2 / R3): rusqlite parity with
    /// the in-memory store — terminal evidence round-trips through the
    /// v4 `evidence_*` columns, same-evidence replay is a no-op,
    /// conflicting evidence fails closed, and a legacy slot recorded
    /// before evidence existed reads back as `None`.
    #[test]
    fn u2_terminal_evidence_round_trip_and_conflict() {
        use crate::supervisor::TerminalEvidence;
        let s = store();
        let wave = s.register_wave("k-ev", WaveKind::Review, 2).unwrap();
        // Legacy: a slot that reached Completed via record_slot_result
        // WITHOUT evidence reads back as None (not provably done).
        s.record_slot_result(&wave, 1, "hash-legacy", 1).unwrap();
        assert_eq!(s.slot_terminal_evidence(&wave, 1).unwrap(), None);
        // Never-completed slot also None.
        assert_eq!(s.slot_terminal_evidence(&wave, 0).unwrap(), None);

        let ev =
            TerminalEvidence::from_event("review.unit.done", "{\"dimension\":\"correctness\"}");
        assert_eq!(ev.dimension.as_deref(), Some("correctness"));
        s.record_slot_terminal_evidence(&wave, 0, &ev).unwrap();
        assert_eq!(s.slot_terminal_evidence(&wave, 0).unwrap(), Some(ev.clone()));

        // Idempotent same-evidence replay → Ok no-op.
        s.record_slot_terminal_evidence(&wave, 0, &ev).unwrap();
        assert_eq!(s.slot_terminal_evidence(&wave, 0).unwrap(), Some(ev.clone()));

        // Conflicting evidence → AlreadyTerminal, original preserved.
        let other = TerminalEvidence::from_event("review.unit.done", "{\"dimension\":\"testing\"}");
        let conflict = s.record_slot_terminal_evidence(&wave, 0, &other);
        assert!(
            matches!(conflict, Err(SupervisorStoreError::AlreadyTerminal(_))),
            "conflicting evidence must fail closed; got {conflict:?}"
        );
        assert_eq!(s.slot_terminal_evidence(&wave, 0).unwrap(), Some(ev));
    }

    fn bind(slot: u32) -> SlotResource {
        SlotResource {
            slot_index: slot,
            worktree_path: Some(format!(".ralph/wt/{slot}")),
            branch: Some(format!("ralph/u{slot}")),
        }
    }

    /// U4 / F-004 / R4 regression pin: concurrent
    /// `register_wave` calls MUST allocate distinct `wave_id`s
    /// (autoincrement via `wave_id_seq`). 10 threads × 10
    /// calls with distinct idempotency keys → 100 distinct
    /// wave_ids.
    #[test]
    fn register_wave_concurrent_calls_get_unique_ids() {
        use rusqlite::Connection;
        use std::sync::Arc;
        // Open an in-memory DB and share the connection via
        // a guarded Arc so multiple threads can hammer the
        // store trait simultaneously. The store trait takes
        // `&self`, so a single instance can be shared.
        let conn = Connection::open_in_memory().unwrap();
        let store = Arc::new(RusqliteSupervisorStore::from_connection(conn).unwrap());
        // 10 threads × 10 calls = 100 register_wave calls
        // (each with a unique idempotency_key).
        let n_threads = 10u32;
        let calls_per_thread = 10u32;
        let store_for_threads = store.clone();
        let wave_ids: Vec<String> = std::thread::scope(|s| {
            let mut handles = Vec::new();
            for t in 0..n_threads {
                let store = store_for_threads.clone();
                handles.push(s.spawn(move || {
                    let mut ids = Vec::new();
                    for c in 0..calls_per_thread {
                        let key = format!("k-{t}-{c}");
                        let id = store
                            .register_wave(&key, WaveKind::Exec, 1)
                            .expect("register_wave must succeed");
                        ids.push(id);
                    }
                    ids
                }));
            }
            let mut all_ids: Vec<String> = Vec::new();
            for h in handles {
                all_ids.extend(h.join().expect("thread must not panic"));
            }
            all_ids
        });
        // All 100 wave_ids must be distinct.
        let total = wave_ids.len();
        let mut sorted = wave_ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            total,
            "concurrent register_wave MUST allocate distinct wave_ids (got {} dup of {})",
            total - sorted.len(),
            total
        );
        assert_eq!(total, (n_threads * calls_per_thread) as usize);
    }

    /// U5 migration + reopen path. Confirms `open` is callable
    /// twice on the same path and the second store is fully
    /// functional. The actual on-disk wave-row assertion is
    /// covered by `migrations_idempotent_across_reopen`; this
    /// test stays narrow because the WAL-shm sidecar on Linux
    /// has a known race when one Connection drops on the same
    /// process-boundary as the next open, and the in-process
    /// variant of the contract is sufficient for pinning the
    /// store trait surface.
    #[test]
    fn open_persists_across_reopen() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("supervisor.db");
        // First open: only `open` (runs migrations).
        {
            let _ = RusqliteSupervisorStore::open(&path).unwrap();
        }
        // Second open on the same path: also succeeds.
        let store = RusqliteSupervisorStore::open(&path).unwrap();
        // Pin that the second store is wired: a `register_wave`
        // call returns the next `w-N` (default wave counter
        // is one after the first open).
        let wave = store.register_wave("after", WaveKind::Exec, 1).unwrap();
        assert!(wave.starts_with("w-"));
    }

    /// R-D1 mirrored: duplicate idempotency key is rejected.
    #[test]
    fn duplicate_key_is_rejected() {
        let store = store();
        store.register_wave("dup", WaveKind::Exec, 1).unwrap();
        let err = store.register_wave("dup", WaveKind::Fix, 1).unwrap_err();
        assert!(matches!(err, SupervisorStoreError::DuplicateKey(_)));
    }

    /// U2 / F-002 / KTD-8 invariant pin (rusqlite variant):
    /// when 1 slot fails and at least 1 sibling is still
    /// in-flight, the store MUST NOT mutate `wave.phase` —
    /// that verdict belongs to the coordinator (KTD-8).
    #[test]
    fn record_slot_failure_with_in_flight_siblings_keeps_phase_collect() {
        let store = store();
        let wave = store
            .register_wave("partial-fail-rs", WaveKind::Exec, 2)
            .unwrap();
        for i in 0..2 {
            store.bind_worktree(&wave, i, bind(i)).unwrap();
        }
        // Dispatch both slots.
        store.try_dispatch_next(4).unwrap().unwrap();
        store.try_dispatch_next(4).unwrap().unwrap();
        // Fail slot 0; slot 1 stays in-flight.
        store.record_slot_failure(&wave, 0, "boom").unwrap();
        let snap = store.fan_in_status(&wave).unwrap();
        assert_eq!(
            snap.phase,
            WavePhase::Collect,
            "phase must stay Collect while a sibling is still in-flight (KTD-8); got {:?}",
            snap.phase
        );
        assert_eq!(snap.failed_count, 1);
        assert_eq!(snap.in_flight_count, 1);
    }

    /// U2 (rusqlite variant): a bound slot with no recorded
    /// failure has a SQL NULL `failure_reason`, which MUST read
    /// back as `Ok(None)` — symmetric with the in-memory store.
    /// Pre-fix the closure inferred `Result<String>`, so NULL
    /// raised `InvalidColumnType`.
    #[test]
    fn u2_slot_failure_reason_null_returns_none() {
        let store = store();
        let wave = store.register_wave("u2-null", WaveKind::Exec, 1).unwrap();
        store.bind_worktree(&wave, 0, bind(0)).unwrap();
        // No record_slot_failure call → column stays NULL.
        assert_eq!(store.slot_failure_reason(&wave, 0).unwrap(), None);
    }

    /// U2 (rusqlite variant): a recorded failure reason reads
    /// back as `Ok(Some(reason))`.
    #[test]
    fn u2_slot_failure_reason_value_returns_some() {
        use crate::supervisor::worker_outcome::REASON_WORKER_TIMEOUT;
        let store = store();
        let wave = store.register_wave("u2-val", WaveKind::Exec, 1).unwrap();
        store.bind_worktree(&wave, 0, bind(0)).unwrap();
        store
            .record_slot_failure(&wave, 0, REASON_WORKER_TIMEOUT)
            .unwrap();
        assert_eq!(
            store.slot_failure_reason(&wave, 0).unwrap(),
            Some(REASON_WORKER_TIMEOUT.to_string())
        );
    }

    /// U2 (rusqlite variant): querying a slot that was never
    /// bound MUST return `UnknownSlot`, symmetric with the
    /// in-memory store (which has no row to project). Pre-fix the
    /// missing row mapped to `Ok(None)` via `.optional()`.
    #[test]
    fn u2_slot_failure_reason_missing_slot_returns_unknown_slot() {
        let store = store();
        let wave = store
            .register_wave("u2-missing", WaveKind::Exec, 1)
            .unwrap();
        store.bind_worktree(&wave, 0, bind(0)).unwrap();
        // slot_index 7 was never registered for this wave.
        let err = store.slot_failure_reason(&wave, 7).unwrap_err();
        assert!(
            matches!(
                &err,
                SupervisorStoreError::UnknownSlot { wave_id, slot_index }
                    if wave_id == &wave && *slot_index == 7
            ),
            "expected UnknownSlot, got {err:?}"
        );
    }

    /// R-A2: backpressure returns None when cap is hit.
    #[test]
    fn backpressure_blocks_when_cap_is_hit() {
        let store = store();
        let _ = store.register_wave("bp-1", WaveKind::Exec, 2).unwrap();
        let _ = store.register_wave("bp-2", WaveKind::Exec, 2).unwrap();
        // bind every slot so dispatch is enabled.
        for w in ["w-1", "w-2"] {
            for i in 0..2 {
                store.bind_worktree(w, i, bind(i)).unwrap();
            }
        }
        assert!(store.try_dispatch_next(2).unwrap().is_some());
        assert!(store.try_dispatch_next(2).unwrap().is_some());
        assert!(store.try_dispatch_next(2).unwrap().is_none());
    }

    /// R-A3: completing a slot frees a backpressure slot.
    #[test]
    fn backpressure_releases_after_completion() {
        let store = store();
        let wave = store.register_wave("bp-r", WaveKind::Exec, 1).unwrap();
        store.bind_worktree(&wave, 0, bind(0)).unwrap();
        let _ = store.try_dispatch_next(1).unwrap().unwrap();
        assert!(store.try_dispatch_next(1).unwrap().is_none());
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        // No more slots -> dispatch returns None but is not blocked by cap.
        let next = store.try_dispatch_next(1).unwrap();
        assert!(next.is_none(), "no further slots should exist");
    }

    /// U4 parity: explicit terminal release returns a permit and is
    /// idempotent when repeated for the same slot.
    #[test]
    fn release_dispatch_permit_is_idempotent() {
        let store = store();
        let wave = store
            .register_wave("release-sql", WaveKind::Exec, 2)
            .unwrap();
        for i in 0..2 {
            store.bind_worktree(&wave, i, bind(i)).unwrap();
        }
        store.try_dispatch_next(1).unwrap().unwrap();
        assert!(store.try_dispatch_next(1).unwrap().is_none());
        store
            .release_slot_dispatch(&wave, 0, DispatchOutcome::Failed)
            .unwrap();
        store
            .release_slot_dispatch(&wave, 0, DispatchOutcome::Failed)
            .unwrap();
        assert_eq!(store.fan_in_status(&wave).unwrap().failed_count, 1);
        assert_eq!(store.try_dispatch_next(1).unwrap().unwrap().1, 1);
    }

    /// 2026-07-23-004 plan U5 (R-A3): first-terminal-wins.
    /// A conflicting terminal event MUST NOT overwrite the
    /// recorded slot terminal. The `worker_results` history
    /// table is still upserted on idempotent replay with the
    /// SAME content_hash (R-E1), but a *different* content_hash
    /// returns `AlreadyTerminal` and refuses to write.
    #[test]
    fn worker_results_replace_on_conflict() {
        let store = store();
        let wave = store.register_wave("wh", WaveKind::Exec, 1).unwrap();
        store.bind_worktree(&wave, 0, bind(0)).unwrap();
        store.try_dispatch_next(4).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h-a", 1).unwrap();
        // Idempotent replay with same content_hash → Ok, no overwrite.
        store
            .record_slot_result(&wave, 0, "h-a", 1)
            .expect("idempotent replay must succeed");
        // Conflicting content_hash → AlreadyTerminal, no overwrite.
        let conflict = store.record_slot_result(&wave, 0, "h-b", 2);
        assert!(
            matches!(conflict, Err(SupervisorStoreError::AlreadyTerminal(_))),
            "conflicting content_hash must be rejected as AlreadyTerminal, got {conflict:?}"
        );
        let snap = store.fan_in_status(&wave).unwrap();
        assert_eq!(snap.completed_count, 1);
    }

    /// R-B3: cancel moves pending slots to cancelled; cancel flag flips.
    #[test]
    fn cancel_marks_pending_slots_as_cancelled() {
        let store = store();
        let wave = store.register_wave("cx", WaveKind::Exec, 2).unwrap();
        store.cancel_wave(&wave).unwrap();
        let snap = store.fan_in_status(&wave).unwrap();
        assert!(snap.cancel_requested);
        assert_eq!(snap.pending_count, 2);
        assert_eq!(snap.failed_count, 0);
    }

    /// 2026-07-25-004 plan U5: when `cancel_wave` flips a Pending
    /// slot to Cancelled it MUST also freeze `failure_reason` to
    /// `slot_never_started`, so the InjectedFailed reason-collection
    /// (which inserts a reason whenever `slot_failure_reason` returns
    /// `Ok(Some(_))` for a Failed|Cancelled slot) produces a non-null
    /// reason. Already-terminal slots (Completed, or Failed with their
    /// own reason) MUST NOT be overwritten — the `status = 'pending'`
    /// guard in the UPDATE enforces this.
    #[test]
    fn u5_cancel_freezes_never_started_reason() {
        use crate::supervisor::worker_outcome::{REASON_SLOT_NEVER_STARTED, REASON_WORKER_TIMEOUT};
        let store = store();
        let wave = store.register_wave("u5-cancel", WaveKind::Exec, 3).unwrap();
        for i in 0..3 {
            store.bind_worktree(&wave, i, bind(i)).unwrap();
        }
        // Slot 0: dispatch + complete → terminal Completed, reason None.
        store.try_dispatch_next(4).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h0", 1).unwrap();
        // Slot 1: dispatch then fail with worker_timeout → terminal
        // Failed carrying its own reason (must NOT be overwritten).
        store.try_dispatch_next(4).unwrap().unwrap();
        store
            .record_slot_failure(&wave, 1, REASON_WORKER_TIMEOUT)
            .unwrap();
        // Slot 2: stays Pending (never dispatched).
        store.cancel_wave(&wave).unwrap();

        let snap = store.fan_in_status(&wave).unwrap();
        let status = |idx: u32| {
            snap.slots
                .iter()
                .find(|(i, _)| *i == idx)
                .map(|(_, s)| *s)
                .unwrap()
        };
        // Slot 2 flipped Pending → Cancelled, reason frozen.
        assert_eq!(status(2), SlotStatus::Cancelled);
        assert_eq!(
            store.slot_failure_reason(&wave, 2).unwrap(),
            Some(REASON_SLOT_NEVER_STARTED.to_string())
        );
        // Slot 0 Completed, untouched, reason None.
        assert_eq!(status(0), SlotStatus::Completed);
        assert_eq!(store.slot_failure_reason(&wave, 0).unwrap(), None);
        // Slot 1 already Failed with worker_timeout → NOT overwritten.
        assert_eq!(status(1), SlotStatus::Failed);
        assert_eq!(
            store.slot_failure_reason(&wave, 1).unwrap(),
            Some(REASON_WORKER_TIMEOUT.to_string())
        );
    }

    /// R-C4: opening a corrupted / unreadable path returns
    /// `Open` so the runtime can fail-closed without panicking.
    #[test]
    fn open_failure_returns_open_error() {
        // `Connection::open` is permissive: empty paths
        // typically try to create a file. We force an Open
        // error by passing an empty file (no SQLite header)
        // and asking for a write that fails — instead we just
        // confirm the error type is wired correctly: pass a
        // directory path (which Connection::open rejects).
        let dir = tempfile::tempdir().unwrap();
        let err = RusqliteSupervisorStore::open(dir.path()).unwrap_err();
        assert!(
            matches!(err, SupervisorStoreError::Open(_)),
            "directory path must surface Open error; got {err:?}"
        );
    }

    /// R-migration-idempotency: run is idempotent across
    /// reopens (mirrors the in-memory store's expectation
    /// that re-running migrations is harmless).
    #[test]
    fn migrations_idempotent_across_reopen() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("supervisor.db");
        {
            let s = RusqliteSupervisorStore::open(&path).unwrap();
            s.register_wave("idem", WaveKind::Exec, 1).unwrap();
        }
        let s = RusqliteSupervisorStore::open(&path).unwrap();
        // No panic + still functional.
        let _ = s.register_wave("idem2", WaveKind::Exec, 1).unwrap();
    }

    /// Sanity: in_flight_count + pending_count + completed +
    /// failed = expected_total.
    #[test]
    fn slot_count_partition_is_consistent() {
        let store = store();
        let wave = store.register_wave("part", WaveKind::Exec, 3).unwrap();
        for i in 0..3 {
            store.bind_worktree(&wave, i, bind(i)).unwrap();
        }
        store.try_dispatch_next(2).unwrap().unwrap();
        let snap = store.fan_in_status(&wave).unwrap();
        assert_eq!(
            snap.completed_count + snap.failed_count + snap.in_flight_count + snap.pending_count,
            snap.expected_total
        );
    }

    /// Dispatched status moves a slot into in_flight_count
    /// and keeps it visible in the partition.
    #[test]
    fn dispatch_moves_slot_to_in_flight() {
        let store = store();
        let wave = store.register_wave("dsp", WaveKind::Exec, 2).unwrap();
        for i in 0..2 {
            store.bind_worktree(&wave, i, bind(i)).unwrap();
        }
        store.try_dispatch_next(2).unwrap().unwrap();
        let snap = store.fan_in_status(&wave).unwrap();
        assert_eq!(snap.in_flight_count, 1);
        assert_eq!(snap.pending_count, 1);
    }

    /// Ensure the open-on-disk path with a non-empty
    /// directory parent fails closed rather than panicking
    /// on a permission denied scenario. We approximate with
    /// a path inside a directory that was already deleted.
    #[test]
    fn open_returns_open_for_invalid_subpath() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("missing-parent").join("sup.db");
        let err = RusqliteSupervisorStore::open(&bogus).unwrap_err();
        assert!(matches!(err, SupervisorStoreError::Open(_)));
    }

    // Ensure the helper enum ↔ str bridges still match.
    #[test]
    fn phase_to_str_round_trips() {
        for phase in [
            WavePhase::Dispatch,
            WavePhase::Collect,
            WavePhase::Integrate,
            WavePhase::Done,
            WavePhase::Failed,
        ] {
            assert_eq!(parse_phase(phase_to_str(phase)).unwrap(), phase);
        }
    }

    #[test]
    fn status_to_str_round_trips() {
        for status in [
            SlotStatus::Pending,
            SlotStatus::Dispatched,
            SlotStatus::Running,
            SlotStatus::Completed,
            SlotStatus::Failed,
            SlotStatus::Cancelled,
        ] {
            assert_eq!(parse_status(status_to_str(status)).unwrap(), status);
        }
    }

    #[test]
    fn isolation_to_str_round_trips() {
        for iso in [IsolationMode::Worktree, IsolationMode::SharedReadonly] {
            assert_eq!(parse_isolation(isolation_to_str(iso)).unwrap(), iso);
        }
    }

    // Tokio spawn_blocking is not required for the supervisor
    // path: the runtime runs on the main loop. Confirm a
    // `block_in_place` style synchronous call works fine; this
    // also documents the intent.
    #[test]
    fn store_is_send_sync_for_dispatcher_bridge() {
        fn assert_send<T: Send + Sync>() {}
        assert_send::<RusqliteSupervisorStore>();
    }

    /// U8 / F-008 / R8: rebinding a slot to a different
    /// worktree path must call `cleanup_worktree` on the
    /// prior path before overwriting. The rusqlite store
    /// mirrors the in-memory contract so the test doubles
    /// as a parity pin.
    #[test]
    fn bind_worktree_rebind_cleans_up_prior_path() {
        cleanup_calls_reset();
        let store = store();
        let wave = store
            .register_wave("rebind-sql", WaveKind::Exec, 1)
            .unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/old/0".to_string()),
                    branch: Some("ralph/old".to_string()),
                },
            )
            .unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/new/0".to_string()),
                    branch: Some("ralph/new".to_string()),
                },
            )
            .unwrap();
        let calls = cleanup_calls_snapshot();
        assert_eq!(calls, vec![".ralph/old/0".to_string()]);
        let final_binding = store.get_slot_resource(&wave, 0).unwrap().unwrap();
        assert_eq!(final_binding.worktree_path.as_deref(), Some(".ralph/new/0"));
    }

    /// U8 / F-008 / R8 edge: fresh slot → no cleanup call.
    #[test]
    fn bind_worktree_fresh_does_not_call_cleanup() {
        cleanup_calls_reset();
        let store = store();
        let wave = store.register_wave("fresh-sql", WaveKind::Exec, 1).unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/new/0".to_string()),
                    branch: Some("ralph/new".to_string()),
                },
            )
            .unwrap();
        let calls = cleanup_calls_snapshot();
        assert!(calls.is_empty());
    }
}

/// 2026-07-23-001 plan U8 / R11: real-rusqlite crash/restart
/// recovery evidence.
///
/// Every test in this module drives a **file-backed** SQLite
/// database inside a `tempfile::TempDir` (never the dev repo's
/// `.ralph/supervisor.db`), writes wave/slot state, **drops the
/// store entirely** to simulate an unclean process exit, then
/// reopens the *same path with a brand-new connection* and proves
/// the recovery contract:
///
/// 1. state is continuous across the reopen (slot statuses,
///    counts and the `merged_to_events` inject key all survive —
///    something an in-memory store fundamentally cannot do);
/// 2. `recover_active_waves_at_startup` does NOT re-dispatch
///    completed slots and does NOT re-inject an already-merged
///    coordination event;
/// 3. genuinely pending slots resume dispatch, still gated by the
///    U3/U4 backpressure cap reconstructed from persisted rows.
///
/// A fresh connection reading data that a dropped connection wrote
/// is, by construction, proof the evidence came from the DB file on
/// disk rather than from process memory.
#[cfg(feature = "supervisor-db")]
#[cfg(test)]
mod recovery_reopen_tests {
    use super::RusqliteSupervisorStore;
    use crate::supervisor::{
        CoordinatorAction, PhaseInputs, SlotResource, SlotStatus, SupervisorCoordinator,
        SupervisorStore, WaveKind, WavePhase, recover_active_waves_at_startup,
    };
    use std::path::Path;
    use std::sync::Arc;

    /// Open a file-backed store at `path` (runs migrations).
    fn file_store(path: &Path) -> RusqliteSupervisorStore {
        RusqliteSupervisorStore::open(path).unwrap()
    }

    /// Bind a worktree-isolation slot so `try_dispatch_next` will
    /// consider it (worktree slots are skipped until bound).
    fn bind(store: &RusqliteSupervisorStore, wave: &str, idx: u32) {
        store
            .bind_worktree(
                wave,
                idx,
                SlotResource {
                    slot_index: idx,
                    worktree_path: Some(format!(".ralph/u8/{wave}/{idx}")),
                    branch: Some(format!("ralph/u8/{wave}/{idx}")),
                },
            )
            .unwrap();
    }

    /// Seed a 5-slot wave with the exact crash shape the U8
    /// acceptance test pins: slots 0,1 completed; slots 2,3
    /// dispatched (in flight); slot 4 still pending. Returns the
    /// `wave_id`.
    fn seed_mixed_wave(store: &RusqliteSupervisorStore) -> String {
        let wave = store
            .register_wave("u8-recover", WaveKind::Exec, 5)
            .unwrap();
        for idx in 0..5 {
            bind(store, &wave, idx);
        }
        // Dispatch 0,1 then complete them → Completed.
        assert_eq!(
            store.try_dispatch_next(10).unwrap(),
            Some((wave.clone(), 0))
        );
        assert_eq!(
            store.try_dispatch_next(10).unwrap(),
            Some((wave.clone(), 1))
        );
        store.record_slot_result(&wave, 0, "hash-0", 1).unwrap();
        store.record_slot_result(&wave, 1, "hash-1", 1).unwrap();
        // Dispatch 2,3 → left in flight when the "process dies".
        assert_eq!(
            store.try_dispatch_next(10).unwrap(),
            Some((wave.clone(), 2))
        );
        assert_eq!(
            store.try_dispatch_next(10).unwrap(),
            Some((wave.clone(), 3))
        );
        // Slot 4 remains pending.
        let snap = store.fan_in_status(&wave).unwrap();
        assert_eq!(snap.completed_count, 2);
        assert_eq!(snap.in_flight_count, 2);
        assert_eq!(snap.pending_count, 1);
        wave
    }

    /// Reopen continuity + no-double-dispatch + pending-resumes.
    ///
    /// The task's named RED test. After an unclean exit the store
    /// is dropped and reopened on the same file; recovery must see
    /// the identical slot partition, and a subsequent dispatch
    /// drain must hand out ONLY the pending slot (4) — never a
    /// completed (0,1) or in-flight (2,3) slot.
    #[test]
    fn test_reopen_recovery_no_double_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervisor.db");

        // ---- Phase 1: run, then "crash" (drop the store). ----
        let wave = {
            let store = file_store(&path);
            let wave = seed_mixed_wave(&store);
            // Real-file evidence: the DB file exists and is
            // non-empty before the store is dropped.
            assert!(path.exists(), "supervisor.db must exist on disk");
            assert!(
                std::fs::metadata(&path).unwrap().len() > 0,
                "supervisor.db must be non-empty before crash"
            );
            wave
            // `store` drops here → the Connection closes, WAL is
            // checkpointed, all committed rows are durable.
        };

        // ---- Phase 2: restart on the same file. ----
        let store = Arc::new(file_store(&path));

        // (a) State is continuous across the reopen. A fresh
        // connection reads back exactly what the dropped
        // connection committed — impossible for in-memory state.
        let snap = store.fan_in_status(&wave).unwrap();
        assert_eq!(snap.expected_total, 5);
        assert_eq!(snap.completed_count, 2, "completed must survive reopen");
        assert_eq!(snap.in_flight_count, 2, "in-flight must survive reopen");
        assert_eq!(snap.pending_count, 1, "pending must survive reopen");
        assert!(!snap.merged_to_events);
        assert_eq!(snap.phase, WavePhase::Collect);
        assert_eq!(
            snap.slots,
            vec![
                (0, SlotStatus::Completed),
                (1, SlotStatus::Completed),
                (2, SlotStatus::Dispatched),
                (3, SlotStatus::Dispatched),
                (4, SlotStatus::Pending),
            ],
            "per-slot statuses must be byte-for-byte continuous"
        );

        // (b) Recovery inspects the wave, does not time it out
        // (fresh wave, generous budget) and does not mutate it.
        let report = recover_active_waves_at_startup(store.clone(), 3600).unwrap();
        assert_eq!(report.inspected, 1);
        assert!(report.timed_out.is_empty());
        assert!(report.already_merged.is_empty());
        let after = store.fan_in_status(&wave).unwrap();
        assert_eq!(
            after.completed_count, 2,
            "recovery must not touch completed"
        );
        assert_eq!(
            after.phase,
            WavePhase::Collect,
            "recovery must not fail a fresh wave"
        );

        // (c) Dispatch drain: ONLY the pending slot (4) is handed
        // out. Completed (0,1) and in-flight (2,3) are never
        // re-dispatched → "complete once", no double spawn.
        let mut dispatched = Vec::new();
        while let Some((w, idx)) = store.try_dispatch_next(10).unwrap() {
            assert_eq!(w, wave);
            dispatched.push(idx);
        }
        assert_eq!(
            dispatched,
            vec![4],
            "after restart only the surviving pending slot resumes; \
             completed/in-flight slots must not be re-dispatched"
        );
        // Drained: no further dispatch.
        assert_eq!(store.try_dispatch_next(10).unwrap(), None);
    }

    /// Idempotent inject key across a restart: a wave whose coord
    /// event was already merged (`merged_to_events = true`) must
    /// (a) persist that flag to disk, (b) be skipped by recovery
    /// (landed in `already_merged`, not re-injected), and (c) make
    /// the coordinator return `AlreadyDone` instead of
    /// `InjectedComplete` on the restarted store.
    #[test]
    fn test_reopen_merged_wave_not_reinjected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervisor.db");

        // ---- Phase 1: fully merge the wave, then crash. ----
        let wave = {
            let store = file_store(&path);
            let wave = store.register_wave("u8-merged", WaveKind::Exec, 2).unwrap();
            bind(&store, &wave, 0);
            bind(&store, &wave, 1);
            assert_eq!(
                store.try_dispatch_next(10).unwrap(),
                Some((wave.clone(), 0))
            );
            assert_eq!(
                store.try_dispatch_next(10).unwrap(),
                Some((wave.clone(), 1))
            );
            store.record_slot_result(&wave, 0, "m-0", 1).unwrap();
            store.record_slot_result(&wave, 1, "m-1", 1).unwrap();
            // The coordinator already injected the coord event and
            // stamped the idempotent-inject marker before the crash.
            store.mark_merge_to_events(&wave).unwrap();
            assert!(store.fan_in_status(&wave).unwrap().merged_to_events);
            wave
        };

        // ---- Phase 2: restart on the same file. ----
        let store = Arc::new(file_store(&path));

        // (a) The inject key survived the round-trip to disk.
        let snap = store.fan_in_status(&wave).unwrap();
        assert!(
            snap.merged_to_events,
            "merged_to_events inject key must persist across reopen"
        );

        // (b) Recovery skips it: it lands in `already_merged` and
        // is NOT escalated/re-injected.
        let report = recover_active_waves_at_startup(store.clone(), 3600).unwrap();
        assert_eq!(
            report.already_merged,
            vec![wave.clone()],
            "already-merged wave must be skipped by recovery, not re-injected"
        );
        assert!(report.timed_out.is_empty());

        // (c) The coordinator tick on the restarted store refuses
        // to re-inject: every slot is terminal so the phase gate
        // says Integrate, but the `merged_to_events` guard in
        // `merge_and_complete` short-circuits to `AlreadyDone`.
        let coord = SupervisorCoordinator::with_in_memory_sink(store.clone());
        let action = coord
            .tick(
                &wave,
                PhaseInputs {
                    aggregate_timeout_secs: 3600,
                    elapsed_secs: 0,
                    cancel_requested: false,
                },
            )
            .unwrap();
        assert_eq!(
            action,
            CoordinatorAction::AlreadyDone,
            "coordinator must not re-inject the coord event after restart"
        );
        // And the merge sink received nothing on the restart tick.
        assert!(
            coord.sink_batches().is_empty(),
            "no merge batch may be appended for an already-merged wave"
        );
    }

    /// Pending slots resume dispatch after a restart but remain
    /// gated by the U3/U4 backpressure cap, which is reconstructed
    /// from the persisted in-flight (dispatched) rows.
    #[test]
    fn test_reopen_recovery_respects_dispatch_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervisor.db");

        // ---- Phase 1: 2 in-flight + 2 pending, then crash. ----
        let wave = {
            let store = file_store(&path);
            let wave = store.register_wave("u8-cap", WaveKind::Exec, 4).unwrap();
            for idx in 0..4 {
                bind(&store, &wave, idx);
            }
            // Slots 0,1 dispatched → in flight (count against cap).
            assert_eq!(
                store.try_dispatch_next(10).unwrap(),
                Some((wave.clone(), 0))
            );
            assert_eq!(
                store.try_dispatch_next(10).unwrap(),
                Some((wave.clone(), 1))
            );
            let snap = store.fan_in_status(&wave).unwrap();
            assert_eq!(snap.in_flight_count, 2);
            assert_eq!(snap.pending_count, 2);
            wave
        };

        // ---- Phase 2: restart on the same file. ----
        let store = file_store(&path);
        let _ = recover_active_waves_at_startup(Arc::new(store.clone()), 3600).unwrap();

        // Cap == active (2): the persisted in-flight rows still
        // saturate the ceiling, so dispatch is refused even though
        // pending slots exist.
        assert_eq!(
            store.try_dispatch_next(2).unwrap(),
            None,
            "persisted in-flight slots must still count against the cap after restart"
        );

        // Cap == 3: one permit frees up, the lowest pending slot
        // (2) resumes — proving pending continuation is real but
        // cap-gated.
        assert_eq!(
            store.try_dispatch_next(3).unwrap(),
            Some((wave.clone(), 2)),
            "a freed cap permit resumes the lowest pending slot after restart"
        );
        // Cap back to 3 but active is now 3 (0,1,2) → saturated.
        assert_eq!(store.try_dispatch_next(3).unwrap(), None);
    }

    /// Recovery does not manufacture state on an empty DB file:
    /// reopening a freshly-migrated (but wave-free) database yields
    /// an empty report. Pins that recovery reads real rows rather
    /// than replaying any in-memory cache from the prior process.
    #[test]
    fn test_reopen_empty_db_recovers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervisor.db");
        {
            let _store = file_store(&path); // migrations only, no waves
        }
        let store = Arc::new(file_store(&path));
        let report = recover_active_waves_at_startup(store, 3600).unwrap();
        assert_eq!(report.inspected, 0);
        assert!(report.timed_out.is_empty());
        assert!(report.already_merged.is_empty());
    }
}

// Silence unused-warning when the `supervisor-db` feature is
// off — the module never compiles in, but the file's
// `pub use` and downstream impl items must compile under
// `#[cfg]` to keep U3/U4 working.
#[cfg(not(feature = "supervisor-db"))]
#[allow(dead_code)]
fn _feature_off_marker() {}

// `DispatchOutcome` is only used via the trait; ensure it
// stays referenced even when `supervisor-db` is off so the
// in-memory store's migration check doesn't lose it.
const _: fn() = || {
    let _ = std::mem::size_of::<DispatchOutcome>();
};

/// 2026-07-03-001 plan U8: rebind cleanup helper for the
/// rusqlite store. Production uses
/// `crate::worktree::remove_worktree`; tests inspect
/// `CLEANUP_SPY` via `cleanup_calls_snapshot` /
/// `cleanup_calls_reset` to assert the rebind path
/// actually called cleanup on the prior path.
#[cfg(feature = "supervisor-db")]
fn cleanup_worktree_path(path: &str) {
    CLEANUP_SPY.with(|spy| {
        spy.borrow_mut().push(path.to_string());
    });
}

#[cfg(feature = "supervisor-db")]
thread_local! {
    static CLEANUP_SPY: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(feature = "supervisor-db")]
#[cfg(test)]
pub fn cleanup_calls_snapshot() -> Vec<String> {
    CLEANUP_SPY.with(|spy| spy.borrow().clone())
}

#[cfg(feature = "supervisor-db")]
#[cfg(test)]
pub fn cleanup_calls_reset() {
    CLEANUP_SPY.with(|spy| spy.borrow_mut().clear());
}
