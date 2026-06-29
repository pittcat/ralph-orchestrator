//! 2026-06-29-007 plan U6b: `CoordinatorDecisionGateStage`
//!
//! Rejects `work.ready` emits while the review walk has
//! not yet been closed. The stage reads
//! `flow_lifecycle.review_walk_closed` (U6b-introduced
//! field) and either accepts the event (closed) or
//! rejects it with `upstream_review_incomplete` (not yet
//! closed).
//!
//! Why this is a separate stage from `FlowStepScope`:
//! `FlowStepScope` enforces the *flow declaration* (which
//! topics are allowed at which step), whereas this stage
//! enforces the *runtime ordering contract* (no fix-unit
//! or next-step work can start before the review chain
//! finishes). Putting the two checks in distinct stages
//! keeps the failure modes greppable and the
//! BDD-scenario assertions stable.

use crate::event_loop::stage_pipeline::{EmitStage, StageContext, StageReject};
use ralph_proto::Event;
use std::cell::Cell;

const GUARDED_TOPIC: &str = "work.ready";

/// Per-loop flag tracking whether the review walk
/// (review-coordinator → dimension-reviewer →
/// review-synthesizer) has emitted its terminal
/// `review.complete`. Set on `review.complete` accept,
/// reset only by loop construction.
#[derive(Debug, Default, Clone)]
pub struct ReviewWalkClosedFlag {
    closed: Cell<bool>,
}

impl ReviewWalkClosedFlag {
    pub const fn new() -> Self {
        Self {
            closed: Cell::new(false),
        }
    }

    pub fn mark_closed(&self) {
        self.closed.set(true);
    }

    pub fn reset(&self) {
        self.closed.set(false);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.get()
    }
}

pub struct CoordinatorDecisionGateStage {
    pub flag: ReviewWalkClosedFlag,
}

impl CoordinatorDecisionGateStage {
    pub const fn new(flag: ReviewWalkClosedFlag) -> Self {
        Self { flag }
    }
}

impl EmitStage for CoordinatorDecisionGateStage {
    fn name(&self) -> &'static str {
        "CoordinatorDecisionGate"
    }

    fn check(&self, ctx: &mut StageContext, event: &Event) -> Result<(), StageReject> {
        if event.topic.as_str() != GUARDED_TOPIC {
            // Not a guarded topic. If the event is
            // `review.complete`, mark the walk closed so
            // the next `work.ready` is accepted.
            if event.topic.as_str() == "review.complete" {
                self.flag.mark_closed();
            }
            return Ok(());
        }

        if !self.flag.is_closed() {
            return Err(StageReject::new(
                self.name(),
                "upstream_review_incomplete",
            ));
        }

        // Re-borrow the context to ensure ctx is not
        // unused when the flag is already closed.
        let _ = ctx;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_loop::stage_pipeline::{FlowStep, StageContext};
    use crate::event_loop::repair_flow::RepairStateMachine;

    fn ctx<'a>(repair: &'a mut RepairStateMachine) -> StageContext<'a> {
        StageContext::for_test_machine(FlowStep::new("review_walk"), "loop-1", 1, repair)
    }

    fn event(topic: &str) -> Event {
        Event::new(topic, "{}")
    }

    #[test]
    fn work_ready_rejected_when_review_open() {
        let stage = CoordinatorDecisionGateStage::new(ReviewWalkClosedFlag::new());
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        let err = stage.check(&mut c, &event("work.ready")).unwrap_err();
        assert_eq!(err.reason_code, "upstream_review_incomplete");
    }

    #[test]
    fn work_ready_accepted_after_review_complete() {
        let stage = CoordinatorDecisionGateStage::new(ReviewWalkClosedFlag::new());
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        // First, accept review.complete to mark the walk closed.
        assert!(stage.check(&mut c, &event("review.complete")).is_ok());
        assert!(stage.flag.is_closed());
        // Then work.ready is accepted.
        assert!(stage.check(&mut c, &event("work.ready")).is_ok());
    }

    #[test]
    fn non_guarded_topics_pass_through() {
        let stage = CoordinatorDecisionGateStage::new(ReviewWalkClosedFlag::new());
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        for topic in ["work.done", "test.passed", "plan.complete"] {
            assert!(stage.check(&mut c, &event(topic)).is_ok());
        }
    }

    #[test]
    fn flag_reset_closes_walk_again() {
        let stage = CoordinatorDecisionGateStage::new(ReviewWalkClosedFlag::new());
        let mut repair = RepairStateMachine::default();
        let mut c = ctx(&mut repair);
        stage.check(&mut c, &event("review.complete")).unwrap();
        assert!(stage.flag.is_closed());
        stage.flag.reset();
        assert!(!stage.flag.is_closed());
        let err = stage.check(&mut c, &event("work.ready")).unwrap_err();
        assert_eq!(err.reason_code, "upstream_review_incomplete");
    }
}