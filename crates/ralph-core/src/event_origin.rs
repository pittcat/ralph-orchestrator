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
/// Control topics that the builtin `ralph` hat is allowed to publish.
///
/// All other topics are treated as business topics and rejected for the `ralph`
/// pseudo-hat.  This prevents the orchestration loop's fallback hat from
/// masquerading as a workflow hat.
pub const RALPH_CONTROL_TOPICS: &[&str] = &[
    "LOOP_COMPLETE",
    "loop.cancel",
    "loop.start",
    "human.interact",
    "human.response",
    "human.guidance",
    // U1: keep parity with `is_jsonl_control_topic` so the ralph-pseudo-hat
    // guard (origin guard and CLI emit guard) agrees on what `ralph` is
    // allowed to publish. `task.resume` is the recovery signal the loop
    // runner injects when a hat stalls — without it the loop would be
    // unable to recover a stalled iteration.
    "task.resume",
];

pub(crate) fn is_jsonl_control_topic(topic: &str, cancellation_topic: &str) -> bool {
    is_orchestrator_control_topic(topic, cancellation_topic)
}

/// Returns `true` when `topic` is a true orchestrator-internal control
/// topic that is produced by Ralph itself, not by an agent.
///
/// These topics bypass the per-hat `can_publish` check **and** the
/// isolated-mode single-event budget because they are *not* agent
/// progress signals — they are orchestrator coordination primitives
/// (recovery, human interaction, cancellation, abandonment).
///
/// **CRITICAL (U3)**: `completion_promise` MUST NOT be in this list.
/// Completion is an agent progress signal (the hat declaring it owns
/// the completion); the loop has dedicated handling downstream of
/// the per-event budget check. Treating it as a control topic here
/// would let any isolated hat bypass its `publishes` scope check
/// and flood the turn with terminal events.
///
/// **P1-2 fix (post-review)**: case-insensitive match on the input topic
/// — lowercase before comparing — so a hat that emits `Loop.Cancel`
/// or `HUMAN.GUIDANCE` (case-mismatched) still falls into the
/// correct diagnostic path (`event.isolation.boundary_violation` for
/// the rejection, not `executor.scope_violation`). The cancellation
/// topic comparison also lowercases `cancellation_topic` to keep the
/// match symmetric.
pub fn is_orchestrator_control_topic(topic: &str, cancellation_topic: &str) -> bool {
    let topic_lc = topic.to_ascii_lowercase();
    let cancellation_lc = cancellation_topic.to_ascii_lowercase();
    matches!(
        topic_lc.as_str(),
        "human.interact" | "human.guidance" | "task.resume" | "build.task.abandoned"
    ) || (!cancellation_lc.is_empty() && topic_lc == cancellation_lc)
}

/// Authoritative set of orchestrator-produced diagnostic topics that the
/// loop itself emits to the bus (and which therefore bypass the per-hat
/// `can_publish` check and the isolated single-event budget).
///
/// P1-1 fix (post-review): replaced the previous `topic.starts_with("event.")`
/// prefix match with an explicit allowlist. The prefix form was
/// defense-in-depth unsafe — it would let any hat that declared a
/// topic with the `event.` prefix (e.g. `event.something_custom`) bypass
/// the per-turn budget gate. The set below mirrors the topics that
/// `loop_state::is_system_topic` already enumerates as "not agent progress".
const ORCHESTRATOR_DIAGNOSTIC_TOPICS: &[&str] = &[
    "event.malformed",
    "event.scope_violation",
    "event.workflow_guard_rejected",
    "event.state_machine.rejected",
    "event.state_machine.ignored",
    "event.state_machine.diagnostic",
    "event.policy_warning",
    "event.completion.blocked",
    "event.completion.ignored",
    "event.isolation.boundary_violation",
    "event.topic_format.rejected",
    "event.execution_contract.rejected",
    "event.payload_contract.rejected",
    "event.step_handoff.gate_rejected",
];

/// Returns `true` when `topic` is an orchestrator-produced diagnostic
/// event that the loop itself emits to the bus.
///
/// These events are observability/audit signals, not hat progress.
/// They bypass the per-hat `can_publish` check and the isolated
/// single-event budget because they are not agent business events.
///
/// P1-1 fix: explicit allowlist (see `ORCHESTRATOR_DIAGNOSTIC_TOPICS`)
/// instead of `event.*` prefix match.
pub fn is_orchestrator_diagnostic_topic(topic: &str) -> bool {
    ORCHESTRATOR_DIAGNOSTIC_TOPICS.contains(&topic)
}

/// 2026-06-18-001 plan U5: 判断 business topic 是否因缺乏 provenance
/// 应被 fail-closed 拒收。
///
/// 在 isolated 模式下,JSONL 读入的事件至少要能追溯到一个注册 hat。
/// 可接受的 provenance 形式(按优先级):
/// 1. `event.hat` 字段**非空**(已注册的交给 scope enforcement,
///    未注册的触发 `isolated_scope_violation` recovery envelope)
/// 2. `event.source` 字段指向注册 hat
/// 3. `event.triggered` 字段指向注册 hat
/// 4. topic 本身就是 orchestrator control / diagnostic
///
/// 不满足以上任意条件且 topic 不是 control/diagnostic 的
/// 事件,被认为"匿名 business topic",需要在 scope enforcement
/// 之前 fail-closed(原因:`isolated_anonymous_business_topic`)。
///
/// 注意:这与 "agent backend 无 hat 但会 fallback 到 `current_isolated_hat`"
/// 放行路径**不冲突**——fallback 仅在 isolated mode +
/// `current_isolated_hat` 存在时启动,这里要求的事件完全没
/// hat/source/triggered 且 topic 是非控制诊断类的"双重缺失"才拦截。
///
/// `current_isolated_hat` 参数:`Some(id)` 表示 isolated 模式当前 hat,
/// 若事件既无 provenance 又存在 current_isolated_hat,scope enforcement
/// 会用 current_isolated_hat 作 fallback —— 这种情形**不**判匿名
/// (与既有 scenario 测试 / mock agent 输出兼容)。`None` 视为
/// 非 isolated 模式,直接放行。
pub fn is_anonymous_business_topic(
    event: &JsonlEvent,
    registered_hats: &HatRegistry,
    cancellation_topic: &str,
    current_isolated_hat: Option<&str>,
) -> bool {
    let topic = event.topic.as_str();
    // 1) 任何 control / diagnostic topic 都不是 business,不需要 provenance
    if is_orchestrator_control_topic(topic, cancellation_topic) {
        return false;
    }
    if is_orchestrator_diagnostic_topic(topic) {
        return false;
    }
    // 2) 非 isolated 模式:不做此 fail-closed 拦截(coordinator 模式按既有逻辑)
    let Some(isolated_hat) = current_isolated_hat else {
        return false;
    };
    // 3) hat 字段存在:交给 scope enforcement
    //    (不论 hat 注册与否,scope_hat = event.hat 路径已被覆盖)
    if let Some(ref hat) = event.hat {
        if !hat.is_empty() {
            return false;
        }
    }
    // 4) source / triggered 字段指向注册 hat → 有 provenance,放行
    if let Some(ref source) = event.source {
        if !source.is_empty()
            && registered_hats.ids().any(|h| h.as_str() == source)
        {
            return false;
        }
    }
    if let Some(ref trig) = event.triggered {
        if !trig.is_empty() && registered_hats.ids().any(|h| h.as_str() == trig) {
            return false;
        }
    }
    // 5) isolated 模式下,若 isolated_hat 是注册 hat(几乎都是),
    //    scope enforcement 会 fallback 到 isolated_hat——属于既有的
    //    "agent backend 无 hat 但 fallback 到 current_isolated_hat"
    //    路径,**不**判匿名。这一拆分避免 scenario 测试 / mock agent
    //    输出被 U5 误杀。
    if !isolated_hat.is_empty()
        && registered_hats.ids().any(|h| h.as_str() == isolated_hat)
    {
        return false;
    }
    // 6) 所有 provenance 字段缺失、isolated_hat 也不在 registry 中、
    //    topic 又是 business → 真正的匿名,拒收
    true
}

/// Source identifier stamped on `human.response` events produced by the trusted
/// in-process channel of an active Robot service (e.g., Telegram). The waiter
/// rejects JSONL events without this marker when a Robot service is active.
pub const TRUSTED_HUMAN_RESPONSE_SOURCE: &str = "robot-trusted";

/// 控制主题判断（P1-12 未来兼容）。
///
/// 除了精确匹配 `RALPH_CONTROL_TOPICS` 中的主题外，任何以 `ralph.`
/// 为前缀的主题也被视为 control topic，这样未来新增控制主题时
/// 旧版本无需更新常量列表即可识别。
pub fn is_ralph_control_topic(topic: &str) -> bool {
    RALPH_CONTROL_TOPICS.contains(&topic) || topic.starts_with("ralph.")
}

/// Returns `true` when the event is a `human.response` carrying the trusted
/// in-process source marker. Events without this marker are treated as forged
/// and ignored by the trusted waiter path.
pub fn is_trusted_human_response(event: &JsonlEvent) -> bool {
    event.topic == "human.response"
        && event.source.as_deref() == Some(TRUSTED_HUMAN_RESPONSE_SOURCE)
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
///   in the caller (`process_parse_result`) still validates them against
///   active hats when `enforce_hat_scope` is on.
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
/// * `completion_promise` — configured completion promise (e.g. "LOOP_COMPLETE")
pub fn validate_event_origin(
    event: &JsonlEvent,
    registry: &HatRegistry,
    cancellation_topic: &str,
    _completion_promise: &str,
) -> OriginCheck {
    let topic_str = event.topic.as_str();
    let is_control = is_jsonl_control_topic(topic_str, cancellation_topic);

    // Solo / hatless mode: be permissive for business events
    if registry.is_empty() {
        return OriginCheck::Accepted;
    }

    // No-hat events pass through the origin guard; downstream scope enforcement
    // in `process_parse_result` validates them against active hats when
    // `enforce_hat_scope` is on. Events parsed from agent output are inherently
    // no-hat (they come from text parsing), so rejecting them here would break
    // the primary event ingestion path.
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

    // Builtin `ralph` hat: only control topics allowed; all business topics
    // rejected.  Prevents the fallback orchestration hat from masquerading as
    // a workflow hat (e.g. signing `review.complete` or `work.start`).
    // P1-12: uses `is_ralph_control_topic` so future `ralph.*` topics are
    // automatically recognized without updating the constant list.
    if event.hat.as_deref() == Some("ralph") {
        if !is_ralph_control_topic(topic_str) {
            warn!(
                topic = %topic_str,
                "Builtin ralph hat may only publish control topics; rejecting business topic"
            );
            return OriginCheck::Rejected {
                topic: topic_str.to_string(),
                hat: event.hat.clone(),
                reason: "ralph_control_only",
            };
        }
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

/// Information about an event that was rejected by the origin guard.
/// Used by the CLI runner to produce unified recovery diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginRejection {
    pub topic: String,
    pub source_hat: Option<String>,
    pub reason: &'static str,
}

/// Filters a batch of JSONL events through origin validation.
///
/// Returns `(accepted, rejections)` so the caller can produce unified
/// recovery diagnostics for all three rejection sources (origin, policy,
/// execution contract).
pub fn filter_events_by_origin(
    events: Vec<JsonlEvent>,
    registry: &HatRegistry,
    cancellation_topic: &str,
    completion_promise: &str,
) -> (Vec<JsonlEvent>, Vec<OriginRejection>) {
    let mut accepted = Vec::with_capacity(events.len());
    let mut rejections = Vec::new();
    for event in events {
        match validate_event_origin(&event, registry, cancellation_topic, completion_promise) {
            OriginCheck::Accepted => accepted.push(event),
            OriginCheck::Rejected { topic, hat, reason } => {
                debug!(
                    topic = %topic,
                    hat = ?hat,
                    reason = reason,
                    "JSONL event rejected by origin guard"
                );
                rejections.push(OriginRejection {
                    topic,
                    source_hat: hat,
                    reason,
                });
            }
        }
    }
    (accepted, rejections)
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
            validate_event_origin(&event, &registry, "", ""),
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
            validate_event_origin(&event, &registry, "", ""),
            OriginCheck::Rejected {
                topic: "experiment.planned".to_string(),
                hat: Some("strategist".to_string()),
                reason: "unknown hat rejected"
            }
        );
    }

    #[test]
    fn test_event_origin_no_hat_business_event_accepted() {
        // No-hat business events pass through the origin guard in hat-based mode.
        // Scope enforcement in the caller (process_parse_result) validates them
        // against active hats when enforce_hat_scope is on. The origin guard
        // itself does not reject no-hat events because events parsed from agent
        // output are inherently no-hat.
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
            validate_event_origin(&event, &registry, "", ""),
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
            validate_event_origin(&event, &registry, "", ""),
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
            validate_event_origin(&event, &registry, "", ""),
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
            validate_event_origin(&event, &registry, "", ""),
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
            validate_event_origin(&event, &registry, "", ""),
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
            validate_event_origin(&event, &registry, "", ""),
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
            validate_event_origin(&event, &registry, "", ""),
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
            validate_event_origin(&event, &registry, "", ""),
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
            validate_event_origin(&event, &registry, "", ""),
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
            validate_event_origin(&event, &registry, "loop.cancel", ""),
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
        let (filtered, rejections) = filter_events_by_origin(events, &registry, "", "");
        assert_eq!(filtered.len(), 2);
        assert_eq!(rejections.len(), 1);
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
            validate_event_origin(&event, &registry, "", ""),
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
            validate_event_origin(&event, &registry, "", ""),
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
            validate_event_origin(&event, &registry, "", ""),
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

    /// Creates a runtime-aware registry with builtin "ralph" hat for origin guard tests.
    fn runtime_registry_with_hats(yaml_with_config: &str) -> HatRegistry {
        // We need the full yaml including event_loop config for from_runtime_config.
        // If the input doesn't start with "hats:", wrap it in a minimal config.
        let full_yaml = if yaml_with_config.trim().starts_with("hats:") {
            format!(
                "{}\nevent_loop:\n  completion_promise: LOOP_COMPLETE\n  cancellation_promise: loop.cancel",
                yaml_with_config
            )
        } else {
            yaml_with_config.to_string()
        };
        let config: RalphConfig = serde_yaml::from_str(&full_yaml).unwrap();
        HatRegistry::from_runtime_config(&config)
    }

    #[test]
    fn test_ralph_as_builtin_hat_passes_origin_guard() {
        // U2: `ralph` pseudo-hat is restricted to control topics only.
        // Business topics (work.start, totally.fake) are now rejected
        // with reason "ralph_control_only".
        let registry = runtime_registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.done"]
"#,
        );

        // hat=ralph topic=work.start: business topic — rejected (ralph_control_only)
        let event = make_event("work.start", Some("ralph"));
        assert_eq!(
            validate_event_origin(&event, &registry, "loop.cancel", ""),
            OriginCheck::Rejected {
                topic: "work.start".to_string(),
                hat: Some("ralph".to_string()),
                reason: "ralph_control_only"
            },
            "hat=ralph with business topic should be rejected (ralph_control_only)"
        );

        // hat=ralph topic=LOOP_COMPLETE: completion promise is a control topic
        let event = make_event("LOOP_COMPLETE", Some("ralph"));
        assert_eq!(
            validate_event_origin(&event, &registry, "loop.cancel", ""),
            OriginCheck::Accepted,
            "hat=ralph should pass origin guard for LOOP_COMPLETE (control topic)"
        );

        // hat=ralph topic=loop.cancel: cancellation promise is a control topic
        let event = make_event("loop.cancel", Some("ralph"));
        assert_eq!(
            validate_event_origin(&event, &registry, "loop.cancel", ""),
            OriginCheck::Accepted,
            "hat=ralph should pass origin guard for loop.cancel (control topic)"
        );

        // hat=ralph topic=totally.fake: not a control topic — rejected
        let event = make_event("totally.fake", Some("ralph"));
        assert_eq!(
            validate_event_origin(&event, &registry, "loop.cancel", ""),
            OriginCheck::Rejected {
                topic: "totally.fake".to_string(),
                hat: Some("ralph".to_string()),
                reason: "ralph_control_only"
            },
            "hat=ralph with off-graph topic should be rejected (ralph_control_only)"
        );

        // hat=fake topic=work.start: unknown hat — should be rejected
        let event = make_event("work.start", Some("fake"));
        assert_eq!(
            validate_event_origin(&event, &registry, "loop.cancel", ""),
            OriginCheck::Rejected {
                topic: "work.start".to_string(),
                hat: Some("fake".to_string()),
                reason: "unknown hat rejected"
            },
            "unknown hat should be rejected even for valid topic"
        );
    }

    #[test]
    fn test_ralph_pseudo_hat_rejected_for_business_topics() {
        // U2: `ralph` pseudo-hat must NOT be able to publish business topics
        // like work.start or review.ready. This was the original vulnerability
        // in the worktree loop — `ralph` masquerading as executor.
        let registry = runtime_registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.start", "review.ready"]
    publishes: ["work.done"]
"#,
        );

        // work.start is a business topic → rejected
        let event = make_event("work.start", Some("ralph"));
        assert_eq!(
            validate_event_origin(&event, &registry, "", ""),
            OriginCheck::Rejected {
                topic: "work.start".to_string(),
                hat: Some("ralph".to_string()),
                reason: "ralph_control_only"
            }
        );

        // review.ready is a business topic → rejected
        let event = make_event("review.ready", Some("ralph"));
        assert_eq!(
            validate_event_origin(&event, &registry, "", ""),
            OriginCheck::Rejected {
                topic: "review.ready".to_string(),
                hat: Some("ralph".to_string()),
                reason: "ralph_control_only"
            }
        );
    }

    #[test]
    fn test_ralph_pseudo_hat_rejected_for_workflow_publish_topics() {
        // U2: `ralph` pseudo-hat must also be rejected for workflow publish
        // topics like work.done and review.result — only control topics allowed.
        let registry = runtime_registry_with_hats(
            r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.done", "review.result"]
"#,
        );

        // work.done is a business topic → rejected
        let event = make_event("work.done", Some("ralph"));
        assert_eq!(
            validate_event_origin(&event, &registry, "", ""),
            OriginCheck::Rejected {
                topic: "work.done".to_string(),
                hat: Some("ralph".to_string()),
                reason: "ralph_control_only"
            }
        );

        // review.result is a business topic → rejected
        let event = make_event("review.result", Some("ralph"));
        assert_eq!(
            validate_event_origin(&event, &registry, "", ""),
            OriginCheck::Rejected {
                topic: "review.result".to_string(),
                hat: Some("ralph".to_string()),
                reason: "ralph_control_only"
            }
        );
    }

    // -------------------------------------------------------------------
    // P1-1 fix (post-review): `is_orchestrator_diagnostic_topic` is an
    // explicit allowlist, not a `event.*` prefix match. Any hat-declared
    // topic with the `event.` prefix must NOT bypass the per-turn
    // budget gate. The known orchestrator-internal diagnostic topics
    // must still be accepted.
    // -------------------------------------------------------------------

    #[test]
    fn test_p1_1_diagnostic_allowlist_known_topics_accepted() {
        // Each entry in ORCHESTRATOR_DIAGNOSTIC_TOPICS must return true.
        for topic in [
            "event.malformed",
            "event.scope_violation",
            "event.workflow_guard_rejected",
            "event.state_machine.rejected",
            "event.state_machine.ignored",
            "event.state_machine.diagnostic",
            "event.policy_warning",
            "event.completion.blocked",
            "event.completion.ignored",
            "event.isolation.boundary_violation",
            "event.topic_format.rejected",
            "event.execution_contract.rejected",
            "event.payload_contract.rejected",
        ] {
            assert!(
                is_orchestrator_diagnostic_topic(topic),
                "P1-1: {topic} must be recognised as an orchestrator diagnostic"
            );
        }
    }

    #[test]
    fn test_p1_1_diagnostic_allowlist_rejects_arbitrary_event_prefix() {
        // A hat that declares a custom `event.*` topic in its `publishes`
        // list must NOT have it bypass the per-turn budget. The fix moves
        // us from `starts_with("event.")` (defense-in-depth unsafe) to an
        // explicit allowlist.
        for topic in [
            "event.something_custom",
            "event.custom_diagnostic",
            "event.user_defined",
            "event.private",
            // Edge case: a topic that looks like a known diagnostic but
            // is in fact a different string.
            "event.malformed_extra",
            "event.isolation.boundary_violation_typo",
        ] {
            assert!(
                !is_orchestrator_diagnostic_topic(topic),
                "P1-1: {topic} must NOT bypass the per-turn budget (allowlist only)"
            );
        }
    }

    // -------------------------------------------------------------------
    // P1-2 fix (post-review): `is_orchestrator_control_topic` matches
    // case-insensitively. A hat that emits `HUMAN.GUIDANCE` or
    // `loop.cancel` (case-mismatched) must still be classified as a
    // control topic so the rejection goes through the correct
    // diagnostic (boundary_violation, not scope_violation).
    // -------------------------------------------------------------------

    #[test]
    fn test_p1_2_control_topic_case_insensitive_match() {
        // Lowercase baseline (no change in behavior).
        assert!(is_orchestrator_control_topic(
            "human.interact",
            "loop.cancel"
        ));
        assert!(is_orchestrator_control_topic(
            "human.guidance",
            "loop.cancel"
        ));
        assert!(is_orchestrator_control_topic("task.resume", "loop.cancel"));
        assert!(is_orchestrator_control_topic(
            "build.task.abandoned",
            "loop.cancel"
        ));

        // Uppercase — same control topics, different case.
        assert!(is_orchestrator_control_topic(
            "HUMAN.INTERACT",
            "loop.cancel"
        ));
        assert!(is_orchestrator_control_topic(
            "Human.Guidance",
            "loop.cancel"
        ));
        assert!(is_orchestrator_control_topic("TASK.RESUME", "loop.cancel"));
        assert!(is_orchestrator_control_topic(
            "BUILD.TASK.ABANDONED",
            "loop.cancel"
        ));

        // Cancellation topic is also case-insensitive.
        assert!(is_orchestrator_control_topic("LOOP.CANCEL", "loop.cancel"));
        assert!(is_orchestrator_control_topic("Loop.Cancel", "loop.cancel"));
    }

    #[test]
    fn test_p1_2_control_topic_does_not_match_business_topics() {
        // Even with case variation, a business topic like loop.complete
        // (lowercase) is NOT a control topic — it must be rejected via
        // can_publish (scope_violation), not bypassed as control.
        assert!(!is_orchestrator_control_topic(
            "loop.complete",
            "loop.cancel"
        ));
        assert!(!is_orchestrator_control_topic(
            "LOOP.COMPLETE",
            "loop.cancel"
        ));
        assert!(!is_orchestrator_control_topic(
            "review.complete",
            "loop.cancel"
        ));
        assert!(!is_orchestrator_control_topic(
            "plan.blocked",
            "loop.cancel"
        ));
    }

    // ── 2026-06-18-001 plan U5: is_anonymous_business_topic ──────────

    /// 构造一个含 hat_a、hat_b 两 hat 的 registry。
    fn two_hat_registry() -> HatRegistry {
        registry_with_hats(
            r#"
hats:
  hat_a:
    name: "HatA"
    triggers: ["a.ready"]
    publishes: ["a.done"]
  hat_b:
    name: "HatB"
    triggers: ["b.ready"]
    publishes: ["b.done"]
"#,
        )
    }

    /// U5: 完全无 hat/source/triggered 的 business topic + 非 isolated 模式 → 放行。
    #[test]
    fn u5_non_isolated_mode_passes() {
        let registry = two_hat_registry();
        let event = make_event("work.ready", None);
        assert!(!is_anonymous_business_topic(
            &event,
            &registry,
            "loop.cancel",
            None,
        ));
    }

    /// U5: 完全无 hat/source/triggered + isolated_hat 在 registry → 放行
    /// (走既有 scope enforcement fallback 路径)。
    #[test]
    fn u5_isolated_with_known_isolated_hat_passes() {
        let registry = two_hat_registry();
        let event = make_event("work.ready", None);
        assert!(!is_anonymous_business_topic(
            &event,
            &registry,
            "loop.cancel",
            Some("hat_a"),
        ));
    }

    /// U5: 完全无 hat/source/triggered + isolated_hat 不在 registry → 拒收。
    #[test]
    fn u5_isolated_with_unknown_isolated_hat_anonymous() {
        let registry = two_hat_registry();
        let event = make_event("work.ready", None);
        assert!(is_anonymous_business_topic(
            &event,
            &registry,
            "loop.cancel",
            Some("nonexistent-isolated-hat"),
        ));
    }

    /// U5: hat 字段存在但指向未注册 hat → **不**判匿名,交给 scope enforcement
    /// 触发 `isolated_scope_violation` recovery envelope。
    #[test]
    fn u5_unknown_hat_field_falls_through_to_scope_enforcement() {
        let registry = two_hat_registry();
        let mut ev = make_event("work.ready", None);
        ev.hat = Some("nonexistent-hat".to_string());
        // U5 修订:hat 字段存在(即使未注册)由 scope enforcement 处理,
        // 此处不判匿名,避免双门覆盖导致 recovery envelope 路径被绕开。
        assert!(!is_anonymous_business_topic(
            &ev,
            &registry,
            "loop.cancel",
            Some("hat_a"),
        ));
    }

    /// U5: hat 字段指向注册 hat → 放行。
    #[test]
    fn u5_known_hat_field_passes() {
        let registry = two_hat_registry();
        let event = make_event("work.ready", Some("hat_a"));
        assert!(!is_anonymous_business_topic(
            &event,
            &registry,
            "loop.cancel",
            Some("hat_a"),
        ));
    }

    /// U5: source 字段指向注册 hat → 放行。
    #[test]
    fn u5_known_source_field_passes() {
        let registry = two_hat_registry();
        let mut ev = make_event("work.ready", None);
        ev.source = Some("hat_b".to_string());
        assert!(!is_anonymous_business_topic(
            &ev,
            &registry,
            "loop.cancel",
            Some("hat_a"),
        ));
    }

    /// U5: triggered 字段指向注册 hat → 放行。
    #[test]
    fn u5_known_triggered_field_passes() {
        let registry = two_hat_registry();
        let mut ev = make_event("work.ready", None);
        ev.triggered = Some("hat_a".to_string());
        assert!(!is_anonymous_business_topic(
            &ev,
            &registry,
            "loop.cancel",
            Some("hat_a"),
        ));
    }

    /// U5: control topic 即使无 provenance 也放行(不属 business)。
    #[test]
    fn u5_control_topic_passes_without_provenance() {
        let registry = two_hat_registry();
        let event = make_event("human.guidance", None);
        assert!(!is_anonymous_business_topic(
            &event,
            &registry,
            "loop.cancel",
            Some("hat_a"),
        ));
    }

    /// U5: diagnostic topic 即使无 provenance 也放行。
    #[test]
    fn u5_diagnostic_topic_passes_without_provenance() {
        let registry = two_hat_registry();
        let event = make_event("event.isolation.boundary_violation", None);
        assert!(!is_anonymous_business_topic(
            &event,
            &registry,
            "loop.cancel",
            Some("hat_a"),
        ));
    }

    /// P1-12: `is_ralph_control_topic` accepts existing control topics
    /// and future `ralph.*` topics via prefix match.
    #[test]
    fn test_p1_12_is_ralph_control_topic_existing_and_future() {
        // Existing control topics (exact match).
        for topic in RALPH_CONTROL_TOPICS {
            assert!(
                is_ralph_control_topic(topic),
                "P1-12: {topic} must be recognised as control"
            );
        }
        // Future `ralph.*` topics (prefix match).
        assert!(is_ralph_control_topic("ralph.status"));
        assert!(is_ralph_control_topic("ralph.diagnose.request"));
        assert!(is_ralph_control_topic("ralph.unknown_future_topic"));
        // Business topics must NOT match.
        assert!(!is_ralph_control_topic("work.ready"));
        assert!(!is_ralph_control_topic("work.done"));
        assert!(!is_ralph_control_topic("review.complete"));
        assert!(!is_ralph_control_topic("plan.blocked"));
    }

    /// P1-8: cross-reference sanity check — the `RALPH_CONTROL_TOPICS`
    /// constant and `is_ralph_control_topic` must agree on the exact set.
    /// If a topic is in the constant but not recognised by the function,
    /// or vice versa, the three layers (event_origin, emit, publish_event)
    /// will disagree.
    #[test]
    fn test_p1_8_ralph_control_topics_cross_reference() {
        for topic in RALPH_CONTROL_TOPICS {
            assert!(
                is_ralph_control_topic(topic),
                "P1-8: constant topic '{topic}' must be recognised by is_ralph_control_topic"
            );
        }
    }
}

