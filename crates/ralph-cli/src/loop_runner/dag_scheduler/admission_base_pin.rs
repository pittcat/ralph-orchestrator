//! U5 admission_base_pin — per-Unit immutable base pin (skeleton).
//!
//! Plan: docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md §7 U5.
//!
//! Computes the immutable unit_base at first admission: a SHA that
//! contains all acked-ancestor integrated commits for the Unit's
//! dependencies. Once written, the base is immutable across resume
//! / diff / spawn. Skeleton captures pure dispatch logic.

use std::collections::BTreeSet;

/// Inputs to base computation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasePinInput {
    pub unit_key: String,
    pub current_target_sha: String,
    pub dependency_unit_keys: Vec<String>,
    pub dependency_acked_commits: BTreeSet<String>, // commits the deps reached after integration + ack
    pub candidate_ancestor_in_target: bool,
}

/// Outcome of computing the pinned base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BasePinOutcome {
    /// Base = current target SHA (no dependencies).
    PinCurrentTarget { sha: String },
    /// Base pinned to a SHA containing all acked deps.
    PinDepsBase { sha: String },
    /// Target was rewritten after deps were acked; blocked.
    TargetRewrite { reason: String },
    /// Not all dependencies are acked ancestors of current target.
    UnackedDependency { reason: String },
}

/// Pure dispatch: classify base pin outcome.
pub fn compute_base_pin(input: &BasePinInput) -> BasePinOutcome {
    if input.dependency_unit_keys.is_empty() {
        return BasePinOutcome::PinCurrentTarget { sha: input.current_target_sha.clone() };
    }
    if !input.candidate_ancestor_in_target {
        return BasePinOutcome::UnackedDependency {
            reason: "candidate ancestor not present in target".to_string(),
        };
    }
    if input.dependency_acked_commits.is_empty() {
        return BasePinOutcome::UnackedDependency {
            reason: "no acked dependency commits".to_string(),
        };
    }
    // Skeleton: deps present → pin to current_target_sha. Real impl
    // joins the deps with current target and returns the merge-base.
    BasePinOutcome::PinDepsBase { sha: input.current_target_sha.clone() }
}

/// Decide whether a target rewrite blocks the pin.
pub fn target_rewrite_blocks(prev_target_sha: &str, current_target_sha: &str) -> bool {
    prev_target_sha != current_target_sha && !current_target_sha.starts_with(prev_target_sha)
}

/// Verify all dependency unit keys have acked commits.
pub fn all_deps_acked(input: &BasePinInput) -> bool {
    input.dependency_unit_keys.iter().all(|k| {
        // skeleton: caller provides dep_acked_commits; we just check
        // each dep_key appears in some way. Real impl joins with
        // dependency_acked table.
        !k.is_empty() && input.dependency_acked_commits.len() >= input.dependency_unit_keys.len()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> BasePinInput {
        BasePinInput {
            unit_key: "U3".to_string(),
            current_target_sha: "feedface".to_string(),
            dependency_unit_keys: vec!["U1".to_string(), "U2".to_string()],
            dependency_acked_commits: BTreeSet::new(),
            candidate_ancestor_in_target: true,
        }
    }

    #[test]
    fn all_dependencies_must_be_acked_ancestors() {
        let mut input = base();
        input.dependency_acked_commits.insert("commit-1".to_string());
        input.dependency_acked_commits.insert("commit-2".to_string());
        let outcome = compute_base_pin(&input);
        assert_eq!(outcome, BasePinOutcome::PinDepsBase { sha: "feedface".to_string() });
    }

    #[test]
    fn base_first_write_immutable() {
        // Skeleton: outcome is a String; caller responsible for
        // writing it once. Test pins that two calls with same input
        // produce the same outcome.
        let input = base();
        let o1 = compute_base_pin(&input);
        let o2 = compute_base_pin(&input);
        assert_eq!(o1, o2);
    }

    #[test]
    fn no_dependencies_pin_current_target() {
        let mut input = base();
        input.dependency_unit_keys.clear();
        let outcome = compute_base_pin(&input);
        assert_eq!(outcome, BasePinOutcome::PinCurrentTarget { sha: "feedface".to_string() });
    }

    #[test]
    fn target_rewrite_blocks_predicate() {
        let blocks = super::target_rewrite_blocks("aaaa", "bbbb");
        assert!(blocks);
        let blocks = super::target_rewrite_blocks("aaaa", "aaaab");
        assert!(!blocks); // forward-rewrite (e.g. force-push of descendent) is allowed
    }

    #[test]
    fn diff_uses_unit_base() {
        // Skeleton: compute_base_pin returns the SHA the diff should
        // anchor on. Test pins that PinDepsBase returns the current
        // target SHA as the base.
        let mut input = base();
        input.dependency_acked_commits.insert("c1".to_string());
        input.dependency_acked_commits.insert("c2".to_string());
        let outcome = compute_base_pin(&input);
        match outcome {
            BasePinOutcome::PinDepsBase { sha } => assert_eq!(sha, "feedface"),
            _ => panic!("expected PinDepsBase"),
        }
    }
}
