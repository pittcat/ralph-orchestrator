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

/// 2026-06-29-007 plan U6b + U10: classify the
/// coordinator's `work.ready` payload to decide which
/// phase it belongs to. The decision drives whether the
/// stage rejects the emit (`upstream_review_incomplete`)
/// or rewrites the topic (`plan.complete` for last
/// `fix-NN`).
///
/// Plan §U10: branch table:
/// | step prefix | position | emit |
/// | step-NN | mid | work.ready (next step) |
/// | step-NN | last | review.start |
/// | fix-NN | mid | work.ready (next fix) |
/// | fix-NN | last | plan.complete (NOT review.start) |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseClass {
    /// step-NN, more units remaining → work.ready(next step)
    PlanUnitMid,
    /// step-NN, last unit → review.start
    PlanUnitLast,
    /// fix-NN, more fix-units remaining → work.ready(next fix)
    FixUnitMid,
    /// fix-NN, last fix-unit → plan.complete (U10 fix; was
    /// review.start before the 2026-06-29 fix).
    FixUnitLast,
    /// trivial plan → plan.complete
    Trivial,
    /// step prefix not recognised → no phase override.
    Unknown,
}

/// Parse the `step` field out of the work.ready payload
/// JSON. Returns `None` for malformed payloads — the
/// caller treats those as `Unknown`.
pub fn classify_work_ready(payload: &str) -> PhaseClass {
    let value: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return PhaseClass::Unknown,
    };
    let step_value = match value.get("step") {
        Some(v) => v,
        None => return PhaseClass::Unknown,
    };
    // step may be either a plain string ("fix-02") or an
    // object ({id, last_in_phase}). The object form is
    // the canonical emit from the projector (the legacy
    // string form is kept for ad-hoc test payloads).
    match step_value {
        serde_json::Value::String(s) => classify_plain_step_with_last(s, false),
        serde_json::Value::Object(_) => {
            let step_str = step_value
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let last = step_value
                .get("last_in_phase")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            classify_plain_step_with_last(step_str, last)
        }
        _ => PhaseClass::Unknown,
    }
}

fn classify_plain_step(step: &str) -> PhaseClass {
    classify_plain_step_with_last(step, false)
}

fn classify_plain_step_with_last(step: &str, last_in_phase: bool) -> PhaseClass {
    if step.starts_with("fix-") {
        if last_in_phase {
            PhaseClass::FixUnitLast
        } else {
            PhaseClass::FixUnitMid
        }
    } else if step.starts_with("step-") {
        if last_in_phase {
            PhaseClass::PlanUnitLast
        } else {
            PhaseClass::PlanUnitMid
        }
    } else if step == "trivial" {
        PhaseClass::Trivial
    } else {
        PhaseClass::Unknown
    }
}

/// Recommended topic for the given phase class. The
/// CoordinatorDecisionGate rewrites the event topic when
/// the class implies a non-`work.ready` emit (last
/// fix-unit → `plan.complete`, trivial → `plan.complete`).
pub fn topic_for_phase(class: PhaseClass) -> Option<&'static str> {
    match class {
        PhaseClass::PlanUnitLast => Some("review.start"),
        PhaseClass::FixUnitLast => Some("plan.complete"),
        PhaseClass::Trivial => Some("plan.complete"),
        PhaseClass::PlanUnitMid | PhaseClass::FixUnitMid | PhaseClass::Unknown => None,
    }
}

impl CoordinatorDecisionGateStage {
    /// U10 helper: rewrite a `work.ready` event's topic
    /// when the step prefix mandates a different terminal
    /// emit. The rewrite happens in-place on the event so
    /// the rest of the pipeline sees the new topic.
    /// Returns `true` when a rewrite happened.
    pub fn rewrite_work_ready_topic(event: &mut Event) -> bool {
        if event.topic.as_str() != "work.ready" {
            return false;
        }
        let class = classify_work_ready(&event.payload);
        if let Some(new_topic) = topic_for_phase(class) {
            event.topic = ralph_proto::Topic::new(new_topic);
            return true;
        }
        false
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

    #[test]
    fn classify_step_prefix_table() {
        assert_eq!(classify_plain_step("step-01"), PhaseClass::PlanUnitMid);
        assert_eq!(classify_plain_step("step-04"), PhaseClass::PlanUnitMid);
        assert_eq!(
            classify_plain_step_with_last("step-04", true),
            PhaseClass::PlanUnitLast
        );
        assert_eq!(classify_plain_step("fix-01"), PhaseClass::FixUnitMid);
        assert_eq!(
            classify_plain_step_with_last("fix-02", true),
            PhaseClass::FixUnitLast
        );
        assert_eq!(classify_plain_step("trivial"), PhaseClass::Trivial);
        assert_eq!(classify_plain_step("not-a-step"), PhaseClass::Unknown);
    }

    #[test]
    fn topic_for_phase_table() {
        assert_eq!(topic_for_phase(PhaseClass::PlanUnitMid), None);
        assert_eq!(topic_for_phase(PhaseClass::PlanUnitLast), Some("review.start"));
        assert_eq!(topic_for_phase(PhaseClass::FixUnitMid), None);
        assert_eq!(topic_for_phase(PhaseClass::FixUnitLast), Some("plan.complete"));
        assert_eq!(topic_for_phase(PhaseClass::Trivial), Some("plan.complete"));
        assert_eq!(topic_for_phase(PhaseClass::Unknown), None);
    }

    #[test]
    fn rewrite_work_ready_topic_for_last_fix_unit() {
        let mut e = event("work.ready");
        let _ = serde_json::from_str::<serde_json::Value>(&e.payload).unwrap(); // sanity
        e.payload =
            r#"{"step":{"id":"fix-02","last_in_phase":true},"task_id":"t"}"#.to_string();
        assert!(CoordinatorDecisionGateStage::rewrite_work_ready_topic(&mut e));
        assert_eq!(e.topic.as_str(), "plan.complete");
    }

    #[test]
    fn rewrite_work_ready_topic_for_mid_fix_unit_keeps_work_ready() {
        let mut e = event("work.ready");
        e.payload =
            r#"{"step":{"id":"fix-01","last_in_phase":false},"task_id":"t"}"#.to_string();
        assert!(!CoordinatorDecisionGateStage::rewrite_work_ready_topic(&mut e));
        assert_eq!(e.topic.as_str(), "work.ready");
    }

    #[test]
    fn classify_work_ready_with_plain_step_string() {
        // Plain-string step form (legacy / ad-hoc test
        // payloads) cannot carry `last_in_phase`, so the
        // classifier defaults to mid.
        assert_eq!(
            classify_work_ready(r#"{"step":"fix-02"}"#),
            PhaseClass::FixUnitMid
        );
        assert_eq!(
            classify_work_ready(r#"{"step":"step-03"}"#),
            PhaseClass::PlanUnitMid
        );
        assert_eq!(
            classify_work_ready(r#"{"step":"trivial"}"#),
            PhaseClass::Trivial
        );
        // Object form carries the `last_in_phase` flag.
        assert_eq!(
            classify_work_ready(
                r#"{"step":{"id":"fix-02","last_in_phase":true}}"#
            ),
            PhaseClass::FixUnitLast
        );
    }
}