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
pub struct StepKey {
    pub plan_name: String,
    pub task_id: String,
    pub step: String,
}

/// U3 (2026-06-17-003 plan): minimal projection of an open wave
/// used by `publish_policy_rejection_resume` to print the
/// `## WAVE_OPEN HINT` block. Carries no time information — the
/// textual hint only needs the wave id + receive/total counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenWaveSnapshot {
    pub wave_id: String,
    pub received: u32,
    pub expected: u32,
}

#[derive(Debug, Clone, Default)]
struct StepReviewState {
    open_wave_id: Option<String>,
    wave_expected: u32,
    wave_started_at: Option<Instant>,
    /// U2 (2026-06-17-003 plan): wall-clock of the most recent
    /// `review.dimension.done` for this step's open wave. Used by
    /// `open_waves_needing_intervention` to detect the
    /// "incomplete + no-progress" stall that triggers the
    /// mechanism-emitted `plan.blocked`. `None` until the first
    /// `review.dimension.done` arrives.
    last_dimension_at: Option<Instant>,
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

/// U2 (2026-06-17-003 plan): describe an open wave that the
/// mechanism should emit `plan.blocked` for. Constructed by
/// [`ReviewStepTracker::open_waves_needing_intervention`].
///
/// `expected` is the wave's `wave_total`. `received` is the
/// count of **unique** dimensions already reported (the
/// tracker's set deduplicates duplicate `dimension.done`
/// events for the same dimension). `missing_dimensions` is
/// the set of dimension labels the wave still expects — when
/// the agent emits `dimension` strings (e.g. `sec`, `perf`),
/// those flow into the audit payload; otherwise the set is
/// empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompleteWaveInfo {
    pub plan_name: String,
    pub task_id: String,
    pub step: String,
    pub wave_id: String,
    pub expected: u32,
    pub received: u32,
    pub missing_dimensions: Vec<String>,
    pub started_at: Instant,
    pub last_dimension_at: Option<Instant>,
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
                    // U1 (2026-06-17-003 plan): emit the
                    // dedicated `SemanticGateViolation` variant
                    // instead of forging `InvalidFieldValue {
                    // field: "skip_reason" }`. The payload itself
                    // is well-formed; the violation is in the
                    // **state** (wave open + coordinator
                    // fast-pathing). The runtime loop classifies
                    // this as recoverable and continues — see
                    // `is_recoverable_policy_finding` and the
                    // runner's `PayloadContractViolation` branch.
                    // The `gate` field carries the canonical name
                    // (`review_passed_while_wave_open`) for
                    // audit/diagnostics.
                    return Some(PolicyFinding {
                        topic: topic.to_string(),
                        violation_type: ViolationType::SemanticGateViolation {
                            gate: "review_passed_while_wave_open".to_string(),
                            context: format!(
                                "wave='{}' received={}/{} expected",
                                state.open_wave_id.as_deref().unwrap_or("?"),
                                state.dimensions_received.len(),
                                state.wave_expected,
                            ),
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
                // U2 (2026-06-17-003 plan): bump the
                // "last progress" timestamp so the staleness
                // gate in `open_waves_needing_intervention`
                // can distinguish "stalled" (no recent
                // dimension.done) from "slow but moving".
                state.last_dimension_at = Some(Instant::now());
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

    /// R-F5 / 003-U5: query whether the review wave for a given step
    /// has fully closed (all dimensions received OR a verdict terminal
    /// has been emitted). Returns `true` only when the tracker has
    /// NO open wave for that step AND either no wave was ever opened
    /// or it was already completed (received >= expected or terminal
    /// event seen).
    ///
    /// Used by agents and the runner to gate `last_reviewed_sha`
    /// persistence: writing the SHA is only safe after the wave
    /// closes, so DEC-002 empty_diff fast-paths cannot use a premature
    /// SHA as fuel.
    pub fn is_wave_closed(&self, plan_name: &str, task_id: &str, step: &str) -> bool {
        let key = StepKey {
            plan_name: plan_name.to_string(),
            task_id: task_id.to_string(),
            step: step.to_string(),
        };
        match self.steps.get(&key) {
            None => true, // No tracker entry means no wave ever opened.
            Some(state) => !wave_open(state),
        }
    }

    /// U3 (2026-06-17-003 plan): return a small snapshot of the
    /// first open review wave tracked by the registry, or `None`
    /// if every wave is closed. The snapshot carries the fields
    /// `publish_policy_rejection_resume` needs to print the
    /// `## WAVE_OPEN HINT` block on a `work.done` rejection —
    /// `wave_id`, `received` (`dimensions_received.len()`),
    /// `expected` (`wave_expected`). Used only for the textual
    /// rejection hint; the mechanism layer (`open_waves_needing_intervention`
    /// + `maybe_emit_incomplete_wave_blocked`) remains the
    /// single source of truth for whether the wave is actually
    /// stalled and whether `plan.blocked` should be emitted.
    pub fn first_open_wave_snapshot(&self) -> Option<OpenWaveSnapshot> {
        for state in self.steps.values() {
            if !wave_open(state) {
                continue;
            }
            let Some(wave_id) = state.open_wave_id.clone() else {
                continue;
            };
            return Some(OpenWaveSnapshot {
                wave_id,
                received: state.dimensions_received.len() as u32,
                expected: state.wave_expected,
            });
        }
        None
    }

    /// U2 (2026-06-17-003 plan): close the wave tracked under
    /// `key`. Idempotent — returns `true` if a wave was actually
    /// open and was closed; `false` otherwise. Used by the
    /// mechanism's `plan.blocked` emit path so the gate does
    /// not re-fire on subsequent iterations.
    pub fn close_wave(&mut self, key: &StepKey) -> bool {
        if let Some(state) = self.steps.get_mut(key)
            && state.open_wave_id.is_some()
        {
            state.open_wave_id = None;
            state.aggregate_timeout_dispatched = true;
            return true;
        }
        false
    }

    /// U2 (2026-06-17-003 plan): enumerate the open review waves
    /// that exceed `staleness_secs` past their **last dimension
    /// progress** without converging. The caller compares
    /// `now.duration_since(last_dimension_at) > staleness_secs`
    /// to decide whether to emit the mechanism-level
    /// `plan.blocked`.
    ///
    /// `staleness_secs` is the configured aggregate timeout in
    /// seconds; the production gate uses `0.8 * aggregate_timeout_secs`
    /// but the function takes the absolute threshold so unit
    /// tests can compress time without depending on the
    /// configured `aggregate.timeout`.
    ///
    /// Returns **one entry per (plan_name, task_id, step) wave**.
    /// The caller is expected to dedup across iterations (via the
    /// `aggregate_timeout_dispatched` flag pattern or an external
    /// ledger) so this is a pure observation — emitting is the
    /// caller's job.
    pub fn open_waves_needing_intervention(
        &self,
        staleness_secs: u64,
    ) -> Vec<IncompleteWaveInfo> {
        let now = Instant::now();
        let staleness = std::time::Duration::from_secs(staleness_secs);
        let mut out = Vec::new();
        for (key, state) in &self.steps {
            if !wave_open(state) {
                continue;
            }
            // Only intervene when at least one dimension has
            // arrived — without a baseline, the wave is simply
            // "just started" and the staleness math has no
            // anchor. We skip pure "no workers yet" cases; the
            // aggregate-timeout path (U4 / `inject_review_aggregate_timeouts`)
            // still covers them.
            let Some(last_dim) = state.last_dimension_at else {
                continue;
            };
            if now.duration_since(last_dim) <= staleness {
                continue;
            }
            // Expected vs received counts are unique (set-based).
            let received = state.dimensions_received.len() as u32;
            let expected = state.wave_expected;
            // Missing dimensions: the caller does not know what
            // names the wave expects unless the tracker can
            // observe them. Today the tracker only learns
            // dimensions on `dimension.done`, so we expose the
            // **unfilled** ones the agent has not yet reported
            // for this wave. When the wave's `wave_total` is
            // known but the per-dimension labels are not (most
            // `review.wave.ready` events), `missing_dimensions`
            // is empty and the audit surfaces counts only.
            let missing_dimensions: Vec<String> = Vec::new();
            out.push(IncompleteWaveInfo {
                plan_name: key.plan_name.clone(),
                task_id: key.task_id.clone(),
                step: key.step.clone(),
                wave_id: state.open_wave_id.clone().unwrap_or_default(),
                expected,
                received,
                missing_dimensions,
                started_at: state.wave_started_at.unwrap_or(now),
                last_dimension_at: Some(last_dim),
            });
        }
        out
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

    /// U2 (2026-06-17-003 plan): test-only helper to back-date
    /// the `last_dimension_at` field so the staleness gate in
    /// `open_waves_needing_intervention` can be exercised
    /// without sleeping.
    #[cfg(test)]
    fn backdate_last_dimension_for_test(
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
            state.last_dimension_at =
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
        // U1 (2026-06-17-003 plan): the finding must be the
        // dedicated `SemanticGateViolation` variant — NOT a
        // forged `InvalidFieldValue { field: "skip_reason" }`.
        // The gate field carries the canonical name for audit.
        match &finding.violation_type {
            ViolationType::SemanticGateViolation { gate, context } => {
                assert_eq!(gate, "review_passed_while_wave_open");
                assert!(
                    context.contains("received=0/3"),
                    "context must surface dimensions counts, got: {context}"
                );
            }
            other => panic!(
                "expected SemanticGateViolation, got {other:?} (must NOT be the legacy \
                 InvalidFieldValue{{field: skip_reason}} forged variant)"
            ),
        }
        // And the event must be classified as recoverable in the
        // independent bucket — this is what keeps the loop from
        // terminating with `PayloadContractViolation`.
        use crate::event_policy::is_recoverable_policy_finding;
        let class = is_recoverable_policy_finding(&finding)
            .expect("SemanticGateViolation must be in the recoverable set");
        assert_eq!(
            class,
            crate::event_policy::ReasonClass::SemanticGateViolation
        );
    }

    /// U1 (2026-06-17-003 plan): ensure the real schema-level
    /// `skip_reason` allowed_values mismatch still routes to
    /// `AllowedValueMismatch` and stays in the **non-recoverable**
    /// fatal bucket. This is the regression guard that
    /// `finding_to_payload_contract_violation`'s
    /// `InvalidFieldValue` arm remains unchanged.
    #[test]
    fn real_skip_reason_allowed_value_mismatch_stays_fatal() {
        use crate::event_policy::{is_recoverable_policy_finding, ViolationType};
        let finding = PolicyFinding {
            topic: "review.passed".to_string(),
            violation_type: ViolationType::InvalidFieldValue {
                field: "skip_reason".to_string(),
                value: serde_json::Value::String("not_an_allowed_value".to_string()),
            },
            message: "Field 'skip_reason' has invalid value \"not_an_allowed_value\".".to_string(),
        };
        // Schema-derived `InvalidFieldValue` MUST remain
        // non-recoverable so the U6 `PayloadContractViolation`
        // fatal path still triggers. U1 only re-classifies the
        // semantic-gate variant — not the real allowed_values
        // mismatch.
        assert!(
            is_recoverable_policy_finding(&finding).is_none(),
            "real skip_reason AllowedValueMismatch must stay fatal"
        );
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

    /// U2 (2026-06-17-003 plan): an open wave with at least one
    /// `dimension.done` arrival but no progress past the staleness
    /// window must surface in `open_waves_needing_intervention`.
    /// The expected/received counts are unique (set-based), so the
    /// caller can detect "wave started, some progress, then stalled".
    #[test]
    fn open_waves_needing_intervention_returns_stalled_wave() {
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-stall".to_string());
        wave.wave_total = Some(11);
        tracker.observe_accepted(&wave);

        // Two distinct dimensions arrive.
        let mut d1 = jsonl(
            "review.dimension.done",
            "dimension-reviewer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec","findings_count":0,"findings_file":"f.json"}"#,
        );
        d1.wave_id = Some("w-stall".to_string());
        tracker.observe_accepted(&d1);

        let mut d2 = jsonl(
            "review.dimension.done",
            "dimension-reviewer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"perf","findings_count":0,"findings_file":"f.json"}"#,
        );
        d2.wave_id = Some("w-stall".to_string());
        tracker.observe_accepted(&d2);

        // Before staleness elapses, no intervention needed.
        let actions = tracker.open_waves_needing_intervention(60);
        assert!(
            actions.is_empty(),
            "before staleness the wave must not surface, got {actions:?}"
        );

        // Compress: pretend the last dimension arrived 600s ago.
        tracker.backdate_last_dimension_for_test("p", "t1", "1", Duration::from_secs(600));

        // Now at 60s staleness, the wave is stalled (4/11 unique).
        let actions = tracker.open_waves_needing_intervention(60);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].wave_id, "w-stall");
        assert_eq!(actions[0].expected, 11);
        assert_eq!(
            actions[0].received, 2,
            "received count must be unique (set-based)"
        );
        assert!(actions[0].last_dimension_at.is_some());
    }

    /// U2 (2026-06-17-003 plan): a wave that has **not yet** seen
    /// any dimension.done (just-started, no workers yet) must NOT
    /// surface as needing intervention — the staleness math has no
    /// anchor. The aggregate-timeout path (`drain_expired_aggregate_timeouts`)
    /// still covers it.
    #[test]
    fn open_waves_needing_intervention_skips_waves_with_no_dimensions() {
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-fresh".to_string());
        wave.wave_total = Some(11);
        tracker.observe_accepted(&wave);

        // No dimension.done arrives. Even with a generous
        // staleness window, the wave must not surface because
        // there is no baseline to compare against.
        let actions = tracker.open_waves_needing_intervention(0);
        assert!(
            actions.is_empty(),
            "fresh wave without dimensions must not surface, got {actions:?}"
        );
    }

    /// U2 (2026-06-17-003 plan): a wave that **closed cleanly**
    /// (received == expected) must NOT surface as needing
    /// intervention — the aggregate path is the synthesizer's
    /// job now.
    #[test]
    fn open_waves_needing_intervention_skips_closed_waves() {
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-closed".to_string());
        wave.wave_total = Some(2);
        tracker.observe_accepted(&wave);

        // Two distinct dimensions → received == expected → wave
        // closes (open_wave_id becomes None).
        for dim in ["sec", "perf"] {
            let mut d = jsonl(
                "review.dimension.done",
                "dimension-reviewer",
                &format!(
                    r#"{{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"{dim}","findings_count":0,"findings_file":"f.json"}}"#
                ),
            );
            d.wave_id = Some("w-closed".to_string());
            tracker.observe_accepted(&d);
        }

        let actions = tracker.open_waves_needing_intervention(0);
        assert!(
            actions.is_empty(),
            "closed wave must not surface, got {actions:?}"
        );
    }
}
    // 003-U5 / R-F5: last_reviewed_sha wave-closed gate tests
    //
    // `is_wave_closed` is the query that agents and the runner use to
    // decide whether writing `last_reviewed_sha` is safe. The gate MUST
    // return `false` when a wave is open (even if `review.wave.ready`
    // was emitted) and `true` only when the wave is fully closed
    // (all dimensions received OR a verdict terminal seen).
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn u5_is_wave_closed_no_tracker_entry_returns_true() {
        // No wave ever opened for this step — writing SHA is safe.
        let tracker = ReviewStepTracker::default();
        assert!(
            tracker.is_wave_closed("p", "t1", "1"),
            "R-F5: no tracker entry means no open wave, SHA write is safe"
        );
    }

    #[test]
    fn u5_is_wave_closed_after_wave_ready_returns_false() {
        // `review.wave.ready` just emitted, no dimensions yet.
        // Writing SHA here is the DEC-002 empty_diff fuel the plan
        // explicitly forbids.
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(11);
        tracker.observe_accepted(&wave);

        assert!(
            !tracker.is_wave_closed("p", "t1", "1"),
            "R-F5: wave just opened, SHA write must be blocked"
        );
    }

    #[test]
    fn u5_is_wave_closed_partial_dimensions_returns_false() {
        // 4/11 dimensions received, wave still open.
        // This is the zippy-sparrow stall scenario: a premature SHA
        // write would let the next pass claim empty diff.
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(11);
        tracker.observe_accepted(&wave);

        for dim in ["sec", "rel", "perf", "a11y"] {
            let mut d = jsonl(
                "review.dimension.done",
                "dimension-reviewer",
                &format!(
                    r#"{{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"{dim}","findings_count":0,"findings_file":"f.json"}}"#
                ),
            );
            d.wave_id = Some("w-1".to_string());
            tracker.observe_accepted(&d);
        }

        assert!(
            !tracker.is_wave_closed("p", "t1", "1"),
            "R-F5: 4/11 dimensions received, wave open, SHA write must be blocked"
        );
    }

    #[test]
    fn u5_is_wave_closed_all_dimensions_returns_true() {
        // All 11 dimensions received — wave fully closed.
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(2);
        tracker.observe_accepted(&wave);

        for dim in ["sec", "rel"] {
            let mut d = jsonl(
                "review.dimension.done",
                "dimension-reviewer",
                &format!(
                    r#"{{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"{dim}","findings_count":0,"findings_file":"f.json"}}"#
                ),
            );
            d.wave_id = Some("w-1".to_string());
            tracker.observe_accepted(&d);
        }

        assert!(
            tracker.is_wave_closed("p", "t1", "1"),
            "R-F5: all dimensions received, wave closed, SHA write is safe"
        );
    }

    #[test]
    fn u5_is_wave_closed_after_verdict_returns_true() {
        // Wave opened then `review.passed` verdict seen — wave closed.
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
            "review-synthesizer",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#,
        );
        tracker.observe_accepted(&passed);

        assert!(
            tracker.is_wave_closed("p", "t1", "1"),
            "R-F5: verdict terminal seen, wave closed, SHA write is safe"
        );
    }

    #[test]
    fn u5_is_wave_closed_different_step_isolated() {
        // Wave open for step "1" must not affect step "2" gate.
        let mut tracker = ReviewStepTracker::default();
        let mut wave = jsonl(
            "review.wave.ready",
            "review-coordinator",
            r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#,
        );
        wave.wave_id = Some("w-1".to_string());
        wave.wave_total = Some(5);
        tracker.observe_accepted(&wave);

        assert!(
            !tracker.is_wave_closed("p", "t1", "1"),
            "R-F5: step 1 wave is open"
        );
        assert!(
            tracker.is_wave_closed("p", "t1", "2"),
            "R-F5: step 2 has no wave, SHA write is safe (different step)"
        );
    }
}
