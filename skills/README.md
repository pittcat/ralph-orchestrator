# Ralph Orchestrator Agent Skills

This directory is the canonical public skill package for external agent
harnesses that operate Ralph.

It ships operator skills:

| Skill | Purpose |
|---|---|
| `ralph-e2e-bootstrap` | Bootstrap an E2E sandbox directory from a development plan + git diff via combo-box decisions and static gates; deliver a copy-paste launch command |
| `ralph-preset-author` | Draft presets (builtin + local) with per-hat AAF tables **+ payload contract notes** before review |
| `ralph-preset-review` | Per-hat activation dry-run + **payload audit** + mechanical lint → `preset-review-report.md` |
| `ralph-project-bootstrap` | Audit a target project via ONE unified pipeline entry (`bootstrap_pipeline.py`), generate or safely update the preset-bound `ralph.<stem>.yml` + `PROMPT.<stem>.md` suite and the AGENTS.md / CLAUDE.md managed sections from an existing preset, run staged validation, and hand off the official launch command (`dry-run green != loop closed`) |
| `ralph-run-diagnosis` | Post-run deep diagnosis: artifacts, OPAC, mechanism vs preset attribution |

> **Plan 2026-08-02-001:** the previously bundled `ralph-loop` and
> `ralph-preset-common` skills are gone. `ralph-loop` is retired (loop
> operations stay in the runner CLI / web dashboard). The author/review
> pair now ships its own `references/`, `fixtures/`, and `tests/` files,
> so there is no shared common directory to bundle. `install.py` rejects
> `ralph-loop` requests and copies each skill's local `references/`
> directory verbatim.

These are public agent skills. They are not part of Ralph's internal
`ralph tools skill` registry.

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

Install preset + bootstrap skills for Claude Code:

```bash
npx skills add mikeyobrien/ralph-orchestrator \
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
