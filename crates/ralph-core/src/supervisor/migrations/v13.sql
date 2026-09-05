-- PMI-006 / B2 plan 2026-09-03-0959 U3 (R2 / R17 / D4 / D17 / E5 /
-- E7 / E9): durable DAG state family. Until this migration the DAG
-- scheduler's plan registrations and integration records lived only
-- in process memory (`dag_store_memory.rs` / `dag_integration.rs`
-- InMemory variants), so the "exactly-once projection" and
-- "crash-window recovery" promises held only within a single
-- process lifetime. The two tables below give the rusqlite DAG
-- store (`dag_store_rusqlite.rs`) its schema:
--
--   dag_plans        — one row per registered canonical plan
--                      (plan_key unique; idempotent re-register on
--                      same (plan_key, artifact_digest); digest
--                      drift fails closed in the store layer).
--   dag_integrations— one row per (unit_id, target_branch)
--                      integration record (idempotent on the
--                      natural-key tuple; fingerprint drift and
--                      duplicate-unit-for-target fail closed in the
--                      store layer).
--
-- Both tables are additive: v12 DBs upgrade losslessly. They are
-- intentionally SEPARATE from the wave tables — the DAG scheduler
-- owns its state family; the wave store keeps its single authority
-- (04 audit single-authority principle). `unit_ids` is stored as a
-- JSON array TEXT column (bounded identity list; never raw
-- artifact bytes, per E9 / E16 receipt-content rules).
-- `created_at_ms` values are injected-clock epoch-ms from the
-- store callers; no SQL-side clock functions so tests stay
-- deterministic.

CREATE TABLE IF NOT EXISTS dag_plans (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_key        TEXT    NOT NULL UNIQUE,
    artifact_digest TEXT    NOT NULL,
    target_branch   TEXT    NOT NULL,
    unit_ids        TEXT    NOT NULL DEFAULT '[]',
    status          TEXT    NOT NULL DEFAULT 'pending',
    created_at_ms   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS dag_plans_status_idx
    ON dag_plans(status);

CREATE TABLE IF NOT EXISTS dag_integrations (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    unit_id              TEXT    NOT NULL,
    target_branch        TEXT    NOT NULL,
    base_commit          TEXT    NOT NULL,
    integrated_commit    TEXT    NOT NULL,
    expected_head_before TEXT    NOT NULL,
    commit_fingerprint   TEXT    NOT NULL,
    acked                INTEGER NOT NULL DEFAULT 0,
    created_at_ms        INTEGER NOT NULL,
    UNIQUE (unit_id, target_branch)
);

CREATE INDEX IF NOT EXISTS dag_integrations_target_idx
    ON dag_integrations(target_branch);
