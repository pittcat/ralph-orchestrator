-- v24: parallel-forge DAG artifact handoff (U1, plan 2026-09-13-001)
-- Stores per-stage accepted + review-rejected completion artifact
-- paths from the prior hat's payload, so the next-stage hat can
-- pick up files via the durable DAG store instead of relying on
-- the JSONL log fan-out alone. Fail-soft semantics: a missing or
-- unsafe path is logged at the spawn seam and skipped, never
-- propagated as a migration error.

CREATE TABLE IF NOT EXISTS dag_stage_artifacts(
    plan_key TEXT NOT NULL,
    unit_key TEXT NOT NULL,
    stage TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    field_name TEXT NOT NULL,
    artifact_path TEXT NOT NULL,
    artifact_digest TEXT NOT NULL,
    recorded_at_ms INTEGER NOT NULL,
    PRIMARY KEY(plan_key, unit_key, stage, attempt, field_name)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_dag_stage_artifacts_lookup
    ON dag_stage_artifacts(plan_key, unit_key, stage, attempt);
