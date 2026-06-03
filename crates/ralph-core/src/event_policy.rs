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
}

impl PolicyRuntimeState {
    /// Replays events from a JSONL file to build up the policy runtime state.
    ///
    /// Reads all events from the file, tracking which terminal topics have been
    /// observed and which business topics have been seen. Malformed lines are
    /// skipped. String, object, and null payloads are all handled with the same
    /// compatibility semantics as `EventReader`.
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
    use crate::config::EventSchema;
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
}
