# Configuration

Complete reference for Ralph's YAML configuration.

## Configuration File

Ralph composes configuration from up to three layers:

1. `~/.ralph/config.yml` when present — user-level defaults loaded automatically
2. `ralph.yml` in the current workspace (or `$RALPH_CONFIG` / `-c <file>`) — project-level overrides
3. `-c core.field=value` overrides — applied last

Project config overlays on top of the user config via deep merge. Mappings are merged recursively and scalar values or arrays from the project config replace the user-level value.

```bash
# Use the workspace config (and automatically merge ~/.ralph/config.yml if present)
ralph run

# Override the project config path
RALPH_CONFIG=/path/to/config.yml ralph run ...
ralph run -c custom-config.yml
```

### User-level config (`~/.ralph/config.yml`)

Use `~/.ralph/config.yml` for defaults you want everywhere, such as shared backend settings, global lifecycle hooks, or organization-wide guardrails.

A common pattern is keeping notification hooks global while leaving project-specific automation in the repo-local `ralph.yml`:

```yaml
# ~/.ralph/config.yml
hooks:
  enabled: true
  events:
    post.loop.complete:
      - name: notify-success
        command: ["./scripts/notify.sh", "complete"]
        on_error: warn
    post.loop.error:
      - name: notify-failure
        command: ["./scripts/notify.sh", "error"]
        on_error: warn
```

```yaml
# ./ralph.yml
hooks:
  events:
    pre.loop.start:
      - name: env-guard
        command: ["./scripts/hooks/env-guard.sh"]
        on_error: block
```

With those two files, Ralph loads both and deep-merges them before validation and execution.

## MCP Workspace Resolution

`ralph mcp serve` resolves its workspace root in this order:

1. `--workspace-root <path>`
2. `RALPH_API_WORKSPACE_ROOT`
3. current working directory

Use one MCP server instance per workspace/repo. Ralph's current control-plane APIs are
workspace-scoped: `config.*`, `task.*`, `loop.*`, `planning.*`, and `collection.*` all
read or persist state under a single root.

## CLI Config Overrides

You can override specific core fields from the command line without creating a separate config file. This is useful for:

- Running parallel Ralph instances with isolated scratchpads
- Testing with different specs directories
- CI/CD pipelines with dynamic paths

**Syntax:** `-c core.field=value`

**Supported fields:**

| Field | Description |
|-------|-------------|
| `core.scratchpad` | Path to scratchpad file (string shorthand for `scratchpad.path`) |
| `core.specs_dir` | Path to specs directory |

**Examples:**

```bash
# Override scratchpad (loads ralph.yml + applies override)
ralph run -c core.scratchpad=.ralph/agent/feature-auth/scratchpad.md

# Explicit config + override
ralph run -c ralph.yml -c core.scratchpad=.ralph/agent/feature-auth/scratchpad.md

# Multiple overrides
ralph run -c core.scratchpad=.runs/task-1/scratchpad.md -c core.specs_dir=./custom-specs/
```

Overrides are applied after `ralph.yml` is loaded, so they take precedence. The scratchpad directory is auto-created if it doesn't exist.

## Combined Config Compatibility (`-c` + `-H`)

Ralph supports both styles:
- **Single-file combined config**: `-c ralph.yml` with core + hats in one file
- **Split config**: `-c <core>` plus `-H <hats source>`

If both are used (`-c` contains hats and `-H` is provided), `-H` wins for workflow sections:
- `hats` and `events` from `-H` replace `hats`/`events` from `-c`
- `event_loop` values from `-H` override matching `event_loop` keys from `-c`
- `-c core.*=...` overrides still apply last

## Full Configuration Reference

```yaml
# Event loop settings
event_loop:
  completion_promise: "LOOP_COMPLETE"  # Output that signals completion
  max_iterations: 100                   # Maximum orchestration loops
  max_runtime_seconds: 14400            # 4 hours max runtime
  starting_event: "task.start"          # First event published (hat mode)
  checkpoint_interval: 5                # Git checkpoint frequency
  prompt_file: "PROMPT.md"              # Default prompt file

# CLI backend settings
cli:
  backend: "claude"                     # Backend name
  prompt_mode: "arg"                    # arg or stdin
  idle_timeout_secs: 30                 # Interactive-mode idle timeout; 0 disables
  autonomous_idle_timeout_secs: null    # Autonomous/RPC/worktree watchdog; null inherits adapter timeout, 0 disables

# Core behaviors
core:
  scratchpad:                            # Scratchpad configuration
    enabled: true                        # Enable scratchpad (default: true)
    path: .ralph/agent/scratchpad.md     # Scratchpad file path
  specs_dir: "./specs/"                  # Specifications directory
  guardrails:                            # Rules injected into every prompt
    - "Fresh context each iteration"
    - "Never modify production database"

# Memories — persistent learning
memories:
  enabled: true                         # Enable memory system
  inject: auto                          # auto, manual, none
  budget: 2000                          # Max tokens to inject
  filter:
    types: []                           # Filter by memory type
    tags: []                            # Filter by memory tags
    recent: 0                           # Days limit (0 = no limit)

  # P3 visibility / owner semantics
  # New memories default to `shared` visibility. In agent context,
  # `ralph tools memory add --private` records a private memory stamped
  # with the current hat id (from RALPH_CURRENT_HAT). Private memories
  # are visible only to their owner; shared memories are visible to
  # every caller. Agents cannot delete or mutate shared memories — only
  # the human CLI may. See `ralph tools memory --help` for the full
  # authorization table.

# Tasks — runtime work tracking
tasks:
  enabled: true                         # Enable task system
  # Hats allowed to mutate any task in the loop, regardless of owner_hat_id.
  # When unset (default), only the owner hat may start/close/fail/reopen a
  # task. Used by the P2 cross-hat authorization guard.
  coordinator_hats: []                 # e.g. ["coordinator", "executor"]

# Runtime profile overlays (v1) — append markdown fragments to matching
# hat instructions at startup. CLI `--profile` flags are appended after
# this list; `--no-default-profiles` suppresses only this list.
profiles:
  default: repo:strict, user:my-style  # String form (comma-separated)
  # default: [repo:strict, user:my-style]  # YAML sequence form (equivalent)

# Optional features
features:
  parallel: true                        # Allow worktree loops when primary lock is held
  auto_merge: false                     # Auto-merge worktree loops on completion
  preflight:
    enabled: false                      # Run preflight automatically on `ralph run`
    strict: false                       # Treat warnings as failures
    skip: []                            # Skip checks by name (for example: ["hooks"])

# Lifecycle hooks (v1)
hooks:
  enabled: false
  defaults:
    timeout_seconds: 30
    max_output_bytes: 8192
    suspend_mode: wait_for_resume
  events:
    pre.loop.start:
      - name: env-guard
        command: ["./scripts/hooks/env-guard.sh"]
        on_error: block
        mutate:
          enabled: false

# Hats — specialized personas
hats:
  my_hat:
    name: "My Hat"                      # Display name
    description: "Purpose"              # Optional description
    triggers: ["event.*"]               # Subscription patterns
    publishes: ["event.done"]           # Allowed event types
    default_publishes: "event.done"     # Default when no explicit
    max_activations: 10                 # Activation limit
    backend: "claude"                   # Backend override
    scratchpad:                         # Per-hat scratchpad override
      enabled: true                     #   Enable scratchpad (default: true)
      path: .ralph/agent/my-hat.md      #   Scratchpad file path. Inherits from core if omitted.
    instructions: |
      Hat-specific instructions...
```

## Section Details

### event_loop

Controls the orchestration loop behavior.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `completion_promise` | string | `"LOOP_COMPLETE"` | Output text that ends the loop |
| `max_iterations` | integer | `100` | Maximum iterations before stopping |
| `max_runtime_seconds` | integer | `14400` | Maximum runtime (4 hours) |
| `starting_event` | string | `null` | First event (enables hat mode) |
| `checkpoint_interval` | integer | `5` | Git checkpoint frequency |
| `prompt_file` | string | `"PROMPT.md"` | Default prompt file |
| `execution_mode` | string | `"coordinator"` | Hat execution mode: `coordinator` or `isolated`. See [Multi-Hat Isolation Policy](#multi-hat-isolation-policy-mandatory) below for the fixed threshold that overrides the default. |
| `workflow_guards` | object | `null` | Ordered event chain enforcement (see below) |
| `event_policy` | object | `null` | Typed event payload validation and lifecycle enforcement (see below) |

| CLI Option | Type | Default | Description |
|------------|------|---------|-------------|
| `idle_timeout_secs` | integer | `30` | Interactive-mode idle timeout; `0` disables it |
| `autonomous_idle_timeout_secs` | integer/null | `null` | Autonomous/RPC/worktree backend inactivity watchdog; `null` inherits `adapters.<backend>.timeout`, `0` disables it |

#### Multi-Hat Isolation Policy (mandatory)

`event_loop.execution_mode` interacts with the number of configured `hats` through a **fixed threshold** with **no exemption path**:

| Hat count | Allowed `execution_mode` values | Behavior |
|-----------|---------------------------------|----------|
| `0`–`3` | `coordinator` (default) or `isolated` | Both modes start. The default `coordinator` runs all hats in a single prompt per iteration. |
| `4`+ | **`isolated` only** | `coordinator` is rejected at startup with the fixed error below. The preset must declare `event_loop.execution_mode: isolated` explicitly. |

The `3`-hat limit is a hard cap. Adding a 4th hat without flipping `execution_mode` to `isolated` is the most common cause of multi-hat preset failures. The lint rule `preset_lint::check_multi_hat_isolation` enforces this at `ralph preset check`, `ralph preflight`, and `ralph run` startup. Failure is fatal at every gate — the loop never enters a half-started state.

**Fixed error message** (literal — tools and Skills may grep for it):

```
preset declares N hats which exceeds the coordinator limit of 3;
set `event_loop.execution_mode: isolated` to run this preset
```

**Fix**:

```yaml
event_loop:
  execution_mode: isolated
  # ... rest of event_loop config ...
hats:
  # ≥ 4 hats declared here
```

**No exemptions.** The following are NOT supported and will not be honored if added:

- Environment variables such as `RALPH_ALLOW_COORDINATOR_OVERRIDE=1`.
- Test-only feature flags or `[dev].allow_large_coordinator` config keys.
- Preset-name based exemptions (e.g., allowing `ce-executor*` to exceed 3 hats in coordinator mode).
- Setting `execution_mode: coordinator` and adding a 4th hat anyway.

If your workflow needs more than 3 hats, the only correct option is `execution_mode: isolated`. In that mode, additional invariants apply (see [Harness Extensions](harness-extensions.md#isolated-mode-with-extensions) for the isolated runtime contract: terminal-topic authority and round-robin fair scheduling).

### event_policy

Event policy provides typed payload validation and lifecycle enforcement for events entering the event bus. It complements `workflow_guards` by checking *what* an event contains, not just *when* it arrives.

**Use event policy when:**
- Events must carry JSON payloads with specific required fields
- Field values must be restricted to an allowed set (e.g. `status` must be `keep`, `discard`, or `blocked`)
- Business events must not appear after a terminal topic like `LOOP_COMPLETE`
- You want to observe policy violations before enforcing them

**Configuration structure:**

```yaml
event_loop:
  event_policy:
    enabled: true
    mode: observe              # observe | enforce
    on_violation: warn         # warn | reject_with_resume | hold | block
    require_policy_check_for_cli_emit: false
    allow_unsafe_cli_emit: true
    require_emit_provenance: false
    completion_after_terminal:
      duplicate_terminal: warn
      business_after_completion: warn
      write_diagnostic_event: false
    schemas:
      experiment.planned:
        payload: json_object
        required_fields:
          - task_key
        field_types:
          task_key: string
      experiment.evaluated:
        payload: json_object
        required_fields:
          - task_key
          - evaluation.decision
        allowed_values:
          evaluation.decision: [keep, discard, blocked]
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
      - experiment.ready
      - experiment.measured
      - experiment.scored
      - experiment.evaluated
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Whether event policy is active |
| `mode` | string | `observe` | `observe` — log violations only; `enforce` — act on violations |
| `on_violation` | string | `warn` | Action when `mode: enforce` and a violation is found |
| `require_policy_check_for_cli_emit` | boolean | `false` | When true, `ralph event emit` always runs policy checks |
| `allow_unsafe_cli_emit` | boolean | `true` | When false, unsafe bypasses of CLI policy checks are disallowed |
| `require_emit_provenance` | boolean | `false` | When true, CLI emit must include `hat` / `triggered` provenance |
| `completion_after_terminal` | object | see below | Behavior after a terminal event has been observed |
| `schemas` | map | `{}` | Per-topic schema definitions (see below) |
| `terminal_topics` | list | `[]` | Topics that mark the end of a business session |
| `business_topics` | list | `[]` | Topics that should not appear after a terminal topic |

**`completion_after_terminal` fields:**

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `duplicate_terminal` | string | `warn` | Action when a duplicate terminal event arrives: `warn`, `reject`, `ignore` |
| `business_after_completion` | string | `warn` | Action when a business event arrives after completion: `warn`, `reject`, `ignore` |
| `write_diagnostic_event` | boolean | `false` | Write a diagnostic event when an event is blocked/ignored due to completion guards |

**Schema fields (`schemas.<topic>`):**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `payload` | string | `null` | Expected payload type: `json_object`, `string`, `number`, `bool`, `array` |
| `required_fields` | list | `[]` | Dot-notation paths that must exist in a JSON object payload |
| `allowed_values` | map | `{}` | Dot-notation path → list of allowed JSON values |

**Violation actions (`on_violation`):**

| Action | Behavior |
|--------|----------|
| `warn` | Log the violation; the event still enters the bus |
| `reject_with_resume` | Drop the event and publish `task.resume` with the violation reason |
| `hold` | Write a hold artifact and pause the loop (requires manual resume) |
| `block` | Silently drop the event without recovery signaling |

**Key behaviors:**

- Event policy is **opt-in** and defaults to `enabled: false`. Existing configs without `event_policy` behave exactly as before.
- `mode: observe` is the recommended starting point. It collects diagnostics without changing dispatch behavior.
- `mode: enforce` activates `on_violation`. Violations are processed in order; only the first finding drives the action.
- Schema validation runs only for topics declared in `schemas`. Unlisted topics are accepted without payload checks.
- `terminal_topics` and `business_topics` together enforce **terminal monotonicity**: once a terminal topic is observed, any subsequent business topic produces a violation.
- Policy validation happens **after** scope enforcement and **before** workflow guard validation and bus publication.
- `required_fields` uses dot notation for nested paths: `evaluation.decision` matches `{"evaluation": {"decision": "keep"}}`.
- `allowed_values` compares exact JSON values, including type. `"1"` and `1` are different values.

**Comparison with other enforcement mechanisms:**

| Mechanism | Purpose | Scope |
|-----------|---------|-------|
| `required_events` | Completion gate — has this topic appeared? | Global topic list |
| `enforce_hat_scope` | Publisher gate — can this hat emit this topic? | Per-hat topic allowlist |
| `workflow_guards` | Runtime order — can this topic appear now? | Per-chain ordered sequence |
| `event_policy` | Payload & lifecycle — does this event satisfy schema and state rules? | Per-topic schema + global terminal/business rules |

### workflow_guards

Workflow guards enforce ordered event chains for sequential multi-hat workflows. Use them when a workflow has mandatory step sequences and bypass events could corrupt state.

**When to use workflow guards:**
- Sequential workflows where each phase must complete before the next begins
- AutoResearch-style pipelines where `experiment.scored` must precede `experiment.evaluated`
- Any hat chain where out-of-order events could leave the workflow in an inconsistent state

**Configuration structure:**

```yaml
event_loop:
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
        correlation:
          from_payload: experiment_id
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `chains` | list | `[]` | Named ordered topic chains |
| `chains[].name` | string | — | Chain identifier |
| `chains[].topics` | list | — | Ordered event topics (first to last) |
| `chains[].mode` | string | `strict` | Enforcement mode: `strict` or `advisory` |
| `chains[].correlation` | object | `null` | Per-instance tracking configuration |
| `chains[].correlation.from_payload` | string | — | JSON payload field to extract the instance key |
| `chains[].correlation.from_topic` | string | first topic | Optional: topic whose payload contains the correlation key |

**Key behaviors:**

- When `workflow_guards` is configured, events must follow the declared topic sequence before being published to the event bus
- Out-of-order events are rejected and replaced with a `task.resume` recovery event
- Side-channel events (topics not in any chain) are accepted but do not advance chain progress
- `mode: strict` enforces ordered advancement; `mode: advisory` records topics but does not reject
- `correlation.from_payload` extracts an instance key from JSON payload for per-instance tracking
- Per-instance tracking allows parallel workflow instances to be guarded independently
- Completion (`LOOP_COMPLETE`) is rejected if any started guarded instance has not reached terminal phase
- Existing configs without `workflow_guards` behave exactly as before

**Comparison with other enforcement mechanisms:**

| Mechanism | Purpose | Scope |
|-----------|---------|-------|
| `required_events` | Completion gate: ensures topics have appeared before loop can end | Global topic list |
| `enforce_hat_scope` | Publisher gate: restricts which hats can emit which topics | Per-hat topic allowlist |
| `workflow_guards` | Runtime order: rejects out-of-order events before bus publication | Per-chain ordered sequence |

### cli

Backend configuration.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `backend` | string | auto-detect | Backend name |
| `prompt_mode` | string | `"arg"` | How prompt is passed |

**Backend values:**
- `claude` — Claude Code
- `kiro` — Kiro
- `gemini` — Gemini CLI
- `codex` — Codex
- `amp` — Amp
- `copilot` — Copilot CLI
- `opencode` — OpenCode
- `pi` — Pi
- `custom` — Custom adapter/backend

**Prompt mode values:**
- `arg` — Pass as CLI argument: `cli -p "prompt"`
- `stdin` — Pass via stdin: `echo "prompt" | cli`

### core

Core behaviors, scratchpad, and guardrails.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `scratchpad` | string or object | `{ enabled: true, path: ".ralph/agent/scratchpad.md" }` | Scratchpad configuration (see below) |
| `scratchpad.enabled` | boolean | `true` | Enable the scratchpad |
| `scratchpad.path` | string | `".ralph/agent/scratchpad.md"` | Scratchpad file path |
| `specs_dir` | string | `"./specs/"` | Specifications directory |
| `guardrails` | list | `[]` | Rules injected into every prompt |

The `scratchpad` field accepts a plain string (shorthand for setting `path` with `enabled: true`) or a structured object with `enabled` and `path`:

```yaml
# String shorthand — sets path, enabled defaults to true
core:
  scratchpad: ".workspace/plan.md"

# Structured object — full control
core:
  scratchpad:
    enabled: true
    path: .ralph/agent/scratchpad.md
```

> **Solo mode safety:** If scratchpad is disabled (`enabled: false`) but no hats are defined, Ralph force-enables it with a warning. Scratchpad is the only continuity mechanism in solo mode.

### core.event_projection

Auto-copy matching events to sidecar JSONL files for downstream consumption.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable event projection |
| `rules` | list | `[]` | Projection rules (see below) |

**ProjectionRule fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Human-readable rule name |
| `trigger_events` | list | `[]` | Event topics that trigger this rule |
| `fields` | list | `[]` | Fields to extract (`topic`, `payload`, `wave_id`, or JSON key) |
| `target_file` | string | required | Output file path (relative to workspace root) |
| `mode` | string | `"append"` | Projection mode (`append` only today) |

### core.state_files

Inject external structured files (JSON/JSONL) into prompts as typed XML blocks.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable state file injection |
| `inject_preamble` | string | `null` | Optional text injected before state file contents |
| `files` | list | `[]` | State file entries (see below) |

**StateFileEntry fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | string | required | File path (relative to workspace root) |
| `format` | string | `"json"` | File format (`json` or `jsonl`) |
| `char_budget` | integer | `null` | Max chars to keep (tail truncation) |
| `tail_lines` | integer | `null` | Max trailing lines to keep |

### core.preflight_extensions

Run external commands as part of `ralph preflight`.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable preflight extension hooks |
| `hooks` | list | `[]` | Hook definitions (see below) |

**PreflightHook fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Human-readable hook name (must be unique) |
| `command` | string | required | Shell command to execute |
| `stage` | string | `"after_native"` | When to run (`before_native` or `after_native`) |
| `fail_on_error` | boolean | `false` | Whether non-zero exit fails preflight |

**Template variables** in `command`:
- `{{config_path}}` — Absolute path to the loaded config file
- `{{config_dir}}` — Directory containing the config file
- `{{project_root}}` — Workspace root directory

### memories

Persistent learning across sessions.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `true` | Enable memory system |
| `inject` | string | `"auto"` | Injection mode |
| `budget` | integer | `2000` | Max tokens to inject |
| `filter.types` | list | `[]` | Filter by memory type |
| `filter.tags` | list | `[]` | Filter by tags |
| `filter.recent` | integer | `0` | Days limit |

**Injection modes:**
- `auto` — Automatically inject at iteration start
- `manual` — Agent must call `ralph tools memory prime`
- `none` — No injection

### tasks

Runtime work tracking.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `true` | Enable task system |

### features

Optional runtime capabilities.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `parallel` | boolean | `true` | Spawn worktree loops when another loop holds the primary lock |
| `auto_merge` | boolean | `false` | Auto-merge completed worktree loops |
| `preflight.enabled` | boolean | `false` | Run `ralph preflight` checks automatically before `ralph run` |
| `preflight.strict` | boolean | `false` | Treat preflight warnings as failures |
| `preflight.skip` | list | `[]` | Skip checks by name (for example `hooks`, `git`) |

When `features.preflight.enabled: true`, `ralph run` uses the default preflight suite:
`config`, `hooks`, `backend`, `git`, `paths`, `tools`, and `specs`.

### hooks

Lifecycle hooks for orchestrator phase-events (v1).

Hooks can be defined in either the user-level `~/.ralph/config.yml` or the workspace `ralph.yml`. Ralph loads the user config first, then overlays the project config on top. That means hooks in the user config apply globally unless the project config replaces the same event mapping.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable hook dispatch for lifecycle events |
| `defaults.timeout_seconds` | integer | `30` | Default per-hook timeout in seconds |
| `defaults.max_output_bytes` | integer | `8192` | Default stdout/stderr cap per stream |
| `defaults.suspend_mode` | enum | `wait_for_resume` | Default suspend mode for `on_error: suspend` |
| `events` | map | `{}` | Mapping from lifecycle phase-event key to list of hook specs |

Supported v1 lifecycle phase-event keys under `hooks.events`:

- `pre.loop.start`, `post.loop.start`
- `pre.iteration.start`, `post.iteration.start`
- `pre.plan.created`, `post.plan.created`
- `pre.loop.complete`, `post.loop.complete`
- `pre.loop.error`, `post.loop.error`

> **Note:** The `pre.human.interact` / `post.human.interact` keys are not
> listed because the human-in-the-loop channel was retired in the
> 2026-06-25 refactor; using those keys in a `hooks.events` block will be
> rejected by the hook config validator. The `human.guidance` /
> `task.resume` runtime-diagnosis recovery events have no hook surface
> by design.

Hook spec (`HookSpec`) fields:

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Stable identifier used in telemetry/diagnostics |
| `command` | Yes | Command argv array (`command[0]` must resolve to an executable) |
| `cwd` | No | Working directory override (absolute or workspace-relative) |
| `env` | No | Environment variable overrides for the hook process |
| `timeout_seconds` | No | Per-hook timeout override (must be > 0) |
| `max_output_bytes` | No | Per-hook output cap override per stream (must be > 0) |
| `on_error` | Yes | Failure disposition: `warn`, `block`, or `suspend` |
| `suspend_mode` | No | Suspend strategy override (`wait_for_resume`, `retry_backoff`, `wait_then_retry`) |
| `mutate.enabled` | No | Opt-in hook stdout mutation parsing (default `false`) |
| `mutate.format` | No | Optional format guardrail; only `json` is allowed in v1 |

Mutation scope in v1 is intentionally narrow:

- Mutation parsing only happens when `mutate.enabled: true`.
- Hook stdout must be JSON using the v1 contract: `{"metadata": { ... }}`.
- Only metadata namespace updates are allowed (`metadata.accumulated.hook_metadata.<hook_name>`).
- Prompt/event/config mutation is out of scope for v1.

Minimal runnable example:

- Config: [`examples/hooks/minimal/ralph.hooks.yml`](https://github.com/mikeyobrien/ralph-orchestrator/blob/main/examples/hooks/minimal/ralph.hooks.yml)
- Scripts: [`examples/hooks/scripts/env-guard.sh`](https://github.com/mikeyobrien/ralph-orchestrator/blob/main/examples/hooks/scripts/env-guard.sh), [`examples/hooks/scripts/notify.sh`](https://github.com/mikeyobrien/ralph-orchestrator/blob/main/examples/hooks/scripts/notify.sh)
- Validate: `ralph hooks validate -c examples/hooks/minimal/ralph.hooks.yml`

### profiles

运行时 profile overlay 的默认激活列表。Profile 是一种把同 preset 在不同场景下切成不同「风格」的轻量机制——把差异片段按 markdown 文件组织在 `ralph-profiles/<name>/<preset>/<hat>.md`(repo 级)或 `~/.config/ralph/profiles/<name>/<preset>/<hat>.md`(user 级),运行时按激活顺序追加到对应 hat 的 `instructions` 末尾。

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `default` | string or string list | `[]` | 默认激活的 profile spec 列表;每个 spec 形如 `<scope>:<name>`(`scope` ∈ `repo` / `user`)。可以是逗号分隔字符串,也可以是 YAML 字符串序列——两者等价。CLI `--profile` 标志按出现顺序追加到本列表之后;`--no-default-profiles` 仅关闭本列表。 |

**示例**(两种形式等价):

```yaml
# 逗号分隔字符串(便于在单行表达)
profiles:
  default: repo:strict, user:my-style
```

```yaml
# YAML 序列(便于多行注释与排序)
profiles:
  default:
    - repo:strict       # 团队共享的严格验证风格
    - user:my-style     # 个人偏好片段
```

**作用域与路径解析**:

- `repo:<name>` → `<project-root>/ralph-profiles/<name>/`
- `user:<name>` → `$XDG_CONFIG_HOME/ralph/profiles/<name>/`(优先),否则 `~/.config/ralph/profiles/<name>/`
- 在 `--worktree` 子进程中,repo profile 的 `<project-root>` 由 `RALPH_WORKSPACE_ROOT` 或 `config.core.workspace_root` 解析为主仓库根目录,避免被解析到 worktree 路径下。

**片段加载语义**(与 `ralph run --help` 同源):

- 仅加载 `<dir>/<preset>/<hat>.md` 形式的 `.md` 文件;同一 profile 内按文件名升序加载;多个 profile 按激活顺序拼接。
- 缺失当前 preset 子目录时打 warning 并跳过;片段对应 hat 不在当前 preset 中时打 warning 并忽略。
- profile 目录不存在(`--profile` 显式指定或 `profiles.default` 中)时,**立即报错**并给出完整路径——可用 `--no-default-profiles` 绕过 `profiles.default`。
- profile name 必须非空、不含路径分隔符或 `..`,校验失败立即报错。
- 仅修改 `HatConfig.instructions`,不触碰 topology、backend、event_loop 等结构字段(R15 不变性)。

**应用时机**:

profile 在 `load_config_for_preflight` 返回后、`config.validate()` 与 `run_auto_preflight` 之前生效,因此 preflight、validate、event loop 看到的 instructions 完全一致。

**预览**:

- 不修改配置的只读预览见 `ralph inspect profiles [--profile ...] [--format human|json]`。
- 详细目录结构与典型用法见 [Profiles 概念说明](../concepts/profiles.md)。

**v1 范围**:

- 仅追加 hat instructions;不支持覆盖 backend / event_loop / topology。
- 仅精确匹配 profile / preset / hat-id;不支持通配符或正则。
- 不支持继承或嵌套;不支持 `-c profiles.default=...` 形式的 CLI 覆盖。
- 不提供 `ralph profile create/init` 等脚手架命令。

### hats

Specialized personas for hat-based mode.

| Option | Type | Required | Description |
|--------|------|----------|-------------|
| `name` | string | Yes | Display name |
| `description` | string | No | Purpose description |
| `triggers` | list | Yes | Event subscription patterns |
| `publishes` | list | Yes | Allowed event types |
| `default_publishes` | string | No | Default event if none explicit |
| `max_activations` | integer | No | Limit activations |
| `backend` | string | No | Backend override |
| `scratchpad` | string or object | No | Per-hat scratchpad override (inherits `core.scratchpad` if omitted) |
| `event_filter` | object | No | Per-hat event allowlist filter (see below) |
| `instructions` | string | Yes | Hat-specific prompt |

**Hat event_filter fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable filtering for this hat |
| `mode` | string | `"allowlist"` | Filter mode (`allowlist` only today) |
| `events` | list | `[]` | Event topics this hat is allowed to see in its prompt |

Each hat can override the global scratchpad with its own `scratchpad` field. Like the core-level setting, it accepts a plain string or a structured object:

```yaml
hats:
  planner:
    scratchpad: .ralph/agent/planner.md       # String shorthand
    # ...
  builder:
    scratchpad:
      path: .ralph/agent/builder.md           # Structured with custom path
    # ...
  validator:
    scratchpad:
      enabled: false                          # Disable scratchpad entirely
    # ...
  reviewer:                                   # No scratchpad key = inherits global
    # ...
```

**Resolution order:** hat override → `core.scratchpad` → defaults.

## Example Configurations

### Traditional Mode (Minimal)

```yaml
cli:
  backend: "claude"

event_loop:
  completion_promise: "LOOP_COMPLETE"
  max_iterations: 100
```

### Hat-Based Mode

```yaml
cli:
  backend: "claude"

event_loop:
  completion_promise: "LOOP_COMPLETE"
  max_iterations: 100
  starting_event: "task.start"

hats:
  planner:
    name: "Planner"
    triggers: ["task.start"]
    publishes: ["plan.ready"]
    instructions: |
      Create an implementation plan.

  builder:
    name: "Builder"
    triggers: ["plan.ready"]
    publishes: ["build.done"]
    instructions: |
      Implement the plan.
      Evidence required: tests pass.
```

### With Memories Disabled

```yaml
cli:
  backend: "claude"

event_loop:
  completion_promise: "LOOP_COMPLETE"

memories:
  enabled: false

tasks:
  enabled: false
```

### With Per-Hat Scratchpads

```yaml
cli:
  backend: "claude"

event_loop:
  completion_promise: "LOOP_COMPLETE"
  starting_event: "task.start"

core:
  scratchpad:
    enabled: true
    path: .ralph/agent/scratchpad.md

hats:
  planner:
    name: "Planner"
    scratchpad:
      path: .ralph/agent/planner.md
    triggers: ["task.start"]
    publishes: ["plan.ready"]
    instructions: |
      Create an implementation plan.

  builder:
    name: "Builder"
    triggers: ["plan.ready"]
    publishes: ["build.done"]
    instructions: |
      Implement the plan.

  reviewer:
    name: "Reviewer"
    scratchpad:
      enabled: false
    triggers: ["build.done"]
    publishes: ["review.done"]
    instructions: |
      Review the implementation. No scratchpad needed.
```

### Strict Event Policy

Use this when you want Ralph to enforce typed payloads, prevent naked `ralph emit` bypass, and protect completion semantics.

```yaml
cli:
  backend: "claude"

event_loop:
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: false
    require_emit_provenance: true
    completion_after_terminal:
      duplicate_terminal: reject
      business_after_completion: ignore
      write_diagnostic_event: true
    schemas:
      experiment.planned:
        payload: json_object
        required_fields:
          - task_key
          - hypothesis
          - falsification_condition
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
```

With this configuration:

- `ralph emit` always validates against policy, even without `--policy-check`.
- `--unsafe-no-policy-check` is disallowed.
- Every CLI emit must include `--hat` or have `$RALPH_CURRENT_HAT` set.
- After `LOOP_COMPLETE` is accepted, duplicate terminal events are rejected and subsequent business events are ignored.
- Diagnostic events are written when the completion guard blocks an event.

### With Custom Guardrails

```yaml
cli:
  backend: "claude"

event_loop:
  completion_promise: "LOOP_COMPLETE"

core:
  guardrails:
    - "Always run tests before declaring done"
    - "Never modify production database"
    - "Follow existing code patterns"
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `RALPH_CONFIG` | Default config file path |
| `RALPH_DIAGNOSTICS` | Enable diagnostics (`1`) |
| `NO_COLOR` | Disable color output |

## Migration Guidance for Downstream Skills

If you maintain a Skill or wrapper that calls `ralph emit`, here is what changes with the new native capabilities.

### What You Should Stop Doing

- **Stop relying on `echo >> .ralph/events.jsonl`**. It bypasses policy, schema, and provenance. Use `ralph emit` exclusively.
- **Stop writing provenance only to sidecar guard logs**. Pass `--hat` or rely on env injection so the main event log carries attribution.
- **Stop assuming `LOOP_COMPLETE` is just another event**. After the first accepted completion, Ralph treats the loop as complete. Emitting more business events afterward is blocked or ignored depending on config.

### What You Should Start Doing

- **Pass `--hat <id>` on every `ralph emit`** (or rely on `$RALPH_CURRENT_HAT`).
- **Pass `--triggered <id>`** when you know the downstream hat.
- **Use `--policy-check`** during development, and rely on config-level `require_policy_check_for_cli_emit: true` in production.
- **Use `--unsafe-no-policy-check` only for emergency manual bypass**, and only when the config allows it.

### Environment Injection

When Ralph runs a Hat backend, it automatically sets:

| Variable | Value |
|----------|-------|
| `RALPH_CURRENT_HAT` | The hat currently executing |
| `RALPH_CURRENT_LOOP_ID` | The active loop ID |
| `RALPH_EVENTS_FILE` | Resolved path to the current events file |
| `RALPH_TRIGGERED_HAT` | The hat triggered by the current event (if known) |

This means a Hat running under Ralph can usually emit without any extra flags:

```bash
ralph emit experiment.planned --json '{"task_key":"x","hypothesis":"h","falsification_condition":"f"}'
```

The `hat` field is populated automatically from `$RALPH_CURRENT_HAT`.

### Completion Monotonicity

Ralph treats completion as a **loop-level monotonic state**:

1. The first accepted `LOOP_COMPLETE` (or configured `completion_promise`) sets the loop as complete.
2. The termination reason becomes `CompletionPromise` and stays that way.
3. Any duplicate terminal event (e.g., a second `LOOP_COMPLETE`) is rejected or ignored.
4. Any business event arriving after completion is rejected or ignored.
5. Events in the same batch after the completion topic are also guarded.

This protects downstream Skills from race conditions where a late event could corrupt state after the loop has already decided to finish.

### About `.ralph/loops.json`

`.ralph/loops.json` is a **registry of tracked loops** (metadata like IDs, branches, and status). It is **not** an event history. For the full event stream, read the events JSONL file referenced by `.ralph/current-events`.

## Next Steps

- Explore [Presets](presets.md) for pre-configured workflows
- Learn about [CLI Reference](cli-reference.md)
- Understand [Backends](backends.md)
