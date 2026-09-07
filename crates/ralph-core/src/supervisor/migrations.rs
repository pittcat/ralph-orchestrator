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
    /// v11 (2026-08-07-009 plan U1) adds the `slot_attempts`
    /// receipt ledger so the dispatcher can persist per-Worker
    /// attempt start/finish state across reopens without
    /// rewriting the main JSONL log.
    /// v12 (2026-09-01-001 plan U1) adds the `slot_event_payloads`
    /// ledger so a wave's accepted slot events survive a loop
    /// death between worker exit and `run_supervisor_fan_in`.
    /// Crash recovery (U2 / U3) replays these rows through the
    /// existing salvage seam to bring the main ledger back to
    /// the same state a healthy fan-in would have produced.
    /// v13 (PMI-006 / B2 2026-09-03-0959 plan U3) adds the
    /// `dag_plans` / `dag_integrations` tables so the DAG
    /// scheduler's plan registrations and integration records
    /// survive process restarts (durable DAG store in
    /// `dag_store_rusqlite.rs`).
    /// v14 (PMI-013 / 2026-09-03-0959 plan Step 0, U3 residual) adds
    /// the `dag_plan_receipts` durable registration receipt table
    /// (R17/D18: the receipt survives the crash window between the
    /// `forge.plan.ready` accepted boundary and task projection /
    /// ack) plus the DAG unit / resource-lease / job-attempt
    /// persistence tables (`dag_units` / `dag_resource_leases` /
    /// `dag_jobs`) the runtime-owned scheduler wires up in later
    /// steps.
    #[allow(dead_code)] // pinned by `migrations_idempotent_across_reopen`; production writes via pragma_update
    pub const CURRENT_VERSION: i64 = 14;

    /// PMI-013 / TGP-02: typed error returned when a database's
    /// `user_version` is ABOVE this binary's migration ledger tail
    /// — i.e. a newer binary migrated the DB and the current
    /// (older) binary is now opening it. The schema-negotiation
    /// contract is fail-closed in BOTH directions: older DB +
    /// newer binary migrates forward; newer DB + older binary is
    /// refused instead of silently running unknown-schema code.
    /// The rejection is side-effect free: `user_version` is left
    /// at the database's own value.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DatabaseAheadOfBinary {
        /// The database's current `user_version`.
        pub db_version: i64,
        /// The highest migration version this binary knows about.
        pub ledger_tail: i64,
    }

    impl std::fmt::Display for DatabaseAheadOfBinary {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "supervisor database schema version {} is newer than this binary's \
                 migration ledger tail {}; refusing to run against an unknown schema — \
                 upgrade the binary or reset the workspace database",
                self.db_version, self.ledger_tail,
            )
        }
    }

    impl std::error::Error for DatabaseAheadOfBinary {}

    /// Apply migrations sequentially. Each migration is a
    /// closure that performs the SQL DDL and bumps the
    /// `user_version` of the database. The closure MUST be
    /// idempotent: re-running on an already-current database
    /// is a no-op (SQLite `IF NOT EXISTS` clauses guarantee
    /// this for table/index creation).
    ///
    /// PMI-013 / TGP-02: when the database's `user_version` is
    /// strictly greater than the ledger tail, `run` fails closed
    /// with [`DatabaseAheadOfBinary`] before applying any DDL — a
    /// newer DB must never be silently accepted by an older
    /// binary (the pre-fix loop skipped every migration and
    /// returned `Ok(())`, letting unknown-schema code run).
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
        let ledger_tail = migrations().last().map_or(0, |m| m.version);
        if current > ledger_tail {
            return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(
                DatabaseAheadOfBinary {
                    db_version: current,
                    ledger_tail,
                },
            )));
        }
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
            // 2026-07-27-004 plan U1 (R1-R4 / D1): no-DDL
            // marker that the public-id-only contract is in
            // force. The wave row shape is unchanged from v1
            // (caller supplies the primary key); the migration
            // bumps `user_version` so a reopen can detect the
            // contract switch and refuse to silently re-enable
            // the legacy `w-{seq}` allocator.
            Migration {
                version: 9,
                ddl: include_str!("migrations/v9.sql"),
                column_probe: None,
            },
            // 2026-07-28-002 plan U2 (R4 / R5 / R6 / S2a / S4 / S5):
            // adds `slot_descriptors` table for bounded redrive
            // activation descriptors. The boot redrive scan reads
            // this table to build the expected_digest for the
            // parent → child mapping. The column-probe path is
            // NOT needed (no ALTER TABLE).
            Migration {
                version: 10,
                ddl: include_str!("migrations/v10.sql"),
                column_probe: None,
            },
            // 2026-08-07-009 plan U1 (R1 / R2 / KTD3): adds the
            // `slot_attempts` table for per-slot attempt start /
            // finish receipts. Forward-only `CREATE TABLE`; no
            // ALTERs against existing tables so the column-probe
            // path is unnecessary. `attempt_seq` is monotonic per
            // `(wave_id, slot_index)` and is allocated inside
            // `BEGIN IMMEDIATE` to keep concurrent openers safe.
            Migration {
                version: 11,
                ddl: include_str!("migrations/v11.sql"),
                column_probe: None,
            },
            // 2026-09-01-001 plan U1 (R1 / D1-D3): adds the
            // `slot_event_payloads` ledger so crash recovery can
            // replay accepted slot events to the main ledger when
            // fan-in was interrupted by a loop process death.
            // Forward-only `CREATE TABLE`; no ALTERs against
            // existing tables, so the column-probe path is
            // unnecessary. PRIMARY KEY on `(wave_id, slot_index,
            // attempt_seq, event_seq)` keeps per-event idempotency
            // inside a single (wave, slot, attempt) — replays are
            // a no-op rather than a duplicate-write.
            Migration {
                version: 12,
                ddl: include_str!("migrations/v12.sql"),
                column_probe: None,
            },
            // PMI-006 / 2026-09-03-0959 plan U3 (R2 / R17 / E9):
            // adds the `dag_plans` / `dag_integrations` tables
            // backing the durable DAG store
            // (`dag_store_rusqlite.rs`). Forward-only `CREATE
            // TABLE`; no ALTERs against existing tables, so the
            // column-probe path is unnecessary. `plan_key` /
            // `(unit_id, target_branch)` UNIQUE constraints keep
            // registration and integration idempotency at the
            // schema level; semantic checks (digest conflict /
            // duplicate-unit-for-target / fingerprint drift)
            // fail closed in the store layer.
            Migration {
                version: 13,
                ddl: include_str!("migrations/v13.sql"),
                column_probe: None,
            },
            // PMI-013 / 2026-09-03-0959 plan Step 0 (U3 residual /
            // R10 / R17 / S12 / S20): adds the `dag_plan_receipts`
            // durable registration receipt table (backing
            // `dag_plan_receipt.rs` + the receipt half of
            // `dag_store_rusqlite.rs`) and the DAG unit /
            // resource-lease / job-attempt tables
            // (`dag_units` / `dag_resource_leases` / `dag_jobs`)
            // the runtime-owned scheduler persists into in later
            // steps. Forward-only `CREATE TABLE`; no ALTERs against
            // existing tables, so the column-probe path is
            // unnecessary. `plan_key` PRIMARY KEY and
            // `UNIQUE (unit_key, stage, attempt)` keep receipt
            // idempotency and job attempt uniqueness at the schema
            // level; digest drift fails closed in the store layer.
            Migration {
                version: 14,
                ddl: include_str!("migrations/v14.sql"),
                column_probe: None,
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
            // U1 (2026-08-07-009 plan U1): per-slot attempt
            // receipt ledger. The migration test pins its
            // existence so a future DDL drop would surface
            // before runtime.
            "slot_attempts",
            // U1 (2026-09-01-001 plan U1): accepted slot event
            // payload ledger for crash recovery replay.
            "slot_event_payloads",
            // PMI-006 / B2 (2026-09-03-0959 plan U3): durable
            // DAG state family.
            "dag_plans",
            "dag_integrations",
            // PMI-013 / Step 0 (2026-09-03-0959 plan U3 residual):
            // durable registration receipt + DAG unit/lease/job
            // persistence family.
            "dag_plan_receipts",
            "dag_units",
            "dag_resource_leases",
            "dag_jobs",
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

    /// 2026-08-07-009 plan U1 (S5 / U1 §10): a v10 supervisor DB
    /// upgraded to v11 must (a) bump `user_version` to 11, (b)
    /// create the `slot_attempts` table, and (c) keep every v10
    /// row intact. The test seeds a v10 fixture by running
    /// migrations on a fresh DB then rolling back to v10, so the
    /// assertion is on a "real" v10 instance — not a hand-crafted
    /// DDL script.
    #[test]
    fn migration_v10_to_v11_preserves_existing_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervisor.db");

        // Phase 1: open fresh DB and migrate to v11 (CURRENT_VERSION).
        // Then manually rewind `user_version` to 10 so we can
        // simulate "v10 DB being opened by the upgraded code".
        {
            let conn = Connection::open(&path).unwrap();
            run(&conn).unwrap();
            assert_eq!(user_version(&conn).unwrap(), CURRENT_VERSION);
            // Drop the v11 table so the upgrade has work to do.
            conn.execute_batch("DROP TABLE IF EXISTS slot_attempts")
                .unwrap();
            // Roll the user_version back to v10 so the v11
            // migration re-fires.
            conn.pragma_update(None, "user_version", 10_i64).unwrap();
        }

        // Phase 2: seed a representative v10 dataset.
        // - one wave row (kind=exec, parent_wave_id null)
        // - one wave_slots row (with the v6 attempt_count column)
        // - one slot_resources row (parent Worktree binding)
        // - one slot_descriptors row (so a child can resolve)
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "INSERT INTO waves (wave_id, idempotency_key, kind, phase, expected_total, slot_retry_budget)
                   VALUES ('w-v10-legacy', 'idem-v10-legacy', 'exec', 'dispatch', 2, 1);
                 INSERT INTO wave_slots (wave_id, slot_index, status, isolation, attempt_count, max_attempts)
                   VALUES ('w-v10-legacy', 0, 'failed', 'worktree', 1, 1);
                 INSERT INTO slot_resources (wave_id, slot_index, worktree_path, branch)
                   VALUES ('w-v10-legacy', 0, '/tmp/legacy-worktree', 'ralph/w-v10-legacy-0');
                 INSERT INTO slot_descriptors
                   (wave_id, slot_index, slot_index_in_parent, topic, payload_json, wave_kind, payload_digest)
                   VALUES ('w-v10-legacy', 0, 0, 'exec.unit.ready', '{}', 'exec', 'digest');",
            )
            .unwrap();
        }

        // Phase 3: reopen the DB. `run` must observe user_version=10
        // and apply the v11 migration, then bump user_version to
        // 11. Every v10 row must remain unchanged.
        let conn = Connection::open(&path).unwrap();
        run(&conn).unwrap();
        assert_eq!(
            user_version(&conn).unwrap(),
            CURRENT_VERSION,
            "user_version must be CURRENT_VERSION after the upgrade"
        );

        // v11 table exists.
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='slot_attempts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            table_count, 1,
            "slot_attempts table must exist after upgrade"
        );

        // Legacy wave row preserved.
        let wave_kind: String = conn
            .query_row(
                "SELECT kind FROM waves WHERE wave_id = 'w-v10-legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(wave_kind, "exec", "legacy wave row preserved");

        // v6 columns preserved.
        let attempt_count: i64 = conn
            .query_row(
                "SELECT attempt_count FROM wave_slots WHERE wave_id = 'w-v10-legacy' AND slot_index = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attempt_count, 1, "v6 attempt_count preserved");

        // slot_resources row preserved.
        let path: String = conn
            .query_row(
                "SELECT worktree_path FROM slot_resources WHERE wave_id = 'w-v10-legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(path, "/tmp/legacy-worktree", "slot_resources row preserved");

        // slot_descriptors row preserved.
        let digest: String = conn
            .query_row(
                "SELECT payload_digest FROM slot_descriptors WHERE wave_id = 'w-v10-legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(digest, "digest", "slot_descriptors row preserved");

        // The v11 table is empty (no attempt rows existed before).
        let attempt_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM slot_attempts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            attempt_rows, 0,
            "slot_attempts starts empty for legacy wave"
        );
    }

    /// PMI-006 / B2 plan U3 §14: a v12 fixture upgraded to v13
    /// keeps every wave row intact and gains the (empty) DAG
    /// tables. Mirrors the v10→v11 differential technique: run
    /// migrations on a fresh DB, drop the v13 tables, rewind
    /// `user_version` to 12, seed a representative wave row,
    /// reopen, upgrade.
    #[test]
    fn migration_v12_to_v13_preserves_existing_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervisor.db");

        // Phase 1: fresh DB at CURRENT_VERSION, then rewind to a
        // "real" v12 instance (drop the v13 tables + rewind the
        // version so the v13 migration re-fires).
        {
            let conn = Connection::open(&path).unwrap();
            run(&conn).unwrap();
            conn.execute_batch(
                "DROP TABLE IF EXISTS dag_integrations;
                 DROP TABLE IF EXISTS dag_plans;
                 DROP INDEX IF EXISTS dag_integrations_target_idx;
                 DROP INDEX IF EXISTS dag_plans_status_idx;",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 12_i64).unwrap();
        }

        // Phase 2: seed a representative v12 dataset (one wave +
        // one slot — the wave-family state the upgrade must not
        // disturb).
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "INSERT INTO waves (wave_id, idempotency_key, kind, phase, expected_total, slot_retry_budget)
                   VALUES ('w-v12-legacy', 'idem-v12-legacy', 'exec', 'dispatch', 2, 1);
                 INSERT INTO wave_slots (wave_id, slot_index, status, isolation, attempt_count, max_attempts)
                   VALUES ('w-v12-legacy', 0, 'pending', 'worktree', 0, 1);",
            )
            .unwrap();
        }

        // Phase 3: reopen → v13 migration applies; every v12 row
        // is unchanged; the DAG tables exist and start empty.
        let conn = Connection::open(&path).unwrap();
        run(&conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), CURRENT_VERSION);

        for table in ["dag_plans", "dag_integrations"] {
            let table_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(table_count, 1, "{table} must exist after upgrade");
            let row_count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(row_count, 0, "{table} starts empty for legacy DB");
        }

        // Legacy wave row preserved.
        let wave_kind: String = conn
            .query_row(
                "SELECT kind FROM waves WHERE wave_id = 'w-v12-legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(wave_kind, "exec", "wave row preserved across v13");
        let slot_status: String = conn
            .query_row(
                "SELECT status FROM wave_slots WHERE wave_id = 'w-v12-legacy' AND slot_index = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(slot_status, "pending", "slot row preserved across v13");
    }

    /// PMI-013 / Step 0 (2026-09-03-0959 plan U3 residual §14): a
    /// v13 fixture upgraded to v14 keeps every wave-family AND
    /// v13 DAG row intact and gains the (empty) receipt / unit /
    /// lease / job tables. Mirrors the v12→v13 differential
    /// technique: run migrations on a fresh DB, drop the v14
    /// tables + indexes, rewind `user_version` to 13, seed a
    /// representative v13 dataset, reopen, upgrade.
    #[test]
    fn migration_v13_to_v14_preserves_existing_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervisor.db");

        // Phase 1: fresh DB at CURRENT_VERSION, then rewind to a
        // "real" v13 instance (drop the v14 tables + indexes and
        // rewind the version so the v14 migration re-fires).
        {
            let conn = Connection::open(&path).unwrap();
            run(&conn).unwrap();
            conn.execute_batch(
                "DROP TABLE IF EXISTS dag_jobs;
                 DROP TABLE IF EXISTS dag_resource_leases;
                 DROP TABLE IF EXISTS dag_units;
                 DROP TABLE IF EXISTS dag_plan_receipts;
                 DROP INDEX IF EXISTS dag_plan_receipts_status_idx;
                 DROP INDEX IF EXISTS dag_units_plan_idx;
                 DROP INDEX IF EXISTS dag_units_state_idx;
                 DROP INDEX IF EXISTS dag_resource_leases_resource_idx;
                 DROP INDEX IF EXISTS dag_resource_leases_plan_idx;
                 DROP INDEX IF EXISTS dag_jobs_plan_idx;
                 DROP INDEX IF EXISTS dag_jobs_unit_idx;",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 13_i64).unwrap();
        }

        // Phase 2: seed a representative v13 dataset — one wave row
        // (wave family must not be disturbed) plus one dag_plans row
        // (the v13 DAG family must survive the v14 upgrade).
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "INSERT INTO waves (wave_id, idempotency_key, kind, phase, expected_total, slot_retry_budget)
                   VALUES ('w-v13-legacy', 'idem-v13-legacy', 'exec', 'dispatch', 2, 1);
                 INSERT INTO dag_plans (plan_key, artifact_digest, target_branch, unit_ids, status, created_at_ms)
                   VALUES ('p-v13-legacy', 'digest-v13', 'feat/legacy', '[\"U1\"]', 'active', 1700000000000);",
            )
            .unwrap();
        }

        // Phase 3: reopen → v14 migration applies; every v13 row is
        // unchanged; the v14 tables exist and start empty.
        let conn = Connection::open(&path).unwrap();
        run(&conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), CURRENT_VERSION);

        for table in [
            "dag_plan_receipts",
            "dag_units",
            "dag_resource_leases",
            "dag_jobs",
        ] {
            let table_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(table_count, 1, "{table} must exist after upgrade");
            let row_count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(row_count, 0, "{table} starts empty for legacy DB");
        }

        // Legacy wave row preserved.
        let wave_kind: String = conn
            .query_row(
                "SELECT kind FROM waves WHERE wave_id = 'w-v13-legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(wave_kind, "exec", "wave row preserved across v14");

        // v13 DAG plan row preserved.
        let plan_status: String = conn
            .query_row(
                "SELECT status FROM dag_plans WHERE plan_key = 'p-v13-legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(plan_status, "active", "dag_plans row preserved across v14");
        let unit_ids: String = conn
            .query_row(
                "SELECT unit_ids FROM dag_plans WHERE plan_key = 'p-v13-legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            unit_ids, "[\"U1\"]",
            "dag_plans unit_ids preserved across v14"
        );
    }

    /// TGP-02 (PMI-013, P1, post-merge-converge 09-test-gap-plan
    /// §3): 「新 db + 旧二进制」的 downgrade 窗口必须双向
    /// fail-closed——db `user_version` 高于本二进制 ledger 尾部
    /// 时,`run` 必须显式拒绝,而不是静默返回 Ok 让旧代码在
    /// 未知 schema 上跑。
    ///
    /// 场景构造(按 test-gap 计划 TGP-02 steps): 一个已被新
    /// 二进制迁移到 v13 的 db(模拟: 跑完整 ledger 后把
    /// user_version 再推高一格到 CURRENT_VERSION+1,代表「未来
    /// v14 二进制写过、现被回退的当前二进制」),由当前二进制
    /// 重新打开。ledger 尾部 = CURRENT_VERSION(13),db 版本
    /// 14 > 13,循环条件 `current < migration.version` 对所有
    /// 条目为假——当前实现静默跳过全部迁移返回 Ok(())
    /// (PMI-013 缺陷形态)。
    ///
    /// **当前 RED**: 断言「高版本 db 必须被 typed 错误拒绝」,
    /// 现状实现返回 Ok(())。修复落地(run 循环加
    /// `current > CURRENT_VERSION` → fail-closed 错误)后本测试
    /// 转绿;错误消息按 PMI-013 expected 含版本语义(db N newer
    /// than binary max M)。
    ///
    /// invariant: schema 版本协商双向 fail-closed
    /// (旧 db+新二进制=迁移;新 db+旧二进制=拒绝)。
    #[test]
    fn tgp02_migration_run_rejects_db_newer_than_binary_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervisor.db");

        // Phase 1: 跑完整 ledger 到 CURRENT_VERSION(真实 v13 db
        // 的产生路径,非手写 DDL),再把 user_version 推到
        // CURRENT_VERSION+1——模拟「未来二进制已迁移、当前
        // 二进制被回退」打开的 db。
        {
            let conn = Connection::open(&path).unwrap();
            run(&conn).unwrap();
            assert_eq!(user_version(&conn).unwrap(), CURRENT_VERSION);
            conn.pragma_update(None, "user_version", CURRENT_VERSION + 1)
                .unwrap();
        }

        // Phase 2: 旧二进制(ledger 只到 CURRENT_VERSION)重开
        // 该 db。PMI-013 expected: fail-closed 拒绝。
        let conn = Connection::open(&path).unwrap();
        let outcome = run(&conn);
        let err = outcome.expect_err(
            "TGP-02: db user_version above the binary's ledger tail must be \
             rejected fail-closed, but `migrations::run` silently returned Ok",
        );

        // Typed 错误必须携带版本协商语义(两个版本号都可见),
        // 让操作者知道下一步是升级二进制或换 workspace。
        let msg = err.to_string();
        assert!(
            msg.contains(&(CURRENT_VERSION + 1).to_string())
                && msg.contains(&CURRENT_VERSION.to_string()),
            "TGP-02: rejection must carry both the db version ({}) and the \
             binary max ({}) for actionable diagnostics, got: {msg}",
            CURRENT_VERSION + 1,
            CURRENT_VERSION,
        );

        // 拒绝必须无副作用: user_version 保持 db 原值,不被
        // 任何迁移改写(旧二进制不得在被拒后留下指纹)。
        assert_eq!(
            user_version(&conn).unwrap(),
            CURRENT_VERSION + 1,
            "TGP-02: a rejected (newer-db) open must leave user_version \
             untouched at the db's own value"
        );
    }

    /// TGP-02 对照路径(半边): db 版本恰在 ledger 内且不小于
    /// 尾部(v == CURRENT_VERSION)时,run 幂等通过且版本不变
    /// ——确认修复加 above-version guard 时不会把「恰好当前
    /// 版本」的正常重开也误拒(边界是严格大于,不是 >=)。
    #[test]
    fn tgp02_migration_run_accepts_db_exactly_at_ledger_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervisor.db");
        {
            let conn = Connection::open(&path).unwrap();
            run(&conn).unwrap();
            assert_eq!(user_version(&conn).unwrap(), CURRENT_VERSION);
        }
        let conn = Connection::open(&path).unwrap();
        run(&conn).expect("reopen at exactly CURRENT_VERSION must pass");
        assert_eq!(
            user_version(&conn).unwrap(),
            CURRENT_VERSION,
            "idempotent reopen must keep the version at the ledger tail"
        );
    }
}
