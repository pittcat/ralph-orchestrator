// Cross-submodule imports (previously in same file)
use super::runtime::PolicyRuntimeState;
use super::types::{PolicyDecision, PolicyFinding, ViolationType};
use super::validation::{
    build_allowed_topics, check_topic_format, is_system_topic, validate_event_with_hat,
};
use crate::config::{
    ElementConstraint, EventPolicyConfig, EventPolicyMode, EventSchema, HatAllowedValues,
    RalphConfig, TopicDenyRule, ViolationAction,
};
use crate::hat_registry::HatRegistry;
use ralph_proto::{Hat, HatId, Topic};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateEmitPreview {
    /// "accept" | "reject"
    pub policy_decision: String,
    /// Reasons when rejected (empty when accepted).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<PolicyReasonEntry>,
    /// Projection preview (what state changes would occur).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection: Option<ProjectionPreview>,
    /// Next hat candidates (who receives this event).
    pub next_hat_candidates: NextHatCandidates,
}

/// One structured reason for rejection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyReasonEntry {
    pub gate: String,
    pub field: String,
    pub reason_code: String,
}

/// Projection (state changes) that would result from the event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionPreview {
    pub state_changes: Vec<ProjectionAction>,
}

/// One projection action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionAction {
    pub field: String,
    pub action: String,
    pub value: serde_json::Value,
}

/// Who receives the event downstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NextHatCandidates {
    /// At least one matching hat, all verified against config.hats.
    Verified { hats: Vec<String> },
    /// No hat registry available (hatless mode / empty registry).
    Unverified,
    /// Some hats matched but were not all verifiable.
    Mixed { entries: Vec<CandidateHatEntry> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateHatEntry {
    pub hat_id: String,
    pub verified: bool,
}

fn empty_next_hat_candidates() -> NextHatCandidates {
    NextHatCandidates::Verified { hats: Vec::new() }
}

/// Evaluate a candidate event for the inspect prompt path (read-only).
///
/// Returns a structured preview of whether the event would be accepted
/// by the policy gateway, what projections would apply, and which hats
/// would receive the event.
///
/// This function is **read-only**: it never writes to disk, never
/// publishes events, and never mutates real runtime state. It uses
/// `PolicyRuntimeState::default()` for a dry-run evaluation.
pub fn evaluate_candidate_emit(
    config: &RalphConfig,
    hat_id: &HatId,
    topic: &str,
    payload_json: &str,
    triggered: Option<&str>,
) -> Result<CandidateEmitPreview, String> {
    // 1. Check that hat_id exists in config.
    let hat_config = config
        .hats
        .get(hat_id.as_str())
        .ok_or_else(|| format!("hat {} not found in config", hat_id.as_str()))?;

    // U6: reject if the hat does not publish this topic (and it is not the
    // default_publishes fallback).  Checked before topic_format so the
    // topology gate takes priority over format validation.
    let hat_publishes_topic = hat_config.publishes.iter().any(|p| p == topic)
        || hat_config
            .default_publishes
            .as_deref()
            .is_some_and(|d| d == topic);

    if !hat_publishes_topic {
        return Ok(CandidateEmitPreview {
            policy_decision: "reject".to_string(),
            reasons: vec![PolicyReasonEntry {
                gate: "topic_publishes".to_string(),
                field: "topic".to_string(),
                reason_code: "hat_does_not_publish_topic".to_string(),
            }],
            projection: None,
            next_hat_candidates: empty_next_hat_candidates(),
        });
    }

    // 2. Check topic format via build_allowed_topics + check_topic_format.
    let completion_promise = &config.event_loop.completion_promise;
    let event_policy = config.event_loop.event_policy.as_ref();
    let allowed_topics = build_allowed_topics(&config.hats, completion_promise, event_policy);

    // System topics are allowed regardless.
    let topic_format_ok =
        is_system_topic(topic) || check_topic_format(topic, &allowed_topics).is_none();

    if !topic_format_ok {
        return Ok(CandidateEmitPreview {
            policy_decision: "reject".to_string(),
            reasons: vec![PolicyReasonEntry {
                gate: "topic_format".to_string(),
                field: "topic".to_string(),
                reason_code: "invalid_topic_format".to_string(),
            }],
            projection: None,
            next_hat_candidates: empty_next_hat_candidates(),
        });
    }

    // U6: reject if `triggered` is specified but is not a registered hat.
    if let Some(triggered_hat) = triggered
        && !config.hats.contains_key(triggered_hat)
    {
        return Ok(CandidateEmitPreview {
            policy_decision: "reject".to_string(),
            reasons: vec![PolicyReasonEntry {
                gate: "triggered_not_in_topology".to_string(),
                field: "triggered".to_string(),
                reason_code: "triggered_hat_not_in_config".to_string(),
            }],
            projection: None,
            next_hat_candidates: empty_next_hat_candidates(),
        });
    }

    // 3. Run validate_event_with_hat for policy validation (dry-run with default state).
    let policy_config = match event_policy {
        Some(ep) => ep.clone(),
        None => {
            // No policy config — accept by default.
            return Ok(CandidateEmitPreview {
                policy_decision: "accept".to_string(),
                reasons: Vec::new(),
                projection: None,
                next_hat_candidates: compute_next_hat_candidates(config, topic),
            });
        }
    };

    let mut state = PolicyRuntimeState::default();
    let hat_str = Some(hat_id.as_str());
    let decision = validate_event_with_hat(
        topic,
        Some(payload_json),
        &policy_config,
        &mut state,
        hat_str,
    );

    // Build the preview from the policy decision.
    let (policy_decision, reasons) = match &decision {
        PolicyDecision::Accept => ("accept".to_string(), Vec::new()),
        PolicyDecision::Warn(_findings) => {
            // Warnings are still accepted.
            ("accept".to_string(), Vec::new())
        }
        PolicyDecision::RejectWithResume(finding) => {
            let reason = policy_reason_entry_from_finding(finding);
            ("reject".to_string(), vec![reason])
        }
        PolicyDecision::Hold(finding) => {
            let reason = policy_reason_entry_from_finding(finding);
            ("reject".to_string(), vec![reason])
        }
        PolicyDecision::AcknowledgeAndForward(finding) => {
            let reason = policy_reason_entry_from_finding(finding);
            ("accept".to_string(), vec![reason])
        }
        PolicyDecision::Block(finding) => {
            let reason = policy_reason_entry_from_finding(finding);
            ("reject".to_string(), vec![reason])
        }
        PolicyDecision::Ignore(finding) => {
            let reason = policy_reason_entry_from_finding(finding);
            ("reject".to_string(), vec![reason])
        }
    };

    let projection = if policy_decision == "accept" {
        build_projection_preview(&state)
    } else {
        None
    };

    let next_hat_candidates = if policy_decision == "accept" {
        compute_next_hat_candidates(config, topic)
    } else {
        empty_next_hat_candidates()
    };

    Ok(CandidateEmitPreview {
        policy_decision,
        reasons,
        projection,
        next_hat_candidates,
    })
}

/// Convert a `PolicyFinding` into a structured `PolicyReasonEntry`.
fn policy_reason_entry_from_finding(finding: &PolicyFinding) -> PolicyReasonEntry {
    let (gate, field, reason_code) = match &finding.violation_type {
        ViolationType::PayloadTypeMismatch { expected, actual } => (
            "payload_type".to_string(),
            format!("expected={expected}, actual={actual}"),
            "payload_type_mismatch".to_string(),
        ),
        ViolationType::MissingRequiredField { field } => (
            "required_fields".to_string(),
            field.clone(),
            "missing_required_field".to_string(),
        ),
        ViolationType::InvalidFieldValue { field, value: _ } => (
            "field_value".to_string(),
            field.clone(),
            "invalid_field_value".to_string(),
        ),
        ViolationType::TerminalMonotonicityViolation {
            terminal_topic,
            business_topic,
        } => (
            "terminal_monotonicity".to_string(),
            format!("terminal={terminal_topic}, business={business_topic}"),
            "terminal_monotonicity_violation".to_string(),
        ),
        ViolationType::DuplicateTerminalEvent { topic } => (
            "terminal_duplicate".to_string(),
            topic.clone(),
            "duplicate_terminal_event".to_string(),
        ),
        ViolationType::BusinessEventAfterCompletion { topic } => (
            "completion_guard".to_string(),
            topic.clone(),
            "business_event_after_completion".to_string(),
        ),
        ViolationType::InvalidTopicFormat {
            topic,
            allowed_topics: _,
        } => (
            "topic_format".to_string(),
            topic.clone(),
            "invalid_topic_format".to_string(),
        ),
        ViolationType::TopicDenied {
            rule_hat,
            rule_topic,
        } => (
            "topic_denied".to_string(),
            format!("rule_hat={rule_hat}, topic={rule_topic}"),
            "topic_denied".to_string(),
        ),
        ViolationType::SemanticGateViolation { gate, .. } => (
            "semantic_gate".to_string(),
            gate.clone(),
            "semantic_gate_violation".to_string(),
        ),
        ViolationType::DuplicateWorkDone { key, .. } => (
            "duplicate_work_done".to_string(),
            key.clone(),
            "duplicate_work_done".to_string(),
        ),
    };

    PolicyReasonEntry {
        gate,
        field,
        reason_code,
    }
}

/// Build a projection preview from the policy runtime state after validation.
fn build_projection_preview(state: &PolicyRuntimeState) -> Option<ProjectionPreview> {
    let default = PolicyRuntimeState::default();
    let mut actions = Vec::new();

    if state.terminal_observed != default.terminal_observed {
        actions.push(ProjectionAction {
            field: "terminal_observed".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.terminal_observed),
        });
    }

    if state.completion_honored != default.completion_honored {
        actions.push(ProjectionAction {
            field: "completion_honored".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.completion_honored),
        });
    }

    if state.completion_topic != default.completion_topic {
        actions.push(ProjectionAction {
            field: "completion_topic".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.completion_topic),
        });
    }

    if state.completion_event_index != default.completion_event_index {
        actions.push(ProjectionAction {
            field: "completion_event_index".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.completion_event_index),
        });
    }

    if state.completion_iteration != default.completion_iteration {
        actions.push(ProjectionAction {
            field: "completion_iteration".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.completion_iteration),
        });
    }

    if state.current_plan_name != default.current_plan_name {
        actions.push(ProjectionAction {
            field: "current_plan_name".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.current_plan_name),
        });
    }

    if !state.work_done_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "work_done_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.work_done_seen_keys),
        });
    }

    if !state.work_done_task_id_to_key.is_empty() {
        actions.push(ProjectionAction {
            field: "work_done_task_id_to_key".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.work_done_task_id_to_key),
        });
    }

    if !state.review_dimension_ready_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "review_dimension_ready_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.review_dimension_ready_seen_keys),
        });
    }

    if !state.review_dimensions_complete_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "review_dimensions_complete_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.review_dimensions_complete_seen_keys),
        });
    }

    if !state.work_ready_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "work_ready_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.work_ready_seen_keys),
        });
    }

    if !state.pruned_work_ready_buckets.is_empty() {
        actions.push(ProjectionAction {
            field: "pruned_work_ready_buckets".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.pruned_work_ready_buckets),
        });
    }

    if !state.test_passed_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "test_passed_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.test_passed_seen_keys),
        });
    }

    if !state.test_failed_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "test_failed_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.test_failed_seen_keys),
        });
    }

    if !state.forge_wave_verified_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "forge_wave_verified_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.forge_wave_verified_seen_keys),
        });
    }

    if !state.review_start_seen_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "review_start_seen_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.review_start_seen_keys),
        });
    }

    if !state.precheck_proposed_pending_keys.is_empty() {
        actions.push(ProjectionAction {
            field: "precheck_proposed_pending_keys".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.precheck_proposed_pending_keys),
        });
    }

    if state.last_plan_blocked_reason != default.last_plan_blocked_reason {
        actions.push(ProjectionAction {
            field: "last_plan_blocked_reason".to_string(),
            action: "set".to_string(),
            value: serde_json::json!(state.last_plan_blocked_reason),
        });
    }

    if actions.is_empty() {
        None
    } else {
        Some(ProjectionPreview {
            state_changes: actions,
        })
    }
}

/// Compute which hats receive the event downstream.
pub(crate) fn compute_next_hat_candidates(config: &RalphConfig, topic: &str) -> NextHatCandidates {
    let registry = HatRegistry::from_config(config);

    // Find all hats subscribed to this topic.
    let topic_ref = ralph_proto::Topic::new(topic);
    let subscribers = registry.subscribers(&topic_ref);

    if subscribers.is_empty() {
        return NextHatCandidates::Verified { hats: Vec::new() };
    }

    // Separate subscribers into those that are in config.hats (verified) vs unknown.
    let mut verified_ids = Vec::new();
    let mut entries = Vec::new();

    for hat in subscribers {
        let hat_id_str = hat.id.as_str();
        if config.hats.contains_key(hat_id_str) {
            verified_ids.push(hat_id_str.to_string());
        } else {
            entries.push(CandidateHatEntry {
                hat_id: hat_id_str.to_string(),
                verified: false,
            });
        }
    }

    if entries.is_empty() {
        // All subscribers are known hats → Verified.
        NextHatCandidates::Verified { hats: verified_ids }
    } else {
        // Mixed: some verified, some not.
        // Prepend verified entries to entries list.
        for id in verified_ids.into_iter().rev() {
            entries.insert(
                0,
                CandidateHatEntry {
                    hat_id: id,
                    verified: true,
                },
            );
        }
        NextHatCandidates::Mixed { entries }
    }
}
