-- 2026-09-01-001 feat-forge-signal-delivery-reliability-plan U1
-- (R1 / D1): per-slot event payload ledger.
--
-- Persists the worker's accepted Event list (topic / payload /
-- source / wave envelope) immediately after `read_worker_events`
-- returns it but BEFORE the slot channel file is removed.
-- Crash recovery (U2 / U3) re-reads this table to redeliver
-- accepted slot events to the main ledger when fan-in was
-- interrupted by a loop process death.
--
-- The table is additive: v11 DBs upgrade losslessly. Each event
-- becomes one row (event_seq preserves source order). payload /
-- topic are stored as TEXT to mirror the JSONL wire format used
-- by the merge sink; rebuilding an `Event` from these columns
-- is straightforward (`Event::new(topic, payload)` plus
-- `with_source` / `with_wave`).
--
-- The wave envelope columns mirror `Event::with_wave(..)`:
-- source is the producing subsystem string, wave_id / wave_index
-- / wave_total round-trip the wave descriptor so the recovered
-- events look identical to a healthy fan-in's output. system_injected
-- is preserved so a recovered event keeps its system_injected
-- attribution, which `event_origin.rs:341` reads on the
-- dispatcher's main-ledger re-read path.

CREATE TABLE IF NOT EXISTS slot_event_payloads (
    wave_id          TEXT    NOT NULL,
    slot_index       INTEGER NOT NULL,
    attempt_seq      INTEGER NOT NULL,
    event_seq        INTEGER NOT NULL,
    topic            TEXT    NOT NULL,
    payload          TEXT    NOT NULL,
    source           TEXT,
    wave_index       INTEGER,
    wave_total       INTEGER,
    system_injected  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (wave_id, slot_index, attempt_seq, event_seq)
);

CREATE INDEX IF NOT EXISTS slot_event_payloads_wave_idx
    ON slot_event_payloads(wave_id);