//! Backend/tool timing observations (plan 2026-10-29-0104 U3).
//!
//! `TimingHandler` wraps any [`StreamHandler`] and passes every event
//! through unchanged — display behavior is identical with or without
//! timing attached. In parallel it records `timing.jsonl` span rows
//! for what a backend stream can actually expose:
//!
//! - `Startup` span: handler construction → first text or first tool
//!   call. This is *first client-observed output* latency — it is NOT
//!   server-side TTFT, and the attribute `boundary: first_output`
//!   keeps the distinction explicit (R3).
//! - `ToolExec` spans keyed by tool-call id: `on_tool_call` opens,
//!   `on_tool_result` closes. Interleaved ids pair correctly; a call
//!   with no result stays censored — interruption never fabricates
//!   completion (AE4).
//! - Session usage: written as attributes on the exec-span closing
//!   row. A backend that reported no usage (`num_turns == 0` and all
//!   token counters zero) is stamped `usage_present: false` — an
//!   absent signal is never serialized as a real zero.
//!
//! Rows are pushed through a caller-supplied sink so the handler has
//! no filesystem opinion; the runner wires it to the session's
//! `TimingWriter` when diagnostics are active.
//!
//! `Drop` closes every still-open span as `Interrupted`, so an
//! interrupted session leaves honest censored/interrupted records
//! instead of silent gaps.

use std::collections::HashMap;

use ralph_core::diagnostics::{
    ClockEpoch, ExecutionIdentity, SpanKind, SpanStatus, TimingSpan,
};
use serde_json::json;

use crate::stream_handler::{SessionResult, StreamHandler};

/// Sink for finished/open span rows. `TimingWriter::append` satisfies
/// the signature via a closure; tests can collect into a `Vec`.
pub type SpanSink = Box<dyn FnMut(TimingSpan) + Send>;

/// Stream-handler wrapper that emits timing observations while
/// delegating display to `inner` unchanged.
pub struct TimingHandler<H: StreamHandler> {
    inner: H,
    epoch: ClockEpoch,
    identity: ExecutionIdentity,
    sink: SpanSink,
    /// Parent span linking tool calls to this activation.
    parent_span_id: Option<String>,
    /// Startup span awaiting the first observable output.
    startup: Option<TimingSpan>,
    /// Exec observation span closed on completion/error/drop.
    exec: Option<TimingSpan>,
    /// Tool spans still open, keyed by tool-call id.
    open_tools: HashMap<String, TimingSpan>,
    /// Set once `on_complete`/`on_error` resolved the session.
    finished: bool,
}

impl<H: StreamHandler> TimingHandler<H> {
    /// Wrap `inner`. `identity` correlates produced rows with the
    /// loop/session/activation they belong to (R1); `parent_span_id`
    /// links tool rows under the runner's AgentExec span when known.
    pub fn new(
        inner: H,
        identity: ExecutionIdentity,
        parent_span_id: Option<String>,
        sink: SpanSink,
    ) -> Self {
        let epoch = ClockEpoch::new();
        let mut startup = TimingSpan::open(
            format!("obs-{}", epoch.next_span_id()),
            identity.clone(),
            SpanKind::Startup,
            &epoch,
        )
        .with_attributes(json!({"boundary": "first_output"}));
        if let Some(p) = &parent_span_id {
            startup = startup.with_parent(p.clone());
        }
        let mut exec = TimingSpan::open(
            format!("obs-{}", epoch.next_span_id()),
            identity.clone(),
            SpanKind::AgentExec,
            &epoch,
        )
        .with_attributes(json!({"observed_by": "stream"}));
        if let Some(p) = &parent_span_id {
            exec = exec.with_parent(p.clone());
        }
        let mut this = Self {
            inner,
            epoch,
            identity,
            sink,
            parent_span_id,
            startup: Some(startup),
            exec: Some(exec),
            open_tools: HashMap::new(),
            finished: false,
        };
        // Open rows are persisted immediately so a crash leaves
        // honest censored records (KTD3).
        this.emit_open_rows();
        this
    }

    fn emit_open_rows(&mut self) {
        if let Some(s) = &self.startup {
            (self.sink)(s.clone());
        }
        if let Some(s) = &self.exec {
            (self.sink)(s.clone());
        }
    }

    /// Close the startup span at first observable output.
    fn observe_first_output(&mut self) {
        if let Some(mut s) = self.startup.take() {
            s.complete(&self.epoch);
            (self.sink)(s);
        }
    }

    /// Close any remaining spans as interrupted — used by `Drop` and
    /// terminal callbacks. Never fabricates `Completed`.
    fn drain_interrupted(&mut self) {
        if let Some(mut s) = self.startup.take() {
            s.interrupt(&self.epoch);
            (self.sink)(s);
        }
        for (_, mut s) in self.open_tools.drain() {
            s.interrupt(&self.epoch);
            (self.sink)(s);
        }
        if let Some(mut s) = self.exec.take() {
            if s.status == SpanStatus::Open {
                s.interrupt(&self.epoch);
            }
            (self.sink)(s);
        }
    }

    /// Access the wrapped handler (e.g. for mode-specific downcasts).
    pub fn inner(&self) -> &H {
        &self.inner
    }

    /// Mutable access to the wrapped handler.
    pub fn inner_mut(&mut self) -> &mut H {
        &mut self.inner
    }
}

impl<H: StreamHandler> StreamHandler for TimingHandler<H> {
    fn on_text(&mut self, text: &str) {
        self.observe_first_output();
        self.inner.on_text(text);
    }

    fn on_tool_call(&mut self, name: &str, id: &str, input: &serde_json::Value) {
        self.observe_first_output();
        let mut span = TimingSpan::open(
            format!("obs-{}", self.epoch.next_span_id()),
            self.identity.clone(),
            SpanKind::ToolExec,
            &self.epoch,
        )
        .with_attributes(json!({"tool": name, "tool_call_id": id}));
        if let Some(p) = &self.parent_span_id {
            span = span.with_parent(p.clone());
        }
        (self.sink)(span.clone());
        self.open_tools.insert(id.to_string(), span);
        self.inner.on_tool_call(name, id, input);
    }

    fn on_tool_result(&mut self, id: &str, output: &str) {
        if let Some(mut span) = self.open_tools.remove(id) {
            span.complete(&self.epoch);
            (self.sink)(span);
        }
        self.inner.on_tool_result(id, output);
    }

    fn on_error(&mut self, error: &str) {
        self.finished = true;
        // Errors never fabricate completions: open spans close as
        // interrupted so readers see honest censored intervals.
        self.drain_interrupted();
        self.inner.on_error(error);
    }

    fn on_complete(&mut self, result: &SessionResult) {
        self.finished = true;
        if let Some(mut s) = self.exec.take() {
            s.complete(&self.epoch);
            let usage_present = result.num_turns > 0
                || result.input_tokens > 0
                || result.output_tokens > 0;
            s.attributes = Some(json!({
                "observed_by": "stream",
                "usage_present": usage_present,
                "is_error": result.is_error,
                "duration_ms": result.duration_ms,
                "input_tokens": result.input_tokens,
                "output_tokens": result.output_tokens,
                "num_turns": result.num_turns,
            }));
            (self.sink)(s);
        }
        // Any tool span still open at session end is genuinely
        // censored — the backend never sent its result.
        for (_, mut s) in self.open_tools.drain() {
            s.interrupt(&self.epoch);
            (self.sink)(s);
        }
        self.inner.on_complete(result);
    }
}

impl<H: StreamHandler> Drop for TimingHandler<H> {
    fn drop(&mut self) {
        if !self.finished {
            self.drain_interrupted();
        }
    }
}
