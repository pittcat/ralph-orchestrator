// Cross-submodule import for types defined in sibling modules
use super::runtime::{PolicyRuntimeState, precheck_proposed_dedup_key, review_start_dedup_key};
use super::types::{DuplicateWorkDoneHint, PolicyDecision, PolicyFinding, ViolationType};
use crate::config::{
    CompletionAfterTerminalAction, ElementConstraint, EventPolicyConfig, EventPolicyMode,
    EventSchema, HatAllowedValues, PayloadType, TopicDenyRule, ViolationAction,
};
use ralph_proto::{Hat, HatId, Topic};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub fn check_completion_honored(
    topic: &str,
    config: &EventPolicyConfig,
    state: &PolicyRuntimeState,
) -> Option<PolicyDecision> {
    check_completion_guard(topic, config, state.completion_honored)
}

/// Check if an event should be guarded when a completion signal has been seen.
///
/// This is the core logic used both for persistent `completion_honored` state
/// and for per-batch same-batch guarding.
pub fn check_completion_guard(
    topic: &str,
    config: &EventPolicyConfig,
    guard_active: bool,
) -> Option<PolicyDecision> {
    if !guard_active {
        return None;
    }

    if config.terminal_topics.contains(&topic.to_string()) {
        Some(apply_completion_after_terminal_action(
            &config.completion_after_terminal.duplicate_terminal,
            topic,
            ViolationType::DuplicateTerminalEvent {
                topic: topic.to_string(),
            },
        ))
    } else if config.business_topics.contains(&topic.to_string()) {
        Some(apply_completion_after_terminal_action(
            &config.completion_after_terminal.business_after_completion,
            topic,
            ViolationType::BusinessEventAfterCompletion {
                topic: topic.to_string(),
            },
        ))
    } else {
        None
    }
}

fn apply_completion_after_terminal_action(
    action: &CompletionAfterTerminalAction,
    topic: &str,
    violation_type: ViolationType,
) -> PolicyDecision {
    let finding = PolicyFinding {
        topic: topic.to_string(),
        violation_type,
        message: format!("Event '{}' arrived after completion was honored", topic),
        evidence: None,
    };

    match action {
        CompletionAfterTerminalAction::Reject => PolicyDecision::Block(finding),
        CompletionAfterTerminalAction::Ignore => PolicyDecision::Ignore(finding),
        CompletionAfterTerminalAction::Warn => PolicyDecision::Warn(vec![finding]),
    }
}

fn redteam_queue_enabled(config: &EventPolicyConfig) -> bool {
    config.schemas.contains_key("redteam.attack.mapped")
        && config.schemas.contains_key("redteam.experiment.done")
        && config.schemas.contains_key("redteam.experiment.next")
        && config.schemas.contains_key("redteam.evidence.gated")
}

fn redteam_counter(
    topic: &str,
    obj: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<u64>, Box<PolicyFinding>> {
    let Some(value) = obj.get(field) else {
        return Ok(None);
    };
    value.as_u64().map(Some).ok_or_else(|| Box::new(PolicyFinding {
        topic: topic.to_string(),
        violation_type: ViolationType::PayloadTypeMismatch {
            expected: "non-negative integer".to_string(),
            actual: type_name(value).to_string(),
        },
        message: format!(
            "{topic} 的字段 '{field}' 必须是非负整数，当前类型是 {}。请从证据板重新计算后再发送。",
            type_name(value)
        ),
        evidence: None,
    }))
}

fn redteam_queue_finding(topic: &str, context: String, fields: &[&str]) -> PolicyFinding {
    PolicyFinding {
        topic: topic.to_string(),
        violation_type: ViolationType::SemanticGateViolation {
            gate: "redteam_experiment_queue_consistency".to_string(),
            context: context.clone(),
            referenced_fields: fields.iter().map(|field| (*field).to_string()).collect(),
        },
        message: format!(
            "redteam_experiment_queue_consistency: {context}；请读取最新 evidence board，修正本次 handoff，只发送一个与当前队列状态一致的事件。"
        ),
        evidence: None,
    }
}

/// Validate the red-team queue's cross-event invariants.
///
/// The generic `payload_consistency` evaluator deliberately examines only the
/// current payload. This gate is the stateful counterpart for the red-team
/// preset: it binds queue counters to accepted events and makes each serial
/// handoff idempotent.
fn check_redteam_queue_invariant(
    topic: &str,
    payload: Option<&str>,
    config: &EventPolicyConfig,
    state: &PolicyRuntimeState,
) -> Option<PolicyFinding> {
    if !redteam_queue_enabled(config) {
        return None;
    }
    let payload = payload?;
    let obj = match serde_json::from_str::<Value>(payload) {
        Ok(Value::Object(obj)) => obj,
        _ => return None,
    };

    match topic {
        "redteam.attack.mapped" => {
            if let Err(finding) = redteam_counter(topic, &obj, "experiment_count") {
                return Some(*finding);
            }
            if state.redteam_experiment_total.is_some() {
                Some(redteam_queue_finding(
                    topic,
                    "attack.mapped 只能初始化一次实验队列；检测到重复初始化".to_string(),
                    &["experiment_count"],
                ))
            } else {
                None
            }
        }
        "redteam.experiment.done" => {
            let experiment_id_value = obj.get("experiment_id")?;
            let Some(experiment_id) = experiment_id_value.as_str() else {
                return Some(redteam_queue_finding(
                    topic,
                    format!(
                        "experiment_id 必须是字符串，当前类型是 {}",
                        type_name(experiment_id_value)
                    ),
                    &["experiment_id"],
                ));
            };
            if experiment_id.is_empty() {
                return Some(redteam_queue_finding(
                    topic,
                    "experiment_id 不能为空".to_string(),
                    &["experiment_id"],
                ));
            }
            if let Some(expected) = &state.redteam_experiment_pending_id
                && expected != experiment_id
            {
                return Some(redteam_queue_finding(
                    topic,
                    format!(
                        "experiment.done 的 experiment_id={experiment_id} 与队列当前等待的 {expected} 不一致，不能跳过 next handoff"
                    ),
                    &["experiment_id", "next_experiment_id"],
                ));
            }
            if state.redteam_experiment_done_ids.contains(experiment_id) {
                return Some(redteam_queue_finding(
                    topic,
                    format!("experiment_id={experiment_id} 已经完成过，拒绝重复 experiment.done"),
                    &["experiment_id"],
                ));
            }
            if let Some(total) = state.redteam_experiment_total
                && state.redteam_experiment_done_count >= total
            {
                return Some(redteam_queue_finding(
                    topic,
                    format!("实验队列已经完成 {total} 项，不能再接受 experiment.done"),
                    &["experiment_id"],
                ));
            }
            None
        }
        "redteam.experiment.next" => {
            let completed = match redteam_counter(topic, &obj, "completed_count") {
                Ok(value) => value,
                Err(finding) => return Some(*finding),
            };
            let remaining = match redteam_counter(topic, &obj, "remaining_count") {
                Ok(value) => value,
                Err(finding) => return Some(*finding),
            };
            let accepted = match redteam_counter(topic, &obj, "accepted_count") {
                Ok(value) => value,
                Err(finding) => return Some(*finding),
            };
            let rejected = match redteam_counter(topic, &obj, "rejected_count") {
                Ok(value) => value,
                Err(finding) => return Some(*finding),
            };
            let next_id_value = obj.get("next_experiment_id")?;
            let Some(next_id) = next_id_value.as_str() else {
                return Some(redteam_queue_finding(
                    topic,
                    format!(
                        "next_experiment_id 必须是字符串，当前类型是 {}",
                        type_name(next_id_value)
                    ),
                    &["next_experiment_id"],
                ));
            };
            if next_id.is_empty() {
                return Some(redteam_queue_finding(
                    topic,
                    "next_experiment_id 不能为空".to_string(),
                    &["next_experiment_id"],
                ));
            }
            let (Some(completed), Some(remaining), Some(accepted), Some(rejected)) =
                (completed, remaining, accepted, rejected)
            else {
                return None;
            };
            let Some(total) = state.redteam_experiment_total else {
                return Some(redteam_queue_finding(
                    topic,
                    "实验队列尚未由 attack.mapped 初始化，不能发送 next handoff".to_string(),
                    &["experiment_count", "next_experiment_id"],
                ));
            };
            if remaining == 0 {
                return Some(redteam_queue_finding(
                    topic,
                    "队列已经耗尽，必须发送 redteam.evidence.gated 或 redteam.failed，而不是 next"
                        .to_string(),
                    &["remaining_count"],
                ));
            }
            if completed != state.redteam_experiment_done_count {
                return Some(redteam_queue_finding(
                    topic,
                    format!(
                        "completed_count={completed} 与已接受的 experiment.done 数量 {} 不一致",
                        state.redteam_experiment_done_count
                    ),
                    &["completed_count"],
                ));
            }
            if accepted.checked_add(rejected) != Some(completed) {
                return Some(redteam_queue_finding(
                    topic,
                    format!(
                        "accepted_count={accepted} 加 rejected_count={rejected} 必须等于 completed_count={completed}"
                    ),
                    &["completed_count", "accepted_count", "rejected_count"],
                ));
            }
            if completed.checked_add(remaining) != Some(total) {
                return Some(redteam_queue_finding(
                    topic,
                    format!(
                        "completed_count={completed} 加 remaining_count={remaining} 必须等于 experiment_count={total}"
                    ),
                    &["completed_count", "remaining_count", "experiment_count"],
                ));
            }
            if state.redteam_experiment_last_next_completed_count == Some(completed) {
                return Some(redteam_queue_finding(
                    topic,
                    format!(
                        "completed_count={completed} 的 next handoff 已经接受过；同一轮只能发送一次"
                    ),
                    &["completed_count", "next_experiment_id"],
                ));
            }
            if state.redteam_experiment_next_seen_ids.contains(next_id)
                || state.redteam_experiment_done_ids.contains(next_id)
            {
                return Some(redteam_queue_finding(
                    topic,
                    format!("next_experiment_id={next_id} 已经被调度或完成，不能重复或回退队列"),
                    &["next_experiment_id"],
                ));
            }
            None
        }
        "redteam.evidence.gated" => {
            let total = match redteam_counter(topic, &obj, "total_experiment_count") {
                Ok(value) => value,
                Err(finding) => return Some(*finding),
            };
            let qualified = match redteam_counter(topic, &obj, "qualified_experiment_count") {
                Ok(value) => value,
                Err(finding) => return Some(*finding),
            };
            let rejected = match redteam_counter(topic, &obj, "rejected_experiment_count") {
                Ok(value) => value,
                Err(finding) => return Some(*finding),
            };
            let (Some(total), Some(qualified), Some(rejected)) = (total, qualified, rejected)
            else {
                return None;
            };
            let Some(expected_total) = state.redteam_experiment_total else {
                return Some(redteam_queue_finding(
                    topic,
                    "evidence.gated 缺少已初始化的 attack.mapped 队列".to_string(),
                    &["total_experiment_count", "experiment_count"],
                ));
            };
            if total != expected_total || state.redteam_experiment_done_count != expected_total {
                return Some(redteam_queue_finding(
                    topic,
                    format!(
                        "最终 total_experiment_count={total}、已完成数量 {} 必须都等于 experiment_count={expected_total}",
                        state.redteam_experiment_done_count
                    ),
                    &["total_experiment_count", "experiment_count"],
                ));
            }
            if qualified.checked_add(rejected) != Some(total) {
                return Some(redteam_queue_finding(
                    topic,
                    format!(
                        "qualified_experiment_count={qualified} 加 rejected_experiment_count={rejected} 必须等于 total_experiment_count={total}"
                    ),
                    &[
                        "qualified_experiment_count",
                        "rejected_experiment_count",
                        "total_experiment_count",
                    ],
                ));
            }
            let ids_value = obj.get("qualified_experiment_ids")?;
            let Some(ids) = ids_value.as_array() else {
                return Some(redteam_queue_finding(
                    topic,
                    format!(
                        "qualified_experiment_ids 必须是数组，当前类型是 {}",
                        type_name(ids_value)
                    ),
                    &["qualified_experiment_ids"],
                ));
            };
            let ids: Option<Vec<&str>> = ids.iter().map(Value::as_str).collect();
            let Some(ids) = ids else {
                return Some(redteam_queue_finding(
                    topic,
                    "qualified_experiment_ids 必须是字符串 ID 数组".to_string(),
                    &["qualified_experiment_ids"],
                ));
            };
            let unique_ids: HashSet<&str> = ids.iter().copied().collect();
            if ids.len() as u64 != qualified || unique_ids.len() != ids.len() {
                return Some(redteam_queue_finding(topic,
                    "qualified_experiment_ids 的长度必须等于 qualified_experiment_count，且不能含重复 ID"
                        .to_string(),
                    &["qualified_experiment_ids", "qualified_experiment_count"],
                ));
            }
            if ids
                .iter()
                .any(|experiment_id| !state.redteam_experiment_done_ids.contains(*experiment_id))
            {
                return Some(redteam_queue_finding(
                    topic,
                    "qualified_experiment_ids 必须全部来自已接受的 experiment.done".to_string(),
                    &["qualified_experiment_ids"],
                ));
            }
            None
        }
        _ => None,
    }
}

fn apply_redteam_queue_state(topic: &str, payload: Option<&str>, state: &mut PolicyRuntimeState) {
    let Some(payload) = payload else {
        return;
    };
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(payload) else {
        return;
    };
    match topic {
        "redteam.attack.mapped" => {
            if let Some(total) = obj.get("experiment_count").and_then(Value::as_u64) {
                state.redteam_experiment_total = Some(total);
                state.redteam_experiment_done_count = 0;
                state.redteam_experiment_done_ids.clear();
                state.redteam_experiment_next_seen_ids.clear();
                state.redteam_experiment_pending_id = None;
                state.redteam_experiment_last_next_completed_count = None;
            }
        }
        "redteam.experiment.done" => {
            if let Some(experiment_id) = obj.get("experiment_id").and_then(Value::as_str)
                && state
                    .redteam_experiment_done_ids
                    .insert(experiment_id.to_string())
            {
                state.redteam_experiment_done_count =
                    state.redteam_experiment_done_count.saturating_add(1);
                state.redteam_experiment_pending_id = None;
            }
        }
        "redteam.experiment.next" => {
            if let Some(next_id) = obj.get("next_experiment_id").and_then(Value::as_str) {
                state
                    .redteam_experiment_next_seen_ids
                    .insert(next_id.to_string());
                state.redteam_experiment_pending_id = Some(next_id.to_string());
            }
            if let Some(completed) = obj.get("completed_count").and_then(Value::as_u64) {
                state.redteam_experiment_last_next_completed_count = Some(completed);
            }
        }
        _ => {}
    }
}

/// R9: Check topic format against the whitelist of known topics.
///
/// Rejects topics not in the whitelist **before** payload schema validation.
/// Rejection is non-retryable — only writes a recovery signal (R10), no
/// `task.resume` is emitted.
///
/// The whitelist is built from:
/// - All hat `publishes` topics (from hat registry)
/// - System/control topics (`event.*`, `human.*`, `loop.cancel`, `task.resume`,
///   `build.task.abandoned`, completion promise)
///
/// Returns `None` if the topic is valid (accepted), or `Some(PolicyDecision::Block(...))`
/// if the topic is not in the whitelist.
/// 2026-07-06-004 plan U8: handoff envelope validator. When
/// `event_loop.handoff_envelope.validate_payload` is true, every
/// event whose payload parses as a JSON object must contain a
/// valid `payload.handoff_envelope` (per `handoff_envelope.v1`).
/// Returns `Some(PolicyFinding)` on failure, `None` on success.
/// The validator delegates to `handoff_envelope::validate_handoff_envelope_payload`
/// so the (code, message) error envelope is shared between the
/// prompt-injection path and the policy-check pipeline.
pub fn check_handoff_envelope(topic: &str, payload: &Value) -> Option<PolicyFinding> {
    use crate::handoff_envelope;
    // U3 (2026-07-06-004 fix-plan): callers in the CLI
    // boundary cannot construct a `HatRegistry` from inside
    // `check_handoff_envelope` (the registry lives on the
    // pipeline, not on the policy), so the registry check is
    // performed by callers that hold a registry reference
    // (production path: `EventPolicyRule::validate` in
    // `validation/rules_event_policy.rs`). The policy gate
    // here keeps the no-registry shape for parity with
    // pre-fix callers.
    match handoff_envelope::validate_handoff_envelope_payload(payload, None) {
        Ok(_) => None,
        Err(err) => Some(PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::MissingRequiredField {
                field: "handoff_envelope".to_string(),
            },
            message: format!("handoff_envelope validation failed: {}", err),
            evidence: None,
        }),
    }
}

/// 2026-07-06-004 plan U8: in-process gating helper. Returns
/// true iff the policy-check pipeline should run
/// `check_handoff_envelope` for the supplied payload. The
/// condition is exactly `handoff_config.enabled &&
/// handoff_config.validate_payload && payload.is_some()`.
// 2026-07-16 cleanup U4 (KTD-3): reserved for U15 / future
// policy-check parity; pinning the public signature now avoids
// churn when downstream consumers start importing it.
#[allow(dead_code)]
pub fn handoff_envelope_validation_enabled<H: HandoffEnvelopeConfigAccess>(
    payload: Option<&str>,
    handoff_config: &H,
) -> bool {
    handoff_config.handoff_envelope_enabled()
        && handoff_config.handoff_envelope_validate_payload()
        && payload.is_some()
}

/// 2026-07-06-004 plan U8: typed adapter that bridges
/// `EventLoopConfig.handoff_envelope` into the policy pipeline
/// via the `HandoffEnvelopeConfigAccess` trait. Used by the
/// `ralph emit --policy-check` path once U10 wires the real
/// config in.
pub struct EventLoopHandoffConfig<'a> {
    pub handoff_envelope: &'a crate::config::HandoffEnvelopeConfig,
}

impl HandoffEnvelopeConfigAccess for EventLoopHandoffConfig<'_> {
    fn handoff_envelope_enabled(&self) -> bool {
        self.handoff_envelope.enabled
    }
    fn handoff_envelope_validate_payload(&self) -> bool {
        self.handoff_envelope.validate_payload
    }
}

/// - System/control topics (`event.*`, `human.*`, `loop.cancel`, `task.resume`,
///   `build.task.abandoned`, completion promise)
///
/// Returns `None` if the topic is valid (accepted), or `Some(PolicyDecision::Block(...))`
/// if the topic is not in the whitelist.
pub fn check_topic_format(topic: &str, allowed_topics: &HashSet<String>) -> Option<PolicyDecision> {
    if allowed_topics.contains(topic) {
        return None;
    }

    // R6 (2026-06-17-004 plan): make the diagnostic list deterministic.
    // `HashSet` iteration order is undefined, so sort before serialising
    // into the finding/message to keep regression snapshots stable.
    let mut allowed_list: Vec<String> = allowed_topics.iter().cloned().collect();
    allowed_list.sort();

    let finding = PolicyFinding {
        topic: topic.to_string(),
        violation_type: ViolationType::InvalidTopicFormat {
            topic: topic.to_string(),
            allowed_topics: allowed_list.clone(),
        },
        message: format!(
            "Topic '{}' is not in the whitelist of known topics. \
             Valid topics: {:?}",
            topic, allowed_list
        ),
        evidence: None,
    };

    // R10: Block (not RejectWithResume) — no retry, only recovery signal
    Some(PolicyDecision::Block(finding))
}

/// Build the set of allowed topics from hat configs and system control topics.
///
/// Includes:
/// - All hat `publishes` topics (what hats emit)
/// - All hat `triggers` topics (what activates hats)
/// - Event policy `terminal_topics` and `business_topics` (if configured)
/// - System control topics: `loop.cancel`, `task.resume`, `build.task.abandoned`,
///   completion promise
///
/// Note: `event.*` and `human.*` topics are NOT stored here as prefixes.
/// They are allowed by the `is_system_topic()` check which is applied
/// BEFORE `check_topic_format` in the event loop validation flow.
pub fn build_allowed_topics(
    hats: &std::collections::HashMap<String, crate::config::HatConfig>,
    completion_promise: &str,
    event_policy: Option<&EventPolicyConfig>,
) -> HashSet<String> {
    let mut allowed = HashSet::new();

    // Add all hat publishes and triggers topics
    for hat_config in hats.values() {
        for topic in &hat_config.publishes {
            allowed.insert(topic.clone());
        }
        for topic in &hat_config.triggers {
            allowed.insert(topic.clone());
        }
    }

    // Add event policy terminal and business topics
    if let Some(policy) = event_policy {
        for topic in &policy.terminal_topics {
            allowed.insert(topic.clone());
        }
        for topic in &policy.business_topics {
            allowed.insert(topic.clone());
        }
    }

    // System/control topics (exact match)
    allowed.insert("loop.cancel".to_string());
    allowed.insert("task.resume".to_string());
    allowed.insert("build.task.abandoned".to_string());
    allowed.insert(completion_promise.to_string());

    // Note: event.* and human.* topics are handled by is_system_topic() check
    // (tested BEFORE check_topic_format in the event loop), not by prefix
    // matching in this set. The comment above about "stored as actual prefixes"
    // was incorrect - they are not inserted here.

    allowed
}

/// Check if a topic matches a system/control prefix pattern.
///
/// System topics start with `event.` or `human.` and are always allowed
/// regardless of the whitelist. This check is applied BEFORE
/// check_topic_format in the event loop.
pub fn is_system_topic(topic: &str) -> bool {
    topic.starts_with("event.") || topic.starts_with("human.")
}

/// Check if a topic is a system control topic the loop runner
/// itself publishes (`loop.cancel`, `task.resume`,
/// `build.task.abandoned`).
///
/// Unlike [`is_system_topic`], this matches exact topic
/// strings rather than `event.*` / `human.*` prefixes.  The
/// unified validation pipeline calls
/// `check_topic_deny_rules` after `is_system_topic` has
/// already admitted the prefix-matched topics; this
/// short-circuit covers the remaining runner-emitted topics
/// so a deny rule that happens to match the originating hat
/// cannot reject a recovery injection.  See
/// `check_topic_deny_rules` for the regression that motivated
/// the helper.
pub fn is_system_control_topic(topic: &str) -> bool {
    matches!(
        topic,
        "loop.cancel" | "task.resume" | "build.task.abandoned"
    )
}

/// WAC-U7 (2026-06-12-002) R10: hard-reject topics for which a
/// null payload is never acceptable. Any event whose topic is in
/// this set and whose payload is `None` is rejected with
/// `RejectWithResume` regardless of `EventPolicyMode::Observe`.
/// The list is the minimum required by R10; it is intentionally
/// not configurable so the operational contract is uniform
/// across presets.
///
/// Step-handoff (2026-06-17-002) U5: extended with `work.ready`,
/// `plan.complete`, `plan.blocked` so the hard gate uniformly
/// covers every handoff/terminal topic in the ce-executor step
/// chain — independent of whether the preset ships a
/// `payload: json_object` schema for that topic (Observe mode
/// would otherwise let null payloads slip past the schema layer).
pub const NULL_PAYLOAD_REJECT_TOPICS: &[&str] = &[
    "review.passed",
    "review.failed",
    "review.complete",
    "work.done",
    "queue.advance",
    "review.wave.ready",
    "work.ready",
    "plan.complete",
    "plan.blocked",
];

/// Returns `true` if `topic` is in [`NULL_PAYLOAD_REJECT_TOPICS`].
pub fn is_null_payload_rejected_topic(topic: &str) -> bool {
    NULL_PAYLOAD_REJECT_TOPICS.contains(&topic)
}

/// Glob-capable topic match for `topic_deny_rules` (and any other
/// per-rule matcher the runtime shares with the contract compiler).
pub fn matches_topic_rule(rule_topic: &str, event_topic: &str) -> bool {
    if rule_topic.contains('*') {
        Topic::new(rule_topic).matches_str(event_topic)
    } else {
        rule_topic == event_topic
    }
}

/// Check topic-deny rules against a (hat, topic) pair.
///
/// When the event policy is in `Enforce` mode and the (hat_id, topic) pair
/// matches any `topic_deny_rules` entry, returns `Some(PolicyDecision::Block)`
/// with reason `"topic_denied"`.  Otherwise returns `None`.
///
/// In `Observe` mode, matching a deny rule produces a `Warn` decision instead.
///
/// Topic matching supports glob patterns:
/// - Exact match: `build.done` matches `build.done`
/// - Segment wildcard: `debug.*` matches `debug.step`, `debug.done`, etc.
/// - Global wildcard: `*` matches any topic
pub fn check_topic_deny_rules(
    hat: Option<&str>,
    topic: &str,
    config: &EventPolicyConfig,
) -> Option<PolicyDecision> {
    let hat_id = hat.unwrap_or("");
    // 2026-06-30 P0-2 (primary-20260629-170451 diagnosis):
    //
    // System control topics (`loop.cancel`, `task.resume`,
    // `build.task.abandoned`) are orchestrated by the loop
    // runner — the per-hat `topic_deny_rules` must not gate
    // them, even when `event.hat` falls under a hat the preset
    // declared a deny rule for (e.g. validator / coordinator /
    // executor are all on the deny list for `task.resume`).
    // Without this short-circuit the runner's stall-recovery
    // `task.resume` injection was rejected with
    // `EVENT_POLICY_TOPIC_DENIED` while the events file still
    // captured it, leaving ledger vs events out-of-sync and
    // deadlocking the loop on `consecutive_failures` once a
    // single retry exhaustion happened
    // (`loop-termination-reason.json: "consecutive_failures"`).
    //
    // We deliberately do NOT special-case `event.*` or
    // `human.*` here — those are admitted by the existing
    // `is_system_topic` short-circuit that runs BEFORE this
    // function in the unified validation pipeline. The
    // completion promise (`LOOP_COMPLETE` by default) is
    // matched against the deny rules directly: a denial there
    // is the legitimate guard against a hat driving past
    // terminal, so we do not bypass it.
    if is_system_control_topic(topic) {
        return None;
    }
    for rule in &config.topic_deny_rules {
        if rule.hat_id == hat_id && matches_topic_rule(&rule.topic, topic) {
            let finding = PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::TopicDenied {
                    rule_hat: rule.hat_id.clone(),
                    rule_topic: rule.topic.clone(),
                },
                message: format!(
                    "Hat '{}' is denied from publishing topic '{}'",
                    rule.hat_id, rule.topic
                ),
                evidence: None,
            };
            return Some(match config.mode {
                EventPolicyMode::Observe => PolicyDecision::Warn(vec![finding]),
                EventPolicyMode::Enforce => match config.on_violation {
                    ViolationAction::Warn => PolicyDecision::Warn(vec![finding]),
                    ViolationAction::RejectWithResume => PolicyDecision::RejectWithResume(finding),
                    ViolationAction::Hold => PolicyDecision::Hold(finding),
                    ViolationAction::Block => PolicyDecision::Block(finding),
                },
            });
        }
    }
    None
}

/// Validates an event against the event policy.
pub fn validate_event(
    topic: &str,
    payload: Option<&str>,
    config: &EventPolicyConfig,
    state: &mut PolicyRuntimeState,
) -> PolicyDecision {
    validate_event_with_hat(topic, payload, config, state, None)
}

/// Validates an event against the event policy with hat-aware checks.
///
/// `hat` is the emitting hat id (if known). When provided, it enables
/// hat-specific schema restrictions such as per-hat allowed values and
/// topic-deny rules. When omitted, only hat-agnostic checks run.
pub fn validate_event_with_hat(
    topic: &str,
    payload: Option<&str>,
    config: &EventPolicyConfig,
    state: &mut PolicyRuntimeState,
    hat: Option<&str>,
) -> PolicyDecision {
    validate_event_with_options(topic, payload, config, state, hat, &DefaultHandoffConfig)
}

/// 2026-07-06-004 plan U8: the handoff envelope validator is
/// opt-in per preset. The default no-op implementation returns
/// `false` so existing call sites see zero behavioural change
/// (regression defence #5). U10 is the unit that wires the real
/// config into the policy pipeline.
pub trait HandoffEnvelopeConfigAccess {
    fn handoff_envelope_enabled(&self) -> bool;
    fn handoff_envelope_validate_payload(&self) -> bool;
}

pub struct DefaultHandoffConfig;

impl HandoffEnvelopeConfigAccess for DefaultHandoffConfig {
    fn handoff_envelope_enabled(&self) -> bool {
        false
    }
    fn handoff_envelope_validate_payload(&self) -> bool {
        false
    }
}

/// Public entry point used by U8's wiring tests and by the real
/// `ralph emit --policy-check` path once U10 feeds the typed
/// config in. Returns the policy decision for the supplied
/// payload against the supplied event policy.
pub fn validate_event_with_options<H: HandoffEnvelopeConfigAccess>(
    topic: &str,
    payload: Option<&str>,
    config: &EventPolicyConfig,
    state: &mut PolicyRuntimeState,
    hat: Option<&str>,
    handoff_config: &H,
) -> PolicyDecision {
    if !config.enabled {
        return PolicyDecision::Accept;
    }

    state.observed_topics.insert(topic.to_string());

    let mut findings = Vec::new();

    // 2026-07-02-004 U7 (R6): close the retryable precheck gate
    // obligation when the gate emits `<X>.rejected`. A passed
    // candidate stays deduplicated for the lifetime of this loop:
    // clearing it on `<X>` allowed the same successful
    // `<X>.proposed` payload to activate the gate again on a later
    // iteration. A new candidate is still allowed because its
    // payload produces a different dedup key.
    if let Some(guarded) = topic.strip_suffix(".rejected") {
        state.prune_precheck_proposed_bucket(guarded);
    }

    // 2026-07-02-004 U7 (R6): duplicate `<X>.proposed` detection.
    // A 2nd emit with the same `(guarded, payload)` while the
    // gate obligation is still open is rejected so the runtime
    // does not schedule two gate activations for one candidate.
    if let Some(guarded) = topic.strip_suffix(".proposed")
        && let Some(p) = payload
    {
        let key = precheck_proposed_dedup_key(guarded, p);
        if state.precheck_proposed_pending_keys.contains(&key) {
            let finding = PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::DuplicateWorkDone {
                    key: key.clone(),
                    hint: DuplicateWorkDoneHint::DuplicateSameStep,
                    seen_count: None,
                },
                message: format!(
                    "duplicate_precheck_proposed: {topic} for key '{key}' was already accepted. \
                     Wait for the precheck gate to emit {guarded} or {guarded}.rejected before \
                     re-emitting the same candidate."
                ),
                evidence: None,
            };
            return PolicyDecision::RejectWithResume(finding);
        }
        state.precheck_proposed_pending_keys.insert(key);
    }

    // 2026-07-01-001 U1: duplicate `review.start` detection.
    // U8 of plan 2026-07-02-005: prefer the semantic key
    // `(plan_name, fix_round, total_units)` so a 2nd emit with
    // only `triggered` differing (175407 root cause) is still
    // recognised as a duplicate. Falls back to the legacy
    // `(plan_name, task_id [, step])` key when fix_round /
    // total_units are absent.
    if topic == "review.start"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        let fix_round = obj
            .get("fix_round")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);
        let total_units = obj
            .get("total_units")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);
        if let (Some(pn), Some(ti)) = (plan_name, task_id) {
            let dedup_key = review_start_dedup_key(pn, step, ti, fix_round, total_units);
            if state.review_start_seen_keys.contains(&dedup_key) {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        hint: DuplicateWorkDoneHint::DuplicateSameStep,
                        seen_count: None,
                    },
                    message: format!(
                        "duplicate_review_start: review.start for key '{dedup_key}' was already accepted. \
                         Wait for the review sequence to complete before re-sending review.start."
                    ),
                    evidence: None,
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            state.review_start_seen_keys.insert(dedup_key);
        }
    }

    // U4 (2026-06-17-003 plan): duplicate `work.done` detection.
    // The dedup key is `(plan_name, step, task_id)`. A 2nd
    // `work.done` with the same key is rejected as
    // `RecoverableRejection` (NOT fatal) so the runner can
    // re-route to the source hat with a `task.resume` carrying
    // the correct `fix_hint`. The hint distinguishes
    // `duplicate_stall_bypass` (wave_id is set → agent trying
    // to bypass a stalled review cycle) from `duplicate_same_step`
    // (no wave → pure same-step re-emit, fix-round did not
    // advance). The check is applied before all other policy
    // layers so a duplicate is a duplicate regardless of
    // schema/terminal state.
    if topic == "work.done"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        let task_key = obj.get("task_key").and_then(|v| v.as_str());
        if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
            let dedup_key = format!("{pn}::{st}::{ti}");
            // U-fixes-2026-07-04: (task_id, task_key) binding check
            // must come BEFORE dedup. Without it, an agent that
            // changes task_key on retry is misclassified as a
            // duplicate and routed to `task.resume` with no
            // actionable hint. We track the canonical
            // `(task_id) -> task_key` binding seen on the first
            // accept and reject later emits that disagree.
            if let Some(seen_key) = state.work_done_task_id_to_key.get(ti).cloned()
                && let Some(tk) = task_key
                && seen_key != tk
            {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::InvalidFieldValue {
                        field: "task_key".to_string(),
                        value: Value::String(tk.to_string()),
                    },
                    message: format!(
                        "task_id_task_key_mismatch: work.done task_id '{ti}' was first \
                         accepted with task_key '{seen_key}', but this emit uses \
                         task_key '{tk}'. Re-emit with the SAME task_key that \
                         coordinator published in work.ready, OR mint a fresh \
                         task_id via `ralph tools task ensure` before re-sending \
                         work.done."
                    ),
                    evidence: None,
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            if state.work_done_seen_keys.contains(&dedup_key) {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        hint: DuplicateWorkDoneHint::DuplicateSameStep,
                        seen_count: None,
                    },
                    message: format!(
                        "duplicate_same_step: work.done for key '{dedup_key}' was already accepted. \
                         Wait for fix.applied / queue.advance / step close before re-sending work.done \
                         for the same (plan_name, step, task_id)."
                    ),
                    evidence: None,
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            // Record the key so a 3rd emit in the same batch is
            // also rejected. The in-batch set is drained by the
            // event loop after `process_output` completes; the
            // per-loop lifetime set lives in
            // `LoopState::work_done_seen_tasks` and is pruned
            // on step-boundary events.
            state.work_done_seen_keys.insert(dedup_key);
            // Track (task_id) → task_key binding so a later
            // emit with the same task_id but a different
            // task_key is rejected as InvalidFieldValue (not
            // DuplicateWorkDone). Without this, retry storms
            // from agents that swap task_key on re-emit
            // silently cycle through dedup rejections with no
            // actionable hint. Pruned alongside
            // `work_done_seen_tasks` on step boundaries.
            if let Some(tk) = task_key {
                state
                    .work_done_task_id_to_key
                    .insert(ti.to_string(), tk.to_string());
            }
        }
    }

    // U5 (2026-06-17-003 plan, R6): duplicate
    // `review.dimension.ready` detection. The dedup key is
    // `(plan_name, step, task_id, dimension)`. A 2nd
    // `review.dimension.ready` with the same key is rejected
    // as `RejectWithResume` so the runner publishes a
    // `task.resume` with `fix_hint` pointing the agent to wait
    // for the matching `review.dimension.done` /
    // `review.dimension.failed` before re-sending
    // `review.dimension.ready`. The check is applied before
    // schema/terminal layers so a duplicate is a duplicate
    // regardless of state.
    //
    // We reuse the `DuplicateWorkDone` variant (same key/hint
    // shape) rather than introducing a new ViolationType
    // because the recovery flow is identical: both are
    // recoverable rejections that carry a retry-key, and
    // `is_recoverable_policy_finding` already maps the variant
    // to the correct bucket. Adding a new variant would force
    // a parallel `is_recoverable` arm and a parallel
    // `reason_code` mapping for no behavioral gain.
    if topic == "review.dimension.ready"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        let dimension = obj.get("dimension").and_then(|v| v.as_str());
        if let (Some(pn), Some(st), Some(ti), Some(dim)) = (plan_name, step, task_id, dimension) {
            let dedup_key = format!("{pn}::{st}::{ti}::{dim}");
            if state.review_dimension_ready_seen_keys.contains(&dedup_key) {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        // 2026-07-04-024019 run P0-1: distinct hint so
                        // `reason_code` reports `duplicate_review_dimension_ready`
                        // instead of `duplicate_work_done_same_step`.
                        hint: DuplicateWorkDoneHint::ReviewDimensionDuplicate,
                        seen_count: None,
                    },
                    message: format!(
                        "duplicate_dimension_ready: review.dimension.ready for key '{dedup_key}' \
                         was already accepted. Wait for review.dimension.done / \
                         review.dimension.failed for the same dimension before re-sending \
                         review.dimension.ready."
                    ),
                    evidence: None,
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            // Record the key so a 3rd emit in the same batch
            // is also rejected. The in-batch set is drained by
            // the event loop after `process_output` completes;
            // the per-loop lifetime set is populated by
            // `from_events` on restart so cross-batch replays
            // honor the dedup.
            state.review_dimension_ready_seen_keys.insert(dedup_key);
        }
    }

    // Parallel-forge verification is a terminal decision for one concrete
    // wave candidate. Duplicate emits must not fan out another verifier /
    // tester activation, including when the duplicate arrives in a later
    // output batch after the first event has already been persisted.
    if topic == "forge.wave.verified"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_key = obj.get("plan_key").and_then(|v| v.as_str());
        let wave_id = obj.get("wave_id").and_then(|v| v.as_str());
        let candidate = obj.get("candidate_commit_sha").and_then(|v| v.as_str());
        if let (Some(plan_key), Some(wave_id), Some(candidate)) = (plan_key, wave_id, candidate) {
            let dedup_key = format!("{plan_key}::{wave_id}::{candidate}");
            if state.forge_wave_verified_seen_keys.contains(&dedup_key) {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        hint: DuplicateWorkDoneHint::DuplicateSameStep,
                        seen_count: None,
                    },
                    message: format!(
                        "duplicate_forge_wave_verified: forge.wave.verified for key \
                         '{dedup_key}' was already accepted. Emit verification only once \
                         for a wave and candidate commit."
                    ),
                    evidence: None,
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            state.forge_wave_verified_seen_keys.insert(dedup_key);
        }
    }

    // 2026-07-02 P1-A: `review.dimension.failed` schema gate.
    // The dedup / stage gate only checks `(hat, topic)`, not the
    // payload, so a `dimension-reviewer` emit with
    // `dimension=unknown` (or missing the field entirely) would
    // slip through and leave review-coordinator with an unknown
    // dimension to retry. The 6-dimension whitelist mirrors
    // `ce-executor-serial.yml` line 1505-1528
    // (goal-alignment → correctness → testing →
    // maintainability → project-standards → adversarial) so
    // a wrong / missing dimension is rejected as
    // `InvalidFieldValue` instead of surfacing as
    // `flow_unknown_emit` downstream. The check sits in the
    // policy layer (not the flow-scope stage) so the same gate
    // fires for both the in-loop emit path and the
    // CLI precheck emit path; the reason code is
    // `invalid_field_value` so the existing
    // `InvalidFieldValue` recovery hint is reused.
    if topic == "review.dimension.failed"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        const DIMENSION_WHITELIST: &[&str] = &[
            "goal-alignment",
            "correctness",
            "testing",
            "maintainability",
            "project-standards",
            "adversarial",
        ];
        match obj.get("dimension") {
            None => {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::MissingRequiredField {
                        field: "dimension".to_string(),
                    },
                    message: format!(
                        "review.dimension.failed payload is missing required 'dimension' field \
                         (allowed: {})",
                        DIMENSION_WHITELIST.join(", ")
                    ),
                    evidence: None,
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            Some(Value::String(dim)) if !DIMENSION_WHITELIST.contains(&dim.as_str()) => {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::InvalidFieldValue {
                        field: "dimension".to_string(),
                        value: Value::String(dim.clone()),
                    },
                    message: format!(
                        "review.dimension.failed payload has unknown 'dimension' value \
                         '{dim}'; allowed: {}",
                        DIMENSION_WHITELIST.join(", ")
                    ),
                    evidence: None,
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            _ => {} // allowed dimension, fall through.
        }
    }

    // U5 (2026-06-18-004 plan, R4, KTD3) + U6 (2026-06-18-006
    // plan, R6, KTD4): duplicate `review.dimensions.complete`
    // detection. The dedup key is
    // `(plan_name, step, task_id, fix_round)`. A 2nd emit with
    // the same key is rejected as `RejectWithResume` so the
    // runner publishes a `task.resume` with `fix_hint`. The
    // `fix_round` segment distinguishes re-review rounds so a
    // `fix.applied`-pruned bucket (U1) lets a 2nd
    // `review.dimensions.complete` land for `fix_round=N+1`
    // without colliding with `fix_round=N`.
    //
    // U6 (KTD4): `fix_round` is required by the schema
    // (2026-06-18-004 plan U0 made it required). The dedup
    // layer now mirrors that requirement — missing or
    // non-numeric `fix_round` falls through without recording
    // the dedup key, so the schema validator reports
    // `missing_required_field` (or `type_mismatch`) instead of
    // the dedup layer hiding the failure behind
    // `DuplicateWorkDone`. The previous behavior (defaults
    // `0`, silent dedup) masked schema-invalid emits behind a
    // misleading "duplicate" recovery hint.
    //
    // We reuse the `DuplicateWorkDone` variant for parity
    // with the `review.dimension.ready` check above — same
    // recovery shape, same hint bucket.
    if topic == "review.dimensions.complete"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        // U6 (KTD4): only treat the dedup key as a real
        // dimension-complete when `fix_round` is a present
        // u64. Missing or non-numeric `fix_round` falls
        // through — the event will be rejected by the schema
        // layer with `missing_required_field` (or
        // `type_mismatch`), which is the correct error
        // message for the agent. Deduping a schema-invalid
        // event hides the real failure mode behind
        // `DuplicateWorkDone`.
        let fix_round = match obj.get("fix_round") {
            Some(Value::Number(n)) => n.as_u64(),
            _ => None, // missing or non-numeric → None (not Some(0))
        };
        if let (Some(pn), Some(st), Some(ti), Some(fr)) = (plan_name, step, task_id, fix_round) {
            let dedup_key = format!("{pn}::{st}::{ti}::{fr}");
            if state
                .review_dimensions_complete_seen_keys
                .contains(&dedup_key)
            {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        // U6 (plan 2026-07-04-004): switch to the
                        // dedicated `ReviewDimensionsComplete` hint
                        // so the dedup reason code is
                        // `duplicate_review_dimensions_complete`
                        // rather than the misleading generic
                        // `duplicate_work_done`.
                        hint: DuplicateWorkDoneHint::ReviewDimensionsComplete,
                        seen_count: None,
                    },
                    message: format!(
                        "duplicate_dimensions_complete: review.dimensions.complete for key \
                         '{dedup_key}' was already accepted for the same fix_round. \
                         After fix.applied the next round must use fix_round=N+1 and walk \
                         review.dimension.ready first (see U3 obligations)."
                    ),
                    evidence: None,
                };
                // U2 (plan 2026-07-04-004): silently-success
                // `review.dimensions.complete` re-emits must not
                // trigger `task.resume` storms (per
                // `docs/report/2026-07-04-...` silent-success
                // diagnosis). Returning `AcknowledgeAndForward`
                // keeps the dedup invariant (mirror is unchanged)
                // while letting the event reach the bus without
                // injecting a recovery directive. Other dedup
                // branches continue to surface
                // `RejectWithResume` so existing semantics stay
                // intact; this carve-out is intentionally narrow.
                return PolicyDecision::AcknowledgeAndForward(finding);
            }
            state.review_dimensions_complete_seen_keys.insert(dedup_key);
        }
        // else: any of `plan_name`/`step`/`task_id`/`fix_round`
        // missing or non-string/non-u64 → no dedup mirror write,
        // no `DuplicateWorkDone` rejection. The downstream schema
        // validator is responsible for emitting the precise
        // `missing_required_field` / `type_mismatch` message.
    }

    // 2026-06-24 P1-3: duplicate `work.ready` detection. The
    // dedup key is `(plan_name, step, task_id)` — same shape as
    // `work.done`. A 2nd `work.ready` with the same key is
    // rejected as `RejectWithResume` so the agent stops
    // re-announcing an already-started unit. The check fires
    // before schema/terminal layers so a duplicate is a
    // duplicate regardless of state. Pruned on `fix.applied` /
    // step close so a legitimate re-emit after a fix round is
    // allowed.
    if topic == "work.ready"
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        if let (Some(pn), Some(st), Some(ti)) = (plan_name, step, task_id) {
            let dedup_key = format!("{pn}::{st}::{ti}");
            // U5 of plan 2026-07-05-005 (fix-plan §R8): a re-emit
            // after `fix.applied` pruning is allowed (the bucket
            // classification is cleared), but the dedup hit
            // counter must survive — we increment it without
            // rejecting so the dup-storm signal remains
            // observable. The bucket prune marks the key in
            // `pruned_work_ready_buckets`; the check below uses
            // that side-table to accept the emit and bump the
            // counter.
            if state.pruned_work_ready_buckets.contains(&dedup_key) {
                let count = state
                    .work_ready_seen_keys
                    .get(&dedup_key)
                    .copied()
                    .unwrap_or(0);
                state
                    .work_ready_seen_keys
                    .insert(dedup_key.clone(), count.saturating_add(1));
                // Bucket-pruned emit falls through to Accept —
                // count is observation, not dedup state.
            } else if state.work_ready_seen_keys.contains_key(&dedup_key) {
                // U5 of plan 2026-07-05-005 (R8): bump the
                // counter on every observed hit. The counter is
                // observation, not dedup state — `fix.applied`
                // pruning never resets it (see the prune helper
                // below).
                let count = state
                    .work_ready_seen_keys
                    .get(&dedup_key)
                    .copied()
                    .unwrap_or(0);
                state
                    .work_ready_seen_keys
                    .insert(dedup_key.clone(), count.saturating_add(1));
                let hit_count = count.saturating_add(1);
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        hint: DuplicateWorkDoneHint::DuplicateSameStep,
                        seen_count: Some(hit_count),
                    },
                    message: format!(
                        "duplicate_work_ready: work.ready for key '{dedup_key}' was already accepted \
                         (seen_count={hit_count}). Wait for fix.applied / step close before re-sending \
                         work.ready for the same (plan_name, step, task_id)."
                    ),
                    evidence: None,
                };
                return PolicyDecision::RejectWithResume(finding);
            } else {
                // First acceptance: seed the counter at 1 so a
                // subsequent hit reads `seen_count: 2`.
                state.work_ready_seen_keys.insert(dedup_key, 1);
            }
        }
    }

    // 2026-06-24 P1-3: duplicate `test.passed` / `test.failed`
    // detection. The dedup key is
    // `(plan_name, step, task_id, fix_round)` — same shape as
    // `review.dimensions.complete`. A 2nd emit with the same
    // key is rejected as `RejectWithResume`. The `fix_round`
    // segment distinguishes re-test rounds so a
    // `fix.applied`-pruned bucket allows a 2nd `test.passed` /
    // `test.failed` to land for a new fix round without
    // colliding with the prior round's entry.
    //
    // Mirrors the U6 KTD4 rule: missing or non-numeric
    // `fix_round` falls through without recording the dedup
    // key, so the schema validator reports
    // `missing_required_field` (or `type_mismatch`) instead of
    // the dedup layer hiding the failure behind
    // `DuplicateWorkDone`.
    if (topic == "test.passed" || topic == "test.failed")
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let plan_name = obj.get("plan_name").and_then(|v| v.as_str());
        let step = obj.get("step").and_then(|v| v.as_str());
        let task_id = obj.get("task_id").and_then(|v| v.as_str());
        let fix_round = match obj.get("fix_round") {
            Some(Value::Number(n)) => n.as_u64(),
            _ => None,
        };
        if let (Some(pn), Some(st), Some(ti), Some(fr)) = (plan_name, step, task_id, fix_round) {
            let dedup_key = format!("{pn}::{st}::{ti}::{fr}");
            let seen = if topic == "test.passed" {
                state.test_passed_seen_keys.contains(&dedup_key)
            } else {
                state.test_failed_seen_keys.contains(&dedup_key)
            };
            if seen {
                let finding = PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::DuplicateWorkDone {
                        key: dedup_key.clone(),
                        hint: DuplicateWorkDoneHint::DuplicateSameStep,
                        seen_count: None,
                    },
                    message: format!(
                        "duplicate_test_result: {topic} for key '{dedup_key}' was already accepted \
                         for the same fix_round. After fix.applied the next round must use \
                         fix_round=N+1 before re-sending {topic}."
                    ),
                    evidence: None,
                };
                return PolicyDecision::RejectWithResume(finding);
            }
            if topic == "test.passed" {
                state.test_passed_seen_keys.insert(dedup_key);
            } else {
                state.test_failed_seen_keys.insert(dedup_key);
            }
        }
        // else: missing/non-numeric `fix_round` or missing
        // `plan_name`/`step`/`task_id` → no dedup mirror write.
        // The schema validator reports the precise
        // `missing_required_field` / `type_mismatch` error.
    }

    // WAC-U7 R10 (2026-06-12-002): null payloads on the
    // `NULL_PAYLOAD_REJECT_TOPICS` whitelist are hard-rejected
    // with `RejectWithResume`, overriding any `Observe`-mode
    // downgrades. The check is applied before schema
    // validation so a topic without an explicit `schemas`
    // entry still gets the R10 treatment. KTD-9.
    if payload.is_none() && is_null_payload_rejected_topic(topic) {
        let finding = PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::PayloadTypeMismatch {
                expected: "non-null payload".to_string(),
                actual: "null".to_string(),
            },
            message: format!(
                "WAC R10: null payload on whitelist topic `{}` is hard-rejected; \
                 a structured payload is required for this topic",
                topic
            ),
            evidence: None,
        };
        return PolicyDecision::RejectWithResume(finding);
    }

    // Terminal monotonicity check (read-only on state; caller applies terminal_observed)
    if state.terminal_observed && config.business_topics.contains(&topic.to_string()) {
        let terminal_topic = config.terminal_topics.first().cloned().unwrap_or_default();
        findings.push(PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::TerminalMonotonicityViolation {
                terminal_topic: terminal_topic.clone(),
                business_topic: topic.to_string(),
            },
            message: format!(
                "Business event '{}' after terminal topic '{}' violates monotonicity",
                topic, terminal_topic
            ),
            evidence: None,
        });
    }

    // Duplicate terminal check (read-only on state; caller applies terminal_observed)
    if state.terminal_observed && config.terminal_topics.contains(&topic.to_string()) {
        findings.push(PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::DuplicateTerminalEvent {
                topic: topic.to_string(),
            },
            message: format!(
                "Duplicate terminal event '{}' after terminal topic was already observed",
                topic
            ),
            evidence: None,
        });
    }

    // Schema validation
    if let Some(schema) = config.schemas.get(topic) {
        if let Some(expected_type) = &schema.payload
            && matches!(expected_type, PayloadType::JsonObject)
        {
            // WAC-U7 R11 (2026-06-12-002) KTD-10: a string payload
            // that parses to a JSON object is normalized to the
            // serialized object form before required-field
            // validation runs. Non-object strings fall through
            // to the regular type-mismatch finding. The
            // normalized string is captured in
            // `normalized_payload` so the required-fields block
            // below sees the object form.
            let mut normalized_payload: Option<String> = None;
            match payload {
                Some(p) => match serde_json::from_str::<Value>(p) {
                    Ok(Value::Object(map)) => {
                        normalized_payload = Some(
                            serde_json::to_string(&Value::Object(map))
                                .unwrap_or_else(|_| p.to_string()),
                        );
                    }
                    Ok(other) => {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::PayloadTypeMismatch {
                                expected: "json_object".to_string(),
                                actual: format!("{:?}", other),
                            },
                            message: format!("Payload must be JSON object, got {:?}", other),
                            evidence: None,
                        });
                        normalized_payload = Some(p.to_string());
                    }
                    Err(e) => {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::PayloadTypeMismatch {
                                expected: "json_object".to_string(),
                                actual: format!("parse error: {}", e),
                            },
                            message: format!("Payload is not valid JSON: {}", e),
                            evidence: None,
                        });
                        normalized_payload = Some(p.to_string());
                    }
                },
                None => {
                    findings.push(PolicyFinding {
                        topic: topic.to_string(),
                        violation_type: ViolationType::PayloadTypeMismatch {
                            expected: "json_object".to_string(),
                            actual: "null".to_string(),
                        },
                        message: "Payload is required to be JSON object but is missing".to_string(),
                        evidence: None,
                    });
                }
            }

            // Required fields — applied AFTER normalize (KTD-10).
            if !schema.required_fields.is_empty() {
                let payload_for_required = normalized_payload.as_deref().or(payload);
                if let Some(p) = payload_for_required {
                    if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p) {
                        for field in &schema.required_fields {
                            if extract_json_field(&Value::Object(obj.clone()), field).is_none() {
                                findings.push(PolicyFinding {
                                    topic: topic.to_string(),
                                    violation_type: ViolationType::MissingRequiredField {
                                        field: field.clone(),
                                    },
                                    message: format!("Missing required field: {}", field),
                                    evidence: None,
                                });
                            }
                        }
                    }
                } else {
                    // Payload is missing but required fields are specified
                    for field in &schema.required_fields {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::MissingRequiredField {
                                field: field.clone(),
                            },
                            message: format!(
                                "Missing required field '{}' (payload is missing)",
                                field
                            ),
                            evidence: None,
                        });
                    }
                }
            }
        } else {
            // Required fields (no json_object payload requirement)
            if !schema.required_fields.is_empty() {
                if let Some(p) = payload {
                    if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p) {
                        for field in &schema.required_fields {
                            if extract_json_field(&Value::Object(obj.clone()), field).is_none() {
                                findings.push(PolicyFinding {
                                    topic: topic.to_string(),
                                    violation_type: ViolationType::MissingRequiredField {
                                        field: field.clone(),
                                    },
                                    message: format!("Missing required field: {}", field),
                                    evidence: None,
                                });
                            }
                        }
                    }
                } else {
                    for field in &schema.required_fields {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::MissingRequiredField {
                                field: field.clone(),
                            },
                            message: format!(
                                "Missing required field '{}' (payload is missing)",
                                field
                            ),
                            evidence: None,
                        });
                    }
                }
            }
        }

        // String-only field guard (2026-06-24 P0-D regression).
        //
        // `review.complete.fix_plan_file` is documented in the SSOT
        // (`presets/schemas/ce-executor-serial.yml` `review.complete` schema
        // and the ce-executor-serial preset coordinator instructions) as the
        // literal string `"null"` when there are no P0/P1 findings. The
        // 2026-06-24 ralph-e2e run on `python-sort-algorithms` shipped
        // `fix_plan_file: null` (a JSON `null` literal) for the fix-01
        // review round, which slipped through `required_fields` (the field
        // existed), passed through the orchestrator, and broke the
        // downstream coordinator's `fix_plan_file == "null"` string
        // equality check — leaving `plan.complete` un-emitted and the
        // loop stuck for 30+ minutes until progress-steward eventually
        // rescued it.
        //
        // `required_fields` only asserts the field exists; it does NOT
        // assert a JSON value type. This block fills that gap for the
        // single field where the runtime contract is type-strict.
        // `allowed_values` cannot enforce it cleanly because `"null"` is
        // a single-element allowed set — JSON `null` would compare
        // unequal but the runner would never see the violation as a
        // `PayloadTypeMismatch`. A dedicated violation keeps the error
        // message actionable.
        if topic == "review.complete"
            && let Some(p) = payload
            && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
            && let Some(field_value) = obj.get("fix_plan_file")
            && !field_value.is_string()
        {
            findings.push(PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::PayloadTypeMismatch {
                    expected: "string".to_string(),
                    actual: type_name(field_value).to_string(),
                },
                message: format!(
                    "review.complete.fix_plan_file must be a string (use the literal \"null\" for no fix plan), got JSON {}",
                    type_name(field_value)
                ), evidence: None,});
        }

        // Allowed values (hat-agnostic)
        for (field_path, allowed) in &schema.allowed_values {
            if let Some(p) = payload
                && let Ok(value) = serde_json::from_str::<Value>(p)
                && let Some(field_value) = extract_json_field(&value, field_path)
                && !allowed.contains(&field_value)
            {
                findings.push(PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::InvalidFieldValue {
                        field: field_path.clone(),
                        value: field_value.clone(),
                    },
                    message: format!(
                        "Field '{}' has invalid value {:?}. Allowed: {:?}",
                        field_path, field_value, allowed
                    ),
                    evidence: None,
                });
            }
        }

        // Hat-aware allowed values.
        // U1 (2026-06-17-004 plan, R2): fail-closed when provenance is
        // missing and the schema carries per-hat restrictions. Without
        // a known hat, no hat-specific value can be validated, so the
        // event must be rejected — leaving the question of "which hat"
        // to the caller (CLI emit pipeline enforces `check_emit_provenance`
        // before reaching this function; programmatic callers are still
        // required to supply a hat for topics with `hat_allowed_values`).
        //
        // The previous code silently skipped the entire hat-aware block
        // when `hat = None`. That let a hat-less emit bypass the
        // per-hat restriction (e.g. review-coordinator could emit
        // `review.passed(skip_reason=aggregate_timeout)` by dropping
        // the `--hat` flag). This is now a hard `MissingRequiredField`
        // finding — the gate fails closed.
        if schema.hat_allowed_values.is_empty() {
            // No per-hat restrictions on this topic — skip the block.
            // (Implicit: when `hat = None` and no `hat_allowed_values`
            // are configured, nothing to validate.)
        } else if let Some(hat_id) = hat {
            for (field_path, per_hat_rules) in &schema.hat_allowed_values {
                if let Some(rule) = per_hat_rules.iter().find(|r| r.hat_id == hat_id)
                    && let Some(p) = payload
                    && let Ok(value) = serde_json::from_str::<Value>(p)
                    && let Some(field_value) = extract_json_field(&value, field_path)
                    && !rule.values.contains(&field_value)
                {
                    findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::InvalidFieldValue {
                                field: field_path.clone(),
                                value: field_value.clone(),
                            },
                            message: format!(
                                "Hat '{}' may not use value {:?} for field '{}'. Allowed for this hat: {:?}",
                                hat_id, field_value, field_path, rule.values
                            ), evidence: None,});
                }
            }
        } else {
            // Hat is missing but schema has hat-specific allowed values.
            // Without provenance we cannot pick the right rule, so we
            // emit a single finding that names the topic + the per-hat
            // restrictions. The CLI emit pipeline's
            // `check_emit_provenance` rejects this event earlier; this
            // finding covers programmatic callers (API server,
            // in-process emitters) that go straight to
            // `validate_event_with_hat`.
            let mut per_hat_summary: Vec<String> = Vec::new();
            for (field_path, per_hat_rules) in &schema.hat_allowed_values {
                for rule in per_hat_rules {
                    per_hat_summary.push(format!(
                        "hat='{}' field='{}' allowed={:?}",
                        rule.hat_id, field_path, rule.values
                    ));
                }
            }
            findings.push(PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::MissingRequiredField {
                    field: "hat".to_string(),
                },
                message: format!(
                    "Topic '{topic}' has hat-specific allowed values; a hat is required \
                     to validate the payload. Provenance rules: {per_hat_summary:?}. \
                     Pass --hat <hat-id> or set RALPH_CURRENT_HAT=<hat-id>."
                ),
                evidence: None,
            });
        }
    }

    // U4: plan_name equality — when enabled, work.done's plan_name must equal
    // the current_plan_name extracted from the most recent work.ready event.
    if config.plan_name_equality_required
        && topic == "work.done"
        && let Some(expected) = &state.current_plan_name
        && let Some(p) = payload
        && let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(p)
    {
        let actual = obj.get("plan_name").and_then(|v| v.as_str());
        if actual != Some(expected.as_str()) {
            findings.push(PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::InvalidFieldValue {
                    field: "plan_name".to_string(),
                    value: actual
                        .map(|s| Value::String(s.to_string()))
                        .unwrap_or(Value::Null),
                },
                message: format!(
                    "work.done plan_name mismatch: expected '{}', got {:?}",
                    expected,
                    actual.unwrap_or("(missing)")
                ),
                evidence: None,
            });
        }
    }

    // U7 of plan 2026-07-02-005: runtime shipper strict-match backstop.
    if topic == "REVIEW_COMPLETE"
        && let Some(finding) = crate::shipper_reason::check_review_complete_shipper_routing(
            payload,
            state.last_plan_blocked_reason.as_deref(),
        )
    {
        findings.push(finding);
    }

    // 2026-07-03-005 plan (P0 fix C7): per-element shape validation for
    // array fields declared in the schema's `element_constraints` map.
    // Today this single-handedly closes the
    // `review.dimensions.complete` silent-drop bug — when the agent
    // fabricates a `status: done` element with a null findings_file,
    // the schema rejects the element and the runtime surfaces the
    // real cause instead of accepting the inflated review summary.
    if !config.schemas.is_empty()
        && let Some(p) = payload
        && let Ok(Value::Object(_)) = serde_json::from_str::<Value>(p)
    {
        let topic_schema = config.schemas.get(topic);
        if let Some(schema) = topic_schema
            && !schema.element_constraints.is_empty()
            && let Some(value) = serde_json::from_str::<Value>(p).ok()
        {
            for (array_field, constraint) in &schema.element_constraints {
                if let Some(field) = obj_get(&value, array_field) {
                    if let Value::Array(elements) = field {
                        for (idx, element) in elements.iter().enumerate() {
                            if let Some(finding) =
                                validate_element_shape(topic, array_field, idx, element, constraint)
                            {
                                findings.push(finding);
                            }
                        }
                    } else {
                        findings.push(PolicyFinding {
                            topic: topic.to_string(),
                            violation_type: ViolationType::PayloadTypeMismatch {
                                expected: "array".to_string(),
                                actual: type_name(field).to_string(),
                            },
                            message: format!(
                                "element_constraints: field '{}' must be an array, got {}",
                                array_field,
                                type_name(field)
                            ),
                            evidence: None,
                        });
                    }
                }
            }
        }
    }

    // 2026-07-06-004 plan U8: handoff envelope validation. When
    // `event_loop.handoff_envelope.enabled` is on AND
    // `validate_payload` is on, every business event whose
    // payload parses as a JSON object must contain a valid
    // `payload.handoff_envelope` (per `handoff_envelope.v1`).
    // Runtime-injected control topics (`task.resume`) skip
    // the check — the recovery path is synthesised by the
    // runner and cannot carry an agent-authored envelope.
    // The check is gated on the typed config so non-serial
    // presets and ad-hoc loops are unaffected (regression
    // defence #5). When the flag fires for a `task.resume`
    // it would otherwise deadlock the recovery channel.
    if handoff_config.handoff_envelope_enabled()
        && handoff_config.handoff_envelope_validate_payload()
        && topic != "task.resume"
        && let Some(p) = payload
    {
        match serde_json::from_str::<Value>(p) {
            Ok(value) => {
                if let Some(finding) = check_handoff_envelope(topic, &value) {
                    findings.push(finding);
                }
            }
            Err(_) => {
                // If the payload does not parse as JSON we
                // don't add a finding here — earlier
                // validation layers will surface that.
            }
        }
    }

    // U3 (plan 2026-07-22-004): opt-in same-payload consistency gates.
    // After schema / allowed-values / hat-aware / element_constraints
    // checks have gathered their findings, evaluate any enabled
    // `payload_consistency` rule whose `topic` matches the current
    // topic against the CURRENT payload only (R2 — never event
    // history). The first hit in stable declaration order is surfaced
    // as a `SemanticGateViolation` with gate `payload_consistency:<id>`;
    // the decision mapper below takes `findings.into_iter().next()`, so
    // we push only the first hit and break (simplest correct approach,
    // preserves declaration order). Reuses the existing
    // `ViolationType::SemanticGateViolation` variant (KTD3) — the
    // `payload_consistency:` gate prefix distinguishes it from
    // timing/state semantic gates. A missing or non-object payload
    // cannot satisfy a field predicate, so it is treated as no-hit
    // (NOT an error) — schema validation already handles payload shape.
    if config.payload_consistency.enabled
        && let Some(p) = payload
        && let Ok(value) = serde_json::from_str::<Value>(p)
        && value.is_object()
    {
        for rule in &config.payload_consistency.rules {
            if rule.topic != topic {
                continue;
            }
            if crate::event_policy_payload_consistency::evaluate(&rule.when, &value)
                == crate::event_policy_payload_consistency::EvalOutcome::Hit
            {
                let gate = format!("payload_consistency:{}", rule.id);
                // U2 (2026-07-23-002 plan, KTD2): collect the stable,
                // declaration-order set of business fields the rule's
                // predicate AST references so agent repair tooling can
                // know which payload fields to inspect without parsing
                // `rule.message`. This is the static declared set, not
                // the short-circuited "matched" subset.
                let referenced_fields =
                    crate::event_policy_payload_consistency::collect_referenced_fields(&rule.when);
                // U2 (plan 2026-08-06-001, D6): pull bounded field
                // observations from the current payload so the
                // correction prompt and the CLI --policy-check JSON
                // share one source of "this is what the payload
                // actually said when the rule fired".  Failures to
                // serialise a value are downgraded to
                // `ObservationValue::Unavailable` — never invented.
                let observed = crate::event_policy_payload_consistency::observe_referenced_fields(
                    &rule.when, &value,
                );
                let observations = observed
                    .into_iter()
                    .map(|(field, v)| crate::correction::ObservationEntry { field, value: v })
                    .collect();
                let evidence = crate::correction::EvidenceDetail {
                    observed: observations,
                    invariant: rule.message.clone(),
                    proof: format!(
                        "Rebuild the payload from the artifact so {topic} satisfies the rule (run `ralph emit {topic} --policy-check` to re-validate before re-emitting)."
                    ),
                    synthetic: false,
                    // U4 (plan 2026-08-17-1841, R1/R4/D1): thread
                    // the rule's preset-supplied recovery guidance
                    // into the evidence so the correction renderer
                    // surfaces `common` and the `by_check[<rule
                    // id>]` items at the target hat's prompt.
                    // `failed_check_keys` stays `None` so the
                    // renderer falls back to "render every
                    // `by_check` key" (the consistency path never
                    // reports a list of failed checks — only the
                    // matched rule id, which is encoded by the
                    // rule id the evaluator already selected).
                    guidance: rule.recovery_guidance.clone(),
                    failed_check_keys: None,
                };
                findings.push(PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::SemanticGateViolation {
                        gate: gate.clone(),
                        context: rule.message.clone(),
                        referenced_fields,
                    },
                    message: format!("{gate}: {}", rule.message),
                    evidence: Some(evidence),
                });
                break;
            }
        }
    }

    // Stateful red-team queue gate. This intentionally runs after the
    // declarative same-payload rules so an existing local payload error keeps
    // its stable finding code; cross-event queue errors are added only when
    // the current payload is otherwise structurally usable.
    if let Some(finding) = check_redteam_queue_invariant(topic, payload, config, state) {
        findings.push(finding);
    }

    if findings.is_empty() {
        apply_redteam_queue_state(topic, payload, state);
        if topic == "plan.blocked" {
            state.last_plan_blocked_reason =
                crate::shipper_reason::extract_plan_blocked_reason(payload);
        } else if topic == "plan.complete" {
            state.last_plan_blocked_reason = None;
        }
        return PolicyDecision::Accept;
    }

    // Observe/Warn modes still forward the event, so the stateful queue mirror
    // must advance with the accepted event just as it does on a clean accept.
    if matches!(config.mode, EventPolicyMode::Observe)
        || (matches!(config.mode, EventPolicyMode::Enforce)
            && matches!(config.on_violation, ViolationAction::Warn))
    {
        apply_redteam_queue_state(topic, payload, state);
    }

    match config.mode {
        EventPolicyMode::Observe => PolicyDecision::Warn(findings),
        EventPolicyMode::Enforce => match config.on_violation {
            ViolationAction::Warn => PolicyDecision::Warn(findings),
            ViolationAction::RejectWithResume => {
                PolicyDecision::RejectWithResume(findings.into_iter().next().unwrap())
            }
            ViolationAction::Hold => PolicyDecision::Hold(findings.into_iter().next().unwrap()),
            ViolationAction::Block => PolicyDecision::Block(findings.into_iter().next().unwrap()),
        },
    }
}

/// Extract a nested field from a JSON value using dot notation.
///
/// Shared with the payload-consistency evaluator and lint. Keep this
/// single implementation; do not re-introduce local copies.
pub(crate) fn extract_json_field(value: &Value, path: &str) -> Option<Value> {
    let mut current = value;
    for part in path.split('.') {
        match current {
            Value::Object(obj) => {
                current = obj.get(part)?;
            }
            _ => return None,
        }
    }
    Some(current.clone())
}

/// Human-friendly name for a JSON value's runtime type.
/// Used by the 2026-06-24 P0-D `review.complete.fix_plan_file` string-only
/// guard to produce actionable error messages.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// 2026-07-03-005 plan (P0 fix C7): look up a top-level field on a JSON
/// value (avoids the dot-notation `extract_json_field` semantics, which
/// would split on `.` and is wrong for array element field names that
/// may contain dots in the future). Returns `Some(value)` for both
/// present-and-null and present-with-value.
fn obj_get<'a>(value: &'a Value, field: &str) -> Option<&'a Value> {
    value.as_object()?.get(field)
}

/// 2026-07-03-005 plan (P0 fix C7): validate one element of an array
/// field against its `ElementConstraint`. Returns `Some(PolicyFinding)`
/// on the first violation per element, or `None` if the element
/// passes. The constraint covers: existence (`required`), value
/// restriction (`allowed_values`), conditional existence
/// (`required_when` + `forbid_null_when_required`).
fn validate_element_shape(
    topic: &str,
    array_field: &str,
    idx: usize,
    element: &Value,
    constraint: &crate::config::ElementConstraint,
) -> Option<PolicyFinding> {
    // 1. required field exists
    let present = obj_get(element, &constraint.field);
    if constraint.required && present.is_none() {
        return Some(PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::MissingRequiredField {
                field: format!("{}[{}].{}", array_field, idx, constraint.field),
            },
            message: format!(
                "element_constraints: {}[{}] is missing required field '{}'",
                array_field, idx, constraint.field
            ),
            evidence: None,
        });
    }

    // 2. allowed_values check
    if !constraint.allowed_values.is_empty()
        && let Some(value) = present
        && !constraint.allowed_values.iter().any(|v| v == value)
    {
        return Some(PolicyFinding {
            topic: topic.to_string(),
            violation_type: ViolationType::InvalidFieldValue {
                field: format!("{}[{}].{}", array_field, idx, constraint.field),
                value: value.clone(),
            },
            message: format!(
                "element_constraints: {}[{}].{} = {} not in allowed list {:?}",
                array_field,
                idx,
                constraint.field,
                type_name(value),
                constraint.allowed_values
            ),
            evidence: None,
        });
    }

    // 3. required_when + forbid_null_when_required
    if !constraint.required_when.is_empty() {
        let mut all_conditions_match = true;
        for (key, expected) in &constraint.required_when {
            let actual = obj_get(element, key);
            if actual != Some(expected) {
                all_conditions_match = false;
                break;
            }
        }
        if all_conditions_match {
            if present.is_none() {
                return Some(PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::MissingRequiredField {
                        field: format!("{}[{}].{}", array_field, idx, constraint.field),
                    },
                    message: format!(
                        "element_constraints: {}[{}].{} is required when sibling conditions {:?} match",
                        array_field, idx, constraint.field, constraint.required_when
                    ),
                    evidence: None,
                });
            }
            if constraint.forbid_null_when_required && matches!(present, Some(Value::Null)) {
                return Some(PolicyFinding {
                    topic: topic.to_string(),
                    violation_type: ViolationType::InvalidFieldValue {
                        field: format!("{}[{}].{}", array_field, idx, constraint.field),
                        value: Value::Null,
                    },
                    message: format!(
                        "element_constraints: {}[{}].{} is null but must be non-null when sibling conditions {:?} match",
                        array_field, idx, constraint.field, constraint.required_when
                    ),
                    evidence: None,
                });
            }
        }
    }

    None
}

// ─────────────────────────────────────────────────────────────────────
// Unit 2 of plan 2026-07-27-002: read-only `evaluate_candidate_emit`
// preview for the `ralph inspect prompt --topic` path.
