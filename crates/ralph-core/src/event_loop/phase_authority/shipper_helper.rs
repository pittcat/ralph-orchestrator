//! 2026-07-02-006 plan U20: shipper routing helper.
//!
//! Pure decision for the shipper hat's KTD8 routing:
//! when the phase engine is enabled, the shipper must
//! honour `plan.complete` (i.e. forward to `REVIEW_COMPLETE`)
//! **only** when the engine is in a terminal-acceptable phase.
//! All other phases (or stalled recovery, or off-band emits)
//! get a `Deny` outcome.
//!
//! Test scenarios include the AE4 subset: stall-recovery
//! sub-strings must yield `Deny` so the shipper never
//! routes a stalled `REVIEW_COMPLETE` to `report.done`.

use serde::{Deserialize, Serialize};

/// Verdict from the shipper routing helper.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShipperDecision {
    /// Forward the emit to `REVIEW_COMPLETE` / `report.done`.
    Allow,
    /// Drop the emit (do not route to shipper). The runtime
    /// typically also writes a `plan.blocked(reason=
    /// phase_authority_deny)` so the operator can see why.
    Deny,
}

/// Inputs the runtime supplies when consulting the helper.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShipperRoutingContext {
    /// `true` when the phase engine is enabled (R1).
    pub phase_authority_enabled: bool,
    /// Current phase id (`None` when disabled or before
    /// first event).
    pub current_phase: Option<String>,
    /// Reason text the shipper would forward, when present.
    /// Stall-recovery sub-strings must trigger `Deny`.
    pub reason: Option<String>,
    /// `true` when the call carries a `plan.complete` payload.
    pub plan_complete_present: bool,
}

/// Pure decision: should the shipper forward this emit to
/// `REVIEW_COMPLETE`?
pub fn shipper_requires_plan_complete_when_phase_enabled(
    ctx: &ShipperRoutingContext,
) -> ShipperDecision {
    // The disabled-engine path keeps the pre-006 baseline:
    // the shipper accepts `plan.complete` and any reason.
    if !ctx.phase_authority_enabled {
        return ShipperDecision::Allow;
    }

    // Stall-recovery is a hard deny regardless of phase. The
    // substring check covers the canonical recovery phrases
    // ("stall_no_events", "loop_stalled_max_iterations",
    // etc.) and is intentionally conservative — false
    // positives here translate to an extra loop iteration,
    // not a missed terminal.
    if let Some(reason) = ctx.reason.as_deref() {
        if is_stall_recovery_reason(reason) {
            return ShipperDecision::Deny;
        }
    }

    // Engine is on: only `plan_end` may forward. Every other
    // phase holds the emit so the engine can complete its
    // transition table (the runtime's task.resume path is
    // responsible for re-driving the emit).
    match ctx.current_phase.as_deref() {
        Some("plan_end") if ctx.plan_complete_present => ShipperDecision::Allow,
        _ => ShipperDecision::Deny,
    }
}

/// Recognised stall-recovery sub-strings (AE4 subset).
/// Match is case-insensitive and substring-based; the goal
/// is to catch every canonical recovery bucket.
fn is_stall_recovery_reason(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    const STALL_TOKENS: &[&str] = &[
        "stall_no_events",
        "stall_no_progress",
        "loop_stalled_max_iterations",
        "stall_recovery",
        "loop_stalled",
    ];
    STALL_TOKENS.iter().any(|t| lower.contains(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_engine_accepts_any_reason() {
        let ctx = ShipperRoutingContext {
            phase_authority_enabled: false,
            current_phase: None,
            reason: Some("stall_no_events".to_string()),
            plan_complete_present: true,
        };
        assert_eq!(
            shipper_requires_plan_complete_when_phase_enabled(&ctx),
            ShipperDecision::Allow
        );
    }

    #[test]
    fn enabled_engine_deny_when_phase_is_unit_loop() {
        let ctx = ShipperRoutingContext {
            phase_authority_enabled: true,
            current_phase: Some("unit_loop".to_string()),
            reason: None,
            plan_complete_present: true,
        };
        assert_eq!(
            shipper_requires_plan_complete_when_phase_enabled(&ctx),
            ShipperDecision::Deny
        );
    }

    #[test]
    fn enabled_engine_allow_when_phase_is_plan_end_and_complete_present() {
        let ctx = ShipperRoutingContext {
            phase_authority_enabled: true,
            current_phase: Some("plan_end".to_string()),
            reason: None,
            plan_complete_present: true,
        };
        assert_eq!(
            shipper_requires_plan_complete_when_phase_enabled(&ctx),
            ShipperDecision::Allow
        );
    }

    #[test]
    fn enabled_engine_deny_plan_end_without_plan_complete() {
        let ctx = ShipperRoutingContext {
            phase_authority_enabled: true,
            current_phase: Some("plan_end".to_string()),
            reason: None,
            plan_complete_present: false,
        };
        assert_eq!(
            shipper_requires_plan_complete_when_phase_enabled(&ctx),
            ShipperDecision::Deny
        );
    }

    #[test]
    fn enabled_engine_deny_when_phase_is_review() {
        let ctx = ShipperRoutingContext {
            phase_authority_enabled: true,
            current_phase: Some("review".to_string()),
            reason: None,
            plan_complete_present: true,
        };
        assert_eq!(
            shipper_requires_plan_complete_when_phase_enabled(&ctx),
            ShipperDecision::Deny
        );
    }

    #[test]
    fn stall_recovery_reason_is_always_deny_ae4_subset() {
        // AE4 subset: every canonical recovery phrase must
        // yield Deny regardless of the current phase.
        for reason in [
            "stall_no_events",
            "stall_no_progress",
            "loop_stalled_max_iterations",
            "stall_recovery",
            "loop_stalled",
            "STALL_NO_EVENTS", // case-insensitive
            "substr_loop_stalled_max_iterations_xyz", // substring
        ] {
            let ctx = ShipperRoutingContext {
                phase_authority_enabled: true,
                current_phase: Some("plan_end".to_string()),
                reason: Some(reason.to_string()),
                plan_complete_present: true,
            };
            assert_eq!(
                shipper_requires_plan_complete_when_phase_enabled(&ctx),
                ShipperDecision::Deny,
                "reason {reason:?} must yield Deny"
            );
        }
    }

    #[test]
    fn non_stall_reason_at_plan_end_allows() {
        let ctx = ShipperRoutingContext {
            phase_authority_enabled: true,
            current_phase: Some("plan_end".to_string()),
            reason: Some("verdict_pass".to_string()),
            plan_complete_present: true,
        };
        assert_eq!(
            shipper_requires_plan_complete_when_phase_enabled(&ctx),
            ShipperDecision::Allow
        );
    }
}