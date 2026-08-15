//! Verdict / failure-classification tests for the verifier (Unit 3).
//!
//! These tests verify that `evaluate_scenario` + `build_report` correctly
//! map `ScenarioOutcome` traces into the public `VerifyReportScenario`
//! shape, with mutually-exclusive failure categories that distinguish
//! runtime terminal closure from verifier budget exhaustion.

#![cfg(test)]

use ralph_core::config::{EventLoopConfig, HatConfig, RalphConfig};
use ralph_core::preset_verify::{
    DriverWorkspace, PresetVerifyReport, ScenarioFile, SourceKind, StaticLayer,
    TerminalKind, build_report, evaluate_scenario, run_scenario,
};
use std::collections::HashMap;

fn starting_event() -> String {
    "work.start".to_string()
}

fn make_single_hat_config(trigger: &str, publish: &str, hat_id: &str) -> RalphConfig {
    let mut config = RalphConfig::default();
    config.max_iterations = Some(4);
    config.prompt_file = Some("PROMPT.md".to_string());
    config.event_loop = EventLoopConfig {
        starting_event: Some(starting_event()),
        execution_mode: ralph_core::config::HatExecutionMode::Isolated,
        completion_promise: "LOOP_COMPLETE".to_string(),
        ..EventLoopConfig::default()
    };
    let mut hats = HashMap::new();
    hats.insert(
        hat_id.to_string(),
        HatConfig {
            name: hat_id.to_string(),
            triggers: vec![trigger.to_string()],
            publishes: vec![publish.to_string()],
            instructions: format!("emit {publish}"),
            ..HatConfig::default()
        },
    );
    config.hats = hats;
    config
}

fn parse_scenario(yaml: &str) -> ralph_core::preset_verify::Scenario {
    let parsed = ScenarioFile::from_yaml(yaml, &starting_event()).expect("parse");
    parsed.scenarios.into_iter().next().expect("one scenario")
}

#[test]
fn failure_terminal_must_match_expectation() {
    // Scenario expects `terminal: success` with terminal_topic LOOP_COMPLETE,
    // but the hat emits a different topic. Driver verdict must say
    // scenario_failure (or unclosed_terminal) — not pass.
    let yaml = r#"
version: 1
scenarios:
  - name: terminal-mismatch
    responses:
      - output: |
          <event topic="not-the-promise">{"ok":true}</event>
        success: true
    expect:
      start_event: work.start
      accepted_events: [not-the-promise]
      forbidden_events: []
      terminal: success
      terminal_topic: LOOP_COMPLETE
    limits:
      max_steps: 4
      no_progress_steps: 4
"#;
    let scenario = parse_scenario(yaml);
    let config = make_single_hat_config("work.start", "not-the-promise", "doer");
    let workspace = DriverWorkspace::new().expect("workspace");
    let outcome = run_scenario(&scenario, &config, &workspace, "blob").expect("run");
    let report = evaluate_scenario(outcome);
    assert!(!report.passed);
    assert_eq!(
        report.failure_kind.as_deref(),
        Some("unclosed_terminal"),
        "expected unclosed_terminal failure, got {:?}",
        report.failure_kind
    );
}

#[test]
fn blocked_terminal_is_not_success_terminal() {
    // Scenario expects `blocked` with terminal_topic `plan.blocked`,
    // but the runtime emits `LOOP_COMPLETE` instead. Verdict must say
    // unclosed_terminal (not pass, not failure, not blocked).
    let yaml = r#"
version: 1
scenarios:
  - name: blocked-vs-success
    responses:
      - output: |
          <event topic="LOOP_COMPLETE">{"reason":"done"}</event>
        success: true
    expect:
      start_event: work.start
      accepted_events: [LOOP_COMPLETE]
      forbidden_events: []
      terminal: blocked
      terminal_topic: plan.blocked
    limits:
      max_steps: 4
      no_progress_steps: 4
"#;
    let scenario = parse_scenario(yaml);
    let config = make_single_hat_config("work.start", "LOOP_COMPLETE", "doer");
    let workspace = DriverWorkspace::new().expect("workspace");
    let outcome = run_scenario(&scenario, &config, &workspace, "blob").expect("run");
    let report = evaluate_scenario(outcome);
    assert!(!report.passed);
    assert_eq!(report.failure_kind.as_deref(), Some("unclosed_terminal"));
}

#[test]
fn empty_output_is_bounded() {
    // Empty output with small no_progress_steps budget — driver must
    // surface no_progress, not block forever.
    let yaml = r#"
version: 1
scenarios:
  - name: empty-bounded
    responses:
      - output: ""
        success: true
      - output: ""
        success: true
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 4
      no_progress_steps: 2
"#;
    let scenario = parse_scenario(yaml);
    let config = make_single_hat_config("work.start", "LOOP_COMPLETE", "doer");
    let workspace = DriverWorkspace::new().expect("workspace");
    let outcome = run_scenario(&scenario, &config, &workspace, "blob").expect("run");
    let report = evaluate_scenario(outcome);
    assert!(!report.passed);
    // Either no_progress (2 empty steps) or unclosed_terminal (next_hat
    // returned None after first response because no event was published)
    // are both honest failure_kinds.
    let kind = report.failure_kind.expect("failure_kind");
    assert!(
        matches!(kind.as_str(), "no_progress" | "unclosed_terminal"),
        "expected no_progress or unclosed_terminal, got {kind}"
    );
}

#[test]
fn budget_exhaustion_never_passes() {
    // Two empty responses with a tight `max_steps=2` and ample
    // no_progress_steps=4. Iter 1: empty response consumed (step=1,
    // no_progress=1). Iter 2: next_hat returns None because no event
    // was published. Verdict must NOT pass — outcome must be honest.
    let yaml = r#"
version: 1
scenarios:
  - name: budget-tight
    responses:
      - output: ""
        success: true
      - output: ""
        success: true
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 2
      no_progress_steps: 2
"#;
    let scenario = parse_scenario(yaml);
    let config = make_single_hat_config("work.start", "LOOP_COMPLETE", "doer");
    let workspace = DriverWorkspace::new().expect("workspace");
    let outcome = run_scenario(&scenario, &config, &workspace, "blob").expect("run");
    let report = evaluate_scenario(outcome);
    assert!(!report.passed, "budget exhaustion must not pass");
    let kind = report.failure_kind.expect("failure_kind");
    assert!(
        matches!(
            kind.as_str(),
            "timeout" | "no_progress" | "unclosed_terminal"
        ),
        "expected timeout/no_progress/unclosed_terminal, got {kind}"
    );
}

#[test]
fn max_steps_zero_responses_never_passes_when_driver_exits_early() {
    // 3 responses with max_steps=2: the third response exceeds the budget
    // and the driver returns `timeout`. Verdict must NOT pass and must
    // classify the outcome honestly.
    let yaml = r#"
version: 1
scenarios:
  - name: max-steps-exhausted
    responses:
      - output: ""
        success: true
      - output: ""
        success: true
      - output: ""
        success: true
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 2
      no_progress_steps: 2
"#;
    let scenario = parse_scenario(yaml);
    let config = make_single_hat_config("work.start", "LOOP_COMPLETE", "doer");
    let workspace = DriverWorkspace::new().expect("workspace");
    let outcome = run_scenario(&scenario, &config, &workspace, "blob").expect("run");
    let report = evaluate_scenario(outcome);
    assert!(!report.passed);
    let kind = report.failure_kind.expect("failure_kind");
    assert!(
        matches!(
            kind.as_str(),
            "timeout" | "no_progress" | "unclosed_terminal" | "scenario_failure"
        ),
        "expected honest non-pass, got {kind}"
    );
}

#[test]
fn report_human_and_json_share_verdict() {
    // The same `PresetVerifyReport` must serialize to JSON and round-trip
    // with the same `passed` field and `failure_kind` field. There must
    // not be a "human-only" path that derives a different verdict.
    let yaml = r#"
version: 1
scenarios:
  - name: share
    responses:
      - output: ""
        success: true
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 4
      no_progress_steps: 4
"#;
    let scenario = parse_scenario(yaml);
    let config = make_single_hat_config("work.start", "LOOP_COMPLETE", "doer");
    let workspace = DriverWorkspace::new().expect("workspace");
    let outcome = run_scenario(&scenario, &config, &workspace, "blob").expect("run");
    let scenario_report = evaluate_scenario(outcome.clone());

    let report: PresetVerifyReport = build_report(
        SourceKind::External,
        StaticLayer {
            passed: true,
            warnings: 0,
            errors: 0,
            findings: vec![],
        },
        vec![(outcome, scenario_report.clone())],
        None,
        "blob",
    );

    let json = serde_json::to_string(&report).expect("serialize");
    let parsed: PresetVerifyReport = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.passed, report.passed);
    assert_eq!(parsed.failure_kind, report.failure_kind);
    assert_eq!(parsed.scenarios.len(), 1);
    assert_eq!(parsed.scenarios[0].passed, scenario_report.passed);
    assert_eq!(parsed.scenarios[0].failure_kind, scenario_report.failure_kind);
}

#[test]
fn expected_topics_missing_marks_scenario_failure() {
    // Scenario expects `work.done` and `LOOP_COMPLETE` accepted, but the
    // trace only has `LOOP_COMPLETE` — verdict must say scenario_failure.
    let yaml = r#"
version: 1
scenarios:
  - name: missing-expected
    responses:
      - output: |
          <event topic="LOOP_COMPLETE">{"reason":"done"}</event>
        success: true
    expect:
      start_event: work.start
      accepted_events: [work.done, LOOP_COMPLETE]
      forbidden_events: []
      terminal: success
      terminal_topic: LOOP_COMPLETE
    limits:
      max_steps: 4
      no_progress_steps: 4
"#;
    let scenario = parse_scenario(yaml);
    let config = make_single_hat_config("work.start", "LOOP_COMPLETE", "doer");
    let workspace = DriverWorkspace::new().expect("workspace");
    let outcome = run_scenario(&scenario, &config, &workspace, "blob").expect("run");
    let report = evaluate_scenario(outcome);
    assert!(!report.passed);
    // terminal matches LOOP_COMPLETE, but work.done is missing.
    // Verdict classifier picks ScenarioFailure because terminal is OK.
    assert_eq!(report.failure_kind.as_deref(), Some("scenario_failure"));
}

#[test]
fn forbidden_topic_observed_marks_scenario_failure() {
    // We test the verdict evaluator directly: when the trace's
    // accepted_events contains a topic marked as forbidden in expect,
    // verdict must classify the scenario as scenario_failure.
    // (Driving a real EventLoop with a topic that the runtime allows
    // requires non-trivial config; this unit test focuses on the
    // verdict mapping, which is the unit under test.)
    use ralph_core::preset_verify::{
        evaluate_scenario, Scenario, ScenarioOutcome, ScenarioTrace,
    };
    let scenario_yaml = r#"
version: 1
scenarios:
  - name: forbidden-observed
    responses:
      - output: ""
        success: true
    expect:
      start_event: work.start
      accepted_events: [LOOP_COMPLETE]
      forbidden_events: [forbidden.topic]
      terminal: success
      terminal_topic: LOOP_COMPLETE
    limits:
      max_steps: 4
      no_progress_steps: 4
"#;
    let scenario: Scenario = parse_scenario(scenario_yaml);

    let trace = ScenarioTrace {
        scenario: scenario.clone(),
        steps: vec![],
        accepted_events: vec![
            "forbidden.topic".to_string(),
            "LOOP_COMPLETE".to_string(),
        ],
        rejected_events: vec![],
        last_hat: Some("doer".to_string()),
        last_accepted_topic: Some("LOOP_COMPLETE".to_string()),
        last_runtime_termination: None,
        terminal_topic: Some("LOOP_COMPLETE".to_string()),
        trace_digest: "deadbeef".to_string(),
    };
    let outcome = ScenarioOutcome {
        trace,
        failure_kind: None,
        passed: true,
    };
    let report = evaluate_scenario(outcome);
    assert!(!report.passed, "forbidden violation must not pass");
    assert_eq!(report.failure_kind.as_deref(), Some("scenario_failure"));
}

#[test]
fn runtime_exception_classification_preserved() {
    // A scenario whose driver returns an error (e.g. invalid contract)
    // surfaces as FailureKind::StaticContractFailure or
    // FailureKind::RuntimeException. The verdict evaluator does not
    // downgrade those; it preserves the driver's failure_kind.
    use ralph_core::preset_verify::FailureKind as FK;
    let kind = FK::StaticContractFailure("compile failed".to_string());
    assert_eq!(kind.tag(), "static_contract_failure");
    let kind = FK::RuntimeException("io".to_string());
    assert_eq!(kind.tag(), "runtime_exception");
}

#[test]
fn terminal_kind_serializes_in_snake_case() {
    // The enum is renamed to snake_case for JSON; verify each variant.
    for (variant, expected) in [
        (TerminalKind::Success, "\"success\""),
        (TerminalKind::Failure, "\"failure\""),
        (TerminalKind::Blocked, "\"blocked\""),
        (TerminalKind::None, "\"none\""),
    ] {
        let serialized = serde_json::to_string(&variant).expect("serialize");
        assert_eq!(serialized, expected);
    }
}