//! 2026-07-02-006 plan U13: `PhaseAuthorityStage` — emit-time
//! gate that consults the `WorkflowPhaseAuthority` facade and
//! rejects out-of-phase topics with a stable `phase_violation`
//! reason code.
//!
//! The stage is intentionally **stateless** with respect to the
//! engine: it owns only an `Arc<WorkflowPhaseAuthority>`
//! reference. The runtime builds the engine (U11 / U23), stores
//! it on the `EventLoop`, and threads the `Arc` into the stage
//! at construction. The stage's `check` does NOT advance the
//! snapshot — that is `handle_phase_on_event_accepted` (U23)'s
//! job after the pipeline accepts the event.
//!
//! The unit-test scope is limited to: stub authority with a
//! fixed phase id, drive `check` directly, observe
//! accept / reject. The stage is **not** wired into
//! `build_stage_pipeline_from_config` here (U15).

use std::sync::Arc;

use crate::event_loop::phase_authority::WorkflowPhaseAuthority;
use crate::event_loop::stage_pipeline::{EmitStage, StageContext, StageReject};
use ralph_proto::Event;

/// Emit-gate stage that consults the phase authority facade.
/// Constructed via `PhaseAuthorityStage::new(Arc<...>)`; the
/// runtime owns the engine and passes a clone of the `Arc`.
pub struct PhaseAuthorityStage {
    authority: Arc<WorkflowPhaseAuthority>,
}

impl PhaseAuthorityStage {
    pub fn new(authority: Arc<WorkflowPhaseAuthority>) -> Self {
        Self { authority }
    }
}

impl EmitStage for PhaseAuthorityStage {
    fn name(&self) -> &'static str {
        "PhaseAuthority"
    }

    fn check(&self, _ctx: &mut StageContext, event: &Event) -> Result<(), StageReject> {
        // When the engine is disabled the facade short-circuits
        // every lookup to `allowed = true`; the stage is then
        // inert. This preserves R1 (opt-in only) and keeps the
        // pre-006 baseline for pipeline / coordinator presets.
        if !self.authority.is_enabled() {
            return Ok(());
        }

        // The hat that owns the event is the gate's source.
        // The facade's `allows` takes a hat id and resolves
        // the current phase id internally. When the event has
        // no source (legacy fixtures, test scaffolding) the
        // stage skips the whitelist check — the missing
        // source is the schema gate's job, not ours.
        let Some(source) = event.source.as_ref() else {
            return Ok(());
        };

        let decision = self.authority.allows(source.as_str(), event.topic.as_str());
        if decision.allowed {
            return Ok(());
        }

        let mut reject = StageReject::new(self.name(), "phase_violation");
        reject.missing_fields = vec![
            format!("phase={}", decision.phase_id),
            format!(
                "allowed_topics={}",
                if decision.allowed_topics.is_empty() {
                    "<none>".to_string()
                } else {
                    decision.allowed_topics.join(",")
                }
            ),
            format!("rejected_topic={}", event.topic),
            format!("source_hat={}", source.as_str()),
        ];
        Err(reject)
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::phase_authority::config::*;
    use super::super::super::phase_authority::declaration::PhaseAuthorityDeclaration;
    use crate::event_loop::phase_authority::WorkflowPhaseAuthority;
    use crate::event_loop::repair_flow::RepairStateMachine;
    use crate::event_loop::stage_pipeline::{FlowStep, StageContext};
    use ralph_proto::{Event, HatId};

    use super::*;

    fn ctx<'a>(repair: &'a mut RepairStateMachine) -> StageContext<'a> {
        StageContext::for_test_machine(FlowStep::new("unit_loop"), "loop-1", 1, repair)
    }

    fn engine_in_plan_end() -> Arc<WorkflowPhaseAuthority> {
        let cfg = PhaseAuthorityConfig {
            enabled: true,
            initial_phase: Some("plan_end".to_string()),
            phases: vec![PhaseDeclConfig {
                id: "plan_end".to_string(),
                label: None,
                allowed_emits: [(
                    "coordinator".to_string(),
                    vec!["plan.complete".to_string(), "plan.blocked".to_string()],
                )]
                .into_iter()
                .collect(),
            }],
            transitions: Vec::new(),
            violation_policy: ViolationPolicyConfig::default(),
            progress_projection: ProgressProjectionConfig::default(),
        };
        let decl = PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap();
        Arc::new(WorkflowPhaseAuthority::from_declaration(decl))
    }

    fn event_with(topic: &str, source: Option<&str>) -> Event {
        let mut e = Event::new(topic, "{}");
        if let Some(s) = source {
            e.source = Some(HatId::new(s));
        }
        e
    }

    #[test]
    fn plan_end_rejects_review_start_from_coordinator() {
        let stage = PhaseAuthorityStage::new(engine_in_plan_end());
        let mut repair = RepairStateMachine::default();
        let mut ctx = ctx(&mut repair);
        let e = event_with("review.start", Some("coordinator"));
        let err = stage.check(&mut ctx, &e).unwrap_err();
        assert_eq!(err.stage_name, "PhaseAuthority");
        assert_eq!(err.reason_code, "phase_violation");
        // Diagnostic fields are surfaced via `missing_fields` so
        // the runtime can render a useful correction envelope.
        assert!(
            err.missing_fields
                .iter()
                .any(|f| f.contains("phase=plan_end"))
        );
        assert!(
            err.missing_fields
                .iter()
                .any(|f| f.contains("rejected_topic=review.start"))
        );
    }

    #[test]
    fn plan_end_allows_plan_complete_from_coordinator() {
        let stage = PhaseAuthorityStage::new(engine_in_plan_end());
        let mut repair = RepairStateMachine::default();
        let mut ctx = ctx(&mut repair);
        let e = event_with("plan.complete", Some("coordinator"));
        assert!(stage.check(&mut ctx, &e).is_ok());
    }

    #[test]
    fn disabled_engine_short_circuits_to_ok() {
        let stage = PhaseAuthorityStage::new(Arc::new(WorkflowPhaseAuthority::disabled()));
        let mut repair = RepairStateMachine::default();
        let mut ctx = ctx(&mut repair);
        // Anything goes when the engine is off.
        let e = event_with("review.start", Some("coordinator"));
        assert!(stage.check(&mut ctx, &e).is_ok());
    }

    #[test]
    fn event_without_source_falls_through() {
        let stage = PhaseAuthorityStage::new(engine_in_plan_end());
        let mut repair = RepairStateMachine::default();
        let mut ctx = ctx(&mut repair);
        let e = event_with("review.start", None);
        // No source → stage cannot attribute the emit; the
        // schema gate already validates required fields, the
        // missing source is not our problem.
        assert!(stage.check(&mut ctx, &e).is_ok());
    }
}
