-- 2026-07-03-001 plan U4 / F-004 / R4: supervisor schema v2.
--
-- Adds the `wave_id_seq` autoincrement table used by
-- `register_wave` to allocate `wave_id`s atomically (one
-- row per `INSERT`, two concurrent callers cannot collide
-- on the same seq value). The pre-fix `SELECT COUNT(*) +
-- 1 FROM waves` allocator was racy because two callers
-- could observe the same count and insert a duplicate
-- `wave_id` PK.
--
-- The seq table has exactly one row; `next_value` is
-- initialised on first INSERT but stays a single row so
-- we don't accumulate empty inserts. We rely on the
-- `INTEGER PRIMARY KEY AUTOINCREMENT` semantics: every
-- successful INSERT yields a unique seq value.
--
-- Migration is idempotent (CREATE TABLE IF NOT EXISTS).

CREATE TABLE IF NOT EXISTS wave_id_seq (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    placeholder INTEGER
);

-- Seed row so the autoincrement starts at 1.
INSERT OR IGNORE INTO wave_id_seq (seq, placeholder) VALUES (0, 0);
