---
title: "ce-executor review-coordinator wave emission must batch all dimensions into a single `ralph wave emit` call"
date: 2026-06-09
category: integration-issues
module: ralph-presets
problem_type: configuration_drift
component: presets
severity: high
symptoms:
  - "TUI/log shows N consecutive 'WAVE: 🔬 Dimension Reviewer | 1 workers | timeout 1800s' segments instead of one '9 workers' segment"
  - "9–10 dimension reviews serialize back-to-back; wall time ≈ N × single-worker cost (~30 min observed for N=9)"
  - "Each `ralph wave emit` call in the events file has a different `wave_id` with `wave_index: 0, wave_total: 1`"
  - "`dimension-reviewer` `concurrency: 9` is configured but never produces > 1 concurrent worker"
root_cause: wrong_orchestration_instructions
resolution_type: preset_text_fix
tags:
  - ce-executor
  - wave-emit
  - dimension-reviewer
  - review-coordinator
  - concurrency
  - preset-orchestration
related_components:
  - presets/en/ce-executor.yml
  - presets/zh/ce-executor-zh.yml
  - ralph-core/wave_detection.rs
  - ralph-cli/loop_runner/wave/dispatcher.rs
---

# ce-executor review-coordinator wave emission must batch all dimensions into a single `ralph wave emit` call

## What broke

A field trace on worktree `2026-06-09-001-feat-managed-agent-doc-blocks-plan-bold-lotus`
(showing 9–10 WAVE segments, each with 1 worker, in `.ralph/events-20260609-074332.jsonl`)
revealed that `ce-executor` was producing **N independent `wave_total=1` waves** instead
of **one `wave_total=N` wave**, despite `dimension-reviewer` being configured with
`concurrency: 9`. Wall time for a 9-dimension review ballooned from the expected
~max(worker_time) to ~Σ(worker_time) (≈ 30 min observed).

## Full causal chain

1. **Preset instruction wording (root cause).**
   `presets/en/ce-executor.yml` Wave Emission section (line 587 pre-fix) read:
   ```yaml
   ### Wave Emission
   - Use `ralph wave emit` for each selected dimension
   ```
   The same anti-pattern lived in the Chinese variant
   (`presets/zh/ce-executor-zh.yml`, "Wave 发射" section).

2. **LLM agent follows literal instructions.** The review-coordinator agent (an LLM
   reading the preset) interpreted "for each selected dimension" as a loop and called
   `ralph wave emit` N times, once per dimension, with a single `--payloads` argument
   per call.

3. **`ralph wave emit` with one payload produces a `wave_total=1` wave.**
   `crates/ralph-cli/src/wave.rs:write_wave_events_with_provenance` (defined at
   line 152, body spans 152-200) computes `let total = payloads.len() as u32;` at
   line 164 and serializes it into the JSON record as `wave_total: total` near
   line 184. Each invocation produced one event with
   `wave_id=unique, wave_index=0, wave_total=1`. Observed:
   ```json
   {"topic":"review.wave.ready","wave_id":"w-18b75b0e538e8e04-1454894-0","wave_index":0,"wave_total":1, ...}
   {"topic":"review.wave.ready","wave_id":"w-18b75b10358902d2-1454988-0","wave_index":0,"wave_total":1, ...}
   {"topic":"review.wave.ready","wave_id":"w-18b75b10efb576f2-1455046-0","wave_index":0,"wave_total":1, ...}
   ```
   (10 such events on the trace.)

4. **`detect_all_wave_events` groups by `wave_id` and trusts the caller's batching.**
   `crates/ralph-core/src/wave_detection.rs:171-190` groups events by `wave_id`. With
   10 different `wave_id`s, the function returned 10 separate `DetectedWave`s, each
   with `total=1`.

5. **`try_build_wave` enforces batch-size = wave_total.**
   `wave_detection.rs:96-104` rejects any wave where
   `wave_events.len() != wave_total`. With 10 separate N=1 groups, none of them
   collides with this rule (1 event with `wave_total=1` is well-formed); the
   "concurrency: 9" intent of the hat config is silently dropped because each
   `DetectedWave.events.len() == 1`.

6. **Dispatcher renders one WAVE segment per `DetectedWave` and spawns 1 worker.**
   `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:120-134` prints
   `── WAVE: {name} | {total} workers | timeout {N}s ──` per wave. With `total=1`,
   the log becomes 9-10 segments each showing "1 workers". The semaphore inside
   `execute_wave` is sized to `hat_config.concurrency` (9), but only 1 event is
   iterated, so only 1 worker spawns.

7. **Outer `for detected in waves` loop in `handle_wave_events` serializes the 9
   `DetectedWave`s back-to-back** (`dispatcher.rs:70`). Total wall time =
   Σ(per-wave time), not max(per-wave time).

## Why existing tests didn't catch this

- `wave_detection.rs` unit tests cover `wave_total` consistency and single-wave
  detection. They assume the caller batches correctly.
- `dispatcher.rs` renders whatever wave shape it gets. The `1 workers` output is
  technically correct given the input — it's the input shape that's wrong.
- `ralph-cli/src/wave.rs:test_write_wave_events_single_payload` explicitly verifies
  that single-payload `ralph wave emit` produces `wave_total=1`. The CLI behavior is
  the contract; the issue is the *caller's* use of the CLI.

There was no contract test asserting that preset orchestration instructions
steer the caller toward batched emit.

## The fix

`presets/en/ce-executor.yml` Wave Emission section now reads:

```yaml
### Wave Emission
- **HARD RULE — emit ALL selected dimensions in ONE wave call.** Collect every
  selected dimension's payload into a list, then run exactly one
  `ralph wave emit review.wave.ready` with the full list (either
  `ralph wave emit review.wave.ready --payloads '<json1>' '<json2>' ... '<jsonN>'`
  or pipe via `printf '%s\n' '<json1>' ... '<jsonN>' | ralph wave emit review.wave.ready --payloads-stdin`).
  This produces ONE wave with `wave_total=N`, so `dimension-reviewer`'s
  `concurrency: 9` actually runs N workers in parallel.
- **DO NOT call `ralph wave emit` once per dimension.** That produces N
  independent waves with `wave_total=1` each. The dispatcher will then
  serialize them back-to-back (wall time ≈ N × single-worker cost),
  completely defeating `concurrency: 9`. Field trace 2026-06-09
  (worktree 2026-06-09-001): this anti-pattern turned a 9-worker
  review into ~30 minutes of sequential 1-worker waves.
- Each payload MUST include: ... (unchanged)
```

The Chinese variant (`presets/zh/ce-executor-zh.yml`) is updated in lockstep with
the equivalent 硬性规则 wording.

## Regression tests added

Two new tests in `crates/ralph-cli/src/presets.rs`:

- `test_ce_executor_review_coordinator_must_batch_wave_emission` — loads the root
  `presets/en/ce-executor.yml`, parses the `review-coordinator` hat's `instructions`
  block, and asserts:
  1. The anti-pattern "for each selected dimension" is **absent**.
  2. The instructions contain a batched-emit marker
     (one of: `single`, `one`, `batch`, `--payloads`).
- `test_ce_executor_zh_review_coordinator_must_batch_wave_emission` — same shape for
  the Chinese variant (asserts absence of "对每个选中的 dimension 发射" and presence
  of `一次` / `单个` / `一次性` / `--payloads`).

Both tests follow the existing convention in `presets.rs`
(`test_ce_executor_has_hard_commit_cadence`, `test_ce_executor_zh_*`).

## Why not also fix this at the dispatcher / detector layer

A defensive dispatcher could merge N same-topic N=1 waves into a single N=N wave
before dispatch. We deliberately did not do this because:

1. It would silently mask caller misuse, making the next "for each" regression
   produce a working but **invisible** performance cliff (3× slower instead of
   9× slower, with no diagnostic to find it).
2. The contract between caller and dispatcher is "one `wave_id` = one parallel
   batch". Auto-merging would change that contract and break the existing
   `test_detect_all_returns_multiple_waves` use case (where independent multi-wave
   batches are *intentional*, e.g. review-coordinator retrying after an empty
   payload — see the doc comment on `detect_all_wave_events`).
3. The fix at the preset level is the smallest correct change and matches the
   pattern already used by `ce-executor-wave.yml` (the execution side, which
   already uses `--payloads-stdin` correctly).

## How to apply this lesson elsewhere

When designing orchestration hat instructions that emit waves:

- **Always include a concrete CLI snippet** in the preset that shows the exact
  invocation pattern, including the flag name (`--payloads` or `--payloads-stdin`).
  LLM agents pattern-match from examples more reliably than from prose.
- **Use the word "HARD RULE" or 硬性规则** to mark the batching invariant — same
  convention as the existing `Commit Cadence (HARD RULE)` and
  `Preflight Contract (HARD RULE)` sections.
- **Cite the failure mode in the instruction itself** ("produces N independent
  waves with `wave_total=1`..."), so future agents that see the warning can connect
  the wording to the runtime symptom without external documentation.
- **Add a contract test** (preset text assertion) for any new orchestration
  invariant the LLM caller is responsible for upholding. Test should assert both
  the negative (forbidden phrasing) and the positive (required marker).

## Verification commands

```bash
# Unit tests for the new contract:
cargo test -p ralph-cli --bin ralph -- \
  presets::tests::test_ce_executor_review_coordinator_must_batch_wave_emission \
  presets::tests::test_ce_executor_zh_review_coordinator_must_batch_wave_emission

# Full preset test sweep (regression check):
cargo test -p ralph-cli --bin ralph -- presets::tests
```

## Related diagnostics

1. [[2026-06-08-ce-executor-review-wave-not-firing-diagnosis]] — 24h prior
   same-root-cause diagnosis. Identified `presets/en/ce-executor.yml:514-525`
   "Wave Emission" section (now L587 pre-fix) as the P0 root cause and
   prescribed the same `ralph wave emit --payloads` batched invocation.
   Different in that the trace there had `review-coordinator` skipping the emit
   entirely (0 `review.wave.ready` events); this trace has it emitting per-dimension
   (N=9 separate waves). Same fix site.
2. [[2026-06-05-wave-abort-root-cause-analysis]] — precedent for the
   "HARD RULE" convention in `dimension-reviewer` instructions
   (Hat Identity HARD RULE at L197). Establishes the wording pattern this fix
   re-uses for the wave-emission invariant.
3. [[2026-06-09-ce-executor-mechanism-vs-orchestration-diagnosis]] — same family
   orchestration gap (preset YML 弱约束 vs 兜底机制): the runtime safeguards
   (`task.resume` / `inject_fallback_event` / drift 写盘) are correct; the
   preset is the layer that needs hardening.
4. [[2026-06-08-002-fix-ce-executor-preset-forgot-close-step-guard-plan]] — same
   "HARD RULE 段 + 兜底 backpressure hint" pattern precedent for hardening a
   `ce-executor` instruction block. This wave-emit fix follows the same
   shape (mark invariant with HARD RULE, add a contract test).

