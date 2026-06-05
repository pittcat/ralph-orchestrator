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
        BackendOutputFormat::CopilotStreamJson => CopilotStreamParser::extract_all_text(raw_output),
        BackendOutputFormat::PiStreamJson => extract_pi_stream_text(raw_output),
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
