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
    /// 2026-06-23 fix plan P0-2 / P0-1: hat_handoff artifact
    /// filename's `iter` / `seq` does not match
    /// `LoopState::hat_handoff_seq + 1`. Routes to the source
    /// hat so it can regenerate the artifact with the SSOT
    /// filename (currently `current_seq + 1`).
    HandoffFilenameMismatch,
    /// 2026-06-23 fix plan P0-2 / P2-1: hat_handoff artifact
    /// body fails the five-section structural check (missing
    /// section, out-of-order section, missing `## next` field,
    /// `## notes` over 15 words, or antipattern action line).
    /// Routes to the source hat so the agent can rewrite the
    /// artifact body.
    HandoffStructureInvalid,
    /// 2026-06-23 fix plan P0-2 / P1-1: hat_handoff artifact's
    /// `## next` action line references a topic that is not
    /// in the union `from_hat.publishes ∪ from_hat.subscribes_to`.
    /// Routes to the source hat so the agent can rewrite the
    /// `## next` line to a legal action topic.
    HandoffIllegalEmitTopic,
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
            // 2026-06-23 fix: all three hat_handoff_* kinds
            // route back to the source hat (the agent that
            // emitted the event). The error is in the artifact
            // the source hat produced, so the source hat must
            // regenerate it.
            RejectionKind::HandoffFilenameMismatch => LintFailureClass::HandoffArtifact,
            RejectionKind::HandoffStructureInvalid => LintFailureClass::HandoffArtifact,
            RejectionKind::HandoffIllegalEmitTopic => LintFailureClass::HandoffArtifact,
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
            // 2026-06-23 fix: three new hat_handoff reasons
            // carry stable reason_codes so `recovery.jsonl`
            // and `drift_findings` aggregations can distinguish
            // them.
            RejectionKind::HandoffFilenameMismatch => "hat_handoff_filename_mismatch",
            RejectionKind::HandoffStructureInvalid => "hat_handoff_structure_invalid",
            RejectionKind::HandoffIllegalEmitTopic => "hat_handoff_illegal_emit_topic",
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
///
/// `from_hat` is the hat emitting the event; `None` when the caller
/// does not have hat information (e.g. lint phase). Used for self-loop
/// exclusion in the macro-edge handoff-path check.
pub fn run_gates<C: GateContext>(
    view: &ProtocolView,
    ctx: &C,
    topic: &str,
    payload: &Value,
    from_hat: Option<&str>,
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

    // Handoff artifact check: macro edge must carry a non-empty handoff_path.
    if view.is_macro_edge_from(topic, from_hat) && !has_handoff_path(payload) {
        return GateDecision::Reject {
            kind: RejectionKind::HandoffArtifact,
            message: format!(
                "macro edge '{topic}' requires payload field 'handoff_path'; use `ralph tools handoff prepare`"
            ),
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

fn has_handoff_path(payload: &Value) -> bool {
    match payload {
        Value::Object(map) => map
            .get("handoff_path")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        Value::String(s) => crate::hat_handoff::payload::extract_handoff_path(s).is_some(),
        _ => false,
    }
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
            workflow_guards: None,
            state_projection: None,
            execution_contracts: Some(ExecutionContractsConfig::default()),
            event_policy: None,
            hat_handoff: HatHandoffConfig::default(),
            macro_edges_resolved: HashSet::new(),
            macro_edge_consumers: HashMap::new(),
            execution_mode: crate::config::HatExecutionMode::default(),
            protocol_hash: "0".to_string(),
            feature_flag_enabled: false,
        }
    }

    #[test]
    fn accept_when_no_required_fields() {
        let view = empty_view();
        let decision = run_gates(&view, &LintContext, "any", &json!({}), None);
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
        let decision = run_gates(&view, &LintContext, "work.done", &json!({}), None);
        match decision {
            GateDecision::Reject { kind, message } => {
                assert_eq!(kind, RejectionKind::MissingField);
                assert!(message.contains("plan_name"));
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    /// 2026-06-23 fix (adversarial review P1-4): a MissingField
    /// regression that flips the kind to TopicOwnership MUST
    /// trip this test. The P1-4 fix is to assert the *kind*
    /// explicitly (not just the message text), so a kind
    /// change is caught by `cargo nextest`.
    #[test]
    fn reject_when_required_missing_kind_typed() {
        let mut view = empty_view();
        let mut reqs = HashSet::new();
        reqs.insert("plan_name".to_string());
        view.effective_required_fields
            .insert("work.done".to_string(), reqs);
        let decision = run_gates(&view, &LintContext, "work.done", &json!({}), None);
        match decision {
            GateDecision::Reject { kind, .. } => {
                assert_eq!(
                    kind,
                    RejectionKind::MissingField,
                    "missing required fields MUST keep MissingField kind; flipping the kind breaks the typed escalation chain"
                );
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
            None,
        );
        assert_eq!(decision, GateDecision::Accept);
    }

    /// P1-1: handoff artifact rejection — macro edge without
    /// handoff_path is rejected with HandoffArtifact kind.
    #[test]
    fn reject_macro_edge_without_handoff_path() {
        let mut view = empty_view();
        view.execution_mode = crate::config::HatExecutionMode::Isolated;
        view.hat_handoff.enabled = true;
        view.macro_edges_resolved.insert("work.done".to_string());
        let decision = run_gates(
            &view,
            &LintContext,
            "work.done",
            &json!({"plan_name": "x", "step": "s"}),
            None,
        );
        match decision {
            GateDecision::Reject { kind, message } => {
                assert_eq!(kind, RejectionKind::HandoffArtifact);
                assert!(message.contains("handoff_path"));
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    /// 2026-06-23 fix (adversarial review P1-4): explicitly
    /// assert the kind for the macro-edge rejection. The legacy
    /// test only checks the message text, which would let a
    /// kind flip (e.g. from HandoffArtifact to MissingField)
    /// silently slip through `cargo nextest` and break the
    /// typed escalation.
    #[test]
    fn reject_macro_edge_without_handoff_path_kind_typed() {
        let mut view = empty_view();
        view.execution_mode = crate::config::HatExecutionMode::Isolated;
        view.hat_handoff.enabled = true;
        view.macro_edges_resolved.insert("work.done".to_string());
        let decision = run_gates(
            &view,
            &LintContext,
            "work.done",
            &json!({"plan_name": "x", "step": "s"}),
            None,
        );
        match decision {
            GateDecision::Reject { kind, .. } => {
                assert_eq!(
                    kind,
                    RejectionKind::HandoffArtifact,
                    "macro-edge rejection MUST keep HandoffArtifact kind so the typed resume hints reach the source hat"
                );
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    /// P1-1: macro edge with handoff_path is accepted (missing-field
    /// check still runs after the artifact check, but here all fields
    /// are present).
    #[test]
    fn accept_macro_edge_with_handoff_path() {
        let mut view = empty_view();
        view.execution_mode = crate::config::HatExecutionMode::Isolated;
        view.hat_handoff.enabled = true;
        view.macro_edges_resolved.insert("work.done".to_string());
        let decision = run_gates(
            &view,
            &LintContext,
            "work.done",
            &json!({"plan_name": "x", "step": "s", "handoff_path": ".ralph/handoff/test.md"}),
            None,
        );
        assert_eq!(decision, GateDecision::Accept);
    }

    /// P1-1: self-loop exclusion — when from_hat == consumer, the
    /// edge is NOT a macro edge even if the topic is in the resolved set.
    #[test]
    fn accept_self_loop_even_without_handoff_path() {
        let mut view = empty_view();
        view.execution_mode = crate::config::HatExecutionMode::Isolated;
        view.hat_handoff.enabled = true;
        view.macro_edges_resolved.insert("work.ready".to_string());
        view.macro_edge_consumers
            .insert("work.ready".to_string(), "executor".to_string());
        let decision = run_gates(
            &view,
            &LintContext,
            "work.ready",
            &json!({"plan_name": "x"}),
            Some("executor"),
        );
        assert_eq!(decision, GateDecision::Accept);
    }

    /// P1-1: reason codes are stable, well-known strings. Operators
    /// rely on them for alert routing; renaming requires a
    /// migration plan.
    #[test]
    fn reason_codes_are_stable() {
        assert_eq!(RejectionKind::MissingField.reason_code(), "missing_field");
        assert_eq!(
            RejectionKind::TopicOwnership.reason_code(),
            "topic_ownership"
        );
        assert_eq!(RejectionKind::UpstreamState.reason_code(), "upstream_state");
        assert_eq!(
            RejectionKind::HandoffArtifact.reason_code(),
            "handoff_artifact"
        );
        assert_eq!(RejectionKind::PreCheck.reason_code(), "pre_check");
        // 2026-06-23 fix plan: three new hat_handoff reasons
        // carry stable reason_codes that match the historical
        // `recovery.jsonl` strings (`hat_handoff_filename_mismatch`
        // / `hat_handoff_structure_invalid` /
        // `hat_handoff_illegal_emit_topic`). Operators depend
        // on those strings for grep-based aggregations.
        assert_eq!(
            RejectionKind::HandoffFilenameMismatch.reason_code(),
            "hat_handoff_filename_mismatch"
        );
        assert_eq!(
            RejectionKind::HandoffStructureInvalid.reason_code(),
            "hat_handoff_structure_invalid"
        );
        assert_eq!(
            RejectionKind::HandoffIllegalEmitTopic.reason_code(),
            "hat_handoff_illegal_emit_topic"
        );
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

    /// 2026-06-23 fix plan P0-2: the three new
    /// `HandoffFilenameMismatch` / `HandoffStructureInvalid` /
    /// `HandoffIllegalEmitTopic` kinds must route to
    /// `HandoffArtifact` so the typed `LintResumeHint` reaches
    /// the source hat, **not** `PlanGate` (the agent
    /// emitting the artifact is the only hat that can rewrite
    /// it). Without this routing, the typed hint would
    /// silently route to `PlanGate` and the agent would never
    /// learn the failure was in its own artifact body.
    #[test]
    fn p0_2_hat_handoff_kinds_route_to_artifact_class() {
        use crate::preset::engine::hint::LintFailureClass;
        for kind in [
            RejectionKind::HandoffFilenameMismatch,
            RejectionKind::HandoffStructureInvalid,
            RejectionKind::HandoffIllegalEmitTopic,
        ] {
            assert_eq!(
                kind.to_lint_class(),
                LintFailureClass::HandoffArtifact,
                "{kind:?} must map to HandoffArtifact so LintResumeHint routes to source hat"
            );
        }
    }

    /// 2026-06-23 fix (adversarial review P1-2 cross-file
    /// integration): the typed kind MUST drive the resume
    /// hint's `target` to `SourceHat`. This locks the
    /// end-to-end flow that the v1 fix promised but did not
    /// actually verify:
    ///
    ///   `evaluate_event` (hat_handoff::gate) produces `Reject { kind, .. }`
    ///   → `LintResumeHint::from_typed_rejection(topic, kind, msg)`
    ///   → `target == LintResumeTarget::SourceHat`
    ///
    /// Without this assertion, the v1 tests would still pass
    /// even if a future refactor routed the hat_handoff kinds
    /// to `PlanGate` (which would silently break
    /// `primary-20260622-182705`-style fixes because the
    /// coordinator hat would never receive the typed hint).
    #[test]
    fn p1_2_typed_kinds_route_to_source_hat_end_to_end() {
        use crate::preset::engine::hint::{LintFailureClass, LintResumeHint, LintResumeTarget};
        for kind in [
            RejectionKind::HandoffFilenameMismatch,
            RejectionKind::HandoffStructureInvalid,
            RejectionKind::HandoffIllegalEmitTopic,
            RejectionKind::HandoffArtifact,
        ] {
            let hint = LintResumeHint::from_typed_rejection("work.ready", kind, "fixture message");
            assert_eq!(
                hint.class,
                LintFailureClass::HandoffArtifact,
                "{kind:?} must keep the HandoffArtifact class so the typed resume reaches the source hat"
            );
            assert_eq!(
                hint.target,
                LintResumeTarget::SourceHat,
                "{kind:?} must drive target == SourceHat, not PlanGate"
            );
        }
    }

    /// 2026-06-23 fix (adversarial review P1-3): the
    /// `reason_code()` mapping is a public SSOT — operators
    /// grep `.ralph/recovery.jsonl` for these strings. This
    /// test asserts every variant's `reason_code()` is part of
    /// the locked surface so an accidental rename is caught by
    /// `cargo nextest run -p ralph-core -- reason_code_locked`.
    #[test]
    fn p1_3_reason_code_locked_for_all_kinds() {
        // Pair each variant with the expected reason_code() string.
        // Adding a new variant MUST add a matching pair here.
        let cases: &[(RejectionKind, &str)] = &[
            (RejectionKind::MissingField, "missing_field"),
            (RejectionKind::TopicOwnership, "topic_ownership"),
            (RejectionKind::UpstreamState, "upstream_state"),
            (RejectionKind::HandoffArtifact, "handoff_artifact"),
            (RejectionKind::PreCheck, "pre_check"),
            (
                RejectionKind::HandoffFilenameMismatch,
                "hat_handoff_filename_mismatch",
            ),
            (
                RejectionKind::HandoffStructureInvalid,
                "hat_handoff_structure_invalid",
            ),
            (
                RejectionKind::HandoffIllegalEmitTopic,
                "hat_handoff_illegal_emit_topic",
            ),
        ];
        for (kind, expected_code) in cases {
            assert_eq!(
                kind.reason_code(),
                *expected_code,
                "reason_code drifted for {kind:?}"
            );
        }
    }
}
