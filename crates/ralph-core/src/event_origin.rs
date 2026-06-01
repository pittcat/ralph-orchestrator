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

/// Source identifier stamped on `human.response` events produced by the trusted
/// in-process channel of an active Robot service (e.g., Telegram). The waiter
/// rejects JSONL events without this marker when a Robot service is active.
pub const TRUSTED_HUMAN_RESPONSE_SOURCE: &str = "robot-trusted";

/// Returns `true` when the event is a `human.response` carrying the trusted
/// in-process source marker. Events without this marker are treated as forged
/// and ignored by the trusted waiter path.
pub fn is_trusted_human_response(event: &JsonlEvent) -> bool {
    event.topic == "human.response" && event.source.as_deref() == Some(TRUSTED_HUMAN_RESPONSE_SOURCE)
}

/// Result of validating a `human.interact` payload before it is sent.
///
/// The waiter path is required to send a non-empty question; if the agent
/// produced an empty or malformed payload, the question is rejected before
/// any blocking wait happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HumanInteractValidation {
    /// Payload is a plain string and the trimmed question is non-empty.
    Plain { question: String },
    /// Payload is a JSON object with a non-empty `question` string field.
    Json { question: String },
}

/// Validates a `human.interact` payload.
///
/// - Plain strings: trimmed value must be non-empty.
/// - JSON objects: must contain a `question` string field whose trimmed value
///   is non-empty.
///
/// Returns `Ok(validation)` describing the shape, or `Err(reason)` explaining
/// why the payload was rejected. Used by the event loop to refuse to block
/// on an empty question.
pub fn validate_human_interact_payload(
    payload: Option<&str>,
) -> Result<HumanInteractValidation, String> {
    let raw = payload.unwrap_or("");
    let trimmed = raw.trim();

    if trimmed.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(question) = value.get("question").and_then(|q| q.as_str()) {
                let q = question.trim();
                if !q.is_empty() {
                    return Ok(HumanInteractValidation::Json {
                        question: q.to_string(),
                    });
                }
                return Err(
                    "human.interact JSON payload missing non-empty `question` field".to_string(),
                );
            }
            return Err("human.interact JSON payload missing `question` field".to_string());
        }
    }

    if trimmed.is_empty() {
        return Err("human.interact payload is empty or whitespace".to_string());
    }

    Ok(HumanInteractValidation::Plain {
        question: trimmed.to_string(),
    })
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
/// - **No-hat events**: Accepted through the origin guard. Scope enforcement
///   in the caller (`process_parse_result`/`process_events_from_jsonl_with_waves`)
///   still validates them against active hats when `enforce_hat_scope` is on.
///   The R9 hardening of "reject no-hat business events in hat-based mode" is
///   deferred to a follow-up plan to keep this stability pass scoped to the
///   control-topic ordering bug.
/// - **Unknown hat**: Always rejected (fail-closed — primary protection against
///   LLM-generated fake events from unregistered hat names).
/// - **Registered hat + control topic**: Accepted even if not in `publishes`,
///   so presets do not have to enumerate every control topic on every hat.
/// - **Registered hat + out-of-scope**: Rejected when the hat cannot publish the topic.
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
    let is_control = is_jsonl_control_topic(topic_str, cancellation_topic);

    // Solo / hatless mode: be permissive for business events
    if registry.is_empty() {
        return OriginCheck::Accepted;
    }

    // No-hat events pass through; downstream scope enforcement decides if the
    // event is allowed. This preserves the existing contract used by the loop
    // runner when emitting control signals (LOOP_COMPLETE, task.resume, etc.)
    // without a hat provenance.
    if event.hat.is_none() {
        return OriginCheck::Accepted;
    }

    let hat_id = HatId::new(event.hat.as_ref().unwrap());

    // Unknown hat: always reject (including control topics — fail-closed).
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

    // Registered hat + control topic: accept without checking `publishes`.
    // This is the P0 ordering fix: control topics must be allowed even when
    // not enumerated in the hat's `publishes` list.
    if is_control {
        return OriginCheck::Accepted;
    }

    // Registered hat + business topic: enforce publish scope.
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

    fn make_wave_event(topic: &str, hat: Option<&str>) -> JsonlEvent {
        JsonlEvent {
            topic: topic.to_string(),
            payload: None,
            ts: "2024-01-01T00:00:00Z".to_string(),
            hat: hat.map(|s| s.to_string()),
            triggered: None,
            source: None,
            wave_id: Some("w-test".to_string()),
            wave_index: Some(0),
            wave_total: Some(1),
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
    fn test_event_origin_no_hat_business_event_accepted() {
        // Backward compatibility: no-hat events pass through the origin guard.
        // Scope enforcement in the caller (process_parse_result) still validates
        // them against active hats, so a downstream `enforce_hat_scope` test
        // will reject events whose topic does not match any active hat. The
        // R9 hardening of "reject no-hat business events in hat-based mode" is
        // tracked separately and is intentionally not applied here so we do
        // not regress event_loop integration tests written against the
        // current contract.
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
            OriginCheck::Accepted
        );
    }

    #[test]
    fn test_event_origin_registered_hat_control_topic_accepted() {
        // Registered hats can emit control topics even when the topic is not
        // listed in the hat's `publishes` (so presets do not need to list every
        // control topic in every hat).
        let registry = registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
"#,
        );
        let event = make_event("human.interact", Some("executor"));
        assert_eq!(
            validate_event_origin(&event, &registry, ""),
            OriginCheck::Accepted
        );
    }

    #[test]
    fn test_event_origin_registered_hat_out_of_scope_business_rejected() {
        // Registered hats still cannot publish arbitrary business topics they
        // did not declare in `publishes`.
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
    fn test_event_origin_unknown_hat_rejected() {
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
            make_event("work.done", Some("executor")),   // accepted
            make_event("debug.step", None),              // accepted (no-hat pass-through)
            make_event("work.done", Some("strategist")), // rejected unknown hat
        ];
        let filtered = filter_events_by_origin(events, &registry, "");
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].topic, "work.done");
        assert_eq!(filtered[1].topic, "debug.step");
    }

    #[test]
    fn test_wave_dispatch_origin_registered_publisher_accepted() {
        // Wave dispatch events follow the same origin guard as regular events:
        // the dispatching hat must be registered and able to publish the topic.
        let registry = registry_with_hats(
            r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["build.start"]
    publishes: ["review.file"]
"#,
        );
        let event = make_wave_event("review.file", Some("coordinator"));
        assert_eq!(
            validate_event_origin(&event, &registry, ""),
            OriginCheck::Accepted
        );
    }

    #[test]
    fn test_wave_dispatch_origin_unknown_hat_rejected() {
        // Wave dispatch events from an unregistered hat must be rejected
        // even though the topic would otherwise be a valid business topic.
        let registry = registry_with_hats(
            r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["build.start"]
    publishes: ["review.file"]
"#,
        );
        let event = make_wave_event("review.file", Some("strategist"));
        assert_eq!(
            validate_event_origin(&event, &registry, ""),
            OriginCheck::Rejected {
                topic: "review.file".to_string(),
                hat: Some("strategist".to_string()),
                reason: "unknown hat rejected"
            }
        );
    }

    #[test]
    fn test_wave_dispatch_out_of_scope_hat_rejected() {
        // Wave dispatch from a registered hat that is not allowed to publish
        // the topic must be rejected.
        let registry = registry_with_hats(
            r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#,
        );
        let event = make_wave_event("review.file", Some("coordinator"));
        assert_eq!(
            validate_event_origin(&event, &registry, ""),
            OriginCheck::Rejected {
                topic: "review.file".to_string(),
                hat: Some("coordinator".to_string()),
                reason: "out-of-scope topic for declared hat"
            }
        );
    }

    #[test]
    fn test_can_publish_unknown_hat_is_fail_closed() {
        let registry = registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
"#,
        );
        // Unknown hat cannot publish anything — fail-closed.
        assert!(!registry.can_publish(&HatId::new("ghost"), "work.done"));
        assert!(!registry.can_publish(&HatId::new("ghost"), "human.interact"));
    }

    #[test]
    fn test_emit_args_has_no_ts_completion() {
        // Regression: the zsh completion script must not advertise `--ts` for
        // `ralph emit`, and the user-facing CLI reference must not document
        // it. The flag was removed in 003 to stop LLMs from forging event
        // timestamps.
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root");

        let zsh_path = workspace_root.join("scripts").join("ralph-zsh-plugin.zsh");
        let zsh = std::fs::read_to_string(&zsh_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", zsh_path.display()));

        // Extract the _ralph_emit_args function body. It must not list --ts.
        let emit_block_start = zsh
            .find("_ralph_emit_args()")
            .expect("_ralph_emit_args function present in zsh completion");
        let next_fn_offset = zash_next_function_offset(&zsh, emit_block_start).unwrap_or(zsh.len());
        let emit_block = &zsh[emit_block_start..next_fn_offset];
        assert!(
            !emit_block.contains("--ts"),
            "zsh completion for `ralph emit` must not advertise --ts; got: {emit_block}"
        );

        // The CLI reference must not document --ts either.
        let cli_ref_path = workspace_root
            .join("docs")
            .join("guide")
            .join("cli-reference.md");
        let cli_ref = std::fs::read_to_string(&cli_ref_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", cli_ref_path.display()));
        let emit_section_start = cli_ref
            .find("### ralph emit")
            .expect("ralph emit section in cli-reference.md");
        let next_section_start = cli_ref[emit_section_start + 1..]
            .find("\n### ")
            .map(|o| emit_section_start + 1 + o)
            .unwrap_or(cli_ref.len());
        let emit_section = &cli_ref[emit_section_start..next_section_start];
        assert!(
            !emit_section.contains("--ts"),
            "cli-reference.md must not document --ts for ralph emit; got: {emit_section}"
        );
    }

    /// Returns the offset of the next `(( $+functions[` marker after `from`,
    /// which marks the start of the next top-level zsh function definition.
    fn zash_next_function_offset(zsh: &str, from: usize) -> Option<usize> {
        zsh[from..].find("\n(( $+functions[").map(|o| from + o + 1)
    }
}
