//! Trae CLI stream event types for parsing `--output-format stream-json` NDJSON output.
//!
//! When invoked with `--output-format stream-json`, trae-cli emits newline-delimited JSON events.
//! This module provides typed Rust structures for deserializing and processing these events,
//! plus a dispatch function for mapping them to `StreamHandler` calls.
//!
//! Only events that Ralph needs are modeled as typed variants. All other event types
//! are captured by `#[serde(other)]` and silently ignored, providing forward compatibility
//! with new trae-cli event types.

use crate::stream_handler::StreamHandler;
use serde::{Deserialize, Serialize};

/// Events from trae-cli's `--output-format stream-json` NDJSON output.
///
/// Only the events Ralph needs are modeled. All other event types
/// (ping, status, etc.) are captured by the `Other` variant.
///
/// The `assistant.message` and `user` inner shapes are NOT discriminated by
/// a `type` tag — trae-cli uses field presence (`content` string for text,
/// `tool_calls` array for tool calls, `subtype: "tool_result"` for user
/// tool results). Those inner shapes are captured as `serde_json::Value` and
/// interpreted by `extract_assistant_text`, `extract_assistant_tool_calls`,
/// `extract_user_tool_result_text`, and `user_is_tool_result`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraeStreamEvent {
    /// System initialization and status events.
    System {
        #[serde(rename = "subtype")]
        subtype: String,
        #[serde(rename = "session_id")]
        session_id: Option<String>,
        #[serde(flatten)]
        extra: serde_json::Value,
    },

    /// Assistant message events. `message` is captured as raw JSON because
    /// trae-cli uses field presence (not a `type` tag) to discriminate
    /// between text and tool-call responses.
    Assistant {
        #[serde(default)]
        message: serde_json::Value,
    },

    /// User events. Two real shapes:
    /// 1. **User input**: `message: { role, content: <string>, extra }`
    /// 2. **Tool result**: no `message`; `subtype: "tool_result"`, `tool_use_id`,
    ///    `tool_name`, `content: { content: [{type, text}, ...] }`
    User {
        #[serde(default)]
        message: serde_json::Value,
        #[serde(default, rename = "subtype")]
        subtype: Option<String>,
        #[serde(default, rename = "tool_use_id")]
        tool_use_id: Option<String>,
        #[serde(default, rename = "tool_name")]
        tool_name: Option<String>,
        #[serde(default)]
        content: serde_json::Value,
    },

    /// Final result event. Real shape includes top-level `result` field with
    /// the final assistant output string.
    Result {
        #[serde(rename = "duration_ms", default)]
        duration_ms: u64,
        #[serde(rename = "is_error", default)]
        is_error: bool,
        /// Final assistant output text, if present in the result event.
        #[serde(default)]
        result: Option<String>,
        #[serde(flatten)]
        extra: serde_json::Value,
    },

    /// All other events (ping, status updates, etc.)
    #[serde(other)]
    Other,
}

/// One tool call as observed in `assistant.message.tool_calls[i]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraeAssistantToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    /// Free-form function descriptor. Real trae-cli uses
    /// `function: { name, arguments }` where `arguments` is a JSON-encoded
    /// string (not an object).
    #[serde(default)]
    pub function: TraeToolFunction,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TraeToolFunction {
    #[serde(default)]
    pub name: String,
    /// trae-cli emits the arguments as a JSON-encoded string. Stored raw so
    /// callers can decide whether to parse.
    #[serde(default)]
    pub arguments: String,
}

/// Extracted assistant text from a trae assistant message (real shape).
///
/// Returns `Some(text)` if the message contains a non-empty `content` string
/// AND no `tool_calls` (i.e. it is a pure text response). Tool-call messages
/// return `None` here because their text is the tool's name + arguments,
/// which is exposed via `extract_assistant_tool_calls`.
pub fn extract_assistant_text(message: &serde_json::Value) -> Option<String> {
    if !message.is_object() {
        return None;
    }
    let has_tool_calls = message
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if has_tool_calls {
        return None;
    }
    message
        .get("content")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Extracted tool calls from a trae assistant message (real shape).
pub fn extract_assistant_tool_calls(message: &serde_json::Value) -> Vec<TraeAssistantToolCall> {
    if !message.is_object() {
        return Vec::new();
    }
    let Some(arr) = message.get("tool_calls").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| serde_json::from_value::<TraeAssistantToolCall>(v.clone()).ok())
        .collect()
}

/// Extracted tool result text from a trae user tool_result event.
///
/// Real shape: `content: { content: [{ type: "text", text: "..." }] }`.
/// Returns joined text blocks.
pub fn extract_user_tool_result_text(content: &serde_json::Value) -> Option<String> {
    let blocks = content.get("content").and_then(|v| v.as_array())?;
    let parts: Vec<&str> = blocks
        .iter()
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// True if the user event carries a tool result (vs. raw user input).
pub fn user_is_tool_result(
    subtype: Option<&str>,
    tool_use_id: Option<&str>,
    content: &serde_json::Value,
) -> bool {
    subtype == Some("tool_result") || (tool_use_id.is_some() && !content.is_null())
}

/// Backwards-compat alias: some callers still expect `TraeAssistantMessage`
/// as a token; the new model is a `serde_json::Value` so we re-export a
/// type alias. This avoids touching every call site in `cli_executor` etc.
pub type TraeAssistantMessage = serde_json::Value;

/// Backwards-compat alias: `TraeUserMessage` is now a struct-like tuple
/// exposing the user event's discriminating fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TraeUserMessage {
    #[serde(default)]
    pub message: serde_json::Value,
    #[serde(default, rename = "subtype")]
    pub subtype: Option<String>,
    #[serde(default, rename = "tool_use_id")]
    pub tool_use_id: Option<String>,
    #[serde(default, rename = "tool_name")]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub content: serde_json::Value,
}

/// Backwards-compat: text content helper (legacy name).
pub type TraeTextContent = TraeTextPayload;

/// Legacy text payload wrapper. Used by tests that referenced the old
/// `TraeTextContent { text: String }` shape.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TraeTextPayload {
    pub text: String,
}

/// Backwards-compat: tool use content wrapper (legacy name).
pub type TraeToolUseContent = TraeLegacyToolUse;

/// Legacy tool use content wrapper.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TraeLegacyToolUse {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// Backwards-compat: tool result content wrapper (legacy name).
pub type TraeToolResultContent = TraeTextPayload;

/// Parses NDJSON lines from trae-cli's stream output.
pub struct TraeStreamParser;

impl TraeStreamParser {
    /// Parse a single line of NDJSON output.
    ///
    /// Returns `None` for empty lines or malformed JSON (logged at debug level).
    pub fn parse_line(line: &str) -> Option<TraeStreamEvent> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        match serde_json::from_str::<TraeStreamEvent>(trimmed) {
            Ok(event) => Some(event),
            Err(e) => {
                tracing::debug!(
                    "Skipping malformed trae JSON: {} (error: {})",
                    truncate(trimmed, 100),
                    e
                );
                None
            }
        }
    }

    /// Extract assistant text from a single trae-cli NDJSON line.
    ///
    /// Returns the text content of an assistant text event, or `None` for any
    /// other event type (system, tool calls, tool results, result, etc.).
    /// Used by `CliExecutor` for headless output rendering.
    pub fn extract_text(line: &str) -> Option<String> {
        match Self::parse_line(line)? {
            TraeStreamEvent::Assistant { message } => extract_assistant_text(&message),
            _ => None,
        }
    }

    /// Extract all assistant text from a raw NDJSON buffer.
    ///
    /// Concatenates text deltas across all assistant events, terminating each
    /// with a newline. Falls back to the `result` field of the final
    /// `result` event if no assistant text is found (preserves the final
    /// output even when the assistant only emitted tool calls). Returns the
    /// raw output unchanged if neither source yields text (preserves
    /// debug visibility).
    pub fn extract_all_text(raw_output: &str) -> String {
        let mut extracted = String::new();
        let mut fallback_result: Option<String> = None;

        for line in raw_output.lines() {
            if let Some(event) = Self::parse_line(line) {
                match event {
                    TraeStreamEvent::Assistant { message } => {
                        if let Some(text) = extract_assistant_text(&message) {
                            extracted.push_str(&text);
                            if !text.ends_with('\n') {
                                extracted.push('\n');
                            }
                        }
                    }
                    TraeStreamEvent::Result { result, .. } => {
                        if let Some(text) = result {
                            fallback_result = Some(text);
                        }
                    }
                    _ => {}
                }
            }
        }

        if extracted.is_empty() {
            if let Some(text) = fallback_result {
                text
            } else {
                raw_output.to_string()
            }
        } else {
            extracted
        }
    }
}

/// State accumulated across events for session summary.
#[derive(Default)]
pub struct TraeSessionState {
    pub duration_ms: u64,
    pub is_error: bool,
}

/// Dispatch a trae stream event to the `StreamHandler`.
///
/// Accumulates result data in `state` for the final `on_complete()` call.
/// Appends text content to `extracted_text` for LOOP_COMPLETE detection.
pub fn dispatch_trae_stream_event<H: StreamHandler>(
    event: TraeStreamEvent,
    handler: &mut H,
    extracted_text: &mut String,
    state: &mut TraeSessionState,
) {
    match event {
        TraeStreamEvent::Assistant { message } => {
            // Pure text response (no tool_calls): emit on_text.
            if let Some(text) = extract_assistant_text(&message) {
                handler.on_text(&text);
                extracted_text.push_str(&text);
            }
            // Tool calls: emit on_tool_call per call.
            for call in extract_assistant_tool_calls(&message) {
                let input: serde_json::Value = if call.function.arguments.is_empty() {
                    serde_json::Value::Object(Default::default())
                } else {
                    serde_json::from_str(&call.function.arguments).unwrap_or_else(|_| {
                        serde_json::Value::String(call.function.arguments.clone())
                    })
                };
                handler.on_tool_call(&call.function.name, &call.id, &input);
            }
        }
        TraeStreamEvent::User {
            message,
            subtype,
            tool_use_id,
            content,
            ..
        } => {
            if user_is_tool_result(subtype.as_deref(), tool_use_id.as_deref(), &content) {
                if let Some(output) = extract_user_tool_result_text(&content) {
                    if !output.is_empty() {
                        // We don't have the tool call ID here in the legacy
                        // dispatch path; the executor routes this through
                        // on_text so LOOP_COMPLETE detection still works.
                        handler.on_text(&output);
                        extracted_text.push_str(&output);
                    }
                }
            } else if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
                // Original user input is also text — include it for debug
                // visibility but don't drive tool flow.
                handler.on_text(text);
            }
        }
        TraeStreamEvent::Result {
            duration_ms,
            is_error,
            result,
            ..
        } => {
            state.duration_ms = duration_ms;
            state.is_error = is_error;
            if let Some(text) = result {
                // Final assistant output: emit on_text so LOOP_COMPLETE and
                // downstream pipelines see it. Also accumulate to extracted_text.
                handler.on_text(&text);
                extracted_text.push_str(&text);
            }
        }
        TraeStreamEvent::System { .. } => {}
        TraeStreamEvent::Other => {}
    }
}

/// Truncates a string to a maximum length, adding "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let boundary = s
            .char_indices()
            .take_while(|(i, _)| *i < max_len)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}...", &s[..boundary])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionResult;
    use serde_json::json;

    // =========================================================================
    // TraeStreamParser::parse_line tests
    // =========================================================================

    #[test]
    fn test_parse_system_event() {
        let json = r#"{"type":"system","subtype":"init","session_id":"sess_123"}"#;
        let event = TraeStreamParser::parse_line(json).unwrap();

        match event {
            TraeStreamEvent::System {
                subtype,
                session_id,
                ..
            } => {
                assert_eq!(subtype, "init");
                assert_eq!(session_id, Some("sess_123".to_string()));
            }
            _ => panic!("Expected System event, got {:?}", event),
        }
    }

    #[test]
    fn test_parse_assistant_text_event() {
        let json = r#"{"type":"assistant","message":{"role":"assistant","content":"Hello world"}}"#;
        let event = TraeStreamParser::parse_line(json).unwrap();

        match event {
            TraeStreamEvent::Assistant { message } => {
                assert_eq!(
                    extract_assistant_text(&message).as_deref(),
                    Some("Hello world")
                );
            }
            _ => panic!("Expected Assistant event"),
        }
    }

    #[test]
    fn test_parse_assistant_tool_use_event() {
        let json = r#"{"type":"assistant","message":{"role":"assistant","content":"","tool_calls":[{"id":"toolu_001","type":"function","function":{"name":"Bash","arguments":"{\"command\":\"echo hi\"}"}}]}}"#;
        let event = TraeStreamParser::parse_line(json).unwrap();

        match event {
            TraeStreamEvent::Assistant { message } => {
                let calls = extract_assistant_tool_calls(&message);
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "toolu_001");
                assert_eq!(calls[0].function.name, "Bash");
                let parsed: serde_json::Value =
                    serde_json::from_str(&calls[0].function.arguments).unwrap();
                assert_eq!(parsed["command"], "echo hi");
            }
            _ => panic!("Expected Assistant event"),
        }
    }

    #[test]
    fn test_parse_user_tool_result_event() {
        let json = r#"{"type":"user","subtype":"tool_result","tool_use_id":"t1","tool_name":"Bash","content":{"content":[{"type":"text","text":"done"}]}}"#;
        let event = TraeStreamParser::parse_line(json).unwrap();

        match event {
            TraeStreamEvent::User {
                content,
                subtype,
                tool_use_id,
                ..
            } => {
                assert_eq!(subtype.as_deref(), Some("tool_result"));
                assert_eq!(tool_use_id.as_deref(), Some("t1"));
                let text = extract_user_tool_result_text(&content);
                assert_eq!(text.as_deref(), Some("done"));
            }
            _ => panic!("Expected User event"),
        }
    }

    #[test]
    fn test_parse_result_event() {
        let json = r#"{"type":"result","duration_ms":1234,"is_error":false}"#;
        let event = TraeStreamParser::parse_line(json).unwrap();

        match event {
            TraeStreamEvent::Result {
                duration_ms,
                is_error,
                ..
            } => {
                assert_eq!(duration_ms, 1234);
                assert!(!is_error);
            }
            _ => panic!("Expected Result event"),
        }
    }

    #[test]
    fn test_parse_result_error_event() {
        let json = r#"{"type":"result","duration_ms":500,"is_error":true}"#;
        let event = TraeStreamParser::parse_line(json).unwrap();

        match event {
            TraeStreamEvent::Result { is_error, .. } => {
                assert!(is_error);
            }
            _ => panic!("Expected Result event"),
        }
    }

    #[test]
    fn test_parse_unknown_event_type() {
        // ping, status, etc. should parse as Other
        let json = r#"{"type":"ping"}"#;
        let event = TraeStreamParser::parse_line(json).unwrap();
        assert!(matches!(event, TraeStreamEvent::Other));

        let json = r#"{"type":"status","value":"ok"}"#;
        let event = TraeStreamParser::parse_line(json).unwrap();
        assert!(matches!(event, TraeStreamEvent::Other));
    }

    #[test]
    fn test_parse_empty_line() {
        assert!(TraeStreamParser::parse_line("").is_none());
        assert!(TraeStreamParser::parse_line("   ").is_none());
        assert!(TraeStreamParser::parse_line("\n").is_none());
    }

    #[test]
    fn test_parse_malformed_json() {
        assert!(TraeStreamParser::parse_line("{not valid json}").is_none());
        assert!(TraeStreamParser::parse_line("plain text").is_none());
    }

    // =========================================================================
    // dispatch_trae_stream_event tests
    // =========================================================================

    /// Recording handler for testing dispatch behavior.
    #[derive(Default)]
    struct RecordingHandler {
        texts: Vec<String>,
        tool_calls: Vec<(String, String, serde_json::Value)>,
        tool_results: Vec<(String, String)>,
        errors: Vec<String>,
        completions: Vec<SessionResult>,
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
            self.completions.push(result.clone());
        }
    }

    #[test]
    fn test_dispatch_assistant_text() {
        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = TraeSessionState::default();

        let event = TraeStreamEvent::Assistant {
            message: json!({"role": "assistant", "content": "Hello"}),
        };

        dispatch_trae_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert_eq!(handler.texts, vec!["Hello"]);
        assert_eq!(extracted, "Hello");
    }

    #[test]
    fn test_dispatch_assistant_tool_use() {
        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = TraeSessionState::default();

        let event = TraeStreamEvent::Assistant {
            message: json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "toolu_001",
                    "type": "function",
                    "function": {
                        "name": "Bash",
                        "arguments": "{\"command\":\"echo hi\"}"
                    }
                }]
            }),
        };

        dispatch_trae_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert_eq!(handler.tool_calls.len(), 1);
        assert_eq!(handler.tool_calls[0].0, "Bash");
        assert_eq!(handler.tool_calls[0].1, "toolu_001");
        assert_eq!(handler.tool_calls[0].2["command"], "echo hi");
    }

    #[test]
    fn test_dispatch_result_updates_state() {
        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = TraeSessionState::default();

        let event = TraeStreamEvent::Result {
            duration_ms: 1234,
            is_error: false,
            result: None,
            extra: serde_json::Value::Null,
        };

        dispatch_trae_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert_eq!(state.duration_ms, 1234);
        assert!(!state.is_error);
    }

    #[test]
    fn test_dispatch_result_error() {
        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = TraeSessionState::default();

        let event = TraeStreamEvent::Result {
            duration_ms: 500,
            is_error: true,
            result: None,
            extra: serde_json::Value::Null,
        };

        dispatch_trae_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert!(state.is_error);
    }

    #[test]
    fn test_dispatch_system_is_noop() {
        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = TraeSessionState::default();

        let event = TraeStreamEvent::System {
            subtype: "init".to_string(),
            session_id: Some("sess_123".to_string()),
            extra: serde_json::Value::Null,
        };

        dispatch_trae_stream_event(event, &mut handler, &mut extracted, &mut state);

        assert!(handler.texts.is_empty());
        assert!(handler.tool_calls.is_empty());
        assert!(extracted.is_empty());
    }

    #[test]
    fn test_dispatch_other_is_noop() {
        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = TraeSessionState::default();

        dispatch_trae_stream_event(
            TraeStreamEvent::Other,
            &mut handler,
            &mut extracted,
            &mut state,
        );

        assert!(handler.texts.is_empty());
        assert!(handler.tool_calls.is_empty());
        assert!(handler.tool_results.is_empty());
        assert!(handler.errors.is_empty());
        assert!(handler.completions.is_empty());
        assert!(extracted.is_empty());
    }

    #[test]
    fn test_dispatch_user_tool_result_emits_text() {
        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = TraeSessionState::default();

        let event = TraeStreamEvent::User {
            message: serde_json::Value::Null,
            subtype: Some("tool_result".to_string()),
            tool_use_id: Some("t1".to_string()),
            tool_name: Some("Read".to_string()),
            content: json!({
                "content": [
                    {"type": "text", "text": "file contents"}
                ]
            }),
        };

        dispatch_trae_stream_event(event, &mut handler, &mut extracted, &mut state);

        // Tool result is emitted as text since we don't have the call ID
        assert_eq!(handler.texts, vec!["file contents"]);
        assert_eq!(extracted, "file contents");
    }

    #[test]
    fn test_dispatch_multiple_texts_accumulate() {
        let mut handler = RecordingHandler::default();
        let mut extracted = String::new();
        let mut state = TraeSessionState::default();

        let event1 = TraeStreamEvent::Assistant {
            message: json!({"role": "assistant", "content": "Hello "}),
        };
        let event2 = TraeStreamEvent::Assistant {
            message: json!({"role": "assistant", "content": "world"}),
        };

        dispatch_trae_stream_event(event1, &mut handler, &mut extracted, &mut state);
        dispatch_trae_stream_event(event2, &mut handler, &mut extracted, &mut state);

        assert_eq!(handler.texts, vec!["Hello ", "world"]);
        assert_eq!(extracted, "Hello world");
    }

    // =========================================================================
    // Real trae-cli sample tests (captured 2026-06-05 from trae-cli 0.120.37).
    //
    // These tests assert the parser matches the actual NDJSON shape emitted
    // by `trae-cli --output-format stream-json`, not a hypothesized schema.
    // If trae-cli changes its event format, these tests should break first
    // and force a parser update before silent regressions reach production.
    // =========================================================================

    /// Assistant text event: real shape has `message.role` + `message.content`
    /// (plain string), with `tool_calls` absent or empty. There is NO `type`
    /// tag inside `message`.
    #[test]
    fn test_real_assistant_text_event() {
        let json = r#"{
            "type": "assistant",
            "session_id": "19975a0b-85be-40e5-8472-2a3e580348d1",
            "uuid": "760a1e8b535bf649e3ad00727be04c65",
            "message": {
                "role": "assistant",
                "content": "HELLO",
                "response_meta": {"finish_reason": "stop"},
                "extra": {"_source_model": "DeepSeek-V4-Flash"}
            }
        }"#;
        let event =
            TraeStreamParser::parse_line(json).expect("real assistant text event must parse");

        match event {
            TraeStreamEvent::Assistant { message } => {
                let text = extract_assistant_text(&message);
                assert_eq!(text.as_deref(), Some("HELLO"));
            }
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    /// Assistant tool_call event: real shape has `message.tool_calls` array
    /// with `function.{name, arguments}` and a top-level `tool_use.id`.
    #[test]
    fn test_real_assistant_tool_call_event() {
        let json = r#"{
            "type": "assistant",
            "session_id": "19975a0b-85be-40e5-8472-2a3e580348d1",
            "uuid": "9d55bd3ed7a395cdc6143ef9dd362147",
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "d5eb95f2-f236-48d3-8153-d49edd588d24",
                    "type": "function",
                    "function": {
                        "name": "Read",
                        "arguments": "{\"file_path\":\"/tmp/x\"}"
                    }
                }]
            }
        }"#;
        let event =
            TraeStreamParser::parse_line(json).expect("real assistant tool_call event must parse");

        match event {
            TraeStreamEvent::Assistant { message } => {
                let tool_calls = extract_assistant_tool_calls(&message);
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].function.name, "Read");
                assert_eq!(tool_calls[0].id, "d5eb95f2-f236-48d3-8153-d49edd588d24");
                let parsed: serde_json::Value =
                    serde_json::from_str(&tool_calls[0].function.arguments).unwrap();
                assert_eq!(parsed["file_path"], "/tmp/x");
            }
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    /// User input event: real shape has `message.role` + `message.content`
    /// (plain string). No `type` tag inside `message`.
    #[test]
    fn test_real_user_input_event() {
        let json = r#"{
            "type": "user",
            "session_id": "19975a0b-85be-40e5-8472-2a3e580348d1",
            "uuid": "a0b79db43cdb76c9017d5a57566b8478",
            "message": {
                "role": "user",
                "content": "Print HELLO",
                "extra": {"is_original_user_input": true}
            }
        }"#;
        let event = TraeStreamParser::parse_line(json).expect("real user input event must parse");

        match event {
            TraeStreamEvent::User { .. } => {}
            other => panic!("expected User, got {other:?}"),
        }
    }

    /// User tool_result event: real shape has NO `message` field. Instead
    /// top-level fields: `subtype: "tool_result"`, `tool_use_id`, `tool_name`,
    /// `content: { content: [{ type: "text", text: "..." }] }`.
    #[test]
    fn test_real_user_tool_result_event() {
        let json = r#"{
            "type": "user",
            "subtype": "tool_result",
            "session_id": "19975a0b-85be-40e5-8472-2a3e580348d1",
            "uuid": "4bc021ef9bee7312fa245a08b3fbde5c",
            "tool_use_id": "d5eb95f2-f236-48d3-8153-d49edd588d24",
            "tool_name": "Read",
            "content": {
                "content": [
                    {"type": "text", "text": "file contents here"}
                ]
            }
        }"#;
        let event =
            TraeStreamParser::parse_line(json).expect("real user tool_result event must parse");

        match event {
            TraeStreamEvent::User { .. } => {}
            other => panic!("expected User, got {other:?}"),
        }
    }

    /// Result event: real shape includes a top-level `result` field with the
    /// final output string. The parser must preserve and expose it.
    #[test]
    fn test_real_result_with_text_field() {
        let json = r#"{
            "type": "result",
            "subtype": "success",
            "session_id": "19975a0b-85be-40e5-8472-2a3e580348d1",
            "uuid": "5626e8902f06f87d074295bd87c50f11",
            "result": "HELLO",
            "is_error": false,
            "num_turns": 1,
            "duration_ms": 3518,
            "usage": {"input_tokens": 30850, "output_tokens": 46},
            "total_cost_usd": 0,
            "permission_mode": "bypass_permissions"
        }"#;
        let event = TraeStreamParser::parse_line(json).expect("real result event must parse");

        match event {
            TraeStreamEvent::Result {
                result,
                duration_ms,
                is_error,
                ..
            } => {
                assert_eq!(result.as_deref(), Some("HELLO"));
                assert_eq!(duration_ms, 3518);
                assert!(!is_error);
            }
            other => panic!("expected Result, got {other:?}"),
        }
    }

    /// End-to-end on a multi-line buffer of real trae-cli output: text from
    /// the result event must be extracted when the assistant message has no
    /// visible text (i.e. result is the final source of truth).
    #[test]
    fn test_real_extract_all_text_falls_back_to_result_field() {
        let raw = r#"{"type":"system","subtype":"init","session_id":"s1","uuid":"u1"}
{"type":"assistant","session_id":"s1","uuid":"u2","message":{"role":"assistant","content":"","tool_calls":[{"id":"t1","type":"function","function":{"name":"Read","arguments":"{}"}}]}}
{"type":"user","subtype":"tool_result","session_id":"s1","uuid":"u3","tool_use_id":"t1","tool_name":"Read","content":{"content":[{"type":"text","text":"secret data"}]}}
{"type":"result","subtype":"success","session_id":"s1","uuid":"u4","result":"HELLO","is_error":false,"num_turns":1,"duration_ms":100}
"#;
        let extracted = TraeStreamParser::extract_all_text(raw);
        assert!(
            extracted.contains("HELLO"),
            "result field must contribute to extracted text, got: {extracted:?}"
        );
    }

    /// Forward-compat: a `user` event that is neither a tool_result (no
    /// `subtype`) nor has a `message` field should still parse as User
    /// without panicking.
    #[test]
    fn test_real_user_event_minimal() {
        let json = r#"{"type":"user","session_id":"s1","uuid":"u9","subtype":"status"}"#;
        let event = TraeStreamParser::parse_line(json).expect("minimal user event must parse");
        assert!(matches!(event, TraeStreamEvent::User { .. }));
    }
}
