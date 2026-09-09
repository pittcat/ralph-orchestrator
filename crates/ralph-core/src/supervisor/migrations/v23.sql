-- v23: parallel-forge DAG P1 closure — integration failure facts (U12)
-- Stores durable integration-failure facts tied to verify attempt + checkout generation.

CREATE TABLE IF NOT EXISTS dag_integration_failures (
    -- Composite primary key
    unit_key TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    generation INTEGER NOT NULL,

    -- Verify identity (the verify accepted attempt that produced the candidate)
    verify_job_id TEXT NOT NULL,
    verify_job_token TEXT NOT NULL,
    verify_attempt INTEGER NOT NULL,

    -- Failure classification
    failure_class TEXT NOT NULL CHECK (failure_class IN ('merge_conflict', 'gate_failure', 'foreign_target', 'unknown')),
    observation_path TEXT,
    observation_hash TEXT,

    -- Candidate reference (linked to v19 checkout intent)
    candidate_target_branch TEXT NOT NULL,
    candidate_generation INTEGER NOT NULL,

    -- Correction consumption tracking
    consumed_correction_digest TEXT,

    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,

    PRIMARY KEY (unit_key, attempt, generation)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_dag_integration_failures_unit ON dag_integration_failures(unit_key);
CREATE INDEX IF NOT EXISTS idx_dag_integration_failures_class ON dag_integration_failures(failure_class);
