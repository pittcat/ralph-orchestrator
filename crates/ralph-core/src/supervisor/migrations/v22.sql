-- v22: parallel-forge DAG P1 closure — correction request ledger (U11)
-- Stores durable execute-failure correction requests with bounded feedback path.

CREATE TABLE IF NOT EXISTS dag_correction_requests (
    -- Composite primary key
    unit_key TEXT NOT NULL,
    failure_fingerprint TEXT NOT NULL,

    -- Identity / fingerprint
    failed_job_id TEXT NOT NULL,
    failed_job_token TEXT NOT NULL,
    failed_attempt INTEGER NOT NULL,
    correction_digest TEXT NOT NULL,

    -- Bounded feedback
    feedback_path TEXT NOT NULL,
    feedback_hash TEXT NOT NULL,

    -- State machine
    state TEXT NOT NULL CHECK (state IN ('pending', 'reserved', 'blocked')),
    reserved_job_id TEXT,
    reserved_attempt INTEGER,

    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    blocked_reason TEXT,

    PRIMARY KEY (unit_key, failure_fingerprint)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_dag_correction_requests_state ON dag_correction_requests(state);
CREATE INDEX IF NOT EXISTS idx_dag_correction_requests_unit ON dag_correction_requests(unit_key);