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
/// (status, ping, etc.) are captured by the `Other` variant.
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

    /// Assistant message events containing text deltas or tool calls.
    Assistant {
        message: TraeAssistantMessage,
    },

    /// User message events (e.g., tool results).
    User {
        message: TraeUserMessage,
    },

    /// Final result event.
    Result {
        #[serde(rename = "duration_ms")]
        duration_ms: u64,
        #[serde(rename = "is_error")]
        is_error: bool,
        #[serde(flatten)]
        extra: serde_json::Value,
    },

    /// All other events (ping, status updates, etc.)
    #[serde(other)]
    Other,
}

/// Assistant message within an assistant event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraeAssistantMessage {
    /// Text content delta.
    Text {
        content: TraeTextContent,
    },
    /// Tool use / tool call.
    ToolUse {
        content: TraeToolUseContent,
    },
    /// All other message types
    #[serde(other)]
    Other,
}

/// Text content block within an assistant message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraeTextContent {
    pub text: String,
}

/// Tool use content block within an assistant message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraeToolUseContent {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// User message (e.g., tool result returned to the assistant).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraeUserMessage {
    /// Tool result content.
    ToolResult {
        content: Vec<TraeToolResultContent>,
    },
    /// All other user message types.
    #[serde(other)]
    Other,
}

/// Content block within a tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraeToolResultContent {
    Text {
        text: String,
    },
    #[serde(other)]
    Other,
}

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
        TraeStreamEvent::Assistant { message } => match message {
            TraeAssistantMessage::Text { content } => {
                handler.on_text(&content.text);
                extracted_text.push_str(&content.text);
            }
            TraeAssistantMessage::ToolUse { content } => {
                handler.on_tool_call(&content.name, &content.id, &content.input);
            }
            TraeAssistantMessage::Other => {}
        },
        TraeStreamEvent::User { message } => match message {
            TraeUserMessage::ToolResult { content } => {
                let output = content
                    .iter()
                    .filter_map(|b| match b {
                        TraeToolResultContent::Text { text } => Some(text.as_str()),
                        TraeToolResultContent::Other => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                // We don't have the tool call ID here, so we emit it as a text result
                // The ID association is handled by the executor
                if !output.is_empty() {
                    handler.on_text(&output);
                    extracted_text.push_str(&output);
                }
            }
            TraeUserMessage::Other => {}
        },
        TraeStreamEvent::Result { duration_ms, is_error, .. } => {
            state.duration_ms = duration_ms;
            state.is_error = is_error;
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
            TraeStreamEvent::System { subtype, session_id, .. } => {
                assert_eq!(subtype, "init");
                assert_eq!(session_id, Some("sess_123".to_string()));
            }
            _ => panic!("Expected System event, got {:?}", event),
        }
    }

    #[test]
    fn test_parse_assistant_text_event() {
        let json = r#"{"type":"assistant","message":{"type":"text","content":{"text":"Hello world"}}}"#;
        let event = TraeStreamParser::parse_line(json).unwrap();

        match event {
            TraeStreamEvent::Assistant { message } => match message {
                TraeAssistantMessage::Text { content } => {
                    assert_eq!(content.text, "Hello world");
                }
                _ => panic!("Expected Text message"),
            },
            _ => panic!("Expected Assistant event"),
        }
    }

    #[test]
    fn test_parse_assistant_tool_use_event() {
        let json = r#"{"type":"assistant","message":{"type":"tool_use","content":{"id":"toolu_001","name":"Bash","input":{"command":"echo hi"}}}}"#;
        let event = TraeStreamParser::parse_line(json).unwrap();

        match event {
            TraeStreamEvent::Assistant { message } => match message {
                TraeAssistantMessage::ToolUse { content } => {
                    assert_eq!(content.id, "toolu_001");
                    assert_eq!(content.name, "Bash");
                    assert_eq!(content.input["command"], "echo hi");
                }
                _ => panic!("Expected ToolUse message"),
            },
            _ => panic!("Expected Assistant event"),
        }
    }

    #[test]
    fn test_parse_user_tool_result_event() {
        let json = r#"{"type":"user","message":{"type":"tool_result","content":[{"type":"text","text":"done"}]}}"#;
        let event = TraeStreamParser::parse_line(json).unwrap();

        match event {
            TraeStreamEvent::User { message } => match message {
                TraeUserMessage::ToolResult { content } => {
                    assert_eq!(content.len(), 1);
                    match &content[0] {
                        TraeToolResultContent::Text { text } => {
                            assert_eq!(text, "done");
                        }
                        _ => panic!("Expected Text content"),
                    }
                }
                _ => panic!("Expected ToolResult message"),
            },
            _ => panic!("Expected User event"),
        }
    }

    #[test]
    fn test_parse_result_event() {
        let json = r#"{"type":"result","duration_ms":1234,"is_error":false}"#;
        let event = TraeStreamParser::parse_line(json).unwrap();

        match event {
            TraeStreamEvent::Result { duration_ms, is_error, .. } => {
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
            message: TraeAssistantMessage::Text {
                content: TraeTextContent {
                    text: "Hello".to_string(),
                },
            },
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
            message: TraeAssistantMessage::ToolUse {
                content: TraeToolUseContent {
                    id: "toolu_001".to_string(),
                    name: "Bash".to_string(),
                    input: json!({"command": "echo hi"}),
                },
            },
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
            message: TraeUserMessage::ToolResult {
                content: vec![TraeToolResultContent::Text {
                    text: "file contents".to_string(),
                }],
            },
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
            message: TraeAssistantMessage::Text {
                content: TraeTextContent {
                    text: "Hello ".to_string(),
                },
            },
        };
        let event2 = TraeStreamEvent::Assistant {
            message: TraeAssistantMessage::Text {
                content: TraeTextContent {
                    text: "world".to_string(),
                },
            },
        };

        dispatch_trae_stream_event(event1, &mut handler, &mut extracted, &mut state);
        dispatch_trae_stream_event(event2, &mut handler, &mut extracted, &mut state);

        assert_eq!(handler.texts, vec!["Hello ", "world"]);
        assert_eq!(extracted, "Hello world");
    }
}
