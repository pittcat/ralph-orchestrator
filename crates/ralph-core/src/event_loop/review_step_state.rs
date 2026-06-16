//! Per-step review terminal state for plan-gate hard enforcement (U1/U3).

use crate::event_policy::{PolicyFinding, ViolationType};
use crate::event_reader::Event as JsonlEvent;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Emitted when a review wave exceeds the synthesizer aggregate window (U4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggregateTimeoutAction {
    pub plan_name: String,
    pub task_id: String,
    pub step: String,
    pub wave_id: String,
    pub received: u32,
    pub expected: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StepKey {
    plan_name: String,
    task_id: String,
    step: String,
}

#[derive(Debug, Clone, Default)]
struct StepReviewState {
    open_wave_id: Option<String>,
    wave_expected: u32,
    wave_started_at: Option<Instant>,
    aggregate_timeout_dispatched: bool,
    dimensions_received: HashSet<String>,
    synth_terminal: Option<String>,
    synth_pass: bool,
    failed_pending_fix: bool,
}

#[derive(Debug, Default)]
pub struct ReviewStepTracker {
    steps: HashMap<StepKey, StepReviewState>,
}

fn step_key_from_event(topic: &str, payload: Option<&str>) -> Option<StepKey> {
    let p = payload?;
    let obj = serde_json::from_str::<Value>(p).ok()?;
    let plan_name = obj.get("plan_name")?.as_str()?.to_string();
    match topic {
        "queue.advance" | "work.ready" => {
            // Step-advance handoffs from plan-gate carry reviewed-step
            // correlation fields; coordinator's initial work.ready does not.
            if let Some(task_id) = obj.get("reviewed_task_id").and_then(|v| v.as_str()) {
                let step = obj.get("completed_step")?.as_str()?.to_string();
                return Some(StepKey {
                    plan_name,
                    task_id: task_id.to_string(),
                    step,
                });
            }
            if topic == "queue.advance" {
                return None;
            }
            let task_id = obj.get("task_id")?.as_str()?.to_string();
            let step = obj.get("step")?.as_str()?.to_string();
            Some(StepKey {
                plan_name,
                task_id,
                step,
            })
        }
        _ => {
            let task_id = obj.get("task_id")?.as_str()?.to_string();
            let step = obj.get("step")?.as_str()?.to_string();
            Some(StepKey {
                plan_name,
                task_id,
                step,
            })
        }
    }
}

fn plan_gate_step_gate(topic: &str, state: &StepReviewState) -> Option<PolicyFinding> {
    if state.failed_pending_fix {
        return Some(plan_gate_finding(
            topic,
            "plan_gate_review_failed_pending_fix",
        ));
    }
    let terminal_ok = state
        .synth_terminal
        .as_deref()
        .is_some_and(|t| matches!(t, "review.passed" | "review.complete") && state.synth_pass);
    if !terminal_ok {
        Some(plan_gate_finding(topic, "plan_gate_review_not_terminal"))
    } else {
        None
    }
}

fn wave_open(state: &StepReviewState) -> bool {
    state.open_wave_id.is_some()
        && (state.wave_expected == 0
            || (state.dimensions_received.len() as u32) < state.wave_expected)
}

fn plan_gate_finding(topic: &str, reason: &str) -> PolicyFinding {
    PolicyFinding {
        topic: topic.to_string(),
        violation_type: ViolationType::BusinessEventAfterCompletion {
            topic: topic.to_string(),
        },
        message: format!(
            "{reason}: cannot emit '{topic}' until review-synthesizer terminal \
             (review.passed or review.complete with pass verdict) for this step"
        ),
    }
}

impl ReviewStepTracker {
    /// Semantic gates that run after schema validation (U1/U3).
    pub fn check_semantic_gates(&self, event: &JsonlEvent) -> Option<PolicyFinding> {
        let hat = event.hat.as_deref().unwrap_or("");
        let topic = event.topic.as_str();

        if hat == "review-coordinator" && topic == "review.passed" {
            if let Some(key) = step_key_from_event(topic, event.payload.as_deref()) {
                if let Some(state) = self.steps.get(&key)
                    && wave_open(state)
                {
                    return Some(PolicyFinding {
                        topic: topic.to_string(),
                        violation_type: ViolationType::InvalidFieldValue {
                            field: "skip_reason".to_string(),
                            value: Value::String("review_passed_while_wave_open".to_string()),
                        },
                        message: format!(
                            "review_passed_while_wave_open: review-coordinator must not emit \
                             review.passed while wave '{}' is incomplete ({}/{} dimensions)",
                            state.open_wave_id.as_deref().unwrap_or("?"),
                            state.dimensions_received.len(),
                            state.wave_expected
                        ),
                    });
                }
            }
        }

        if topic == "review.passed"
            && let Some(p) = event.payload.as_deref()
            && let Ok(Value::Object(obj)) = serde_json::from_str(p)
            && obj.get("skip_reason").and_then(|v| v.as_str()) == Some("aggregate_timeout")
            && hat != "review-synthesizer"
        {
            return Some(PolicyFinding {
                topic: topic.to_string(),
                violation_type: ViolationType::InvalidFieldValue {
                    field: "skip_reason".to_string(),
                    value: Value::String("aggregate_timeout".to_string()),
                },
                message: "aggregate_timeout skip_reason is reserved for review-synthesizer"
                    .to_string(),
            });
        }

        if topic == "queue.advance" {
            let key = step_key_from_event(topic, event.payload.as_deref())?;
            let Some(state) = self.steps.get(&key) else {
                return Some(plan_gate_finding(topic, "plan_gate_review_not_terminal"));
            };
            return plan_gate_step_gate(topic, state);
        }

        if topic == "work.ready" {
            let p = event.payload.as_deref()?;
            let obj = serde_json::from_str::<Value>(p).ok()?;
            // Coordinator bootstrap work.ready has no reviewed-step correlation;
            // only step-advance handoffs from plan-gate are gated.
            if obj
                .get("reviewed_task_id")
                .and_then(|v| v.as_str())
                .is_none()
            {
                return None;
            }
            let key = step_key_from_event(topic, event.payload.as_deref())?;
            let Some(state) = self.steps.get(&key) else {
                return Some(plan_gate_finding(topic, "plan_gate_review_not_terminal"));
            };
            return plan_gate_step_gate(topic, state);
        }

        if topic == "plan.complete" {
            let p = event.payload.as_deref()?;
            let obj = serde_json::from_str::<Value>(p).ok()?;
            let plan_name = obj.get("plan_name")?.as_str()?;
            let task_id = obj.get("task_id")?.as_str()?;
            let matching: Vec<_> = self
                .steps
                .iter()
                .filter(|(k, _)| k.plan_name == plan_name && k.task_id == task_id)
                .collect();
            if matching.is_empty() {
                return Some(plan_gate_finding(topic, "plan_gate_review_not_terminal"));
            }
            if matching.iter().any(|(_, s)| s.failed_pending_fix) {
                return Some(plan_gate_finding(
                    topic,
                    "plan_gate_review_failed_pending_fix",
                ));
            }
            let terminal_ok = matching.iter().all(|(_, s)| {
                s.synth_terminal.as_deref().is_some_and(|t| {
                    matches!(t, "review.passed" | "review.complete") && s.synth_pass
                })
            });
            if !terminal_ok {
                return Some(plan_gate_finding(topic, "plan_gate_review_not_terminal"));
            }
        }

        None
    }

    /// Update step state after an event passes all validation layers.
    pub fn observe_accepted(&mut self, event: &JsonlEvent) {
        let hat = event.hat.as_deref().unwrap_or("");
        let topic = event.topic.as_str();

        if matches!(topic, "plan.complete" | "queue.advance") {
            return;
        }

        let Some(key) = step_key_from_event(topic, event.payload.as_deref()) else {
            return;
        };
        let state = self.steps.entry(key).or_default();

        match topic {
            "review.wave.ready" => {
                state.open_wave_id = event.wave_id.clone();
                state.wave_expected = event.wave_total.unwrap_or(0);
                state.wave_started_at = Some(Instant::now());
                state.aggregate_timeout_dispatched = false;
                state.dimensions_received.clear();
            }
            "review.dimension.done" => {
                if let Some(open) = &state.open_wave_id
                    && event.wave_id.as_ref() != Some(open)
                {
                    return;
                }
                if let Some(p) = event.payload.as_deref()
                    && let Ok(Value::Object(obj)) = serde_json::from_str(p)
                    && let Some(dim) = obj.get("dimension").and_then(|v| v.as_str())
                {
                    state.dimensions_received.insert(dim.to_string());
                }
                if state.wave_expected > 0
                    && state.dimensions_received.len() as u32 >= state.wave_expected
                {
                    state.open_wave_id = None;
                }
            }
            "review.passed" | "review.complete" => {
                let pass = event
                    .payload
                    .as_deref()
                    .and_then(|p| serde_json::from_str::<Value>(p).ok())
                    .and_then(|obj| {
                        obj.get("verdict")
                            .and_then(|v| v.as_str())
                            .map(|v| v != "fail")
                    })
                    .unwrap_or(true);
                if hat == "review-coordinator" && wave_open(state) {
                    return;
                }
                state.synth_terminal = Some(topic.to_string());
                state.synth_pass = pass;
                state.open_wave_id = None;
            }
            "review.failed" => {
                state.failed_pending_fix = true;
                state.synth_terminal = None;
                state.synth_pass = false;
            }
            "fix.applied" => {
                state.failed_pending_fix = false;
            }
            _ => {}
        }
    }

    /// True when any tracked step still has an incomplete review wave.
    pub fn has_open_review_wave(&self) -> bool {
        self.steps.values().any(wave_open)
    }

    /// Steps whose review wave exceeded `timeout` without receiving all dimensions (U4).
    pub fn drain_expired_aggregate_timeouts(
        &mut self,
        timeout: Duration,
    ) -> Vec<AggregateTimeoutAction> {
        let now = Instant::now();
        let mut actions = Vec::new();
        for (key, state) in &mut self.steps {
            if !wave_open(state) || state.aggregate_timeout_dispatched {
                continue;
            }
            let Some(started) = state.wave_started_at else {
                continue;
            };
            if now.duration_since(started) <= timeout {
                continue;
            }
            state.aggregate_timeout_dispatched = true;
            actions.push(AggregateTimeoutAction {
                plan_name: key.plan_name.clone(),
                task_id: key.task_id.clone(),
                step: key.step.clone(),
                wave_id: state.open_wave_id.clone().unwrap_or_default(),
                received: state.dimensions_received.len() as u32,
                expected: state.wave_expected,
            });
        }
        actions
    }

    #[cfg(test)]
    fn backdate_open_wave_for_test(
        &mut self,
        plan_name: &str,
        task_id: &str,
        step: &str,
        ago: Duration,
    ) {
        let key = StepKey {
            plan_name: plan_name.to_string(),
            task_id: task_id.to_string(),
            step: step.to_string(),
        };
        if let Some(state) = self.steps.get_mut(&key) {
            state.wave_started_at =
                Some(Instant::now().checked_sub(ago).unwrap_or_else(Instant::now));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        EventPolicyConfig, EventPolicyMode, EventSchema, PayloadType, ViolationAction,
    };
    use crate::event_policy::{PolicyDecision, PolicyRuntimeState, validate_event};
    use std::collections::HashMap;

    fn jsonl(topic: &str, hat: &str, payload: &str) -> JsonlEvent {
        JsonlEvent {
            topic: topic.to_string(),
            payload: Some(payload.to_string()),
            ts: String::new(),
            hat: Some(hat.to_string()),
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
        }
    }

    fn ce_executor_schemas() -> EventPolicyConfig {
        let mut config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::RejectWithResume,
            schemas: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert(
            "review.passed".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![
                    "plan_name".into(),
                    "task_id".into(),
                    "task_key".into(),
                    "step".into(),
                    "findings_count".into(),
                    "fix_round".into(),
                    "verdict".into(),
                    "skip_reason".into(),
                ],
                allowed_values: HashMap::new(),
                hat_allowed_values: HashMap::new(),
            },
        );
        config.schemas.insert(
            "review.failed".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![
                    "plan_name".into(),
                    "fix_round".into(),
                    "safe_auto_count".into(),
                    "gated_manual_count".into(),
                    "findings_summary".into(),
                    "task_id".into(),
                    "task_key".into(),
                    "step".into(),
                ],
                allowed_values: HashMap::new(),
                hat_allowed_values: HashMap::new(),
            },
        );
        config
    }

    #[test]
    fn plan_complete_rejected_without_synth_terminal() {
        let mut tracker = ReviewStepTracker::default();
        let passed = jsonl(
            "review.passed",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
        );
        let plan_complete = jsonl(
            "plan.complete",
            "plan-gate",
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","verdict":"pass"}"#,
        );

        tracker.observe_accepted(&passed);
        assert!(tracker.check_semantic_gates(&plan_complete).is_none());

        let tracker2 = ReviewStepTracker::default();
        let finding = tracker2
            .check_semantic_gates(&plan_complete)
            .expect("must reject");
        assert!(finding.message.contains("plan_gate_review_not_terminal"));
    }

    #[test]
    fn session_b_incomplete_passed_rejected_by_schema() {
        let config = ce_executor_schemas();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","skip_reason":"empty_diff"}"#;
        let decision = validate_event("review.passed", Some(payload), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn session_b_string_failed_rejected_by_schema() {
        let config = ce_executor_schemas();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event(
            "review.failed",
            Some("Review failed due to critical issues in src/lib.rs"),
            &config,
            &mut state,
        );
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn coordinator_passed_while_wave_open_rejected() {
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(3);
        tracker.observe_accepted(&wave);

        let passed = jsonl(
            "review.passed",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
        );
        let finding = tracker.check_semantic_gates(&passed).expect("must reject");
        assert!(finding.message.contains("review_passed_while_wave_open"));
    }

    #[test]
    fn failed_then_passed_blocks_plan_complete() {
        let mut tracker = ReviewStepTracker::default();
        let failed = jsonl(
            "review.failed",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","fix_round":0,"safe_auto_count":1,"gated_manual_count":0,"findings_summary":"x"}"#,
        );
        let passed = jsonl(
            "review.passed",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
        );
        let plan_complete = jsonl(
            "plan.complete",
            "plan-gate",
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","verdict":"pass"}"#,
        );

        tracker.observe_accepted(&failed);
        tracker.observe_accepted(&passed);
        let finding = tracker
            .check_semantic_gates(&plan_complete)
            .expect("must reject");
        assert!(
            finding
                .message
                .contains("plan_gate_review_failed_pending_fix")
        );
    }

    #[test]
    fn queue_advance_rejected_without_review_state() {
        let tracker = ReviewStepTracker::default();
        let advance = jsonl(
            "queue.advance",
            "plan-gate",
            r#"{"plan_name":"p","completed_step":"1","next_step":"2","reviewed_task_id":"t1","reviewed_task_key":"k1"}"#,
        );
        let finding = tracker.check_semantic_gates(&advance).expect("must reject");
        assert!(finding.message.contains("plan_gate_review_not_terminal"));
    }

    #[test]
    fn work_ready_step_advance_rejected_without_synth_terminal() {
        let tracker = ReviewStepTracker::default();
        let ready = jsonl(
            "work.ready",
            "plan-gate",
            r#"{"plan_name":"p","plan_path":"docs/plans/p.md","task_id":"t2","task_key":"k2","step":"2","complexity":"small","reviewed_task_id":"t1","reviewed_task_key":"k1","completed_step":"1","next_step":"2"}"#,
        );
        let finding = tracker
            .check_semantic_gates(&ready)
            .expect("must reject step-advance work.ready without synth terminal");
        assert!(finding.message.contains("plan_gate_review_not_terminal"));
    }

    #[test]
    fn work_ready_step_advance_allowed_after_synth_terminal() {
        let mut tracker = ReviewStepTracker::default();
        let passed = jsonl(
            "review.passed",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
        );
        tracker.observe_accepted(&passed);

        let advance = jsonl(
            "queue.advance",
            "plan-gate",
            r#"{"plan_name":"p","completed_step":"1","next_step":"2","reviewed_task_id":"t1","reviewed_task_key":"k1"}"#,
        );
        let ready = jsonl(
            "work.ready",
            "plan-gate",
            r#"{"plan_name":"p","plan_path":"docs/plans/p.md","task_id":"t2","task_key":"k2","step":"2","complexity":"small","reviewed_task_id":"t1","reviewed_task_key":"k1","completed_step":"1","next_step":"2"}"#,
        );

        assert!(
            tracker.check_semantic_gates(&advance).is_none(),
            "queue.advance must pass after synth terminal"
        );
        assert!(
            tracker.check_semantic_gates(&ready).is_none(),
            "work.ready handoff must pass after synth terminal (P1 / merry-wren fix)"
        );
    }

    #[test]
    fn coordinator_initial_work_ready_not_gated_by_review_state() {
        let tracker = ReviewStepTracker::default();
        let ready = jsonl(
            "work.ready",
            "coordinator",
            r#"{"plan_name":"p","plan_path":"docs/plans/p.md","task_id":"t1","task_key":"k1","step":"1","complexity":"small"}"#,
        );
        assert!(
            tracker.check_semantic_gates(&ready).is_none(),
            "coordinator bootstrap work.ready must not require prior synth terminal"
        );
    }

    #[test]
    fn expired_open_wave_surfaces_aggregate_timeout_action() {
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(3);
        tracker.observe_accepted(&wave);

        let mut dim = jsonl(
            "review.dimension.done",
            "dimension-reviewer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec","findings_count":0,"findings_file":"f.json"}"#,
        );
        dim.wave_id = Some("w-1".to_string());
        tracker.observe_accepted(&dim);

        tracker.backdate_open_wave_for_test("p", "t1", "1", Duration::from_secs(301));

        let actions = tracker.drain_expired_aggregate_timeouts(Duration::from_secs(300));
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].received, 1);
        assert_eq!(actions[0].expected, 3);
        assert_eq!(actions[0].wave_id, "w-1");
        assert!(
            tracker
                .drain_expired_aggregate_timeouts(Duration::from_secs(300))
                .is_empty(),
            "second drain must be idempotent"
        );
    }

    /// Step-handoff (2026-06-17-002) U5: a null `review.passed`
    /// payload is hard-rejected by `validate_event_with_hat` with
    /// `RejectWithResume` and a WAC R10 finding. The state machine
    /// never sees this event, so `synth_terminal` stays unset and
    /// any subsequent `plan.complete` stays blocked. This test
    /// pins the end-to-end hard gate at the policy boundary.
    #[test]
    fn step_handoff_u5_review_passed_null_rejected_by_policy() {
        let config = ce_executor_schemas();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("review.passed", None, &config, &mut state);
        match decision {
            PolicyDecision::RejectWithResume(finding) => {
                assert!(
                    finding.message.contains("WAC R10")
                        || finding.message.contains("null payload"),
                    "review.passed null must be flagged with WAC R10 finding, got: {}",
                    finding.message
                );
            }
            other => panic!(
                "review.passed null must RejectWithResume, got {:?}",
                other
            ),
        }
    }

    /// Step-handoff U5: `observe_accepted` is a no-op when the
    /// payload is missing or unparseable (step_key_from_event
    /// returns None). So even if a null `review.passed` ever
    /// leaks past the policy gate, the state machine cannot
    /// accidentally set `synth_terminal` from it.
    #[test]
    fn step_handoff_u5_review_passed_null_does_not_set_synth_terminal() {
        let mut tracker = ReviewStepTracker::default();

        // (1) A null-payload review.passed routed into the state
        // machine must be a no-op (no step_key, no state mutation).
        let null_passed = JsonlEvent {
            topic: "review.passed".to_string(),
            payload: None,
            ts: String::new(),
            hat: Some("review-synthesizer".to_string()),
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
        };
        tracker.observe_accepted(&null_passed);

        // (2) After the no-op, plan.complete is still blocked
        // because synth_terminal was never set.
        let plan_complete_blocked = jsonl(
            "plan.complete",
            "plan-gate",
            r#"{"plan_name":"p","completed_steps":1,"task_id":"t1","task_key":"k1","verdict":"pass"}"#,
        );
        let finding = tracker
            .check_semantic_gates(&plan_complete_blocked)
            .expect("plan.complete must stay blocked when synth_terminal was never set");
        assert!(
            finding.message.contains("plan_gate_review_not_terminal"),
            "expected plan_gate_review_not_terminal, got: {}",
            finding.message
        );

        // (3) A subsequent well-formed review.passed unlocks the
        // gate cleanly — the no-op did not corrupt state.
        let passed = jsonl(
            "review.passed",
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
        );
        tracker.observe_accepted(&passed);
        assert!(
            tracker.check_semantic_gates(&plan_complete_blocked).is_none(),
            "synth_terminal must be set after a well-formed review.passed, \
             so plan.complete must pass the gate"
        );
    }
}
