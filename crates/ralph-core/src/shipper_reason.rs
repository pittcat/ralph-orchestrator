//! Runtime strict-match routing for shipper `plan.blocked` → `REVIEW_COMPLETE`.
//!
//! U7 of plan 2026-07-02-005: the recoverable-reason whitelist lives in the
//! preset prompt for agent guidance; this module is the mechanism backstop so
//! a shipper cannot promote a non-whitelist `plan.blocked.reason` to
//! `REVIEW_COMPLETE(pass_or_fail=pass)`.

use crate::event_policy::{PolicyFinding, ViolationType};

/// Canonical recoverable `plan.blocked.reason` literals (trim + lowercase exact
/// match). Mirrors `presets/en/ce-executor-serial.yml` shipper STRICT-MATCH list.
const RECOVERABLE_REASONS: &[&str] = &[
    "loop_stalled_max_iterations",
    "steward_escalation",
    "review_terminal_drift",
    "recovery_exhausted",
    "review_failed",
    "precheck_failed",
    "stall_recovery:coordinator:task_resume:handoff_dispatch_timeout:*",
    "stall_recovery:dimension_reviewer:review_dimension_ready:handoff_dispatch_timeout:*",
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
}
