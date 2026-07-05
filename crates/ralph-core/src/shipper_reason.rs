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
    // 2026-07-06 U3 (DEV-003 fix): bare `recovery_exhausted` removed.
    // No current code path emits a plan.blocked reason with the bare
    // literal (runtime-recovery injects `recovery_exhausted:<retry_key>`
    // since the 2026-07-06 U2 record_event fix). The bare literal
    // short-circuit was a fail-open that masked unknown retry_keys
    // (`recovery_exhausted:stall_recovery:validator:work_done:*`); with
    // the prefix allowlist tightened below, fail-close is the default.
    "review_failed",
    "precheck_failed",
    "default_publishes",
];

/// 2026-07-04-024019 run P0-3: explicit allowlist of
/// `recovery_exhausted:<retry_key>` prefixes that the drift engine is
/// known to emit. The previous `starts_with("recovery_exhausted:")`
/// blanket admitted any structured suffix, which surfaced as
/// `REVIEW_COMPLETE(pass_with_residuals)` for retry_keys we never
/// meant to whitelist (e.g. an unknown drift-engine escalation). New
/// rules: keep admitting the prefixes that shipper routing knows
/// how to translate; reject any other `recovery_exhausted:<...>` as
/// not recoverable (fail-close).
///
/// Note: drift-engine emit paths use BOTH dotted (`task.resume`) and
/// underscored (`task_resume`) retry-key formats depending on which
/// uploader invoked them (`event_loop/mod.rs:5543` + loop_runner
/// `runner.rs:1855`). Both variants are pinned here so the
/// `is_recoverable_plan_blocked_reason` decision matches existing
/// test fixtures (`recoverable_literals_match` asserts
/// `recovery_exhausted:coordinator:task.resume:handoff` is recoverable).
const RECOVERABLE_RECOVERY_EXHAUSTED_PREFIXES: &[&str] = &[
    // Dotted retry-key variants (legacy / drift-engine direct emit)
    "recovery_exhausted:coordinator:task.resume",
    "recovery_exhausted:dimension_reviewer:review.dimension.ready",
    // Underscored retry-key variants (2026-07-03-005 plan / P0-1 path)
    "recovery_exhausted:coordinator:task_resume",
    "recovery_exhausted:dimension_reviewer:review_dimension_ready",
    // Stall-recovery escalation paths (2026-07-03-005 explicitly closed these)
    "recovery_exhausted:stall_recovery:coordinator:task_resume:handoff_dispatch_timeout",
    "recovery_exhausted:stall_recovery:coordinator:task.resume:handoff_dispatch_timeout",
    "recovery_exhausted:stall_recovery:dimension_reviewer:review_dimension_ready:handoff_dispatch_timeout",
    "recovery_exhausted:stall_recovery:dimension_reviewer:review.dimension.ready:handoff_dispatch_timeout",
];

/// Normalize a `plan.blocked.reason` for whitelist lookup.
pub fn normalize_plan_blocked_reason(reason: &str) -> String {
    reason.trim().to_lowercase()
}

/// True when `reason` is on the shipper recoverable whitelist.
///
/// `recovery_exhausted:<retry_key>` (drift engine) is treated as recoverable
/// only when `<retry_key>` is on the explicit prefix allowlist (see
/// `RECOVERABLE_RECOVERY_EXHAUSTED_PREFIXES`). Bare `recovery_exhausted`
/// (no retry_key) is **not** recoverable — fail-close into
/// `REVIEW_COMPLETE(pass_or_fail=fail)`. Anything else
/// (`recovery_exhausted:foo`, `recovery_exhausted:stall_recovery:...:weird`,
/// etc.) is NOT recoverable — fail-close into
/// `REVIEW_COMPLETE(pass_or_fail=fail)`.
pub fn is_recoverable_plan_blocked_reason(reason: &str) -> bool {
    let normalized = normalize_plan_blocked_reason(reason);
    if RECOVERABLE_REASONS.contains(&normalized.as_str()) {
        return true;
    }
    // 2026-07-04-024019 run P0-3: replace blanket `starts_with("recovery_exhausted:")`
    // with an explicit prefix allowlist so the drift engine cannot widen
    // shipper routing to new retry_keys.
    RECOVERABLE_RECOVERY_EXHAUSTED_PREFIXES
        .iter()
        .any(|p| normalized.starts_with(p))
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
        // 2026-07-06 U3 (DEV-003 fix): bare `recovery_exhausted` is
        // NOT recoverable any more. The runtime always emits
        // `recovery_exhausted:<retry_key>` (see event_loop/mod.rs
        // ForcePlanBlocked payload), so bare literal would only appear
        // from a malformed drift-engine path; fail-close is the
        // intended behavior.
        assert!(!is_recoverable_plan_blocked_reason("recovery_exhausted"));
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
    fn stall_recovery_validator_not_recoverable() {
        // 2026-07-06 U3 (DEV-003 fix): the validator-stall retry_key
        // was specifically removed from the prefix allowlist in
        // 2026-07-03-005 (P0 fix C2+C8) because it masked the
        // mechanism-side silent drop / handoff_dispatch misroute as
        // pass_with_residuals. Verify it remains fail-close.
        assert!(!is_recoverable_plan_blocked_reason(
            "recovery_exhausted:stall_recovery:validator:work_done:handoff_dispatch_timeout:*"
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
        // 2026-07-06 U3 (DEV-003 fix): bare `recovery_exhausted` is
        // NOT recoverable any more. REVIEW_COMPLETE(pass) after a
        // bare `recovery_exhausted` plan.blocked reason must be
        // rejected by `check_review_complete_shipper_routing` so
        // shipper fail-closes instead of masking the stall as
        // pass_with_residuals. Previously this test asserted that
        // bare `recovery_exhausted` admitted pass — that was the
        // exact fail-open path that masked the silent-success
        // mechanism defect fixed in DEV-002.
        let finding = check_review_complete_shipper_routing(
            Some(r#"{"pass_or_fail":"pass","verdict":"pass_with_residuals"}"#),
            Some("recovery_exhausted"),
        )
        .expect("bare recovery_exhausted must NOT admit pass after DEV-003 fix");
        assert!(
            finding
                .message
                .contains("shipper_non_recoverable_reason_promoted_to_pass")
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

    // 2026-07-04-024019 run P0-3: an UNKNOWN `recovery_exhausted:<retry_key>`
    // (not on the explicit prefix allowlist) must NOT translate into
    // `pass_with_residuals`. The drift engine's blanket `starts_with`
    // promotion has been replaced with a prefix allowlist so unknown
    // retry_keys fail-close into `REVIEW_COMPLETE(pass_or_fail=fail)`.
    #[test]
    fn unknown_recovery_exhausted_prefix_fail_closes() {
        assert!(!is_recoverable_plan_blocked_reason(
            "recovery_exhausted:unknown:retry:key"
        ));
        assert!(!is_recoverable_plan_blocked_reason(
            "recovery_exhausted:stall_recovery:coordinator:task_resume:NOT_in_allowlist"
        ));
        // still recoverable: the four allowlisted prefixes
        assert!(is_recoverable_plan_blocked_reason(
            "recovery_exhausted:stall_recovery:coordinator:task_resume:handoff_dispatch_timeout:anything-else"
        ));
    }

    // 2026-07-04-024019 run P0-3: BOTH dotted (`task.resume`) and
    // underscored (`task_resume`) retry-key variants must keep their
    // recoverable status. `recoverable_literals_match` and the existing
    // `recovery_exhausted_drift_engine_promotion_still_recoverable`
    // tests already assert the dotted form; this regression asserts
    // the underscored form remains recoverable after we replaced the
    // blanket `starts_with("recovery_exhausted:")` with a prefix
    // allowlist.
    #[test]
    fn recovery_exhausted_underscored_variants_recoverable() {
        assert!(is_recoverable_plan_blocked_reason(
            "recovery_exhausted:coordinator:task_resume:handoff"
        ));
        assert!(is_recoverable_plan_blocked_reason(
            "recovery_exhausted:dimension_reviewer:review_dimension_ready:handoff"
        ));
        assert!(is_recoverable_plan_blocked_reason(
            "recovery_exhausted:stall_recovery:dimension_reviewer:review_dimension_ready:handoff_dispatch_timeout:drift-engine"
        ));
    }
}
