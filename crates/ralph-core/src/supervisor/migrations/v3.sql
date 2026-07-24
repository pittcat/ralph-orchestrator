-- 2026-07-24-003 plan U4: supervisor schema v3 — emission reservation table.
--
-- Adds the `wave_emissions` table that backs `SupervisorStore`'s
-- emission reservation state machine. CLI `ralph wave emit` writes
-- here (U5) instead of the legacy `.idempotency.jsonl` sidecar.
--
-- Design notes:
--
-- - `scope_key` is the SHA-256 hex digest of
--   "<loop_id>|<hat>|<topic>|<idempotency_key>" — the dedup
--   dimension already established in `wave.rs::compute_scope_key`.
--   The `UNIQUE` constraint is the SQLite barrier that turns a
--   concurrent emit race into a clean `SQLITE_CONSTRAINT` error
--   the trait method can map to `AlreadyApplied` / `Conflict`.
-- - `payload_digest` is the SHA-256 hex digest of the canonical
--   payload list. Different payload under the same scope_key
--   becomes `Conflict` (the InMemory implementation enforces the
--   same invariant).
-- - `expected_count` is the number of events that should exist
--   in `events.jsonl` after Apply. A recovery scan finds N events
--   on disk → `Recovered`. 0 events → `FailedPartial` (fail-closed).
-- - `state` is one of `reserved`, `applying`, `applied`,
--   `recovery_required`, `failed`. Transitions are owned by
--   `reserve_emission` / `mark_emission_applying` /
--   `mark_emission_applied` / `mark_emission_recovery_required` /
--   `mark_emission_failed`.
-- - `public_wave_id` is the value the CLI returns to the agent
--   (`ralph wave emit` echoes it on success). It is allocated by
--   the store on first `reserve_emission` and reused on
--   `AlreadyApplied`. The `UNIQUE` constraint on `public_wave_id`
--   guarantees two parallel reserves cannot mint the same id.
-- - `applied_at` is the unix-second timestamp the row reached
--   `applied`. NULL while the row is `reserved` / `applying`.
-- - No foreign key to `waves`: emission is a separate concern
--   from runtime waves (U4 note in plan §5 U4). The
--   `public_wave_id` correlates the two surfaces without an
--   enforced referential link, which is the contract for the
--   CLI's emission side.

CREATE TABLE IF NOT EXISTS wave_emissions (
    scope_key        TEXT PRIMARY KEY,
    public_wave_id   TEXT NOT NULL UNIQUE,
    payload_digest   TEXT NOT NULL,
    expected_count   INTEGER NOT NULL CHECK (expected_count > 0),
    state            TEXT NOT NULL CHECK (state IN
                       ('reserved', 'applying', 'applied',
                        'recovery_required', 'failed')),
    reserved_at      INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    applied_at       INTEGER
);
CREATE INDEX IF NOT EXISTS idx_wave_emissions_state ON wave_emissions(state);