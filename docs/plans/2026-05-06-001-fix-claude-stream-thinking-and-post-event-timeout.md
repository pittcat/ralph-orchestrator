# Fix Claude Stream `thinking` Support + Post-Event Timeout False-Failure

**Date:** 2026-05-06  
**Type:** fix  
**Status:** draft  
**Related:** modem-batch-analysis workflow `job-1778068455-0b37`

---

## 1. Problem Statement

When Ralph drives Claude Code (`--output-format stream-json`) through the modem-batch-analysis workflow, every iteration ends with:

1. **Hundreds of `Skipping malformed JSON line` debug logs** — Claude Code emits `{"type":"thinking",...}` content blocks, but `ralph-adapters::claude_stream::ContentBlock` only knows `text` and `tool_use` variants.

2. **Loop terminates with `Too many consecutive failures`** after ~8 iterations (default threshold = 5). Each iteration is incorrectly counted as a failure because:
   - Claude Code `emit`s the routing event successfully
   - The process does **not** exit immediately afterward
   - Ralph's `POST_EVENT_GRACE_TIMEOUT` (5s) fires → SIGTERM → non-zero exit code
   - `CliExecutor` sets `success = status.success() && !timed_out` → `false`
   - `EventLoop::process_output(..., success=false)` increments `consecutive_failures`
   - After 8 iterations, `8 >= 5` triggers `TerminationReason::ConsecutiveFailures`

**Result:** The workflow logic (test_doc_parser → intake_preprocess → case_queue_coordinator → … → judgment_referee) all ran correctly, but Ralph killed the loop because of a **false-failure counter**.

---

## 2. Root Cause Chain

| Layer | File | Line | Issue |
|-------|------|------|-------|
| Stream parser | `crates/ralph-adapters/src/claude_stream.rs` | 58-67 | `ContentBlock` enum missing `Thinking` variant |
| Executor | `crates/ralph-adapters/src/cli_executor.rs` | 22, 125-176 | Post-event grace timeout uses same `timed_out` flag as inactivity timeout |
| Result builder | `crates/ralph-adapters/src/cli_executor.rs` | 241-246 | `success = status.success() && !timed_out` treats **any** timeout as failure |
| Failure counter | `crates/ralph-core/src/event_loop/mod.rs` | 1857-1862 | No distinction between "task completed but process lingered" vs "actual failure" |
| Threshold | `crates/ralph-core/src/config.rs` | 963-965 | `default_max_failures() = 5` |

---

## 3. Goals

1. **Eliminate malformed-JSON noise** — `thinking` blocks should parse silently (no-op is fine).
2. **Stop false-failure accumulation** — An iteration that successfully emits an event and is then cleaned up by post-event timeout must **not** increment `consecutive_failures`.
3. **Preserve real failure detection** — A backend that crashes, exits non-zero, or times out **without** emitting an event must still be counted as a failure.

---

## 4. Proposed Fix (Two-Part)

### Part B — Add `Thinking` Variant to `ContentBlock`

**File:** `crates/ralph-adapters/src/claude_stream.rs`

**Change:**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    Thinking { thinking: String },   // <-- NEW
}
```

**Impact:** `ClaudeStreamParser::parse_line()` will successfully deserialize `thinking` blocks instead of logging `Skipping malformed JSON line`.

**Open question:** Should `Thinking` carry a `signature` field? Claude Code's thinking blocks sometimes include a signature for verification. We can add it as optional if needed:

```rust
Thinking {
    thinking: String,
    #[serde(default)]
    signature: Option<String>,
},
```

### Part C — Distinguish Post-Event Timeout from Real Failure

**File:** `crates/ralph-adapters/src/cli_executor.rs`

**Approach A (Minimal — recommended):** Add a `post_event_timed_out` flag to `ExecutionResult` and treat post-event timeout as success.

**Step 1:** Extend `ExecutionResult`:

```rust
#[derive(Debug)]
pub struct ExecutionResult {
    pub output: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub post_event_timed_out: bool,  // <-- NEW
}
```

**Step 2:** Track post-event timeout separately in `execute()`:

```rust
let mut timed_out = false;
let mut post_event_timed_out = false;
// ...
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

**Step 3:** Build result with adjusted `success`:

```rust
Ok(ExecutionResult {
    output: accumulated_output,
    success: (status.success() && !timed_out) || post_event_timed_out,
    exit_code: status.code(),
    timed_out,
    post_event_timed_out,
})
```

**Rationale:** If the backend emitted an event and then was gracefully terminated by the post-event cleanup mechanism, the **orchestration goal** was achieved. The non-zero exit code is a side-effect of SIGTERM, not a task failure.

**Alternative Approach B (Even simpler):** In `loop_runner.rs`, override `success` when "Event emitted:" is detected in output:

```rust
let effective_success = success || output.contains("Event emitted:");
if let Some(reason) = event_loop.process_output(&hat_id, &output, effective_success) {
```

**Why not B:** It conflates "output text says event emitted" with "event was actually written to JSONL". Approach A is safer because it reasons about the executor's intent (post-event cleanup vs real timeout).

---

## 5. Implementation Steps

| # | Task | File(s) | Est. Time | Blockers |
|---|------|---------|-----------|----------|
| 1 | Add `Thinking` variant to `ContentBlock` + tests | `claude_stream.rs` | 15 min | None |
| 2 | Extend `ExecutionResult` with `post_event_timed_out` | `cli_executor.rs` | 10 min | None |
| 3 | Update timeout handling to set `post_event_timed_out` | `cli_executor.rs` | 15 min | None |
| 4 | Update `success` computation | `cli_executor.rs` | 5 min | Step 2-3 |
| 5 | Update all `ExecutionResult` construction sites | `cli_executor.rs` (tests), `loop_runner.rs`, `ralph-bench` | 20 min | Step 2 |
| 6 | Run `cargo test -p ralph-adapters` | — | 5 min | Steps 1-4 |
| 7 | Run `cargo test -p ralph-core` | — | 5 min | Steps 1-4 |
| 8 | Run smoke test (`cargo test -p ralph-core smoke_runner`) | — | 10 min | Steps 6-7 |
| 9 | Verify with a live Ralph loop against Claude Code | — | 15 min | All above |

**Total estimate:** ~1.5 hours

---

## 6. Test Plan

### Unit Tests (claude_stream.rs)

Add to existing test module:

```rust
#[test]
fn test_parse_assistant_thinking() {
    let json = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Let me analyze this...","signature":"abc123"}]}}"#;
    let event = ClaudeStreamParser::parse_line(json).unwrap();
    match event {
        ClaudeStreamEvent::Assistant { message, .. } => {
            match &message.content[0] {
                ContentBlock::Thinking { thinking, .. } => {
                    assert_eq!(thinking, "Let me analyze this...");
                }
                _ => panic!("Expected Thinking content"),
            }
        }
        _ => panic!("Expected Assistant event"),
    }
}
```

### Unit Tests (cli_executor.rs)

Add test for post-event timeout behavior:

```rust
#[tokio::test]
async fn test_post_event_timeout_is_success() {
    let backend = CliBackend {
        command: "sh".to_string(),
        args: vec!["-c".to_string(), "printf 'Event emitted: test\\n'; sleep 30".to_string()],
        prompt_mode: PromptMode::Stdin,
        prompt_flag: None,
        output_format: OutputFormat::Text,
        env_vars: vec![],
    };
    let executor = CliExecutor::new(backend);
    let result = executor
        .execute_capture_with_timeout("", Some(Duration::from_secs(30)))
        .await
        .unwrap();

    assert!(result.timed_out, "Should have timed out");
    assert!(result.post_event_timed_out, "Should be post-event timeout");
    assert!(result.success, "Post-event timeout should be treated as success");
}
```

### Smoke Test

Run the existing smoke test replay that exercises `claude_stream` parsing to ensure no regressions.

### Live Validation

Run a minimal Ralph workflow (e.g., `ralph run` with a simple hat that emits one event) and confirm:
1. No `Skipping malformed JSON line` logs when Claude Code emits thinking blocks
2. Loop continues past iteration 6+ without `consecutive_failures` termination
3. `loop.terminate` event (if any) shows `reason=completed` or similar, not `consecutive_failures`

---

## 7. Rollback / Safety

- **Part B** (`Thinking` variant) is pure additive — zero risk of breaking existing logic.
- **Part C** (`post_event_timed_out`) changes the `success` semantics. If this causes issues with other backends (e.g., Copilot, custom CLIs), we can:
  1. Revert the `success` computation to original
  2. Instead, increase `max_consecutive_failures` in `ralph.yml` as a workaround

**Feature flag option:** If we want to be extra safe, gate the new behavior behind a config flag:

```yaml
event_loop:
  treat_post_event_timeout_as_success: true  # default false for backward compat
```

---

## 8. Related Work

- `crates/ralph-adapters/src/copilot_stream.rs` — May have similar stream parsing gaps if Copilot adds new content types.
- `docs/specs/context-window-utilization.md` — Thinking blocks consume context window; future work could surface thinking-token usage in cost tracking.

---

## 9. Acceptance Criteria

- [ ] `cargo test -p ralph-adapters` passes (including new tests)
- [ ] `cargo test -p ralph-core` passes
- [ ] Smoke tests pass
- [ ] Live Ralph loop with Claude Code survives >10 iterations without `consecutive_failures`
- [ ] No `Skipping malformed JSON line` logs for `thinking` variant
- [ ] PR includes before/after log excerpts showing the fix
