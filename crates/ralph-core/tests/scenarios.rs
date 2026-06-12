//! E2E scenario tests for event-loop redesign.
//!
//! Tests cover:
//! - Solo mode (Ralph with no hats)
//! - Multi-hat delegation
//! - Orphaned event handling
//! - Default publishes fallback
//! - Mixed backends
//! - AutoResearch workflow guards

use ralph_core::testing::{MockBackend, Scenario, ScenarioRunner};
use ralph_core::{EventLoop, EventParser, HatConfig, LoopContext, RalphConfig};
use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize)]
struct ScenarioYaml {
    name: String,
    description: String,
    config: ConfigYaml,
    mock_responses: Vec<String>,
    #[serde(default)]
    checkpoints: Vec<CheckpointYaml>,
    expected: ExpectedYaml,
}

#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize)]
struct ConfigYaml {
    prompt_file: String,
    max_iterations: u32,
    #[serde(default)]
    hats: serde_yaml::Value,
    #[serde(default)]
    event_loop: serde_yaml::Value,
    #[serde(default)]
    core: serde_yaml::Value,
    #[serde(default)]
    tasks: serde_yaml::Value,
    #[serde(default)]
    topic_owners: serde_yaml::Value,
    #[serde(default)]
    topic_format_whitelist: serde_yaml::Value,
}

#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize)]
struct ExpectedYaml {
    iterations: usize,
    events: Vec<EventYaml>,
    #[serde(default)]
    workflow_progress: Vec<WorkflowProgressYaml>,
    completion: bool,
}

#[allow(dead_code)] // Test infrastructure - fields used for YAML deserialization
#[derive(Debug, Deserialize)]
struct EventYaml {
    topic: String,
}

#[derive(Debug, Deserialize, Default)]
struct CheckpointYaml {
    after_response: usize,
    #[serde(default)]
    workflow_progress: Vec<WorkflowProgressYaml>,
    #[serde(default)]
    completion_rejected: bool,
}

#[derive(Debug, Deserialize)]
struct WorkflowProgressYaml {
    chain: String,
    phase: usize,
    #[serde(default)]
    instance: Option<String>,
}

fn load_scenario(path: &str) -> ScenarioYaml {
    let content =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e));
    serde_yaml::from_str(&content).unwrap_or_else(|e| panic!("Failed to parse {}: {}", path, e))
}

fn run_scenario(yaml: ScenarioYaml) {
    let backend = MockBackend::new(yaml.mock_responses);
    let runner = ScenarioRunner::new(backend.clone());

    let mut config = RalphConfig::default();
    config.max_iterations = Some(yaml.config.max_iterations);
    config.prompt_file = Some(yaml.config.prompt_file);

    let scenario =
        Scenario::new(yaml.name.clone(), config).with_iterations(yaml.expected.iterations);

    let trace = runner.run(&scenario);

    // Verify iteration count
    assert_eq!(
        trace.iterations, yaml.expected.iterations,
        "{}: Expected {} iterations, got {}",
        yaml.name, yaml.expected.iterations, trace.iterations
    );

    // Verify backend was called
    assert!(
        backend.execution_count() > 0,
        "{}: Backend should have been called",
        yaml.name
    );

    println!("✓ {} passed", yaml.description);
}

/// Runs a scenario that validates workflow guard behavior by feeding parsed
/// events through a real EventLoop and asserting on workflow progress.
fn run_workflow_guard_scenario(yaml: ScenarioYaml) {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().unwrap();
    let ralph_dir = temp_dir.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    let events_path = ralph_dir.join("events.jsonl");

    // Build RalphConfig from the YAML config section
    let mut config = RalphConfig::default();
    config.max_iterations = Some(yaml.config.max_iterations);
    config.prompt_file = Some(yaml.config.prompt_file);
    // Parse hats if present (inject map key as name if missing)
    if !yaml.config.hats.is_null() {
        if let Ok(hat_map) = serde_yaml::from_value::<
            std::collections::HashMap<String, serde_yaml::Value>,
        >(yaml.config.hats.clone())
        {
            let mut hats = std::collections::HashMap::new();
            for (hat_id, mut hat_value) in hat_map {
                if let Some(map) = hat_value.as_mapping_mut() {
                    if !map.contains_key(&serde_yaml::Value::String("name".to_string())) {
                        map.insert(
                            serde_yaml::Value::String("name".to_string()),
                            serde_yaml::Value::String(hat_id.clone()),
                        );
                    }
                }
                let hat_config: HatConfig = serde_yaml::from_value(hat_value).unwrap_or_else(|e| {
                    panic!("Failed to parse hat config for '{}': {}", hat_id, e)
                });
                hats.insert(hat_id, hat_config);
            }
            config.hats = hats;
        }
    }
    if !yaml.config.event_loop.is_null() {
        config.event_loop = serde_yaml::from_value(yaml.config.event_loop).unwrap();
    }

    let context = LoopContext::primary(temp_dir.path().to_path_buf());

    let mut event_loop = EventLoop::with_context(config, context);
    event_loop.initialize("Test");

    let parser = EventParser::new();

    for (idx, response) in yaml.mock_responses.iter().enumerate() {
        // Simulate hat execution so isolated mode scope enforcement is active.
        // build_prompt() consumes pending events from the bus, matching real loop behavior.
        if let Some(hat) = event_loop.next_hat() {
            let hat = hat.clone();
            let _ = event_loop.build_prompt(&hat);
            let _ = event_loop.process_output(&hat, "", true);
        }

        let events = parser.parse(response);
        {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path)
                .unwrap();
            for event in &events {
                let json = serde_json::json!({
                    "topic": event.topic,
                    "payload": event.payload,
                    "ts": "2024-01-01T00:00:00Z",
                });
                writeln!(file, "{}", json).unwrap();
            }
        }

        let _result = event_loop.process_events_from_jsonl();

        // Evaluate checkpoints tied to this response index (1-based in YAML)
        for checkpoint in &yaml.checkpoints {
            if checkpoint.after_response == idx + 1 {
                for progress in &checkpoint.workflow_progress {
                    let instance = progress.instance.as_deref();
                    let actual_phase = event_loop
                        .state()
                        .workflow_progress
                        .get_phase(&progress.chain, instance);
                    assert_eq!(
                        actual_phase,
                        Some(progress.phase),
                        "{}: After response {}, expected workflow progress phase {} for chain '{}', got {:?}",
                        yaml.name,
                        idx + 1,
                        progress.phase,
                        progress.chain,
                        actual_phase
                    );
                }

                if checkpoint.completion_rejected {
                    let reason = event_loop.check_completion_event();
                    assert!(
                        reason.is_none(),
                        "{}: After response {}, expected LOOP_COMPLETE to be rejected, but got {:?}",
                        yaml.name,
                        idx + 1,
                        reason
                    );
                }
            }
        }
    }

    // Verify all expected events were seen (accepted) at least once
    for expected_event in &yaml.expected.events {
        assert!(
            event_loop
                .state()
                .seen_topics
                .contains(&expected_event.topic),
            "{}: Expected event '{}' to be seen (accepted), but it was not recorded",
            yaml.name,
            expected_event.topic
        );
    }

    // Verify final workflow progress
    for progress in &yaml.expected.workflow_progress {
        let instance = progress.instance.as_deref();
        let actual_phase = event_loop
            .state()
            .workflow_progress
            .get_phase(&progress.chain, instance);
        assert_eq!(
            actual_phase,
            Some(progress.phase),
            "{}: Expected final workflow progress phase {} for chain '{}', got {:?}",
            yaml.name,
            progress.phase,
            progress.chain,
            actual_phase
        );
    }

    // Verify completion behavior
    if yaml.expected.completion {
        let reason = event_loop.check_completion_event();
        assert!(
            reason.is_some(),
            "{}: Expected LOOP_COMPLETE to be accepted, but it was rejected or not present",
            yaml.name
        );
    } else {
        let reason = event_loop.check_completion_event();
        assert!(
            reason.is_none(),
            "{}: Expected LOOP_COMPLETE to be rejected, but got {:?}",
            yaml.name,
            reason
        );
    }

    // Verify iteration count matches the number of mock responses
    assert_eq!(
        yaml.mock_responses.len(),
        yaml.expected.iterations,
        "{}: Expected {} iterations, but scenario has {} mock responses",
        yaml.name,
        yaml.expected.iterations,
        yaml.mock_responses.len()
    );

    println!("✓ {} passed", yaml.description);
}

#[test]
fn test_solo_mode() {
    let yaml = load_scenario("tests/scenarios/solo_mode.yml");
    run_scenario(yaml);
}

#[test]
fn test_multi_hat() {
    let yaml = load_scenario("tests/scenarios/multi_hat.yml");
    run_scenario(yaml);
}

#[test]
fn test_orphaned_events() {
    let yaml = load_scenario("tests/scenarios/orphaned_events.yml");
    run_scenario(yaml);
}

#[test]
fn test_default_publishes() {
    let yaml = load_scenario("tests/scenarios/default_publishes.yml");
    run_scenario(yaml);
}

#[test]
fn test_mixed_backends() {
    let yaml = load_scenario("tests/scenarios/mixed_backends.yml");
    run_scenario(yaml);
}

#[test]
fn test_autoresearch_guard() {
    let yaml = load_scenario("tests/scenarios/autoresearch_guard.yml");
    run_workflow_guard_scenario(yaml);
}

// BDD scenario for feat-ralph-cli-agent-reference-split has been removed.
// Real CLI acceptance is covered by integration tests in
// crates/ralph-cli/tests/integration_agent_reference.rs.

#[test]
fn test_isolated_multi_hat() {
    let yaml = load_scenario("tests/scenarios/isolated_multi_hat.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_isolated_boundary_violation() {
    let yaml = load_scenario("tests/scenarios/isolated_boundary_violation.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_workflow_activation_contract_re_emit_trap() {
    // WAC-U8 AE1 (2026-06-12-002): a hat that triggers on a
    // topic published by another hat and does not declare that
    // topic in its own `publishes` is a re-emit trap. The
    // strict WAC lint must surface this as a finding.
    use ralph_core::preset_lint::run_workflow_activation_contract;
    let config_yaml = r#"
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["queue.advance"]
  executor:
    name: "Executor"
    triggers: ["queue.advance"]
    publishes: ["work.done"]
"#;
    let config: ralph_core::RalphConfig =
        serde_yaml::from_str(config_yaml).expect("parse WAC AE1 fixture");
    let findings = run_workflow_activation_contract(&config, true);
    let re_emit = findings
        .iter()
        .find(|f| f.id == "preset.re_emit_trap")
        .expect("strict WAC must surface the re_emit_trap finding for executor+queue.advance");
    assert_eq!(re_emit.hat.as_deref(), Some("executor"));
    assert_eq!(re_emit.topic.as_deref(), Some("queue.advance"));
}

#[test]
fn test_workflow_activation_contract_handoff_pairing_broken() {
    // WAC-U8 AE1 sibling: a handoff (unique consumer) whose
    // publishes do not reach a terminal topic is flagged by
    // R4. The executor consumes `work.ready` uniquely and
    // emits a topic that no other hat triggers on, so R4 fires.
    use ralph_core::preset_lint::run_workflow_activation_contract;
    let config_yaml = r#"
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["executor.dead_end"]
"#;
    let config: ralph_core::RalphConfig =
        serde_yaml::from_str(config_yaml).expect("parse WAC AE1 handoff fixture");
    let findings = run_workflow_activation_contract(&config, true);
    let finding = findings
        .iter()
        .find(|f| f.id == "preset.handoff_pairing_broken")
        .expect("strict WAC must surface the handoff_pairing_broken finding for work.ready+executor");
    assert_eq!(finding.topic.as_deref(), Some("work.ready"));
    assert_eq!(finding.hat.as_deref(), Some("executor"));
}

#[test]
fn test_workflow_activation_contract_null_payload_rejected() {
    // WAC-U8 AE3: a null `review.passed` payload is hard-rejected
    // by `event_policy::validate_event` even when the policy is
    // in Observe mode (KTD-9). The dispatcher never sees the
    // event in the validated stream.
    use ralph_core::config::{EventPolicyConfig, EventPolicyMode, ViolationAction};
    use ralph_core::{PolicyDecision, PolicyRuntimeState, validate_event};

    let mut config = EventPolicyConfig::default();
    config.enabled = true;
    config.mode = EventPolicyMode::Observe;
    config.on_violation = ViolationAction::RejectWithResume;

    let mut state = PolicyRuntimeState::default();
    let decision = validate_event("review.passed", None, &config, &mut state);
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "WAC R10 must RejectWithResume null review.passed, got {:?}",
        decision
    );
}

#[test]
fn test_workflow_activation_contract_handoff_priority_dispatch() {
    // WAC-U8 AE5: when the EventBus's priority pre-emption is
    // armed and the priority hat has a non-empty pending
    // queue, the dispatcher selects that hat immediately,
    // skipping the round-robin scan.
    use ralph_proto::{Event, EventBus, Hat, HatId};

    let mut bus = EventBus::new();
    for id in ["alpha", "beta", "gamma"] {
        bus.register(Hat::new(id, id).subscribe("work.*"));
    }
    for (id, label) in [("alpha", "a1"), ("beta", "b1"), ("gamma", "g1")] {
        bus.publish(Event::new("work", label).with_target(id));
    }
    let sel = bus
        .select_next_hat_with_pending(Some(&HatId::from("gamma")))
        .expect("priority pre-emption must select gamma");
    assert_eq!(sel.as_str(), "gamma");
}

#[test]
fn test_isolated_with_event_projection() {
    use std::io::Write;

    let yaml = load_scenario("tests/scenarios/isolated_with_event_projection.yml");

    let temp_dir = tempfile::tempdir().unwrap();
    let ralph_dir = temp_dir.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    let events_path = ralph_dir.join("events.jsonl");

    // Build RalphConfig from the YAML config section
    let mut config = RalphConfig::default();
    config.max_iterations = Some(yaml.config.max_iterations);
    config.prompt_file = Some(yaml.config.prompt_file);
    // Parse hats if present (inject map key as name if missing)
    if !yaml.config.hats.is_null() {
        if let Ok(hat_map) = serde_yaml::from_value::<
            std::collections::HashMap<String, serde_yaml::Value>,
        >(yaml.config.hats.clone())
        {
            let mut hats = std::collections::HashMap::new();
            for (hat_id, mut hat_value) in hat_map {
                if let Some(map) = hat_value.as_mapping_mut() {
                    if !map.contains_key(&serde_yaml::Value::String("name".to_string())) {
                        map.insert(
                            serde_yaml::Value::String("name".to_string()),
                            serde_yaml::Value::String(hat_id.clone()),
                        );
                    }
                }
                let hat_config: HatConfig = serde_yaml::from_value(hat_value).unwrap_or_else(|e| {
                    panic!("Failed to parse hat config for '{}': {}", hat_id, e)
                });
                hats.insert(hat_id, hat_config);
            }
            config.hats = hats;
        }
    }
    if !yaml.config.event_loop.is_null() {
        config.event_loop = serde_yaml::from_value(yaml.config.event_loop).unwrap();
    }
    if !yaml.config.core.is_null() {
        config.core = serde_yaml::from_value(yaml.config.core.clone()).unwrap();
    }
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let context = LoopContext::primary(temp_dir.path().to_path_buf());

    let mut event_loop = EventLoop::with_context(config, context);
    event_loop.initialize("Test");

    let parser = EventParser::new();

    for (idx, response) in yaml.mock_responses.iter().enumerate() {
        // Simulate hat execution so isolated mode scope enforcement is active.
        // build_prompt() consumes pending events from the bus, matching real loop behavior.
        if let Some(hat) = event_loop.next_hat() {
            let hat = hat.clone();
            let _ = event_loop.build_prompt(&hat);
            let _ = event_loop.process_output(&hat, "", true);
        }

        let events = parser.parse(response);
        {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path)
                .unwrap();
            for event in &events {
                let json = serde_json::json!({
                    "topic": event.topic,
                    "payload": event.payload,
                    "ts": "2024-01-01T00:00:00Z",
                });
                writeln!(file, "{}", json).unwrap();
            }
        }

        let _result = event_loop.process_events_from_jsonl();

        // Evaluate checkpoints tied to this response index (1-based in YAML)
        for checkpoint in &yaml.checkpoints {
            if checkpoint.after_response == idx + 1 {
                for progress in &checkpoint.workflow_progress {
                    let instance = progress.instance.as_deref();
                    let actual_phase = event_loop
                        .state()
                        .workflow_progress
                        .get_phase(&progress.chain, instance);
                    assert_eq!(
                        actual_phase,
                        Some(progress.phase),
                        "{}: After response {}, expected workflow progress phase {} for chain '{}', got {:?}",
                        yaml.name,
                        idx + 1,
                        progress.phase,
                        progress.chain,
                        actual_phase
                    );
                }

                if checkpoint.completion_rejected {
                    let reason = event_loop.check_completion_event();
                    assert!(
                        reason.is_none(),
                        "{}: After response {}, expected LOOP_COMPLETE to be rejected, but got {:?}",
                        yaml.name,
                        idx + 1,
                        reason
                    );
                }
            }
        }
    }

    // Verify all expected events were seen (accepted) at least once
    for expected_event in &yaml.expected.events {
        assert!(
            event_loop
                .state()
                .seen_topics
                .contains(&expected_event.topic),
            "{}: Expected event '{}' to be seen (accepted), but it was not recorded",
            yaml.name,
            expected_event.topic
        );
    }

    // Verify final workflow progress
    for progress in &yaml.expected.workflow_progress {
        let instance = progress.instance.as_deref();
        let actual_phase = event_loop
            .state()
            .workflow_progress
            .get_phase(&progress.chain, instance);
        assert_eq!(
            actual_phase,
            Some(progress.phase),
            "{}: Expected final workflow progress phase {} for chain '{}', got {:?}",
            yaml.name,
            progress.phase,
            progress.chain,
            actual_phase
        );
    }

    // Verify completion behavior
    if yaml.expected.completion {
        let reason = event_loop.check_completion_event();
        assert!(
            reason.is_some(),
            "{}: Expected LOOP_COMPLETE to be accepted, but it was rejected or not present",
            yaml.name
        );
    } else {
        let reason = event_loop.check_completion_event();
        assert!(
            reason.is_none(),
            "{}: Expected LOOP_COMPLETE to be rejected, but got {:?}",
            yaml.name,
            reason
        );
    }

    // Verify iteration count matches the number of mock responses
    assert_eq!(
        yaml.mock_responses.len(),
        yaml.expected.iterations,
        "{}: Expected {} iterations, but scenario has {} mock responses",
        yaml.name,
        yaml.expected.iterations,
        yaml.mock_responses.len()
    );

    // Verify projection file was created and contains expected events
    let projection_path = ralph_dir.join("projected-events.jsonl");
    assert!(
        projection_path.exists(),
        "{}: Expected projection file to exist at {:?}",
        yaml.name,
        projection_path
    );

    let projection_content = std::fs::read_to_string(&projection_path).unwrap();
    let lines: Vec<&str> = projection_content.trim().lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "{}: Expected 2 projected events, got {}",
        yaml.name,
        lines.len()
    );

    let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first["topic"], "experiment.planned");
    assert_eq!(first["payload"], "plan: \"Build feature\"");

    let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(second["topic"], "experiment.ready");
    assert_eq!(second["payload"], "status: \"done\"");

    println!("✓ {} passed", yaml.description);
}

// ──────────────────────────────────────────────────────────────────────
// 2026-06-07 plan U7: end-to-end recovery contract
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_ce_executor_recovery_scenario() {
    // The YAML ships a 5-mock-response loop that mirrors the
    // 2026-06-06 drift run: executor activated, no event in iter 1
    // (missing-event), valid work.done in iter 2, wave dispatch
    // in iter 3, LOOP_COMPLETE in iter 4.  The deeper per-hypothesis
    // assertions (origin guard, contract rejection, obligation
    // alignment) live in `ralph-cli/tests/ce_executor_recovery.rs`.
    // This scenario asserts the wire-level flow.
    let yaml = load_scenario("tests/scenarios/ce_executor_recovery.yml");
    run_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// 2026-06-09: O3 regression — verdict_gate keeps loop open on fail
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_verdict_gate_fail_keeps_loop_open() {
    // Defense-in-depth verification of the 2026-06-09 fix.
    // Three iterations exercise: pass, fail-without-rogue,
    // fail-with-rogue.  After the third (failing) response,
    // `completion_rejected: true` checkpoint confirms that
    // `check_completion_event` returns None — the LOOP_COMPLETE
    // is rejected by the verdict_gate because the most recent
    // `report.done` carried pass_or_fail="fail".
    let yaml = load_scenario("tests/scenarios/verdict_gate_fail_keeps_loop_open.yml");
    run_workflow_guard_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// ──────────────────────────────────────────────────────────────────────
// U6: Hat lifecycle contract — terminal events close activations
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_hat_lifecycle_contract() {
    // Verifies that terminal events (work.done, review.complete, LOOP_COMPLETE)
    // close hat activations as expected in a simple pipeline topology.
    let yaml = load_scenario("tests/scenarios/hat_lifecycle_contract.yml");
    run_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// U6: Preset static lint BDD — AE1 coverage
//
// Exercises real config parsing, HatRegistry construction, and
// RuntimeContractAggregator with strict preset_check_strict()
// through the same path that `ralph preset check --strict` uses.
// This is NOT a source-level string assertion — it runs the full
// authoring lint pipeline.
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_preset_static_lint_scenario() {
    use ralph_core::HatRegistry;
    use ralph_core::runtime_contract::{
        FindingSeverity, RuntimeContractAggregator, RuntimeContractStrictness,
    };

    let yaml = load_scenario("tests/scenarios/preset_static_lint.yml");

    // Build RalphConfig from the YAML config section (reuse workflow guard
    // helper pattern for hat parsing).
    let mut config = RalphConfig::default();
    config.max_iterations = Some(yaml.config.max_iterations);
    config.prompt_file = Some(yaml.config.prompt_file);
    if !yaml.config.hats.is_null() {
        if let Ok(hat_map) = serde_yaml::from_value::<
            std::collections::HashMap<String, serde_yaml::Value>,
        >(yaml.config.hats.clone())
        {
            let mut hats = std::collections::HashMap::new();
            for (hat_id, mut hat_value) in hat_map {
                if let Some(map) = hat_value.as_mapping_mut() {
                    if !map.contains_key(&serde_yaml::Value::String("name".to_string())) {
                        map.insert(
                            serde_yaml::Value::String("name".to_string()),
                            serde_yaml::Value::String(hat_id.clone()),
                        );
                    }
                }
                let hat_config: HatConfig = serde_yaml::from_value(hat_value)
                    .unwrap_or_else(|e| panic!("Failed to parse hat '{}': {}", hat_id, e));
                hats.insert(hat_id, hat_config);
            }
            config.hats = hats;
        }
    }
    if !yaml.config.event_loop.is_null() {
        config.event_loop = serde_yaml::from_value(yaml.config.event_loop).unwrap();
    }
    if !yaml.config.tasks.is_null() {
        config.tasks = serde_yaml::from_value(yaml.config.tasks).unwrap();
    }
    if !yaml.config.topic_owners.is_null() {
        config.topic_owners = serde_yaml::from_value(yaml.config.topic_owners).unwrap();
    }
    if !yaml.config.topic_format_whitelist.is_null() {
        config.topic_format_whitelist =
            serde_yaml::from_value(yaml.config.topic_format_whitelist).unwrap();
    }

    // Run the aggregator with strict preset_check_strict() — same path
    // as `ralph preset check --strict` and the run hard gate.
    let registry = HatRegistry::from_runtime_config(&config);
    let strictness = RuntimeContractStrictness::preset_check_strict();
    let report = RuntimeContractAggregator::aggregate(
        "bdd:preset_static_lint",
        &config,
        &registry,
        strictness,
    );

    // AE1: valid preset must pass strict lint.
    assert!(
        report.passed,
        "preset_static_lint BDD scenario must pass strict lint: {:?}",
        report
            .findings
            .iter()
            .filter(|f| matches!(f.severity, FindingSeverity::Error | FindingSeverity::Warn))
            .map(|f| format!("[{:?}] {}: {}", f.severity, f.id, f.message))
            .collect::<Vec<_>>()
    );
}

// ──────────────────────────────────────────────────────────────────────
// 2026-06-09: four P0 guards BDD scenarios (U1–U4)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_u1_partial_wave_dispatch() {
    let yaml = load_scenario("tests/scenarios/four-p0-guards/u1-partial-wave-dispatch.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_u2_ralph_pseudo_hat_rejection() {
    let yaml = load_scenario("tests/scenarios/four-p0-guards/u2-ralph-pseudo-hat-rejection.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_u3_topic_deny_rule() {
    let yaml = load_scenario("tests/scenarios/four-p0-guards/u3-topic-deny-rule.yml");
    run_workflow_guard_scenario(yaml);
}

#[test]
fn test_u4_plan_name_equality() {
    let yaml = load_scenario("tests/scenarios/four-p0-guards/u4-plan-name-equality.yml");
    run_workflow_guard_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// 2026-06-10: ce-executor worktree isolation BDD scenario (U4)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_ce_executor_worktree_isolation() {
    // U4 (2026-06-10): worktree isolation contract at the event-loop level.
    // The cross-process filesystem isolation is verified end-to-end by
    // `crates/ralph-cli/tests/integration_worktree_isolation.rs`. This
    // BDD scenario complements that test by exercising the event flow
    // with a worktree-mode-shaped config, ensuring no leakage at the
    // event-loop layer.
    let yaml = load_scenario("tests/scenarios/ce-executor-worktree-isolation.yml");
    run_scenario(yaml);
}

// ──────────────────────────────────────────────────────────────────────
// U2 of 2026-06-11-003: multi-hat isolation policy BDD scenario
//
// AE2: 4-hat preset with default (Coordinator) execution mode MUST
// be rejected by the strict preset lint aggregator with a single
// `lint.preset.multi_hat_requires_isolated` finding. The same
// finding shape drives the `ralph preset check` CLI surface, the
// `ralph preflight --check multi-hat-isolation` check, and the
// `ralph run` hard gate.
// ──────────────────────────────────────────────────────────────────────

#[test]
fn test_multi_hat_isolation_lint_bdd_4_hat_default_fails() {
    use ralph_core::HatRegistry;
    use ralph_core::preset_lint::finding_id::FINDING_MULTI_HAT_REQUIRES_ISOLATED;
    use ralph_core::runtime_contract::{
        FindingSeverity, RuntimeContractAggregator, RuntimeContractStrictness,
    };

    let yaml = load_scenario("tests/scenarios/multi_hat_isolation_lint.yml");

    // Build the resolved config exactly the way the lint aggregator
    // sees it. (Mirrors the test_preset_static_lint_scenario helper.)
    let mut config = RalphConfig::default();
    config.max_iterations = Some(yaml.config.max_iterations);
    config.prompt_file = Some(yaml.config.prompt_file);
    if !yaml.config.hats.is_null() {
        if let Ok(hat_map) = serde_yaml::from_value::<
            std::collections::HashMap<String, serde_yaml::Value>,
        >(yaml.config.hats.clone())
        {
            let mut hats = std::collections::HashMap::new();
            for (hat_id, mut hat_value) in hat_map {
                if let Some(map) = hat_value.as_mapping_mut()
                    && !map.contains_key(&serde_yaml::Value::String("name".to_string()))
                {
                    map.insert(
                        serde_yaml::Value::String("name".to_string()),
                        serde_yaml::Value::String(hat_id.clone()),
                    );
                }
                let hat_config: HatConfig = serde_yaml::from_value(hat_value)
                    .unwrap_or_else(|e| panic!("Failed to parse hat '{}': {}", hat_id, e));
                hats.insert(hat_id, hat_config);
            }
            config.hats = hats;
        }
    }
    if !yaml.config.event_loop.is_null() {
        config.event_loop = serde_yaml::from_value(yaml.config.event_loop).unwrap();
    }
    if !yaml.config.tasks.is_null() {
        config.tasks = serde_yaml::from_value(yaml.config.tasks).unwrap();
    }
    if !yaml.config.topic_owners.is_null() {
        config.topic_owners = serde_yaml::from_value(yaml.config.topic_owners).unwrap();
    }
    if !yaml.config.topic_format_whitelist.is_null() {
        config.topic_format_whitelist =
            serde_yaml::from_value(yaml.config.topic_format_whitelist).unwrap();
    }

    assert_eq!(
        config.hats.len(),
        4,
        "fixture must declare 4 hats for AE2 to be meaningful"
    );

    let registry = HatRegistry::from_runtime_config(&config);
    let strictness = RuntimeContractStrictness::preset_check_strict();
    let report = RuntimeContractAggregator::aggregate(
        "bdd:multi_hat_isolation_lint",
        &config,
        &registry,
        strictness,
    );

    // 4 hats, default Coordinator mode → aggregator must fail.
    assert!(
        !report.passed,
        "4-hat default coordinator preset MUST fail strict lint: {:?}",
        report
            .findings
            .iter()
            .map(|f| format!("[{:?}] {}: {}", f.severity, f.id, f.message))
            .collect::<Vec<_>>()
    );

    // Exactly one multi_hat_requires_isolated error finding, with the
    // expected details. This is the same shape the preflight check
    // and the run gate consume.
    let multi_hat_findings: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.id == format!("lint.{}", FINDING_MULTI_HAT_REQUIRES_ISOLATED))
        .collect();
    assert_eq!(
        multi_hat_findings.len(),
        1,
        "expected exactly one multi_hat_requires_isolated finding, got: {:?}",
        report
            .findings
            .iter()
            .map(|f| format!("[{:?}] {}: {}", f.severity, f.id, f.message))
            .collect::<Vec<_>>()
    );
    let finding = &multi_hat_findings[0];
    assert_eq!(finding.severity, FindingSeverity::Error);
    assert_eq!(
        finding.details.get("actual").map(String::as_str),
        Some("4"),
        "details.actual must be 4: {:?}",
        finding.details
    );
    assert_eq!(
        finding.details.get("limit").map(String::as_str),
        Some("3"),
        "details.limit must be 3: {:?}",
        finding.details
    );
    assert_eq!(
        finding.details.get("required_mode").map(String::as_str),
        Some("isolated"),
        "details.required_mode must be 'isolated': {:?}",
        finding.details
    );
}
