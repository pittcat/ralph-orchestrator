//! Event policy configuration for typed payload validation and lifecycle enforcement.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use super::default_true;
use super::loop_config::EventSchema;

/// Opt-in recovery guidance attached to a precheck or payload-consistency
/// rule (plan 2026-08-17-1841 R1/D2/D5). When the rule rejects an emit, the
/// preset-supplied `common` strings are rendered into the target hat's
/// correction prompt unconditionally, and the `by_check` strings for the
/// specific failed check (1-based precheck checklist index, or the
/// consistency rule's stable `id`) are rendered alongside.
///
/// Omitting the block is a no-op (matches the legacy "no custom
/// guidance" baseline). Both fields use `serde(default)` so old
/// presets keep parsing without modification.
///
/// Safety: the runtime renderer still applies `safe_display` to each
/// item at prompt-build time. The preset lint in
/// `crate::preset_lint::recovery_guidance` is the first line of defence
/// and rejects empty items, unsafe characters, oversized items, and
/// out-of-range keys.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct RecoveryGuidance {
    /// Items shown in the target hat's correction prompt regardless of
    /// which check failed. Render order matches insertion order.
    #[serde(default)]
    pub common: Vec<String>,
    /// Per-check items. For a precheck rule the key is the 1-based
    /// checklist index as a decimal string ("1", "2", ...). For a
    /// payload-consistency rule the key MUST equal the rule's stable
    /// `id`. Out-of-range / unknown keys are surfaced by the lint.
    #[serde(default)]
    pub by_check: BTreeMap<String, Vec<String>>,
}

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

/// Opt-in payload consistency checks; default-off preserves existing event behavior.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PayloadConsistencyConfig {
    /// Whether payload consistency checks are enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Consistency rules evaluated for matching topics.
    #[serde(default)]
    pub rules: Vec<PayloadConsistencyRule>,
}

/// A payload consistency rule declared by a preset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PayloadConsistencyRule {
    /// Stable rule identifier.
    pub id: String,
    /// Event topic to which the rule applies.
    pub topic: String,
    /// Permissive predicate placeholder; a later unit tightens this shape.
    pub when: serde_json::Value,
    /// Human-readable validation failure message.
    pub message: String,
    /// Optional recovery guidance attached to this rule. When the
    /// evaluator hits this rule, `common` is shown unconditionally and
    /// `by_check["<this rule id>"]` is shown as the check-specific
    /// item (plan 2026-08-17-1841 R1/D2/D3). The lint rejects any
    /// `by_check` key that does not equal this rule's `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_guidance: Option<RecoveryGuidance>,
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
    /// Opt-in payload consistency checks. Missing configuration remains disabled.
    #[serde(default)]
    pub payload_consistency: PayloadConsistencyConfig,
    /// When true, `work.done` events are validated to have their `plan_name`
    /// payload field equal to the `current_plan_name` extracted from the most
    /// recent `work.ready` event.  Default false (backward compatible).
    #[serde(default)]
    pub plan_name_equality_required: bool,
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
            payload_consistency: PayloadConsistencyConfig::default(),
            plan_name_equality_required: false,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_consistency_defaults_to_disabled() {
        let config = EventPolicyConfig::default();

        assert!(!config.payload_consistency.enabled);
        assert!(config.payload_consistency.rules.is_empty());
    }

    #[test]
    fn parses_payload_consistency_block() {
        let config: EventPolicyConfig = serde_yaml::from_str(
            "
enabled: true
mode: enforce
payload_consistency:
  enabled: true
  rules:
    - id: r1
      topic: fix.done
      when:
        all:
          - field: x
            eq: y
      message: payload fields must agree
",
        )
        .expect("payload_consistency block should parse");

        assert!(config.payload_consistency.enabled);
        assert_eq!(config.payload_consistency.rules.len(), 1);
        let rule = &config.payload_consistency.rules[0];
        assert_eq!(rule.id, "r1");
        assert_eq!(rule.topic, "fix.done");
        assert_eq!(rule.message, "payload fields must agree");
    }

    #[test]
    fn missing_payload_consistency_block_defaults_to_disabled() {
        let config: EventPolicyConfig = serde_yaml::from_str(
            "
enabled: true
mode: observe
",
        )
        .expect("legacy event policy should parse");

        assert!(!config.payload_consistency.enabled);
        assert!(config.payload_consistency.rules.is_empty());
    }

    #[test]
    fn parses_rule_when_as_json_value() {
        let config: EventPolicyConfig = serde_yaml::from_str(
            "
enabled: true
mode: enforce
payload_consistency:
  enabled: true
  rules:
    - id: r1
      topic: fix.done
      when:
        all:
          - field: x
            eq: y
      message: payload fields must agree
",
        )
        .expect("minimal payload consistency rule should parse");

        assert_eq!(
            config.payload_consistency.rules[0].when,
            serde_json::json!({"all": [{"field": "x", "eq": "y"}]})
        );
    }

    #[test]
    fn existing_event_policy_yaml_parses_with_unchanged_fields() {
        let config: EventPolicyConfig = serde_yaml::from_str(
            "
enabled: true
mode: enforce
on_violation: reject_with_resume
terminal_topics:
  - plan.complete
  - LOOP_COMPLETE
business_topics:
  - work.done
require_policy_check_for_cli_emit: true
allow_unsafe_cli_emit: false
require_emit_provenance: true
",
        )
        .expect("existing event policy YAML should remain valid");

        assert!(config.enabled);
        assert_eq!(config.mode, EventPolicyMode::Enforce);
        assert_eq!(config.on_violation, ViolationAction::RejectWithResume);
        assert_eq!(config.terminal_topics, ["plan.complete", "LOOP_COMPLETE"]);
        assert_eq!(config.business_topics, ["work.done"]);
        assert!(config.require_policy_check_for_cli_emit);
        assert!(!config.allow_unsafe_cli_emit);
        assert!(config.require_emit_provenance);
        assert!(!config.payload_consistency.enabled);
    }

    #[test]
    fn rejects_non_boolean_payload_consistency_enabled() {
        let result = serde_yaml::from_str::<EventPolicyConfig>(
            "
enabled: true
mode: enforce
payload_consistency:
  enabled: \"yes\"
  rules: []
",
        );

        assert!(result.is_err());
    }

    #[test]
    fn rejects_payload_consistency_rule_missing_required_field() {
        let result = serde_yaml::from_str::<EventPolicyConfig>(
            "
enabled: true
mode: enforce
payload_consistency:
  enabled: true
  rules:
    - id: r1
      topic: fix.done
      when: {}
",
        );

        assert!(result.is_err());
    }
}
