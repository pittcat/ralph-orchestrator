# Backends

Ralph supports multiple AI CLI backends. This guide covers setup and selection.

## Supported Backends

| Backend | CLI Tool | Notes |
|---------|----------|-------|
| Claude Code | `claude` | Recommended, primary support |
| Gemini CLI | `gemini` | Google |
| Codex | `codex` | OpenAI |
| OpenCode | `opencode` | Community |
| Pi | `pi` | Multi-provider |
| Trae CLI | `trae-cli` | Trae |

## Auto-Detection

Ralph automatically detects installed backends:

```bash
ralph init
# Auto-detects available backend
```

Detection order (first available wins):
1. Claude
2. Gemini
3. Codex
4. OpenCode
5. Pi
6. Trae CLI

## Explicit Selection

Override auto-detection:

```bash
# Via CLI
ralph init --backend gemini
ralph run --backend claude

# Via config
# ralph.yml
cli:
  backend: "claude"
```

## Backend Setup

Each backend below includes:
- **Install** instructions
- **Auth & env vars** (API keys or login)
- **Hat YAML** configuration
- **`ralph doctor`** validation notes

Backend names (used in YAML and CLI flags): `claude`, `gemini`, `codex`, `opencode`, `pi`, `traecli`, `custom`.

### Claude Code (`claude`)

The recommended backend with full feature support.

```bash
# Install
npm install -g @anthropic-ai/claude-code

# Authenticate
claude login

# Verify
claude --version
```

**Auth & env vars:**
- `claude login` (preferred)
- `ANTHROPIC_API_KEY` (used by `ralph doctor` auth hints)

**Hat YAML:**
```yaml
hats:
  planner:
    backend: "claude"
```

**Doctor checks:**
- `claude --version` must succeed
- Warns if `ANTHROPIC_API_KEY` is missing

**Features:**
- Full streaming support
- All hat features
- Memory integration

### Gemini CLI (`gemini`)

Google's AI CLI.

```bash
# Install
npm install -g @google/gemini-cli

# Configure API key
export GEMINI_API_KEY=your-key

# Verify
gemini --version
```

**Auth & env vars:**
- `GEMINI_API_KEY` (used by `ralph doctor` auth hints)

**Hat YAML:**
```yaml
hats:
  analyst:
    backend: "gemini"
```

**Doctor checks:**
- `gemini --version` must succeed
- Warns if `GEMINI_API_KEY` is missing

### Codex (`codex`)

OpenAI's code-focused model.

```bash
# Install
# Visit https://github.com/openai/codex

# Configure
export OPENAI_API_KEY=your-key

# Verify
codex --version
```

**Auth & env vars:**
- `OPENAI_API_KEY` or `CODEX_API_KEY` (either satisfies `ralph doctor` auth hints)

**Hat YAML:**
```yaml
hats:
  coder:
    backend: "codex"
```

**Doctor checks:**
- `codex --version` must succeed
- Warns if neither `OPENAI_API_KEY` nor `CODEX_API_KEY` is set

### OpenCode (`opencode`)

Community AI CLI.

```bash
# Install
curl -fsSL https://opencode.ai/install | bash

# Verify
opencode --version
```

**Auth & env vars:**
- Set one of: `OPENCODE_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`
- OpenCode can proxy multiple providers; use the env var matching your provider

**Hat YAML:**
```yaml
hats:
  strategist:
    backend: "opencode"
```

**Doctor checks:**
- `opencode --version` must succeed
- Warns if none of `OPENCODE_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` are set

### Pi (`pi`)

Multi-provider AI coding assistant.

```bash
# Install
npm install -g @mariozechner/pi-coding-agent

# Verify
pi --version
```

**Auth & env vars:**
- Set one of: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, or any supported provider key
- Pi routes to the provider specified via `--provider` (default: google)
- Pass API key explicitly with `--api-key` or rely on provider-specific env vars

**Hat YAML:**
```yaml
hats:
  coder:
    backend: "pi"
```

**Pi provider selection (optional):**
```yaml
hats:
  coder:
    backend:
      type: "pi"
      args: ["--provider", "anthropic", "--model", "claude-sonnet-4"]
```

**Doctor checks:**
- `pi --version` must succeed
- Warns if no provider API key is set

### Trae CLI (`traecli`)

Trae CLI AI coding assistant.

```bash
# Install
# Visit https://trae.ai/ for installation

# Verify
trae-cli --version
```

**Auth & env vars:**
- Authenticate via Trae CLI per Trae docs
- `TRAE_API_KEY` (optional; used by `ralph doctor` auth hints)

**Hat YAML:**
```yaml
hats:
  reviewer:
    backend: "traecli"
```

**Doctor checks:**
- `trae-cli --version` must succeed

## Per-Hat Backend Override

Different hats can use different backends:

```yaml
hats:
  planner:
    backend: "claude"  # Use Claude for planning
    triggers: ["task.start"]
    instructions: "Create a plan..."

  coder:
    backend: "gemini"  # Use Gemini for coding
    triggers: ["plan.ready"]
    instructions: "Implement..."
```

## Custom Backends

For unsupported CLIs, use the custom backend:

```yaml
cli:
  backend: "custom"
  custom_command: "my-ai-cli"
  prompt_mode: "arg"  # or "stdin"
```

**Prompt modes:**

| Mode | How Prompt is Passed |
|------|---------------------|
| `arg` | `my-ai-cli -p "prompt"` |
| `stdin` | `echo "prompt" \| my-ai-cli` |

## Backend Comparison

| Feature | Claude | Gemini | Codex | Pi |
|---------|--------|--------|-------|----|
| Streaming | Yes | Yes | Yes | Yes |
| Tool use | Full | Partial | Partial | Full |
| Context size | Large | Large | Medium | Large |
| Speed | Fast | Fast | Medium | Fast |
| Cost | $$ | $ | $$ | $ |

## Troubleshooting

### Backend Not Found

```
ERROR: No AI agents detected
```

**Solution:**
1. Install a supported backend
2. Ensure it's in your PATH
3. Test directly: `claude -p "test"` or `pi -p "test"`

### Authentication Failed

```
ERROR: Authentication required
```

**Solution:**
```bash
# Claude
claude login

# Gemini - set API key
export GEMINI_API_KEY=your-key

# Pi - set provider API key
export ANTHROPIC_API_KEY=your-key
```

If the CLI is already authenticated but `ralph doctor` still warns, ensure the
expected env vars above are set (doctor checks are hints, not hard failures).

### Wrong Backend Used

```bash
# Force specific backend
ralph run --backend claude

# Or set in config
cli:
  backend: "claude"
```

### Backend Hanging

Some backends need interactive authentication on first run:

```bash
# Run backend directly first
claude -p "test"

# Then use with Ralph
ralph run
```

## Best Practices

1. **Pick one primary backend** — Consistency helps
2. **Test backend directly** — Before using with Ralph
3. **Use per-hat overrides sparingly** — Can complicate debugging
4. **Keep backends updated** — New features, bug fixes

## Next Steps

- Configure [Presets](presets.md) for your workflow
- Learn about [Cost Management](cost-management.md)
- Explore [Writing Prompts](prompts.md)
