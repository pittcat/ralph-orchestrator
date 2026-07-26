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

// =====================================================================
// 2026-07-25-006 U5: pure lease decision function.
// =====================================================================

/// Configuration snapshot fed into the lease decision. All timing values
/// use whole milliseconds relative to the same epoch (the worker's spawn
/// instant) so the decision is deterministic and trivially testable
/// without a real clock.
///
/// `idle_enabled` is a pre-resolved flag (i.e. `idle_window_secs > 0`);
/// the worker feeds `false` here when the YAML field is absent or `0`
/// so the decision treats idle as "not in this loop" and only enforces
/// the hard ceiling. This mirrors `DetectedWave::idle_enabled()`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseConfig {
    /// Hard ceiling (StartToClose). The worker MUST NOT survive past
    /// `now_ms - start_ms >= hard_cap_ms`. Required.
    pub hard_cap_ms: u64,
    /// Idle window (`Some(ms)` when idle mode is enabled; `None` /
    /// disabled disables the idle branch entirely).
    pub idle_window_ms: Option<u64>,
    /// Maximum number of consecutive Weak-only signals that may
    /// refresh the lease before further Weak signals stop refreshing
    /// it. Per KTD3 — cap exhaustion forces `IdleKill` even though
    /// `< idle_window`.
    pub weak_cap: u32,
}

/// Input snapshot for one `decide_lease` call. All values are relative
/// to the same spawn-relative epoch as `LeaseConfig`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseSnapshot {
    /// `now_ms`: monotonic time elapsed since spawn, in milliseconds.
    pub now_ms: u64,
    /// `last_hb_ms`: monotonic time elapsed since spawn at which the
    /// most recent qualifying signal (Strong or Weak under cap) was
    /// observed. May equal `now_ms` if a signal arrived this tick.
    pub last_hb_ms: u64,
    /// Consecutive Weak-only renewals observed since the last Strong
    /// signal. Reset to 0 by Strong. Carried over across the
    /// idle-window boundary so the cap is honored even after a long
    /// stream of Weak-only deltas.
    pub weak_count: u32,
    /// What kind of signal was just observed on this tick. The
    /// caller feeds `HeartbeatKind::None` when the tick is just a
    /// timer tick (e.g. while waiting for stdout before any line has
    /// arrived).
    pub kind: HeartbeatKind,
}

/// Outcome of consulting the lease on one tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum LeaseDecision {
    /// Worker may continue. The caller MUST update its `last_hb_ms` /
    /// `weak_count` state to reflect the snapshot the decision saw
    /// (use `apply_lease_decision` below).
    Continue,
    /// Worker exceeded the idle heartbeat window. The caller MUST
    /// tear the worker down with reason `idle heartbeat exceeded`.
    IdleKill,
    /// Worker exceeded the StartToClose hard ceiling. The caller MUST
    /// tear the worker down with reason `start-to-close exceeded`.
    HardKill,
}

impl std::fmt::Display for LeaseDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LeaseDecision::Continue => f.write_str("continue"),
            LeaseDecision::IdleKill => f.write_str("idle_kill"),
            LeaseDecision::HardKill => f.write_str("hard_kill"),
        }
    }
}

/// Pure lease-decision function. Encodes the plan contract:
///
/// - **Hard ceiling wins** (R1 / R6 / KTD8): when
///   `now_ms >= hard_cap_ms`, the only legal answer is `HardKill`,
///   regardless of idle settings.
/// - **Idle disabled**: when `idle_window_ms` is `None`, the function
///   collapses to the legacy StartToClose behavior — `Continue` until
///   the hard ceiling (R7 / KTD2).
/// - **Weak-signal cap** (R3 / KTD3): when idle is enabled and the
///   current signal is `Weak`, increment `weak_count`. If the new
///   count strictly exceeds `weak_cap`, return `IdleKill` even though
///   `now - last_hb < idle_window`. A `Weak` signal at exactly the
///   cap boundary (i.e. `weak_count == weak_cap` after increment)
///   still refreshes — the next Weak tick would trip the kill.
///   Setting `weak_cap == 0` therefore disables weak-renewals outright.
/// - **Strong resets weak_count** implicitly: the caller's
///   `apply_lease_decision` zeros it on `Strong`; this function only
///   reads it.
///
/// The function NEVER mutates state; the caller is responsible for
/// updating `last_hb_ms` and `weak_count` via `apply_lease_decision`
/// after consulting this function on each signal / timer tick.
pub fn decide_lease(cfg: &LeaseConfig, snap: LeaseSnapshot) -> LeaseDecision {
    // (R1) Hard ceiling always wins.
    if snap.now_ms >= cfg.hard_cap_ms {
        return LeaseDecision::HardKill;
    }

    // (R7 / KTD2) Idle disabled → just HardKill (already checked)
    // or Continue. No idle branch at all.
    let Some(idle_window_ms) = cfg.idle_window_ms else {
        return LeaseDecision::Continue;
    };

    // (R3 / KTD3) Weak-cap kicker fires BEFORE the idle-window check
    // so a slow trickle of Weak-only deltas can never extend the
    // lease indefinitely. Count the current signal first to handle
    // the "this tick is Weak" case correctly.
    let next_weak = match snap.kind {
        HeartbeatKind::Weak => snap.weak_count.saturating_add(1),
        // Strong / None do not bump the counter; the caller is
        // expected to reset the counter on Strong via
        // `apply_lease_decision`.
        HeartbeatKind::Strong | HeartbeatKind::None => snap.weak_count,
    };

    // (R3) Hard cap = 0 means "Weak can never refresh the lease".
    // The counter still tracks signal arrival so the worker's
    // diagnostic logs reflect what was observed, but every tick
    // with `Weak` trips IdleKill — even on the very first one.
    if cfg.weak_cap == 0 && snap.kind == HeartbeatKind::Weak {
        return LeaseDecision::IdleKill;
    }

    // (R3) Stricter-than cap: when next_weak is strictly greater
    // than weak_cap, this Weak tick is the one that crosses the
    // boundary — refuse to renew the lease on this tick.
    if cfg.weak_cap > 0 && snap.kind == HeartbeatKind::Weak && next_weak > cfg.weak_cap {
        return LeaseDecision::IdleKill;
    }

    // (R2) Idle window expiration.
    // `last_hb_ms` may be in the future relative to `now_ms` if the
    // caller fast-forwarded a tick; saturate to 0 to avoid panicking
    // on `sub`.
    let since_last_hb = snap.now_ms.saturating_sub(snap.last_hb_ms);
    if since_last_hb >= idle_window_ms {
        return LeaseDecision::IdleKill;
    }

    LeaseDecision::Continue
}

/// State the worker tracks between `decide_lease` calls. Pure data,
/// `Copy`-able, no I/O. The worker mutates a local copy of this on
/// every line / timer tick and feeds the next `LeaseSnapshot` from the
/// updated state. Kept tiny on purpose so the unit suite can pin
/// every transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeaseState {
    pub now_ms: u64,
    pub last_hb_ms: u64,
    pub weak_count: u32,
}

impl LeaseState {
    /// Fresh state for a brand-new worker: clock has not started,
    /// `last_hb_ms == now_ms` (i.e. the spawn IS the first signal —
    /// its weak_count is zero and the lease window starts ticking
    /// from this instant).
    pub const fn fresh(now_ms: u64) -> Self {
        Self {
            now_ms,
            last_hb_ms: now_ms,
            weak_count: 0,
        }
    }

    /// Apply the results of one tick. `kind` is the classification of
    /// the line just observed (or `None` for a pure timer tick).
    /// `now_ms` is the time elapsed since spawn at which this tick
    /// happened. Returns the new state and the lease decision so the
    /// caller can act on it (e.g. forward to kill channel on
    /// `IdleKill`/`HardKill`). Caller MUST stop on `HardKill` and
    /// SHOULD stop on `IdleKill`.
    pub fn tick(&mut self, kind: HeartbeatKind, now_ms: u64, cfg: &LeaseConfig) -> LeaseDecision {
        let snap = LeaseSnapshot {
            now_ms,
            last_hb_ms: self.last_hb_ms,
            weak_count: self.weak_count,
            kind,
        };
        let decision = decide_lease(cfg, snap);
        match (decision, kind) {
            // On HardKill the caller is tearing the worker down.
            // The clock is FROZEN — repeated HardKill ticks at
            // later `now_ms` values MUST NOT advance the visible
            // state, otherwise log timestamps for the kill would
            // drift after the fact. Must be matched BEFORE the
            // kind-specific refresh arms so Strong / Weak arrivals
            // at the cap do not bump last_hb or weak_count.
            (LeaseDecision::HardKill, _) => {}
            // Strong refreshes and resets weak_count regardless of
            // decision (we may be inside an idle window that
            // successfully refreshed).
            (_, HeartbeatKind::Strong) => {
                self.now_ms = now_ms;
                self.last_hb_ms = now_ms;
                self.weak_count = 0;
            }
            // Weak refreshes only if we decided to continue. When
            // the cap ticks over, we refuse to refresh.
            (LeaseDecision::Continue, HeartbeatKind::Weak) => {
                self.now_ms = now_ms;
                self.last_hb_ms = now_ms;
                self.weak_count = self.weak_count.saturating_add(1);
            }
            // Weak on IdleKill: do not advance last_hb or weak_count;
            // the rejection must not reset the idle window, and the
            // counter snapshot used by the caller already reflects
            // the value fed to the decision.
            (LeaseDecision::IdleKill, HeartbeatKind::Weak) => {
                self.now_ms = now_ms;
            }
            // None does not refresh and does not move the weak
            // counter — only the clock moves.
            (_, HeartbeatKind::None) => {
                self.now_ms = now_ms;
            } // (LeaseDecision::Continue, HeartbeatKind::Strong)
              // already covered above by the Strong arm.
              // (LeaseDecision::Continue, HeartbeatKind::None)
              // already covered above by the None arm.
        }
        decision
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
        Some(PiStreamEvent::MessageUpdate {
            assistant_message_event,
        }) => match assistant_message_event {
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
            classify_heartbeat_line(r#"{"type":"message_stop"}"#, OutputFormat::StreamJson),
            HeartbeatKind::None
        );
        assert_eq!(
            classify_heartbeat_line(r#"{"type":"ping","ts":1}"#, OutputFormat::StreamJson),
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
        let line =
            r#"{"type":"message_update","assistantMessageEvent":{"type":"error","reason":"boom"}}"#;
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
            classify_heartbeat_line("{garbage", OutputFormat::AgentStreamJson),
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

    // =====================================================================
    // U5 table-driven suite — pure lease decision.
    // =====================================================================

    fn cfg_idle(hard_cap_ms: u64, idle_window_ms: u64, weak_cap: u32) -> LeaseConfig {
        LeaseConfig {
            hard_cap_ms,
            idle_window_ms: Some(idle_window_ms),
            weak_cap,
        }
    }

    fn cfg_legacy(hard_cap_ms: u64) -> LeaseConfig {
        LeaseConfig {
            hard_cap_ms,
            idle_window_ms: None,
            weak_cap: 0,
        }
    }

    fn snap(now_ms: u64, last_hb_ms: u64, weak_count: u32, kind: HeartbeatKind) -> LeaseSnapshot {
        LeaseSnapshot {
            now_ms,
            last_hb_ms,
            weak_count,
            kind,
        }
    }

    // ---- R7 / KTD2: idle disabled collapses to StartToClose only. ----
    #[test]
    fn lease_idle_disabled_is_legacy_start_to_close() {
        let cfg = cfg_legacy(5_000);
        // Within wall clock → Continue.
        assert_eq!(
            decide_lease(&cfg, snap(1_000, 1_000, 0, HeartbeatKind::None)),
            LeaseDecision::Continue
        );
        // Past wall clock → HardKill regardless of any other state.
        assert_eq!(
            decide_lease(&cfg, snap(5_000, 5_000, 0, HeartbeatKind::None)),
            LeaseDecision::HardKill
        );
        assert_eq!(
            decide_lease(&cfg, snap(99_999, 0, 99, HeartbeatKind::Strong)),
            LeaseDecision::HardKill
        );
    }

    #[test]
    fn lease_idle_disabled_continues_through_long_idle_window() {
        // Idle disabled → a 60-second gap that would otherwise have
        // been an IdleKill under idle mode is fine; only the hard
        // ceiling matters. The hard ceiling itself is large enough
        // (10 minutes) to keep this purely about the idle-disabled
        // behavior.
        let cfg = cfg_legacy(600_000);
        assert_eq!(
            decide_lease(&cfg, snap(60_000, 0, 0, HeartbeatKind::None)),
            LeaseDecision::Continue
        );
    }

    // ---- R1 / R6 / KTD8: hard ceiling always wins. ----
    #[test]
    fn lease_hard_cap_wins_over_idle_continue() {
        // Even with idle enabled and `Weak` refreshing under cap,
        // hitting the hard ceiling kills the worker first.
        let cfg = cfg_idle(1_000, 5_000, 4);
        assert_eq!(
            decide_lease(&cfg, snap(1_000, 1_000, 0, HeartbeatKind::Strong)),
            LeaseDecision::HardKill
        );
        // Strict `==` is in-band per the spec.
        assert_eq!(
            decide_lease(&cfg, snap(999, 999, 0, HeartbeatKind::Strong)),
            LeaseDecision::Continue
        );
    }

    // ---- R2: silent idle expiration when no signal arrives. ----
    #[test]
    fn lease_idle_kill_on_silence() {
        let cfg = cfg_idle(60_000, 2_000, 4);
        // 2s+ since last signal → IdleKill, even on a None tick.
        assert_eq!(
            decide_lease(&cfg, snap(2_500, 500, 0, HeartbeatKind::None)),
            LeaseDecision::IdleKill
        );
        // At the boundary (== idle_window) → in-band IdleKill.
        assert_eq!(
            decide_lease(&cfg, snap(2_000, 0, 0, HeartbeatKind::None)),
            LeaseDecision::IdleKill
        );
        // Just before the boundary → Continue.
        assert_eq!(
            decide_lease(&cfg, snap(1_999, 0, 0, HeartbeatKind::None)),
            LeaseDecision::Continue
        );
    }

    // ---- R3 / KTD3: weak-signal cap exhaustion. ----
    #[test]
    fn lease_weak_cap_exhausted_kicks_idle() {
        // cap = 2 → first two Weak renewals refresh (→ weak_count 1
        // then 2), third Weak crosses the boundary → IdleKill.
        let cfg = cfg_idle(60_000, 5_000, 2);

        // Tick 1: Weak on a fresh state. weak_count becomes 1.
        assert_eq!(
            decide_lease(&cfg, snap(1_000, 0, 0, HeartbeatKind::Weak)),
            LeaseDecision::Continue
        );
        // Tick 2: Weak again, weak_count becomes 2 (= cap).
        assert_eq!(
            decide_lease(&cfg, snap(2_000, 1_000, 1, HeartbeatKind::Weak)),
            LeaseDecision::Continue
        );
        // Tick 3: Weak again, weak_count would become 3 (> cap) → kick.
        assert_eq!(
            decide_lease(&cfg, snap(3_000, 2_000, 2, HeartbeatKind::Weak)),
            LeaseDecision::IdleKill
        );
    }

    #[test]
    fn lease_strong_resets_weak_counter() {
        // Counter at cap, Strong arrives → should NOT kick. After
        // applying the result the state will reset weak_count = 0
        // and last_hb = now (handled by `LeaseState::tick`, exercised
        // by `lease_state_tick_strong_resets` below). The decision
        // function itself just sees the Strong signal and does
        // not increment.
        let cfg = cfg_idle(60_000, 5_000, 2);
        assert_eq!(
            decide_lease(&cfg, snap(2_000, 2_000, 2, HeartbeatKind::Strong)),
            LeaseDecision::Continue
        );
    }

    #[test]
    fn lease_weak_cap_zero_disables_weak_renewal() {
        // weak_cap = 0: even the first Weak does not refresh the
        // lease. The first Strong still refreshes because Strong
        // bumps the counter only via Weak match.
        let cfg = cfg_idle(60_000, 5_000, 0);
        assert_eq!(
            decide_lease(&cfg, snap(1_000, 0, 0, HeartbeatKind::Weak)),
            LeaseDecision::IdleKill
        );
        // Subsequent Strong after a long silence should still be
        // considered via the idle-window branch (it refreshed last_hb
        // before this tick so the decision logic sees a fresh signal).
        assert_eq!(
            decide_lease(&cfg, snap(2_000, 2_000, 0, HeartbeatKind::Strong)),
            LeaseDecision::Continue
        );
    }

    #[test]
    fn lease_none_signal_does_not_increment_weak_count() {
        // None is not Weak; even with the cap primed to the
        // boundary, blank lines / unknown events must not push the
        // counter over. Idle window is exercised independently.
        let cfg = cfg_idle(60_000, 5_000, 2);
        assert_eq!(
            decide_lease(&cfg, snap(1_000, 0, 2, HeartbeatKind::None)),
            LeaseDecision::Continue
        );
    }

    // ---- S5: hard cap still wins over healthy heartbeat. ----
    #[test]
    fn lease_continuous_strong_does_not_pass_hard_cap() {
        // Idle disabled when explicit leg; here we mix idle on with
        // a very small hard cap to prove hard wins. The idle window
        // is set wider than the hard cap so it can't preempt on
        // its own.
        let cfg = cfg_idle(5_000, 60_000, 4);
        // Every 1s Strong signal, but the hard cap is 5s; the 5th
        // tick crosses.
        for elapsed in [1_000u64, 2_000, 3_000, 4_000] {
            assert_eq!(
                decide_lease(&cfg, snap(elapsed, elapsed, 0, HeartbeatKind::Strong)),
                LeaseDecision::Continue
            );
        }
        assert_eq!(
            decide_lease(&cfg, snap(5_000, 5_000, 0, HeartbeatKind::Strong)),
            LeaseDecision::HardKill
        );
    }

    // ---- Defense: now_ms saturating_sub last_hb_ms. ----
    #[test]
    fn lease_last_hb_in_future_saturates_to_zero() {
        // Caller bug: last_hb_ms accidentally ahead of now_ms. We
        // must not panic; saturating_sub keeps us safe and the
        // branch returns Continue (since 0 < idle_window).
        let cfg = cfg_idle(60_000, 1_000, 4);
        assert_eq!(
            decide_lease(&cfg, snap(500, 2_000, 0, HeartbeatKind::None)),
            LeaseDecision::Continue
        );
    }

    // ---- Apply-loop: `LeaseState::tick` runs end-to-end. ----
    #[test]
    fn lease_state_tick_strong_resets() {
        let cfg = cfg_idle(60_000, 5_000, 2);
        let mut state = LeaseState::fresh(0);
        // Drive two Weak ticks to bring weak_count to 2.
        assert_eq!(
            state.tick(HeartbeatKind::Weak, 1_000, &cfg),
            LeaseDecision::Continue
        );
        assert_eq!(
            state.tick(HeartbeatKind::Weak, 2_000, &cfg),
            LeaseDecision::Continue
        );
        assert_eq!(state.weak_count, 2);
        // A Strong signal continues and resets the counter.
        assert_eq!(
            state.tick(HeartbeatKind::Strong, 3_000, &cfg),
            LeaseDecision::Continue
        );
        assert_eq!(state.last_hb_ms, 3_000);
        assert_eq!(state.weak_count, 0);
    }

    #[test]
    fn lease_state_tick_weak_kick_at_cap() {
        let cfg = cfg_idle(60_000, 5_000, 2);
        let mut state = LeaseState::fresh(0);
        assert_eq!(
            state.tick(HeartbeatKind::Weak, 1_000, &cfg),
            LeaseDecision::Continue
        );
        assert_eq!(
            state.tick(HeartbeatKind::Weak, 2_000, &cfg),
            LeaseDecision::Continue
        );
        // Third Weak → IdleKill; weak_count does NOT advance on a
        // rejection, mirroring the decision logic.
        assert_eq!(
            state.tick(HeartbeatKind::Weak, 3_000, &cfg),
            LeaseDecision::IdleKill
        );
        assert_eq!(state.weak_count, 2, "IdleKill must not advance the counter");
        // `last_hb_ms` stays put — a rejected Weak must not reset
        // the idle window.
        assert_eq!(state.last_hb_ms, 2_000);
    }

    #[test]
    fn lease_state_tick_continues_through_silence_then_idle_kills() {
        let cfg = cfg_idle(60_000, 1_000, 4);
        let mut state = LeaseState::fresh(0);
        // Strong at t=0 — refreshes. weak_count = 0, last_hb = 0.
        state.tick(HeartbeatKind::Strong, 0, &cfg);
        // Now sit silent for 999ms — still Continue.
        assert_eq!(
            state.tick(HeartbeatKind::None, 999, &cfg),
            LeaseDecision::Continue
        );
        // At t=1000ms (boundary), it kills.
        assert_eq!(
            state.tick(HeartbeatKind::None, 1_000, &cfg),
            LeaseDecision::IdleKill
        );
    }

    #[test]
    fn lease_state_tick_strong_inside_idle_window_after_silence() {
        let cfg = cfg_idle(60_000, 5_000, 4);
        let mut state = LeaseState::fresh(0);
        state.tick(HeartbeatKind::Strong, 0, &cfg);
        // Sit silent 3s — still within the 5s window.
        assert_eq!(
            state.tick(HeartbeatKind::None, 3_000, &cfg),
            LeaseDecision::Continue
        );
        // A Strong signal at 3.5s refreshes; weak counter untouched
        // because Strong does not bump it.
        assert_eq!(
            state.tick(HeartbeatKind::Strong, 3_500, &cfg),
            LeaseDecision::Continue
        );
        assert_eq!(state.last_hb_ms, 3_500);
    }

    #[test]
    fn lease_state_tick_hard_kill_stops_clock() {
        let cfg = cfg_idle(60_000, 5_000, 4);
        let mut state = LeaseState::fresh(0);
        // Drive to just under the cap so we have non-trivial
        // last_hb / weak_count state.
        assert_eq!(
            state.tick(HeartbeatKind::Strong, 0, &cfg),
            LeaseDecision::Continue
        );
        assert_eq!(
            state.tick(HeartbeatKind::Strong, 1_000, &cfg),
            LeaseDecision::Continue
        );
        // Cross the hard ceiling → HardKill.
        assert_eq!(
            state.tick(HeartbeatKind::Strong, 60_000, &cfg),
            LeaseDecision::HardKill
        );
        // The clock is frozen: HardKill ticks at later `now_ms`
        // must not advance the visible state.
        let snap_now = state.now_ms;
        let snap_last_hb = state.last_hb_ms;
        let snap_weak = state.weak_count;
        assert_eq!(
            state.tick(HeartbeatKind::Strong, 99_999, &cfg),
            LeaseDecision::HardKill
        );
        assert_eq!(state.now_ms, snap_now);
        assert_eq!(state.last_hb_ms, snap_last_hb);
        assert_eq!(state.weak_count, snap_weak);
    }

    // ---- Display for greppable worker logs (used in U9 wiring). ----
    #[test]
    fn lease_decision_display_is_stable() {
        assert_eq!(LeaseDecision::Continue.to_string(), "continue");
        assert_eq!(LeaseDecision::IdleKill.to_string(), "idle_kill");
        assert_eq!(LeaseDecision::HardKill.to_string(), "hard_kill");
    }
}
