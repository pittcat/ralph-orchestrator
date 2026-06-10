//! U1: Topic format validation, surface enumeration, and shared types.
//!
//! This module owns:
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

use std::collections::{BTreeMap, BTreeSet};

use crate::config::RalphConfig;

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
pub(super) fn is_valid_lowercase_dot_case(token: &str) -> bool {
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
