//! `run_gates` — unified gate engine used by both the linter
//! and the runtime loop (R15, plan 2026-06-20-001).
//!
//! Single source of truth: rules come from `ProtocolView` (the
//! embedded protocol SSOT), the *context* decides whether we are
//! in lint (stateless) or runtime (stateful) mode. The same gate
//! function is invoked twice — pre-write by the linter, post-
//! receive by the loop — so the two layers cannot drift.
//!
//! ## P1-1: structured rejection classification
//!
//! `GateDecision::Reject` carries a typed [`RejectionKind`] enum
//! in addition to the human-readable message. The linter's
//! `LintResumeHint` derives its class directly from the enum, so
//! the routing target (`SourceHat` / `PlanGate`) is determined by
//! the *kind of failure*, not by string-substring matching. This
//! eliminates the previous P1-1 vulnerability: a reason string
//! that happened to contain the word "artifact" would have
//! mis-classified a payload error as a handoff-artifact error.

use std::collections::HashSet;

use serde_json::Value;

use super::protocol::ProtocolView;

/// Gate decision returned by [`run_gates`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Event is admitted.
    Accept,
    /// Event is rejected. The `kind` carries the structural
    /// classification (used by the linter to pick the resume
    /// target hat); `message` is the human-readable detail
    /// (logged + shown to the agent).
    Reject {
        kind: RejectionKind,
        message: String,
    },
}

/// Structural rejection classification (P1-1).
///
/// The linter maps `RejectionKind` to `LintFailureClass` to
/// decide whether the resume hint should route back to the
/// source hat or to `plan-gate`. The runtime uses the same
/// classification to populate `recent_rejection_digest`
/// reason_codes.
///
/// Adding a new variant is the supported way to add a new
/// rejection class. String-substring matching on `message` is
/// **not** supported; do not reintroduce it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectionKind {
    /// Required payload field is missing. Routes to the
    /// source hat (the agent that emitted the event) so the
    /// payload can be corrected.
    MissingField,
    /// Topic emitted by a hat that does not own it. Routes
    /// back to the source hat to discourage cross-hat
    /// publishing.
    TopicOwnership,
    /// Upstream state mismatch (progress.md / step / state
    /// projection). Routes to `plan-gate` which owns the
    /// orchestration state.
    UpstreamState,
    /// Handoff artifact missing required sections / `## next`
    /// marker. Routes back to the source hat so the agent
    /// can regenerate the artifact.
    HandoffArtifact,
    /// Gate context refused the event before any field check
    /// (e.g. runtime TTL exceeded). Routes to the source hat.
    PreCheck,
}

impl RejectionKind {
    /// Map a gate-rejection kind to the linter's failure class.
    /// The two enums are kept distinct so the engine layer
    /// does not depend on the linter layer; this mapping is
    /// the only cross-layer surface.
    pub fn to_lint_class(self) -> crate::preset::engine::hint::LintFailureClass {
        use crate::preset::engine::hint::LintFailureClass;
        match self {
            RejectionKind::MissingField => LintFailureClass::PayloadError,
            RejectionKind::TopicOwnership => LintFailureClass::TopicOwnership,
            RejectionKind::UpstreamState => LintFailureClass::UpstreamStateMissing,
            RejectionKind::HandoffArtifact => LintFailureClass::HandoffArtifact,
            RejectionKind::PreCheck => LintFailureClass::PayloadError,
        }
    }

    /// Stable string identifier for log / reason_code
    /// aggregation. Operators rely on this in scripts so the
    /// values are part of the public surface — do not rename
    /// without a migration plan.
    pub fn reason_code(self) -> &'static str {
        match self {
            RejectionKind::MissingField => "missing_field",
            RejectionKind::TopicOwnership => "topic_ownership",
            RejectionKind::UpstreamState => "upstream_state",
            RejectionKind::HandoffArtifact => "handoff_artifact",
            RejectionKind::PreCheck => "pre_check",
        }
    }
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
    /// file, rejection TTL) before deciding. Implementations
    /// return a [`Rejection`] when refusing so the linter can
    /// route the resume hint correctly.
    fn pre_check(&self, _topic: &str, _payload: &Value) -> Result<(), Rejection> {
        Ok(())
    }
}

/// Structured rejection. The `kind` carries the classification;
/// the `message` is human-readable detail shown in logs and to
/// the agent. Prefer this over `Result<(), String>` for any new
/// gate code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub kind: RejectionKind,
    pub message: String,
}

impl Rejection {
    pub fn new(kind: RejectionKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
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
    if let Err(rej) = ctx.pre_check(topic, payload) {
        return GateDecision::Reject {
            kind: rej.kind,
            message: rej.message,
        };
    }
    let required = view.required_fields(topic);
    let missing = missing_fields(&required, payload);
    if !missing.is_empty() {
        return GateDecision::Reject {
            kind: RejectionKind::MissingField,
            message: format!(
                "missing required fields: {}",
                missing.into_iter().collect::<Vec<_>>().join(",")
            ),
        };
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
            macro_edges_resolved: HashSet::new(),
            execution_mode: crate::config::HatExecutionMode::default(),
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
        match decision {
            GateDecision::Reject { kind, message } => {
                assert_eq!(kind, RejectionKind::MissingField);
                assert!(message.contains("plan_name"));
            }
            other => panic!("expected Reject, got {other:?}"),
        }
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

    /// P1-1: reason codes are stable, well-known strings. Operators
    /// rely on them for alert routing; renaming requires a
    /// migration plan.
    #[test]
    fn reason_codes_are_stable() {
        assert_eq!(RejectionKind::MissingField.reason_code(), "missing_field");
        assert_eq!(RejectionKind::TopicOwnership.reason_code(), "topic_ownership");
        assert_eq!(RejectionKind::UpstreamState.reason_code(), "upstream_state");
        assert_eq!(RejectionKind::HandoffArtifact.reason_code(), "handoff_artifact");
        assert_eq!(RejectionKind::PreCheck.reason_code(), "pre_check");
    }

    /// P1-1: the rejection kind drives the linter's failure
    /// class. The mapping is the only cross-layer surface, and
    /// it is defined in one place.
    #[test]
    fn kind_maps_to_lint_class() {
        use crate::preset::engine::hint::LintFailureClass;
        assert_eq!(
            RejectionKind::MissingField.to_lint_class(),
            LintFailureClass::PayloadError
        );
        assert_eq!(
            RejectionKind::UpstreamState.to_lint_class(),
            LintFailureClass::UpstreamStateMissing
        );
        assert_eq!(
            RejectionKind::HandoffArtifact.to_lint_class(),
            LintFailureClass::HandoffArtifact
        );
    }
}