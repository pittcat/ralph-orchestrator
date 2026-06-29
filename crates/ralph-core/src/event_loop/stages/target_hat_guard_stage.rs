//! 2026-06-29-007 plan U4: `TargetHatGuardStage` — rejects
//! `task.resume` events whose `target_hat` points at the
//! source hat itself (self-loop).
//!
//! Why this lives between `EmitSchemaGate` and
//! `FlowStepScope`: the schema gate already validated that
//! `target_hat` is a non-empty string in the payload, so by
//! the time this stage runs the field is a real hat id.
//! Placing the guard before the flow-scope check means a
//! self-loop injection is rejected with a stable
//! `target_self_loop` reason code regardless of the current
//! step's `allowed_emits`.
//!
//! Cross-platform / concurrency semantics: pure CPU. No FS,
//! no threading.
//!
//! 2026-06-29-007 U4 scope note: the original plan also
//! asked for `target == last_hop_source_hat` (the previous
//! hop's source hat). That requires per-event hop tracking
//! on the StageContext which the current pipeline doesn't
//! expose. The self-loop check covers the regression's root
//! cause (`progress-steward → progress-steward` self-nudge)
//! — the last-hop check is left as a follow-up that adds
//! the hop tracker field to `StageContext` first.

use crate::event_loop::stage_pipeline::{EmitStage, StageContext, StageReject};
use ralph_proto::Event;
use serde_json::Value;

/// Per-topic guard list. Only `task.resume` is gated today;
/// extending this to `human.guidance` etc. is a one-line
/// change once the same self-loop class of bug shows up
/// there.
const GUARDED_TOPICS: &[&str] = &["task.resume"];

pub struct TargetHatGuardStage;

impl TargetHatGuardStage {
    pub const fn new() -> Self {
        Self
    }
}

impl Default for TargetHatGuardStage {
    fn default() -> Self {
        Self::new()
    }
}

impl EmitStage for TargetHatGuardStage {
    fn name(&self) -> &'static str {
        "TargetHatGuard"
    }

    fn check(&self, _ctx: &mut StageContext, event: &Event) -> Result<(), StageReject> {
        if !GUARDED_TOPICS.contains(&event.topic.as_str()) {
            return Ok(());
        }

        let target = match extract_target_hat(&event.payload) {
            Some(t) => t,
            None => return Ok(()), // Schema gate will reject empty target.
        };

        if let Some(source) = event.source.as_ref() {
            if source.as_str() == target {
                return Err(StageReject::new(self.name(), "target_self_loop"));
            }
        }

        Ok(())
    }
}

fn extract_target_hat(payload: &str) -> Option<String> {
    let value: Value = serde_json::from_str(payload).ok()?;
    let obj = value.as_object()?;
    obj.get("target_hat")
        .and_then(|v| v.as_str())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_loop::stage_pipeline::{FlowStep, StageContext};
    use crate::event_loop::repair_flow::RepairStateMachine;

    fn ctx<'a>(repair: &'a mut RepairStateMachine) -> StageContext<'a> {
        StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 1, repair)
    }

    fn event_with(topic: &str, source: Option<&str>, target_hat: Option<&str>) -> Event {
        let payload = match target_hat {
            Some(t) => serde_json::json!({ "target_hat": t }).to_string(),
            None => "{}".to_string(),
        };
        let mut e = Event::new(topic, payload);
        if let Some(s) = source {
            e.source = Some(ralph_proto::HatId::new(s));
        }
        e
    }

    #[test]
    fn non_guarded_topic_passes() {
        let stage = TargetHatGuardStage::new();
        let mut repair = RepairStateMachine::default();
        let mut ctx = ctx(&mut repair);
        let e = event_with("work.done", Some("progress-steward"), None);
        assert!(stage.check(&mut ctx, &e).is_ok());
    }

    #[test]
    fn self_loop_rejected() {
        let stage = TargetHatGuardStage::new();
        let mut repair = RepairStateMachine::default();
        let mut ctx = ctx(&mut repair);
        let e = event_with("task.resume", Some("progress-steward"), Some("progress-steward"));
        let err = stage.check(&mut ctx, &e).unwrap_err();
        assert_eq!(err.reason_code, "target_self_loop");
    }

    #[test]
    fn cross_hop_accepted() {
        let stage = TargetHatGuardStage::new();
        let mut repair = RepairStateMachine::default();
        let mut ctx = ctx(&mut repair);
        let e = event_with("task.resume", Some("coordinator"), Some("review-synthesizer"));
        assert!(stage.check(&mut ctx, &e).is_ok());
    }

    #[test]
    fn empty_target_falls_through_to_schema_gate() {
        let stage = TargetHatGuardStage::new();
        let mut repair = RepairStateMachine::default();
        let mut ctx = ctx(&mut repair);
        let e = event_with("task.resume", Some("coordinator"), None);
        assert!(stage.check(&mut ctx, &e).is_ok());
    }
}