//! Tests for execution_contract.

use super::*;

fn contract_enabled_config() -> RalphConfig {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done", "work.failed"]
  reviewer:
    name: "Reviewer"
    triggers: ["work.done"]
    publishes: ["review.done"]
event_loop:
  execution_contracts:
    enabled: true
    rules:
      work.done:
        require_payload_fields:
          - plan_name
          - plan_path
          - task_id
          - task_key
          - step
        require_task:
          id_field: "task_id"
          key_field: "task_key"
          loop_scoped: false
          allowed_terminal_statuses: ["closed"]
          auto_close_on_valid: false
        require_git_change:
          mode: "diff_or_commit"
          allow_empty_for_steps: ["trivial"]
        require_test_evidence:
          mode: "optional"
"#;
    serde_yaml::from_str(yaml).unwrap()
}

fn make_work_done_event() -> crate::event_reader::Event {
    crate::event_reader::Event {
        topic: "work.done".to_string(),
        payload: Some(
            r#"{"plan_name":"p","plan_path":"/p","task_id":"t1","task_key":"k1","step":"step-01"}"#
                .to_string(),
        ),
        ts: "2024-01-01T00:00:00Z".to_string(),
        wave_id: None,
        hat: Some("executor".to_string()),
        triggered: None,
        source: None,
        wave_index: None,
        wave_total: None,
    }
}

#[test]
fn test_execution_contract_rejects_work_done_with_missing_payload() {
    // Test that work.done without required payload fields is rejected
    // This tests the execution contract validator directly
    use crate::config::{
        ExecutionContractRule,
        execution_contracts::{
            ContractRejectConfig, GitChangeRequirement, TaskCompletionRequirement,
            TestEvidenceRequirement,
        },
    };
    use crate::execution_contract::{
        DefaultGitEvidenceProvider, ExecutionContractDecision, ExecutionContractViolationKind,
        validate_execution_contract,
    };

    let rule = ExecutionContractRule {
        require_payload_fields: vec![
            "task_id".to_string(),
            "task_key".to_string(),
            "step".to_string(),
        ],
        require_task: TaskCompletionRequirement::default(),
        require_git_change: GitChangeRequirement::default(),
        require_test_evidence: TestEvidenceRequirement::default(),
        reject: ContractRejectConfig::default(),
    };

    let event = Event::new("work.done", r#"{"task_id":"t1"}"#);

    let decision = validate_execution_contract(
        &event,
        &rule,
        std::path::Path::new("/tmp"),
        "loop-1",
        std::path::Path::new("/tmp/tasks.jsonl"),
        None,
        &DefaultGitEvidenceProvider,
        None,
    );

    match &decision {
        ExecutionContractDecision::Reject(findings) => {
            assert!(
                findings.iter().any(|f| matches!(
                    f.kind,
                    ExecutionContractViolationKind::MissingPayloadField { .. }
                )),
                "Should have MissingPayloadField rejection"
            );
        }
        ExecutionContractDecision::Accept => {
            panic!("Expected rejection for missing payload fields");
        }
    }
}

#[test]
fn test_execution_contract_disabled_passes_through() {
    // When execution_contracts is disabled (default), events pass through normally
    let yaml = r#"
event_loop:
  execution_contracts:
    enabled: false
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    let result = event_loop
        .process_parse_result(crate::event_reader::ParseResult {
            events: vec![crate::event_reader::Event {
                topic: "work.done".to_string(),
                payload: Some(r#"{"task_id":"t1","task_key":"k1","step":"s1"}"#.to_string()),
                ts: "2024-01-01T00:00:00Z".to_string(),
                wave_id: None,
                hat: Some("executor".to_string()),
                triggered: None,
                source: None,
                wave_index: None,
                wave_total: None,
            }],
            malformed: vec![],
        })
        .expect("process_parse_result should succeed");

    // Without execution contract enabled, the event should be processed
    // (not rejected at contract validation stage since contract is disabled)
    assert!(
        result.contract_rejections.is_empty(),
        "No contract rejections when contract is disabled"
    );
}

#[test]
fn test_execution_contract_validates_task_status() {
    // Test that execution contract config is parsed correctly
    let yaml = r#"
event_loop:
  execution_contracts:
    enabled: true
    rules:
      work.done:
        require_payload_fields: ["task_id"]
        require_task:
          id_field: "task_id"
          key_field: "task_key"
          loop_scoped: false
          allowed_terminal_statuses: ["closed"]
          auto_close_on_valid: false
        require_git_change:
          mode: "diff_or_commit"
          allow_empty_for_steps: []
        require_test_evidence:
          mode: "optional"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

    // Verify the config parses correctly and the contract structure is sound
    assert!(
        config.event_loop.execution_contracts.is_some(),
        "Execution contracts should be parsed from config"
    );
    let contracts = config.event_loop.execution_contracts.unwrap();
    assert!(contracts.enabled, "Contracts should be enabled");
    assert!(
        contracts.rules.contains_key("work.done"),
        "work.done rule should exist"
    );
}

#[test]
fn test_contract_rejection_does_not_publish_original_event() {
    // When the contract rejects work.done, the original event must NOT be
    // published to bus subscribers. Reviewer hat must remain untriggered.
    use ralph_proto::Event;
    let config = contract_enabled_config();
    let mut event_loop = EventLoop::new(config);

    // Use an observer to record all events published to the bus.
    let observed: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed_clone = std::sync::Arc::clone(&observed);
    event_loop.bus().add_observer(move |event: &Event| {
        observed_clone
            .lock()
            .unwrap()
            .push(event.topic.as_str().to_string());
    });

    let result = event_loop
        .process_parse_result(crate::event_reader::ParseResult {
            events: vec![make_work_done_event()],
            malformed: vec![],
        })
        .expect("process_parse_result should succeed");

    // The contract was rejected (no task in store, so task validation fails)
    assert!(
        !result.contract_rejections.is_empty(),
        "Expected contract rejections for missing task"
    );

    // The original work.done is NOT in accepted_events.
    assert!(
        !result
            .accepted_events
            .iter()
            .any(|e| e.topic.as_str() == "work.done"),
        "Original work.done must not be accepted when contract rejects it"
    );

    // had_rejected_events is true and had_raw_events is true.
    assert!(
        result.had_rejected_events,
        "had_rejected_events should be true"
    );
    assert!(
        result.had_raw_events,
        "had_raw_events should be true (rejected events count as observed)"
    );

    // had_events is false because the original event was rejected, not accepted.
    assert!(
        !result.had_events,
        "had_events should be false (no accepted events)"
    );

    // The bus observer saw the diagnostic and guidance events.
    let observed_topics = observed.lock().unwrap().clone();
    assert!(
        observed_topics
            .iter()
            .any(|t| t == "event.execution_contract.rejected"),
        "Diagnostic event should be published. observed: {:?}",
        observed_topics
    );
    assert!(
        observed_topics.iter().any(|t| t == "human.guidance"),
        "Guidance event should be published. observed: {:?}",
        observed_topics
    );
    // The original work.done event was NOT published to the bus.
    assert!(
        !observed_topics.iter().any(|t| t == "work.done"),
        "Original work.done must not be published. observed: {:?}",
        observed_topics
    );
}

#[test]
fn test_contract_rejection_with_trivial_step_passes() {
    // A `trivial` step is in `allow_empty_for_steps` so the git evidence
    // check is skipped. With no git repo and trivial step, the contract
    // should still fail on task validation (no task in store) — confirming
    // that `allow_empty_for_steps` only relaxes the git check, not task
    // validation.
    use ralph_proto::Event;
    let config = contract_enabled_config();
    let mut event_loop = EventLoop::new(config);

    let observed: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed_clone = std::sync::Arc::clone(&observed);
    event_loop.bus().add_observer(move |event: &Event| {
        observed_clone
            .lock()
            .unwrap()
            .push(event.topic.as_str().to_string());
    });

    let mut event = make_work_done_event();
    event.payload = Some(
        r#"{"plan_name":"p","plan_path":"/p","task_id":"t1","task_key":"k1","step":"trivial"}"#
            .to_string(),
    );

    let result = event_loop
        .process_parse_result(crate::event_reader::ParseResult {
            events: vec![event],
            malformed: vec![],
        })
        .expect("process_parse_result should succeed");

    // Task validation still rejects (no task in store), so work.done rejected.
    assert!(
        !result.contract_rejections.is_empty(),
        "Task validation must still reject even with trivial step"
    );
    assert!(
        result.had_rejected_events,
        "had_rejected_events should be true"
    );
    let observed_topics = observed.lock().unwrap().clone();
    assert!(
        observed_topics
            .iter()
            .any(|t| t == "event.execution_contract.rejected"),
        "Diagnostic event should fire"
    );
}

#[test]
fn test_contract_disabled_does_not_set_had_rejected_events() {
    // When execution contracts are disabled and origin validation accepts
    // the event, no validation-layer rejection should be reported.
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done"]
event_loop:
  execution_contracts:
    enabled: false
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    let result = event_loop
        .process_parse_result(crate::event_reader::ParseResult {
            events: vec![make_work_done_event()],
            malformed: vec![],
        })
        .expect("process_parse_result should succeed");

    assert!(
        result.contract_rejections.is_empty(),
        "No rejections when contract disabled"
    );
    assert!(
        !result.had_rejected_events,
        "had_rejected_events should be false when contract disabled"
    );
}

#[test]
fn test_contract_rejection_publishes_targeted_retry_to_source_hat() {
    // When executor's `work.done` is rejected, a regular targeted recovery
    // event must be published to executor's pending queue. This is the
    // characterization test for the gap fixed by 2026-06-04 plan U2.
    use ralph_proto::Event;
    let config = contract_enabled_config();
    let mut event_loop = EventLoop::new(config);

    // Capture every event seen on the bus to assert guidance is still
    // persisted for operator visibility.
    let observed: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed_clone = std::sync::Arc::clone(&observed);
    event_loop.bus().add_observer(move |event: &Event| {
        observed_clone
            .lock()
            .unwrap()
            .push(event.topic.as_str().to_string());
    });

    let result = event_loop
        .process_parse_result(crate::event_reader::ParseResult {
            events: vec![make_work_done_event()],
            malformed: vec![],
        })
        .expect("process_parse_result should succeed");

    let observed_topics = observed.lock().unwrap().clone();

    // The contract was rejected.
    assert!(
        !result.contract_rejections.is_empty(),
        "Expected contract rejections for missing task"
    );

    // The executor's pending queue must contain a regular recovery event
    // with `target=executor`.
    let executor_id = HatId::new("executor");
    let pending = event_loop
        .bus
        .peek_pending(&executor_id)
        .cloned()
        .unwrap_or_default();
    let targeted_retry = pending.iter().find(|e| {
        e.topic.as_str() != "human.guidance"
            && e.target.as_ref().map(|t| t.as_str()) == Some("executor")
    });
    assert!(
        targeted_retry.is_some(),
        "Rejected work.done must publish a targeted recovery event to executor's pending queue. \
         Pending events: {:?}",
        pending
            .iter()
            .map(|e| (e.topic.as_str(), e.target.as_ref().map(|t| t.as_str())))
            .collect::<Vec<_>>()
    );
    // The recovery event must mention the rejected topic so executor can
    // reason about what to re-emit.
    let payload = targeted_retry.unwrap().payload.as_str();
    assert!(
        payload.contains("work.done"),
        "Recovery event payload must mention the rejected topic 'work.done'. payload={}",
        payload
    );
    // human.guidance is still persisted for operator visibility.
    assert!(
        observed_topics.iter().any(|t| t == "human.guidance"),
        "human.guidance must still be published for operator visibility. observed: {:?}",
        observed_topics
    );
    // The structured diagnostic event is also published.
    assert!(
        observed_topics
            .iter()
            .any(|t| t == "event.execution_contract.rejected"),
        "Diagnostic event must be published. observed: {:?}",
        observed_topics
    );
}

#[test]
fn test_contract_rejection_activates_source_hat_for_next_prompt() {
    // After a rejected `work.done`, the next active hat must be executor
    // (via targeted retry), not the Ralph fallback. Today this assertion
    // fails because only `human.guidance` is published and it is partitioned
    // away from active hat selection.
    let config = contract_enabled_config();
    let mut event_loop = EventLoop::new(config);

    let result = event_loop
        .process_parse_result(crate::event_reader::ParseResult {
            events: vec![make_work_done_event()],
            malformed: vec![],
        })
        .expect("process_parse_result should succeed");
    assert!(
        !result.contract_rejections.is_empty(),
        "Expected contract rejections for missing task"
    );

    let active_hat_id = event_loop.get_active_hat_id();
    assert_eq!(
        active_hat_id.as_str(),
        "executor",
        "After rejected work.done, the next active hat must be the source hat \
         (executor) via targeted retry, not Ralph fallback. Got: {}",
        active_hat_id.as_str()
    );
}

#[test]
fn test_contract_rejection_does_not_activate_reviewer() {
    // Regression guard: even though the contract path publishes a targeted
    // retry to executor, reviewer must not be activated by a rejected
    // `work.done`. The original event must stay out of the bus.
    let config = contract_enabled_config();
    let mut event_loop = EventLoop::new(config);

    let result = event_loop
        .process_parse_result(crate::event_reader::ParseResult {
            events: vec![make_work_done_event()],
            malformed: vec![],
        })
        .expect("process_parse_result should succeed");
    assert!(
        !result.contract_rejections.is_empty(),
        "Expected contract rejections for missing task"
    );

    let reviewer_id = HatId::new("reviewer");
    let reviewer_pending = event_loop
        .bus
        .peek_pending(&reviewer_id)
        .cloned()
        .unwrap_or_default();
    assert!(
        !reviewer_pending
            .iter()
            .any(|e| e.topic.as_str() == "work.done"),
        "Reviewer must not receive a rejected work.done. Pending: {:?}",
        reviewer_pending
            .iter()
            .map(|e| e.topic.as_str())
            .collect::<Vec<_>>()
    );
    let active_hat_id = event_loop.get_active_hat_id();
    assert_ne!(
        active_hat_id.as_str(),
        "reviewer",
        "Reviewer must not be activated by rejected work.done"
    );
}

#[test]
fn test_valid_work_done_directly_published_activates_reviewer() {
    // Regression guard for the accepted path: a valid `work.done` published
    // directly to the bus (bypassing contract validation, which would
    // require real task/git setup) must still activate reviewer via the
    // registry's trigger mapping. This proves the fix to U2 does not regress
    // the accepted path.
    use ralph_proto::Event;
    let config = contract_enabled_config();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("work.done", "valid work complete"));

    let active_hat_id = event_loop.get_active_hat_id();
    assert_eq!(
        active_hat_id.as_str(),
        "reviewer",
        "A valid work.done event must activate reviewer"
    );
}

// -------------------------------------------------------------------------
// U6 (2026-06-18-004 plan, R3): `fix.applied` execution contract
// with `require_git_change.mode: commit_only`. Pins the perky-maple
// P2-3 commit-count drift path — the agent emitted `commit_count=0`
// while the real commit was still in flight. The contract MUST
// require a real commit before downstream review can proceed.
// The execution_contract.rs error messages now use the dynamic
// `event.topic` (no more hardcoded `work.done`) so the same
// message surface covers `fix.applied`.
// -------------------------------------------------------------------------

#[test]
fn u6_fix_applied_missing_payload_field_rejected_with_dynamic_topic() {
    // Pin U6: a `fix.applied` payload missing a required field
    // must produce a finding whose message uses `fix.applied`,
    // not the legacy hardcoded `work.done`.
    use crate::config::{
        ExecutionContractRule,
        execution_contracts::{
            ContractRejectConfig, GitChangeRequirement, TaskCompletionRequirement,
            TestEvidenceRequirement,
        },
    };
    use crate::execution_contract::validate_execution_contract;

    let rule = ExecutionContractRule {
        require_payload_fields: vec!["commit_count".into(), "fix_round".into()],
        require_task: TaskCompletionRequirement::default(),
        require_git_change: GitChangeRequirement::default(),
        require_test_evidence: TestEvidenceRequirement::default(),
        reject: ContractRejectConfig::default(),
    };

    let event = Event::new(
        "fix.applied",
        r#"{"plan_name":"p1","task_id":"t1","task_key":"k1","step":"step-01","applied_count":1,"failed_count":0,"commit_count":1}"#,
    );

    let decision = validate_execution_contract(
        &event,
        &rule,
        std::path::Path::new("/nonexistent"),
        "loop-1",
        std::path::Path::new("/nonexistent/tasks.jsonl"),
        None, // hat_id
        &crate::execution_contract::DefaultGitEvidenceProvider,
        None, // loop_start_sha
    );

    match decision {
        crate::execution_contract::ExecutionContractDecision::Reject(findings) => {
            assert!(
                !findings.is_empty(),
                "fix.applied missing-payload-field MUST produce at least one finding"
            );
            let finding = &findings[0];
            assert!(
                finding.message.contains("fix.applied"),
                "fix.applied contract violation MUST mention fix.applied in the recovery hint, got: {}",
                finding.message
            );
            assert!(
                !finding.message.contains("work.done"),
                "fix.applied contract violation MUST NOT mention the legacy work.done, got: {}",
                finding.message
            );
        }
        other => panic!("expected Reject decision, got {:?}", other),
    }
}

#[test]
fn u6_fix_applied_contract_present_in_ce_executor_serial_preset() {
    // Static preset-level assertion: the
    // `ce-executor-serial` preset MUST declare a
    // `fix.applied` execution contract with
    // `commit_only` git-evidence mode. Without this the
    // perky-maple P2-3 drift path re-opens.
    use crate::config::RalphConfig;
    let yaml_text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../presets/en/ce-executor-serial.yml"),
    )
    .expect("ce-executor-serial.yml must be readable");
    let config = RalphConfig::parse_yaml(&yaml_text)
        .expect("ce-executor-serial.yml must parse");

    let contract = config
        .event_loop
        .execution_contracts
        .as_ref()
        .expect("ce-executor-serial must declare execution_contracts");
    let rule = contract
        .rules
        .get("fix.applied")
        .expect("ce-executor-serial MUST declare a fix.applied execution contract rule (U6)");

    assert!(
        rule.require_payload_fields.contains(&"commit_count".to_string()),
        "fix.applied rule MUST require commit_count, got {:?}",
        rule.require_payload_fields
    );
    assert!(
        rule.require_payload_fields.contains(&"fix_round".to_string()),
        "fix.applied rule MUST require fix_round, got {:?}",
        rule.require_payload_fields
    );
    assert_eq!(
        rule.require_git_change.mode, "commit_only",
        "fix.applied rule MUST use commit_only mode (NOT diff_or_commit and NOT fictional strict), got {:?}",
        rule.require_git_change.mode
    );
}
