# Combo-box Interaction Contract — ralph-e2e-bootstrap

This file is the source of truth for the decision-point table that
`SKILL.md` mirrors. Every user decision in the skill is rendered as
a **combo-box**: 2–4 mutually exclusive options, recommended option
listed first, each option carries a one-line consequence, and an
`Other` (free-text custom) escape hatch is always available.

Decision points are surfaced one at a time across multiple turns; the
skill MUST NOT batch multiple decisions into a single AskUser.

## Decision points

### `sandbox_plan_write` — create or edit a sandbox workload plan

Triggered when discovery finds **no** suitable workload under
`<sandbox>/docs/plans/`, or when the operator explicitly asks to
create/edit a sandbox plan. **Silent writes are forbidden.**

| # | Option | Consequence |
|---|--------|-------------|
| 1 (**recommended**) | Author minimal E2E plan | Skill writes `docs/plans/<date>-e2e-bootstrap-minimal-<stem>-plan.md` after this confirm. |
| 2 | Halt — operator will add a plan manually | Skill emits blocked / incomplete handoff; no write. |
| 3 | Point to an existing sandbox-relative path | Re-run fitness; unfit paths rejected. |
| Other | Free-text | Recorded; no write unless paired with a concrete create/edit action. |

### `plan_resolve_choice` — which suitable sandbox-local workload

Triggered when **multiple** suitable sandbox plans score close.

| # | Option | Consequence |
|---|--------|-------------|
| 1 (**recommended**) | Use highest-scored sandbox plan | That path becomes `ralph run --plan`. |
| 2 | Use another listed suitable plan | Operator picks among suitable discovered plans only. |
| Other | Free-text suitable sandbox-relative path | Re-run fitness; change plans from other repos rejected as workload. |

**Not a decision point.** The orchestrator **change plan** is never
offered as workload `--plan`. It is verification context only
(injected into `PROMPT.<stem>.md`). Binding it as `--plan` is forbidden.

**Not a decision point.** Change plan declares `presets/` intent →
hard-handoff `ralph-preset-author` (see `preset_gap`), not a soft
override.

### `plan_diff_clarify` — workload plan ↔ sandbox git diff disagree

Triggered by `scripts/plan_diff.py` when the **workload** plan
disagrees with the sandbox git diff.

**Cross-repo auto-pass (not a decision point).** When
`AuditDecision.cross_repo` is `True`, skip `scope_drift` /
`plan_stale` combo-box for those codes. Plan-quality codes and
`diff_unavailable` still trigger this decision point.

| # | Option | Consequence |
|---|--------|-------------|
| 1 (**recommended**) | Accept plan intent, re-audit diff | Trust workload plan for downstream scope. |
| 2 | Accept diff, flag plan as stale | Record `plan_stale` risk; continue with diff-derived scope. |
| 3 | Halt, manual reconcile | Blocked handoff; operator edits workload plan and re-runs. |
| Other | Free-text custom intent | Recorded; re-audit once. |

### `binary_resolution` — missing or stale ralph binary

Triggered when no usable `ralph` is on PATH, version probe fails, **or**
`check_binary_freshness` reports the binary is not a fresh build of
the change-plan (orchestrator) repo.

| # | Option | Consequence |
|---|--------|-------------|
| 1 (**recommended**) | Rebuild via `cargo build -p ralph-cli` | Build in the change-plan repo; use `target/debug/ralph`. |
| 2 | Install via PATH (`cargo install`) | `cargo install --path crates/ralph-cli --locked`. |
| 3 | Provide absolute path to a repo build | Re-probe; must pass freshness when verifying a change plan. |
| Other | Free-text path or build override | Recorded; re-probed once. |

### `preset_gap` — preset missing or change plan touches presets/

Triggered when no preset covers verification intent, **or**
`resolve_plans` sets `change_plan_touches_presets` (change plan
declares `presets/` paths).

| # | Option | Consequence |
|---|--------|-------------|
| 1 (**recommended** when preset already landed) | Preset already updated in orchestrator — continue | Record confirmation; proceed with bootstrap. |
| 2 | Handoff to `ralph-preset-author` | Blocked handoff; resume after preset lands. |
| 3 | Halt, operator edits preset manually | Blocked handoff with operator action recorded. |
| Other | Free-text | Recorded; skill halts unless a concrete continue/handoff is clear. |

When the builtin/file preset is **missing**, option 1 is not offered —
only handoff / halt.

### `write_conflict` — owned pair exists under different provenance

Triggered by `scripts/sandbox_suite.py` when an existing
`ralph.<stem>.yml` / `PROMPT.<stem>.md` pair is on disk but the
embedded hashes do not match.

| # | Option | Consequence |
|---|--------|-------------|
| 1 (recommended) | Refresh with current preset hash | Rewrite both files under current provenance. |
| 2 | Preserve existing pair | Keep pair; incomplete static-only handoff. |
| 3 | Back up and overwrite | `.bak-<sha>` then refresh. |
| Other | Free-text | Recorded; halt unless paired with a concrete action. |

### `argv_shape` — `--plan` vs `--prompt-file`

| # | Option | Consequence |
|---|--------|-------------|
| 1 (recommended) | `--plan <sandbox-relative workload>` | Workload staged/used as `docs/plans/<basename>`. Change plan stays in PROMPT only. |
| 2 | `--prompt-file <abs>` | External prompt authoritative; rare for this skill. |
| Other | Free-text | Recorded; halt unless paired with a concrete argv slot. |

### `live_run` — operator asks whether to spawn a live loop

Triggered by an explicit operator message after the handoff.
Default remains static-only. Intermediate artifacts after a live run
are owned by `ralph-run-diagnosis`, not this skill.

| # | Option | Consequence |
|---|--------|-------------|
| 1 (recommended) | Stay static-only | No further skill action; operator runs launch command separately. |
| 2 | Spawn live `ralph run` (operator-supervised) | Emit argv; skill itself never spawns the loop. |
| Other | Free-text | Recorded; default Stay static-only. |

## Forbidden patterns

- A single AskUser containing two or more decision points.
- A combo-box with a single option (must offer at least 2 mutually
  exclusive options, recommended listed first).
- A combo-box where the recommended option's consequence is not
  stated.
- A combo-box without an `Other` escape hatch.
- A combo-box whose options are not mutually exclusive.
- Offering the orchestrator change plan as workload `--plan`.
- Silently creating or editing `<sandbox>/docs/plans/*`.
