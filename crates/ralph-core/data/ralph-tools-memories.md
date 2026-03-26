---
name: ralph-tools-memories
description: Use when managing runtime memories during Ralph orchestration runs
metadata:
  internal: true
---

# Ralph Tools — Memories

## Memory Commands

```bash
ralph tools memory add "content" -t pattern --tags tag1,tag2
ralph tools memory list [-t type] [--tags tags]
ralph tools memory search "query" [-t type] [--tags tags]
ralph tools memory prime --budget 2000    # Output for context injection
ralph tools memory show <mem-id>
ralph tools memory delete <mem-id>
```

**Memory types:**

| Type | Flag | Use For |
|------|------|---------|
| pattern | `-t pattern` | "Uses barrel exports", "API routes use kebab-case" |
| decision | `-t decision` | "Chose Postgres over SQLite for concurrent writes" |
| fix | `-t fix` | "ECONNREFUSED on :5432 means run docker-compose up" |
| context | `-t context` | "ralph-core is shared lib, ralph-cli is binary" |

**Memory ID format:** `mem-{timestamp}-{4hex}` (e.g., `mem-1737372000-a1b2`)

### First thing every iteration
```bash
ralph tools memory search "area-name"   # If you're entering an unfamiliar area
```

### When to Search Memories

**Search BEFORE starting work when:**
- Entering unfamiliar code area → `ralph tools memory search "area-name"`
- Encountering an error → `ralph tools memory search -t fix "error message"`
- Making architectural decisions → `ralph tools memory search -t decision "topic"`
- Something feels familiar → there might be a memory about it

**Search strategies:**
- Start broad, narrow with filters: `search "api"` → `search -t pattern --tags api`
- Check fixes first for errors: `search -t fix "ECONNREFUSED"`
- Review decisions before changing architecture: `search -t decision`

### When to Create Memories

**Create a memory when:**
- You discover how this codebase does things (pattern)
- You make or learn why an architectural choice was made (decision)
- You solve a problem that might recur (fix)
- You learn project-specific knowledge others need (context)
- Any non-zero command, missing dependency/skill, or blocked step (fix + task if unresolved)

**Do NOT create memories for:**
- Session-specific state (use tasks instead)
- Obvious/universal practices
- Temporary workarounds

### Failure Capture — Memory Half

If any command fails (non-zero exit), or you hit a missing dependency/skill, or you are blocked:
- **Record a fix memory** with the exact command, error, and intended fix.

```bash
ralph tools memory add \
  "failure: cmd=<command>, exit=<code>, error=<message>, next=<intended fix>" \
  -t fix --tags tooling,error-handling
```

### Discover Available Tags

Before searching or adding, check what tags already exist:

```bash
ralph tools memory list
grep -o 'tags: [^|]*' .agent/memories.md | sort -u
```

Reuse existing tags for consistency. Common tag patterns:
- Component names: `api`, `auth`, `database`, `cli`
- Concerns: `testing`, `performance`, `error-handling`
- Tools: `docker`, `postgres`, `redis`

### Memory Best Practices

1. **Be specific**: "Uses barrel exports in each module" not "Has good patterns"
2. **Include why**: "Chose X because Y" not just "Uses X"
3. **One concept per memory**: Split complex learnings
4. **Tag consistently**: Reuse existing tags when possible

## Decision Journal

Use `.ralph/agent/decisions.md` to capture consequential decisions and their
confidence scores. Follow the template at the top of the file and keep IDs
sequential (DEC-001, DEC-002, ...).

Confidence thresholds:
- **>80**: Proceed autonomously.
- **50-80**: Proceed, but document the decision in `.ralph/agent/decisions.md`.
- **<50**: Choose the safest default and document the decision in `.ralph/agent/decisions.md`.

Template fields:
- Decision
- Chosen Option
- Confidence (0-100)
- Alternatives Considered
- Reasoning
- Reversibility
- Timestamp (UTC ISO 8601)

## Common Workflows

### Store a discovery
```bash
ralph tools memory add "Parser requires snake_case keys" -t pattern --tags config,yaml
```

### Find relevant memories
```bash
ralph tools memory search "config" --tags yaml
ralph tools memory prime --budget 1000 -t pattern  # For injection
```

### Memory examples
```bash
# Pattern: discovered codebase convention
ralph tools memory add "All API handlers return Result<Json<T>, AppError>" -t pattern --tags api,error-handling

# Decision: learned why something was chosen
ralph tools memory add "Chose JSONL over SQLite: simpler, git-friendly, append-only" -t decision --tags storage,architecture

# Fix: solved a recurring problem
ralph tools memory add "cargo test hangs: kill orphan postgres from previous run" -t fix --tags testing,postgres

# Context: project-specific knowledge
ralph tools memory add "The /legacy folder is deprecated, use /v2 endpoints" -t context --tags api,migration
```
