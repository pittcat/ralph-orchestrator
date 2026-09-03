2026-09-03T02:56:40Z step 2.5b: resolved_baseline_sha=33a126b0fe19acc798b1b5245be11aec834fb252 source=git-log-reverse plan_key=plans-2026-09-03-0959-feat-parallel-forge-runtime-dag-scheduler-plan

## Executor — 2026-09-03 parallel-forge-runtime-dag-scheduler-plan

**Activation start**: 2026-09-03
**Plan**: `docs/plans/2026-09-03-0959-feat-parallel-forge-runtime-dag-scheduler-plan.md`
**Trace**: `.ralph/review/2026-09-03-0959-feat-parallel-forge-runtime-dag-scheduler-plan/trace.md`
**resolved_baseline_sha**: `33a126b0fe19acc798b1b5245be11aec834fb252`

### Step 1.25 — Re-entry reconciliation
- `git status --porcelain` (excluding `.ralph/`) → empty
- `decisions.md` only has prior reviewer step 2.5b line
- `git log 33a126b0..HEAD` → empty
- → no attributable dirty paths; continue normally with first-run baseline.

### Step 1.5 — Flow-Audit Cross-Check
- `flow_audit == "first_run"` → **not** skip path; enter Execution Loop.

### Step 1.75 — Baseline Verification Snapshot
- Wrote `.ralph/review/2026-09-03-0959-feat-parallel-forge-runtime-dag-scheduler-plan/baseline-verification.md`.
- `cargo check -p ralph-cli --bin ralph` green.
- `git status --porcelain` (excluding `.ralph/`) → empty.
- Baseline feature gap: all 10 Units are greenfield or partial-scaffold (Explore survey verified `scheduler_mode`, `DagScheduler`, `DagSchedulerDriver`, `runtime_job`, `JobKernel`, `forge.unit.integrated` absent everywhere outside plan doc).
- Total fresh LOC ~6,000-11,000 + v13 migration + preset/schema/BDD/E2E/doc churn.
- `baseline_verification_status: green` (build-only; full-suite gate is U10's responsibility per plan §7).
- Per-Unit subagent dispatch (1 subagent = 1 U-ID) is the natural unit; full plan walk is not realistic in a single activation — plan §7 strict serial dependency graph.

### Planned U-ID walk
U1 → U2 → U3 → U4 → U5 → U6 → U7 → U8 → U9 → U10 (strict serial)

### Scope decision (recorded for emission honesty)
- Per executor HARD RULE "Finish-the-plan / in-activation retry": must attempt every U-ID end-to-end.
- Plan §7 is strict serial (U_i code depends on U_{i-1}); no parallelism in the Unit dispatch chain.
- Per-Unit subagent budget (~30k tokens) is materially smaller than the per-Unit LOC estimate (~200-2000 LOC + tests). Realistic outcome: some Units complete with commit; others attempt and fail their RED→GREEN→commit cycle.
- Settlement target: emit `work.done` (proposed) with `execution_status: partial` once ≥1 U-ID commits; `work.failed` only if the full walk delivers zero commits.

executor checkpoint: U1 committed=a74928f217b63334f05efb1d8f9b1a0f5639e5e4 unit_tests=cargo nextest run -p ralph-core -- scheduler_mode && cargo nextest run -p ralph-cli --bin ralph -- dag_scheduler_mode remaining=U2,U3,U4,U5,U6,U7,U8,U9,U10
executor checkpoint: U2 committed=819a82f3 unit_tests=cargo nextest run -p ralph-core -- parallel_forge_handoff (21/21 pass; clippy clean; fmt clean; cargo check -p ralph-core clean) remaining=U3,U4,U5,U6,U7,U8,U9,U10
executor checkpoint: U3 committed=421b3f2f unit_tests=cargo nextest run -p ralph-core -- dag_store dag_plan_receipt (15/15 pass; cargo check -p ralph-core --all-targets clean; cargo fmt -p ralph-core clean) remaining=U4,U5,U6,U7,U8,U9,U10
executor checkpoint: U4 committed=7026c047 unit_tests=cargo nextest run -p ralph-core -- supervisor::dag_scheduler (10/10 pass; cargo fmt -p ralph-core clean; cargo check -p ralph-core --all-targets clean; clippy pre-existing warn in ralph_config.rs:186 unrelated) remaining=U5,U6,U7,U8,U9,U10
executor checkpoint: U5 committed=d18a774f unit_tests=cargo nextest run -p ralph-core -- supervisor::dag_shadow supervisor::dag_inspect (7/7 pass; cargo fmt --all --check clean; cargo check -p ralph-core --all-targets clean; 494 LOC new+modified <=500 cap) remaining=U6,U7,U8,U9,U10

### U2 — known scenario-test regression
- `test_parallel_forge_task_dispatch_runtime` (in `crates/ralph-core/tests/scenarios.rs`) fails after U2 strict `target_branch` validation is wired into `derive_plan_handoff`.
- Root cause: scenario fixture `crates/ralph-core/tests/scenarios/parallel_forge_task_dispatch_runtime.yml` ships v1-shaped units with no `target_branch` field; U2 validator rejects the plan at `EmptyTargetBranch` and `forge.plan.ready` is never accepted by the runtime.
- Verified by `git stash` of U2 changes: test passes without U2, fails with U2.
- Per U2 hard constraint (do NOT touch scenario/E2E files), the scenario YAML is intentionally not modified by this commit. It is a downstream migration item: a future Unit (or operator-approved plan delta) must add `target_branch: feat/<id>` to each unit in that fixture, mirroring what U2 did for the five existing inline fixtures inside `parallel_forge_handoff.rs`.
- Targeted parallel_forge_handoff tests (21) all pass; production fixtures (`presets/templates/parallel-forge/*.template.yml`) already carry the new fields.
