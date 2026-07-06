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

    // 2026-07-06 silent-success P0-3 fix (long-term): the
    // `loop.stalled` event is consumed by progress-steward,
    // which can eventually trigger `ForcePlanBlocked` with a
    // `recovery_exhausted:<retry_key>` reason. To keep
    // events / workspace recovery / shipper reason literals
    // aligned (see `shipper_reason::is_recoverable_plan_blocked_reason`
    // + the 2026-07-04-024019 run P0-3 prefix allowlist),
    // emit the loop.stalled payload `reason` with the same
    // `recovery_exhausted:<retry_key>` prefix that
    // `ForcePlanBlocked` uses. This removes the historical
    // "stall_recovery" / "recovery_exhausted:stall_recovery:*"
    // dual-literal drift that masked the silent-success path.
    stall_envelopes
        .iter()
        .map(|e| {
            let payload = serde_json::json!({
                "reason": format!("recovery_exhausted:{}", e.retry_key),
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

    // 2026-07-06 silent-success P0-3 fix: the `reason` literal
    // emitted on `loop.stalled` MUST use the same
    // `recovery_exhausted:<retry_key>` prefix that
    // `ForcePlanBlocked` (event_loop/mod.rs:5589) uses for
    // `plan.blocked`. This keeps the workspace recovery ledger,
    // trusted events.jsonl, and shipper reason lookup aligned
    // so `shipper_reason::is_recoverable_plan_blocked_reason`
    // (shipper_reason.rs:64-77 prefix allowlist) cannot drift
    // across the two records. The old bare `"stall_recovery"`
    // literal would never appear in the prefix allowlist and
    // triggered the silent-success path masked as
    // REVIEW_COMPLETE(pass_with_residuals).
    #[test]
    fn emits_recovery_exhausted_prefix_reason_literal() {
        let ctx = RuntimeContext {
            recovery_envelopes: vec![EnvelopeSnapshot {
                retry_key: "stall_recovery:coordinator:task_resume:handoff_dispatch_timeout:*"
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
        match &actions[0] {
            RecoveryAction::PublishEvent { topic, payload } => {
                assert_eq!(topic, "loop.stalled");
                let parsed: serde_json::Value = serde_json::from_str(payload).unwrap();
                let reason = parsed.get("reason").and_then(|v| v.as_str()).unwrap();
                assert!(
                    reason.starts_with("recovery_exhausted:stall_recovery:"),
                    "loop.stalled reason literal must be aligned with ForcePlanBlocked's \
                     `recovery_exhausted:<retry_key>` prefix; got `{reason}`"
                );
                assert_eq!(
                    reason,
                    "recovery_exhausted:stall_recovery:coordinator:task_resume:handoff_dispatch_timeout:*"
                );
            }
            other => panic!("expected PublishEvent, got {other:?}"),
        }
    }
}
