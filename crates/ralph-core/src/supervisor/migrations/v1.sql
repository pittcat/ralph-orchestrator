-- 2026-07-03-001 plan U5: supervisor schema v1.
--
-- The eight tables mirror the requirements doc's data model:
-- - waves:                one row per registered wave
-- - wave_slots:           per-slot lifecycle row
-- - slot_resources:       per-slot worktree binding (NULL for shared_readonly)
-- - dispatch_records:     per-(wave,slot) dispatch outcome (R-D1: UNIQUE)
-- - worker_results:       per-(wave,slot) content_hash + event_count (R-E1)
-- - wave_queue:           FIFO backpressure queue (R-A4)
-- - compensation_jobs:    per-wave compensation entries (R-F2)
-- (schema_migrations is not present — we use SQLite user_version.)

CREATE TABLE IF NOT EXISTS waves (
    wave_id           TEXT PRIMARY KEY,
    idempotency_key   TEXT NOT NULL UNIQUE,
    kind              TEXT NOT NULL,
    expected_total    INTEGER NOT NULL CHECK (expected_total > 0),
    phase             TEXT NOT NULL,
    cancel_requested  INTEGER NOT NULL DEFAULT 0,
    merged_to_events  INTEGER NOT NULL DEFAULT 0,
    created_at        INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    updated_at        INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);
CREATE INDEX IF NOT EXISTS idx_waves_phase ON waves(phase);

CREATE TABLE IF NOT EXISTS wave_slots (
    wave_id        TEXT NOT NULL REFERENCES waves(wave_id) ON DELETE CASCADE,
    slot_index     INTEGER NOT NULL,
    status         TEXT NOT NULL,
    isolation      TEXT NOT NULL,
    failure_reason TEXT,
    content_hash   TEXT,
    event_count    INTEGER,
    PRIMARY KEY (wave_id, slot_index)
);
CREATE INDEX IF NOT EXISTS idx_wave_slots_status ON wave_slots(wave_id, status);

CREATE TABLE IF NOT EXISTS slot_resources (
    wave_id        TEXT NOT NULL,
    slot_index     INTEGER NOT NULL,
    worktree_path  TEXT,
    branch         TEXT,
    PRIMARY KEY (wave_id, slot_index),
    FOREIGN KEY (wave_id, slot_index) REFERENCES wave_slots(wave_id, slot_index) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS dispatch_records (
    wave_id        TEXT NOT NULL,
    slot_index     INTEGER NOT NULL,
    pid            INTEGER,
    outcome        TEXT,
    dispatched_at  INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    UNIQUE (wave_id, slot_index)
);

CREATE TABLE IF NOT EXISTS worker_results (
    wave_id        TEXT NOT NULL,
    slot_index     INTEGER NOT NULL,
    content_hash   TEXT NOT NULL,
    event_count    INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    PRIMARY KEY (wave_id, slot_index),
    FOREIGN KEY (wave_id, slot_index) REFERENCES wave_slots(wave_id, slot_index) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS wave_queue (
    seq           INTEGER PRIMARY KEY AUTOINCREMENT,
    wave_id       TEXT NOT NULL UNIQUE REFERENCES waves(wave_id) ON DELETE CASCADE,
    enqueued_at   INTEGER NOT NULL DEFAULT (strftime('%s','now'))
);

CREATE TABLE IF NOT EXISTS compensation_jobs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    wave_id      TEXT NOT NULL REFERENCES waves(wave_id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    status       TEXT NOT NULL,
    enqueued_at  INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    executed_at  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_compensation_jobs_status ON compensation_jobs(status);
