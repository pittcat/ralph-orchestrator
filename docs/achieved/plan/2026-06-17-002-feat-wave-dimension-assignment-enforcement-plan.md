---
title: "feat: Enforce wave dimension assignment in ce-executor-isolated review waves"
type: feat
status: active
date: 2026-06-17
deepened: 2026-06-17
origin: docs/brainstorms/2026-06-17-wave-dimension-assignment-enforcement-requirements.md
---

# feat: Enforce wave dimension assignment in ce-executor-isolated review waves

## Overview

Add runtime binding and validation so that each worker in a `review.wave.ready` wave is hard-locked to the dimension it was assigned. When a `dimension-reviewer` worker emits `review.dimension.done` with the wrong `dimension`, the bad event is rejected, a per-slot retry is triggered, and the synthesizer still receives an accurate partial-wave signal. This prevents the "same dimension reviewed twice, other dimensions missing" failure seen in the 2026-06-17 keen-fern run.

> Note: the origin requirements doc suggested sequence `001` for this plan, but `docs/plans/2026-06-17-001-fix-stall-detector-reset-and-policy-ttl-plan.md` already exists, so this plan uses `002`.

---

## Problem Frame

In `ce-executor-isolated`, `review-coordinator` emits a wave of `review.wave.ready` events, one per selected dimension. The dispatcher spawns one `dimension-reviewer` worker per payload. In practice, workers under pressure return the wrong `dimension` or fail to return, leaving `review-synthesizer` with an incomplete/inconsistent set of `review.dimension.done` events. The existing `incomplete_wave_gate` correctly intercepts the symptom by emitting `plan.blocked(reason=dimension_reviewers_failed_to_converge)`, but it does not prevent the bad data from entering the stream or recover the individual slot.

This plan implements three hard gates — dispatch binding, CLI precheck, and merge validation — plus targeted per-slot recovery, so that a single wrong-dimension worker can be retried without failing the whole wave.

(See origin: `docs/brainstorms/2026-06-17-wave-dimension-assignment-enforcement-requirements.md` and `docs/report/2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md`.)

---

## Requirements Trace

- **R1 — Dispatch binding:** Dispatcher parses `dimension` from each `review.wave.ready` payload and stores it as the assigned dimension for that `wave_index`.
- **R2 — Worker input channels:** Assigned dimension is passed to the worker via both a prompt block and the `RALPH_WAVE_DIMENSION` environment variable.
- **R3 — CLI precheck:** `ralph emit review.dimension.done` rejects mismatched `dimension` when `RALPH_WAVE_DIMENSION` is set, with structured `expected_dimension` / `actual_dimension` error.
- **R4 — Merge validation:** Dispatcher drops `review.dimension.done` events whose `dimension` does not match the assigned value before merging into the main events file.
- **R5 — Per-slot retry:** On mismatch, dispatcher records `wave.worker.failed(dimension_mismatch)` and injects a targeted `task.resume` to retry the offending worker slot.
- **R6 — Missing dimension signal:** Timeout or rejected slots produce a placeholder/equivalent signal so `review-synthesizer` knows which dimensions are missing.
- **R7 — Preset instructions:** `dimension-reviewer` instructions explicitly reference `## ASSIGNED DIMENSION` and `RALPH_WAVE_DIMENSION`, and state the HARD RULE.
- **R8 — Schema freedom:** `review.dimension.done` keeps `dimension` as a free string; enforcement is dynamic, not static schema.
- **R9 — Observability:** Mismatch / missing events write `recovery.jsonl` envelopes with `source=wave_dimension_guard` and `reason_code=dimension_mismatch` / `dimension_missing`.
- **R10 — Regression test:** New BDD scenario / replay fixture proving a 4-dimension wave with one wrong-dimension worker converges to 4 correct results via retry.
- **R11 — Test gate:** `cargo nextest run --workspace --exclude ralph-e2e` passes.

---

## Scope Boundaries

- **In scope:** wave worker dimension binding, prompt/env injection, CLI/merge validation, per-slot retry, recovery envelopes, preset instruction updates, and regression tests for `ce-executor-isolated`.
- **Out of scope for this plan:**
  - Changing review-coordinator dimension selection strategy.
  - Removing waves or making review serial.
  - Modifying `incomplete_wave_gate` thresholds.
  - Fixing the keen-fern U1 residuals (`audit-file-sizes.sh`, `test_u2_*` fixture schema).
  - Adding new agent backends or model capabilities.
  - General recovery.jsonl noise reduction (covered separately).

### Deferred to Follow-Up Work

- Same-iteration dispatcher retry for mismatched slots (this plan uses `task.resume` loop-round retry first; optimize only if acceptance tests show excessive latency).
- Duplicate expected-dimension detection at dispatch time (this plan logs/observes but does not hard-fail; revisit if field data shows it is a real failure mode).

---

## Context & Research

### Relevant Code and Patterns

- **Wave dispatch / worker construction:** `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` (`execute_wave_structured`, `WorkerRequest`, env-injection block) builds per-worker requests and calls `merge_wave_results_to_events_file`.
- **Per-worker I/O and merge:** `crates/ralph-cli/src/loop_runner/wave/io.rs` (`merge_wave_results_to_events_file`) already validates source-hat provenance and writes synthetic `wave.worker.failed` records for worker failures.
- **Worker prompt builder:** `crates/ralph-core/src/wave_prompt.rs` (`build_wave_worker_prompt`, `WaveWorkerContext`) renders the prompt that the worker backend receives.
- **Wave context for synthesizer:** `crates/ralph-core/src/wave_context.rs` derives `expected_dimensions` and `missing_dimensions` from the last N events.
- **CLI emit and policy check:** `crates/ralph-cli/src/commands/emit.rs` (`emit_command_with_root_and_hats`) and `crates/ralph-cli/src/policy_check.rs` perform pre-write validation; recovery envelopes are written via `write_cli_emit_recovery_envelope`.
- **Recovery / rejection routing:** `crates/ralph-core/src/event_loop/mod.rs` (`publish_policy_rejection_resume`) and `crates/ralph-core/src/event_loop/rejection.rs` (`build_task_resume_payload`) show how `task.resume` is routed back to a source hat.
- **Recovery envelope model:** `crates/ralph-core/src/diagnosis/envelope.rs` (`RecoveryDiagnosisEnvelope`, `DiagnosisSource`) and `crates/ralph-core/src/diagnosis/journal.rs`.
- **Incomplete-wave gate:** `crates/ralph-core/src/flow_lifecycle/incomplete_wave_gate.rs` and `crates/ralph-core/src/event_loop/review_step_state.rs` track open waves and emit `plan.blocked` on staleness.
- **Preset / schema:** `presets/en/ce-executor-isolated.yml` (hat definitions), `presets/schemas/ce-executor-isolated.yml` (schema SSOT), `crates/ralph-cli/src/presets.rs` (embedded registry), `presets/manifest.yml`, `presets/index.json`.

### Institutional Learnings

- `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md`: Isolated wave budget, source-hat provenance for synthetic `wave.worker.failed`, and `task.resume` TTL/freshness are all recent concerns. New synthetic records must use a publisher that is in the preset's `publishes` list (currently `review-synthesizer` for `wave.worker.failed`).
- `docs/solutions/integration-issues/ce-executor-wave-emission-must-batch-in-single-emit-2026-06-09.md`: Preset wording should include concrete CLI snippets and HARD RULE markers; consider adding a contract test for new preset invariants.
- `AGENTS.md` (Build & Test): use `cargo nextest run` series; `ralph-cli` tests run serially via `cli-serial` group. Any new tests in `ralph-cli` must tolerate nextest's process-per-test isolation.

### External References

- None required — local patterns are sufficient and recent incident docs provide the necessary context.

---

## Key Technical Decisions

1. **Assigned dimension travels with `WaveWorkerContext` and `WorkerRequest`.**
   - Rationale: both the prompt builder and the worker spawn already receive these per-worker structs, so adding `assigned_dimension: Option<String>` is the smallest change and avoids a separate in-memory map that could drift from the actual worker list.
   - (Resolves origin deferred question: assigned dimension context storage.)

2. **CLI precheck is a dedicated guard in `policy_check.rs`, not a schema change.**
   - Rationale: the requirement is dynamic (env var `RALPH_WAVE_DIMENSION` may or may not be present), and `dimension` must remain a free string per R8. A dedicated helper keeps the generic schema validator clean.
   - (Resolves origin deferred question: CLI precheck implementation.)

3. **Retry is `task.resume`-driven, routed to `dimension-reviewer`, with a per-slot budget of two attempts.**
   - Rationale: matches the existing R5 hard-gate routing pattern and the requirement doc's F2/R5. `dimension-reviewer` must be added to `task.resume` triggers so the retry wakes the hat in the next loop iteration. The `task.resume` JSONL record must include `triggered: "dimension-reviewer"` and a payload satisfying the preset `task.resume` schema (`reason`, `target_hat`), plus wave context (`wave_id`, `wave_index`, `expected_dimension`). After two attempts (initial + one retry) the slot is treated as missing and the wave may proceed to `incomplete_wave_gate`.
   - (Resolves origin deferred question: retry strategy.)

4. **Missing-dimension signal is `wave.worker.failed` + accurate `WaveContext.missing_dimensions`.**
   - Rationale: writing a synthetic `review.dimension.done` placeholder would require the placeholder to satisfy the full schema and would risk being counted as a received dimension. The existing `wave.worker.failed` topic already carries structured failure reasons and is consumed by `review-synthesizer`; extending it with `reason: dimension_mismatch` / `dimension_missing` gives the synthesizer the missing-dimension list while keeping `WaveContext` derivation simple. `WaveContext` computes `missing_dimensions` from expected `review.wave.ready` dimensions minus valid received `review.dimension.done` dimensions.
   - (Resolves origin deferred question: placeholder / missing dimension signal shape.)

5. **Mismatched events are dropped, not rewritten.**
   - Rationale: rewriting the `dimension` field would hide the agent error and mis-attribute findings to the wrong dimension. Dropping + retry preserves observability and correct provenance.

---

## Open Questions

### Resolved During Planning

- **Where to store the assigned dimension?** Extend `WaveWorkerContext` and `WorkerRequest` with `assigned_dimension: Option<String>`.
- **How to implement CLI precheck?** New `check_wave_dimension_assignment` helper in `crates/ralph-cli/src/policy_check.rs`, called from `commands/emit.rs` after the step-handoff gate.
- **How to retry a mismatched slot?** Inject `task.resume` targeted at `dimension-reviewer`; add `task.resume` to `dimension-reviewer.triggers`; payload must satisfy preset schema (`reason`, `target_hat`) plus `wave_id`, `wave_index`, `expected_dimension`. Per-slot retry budget is two attempts (initial + one retry); exhausted slots are treated as missing.
- **What shape for missing-dimension signal?** `wave.worker.failed` synthetic record with `reason: dimension_mismatch` or `dimension_missing`; `WaveContext.missing_dimensions` derived from expected vs valid received dimensions.

### Deferred to Implementation

- Whether to add dispatch-time hard failure for duplicate expected dimensions in the same wave (observe first; current field data suggests this is not the active failure mode).

---

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

```text
review-coordinator emits review.wave.ready [dim=c] [dim=t] [dim=m] [dim=r]
                    │
                    ▼
        dispatcher execute_wave_structured
                    │
    ┌───────────────┼───────────────┐
    │               │               │
    ▼               ▼               ▼
 Worker 0       Worker 1       Worker N
 (dim=c)        (dim=t)        (dim=r)
    │               │               │
    ▼               ▼               ▼
RALPH_WAVE_    RALPH_WAVE_    RALPH_WAVE_
DIMENSION=c    DIMENSION=t    DIMENSION=r
prompt block   prompt block   prompt block
    │               │               │
    ▼               ▼               ▼
ralph emit review.dimension.done --json '{"dimension":"t",...}'
    │               │               │
    ▼               ▼               ▼
CLI precheck (R3) ──┘               │
    │                               │
    ▼                               ▼
merge_wave_results_to_events_file
    │
    ├─ correct dimension  → merged to events.jsonl
    ├─ wrong dimension    → dropped; wave.worker.failed(dimension_mismatch);
    │                       recovery.jsonl envelope; task.resume → dimension-reviewer
    └─ timeout / no emit  → wave.worker.failed(dimension_missing)
                    │
                    ▼
        review-synthesizer sees missing_dimensions + retries
                    │
                    ▼
        incomplete_wave_gate only fires if retries also fail to converge
```

---

## Implementation Units

- [ ] U1. **Bind assigned dimension in dispatcher**

**Goal:** Parse `dimension` from each `review.wave.ready` payload and attach it to the worker request so it is available for prompt/env injection and merge validation.

**Requirements:** R1

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` (`WorkerRequest` is defined here)
- Modify: `crates/ralph-core/src/wave_prompt.rs` (`WaveWorkerContext`)
- Test: `crates/ralph-cli/src/loop_runner/tests.rs`

**Approach:**
- Add `assigned_dimension: Option<String>` to `WaveWorkerContext` and to `WorkerRequest`.
- In `execute_wave_structured`, parse `event.payload` JSON for the `dimension` field when building each worker; trim whitespace and validate it is a non-empty string; store it in both structs.
- If `dimension` is missing, empty, whitespace-only, or not a string, record a `WaveFailure` for that index with reason `dimension_missing` (do not spawn a worker for an uncloseable slot).

**Patterns to follow:**
- Existing `RALPH_WAVE_*` env injection block in `dispatcher.rs`.
- `serde_json` payload parsing pattern used in `event_loop/mod.rs`.

**Test scenarios:**
- Happy path: 4-dimension wave produces 4 `WorkerRequest`s with `assigned_dimension` matching each payload.
- Edge case: payload missing `dimension` → dispatcher records a `WaveFailure(dimension_missing)` for that index and skips worker spawn.
- Edge case: duplicate expected dimensions in one wave → dispatcher logs a diagnostic warning but still spawns workers for each payload (behavior to be hardened later if field data warrants it).

**Verification:**
- Unit tests assert `WorkerRequest.assigned_dimension` matches `review.wave.ready` payload `dimension`.

---

- [ ] U2. **Inject assigned dimension into worker prompt and environment**

**Goal:** Ensure the worker receives its assigned dimension through both prompt and env var channels.

**Requirements:** R2, R7

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/src/wave_prompt.rs`
- Modify: `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`
- Test: `crates/ralph-core/src/wave_prompt.rs` (unit tests)

**Approach:**
- Extend `WaveWorkerContext` with `assigned_dimension`.
- In `build_wave_worker_prompt`, render a `## ASSIGNED DIMENSION: {dimension}` block before `# Your Task`.
- In `dispatcher.rs`, inject `RALPH_WAVE_DIMENSION` into each worker's env vars with the assigned dimension.
- Keep the block minimal and authoritative; the HARD RULE language lives in the preset (U6), not in code.

**Patterns to follow:**
- `WaveContext::to_prompt_block` formatting style in `crates/ralph-core/src/wave_context.rs`.
- Existing `RALPH_WAVE_ID` / `RALPH_WAVE_INDEX` env injection.

**Test scenarios:**
- Happy path: worker prompt contains `## ASSIGNED DIMENSION: testing` when assigned `testing`.
- Edge case: `assigned_dimension` is `None` → no block and no env var injected.
- Integration: spawned worker process sees `RALPH_WAVE_DIMENSION=testing`.

**Verification:**
- Unit test on `build_wave_worker_prompt` asserts block presence and value.
- Dispatcher test asserts env var list contains `RALPH_WAVE_DIMENSION`.

---

- [ ] U3. **Add CLI precheck for RALPH_WAVE_DIMENSION**

**Goal:** Reject `ralph emit review.dimension.done` with a mismatched `dimension` before the event reaches any events file.

**Requirements:** R3, R9

**Dependencies:** U1 (for concept; implementation can be independent), U7 (DiagnosisSource::WaveDimensionGuard must exist)

**Files:**
- Modify: `crates/ralph-cli/src/policy_check.rs`
- Modify: `crates/ralph-cli/src/commands/emit.rs`
- Test: `crates/ralph-cli/tests/integration_emit_policy.rs`

**Approach:**
- Add `check_wave_dimension_assignment(payload_json: &str, expected: &str) -> Result<(), ValidationError>` in `policy_check.rs`.
- Call it from `emit_command_with_root_and_hats` after the step-handoff gate and before timestamp generation, only when `args.topic == "review.dimension.done"` and `RALPH_WAVE_DIMENSION` is set and non-empty.
- On mismatch, build a recovery envelope with `source: DiagnosisSource::WaveDimensionGuard`, `reason_code: "dimension_mismatch"`, and a message containing `expected_dimension=... actual_dimension=...`. (The variant is added in U7; sequence U7 before this recovery-envelope path compiles.)
- Exit non-zero; do not append to events file.

**Patterns to follow:**
- `check_step_handoff_gate` shape and error handling in `policy_check.rs`.
- `write_cli_emit_recovery_envelope` in `commands/emit.rs`.

**Test scenarios:**
- Happy path: `RALPH_WAVE_DIMENSION=testing` and payload `dimension=testing` → accepted.
- Error path: `RALPH_WAVE_DIMENSION=testing` and payload `dimension=correctness` → non-zero exit, stderr contains `expected_dimension=testing actual_dimension=correctness`, recovery envelope written with `source=wave_dimension_guard`.
- Edge case: payload is not JSON or lacks `dimension` → treated as mismatch (actual = `<missing>`).
- Edge case: `RALPH_WAVE_DIMENSION` unset → no check, normal policy validation applies.

**Verification:**
- Integration test spawns `ralph emit` and asserts exit code, stderr, and recovery.jsonl contents.

---

- [ ] U4. **Validate dimensions at merge and emit synthetic failure records**

**Goal:** Drop mismatched `review.dimension.done` events when merging worker results back to the main events file, record the failure, and produce accurate missing-dimension signals.

**Requirements:** R4, R5, R6, R8

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner/wave/io.rs`
- Modify: `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`
- Modify: `crates/ralph-core/src/wave_tracker.rs` (extend `WaveFailure` / `WaveFailureReason` to carry expected/actual)
- Modify: `crates/ralph-core/src/wave_context.rs` (exclude failed slots from `received_dimensions`)
- Test: `crates/ralph-cli/src/loop_runner/tests.rs`

**Approach:**
- Add `assigned_dimensions: HashMap<u32, String>` to `CompletedWave` (default empty for legacy callers).
- Validate dimensions **before** calling `tracker.record_result`: if the worker's emitted `dimension` does not match `assigned_dimensions.get(&index)`, route the outcome to `tracker.record_failure` and never create a `WaveResult` for that index.
- Extend `WaveFailure` / `WaveFailureReason` to carry `expected_dimension` and `actual_dimension` for `dimension_mismatch`.
- For every `WaveFailure` (timeout or mismatch), write a synthetic `wave.worker.failed` record (source hat `review-synthesizer`, as established by 2026-06-16 U2) with a JSON payload containing `reason`, `wave_id`, `wave_index`, `expected_dimension`, and `actual_dimension` when applicable.
- Do **not** write synthetic `review.dimension.done` placeholders; rely on `WaveContext.missing_dimensions` to inform the synthesizer.
- Ensure `merge_wave_results_to_events_file` only adds an index to `merged_indexes` when at least one event from that result is actually written.
- Ensure mismatched dimensions are **not** counted as received in `WaveContext` or `ReviewStepTracker`.

**Patterns to follow:**
- Existing source-hat spoofing check and synthetic `wave.worker.failed` writing in `io.rs`.
- Existing `WaveFailure` / `WaveResult` handling in `wave_tracker.rs`.
- Recent 2026-06-16 U2 change to `wave.worker.failed` provenance.

**Test scenarios:**
- Happy path: all 4 workers return correct dimensions → main events file contains 4 valid `review.dimension.done` records and no `wave.worker.failed`.
- Error path: worker 1 returns `correctness` when assigned `testing` → event dropped; main file has 3 valid records + 1 `wave.worker.failed(dimension_mismatch)`; `WaveContext.missing_dimensions` includes `testing`.
- Edge case: worker returns non-JSON payload or no `dimension` → treated as mismatch with `actual_dimension=<missing>`.
- Edge case: worker returns correct dimension plus an extra wrong-dimension event → first correct accepted, extra dropped as mismatch (the index is already satisfied, so the extra becomes a mismatch failure).
- Edge case: timeout slot → `wave.worker.failed(dimension_missing)` and `missing_dimensions` includes the expected dimension.

**Verification:**
- Unit tests assert main events file contents, `WaveFailure` state, and `WaveContext.missing_dimensions` for mismatch and timeout cases.

---

- [ ] U5. **Inject targeted task.resume for mismatched slots**

**Goal:** Give the offending `dimension-reviewer` worker a recoverable signal to re-emit the correct dimension.

**Requirements:** R5

**Dependencies:** U4

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`
- Modify: `crates/ralph-core/src/event_loop/rejection.rs` (reuse `build_task_resume_payload` pattern or add dispatcher-specific helper)
- Modify: `presets/en/ce-executor-isolated.yml` (add `task.resume` to `dimension-reviewer.triggers`)
- Test: `crates/ralph-cli/src/loop_runner/tests.rs`, `crates/ralph-core/tests/scenarios/`

**Approach:**
- After merge detects a mismatch, write a `task.resume` JSONL record to the main events file with:
  - `topic: "task.resume"`
  - `triggered: "dimension-reviewer"` (maps to `Event.target`)
  - `hat` / `source`: the runtime (`ralph`) or `dimension-reviewer` as permitted by origin guard
  - Payload satisfying the preset `task.resume` schema:
    - `reason: "dimension_mismatch"`
    - `target_hat: "dimension-reviewer"`
    - `wave_id`, `wave_index`, `wave_total`
    - `expected_dimension`, `actual_dimension`
    - The original `review.wave.ready` payload content (or enough context for the agent to reproduce the review).
- Track a per-slot retry counter keyed by `(wave_id, wave_index)`; only emit `task.resume` if the slot has not exhausted its budget of two attempts (initial + one retry). On exhaustion, treat the slot as missing and do not emit further resumes for it.
- Add `task.resume` to `dimension-reviewer.triggers` in the preset so the retry wakes the hat.
- Update `dimension-reviewer.instructions` (U6) to explain how to act on this specific `task.resume` (look for `reason=dimension_mismatch`, re-emit `review.dimension.done` with `expected_dimension`).

**Patterns to follow:**
- `publish_policy_rejection_resume` in `crates/ralph-core/src/event_loop/mod.rs`.
- `build_task_resume_payload` in `crates/ralph-core/src/event_loop/rejection.rs`.

**Test scenarios:**
- Happy path: mismatch on first attempt → `task.resume` record appears in main events file with `triggered=dimension-reviewer`, payload `reason=dimension_mismatch`, `target_hat=dimension-reviewer`, and correct `expected_dimension`.
- Error path: second consecutive mismatch for same slot → no second `task.resume`; slot is treated as missing.
- Edge case: multiple mismatches in one wave (different slots) → one `task.resume` per mismatched slot, up to budget.
- Edge case: `task.resume` record validates against preset `task.resume` schema (`reason` and `target_hat` present).

**Verification:**
- Unit test asserts `task.resume` JSONL record shape and retry-budget behavior after merge.
- BDD scenario verifies retry eventually produces a correct `review.dimension.done` and no `plan.blocked(dimension_reviewers_failed_to_converge)` when convergence succeeds.

---

- [ ] U6. **Update preset instructions and schema**

**Goal:** Align the `ce-executor-isolated` preset with the new hard-binding mechanism so agents know how to use `## ASSIGNED DIMENSION` and `RALPH_WAVE_DIMENSION`.

**Requirements:** R7, R8

**Dependencies:** U2, U5

**Files:**
- Modify: `presets/en/ce-executor-isolated.yml`
- Modify: `presets/zh/ce-executor-isolated-zh.yml`
- Modify: `crates/ralph-core/data/ralph-tools.md` (document `RALPH_WAVE_DIMENSION`)
- Test: `crates/ralph-cli/src/presets.rs` (preset text contract tests)

**Approach:**
- In `dimension-reviewer.instructions`, replace the soft "Your dimension is from the event payload's `dimension` field" with:
  - Reference to `## ASSIGNED DIMENSION: <dimension>` at the top of the prompt.
  - Reference to `RALPH_WAVE_DIMENSION` env var.
  - HARD RULE: emitted `review.dimension.done` `dimension` must exactly match the assigned dimension; mismatches are rejected and retried.
  - New subsection: how to handle a `task.resume` with `reason=dimension_mismatch` (re-emit `review.dimension.done` for the expected dimension).
- Add `task.resume` to `dimension-reviewer.triggers`.
- Verify `task.resume` is already allowed by preset lint / runner-internal topics (it was added by 2026-06-16 U5).
- Sync the Chinese reference preset `presets/zh/ce-executor-isolated-zh.yml`.
- Add a contract test in `crates/ralph-cli/src/presets.rs` asserting the `dimension-reviewer` instructions contain `## ASSIGNED DIMENSION`, `RALPH_WAVE_DIMENSION`, and a HARD RULE marker about exact dimension match.

**Patterns to follow:**
- Existing HARD RULE sections in `ce-executor-isolated.yml` (Commit Cadence, Preflight Contract, Wave Emission batching).
- Existing preset text contract tests in `crates/ralph-cli/src/presets.rs`.

**Test scenarios:**
- Preset lint: `ralph preset check builtin:ce-executor-isolated` passes.
- Contract test: `dimension-reviewer` instructions contain required markers.
- Chinese variant mirrors the English invariant markers.

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- presets::tests` passes.

---

- [ ] U7. **Add recovery envelopes and observability**

**Goal:** Ensure every mismatch / missing dimension is observable in `recovery.jsonl` with the right source and reason code.

**Requirements:** R9

**Dependencies:** None (must be implemented before U3's recovery-envelope path and U4's dispatcher envelope can compile)

**Files:**
- Modify: `crates/ralph-core/src/diagnosis/envelope.rs`
- Modify: `crates/ralph-cli/src/commands/emit.rs`
- Modify: `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`
- Test: `crates/ralph-cli/tests/integration_emit_policy.rs`, `crates/ralph-cli/src/loop_runner/tests.rs`

**Approach:**
- Add `DiagnosisSource::WaveDimensionGuard` variant with serialization `"wave_dimension_guard"`; ensure it is included in any `as_str` / serialization match.
- CLI precheck (U3) writes recovery envelope with `source=wave_dimension_guard`, `reason_code=dimension_mismatch`, and evidence for `dimension` field.
- Dispatcher merge path (U4) writes recovery envelope with `source=wave_dimension_guard`, `reason_code=dimension_mismatch` (or `dimension_missing` for timeout slots), and evidence carrying `wave_id`, `wave_index`, `expected_dimension`.
- Reuse existing `RecoveryDiagnosisEnvelope::builder()` and `RecoveryJournalEntry::from_envelope` patterns.

**Patterns to follow:**
- `write_cli_emit_recovery_envelope` and `record_wave_timeout_envelope` patterns.
- `RecoveryDiagnosisEnvelopeBuilder` usage in `crates/ralph-core/src/diagnosis/envelope.rs`.

**Test scenarios:**
- CLI mismatch → recovery.jsonl contains envelope with `source=wave_dimension_guard`, `reason_code=dimension_mismatch`, `expected_dimension=testing`, `actual_dimension=correctness`.
- Merge mismatch → recovery.jsonl contains envelope with `source=wave_dimension_guard`, `reason_code=dimension_mismatch`, plus `wave_id` and `wave_index` evidence.
- Timeout slot → recovery.jsonl contains envelope with `reason_code=dimension_missing`.

**Verification:**
- Unit/integration tests parse `recovery.jsonl` and assert envelope fields.

---

- [ ] U8. **Add regression tests and run test gate**

**Goal:** Prove the new behavior works end-to-end and does not break existing wave scenarios.

**Requirements:** R10, R11

**Dependencies:** U1–U7

**Files:**
- Create: `crates/ralph-core/tests/scenarios/flow_reliability/wave_dimension_mismatch_retry.yml`
- Create / modify: relevant replay fixture under `crates/ralph-core/tests/fixtures/`
- Modify: `crates/ralph-cli/src/loop_runner/tests.rs`
- Modify: `crates/ralph-core/src/wave_prompt.rs` (unit tests)
- Modify: `crates/ralph-cli/tests/integration_emit_policy.rs`
- Create (if needed): `crates/ralph-core/src/event_loop/tests/wave_dimension_assignment.rs` for event-loop-level assertions

**Approach:**
- Add a BDD scenario that replays a 4-dimension wave where `wave_index=1` first emits `dimension=correctness` (wrong) and later emits `dimension=testing` (correct after retry). Assert the loop ends with 4 correct `review.dimension.done` records and no `plan.blocked(dimension_reviewers_failed_to_converge)`.
- Add dispatcher unit tests for binding, mismatch drop, `wave.worker.failed` emission, retry-budget exhaustion, and `task.resume` injection.
- Add `wave_prompt` unit test for `## ASSIGNED DIMENSION` block.
- Add CLI integration test for `RALPH_WAVE_DIMENSION` mismatch rejection and recovery envelope.
- Add `WaveContext` unit test asserting mismatched/timeout slots appear in `missing_dimensions` and do not count as received.
- Run `./scripts/run-tests.sh` as the final verification gate.

**Patterns to follow:**
- Existing BDD scenarios in `crates/ralph-core/tests/scenarios/flow_reliability/`.
- Existing replay fixtures in `crates/ralph-core/tests/fixtures/wave-isolated-dimension-done/`.

**Test scenarios:**
- Integration (BDD): 4-dim wave with one mismatch → converges via retry; no `plan.blocked(dimension_reviewers_failed_to_converge)`.
- Unit: dispatcher merge drops wrong-dimension event and emits `wave.worker.failed(dimension_mismatch)`.
- Unit: retry budget limits `task.resume` to one per slot.
- Unit: prompt contains assigned dimension block.
- Unit: CLI rejects mismatched dimension emit and recovery envelope has `source=wave_dimension_guard`.
- Unit: `WaveContext.missing_dimensions` excludes mismatched/timeout slots from received count.
- Regression: existing wave scenarios still pass.

**Verification:**
- `./scripts/run-tests.sh` passes (or `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` if flake recovery is needed).

---

## System-Wide Impact

- **Interaction graph:**
  - `dimension-reviewer` now triggers on `task.resume` in addition to `review.wave.ready`.
  - `review-synthesizer` already consumes `wave.worker.failed`; it will now see `dimension_mismatch` and `dimension_missing` reasons.
  - Event-loop per-turn budget / isolated wave budget must tolerate `wave.worker.failed` and `task.resume` records correctly. `task.resume` is already in `LOOP_RUNNER_INTERNAL_TOPICS` after 2026-06-16 U5.
- **Error propagation:**
  - CLI mismatch: non-zero exit + recovery envelope; worker process sees the error and can retry in the same shell context.
  - Merge mismatch: event dropped + `wave.worker.failed(dimension_mismatch)` + `task.resume`; next loop iteration wakes `dimension-reviewer` for retry.
- **State lifecycle risks:**
  - A mismatch must not count as a received dimension in `ReviewStepTracker` or `WaveContext`; otherwise `incomplete_wave_gate` would be masked.
  - `wave.worker.failed` records and real `review.dimension.done` records are additive; a later retry success is merged normally.
- **API surface parity:**
  - `ralph emit` behavior changes only when `RALPH_WAVE_DIMENSION` is set and topic is `review.dimension.done`.
  - No change to `ralph wave emit` CLI contract.
- **Integration coverage:**
  - The BDD scenario must prove that a mismatched event does not reach `review-synthesizer` and that retry eventually converges.
- **Unchanged invariants:**
  - `review.dimension.done` schema still treats `dimension` as a free string.
  - `incomplete_wave_gate` thresholds are not modified.
  - Wave isolated scope / distinct `wave_id` rejection is unchanged.

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| `task.resume` retry is too slow to beat `incomplete_wave_gate` 0.8× timeout | Per-slot retry budget of two attempts gives one loop-round retry; acceptance test validates convergence under default 1800 s aggregate timeout. If field data shows repeated timeout-before-retry, promote same-iteration dispatcher retry from deferred work. |
| `dimension-reviewer` instructions do not reliably act on `task.resume` | Preset instructions include explicit retry subsection and a concrete example payload; contract test asserts the instructions contain the required markers. CLI precheck provides same-process backpressure for immediate retry. |
| Adding `task.resume` to `dimension-reviewer.triggers` creates unwanted wake-ups for unrelated resumes | Targeted routing uses `triggered: "dimension-reviewer"`; only resumes explicitly targeted at this hat wake it. Instructions further gate on `reason=dimension_mismatch`. |
| `task.resume` payload does not satisfy preset schema | U5 explicitly requires `reason` and `target_hat` fields and a `triggered` JSONL field; U8 includes a schema-validation test. |
| Mismatched event counted as received in `WaveContext` or `ReviewStepTracker` | U4 converts mismatches to `WaveFailure` before recording results; U8 tests assert `missing_dimensions` excludes failed slots. |
| Preset lint rejects new `task.resume` trigger | `task.resume` is already in `LOOP_RUNNER_INTERNAL_TOPICS` after 2026-06-16 U5; run `ralph preset check` to confirm. |
| CLI precheck bypassed by agent writing directly to per-worker events file | Merge-time validation (U4) is the required second gate; both gates must be implemented. |

---

## Documentation / Operational Notes

- Update `crates/ralph-core/data/ralph-tools.md` to document `RALPH_WAVE_DIMENSION` for wave workers.
- Update `presets/zh/ce-executor-isolated-zh.yml` in lockstep with English preset changes.
- No runbook or operational rollout steps required; this is a runtime behavior change activated by the preset and dispatcher.

---

## Sources & References

- **Origin document:** [docs/brainstorms/2026-06-17-wave-dimension-assignment-enforcement-requirements.md](docs/brainstorms/2026-06-17-wave-dimension-assignment-enforcement-requirements.md)
- **Related diagnosis:** [docs/report/2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md](docs/report/2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md)
- **Institutional learnings:**
  - [docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md](docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md)
  - [docs/solutions/integration-issues/ce-executor-wave-emission-must-batch-in-single-emit-2026-06-09.md](docs/solutions/integration-issues/ce-executor-wave-emission-must-batch-in-single-emit-2026-06-09.md)
- **Key code paths:**
  - `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`
  - `crates/ralph-cli/src/loop_runner/wave/io.rs`
  - `crates/ralph-core/src/wave_prompt.rs`
  - `crates/ralph-core/src/wave_context.rs`
  - `crates/ralph-cli/src/policy_check.rs`
  - `crates/ralph-cli/src/commands/emit.rs`
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/event_loop/rejection.rs`
  - `crates/ralph-core/src/diagnosis/envelope.rs`
  - `presets/en/ce-executor-isolated.yml`
