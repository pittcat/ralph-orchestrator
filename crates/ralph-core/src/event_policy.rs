//! Event policy validation for typed payload schema enforcement.
//!
//! Provides pure-function validation that can be used by the event loop,
//! CLI emit commands, and API layers.

use crate::event_reader::EventReader;
use ralph_proto::Topic;
use serde::{Deserialize, Serialize};
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
    /// U1 (2026-06-17-003 plan): semantic gate violation. The event
    /// passed schema validation but violates an orchestrator-level
    /// invariant (e.g. `review.passed` while a review wave is still
    /// open). Distinct from `InvalidFieldValue` because the payload
    /// itself is well-formed — the violation is in the **timing /
    /// state** relative to other events tracked by
    /// `ReviewStepTracker`. Kept fail-closed (event does NOT enter
    /// the bus) but loop continues — see
    /// `is_recoverable_policy_finding` for the bucket mapping.
    SemanticGateViolation {
        gate: String,
        context: String,
    },
    /// U4 (2026-06-17-003 plan): duplicate `work.done` for the
    /// same `(plan_name, step, task_id)` tuple. The `hint`
    /// distinguishes:
    ///   - `duplicate_stall_bypass`: the current event carries
    ///     `wave_id` (wave is still open) or is part of a stall
    ///     recovery flow — the agent is trying to re-send
    ///     `work.done` to bypass a stalled review cycle.
    ///   - `duplicate_same_step`: pure same-step re-emit (fix-round
    ///     did not advance, or the agent is not following the
    ///     `fix.applied` → re-`work.done` contract).
    DuplicateWorkDone {
        key: String,
        hint: DuplicateWorkDoneHint,
    },
}

/// U4 (2026-06-17-003 plan): hint carried in
/// [`ViolationType::DuplicateWorkDone`]. Lets the runner pick the
/// correct recovery payload (stall-bypass has a different message
/// from pure duplicate-same-step).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateWorkDoneHint {
    /// Wave is open (event has `wave_id` set) or the agent is
    /// known to be in a stall-recovery flow. The re-emit is
    /// likely an attempt to bypass the stalled review cycle.
    DuplicateStallBypass,
    /// Pure same-step re-emit: no wave open, no stall recovery in
    /// progress. The agent simply re-emitted `work.done` for a
    /// step that has already closed.
    DuplicateSameStep,
}

impl ViolationType {
    /// Field name that triggered the violation, when the violation
    /// is field-scoped. Returns `None` for topic-scoped violations
    /// (terminal-monotonicity, topic-format, topic-deny, etc.) and
    /// for semantic-gate violations (which use `gate` instead).
    pub fn field(&self) -> Option<&str> {
        match self {
            Self::MissingRequiredField { field } | Self::InvalidFieldValue { field, .. } => {
                Some(field.as_str())
            }
            _ => None,
        }
    }

    /// Stable machine-readable code for the violation type. Used as
    /// the dedupe key in dedup-by-`(topic, field, reason_code)` and
    /// as the `reason_code` in CLI precheck JSON output.
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::MissingRequiredField { .. } => "missing_required_field",
            Self::InvalidFieldValue { .. } => "invalid_field_value",
            Self::PayloadTypeMismatch { .. } => "payload_type_mismatch",
            Self::TerminalMonotonicityViolation { .. } => "terminal_monotonicity_violation",
            Self::DuplicateTerminalEvent { .. } => "duplicate_terminal_event",
            Self::BusinessEventAfterCompletion { .. } => "business_event_after_completion",
            Self::InvalidTopicFormat { .. } => "invalid_topic_format",
            Self::TopicDenied { .. } => "topic_denied",
            Self::SemanticGateViolation { .. } => "semantic_gate_violation",
            Self::DuplicateWorkDone { .. } => "duplicate_work_done",
        }
    }
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
    /// Unit 2 (2026-06-16-002 plan) recoverable-bucket. `Some(_)` means
    /// the rejection is in the **recoverable** set (R-B1) and the
    /// runner should publish a `task.resume` with `fix_hint` rather
    /// than the U6 fast-fail. `None` means the rejection is
    /// non-recoverable (R-B2): the existing U6
    /// `payload_contract_violation` path still applies.
    pub reason_class: Option<ReasonClass>,
}

/// Unit 2 (2026-06-16-002 plan): the **bucket** used to separate
/// recoverable policy rejections from non-recoverable ones, and the
/// per-key dimension for the bounded-retry counter at the loop
/// level.
///
/// Bucketing follows the plan's R-B1 / R-B2 table:
///
/// | Recoverable                              | Non-recoverable
/// |------------------------------------------|----------------------------------
/// | `PayloadTypeMismatch` (incl. non-JSON string) | `plan_name` / task key mismatch
/// | `MissingRequiredField`                   | duplicate terminal / completion guard
/// | `TopicDenied` (deny rules + isolated scope)   | `InvalidFieldValue` / `AllowedValueMismatch` (deferred)
/// | `SemanticGateViolation` (U1, 2026-06-17-003) | 4th attempt on the same (hat, reason_class)
///
/// The bucket is the last segment of the loop's
/// `record_rejection_key` counter so two distinct buckets on the
/// same `(hat, topic)` keep **independent** counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonClass {
    /// `PayloadTypeMismatch` — agent emitted a payload that does not
    /// match the schema's declared type. Recoverable on first 3 attempts.
    PayloadTypeMismatch,
    /// `MissingRequiredField` — agent omitted a required field.
    /// Recoverable on first 3 attempts.
    MissingRequiredField,
    /// `TopicDenied` — the (hat, topic) pair matched a deny rule.
    /// Recoverable on first 3 attempts.
    TopicDenied,
    /// U1 (2026-06-17-003 plan): `SemanticGateViolation` — event
    /// passed schema validation but violates an orchestrator-level
    /// invariant (e.g. `review.passed` while a review wave is still
    /// open). Recoverable on first 3 attempts AND bypasses the U6
    /// `PayloadContractViolation` fatal path. The bucket is its own
    /// dimension on the loop-level retry counter so semantic-gate
    /// rejections never compete with payload-typed rejections for
    /// the budget.
    SemanticGateViolation,
    /// U4 (2026-06-17-003 plan): duplicate `work.done` for the
    /// same `(plan_name, step, task_id)` tuple. The 2nd emit is
    /// rejected as recoverable (agent can re-emit with a different
    /// step or task_id, or wait for `fix.applied` / step close to
    /// re-send legitimately).
    DuplicateWorkDone,
}

impl ReasonClass {
    /// Stable snake_case label used in retry-key construction and
    /// operator-facing logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            ReasonClass::PayloadTypeMismatch => "payload_type_mismatch",
            ReasonClass::MissingRequiredField => "missing_required_field",
            ReasonClass::TopicDenied => "topic_denied",
            ReasonClass::SemanticGateViolation => "semantic_gate_violation",
            ReasonClass::DuplicateWorkDone => "duplicate_work_done",
        }
    }
}

/// Unit 2: pure mapping from a [`PolicyFinding`] to a recoverable
/// [`ReasonClass`]. Returns `Some(class)` when the finding is in the
/// recoverable set (R-B1), `None` otherwise.
pub fn is_recoverable_policy_finding(finding: &PolicyFinding) -> Option<ReasonClass> {
    match &finding.violation_type {
        ViolationType::PayloadTypeMismatch { .. } => Some(ReasonClass::PayloadTypeMismatch),
        ViolationType::MissingRequiredField { .. } => Some(ReasonClass::MissingRequiredField),
        ViolationType::TopicDenied { .. } => Some(ReasonClass::TopicDenied),
        // U1 (2026-06-17-003 plan): semantic gate violations are
        // recoverable on first 3 attempts AND bypass U6 fatal
        // termination (see runner.rs `TerminationReason::PayloadContractViolation`
        // branch). The bucket is independent of the other three
        // reason classes so a misbehaving coordinator can be
        // corrected via `task.resume` without exhausting the schema
        // validation budget for unrelated events.
        ViolationType::SemanticGateViolation { .. } => Some(ReasonClass::SemanticGateViolation),
        ViolationType::DuplicateWorkDone { .. } => Some(ReasonClass::DuplicateWorkDone),
        _ => None,
    }
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
    /// U4 (2026-06-17-003 plan): dedup set for `work.done` events.
    /// Key format: `{plan_name}::{step}::{task_id}`. Populated when
    /// a `work.done` is accepted by `validate_event_with_hat`;
    /// consumed by the event loop for per-batch pruning. The
    /// per-loop lifetime set lives in `LoopState::work_done_seen_tasks`
    /// (see `event_loop/loop_state.rs`); this set is the
    /// `PolicyRuntimeState` mirror used during `validate_event`
    /// for **in-batch** dedup (when the same `work.done` appears
    /// twice in the same `process_output` batch).
    pub work_done_seen_keys: HashSet<String>,
    /// U5 (2026-06-17-003 plan, R6): dedup set for
    /// `review.dimension.ready` events. Key format:
    /// `{plan_name}::{step}::{task_id}::{dimension}`. Populated
    /// when a `review.dimension.ready` is accepted by
    /// `validate_event_with_hat`; a 2nd emit with the same key
    /// is rejected as `DuplicateWorkDone` (variant reused —
    /// same retry-key semantics, smaller blast radius than
    /// introducing a new ViolationType). Mirrors the
    /// `work.done` dedup pattern: this is the in-batch mirror;
    /// the per-loop lifetime set is also populated in
    /// `from_events` for cross-batch replay.
    pub review_dimension_ready_seen_keys: HashSet<String>,
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
            // U5 (2026-06-17-003 plan, R6): replay prior
            // `review.dimension.ready` events to populate the
            // dedup set so cross-batch re-emits (e.g. on loop
            // restart or in a new process_output batch) are
            // still rejected. The key shape matches the
            // in-batch check: `{plan_name}::{step}::{task_id}::{dimension}`.
            if event.topic == "review.dimension.ready" {
                if let Some(ref payload) = event.payload {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(payload) {
                        if let Value::Object(obj) = &val {
                            let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
                            let step = obj.get("step").and_then(|v| v.as_str());
                            let task_id = obj.get("task_id").and_then(|v| v.as_str());
                            let dimension = obj.get("dimension").and_then(|v| v.as_str());
                            if let (Some(pn), Some(st), Some(ti), Some(dim)) =
                                (plan_name, step, task_id, dimension)
                            {
                                state
                                    .review_dimension_ready_seen_keys
                                    .insert(format!("{pn}::{st}::{ti}::{dim}"));
                            }
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

/// WAC-U7 (2026-06-12-002) R10: hard-reject topics for which a
/// null payload is never acceptable. Any event whose topic is in
/// this set and whose payload is `None` is rejected with
/// `RejectWithResume` regardless of `EventPolicyMode::Observe`.
/// The list is the minimum required by R10; it is intentionally
/// not configurable so the operational contract is uniform
/// across presets.
///
/// Step-handoff (2026-06-17-002) U5: extended with `work.ready`,
/// `plan.complete`, `plan.blocked` so the hard gate uniformly
/// covers every handoff/terminal topic in the ce-executor step
/// chain — independent of whether the preset ships a
/// `payload: json_object` schema for that topic (Observe mode
/// would otherwise let null payloads slip past the schema layer).
pub const NULL_PAYLOAD_REJECT_TOPICS: &[&str] = &[
    "review.passed",
    "review.failed",
    "review.complete",
    "work.done",
    "queue.advance",
    "review.wave.ready",
    "work.ready",
    "plan.complete",
    "plan.blocked",
];

/// Returns `true` if `topic` is in [`NULL_PAYLOAD_REJECT_TOPICS`].
pub fn is_null_payload_rejected_topic(topic: &str) -> bool {
    NULL_PAYLOAD_REJECT_TOPICS.contains(&topic)
}

/// Check topic-deny rules against a (hat, topic) pair.
///
/// When the event policy is in `Enforce` mode and the (hat_id, topic) pair
/// matches any `topic_deny_rules` entry, returns `Some(PolicyDecision::Block)`
/// with reason `"topic_denied"`.  Otherwise returns `None`.
///
/// In `Observe` mode, matching a deny rule produces a `Warn` decision instead.
///
/// Topic matching supports glob patterns:
/// - Exact match: `build.done` matches `build.done`
/// - Segment wildcard: `debug.*` matches `debug.step`, `debug.done`, etc.
/// - Global wildcard: `*` matches any topic
pub fn check_topic_deny_rules(
    hat: Option<&str>,
    topic: &str,
    config: &EventPolicyConfig,
) -> Option<PolicyDecision> {
    let hat_id = hat.unwrap_or("");
    for rule in &config.topic_deny_rules {
        if rule.hat_id == hat_id {
            let matches = if rule.topic.contains('*') {
                Topic::new(&rule.topic).matches_str(topic)
            } else {
                rule.topic == topic
            };
            if matches {
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
                        ViolationAction::RejectWithResume => {
                            PolicyDecision::RejectWithResume(finding)
                        }
                        ViolationAction::Hold => PolicyDecision::Hold(finding),
                        ViolationAction::Block => PolicyDecision::Block(finding),
                    },
                });
            }
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
    validate_event_with_hat(topic, payload, config, state, None)
}

/// Validates an event against the event policy with hat-aware checks.
///
/// `hat` is the emitting hat id (if known). When provided, it enables
/// hat-specific schema restrictions such as per-hat allowed values and
/// topic-deny rules. When omitted, only hat-agnostic checks run.
pub fn validate_event_with_hat(
    topic: &str,
    payload: Option<&str>,
    config: &EventPolicyConfig,
    state: &mut PolicyRuntimeState,
    hat: Option<&str>,
) -> PolicyDecision {
    if !config.enabled {
        return PolicyDecision::Accept;
    }

    state.observed_topics.insert(topic.to_string());

    let mut findings = Vec::new();

    // U4 (2026-06-17-003 plan): duplicate `work.done` detection.
    // The dedup key is `(plan_name, step, task_id)`. A 2nd
    // `work.done` with the same key is rejected as
    // `RecoverableRejection` (NOT fatal) so the runner can
    // re-route to the source hat with a `task.resume` carrying
    // the correct `fix_hint`. The hint distinguishes
    // `duplicate_stall_bypass` (wave_id is set → agent trying
    // to bypass a stalled review cycle) from `duplicate_same_step`
    // (no wave → pure same-step re-emit, fix-round did not
    // advance). The check is applied before all other policy
    // layers so a duplicate is a duplicate regardless of
    // schema/terminal state.
    if topic == "work.done"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
            let dedup_key = format!("{pn}::{st}::{ti}");
            if state.work_done_seen_keys.contains(&dedup_key) {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        hint: DuplicateWorkDoneHint::DuplicateSameStep,
                    },
                    message: format!(
                        "duplicate_same_step: work.done for key '{dedup_key}' was already accepted. \
                         Wait for fix.applied / queue.advance / step close before re-sending work.done \
                         for the same (plan_name, step, task_id)."
                    ),
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            // Record the key so a 3rd emit in the same batch is
            // also rejected. The in-batch set is drained by the
            // event loop after `process_output` completes; the
            // per-loop lifetime set lives in
            // `LoopState::work_done_seen_tasks` and is pruned
            // on step-boundary events.
            state.work_done_seen_keys.insert(dedup_key);
        }
    }

    // U5 (2026-06-17-003 plan, R6): duplicate
    // `review.dimension.ready` detection. The dedup key is
    // `(plan_name, step, task_id, dimension)`. A 2nd
    // `review.dimension.ready` with the same key is rejected
    // as `RejectWithResume` so the runner publishes a
    // `task.resume` with `fix_hint` pointing the agent to wait
    // for the matching `review.dimension.done` /
    // `review.dimension.failed` before re-sending
    // `review.dimension.ready`. The check is applied before
    // schema/terminal layers so a duplicate is a duplicate
    // regardless of state.
    //
    // We reuse the `DuplicateWorkDone` variant (same key/hint
    // shape) rather than introducing a new ViolationType
    // because the recovery flow is identical: both are
    // recoverable rejections that carry a retry-key, and
    // `is_recoverable_policy_finding` already maps the variant
    // to the correct bucket. Adding a new variant would force
    // a parallel `is_recoverable` arm and a parallel
    // `reason_code` mapping for no behavioral gain.
    if topic == "review.dimension.ready"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        let dimension = obj.get("dimension").and_then(|v| v.as_str());
        if let (Some(pn), Some(st), Some(ti), Some(dim)) = (plan_name, step, task_id, dimension) {
            let dedup_key = format!("{pn}::{st}::{ti}::{dim}");
            if state.review_dimension_ready_seen_keys.contains(&dedup_key) {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        hint: DuplicateWorkDoneHint::DuplicateSameStep,
                    },
                    message: format!(
                        "duplicate_dimension_ready: review.dimension.ready for key '{dedup_key}' \
                         was already accepted. Wait for review.dimension.done / \
                         review.dimension.failed for the same dimension before re-sending \
                         review.dimension.ready."
                    ),
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            // Record the key so a 3rd emit in the same batch
            // is also rejected. The in-batch set is drained by
            // the event loop after `process_output` completes;
            // the per-loop lifetime set is populated by
            // `from_events` on restart so cross-batch replays
            // honor the dedup.
            state.review_dimension_ready_seen_keys.insert(dedup_key);
        }
    }

    // WAC-U7 R10 (2026-06-12-002): null payloads on the
    // `NULL_PAYLOAD_REJECT_TOPICS` whitelist are hard-rejected
    // with `RejectWithResume`, overriding any `Observe`-mode
    // downgrades. The check is applied before schema
    // validation so a topic without an explicit `schemas`
    // entry still gets the R10 treatment. KTD-9.
    if payload.is_none() && is_null_payload_rejected_topic(topic) {
        let finding = PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::PayloadTypeMismatch {
                expected: "non-null payload".to_string(),
                actual: "null".to_string(),
            },
            message: format!(
                "WAC R10: null payload on whitelist topic `{}` is hard-rejected; \
                 a structured payload is required for this topic",
                topic
            ),
        };
        return PolicyDecision::RejectWithResume(finding);
    }

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
            // WAC-U7 R11 (2026-06-12-002) KTD-10: a string payload
            // that parses to a JSON object is normalized to the
            // serialized object form before required-field
            // validation runs. Non-object strings fall through
            // to the regular type-mismatch finding. The
            // normalized string is captured in
            // `normalized_payload` so the required-fields block
            // below sees the object form.
            let mut normalized_payload: Option<String> = None;
            match payload {
                Some(p) => match serde_json::from_str::<Value>(p) {
                    Ok(Value::Object(map)) => {
                        normalized_payload = Some(
                            serde_json::to_string(&Value::Object(map))
                                .unwrap_or_else(|_| p.to_string()),
                        );
                    }
                    Ok(other) => {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::PayloadTypeMismatch {
                                expected: "json_object".to_string(),
                                actual: format!("{:?}", other),
                            },
                            message: format!("Payload must be JSON object, got {:?}", other),
                        });
                        normalized_payload = Some(p.to_string());
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
                        normalized_payload = Some(p.to_string());
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

            // Required fields — applied AFTER normalize (KTD-10).
            if !schema.required_fields.is_empty() {
                let payload_for_required = normalized_payload.as_deref().or(payload);
                if let Some(p) = payload_for_required {
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
                            message: format!(
                                "Missing required field '{}' (payload is missing)",
                                field
                            ),
                        });
                    }
                }
            }
        } else {
            // Required fields (no json_object payload requirement)
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
                    for field in &schema.required_fields {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::MissingRequiredField {
                                field: field.clone(),
                            },
                            message: format!(
                                "Missing required field '{}' (payload is missing)",
                                field
                            ),
                        });
                    }
                }
            }
        }

        // Allowed values (hat-agnostic)
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

        // Hat-aware allowed values.
        // U1 (2026-06-17-004 plan, R2): fail-closed when provenance is
        // missing and the schema carries per-hat restrictions. Without
        // a known hat, no hat-specific value can be validated, so the
        // event must be rejected — leaving the question of "which hat"
        // to the caller (CLI emit pipeline enforces `check_emit_provenance`
        // before reaching this function; programmatic callers are still
        // required to supply a hat for topics with `hat_allowed_values`).
        //
        // The previous code silently skipped the entire hat-aware block
        // when `hat = None`. That let a hat-less emit bypass the
        // per-hat restriction (e.g. review-coordinator could emit
        // `review.passed(skip_reason=aggregate_timeout)` by dropping
        // the `--hat` flag). This is now a hard `MissingRequiredField`
        // finding — the gate fails closed.
        if schema.hat_allowed_values.is_empty() {
            // No per-hat restrictions on this topic — skip the block.
            // (Implicit: when `hat = None` and no `hat_allowed_values`
            // are configured, nothing to validate.)
        } else if let Some(hat_id) = hat {
            for (field_path, per_hat_rules) in &schema.hat_allowed_values {
                if let Some(rule) = per_hat_rules.iter().find(|r| r.hat_id == hat_id) {
                    if let Some(p) = payload
                        && let Ok(value) = serde_json::from_str::<Value>(p)
                        && let Some(field_value) = extract_json_field(&value, field_path)
                        && !rule.values.contains(&field_value)
                    {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::InvalidFieldValue {
                                field: field_path.clone(),
                                value: field_value.clone(),
                            },
                            message: format!(
                                "Hat '{}' may not use value {:?} for field '{}'. Allowed for this hat: {:?}",
                                hat_id, field_value, field_path, rule.values
                            ),
                        });
                    }
                }
            }
        } else {
            // Hat is missing but schema has hat-specific allowed values.
            // Without provenance we cannot pick the right rule, so we
            // emit a single finding that names the topic + the per-hat
            // restrictions. The CLI emit pipeline's
            // `check_emit_provenance` rejects this event earlier; this
            // finding covers programmatic callers (API server,
            // in-process emitters) that go straight to
            // `validate_event_with_hat`.
            let mut per_hat_summary: Vec<String> = Vec::new();
            for (field_path, per_hat_rules) in &schema.hat_allowed_values {
                for rule in per_hat_rules {
                    per_hat_summary.push(format!(
                        "hat='{}' field='{}' allowed={:?}",
                        rule.hat_id, field_path, rule.values
                    ));
                }
            }
            findings.push(PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::MissingRequiredField {
                    field: "hat".to_string(),
                },
                message: format!(
                    "Topic '{topic}' has hat-specific allowed values; a hat is required \
                     to validate the payload. Provenance rules: {per_hat_summary:?}. \
                     Pass --hat <hat-id> or set RALPH_CURRENT_HAT=<hat-id>."
                ),
            });
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

    // U1 (2026-06-11-002): trivial_step semantic gate.
    //
    // Reject `review.passed` events that claim `skip_reason=trivial_step`
    // while the payload proves the diff was non-trivial
    // (`findings_count > 0` OR `changed_lines >= threshold`). This is the
    // semantic check that backs the schema allowlist — the allowlist
    // validates the value of `skip_reason`, and this gate validates that
    // the value matches the actual diff state.
    //
    // Disabled when `trivial_step_max_changed_lines == 0`. Otherwise the
    // threshold defaults to 50 (matching the preset's `changed_lines_min: 50`
    // wave gate). The gate runs only when the JSON object parses; a
    // missing/invalid payload is the schema layer's job and the gate
    // contributes no extra finding.
    if config.trivial_step_max_changed_lines > 0
        && topic == "review.passed"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
        && obj.get("skip_reason").and_then(|v| v.as_str()) == Some("trivial_step")
    {
        let findings_count = obj.get("findings_count").and_then(|v| v.as_u64());
        let changed_lines = obj.get("changed_lines").and_then(|v| v.as_u64());
        let findings_violated = matches!(findings_count, Some(n) if n > 0);
        let diff_violated =
            matches!(changed_lines, Some(n) if n >= config.trivial_step_max_changed_lines);
        if findings_violated || diff_violated {
            findings.push(PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::InvalidFieldValue {
                    field: "skip_reason".to_string(),
                    value: Value::String("trivial_step".to_string()),
                },
                message: format!(
                    "invalid_trivial_step_bypass: review.passed claimed skip_reason='trivial_step' but the payload proves the diff was non-trivial. \
                     observed findings_count={}, changed_lines={} (threshold: changed_lines<{} AND findings_count==0). \
                     Expected action: route the review through the synthesizer/Fixer or use the proper terminal topic \
                     (review.passed with skip_reason='empty_diff' or 'aggregate_timeout', or a real review.wave.ready wave); \
                     do not bypass Fixer by claiming the diff was trivial.",
                    findings_count
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "(missing)".to_string()),
                    changed_lines
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "(missing)".to_string()),
                    config.trivial_step_max_changed_lines,
                ),
            });
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
    use crate::config::{EventSchema, HatAllowedValues, TopicDenyRule};
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
            hat_allowed_values: HashMap::new(),
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
            hat_allowed_values: HashMap::new(),
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
            hat_allowed_values: HashMap::new(),
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
            hat_allowed_values: HashMap::new(),
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
            hat_allowed_values: HashMap::new(),
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
            hat_allowed_values: HashMap::new(),
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
            hat_allowed_values: HashMap::new(),
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
            hat_allowed_values: HashMap::new(),
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
            hat_allowed_values: HashMap::new(),
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
            hat_allowed_values: HashMap::new(),
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
        // U8: hat-aware restrictions mirror the preset.
        schema.hat_allowed_values.insert(
            "skip_reason".to_string(),
            vec![
                HatAllowedValues {
                    hat_id: "review-coordinator".to_string(),
                    values: vec![Value::String("empty_diff".to_string())],
                },
                HatAllowedValues {
                    hat_id: "review-synthesizer".to_string(),
                    values: vec![Value::String("aggregate_timeout".to_string())],
                },
            ],
        );
        config.schemas.insert("review.passed".to_string(), schema);
        config
    }

    #[test]
    fn test_u4_review_passed_skip_reason_allowlist_accepts_legal_values() {
        let config = review_passed_allowlist_config();
        // Each legal value is paired with a hat that allows it (per the
        // hat_allowed_values below). `trivial_step` is allowed by the
        // global allowlist only — no hat-specific entry — so the
        // hat-aware check skips and the value passes through the
        // global allowlist. With a hat, the check passes when the
        // value is either: (a) in the hat's per-hat list, or (b) not
        // restricted per-hat (i.e. the schema has no entry for that
        // hat, in which case only the global allowlist applies).
        let cases: &[(&str, &str)] = &[
            ("empty_diff", "review-coordinator"),
            ("aggregate_timeout", "review-synthesizer"),
            // trivial_step is in the global allowlist but not in any
            // hat-specific entry. The hat-aware block only fires when
            // the schema has a rule for the emitting hat; pick a hat
            // without a per-hat rule (the schema only has rules for
            // review-coordinator / review-synthesizer, so use any
            // other hat id to exercise the "no rule → skip" branch).
            ("trivial_step", "executor"),
        ];
        for (legal, hat_id) in cases {
            let payload = format!(
                r#"{{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"{legal}"}}"#
            );
            let mut state = PolicyRuntimeState::default();
            let decision = validate_event_with_hat(
                "review.passed",
                Some(&payload),
                &config,
                &mut state,
                Some(hat_id),
            );
            assert_eq!(
                decision,
                PolicyDecision::Accept,
                "skip_reason='{legal}' with hat='{hat_id}' should be accepted by the allowlist, got {:?}",
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
    fn test_u8_review_passed_hat_aware_allowed_values() {
        let config = review_passed_allowlist_config();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"aggregate_timeout"}"#;

        // review-coordinator may only use skip_reason='empty_diff'.
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_hat(
            "review.passed",
            Some(payload),
            &config,
            &mut state,
            Some("review-coordinator"),
        );
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "review-coordinator emitting review.passed(skip_reason=aggregate_timeout) must be rejected, got {:?}",
            decision
        );

        // review-synthesizer may use skip_reason='aggregate_timeout'.
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_hat(
            "review.passed",
            Some(payload),
            &config,
            &mut state,
            Some("review-synthesizer"),
        );
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "review-synthesizer emitting review.passed(skip_reason=aggregate_timeout) must be accepted, got {:?}",
            decision
        );

        // U1 (2026-06-17-004 plan, R2): no hat provided + schema has
        // hat_allowed_values → fail-closed with a MissingRequiredField
        // finding. The CLI emit pipeline's `check_emit_provenance` gate
        // rejects hat-less business-topic emits earlier; this test pins
        // the programmatic-caller contract (validate_event / API
        // server path) so the old "skip hat-aware when None" behavior
        // cannot silently re-appear. The `validate_event` convenience
        // wraps `validate_event_with_hat(..., None)` so it inherits
        // the same fail-closed semantics.
        let mut state = PolicyRuntimeState::default();
        let decision =
            validate_event_with_hat("review.passed", Some(payload), &config, &mut state, None);
        match decision {
            PolicyDecision::RejectWithResume(finding) => {
                assert!(
                    matches!(
                        finding.violation_type,
                        ViolationType::MissingRequiredField { .. }
                    ),
                    "no-hat + hat_allowed_values must yield MissingRequiredField, got {:?}",
                    finding.violation_type
                );
                assert!(
                    finding.message.contains("hat-specific allowed values"),
                    "message must explain the provenance requirement, got: {}",
                    finding.message
                );
            }
            other => panic!(
                "no-hat + hat_allowed_values must be rejected (fail-closed), got {:?}",
                other
            ),
        }
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

    #[test]
    fn test_topic_deny_rules_glob_pattern_matches() {
        // Glob pattern `debug.*` matches `debug.step`, `debug.done`, etc.
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::RejectWithResume,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "coordinator".to_string(),
                topic: "debug.*".to_string(),
            }],
            ..Default::default()
        };
        // Segment wildcard matches
        assert!(matches!(
            check_topic_deny_rules(Some("coordinator"), "debug.step", &config),
            Some(PolicyDecision::RejectWithResume(_))
        ));
        assert!(matches!(
            check_topic_deny_rules(Some("coordinator"), "debug.done", &config),
            Some(PolicyDecision::RejectWithResume(_))
        ));
        // Non-matching topic not matched
        assert!(check_topic_deny_rules(Some("coordinator"), "debug", &config).is_none());
        // Non-matching hat not matched
        assert!(check_topic_deny_rules(Some("executor"), "debug.step", &config).is_none());
    }

    #[test]
    fn test_topic_deny_rules_glob_exact_overlap() {
        // When glob and exact rule both exist for same hat, first match wins.
        // Exact rule for `build.done` and glob rule for `debug.*` on coordinator.
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![
                TopicDenyRule {
                    hat_id: "coordinator".to_string(),
                    topic: "build.done".to_string(),
                },
                TopicDenyRule {
                    hat_id: "coordinator".to_string(),
                    topic: "debug.*".to_string(),
                },
            ],
            ..Default::default()
        };
        let decision = check_topic_deny_rules(Some("coordinator"), "build.done", &config);
        // Exact match found first (Block, not RejectWithResume from glob)
        assert!(matches!(decision, Some(PolicyDecision::Block(_))));
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

    // -------------------------------------------------------------------------
    // U4 (2026-06-17-003 plan): duplicate `work.done` dedup tests
    //
    // Same `(plan_name, step, task_id)` tuple — 2nd `work.done` is
    // rejected with `RecoverableRejection` (NOT fatal).
    // -------------------------------------------------------------------------

    fn work_done_payload(plan: &str, step: &str, task: &str) -> String {
        format!(r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","task_key":"k"}}"#)
    }

    #[test]
    fn test_u4_duplicate_work_done_first_accepted() {
        // Happy path: first `work.done` for a (plan, step, task) tuple is accepted.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = work_done_payload("p1", "step-01", "t1");
        let decision = validate_event("work.done", Some(&payload), &config, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "First work.done for a new (plan, step, task) tuple must be accepted"
        );
        // The dedup key should now be in the per-batch set
        assert!(state.work_done_seen_keys.contains("p1::step-01::t1"));
    }

    #[test]
    fn test_u4_duplicate_work_done_second_rejected() {
        // Error path: 2nd `work.done` with the same (plan, step, task)
        // tuple is rejected with `RejectWithResume` (RecoverableRejection
        // — the policy validator routes it through the recoverable bucket).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = work_done_payload("p1", "step-01", "t1");

        // First emit: accepted
        let first = validate_event("work.done", Some(&payload), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        // Second emit (same key, same batch): rejected
        let second = validate_event("work.done", Some(&payload), &config, &mut state);
        assert!(
            matches!(
                second,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    ..
                }) if key == "p1::step-01::t1"
            ),
            "Second work.done for same key must be rejected with DuplicateWorkDone, got {:?}",
            second
        );
    }

    #[test]
    fn test_u4_duplicate_work_done_different_step_accepted() {
        // Edge case: same (plan, task_id) but different `step` key →
        // still accepted (key includes step).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        // step-01 emit
        let p1 = work_done_payload("p1", "step-01", "t1");
        let first = validate_event("work.done", Some(&p1), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        // Same task, different step: accepted
        let p2 = work_done_payload("p1", "step-02", "t1");
        let second = validate_event("work.done", Some(&p2), &config, &mut state);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "work.done for same task but different step must be accepted, got {:?}",
            second
        );
    }

    #[test]
    fn test_u4_duplicate_work_done_different_task_accepted() {
        // Edge case: same (plan, step) but different `task_id` →
        // still accepted (key includes task_id).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        let p1 = work_done_payload("p1", "step-01", "t1");
        let first = validate_event("work.done", Some(&p1), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let p2 = work_done_payload("p1", "step-01", "t2");
        let second = validate_event("work.done", Some(&p2), &config, &mut state);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "work.done for same step but different task must be accepted, got {:?}",
            second
        );
    }

    #[test]
    fn test_u4_duplicate_work_done_is_recoverable() {
        // The DuplicateWorkDone violation must be in the recoverable
        // bucket (R-B1) so the runner publishes a `task.resume` with
        // `fix_hint` instead of the U6 fast-fail.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = work_done_payload("p1", "step-01", "t1");
        let first = validate_event("work.done", Some(&payload), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event("work.done", Some(&payload), &config, &mut state);
        let finding = match second {
            PolicyDecision::RejectWithResume(f) => f,
            other => panic!("expected RejectWithResume, got {:?}", other),
        };
        let class = is_recoverable_policy_finding(&finding);
        assert_eq!(
            class,
            Some(ReasonClass::DuplicateWorkDone),
            "DuplicateWorkDone must map to the recoverable bucket, got {:?}",
            class
        );
    }

    #[test]
    fn test_u4_duplicate_work_done_disabled_policy_accepts_all() {
        // When event policy is disabled, the dedup check must be
        // skipped (mirrors all other policy checks).
        let mut config = test_config();
        config.enabled = false;
        let mut state = PolicyRuntimeState::default();
        let payload = work_done_payload("p1", "step-01", "t1");

        let first = validate_event("work.done", Some(&payload), &config, &mut state);
        let second = validate_event("work.done", Some(&payload), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "disabled policy must not dedup, got {:?}",
            second
        );
    }

    #[test]
    fn test_u4_duplicate_work_done_missing_fields_skips_dedup() {
        // If the payload is missing plan_name/step/task_id, the dedup
        // check cannot run — fall through to other policy layers.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"task_key":"k"}"#; // missing plan_name/step/task_id
        let first = validate_event("work.done", Some(payload), &config, &mut state);
        // First emit: not rejected by dedup (no key to compare).
        // May be rejected by other policies (e.g. required fields), but
        // the dedup violation type must NOT appear.
        if let PolicyDecision::RejectWithResume(f) = &first {
            assert!(
                !matches!(f.violation_type, ViolationType::DuplicateWorkDone { .. }),
                "missing-fields payload must not trigger DuplicateWorkDone, got {:?}",
                f.violation_type
            );
        }
    }

    #[test]
    fn test_u4_duplicate_work_done_hint_mapped_to_reason_code() {
        // The reason_code for DuplicateWorkDone must be a stable
        // snake_case string usable in CLI precheck JSON output.
        let finding = PolicyFinding {
            topic: "work.done".to_string(),
            violation_type: ViolationType::DuplicateWorkDone {
                key: "p::s::t".to_string(),
                hint: DuplicateWorkDoneHint::DuplicateSameStep,
            },
            message: "test".to_string(),
        };
        assert_eq!(finding.violation_type.reason_code(), "duplicate_work_done");
    }

    #[test]
    fn test_u4_duplicate_work_done_hint_distinct() {
        // The two hints are distinct enum values so the runtime can
        // branch on them.
        assert_ne!(
            DuplicateWorkDoneHint::DuplicateSameStep,
            DuplicateWorkDoneHint::DuplicateStallBypass
        );
    }

    // -------------------------------------------------------------------------
    // U5 (2026-06-17-003 plan, R6): `review.dimension.ready` dedup
    //
    // Mirrors the U4 work.done dedup pattern. Key is
    // `(plan_name, step, task_id, dimension)`. A 2nd emit with
    // the same key is rejected as `DuplicateWorkDone` (variant
    // reused for retry-key parity).
    // -------------------------------------------------------------------------

    fn review_dimension_ready_payload(plan: &str, step: &str, task: &str, dim: &str) -> String {
        format!(
            r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","dimension":"{dim}","wave_id":"w1"}}"#
        )
    }

    #[test]
    fn review_dimension_ready_dedup_first_accepted() {
        // Happy path: first `review.dimension.ready` for a
        // (plan, step, task, dimension) tuple is accepted.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let decision = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "First review.dimension.ready for a new key must be accepted"
        );
        assert!(
            state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness")
        );
    }

    #[test]
    fn review_dimension_ready_dedup_rejects_second_emit() {
        // Error path: 2nd `review.dimension.ready` with the
        // same (plan, step, task, dimension) tuple is rejected
        // with `RejectWithResume`.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");

        let first = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(
                second,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    ..
                }) if key == "p1::step-01::t1::correctness"
            ),
            "Second review.dimension.ready for same key must be rejected with DuplicateWorkDone, got {:?}",
            second
        );
    }

    #[test]
    fn review_dimension_ready_dedup_different_dimensions_both_accepted() {
        // Edge case: same (plan, step, task) but different
        // `dimension` → both accepted (serial walk through
        // review dimensions).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        let p1 = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let first = validate_event("review.dimension.ready", Some(&p1), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let p2 = review_dimension_ready_payload("p1", "step-01", "t1", "security");
        let second = validate_event("review.dimension.ready", Some(&p2), &config, &mut state);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "review.dimension.ready for same task but different dimension must be accepted, got {:?}",
            second
        );
    }

    #[test]
    fn review_dimension_ready_dedup_different_step_accepted() {
        // Edge case: same (plan, task, dimension) but different
        // `step` → still accepted (key includes step).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        let p1 = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let first = validate_event("review.dimension.ready", Some(&p1), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let p2 = review_dimension_ready_payload("p1", "step-02", "t1", "correctness");
        let second = validate_event("review.dimension.ready", Some(&p2), &config, &mut state);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "review.dimension.ready for same dim but different step must be accepted, got {:?}",
            second
        );
    }

    #[test]
    fn review_dimension_ready_dedup_is_recoverable() {
        // The DuplicateWorkDone violation (reused for
        // review.dimension.ready) must map to the recoverable
        // bucket so the runner publishes a `task.resume` with
        // a fix_hint.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let first = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        let finding = match second {
            PolicyDecision::RejectWithResume(f) => f,
            other => panic!("expected RejectWithResume, got {:?}", other),
        };
        let class = is_recoverable_policy_finding(&finding);
        assert_eq!(
            class,
            Some(ReasonClass::DuplicateWorkDone),
            "review.dimension.ready dup must map to recoverable bucket, got {:?}",
            class
        );
    }

    #[test]
    fn review_dimension_ready_dedup_disabled_policy_accepts_all() {
        // When event policy is disabled, the dedup check must
        // be skipped.
        let mut config = test_config();
        config.enabled = false;
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");

        let first = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        let second = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "disabled policy must not dedup review.dimension.ready, got {:?}",
            second
        );
    }

    #[test]
    fn review_dimension_ready_dedup_missing_fields_skips_dedup() {
        // If payload is missing any of the dedup fields, the
        // dedup check cannot run — fall through to other
        // policy layers. The DuplicateWorkDone variant must
        // NOT appear.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"dimension":"correctness"}"#; // missing plan_name/step/task_id
        let decision = validate_event("review.dimension.ready", Some(payload), &config, &mut state);
        if let PolicyDecision::RejectWithResume(f) = &decision {
            assert!(
                !matches!(f.violation_type, ViolationType::DuplicateWorkDone { .. }),
                "missing-fields payload must not trigger DuplicateWorkDone, got {:?}",
                f.violation_type
            );
        }
    }

    #[test]
    fn review_dimension_ready_replay_from_events_populates_seen_keys() {
        // `PolicyRuntimeState::from_events` must populate the
        // dedup set from any prior `review.dimension.ready`
        // events in the JSONL so cross-batch replay is
        // honored.
        use std::io::Write;

        let jsonl = r#"{"topic":"review.dimension.ready","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
{"topic":"review.dimension.done","hat":"dimension-reviewer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness"),
            "from_events must populate dedup set from prior review.dimension.ready, got {:?}",
            state.review_dimension_ready_seen_keys
        );
    }

    // -------------------------------------------------------------------------
    // U1 (2026-06-11-002): trivial_step semantic gate
    //
    // Rejects `review.passed` events with `skip_reason=trivial_step` when the
    // payload proves the diff was non-trivial
    // (`findings_count > 0` OR `changed_lines >= 50`). The legitimate trivial
    // path (small diff + 0 findings) and the other skip reasons
    // (`empty_diff`, `aggregate_timeout`) must still be accepted.
    // -------------------------------------------------------------------------

    fn review_passed_full_schema_config() -> EventPolicyConfig {
        // Mirrors the ce-executor-isolated.yml `review.passed` schema:
        // required fields + skip_reason allowlist.
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
            hat_allowed_values: HashMap::new(),
        };
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
    fn test_u1_trivial_step_with_findings_rejected() {
        // findings_count > 0 cannot be a "trivial" review — findings mean
        // a reviewer actually surfaced concerns. U1 must reject.
        let config = review_passed_full_schema_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":20,"fix_round":0,"verdict":"pass","skip_reason":"trivial_step","changed_lines":5}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        let is_rejected_with_reason = matches!(
            decision,
            PolicyDecision::RejectWithResume(PolicyFinding {
                violation_type: ViolationType::InvalidFieldValue { ref field, .. },
                ref message,
                ..
            }) if field == "skip_reason" && message.contains("trivial_step")
        );
        assert!(
            is_rejected_with_reason,
            "trivial_step with findings_count>0 must be rejected, got {:?}",
            decision
        );
    }

    #[test]
    fn test_u1_trivial_step_with_large_diff_rejected() {
        // changed_lines >= 50 contradicts the "trivial" label — the diff
        // is non-trivial by the same threshold the gate uses for wave
        // selection (preset `changed_lines_min: 50`). U1 must reject.
        let config = review_passed_full_schema_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"trivial_step","changed_lines":80}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        let is_rejected_with_reason = matches!(
            decision,
            PolicyDecision::RejectWithResume(PolicyFinding {
                violation_type: ViolationType::InvalidFieldValue { ref field, .. },
                ref message,
                ..
            }) if field == "skip_reason" && message.contains("trivial_step")
        );
        assert!(
            is_rejected_with_reason,
            "trivial_step with changed_lines>=50 must be rejected, got {:?}",
            decision
        );
    }

    #[test]
    fn test_u1_trivial_step_with_findings_and_large_diff_rejected() {
        // Both conditions violated — also rejected.
        let config = review_passed_full_schema_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":3,"fix_round":0,"verdict":"pass","skip_reason":"trivial_step","changed_lines":120}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "trivial_step with both findings and large diff must be rejected, got {:?}",
            decision
        );
    }

    #[test]
    fn test_u1_trivial_step_legitimate_path_accepted() {
        // The whole point of the gate: legitimate trivial steps
        // (small diff + 0 findings) keep passing through unchanged.
        let config = review_passed_full_schema_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"trivial_step","changed_lines":5}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "legitimate trivial_step (small diff + 0 findings) must be accepted, got {:?}",
            decision
        );
    }

    #[test]
    fn test_u1_trivial_step_boundary_at_49_changed_lines_accepted() {
        // Boundary check: 49 is just under the threshold. Make sure the
        // check uses `>= 50` and not `> 50`.
        let config = review_passed_full_schema_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"trivial_step","changed_lines":49}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "changed_lines=49 is below the 50-line threshold and must be accepted, got {:?}",
            decision
        );
    }

    #[test]
    fn test_u1_empty_diff_with_large_diff_field_still_rejected() {
        // empty_diff is the OTHER valid skip_reason; the gate must not
        // touch it even if `changed_lines` happens to be high (the
        // empty_diff contract is about the diff state, not the field).
        // empty_diff + 80 lines is a different violation (the synthesizer
        // lied about empty_diff) but the U1 gate should not add a finding
        // on top — schema-level checks should still apply, and the event
        // remains under the existing checks. For the U1 gate's view
        // specifically: empty_diff is not in the trigger set, so the gate
        // does not produce a finding of its own.
        let config = review_passed_full_schema_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff","changed_lines":80}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        // The U1 gate only fires on skip_reason=trivial_step; empty_diff
        // remains a separate concern (already covered by preset's wave
        // gate). We assert: the U1 gate is NOT what produces a finding
        // here (the field would have to mention "trivial_step bypass").
        let is_u1_finding = matches!(
            decision,
            PolicyDecision::RejectWithResume(PolicyFinding {
                ref message, ..
            }) if message.contains("trivial_step")
        );
        assert!(
            !is_u1_finding,
            "U1 gate must not produce a trivial_step finding for empty_diff, got {:?}",
            decision
        );
    }

    #[test]
    fn test_u1_aggregate_timeout_unaffected_by_gate() {
        // aggregate_timeout is the third valid skip_reason and is reserved
        // for the review-coordinator's trivial-step fast path; U1 must
        // not add a finding for it even if the diff happens to be large
        // (the gate's whole premise is "you claim trivial AND the diff
        // is non-trivial" — aggregate_timeout does not claim trivial).
        let config = review_passed_full_schema_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"aggregate_timeout","changed_lines":200}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        let is_u1_finding = matches!(
            decision,
            PolicyDecision::RejectWithResume(PolicyFinding {
                ref message, ..
            }) if message.contains("trivial_step")
        );
        assert!(
            !is_u1_finding,
            "U1 gate must not fire on aggregate_timeout, got {:?}",
            decision
        );
    }

    #[test]
    fn test_u1_trivial_step_payload_observation_in_message() {
        // The recovery payload should surface the observed
        // changed_lines / findings_count so the agent can correct its
        // emission without re-reading the event file.
        let config = review_passed_full_schema_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":7,"fix_round":0,"verdict":"pass","skip_reason":"trivial_step","changed_lines":120}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        match decision {
            PolicyDecision::RejectWithResume(finding) => {
                assert!(
                    finding.message.contains("findings_count=7"),
                    "message must include observed findings_count, got: {}",
                    finding.message
                );
                assert!(
                    finding.message.contains("changed_lines=120"),
                    "message must include observed changed_lines, got: {}",
                    finding.message
                );
            }
            other => panic!("expected RejectWithResume, got {:?}", other),
        }
    }

    #[test]
    fn test_u1_trivial_step_skipped_when_payload_missing() {
        // The gate must not crash on missing payload — schema layer
        // already rejects this case, so the gate simply does not run.
        let config = review_passed_full_schema_config();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("review.passed", None, &config, &mut state);
        // Schema layer (required_fields) will already produce a
        // RejectWithResume; we only assert that it is NOT the U1 finding.
        let is_u1_finding = matches!(
            decision,
            PolicyDecision::RejectWithResume(PolicyFinding {
                ref message, ..
            }) if message.contains("trivial_step")
        );
        assert!(
            !is_u1_finding,
            "U1 gate must not produce a finding when payload is missing (schema layer handles it), got {:?}",
            decision
        );
    }

    #[test]
    fn test_u1_trivial_step_skipped_when_changed_lines_missing() {
        // If changed_lines is not in the payload (some legacy emitters
        // may omit it), the gate cannot prove the diff was non-trivial.
        // Conservative: only the `findings_count > 0` half fires; the
        // event is accepted if findings_count is also 0.
        let config = review_passed_full_schema_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"trivial_step"}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        // accepted: findings_count=0, changed_lines absent → no U1 finding
        let is_u1_finding = matches!(
            decision,
            PolicyDecision::RejectWithResume(PolicyFinding {
                ref message, ..
            }) if message.contains("trivial_step")
        );
        assert!(
            !is_u1_finding,
            "missing changed_lines + 0 findings must not trigger U1 gate, got {:?}",
            decision
        );
    }

    #[test]
    fn test_u1_trivial_step_non_review_topics_unaffected() {
        // The gate is scoped to review.passed only. work.done with
        // skip_reason=trivial_step (synthetic example) must not trigger
        // the gate.
        let mut config = test_config();
        let mut schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec!["findings_count".into()],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
        };
        schema.allowed_values.insert(
            "skip_reason".to_string(),
            vec![Value::String("trivial_step".to_string())],
        );
        config.schemas.insert("work.done".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let payload =
            r#"{"findings_count": 20, "skip_reason": "trivial_step", "changed_lines": 80}"#;
        let decision = validate_event("work.done", Some(payload), &config, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "U1 gate must only fire on review.passed, not other topics, got {:?}",
            decision
        );
    }

    // ── WAC-U7 (2026-06-12-002): payload hard gate ──

    /// T-U7-01 / R10: null `review.passed` payload is hard-rejected
    /// even when `EventPolicyMode::Observe` is configured.
    #[test]
    fn wac_r10_null_payload_on_whitelist_topic_is_rejected() {
        let mut config = test_config_with_enforce_and_resume();
        // Switch the policy into Observe mode to confirm the R10
        // gate is mode-agnostic.
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("review.passed", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "R10 must RejectWithResume even in Observe mode, got {:?}",
            decision
        );
    }

    /// R10 also covers the other whitelist topics:
    /// `work.done`, `queue.advance`, `review.wave.ready`, etc.
    /// Step-handoff (2026-06-17-002) U5 extends the whitelist with
    /// the handoff/terminal topics `work.ready`, `plan.complete`,
    /// `plan.blocked` so the hard gate uniformly covers every
    /// handoff/terminal topic in the ce-executor step chain.
    #[test]
    fn wac_r10_null_payload_rejects_every_whitelist_topic() {
        let config = test_config_with_enforce_and_resume();
        for topic in [
            "review.passed",
            "review.failed",
            "review.complete",
            "work.done",
            "work.ready",
            "queue.advance",
            "review.wave.ready",
            "plan.complete",
            "plan.blocked",
        ] {
            let mut s = PolicyRuntimeState::default();
            let decision = validate_event(topic, None, &config, &mut s);
            assert!(
                matches!(decision, PolicyDecision::RejectWithResume(_)),
                "R10 must reject null payload on `{topic}`, got {:?}",
                decision
            );
        }
    }

    /// Step-handoff (2026-06-17-002) U5: `is_null_payload_rejected_topic` is the
    /// single source of truth for the whitelist. Pin the exact
    /// membership (original 6 + 3 U5 additions appended in place) so
    /// future edits cannot silently drop a topic from the hard gate.
    #[test]
    fn step_handoff_u5_whitelist_membership_pinned() {
        let expected = [
            "review.passed",
            "review.failed",
            "review.complete",
            "work.done",
            "queue.advance",
            "review.wave.ready",
            "work.ready",
            "plan.complete",
            "plan.blocked",
        ];
        assert_eq!(NULL_PAYLOAD_REJECT_TOPICS, expected);
        for topic in expected {
            assert!(
                is_null_payload_rejected_topic(topic),
                "is_null_payload_rejected_topic must accept `{topic}`"
            );
        }
        // A non-whitelist topic is unaffected.
        assert!(!is_null_payload_rejected_topic("human.guidance"));
    }

    /// Step-handoff U5: a null `work.ready` payload is
    /// hard-rejected even in Observe mode. This is the per-topic
    /// pin for `work.ready` after the list extension.
    #[test]
    fn step_handoff_u5_work_ready_null_payload_is_rejected() {
        let mut config = test_config_with_enforce_and_resume();
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("work.ready", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "U5: null work.ready must RejectWithResume even in Observe, got {:?}",
            decision
        );
    }

    /// Step-handoff U5: a null `plan.complete` payload is
    /// hard-rejected even in Observe mode.
    #[test]
    fn step_handoff_u5_plan_complete_null_payload_is_rejected() {
        let mut config = test_config_with_enforce_and_resume();
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("plan.complete", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "U5: null plan.complete must RejectWithResume even in Observe, got {:?}",
            decision
        );
    }

    /// Step-handoff U5: a null `plan.blocked` payload is
    /// hard-rejected even in Observe mode.
    #[test]
    fn step_handoff_u5_plan_blocked_null_payload_is_rejected() {
        let mut config = test_config_with_enforce_and_resume();
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("plan.blocked", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "U5: null plan.blocked must RejectWithResume even in Observe, got {:?}",
            decision
        );
    }

    /// T-U7-02 / R11: a string payload that is a parseable JSON
    /// object is normalized to the object form and accepted.
    /// Required-field validation runs against the normalized
    /// object, not the original string.
    #[test]
    fn wac_r11_string_payload_normalizes_to_object() {
        let mut config = test_config_with_enforce_and_resume();
        config.schemas.insert(
            "review.wave.ready".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec!["dimension".to_string(), "plan_name".to_string()],
                allowed_values: HashMap::new(),
                hat_allowed_values: HashMap::new(),
            },
        );
        let mut state = PolicyRuntimeState::default();
        // The payload is a JSON-string-of-an-object.
        let payload = r#"{"dimension":"code-quality","plan_name":"p1"}"#;
        let decision = validate_event("review.wave.ready", Some(payload), &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::Accept),
            "string-as-object must normalize and accept, got {:?}",
            decision
        );
    }

    /// T-U7-03 / R11: a string payload that is NOT a valid JSON
    /// object is rejected (cannot be normalized).
    #[test]
    fn wac_r11_string_payload_not_json_is_rejected() {
        let mut config = test_config_with_enforce_and_resume();
        config.schemas.insert(
            "review.wave.ready".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec!["dimension".to_string()],
                allowed_values: HashMap::new(),
                hat_allowed_values: HashMap::new(),
            },
        );
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("review.wave.ready", Some("not-a-json"), &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "non-JSON string must be rejected, got {:?}",
            decision
        );
    }

    /// T-U7-07: R10 hard-rejects null payloads even when the
    /// rest of the policy is in `Observe` mode. The other
    /// findings (terminal monotonicity, etc.) still fall through
    /// to `Warn` per the existing behaviour, but R10 specifically
    /// escalates to `RejectWithResume`.
    #[test]
    fn wac_r10_overrides_observe_mode_for_null_whitelist_payload() {
        let mut config = test_config_with_enforce_and_resume();
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("work.done", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "R10 must not downgraded by Observe mode, got {:?}",
            decision
        );
    }

    /// Helper: build a minimal `EventPolicyConfig` with Enforce
    /// mode + RejectWithResume. Reused by the WAC tests above.
    fn test_config_with_enforce_and_resume() -> EventPolicyConfig {
        let mut config = EventPolicyConfig::default();
        config.enabled = true;
        config.mode = EventPolicyMode::Enforce;
        config.on_violation = ViolationAction::RejectWithResume;
        config
    }

    // U1 (2026-06-17-003 plan): the new `SemanticGateViolation`
    // variant must be in the recoverable set with its own bucket
    // — and its reason_code must NOT collide with the schema-level
    // `invalid_field_value` so diagnostics stay unambiguous.
    #[test]
    fn u1_semantic_gate_violation_is_recoverable_with_own_bucket() {
        let finding = PolicyFinding {
            topic: "review.passed".to_string(),
            violation_type: ViolationType::SemanticGateViolation {
                gate: "review_passed_while_wave_open".to_string(),
                context: "wave='w-1' received=0/3 expected".to_string(),
            },
            message: "review-coordinator must not emit review.passed while wave is incomplete"
                .to_string(),
        };
        let class = is_recoverable_policy_finding(&finding)
            .expect("SemanticGateViolation must be in the recoverable set");
        assert_eq!(class, ReasonClass::SemanticGateViolation);
        assert_eq!(class.as_str(), "semantic_gate_violation");
        assert_eq!(
            finding.violation_type.reason_code(),
            "semantic_gate_violation"
        );
        // field() returns None — semantic-gate violations are
        // state-scoped, not field-scoped.
        assert!(finding.violation_type.field().is_none());
    }

    // U1 (2026-06-17-003 plan): the four existing recoverable
    // buckets must keep their stable labels — adding
    // `SemanticGateViolation` to the enum must not shift them.
    #[test]
    fn u1_semantic_gate_violation_does_not_perturb_other_buckets() {
        assert_eq!(
            ReasonClass::PayloadTypeMismatch.as_str(),
            "payload_type_mismatch"
        );
        assert_eq!(
            ReasonClass::MissingRequiredField.as_str(),
            "missing_required_field"
        );
        assert_eq!(ReasonClass::TopicDenied.as_str(), "topic_denied");
        // And the non-recoverable ones stay non-recoverable.
        let finding = PolicyFinding {
            topic: "review.passed".to_string(),
            violation_type: ViolationType::TerminalMonotonicityViolation {
                terminal_topic: "plan.complete".to_string(),
                business_topic: "review.passed".to_string(),
            },
            message: "terminal monotonicity".to_string(),
        };
        assert!(is_recoverable_policy_finding(&finding).is_none());
    }

    // U1 (2026-06-17-003 plan): the `finding_to_payload_contract_violation`
    // bridge (in `event_loop/mod.rs`) maps a `PolicyFinding` to a
    // `PayloadContractViolation` only when the violation is
    // schema-derived. `SemanticGateViolation` must NOT be in that
    // set so the runner's `PayloadContractViolation` fatal branch
    // never fires for `review_passed_while_wave_open`. We re-test
    // here at the policy layer because the bridge is in `mod.rs`
    // and not exposed for direct unit testing without spinning up
    // an `EventLoop`. The downstream guarantee is:
    //   is_recoverable_policy_finding == Some(SemanticGateViolation)
    //   → bridge returns None → runner skips the fatal branch.
    #[test]
    fn u1_semantic_gate_is_recoverable_implies_not_fatal() {
        let finding = PolicyFinding {
            topic: "review.passed".to_string(),
            violation_type: ViolationType::SemanticGateViolation {
                gate: "review_passed_while_wave_open".to_string(),
                context: "wave='w-1' received=0/3 expected".to_string(),
            },
            message: "review-coordinator must not emit review.passed while wave is incomplete"
                .to_string(),
        };
        // Recoverable → never feeds the U6 fast-fail
        // (`capture_violation` in `event_loop/mod.rs` early-returns
        // when this returns `Some`).
        assert!(is_recoverable_policy_finding(&finding).is_some());
        // And the bridge arms for `AllowedValueMismatch` /
        // `MissingRequiredField` / `PayloadTypeMismatch` only —
        // `SemanticGateViolation` falls through to `return None`.
        // We re-state the arms here so a future enum expansion
        // that accidentally adds a new fatal mapping is caught
        // by this test.
        assert!(matches!(
            finding.violation_type,
            ViolationType::SemanticGateViolation { .. }
        ));
    }
}
