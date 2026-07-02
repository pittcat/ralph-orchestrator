//! 2026-07-02-006 plan U5: `PhaseSnapshot` value type.
//!
//! Pure value type — no I/O, no events, no thread-safety concerns.
//! KTD10 pins the fields; the runtime projects one of these per
//! loop onto `LedgerSnapshot.workflow_phase` (U12). Updates are
//! non-destructive: helpers return a fresh snapshot so the engine
//! can keep an immutable history for diagnostics.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Snapshot of the workflow phase state. KTD10.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PhaseSnapshot {
    /// Active phase id (e.g. `unit_loop`).
    pub phase_id: String,

    /// `events.jsonl` sequence number when the snapshot was
    /// entered. Used for ordering and for the `entered_at_seq`
    /// tie-breaker in `progress_projection`.
    pub entered_at_seq: u64,

    /// Resume / violation counts keyed by `(hat_id,
    /// ViolationKind)`. U22 (`phase_violation_resume_budget`)
    /// reads this map to decide whether to honour the next
    /// `task.resume(reason_code=phase_violation)` request.
    #[serde(default)]
    pub violation_counts: HashMap<(String, ViolationKind), u32>,

    /// Absorbed from `CoordinatorDecisionGateStage` — set to
    /// `true` once the review walk (all dimensions received
    /// or aggregate timeout) closes. The shipper hat reads
    /// this in U20 to decide whether `plan.complete` is
    /// honoured before forwarding to `REVIEW_COMPLETE`.
    #[serde(default)]
    pub review_walk_closed: bool,

    /// Id of the most recently completed plan step (U19 reads
    /// this to render `progress.md` on phase-enter).
    #[serde(default)]
    pub last_completed_step: Option<String>,

    /// `true` when the fix-unit queue has been exhausted (U21
    /// sets it; U20 reads it to decide shipper routing).
    #[serde(default)]
    pub fix_unit_queue_exhausted: bool,
}

/// Discriminant for `violation_counts`. New variants are
/// added in lockstep with U22 (resume budget).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ViolationKind {
    /// `PhaseAuthorityStage` rejected an emit (out-of-phase
    /// topic). KTD6 step 1.
    PhaseViolation,
    /// `FlowStepScopeStage` rejected an emit (out-of-step
    /// topic). Future work — kept as a placeholder.
    FlowScopeViolation,
}

impl PhaseSnapshot {
    /// Build a fresh snapshot for the given phase id. `seq`
    /// defaults to 0 when the caller does not have the
    /// events-file sequence at hand (e.g. tests).
    pub fn with_phase_id(phase_id: impl Into<String>) -> Self {
        Self {
            phase_id: phase_id.into(),
            entered_at_seq: 0,
            violation_counts: HashMap::new(),
            review_walk_closed: false,
            last_completed_step: None,
            fix_unit_queue_exhausted: false,
        }
    }

    /// Set the `entered_at_seq` field. Returns a fresh
    /// snapshot so callers can chain.
    pub fn with_entered_at_seq(mut self, seq: u64) -> Self {
        self.entered_at_seq = seq;
        self
    }

    /// Bump the violation count for `(hat, kind)` by 1 and
    /// return the post-bump count. Returns a fresh snapshot.
    pub fn bump_violation(mut self, hat: &str, kind: ViolationKind) -> Self {
        let key = (hat.to_string(), kind);
        let entry = self.violation_counts.entry(key).or_insert(0);
        *entry += 1;
        self
    }

    /// Mark the review walk closed. Returns a fresh snapshot.
    pub fn mark_review_walk_closed(mut self) -> Self {
        self.review_walk_closed = true;
        self
    }

    /// Update `last_completed_step`. Returns a fresh snapshot.
    pub fn with_last_completed_step(mut self, step: impl Into<String>) -> Self {
        self.last_completed_step = Some(step.into());
        self
    }

    /// Mark the fix-unit queue exhausted. Returns a fresh
    /// snapshot.
    pub fn mark_fix_unit_queue_exhausted(mut self) -> Self {
        self.fix_unit_queue_exhausted = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_phase_id_starts_with_zero_violations() {
        let snap = PhaseSnapshot::with_phase_id("unit_loop");
        assert_eq!(snap.phase_id, "unit_loop");
        assert_eq!(snap.entered_at_seq, 0);
        assert!(snap.violation_counts.is_empty());
        assert!(!snap.review_walk_closed);
        assert!(snap.last_completed_step.is_none());
        assert!(!snap.fix_unit_queue_exhausted);
    }

    #[test]
    fn violation_counts_increment() {
        let snap = PhaseSnapshot::with_phase_id("unit_loop")
            .bump_violation("coordinator", ViolationKind::PhaseViolation)
            .bump_violation("coordinator", ViolationKind::PhaseViolation)
            .bump_violation("executor", ViolationKind::PhaseViolation);

        assert_eq!(
            snap.violation_counts
                .get(&("coordinator".to_string(), ViolationKind::PhaseViolation)),
            Some(&2)
        );
        assert_eq!(
            snap.violation_counts
                .get(&("executor".to_string(), ViolationKind::PhaseViolation)),
            Some(&1)
        );
    }

    #[test]
    fn review_walk_closed_flag_toggles() {
        let snap = PhaseSnapshot::with_phase_id("review").mark_review_walk_closed();
        assert!(snap.review_walk_closed);
    }

    #[test]
    fn entered_at_seq_propagates() {
        let snap = PhaseSnapshot::with_phase_id("review").with_entered_at_seq(42);
        assert_eq!(snap.entered_at_seq, 42);
    }

    #[test]
    fn fix_unit_queue_exhausted_flag_toggles() {
        let snap = PhaseSnapshot::with_phase_id("fix_units").mark_fix_unit_queue_exhausted();
        assert!(snap.fix_unit_queue_exhausted);
    }

    #[test]
    fn last_completed_step_records() {
        let snap = PhaseSnapshot::with_phase_id("unit_loop")
            .with_last_completed_step("step-03");
        assert_eq!(snap.last_completed_step.as_deref(), Some("step-03"));
    }
}