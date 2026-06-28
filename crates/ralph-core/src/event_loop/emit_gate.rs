//! U1 (2026-06-27 mechanism foundation completion): single
//! emit-gate facade for hat business events.
//!
//! Why this module exists: the original 001 plan wired the
//! stage pipeline into `publish_event` only. The JSONL
//! ingest path (`process_parse_result`) still calls
//! `bus.publish` directly, so a `plan.blocked(reason="")`
//! written to events.jsonl silently lands in the main
//! `EventBus` and downstream code treats it as a real
//! blockage. The emit-gate facade is the **single** entry
//! point that every emit path must call: it runs the locked
//! stage pipeline, observes the repair-topic early-return,
//! and returns a structured outcome that the caller routes
//! to either the main bus, the repair sink, or a
//! `record_stage_rejection` envelope.
//!
//! Cross-platform / concurrency semantics: pure CPU. No FS,
//! no threading, no async. The same input always yields
//! the same output.
//!
//! # Example
//!
//! ```no_run
//! use ralph_core::event_loop::emit_gate::{evaluate_emit_gate, EmitGateOutcome};
//! use ralph_core::event_loop::flow_declaration::FlowDeclaration;
//! use ralph_core::event_loop::repair_flow::RepairStateMachine;
//! use ralph_core::event_loop::stage_pipeline::{
//!     FlowStep, StageContext, StagePipeline,
//! };
//! use ralph_proto::Event;
//!
//! let flow = FlowDeclaration::from_yaml(
//!     "mechanism:\n  flow:\n    type: declared\n    version: 1\n    steps: []\n",
//! )
//! .expect("parse minimal flow");
//! let pipeline = StagePipeline::with_default_stages(flow);
//! // P1-5 (2026-06-27 adversarial review): the
//! // stage context now carries a per-task
//! // `HashMap<String, RepairStateMachine>`
//! // instead of a single machine. Build a
//! // one-element `HashMap` so the doctest
//! // matches the new signature.
//! let mut states: std::collections::HashMap<String, RepairStateMachine> =
//!     std::collections::HashMap::new();
//! states.insert("_loop_default".to_string(), RepairStateMachine::default());
//! let mut ctx = StageContext::with_pipeline(
//!     FlowStep::new("work.done"),
//!     "loop-doc",
//!     0,
//!     &mut states,
//!     &pipeline,
//! );
//! let event = Event::new("work.done", r#"{"task_id":"t1"}"#);
//! let outcome = evaluate_emit_gate(&mut ctx, &event);
//! // outcome is AcceptMainBus on the default pipeline
//! // (work.done has the required `task_id` field).
//! let _ = outcome;
//! ```

use crate::event_loop::stage_pipeline::StageReject;
use crate::event_loop::stages::repair_dispatch_stage::is_repair_topic;
use ralph_proto::Event;

/// Outcome of the emit-gate facade.
///
/// The caller (`publish_event` or `process_parse_result`)
/// matches on this enum and routes the event accordingly:
/// - `AcceptMainBus` → call `bus.publish(event)`
/// - `AcceptRepairStream` → call the U6 `RepairStreamSink`
///   (the event is **not** admitted to the main bus)
/// - `Reject(reason)` → call `record_stage_rejection` and
///   write a recovery envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitGateOutcome {
    /// Pipeline accepted the event and it is NOT a repair
    /// topic. Route to the main `EventBus`.
    AcceptMainBus,
    /// The event's topic is a repair topic (`is_repair_topic`
    /// returned `true`) AND the pipeline accepted it.
    /// Route to the isolated repair stream.
    AcceptRepairStream,
    /// The pipeline rejected the event. The caller must
    /// record a stage-rejection recovery envelope. The
    /// `StageReject` carries the stable reason code, the
    /// stage name, and any missing fields.
    Reject(StageReject),
}

/// Single emit-gate facade: run the locked stage pipeline
/// on `event` using the supplied `StageContext` (built by
/// the caller) and combine the result with the
/// `is_repair_topic` routing hint to produce an
/// `EmitGateOutcome`.
///
/// Routing rules (locked by this Unit; do not change
/// without updating the U2/U3 tests):
///
/// 1. Pipeline rejects (any stage returns `Err`) →
///    `EmitGateOutcome::Reject(...)`. The repair-topic
///    hint is ignored — a malformed repair event still
///    surfaces as a rejection so the recovery envelope
///    records the schema failure.
/// 2. Pipeline accepts AND topic is a repair topic →
///    `EmitGateOutcome::AcceptRepairStream`. The event is
///    NEVER admitted to the main `EventBus`.
/// 3. Pipeline accepts AND topic is NOT a repair topic →
///    `EmitGateOutcome::AcceptMainBus`.
///
/// The `StageContext` is the caller's responsibility —
/// `EventLoop` carries the live loop id, expected
/// version, and current flow step. Keeping the facade
/// free of those inputs lets U1 test it with a throwaway
/// `StageContext`.
pub fn evaluate_emit_gate(
    ctx: &mut crate::event_loop::stage_pipeline::StageContext<'_>,
    event: &Event,
) -> EmitGateOutcome {
    match ctx
        .pipeline
        .as_ref()
        .expect("StageContext must carry a pipeline")
        .run(ctx, event)
    {
        Err(reject) => EmitGateOutcome::Reject(reject),
        Ok(()) if is_repair_topic(event.topic.as_str()) => EmitGateOutcome::AcceptRepairStream,
        Ok(()) => EmitGateOutcome::AcceptMainBus,
    }
}

#[cfg(test)]
mod tests;
