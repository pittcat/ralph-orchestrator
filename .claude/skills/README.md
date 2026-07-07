# Ralph Orchestrator Agent Skills

This directory is the canonical public skill package for external agent
harnesses that operate Ralph.

It ships operator skills:

| Skill | Purpose |
|---|---|
| `ralph-hats` | Create, inspect, validate user `.ralph/hats/` collections |
| `ralph-loop` | Run, monitor, resume, merge, debug Ralph loops |
| `ralph-preset-author` | Draft presets (builtin + local) with per-hat AAF tables |
| `ralph-preset-review` | AAF review + `preset-review-report.md` + mechanical lint |
| `ralph-run-diagnosis` | Post-run deep diagnosis: artifacts, OPAC, mechanism vs preset attribution |

`ralph-preset-common/` holds shared `references/` and fixtures (not a standalone marketplace skill).

These are public agent skills. They are not part of Ralph's internal
`ralph tools skill` registry.

## Symlinks (local dev)

Author and review skills symlink `references/` to `ralph-preset-common/references/`:

```bash
ln -sf ../ralph-preset-common/references skills/ralph-preset-author/references
ln -sf ../ralph-preset-common/references skills/ralph-preset-review/references
ln -sf ../../skills/ralph-preset-author .claude/skills/ralph-preset-author
ln -sf ../../skills/ralph-preset-review .claude/skills/ralph-preset-review
ln -sf ../../skills/ralph-run-diagnosis .claude/skills/ralph-run-diagnosis
mkdir -p .cursor/skills
ln -sf ../../skills/ralph-run-diagnosis .cursor/skills/ralph-run-diagnosis
```

On Windows without symlink support, duplicate `references/` and keep in sync manually.

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

Install hat + loop + preset skills for Claude Code:

```bash
npx skills add mikeyobrien/ralph-orchestrator \
  --skill ralph-hats \
  --skill ralph-loop \
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
