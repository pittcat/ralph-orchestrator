# Ralph Orchestrator Agent Skills

This directory is the canonical public skill package for external agent
harnesses that operate Ralph.

It ships operator skills:

| Skill | Purpose |
|---|---|
| `ralh-hats` | Create, inspect, validate user `.ralph/hats/` collections |
| `ralph-preset-author` | Draft presets (builtin + local) with per-hat AAF tables **+ payload contract notes** before review |
| `ralph-preset-review` | Per-hat activation dry-run + **payload audit** + mechanical lint → `preset-review-report.md` |
| `ralph-run-diagnosis` | Post-run deep diagnosis: artifacts, OPAC, mechanism vs preset attribution |

> **Plan 2026-08-02-001:** the previously bundled `ralph-loop` and
> `ralph-preset-common` skills are gone. `ralph-loop` is retired (loop
> operations stay in the runner CLI / web dashboard). The
> author/review pair now ships its own `references/`, `fixtures/`,
> and `tests/` files, so there is no shared common directory to bundle.

These are public agent skills. They are not part of Ralph's internal
`ralph tools skill` registry.

## Agent-flow audit standard

`ralph-preset-author` and `ralph-preset-review` are a closed-loop agent-flow audit pair:

- **Author** records per-hat AAF tables **plus payload contract rows** (field, value source, visibility evidence, identity check, downstream use) before handoff.
- **Review** independently simulates each hat's activation from the visible prompt stack — trigger, context, command plan, payload construction, emit precheck, handoff — and produces a report with a per-hat section, a payload audit table, a handoff audit table, and remediation ordered by runtime unblock.

Mechanical lint (`ralph preset check`) only proves shape and topology. **Invisible inputs, fabricated identity fields, vague decision fields, and broken handoffs are caught by these skills, not by lint.** Neither skill replaces `ralph preset check`; both complement it.

## Symlinks (local dev)

> Symlinks are no longer required for `references/`: both author and
> review ship their own. If you want them in `.claude/skills` and
> `.cursor/skills` for local development, use plain `ln -s`:

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

Install hat + preset skills for Claude Code:

```bash
npx skills add mikeyobrien/ralph-orchestrator \
  --skill ralph-hats \
  --skill ralph-preset-author \
  --skill ralph-preset-review \
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
```
