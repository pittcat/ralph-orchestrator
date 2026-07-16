use super::*;
use crate::preset_lint::finding_id::FINDING_FLOW_REVIEW_COMPLETE_IN_UNIT_LOOP_BODY;
use serde_yaml::Value;

const LEGAL_YAML: &str = r#"
event_loop:
  event_policy:
    enabled: true
    schemas:
      work.ready:
        required_fields: []
      work.done:
        required_fields: [task_id]
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        kind: foreach
        allowed_emits: [work.ready, work.done]
        terminal_when: all_done
        on_partial:
          all_done: plan.complete
          any_failed: plan.blocked(reason="unit_failed")
          partial_units_done: plan.blocked(reason="4_of_8_partial")
      - id: ship
        kind: sequence
        allowed_emits: [REPORT_DONE, LOOP_COMPLETE]
"#;

#[test]
fn flow_declaration_lint_passes_on_legal_yaml() {
    let findings = check_flow_declaration(LEGAL_YAML).unwrap();
    assert!(
        findings.is_empty(),
        "expected no findings, got {:?}",
        findings
            .iter()
            .map(|f| (f.id, &f.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn flow_declaration_lint_fires_when_mechanism_missing() {
    let yaml = r"
event_loop:
  event_policy:
    enabled: true
    schemas:
      work.ready:
        required_fields: []
";
    let findings = check_flow_declaration(yaml).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, FINDING_FLOW_DECLARATION_MISSING);
}

#[test]
fn flow_declaration_lint_fires_on_partial_state_undeclared() {
    let yaml = r"
mechanism:
  flow:
    type: declared
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        terminal_when: all_done
        allowed_emits: [work.ready]
";
    let findings = check_flow_declaration(yaml).unwrap();
    let ids: Vec<_> = findings.iter().map(|f| f.id).collect();
    assert!(ids.contains(&FINDING_FLOW_PARTIAL_STATE_UNDECLARED));
}

#[test]
fn flow_declaration_lint_fires_on_empty_partial_branch() {
    let yaml = r#"
mechanism:
  flow:
    type: declared
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        terminal_when: all_done
        allowed_emits: [work.ready]
        on_partial:
          all_done: ""
"#;
    let findings = check_flow_declaration(yaml).unwrap();
    let ids: Vec<_> = findings.iter().map(|f| f.id).collect();
    assert!(ids.contains(&FINDING_FLOW_PARTIAL_BRANCH_EMPTY));
}

#[test]
fn flow_declaration_lint_fires_when_terminal_emits_lacks_loop_complete() {
    let yaml = r"
mechanism:
  flow:
    type: declared
    terminal_emits: [REPORT_DONE]
    steps:
      - id: unit_loop
        allowed_emits: [work.ready]
";
    let findings = check_flow_declaration(yaml).unwrap();
    let ids: Vec<_> = findings.iter().map(|f| f.id).collect();
    assert!(ids.contains(&FINDING_FLOW_TERMINAL_EMIT_MISSING));
}

#[test]
fn flow_declaration_lint_fires_on_unknown_allowed_emit() {
    let yaml = r"
event_loop:
  event_policy:
    enabled: true
    schemas:
      work.ready:
        required_fields: []
mechanism:
  flow:
    type: declared
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        allowed_emits: [bogus.topic]
";
    let findings = check_flow_declaration(yaml).unwrap();
    let ids: Vec<_> = findings.iter().map(|f| f.id).collect();
    assert!(ids.contains(&FINDING_FLOW_UNKNOWN_EMIT_REJECTED));
}

#[test]
fn flow_declaration_lint_finds_multiple_findings_independently() {
    let yaml = r"
mechanism:
  flow:
    type: declared
    terminal_emits: []
    steps:
      - id: unit_loop
        terminal_when: all_done
        allowed_emits: [bogus.topic]
";
    let findings = check_flow_declaration(yaml).unwrap();
    let ids: std::collections::HashSet<&str> = findings.iter().map(|f| f.id).collect();
    assert!(ids.contains(FINDING_FLOW_PARTIAL_STATE_UNDECLARED));
    assert!(ids.contains(FINDING_FLOW_TERMINAL_EMIT_MISSING));
    assert!(ids.contains(FINDING_FLOW_UNKNOWN_EMIT_REJECTED));
    assert_eq!(findings.len(), 3);
}

#[test]
fn collect_known_topics_reads_from_event_policy_schemas() {
    let yaml = r"
event_loop:
  event_policy:
    enabled: true
    schemas:
      work.ready:
        required_fields: []
";
    let topics = collect_known_topics(yaml);
    assert!(topics.contains("work.ready"));
    assert!(topics.contains("LOOP_COMPLETE"));
}

#[test]
fn raw_yaml_value_parses_round_trip() {
    // The lint must accept raw YAML; this test ensures the
    // helper that maps raw_yaml -> Value still works.
    let v: Value = serde_yaml::from_str(LEGAL_YAML).unwrap();
    assert!(v.get("mechanism").is_some());
}

#[test]
fn flow_declaration_review_complete_in_review_walk_body_passes() {
    // Per `ce-executor-serial.yml:108-112`, `review.complete` is
    // the terminal topic of the per-plan `review_walk` step
    // (after all units are done). That is the CORRECT location.
    // The lint must stay silent.
    let yaml = r"
event_loop:
  event_policy:
    enabled: true
    schemas:
      review.complete:
        required_fields: []
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        kind: foreach
        allowed_emits: [work.ready, work.done]
        body:
          - work.ready
          - work.done
      - id: review_walk
        kind: sequence
        allowed_emits: [review.dimension.ready, review.dimensions.complete, review.complete]
        body:
          - review.dimension.ready
          - review.dimensions.complete
          - review.complete
";
    let findings = check_flow_declaration(yaml).unwrap();
    let review_complete_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.id == FINDING_FLOW_REVIEW_COMPLETE_IN_UNIT_LOOP_BODY)
        .collect();
    assert!(
        review_complete_findings.is_empty(),
        "review.complete in review_walk.body must NOT trigger the U8 guard; got {:?}",
        findings
    );
}

#[test]
fn flow_declaration_review_complete_in_unit_loop_body_fails() {
    // The anti-pattern: `review.complete` in `unit_loop.body`.
    // The unit_loop is `foreach over plan units`; review.complete
    // only fires after all units are done via the review_walk
    // step. The U8 guard MUST fire here.
    let yaml = r"
event_loop:
  event_policy:
    enabled: true
    schemas:
      review.complete:
        required_fields: []
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        kind: foreach
        allowed_emits: [work.ready, work.done]
        body:
          - work.ready
          - work.done
          - review.complete
";
    let findings = check_flow_declaration(yaml).unwrap();
    let review_complete_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.id == FINDING_FLOW_REVIEW_COMPLETE_IN_UNIT_LOOP_BODY)
        .collect();
    assert_eq!(
        review_complete_findings.len(),
        1,
        "expected exactly 1 U8 finding; got {:?}",
        findings
    );
    let f = review_complete_findings[0];
    assert!(f.message.contains("review.complete"));
    assert!(f.message.contains("unit_loop"));
    let hint = f.action_hint.as_ref().expect("action_hint must be Some");
    assert!(hint.contains("review_walk"));
    assert_eq!(f.hat.as_deref(), Some("unit_loop"));
}

#[test]
fn flow_declaration_review_complete_subtopic_in_unit_loop_body_fails() {
    // A future topic family like `review.complete.something`
    // must also be rejected so the anti-pattern cannot be
    // smuggled in via a different name.
    let yaml = r"
event_loop:
  event_policy:
    enabled: true
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        kind: foreach
        allowed_emits: [work.done]
        body:
          - work.done
          - review.complete.summary
";
    let findings = check_flow_declaration(yaml).unwrap();
    let review_complete_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.id == FINDING_FLOW_REVIEW_COMPLETE_IN_UNIT_LOOP_BODY)
        .collect();
    assert_eq!(
        review_complete_findings.len(),
        1,
        "review.complete.* subtopics must also trigger the U8 guard; got {:?}",
        findings
    );
}

#[test]
fn flow_declaration_no_unit_loop_step_is_silent() {
    // Presets without a `unit_loop` step (e.g.
    // `ce-executor-pipeline` is a linear hat-only pipeline)
    // are out of scope for the U8 guard.
    let yaml = r"
event_loop:
  event_policy:
    enabled: true
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: ship
        kind: sequence
        allowed_emits: [REPORT_DONE, LOOP_COMPLETE]
";
    let findings = check_flow_declaration(yaml).unwrap();
    let review_complete_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.id == FINDING_FLOW_REVIEW_COMPLETE_IN_UNIT_LOOP_BODY)
        .collect();
    assert!(
        review_complete_findings.is_empty(),
        "non-unit_loop presets must NOT trigger U8; got {:?}",
        findings
    );
}
