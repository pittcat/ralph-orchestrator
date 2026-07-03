//! 2026-07-02-006 plan U23: `handle_phase_on_event_accepted`.
//!
//! Slim free function the event loop calls after every
//! accepted business event. It (a) parses the event into
//! an `EventFixture`, (b) consults the
//! `WorkflowPhaseAuthority` facade, and (c) returns the
//! updated snapshot + side-effects the runtime needs to
//! commit (violation counts, review-walk close,
//! progress-projection text).
//!
//! The function is **not** `EventLoop::run`; the runtime
//! owns the engine and persists the returned state.
//! This module only does the pure translation.

use super::primitives::on_review_complete_verdict;
use super::primitives::on_test_passed_step::StepProgressFixture;
use super::evaluator::EventFixture;
use super::snapshot::{PhaseSnapshot, ViolationKind};
use super::step_parse::{parse_test_passed_step, TestPassedRecord};
use super::step_transition::advance_step_on_test_passed;
use super::WorkflowPhaseAuthority;
use serde_json::Value;

/// Inputs the runtime threads in. `event_payload` is the
/// JSON-decoded payload of the accepted event; the runtime
/// already has it.
#[derive(Debug, Clone)]
pub struct AcceptedEvent<'a> {
    pub topic: &'a str,
    pub payload: &'a Value,
    pub honored: bool,
}

/// Side-effects the runtime must apply. All fields are
/// optional so the runtime can decide whether to act.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhaseSideEffects {
    /// True when the engine has flipped into a new phase
    /// and the runtime should emit a diagnostic
    /// `phase.entered` envelope.
    pub phase_entered: bool,
    /// Markdown fragment the runtime should append to
    /// `progress.md` (U19).
    pub progress_md_fragment: String,
    /// `true` when the engine's review walk has just closed
    /// (U20 reads this for shipper routing).
    pub review_walk_closed: bool,
}

/// Pure-ish: consumes the event + facade, returns
/// `(updated_snapshot, side_effects)`. Does not write
/// anything to disk or commit a `CommitDelta`; the caller
/// owns those.
pub fn handle_phase_on_event_accepted(
    authority: &WorkflowPhaseAuthority,
    snapshot: PhaseSnapshot,
    event: &AcceptedEvent,
) -> (PhaseSnapshot, PhaseSideEffects) {
    if !authority.is_enabled() {
        return (snapshot, PhaseSideEffects::default());
    }

    let prev_snapshot = authority.snapshot().unwrap_or(snapshot);
    let fixture = build_fixture(event, authority.is_enabled());
    let next_snapshot = authority.on_event_accepted(&fixture);

    let phase_entered = next_snapshot.phase_id != prev_snapshot.phase_id;
    let review_walk_closed =
        !prev_snapshot.review_walk_closed && next_snapshot.review_walk_closed;

    let progress_md_fragment = if phase_entered {
        use super::progress_projection::{apply_progress_on_phase_enter, PhaseEnterContext};
        let cfg = authority.progress_projection().unwrap_or_default();
        let ctx = PhaseEnterContext {
            phase_id: next_snapshot.phase_id.clone(),
            last_completed_step: next_snapshot.last_completed_step.clone(),
            fix_unit_queue_exhausted: next_snapshot.fix_unit_queue_exhausted,
        };
        apply_progress_on_phase_enter(&cfg, &ctx)
    } else {
        String::new()
    };

    (
        next_snapshot,
        PhaseSideEffects {
            phase_entered,
            progress_md_fragment,
            review_walk_closed,
        },
    )
}

/// Convenience: build an `EventFixture` from an accepted
/// event. The runtime calls this once per event and feeds
/// the fixture into `WorkflowPhaseAuthority::on_event_accepted`.
pub fn build_fixture<'a>(
    event: &'a AcceptedEvent<'_>,
    phase_authority_enabled: bool,
) -> EventFixture<'a> {
    match event.topic {
        "test.passed" => {
            let fixture = if let Ok(record) =
                serde_json::from_value::<TestPassedRecord>(event.payload.clone())
            {
                advance_step_on_test_passed(phase_authority_enabled, &record)
            } else {
                parse_test_passed_step(event.payload).unwrap_or(StepProgressFixture {
                    kind: super::primitives::on_test_passed_step::StepKind::PlanUnit,
                    index: 0,
                    total: 0,
                })
            };
            EventFixture::TestPassed(fixture)
        }
        "review.complete" => EventFixture::ReviewComplete(
            on_review_complete_verdict::ReviewCompleteFixture {
                verdict: parse_verdict(event.payload),
                fix_plan_attached: !event
                    .payload
                    .get("fix_plan_file")
                    .and_then(|v| v.as_str())
                    .map(str::is_empty)
                    .unwrap_or(true),
            },
        ),
        "LOOP_COMPLETE" => EventFixture::LoopComplete { honored: event.honored },
        // Other topics drive only `on_event` rules; the
        // evaluator walks the declaration.
        other => EventFixture::Bare(other),
    }
}

fn parse_verdict(payload: &Value) -> on_review_complete_verdict::Verdict {
    let raw = payload
        .get("verdict")
        .and_then(|v| v.as_str())
        .unwrap_or("pass");
    on_review_complete_verdict::Verdict::from_token(raw)
        .unwrap_or(on_review_complete_verdict::Verdict::Pass)
}

// The runtime consults the snapshot's violation_counts to
// decide whether to admit a `task.resume` envelope. The
// canonical type lives in `super::snapshot::ViolationKind`
// and is re-exported from the `phase_authority` module root.

#[cfg(test)]
mod tests {
    use super::super::config::*;
    use super::super::declaration::PhaseAuthorityDeclaration;
    use super::super::snapshot::ViolationKind;
    use super::super::WorkflowPhaseAuthority;
    use super::*;
    use serde_json::json;

    fn serial_decl() -> PhaseAuthorityDeclaration {
        let cfg = PhaseAuthorityConfig {
            enabled: true,
            initial_phase: Some("unit_loop".to_string()),
            phases: vec![
                PhaseDeclConfig {
                    id: "unit_loop".to_string(),
                    label: None,
                    allowed_emits: Default::default(),
                },
                PhaseDeclConfig {
                    id: "review".to_string(),
                    label: None,
                    allowed_emits: Default::default(),
                },
                PhaseDeclConfig {
                    id: "plan_end".to_string(),
                    label: None,
                    allowed_emits: Default::default(),
                },
            ],
            transitions: vec![
                PhaseTransitionConfig {
                    from: "unit_loop".to_string(),
                    to: "review".to_string(),
                    on: serde_yaml::from_str(
                        r#"
primitive: on_test_passed_step
step_kind: plan_unit
when: last
"#,
                    )
                    .unwrap(),
                },
                PhaseTransitionConfig {
                    from: "review".to_string(),
                    to: "plan_end".to_string(),
                    on: serde_yaml::from_str(
                        r#"
primitive: on_review_complete_verdict
matrix: serial_default
"#,
                    )
                    .unwrap(),
                },
            ],
            violation_policy: ViolationPolicyConfig::default(),
            progress_projection: ProgressProjectionConfig::default(),
        };
        PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap()
    }

    #[test]
    fn disabled_engine_returns_unchanged_snapshot() {
        let fac = WorkflowPhaseAuthority::disabled();
        let snap = PhaseSnapshot::with_phase_id("unit_loop");
        let payload = json!({});
        let event = AcceptedEvent {
            topic: "work.start",
            payload: &payload,
            honored: false,
        };
        let (next, effects) = handle_phase_on_event_accepted(&fac, snap.clone(), &event);
        assert_eq!(next.phase_id, snap.phase_id);
        assert!(!effects.phase_entered);
        assert!(effects.progress_md_fragment.is_empty());
    }

    #[test]
    fn last_test_passed_advances_phase_to_review() {
        let fac = WorkflowPhaseAuthority::from_declaration(serial_decl());
        let snap = PhaseSnapshot::with_phase_id("unit_loop");
        let payload = json!({"index": 8, "total_units": 8});
        let event = AcceptedEvent {
            topic: "test.passed",
            payload: &payload,
            honored: false,
        };
        let (next, effects) = handle_phase_on_event_accepted(&fac, snap, &event);
        assert_eq!(next.phase_id, "review");
        assert!(effects.phase_entered);
    }

    #[test]
    fn non_last_test_passed_leaves_phase_unchanged() {
        let fac = WorkflowPhaseAuthority::from_declaration(serial_decl());
        let snap = PhaseSnapshot::with_phase_id("unit_loop");
        let payload = json!({"index": 3, "total_units": 8});
        let event = AcceptedEvent {
            topic: "test.passed",
            payload: &payload,
            honored: false,
        };
        let (next, effects) = handle_phase_on_event_accepted(&fac, snap, &event);
        assert_eq!(next.phase_id, "unit_loop");
        assert!(!effects.phase_entered);
    }

    #[test]
    fn review_complete_pass_routes_to_plan_end() {
        // Manually drive the engine through unit_loop → review
        // → plan_end. The runtime is expected to call
        // `update_snapshot` between events; for the unit test
        // we mutate a local facade mirror through the public
        // helper that calls update_snapshot.
        let mut fac = WorkflowPhaseAuthority::from_declaration(serial_decl());
        let snap0 = PhaseSnapshot::with_phase_id("unit_loop");
        fac.update_snapshot(snap0.clone());
        let payload1 = json!({"index": 8, "total_units": 8});
        let event1 = AcceptedEvent {
            topic: "test.passed",
            payload: &payload1,
            honored: false,
        };
        let (snap1, _) = handle_phase_on_event_accepted(&fac, snap0, &event1);
        fac.update_snapshot(snap1.clone());
        assert_eq!(snap1.phase_id, "review");

        let payload2 = json!({"verdict": "pass"});
        let event2 = AcceptedEvent {
            topic: "review.complete",
            payload: &payload2,
            honored: false,
        };
        let (snap2, effects) = handle_phase_on_event_accepted(&fac, snap1, &event2);
        fac.update_snapshot(snap2.clone());
        assert_eq!(snap2.phase_id, "plan_end");
        assert!(effects.phase_entered);
    }

    #[test]
    fn violation_kind_is_exported() {
        // Pin the re-export so the runtime's path remains stable.
        let _: ViolationKind = ViolationKind::PhaseViolation;
    }
}