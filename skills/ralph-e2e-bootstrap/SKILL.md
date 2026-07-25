---
name: ralph-e2e-bootstrap
description: >-
  Bootstrap an external E2E sandbox onto a Ralph preset to verify an
  orchestrator change plan: resolve sandbox workload, inject change
  intent into PROMPT, ensure a fresh repo ralph binary, run static
  gates, deliver a copy-paste launch command. Use when dogfooding
  orchestrator changes against a product/sibling E2E harness.
---

# Ralph E2E Bootstrap

Bring an **external** E2E sandbox onto an existing Ralph preset so the
operator can verify a **change plan** from the current (orchestrator)
repo with a real scenario. The skill writes a **preset-bound** suite
(`ralph.<stem>.yml` / `PROMPT.<stem>.md`). It is **not** arbitrary
project bootstrap (`ralph-project-bootstrap`) and **not** preset
authoring (`ralph-preset-author`).

## Boundaries

- **Inputs (operator).**
  1. **Sandbox** (required) — external product/E2E harness directory
     (sibling repo). Same-repo / in-tree harness is out of scope.
  2. **Change plan** (required / strongly recommended) — path in the
     **current orchestrator repo** describing what changed and what
     to verify. Never used as `ralph run --plan`.
  3. **Preset** (recommended) — e.g. `builtin:ce-executor-supervisor`.
     If omitted, infer from context. If the change plan declares
     `presets/` intent, raise `preset_gap` combo-box: either confirm
     preset work is already done in the orchestrator repo, or
     **hard-handoff** `ralph-preset-author` before continuing.
- **Dual-plan model (R15).**
  - **Workload plan** — sandbox-local business scenario. Always
    auto-discovered under `<sandbox>/docs/plans/`. Becomes
    `ralph run --plan` (loop primary prompt).
  - **Change plan** — verification intent. Injected into
    `PROMPT.<stem>.md` (path + hash + Goal Capsule summary). Not
    `--plan`.
- **Sandbox plan mutations require confirmation.** Discovering and
  selecting an existing suitable workload is silent. **Creating or
  editing** any file under `<sandbox>/docs/plans/` MUST go through
  the `sandbox_plan_write` combo-box first — never silent author.
- **Read-only on caller change plans (R13).** Never rewrite the
  orchestrator change plan file.
- **Binary freshness.** Prefer `{orchestrator}/target/{debug,release}/ralph`.
  If PATH/`RALPH_BINARY` is not a fresh build of the change-plan repo,
  raise `binary_resolution` recommending `cargo build -p ralph-cli`.
- **No live run / no diagnosis (R10).** Static gates + launch command
  only. Live loop and intermediate-artifact inspection belong to the
  operator terminal + `ralph-run-diagnosis`.
- **No Rust / CLI mutation (R14).** Implementation only under
  `skills/ralph-e2e-bootstrap/**`, `skills/tests/**`, catalog,
  `CONCEPTS.md`.

## Combo-box Interaction Contract

Every user-decision point is a **combo-box**: 2–4 options, recommended
first with consequence, `Other` escape, one decision per turn.

| Decision point | Trigger | Default (recommended first) |
|----------------|---------|------------------------------|
| `sandbox_plan_write` | no suitable workload / operator wants a new or edited sandbox plan | author minimal E2E plan (after explicit confirm) |
| `plan_resolve_choice` | multiple suitable sandbox plans | highest-scored discovered plan |
| `plan_diff_clarify` | workload plan ↔ sandbox git diff disagree | accept plan intent and re-audit |
| `binary_resolution` | no usable ralph OR binary not a fresh repo build | `cargo build -p ralph-cli` in change-plan repo |
| `preset_gap` | change plan / verification needs a missing preset | hard-handoff `ralph-preset-author` |
| `write_conflict` | owned pair provenance mismatch | refresh with current hashes |
| `argv_shape` | `--plan` vs `--prompt-file` | `--plan` (workload) |
| `live_run` | operator asks to spawn live loop | stay static-only |

**Not a decision point:** using the change plan as workload `--plan`
(forbidden). Preset intent on the change plan → hard handoff, not
"force continue".

Full tables: `references/interaction.md`.

## Workflow

U1 → U2 → U3 → U4 → U5 → U6 → U7 strictly serial. Record acceptance in
`.ralph/agent/decisions.md`.

1. **U1 — Catalog anchors.** Skill installed / discoverable.
2. **U2 — Plan resolve (R15).**
   `resolve_plans(sandbox, change_plan=…, preset=…)`.
   - `needs_preset_author` → hard-handoff author, stop.
   - Discover workload; if none → `sandbox_plan_write` combo-box
     (only then `author_minimal_plan`).
   - Record change hash + workload path + sources.
3. **U3 — Plan × Diff Audit.** `plan_diff.run_audit` on the
   **workload** plan with `repo_root=sandbox` (cross-repo rules apply).
4. **U4 — Binary.** `resolve_binary` then
   `check_binary_freshness(binary, change_plan_repo)`. Stale →
   `binary_resolution` → build → re-resolve.
5. **U5 — Sandbox suite.** `generate_suite(..., plan_path=workload,
   change_plan_path=…, change_plan_hash=…, change_summary=…)`.
   `--plan` is workload only; PROMPT carries change intent.
6. **U6 — Static gate + handoff.** capability → preset check →
   preflight → dry-run. Handoff `static_only: true`; point diagnosis
   to `ralph-run-diagnosis`.
7. **U7 — Orchestration audit.** Contract / plan_resolve tests green;
   project-bootstrap contract untouched.

## Static Gates (R7)

| Stage | Command | Outcome on failure |
|-------|---------|--------------------|
| capability | `ralph --version` / help | `blocked_cli` |
| preset check --strict | `ralph -c ralph.<stem>.yml -H <preset> preset check --strict` | `blocked_preset` |
| preflight --strict | `ralph -c … preflight --strict` | `blocked_cli` / `blocked_backend` |
| dry-run | `ralph -c … run --dry-run --plan docs/plans/<workload-basename>` | `blocked_command` |

Dry-run argv **must** use the **workload** sandbox-relative plan and
explicit `-c` / `-H`.

## Guardrails

- NEVER mutate `presets/**` or `crates/**` (R14).
- NEVER rewrite the orchestrator change plan (R13).
- NEVER use the change plan as `ralph run --plan`.
- NEVER create/edit `<sandbox>/docs/plans/*` without `sandbox_plan_write`.
- NEVER spawn live `ralph run` without handoff-captured approval (R10).
- NEVER parse `.ralph/events.jsonl` / supervisor ledgers here.
- Handoff `--plan` MUST be the resolved workload basename.

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
