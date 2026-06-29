//! 2026-06-29-007 plan U7: `TerminalStateGuardStage`
//!
//! Once `flow_lifecycle.phase` has reached
//! `Closed`/`Failed` (shipper REVIEW_COMPLETE verdict
//! promotes the phase), any further business event
//! (`review.dimension.*`, `review.start`, `work.ready`,
//! `task.resume`, `plan.*`) must be rejected with
//! `flow_state_closed`. The `LOOP_COMPLETE` topic is
//! intentionally NOT in the reject set — the
//! VerdictGate / `VERDICT_GATE_TOPICS` whitelist lets it
//! through so the explicit `LOOP_COMPLETE(success=verdict)`
//! emit from the event_loop can terminate the run.
//!
//! 2026-06-29-007 plan U7 rationale: the 2026-06-29
//! regression saw `event #40 = review.dimension.ready`
//! emitted *after* shipper verdict=fail, which kept the
//! loop alive and triggered the stall_recovery upgrade
//! path. This stage makes that emit a hard reject.

use crate::event_loop::stage_pipeline::{EmitStage, StageContext, StageReject};
use ralph_proto::Event;

/// Topics the stage rejects once `phase ∈ {Closed,
/// Failed}`. Anything outside this set (notably
/// `LOOP_COMPLETE` via `VERDICT_GATE_TOPICS`) flows
/// through normally — the event_loop's explicit
/// `LOOP_COMPLETE(success=verdict)` emit must reach the
/// bus to terminate the run.
const GUARDED_TOPICS: &[&str] = &[
    "review.start",
    "review.dimension.ready",
    "review.dimension.done",
    "review.dimension.failed",
    "review.dimensions.complete",
    "review.complete",
    "REVIEW_COMPLETE",
    "work.ready",
    "work.done",
    "task.resume",
    "human.guidance",
    "plan.complete",
    "plan.blocked",
];

/// Terminal phase values that trigger the reject.
const TERMINAL_PHASES: &[&str] = &["Closed", "Failed"];

/// Read `flow_lifecycle.phase` from the registry via the
/// stage context. `None` means the registry is still
/// active; `Some("Closed")` / `Some("Failed")` puts the
/// guard into reject mode.
pub fn current_phase_is_terminal(ctx: &StageContext) -> bool {
    match ctx.flow_phase.as_deref() {
        Some(p) => TERMINAL_PHASES.contains(&p),
        None => false,
    }
}

pub struct TerminalStateGuardStage;

impl TerminalStateGuardStage {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for TerminalStateGuardStage {
    fn default() -> Self {
        Self::new()
    }
}

impl EmitStage for TerminalStateGuardStage {
    fn name(&self) -> &'static str {
        "TerminalStateGuard"
    }

    fn check(&self, ctx: &mut StageContext, event: &Event) -> Result<(), StageReject> {
        if !current_phase_is_terminal(ctx) {
            return Ok(());
        }
        if !GUARDED_TOPICS.contains(&event.topic.as_str()) {
            return Ok(());
        }
        Err(StageReject::new(self.name(), "flow_state_closed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_loop::stage_pipeline::{FlowStep, StageContext};
    use crate::event_loop::repair_flow::RepairStateMachine;

    fn ctx_with_phase<'a>(
        repair: &'a mut RepairStateMachine,
        phase: Option<&str>,
    ) -> StageContext<'a> {
        let mut ctx = StageContext::for_test_machine(FlowStep::new("ship"), "loop-1", 1, repair);
        ctx.flow_phase = phase.map(String::from);
        ctx
    }

    fn event(topic: &str) -> Event {
        Event::new(topic, "{}")
    }

    #[test]
    fn non_terminal_phase_passes_through() {
        let stage = TerminalStateGuardStage::new();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx_with_phase(&mut repair, None);
        for topic in GUARDED_TOPICS {
            assert!(stage.check(&mut c, &event(topic)).is_ok());
        }
    }

    #[test]
    fn closed_phase_rejects_business_events() {
        let stage = TerminalStateGuardStage::new();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx_with_phase(&mut repair, Some("Closed"));
        for topic in GUARDED_TOPICS {
            let err = stage.check(&mut c, &event(topic)).unwrap_err();
            assert_eq!(err.reason_code, "flow_state_closed");
        }
    }

    #[test]
    fn failed_phase_rejects_business_events() {
        let stage = TerminalStateGuardStage::new();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx_with_phase(&mut repair, Some("Failed"));
        let err = stage.check(&mut c, &event("review.dimension.ready")).unwrap_err();
        assert_eq!(err.reason_code, "flow_state_closed");
    }

    #[test]
    fn closed_phase_passes_loop_complete() {
        let stage = TerminalStateGuardStage::new();
        let mut repair = RepairStateMachine::default();
        let mut c = ctx_with_phase(&mut repair, Some("Closed"));
        assert!(stage.check(&mut c, &event("LOOP_COMPLETE")).is_ok());
    }
}