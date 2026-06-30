> ⚠️ **SUPERSEDED 2026-06-28 by plan `docs/plans/2026-06-28-005-refactor-remove-human-guidance-topic-plan.md`**:
> The `human.guidance` topic was physically deleted from the runtime
> (no external operator channel exists in this build of
> ralph-orchestrator). The recovery patterns described in this
> document — injecting a `human.guidance` event so the operator
> can intervene between iterations, and tracking the operator's
> response on the loop's pending queue — are no longer applicable.
>
> Plan 2026-06-28-005 redirects the 3-strike escalation output to
> `plan.blocked(reason=correction_3_strike_exhausted)` so the
> shipper / reporter chain runs the preset's failure path; the
> drift Warning + Final hint is routed to
> `TerminationReason::RecoveryExhausted` directly without an
> intermediate guidance publish. Historical diagnoses
> (`merry-lotus`, `noble-peacock`, etc.) are preserved below for
> context, but the recovery mechanisms they describe have been
> replaced.

---
title: "ce-executor-serial noble-peacock review chain stall: end-to-end BDD + smoke replay validation"
date: 2026-06-17
category: integration-issues
module: crates/ralph-core + crates/ralph-cli
problem_type: integration_issue
component: test_validation
symptoms:
  - "noble-peacock run (2026-06-17, worktree 2026-06-10-003-noble-peacock) terminated at iter 5 with ralph hat emitting loop.cancel after dimension-reviewer produced 0 events on review.dimension.ready(correctness)"
  - "no targeted regression test exists for the serial review chain with silent DR + recovery; the only baseline scenario tests the happy 4-dim walk"
  - "no smoke replay fixture captures the noble-peacock wire-level shape (ready → silence → resume → done → loop.cancel); future regressions of the same shape will only be caught by full e2e"
  - "ralph-cli tests for missing_event_gate were not exercised end-to-end against the ce-executor-serial preset; flake risk for the 500ms-iteration clock class was not measured"
root_cause: missing_regression_test
resolution_type: test_addition
severity: high
tags:
  - ce-executor-serial
  - noble-peacock
  - bdd
  - smoke-replay
  - regression
  - recovery
---

# ce-executor-serial noble-peacock E2E validation (plan 004 U6)

## Problem

The noble-peacock run (2026-06-17, 28m 45s, 6 iterations, 7 events)
terminated at iter 5 with `loop.cancel` after `dimension-reviewer`
produced zero events on `review.dimension.ready(correctness)`. Root
causes for the recovery gap are documented in the plan
(`docs/plans/2026-06-17-004-fix-ce-executor-serial-noble-peacock-review-chain-plan.md`)
and the diagnostic report. This solutions entry covers the **U6
validation layer** that pins the post-fix behavior at the wire level.

Without U6, future regressions of the same shape (silent DR, missing
`stage` field, `review.passed` leaking into jsonl) will only be caught
by full e2e runs against a real LLM backend. The fix units (U1-U5)
are correct, but without targeted regression tests they can be
re-introduced by an innocuous refactor.

## Symptoms (Pre-Fix)

- `review.passed` from `executor` with `skip_reason=aggregate_timeout`
  landed in `events.jsonl` (noble-peacock `events-20260617-095504.jsonl:3-4`).
- `task.resume` payload was missing the `stage` field, breaking
  `field_completeness=0%` in the drift monitor.
- 49s after `review.dimension.ready(correctness)`, `missing_event_gate`
  injected `task.resume` routing to `dimension-reviewer`, but the
  recovery path produced no `review.dimension.done` — DR was never
  re-activated with the original `review.dimension.ready` trigger
  context.
- `recovery_count` in `diagnosis-summary.json` was hard-coded to `0`,
  ignoring the 26 cli_emit envelopes already in `.ralph/recovery.jsonl`.
- The single BDD scenario for serial review
  (`ce_executor_serial_review.yml`) tested only the happy 4-dim walk;
  there was no scenario that exercised a silent first-activation DR.

## Solution (U6 = 4 Steps)

### 1. Noble-peacock smoke replay fixture

New directory
`crates/ralph-core/tests/fixtures/noble-peacock-review-stall/`:

- `replay.jsonl` — 8 hand-crafted JSONL events capturing the exact
  wire-level shape of the noble-peacock run (work.start → work.ready
  → work.done → `review.passed` 越权 → `review.dimension.ready` →
  [silence] → `task.resume` → `review.dimension.done` → `loop.cancel`).
  This is a **synthetic** fixture (not a real recording) — the README
  in the directory makes this explicit.
- `topology.yml` — minimal isolated-mode topology matching the
  ce-executor-serial hat layout (coordinator, executor,
  review-coordinator, dimension-reviewer).

Three smoke-runner tests in `smoke_runner.rs` pin the fixture:

- `test_noble_peacock_replay_fixture_exists` — guard against accidental
  deletion of the fixture.
- `test_noble_peacock_replay_fixture_contains_critical_topics` —
  asserts the wire-level ordering (work.start < work.ready < work.done
  < review.dimension.ready < task.resume < review.dimension.done) is
  preserved.
- `test_noble_peacock_replay_fixture_runs_without_panic` — smoke
  runner can read the fixture without unrecoverable parse errors.

### 2. Silent DR recovery BDD scenario

New file
`crates/ralph-core/tests/scenarios/ce_executor_serial_review_silent_reviewer_recovers.yml`:

- 18 iterations, same topology as `ce_executor_serial_review.yml`
- iter 4 is the **silent turn** (empty mock response, no event
  emitted) — this is the noble-peacock failure shape
- iter 5 is the **recovery turn** where `dimension-reviewer` emits
  `review.dimension.done` with the original `correctness` dimension
  context preserved

Test function in `scenarios.rs`:
`test_ce_executor_serial_review_silent_reviewer_recovers_scenario`.

This is the first BDD scenario that exercises the
`last_activation_events` replay contract (U3) at the wire level.

### 3. Noble-peacock review.passed never-lands regression test

New test in `integration_emit_policy.rs`:
`test_noble_peacock_executor_review_passed_never_lands`.

Three assertions, all derived from the diagnostic report:

1. CLI exits non-zero with an actionable reason code
   (missing_provenance / isolated_scope_violation / topic_denied /
   invalid_field_value / skip_reason / allowed_values — any of these
   is acceptable as long as the agent receives backpressure)
2. `events.jsonl` does NOT contain the rejected `review.passed` event
3. `events.jsonl` does NOT contain the `aggregate_timeout` payload

This is the U1 (CLI provenance fail-closed) regression guard for the
exact payload shape that caused the noble-peacock leak. If this test
ever passes with `status.success() == true` or with `events.jsonl`
containing the payload, the noble-peacock P0-1 leak has been
re-introduced.

### 4. Verification command block (T6.1–T6.4)

| ID | Command | What it proves |
|----|---------|----------------|
| T6.1 | `cargo nextest run -p ralph-core --test scenarios ce_executor_serial` | BDD scenarios for both happy and silent-recovery variants pass |
| T6.2 | `cargo nextest run -p ralph-core --features recording --test smoke_runner -- noble_peacock` | Synthetic fixture can be replayed by the smoke runner without panic |
| T6.3 | `cargo nextest run -p ralph-cli --bin ralph -- missing_event` (×3) | No flake on the `missing_event_gate` test class; U2 defer contract holds under load |
| T6.4 | `./scripts/run-tests.sh` (conditional) | Full workspace passes — only meaningful after U1-U5 are all committed (currently U1-U3 are committed, U4-U5 are not) |

## What Didn't Work

- **Tying the smoke fixture to a real recording**: the noble-peacock
  worktree's `.ralph/events-20260617-095504.jsonl` contains 7 events
  but is 18KB of mixed business + recovery envelopes. A real
  recording would require anonymization, schema validation, and a
  100+ event fixture for the full chain. The synthetic 8-event fixture
  captures only the load-bearing shape (silent-DR + recovery) at
  ~600 bytes.
- **Asserting the fixture drives the full smoke runner to completion**:
  the noble-peacock wire shape is a failure scenario (loop.cancel at
  the end), not a success scenario. The test asserts the fixture is
  well-formed and can be parsed, not that the runner reaches
  `LOOP_COMPLETE`.
- **Testing the recovery path in isolation**: the silent-DR scenario
  relies on `last_activation_events` replay to carry the
  `review.dimension.ready` context forward. Testing that
  independently requires a unit test on the runner state, which is
  out of scope for U6 (the plan defers the unit-level test to
  `ce_executor_recovery.rs`).

## Why This Works

The three additions cover three different validation surfaces:

1. **Smoke fixture (smoke_runner.rs)** — proves the wire-level shape
   of the failure mode is captured and can be re-replayed. If the
   fixture breaks (missing topics, reordered events), a future
   regression in the recording toolchain or a hand-edit will be
   caught.

2. **BDD scenario (scenarios.rs)** — proves the scenario runner can
   handle a silent DR turn without the loop dying. The `run_scenario`
   harness drives the mock backend through the full chain including
   the silent turn, and the `expected.events` assertion in the YAML
   fires if any topic is dropped or reordered.

3. **Integration test (integration_emit_policy.rs)** — proves the
   CLI boundary rejects the specific noble-peacock payload shape at
   the gate, not at runtime. This is the only one of the three that
   tests the **fix** (U1 provenance fail-closed) rather than the
   **shape** (U2/U3 recovery).

Together, they form a regression firewall: any future change to U1
provenance, U2 recovery defer, or U3 trigger replay that re-introduces
the noble-peacock failure mode will be caught by at least one of the
three tests, depending on which surface the regression touches.

## Prevention

- **Add a smoke fixture whenever a new failure mode is captured in a
  diagnostic report**. The fixture directory pattern
  (`tests/fixtures/<short-id>/{replay.jsonl,topology.yml}`) is
  reusable; the `noble-peacock-review-stall/` directory is a template
  for future fixtures (merry-lotus, etc.).

- **Add a BDD scenario variant whenever the happy path is already
  covered but a failure shape is not**. The
  `ce_executor_serial_review_silent_reviewer_recovers.yml` variant is
  a template: same topology, one silent iteration injected in the
  middle, same `expected.events` wire contract.

- **For any `ralph emit` payload shape that leaked into
  `events.jsonl` in a real run, add a `test_<run>_<hat>_<topic>_never_lands`
  test in `integration_emit_policy.rs`**. The
  `test_noble_peacock_executor_review_passed_never_lands` test is a
  template: literal payload from the diagnostic report, three
  assertions (exit code, jsonl absence, payload field absence).

- **When the fix unit has a documented failure mode in a real run,
  the fix MUST be guarded by an integration test that uses the same
  payload shape**. No U1 fix is complete without a
  `test_*_never_lands` regression test.

## Related Issues

- `docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md` —
  the diagnostic report that drives this solutions entry. The smoke
  fixture and the integration test use the literal payload shapes
  documented there (`plan_name="p"`, `task_id="t"`,
  `skip_reason="aggregate_timeout"`).
- `docs/plans/2026-06-17-004-fix-ce-executor-serial-noble-peacock-review-chain-plan.md` —
  the plan that introduces U6 and the U1-U5 fix units. U6 is the
  validation layer for U1-U5; without U6, the fix units lack
  regression coverage.
- `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md` —
  the U1-U5 fix units (merry-lotus) that this solutions entry
  validates. U1 (check_isolated_scope + check_emit_provenance) is the
  fix that the `test_noble_peacock_executor_review_passed_never_lands`
  test guards.
- `docs/achieved/plan/2026-06-17-003-fix-ce-executor-serial-precheck-recovery-gates-plan.md` —
  the previous plan (U0-U5 Runtime Diagnosis) that this solutions
  entry builds on. U6 is a follow-up validation layer.
- `docs/achieved/plan/2026-06-17-002-feat-ce-executor-serial-review-plan.md` —
  the plan that introduced the `ce-executor-serial` preset and the
  original BDD scenario (`ce_executor_serial_review.yml`). U6 extends
  that scenario with the silent-DR variant.
