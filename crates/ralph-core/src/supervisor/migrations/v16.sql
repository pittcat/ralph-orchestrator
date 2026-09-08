-- 2026-09-03-0959 plan Step E2 (S15): exactly-once fence for
-- runtime-emitted terminal coordination events in dag mode
-- (`forge.exec.development.done`). The PRIMARY KEY on
-- (plan_key, topic) makes a duplicate emit attempt a no-op at the
-- schema level; `try_record_terminal_emit` reads the insert
-- outcome as the emit permit. `created_at_ms` is an injected-clock
-- epoch-ms from the caller; no SQL-side clock functions so tests
-- stay deterministic.
CREATE TABLE IF NOT EXISTS dag_terminal_emits (
    plan_key        TEXT    NOT NULL,
    topic           TEXT    NOT NULL,
    idempotency_key TEXT    NOT NULL,
    created_at_ms   INTEGER NOT NULL,
    PRIMARY KEY (plan_key, topic)
);
