//! Preset Static Lint — configuration model and shared topic enumeration.
//!
//! This module provides the foundation for authoring-time static lint
//! checks on preset configurations. It defines:
//!
//! - **Topic format validation**: a single lowercase dot-case validator
//!   shared across all topic surfaces, with an explicit whitelist for
//!   protocol tokens like `LOOP_COMPLETE`.
//! - **Topic occurrence enumeration**: collects every topic from hat
//!   `triggers`, `publishes`, `default_publishes`, obligations, event
//!   policy schemas, required events, starting/cancellation/completion
//!   topics, verdict gate topics, workflow guard topics, and ownership
//!   keys.
//! - **Deterministic suggestions**: given an invalid token, produce a
//!   stable, lowercased-and-hyphenated suggestion.
//!
//! Implementation Plan Unit: U1 of `2026-06-08-003-feat-preset-static-lint-plan`.
//!
//! Stability rules:
//! - The `finding_id` constants are part of the public contract.
//! - The `TopicSurface` enum variants are source of truth for which
//!   config locations are linted.
//! - `TopicOccurrence` fields (`topic`, `surface`, `hat`) are stable.

use std::collections::{BTreeMap, BTreeSet};

// ──────────────────────────────────────────────────────────────────────────
// Finding ID constants
// ──────────────────────────────────────────────────────────────────────────

/// Stable machine ID for a topic that violates the lowercase dot-case format
/// and is NOT in the whitelist.
pub const FINDING_INVALID_TOPIC_FORMAT: &str = "preset.invalid_topic_format";

/// Stable machine ID for a topic that matches the whitelist — reported as
/// `Pass` severity for informational purposes.
pub const FINDING_WHITELIST_EXEMPT_TOPIC: &str = "preset.whitelist_exempt_topic";

// ──────────────────────────────────────────────────────────────────────────
// Topic surface — where a topic was found in the config.
// ──────────────────────────────────────────────────────────────────────────

/// Source location of a topic token in the preset configuration.
///
/// Each variant maps to one or more config fields. The lint enumerates
/// all surfaces to build a complete topic graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TopicSurface {
    /// Hat `triggers` field.
    Triggers,
    /// Hat `publishes` field.
    Publishes,
    /// Hat `default_publishes` field.
    DefaultPublishes,
    /// Hat `obligations[].must_emit_any_of` field.
    ObligationEmit,
    /// `event_loop.event_policy.schemas` keys.
    EventPolicySchema,
    /// `event_loop.required_events` entries.
    RequiredEvent,
    /// `event_loop.starting_event` field.
    StartingEvent,
    /// `event_loop.cancellation_promise` field.
    CancellationPromise,
    /// `event_loop.completion_promise` field.
    CompletionPromise,
    /// `event_loop.verdict_gate` topic field.
    VerdictGate,
    /// `event_loop.workflow_guards.chains[].topics` entries.
    WorkflowGuardTopic,
    /// `topic_owners` keys (the topic being owned).
    OwnershipKey,
}

impl TopicSurface {
    /// Human-readable label for display in lint reports.
    pub fn label(self) -> &'static str {
        match self {
            Self::Triggers => "triggers",
            Self::Publishes => "publishes",
            Self::DefaultPublishes => "default_publishes",
            Self::ObligationEmit => "obligations.must_emit_any_of",
            Self::EventPolicySchema => "event_policy.schemas",
            Self::RequiredEvent => "required_events",
            Self::StartingEvent => "starting_event",
            Self::CancellationPromise => "cancellation_promise",
            Self::CompletionPromise => "completion_promise",
            Self::VerdictGate => "verdict_gate",
            Self::WorkflowGuardTopic => "workflow_guards.topics",
            Self::OwnershipKey => "topic_owners",
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Topic occurrence — a single occurrence of a topic in the config.
// ──────────────────────────────────────────────────────────────────────────

/// A single occurrence of a topic token in the preset configuration.
///
/// Used by the topic enumerator to build a complete surface graph for
/// format validation and ownership checks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub struct TopicOccurrence {
    /// The raw topic token as it appears in the config.
    pub topic: String,
    /// Where the topic was found.
    pub surface: TopicSurface,
    /// The hat that declared this topic (if applicable).
    pub hat: Option<String>,
}

impl TopicOccurrence {
    pub fn new(topic: impl Into<String>, surface: TopicSurface, hat: Option<String>) -> Self {
        Self {
            topic: topic.into(),
            surface,
            hat,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Topic format validation
// ──────────────────────────────────────────────────────────────────────────

/// Result of validating a single topic token against the format rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicFormatResult {
    /// The original token.
    pub token: String,
    /// Whether the token matches the lowercase dot-case format.
    pub is_valid: bool,
    /// Whether the token is in the whitelist (exempt from format checks).
    pub is_whitelisted: bool,
    /// A deterministic suggestion for fixing the token (only if `is_valid`
    /// and `!is_whitelisted`).
    pub suggestion: Option<String>,
}

/// Validate a topic token against the lowercase dot-case format.
///
/// The canonical format is: `^[a-z][a-z0-9]*(\.[a-z][a-z0-9]*)*$`
///
/// - Must start with a lowercase letter.
/// - Segments are separated by dots.
/// - Each segment starts with a lowercase letter or digit.
/// - No uppercase, no underscores, no hyphens.
///
/// If the token is in the whitelist, it is reported as exempt (valid).
pub fn validate_topic_format(token: &str, whitelist: &[String]) -> TopicFormatResult {
    let is_whitelisted = whitelist.iter().any(|w| w == token);
    let is_valid = is_whitelisted || is_valid_lowercase_dot_case(token);
    let suggestion = if is_valid {
        None
    } else {
        Some(suggest_topic_fix(token))
    };
    TopicFormatResult {
        token: token.to_string(),
        is_valid,
        is_whitelisted,
        suggestion,
    }
}

/// Check whether a token matches the canonical lowercase dot-case format.
///
/// Pattern: `[a-z][a-z0-9]*(\.[a-z][a-z0-9]*)*`
fn is_valid_lowercase_dot_case(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let mut chars = token.chars();
    // First char must be lowercase letter.
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    let mut prev_was_dot = false;
    for c in chars {
        if c == '.' {
            if prev_was_dot {
                return false; // consecutive dots
            }
            prev_was_dot = true;
        } else if prev_was_dot {
            // After a dot, must be lowercase letter or digit.
            if !(c.is_ascii_lowercase() || c.is_ascii_digit()) {
                return false;
            }
            prev_was_dot = false;
        } else {
            // Within a segment: lowercase letter, digit.
            if !(c.is_ascii_lowercase() || c.is_ascii_digit()) {
                return false;
            }
        }
    }
    // Token must not end with a dot.
    !prev_was_dot
}

/// Generate a deterministic suggestion for an invalid topic token.
///
/// Rules:
/// - Lowercase all characters.
/// - Insert dots at CamelCase boundaries (e.g. `camelCase` → `camel.case`).
/// - Replace underscores with dots.
/// - Collapse consecutive dots.
/// - Ensure each segment starts with a letter.
/// - Ensure the token does not start or end with a dot.
///
/// Examples:
/// - `REVIEW_COMPLETE` → `review.complete`
/// - `camelCase` → `camel.case`
/// - `123start` → `a123start`
pub fn suggest_topic_fix(token: &str) -> String {
    // Step 1: Split CamelCase boundaries into separate segments.
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut prev_was_upper = false;
    let mut prev_was_digit = false;

    for c in token.chars() {
        if c == '_' || c == '.' || c == '-' || c == ' ' {
            // Explicit separator — flush current and use dot.
            if !current.is_empty() {
                segments.push(current.clone());
                current.clear();
            }
            prev_was_upper = false;
            prev_was_digit = false;
        } else if c.is_ascii_uppercase() {
            if !current.is_empty() && !prev_was_upper {
                // CamelCase boundary: flush before uppercase.
                segments.push(current.clone());
                current.clear();
            }
            current.push(c.to_ascii_lowercase());
            prev_was_upper = true;
            prev_was_digit = false;
        } else if c.is_ascii_digit() {
            if !current.is_empty() && !prev_was_digit && !prev_was_upper {
                // Digit boundary: flush before digit (if previous was letter).
                segments.push(current.clone());
                current.clear();
            }
            current.push(c);
            prev_was_upper = false;
            prev_was_digit = true;
        } else {
            current.push(c.to_ascii_lowercase());
            prev_was_upper = false;
            prev_was_digit = false;
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }

    // Step 2: Filter empty segments and ensure each starts with a letter.
    let fixed_segments: Vec<String> = segments
        .iter()
        .filter(|s| !s.is_empty())
        .map(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                Some(c) if c.is_ascii_alphabetic() => {
                    format!("{}{}", c, chars.collect::<String>())
                }
                Some(c) if c.is_ascii_digit() => {
                    format!("a{}{}", c, chars.collect::<String>())
                }
                _ => format!("a{}", seg),
            }
        })
        .collect();

    if fixed_segments.is_empty() {
        "invalid".to_string()
    } else {
        fixed_segments.join(".")
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Topic surface enumeration from RalphConfig
// ──────────────────────────────────────────────────────────────────────────

use crate::config::RalphConfig;

/// Collect all topic tokens from the configuration across every surface.
///
/// Returns a deduplicated, sorted list of `TopicOccurrence` values.
/// This is the shared enumeration used by format validation, ownership
/// checks, and the aggregator (U3).
pub fn enumerate_topics(config: &RalphConfig) -> Vec<TopicOccurrence> {
    let mut occurrences: BTreeSet<TopicOccurrence> = BTreeSet::new();

    for (hat_id, hat_config) in &config.hats {
        // triggers
        for topic in &hat_config.triggers {
            occurrences.insert(TopicOccurrence::new(
                topic.clone(),
                TopicSurface::Triggers,
                Some(hat_id.clone()),
            ));
        }
        // publishes
        for topic in &hat_config.publishes {
            occurrences.insert(TopicOccurrence::new(
                topic.clone(),
                TopicSurface::Publishes,
                Some(hat_id.clone()),
            ));
        }
        // default_publishes
        if let Some(default) = &hat_config.default_publishes {
            occurrences.insert(TopicOccurrence::new(
                default.clone(),
                TopicSurface::DefaultPublishes,
                Some(hat_id.clone()),
            ));
        }
        // obligations
        for obligation in &hat_config.obligations {
            for topic in &obligation.must_emit_any_of {
                occurrences.insert(TopicOccurrence::new(
                    topic.clone(),
                    TopicSurface::ObligationEmit,
                    Some(hat_id.clone()),
                ));
            }
        }
    }

    // event_loop surfaces
    if let Some(ep) = &config.event_loop.event_policy {
        for topic in ep.schemas.keys() {
            occurrences.insert(TopicOccurrence::new(
                topic.clone(),
                TopicSurface::EventPolicySchema,
                None,
            ));
        }
    }
    for topic in &config.event_loop.required_events {
        occurrences.insert(TopicOccurrence::new(
            topic.clone(),
            TopicSurface::RequiredEvent,
            None,
        ));
    }
    if let Some(topic) = &config.event_loop.starting_event {
        occurrences.insert(TopicOccurrence::new(
            topic.clone(),
            TopicSurface::StartingEvent,
            None,
        ));
    }
    if !config.event_loop.cancellation_promise.is_empty() {
        occurrences.insert(TopicOccurrence::new(
            config.event_loop.cancellation_promise.clone(),
            TopicSurface::CancellationPromise,
            None,
        ));
    }
    // completion_promise is always present (has a default).
    occurrences.insert(TopicOccurrence::new(
        config.event_loop.completion_promise.clone(),
        TopicSurface::CompletionPromise,
        None,
    ));
    if let Some(vg) = &config.event_loop.verdict_gate {
        occurrences.insert(TopicOccurrence::new(
            vg.topic.clone(),
            TopicSurface::VerdictGate,
            None,
        ));
    }
    if let Some(wg) = &config.event_loop.workflow_guards {
        for chain in &wg.chains {
            for topic in &chain.topics {
                occurrences.insert(TopicOccurrence::new(
                    topic.clone(),
                    TopicSurface::WorkflowGuardTopic,
                    None,
                ));
            }
        }
    }

    // topic_owners keys
    for topic in config.topic_owners.keys() {
        occurrences.insert(TopicOccurrence::new(
            topic.clone(),
            TopicSurface::OwnershipKey,
            None,
        ));
    }

    occurrences.into_iter().collect()
}

/// Validate all topics in the config against the format rules.
///
/// Returns a `TopicFormatResult` for each unique topic token. The
/// results are sorted by token for deterministic output.
pub fn validate_all_topics(config: &RalphConfig) -> Vec<TopicFormatResult> {
    let occurrences = enumerate_topics(config);
    let mut seen: BTreeMap<String, TopicFormatResult> = BTreeMap::new();

    for occ in &occurrences {
        if !seen.contains_key(&occ.topic) {
            let result = validate_topic_format(&occ.topic, &config.topic_format_whitelist);
            seen.insert(occ.topic.clone(), result);
        }
    }

    seen.into_values().collect()
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Topic format validation ────────────────────────────────────────

    #[test]
    fn valid_lowercase_dot_case() {
        assert!(is_valid_lowercase_dot_case("work.start"));
        assert!(is_valid_lowercase_dot_case("review.done"));
        assert!(is_valid_lowercase_dot_case("task.created"));
        assert!(is_valid_lowercase_dot_case("a"));
        assert!(is_valid_lowercase_dot_case("a.b.c"));
        assert!(is_valid_lowercase_dot_case("x1.y2"));
    }

    #[test]
    fn invalid_uppercase() {
        assert!(!is_valid_lowercase_dot_case("REVIEW_COMPLETE"));
        assert!(!is_valid_lowercase_dot_case("Work.Start"));
    }

    #[test]
    fn invalid_underscores() {
        assert!(!is_valid_lowercase_dot_case("work_start"));
    }

    #[test]
    fn invalid_starting_digit() {
        assert!(!is_valid_lowercase_dot_case("123start"));
    }

    #[test]
    fn invalid_empty() {
        assert!(!is_valid_lowercase_dot_case(""));
    }

    #[test]
    fn invalid_consecutive_dots() {
        assert!(!is_valid_lowercase_dot_case("a..b"));
    }

    #[test]
    fn invalid_trailing_dot() {
        assert!(!is_valid_lowercase_dot_case("a."));
    }

    #[test]
    fn invalid_leading_dot() {
        assert!(!is_valid_lowercase_dot_case(".a"));
    }

    // ── Whitelist ──────────────────────────────────────────────────────

    #[test]
    fn whitelist_exempt() {
        let whitelist = vec!["LOOP_COMPLETE".to_string(), "REVIEW_COMPLETE".to_string()];
        let result = validate_topic_format("LOOP_COMPLETE", &whitelist);
        assert!(result.is_valid);
        assert!(result.is_whitelisted);
        assert!(result.suggestion.is_none());
    }

    #[test]
    fn non_whitelisted_invalid_returns_suggestion() {
        let whitelist = vec!["LOOP_COMPLETE".to_string()];
        let result = validate_topic_format("REVIEW_COMPLETE", &whitelist);
        assert!(!result.is_valid);
        assert!(!result.is_whitelisted);
        assert_eq!(result.suggestion.as_deref(), Some("review.complete"));
    }

    #[test]
    fn valid_token_not_whitelisted_is_valid() {
        let result = validate_topic_format("work.start", &[]);
        assert!(result.is_valid);
        assert!(!result.is_whitelisted);
        assert!(result.suggestion.is_none());
    }

    // ── Suggestion generation ──────────────────────────────────────────

    #[test]
    fn suggest_uppercase_to_dot_case() {
        assert_eq!(suggest_topic_fix("REVIEW_COMPLETE"), "review.complete");
    }

    #[test]
    fn suggest_camel_case() {
        assert_eq!(suggest_topic_fix("camelCase"), "camel.case");
    }

    #[test]
    fn suggest_leading_digit() {
        assert_eq!(suggest_topic_fix("123start"), "a123start");
    }

    #[test]
    fn suggest_underscores_to_dots() {
        assert_eq!(suggest_topic_fix("work_start"), "work.start");
    }

    #[test]
    fn suggest_empty_string() {
        assert_eq!(suggest_topic_fix(""), "invalid");
    }

    // ── Topic surface enumeration ──────────────────────────────────────

    #[test]
    fn enumerate_empty_config() {
        let config = RalphConfig::default();
        let occurrences = enumerate_topics(&config);
        // Should at least have the completion_promise default.
        assert!(!occurrences.is_empty());
        assert!(
            occurrences
                .iter()
                .any(|o| o.surface == TopicSurface::CompletionPromise)
        );
    }

    #[test]
    fn enumerate_hats_topics() {
        let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done", "work.failed"]
    default_publishes: "work.done"
  reviewer:
    name: "Reviewer"
    triggers: ["work.done"]
    publishes: ["review.complete"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let occurrences = enumerate_topics(&config);

        // Check triggers
        assert!(occurrences.iter().any(|o| {
            o.topic == "work.ready"
                && o.surface == TopicSurface::Triggers
                && o.hat.as_deref() == Some("executor")
        }));
        // Check publishes
        assert!(occurrences.iter().any(|o| {
            o.topic == "work.done"
                && o.surface == TopicSurface::Publishes
                && o.hat.as_deref() == Some("executor")
        }));
        // Check default_publishes
        assert!(occurrences.iter().any(|o| {
            o.topic == "work.done"
                && o.surface == TopicSurface::DefaultPublishes
                && o.hat.as_deref() == Some("executor")
        }));
        // Check starting_event
        assert!(
            occurrences
                .iter()
                .any(|o| { o.topic == "work.start" && o.surface == TopicSurface::StartingEvent })
        );
        // Check completion_promise
        assert!(occurrences.iter().any(|o| {
            o.topic == "LOOP_COMPLETE" && o.surface == TopicSurface::CompletionPromise
        }));
    }

    #[test]
    fn enumerate_topic_owners() {
        let yaml = r#"
topic_owners:
  work.done:
    - executor
  review.complete:
    - reviewer
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let occurrences = enumerate_topics(&config);
        assert!(
            occurrences
                .iter()
                .any(|o| { o.topic == "work.done" && o.surface == TopicSurface::OwnershipKey })
        );
    }

    // ── validate_all_topics ────────────────────────────────────────────

    #[test]
    fn validate_all_topics_empty_config() {
        let config = RalphConfig::default();
        let results = validate_all_topics(&config);
        // LOOP_COMPLETE is in the default config; it should be valid
        // (not whitelisted by default, but it IS lowercase dot-case? No,
        // LOOP_COMPLETE has uppercase — it will be invalid unless whitelisted).
        let loop_complete = results.iter().find(|r| r.token == "LOOP_COMPLETE");
        assert!(loop_complete.is_some());
        assert!(!loop_complete.unwrap().is_valid);
        assert_eq!(
            loop_complete.unwrap().suggestion.as_deref(),
            Some("loop.complete")
        );
    }

    #[test]
    fn validate_all_topics_with_whitelist() {
        let yaml = r#"
topic_format_whitelist:
  - LOOP_COMPLETE
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let results = validate_all_topics(&config);
        let loop_complete = results.iter().find(|r| r.token == "LOOP_COMPLETE");
        assert!(loop_complete.is_some());
        assert!(loop_complete.unwrap().is_valid);
        assert!(loop_complete.unwrap().is_whitelisted);
    }

    // ── TopicSurface label ─────────────────────────────────────────────

    #[test]
    fn topic_surface_labels_are_stable() {
        assert_eq!(TopicSurface::Triggers.label(), "triggers");
        assert_eq!(TopicSurface::Publishes.label(), "publishes");
        assert_eq!(TopicSurface::DefaultPublishes.label(), "default_publishes");
        assert_eq!(
            TopicSurface::ObligationEmit.label(),
            "obligations.must_emit_any_of"
        );
        assert_eq!(
            TopicSurface::EventPolicySchema.label(),
            "event_policy.schemas"
        );
        assert_eq!(TopicSurface::RequiredEvent.label(), "required_events");
        assert_eq!(TopicSurface::StartingEvent.label(), "starting_event");
        assert_eq!(
            TopicSurface::CancellationPromise.label(),
            "cancellation_promise"
        );
        assert_eq!(
            TopicSurface::CompletionPromise.label(),
            "completion_promise"
        );
        assert_eq!(TopicSurface::VerdictGate.label(), "verdict_gate");
        assert_eq!(
            TopicSurface::WorkflowGuardTopic.label(),
            "workflow_guards.topics"
        );
        assert_eq!(TopicSurface::OwnershipKey.label(), "topic_owners");
    }

    // ── Serde round-trip for new config fields ─────────────────────────

    #[test]
    fn topic_owners_and_whitelist_default_to_empty() {
        let config = RalphConfig::default();
        assert!(config.topic_owners.is_empty());
        assert!(config.topic_format_whitelist.is_empty());
    }

    #[test]
    fn topic_owners_and_whitelist_parse_from_yaml() {
        let yaml = r#"
topic_owners:
  work.done:
    - executor
  review.complete:
    - reviewer
topic_format_whitelist:
  - LOOP_COMPLETE
  - REVIEW_COMPLETE
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.topic_owners.len(), 2);
        assert_eq!(
            config.topic_owners.get("work.done").unwrap(),
            &vec!["executor".to_string()]
        );
        assert_eq!(config.topic_format_whitelist.len(), 2);
        assert!(
            config
                .topic_format_whitelist
                .contains(&"LOOP_COMPLETE".to_string())
        );
    }

    #[test]
    fn topic_owners_and_whitelist_survive_roundtrip() {
        let yaml = r#"
topic_owners:
  work.done:
    - executor
topic_format_whitelist:
  - LOOP_COMPLETE
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let serialized = serde_yaml::to_string(&config).unwrap();
        let deserialized: RalphConfig = serde_yaml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.topic_owners, config.topic_owners);
        assert_eq!(
            deserialized.topic_format_whitelist,
            config.topic_format_whitelist
        );
    }

    // ── Finding ID constants ───────────────────────────────────────────

    #[test]
    fn finding_ids_are_stable() {
        assert_eq!(FINDING_INVALID_TOPIC_FORMAT, "preset.invalid_topic_format");
        assert_eq!(
            FINDING_WHITELIST_EXEMPT_TOPIC,
            "preset.whitelist_exempt_topic"
        );
    }
}
