use super::*;
use crate::preset_lint::finding_id::{
    FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY, FINDING_FLOW_REVIEW_COMPLETE_IN_UNIT_LOOP_BODY,
};
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

// 2026-07-28-001 plan U4 (R8 / S8): the new
// `flow_linear_positional_ambiguity` lint catches a non-final
// `kind: linear` step that has multiple allowed emits but no
// forward step's `on` / `on_any_of` names any of them. This
// shape is the precondition for the runtime's positional
// fallback to silently misroute the flow.

/// U4 invalid shape: non-final linear step with multiple emits
/// and no forward `on` target names any of them.
#[test]
fn flow_linear_positional_ambiguity_fires_on_legacy_parallel_forge_shape() {
    // The legacy 5-step parallel-forge flow (pre-§3.1) had a
    // `planning` step that bundled five topics (inspected / ready /
    // concurrency_approved / worktrees_ready / plan.blocked) with
    // no forward `on` declarations — exactly the anti-pattern the
    // lint targets.
    let yaml = r"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: planning
        kind: linear
        allowed_emits:
          - forge.plan.inspected
          - forge.plan.ready
          - forge.concurrency.approved
          - forge.worktrees.ready
          - forge.plan.blocked
      - id: exec_wave
        kind: side_effect
        allowed_emits:
          - exec.wave.complete
          - exec.wave.failed
          - exec.unit.ready
          - exec.unit.done
          - exec.unit.failed
      - id: plan_end
        kind: terminal
        allowed_emits:
          - LOOP_COMPLETE
";
    let findings = check_flow_declaration(yaml).unwrap();
    let positional_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.id == FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY)
        .collect();
    assert_eq!(
        positional_findings.len(),
        1,
        "legacy multi-topic linear step must trigger the U4 finding; got {:?}",
        findings
    );
    let f = positional_findings[0];
    assert!(f.message.contains("planning"));
    assert!(f.message.contains("positional advance"));
    assert!(f.message.contains("forge.plan.inspected"));
    let hint = f.action_hint.as_ref().expect("action_hint must be Some");
    assert!(hint.contains("on"));
    assert_eq!(f.hat.as_deref(), Some("planning"));
}

/// U4 exemption: linear step with `on` target — no finding.
#[test]
fn flow_linear_positional_ambiguity_silent_with_on_target() {
    let yaml = r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: planning
        kind: linear
        allowed_emits:
          - forge.plan.inspected
      - id: plan_authoring
        kind: linear
        "on": forge.plan.inspected
        allowed_emits:
          - forge.plan.ready
      - id: plan_end
        kind: terminal
        allowed_emits:
          - LOOP_COMPLETE
"#;
    let findings = check_flow_declaration(yaml).unwrap();
    let positional_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.id == FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY)
        .collect();
    assert!(
        positional_findings.is_empty(),
        "explicit `on` must silence the U4 finding; got {:?}",
        findings
    );
}

/// U4 exemption: linear step with `on_any_of` branch — no finding.
#[test]
fn flow_linear_positional_ambiguity_silent_with_on_any_of_branch() {
    let yaml = r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: audit
        kind: linear
        allowed_emits:
          - forge.audit.done
          - forge.plan.blocked
      - id: report
        kind: await
        on_any_of:
          - forge.audit.done
          - forge.plan.blocked
        allowed_emits:
          - forge.report.done
      - id: plan_end
        kind: terminal
        allowed_emits:
          - LOOP_COMPLETE
"#;
    let findings = check_flow_declaration(yaml).unwrap();
    let positional_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.id == FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY)
        .collect();
    assert!(
        positional_findings.is_empty(),
        "explicit `on_any_of` must silence the U4 finding; got {:?}",
        findings
    );
}

/// U4 exemption: single-topic linear step — no finding.
#[test]
fn flow_linear_positional_ambiguity_silent_for_single_topic_linear() {
    let yaml = r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: planning
        kind: linear
        allowed_emits:
          - forge.plan.inspected
      - id: plan_authoring
        kind: linear
        "on": forge.plan.inspected
        allowed_emits:
          - forge.plan.ready
      - id: plan_end
        kind: terminal
        allowed_emits:
          - LOOP_COMPLETE
"#;
    let findings = check_flow_declaration(yaml).unwrap();
    let positional_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.id == FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY)
        .collect();
    assert!(
        positional_findings.is_empty(),
        "single-topic linear step must NOT trigger U4; got {:?}",
        findings
    );
}

/// U4 exemption: last step is exempt even if it has multiple emits.
#[test]
fn flow_linear_positional_ambiguity_silent_for_last_step() {
    // Last step is exempt — the rule is about non-final steps
    // whose runtime would try to advance past them. Here the
    // last step is a `kind: linear` step with multiple allowed
    // emits but no successor, so the lint must stay silent.
    let yaml = r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: planning
        kind: linear
        allowed_emits:
          - forge.plan.inspected
      - id: plan_end
        kind: linear
        allowed_emits:
          - forge.audit.done
          - forge.plan.blocked
          - forge.report.done
"#;
    let findings = check_flow_declaration(yaml).unwrap();
    let positional_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.id == FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY)
        .collect();
    assert!(
        positional_findings.is_empty(),
        "last step must NOT trigger U4 (no successor to advance to); got {:?}",
        findings
    );
}

/// U4 exemption: non-linear kinds (side_effect, await, foreach,
/// sequence, terminal) bypass the rule.
#[test]
fn flow_linear_positional_ambiguity_silent_for_non_linear_kinds() {
    let yaml = r"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: exec_wave
        kind: side_effect
        allowed_emits:
          - exec.unit.ready
          - exec.unit.done
          - exec.unit.failed
      - id: exec_finalize
        kind: await
        allowed_emits:
          - forge.exec.development.done
      - id: plan_end
        kind: terminal
        allowed_emits:
          - LOOP_COMPLETE
";
    let findings = check_flow_declaration(yaml).unwrap();
    let positional_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.id == FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY)
        .collect();
    assert!(
        positional_findings.is_empty(),
        "non-linear steps must NOT trigger U4; got {:?}",
        findings
    );
}

/// U4 exemption: `work.failed` and `work.done` are non-transition
/// topics — the runtime never advances on them, so their absence
/// from a forward `on` is not ambiguous.
#[test]
fn flow_linear_positional_ambiguity_ignores_non_transition_topics() {
    // A failure-capable linear step that allows `work.failed`
    // alongside a transition topic — the lint must NOT flag
    // `work.failed` as ambiguous since it is a non-transition.
    let yaml = r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: integration
        kind: linear
        allowed_emits:
          - forge.integration.done
          - work.failed
          - forge.report.done
      - id: incremental_verify
        kind: linear
        "on": forge.integration.done
        allowed_emits:
          - forge.incremental.verified
          - work.failed
          - forge.report.done
      - id: full_verify
        kind: linear
        "on": forge.incremental.verified
        allowed_emits:
          - forge.full.verified
          - work.failed
          - forge.report.done
      - id: audit
        kind: linear
        "on": forge.full.verified
        allowed_emits:
          - forge.audit.done
      - id: report
        kind: await
        "on": forge.audit.done
        allowed_emits:
          - forge.report.done
      - id: plan_end
        kind: terminal
        "on": forge.report.done
        allowed_emits:
          - LOOP_COMPLETE
"#;
    let findings = check_flow_declaration(yaml).unwrap();
    let positional_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.id == FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY)
        .collect();
    assert!(
        positional_findings.is_empty(),
        "non-transition topics (work.failed) must not trigger U4; got {:?}",
        findings
    );
}

/// U4: only transition-capable topics trigger the finding.
#[test]
fn flow_linear_positional_ambiguity_fires_on_transition_topic_only() {
    // integration step has 3 allowed emits. Two of them
    // (`work.failed`, `forge.report.done`) are non-transition /
    // non-ambiguous. The remaining one (`forge.integration.done`)
    // is transition-capable. Here we drop the `on: forge.integration.done`
    // declaration on incremental_verify so the U4 finding fires on
    // the transition topic only — the non-transition topics
    // (`work.failed`, `forge.report.done`) are correctly NOT
    // surfaced in the finding's topic list.
    let yaml = r#"
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: [LOOP_COMPLETE]
    steps:
      - id: integration
        kind: linear
        allowed_emits:
          - forge.integration.done
          - work.failed
          - forge.report.done
      - id: incremental_verify
        kind: linear
        allowed_emits:
          - forge.incremental.verified
          - work.failed
          - forge.report.done
      - id: full_verify
        kind: linear
        "on": forge.incremental.verified
        allowed_emits:
          - forge.full.verified
          - work.failed
          - forge.report.done
      - id: audit
        kind: linear
        "on": forge.full.verified
        allowed_emits:
          - forge.audit.done
      - id: report
        kind: await
        "on": forge.audit.done
        allowed_emits:
          - forge.report.done
      - id: plan_end
        kind: terminal
        "on": forge.report.done
        allowed_emits:
          - LOOP_COMPLETE
"#;
    let findings = check_flow_declaration(yaml).unwrap();
    let positional_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.id == FINDING_FLOW_LINEAR_POSITIONAL_AMBIGUITY)
        .collect();
    assert_eq!(
        positional_findings.len(),
        1,
        "forge.integration.done ambiguity must trigger U4; got {:?}",
        findings
    );
    let f = positional_findings[0];
    assert!(f.message.contains("integration"));
    assert!(f.message.contains("forge.integration.done"));
    // The non-transition topics (`work.failed`, `forge.report.done`)
    // are in the message's `topics_str` (all allowed emits) but
    // NOT in the ambiguous topics list. Verify the action_hint
    // only surfaces the transition-capable topic.
    let hint = f.action_hint.as_ref().expect("action_hint must be Some");
    assert!(hint.contains("forge.integration.done"));
    assert!(
        !hint.contains("work.failed"),
        "non-transition topic must not appear in the action_hint; got: {hint}"
    );
    assert!(
        !hint.contains("forge.report.done"),
        "forge.report.done is reachable via on_any_of on report step; must not appear in ambiguous list"
    );
}
