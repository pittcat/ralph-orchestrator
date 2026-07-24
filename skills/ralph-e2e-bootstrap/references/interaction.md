# Combo-box Interaction Contract — ralph-e2e-bootstrap

This file is the source of truth for the decision-point table that
`SKILL.md` mirrors. Every user decision in the skill is rendered as
a **combo-box**: 2–4 mutually exclusive options, recommended option
listed first, each option carries a one-line consequence, and an
`Other` (free-text custom) escape hatch is always available.

Decision points are surfaced one at a time across multiple turns; the
skill MUST NOT batch multiple decisions into a single AskUser.

## Decision points

### `plan_diff_clarify` — plan intent ↔ git diff disagree

Triggered by `scripts/plan_diff.py` when the development plan's
intent fields disagree with the current git diff (paths changed, U-ID
list, scope).

| # | Option | Consequence |
|---|--------|-------------|
| 1 (**recommended**) | Accept plan intent, re-audit diff | The skill trusts the plan; subsequent stages use plan-derived scope. |
| 2 | Accept diff, flag plan as stale | The skill captures a `plan_stale` risk in the handoff and continues with diff-derived scope. |
| 3 | Halt, manual reconcile | The skill emits a `blocked` handoff and stops; the operator edits the plan and re-runs. |
| Other | Free-text custom intent | The skill records the operator text verbatim and re-runs the audit once. |

### `binary_resolution` — no usable `ralph` on PATH (or feature miss)

Triggered by `scripts/binary_resolve.py` when no `ralph` binary
exists on `PATH`, the located binary fails version detection, or its
declared features do not match the resolved preset.

| # | Option | Consequence |
|---|--------|-------------|
| 1 (recommended) | Rebuild via `cargo build` | The skill runs `cargo build -p ralph-cli`; the freshly-built binary becomes the resolved one. |
| 2 | Install via PATH (`cargo install`) | The skill runs `cargo install --path crates/ralph-cli --locked`; the install dir is added to `PATH` for this run. |
| 3 | Provide absolute path | The operator supplies an absolute path; the skill re-runs the probe with the supplied path. |
| Other | Free-text path or build override | Recorded verbatim; re-probed once. |

### `preset_gap` — no preset covers the plan's verification intent

Triggered by `scripts/sandbox_suite.py` (or its upstream chain) when
the resolved preset does not cover the plan's required intents
(testing / lint / build / e2e). The skill MUST NOT silently fall
through; it MUST stop and hard-handoff to `ralph-preset-author`.

| # | Option | Consequence |
|---|--------|-------------|
| 1 (recommended) | Handoff to `ralph-preset-author` | The skill emits a blocked handoff naming the missing intent and pointing to the preset-author workflow. |
| 2 | Force-resolve with closest builtin | Recorded as `forced_preset_gap` risk; the handoff declares the gap loudly. |
| 3 | Halt, operator edits preset manually | The skill emits a blocked handoff with the operator action recorded. |
| Other | Free-text override | Recorded verbatim; skill halts unless the operator also confirms a preset identifier. |

### `write_conflict` — owned pair exists under different provenance

Triggered by `scripts/sandbox_suite.py` when an existing
`ralph.<stem>.yml` / `PROMPT.<stem>.md` pair is on disk but the
embedded `profile_sha256` / `prompt_sha256` do not match the
resolved preset / inline prompt.

| # | Option | Consequence |
|---|--------|-------------|
| 1 (recommended) | Refresh with current preset hash | The skill rewrites both files under the current provenance and records the prior hash in `decisions.md`. |
| 2 | Preserve existing pair | The skill keeps the pair and emits an `incomplete_static_only` handoff noting the provenance mismatch. |
| 3 | Back up and overwrite | The skill writes the prior pair to `ralph.<stem>.yml.bak-<sha>` / `PROMPT.<stem>.md.bak-<sha>` and refreshes. |
| Other | Free-text | Recorded verbatim; skill halts unless paired with a concrete action. |

### `argv_shape` — `--plan` vs `--prompt-file`

Triggered when the caller supplied both a plan path and a prompt
file (or neither). The skill must choose exactly one explicit prompt
source for the dry-run argv.

| # | Option | Consequence |
|---|--------|-------------|
| 1 (recommended) | `--plan <abs>` | The plan is the authoritative prompt; the dry-run argv uses `--plan`. The skill does NOT also pass `--prompt-file`. |
| 2 | `--prompt-file <abs>` | An external prompt file is authoritative; the plan is read-only context. |
| Other | Free-text | Recorded verbatim; skill halts unless paired with a concrete argv slot. |

### `live_run` — operator asks whether to spawn a live loop

Triggered by an explicit operator message after the handoff has
been rendered. By default the skill stops at the static-only
handoff (R10).

| # | Option | Consequence |
|---|--------|-------------|
| 1 (recommended) | Stay static-only | The skill emits nothing further; the operator runs the handoff command in a separate terminal. |
| 2 | Spawn live `ralph run` (operator-supervised) | The skill emits the resolved argv; loop startup is the operator's terminal action. The skill itself never spawns the loop. |
| Other | Free-text | Recorded verbatim; default behavior is `Stay static-only` unless the operator supplies an explicit `spawn live` confirmation. |

## Forbidden patterns

- A single AskUser containing two or more decision points.
- A combo-box with a single option (must offer at least 2 mutually
  exclusive options, recommended listed first).
- A combo-box where the recommended option's consequence is not
  stated.
- A combo-box without an `Other` escape hatch.
- A combo-box whose options are not mutually exclusive (e.g. "yes
  and continue" + "yes and stop" presented side-by-side).

## Cross-skill alignment

The combo-box shape mirrors `skills/ralph-preset-author`'s
`Workflow 0` decision points (resolve preset, resolve plan, argv
shape, write-conflict, binary-missing). The wording is intentionally
similar so an operator fluent in one skill is fluent in the other.
