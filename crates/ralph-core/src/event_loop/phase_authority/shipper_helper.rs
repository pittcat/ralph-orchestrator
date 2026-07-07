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

/// Validator terminal kind for the current step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidatorTerminalKind {
    Passed,
    Failed,
}

/// 2026-07-07-002 plan Unit 5: shipper must wait for validator terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShipperValidatorGateContext {
    /// Current plan step id (e.g. `step-02`).
    pub current_step: Option<String>,
    /// Step recorded by the latest validator terminal (`test.passed` / `test.failed`).
    pub validator_terminal_step: Option<String>,
    pub validator_terminal_kind: Option<ValidatorTerminalKind>,
    /// Incoming `plan.blocked` / stall recovery reason when shipper would pass.
    pub plan_blocked_reason: Option<String>,
    /// True when shipper would emit `pass_or_fail=pass` or `pass_with_residuals`.
    pub attempting_success_ship: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ShipperValidatorGateDecision {
    Allow,
    DenyWaitForValidator { current_step: String },
    HardFail { reason: String },
}

/// Pure decision: may shipper emit success `REVIEW_COMPLETE` for the current step?
#[must_use]
pub fn evaluate_shipper_validator_gate(
    ctx: &ShipperValidatorGateContext,
) -> ShipperValidatorGateDecision {
    let current = match ctx.current_step.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => {
            return ShipperValidatorGateDecision::HardFail {
                reason: "shipper_validator_gate:missing_current_step".to_string(),
            };
        }
    };

    if !ctx.attempting_success_ship {
        return ShipperValidatorGateDecision::Allow;
    }

    if let Some(reason) = ctx.plan_blocked_reason.as_deref() {
        if is_stall_recovery_reason(reason) && ctx.validator_terminal_step.is_none() {
            return ShipperValidatorGateDecision::HardFail {
                reason: "shipper_validator_gate:stall_recovery_without_validator_terminal"
                    .to_string(),
            };
        }
    }

    let terminal_step = match ctx.validator_terminal_step.as_deref() {
        Some(s) => s,
        None => {
            return ShipperValidatorGateDecision::DenyWaitForValidator {
                current_step: current.to_string(),
            };
        }
    };

    if terminal_step != current {
        return ShipperValidatorGateDecision::DenyWaitForValidator {
            current_step: current.to_string(),
        };
    }

    ShipperValidatorGateDecision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validator_gate_stall_without_terminal_hard_fails() {
        let ctx = ShipperValidatorGateContext {
            current_step: Some("step-02".to_string()),
            validator_terminal_step: None,
            validator_terminal_kind: None,
            plan_blocked_reason: Some(
                "recovery_exhausted:stall_recovery:validator:work_done:handoff_dispatch_timeout"
                    .to_string(),
            ),
            attempting_success_ship: true,
        };
        assert!(matches!(
            evaluate_shipper_validator_gate(&ctx),
            ShipperValidatorGateDecision::HardFail { .. }
        ));
    }

    #[test]
    fn validator_gate_current_step_passed_allows() {
        let ctx = ShipperValidatorGateContext {
            current_step: Some("step-02".to_string()),
            validator_terminal_step: Some("step-02".to_string()),
            validator_terminal_kind: Some(ValidatorTerminalKind::Passed),
            plan_blocked_reason: None,
            attempting_success_ship: true,
        };
        assert_eq!(
            evaluate_shipper_validator_gate(&ctx),
            ShipperValidatorGateDecision::Allow
        );
    }

    #[test]
    fn validator_gate_current_step_failed_allows_fail_semantics() {
        let ctx = ShipperValidatorGateContext {
            current_step: Some("step-02".to_string()),
            validator_terminal_step: Some("step-02".to_string()),
            validator_terminal_kind: Some(ValidatorTerminalKind::Failed),
            plan_blocked_reason: None,
            attempting_success_ship: false,
        };
        assert_eq!(
            evaluate_shipper_validator_gate(&ctx),
            ShipperValidatorGateDecision::Allow
        );
    }

    #[test]
    fn validator_gate_old_step_denies() {
        let ctx = ShipperValidatorGateContext {
            current_step: Some("step-02".to_string()),
            validator_terminal_step: Some("step-01".to_string()),
            validator_terminal_kind: Some(ValidatorTerminalKind::Passed),
            plan_blocked_reason: None,
            attempting_success_ship: true,
        };
        assert!(matches!(
            evaluate_shipper_validator_gate(&ctx),
            ShipperValidatorGateDecision::DenyWaitForValidator { .. }
        ));
    }

    #[test]
    fn validator_gate_missing_terminal_denies_success_ship() {
        let ctx = ShipperValidatorGateContext {
            current_step: Some("step-02".to_string()),
            validator_terminal_step: None,
            validator_terminal_kind: None,
            plan_blocked_reason: None,
            attempting_success_ship: true,
        };
        assert!(matches!(
            evaluate_shipper_validator_gate(&ctx),
            ShipperValidatorGateDecision::DenyWaitForValidator { .. }
        ));
    }

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
            "STALL_NO_EVENTS",                        // case-insensitive
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
