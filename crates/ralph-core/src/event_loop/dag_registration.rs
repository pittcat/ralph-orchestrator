//! U9 dag_registration — receipt-before-projection (skeleton).
//!
//! Plan: docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md §7 U9.
//!
//! Writes a candidate receipt for each DAG plan-ready event BEFORE the
//! StateProjector.apply runs. The candidate receipt alone is not
//! authoritative; real acceptance is recorded after the
//! pending_publish/AcceptedTransition completes. v21 schema is the
//! durable backing for both receipt and accepted evidence.

use std::collections::BTreeMap;

/// State of a registration receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReceiptState {
    Candidate,
    Accepted,
    Rejected,
}

impl ReceiptState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }
}

/// Input describing the receipt candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptCandidate {
    pub plan_key: String,
    pub loop_id: String,
    pub source_hat: String,
    pub contract_revision: String,
    pub artifact_path: String,
    pub artifact_digest: String,
}

/// Outcome of attempting to register a plan-ready candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// Candidate receipt written successfully.
    CandidateWritten,
    /// Existing candidate with same identity; idempotent no-op.
    CandidateIdempotent,
    /// Same loop_id, different artifact digest; conflict.
    DigestConflict { reason: String },
    /// I/O failed before projection; do NOT project.
    IoFailed { reason: String },
    /// Replay path: pending receipt needs to revalidate artifact.
    ReplayRevalidate { reason: String },
    /// Real acceptance marker written after pending_publish success.
    AcceptedMarkerWritten,
}

/// Pure dispatch: classify the registration outcome from explicit inputs.
pub fn classify_registration(
    candidate: &ReceiptCandidate,
    existing_artifact_digest: Option<&str>,
    existing_receipt_state: Option<ReceiptState>,
    io_succeeded: bool,
    real_acceptance_observed: bool,
) -> RegistrationOutcome {
    if !io_succeeded {
        return RegistrationOutcome::IoFailed {
            reason: "receipt I/O failed before projection".to_string(),
        };
    }
    if real_acceptance_observed {
        return RegistrationOutcome::AcceptedMarkerWritten;
    }
    if existing_receipt_state == Some(ReceiptState::Candidate) {
        match existing_artifact_digest {
            Some(prev) if prev != candidate.artifact_digest => {
                RegistrationOutcome::DigestConflict {
                    reason: format!(
                        "existing candidate digest {prev} != new {}",
                        candidate.artifact_digest
                    ),
                }
            }
            _ => RegistrationOutcome::CandidateIdempotent,
        }
    } else {
        RegistrationOutcome::CandidateWritten
    }
}

/// Reference table: allowed receipt state transitions.
pub fn legal_receipt_transitions() -> BTreeMap<ReceiptState, Vec<ReceiptState>> {
    let mut m = BTreeMap::new();
    m.insert(
        ReceiptState::Candidate,
        vec![ReceiptState::Accepted, ReceiptState::Rejected],
    );
    m.insert(ReceiptState::Accepted, vec![]);
    m.insert(ReceiptState::Rejected, vec![]);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ReceiptCandidate {
        ReceiptCandidate {
            plan_key: "plan-A".to_string(),
            loop_id: "loop-1".to_string(),
            source_hat: "plan-reviewer".to_string(),
            contract_revision: "v1".to_string(),
            artifact_path: "/path/to/artifact".to_string(),
            artifact_digest: "deadbeef".to_string(),
        }
    }

    #[test]
    fn candidate_is_not_accepted() {
        let outcome = classify_registration(&base(), None, None, true, false);
        assert_eq!(outcome, RegistrationOutcome::CandidateWritten);
    }

    #[test]
    fn receipt_digest_conflict() {
        let outcome = classify_registration(
            &base(),
            Some("cafef00d"),
            Some(ReceiptState::Candidate),
            true,
            false,
        );
        assert!(matches!(
            outcome,
            RegistrationOutcome::DigestConflict { .. }
        ));
    }

    #[test]
    fn receipt_before_projection_failure() {
        let outcome = classify_registration(&base(), None, None, false, false);
        assert!(matches!(outcome, RegistrationOutcome::IoFailed { .. }));
    }

    #[test]
    fn replay_pending_revalidates_artifact() {
        // Skeleton contract: same identity → idempotent, but the caller
        // should revalidate the artifact regardless. We classify as
        // CandidateIdempotent; caller decides revalidation.
        let outcome = classify_registration(
            &base(),
            Some("deadbeef"),
            Some(ReceiptState::Candidate),
            true,
            false,
        );
        assert_eq!(outcome, RegistrationOutcome::CandidateIdempotent);
    }

    #[test]
    fn accepted_marker_after_real_acceptance() {
        let outcome = classify_registration(&base(), None, None, true, true);
        assert_eq!(outcome, RegistrationOutcome::AcceptedMarkerWritten);
    }
}
