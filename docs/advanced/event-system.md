# Event System Design

Ralph's event system provides the communication backbone for hat orchestration, enabling agents to emit signals that trigger hat switches and backpressure mechanisms.

## Overview

Events are the only way hats communicate. Each event has:

- **Topic** — What kind of event (e.g. `build.done`)
- **Payload** — Optional string or JSON data
- **Source hat** — Which hat published it
- **Wave ID** — Optional grouping for parallel wave execution

Events flow through a validation pipeline before reaching the event bus:

```
Read JSONL → Malformed check → Scope enforcement → Event policy → Workflow guards → Bus
```

## Event Types

| Event | Description |
|-------|-------------|
| `task.start` | Signals the beginning of work |
| `plan.ready` | Planning phase finished |
| `build.done` | Implementation phase finished |
| `test.pass` | Tests passed |
| `test.fail` | Tests failed |
| `LOOP_COMPLETE` | Task fully complete — ends the loop |

## Emitting Events

```bash
# Plain string payload
ralph emit plan:complete

# JSON payload (validated when event_policy is configured)
ralph emit review.done --json '{"status": "approved", "issues": 0}'

# With provenance (recommended for hat-based workflows)
ralph emit experiment.planned --json '{"task_key":"x"}' --hat strategist --triggered implementer
```

### Provenance Flags

`ralph emit` supports three provenance flags. When a flag is omitted, Ralph falls back to an environment variable:

| CLI Flag | Env Fallback | Purpose |
|----------|--------------|---------|
| `--hat <HAT>` | `RALPH_CURRENT_HAT` | Hat that published this event |
| `--triggered <HAT>` | `RALPH_TRIGGERED_HAT` | Hat triggered by this event |
| `--source <SOURCE>` | `RALPH_EVENT_SOURCE` | Source identifier (e.g., `agent`, `cli`, `system`) |

Priority is always **CLI flag > environment variable > empty**.

When `event_policy.require_emit_provenance: true` is configured, emitting without `--hat` or `$RALPH_CURRENT_HAT` returns a non-zero exit code and writes nothing.

### Environment Provenance Injection

Ralph automatically injects provenance environment variables when spawning a Hat backend:

- `RALPH_CURRENT_HAT` — the hat currently executing
- `RALPH_CURRENT_LOOP_ID` — the active loop ID
- `RALPH_EVENTS_FILE` — resolved path to the current events file
- `RALPH_TRIGGERED_HAT` — the hat triggered by the current event (if known)

This means Hats can usually emit events without passing explicit provenance flags:

```bash
ralph emit build.done "tests: pass, lint: pass"
```

The event record in the JSONL will include `"hat": "<current-hat>"` automatically.

## Event Policy

Event policy adds typed validation and lifecycle rules on top of the basic event flow. It is opt-in and defaults to off.

### Why Use Event Policy

Without policy, events are untyped strings. A hat can emit any payload, and downstream hats must parse defensively. Event policy lets you:

1. **Require JSON object payloads** with specific fields
2. **Restrict field values** to an allowed enumeration
3. **Prevent business events after terminal topics** (e.g. nothing after `LOOP_COMPLETE`)

### Policy Modes

| Mode | Behavior |
|------|----------|
| `observe` | Violations are logged as diagnostics; events still pass through. Use this to audit existing workflows before turning on enforcement. |
| `enforce` | Violations trigger the configured `on_violation` action. |

### Violation Actions

When `mode: enforce`, Ralph handles violations as follows:

| Action | Result |
|--------|--------|
| `warn` | Log only; event enters the bus |
| `reject_with_resume` | Drop the event; publish `task.resume` with the reason |
| `hold` | Pause the loop with a hold artifact |
| `block` | Silently drop the event |

### Example: AutoResearch Pipeline

```yaml
event_loop:
  event_policy:
    enabled: true
    mode: observe                 # Start with observe, then switch to enforce
    on_violation: reject_with_resume
    schemas:
      experiment.planned:
        payload: json_object
        required_fields:
          - task_key
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

With this configuration:

- `experiment.planned` must carry a JSON payload with a `task_key` field
- `experiment.evaluated` must have `evaluation.decision` set to `keep`, `discard`, or `blocked`
- After `LOOP_COMPLETE`, any `experiment.*` event produces a terminal monotonicity violation

### Policy vs. Workflow Guards

| | Event Policy | Workflow Guards |
|---|---|---|
| **Checks** | Payload shape, field values, terminal state | Topic sequence order |
| **Rejects** | Malformed or out-of-lifecycle events | Out-of-order chain events |
| **Recovery** | `task.resume` or hold artifact | `task.resume` |
| **Scope** | Per-topic schema + global rules | Per-chain ordered sequence |

They work together: policy validates *what* an event contains; guards validate *when* it may appear.

## Strict CLI Policy Enforcement

When `event_policy.require_policy_check_for_cli_emit: true` is configured, `ralph emit` validates the event against policy **by default**, even if the user does not pass `--policy-check`.

| Config | User Action | Result |
|--------|-------------|--------|
| `require_policy_check_for_cli_emit: false` (default) | No flags | Old behavior: emit directly |
| `require_policy_check_for_cli_emit: false` | `--policy-check` | Validate once, explicit opt-in |
| `require_policy_check_for_cli_emit: true` | No flags | Validate automatically |
| `require_policy_check_for_cli_emit: true` | `--unsafe-no-policy-check` | Skip only if `allow_unsafe_cli_emit: true` |
| `require_policy_check_for_cli_emit: true` | `--unsafe-no-policy-check` with `allow_unsafe_cli_emit: false` | Rejected |

This closes the bypass path where an agent or user could emit malformed events by omitting `--policy-check`.

## Completion Monotonicity

Ralph treats completion as a **loop-level monotonic state**. Once the first `LOOP_COMPLETE` (or configured `completion_promise`) is accepted, the loop enters a protected state:

- **Duplicate terminal events** are rejected or ignored (configurable via `completion_after_terminal.duplicate_terminal`).
- **Business events after completion** are rejected or ignored (configurable via `completion_after_terminal.business_after_completion`).
- **Same-batch protection** applies: if a batch contains `LOOP_COMPLETE` followed by other events, those later events are also guarded.
- **Termination reason** remains `CompletionPromise` and is not overwritten by later noise.

This prevents race conditions where a late event could re-trigger hat routing or corrupt state after the loop has decided to finish.

### Configuration

```yaml
event_loop:
  event_policy:
    enabled: true
    completion_after_terminal:
      duplicate_terminal: reject      # warn | reject | ignore
      business_after_completion: ignore
      write_diagnostic_event: true    # emit diagnostic events when blocking
```

When `write_diagnostic_event: true`, Ralph writes events like `event.policy_warning` so downstream audit tools can see what was blocked and why.

## Loop State Snapshot

Ralph can derive a read-only snapshot from the events JSONL:

```bash
ralph loops inspect --json
```

The snapshot includes:

- **Last index** — How many events have been processed
- **Terminal** — Whether a terminal topic was observed
- **Open instances** — Workflow instances that have not reached terminal phase
- **Closed instances** — Workflow instances that have completed
- **Findings** — Policy violations observed during replay

This is useful for debugging, API queries, and TUI status display without modifying the event log.

## See Also

- [Hats & Events](../concepts/hats-and-events.md) — Core concepts
- [Configuration](../guide/configuration.md) — Full config reference including `event_policy` and migration guidance
- [Backpressure](../concepts/backpressure.md) — Backpressure mechanisms
- [Creating Custom Hats](custom-hats.md) — Custom hat development
