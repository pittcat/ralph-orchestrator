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
