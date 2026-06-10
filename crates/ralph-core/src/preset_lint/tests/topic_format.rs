//! U1: Topic format validation, enumeration, and finding ID tests.
//!
//! 26 tests covering:
//! - `is_valid_lowercase_dot_case` (8 tests)
//! - whitelist behavior (3 tests)
//! - `suggest_topic_fix` (5 tests)
//! - `enumerate_topics` (3 tests)
//! - `validate_all_topics` (2 tests)
//! - `TopicSurface::label` (1 test)
//! - config field defaults / serde round-trip (3 tests)
//! - finding ID constants (1 test)

use super::*;
use crate::config::RalphConfig;
use crate::preset_lint::topic_format::is_valid_lowercase_dot_case;

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
    assert!(
        occurrences.iter().any(|o| {
            o.topic == "LOOP_COMPLETE" && o.surface == TopicSurface::CompletionPromise
        })
    );
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
