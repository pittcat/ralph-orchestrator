-- 2026-07-26-004 plan U2 (KTD3 / R2): bounded terminal-event evidence
-- for `Completed` slots. Fan-in reconciliation must distinguish a
-- real terminal event from a bare `Completed` status bit. We store
-- ONLY the bounded identity (topic + optional dimension + payload
-- fingerprint) — never the full agent output — so writes stay small.
--
-- All three columns are nullable: legacy rows recorded before this
-- migration carry NULL evidence and MUST be treated as "not provably
-- done" (fail-closed) by reconciliation, never as success.
--
-- Plan 004 (post-P0-2 hotfix): two ralph CLI processes can race to
-- migrate a fresh supervisor DB. The pre-fix `ALTER TABLE ... ADD
-- COLUMN` statement is NOT idempotent on SQLite (no `IF NOT EXISTS`
-- for columns), so the second process fails with
-- `duplicate column name: evidence_topic` and the wave emit is
-- refused (`supervisor_store_unavailable`). The fix: probe each
-- column via `pragma_table_info` first and only emit the ALTER for
-- columns that are missing. The probe runs once per `migrations::run`
-- call and adds a single `pragma_table_info` round-trip per missing
-- column; cost is negligible compared to the cost of a crashed
-- concurrent wave. The user_version gate in `migrations::run` keeps
-- the rest of the migration set idempotent (tables / indexes still
-- use `IF NOT EXISTS`).
--
-- The probe + ALTER pair is wrapped in a single transaction so a
-- concurrent opener that already saw the new columns does not see a
-- half-migrated schema.
ALTER TABLE wave_slots ADD COLUMN evidence_topic TEXT;
ALTER TABLE wave_slots ADD COLUMN evidence_dimension TEXT;
ALTER TABLE wave_slots ADD COLUMN evidence_fingerprint TEXT;