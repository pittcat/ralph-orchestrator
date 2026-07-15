# Backend Selection Guide

Ralph is a Rust CLI (`cargo build -p ralph-cli` / `cargo install ralph-cli`). The entry point is `ralph [COMMAND]`. The **backend** is the AI CLI that Ralph invokes for each hat activation. This guide helps you pick and configure one.

For per-backend installation, authentication, and doctor checks, see [Backends](backends.md).

## Supported Backends

| Backend | CLI Binary | Rough Cost (per 1M tokens) | Best For |
|---------|------------|----------------------------|----------|
| **Claude Code** (`claude`) | `claude` | Input ~$3, Output ~$15 | Complex code, reasoning, documentation |
| **Gemini CLI** (`gemini`) | `gemini` | Input ~$0.50, Output ~$1.50 | Data analysis, math, multi-language |
| **Codex** (`codex`) | `codex` | Varies (OpenAI) | OpenAI code generation |
| **OpenCode** (`opencode`) | `opencode` | Provider-dependent | Multi-provider proxy |
| **Pi** (`pi`) | `pi` | Provider-dependent | Multi-provider coding agent |
| **Trae CLI** (`traecli`) | `trae-cli` | Varies (Trae) | Trae CLI workflows |
| **Custom** (`custom`) | Any | - | Unsupported or experimental CLIs |

Costs are rough guidance; check the backend provider for current pricing.

## Choosing a Backend

- **Complex logic, production code, or deep reasoning** → `claude`
- **Data analysis, math, or multi-language support** → `gemini`
- **OpenAI/Codex ecosystem** → `codex`
- **Need to switch providers easily** → `opencode` or `pi`
- **Trae CLI workflows** → `traecli`
- **Using a CLI not listed above** → `custom`

## Configuring the Backend

Set the default backend in `ralph.yml`:

```yaml
cli:
  backend: "claude"
```

Override on the CLI:

```bash
# Pick a backend during init
ralph init --backend claude

# Override for a single run
ralph run --backend gemini -P task.md
```

If `cli.backend` is omitted, Ralph auto-detects installed backends in this order: `claude`, `gemini`, `codex`, `opencode`, `pi`, `traecli`.

## Per-Hat Backend Override

Different hats can use different backends. Set `backend` on the hat:

```yaml
hats:
  planner:
    backend: "claude"
    triggers: ["task.start"]
    instructions: "Create a plan..."

  coder:
    backend: "gemini"
    triggers: ["plan.ready"]
    instructions: "Implement the plan..."
```

## Backend-Specific Arguments

Pass extra arguments to the backend CLI with `cli.args` (applied before the prompt):

```yaml
cli:
  backend: "claude"
  args: ["--model", "claude-sonnet-4"]
```

Per-hat arguments work the same way:

```yaml
hats:
  planner:
    backend: "pi"
    args: ["--provider", "anthropic"]
```

For a custom backend, `cli.command` is required and `cli.args` is optional:

```yaml
cli:
  backend: "custom"
  command: "my-ai-cli"
  args: ["--temperature", "0.7"]
  prompt_mode: "arg"  # or "stdin"
```

See [Backends](backends.md) for more on custom and per-hat configuration.

## Environment Variables

Backend authentication is handled by the backend CLI itself. Typical env vars include:

- `ANTHROPIC_API_KEY` — Claude
- `OPENAI_API_KEY` / `CODEX_API_KEY` — Codex
- `GEMINI_API_KEY` — Gemini
- `OPENCODE_API_KEY` — OpenCode

`ralph doctor` checks for the expected keys as hints, but the backend's own login flow (e.g., `claude login`) is usually sufficient.

## Troubleshooting

### Backend Not Found

```
ERROR: No AI backend detected
```

1. Install a supported backend CLI.
2. Ensure it is on your `PATH`.
3. Test it directly: `claude -p "hello"` or `pi -p "hello"`.

### Authentication Failed

Follow the backend's login flow and/or set the expected env var:

```bash
claude login
# or
export GEMINI_API_KEY=your-key
```

### Wrong Backend Used

```bash
# Force a backend for one run
ralph run --backend claude -P task.md

# Or pin it in ralph.yml
cli:
  backend: "claude"
```

### Backend Hangs

Some backends need interactive auth on first run. Run the backend directly once, then use Ralph:

```bash
claude -p "test"
ralph run -P task.md
```

If a backend legitimately runs silently for a long time, increase `adapters.<backend>.timeout` or `cli.autonomous_idle_timeout_secs` in `ralph.yml`.

## Next Steps

- [Backends](backends.md) — installation and auth details
- [Prompts](prompts.md) — write effective prompts
- [Configuration](configuration.md) — full config reference
