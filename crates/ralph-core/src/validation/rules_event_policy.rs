//! U11: `EventPolicyRule` — lifts `apply_event_policy_validation` into the
//! unified validation pipeline.
//!
//! The rule is a `PreCommit` gate that mutates `PolicyRuntimeState` and
//! `ReviewStepTracker` through [`ValidationContext`]. Side effects that
//! rules cannot produce (diagnostic events, recovery envelopes, structured
//! `PayloadContractViolation` attribution) are accumulated back into the
//! context so the caller can emit them.

use crate::event_policy::{
    DuplicateWorkDoneHint, EventPolicyConfig, PolicyDecision, PolicyFinding, PolicyRejection,
    ViolationType, check_completion_honored, check_topic_deny_rules,
    is_recoverable_policy_finding, validate_event_with_hat,
};
use crate::event_reader::Event;
use crate::payload_contract::{PayloadContractViolation, PayloadContractViolationKind};
use crate::preset::engine::protocol::ProtocolView;

use super::context::ValidationContext;
use super::pipeline::{RulePhase, ValidationRule};
use super::result::{ReasonCode, ValidationResult, ValidationStage};

/// Unified event-policy validation rule.
pub struct EventPolicyRule;

impl ValidationRule for EventPolicyRule {
    fn name(&self) -> &'static str {
        ValidationStage::EventPolicy.as_str()
    }

    fn applies_to(&self) -> RulePhase {
        RulePhase::PreCommit
    }

    fn validate(
        &self,
        protocol_view: &ProtocolView,
        ctx: &mut ValidationContext<'_>,
        event: &Event,
    ) -> ValidationResult {
        let Some(policy_config) = protocol_view.event_policy.as_ref() else {
            return ValidationResult::accept_with(ValidationStage::EventPolicy);
        };
        if !policy_config.enabled {
            return ValidationResult::accept_with(ValidationStage::EventPolicy);
        }

        // 1. Completion/terminal guard (e.g. duplicate terminal after completion).
        let completion_decision = {
            let state = ctx.policy_runtime_state();
            check_completion_honored(&event.topic, policy_config, state)
        };
        if let Some(decision) = completion_decision {
            return handle_completion_decision(decision, ctx, event, policy_config);
        }

        // 2. Topic-deny rules.
        let deny_decision =
            check_topic_deny_rules(event.hat.as_deref(), &event.topic, policy_config);
        if let Some(decision) = deny_decision {
            return handle_topic_deny_decision(decision, ctx, event);
        }

        // 3. Payload-level policy validation (dedup, schema, terminal monotonicity).
        let mut decision = {
            let state = ctx.policy_runtime_state();
            validate_event_with_hat(
                &event.topic,
                event.payload.as_deref(),
                policy_config,
                state,
                event.hat.as_deref(),
            )
        };

        // U4: upgrade duplicate work.done hint when a wave is still open.
        if let PolicyDecision::RejectWithResume(ref mut finding) = decision {
            if let ViolationType::DuplicateWorkDone {
                ref mut hint,
                ref key,
            } = finding.violation_type
            {
                if event.wave_id.is_some() {
                    *hint = DuplicateWorkDoneHint::DuplicateStallBypass;
                    finding.message = format!(
                        "duplicate_stall_bypass: work.done for key '{key}' was already accepted \
                         but wave_id={:?} is still open. Wait for review-synthesizer terminal \
                         (review.passed or review.complete) or plan.blocked before re-sending work.done.",
                        event.wave_id
                    );
                }
            }
        }

        handle_payload_decision(decision, ctx, event, policy_config)
    }
}

fn handle_completion_decision(
    decision: PolicyDecision,
    ctx: &mut ValidationContext<'_>,
    event: &Event,
    policy_config: &EventPolicyConfig,
) -> ValidationResult {
    match decision {
        PolicyDecision::Accept => accept_and_observe(ctx, event),
        PolicyDecision::Warn(findings) => accept_with_warning(ctx, event, &findings),
        PolicyDecision::Block(finding) => {
            record_non_recoverable_violation(&finding, ctx, event, policy_config);
            reject(
                ReasonCode::EVENT_POLICY_COMPLETION_BLOCKED,
                finding.message,
                false,
            )
        }
        PolicyDecision::Ignore(finding) => reject(
            ReasonCode::EVENT_POLICY_COMPLETION_IGNORED,
            finding.message,
            false,
        ),
        // Completion guard does not emit RejectWithResume/Hold in the
        // current configuration; treat them as blocked for safety.
        PolicyDecision::RejectWithResume(finding) => {
            record_non_recoverable_violation(&finding, ctx, event, policy_config);
            reject(
                ReasonCode::EVENT_POLICY_COMPLETION_BLOCKED,
                finding.message,
                false,
            )
        }
        PolicyDecision::Hold(finding) => reject(
            ReasonCode::EVENT_POLICY_COMPLETION_BLOCKED,
            finding.message,
            false,
        ),
    }
}

fn handle_topic_deny_decision(
    decision: PolicyDecision,
    ctx: &mut ValidationContext<'_>,
    event: &Event,
) -> ValidationResult {
    match decision {
        PolicyDecision::Accept => accept_and_observe(ctx, event),
        PolicyDecision::Warn(findings) => accept_with_warning(ctx, event, &findings),
        PolicyDecision::RejectWithResume(finding) => {
            record_policy_rejection(&finding, ctx, event);
            let retry = is_recoverable_policy_finding(&finding).is_some();
            reject(event_policy_reason(&finding), finding.message, retry)
        }
        PolicyDecision::Hold(finding) => {
            record_policy_rejection(&finding, ctx, event);
            reject(ReasonCode::EVENT_POLICY_HOLD, finding.message, false)
        }
        PolicyDecision::Block(finding) => {
            reject(ReasonCode::EVENT_POLICY_BLOCKED, finding.message, false)
        }
        PolicyDecision::Ignore(finding) => {
            reject(ReasonCode::EVENT_POLICY_IGNORED, finding.message, false)
        }
    }
}

fn handle_payload_decision(
    decision: PolicyDecision,
    ctx: &mut ValidationContext<'_>,
    event: &Event,
    policy_config: &EventPolicyConfig,
) -> ValidationResult {
    match decision {
        PolicyDecision::Accept => {
            let semantic_finding = {
                let tracker = ctx.review_step_tracker();
                tracker.check_semantic_gates(event)
            };
            if let Some(finding) = semantic_finding {
                record_policy_rejection(&finding, ctx, event);
                return reject(
                    ReasonCode::EVENT_POLICY_SEMANTIC_GATE,
                    finding.message,
                    true,
                );
            }
            accept_and_observe(ctx, event)
        }
        PolicyDecision::Warn(findings) => {
            // Warnings still need to run semantic gates; a semantic gate
            // violation is recoverable and fail-closed.
            let semantic_finding = {
                let tracker = ctx.review_step_tracker();
                tracker.check_semantic_gates(event)
            };
            if let Some(finding) = semantic_finding {
                record_policy_rejection(&finding, ctx, event);
                return reject(
                    ReasonCode::EVENT_POLICY_SEMANTIC_GATE,
                    finding.message,
                    true,
                );
            }
            accept_with_warning(ctx, event, &findings)
        }
        PolicyDecision::RejectWithResume(finding) => {
            record_policy_rejection(&finding, ctx, event);
            record_non_recoverable_violation(&finding, ctx, event, policy_config);
            let retry = is_recoverable_policy_finding(&finding).is_some();
            reject(event_policy_reason(&finding), finding.message, retry)
        }
        PolicyDecision::Hold(finding) => {
            record_policy_rejection(&finding, ctx, event);
            record_non_recoverable_violation(&finding, ctx, event, policy_config);
            reject(ReasonCode::EVENT_POLICY_HOLD, finding.message, false)
        }
        PolicyDecision::Block(finding) => {
            record_non_recoverable_violation(&finding, ctx, event, policy_config);
            reject(ReasonCode::EVENT_POLICY_BLOCKED, finding.message, false)
        }
        PolicyDecision::Ignore(finding) => {
            reject(ReasonCode::EVENT_POLICY_IGNORED, finding.message, false)
        }
    }
}

fn accept_and_observe(ctx: &mut ValidationContext<'_>, event: &Event) -> ValidationResult {
    {
        let tracker = ctx.review_step_tracker();
        tracker.observe_accepted(event);
    }
    ValidationResult::accept_with(ValidationStage::EventPolicy)
}

fn accept_with_warning(
    ctx: &mut ValidationContext<'_>,
    event: &Event,
    findings: &[PolicyFinding],
) -> ValidationResult {
    // Observe the accepted event so the review-step tracker advances.
    {
        let tracker = ctx.review_step_tracker();
        tracker.observe_accepted(event);
    }
    let hint = findings
        .iter()
        .map(|f| f.message.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    ValidationResult {
        accepted: true,
        reason_code: Some(ReasonCode::EVENT_POLICY_WARNING.to_string()),
        stage: ValidationStage::EventPolicy,
        correction_hint: Some(hint),
        retry_eligible: false,
    }
}

fn reject(code: &'static str, message: String, retry_eligible: bool) -> ValidationResult {
    ValidationResult::reject(
        ValidationStage::EventPolicy,
        code,
        Some(message),
        retry_eligible,
    )
}

fn event_policy_reason(finding: &PolicyFinding) -> &'static str {
    match &finding.violation_type {
        ViolationType::MissingRequiredField { .. } => {
            ReasonCode::EVENT_POLICY_MISSING_REQUIRED_FIELD
        }
        ViolationType::InvalidFieldValue { .. } => ReasonCode::EVENT_POLICY_INVALID_FIELD_VALUE,
        ViolationType::PayloadTypeMismatch { .. } => ReasonCode::EVENT_POLICY_PAYLOAD_TYPE_MISMATCH,
        ViolationType::TerminalMonotonicityViolation { .. } => {
            ReasonCode::EVENT_POLICY_TERMINAL_MONOTONICITY
        }
        ViolationType::DuplicateTerminalEvent { .. } => ReasonCode::EVENT_POLICY_DUPLICATE_TERMINAL,
        ViolationType::BusinessEventAfterCompletion { .. } => {
            ReasonCode::EVENT_POLICY_BUSINESS_AFTER_COMPLETION
        }
        ViolationType::InvalidTopicFormat { .. } => ReasonCode::EVENT_POLICY_TOPIC_FORMAT,
        ViolationType::TopicDenied { .. } => ReasonCode::EVENT_POLICY_TOPIC_DENIED,
        ViolationType::SemanticGateViolation { .. } => ReasonCode::EVENT_POLICY_SEMANTIC_GATE,
        ViolationType::DuplicateWorkDone { .. } => ReasonCode::EVENT_POLICY_DUPLICATE_WORK_DONE,
    }
}

fn record_policy_rejection(
    finding: &PolicyFinding,
    ctx: &mut ValidationContext<'_>,
    event: &Event,
) {
    let reason_class = is_recoverable_policy_finding(finding);
    ctx.record_policy_rejection(PolicyRejection {
        topic: event.topic.clone(),
        source_hat: event.hat.clone(),
        finding: finding.clone(),
        reason_class,
    });
}

/// Capture the first non-recoverable schema-level finding as a structured
/// `PayloadContractViolation` for the U6 fatal-termination path.
fn record_non_recoverable_violation(
    finding: &PolicyFinding,
    ctx: &mut ValidationContext<'_>,
    event: &Event,
    policy_config: &EventPolicyConfig,
) {
    if is_recoverable_policy_finding(finding).is_some() {
        return;
    }
    let Some(violation) = build_payload_contract_violation(finding, ctx, event, policy_config)
    else {
        return;
    };
    ctx.record_payload_contract_violation(violation);
}

fn build_payload_contract_violation(
    finding: &PolicyFinding,
    ctx: &mut ValidationContext<'_>,
    event: &Event,
    policy_config: &EventPolicyConfig,
) -> Option<PayloadContractViolation> {
    let (error_type, field) = match &finding.violation_type {
        ViolationType::MissingRequiredField { field } => (
            PayloadContractViolationKind::MissingRequiredField,
            Some(field.clone()),
        ),
        ViolationType::PayloadTypeMismatch { .. } => {
            (PayloadContractViolationKind::PayloadTypeMismatch, None)
        }
        ViolationType::InvalidFieldValue { field, .. } => (
            PayloadContractViolationKind::AllowedValueMismatch,
            Some(field.clone()),
        ),
        // Terminal/completion/topic-format/topic-deny/semantic-gate/dedup
        // violations are NOT payload contract violations.
        _ => return None,
    };

    let fix_hint = match error_type {
        PayloadContractViolationKind::MissingRequiredField => format!(
            "Add the missing field to the payload of the '{}' event. \
             If the field is optional, remove it from the schema's required_fields.",
            finding.topic
        ),
        PayloadContractViolationKind::PayloadTypeMismatch => format!(
            "Ensure the payload of '{}' matches the schema's declared payload type.",
            finding.topic
        ),
        PayloadContractViolationKind::AllowedValueMismatch => format!(
            "Update the payload of '{}' to a value allowed by the schema.",
            finding.topic
        ),
        PayloadContractViolationKind::SchemaMissingForRequiredTopic => {
            "Add a schema for this topic.".to_string()
        }
    };

    let schema_defined_in = match policy_config.schemas.get(&finding.topic) {
        Some(_) => match &policy_config.schema_file {
            Some(f) => format!("inline + file:{f}"),
            None => "inline".to_string(),
        },
        None => "(none)".to_string(),
    };

    Some(PayloadContractViolation {
        error_type,
        timestamp: chrono::Utc::now().to_rfc3339(),
        topic: finding.topic.clone(),
        field,
        source_hat: ctx.source_hats_for(&finding.topic),
        target_hat: ctx.target_hats_for(&finding.topic),
        schema_defined_in,
        downstream_reference: None,
        upstream_reference: None,
        fix_hint,
        payload_excerpt: event.payload.as_deref().map(truncate_payload),
    })
}

fn truncate_payload(payload: &str) -> String {
    const MAX: usize = 240;
    if payload.len() > MAX {
        format!("{}…", &payload[..MAX])
    } else {
        payload.to_string()
    }
}
