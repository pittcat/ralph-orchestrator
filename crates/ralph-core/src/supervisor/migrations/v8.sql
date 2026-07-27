-- 2026-07-27-003 plan U5: replace the legacy `merged_to_events` /
-- `salvage_merged` boolean pair with a single `delivery_state`
-- column so the four-phase commit protocol
-- (Pending → BusinessProjected → SalvageCommitted →
-- CoordinationWritten → CoordinationCommitted) is forward-only
-- and survives a crash without a partial-success gap.
--
-- Migration semantics (per plan R12 / R10):
--   old_merged_to_events = 0 AND old_salvage_merged = 0 → 'pending'
--   old_merged_to_events = 0 AND old_salvage_merged = 1 → 'salvage_committed'
--   old_merged_to_events = 1 AND old_salvage_merged = 0 → 'pending' (REFUSED)
--     — invariant violation: coord-event injection cannot precede
--       the salvage merge. The runtime must rebuild the salvage
--       seam on recovery; the row is downgraded to Pending so the
--       dispatcher re-runs the merge and advances forward.
--   old_merged_to_events = 1 AND old_salvage_merged = 1 → 'coordination_committed'
--     — fully delivered under the old protocol.
--
-- Additional columns track the persisted receipt summaries so a
-- restart can verify the same receipt is replayed (and reject a
-- mismatched one as InvalidTransition).

ALTER TABLE waves ADD COLUMN delivery_state TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE waves ADD COLUMN salvage_fingerprint TEXT NOT NULL DEFAULT '';
ALTER TABLE waves ADD COLUMN salvage_write_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE waves ADD COLUMN salvage_already_present INTEGER NOT NULL DEFAULT 0;
ALTER TABLE waves ADD COLUMN salvage_committed_at INTEGER NOT NULL DEFAULT 0;

ALTER TABLE waves ADD COLUMN coordination_topic TEXT NOT NULL DEFAULT '';
ALTER TABLE waves ADD COLUMN coordination_idempotency_key TEXT NOT NULL DEFAULT '';
ALTER TABLE waves ADD COLUMN coordination_fingerprint TEXT NOT NULL DEFAULT '';
ALTER TABLE waves ADD COLUMN coordination_write_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE waves ADD COLUMN coordination_already_present INTEGER NOT NULL DEFAULT 0;
ALTER TABLE waves ADD COLUMN coordination_committed_at INTEGER NOT NULL DEFAULT 0;

-- Backfill: pick the highest safe phase based on the legacy
-- booleans. Illegal combinations (merged_to_events=1,
-- salvage_merged=0) collapse to 'pending' so the dispatcher
-- re-runs the merge seam on the next tick; the coord-event
-- injection can then advance legitimately.
UPDATE waves
   SET delivery_state = CASE
         WHEN merged_to_events = 1 AND salvage_merged = 1 THEN 'coordination_committed'
         WHEN merged_to_events = 0 AND salvage_merged = 1 THEN 'salvage_committed'
         ELSE 'pending'
       END;