use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use ralph_adapters::{
    CopilotStreamParser, OutputFormat as BackendOutputFormat, PiAssistantEvent, PiContentBlock,
    PiStreamEvent, PiStreamParser, TraeStreamEvent, TraeStreamParser, extract_assistant_text,
    extract_assistant_tool_calls, extract_user_tool_result_text, user_is_tool_result,
};
use ratatui::text::Line;

/// Push a styled line to a TUI wave worker's output buffer.
pub fn push_to_wave_worker_buffer(
    state: &Arc<std::sync::Mutex<ralph_tui::TuiState>>,
    worker_idx: usize,
    lines: &[Line<'static>],
) {
    let Ok(s) = state.lock() else { return };
    let Some(ref wave) = s.wave_active else {
        return;
    };
    let Some(buffer) = wave.worker_buffers.get(worker_idx) else {
        return;
    };
    let handle = buffer.lines_handle();
    let Ok(mut buf_lines) = handle.lock() else {
        return;
    };
    buf_lines.extend_from_slice(lines);
}
/// Push a line to the latest iteration's output in the TUI.
pub fn push_to_tui_iteration(
    state: &Arc<std::sync::Mutex<ralph_tui::TuiState>>,
    line: Line<'static>,
) {
    let Ok(s) = state.lock() else { return };
    let Some(handle) = s.latest_iteration_lines_handle() else {
        return;
    };
    let Ok(mut lines) = handle.lock() else { return };
    lines.push(line);
}
pub fn truncate_wave_worker_preview(text: &str) -> String {
    if text.len() > 200 {
        let end = ralph_core::floor_char_boundary(text, 200);
        format!("{}…", &text[..end])
    } else {
        text.to_string()
    }
}
/// Extract a human-readable text delta from a single stdout line.
pub fn extract_readable_delta(line: &str, output_format: BackendOutputFormat) -> Option<String> {
    match output_format {
        BackendOutputFormat::Text | BackendOutputFormat::Acp => Some(format!("{line}\n")),
        BackendOutputFormat::StreamJson => {
            use ralph_adapters::{ClaudeStreamEvent, ClaudeStreamParser, ContentBlock};
            match ClaudeStreamParser::parse_line(line) {
                Some(ClaudeStreamEvent::Assistant { message, .. }) => {
                    let mut text = String::new();
                    for block in message.content {
                        match block {
                            ContentBlock::Text { text: t } => {
                                text.push_str(&t);
                                text.push('\n');
                            }
                            ContentBlock::Thinking { thinking, .. } => {
                                text.push_str(&thinking);
                                text.push('\n');
                            }
                            ContentBlock::ToolUse { name, input, .. } => {
                                text.push_str(&format!("⚙ {name}({input})\n"));
                            }
                        }
                    }
                    if text.is_empty() { None } else { Some(text) }
                }
                Some(ClaudeStreamEvent::User { message }) => {
                    let mut text = String::new();
                    for block in message.content {
                        let ralph_adapters::UserContentBlock::ToolResult { content, .. } = block;
                        if !content.is_empty() {
                            text.push_str(&format!(
                                "→ {}\n",
                                truncate_wave_worker_preview(&content)
                            ));
                        }
                    }
                    if text.is_empty() { None } else { Some(text) }
                }
                _ => None,
            }
        }
        BackendOutputFormat::CopilotStreamJson => {
            CopilotStreamParser::extract_text(line).map(|text| {
                if text.ends_with('\n') {
                    text
                } else {
                    format!("{text}\n")
                }
            })
        }
        BackendOutputFormat::PiStreamJson => match PiStreamParser::parse_line(line) {
            Some(PiStreamEvent::MessageUpdate {
                assistant_message_event: PiAssistantEvent::TextDelta { delta },
            }) => Some(delta),
            Some(PiStreamEvent::MessageUpdate {
                assistant_message_event: PiAssistantEvent::Error { reason },
            }) => Some(format!("✗ {}\n", truncate_wave_worker_preview(&reason))),
            Some(PiStreamEvent::ToolExecutionStart {
                tool_name, args, ..
            }) => Some(format!("⚙ {tool_name}({args})\n")),
            Some(PiStreamEvent::ToolExecutionEnd {
                result, is_error, ..
            }) => {
                let output = result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        PiContentBlock::Text { text } => Some(text.as_str()),
                        PiContentBlock::Other => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if output.is_empty() {
                    None
                } else if is_error {
                    Some(format!("✗ {}\n", truncate_wave_worker_preview(&output)))
                } else {
                    Some(format!("→ {}\n", truncate_wave_worker_preview(&output)))
                }
            }
            _ => None,
        },
        // Parse trae NDJSON: extract assistant text, tool calls, and tool results
        // for the wave worker preview pane (mirrors pi/copilot patterns above).
        BackendOutputFormat::TraeStreamJson => match TraeStreamParser::parse_line(line) {
            Some(TraeStreamEvent::Assistant { message }) => {
                if let Some(text) = extract_assistant_text(&message) {
                    let text = if text.ends_with('\n') {
                        text
                    } else {
                        format!("{}\n", text)
                    };
                    return Some(text);
                }
                let calls = extract_assistant_tool_calls(&message);
                if let Some(call) = calls.into_iter().next() {
                    let args_display = if call.function.arguments.is_empty() {
                        String::new()
                    } else {
                        truncate_wave_worker_preview(&call.function.arguments)
                    };
                    return Some(format!("⚙ {}({})\n", call.function.name, args_display));
                }
                None
            }
            Some(TraeStreamEvent::User {
                subtype,
                tool_use_id,
                content,
                ..
            }) => {
                if user_is_tool_result(subtype.as_deref(), tool_use_id.as_deref(), &content) {
                    if let Some(output) = extract_user_tool_result_text(&content) {
                        if !output.is_empty() {
                            return Some(format!("→ {}\n", truncate_wave_worker_preview(&output)));
                        }
                    }
                }
                None
            }
            _ => None,
        },
    }
}
/// Read events from a per-worker events file.
pub fn read_worker_events(path: &Path) -> Vec<ralph_core::Event> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };

    content
        .lines()
        .filter_map(|line| serde_json::from_str::<ralph_core::Event>(line).ok())
        .collect()
}
pub fn read_worker_events_with_retry(path: &Path, timeout: Duration) -> Vec<ralph_core::Event> {
    let start = std::time::Instant::now();
    loop {
        let events = read_worker_events(path);
        if !events.is_empty() || start.elapsed() >= timeout {
            return events;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
/// Merge wave result events into the main events file.
///
/// Appends all result events to the main JSONL file so the aggregator hat
/// picks them up on the next iteration.
///
/// 2026-06-07 plan Unit 3 (R8): every record this function writes MUST
/// carry `wave_id`, `wave_index`, `wave_total` and a `ts` field.  The
/// worker process is forbidden from writing to the main events file
/// (its `RALPH_EVENTS_FILE` env var points at a per-worker file), so
/// any record missing these fields is a bypass attempt or a stale
/// hand-written file from a historical run.
pub fn merge_wave_results_to_events_file(
    completed: &ralph_core::CompletedWave,
    events_file: &Path,
    publish_topics: &[String],
) -> Result<()> {
    use std::io::Write;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_file)
        .with_context(|| format!("Failed to open events file: {}", events_file.display()))?;

    let ts = chrono::Utc::now().to_rfc3339();

    // Build all records into a single buffer, then write_all once for atomic
    // append (consistent with EventLogger::log and write_wave_events).
    let mut buf = String::new();

    let mut merged_indexes: Vec<u32> = Vec::new();
    let mut duplicate_indexes: Vec<u32> = Vec::new();

    for result in &completed.results {
        if merged_indexes.contains(&result.index) {
            duplicate_indexes.push(result.index);
        } else {
            merged_indexes.push(result.index);
        }
        for event in &result.events {
            let record = serde_json::json!({
                "topic": event.topic.as_str(),
                "payload": event.payload,
                "ts": ts,
                "wave_id": completed.wave_id,
                "wave_index": result.index,
                "wave_total": completed.wave_total,
            });
            buf.push_str(&serde_json::to_string(&record)?);
            buf.push('\n');
        }
    }

    // Also write failure events so the aggregator knows about partial results
    for failure in &completed.failures {
        let record = serde_json::json!({
            "topic": "wave.worker.failed",
            "payload": format!("Worker {} failed: {}", failure.index, failure.error),
            "ts": ts,
            "wave_id": completed.wave_id,
            "wave_index": failure.index,
            "wave_total": completed.wave_total,
        });
        buf.push_str(&serde_json::to_string(&record)?);
        buf.push('\n');

        // Emit synthetic events on the hat's publish topics so downstream
        // aggregators can still trigger even when workers fail/timeout
        for topic in publish_topics {
            let record = serde_json::json!({
                "topic": topic,
                "payload": format!(
                    "## Worker {} (FAILED)\n\nError: {}",
                    failure.index, failure.error
                ),
                "ts": ts,
                "wave_id": completed.wave_id,
                "wave_index": failure.index,
                "wave_total": completed.wave_total,
            });
            buf.push_str(&serde_json::to_string(&record)?);
            buf.push('\n');
        }
    }

    file.write_all(buf.as_bytes())?;

    // R8 observability: log expected/merged/missing/duplicate indexes so a
    // postmortem can tell at a glance whether the wave was complete.
    let expected_indexes: std::collections::BTreeSet<u32> =
        (0..completed.wave_total).collect();
    let merged_set: std::collections::BTreeSet<u32> = merged_indexes.iter().copied().collect();
    let missing_indexes: Vec<u32> =
        expected_indexes.difference(&merged_set).copied().collect();
    let failure_indexes: Vec<u32> = completed.failures.iter().map(|f| f.index).collect();

    if !missing_indexes.is_empty() || !duplicate_indexes.is_empty() {
        tracing::warn!(
            wave_id = %completed.wave_id,
            wave_total = completed.wave_total,
            merged = merged_indexes.len(),
            missing = ?missing_indexes,
            duplicate = ?duplicate_indexes,
            failures = ?failure_indexes,
            "Wave merge produced incomplete or duplicate index set"
        );
    } else {
        tracing::info!(
            wave_id = %completed.wave_id,
            wave_total = completed.wave_total,
            merged = merged_indexes.len(),
            "Wave merge complete with all expected indexes present"
        );
    }

    Ok(())
}
