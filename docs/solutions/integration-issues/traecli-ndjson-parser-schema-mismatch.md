---
title: traecli NDJSON parser schema mismatch
date: 2026-06-05
category: integration-issues
module: ralph-adapters
problem_type: integration_issue
component: tooling
severity: high
symptoms:
  - "Subprocess exited before starting the orchestration loop"
  - "All trae-cli events rejected as malformed JSON: `missing field 'type'`, `missing field 'message'`"
  - "Assistant text never reaches output_writer, LOOP_COMPLETE never fires"
root_cause: wrong_api
resolution_type: code_fix
tags:
  - trae
  - traecli
  - ndjson
  - serde
  - stream-json
  - backend-integration
---

# traecli NDJSON parser schema mismatch

## Problem

`ralph run --backend traecli` (or any traecli-configured execution) silently
fails with "Subprocess exited before starting the orchestration loop".
Every trae-cli NDJSON event is dropped at the parser because
`TraeStreamEvent`/`TraeAssistantMessage`/`TraeUserMessage` were modeled
against a hypothesized schema that does not match the real trae-cli 0.120.37
output.

## Symptoms

- Terminal shows `Subprocess exited before starting the orchestration loop`
  with `Loop lock held by another process` / `Configuration error` in
  `.ralph/diagnostics/logs/*.log`
- DEBUG log shows repeated `Skipping malformed trae JSON: ...` for almost
  every event: `assistant`, `user`, even `result`
- No assistant text is ever extracted — the loop runs the iteration but
  produces no output and never detects `LOOP_COMPLETE`
- `--no-tui --verbose` shows the full underlying error:
  `error: missing field 'type'` and `error: missing field 'message'`

## What Didn't Work

- **Modeling `assistant.message` and `user` as `#[serde(tag = "type")]`
  enums**. The original schema assumed trae-cli uses an inner `type` tag
  to discriminate text vs tool-call responses, but trae-cli uses field
  presence (`content: <string>` for text, `tool_calls: [...]` for tool
  calls, `subtype: "tool_result"` for user tool results). Every assistant
  and user event failed to deserialize.
- **Modeling `user.tool_result` as a `message: TraeUserMessage`
  variant**. Real trae-cli tool_result events have **no `message` field at
  all** — they are flat with `subtype`, `tool_use_id`, `tool_name`, and
  `content: { content: [{type, text}, ...] }` at the top level. The
  `tag = "type"` requirement on `TraeUserMessage` could never match.
- **Using `#[serde(flatten)] extra: Value` on `Result` to absorb
  unknown fields**. The final `result.result: "<text>"` field got silently
  swallowed into `extra` and was never read. The loop's final output
  source of truth was lost.

## Solution

Replace the tag-based enum discrimination with **field-presence detection
on raw `serde_json::Value`**. The new shape:

```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraeStreamEvent {
    System { subtype, session_id, #[serde(flatten)] extra },
    Assistant { #[serde(default)] message: serde_json::Value },
    User {
        #[serde(default)] message: serde_json::Value,
        #[serde(default, rename = "subtype")] subtype: Option<String>,
        #[serde(default, rename = "tool_use_id")] tool_use_id: Option<String>,
        #[serde(default, rename = "tool_name")] tool_name: Option<String>,
        #[serde(default)] content: serde_json::Value,
    },
    Result {
        #[serde(rename = "duration_ms", default)] duration_ms: u64,
        #[serde(rename = "is_error", default)] is_error: bool,
        #[serde(default)] result: Option<String>,   // <-- explicit final text
        #[serde(flatten)] extra: serde_json::Value,
    },
    #[serde(other)] Other,
}
```

And four new free functions that read field presence:

```rust
pub fn extract_assistant_text(message: &Value) -> Option<String> {
    // Returns Some(text) iff message has no `tool_calls` AND a non-empty `content` string.
    // Returns None for tool-call messages (their text is the tool's name+args).
}
pub fn extract_assistant_tool_calls(message: &Value) -> Vec<TraeAssistantToolCall> {
    // Returns one struct per `message.tool_calls[]` element.
}
pub fn extract_user_tool_result_text(content: &Value) -> Option<String> {
    // Joins `content.content[].text` from the real tool_result shape.
}
pub fn user_is_tool_result(subtype: Option<&str>, tool_use_id: Option<&str>, content: &Value) -> bool {
    subtype == Some("tool_result") || (tool_use_id.is_some() && !content.is_null())
}
```

`extract_all_text()` additionally falls back to `Result.result` when no
assistant text was found, so the final output is preserved even when the
assistant only emitted tool calls. Callers in `cli_executor.rs`,
`pty_executor.rs`, `output_parsing.rs`, and `wave/io.rs` are updated to use
the new helpers via `ralph_adapters::{extract_assistant_text, ...}`.

## Why This Works

Real trae-cli does not use a discriminator tag inside `message` — it just
nests everything under `message: { role, content?, tool_calls? }` and
relies on which fields are present. Using `tag = "type"` forced the schema
to be self-describing, which broke the moment trae-cli shipped a real
shape. Capturing the inner bag as `serde_json::Value` and dispatching on
field presence is closer to what trae-cli actually emits and tolerates
forward-compatible additions (e.g. `reasoning_content`, `response_meta`)
that the old schema would have rejected.

For the `result.result` field, the old `#[serde(flatten)] extra` worked
mechanically (the field was parsed into `extra: Value`) but the dispatch
function never looked there. Promoting it to a first-class
`result: Option<String>` makes it impossible to forget again.

## Prevention

- **Capture real CLI samples before designing the parser schema.** The
  original implementation was designed from a partial diagnostic capture
  (only system init and one error result were visible). The real shape
  includes a tool-call assistant, a tool_result user, and a success
  result — all of which the old schema rejected. The next time we
  integrate a new backend's stream format, capture a complete
  `--output-format stream-json` run on a real prompt *before* writing
  the parser.
- **Test the parser against real captured events, not hypothesized
  shapes.** Seven new tests in `crates/ralph-adapters/src/trae_stream.rs`
  (under `// Real trae-cli sample tests`) lock in the verified schema.
  They use real JSON captured 2026-06-05 from trae-cli 0.120.37 and
  assert the parser accepts each event type and extracts the expected
  text / tool call / result. If trae-cli changes its event format, these
  tests will break first.
- **Prefer `serde_json::Value` over tag-based enums when the upstream
  uses field presence.** Field-presence dispatch with helper functions is
  more robust to forward-compatible additions and doesn't force the
  upstream to be self-describing when it isn't.
- **Promote final-output fields to first-class enum variants** instead
  of letting `#[serde(flatten)] extra: Value` swallow them. If a field
  is "the answer", make it a required or `Option`-typed variant field so
  the compiler and the dispatch function can both see it.

## Related Issues

- `docs/plans/2026-06-02-005-fix-traecli-backend-availability-plan.md` —
  the original plan this fix builds on; documents the prior round of
  shallow U3/U5/U6 completion and the post-mortem remediation that
  followed.
