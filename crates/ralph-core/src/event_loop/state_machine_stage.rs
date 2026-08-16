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

use crate::config::state_machine::StateMachineConfig;
use crate::event_reader::Event as JsonlEvent;
use crate::state_machine::{
    StateMachineDecision, StateMachineRuntimeState, StateMachineTransitionDelta,
    StateMachineTransitionId,
};

/// Plan GAP-02 / Unit 2 + 2026-08-15-2211 U5/U7: revalidation helper.
/// Validates each event in `events` against the candidate runtime
/// snapshot. A downstream-rejected predecessor cannot influence a
/// later survivor's decision because revalidation starts from the
/// snapshot for every call.
///
/// Returns the in-flight [`CandidateStateMachineDecision`]s (used
/// by the apply stage to materialise [`StateMachineTransitionDelta`]
/// + outbox receipt), plus the rejected/ignored findings so the
/// caller can surface them on the bus.
///
/// Plan 2026-08-15-2211 U7: terminal_observed is captured from the
/// candidate snapshot (per the original U2 capture semantics — the
/// validator mutates terminal_observed=true before returning Accept
/// for a terminal event, so reading it from the candidate AFTER
/// `validate_event` reflects the post-mutation truth). terminal_honored
/// is captured at `apply_state_machine_decisions` entry from the
/// LIVE state, NOT here — `mark_terminal_honored` is invoked by the
/// per-event processing flow (e.g. `wave_scope.rs::mark_terminal_honored`)
/// between candidate stage and apply, so reading terminal_honored
/// from the candidate clone would miss the honored transition and
/// produce deltas that disagree with the live runtime / cold-start
/// rehydration. See U7 §"capture 时机" for the TOCTOU trace.
fn validate_events_against_candidate(
    candidate: &mut StateMachineRuntimeState,
    events: &[JsonlEvent],
    sm_config: &StateMachineConfig,
    bus: &mut EventBus,
) -> (
    Vec<CandidateStateMachineDecision>,
    Vec<crate::state_machine::StateMachineFinding>,
    Vec<crate::state_machine::StateMachineFinding>,
) {
    let mut pending: Vec<CandidateStateMachineDecision> = Vec::with_capacity(events.len());
    let mut rejected: Vec<crate::state_machine::StateMachineFinding> = Vec::new();
    let mut ignored: Vec<crate::state_machine::StateMachineFinding> = Vec::new();

    for event in events {
        let topic = event.topic.clone();
        let payload = event.payload.clone();
        let event_for_emit = event.clone();

        // Capture the candidate's instance maps BEFORE the validator
        // mutates them so the open/close decision reflects the
        // pre-validation state — validate_event inserts the
        // transition's effect into the maps, so a post-mutation read
        // would always report `opens_instance=false, closes_instance=true`
        // for opening transitions, which inverts the semantics the
        // apply stage relies on.
        let (pre_opens_map, pre_closed_map) = {
            let (opens, closed) = candidate.instance_maps();
            (opens.clone(), closed.clone())
        };

        let decision = candidate.validate_event(topic.as_str(), payload.as_deref(), sm_config);

        match &decision {
            StateMachineDecision::Accept { instance_key, .. } => {
                let instance_key = instance_key.clone();
                let key_ref = instance_key.as_deref().unwrap_or("");
                // Plan 2026-08-15-2211 U5: compute the open/close
                // decision from the PRE-validation instance maps so
                // the apply stage gets the correct semantic flags.
                let opens_instance =
                    !pre_opens_map.contains_key(key_ref) && !pre_closed_map.contains_key(key_ref);
                let closes_instance =
                    pre_opens_map.contains_key(key_ref) && !pre_closed_map.contains_key(key_ref);
                // Plan 2026-08-15-2211 U7: capture terminal_observed
                // from the candidate snapshot (post-validate) but
                // defer terminal_honored capture to apply time (see
                // function-level comment for the TOCTOU rationale).
                let (term_obs, _term_hon) = candidate.observed_snapshot();
                pending.push(CandidateStateMachineDecision {
                    event: event_for_emit.clone(),
                    decision: decision.clone(),
                    opens_instance,
                    closes_instance,
                    accepted_at_terminal_observed: term_obs,
                });
            }
            StateMachineDecision::Reject { finding } => {
                let finding = finding.clone();
                let finding_json = serde_json::to_string(&finding).unwrap_or_else(|serde_err| {
                    // Plan 2026-08-15-2211 U9 A3: surface the
                    // serde fallback instead of silently
                    // dropping topic / source_hat / instance_key
                    // / code. The diagnostic payload keeps the
                    // original `reason` for human readability
                    // and adds `_serde_fallback: true` plus the
                    // underlying serde error so operators can
                    // debug without losing the structured
                    // fields when JSON re-serialisation fails.
                    format!(
                        "{{\"reason\":{},\"_serde_fallback\":true,\"_serde_error\":{}}}",
                        serde_json::to_string(&finding.reason)
                            .unwrap_or_else(|_| "\"<unserializable reason>\"".to_string()),
                        serde_json::to_string(&serde_err.to_string())
                            .unwrap_or_else(|_| "\"<unserializable error>\"".to_string()),
                    )
                });
                bus.publish(ralph_proto::Event::new(
                    "event.state_machine.rejected",
                    finding_json,
                ));
                rejected.push(finding);
            }
            StateMachineDecision::Ignore { finding } => {
                let finding = finding.clone();
                let finding_json = serde_json::to_string(&finding).unwrap_or_else(|serde_err| {
                    format!(
                        "{{\"reason\":{},\"_serde_fallback\":true,\"_serde_error\":{}}}",
                        serde_json::to_string(&finding.reason)
                            .unwrap_or_else(|_| "\"<unserializable reason>\"".to_string()),
                        serde_json::to_string(&serde_err.to_string())
                            .unwrap_or_else(|_| "\"<unserializable error>\"".to_string()),
                    )
                });
                bus.publish(ralph_proto::Event::new(
                    "event.state_machine.ignored",
                    finding_json,
                ));
                ignored.push(finding);
            }
            StateMachineDecision::DiagnosticOnly { finding } => {
                let finding = finding.clone();
                let finding_json = serde_json::to_string(&finding).unwrap_or_else(|serde_err| {
                    format!(
                        "{{\"reason\":{},\"_serde_fallback\":true,\"_serde_error\":{}}}",
                        serde_json::to_string(&finding.reason)
                            .unwrap_or_else(|_| "\"<unserializable reason>\"".to_string()),
                        serde_json::to_string(&serde_err.to_string())
                            .unwrap_or_else(|_| "\"<unserializable error>\"".to_string()),
                    )
                });
                bus.publish(ralph_proto::Event::new(
                    "event.state_machine.diagnostic",
                    finding_json,
                ));
                // DiagnosticOnly keeps the event on the survivor
                // list (it does NOT advance state) so we record a
                // synthetic Accept decision with opens/closes=false
                // and the current terminal flags. The caller decides
                // whether to publish this as a business event based
                // on the helper's downstream filter.
                let (term_obs, _term_hon) = candidate.observed_snapshot();
                pending.push(CandidateStateMachineDecision {
                    event: event_for_emit.clone(),
                    decision: StateMachineDecision::Accept {
                        instance_key: None,
                        new_state: String::new(),
                    },
                    opens_instance: false,
                    closes_instance: false,
                    accepted_at_terminal_observed: term_obs,
                });
            }
        }
    }

    (pending, rejected, ignored)
}

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

        // Delegate to the shared helper — same validation logic
        // for both candidate-stage and revalidation at the apply
        // boundary. The candidate stage publishes rejected /
        // ignored diagnostics to the bus exactly once per
        // rejection (the helper emits the diagnostic event
        // synchronously inside the match arm above).
        let (pending, _rejected, _ignored) =
            validate_events_against_candidate(&mut candidate, &events, &sm_config, &mut self.bus);

        // Plan 2026-08-15-2211 U5: the legacy candidate stage
        // returned an `accepted: Vec<JsonlEvent>` so the caller
        // could map survivors back to raw events. The new
        // signature returns pending decisions only; the caller
        // already has the input list and uses each pending
        // decision's embedded `event` field as the survivor key.
        // To preserve the wire contract at the apply boundary
        // (legacy.rs filter), we re-emit the events whose
        // decision landed in `pending` as the first tuple slot.
        let accepted_events: Vec<JsonlEvent> =
            pending.iter().map(|cand| cand.event.clone()).collect();
        (accepted_events, pending)
    }

    /// Plan GAP-02 / Unit 1: re-validates the final survivor events
    /// against the **live** runtime snapshot. Unlike
    /// `run_state_machine_candidate_stage` which uses a cumulative
    /// clone (each event sees prior accepts' mutations), this
    /// function starts from the live state for every event so a
    /// downstream-rejected predecessor cannot influence a later
    /// survivor's decision.
    ///
    /// The live runtime is NOT mutated by this function; it only
    /// produces fresh candidates for the apply stage. Plan
    /// 2026-08-15-2211 U5: rejected/ignored diagnostics from the
    /// revalidation stage ARE published exactly once to the bus
    /// (the helper emits them inside the match arm) — this lets
    /// operators see when a survivor that the candidate stage
    /// accepted gets re-rejected against the live snapshot, e.g.
    /// due to a downstream-rejected predecessor. The accepted
    /// Vec is no longer returned because the revalidation
    /// boundary uses pending decisions only; the caller has the
    /// survivor events from `pending_state_machine_candidates`.
    pub(super) fn revalidate_state_machine_candidates_in_order(
        &mut self,
        survivor_events: &[JsonlEvent],
    ) -> Vec<CandidateStateMachineDecision> {
        let sm_config = match self.config.event_loop.state_machine.as_ref() {
            Some(cfg) if cfg.enabled => cfg.clone(),
            _ => return Vec::new(),
        };

        // Start from the LIVE runtime snapshot — NOT the cumulative
        // candidate clone. This is the key difference from
        // `run_state_machine_candidate_stage`.
        let candidate = self
            .state
            .state_machine_runtime_state
            .clone()
            .unwrap_or_default();

        let mut candidate = candidate;
        let (pending, _rejected, _ignored) = validate_events_against_candidate(
            &mut candidate,
            survivor_events,
            &sm_config,
            &mut self.bus,
        );
        pending
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
    ///
    /// Plan 2026-08-15-2211 U1: this method captures the
    /// pre-batch live snapshot for rollback and mutates live for
    /// every projection in the batch. The per-projection rollback
    /// lives in [`Self::commit_state_machine_projection`]: on a
    /// `StateLedger::commit` failure for projection N, the
    /// rollback restores live to the pre-batch snapshot, but the
    /// caller (the legacy.rs per-event loop) re-applies
    /// projections 1..N-1 in subsequent iterations. The original
    /// per-batch-snapshot code stored the snapshot here and
    /// consumed it via `take()` in commit — the `take()` model
    /// degraded projection N+1's rollback to no-op because the
    /// snapshot was already drained by projection 1's commit.
    /// The new model uses a per-call snapshot inside
    /// `commit_state_machine_projection` itself so each commit
    /// gets its own rollback window (Plan 2026-08-15-2211 U1).
    pub(super) fn apply_state_machine_decisions(
        &mut self,
        decisions: &[CandidateStateMachineDecision],
        loop_id: &str,
    ) -> Vec<StateMachineTransitionDelta> {
        if decisions.is_empty() {
            return Vec::new();
        }
        // Reset the per-batch snapshot slot at apply entry so a
        // stale snapshot from a prior batch cannot leak into this
        // batch's rollback. We then capture the live runtime
        // BEFORE any projection mutates it.
        self.state_machine_apply_snapshot = None;
        self.state_machine_committed_deltas.clear();
        let mut projected = Vec::with_capacity(decisions.len());
        // The live runtime is materialised here so projection can
        // read it (instance maps, terminal flags) without `unwrap`.
        let live = self
            .state
            .state_machine_runtime_state
            .get_or_insert_with(StateMachineRuntimeState::default);
        // Plan GAP-02 / Unit 3 + 2026-08-15-2211 U1: capture the
        // pre-apply live snapshot BEFORE any projection mutates
        // live. The snapshot slot is NOT drained here (we just
        // assign); `commit_state_machine_projection` reads it
        // directly so every per-event commit in the batch sees
        // the same pre-apply-batch snapshot for rollback.
        self.state_machine_apply_snapshot = Some(live.clone());
        // Plan 2026-08-15-2211 U7: capture `terminal_honored`
        // from the LIVE runtime at apply entry. `mark_terminal_honored`
        // is invoked by per-event processing (e.g. wave_scope.rs)
        // BETWEEN the candidate stage and apply; capturing from the
        // candidate clone here would miss it and produce a delta
        // that disagrees with the post-batch live runtime / cold-
        // start rehydration. terminal_observed is still sourced from
        // the candidate decision because `validate_terminal_event`
        // mutates the candidate cumulative clone to set
        // `terminal_observed=true` for the terminal event, and the
        // mutation happens before the candidate stage returns.
        let apply_terminal_honored = live.is_terminal_honored();
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
                // Plan GAP-02 / Unit 2: terminal_observed comes from
                // the candidate's captured snapshot (the validator
                // mutated the cumulative clone).
                candidate.accepted_at_terminal_observed,
                // Plan 2026-08-15-2211 U7: terminal_honored is
                // captured from the LIVE runtime at apply entry so
                // a mark_terminal_honored invocation between
                // candidate stage and apply propagates into the
                // delta (no TOCTOU mismatch with rehydration).
                apply_terminal_honored,
            );
            if let Some(delta) = delta {
                let mut delta = delta;
                delta.source_hat = Some("executor".to_string());
                // Apply the delta to live so test / legacy paths
                // observe the StateMachine progression. The
                // projection-aware commit path
                // (`commit_state_machine_projection` with a compiled
                // execution contract) re-applies the delta BEFORE
                // the durable commit; on commit failure the
                // pre-apply snapshot restores live to its prior
                // state. Net effect on a successful commit: live
                // has the projection applied exactly once.
                live.apply_transition_delta(&delta);
                projected.push(delta);
            }
        }
        projected
    }

    /// Plan GAP-02 / Unit 3: commit a single projected
    /// StateMachine transition through the disposition helper, with
    /// rollback that restores the pre-apply live runtime snapshot
    /// if the `StateLedger::commit` step faults.
    ///
    /// Plan 2026-08-15-2211 U1 + U8: this method no longer threads
    /// the snapshot through a `'static` closure + `Rc<RefCell<>>`.
    /// The apply (live mutation) happens here synchronously before
    /// the disposition helper is called, and the rollback happens
    /// here synchronously after the helper returns Err — single
    /// linear control flow, no shared mutable cell.
    ///
    /// Per-batch snapshot semantics: the pre-apply snapshot is
    /// captured once by `apply_state_machine_decisions` (BEFORE
    /// any projection mutates live) and stored in
    /// `state_machine_apply_snapshot`. Every commit in the
    /// per-event loop reads the same snapshot (via clone, not
    /// `take()`) so projections 2..N still see a non-`None`
    /// rollback target — pre-fix the `take()` model drained the
    /// slot on the first commit and degraded projection N+1's
    /// rollback to no-op.
    pub(super) fn commit_state_machine_projection(
        &mut self,
        event: &ralph_proto::Event,
        disposition: crate::event_loop::disposition::Disposition,
        loop_id: &str,
        activation_id: &str,
        contract_revision: &str,
        projection: Option<StateMachineTransitionDelta>,
    ) -> Result<
        Option<crate::event_loop::accepted_transition::OutboxEntry>,
        crate::event_loop::accepted_transition::TransitionError,
    > {
        // Plan GAP-02 / Unit 3 + 2026-08-15-2211 U1: rollback uses
        // the pre-apply-batch snapshot captured in
        // `apply_state_machine_decisions` BEFORE any projection
        // mutated live. We read it (no `take()`) so subsequent
        // commits in the same batch also see a non-`None`
        // snapshot for their rollback. Pre-fix the slot was
        // drained by `take()` on the first commit, which left
        // projection N+1 with `None` and degraded its rollback to
        // no-op when the durable commit failed.
        let mut pre_apply_snapshot: Option<StateMachineRuntimeState> =
            self.state_machine_apply_snapshot.clone();
        if let Some(ref delta) = projection {
            let live = self
                .state
                .state_machine_runtime_state
                .as_mut()
                .expect("apply_state_machine_decisions must materialise runtime before commit");
            // Apply is idempotent on transition_id; if
            // apply_state_machine_decisions already applied this
            // delta, this is a no-op and the snapshot already
            // reflects the post-apply state.
            live.apply_transition_delta(delta);
        }

        // The disposition helper expects an
        // `impl FnOnce() -> Result<Box<dyn FnOnce()>, String>`
        // materialize closure. We no longer need a `'static` +
        // `Rc<RefCell<>>` bridge to communicate the snapshot
        // back: the rollback now happens in the post-dispatch
        // Err branch below using the local
        // `pre_apply_snapshot`.
        let materialize =
            || -> Result<Box<dyn FnOnce()>, String> { Ok(Box::new(|| {}) as Box<dyn FnOnce()>) };

        let committed_projection = projection.clone();
        let result = {
            // Reborrow the ledger for the duration of the dispatch.
            let ledger = match self.state.state_ledger.as_mut() {
                Some(l) => l,
                None => {
                    // Roll back the live mutation we just performed
                    // before returning the error.
                    if let Some(snap) = pre_apply_snapshot.take()
                        && let Some(live) = self.state.state_machine_runtime_state.as_mut()
                    {
                        *live = snap;
                        for committed in &self.state_machine_committed_deltas {
                            live.apply_transition_delta(committed);
                        }
                    }
                    return Err(
                        crate::event_loop::accepted_transition::TransitionError::CommitFailed {
                            source: "state ledger missing".to_string(),
                        },
                    );
                }
            };
            crate::event_loop::disposition::publish_synthetic_with_state_machine_projection(
                event,
                disposition,
                loop_id,
                activation_id,
                contract_revision,
                ledger,
                &mut self.bus,
                materialize,
                projection,
            )
        };

        // Plan 2026-08-15-2211 U1: rollback only the failed
        // projection. If earlier projections already committed, restore
        // the pre-batch snapshot and replay that durable prefix so live
        // state remains aligned with ledger/outbox state.
        if result.is_err() {
            if let Some(snap) = pre_apply_snapshot.take()
                && let Some(live) = self.state.state_machine_runtime_state.as_mut()
            {
                *live = snap;
                // Keep live state aligned with the durable prefix. The
                // earlier projections may already have committed and
                // published before this projection failed.
                for committed in &self.state_machine_committed_deltas {
                    live.apply_transition_delta(committed);
                }
            }
        } else if let Some(delta) = committed_projection {
            self.state_machine_committed_deltas.push(delta);
        }

        result
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
    /// Plan GAP-02 / Unit 2 — terminal_observed is captured from the
    /// candidate snapshot AFTER `validate_terminal_event` mutated
    /// `candidate.terminal_observed=true` (for terminal events).
    /// Plan 2026-08-15-2211 U7 removes the `accepted_at_terminal_honored`
    /// field; terminal_honored is captured fresh from the LIVE
    /// runtime at `apply_state_machine_decisions` entry to avoid
    /// the TOCTOU between candidate stage (captures too early) and
    /// `mark_terminal_honored` (called by per-event processing
    /// between candidate and apply).
    pub accepted_at_terminal_observed: bool,
}

#[cfg(test)]
impl EventLoop {
    /// Test-only helper: install a state ledger bypassing the
    /// normal loop-context wiring. Used by the U3 rollback test to
    /// drive `commit_state_machine_projection` against a fault-injected
    /// ledger without an `unsafe` borrow in the test body.
    pub(crate) fn install_state_ledger_for_test(&mut self, ledger: crate::state::StateLedger) {
        self.state.state_ledger = Some(ledger);
    }

    /// Test-only helper: toggle `bypass_active_for_test` on the
    /// installed ledger without exposing the inner borrow to the
    /// caller.
    pub(crate) fn set_state_ledger_bypass_active_for_test(&mut self, active: bool) {
        if let Some(l) = self.state.state_ledger.as_mut() {
            l.set_bypass_active_for_test(active);
        }
    }

    /// Test-only helper: read the installed ledger's commit log.
    pub(crate) fn state_ledger_commit_log(&self) -> Vec<crate::state::Commit> {
        self.state
            .state_ledger
            .as_ref()
            .map(|l| l.commit_log().to_vec())
            .unwrap_or_default()
    }
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
