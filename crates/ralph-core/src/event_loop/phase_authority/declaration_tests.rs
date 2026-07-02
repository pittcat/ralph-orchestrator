//! 2026-07-02-006 plan U2: tests for
//! `PhaseAuthorityDeclaration::try_from_config`.
//!
//! Test entry point:
//! `cargo nextest run -p ralph-core -- phase_authority_declaration`.

use super::config::*;
use super::declaration::*;

fn minimal_two_phase_config() -> PhaseAuthorityConfig {
    PhaseAuthorityConfig {
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
            from: "unit_loop".to_string(),
            to: "review".to_string(),
            on: TransitionOnConfig(serde_yaml::Value::Null),
        }],
        violation_policy: ViolationPolicyConfig::default(),
        progress_projection: ProgressProjectionConfig::default(),
    }
}

#[test]
fn declaration_parses_minimal_two_phase() {
    let cfg = minimal_two_phase_config();
    let decl = PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap();

    assert_eq!(decl.phases.len(), 2);
    assert_eq!(decl.phases[0].id, "unit_loop");
    assert_eq!(decl.phases[1].id, "review");
    assert_eq!(decl.transitions.len(), 1);
    assert_eq!(decl.transitions[0].from, "unit_loop");
    assert_eq!(decl.transitions[0].to, "review");
    assert_eq!(decl.initial_phase.as_deref(), Some("unit_loop"));
}

#[test]
fn declaration_rejects_duplicate_phase_id() {
    let mut cfg = minimal_two_phase_config();
    cfg.phases.push(PhaseDeclConfig {
        id: "unit_loop".to_string(),
        label: None,
        allowed_emits: Default::default(),
    });

    let err = PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap_err();
    assert_eq!(err, DeclarationError::DuplicatePhaseId("unit_loop".to_string()));
}

#[test]
fn declaration_rejects_dangling_transition_from() {
    let mut cfg = minimal_two_phase_config();
    cfg.phases = vec![PhaseDeclConfig {
        id: "unit_loop".to_string(),
        label: None,
        allowed_emits: Default::default(),
    }];
    // transitions[0] still references `review` which no longer
    // exists.

    let err = PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap_err();
    assert!(matches!(
        err,
        DeclarationError::UnknownPhase { ref phase, .. } if phase == "review"
    ));
}

#[test]
fn declaration_rejects_dangling_transition_to() {
    let mut cfg = minimal_two_phase_config();
    cfg.transitions.push(PhaseTransitionConfig {
        from: "unit_loop".to_string(),
        to: "nowhere".to_string(),
        on: TransitionOnConfig(serde_yaml::Value::Null),
    });

    let err = PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap_err();
    assert!(matches!(
        err,
        DeclarationError::UnknownPhase { ref phase, .. } if phase == "nowhere"
    ));
}

#[test]
fn declaration_resolves_initial_phase_when_absent_to_first_phase() {
    let mut cfg = minimal_two_phase_config();
    cfg.initial_phase = None;

    let decl = PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap();
    assert_eq!(decl.initial_phase.as_deref(), Some("unit_loop"));
}

#[test]
fn declaration_rejects_unknown_initial_phase() {
    let mut cfg = minimal_two_phase_config();
    cfg.initial_phase = Some("nope".to_string());

    let err = PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap_err();
    assert_eq!(err, DeclarationError::UnknownInitialPhase("nope".to_string()));
}

#[test]
fn declaration_rejects_incomplete_transition() {
    let mut cfg = minimal_two_phase_config();
    cfg.transitions.push(PhaseTransitionConfig {
        from: "".to_string(),
        to: "review".to_string(),
        on: TransitionOnConfig(serde_yaml::Value::Null),
    });

    let err = PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap_err();
    assert!(matches!(err, DeclarationError::IncompleteTransition(_)));
}

#[test]
fn declaration_accepts_wildcard_from_phase() {
    // `from: "*"` is reserved by U10 for "any phase"; U2 only
    // verifies the literal token survives normalisation.
    let mut cfg = minimal_two_phase_config();
    cfg.transitions.push(PhaseTransitionConfig {
        from: "*".to_string(),
        to: "review".to_string(),
        on: TransitionOnConfig(serde_yaml::Value::Null),
    });

    let decl = PhaseAuthorityDeclaration::try_from_config(&cfg).unwrap();
    assert_eq!(decl.transitions.len(), 2);
    assert_eq!(decl.transitions[1].from, "*");
}