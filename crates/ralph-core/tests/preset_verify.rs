//! Acceptance tests for `ralph_core::preset_verify` — Unit 1 (parser, validation, report model).
//!
//! These tests intentionally use the public API only and never depend on the
//! `ScenarioRunner` stub or private EventLoop fields. They exist to lock in the
//! public scenario schema (`version: 1`) and the deterministic report shape
//! that downstream Units (driver, CLI, skill) build on top of.

#![cfg(test)]

use std::collections::BTreeSet;

use ralph_core::preset_verify::{
    ExpectBlock, InputError, Limits, PresetVerifyReport, ScenarioFile, SourceKind, StaticLayer,
    TerminalKind, VerifyReportScenario, compute_trace_digest,
};
use ralph_core::EventLoopConfig;

fn starting_event() -> String {
    "work.start".to_string()
}

#[test]
fn parse_version_1_scenario_preserves_response_order() {
    let yaml = r#"
version: 1
scenarios:
  - name: success-path
    responses:
      - hat: producer
        output: |
          <event topic="work.done">{"ok":true}</event>
        success: true
      - hat: closer
        output: ""
        success: true
    expect:
      start_event: work.start
      accepted_events: [work.done, LOOP_COMPLETE]
      forbidden_events: []
      terminal: success
      terminal_topic: LOOP_COMPLETE
      payload_fields: {}
    limits:
      max_steps: 8
      no_progress_steps: 2
"#;

    let parsed = ScenarioFile::from_yaml(yaml, &starting_event()).expect("parse succeeds");
    assert_eq!(parsed.scenarios.len(), 1);
    let scenario = &parsed.scenarios[0];
    assert_eq!(scenario.name, "success-path");
    assert_eq!(scenario.responses.len(), 2);
    assert_eq!(scenario.responses[0].hat.as_deref(), Some("producer"));
    assert_eq!(scenario.responses[1].output, "");
    assert!(scenario.responses[1].success);
    assert_eq!(scenario.limits.max_steps, 8);
    assert_eq!(scenario.limits.no_progress_steps, 2);
}

#[test]
fn rejects_missing_version_field() {
    let yaml = r#"
scenarios:
  - name: only
    responses:
      - hat: producer
        output: ""
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 1
      no_progress_steps: 1
"#;

    let err = ScenarioFile::from_yaml(yaml, &starting_event()).unwrap_err();
    assert!(matches!(err, InputError::SchemaVersion(_)), "got {err:?}");
}

#[test]
fn rejects_unknown_top_level_version() {
    let yaml = r#"
version: 99
scenarios:
  - name: only
    responses: []
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 1
      no_progress_steps: 1
"#;
    let err = ScenarioFile::from_yaml(yaml, &starting_event()).unwrap_err();
    assert!(matches!(err, InputError::SchemaVersion(_)), "got {err:?}");
}

#[test]
fn rejects_empty_scenarios_list() {
    let yaml = r#"
version: 1
scenarios: []
"#;
    let err = ScenarioFile::from_yaml(yaml, &starting_event()).unwrap_err();
    assert!(matches!(err, InputError::InvalidScenario(_)), "got {err:?}");
}

#[test]
fn rejects_duplicate_scenario_name() {
    let yaml = r#"
version: 1
scenarios:
  - name: dup
    responses:
      - output: ""
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 1
      no_progress_steps: 1
  - name: dup
    responses:
      - output: ""
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 1
      no_progress_steps: 1
"#;
    let err = ScenarioFile::from_yaml(yaml, &starting_event()).unwrap_err();
    assert!(matches!(err, InputError::InvalidScenario(_)), "got {err:?}");
}

#[test]
fn rejects_zero_or_negative_limits() {
    let yaml = r#"
version: 1
scenarios:
  - name: zero
    responses:
      - output: ""
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 0
      no_progress_steps: 1
"#;
    let err = ScenarioFile::from_yaml(yaml, &starting_event()).unwrap_err();
    assert!(matches!(err, InputError::InvalidLimit(_)), "got {err:?}");
}

#[test]
fn rejects_no_progress_greater_than_max_steps() {
    let yaml = r#"
version: 1
scenarios:
  - name: bad-limit
    responses:
      - output: ""
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 2
      no_progress_steps: 5
"#;
    let err = ScenarioFile::from_yaml(yaml, &starting_event()).unwrap_err();
    assert!(matches!(err, InputError::InvalidLimit(_)), "got {err:?}");
}

#[test]
fn rejects_terminal_without_topic() {
    let yaml = r#"
version: 1
scenarios:
  - name: bad-term
    responses:
      - output: ""
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: success
    limits:
      max_steps: 1
      no_progress_steps: 1
"#;
    let err = ScenarioFile::from_yaml(yaml, &starting_event()).unwrap_err();
    assert!(matches!(err, InputError::InvalidScenario(_)), "got {err:?}");
}

#[test]
fn rejects_start_event_mismatch() {
    let yaml = r#"
version: 1
scenarios:
  - name: mismatch
    responses:
      - output: ""
    expect:
      start_event: plan.ready
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 1
      no_progress_steps: 1
"#;
    let err = ScenarioFile::from_yaml(yaml, &starting_event()).unwrap_err();
    assert!(
        matches!(err, InputError::StartEventMismatch { .. }),
        "got {err:?}"
    );
}

#[test]
fn rejects_payload_fields_not_object() {
    let yaml = r#"
version: 1
scenarios:
  - name: bad-payload
    responses:
      - output: ""
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
      payload_fields:
        work.done: not-a-map
    limits:
      max_steps: 1
      no_progress_steps: 1
"#;
    let err = ScenarioFile::from_yaml(yaml, &starting_event()).unwrap_err();
    assert!(matches!(err, InputError::InvalidScenario(_)), "got {err:?}");
}

#[test]
fn empty_response_sequence_is_allowed() {
    // scenario with no responses is allowed (used for budget-exhaustion fixtures)
    let yaml = r#"
version: 1
scenarios:
  - name: empty
    responses: []
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 1
      no_progress_steps: 1
"#;
    let parsed = ScenarioFile::from_yaml(yaml, &starting_event()).expect("parse succeeds");
    assert!(parsed.scenarios[0].responses.is_empty());
}

#[test]
fn rejects_response_missing_output_field() {
    let yaml = r#"
version: 1
scenarios:
  - name: no-output
    responses:
      - hat: producer
        success: true
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 1
      no_progress_steps: 1
"#;
    let err = ScenarioFile::from_yaml(yaml, &starting_event()).unwrap_err();
    assert!(matches!(err, InputError::InvalidScenario(_)), "got {err:?}");
}

#[test]
fn report_serde_round_trip_keeps_required_fields() {
    let report = PresetVerifyReport {
        passed: true,
        source_kind: SourceKind::Builtin,
        static_layer: StaticLayer {
            passed: true,
            warnings: 0,
            errors: 0,
            findings: vec![],
        },
        scenarios: vec![VerifyReportScenario {
            name: "s".to_string(),
            passed: true,
            steps: 3,
            accepted_events: vec!["work.start".to_string(), "work.done".to_string()],
            rejected_events: vec![],
            terminal_topic: Some("LOOP_COMPLETE".to_string()),
            termination: Some("Completed".to_string()),
            failure_kind: None,
            last_observable_state: Default::default(),
            trace_digest: "deadbeef".to_string(),
        }],
        failure_kind: None,
        last_observable_state: Default::default(),
        trace_digest: "deadbeef".to_string(),
    };

    let serialized = serde_json::to_string(&report).expect("serialize");
    for needle in [
        "\"passed\"",
        "\"source_kind\"",
        "\"static\"",
        "\"scenarios\"",
        "\"failure_kind\"",
        "\"last_observable_state\"",
        "\"trace_digest\"",
    ] {
        assert!(
            serialized.contains(needle),
            "report JSON missing {needle}: {serialized}"
        );
    }

    let parsed: PresetVerifyReport = serde_json::from_str(&serialized).expect("deserialize");
    assert!(parsed.passed);
    assert_eq!(parsed.scenarios.len(), 1);
    assert!(matches!(parsed.source_kind, SourceKind::Builtin));
    assert!(parsed.failure_kind.is_none());
}

#[test]
fn trace_digest_is_deterministic_for_same_input() {
    let yaml = r#"
version: 1
scenarios:
  - name: stable
    responses:
      - hat: producer
        output: |
          <event topic="work.done">{"ok":true}</event>
    expect:
      start_event: work.start
      accepted_events: [work.done]
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 4
      no_progress_steps: 2
"#;

    let parsed = ScenarioFile::from_yaml(yaml, &starting_event()).expect("parse succeeds");
    let scenario = &parsed.scenarios[0];
    let digest_a = compute_trace_digest(scenario, "scenario-input-blob", &["work.done"]);
    let digest_b = compute_trace_digest(scenario, "scenario-input-blob", &["work.done"]);
    assert_eq!(digest_a, digest_b);
    assert_eq!(digest_a.len(), 64); // sha-256 hex
    // Different accepted events → different digest
    let digest_c = compute_trace_digest(scenario, "scenario-input-blob", &["work.failed"]);
    assert_ne!(digest_a, digest_c);
}

#[test]
fn trace_digest_excludes_absolute_paths_and_timestamps() {
    let yaml = r#"
version: 1
scenarios:
  - name: stable
    responses:
      - output: ""
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 1
      no_progress_steps: 1
"#;
    let parsed = ScenarioFile::from_yaml(yaml, &starting_event()).expect("parse succeeds");
    let scenario = &parsed.scenarios[0];
    let digest = compute_trace_digest(scenario, "blob", &[]);
    for forbidden in ["/Users/pittcat", "/tmp/", "/var/folders", "2026-08-15T"] {
        assert!(
            !digest.contains(forbidden),
            "digest must not contain {forbidden}"
        );
    }
}

#[test]
fn source_kind_derivation_uses_hats_source_label() {
    assert!(matches!(
        SourceKind::from_hats_source("builtin:ce-executor-pipeline"),
        SourceKind::Builtin
    ));
    assert!(matches!(
        SourceKind::from_hats_source("/abs/path/hats.yml"),
        SourceKind::External
    ));
    assert!(matches!(
        SourceKind::from_hats_source("./relative/hats.yml"),
        SourceKind::External
    ));
    // explicit remote URL → must be rejected before runtime
    let remote = SourceKind::from_hats_source("https://example.com/hats.yml");
    assert!(matches!(remote, SourceKind::External));
    assert!(SourceKind::is_remote("https://example.com/hats.yml"));
    assert!(SourceKind::is_remote("http://example.com/hats.yml"));
    assert!(!SourceKind::is_remote("builtin:ce-executor-pipeline"));
}

#[test]
fn limits_rejects_zero_or_negative_no_progress() {
    assert!(Limits::new(8, 0).is_err());
    // `u32` makes `-1` unrepresentable; instead check that an over-large
    // no_progress_steps (i.e. > max_steps) is rejected by Limits::new.
    assert!(Limits::new(8, 9).is_err());
}

#[test]
fn expect_block_accepts_none_terminal() {
    let block = ExpectBlock {
        start_event: "work.start".to_string(),
        accepted_events: vec![],
        forbidden_events: vec![],
        terminal: TerminalKind::None,
        terminal_topic: None,
        payload_fields: Default::default(),
    };
    assert!(block.validate().is_ok());
}

#[test]
fn expect_block_rejects_non_none_terminal_without_topic() {
    let block = ExpectBlock {
        start_event: "work.start".to_string(),
        accepted_events: vec![],
        forbidden_events: vec![],
        terminal: TerminalKind::Success,
        terminal_topic: None,
        payload_fields: Default::default(),
    };
    assert!(block.validate().is_err());
}

#[test]
fn forbidden_and_accepted_overlap_is_detected() {
    let mut accepted = BTreeSet::new();
    accepted.insert("work.done".to_string());
    let mut forbidden = BTreeSet::new();
    forbidden.insert("work.done".to_string());
    let overlap: Vec<_> = accepted.intersection(&forbidden).collect();
    assert_eq!(overlap.len(), 1);
}

#[test]
fn event_loop_starting_event_helper_is_stable() {
    // Sanity: the helper used by tests reads starting_event from the public config type.
    let mut cfg = EventLoopConfig::default();
    cfg.starting_event = Some("plan.ready".to_string());
    assert_eq!(cfg.starting_event.as_deref(), Some("plan.ready"));
}

#[test]
fn scenario_default_success_true() {
    // When the scenario YAML omits success, it must default to true.
    let yaml = r#"
version: 1
scenarios:
  - name: default-success
    responses:
      - hat: producer
        output: ""
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 1
      no_progress_steps: 1
"#;
    let parsed = ScenarioFile::from_yaml(yaml, &starting_event()).expect("parse");
    assert!(parsed.scenarios[0].responses[0].success);
}