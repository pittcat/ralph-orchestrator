//! Runtime strict-match routing for shipper `plan.blocked` → `REVIEW_COMPLETE`.
//!
//! U7 of plan 2026-07-02-005: the recoverable-reason whitelist lives in the
//! preset prompt for agent guidance; this module is the mechanism backstop so
//! a shipper cannot promote a non-whitelist `plan.blocked.reason` to
//! `REVIEW_COMPLETE(pass_or_fail=pass)`.

use crate::event_policy::{PolicyFinding, ViolationType};

/// Canonical recoverable `plan.blocked.reason` literals (trim + lowercase exact
/// match). Mirrors `presets/en/ce-executor-serial.yml` shipper STRICT-MATCH list.
///
/// 2026-07-03-002 plan U3 (P0-2 fix): added `default_publishes`. The 075227
/// incident root cause was that the preset prompt + schema listed
/// `default_publishes` as recoverable but this mechanism backstop did NOT,
/// so a coordinator silence that triggered runtime default-injection was
/// still hard-failed by `check_review_complete_shipper_routing`. Both the
/// preset (agent guidance) and this module (mechanism) must agree.
///
/// 2026-07-03-005 plan (P0 fix C2+C8): explicitly removed
/// `stall_recovery:coordinator:task_resume:handoff_dispatch_timeout:*` and
/// `stall_recovery:dimension_reviewer:review_dimension_ready:handoff_dispatch_timeout:*`.
/// These two retry_keys represent the "mechanism-side silent drop → retry
/// loop → stall counter escalation" path that the shipper previously
/// translated into `pass_with_residuals`, masking the true root cause
/// (M-1 isolated budget + M-2 handoff_dispatch routing). After removal, the
/// escalation path takes the hard-fail branch, surfacing the real cause
/// through `REVIEW_COMPLETE(fail)`. The `recovery_exhausted:stall_recovery:...`
/// drift-engine promotion path is preserved by the `starts_with` fallback
/// in `is_recoverable_plan_blocked_reason` (see 2026-07-02-005 U7).
const RECOVERABLE_REASONS: &[&str] = &[
    "loop_stalled_max_iterations",
    "steward_escalation",
    "review_terminal_drift",
    "recovery_exhausted",
    "review_failed",
    "precheck_failed",
    "default_publishes",
];

/// Normalize a `plan.blocked.reason` for whitelist lookup.
pub fn normalize_plan_blocked_reason(reason: &str) -> String {
    reason.trim().to_lowercase()
}

/// True when `reason` is on the shipper recoverable whitelist.
///
/// `recovery_exhausted:{retry_key}` (drift engine) is treated as recoverable
/// because the schema whitelist entry is the bare literal `recovery_exhausted`
/// while production emits a structured suffix — still STRICT, not substring
/// promotion of unrelated recovery buckets like `stall_no_events recovery:`.
pub fn is_recoverable_plan_blocked_reason(reason: &str) -> bool {
    let normalized = normalize_plan_blocked_reason(reason);
    if RECOVERABLE_REASONS.contains(&normalized.as_str()) {
        return true;
    }
    normalized.starts_with("recovery_exhausted:")
}

/// Extract `reason` from a `plan.blocked` JSON payload.
pub fn extract_plan_blocked_reason(payload: Option<&str>) -> Option<String> {
    let p = payload?;
    let obj = serde_json::from_str::<serde_json::Value>(p).ok()?;
    obj.get("reason")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Reject `REVIEW_COMPLETE(pass)` when the preceding `plan.blocked` reason is
/// outside the recoverable whitelist.
pub fn check_review_complete_shipper_routing(
    payload: Option<&str>,
    last_plan_blocked_reason: Option<&str>,
) -> Option<PolicyFinding> {
    let blocked_reason = last_plan_blocked_reason?;
    if is_recoverable_plan_blocked_reason(blocked_reason) {
        return None;
    }
    let pass_or_fail = payload
        .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
        .and_then(|obj| {
            obj.get("pass_or_fail")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();
    if pass_or_fail.trim().eq_ignore_ascii_case("pass") {
        return Some(PolicyFinding {
            topic: "REVIEW_COMPLETE".to_string(),
            violation_type: ViolationType::SemanticGateViolation {
                gate: "shipper_non_recoverable_reason_promoted_to_pass".to_string(),
                context: format!(
                    "plan.blocked reason '{}' is not on the recoverable whitelist",
                    blocked_reason
                ),
            },
            message: format!(
                "shipper_non_recoverable_reason_promoted_to_pass: REVIEW_COMPLETE \
                 pass_or_fail=pass is forbidden after plan.blocked(reason='{blocked_reason}'); \
                 route to pass_or_fail=fail"
            ),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recoverable_literals_match() {
        assert!(is_recoverable_plan_blocked_reason("recovery_exhausted"));
        assert!(is_recoverable_plan_blocked_reason("  REVIEW_FAILED "));
        assert!(is_recoverable_plan_blocked_reason(
            "recovery_exhausted:coordinator:task.resume:handoff"
        ));
        // 2026-07-03-002 U3: default_publishes must be recoverable so that
        // runtime-injected plan.blocked (coordinator silence) routes to
        // REVIEW_COMPLETE(pass_with_residuals) instead of hard-failing.
        assert!(is_recoverable_plan_blocked_reason("default_publishes"));
        assert!(is_recoverable_plan_blocked_reason("  Default_Publishes "));
    }

    #[test]
    fn non_recoverable_substring_buckets_rejected() {
        assert!(!is_recoverable_plan_blocked_reason(
            "stall_no_events recovery: progress-steward did not advance"
        ));
        assert!(!is_recoverable_plan_blocked_reason("work_failed"));
    }

    #[test]
    fn review_complete_pass_after_hard_fail_reason_blocked() {
        let finding = check_review_complete_shipper_routing(
            Some(r#"{"pass_or_fail":"pass","verdict":"pass"}"#),
            Some("stall_no_events recovery: steward"),
        )
        .expect("must reject pass promotion");
        assert!(
            finding
                .message
                .contains("shipper_non_recoverable_reason_promoted_to_pass")
        );
    }

    #[test]
    fn review_complete_fail_after_hard_fail_reason_allowed() {
        assert!(
            check_review_complete_shipper_routing(
                Some(r#"{"pass_or_fail":"fail","verdict":"fail"}"#),
                Some("stall_no_events recovery: steward"),
            )
            .is_none()
        );
    }

    #[test]
    fn review_complete_pass_after_recoverable_reason_allowed() {
        assert!(
            check_review_complete_shipper_routing(
                Some(r#"{"pass_or_fail":"pass","verdict":"pass_with_residuals"}"#),
                Some("recovery_exhausted"),
            )
            .is_none()
        );
    }

    // 2026-07-03-005 plan (P0 fix C2+C8): stall_recovery:* retry_keys are no
    // longer recoverable. The escalation path must take the hard-fail branch
    // so the shipper surfaces the real root cause instead of masking it as
    // pass_with_residuals.
    #[test]
    fn stall_recovery_coordinator_retry_key_no_longer_recoverable() {
        assert!(!is_recoverable_plan_blocked_reason(
            "stall_recovery:coordinator:task_resume:handoff_dispatch_timeout:*"
        ));
        assert!(!is_recoverable_plan_blocked_reason(
            "  STALL_RECOVERY:COORDINATOR:TASK_RESUME:HANDOFF_DISPATCH_TIMEOUT:* "
        ));
    }

    #[test]
    fn stall_recovery_dimension_reviewer_retry_key_no_longer_recoverable() {
        assert!(!is_recoverable_plan_blocked_reason(
            "stall_recovery:dimension_reviewer:review_dimension_ready:handoff_dispatch_timeout:*"
        ));
    }

    // Regression: shipper must hard-fail pass promotion after a stall_recovery
    // plan.blocked, instead of letting it through as pass_with_residuals.
    #[test]
    fn review_complete_pass_after_stall_recovery_blocked() {
        let finding = check_review_complete_shipper_routing(
            Some(r#"{"pass_or_fail":"pass","verdict":"pass_with_residuals"}"#),
            Some("stall_recovery:coordinator:task_resume:handoff_dispatch_timeout:*"),
        )
        .expect("stall_recovery must be hard-failed, not pass_with_residuals");
        assert!(
            finding
                .message
                .contains("shipper_non_recoverable_reason_promoted_to_pass")
        );
    }

    // Regression: drift-engine promotion `recovery_exhausted:stall_recovery:*`
    // (different prefix) must still pass through the `starts_with` fallback.
    #[test]
    fn recovery_exhausted_drift_engine_promotion_still_recoverable() {
        assert!(is_recoverable_plan_blocked_reason(
            "recovery_exhausted:stall_recovery:dimension_reviewer:review_dimension_ready:handoff_dispatch_timeout:drift-engine"
        ));
        assert!(is_recoverable_plan_blocked_reason(
            "recovery_exhausted:coordinator:task_resume:handoff"
        ));
    }
}
