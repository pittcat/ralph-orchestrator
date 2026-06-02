---
name: ralph-tools-memories
description: Use when managing runtime memories during Ralph orchestration runs
metadata:
  internal: true
---

# Ralph Tools — Memories

## Memory Commands

```bash
ralph tools memory add "content" -t pattern -t tag1 -t tag2
ralph tools memory add "content" -t private    # Hat-scoped (agent context only)
ralph tools memory list [-t type] [--format FORMAT]
ralph tools memory search "query" [-t type] [--format FORMAT]
ralph tools memory prime -t pattern       # Output for context injection
ralph tools memory show <mem-id>
ralph tools memory delete <mem-id>
ralph tools memory init                   # Initialize memories store
```

Note: All memory commands accept `--type <TYPE>`, `--format <FORMAT>`, `--root <ROOT>`,
and global options. Additional flags per command: `memory list` accepts `--last <N>`,
`memory search` accepts `--all`, `memory prime` accepts `--recent`, `memory init` accepts `--force`.

**Memory types:**

| Type | Flag | Use For |
|------|------|---------|
| pattern | `-t pattern` | "Uses barrel exports", "API routes use kebab-case" |
| decision | `-t decision` | "Chose Postgres over SQLite for concurrent writes" |
| fix | `-t fix` | "ECONNREFUSED on :5432 means run docker-compose up" |
| context | `-t context` | "ralph-core is shared lib, ralph-cli is binary" |

**Memory ID format:** `mem-{timestamp}-{4hex}` (e.g., `mem-1737372000-a1b2`)

## Visibility & Owner

Every memory has a `visibility` (`shared` or `private`) and an optional
`owner_hat_id`:

- **shared** (default): visible to every caller — human CLI and every hat.
  New memories created without `private` flag are shared. **Agents cannot
  delete or mutate shared memories** — only the human CLI may.
- **private** (set with `-t private` in agent context): visible only to the
  owning hat. The owning hat is taken from `RALPH_CURRENT_HAT`. The CLI
  fails closed if `private` flag is used without an agent context.

| Operation | Human CLI | Agent (owner) | Agent (other) |
|-----------|-----------|---------------|---------------|
| `add` (default) | shared | shared | shared |
| `add -t private` | **rejected** (no agent ctx) | private, owned by current hat | private, owned by current hat |
| `list/show/search/prime` | sees all | sees shared + own private | sees shared + own private |
| `delete shared` | allowed | **rejected** | **rejected** |
| `delete private` (any) | allowed | allowed (own) | **rejected** |

The human CLI is the only path that can prune or correct shared knowledge
— this prevents a hat from poisoning or wiping institutional memory.

**Limits enforced by the CLI:**

- `add` rejects empty content.
- `add` rejects content longer than 10 000 characters.
- `add` with `private` visibility rejects after a hat already owns 1 000 private memories.

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
- Start broad, narrow with filters: `search "api"` → `search -t pattern -t api`
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
  -t fix # tag with tooling,error-handling
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
ralph tools memory add -t pattern "Parser requires snake_case keys"
```

### Find relevant memories
```bash
ralph tools memory search -t yaml "config"
ralph tools memory prime -t pattern  # For context injection
```

### Memory examples
```bash
# Pattern: discovered codebase convention
ralph tools memory add -t pattern "All API handlers return Result<Json<T>, AppError>"

# Decision: learned why something was chosen
ralph tools memory add -t decision "Chose JSONL over SQLite: simpler, git-friendly, append-only"

# Fix: solved a recurring problem
ralph tools memory add -t fix "cargo test hangs: kill orphan postgres from previous run"

# Context: project-specific knowledge
ralph tools memory add -t context "The /legacy folder is deprecated, use /v2 endpoints"
```
