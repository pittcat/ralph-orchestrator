# Ralph Run Diagnosis

- loop: `2026-09-09-0917-fix-forge-dag-p1-closure-plan`
- preset: `builtin:ce-executor-pipeline`
- command: `ralph -H builtin:ce-executor-pipeline run --worktree --reuse-worktree --plan docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md -c ralph.pipeline.yml`
- diagnosis time: 2026-09-10
- history search: disabled by default; no prior unrelated runs were searched
- run status at diagnosis: still running; current event file has stopped at `stabilization.done.rejected`

## Executive finding

This run did **not** dispatch `test-stabilizer` before the accepted `work.done`. The accepted `work.done` payload contains all planned Units `U1` through `U15`, with no failed, blocked, or skipped Units. The event sequence is:

1. executor emitted `work.done.proposed`;
2. the runtime rejected that proposal at the worktree handoff gate;
3. the runtime resumed executor;
4. executor emitted a second `work.done.proposed` containing `completed_units: U1..U15`;
5. the precheck accepted it as `work.done`, which triggered `test-stabilizer`;
6. test-stabilizer emitted `stabilization.done.proposed`, which was rejected because its evidence was only a targeted test subset rather than the required full-suite evidence.

So the observed behavior is a retry/re-entry that may look like an early transition, not a test-hat transition before the accepted whole-plan completion.

There is nevertheless a real completeness weakness: the runtime independently verifies the worktree HEAD, dirty-state handoff, commit count, and that the payload has a `completed_units` list, but it does not independently prove each Unit's implementation evidence. The `completed_units` list is still executor-reported evidence. The current preset is a whole-plan executor; there are no per-Unit terminal events in this event stream.

The Git audit makes the weakness concrete: the current plan's U2 has no implementation commit. The only post-baseline commit whose subject says `U2` is `e7956357`, and that commit is a clippy/dead-code cleanup; its own diff adds `SKELETON-ONLY` markers and says the relevant modules are not wired into production callers. Several other Unit commits are also explicitly named `skeleton` (`U8`, `U9`, `U10`, `U11`, `U13`, `U14`, `U15`). Therefore the executor's aggregate `completed_units: U1..U15` claim is contradicted by the repository history and cannot be accepted as truthful completion evidence.

## Phase 0 inventory

The diagnosis bundle was present and valid enough to run both diagnostic modes:

- `ralph diagnose --legacy`: completed successfully.
- `ralph diagnose --causal`: completed successfully, but returned `status: not_evaluable` because the bundle has no orchestration boundary records and no feedback records.
- Runtime trace: present and monotonic.
- Event ledger: present at `.ralph/events-20260910-022854.jsonl`.
- Accepted-transition outbox: present at `.ralph/agent/accepted-transitions.jsonl`.
- Reuse artifacts: present under `.ralph/reuse-history/20260910T022854.655245999Z/`.

The missing orchestration artifact limits causal attribution, but the event ledger, runtime trace, preset, and source checks are sufficient to establish the ordering above.

## Four diagnostic questions

### 1. What was intended?

The preset defines executor as whole-plan execution and requires one subagent per Implementation Unit. Its `work.done` is the executor terminal event. Test-stabilizer is configured with `triggers: ["work.done"]`, so it is intended to start only after an accepted `work.done`.

References:

- `presets/en/ce-executor-pipeline.yml:2392-2424`
- `presets/en/ce-executor-pipeline.yml:3231-3242`

### 2. What actually happened?

The current event file contains:

- executor `work.done.proposed` with `completed_units` equal to `U1..U15`;
- `precheck-work.done` publishing accepted `work.done` with the same complete Unit list;
- test-stabilizer `stabilization.done.proposed`;
- `precheck-stabilization.done` publishing `stabilization.done.rejected`.

The rejected stabilization payload says it ran `cargo nextest run -p ralph-cli --bin ralph -- dag_scheduler`, reported `217/217`, and did not record `./scripts/run-tests.sh` or `RUSTFLAGS=-D warnings cargo check --workspace --all-targets`. That rejection is correct and is separate from Unit completion ordering.

The Git evidence is stronger than the executor payload: `git log 7ede210e..HEAD` contains no real current-plan U2 implementation commit. The candidate `e7956357` modifies lint/dead-code allowances and explicitly documents skeleton-only modules. The current HEAD also contains commits named `... skeleton` for U8, U9, U10, U11, U13, U14, and U15. A green `dag_scheduler` subset does not turn those skeletons into completed Units.

### 3. Where did behavior deviate?

The first executor completion did not pass the worktree handoff gate. `validate_work_done_handoff` compares the current HEAD and dirty fingerprint with the activation baseline. On mismatch it rejects the terminal event. The runtime then creates a recovery/resume for the executor; it does not dispatch test-stabilizer from the rejected proposal.

The likely concrete cause in this run is `--reuse-worktree`: the plan-reviewer reported six pre-existing but attributable dirty files, and the executor reconciled them. The executor activation baseline was captured before that reconciliation, so the dirty fingerprint changed during the activation. The retry started from the reconciled state and passed the handoff.

References:

- `crates/ralph-core/src/event_loop/worktree_handoff.rs:105-121`
- `crates/ralph-core/src/event_loop/parse_and_emit/legacy.rs:22-47`
- `crates/ralph-core/src/event_loop/parse_and_emit/step_dispatch.rs:265-283`
- `.ralph/review/2026-09-09-0917-fix-forge-dag-p1-closure-plan/reuse-guidance.md`
- `.ralph/agent/decisions.md`

### 4. Is the evidence complete enough to establish causality?

Partially. The missing `orchestration.jsonl` makes the causal diagnostic officially `not_evaluable`, and the process is still running. However, the accepted event order and source-level dispatch rules are unambiguous. The report cannot independently certify that every claimed Unit was actually completed because the current contract accepts the executor's aggregate Unit list rather than per-Unit proof.

## OPAC assessment

- **Outcome:** ordering is correct for this run; `work.done` preceded `test-stabilizer`. Stabilization itself is currently blocked/rejected for insufficient full-suite evidence.
- **Process:** the first executor handoff failed, causing an executor retry. The retry path is expected behavior, but its repeated appearance makes the run look as if it advanced prematurely.
- **Agent:** executor claimed all 15 Units complete; test-stabilizer overclaimed targeted-test evidence as full-suite evidence. The latter was caught by the stabilization precheck.
- **Context:** reuse-worktree carried six attributable dirty files into the activation. The baseline/handoff contract treats the reconciliation as an in-activation worktree change, so the first proposal failed.

## Root cause summary

1. **Perceived early test transition:** caused by confusing `work.done.proposed` / rejected handoff with accepted `work.done`. Rejected proposals do not trigger the downstream hat.
2. **Repeated executor retry:** caused by the worktree dirty-fingerprint mismatch after reuse-worktree reconciliation.
3. **Actual contract weakness:** Unit completion is aggregate, executor-authenticated evidence. Runtime does not verify one durable completion record per Unit before allowing `work.done`; in this run that allowed a false all-15 completion claim despite Git showing U2 absent and multiple Units still explicitly skeleton-only.
4. **Current follow-on failure:** test-stabilizer used targeted evidence and was correctly rejected; this is a separate validation failure, not proof that it started too early.

## Current run state

At report generation the Ralph process was still alive. The current event file ended at `stabilization.done.rejected`, with a recovery/resume expected for test-stabilizer. No final completion judgment is made while the process remains active.

## Recommended next investigation

If the intended invariant is “test-stabilizer must not start until every Unit is independently proven,” the relevant follow-up is to strengthen the `work.done` precheck/contract with per-Unit evidence or a durable Unit completion projection. This diagnosis does not implement that change.
