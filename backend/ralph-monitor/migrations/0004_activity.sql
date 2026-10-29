-- Plan3-U2: backend-activity ledger projection + health evidence.
--
-- `run_activity` projects `.ralph/runs/<run_id>.activity.jsonl`
-- start/end pairs written by the runtime (Plan3-U1). seq is the
-- file's own seq_in_run (already per-run monotonic); the
-- (file, byte_offset, generation) unique index dedupes replays the
-- same way run_events does — a truncated/rewritten ledger bumps its
-- cursor generation instead of colliding on offsets.
CREATE TABLE run_activity (
  project_id  TEXT NOT NULL,
  run_id      TEXT NOT NULL,
  seq         INTEGER NOT NULL,
  file        TEXT NOT NULL,
  byte_offset INTEGER NOT NULL,
  generation  INTEGER NOT NULL DEFAULT 0,
  phase       TEXT NOT NULL,
  kind        TEXT NOT NULL DEFAULT 'backend_call',
  at          TEXT,
  detail      TEXT,
  PRIMARY KEY (project_id, run_id, generation, seq)
);
CREATE UNIQUE INDEX idx_run_activity_source
  ON run_activity (project_id, run_id, file, byte_offset, generation);
CREATE INDEX idx_run_activity_phase
  ON run_activity (project_id, run_id, phase);

-- Byte position per activity ledger per run (same contract as
-- event_cursors: shrink/rewrite → generation++ and re-read from 0).
CREATE TABLE activity_cursors (
  project_id  TEXT NOT NULL,
  run_id      TEXT NOT NULL,
  file        TEXT NOT NULL,
  byte_offset INTEGER NOT NULL DEFAULT 0,
  generation  INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (project_id, run_id, file)
);

-- Health classification reads the pid and resolves the activity
-- ledger path from the stored workspace — both were already captured
-- in the snapshot but never persisted (Plan3-U1 added the
-- `activity_file` observation field).
ALTER TABLE runs ADD COLUMN pid INTEGER;
ALTER TABLE runs ADD COLUMN workspace TEXT;
ALTER TABLE runs ADD COLUMN activity_file TEXT;
