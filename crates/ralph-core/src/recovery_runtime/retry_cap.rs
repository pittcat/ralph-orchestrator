//! 2026-06-29-007 plan U3: detect when a stall_recovery
//! injection on a review-chain hat (`review-coordinator` /
//! `review-synthesizer`) has reached the retry cap and
//! emit `plan.blocked` instead of another `task.resume`.
//!
//! Only the whitelist applies — stalls on other hats still
//! fall through to the existing stall_recovery path. The
//! whitelist pairs with the typed `RejectionKind::RetryCap`
//! introduced by U8, but the helper here is a pure
//! function over `RuntimeContext` so it can be unit-tested
//! independently.

use super::{RecoveryAction, RuntimeContext};

/// 2026-06-29-007 plan U3: retry cap for the review-chain
/// stall path. When the same `(hat, topic)` retry key has
/// been incremented `cap` times we escalate to
/// `plan.blocked(reason=<hat>_unrecoverable_after_<N>_retries)`
/// instead of letting the loop churn through another
/// `task.resume` injection.
pub const RETRY_CAP: u32 = 2;

/// 2026-06-29-007 plan U3: whitelist of `(hat, topic_prefix)`
/// pairs that qualify for the cap. Only review-chain stalls
/// count; other stalls continue to use the regular
/// stall_recovery path (which delegates to
/// progress-steward for self-healing attempts).
const REVIEW_WHITELIST: &[(&str, &str)] = &[
    ("review-coordinator", "review."),
    ("review-synthesizer", "review."),
];

/// Inspect the runtime context and, when the current
/// retry_key is on the whitelist AND has reached
/// [`RETRY_CAP`], emit a `ForcePlanBlocked` action. The
/// caller (recovery_runtime dispatcher) applies the action
/// at the end of the iteration.
pub fn detect_retry_cap_escalation(ctx: &RuntimeContext) -> Vec<RecoveryAction> {
    let Some(retry_key) = ctx.current_retry_key.clone() else {
        return Vec::new();
    };
    let Some(hat) = ctx.current_hat.as_deref() else {
        return Vec::new();
    };
    if !matches_whitelist(hat, &retry_key) {
        return Vec::new();
    }
    let Some(state) = ctx
        .retry_key_states
        .iter()
        .find(|s| s.retry_key == retry_key)
    else {
        return Vec::new();
    };
    if state.attempt_count < RETRY_CAP {
        return Vec::new();
    }
    vec![RecoveryAction::ForcePlanBlocked {
        reason: format!("{}_unrecoverable_after_{}_retries", hat, RETRY_CAP),
        retry_key,
    }]
}

fn matches_whitelist(hat: &str, retry_key: &str) -> bool {
    REVIEW_WHITELIST
        .iter()
        .any(|(h, prefix)| *h == hat && retry_key.contains(prefix))
}

/// 2026-06-29-007 plan U3 + U8: accessor for the current
/// attempt count of a retry key. Returns 0 when the key is
/// not tracked (no state yet — first injection).
pub fn get_retry_attempt(ctx: &RuntimeContext, retry_key: &str) -> u32 {
    ctx.retry_key_states
        .iter()
        .find(|s| s.retry_key == retry_key)
        .map_or(0, |s| s.attempt_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::RetryKeyState;

    fn ctx_with(
        hat: Option<&str>,
        key: Option<&str>,
        attempt_count: u32,
    ) -> RuntimeContext {
        let retry_key_states = match key {
            Some(k) => vec![RetryKeyState {
                retry_key: k.to_string(),
                last_outcome: "Pending".to_string(),
                outcome_history: vec![],
                attempt_count,
            }],
            None => vec![],
        };
        RuntimeContext {
            current_hat: hat.map(String::from),
            current_retry_key: key.map(String::from),
            retry_key_states,
            ..Default::default()
        }
    }

    #[test]
    fn non_whitelist_hat_does_not_escalate() {
        let ctx = ctx_with(
            Some("executor"),
            Some("stall_recovery:executor:work_done:handoff_dispatch_timeout:*"),
            5,
        );
        assert!(detect_retry_cap_escalation(&ctx).is_empty());
    }

    #[test]
    fn whitelist_hat_under_cap_does_not_escalate() {
        let ctx = ctx_with(
            Some("review-synthesizer"),
            Some("stall_recovery:review-synthesizer:review.dimensions.complete:timeout:*"),
            1,
        );
        assert!(detect_retry_cap_escalation(&ctx).is_empty());
    }

    #[test]
    fn whitelist_hat_at_cap_escalates() {
        let ctx = ctx_with(
            Some("review-synthesizer"),
            Some("stall_recovery:review-synthesizer:review.dimensions.complete:timeout:*"),
            RETRY_CAP,
        );
        let actions = detect_retry_cap_escalation(&ctx);
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            RecoveryAction::ForcePlanBlocked { reason, retry_key } => {
                assert!(reason.contains("review-synthesizer"));
                assert!(reason.contains("unrecoverable"));
                assert!(reason.contains("2"));
                assert!(retry_key.contains("review-synthesizer"));
            }
            other => panic!("expected ForcePlanBlocked, got {other:?}"),
        }
    }

    #[test]
    fn get_retry_attempt_returns_zero_when_untracked() {
        let ctx = ctx_with(None, None, 0);
        assert_eq!(get_retry_attempt(&ctx, "anything"), 0);
    }

    #[test]
    fn get_retry_attempt_returns_count_when_tracked() {
        let ctx = ctx_with(
            Some("review-coordinator"),
            Some("stall_recovery:review-coordinator:review.wave.ready:timeout:*"),
            3,
        );
        assert_eq!(
            get_retry_attempt(&ctx, "stall_recovery:review-coordinator:review.wave.ready:timeout:*"),
            3
        );
    }
}