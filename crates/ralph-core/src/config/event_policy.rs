//! Event policy configuration for typed payload validation and lifecycle enforcement.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::default_true;
use super::loop_config::EventSchema;

/// Threshold above which a `review.passed(skip_reason=trivial_step)` event
/// is considered an attempt to bypass the wave review. Default 50, matching
/// the preset's `changed_lines_min: 50` wave gate.
pub const DEFAULT_TRIVIAL_STEP_CHANGED_LINES: u64 = 50;

/// A rule that denies a specific hat from publishing a specific topic.
///
/// Matching semantics: exact `hat_id` + exact `topic` (no glob).  When the
/// event policy is in `Enforce` mode, a matching rule produces a
/// `PolicyDecision::Block` with reason `"topic_denied"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopicDenyRule {
    /// Hat ID to match (exact).
    pub hat_id: String,
    /// Topic to deny (exact match).
    pub topic: String,
}

/// Opt-in event policy for typed payload validation and lifecycle enforcement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventPolicyConfig {
    pub enabled: bool,
    pub mode: EventPolicyMode,
    #[serde(default)]
    pub on_violation: ViolationAction,
    #[serde(default)]
    pub schemas: HashMap<String, EventSchema>,
    /// Path to an external schema file (relative to the preset/config file directory).
    /// Schema definitions in this file are merged with inline `schemas`.
    /// Inline schemas take priority over file schemas when both define the same topic.
    #[serde(default)]
    pub schema_file: Option<String>,
    #[serde(default)]
    pub terminal_topics: Vec<String>,
    #[serde(default)]
    pub business_topics: Vec<String>,
    /// When true, CLI emit commands must pass policy checks even without `--policy-check`.
    #[serde(default)]
    pub require_policy_check_for_cli_emit: bool,
    /// When true, allow unsafe CLI emit bypasses. Defaults to true for backward compatibility.
    #[serde(default = "default_true")]
    pub allow_unsafe_cli_emit: bool,
    /// When true, CLI emit must include provenance (`hat` / `triggered`).
    #[serde(default)]
    pub require_emit_provenance: bool,
    /// Behavior after a terminal event has been observed.
    #[serde(default)]
    pub completion_after_terminal: CompletionAfterTerminalConfig,
    /// Topic-deny rules: for each matching (hat_id, topic) pair, the event is
    /// rejected with reason "topic_denied".  Exact match only (no glob).
    #[serde(default)]
    pub topic_deny_rules: Vec<TopicDenyRule>,
    /// When true, `work.done` events are validated to have their `plan_name`
    /// payload field equal to the `current_plan_name` extracted from the most
    /// recent `work.ready` event.  Default false (backward compatible).
    #[serde(default)]
    pub plan_name_equality_required: bool,
    /// U1 (2026-06-11-002): semantic gate for `review.passed`. When the
    /// `skip_reason` is `trivial_step` AND the payload shows either
    /// `findings_count > 0` OR `changed_lines >= trivial_step_max_changed_lines`,
    /// the event is rejected with reason `invalid_trivial_step_bypass` and
    /// the source hat receives a `task.resume` pointing it at the
    /// synthesizer/Fixer or the proper terminal topic. Defaults to the
    /// preset's wave threshold (50); setting this to `0` disables the gate.
    #[serde(default = "default_trivial_step_max_changed_lines")]
    pub trivial_step_max_changed_lines: u64,
}

fn default_trivial_step_max_changed_lines() -> u64 {
    DEFAULT_TRIVIAL_STEP_CHANGED_LINES
}

impl Default for EventPolicyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: EventPolicyMode::default(),
            on_violation: ViolationAction::default(),
            schemas: HashMap::new(),
            schema_file: None,
            terminal_topics: Vec::new(),
            business_topics: Vec::new(),
            require_policy_check_for_cli_emit: false,
            allow_unsafe_cli_emit: true,
            require_emit_provenance: false,
            completion_after_terminal: CompletionAfterTerminalConfig::default(),
            topic_deny_rules: Vec::new(),
            plan_name_equality_required: false,
            trivial_step_max_changed_lines: DEFAULT_TRIVIAL_STEP_CHANGED_LINES,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EventPolicyMode {
    /// Observe mode: violations are logged but events still pass through.
    #[default]
    Observe,
    /// Enforce mode: violations may reject or hold events based on on_violation.
    Enforce,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ViolationAction {
    /// Log warning only.
    #[default]
    Warn,
    /// Reject event and publish task.resume with reason.
    RejectWithResume,
    /// Hold the loop (write hold artifact).
    Hold,
    /// Block the event silently (drop it).
    Block,
}

/// Action to take for events that arrive after a terminal/completion event.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CompletionAfterTerminalAction {
    /// Log a warning but allow the event.
    #[default]
    Warn,
    /// Reject the event and publish a recovery event.
    Reject,
    /// Silently ignore business events after terminal.
    Ignore,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct CompletionAfterTerminalConfig {
    /// Action for duplicate terminal events after completion.
    #[serde(default)]
    pub duplicate_terminal: CompletionAfterTerminalAction,
    /// Action for business events after completion.
    #[serde(default)]
    pub business_after_completion: CompletionAfterTerminalAction,
    /// Whether to write diagnostic events for blocked/ignored events.
    #[serde(default)]
    pub write_diagnostic_event: bool,
}
