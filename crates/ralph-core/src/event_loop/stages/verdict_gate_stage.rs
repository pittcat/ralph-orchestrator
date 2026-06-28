//! `VerdictGateStage` — terminal-alignment gate (U9.5).
//!
//! Why this stage lives last in the pipeline: terminal
//! alignment is a high-stakes decision (it triggers loop
//! termination). Catching type errors (schema gate) and
//! step-scope errors (flow-scope) first means a verdict
//! misfire cannot be caused by a malformed payload or a
//! cross-step publish.
//!
//! Cross-platform / concurrency semantics: pure CPU. The
//! `terminal_emits` set is locked at `[LOOP_COMPLETE]` and
//! is the single source of truth for "what topic may
//! terminate the loop".

use crate::event_loop::flow_declaration::FlowDeclaration;
use crate::event_loop::stage_pipeline::{EmitStage, StageContext, StageReject};
use ralph_proto::Event;

/// Default `terminal_emits` set. The plan locks this to
/// `[LOOP_COMPLETE]` because:
///
/// - `LOOP_COMPLETE` is the only topic whose emission
///   signals "the loop is done — close the ledger and exit".
/// - `REPORT_DONE` is the reporter's success signal but the
///   loop continues afterwards (the shipper may still route
///   it), so it must not be terminal.
/// - `REVIEW_COMPLETE` is review_walk's internal close — it
///   does not by itself end the loop, and is **not** in
///   `terminal_emits`.
pub const DEFAULT_TERMINAL_EMITS: &[&str] = &["LOOP_COMPLETE"];

/// Verdict gate. Any event whose topic is in
/// `FlowDeclaration.terminal_emits` is **not** rejected —
/// the stage accepts it and the pipeline dispatcher
/// recognises the match to write the loop-termination
/// record.
///
/// Any topic that is *not* in `terminal_emits` is **also**
/// accepted: the verdict gate does not police general emit
/// topics (the schema gate and flow-scope stage already
/// did). Its single job is to expose `is_terminal` so the
/// pipeline dispatcher can do "is this the loop-terminating
/// emit?".
pub struct VerdictGateStage {
    flow: FlowDeclaration,
}

/// U10 (2026-06-27-002 plan completion): free-function
/// form of `VerdictGateStage::is_terminal`. The
/// `StagePipeline::is_terminal` probe uses this when
/// the locked-last stage is a `VerdictGateStage` so
/// the dispatcher can decide whether to write the
/// loop-termination record without holding a
/// `VerdictGateStage` instance directly.
pub fn is_terminal_topic(topic: &str) -> bool {
    DEFAULT_TERMINAL_EMITS.contains(&topic)
}

impl VerdictGateStage {
    pub fn new(flow: FlowDeclaration) -> Self {
        Self { flow }
    }

    /// True if `topic` is in the locked `terminal_emits` set.
    pub fn is_terminal(&self, topic: &str) -> bool {
        self.flow
            .terminal_emits
            .iter()
            .any(|t| t == topic)
    }
}

impl EmitStage for VerdictGateStage {
    fn name(&self) -> &'static str {
        "VerdictGate"
    }

    fn check(&self, _ctx: &mut StageContext, event: &Event) -> Result<(), StageReject> {
        // Topics inside `terminal_emits` pass through. The
        // pipeline dispatcher reads `is_terminal` after a
        // successful `run` to decide whether to write the
        // termination record.
        if self.is_terminal(event.topic.as_str()) {
            return Ok(());
        }

        // The verdict gate does **not** reject non-terminal
        // topics — that is the schema gate's job (missing
        // fields) and the flow-scope stage's job (wrong step).
        // Cross-step publish of `LOOP_COMPLETE` is still
        // allowed; the dispatcher treats it as terminal
        // regardless of which step the hat thought it was in.
        Ok(())
    }
}

#[cfg(test)]
mod tests;