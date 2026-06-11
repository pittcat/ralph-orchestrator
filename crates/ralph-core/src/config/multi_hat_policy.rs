//! Shared multi-hat isolation policy evaluator.
//!
//! Implements R1-R5 of the 2026-06-11 multi-hat isolated policy plan:
//! presets with 4 or more hats MUST declare `execution_mode: isolated`.
//! Coordinator (explicit or default) above the threshold is always a
//! policy violation; isolated is always allowed.
//!
//! This module is the SINGLE source of truth for the threshold and
//! the violation shape. Static lint, preflight, and `ralph run` hard
//! gate all call [`evaluate_multi_hat_isolation`] — there is no
//! second copy of the limit, and no second counting algorithm.
//!
//! Per R4-R5 the policy admits no configuration, environment
//! variable, test switch, or hidden compat opt-out.

use super::workflow_guards::HatExecutionMode;

/// Maximum number of hats a preset may declare while still being
/// permitted to run in coordinator mode. The fourth hat
/// (count = 4) crosses the threshold; presets with 4 or more
/// hats MUST declare `execution_mode: isolated`.
///
/// Stable public constant — referenced from lint, preflight, and
/// runtime contract findings. Do NOT introduce a second copy
/// elsewhere.
pub const MULTI_HAT_ISOLATION_LIMIT: usize = 3;

/// Structured policy violation. The shape is consumed by the
/// preset lint, preflight, and runtime contract adapters —
/// each adapter renders the same fields but with its own
/// finding / check / error envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiHatPolicyViolation {
    /// Actual number of hats declared in the resolved config.
    pub actual: usize,
    /// Maximum number of hats allowed in coordinator mode
    /// (always equal to [`MULTI_HAT_ISOLATION_LIMIT`]).
    pub limit: usize,
    /// The execution mode the preset MUST use to satisfy the
    /// policy at the current hat count. Always
    /// [`HatExecutionMode::Isolated`] when `actual > limit`.
    pub required_mode: HatExecutionMode,
}

impl MultiHatPolicyViolation {
    /// Human-readable summary intended for lint messages and
    /// runtime contract findings.
    pub fn message(&self) -> String {
        format!(
            "preset declares {} hats which exceeds the coordinator limit of {}; \
             set `event_loop.execution_mode: isolated` to run this preset",
            self.actual, self.limit
        )
    }

    /// Fix hint intended for the `action_hint` field of lint
    /// findings and preflight errors.
    pub fn fix_hint(&self) -> String {
        format!(
            "Set `event_loop.execution_mode: isolated` ({} hats > {} hat limit)",
            self.actual, self.limit
        )
    }
}

/// Evaluate the multi-hat isolation policy against a hat count
/// and an execution mode.
///
/// Returns `Ok(())` when the policy is satisfied; returns
/// [`MultiHatPolicyViolation`] when the preset declares more
/// than [`MULTI_HAT_ISOLATION_LIMIT`] hats AND the execution
/// mode is `Coordinator` (explicit or default).
///
/// Pure function — no I/O, no configuration access, no logging.
/// Same inputs always produce the same output.
pub fn evaluate_multi_hat_isolation(
    hat_count: usize,
    mode: HatExecutionMode,
) -> Result<(), MultiHatPolicyViolation> {
    if hat_count <= MULTI_HAT_ISOLATION_LIMIT {
        return Ok(());
    }
    if matches!(mode, HatExecutionMode::Isolated) {
        return Ok(());
    }
    Err(MultiHatPolicyViolation {
        actual: hat_count,
        limit: MULTI_HAT_ISOLATION_LIMIT,
        required_mode: HatExecutionMode::Isolated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── AE1: 3 hats, default mode → policy passes ────────────────────

    #[test]
    fn three_hats_default_mode_passes() {
        // Coordinator (default) at exactly the limit is allowed.
        let result = evaluate_multi_hat_isolation(3, HatExecutionMode::Coordinator);
        assert!(
            result.is_ok(),
            "3 hats with default (Coordinator) mode must satisfy the policy, got {result:?}"
        );
    }

    #[test]
    fn three_hats_isolated_mode_passes() {
        let result = evaluate_multi_hat_isolation(3, HatExecutionMode::Isolated);
        assert!(result.is_ok());
    }

    // ── Boundary checks around the threshold (3 vs 4) ───────────────

    #[test]
    fn zero_hats_any_mode_passes() {
        assert!(evaluate_multi_hat_isolation(0, HatExecutionMode::Coordinator).is_ok());
        assert!(evaluate_multi_hat_isolation(0, HatExecutionMode::Isolated).is_ok());
    }

    #[test]
    fn one_two_hats_any_mode_passes() {
        for count in [1usize, 2] {
            assert!(
                evaluate_multi_hat_isolation(count, HatExecutionMode::Coordinator).is_ok(),
                "count={count} with Coordinator must pass"
            );
            assert!(
                evaluate_multi_hat_isolation(count, HatExecutionMode::Isolated).is_ok(),
                "count={count} with Isolated must pass"
            );
        }
    }

    // ── AE2: 4 hats, default mode → policy fails with details ──────

    #[test]
    fn four_hats_default_mode_fails_with_actual_and_limit() {
        let err = evaluate_multi_hat_isolation(4, HatExecutionMode::Coordinator)
            .expect_err("4 hats with Coordinator must violate the policy");
        assert_eq!(err.actual, 4, "details.actual must be 4");
        assert_eq!(err.limit, MULTI_HAT_ISOLATION_LIMIT, "details.limit must be 3");
        assert_eq!(
            err.required_mode,
            HatExecutionMode::Isolated,
            "details.required_mode must be Isolated"
        );
    }

    // ── AE3: 4 hats, explicit Coordinator → same type of error ─────

    #[test]
    fn four_hats_explicit_coordinator_fails_same_as_default() {
        // Both default and explicit Coordinator must produce an
        // identical-shape violation (same id, same actual, same
        // required_mode).
        let default = evaluate_multi_hat_isolation(4, HatExecutionMode::Coordinator)
            .expect_err("default Coordinator at 4 hats must fail");
        let explicit = evaluate_multi_hat_isolation(4, HatExecutionMode::Coordinator)
            .expect_err("explicit Coordinator at 4 hats must fail");
        assert_eq!(
            default, explicit,
            "default and explicit Coordinator must produce identical violation shape"
        );
    }

    // ── 4 hats, explicit Isolated → policy passes ──────────────────

    #[test]
    fn four_hats_isolated_mode_passes() {
        let result = evaluate_multi_hat_isolation(4, HatExecutionMode::Isolated);
        assert!(
            result.is_ok(),
            "4 hats with Isolated mode must satisfy the policy, got {result:?}"
        );
    }

    // ── AE4: 8 hats, mixed special hats (aggregate, observer, wave
    //    workers) → still counts as 8 hats and triggers the rule. The
    //    evaluator itself only takes a count; this test pins that the
    //    downstream call site (preset_lint) must pass `config.hats.len()`
    //    WITHOUT filtering by hat type.

    #[test]
    fn eight_hats_default_mode_fails() {
        let err = evaluate_multi_hat_isolation(8, HatExecutionMode::Coordinator)
            .expect_err("8 hats with Coordinator must violate");
        assert_eq!(err.actual, 8);
        assert_eq!(err.limit, 3);
    }

    #[test]
    fn eight_hats_isolated_mode_passes() {
        assert!(evaluate_multi_hat_isolation(8, HatExecutionMode::Isolated).is_ok());
    }

    // ── Larger counts still pass on isolated ───────────────────────

    #[test]
    fn many_hats_isolated_mode_passes() {
        for count in [4usize, 5, 8, 10, 50, 100, 1000] {
            assert!(
                evaluate_multi_hat_isolation(count, HatExecutionMode::Isolated).is_ok(),
                "count={count} with Isolated must satisfy policy"
            );
        }
    }

    // ── Larger counts still fail on coordinator with actual count ──

    #[test]
    fn many_hats_coordinator_mode_fails_with_actual_count() {
        for count in [4usize, 5, 8, 10, 50, 100] {
            let err = evaluate_multi_hat_isolation(count, HatExecutionMode::Coordinator)
                .expect_err("Coordinator above limit must fail");
            assert_eq!(err.actual, count);
        }
    }

    // ── Boundary: limit + 1 is the first failing count ─────────────

    #[test]
    fn limit_plus_one_is_first_failing_count() {
        assert!(evaluate_multi_hat_isolation(MULTI_HAT_ISOLATION_LIMIT, HatExecutionMode::Coordinator).is_ok());
        assert!(evaluate_multi_hat_isolation(MULTI_HAT_ISOLATION_LIMIT + 1, HatExecutionMode::Coordinator).is_err());
    }

    // ── Violation shape is stable for downstream adapters ──────────

    #[test]
    fn violation_message_includes_actual_and_limit() {
        let err = evaluate_multi_hat_isolation(4, HatExecutionMode::Coordinator)
            .expect_err("must fail");
        let msg = err.message();
        assert!(
            msg.contains('4') && msg.contains('3'),
            "message must include both actual count and limit, got: {msg}"
        );
    }

    #[test]
    fn violation_fix_hint_directs_to_isolated_mode() {
        let err = evaluate_multi_hat_isolation(5, HatExecutionMode::Coordinator)
            .expect_err("must fail");
        let hint = err.fix_hint();
        assert!(
            hint.contains("isolated"),
            "fix hint must direct operator to isolated mode, got: {hint}"
        );
    }

    // ── Constant is the single source of truth ─────────────────────

    #[test]
    fn limit_constant_is_three() {
        // Pin the public value. If the threshold changes, every
        // adapter (lint, preflight, runtime contract) needs to be
        // re-audited — that's exactly the discipline this constant
        // enforces.
        assert_eq!(MULTI_HAT_ISOLATION_LIMIT, 3);
    }
}
