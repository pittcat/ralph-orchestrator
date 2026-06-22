//! U4a: `RequiredFieldsRule` — wraps `ProtocolView::required_fields_for`.
//!
//! Pre-commit phase. Checks the event payload (a JSON-encoded
//! `String`) against the topic's required-field schema. The
//! resulting `reason_code` follows the existing engine gate
//! convention: `engine_rejected:required_field:<missing>` so the
//! downstream tooling that already parses that prefix continues
//! to work.
//!
//! The rule is intentionally conservative: only structured
//! object payloads are inspected. A non-JSON payload or a
//! payload that fails to parse is **accepted** (the existing
//! execution-contract / payload-contract rules are the
//! authoritative place for malformed-payload handling).

use std::collections::HashSet;

use crate::event_reader::Event;
use crate::preset::engine::protocol::ProtocolView;
use serde_json::Value;

use super::context::ValidationContext;
use super::pipeline::{RulePhase, ValidationRule};
use super::result::{ReasonCode, ValidationResult, ValidationStage};

/// `RequiredFieldsRule` — pre-commit required-fields check.
pub struct RequiredFieldsRule;

impl ValidationRule for RequiredFieldsRule {
    fn name(&self) -> &'static str {
        ValidationStage::RequiredFields.as_str()
    }

    fn applies_to(&self) -> RulePhase {
        RulePhase::PreCommit
    }

    fn validate(
        &self,
        protocol_view: &ProtocolView,
        _ctx: &mut ValidationContext<'_>,
        event: &Event,
    ) -> ValidationResult {
        let required: HashSet<String> = protocol_view
            .required_fields_for(event.topic.as_str())
            .cloned()
            .unwrap_or_default();
        if required.is_empty() {
            return ValidationResult::accept_with(ValidationStage::RequiredFields);
        }

        let payload = match event.payload.as_deref() {
            Some(s) if !s.trim().is_empty() => s,
            // Empty payload + required fields → fail-closed
            // (matching `validate_payload` in `execution_contract`).
            _ => {
                let missing = required.iter().next().cloned().unwrap_or_default();
                return ValidationResult::reject(
                    ValidationStage::RequiredFields,
                    format!("{}:{}", ReasonCode::REQUIRED_FIELD_MISSING, missing),
                    Some(format!(
                        "topic `{}` payload is empty but requires fields {:?}",
                        event.topic, required
                    )),
                    true,
                );
            }
        };

        let parsed: Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            // Malformed JSON: defer to the execution-contract /
            // payload-contract layers; this rule is not the
            // authoritative place for parse failures.
            Err(_) => return ValidationResult::accept_with(ValidationStage::RequiredFields),
        };

        let Value::Object(map) = parsed else {
            // Non-object payload (string / array): the engine
            // schema assumes an object; a non-object payload
            // cannot satisfy any required-field check, so
            // reject. This matches `validate_payload` semantics.
            let missing = required.iter().next().cloned().unwrap_or_default();
            return ValidationResult::reject(
                ValidationStage::RequiredFields,
                format!("{}:{}", ReasonCode::REQUIRED_FIELD_MISSING, missing),
                Some(format!(
                    "topic `{}` payload must be a JSON object to satisfy required fields {:?}",
                    event.topic, required
                )),
                true,
            );
        };

        for field in &required {
            if !map.contains_key(field) {
                return ValidationResult::reject(
                    ValidationStage::RequiredFields,
                    format!("{}:{}", ReasonCode::REQUIRED_FIELD_MISSING, field),
                    Some(format!(
                        "topic `{}` payload is missing required field `{field}`",
                        event.topic
                    )),
                    true,
                );
            }
        }

        ValidationResult::accept_with(ValidationStage::RequiredFields)
    }
}
