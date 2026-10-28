-- 0001_init.sql — monitor foundation schema.
--
-- feat(monitor) foundation plan (2026-10-28-1949) Unit 2.
-- `projects` + `meta` + `schema_migrations` only; runs/cursors/
-- events/notifications/operations land in their own units.

CREATE TABLE IF NOT EXISTS schema_migrations (
  version TEXT PRIMARY KEY,
  applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  canonical_path TEXT NOT NULL UNIQUE,
  display_name TEXT NOT NULL,
  created_at TEXT NOT NULL
);
