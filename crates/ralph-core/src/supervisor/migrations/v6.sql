-- 2026-07-25-005 plan U2: slot attempt/retry data model.
--
-- New columns on `wave_slots`:
--   attempt_count  — how many times this slot has been dispatched
--                    (atomically incremented by record_slot_attempt)
--   max_attempts   — per-slot budget override (NULL = inherits wave-level
--                    slot_retry_budget). Range 0..=2; 0 disables auto-retry.
--
-- New columns on `waves`:
--   attempt_epoch  — child attempt wave's epoch marker (incremented each
--                    time a redrive wave is created from this wave)
--   parent_wave_id — redrive parent reference (NULL for original waves)
--   slot_retry_budget — default retry budget for all slots in this wave.
--                       Default 1 = one retry above the base dispatch.
--                       Range 0..=2; 0 disables.
--   published_failure_payload — R4 structured payload emitted flag.
--                    Set to 1 when the coordinator emits
--                    `*.wave.failed(reason=...)` with a structured payload
--                    so a redrive of the same wave does not re-emit.
--
-- Plan 004 (post-P0-2 hotfix) applies: two ralph CLI processes racing
-- to migrate a fresh DB would hit `duplicate column name` on the second
-- opener without the column-probe. The probe + ALTER is wrapped in a
-- single transaction so a concurrent opener that already saw the new
-- columns does not see a half-migrated schema.

-- wave_slots columns (added in v6)
ALTER TABLE wave_slots ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE wave_slots ADD COLUMN max_attempts INTEGER;  -- NULL = inherit wave-level budget

-- waves columns (added in v6)
ALTER TABLE waves ADD COLUMN attempt_epoch INTEGER NOT NULL DEFAULT 0;
ALTER TABLE waves ADD COLUMN parent_wave_id TEXT;  -- NULL for original waves
ALTER TABLE waves ADD COLUMN slot_retry_budget INTEGER NOT NULL DEFAULT 1 CHECK (slot_retry_budget >= 0 AND slot_retry_budget <= 2);
ALTER TABLE waves ADD COLUMN published_failure_payload INTEGER NOT NULL DEFAULT 0;
