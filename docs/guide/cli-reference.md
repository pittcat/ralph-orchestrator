# CLI Reference

Complete reference for Ralph's command-line interface.

## Global Options

These options are accepted by all commands.

| Option | Description |
|--------|-------------|
| `-c, --config <SOURCE>` | Primary config source (can be specified multiple times). Defaults to `ralph.yml`, or `$RALPH_CONFIG` when set. |
| `-H, --hats <SOURCE>` | Hat collection source (`file`, `builtin:<name>`, or URL). |
| `-v, --verbose` | Verbose output |
| `--color <MODE>` | Color output: `auto`, `always`, `never` |
| `-h, --help` | Show help |
| `-V, --version` | Show version |

### Core Config Sources (`-c`)

The `-c` flag specifies where to load **core** configuration from. If not provided, `ralph` falls back to:

1. `$RALPH_CONFIG` when present
2. `ralph.yml`

**Core source types:**

| Format | Description |
|--------|-------------|
| `ralph.yml` | Local file path |
| `https://example.com/ralph.core.yml` | Remote URL |
| `core.field=value` | Core config override |

> `-c builtin:<name>` is no longer supported. Use `-H builtin:<name>` for hat collections.

The first non-override core source is used as the base config. Later core overrides replace earlier values.

Backward compatibility: a `-c` config file may still contain `hats`/`events` (single-file combined config).

If `-H/--hats` is provided, it takes precedence over hats in `-c`:
- `hats` and `events` from `-H` replace `hats`/`events` from `-c`
- `event_loop` values from `-H` override matching `event_loop` keys from `-c`
- `-c core.*=...` overrides are still applied last

**Supported override fields:**

| Field | Description |
|-------|-------------|
| `core.scratchpad` | Path to scratchpad file (string shorthand for `scratchpad.path`) |
| `core.specs_dir` | Path to specs directory |

### Hat Collection Sources (`-H`)

The `-H` flag specifies where to load hat collections from.

| Format | Description |
|--------|-------------|
| `hats/feature.yml` | Local hats file |
| `builtin:code-assist` | Built-in hat collection |
| `https://example.com/hats.yml` | Remote hats file |

**Examples:**

```bash
# Core only (hatless)
ralph run -c ralph.yml

# Core + built-in hat collection
ralph run -c ralph.yml -H builtin:code-assist

# Core + file hat collection
ralph run -c ralph.yml -H hats/review.yml

# Core override + hats
ralph run -c ralph.yml -c core.specs_dir=./my-specs -H builtin:debug
```

## Commands

### ralph run

Run the orchestration loop.

```bash
ralph run [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `-p, --prompt <TEXT>` | Inline prompt text |
| `-P, --prompt-file <FILE>` | Prompt file path |
| `--max-iterations <N>` | Override max iterations |
| `--completion-promise <TEXT>` | Override completion trigger |
| `--dry-run` | Show what would execute |
| `--no-tui` | Disable TUI mode |
| `-a, --autonomous` | Force headless mode |
| `--idle-timeout <SECS>` | Interactive-mode idle timeout; `0` disables it |
| `--autonomous-idle-timeout <SECS>` | Autonomous/RPC/worktree backend inactivity watchdog; defaults to adapter timeout, `0` disables it |
| `--exclusive` | Wait for primary loop slot (conflicts with `--worktree`) |
| `--worktree` | Create isolated git worktree for this run at `.worktrees/<loop-id>/`. **End-to-end isolation contract**: all loop artifacts (`.ralph/events.jsonl`, `.ralph/diagnostics/logs/`, `.ralph/current-events` marker) land inside the worktree, never the main repo. The only exception is `.ralph/loops.json` (the registry is shared across worktrees and lives in the main repo). The worktree and branch are preserved after completion for manual merge or cleanup. Conflicts with `--exclusive` |
| `--no-auto-merge` | Skip automatic merge after worktree loops complete |
| `--skip-preflight` | Skip auto preflight checks (even when `features.preflight.enabled: true`) |
| `--record-session <FILE>` | Record session JSONL |
| `-q, --quiet` | Suppress streaming output |
| `--continue` | Resume from existing state |

### ralph init

Initialize `ralph.yml`.

```bash
ralph init [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--backend <NAME>` | Backend: `claude`, `kiro`, `gemini`, `codex`, `amp`, `copilot`, `opencode`, `pi`, `custom` |
| `--preset <NAME>` | Removed (monolithic presets no longer supported) |
| `--list-presets` | List available built-in hat collections |
| `--force` | Overwrite existing config |

### ralph preflight

Run the preflight check suite.

```bash
ralph preflight [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--format <human|json>` | Output format |
| `--strict` | Treat warnings as failures |
| `--check <NAME>` | Run one or more checks by name |

Default check names:

- `config`
- `hooks`
- `backend`
- `telegram`
- `git`
- `paths`
- `tools`
- `specs`

Notes:

- `--check` can be repeated (for example: `--check hooks --check config`).
- `--strict` fails when there are warnings (not just failures).
- During `ralph run`, auto-preflight uses `features.preflight.skip` to skip checks by these names.

### ralph hooks

Validate hooks configuration and command wiring without starting loop execution.

```bash
ralph hooks <COMMAND>
```

**Subcommands:**

- `validate [--format human|json]`

`ralph hooks validate` behavior:

- Exit code `0`: validation passed.
- Exit code `1`: one or more diagnostics (or config load/parse failure).
- `--format human` (default): readable report with diagnostics.
- `--format json`: structured report (`pass`, `source`, `hooks_enabled`, `checked_hooks`, `diagnostics`).

Try it against the minimal sample hooks config:

- `ralph hooks validate -c examples/hooks/minimal/ralph.hooks.yml`
- Config: [`examples/hooks/minimal/ralph.hooks.yml`](https://github.com/mikeyobrien/ralph-orchestrator/blob/main/examples/hooks/minimal/ralph.hooks.yml)
- Scripts: [`examples/hooks/scripts/env-guard.sh`](https://github.com/mikeyobrien/ralph-orchestrator/blob/main/examples/hooks/scripts/env-guard.sh), [`examples/hooks/scripts/notify.sh`](https://github.com/mikeyobrien/ralph-orchestrator/blob/main/examples/hooks/scripts/notify.sh)

### ralph doctor

Run environment and first-run diagnostic checks.

```bash
ralph doctor [OPTIONS]
```

### ralph tutorial

Run interactive intro walkthrough.

```bash
ralph tutorial [OPTIONS]
```

### ralph preset

Manage and validate presets (includes template authoring commands).

```bash
ralph preset [OPTIONS] [COMMAND]
```

**Subcommands:**

- `list [--format human|json]` — List available workflow templates
- `show <NAME> [--format human|yaml]` — Show template details
- `new <TEMPLATE> [--name <NAME>] [--description <TEXT>] [--output <PATH>] [--force] [--check] [--format human|json]` — Generate a new preset from a template
- `check [--format human|json] [--strict]` — Check preset/workflow contract (config, topology, payload, orphan)
- `diff --file <FILE> [--format human|json]` — Show differences between a local preset and its template baseline
- `upgrade --file <FILE> [--format human|json]` — Preview upgrade information (dry-run only)

**Examples:**

```bash
# List available templates
ralph preset list

# Show template details
ralph preset show code-assist --format yaml

# Generate a local preset
ralph preset new code-assist --name my-flow --output .ralph/hats/my-flow.yml

# Validate a preset
ralph preset check -H .ralph/hats/my-flow.yml --strict

# See differences from template baseline
ralph preset diff --file .ralph/hats/my-flow.yml

# Preview upgrade (dry-run)
ralph preset upgrade --file .ralph/hats/my-flow.yml --dry-run
```

Notes:
- Templates are authoring scaffolds; they do not become builtin presets.
- `x_preset` metadata in generated files does not affect runtime behavior.
- See [Preset Authoring Guide](./preset-authoring.md) for the full authoring workflow.

### ralph plan

Start an interactive PDD planning session.

```bash
ralph plan [OPTIONS] [IDEA]
```

**Options:**

| Option | Description |
|--------|-------------|
| `<IDEA>` | Optional rough idea |
| `-b, --backend <BACKEND>` | Backend override |
| `--teams` | Enable Claude Code agent teams mode |
| `-- <ARGUMENTS>` | Custom backend arguments |

### ralph code-task

Generate code task files from a description or PDD plan.

```bash
ralph code-task [OPTIONS] [INPUT]
```

### ralph task

Deprecated legacy alias for `ralph code-task`.

```bash
ralph task [OPTIONS] [INPUT]
```

### ralph events

View event history for the current or selected run.

```bash
ralph events [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--file <PATH>` | Use a specific events file |
| `--clear` | Clear event history. **Requires `--confirm <loop_id>`** to authorize (P8). |
| `--confirm <LOOP_ID>` | Confirmation token: must equal the active loop id (from `.ralph/current-loop-id`), or `current` / `default` when no loop marker exists. |

**Example (safe):**

```bash
# Discover the active loop id, then authorize the clear.
cat .ralph/current-loop-id
ralph events --clear --confirm loop-1735731234-abcd
```

Without `--confirm`, the clear is refused. A wrong confirm is also refused.

### ralph emit

Emit an event to the current run's events file.

```bash
ralph emit <TOPIC> [PAYLOAD] [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `<TOPIC>` | Event topic (e.g., `build.done`) |
| `[PAYLOAD]` | Optional payload (string or JSON when `--json` is set) |
| `-j, --json` | Parse payload as JSON object |
| `--file <PATH>` | Events file path (`.ralph/events.jsonl`) |
| `--policy-check` | Validate event against current event policy before emitting |
| `--unsafe-no-policy-check` | Bypass mandatory policy check (only when config permits) |
| `--hat <HAT>` | Hat that published this event (falls back to `$RALPH_CURRENT_HAT`) |
| `--triggered <HAT>` | Target hat triggered by this event (falls back to `$RALPH_TRIGGERED_HAT`) |
| `--source <SOURCE>` | Source identifier for this event (falls back to `$RALPH_EVENT_SOURCE`) |

**P6 path restriction:** the resolved events file is validated against an
allowlist. Allowed targets are the `current-candidate-events` marker
target, the `current-events` marker target, and the default
`events.jsonl` (only when no marker exists). Any other path — from
`--file`, `RALPH_EVENTS_FILE`, or path traversal — is rejected. Symlinks
that alias an allowlist entry to a path outside the workspace are also
rejected.

### ralph clean

Clean `.ralph/agent` scratchpad and memory state.

```bash
ralph clean [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--diagnostics` | Clean diagnostics directory |
| `--dry-run` | Preview deletions |

### ralph loops

Manage parallel loops and worktree loop lifecycle.

```bash
ralph loops [OPTIONS] [COMMAND]
```

**Subcommands:**

- `list [--json] [--all]`
- `logs <loop-id> [--follow]`
- `history <loop-id> [--json]`
- `retry <loop-id>`
- `discard <loop-id> [--yes]`
- `stop [loop-id] [--force]`
- `resume <loop-id>`
- `prune`
- `attach <loop-id>`
- `diff <loop-id> [--stat]`
- `merge <loop-id> [--force]`
- `process`
- `merge-button-state <loop-id>`

`ralph loops resume <loop-id>` writes a resume signal for suspended loops. It is idempotent:
re-running the command reports that resume was already requested (or that the loop is not suspended).

### ralph hats

Manage and inspect configured hats.

```bash
ralph hats [OPTIONS] [COMMAND]
```

**Subcommands:**

- `list [--format table|json]`
- `show <name>`
- `validate`
- `graph [--format unicode|ascii|compact|mermaid] [--backend <backend>]`

### ralph web

Run the web dashboard.

```bash
ralph web [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--backend-port <BACKEND_PORT>` | RPC API port (default: 3000) |
| `--frontend-port <FRONTEND_PORT>` | Frontend port (default: 5173) |
| `--workspace <WORKSPACE>` | Workspace root |
| `--legacy-node-api` | Run deprecated Node tRPC backend instead of Rust RPC API |
| `--no-open` | Do not open browser |

### ralph mcp

Run Ralph as a Model Context Protocol server over `stdio`.

```bash
ralph mcp serve
```

Notes:

- v1 is tools-only and `stdio`-only.
- Launch it from an MCP client configuration, not an interactive terminal workflow.
- The server exposes Ralph control-plane methods as MCP tools, including polling stream tools such as `stream_next`.

### ralph bot

Manage Telegram bot setup and testing.

```bash
ralph bot [OPTIONS] <COMMAND>
```

**Subcommands:**

- `onboard [--token <TOKEN>] [--chat-id <CHAT_ID>] [--timeout <SECONDS>]`
- `status`
- `test [MESSAGE]`
- `token set <TOKEN> [--config <path>]`
- `daemon`

### ralph wave

Dispatch wave events for parallel hat execution.

```bash
ralph wave emit <TOPIC> --payloads <ITEM>...
```

**Options:**

| Option | Description |
|--------|-------------|
| `<TOPIC>` | Event topic targeting a wave-capable hat |
| `--payloads <ITEM>...` | One or more payloads, each becomes a separate event |

Each payload becomes an event tagged with a shared `wave_id`. The loop runner spawns parallel backend instances bounded by the target hat's `concurrency` setting.

Blocked when `RALPH_WAVE_WORKER=1` (prevents nested waves).

See [Agent Waves](../advanced/agent-waves.md) for full details.

### ralph tools

Runtime tools for memories, tasks, and skills.

#### ralph tools memory

```bash
ralph tools memory <SUBCOMMAND>
```

**Subcommands:**

| Command | Description |
|---------|-------------|
| `init` | Initialize memory file |
| `add <CONTENT>` | Store a new memory |
| `search <QUERY>` | Search memories |
| `list` | List memories |
| `show <ID>` | Show a memory |
| `delete <ID>` | Delete a memory |
| `prime` | Prime context memory output |

#### ralph tools task

```bash
ralph tools task <SUBCOMMAND>
```

**Subcommands:**

| Command | Description |
|---------|-------------|
| `add <TITLE>` | Create a task |
| `list` | List all tasks |
| `ready` | List unblocked tasks |
| `close <ID>` | Mark task complete |
| `fail <ID>` | Mark task failed |
| `show <ID>` | Show task details |

#### ralph tools skill

```bash
ralph tools skill <SUBCOMMAND>
```

#### ralph tools interact

Interact with human via Telegram progress/proactiveness hooks.

### ralph completions

Generate shell completions.

```bash
ralph completions <SHELL>
```

Supported shells: `bash`, `elvish`, `fish`, `powershell`, `zsh`.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Completion promise reached (`LOOP_COMPLETE`) |
| 1 | Failure or stop condition (failure/cancelled/throttled state) |
| 2 | Runtime limits reached (`max-iterations`, `max-runtime`, or `max-cost`) |
| 3 | Loop requested restart |
| 130 | Interrupted by signal (Ctrl-C / SIGINT) |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `RALPH_DIAGNOSTICS` | Set to `1` to enable diagnostics |
| `RALPH_CONFIG` | Default config file path |
| `NO_COLOR` | Disable color output |
| `RALPH_WAVE_WORKER` | Set to `1` inside wave workers (blocks nested waves) |
| `RALPH_WAVE_ID` | Wave correlation ID (set on wave workers) |
| `RALPH_WAVE_INDEX` | 0-based worker index within the wave |
| `RALPH_EVENTS_FILE` | Per-worker events file path (set on wave workers) |
| `RALPH_CURRENT_HAT` | Current hat ID injected by the loop runner |
| `RALPH_CURRENT_LOOP_ID` | Current loop ID injected by the loop runner |
| `RALPH_TRIGGERED_HAT` | Target hat triggered by the current event |
| `RALPH_EVENT_SOURCE` | Override source identifier for CLI emits |

## Shell Completion

Generate shell completions:

```bash
# Bash
ralph completions bash > ~/.local/share/bash-completion/completions/ralph

# Zsh
ralph completions zsh > ~/.zfunc/_ralph

# Fish
ralph completions fish > ~/.config/fish/completions/ralph.fish
```
