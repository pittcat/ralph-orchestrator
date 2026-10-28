//! Replay tests for `TimingHandler` (plan 2026-10-29-0104 U3).
//!
//! Fixed event streams are driven through a `TimingHandler` wrapping
//! a recording sink handler; each scenario asserts the produced span
//! rows (never against source text). No live backend is involved.

use std::sync::{Arc, Mutex};

use ralph_adapters::{SessionResult, StreamHandler, TimingHandler};
use ralph_core::diagnostics::{
    ExecutionIdentity, SpanKind, SpanStatus, TimingSpan,
};

/// Handler that records the display events it receives, so tests can
/// prove the wrapper preserves inner-handler behavior exactly.
#[derive(Default)]
struct RecordingHandler {
    events: Vec<String>,
}

impl StreamHandler for RecordingHandler {
    fn on_text(&mut self, text: &str) {
        self.events.push(format!("text:{text}"));
    }
    fn on_tool_call(&mut self, name: &str, id: &str, _input: &serde_json::Value) {
        self.events.push(format!("call:{name}:{id}"));
    }
    fn on_tool_result(&mut self, id: &str, _output: &str) {
        self.events.push(format!("result:{id}"));
    }
    fn on_error(&mut self, error: &str) {
        self.events.push(format!("error:{error}"));
    }
    fn on_complete(&mut self, result: &SessionResult) {
        self.events.push(format!("complete:{}", result.duration_ms));
    }
}

struct Replay {
    spans: Arc<Mutex<Vec<TimingSpan>>>,
    handler: TimingHandler<RecordingHandler>,
}

fn replay() -> Replay {
    let spans = Arc::new(Mutex::new(Vec::new()));
    let sink_spans = Arc::clone(&spans);
    let handler = TimingHandler::new(
        RecordingHandler::default(),
        ExecutionIdentity::for_hat("executor"),
        Some("parent-exec".to_string()),
        Box::new(move |row: TimingSpan| sink_spans.lock().unwrap().push(row)),
    );
    Replay { spans, handler }
}

fn result(duration_ms: u64, turns: u32, input: u64, output: u64) -> SessionResult {
    SessionResult {
        duration_ms,
        total_cost_usd: 0.01,
        num_turns: turns,
        is_error: false,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
    }
}

fn kinds(spans: &[TimingSpan], kind: SpanKind) -> Vec<&TimingSpan> {
    spans.iter().filter(|s| s.kind == kind).collect()
}

#[test]
fn interleaved_tool_ids_pair_correctly() {
    let mut r = replay();
    r.handler.on_text("planning");
    r.handler.on_tool_call("Bash", "t-1", &serde_json::json!({}));
    r.handler.on_tool_call("Read", "t-2", &serde_json::json!({}));
    // Results arrive out of order — pairing must be by id.
    r.handler.on_tool_result("t-2", "file");
    r.handler.on_tool_result("t-1", "ok");
    r.handler.on_complete(&result(500, 2, 100, 50));

    let spans = r.spans.lock().unwrap();
    let tools: Vec<&TimingSpan> = spans
        .iter()
        .filter(|s| s.kind == SpanKind::ToolExec && s.status == SpanStatus::Completed)
        .collect();
    assert_eq!(tools.len(), 2);
    let ids: Vec<&str> = tools
        .iter()
        .filter_map(|s| s.attributes.as_ref()?.get("tool_call_id")?.as_str())
        .collect();
    assert!(ids.contains(&"t-1"));
    assert!(ids.contains(&"t-2"));
    // Display behavior was preserved verbatim through the wrapper.
    let events = &r.handler.inner().events;
    assert_eq!(events.len(), 6);
    assert_eq!(events[1], "call:Bash:t-1");
}

#[test]
fn missing_result_keeps_span_censored_until_session_end() {
    let mut r = replay();
    r.handler.on_tool_call("Bash", "t-1", &serde_json::json!({}));
    r.handler.on_tool_call("Read", "t-2", &serde_json::json!({}));
    r.handler.on_tool_result("t-1", "ok");
    // t-2 never returns; session completes anyway.
    r.handler.on_complete(&result(300, 1, 10, 10));

    let spans = r.spans.lock().unwrap();
    let t2_close = spans
        .iter()
        .filter(|s| {
            s.kind == SpanKind::ToolExec
                && s.attributes
                    .as_ref()
                    .and_then(|a| a.get("tool_call_id"))
                    .and_then(|v| v.as_str())
                    == Some("t-2")
                && s.status != SpanStatus::Open
        })
        .count();
    // Exactly one closing row for t-2 and it is interrupted, never
    // fabricated as completed.
    assert_eq!(t2_close, 1);
    let t2 = spans
        .iter()
        .find(|s| {
            s.kind == SpanKind::ToolExec
                && s.status != SpanStatus::Open
                && s.attributes
                    .as_ref()
                    .and_then(|a| a.get("tool_call_id"))
                    .and_then(|v| v.as_str())
                    == Some("t-2")
        })
        .unwrap();
    assert_eq!(t2.status, SpanStatus::Interrupted);
}

#[test]
fn text_only_session_has_startup_and_exec_duration() {
    let mut r = replay();
    r.handler.on_text("hello");
    r.handler.on_text(" world");
    r.handler.on_complete(&result(120, 1, 5, 5));

    let spans = r.spans.lock().unwrap();
    let startups = kinds(&spans, SpanKind::Startup);
    assert_eq!(startups.len(), 2); // open row + closing row
    let closed = startups
        .iter()
        .find(|s| s.status == SpanStatus::Completed)
        .unwrap();
    assert_eq!(
        closed
            .attributes
            .as_ref()
            .and_then(|a| a.get("boundary"))
            .and_then(|v| v.as_str()),
        Some("first_output")
    );
    let exec = spans
        .iter()
        .find(|s| s.kind == SpanKind::AgentExec && s.status == SpanStatus::Completed)
        .unwrap();
    let attrs = exec.attributes.as_ref().unwrap();
    assert_eq!(attrs["usage_present"], serde_json::json!(true));
    assert_eq!(attrs["input_tokens"], serde_json::json!(5));
}

#[test]
fn interrupted_session_never_fabricates_completion() {
    let mut r = replay();
    r.handler.on_tool_call("Bash", "t-1", &serde_json::json!({}));
    // Session dies mid-tool (drop without on_complete).
    drop(r.handler);

    let spans = r.spans.lock().unwrap();
    // No tool or exec span may be completed (startup completing on
    // first observed output is correct — the tool call WAS observed).
    assert!(spans
        .iter()
        .filter(|s| matches!(s.kind, SpanKind::ToolExec | SpanKind::AgentExec))
        .all(|s| s.status != SpanStatus::Completed));
    assert!(spans
        .iter()
        .any(|s| s.kind == SpanKind::ToolExec && s.status == SpanStatus::Interrupted));
    assert!(spans
        .iter()
        .any(|s| s.kind == SpanKind::AgentExec && s.status == SpanStatus::Interrupted));
}

#[test]
fn absent_usage_is_distinguished_from_real_zero() {
    let mut r = replay();
    r.handler.on_text("x");
    // num_turns=0 and all-zero token counters = backend reported no
    // usage signal at all — must not serialize as measured zeros.
    r.handler.on_complete(&result(10, 0, 0, 0));

    let spans = r.spans.lock().unwrap();
    let exec = spans
        .iter()
        .find(|s| s.kind == SpanKind::AgentExec && s.status == SpanStatus::Completed)
        .unwrap();
    assert_eq!(
        exec.attributes.as_ref().unwrap()["usage_present"],
        serde_json::json!(false)
    );
}
