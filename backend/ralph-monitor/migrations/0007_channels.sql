-- Plan4-U2: notification channel config + runtime-webhook hint.
--
-- `channel_config` is keyed by channel name (v1 single 'webhook'
-- endpoint — schema is per-channel JSON so a second channel needs no
-- migration). The secret lives here but is masked on every API read
-- (KTD4). `projects.runtime_webhook_hint` mirrors the read-only
-- `ralph.yml notifications.enabled` probe (D7): collector refreshes
-- it each round; the UI warns about possible duplicate notifications.
CREATE TABLE channel_config (
  channel    TEXT PRIMARY KEY,
  config     TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

ALTER TABLE projects ADD COLUMN runtime_webhook_hint TEXT DEFAULT 'none';
