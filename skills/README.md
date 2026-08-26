# Ralph Orchestrator Agent Skills

This directory is the canonical public skill package for external agent
harnesses that operate Ralph.

It ships operator skills:

| Skill | Purpose |
|---|---|
| `ralph-preset-author` | Draft presets (builtin + local) with per-hat AAF tables **+ payload contract notes** before review |
| `ralph-preset-review` | Per-hat activation dry-run + **payload audit** + mechanical lint → `preset-review-report.md` |
| `ralph-project-bootstrap` | Audit a target project via ONE unified pipeline entry (`bootstrap_pipeline.py`), generate or safely update the preset-bound `ralph.<stem>.yml` + `PROMPT.<stem>.md` suite and the AGENTS.md / CLAUDE.md managed sections from an existing preset, run staged validation, and hand off the official launch command (`dry-run green != loop closed`) |
| `ralph-run-diagnosis` | Post-run deep diagnosis: artifacts, OPAC, mechanism vs preset attribution |
| `ralph-task-discovery` | Structured task discovery before formal planning: environment-sourced evidence, one-question-at-a-time business decisions, and a validated task brief handed off to `ralph-preset-author` |

> **Plan 2026-08-02-001:** the previously bundled `ralph-loop` and
> `ralph-preset-common` skills are gone. `ralph-loop` is retired (loop
> operations stay in the runner CLI / web dashboard). The author/review
> pair now ships its own `references/`, `fixtures/`, and `tests/` files,
> so there is no shared common directory to bundle. `install.py` rejects
> `ralph-loop` requests and copies each skill's local `references/`
> directory verbatim.

These are public agent skills. They are not part of Ralph's internal
`ralph tools skill` registry.

## Invocation order

```text
ralph-task-discovery → ralph-preset-author → ralph-preset-review
```

- `ralph-task-discovery` runs **before any plan is written**: facts are
  unknown, business decisions are open, terminology or boundary conflicts
  are suspected, or a bug task needs a red-capable feedback loop before
  root-causing. It converges a validated task brief
  (Evidence/Decision/Candidate ledger) and hands off **only the brief
  path**, at `author_ready`, to `ralph-preset-author`.
- `ralph-preset-author` drafts the preset from the brief;
  `ralph-preset-review` then audits it (the closed-loop pair below).
- Orthogonal to this chain: `ralph-project-bootstrap` provisions a target
  project onto an existing preset, and `ralph-run-diagnosis` diagnoses a loop
  that already ran. Neither replaces task discovery before planning,
  and task discovery never authors presets itself.

## Verification levels

Three test layers guard these skills (interpreter: `skills/.venv/bin/python`,
run from the repo root):

| Layer | Suite | Locks |
|---|---|---|
| Contract | `skills/tests/test_task_discovery_contract.py` | Frozen task-brief constants, hard gates, stable error codes / next_action vocabulary |
| E2E transcript | `skills/tests/test_task_discovery_e2e.py` | Deterministic transcripts and the full transcript → brief → validator → author-handoff pipeline; no failure branch may reach the author |
| Install parity | `skills/tests/test_install.py` (+ execution-model / prompt-visibility / project-bootstrap contract suites) | Skill catalog, marketplace manifest, installed-tree parity |

`skills/.venv/bin/python -m pytest skills/tests -q` runs all three layers.

## External skill corpus (task discovery only)

`ralph-task-discovery` consults an operator-local external skill corpus at
`/Users/pittcat/Dev/agent_tools/skills` in **read-only** mode: its skills
inform discovery methods (grilling, domain modeling, bug diagnosis, triage,
etc.) but are never modified, copied, or bundled by this repository. When
the corpus is unavailable, discovery records `external_skill_unavailable:`
+ `fallback:` provenance in the brief and continues with the built-in
fallback checklists — it never fakes an executed external skill. Adapter
rules live in `ralph-task-discovery/references/external-skill-adapters.md`.

## Agent-flow audit standard

`ralph-preset-author` and `ralph-preset-review` are a closed-loop agent-flow audit pair:

- **Author** records per-hat AAF tables **plus payload contract rows** (field, value source, visibility evidence, identity check, downstream use) before handoff.
- **Review** independently simulates each hat's activation from the visible prompt stack — trigger, context, command plan, payload construction, emit precheck, handoff — and produces a report with a per-hat section, a payload audit table, a handoff audit table, and remediation ordered by runtime unblock.

Mechanical lint (`ralph preset check`) only proves shape and topology. **Invisible inputs, fabricated identity fields, vague decision fields, and broken handoffs are caught by these skills, not by lint.** Neither skill replaces `ralph preset check`; both complement it.

## Symlinks (local dev)

> Both skills now ship their own `references/` directory as plain files,
> so the legacy `ln -sf ../ralph-preset-common/references …` step is no
> longer needed. If you want them visible under `.claude/skills` /
> `.cursor/skills` for local development, symlink the whole skill:

```bash
mkdir -p .claude/skills .cursor/skills
ln -sf ../../skills/ralph-preset-author    .claude/skills/ralph-preset-author
ln -sf ../../skills/ralph-preset-review    .claude/skills/ralph-preset-review
ln -sf ../../skills/ralph-run-diagnosis    .claude/skills/ralph-run-diagnosis
ln -sf ../../skills/ralph-run-diagnosis    .cursor/skills/ralph-run-diagnosis
```

On Windows without symlink support, copy the skills manually.

## Install with Claude Code

Add this repository as a marketplace source:

```text
/plugin marketplace add mikeyobrien/ralph-orchestrator
```

Then install the `ralph-orchestrator` plugin from the marketplace browser.

## Install with Vercel `npx skills`

List the skills in this repository:

```bash
npx skills add mikeyobrien/ralph-orchestrator --list
```

Install discovery + preset + bootstrap skills for Claude Code:

```bash
npx skills add mikeyobrien/ralph-orchestrator \
  --skill ralph-task-discovery \
  --skill ralph-preset-author \
  --skill ralph-preset-review \
  --skill ralph-project-bootstrap \
  --skill ralph-run-diagnosis \
  -a claude-code \
  -y
```

Install one skill for Codex-style agents:

```bash
npx skills add mikeyobrien/ralph-orchestrator \
  --skill ralph-preset-review \
  -a codex \
  -y
```

During local development you can also install from the checked-out repo:

```bash
npx skills add . --list

# Or copy public skills into this repo's .claude/skills + .agents/skills
./skills/install.py --force

# Global user install: ~/.claude/skills + ~/.agents/skills
./skills/install.py --global --force
```
