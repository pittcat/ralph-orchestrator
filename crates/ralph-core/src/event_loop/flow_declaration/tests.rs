use super::{FlowDeclaration, FlowParseError, is_partial_state};

const CE_EXECUTOR_SERIAL_FLOW: &str = r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: unit_loop
        kind: foreach
        allowed_emits:
          - work.ready
          - work.done
          - work.failed
        terminal_when: all_done
      - id: review_walk
        kind: sequence
        allowed_emits:
          - review.start
          - review.complete
        emit_when: unit_loop.terminal == all_done
      - id: plan_end
        kind: branch
        allowed_emits:
          - plan.complete
          - plan.blocked
        on_partial:
          all_done: plan.complete
          any_failed: plan.blocked(reason="unit_failed")
          partial_units_done: plan.blocked(reason="4_of_8_partial")
      - id: ship
        kind: sequence
        allowed_emits:
          - REPORT_DONE
          - LOOP_COMPLETE
    repair_budget: 3
    enforce_schema: hard
    state_idempotency: required
"#;

#[test]
fn flow_declaration_parses_minimal_legal_flow() {
    let yaml = r"
mechanism:
  flow:
    type: declared
    steps:
      - id: unit_loop
        allowed_emits: [work.ready, work.done]
";
    let decl = FlowDeclaration::from_yaml(yaml).unwrap();
    assert_eq!(decl.steps.len(), 1);
    assert_eq!(decl.steps[0].id, "unit_loop");
    assert!(decl.allows("unit_loop", "work.ready"));
    assert!(!decl.allows("unit_loop", "plan.blocked"));
}

#[test]
fn flow_declaration_parses_full_ce_executor_serial_flow() {
    let decl = FlowDeclaration::from_yaml(CE_EXECUTOR_SERIAL_FLOW).unwrap();
    assert_eq!(decl.terminal_emits, vec!["LOOP_COMPLETE".to_string()]);
    assert_eq!(decl.steps.len(), 4);
    assert_eq!(decl.steps[0].id, "unit_loop");
    assert_eq!(decl.steps[1].id, "review_walk");
    assert_eq!(decl.steps[2].id, "plan_end");
    assert_eq!(decl.steps[3].id, "ship");
    assert_eq!(decl.repair_budget, 3);
    assert_eq!(decl.enforce_schema, "hard");
    assert_eq!(decl.state_idempotency, "required");
}

#[test]
fn flow_declaration_rejects_missing_mechanism_flow() {
    let yaml = r"some_other_key: true";
    let err = FlowDeclaration::from_yaml(yaml).unwrap_err();
    assert!(matches!(err, FlowParseError::MissingMechanismFlow));
}

#[test]
fn flow_declaration_rejects_unsupported_type() {
    let yaml = r"
mechanism:
  flow:
    type: inferred
    steps: []
";
    let err = FlowDeclaration::from_yaml(yaml).unwrap_err();
    assert!(matches!(err, FlowParseError::UnsupportedFlowType(ref t) if t == "inferred"));
}

#[test]
fn flow_declaration_rejects_soft_enforce_schema() {
    let yaml = r"
mechanism:
  flow:
    type: declared
    enforce_schema: soft
    steps: []
";
    let err = FlowDeclaration::from_yaml(yaml).unwrap_err();
    assert!(matches!(
        err,
        FlowParseError::UnsupportedEnforceSchema(ref t) if t == "soft"
    ));
}

#[test]
fn flow_declaration_rejects_optional_state_idempotency() {
    let yaml = r"
mechanism:
  flow:
    type: declared
    state_idempotency: optional
    steps: []
";
    let err = FlowDeclaration::from_yaml(yaml).unwrap_err();
    assert!(matches!(
        err,
        FlowParseError::UnsupportedStateIdempotency(ref t) if t == "optional"
    ));
}

#[test]
fn flow_declaration_rejects_duplicate_step_ids() {
    let yaml = r"
mechanism:
  flow:
    type: declared
    steps:
      - id: unit_loop
        allowed_emits: [work.ready]
      - id: unit_loop
        allowed_emits: [work.done]
";
    let err = FlowDeclaration::from_yaml(yaml).unwrap_err();
    assert!(matches!(err, FlowParseError::DuplicateStepId(ref s) if s == "unit_loop"));
}

#[test]
fn is_partial_state_matches_three_terminal_when_values() {
    assert!(is_partial_state("all_done"));
    assert!(is_partial_state("any_failed"));
    assert!(is_partial_state("partial_units_done"));
    assert!(!is_partial_state("all_units_done"));
    assert!(!is_partial_state("not_a_real_value"));
    assert!(!is_partial_state(""));
}

#[test]
fn flow_declaration_allows_topic_returns_false_for_unknown_step() {
    let decl = FlowDeclaration::from_yaml(CE_EXECUTOR_SERIAL_FLOW).unwrap();
    assert!(!decl.allows("does_not_exist", "work.ready"));
    assert!(!decl.allows("unit_loop", "plan.complete"));
}
