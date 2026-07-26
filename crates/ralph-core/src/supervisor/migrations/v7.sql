-- 2026-07-25-005 plan U4 (R1 / R2 / R4): idempotent redrive request ledger.
--
-- The triple (parent_wave_id, slot_index, attempt_epoch) is unique-constrained
-- so concurrent redrive requests for the same slot converge on a single row
-- instead of creating duplicates.
--
-- Status values (CHECK constraint):
--   pending            — child wave has been or will be spawned
--   applied            — child wave was already spawned from this triple
--   rejected_duplicate — same triple was already recorded (Pending or Applied)
--   rejected_terminal  — slot is Completed; redrive is not valid
--
-- The table does NOT track which child wave was spawned (that belongs to
-- the child wave's own row, which links back via parent_wave_id in v6).
-- This table is purely the idempotency ledger for redrive requests.

CREATE TABLE IF NOT EXISTS redrive_requests (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_wave_id TEXT NOT NULL,
    slot_index    INTEGER NOT NULL,
    attempt_epoch INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    status        TEXT NOT NULL CHECK (status IN (
        'pending',
        'applied',
        'rejected_duplicate',
        'rejected_terminal'
    )),
    UNIQUE(parent_wave_id, slot_index, attempt_epoch)
);

-- Index for listing all redrive requests for a given parent wave.
-- (The UNIQUE constraint already implies an index on the 3 columns,
-- but a covering index on parent_wave_id alone speeds up the
-- list_redrive_requests query without the payload overhead.)
CREATE INDEX IF NOT EXISTS idx_redrive_parent
    ON redrive_requests(parent_wave_id);
