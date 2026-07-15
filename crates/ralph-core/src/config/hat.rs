//! Hat configuration types.

use std::collections::{HashMap, HashSet};

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
///
/// 2026-06-09 fix: added `conditional_forbid_topics` to support
/// the inverse — "when the trigger payload matches this predicate,
/// the candidate topics MUST NOT include any of these forbidden
/// topics".  Combined with `must_emit_any_of`, this expresses
/// per-payload emit contracts that go beyond pure OR semantics.
/// Primary use case: a `reporter` hat that must emit `report.done`
/// but MUST NOT also emit `LOOP_COMPLETE` when the upstream
/// `REVIEW_COMPLETE` carried `pass_or_fail: "fail"`.  Closes the
/// "rogue LOOP_COMPLETE masks review failure" gap that was
/// documented in the 2026-06-09 ce-executor mechanism-vs-orchestration
/// diagnosis.
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
    /// 2026-06-09: per-payload forbidden topics.  When a `when`
    /// predicate matches, the candidate topics MUST NOT include
    /// any topic in `forbid_topics`.  Multiple `ConditionalForbid`
    /// entries are AND-ed: every matching entry must pass.  When
    /// no `when` predicate matches, the forbid list does not
    /// apply (the obligation falls back to the standard
    /// `must_emit_any_of` rule).  This is the inverse of
    /// `conditional_must_emit` and complements it.
    #[serde(default)]
    pub conditional_forbid_topics: Vec<ConditionalForbid>,
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

/// 2026-06-09: a single conditional `forbid` rule on an
/// `ActivationObligation`.  The inverse of `ConditionalEmission`:
/// when `when` matches, the candidate topics MUST NOT include
/// any topic in `forbid_topics`.  Empties (no `when` match)
/// leave the rule inapplicable.
///
/// Example — forbid `LOOP_COMPLETE` on a failing review:
///
/// ```yaml
/// - on_trigger: "REVIEW_COMPLETE"
///   must_emit_any_of: ["report.done"]
///   conditional_forbid_topics:
///     - when:
///         payload_field_equals:
///           pass_or_fail: "fail"
///       forbid_topics: ["LOOP_COMPLETE"]
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConditionalForbid {
    /// Predicate over `TriggerContext`.  When `when` matches, this
    /// forbid rule applies.  Empty `TriggerPredicate` matches
    /// every context — use with care, it forbids the listed
    /// topics unconditionally.
    #[serde(default)]
    pub when: TriggerPredicate,
    /// Topics that MUST NOT appear in the candidate set when
    /// `when` matches.  At least one of these appearing in
    /// `candidate_topics` makes the obligation fail.
    pub forbid_topics: Vec<String>,
}

/// Predicate over `TriggerContext` (the trigger event payload
/// snapshot taken at hard-gate evaluation time).
///
/// All fields are AND-ed.  A field set to `None` / empty is a
/// wildcard (matches anything).  An empty `TriggerPredicate`
/// matches everything.
///
/// 2026-06-09 fix: added `payload_field_equals` so predicates
/// can match arbitrary string-valued payload fields (e.g.
/// `pass_or_fail: "fail"` from `REVIEW_COMPLETE`).  This is the
/// mechanism that lets `conditional_forbid_topics` tighten
/// obligations per-reporter-verdict.
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
    /// 2026-06-09: per-field string equality predicates over the
    /// trigger event's payload snapshot.  All entries are AND-ed
    /// with the other predicate fields.  Each entry requires the
    /// trigger payload to have the named field with the exact
    /// string value.  Missing or differently-typed fields cause
    /// the predicate to NOT match (consistent with the "unset
    /// field" semantics on the typed predicates above).
    ///
    /// Example:
    /// ```yaml
    /// payload_field_equals:
    ///   pass_or_fail: "fail"
    ///   verdict: "fail"
    /// ```
    #[serde(default)]
    pub payload_field_equals: HashMap<String, String>,
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
        // payload_field_equals: every (key, want) pair must match the
        // trigger context's payload_fields.  A missing field (None in
        // ctx) causes the predicate to NOT match, mirroring the
        // "None treated as neutral" semantic on the typed fields.
        for (field, want) in &self.payload_field_equals {
            match ctx.payload_fields.get(field) {
                Some(got) if got == want => continue,
                _ => return false,
            }
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
///
/// 2026-06-09 fix: added `payload_fields` (string snapshot of the
/// full trigger payload) so `TriggerPredicate::payload_field_equals`
/// can match arbitrary string-valued fields like `pass_or_fail`.
/// Numeric / boolean fields are still surfaced via the typed
/// fields above for backward compatibility.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TriggerContext {
    /// `commit_count` field from the trigger event payload.
    pub commit_count: Option<u32>,
    /// `changed_lines` field from the trigger event payload.
    pub changed_lines: Option<u32>,
    /// `has_untracked` field from the trigger event payload.
    pub has_untracked: Option<bool>,
    /// 2026-06-09: string snapshot of all string-valued payload
    /// fields on the trigger event.  Populated by `from_payload`
    /// and consumed by `TriggerPredicate::payload_field_equals`.
    /// Non-string fields (numbers, bools, arrays, objects) are
    /// skipped — use the typed fields above for those.
    #[serde(default)]
    pub payload_fields: HashMap<String, String>,
}

impl TriggerContext {
    /// Construct from a trigger event's JSON payload (best-effort).
    /// Missing fields stay `None`; non-numeric / non-bool values
    /// also yield `None`.  All string-valued fields are mirrored
    /// into `payload_fields` for predicate lookup.
    pub fn from_payload(payload: &serde_json::Value) -> Self {
        // Mirror all string-valued top-level payload fields for
        // generic predicate lookup.  Skip non-string scalars and
        // nested structures — those are still surfaced via the
        // typed fields above when their meaning is known.
        let payload_fields = payload
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
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
            payload_fields,
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

    /// Terminal event topics that signal activation completion (U1 lifecycle config).
    ///
    /// Each activation of this hat is considered complete when any one of these
    /// topics is emitted.  The set is non-empty for hats that participate in the
    /// lifecycle contract; an empty set means the hat has no terminal events
    /// configured (legacy / non-participating hats).
    ///
    /// Supports two YAML forms:
    /// - **Array:** `terminal_events: ["work.done", "work.failed"]`
    /// - **Single string alias:** `terminal_event: "work.done"` (resolves to
    ///   a single-element set)
    ///
    /// The strict authoring contract (`ralph preset check --strict`) requires
    /// non-empty terminal sets; non-strict mode emits a warning for empty sets.
    #[serde(
        default,
        deserialize_with = "deserialize_terminal_events",
        alias = "terminal_event"
    )]
    pub terminal_events: Vec<String>,

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

    /// 2026-06-17-004 U2 (R3): per-hat missing-event gate grace
    /// window in seconds.  When the gate evaluates the obligation
    /// for this hat and the elapsed time since the hat's last
    /// activation is **less** than this value, the gate is
    /// suppressed (`should_gate_missing_events` returns `false`).
    /// This protects long-running hats (e.g. `dimension-reviewer`
    /// with `timeout: 1800`) from being mis-fired during the first
    /// ~30-60s of model warm-up just because no event has
    /// appeared on the bus yet.
    ///
    /// Resolution order in [`resolve_missing_event_grace_secs`]:
    ///   1. This per-hat value (highest priority).
    ///   2. The `EventLoopConfig::default_missing_event_grace_secs`
    ///      preset default (operator-controlled).
    ///   3. `min(idle_timeout_secs * 0.3, 540)` — diagnostic-
    ///      recommended default that scales with the backend idle
    ///      timeout, capped at 540s to prevent extremely short
    ///      adapter timeouts from collapsing the grace window to
    ///      a uselessly small value.
    ///   4. `0` — gate is never suppressed (legacy / opt-out).
    ///
    /// `None` means "fall through to the default chain".  Set to
    /// `Some(0)` to opt out of the grace entirely (the gate fires
    /// on the very first missing-event iteration).
    #[serde(default)]
    pub missing_event_grace_secs: Option<u32>,

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

    /// 2026-06-26 plan U2: topics this hat is **explicitly allowed**
    /// to publish even when the lint invariant would otherwise flag
    /// them. The list is consumed by the `hat_scope_invariant` lint
    /// (rule 2) so a hat can declare an escape hatch for topics that
    /// are too operational to put under `topic_deny_rules` (e.g.
    /// `work.internal`).
    #[serde(default)]
    pub exempt_topics: Vec<String>,

    /// 2026-06-29-007 plan U5a: paths this hat is allowed to
    /// write. The dimension-reviewer lint
    /// (`dimension_reviewer_write_paths`) rejects any preset
    /// that grants `dimension-reviewer` access to `docs/plans/`
    /// (the reviewer is a code-only reviewer; letting it
    /// rewrite the plan mid-loop is the 2026-06-28
    /// scope_violation 早班 pattern).
    #[serde(default)]
    pub allowed_write_paths: Option<Vec<String>>,

    /// Phase-aware triggers: map from phase name to list of trigger topics.
    ///
    /// When present, the hat subscribes to the triggers of the current phase
    /// instead of the global `triggers` field. Useful for hats that should
    /// behave differently in warmup vs production (e.g., harness hat).
    #[serde(default)]
    pub phase_triggers: Option<HashMap<String, Vec<String>>>,

    /// Topics explicitly allowed to be claimed by multiple hats.
    ///
    /// Used for design-level multi-consumer topics (e.g. `fix.exhausted` /
    /// `debug.exhausted` consumed by both `plan-gate` and
    /// `debug-resolver`/`shipper`). When ALL hats subscribed to a given
    /// trigger list that trigger in their `trigger_multi_consumer_topics`,
    /// `validate_ambiguous_routing` skips the strict 1:1 check for that
    /// trigger. All consumer hats must opt in — a single missing entry
    /// triggers the standard AmbiguousRouting error.
    ///
    /// Empty (the default) means "use the legacy strict 1:1 mapping".
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub trigger_multi_consumer_topics: HashSet<String>,

    /// Fields to ignore when extracting payload field references from instructions.
    ///
    /// Used by the static payload contract validator to exclude false positives.
    /// Does not affect runtime event policy enforcement.
    #[serde(default)]
    pub ignore_payload_fields: Vec<String>,
}

/// Custom deserializer for `terminal_events` that accepts both:
/// - A JSON/YAML array: `["topic1", "topic2"]`
/// - A single string: `"topic1"` (resolved to a single-element array)
///
/// The `#[serde(alias = "terminal_event")]` on the struct field handles
/// the YAML key alias, while this function handles the value type alias.
fn deserialize_terminal_events<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum TerminalEventsInput {
        Array(Vec<String>),
        Single(String),
    }

    match TerminalEventsInput::deserialize(deserializer)? {
        TerminalEventsInput::Array(v) => Ok(v),
        TerminalEventsInput::Single(s) => Ok(vec![s]),
    }
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
            terminal_events: Vec::new(),
            instructions: String::new(),
            extra_instructions: Vec::new(),
            backend: None,
            backend_args: None,
            default_publishes: None,
            max_activations: None,
            scratchpad: None,
            disallowed_tools: Vec::new(),
            timeout: None,
            // 2026-06-17-004 U2 (R3): default `None` so the
            // `resolve_missing_event_grace_secs` helper falls
            // through to the operator-controlled default chain.
            missing_event_grace_secs: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            // 2026-06-26 plan U2: default no exempt list.
            exempt_topics: Vec::new(),
            // 2026-06-29-007 plan U5a: default no write paths.
            allowed_write_paths: None,
            phase_triggers: None,
            ignore_payload_fields: Vec::new(),
            obligations: Vec::new(),
            trigger_multi_consumer_topics: HashSet::new(),
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

    /// Returns the terminal event topics as a `HashSet` for efficient
    /// membership checks.  Empty set means no terminal events configured.
    pub fn terminal_topic_set(&self) -> std::collections::HashSet<&str> {
        self.terminal_events.iter().map(String::as_str).collect()
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
/// 4. **2026-06-09 fix**: every matching `conditional_forbid_topics`
///    entry MUST be respected — if any forbidden topic appears in
///    the candidate set when its `when` predicate matches, the
///    obligation is NOT satisfied.  This is the inverse of
///    `conditional_must_emit` and runs as an independent gate
///    (AND-ed with the must-emit checks above).  When no `when`
///    predicate matches, the forbid list does not apply.
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
    let has_forbids = !o.conditional_forbid_topics.is_empty();
    if !has_top_level && !has_conditionals && !has_forbids {
        return true; // Empty obligation → no enforcement.
    }
    let ctx = trigger_context.cloned().unwrap_or_default();
    // 2026-06-09: forbid gate runs first, independent of must-emit.
    // This is intentional — even if a hat satisfies the must-emit
    // rule (e.g. by emitting report.done), a forbidden topic in
    // the candidate set (e.g. LOOP_COMPLETE on a failing review)
    // must still fail the obligation.  Mirrors the "deny-list
    // pre-condition" pattern common in security policies.
    for forbid in &o.conditional_forbid_topics {
        if !forbid.when.matches(&ctx) {
            continue;
        }
        for forbidden in &forbid.forbid_topics {
            if candidate_topics.iter().any(|t| t == forbidden) {
                return false; // Deny-list hit → obligation fails.
            }
        }
    }
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

/// 2026-06-17-004 U2 (R3): resolve the missing-event gate grace
/// window for a given hat.  Resolution chain (KTD-4 in the plan):
///
///   1. `hat.missing_event_grace_secs` (per-hat override; explicit
///      `Some(0)` opts out and disables the grace).
///   2. `preset_default` — the operator-controlled default for the
///      preset, typically wired from `EventLoopConfig`.
///   3. `min(adapter_idle_secs * 0.3, 540)` — diagnostic-recommended
///      fallback that scales with the backend idle timeout, capped
///      at 540s to prevent extremely short adapter timeouts from
///      collapsing the grace window to a uselessly small value.
///   4. `0` — never suppress (legacy / opt-out).
///
/// The `0.3` multiplier matches the diagnostic report's
/// "≥ timeout×0.3" recommendation; the 540s floor keeps the
/// recommended default from collapsing to <30s for adapters with
/// `idle_timeout_secs < 100`.  `u32` is the natural unit
/// throughout the `HatConfig` API.
pub fn resolve_missing_event_grace_secs(
    hat: &HatConfig,
    preset_default: Option<u32>,
    adapter_idle_secs: u32,
) -> u32 {
    if let Some(secs) = hat.missing_event_grace_secs {
        return secs;
    }
    if let Some(secs) = preset_default {
        return secs;
    }
    // Fallback: scale with adapter idle, cap at 540s.
    let scaled = (adapter_idle_secs as f64 * 0.3).floor() as u32;
    scaled.min(540)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hat_backend_kiro_agent_variant_removed() {
        // U4: HatBackend::KiroAgent variant and its `agent` field are deleted.
        // The enum should now contain only Named, NamedWithArgs, Custom.
        // We assert by source-grepping hat.rs (not at runtime, since the
        // enum no longer compiles if the variant exists).
        let src = include_str!("hat.rs");
        let has_variant = src
            .lines()
            .map(str::trim_start)
            .any(|l| l.starts_with("KiroAgent {"));
        assert!(
            !has_variant,
            "HatBackend::KiroAgent variant must be deleted from hat.rs"
        );
    }

    fn hat_with_obligations(obligations: Vec<ActivationObligation>) -> HatConfig {
        HatConfig {
            name: "test".into(),
            description: None,
            triggers: vec!["work.done".into()],
            publishes: vec!["review.passed".into(), "review.wave.ready".into()],
            terminal_events: Vec::new(),
            instructions: String::new(),
            extra_instructions: Vec::new(),
            backend: None,
            backend_args: None,
            default_publishes: None,
            max_activations: None,
            scratchpad: None,
            disallowed_tools: Vec::new(),
            timeout: None,
            // 2026-06-17-004 U2 (R3): test helper does not need
            // the new field; explicit `None` keeps the helper
            // aligned with `HatConfig::default()`.
            missing_event_grace_secs: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            // 2026-06-26 plan U2: test helper does not exercise
            // the exempt list; default empty mirrors the
            // production default.
            exempt_topics: Vec::new(),
            // 2026-06-29-007 plan U5a: test helper does not
            // exercise write paths; default `None` mirrors
            // the production default.
            allowed_write_paths: None,
            phase_triggers: None,
            ignore_payload_fields: Vec::new(),
            obligations,
            trigger_multi_consumer_topics: HashSet::new(),
        }
    }

    #[test]
    fn obligation_for_trigger_returns_matching_obligation() {
        let hat = hat_with_obligations(vec![ActivationObligation {
            on_trigger: "work.done".into(),
            must_emit_any_of: vec!["review.wave.ready".into(), "review.passed".into()],
            conditional_must_emit: vec![],
            conditional_forbid_topics: vec![],
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
            conditional_forbid_topics: vec![],
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
            conditional_forbid_topics: vec![],
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
            conditional_forbid_topics: vec![],
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
            conditional_forbid_topics: vec![],
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
            conditional_forbid_topics: vec![],
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
            conditional_forbid_topics: vec![],
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
            conditional_forbid_topics: vec![],
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
            conditional_forbid_topics: vec![],
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

    // ─── 2026-06-09 fix: conditional_forbid_topics tests ───
    //
    // The reporter's "fail → do not emit LOOP_COMPLETE" hard rule
    // requires the obligation to express a deny-list.  These
    // tests pin the semantics of `conditional_forbid_topics` so
    // that:
    //   - fail payload + LOOP_COMPLETE candidate → obligation fails
    //   - pass payload + LOOP_COMPLETE candidate → obligation passes
    //   - fail payload + report.done candidate (no LOOP_COMPLETE) → passes
    //   - multiple forbids AND-ed when multiple predicates match
    //   - non-matching `when` predicate → forbid does not apply
    //   - deny-list pre-condition runs even when must-emit is satisfied

    /// Primary use case: REVIEW_COMPLETE with pass_or_fail=fail must
    /// reject a candidate set that contains LOOP_COMPLETE.  This is
    /// the exact "rogue LOOP_COMPLETE masks a failing review" bug
    /// that the 2026-06-09 diagnosis flagged as a real regression
    /// risk in the ce-executor preset.
    #[test]
    fn conditional_forbid_rejects_loop_complete_on_failing_review() {
        let o = ActivationObligation {
            on_trigger: "REVIEW_COMPLETE".into(),
            must_emit_any_of: vec!["report.done".into()],
            conditional_must_emit: vec![],
            conditional_forbid_topics: vec![ConditionalForbid {
                when: TriggerPredicate {
                    payload_field_equals: [("pass_or_fail".to_string(), "fail".to_string())]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
                forbid_topics: vec!["LOOP_COMPLETE".into()],
            }],
        };
        // Failing review + rogue LOOP_COMPLETE → NOT satisfied.
        let ctx_fail = TriggerContext {
            payload_fields: [("pass_or_fail".to_string(), "fail".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        assert!(
            !obligation_satisfied(
                Some(&o),
                &vec!["report.done".into(), "LOOP_COMPLETE".into()],
                Some(&ctx_fail)
            ),
            "failing review + LOOP_COMPLETE in candidate set must fail obligation"
        );
        // Failing review + only report.done → satisfied (the deny-list
        // pre-condition does not fire because LOOP_COMPLETE is absent).
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["report.done".into()],
            Some(&ctx_fail)
        ));
        // Passing review + LOOP_COMPLETE → satisfied (forbid does not
        // apply because the payload_field_equals predicate does not
        // match pass).  The legacy OR on must_emit_any_of still
        // accepts report.done.
        let ctx_pass = TriggerContext {
            payload_fields: [("pass_or_fail".to_string(), "pass".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["report.done".into(), "LOOP_COMPLETE".into()],
            Some(&ctx_pass)
        ));
    }

    /// `TriggerPredicate::payload_field_equals` predicate semantics.
    /// Mirrors the typed predicate fields above: every entry must
    /// match the trigger context's payload_fields, missing fields
    /// cause the predicate to NOT match.
    #[test]
    fn payload_field_equals_predicate_matches_string_payload() {
        let pred = TriggerPredicate {
            payload_field_equals: [("pass_or_fail".to_string(), "fail".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let ctx_match = TriggerContext {
            payload_fields: [("pass_or_fail".to_string(), "fail".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        assert!(pred.matches(&ctx_match));

        let ctx_mismatch = TriggerContext {
            payload_fields: [("pass_or_fail".to_string(), "pass".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        assert!(!pred.matches(&ctx_mismatch));

        // Missing field → predicate does NOT match.
        let ctx_missing = TriggerContext::default();
        assert!(!pred.matches(&ctx_missing));
    }

    /// `TriggerContext::from_payload` mirrors all string-valued
    /// top-level payload fields into `payload_fields`, so the
    /// `pass_or_fail` string from a `REVIEW_COMPLETE` event
    /// becomes available to predicates without any custom
    /// wiring.
    #[test]
    fn trigger_context_from_payload_mirrors_string_fields() {
        let payload = serde_json::json!({
            "pass_or_fail": "fail",
            "verdict": "fail",
            "plan_name": "noise field",
            "commit_count": 3,
            "nested": { "key": "value" }
        });
        let ctx = TriggerContext::from_payload(&payload);
        assert_eq!(
            ctx.payload_fields.get("pass_or_fail").map(String::as_str),
            Some("fail")
        );
        assert_eq!(
            ctx.payload_fields.get("verdict").map(String::as_str),
            Some("fail")
        );
        assert_eq!(
            ctx.payload_fields.get("plan_name").map(String::as_str),
            Some("noise field")
        );
        // Nested objects / non-string scalars should NOT be mirrored.
        assert!(ctx.payload_fields.get("nested").is_none());
        assert!(ctx.payload_fields.get("commit_count").is_none());
        // Typed fields are still extracted for backward compatibility.
        assert_eq!(ctx.commit_count, Some(3));
    }

    /// When no `when` predicate matches, the forbid list does not
    /// apply — the obligation falls through to the standard
    /// must_emit_any_of check.  This is the "fall-through"
    /// semantic that lets a single obligation express both
    /// "pass: allow LOOP_COMPLETE" and "fail: forbid LOOP_COMPLETE".
    #[test]
    fn conditional_forbid_falls_through_when_predicate_does_not_match() {
        // No payload_field_equals → empty predicate matches every context,
        // so the forbid ALWAYS applies.  This test uses a non-matching
        // commit_count_min to make the predicate selective.
        let o = ActivationObligation {
            on_trigger: "REVIEW_COMPLETE".into(),
            must_emit_any_of: vec!["report.done".into()],
            conditional_must_emit: vec![],
            conditional_forbid_topics: vec![ConditionalForbid {
                when: TriggerPredicate {
                    commit_count_min: Some(10),
                    ..Default::default()
                },
                forbid_topics: vec!["LOOP_COMPLETE".into()],
            }],
        };
        // commit_count=1 → predicate does not match → forbid does not apply.
        let ctx_small = TriggerContext {
            commit_count: Some(1),
            ..Default::default()
        };
        assert!(obligation_satisfied(
            Some(&o),
            &vec!["report.done".into(), "LOOP_COMPLETE".into()],
            Some(&ctx_small)
        ));
        // commit_count=42 → predicate matches → forbid applies.
        let ctx_big = TriggerContext {
            commit_count: Some(42),
            ..Default::default()
        };
        assert!(!obligation_satisfied(
            Some(&o),
            &vec!["report.done".into(), "LOOP_COMPLETE".into()],
            Some(&ctx_big)
        ));
    }

    /// Deny-list runs as a pre-condition, even when the candidate
    /// set would otherwise satisfy the must_emit rule.  Mirrors
    /// the "deny-list beats allow-list" pattern from security
    /// policies — the obligation reports failure the moment any
    /// forbidden topic is detected, regardless of what else the
    /// agent emitted.
    #[test]
    fn deny_list_pre_condition_runs_even_when_must_emit_satisfied() {
        let o = ActivationObligation {
            on_trigger: "REVIEW_COMPLETE".into(),
            must_emit_any_of: vec!["report.done".into()],
            conditional_must_emit: vec![],
            conditional_forbid_topics: vec![ConditionalForbid {
                when: TriggerPredicate {
                    payload_field_equals: [("pass_or_fail".to_string(), "fail".to_string())]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
                forbid_topics: vec!["LOOP_COMPLETE".into()],
            }],
        };
        let ctx_fail = TriggerContext {
            payload_fields: [("pass_or_fail".to_string(), "fail".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        // Both report.done AND LOOP_COMPLETE present:
        //   - must_emit satisfied (report.done ∈ must_emit_any_of)
        //   - forbid fails (LOOP_COMPLETE ∈ forbid_topics)
        //   → overall: NOT satisfied
        assert!(!obligation_satisfied(
            Some(&o),
            &vec!["report.done".into(), "LOOP_COMPLETE".into()],
            Some(&ctx_fail)
        ));
    }

    /// YAML round-trip: `conditional_forbid_topics` parses and
    /// serializes the same way as `conditional_must_emit`.  This
    /// is the contract the preset file relies on.
    #[test]
    fn hat_config_parses_conditional_forbid_from_yaml() {
        let yaml = r#"
name: "Reporter"
triggers: ["REVIEW_COMPLETE"]
publishes: ["report.done", "LOOP_COMPLETE"]
obligations:
  - on_trigger: "REVIEW_COMPLETE"
    must_emit_any_of: ["report.done"]
    conditional_forbid_topics:
      - when:
          payload_field_equals:
            pass_or_fail: "fail"
        forbid_topics: ["LOOP_COMPLETE"]
"#;
        let hat: HatConfig = serde_yaml::from_str(yaml).expect("parse hat yaml");
        assert_eq!(hat.obligations.len(), 1);
        let forbids = &hat.obligations[0].conditional_forbid_topics;
        assert_eq!(forbids.len(), 1);
        assert_eq!(forbids[0].forbid_topics, vec!["LOOP_COMPLETE".to_string()]);
        assert_eq!(
            forbids[0]
                .when
                .payload_field_equals
                .get("pass_or_fail")
                .map(String::as_str),
            Some("fail")
        );
    }

    // ─── U1: Lifecycle 配置模型 tests ───

    /// T-U1-1: 单字符串 `terminal_event` alias 解析为单元素集合，
    /// 与 `terminal_events` 数组形式等价。
    #[test]
    fn terminal_event_string_alias_parses_to_single_element_set() {
        let yaml_alias = r#"
name: "Executor"
triggers: ["work.ready"]
publishes: ["work.done", "work.failed"]
terminal_event: "work.done"
"#;
        let yaml_array = r#"
name: "Executor"
triggers: ["work.ready"]
publishes: ["work.done", "work.failed"]
terminal_events:
  - "work.done"
"#;
        let hat_alias: HatConfig = serde_yaml::from_str(yaml_alias).expect("parse yaml alias");
        let hat_array: HatConfig = serde_yaml::from_str(yaml_array).expect("parse yaml array");
        assert_eq!(hat_alias.terminal_events, hat_array.terminal_events);
        assert_eq!(hat_alias.terminal_events, vec!["work.done".to_string()]);
    }

    /// T-U1-2: `terminal_events` 数组形式解析为完整集合。
    #[test]
    fn terminal_events_array_parses_correctly() {
        let yaml = r#"
name: "Reporter"
triggers: ["REVIEW_COMPLETE"]
publishes: ["report.done", "LOOP_COMPLETE"]
terminal_events:
  - "report.done"
  - "LOOP_COMPLETE"
"#;
        let hat: HatConfig = serde_yaml::from_str(yaml).expect("parse yaml");
        assert_eq!(hat.terminal_events.len(), 2);
        assert!(hat.terminal_events.contains(&"report.done".to_string()));
        assert!(hat.terminal_events.contains(&"LOOP_COMPLETE".to_string()));
    }

    /// T-U1-3: 旧 preset（无 `terminal_event`/`terminal_events` 字段）
    /// 解析为默认空集合，不阻塞。
    #[test]
    fn old_preset_without_terminal_events_parses_to_empty_vec() {
        let yaml = r#"
name: "Legacy"
triggers: ["work.start"]
publishes: ["work.done"]
"#;
        let hat: HatConfig = serde_yaml::from_str(yaml).expect("parse yaml");
        assert!(hat.terminal_events.is_empty());
    }

    /// T-U1-4: `terminal_topic_set()` 返回高效成员检查集合。
    #[test]
    fn terminal_topic_set_returns_hashset() {
        let hat = HatConfig {
            terminal_events: vec!["work.done".into(), "work.failed".into()],
            ..HatConfig::default()
        };
        let set = hat.terminal_topic_set();
        assert!(set.contains("work.done"));
        assert!(set.contains("work.failed"));
        assert!(!set.contains("review.passed"));
    }

    /// T-U1-5: 空 `terminal_events` 时 `terminal_topic_set()` 返回空集合。
    #[test]
    fn terminal_topic_set_empty_when_no_terminal_events() {
        let hat = HatConfig::default();
        assert!(hat.terminal_topic_set().is_empty());
    }

    /// T-U1-6: YAML 序列化往返 — terminal_events 写出后重新解析保持一致。
    #[test]
    fn terminal_events_roundtrip_through_yaml() {
        let hat = HatConfig {
            name: "Executor".into(),
            description: Some("Test".into()),
            triggers: vec!["work.ready".into()],
            publishes: vec!["work.done".into(), "work.failed".into()],
            terminal_events: vec!["work.done".into(), "work.failed".into()],
            ..HatConfig::default()
        };
        let yaml = serde_yaml::to_string(&hat).expect("serialize");
        let parsed: HatConfig = serde_yaml::from_str(&yaml).expect("deserialize");
        assert_eq!(parsed.terminal_events, hat.terminal_events);
    }
}
