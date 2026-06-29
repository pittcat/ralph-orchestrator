//! Runtime recovery engine — compiled-in corrective actions for recurring
//! loop failure modes. See plan 2026-06-28-003.
//!
//! The engine exposes four independent detectors, each looking at a small
//! slice of runtime state and returning zero or more `RecoveryAction`s.
//! A thin dispatcher calls all four detectors and merges the actions. Each
//! detector is intentionally self-contained so a schema mismatch in one path
//! silently skips instead of aborting the loop.

use block_executor_resend::block_executor_resend_storm;
use dedupe_stall_recovery::dedupe_stall_recovery_with_missing_event_gate;
use finalize_recovery_outcome::finalize_recovery_outcome_on_flapping;
use publish_loop_stalled::publish_loop_stalled_business_event;

pub mod block_executor_resend;
pub mod dedupe_stall_recovery;
pub mod finalize_recovery_outcome;
pub mod publish_loop_stalled;

/// Lightweight snapshot of a recovery envelope relevant to the detectors.
#[derive(Debug, Clone, Default)]
pub struct EnvelopeSnapshot {
    pub retry_key: String,
    pub source: String,
    pub outcome: String,
    pub iteration: u32,
    pub attempt: u32,
}

/// Lightweight snapshot of a business event relevant to the detectors.
#[derive(Debug, Clone, Default)]
pub struct EventSnapshot {
    pub topic: String,
    pub payload: String,
    pub iteration: u32,
}

/// Per-retry-key state tracked by the recovery responder.
#[derive(Debug, Clone, Default)]
pub struct RetryKeyState {
    pub retry_key: String,
    pub last_outcome: String,
    pub outcome_history: Vec<String>,
    pub attempt_count: u32,
}

/// Context passed to every detector. Only the fields a detector cares about
/// need to be populated; missing/empty fields are treated as "no signal".
#[derive(Debug, Clone, Default)]
pub struct RuntimeContext {
    pub current_iteration: u32,
    pub recovery_envelopes: Vec<EnvelopeSnapshot>,
    pub events: Vec<EventSnapshot>,
    pub retry_key_states: Vec<RetryKeyState>,
    pub current_retry_key: Option<String>,
    pub current_hat: Option<String>,
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
    actions
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
    fn dispatch_merges_actions_from_multiple_detectors() {
        let ctx = RuntimeContext {
            current_iteration: 10,
            current_hat: Some("executor".to_string()),
            current_retry_key: Some("stall_recovery:executor:work_done:handoff_dispatch_timeout:*".to_string()),
            recovery_envelopes: vec![
                EnvelopeSnapshot {
                    retry_key: "stall_recovery:executor:work_done:handoff_dispatch_timeout:*".to_string(),
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
            actions.iter().any(|a| matches!(a, RecoveryAction::DedupeEnvelope { .. })),
            "dedupe action should be present"
        );
    }
}
