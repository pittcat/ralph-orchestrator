use super::*;
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
        findings.iter().map(|f| (f.id, &f.message)).collect::<Vec<_>>()
    );
}

#[test]
fn flow_declaration_lint_fires_when_mechanism_missing() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    schemas:
      work.ready:
        required_fields: []
"#;
    let findings = check_flow_declaration(yaml).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, FINDING_FLOW_DECLARATION_MISSING);
}

#[test]
fn flow_declaration_lint_fires_on_partial_state_undeclared() {
    let yaml = r#"
mechanism:
  flow:
    type: declared
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        terminal_when: all_done
        allowed_emits: [work.ready]
"#;
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
    let yaml = r#"
mechanism:
  flow:
    type: declared
    terminal_emits: [REPORT_DONE]
    steps:
      - id: unit_loop
        allowed_emits: [work.ready]
"#;
    let findings = check_flow_declaration(yaml).unwrap();
    let ids: Vec<_> = findings.iter().map(|f| f.id).collect();
    assert!(ids.contains(&FINDING_FLOW_TERMINAL_EMIT_MISSING));
}

#[test]
fn flow_declaration_lint_fires_on_unknown_allowed_emit() {
    let yaml = r#"
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
"#;
    let findings = check_flow_declaration(yaml).unwrap();
    let ids: Vec<_> = findings.iter().map(|f| f.id).collect();
    assert!(ids.contains(&FINDING_FLOW_UNKNOWN_EMIT_REJECTED));
}

#[test]
fn flow_declaration_lint_finds_multiple_findings_independently() {
    let yaml = r#"
mechanism:
  flow:
    type: declared
    terminal_emits: []
    steps:
      - id: unit_loop
        terminal_when: all_done
        allowed_emits: [bogus.topic]
"#;
    let findings = check_flow_declaration(yaml).unwrap();
    let ids: std::collections::HashSet<&str> = findings.iter().map(|f| f.id).collect();
    assert!(ids.contains(FINDING_FLOW_PARTIAL_STATE_UNDECLARED));
    assert!(ids.contains(FINDING_FLOW_TERMINAL_EMIT_MISSING));
    assert!(ids.contains(FINDING_FLOW_UNKNOWN_EMIT_REJECTED));
    assert_eq!(findings.len(), 3);
}

#[test]
fn collect_known_topics_reads_from_event_policy_schemas() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    schemas:
      work.ready:
        required_fields: []
"#;
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