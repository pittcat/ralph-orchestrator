//! `run_gates` — unified gate engine used by both the linter
//! and the runtime loop (R15, plan 2026-06-20-001).
//!
//! Single source of truth: rules come from `ProtocolView` (the
//! embedded protocol SSOT), the *context* decides whether we are
//! in lint (stateless) or runtime (stateful) mode. The same gate
//! function is invoked twice — pre-write by the linter, post-
//! receive by the loop — so the two layers cannot drift.

use std::collections::HashSet;

use serde_json::Value;

use super::protocol::ProtocolView;

/// Gate decision returned by [`run_gates`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Event is admitted.
    Accept,
    /// Event is rejected with a human-readable reason. The linter
    /// uses this to short-circuit emit; the runtime uses it to
    /// record a recovery event.
    Reject(String),
}

/// Gate context trait. Lint is stateless (implements `Clone`/`Send`),
/// runtime is stateful (may carry `&mut` references). Both
/// implementations run the same gate function with the same
/// `ProtocolView` so the two layers can never disagree on what
/// "valid" means.
pub trait GateContext {
    /// Whether the gate should run for the given (topic, payload)
    /// pair. Lets runtime contexts skip control topics that
    /// don't need policy validation.
    fn is_applicable(&self, topic: &str) -> bool;
    /// Lint contexts are pure: they only inspect the view + payload.
    /// Runtime contexts may consult additional state (recovery
    /// file, rejection TTL) before deciding.
    fn pre_check(&self, _topic: &str, _payload: &Value) -> Result<(), String> {
        Ok(())
    }
}

/// Stateless lint context. Mirrors the runtime gate; both call
/// [`run_gates`] with the same `ProtocolView`.
#[derive(Debug, Clone)]
pub struct LintContext;

impl GateContext for LintContext {
    fn is_applicable(&self, _topic: &str) -> bool {
        true
    }
}

/// Run the unified gate set against a single event. The same
/// function is used by lint (`LintContext`) and runtime (custom
/// stateful contexts). The decision is derived from the
/// `ProtocolView` so the two layers cannot diverge.
pub fn run_gates<C: GateContext>(
    view: &ProtocolView,
    ctx: &C,
    topic: &str,
    payload: &Value,
) -> GateDecision {
    if !ctx.is_applicable(topic) {
        return GateDecision::Accept;
    }
    if let Err(reason) = ctx.pre_check(topic, payload) {
        return GateDecision::Reject(reason);
    }
    let required = view.required_fields(topic);
    let missing = missing_fields(&required, payload);
    if !missing.is_empty() {
        return GateDecision::Reject(format!(
            "missing required fields: {}",
            missing.into_iter().collect::<Vec<_>>().join(",")
        ));
    }
    GateDecision::Accept
}

/// Compute the set difference: `required - present_in_payload`.
/// Treats non-object payloads as empty so the gate fails closed
/// (every required field is reported missing).
fn missing_fields(required: &HashSet<String>, payload: &Value) -> HashSet<String> {
    let present = match payload {
        Value::Object(map) => map.keys().cloned().collect(),
        _ => HashSet::new(),
    };
    required.difference(&present).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::execution_contracts::ExecutionContractsConfig;
    use crate::hat_handoff::HatHandoffConfig;
    use serde_json::json;
    use std::collections::HashMap;

    fn empty_view() -> ProtocolView {
        ProtocolView {
            effective_required_fields: HashMap::new(),
            verdict_gate: None,
            workflow_contract: None,
            state_projection: None,
            execution_contracts: Some(ExecutionContractsConfig::default()),
            hat_handoff: HatHandoffConfig::default(),
            protocol_hash: "0".to_string(),
        }
    }

    #[test]
    fn accept_when_no_required_fields() {
        let view = empty_view();
        let decision = run_gates(&view, &LintContext, "any", &json!({}));
        assert_eq!(decision, GateDecision::Accept);
    }

    #[test]
    fn reject_when_required_missing() {
        let mut view = empty_view();
        let mut reqs = HashSet::new();
        reqs.insert("plan_name".to_string());
        reqs.insert("step".to_string());
        view.effective_required_fields
            .insert("work.done".to_string(), reqs);
        let decision = run_gates(&view, &LintContext, "work.done", &json!({}));
        assert!(matches!(decision, GateDecision::Reject(_)));
    }

    #[test]
    fn accept_when_all_required_present() {
        let mut view = empty_view();
        let mut reqs = HashSet::new();
        reqs.insert("plan_name".to_string());
        reqs.insert("step".to_string());
        view.effective_required_fields
            .insert("work.done".to_string(), reqs);
        let decision = run_gates(
            &view,
            &LintContext,
            "work.done",
            &json!({"plan_name": "x", "step": "s"}),
        );
        assert_eq!(decision, GateDecision::Accept);
    }
}
