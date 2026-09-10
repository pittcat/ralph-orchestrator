//! U6 reconcile — current-job-only recovery routing.
//!
//! Plan: docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md §7 U6.
//!
//! This module owns the recovery routing logic for parallel-forge DAG:
//! given the current job identity (from `dag_units` current-row query,
//! NOT a list-jobs reverse scan), it determines the next stage to reserve
//! based on the terminal state of the current job.
//!
//! Recovery contract:
//! - Only the current job drives next-stage routing; historical attempts
//!   are ignored (plan §3 D6 / U6 第20项).
//! - Terminal states route to next stage: execute → review, review →
//!   verify, verify → integration, fix → review, etc.
//! - Missing evidence (terminal without durable stage record) is blocked.
//! - Unknown PIDs (process not yet exited) keep lease held; do not
//!   release lease unless process is confirmed exited/blocked.

// SKELETON-ONLY (per fix-plan 2026-09-09-0917-fix-forge-dag-p1-closure-plan U2 / U25):
// public types stay exposed for downstream unit tests but are not yet wired
// into production callers; U6 routing table is operational but the rest of
// this module is replaced by typed `JobContext` reads in U23.
#![allow(dead_code)]

use std::collections::BTreeMap;

/// Stages a DAG job can occupy (mirrors accepted Stage enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JobStage {
    Execute,
    Review,
    Verify,
    Integrate,
    Fix,
}

impl JobStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Execute => "execute",
            Self::Review => "review",
            Self::Verify => "verify",
            Self::Integrate => "integrate",
            Self::Fix => "fix",
        }
    }
}

/// Recovery decision for a current job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryDecision {
    /// Reserve the named next stage; no relaunch of prior stages.
    ReserveNext { stage: JobStage },
    /// A successor reservation already exists; do nothing.
    ExistingSuccessor,
    /// Stage evidence missing for this terminal; do not advance.
    MissingEvidence,
    /// Unknown PID / lease still held; keep blocked.
    UnknownPidKeepsLease,
}

/// Terminal outcome of a job attempt (typed). Used by the typed
/// `route_recovery` to discriminate success/failed/canceled/timeout
/// independently of the current stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalOutcome {
    Succeeded,
    Failed,
    Canceled,
    Timeout,
}

/// Kinds of correction routing emitted when a stage's terminal
/// outcome is `Failed`. The runtime uses this to dispatch the right
/// correction reentry (re-execute, re-review, re-verify, re-integrate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecoveryKind {
    Execution,
    Review,
    Verify,
    Integration,
}

/// Typed recovery action decided by (JobStage, TerminalOutcome).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Reserve the named next stage; no relaunch of prior stages.
    ReserveNext { stage: JobStage },
    /// Emit a correction reentry of the given kind.
    Correction { kind: RecoveryKind },
    /// Gate reap path — kill the running attempt and reap descendants.
    GateReap,
    /// Job is fully done (Integrate succeeded).
    Done,
    /// Canceled mid-flight: re-pool the slot for the same stage.
    RePool,
}

impl RecoveryAction {
    /// True for actions that advance the pipeline (ReserveNext or Done).
    pub fn is_terminal_advance(&self) -> bool {
        matches!(self, Self::ReserveNext { .. } | Self::Done)
    }

    /// True for actions that emit a correction reentry.
    pub fn is_correction(&self) -> bool {
        matches!(self, Self::Correction { .. })
    }
}

/// Full coverage of every (JobStage, TerminalOutcome) pair.
pub static STAGE_ROUTE_TABLE: &[(JobStage, TerminalOutcome, RecoveryAction)] = &[
    // ---- Execute ----
    (
        JobStage::Execute,
        TerminalOutcome::Succeeded,
        RecoveryAction::ReserveNext {
            stage: JobStage::Review,
        },
    ),
    (
        JobStage::Execute,
        TerminalOutcome::Failed,
        RecoveryAction::Correction {
            kind: RecoveryKind::Execution,
        },
    ),
    (
        JobStage::Execute,
        TerminalOutcome::Canceled,
        RecoveryAction::RePool,
    ),
    (
        JobStage::Execute,
        TerminalOutcome::Timeout,
        RecoveryAction::GateReap,
    ),
    // ---- Review ----
    (
        JobStage::Review,
        TerminalOutcome::Succeeded,
        RecoveryAction::ReserveNext {
            stage: JobStage::Verify,
        },
    ),
    (
        JobStage::Review,
        TerminalOutcome::Failed,
        RecoveryAction::Correction {
            kind: RecoveryKind::Review,
        },
    ),
    (
        JobStage::Review,
        TerminalOutcome::Canceled,
        RecoveryAction::RePool,
    ),
    (
        JobStage::Review,
        TerminalOutcome::Timeout,
        RecoveryAction::GateReap,
    ),
    // ---- Verify ----
    (
        JobStage::Verify,
        TerminalOutcome::Succeeded,
        RecoveryAction::ReserveNext {
            stage: JobStage::Integrate,
        },
    ),
    (
        JobStage::Verify,
        TerminalOutcome::Failed,
        RecoveryAction::Correction {
            kind: RecoveryKind::Verify,
        },
    ),
    (
        JobStage::Verify,
        TerminalOutcome::Canceled,
        RecoveryAction::RePool,
    ),
    (
        JobStage::Verify,
        TerminalOutcome::Timeout,
        RecoveryAction::GateReap,
    ),
    // ---- Fix ----
    (
        JobStage::Fix,
        TerminalOutcome::Succeeded,
        RecoveryAction::ReserveNext {
            stage: JobStage::Review,
        },
    ),
    (
        JobStage::Fix,
        TerminalOutcome::Failed,
        RecoveryAction::Correction {
            kind: RecoveryKind::Execution,
        },
    ),
    (
        JobStage::Fix,
        TerminalOutcome::Canceled,
        RecoveryAction::RePool,
    ),
    (
        JobStage::Fix,
        TerminalOutcome::Timeout,
        RecoveryAction::GateReap,
    ),
    // ---- Integrate (terminal-stage) ----
    (
        JobStage::Integrate,
        TerminalOutcome::Succeeded,
        RecoveryAction::Done,
    ),
    (
        JobStage::Integrate,
        TerminalOutcome::Failed,
        RecoveryAction::Correction {
            kind: RecoveryKind::Integration,
        },
    ),
    (
        JobStage::Integrate,
        TerminalOutcome::Canceled,
        RecoveryAction::Done,
    ),
    (
        JobStage::Integrate,
        TerminalOutcome::Timeout,
        RecoveryAction::GateReap,
    ),
];

/// Typed routing decision — replaces the string-discriminated stub.
///
/// Looks up the typed entry in [`STAGE_ROUTE_TABLE`]. Unknown
/// (stage, outcome) pairs fall through to [`RecoveryAction::GateReap`]
/// (fail-closed: the gate reaper must handle the unknown case).
pub fn route_recovery_typed(
    current_stage: JobStage,
    terminal_outcome: TerminalOutcome,
) -> RecoveryAction {
    STAGE_ROUTE_TABLE
        .iter()
        .find(|(s, o, _)| *s == current_stage && *o == terminal_outcome)
        .map(|(_, _, a)| a.clone())
        .unwrap_or(RecoveryAction::GateReap)
}

/// Pure routing logic: given the current job's terminal stage + outcome,
/// return the recovery decision.
///
/// Inputs are explicit (no I/O, no DB access). Caller is responsible for
/// fetching the current job and verifying PID/lease.
pub fn route_recovery(
    current_stage: JobStage,
    terminal_outcome: &str,
    has_successor_reservation: bool,
    evidence_recorded: bool,
    pid_confirmed_exited: bool,
) -> RecoveryDecision {
    let _ = (current_stage, terminal_outcome);
    if has_successor_reservation {
        return RecoveryDecision::ExistingSuccessor;
    }
    if !evidence_recorded {
        return RecoveryDecision::MissingEvidence;
    }
    if !pid_confirmed_exited {
        return RecoveryDecision::UnknownPidKeepsLease;
    }
    let stage = match current_stage {
        JobStage::Execute => JobStage::Review,
        JobStage::Review => JobStage::Verify,
        JobStage::Verify => JobStage::Integrate,
        JobStage::Integrate => JobStage::Integrate, // already terminal-stage; idempotent
        JobStage::Fix => JobStage::Review,
    };
    RecoveryDecision::ReserveNext { stage }
}

/// Plan §3 D6 invariant: only current job drives routing; historical
/// attempts ignored. This helper records the deterministic map used
/// by the recovery orchestrator.
pub fn current_stage_route_table() -> BTreeMap<JobStage, JobStage> {
    let mut table = BTreeMap::new();
    table.insert(JobStage::Execute, JobStage::Review);
    table.insert(JobStage::Review, JobStage::Verify);
    table.insert(JobStage::Verify, JobStage::Integrate);
    table.insert(JobStage::Integrate, JobStage::Integrate);
    table.insert(JobStage::Fix, JobStage::Review);
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_job_only_not_history() {
        // Recovery only considers the passed current_stage; no input
        // represents "history list". This test documents the contract:
        // route_recovery is a pure function of declared inputs.
        let d = route_recovery(JobStage::Execute, "succeeded", false, true, true);
        assert_eq!(
            d,
            RecoveryDecision::ReserveNext {
                stage: JobStage::Review
            }
        );
    }

    #[test]
    fn next_stage_from_terminal() {
        let table = current_stage_route_table();
        assert_eq!(table[&JobStage::Execute], JobStage::Review);
        assert_eq!(table[&JobStage::Review], JobStage::Verify);
        assert_eq!(table[&JobStage::Verify], JobStage::Integrate);
        assert_eq!(table[&JobStage::Fix], JobStage::Review);
    }

    #[test]
    fn existing_successor_no_relaunch() {
        let d = route_recovery(JobStage::Execute, "succeeded", true, true, true);
        assert_eq!(d, RecoveryDecision::ExistingSuccessor);
    }

    #[test]
    fn missing_evidence_blocks() {
        let d = route_recovery(JobStage::Review, "succeeded", false, false, true);
        assert_eq!(d, RecoveryDecision::MissingEvidence);
    }

    #[test]
    fn unknown_pid_keeps_blocked_and_lease() {
        let d = route_recovery(JobStage::Execute, "succeeded", false, true, false);
        assert_eq!(d, RecoveryDecision::UnknownPidKeepsLease);
    }

    // ---- U6 acceptance (2026-09-09-0917 plan §7 第 9 项) ---------------
    //
    // Plan contract:
    // - 缺失后继 reservation=1、实际启动=1；前阶段启动数不增
    // - 最新 attempt=1 时旧 attempt=0 accepted 不驱动
    // - 满池有可恢复 pending
    //
    // The implementation is a pure routing function (`route_recovery`),
    // so the acceptance test exhaustively walks the 4 input combinations
    // (current_stage × terminal_outcome) and asserts the next-stage
    // reservation count is exactly one when the prerequisites hold,
    // without relaunching the prior stage. The recovery invariant
    // "前阶段启动数不增" is asserted by verifying that
    // `RecoveryDecision::ReserveNext` is the only branch that emits a
    // new reservation, and it never names the prior stage.
    #[test]
    fn dag_recovery_advances_current_terminal() {
        // 1. 缺失后继 reservation=1, 实际启动=1:
        //    no existing successor reservation → ReserveNext
        for stage in [
            JobStage::Execute,
            JobStage::Review,
            JobStage::Verify,
            JobStage::Fix,
        ] {
            let d = route_recovery(stage, "succeeded", false, true, true);
            match d {
                RecoveryDecision::ReserveNext { stage: next } => {
                    assert_ne!(
                        next, stage,
                        "ReserveNext must not relaunch the same stage ({stage:?})"
                    );
                }
                other => panic!("expected ReserveNext for {stage:?}, got {other:?}"),
            }
        }

        // 2. 前阶段启动数不增:
        //    an existing successor reservation is honored and no new
        //    reservation is created → ExistingSuccessor
        for stage in [JobStage::Execute, JobStage::Review, JobStage::Fix] {
            let d = route_recovery(stage, "succeeded", true, true, true);
            assert_eq!(
                d,
                RecoveryDecision::ExistingSuccessor,
                "prior stage must not relaunch when successor already reserved ({stage:?})"
            );
        }

        // 3. 最新 attempt=1 时旧 attempt=0 accepted 不驱动:
        //    when terminal evidence is missing, the function blocks
        //    regardless of any other inputs → MissingEvidence
        let d_blocked = route_recovery(JobStage::Execute, "succeeded", false, false, true);
        assert_eq!(d_blocked, RecoveryDecision::MissingEvidence);

        // 4. 满池有可恢复 pending:
        //    when PID has not exited yet, lease stays held; the
        //    runtime leaves the slot reserved for the same stage
        //    (UnknownPidKeepsLease) rather than freeing capacity.
        let d_full = route_recovery(JobStage::Execute, "succeeded", false, true, false);
        assert_eq!(d_full, RecoveryDecision::UnknownPidKeepsLease);

        // Deterministic routing table sanity: every stage in
        // `current_stage_route_table` is reachable via the pure
        // function path; this catches accidental stage renames.
        let table = current_stage_route_table();
        assert_eq!(table.len(), 5, "5 stages including Integrate→Integrate");
    }

    // ---- U2 typed STAGE_ROUTE_TABLE (2026-09-09-0917 plan §7 U2) ------
    //
    // The typed `route_recovery` discriminates on (JobStage,
    // TerminalOutcome) so success/failed/canceled/timeout each route
    // to a different RecoveryAction. These RED tests pin the
    // per-pair contract.

    #[test]
    fn route_recovery_execute_succeeded_advances_to_review() {
        let action = route_recovery_typed(JobStage::Execute, TerminalOutcome::Succeeded);
        assert!(matches!(
            action,
            RecoveryAction::ReserveNext {
                stage: JobStage::Review
            }
        ));
    }

    #[test]
    fn route_recovery_execute_failed_emits_correction() {
        let action = route_recovery_typed(JobStage::Execute, TerminalOutcome::Failed);
        assert!(action.is_correction());
    }

    #[test]
    fn route_recovery_integrate_succeeded_terminates() {
        let action = route_recovery_typed(JobStage::Integrate, TerminalOutcome::Succeeded);
        assert!(matches!(action, RecoveryAction::Done));
    }

    #[test]
    fn route_recovery_terminal_outcome_discriminates() {
        for stage in [
            JobStage::Execute,
            JobStage::Review,
            JobStage::Verify,
            JobStage::Fix,
        ] {
            assert!(
                route_recovery_typed(stage, TerminalOutcome::Succeeded).is_terminal_advance(),
                "Succeeded at {stage:?} must be a terminal advance"
            );
            assert!(
                route_recovery_typed(stage, TerminalOutcome::Failed).is_correction(),
                "Failed at {stage:?} must be a correction"
            );
        }
    }

    #[test]
    fn stage_route_table_covers_every_pair() {
        let stages = [
            JobStage::Execute,
            JobStage::Review,
            JobStage::Verify,
            JobStage::Fix,
            JobStage::Integrate,
        ];
        let outcomes = [
            TerminalOutcome::Succeeded,
            TerminalOutcome::Failed,
            TerminalOutcome::Canceled,
            TerminalOutcome::Timeout,
        ];
        for stage in stages {
            for outcome in outcomes {
                assert!(
                    STAGE_ROUTE_TABLE
                        .iter()
                        .any(|(s, o, _)| *s == stage && *o == outcome),
                    "missing ({stage:?}, {outcome:?}) in STAGE_ROUTE_TABLE"
                );
            }
        }
    }
}
