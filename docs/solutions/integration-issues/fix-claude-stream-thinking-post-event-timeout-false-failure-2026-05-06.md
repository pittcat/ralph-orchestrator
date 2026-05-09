---
title: "Claude Stream thinking blocks cause malformed JSON logs and post-event timeout triggers false consecutive failures"
date: 2026-05-06
category: integration-issues
module: ralph-adapters
problem_type: integration_issue
component: tooling
symptoms:
  - "Claude Code emits thinking blocks that trigger \"Skipping malformed JSON line\" debug logs"
  - "Loop terminates with consecutive failures after approximately 8 iterations"
  - "Backend successfully emits event but process lingers after post-event grace timeout"
root_cause: logic_error
resolution_type: code_fix
severity: high
tags:
  - claude-stream
  - cli-executor
  - post-event-timeout
  - consecutive-failures
  - thinking-blocks
  - stream-json
related_components:
  - ralph-cli
  - ralph-core
---

# Claude Stream `thinking` Support + Post-Event Timeout False-Failure

## Problem

When Ralph drives Claude Code (`--output-format stream-json`) through workflows, every iteration ends with two related issues:

1. **Malformed JSON noise**: Claude Code emits `{"type":"thinking",...}` content blocks, but `ralph-adapters::claude_stream::ContentBlock` only knew `text` and `tool_use` variants. This produced hundreds of `Skipping malformed JSON line` debug logs per run.

2. **False-failure loop termination**: After ~8 iterations, Ralph terminates with `Too many consecutive failures`. The workflow logic ran correctly — Claude emitted the routing event successfully — but the process did not exit immediately. Ralph's `POST_EVENT_GRACE_TIMEOUT` (5s) fired SIGTERM, the process exited non-zero, and `CliExecutor` reported `success=false`. The event loop incremented `consecutive_failures` each iteration until it hit the default threshold of 5.

## Symptoms

- Debug logs flooded with `Skipping malformed JSON line` for `thinking` blocks
- Loop terminates with `TerminationReason::ConsecutiveFailures` after ~8 iterations
- `Event emitted:` appears in output, but iteration is counted as a failure
- Process receives SIGTERM from post-event grace timeout, not from inactivity timeout

## What Didn't Work

- Increasing `max_consecutive_failures` in config would mask the symptom but not fix the root cause
- Filtering logs at a higher level would hide the `thinking` block parsing issue

## Solution

### Part 1: Add `Thinking` variant to `ContentBlock`

**File**: `crates/ralph-adapters/src/claude_stream.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
}
```

This allows `ClaudeStreamParser::parse_line()` to deserialize `thinking` blocks silently instead of logging malformed JSON warnings.

### Part 2: Distinguish post-event timeout from real failure

**File**: `crates/ralph-adapters/src/cli_executor.rs`

1. Extend `ExecutionResult` with a new flag:

```rust
pub struct ExecutionResult {
    pub output: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub post_event_timed_out: bool,  // NEW
}
```

2. In `execute()`, track whether the timeout was a post-event grace timeout:

```rust
let mut timed_out = false;
let mut post_event_timed_out = false;

// ... in the timeout handler ...
Err(_) => {
    warn!(..., "Execution inactivity timeout reached, sending SIGTERM");
    timed_out = true;
    if post_event_deadline.is_some() {
        post_event_timed_out = true;
    }
    terminated_status = Some(Self::terminate_child_and_wait(&mut child).await?);
    break;
}
```

3. Adjust `success` computation so post-event timeout counts as success:

```rust
Ok(ExecutionResult {
    output: accumulated_output,
    success: (status.success() && !timed_out) || post_event_timed_out,
    exit_code: status.code(),
    timed_out,
    post_event_timed_out,
})
```

### Part 3: Update downstream match sites

Any code that pattern-matches on `ContentBlock` must handle the new variant. Two locations were updated:

- `crates/ralph-adapters/src/pty_executor.rs` — `dispatch_stream_event`: added `ContentBlock::Thinking { .. } => { /* noop */ }`
- `crates/ralph-cli/src/loop_runner.rs` — `extract_readable_delta`: added `ContentBlock::Thinking { thinking, .. }` to extract thinking text as readable output

## Why This Works

- **Thinking variant**: Claude's `stream-json` output includes `thinking` content blocks (with an optional `signature` for verification). By adding the variant to the enum, serde can deserialize these blocks without falling through to the error path.

- **Post-event timeout semantics**: The post-event grace timeout (`POST_EVENT_GRACE_TIMEOUT = 5s`) exists specifically to clean up backends that have already emitted their event but don't exit cleanly (e.g., Claude Code may keep the process alive). If the backend emitted the event, the orchestration goal was achieved. The non-zero exit code is a side-effect of SIGTERM, not a task failure. By tracking `post_event_timed_out` separately, we preserve real failure detection (crashes, non-zero exits without event emission, inactivity timeouts without events) while eliminating the false-positive failure count.

## Prevention

- When adding new backend stream formats, ensure the parser enum covers all known content block types from the backend's documentation
- When adding timeout-related logic to executors, consider whether the timeout represents a genuine failure or a cleanup mechanism — if cleanup, distinguish it in the result struct
- Add unit tests that simulate "emit event + linger" behavior to catch false-failure regressions

## Related

- `docs/plans/2026-05-06-001-fix-claude-stream-thinking-and-post-event-timeout.md` — Implementation plan for this fix
- `crates/ralph-adapters/src/claude_stream.rs` — Stream parser
- `crates/ralph-adapters/src/cli_executor.rs` — CLI executor with timeout logic
- `crates/ralph-core/src/event_loop/mod.rs` — Failure counting and termination logic
