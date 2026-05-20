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
```

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
- [Configuration](../guide/configuration.md) — Full config reference including `event_policy`
- [Backpressure](../concepts/backpressure.md) — Backpressure mechanisms
- [Creating Custom Hats](custom-hats.md) — Custom hat development
