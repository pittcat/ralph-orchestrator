---
name: ralph-e2e-bootstrap
description: >-
  Bootstrap an external E2E sandbox onto a Ralph preset to verify an
  orchestrator change plan: resolve sandbox workload, inject change
  intent into PROMPT (agent-visible via --prompt-file), ensure a fresh
  repo ralph binary, run static gates, deliver a copy-paste launch
  command. Use when dogfooding orchestrator changes against a
  product/sibling E2E harness.
---

# Ralph E2E Bootstrap

Bring an **external** E2E sandbox onto an existing Ralph preset so the
operator can verify a **change plan** from the current (orchestrator)
repo with a real scenario. The skill writes a **preset-bound** suite
(`ralph.<stem>.yml` / `PROMPT.<stem>.md`). It is **not** arbitrary
project bootstrap (`ralph-project-bootstrap`) and **not** preset
authoring (`ralph-preset-author`).

## Forced entry (P1)

**MUST** drive the skill through:

```bash
python skills/ralph-e2e-bootstrap/scripts/bootstrap_pipeline.py \
  --sandbox <external-repo> \
  --change-plan <orchestrator-plan.md> \
  --preset builtin:<name> \
  [--ralph-binary <path>] \
  [--preset-continue-confirmed] \
  [--author-confirmed] \
  --json
```

Or call `scripts/bootstrap_pipeline.run_pipeline(...)` from the agent.
Do **not** call `generate_suite` / handoff alone — that skips dual-plan,
freshness, and prompt-file wiring.

## Boundaries

- **Inputs (operator).**
  1. **Sandbox** (required) — external product/E2E harness directory
     (sibling repo). Same-repo / in-tree harness is out of scope;
     `resolve_plans` / pipeline refuse same git toplevel as the change
     plan.
  2. **Change plan** (required) — path in the **current orchestrator
     repo** describing what changed and what to verify. Never used as
     `ralph run --plan`.
  3. **Preset** (required for pipeline) — e.g.
     `builtin:ce-executor-supervisor`. If the change plan declares
     `presets/` intent, raise `preset_gap` combo-box: either confirm
     preset work is already done (`--preset-continue-confirmed`), or
     **hard-handoff** `ralph-preset-author`.
- **Dual-plan model (R15) + agent-visible change intent (P0).**
  - **Workload plan** — sandbox-local business scenario. Auto-discovered
    under `<sandbox>/docs/plans/`. Becomes `ralph run --plan`
    (workload identity / worktree key). Full body is also embedded in
    `PROMPT.<stem>.md`.
  - **Change plan** — verification intent. Injected into
    `PROMPT.<stem>.md` (path + hash + Goal Capsule summary).
  - **Launch argv** — `--prompt-file PROMPT.<stem>.md --plan
    docs/plans/<workload>`. Ralph gives `--prompt-file` precedence for
    the loop prompt, so agents **see** change intent + workload.
- **Sandbox plan mutations require confirmation.** Discovering and
  selecting an existing suitable workload is silent. **Creating or
  editing** any file under `<sandbox>/docs/plans/` MUST go through
  the `sandbox_plan_write` combo-box first — `author_minimal_plan`
  requires `confirmed=True` + `confirmation_token="sandbox_plan_write"`.
- **Read-only on caller change plans (R13).** Never rewrite the
  orchestrator change plan file.
- **Binary freshness.** Prefer `{orchestrator}/target/{debug,release}/ralph`.
  Pipeline **blocks** handoff until `check_binary_freshness` passes;
  otherwise raise `binary_resolution` / `cargo build -p ralph-cli`.
- **No live run / no diagnosis (R10).** Static gates + launch command
  only. Live loop and intermediate-artifact inspection belong to the
  operator terminal + `ralph-run-diagnosis`.
- **No Rust / CLI mutation (R14).** Implementation only under
  `skills/ralph-e2e-bootstrap/**`, `skills/tests/**`, catalog,
  `CONCEPTS.md` (shared probe argv helper in
  `ralph-project-bootstrap/scripts/cli_probe.py` may emit both
  `--prompt-file` and `--plan`).

## Combo-box Interaction Contract

Every user-decision point is a **combo-box**: 2–4 options, recommended
first with consequence, `Other` escape, one decision per turn.

| Decision point | Trigger | Default (recommended first) |
|----------------|---------|------------------------------|
| `sandbox_plan_write` | no suitable workload / operator wants a new or edited sandbox plan | author minimal E2E plan (after explicit confirm) |
| `plan_resolve_choice` | multiple suitable sandbox plans | highest-scored discovered plan |
| `plan_diff_clarify` | workload plan ↔ sandbox git diff disagree | accept plan intent and re-audit |
| `binary_resolution` | no usable ralph OR binary not a fresh repo build | `cargo build -p ralph-cli` in change-plan repo |
| `preset_gap` | change plan touches `presets/` or preset missing | continue if already updated, else hard-handoff `ralph-preset-author` |
| `write_conflict` | owned pair provenance mismatch | refresh with current hashes |
| `argv_shape` | prompt vs plan shape | `--prompt-file PROMPT.<stem>.md` + `--plan` workload |
| `live_run` | operator asks to spawn live loop | stay static-only |

**Not a decision point:** using the change plan as workload `--plan`
(forbidden).

Full tables: `references/interaction.md`.

## Workflow

U1 → U2 → U3 → U4 → U5 → U6 → U7 via **`run_pipeline`** (serial).
Record acceptance in `.ralph/agent/decisions.md`.

1. **U1 — Catalog anchors.** Skill installed / discoverable.
2. **U2 — Plan resolve (R15).** Pipeline calls
   `resolve_plans(sandbox, change_plan=…, preset=…)`.
   - Missing change plan / same-repo sandbox → blocked.
   - `change_plan_touches_presets` → `preset_gap` until confirmed or
     handoff.
   - No workload → `sandbox_plan_write` (only then author with token).
3. **U3 — Plan × Diff Audit.** Workload vs sandbox git diff.
4. **U4 — Binary.** `resolve_binary` + **mandatory**
   `check_binary_freshness`. Stale → `binary_resolution`.
5. **U5 — Sandbox suite.** `generate_suite` with required change
   context; PROMPT embeds change intent + full workload body.
6. **U6 — Static gate + handoff.** capability → preset check →
   preflight → dry-run with `--prompt-file` + `--plan`. Handoff
   `static_only: true`; diagnosis → `ralph-run-diagnosis`.
7. **U7 — Orchestration audit.** Contract / plan_resolve /
   bootstrap_pipeline tests green.

## Static Gates (R7)

| Stage | Command | Outcome on failure |
|-------|---------|--------------------|
| capability | `ralph --version` / help | `blocked_cli` |
| preset check --strict | `ralph -c ralph.<stem>.yml -H <preset> preset check --strict` | `blocked_preset` |
| preflight --strict | `ralph -c … preflight --strict` | `blocked_cli` / `blocked_backend` |
| dry-run | `ralph -c … run --dry-run --prompt-file PROMPT.<stem>.md --plan docs/plans/<workload>` | `blocked_command` |

Dry-run argv **must** use sandbox-relative config / prompt / workload
plan with explicit `-c` / `-H`.

## Guardrails

- NEVER mutate `presets/**` or `crates/**` (R14).
- NEVER rewrite the orchestrator change plan (R13).
- NEVER use the change plan as `ralph run --plan`.
- NEVER create/edit `<sandbox>/docs/plans/*` without `sandbox_plan_write`.
- NEVER spawn live `ralph run` without handoff-captured approval (R10).
- NEVER parse `.ralph/events.jsonl` / supervisor ledgers here.
- NEVER skip `bootstrap_pipeline` / freshness when emitting handoff.
- Handoff `--plan` MUST be the resolved workload basename path;
  `--prompt-file` MUST be `PROMPT.<stem>.md`.

## Catalog Wiring (U1)

- `skills/install.py` `PUBLIC_SKILLS` includes `ralph-e2e-bootstrap`.
- `.claude-plugin/marketplace.json` lists `./skills/ralph-e2e-bootstrap`.
- `skills/README.md` one-line description.
- `CONCEPTS.md` glossary entry.

## See also

- `references/interaction.md`
- `skills/ralph-project-bootstrap/`
- `skills/ralph-preset-author/`
- `skills/ralph-run-diagnosis/` — post-run artifacts
