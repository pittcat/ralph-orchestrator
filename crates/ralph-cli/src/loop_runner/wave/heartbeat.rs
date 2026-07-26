//! 2026-07-25-006 plan U4: single-line stdout heartbeat classifier.
//!
//! Maps one `[line]` of NDJSON (or plain text) coming off a wave worker's
//! PTY into a [`HeartbeatKind`]:
//!
//! - `Strong`: Claude `ToolUse` / `ToolResult`, Pi
//!   `ToolExecutionStart` / `ToolExecutionEnd`, Cursor `ToolCall`,
//!   Trae `tool_use`. These represent real agent-side progress the
//!   worker could not produce without producing the event.
//! - `Weak`: assistant `Text` block, `Thinking` block, Pi `TextDelta`,
//!   Cursor assistant text. Useful for the lease window — the model
//!   is still streaming — but, per `HeartbeatLease`, weak signals only
//!   refresh the lease up to `idle_weak_signal_cap` consecutive times.
//! - `None`: blank line, malformed JSON, unknown shape. Never refreshes
//!   the lease.
//!
//! The classifier is a pure function. It does not touch the worker, the
//! events file, the kill switch, or any timing state; the [`super::worker`]
//! loop owns those concerns and consults this classifier on every line.
//! Keeping the classifier pure means the same table-driven suite can pin
//! every backend's behavior without spinning up a real PTY (`tests.rs`
//! at the bottom of this file).
//!
//! Why the indirection? `extract_readable_delta` ([`super::io`]) already
//! classifies lines for the TUI preview pane, but its return type is
//! `Option<String>` (rendered text). The heartbeat lease needs a richer
//! typed signal (Strong/Weak/None) without paying for the String
//! allocation on every line, so this module exists alongside it.
use ralph_adapters::{
    AgentStreamEvent, AgentStreamParser, ClaudeStreamEvent, ClaudeStreamParser, ContentBlock,
    OutputFormat, PiAssistantEvent, PiStreamEvent, PiStreamParser, TraeStreamEvent,
    TraeStreamParser,
};

/// Outcome of classifying a single stdout line.
///
/// `Display` is implemented so the worker can produce a stable, grep-able
/// `heartbeat=<kind>` log line without each call site inventing its own
/// spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum HeartbeatKind {
    /// Tool lifecycle / external IO event that proves the worker is
    /// making real progress (Claude `ToolUse`/`ToolResult`, Pi
    /// `ToolExecutionStart`/`ToolExecutionEnd`, Cursor `ToolCall`).
    /// Refreshes the lease and resets the weak-signal counter.
    Strong,
    /// Assistant text / thinking / `TextDelta`. Streams but does not
    /// externalize IO. Refreshes the lease only up to
    /// `idle_weak_signal_cap` consecutive uses; cap exceeded → next line
    /// either refreshes again (Strong) or trips idle kill (None / cap
    /// exceeded).
    Weak,
    /// Blank line, malformed JSON, or a backend event type that is not
    /// one of the recognised progress shapes. Does not refresh the lease.
    None,
}

impl std::fmt::Display for HeartbeatKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeartbeatKind::Strong => f.write_str("strong"),
            HeartbeatKind::Weak => f.write_str("weak"),
            HeartbeatKind::None => f.write_str("none"),
        }
    }
}

/// Pure-function classifier. See module docs for the Strong/Weak
/// mapping. Returns `HeartbeatKind::None` for blank, malformed, or
/// unrecognised lines (so the caller never has to reason about the
/// distinction).
pub fn classify_heartbeat_line(line: &str, format: OutputFormat) -> HeartbeatKind {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return HeartbeatKind::None;
    }
    // Plain text backends (e.g. --output-format text) do not emit
    // structured events at all. Every line is "the model is still
    // talking" — classify as Weak so the lease refreshes under the
    // weak-cap.
    if matches!(format, OutputFormat::Text) {
        return HeartbeatKind::Weak;
    }
    match format {
        OutputFormat::StreamJson => classify_claude(trimmed),
        OutputFormat::PiStreamJson => classify_pi(trimmed),
        OutputFormat::AgentStreamJson => classify_cursor(trimmed),
        OutputFormat::TraeStreamJson => classify_trae(trimmed),
        // Defensive: any future variant defaults to None rather than
        // guessing. The plan scope is the four known backends.
        OutputFormat::Text => HeartbeatKind::Weak,
    }
}

fn classify_claude(line: &str) -> HeartbeatKind {
    match ClaudeStreamParser::parse_line(line) {
        Some(ClaudeStreamEvent::Assistant { message, .. }) => {
            for block in &message.content {
                match block {
                    ContentBlock::ToolUse { .. } => return HeartbeatKind::Strong,
                    ContentBlock::Text { .. } | ContentBlock::Thinking { .. } => {
                        // Keep scanning — a single Assistant event can
                        // contain multiple blocks; a tool-use among
                        // text blocks still classifies as Strong.
                    }
                }
            }
            // Assistant event with no recognised blocks (e.g.
            // future-only shapes) does not refresh the lease.
            classify_claude_assistant_fallback(line)
        }
        Some(ClaudeStreamEvent::User { message }) => {
            // Assistant tool_result blocks back in the User channel —
            // those are real IO completions.
            for block in &message.content {
                if matches!(block, ralph_adapters::UserContentBlock::ToolResult { .. }) {
                    return HeartbeatKind::Strong;
                }
            }
            HeartbeatKind::None
        }
        Some(_) => HeartbeatKind::None,
        None => HeartbeatKind::None,
    }
}

/// If the Assistant event had no `Text` / `Thinking` / `ToolUse` block,
/// treat it as Weak: the model emitted SOMETHING that landed on the
/// stream (e.g. a stop_reason-only delta). Keeping it Weak avoids the
/// `Strong → IdleKill → resume Strong → IdleKill` oscillation when
/// only the wire envelope is observable.
fn classify_claude_assistant_fallback(_line: &str) -> HeartbeatKind {
    HeartbeatKind::Weak
}

fn classify_pi(line: &str) -> HeartbeatKind {
    match PiStreamParser::parse_line(line) {
        Some(PiStreamEvent::MessageUpdate { assistant_message_event }) => match assistant_message_event {
            // Text + extended-thinking deltas count as Weak (per R5).
            // The model is still streaming progress; the lease just
            // only refreshes up to `idle_weak_signal_cap` in a row.
            PiAssistantEvent::TextDelta { .. } | PiAssistantEvent::ThinkingDelta { .. } => {
                HeartbeatKind::Weak
            }
            // Error has no progress signal value.
            PiAssistantEvent::Error { .. } => HeartbeatKind::None,
            // Any other assistant-message sub-event (`text_start`,
            // `text_end`, `toolcall_*`, `done`, future-only shapes)
            // is captured by `#[serde(other)] Other` and carries no
            // lease signal.
            _ => HeartbeatKind::None,
        },
        Some(PiStreamEvent::ToolExecutionStart { .. })
        | Some(PiStreamEvent::ToolExecutionEnd { .. }) => HeartbeatKind::Strong,
        // `turn_end` and the catch-all `Other` cover session / model
        // info / available-commands / message boundary / future-only
        // frames. None of these prove external IO — no progress
        // signal for the lease.
        Some(_) => HeartbeatKind::None,
        None => HeartbeatKind::None,
    }
}

fn classify_cursor(line: &str) -> HeartbeatKind {
    match AgentStreamParser::parse_line(line) {
        Some(AgentStreamEvent::ToolCall { .. }) => HeartbeatKind::Strong,
        Some(AgentStreamEvent::Assistant { .. }) => HeartbeatKind::Weak,
        // Result / system / Other carry no live-progress signal.
        Some(_) => HeartbeatKind::None,
        None => HeartbeatKind::None,
    }
}

fn classify_trae(line: &str) -> HeartbeatKind {
    match TraeStreamParser::parse_line(line) {
        Some(TraeStreamEvent::Assistant { .. }) => {
            // Trae's assistant message carries both text and tool_use
            // blocks; only tool_use is a strong signal. Re-parse the
            // payload by walking the JSON so we do not double-borrow
            // the parser. Both flavors reduce to None / Weak / Strong.
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|v| {
                    v.get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_array())
                        .cloned()
                })
                .map(|blocks| {
                    let mut has_tool = false;
                    let mut has_text = false;
                    for block in &blocks {
                        let kind = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        if kind == "tool_use" {
                            has_tool = true;
                        } else if kind == "text" {
                            has_text = true;
                        }
                    }
                    if has_tool {
                        HeartbeatKind::Strong
                    } else if has_text {
                        HeartbeatKind::Weak
                    } else {
                        HeartbeatKind::None
                    }
                })
                .unwrap_or(HeartbeatKind::None)
        }
        // Trae's user channel carries tool_result blocks → Strong.
        Some(TraeStreamEvent::User { .. }) => serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .cloned()
            })
            .map(|blocks| {
                let mut has_tool_result = false;
                for block in &blocks {
                    let kind = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if kind == "tool_result" {
                        has_tool_result = true;
                    }
                }
                if has_tool_result {
                    HeartbeatKind::Strong
                } else {
                    HeartbeatKind::None
                }
            })
            .unwrap_or(HeartbeatKind::None),
        Some(_) => HeartbeatKind::None,
        None => HeartbeatKind::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────────────────────────────────────────────────────
    // U4 table-driven suite — covers every backend's Strong/Weak/None
    // surfaces plus malformed / blank / future-only inputs. The plan
    // calls for "every backend × {2 strong, 2 weak, 1 none}".
    // ─────────────────────────────────────────────────────────────────

    // ---- Plain-text fallback: any non-blank line is Weak. ----
    #[test]
    fn text_format_any_line_is_weak() {
        assert_eq!(
            classify_heartbeat_line("hello world", OutputFormat::Text),
            HeartbeatKind::Weak
        );
        assert_eq!(
            classify_heartbeat_line("", OutputFormat::Text),
            HeartbeatKind::None
        );
        assert_eq!(
            classify_heartbeat_line("   \t  ", OutputFormat::Text),
            HeartbeatKind::None
        );
    }

    // ---- Claude StreamJson. ----
    #[test]
    fn claude_tool_use_is_strong() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file":"/a"}}]}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::StreamJson),
            HeartbeatKind::Strong
        );
    }

    #[test]
    fn claude_tool_result_user_is_strong() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::StreamJson),
            HeartbeatKind::Strong
        );
    }

    #[test]
    fn claude_text_is_weak() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::StreamJson),
            HeartbeatKind::Weak
        );
    }

    #[test]
    fn claude_thinking_is_weak() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"ponder"}]}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::StreamJson),
            HeartbeatKind::Weak
        );
    }

    #[test]
    fn claude_unknown_assistant_payload_is_none() {
        // A future-only block shape (anything other than
        // `Text` / `Thinking` / `ToolUse`) is not in
        // `ContentBlock`'s serde schema, so the parser drops the
        // whole event and the classifier returns `None`. The
        // Weak-fallback only fires when the assistant event
        // parsed cleanly but the per-block scan produced nothing
        // recognised — which Claude's strict wire protocol never
        // produces today, but we still keep the fallback for
        // forward-compat.
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"unknown_future_block","data":1}]}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::StreamJson),
            HeartbeatKind::None
        );
    }

    #[test]
    fn claude_unknown_shape_is_none() {
        // Non-assistant / non-user events (e.g. `message_start`,
        // `message_delta`, `message_stop`, or future-only `type` tags).
        assert_eq!(
            classify_heartbeat_line(
                r#"{"type":"message_stop"}"#,
                OutputFormat::StreamJson
            ),
            HeartbeatKind::None
        );
        assert_eq!(
            classify_heartbeat_line(
                r#"{"type":"ping","ts":1}"#,
                OutputFormat::StreamJson
            ),
            HeartbeatKind::None
        );
    }

    #[test]
    fn claude_malformed_is_none() {
        // The line is non-blank so we cannot short-circuit on the
        // `trimmed.is_empty()` branch; the parser must produce None.
        assert_eq!(
            classify_heartbeat_line("{not-json", OutputFormat::StreamJson),
            HeartbeatKind::None
        );
    }

    // ---- Pi StreamJson. ----
    #[test]
    fn pi_text_delta_is_weak() {
        let line = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"hi"}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::PiStreamJson),
            HeartbeatKind::Weak
        );
    }

    #[test]
    fn pi_tool_execution_start_is_strong() {
        let line = r#"{"type":"tool_execution_start","toolCallId":"t1","toolName":"read_file","args":{"path":"/a"}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::PiStreamJson),
            HeartbeatKind::Strong
        );
    }

    #[test]
    fn pi_tool_execution_end_is_strong() {
        let line = r#"{"type":"tool_execution_end","toolCallId":"t1","toolName":"read_file","result":{"content":[{"type":"text","text":"ok"}]},"isError":false}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::PiStreamJson),
            HeartbeatKind::Strong
        );
    }

    #[test]
    fn pi_error_is_none() {
        let line = r#"{"type":"message_update","assistantMessageEvent":{"type":"error","reason":"boom"}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::PiStreamJson),
            HeartbeatKind::None
        );
    }

    #[test]
    fn pi_malformed_is_none() {
        assert_eq!(
            classify_heartbeat_line("not json", OutputFormat::PiStreamJson),
            HeartbeatKind::None
        );
    }

    // ---- Cursor AgentStreamJson. ----
    #[test]
    fn cursor_tool_call_is_strong() {
        let line = r#"{"type":"tool_call","subtype":"started","call_id":"c1","tool_call":{"readToolCall":{"args":{"file":"/a"}}}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::AgentStreamJson),
            HeartbeatKind::Strong
        );
    }

    #[test]
    fn cursor_assistant_text_is_weak() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::AgentStreamJson),
            HeartbeatKind::Weak
        );
    }

    #[test]
    fn cursor_result_is_none() {
        let line = r#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::AgentStreamJson),
            HeartbeatKind::None
        );
    }

    #[test]
    fn cursor_unknown_event_is_none() {
        let line = r#"{"type":"ping","ts":1700000000}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::AgentStreamJson),
            HeartbeatKind::None
        );
    }

    #[test]
    fn cursor_malformed_is_none() {
        assert_eq!(
            classify_heartbeat_line(
                "{garbage",
                OutputFormat::AgentStreamJson
            ),
            HeartbeatKind::None
        );
    }

    // ---- Trae StreamJson. ----
    #[test]
    fn trae_tool_use_is_strong() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"path":"/a"}}]}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::TraeStreamJson),
            HeartbeatKind::Strong
        );
    }

    #[test]
    fn trae_text_is_weak() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::TraeStreamJson),
            HeartbeatKind::Weak
        );
    }

    #[test]
    fn trae_tool_result_is_strong() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::TraeStreamJson),
            HeartbeatKind::Strong
        );
    }

    #[test]
    fn trae_unknown_event_is_none() {
        let line = r#"{"type":"message_start"}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::TraeStreamJson),
            HeartbeatKind::None
        );
    }

    #[test]
    fn trae_malformed_is_none() {
        assert_eq!(
            classify_heartbeat_line("not json", OutputFormat::TraeStreamJson),
            HeartbeatKind::None
        );
    }

    #[test]
    fn trae_assistant_with_no_blocks_is_none() {
        // `content` is empty / missing so there is nothing to classify.
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[]}}"#;
        assert_eq!(
            classify_heartbeat_line(line, OutputFormat::TraeStreamJson),
            HeartbeatKind::None
        );
    }

    // ---- Display for greppable reason strings (used in U9 wiring). ----
    #[test]
    fn display_lowercase_token_is_stable() {
        assert_eq!(HeartbeatKind::Strong.to_string(), "strong");
        assert_eq!(HeartbeatKind::Weak.to_string(), "weak");
        assert_eq!(HeartbeatKind::None.to_string(), "none");
    }
}
