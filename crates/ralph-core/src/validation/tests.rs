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
// 2026-07-07-001 plan U1: runtime unified pipeline must wire
// the handoff HatRegistry so an envelope addressed to an
// unknown to_hat is rejected. The previous fix-plan
// (2026-07-06-004) wired the registry into the *pure*
// `validate_handoff_envelope_payload` helper but left the
// `ValidationPipeline::from_config` constructor (and its two
// production callers) without a registry. This pair of tests
// pins the wiring end-to-end through the production entry
// point `build_unified_validation_pipeline`.
// ============================================================

fn envelope_payload_with_to_hat(to_hat: &str) -> String {
    format!(
        r#"{{
  "plan_name":"p1",
  "plan_path":"docs/plans/p1.md",
  "task_id":"t1",
  "task_key":"p1:step-1:implement",
  "step":"step-1",
  "handoff_envelope":{{
    "schema_version":"handoff-envelope.v1",
    "root_goal":"ship without regressions",
    "plan":{{
      "name":"p1",
      "path":"docs/plans/p1.md",
      "current_step":"step-1",
      "completed_steps":["step-0"]
    }},
    "state":{{
      "current_status":"ready_for_review",
      "last_signal":"work.done",
      "blocking_reason":null
    }},
    "receiver_contract":{{
      "to_hat":"{to_hat}",
      "must_do":["review step-1"],
      "must_not_do":["regress step-0"],
      "success_signal":"work.done",
      "failure_signal":"work.failed"
    }}
  }}
}}"#
    )
}

#[test]
fn runtime_validation_pipeline_rejects_unknown_handoff_to_hat() {
    // Build the production entry point (`build_unified_validation_pipeline`)
    // and confirm it wires the registry automatically — no manual
    // `from_registry` call. If the wiring regresses (e.g. someone drops
    // the registry back to `None`), this test must fail.
    use crate::config::RalphConfig;
    use crate::event_loop::policy::build_unified_validation_pipeline;
    let yaml = r#"
hats:
  executor:
    name: Executor
    triggers: ["work.ready"]
    publishes: ["work.done", "work.failed"]
    instructions: ""
  reviewer:
    name: Reviewer
    triggers: ["work.done"]
    publishes: ["review.passed"]
    instructions: ""
event_loop:
  handoff_envelope:
    enabled: true
    validate_payload: true
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      work.done:
        required_fields: ["handoff_envelope", "task_id", "step"]
      work.failed:
        required_fields: ["handoff_envelope", "task_id"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let pipeline = build_unified_validation_pipeline(&config);
    assert!(
        pipeline.handoff_registry.is_some(),
        "build_unified_validation_pipeline must wire the runtime HatRegistry; got None"
    );

    let view =
        ProtocolView::from_event_loop_with_feature_hats(&config.event_loop, &config.hats, true);
    let mut snap = minimal_snapshot();
    let mut projected = snap.clone();
    let mut ctx = ValidationContext::new(&mut snap);
    let mut projected_ctx = ValidationContext::new(&mut projected);

    let event = make_event(
        "work.done",
        &envelope_payload_with_to_hat("ghost-hat"),
        Some("executor"),
    );
    let report = pipeline.validate_with_preview(&view, &mut ctx, &mut projected_ctx, &event);
    assert!(
        !report.accepted,
        "unknown to_hat must surface as a pipeline rejection; report: {report:?}"
    );
    // The rejection must originate from EventPolicy (the registry-aware
    // rule); pin the stage so we don't regress to a different gate.
    let first_reject = report
        .first_rejection()
        .expect("at least one rule must reject the envelope");
    assert_eq!(
        first_reject.stage,
        ValidationStage::EventPolicy,
        "the registry-aware check is owned by EventPolicyRule"
    );
    // The correction hint is the carrier for the unstable envelope
    // validator's message (which already contains the offending
    // `ghost-hat` and the stable unknown-to_hat code). Pin both.
    let hint = first_reject
        .correction_hint
        .as_deref()
        .unwrap_or("")
        .to_string();
    assert!(
        hint.contains("handoff_envelope_unknown_to_hat"),
        "correction_hint must surface the stable unknown-to_hat code; got: {hint}"
    );
    assert!(
        hint.contains("ghost-hat"),
        "correction_hint must name the offending to_hat id; got: {hint}"
    );
}

#[test]
fn runtime_validation_pipeline_accepts_known_handoff_to_hat() {
    // Symmetric happy-path: production entry point must wire the
    // registry and still accept a known to_hat.
    use crate::config::RalphConfig;
    use crate::event_loop::policy::build_unified_validation_pipeline;
    let yaml = r#"
hats:
  executor:
    name: Executor
    triggers: ["work.ready"]
    publishes: ["work.done", "work.failed"]
    instructions: ""
  reviewer:
    name: Reviewer
    triggers: ["work.done"]
    publishes: ["review.passed"]
    instructions: ""
event_loop:
  handoff_envelope:
    enabled: true
    validate_payload: true
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      work.done:
        required_fields: ["handoff_envelope", "task_id", "step"]
      work.failed:
        required_fields: ["handoff_envelope", "task_id"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let pipeline = build_unified_validation_pipeline(&config);
    let view =
        ProtocolView::from_event_loop_with_feature_hats(&config.event_loop, &config.hats, true);
    let mut snap = minimal_snapshot();
    let mut projected = snap.clone();
    let mut ctx = ValidationContext::new(&mut snap);
    let mut projected_ctx = ValidationContext::new(&mut projected);
    let event = make_event(
        "work.done",
        &envelope_payload_with_to_hat("reviewer"),
        Some("executor"),
    );
    let report = pipeline.validate_with_preview(&view, &mut ctx, &mut projected_ctx, &event);
    assert!(
        report.accepted,
        "known to_hat must pass; first_rejection={:?}",
        report.first_rejection()
    );
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
    // U1 of plan 2026-07-05-005 (KTD-1): the gate's current
    // step is derived from `completed_steps.last()`. To pass
    // step-alignment with an inbound step=step-02, the
    // completed list must end with step-02. The legacy
    // `current_step` field is ignored at read time.
    progress.completed_steps = vec!["step-01".to_string(), "step-02".to_string()];
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

// ============================================================
// U5 of plan 2026-07-02-005: StepHandoffRule disk fallback
// ============================================================

#[test]
fn u5_step_handoff_rule_disk_reload_accepts_when_in_memory_empty() {
    // 140149 / 175407 root cause: the runtime's in-memory
    // LedgerSnapshot.tasks is stale (the new task landed on disk
    // after the snapshot was taken). The gate, when wired with a
    // tasks_path, must fall back to a disk reload via
    // `resolve_task_for_gate` and accept the event.
    use crate::step_handoff::ProgressSnapshot;
    use crate::task::{Task, TaskStatus};
    use crate::validation::rules_step_handoff::StepHandoffRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    // Progress: derived `current_step = step-02` (U1 of plan
    // 2026-07-05-005 KTD-1: derived from completed_steps.last()).
    // The closed task's title is `step-01`, so the task
    // alignment branch (3) requires the closed task's title to
    // be in progress.
    let mut progress = ProgressSnapshot::default();
    progress.completed_steps = vec!["step-01".to_string(), "step-02".to_string()];
    snap.progress = progress;
    // In-memory is EMPTY (the stale-view scenario).
    snap.tasks = Vec::new();

    // Build a real tasks.jsonl on disk with the missing row.
    let dir = tempfile::tempdir().unwrap();
    let tasks_path = dir.path().join("tasks.jsonl");
    let mut disk_task = Task::new("step-01".to_string(), 1);
    disk_task.id = "task-1".to_string();
    disk_task.status = TaskStatus::Closed;
    std::fs::write(
        &tasks_path,
        format!("{}\n", serde_json::to_string(&disk_task).unwrap()),
    )
    .unwrap();

    // Wire the context with the disk path.
    let mut ctx = ValidationContext::new(&mut snap).with_tasks_path(tasks_path.clone());

    let event = make_event(
        "queue.advance",
        r#"{"step":"step-02","task_id":"task-1"}"#,
        None,
    );
    let result = StepHandoffRule.validate(&view, &mut ctx, &event);
    assert!(
        result.accepted,
        "U5: disk reload must rescue the event when in-memory is empty; \
         got: {result:?}"
    );
}

#[test]
fn u5_step_handoff_rule_without_tasks_path_keeps_legacy_reject() {
    // Same setup, but `ValidationContext` is built WITHOUT
    // `with_tasks_path`. Legacy behaviour: in-memory miss → reject.
    // U1 of plan 2026-07-05-005 (KTD-1): derived
    // `current_step = step-02` (last in completed list).
    use crate::step_handoff::ProgressSnapshot;
    use crate::validation::rules_step_handoff::StepHandoffRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut progress = ProgressSnapshot::default();
    progress.completed_steps = vec!["step-01".to_string(), "step-02".to_string()];
    snap.progress = progress;
    snap.tasks = Vec::new();

    let mut ctx = ValidationContext::new(&mut snap);
    // No with_tasks_path call.

    let event = make_event(
        "queue.advance",
        r#"{"step":"step-02","task_id":"task-1"}"#,
        None,
    );
    let result = StepHandoffRule.validate(&view, &mut ctx, &event);
    assert!(
        !result.accepted,
        "U5: without tasks_path, the gate must keep the legacy in-memory reject path"
    );
    assert!(
        result
            .reason_code
            .as_deref()
            .unwrap_or("")
            .contains("task_not_found"),
        "expected task_not_found reason, got: {:?}",
        result.reason_code
    );
}

#[test]
fn u5_step_handoff_rule_disk_reload_accepts_plan_complete_terminal() {
    // P0-2: plan.complete on a closed task whose in-memory row
    // is missing must still pass through the disk fallback.
    // U1 of plan 2026-07-05-005 (KTD-1): derived
    // `current_step = step-02` (last in completed list).
    use crate::step_handoff::ProgressSnapshot;
    use crate::task::{Task, TaskStatus};
    use crate::validation::rules_step_handoff::StepHandoffRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut progress = ProgressSnapshot::default();
    progress.completed_steps = vec!["step-01".to_string(), "step-02".to_string()];
    snap.progress = progress;
    snap.tasks = Vec::new();

    let dir = tempfile::tempdir().unwrap();
    let tasks_path = dir.path().join("tasks.jsonl");
    let mut disk_task = Task::new("step-01".to_string(), 1);
    disk_task.id = "task-1".to_string();
    disk_task.status = TaskStatus::Closed;
    std::fs::write(
        &tasks_path,
        format!("{}\n", serde_json::to_string(&disk_task).unwrap()),
    )
    .unwrap();

    let mut ctx = ValidationContext::new(&mut snap).with_tasks_path(tasks_path);

    let event = make_event(
        "plan.complete",
        r#"{"plan_name":"p","completed_steps":2,"task_id":"task-1","task_key":"k1","step":"step-02","verdict":"pass"}"#,
        None,
    );
    let result = StepHandoffRule.validate(&view, &mut ctx, &event);
    assert!(
        result.accepted,
        "U5: plan.complete with closed task on disk must accept; got: {result:?}"
    );
}

// ============================================================
// U6 of plan 2026-07-02-005: progress stale refresh
// ============================================================

#[test]
fn u6_step_handoff_rule_refreshes_stale_progress_from_disk() {
    // 175407 root cause: in-memory `LedgerSnapshot.progress` is
    // stale (the projector wrote a fresh `progress.md` but the
    // snapshot mirror kept the pre-flush view). The gate would
    // emit `progress_missing_current_step` on a perfectly valid
    // event. The rule must reconcile the in-memory view from
    // disk BEFORE running the gate check.
    use crate::step_handoff::ProgressSnapshot;
    use crate::validation::rules_step_handoff::StepHandoffRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    // Stale in-memory: empty completed list, current step is
    // step-05 (does not match event step).
    // U1 of plan 2026-07-05-005 (KTD-1): the gate's current step
    // is derived from `completed_steps.last()`; the in-memory
    // `current_step` field is irrelevant for read.
    let mut progress = ProgressSnapshot::default();
    progress.current_step = Some("step-05".to_string());
    progress.completed_steps = Vec::new();
    snap.progress = progress;

    // Disk has the real state: step-01 and step-02 done so the
    // derived current step is step-02 (matching the inbound
    // event).
    let dir = tempfile::tempdir().unwrap();
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("progress.md"),
        "## Current Step\nstep-99\n\n## Completed Steps\n- step-01\n- step-02\n",
    )
    .unwrap();
    let tasks_path = agent_dir.join("tasks.jsonl");
    std::fs::write(&tasks_path, "").unwrap();

    let mut ctx = ValidationContext::new(&mut snap).with_tasks_path(tasks_path);

    let event = make_event("queue.advance", r#"{"step":"step-02"}"#, None);
    let result = StepHandoffRule.validate(&view, &mut ctx, &event);
    assert!(
        result.accepted,
        "U6: stale in-memory progress must be reconciled from disk; got: {result:?}"
    );
}

#[test]
fn u6_step_handoff_rule_no_refresh_without_tasks_path() {
    // No tasks_path wired → no progress reconciliation → the
    // gate keeps using the in-memory view (legacy behaviour).
    use crate::step_handoff::ProgressSnapshot;
    use crate::validation::rules_step_handoff::StepHandoffRule;
    let view = ProtocolView::from_event_loop(&minimal_config());
    let mut snap = minimal_snapshot();
    let mut progress = ProgressSnapshot::default();
    progress.current_step = Some("step-05".to_string());
    progress.completed_steps = Vec::new();
    snap.progress = progress;
    // No with_tasks_path call.
    let mut ctx = ValidationContext::new(&mut snap);
    let event = make_event("queue.advance", r#"{"step":"step-02"}"#, None);
    let result = StepHandoffRule.validate(&view, &mut ctx, &event);
    assert!(
        !result.accepted,
        "U6: without tasks_path, the gate must keep using the in-memory view"
    );
}
