//! Schema migrations for the rusqlite supervisor store.
//!
//! U5 must mirror the in-memory store contract. The migration
//! ledger is keyed by `user_version` (no separate
//! `schema_migrations` table) so the migration is idempotent
//! across re-opens and there is no extra metadata table to
//! keep in sync with `user_version`. The migrations are
//! forward-only — U13 cannot regress here without an explicit
//! reset path, which is gated on a separate `--reset` CLI
//! flag added when the preset ships.
//!
//! The migration applies under a single `Connection` so the
//! SQLite engine bumps `user_version` atomically with the
//! last DDL change.

#[cfg(feature = "supervisor-db")]
mod imp {
    use rusqlite::Connection;

    /// Bumped whenever the schema below changes. The migrations
    /// list is ordered; running each migration brings the
    /// database up to that version.
    ///
    /// U4 bump: `wave_id_seq` autoincrement table replaces the
    /// pre-fix `SELECT COUNT(*) + 1 FROM waves` allocator.
    /// U4 (2026-07-24-003) bump: `wave_emissions` reservation
    /// table backs the CLI emission state machine.
    /// v6 (2026-07-25-005 plan U2) adds `attempt_count` /
    /// `max_attempts` on `wave_slots` and `attempt_epoch` /
    /// `parent_wave_id` / `slot_retry_budget` /
    /// `published_failure_payload` on `waves`.
    /// v7 (2026-07-25-005 plan U4) adds `redrive_requests`
    /// idempotency ledger.
    /// v8 (2026-07-27-003 plan U5) replaces the legacy
    /// `merged_to_events` / `salvage_merged` boolean pair with
    /// `delivery_state` (Pending / BusinessProjected /
    /// SalvageCommitted / CoordinationWritten /
    /// CoordinationCommitted) and persists the salvage /
    /// coordination receipt summaries.
    #[allow(dead_code)] // pinned by `migrations_idempotent_across_reopen`; production writes via pragma_update
    pub const CURRENT_VERSION: i64 = 8;

    /// Apply migrations sequentially. Each migration is a
    /// closure that performs the SQL DDL and bumps the
    /// `user_version` of the database. The closure MUST be
    /// idempotent: re-running on an already-current database
    /// is a no-op (SQLite `IF NOT EXISTS` clauses guarantee
    /// this for table/index creation).
    pub fn run(connection: &Connection) -> rusqlite::Result<()> {
        // Pragmas: busy_timeout FIRST so the WAL header switch
        // below tolerates a concurrent process racing the same
        // fresh database (2026-07-25).  WAL mode (R-DB-0) and
        // foreign keys ON follow.  `PRAGMA journal_mode = WAL`
        // returns the new mode; `WAL` is what we asked for, so
        // the assignment is a success path even though `execute`
        // ignores the row.
        connection.execute_batch(
            "PRAGMA busy_timeout = 5000;
             PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )?;

        let current = user_version(connection)?;
        for migration in migrations() {
            if current < migration.version {
                if let Some(per_column) = migration.column_probe {
                    apply_with_column_probe(connection, per_column)?;
                } else {
                    connection.execute_batch(migration.ddl)?;
                }
                connection.pragma_update(None, "user_version", migration.version)?;
            }
        }
        Ok(())
    }

    /// Plan 004 (post-P0-2 hotfix): when two ralph CLI processes
    /// race to migrate a fresh supervisor DB, the second
    /// process can hit `duplicate column name` on the
    /// `ALTER TABLE ... ADD COLUMN` statements inside a
    /// migration. SQLite has no `ADD COLUMN IF NOT EXISTS`,
    /// so we probe via `pragma_table_info` first and skip the
    /// ALTER for columns that already exist. We wrap the
    /// probe + ALTER in a transaction so a concurrent opener
    /// that already saw the new columns does not see a
    /// half-migrated schema.
    fn apply_with_column_probe(
        connection: &Connection,
        columns: &[(
            /* table */ &str,
            /* column */ &str,
            /* ddl */ &str,
        )],
    ) -> rusqlite::Result<()> {
        connection.execute_batch("BEGIN IMMEDIATE")?;
        for (table, column, ddl) in columns {
            let present: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                    rusqlite::params![table, column],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            if present == 0 {
                connection.execute_batch(ddl)?;
            }
        }
        connection.execute_batch("COMMIT")?;
        Ok(())
    }

    /// Read the current `user_version` value. SQLite stores
    /// it as a 32-bit signed integer in the header; the
    /// `user_version` pragma surfaces it as a normal scalar.
    pub fn user_version(connection: &Connection) -> rusqlite::Result<i64> {
        connection.pragma_query_value(None, "user_version", |row| row.get(0))
    }

    /// One DDL step. The `version` field is what
    /// `user_version` becomes after this step succeeds.
    struct Migration {
        version: i64,
        ddl: &'static str,
        /// Optional column-probe list: `(table, column, ALTER)`.
        /// When present, the migration runner probes each
        /// column via `pragma_table_info` and only emits the
        /// ALTER for missing columns. Use this for any migration
        /// that contains `ALTER TABLE ... ADD COLUMN`
        /// statements — the second concurrent opener would
        /// otherwise fail with `duplicate column name`.
        /// `None` (default) falls back to a plain
        /// `execute_batch(ddl)`.
        column_probe: Option<&'static [(&'static str, &'static str, &'static str)]>,
    }

    fn migrations() -> &'static [Migration] {
        // U4 / F-004 / R4: `wave_id_seq` autoincrement
        // replaces the `SELECT COUNT(*) + 1 FROM waves`
        // allocator. v1 keeps the original schema; v2 adds
        // the singleton seq row used by `register_wave`.
        // v3 (2026-07-24-003 plan U4) adds `wave_emissions`
        // for the CLI emission state machine. The migrations
        // are idempotent because every step uses
        // `CREATE TABLE IF NOT EXISTS`; existing v1/v2
        // databases auto-upgrade without touching existing
        // rows.
        /// Plan 004 (post-P0-2 hotfix): v4 + v5 use the
        /// column-probe path because their DDL contains
        /// `ALTER TABLE ... ADD COLUMN` statements. SQLite
        /// has no `ADD COLUMN IF NOT EXISTS`, so two ralph
        /// CLI processes racing to migrate a fresh DB would
        /// otherwise fail the second opener with
        /// `duplicate column name: evidence_topic` (or
        /// `salvage_merged`).
        const V4_PROBE: &[(
            /* table */ &str,
            /* column */ &str,
            /* ddl */ &str,
        )] = &[
            (
                "wave_slots",
                "evidence_topic",
                "ALTER TABLE wave_slots ADD COLUMN evidence_topic TEXT",
            ),
            (
                "wave_slots",
                "evidence_dimension",
                "ALTER TABLE wave_slots ADD COLUMN evidence_dimension TEXT",
            ),
            (
                "wave_slots",
                "evidence_fingerprint",
                "ALTER TABLE wave_slots ADD COLUMN evidence_fingerprint TEXT",
            ),
        ];
        const V5_PROBE: &[(
            /* table */ &str,
            /* column */ &str,
            /* ddl */ &str,
        )] = &[(
            "waves",
            "salvage_merged",
            "ALTER TABLE waves ADD COLUMN salvage_merged INTEGER NOT NULL DEFAULT 0",
        )];
        /// 2026-07-25-005 plan U2: slot attempt/retry model.
        /// Adds `attempt_count` / `max_attempts` to `wave_slots` and
        /// `attempt_epoch` / `parent_wave_id` / `slot_retry_budget` /
        /// `published_failure_payload` to `waves`. Each column gets its
        /// own ALTER so the column-probe skips only columns already
        /// present from a prior migration run on a concurrent opener.
        const V6_PROBE: &[(
            /* table */ &str,
            /* column */ &str,
            /* ddl */ &str,
        )] = &[
            (
                "wave_slots",
                "attempt_count",
                "ALTER TABLE wave_slots ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "wave_slots",
                "max_attempts",
                "ALTER TABLE wave_slots ADD COLUMN max_attempts INTEGER",
            ),
            (
                "waves",
                "attempt_epoch",
                "ALTER TABLE waves ADD COLUMN attempt_epoch INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "waves",
                "parent_wave_id",
                "ALTER TABLE waves ADD COLUMN parent_wave_id TEXT",
            ),
            (
                "waves",
                "slot_retry_budget",
                "ALTER TABLE waves ADD COLUMN slot_retry_budget INTEGER NOT NULL DEFAULT 1",
            ),
            (
                "waves",
                "published_failure_payload",
                "ALTER TABLE waves ADD COLUMN published_failure_payload INTEGER NOT NULL DEFAULT 0",
            ),
        ];
        /// 2026-07-27-003 plan U5 (R12 / R10): each new column gets
        /// its own ALTER inside the column-probe path so the
        /// concurrent-opener race mirrors the v4/v5 fix.
        const V8_PROBE: &[(
            /* table */ &str,
            /* column */ &str,
            /* ddl */ &str,
        )] = &[
            (
                "waves",
                "delivery_state",
                "ALTER TABLE waves ADD COLUMN delivery_state TEXT NOT NULL DEFAULT 'pending'",
            ),
            (
                "waves",
                "salvage_fingerprint",
                "ALTER TABLE waves ADD COLUMN salvage_fingerprint TEXT NOT NULL DEFAULT ''",
            ),
            (
                "waves",
                "salvage_write_count",
                "ALTER TABLE waves ADD COLUMN salvage_write_count INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "waves",
                "salvage_already_present",
                "ALTER TABLE waves ADD COLUMN salvage_already_present INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "waves",
                "salvage_committed_at",
                "ALTER TABLE waves ADD COLUMN salvage_committed_at INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "waves",
                "coordination_topic",
                "ALTER TABLE waves ADD COLUMN coordination_topic TEXT NOT NULL DEFAULT ''",
            ),
            (
                "waves",
                "coordination_idempotency_key",
                "ALTER TABLE waves ADD COLUMN coordination_idempotency_key TEXT NOT NULL DEFAULT ''",
            ),
            (
                "waves",
                "coordination_fingerprint",
                "ALTER TABLE waves ADD COLUMN coordination_fingerprint TEXT NOT NULL DEFAULT ''",
            ),
            (
                "waves",
                "coordination_write_count",
                "ALTER TABLE waves ADD COLUMN coordination_write_count INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "waves",
                "coordination_already_present",
                "ALTER TABLE waves ADD COLUMN coordination_already_present INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "waves",
                "coordination_committed_at",
                "ALTER TABLE waves ADD COLUMN coordination_committed_at INTEGER NOT NULL DEFAULT 0",
            ),
        ];
        &[
            Migration {
                version: 1,
                ddl: include_str!("migrations/v1.sql"),
                column_probe: None,
            },
            Migration {
                version: 2,
                ddl: include_str!("migrations/v2.sql"),
                column_probe: None,
            },
            Migration {
                version: 3,
                ddl: include_str!("migrations/v3.sql"),
                column_probe: None,
            },
            Migration {
                version: 4,
                ddl: include_str!("migrations/v4.sql"),
                column_probe: Some(V4_PROBE),
            },
            Migration {
                version: 5,
                ddl: include_str!("migrations/v5.sql"),
                column_probe: Some(V5_PROBE),
            },
            Migration {
                version: 6,
                ddl: include_str!("migrations/v6.sql"),
                column_probe: Some(V6_PROBE),
            },
            Migration {
                version: 7,
                ddl: include_str!("migrations/v7.sql"),
                column_probe: None,
            },
            Migration {
                version: 8,
                ddl: include_str!("migrations/v8.sql"),
                column_probe: Some(V8_PROBE),
            },
        ]
    }
}

#[cfg(feature = "supervisor-db")]
pub use imp::run;

#[cfg(test)]
#[cfg(feature = "supervisor-db")]
pub(crate) use imp::{CURRENT_VERSION, user_version};

#[cfg(test)]
#[cfg(feature = "supervisor-db")]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn user_version_starts_at_zero_on_fresh_database() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(user_version(&conn).unwrap(), 0);
    }

    #[test]
    fn run_bumps_user_version_to_current() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn run_is_idempotent_across_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervisor.db");
        {
            let conn = Connection::open(&path).unwrap();
            run(&conn).unwrap();
            assert_eq!(user_version(&conn).unwrap(), CURRENT_VERSION);
        }
        // Reopen → run again. Already at current, but the
        // `if current < version` guard means DDL is skipped.
        let conn = Connection::open(&path).unwrap();
        run(&conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn required_tables_exist_after_run() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        let tables = [
            "waves",
            "wave_slots",
            "slot_resources",
            "dispatch_records",
            "worker_results",
            "wave_queue",
            "compensation_jobs",
            // U4: `wave_id_seq` is the atomic wave_id
            // allocator; the migration test pins its
            // existence so a future DDL drop would surface
            // before runtime.
            "wave_id_seq",
            // U7 (2026-07-25-005 plan U4): idempotent redrive
            // request ledger.
            "redrive_requests",
        ];
        for table in tables {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                count, 1,
                "table `{table}` must exist after run() (got {count})"
            );
        }
    }

    /// Plan 004 (post-P0-2 hotfix): two threads racing to
    /// migrate the same fresh DB must BOTH succeed. The
    /// pre-fix `ALTER TABLE ... ADD COLUMN` raised
    /// `duplicate column name` on the second opener; the
    /// column-probe path lets the second opener see the
    /// columns already exist and skip the ALTER. We mirror
    /// `RusqliteSupervisorStore::open`'s `SQLITE_BUSY` retry
    /// so the test does not flake on filesystem-level WAL
    /// sidecar races that bypass SQLite's busy handler.
    #[test]
    fn concurrent_openers_do_not_collide_on_v4_v5_columns() {
        use std::sync::{Arc, Barrier};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervisor.db");
        let path = Arc::new(path);
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let path = Arc::clone(&path);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let conn = Connection::open(path.as_ref()).unwrap();
                conn.pragma_update(None, "busy_timeout", 5000).unwrap();
                barrier.wait();
                // Mirror RusqliteSupervisorStore::open's
                // SQLITE_BUSY retry on WAL sidecar races.
                const MIGRATION_RETRIES: u32 = 5;
                let mut attempt = 0u32;
                loop {
                    match run(&conn) {
                        Ok(()) => return,
                        Err(err) if attempt < MIGRATION_RETRIES => {
                            let busy = matches!(
                                &err,
                                rusqlite::Error::SqliteFailure(
                                    rusqlite::ffi::Error {
                                        code: rusqlite::ErrorCode::DatabaseBusy,
                                        ..
                                    },
                                    _,
                                )
                            );
                            if !busy {
                                panic!("non-busy migration error: {err}");
                            }
                            std::thread::sleep(std::time::Duration::from_millis(
                                50 * (attempt as u64 + 1),
                            ));
                            attempt += 1;
                        }
                        Err(err) => panic!("migration failed after retries: {err}"),
                    }
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // Open a third connection and verify the columns
        // are present and the schema is the expected
        // post-v5 shape.
        let conn = Connection::open(path.as_ref()).unwrap();
        for col in [
            "evidence_topic",
            "evidence_dimension",
            "evidence_fingerprint",
        ] {
            let present: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('wave_slots') WHERE name = ?1",
                    [col],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                present, 1,
                "wave_slots.{col} must exist after concurrent migration"
            );
        }
        // waves must carry salvage_merged (v5).
        let present: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('waves') WHERE name = 'salvage_merged'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            present, 1,
            "waves.delivery_state.at_least(super::WaveDeliveryState::SalvageCommitted) must exist after concurrent migration"
        );
    }
}
