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

/// Activation-level publish obligation (2026-06-07 plan U4, hardened 2026-06-08).
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
///
/// 2026-06-08 fix: added `conditional_must_emit` to support
/// per-trigger-payload tightening.  When the trigger event's payload
/// matches a conditional's `when` predicate, the candidate topics
/// must satisfy the *conditional* `must_emit_any_of` (which is
/// strictly tighter than the legacy OR semantics on the top-level
/// `must_emit_any_of`).  This closes the gap where a hat
/// (e.g. `review-coordinator`) was technically satisfying its
/// obligation by emitting `review.passed` for a non-trivial diff,
/// skipping the wave entirely.
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
    /// 2026-06-08 fix: conditional tightening.  When a conditional's
    /// `when` predicate matches the trigger event context, the
    /// candidate topics must satisfy that conditional's
    /// `must_emit_any_of` (stricter than the legacy OR).  When no
    /// conditional matches, the obligation falls back to the
    /// top-level `must_emit_any_of` (legacy OR semantics).
    #[serde(default)]
    pub conditional_must_emit: Vec<ConditionalEmission>,
}

/// A single conditional tightening of an `ActivationObligation`.
///
/// When `when` matches the trigger event context, the candidate
/// topics emitted by the agent must intersect the conditional's
/// `must_emit_any_of`.  This is strictly tighter than the
/// top-level `must_emit_any_of` OR semantics — it expresses
/// "if the trigger payload shows non-trivial work, the agent MUST
/// pick this specific topic (or one of these)".
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConditionalEmission {
    /// Predicate over `TriggerContext`.  When `when` matches, this
    /// conditional's `must_emit_any_of` applies.  An empty
    /// `TriggerPredicate` (no fields set) matches all contexts and
    /// is equivalent to "always apply this strict rule".
    #[serde(default)]
    pub when: TriggerPredicate,
    /// Topics that satisfy this conditional.  Candidate topics
    /// must include at least one of these when `when` matches.
    pub must_emit_any_of: Vec<String>,
}

/// Predicate over `TriggerContext` (the trigger event payload
/// snapshot taken at hard-gate evaluation time).
///
/// All fields are AND-ed.  A field set to `None` is a wildcard
/// (matches anything).  An empty `TriggerPredicate` matches
/// everything.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TriggerPredicate {
    /// Minimum `commit_count` for the predicate to match.  When
    /// set, the trigger event's `commit_count` (from the event
    /// payload) must be `>= commit_count_min`.
    #[serde(default)]
    pub commit_count_min: Option<u32>,
    /// Minimum `changed_lines` for the predicate to match.  Same
    /// semantics as `commit_count_min` but on the `changed_lines`
    /// payload field.
    #[serde(default)]
    pub changed_lines_min: Option<u32>,
    /// Whether the working tree has untracked files.  When set,
    /// the trigger event's `has_untracked` payload field must
    /// equal this value.
    #[serde(default)]
    pub has_untracked: Option<bool>,
}

impl TriggerPredicate {
    /// Evaluate this predicate against a `TriggerContext`.  Returns
    /// `true` when all set fields match the context.  Unset fields
    /// are wildcards.
    pub fn matches(&self, ctx: &TriggerContext) -> bool {
        if let Some(min) = self.commit_count_min
            && ctx.commit_count.unwrap_or(0) < min
        {
            return false;
        }
        if let Some(min) = self.changed_lines_min
            && ctx.changed_lines.unwrap_or(0) < min
        {
            return false;
        }
        if let Some(want) = self.has_untracked
            && ctx.has_untracked.unwrap_or(false) != want
        {
            return false;
        }
        true
    }
}

/// Snapshot of the trigger event payload, used by
/// `obligation_satisfied` to evaluate `conditional_must_emit`.
///
/// Fields are `Option<...>` because not all trigger events carry
/// diff-state metadata — only the work-done / fix-applied family
/// does.  When a field is `None`, it is treated as the "neutral"
/// value (0 for counts, false for booleans) during predicate
/// evaluation, so a `commit_count_min: 1` predicate will not
/// match a `None` context.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TriggerContext {
    /// `commit_count` field from the trigger event payload.
    pub commit_count: Option<u32>,
    /// `changed_lines` field from the trigger event payload.
    pub changed_lines: Option<u32>,
    /// `has_untracked` field from the trigger event payload.
    pub has_untracked: Option<bool>,
}

impl TriggerContext {
    /// Construct from a trigger event's JSON payload (best-effort).
    /// Missing fields stay `None`; non-numeric / non-bool values
    /// also yield `None`.
    pub fn from_payload(payload: &serde_json::Value) -> Self {
        Self {
            commit_count: payload
                .get("commit_count")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok()),
            changed_lines: payload
                .get("changed_lines")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok()),
            has_untracked: payload.get("has_untracked").and_then(|v| v.as_bool()),
        }
    }
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
/// satisfies an obligation when:
///
/// 1. **No obligation** (`None` or empty lists) → always satisfied.
/// 2. **Any matching `conditional_must_emit` is satisfied** →
///    candidate topics must include at least one topic from the
///    conditional's `must_emit_any_of`.  ALL matching conditionals
///    must be satisfied (AND across conditionals).
/// 3. **Falls back to legacy OR semantics** when no conditional
///    matched: candidate topics must include at least one topic
///    from the top-level `must_emit_any_of`.
///
/// 2026-06-08 fix: the `trigger_context` parameter carries the
/// trigger event payload snapshot (commit_count / changed_lines /
/// has_untracked) so that conditional tightening can fire when the
/// work is non-trivial.  Pass `None` for legacy behavior
/// (no payload available).  Lives at module scope so the
/// `hat.rs` test module can exercise it without touching the public
/// `HatConfig` API.
pub fn obligation_satisfied(
    obligation: Option<&ActivationObligation>,
    candidate_topics: &[String],
    trigger_context: Option<&TriggerContext>,
) -> bool {
    let Some(o) = obligation else {
        return true; // No obligation → any outcome is fine.
    };
    let has_top_level = !o.must_emit_any_of.is_empty();
    let has_conditionals = !o.conditional_must_emit.is_empty();
    if !has_top_level && !has_conditionals {
        return true; // Empty obligation → no enforcement.
    }
    let ctx = trigger_context.cloned().unwrap_or_default();
    let mut any_conditional_matched = false;
    for cond in &o.conditional_must_emit {
        if !cond.when.matches(&ctx) {
            continue;
        }
        any_conditional_matched = true;
        // Conditional matched → candidate MUST be in the strict set.
        let matched = cond
            .must_emit_any_of
            .iter()
            .any(|m| candidate_topics.iter().any(|t| t == m));
        if !matched {
            return false;
        }
    }
    if any_conditional_matched {
        // Strict conditionals already validated; obligation satisfied.
        return true;
    }
    // No conditional matched: fall back to legacy OR semantics on the
    // top-level must_emit_any_of (preserves the 2026-06-07 behaviour
    // for presets that do not opt into conditional tightening).
    has_top_level
        && o.must_emit_any_of
            .iter()
            .any(|m| candidate_topics.iter().any(|t| t == m))
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
            conditional_must_emit: vec![],
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
        assert!(obligation_satisfied(None, &candidates, None));
        assert!(obligation_satisfied(None, &["anything".into()], None));
    }

    #[test]
    fn obligation_satisfied_with_empty_lists_is_always_true() {
        // R3: 空 must_emit_any_of + 空 conditional_must_emit 等同于无 obligation
        let o = ActivationObligation {
            on_trigger: "work.done".into(),
            must_emit_any_of: vec![],
            conditional_must_emit: vec![],
        };
        assert!(obligation_satisfied(Some(&o), &[], None));
    }

    #[test]
    fn obligation_satisfied_when_candidate_matches_must_emit() {
        // review-coordinator 选 wave 或 passed，agent 发 review.passed 满足
        let o = ActivationObligation {
            on_trigger: "work.done".into(),
            must_emit_any_of: vec!["review.wave.ready".into(), "review.passed".into()],
            conditional_must_emit: vec![],
        };
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["review.passed".into()],
            None
        ));
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["review.wave.ready".into()],
            None
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
            conditional_must_emit: vec![],
        };
        assert!(!obligation_satisfied(
            Some(&o),
            &vec!["work.failed".into()],
            None
        ));
        assert!(!obligation_satisfied(Some(&o), &vec![], None));
    }

    // ─── 2026-06-08 fix: conditional tightening tests ───

    /// When the trigger payload shows non-trivial work (commit_count >= 1),
    /// the candidate must be `review.wave.ready`, NOT `review.passed`.
    /// This is the bug the diagnostic report identified: review-coordinator
    /// was short-circuiting to `review.passed` even for 400-line diffs.
    #[test]
    fn conditional_must_emit_tightens_when_commit_count_positive() {
        let o = ActivationObligation {
            on_trigger: "work.done".into(),
            // legacy top-level still allows both (preserves OR semantics
            // for backward compatibility)
            must_emit_any_of: vec!["review.wave.ready".into(), "review.passed".into()],
            conditional_must_emit: vec![ConditionalEmission {
                when: TriggerPredicate {
                    commit_count_min: Some(1),
                    ..Default::default()
                },
                must_emit_any_of: vec!["review.wave.ready".into()],
            }],
        };
        // Non-trivial: agent emitted review.passed → NOT satisfied
        let ctx_non_trivial = TriggerContext {
            commit_count: Some(2),
            ..Default::default()
        };
        assert!(
            !obligation_satisfied(
                Some(&o),
                &vec!["review.passed".into()],
                Some(&ctx_non_trivial)
            ),
            "non-trivial diff with review.passed must NOT satisfy obligation"
        );
        // Non-trivial: agent emitted review.wave.ready → satisfied
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["review.wave.ready".into()],
            Some(&ctx_non_trivial)
        ));
        // Trivial: agent emitted review.passed → satisfied (legacy OR)
        let ctx_trivial = TriggerContext {
            commit_count: Some(0),
            ..Default::default()
        };
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["review.passed".into()],
            Some(&ctx_trivial)
        ));
    }

    /// When `changed_lines >= 50` the candidate must be `review.wave.ready`.
    /// Mirrors the preset hard rule (200-line / 400-line diffs in the
    /// diagnostic report).
    #[test]
    fn conditional_must_emit_tightens_when_changed_lines_meet_threshold() {
        let o = ActivationObligation {
            on_trigger: "work.done".into(),
            must_emit_any_of: vec!["review.wave.ready".into(), "review.passed".into()],
            conditional_must_emit: vec![ConditionalEmission {
                when: TriggerPredicate {
                    changed_lines_min: Some(50),
                    ..Default::default()
                },
                must_emit_any_of: vec!["review.wave.ready".into()],
            }],
        };
        // 400-line diff: review.passed must NOT satisfy
        let ctx_big = TriggerContext {
            changed_lines: Some(400),
            ..Default::default()
        };
        assert!(!obligation_satisfied(
            Some(&o),
            &vec!["review.passed".into()],
            Some(&ctx_big)
        ));
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["review.wave.ready".into()],
            Some(&ctx_big)
        ));
        // 10-line diff: review.passed still satisfies (legacy OR)
        let ctx_small = TriggerContext {
            changed_lines: Some(10),
            ..Default::default()
        };
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["review.passed".into()],
            Some(&ctx_small)
        ));
    }

    /// Untracked files (a common source of "review.passed when there is
    /// actual untracked work") must trigger the wave.
    #[test]
    fn conditional_must_emit_tightens_when_has_untracked() {
        let o = ActivationObligation {
            on_trigger: "work.done".into(),
            must_emit_any_of: vec!["review.wave.ready".into(), "review.passed".into()],
            conditional_must_emit: vec![ConditionalEmission {
                when: TriggerPredicate {
                    has_untracked: Some(true),
                    ..Default::default()
                },
                must_emit_any_of: vec!["review.wave.ready".into()],
            }],
        };
        let ctx_untracked = TriggerContext {
            has_untracked: Some(true),
            ..Default::default()
        };
        assert!(!obligation_satisfied(
            Some(&o),
            &vec!["review.passed".into()],
            Some(&ctx_untracked)
        ));
        let ctx_clean = TriggerContext {
            has_untracked: Some(false),
            ..Default::default()
        };
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["review.passed".into()],
            Some(&ctx_clean)
        ));
    }

    /// Multiple conditionals all match → ALL must be satisfied
    /// (AND across conditionals).  Defends against a "close enough"
    /// emit that hits one conditional but not another.
    #[test]
    fn multiple_matching_conditionals_all_must_be_satisfied() {
        let o = ActivationObligation {
            on_trigger: "work.done".into(),
            must_emit_any_of: vec!["review.wave.ready".into(), "review.passed".into()],
            conditional_must_emit: vec![
                ConditionalEmission {
                    when: TriggerPredicate {
                        commit_count_min: Some(1),
                        ..Default::default()
                    },
                    must_emit_any_of: vec!["review.wave.ready".into()],
                },
                ConditionalEmission {
                    when: TriggerPredicate {
                        changed_lines_min: Some(50),
                        ..Default::default()
                    },
                    must_emit_any_of: vec!["review.wave.ready".into()],
                },
            ],
        };
        // 2 commits, 400 lines: both conditionals match → must emit wave
        let ctx = TriggerContext {
            commit_count: Some(2),
            changed_lines: Some(400),
            ..Default::default()
        };
        assert!(!obligation_satisfied(
            Some(&o),
            &vec!["review.passed".into()],
            Some(&ctx)
        ));
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["review.wave.ready".into()],
            Some(&ctx)
        ));
    }

    /// `trigger_context = None` (legacy callers that cannot supply
    /// payload) → fall back to top-level OR semantics, conditionals
    /// are skipped (they need a context to evaluate).
    #[test]
    fn no_trigger_context_skips_conditionals_falls_back_to_top_level() {
        let o = ActivationObligation {
            on_trigger: "work.done".into(),
            must_emit_any_of: vec!["review.wave.ready".into(), "review.passed".into()],
            conditional_must_emit: vec![ConditionalEmission {
                when: TriggerPredicate {
                    commit_count_min: Some(1),
                    ..Default::default()
                },
                must_emit_any_of: vec!["review.wave.ready".into()],
            }],
        };
        // No context → conditionals skipped → legacy OR applies.
        // review.passed satisfies the top-level OR even though
        // a context would have rejected it.  This preserves the
        // 2026-06-07 behavior for callers that haven't been
        // updated to pass context yet.
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["review.passed".into()],
            None
        ));
    }

    /// When the payload explicitly carries `has_untracked: false` and
    /// the predicate asks for `has_untracked: true`, the predicate
    /// must NOT match.  Guards against a default-true slipping
    /// through when the payload actually says "no untracked".
    #[test]
    fn has_untracked_false_does_not_match_true_predicate() {
        let pred = TriggerPredicate {
            has_untracked: Some(true),
            ..Default::default()
        };
        let ctx = TriggerContext {
            has_untracked: Some(false),
            ..Default::default()
        };
        assert!(!pred.matches(&ctx));
    }

    /// `TriggerContext::from_payload` extracts numeric and bool fields,
    /// tolerates missing fields (yields `None`), and ignores
    /// non-numeric / non-bool types.
    #[test]
    fn trigger_context_from_payload_extracts_fields() {
        let payload = serde_json::json!({
            "commit_count": 3,
            "changed_lines": 400u64,
            "has_untracked": true,
            "plan_name": "noise field"
        });
        let ctx = TriggerContext::from_payload(&payload);
        assert_eq!(ctx.commit_count, Some(3));
        assert_eq!(ctx.changed_lines, Some(400));
        assert_eq!(ctx.has_untracked, Some(true));

        let empty = serde_json::json!({});
        let ctx_empty = TriggerContext::from_payload(&empty);
        assert_eq!(ctx_empty.commit_count, None);
        assert_eq!(ctx_empty.changed_lines, None);
        assert_eq!(ctx_empty.has_untracked, None);

        // Wrong types → None
        let bad = serde_json::json!({
            "commit_count": "three",
            "has_untracked": 1
        });
        let ctx_bad = TriggerContext::from_payload(&bad);
        assert_eq!(ctx_bad.commit_count, None);
        assert_eq!(ctx_bad.has_untracked, None);
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

    /// 2026-06-08 fix: YAML must accept `conditional_must_emit` and
    /// round-trip the new structure.
    #[test]
    fn hat_config_parses_conditional_must_emit_from_yaml() {
        let yaml = r#"
name: "Review Coordinator"
triggers: ["work.done"]
publishes: ["review.wave.ready", "review.passed"]
obligations:
  - on_trigger: "work.done"
    must_emit_any_of: ["review.wave.ready", "review.passed"]
    conditional_must_emit:
      - when:
          commit_count_min: 1
        must_emit_any_of: ["review.wave.ready"]
      - when:
          changed_lines_min: 50
        must_emit_any_of: ["review.wave.ready"]
      - when:
          has_untracked: true
        must_emit_any_of: ["review.wave.ready"]
"#;
        let hat: HatConfig = serde_yaml::from_str(yaml).expect("parse hat yaml");
        assert_eq!(hat.obligations.len(), 1);
        let conds = &hat.obligations[0].conditional_must_emit;
        assert_eq!(conds.len(), 3);
        assert_eq!(conds[0].when.commit_count_min, Some(1));
        assert_eq!(conds[1].when.changed_lines_min, Some(50));
        assert_eq!(conds[2].when.has_untracked, Some(true));
        for c in conds {
            assert_eq!(c.must_emit_any_of, vec!["review.wave.ready".to_string()]);
        }
    }
}
