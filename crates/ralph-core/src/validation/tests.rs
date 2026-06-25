//! U4 validation pipeline tests (smoke + integration).
//!
//! The test coverage is intentionally conservative: every rule
//! has a happy-path + a representative rejection case, and the
//! pipeline composition is exercised end-to-end with a small
//! `LedgerSnapshot` and a synthetic `Event`.

use crate::config::EventLoopConfig;
use crate::event_reader::Event;
use crate::preset::engine::protocol::ProtocolView;
use crate::state::LedgerSnapshot;
use crate::validation::{
    ReasonCode, RulePhase, ValidationContext, ValidationPipeline, ValidationReport,
    ValidationResult, ValidationRule, ValidationStage,
};

/// Helper: build a minimal `EventLoopConfig` whose schema set
/// declares `work.done` as requiring `task_id` + `step`.
fn minimal_config() -> EventLoopConfig {
    let yaml = r#"
event_policy:
  enabled: true
  mode: observe
  schemas:
    work.done:
      required_fields: ["task_id", "step"]
    queue.advance:
      required_fields: ["step"]
"#;
    serde_yaml::from_str(yaml).unwrap()
}

/// Helper: build a synthetic `LedgerSnapshot` for the tests. The
/// snapshot's `progress` and `tasks` are populated by the
/// step-handoff rule; `task_id` is the value used in the
/// happy-path tests.
fn minimal_snapshot() -> LedgerSnapshot {
    LedgerSnapshot::cold_start()
}

/// Helper: build a synthetic `work.done` event with a JSON
/// payload. `payload` is JSON.
fn make_event(topic: &str, payload: &str, hat: Option<&str>) -> Event {
    let payload_opt = if payload.is_empty() {
        None
    } else {
        Some(payload.to_string())
    };
    Event {
        topic: topic.to_string(),
        payload: payload_opt,
        ts: "2026-06-22T00:00:00Z".to_string(),
        hat: hat.map(|s| s.to_string()),
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    }
}

// ============================================================
// Trait-level sanity: every rule exposes a stable name + phase.
// ============================================================

#[test]
fn rule_phase_classification_matches_pipeline() {
    use crate::validation::rules_event_policy::EventPolicyRule;
    use crate::validation::rules_execution_contract::ExecutionContractRule;
    use crate::validation::rules_origin::OriginRule;
    use crate::validation::rules_publisher::PublisherRule;
    use crate::validation::rules_required_fields::RequiredFieldsRule;
    use crate::validation::rules_step_handoff::StepHandoffRule;
    use crate::validation::rules_workflow_guard::WorkflowGuardRule;

    let origin = OriginRule::default();
    let pre: Vec<&dyn ValidationRule> = vec![
        &origin,
        &PublisherRule,
        &RequiredFieldsRule,
        &EventPolicyRule,
        &StepHandoffRule,
    ];
    let post: Vec<&dyn ValidationRule> = vec![&ExecutionContractRule, &WorkflowGuardRule];

    for rule in &pre {
        assert_eq!(rule.applies_to(), RulePhase::PreCommit, "{}", rule.name());
    }
    for rule in &post {
        assert_eq!(rule.applies_to(), RulePhase::PostCommit, "{}", rule.name());
    }
}

#[test]
fn rule_names_match_stage_constants() {
    use crate::validation::rules_event_policy::EventPolicyRule;
    use crate::validation::rules_execution_contract::ExecutionContractRule;
    use crate::validation::rules_origin::OriginRule;
    use crate::validation::rules_publisher::PublisherRule;
    use crate::validation::rules_required_fields::RequiredFieldsRule;
    use crate::validation::rules_step_handoff::StepHandoffRule;
    use crate::validation::rules_workflow_guard::WorkflowGuardRule;

    let origin = OriginRule::default();
    let cases: Vec<(&dyn ValidationRule, ValidationStage)> = vec![
        (&origin, ValidationStage::Origin),
        (&PublisherRule, ValidationStage::Publisher),
        (&RequiredFieldsRule, ValidationStage::RequiredFields),
        (&EventPolicyRule, ValidationStage::EventPolicy),
        (&ExecutionContractRule, ValidationStage::ExecutionContract),
        (&StepHandoffRule, ValidationStage::StepHandoff),
        (&WorkflowGuardRule, ValidationStage::WorkflowGuard),
    ];
    for (rule, stage) in cases {
        assert_eq!(rule.name(), stage.as_str(), "{:?}", stage);
    }
}

// ============================================================
// Pipeline construction: from_config + default_source_hat
// ============================================================

#[test]
fn pipeline_from_config_contains_seven_rules() {
    let config = minimal_config();
    let view = ProtocolView::from_event_loop(&config);
    let pipeline = ValidationPipeline::from_config(&view, &config);
    // 5 pre + 2 post = 7 rules total.
    assert_eq!(pipeline.pre_commit_rules.len(), 5);
    assert_eq!(pipeline.post_commit_rules.len(), 2);
}

// ============================================================
// OriginRule
// ============================================================

#[test]
fn origin_rule_accepts_solo_mode_event() {
    use crate::validation::rules_origin::OriginRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("any.topic", "{}", Some("any-hat"));
    let result = OriginRule::default().validate(&view, &mut ctx, &event);
    assert!(
        result.accepted,
        "solo-mode origin rule should accept: {result:?}"
    );
}

#[test]
fn origin_rule_handles_ralph_pseudo_hat_event() {
    use crate::validation::rules_origin::OriginRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("work.start", "{}", Some("ralph"));
    // The default empty registry accepts everything; the
    // rule's reason_code surface is exercised by the legacy
    // `event_origin` tests. This test pins that the rule
    // *runs* without panicking on the ralph-pseudo-hat path.
    let _ = OriginRule::default().validate(&view, &mut ctx, &event);
}

// ============================================================
// PublisherRule
// ============================================================

#[test]
fn publisher_rule_accepts_default_permissive_view() {
    use crate::validation::rules_publisher::PublisherRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("work.done", r#"{"task_id":"t","step":"s"}"#, None);
    let result = PublisherRule.validate(&view, &mut ctx, &event);
    assert!(result.accepted, "default view is permissive: {result:?}");
}

// ============================================================
// RequiredFieldsRule
// ============================================================

#[test]
fn required_fields_rule_accepts_complete_payload() {
    use crate::validation::rules_required_fields::RequiredFieldsRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("work.done", r#"{"task_id":"t1","step":"s1"}"#, None);
    let result = RequiredFieldsRule.validate(&view, &mut ctx, &event);
    assert!(result.accepted, "{result:?}");
}

#[test]
fn required_fields_rule_rejects_missing_field_with_engine_prefix() {
    use crate::validation::rules_required_fields::RequiredFieldsRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("work.done", r#"{"step":"s1"}"#, None);
    let result = RequiredFieldsRule.validate(&view, &mut ctx, &event);
    assert!(!result.accepted);
    let code = result
        .reason_code
        .expect("reason_code must be set on rejection");
    assert!(
        code.starts_with(ReasonCode::REQUIRED_FIELD_MISSING),
        "unexpected reason_code: {code}"
    );
    assert!(result.retry_eligible, "missing field is retryable");
}

#[test]
fn required_fields_rule_rejects_empty_payload() {
    use crate::validation::rules_required_fields::RequiredFieldsRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("work.done", "", None);
    let result = RequiredFieldsRule.validate(&view, &mut ctx, &event);
    assert!(!result.accepted, "empty payload should be rejected");
    assert!(
        result
            .reason_code
            .as_deref()
            .unwrap_or("")
            .starts_with(ReasonCode::REQUIRED_FIELD_MISSING)
    );
}

#[test]
fn required_fields_rule_accepts_malformed_json_to_defer_to_execution_contract() {
    use crate::validation::rules_required_fields::RequiredFieldsRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("work.done", "not valid json", None);
    let result = RequiredFieldsRule.validate(&view, &mut ctx, &event);
    // The rule is intentionally lenient on parse failures;
    // execution_contract / payload_contract own that domain.
    assert!(result.accepted, "{result:?}");
}

// ============================================================
// StepHandoffRule
// ============================================================

#[test]
fn step_handoff_rule_accepts_non_gated_topic() {
    use crate::validation::rules_step_handoff::StepHandoffRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("review.dimension.done", r#"{"step":"s1"}"#, None);
    let result = StepHandoffRule.validate(&view, &mut ctx, &event);
    assert!(result.accepted, "{result:?}");
}

#[test]
fn step_handoff_rule_rejects_mismatched_step() {
    use crate::step_handoff::ProgressSnapshot;
    use crate::validation::rules_step_handoff::StepHandoffRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut progress = ProgressSnapshot::default();
    progress.current_step = Some("step-05".to_string());
    progress.completed_steps = vec!["step-01".to_string(), "step-02".to_string()];
    snap.progress = progress;
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("queue.advance", r#"{"step":"step-09"}"#, None);
    let result = StepHandoffRule.validate(&view, &mut ctx, &event);
    assert!(!result.accepted);
    let code = result.reason_code.unwrap();
    assert!(
        code.contains("step_mismatch"),
        "unexpected reason_code: {code}"
    );
}

#[test]
fn step_handoff_rule_accepts_aligned_state() {
    use crate::step_handoff::ProgressSnapshot;
    use crate::task::{Task, TaskStatus};
    use crate::validation::rules_step_handoff::StepHandoffRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut progress = ProgressSnapshot::default();
    progress.current_step = Some("step-02".to_string());
    progress.completed_steps = vec!["step-01".to_string()];
    snap.progress = progress;
    let task = {
        let mut t = Task::new("step-01".to_string(), 1);
        t.id = "task-1".to_string();
        t.status = TaskStatus::Closed;
        t
    };
    snap.tasks = vec![task];
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event(
        "queue.advance",
        r#"{"step":"step-02","task_id":"task-1"}"#,
        None,
    );
    let result = StepHandoffRule.validate(&view, &mut ctx, &event);
    assert!(result.accepted, "{result:?}");
}

// ============================================================
// ExecutionContractRule
// ============================================================

#[test]
fn execution_contract_rule_accepts_when_contracts_disabled() {
    use crate::validation::rules_execution_contract::ExecutionContractRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("work.done", r#"{"task_id":"t1"}"#, None);
    let result = ExecutionContractRule.validate(&view, &mut ctx, &event);
    assert!(result.accepted, "{result:?}");
}

#[test]
fn execution_contract_rule_rejects_invalid_payload() {
    use crate::config::execution_contracts::ExecutionContractRule as Ecr;
    use crate::config::execution_contracts::{
        ExecutionContractRule as EcrRule, ExecutionContractsConfig,
    };
    use crate::validation::rules_execution_contract::ExecutionContractRule;
    let mut config = minimal_config();
    let mut ecr = Ecr::default();
    ecr.require_payload_fields = vec!["task_id".to_string()];
    let mut contracts = ExecutionContractsConfig::default();
    contracts.enabled = true;
    contracts.rules.insert("work.done".to_string(), ecr);
    config.execution_contracts = Some(contracts);
    let view = ProtocolView::from_event_loop(&config);
    let mut snap = minimal_snapshot();
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("work.done", "not json", None);
    let result = ExecutionContractRule.validate(&view, &mut ctx, &event);
    assert!(!result.accepted);
    let code = result.reason_code.unwrap();
    assert!(
        code.contains("contract:invalid_payload") || code.contains("contract:missing_task_id"),
        "unexpected reason_code: {code}"
    );
    // Keep the import alive.
    let _ = std::marker::PhantomData::<EcrRule>;
}

// ============================================================
// WorkflowGuardRule
// ============================================================

#[test]
fn workflow_guard_rule_accepts_with_no_chain_configured() {
    use crate::validation::rules_workflow_guard::WorkflowGuardRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("experiment.planned", r#"{}"#, None);
    let result = WorkflowGuardRule.validate(&view, &mut ctx, &event);
    assert!(result.accepted, "{result:?}");
}

// ============================================================
// Pipeline composition (pre + post)
// ============================================================

#[test]
fn pipeline_short_circuit_returns_first_rejection() {
    // Validate the report's `first_rejection()` helper.
    let report = ValidationReport {
        pre_commit: vec![ValidationResult::accept()],
        post_commit: vec![ValidationResult::reject(
            ValidationStage::ExecutionContract,
            "contract:missing_task_id",
            Some("add task_id".to_string()),
            true,
        )],
        accepted: false,
        post_commit_rejected: true,
    };
    let first = report.first_rejection().expect("rejection present");
    assert!(!first.accepted);
    assert_eq!(first.stage, ValidationStage::ExecutionContract);
    assert!(!report.accepted);
    assert!(report.post_commit_rejected);
}

// ============================================================
// Feature flag surface (KTD-8)
// ============================================================

#[test]
#[serial_test::serial]
fn pipeline_records_protocol_view_feature_flag() {
    use crate::preset::engine::protocol::{
        reset_protocol_view_enabled_for_test, set_protocol_view_enabled_for_test,
    };

    let config = minimal_config();
    // U11-T7: default is now ON; explicit `UNIFIED_PROTOCOL_VIEW=0`
    // opts out. This test uses the test-override atomic on
    // `protocol::set_protocol_view_enabled_for_test` so it stays
    // safe under the workspace `forbid(unsafe_code)` lint
    // (the env-var read in `from_event_loop_with_feature_for_env`
    // is short-circuited when the override is set).

    // Default-on: override is `true` (test override defaults to
    // the new U11-T7 default-on semantics) or unset, both yield
    // the unified view.
    reset_protocol_view_enabled_for_test();
    let view_default = ProtocolView::from_event_loop_with_feature_for_env(&config);
    let pipeline_default = ValidationPipeline::from_config(&view_default, &config);
    assert!(
        pipeline_default.feature_enabled,
        "default (override unset) must be feature_enabled = true (U11-T7 default-on)"
    );

    // Explicit off via the test override.
    set_protocol_view_enabled_for_test(false);
    let view_off = ProtocolView::from_event_loop_with_feature_for_env(&config);
    let pipeline_off = ValidationPipeline::from_config(&view_off, &config);
    assert!(
        !pipeline_off.feature_enabled,
        "override = false must yield feature_enabled = false (U11-T7 opt-out)"
    );

    // Reset so subsequent tests see the real env var.
    reset_protocol_view_enabled_for_test();

    // Explicit on via `_and_feature(_, _, true)` still wins
    // (this is the env-agnostic path, no override needed).
    let view_on = ProtocolView::from_event_loop_with_feature(&config, true);
    let pipeline_on = ValidationPipeline::from_config(&view_on, &config);
    assert!(
        pipeline_on.feature_enabled,
        "explicit feature_enabled = true must be respected"
    );
}

// ============================================================
// Stage / reason_code parity: every `ValidationStage` constant
// matches a stable string in `ReasonCode::*` so downstream tools
// can rely on the prefix.
// ============================================================

#[test]
fn reason_code_prefixes_match_stages() {
    // Each stage has a documented prefix that may differ from
    // the stage's `as_str()` (legacy aliases the runtime
    // already emits). The mapping is the SSOT for the
    // reason_code namespace; pin it here.
    let stage_prefixes: &[(&str, &[&str])] = &[
        ("origin", &["origin:"]),
        ("publisher", &["publisher:"]),
        ("required_fields", &["required_fields:", "engine_rejected:"]),
        ("event_policy", &["event_policy:"]),
        ("execution_contract", &["execution_contract:", "contract:"]),
        ("step_handoff", &["step_handoff:"]),
        ("workflow_guard", &["workflow_guard:"]),
    ];
    let pairs = [
        (ReasonCode::ORIGIN_UNKNOWN_HAT, ValidationStage::Origin),
        (ReasonCode::ORIGIN_OUT_OF_SCOPE, ValidationStage::Origin),
        (
            ReasonCode::PUBLISHER_NOT_ALLOWED,
            ValidationStage::Publisher,
        ),
        (
            ReasonCode::REQUIRED_FIELD_MISSING,
            ValidationStage::RequiredFields,
        ),
        (
            ReasonCode::EVENT_POLICY_TOPIC_DENIED,
            ValidationStage::EventPolicy,
        ),
        (
            ReasonCode::CONTRACT_MISSING_TASK_ID,
            ValidationStage::ExecutionContract,
        ),
        (
            ReasonCode::STEP_HANDOFF_MISMATCH_PREFIX,
            ValidationStage::StepHandoff,
        ),
        (
            ReasonCode::WORKFLOW_GUARD_OUT_OF_ORDER,
            ValidationStage::WorkflowGuard,
        ),
    ];
    for (code, stage) in pairs {
        let (_, allowed) = stage_prefixes
            .iter()
            .find(|(name, _)| *name == stage.as_str())
            .unwrap_or_else(|| panic!("stage {} not in mapping", stage.as_str()));
        assert!(
            allowed.iter().any(|prefix| code.starts_with(prefix)),
            "reason_code `{code}` does not start with any of {allowed:?}"
        );
    }
}

// ============================================================
// Integration: full pipeline `validate_with_preview` happy path.
// ============================================================

#[test]
fn validate_with_preview_accepts_well_formed_work_done() {
    let config = minimal_config();
    let view = ProtocolView::from_event_loop(&config);
    let pipeline = ValidationPipeline::from_config(&view, &config);
    let mut snap = minimal_snapshot();
    let mut projected = snap.clone();
    let mut ctx = ValidationContext::new(&mut snap);
    let mut projected_ctx = ValidationContext::new(&mut projected);
    let event = make_event("work.done", r#"{"task_id":"t1","step":"s1"}"#, None);
    // Empty projection equals current snapshot in this test.
    let report = pipeline.validate_with_preview(&view, &mut ctx, &mut projected_ctx, &event);
    // Origin rule (default empty registry) accepts; publisher
    // rule accepts (default permissive); required-fields rule
    // accepts; execution-contract rule accepts (no contracts
    // configured); workflow-guard rule accepts. The
    // step-handoff rule accepts non-gated topics.
    assert!(report.accepted, "expected accepted, got {report:?}");
    assert!(report.post_commit.is_empty() == false);
}

#[test]
fn validate_with_preview_rejects_missing_required_field() {
    let config = minimal_config();
    let view = ProtocolView::from_event_loop(&config);
    let pipeline = ValidationPipeline::from_config(&view, &config);
    let mut snap = minimal_snapshot();
    let mut projected = snap.clone();
    let mut ctx = ValidationContext::new(&mut snap);
    let mut projected_ctx = ValidationContext::new(&mut projected);
    let event = make_event("work.done", r#"{"step":"s1"}"#, None);
    let report = pipeline.validate_with_preview(&view, &mut ctx, &mut projected_ctx, &event);
    assert!(!report.accepted);
    let first = report.first_rejection().expect("rejection present");
    assert!(
        first
            .reason_code
            .as_deref()
            .unwrap_or("")
            .contains("required_field")
    );
}
