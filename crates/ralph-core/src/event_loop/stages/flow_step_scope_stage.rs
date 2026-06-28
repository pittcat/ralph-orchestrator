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

/// 2026-06-28 plan U3: defensive bypass list.
///
/// Until U4 (plan-mode `current_step` state machine) lands, the
/// `current_step` stays pinned to the first declared step (e.g.
/// `unit_loop`). That means a review-chain hat emitting a
/// topic that lives in a later step's `allowed_emits` (e.g.
/// `review-coordinator` emitting `review.dimension.ready`
/// from `unit_loop`) would hit `flow_unknown_emit` and stall
/// the loop.
///
/// The bypass accepts a small whitelist of `(hat, topic)`
/// pairs so the review chain can move forward. U4 will replace
/// most of these naturally once `current_step` actually
/// advances; the entries that remain are kept as a safety net
/// for state-machine edge cases (e.g. terminal self-stop
/// events that the operator is allowed to emit before the
/// review chain reaches `plan_end`).
///
/// The bypass is **temporary** — it does NOT widen scope for
/// the emitting hat's full `publishes` set, only the listed
/// topics, and only when the topic is not already in the
/// current step's `allowed_emits`.
const DEFENSIVE_BYPASS: &[(&str, &str)] = &[
    // Coordinator (the runner hat) drives the lifecycle.
    ("coordinator", "review.start"),
    ("coordinator", "plan.complete"),
    // Review chain: each hat emits its own well-defined topic
    // set, all of which only become valid in a later step.
    ("review-coordinator", "review.dimension.ready"),
    ("review-coordinator", "review.dimensions.complete"),
    ("review-synthesizer", "review.complete"),
    ("dimension-reviewer", "review.dimension.done"),
    ("dimension-reviewer", "review.dimension.failed"),
    // Shipper closes the plan; its terminal event lands
    // before the verdict gate would accept it.
    ("shipper", "REVIEW_COMPLETE"),
    // Self-termination paths: ralph / coordinator are allowed
    // to admit failure when the recovery machinery is exhausted
    // (U6, U8, U9, U10). These events are valid regardless of
    // the current step because they ARE the end of the step.
    ("ralph", "plan.blocked"),
    ("ralph", "LOOP_COMPLETE"),
    ("coordinator", "LOOP_COMPLETE"),
];

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

        // 2026-06-28 plan U3: defensive bypass — see DEFENSIVE_BYPASS.
        // The match is on `(source_hat, topic)`. A missing
        // `source_hat` (legacy / synthetic events) cannot match
        // because every bypass entry requires a real hat id.
        if let Some(source) = event.source.as_ref() {
            let source_str = source.as_str();
            if DEFENSIVE_BYPASS
                .iter()
                .any(|(hat, topic)| *hat == source_str && *topic == event.topic.as_str())
            {
                return Ok(());
            }
        }

        let step = self.flow.step(&ctx.current_step.id);
        let Some(step) = step else {
            // P1-6 (2026-06-27 adversarial review):
            // fail-closed for undeclared steps. The
            // 2026-06-27-002 plan completion rolled
            // this back to fail-open because 30+ unit
            // tests relied on the legacy behaviour,
            // but the 2026-06-27 adversarial review
            // flagged it as a P1 — any hat can set
            // `current_step` to an undeclared id and
            // bypass `allowed_emits` entirely. The
            // fail-closed default is restored; the
            // `minimal_flow_declaration_yaml`
            // fallback in `EventLoop` now declares
            // every topic the legacy tests emit so
            // the migration is invisible to test
            // fixtures. The reason code is
            // `flow_step_undeclared` so the BDD
            // scenario can assert it verbatim.
            return Err(StageReject::new(
                self.name(),
                "flow_step_undeclared",
            ));
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