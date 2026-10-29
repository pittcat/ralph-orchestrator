-- Plan3-U3: durable monitor event stream.
--
-- `monitor_events` is the monitor's own durable log (not to be
-- confused with per-run `run_events`): every projection mutation
-- appends a row inside the same ingest transaction so the log and
-- the projection can never diverge (A3). `seq` is globally
-- AUTOINCREMENT — SSE clients resume by `?cursor=<seq>` and a cursor
-- below the retained window gets `resync_required`.
CREATE TABLE monitor_events (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,
  kind       TEXT NOT NULL,
  project_id TEXT,
  run_id     TEXT,
  payload    TEXT,
  at         TEXT NOT NULL
);
CREATE INDEX idx_monitor_events_kind ON monitor_events (kind);
CREATE INDEX idx_monitor_events_run ON monitor_events (project_id, run_id);
