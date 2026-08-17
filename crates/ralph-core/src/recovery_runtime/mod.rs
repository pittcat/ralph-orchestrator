//! Runtime recovery engine — compiled-in corrective actions for recurring
//! loop failure modes. See plan 2026-06-28-003.
//!
//! The engine exposes four independent detectors, each looking at a small
//! slice of runtime state and returning zero or more `RecoveryAction`s.
//! A thin dispatcher calls all four detectors and merges the actions. Each
//! detector is intentionally self-contained so a schema mismatch in one path
//! silently skips instead of aborting the loop.

use serde::Deserialize;

use block_executor_resend::block_executor_resend_storm;
use dedupe_stall_recovery::dedupe_stall_recovery_with_missing_event_gate;
use finalize_recovery_outcome::finalize_recovery_outcome_on_flapping;
use publish_loop_stalled::publish_loop_stalled_business_event;
use retry_cap::{detect_retry_cap_escalation, get_retry_attempt};

pub mod block_executor_resend;
pub mod dedupe_stall_recovery;
pub mod finalize_recovery_outcome;
pub mod publish_loop_stalled;
pub mod retry_cap;

/// Lightweight snapshot of a recovery envelope relevant to the detectors.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EnvelopeSnapshot {
    pub retry_key: String,
    pub source: String,
    pub outcome: String,
    pub iteration: u32,
    pub attempt: u32,
}

/// Lightweight snapshot of a business event relevant to the detectors.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventSnapshot {
    pub topic: String,
    pub payload: String,
    pub iteration: u32,
}

/// Per-retry-key state tracked by the recovery responder.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RetryKeyState {
    pub retry_key: String,
    pub last_outcome: String,
    pub outcome_history: Vec<String>,
    pub attempt_count: u32,
}

/// Context passed to every detector. Only the fields a detector cares about
/// need to be populated; missing/empty fields are treated as "no signal".
#[derive(Debug, Clone)]
pub struct RuntimeContext {
    pub current_iteration: u32,
    pub recovery_envelopes: Vec<EnvelopeSnapshot>,
    pub events: Vec<EventSnapshot>,
    pub retry_key_states: Vec<RetryKeyState>,
    pub current_retry_key: Option<String>,
    pub current_hat: Option<String>,
    /// Hat IDs considered "executor-class" for the resend-storm
    /// detector: any hat whose `publishes` list contains `work.done`.
    /// Populated by [`crate::event_loop::EventLoop::runtime_recovery_context`]
    /// from the live [`crate::hat_registry::HatRegistry`] so a preset
    /// that renames the executor hat (e.g. `executor-fix`,
    /// `executor-integration`) still triggers the detector.
    pub executor_hat_ids: Vec<String>,
    /// Plan 2026-08-16-1015 Unit 3: cap on consecutive
    /// `handoff_dispatch_timeout` envelopes for a single retry key
    /// before `finalize_recovery_outcome` escalates to
    /// `ForcePlanBlocked`. Source-of-truth is the existing
    /// `TelemetryConfig::max_repeated_recoveries` (default 3, 0
    /// rejected by config validation). A safe Default of 3 lets
    /// hand-rolled test contexts opt out of plumbing the cap.
    pub handoff_retry_cap: u32,
}

fn default_handoff_retry_cap() -> u32 {
    3
}

impl Default for RuntimeContext {
    fn default() -> Self {
        Self {
            current_iteration: 0,
            recovery_envelopes: Vec::new(),
            events: Vec::new(),
            retry_key_states: Vec::new(),
            current_retry_key: None,
            current_hat: None,
            executor_hat_ids: Vec::new(),
            handoff_retry_cap: default_handoff_retry_cap(),
        }
    }
}

/// U7 (extended for PMI-006 round-trip safety): `Deserialize` parses ALL
/// fields via a Helper struct with `#[serde(default)]`, so missing fields
/// fall back to their Default values. The snapshot types
/// (`EnvelopeSnapshot`, `EventSnapshot`, `RetryKeyState`) gain `Deserialize`
/// derives — they're plain data, so the original surface-area concern that
/// motivated the partial impl no longer applies. This guarantees round-trip
/// safety: a serialized populated `RuntimeContext` deserializes back to a
/// `RuntimeContext` with every field intact, including `retry_key_states`
/// (feeds `finalize_recovery_outcome::handoff_timeout_pending`) and
/// `executor_hat_ids` (feeds `block_executor_resend_storm`).
impl<'de> Deserialize<'de> for RuntimeContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(default)]
        struct Helper {
            current_iteration: u32,
            recovery_envelopes: Vec<EnvelopeSnapshot>,
            events: Vec<EventSnapshot>,
            retry_key_states: Vec<RetryKeyState>,
            current_retry_key: Option<String>,
            current_hat: Option<String>,
            executor_hat_ids: Vec<String>,
            handoff_retry_cap: u32,
        }

        impl Default for Helper {
            fn default() -> Self {
                Self {
                    current_iteration: 0,
                    recovery_envelopes: Vec::new(),
                    events: Vec::new(),
                    retry_key_states: Vec::new(),
                    current_retry_key: None,
                    current_hat: None,
                    executor_hat_ids: Vec::new(),
                    handoff_retry_cap: default_handoff_retry_cap(),
                }
            }
        }

        let helper = Helper::deserialize(deserializer)?;
        Ok(Self {
            current_iteration: helper.current_iteration,
            recovery_envelopes: helper.recovery_envelopes,
            events: helper.events,
            retry_key_states: helper.retry_key_states,
            current_retry_key: helper.current_retry_key,
            current_hat: helper.current_hat,
            executor_hat_ids: helper.executor_hat_ids,
            handoff_retry_cap: helper.handoff_retry_cap,
        })
    }
}

/// Corrective action produced by a detector. Callers apply actions in the
/// order declared here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Drop a duplicate recovery envelope before it is persisted.
    DedupeEnvelope { drop_retry_key: String },
    /// Publish a business event to the bus.
    PublishEvent { topic: String, payload: String },
    /// Inject a hard directive into the agent prompt (via hard_gate).
    InjectDirective { text: String },
    /// Force the loop toward a plan.blocked terminal state.
    ForcePlanBlocked { reason: String, retry_key: String },
}

/// Run all four detectors against the supplied context and return the merged
/// list of actions. Detectors are independent; each silently skips when its
/// required signals are absent.
pub fn dispatch(ctx: &RuntimeContext) -> Vec<RecoveryAction> {
    let mut actions = Vec::new();
    actions.extend(dedupe_stall_recovery_with_missing_event_gate(ctx));
    actions.extend(finalize_recovery_outcome_on_flapping(ctx));
    actions.extend(publish_loop_stalled_business_event(ctx));
    actions.extend(block_executor_resend_storm(ctx));
    // 2026-06-29-007 plan U3: review-chain retry cap runs
    // LAST so its `ForcePlanBlocked` action wins over any
    // earlier escalation. This is the path that breaks
    // the "loop 停不下来" recursion: after `RETRY_CAP`
    // task.resume injections on a review-chain retry_key,
    // the loop is forced to plan.blocked.
    actions.extend(detect_retry_cap_escalation(ctx));
    actions
}

/// 2026-06-29-007 plan U3 + U8 (KTD-6): convenience accessor
/// for the BDD scenario runner. Delegates to
/// [`retry_cap::get_retry_attempt`] so scenario assertions
/// can read the current attempt count without poking into
/// internal state directly.
pub fn get_retry_attempt_for(retry_key: &str, ctx: &RuntimeContext) -> u32 {
    get_retry_attempt(ctx, retry_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_returns_empty_when_no_signals() {
        let ctx = RuntimeContext::default();
        assert!(dispatch(&ctx).is_empty());
    }

    #[test]
    fn runtime_context_default_handoff_retry_cap_is_three() {
        // U7: default_handoff_retry_cap() is the single source-of-truth;
        // impl Default delegates to it, so Default::default() must also yield 3.
        assert_eq!(RuntimeContext::default().handoff_retry_cap, 3);
    }

    #[test]
    fn runtime_context_serde_default_handoff_retry_cap_is_three() {
        // U7: #[serde(default = "default_handoff_retry_cap")] drives missing-field
        // deserialization; verify both paths agree on the cap value.
        let ctx: RuntimeContext = serde_json::from_str("{}").unwrap();
        assert_eq!(ctx.handoff_retry_cap, 3);
    }

    #[test]
    fn dispatch_merges_actions_from_multiple_detectors() {
        let ctx = RuntimeContext {
            current_iteration: 10,
            current_hat: Some("executor".to_string()),
            current_retry_key: Some(
                "stall_recovery:executor:work_done:handoff_dispatch_timeout:*".to_string(),
            ),
            recovery_envelopes: vec![
                EnvelopeSnapshot {
                    retry_key: "stall_recovery:executor:work_done:handoff_dispatch_timeout:*"
                        .to_string(),
                    source: "StallRecovery".to_string(),
                    outcome: "Pending".to_string(),
                    iteration: 10,
                    attempt: 1,
                },
                EnvelopeSnapshot {
                    retry_key: "missing_event_gate:executor:work_done:missing_event:*".to_string(),
                    source: "MissingEventGate".to_string(),
                    outcome: "Pending".to_string(),
                    iteration: 10,
                    attempt: 1,
                },
            ],
            ..Default::default()
        };
        let actions = dispatch(&ctx);
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, RecoveryAction::DedupeEnvelope { .. })),
            "dedupe action should be present"
        );
    }

    /// PMI-006: `RuntimeContext::deserialize` only parses `handoff_retry_cap`;
    /// every other field silently falls back to `Default::default()`. The
    /// `Deserialize` trait impl implies round-trip safety while the custom
    /// implementation strips every detector signal. This test feeds JSON that
    /// contains populated `retry_key_states` and `executor_hat_ids` and
    /// asserts those fields survive the deserialization.
    ///
    /// Repro: any consumer that serializes a populated `RuntimeContext`
    /// (or any future tooling that adopts the trait impl as a wire
    /// contract) loses the bounded-retry budget
    /// (`retry_key_states`) and the executor hat registry
    /// (`executor_hat_ids`) — both of which feed detectors that protect
    /// against unbounded handoff retries and executor resend storms.
    #[test]
    fn pmi006_deserialize_preserves_retry_key_states_and_executor_hat_ids() {
        // Hypothetical wire shape: a populated RuntimeContext serialized by a
        // future Serialize impl (or hand-constructed by debug tooling). Today
        // there is no Serialize impl, but the Deserialize contract is supposed
        // to handle this shape — otherwise the trait impl implies more than
        // it delivers.
        let json = r#"{
            "current_iteration": 7,
            "recovery_envelopes": [],
            "events": [],
            "retry_key_states": [
                {
                    "retry_key": "stall_recovery:executor:work_done:handoff_dispatch_timeout:*",
                    "last_outcome": "Pending",
                    "outcome_history": ["Pending", "Pending"],
                    "attempt_count": 2
                }
            ],
            "current_retry_key": "stall_recovery:executor:work_done:handoff_dispatch_timeout:*",
            "current_hat": "executor",
            "executor_hat_ids": ["executor", "executor-fix"],
            "handoff_retry_cap": 3
        }"#;
        let ctx: RuntimeContext = serde_json::from_str(json)
            .expect("Deserialize must accept the wire shape implied by the trait impl");

        // Critical detector signals MUST survive the round-trip. Both
        // `retry_key_states` and `executor_hat_ids` are inputs to
        // detectors that prevent unbounded retry / resend storms; losing
        // them silently neutralizes those detectors on any consumer that
        // adopts the Deserialize impl as a wire contract.
        assert_eq!(
            ctx.retry_key_states.len(),
            1,
            "retry_key_states must survive Deserialize; the bounded-retry \
             budget is read by finalize_recovery_outcome (finalize_recovery_outcome.rs:102-120)"
        );
        assert_eq!(
            ctx.retry_key_states[0].retry_key,
            "stall_recovery:executor:work_done:handoff_dispatch_timeout:*",
            "retry_key_states[0].retry_key must survive Deserialize"
        );
        assert_eq!(
            ctx.executor_hat_ids,
            vec!["executor".to_string(), "executor-fix".to_string()],
            "executor_hat_ids must survive Deserialize; feeds \
             block_executor_resend_storm (mod.rs:141)"
        );
        // current_iteration and current_hat are also dropped today;
        // assert them too to make the contract violation explicit.
        assert_eq!(
            ctx.current_iteration, 7,
            "current_iteration must survive Deserialize"
        );
        assert_eq!(
            ctx.current_hat.as_deref(),
            Some("executor"),
            "current_hat must survive Deserialize"
        );
        // The one field the impl does parse — regression guard so the
        // contract doesn't regress in the other direction.
        assert_eq!(
            ctx.handoff_retry_cap, 3,
            "handoff_retry_cap must survive Deserialize"
        );
    }
}
