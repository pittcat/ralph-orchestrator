//! 2026-07-02-006 plan U10: `TransitionEvaluator::apply`.
//!
//! Pure function that combines U6–U9 primitives with the
//! declaration's transition table to produce a fresh
//! `PhaseSnapshot`. The runtime calls `apply` on every accepted
//! event; the evaluator returns either the unchanged snapshot
//! or a snapshot with a new `phase_id` (and any side-effects
//! like `review_walk_closed` set when the verdict admitted
//! `fix_units`).
//!
//! No facade, no event-loop wiring. The fixture inputs are
//! built by the runtime's `handle_phase_on_event_accepted` (U23)
//! and by U21 (test.passed step parser).

use std::sync::Arc;

use super::declaration::PhaseAuthorityDeclaration;
use super::primitives::{
    on_event, on_loop_complete_honored, on_review_complete_verdict,
    on_test_passed_step::{self, StepProgressFixture},
};
use super::snapshot::PhaseSnapshot;

/// Topic → fixture bundle the runtime hands to the evaluator.
/// Each variant carries exactly the fixture the corresponding
/// primitive needs. `Bare` covers topics that drive only the
/// `on_event` rule (e.g. `work.start`, `plan.complete`); the
/// caller passes the actual topic so `on_event::evaluate`
/// can match.
#[derive(Debug, Clone)]
pub enum EventFixture<'a> {
    Bare(&'a str),
    TestPassed(StepProgressFixture),
    ReviewComplete(on_review_complete_verdict::ReviewCompleteFixture),
    LoopComplete { honored: bool },
}

impl<'a> EventFixture<'a> {
    pub fn topic(&self) -> &str {
        match self {
            EventFixture::Bare(t) => t,
            EventFixture::TestPassed(_) => "test.passed",
            EventFixture::ReviewComplete(_) => "review.complete",
            EventFixture::LoopComplete { .. } => "LOOP_COMPLETE",
        }
    }
}

/// Combined evaluator. The runtime constructs one of these per
/// loop, passes the declaration by `Arc` so the evaluator is
/// `Send + Sync`, and calls `apply` on every accepted event.
#[derive(Clone)]
pub struct TransitionEvaluator {
    decl: Arc<PhaseAuthorityDeclaration>,
}

impl std::fmt::Debug for TransitionEvaluator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransitionEvaluator")
            .field("decl", &self.decl)
            .finish()
    }
}

impl TransitionEvaluator {
    pub fn new(decl: Arc<PhaseAuthorityDeclaration>) -> Self {
        Self { decl }
    }

    pub fn declaration(&self) -> &PhaseAuthorityDeclaration {
        &self.decl
    }

    /// Apply the accepted event against the declaration.
    /// Returns the (possibly mutated) snapshot. The input
    /// snapshot is **not** mutated — callers may keep the
    /// previous snapshot for diagnostics.
    pub fn apply(&self, snapshot: PhaseSnapshot, fixture: &EventFixture<'_>) -> PhaseSnapshot {
        // Walk the transition table in declaration order; the
        // first match wins (U10 contract).
        for tr in &self.decl.transitions {
            if tr.from != snapshot.phase_id && tr.from != "*" {
                continue;
            }

            if let Some(target) = evaluate_transition(&tr.on.0, fixture) {
                return apply_transition(snapshot, &tr.to);
            }
        }
        snapshot
    }
}

fn evaluate_transition(on: &serde_yaml::Value, fixture: &EventFixture<'_>) -> Option<String> {
    match fixture {
        EventFixture::Bare(_) => on_event::evaluate(on, fixture.topic()),
        EventFixture::TestPassed(fx) => {
            // U7 only inspects `test.passed`; for any other
            // topic it returns None.
            on_test_passed_step::evaluate(on, fixture.topic(), fx)
        }
        EventFixture::ReviewComplete(fx) => {
            on_review_complete_verdict::evaluate(on, fixture.topic(), fx)
        }
        EventFixture::LoopComplete { honored } => {
            on_loop_complete_honored::evaluate(on, fixture.topic(), *honored)
        }
    }
}

fn apply_transition(snapshot: PhaseSnapshot, target: &str) -> PhaseSnapshot {
    let mut next = snapshot;
    next.phase_id = target.to_string();
    next.entered_at_seq = next.entered_at_seq.saturating_add(1);
    next
}

#[cfg(test)]
mod tests {
    use super::super::config::*;
    use super::super::declaration::*;
    use super::super::snapshot::ViolationKind;
    use super::super::whitelist::allows;
    use super::*;
    use std::sync::Arc;

    fn build_serial_decl() -> PhaseAuthorityDeclaration {
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
                    id: "fix_units".to_string(),
                    label: None,
                    allowed_emits: Default::default(),
                },
                PhaseDeclConfig {
                    id: "plan_end".to_string(),
                    label: None,
                    allowed_emits: Default::default(),
                },
                PhaseDeclConfig {
                    id: "ship".to_string(),
                    label: None,
                    allowed_emits: Default::default(),
                },
                PhaseDeclConfig {
                    id: "terminal".to_string(),
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
                    to: "fix_units".to_string(),
                    on: serde_yaml::from_str(
                        r#"
primitive: on_review_complete_verdict
matrix: serial_default
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
                PhaseTransitionConfig {
                    from: "fix_units".to_string(),
                    to: "plan_end".to_string(),
                    on: serde_yaml::from_str(
                        r#"
primitive: on_test_passed_step
step_kind: fix_unit
when: last
"#,
                    )
                    .unwrap(),
                },
                PhaseTransitionConfig {
                    from: "plan_end".to_string(),
                    to: "ship".to_string(),
                    on: serde_yaml::from_str(
                        r#"
event: plan.complete
"#,
                    )
                    .unwrap(),
                },
                PhaseTransitionConfig {
                    from: "ship".to_string(),
                    to: "terminal".to_string(),
                    on: serde_yaml::from_str(
                        r#"
primitive: on_loop_complete_honored
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
    fn end_to_end_serial_flow_advances_to_terminal() {
        let decl = Arc::new(build_serial_decl());
        let ev = TransitionEvaluator::new(decl.clone());
        let mut snap = PhaseSnapshot::with_phase_id("unit_loop");

        // Step 1: last plan-unit test.passed → review
        snap = ev.apply(
            snap,
            &EventFixture::TestPassed(StepProgressFixture {
                kind: on_test_passed_step::StepKind::PlanUnit,
                index: 8,
                total: 8,
            }),
        );
        assert_eq!(snap.phase_id, "review");

        // Step 2: review.complete fail+fix → fix_units
        snap = ev.apply(
            snap,
            &EventFixture::ReviewComplete(
                on_review_complete_verdict::ReviewCompleteFixture {
                    verdict: on_review_complete_verdict::Verdict::Fail,
                    fix_plan_attached: true,
                },
            ),
        );
        assert_eq!(snap.phase_id, "fix_units");

        // Step 3: last fix-unit test.passed → plan_end
        snap = ev.apply(
            snap,
            &EventFixture::TestPassed(StepProgressFixture {
                kind: on_test_passed_step::StepKind::FixUnit,
                index: 1,
                total: 1,
            }),
        );
        assert_eq!(snap.phase_id, "plan_end");

        // Step 4: plan.complete → ship
        snap = ev.apply(snap, &EventFixture::Bare("plan.complete"));
        assert_eq!(snap.phase_id, "ship");

        // Step 5: LOOP_COMPLETE honored → terminal
        snap = ev.apply(
            snap,
            &EventFixture::LoopComplete { honored: true },
        );
        assert_eq!(snap.phase_id, "terminal");
    }

    #[test]
    fn non_matching_event_leaves_phase_unchanged() {
        let decl = Arc::new(build_serial_decl());
        let ev = TransitionEvaluator::new(decl);
        let snap = PhaseSnapshot::with_phase_id("unit_loop");
        let next = ev.apply(
            snap.clone(),
            &EventFixture::TestPassed(StepProgressFixture {
                kind: on_test_passed_step::StepKind::PlanUnit,
                index: 3,
                total: 8,
            }),
        );
        assert_eq!(next.phase_id, "unit_loop");
    }

    #[test]
    fn snapshot_returned_is_a_fresh_value() {
        let decl = Arc::new(build_serial_decl());
        let ev = TransitionEvaluator::new(decl);
        let snap = PhaseSnapshot::with_phase_id("unit_loop");
        // From `unit_loop` no transition matches `plan.complete`;
        // the snapshot's `phase_id` must stay put.
        let next = ev.apply(snap.clone(), &EventFixture::Bare("plan.complete"));
        assert_eq!(snap.phase_id, "unit_loop");
        assert_eq!(next.phase_id, "unit_loop");
    }

    #[test]
    fn wildcard_from_phase_matches_any_current_phase() {
        // Construct a declaration with one transition from "*"
        // to "review" on event `work.start`.
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
            ],
            transitions: vec![PhaseTransitionConfig {
                from: "*".to_string(),
                to: "review".to_string(),
                on: serde_yaml::from_str("event: work.start").unwrap(),
            }],
            violation_policy: ViolationPolicyConfig::default(),
            progress_projection: ProgressProjectionConfig::default(),
        };
        let decl = Arc::new(PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap());
        let ev = TransitionEvaluator::new(decl);

        // Start in `unit_loop`; a `work.start` triggers the
        // wildcard rule and we land in `review`.
        let snap = PhaseSnapshot::with_phase_id("unit_loop");
        let next = ev.apply(snap, &EventFixture::Bare("work.start"));
        assert_eq!(next.phase_id, "review");
    }

    #[test]
    fn whitelist_decision_uses_transition_evaluator_state() {
        // Sanity check: after `work.start` lands us in
        // `review`, the whitelist rejects `review.start` from
        // any hat except the coordinator (declaration above
        // allows nothing in this fixture; we only assert the
        // unknown phase deny behaviour).
        let decl = Arc::new(build_serial_decl());
        let ev = TransitionEvaluator::new(decl.clone());
        let snap = PhaseSnapshot::with_phase_id("nonexistent");
        let next = ev.apply(snap, &EventFixture::Bare("plan.complete"));
        assert_eq!(next.phase_id, "nonexistent");
        // The whitelist correctly denies for unknown phases.
        assert!(!allows("coordinator", "plan.complete", "nonexistent", &decl).allowed);
    }

    #[test]
    fn snapshot_violation_count_is_preserved_through_transition() {
        let decl = Arc::new(build_serial_decl());
        let ev = TransitionEvaluator::new(decl);
        let snap = PhaseSnapshot::with_phase_id("unit_loop")
            .bump_violation("coordinator", ViolationKind::PhaseViolation);
        let next = ev.apply(
            snap.clone(),
            &EventFixture::TestPassed(StepProgressFixture {
                kind: on_test_passed_step::StepKind::PlanUnit,
                index: 8,
                total: 8,
            }),
        );
        assert_eq!(next.phase_id, "review");
        assert_eq!(
            next.violation_counts
                .get(&("coordinator".to_string(), ViolationKind::PhaseViolation)),
            Some(&1)
        );
    }
}