//! `RepairDispatchStage` — early-return for repair topics (U7)
//! + budget exhaustion gate (U5).
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
//! U5 (2026-06-27-002 plan completion): the same stage also
//! drives the per-task retry budget. A repair topic mapped
//! to `RepairAction::Retry` consumes one unit of the budget;
//! when the budget is exhausted the stage returns
//! `StageReject { reason_code: repair_unrecoverable_after_N_retries }`
//! so the recovery envelope records the failure instead of
//! silently burning through retries.
//!
//! Cross-platform / concurrency semantics: pure CPU. No FS,
//! no threading. The decision is a pure function of `topic`,
//! `payload`, and the current state of the per-loop
//! `RepairStateMachine`.

use crate::event_loop::repair_flow::{
    RepairAction, RepairBudget, RepairStateMachine, RepairTransitionResult,
};
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

/// U5 (2026-06-27-002 plan completion): map a repair
/// topic + payload to a `RepairAction` so the
/// `RepairDispatchStage` can advance the per-task
/// budget. The mapping is deliberately simple — every
/// repair topic starts the diagnostic lifecycle; only
/// `repair.close` short-circuits to `Close`. U6 will
/// extend this with the per-task `task_key` extraction
/// so retries are scoped to a single task instead of the
/// whole loop.
pub fn repair_action_for(topic: &str, _payload: &Value) -> RepairAction {
    match topic {
        "repair.close" => RepairAction::Close,
        // Everything else on the repair stream enters
        // diagnosis. The `BeginDiagnosis` action is
        // idempotent so subsequent retry topics on the
        // same task consume the budget via `Retry`.
        _ => RepairAction::BeginDiagnosis,
    }
}

/// Stage that absorbs every repair topic so subsequent
/// stages do not block it (or accidentally let it through to
/// the main bus). U5 also advances the per-task budget.
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

    fn check(&self, ctx: &mut StageContext, event: &Event) -> Result<(), StageReject> {
        if !is_repair_topic(event.topic.as_str()) {
            // Non-repair events pass through unchanged.
            // No budget is consumed.
            return Ok(());
        }

        // U5: advance the per-task budget via
        // `try_transition`. The mapping from topic →
        // action is locked by `repair_action_for`.
        let payload: Value = serde_json::from_str(event.payload.as_str())
            .unwrap_or(Value::Object(Default::default()));
        let action = repair_action_for(event.topic.as_str(), &payload);
        match ctx.repair_state.try_transition(action) {
            RepairTransitionResult::Accepted => {
                // The pipeline dispatcher reads
                // `is_repair_topic` after a successful run;
                // we must not return Err for repair
                // events because Err means "reject and
                // write recovery envelope", which would
                // lose the event entirely. Return Ok so
                // the dispatcher routes the event to the
                // repair sink.
                Ok(())
            }
            RepairTransitionResult::BudgetExhausted(exhausted) => {
                // U5: budget exhausted. Return Reject so
                // the dispatcher writes a stage-rejection
                // envelope and the event is NOT routed to
                // the repair sink (the budget is the
                // fail-closed gate). The reason code is
                // stable so the BDD scenario
                // `repair_budget_exhausted_blocks_plan`
                // (U15) can assert it verbatim.
                Err(StageReject::new(self.name(), exhausted.reason_code)
                    .with_missing_fields(vec![
                        format!("retries_consumed={}", exhausted.retries_consumed),
                        format!("max={}", exhausted.max),
                    ]))
            }
            RepairTransitionResult::IllegalTransition { from, action: _ } => {
                // An illegal transition is a programming
                // error: a repair topic was emitted in a
                // state that does not accept the mapped
                // action. Reject so the recovery envelope
                // records the operator-visible signal.
                Err(StageReject::new(
                    self.name(),
                    format!("repair_illegal_transition_from_{from:?}"),
                ))
            }
        }
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

/// U5: build a `RepairStateMachine` with a custom budget
/// (test-only helper).
pub fn repair_state_machine_with_budget(max: u32) -> RepairStateMachine {
    RepairStateMachine::new(RepairBudget::new(max))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_u5;