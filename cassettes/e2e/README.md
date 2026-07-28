# E2E Test Cassettes

This directory contains recorded cassettes for mock-mode E2E testing. Cassettes are JSONL files that record the output from real Ralph sessions, enabling deterministic, cost-free test execution.

## Directory Structure

```
cassettes/e2e/
├── README.md           # This file
├── connect.jsonl       # Generic connectivity cassette (all backends)
├── events.jsonl        # Event XML parsing test
├── completion.jsonl    # LOOP_COMPLETE detection test
├── single-iter.jsonl   # Single iteration orchestration
├── multi-iter.jsonl    # Multi-iteration orchestration
└── ...
```

## Available Cassettes

| Cassette | Scenario | Status |
|----------|----------|--------|
| `connect.jsonl` | Connectivity | ✅ Passes all backends |
| `events.jsonl` | Event parsing | ✅ Passes all backends |
| `completion.jsonl` | LOOP_COMPLETE | ✅ Passes all backends |
| `single-iter.jsonl` | Single iteration | ⚠️ Scratchpad assertion fails (no file writes) |
| `multi-iter.jsonl` | Multi-iteration | ⚠️ Iteration count fails (architecture limitation) |

## Known Limitations

1. **Multi-iteration scenarios**: Mock-cli replays entire cassette in one invocation, so Ralph sees only one iteration
2. **File write assertions**: Scenarios checking scratchpad/artifact content fail unless whitelisted commands execute
3. **Task/Memory scenarios**: Require cassettes with `bus.publish` events containing whitelisted commands

## Naming Convention

Cassettes follow this naming pattern:
- `<scenario-id>.jsonl` - Generic fallback cassette
- `<scenario-id>-<backend>.jsonl` - Backend-specific cassette

When running in mock mode, the cassette resolver checks for:
1. `<scenario>-<backend>.jsonl` (backend-specific, preferred)
2. `<scenario>.jsonl` (generic fallback)

## Recording New Cassettes

To record a new cassette from a live session:

```bash
# Record with ralph's built-in recording
cargo run --bin ralph -- run \
  -c ralph.yml \
  --record-session cassettes/e2e/my-scenario.jsonl \
  -p "Your prompt here"
```

## Cassette Format

Each line is a JSON object with these fields:
- `ts`: Unix timestamp in milliseconds
- `event`: Event type (e.g., `ux.terminal.write`, `bus.publish`, `_meta.iteration`)
- `data`: Event-specific data

### Event Types

| Event | Description |
|-------|-------------|
| `ux.terminal.write` | Terminal output (base64-encoded bytes) |
| `ux.terminal.resize` | Terminal size change |
| `bus.publish` | EventBus event (includes tool calls) |
| `_meta.loop_start` | Orchestration loop started |
| `_meta.iteration` | Iteration completed |
| `_meta.termination` | Loop terminated |

### Terminal Write Data

```json
{
  "bytes": "SGVsbG8=",  // Base64-encoded output
  "stdout": true,       // true for stdout, false for stderr
  "offset_ms": 100      // Time offset from session start
}
```

## Marker Cassettes (Plan 2026-07-28-001 U3 / R16 / S13)

Cassettes used by the **per-activation mock-cli harness** (see
`crates/ralph-e2e/src/scenarios/parallel_forge.rs` and
`crates/ralph-e2e/src/mock_cli.rs`) follow a stricter wire format.
The scenario ID `parallel-forge-dispatch-contract` is the first
consumer; the format is intentionally reusable for future preset
E2E suites that need deterministic per-activation grouping.

### Layout

Each cassette is a JSONL stream whose records are one of:

```jsonc
{
  "ts": 1700000000000,                          // RFC 3339-ish millis
  "event": "_meta.activation",                   // group boundary marker
  "data": { "index": 0 }                        // monotonic 0..N-1
}
```

```jsonc
{
  "ts": 1700000000100,
  "event": "ux.terminal.write",
  "data": { "bytes": "SGVsbG8=", "stdout": true, "offset_ms": 10 }
}
```

Activation groups start on a `_meta.activation` marker and end at
the next such marker. Indices are contiguous and 0-based; the
mock-cli harness aborts non-zero on any gap or duplicate. Markers
themselves are **not** forwarded to the `SessionPlayer`; they are
pure boundary signals for the harness cursor.

`bus.publish` records carry a `data.command` field; for marker
cassettes that field MUST be exactly `ralph emit`,
`ralph wave verify`, or `ralph wave emit` (the mock-cli replaces
the leading `ralph` with `--ralph-bin`). Any other command verb is
non-zero exit and the cursor does not advance.

### Cursor

The harness advances a workspace-local cursor at
`.ralph/e2e-mock/<scenario-id>-<backend>.cursor` (atomic temp +
rename under `ralph_core::FileLock`); each invocation consumes the
current group, on success writes the next index, then releases the
lock. The scenario's `cleanup` removes the directory after the
scenario's run body finishes; the run body asserts `cursor_index`
equals the declared group count.

### Recording Recipe (per-activation cassettes)

Recording by `--record-session` does not currently emit
`_meta.activation` markers. Until the recording path is upgraded,
follow this recipe to extend a marker cassette manually:

```text
1. ralph run --worktree --plan docs/plans/... (live, real backend)
   each backend invocation writes its full stdout / stderr plus
   the parsed `bus.publish` events to cassettes/e2e/<id>.jsonl
2. Insert `_meta.activation` markers at every hat boundary
   between recorded groups; consecutive markers may not skip an
   index and may not collide on the same index.
3. Replace any local file / worktree paths with the cassette
   workspace-relative forms; the harness substitutes
   `{{task_id:<plan_key>:<unit_id>}}` in `bus.publish.data.command`
   payloads against the workspace's `tasks.jsonl` after the projector
   pass.
```

## Usage

Run E2E tests in mock mode:

```bash
# Run all tests with cassettes
cargo run -p ralph-e2e -- --mock

# Run with specific cassette directory
cargo run -p ralph-e2e -- --mock --cassette-dir ./my-cassettes

# Run with real-time playback (1x speed)
cargo run -p ralph-e2e -- --mock --mock-speed 1.0

# Check cassette availability
cargo run -p ralph-e2e -- --mock --list
```

## Creating Cassettes for New Scenarios

1. Run the scenario against a live backend with recording enabled
2. Copy the recorded JSONL to the appropriate location
3. Name it according to the convention above
4. Run `ralph-e2e --mock --filter <scenario-id>` to verify
