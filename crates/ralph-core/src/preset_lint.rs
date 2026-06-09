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
// U2: Ownership & coordinator finding IDs
// ──────────────────────────────────────────────────────────────────────────

/// `topic_owners` references a hat that does not exist in the config.
///
/// Always `Error` severity (regardless of strict mode).
pub const FINDING_OWNER_UNKNOWN_HAT: &str = "preset.owner_unknown_hat";

/// The owner hat of a topic does not declare that topic in its
/// `publishes` or `default_publishes`.
///
/// `Warn` in default mode, `Error` in strict.
pub const FINDING_OWNER_NOT_PUBLISHER: &str = "preset.owner_not_publisher";

/// A non-owner hat publishes a topic that has a declared owner.
///
/// `Warn` in default mode, `Error` in strict.
pub const FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH: &str = "preset.cross_hat_unauthorized_publish";

/// A topic is declared in `topic_owners` but no hat publishes it.
///
/// `Warn` in default mode, `Error` in strict.
pub const FINDING_MISSING_TOPIC_OWNER: &str = "preset.missing_topic_owner";

/// `tasks.enabled=true` but `tasks.coordinator_hats` is empty.
///
/// Always `Error` severity.
pub const FINDING_COORDINATOR_MISSING: &str = "preset.coordinator_missing";

/// A hat publishes a `task.*` topic but is not listed in
/// `tasks.coordinator_hats`.
///
/// Always `Error` severity.
pub const FINDING_TASK_PUBLISHER_NOT_COORDINATED: &str = "preset.task_publisher_not_coordinated";

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

// ──────────────────────────────────────────────────────────────────────────
// U2: Ownership & coordinator static rules
// ──────────────────────────────────────────────────────────────────────────

/// Severity override for strict mode (U2 checks that are warn-by-default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintStrictness {
    /// Default mode: ownership warnings remain warnings.
    Default,
    /// Strict mode: ownership warnings become errors.
    Strict,
}

impl LintStrictness {
    /// Returns the severity to use for checks that are warn-by-default.
    pub fn ownership_severity(self) -> &'static str {
        match self {
            Self::Default => "warn",
            Self::Strict => "error",
        }
    }
}

/// Result of a single U2 ownership / coordinator check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintFinding {
    /// Stable machine finding ID (e.g. `preset.owner_unknown_hat`).
    pub id: &'static str,
    /// Severity: `"error"`, `"warn"`, or `"pass"`.
    pub severity: String,
    /// Human-readable summary.
    pub message: String,
    /// Optional topic involved.
    pub topic: Option<String>,
    /// Optional hat involved.
    pub hat: Option<String>,
    /// Optional owner hat.
    pub owner: Option<String>,
    /// Optional fix hint.
    pub action_hint: Option<String>,
}

impl LintFinding {
    fn error(id: &'static str, message: impl Into<String>) -> Self {
        Self {
            id,
            severity: "error".to_string(),
            message: message.into(),
            topic: None,
            hat: None,
            owner: None,
            action_hint: None,
        }
    }

    fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    fn with_hat(mut self, hat: impl Into<String>) -> Self {
        self.hat = Some(hat.into());
        self
    }

    fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    fn with_action_hint(mut self, hint: impl Into<String>) -> Self {
        self.action_hint = Some(hint.into());
        self
    }
}

/// Check R2: Every owner hat referenced in `topic_owners` must exist
/// in the config's hat map.
///
/// Returns `Error` findings for unknown hats.
pub fn check_owner_references(config: &RalphConfig) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    for (topic, owners) in &config.topic_owners {
        for owner in owners {
            if !config.hats.contains_key(owner) {
                findings.push(
                    LintFinding::error(
                        FINDING_OWNER_UNKNOWN_HAT,
                        format!(
                            "topic_owners[\"{topic}\"] references unknown hat \"{owner}\"; \
                             add a hat definition or remove the owner entry"
                        ),
                    )
                    .with_topic(topic)
                    .with_owner(owner)
                    .with_action_hint(format!("Add hat \"{owner}\" to the hats section")),
                );
            }
        }
    }

    findings
}

/// Collect all topics a hat explicitly publishes (via `publishes` or
/// `default_publishes`).
fn hat_publishes(hat_config: &crate::config::HatConfig) -> Vec<String> {
    let mut topics: Vec<String> = hat_config.publishes.clone();
    if let Some(dp) = &hat_config.default_publishes {
        topics.push(dp.clone());
    }
    topics
}

/// Check R2 + R3: Owner hats must publish their owned topic, and
/// non-owner hats must not publish owner topics.
///
/// In strict mode, all warnings become errors.
pub fn check_ownership_rules(config: &RalphConfig, strictness: LintStrictness) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    for (topic, owners) in &config.topic_owners {
        // Build set of hats that publish this topic.
        let publishers: Vec<&str> = config
            .hats
            .iter()
            .filter(|(_, hat)| {
                let publishes = hat_publishes(hat);
                publishes.iter().any(|p| p == topic)
            })
            .map(|(hat_id, _)| hat_id.as_str())
            .collect();

        // R2: Each owner must publish the topic.
        for owner in owners {
            if !publishers.iter().any(|p| *p == owner) {
                let severity = strictness.ownership_severity();
                findings.push(LintFinding {
                    id: FINDING_OWNER_NOT_PUBLISHER,
                    severity: severity.to_string(),
                    message: format!(
                        "hat \"{owner}\" is the declared owner of topic \"{topic}\" \
                             but does not publish it; add \"{topic}\" to its publishes \
                             or default_publishes"
                    ),
                    topic: Some(topic.clone()),
                    hat: Some(owner.clone()),
                    owner: Some(owner.clone()),
                    action_hint: Some(format!("Add \"{topic}\" to hat \"{owner}\" publishes list")),
                });
            }
        }

        // R3: Non-owner hats publishing owner topic produce unauthorized publish.
        let owner_set: std::collections::HashSet<&str> =
            owners.iter().map(|s| s.as_str()).collect();
        for publisher in &publishers {
            if !owner_set.contains(*publisher) {
                let severity = strictness.ownership_severity();
                findings.push(LintFinding {
                    id: FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH,
                    severity: severity.to_string(),
                    message: format!(
                        "hat \"{publisher}\" publishes topic \"{topic}\" which is \
                             owned by [{}]; non-owner publishing is not allowed",
                        owners.join(", ")
                    ),
                    topic: Some(topic.clone()),
                    hat: Some(publisher.to_string()),
                    owner: Some(owners.join(", ")),
                    action_hint: Some(format!(
                        "Remove \"{topic}\" from hat \"{publisher}\" publishes, \
                             or add \"{publisher}\" as an owner"
                    )),
                });
            }
        }
    }

    findings
}

/// Check R5: When `tasks.enabled=true`, `coordinator_hats` must be
/// non-empty, and every hat that publishes a `task.*` topic must be
/// listed in `coordinator_hats`.
///
/// Always returns `Error` severity findings.
pub fn check_coordinator_rules(config: &RalphConfig) -> Vec<LintFinding> {
    let mut findings = Vec::new();

    if !config.tasks.enabled {
        return findings;
    }

    // R5a: coordinator_hats must be non-empty.
    if config.tasks.coordinator_hats.is_empty() {
        // Collect candidate hats that publish task.* topics.
        let candidates: Vec<&str> = config
            .hats
            .iter()
            .filter(|(_, hat)| {
                let publishes = hat_publishes(hat);
                publishes.iter().any(|p| p.starts_with("task."))
            })
            .map(|(hat_id, _)| hat_id.as_str())
            .collect();

        let hint = if candidates.is_empty() {
            "Add coordinator_hats to the tasks section".to_string()
        } else {
            format!(
                "Add coordinator_hats: [{}] to the tasks section",
                candidates.join(", ")
            )
        };

        findings.push(
            LintFinding::error(
                FINDING_COORDINATOR_MISSING,
                "tasks.enabled is true but tasks.coordinator_hats is empty; \
                 at least one coordinator hat is required",
            )
            .with_action_hint(hint),
        );

        // Don't check task publishers if coordinator is empty —
        // the error above is sufficient and more actionable.
        return findings;
    }

    let coordinator_set: std::collections::HashSet<&str> = config
        .tasks
        .coordinator_hats
        .iter()
        .map(|s| s.as_str())
        .collect();

    // R5b: Every hat publishing task.* must be in coordinator_hats.
    for (hat_id, hat_config) in &config.hats {
        let publishes = hat_publishes(hat_config);
        let has_task_topic = publishes.iter().any(|p| p.starts_with("task."));
        if has_task_topic && !coordinator_set.contains(hat_id.as_str()) {
            let task_topics: Vec<&str> = publishes
                .iter()
                .filter(|p| p.starts_with("task."))
                .map(|s| s.as_str())
                .collect();
            findings.push(
                LintFinding::error(
                    FINDING_TASK_PUBLISHER_NOT_COORDINATED,
                    format!(
                        "hat \"{hat_id}\" publishes task topics [{}] but is not \
                         listed in tasks.coordinator_hats",
                        task_topics.join(", ")
                    ),
                )
                .with_hat(hat_id)
                .with_action_hint(format!("Add \"{hat_id}\" to tasks.coordinator_hats")),
            );
        }
    }

    findings
}

/// Run all U2 ownership and coordinator checks.
///
/// Returns a sorted, deterministic list of findings.
pub fn validate_ownership_and_coordinator(
    config: &RalphConfig,
    strictness: LintStrictness,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    findings.extend(check_owner_references(config));
    findings.extend(check_ownership_rules(config, strictness));
    findings.extend(check_coordinator_rules(config));

    // Sort by (id, topic, hat) for deterministic output.
    findings.sort_by(|a, b| {
        a.id.cmp(b.id)
            .then(a.topic.cmp(&b.topic))
            .then(a.hat.cmp(&b.hat))
    });

    findings
}

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

    // ── U2: Ownership & coordinator static rules ───────────────────────

    // T1: owner references unknown hat → always error.
    #[test]
    fn owner_unknown_hat_always_error() {
        let yaml = r#"
topic_owners:
  work.done:
    - non_existent_hat
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = check_owner_references(&config);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_OWNER_UNKNOWN_HAT);
        assert_eq!(findings[0].severity, "error");
        assert_eq!(findings[0].topic.as_deref(), Some("work.done"));
        assert_eq!(findings[0].owner.as_deref(), Some("non_existent_hat"));
    }

    // T2: owner does not publish its topic → warn in default, error in strict.
    #[test]
    fn owner_not_publisher_warn_default() {
        let yaml = r#"
topic_owners:
  work.done:
    - executor
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    # Does NOT publish work.done
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = check_ownership_rules(&config, LintStrictness::Default);
        let f = findings
            .iter()
            .find(|f| f.id == FINDING_OWNER_NOT_PUBLISHER);
        assert!(f.is_some(), "expected owner_not_publisher finding");
        assert_eq!(f.unwrap().severity, "warn");
    }

    #[test]
    fn owner_not_publisher_error_strict() {
        let yaml = r#"
topic_owners:
  work.done:
    - executor
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = check_ownership_rules(&config, LintStrictness::Strict);
        let f = findings
            .iter()
            .find(|f| f.id == FINDING_OWNER_NOT_PUBLISHER);
        assert!(f.is_some());
        assert_eq!(f.unwrap().severity, "error");
    }

    // T3: non-owner publishes owner topic → warn in default, error in strict.
    #[test]
    fn cross_hat_unauthorized_publish_warn_default() {
        let yaml = r#"
topic_owners:
  work.done:
    - executor
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
  reviewer:
    name: "Reviewer"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = check_ownership_rules(&config, LintStrictness::Default);
        let f = findings
            .iter()
            .find(|f| f.id == FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH);
        assert!(
            f.is_some(),
            "expected cross_hat_unauthorized_publish finding"
        );
        assert_eq!(f.unwrap().severity, "warn");
        assert_eq!(f.unwrap().hat.as_deref(), Some("reviewer"));
    }

    #[test]
    fn cross_hat_unauthorized_publish_error_strict() {
        let yaml = r#"
topic_owners:
  work.done:
    - executor
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
  reviewer:
    name: "Reviewer"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = check_ownership_rules(&config, LintStrictness::Strict);
        let f = findings
            .iter()
            .find(|f| f.id == FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH);
        assert!(f.is_some());
        assert_eq!(f.unwrap().severity, "error");
    }

    // T4: no owner declared → no findings (missing_topic_owner not triggered
    //     unless topic_owners has an entry with no publisher).
    #[test]
    fn no_owner_topics_no_findings() {
        let yaml = r#"
tasks:
  enabled: false
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = validate_ownership_and_coordinator(&config, LintStrictness::Default);
        assert!(findings.is_empty(), "no ownership findings expected");
    }

    // T5: tasks disabled → no coordinator findings.
    #[test]
    fn tasks_disabled_no_coordinator_findings() {
        let yaml = r#"
tasks:
  enabled: false
hats:
  executor:
    name: "Executor"
    publishes: ["task.created"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = check_coordinator_rules(&config);
        assert!(
            findings.is_empty(),
            "no coordinator findings when tasks disabled"
        );
    }

    // T6: tasks enabled + empty coordinator_hats → error.
    #[test]
    fn tasks_enabled_empty_coordinator_error() {
        let yaml = r#"
tasks:
  enabled: true
  coordinator_hats: []
hats:
  executor:
    name: "Executor"
    publishes: ["task.created"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = check_coordinator_rules(&config);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_COORDINATOR_MISSING);
        assert_eq!(findings[0].severity, "error");
        // The hint should list candidate hats.
        assert!(
            findings[0]
                .action_hint
                .as_deref()
                .unwrap()
                .contains("executor")
        );
    }

    // T7: task publisher not in coordinator_hats → error with candidate list.
    #[test]
    fn task_publisher_not_coordinated() {
        let yaml = r#"
tasks:
  enabled: true
  coordinator_hats:
    - plan-gate
hats:
  executor:
    name: "Executor"
    publishes: ["task.created"]
  plan-gate:
    name: "Plan Gate"
    publishes: ["queue.advance"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = check_coordinator_rules(&config);
        let f = findings
            .iter()
            .find(|f| f.id == FINDING_TASK_PUBLISHER_NOT_COORDINATED);
        assert!(
            f.is_some(),
            "expected task_publisher_not_coordinated finding"
        );
        assert_eq!(f.unwrap().hat.as_deref(), Some("executor"));
        assert_eq!(f.unwrap().severity, "error");
    }

    // T8: task publisher IS in coordinator_hats → no error.
    #[test]
    fn task_publisher_coordinated_ok() {
        let yaml = r#"
tasks:
  enabled: true
  coordinator_hats:
    - executor
hats:
  executor:
    name: "Executor"
    publishes: ["task.created"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = check_coordinator_rules(&config);
        assert!(
            findings.is_empty(),
            "no error when task publisher is in coordinator_hats"
        );
    }

    // T9: valid ownership — owner publishes topic, no non-owner publish.
    #[test]
    fn valid_ownership_no_findings() {
        let yaml = r#"
topic_owners:
  work.done:
    - executor
tasks:
  enabled: false
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = validate_ownership_and_coordinator(&config, LintStrictness::Default);
        assert!(
            findings.is_empty(),
            "valid ownership should produce no findings"
        );
    }

    // T10: multiple owners of same topic, all publish → no findings.
    #[test]
    fn multiple_owners_all_publish() {
        let yaml = r#"
topic_owners:
  work.done:
    - executor
    - reviewer
tasks:
  enabled: false
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
  reviewer:
    name: "Reviewer"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = validate_ownership_and_coordinator(&config, LintStrictness::Default);
        assert!(findings.is_empty());
    }

    // T11: finding details (topic, hat, owner) are machine-readable.
    #[test]
    fn finding_details_are_machine_readable() {
        let yaml = r#"
topic_owners:
  work.done:
    - non_existent
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = check_owner_references(&config);
        let f = &findings[0];
        assert!(f.topic.is_some(), "topic field must be present");
        assert!(f.owner.is_some(), "owner field must be present");
        assert!(f.action_hint.is_some(), "action_hint must be present");
    }

    // T12: task.* prefix detection does not误把 wildcard trigger 当 publisher.
    #[test]
    fn task_prefix_only_matches_publishes_not_triggers() {
        let yaml = r#"
tasks:
  enabled: true
  coordinator_hats:
    - plan-gate
hats:
  executor:
    name: "Executor"
    triggers: ["task.created"]  # trigger, not publish
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = check_coordinator_rules(&config);
        // triggers don't count as publishing task.* — no finding expected.
        assert!(
            findings.is_empty(),
            "trigger-only task.* should not produce coordinator finding"
        );
    }

    // T13: LintStrictness ownership_severity returns correct values.
    #[test]
    fn strictness_severity_mapping() {
        assert_eq!(LintStrictness::Default.ownership_severity(), "warn");
        assert_eq!(LintStrictness::Strict.ownership_severity(), "error");
    }

    // T14: validate_ownership_and_coordinator returns deterministic sorted order.
    #[test]
    fn ownership_findings_are_sorted() {
        let yaml = r#"
topic_owners:
  alpha.topic:
    - non_existent_a
  beta.topic:
    - non_existent_b
tasks:
  enabled: false
hats:
  executor:
    name: "Executor"
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let findings = validate_ownership_and_coordinator(&config, LintStrictness::Default);
        // All findings should be sorted by id then topic.
        for window in findings.windows(2) {
            let a = &window[0];
            let b = &window[1];
            assert!(
                (a.id, a.topic.as_deref(), a.hat.as_deref())
                    <= (b.id, b.topic.as_deref(), b.hat.as_deref()),
                "findings not sorted: {a:?} > {b:?}"
            );
        }
    }
}
