//! 2026-07-02-006 plan U3: tests for
//! `check_phase_authority_block`.
//!
//! Test entry point:
//! `cargo nextest run -p ralph-core -- preset_lint_phase_authority`.

use super::phase_authority::*;
use crate::preset_lint::finding_id::{
    FINDING_PHASE_AUTHORITY_EMPTY, FINDING_PHASE_AUTHORITY_PIPELINE_NOT_ALLOWED,
    FINDING_PHASE_AUTHORITY_UNKNOWN_PRIMITIVE,
};

#[test]
fn unknown_primitive_produces_finding() {
    let yaml = r#"
mechanism:
  phase_authority:
    enabled: true
    initial_phase: unit_loop
    phases:
      - id: unit_loop
      - id: review
    transitions:
      - from: unit_loop
        to: review
        on:
          primitive: on_magical_step
"#;
    let findings = check_phase_authority_block(yaml);
    assert!(
        findings
            .iter()
            .any(|f| f.id == FINDING_PHASE_AUTHORITY_UNKNOWN_PRIMITIVE),
        "expected unknown primitive finding, got {:?}",
        findings
    );
}

#[test]
fn pipeline_style_preset_with_phase_authority_produces_finding() {
    // Hat-only pipeline (no `tasks.coordinator_hats` and no
    // `event_loop.tasks.coordinator_hats`) that nonetheless
    // declares `phase_authority` should be flagged.
    let yaml = r#"
hats:
  executor:
    publishes: [work.done]
  fixer:
    publishes: [fix.done]
mechanism:
  phase_authority:
    enabled: true
    initial_phase: work
    phases:
      - id: work
"#;
    let findings = check_phase_authority_block(yaml);
    assert!(
        findings
            .iter()
            .any(|f| f.id == FINDING_PHASE_AUTHORITY_PIPELINE_NOT_ALLOWED),
        "expected pipeline-not-allowed finding, got {:?}",
        findings
    );
}

#[test]
fn empty_enabled_phase_authority_produces_empty_finding() {
    let yaml = r#"
mechanism:
  phase_authority:
    enabled: true
"#;
    let findings = check_phase_authority_block(yaml);
    let ids: Vec<&'static str> = findings.iter().map(|f| f.id).collect();
    assert!(ids.contains(&FINDING_PHASE_AUTHORITY_EMPTY));
}

#[test]
fn disabled_phase_authority_is_silent() {
    let yaml = r#"
mechanism:
  phase_authority:
    enabled: false
"#;
    let findings = check_phase_authority_block(yaml);
    assert!(
        findings.is_empty(),
        "disabled phase_authority must be silent, got {:?}",
        findings
    );
}

#[test]
fn absent_phase_authority_is_silent() {
    let yaml = r#"
hats:
  coordinator:
    publishes: [plan.complete]
"#;
    let findings = check_phase_authority_block(yaml);
    assert!(
        findings.is_empty(),
        "absent phase_authority must be silent, got {:?}",
        findings
    );
}

#[test]
fn known_primitive_does_not_fire_unknown() {
    let yaml = r#"
mechanism:
  phase_authority:
    enabled: true
    initial_phase: unit_loop
    phases:
      - id: unit_loop
      - id: review
    transitions:
      - from: unit_loop
        to: review
        on:
          primitive: on_event
          event: work.start
"#;
    let findings = check_phase_authority_block(yaml);
    assert!(
        !findings
            .iter()
            .any(|f| f.id == FINDING_PHASE_AUTHORITY_UNKNOWN_PRIMITIVE),
        "known primitive must not fire unknown, got {:?}",
        findings
    );
}

#[test]
fn known_primitives_whitelist_includes_u6_u9() {
    // Pin the engine-known set so the lint cannot drift
    // unnoticed when U6–U9 land.
    assert!(KNOWN_PRIMITIVES.contains(&"on_event"));
    assert!(KNOWN_PRIMITIVES.contains(&"on_test_passed_step"));
    assert!(KNOWN_PRIMITIVES.contains(&"on_review_complete_verdict"));
    assert!(KNOWN_PRIMITIVES.contains(&"on_loop_complete_honored"));
}
