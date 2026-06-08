//! Hat configuration types.

use std::collections::HashMap;

use ralph_proto::Topic;
use serde::{Deserialize, Serialize};

use super::core::ScratchpadConfig;
use super::core::deserialize_optional_scratchpad_config;
use super::event_filter::EventFilterConfig;
use super::loop_config::Phase;

/// Metadata for an event topic.
///
/// Defines what an event means, enabling auto-derived instructions for hats.
/// When a hat triggers on or publishes an event, this metadata is used to
/// generate appropriate behavior instructions.
///
/// Example:
/// ```yaml
/// events:
///   deploy.start:
///     description: "Deployment has been requested"
///     on_trigger: "Prepare artifacts, validate config, check dependencies"
///     on_publish: "Signal that deployment should begin"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventMetadata {
    /// Brief description of what this event represents.
    #[serde(default)]
    pub description: String,

    /// Instructions for a hat that triggers on (receives) this event.
    /// Describes what the hat should do when it receives this event.
    #[serde(default)]
    pub on_trigger: String,

    /// Instructions for a hat that publishes (emits) this event.
    /// Describes when/how the hat should emit this event.
    #[serde(default)]
    pub on_publish: String,
}

/// Backend configuration for a hat.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HatBackend {
    // Order matters for serde untagged - most specific first
    /// Kiro agent with custom agent name and optional args.
    KiroAgent {
        #[serde(rename = "type")]
        backend_type: String,
        agent: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Named backend with args (has `type` but no `agent`).
    NamedWithArgs {
        #[serde(rename = "type")]
        backend_type: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Simple named backend (string form).
    Named(String),
    /// Custom backend with command and args.
    Custom {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl HatBackend {
    /// Converts to CLI backend string for execution.
    pub fn to_cli_backend(&self) -> String {
        match self {
            HatBackend::Named(name) => name.clone(),
            HatBackend::NamedWithArgs { backend_type, .. } => backend_type.clone(),
            HatBackend::KiroAgent { backend_type, .. } => backend_type.clone(),
            HatBackend::Custom { .. } => "custom".to_string(),
        }
    }
}

/// Activation-level publish obligation (2026-06-07 plan U4).
///
/// Pins a single trigger topic to the set of topics the hat MUST emit
/// at least one of when that trigger fires.  Used by `hard_gate` to
/// distinguish:
///
///   - "agent did not run / produced no events at all" (hard-gate
///     fires when the obligation is not satisfied).
///   - "agent claimed to emit but the event did not make it to the
///     trusted reader" (late-event path, not hard-gate).
///   - "agent's emitted event was rejected by origin / policy /
///     execution contract" (rejection recovery, not hard-gate).
///   - "agent chose a different but legitimate topic from the
///     obligation set" (no hard-gate — obligation satisfied).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivationObligation {
    /// Trigger topic that activates this obligation.  When the hat
    /// is activated by this topic, it must emit one of
    /// `must_emit_any_of`.
    pub on_trigger: String,
    /// Allowed result topics.  Emitting any one of them satisfies
    /// the obligation.  Empty is treated as "no obligation" (i.e.
    /// the trigger is informational, not enforceable).
    #[serde(default)]
    pub must_emit_any_of: Vec<String>,
}

/// Configuration for a single hat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HatConfig {
    /// Human-readable name for the hat.
    pub name: String,

    /// Short description of the hat's purpose (required).
    /// Used in the HATS table to help Ralph understand when to delegate to this hat.
    pub description: Option<String>,

    /// Events that trigger this hat to be worn.
    /// Per spec: "Hats define triggers — which events cause Ralph to wear this hat."
    #[serde(default, alias = "subscribes_to")]
    pub triggers: Vec<String>,

    /// Topics this hat publishes.
    #[serde(default)]
    pub publishes: Vec<String>,

    /// Instructions prepended to prompts.
    #[serde(default)]
    pub instructions: String,

    /// Additional instruction fragments appended to `instructions`.
    ///
    /// Use with YAML anchors to share common instruction blocks across hats:
    /// ```yaml
    /// _confidence_protocol: &confidence_protocol |
    ///   ### Confidence-Based Decision Protocol
    ///   ...
    ///
    /// hats:
    ///   architect:
    ///     instructions: |
    ///       ## ARCHITECT MODE
    ///       ...
    ///     extra_instructions:
    ///       - *confidence_protocol
    /// ```
    #[serde(default)]
    pub extra_instructions: Vec<String>,

    /// Backend to use for this hat (inherits from cli.backend if not specified).
    #[serde(default)]
    pub backend: Option<HatBackend>,

    /// Custom args to append to the backend CLI when this hat is active.
    ///
    /// Accepts both `backend_args:` and shorthand `args:`.
    #[serde(default, alias = "args")]
    pub backend_args: Option<Vec<String>>,

    /// Default event to publish if hat forgets to write an event.
    #[serde(default)]
    pub default_publishes: Option<String>,

    /// Maximum number of times this hat may be activated in a single loop run.
    ///
    /// When the limit is exceeded, the orchestrator publishes `<hat_id>.exhausted`
    /// instead of activating the hat again.
    pub max_activations: Option<u32>,

    /// Per-hat scratchpad override. If None, inherits from core.scratchpad.
    /// Accepts both a plain string shorthand and a structured object.
    #[serde(default, deserialize_with = "deserialize_optional_scratchpad_config")]
    pub scratchpad: Option<ScratchpadConfig>,

    /// Tools the hat is not allowed to use.
    ///
    /// Injected as a TOOL RESTRICTIONS section in the prompt (soft enforcement).
    /// After each iteration, a file-modification audit checks compliance when
    /// `Edit` or `Write` are disallowed (hard enforcement via scope_violation event).
    #[serde(default)]
    pub disallowed_tools: Vec<String>,

    /// Execution timeout in seconds for this hat.
    ///
    /// For wave workers, this controls how long each parallel worker can run.
    /// Defaults to the adapter-level timeout (typically 300s) if not set.
    #[serde(default)]
    pub timeout: Option<u32>,

    /// Maximum concurrent wave instances for this hat.
    ///
    /// When > 1, the loop runner spawns multiple backend instances in parallel
    /// for wave events targeting this hat. Default is 1 (sequential execution).
    #[serde(default = "default_concurrency")]
    pub concurrency: u32,

    /// Activation-level publish obligations (2026-06-07 plan U4).
    ///
    /// Each entry pins a specific activation trigger to the set of topics
    /// the hat MUST emit at least one of when that trigger fires.  When
    /// obligations are configured for a hat, hard_gate checks them
    /// directly instead of falling back to the blanket
    /// `!publishes.is_empty() && default_publishes.is_none()` rule.
    ///
    /// Empty (the default) means "use the legacy blanket rule" — a
    /// hat with `publishes` and no `default_publishes` will still be
    /// hard-gated, but only for non-conditional / non-aggregate
    /// terminal events.  This keeps backwards compatibility for
    /// presets that do not opt in.
    ///
    /// Example:
    /// ```yaml
    /// review-coordinator:
    ///   triggers: ["work.done", "fix.applied"]
    ///   publishes: ["review.wave.ready", "review.passed"]
    ///   obligations:
    ///     - on_trigger: "work.done"
    ///       must_emit_any_of: ["review.wave.ready", "review.passed"]
    /// ```
    #[serde(default)]
    pub obligations: Vec<ActivationObligation>,

    /// Aggregation configuration for this hat.
    ///
    /// When set, this hat acts as an aggregator — it buffers wave results and
    /// activates only when all correlated results have arrived (or timeout).
    /// Cannot be set on a hat with `concurrency > 1`.
    #[serde(default)]
    pub aggregate: Option<AggregateConfig>,

    /// Event filter configuration for this hat.
    ///
    /// When enabled, only events matching the filter rules are passed to this hat.
    #[serde(default)]
    pub event_filter: Option<EventFilterConfig>,

    /// Phase-aware triggers: map from phase name to list of trigger topics.
    ///
    /// When present, the hat subscribes to the triggers of the current phase
    /// instead of the global `triggers` field. Useful for hats that should
    /// behave differently in warmup vs production (e.g., harness hat).
    #[serde(default)]
    pub phase_triggers: Option<HashMap<String, Vec<String>>>,

    /// Fields to ignore when extracting payload field references from instructions.
    ///
    /// Used by the static payload contract validator to exclude false positives.
    /// Does not affect runtime event policy enforcement.
    #[serde(default)]
    pub ignore_payload_fields: Vec<String>,
}

fn default_concurrency() -> u32 {
    1
}

/// Configuration for wave result aggregation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateConfig {
    /// Aggregation mode.
    pub mode: AggregateMode,

    /// Timeout in seconds for waiting on all wave results.
    /// After this timeout, the aggregator activates with whatever results are available.
    pub timeout: u32,
}

/// Aggregation mode for wave results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateMode {
    /// Wait for all wave instances to complete before activating the aggregator.
    WaitForAll,
}

impl Default for HatConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: None,
            triggers: Vec::new(),
            publishes: Vec::new(),
            instructions: String::new(),
            extra_instructions: Vec::new(),
            backend: None,
            backend_args: None,
            default_publishes: None,
            max_activations: None,
            scratchpad: None,
            disallowed_tools: Vec::new(),
            timeout: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            phase_triggers: None,
            ignore_payload_fields: Vec::new(),
            obligations: Vec::new(),
        }
    }
}

impl HatConfig {
    /// Converts trigger strings to Topic objects.
    pub fn trigger_topics(&self) -> Vec<Topic> {
        self.triggers.iter().map(|s| Topic::new(s)).collect()
    }

    /// Converts publish strings to Topic objects.
    pub fn publish_topics(&self) -> Vec<Topic> {
        self.publishes.iter().map(|s| Topic::new(s)).collect()
    }

    /// Returns triggers for a specific phase, or fall back to global triggers.
    pub fn triggers_for_phase(&self, phase: &Phase) -> Vec<Topic> {
        if let Some(ref phase_triggers) = self.phase_triggers {
            let phase_name = match phase {
                Phase::Warmup => "warmup",
                Phase::Production => "production",
            };
            if let Some(triggers) = phase_triggers.get(phase_name) {
                return triggers.iter().map(|s| Topic::new(s)).collect();
            }
        }
        // Fall back to global triggers
        self.trigger_topics()
    }

    /// Returns all trigger topics for registration purposes.
    /// When phase_triggers is set, returns the union of all phase triggers.
    pub fn all_trigger_topics(&self) -> Vec<Topic> {
        if let Some(ref phase_triggers) = self.phase_triggers {
            let mut topics: Vec<Topic> = phase_triggers
                .values()
                .flat_map(|triggers| triggers.iter().map(|s| Topic::new(s)))
                .collect();
            topics.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            topics.dedup();
            topics
        } else {
            self.trigger_topics()
        }
    }

    /// Look up the obligation that applies to a given trigger topic.
    ///
    /// Returns `None` when the hat has no obligation for the trigger
    /// (which is the case for legacy presets that do not opt into
    /// activation-level obligations).  Callers that get `None` should
    /// fall back to the blanket `publishes + default_publishes` rule.
    pub fn obligation_for_trigger(&self, trigger_topic: &str) -> Option<&ActivationObligation> {
        self.obligations
            .iter()
            .find(|o| o.on_trigger == trigger_topic)
    }

    /// Returns `true` if the hat has any activation-level obligation
    /// for the given trigger topic.  Used by `hard_gate` to decide
    /// whether to take the activation-level path (preferred) or the
    /// legacy blanket rule (fallback).
    pub fn has_obligation_for(&self, trigger_topic: &str) -> bool {
        self.obligation_for_trigger(trigger_topic).is_some()
    }
}

/// Returns `true` when the candidate topic set satisfies the
/// activation obligation for a given trigger.  A candidate set
/// satisfies an obligation when at least one candidate topic is
/// listed in the obligation's `must_emit_any_of` set.  An empty
/// `must_emit_any_of` is treated as "no obligation" and always
/// satisfied (legacy behaviour).
///
/// Caller-supplied helper so `hard_gate` does not have to know about
/// the `ActivationObligation` shape.  Lives at module scope so the
/// `hat.rs` test module can exercise it without touching the public
/// `HatConfig` API.
pub fn obligation_satisfied(
    obligation: Option<&ActivationObligation>,
    candidate_topics: &[String],
) -> bool {
    match obligation {
        None => true, // No obligation → any outcome is fine.
        Some(o) if o.must_emit_any_of.is_empty() => true,
        Some(o) => candidate_topics
            .iter()
            .any(|t| o.must_emit_any_of.iter().any(|m| m == t)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hat_with_obligations(obligations: Vec<ActivationObligation>) -> HatConfig {
        HatConfig {
            name: "test".into(),
            description: None,
            triggers: vec!["work.done".into()],
            publishes: vec!["review.passed".into(), "review.wave.ready".into()],
            instructions: String::new(),
            extra_instructions: Vec::new(),
            backend: None,
            backend_args: None,
            default_publishes: None,
            max_activations: None,
            scratchpad: None,
            disallowed_tools: Vec::new(),
            timeout: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            phase_triggers: None,
            ignore_payload_fields: Vec::new(),
            obligations,
        }
    }

    #[test]
    fn obligation_for_trigger_returns_matching_obligation() {
        let hat = hat_with_obligations(vec![ActivationObligation {
            on_trigger: "work.done".into(),
            must_emit_any_of: vec!["review.wave.ready".into(), "review.passed".into()],
        }]);
        let o = hat.obligation_for_trigger("work.done").expect("obligation");
        assert_eq!(o.on_trigger, "work.done");
        assert_eq!(o.must_emit_any_of.len(), 2);
    }

    #[test]
    fn obligation_for_trigger_returns_none_when_unconfigured() {
        let hat = hat_with_obligations(vec![]);
        assert!(hat.obligation_for_trigger("work.done").is_none());
        assert!(!hat.has_obligation_for("work.done"));
    }

    #[test]
    fn obligation_satisfied_with_no_obligation_is_always_true() {
        // R3: 没有 obligation 时 hard_gate 不应误报未履约
        let candidates = vec![];
        assert!(obligation_satisfied(None, &candidates));
        assert!(obligation_satisfied(None, &["anything".into()]));
    }

    #[test]
    fn obligation_satisfied_with_empty_must_emit_is_always_true() {
        // R3: 空 must_emit_any_of 等同于无 obligation
        let o = ActivationObligation {
            on_trigger: "work.done".into(),
            must_emit_any_of: vec![],
        };
        assert!(obligation_satisfied(Some(&o), &[]));
    }

    #[test]
    fn obligation_satisfied_when_candidate_matches_must_emit() {
        // review-coordinator 选 wave 或 passed，agent 发 review.passed 满足
        let o = ActivationObligation {
            on_trigger: "work.done".into(),
            must_emit_any_of: vec!["review.wave.ready".into(), "review.passed".into()],
        };
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["review.passed".into()]
        ));
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["review.wave.ready".into()]
        ));
    }

    #[test]
    fn obligation_not_satisfied_when_candidate_is_off_obligation_set() {
        // R3: agent 发出 work.failed 不在 review-coordinator 的 obligation
        // 集合中 → obligation 未满足 → 进入 missing-event 分支
        // (但 hard_gate 自身不区分 0 candidate vs candidate-off-set,
        //  下游 reporter 必须根据候选 topic 决定是 missing 还是 wrong)
        let o = ActivationObligation {
            on_trigger: "work.done".into(),
            must_emit_any_of: vec!["review.wave.ready".into(), "review.passed".into()],
        };
        assert!(!obligation_satisfied(Some(&o), &vec!["work.failed".into()]));
        assert!(!obligation_satisfied(Some(&o), &vec![]));
    }

    #[test]
    fn hat_config_parses_obligations_from_yaml() {
        // 序列化往返：YAML 解析与写出保持 obligation 结构稳定
        let yaml = r#"
name: "Review Coordinator"
triggers: ["work.done", "fix.applied"]
publishes: ["review.wave.ready", "review.passed"]
obligations:
  - on_trigger: "work.done"
    must_emit_any_of: ["review.wave.ready", "review.passed"]
  - on_trigger: "fix.applied"
    must_emit_any_of: ["review.passed"]
"#;
        let hat: HatConfig = serde_yaml::from_str(yaml).expect("parse hat yaml");
        assert_eq!(hat.obligations.len(), 2);
        assert_eq!(hat.obligations[0].on_trigger, "work.done");
        assert_eq!(hat.obligations[0].must_emit_any_of.len(), 2);
        assert_eq!(hat.obligations[1].on_trigger, "fix.applied");
        assert_eq!(
            hat.obligations[1].must_emit_any_of,
            vec!["review.passed".to_string()]
        );
    }
}
