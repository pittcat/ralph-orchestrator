-- 2026-07-28-002 plan U2 (R4 / R5 / R6 / S2a / S4 / S5):
-- persist slot activation descriptors for the redrive boot scan.
--
-- The table stores bounded activation descriptors for individual
-- slots of redrive child waves. The dispatcher writes a snapshot
-- of the ready event (topic + JSON payload + wave kind + digest)
-- at registration time; `ralph run --resume` consumes the same
-- descriptor to decide whether a worker can be spawned.
--
-- Key design decisions:
-- - wave_id + slot_index is the PRIMARY KEY (same as in-memory
--   HashMap key used by the memory store).
-- - slot_index_in_parent stores the ORIGINAL parent slot index
--   so the boot scan can join child → parent descriptor to derive
--   expected_digest for the parent → child mapping.
-- - payload_digest mirrors the SlotDescriptor.payload_digest field
--   so take_dispatchable_redrive_descriptor can do a strict digest
--   equality check without re-computing the fingerprint.

CREATE TABLE IF NOT EXISTS slot_descriptors (
    wave_id         TEXT NOT NULL,
    slot_index      INTEGER NOT NULL,
    slot_index_in_parent INTEGER,
    topic           TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    wave_kind       TEXT NOT NULL,
    payload_digest  TEXT NOT NULL,
    PRIMARY KEY (wave_id, slot_index)
);
