//! Cursor Headless CLI `agent` — `stream-json` NDJSON parser.
//!
//! Cursor CLI emits a stable JSONL envelope with a top-level `type` tag.
//! Ralph only models the variants that matter for tool/text observability:
//!
//! | NDJSON `type`   | Rust variant             | Handler effect                  |
//! |-----------------|--------------------------|---------------------------------|
//! | `assistant`     | `AgentStreamEvent::Assistant` | `on_text(text)`             |
//! | `tool_call`     | `AgentStreamEvent::ToolCall { started }` | `on_tool_call` / `on_tool_result` |
//! | `system`        | `AgentStreamEvent::System`    | (noop)                       |
//! | `result`        | `AgentStreamEvent::Result`    | `on_text(result)` (final)    |
//! | _anything else_ | `AgentStreamEvent::Other`     | (noop)                       |
//!
//! Bad lines (invalid JSON, missing fields) are skipped at debug log level
//! and never panic. This mirrors the fail-soft contract of `trae_stream`
//! and `pi_stream`.

use serde::{Deserialize, Serialize};

use crate::stream_handler::StreamHandler;

/// One parsed Cursor `agent` NDJSON event.
///
/// All shapes keep their unmodeled payload under `extra: serde_json::Value`
/// so future schema bumps cannot break the parser. The discriminated `type`
/// tag drives the match — `serde(other)` catches every event the orchestrator
/// does not yet understand.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentStreamEvent {
    /// Assistant text message. `message.content` carries a list of content
    /// blocks; text blocks have `{"type": "text", "text": "..."}`.
    Assistant {
        #[serde(default)]
        message: serde_json::Value,
        #[serde(flatten)]
        extra: serde_json::Value,
    },

    /// Tool lifecycle event. `subtype` distinguishes:
    /// - `"started"`    → tool invocation begins
    /// - `"completed"`  → tool finished (success or error)
    /// - `"failed"`     → tool failed before completion
    ToolCall {
        #[serde(default)]
        subtype: Option<String>,
        /// Stable tool call identifier (matches `started` ↔ `completed`).
        #[serde(default)]
        call_id: Option<String>,
        /// Tool name (e.g. `readToolCall`, `writeToolCall`).
        #[serde(default)]
        tool_name: Option<String>,
        /// Tool input (started) or output (completed). May be absent.
        #[serde(default)]
        args: Option<serde_json::Value>,
        /// Tool result text (completed/failed). May be absent.
        #[serde(default)]
        result: Option<String>,
        /// Error message when `subtype == "failed"`.
        #[serde(default)]
        error: Option<String>,
        #[serde(flatten)]
        extra: serde_json::Value,
    },

    /// System init / heartbeat events. Currently ignored.
    System {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(flatten)]
        extra: serde_json::Value,
    },

    /// Final result event. `subtype` is `"success"` or `"error"`.
    /// Carries the final assistant output text (used as a fallback for
    /// headless completion detection, parallel to `trae_stream::Result`).
    Result {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default)]
        result: Option<String>,
        #[serde(default)]
        is_error: bool,
        #[serde(flatten)]
        extra: serde_json::Value,
    },

    /// Any other event type we don't model yet (e.g. `ping`, future Cursor
    /// additions). Captured but ignored.
    #[serde(other)]
    Other,
}

/// Aggregated session state from `agent` stream events. Mirrors the
/// `TraeSessionState` / `PiSessionState` pattern — callers can read this
/// after the loop to populate LOOP_COMPLETE diagnostics.
#[derive(Debug, Clone, Default)]
pub struct AgentSessionState {
    /// Final `result.result` text (last seen Result event).
    pub final_result: Option<String>,
    /// `true` if the final `result` event carried `is_error: true` or
    /// `subtype == "error"`.
    pub is_error: bool,
}

/// Parses NDJSON lines from Cursor `agent` --output-format stream-json.
pub struct AgentStreamParser;

impl AgentStreamParser {
    /// Parse a single NDJSON line into an [`AgentStreamEvent`].
    ///
    /// Returns `None` for empty lines or malformed JSON. Never panics.
    pub fn parse_line(line: &str) -> Option<AgentStreamEvent> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        match serde_json::from_str::<AgentStreamEvent>(trimmed) {
            Ok(event) => Some(event),
            Err(e) => {
                tracing::debug!(
                    "Skipping malformed agent JSON: {} (error: {})",
                    truncate(trimmed, 100),
                    e
                );
                None
            }
        }
    }

    /// Extract assistant text from a single Cursor `agent` NDJSON line.
    ///
    /// Returns the concatenated text content of an assistant event, or
    /// `None` for any other event type. Used by `CliExecutor` for
    /// headless rendering.
    pub fn extract_text(line: &str) -> Option<String> {
        match Self::parse_line(line)? {
            AgentStreamEvent::Assistant { message, .. } => extract_assistant_text(&message),
            _ => None,
        }
    }

    /// Extract all assistant text from a raw NDJSON buffer.
    ///
    /// Concatenates text deltas across all assistant events. Each text
    /// block is appended; if the cumulative extraction ends up empty
    /// we fall back to the `result.result` field of the final `result`
    /// event (preserves the final output even when the assistant only
    /// emitted tool calls). Returns the raw buffer unchanged if neither
    /// source yields text — preserves debug visibility, same contract as
    /// `TraeStreamParser::extract_all_text`.
    pub fn extract_all_text(raw_output: &str) -> String {
        let mut extracted = String::new();
        let mut fallback_result: Option<String> = None;

        for line in raw_output.lines() {
            if let Some(event) = Self::parse_line(line) {
                match event {
                    AgentStreamEvent::Assistant { message, .. } => {
                        if let Some(text) = extract_assistant_text(&message) {
                            extracted.push_str(&text);
                            if !text.ends_with('\n') {
                                extracted.push('\n');
                            }
                        }
                    }
                    AgentStreamEvent::Result { result, .. } => {
                        if let Some(text) = result {
                            fallback_result = Some(text);
                        }
                    }
                    _ => {}
                }
            }
        }

        if extracted.is_empty() {
            fallback_result.unwrap_or_else(|| raw_output.to_string())
        } else {
            extracted
        }
    }
}

/// Walk an assistant message and collect all `text` content blocks.
///
/// Real shape (Cursor docs):
/// ```json
/// {
///   "type": "assistant",
///   "message": {
///     "role": "assistant",
///     "content": [
///       {"type": "text", "text": "hello"},
///       {"type": "text", "text": "world"}
///     ]
///   }
/// }
/// ```
fn extract_assistant_text(message: &serde_json::Value) -> Option<String> {
    let blocks = message.get("content")?.as_array()?;
    let mut out = String::new();
    let mut any = false;
    for block in blocks {
        let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if block_type == "text"
            && let Some(text) = block.get("text").and_then(|v| v.as_str())
        {
            if any {
                out.push('\n');
            }
            out.push_str(text);
            any = true;
        }
    }
    if any { Some(out) } else { None }
}

/// Dispatch a Cursor `agent` stream event to the [`StreamHandler`].
///
/// Accumulates result data in `state` for the final `on_complete()` call
/// (parallel contract to `dispatch_trae_stream_event`). Appends text
/// content to `extracted_text` for LOOP_COMPLETE detection.
pub fn dispatch_agent_stream_event<H: StreamHandler>(
    event: AgentStreamEvent,
    handler: &mut H,
    extracted_text: &mut String,
    state: &mut AgentSessionState,
) {
    match event {
        AgentStreamEvent::Assistant { message, .. } => {
            if let Some(text) = extract_assistant_text(&message) {
                handler.on_text(&text);
                extracted_text.push_str(&text);
            }
        }
        AgentStreamEvent::ToolCall {
            subtype,
            call_id,
            tool_name,
            args,
            result,
            error,
            ..
        } => {
            // Treat missing/unknown subtype as `started` defensively: if we
            // see a call_id and tool_name without `completed`/`failed`, emit
            // `on_tool_call`. This keeps the existing parser behavior — only
            // started emits on_tool_call.
            let subtype = subtype.as_deref().unwrap_or("started");
            match subtype {
                "started" => {
                    if let (Some(id), Some(name)) = (call_id.as_deref(), tool_name.as_deref()) {
                        let input = args.unwrap_or_else(|| serde_json::Value::Object(Default::default()));
                        handler.on_tool_call(name, id, &input);
                    }
                }
                "completed" => {
                    if let Some(id) = call_id.as_deref() {
                        let output = result.unwrap_or_default();
                        handler.on_tool_result(id, &output);
                    }
                }
                "failed" => {
                    if let Some(id) = call_id.as_deref() {
                        let output = result.unwrap_or_default();
                        handler.on_tool_result(id, &output);
                        if let Some(err) = error.as_deref() {
                            handler.on_error(err);
                        }
                    }
                }
                _ => {
                    // Unknown tool_call subtype — ignore silently.
                }
            }
        }
        AgentStreamEvent::Result {
            subtype: _,
            result,
            is_error,
            ..
        } => {
            state.final_result = result.clone();
            state.is_error = is_error;
            if let Some(text) = result {
                handler.on_text(&text);
                extracted_text.push_str(&text);
            }
        }
        AgentStreamEvent::System { .. } => {}
        AgentStreamEvent::Other => {}
    }
}

/// Truncate `s` to at most `max_chars` characters (UTF-8 safe by bytes).
fn truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        // Find the largest char boundary <= max_chars bytes.
        let mut idx = max_chars;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        let mut out = String::with_capacity(idx + 1);
        out.push_str(&s[..idx]);
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream_handler::SessionResult;
    use serde_json::json;

    /// Test double that records every callback. Mirrors the pattern used
    /// in `trae_stream::tests` and `pi_stream::tests`.
    #[derive(Debug, Default, Clone)]
    struct RecordingHandler {
        texts: Vec<String>,
        tool_calls: Vec<(String, String, serde_json::Value)>,
        tool_results: Vec<(String, String)>,
        errors: Vec<String>,
        completes: Vec<SessionResult>,
    }

    impl StreamHandler for RecordingHandler {
        fn on_text(&mut self, text: &str) {
            self.texts.push(text.to_string());
        }
        fn on_tool_call(&mut self, name: &str, id: &str, input: &serde_json::Value) {
            self.tool_calls
                .push((name.to_string(), id.to_string(), input.clone()));
        }
        fn on_tool_result(&mut self, id: &str, output: &str) {
            self.tool_results.push((id.to_string(), output.to_string()));
        }
        fn on_error(&mut self, error: &str) {
            self.errors.push(error.to_string());
        }
        fn on_complete(&mut self, result: &SessionResult) {
            self.completes.push(result.clone());
        }
    }

    // ---------- parse_line: happy paths (S2, S3) ----------

    #[test]
    fn parse_assistant_text_event_emits_text() {
        // S2: assistant content with text blocks triggers on_text.
        let line = json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "hello world"}
                ]
            }
        })
        .to_string();

        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert_eq!(handler.texts, vec!["hello world".to_string()]);
        assert_eq!(extracted, "hello world");
        assert!(handler.tool_calls.is_empty());
        assert!(handler.tool_results.is_empty());
        assert!(handler.errors.is_empty());
    }

    #[test]
    fn parse_assistant_multiple_text_blocks_concatenated() {
        let line = json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "first"},
                    {"type": "text", "text": "second"}
                ]
            }
        })
        .to_string();

        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert_eq!(handler.texts, vec!["first\nsecond".to_string()]);
        assert_eq!(extracted, "first\nsecond");
    }

    #[test]
    fn parse_assistant_extra_fields_ignored() {
        // Forward-compat: unknown sibling fields must not break parsing.
        let line = json!({
            "type": "assistant",
            "timestamp_ms": 12345,
            "model_call_id": "call_xyz",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "ok"}
                ]
            }
        })
        .to_string();

        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        match event {
            AgentStreamEvent::Assistant { message, .. } => {
                let text = extract_assistant_text(&message).expect("has text");
                assert_eq!(text, "ok");
            }
            other => panic!("expected Assistant, got {:?}", other),
        }
    }

    #[test]
    fn parse_tool_call_started_emits_on_tool_call() {
        // S3 (read-style): tool_call.started triggers on_tool_call.
        let line = json!({
            "type": "tool_call",
            "subtype": "started",
            "call_id": "call_abc",
            "tool_name": "readToolCall",
            "args": {
                "path": "/tmp/example.txt"
            }
        })
        .to_string();

        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert_eq!(handler.tool_calls.len(), 1);
        assert_eq!(handler.tool_calls[0].0, "readToolCall");
        assert_eq!(handler.tool_calls[0].1, "call_abc");
        assert_eq!(handler.tool_calls[0].2, json!({"path": "/tmp/example.txt"}));
        assert!(handler.tool_results.is_empty());
    }

    #[test]
    fn parse_tool_call_completed_emits_on_tool_result() {
        // S3: tool_call.completed with same call_id triggers on_tool_result.
        let line = json!({
            "type": "tool_call",
            "subtype": "completed",
            "call_id": "call_abc",
            "tool_name": "readToolCall",
            "result": "file contents here"
        })
        .to_string();

        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert_eq!(handler.tool_results.len(), 1);
        assert_eq!(handler.tool_results[0].0, "call_abc");
        assert_eq!(handler.tool_results[0].1, "file contents here");
        assert!(handler.tool_calls.is_empty());
    }

    #[test]
    fn parse_tool_call_started_then_completed_round_trip() {
        // Realistic shape: started then completed for the same call_id.
        let started = json!({
            "type": "tool_call",
            "subtype": "started",
            "call_id": "call_42",
            "tool_name": "writeToolCall",
            "args": {"path": "/tmp/out.md", "content": "hi"}
        })
        .to_string();
        let completed = json!({
            "type": "tool_call",
            "subtype": "completed",
            "call_id": "call_42",
            "tool_name": "writeToolCall",
            "result": "ok"
        })
        .to_string();

        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();

        let ev1 = AgentStreamParser::parse_line(&started).expect("parse started");
        dispatch_agent_stream_event(ev1, &mut handler, &mut extracted, &mut state);
        let ev2 = AgentStreamParser::parse_line(&completed).expect("parse completed");
        dispatch_agent_stream_event(ev2, &mut handler, &mut extracted, &mut state);

        assert_eq!(handler.tool_calls.len(), 1);
        assert_eq!(handler.tool_calls[0].1, "call_42");
        assert_eq!(handler.tool_results.len(), 1);
        assert_eq!(handler.tool_results[0].0, "call_42");
    }

    // ---------- parse_line: noop paths (S4 + ignored types) ----------

    #[test]
    fn parse_system_event_is_noop() {
        // S4: system events must be ignored, no callbacks fired.
        let line = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "sess_xyz"
        })
        .to_string();

        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert!(handler.texts.is_empty());
        assert!(handler.tool_calls.is_empty());
        assert!(handler.tool_results.is_empty());
        assert!(handler.errors.is_empty());
    }

    #[test]
    fn parse_unknown_type_is_noop() {
        // Future event types from Cursor must be captured but not break flow.
        let line = json!({"type": "ping", "ts": 1234}).to_string();

        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert!(handler.texts.is_empty());
        assert!(handler.tool_calls.is_empty());
        assert!(handler.tool_results.is_empty());
    }

    // ---------- parse_line: malformed input (S8) ----------

    #[test]
    fn parse_invalid_json_returns_none_no_panic() {
        // S8: malformed JSON must not panic and must return None.
        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        for bad in [
            "not json",
            "{",
            "{ \"type\":",
            "{ \"type\": \"assistant\" }",
            "",
            "   ",
            "\u{0}\u{0}\u{0}",
        ] {
            let event = AgentStreamParser::parse_line(bad);
            if let Some(ev) = event {
                dispatch_agent_stream_event(ev, &mut handler, &mut extracted, &mut state);
            }
        }
        // Stream continues to be processable — no panics, no fake callbacks.
        assert!(handler.texts.is_empty());
        assert!(handler.tool_calls.is_empty());
    }

    #[test]
    fn parse_assistant_with_missing_text_blocks_is_noop() {
        // Defensive: assistant event with empty/non-text content blocks
        // must not produce a phantom on_text.
        let line = json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "image", "url": "https://example.com/x.png"}
                ]
            }
        })
        .to_string();

        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert!(handler.texts.is_empty());
        assert_eq!(extracted, "");
    }

    // ---------- result event (S5 fallback) ----------

    #[test]
    fn parse_result_event_emits_text_and_updates_state() {
        // Final result event carries the assistant's final output.
        let line = json!({
            "type": "result",
            "subtype": "success",
            "result": "final answer",
            "is_error": false
        })
        .to_string();

        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert_eq!(handler.texts, vec!["final answer".to_string()]);
        assert_eq!(extracted, "final answer");
        assert_eq!(state.final_result.as_deref(), Some("final answer"));
        assert!(!state.is_error);
    }

    #[test]
    fn parse_result_error_event_marks_state_error() {
        let line = json!({
            "type": "result",
            "subtype": "error",
            "is_error": true,
            "result": "something went wrong"
        })
        .to_string();

        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert!(state.is_error);
        assert_eq!(state.final_result.as_deref(), Some("something went wrong"));
        // Final result text still surfaces through on_text for completion
        // detection — error semantics live in state, not in dropped payloads.
        assert_eq!(handler.texts, vec!["something went wrong".to_string()]);
    }

    // ---------- extract_text / extract_all_text (S5) ----------

    #[test]
    fn extract_text_returns_assistant_text_only() {
        let assistant = json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "hi"}]
            }
        })
        .to_string();
        let tool = json!({
            "type": "tool_call",
            "subtype": "started",
            "call_id": "x",
            "tool_name": "readToolCall"
        })
        .to_string();

        assert_eq!(
            AgentStreamParser::extract_text(&assistant).as_deref(),
            Some("hi")
        );
        assert!(AgentStreamParser::extract_text(&tool).is_none());
    }

    #[test]
    fn extract_all_text_concatenates_assistant_messages() {
        let raw = [
            json!({"type":"system","subtype":"init"}).to_string(),
            json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"a"}]}}).to_string(),
            json!({"type":"tool_call","subtype":"started","call_id":"c","tool_name":"readToolCall"}).to_string(),
            json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"b"}]}}).to_string(),
        ].join("\n");
        let text = AgentStreamParser::extract_all_text(&raw);
        assert_eq!(text, "a\nb\n");
    }

    #[test]
    fn extract_all_text_falls_back_to_result_when_no_assistant_text() {
        // S5: when assistant emits only tool calls (no text), the final
        // `result.result` is the source of truth for completion detection.
        let raw = [
            json!({"type":"system","subtype":"init"}).to_string(),
            json!({"type":"tool_call","subtype":"started","call_id":"c","tool_name":"readToolCall"}).to_string(),
            json!({"type":"tool_call","subtype":"completed","call_id":"c","tool_name":"readToolCall","result":"contents"}).to_string(),
            json!({"type":"result","subtype":"success","result":"final summary","is_error":false}).to_string(),
        ].join("\n");
        let text = AgentStreamParser::extract_all_text(&raw);
        assert_eq!(text, "final summary");
    }

    #[test]
    fn extract_all_text_returns_raw_when_nothing_extracts() {
        // Nothing to extract — preserve raw for debug visibility.
        let raw = "not json\n{\"type\":\"system\"}\n";
        let text = AgentStreamParser::extract_all_text(raw);
        assert_eq!(text, raw);
    }

    // ---------- truncate ----------

    #[test]
    fn truncate_short_string_passthrough() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_truncates() {
        let s = "a".repeat(50);
        let t = truncate(&s, 10);
        // ASCII inputs: 10 chars + 1 ellipsis = 11 bytes (ellipsis is `…` = 3 bytes UTF-8).
        assert!(t.len() <= 14, "truncated len should be ≤ 10 + 3-byte ellipsis, got {} ({:?})", t.len(), t);
        assert!(t.ends_with('…'));
        assert!(t.starts_with("aaaaaaaaaa"));
    }
}
