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
/// tag drives the match, while `serde(other)` catches unknown event types.
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
    ///
    /// Cursor nests the concrete tool kind under `tool_call`, for example
    /// `{ "readToolCall": { "args": ... } }`.
    ToolCall {
        #[serde(default)]
        subtype: Option<String>,
        /// Stable tool call identifier (matches `started` ↔ `completed`).
        #[serde(default)]
        call_id: Option<String>,
        #[serde(default)]
        tool_call: serde_json::Value,
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

    /// Extract terminal result text from one NDJSON line.
    ///
    /// Headless callers use this only as a fallback when no assistant text was
    /// streamed, avoiding duplicate display of the same final answer.
    pub fn extract_result_text(line: &str) -> Option<String> {
        match Self::parse_line(line)? {
            AgentStreamEvent::Result { result, .. } => result,
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

/// Pull the single known tool kind out of a Cursor `tool_call` object.
///
/// Cursor `agent` emits one concrete tool per event, nested under its
/// tool name (PascalCase):
///
/// ```text
/// { "readToolCall":  { "args": {...}, "result": {...} } }
/// { "writeToolCall": { "args": {...}, "result": {...} } }
/// ```
///
/// The object must carry **exactly one** key — that key is the tool name.
/// Multi-key objects are rejected as off-spec (this fails loud if Cursor
/// ever ships an event with sibling tools instead of silently picking
/// whichever key BTreeMap iteration surfaces first). Empty objects are
/// also rejected; that means the JSON value is not a Cursor tool envelope
/// at all (e.g. a stray `{}` from a partial line) and the caller should
/// drop it rather than synthesize a phantom tool name.
fn parse_tool_call(tool_call: &serde_json::Value) -> Option<(&str, &serde_json::Value)> {
    let obj = tool_call.as_object()?;
    if obj.len() != 1 {
        return None;
    }
    obj.iter().next().map(|(name, body)| (name.as_str(), body))
}

/// Reduce a Cursor `tool_call.*.result` body to a single user-visible
/// string. Cursor wraps success / error under `result.success.*` /
/// `result.error.*`; the `success` and `error` keys can co-exist when a
/// tool emits partial output plus a failure marker.
///
/// Precedence (most important first):
/// 1. **`error`** is preferred when present — a failed tool result must
///    surface the failure to the operator pane, never be hidden by a
///    co-emitted success marker. Picking `success` here would silently
///    swallow real failures.
/// 2. **`success`** is the happy-path text (typically `{ "content":
///    "..." }` from `readToolCall`).
/// 3. **`result` itself** is the fallback when neither wrapper is set
///    (Cursor may evolve the schema — keep the function tolerant).
///
/// Empty `result` objects (`{}`) and `Null` collapse to `""` so the
/// caller never receives the literal `"{}"` string for a missing body.
fn tool_result_text(body: &serde_json::Value) -> String {
    let result = body.get("result").unwrap_or(&serde_json::Value::Null);

    // Empty object / null body → empty preview (no spurious "{}").
    if result.is_null() || matches!(result, serde_json::Value::Object(map) if map.is_empty()) {
        return String::new();
    }

    // Failure semantics win: pick `error` when present, even if a sibling
    // `success` key also exists.
    let value = result
        .get("error")
        .or_else(|| result.get("success"))
        .unwrap_or(result);

    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Object(map) if map.len() == 1 => map
            .values()
            .next()
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| value.to_string()),
        _ => value.to_string(),
    }
}

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
            tool_call,
            error,
            ..
        } => {
            let Some(id) = call_id.as_deref() else {
                return;
            };
            let Some((name, body)) = parse_tool_call(&tool_call) else {
                return;
            };

            match subtype.as_deref().unwrap_or("started") {
                "started" => {
                    let input = body.get("args").unwrap_or(&serde_json::Value::Null);
                    handler.on_tool_call(name, id, input);
                }
                "completed" => handler.on_tool_result(id, &tool_result_text(body)),
                "failed" => {
                    handler.on_tool_result(id, &tool_result_text(body));
                    if let Some(err) = error.as_deref() {
                        handler.on_error(err);
                    }
                }
                _ => {}
            }
        }
        AgentStreamEvent::Result {
            subtype,
            result,
            is_error,
            ..
        } => {
            state.is_error = is_error || subtype.as_deref() == Some("error");
            if extracted_text.is_empty() {
                if let Some(text) = result {
                    handler.on_text(&text);
                    extracted_text.push_str(&text);
                }
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
        // S3 (read-style): Cursor's nested tool_call.started shape.
        let line = json!({
            "type": "tool_call",
            "subtype": "started",
            "call_id": "call_abc",
            "tool_call": {
                "readToolCall": {
                    "args": {"path": "/tmp/example.txt"}
                }
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
        // S3: Cursor nests completed output under result.success.
        let line = json!({
            "type": "tool_call",
            "subtype": "completed",
            "call_id": "call_abc",
            "tool_call": {
                "readToolCall": {
                    "result": {"success": {"content": "file contents here"}}
                }
            }
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
        let started = json!({
            "type": "tool_call",
            "subtype": "started",
            "call_id": "call_42",
            "tool_call": {
                "writeToolCall": {
                    "args": {"path": "/tmp/out.md", "content": "hi"}
                }
            }
        })
        .to_string();
        let completed = json!({
            "type": "tool_call",
            "subtype": "completed",
            "call_id": "call_42",
            "tool_call": {
                "writeToolCall": {
                    "result": {"success": "ok"}
                }
            }
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
        assert_eq!(
            handler.tool_results[0],
            ("call_42".to_string(), "ok".to_string())
        );
    }

    /// U3 / R6 (testing:T4 + adversarial:A2): a `ToolCall { subtype:
    /// "failed" }` payload must route through the same dispatch as
    /// `completed` (callers get `on_tool_result` with the failure body)
    /// AND additionally raise `on_error` when the envelope carries an
    /// `error` string. Without this guard, Cursor's failure path silently
    /// disappears from the operator pane because `subtype` defaults to
    /// `"started"` when omitted.
    #[test]
    fn parse_tool_call_completed_failed_subtype() {
        let line = json!({
            "type": "tool_call",
            "subtype": "failed",
            "call_id": "call_fail",
            "error": "permission denied",
            "tool_call": {
                "readToolCall": {
                    "result": {"error": {"message": "permission denied"}}
                }
            }
        })
        .to_string();

        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        // `on_tool_result` fires with the error body so the agent loop
        // can still correlate call_id ↔ body.
        assert_eq!(handler.tool_results.len(), 1);
        assert_eq!(handler.tool_results[0].0, "call_fail");
        assert!(
            handler.tool_results[0].1.contains("permission denied"),
            "tool result text must surface the failure body, got {:?}",
            handler.tool_results[0].1
        );
        // `on_error` fires with the envelope-level error message.
        assert_eq!(handler.errors.len(), 1);
        assert_eq!(handler.errors[0], "permission denied");
        // Failure does not flip dispatch state to success.
        assert!(
            !state.is_error,
            "tool_call.failed must not mark session success"
        );
    }

    /// U3 / R7 (testing:T4): `tool_result_text` must collapse an empty
    /// `result` object (`{}`) to an empty string instead of the literal
    /// `"{}"` string. The previous implementation ran `value.to_string()`
    /// on the empty object, which surfaced `{}` in the operator pane —
    /// a UX bug because the operator would see `{` braces with no
    /// content and assume the tool emitted JSON output.
    #[test]
    fn tool_result_text_empty_object_returns_empty_str() {
        let body = json!({"result": {}});
        assert_eq!(tool_result_text(&body), "");
        // Null result also collapses to empty.
        let body_null = json!({"result": null});
        assert_eq!(tool_result_text(&body_null), "");
    }

    /// U3 / R7 (testing:T4): when `success` and `error` co-exist on the
    /// same `result` body, the failure `error` marker wins. Picking
    /// `success` here would silently hide real failures behind a
    /// stale success marker (a Cursor agent may stream partial success
    /// data before the tool aborts). This test pins the
    /// `error → success → bare result` precedence documented in
    /// `tool_result_text`'s rustdoc.
    #[test]
    fn tool_result_text_prefers_error_on_failure() {
        // Co-presence: error must win over success.
        let body = json!({
            "result": {
                "success": {"content": "partial output"},
                "error":   {"message": "tool aborted"}
            }
        });
        let text = tool_result_text(&body);
        assert!(
            text.contains("tool aborted"),
            "error marker must surface, got {text:?}"
        );
        assert!(
            !text.contains("partial output"),
            "success body must NOT override error, got {text:?}"
        );
    }

    /// U3 / R7 (testing:T4): happy-path `{"result": {"success": ...}}`
    /// still picks the success body — regression guard for the new
    /// precedence (only the failure branch flips, success-only payloads
    /// are unchanged).
    #[test]
    fn tool_result_text_success_only_unchanged() {
        let body = json!({"result": {"success": {"content": "file contents"}}});
        assert_eq!(tool_result_text(&body), "file contents");
    }

    /// U3 / R7 (testing:T4): `parse_tool_call` honors the documented
    /// "exactly one known tool key" contract. Picking an arbitrary first
    /// key from a multi-key object would silently misroute if Cursor
    /// ever ships an event with sibling tools; this test pins the
    /// single-key invariant by rejecting multi-key payloads.
    #[test]
    fn parse_tool_call_rejects_multi_key_object() {
        // Two sibling tools under `tool_call` is off-spec — fail loud.
        let bad = json!({
            "readToolCall":  {"args": {"path": "a"}},
            "writeToolCall": {"args": {"path": "b"}}
        });
        assert!(
            parse_tool_call(&bad).is_none(),
            "multi-key tool_call object must be rejected (off-spec)"
        );
        // Empty object is also rejected (defensive against partial JSON).
        let empty = json!({});
        assert!(
            parse_tool_call(&empty).is_none(),
            "empty tool_call object must be rejected"
        );
    }

    /// U3 / R7 (testing:T4): `parse_tool_call` picks the only key the
    /// payload carries — keeps the existing `readToolCall` /
    /// `writeToolCall` protocol shape. Pairs with the multi-key
    /// rejection test above.
    #[test]
    fn parse_tool_call_picks_documented_key() {
        let read = json!({"readToolCall": {"args": {"path": "x"}}});
        let (name, body) = parse_tool_call(&read).expect("single-key object must parse");
        assert_eq!(name, "readToolCall");
        assert_eq!(body.get("args").cloned(), Some(json!({"path": "x"})));

        let write = json!({"writeToolCall": {"args": {"path": "y", "content": "hi"}}});
        let (name, body) = parse_tool_call(&write).expect("single-key object must parse");
        assert_eq!(name, "writeToolCall");
        assert_eq!(
            body.get("args").cloned(),
            Some(json!({"path": "y", "content": "hi"}))
        );
    }

    /// U3 / R7 (testing:T4): end-to-end `ToolCall { subtype: "failed" }`
    /// dispatch: combined failure-path coverage with the error-body
    /// precedence (above). Asserts that a `failed` event whose
    /// `tool_call.readToolCall.result` carries only `success` still
    /// routes through the `failed` branch and surfaces a useful
    /// fallback (the success body) instead of an empty string.
    #[test]
    fn parse_tool_call_failed_falls_back_to_success_body() {
        let line = json!({
            "type": "tool_call",
            "subtype": "failed",
            "call_id": "call_fb",
            "error": "transient network error",
            "tool_call": {
                "readToolCall": {
                    "result": {"success": {"content": "partial file content"}}
                }
            }
        })
        .to_string();

        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();
        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert_eq!(handler.tool_results.len(), 1);
        assert_eq!(handler.tool_results[0].1, "partial file content");
        assert_eq!(handler.errors.len(), 1);
        assert_eq!(handler.errors[0], "transient network error");
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
        assert!(!state.is_error);
    }

    #[test]
    fn result_does_not_duplicate_preceding_assistant_text() {
        let assistant = json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": "final answer"}]}
        });
        let result = json!({
            "type": "result",
            "subtype": "success",
            "result": "final answer",
            "is_error": false
        });
        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();

        for line in [assistant.to_string(), result.to_string()] {
            let event = AgentStreamParser::parse_line(&line).expect("parse ok");
            dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);
        }

        assert_eq!(handler.texts, vec!["final answer".to_string()]);
        assert_eq!(extracted, "final answer");
        assert!(!state.is_error);
    }

    #[test]
    fn result_error_subtype_marks_state_error_without_is_error() {
        let line = json!({
            "type": "result",
            "subtype": "error",
            "result": "failed"
        })
        .to_string();
        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = AgentSessionState::default();

        let event = AgentStreamParser::parse_line(&line).expect("parse ok");
        dispatch_agent_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert!(state.is_error);
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
        assert!(
            t.len() <= 14,
            "truncated len should be ≤ 10 + 3-byte ellipsis, got {} ({:?})",
            t.len(),
            t
        );
        assert!(t.ends_with('…'));
        assert!(t.starts_with("aaaaaaaaaa"));
    }
}
