use super::*;

#[cfg(test)]
pub fn detect_solo_output_completion(
    registry: &ralph_core::HatRegistry,
    output: &str,
    completion_promise: &str,
) -> bool {
    registry.is_empty() && EventParser::contains_promise(output, completion_promise)
}

pub fn normalize_cli_output_for_parsing(
    output_format: BackendOutputFormat,
    raw_output: &str,
) -> String {
    match output_format {
        BackendOutputFormat::StreamJson => extract_claude_stream_text(raw_output),
        BackendOutputFormat::PiStreamJson => extract_pi_stream_text(raw_output),
        BackendOutputFormat::TraeStreamJson => extract_trae_stream_text(raw_output),
        BackendOutputFormat::AgentStreamJson => extract_agent_stream_text(raw_output),
        _ => raw_output.to_string(),
    }
}

pub fn extract_claude_stream_text(raw_output: &str) -> String {
    let mut extracted = String::new();

    for line in raw_output.lines() {
        let Some(event) = ClaudeStreamParser::parse_line(line) else {
            continue;
        };

        if let ClaudeStreamEvent::Assistant { message, .. } = event {
            for block in message.content {
                if let ContentBlock::Text { text } = block {
                    extracted.push_str(&text);
                    extracted.push('\n');
                }
            }
        }
    }

    if extracted.is_empty() {
        raw_output.to_string()
    } else {
        extracted
    }
}

pub fn extract_pi_stream_text(raw_output: &str) -> String {
    let mut extracted = String::new();

    for line in raw_output.lines() {
        let Some(event) = PiStreamParser::parse_line(line) else {
            continue;
        };

        if let PiStreamEvent::MessageUpdate {
            assistant_message_event,
        } = event
            && let PiAssistantEvent::TextDelta { delta } = assistant_message_event
        {
            extracted.push_str(&delta);
        }
    }

    if extracted.is_empty() {
        raw_output.to_string()
    } else {
        extracted
    }
}

pub fn extract_trae_stream_text(raw_output: &str) -> String {
    use ralph_adapters::{TraeStreamEvent, TraeStreamParser, extract_assistant_text};

    let mut extracted = String::new();

    for line in raw_output.lines() {
        let Some(event) = TraeStreamParser::parse_line(line) else {
            continue;
        };

        if let TraeStreamEvent::Assistant { message } = event
            && let Some(text) = extract_assistant_text(&message)
        {
            extracted.push_str(&text);
            extracted.push('\n');
        }
    }

    if extracted.is_empty() {
        raw_output.to_string()
    } else {
        extracted
    }
}

/// Extract assistant text from a raw Cursor `agent` stream-json buffer.
///
/// Reuses `AgentStreamParser::extract_all_text` from the adapters crate —
/// same fall-back contract as `extract_trae_stream_text` (assistant text →
/// `result.result` → raw buffer). Unit 4 completes S5 with concrete tests
/// around this entry point.
pub fn extract_agent_stream_text(raw_output: &str) -> String {
    use ralph_adapters::AgentStreamParser;
    AgentStreamParser::extract_all_text(raw_output)
}

#[cfg(test)]
mod agent_stream_extraction_tests {
    //! Unit 4 (S5) — `normalize_cli_output_for_parsing` for `AgentStreamJson`
    //! must extract assistant text (or fall back to the `result.result` field)
    //! instead of leaking raw NDJSON envelopes into completion detection.

    use super::*;
    // `BackendOutputFormat` is `ralph_adapters::OutputFormat` re-exported
    // by `super::*` (via `loop_runner/mod.rs`'s `use ... as BackendOutputFormat`).

    #[test]
    fn normalize_agent_stream_extracts_assistant_text_only() {
        // Real Cursor `agent` NDJSON (per R7 / agent_stream fixture shape).
        let raw = concat!(
            r#"{"type":"system","subtype":"init","session_id":"s1"}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello from cursor"}]}}"#,
            "\n",
            r#"{"type":"tool_call","subtype":"started","call_id":"c1","tool_name":"readToolCall","args":{"path":"/etc/hostname"}}"#,
            "\n",
            r#"{"type":"tool_call","subtype":"completed","call_id":"c1","tool_name":"readToolCall","result":"myhost"}}"#,
            "\n",
        );
        let out = normalize_cli_output_for_parsing(BackendOutputFormat::AgentStreamJson, raw);
        // Tool NDJSON lines must NOT bleed into the assistant text output.
        assert_eq!(out, "hello from cursor\n");
        assert!(!out.contains("tool_call"));
        assert!(!out.contains("\"type\""));
    }

    #[test]
    fn normalize_agent_stream_falls_back_to_result_field() {
        // When the assistant only emits tool calls (no text), the final
        // `result.result` is the source of truth — same contract as Trae.
        let raw = concat!(
            r#"{"type":"system","subtype":"init"}"#,
            "\n",
            r#"{"type":"tool_call","subtype":"started","call_id":"c1","tool_name":"writeToolCall","args":{}}"#,
            "\n",
            r#"{"type":"result","subtype":"success","result":"final summary","is_error":false}"#,
            "\n",
        );
        let out = normalize_cli_output_for_parsing(BackendOutputFormat::AgentStreamJson, raw);
        assert_eq!(out, "final summary");
    }

    #[test]
    fn extract_agent_stream_text_returns_raw_when_no_events() {
        // No assistant events and no result event: preserve raw output
        // (debug visibility, parallel to TraeStreamParser::extract_all_text).
        let raw = "{\"type\":\"system\"}\n{\"type\":\"ping\"}\n";
        let out = extract_agent_stream_text(raw);
        assert_eq!(out, raw);
    }
}
