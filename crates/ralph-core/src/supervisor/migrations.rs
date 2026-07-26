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
    #[allow(dead_code)] // pinned by `migrations_idempotent_across_reopen`; production writes via pragma_update
    pub const CURRENT_VERSION: i64 = 5;

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
                connection.execute_batch(migration.ddl)?;
                connection.pragma_update(None, "user_version", migration.version)?;
            }
        }
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
        &[
            Migration {
                version: 1,
                ddl: include_str!("migrations/v1.sql"),
            },
            Migration {
                version: 2,
                ddl: include_str!("migrations/v2.sql"),
            },
            Migration {
                version: 3,
                ddl: include_str!("migrations/v3.sql"),
            },
            Migration {
                version: 4,
                ddl: include_str!("migrations/v4.sql"),
            },
            Migration {
                version: 5,
                ddl: include_str!("migrations/v5.sql"),
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
}
