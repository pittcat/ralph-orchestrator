# Execution Contracts

Execution contracts validate agent completion obligations before events can trigger downstream hats. They prevent false positives when agents forget to emit or emit incomplete completion signals.

## Overview

An execution contract validates three things before a `work.done` event enters the bus:

1. **Payload fields** — Required fields are present
2. **Task state** — The referenced task is closed
3. **Git evidence** — There are real changes (unless trivial)

If any validation fails, the event is rejected and guidance is injected to drive correction.

## Configuration

```yaml
event_loop:
  execution_contracts:
    enabled: true
    rules:
      work.done:
        # Required payload fields
        require_payload_fields: ["task_id", "task_key", "step"]

        # Task validation
        require_task:
          id_field: "task_id"
          key_field: "task_key"
          loop_scoped: true
          allowed_terminal_statuses: ["closed"]
          auto_close_on_valid: false

        # Git validation
        require_git_change:
          mode: "diff_or_commit"
          allow_empty_for_steps: ["trivial"]

        # Test evidence (future)
        require_test_evidence:
          mode: "optional"
```

## Field Reference

### `require_payload_fields`

List of JSON field names that must be present in the event payload.

### `require_task`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `id_field` | string | `"task_id"` | JSON field containing task ID |
| `key_field` | string | `"task_key"` | JSON field containing task key |
| `loop_scoped` | bool | `true` | Task must belong to current loop |
| `allowed_terminal_statuses` | list | `["closed"]` | Valid task statuses |
| `auto_close_on_valid` | bool | `false` | Auto-close task if contract passes |

### `require_git_change`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | string | `"diff_or_commit"` | Git evidence mode |
| `allow_empty_for_steps` | list | `[]` | Steps that don't need git evidence |

**Modes:**
- `diff_or_commit`: Accept if `git diff` or `git log` shows changes
- `diff_only`: Only accept if `git diff` has changes
- `commit_only`: Only accept if there are commits

### `require_test_evidence`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | string | `"optional"` | Evidence requirement level |

**Modes:**
- `optional`: No test evidence required
- `required_payload_field`: Check for `tests` field in payload (future)

## Rejection Behavior

When a contract is rejected:

1. The original event is NOT published to the bus
2. A diagnostic event is published to `event.execution_contract.rejected`
3. Guidance is published to `human.guidance`
4. The downstream hat does NOT receive the event

## ce-executor Example

The `ce-executor` preset uses execution contracts to protect the executor:

```yaml
event_loop:
  execution_contracts:
    enabled: true
    rules:
      work.done:
        require_payload_fields: ["plan_name", "plan_path", "task_id", "task_key", "step"]
        require_task:
          id_field: "task_id"
          key_field: "task_key"
          loop_scoped: true
          allowed_terminal_statuses: ["closed"]
        require_git_change:
          mode: "diff_or_commit"
          allow_empty_for_steps: ["trivial"]
```

This prevents:
- Fake `work.done` when executor forgets to emit
- `work.done` with open tasks (not closed)
- `work.done` without real git changes

## Diagnostics

Contract rejections are logged and visible in:

1. **Warnings**: Logged at `warn!` level with topic, hat, and violation reason
2. **Diagnostics file**: When `RALPH_DIAGNOSTICS=1`, written to `.ralph/diagnostics/*/execution-contract.jsonl`
3. **Human guidance**: Published to `human.guidance` for next iteration

## Testing

Run execution contract tests:

```bash
cargo test -p ralph-core execution_contract -- --nocapture
cargo test -p ralph-core event_loop::tests::test_execution_contract -- --nocapture
```
