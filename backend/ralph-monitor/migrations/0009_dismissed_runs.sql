-- Dismissed runs: DELETE /runs/:gid removes the projection rows AND
-- records the (project, run) pair here so the collector does not
-- re-import the run while on-disk evidence still exists (a stale
-- `.ralph/current-loop-id` or events file keeps reporting it). This
-- is a view-layer dismissal — the project's own .ralph evidence is
-- never touched. Project deletion cascades the marks: re-registering
-- a project revives whatever evidence remains.
CREATE TABLE IF NOT EXISTS dismissed_runs (
  project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  run_id        TEXT NOT NULL,
  dismissed_at  TEXT NOT NULL,
  PRIMARY KEY (project_id, run_id)
);
