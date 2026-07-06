//! 2026-07-02-006 plan: opt-in `WorkflowPhaseAuthority` engine.
//!
//! U1 — pure serde config (`config.rs`).
//! U2 — YAML → `PhaseAuthorityDeclaration` parser (`declaration.rs`).
//! U4 — `WhitelistIndex` (`whitelist.rs`).
//! U5 — `PhaseSnapshot` (`snapshot.rs`).
//! U6–U9 — transition primitives (`primitives/`).
//! U10 — `TransitionEvaluator` (`evaluator.rs`).
//! U11 — `WorkflowPhaseAuthority` facade (this file).

pub mod config;
pub mod declaration;
// U4: per-phase per-hat topic whitelist (pure decision fn).
pub mod whitelist;
// U5: PhaseSnapshot value type (no I/O).
pub mod snapshot;
// U6+: transition primitives. U6 lands `on_event`; U7–U9
// follow in lockstep with the plan.
pub mod primitives;
// U10: TransitionEvaluator composes U6–U9.
pub mod evaluator;
// U16: plan_gate skip helper.
pub mod plan_gate_helper;
// U17: progress_gate skip helper.
pub mod progress_gate_helper;
// U19: progress.md projection on phase enter.
pub mod progress_projection;
// U20: shipper routing helper.
pub mod shipper_helper;
// U21: test.passed step parser.
pub mod step_parse;
// U22: phase-violation resume budget.
pub mod resume_budget;
// U23: handle_phase_on_event_accepted free function.
pub mod on_accepted;
// U25: minimal second-preset fixture.
pub mod second_preset_fixture;
// U26: dual-check diagnosis helper (R14).
pub mod diagnosis;
// U27: advance_step_on_test_passed pure fn.
pub mod step_transition;

pub use on_accepted::{AcceptedEvent, PhaseSideEffects, handle_phase_on_event_accepted};

pub use declaration::PhaseAuthorityDeclaration;
pub use evaluator::{EventFixture, TransitionEvaluator};
pub use snapshot::{PhaseSnapshot, ViolationKind};
pub use whitelist::{WhitelistDecision, allows};

use std::sync::{Arc, Mutex};

use crate::event_loop::phase_authority::config::PhaseAuthorityConfig;

/// Engine facade. Construct once per loop; share via `Arc` so
/// emit gate and event-loop post-accept handler read/write the
/// same canonical snapshot.
#[derive(Clone)]
pub struct WorkflowPhaseAuthority {
    inner: Option<Arc<Mutex<EngineState>>>,
}

struct EngineState {
    decl: Arc<PhaseAuthorityDeclaration>,
    evaluator: TransitionEvaluator,
    snapshot: PhaseSnapshot,
    violation_policy: config::ViolationPolicyConfig,
    progress_projection: config::ProgressProjectionConfig,
}

impl std::fmt::Debug for WorkflowPhaseAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowPhaseAuthority")
            .field("enabled", &self.is_enabled())
            .field("phase_id", &self.current_phase_id())
            .finish()
    }
}

impl WorkflowPhaseAuthority {
    /// Disabled facade (KTD1 / R1). The runtime uses this when
    /// `mechanism.phase_authority.enabled == false` or the
    /// block is absent.
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// Build the engine from a parsed declaration. The runtime
    /// builds the declaration via `PhaseAuthorityDeclaration::try_from_config`
    /// (U2); the facade does not re-parse.
    pub fn from_declaration(decl: PhaseAuthorityDeclaration) -> Self {
        Self::from_declaration_with_policy(
            decl,
            config::ViolationPolicyConfig::default(),
            config::ProgressProjectionConfig::default(),
        )
    }

    fn from_declaration_with_policy(
        decl: PhaseAuthorityDeclaration,
        violation_policy: config::ViolationPolicyConfig,
        progress_projection: config::ProgressProjectionConfig,
    ) -> Self {
        let initial_phase = decl.initial_phase.clone().unwrap_or_default();
        let evaluator = TransitionEvaluator::new(Arc::new(decl.clone()));
        Self {
            inner: Some(Arc::new(Mutex::new(EngineState {
                decl: Arc::new(decl),
                evaluator,
                snapshot: PhaseSnapshot::with_phase_id(initial_phase),
                violation_policy,
                progress_projection,
            }))),
        }
    }

    /// Build from a typed config. When `enabled == false`
    /// returns `disabled()`. On declaration parse failure the
    /// caller is expected to surface the error via the lint
    /// (U3) and reject the preset before this is called.
    pub fn from_config(cfg: &PhaseAuthorityConfig) -> Result<Self, declaration::DeclarationError> {
        if !cfg.enabled {
            return Ok(Self::disabled());
        }
        let decl = PhaseAuthorityDeclaration::try_from_config(cfg)?;
        Ok(Self::from_declaration_with_policy(
            decl,
            cfg.violation_policy.clone(),
            cfg.progress_projection.clone(),
        ))
    }

    /// `true` when the engine is active.
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Current phase id. `None` when disabled.
    pub fn current_phase_id(&self) -> Option<String> {
        self.snapshot().map(|s| s.phase_id.clone())
    }

    /// Snapshot accessor. `None` when disabled.
    pub fn snapshot(&self) -> Option<PhaseSnapshot> {
        let state = self.inner.as_ref()?;
        let guard = state.lock().ok()?;
        Some(guard.snapshot.clone())
    }

    /// Run the evaluator against an accepted event. Updates the
    /// canonical snapshot and returns the post-event value.
    pub fn on_event_accepted(&self, fixture: &EventFixture) -> PhaseSnapshot {
        let Some(state) = self.inner.as_ref() else {
            return PhaseSnapshot::with_phase_id("disabled");
        };
        let Ok(mut guard) = state.lock() else {
            return PhaseSnapshot::with_phase_id("disabled");
        };
        let next = guard.evaluator.apply(guard.snapshot.clone(), fixture);
        guard.snapshot = next.clone();
        next
    }

    /// Persist a new snapshot (e.g. violation bump from resume budget).
    pub fn update_snapshot(&self, snapshot: PhaseSnapshot) {
        if let Some(state) = self.inner.as_ref()
            && let Ok(mut guard) = state.lock()
        {
            guard.snapshot = snapshot;
        }
    }

    /// Pure whitelist lookup. Mirrors
    /// `phase_authority::whitelist::allows` but resolves the
    /// current phase id automatically.
    pub fn allows(&self, hat_id: &str, topic: &str) -> WhitelistDecision {
        let Some(state) = self.inner.as_ref() else {
            return WhitelistDecision {
                allowed: true,
                phase_id: "disabled".to_string(),
                allowed_topics: Vec::new(),
            };
        };
        let Ok(guard) = state.lock() else {
            return WhitelistDecision {
                allowed: true,
                phase_id: "disabled".to_string(),
                allowed_topics: Vec::new(),
            };
        };
        let phase_id = guard.snapshot.phase_id.clone();
        allows(hat_id, topic, &phase_id, &guard.decl)
    }

    /// Accessor for tests / diagnostics. `None` when disabled.
    pub fn declaration(&self) -> Option<PhaseAuthorityDeclaration> {
        self.inner
            .as_ref()
            .and_then(|s| s.lock().ok().map(|g| (*g.decl).clone()))
    }

    /// Violation policy when the engine is enabled.
    pub fn violation_policy(&self) -> Option<config::ViolationPolicyConfig> {
        self.inner
            .as_ref()
            .and_then(|s| s.lock().ok().map(|g| g.violation_policy.clone()))
    }

    /// Progress projection config when the engine is enabled.
    pub fn progress_projection(&self) -> Option<config::ProgressProjectionConfig> {
        self.inner
            .as_ref()
            .and_then(|s| s.lock().ok().map(|g| g.progress_projection.clone()))
    }

    /// Bump phase-violation counter for a hat after a stage reject.
    pub fn record_phase_violation(&self, hat: &str) -> PhaseSnapshot {
        let Some(state) = self.inner.as_ref() else {
            return PhaseSnapshot::with_phase_id("disabled");
        };
        let Ok(mut guard) = state.lock() else {
            return PhaseSnapshot::with_phase_id("disabled");
        };
        guard.snapshot = guard
            .snapshot
            .clone()
            .bump_violation(hat, ViolationKind::PhaseViolation);
        guard.snapshot.clone()
    }
}

#[cfg(test)]
mod facade_tests {
    use crate::event_loop::phase_authority::config::*;
    use crate::event_loop::phase_authority::declaration::PhaseAuthorityDeclaration;
    use crate::event_loop::phase_authority::primitives::on_review_complete_verdict;
    use crate::event_loop::phase_authority::primitives::on_test_passed_step;
    use crate::event_loop::phase_authority::whitelist::allows as whitelist_allows;
    use crate::event_loop::phase_authority::*;

    fn enabled_declaration() -> PhaseAuthorityDeclaration {
        let cfg = PhaseAuthorityConfig {
            enabled: true,
            initial_phase: Some("unit_loop".to_string()),
            phases: vec![
                PhaseDeclConfig {
                    id: "unit_loop".to_string(),
                    label: None,
                    allowed_emits: [(
                        "coordinator".to_string(),
                        vec!["work.ready".to_string(), "queue.advance".to_string()],
                    )]
                    .into_iter()
                    .collect(),
                },
                PhaseDeclConfig {
                    id: "review".to_string(),
                    label: None,
                    allowed_emits: Default::default(),
                },
            ],
            transitions: vec![PhaseTransitionConfig {
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
            }],
            violation_policy: ViolationPolicyConfig::default(),
            progress_projection: ProgressProjectionConfig::default(),
        };
        PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap()
    }

    #[test]
    fn disabled_is_no_op() {
        let fac = WorkflowPhaseAuthority::disabled();
        assert!(!fac.is_enabled());
        assert_eq!(fac.current_phase_id(), None);
        let d = fac.allows("coordinator", "work.ready");
        assert!(d.allowed);
    }

    #[test]
    fn from_declaration_carries_initial_phase() {
        let fac = WorkflowPhaseAuthority::from_declaration(enabled_declaration());
        assert!(fac.is_enabled());
        assert_eq!(fac.current_phase_id(), Some("unit_loop".to_string()));
    }

    #[test]
    fn work_start_to_last_test_passed_walks_phase_to_review() {
        let fac = WorkflowPhaseAuthority::from_declaration(enabled_declaration());

        fac.on_event_accepted(&EventFixture::TestPassed(
            on_test_passed_step::StepProgressFixture {
                kind: on_test_passed_step::StepKind::PlanUnit,
                index: 8,
                total: 8,
            },
        ));
        assert_eq!(fac.current_phase_id(), Some("review".to_string()));
    }

    #[test]
    fn non_matching_event_does_not_change_phase() {
        let fac = WorkflowPhaseAuthority::from_declaration(enabled_declaration());

        fac.on_event_accepted(&EventFixture::TestPassed(
            on_test_passed_step::StepProgressFixture {
                kind: on_test_passed_step::StepKind::PlanUnit,
                index: 1,
                total: 8,
            },
        ));
        assert_eq!(fac.current_phase_id(), Some("unit_loop".to_string()));
    }

    #[test]
    fn whitelist_lookup_uses_current_phase() {
        let fac = WorkflowPhaseAuthority::from_declaration(enabled_declaration());
        let d = fac.allows("coordinator", "work.ready");
        assert!(d.allowed);
        assert_eq!(d.phase_id, "unit_loop");
    }

    #[test]
    fn review_complete_routes_through_matrix() {
        let cfg = PhaseAuthorityConfig {
            enabled: true,
            initial_phase: Some("review".to_string()),
            phases: vec![
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
            ],
            transitions: vec![PhaseTransitionConfig {
                from: "review".to_string(),
                to: "fix_units".to_string(),
                on: serde_yaml::from_str(
                    r#"
primitive: on_review_complete_verdict
matrix: serial_default
"#,
                )
                .unwrap(),
            }],
            violation_policy: ViolationPolicyConfig::default(),
            progress_projection: ProgressProjectionConfig::default(),
        };
        let decl = PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap();
        let fac = WorkflowPhaseAuthority::from_declaration(decl);

        fac.on_event_accepted(&EventFixture::ReviewComplete(
            on_review_complete_verdict::ReviewCompleteFixture {
                verdict: on_review_complete_verdict::Verdict::Fail,
                fix_plan_attached: true,
            },
        ));
        assert_eq!(fac.current_phase_id(), Some("fix_units".to_string()));
    }

    #[test]
    fn whitelist_lookup_in_unknown_phase_denies() {
        let fac = WorkflowPhaseAuthority::from_declaration(enabled_declaration());
        fac.update_snapshot(PhaseSnapshot::with_phase_id("nope"));
        let d = fac.allows("coordinator", "work.ready");
        assert!(!d.allowed);
    }

    #[test]
    fn declaration_accessor_returns_decl() {
        let fac = WorkflowPhaseAuthority::from_declaration(enabled_declaration());
        let decl = fac.declaration().expect("decl");
        assert!(decl.phases.iter().any(|p| p.id == "unit_loop"));
        let d = whitelist_allows("coordinator", "work.ready", "unit_loop", &decl);
        assert!(d.allowed);
    }
}
