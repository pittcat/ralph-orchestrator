---
title: "ce-executor-serial 30-day 6th recurrence — auto_handoff_prepare wrote 1-segment filename dead on arrival"
date: 2026-06-23
category: developer-experience
module: ralph-core
problem_type: mechanism_regression
component: hat_handoff
symptoms:
  - "coordinator dead-letter: 3 consecutive task.resume — emitting plan.blocked (kind=hat_handoff_filename_mismatch count=3)"
  - "lint auto_prepare writes `auto_work_ready.md` (1 segment) that runtime `parse_filename` rejects"
  - "agent hand-fills stale `handoff_path` (e.g. `0-1-…` when loop is at iter=1)"
  - "recovery.jsonl only shows the most-recent 1-2 rejections; dead-letter cumulative count not visible"
root_cause: ssot_filename_shape_drift
resolution_type: ssot_filename_alignment
severity: high
tags:
  - ce-executor
  - ce-executor-serial
  - hat_handoff
  - auto_handoff_prepare
  - parse_filename
  - ssot
  - dead-letter
  - recovery.jsonl
  - 30-day-recurrence
related_components:
  - ralph-core
  - ralph-cli
  - presets
related_commits:
  - ac3a78ed
  - 9a403cb6
  - 58422696
  - 3350c0ab
  - "[pending — this fix]"
---

# ce-executor-serial 30-day 6th Recurrence

## Problem

`ce-executor-serial` preset 多次进入"3 consecutive task.resume — emitting plan.blocked" dead-letter path. Coordinator (re)emits `work.ready` with a `handoff_path` that the runtime gate rejects with `hat_handoff_filename_mismatch`, and after 3 consecutive same-kind rejections the loop emits `plan.blocked` and stalls. The most recent instance (2026-06-23, ralph-e2e loop `primary-20260623-062301`) reached `iter=3, sequence=3` on the ledger before stalling.

This is the **6th recurrence in 30 days** of the same root-cause family. The previous fixes (commits `ac3a78ed`, `9a403cb6`, `58422696`, `3350c0ab`) each closed a different entry point but never reached the underlying mechanism.

## Root cause

The mechanism has **two** cooperating layers that drifted apart:

1. **Linter auto-prepare** (`crates/ralph-core/src/preset/engine/linter.rs`): when the agent does not hand-fill `handoff_path`, the linter calls `auto_handoff_prepare` to write the handoff artifact. The pre-fix implementation used `write_artifact` with a hard-coded `auto_{topic}.md` filename — **1 dash, 1 segment**.

2. **Runtime gate** (`crates/ralph-core/src/hat_handoff/gate.rs`): the gate's `parse_filename` (delegated to `allocator::parse_filename`) requires the filename to be **4 dash-separated segments** (`{iter}-{seq+1}-{from}-{to}.md`).

The two layers are **incompatible by construction**: the linter writes a 1-segment file, the gate rejects 1-segment filenames. Every lint-auto-prepared handoff was dead on arrival.

The agent-hand-fills path was supposed to be the rescue: when the agent fills `handoff_path` itself, the linter's `!has_handoff_path(payload)` short-circuit (linter.rs:280, pre-fix) skips auto-prepare and the agent's hand-filled path goes straight to the gate. But the agent cannot know the loop's true iter/seq (it has no live state — the `RALPH_LOOP_ITERATION` / `RALPH_HAT_HANDOFF_SEQ` env vars are only injected to the **loop runner's** backend subprocess, not to agents re-emitting through `task.resume`). The agent's hand-fills are therefore often stale or wrong, and the gate's `read_handoff_ssot_first` rescue (`gate.rs:154`) only works when the SSOT-derived file already exists on disk — which requires the linter to have written it, which requires the agent to have NOT hand-filled.

The recovery.jsonl file showed only the latest 1-2 rejections (rotation, plus the dead-letter cumulative `count=3` only went to stdout, never to recovery.jsonl). Operators triaged on the wrong symptoms ("two `hat_handoff_structure_invalid` rejections, just fix the structure") and missed the real dead-letter condition.

## Resolution

This fix (commit pending) makes **three** minimal mechanism changes:

### Fix 1: `auto_handoff_prepare` writes the SSOT 4-segment filename

`crates/ralph-core/src/preset/engine/linter.rs`:

- `auto_handoff_prepare` (called from `lint_emit` on macro edges with no `handoff_path` in payload) now reads `RALPH_LOOP_ITERATION` / `RALPH_HAT_HANDOFF_SEQ` / `RALPH_CURRENT_HAT` env vars (injected by `loop_runner` per `runner.rs:2931-2938`) and computes the SSOT filename via `allocator::compute_filename(iter, current_seq+1, from, to)`. Falls back to `0` / `"unknown"` when env is missing (coordinator mode, tests).
- Refactored `write_artifact` into a thin wrapper over a new `write_artifact_with_name` that accepts an explicit filename. The legacy 1-segment `auto_{topic}.md` path is preserved as the wrapper's default but is no longer reached by the linter's macro-edge path.
- The `!has_handoff_path(payload)` short-circuit (linter.rs:280) is **kept**. The fix only changes what the linter writes when it does fire.

### Fix 2: dead-letter persistence to recovery.jsonl

`crates/ralph-core/src/event_loop/mod.rs`:

- In the `CoordinatorAction::PlanBlocked` branch (dead-letter path), persist a `task_resume_dead_letter` envelope to `.ralph/recovery.jsonl` via the SSOT `RecoveryDiagnosisEnvelope::builder()` (in addition to the existing stdout WARN). Reason code `task_resume_dead_letter` is **distinct** from the gate's `hat_handoff_filename_mismatch` so downstream tools can filter the terminal state separately from the per-iteration rejections.
- `recovery.jsonl` write failures now log via `tracing::warn!` (target `ralph::event_loop::dead_letter`) instead of `let _ =`, so the operator sees when the audit is missing.
- Source is `DiagnosisSource::LoopStale` (the closest existing variant for "loop state went wrong"; the absence of a dedicated `EventLoop` variant is a minor schema gap, see Followups below).

### Fix 3: coordinator hat instructions stop telling the agent to compute filenames

`presets/en/ce-executor-serial.yml`:

- The "Task Resume Reception" section (the only place where the coordinator is told to recover from a `task.resume`) previously instructed the agent to call `ralph tools handoff prepare` and re-emit with the SSOT-derived filename. The fix replaces that with: **"Do NOT include `handoff_path` in the payload; `ralph emit` derives it from the loop's iter/seq"** — letting the linter's fix-1 path run.

### Fix that was attempted and rolled back: hard CLI guard

A hard CLI guard (`emit.rs`) rejecting hand-filled `handoff_path` was **implemented and then removed**. Rationale for the rollback:

- The guard broke the integration test
  `tests/integration_emit_policy.rs::test_emit_ce_executor_serial_executor_can_emit_work_done`
  (and 29 other tests), which legitimately exercise the
  "agent used `ralph tools handoff prepare` to compute the path, then
  passed it through" path. That is a valid re-emit flow.
- An escape hatch (`RALPH_ALLOW_MANUAL_HANDOFF_PATH=1`) was added and
  also removed: bypassing the guard for tests is the same shape as the
  "agent bypasses lint" pattern that produced the original bug. It
  would have hidden the next regression.
- The actual root cause was the linter's broken filename shape, not
  the agent's hand-fill. Fix 1 covers both: the agent's hand-fill
  goes through the runtime gate's `read_handoff_ssot_first` rescue
  (already present, `gate.rs:154`), and the linter's no-fill path
  produces a valid SSOT file. The guard was solving a non-problem.

Lesson: **a "guard" that requires an escape hatch to keep the
existing tests passing is a guard that solves the wrong problem.**

## Regression tests

Three new tests pin the fixes:

- `crates/ralph-core/src/preset/engine/linter.rs::tests::cb6_write_artifact_with_ssot_filename_creates_parseable_artifact` — confirms the new `write_artifact_with_name` path produces a file that `parse_filename` accepts and round-trips `(iter, seq, from, to)` correctly.
- `crates/ralph-core/src/preset/engine/linter.rs::tests::cb6_write_artifact_rejects_legacy_1segment_filename_shape` — pins the SSOT contract: `parse_filename` MUST reject any non-4-segment shape. If a future revert reintroduces `auto_{topic}.md`, this test documents the contract violation.
- `crates/ralph-core/src/event_loop/rejection.rs::tests::task_resume_consumer::dead_letter_envelope_carries_kind_and_count` — pins the `task_resume_dead_letter` envelope shape (reason_code, kind, count, source) for operators reading recovery.jsonl.

Existing test coverage that should also be checked:

- `crates/ralph-core/src/hat_handoff/gate.rs::tests::ssot_first_read_*` (line 936+) — confirms the `read_handoff_ssot_first` rescue handles stale agent hand-fills when the SSOT file exists on disk.

## Anti-pattern (for future)

The 30-day 6-recurrence pattern is now visible:

1. **Symptoms** (coordinator dead-letter, count=3) get filed.
2. **A fix** lands that closes one entry point (`prepare` defaults, `read_handoff_ssot_first`, etc.).
3. **The fix is called "mechanism closed loop"** in its commit message.
4. **A different entry point** (linter, gate, agent prompt, recovery.jsonl) re-introduces the same symptom within days.

The fix that breaks the pattern: **a single invariant — the artifact written to disk MUST be the SSOT 4-segment filename**. The linter, the agent, and the gate must all converge on this invariant. Future fixes should test the invariant directly, not the per-entry-point symptom.

## Followups (not in this fix)

- **Delete the legacy `auto_handoff_prepare` function body** in `linter.rs:396-421`. It is now dead code (lint calls the inlined SSOT path) but `pub fn` still exists and may be picked up by future code or tests. Deletion is a separate commit because the existing test `p0_2_auto_prepare_lands_under_caller_workspace` exercises it.
- **Add `DiagnosisSource::EventLoop`** to the diagnosis enum for finer-grained recovery.jsonl filtering. Current code uses `LoopStale` as the closest match; an `EventLoop` variant would let operators distinguish loop-internal diagnostics from drift / staleness diagnostics.
- **Consider banning `ralph tools handoff prepare` from coordinator hats in 19 builtin presets** (cf. commit `19484eb` precedent for `kill self parent ralph`). The current fix removes the prompt instruction but does not enforce the ban at the CLI level.
- **Add a metric** `agents_still_hand_filling_handoff_path` to track whether the prompt change in fix 3 actually changes agent behavior. Without the metric, a future regression in agent behavior is invisible.
