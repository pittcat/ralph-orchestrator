-- 2026-08-07-009 plan U1 (R1 / R2 / KTD3-KTD4 / KTD11): per-slot
-- attempt receipt ledger.
--
-- The dispatcher writes a `running` row in `BEGIN IMMEDIATE` so
-- concurrent threads converge on a unique `attempt_seq`; on finish
-- the row advances to `succeeded` or `failed` and the start/end
-- Git checkpoints are recorded.
--
-- The table is additive: v10 DBs upgrade losslessly. `wave_id`
-- has no FOREIGN KEY because `waves.wave_id` is not the literal
-- `PRIMARY KEY` of the legacy `waves` table (the row PK is an
-- internal autoincrement id and `wave_id` is UNIQUE). We mirror
-- the existing `slot_descriptors` style and keep the column
-- constraint as NOT NULL + matching UNIQUE INDEX for upsert.

CREATE TABLE IF NOT EXISTS slot_attempts (
    wave_id              TEXT    NOT NULL,
    slot_index           INTEGER NOT NULL,
    attempt_seq          INTEGER NOT NULL,
    status               TEXT    NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
    started_at_unix_ms   INTEGER NOT NULL DEFAULT 0,
    finished_at_unix_ms  INTEGER NOT NULL DEFAULT 0,
    start_head_sha       TEXT,
    start_dirty          INTEGER,  -- NULL = unknown / helper unavailable
    end_head_sha         TEXT,
    end_dirty            INTEGER,
    failure_code         TEXT,
    PRIMARY KEY (wave_id, slot_index, attempt_seq)
);

CREATE INDEX IF NOT EXISTS slot_attempts_wave_slot_idx
    ON slot_attempts(wave_id, slot_index, attempt_seq);