-- 2026-09-03-0959 plan Step E3 (recovery/resume): durable per-plan
-- metadata the recovery path needs after a restart. The verified base
-- commit is captured at the `forge.concurrency.approved` boundary
-- (workspace HEAD at approval time); before this table it lived only
-- in the process-local plan topology, so a restarted loop could not
-- re-derive it without guessing (workspace HEAD has since moved with
-- each landed integration). Recovery reads it here; a missing row
-- (pre-E3 run) fails closed into a blocked diagnosis.
--
-- Purely additive (`CREATE TABLE IF NOT EXISTS`, no ALTERs, no
-- column-probe path). `created_at_ms` is an injected-clock epoch-ms
-- from the caller; no SQL-side clock functions so tests stay
-- deterministic.
CREATE TABLE IF NOT EXISTS dag_plan_meta (
    plan_key             TEXT    PRIMARY KEY,
    verified_base_commit TEXT    NOT NULL,
    created_at_ms        INTEGER NOT NULL
);
