# User Guide

Practical guides for using Ralph Orchestrator effectively.

## In This Section

| Guide | Description |
|-------|-------------|
| [Configuration](configuration.md) | Full core config reference |
| [Presets](presets.md) | Built-in hat collections |
| [Preset Authoring](preset-authoring.md) | Creating and validating your own presets |
| [OPAC Agent Discipline](opac.md) | Observe → Precheck → Apply → Confirm workflow for isolated mode |
| [CLI Reference](cli-reference.md) | Command-line interface |
| [Backends](backends.md) | Supported AI backends |
| [Agents](agents.md) | Backend selection and configuration |
| [Writing Prompts](prompts.md) | Prompt engineering tips |
| [Cost Management](cost-management.md) | Controlling API costs |
| [Harness Extensions](harness-extensions.md) | Event filter / projection / state injection / preflight hooks |
| [Payload Contracts](payload-contracts.md) | Schema-based field enforcement between hats, schema metadata (`field_docs` / `examples` / `known_fields` / `trigger_context`), `--policy-check` 拒收后 5 个 agent-facing 字段如何读，`## TRIGGER CONTEXT` 区块如何解读 |
| [Execution Contracts](execution-contracts.md) | `work.done` completion gate (task closed, git evidence, tests) |
| [Precheck Gates](precheck-gates.md) | Opt-in LLM-as-judge gate before key topics (`event_loop.precheck`) |
| [Runtime Contracts](runtime-contracts.md) | Unified preset/workflow quality gates |
| [Runtime Diagnosis](runtime-diagnosis.md) | Recovery / drift journals, `ralph diagnose` offline report, telemetry config |
| [Iteration Boundary Hooks & Skills](iteration-boundary-hooks-and-skills.md) | Hooks, skills, and mutate extensions |
| [Managed Blocks](managed-blocks.md) | `CLAUDE.md` / `AGENTS.md` managed block sync |
| [Project Usage](project-usage.md) | Using Ralph in this repository |
| [Overview](overview.md) | High-level concepts and safety mechanisms |
| [Web Search](websearch.md) | Web search configuration |
| [Zsh Plugin](zsh-plugin.md) | Shell completion and zsh plugin setup |

## Quick Links

### Getting Started

- Initialize core config: `ralph init --backend claude`
- List built-in hat collections: `ralph init --list-presets`
- Run with hats: `ralph run -c ralph.yml -H builtin:ce-executor-pipeline`

### Running Ralph

- Basic run (core only): `ralph run -c ralph.yml`
- With hats: `ralph run -c ralph.yml -H builtin:debug`
- With inline prompt: `ralph run -c ralph.yml -H builtin:ce-executor-pipeline -p "Implement feature X"`
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
| Implementation work | `-H builtin:ce-executor-pipeline` |
| Bug investigation | `-H builtin:debug` |

## Common Tasks

### Start a New Feature

```bash
ralph init --backend claude
ralph run -c ralph.yml -H builtin:ce-executor-pipeline -p "Add OAuth login"
```

### Debug an Issue

```bash
ralph run -c ralph.yml -H builtin:debug -p "Investigate why user authentication fails on mobile"
```

### Run a Plan-Driven Workflow

```bash
ralph run -c ralph.yml -H builtin:ce-executor-pipeline -p "docs/plans/my-plan.md"
```

## Next Steps

Start with [Configuration](configuration.md) to understand all options.
