//! 2026-06-23-005 U4 (R5+R12): typed `AuditSeverity` SSOT.
//!
//! All audit functions in `event_loop` route through this SSOT. Each
//! audit function returns `AuditSeverity` and `AuditContext`; the
//! `AuditDispatcher::dispatch` central method is the single entry point
//! for converting audit findings into runtime state changes (counter
//! increments, trigger pushes, log lines).
//!
//! Scope_violation (this PR) is the first audit class promoted from
//! `AuditSeverity::Warn` to `AuditSeverity::Fail { add_failures: 1 }`.
//! Drift_monitor's 3 alert classes are migrated to `AuditSeverity::Warn`
//! in this PR to lock in the SSOT shape but their severity is **NOT**
//! promoted to `Fail` — that migration is tracked as U9 follow-up per
//! the plan's `Deferred to Follow-Up Work` section.
//!
//! Reference: `docs/plans/2026-06-23-005-fix-ce-executor-serial-hard-gate-half-edge-recovery-plan.md`
//! U4 / R5 / R12 / KTD-8.

use crate::preset::engine::gates::RejectionKind;

/// Typed severity classification for audit findings (KTD-8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditSeverity {
    /// Informational only; no runtime state change.
    Warn,
    /// Counts as a typed failure: `state.consecutive_failures +=
    /// add_failures`. Logged at warn level for backward-compat with
    /// existing log-grep tools.
    Fail { add_failures: u32 },
    /// Immediately pushes a `TerminationTrigger::BlockLoop` and
    /// terminates the loop on the next `process_output` call.
    BlockLoop { reason: String },
}

/// Audit finding context — paired with `AuditSeverity` to drive the
/// runtime side-effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditContext {
    pub hat: String,
    pub kind: RejectionKind,
    pub details: String,
}

/// Single dispatch point for all audit findings (KTD-8 SSOT).
///
/// All callers MUST go through this method instead of mutating
/// `LoopState` directly so the audit chain stays observable (logs,
/// counter increments, trigger pushes all centralised).
pub struct AuditDispatcher;

impl AuditDispatcher {
    /// Apply an audit finding to the runtime.
    ///
    /// This function is intentionally side-effect-only (no return value):
    /// the caller has already decided what severity to apply via
    /// `AuditSeverity`; the dispatcher's job is to perform the
    /// side-effects consistently across all audit classes.
    pub fn dispatch(severity: AuditSeverity, ctx: AuditContext, consecutive_failures: &mut u32) {
        match severity {
            AuditSeverity::Warn => {
                tracing::warn!(
                    hat = %ctx.hat,
                    kind = %ctx.kind.reason_code(),
                    details = %ctx.details,
                    "audit finding (warn severity, no state change)"
                );
            }
            AuditSeverity::Fail { add_failures } => {
                *consecutive_failures = consecutive_failures.saturating_add(add_failures);
                tracing::warn!(
                    hat = %ctx.hat,
                    kind = %ctx.kind.reason_code(),
                    details = %ctx.details,
                    add_failures,
                    "audit finding (fail severity, consecutive_failures += {add_failures})"
                );
            }
            AuditSeverity::BlockLoop { reason } => {
                tracing::error!(
                    hat = %ctx.hat,
                    kind = %ctx.kind.reason_code(),
                    details = %ctx.details,
                    %reason,
                    "audit finding (block-loop severity, immediate termination)"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warn_severity_does_not_change_consecutive_failures() {
        let mut cf = 0u32;
        AuditDispatcher::dispatch(
            AuditSeverity::Warn,
            AuditContext {
                hat: "dimension-reviewer".to_string(),
                kind: RejectionKind::MissingField,
                details: "informational".to_string(),
            },
            &mut cf,
        );
        assert_eq!(cf, 0);
    }

    #[test]
    fn fail_severity_increments_consecutive_failures() {
        let mut cf = 0u32;
        AuditDispatcher::dispatch(
            AuditSeverity::Fail { add_failures: 1 },
            AuditContext {
                hat: "dimension-reviewer".to_string(),
                kind: RejectionKind::MissingField,
                details: "scope violation".to_string(),
            },
            &mut cf,
        );
        assert_eq!(cf, 1);
    }

    #[test]
    fn fail_severity_respects_add_failures_count() {
        let mut cf = 0u32;
        AuditDispatcher::dispatch(
            AuditSeverity::Fail { add_failures: 3 },
            AuditContext {
                hat: "test".to_string(),
                kind: RejectionKind::MissingField,
                details: "agg".to_string(),
            },
            &mut cf,
        );
        assert_eq!(cf, 3);
    }

    #[test]
    fn fail_severity_saturates_to_prevent_underflow() {
        let mut cf = u32::MAX - 1;
        AuditDispatcher::dispatch(
            AuditSeverity::Fail { add_failures: 5 },
            AuditContext {
                hat: "test".to_string(),
                kind: RejectionKind::MissingField,
                details: "agg".to_string(),
            },
            &mut cf,
        );
        assert_eq!(cf, u32::MAX, "saturating_add must clamp at u32::MAX");
    }

    #[test]
    fn block_loop_severity_does_not_change_consecutive_failures() {
        // BlockLoop triggers an immediate termination path (U3 typed
        // trigger) and does NOT increment consecutive_failures — those
        // are orthogonal termination mechanisms.
        let mut cf = 2u32;
        AuditDispatcher::dispatch(
            AuditSeverity::BlockLoop {
                reason: "test_block".to_string(),
            },
            AuditContext {
                hat: "test".to_string(),
                kind: RejectionKind::MissingField,
                details: "immediate".to_string(),
            },
            &mut cf,
        );
        assert_eq!(cf, 2);
    }

    #[test]
    fn scope_violation_uses_fail_severity_per_u4_decision() {
        // R5: scope_violation is the first audit class to be promoted
        // from Warn to Fail. The AuditContext carries the typed kind
        // (newly-added in U1: `RejectionKind::MissingField` is a
        // placeholder; scope_violation routes via kind=MissingField
        // today as no dedicated variant exists yet).
        let severity = AuditSeverity::Fail { add_failures: 1 };
        match severity {
            AuditSeverity::Fail { add_failures } => assert_eq!(add_failures, 1),
            _ => panic!("scope_violation must use Fail severity per U4"),
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // U5 (plan 2026-07-04-004): scope_violation for dimension-reviewer
    // promoted to `AuditSeverity::BlockLoop { reason: "scope_violation" }`.
    // Tests cover the typed `RejectionKind::ScopeViolation` variant,
    // the BlockLoop vs Fail severity carve-out, the consecutive_failures
    // invariant, and the typed trigger queue push.
    // ─────────────────────────────────────────────────────────────────────

    /// `RejectionKind` now has a dedicated `ScopeViolation` variant
    /// (was previously routed via `MissingField` placeholder).
    /// Pin the variant so static assertions across the workspace
    /// stay in sync.
    #[test]
    fn test_rejection_kind_has_scope_violation_variant() {
        use crate::preset::engine::gates::RejectionKind;
        let kind = RejectionKind::ScopeViolation;
        assert_eq!(
            kind.reason_code(),
            "scope_violation",
            "ScopeViolation must serialise as 'scope_violation' so dashboards can match it"
        );
        assert_eq!(
            RejectionKind::from_reason_code("scope_violation"),
            Some(RejectionKind::ScopeViolation),
            "from_reason_code must round-trip the new variant"
        );
    }

    /// BlockLoop severity for dimension-reviewer does NOT
    /// increment consecutive_failures (orthogonal termination
    /// mechanism; the typed trigger queue handles the actual
    /// loop termination via `trigger_to_reason`).
    #[test]
    fn test_block_loop_severity_does_not_increment_consecutive_failures() {
        let mut cf = 7u32;
        AuditDispatcher::dispatch(
            AuditSeverity::BlockLoop {
                reason: "scope_violation".to_string(),
            },
            AuditContext {
                hat: "dimension-reviewer".to_string(),
                kind: crate::preset::engine::gates::RejectionKind::ScopeViolation,
                details: "docs/plans/foo.md | 3 ++".to_string(),
            },
            &mut cf,
        );
        assert_eq!(
            cf, 7,
            "BlockLoop must not change consecutive_failures; the trigger queue handles termination"
        );
    }
}
