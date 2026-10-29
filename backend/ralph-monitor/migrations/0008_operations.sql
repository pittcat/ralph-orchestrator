-- Plan5-U2: durable operations machine + capability/preset caches.
--
-- `operations` doubles as the audit table (D7): actor (session id),
-- origin, params, and the typed receipt live on the same row. The
-- partial unique index enforces "one non-terminal operation per run"
-- (concurrency §15).
ALTER TABLE runs ADD COLUMN actions TEXT;
ALTER TABLE runs ADD COLUMN client_request_id TEXT;
ALTER TABLE runs ADD COLUMN events_file TEXT;
ALTER TABLE projects ADD COLUMN presets_cache TEXT;
ALTER TABLE projects ADD COLUMN presets_at TEXT;

CREATE TABLE operations (
  id               TEXT PRIMARY KEY,
  idempotency_key  TEXT NOT NULL UNIQUE,
  kind             TEXT NOT NULL
    CHECK (kind IN ('launch','stop','retry','merge','emit')),
  project_id       TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  run_gid          TEXT,
  client_request_id TEXT,
  params           TEXT NOT NULL DEFAULT '{}',
  expected_revision TEXT,
  state            TEXT NOT NULL DEFAULT 'recorded'
    CHECK (state IN ('recorded','dispatching','dispatched',
                     'confirmed','failed','confirm_timeout')),
  receipt          TEXT,
  actor            TEXT,
  origin           TEXT,
  error            TEXT,
  created_at       TEXT NOT NULL,
  dispatched_at    TEXT,
  confirmed_at     TEXT
);

CREATE UNIQUE INDEX operations_one_open_per_run
  ON operations(run_gid)
  WHERE run_gid IS NOT NULL AND state NOT IN ('confirmed','failed');
CREATE INDEX operations_created_at ON operations(created_at);

-- v1 rule: a confirmed operation produces an operation.confirmed
-- notification (S12) with a deep link into the run detail.
INSERT INTO notification_rules (id, version, kind, matcher, enabled) VALUES
  ('operation.confirmed', 1, 'operation.confirmed',
   '{"on":"operation_confirmed"}', 1);
