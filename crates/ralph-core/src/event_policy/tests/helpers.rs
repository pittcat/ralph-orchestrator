//! Test helpers shared between tests_part1 and tests_part2.
//! Plan 2026-08-07-002 §7 U6: dead helpers removed; all remaining helpers
//! are actively used by the test suite.

pub use crate::config::{
    ElementConstraint, EventSchema, HandoffEnvelopeConfig, HatAllowedValues, RalphConfig,
    TopicDenyRule,
};
pub(crate) use crate::event_policy::validation::extract_json_field;
pub use crate::event_policy::{
    CandidateEmitPreview, CandidateHatEntry, CompletionAfterTerminalAction, DefaultHandoffConfig,
    DuplicateWorkDoneHint, EventLoopHandoffConfig, EventPolicyConfig, EventPolicyMode,
    HandoffEnvelopeConfigAccess, NULL_PAYLOAD_REJECT_TOPICS, NextHatCandidates, PayloadType,
    PolicyDecision, PolicyFinding, PolicyReasonEntry, PolicyRejection, PolicyRuntimeState,
    ProjectionAction, ProjectionPreview, ReasonClass, ViolationAction, ViolationType,
    build_allowed_topics, check_completion_guard, check_completion_honored, check_handoff_envelope,
    check_topic_deny_rules, check_topic_format, evaluate_candidate_emit,
    handoff_envelope_validation_enabled, is_null_payload_rejected_topic,
    is_recoverable_policy_finding, is_system_control_topic, is_system_topic, matches_topic_rule,
    precheck_proposed_dedup_key, validate_event, validate_event_with_hat,
    validate_event_with_options,
};
pub use crate::event_reader::EventReader;
pub use ralph_proto::{Hat, HatId, Topic};
pub use serde_json::Value;
pub use std::collections::{HashMap, HashSet};
pub use std::io::Write;
pub use tempfile::NamedTempFile;

// === Fixture constants (extracted verbatim from original tests block) ===
pub const FIXTURE_VALID_CHAIN: &str = r#"{"topic":"experiment.planned","payload":{"task_key":"a","hypothesis":"h","falsification_condition":"f"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:01Z"}"#;

pub const FIXTURE_DUPLICATE_TERMINAL: &str = r#"{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"LOOP_COMPLETE","payload":{"reason":"retry"},"ts":"2026-05-22T00:00:01Z"}"#;

pub const FIXTURE_BUSINESS_AFTER_TERMINAL: &str = r#"{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"experiment.planned","payload":{"task_key":"b","hypothesis":"h","falsification_condition":"f"},"ts":"2026-05-22T00:00:01Z"}"#;

pub const FIXTURE_MISSING_REQUIRED_FIELDS: &str =
    r#"{"topic":"experiment.planned","payload":{"task_key":"a"},"ts":"2026-05-22T00:00:00Z"}"#;

pub const HITTING_PAYLOAD: &str =
    r#"{"review_verdict":"blocked","fixes_applied":0,"fix_status":"applied"}"#;

// === StubHandoff (extracted verbatim) ===
pub struct StubHandoff {
    pub enabled: bool,
    pub validate_payload: bool,
}

impl HandoffEnvelopeConfigAccess for StubHandoff {
    fn handoff_envelope_enabled(&self) -> bool {
        self.enabled
    }
    fn handoff_envelope_validate_payload(&self) -> bool {
        self.validate_payload
    }
}

pub fn test_config() -> EventPolicyConfig {
    EventPolicyConfig {
        enabled: true,
        mode: EventPolicyMode::Enforce,
        on_violation: ViolationAction::RejectWithResume,
        schemas: HashMap::new(),
        terminal_topics: vec!["LOOP_COMPLETE".to_string()],
        business_topics: vec!["experiment.planned".to_string()],
        ..Default::default()
    }
}

pub fn fixture_config() -> EventPolicyConfig {
    let mut config = test_config();
    let schema = EventSchema {
        payload: Some(PayloadType::JsonObject),
        required_fields: vec![
            "task_key".to_string(),
            "hypothesis".to_string(),
            "falsification_condition".to_string(),
        ],
        allowed_values: HashMap::new(),
        hat_allowed_values: HashMap::new(),
        ..Default::default()
    };
    config
        .schemas
        .insert("experiment.planned".to_string(), schema);
    config.completion_after_terminal.duplicate_terminal = CompletionAfterTerminalAction::Reject;
    config.completion_after_terminal.business_after_completion =
        CompletionAfterTerminalAction::Reject;
    config
}

pub fn parse_fixture_line(line: &str) -> (String, Option<String>) {
    let event: crate::event_reader::Event = serde_json::from_str(line).expect("valid fixture line");
    (event.topic, event.payload)
}

pub fn is_accept(decision: &PolicyDecision) -> bool {
    matches!(decision, PolicyDecision::Accept)
}

pub fn replay_and_validate(fixture: &str) -> (PolicyRuntimeState, PolicyDecision) {
    let config = fixture_config();
    let lines: Vec<&str> = fixture.lines().collect();
    let mut file = NamedTempFile::new().unwrap();
    for line in &lines[..lines.len().saturating_sub(1)] {
        writeln!(file, "{}", line).unwrap();
    }
    file.flush().unwrap();
    let mut state = PolicyRuntimeState::from_events(file.path(), &config).unwrap();
    // Simulate the event loop marking completion as honored once a terminal
    // event has been observed in the replayed history.
    if state.terminal_observed {
        state.completion_honored = true;
    }
    let (topic, payload) = parse_fixture_line(lines.last().unwrap());
    let decision = validate_event(&topic, payload.as_deref(), &config, &mut state);
    (state, decision)
}

pub fn review_dimension_ready_payload(plan: &str, step: &str, task: &str, dim: &str) -> String {
    format!(
        r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","dimension":"{dim}","wave_id":"w1"}}"#
    )
}

pub fn review_start_payload(plan: &str, step: Option<&str>, task: &str) -> String {
    if let Some(st) = step {
        format!(
            r#"{{"plan_name":"{plan}","step":"{st}","task_id":"{task}","task_key":"k-{task}"}}"#
        )
    } else {
        format!(r#"{{"plan_name":"{plan}","task_id":"{task}","task_key":"k-{task}"}}"#)
    }
}

pub fn review_dimension_failed_payload(dim: Option<&str>) -> String {
    match dim {
        Some(d) => {
            format!(r#"{{"dimension":"{d}","plan_name":"p1","step":"step-01","task_id":"t1"}}"#)
        }
        None => r#"{"plan_name":"p1","step":"step-01","task_id":"t1"}"#.to_string(),
    }
}

pub fn full_payload() -> serde_json::Value {
    serde_json::json!({
        "plan_name": "2026-07-06-u8-fixture",
        "plan_path": "docs/plans/2026-07-06-u8-fixture.md",
        "task_id": "task-live-id",
        "task_key": "2026-07-06-u8-fixture:step-3:implement",
        "step": "step-3",
        "handoff_envelope": {
            "schema_version": "handoff-envelope.v1",
            "root_goal": "ship the plan without regressions",
            "plan": {
                "name": "2026-07-06-u8-fixture",
                "path": "docs/plans/2026-07-06-u8-fixture.md",
                "current_step": "step-3",
                "completed_steps": ["step-1", "step-2"]
            },
            "state": {
                "current_status": "ready_for_review",
                "last_signal": "work.done",
                "blocking_reason": null
            },
            "receiver_contract": {
                "to_hat": "goal-alignment-reviewer",
                "must_do": ["review step-3"],
                "must_not_do": ["regress step-2"],
                "success_signal": "work.done",
                "failure_signal": "work.failed"
            }
        }
    })
}

pub fn policy_minimal() -> EventPolicyConfig {
    use crate::config::{EventPolicyMode, ViolationAction};
    // U1 (2026-07-06-004 fix-plan) does NOT change the
    // gate semantics — `validate_event_with_options`'s
    // `check_handoff_envelope` gate keeps running for
    // every event whose payload parses as a JSON object
    // whenever the typed `validate_payload: true` flag
    // is on. The wire-up merely replaces the no-op
    // `DefaultHandoffConfig` with the real typed
    // `EventLoopHandoffConfig` so the gate fires at the
    // production CLI / loop boundary instead of being
    // invisible behind a default-off trait.
    EventPolicyConfig {
        enabled: true,
        mode: EventPolicyMode::Enforce,
        on_violation: ViolationAction::RejectWithResume,
        schemas: HashMap::new(),
        schema_file: None,
        terminal_topics: vec![],
        business_topics: vec![],
        require_policy_check_for_cli_emit: false,
        allow_unsafe_cli_emit: true,
        require_emit_provenance: false,
        completion_after_terminal: Default::default(),
        topic_deny_rules: vec![],
        payload_consistency: Default::default(),
        plan_name_equality_required: false,
    }
}

pub fn consistency_rule(
    id: &str,
    topic: &str,
    when: Value,
    message: &str,
) -> crate::config::PayloadConsistencyRule {
    crate::config::PayloadConsistencyRule {
        id: id.to_string(),
        topic: topic.to_string(),
        when,
        message: message.to_string(),
    }
}

pub fn fix_done_contradiction_rule() -> crate::config::PayloadConsistencyRule {
    consistency_rule(
        "fix-done-no-fixes",
        "fix.done",
        serde_json::json!({"all": [
            {"field": "review_verdict", "eq": "blocked"},
            {"field": "fixes_applied", "eq": 0},
            {"field": "fix_status", "eq": "applied"}
        ]}),
        "fix.done claims applied but no fixes were applied while verdict is blocked",
    )
}

pub fn consistency_config(
    enabled: bool,
    rules: Vec<crate::config::PayloadConsistencyRule>,
) -> EventPolicyConfig {
    let mut config = test_config();
    config.payload_consistency = crate::config::PayloadConsistencyConfig { enabled, rules };
    config
}

pub fn candidate_emit_test_config() -> RalphConfig {
    use crate::config::{
        EventPolicyConfig, EventPolicyMode, EventSchema, HatConfig, PayloadType, ViolationAction,
    };
    let mut cfg = RalphConfig::default();

    let hat_cfg = HatConfig {
        name: "worker".to_string(),
        publishes: vec!["work.ready".to_string()],
        triggers: vec!["build.task".to_string()],
        ..Default::default()
    };
    cfg.hats.insert("worker".to_string(), hat_cfg);

    let schema = EventSchema {
        payload: Some(PayloadType::JsonObject),
        required_fields: vec!["task_key".to_string()],
        ..Default::default()
    };
    let policy = EventPolicyConfig {
        enabled: true,
        mode: EventPolicyMode::Enforce,
        on_violation: ViolationAction::RejectWithResume,
        schemas: [("work.ready".to_string(), schema)].into_iter().collect(),
        ..Default::default()
    };
    cfg.event_loop.event_policy = Some(policy);
    cfg
}

pub fn projection_test_config() -> RalphConfig {
    use crate::config::{
        EventPolicyConfig, EventPolicyMode, EventSchema, HatConfig, PayloadType, ViolationAction,
    };
    let mut cfg = RalphConfig::default();

    let hat_cfg = HatConfig {
        name: "reviewer".to_string(),
        publishes: vec!["review.start".to_string()],
        triggers: vec!["build.task".to_string()],
        ..Default::default()
    };
    cfg.hats.insert("reviewer".to_string(), hat_cfg);

    let schema = EventSchema {
        payload: Some(PayloadType::JsonObject),
        required_fields: vec!["plan_name".to_string(), "task_id".to_string()],
        ..Default::default()
    };
    let policy = EventPolicyConfig {
        enabled: true,
        mode: EventPolicyMode::Enforce,
        on_violation: ViolationAction::RejectWithResume,
        schemas: [("review.start".to_string(), schema)].into_iter().collect(),
        ..Default::default()
    };
    cfg.event_loop.event_policy = Some(policy);
    cfg
}
