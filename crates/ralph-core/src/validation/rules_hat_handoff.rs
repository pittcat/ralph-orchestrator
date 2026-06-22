//! U4c: `HatHandoffRule` — wraps `hat_handoff::gate::evaluate_event`.
//!
//! Pre-commit phase. The rule inspects macro-edge events for a
//! valid handoff artifact. The legacy `evaluate_event` takes a
//! `GateInputs` struct that carries the `HandoffIndex`,
//! `HatHandoffConfig`, and a pre-read `FileContent`. U4c exposes
//! the **decision** surface as a `ValidationRule` so the unified
//! pipeline can compose it.
//!
//! The rule surfaces macro-edge handoffs **without bypassing
//! the gate**: when `ProtocolView::is_macro_edge(topic)` is
//! `true`, the rule inspects the event payload for a
//! `handoff_path` field and rejects the event with
//! `hat_handoff:missing_path` if the field is missing. When the
//! field is present, the rule accepts the event so the legacy
//! `hat_handoff::gate::evaluate_event` path can run the deeper
//! validator (artifact-on-disk check) — the unified rule is a
//! thin pre-filter, not a replacement.
//!
//! P1-#3 (002-adversarial-review): the previous version
//! produced a structured passthrough for *every* macro-edge
//! event, which silently disabled hat-handoff validation
//! whenever the unified pipeline was enabled. The new version
//! at least catches the most common bug (macro-edge event with
//! no `handoff_path` in the payload).
//!
//! The rule preserves the `ReasonCode::HAT_HANDOFF_*` strings
//! from the legacy gate (see `super::result::ReasonCode`).

use crate::event_reader::Event;
use crate::preset::engine::protocol::ProtocolView;
use crate::state::LedgerSnapshot;

use super::pipeline::{RulePhase, ValidationRule};
use super::result::{ReasonCode, ValidationResult, ValidationStage};

/// `HatHandoffRule` — pre-commit hat-handoff gate.
pub struct HatHandoffRule;

impl ValidationRule for HatHandoffRule {
    fn name(&self) -> &'static str {
        ValidationStage::HatHandoff.as_str()
    }

    fn applies_to(&self) -> RulePhase {
        RulePhase::PreCommit
    }

    fn validate(
        &self,
        protocol_view: &ProtocolView,
        _ledger_snapshot: &LedgerSnapshot,
        event: &Event,
    ) -> ValidationResult {
        // The rule's macro-edge check is `ProtocolView::is_macro_edge(topic)`.
        // For non-macro topics the gate is `NotRequired` and the
        // event passes through unchanged.
        let topic = event.topic.as_str();
        if !protocol_view.is_macro_edge(topic) {
            return ValidationResult::accept_with(ValidationStage::HatHandoff);
        }

        // P1-#3 (002-adversarial-review): inspect the payload
        // for a `handoff_path` field. Missing-field rejections
        // are a known class of agent bugs (e.g. the agent
        // forgot to put the handoff file in the registry and
        // emitted a bare `handoff.accepted`). The deeper
        // artifact-on-disk validator still runs in the legacy
        // `hat_handoff::gate::evaluate_event` path; this rule
        // is a thin pre-filter that catches the cheapest
        // failure mode first.
        let payload = event.payload.as_deref().unwrap_or("");
        let parsed: serde_json::Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(_) => {
                // Malformed payload: defer to the legacy gate
                // (which has its own malformed-payload
                // handling). The thin pre-filter cannot
                // distinguish "intentionally empty" from
                // "actually missing", and rejecting here would
                // double-fault.
                return ValidationResult::accept_with(ValidationStage::HatHandoff);
            }
        };
        let handoff_path = parsed
            .get("handoff_path")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match handoff_path {
            Some(p) if !p.is_empty() => {
                tracing::debug!(
                    topic = topic,
                    handoff_path = p,
                    "U4c: macro-edge event carries handoff_path; legacy gate will run the deeper check"
                );
                ValidationResult::accept_with(ValidationStage::HatHandoff)
            }
            _ => {
                let hint = format!(
                    "topic `{topic}` is a macro-edge but the event payload has no non-empty `handoff_path` field; the agent must populate it before re-emitting"
                );
                ValidationResult::reject(
                    ValidationStage::HatHandoff,
                    ReasonCode::HAT_HANDOFF_MISSING_PATH,
                    Some(hint),
                    true,
                )
            }
        }
    }
}
