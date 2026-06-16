---
title: "fix: Reject forged future timestamps at events.jsonl write/read boundary"
status: active
created: 2026-06-18
origin: empirical — observed 2026-06-17 in worktree `2026-06-10-003-...-sunny-lotus` events.jsonl lines 17/19
type: fix
---

# fix: Reject forged future timestamps at events.jsonl write/read boundary

## Summary

Two events in the worktree's `events.jsonl` carried `timestamp: "2026-06-17T00:38:00.000000000Z"` and `timestamp: "2026-06-17T00:00:00Z"` — eight hours in the future relative to every other event in the same file (16:22–17:11 UTC on 2026-06-16). The reader silently dropped the `timestamp` field (Event struct only knows `ts`), leaving `ts` as the empty default string. No mechanism in the codebase rejects forged-future or out-of-window timestamps. The fix is the smallest possible hardening: a single read-side timestamp-window check in `EventReader::read_new_events`, plus a `serde(alias = "timestamp")` on the `ts` field so the offending lines actually surface in the validator rather than vanishing into `default`.

This is the minimum-touch fix for the bug the user observed. It does **not** touch the line 22/23 wave_dispatcher placeholder path, the `task_id=u1-scaffold-001` worker self-fabrication, or the `dimension-reviewer.publishes` scope — those are out of scope here.

## Problem Frame

`events.jsonl` is the canonical event stream consumed by the orchestrator. The on-disk schema accepted by `EventReader::Event` (in `crates/ralph-core/src/event_reader.rs`) is:

```text
{ topic, payload?, ts, hat?, triggered?, source?, wave_id?, wave_index?, wave_total? }
```

The `ts` field is a `String` deserialized with `#[serde(default)]` — empty string is silently accepted. There is no validator on the *value* of `ts`; any RFC3339-shaped string (or empty, or any garbage) parses fine. The three write paths in production code all stamp `ts` from `chrono::Utc::now()`, so under normal operation timestamps are always real. But the schema is defenseless against:

1. A producer using a different field name (`timestamp` instead of `ts`).
2. A producer stamping a future or far-past time (e.g., a test fixture, a hand-edited file, or a malicious payload).
3. A producer stamping a non-RFC3339 string (e.g., `T+0`, `0`).

The observed symptom: two events (lines 17, 19 of the worktree file) have `timestamp: "2026-06-17T00:00:00Z"` / `"2026-06-17T00:38:00.000000000Z"`, which a human reading the file notices as "in the future." The mechanism doesn't notice because the field name doesn't match `ts`, so the offending lines enter the event stream with an empty `ts` and continue to be processed as normal business events.

## Causal Chain (root cause → observable symptom)

```text
[producer writes event with field "timestamp" instead of "ts"]
    │
    ▼
[EventReader::Event (crates/ralph-core/src/event_reader.rs:88-123)]
    ts: String with #[serde(default)] — no alias
    │
    ▼
[serde silently drops "timestamp"; ts := ""]
    │
    ▼
[No value-side check on ts; reader pushes event to result.events]
    │
    ▼
[Event flows into EventBus with empty ts]
    │
    ▼
[Downstream consumers can't tell the event was forged/injected;
 wave_tracker sees 7/7 review.dimension.done with 1 invisible failure]
    │
    ▼
[review-synthesizer never dispatches; loop enters human.guidance hard-gate]
    │
    ▼
[Human reads events.jsonl, sees "2026-06-17T00:00:00Z" line 19, asks "is this real?"]
```

For the specific observed line 19 (`agent-native` dimension, `findings_count: 0`, payload structurally complete): the field-name drop means the event is *indistinguishable* from a real one in the reader's output. The only signal is the JSON text a human reads.

## Why a write-side guard alone is insufficient

The three production write paths (verified by `rg "ts.*Utc|Utc::now"` in `crates/ralph-cli/src/commands/emit.rs:585` and `crates/ralph-cli/src/wave.rs:523,1144,1155`) all compute `ts = chrono::Utc::now().to_rfc3339()` immediately before building the record. A future timestamp cannot originate from these paths. The offending lines must have been written by a non-Ralph producer (test fixture, hand-edit, replay script, or smoke session). A write-side guard inside `emit` / `write_wave_events` would not catch them — they bypass the CLI entirely.

The only place every event in `events.jsonl` must pass through is `EventReader::read_new_events`. That is the right enforcement point.

## Goal

Forged or out-of-window `ts` values in `events.jsonl` are detected at read time, converted to `MalformedLine` entries (with a precise error string the operator can grep), and excluded from `result.events`. The existing CLI / loop behavior of routing `malformed` to `event.malformed` backpressure continues to work unchanged.

The `Event` struct accepts both `ts` and `timestamp` as field names for the timestamp, so producer variance in field naming does not silently lose the value into a `default()` empty string.

## Requirements

- R1: `read_new_events` rejects events whose `ts` (after alias resolution) parses as RFC3339 and is more than `5 minutes` in the future relative to wall clock at read time. The event is recorded as a `MalformedLine` with `error` containing `future_timestamp` and the offending value.
- R2: `read_new_events` rejects events whose `ts` (after alias resolution) does not parse as RFC3339. The event is recorded as a `MalformedLine` with `error` containing `invalid_timestamp`.
- R3: `read_new_events` continues to accept events whose `ts` is the empty string (current behavior — `#[serde(default)]` produces `""`). The 5-minute future window check (R1) does not fire on empty `ts`. The RFC3339 parse check (R2) does not fire on empty `ts`. This preserves the existing behavior of accepting `ts`-less legacy fixtures (e.g., `crates/ralph-core/tests/fixtures/basic_session.jsonl`).
- R4: The `Event` struct deserializes both `"ts"` and `"timestamp"` field names into the same `ts: String` field, via `#[serde(alias = "timestamp")]`. Other fields are unchanged.
- R5: The fix does not change any write path (no changes to `emit.rs`, `wave.rs`, or any other emitter). No producer-side changes.
- R6: The fix does not change the `MalformedLine` struct, the `ParseResult` struct, or the public signatures of `read_new_events` / `peek_new_events`. Only their *internal behavior* changes.
- R7: No regression in the existing `event_reader` test suite (`crates/ralph-core/src/event_reader.rs:288` onward, 13+ test cases). All existing tests pass with no modification other than the timestamp-window additions.
- R8: No regression in the `event_loop` integration tests that exercise `read_new_events` (callers in `event_loop/mod.rs:4443,6345,8463` and `loop_state_snapshot.rs:70`). All continue to pass.

## Out of scope (explicit)

- **`task_id=u1-scaffold-001` self-fabrication** (line 18 of the worktree file). Different bug surface; addressing it requires a different fix at the dimension-reviewer worker side.
- **`wave.worker.failed` placeholder path** (lines 22, 23 of the worktree file). A separate defense-in-depth task that touches `wave_dispatcher.rs` and `dimension-reviewer.publishes`; bigger surface, separate plan.
- **EventPolicy timestamp validation.** `EventPolicy::validate_event` (`crates/ralph-core/src/event_policy.rs:574`) operates on the `topic` + `payload`, not the JSONL row. The `ts` field is a JSONL-only concern; moving the check there would require restructuring validate's input.
- **Write-side guards in `emit.rs` / `wave.rs`.** They already use `Utc::now()`; a guard there is redundant. Skipping keeps the diff minimal.
- **Backfilling or scrubbing existing `events.jsonl` files with forged timestamps.** A migration tool is a separate user decision. The fix is a forward-only gate; the user's existing files are unchanged.

## Key Technical Decisions

- **KTD-1**: Enforcement lives in `EventReader::read_new_events` (read path), not in any write path or in `EventPolicy`. Rationale: every event must cross the reader, but forged events may bypass any individual writer. The reader is the universal chokepoint.
- **KTD-2**: Empty `ts` is allowed (R3). The window check skips when `ts.is_empty()`. Rationale: existing fixtures and `serde_json::from_str` without `ts` are common; rejecting them would break the existing test suite (R7).
- **KTD-3**: The 5-minute future window is hardcoded as a `const` in `event_reader.rs` (no config knob). Rationale: a config knob adds a new surface to test, document, and migrate. The 5-minute figure matches the worker boot-up window (5 min is enough to absorb clock skew, container time drift, and PTY spawn latency; not enough to be a meaningful attack or fixture forgery window). If a future plan needs a different value, it can be promoted to config then.
- **KTD-4**: The `serde(alias = "timestamp")` is added on the `ts` field only, not on every field. Rationale: only `ts` has a known divergent alias; the other fields have no observed aliasing in the wild. Adding aliases speculatively would mask future field-naming drift instead of catching it.
- **KTD-5**: The check is `ts > now + 5min` only — no lower bound. Rationale: a forged *past* timestamp is not the bug the user reported. Adding a lower bound would risk rejecting legitimately-stale events replayed from session recordings. The user's observed anomaly is a *future* timestamp, so the gate is asymmetric on purpose.

## System-Wide Impact

- **Affected components**: `EventReader::read_new_events` (and indirectly `peek_new_events` which delegates to it) in `crates/ralph-core/src/event_reader.rs`. Callers: `event_loop/mod.rs`, `loop_state_snapshot.rs`, `event_policy.rs`, `commands/emit.rs` tests.
- **Affected operators**: anyone replaying or hand-editing `events.jsonl` files with non-`ts` field names or future timestamps will now see the line rejected as malformed instead of silently accepted. This is the intended behavior change.
- **No operational change** for the normal case: every real event written by the three production paths still parses and flows through unchanged.
- **Backwards compat**: existing fixture files use `ts` (verified across `crates/ralph-core/tests/fixtures/`); no fixture migration is needed.

## Risks & Mitigations

- **Risk**: A legitimate future-dated event (e.g., a planned `work.start` scheduled for next week) gets rejected. **Mitigation**: the orchestrator does not currently schedule future events. All production writers use `Utc::now()`. If scheduling is added later, the fix point is well-localized: a single `const` in `event_reader.rs`.
- **Risk**: A test that uses `Utc::now() + 10min` as a synthetic future timestamp now fails. **Mitigation**: scan existing tests for `+ 5min` or `+ future` patterns. None found in `crates/ralph-core/src/event_reader.rs` (verified by reading the full 750-line file). If a future test is added with a future ts, it must use a value within 5 minutes of `now()` or be marked as testing the rejection path explicitly.
- **Risk**: The `serde(alias)` change breaks a serde derive order or round-trip test. **Mitigation**: `ts` is still the primary field name; `alias` adds a *secondary* deserializer. Serialization still emits `ts` (no change in `Serialize` direction). Round-trip tests that do `to_string → from_str` continue to work because they go through the primary name.
- **Risk**: The new error string `future_timestamp` / `invalid_timestamp` collides with an existing grep pattern used by users. **Mitigation**: the strings are new vocabulary; no existing grep patterns were found in `docs/` or `crates/ralph-core/data/ralph-tools*.md` that would be confused by them.

## Implementation Units

### U1. Add timestamp alias + window check to `Event` and `EventReader::read_new_events`

**Goal**: Forged-future and non-RFC3339 `ts` values (and `timestamp`-aliased values) are caught at read time, recorded as `MalformedLine`, excluded from `result.events`. Empty `ts` remains accepted.

**Files**:
- `crates/ralph-core/src/event_reader.rs` — modify

**Approach**:
1. Add `#[serde(alias = "timestamp")]` to the `ts` field of the `Event` struct (line 98). Keep `#[serde(default)]`. Result: the field deserializes from `"ts"`, `"timestamp"`, or empty default.
2. Add a private constant `MAX_FUTURE_TS_SKEW_SECS: i64 = 300;` near the top of the file (after imports, before `MalformedLine` impl).
3. Add a private helper function `fn classify_timestamp(ts: &str) -> Result<(), &'static str>` that returns:
   - `Ok(())` for empty `ts` (skip the check; preserves R3).
   - `Ok(())` for ts that parses as RFC3339 and `parsed <= Utc::now() + 300s`.
   - `Err("future_timestamp")` for ts that parses as RFC3339 and `parsed > Utc::now() + 300s`.
   - `Err("invalid_timestamp")` for ts that does not parse as RFC3339.
4. In `read_new_events` (line 178), after the existing `serde_json::from_str::<Event>(&line)` `Ok(event)` branch (line 202), before `result.events.push(event)`, call `classify_timestamp(&event.ts)`:
   - `Ok(())` → push to `result.events` (existing behavior).
   - `Err(reason)` → push a `MalformedLine` with `line_number`, `content` (truncated via `MalformedLine::new`), and `error` formatted as `"{reason}: {ts}"` (where `{ts}` is the raw string).
5. The same check is automatically inherited by `peek_new_events` because it delegates to `read_new_events` on a cloned reader (verified at line 224).

**Patterns to follow**: The existing `MalformedLine::new` truncation (line 41), the `serde(default, skip_serializing_if = ...)` idiom on adjacent fields (lines 101, 105, 109), the existing `#[serde(deserialize_with = "...")]` pattern on `payload` (line 94).

**Test scenarios**:
- **Happy path** (covers R7 regression): existing tests in `event_reader.rs:288,330,506,532,553,586,605,627,690,710,729` all continue to pass — their fixtures use `ts` values like `"2024-01-01T00:00:00Z"` (well in the past) and the new check is a no-op.
- **Edge case (alias)**: write a line with `{"topic":"x","timestamp":"2024-01-01T00:00:00Z"}` and verify `result.events[0].ts == "2024-01-01T00:00:00Z"`. (Covers R4.)
- **Edge case (empty)**: write a line with `{"topic":"x"}` (no `ts` at all). Verify `result.events[0].ts == ""` and the line is **not** malformed. (Covers R3.)
- **Error path (future)**: write a line with `{"topic":"x","ts":"<now+10min RFC3339>"}`. Verify `result.events` is empty and `result.malformed[0].error` contains `future_timestamp`. (Covers R1.)
- **Error path (invalid)**: write a line with `{"topic":"x","ts":"not-a-date"}`. Verify `result.events` is empty and `result.malformed[0].error` contains `invalid_timestamp`. (Covers R2.)
- **Error path (alias with future)**: write a line with `{"topic":"x","timestamp":"<now+10min RFC3339>"}`. Verify `result.events` is empty and `result.malformed[0].error` contains `future_timestamp`. (Covers R1 + R4 combined.)
- **Integration scenario (line 19 of observed file)**: write the exact line 19 verbatim (with `timestamp: "2026-06-17T00:00:00Z"`, payload intact) into a temp file, run `read_new_events`, verify it lands in `result.malformed` with `future_timestamp` in the error. This is the regression test for the specific observed bug.

**Verification**:
- `cargo nextest run -p ralph-core -- event_reader` — all 13+ existing tests pass.
- `cargo nextest run -p ralph-core` — full crate passes (no regression in `event_loop` or `event_policy` callers of `read_new_events`).
- New test cases above all pass.
- `cargo clippy --workspace` clean.

**Dependencies**: none.

### U2. Add CLI / loop test asserting line-19-shaped payload lands in `malformed`

**Goal**: Lock the regression so future changes to `Event` or `read_new_events` cannot silently re-introduce the "future timestamp via alias" path.

**Files**:
- `crates/ralph-core/src/event_reader.rs` — extend existing `#[cfg(test)] mod tests`

**Approach**:
1. Add a test `fn test_future_timestamp_via_timestamp_alias_is_malformed` that writes the line-19 shape into a temp file and asserts the rejection.
2. Add a test `fn test_past_timestamp_accepted` that confirms a ts 1 day in the past is not flagged (no over-rejection).
3. Add a test `fn test_within_5min_future_accepted` that confirms a ts 4 minutes in the future is not flagged (boundary check; below the 5-min threshold).

**Test scenarios**: covered above; each is one new `#[test]` function with assertions on `result.events.len()` and `result.malformed`.

**Verification**: same as U1 plus the three new tests pass.

**Dependencies**: U1 (the helper must exist).

### U3. Reverse-verify the docs that mention `event_reader` or `read_new_events`

**Goal**: Ensure no CLAUDE.md / AGENTS.md / ralph-tools doc references the old (no-window) behavior in a way that would mislead a future agent.

**Files**:
- `crates/ralph-core/data/ralph-tools.md`
- `crates/ralph-core/data/ralph-tools-tasks.md`
- `crates/ralph-core/data/ralph-tools-memories.md`
- `CLAUDE.md`
- `AGENTS.md`

**Approach**:
1. `rg -n "read_new_events|EventReader|future.timestamp" crates/ralph-core/data/ docs/ CLAUDE.md AGENTS.md` to surface any text claims.
2. For each match, verify the text still reflects post-fix behavior. The fix adds a rejection of future `ts`; if any doc says "events.jsonl accepts any well-formed JSON line" or similar, append a sentence about the timestamp window. Do not introduce new sections.
3. If no matches, document the absence in the PR description: "No doc text claims about timestamp handling needed updating."

**Test scenarios**: not applicable (documentation reverse-verification).

**Verification**: `rg` returns no contradictions; CLAUDE.md / AGENTS.md remain in sync per existing project rules.

**Dependencies**: U1 (the behavior must be final before doc verification).

## Open Questions

None. The bug surface, the fix point, and the test cases are all bounded by the observed evidence and the existing test surface. No architectural decisions remain.

## Sources

- `crates/ralph-proto/src/event.rs:8-32` — the on-the-wire `Event` proto (no `ts` field; ts is JSONL-only).
- `crates/ralph-core/src/event_reader.rs:88-123` — the JSONL `Event` struct with `ts: String` (the field that needs `alias`).
- `crates/ralph-core/src/event_reader.rs:178-216` — `read_new_events` (the enforcement point).
- `crates/ralph-core/src/event_reader.rs:288-729` — 13 existing tests that establish the regression baseline.
- `crates/ralph-cli/src/commands/emit.rs:585` — `ts = chrono::Utc::now().to_rfc3339()` (write path 1, real time).
- `crates/ralph-cli/src/wave.rs:523,1144,1155` — same pattern in wave write paths (write paths 2 + 3, real time).
- Worktree evidence: `.worktrees/2026-06-10-003-...-sunny-lotus/.ralph/events-20260616-161905.jsonl` lines 17 and 19 (the offending events).
