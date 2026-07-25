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

- **Inputs.** Caller supplies an E2E **sandbox** directory the skill
  owns for the run, and optionally a development plan path. The
  sandbox is **not** forced to be `crates/ralph-e2e`; any
  caller-owned directory the operator designates as the E2E harness
  root works. Plan path is **optional**: when omitted, or when the
  supplied plan fails the sandbox fitness gate, the skill resolves a
  sandbox-local plan via `scripts/plan_resolve.py` (discover under
  `<sandbox>/docs/plans/`, else author a minimal E2E plan).
- **Plan resolve before audit (R15).** Before `plan_diff` / suite
  generation, run `plan_resolve.resolve_plan(sandbox, candidate=…)`.
  Unfit candidates (e.g. orchestrator `crates/`/`presets/` fix plans
  aimed at a product sandbox with no such trees) are **hard-rejected**
  — do **not** offer a combo-box that re-binds them. Prefer a suitable
  sandbox-local plan; if none exist, author
  `docs/plans/<date>-e2e-bootstrap-minimal-<stem>-plan.md` inside the
  sandbox only.
- **Read-only on caller plans (R13).** The skill NEVER rewrites a
  caller-supplied plan file. Resolved plan bytes are read once,
  hashed, and staged into `<sandbox>/docs/plans/<basename>` when the
  source is outside the sandbox; sandbox-local plans are used in
  place. Launch argv always uses sandbox-relative
  `--plan docs/plans/<basename>`. Authoring a **new** minimal plan
  under the sandbox is allowed (R15) and is not an R13 violation.
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
  and `CONCEPTS.md`. Anything else is
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
| `plan_resolve_choice` | multiple suitable sandbox plans / operator asks which local plan (R15) | highest-scored discovered plan |
| `plan_diff_clarify` | plan intent ↔ git diff disagree (S2) | accept plan intent and re-audit (R4) |
| `binary_resolution` | PATH has no usable `ralph` OR feature requirement unmet (S4) | rebuild via `cargo build` (R6) |
| `preset_gap` | no builtin/file preset covers the plan's verification intent (S3) | handoff to `ralph-preset-author` (R5) |
| `write_conflict` | existing owned pair (`ralph.<stem>.yml` / `PROMPT.<stem>.md`) under different provenance (R8) | refresh with current preset hash (recompute) |
| `argv_shape` | `--plan <abs>` vs `--prompt-file <abs>` (R13) | `--plan` (plan-driven default) |
| `live_run` | operator asks whether to spawn a live loop after the handoff (R10) | stay static-only |

**Not a decision point:** unfit caller plan → hard reject + discover/author (R15). Do not offer "accept unfit orchestrator plan into product sandbox".

The full table is mirrored in `references/interaction.md`.

## Workflow

U1 → U2 → U3 → U4 → U5 → U6 → U7 strictly serial. Each Unit records
acceptance in `.ralph/agent/decisions.md`.

1. **U1 — Scaffolding & Catalog.** Materialise the skill tree and
   register it in `skills/install.py` + `.claude-plugin/marketplace.json`.
   Author `references/interaction.md`. Verify `skills/tests/test_install.py`
   + `skills/tests/test_e2e_bootstrap_contract.py` catalogue anchors
   are green.
2. **U2 — Plan Resolve (R15).** Run `scripts/plan_resolve.py`
   `resolve_plan(sandbox, candidate=optional_plan, preset=…)`.
   Record `source` / `rejected_candidate` in decisions.md. Unfit
   candidate → hard reject (no override combo-box). Prefer discovered
   sandbox plan; else author minimal plan. Only the resolved path
   proceeds.
3. **U3 — Plan × Diff Audit.** Run `scripts/plan_diff.py` on the
   **resolved** plan (sandbox as `repo_root` for sandbox-local work).
   On disagreement, raise `plan_diff_clarify`. On unreadable plan,
   raise `blocked`. No suite writes until `ok`.
4. **U4 — Binary Resolution.** Run `scripts/binary_resolve.py` to pick
   the `ralph` executable. Priority: explicit `--ralph-binary` >
   `RALPH_BINARY` env > `PATH` lookup > suggest `cargo build`.
5. **U5 — Sandbox Suite.** Run `scripts/sandbox_suite.py` with the
   **resolved** plan. Stage when needed (R13: caller plan bytes
   untouched). Argv uses sandbox-relative `--plan docs/plans/<basename>`.
   Pass U4 binary. On `write_conflict` + recommended refresh, rerun
   `generate_suite(..., refresh_existing=True)`.
6. **U6 — Static Gate + Handoff.** Run `scripts/gate.py`; render
   `e2e_handoff.py` with `static_only: true`. Handoff `--plan` MUST
   be the resolved basename.
7. **U7 — Workflow Orchestration.** Keep decision tables in sync;
   `test_plan_resolve.py` + e2e-bootstrap contract/e2e green;
   `test_project_bootstrap_contract.py` untouched.

## Static Gates (R7)

| Stage | Command | Outcome on failure |
|-------|---------|--------------------|
| capability | `ralph --version`, `ralph --help`, per-subcommand help | `blocked_cli` |
| preset check --strict | `ralph -c ralph.<stem>.yml -H <preset> preset check --strict` | `blocked_preset` |
| preflight --strict | `ralph -c ralph.<stem>.yml -H <preset> preflight --strict` | `blocked_cli` / `blocked_backend` |
| dry-run | `ralph -c ralph.<stem>.yml -H <preset> run --dry-run --plan docs/plans/<basename>` | `blocked_command` |

For `builtin:ce-executor-supervisor`, `gate.py` requests JSON for the
first two strict stages and accepts only the exact findings already
exempted by Ralph's embedded-preset tests. Any unknown finding, message,
check status, malformed JSON, backend failure, or environment failure
remains blocking.

The dry-run argv **must** include `--plan docs/plans/<basename>` (the
staged sandbox-relative path; R13: source bytes are never modified).
The argv MUST also include `-c ralph.<stem>.yml -H <preset>` so
`$RALPH_CONFIG` / `ralph.yml` cannot preempt the target suite.

## Guardrails

- NEVER mutate `presets/**` or `crates/**` (R14).
- NEVER rewrite a caller-supplied plan file (R13). Authoring a **new**
  minimal plan under the sandbox is allowed (R15).
- NEVER bind an unfit orchestrator-intent plan into a product sandbox
  via combo-box override (R15 hard reject).
- NEVER spawn a live `ralph run` without explicit operator approval
  captured in the handoff (R10).
- Every user decision is rendered as a combo-box (R12), except R15
  hard rejects which are not decision points.
- Every argv MUST carry `-c ralph.<stem>.yml -H <preset>` explicitly.
- The handoff MUST declare `static_only: true` and a free-form
  `not_live_run` note; `static_only` and `loop closed` MUST NOT be
  conflated.
- All paths persisted to disk are repo-relative; absolute argv values
  are kept inside the handoff only.
- Handoff `--plan` MUST be the **resolved** sandbox-local basename,
  never an unfit rejected candidate.

## Catalog Wiring (U1)

- `skills/install.py` `PUBLIC_SKILLS` includes `ralph-e2e-bootstrap`.
- `.claude-plugin/marketplace.json` lists
  `./skills/ralph-e2e-bootstrap` in `plugins[0].skills[]`.
- `skills/README.md` carries the one-line description.
- `CONCEPTS.md` adds the `ralph-e2e-bootstrap` glossary entry.

## See also

- `references/interaction.md` — combo-box decision-point table
- `skills/ralph-project-bootstrap/` — sibling skill with the same
  static-gate + handoff shape (do not duplicate the underlying
  cli_probe / handoff helpers; reuse them).
- `skills/ralph-preset-author/` — combo-box shape mirroring; preset_gap
  handoff target.
