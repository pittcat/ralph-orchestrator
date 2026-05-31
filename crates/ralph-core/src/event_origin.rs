//! Event origin validation for JSONL ingestion.
//!
//! Ralph treats JSONL output as untrusted until validated. This module provides
//! the shared validation predicate used by both regular event processing and wave
//! dispatch event processing.
//!
//! # Security Model
//!
//! Once a line is read from JSONL, Ralph treats it as untrusted agent output until
//! provenance and scope checks accept it. This guards against LLM-generated fake
//! events (unregistered hat names, out-of-scope topics, forged timestamps).

use crate::event_reader::Event as JsonlEvent;
use crate::hat_registry::HatRegistry;
use ralph_proto::HatId;
use tracing::{debug, warn};

/// Topics that are allowed from JSONL without hat provenance.
///
/// These are orchestration control topics that agents legitimately emit through
/// the event file (human.interact for blocking questions, task.resume for recovery).
/// They are not preset business topics and do not require hat publish scope.
///
/// Note: `event.malformed`, `event.scope_violation`, and similar diagnostics are
/// created by Ralph code and published directly to the bus — they should not need
/// to pass as trusted JSONL events.
fn is_jsonl_control_topic(topic: &str, cancellation_topic: &str) -> bool {
    matches!(
        topic,
        "human.interact" | "human.guidance" | "task.resume" | "build.task.abandoned"
    ) || (cancellation_topic == topic && !cancellation_topic.is_empty())
}

/// Result of origin validation for a JSONL event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginCheck {
    /// Event passed all origin checks and may proceed.
    Accepted,
    /// Event failed origin checks and should be dropped.
    Rejected {
        topic: String,
        hat: Option<String>,
        reason: &'static str,
    },
}

/// Validates whether a JSONL event is allowed to enter the trusted orchestration pipeline.
///
/// This predicate is shared by both regular event processing and wave dispatch
/// processing. It implements the following rules:
///
/// - **Registry empty (solo mode)**: All events are accepted (permissive for hatless baseline).
/// - **Control topics**: No-hat events matching known control topics are accepted.
/// - **Registered hat + can_publish**: Events with a registered hat that can publish
///   the topic are accepted.
/// - **Unknown hat**: Always rejected (fail-closed).
/// - **No-hat business event in hat-based run**: Rejected.
///
/// # Arguments
///
/// * `event` — parsed JSONL event
/// * `registry` — hat registry (empty for solo mode)
/// * `cancellation_topic` — configured cancellation topic (e.g. "loop.cancel")
pub fn validate_event_origin(
    event: &JsonlEvent,
    registry: &HatRegistry,
    cancellation_topic: &str,
) -> OriginCheck {
    let topic_str = event.topic.as_str();

    // Solo / hatless mode: be permissive for business events
    if registry.is_empty() {
        return OriginCheck::Accepted;
    }

    // Control topics: accept no-hat events for recognized orchestration controls
    if event.hat.is_none() && is_jsonl_control_topic(topic_str, cancellation_topic) {
        return OriginCheck::Accepted;
    }

    // No-hat business event in a hat-based registry: reject
    if event.hat.is_none() {
        return OriginCheck::Rejected {
            topic: topic_str.to_string(),
            hat: None,
            reason: "no-hat business event rejected in hat-based mode",
        };
    }

    let hat_id = HatId::new(event.hat.as_ref().unwrap());

    // Unknown hat: always reject
    if registry.get(&hat_id).is_none() {
        warn!(
            topic = %topic_str,
            hat = %event.hat.as_ref().unwrap(),
            "Unknown hat rejected via origin guard"
        );
        return OriginCheck::Rejected {
            topic: topic_str.to_string(),
            hat: event.hat.clone(),
            reason: "unknown hat rejected",
        };
    }

    // Registered hat: check publish scope
    if !registry.can_publish(&hat_id, topic_str) {
        warn!(
            topic = %topic_str,
            hat = %event.hat.as_ref().unwrap(),
            "Out-of-scope event rejected by origin guard"
        );
        return OriginCheck::Rejected {
            topic: topic_str.to_string(),
            hat: event.hat.clone(),
            reason: "out-of-scope topic for declared hat",
        };
    }

    // Control topics from registered hats: accept even if not in publishes
    // (the allowlist check above already passed for no-hat, but registered hats
    // should also be able to emit control topics without boilerplate in every preset)
    if is_jsonl_control_topic(topic_str, cancellation_topic) {
        return OriginCheck::Accepted;
    }

    OriginCheck::Accepted
}

/// Filters a batch of JSONL events through origin validation.
///
/// Returns only the events that passed validation. Logs rejections at warn level.
pub fn filter_events_by_origin(
    events: Vec<JsonlEvent>,
    registry: &HatRegistry,
    cancellation_topic: &str,
) -> Vec<JsonlEvent> {
    let mut accepted = Vec::with_capacity(events.len());
    for event in events {
        match validate_event_origin(&event, registry, cancellation_topic) {
            OriginCheck::Accepted => accepted.push(event),
            OriginCheck::Rejected { topic, hat, reason } => {
                debug!(
                    topic = %topic,
                    hat = ?hat,
                    reason = reason,
                    "JSONL event rejected by origin guard"
                );
            }
        }
    }
    accepted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RalphConfig;

    fn make_event(topic: &str, hat: Option<&str>) -> JsonlEvent {
        JsonlEvent {
            topic: topic.to_string(),
            payload: None,
            ts: "2024-01-01T00:00:00Z".to_string(),
            hat: hat.map(|s| s.to_string()),
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
        }
    }

    fn registry_with_hats(yaml: &str) -> HatRegistry {
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        HatRegistry::from_config(&config)
    }

    #[test]
    fn test_solo_mode_accepts_all() {
        let registry = registry_with_hats("");
        let event = make_event("LOOP_COMPLETE", None);
        assert_eq!(
            validate_event_origin(&event, &registry, ""),
            OriginCheck::Accepted
        );
    }

    #[test]
    fn test_unknown_hat_rejected() {
        let registry = registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
"#,
        );
        let event = make_event("experiment.planned", Some("strategist"));
        assert_eq!(
            validate_event_origin(&event, &registry, ""),
            OriginCheck::Rejected {
                topic: "experiment.planned".to_string(),
                hat: Some("strategist".to_string()),
                reason: "unknown hat rejected"
            }
        );
    }

    #[test]
    fn test_no_hat_business_event_rejected_in_hated_mode() {
        let registry = registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
"#,
        );
        let event = make_event("debug.step", None);
        assert_eq!(
            validate_event_origin(&event, &registry, ""),
            OriginCheck::Rejected {
                topic: "debug.step".to_string(),
                hat: None,
                reason: "no-hat business event rejected in hat-based mode"
            }
        );
    }

    #[test]
    fn test_registered_hat_valid_publish_accepted() {
        let registry = registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
"#,
        );
        let event = make_event("work.done", Some("executor"));
        assert_eq!(
            validate_event_origin(&event, &registry, ""),
            OriginCheck::Accepted
        );
    }

    #[test]
    fn test_registered_hat_out_of_scope_rejected() {
        let registry = registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
"#,
        );
        let event = make_event("build.done", Some("executor"));
        assert_eq!(
            validate_event_origin(&event, &registry, ""),
            OriginCheck::Rejected {
                topic: "build.done".to_string(),
                hat: Some("executor".to_string()),
                reason: "out-of-scope topic for declared hat"
            }
        );
    }

    #[test]
    fn test_no_hat_human_interact_accepted() {
        let registry = registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
"#,
        );
        let event = make_event("human.interact", None);
        assert_eq!(
            validate_event_origin(&event, &registry, ""),
            OriginCheck::Accepted
        );
    }

    #[test]
    fn test_no_hat_task_resume_accepted() {
        let registry = registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
"#,
        );
        let event = make_event("task.resume", None);
        assert_eq!(
            validate_event_origin(&event, &registry, ""),
            OriginCheck::Accepted
        );
    }

    #[test]
    fn test_unknown_hat_human_interact_rejected() {
        let registry = registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
"#,
        );
        // Even though human.interact is a control topic, an unknown hat
        // trying to emit it should be rejected.
        let event = make_event("human.interact", Some("strategist"));
        assert_eq!(
            validate_event_origin(&event, &registry, ""),
            OriginCheck::Rejected {
                topic: "human.interact".to_string(),
                hat: Some("strategist".to_string()),
                reason: "unknown hat rejected"
            }
        );
    }

    #[test]
    fn test_configured_cancellation_topic_accepted_no_hat() {
        let registry = registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
"#,
        );
        let event = make_event("loop.cancel", None);
        assert_eq!(
            validate_event_origin(&event, &registry, "loop.cancel"),
            OriginCheck::Accepted
        );
    }

    #[test]
    fn test_filter_events_by_origin_drops_rejected() {
        let registry = registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
"#,
        );
        let events = vec![
            make_event("work.done", Some("executor")), // accepted
            make_event("debug.step", None), // rejected no-hat
            make_event("work.done", Some("strategist")), // rejected unknown hat
        ];
        let filtered = filter_events_by_origin(events, &registry, "");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].topic, "work.done");
    }
}