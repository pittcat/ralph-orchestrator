//! State machine validator for instance lifecycle enforcement.
//!
//! This module implements pure Rust state machine validation that operates
//! independently of the event loop, allowing it to be unit tested without
//! filesystem or EventBus dependencies.

use crate::config::{BusinessAfterTerminalAction, DuplicateTerminalAction, StateMachineConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

// ---------------------------------------------------------------------------
// Public Types
// ---------------------------------------------------------------------------

/// Result of state machine validation for a single event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StateMachineDecision {
    /// Event is accepted. Contains the instance key (if tracked) and new state.
    Accept {
        instance_key: Option<String>,
        new_state: String,
    },
    /// Event is rejected. A diagnostic event should be published but the event
    /// must not be recorded or published to the business bus.
    Reject { finding: StateMachineFinding },
    /// Event should be silently ignored (e.g., duplicate noise).
    Ignore { finding: StateMachineFinding },
    /// Event validation produced useful diagnostic information but the event
    /// should not be treated as a rejection (for observe-mode tooling).
    DiagnosticOnly { finding: StateMachineFinding },
}

/// Structured finding from a state machine violation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateMachineFinding {
    /// The event topic that was rejected.
    pub topic: String,
    /// The instance key if one was extractable.
    pub instance_key: Option<String>,
    /// The current state of the instance (or "idle" if unknown).
    pub current_state: String,
    /// The states that were expected for this transition.
    pub expected_states: Vec<String>,
    /// Human-readable reason for the rejection.
    pub reason: String,
}

/// Runtime state for an open instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceState {
    /// The current state of this instance.
    pub state: String,
    /// The last topic that was observed for this instance.
    pub last_topic: String,
}

/// Persistent runtime state maintained across the loop lifetime.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateMachineRuntimeState {
    /// Map of instance key -> current state for all open instances.
    open_instances: HashMap<String, InstanceState>,
    /// Map of instance key -> final state for all closed instances.
    closed_instances: HashMap<String, InstanceState>,
    /// Whether a terminal event has been observed at least once.
    terminal_observed: bool,
    /// Whether a terminal event has been honored (accepted + recorded).
    terminal_honored: bool,
    /// Fingerprint of the last terminal rejection to prevent repeat injections.
    last_terminal_rejection: Option<TerminalRejectionFingerprint>,
    /// Count of accepted state machine transitions (business events that advanced state).
    accepted_transition_count: u32,
}

/// Fingerprint of a terminal rejection to detect repeated rejections.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct TerminalRejectionFingerprint {
    /// The terminal topic that was rejected.
    topic: String,
    /// The reason for rejection (to detect same rejection repeated).
    reason: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

impl StateMachineRuntimeState {
    /// Creates a new empty runtime state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true if there are any open instances.
    pub fn has_open_instances(&self) -> bool {
        !self.open_instances.is_empty()
    }

    /// Returns the count of open instances.
    pub fn open_instance_count(&self) -> usize {
        self.open_instances.len()
    }

    /// Returns a snapshot of open instance keys and their states.
    pub fn open_instances_snapshot(&self) -> HashMap<String, InstanceState> {
        self.open_instances.clone()
    }

    /// Returns a snapshot of closed instance keys and their final states.
    pub fn closed_instances_snapshot(&self) -> HashMap<String, InstanceState> {
        self.closed_instances.clone()
    }

    /// Returns whether a terminal event has been honored.
    pub fn is_terminal_honored(&self) -> bool {
        self.terminal_honored
    }

    /// Returns the count of accepted state machine transitions.
    ///
    /// Used by the progress fingerprint to detect state machine progress
    /// between completion rejections.
    pub fn accepted_transition_count(&self) -> u32 {
        self.accepted_transition_count
    }

    /// Marks an accepted terminal event as honored after the event loop's
    /// completion checks have succeeded.
    pub fn mark_terminal_honored(&mut self) {
        if self.terminal_observed {
            self.terminal_honored = true;
        }
    }

    /// Validates a single event against the state machine configuration.
    ///
    /// Returns the validation decision along with any state changes that should
    /// be applied if the event is accepted.
    pub fn validate_event(
        &mut self,
        topic: &str,
        payload: Option<&str>,
        config: &StateMachineConfig,
    ) -> StateMachineDecision {
        // Fast path: if state machine is disabled, accept everything
        if !config.enabled {
            return StateMachineDecision::Accept {
                instance_key: None,
                new_state: String::new(),
            };
        }

        // Fast path: non-business topics pass through without validation
        if !config.business_topics.iter().any(|t| t.as_str() == topic)
            && !config.terminal_topics.iter().any(|t| t.as_str() == topic)
        {
            return StateMachineDecision::Accept {
                instance_key: None,
                new_state: String::new(),
            };
        }

        let instance_key = self.extract_instance_key(topic, payload, config);

        // Terminal event handling
        if config.terminal_topics.iter().any(|t| t.as_str() == topic) {
            return self.validate_terminal_event(topic, instance_key, config);
        }

        // Business event handling
        return self.validate_business_event(topic, payload, instance_key, config);
    }

    /// Extracts the instance key from the event payload if required.
    fn extract_instance_key(
        &self,
        topic: &str,
        payload: Option<&str>,
        config: &StateMachineConfig,
    ) -> Option<String> {
        // If topic is not in required_for list, instance key is not required
        if !config
            .instance_key
            .required_for
            .iter()
            .any(|t| t.as_str() == topic)
        {
            return None;
        }

        let payload = payload?;

        // Parse JSON and extract the field
        let value: serde_json::Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(_) => return None,
        };

        let field_name = &config.instance_key.from_payload;
        let obj = match &value {
            serde_json::Value::Object(map) => map,
            _ => return None,
        };

        let key_value = obj.get(field_name)?;

        match key_value {
            serde_json::Value::String(s) => Some(s.clone()),
            _ => None,
        }
    }

    /// Validates a terminal event against the state machine configuration.
    fn validate_terminal_event(
        &mut self,
        topic: &str,
        instance_key: Option<String>,
        config: &StateMachineConfig,
    ) -> StateMachineDecision {
        let guard = &config.terminal_guard;

        // Check if terminal has already been honored
        if self.terminal_honored {
            match guard.duplicate_terminal {
                DuplicateTerminalAction::Reject => {
                    let finding = StateMachineFinding {
                        topic: topic.to_string(),
                        instance_key,
                        current_state: "terminal".to_string(),
                        expected_states: vec![],
                        reason: "Duplicate terminal event after terminal already honored"
                            .to_string(),
                    };
                    // Record this rejection to prevent repeat
                    self.last_terminal_rejection = Some(TerminalRejectionFingerprint {
                        topic: topic.to_string(),
                        reason: finding.reason.clone(),
                    });
                    return StateMachineDecision::Reject { finding };
                }
                DuplicateTerminalAction::Ignore => {
                    let finding = StateMachineFinding {
                        topic: topic.to_string(),
                        instance_key,
                        current_state: "terminal".to_string(),
                        expected_states: vec![],
                        reason: "Duplicate terminal event ignored".to_string(),
                    };
                    return StateMachineDecision::Ignore { finding };
                }
            }
        }

        // Check for open instances if required
        if guard.require_no_open_instances && self.has_open_instances() {
            let open_keys: Vec<_> = self.open_instances.keys().cloned().collect();
            let finding = StateMachineFinding {
                topic: topic.to_string(),
                instance_key,
                current_state: "terminal".to_string(),
                expected_states: vec![],
                reason: format!(
                    "Terminal event rejected: {} open instance(s) still active: {}",
                    open_keys.len(),
                    open_keys.join(", ")
                ),
            };
            // Record this rejection to prevent repeat
            self.last_terminal_rejection = Some(TerminalRejectionFingerprint {
                topic: topic.to_string(),
                reason: finding.reason.clone(),
            });
            return StateMachineDecision::Reject { finding };
        }

        // Terminal event is acceptable
        self.terminal_observed = true;
        StateMachineDecision::Accept {
            instance_key,
            new_state: "terminal".to_string(),
        }
    }

    /// Validates a business event against the state machine configuration.
    fn validate_business_event(
        &mut self,
        topic: &str,
        _payload: Option<&str>,
        instance_key: Option<String>,
        config: &StateMachineConfig,
    ) -> StateMachineDecision {
        let guard = &config.terminal_guard;

        // Check for business after terminal
        if self.terminal_observed && !self.terminal_honored {
            // Terminal was seen but not yet honored — this is a same-batch situation
            // handled by the completion guard in the event loop
        }

        if self.terminal_honored {
            match guard.business_after_terminal {
                BusinessAfterTerminalAction::Reject => {
                    let finding = StateMachineFinding {
                        topic: topic.to_string(),
                        instance_key,
                        current_state: "terminal".to_string(),
                        expected_states: vec![],
                        reason: "Business event rejected after terminal event".to_string(),
                    };
                    return StateMachineDecision::Reject { finding };
                }
                BusinessAfterTerminalAction::Ignore => {
                    let finding = StateMachineFinding {
                        topic: topic.to_string(),
                        instance_key,
                        current_state: "terminal".to_string(),
                        expected_states: vec![],
                        reason: "Business event ignored after terminal event".to_string(),
                    };
                    return StateMachineDecision::Ignore { finding };
                }
            }
        }

        // If instance key is required but not provided, reject
        if config
            .instance_key
            .required_for
            .iter()
            .any(|t| t.as_str() == topic)
            && instance_key.is_none()
        {
            let finding = StateMachineFinding {
                topic: topic.to_string(),
                instance_key: None,
                current_state: "unknown".to_string(),
                expected_states: vec![],
                reason: format!(
                    "Instance key required for topic '{}' but not found in payload",
                    topic
                ),
            };
            return StateMachineDecision::Reject { finding };
        }

        // Find matching transition
        let transition = match config.transitions.iter().find(|t| t.topic == topic) {
            Some(t) => t,
            None => {
                let finding = StateMachineFinding {
                    topic: topic.to_string(),
                    instance_key,
                    current_state: "unknown".to_string(),
                    expected_states: Vec::new(),
                    reason: format!(
                        "No state machine transition configured for business topic '{}'",
                        topic
                    ),
                };
                return StateMachineDecision::Reject { finding };
            }
        };

        // Determine current state for this instance
        let current_state = match &instance_key {
            Some(key) => {
                if let Some(closed) = self.closed_instances.get(key) {
                    let finding = StateMachineFinding {
                        topic: topic.to_string(),
                        instance_key: instance_key.clone(),
                        current_state: closed.state.clone(),
                        expected_states: transition.from.clone(),
                        reason: format!(
                            "Instance '{}' has already been closed at state '{}' (topic '{}'). Reopening is not allowed.",
                            key, closed.state, closed.last_topic
                        ),
                    };
                    return StateMachineDecision::Reject { finding };
                }
                self.open_instances
                    .get(key)
                    .map(|s| s.state.clone())
                    .unwrap_or_else(|| "idle".to_string())
            }
            None => "idle".to_string(),
        };

        // Check if transition is valid from current state
        if !transition.from.contains(&current_state) && current_state != "idle" {
            let finding = StateMachineFinding {
                topic: topic.to_string(),
                instance_key: instance_key.clone(),
                current_state: current_state.clone(),
                expected_states: transition.from.clone(),
                reason: format!(
                    "Invalid transition: topic '{}' cannot go from state '{}' to '{}'. Expected one of: {:?}",
                    topic, current_state, transition.to, transition.from
                ),
            };
            return StateMachineDecision::Reject { finding };
        }

        // Special case: idle state allows transition if 'idle' is not explicitly in from
        if current_state == "idle" && !transition.from.contains(&"idle".to_string()) {
            // Check if this transition opens an instance from idle
            if !transition.opens_instance {
                let finding = StateMachineFinding {
                    topic: topic.to_string(),
                    instance_key: instance_key.clone(),
                    current_state: "idle".to_string(),
                    expected_states: transition.from.clone(),
                    reason: format!(
                        "Invalid transition: topic '{}' cannot open an instance from 'idle' state",
                        topic
                    ),
                };
                return StateMachineDecision::Reject { finding };
            }
        }

        // Apply the transition
        let new_state = transition.to.clone();
        let key = instance_key.clone();

        if let Some(ref k) = key {
            if transition.opens_instance {
                self.open_instances.insert(
                    k.clone(),
                    InstanceState {
                        state: new_state.clone(),
                        last_topic: topic.to_string(),
                    },
                );
            } else if transition.closes_instance {
                if self.open_instances.remove(k).is_some() {
                    self.closed_instances.insert(
                        k.clone(),
                        InstanceState {
                            state: new_state.clone(),
                            last_topic: topic.to_string(),
                        },
                    );
                }
            } else {
                // Advance state
                if let Some(instance) = self.open_instances.get_mut(k) {
                    instance.state = new_state.clone();
                    instance.last_topic = topic.to_string();
                }
            }
        }

        debug!(
            topic = topic,
            instance_key = ?instance_key,
            current_state = %current_state,
            new_state = %new_state,
            "State machine transition applied"
        );

        self.accepted_transition_count += 1;

        StateMachineDecision::Accept {
            instance_key: key,
            new_state,
        }
    }

    /// Checks if a terminal event with the same fingerprint was recently rejected.
    /// Used to prevent repeated injection of the same terminal rejection.
    pub fn is_terminal_rejection_repeated(&self, topic: &str, reason: &str) -> bool {
        self.last_terminal_rejection
            .as_ref()
            .is_some_and(|f| f.topic == topic && f.reason == reason)
    }

    /// Returns a summary of the current runtime state for snapshotting.
    pub fn summary(&self) -> StateMachineStateSummary {
        StateMachineStateSummary {
            open_instance_count: self.open_instances.len(),
            closed_instance_count: self.closed_instances.len(),
            terminal_observed: self.terminal_observed,
            terminal_honored: self.terminal_honored,
        }
    }
}

/// A snapshot of the state machine runtime state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateMachineStateSummary {
    /// Number of currently open instances.
    pub open_instance_count: usize,
    /// Number of closed instances.
    pub closed_instance_count: usize,
    /// Whether a terminal event has been observed.
    pub terminal_observed: bool,
    /// Whether a terminal event has been honored.
    pub terminal_honored: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::state_machine::{InstanceKeyConfig, TerminalGuardConfig, TransitionConfig};

    fn make_experiment_config() -> StateMachineConfig {
        StateMachineConfig {
            enabled: true,
            instance_key: InstanceKeyConfig {
                from_payload: "task_key".to_string(),
                required_for: vec![
                    "experiment.planned".to_string(),
                    "experiment.ready".to_string(),
                    "experiment.blocked".to_string(),
                ],
            },
            terminal_topics: vec!["LOOP_COMPLETE".to_string()],
            business_topics: vec![
                "experiment.planned".to_string(),
                "experiment.ready".to_string(),
                "experiment.blocked".to_string(),
            ],
            terminal_guard: TerminalGuardConfig::default(),
            transitions: vec![
                TransitionConfig {
                    topic: "experiment.planned".to_string(),
                    from: vec!["idle".to_string()],
                    to: "planned".to_string(),
                    opens_instance: true,
                    closes_instance: false,
                },
                TransitionConfig {
                    topic: "experiment.ready".to_string(),
                    from: vec!["planned".to_string()],
                    to: "ready".to_string(),
                    opens_instance: false,
                    closes_instance: false,
                },
                TransitionConfig {
                    topic: "experiment.blocked".to_string(),
                    from: vec![
                        "planned".to_string(),
                        "ready".to_string(),
                        "measured".to_string(),
                        "scored".to_string(),
                        "attacked".to_string(),
                    ],
                    to: "blocked".to_string(),
                    opens_instance: false,
                    closes_instance: true,
                },
            ],
        }
    }

    #[test]
    fn test_open_instance_from_idle() {
        let mut state = StateMachineRuntimeState::new();
        let config = make_experiment_config();

        let decision =
            state.validate_event("experiment.planned", Some(r#"{"task_key": "t1"}"#), &config);

        match decision {
            StateMachineDecision::Accept {
                instance_key,
                new_state,
            } => {
                assert_eq!(instance_key, Some("t1".to_string()));
                assert_eq!(new_state, "planned");
                assert!(state.open_instances.contains_key("t1"));
            }
            _ => panic!("Expected Accept, got {:?}", decision),
        }
    }

    #[test]
    fn test_advance_instance() {
        let mut state = StateMachineRuntimeState::new();
        let config = make_experiment_config();

        // Open instance
        state.validate_event("experiment.planned", Some(r#"{"task_key": "t1"}"#), &config);

        // Advance instance
        let decision =
            state.validate_event("experiment.ready", Some(r#"{"task_key": "t1"}"#), &config);

        match decision {
            StateMachineDecision::Accept {
                instance_key,
                new_state,
            } => {
                assert_eq!(instance_key, Some("t1".to_string()));
                assert_eq!(new_state, "ready");
                assert_eq!(state.open_instances.get("t1").unwrap().state, "ready");
            }
            _ => panic!("Expected Accept, got {:?}", decision),
        }
    }

    #[test]
    fn test_branch_close_from_planned() {
        let mut state = StateMachineRuntimeState::new();
        let config = make_experiment_config();

        // Open instance
        state.validate_event("experiment.planned", Some(r#"{"task_key": "t1"}"#), &config);

        // Close from planned
        let decision =
            state.validate_event("experiment.blocked", Some(r#"{"task_key": "t1"}"#), &config);

        match decision {
            StateMachineDecision::Accept { new_state, .. } => {
                assert_eq!(new_state, "blocked");
                assert!(!state.open_instances.contains_key("t1"));
                assert!(state.closed_instances.contains_key("t1"));
                assert_eq!(state.closed_instances.get("t1").unwrap().state, "blocked");
                assert_eq!(
                    state.closed_instances.get("t1").unwrap().last_topic,
                    "experiment.blocked"
                );
            }
            _ => panic!("Expected Accept, got {:?}", decision),
        }
    }

    #[test]
    fn test_out_of_order_rejection() {
        let mut state = StateMachineRuntimeState::new();
        let config = make_experiment_config();

        // Try to advance before opening — should fail
        let decision =
            state.validate_event("experiment.ready", Some(r#"{"task_key": "t1"}"#), &config);

        match decision {
            StateMachineDecision::Reject { finding } => {
                assert_eq!(finding.current_state, "idle");
                assert!(finding.reason.contains("Invalid transition"));
            }
            _ => panic!("Expected Reject, got {:?}", decision),
        }
    }

    #[test]
    fn test_terminal_with_open_instance_rejected() {
        let mut state = StateMachineRuntimeState::new();
        let config = make_experiment_config();

        // Open instance but don't close it
        state.validate_event("experiment.planned", Some(r#"{"task_key": "t1"}"#), &config);

        // Try terminal with open instance
        let decision = state.validate_event("LOOP_COMPLETE", None, &config);

        match decision {
            StateMachineDecision::Reject { finding } => {
                assert!(finding.reason.contains("open instance"));
            }
            _ => panic!("Expected Reject, got {:?}", decision),
        }
    }

    #[test]
    fn test_terminal_after_all_closed_accepted() {
        let mut state = StateMachineRuntimeState::new();
        let config = make_experiment_config();

        // Open and immediately close
        state.validate_event("experiment.planned", Some(r#"{"task_key": "t1"}"#), &config);
        state.validate_event("experiment.blocked", Some(r#"{"task_key": "t1"}"#), &config);

        // Now terminal should be accepted
        let decision = state.validate_event("LOOP_COMPLETE", None, &config);

        match decision {
            StateMachineDecision::Accept { new_state, .. } => {
                assert_eq!(new_state, "terminal");
                assert!(!state.is_terminal_honored());
                state.mark_terminal_honored();
                assert!(state.is_terminal_honored());
            }
            _ => panic!("Expected Accept, got {:?}", decision),
        }
    }

    #[test]
    fn test_duplicate_terminal_rejected() {
        let mut state = StateMachineRuntimeState::new();
        let config = make_experiment_config();

        // Complete the loop cleanly
        state.validate_event("experiment.planned", Some(r#"{"task_key": "t1"}"#), &config);
        state.validate_event("experiment.blocked", Some(r#"{"task_key": "t1"}"#), &config);
        state.validate_event("LOOP_COMPLETE", None, &config);
        state.mark_terminal_honored();

        // Try duplicate terminal
        let decision = state.validate_event("LOOP_COMPLETE", None, &config);

        match decision {
            StateMachineDecision::Reject { finding } => {
                assert!(finding.reason.contains("Duplicate terminal"));
            }
            _ => panic!("Expected Reject, got {:?}", decision),
        }
    }

    #[test]
    fn test_business_after_terminal_rejected() {
        let mut state = StateMachineRuntimeState::new();
        let config = make_experiment_config();

        // Complete the loop
        state.validate_event("experiment.planned", Some(r#"{"task_key": "t1"}"#), &config);
        state.validate_event("experiment.blocked", Some(r#"{"task_key": "t1"}"#), &config);
        state.validate_event("LOOP_COMPLETE", None, &config);
        state.mark_terminal_honored();

        // Try business event after terminal
        let decision =
            state.validate_event("experiment.planned", Some(r#"{"task_key": "t2"}"#), &config);

        match decision {
            StateMachineDecision::Reject { finding } => {
                assert!(
                    finding
                        .reason
                        .contains("Business event rejected after terminal")
                );
            }
            _ => panic!("Expected Reject, got {:?}", decision),
        }
    }

    #[test]
    fn test_reopen_closed_instance_rejected() {
        let mut state = StateMachineRuntimeState::new();
        let config = make_experiment_config();

        // Open and close
        state.validate_event("experiment.planned", Some(r#"{"task_key": "t1"}"#), &config);
        state.validate_event("experiment.blocked", Some(r#"{"task_key": "t1"}"#), &config);

        // Try to reopen
        let decision =
            state.validate_event("experiment.planned", Some(r#"{"task_key": "t1"}"#), &config);

        match decision {
            StateMachineDecision::Reject { finding } => {
                assert!(finding.reason.contains("already been closed"));
            }
            _ => panic!("Expected Reject, got {:?}", decision),
        }
    }

    #[test]
    fn test_missing_instance_key_rejected() {
        let mut state = StateMachineRuntimeState::new();
        let config = make_experiment_config();

        let decision = state.validate_event(
            "experiment.planned",
            Some(r#"{"other_field": "value"}"#),
            &config,
        );

        match decision {
            StateMachineDecision::Reject { finding } => {
                assert!(finding.reason.contains("Instance key required"));
            }
            _ => panic!("Expected Reject, got {:?}", decision),
        }
    }

    #[test]
    fn test_disabled_state_machine_accepts_all() {
        let mut state = StateMachineRuntimeState::new();
        let mut config = make_experiment_config();
        config.enabled = false;

        let decision =
            state.validate_event("experiment.planned", Some(r#"{"task_key": "t1"}"#), &config);

        match decision {
            StateMachineDecision::Accept { instance_key, .. } => {
                assert!(instance_key.is_none());
                assert!(state.open_instances.is_empty());
            }
            _ => panic!("Expected Accept, got {:?}", decision),
        }
    }

    #[test]
    fn test_non_business_topic_passes_through() {
        let mut state = StateMachineRuntimeState::new();
        let config = make_experiment_config();

        let decision = state.validate_event("human.guidance", Some("guidance content"), &config);

        match decision {
            StateMachineDecision::Accept { instance_key, .. } => {
                assert!(instance_key.is_none());
            }
            _ => panic!("Expected Accept, got {:?}", decision),
        }
    }

    #[test]
    fn test_business_topic_without_transition_rejected() {
        let mut state = StateMachineRuntimeState::new();
        let mut config = make_experiment_config();
        config.business_topics.push("experiment.ready2".to_string());

        let decision =
            state.validate_event("experiment.ready2", Some(r#"{"task_key": "t1"}"#), &config);

        match decision {
            StateMachineDecision::Reject { finding } => {
                assert!(finding.reason.contains("No state machine transition"));
            }
            _ => panic!("Expected Reject, got {:?}", decision),
        }
    }
}
