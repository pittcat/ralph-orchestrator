//! U11 corrections — execute-failure correction authorization (skeleton).
//!
//! Plan: docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md §7 U11.
//!
//! Owns the durable ledger for execute-failure correction requests.
//! v22 schema is the backing. Budget = 3 fixer attempts per Unit.

// `result_large_err` is allowed at file scope (more granular than
// crate level) per U2 / F17: keep DagStoreError variants human-readable
// for fail-closed evidence while suppressing function-level
// `result_large_err` errors on every method returning
// `Result<_, DagStoreError>`.
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;

/// Per-Unit budget for fixer attempts.
pub const FIXER_BUDGET: i64 = 3;

/// State of a correction request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CorrectionState {
    Pending,
    Reserved,
    Blocked,
}

impl CorrectionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Reserved => "reserved",
            Self::Blocked => "blocked",
        }
    }
}

/// Input describing a correction request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectionRequest {
    pub unit_key: String,
    pub failure_fingerprint: String,
    pub failed_job_id: String,
    pub failed_job_token: String,
    pub failed_attempt: i64,
    pub correction_digest: String,
    pub feedback_path: String,
    pub feedback_hash: String,
}

/// Outcome of attempting to reserve a fixer attempt for a correction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReservationOutcome {
    /// Reserved a new fixer attempt; budget decremented.
    Reserved { attempt: i64 },
    /// Same correction identity replayed; no budget change.
    Replayed,
    /// Pool is full; correction stays pending, no budget change.
    PoolFull { reason: String },
    /// Budget exhausted for this Unit; refused.
    BudgetExhausted,
    /// Unknown unit / mismatched token / unknown source.
    Refused { reason: String },
    /// Worktree is dirty; refuse correction without reset.
    DirtyWorktree,
}

/// Pure dispatch: classify reservation outcome from explicit inputs.
///
/// `current_reserved_attempts` = number of fixer attempts already
/// reserved for this Unit (read from v22 ledger by caller).
pub fn classify_reservation(
    req: &CorrectionRequest,
    current_reserved_attempts: i64,
    pool_capacity_available: bool,
    worktree_clean: bool,
    known_unit: bool,
    token_matches: bool,
    existing_request_digest: Option<&str>,
) -> ReservationOutcome {
    if !known_unit {
        return ReservationOutcome::Refused {
            reason: "unknown unit".to_string(),
        };
    }
    if !token_matches {
        return ReservationOutcome::Refused {
            reason: "failed token mismatch".to_string(),
        };
    }
    if !worktree_clean {
        return ReservationOutcome::DirtyWorktree;
    }
    if let Some(prev) = existing_request_digest {
        if prev == req.correction_digest {
            return ReservationOutcome::Replayed;
        }
        // Different correction for the same failure fingerprint: dedup
        // is per-fingerprint; new digest means a different correction —
        // but the rule says dedup must collapse to one. Treat as
        // Replayed too (caller enforces one-attempt-per-fingerprint).
        return ReservationOutcome::Replayed;
    }
    if current_reserved_attempts >= FIXER_BUDGET {
        return ReservationOutcome::BudgetExhausted;
    }
    if !pool_capacity_available {
        return ReservationOutcome::PoolFull {
            reason: format!("pool full; budget={FIXER_BUDGET} not decremented"),
        };
    }
    ReservationOutcome::Reserved {
        attempt: current_reserved_attempts + 1,
    }
}

/// Reference table: allowed correction state transitions.
pub fn legal_correction_transitions() -> BTreeMap<CorrectionState, Vec<CorrectionState>> {
    let mut m = BTreeMap::new();
    m.insert(
        CorrectionState::Pending,
        vec![CorrectionState::Reserved, CorrectionState::Blocked],
    );
    m.insert(
        CorrectionState::Reserved,
        vec![CorrectionState::Pending, CorrectionState::Blocked],
    );
    m.insert(CorrectionState::Blocked, vec![]);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> CorrectionRequest {
        CorrectionRequest {
            unit_key: "U1".to_string(),
            failure_fingerprint: "fp-1".to_string(),
            failed_job_id: "job-1".to_string(),
            failed_job_token: "tok-1".to_string(),
            failed_attempt: 1,
            correction_digest: "corr-1".to_string(),
            feedback_path: "/tmp/feedback".to_string(),
            feedback_hash: "h-1".to_string(),
        }
    }

    #[test]
    fn execute_failed_authorizes_only_current_correction() {
        let o = classify_reservation(&base(), 0, true, true, true, true, None);
        assert_eq!(o, ReservationOutcome::Reserved { attempt: 1 });
    }

    #[test]
    fn duplicate_request_dedups() {
        let o = classify_reservation(&base(), 0, true, true, true, true, Some("corr-1"));
        assert_eq!(o, ReservationOutcome::Replayed);
    }

    #[test]
    fn pool_wait_does_not_spend_budget() {
        let o = classify_reservation(&base(), 0, false, true, true, true, None);
        assert!(matches!(o, ReservationOutcome::PoolFull { .. }));
    }

    #[test]
    fn attempt_three_is_last() {
        let o = classify_reservation(&base(), 3, true, true, true, true, None);
        assert_eq!(o, ReservationOutcome::BudgetExhausted);
    }

    #[test]
    fn dirty_failed_worktree_blocks_without_reset() {
        let o = classify_reservation(&base(), 0, true, false, true, true, None);
        assert_eq!(o, ReservationOutcome::DirtyWorktree);
    }
}
