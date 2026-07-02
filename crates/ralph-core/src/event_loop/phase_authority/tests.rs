//! 2026-07-02-006 plan U1: round-trip pins for
//! `PhaseAuthorityConfig`. Only this module's tests are exercised
//! by the U1 entry point
//! `cargo nextest run -p ralph-core -- phase_authority_config_roundtrip`.

use super::config::*;

const MINIMAL_YAML: &str = r#"
mechanism:
  phase_authority:
    enabled: true
    initial_phase: unit_loop
    phases:
      - id: unit_loop
        allowed_emits:
          coordinator: [work.ready]
"#;

const ENABLED_FALSE_YAML: &str = r#"
mechanism:
  phase_authority:
    enabled: false
"#;

#[test]
fn phase_authority_config_roundtrip_minimal_yaml() {
    let outer: serde_yaml::Value = serde_yaml::from_str(MINIMAL_YAML).unwrap();
    let mech = outer.get("mechanism").unwrap();
    let phase = mech.get("phase_authority").unwrap();
    let cfg: PhaseAuthorityConfig = serde_yaml::from_value(phase.clone()).unwrap();

    assert!(cfg.enabled);
    assert_eq!(cfg.initial_phase.as_deref(), Some("unit_loop"));
    assert_eq!(cfg.phases.len(), 1);
    assert_eq!(cfg.phases[0].id, "unit_loop");
    assert_eq!(
        cfg.phases[0].allowed_emits.get("coordinator"),
        Some(&vec!["work.ready".to_string()])
    );

    // Re-serialize and re-parse must produce equal structs.
    // Round-trip the cfg directly (the surrounding
    // `mechanism.phase_authority` wrapping is consumed by
    // `RalphConfig`'s `mechanism` field, not by this struct).
    let re_yaml = serde_yaml::to_string(&cfg).unwrap();
    let re_cfg: PhaseAuthorityConfig = serde_yaml::from_str(&re_yaml).unwrap();
    assert_eq!(cfg, re_cfg);
}

#[test]
fn phase_authority_config_enabled_false_parses() {
    let outer: serde_yaml::Value = serde_yaml::from_str(ENABLED_FALSE_YAML).unwrap();
    let mech = outer.get("mechanism").unwrap();
    let phase = mech.get("phase_authority").unwrap();
    let cfg: PhaseAuthorityConfig = serde_yaml::from_value(phase.clone()).unwrap();

    assert!(!cfg.enabled);
    assert_eq!(cfg.initial_phase, None);
    assert!(cfg.phases.is_empty());
    assert!(cfg.transitions.is_empty());
}

#[test]
fn phase_authority_config_violation_policy_defaults_when_absent() {
    let cfg = PhaseAuthorityConfig::default();
    assert_eq!(cfg.violation_policy.max_resume_per_hat, 3);
    assert_eq!(cfg.violation_policy.on_exhausted, "plan_blocked");
    assert!(!cfg.enabled);
    assert_eq!(cfg.initial_phase, None);
    assert!(cfg.phases.is_empty());
    assert!(cfg.transitions.is_empty());
    assert!(cfg.progress_projection.on_enter.is_empty());
}

#[test]
fn phase_authority_config_explicit_violation_policy_roundtrip() {
    let yaml = r#"
mechanism:
  phase_authority:
    enabled: true
    violation_policy:
      max_resume_per_hat: 7
      on_exhausted: silent_drop
"#;
    let outer: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
    let mech = outer.get("mechanism").unwrap();
    let phase = mech.get("phase_authority").unwrap();
    let cfg: PhaseAuthorityConfig = serde_yaml::from_value(phase.clone()).unwrap();

    assert_eq!(cfg.violation_policy.max_resume_per_hat, 7);
    assert_eq!(cfg.violation_policy.on_exhausted, "silent_drop");

    let re_yaml = serde_yaml::to_string(&cfg).unwrap();
    let re_cfg: PhaseAuthorityConfig = serde_yaml::from_str(&re_yaml).unwrap();
    assert_eq!(cfg, re_cfg);
}