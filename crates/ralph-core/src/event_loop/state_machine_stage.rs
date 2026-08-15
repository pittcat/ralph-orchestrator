//! Plan GAP-02 (2026-08-13-002) Unit 2: candidate stage helper
//! for the StateMachine validation step. Extracted from
//! `parse_and_emit.rs` to keep that file under the 5 000-line
//! hard cap (Module size rule HARD RULE in this repo).
//!
//! Unit 2 wiring switches the live-mutating block (which used to
//! call `validate_event` directly on the runtime) to a *candidate*
//! stage: every event is validated against a `clone()` of
//! `state_machine_runtime_state`, the validator returns the
//! decision but the live map is **not** mutated here. The final
//! pending_publish boundary owned by `parse_and_emit.rs` then
//! passes the surviving events to [`apply_state_machine_decisions`]
//! which performs the live mutation + projects the result into a
//! `StateMachineTransitionDelta` for the caller (Unit 3 will turn
//! the projected delta into an outbox receipt).
//!
//! Disabled path semantics — when StateMachine is not enabled,
//! the stage is a no-op passthrough so the default workflow keeps
//! its existing per-event behaviour.

use super::*;

use crate::event_reader::Event as JsonlEvent;
use crate::state_machine::{
    StateMachineDecision, StateMachineTransitionDelta, StateMachineTransitionId,
};

impl EventLoop {
    /// Plan GAP-02 / Unit 2: candidate-stage validation. Each
    /// event is run through the validator against a *clone* of
    /// the live StateMachine runtime; rejected events are dropped
    /// from the candidate list with their diagnostic bus publishes
    /// preserved exactly as before (per Unit 2 §13 "保持
    /// diagnostic topic"). The returned candidate list never
    /// mutates `self.state.state_machine_runtime_state`. Returns
    /// the in-flight decisions so the caller can decide at the
    /// final pending_publish boundary whether to apply them.
    pub(super) fn run_state_machine_candidate_stage(
        &mut self,
        events: Vec<JsonlEvent>,
    ) -> (Vec<JsonlEvent>, Vec<CandidateStateMachineDecision>) {
        let sm_config = match self.config.event_loop.state_machine.as_ref() {
            Some(cfg) if cfg.enabled => cfg.clone(),
            // Disabled / None path — passthrough with empty
            // candidate list. No diagnostic publishes, no live
            // mutation.
            _ => return (events, Vec::new()),
        };

        // Take a snapshot of the live runtime for the candidate
        // stage; the live runtime itself is not mutated by this
        // stage. Unit 2 §3 / §6 explicitly require this
        // separation so downstream reject cannot pollute live
        // StateMachine. The snapshot is recorded as
        // `_live_snapshot` because the cumulative `candidate`
        // below already starts from the live state and the
        // explicit capture is documentation of intent, not an
        // additional read.
        let _live_snapshot = self.state.state_machine_runtime_state.clone();

        let mut accepted: Vec<JsonlEvent> = Vec::with_capacity(events.len());
        let mut pending: Vec<CandidateStateMachineDecision> = Vec::new();

        // Plan GAP-02 / Unit 2 / parity guard: the original
        // state machine stage inlined a `get_or_insert_with` so
        // the runtime lived on the loop even when every event was
        // rejected. Preserving that semi-materialisation keeps
        // tests / observers that peek at
        // `state.state_machine_runtime_state` working. The unit
        // stage itself never mutates the live runtime here —
        // apply happens at the pending_publish boundary.
        if !events.is_empty() && self.state.state_machine_runtime_state.is_none() {
            self.state.state_machine_runtime_state = Some(StateMachineRuntimeState::new());
        }
        // Cumulative candidate: starts at the live snapshot and
        // is forwarded between events so subsequent validates
        // see the prior accepts' mutations. This mirrors the
        // original live validator's per-event mutation
        // semantics without touching the live runtime.
        let mut candidate = self
            .state
            .state_machine_runtime_state
            .clone()
            .unwrap_or_default();

        for event in events {
            let topic = event.topic.clone();
            let payload = event.payload.clone();
            let event_for_emit = event.clone();

            let decision = candidate.validate_event(topic.as_str(), payload.as_deref(), &sm_config);

            match &decision {
                StateMachineDecision::Accept { instance_key, .. } => {
                    let instance_key = instance_key.clone();
                    let (opens_map, closed_map) = candidate.instance_maps();
                    let key_ref = instance_key.as_deref().unwrap_or("");
                    let opens_instance =
                        !opens_map.contains_key(key_ref) && !closed_map.contains_key(key_ref);
                    let closes_instance =
                        opens_map.contains_key(key_ref) && !closed_map.contains_key(key_ref);
                    let (term_obs, term_hon) = candidate.observed_snapshot();
                    pending.push(CandidateStateMachineDecision {
                        event: event_for_emit.clone(),
                        decision: decision.clone(),
                        opens_instance,
                        closes_instance,
                        accepted_at_terminal_observed: term_obs,
                        accepted_at_terminal_honored: term_hon,
                    });
                    accepted.push(event_for_emit);
                }
                StateMachineDecision::Reject { finding } => {
                    self.bus.publish(ralph_proto::Event::new(
                        "event.state_machine.rejected",
                        serde_json::to_string(&finding).unwrap_or_else(|_| finding.reason.clone()),
                    ));
                }
                StateMachineDecision::Ignore { finding } => {
                    self.bus.publish(ralph_proto::Event::new(
                        "event.state_machine.ignored",
                        serde_json::to_string(&finding).unwrap_or_else(|_| finding.reason.clone()),
                    ));
                }
                StateMachineDecision::DiagnosticOnly { finding } => {
                    self.bus.publish(ralph_proto::Event::new(
                        "event.state_machine.diagnostic",
                        serde_json::to_string(&finding).unwrap_or_else(|_| finding.reason.clone()),
                    ));
                    accepted.push(event_for_emit.clone());
                }
            }
        }

        (accepted, pending)
    }

    /// Plan GAP-02 / Unit 2: apply the candidate decisions that
    /// survived every downstream gate. This is the *only* point
    /// where the live StateMachine runtime mutates as a result
    /// of the validation step. The function is a no-op when
    /// `decisions` is empty (disabled path or no candidate
    /// accepted). Re-running with the same decisions is
    /// idempotent on the live runtime: each decision carries the
    /// resulting state directly so a re-apply of the same
    /// decision is a no-op for `open_instances` /
    /// `closed_instances` collision. Unit 3 binds the
    /// `transition_id` with the durable outbox receipt.
    pub(super) fn apply_state_machine_decisions(
        &mut self,
        decisions: &[CandidateStateMachineDecision],
        loop_id: &str,
    ) -> Vec<StateMachineTransitionDelta> {
        if decisions.is_empty() {
            return Vec::new();
        }
        let mut projected = Vec::with_capacity(decisions.len());
        let live = self
            .state
            .state_machine_runtime_state
            .get_or_insert_with(StateMachineRuntimeState::default);
        for candidate in decisions {
            let topic = candidate.event.topic.as_str();
            // The identity is derived from the accepted event and semantic
            // result. It must not depend on batch position or loop iteration.
            let canonical_payload =
                canonical_payload(candidate.event.payload.as_deref().unwrap_or(""));
            let semantic_key = serde_json::to_string(&(
                &canonical_payload,
                &candidate.decision,
                candidate.opens_instance,
                candidate.closes_instance,
            ))
            .expect("state-machine transition identity serializes");
            let id = StateMachineTransitionId::build(
                loop_id,
                None,
                "executor",
                topic,
                candidate.decision.instance_key().map(|s| s.as_str()),
                &semantic_key,
            );
            let delta = live.project_transition_delta(
                id,
                topic,
                &candidate.decision,
                candidate.opens_instance,
                candidate.closes_instance,
            );
            if let Some(delta) = delta {
                let mut delta = delta;
                delta.source_hat = Some("executor".to_string());
                live.apply_transition_delta(&delta);
                projected.push(delta);
            }
        }
        projected
    }
}

fn canonical_payload(payload: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return payload.to_string();
    };
    serde_json::to_string(&canonical_json_value(value)).unwrap_or_else(|_| payload.to_string())
}

fn canonical_json_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonical_json_value(value)))
                    .collect(),
            )
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical_json_value).collect())
        }
        other => other,
    }
}

/// Plan GAP-02 / Unit 2: the candidate-stage decision captured
/// for each accepted event. The apply stage (Unit 3) maps this
/// to a `StateMachineTransitionDelta` and binds it to the outbox
/// receipt at the durable boundary.
#[derive(Debug, Clone)]
pub(crate) struct CandidateStateMachineDecision {
    pub event: JsonlEvent,
    pub decision: StateMachineDecision,
    pub opens_instance: bool,
    pub closes_instance: bool,
    /// Plan GAP-02 / Unit 2 — projection snapshot. Reserved for
    /// the Unit 3 apply stage; not consumed in Unit 2 itself.
    #[allow(dead_code)]
    pub accepted_at_terminal_observed: bool,
    /// Plan GAP-02 / Unit 2 — projection snapshot. Reserved for
    /// the Unit 3 apply stage; not consumed in Unit 2 itself.
    #[allow(dead_code)]
    pub accepted_at_terminal_honored: bool,
}

impl StateMachineDecision {
    /// Borrow the `instance_key` from an `Accept` decision. Used
    /// by the apply stage to thread the identity into the
    /// transition-id builder.
    pub(super) fn instance_key(&self) -> Option<&String> {
        match self {
            StateMachineDecision::Accept { instance_key, .. } => instance_key.as_ref(),
            _ => None,
        }
    }
}
