# User Guide

Practical guides for using Ralph Orchestrator effectively.

## In This Section

| Guide | Description |
|-------|-------------|
| [Configuration](configuration.md) | Full core config reference |
| [Presets](presets.md) | Built-in hat collections |
| [CLI Reference](cli-reference.md) | Command-line interface |
| [Backends](backends.md) | Supported AI backends |
| [Writing Prompts](prompts.md) | Prompt engineering tips |
| [Cost Management](cost-management.md) | Controlling API costs |
| [Harness Extensions](harness-extensions.md) | Event filter / projection / state injection / preflight hooks |
| [Payload Contracts](payload-contracts.md) | Schema-based field enforcement between hats |
| [Execution Contracts](execution-contracts.md) | `work.done` completion gate (task closed, git evidence, tests) |
| [Precheck Gates](precheck-gates.md) | Opt-in LLM-as-judge gate before key topics (`event_loop.precheck`) |
| [Runtime Diagnosis](runtime-diagnosis.md) | Recovery / drift journals, `ralph diagnose` offline report, telemetry config |

## Quick Links

### Getting Started

- Initialize core config: `ralph init --backend claude`
- List built-in hat collections: `ralph init --list-presets`
- Run with hats: `ralph run -c ralph.yml -H builtin:ce-executor-serial`

### Running Ralph

- Basic run (core only): `ralph run -c ralph.yml`
- With hats: `ralph run -c ralph.yml -H builtin:debug`
- With inline prompt: `ralph run -c ralph.yml -H builtin:ce-executor-serial -p "Implement feature X"`
- Headless mode: `ralph run --no-tui`
- Resume session: `ralph run --continue`

### Monitoring

- View event history: `ralph events`
- Check memories: `ralph tools memory list`
- Check tasks: `ralph tools task list`

## Choosing a Workflow

| Your Situation | Recommended Approach |
|----------------|---------------------|
| Simple task | Core only (no hats) |
| Implementation work | `-H builtin:ce-executor-serial` |
| Bug investigation | `-H builtin:debug` |

## Common Tasks

### Start a New Feature

```bash
ralph init --backend claude
ralph run -c ralph.yml -H builtin:ce-executor-serial -p "Add OAuth login"
```

### Debug an Issue

```bash
ralph run -c ralph.yml -H builtin:debug -p "Investigate why user authentication fails on mobile"
```

### Run a Plan-Driven Workflow

```bash
ralph run -c ralph.yml -H builtin:ce-executor-serial -p "docs/plans/my-plan.md"
```

## Next Steps

Start with [Configuration](configuration.md) to understand all options.
