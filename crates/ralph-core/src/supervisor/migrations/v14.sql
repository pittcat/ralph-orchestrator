-- PMI-013 / 2026-09-03-0959 plan Step 0 (U3 residual / R10 / R17 /
-- S12 / S20): durable registration receipt + DAG job/lease/unit
-- persistence tables.
--
-- Until this migration the DAG plan registration receipt lived only
-- in a process-local `Mutex<HashMap>` (`dag_plan_receipt.rs`), so a
-- crash between the `forge.plan.ready` accepted boundary and task
-- projection / ack lost the plan identity. The v13 schema covered
-- `dag_plans` / `dag_integrations` only. This migration is purely
-- additive (`CREATE TABLE IF NOT EXISTS`, no ALTERs, no column-probe
-- path) and touches no wave-era table:
--
--   dag_plan_receipts   — one row per bounded registration receipt,
--                         keyed by plan_key. Status lifecycle:
--                         pending (recorded at the accepted boundary)
--                         → active (accepted approval activated it)
--                         → consumed (recovery/projection used it).
--                         Same (plan_key, artifact_digest) re-record
--                         is an idempotent no-op; digest drift fails
--                         closed in the store layer (DigestConflict).
--   dag_units           — per-unit lifecycle state machine row
--                         (registered/ready/executing/reviewing/
--                         verifying/integration_queued/integrating/
--                         integrated/correction_requested/blocked/
--                         failed per plan §1.4 + state diagram).
--                         `stage` / `attempt` / `current_token` pin
--                         the live attempt token tuple
--                         (unit_key, job_id, hat, stage, attempt)
--                         that jobs.rs mints and recovery.rs fences.
--   dag_resource_leases — typed capacity+permits lease (D6/D21):
--                         owner unit holds (resource_key, permits)
--                         from Ready→Executing through
--                         review/verify/correction/integration and
--                         releases only at Integrated/Blocked/
--                         Failed/cancellation (idempotent release).
--                         UNIQUE (unit_key, resource_key) keeps lease
--                         owner uniqueness at the schema level.
--   dag_jobs            — one row per kernel invocation attempt.
--                         UNIQUE (unit_key, stage, attempt) enforces
--                         job attempt uniqueness (plan U3 #11);
--                         pid/launched_at_ms record launch facts
--                         (D20: raw launch is at-least-once; the
--                         durable attempt row + token fencing is
--                         what makes stale processes effect-free);
--                         terminal_state/result_digest record the
--                         bounded terminal fact (D24: typed fields +
--                         digest only, never raw payload bytes).
--
-- `created_at_ms` / `updated_at_ms` / `acquired_at_ms` /
-- `launched_at_ms` values are injected-clock epoch-ms from the
-- store callers; no SQL-side clock functions so tests stay
-- deterministic. Indexes cover the recovery / inspect query shape:
-- by plan_key and by unit_key.

CREATE TABLE IF NOT EXISTS dag_plan_receipts (
    plan_key        TEXT    PRIMARY KEY,
    artifact_path   TEXT    NOT NULL,
    artifact_digest TEXT    NOT NULL,
    target_branch   TEXT    NOT NULL,
    status          TEXT    NOT NULL DEFAULT 'pending',
    created_at_ms   INTEGER NOT NULL,
    activated_at_ms INTEGER,
    consumed_at_ms  INTEGER
);

CREATE INDEX IF NOT EXISTS dag_plan_receipts_status_idx
    ON dag_plan_receipts(status);

CREATE TABLE IF NOT EXISTS dag_units (
    unit_key          TEXT    PRIMARY KEY,
    plan_key          TEXT    NOT NULL,
    state             TEXT    NOT NULL DEFAULT 'registered',
    stage             TEXT,
    hat               TEXT,
    job_id            TEXT,
    attempt           INTEGER NOT NULL DEFAULT 0,
    current_token     TEXT,
    integration_order INTEGER NOT NULL DEFAULT 0,
    depends_on        TEXT    NOT NULL DEFAULT '[]',
    created_at_ms     INTEGER NOT NULL,
    updated_at_ms     INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS dag_units_plan_idx
    ON dag_units(plan_key);

CREATE INDEX IF NOT EXISTS dag_units_state_idx
    ON dag_units(state);

CREATE TABLE IF NOT EXISTS dag_resource_leases (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    plan_key       TEXT    NOT NULL,
    unit_key       TEXT    NOT NULL,
    resource_key   TEXT    NOT NULL,
    permits        INTEGER NOT NULL,
    status         TEXT    NOT NULL DEFAULT 'held',
    acquired_at_ms INTEGER NOT NULL,
    released_at_ms INTEGER,
    UNIQUE (unit_key, resource_key)
);

CREATE INDEX IF NOT EXISTS dag_resource_leases_resource_idx
    ON dag_resource_leases(resource_key, status);

CREATE INDEX IF NOT EXISTS dag_resource_leases_plan_idx
    ON dag_resource_leases(plan_key);

CREATE TABLE IF NOT EXISTS dag_jobs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id          TEXT    NOT NULL,
    plan_key        TEXT    NOT NULL,
    unit_key        TEXT    NOT NULL,
    hat             TEXT    NOT NULL,
    stage           TEXT    NOT NULL,
    attempt         INTEGER NOT NULL,
    token           TEXT,
    pid             INTEGER,
    launched_at_ms  INTEGER,
    finished_at_ms  INTEGER,
    terminal_state  TEXT,
    result_digest   TEXT,
    created_at_ms   INTEGER NOT NULL,
    UNIQUE (unit_key, stage, attempt)
);

CREATE INDEX IF NOT EXISTS dag_jobs_plan_idx
    ON dag_jobs(plan_key);

CREATE INDEX IF NOT EXISTS dag_jobs_unit_idx
    ON dag_jobs(unit_key);
