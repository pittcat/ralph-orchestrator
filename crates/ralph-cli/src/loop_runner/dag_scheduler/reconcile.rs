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
        assert_eq!(d, RecoveryDecision::ReserveNext { stage: JobStage::Review });
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
}