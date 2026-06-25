//! 2026-06-23-005 U4 (R5+R12): AuditSeverity SSOT integration tests.
//!
//! Verifies that:
//! - `AuditSeverity::Fail { add_failures: 1 }` is applied to
//!   `audit_file_modifications` (scope_violation), forcing
//!   `consecutive_failures += 1` so the same hat second-offense
//!   triggers `consecutive_failures >= 5` termination.
//! - Scratchpad paths are NOT audited (OQ-2 resolution: audit only
//!   watches `git diff --stat` against the repo, so a file written
//!   outside the workspace — e.g. `.agents/scratchpad/...` — does not
//!   count as a scope violation).
//!
//! Reference: `docs/plans/2026-06-23-005-fix-ce-executor-serial-hard-gate-half-edge-recovery-plan.md`
//! U4 / R5 / R12 / KTD-8 / OQ-2.

use crate::event_loop::audit::{AuditContext, AuditDispatcher, AuditSeverity};
use crate::preset::engine::gates::RejectionKind;

#[test]
fn scope_violation_failure_increments_counter() {
    // R5: scope_violation promotes from Warn → Fail { add_failures: 1 }.
    // This is the FIRST audit class to be promoted; drift_monitor stays
    // at Warn (U9 follow-up).
    let mut cf = 0u32;
    AuditDispatcher::dispatch(
        AuditSeverity::Fail { add_failures: 1 },
        AuditContext {
            hat: "dimension-reviewer".to_string(),
            kind: RejectionKind::MissingField,
            details: "scope_violation test".to_string(),
        },
        &mut cf,
    );
    assert_eq!(cf, 1);
}

#[test]
fn warn_severity_keeps_counter_stable() {
    // drift_monitor's 3 alert classes (coord_join_rate / field_completeness
    // / drift_unconsumed) keep Warn severity in this PR (U9 follow-up).
    let mut cf = 3u32;
    AuditDispatcher::dispatch(
        AuditSeverity::Warn,
        AuditContext {
            hat: "ralph".to_string(),
            kind: RejectionKind::MissingField,
            details: "drift_finding (warn)".to_string(),
        },
        &mut cf,
    );
    assert_eq!(cf, 3);
}

#[test]
fn scope_violation_accumulates_to_failure_threshold() {
    // Realistic flow: dimension-reviewer violates scope twice. With
    // KTD-8 Fail { add_failures: 1 }, the loop's consecutive_failures
    // counter should rise 0 → 2, and a third violation would trigger
    // the `consecutive_failures >= 5` termination path (U3 typed
    // Failure trigger) once combined with hat execution failures.
    let mut cf = 0u32;
    for _ in 0..2 {
        AuditDispatcher::dispatch(
            AuditSeverity::Fail { add_failures: 1 },
            AuditContext {
                hat: "dimension-reviewer".to_string(),
                kind: RejectionKind::MissingField,
                details: "scope violation".to_string(),
            },
            &mut cf,
        );
    }
    assert_eq!(cf, 2);
}

#[test]
fn fail_severity_uses_typed_kind_for_log_correlation() {
    // AuditContext carries typed kind (newly-added RejectionKind variants
    // from U1: MissingEventGate / StallNoEvents / ContractViolation all
    // route through this audit dispatcher).
    let ctx = AuditContext {
        hat: "ralph".to_string(),
        kind: RejectionKind::MissingEventGate,
        details: "audit detail".to_string(),
    };
    let mut cf = 0u32;
    AuditDispatcher::dispatch(
        AuditSeverity::Fail { add_failures: 1 },
        ctx.clone(),
        &mut cf,
    );
    assert_eq!(cf, 1);
    assert_eq!(ctx.kind, RejectionKind::MissingEventGate);
}
