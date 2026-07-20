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
