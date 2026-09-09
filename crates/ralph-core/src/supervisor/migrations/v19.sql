-- v19: parallel-forge DAG P1 closure — target checkout intents (U4)
-- Stores durable target materialization state for CAS-then-checkout atomicity.

CREATE TABLE IF NOT EXISTS dag_checkout_intents (
    -- Composite primary key
    plan_key TEXT NOT NULL,
    unit_key TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    generation INTEGER NOT NULL,

    -- Identity (full SHA / hex)
    expected_head TEXT NOT NULL,
    candidate_head TEXT NOT NULL,
    candidate_tree TEXT NOT NULL,
    unit_commit TEXT NOT NULL,
    base_commit TEXT NOT NULL,

    -- Worktree canonical identity (canonicalized absolute path)
    worktree_path TEXT NOT NULL,
    worktree_identity TEXT NOT NULL,

    -- State machine: prepared / ref_advanced / materialized / superseded / blocked
    state TEXT NOT NULL CHECK (state IN ('prepared', 'ref_advanced', 'materialized', 'superseded', 'blocked')),

    -- Bounded metadata
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    blocked_reason TEXT,

    PRIMARY KEY (plan_key, unit_key, target_branch, generation)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_dag_checkout_intents_unit ON dag_checkout_intents(plan_key, unit_key);
CREATE INDEX IF NOT EXISTS idx_dag_checkout_intents_state ON dag_checkout_intents(state);
