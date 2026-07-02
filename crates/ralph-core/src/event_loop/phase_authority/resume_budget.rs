//! 2026-07-02-006 plan U22: phase-violation resume budget.
//!
//! Pure decision over the violation counter and the
//! configured `max_resume_per_hat`. The runtime calls
//! `should_admit_resume(...)` before publishing each
//! `task.resume(reason_code=phase_violation)` envelope;
//! when the budget is exhausted the runtime falls back to
//! the configured `on_exhausted` policy (e.g. emit
//! `plan.blocked(reason=phase_violation_exhausted)`).

use super::config::ViolationPolicyConfig;
use super::snapshot::{PhaseSnapshot, ViolationKind};

/// Outcome of a budget consultation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetDecision {
    /// The resume envelope may be admitted.
    Admit,
    /// The resume envelope must be dropped; the runtime
    /// should also emit `plan.blocked` per the configured
    /// `on_exhausted` policy.
    Exhausted,
}

/// Pure decision: should the next `task.resume` be admitted
/// for `(hat, kind)`?
///
/// `current_count` is the violation count *before* the
/// current candidate resume is admitted. The function
/// returns `Admit` when `current_count < max`, `Exhausted`
/// otherwise. U3's lint pins the default `max = 3`.
pub fn should_admit_resume(
    policy: &ViolationPolicyConfig,
    current_count: u32,
) -> BudgetDecision {
    if current_count < policy.max_resume_per_hat {
        BudgetDecision::Admit
    } else {
        BudgetDecision::Exhausted
    }
}

/// Convenience: pull the current count from a snapshot and
/// delegate to `should_admit_resume`. Returns `Admit` when
/// the snapshot has no entry for `(hat, kind)` — i.e. the
/// candidate is the first violation.
pub fn should_admit_resume_from_snapshot(
    policy: &ViolationPolicyConfig,
    snap: &PhaseSnapshot,
    hat: &str,
    kind: ViolationKind,
) -> BudgetDecision {
    let count = snap
        .violation_counts
        .get(&(hat.to_string(), kind))
        .copied()
        .unwrap_or(0);
    should_admit_resume(policy, count)
}

/// Default exhaustion policy decision: `plan_blocked` →
/// caller should emit `plan.blocked`; `silent_drop` →
/// caller should drop the emit silently.
pub fn on_exhausted_action(policy: &ViolationPolicyConfig) -> ExhaustedAction {
    match policy.on_exhausted.as_str() {
        "silent_drop" | "silent_drop_resume" => ExhaustedAction::SilentDrop,
        // Default and explicit `plan_blocked`.
        _ => ExhaustedAction::PlanBlocked,
    }
}

/// What the runtime should do when the budget is exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExhaustedAction {
    /// Emit `plan.blocked(reason=phase_violation_exhausted)`.
    PlanBlocked,
    /// Drop the offending emit silently.
    SilentDrop,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_admits_three_resumes() {
        let policy = ViolationPolicyConfig::default();
        assert_eq!(policy.max_resume_per_hat, 3);
        assert_eq!(policy.on_exhausted, "plan_blocked");
        assert_eq!(should_admit_resume(&policy, 0), BudgetDecision::Admit);
        assert_eq!(should_admit_resume(&policy, 1), BudgetDecision::Admit);
        assert_eq!(should_admit_resume(&policy, 2), BudgetDecision::Admit);
        assert_eq!(should_admit_resume(&policy, 3), BudgetDecision::Exhausted);
        assert_eq!(should_admit_resume(&policy, 4), BudgetDecision::Exhausted);
    }

    #[test]
    fn explicit_max_seven_admits_seven_resumes() {
        let policy = ViolationPolicyConfig {
            max_resume_per_hat: 7,
            on_exhausted: "silent_drop".to_string(),
        };
        assert_eq!(should_admit_resume(&policy, 6), BudgetDecision::Admit);
        assert_eq!(should_admit_resume(&policy, 7), BudgetDecision::Exhausted);
        assert_eq!(
            on_exhausted_action(&policy),
            ExhaustedAction::SilentDrop
        );
    }

    #[test]
    fn snapshot_lookup_returns_first_violation_as_admit() {
        let policy = ViolationPolicyConfig::default();
        let snap = PhaseSnapshot::with_phase_id("unit_loop");
        assert_eq!(
            should_admit_resume_from_snapshot(
                &policy,
                &snap,
                "coordinator",
                ViolationKind::PhaseViolation
            ),
            BudgetDecision::Admit
        );
    }

    #[test]
    fn snapshot_lookup_after_three_violations_returns_exhausted() {
        let policy = ViolationPolicyConfig::default();
        let snap = PhaseSnapshot::with_phase_id("unit_loop")
            .bump_violation("coordinator", ViolationKind::PhaseViolation)
            .bump_violation("coordinator", ViolationKind::PhaseViolation)
            .bump_violation("coordinator", ViolationKind::PhaseViolation);
        assert_eq!(
            should_admit_resume_from_snapshot(
                &policy,
                &snap,
                "coordinator",
                ViolationKind::PhaseViolation
            ),
            BudgetDecision::Exhausted
        );
    }

    #[test]
    fn snapshot_lookup_isolates_per_hat() {
        let policy = ViolationPolicyConfig::default();
        let snap = PhaseSnapshot::with_phase_id("unit_loop")
            .bump_violation("coordinator", ViolationKind::PhaseViolation)
            .bump_violation("coordinator", ViolationKind::PhaseViolation)
            .bump_violation("coordinator", ViolationKind::PhaseViolation);
        // Different hat: still admits.
        assert_eq!(
            should_admit_resume_from_snapshot(
                &policy,
                &snap,
                "executor",
                ViolationKind::PhaseViolation
            ),
            BudgetDecision::Admit
        );
    }

    #[test]
    fn on_exhausted_action_default_is_plan_blocked() {
        let policy = ViolationPolicyConfig::default();
        assert_eq!(
            on_exhausted_action(&policy),
            ExhaustedAction::PlanBlocked
        );
    }
}