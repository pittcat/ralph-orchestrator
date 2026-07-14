# CLI Reference

Complete reference for Ralph's command-line interface.

## Global Options

These options are accepted by all commands.

| Option | Description |
|--------|-------------|
| `-c, --config <CONFIG>` | Core configuration source: file path, URL, or `core.field=value` override. Can be specified multiple times. If not set, defaults to `ralph.yml` or `$RALPH_CONFIG`. Subcommands that load project config on the agent side (`ralph tools task`, `ralph emit`, `ralph wave emit`, `ralph tools skill`) honour the same precedence and additionally fall back to `RALPH_CONFIG` injected by `ralph run`. |
| `-H, --hats <HATS>` | Hat collection source: file path, `builtin:<name>`, or URL. |
| `-v, --verbose` | Verbose output. |
| `--color <MODE>` | Color output mode: `auto`, `always`, `never` (default: `auto`). |
| `-h, --help` | Show help. |
| `-V, --version` | Show version. |

### Core Config Sources (`-c`)

The `-c` flag specifies where to load **core** configuration from. If not provided, `ralph` falls back to:

1. `$RALPH_CONFIG` when present.
2. `ralph.yml` / `ralph.yaml` (workspace discovery).

**Core source types:**

| Format | Description |
|--------|-------------|
| `ralph.yml` | Local file path. |
| `https://example.com/ralph.core.yml` | Remote URL. |
| `core.field=value` | Core config override. |

> `-c builtin:<name>` is no longer supported. Use `-H builtin:<name>` for hat collections.

The first non-override core source is used as the base config. Later core overrides replace earlier values.

Backward compatibility: a `-c` config file may still contain `hats`/`events` (single-file combined config).

If `-H/--hats` is provided, it takes precedence over hats in `-c`:

- `hats` and `events` from `-H` replace `hats`/`events` from `-c`.
- `event_loop` values from `-H` override matching `event_loop` keys from `-c`.
- `-c core.*=...` overrides are still applied last.

**Supported override fields:**

| Field | Description |
|-------|-------------|
| `core.scratchpad` | Path to scratchpad file (string shorthand for `scratchpad.path`). |
| `core.specs_dir` | Path to specs directory. |

### Hat Collection Sources (`-H`)

The `-H` flag specifies where to load hat collections from.

| Format | Description |
|--------|-------------|
| `hats/feature.yml` | Local hats file. |
| `builtin:debug` | Built-in hat collection. |
| `https://example.com/hats.yml` | Remote hats file. |

**Examples:**

```bash
# Core only (hatless)
ralph run -c ralph.yml

# Core + built-in hat collection
ralph run -c ralph.yml -H builtin:debug

# Core + file hat collection
ralph run -c ralph.yml -H hats/review.yml

# Core override + hats
ralph run -c ralph.yml -c core.specs_dir=./my-specs -H builtin:debug
```

## Commands

### ralph run

Run the orchestration loop.

```bash
ralph run [OPTIONS] [-- <CUSTOM_ARGS>...]
```

**Options:**

| Option | Description |
|--------|-------------|
| `-p, --prompt <PROMPT_TEXT>` | Inline prompt text (mutually exclusive with `-P`). |
| `-P, --prompt-file <PROMPT_FILE>` | Prompt file path (mutually exclusive with `-p`). |
| `-b, --backend <BACKEND>` | Override backend from config. |
| `--max-iterations <N>` | Override max iterations. |
| `--completion-promise <TEXT>` | Override completion trigger. |
| `--dry-run` | Show what would execute without running. |
| `--continue` | Continue from existing scratchpad (resume interrupted loop). |
| `--loop-id <LOOP_ID>` | Explicit loop ID to use with `--continue`. |
| `--no-tui` | Disable TUI observation mode. |
| `-a, --autonomous` | Force headless/autonomous mode. |
| `--rpc` | Run in RPC mode with JSON-lines protocol on stdin/stdout. |
| `--idle-timeout <SECS>` | Interactive-mode idle timeout; `0` disables it (default: 30). |
| `--autonomous-idle-timeout <SECS>` | Watchdog for autonomous/RPC/worktree paths; `0` disables it. |
| `--exclusive` | Wait for the primary loop slot (conflicts with `--worktree`). |
| `--no-auto-merge` | Skip automatic merge after worktree loops complete. |
| `--worktree` | Create isolated git worktree at `.worktrees/<loop-id>/`. |
| `--reuse-worktree` | Reuse an existing, completed worktree (only with `--worktree`). |
| `--plan <PATH>` | Explicit plan file path; also used as worktree name prefix. |
| `--worktree-name <NAME>` | Explicit worktree name (with `--worktree`). |
| `--warmup-only` | Exit loop after warmup completes. |
| `--force-warmup` | Force warmup phase even if already completed. |
| `--skip-preflight` | Skip preflight checks before loop start. |
| `--no-sync-agent-docs` | Skip agent doc sync before loop start. |
| `-q, --quiet` | Suppress streaming output. |
| `--record-session <FILE>` | Record session JSONL. |
| `--profile <SCOPE:NAME>` | Activate a runtime profile overlay (repeatable). |
| `--no-default-profiles` | Disable `profiles.default` from `ralph.yml`. |

`[CUSTOM_ARGS]...` are custom backend command and arguments, passed after `--`.

### ralph inspect

Read-only diagnostics that do not modify runtime state.

```bash
ralph inspect [SUBCOMMAND]
```

**Subcommands:**

- `profiles [OPTIONS]` — Preview profile overlay resolution.
- `loop [OPTIONS]` — Read-only diagnostic of the active loop and hat identity.

#### ralph inspect profiles

```bash
ralph inspect profiles [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--profile <SCOPE:NAME>` | Activate a runtime profile overlay (repeatable). |
| `--no-default-profiles` | Disable `profiles.default` from `ralph.yml`. |
| `--format <human|json>` | Output format (default: `human`). |

#### ralph inspect loop

```bash
ralph inspect loop [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--hat <HAT>` | Optional hat ID override (defaults to `$RALPH_CURRENT_HAT`). |
| `--format <human|json>` | Output format (default: `human`). |
| `--root <ROOT>` | Workspace root (default: current directory). |

### ralph init

Initialize a new `ralph.yml` configuration file.

```bash
ralph init [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--backend <BACKEND>` | Backend: `claude`, `kiro`, `gemini`, `codex`, `amp`, `copilot`, `opencode`, `pi`, `custom`. |
| `--preset <PRESET>` | Removed. Monolithic presets are no longer supported; use `-H builtin:<collection>`. |
| `--list-presets` | List available built-in hat collections. |
| `--force` | Overwrite existing `ralph.yml`. |

### ralph preflight

Run preflight checks to validate configuration and environment.

```bash
ralph preflight [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--format <human|json>` | Output format (default: `human`). |
| `--strict` | Treat warnings as failures. |
| `--check <NAME>` | Run one or more checks by name (repeatable). |

Default check names include `config`, `hooks`, `backend`, `git`, `paths`, `tools`, and `specs`.

### ralph hooks

Validate hooks configuration and command wiring.

```bash
ralph hooks <COMMAND>
```

**Subcommands:**

- `validate [--format <human|json>]` — Validate hooks configuration.

`ralph hooks validate` behavior:

- Exit code `0`: validation passed.
- Exit code `1`: one or more diagnostics (or config load/parse failure).
- `--format human` (default): readable report with diagnostics.
- `--format json`: structured report (`pass`, `source`, `hooks_enabled`, `checked_hooks`, `diagnostics`).

### ralph doctor

Run first-run diagnostics and environment checks.

```bash
ralph doctor [OPTIONS] [COMMAND]
```

**Subcommands:**

- `plan-sync [OPTIONS]` — Detect plan frontmatter drift against `.ralph/agent/tasks.jsonl`.

`ralph doctor plan-sync` options:

| Option | Description |
|--------|-------------|
| `--plan <PLAN>` | Path to the plan markdown file. If omitted, scans for the most recent `.ralph` plan under `docs/plans/` or `docs/achieved/plan/`. |

### ralph tutorial

Interactive walkthrough of hats, hat collections, and workflow.

```bash
ralph tutorial [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--no-input` | Skip prompts and print the tutorial in one pass. |

### ralph preset

Manage and validate presets.

```bash
ralph preset [OPTIONS] <COMMAND>
```

**Subcommands:**

- `list [--format <human|json>]` — List available workflow templates.
- `show <NAME> [--format <human|yaml|json>]` — Show template details.
- `new <TEMPLATE> [--name <NAME>] [--description <TEXT>] [--output <PATH>] [--force] [--check] [--format <human|json>]` — Generate a new preset from a template.
- `check [--format <human|json>] [--strict]` — Check preset/workflow contract.
- `diff --file <FILE> [--format <human|json>]` — Show differences from the template baseline.
- `upgrade --file <FILE> [--format <human|json>] [--dry-run] [--force]` — Preview upgrade information (dry-run only).

**Examples:**

```bash
# List available templates
ralph preset list

# Show template details
ralph preset show minimal-linear --format yaml

# Generate a local preset
ralph preset new minimal-linear --name my-flow --output .ralph/hats/my-flow.yml

# Validate a preset
ralph preset check -H .ralph/hats/my-flow.yml --strict

# See differences from template baseline
ralph preset diff --file .ralph/hats/my-flow.yml

# Preview upgrade (dry-run)
ralph preset upgrade --file .ralph/hats/my-flow.yml --dry-run
```

Notes:

- Templates are authoring scaffolds; they do not become built-in presets.
- `x_preset` metadata in generated files does not affect runtime behavior.
- See [Preset Authoring Guide](./preset-authoring.md) for the full authoring workflow.

### ralph plan

Start a Prompt-Driven Development planning session.

```bash
ralph plan [OPTIONS] [IDEA] [-- <CUSTOM_ARGS>...]
```

**Options:**

| Option | Description |
|--------|-------------|
| `[IDEA]` | Optional rough idea. |
| `-b, --backend <BACKEND>` | Backend override. |
| `--teams` | Enable Claude Code agent teams mode. |

`[CUSTOM_ARGS]...` are custom backend arguments, passed after `--`.

### ralph code-task

Generate code task files from descriptions or plans.

```bash
ralph code-task [OPTIONS] [INPUT] [-- <CUSTOM_ARGS>...]
```

**Options:**

| Option | Description |
|--------|-------------|
| `[INPUT]` | Description text or path to a PDD plan file. |
| `-b, --backend <BACKEND>` | Backend override. |
| `--teams` | Enable Claude Code agent teams mode. |

### ralph task

Deprecated legacy alias for `ralph code-task`.

```bash
ralph task [OPTIONS] [INPUT] [-- <CUSTOM_ARGS>...]
```

### ralph events

View event history for debugging.

```bash
ralph events [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--last <N>` | Show only the last N events. |
| `--topic <TOPIC>` | Filter by topic (e.g., `build.blocked`). |
| `--iteration <N>` | Filter by iteration number. |
| `--format <table|json>` | Output format (default: `table`). |
| `--file <PATH>` | Path to events file (default: auto-detects current run). |
| `--events-source <auto|main|hat-channel>` | Which events ledger to read (default: `auto`). |
| `--clear` | Clear event history; requires `--confirm`. |
| `--confirm <LOOP_ID>` | Confirmation token authorizing `--clear`. |

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
ralph emit [OPTIONS] [TOPIC] [PAYLOAD]
```

**Options:**

| Option | Description |
|--------|-------------|
| `[TOPIC]` | Event topic (e.g., `build.done`). Required unless `--schema` is set. |
| `[PAYLOAD]` | Optional payload; string or JSON when `-j` is set (default: empty). |
| `-j, --json` | Parse payload as JSON object. |
| `--file <PATH>` | Events file path (default: `.ralph/events.jsonl`). |
| `--policy-check` | Validate event against current event policy before emitting. |
| `--unsafe-no-policy-check` | Bypass mandatory policy check (only when config permits). |
| `--hat <HAT>` | Hat that published this event (falls back to `$RALPH_CURRENT_HAT`). |
| `--triggered <HAT>` | Target hat triggered by this event (falls back to `$RALPH_TRIGGERED_HAT`). |
| `--source <SOURCE>` | Source identifier for this event (falls back to `$RALPH_EVENT_SOURCE`). |
| `--schema <TOPIC>` | Print the embedded protocol JSON view for `TOPIC`; no event is emitted. |

**P6 path restriction:** the resolved events file is validated against an
allowlist. Allowed targets are the `current-candidate-events` marker
target, the `current-events` marker target, and the default
`events.jsonl` (only when no marker exists). Any other path — from
`--file`, `RALPH_EVENTS_FILE`, or path traversal — is rejected. Symlinks
that alias an allowlist entry to a path outside the workspace are also
rejected.

### ralph clean

Clean up Ralph artifacts from `.ralph/agent`.

```bash
ralph clean [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--dry-run` | Preview deletions without deleting. |
| `--diagnostics` | Clean diagnostic logs instead of `.ralph/`. |

### ralph loops

Manage parallel loops and worktree loop lifecycle.

```bash
ralph loops [OPTIONS] <COMMAND>
```

**Subcommands:**

| Subcommand | Options |
|------------|---------|
| `list` | `[--json] [--all]` |
| `logs <LOOP_ID>` | `[-f, --follow]` |
| `history <LOOP_ID>` | `[--json]` |
| `retry <LOOP_ID>` | — |
| `discard <LOOP_ID>` | `[-y, --yes]` |
| `stop [LOOP_ID]` | `[--force]` |
| `resume <LOOP_ID>` | — |
| `prune` | — |
| `attach <LOOP_ID>` | — |
| `diff <LOOP_ID>` | `[--stat]` |
| `merge <LOOP_ID>` | `[--force]` |
| `process` | — |
| `merge-button-state <LOOP_ID>` | — |

`ralph loops resume <LOOP_ID>` writes a resume signal for suspended loops. It is idempotent: re-running the command reports that resume was already requested (or that the loop is not suspended).

### ralph hats

Manage and inspect configured hats.

```bash
ralph hats [OPTIONS] <COMMAND>
```

**Subcommands:**

| Subcommand | Options |
|------------|---------|
| `list` | `[--format <table|json>]` |
| `show <NAME>` | — |
| `validate` | `[--strict]` |
| `graph` | `[--format <unicode|ascii|compact|mermaid>] [-b, --backend <BACKEND>]` |

### ralph tui

Attach a TUI to a running `ralph-api` server.

```bash
ralph tui [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `-u, --url <URL>` | `ralph-api` server URL (defaults to `$RALPH_API_URL` or `http://127.0.0.1:3000`). |

### ralph web

Run the web dashboard.

```bash
ralph web [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--backend-port <PORT>` | RPC API port (default: 3000). |
| `--frontend-port <PORT>` | Frontend port (default: 5173). |
| `--workspace <WORKSPACE>` | Workspace root directory. |
| `--legacy-node-api` | Use deprecated Node tRPC backend instead of Rust RPC API. |
| `--no-open` | Do not open the dashboard in the default browser. |

### ralph mcp

Run Ralph as a Model Context Protocol server over `stdio`.

```bash
ralph mcp serve
```

Notes:

- v1 is tools-only and `stdio`-only.
- Launch it from an MCP client configuration, not an interactive terminal workflow.
- The server exposes Ralph control-plane methods as MCP tools, including polling stream tools such as `stream_next`.

### ralph wave

Dispatch wave events for parallel hat execution.

```bash
ralph wave <COMMAND>
```

**Subcommands:**

- `emit <TOPIC> [OPTIONS]` — Emit multiple events as a wave.
- `verify <TOPIC> [OPTIONS]` — Zero-disk precheck; validate payloads without writing JSONL.

#### ralph wave emit

```bash
ralph wave emit [OPTIONS] <TOPIC>
```

**Options:**

| Option | Description |
|--------|-------------|
| `<TOPIC>` | Event topic for all wave events (e.g., `review.file`). |
| `--payloads <ITEM>...` | One or more payloads, one per parallel worker. |
| `--payloads-stdin` | Read payloads from stdin, one per line. |
| `--output <text|json>` | Output format (default: `text`). |
| `--idempotency-key <KEY>` | Optional idempotency key; re-emits return the original wave ID. |
| `--policy-check` | Validate payloads against the active event policy before writing. |
| `--unsafe-no-policy-check` | Bypass mandatory policy check (only when config permits). |

#### ralph wave verify

```bash
ralph wave verify [OPTIONS] <TOPIC>
```

**Options:**

| Option | Description |
|--------|-------------|
| `<TOPIC>` | Event topic that would be emitted. |
| `--payloads <ITEM>...` | Payloads to validate, one per parallel worker. |
| `--payloads-stdin` | Read payloads from stdin, one per line. |
| `--output <text|json>` | Output format (default: `text`). |

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
| `add <CONTENT>` | Store a new memory. |
| `list` | List all memories. |
| `show <ID>` | Show a single memory. |
| `delete <ID>` | Delete a memory. |
| `search [QUERY]` | Find memories by query. |
| `prime` | Output memories for context injection. |
| `init` | Initialize memories file. |

Common `ralph tools memory add` options: `-t, --type <TYPE>`, `--tags <TAGS>`, `--private`, `--format <table|json|markdown|quiet>`.

#### ralph tools task

```bash
ralph tools task <SUBCOMMAND>
```

**Subcommands:**

| Command | Description |
|---------|-------------|
| `add <TITLE>` | Create a task. |
| `ensure <TITLE>` | Create or reuse a task by stable key. |
| `list` | List all tasks. |
| `ready` | Show unblocked tasks. |
| `start <ID>` | Mark a task as in progress. |
| `close <ID>` | Mark a task as complete. |
| `fail <ID>` | Mark a task as failed. |
| `reopen <ID>` | Reopen a closed or failed task. |
| `verify <COMMAND>` | OPAC Precheck: verify a task mutation would succeed without writing. |
| `verify-emit-bridge` | OPAC Precheck: verify the `task_id`/`task_key`/`step` emit-bridge. |
| `show <ID>` | Show a single task by ID. |

Common `ralph tools task add` options: `-p, --priority <1-5>`, `-d, --description <TEXT>`, `--blocked-by <IDS>`.
`ralph tools task ensure` accepts `--key <KEY>` or `--for-fix-unit <PLAN:FIX_STEP:SLUG>` for stable-key deduplication.

#### ralph tools skill

```bash
ralph tools skill <SUBCOMMAND>
```

**Subcommands:**

| Command | Description |
|---------|-------------|
| `load <NAME>` | Load a skill by name and output its content. |
| `list` | List available skills. |

### ralph completions

Generate shell completions.

```bash
ralph completions <SHELL>
```

Supported shells: `bash`, `elvish`, `fish`, `powershell`, `zsh`.

### ralph diagnose

Build an offline diagnosis report from `.ralph/diagnostics/<session>/`.

```bash
ralph diagnose [OPTIONS]
```

**Options:**

| Option | Description |
|--------|-------------|
| `--session <SESSION>` | Session to read from: `latest`, absolute path, relative path, or timestamped session id (default: `latest`). |
| `--format <markdown|json>` | Output format (default: `markdown`). |
| `--output <PATH>` | Write report to path instead of stdout. |
| `--diagnostics-root <PATH>` | Path to the diagnostics root. |
| `--source <SOURCE>` | Filter report to a single `DiagnosisSource` (snake_case name). |
| `--from-ledger` | Force the ledger-aligned view. |
| `--legacy` | Force the legacy session-scoped view. |
| `--supervisor <json|human|off>` | Include supervisor-state section when `supervisor-db` feature is enabled (default: `off`). |

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Completion promise reached (`LOOP_COMPLETE`). |
| 1 | Failure or stop condition (failure/cancelled/throttled state). |
| 2 | Runtime limits reached (`max-iterations`, `max-runtime`, or `max-cost`). |
| 3 | Loop requested restart. |
| 130 | Interrupted by signal (Ctrl-C / SIGINT). |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `RALPH_CURRENT_HAT` | Current hat ID injected by the loop runner. |
| `RALPH_CURRENT_LOOP_ID` | Current loop ID injected by the loop runner. |
| `RALPH_EVENTS_FILE` | Per-worker events file path. |
| `RALPH_TRIGGERED_HAT` | Target hat triggered by the current event. |
| `RALPH_EVENT_SOURCE` | Override source identifier for CLI emits. |
| `RALPH_WAVE_WORKER` | Set to `1` inside wave workers (blocks nested waves). |
| `RALPH_WAVE_ID` | Wave correlation ID (set on wave workers). |
| `RALPH_WAVE_INDEX` | 0-based worker index within the wave. |
| `RALPH_DIAGNOSTICS` | Set to `1` to enable diagnostics. |
| `RALPH_CONFIG` | Default config file path. |
| `NO_COLOR` | Disable color output. |

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
