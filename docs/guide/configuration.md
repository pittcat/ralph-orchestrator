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

### Agent-facing config discovery

Inside a running loop, every agent-facing tool (`ralph tools task`,
`ralph emit`, `ralph wave emit`, `ralph tools skill`) honours the
same project-config precedence as the runner:

1. `ConfigSource::File` paths passed via `-c` (first existing wins)
2. `$RALPH_CONFIG` (when set and non-empty)
3. `<workspace>/ralph.yml`
4. `<workspace>/ralph.yaml`

`ralph run` forwards the resolved absolute or workspace-relative
path as `RALPH_CONFIG` to every hat and wave worker, so agents
running inside the loop inherit the same project config without
re-passing `-c`. Custom project filenames no longer need a
`ralph.yml` symlink — the runner-supplied env var closes the gap.

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
  max_wave_total: 64                    # Maximum allowed wave fan-out
  max_cost_usd: null                    # Max cost before stopping
  max_consecutive_failures: 5           # Stop after N consecutive failures
  cooldown_delay_seconds: 0             # Delay before next iteration
  starting_event: null                  # First event published in hat mode
  checkpoint_interval: 5                # Git checkpoint frequency
  prompt_file: "PROMPT.md"              # Default prompt file
  execution_mode: isolated              # Presets must use isolated
  persistent: false                     # Keep loop alive after LOOP_COMPLETE
  required_events: []                   # Topics required before completion
  cancellation_promise: "loop.cancel"   # Early termination topic
  enforce_hat_scope: false              # Validate hat publish allowlists
  ephemeral_isolation: false            # Relocate stray scratchpad artefacts
  enforce_current_unit: false           # Per-step single-unit task contract
  max_residuals: 8                      # Residual threshold for verdict promotion
  task_resume_ttl_seconds: 300          # Freshness TTL for task.resume injection

# CLI backend settings
cli:
  backend: "claude"                     # Backend name
  command: null                         # Required for custom backend
  prompt_mode: "arg"                    # arg or stdin
  default_mode: "autonomous"            # autonomous or interactive
  idle_timeout_secs: 30                 # Interactive-mode idle timeout; 0 disables
  autonomous_idle_timeout_secs: null    # Autonomous/RPC/worktree watchdog; null inherits adapter timeout
  args: []                              # Extra args for custom backend
  prompt_flag: null                     # Custom prompt flag for custom backend

# Core behaviors
core:
  scratchpad:                            # Scratchpad configuration
    enabled: true
    path: .ralph/agent/scratchpad.md
  specs_dir: ".ralph/specs/"             # Specifications directory
  guardrails:                            # Rules injected into every prompt
    - "Fresh context each iteration - scratchpad is memory"
    - "Don't assume 'not implemented' - search first"
    - "Backpressure is law - tests/typecheck/lint/audit must pass"
  invariant_assertions: false            # Defense-in-depth impersonation checks
  workspace_root: "."                   # Resolved at load time

# Memories — persistent learning
memories:
  enabled: true
  inject: auto                          # auto, manual, none
  budget: 0                             # Max tokens to inject (0 = unlimited)
  filter:
    types: []
    tags: []
    recent: 0

# Tasks — runtime work tracking
tasks:
  enabled: true
  coordinator_hats: []                 # Hats allowed to mutate any task

# Runtime profile overlays
profiles:
  default: []                           # List of "repo:<name>" / "user:<name>" specs

# Optional features
features:
  parallel: true
  auto_merge: false
  preflight:
    enabled: false
    strict: false
    skip: []

# Lifecycle hooks (v1)
hooks:
  enabled: false
  defaults:
    timeout_seconds: 30
    max_output_bytes: 8192
    suspend_mode: wait_for_resume
  events: {}

# Agent doc sync
agent_doc_sync:
  enabled: true
  on_error: warn
  blocks:
    - "builtin:hang-prevention"
  startup_timeout_secs: 30

# Telemetry / runtime diagnosis
telemetry:
  runtime_diagnosis:
    enabled: false
    write_artifacts: false
    prompt_injection_enabled: false
    max_prompt_findings: 5
    max_prompt_chars: 2000
    retry_window_iterations: 5
    max_repeated_recoveries: 3
    artifact_retention: 10
    malformed_jsonl_policy: warn
    drift:
      window_size: 50
      field_completeness_threshold: 0.9
      coord_join_rate_threshold: 0.6
      coord_join_mode: parallel
      emit_cadence_sigma: 2.0

# Loop-completion webhook notifications (best-effort, default off)
notifications:
  enabled: false
  timeout_seconds: 5
  endpoints:
    - name: feishu-success
      url: "https://open.feishu.cn/open-apis/bot/v2/hook/********"
      on: [success]
      headers:
        Content-Type: application/json
      body: '{"msg_type":"text","content":{"text":"Ralph OK {{loop_id}} ({{termination_reason}})"}}'
    - name: feishu-failure
      url: "https://open.feishu.cn/open-apis/bot/v2/hook/********"
      on: [failure]
      body: '{"msg_type":"text","content":{"text":"Ralph FAIL {{loop_id}}: {{termination_reason}}"}}'

# Backend adapter defaults
adapters:
  claude:
    enabled: true
    timeout: 300
  gemini:
    enabled: true
    timeout: 300
  codex:
    enabled: true
    timeout: 300
  opencode:
    enabled: true
    timeout: 300
  pi:
    enabled: true
    timeout: 300
  traecli:
    enabled: true
    timeout: 300

# Skills injection
skills:
  enabled: true
  dirs: []
  overrides: {}

# Hats — specialized personas
hats:
  my_hat:
    name: "My Hat"
    description: "Purpose"              # Required
    triggers: ["event.*"]
    publishes: ["event.done"]
    terminal_events: ["event.done"]
    default_publishes: "event.done"
    max_activations: 10
    instructions: |
      Hat-specific instructions...

# Event metadata
events:
  event.done:
    description: "Event finished"
    on_trigger: "Handle the finished event"
    on_publish: "Emit when finished"

# Mechanism flow declaration (advanced)
mechanism:
  flow:
    type: declared
    version: 1
    repair_budget: 3
    enforce_schema: hard
    state_idempotency: required
    steps: []
```

## Section Details

### event_loop

Controls the orchestration loop behavior.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `prompt` | string | `null` | Inline prompt text (mutually exclusive with `prompt_file`) |
| `prompt_file` | string | `"PROMPT.md"` | Path to prompt file |
| `completion_promise` | string | `"LOOP_COMPLETE"` | Output text that ends the loop |
| `max_iterations` | integer | `100` | Maximum iterations before stopping |
| `max_runtime_seconds` | integer | `14400` | Maximum runtime (4 hours) |
| `max_wave_total` | integer | `64` | Maximum allowed `wave_total`; larger waves are rejected |
| `max_cost_usd` | float | `null` | Maximum cost in USD before stopping |
| `max_consecutive_failures` | integer | `5` | Stop after this many consecutive failures |
| `cooldown_delay_seconds` | integer | `0` | Delay before starting the next iteration |
| `starting_hat` | string | `null` | **Deprecated** — use `starting_event` |
| `starting_event` | string | `null` | First event published to delegate to hats |
| `mutation_score_warn_threshold` | float | `null` | Warn when mutation score drops below this percentage |
| `persistent` | boolean | `false` | When true, `LOOP_COMPLETE` does not terminate the loop |
| `required_events` | list | `[]` | Topics that must be seen before completion is accepted |
| `cancellation_promise` | string | `"loop.cancel"` | Topic that triggers graceful early termination; empty disables it |
| `enforce_hat_scope` | boolean | `false` | Validate hat emissions against `publishes` |
| `ephemeral_isolation` | boolean | `false` | Relocate stray scratchpad artefacts in isolated mode |
| `enforce_current_unit` | boolean | `false` | Enforce per-step single-unit task contract in isolated mode |
| `max_residuals` | integer | `8` | Residual finding threshold for `pass_with_residuals` promotion |
| `task_resume_ttl_seconds` | integer | `300` | Freshness TTL for injected `task.resume` events |
| `execution_mode` | string | `coordinator` (legacy) | Hat execution mode; presets must use `isolated` |
| `workflow_guards` | object | `null` | Ordered event chain enforcement |
| `event_policy` | object | `null` | Typed event payload validation |
| `state_machine` | object | `null` | Instance lifecycle state machine |
| `phase_config` | object | `null` | Two-phase (warmup/production) configuration |
| `verdict_gate` | object | `null` | Reject completion when a verdict event indicates failure |
| `execution_contracts` | object | `null` | Validate agent completion obligations |
| `precheck` | object | `null` | LLM-as-judge precheck gates |
| `workflow_contract` | object | `null` | Workflow Activation Contract runtime config |
| `state_projection` | object | disabled | Project canonical task/progress ledgers from events |
| `supervisor` | object | disabled | rusqlite-backed wave orchestrator |
| `progress_steward` | object | disabled | Auto-wake fallback hat on stalls |
| `macro_edge_next_hint` | object | disabled | Inject `## NEXT ACTION` hint from payload |
| `mechanism` | object | `null` | Mechanism flow declaration (also accepted at top level) |

#### Execution mode

`event_loop.execution_mode` interacts with the number of configured `hats` through a fixed threshold:

| Hat count | Allowed `execution_mode` values | Behavior |
|-----------|---------------------------------|----------|
| `0`–`3` | `isolated` or `coordinator` | `isolated` is recommended for all new presets; `coordinator` is deprecated and retained only for backwards compatibility. |
| `4`+ | **`isolated` only** | `coordinator` is rejected at startup. The preset must declare `event_loop.execution_mode: isolated` explicitly. |

The `3`-hat limit is a hard cap. New presets should always set `execution_mode: isolated`. The lint rule `preset_lint::check_multi_hat_isolation` enforces this at `ralph preset check`, `ralph preflight`, and `ralph run` startup.

**Fixed error message** (literal):

```
preset declares N hats which exceeds the coordinator limit of 3;
set `event_loop.execution_mode: isolated` to run this preset
```

**No exemptions.** Environment variables, test flags, preset-name wildcards, or `[dev]` overrides are not honored.

### event_policy

Event policy provides typed payload validation and lifecycle enforcement.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Whether event policy is active |
| `mode` | string | `observe` | `observe` or `enforce` |
| `on_violation` | string | `warn` | `warn`, `reject_with_resume`, `hold`, or `block` |
| `schemas` | map | `{}` | Per-topic schema definitions |
| `schema_file` | string | `null` | External schema file (inline schemas take priority) |
| `terminal_topics` | list | `[]` | Topics that mark the end of a business session |
| `business_topics` | list | `[]` | Topics that should not appear after a terminal topic |
| `require_policy_check_for_cli_emit` | boolean | `false` | Require policy checks on every `ralph emit` |
| `allow_unsafe_cli_emit` | boolean | `true` | Allow `--unsafe-no-policy-check` bypasses |
| `require_emit_provenance` | boolean | `false` | Require `--hat` / `$RALPH_CURRENT_HAT` on CLI emit |
| `completion_after_terminal` | object | see below | Behavior after a terminal event |
| `topic_deny_rules` | list | `[]` | Exact `(hat_id, topic)` deny pairs |
| `plan_name_equality_required` | boolean | `false` | Require `work.done` `plan_name` to match latest `work.ready` |

**Schema fields (`schemas.<topic>`):**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `payload` | string | `null` | `json_object`, `string`, `number`, `bool`, or `array` |
| `required_fields` | list | `[]` | Dot-notation paths required in the payload |
| `allowed_values` | map | `{}` | Dot-notation path → list of allowed JSON values |
| `hat_allowed_values` | map | `{}` | Dot-notation path → list of `{hat_id, values}` entries |
| `element_constraints` | map | `{}` | Array field name → element shape constraint |

**`element_constraints.<field>`:**

| Field | Type | Description |
|-------|------|-------------|
| `field` | string | Element field name to check |
| `required` | boolean | Whether the field must exist on every element |
| `allowed_values` | list | Allowed values for the element field |
| `required_when` | map | `{field: value}` making the field required when another field matches |
| `forbid_null_when_required` | boolean | Forbid `null` when the field is required |

**`completion_after_terminal` fields:**

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `duplicate_terminal` | string | `warn` | `warn`, `reject`, or `ignore` |
| `business_after_completion` | string | `warn` | `warn`, `reject`, or `ignore` |
| `write_diagnostic_event` | boolean | `false` | Write a diagnostic event when an event is blocked/ignored |

### workflow_guards

Enforce ordered event chains.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `chains` | list | `[]` | Named ordered topic chains |
| `chains[].name` | string | required | Chain identifier |
| `chains[].topics` | list | required | Ordered event topics |
| `chains[].mode` | string | `strict` | `strict` or `advisory` |
| `chains[].correlation.from_payload` | string | required | JSON payload field for instance key |
| `chains[].correlation.from_topic` | string | first topic | Topic whose payload contains the key |

### state_machine

Instance lifecycle validation.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable state machine validation |
| `instance_key.from_payload` | string | `""` | JSON field for instance key |
| `instance_key.required_for` | list | `[]` | Topics that must carry a valid instance key |
| `terminal_topics` | list | `[]` | Terminal/completion topics |
| `business_topics` | list | `[]` | Business progress topics |
| `terminal_guard.require_no_open_instances` | boolean | `true` | Reject terminal events while instances are open |
| `terminal_guard.duplicate_terminal` | string | `reject` | `reject` or `ignore` |
| `terminal_guard.business_after_terminal` | string | `reject` | `reject` or `ignore` |
| `transitions` | list | `[]` | State transitions |
| `transitions[].topic` | string | required | Trigger topic |
| `transitions[].from` | list | required | Source states (`"idle"` means no prior state) |
| `transitions[].to` | string | required | Target state |
| `transitions[].opens_instance` | boolean | `false` | Opens a new instance |
| `transitions[].closes_instance` | boolean | `false` | Closes the instance |

### execution_contracts

Validate agent completion obligations.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enforce contracts |
| `rules` | map | `{}` | Topic → `ExecutionContractRule` |
| `rules.<topic>.require_payload_fields` | list | `[]` | Required payload fields |
| `rules.<topic>.require_task.id_field` | string | `"task_id"` | Payload field containing task ID |
| `rules.<topic>.require_task.key_field` | string | `"task_key"` | Payload field containing task key |
| `rules.<topic>.require_task.loop_scoped` | boolean | `true` | Task must belong to current loop |
| `rules.<topic>.require_task.allowed_terminal_statuses` | list | `["closed"]` | Satisfying task statuses |
| `rules.<topic>.require_task.auto_close_on_valid` | boolean | `false` | Auto-close the task on valid contract |
| `rules.<topic>.require_git_change.mode` | string | `"diff_or_commit"` | Git evidence acceptance mode |
| `rules.<topic>.require_git_change.allow_empty_for_steps` | list | `[]` | Steps allowed to have empty diff/commit |
| `rules.<topic>.require_test_evidence.mode` | string | `"optional"` | `optional` or `required_payload_field` |
| `rules.<topic>.require_test_evidence.payload_field` | string | `null` | Field to check when mode is `required_payload_field` |
| `rules.<topic>.reject.diagnostic_topic` | string | `"event.execution_contract.rejected"` | Diagnostic topic on rejection |
| `rules.<topic>.reject.guidance_topic` | string | `"plan.blocked"` | Guidance topic on rejection |

### workflow_contract

Workflow Activation Contract runtime configuration.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `handoff_dispatch_timeout_seconds` | integer | `600` | Max seconds to wait for a unique-consumer hat activation (clamped to `1800`) |
| `handoff_topic_seeds` | list | `queue.advance`, `work.ready`, `fix.plan.ready`, `work.failed` | Topics the dispatcher monitors |
| `incomplete_wave_gate.enabled` | boolean | `false` | Emit `plan.blocked` for stalled review waves |
| `step_handoff.progress_task_gate` | boolean | `false` | Validate `progress.md` ↔ `tasks.jsonl` before handoff |

### precheck

LLM-as-judge precheck gates.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable precheck gates |
| `rules` | map | `{}` | Topic → `PrecheckRule` |
| `rules.<topic>.prompt` | list | `[]` | Checklist items for the gate hat |
| `rules.<topic>.on_fail.target` | string | `""` | Hat to receive `<topic>.rejected` |
| `rules.<topic>.on_fail.retry_budget` | integer | `3` | Allowed rejections before escalation |
| `rules.<topic>.on_fail.on_exhausted` | string | `""` | Terminal topic emitted on exhaustion |
| `rules.<topic>.on_fail.reason` | string | `""` | Reason recorded on rejected payloads |

### supervisor

rusqlite-backed wave orchestrator.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable the supervisor |
| `db_path` | string | `".ralph/supervisor.db"` | SQLite database path |
| `max_concurrent_workers` | integer | `4` | Active worker slot ceiling |
| `aggregate_timeout_secs` | integer | `600` | Wall-clock budget for one wave's collect phase |

### progress_steward

Auto-wake fallback hat when the loop stalls.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable the progress steward |
| `steward_hat_id` | string | `"progress-steward"` | Hat to wake on stalls |
| `max_steward_iterations` | integer | `3` | Consecutive quiet turns before waking |

### phase_config and warmup_config

Two-phase orchestration.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `initial` | string | `warmup` | `warmup` or `production` |
| `transition_event` | string | `"phase.transition"` | Topic that triggers phase transition |
| `warmup_config.min_iterations` | integer | `10` | Minimum warmup iterations |
| `warmup_config.max_iterations` | integer | `30` | Maximum warmup iterations |
| `warmup_config.exit_quiet_rounds` | integer | `3` | Quiet rounds before exiting warmup |
| `warmup_config.stop_on_exit` | boolean | `false` | Stop after warmup instead of transitioning |

### state_projection

Project canonical ledgers from events.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable projection |
| `actions` | map | `{}` | Topic → single `StateProjectionAction` |
| `actions_chain` | map | `{}` | Topic → ordered list of `StateProjectionAction` (takes precedence) |

**Action kinds:**

- `ensure_task`: `{ kind: ensure_task, key: <pointer>, title: <pointer> }`
- `close_task`: `{ kind: close_task, task_id: <pointer>, step: <pointer> }`
- `advance_step`: `{ kind: advance_step, current_step: <pointer>, completed_step: <pointer> }`
- `plan_complete`: `{ kind: plan_complete, final_step: <pointer> }`
- `mark_step_completed`: `{ kind: mark_step_completed, step: <pointer> }`

### macro_edge_next_hint

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Inject `## NEXT ACTION` hint from payload `next_hint` |

### cli

Backend configuration.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `backend` | string | `"claude"` | Backend name |
| `command` | string | `null` | Command override (required for `custom`) |
| `prompt_mode` | string | `"arg"` | `arg` or `stdin` |
| `default_mode` | string | `"autonomous"` | `autonomous` or `interactive` |
| `idle_timeout_secs` | integer | `30` | Interactive-mode idle timeout; `0` disables |
| `autonomous_idle_timeout_secs` | integer/null | `null` | Autonomous watchdog; `null` inherits adapter `timeout`, `0` disables |
| `args` | list | `[]` | Extra args for custom backend |
| `prompt_flag` | string | `null` | Custom prompt flag for arg mode |

**Backend values:** `claude`, `gemini`, `codex`, `opencode`, `pi`, `traecli`, `custom`.

**Prompt mode values:** `arg` or `stdin`.

### core

Core behaviors, scratchpad, and guardrails.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `scratchpad` | string or object | `{ enabled: true, path: ".ralph/agent/scratchpad.md" }` | Scratchpad configuration |
| `specs_dir` | string | `".ralph/specs/"` | Specifications directory |
| `guardrails` | list | see defaults | Rules injected into every prompt |
| `event_projection` | object | `null` | Event projection configuration |
| `state_files` | object | `null` | External file injection |
| `preflight_extensions` | object | `null` | Custom preflight hooks |
| `invariant_assertions` | boolean | `false` | Enable defense-in-depth checks |
| `workspace_root` | string | `"."` | Resolved at load time from `RALPH_WORKSPACE_ROOT` or cwd |

The `scratchpad` field accepts a plain string (shorthand for setting `path` with `enabled: true`) or a structured object with `enabled` and `path`.

> **Solo mode safety:** If scratchpad is disabled (`enabled: false`) but no hats are defined, Ralph force-enables it with a warning.

### core.event_projection

Auto-copy matching events to sidecar JSONL files.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable event projection |
| `rules` | list | `[]` | Projection rules |

**ProjectionRule fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Rule name |
| `trigger_events` | list | `[]` | Triggering topics |
| `fields` | list | `[]` | Fields to extract (`topic`, `payload`, `wave_id`, or JSON key) |
| `target_file` | string | required | Output file path |
| `mode` | string | `"append"` | `append` only |

### core.state_files

Inject external structured files into prompts as typed XML blocks.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable state file injection |
| `inject_preamble` | string | `null` | Optional text before state file contents |
| `files` | list | `[]` | State file entries |

**StateFileEntry fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | string | required | File path |
| `format` | string | `"json"` | `json` or `jsonl` |
| `char_budget` | integer | `null` | Max chars to keep (tail truncation) |
| `tail_lines` | integer | `null` | Max trailing lines to keep |

### core.preflight_extensions

Run external commands as part of `ralph preflight`.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable preflight extension hooks |
| `hooks` | list | `[]` | Hook definitions |

**PreflightHook fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Hook name (must be unique) |
| `command` | string | required | Shell command |
| `stage` | string | `"after_native"` | `before_native` or `after_native` |
| `fail_on_error` | boolean | `false` | Non-zero exit fails preflight |

Template variables in `command`: `{{config_path}}`, `{{config_dir}}`, `{{project_root}}`.

### memories

Persistent learning across sessions.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `true` | Enable memory system |
| `inject` | string | `"auto"` | `auto`, `manual`, or `none` |
| `budget` | integer | `0` | Max tokens to inject (`0` = unlimited) |
| `filter.types` | list | `[]` | Filter by memory type |
| `filter.tags` | list | `[]` | Filter by tags |
| `filter.recent` | integer | `0` | Days limit |

### tasks

Runtime work tracking.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `true` | Enable task system |
| `coordinator_hats` | list | `[]` | Hats allowed to mutate any task regardless of owner |

### features

Optional runtime capabilities.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `parallel` | boolean | `true` | Spawn worktree loops when another loop holds the primary lock |
| `auto_merge` | boolean | `false` | Auto-merge completed worktree loops |
| `loop_naming` | object | see `ralph-proto` | Loop naming configuration |
| `preflight.enabled` | boolean | `false` | Run `ralph preflight` automatically before `ralph run` |
| `preflight.strict` | boolean | `false` | Treat preflight warnings as failures |
| `preflight.skip` | list | `[]` | Skip checks by name |

When `features.preflight.enabled: true`, `ralph run` uses the default preflight suite:
`config`, `hooks`, `backend`, `git`, `paths`, `tools`, and `specs`.

### hooks

Lifecycle hooks for orchestrator phase-events (v1).

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable hook dispatch |
| `defaults.timeout_seconds` | integer | `30` | Default per-hook timeout |
| `defaults.max_output_bytes` | integer | `8192` | Default stdout/stderr cap per stream |
| `defaults.suspend_mode` | string | `wait_for_resume` | `wait_for_resume`, `retry_backoff`, or `wait_then_retry` |
| `events` | map | `{}` | `phase_event` → list of hook specs |

Supported phase-event keys:

- `pre.loop.start`, `post.loop.start`
- `pre.iteration.start`, `post.iteration.start`
- `pre.plan.created`, `post.plan.created`
- `pre.loop.complete`, `post.loop.complete`
- `pre.loop.error`, `post.loop.error`

Hook spec fields:

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Stable identifier |
| `command` | Yes | argv array |
| `cwd` | No | Working directory override |
| `env` | No | Env overrides |
| `timeout_seconds` | No | Per-hook timeout |
| `max_output_bytes` | No | Per-hook output cap |
| `on_error` | Yes | `warn`, `block`, or `suspend` |
| `suspend_mode` | No | Override suspend strategy |
| `mutate.enabled` | No | Enable stdout mutation parsing |
| `mutate.format` | No | Only `"json"` is allowed in v1 |

### profiles

Runtime profile overlay defaults.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `default` | string or list | `[]` | Default activated profile specs (`repo:<name>` or `user:<name>`) |

- `repo:<name>` → `<project-root>/ralph-profiles/<name>/`
- `user:<name>` → `$XDG_CONFIG_HOME/ralph/profiles/<name>/` or `~/.config/ralph/profiles/<name>/`

CLI `--profile` flags append after `default`; `--no-default-profiles` clears `default`.

### agent_doc_sync

Managed agent doc block synchronization.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `true` | Run sync before backend spawn |
| `on_error` | string | `warn` | `warn` or `strict` (strict exits with code 78) |
| `blocks` | list | `["builtin:hang-prevention"]` | Block identifiers to inject |
| `startup_timeout_secs` | integer | `30` | Sync timeout (`0` disables) |

One-shot disables: `--no-sync-agent-docs`, `RALPH_AGENT_DOC_SYNC=0`.

### telemetry

Runtime-diagnosis configuration.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `runtime_diagnosis.enabled` | boolean | `false` | Master switch |
| `runtime_diagnosis.write_artifacts` | boolean | `false` | Write on-disk diagnosis session |
| `runtime_diagnosis.prompt_injection_enabled` | boolean | `false` | Inject alert blocks into prompts |
| `runtime_diagnosis.max_prompt_findings` | integer | `5` | Max findings per prompt alert |
| `runtime_diagnosis.max_prompt_chars` | integer | `2000` | Max chars per alert |
| `runtime_diagnosis.retry_window_iterations` | integer | `5` | Lookback for repeated recoveries |
| `runtime_diagnosis.max_repeated_recoveries` | integer | `3` | Max repeated recoveries before escalation |
| `runtime_diagnosis.artifact_retention` | integer | `10` | Diagnosis sessions to keep |
| `runtime_diagnosis.malformed_jsonl_policy` | string | `warn` | `skip`, `warn`, or `error` |
| `runtime_diagnosis.drift.window_size` | integer | `50` | Rolling window size |
| `runtime_diagnosis.drift.field_completeness_threshold` | float | `0.9` | Required field completeness fraction |
| `runtime_diagnosis.drift.coord_join_rate_threshold` | float | `0.6` | Required join rate fraction |
| `runtime_diagnosis.drift.coord_join_mode` | string | `parallel` | `parallel` or `serial` |
| `runtime_diagnosis.drift.emit_cadence_sigma` | float | `2.0` | Cadence-drift sigma threshold |

`RALPH_DIAGNOSTICS=1` enables the full diagnostics session regardless of config.

### notifications

Loop 终止 Webhook 通知：loop 以成功或失败终止时，向飞书自定义机器人（或任意支持 HTTP POST 的 Webhook）发送**一次** best-effort 通知。

- 默认关闭：省略整个 `notifications` 段或 `enabled: false` 时零网络、零副作用，校验也永远通过。
- 仅配置在项目级 YAML（`ralph.yml` / `ralph.pipeline.yml` / `ralph.merge.yml` 或 `-c` / `RALPH_CONFIG` 指向的任意 `RalphConfig` YAML），**不进 preset**。
- 发送失败（超时 / DNS 失败 / 非 2xx / 模板渲染错误）只记录 warn 日志，**不会**改变 loop 的 `TerminationReason`，也不影响进程 exit code。

顶层字段：

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | boolean | `false` | 主开关；仅在 `true` 时发送通知并执行下方校验 |
| `timeout_seconds` | integer | `5` | 每个 endpoint 的请求超时（秒）；启用时必须 > 0 |
| `endpoints` | list | `[]` | 通知 endpoint 列表；启用时必须非空 |

`endpoints[]` 子字段：

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | string | `""` | endpoint 显示名（可选，用于日志） |
| `url` | string | `""` | Webhook POST 目标 URL；启用时必填 |
| `on` | string 或 list | `[]` | 状态过滤：`success` 和/或 `failure`；支持单值（`on: success`）或序列（`on: [success, failure]`）；启用时必填；未知值在校验时被拒绝 |
| `headers` | map | `{}` | 附加 HTTP 请求头 |
| `body` | string | `""` | 请求体模板，支持 `{{var}}` 占位符；启用时必填 |

`body` 模板变量：

| 变量 | 含义 |
|------|------|
| `{{loop_id}}` | loop 标识，如 `loop-2026-07-25-001` |
| `{{status}}` | 终止结果：`success` 或 `failure` |
| `{{termination_reason}}` | 稳定的终止原因字符串，如 `completed` / `max_iterations` / `recovery_exhausted` |
| `{{workspace}}` | loop 工作区（worktree）路径 |
| `{{repo_root}}` | 仓库根目录路径 |
| `{{iteration_current}}` | 已消耗迭代数（v1 中可能为空字符串） |
| `{{iteration_max}}` | 配置的迭代上限（v1 中可能为空字符串） |
| `{{active_hat}}` | 终止时活跃的 hat（v1 中可能为空字符串） |

替换值会做 JSON 字符串转义，可安全嵌入 JSON 字符串字面量。`body` 中出现未知 `{{var}}` 时，该 endpoint 被跳过并记录 warn（进程不崩溃）。

语义要点：

- **默认关闭**：省略或 `enabled: false` 时整段 inert，校验永远通过，无任何网络调用。
- **状态过滤基于终止原因**：`success`/`failure` 由 loop 终止边界上的 `TerminationReason` 决定——仅 `CompletionPromise`（即 `LOOP_COMPLETE` 路径）算 `success`，其余（限额、恢复耗尽、取消等）均为 `failure`。触发依据**不是**环内业务事件 `plan.blocked`（它可多次出现且不等于终止）。
- **Best-effort 不阻断**：超时、DNS 失败、非 2xx、渲染错误只记 warn，不改变 `TerminationReason`，不影响 CLI exit code，也不触发 hooks 的 block/suspend。
- **启用时非法配置硬失败**：`timeout_seconds == 0`、`endpoints` 为空、endpoint 缺少 `url`/`body`/`on`、`on` 含未知值，都会在配置校验阶段报 `notifications.*` 字段路径错误并拒绝启动。
- **与 hooks 可并存**：与 `hooks.events.post.loop.complete` / `post.loop.error` 互不干扰，但同开两套可能重复通知，请自行取舍。
- **数组合并为整表替换**：项目 YAML 合并（`-c` + `-H` 或配置覆盖）时，`endpoints` 列表按整表替换（覆盖而非追加）。
- **日志脱敏**：warn 日志与 diagnostics 中的 URL 会对 query string 做 redact，避免 token 明文落盘。

启用示例（飞书自定义机器人，`********` 为你自己的 webhook 路径占位）：

```yaml
notifications:
  enabled: true
  timeout_seconds: 5
  endpoints:
    - name: feishu-success
      url: "https://open.feishu.cn/open-apis/bot/v2/hook/********"
      on: [success]
      headers:
        Content-Type: application/json
      body: '{"msg_type":"text","content":{"text":"Ralph OK {{loop_id}} ({{termination_reason}})"}}'
    - name: feishu-failure
      url: "https://open.feishu.cn/open-apis/bot/v2/hook/********"
      on: [failure]
      headers:
        Content-Type: application/json
      body: '{"msg_type":"text","content":{"text":"Ralph FAIL {{loop_id}}: {{termination_reason}}"}}'
```

飞书自定义机器人的创建与 webhook 路径获取见飞书开放平台文档：<https://open.feishu.cn/document/ukTMukTMukTM/ucTM5YjL3ETO24yNxkjN>

延期项：飞书签名校验（timestamp/sign）、URL 环境变量展开、双向机器人/按钮回调、环内 `plan.blocked` 即时推送、失败自动重试队列均不在 v1 范围内。

### adapters

Per-backend adapter settings.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `<backend>.enabled` | boolean | `true` | Include in auto-detection |
| `<backend>.timeout` | integer | `300` | CLI execution inactivity timeout (seconds) |

`tool_permissions` under adapters is dropped; the CLI tool manages its own permissions.

### skills

Skill discovery and injection.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | boolean | `true` | Enable skills system |
| `dirs` | list | `[]` | Directories to scan for skill files |
| `overrides.<name>.enabled` | boolean | `null` | Disable/enable a skill |
| `overrides.<name>.hats` | list | `[]` | Restrict skill to hats |
| `overrides.<name>.backends` | list | `[]` | Restrict skill to backends |
| `overrides.<name>.tags` | list | `[]` | Tags |
| `overrides.<name>.auto_inject` | boolean | `null` | Inject full content into prompt |

### hats

Specialized personas for hat-based mode.

| Option | Type | Required | Description |
|--------|------|----------|-------------|
| `name` | string | Yes | Display name |
| `description` | string | Yes | Purpose description (required by validator) |
| `triggers` | list | Yes | Event subscription patterns |
| `publishes` | list | Yes | Allowed event types |
| `terminal_events` | list/string | No | Terminal topics signaling activation completion |
| `default_publishes` | string | No | Default event if none explicit |
| `max_activations` | integer | No | Activation limit |
| `backend` | string/object | No | Backend override |
| `backend_args` / `args` | list | No | Args appended to backend CLI |
| `scratchpad` | string or object | No | Per-hat scratchpad override |
| `instructions` | string | Yes | Hat-specific prompt |
| `extra_instructions` | list | No | Fragments appended to `instructions` |
| `disallowed_tools` | list | No | Tools the hat must not use |
| `timeout` | integer | No | Per-hat execution timeout (seconds) |
| `missing_event_grace_secs` | integer | No | Grace window before missing-event gate fires |
| `concurrency` | integer | `1` | Max concurrent wave instances |
| `obligations` | list | No | Activation-level publish obligations |
| `aggregate` | object | No | Wave result aggregation config |
| `event_filter` | object | No | Per-hat event allowlist |
| `exempt_topics` | list | No | Topics allowed despite lint rules |
| `allowed_write_paths` | list | No | Paths this hat may write |
| `phase_triggers` | map | No | Phase → trigger topics |
| `trigger_multi_consumer_topics` | list | No | Topics allowed to have multiple consumers |
| `ignore_payload_fields` | list | No | Payload fields to ignore in static validation |

**Hat event_filter fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | boolean | `false` | Enable filtering |
| `mode` | string | `"allowlist"` | `allowlist` only |
| `events` | list | `[]` | Topics this hat is allowed to see |

Each hat can override the global scratchpad with its own `scratchpad` field. Like the core-level setting, it accepts a plain string or a structured object.

**Resolution order:** hat override → `core.scratchpad` → defaults.

### events

Top-level event metadata definitions.

| Option | Type | Required | Description |
|--------|------|----------|-------------|
| `description` | string | No | What the event means |
| `on_trigger` | string | No | Instructions for hats that receive this event |
| `on_publish` | string | No | Instructions for hats that emit this event |

### mechanism

Top-level mechanism flow declaration for advanced presets. Also accepted under `event_loop.mechanism`.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `flow.type` | string | `"declared"` | Flow type |
| `flow.version` | integer | `1` | Flow version |
| `flow.repair_budget` | integer | `3` | Allowed repair rounds |
| `flow.enforce_schema` | string | `"hard"` | Schema enforcement mode |
| `flow.state_idempotency` | string | `"required"` | State idempotency requirement |
| `flow.terminal_emits` | list | `[]` | Terminal emit topics |
| `flow.steps` | list | `[]` | Flow step declarations |
| `flow.steps[].id` | string | required | Step identifier |
| `flow.steps[].kind` | string | No | Step kind |
| `flow.steps[].allowed_emits` | list | `[]` | Topics the step may emit |
| `flow.steps[].terminal_when` | string | No | Terminal condition |
| `flow.steps[].on_partial` | map | `{}` | Partial-result routing |

### Other top-level fields

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `verbose` | boolean | `false` | Verbose output |
| `agent_priority` | list | `[]` | Fallback auto-detection priority |
| `archive_prompts` | boolean | `false` | **Deferred** — warns if enabled |
| `enable_metrics` | boolean | `false` | **Deferred** — warns if enabled |
| `max_tokens` | integer | `null` | **Dropped** — warns if set |
| `retry_delay` | integer | `null` | **Dropped** — warns if set |
| `topic_owners` | map | `{}` | Topic → owner hats (preset lint) |
| `topic_format_whitelist` | list | `[]` | Exempt tokens from topic-format lint |
| `_suppress_warnings` | boolean | `false` | Suppress config warnings |

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
  execution_mode: isolated
  starting_event: "plan.start"

hats:
  planner:
    name: "Planner"
    description: "Creates implementation plans"
    triggers: ["plan.start"]
    publishes: ["plan.ready"]
    terminal_events: ["plan.ready"]
    instructions: |
      Create an implementation plan.

  builder:
    name: "Builder"
    description: "Implements the plan"
    triggers: ["plan.ready"]
    publishes: ["build.done"]
    terminal_events: ["build.done"]
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
  execution_mode: isolated

core:
  scratchpad:
    enabled: true
    path: .ralph/agent/scratchpad.md

hats:
  planner:
    name: "Planner"
    description: "Creates implementation plans"
    scratchpad:
      path: .ralph/agent/planner.md
    triggers: ["plan.start"]
    publishes: ["plan.ready"]
    terminal_events: ["plan.ready"]
    instructions: |
      Create an implementation plan.

  builder:
    name: "Builder"
    description: "Implements the plan"
    triggers: ["plan.ready"]
    publishes: ["build.done"]
    terminal_events: ["build.done"]
    instructions: |
      Implement the plan.

  reviewer:
    name: "Reviewer"
    description: "Reviews the implementation"
    scratchpad:
      enabled: false
    triggers: ["build.done"]
    publishes: ["review.done"]
    terminal_events: ["review.done"]
    instructions: |
      Review the implementation. No scratchpad needed.
```

### Strict Event Policy

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
| `RALPH_AGENT_DOC_SYNC` | Disable agent doc sync (`0`) |
| `RALPH_WORKSPACE_ROOT` | Override workspace root for path resolution |
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
