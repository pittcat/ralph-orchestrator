-- Plan2-U2: run event projection + per-source byte cursors.
--
-- run_events.seq is per-(project_id,run_id) monotonic and never
-- reused; the (file, byte_offset) unique index makes a replayed byte
-- range idempotent — re-ingesting the same bytes cannot duplicate
-- rows even though new seq values are minted.
CREATE TABLE run_events (
  project_id  TEXT NOT NULL,
  run_id      TEXT NOT NULL,
  seq         INTEGER NOT NULL,
  file        TEXT NOT NULL,
  byte_offset INTEGER NOT NULL,
  generation  INTEGER NOT NULL DEFAULT 0,
  ts          TEXT,
  iteration   INTEGER,
  hat         TEXT,
  topic       TEXT,
  payload     TEXT,
  truncated   INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (project_id, run_id, seq)
);
-- Dedupe anchor: one row per source byte *generation* — a file that
-- was truncated/rewritten bumps its cursor generation so re-read
-- bytes at the same offsets do not collide with the old generation.
CREATE UNIQUE INDEX idx_run_events_source
  ON run_events (project_id, run_id, file, byte_offset, generation);
CREATE INDEX idx_run_events_topic
  ON run_events (project_id, run_id, topic);
CREATE INDEX idx_run_events_iteration
  ON run_events (project_id, run_id, iteration);

-- Byte position per source file per run; a file that shrank below
-- its cursor was truncated/rewritten → cursor resets to 0 and the
-- run_evidence.source_resets counter records the reset.
CREATE TABLE event_cursors (
  project_id  TEXT NOT NULL,
  run_id      TEXT NOT NULL,
  file        TEXT NOT NULL,
  byte_offset INTEGER NOT NULL DEFAULT 0,
  generation  INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (project_id, run_id, file)
);

-- Per-run ingest diagnostics.
CREATE TABLE run_evidence (
  project_id    TEXT NOT NULL,
  run_id        TEXT NOT NULL,
  parse_errors  INTEGER NOT NULL DEFAULT 0,
  source_resets INTEGER NOT NULL DEFAULT 0,
  updated_at    TEXT NOT NULL,
  PRIMARY KEY (project_id, run_id)
);

-- Descriptor/scheduler passthrough captured from the snapshot so
-- /progress and /graph can derive without re-resolving presets.
ALTER TABLE runs ADD COLUMN preset_descriptor TEXT;
ALTER TABLE runs ADD COLUMN scheduler TEXT;
