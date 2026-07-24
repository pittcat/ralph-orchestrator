---
name: ralph-e2e-bootstrap
description: >-
  Bootstrap an E2E sandbox directory into a runnable Ralph preset suite
  from a development plan + git diff, via combo-box decisions and static
  gates; deliver a copy-paste launch command. Use when bringing an E2E
  harness directory onto Ralph with a known preset.
---

# Ralph E2E Bootstrap

Bring an E2E sandbox directory onto an existing Ralph preset using a
plan-driven, combo-box-gated flow. The skill is **not** a sandbox for
arbitrary project bootstrap (`ralph-project-bootstrap` owns that) and
**not** a preset authoring tool (`ralph-preset-author` owns that).

## Boundaries

- **Inputs.** Caller supplies (1) a development plan path
  (`docs/plans/<id>-<slug>.md`) and (2) an E2E sandbox directory the
  skill owns for the duration of the run. The sandbox is **not**
  forced to be `crates/ralph-e2e`; any caller-owned directory the
  operator designates as the E2E harness root works.
- **Read-only on the plan.** The plan file is read as the canonical
  intent source. The skill NEVER rewrites the plan file inside the
  sandbox (R13) — every plan reference becomes an absolute argv
  argument (`--plan <abs-path>`).
- **No preset authoring.** Preset topology, AAF tables and builtin
  completions live in `ralph-preset-author` / `ralph-preset-review`.
  When the existing preset does not cover the plan's verification
  intent, the skill hard-handoffs to preset author and stops.
- **No live `ralph run` by default.** Static gates include
  `ralph run --dry-run` (R7); the handoff always declares
  `static_only: true` (R10). Live execution requires an explicit
  operator decision captured by the handoff; the skill itself never
  spawns a loop.
- **No post-run diagnosis.** `ralph-run-diagnosis` owns that. The
  skill never opens `.ralph/events.jsonl`, never reads supervisor
  ledgers, never parses loop traces.
- **No Rust / CLI mutation.** Implementation lives only under
  `skills/ralph-e2e-bootstrap/**`, `skills/tests/**`, the catalog
  (`skills/install.py` PUBLIC_SKILLS + `.claude-plugin/marketplace.json`),
  `docs/guide/e2e-bootstrap.md`, and `CONCEPTS.md`. Anything else is
  forbidden by R14.

## Combo-box Interaction Contract

Every user-decision point in this skill is rendered as a **combo-box**:

- 2–4 mutually exclusive options.
- Recommended option listed first.
- Each option carries a one-line **consequence** clause.
- An `Other` escape hatch is always available (free-text custom).
- Decision points are surfaced one at a time across multiple turns;
  the skill MUST NOT batch multiple decisions into a single AskUser.

| Decision point | Trigger | Default (recommended first) |
|----------------|---------|------------------------------|
| `plan_diff_clarify` | plan intent ↔ git diff disagree (S2) | accept plan intent and re-audit (R4) |
| `binary_resolution` | PATH has no usable `ralph` OR feature requirement unmet (S4) | rebuild via `cargo build` (R6) |
| `preset_gap` | no builtin/file preset covers the plan's verification intent (S3) | handoff to `ralph-preset-author` (R5) |
| `write_conflict` | existing owned pair (`ralph.<stem>.yml` / `PROMPT.<stem>.md`) under different provenance (R8) | refresh with current preset hash (recompute) |
| `argv_shape` | `--plan <abs>` vs `--prompt-file <abs>` (R13) | `--plan` (plan-driven default) |
| `live_run` | operator asks whether to spawn a live loop after the handoff (R10) | stay static-only |

The full table is mirrored in `references/interaction.md`.

## Workflow

U1 → U2 → U3 → U4 → U5 → U6 strictly serial. Each Unit records
acceptance in `.ralph/agent/decisions.md`.

1. **U1 — Scaffolding & Catalog.** Materialise the skill tree and
   register it in `skills/install.py` + `.claude-plugin/marketplace.json`.
   Author `references/interaction.md`. Verify `skills/tests/test_install.py`
   + `skills/tests/test_e2e_bootstrap_contract.py` catalogue anchors
   are green.
2. **U2 — Plan × Diff Audit.** Run `scripts/plan_diff.py` to reconcile
   the development plan against the working-tree diff. On disagreement,
   raise `plan_diff_clarify` combo-box. On unreadable plan, raise
   `blocked`. No writes happen until this stage reports `ok`.
3. **U3 — Binary Resolution.** Run `scripts/binary_resolve.py` to pick
   the `ralph` executable. Priority: explicit `--ralph-binary` >
   `RALPH_BINARY` env > `PATH` lookup > suggest `cargo build` (which
   becomes a combo-box choice). A non-existent or non-executable path
   raises `blocked`.
4. **U4 — Sandbox Suite.** Run `scripts/sandbox_suite.py` to author
   `ralph.<stem>.yml` + `PROMPT.<stem>.md` inside the caller-supplied
   sandbox directory. The pair is **preset-bound** (`<stem>` derives
   from the resolved preset). The plan file is read-only — the
   generated argv references the absolute path via `--plan`. Refuse to
   write inside `presets/`.
5. **U5 — Static Gate + Handoff.** Run `scripts/gate.py` in the
   four-stage order: capability → preset check --strict →
   preflight --strict → `ralph run --dry-run`. `gate.py` imports the
   sibling probe via the same `spec_from_file_location` shim used by
   tests. Render `scripts/e2e_handoff.py` output with `static_only:
   true` and an explicit `not_live_run` clause. The handoff is the
   final deliverable; nothing else mutates state.
6. **U6 — Workflow Orchestration.** Wire U1–U5 into this SKILL.md as
   the canonical procedure; ensure the decision-point table in this
   file matches `references/interaction.md` exactly; ensure both
   `test_e2e_bootstrap_contract.py` and `test_e2e_bootstrap_e2e.py`
   pass. `test_project_bootstrap_contract.py` MUST remain unchanged.

## Static Gates (R7)

| Stage | Command | Outcome on failure |
|-------|---------|--------------------|
| capability | `ralph --version`, `ralph --help`, per-subcommand help | `blocked_cli` |
| preset check --strict | `ralph -c ralph.<stem>.yml -H <preset> preset check --strict` | `blocked_preset` |
| preflight --strict | `ralph -c ralph.<stem>.yml -H <preset> preflight --strict` | `blocked_cli` / `blocked_backend` |
| dry-run | `ralph -c ralph.<stem>.yml -H <preset> run --dry-run --plan <abs>` | `blocked_command` |

The dry-run argv **must** include `--plan <abs-plan-path>` (R13). The
argv MUST also include `-c ralph.<stem>.yml -H <preset>` so
`$RALPH_CONFIG` / `ralph.yml` cannot preempt the target suite.

## Guardrails

- NEVER mutate `presets/**` or `crates/**` (R14).
- NEVER rewrite the supplied plan file (R13).
- NEVER spawn a live `ralph run` without explicit operator approval
  captured in the handoff (R10).
- Every user decision is rendered as a combo-box (R12).
- Every argv MUST carry `-c ralph.<stem>.yml -H <preset>` explicitly.
- The handoff MUST declare `static_only: true` and a free-form
  `not_live_run` note; `static_only` and `loop closed` MUST NOT be
  conflated.
- All paths persisted to disk are repo-relative; absolute argv values
  are kept inside the handoff only.

## Catalog Wiring (U1)

- `skills/install.py` `PUBLIC_SKILLS` includes `ralph-e2e-bootstrap`.
- `.claude-plugin/marketplace.json` lists
  `./skills/ralph-e2e-bootstrap` in `plugins[0].skills[]`.
- `skills/README.md` carries the one-line description.
- `CONCEPTS.md` adds the `ralph-e2e-bootstrap` glossary entry.

## See also

- `references/interaction.md` — combo-box decision-point table
- `docs/guide/e2e-bootstrap.md` — operator guide (optional, referenced
  from this SKILL)
- `skills/ralph-project-bootstrap/` — sibling skill with the same
  static-gate + handoff shape (do not duplicate the underlying
  cli_probe / handoff helpers; reuse them).
- `skills/ralph-preset-author/` — combo-box shape mirroring; preset_gap
  handoff target.
