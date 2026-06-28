//! Tests for chain_validation.

use super::common::*;
use super::*;

#[test]
fn test_chain_validation_rejects_completion_without_required_events() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.required_events = vec!["plan.approved".to_string(), "all.built".to_string()];
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Only emit plan.approved, missing all.built
    write_event_to_jsonl(&events_path, "plan.approved", "OK");
    let _ = event_loop.process_events_from_jsonl();

    // Now try to complete
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "LOOP_COMPLETE should be rejected when required events are missing"
    );
}

#[test]
fn test_chain_validation_accepts_completion_with_all_required_events() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.required_events = vec!["plan.approved".to_string(), "all.built".to_string()];
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(&mut event_loop, &["plan.approved", "all.built"]);

    // Emit both required events across iterations
    write_event_to_jsonl(&events_path, "plan.approved", "OK");
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(&events_path, "all.built", "Done");
    let _ = event_loop.process_events_from_jsonl();

    // Now complete
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "LOOP_COMPLETE should be accepted when all required events have been seen"
    );
}

#[test]
fn test_chain_validation_tracks_topics_across_iterations() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.required_events = vec![
        "research.complete".to_string(),
        "plan.approved".to_string(),
        "all.built".to_string(),
    ];
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(
        &mut event_loop,
        &["research.complete", "plan.approved", "all.built"],
    );

    // Iteration 1: research.complete
    write_event_to_jsonl(&events_path, "research.complete", "findings");
    let _ = event_loop.process_events_from_jsonl();

    // Iteration 2: plan.approved
    write_event_to_jsonl(&events_path, "plan.approved", "ok");
    let _ = event_loop.process_events_from_jsonl();

    // Iteration 3: all.built + LOOP_COMPLETE
    write_event_to_jsonl(&events_path, "all.built", "done");
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Topics should be tracked across iterations"
    );
}

#[test]
fn test_chain_validation_empty_required_events_allows_completion() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default(); // No required_events
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Empty required_events should allow completion (backward compatible)"
    );
}

#[test]
fn test_chain_validation_injects_correction_on_rejection() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.required_events = vec!["plan.approved".to_string()];
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Try to complete without the required event
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Should reject completion");

    // P0-2 (2026-06-23-003 plan): completion rejection now routes
    // through the deterministic-correction path. The rejection
    // signal lives in `state.prompt_context.correction_blocks`
    // (rendered into the next prompt as `## ORCHESTRATOR CORRECTION`)
    // instead of being published on the EventBus as `task.resume`.
    assert!(
        !event_loop
            .state()
            .prompt_context
            .correction_blocks
            .is_empty(),
        "completion rejection must inject a CorrectionContext into state.prompt_context (P0-2)"
    );
}

#[test]
fn test_loop_cancel_terminates_without_chain_validation() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.cancellation_promise = "loop.cancel".to_string();
    config.event_loop.required_events = vec!["plan.approved".to_string(), "all.built".to_string()];
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(&mut event_loop, &["loop.cancel", "plan.approved", "all.built"]);

    // Send loop.cancel without any required events seen
    write_event_to_jsonl(&events_path, "loop.cancel", "rejected by human");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_cancellation_event();
    assert_eq!(
        reason,
        Some(TerminationReason::Cancelled),
        "loop.cancel should terminate without chain validation"
    );
}

#[test]
fn test_default_publishes_satisfies_required_events_for_completion() {
    use std::collections::HashMap;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.required_events = vec!["plan.draft".to_string(), "all.built".to_string()];

    let mut hats = HashMap::new();
    hats.insert(
        "planner".to_string(),
        crate::config::HatConfig {
            name: "planner".to_string(),
            description: Some("Plans work".to_string()),
            triggers: vec!["research.complete".to_string()],
            publishes: vec!["plan.draft".to_string()],
            instructions: "Plan".to_string(),
            extra_instructions: vec![],
            backend: None,
            backend_args: None,
            default_publishes: Some("plan.draft".to_string()),
            max_activations: None,
            scratchpad: None,
            disallowed_tools: vec![],
            timeout: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            phase_triggers: None,
            ..Default::default()
        },
    );
    config.hats = hats;

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(&mut event_loop, &["plan.draft", "all.built"]);

    // Simulate: planner wrote no events, default_publishes injects plan.draft
    let planner_id = HatId::new("planner");
    event_loop.check_default_publishes(&planner_id);

    // Then all.built arrives via JSONL
    write_event_to_jsonl(&events_path, "all.built", "done");
    let _ = event_loop.process_events_from_jsonl();

    // Now LOOP_COMPLETE should be accepted (plan.draft was from default_publishes)
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "default_publishes events should satisfy required_events chain validation"
    );
}

#[test]
fn test_default_publishes_completion_promise_triggers_termination() {
    use std::collections::HashMap;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.completion_promise = "LOOP_COMPLETE".to_string();
    config.event_loop.required_events = vec!["all.built".to_string()];

    let mut hats = HashMap::new();
    hats.insert(
        "final_committer".to_string(),
        crate::config::HatConfig {
            name: "FinalCommitter".to_string(),
            description: Some("Verifies all work is complete".to_string()),
            triggers: vec!["all.built".to_string()],
            publishes: vec!["LOOP_COMPLETE".to_string()],
            instructions: "Verify and complete".to_string(),
            extra_instructions: vec![],
            backend: None,
            backend_args: None,
            default_publishes: Some("LOOP_COMPLETE".to_string()),
            max_activations: None,
            scratchpad: None,
            disallowed_tools: vec![],
            timeout: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            phase_triggers: None,
            ..Default::default()
        },
    );
    config.hats = hats;

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(&mut event_loop, &["all.built", "LOOP_COMPLETE"]);

    // Satisfy required_events: all.built arrives via JSONL
    write_event_to_jsonl(&events_path, "all.built", "done");
    let _ = event_loop.process_events_from_jsonl();

    // Set active hat so check_default_publishes targets the right hat
    event_loop.state.last_active_hat_ids = vec![HatId::new("final_committer")];

    // Simulate: final_committer wrote no events, default_publishes injects LOOP_COMPLETE
    let hat_id = HatId::new("final_committer");
    event_loop.check_default_publishes(&hat_id);

    // completion_requested should be set directly by check_default_publishes
    // (not requiring a JSONL round-trip)
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "default_publishes of completion_promise should trigger termination directly, \
         not just publish to the bus where it would be lost"
    );
}

#[test]
fn test_loop_cancel_exit_code_is_zero() {
    assert_eq!(
        TerminationReason::Cancelled.exit_code(),
        0,
        "Cancelled should have exit code 0"
    );
}

#[test]
fn test_loop_cancel_is_not_success() {
    assert!(
        !TerminationReason::Cancelled.is_success(),
        "Cancelled should not be a success"
    );
}

#[test]
fn test_loop_cancel_takes_priority_over_completion() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.cancellation_promise = "loop.cancel".to_string();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(&mut event_loop, &["loop.cancel", "LOOP_COMPLETE"]);

    // Both loop.cancel and LOOP_COMPLETE in same batch
    write_event_to_jsonl(&events_path, "loop.cancel", "rejected");
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();

    // Cancellation should take priority (checked first)
    let cancel_reason = event_loop.check_cancellation_event();
    assert_eq!(
        cancel_reason,
        Some(TerminationReason::Cancelled),
        "Cancellation should take priority over completion"
    );
}

#[test]
fn test_loop_cancel_disabled_when_empty_string() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.cancellation_promise = String::new(); // Disabled
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // loop.cancel should pass through as a normal event (no termination)
    write_event_to_jsonl(&events_path, "loop.cancel", "rejected");
    let _ = event_loop.process_events_from_jsonl();

    let reason = event_loop.check_cancellation_event();
    assert_eq!(
        reason, None,
        "loop.cancel should not trigger cancellation when disabled"
    );
}
