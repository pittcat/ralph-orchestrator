//! Event policy validation for typed payload schema enforcement.
//!
//! Provides pure-function validation that can be used by the event loop,
//! CLI emit commands, and API layers.

use crate::event_reader::EventReader;
use serde_json::Value;
use std::collections::HashSet;

// Re-export config types for convenience
pub use crate::config::{
    CompletionAfterTerminalAction, EventPolicyConfig, EventPolicyMode, PayloadType, ViolationAction,
};

/// Types of policy violations.
#[derive(Debug, Clone, PartialEq)]
pub enum ViolationType {
    PayloadTypeMismatch {
        expected: String,
        actual: String,
    },
    MissingRequiredField {
        field: String,
    },
    InvalidFieldValue {
        field: String,
        value: Value,
    },
    TerminalMonotonicityViolation {
        terminal_topic: String,
        business_topic: String,
    },
    DuplicateTerminalEvent {
        topic: String,
    },
    BusinessEventAfterCompletion {
        topic: String,
    },
    /// Topic is not in the whitelist of known topics (R9).
    /// Rejected without retry — only writes a recovery signal (R10).
    InvalidTopicFormat {
        topic: String,
        allowed_topics: Vec<String>,
    },
    /// Event matched a topic-deny rule (hat_id + topic exact match).
    /// The hat is explicitly forbidden from publishing this topic.
    TopicDenied {
        rule_hat: String,
        rule_topic: String,
    },
}

/// A single policy finding.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyFinding {
    pub topic: String,
    pub violation_type: ViolationType,
    pub message: String,
}

/// Decision from policy validation.
#[derive(Debug, Clone, PartialEq)]
pub enum PolicyDecision {
    Accept,
    Warn(Vec<PolicyFinding>),
    RejectWithResume(PolicyFinding),
    Hold(PolicyFinding),
    /// Silently drop the event without publishing recovery or hold artifacts.
    Block(PolicyFinding),
    /// Silently ignore the event without recovery artifacts.
    /// Semantically equivalent to `Block`; used for explicit completion-guard ignore actions.
    Ignore(PolicyFinding),
}

/// Information about an event that was rejected by policy validation.
/// Used by the CLI runner to produce unified recovery diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyRejection {
    /// Topic of the rejected event.
    pub topic: String,
    /// Source hat from the JSONL event (hat field).
    pub source_hat: Option<String>,
    /// The policy finding describing the violation.
    pub finding: PolicyFinding,
}

/// Runtime state for policy validation across events.
#[derive(Debug, Default)]
pub struct PolicyRuntimeState {
    pub terminal_observed: bool,
    pub observed_topics: HashSet<String>,
    /// Whether a completion promise has been honored in this loop.
    pub completion_honored: bool,
    /// The topic that triggered the honored completion.
    pub completion_topic: Option<String>,
    /// The event index at which completion was honored.
    pub completion_event_index: Option<u64>,
    /// The iteration at which completion was honored.
    pub completion_iteration: Option<u32>,
    /// The current plan_name extracted from the most recent `work.ready` event.
    /// Used for plan_name equality validation (U4).
    pub current_plan_name: Option<String>,
}

impl PolicyRuntimeState {
    /// Replays events from a JSONL file to build up the policy runtime state.
    ///
    /// Reads all events from the file, tracking which terminal topics have been
    /// observed and which business topics have been seen. Malformed lines are
    /// skipped. String, object, and null payloads are all handled with the same
    /// compatibility semantics as `EventReader`.
    ///
    /// Also extracts `current_plan_name` from the most recent `work.ready` event,
    /// used by the plan_name equality guard (U4).
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or read.
    pub fn from_events(
        events_path: impl AsRef<std::path::Path>,
        policy: &EventPolicyConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut reader = EventReader::new(events_path.as_ref());
        let result = reader.read_new_events()?;

        let mut state = Self::default();
        for event in result.events {
            state.observed_topics.insert(event.topic.clone());
            if policy.terminal_topics.contains(&event.topic) {
                state.terminal_observed = true;
            }
            // U4: Extract current_plan_name from work.ready events
            if event.topic == "work.ready" {
                if let Some(ref payload) = event.payload {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(payload) {
                        if let Some(name) = val.get("plan_name").and_then(|v| v.as_str()) {
                            state.current_plan_name = Some(name.to_string());
                        }
                    }
                }
            }
        }
        Ok(state)
    }
}

/// Check if an event should be handled differently because a completion promise
/// has already been honored in this loop.
///
/// When `state.completion_honored` is true, subsequent terminal events and
/// business events are subject to the `completion_after_terminal` configuration.
/// Non-terminal/non-business events pass through unchanged.
pub fn check_completion_honored(
    topic: &str,
    config: &EventPolicyConfig,
    state: &PolicyRuntimeState,
) -> Option<PolicyDecision> {
    check_completion_guard(topic, config, state.completion_honored)
}

/// Check if an event should be guarded when a completion signal has been seen.
///
/// This is the core logic used both for persistent `completion_honored` state
/// and for per-batch same-batch guarding.
pub fn check_completion_guard(
    topic: &str,
    config: &EventPolicyConfig,
    guard_active: bool,
) -> Option<PolicyDecision> {
    if !guard_active {
        return None;
    }

    if config.terminal_topics.contains(&topic.to_string()) {
        Some(apply_completion_after_terminal_action(
            &config.completion_after_terminal.duplicate_terminal,
            topic,
            ViolationType::DuplicateTerminalEvent {
                topic: topic.to_string(),
            },
        ))
    } else if config.business_topics.contains(&topic.to_string()) {
        Some(apply_completion_after_terminal_action(
            &config.completion_after_terminal.business_after_completion,
            topic,
            ViolationType::BusinessEventAfterCompletion {
                topic: topic.to_string(),
            },
        ))
    } else {
        None
    }
}

fn apply_completion_after_terminal_action(
    action: &CompletionAfterTerminalAction,
    topic: &str,
    violation_type: ViolationType,
) -> PolicyDecision {
    let finding = PolicyFinding {
        topic: topic.to_string(),
        violation_type,
        message: format!("Event '{}' arrived after completion was honored", topic),
    };

    match action {
        CompletionAfterTerminalAction::Reject => PolicyDecision::Block(finding),
        CompletionAfterTerminalAction::Ignore => PolicyDecision::Ignore(finding),
        CompletionAfterTerminalAction::Warn => PolicyDecision::Warn(vec![finding]),
    }
}

/// R9: Check topic format against the whitelist of known topics.
///
/// Rejects topics not in the whitelist **before** payload schema validation.
/// Rejection is non-retryable — only writes a recovery signal (R10), no
/// `task.resume` is emitted.
///
/// The whitelist is built from:
/// - All hat `publishes` topics (from hat registry)
/// - System/control topics (`event.*`, `human.*`, `loop.cancel`, `task.resume`,
///   `build.task.abandoned`, completion promise)
///
/// Returns `None` if the topic is valid (accepted), or `Some(PolicyDecision::Block(...))`
/// if the topic is not in the whitelist.
pub fn check_topic_format(topic: &str, allowed_topics: &HashSet<String>) -> Option<PolicyDecision> {
    if allowed_topics.contains(topic) {
        return None;
    }

    let finding = PolicyFinding {
        topic: topic.to_string(),
        violation_type: ViolationType::InvalidTopicFormat {
            topic: topic.to_string(),
            allowed_topics: allowed_topics.iter().cloned().collect(),
        },
        message: format!(
            "Topic '{}' is not in the whitelist of known topics. \
             Valid topics: {:?}",
            topic,
            allowed_topics.iter().collect::<Vec<_>>()
        ),
    };

    // R10: Block (not RejectWithResume) — no retry, only recovery signal
    Some(PolicyDecision::Block(finding))
}

/// Build the set of allowed topics from hat configs and system control topics.
///
/// Includes:
/// - All hat `publishes` topics (what hats emit)
/// - All hat `triggers` topics (what activates hats)
/// - Event policy `terminal_topics` and `business_topics` (if configured)
/// - System control topics: `loop.cancel`, `task.resume`, `build.task.abandoned`,
///   completion promise
///
/// Note: `event.*` and `human.*` topics are NOT stored here as prefixes.
/// They are allowed by the `is_system_topic()` check which is applied
/// BEFORE `check_topic_format` in the event loop validation flow.
pub fn build_allowed_topics(
    hats: &std::collections::HashMap<String, crate::config::HatConfig>,
    completion_promise: &str,
    event_policy: Option<&EventPolicyConfig>,
) -> HashSet<String> {
    let mut allowed = HashSet::new();

    // Add all hat publishes and triggers topics
    for hat_config in hats.values() {
        for topic in &hat_config.publishes {
            allowed.insert(topic.clone());
        }
        for topic in &hat_config.triggers {
            allowed.insert(topic.clone());
        }
    }

    // Add event policy terminal and business topics
    if let Some(policy) = event_policy {
        for topic in &policy.terminal_topics {
            allowed.insert(topic.clone());
        }
        for topic in &policy.business_topics {
            allowed.insert(topic.clone());
        }
    }

    // System/control topics (exact match)
    allowed.insert("loop.cancel".to_string());
    allowed.insert("task.resume".to_string());
    allowed.insert("build.task.abandoned".to_string());
    allowed.insert(completion_promise.to_string());

    // Note: event.* and human.* topics are handled by is_system_topic() check
    // (tested BEFORE check_topic_format in the event loop), not by prefix
    // matching in this set. The comment above about "stored as actual prefixes"
    // was incorrect - they are not inserted here.

    allowed
}

/// Check if a topic matches a system/control prefix pattern.
///
/// System topics start with `event.` or `human.` and are always allowed
/// regardless of the whitelist. This check is applied BEFORE
/// check_topic_format in the event loop.
pub fn is_system_topic(topic: &str) -> bool {
    topic.starts_with("event.") || topic.starts_with("human.")
}

/// Check topic-deny rules against a (hat, topic) pair.
///
/// When the event policy is in `Enforce` mode and the (hat_id, topic) pair
/// matches any `topic_deny_rules` entry, returns `Some(PolicyDecision::Block)`
/// with reason `"topic_denied"`.  Otherwise returns `None`.
///
/// In `Observe` mode, matching a deny rule produces a `Warn` decision instead.
pub fn check_topic_deny_rules(
    hat: Option<&str>,
    topic: &str,
    config: &EventPolicyConfig,
) -> Option<PolicyDecision> {
    let hat_id = hat.unwrap_or("");
    for rule in &config.topic_deny_rules {
        if rule.hat_id == hat_id && rule.topic == topic {
            let finding = PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::TopicDenied {
                    rule_hat: rule.hat_id.clone(),
                    rule_topic: rule.topic.clone(),
                },
                message: format!(
                    "Hat '{}' is denied from publishing topic '{}'",
                    rule.hat_id, rule.topic
                ),
            };
            return Some(match config.mode {
                EventPolicyMode::Observe => PolicyDecision::Warn(vec![finding]),
                EventPolicyMode::Enforce => match config.on_violation {
                    ViolationAction::Warn => PolicyDecision::Warn(vec![finding]),
                    ViolationAction::RejectWithResume => PolicyDecision::RejectWithResume(finding),
                    ViolationAction::Hold => PolicyDecision::Hold(finding),
                    ViolationAction::Block => PolicyDecision::Block(finding),
                },
            });
        }
    }
    None
}

/// Validates an event against the event policy.
pub fn validate_event(
    topic: &str,
    payload: Option<&str>,
    config: &EventPolicyConfig,
    state: &mut PolicyRuntimeState,
) -> PolicyDecision {
    if !config.enabled {
        return PolicyDecision::Accept;
    }

    state.observed_topics.insert(topic.to_string());

    let mut findings = Vec::new();

    // Terminal monotonicity check (read-only on state; caller applies terminal_observed)
    if state.terminal_observed && config.business_topics.contains(&topic.to_string()) {
        let terminal_topic = config.terminal_topics.first().cloned().unwrap_or_default();
        findings.push(PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::TerminalMonotonicityViolation {
                terminal_topic: terminal_topic.clone(),
                business_topic: topic.to_string(),
            },
            message: format!(
                "Business event '{}' after terminal topic '{}' violates monotonicity",
                topic, terminal_topic
            ),
        });
    }

    // Duplicate terminal check (read-only on state; caller applies terminal_observed)
    if state.terminal_observed && config.terminal_topics.contains(&topic.to_string()) {
        findings.push(PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::DuplicateTerminalEvent {
                topic: topic.to_string(),
            },
            message: format!(
                "Duplicate terminal event '{}' after terminal topic was already observed",
                topic
            ),
        });
    }

    // Schema validation
    if let Some(schema) = config.schemas.get(topic) {
        if let Some(expected_type) = &schema.payload
            && matches!(expected_type, PayloadType::JsonObject)
        {
            match payload {
                Some(p) => match serde_json::from_str::<Value>(p) {
                    Ok(Value::Object(_)) => {}
                    Ok(other) => {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::PayloadTypeMismatch {
                                expected: "json_object".to_string(),
                                actual: format!("{:?}", other),
                            },
                            message: format!("Payload must be JSON object, got {:?}", other),
                        });
                    }
                    Err(e) => {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::PayloadTypeMismatch {
                                expected: "json_object".to_string(),
                                actual: format!("parse error: {}", e),
                            },
                            message: format!("Payload is not valid JSON: {}", e),
                        });
                    }
                },
                None => {
                    findings.push(PolicyFinding {
                        topic: topic.to_string(),
                        violation_type: ViolationType::PayloadTypeMismatch {
                            expected: "json_object".to_string(),
                            actual: "null".to_string(),
                        },
                        message: "Payload is required to be JSON object but is missing".to_string(),
                    });
                }
            }
        }

        // Required fields
        if !schema.required_fields.is_empty() {
            if let Some(p) = payload {
                if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p) {
                    for field in &schema.required_fields {
                        if extract_json_field(&Value::Object(obj.clone()), field).is_none() {
                            findings.push(PolicyFinding {
                                topic: topic.to_string(),
                                violation_type: ViolationType::MissingRequiredField {
                                    field: field.clone(),
                                },
                                message: format!("Missing required field: {}", field),
                            });
                        }
                    }
                }
            } else {
                // Payload is missing but required fields are specified
                for field in &schema.required_fields {
                    findings.push(PolicyFinding {
                        topic: topic.to_string(),
                        violation_type: ViolationType::MissingRequiredField {
                            field: field.clone(),
                        },
                        message: format!("Missing required field '{}' (payload is missing)", field),
                    });
                }
            }
        }

        // Allowed values
        for (field_path, allowed) in &schema.allowed_values {
            if let Some(p) = payload
                && let Ok(value) = serde_json::from_str::<Value>(p)
                && let Some(field_value) = extract_json_field(&value, field_path)
                && !allowed.contains(&field_value)
            {
                findings.push(PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::InvalidFieldValue {
                        field: field_path.clone(),
                        value: field_value.clone(),
                    },
                    message: format!(
                        "Field '{}' has invalid value {:?}. Allowed: {:?}",
                        field_path, field_value, allowed
                    ),
                });
            }
        }
    }

    // U4: plan_name equality — when enabled, work.done's plan_name must equal
    // the current_plan_name extracted from the most recent work.ready event.
    if config.plan_name_equality_required
        && topic == "work.done"
        && let Some(expected) = &state.current_plan_name
    {
        if let Some(p) = payload {
            if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p) {
                let actual = obj.get("plan_name").and_then(|v| v.as_str());
                if actual != Some(expected.as_str()) {
                    findings.push(PolicyFinding {
                        topic: topic.to_string(),
                        violation_type: ViolationType::InvalidFieldValue {
                            field: "plan_name".to_string(),
                            value: actual
                                .map(|s| Value::String(s.to_string()))
                                .unwrap_or(Value::Null),
                        },
                        message: format!(
                            "work.done plan_name mismatch: expected '{}', got {:?}",
                            expected,
                            actual.unwrap_or("(missing)")
                        ),
                    });
                }
            }
        }
    }

    if findings.is_empty() {
        return PolicyDecision::Accept;
    }

    match config.mode {
        EventPolicyMode::Observe => PolicyDecision::Warn(findings),
        EventPolicyMode::Enforce => match config.on_violation {
            ViolationAction::Warn => PolicyDecision::Warn(findings),
            ViolationAction::RejectWithResume => {
                PolicyDecision::RejectWithResume(findings.into_iter().next().unwrap())
            }
            ViolationAction::Hold => PolicyDecision::Hold(findings.into_iter().next().unwrap()),
            ViolationAction::Block => PolicyDecision::Block(findings.into_iter().next().unwrap()),
        },
    }
}

/// Extract a nested field from a JSON value using dot notation.
fn extract_json_field(value: &Value, path: &str) -> Option<Value> {
    let mut current = value;
    for part in path.split('.') {
        match current {
            Value::Object(obj) => {
                current = obj.get(part)?;
            }
            _ => return None,
        }
    }
    Some(current.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EventSchema, TopicDenyRule};
    use std::collections::HashMap;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn test_config() -> EventPolicyConfig {
        EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::RejectWithResume,
            schemas: HashMap::new(),
            terminal_topics: vec!["LOOP_COMPLETE".to_string()],
            business_topics: vec!["experiment.planned".to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn test_accept_when_disabled() {
        let mut config = test_config();
        config.enabled = false;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some("{}"), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_accept_valid_json_object() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some(r#"{"key": "value"}"#), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_string_payload_when_json_object_required() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some("plain string"), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_missing_required_field() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec!["task_key".to_string()],

            allowed_values: HashMap::new(),
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some(r#"{"other": "value"}"#), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_invalid_allowed_value() {
        let mut config = test_config();
        let mut schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
        };
        schema.allowed_values.insert(
            "decision".to_string(),
            vec![
                Value::String("keep".to_string()),
                Value::String("discard".to_string()),
            ],
        );
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event(
            "test",
            Some(r#"{"decision": "blocked"}"#),
            &config,
            &mut state,
        );
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_terminal_then_business_violation() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        // validate_event no longer mutates terminal_observed; caller applies it
        // after all validation layers have passed. We simulate that here.
        state.terminal_observed = true;
        let decision = validate_event("experiment.planned", Some("{}"), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_observe_mode_does_not_reject() {
        let mut config = test_config();
        config.mode = EventPolicyMode::Observe;
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some("plain string"), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::Warn(_)));
    }

    #[test]
    fn test_enforce_reject_with_resume() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some("plain string"), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_nested_field_extraction() {
        let value = serde_json::json!({"evaluation": {"decision": "keep"}});
        let result = extract_json_field(&value, "evaluation.decision");
        assert_eq!(result, Some(Value::String("keep".to_string())));
    }

    #[test]
    fn test_extract_json_field_nonexistent_path() {
        let value = serde_json::json!({"a": {"b": 1}});
        assert_eq!(extract_json_field(&value, "a.c"), None);
        assert_eq!(extract_json_field(&value, "x.y"), None);
        assert_eq!(extract_json_field(&value, ""), None);
    }

    #[test]
    fn test_extract_json_field_intermediate_non_object() {
        let value = serde_json::json!({"a": [1, 2, 3]});
        assert_eq!(extract_json_field(&value, "a.b"), None);
        let value2 = serde_json::json!({"a": "string"});
        assert_eq!(extract_json_field(&value2, "a.b"), None);
    }

    #[test]
    fn test_required_fields_when_payload_missing() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: None,
            required_fields: vec!["task_key".to_string()],
            allowed_values: HashMap::new(),
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "Missing payload with required fields should be rejected"
        );
    }

    #[test]
    fn test_nested_allowed_values_validation() {
        let mut config = test_config();
        let mut schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],
            allowed_values: HashMap::new(),
        };
        schema.allowed_values.insert(
            "evaluation.decision".to_string(),
            vec![
                Value::String("keep".to_string()),
                Value::String("discard".to_string()),
            ],
        );
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();

        // Valid nested value
        let decision = validate_event(
            "test",
            Some(r#"{"evaluation": {"decision": "keep"}}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);

        // Invalid nested value
        let decision = validate_event(
            "test",
            Some(r#"{"evaluation": {"decision": "blocked"}}"#),
            &config,
            &mut state,
        );
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_duplicate_terminal_event_violation() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        // Caller sets terminal_observed after the first terminal event passes validation
        state.terminal_observed = true;
        let decision = validate_event("LOOP_COMPLETE", None, &config, &mut state);
        assert!(
            matches!(
                decision,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateTerminalEvent { ref topic },
                    ..
                }) if topic == "LOOP_COMPLETE"
            ),
            "Expected DuplicateTerminalEvent violation, got {:?}",
            decision
        );
    }

    #[test]
    fn test_duplicate_terminal_accepted_when_disabled() {
        let mut config = test_config();
        config.enabled = false;
        let mut state = PolicyRuntimeState::default();
        state.terminal_observed = true;
        let decision = validate_event("LOOP_COMPLETE", None, &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_duplicate_terminal_observe_mode_warns() {
        let mut config = test_config();
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        state.terminal_observed = true;
        let decision = validate_event("LOOP_COMPLETE", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::Warn(ref findings) if findings.iter().any(|f| matches!(f.violation_type, ViolationType::DuplicateTerminalEvent { .. }))),
            "Expected Warn with DuplicateTerminalEvent, got {:?}",
            decision
        );
    }

    #[test]
    fn test_from_events_replays_terminal_and_business() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"experiment.planned","payload":"{{}}","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let config = test_config();
        let state = PolicyRuntimeState::from_events(file.path(), &config).unwrap();

        assert!(state.terminal_observed);
        assert!(state.observed_topics.contains("experiment.planned"));
        assert!(state.observed_topics.contains("LOOP_COMPLETE"));
    }

    #[test]
    fn test_from_events_payload_compatibility() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().unwrap();
        // String payload
        writeln!(
            file,
            r#"{{"topic":"task.start","payload":"Start work","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        // Object payload
        writeln!(
            file,
            r#"{{"topic":"task.done","payload":{{"result":"success"}},"ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        // Null payload
        writeln!(
            file,
            r#"{{"topic":"heartbeat","payload":null,"ts":"2024-01-01T00:00:02Z"}}"#
        )
        .unwrap();
        // Missing payload
        writeln!(file, r#"{{"topic":"noop","ts":"2024-01-01T00:00:03Z"}}"#).unwrap();
        file.flush().unwrap();

        let config = test_config();
        let state = PolicyRuntimeState::from_events(file.path(), &config).unwrap();

        assert_eq!(state.observed_topics.len(), 4);
        assert!(state.observed_topics.contains("task.start"));
        assert!(state.observed_topics.contains("task.done"));
        assert!(state.observed_topics.contains("heartbeat"));
        assert!(state.observed_topics.contains("noop"));
    }

    #[test]
    fn test_from_events_missing_file() {
        let config = test_config();
        let state = PolicyRuntimeState::from_events("/nonexistent/events.jsonl", &config).unwrap();
        assert!(!state.terminal_observed);
        assert!(state.observed_topics.is_empty());
    }

    #[test]
    fn test_from_events_skips_malformed_lines() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"experiment.planned","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(file, "this is not json").unwrap();
        writeln!(
            file,
            r#"{{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let config = test_config();
        let state = PolicyRuntimeState::from_events(file.path(), &config).unwrap();

        assert!(state.terminal_observed);
        assert!(state.observed_topics.contains("experiment.planned"));
        assert!(state.observed_topics.contains("LOOP_COMPLETE"));
    }

    // -------------------------------------------------------------------------
    // Completion honored guard tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_check_completion_honored_inactive_returns_none() {
        let config = test_config();
        let state = PolicyRuntimeState::default();
        assert_eq!(
            check_completion_honored("LOOP_COMPLETE", &config, &state),
            None
        );
        assert_eq!(
            check_completion_honored("experiment.planned", &config, &state),
            None
        );
    }

    #[test]
    fn test_check_completion_honored_warns_duplicate_terminal_by_default() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        let decision = check_completion_honored("LOOP_COMPLETE", &config, &state);
        assert!(
            matches!(decision, Some(PolicyDecision::Warn(_))),
            "Expected Warn for duplicate terminal by default, got {:?}",
            decision
        );
    }

    #[test]
    fn test_check_completion_honored_warns_business_after_completion_by_default() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        let decision = check_completion_honored("experiment.planned", &config, &state);
        assert!(
            matches!(decision, Some(PolicyDecision::Warn(_))),
            "Expected Warn for business after completion by default, got {:?}",
            decision
        );
    }

    #[test]
    fn test_check_completion_honored_allows_unrelated_events() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        assert_eq!(
            check_completion_honored("task.resume", &config, &state),
            None
        );
        assert_eq!(
            check_completion_honored("human.response", &config, &state),
            None
        );
    }

    #[test]
    fn test_check_completion_honored_ignore_action() {
        let mut config = test_config();
        config.completion_after_terminal.duplicate_terminal = CompletionAfterTerminalAction::Ignore;
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        let decision = check_completion_honored("LOOP_COMPLETE", &config, &state);
        assert!(
            matches!(decision, Some(PolicyDecision::Ignore(_))),
            "Expected Ignore, got {:?}",
            decision
        );
    }

    #[test]
    fn test_check_completion_honored_warn_action() {
        let mut config = test_config();
        config.completion_after_terminal.business_after_completion =
            CompletionAfterTerminalAction::Warn;
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        let decision = check_completion_honored("experiment.planned", &config, &state);
        assert!(
            matches!(decision, Some(PolicyDecision::Warn(_))),
            "Expected Warn, got {:?}",
            decision
        );
    }

    #[test]
    fn test_check_completion_guard_respects_guard_active_flag() {
        let config = test_config();
        assert_eq!(
            check_completion_guard("LOOP_COMPLETE", &config, false),
            None
        );
        assert!(matches!(
            check_completion_guard("LOOP_COMPLETE", &config, true),
            Some(PolicyDecision::Warn(_))
        ));
    }

    // -------------------------------------------------------------------------
    // Shared fixture tests (U6)
    // -------------------------------------------------------------------------

    const FIXTURE_VALID_CHAIN: &str = r#"{"topic":"experiment.planned","payload":{"task_key":"a","hypothesis":"h","falsification_condition":"f"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:01Z"}"#;

    const FIXTURE_DUPLICATE_TERMINAL: &str = r#"{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"LOOP_COMPLETE","payload":{"reason":"retry"},"ts":"2026-05-22T00:00:01Z"}"#;

    const FIXTURE_BUSINESS_AFTER_TERMINAL: &str = r#"{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"experiment.planned","payload":{"task_key":"b","hypothesis":"h","falsification_condition":"f"},"ts":"2026-05-22T00:00:01Z"}"#;

    const FIXTURE_MISSING_REQUIRED_FIELDS: &str =
        r#"{"topic":"experiment.planned","payload":{"task_key":"a"},"ts":"2026-05-22T00:00:00Z"}"#;

    fn fixture_config() -> EventPolicyConfig {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![
                "task_key".to_string(),
                "hypothesis".to_string(),
                "falsification_condition".to_string(),
            ],
            allowed_values: HashMap::new(),
        };
        config
            .schemas
            .insert("experiment.planned".to_string(), schema);
        config.completion_after_terminal.duplicate_terminal = CompletionAfterTerminalAction::Reject;
        config.completion_after_terminal.business_after_completion =
            CompletionAfterTerminalAction::Reject;
        config
    }

    fn parse_fixture_line(line: &str) -> (String, Option<String>) {
        let event: crate::event_reader::Event =
            serde_json::from_str(line).expect("valid fixture line");
        (event.topic, event.payload)
    }

    fn is_accept(decision: &PolicyDecision) -> bool {
        matches!(decision, PolicyDecision::Accept)
    }

    /// Write all lines except the last to a temp file, replay state, then validate the last line.
    fn replay_and_validate(fixture: &str) -> (PolicyRuntimeState, PolicyDecision) {
        let config = fixture_config();
        let lines: Vec<&str> = fixture.lines().collect();
        let mut file = NamedTempFile::new().unwrap();
        for line in &lines[..lines.len().saturating_sub(1)] {
            writeln!(file, "{}", line).unwrap();
        }
        file.flush().unwrap();
        let mut state = PolicyRuntimeState::from_events(file.path(), &config).unwrap();
        // Simulate the event loop marking completion as honored once a terminal
        // event has been observed in the replayed history.
        if state.terminal_observed {
            state.completion_honored = true;
        }
        let (topic, payload) = parse_fixture_line(lines.last().unwrap());
        let decision = validate_event(&topic, payload.as_deref(), &config, &mut state);
        (state, decision)
    }

    #[test]
    fn test_fixture_valid_chain_accepted() {
        let (_, decision) = replay_and_validate(FIXTURE_VALID_CHAIN);
        assert!(
            is_accept(&decision),
            "Expected Accept for valid chain, got {:?}",
            decision
        );
    }

    #[test]
    fn test_fixture_duplicate_terminal_rejected_or_ignored() {
        let (_, decision) = replay_and_validate(FIXTURE_DUPLICATE_TERMINAL);
        assert!(
            !is_accept(&decision),
            "Expected reject/ignore for duplicate terminal, got {:?}",
            decision
        );
    }

    #[test]
    fn test_fixture_business_after_terminal_rejected_or_ignored() {
        let (_, decision) = replay_and_validate(FIXTURE_BUSINESS_AFTER_TERMINAL);
        assert!(
            !is_accept(&decision),
            "Expected reject/ignore for business after terminal, got {:?}",
            decision
        );
    }

    #[test]
    fn test_fixture_missing_required_fields_rejected_when_strict() {
        let config = fixture_config();
        let mut state =
            PolicyRuntimeState::from_events("/nonexistent/events.jsonl", &config).unwrap();
        let (topic, payload) = parse_fixture_line(FIXTURE_MISSING_REQUIRED_FIELDS);
        let decision = validate_event(&topic, payload.as_deref(), &config, &mut state);
        assert!(
            !is_accept(&decision),
            "Expected reject for missing provenance under strict config, got {:?}",
            decision
        );
    }

    #[test]
    fn test_provenance_fields_preserved_by_reader() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"experiment.planned","payload":{{"task_key":"x"}},"ts":"2024-01-01T00:00:00Z","hat":"strategist","triggered":"implementer","source":"cli"}}"#
        ).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();
        assert_eq!(result.events.len(), 1);
        let event = &result.events[0];
        assert_eq!(event.hat, Some("strategist".to_string()));
        assert_eq!(event.triggered, Some("implementer".to_string()));
        assert_eq!(event.source, Some("cli".to_string()));
    }

    #[test]
    fn test_old_simple_event_fixtures_still_parse() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"task.start","payload":"Start work","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"topic":"task.done","payload":null,"ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        writeln!(file, r#"{{"topic":"noop","ts":"2024-01-01T00:00:02Z"}}"#).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();
        assert_eq!(result.events.len(), 3);
        assert_eq!(result.events[0].topic, "task.start");
        assert_eq!(result.events[0].payload, Some("Start work".to_string()));
        assert!(result.events[1].payload.is_none());
        assert!(result.events[2].payload.is_none());
    }

    // -------------------------------------------------------------------------
    // Topic format check tests (U5)
    // -------------------------------------------------------------------------

    #[test]
    fn test_check_topic_format_accepts_whitelisted_topic() {
        let mut allowed = HashSet::new();
        allowed.insert("work.done".to_string());
        allowed.insert("review.passed".to_string());
        assert_eq!(check_topic_format("work.done", &allowed), None);
        assert_eq!(check_topic_format("review.passed", &allowed), None);
    }

    #[test]
    fn test_check_topic_format_rejects_unknown_topic() {
        let mut allowed = HashSet::new();
        allowed.insert("work.done".to_string());
        let result = check_topic_format("REVIEW_COMPLETE", &allowed);
        assert!(result.is_some());
        let decision = result.unwrap();
        assert!(matches!(decision, PolicyDecision::Block(_)));
    }

    #[test]
    fn test_check_topic_format_rejects_uppercase_topic() {
        let mut allowed = HashSet::new();
        allowed.insert("work.done".to_string());
        // AE2: uppercase topic is rejected
        let result = check_topic_format("LOOP_COMPLETE", &allowed);
        assert!(result.is_some());
        let decision = result.unwrap();
        match decision {
            PolicyDecision::Block(finding) => {
                assert!(matches!(
                    finding.violation_type,
                    ViolationType::InvalidTopicFormat { .. }
                ));
            }
            _ => panic!("Expected Block decision"),
        }
    }

    #[test]
    fn test_check_topic_format_accepts_loop_complete_when_whitelisted() {
        // AE5: whitelisted completion token is accepted
        let mut allowed = HashSet::new();
        allowed.insert("LOOP_COMPLETE".to_string());
        assert_eq!(check_topic_format("LOOP_COMPLETE", &allowed), None);
    }

    #[test]
    fn test_is_system_topic_event_prefix() {
        assert!(is_system_topic("event.malformed"));
        assert!(is_system_topic("event.scope_violation"));
        assert!(is_system_topic("event.policy_warning"));
        assert!(!is_system_topic("work.done"));
        assert!(!is_system_topic("review.passed"));
    }

    #[test]
    fn test_is_system_topic_human_prefix() {
        assert!(is_system_topic("human.interact"));
        assert!(is_system_topic("human.response"));
        assert!(is_system_topic("human.guidance"));
        assert!(!is_system_topic("humanx.interact")); // no dot after prefix
    }

    #[test]
    fn test_build_allowed_topics_includes_hat_publishes() {
        let mut hats = std::collections::HashMap::new();
        let mut hat_config = crate::config::HatConfig::default();
        hat_config.publishes = vec!["work.done".to_string(), "review.passed".to_string()];
        hats.insert("executor".to_string(), hat_config);

        let allowed = build_allowed_topics(&hats, "LOOP_COMPLETE", None);
        assert!(allowed.contains("work.done"));
        assert!(allowed.contains("review.passed"));
        assert!(allowed.contains("LOOP_COMPLETE"));
        assert!(allowed.contains("loop.cancel"));
        assert!(allowed.contains("task.resume"));
        assert!(allowed.contains("build.task.abandoned"));
    }

    #[test]
    fn test_build_allowed_topics_empty_hats() {
        let hats = std::collections::HashMap::new();
        let allowed = build_allowed_topics(&hats, "LOOP_COMPLETE", None);
        // Only system topics
        assert!(allowed.contains("LOOP_COMPLETE"));
        assert!(allowed.contains("loop.cancel"));
        assert!(allowed.contains("task.resume"));
        assert!(allowed.contains("build.task.abandoned"));
        assert!(!allowed.contains("work.done"));
    }

    #[test]
    fn test_build_allowed_topics_includes_event_policy_topics() {
        let hats = std::collections::HashMap::new();
        let policy = EventPolicyConfig {
            terminal_topics: vec!["review.file".to_string()],
            business_topics: vec!["task.update".to_string()],
            ..Default::default()
        };
        let allowed = build_allowed_topics(&hats, "LOOP_COMPLETE", Some(&policy));
        assert!(allowed.contains("review.file"));
        assert!(allowed.contains("task.update"));
        assert!(allowed.contains("LOOP_COMPLETE"));
    }

    // P2 #20: regression guard for `is_system_topic` short-circuit.
    //
    // The `build_allowed_topics` doc (line 235-238) explicitly states
    // that `event.*` and `human.*` topics are NOT inserted into the
    // allowed-topics set; they are admitted by the `is_system_topic()`
    // short-circuit, which the event loop applies BEFORE
    // `check_topic_format`. If a future refactor ever:
    //
    // (a) reorders the event-loop partition so `check_topic_format` runs
    //     first, OR
    // (b) removes the `is_system_topic` short-circuit (e.g. by trying to
    //     be "uniform" with the rest of the validation), OR
    // (c) starts inserting `event.*` / `human.*` as prefix members into
    //     `allowed_topics`,
    //
    // then `event.*` / `human.*` topics that have NEVER been declared
    // anywhere would start failing format checks. The two halves of the
    // contract (`is_system_topic` admits unknown system topics;
    // `check_topic_format` rejects unknown business topics) must stay
    // disjoint and applied in the documented order.
    //
    // This test pins both halves together by simulating the event-loop
    // validation flow as a single composed operation and asserting that
    // a "rogue" system topic (uppercase, would otherwise fail
    // `check_topic_format`) is admitted ONLY when `is_system_topic` is
    // consulted first.
    #[test]
    fn system_topic_short_circuit_runs_before_format_check() {
        // Empty whitelist — `check_topic_format` would reject ANY non-empty
        // topic that is not in the whitelist.
        let allowed = build_allowed_topics(&HashMap::new(), "LOOP_COMPLETE", None);

        // A topic that:
        //   - has uppercase letters → would normally fail format checks
        //   - is an `event.*` topic → admitted by `is_system_topic`
        //   - is NOT in the whitelist (and never will be, by U3 design)
        let rogue_system_topic = "event.foo.BAR";

        // Sanity: the system-topic short-circuit admits it.
        assert!(
            is_system_topic(rogue_system_topic),
            "test premise: '{rogue_system_topic}' must satisfy is_system_topic"
        );

        // Sanity: `check_topic_format` would reject it on its own — this
        // is the whole reason we need the short-circuit.
        assert!(
            check_topic_format(rogue_system_topic, &allowed).is_some(),
            "test premise: '{rogue_system_topic}' must be rejected by check_topic_format \
             when called in isolation, so that the short-circuit is load-bearing"
        );

        // Now compose the two checks in the documented order
        // (`is_system_topic` → `check_topic_format`). The composed
        // operation MUST accept the system topic even though
        // `check_topic_format` alone would reject it.
        let composed_admits = |topic: &str| -> bool {
            if is_system_topic(topic) {
                return true;
            }
            check_topic_format(topic, &allowed).is_none()
        };
        assert!(
            composed_admits(rogue_system_topic),
            "composed validation (is_system_topic → check_topic_format) must admit \
             '{rogue_system_topic}' — this is the order documented in build_allowed_topics"
        );

        // A non-system rogue topic (uppercase business topic) must STILL
        // be rejected by the composed operation — proving we did not
        // accidentally turn the short-circuit into a blanket bypass.
        let rogue_business_topic = "WORK.DONE.WITH_UPPERCASE";
        assert!(!is_system_topic(rogue_business_topic));
        assert!(
            !composed_admits(rogue_business_topic),
            "composed validation must still reject unknown business topics; \
             the short-circuit is for system topics only"
        );

        // And a well-formed business topic that's in the whitelist must
        // still be admitted — proving `check_topic_format` is still
        // doing its real job on the non-system side. Add "work.done"
        // to the whitelist to exercise the admit path explicitly.
        let mut allowed_with_work = allowed.clone();
        allowed_with_work.insert("work.done".to_string());
        let composed_admits_work = |topic: &str| -> bool {
            if is_system_topic(topic) {
                return true;
            }
            check_topic_format(topic, &allowed_with_work).is_none()
        };
        assert!(
            composed_admits_work("work.done"),
            "composed validation must admit whitelisted business topics"
        );
    }

    // -------------------------------------------------------------------------
    // U3: topic-deny rules tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_topic_deny_rules_match_rejected() {
        // Matching deny rule → Block when mode=Enforce
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "executor".to_string(),
                topic: "build.done".to_string(),
            }],
            ..Default::default()
        };
        let decision = check_topic_deny_rules(Some("executor"), "build.done", &config);
        assert!(matches!(decision, Some(PolicyDecision::Block(_))));
    }

    #[test]
    fn test_topic_deny_rules_non_matching_accepted() {
        // Non-matching hat_id → None (allowed)
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "executor".to_string(),
                topic: "build.done".to_string(),
            }],
            ..Default::default()
        };
        // Different hat, same topic → no match
        assert!(check_topic_deny_rules(Some("reviewer"), "build.done", &config).is_none());
        // Same hat, different topic → no match
        assert!(check_topic_deny_rules(Some("executor"), "work.done", &config).is_none());
        // No hat → no match (empty string not matched)
        assert!(check_topic_deny_rules(None, "build.done", &config).is_none());
    }

    #[test]
    fn test_topic_deny_rules_observe_mode_warns() {
        // Observe mode → Warn even when rule matches
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Observe,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "executor".to_string(),
                topic: "build.done".to_string(),
            }],
            ..Default::default()
        };
        let decision = check_topic_deny_rules(Some("executor"), "build.done", &config);
        assert!(matches!(decision, Some(PolicyDecision::Warn(_))));
    }

    // -------------------------------------------------------------------------
    // U4: review.passed skip_reason allowlist + ralph topic_deny_rules
    // (mirrors the three edits in `presets/en/ce-executor.yml`).
    // -------------------------------------------------------------------------

    fn review_passed_allowlist_config() -> EventPolicyConfig {
        let mut config = test_config();
        let mut schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![
                "plan_name".into(),
                "task_id".into(),
                "task_key".into(),
                "step".into(),
                "findings_count".into(),
                "fix_round".into(),
                "verdict".into(),
                "skip_reason".into(),
            ],
            allowed_values: HashMap::new(),
        };
        // Mirror the ce-executor.yml U4 allowlist exactly.
        schema.allowed_values.insert(
            "skip_reason".to_string(),
            vec![
                Value::String("empty_diff".to_string()),
                Value::String("trivial_step".to_string()),
                Value::String("aggregate_timeout".to_string()),
            ],
        );
        config.schemas.insert("review.passed".to_string(), schema);
        config
    }

    #[test]
    fn test_u4_review_passed_skip_reason_allowlist_accepts_legal_values() {
        let config = review_passed_allowlist_config();
        for legal in ["empty_diff", "trivial_step", "aggregate_timeout"] {
            let payload = format!(
                r#"{{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"{legal}"}}"#
            );
            let mut state = PolicyRuntimeState::default();
            let decision = validate_event("review.passed", Some(&payload), &config, &mut state);
            assert_eq!(
                decision,
                PolicyDecision::Accept,
                "skip_reason='{legal}' should be accepted by the allowlist, got {:?}",
                decision
            );
        }
    }

    #[test]
    fn test_u4_review_passed_skip_reason_allowlist_rejects_fabricated() {
        // The P1 root cause: review-synthesizer invented
        // `dimension_reviewer_no_response` as a skip_reason when the
        // aggregate timeout fired. Without the allowlist this passes
        // the required_fields gate. U4 closes that hole.
        let config = review_passed_allowlist_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"dimension_reviewer_no_response"}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "fabricated skip_reason must be rejected, got {:?}",
            decision
        );
    }

    #[test]
    fn test_u4_review_passed_skip_reason_allowlist_rejects_empty_string() {
        let config = review_passed_allowlist_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":""}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_u4_topic_deny_rules_ralph_blocked_from_workflow_topics() {
        // Mirrors the five new deny rules in ce-executor.yml:
        //   {hat_id: ralph, topic: review.wave.ready / review.passed /
        //    queue.advance / plan.complete / plan.blocked}
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "review.wave.ready".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "review.passed".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "queue.advance".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "plan.complete".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "plan.blocked".to_string(),
                },
            ],
            ..Default::default()
        };
        for topic in [
            "review.wave.ready",
            "review.passed",
            "queue.advance",
            "plan.complete",
            "plan.blocked",
        ] {
            let decision = check_topic_deny_rules(Some("ralph"), topic, &config);
            assert!(
                matches!(decision, Some(PolicyDecision::Block(_))),
                "ralph must be blocked from '{topic}', got {:?}",
                decision
            );
        }
    }

    #[test]
    fn test_u4_topic_deny_rules_ralph_unchanged_for_control_topics() {
        // Control topics (e.g. task.resume, LOOP_COMPLETE) must NOT be
        // blocked for ralph — they are ralph's legitimate surface.
        // The ralph deny list only covers business topics.
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            topic_deny_rules: vec![
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "review.wave.ready".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "review.passed".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "queue.advance".to_string(),
                },
            ],
            ..Default::default()
        };
        assert!(check_topic_deny_rules(Some("ralph"), "task.resume", &config).is_none());
        assert!(check_topic_deny_rules(Some("ralph"), "LOOP_COMPLETE", &config).is_none());
        assert!(check_topic_deny_rules(Some("ralph"), "human.guidance", &config).is_none());
    }

    #[test]
    fn test_u4_topic_deny_rules_executor_build_done_preserved() {
        // Regression: the original `executor → build.done` deny rule must
        // still fire after the U4 additions. Otherwise a worktree-loop
        // executor could impersonate the review-synthesizer again.
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![
                TopicDenyRule {
                    hat_id: "executor".to_string(),
                    topic: "build.done".to_string(),
                },
                TopicDenyRule {
                    hat_id: "ralph".to_string(),
                    topic: "review.passed".to_string(),
                },
            ],
            ..Default::default()
        };
        assert!(matches!(
            check_topic_deny_rules(Some("executor"), "build.done", &config),
            Some(PolicyDecision::Block(_))
        ));
        // And the new ralph rule still fires.
        assert!(matches!(
            check_topic_deny_rules(Some("ralph"), "review.passed", &config),
            Some(PolicyDecision::Block(_))
        ));
    }

    // -------------------------------------------------------------------------
    // U4: plan_name equality tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_plan_name_equality_matches_accepted() {
        // work.ready with plan_name=A → work.done with plan_name=A → Accept
        let mut config = test_config();
        config.plan_name_equality_required = true;
        let mut state = PolicyRuntimeState::default();
        state.current_plan_name = Some("plan-x".to_string());

        let decision = validate_event(
            "work.done",
            Some(r#"{"plan_name": "plan-x"}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_plan_name_equality_mismatch_rejected() {
        // work.ready with plan_name=A → work.done with plan_name=B → Reject
        let mut config = test_config();
        config.plan_name_equality_required = true;
        let mut state = PolicyRuntimeState::default();
        state.current_plan_name = Some("plan-x".to_string());

        let decision = validate_event(
            "work.done",
            Some(r#"{"plan_name": "plan-y"}"#),
            &config,
            &mut state,
        );
        let is_rejected = matches!(decision, PolicyDecision::RejectWithResume(PolicyFinding {
            violation_type: ViolationType::InvalidFieldValue { ref field, .. }, ..
        }) if field == "plan_name");
        assert!(
            is_rejected,
            "Expected RejectWithResume for plan_name mismatch, got {:?}",
            decision
        );
    }

    #[test]
    fn test_plan_name_equality_disabled_accepts_mismatch() {
        // plan_name_equality_required=false (default) → work.done plan_name=B still accepted
        let config = test_config(); // default has plan_name_equality_required=false
        let mut state = PolicyRuntimeState::default();
        state.current_plan_name = Some("plan-x".to_string());

        let decision = validate_event(
            "work.done",
            Some(r#"{"plan_name": "plan-y"}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_plan_name_equality_no_work_ready_skips_check() {
        // No work.ready → current_plan_name is None → skip check
        let mut config = test_config();
        config.plan_name_equality_required = true;
        let mut state = PolicyRuntimeState::default();
        // current_plan_name is None (no work.ready received)

        let decision = validate_event(
            "work.done",
            Some(r#"{"plan_name": "anything"}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }
}
