# Hat Collections

Built-in hat collections are now intentionally small. Ralph ships a core working set of defaults and documents broader workflow ideas as examples instead of treating every pattern as a supported builtin.

## Quick Start

```bash
ralph init --backend claude
ralph init --list-presets

ralph run -c ralph.yml -H builtin:code-assist -p "Add user authentication"
```

## Supported Builtins

| Collection | Hats | Best for | Notes |
|---|---|---|---|
| `code-assist` | `planner`, `builder`, `critic`, `finalizer` | Default implementation work | Recommended default; adds fresh-eyes review and a final completion gate |
| `debug` | `investigator`, `tester`, `fixer`, `verifier` | Root-cause debugging | Strong on repro and fix verification |
| `research` | `researcher`, `synthesizer` | Read-only analysis | No code changes |
| `review` | `reviewer`, `analyzer` | Adversarial code review | No code changes |
| `pdd-to-code-assist` | multi-stage design + build pipeline | Idea to code | Advanced and fun, but slower and less predictable |

## Internal Presets

Ralph also keeps a few internal/testing presets available without advertising them in the normal list:

- `merge-loop`
- `hatless-baseline`

## Recommended Workflow

- Use `code-assist` for most implementation tasks.
- Use `debug`, `research`, or `review` when you need a specialized mode.
- Use `pdd-to-code-assist` when you specifically want an end-to-end exploratory workflow and are comfortable paying for extra iterations.

| Collection | Canonical source | Hats | Start event | Completion | Best for |
|---|---|---|---|---|---|
| `bugfix` | `presets/bugfix.yml` | `reproducer`, `fixer`, `verifier`, `committer` | `repro.start` | `LOOP_COMPLETE` (default) | Reproduce/fix/verify/commit bug workflow |
| `code-assist` | `presets/code-assist.yml` | `planner`, `builder`, `validator`, `committer` | `build.start` | `LOOP_COMPLETE` | TDD implementation from specs/tasks/descriptions |
| `debug` | `presets/debug.yml` | `investigator`, `tester`, `fixer`, `verifier` | `debug.start` | `DEBUG_COMPLETE` | Root-cause debugging and hypothesis testing |
| `deploy` | `presets/deploy.yml` | `builder`, `deployer`, `verifier` | `task.start` (default) | `LOOP_COMPLETE` | Deployment and release workflows |
| `docs` | `presets/docs.yml` | `writer`, `reviewer` | `task.start` (default) | `DOCS_COMPLETE` | Documentation writing and review |
| `feature` | `presets/feature.yml` | `builder`, `reviewer` | `task.start` (default) | `LOOP_COMPLETE` | Feature development with integrated review |
| `fresh-eyes` | `presets/fresh-eyes.yml` | `builder`, `fresh_eyes_auditor`, `fresh_eyes_gatekeeper` | `fresh_eyes.start` | `LOOP_COMPLETE` | Enforced repeated skeptical self-review passes |
| `gap-analysis` | `presets/gap-analysis.yml` | `analyzer`, `verifier`, `reporter` | `gap.start` | `GAP_ANALYSIS_COMPLETE` | Spec-vs-implementation auditing |
| `hatless-baseline` | `presets/hatless-baseline.yml` | _(none)_ | `task.start` | `LOOP_COMPLETE` | Baseline no-hat behavior for comparison |
| `merge-loop` | `crates/ralph-cli/presets/merge-loop.yml` | `merger`, `resolver`, `tester`, `cleaner`, `failure_handler` | `merge.start` | `MERGE_COMPLETE` | Internal merge/worktree automation |
| `pdd-to-code-assist` | `presets/pdd-to-code-assist.yml` | `inquisitor`, `architect`, `design_critic`, `explorer`, `planner`, `task_writer`, `builder`, `validator`, `committer` | `design.start` | `LOOP_COMPLETE` | Full idea → plan → implementation pipeline |
| `pr-review` | `presets/pr-review.yml` | `correctness_reviewer`, `security_reviewer`, `architecture_reviewer`, `synthesizer` | `task.start` (default) | `LOOP_COMPLETE` | Multi-perspective PR review |
| `refactor` | `presets/refactor.yml` | `refactorer`, `verifier` | `task.start` (default) | `REFACTOR_COMPLETE` | Incremental, verified refactoring |
| `research` | `presets/research.yml` | `researcher`, `synthesizer` | `research.start` | `RESEARCH_COMPLETE` | Exploration and analysis without code changes |
| `review` | `presets/review.yml` | `reviewer`, `analyzer` | `review.start` | `REVIEW_COMPLETE` | Review-only workflow |
| `spec-driven` | `presets/spec-driven.yml` | `spec_writer`, `spec_reviewer`, `implementer`, `verifier` | `spec.start` | `LOOP_COMPLETE` (default) | Specification-driven implementation |
| `wave-review` | `presets/wave-review.yml` | `coordinator`, `reviewer` (x3), `synthesizer` | `review.start` | `LOOP_COMPLETE` | Specialized parallel code review (wave-enabled) |
| `ce-executor` | `presets/ce-executor.yml` | `coordinator`, `executor`, `review-coordinator`, `dimension-reviewer` (wave), `review-synthesizer` (aggregate), `fixer`, `plan-gate`, `shipper`, `reporter` | `work.start` | `LOOP_COMPLETE` | Plan-driven execution with wave review, auto-fix, and manager report |

## Why The Builtin Set Is Small

Every builtin preset becomes product surface area:

- It must be documented.
- It must be tested and kept working.
- It must appear coherent in API and CLI listings.

Ralph now prefers a small supported set plus documentation examples for more experimental or niche orchestration patterns.

## Examples Instead Of Builtins

Historical workflow ideas such as spec-driven development, red-team review, mob programming, and fresh-eyes loops are now examples rather than shipped builtins. See:

- [Examples Index](../examples/index.md)
- [Spec-Driven Development Example](../examples/spec-driven.md)
- [Multi-Hat Workflow](../examples/multi-hat.md)

## Usage Examples

```bash
# Default implementation workflow
ralph run -c ralph.yml -H builtin:code-assist -p "Add OAuth login"

# Debugging
ralph run -c ralph.yml -H builtin:debug -p "Investigate why login fails on mobile"

# Research
ralph run -c ralph.yml -H builtin:research -p "Map the authentication architecture"

# Review
ralph run -c ralph.yml -H builtin:review -p "Review the changes in src/api/"

# Advanced/fun workflow
ralph run -c ralph.yml -H builtin:pdd-to-code-assist -p "Build a rate limiter"

# ce-executor plan-driven execution
ralph run -c ralph.yml -H builtin:ce-executor -p "docs/plans/my-plan.md"

# ce-executor with worktree isolation (recommended for parallel runs)
ralph run -c ralph.yml -H builtin:ce-executor --worktree -p "docs/plans/my-plan.md"
```

### ce-executor Workflow

`ce-executor` is a plan-driven execution preset with wave-based code review, auto-fix, and manager reporting.

**Key characteristics:**
- Does not auto-create feature branches (runs on current checkout)
- Records `start_sha` at startup to anchor review scope
- Uses wave parallelism for multi-dimensional code review
- Blocks all push operations (local commit only)
- Includes a `plan-gate` hat that reconciles review verdict against `plan.md` / `progress.md` and decides whether to advance to the next step or complete the plan

**When to use `--worktree`:**
- Multiple parallel ce-executor runs
- When you want isolation from main workspace changes
- When the plan might involve significant refactoring

**After a `--worktree` run:**
- The worktree directory (`.worktrees/<loop-id>/`) and branch (`ralph/<loop-id>`) are **preserved**.
- No automatic merge or cleanup happens. Use `git worktree remove` or `ralph loops diff/discard/merge` to handle it manually.
- `ralph loops list` may show it as `orphan` after the loop exits — this is expected.

```bash
# Single run (in-place execution)
ralph run -H builtin:ce-executor -p "docs/plans/my-plan.md"

# Isolated run (worktree, no branch creation)
ralph run -H builtin:ce-executor --worktree -p "docs/plans/my-plan.md"

# Isolated run with higher concurrency for review
ralph run -H builtin:ce-executor --worktree -c ralph.yml -p "docs/plans/my-plan.md"
```

## Common Workflow Patterns

Ralph built-ins usually follow one of these shapes:

### 1) Linear Pipeline
A fixed sequence of specialist hats.

Examples: `feature`, `bugfix`, `deploy`, `docs`

### 2) Critic / Actor Loop
One hat proposes, another critiques/validates, then iterates.

Examples: `spec-driven`, `review`, `fresh-eyes`

### 3) Multi-Reviewer + Synthesis
Parallel perspectives merged into one result.

Example: `pr-review`

### 4) Scatter-Gather (Waves)
One hat dispatches, parallel workers execute, an aggregator synthesizes.

Example: `wave-review`

See [Agent Waves](../advanced/agent-waves.md) for details.

### 5) Extended End-to-End Orchestration
Large multi-stage pipelines from idea through implementation.

Example: `pdd-to-code-assist`

### 6) Guarded Sequential Workflows
Workflows with mandatory phase ordering where out-of-order events could corrupt state.

Example: `autoresearch` (experiment chain with `workflow_guards`)

When a workflow has strict phase dependencies (e.g., scoring must precede evaluation), configure `workflow_guards` to enforce the order at runtime. Use `mode: strict` to reject out-of-order events, or `mode: advisory` to record progress without blocking. This prevents side-channel signals like `periodic.review` from bypassing required phases. Per-instance tracking via `correlation.from_payload` allows parallel experiments to be guarded independently.

## Split Config vs Single-File Config

Recommended:
- Keep core/runtime config in `ralph.yml`
- Select workflow via `-H builtin:<name>`

Backward-compatible single-file mode (still supported):

```bash
# Uses one combined preset file as the main config
ralph run -c presets/feature.yml -p "Add OAuth login"
```

## Creating Your Own Hat Collection

Create a hats file with hats-related sections:

```yaml
event_loop:
  starting_event: "build.start"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]   # <-- all-of gate, see below

hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
    instructions: |
      Implement the requested change and verify it.

  reviewer:
    name: "Reviewer"
    triggers: ["build.done"]
    publishes: ["report.done"]
    instructions: |
      Review the change, request fixes if needed, and close when done.
```

Run it:

```bash
ralph run -c ralph.yml -H .ralph/hats/my-workflow.yml
```

### `required_events` -- All-of Completion Gate

`required_events` is an **all-of** gate: **every** listed topic must have appeared at least once during the loop's lifetime before `LOOP_COMPLETE` is accepted. If even one required event is missing, the completion promise is rejected and a `task.resume` event is injected so the agent can continue working.

```yaml
event_loop:
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]   # all-of: report.done must have been seen
```

**Key behaviors:**

- All listed topics must have been emitted at some point -- they do not need to appear in the same iteration or in a specific order.
- The check is a lifetime check, not a per-iteration check. If `report.done` was emitted on iteration 3, it satisfies the gate on iteration 10.
- When completion is rejected, the agent receives a `task.resume` event explaining which required events are missing.

**Choosing convergence topics:**

Pick topics that sit at the **convergence point** of all successful completion paths. A convergence topic is one that every successful workflow branch eventually emits before reaching `LOOP_COMPLETE`.

```
                  ┌─ builder.done ── fixer ── fix.done ──┐
task.start → ... │                                        ├→ report.done → LOOP_COMPLETE
                  └─ builder.done ── tester ── test.passed┘
```

In this example, both the fix path and the test path converge on `report.done`. Using `report.done` as the required event is safe because it blocks both paths. Using `test.passed` would be wrong because the fix path skips it.

> **Important**: `required_events` uses **all-of** semantics. Every listed event must appear on the event bus before `LOOP_COMPLETE` is accepted. This is not an any-of list. If you need multiple completion paths to converge, choose a single convergence topic that all paths emit.

**Anti-patterns:**

- Using a topic that only one path emits (creates a false gate for alternative paths).
- Using a leaf topic before a convergence point (satisfies the gate too early).
- Listing too many required events (fragile; prefer one or two convergence topics).
- Using mutually exclusive branch events (e.g., `review.passed` and `review.complete` together) -- the validator will reject these since no single path emits both.

### Validating Your Preset Topology

Use `ralph hats validate` to check your preset for topology issues before running:

```bash
ralph hats validate -H .ralph/hats/my-workflow.yml
```

This checks:

1. **Starting event reachability** -- the configured `starting_event` has at least one subscriber.
2. **Completion promise reachability** -- `LOOP_COMPLETE` (or your custom promise) is reachable from at least one hat.
3. **Required event reachability** -- every topic in `required_events` is reachable from the starting event.
4. **All-paths coverage** -- every required event lies on **all** completion paths (not just some). If a required event can be bypassed, the validator emits an error.
5. **Orphan detection** -- events published by no hat subscriber (warnings).
6. **Dead-end detection** -- hats that emit events nobody listens to (warnings).

If any required event is not on all completion paths, adjust your hat topology so that the convergence topic is emitted by the last hat in every branch.

### Preflight Topology Check

The same topology validator also runs automatically during `ralph preflight` and `ralph run` (as the `preset-topology` preflight check). This catches bad preset configurations before any backend API call is made:

```bash
ralph preflight -c ralph.yml -H builtin:ce-executor
# Checks: starting event reachability, completion path, required events all-of coverage
```

The `preset-topology` check is included in `PreflightRunner::default_checks_with_config` and runs as part of the standard preflight sequence. Use `features.preflight.skip` in your config to opt out if needed.

## Source of Truth and Sync

- Canonical preset files: `presets/*.yml`
- Embedded CLI mirror: `crates/ralph-cli/presets/*.yml`
- Sync script: `./scripts/sync-embedded-files.sh`
