//! State machine validator for instance lifecycle enforcement.
//!
//! This module implements pure Rust state machine validation that operates
//! independently of the event loop, allowing it to be unit tested without
//! filesystem or EventBus dependencies.

use crate::config::{BusinessAfterTerminalAction, DuplicateTerminalAction, StateMachineConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use tracing::debug;

// ---------------------------------------------------------------------------
// Public Types
// ---------------------------------------------------------------------------

/// Plan GAP-02 / Unit 1: stable identity for an accepted StateMachine
/// transition. Used as the idempotency key when a transition is replayed
/// from the commit log so the snapshot projection does not double-count
/// or repeat an open/close move. The string form is used in
/// [`crate::state::commit::CommitDelta::StateMachineTransition`] for
/// serde.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StateMachineTransitionId(pub String);

impl StateMachineTransitionId {
    /// Build the canonical identity for a freshly accepted transition.
    /// The fields are derived from the live event submission (loop id,
    /// contract identity, source hat) plus the StateMachine semantic
    /// fields (topic, instance key, and a content-derived semantic key).
    /// Iteration counters and batch indexes are not valid semantic keys.
    /// Hashing the canonical tuple avoids delimiter collisions and keeps the
    /// persisted identity compact.
    pub fn build(
        loop_id: &str,
        contract_id: Option<&str>,
        source_hat: &str,
        topic: &str,
        instance_key: Option<&str>,
        semantic_key: &str,
    ) -> Self {
        let canonical = (
            loop_id,
            contract_id.unwrap_or(""),
            source_hat,
            topic,
            instance_key.unwrap_or(""),
            semantic_key,
        );
        let bytes = serde_json::to_vec(&canonical).expect("transition identity tuple serializes");
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(format!("sm-v2:{:x}", hasher.finalize()))
    }

    /// Expose the underlying string for ledger / outbox comparisons.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Plan GAP-02 / Unit 1: minimal semantic delta describing a single
/// accepted StateMachine transition. Carries only the fields required to
/// replay the runtime projection — no full `StateMachineRuntimeState`
/// snapshot (per plan §1.9 "performance"). The `transition_id` is the
/// idempotency key; replay drop-duplicates on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateMachineTransitionDelta {
    /// Stable identity used to dedupe replay.
    pub transition_id: StateMachineTransitionId,
    /// Hat that produced the semantic transition. Persisted separately so
    /// migration fingerprints retain the source discriminator.
    #[serde(default)]
    pub source_hat: Option<String>,
    /// Topic of the accepted event.
    pub topic: String,
    /// Instance key, if the transition targets a tracked instance.
    #[serde(default)]
    pub instance_key: Option<String>,
    /// New state of the instance after the transition.
    pub new_state: String,
    /// Whether the transition opens a new instance.
    pub opens_instance: bool,
    /// Whether the transition closes the instance.
    pub closes_instance: bool,
    /// Whether this transition sets `terminal_observed`.
    #[serde(default)]
    pub terminal_observed: bool,
    /// Whether this transition sets `terminal_honored`.
    #[serde(default)]
    pub terminal_honored: bool,
}

/// Build a replay-stable semantic fingerprint for a projected transition.
/// This intentionally excludes `transition_id`: it is the bridge that lets
/// old ledger IDs and v2 IDs participate in the same deduplication set.
fn transition_fingerprint(delta: &StateMachineTransitionDelta) -> String {
    let legacy_source_hat = delta
        .source_hat
        .as_deref()
        .or_else(|| legacy_source_hat(delta.transition_id.as_str()));
    let canonical = (
        legacy_source_hat,
        &delta.topic,
        &delta.instance_key,
        &delta.new_state,
        delta.opens_instance,
        delta.closes_instance,
        delta.terminal_observed,
        delta.terminal_honored,
    );
    let bytes = serde_json::to_vec(&canonical).expect("transition fingerprint serializes");
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("tf-v1:{:x}", hasher.finalize())
}

/// Recover the source discriminator from the pre-v2 six-segment identity.
/// New `sm-v2:` IDs are opaque and always carry `source_hat` in the delta.
fn legacy_source_hat(id: &str) -> Option<&str> {
    let mut fields = id.split('|');
    let loop_id = fields.next()?;
    let _ = loop_id;
    let _contract_id = fields.next()?;
    let source_hat = fields.next()?;
    let _topic = fields.next()?;
    let _instance_key = fields.next()?;
    let _sequence = fields.next()?;
    if fields.next().is_some() || source_hat.is_empty() {
        return None;
    }
    Some(source_hat)
}

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
    /// Plan GAP-02 / Unit 1: idempotency set for accepted transitions
    /// materialized through the ledger. The replay path uses this set
    /// to drop duplicate `transition_id`s so a re-applied delta never
    /// double-counts or re-runs an open/close move.
    #[serde(default)]
    applied_transition_ids: HashSet<StateMachineTransitionId>,
    /// Semantic fallback used when a pre-v2 ledger entry is replayed before
    /// a new v2 transition identity is produced. This bridges the persisted
    /// identity-format migration without trusting the legacy numeric suffix.
    #[serde(default)]
    applied_transition_fingerprints: HashSet<String>,
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

    /// Returns whether a terminal event has been observed.
    /// Plan GAP-02 / Unit 2 helper.
    pub fn is_terminal_observed(&self) -> bool {
        self.terminal_observed
    }

    /// Plan GAP-02 / Unit 2 helper: combined observed/honored
    /// flag snapshot used by the candidate stage to thread the
    /// post-validator state into the projection.
    pub fn observed_snapshot(&self) -> (bool, bool) {
        (self.terminal_observed, self.terminal_honored)
    }

    /// Plan GAP-02 / Unit 2 helper: read-only access to the
    /// open/closed maps so the candidate stage can detect
    /// open → close / open → advance transitions without
    /// exposing the private `HashMap` fields.
    pub fn instance_maps(
        &self,
    ) -> (
        &HashMap<String, InstanceState>,
        &HashMap<String, InstanceState>,
    ) {
        (&self.open_instances, &self.closed_instances)
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
        self.validate_business_event(topic, payload, instance_key, config)
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

    /// Plan GAP-02 / Unit 1: project an accepted [`StateMachineDecision`]
    /// into a replayable semantic delta. The caller supplies the
    /// `opens_instance` and `closes_instance` boolean flags because
    /// the underlying `validate_event` mutates the live runtime
    /// before this projection runs (the open→close decision
    /// belongs to the validator, not to a post-hoc introspection).
    /// Unit 2 wires the candidate stage; until then the helper
    /// accepts the flags explicitly to keep the projection honest.
    ///
    /// Plan GAP-02 / Unit 2: the caller also supplies the terminal
    /// `terminal_observed` / `terminal_honored` flags captured at
    /// candidate stage (`accepted_at_terminal_observed/honored`).
    /// Reading them from `self` (live) would lose the candidate's
    /// view because the live runtime is not yet mutated by this
    /// delta. The wire shape of `StateMachineTransitionDelta` is
    /// unchanged; only the input source is corrected.
    pub fn project_transition_delta(
        &self,
        transition_id: StateMachineTransitionId,
        topic: &str,
        decision: &StateMachineDecision,
        opens_instance: bool,
        closes_instance: bool,
        terminal_observed: bool,
        terminal_honored: bool,
    ) -> Option<StateMachineTransitionDelta> {
        // Plan §1.4 — only accepted business/terminal projections become
        // ledger material; rejection/diagnostic decisions are not
        // materialised.
        let (instance_key, new_state) = match decision {
            StateMachineDecision::Accept {
                instance_key,
                new_state,
            } => (instance_key.clone(), new_state.clone()),
            _ => return None,
        };

        Some(StateMachineTransitionDelta {
            transition_id,
            source_hat: None,
            topic: topic.to_string(),
            instance_key,
            new_state,
            opens_instance,
            closes_instance,
            // Unit 2: terminal flags come from the candidate snapshot,
            // not the live runtime. The candidate stage already recorded
            // them via `accepted_at_terminal_observed/honored`; reading
            // from `self` here would be stale until apply mutates live.
            terminal_observed,
            terminal_honored,
        })
    }

    /// Read-only side-channel for callers that want the inferred
    /// open/close classification *before* calling
    /// `validate_event` (which mutates `self`). Takes a snapshot
    /// of the pre-mutation state and the candidate
    /// `instance_key`, and returns the flags that the validator's
    /// open/close decision would produce.
    pub fn classify_open_close(&self, key: &Option<String>) -> (bool, bool) {
        let key = match key {
            Some(k) => k,
            None => return (false, false),
        };
        let opens =
            !self.open_instances.contains_key(key) && !self.closed_instances.contains_key(key);
        let closes =
            self.open_instances.contains_key(key) && !self.closed_instances.contains_key(key);
        (opens, closes)
    }

    /// Plan GAP-02 / Unit 1: apply a replay-only delta onto the
    /// live runtime. Idempotent: if the `transition_id` has already
    /// been materialised (via this helper or a prior live accept),
    /// the call is a no-op. Returns `true` when the delta was
    /// applied.
    ///
    /// On a close delta, `apply_transition_delta` always moves the
    /// key from `open_instances` to `closed_instances` if the
    /// key was tracked in `open_instances`. A close delta
    /// arriving on a cold-start replay where the open instance was
    /// never recorded is a no-op for the instance maps (the
    /// closed map stays empty) — this is the safe behaviour for
    /// partial-restart paths.
    pub fn apply_transition_delta(&mut self, delta: &StateMachineTransitionDelta) -> bool {
        let fingerprint = transition_fingerprint(delta);
        if self.applied_transition_ids.contains(&delta.transition_id)
            || self.applied_transition_fingerprints.contains(&fingerprint)
        {
            return false;
        }
        if let Some(key) = delta.instance_key.as_deref() {
            if delta.closes_instance {
                self.open_instances.remove(key);
                self.closed_instances.insert(
                    key.to_string(),
                    InstanceState {
                        state: delta.new_state.clone(),
                        last_topic: delta.topic.clone(),
                    },
                );
            } else if delta.opens_instance {
                self.open_instances.insert(
                    key.to_string(),
                    InstanceState {
                        state: delta.new_state.clone(),
                        last_topic: delta.topic.clone(),
                    },
                );
            } else if let Some(instance) = self.open_instances.get_mut(key) {
                instance.state = delta.new_state.clone();
                instance.last_topic = delta.topic.clone();
            }
        }
        if delta.terminal_observed {
            self.terminal_observed = true;
        }
        if delta.terminal_honored {
            self.terminal_honored = true;
        }
        self.accepted_transition_count = self.accepted_transition_count.saturating_add(1);
        self.applied_transition_ids
            .insert(delta.transition_id.clone());
        self.applied_transition_fingerprints.insert(fingerprint);
        true
    }

    /// Read-only view over the applied-transition identity set.
    /// Used by the snapshot apply path to mirror the runtime set on
    /// the snapshot so `LedgerSnapshot` can also enforce replay
    /// idempotency (Plan E11).
    pub fn applied_transition_ids(&self) -> Vec<String> {
        self.applied_transition_ids
            .iter()
            .map(|id| id.0.clone())
            .collect()
    }

    /// Returns true if a transition id was already applied to this
    /// runtime. Used by `LedgerSnapshot::apply_delta` to dedupe
    /// before delegating.
    pub fn has_applied_transition_id(&self, id: &StateMachineTransitionId) -> bool {
        self.applied_transition_ids.contains(id)
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

    // PMI-013 reproducer: a re-submitted event must produce the same
    // `transition_id` regardless of the caller's `sequence` choice.
    // A retry of the same semantic event must reuse its content-derived
    // identity even when it happens in a different loop iteration.
    #[test]
    fn test_pmi013_transition_id_is_iteration_stable() {
        // Same semantic event uses the same content-derived identity.
        let id_iter5 = StateMachineTransitionId::build(
            "loop-A",
            Some("contract-X"),
            "executor",
            "experiment.planned",
            Some("task:t1"),
            r#"{"task_key":"t1"}"#,
        );
        let id_iter7 = StateMachineTransitionId::build(
            "loop-A",
            Some("contract-X"),
            "executor",
            "experiment.planned",
            Some("task:t1"),
            r#"{"task_key":"t1"}"#,
        );

        // Invariant: re-submission of the same semantic event from a
        // different iteration must yield the same identity, so the
        // runtime's `applied_transition_ids` dedup set holds.
        assert_eq!(
            id_iter5, id_iter7,
            "PMI-013: same semantic event from different iterations must produce the same transition_id"
        );

        // Same check via the runtime apply path: a delta replayed with
        // the same semantic identity must be recognized as already applied.
        let mut runtime = StateMachineRuntimeState::new();
        let delta_iter5 = StateMachineTransitionDelta {
            transition_id: id_iter5.clone(),
            source_hat: Some("executor".to_string()),
            topic: "experiment.planned".to_string(),
            instance_key: Some("task:t1".to_string()),
            new_state: "planned".to_string(),
            opens_instance: true,
            closes_instance: false,
            terminal_observed: false,
            terminal_honored: false,
        };
        let delta_iter7 = StateMachineTransitionDelta {
            transition_id: id_iter7.clone(),
            source_hat: Some("executor".to_string()),
            topic: "experiment.planned".to_string(),
            instance_key: Some("task:t1".to_string()),
            new_state: "planned".to_string(),
            opens_instance: true,
            closes_instance: false,
            terminal_observed: false,
            terminal_honored: false,
        };

        assert!(
            runtime.apply_transition_delta(&delta_iter5),
            "first apply must succeed"
        );
        assert!(
            !runtime.apply_transition_delta(&delta_iter7),
            "PMI-013: re-submission from a different iteration must dedup, not double-apply"
        );
        assert_eq!(
            runtime.accepted_transition_count, 1,
            "PMI-013: dedup must hold across iterations; runtime must not double-count"
        );

        // A legacy ledger entry must also dedupe against the new v2 identity
        // after restart. This protects the persisted migration boundary.
        let mut migrated_runtime = StateMachineRuntimeState::new();
        let legacy_delta = StateMachineTransitionDelta {
            transition_id: StateMachineTransitionId(
                "loop-A|contract-X|executor|experiment.planned|task:t1|1".to_string(),
            ),
            ..delta_iter5.clone()
        };
        assert!(migrated_runtime.apply_transition_delta(&legacy_delta));
        assert!(!migrated_runtime.apply_transition_delta(&delta_iter5));
        assert_eq!(migrated_runtime.accepted_transition_count, 1);

        let mut legacy_hat_runtime = StateMachineRuntimeState::new();
        let legacy_other_hat = StateMachineTransitionDelta {
            transition_id: StateMachineTransitionId(
                "loop-A|contract-X|sm-handler|experiment.planned|task:t1|1".to_string(),
            ),
            source_hat: None,
            ..legacy_delta.clone()
        };
        assert!(legacy_hat_runtime.apply_transition_delta(&legacy_delta));
        assert!(legacy_hat_runtime.apply_transition_delta(&legacy_other_hat));
        assert_eq!(legacy_hat_runtime.accepted_transition_count, 2);
    }

    // PMI-014 reproducer (deepen-once): the SSoT fix already partitions
    // `transition_id` by `source_hat`, so two distinct hats firing on the
    // same semantic tuple
    // produce distinct identities and `apply_transition_delta` dedupes
    // them independently. This is the LOW_CONFIDENCE / redo-first path
    // for PMI-014: the design already includes the discriminator, so a
    // hypothetical collision is bounded by hat-name discipline in
    // `from_runtime_config` (each hat is registered once). The test below
    // pins that guarantee so a future refactor that drops `source_hat`
    // from the tuple fails fast.
    #[test]
    fn test_pmi014_source_hat_partitions_transition_id() {
        // Same semantic tuple
        // tuple, distinct source_hat values — identities must diverge.
        let id_wave_scope = StateMachineTransitionId::build(
            "loop-A",
            Some("contract-X"),
            "wave-scope",
            "experiment.planned",
            Some("task:t1"),
            "planned:t1",
        );
        let id_sm_handler = StateMachineTransitionId::build(
            "loop-A",
            Some("contract-X"),
            "sm-handler",
            "experiment.planned",
            Some("task:t1"),
            "planned:t1",
        );

        assert_ne!(
            id_wave_scope, id_sm_handler,
            "PMI-014: source_hat must partition the canonical identity; \
            dropping source_hat from the tuple reintroduces cross-source \
            collision risk (PMI-014 LOW_CONFIDENCE)"
        );

        // Both deltas apply independently to the runtime — neither is
        // silently dropped, and the dedup set is keyed on the full tuple.
        let mut runtime = StateMachineRuntimeState::new();
        let delta_wave = StateMachineTransitionDelta {
            transition_id: id_wave_scope.clone(),
            source_hat: Some("wave-scope".to_string()),
            topic: "experiment.planned".to_string(),
            instance_key: Some("task:t1".to_string()),
            new_state: "planned".to_string(),
            opens_instance: true,
            closes_instance: false,
            terminal_observed: false,
            terminal_honored: false,
        };
        let delta_sm = StateMachineTransitionDelta {
            transition_id: id_sm_handler.clone(),
            source_hat: Some("sm-handler".to_string()),
            topic: "experiment.planned".to_string(),
            instance_key: Some("task:t1".to_string()),
            new_state: "planned".to_string(),
            opens_instance: true,
            closes_instance: false,
            terminal_observed: false,
            terminal_honored: false,
        };

        assert!(
            runtime.apply_transition_delta(&delta_wave),
            "wave-scope apply must succeed on first submission"
        );
        assert!(
            runtime.apply_transition_delta(&delta_sm),
            "PMI-014: sm-handler with same topic/key/seq must NOT be \
            silently dropped when source_hat partitions the tuple"
        );
        assert_eq!(
            runtime.accepted_transition_count, 2,
            "PMI-014: distinct source_hats on the same tuple must both apply"
        );
    }
}
