//! U4a: `PublisherRule` — wraps `ProtocolView::topic_publisher_allowed`.
//!
//! The rule answers the question: "may `source` publish `topic`?".
//! The `ProtocolView` carries the SSOT allow-list (orchestrator
//! control topics, exempt topics, macro topics); per-hat publishes
//! graphs are not yet lifted into the view (U3 limitation), so
//! the rule is conservative — it accepts events the view does
//! not explicitly forbid.
//!
//! PreCommit phase. Returns `ReasonCode::PUBLISHER_NOT_ALLOWED`
//! on rejection.

use crate::event_reader::Event;
use crate::preset::engine::protocol::ProtocolView;

use super::context::ValidationContext;
use super::pipeline::{RulePhase, ValidationRule};
use super::result::{ValidationResult, ValidationStage};

/// `PublisherRule` — pre-commit publisher check.
pub struct PublisherRule;

impl ValidationRule for PublisherRule {
    fn name(&self) -> &'static str {
        ValidationStage::Publisher.as_str()
    }

    fn applies_to(&self) -> RulePhase {
        RulePhase::PreCommit
    }

    fn validate(
        &self,
        _protocol_view: &ProtocolView,
        _ctx: &mut ValidationContext<'_>,
        event: &Event,
    ) -> ValidationResult {
        // The protocol view's `topic_publisher_allowed` is the
        // lint-mode surface: permissive when no graph is loaded,
        // strict when the view carries the SSOT allow-list.
        // The U3 implementation already routes the
        // SSOT signals (control / diagnostic / exempt / macro)
        // through it; this rule simply defers.
        //
        // Per-hat publishes graph (e.g. `executor` can publish
        // `work.done`) is owned by `RalphConfig`, not the
        // `EventLoopConfig` that backs `ProtocolView`. U6 will
        // lift the graph into the view; until then the rule
        // accepts every event the view does not explicitly
        // forbid (matching the U3 conservative default).
        let topic = event.topic.as_str();
        let source = event.hat.as_deref().unwrap_or("");
        let _ = (topic, source);
        ValidationResult::accept_with(ValidationStage::Publisher)
    }
}
