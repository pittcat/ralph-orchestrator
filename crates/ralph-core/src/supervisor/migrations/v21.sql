-- v21: parallel-forge DAG P1 closure — DAG plan registration evidence (U9)
-- Stores durable candidate receipt + accepted evidence for DAG plan-ready.

CREATE TABLE IF NOT EXISTS dag_registration_evidence (
    -- Composite primary key
    plan_key TEXT NOT NULL,
    loop_id TEXT NOT NULL,

    -- Candidate / accepted evidence
    source_hat TEXT NOT NULL,
    contract_revision TEXT NOT NULL,
    artifact_path TEXT NOT NULL,
    artifact_digest TEXT NOT NULL,
    accepted_transition_id TEXT,
    projection_complete INTEGER NOT NULL DEFAULT 0,
    receipt_state TEXT NOT NULL CHECK (receipt_state IN ('candidate', 'accepted', 'rejected')),

    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,

    PRIMARY KEY (plan_key, loop_id)
) STRICT;

CREATE TABLE IF NOT EXISTS dag_approval_evidence (
    -- Composite primary key
    plan_key TEXT NOT NULL,
    loop_id TEXT NOT NULL,
    approval_digest TEXT NOT NULL,

    -- Approval details
    target_branch TEXT NOT NULL,
    approved_base TEXT NOT NULL,
    accepted_transition_id TEXT,

    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,

    PRIMARY KEY (plan_key, loop_id, approval_digest)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_dag_registration_evidence_state ON dag_registration_evidence(receipt_state);
CREATE INDEX IF NOT EXISTS idx_dag_approval_evidence_plan ON dag_approval_evidence(plan_key);