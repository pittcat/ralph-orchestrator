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

use std::sync::Arc;

use crate::event_origin::{self, OriginCheck};
use crate::event_reader::Event;
use crate::hat_registry::HatRegistry;
use crate::preset::engine::protocol::ProtocolView;
use crate::state::LedgerSnapshot;

use super::pipeline::{RulePhase, ValidationRule};
use super::result::{ReasonCode, ValidationResult, ValidationStage};

/// `OriginRule` — pre-commit origin guard.
pub struct OriginRule;

impl OriginRule {
    /// Build a registry from the protocol view. The view does
    /// not currently expose a hat registry, so the default empty
    /// registry is used. The runtime is expected to pass a
    /// non-empty registry through the [`super::pipeline::ValidationPipeline`]
    /// builder in production.
    fn registry(_view: &ProtocolView) -> Arc<HatRegistry> {
        // Until `ProtocolView` carries a hat registry (U6
        // wiring), the rule uses an empty registry. The legacy
        // `validate_event_origin` path treats empty registry as
        // solo / hatless mode and accepts all events — exactly
        // the legacy no-hats behaviour the orchestrator has
        // always had.
        Arc::new(HatRegistry::default())
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
        protocol_view: &ProtocolView,
        _ledger_snapshot: &LedgerSnapshot,
        event: &Event,
    ) -> ValidationResult {
        let registry = Self::registry(protocol_view);
        let cancellation_topic = ""; // resolved via `LoopState` in legacy path
        let completion_promise = ""; // same — kept empty for symmetry
        let verdict = event_origin::validate_event_origin(
            event,
            &registry,
            cancellation_topic,
            completion_promise,
        );
        match verdict {
            OriginCheck::Accepted => ValidationResult::accept(),
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