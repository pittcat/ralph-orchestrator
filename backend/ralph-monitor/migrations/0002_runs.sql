-- 0002_runs.sql — runs projection + collector cursors.
--
-- feat(monitor) foundation plan (2026-10-28-1949) Unit 3.
-- `runs` is the SQLite projection of `ralph inspect run --format json`
-- snapshots; the external identity `global_run_id` is derived at the
-- API layer as `machine_id:project_id:run_id` (D1). The DB primary
-- key is `(project_id, run_id)` so identically-named runs in two
-- projects never collide.

CREATE TABLE IF NOT EXISTS runs (
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  run_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  state TEXT NOT NULL,
  state_evidence TEXT NOT NULL DEFAULT '{}',
  cwd TEXT,
  repo_root TEXT,
  worktree_path TEXT,
  prompt_preview TEXT,
  prompt_sha256 TEXT,
  preset TEXT,
  plan_path TEXT,
  started_at TEXT,
  terminal TEXT,
  last_event_at TEXT,
  last_observed_at TEXT NOT NULL,
  manifest_path TEXT,
  snapshot_sha256 TEXT,
  PRIMARY KEY (project_id, run_id)
);

CREATE INDEX IF NOT EXISTS runs_state_idx ON runs(state);
CREATE INDEX IF NOT EXISTS runs_started_idx ON runs(started_at);

CREATE TABLE IF NOT EXISTS source_cursors (
  project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  last_collected_at TEXT,
  last_snapshot_sha256 TEXT,
  consecutive_failures INTEGER NOT NULL DEFAULT 0
);

-- Bounded per-project collector error log (projection layer prunes
-- old rows; see run_projection.ts).
CREATE TABLE IF NOT EXISTS collection_errors (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  at TEXT NOT NULL,
  error TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS collection_errors_project_idx
  ON collection_errors(project_id, id);

-- Audit trail for destructive actions (project deletion cascades
-- runs/cursors/errors; the row survives the cascade).
CREATE TABLE IF NOT EXISTS audit_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  at TEXT NOT NULL,
  project_id TEXT,
  action TEXT NOT NULL,
  details TEXT
);
