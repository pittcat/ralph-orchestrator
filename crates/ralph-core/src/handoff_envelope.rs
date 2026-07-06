//! 2026-07-06-004 plan U2: typed Handoff Envelope payload
//! validation.
//!
//! This module owns the in-memory view of the `handoff_envelope`
//! field that business event payloads carry under serial preset.
//! It is the single source of truth for the schema layout — the
//! event policy SSOT in `presets/schemas/ce-executor-serial.yml`
//! only requires the *top-level* `handoff_envelope` field, but
//! the nested contracts (root_goal, receiver_contract.to_hat,
//! success/failure signal, etc.) are validated here so the agent
//! prompt and the policy-check pipeline can agree on what a valid
//! handoff looks like.
//!
//! The module is deliberately decoupled from policy-check, event
//! policy, and the prompt builder. U2 only guarantees that any
//! payload whose top-level shape matches the JSON contract can be
//! validated independently. U8 wires policy-check, U6 wires
//! prompt injection, and U9 wires `EmitResult` summary.

use serde::{Deserialize, Serialize};

/// Stable schema version identifier. Bumping this string is the
/// only way to evolve the contract: any payload whose
/// `schema_version` differs is rejected with
/// `handoff_envelope_invalid_schema_version`.
pub const HANDOFF_ENVELOPE_SCHEMA_VERSION: &str = "handoff-envelope.v1";

/// Top-level typed view of the `handoff_envelope` field on a
/// business event payload.
///
/// The struct shape mirrors the JSON contract in the plan's
/// "最终 payload 形态" section. Field-level validation rules:
///
/// * `schema_version` must equal `HANDOFF_ENVELOPE_SCHEMA_VERSION`.
/// * `root_goal` must be a non-empty string.
/// * `plan` is required; its `name`, `path`, `current_step`
///   fields must be non-empty.
/// * `state` is required; `current_status` and `last_signal`
///   must be non-empty strings. `blocking_reason` may be `None`.
/// * `receiver_contract.to_hat`, `success_signal`,
///   `failure_signal` must be non-empty strings.
/// * `receiver_contract.must_do` must contain at least one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffEnvelopePayload {
    pub schema_version: String,
    pub root_goal: String,
    pub plan: HandoffEnvelopePlan,
    pub state: HandoffEnvelopeState,
    pub receiver_contract: HandoffEnvelopeReceiverContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffEnvelopePlan {
    pub name: String,
    pub path: String,
    pub current_step: String,
    #[serde(default)]
    pub completed_steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffEnvelopeState {
    pub current_status: String,
    pub last_signal: String,
    #[serde(default)]
    pub blocking_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffEnvelopeReceiverContract {
    pub to_hat: String,
    #[serde(default)]
    pub must_do: Vec<String>,
    #[serde(default)]
    pub must_not_do: Vec<String>,
    pub success_signal: String,
    pub failure_signal: String,
}

/// Stable error envelope for the validator. `code` is a
/// machine-readable stable string; `message` is human readable.
///
/// Stable codes (U8 wires these into policy-check validation
/// reports):
/// * `handoff_envelope_missing`
/// * `handoff_envelope_invalid_schema_version`
/// * `handoff_envelope_missing_root_goal`
/// * `handoff_envelope_missing_plan`
/// * `handoff_envelope_missing_state`
/// * `handoff_envelope_missing_receiver_contract`
/// * `handoff_envelope_missing_success_signal`
/// * `handoff_envelope_missing_failure_signal`
/// * `handoff_envelope_missing_to_hat`
/// * `handoff_envelope_must_do_empty`
/// * `handoff_envelope_invalid_payload`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffEnvelopeValidationError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for HandoffEnvelopeValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for HandoffEnvelopeValidationError {}

/// Validate a JSON `Value` carrying a top-level `handoff_envelope`
/// field. Returns the typed view on success, or the first
/// validation error on failure.
///
/// The validator intentionally does NOT inspect anything above the
/// top-level `handoff_envelope` key — that's policy-check's job.
/// The contract here is "given a `handoff_envelope` object, is it
/// shaped correctly per `HANDOFF_ENVELOPE_SCHEMA_VERSION`?"
pub fn validate_handoff_envelope_payload(
    value: &serde_json::Value,
) -> Result<HandoffEnvelopePayload, HandoffEnvelopeValidationError> {
    let obj = value.as_object().ok_or_else(|| HandoffEnvelopeValidationError {
        code: "handoff_envelope_invalid_payload",
        message: "handoff_envelope must be a JSON object".to_string(),
    })?;

    let payload_value = obj.get("handoff_envelope").ok_or_else(|| HandoffEnvelopeValidationError {
        code: "handoff_envelope_missing",
        message: "payload is missing top-level field `handoff_envelope`".to_string(),
    })?;

    let payload_obj = payload_value.as_object().ok_or_else(|| HandoffEnvelopeValidationError {
        code: "handoff_envelope_invalid_payload",
        message: "handoff_envelope must be a JSON object".to_string(),
    })?;

    // Manually pull required scalar fields first so each missing
    // field maps to a stable `code` (rather than serde's generic
    // "missing field" message). Then parse into the typed struct
    // for shape validation. This dual-pass keeps error codes
    // stable for U8's policy-check wiring.
    let schema_version = payload_obj
        .get("schema_version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HandoffEnvelopeValidationError {
            code: "handoff_envelope_invalid_schema_version",
            message: "schema_version must be a string".to_string(),
        })?
        .to_string();

    if schema_version != HANDOFF_ENVELOPE_SCHEMA_VERSION {
        return Err(HandoffEnvelopeValidationError {
            code: "handoff_envelope_invalid_schema_version",
            message: format!(
                "schema_version must be {} (got {:?})",
                HANDOFF_ENVELOPE_SCHEMA_VERSION, schema_version
            ),
        });
    }

    let root_goal = payload_obj
        .get("root_goal")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_root_goal",
            message: "root_goal must be a string".to_string(),
        })?
        .to_string();

    if root_goal.trim().is_empty() {
        return Err(HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_root_goal",
            message: "root_goal must be a non-empty string".to_string(),
        });
    }

    let contract_obj = payload_obj
        .get("receiver_contract")
        .and_then(|v| v.as_object())
        .ok_or_else(|| HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_receiver_contract",
            message: "receiver_contract must be an object".to_string(),
        })?;

    let to_hat = contract_obj
        .get("to_hat")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_to_hat",
            message: "receiver_contract.to_hat must be a string".to_string(),
        })?
        .to_string();

    if to_hat.trim().is_empty() {
        return Err(HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_to_hat",
            message: "receiver_contract.to_hat must be a non-empty string".to_string(),
        });
    }

    let must_do = contract_obj
        .get("must_do")
        .and_then(|v| v.as_array())
        .ok_or_else(|| HandoffEnvelopeValidationError {
            code: "handoff_envelope_must_do_empty",
            message: "receiver_contract.must_do must be an array".to_string(),
        })?
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect::<Vec<_>>();

    if must_do.is_empty() {
        return Err(HandoffEnvelopeValidationError {
            code: "handoff_envelope_must_do_empty",
            message: "receiver_contract.must_do must contain at least one entry".to_string(),
        });
    }

    let success_signal = contract_obj
        .get("success_signal")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_success_signal",
            message: "receiver_contract.success_signal must be a string".to_string(),
        })?
        .to_string();

    if success_signal.trim().is_empty() {
        return Err(HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_success_signal",
            message: "receiver_contract.success_signal must be a non-empty string".to_string(),
        });
    }

    let failure_signal = contract_obj
        .get("failure_signal")
        .and_then(|v| v.as_str())
        .ok_or_else(|| HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_failure_signal",
            message: "receiver_contract.failure_signal must be a string".to_string(),
        })?
        .to_string();

    if failure_signal.trim().is_empty() {
        return Err(HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_failure_signal",
            message: "receiver_contract.failure_signal must be a non-empty string".to_string(),
        });
    }

    // Plan + state + remaining contract fields go through the
    // typed struct for shape validation. By this point the
    // contract-required signals are already locked.
    let parsed: HandoffEnvelopePayload =
        serde_json::from_value(payload_value.clone()).map_err(|e| HandoffEnvelopeValidationError {
            code: "handoff_envelope_invalid_payload",
            message: format!("handoff_envelope shape mismatch: {}", e),
        })?;

    if parsed.plan.name.trim().is_empty() {
        return Err(HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_plan",
            message: "plan.name must be a non-empty string".to_string(),
        });
    }

    if parsed.plan.path.trim().is_empty() {
        return Err(HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_plan",
            message: "plan.path must be a non-empty string".to_string(),
        });
    }

    if parsed.plan.current_step.trim().is_empty() {
        return Err(HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_plan",
            message: "plan.current_step must be a non-empty string".to_string(),
        });
    }

    if parsed.state.current_status.trim().is_empty() {
        return Err(HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_state",
            message: "state.current_status must be a non-empty string".to_string(),
        });
    }

    if parsed.state.last_signal.trim().is_empty() {
        return Err(HandoffEnvelopeValidationError {
            code: "handoff_envelope_missing_state",
            message: "state.last_signal must be a non-empty string".to_string(),
        });
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    //! 2026-07-06-004 plan U2 RED tests. These tests exercise
    //! `validate_handoff_envelope_payload` against minimal JSON
    //! literals only — no EventPolicy, no preset, no runtime.

    use super::*;
    use serde_json::json;

    fn full_payload() -> serde_json::Value {
        json!({
            "plan_name": "2026-07-06-example",
            "plan_path": "docs/plans/2026-07-06-example.md",
            "task_id": "task-live-id",
            "task_key": "2026-07-06-example:step-3:implement",
            "step": "step-3",
            "handoff_envelope": {
                "schema_version": HANDOFF_ENVELOPE_SCHEMA_VERSION,
                "root_goal": "implement the requested feature without regressions",
                "plan": {
                    "name": "2026-07-06-example",
                    "path": "docs/plans/2026-07-06-example.md",
                    "current_step": "step-3",
                    "completed_steps": ["step-1", "step-2"]
                },
                "state": {
                    "current_status": "ready_for_review",
                    "last_signal": "work.done",
                    "blocking_reason": null
                },
                "receiver_contract": {
                    "to_hat": "goal-alignment-reviewer",
                    "must_do": ["review goal alignment for the current unit"],
                    "must_not_do": ["modify source code"],
                    "success_signal": "review.dimension.passed",
                    "failure_signal": "review.dimension.failed"
                }
            }
        })
    }

    #[test]
    fn valid_payload_deserializes() {
        let payload = full_payload();
        let parsed =
            validate_handoff_envelope_payload(&payload).expect("full payload must validate");
        assert_eq!(parsed.schema_version, HANDOFF_ENVELOPE_SCHEMA_VERSION);
        assert_eq!(parsed.root_goal, "implement the requested feature without regressions");
        assert_eq!(parsed.plan.current_step, "step-3");
        assert_eq!(parsed.plan.completed_steps, vec!["step-1", "step-2"]);
        assert_eq!(parsed.state.current_status, "ready_for_review");
        assert_eq!(parsed.receiver_contract.to_hat, "goal-alignment-reviewer");
        assert_eq!(parsed.receiver_contract.success_signal, "review.dimension.passed");
        assert_eq!(parsed.receiver_contract.failure_signal, "review.dimension.failed");
        assert_eq!(
            parsed.receiver_contract.must_not_do,
            vec!["modify source code".to_string()]
        );
    }

    #[test]
    fn missing_handoff_envelope_is_rejected() {
        let payload = json!({
            "plan_name": "2026-07-06-example",
            "task_id": "task-live-id"
        });
        let err = validate_handoff_envelope_payload(&payload)
            .expect_err("missing handoff_envelope must reject");
        assert_eq!(err.code, "handoff_envelope_missing");
    }

    #[test]
    fn wrong_schema_version_is_rejected() {
        let mut payload = full_payload();
        payload["handoff_envelope"]["schema_version"] = json!("handoff-envelope.v0");
        let err = validate_handoff_envelope_payload(&payload)
            .expect_err("wrong schema version must reject");
        assert_eq!(err.code, "handoff_envelope_invalid_schema_version");
        assert!(err.message.contains("handoff-envelope.v1"));
    }

    #[test]
    fn missing_receiver_success_signal_is_rejected() {
        let mut payload = full_payload();
        let envelope = payload
            .get_mut("handoff_envelope")
            .expect("fixture must have envelope")
            .as_object_mut()
            .expect("envelope must be object");
        let contract = envelope
            .get_mut("receiver_contract")
            .expect("fixture must have contract")
            .as_object_mut()
            .expect("contract must be object");
        contract.remove("success_signal");

        let err = validate_handoff_envelope_payload(&payload)
            .expect_err("missing success_signal must reject");
        assert_eq!(err.code, "handoff_envelope_missing_success_signal");
    }

    #[test]
    fn missing_receiver_failure_signal_is_rejected() {
        let mut payload = full_payload();
        let envelope = payload
            .get_mut("handoff_envelope")
            .expect("fixture must have envelope")
            .as_object_mut()
            .expect("envelope must be object");
        let contract = envelope
            .get_mut("receiver_contract")
            .expect("fixture must have contract")
            .as_object_mut()
            .expect("contract must be object");
        contract.remove("failure_signal");

        let err = validate_handoff_envelope_payload(&payload)
            .expect_err("missing failure_signal must reject");
        assert_eq!(err.code, "handoff_envelope_missing_failure_signal");
    }

    #[test]
    fn empty_must_do_is_rejected() {
        let mut payload = full_payload();
        payload["handoff_envelope"]["receiver_contract"]["must_do"] = json!([]);
        let err = validate_handoff_envelope_payload(&payload)
            .expect_err("empty must_do must reject");
        assert_eq!(err.code, "handoff_envelope_must_do_empty");
    }

    #[test]
    fn empty_must_not_do_is_allowed() {
        // must_not_do can be empty; that's not a violation.
        let mut payload = full_payload();
        payload["handoff_envelope"]["receiver_contract"]["must_not_do"] = json!([]);
        let parsed =
            validate_handoff_envelope_payload(&payload).expect("empty must_not_do must validate");
        assert!(parsed.receiver_contract.must_not_do.is_empty());
    }

    #[test]
    fn blank_root_goal_is_rejected() {
        let mut payload = full_payload();
        payload["handoff_envelope"]["root_goal"] = json!("   ");
        let err = validate_handoff_envelope_payload(&payload)
            .expect_err("blank root_goal must reject");
        assert_eq!(err.code, "handoff_envelope_missing_root_goal");
    }

    #[test]
    fn non_object_payload_is_rejected() {
        let payload = json!("not an object");
        let err = validate_handoff_envelope_payload(&payload)
            .expect_err("non-object payload must reject");
        assert_eq!(err.code, "handoff_envelope_invalid_payload");
    }

    #[test]
    fn non_object_envelope_is_rejected() {
        let payload = json!({"handoff_envelope": "string-not-object"});
        let err = validate_handoff_envelope_payload(&payload)
            .expect_err("non-object envelope must reject");
        assert_eq!(err.code, "handoff_envelope_invalid_payload");
    }
}