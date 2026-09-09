-- 2026-09-09-0917-fix-forge-dag-p1-closure-plan U3 (Per-Unit
-- stage evidence and committed-output handover): durable
-- per-Unit base commit pinning + per-stage accepted evidence
-- ledger so reviewer / verifier / fixer resume from the
-- exact HEAD the prior stage committed and accepted, without
-- resetting base.
--
-- Until this migration the cross-stage hand-off relied on
-- the operator worktree's live HEAD and the plan-level
-- verified-base recorded in `dag_plan_meta` (v17). A
-- reviewer / verifier / fixer could read from a stale HEAD
-- (something else advanced the worktree after the prior
-- stage accepted), could reset base to plan HEAD instead of
-- building on the just-accepted commit, or could land
-- committed work on a different repository. U3 binds the
-- next stage's read to the prior stage's accepted evidence.
--
-- Purely additive (`CREATE TABLE IF NOT EXISTS`, no ALTERs,
-- no column-probe path). `pinned_at_ms` / `accepted_at_ms`
-- are injected-clock epoch-ms from the caller; no SQL-side
-- clock functions so tests stay deterministic.
--
--   dag_unit_bases     — one row per `(plan_key, unit_key)`
--                        pin. Records the base commit the
--                        Unit's first admission took from
--                        the operator worktree; subsequent
--                        stages build on the prior stage's
--                        accepted commit, never on plan
--                        HEAD. PRIMARY KEY
--                        `(plan_key, unit_key)` keeps the
--                        pin idempotent at the schema
--                        level; a re-pin with the same base
--                        is a no-op, a different base fails
--                        closed in the store layer.
--   dag_stage_evidence — one row per
--                        `(plan_key, unit_key, stage, attempt)`.
--                        Records the accepted commit head,
--                        the base it descended from, and an
--                        `evidence_token` +
--                        `evidence_fingerprint` binding so a
--                        resume of the next stage can verify
--                        identity before continuing. PRIMARY
--                        KEY `(plan_key, unit_key, stage,
--                        attempt)` keeps the evidence
--                        idempotent at the schema level.

CREATE TABLE IF NOT EXISTS dag_unit_bases (
    plan_key     TEXT    NOT NULL,
    unit_key     TEXT    NOT NULL,
    base_commit  TEXT    NOT NULL,
    pinned_at_ms INTEGER NOT NULL,
    PRIMARY KEY (plan_key, unit_key)
);

CREATE TABLE IF NOT EXISTS dag_stage_evidence (
    plan_key             TEXT    NOT NULL,
    unit_key             TEXT    NOT NULL,
    stage                TEXT    NOT NULL,
    attempt              INTEGER NOT NULL,
    accepted_commit      TEXT    NOT NULL,
    base_commit          TEXT    NOT NULL,
    evidence_token       TEXT    NOT NULL,
    evidence_fingerprint TEXT    NOT NULL,
    accepted_at_ms       INTEGER NOT NULL,
    PRIMARY KEY (plan_key, unit_key, stage, attempt)
);

CREATE INDEX IF NOT EXISTS dag_unit_bases_plan_idx
    ON dag_unit_bases(plan_key);

CREATE INDEX IF NOT EXISTS dag_stage_evidence_plan_idx
    ON dag_stage_evidence(plan_key);

CREATE INDEX IF NOT EXISTS dag_stage_evidence_unit_idx
    ON dag_stage_evidence(unit_key);
