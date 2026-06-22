//! U4a: `OriginRule` — wraps `event_origin::validate_event_origin`.
//!
//! The rule preserves the existing `OriginVerdict` semantics so
//! the unified pipeline emits identical `reason_code` strings to
//! the legacy code path:
//!
//! * `origin:ralph_control_only` (U2)
//! * `origin:unknown_hat`
//! * `origin:out_of_scope`
//! * `origin:control_topic` (passthrough marker, kept for symmetry)
//!
//! PreCommit phase: origin guard needs the *current* snapshot
//! only to identify `current_isolated_hat` (used by the legacy
//! `is_anonymous_business_topic` check). Until the runtime
//! passes that field through `LedgerSnapshot`, the rule relies on
//! the event's own `hat` field (legacy `process_parse_result`
//! path also accepts no-hat business events).
//!
//! P1-#4 (002-adversarial-review): the previous unit-struct
//! `OriginRule` derived an empty `HatRegistry` at every call,
//! which made `event_origin::validate_event_origin` fall back to
//! the solo / hatless mode and accept every event. The rule is
//! now generic over the registry: callers that need real
//! per-hat origin enforcement construct the rule with
//! [`OriginRule::with_registry`] and pass an
//! `Arc<HatRegistry>` built from the runtime's `RalphConfig`.

use std::sync::Arc;

use crate::event_origin::{self, OriginCheck};
use crate::event_reader::Event;
use crate::hat_registry::HatRegistry;
use crate::preset::engine::protocol::ProtocolView;
use crate::state::LedgerSnapshot;

use super::pipeline::{RulePhase, ValidationRule};
use super::result::{ReasonCode, ValidationResult, ValidationStage};

/// `OriginRule` — pre-commit origin guard.
///
/// The unit struct `OriginRule` defaults to an empty registry
/// (the legacy solo / hatless mode). Production callers that
/// have a real `HatRegistry` should construct
/// `OriginRule::with_registry(arc_registry)` so unknown-hat
/// events are rejected.
pub struct OriginRule {
    registry: Arc<HatRegistry>,
}

impl Default for OriginRule {
    fn default() -> Self {
        Self {
            registry: Arc::new(HatRegistry::default()),
        }
    }
}

impl OriginRule {
    /// Build an `OriginRule` with the supplied `HatRegistry`.
    /// The registry is `Arc`-shared so the rule can be cloned
    /// into multiple pipelines without re-reading the config.
    pub fn with_registry(registry: Arc<HatRegistry>) -> Self {
        Self { registry }
    }
}

impl ValidationRule for OriginRule {
    fn name(&self) -> &'static str {
        ValidationStage::Origin.as_str()
    }

    fn applies_to(&self) -> RulePhase {
        RulePhase::PreCommit
    }

    fn validate(
        &self,
        _protocol_view: &ProtocolView,
        _ledger_snapshot: &LedgerSnapshot,
        event: &Event,
    ) -> ValidationResult {
        let cancellation_topic = ""; // resolved via `LoopState` in legacy path
        let completion_promise = ""; // same — kept empty for symmetry
        let verdict = event_origin::validate_event_origin(
            event,
            &self.registry,
            cancellation_topic,
            completion_promise,
        );
        match verdict {
            OriginCheck::Accepted => ValidationResult::accept_with(ValidationStage::Origin),
            OriginCheck::Rejected { reason, .. } => {
                let stage = ValidationStage::Origin;
                let code = match reason {
                    "ralph_control_only" => ReasonCode::RALPH_CONTROL_ONLY.to_string(),
                    "unknown hat rejected" => ReasonCode::ORIGIN_UNKNOWN_HAT.to_string(),
                    "out-of-scope topic for declared hat" => {
                        ReasonCode::ORIGIN_OUT_OF_SCOPE.to_string()
                    }
                    other => format!("origin:{other}"),
                };
                let retry_eligible = matches!(reason, "ralph_control_only");
                ValidationResult::reject(stage, code, None, retry_eligible)
            }
        }
    }
}
