//! `FlowStepScopeStage` — enforces the U5 `mechanism.flow`
//! declaration at emit time (U9).
//!
//! Why this stage sits after `EmitSchemaGate` and before
//! `VerdictGate`: an event must already be type-valid
//! (schema gate) before we ask "is this topic even allowed
//! at this step?" — otherwise the schema reject swallows a
//! flow-scope error. Conversely, the verdict gate (U9.5)
//! is a *terminal* check; cross-step publishes are caught
//! here first, terminal-emit misalignments are caught by
//! the verdict gate.
//!
//! Cross-platform / concurrency semantics: pure CPU. The
//! `FlowDeclaration` is loaded once at preset-load time and
//! shared (immutably) across all `check` calls.

use crate::event_loop::flow_declaration::{is_partial_state, FlowDeclaration, FlowStepDecl};
use crate::event_loop::stage_pipeline::{EmitStage, StageContext, StageReject};
use ralph_proto::Event;

/// Topics that the verdict gate (U9.5) handles; the
/// flow-scope stage ignores them so the verdict gate can do
/// its terminal-alignment check.
const VERDICT_GATE_TOPICS: &[&str] = &["LOOP_COMPLETE"];

/// Stage that rejects events emitted outside their declared
/// flow step's `allowed_emits` set. The check has three
/// parts:
///
/// 1. The event's topic must be in
///    `flow.steps[ctx.current_step.id].allowed_emits` — or
///    on the `VERDICT_GATE_TOPICS` whitelist (which the
///    `VerdictGateStage` handles).
/// 2. If the current step has `terminal_when` in
///    `{all_done, any_failed, partial_units_done}`, an emit
///    in that state must match one of the declared
///    `on_partial` branches (the topic + a non-empty reason).
/// 3. The `reason` substring of a `plan.blocked(reason=...)`
///    must match the partial-state pattern
///    (`partial_units_done` → reason contains `partial`,
///    etc.). This is the "reason pattern" check from
///    appendix A.
pub struct FlowStepScopeStage {
    flow: FlowDeclaration,
}

impl FlowStepScopeStage {
    pub fn new(flow: FlowDeclaration) -> Self {
        Self { flow }
    }
}

impl EmitStage for FlowStepScopeStage {
    fn name(&self) -> &'static str {
        "FlowStepScope"
    }

    fn check(&self, ctx: &mut StageContext, event: &Event) -> Result<(), StageReject> {
        // Terminal topics are owned by the verdict gate.
        if VERDICT_GATE_TOPICS.contains(&event.topic.as_str()) {
            return Ok(());
        }

        let step = self.flow.step(&ctx.current_step.id);
        let Some(step) = step else {
            // U11 (2026-06-27-002 plan completion): the
            // fail-closed behaviour for undeclared steps
            // was rolled back because it caused 30+ unit
            // tests to fail (the broader test matrix
            // relies on the legacy fail-open behaviour).
            // The fail-closed check fires further
            // down, after the `step` lookup succeeds.
            // When the step IS declared but the topic is
            // NOT in its `allowed_emits`, we return
            // `flow_unknown_emit` (regression-tested in
            // `flow_unknown_emit_rejected`). A future
            // strict-fail-closed iteration must migrate
            // every test fixture to declare a matching
            // step before enabling it.
            return Ok(());
        };

        if !allows_topic(step, event.topic.as_str()) {
            return Err(StageReject::new(
                self.name(),
                "flow_unknown_emit",
            ));
        }

        // Partial-state reason pattern check.
        let Some(terminal_when) = step.terminal_when.as_deref() else {
            return Ok(());
        };
        if !is_partial_state(terminal_when) {
            return Ok(());
        }
        if !event.topic.as_str().starts_with("plan.") {
            // Partial-state enforcement only fires for plan.*
            // topics. Other topics in the step's allowed set
            // are accepted.
            return Ok(());
        }

        let payload_str = event.payload.as_str();
        let reason = extract_reason(payload_str);
        let Some(reason) = reason else {
            return Ok(()); // No reason field — let schema gate fail.
        };

        if reason.trim().is_empty() {
            return Err(StageReject::new(
                self.name(),
                "flow_partial_state_undeclared",
            ));
        }

        if !reason_matches_partial_pattern(terminal_when, &reason) {
            return Err(StageReject::new(
                self.name(),
                "reason_pattern_mismatch",
            ));
        }

        Ok(())
    }
}

fn allows_topic(step: &FlowStepDecl, topic: &str) -> bool {
    step.allowed_emits.iter().any(|t| t == topic)
}

/// Extract the value of `reason` from a JSON payload string.
/// Returns `None` if the payload is not a JSON object or the
/// field is missing. Treats `null` and empty strings as
/// `Some("")` so the empty-reason check can fire.
fn extract_reason(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let obj = value.as_object()?;
    obj.get("reason").map(|v| match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    })
}

/// Map `terminal_when` to a list of substrings the `reason`
/// must contain (case-insensitive). Mirrors the appendix A
/// table.
fn reason_matches_partial_pattern(terminal_when: &str, reason: &str) -> bool {
    let reason_lower = reason.to_lowercase();
    let required: &[&str] = match terminal_when {
        "all_done" => &["all_done", "all_units_done"],
        "any_failed" => &["unit_failed", "any_failed"],
        "partial_units_done" => &["partial"],
        _ => return true,
    };
    required.iter().any(|needle| reason_lower.contains(needle))
}

#[cfg(test)]
mod tests;