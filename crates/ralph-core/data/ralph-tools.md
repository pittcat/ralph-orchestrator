---
name: ralph-tools
description: Shared tool commands for interact, skill, and output format reference during Ralph orchestration
metadata:
  internal: true
---

# Ralph Tools

Quick reference for shared `ralph tools` commands used during orchestration.

## Interact Commands

```bash
ralph tools interact progress "message"
```

Send a non-blocking progress update via the configured RObot (Telegram).

## Skill Commands

```bash
ralph tools skill list
ralph tools skill load <name>
```

List available skills or load a specific skill by name.

## Output Formats

All commands support `--format`:
- `table` (default) - Human-readable
- `json` - Machine-parseable
- `quiet` - IDs only (for scripting)
- `markdown` - Memory prime only

**NEVER use echo/cat to write tasks or memories** — always use CLI tools.
