//! `RepairDispatchStage` — early-return for repair topics (U7).
//!
//! Why this stage sits between the loop-start hook and
//! `EmitSchemaGate`: a `task.relocate_legacy` event with a
//! missing `task_key` must not be rejected by the schema
//! gate (the repair stream has its own consent protocol). It
//! also must not be admitted to the main `EventBus` — repair
//! events live on the isolated stream defined in
//! `RepairStateMachine` (U2). So this stage short-circuits
//! the pipeline: every repair topic returns `Ok(())` from
//! `check`, the caller recognises the early-return via the
//! `is_repair_topic` helper, and the event is routed to the
//! repair sink rather than `EventBus`.
//!
//! Cross-platform / concurrency semantics: pure CPU. No FS,
//! no threading. The decision is a pure function of `topic`
//! and `payload`.

use crate::event_loop::stage_pipeline::{EmitStage, StageContext, StageReject};
use ralph_proto::Event;
use serde_json::Value;

/// Topics that live on the isolated repair stream. Adding a
/// new repair topic requires updating this set AND the
/// preset_lint rule that caps who may emit it (R4.4).
pub const REPAIR_TOPICS: &[&str] = &[
    "task.relocate",
    "task.relocate_legacy",
    "repair.budget.exhausted",
    "repair.close",
];

/// `true` if `topic` is on the repair stream. Used by the
/// pipeline dispatcher to route the event to the repair sink
/// instead of the main `EventBus`.
pub fn is_repair_topic(topic: &str) -> bool {
    REPAIR_TOPICS.contains(&topic)
}

/// Stage that absorbs every repair topic so subsequent
/// stages do not block it (or accidentally let it through to
/// the main bus). The early-return shape is deliberate: the
/// pipeline dispatcher checks `is_repair_topic(event.topic)`
/// after `StagePipeline::run` returns `Ok(())` and routes the
/// event to the repair sink.
pub struct RepairDispatchStage;

impl Default for RepairDispatchStage {
    fn default() -> Self {
        Self
    }
}

impl EmitStage for RepairDispatchStage {
    fn name(&self) -> &'static str {
        "RepairDispatch"
    }

    fn check(&self, _ctx: &StageContext, event: &Event) -> Result<(), StageReject> {
        if is_repair_topic(event.topic.as_str()) {
            // The pipeline dispatcher reads `is_repair_topic`
            // after a successful run; we must not return Err
            // for repair events because Err means "reject and
            // write recovery envelope", which would lose the
            // event entirely.
            return Ok(());
        }

        // Non-repair events pass through unchanged.
        Ok(())
    }
}

/// Extract `task_key` from an event payload. Used by U8 to
/// drive the `stall_recovery_counts` key. Returns `None` if
/// the payload is not an object or the field is absent.
pub fn extract_task_key(event: &Event) -> Option<String> {
    let payload: Value = serde_json::from_str(event.payload.as_str()).ok()?;
    payload
        .get("task_key")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests;