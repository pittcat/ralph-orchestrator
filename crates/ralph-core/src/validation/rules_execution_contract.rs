//! U4b: `ExecutionContractRule` — wraps `execution_contract::validate_execution_contract`.
//!
//! Post-commit phase. The contract rule needs the *projected*
//! snapshot (tasks closed, progress advanced) to verify a
//! `work.done` event's payload against the SSOT
//! `ExecutionContractsConfig`.
//!
//! The rule preserves the existing `reason_code` surface from the
//! legacy path. The existing
//! `ExecutionContractViolationKind` enum maps one-to-one onto
//! the `ReasonCode` constants in [`super::result`].

use crate::event_reader::Event;
use crate::execution_contract::{self, ExecutionContractDecision, ExecutionContractViolationKind};
use crate::preset::engine::protocol::ProtocolView;
use crate::state::LedgerSnapshot;

use super::context::ValidationContext;
use super::pipeline::{RulePhase, ValidationRule};
use super::result::{ReasonCode, RejectionHint, ValidationResult, ValidationStage};

/// `ExecutionContractRule` — post-commit execution-contract check.
pub struct ExecutionContractRule;

impl ValidationRule for ExecutionContractRule {
    fn name(&self) -> &'static str {
        ValidationStage::ExecutionContract.as_str()
    }

    fn applies_to(&self) -> RulePhase {
        RulePhase::PostCommit
    }

    fn validate(
        &self,
        protocol_view: &ProtocolView,
        ctx: &mut ValidationContext<'_>,
        event: &Event,
    ) -> ValidationResult {
        // The rule only fires when the runtime has a contract
        // configured. `ProtocolView::execution_contracts` is the
        // SSOT; `enabled = false` short-circuits.
        let Some(contracts) = protocol_view.execution_contracts.as_ref() else {
            return ValidationResult::accept_with(ValidationStage::ExecutionContract);
        };
        if !contracts.enabled {
            return ValidationResult::accept_with(ValidationStage::ExecutionContract);
        }
        let Some(rule) = contracts.rules.get(event.topic.as_str()) else {
            return ValidationResult::accept_with(ValidationStage::ExecutionContract);
        };

        // Post-commit contract validation requires reading the
        // task ledger from disk (`TaskStore::load`) — the rule
        // therefore needs a workspace path. The legacy path
        // threads `workspace_root` + `tasks_path` through
        // `process_parse_result`. The pipeline currently does
        // not own either, so the rule falls back to a snapshot-
        // only check: it verifies required payload fields and
        // produces the same `reason_code` as the legacy
        // `validate_payload` helper. Heavier checks (task store
        // roundtrip, git evidence) remain in the legacy
        // execution-contract path until U6 wires the workspace
        // path through the pipeline.
        let decision = execution_contract_check(rule, event, ctx.snapshot());
        match decision {
            ExecutionContractDecision::Accept => {
                ValidationResult::accept_with(ValidationStage::ExecutionContract)
            }
            ExecutionContractDecision::Reject(findings) => {
                // Pick the first finding; the legacy path emits
                // one diagnostic per finding so the unified
                // pipeline will follow suit in U6.
                let finding = findings.into_iter().next().expect("non-empty rejection");
                let stage = ValidationStage::ExecutionContract;
                let (code, hint) = map_finding(finding.kind);
                ValidationResult::reject(stage, code, Some(hint), true)
            }
        }
    }
}

/// Snapshot-only contract check. Reuses the legacy
/// `validate_payload` helper via the public surface — the heavier
/// task / git / test checks stay in the legacy path until U6
/// plumbs the workspace through `LedgerSnapshot`.
fn execution_contract_check(
    rule: &crate::config::ExecutionContractRule,
    event: &Event,
    _snapshot: &LedgerSnapshot,
) -> ExecutionContractDecision {
    let payload_str = event.payload.as_deref().unwrap_or("");
    if payload_str.trim().is_empty() {
        if rule.require_payload_fields.is_empty() {
            return ExecutionContractDecision::Accept;
        }
        let field = rule
            .require_payload_fields
            .first()
            .cloned()
            .unwrap_or_default();
        return ExecutionContractDecision::Reject(vec![
            execution_contract::ExecutionContractFinding {
                kind: ExecutionContractViolationKind::MissingPayloadField { field },
                message: format!(
                    "{} payload is empty but contract requires fields: {:?}",
                    event.topic, rule.require_payload_fields
                ),
                topic: event.topic.to_string(),
                source_hat: None,
            },
        ]);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload_str) else {
        return ExecutionContractDecision::Reject(vec![
            execution_contract::ExecutionContractFinding {
                kind: ExecutionContractViolationKind::InvalidPayload,
                message: format!("{} payload is not valid JSON", event.topic),
                topic: event.topic.to_string(),
                source_hat: None,
            },
        ]);
    };
    let serde_json::Value::Object(map) = value else {
        return ExecutionContractDecision::Reject(vec![
            execution_contract::ExecutionContractFinding {
                kind: ExecutionContractViolationKind::InvalidPayload,
                message: format!("{} payload must be a JSON object", event.topic),
                topic: event.topic.to_string(),
                source_hat: None,
            },
        ]);
    };
    for field in &rule.require_payload_fields {
        if !map.contains_key(field) {
            return ExecutionContractDecision::Reject(vec![
                execution_contract::ExecutionContractFinding {
                    kind: ExecutionContractViolationKind::MissingPayloadField {
                        field: field.clone(),
                    },
                    message: format!(
                        "{} payload is missing required field: '{}'",
                        event.topic, field
                    ),
                    topic: event.topic.to_string(),
                    source_hat: None,
                },
            ]);
        }
    }
    ExecutionContractDecision::Accept
}

fn map_finding(kind: ExecutionContractViolationKind) -> (String, String) {
    match kind {
        ExecutionContractViolationKind::MissingPayloadField { field } => (
            ReasonCode::CONTRACT_MISSING_TASK_ID.to_string(),
            format!("payload is missing required field `{field}`"),
        ),
        ExecutionContractViolationKind::TaskNotFound { task_id } => (
            ReasonCode::CONTRACT_TASK_NOT_FOUND.to_string(),
            format!("task `{task_id}` not found in task store"),
        ),
        ExecutionContractViolationKind::TaskNotTerminal { task_id, .. } => (
            ReasonCode::CONTRACT_TASK_NOT_TERMINAL.to_string(),
            RejectionHint::task_not_terminal(&task_id),
        ),
        ExecutionContractViolationKind::InvalidPayload => (
            ReasonCode::CONTRACT_INVALID_PAYLOAD.to_string(),
            "payload is not a valid JSON object".to_string(),
        ),
        ExecutionContractViolationKind::NoGitEvidence { .. } => (
            ReasonCode::CONTRACT_NO_GIT_EVIDENCE.to_string(),
            "requires git evidence before downstream review can proceed".to_string(),
        ),
        ExecutionContractViolationKind::NoTestEvidence { field } => (
            ReasonCode::CONTRACT_NO_TEST_EVIDENCE.to_string(),
            format!("payload is missing or empty test evidence field `{field}`"),
        ),
        ExecutionContractViolationKind::TaskWrongLoop {
            task_id,
            expected_loop,
            actual_loop,
        } => (
            ReasonCode::CONTRACT_TASK_NOT_FOUND.to_string(),
            format!(
                "task `{task_id}` belongs to loop `{}`, expected `{expected_loop}`",
                actual_loop.as_deref().unwrap_or("none"),
            ),
        ),
    }
}
