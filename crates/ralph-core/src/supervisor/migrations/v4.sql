-- 2026-07-26-004 plan U2 (KTD3 / R2): bounded terminal-event evidence
-- for `Completed` slots. Fan-in reconciliation must distinguish a
-- real terminal event from a bare `Completed` status bit. We store
-- ONLY the bounded identity (topic + optional dimension + payload
-- fingerprint) — never the full agent output — so writes stay small.
--
-- All three columns are nullable: legacy rows recorded before this
-- migration carry NULL evidence and MUST be treated as "not provably
-- done" (fail-closed) by reconciliation, never as success.
--
-- Idempotency across re-opens is guaranteed by the user_version gate
-- in `migrations::run` (this step runs once, at version 4); SQLite
-- `ALTER TABLE ... ADD COLUMN` is not itself re-runnable.
ALTER TABLE wave_slots ADD COLUMN evidence_topic TEXT;
ALTER TABLE wave_slots ADD COLUMN evidence_dimension TEXT;
ALTER TABLE wave_slots ADD COLUMN evidence_fingerprint TEXT;
