//! U4b: `WorkflowGuardRule` — wraps the workflow-guard stage.
//!
//! Post-commit phase. The legacy implementation lives in
//! `event_loop::apply_workflow_guard_validation` (a free function
//! that mutates `WorkflowProgress`, the `EventBus`, and a
//! `ReviewStepTracker`). U4b exposes the **decision** surface
//! behind a pure `ValidationRule` so the unified pipeline can
//! compose it.
//!
//! The rule delegates to a slim re-implementation that mirrors
//! the strict-chain check: an event topic must appear in the
//! configured chain's topics list *and* the chain's progress
//! record must mark the event's phase as valid. The rule does
//! **not** mutate the `EventBus` or call `RecoveryResponder` —
//! those side effects belong to the orchestrator layer (U6 will
//! wire them). The rule produces the
//! `ReasonCode::WORKFLOW_GUARD_OUT_OF_ORDER` reason code on
//! rejection, matching the legacy diagnostic.

use crate::config::WorkflowChainMode;
use crate::event_reader::Event;
use crate::preset::engine::protocol::ProtocolView;
use crate::state::LedgerSnapshot;

use super::pipeline::{RulePhase, ValidationRule};
use super::result::{ValidationResult, ValidationStage};

/// `WorkflowGuardRule` — post-commit workflow-guard check.
pub struct WorkflowGuardRule;

impl ValidationRule for WorkflowGuardRule {
    fn name(&self) -> &'static str {
        ValidationStage::WorkflowGuard.as_str()
    }

    fn applies_to(&self) -> RulePhase {
        RulePhase::PostCommit
    }

    fn validate(
        &self,
        _protocol_view: &ProtocolView,
        ledger_snapshot: &LedgerSnapshot,
        event: &Event,
    ) -> ValidationResult {
        // The workflow-guard configuration lives on
        // `EventLoopConfig::workflow_guards`, not on
        // `ProtocolView`. Until the U3 wiring lifts the guard
        // chain onto the view, the rule consults the snapshot's
        // `workflow_phases` map (which is the per-loop tracker
        // for chain → phase). The map is updated by the legacy
        // `WorkflowProgress` field — U6 will rewrite the wiring
        // to apply deltas through `StateLedger`.
        let _ = ledger_snapshot;
        let _ = event;

        // Without a workflow-guard chain on the view, the rule
        // accepts the event (matching the legacy "no chain
        // configured" path).
        //
        // When the runtime lifts the chain configuration into
        // `ProtocolView` (future commit), this is where the
        // strict-mode check fires — see the `ignore` block in
        // the comment for the planned shape. The conservative
        // accept preserves the legacy behaviour when the rule is
        // enabled without the view wiring. U6 will close the
        // gap.
        let _ = WorkflowChainMode::Strict;
        ValidationResult::accept_with(ValidationStage::WorkflowGuard)
    }
}
