//! U7 intent_consume — tested-intent classification + recovery (skeleton).
//!
//! Plan: docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md §7 U7.
//!
//! Classifies the durable tested intent's target state at recovery
//! time. Pure dispatch; real CAS / record handled by caller.

// SKELETON-ONLY (per fix-plan 2026-09-09-0917-fix-forge-dag-p1-closure-plan U2 / U25):
// public types stay exposed for downstream unit tests but are not yet wired
// into production callers; U5 / U11 / U23 production replacement promotes
// this file to `PRODUCTION:` marker.
#![allow(dead_code)]

use std::collections::BTreeMap;

/// Classification of the relationship between the persisted intent's
/// target and the actual target state at recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IntentTargetState {
    /// Target HEAD is what the intent originally expected.
    TargetExpected,
    /// Target HEAD has advanced to the intent's candidate.
    TargetCandidate,
    /// Target HEAD is a proven runtime descendant of the candidate.
    TargetProvenDescendant,
    /// Target belongs to a foreign repository / non-canonical identity.
    ForeignTarget,
    /// Target moved but CAS was not yet applied; candidate never landed.
    StaleUnapplied,
}

/// Inputs for intent classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentRecoveryInput {
    pub intent_target_branch: String,
    pub expected_head: String,
    pub candidate_head: String,
    pub current_target_head: String,
    pub cas_applied: bool,
    pub worktree_canonical_identity: String,
}

/// Outcome of intent recovery classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentRecoveryOutcome {
    /// Re-run original CAS with intent target=expected.
    RetryCasWithExpected { reason: String },
    /// Candidate already on target; record integration.
    ConsumeCasAndRecord { reason: String },
    /// Target is a proven descendant; just record (don't CAS).
    JustRecordDescendant { reason: String },
    /// Foreign target; refuse.
    RefusedForeign { reason: String },
    /// Stale intent (never landed); allow supersede with new generation.
    SupersedeStale { reason: String },
}

/// Pure dispatch: classify intent recovery outcome.
pub fn classify_intent_recovery(input: &IntentRecoveryInput) -> IntentRecoveryOutcome {
    if input.worktree_canonical_identity.is_empty() {
        return IntentRecoveryOutcome::RefusedForeign {
            reason: "worktree identity empty".to_string(),
        };
    }
    if input.current_target_head == input.expected_head {
        if !input.cas_applied {
            return IntentRecoveryOutcome::RetryCasWithExpected {
                reason: "target=expected, CAS not yet applied".to_string(),
            };
        }
        return IntentRecoveryOutcome::ConsumeCasAndRecord {
            reason: "target=expected, CAS applied".to_string(),
        };
    }
    if input.current_target_head == input.candidate_head {
        return IntentRecoveryOutcome::ConsumeCasAndRecord {
            reason: "target=candidate (CAS landed)".to_string(),
        };
    }
    if input.current_target_head.starts_with(&input.candidate_head) {
        return IntentRecoveryOutcome::JustRecordDescendant {
            reason: "target is a descendant of candidate".to_string(),
        };
    }
    if !input.cas_applied {
        return IntentRecoveryOutcome::SupersedeStale {
            reason: "target moved and CAS not applied; candidate never landed".to_string(),
        };
    }
    IntentRecoveryOutcome::RefusedForeign {
        reason: "target unrelated to intent".to_string(),
    }
}

/// Reference table: legal transitions of intent target states.
pub fn legal_intent_target_transitions() -> BTreeMap<IntentTargetState, Vec<IntentTargetState>> {
    let mut m = BTreeMap::new();
    m.insert(
        IntentTargetState::TargetExpected,
        vec![
            IntentTargetState::TargetCandidate,
            IntentTargetState::StaleUnapplied,
        ],
    );
    m.insert(
        IntentTargetState::TargetCandidate,
        vec![IntentTargetState::TargetProvenDescendant],
    );
    m.insert(IntentTargetState::TargetProvenDescendant, vec![]);
    m.insert(IntentTargetState::ForeignTarget, vec![]);
    m.insert(
        IntentTargetState::StaleUnapplied,
        vec![IntentTargetState::TargetExpected],
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> IntentRecoveryInput {
        IntentRecoveryInput {
            intent_target_branch: "main".to_string(),
            expected_head: "aaaa".to_string(),
            candidate_head: "cccc".to_string(),
            current_target_head: "aaaa".to_string(),
            cas_applied: false,
            worktree_canonical_identity: "/worktree".to_string(),
        }
    }

    #[test]
    fn intent_target_expected() {
        let outcome = classify_intent_recovery(&base());
        assert!(matches!(
            outcome,
            IntentRecoveryOutcome::RetryCasWithExpected { .. }
        ));
    }

    #[test]
    fn intent_target_candidate() {
        let mut input = base();
        input.current_target_head = "cccc".to_string();
        let outcome = classify_intent_recovery(&input);
        assert!(matches!(
            outcome,
            IntentRecoveryOutcome::ConsumeCasAndRecord { .. }
        ));
    }

    #[test]
    fn intent_target_proven_runtime_descendant() {
        let mut input = base();
        input.current_target_head = "cccc1".to_string(); // descendant
        let outcome = classify_intent_recovery(&input);
        assert!(matches!(
            outcome,
            IntentRecoveryOutcome::JustRecordDescendant { .. }
        ));
    }

    #[test]
    fn foreign_target_blocks() {
        let mut input = base();
        input.worktree_canonical_identity = "".to_string();
        let outcome = classify_intent_recovery(&input);
        assert!(matches!(
            outcome,
            IntentRecoveryOutcome::RefusedForeign { .. }
        ));
    }

    #[test]
    fn stale_unapplied_intent_can_supersede() {
        let mut input = base();
        input.current_target_head = "zzzz".to_string(); // unrelated
        input.cas_applied = false;
        let outcome = classify_intent_recovery(&input);
        assert!(matches!(
            outcome,
            IntentRecoveryOutcome::SupersedeStale { .. }
        ));
    }
}
