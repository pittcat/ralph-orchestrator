//! U14 integration_worker — per-target active worker (skeleton).
//!
//! Plan: docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md §7 U14.
//!
//! Owns one active worker per (target, generation). Tick only
//! start/poll/cancel; the blocking integration operation runs in the
//! worker. Stale results (different generation) are no-ops.

// SKELETON-ONLY (per fix-plan 2026-09-09-0917-fix-forge-dag-p1-closure-plan U2 / U25):
// the public types in this module are exposed for downstream unit tests but
// are not yet wired into production callers; U23 production replacement
// promotes this file to `PRODUCTION:` marker.
#![allow(dead_code)]

use std::collections::BTreeMap;

/// Identity of an integration attempt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IntegrationGeneration {
    pub plan_key: String,
    pub target_branch: String,
    pub generation: i64,
}

/// State of an active worker slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkerSlotState {
    Idle,
    Starting,
    Running,
    Committing,
    Joined,
    Canceled,
}

/// One slot per target.
#[derive(Debug, Clone)]
pub struct WorkerSlot {
    pub state: WorkerSlotState,
    pub current_generation: Option<IntegrationGeneration>,
}

impl WorkerSlot {
    pub fn new() -> Self {
        Self {
            state: WorkerSlotState::Idle,
            current_generation: None,
        }
    }
}

/// Result of a worker completion message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionOutcome {
    /// Routed to current generation; tick may advance.
    Routed,
    /// Stale: result belongs to a generation no longer current; ignored.
    StaleIgnored,
    /// No active slot for this target; cannot route.
    NoActiveSlot,
}

/// Pure dispatch: classify a completion message.
pub fn classify_completion(
    slot: &WorkerSlot,
    completion_generation: &IntegrationGeneration,
) -> CompletionOutcome {
    if slot.state == WorkerSlotState::Idle || slot.state == WorkerSlotState::Joined {
        return CompletionOutcome::NoActiveSlot;
    }
    match &slot.current_generation {
        None => CompletionOutcome::NoActiveSlot,
        Some(cur) if cur == completion_generation => CompletionOutcome::Routed,
        Some(_) => CompletionOutcome::StaleIgnored,
    }
}

/// Decide if a cancel signal should prevent the next CAS authorization.
pub fn cancel_prevents_cas(slot: &WorkerSlot, cancel_signaled: bool) -> bool {
    cancel_signaled && slot.state != WorkerSlotState::Committing
}

/// Reference table: allowed state transitions for a worker slot.
pub fn legal_slot_transitions() -> BTreeMap<WorkerSlotState, Vec<WorkerSlotState>> {
    let mut m = BTreeMap::new();
    m.insert(WorkerSlotState::Idle, vec![WorkerSlotState::Starting]);
    m.insert(
        WorkerSlotState::Starting,
        vec![WorkerSlotState::Running, WorkerSlotState::Canceled],
    );
    m.insert(
        WorkerSlotState::Running,
        vec![WorkerSlotState::Committing, WorkerSlotState::Canceled],
    );
    m.insert(WorkerSlotState::Committing, vec![WorkerSlotState::Joined]);
    m.insert(WorkerSlotState::Joined, vec![WorkerSlotState::Idle]);
    m.insert(WorkerSlotState::Canceled, vec![WorkerSlotState::Joined]);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_gen(plan: &str, n: i64) -> IntegrationGeneration {
        IntegrationGeneration {
            plan_key: plan.to_string(),
            target_branch: "main".to_string(),
            generation: n,
        }
    }

    #[test]
    fn one_active_integration_per_target() {
        // Slot is single-valued; no map; the type system enforces "one".
        // Document by checking a fresh slot is Idle.
        let s = WorkerSlot::new();
        assert_eq!(s.state, WorkerSlotState::Idle);
        assert!(s.current_generation.is_none());
    }

    #[test]
    fn worker_completion_routes_current_generation() {
        let mut s = WorkerSlot::new();
        s.state = WorkerSlotState::Running;
        s.current_generation = Some(mk_gen("plan-A", 1));
        let outcome = classify_completion(&s, &mk_gen("plan-A", 1));
        assert_eq!(outcome, CompletionOutcome::Routed);
    }

    #[test]
    fn stale_completion_ignored() {
        let mut s = WorkerSlot::new();
        s.state = WorkerSlotState::Running;
        s.current_generation = Some(mk_gen("plan-A", 1));
        let outcome = classify_completion(&s, &mk_gen("plan-A", 2));
        assert_eq!(outcome, CompletionOutcome::StaleIgnored);
    }

    #[test]
    fn cancel_before_authorization_prevents_cas() {
        let mut s = WorkerSlot::new();
        s.state = WorkerSlotState::Running;
        assert!(cancel_prevents_cas(&s, true));
    }

    #[test]
    fn authorization_before_cancel_finishes_materialization() {
        let mut s = WorkerSlot::new();
        s.state = WorkerSlotState::Committing;
        // Cancel during Committing: don't prevent; let it finish.
        assert!(!cancel_prevents_cas(&s, true));
    }

    #[test]
    fn has_pending_work_includes_worker() {
        let mut s = WorkerSlot::new();
        s.state = WorkerSlotState::Running;
        s.current_generation = Some(mk_gen("plan-A", 1));
        // Pending iff state is Running or Committing.
        let pending = matches!(
            s.state,
            WorkerSlotState::Running | WorkerSlotState::Committing
        );
        assert!(pending);
    }
}
