-- Plan4-U1: persistent notification outbox + pending interactions.
--
-- `notifications.idem_key` is UNIQUE — the idempotency guarantee is
-- enforced by the DDL, not by app logic (D2): replaying an ingest
-- transaction can never create a second notification for the same
-- evidence. `deliveries` tracks per-channel state so in_app can land
-- delivered while webhook retries independently (D9). Browser rows
-- register state only — the actual dispatch happens page-side (D6).
CREATE TABLE notifications (
  id                     TEXT PRIMARY KEY,
  idem_key               TEXT UNIQUE NOT NULL,
  kind                   TEXT NOT NULL,
  project_id             TEXT NOT NULL,
  run_gid                TEXT,
  pending_interaction_id TEXT,
  title                  TEXT NOT NULL,
  body                   TEXT,
  detail                 TEXT,
  state                  TEXT NOT NULL DEFAULT 'unread',
  created_at             TEXT NOT NULL,
  resolved_at            TEXT
);
CREATE INDEX idx_notifications_state ON notifications (state, created_at DESC);
CREATE INDEX idx_notifications_run ON notifications (run_gid);
CREATE INDEX idx_notifications_pi ON notifications (pending_interaction_id);

CREATE TABLE deliveries (
  id              TEXT PRIMARY KEY,
  notification_id TEXT NOT NULL REFERENCES notifications(id),
  channel         TEXT NOT NULL,
  state           TEXT NOT NULL,
  attempts        INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT,
  delivery_id     TEXT,
  error           TEXT,
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL
);
CREATE INDEX idx_deliveries_notification ON deliveries (notification_id);
CREATE INDEX idx_deliveries_due ON deliveries (state, next_attempt_at);

CREATE TABLE pending_interactions (
  id          TEXT PRIMARY KEY,
  project_id  TEXT NOT NULL,
  session_id  TEXT NOT NULL,
  run_gid     TEXT,
  prompt      TEXT,
  status      TEXT NOT NULL,
  occurrence  TEXT,
  created_at  TEXT NOT NULL,
  resolved_at TEXT
);
CREATE UNIQUE INDEX idx_pending_session ON pending_interactions (project_id, session_id);
CREATE INDEX idx_pending_status ON pending_interactions (status);

CREATE TABLE notification_rules (
  id       TEXT PRIMARY KEY,
  version  INTEGER NOT NULL,
  kind     TEXT NOT NULL,
  matcher  TEXT NOT NULL,
  enabled  INTEGER NOT NULL DEFAULT 1
);

-- v1 rule set (D8): terminal transition split on reason==completed;
-- planning-session waiting evidence → interaction.pending.
INSERT INTO notification_rules (id, version, kind, matcher, enabled) VALUES
  ('run.terminal.completed', 1, 'run.completed',
   '{"on":"terminal","reasons":["completed"]}', 1),
  ('run.terminal.failed', 1, 'run.failed',
   '{"on":"terminal","reasons":["*"]}', 1),
  ('session.waiting_for_input', 1, 'interaction.pending',
   '{"on":"session_status","statuses":["waiting_for_input"]}', 1);
