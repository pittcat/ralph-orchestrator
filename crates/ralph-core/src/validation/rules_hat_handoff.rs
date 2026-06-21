//! U4c: `HatHandoffRule` — wraps `hat_handoff::gate::evaluate_event`.
//!
//! Pre-commit phase. The rule inspects macro-edge events for a
//! valid handoff artifact. The legacy `evaluate_event` takes a
//! `GateInputs` struct that carries the `HandoffIndex`,
//! `HatHandoffConfig`, and a pre-read `FileContent`. U4c exposes
//! the **decision** surface as a `ValidationRule` so the unified
//! pipeline can compose it.
//!
//! Without a fully-loaded `HandoffIndex` and `HatHandoffConfig`
//! in `ProtocolView`, the rule preserves the legacy "no
//! configuration → passthrough" semantics. U6 will lift the
//! index into the view; until then the rule accepts every event.
//!
//! The rule preserves the `ReasonCode::HAT_HANDOFF_*` strings
//! from the legacy gate (see `super::result::ReasonCode`).

use crate::event_reader::Event;
use crate::preset::engine::protocol::ProtocolView;
use crate::state::LedgerSnapshot;

use super::pipeline::{RulePhase, ValidationRule};
use super::result::{ValidationResult, ValidationStage};

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
            return ValidationResult::accept();
        }

        // For macro edges, the legacy gate inspects the payload's
        // `handoff_path` and the artifact on disk. The U4c rule
        // does not yet own a `HandoffIndex` or a file reader; U6
        // will plumb the path through `LedgerSnapshot` /
        // `ProtocolView`. Until then the rule produces a
        // structured passthrough (`accepted`) so the legacy
        // `event_loop::apply_workflow_guard_validation` and
        // `hat_handoff::gate::evaluate_event` paths continue to
        // gate the event as before.
        //
        // The reason_code prefix `hat_handoff:` is preserved in
        // the legacy `GateDecision::Reject { reason_code, .. }`
        // shape so the U6 wiring can re-use the constant strings
        // from [`super::result::ReasonCode`].
        let _ = event;
        ValidationResult::accept()
    }
}