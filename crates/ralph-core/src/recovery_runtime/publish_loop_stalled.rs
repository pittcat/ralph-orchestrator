//! Detect when a `stall_recovery` envelope has been recorded but no
//! `loop.stalled` business event is visible in the recent event stream, and
//! publish the missing business event so downstream stewardship hats can act.
//!
//! See plan 2026-06-28-003 §Defense 2, function 3.

#[cfg(test)]
use super::EventSnapshot;
use super::{EnvelopeSnapshot, RecoveryAction, RuntimeContext};

pub fn publish_loop_stalled_business_event(ctx: &RuntimeContext) -> Vec<RecoveryAction> {
    let stall_envelopes: Vec<&EnvelopeSnapshot> = ctx
        .recovery_envelopes
        .iter()
        .filter(|e| e.source == "StallRecovery" || e.retry_key.starts_with("stall_recovery:"))
        .collect();

    if stall_envelopes.is_empty() {
        return Vec::new();
    }

    let already_published = ctx.events.iter().any(|e| e.topic == "loop.stalled");

    if already_published {
        return Vec::new();
    }

    // Publish one loop.stalled event per stalled envelope. In practice the
    // bus deduplicates, but emitting one per envelope keeps the detector
    // deterministic and testable.
    stall_envelopes
        .iter()
        .map(|e| {
            let payload = serde_json::json!({
                "reason": "stall_recovery",
                "retry_key": e.retry_key,
                "iteration": e.iteration,
            });
            RecoveryAction::PublishEvent {
                topic: "loop.stalled".to_string(),
                payload: payload.to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_action_without_stall_envelope() {
        assert!(publish_loop_stalled_business_event(&RuntimeContext::default()).is_empty());
    }

    #[test]
    fn publishes_loop_stalled_when_missing() {
        let ctx = RuntimeContext {
            recovery_envelopes: vec![EnvelopeSnapshot {
                retry_key: "stall_recovery:executor:work_done:handoff_dispatch_timeout:*"
                    .to_string(),
                source: "StallRecovery".to_string(),
                outcome: "Pending".to_string(),
                iteration: 5,
                attempt: 1,
            }],
            events: vec![EventSnapshot {
                topic: "work.ready".to_string(),
                payload: "{}".to_string(),
                iteration: 5,
            }],
            ..Default::default()
        };
        let actions = publish_loop_stalled_business_event(&ctx);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], RecoveryAction::PublishEvent { topic, .. } if topic == "loop.stalled")
        );
    }

    #[test]
    fn skips_publish_when_loop_stalled_already_exists() {
        let ctx = RuntimeContext {
            recovery_envelopes: vec![EnvelopeSnapshot {
                retry_key: "stall_recovery:executor:work_done:handoff_dispatch_timeout:*"
                    .to_string(),
                source: "StallRecovery".to_string(),
                outcome: "Pending".to_string(),
                iteration: 5,
                attempt: 1,
            }],
            events: vec![EventSnapshot {
                topic: "loop.stalled".to_string(),
                payload: "{}".to_string(),
                iteration: 5,
            }],
            ..Default::default()
        };
        assert!(publish_loop_stalled_business_event(&ctx).is_empty());
    }
}
