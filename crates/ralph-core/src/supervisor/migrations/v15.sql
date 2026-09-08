-- Persist the tested candidate BEFORE moving the target ref. Recovery can
-- distinguish a pending CAS from a completed CAS whose result write was lost.
CREATE TABLE IF NOT EXISTS dag_integration_intents (
    unit_id TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    base_commit TEXT NOT NULL,
    unit_commit TEXT NOT NULL,
    expected_head_before TEXT NOT NULL,
    integrated_commit TEXT NOT NULL,
    tree_oid TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY(unit_id, target_branch)
);
