-- v20: parallel-forge DAG P1 closure — terminal delivery state (U8)
-- Stores durable terminal-event delivery state for replay once / dedup.

CREATE TABLE IF NOT EXISTS dag_terminal_deliveries (
    -- Composite primary key
    plan_key TEXT NOT NULL,
    topic TEXT NOT NULL,
    delivery_key TEXT NOT NULL UNIQUE,

    -- Payload identity (sha64)
    artifact_digest TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    fixed_timestamp_ms INTEGER NOT NULL,

    -- File identity (canonical path) + offset tracking
    event_file_path TEXT NOT NULL,
    append_offset INTEGER NOT NULL DEFAULT 0,
    serialized_line_digest TEXT,

    -- State machine: prepared / appending / delivered / blocked
    state TEXT NOT NULL CHECK (state IN ('prepared', 'appending', 'delivered', 'blocked')),

    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    blocked_reason TEXT,

    PRIMARY KEY (plan_key, topic)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_dag_terminal_deliveries_state ON dag_terminal_deliveries(state);
CREATE INDEX IF NOT EXISTS idx_dag_terminal_deliveries_delivery_key ON dag_terminal_deliveries(delivery_key);