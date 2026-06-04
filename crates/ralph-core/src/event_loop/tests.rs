use super::*;

#[test]
fn test_initialization_routes_to_ralph_in_multihat_mode() {
    // Per "Hatless Ralph" architecture: When custom hats are defined,
    // Ralph is always the executor. Custom hats define topology only.
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.start", "build.done", "build.blocked"]
    publishes: ["build.task"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.initialize("Test prompt");

    // Per spec: In multi-hat mode, Ralph handles all iterations
    let next = event_loop.next_hat();
    assert!(next.is_some());
    assert_eq!(
        next.unwrap().as_str(),
        "ralph",
        "Multi-hat mode should route to Ralph"
    );

    // Verify Ralph's prompt includes the event
    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        prompt.contains("task.start"),
        "Ralph's prompt should include the event"
    );
}

#[test]
fn test_guidance_persists_across_iterations_solo_mode() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    let ralph_id = HatId::new("ralph");

    event_loop
        .bus
        .publish(Event::new("human.guidance", "Keep this in mind"));

    let prompt = event_loop.build_prompt(&ralph_id).unwrap();
    assert!(
        prompt.contains("## ROBOT GUIDANCE"),
        "Prompt should include guidance section"
    );
    assert!(
        prompt.contains("Keep this in mind"),
        "Prompt should include guidance payload"
    );
    assert!(
        !event_loop.has_pending_events(),
        "Guidance should not remain pending after prompt build"
    );

    let prompt_again = event_loop.build_prompt(&ralph_id).unwrap();
    assert!(
        prompt_again.contains("Keep this in mind"),
        "Guidance should persist across iterations"
    );
}

#[test]
fn test_guidance_persists_across_iterations_multi_hat_mode() {
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.start"]
    publishes: ["task.plan"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let ralph_id = HatId::new("ralph");

    event_loop
        .bus
        .publish(Event::new("human.guidance", "Focus on error handling"));

    let prompt = event_loop.build_prompt(&ralph_id).unwrap();
    assert!(
        prompt.contains("Focus on error handling"),
        "Prompt should include guidance payload"
    );

    let prompt_again = event_loop.build_prompt(&ralph_id).unwrap();
    assert!(
        prompt_again.contains("Focus on error handling"),
        "Guidance should persist across iterations in multi-hat mode"
    );
}

#[test]
fn test_guidance_persisted_to_scratchpad() {
    let dir = tempfile::tempdir().unwrap();
    let scratchpad_path = dir.path().join("scratchpad.md");

    let yaml = format!(
        r#"
core:
  workspace_root: "{}"
  scratchpad: "{}"
"#,
        dir.path().display(),
        scratchpad_path.display()
    );
    let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let ralph_id = HatId::new("ralph");

    // Publish guidance and build prompt to trigger persistence
    event_loop
        .bus
        .publish(Event::new("human.guidance", "Use the new API for auth"));

    let prompt = event_loop.build_prompt(&ralph_id).unwrap();
    assert!(
        prompt.contains("Use the new API for auth"),
        "Prompt should include guidance"
    );

    // Verify guidance was persisted to scratchpad file
    let scratchpad_content = std::fs::read_to_string(&scratchpad_path)
        .expect("Scratchpad file should exist after guidance persistence");
    assert!(
        scratchpad_content.contains("HUMAN GUIDANCE"),
        "Scratchpad should contain HUMAN GUIDANCE header"
    );
    assert!(
        scratchpad_content.contains("Use the new API for auth"),
        "Scratchpad should contain guidance text"
    );
}

#[test]
fn test_guidance_appends_to_existing_scratchpad() {
    let dir = tempfile::tempdir().unwrap();
    let scratchpad_path = dir.path().join("scratchpad.md");

    // Pre-populate scratchpad with existing content
    std::fs::write(&scratchpad_path, "## Existing Notes\n\nSome prior work.\n").unwrap();

    let yaml = format!(
        r#"
core:
  workspace_root: "{}"
  scratchpad: "{}"
"#,
        dir.path().display(),
        scratchpad_path.display()
    );
    let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let ralph_id = HatId::new("ralph");

    event_loop
        .bus
        .publish(Event::new("human.guidance", "Focus on error handling"));
    let _ = event_loop.build_prompt(&ralph_id).unwrap();

    let content = std::fs::read_to_string(&scratchpad_path).unwrap();
    assert!(
        content.starts_with("## Existing Notes"),
        "Existing scratchpad content should be preserved"
    );
    assert!(
        content.contains("Focus on error handling"),
        "New guidance should be appended"
    );
}

#[test]
fn test_hat_max_activations_emits_exhausted_event() {
    // Repro for issue #66: per-hat max_activations should prevent infinite reviewer loops.
    // Events are now published directly to the bus (simulating what ralph emit writes to JSONL
    // and process_events_from_jsonl publishes).
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    description: "Implements requested changes"
    triggers: ["work.start", "review.changes_requested"]
    publishes: ["implementation.done"]
  code_reviewer:
    name: "Code Reviewer"
    description: "Reviews changes and requests fixes"
    triggers: ["implementation.done"]
    publishes: ["review.changes_requested"]
    max_activations: 3
  escalator:
    name: "Escalator"
    description: "Handles exhausted hats"
    triggers: ["code_reviewer.exhausted"]
    publishes: []
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let ralph = HatId::new("ralph");

    // Seed the loop with an executor event.
    event_loop
        .bus
        .publish(Event::new("work.start", "begin").with_source(ralph.clone()));

    // Cycle: executor -> implementation.done; reviewer -> review.changes_requested.
    for _ in 0..3 {
        // Executor active.
        let _ = event_loop.build_prompt(&ralph).unwrap();
        // Simulate event from JSONL (ralph emit writes to file, process_events_from_jsonl publishes)
        event_loop
            .bus
            .publish(Event::new("implementation.done", "done"));

        // Reviewer active (up to max_activations=3).
        let prompt = event_loop.build_prompt(&ralph).unwrap();
        assert!(
            !prompt.contains("Event: code_reviewer.exhausted"),
            "Reviewer should not be exhausted yet"
        );
        event_loop
            .bus
            .publish(Event::new("review.changes_requested", "fix"));
    }

    // One more implementation.done should attempt a 4th reviewer activation.
    let _ = event_loop.build_prompt(&ralph).unwrap();
    event_loop
        .bus
        .publish(Event::new("implementation.done", "done"));

    let prompt = event_loop.build_prompt(&ralph).unwrap();
    assert!(
        prompt.contains("Event: code_reviewer.exhausted"),
        "Expected code_reviewer.exhausted to be emitted when max_activations is exceeded"
    );
    let escalator_id = HatId::new("escalator");
    assert!(
        event_loop
            .bus
            .peek_pending(&escalator_id)
            .is_some_and(|events| {
                events
                    .iter()
                    .any(|e| e.topic.as_str() == "code_reviewer.exhausted")
            }),
        "Expected code_reviewer.exhausted to be published for escalator"
    );

    // Further would-trigger events are dropped (no re-activation beyond max).
    let reviewer_id = HatId::new("code_reviewer");
    assert_eq!(
        *event_loop
            .state
            .hat_activation_counts
            .get(&reviewer_id)
            .unwrap_or(&0),
        3,
        "Reviewer should have exactly max activations recorded"
    );

    event_loop
        .bus
        .publish(Event::new("implementation.done", "done again").with_source(ralph.clone()));
    let prompt = event_loop.build_prompt(&ralph).unwrap();
    assert!(
        !prompt.contains("Event: implementation.done"),
        "Pending events for an exhausted hat should be dropped"
    );
    assert_eq!(
        *event_loop
            .state
            .hat_activation_counts
            .get(&reviewer_id)
            .unwrap_or(&0),
        3,
        "Reviewer should not be activated after exhaustion"
    );
}

#[test]
fn test_termination_max_iterations() {
    let yaml = r"
event_loop:
  max_iterations: 2
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.state.iteration = 2;

    assert_eq!(
        event_loop.check_termination(),
        Some(TerminationReason::MaxIterations)
    );
}

#[test]
fn test_hard_gate_terminates_after_max() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    // Below threshold — should not terminate
    event_loop.state.consecutive_hard_gates = 2;
    assert_eq!(event_loop.check_termination(), None);

    // At threshold — should terminate with Stopped
    event_loop.state.consecutive_hard_gates = 3;
    assert_eq!(
        event_loop.check_termination(),
        Some(TerminationReason::Stopped)
    );
}

#[test]
fn test_hard_gate_count_methods() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    assert_eq!(event_loop.state().consecutive_hard_gates, 0);

    event_loop.increment_hard_gate_count();
    assert_eq!(event_loop.state().consecutive_hard_gates, 1);

    event_loop.increment_hard_gate_count();
    assert_eq!(event_loop.state().consecutive_hard_gates, 2);

    event_loop.reset_hard_gate_count();
    assert_eq!(event_loop.state().consecutive_hard_gates, 0);
}

#[test]
fn test_completion_promise_detection() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    // Create scratchpad with all tasks completed (use absolute path, no set_current_dir)
    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(
        &scratchpad_path,
        "## Tasks\n- [x] Task 1 done\n- [x] Task 2 done\n",
    )
    .unwrap();

    // Configure event loop to use temp directory scratchpad
    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // LOOP_COMPLETE event with all tasks done - should terminate immediately
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Should terminate immediately when LOOP_COMPLETE + tasks verified"
    );
}

#[test]
fn test_completion_promise_with_open_tasks_in_scratchpad_still_terminates() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    // Create scratchpad with PENDING tasks ([ ] markers)
    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(
        &scratchpad_path,
        "## Tasks\n- [x] Task 1 done\n- [ ] Task 2 still pending\n",
    )
    .unwrap();

    // Configure event loop to use temp directory scratchpad
    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Scratchpad mode still trusts the agent's completion signal even with open checklist items.
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Scratchpad mode should still trust the agent's decision"
    );
}

#[test]
fn test_completion_promise_with_pending_tasks_in_task_store_is_rejected() {
    use crate::loop_context::LoopContext;
    use crate::task::{Task, TaskStatus};
    use crate::task_store::TaskStore;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let tasks_path = temp_dir.path().join(".ralph/agent/tasks.jsonl");

    // Create task store with one open and one closed task
    let mut store = TaskStore::load(&tasks_path).unwrap();
    let mut task1 = Task::new("Completed task".to_string(), 1);
    task1.status = TaskStatus::Closed;
    store.add(task1);

    let task2 = Task::new("Still open task".to_string(), 2);
    store.add(task2);
    store.save().unwrap();

    // Configure event loop with memories enabled and pointing to temp dir
    let mut config = RalphConfig::default();
    config.memories.enabled = true;
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let loop_context = LoopContext::primary(temp_dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, loop_context);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Runtime tasks are the canonical queue in memories/tasks mode, so completion should be rejected.
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "Should reject completion while runtime tasks remain pending"
    );
    assert!(
        event_loop.has_pending_events(),
        "Rejecting completion should inject task.resume so the loop continues"
    );
}

#[test]
fn test_completion_promise_accepted_even_when_not_last_event() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Completion is now accepted regardless of position in batch (U5).
    // Events after it in the same batch are protected by completion guard.
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    write_event_to_jsonl(&events_path, "task.resume", "Continue");
    let result = event_loop.process_events_from_jsonl().unwrap();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Completion should be accepted even when not the last event"
    );
    // task.resume after LOOP_COMPLETE in same batch should still be published
    // (task.resume is not a business/terminal topic, so completion guard lets it through)
    assert!(
        result.had_events,
        "Non-business events after completion should still be published"
    );
}

#[test]
fn test_builder_cannot_terminate_loop() {
    // Per spec: completion requires an emitted event; output-only tokens are ignored
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    // Builder output containing completion promise - should be IGNORED
    let hat_id = HatId::new("builder");
    let reason = event_loop.process_output(&hat_id, "Done!\nLOOP_COMPLETE", true);

    // Builder cannot terminate, so no termination reason
    assert_eq!(reason, None);

    // Completion event should still terminate
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let completion = event_loop.check_completion_event();
    assert_eq!(completion, Some(TerminationReason::CompletionPromise));
}

#[test]
fn test_build_prompt_uses_ghuntley_style_for_all_hats() {
    // Per Hatless Ralph spec: All hats use build_custom_hat with ghuntley-style prompts
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.start", "build.done", "build.blocked"]
    publishes: ["build.task"]
  builder:
    name: "Builder"
    triggers: ["build.task"]
    publishes: ["build.done", "build.blocked"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test task");

    // Planner hat should get ghuntley-style prompt via build_custom_hat
    let planner_id = HatId::new("planner");
    let planner_prompt = event_loop.build_prompt(&planner_id).unwrap();

    // Verify ghuntley-style structure (numbered phases, guardrails)
    assert!(
        planner_prompt.contains("### 0. ORIENTATION"),
        "Planner should use ghuntley-style orientation phase"
    );
    assert!(
        planner_prompt.contains("### GUARDRAILS"),
        "Planner prompt should have guardrails section"
    );
    assert!(
        planner_prompt.contains("You have fresh context each iteration"),
        "Planner prompt should have RFC2119 identity"
    );

    // Now trigger builder hat by publishing build.task event
    let hat_id = HatId::new("builder");
    event_loop
        .bus
        .publish(Event::new("build.task", "Build something"));

    let builder_prompt = event_loop.build_prompt(&hat_id).unwrap();

    // Verify RFC2119-style structure for builder too
    assert!(
        builder_prompt.contains("### 0. ORIENTATION"),
        "Builder should use RFC2119-style orientation phase"
    );
    assert!(
        builder_prompt.contains("You MUST NOT use more than 1 subagent for build/tests"),
        "Builder prompt should have subagent limit with MUST NOT"
    );
}

#[test]
fn test_build_prompt_uses_custom_hat_for_non_defaults() {
    // Per spec: Custom hats use build_custom_hat with their instructions
    let yaml = r#"
mode: "multi"
hats:
  reviewer:
    name: "Code Reviewer"
    triggers: ["review.request"]
    instructions: "Review code quality."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Publish event to trigger reviewer
    event_loop
        .bus
        .publish(Event::new("review.request", "Review PR #123"));

    let reviewer_id = HatId::new("reviewer");
    let prompt = event_loop.build_prompt(&reviewer_id).unwrap();

    // Should be custom hat prompt (contains custom instructions)
    assert!(
        prompt.contains("Code Reviewer"),
        "Custom hat should use its name"
    );
    assert!(
        prompt.contains("Review code quality"),
        "Custom hat should include its instructions"
    );
    // Should NOT be planner or builder prompt
    assert!(
        !prompt.contains("PLANNER MODE"),
        "Custom hat should not use planner prompt"
    );
    assert!(
        !prompt.contains("BUILDER MODE"),
        "Custom hat should not use builder prompt"
    );
}

#[test]
fn test_exit_codes_per_spec() {
    // Per spec "Loop Termination" section:
    // - 0: Completion promise detected (success)
    // - 1: Consecutive failures or unrecoverable error (failure)
    // - 2: Max iterations, max runtime, or max cost exceeded (limit)
    // - 130: User interrupt (SIGINT = 128 + 2)
    assert_eq!(TerminationReason::CompletionPromise.exit_code(), 0);
    assert_eq!(TerminationReason::ConsecutiveFailures.exit_code(), 1);
    assert_eq!(TerminationReason::LoopThrashing.exit_code(), 1);
    assert_eq!(TerminationReason::Stopped.exit_code(), 1);
    assert_eq!(TerminationReason::MaxIterations.exit_code(), 2);
    assert_eq!(TerminationReason::MaxRuntime.exit_code(), 2);
    assert_eq!(TerminationReason::MaxCost.exit_code(), 2);
    assert_eq!(TerminationReason::Interrupted.exit_code(), 130);
}

/// Helper to write an event to a JSONL file for testing.
fn write_event_to_jsonl(path: &std::path::Path, topic: &str, payload: &str) {
    use std::io::Write;
    let ts = chrono::Utc::now().to_rfc3339();
    let event_json = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": ts
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{}", event_json).unwrap();
}

/// Like [`write_event_to_jsonl`] but includes hat provenance for origin guard compatibility.
fn write_event_with_hat_to_jsonl(path: &std::path::Path, topic: &str, payload: &str, hat: &str) {
    use std::io::Write;
    let ts = chrono::Utc::now().to_rfc3339();
    let event_json = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": ts,
        "hat": hat,
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{}", event_json).unwrap();
}

#[test]
fn test_loop_thrashing_detection() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    // Builder blocks on "Fix bug" three times (should emit build.task.abandoned)
    write_event_to_jsonl(&events_path, "build.blocked", "Fix bug\nCan't compile");
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "build.blocked",
        "Fix bug\nStill can't compile",
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(&events_path, "build.blocked", "Fix bug\nReally stuck");
    let _ = event_loop.process_events_from_jsonl();

    // Task should be abandoned
    assert!(
        event_loop
            .state
            .abandoned_tasks
            .contains(&"Fix bug".to_string()),
        "Task should be abandoned after 3 blocks"
    );
}

#[test]
fn test_thrashing_counter_increments_on_blocked_events() {
    // Events now come from JSONL file via `ralph emit`, not from text output.
    // Per-hat tracking is removed since events don't carry hat context.
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    // Two blocked events should increment counter
    write_event_to_jsonl(&events_path, "build.blocked", "Stuck");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.consecutive_blocked, 1);

    write_event_to_jsonl(&events_path, "build.blocked", "Still stuck");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.consecutive_blocked, 2);
}

#[test]
fn test_thrashing_counter_resets_on_non_blocked_event() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    // Two blocked events
    write_event_to_jsonl(&events_path, "build.blocked", "Stuck");
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(&events_path, "build.blocked", "Still stuck");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.consecutive_blocked, 2);

    // Non-blocked event should reset counter
    write_event_to_jsonl(&events_path, "build.task", "Working now");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.consecutive_blocked, 0);
}

#[test]
fn test_custom_hat_with_instructions_uses_build_custom_hat() {
    // Per spec: Custom hats with instructions should use build_custom_hat() method
    let yaml = r#"
hats:
  reviewer:
    name: "Code Reviewer"
    triggers: ["review.request"]
    instructions: "Review code for quality and security issues."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Trigger the custom hat
    event_loop
        .bus
        .publish(Event::new("review.request", "Review PR #123"));

    let reviewer_id = HatId::new("reviewer");
    let prompt = event_loop.build_prompt(&reviewer_id).unwrap();

    // Should use build_custom_hat() - verify by checking for ghuntley-style structure
    assert!(
        prompt.contains("Code Reviewer"),
        "Should include custom hat name"
    );
    assert!(
        prompt.contains("Review code for quality and security issues"),
        "Should include custom instructions"
    );
    assert!(
        prompt.contains("### 0. ORIENTATION"),
        "Should include ghuntley-style orientation"
    );
    assert!(
        prompt.contains("### 1. EXECUTE"),
        "Should use ghuntley-style execute phase"
    );
    assert!(
        prompt.contains("### GUARDRAILS"),
        "Should include guardrails section"
    );

    // Should include event context
    assert!(
        prompt.contains("Review PR #123"),
        "Should include event context"
    );
}

#[test]
fn test_custom_hat_instructions_included_in_prompt() {
    // Test that custom instructions are properly included in the generated prompt
    let yaml = r#"
hats:
  tester:
    name: "Test Engineer"
    triggers: ["test.request"]
    instructions: |
      Run comprehensive tests including:
      - Unit tests
      - Integration tests
      - Security scans
      Report results with detailed coverage metrics.
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Trigger the custom hat
    event_loop
        .bus
        .publish(Event::new("test.request", "Test the auth module"));

    let tester_id = HatId::new("tester");
    let prompt = event_loop.build_prompt(&tester_id).unwrap();

    // Verify all custom instructions are included
    assert!(prompt.contains("Run comprehensive tests including"));
    assert!(prompt.contains("Unit tests"));
    assert!(prompt.contains("Integration tests"));
    assert!(prompt.contains("Security scans"));
    assert!(prompt.contains("detailed coverage metrics"));

    // Verify event context is included
    assert!(prompt.contains("Test the auth module"));
}

#[test]
fn test_active_hat_with_instructions_and_publishing_guide() {
    // When a hat is triggered by an event, show ACTIVE HAT section with
    // instructions and Event Publishing Guide instead of full topology.
    let yaml = r#"
hats:
  deployer:
    name: "Deployment Manager"
    triggers: ["deploy.request", "deploy.rollback"]
    publishes: ["deploy.done", "deploy.failed"]
    instructions: "Handle deployment operations safely."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Publish an event that triggers the deployer hat
    event_loop
        .bus
        .publish(Event::new("deploy.request", "Deploy to staging"));

    // In multi-hat mode, next_hat always returns "ralph"
    let next_hat = event_loop.next_hat();
    assert_eq!(
        next_hat.unwrap().as_str(),
        "ralph",
        "Multi-hat mode routes to Ralph"
    );

    // Build Ralph's prompt - should include active hat info
    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    // The event topic should be in PENDING EVENTS
    assert!(
        prompt.contains("deploy.request"),
        "Should include the event topic in pending events"
    );

    // Should have ACTIVE HAT section (not HATS topology table)
    assert!(
        prompt.contains("## ACTIVE HAT"),
        "Should have ACTIVE HAT section when hat is triggered"
    );
    assert!(
        !prompt.contains("| Hat | Triggers On | Publishes |"),
        "Should NOT have topology table when hat is active"
    );

    // Should include the hat's instructions
    assert!(
        prompt.contains("Handle deployment operations safely"),
        "Should include active hat's instructions"
    );

    // Should have Event Publishing Guide
    assert!(
        prompt.contains("### Event Publishing Guide"),
        "Should have Event Publishing Guide"
    );
    assert!(
        prompt.contains("`deploy.done`"),
        "Guide should list deploy.done"
    );
    assert!(
        prompt.contains("`deploy.failed`"),
        "Guide should list deploy.failed"
    );
}

#[test]
fn test_default_hat_with_custom_instructions_uses_build_custom_hat() {
    // Test that even default hats (planner/builder) use build_custom_hat when they have custom instructions
    let yaml = r#"
hats:
  planner:
    name: "Custom Planner"
    triggers: ["task.start", "build.done"]
    instructions: "Custom planning instructions with special focus on security."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.initialize("Test task");

    let planner_id = HatId::new("planner");
    let prompt = event_loop.build_prompt(&planner_id).unwrap();

    // Should use build_custom_hat with ghuntley-style structure
    assert!(prompt.contains("Custom Planner"), "Should use custom name");
    assert!(
        prompt.contains("Custom planning instructions with special focus on security"),
        "Should include custom instructions"
    );
    assert!(
        prompt.contains("### 1. EXECUTE"),
        "Should use ghuntley-style execute phase"
    );
    assert!(
        prompt.contains("### GUARDRAILS"),
        "Should include guardrails section"
    );
}

#[test]
fn test_custom_hat_without_instructions_gets_default_behavior() {
    // Test that custom hats without instructions still work with build_custom_hat
    let yaml = r#"
hats:
  monitor:
    name: "System Monitor"
    triggers: ["monitor.request"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("monitor.request", "Check system health"));

    let monitor_id = HatId::new("monitor");
    let prompt = event_loop.build_prompt(&monitor_id).unwrap();

    // Should still use build_custom_hat with ghuntley-style structure
    assert!(
        prompt.contains("System Monitor"),
        "Should include custom hat name"
    );
    assert!(
        prompt.contains("Follow the incoming event instructions"),
        "Should have default instructions when none provided"
    );
    assert!(
        prompt.contains("### 0. ORIENTATION"),
        "Should include ghuntley-style orientation"
    );
    assert!(
        prompt.contains("### GUARDRAILS"),
        "Should include guardrails section"
    );
    assert!(
        prompt.contains("Check system health"),
        "Should include event context"
    );
}

#[test]
fn test_task_cancellation_with_tilde_marker() {
    // Test that tasks marked with [~] are recognized as cancelled
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test task");

    let ralph_id = HatId::new("ralph");

    // Simulate Ralph output with cancelled task
    let output = r"
## Tasks
- [x] Task 1 completed
- [~] Task 2 cancelled (too complex for current scope)
- [ ] Task 3 pending
";

    // Process output - should not terminate since there are still pending tasks
    let reason = event_loop.process_output(&ralph_id, output, true);
    assert_eq!(reason, None, "Should not terminate with pending tasks");
}

#[test]
fn test_partial_completion_with_cancelled_tasks() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    // Create scratchpad with completed and cancelled tasks (use absolute path, no set_current_dir)
    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    let scratchpad_content = r"## Tasks
- [x] Core feature implemented
- [x] Tests added
- [~] Documentation update (cancelled: out of scope)
- [~] Performance optimization (cancelled: not needed)
";
    fs::write(&scratchpad_path, scratchpad_content).unwrap();

    // Test that cancelled tasks don't block completion when all other tasks are done
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    publishes: ["build.done"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test task");

    // Simulate completion with some cancelled tasks - should complete immediately
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Should complete immediately with partial completion (cancelled tasks ok)"
    );
}

#[test]
fn test_planner_auto_cancellation_after_three_blocks() {
    // Test that task is abandoned after 3 build.blocked events for same task
    // Events now come from JSONL via `ralph emit`.
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test task");

    // First blocked event for "Task X" - should not abandon
    write_event_to_jsonl(&events_path, "build.blocked", "Task X\nmissing dependency");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.task_block_counts.get("Task X"), Some(&1));

    // Second blocked event for "Task X" - should not abandon
    write_event_to_jsonl(
        &events_path,
        "build.blocked",
        "Task X\ndependency issue persists",
    );
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.task_block_counts.get("Task X"), Some(&2));

    // Third blocked event for "Task X" - should emit build.task.abandoned
    write_event_to_jsonl(
        &events_path,
        "build.blocked",
        "Task X\nsame dependency issue",
    );
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.task_block_counts.get("Task X"), Some(&3));
    assert!(
        event_loop
            .state
            .abandoned_tasks
            .contains(&"Task X".to_string()),
        "Task X should be abandoned"
    );
}

#[test]
fn test_default_publishes_injects_when_no_events() {
    use std::collections::HashMap;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    let mut hats = HashMap::new();
    hats.insert(
        "test-hat".to_string(),
        crate::config::HatConfig {
            name: "test-hat".to_string(),
            description: Some("Test hat for default publishes".to_string()),
            triggers: vec!["task.start".to_string()],
            publishes: vec!["task.done".to_string()],
            instructions: "Test hat".to_string(),
            extra_instructions: vec![],
            backend_args: None,
            backend: None,
            default_publishes: Some("task.done".to_string()),
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
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    let hat_id = HatId::new("test-hat");

    // Agent wrote no events — process_events_from_jsonl would return had_events: false
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(!result.had_events, "No events should be found");

    // check_default_publishes should inject the default
    event_loop.check_default_publishes(&hat_id);

    assert!(
        event_loop.has_pending_events(),
        "Default event should be injected"
    );

    // The default_publishes topic should be recorded in seen_topics
    assert!(
        event_loop.state.seen_topics.contains("task.done"),
        "default_publishes should record topic in seen_topics for chain validation"
    );
}

#[test]
fn test_default_publishes_not_injected_when_events_written() {
    use std::collections::HashMap;
    use std::io::Write;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    let mut hats = HashMap::new();
    hats.insert(
        "test-hat".to_string(),
        crate::config::HatConfig {
            name: "test-hat".to_string(),
            description: Some("Test hat for default publishes".to_string()),
            triggers: vec!["task.start".to_string()],
            publishes: vec!["task.done".to_string()],
            instructions: "Test hat".to_string(),
            extra_instructions: vec![],
            backend_args: None,
            backend: None,
            default_publishes: Some("task.done".to_string()),
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
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    let _hat_id = HatId::new("test-hat");

    // Agent writes an event to the JSONL file
    let mut file = std::fs::File::create(&events_path).unwrap();
    writeln!(
        file,
        r#"{{"topic":"task.done","ts":"2024-01-01T00:00:00Z"}}"#
    )
    .unwrap();
    file.flush().unwrap();

    // process_events_from_jsonl reads them — caller should NOT call check_default_publishes
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(result.had_events, "Events should be found from JSONL");

    // Verify: even if someone mistakenly calls check_default_publishes, the
    // call site guards with `if !agent_wrote_events`, so defaults won't fire.
    // But we assert the guard condition here:
    assert!(
        result.had_events,
        "Caller should skip check_default_publishes when agent wrote events"
    );
}

#[test]
fn test_has_pending_plan_events_in_jsonl_peeks_without_consuming() {
    use std::io::Write;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut event_loop = EventLoop::new(RalphConfig::default());
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let mut file = std::fs::File::create(&events_path).unwrap();
    writeln!(
        file,
        r#"{{"topic":"plan.created","payload":"ready","ts":"2024-01-01T00:00:00Z"}}"#
    )
    .unwrap();
    file.flush().unwrap();

    assert!(
        event_loop
            .has_pending_plan_events_in_jsonl()
            .expect("peek should succeed"),
        "peek should report unread plan.* topics"
    );

    let processed = event_loop.process_events_from_jsonl().unwrap();
    assert!(processed.had_events);
    assert!(
        processed.had_plan_events,
        "processed metadata should preserve semantic plan.* detection"
    );
    assert!(
        processed.human_interact_context.is_none(),
        "plan-only batches should not synthesize human.interact metadata"
    );

    assert!(
        !event_loop
            .has_pending_plan_events_in_jsonl()
            .expect("peek after consume should succeed"),
        "peek should return false after unread events are consumed"
    );
}

#[test]
fn test_pending_human_interact_context_in_jsonl_peeks_without_consuming() {
    use std::io::Write;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut event_loop = EventLoop::new(RalphConfig::default());
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let mut file = std::fs::File::create(&events_path).unwrap();
    writeln!(
        file,
        r#"{{"topic":"human.interact","payload":"Need approval?","ts":"2024-01-01T00:00:00Z"}}"#
    )
    .unwrap();
    file.flush().unwrap();

    let pending_context = event_loop
        .pending_human_interact_context_in_jsonl()
        .expect("peek should succeed")
        .expect("peek should include pending human.interact context");
    assert_eq!(
        pending_context["question"],
        serde_json::json!("Need approval?")
    );
    assert!(
        pending_context.get("outcome").is_none(),
        "pre-dispatch context should not include outcome metadata"
    );

    let processed = event_loop.process_events_from_jsonl().unwrap();
    assert!(processed.had_events);
    let processed_context = processed
        .human_interact_context
        .expect("processed metadata should include human.interact context");
    assert_eq!(
        processed_context["question"],
        serde_json::json!("Need approval?")
    );
    assert_eq!(
        processed_context["outcome"],
        serde_json::json!("no_robot_service")
    );

    assert!(
        event_loop
            .pending_human_interact_context_in_jsonl()
            .expect("peek after consume should succeed")
            .is_none(),
        "peek should return no pending human.interact events after consume"
    );
}

#[test]
fn test_process_events_from_jsonl_reports_when_plan_topics_absent() {
    use std::io::Write;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut event_loop = EventLoop::new(RalphConfig::default());
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let mut file = std::fs::File::create(&events_path).unwrap();
    writeln!(
        file,
        r#"{{"topic":"task.start","payload":"start","ts":"2024-01-01T00:00:00Z"}}"#
    )
    .unwrap();
    file.flush().unwrap();

    let processed = event_loop.process_events_from_jsonl().unwrap();
    assert!(processed.had_events);
    assert!(
        !processed.had_plan_events,
        "semantic plan.* flag should remain false when no plan topics were published"
    );
    assert!(
        processed.human_interact_context.is_none(),
        "non-human batches should not expose human.interact metadata"
    );
}

/// Regression: when agent writes a non-orphan event (one whose topic IS a trigger for
/// a hat), the caller must NOT inject default_publishes. This test replicates the exact
/// caller logic from loop_runner.rs to detect the mismatch between has_orphans and had_events.
///
/// Before the fix, `process_events_from_jsonl` returned a single bool = has_orphans.
/// For non-orphan events (e.g. task.start which triggers hat-a), has_orphans was false,
/// causing the caller to think "no events were written" and inject default_publishes.
#[test]
fn test_default_publishes_skipped_when_non_orphan_event_written() {
    use std::collections::HashMap;
    use std::io::Write;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    let mut hats = HashMap::new();
    // hat-a triggers on task.start → task.start is NOT an orphan
    hats.insert(
        "hat-a".to_string(),
        crate::config::HatConfig {
            name: "hat-a".to_string(),
            description: Some("Hat triggered by task.start".to_string()),
            triggers: vec!["task.start".to_string()],
            publishes: vec!["task.done".to_string()],
            instructions: "Do the task".to_string(),
            extra_instructions: vec![],
            backend_args: None,
            backend: None,
            default_publishes: Some("task.done".to_string()),
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
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    let hat_id = HatId::new("hat-a");

    // Consume the initial event from initialize so pending state starts clean
    let _ = event_loop.build_prompt(&hat_id);

    // Agent writes a non-orphan event (task.start → triggers hat-a)
    let mut file = std::fs::File::create(&events_path).unwrap();
    writeln!(
        file,
        r#"{{"topic":"task.start","ts":"2024-01-01T00:00:00Z","payload":"starting work"}}"#
    )
    .unwrap();
    file.flush().unwrap();

    // Process events — this is what the event loop calls
    let result = event_loop.process_events_from_jsonl().unwrap();

    // The caller in loop_runner.rs uses `had_events` to decide whether to inject defaults:
    //   let agent_wrote_events = result.had_events;
    //   if !agent_wrote_events { check_default_publishes(...) }
    //
    // Before the fix, the return was a single bool (= has_orphans). For a non-orphan
    // event like task.start, has_orphans=false, so the caller would see
    // agent_wrote_events=false and incorrectly inject default_publishes.
    assert!(
        result.had_events,
        "had_events must be true when agent wrote events (even non-orphan ones)"
    );
    // Also verify has_orphans is false — this was the old return value that got conflated
    assert!(
        !result.has_orphans,
        "has_orphans should be false for non-orphan events"
    );
}

#[test]
fn test_default_publishes_not_injected_when_not_configured() {
    use std::collections::HashMap;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    let mut hats = HashMap::new();
    hats.insert(
        "test-hat".to_string(),
        crate::config::HatConfig {
            name: "test-hat".to_string(),
            description: Some("Test hat for default publishes".to_string()),
            triggers: vec!["task.start".to_string()],
            publishes: vec!["task.done".to_string()],
            instructions: "Test hat".to_string(),
            extra_instructions: vec![],
            backend_args: None,
            backend: None,
            default_publishes: None, // No default configured
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
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    let hat_id = HatId::new("test-hat");

    // Consume the initial event from initialize
    let _ = event_loop.build_prompt(&hat_id);

    // Agent wrote no events
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(!result.had_events);

    // check_default_publishes should NOT inject since not configured
    event_loop.check_default_publishes(&hat_id);

    assert!(
        !event_loop.has_pending_events(),
        "No default should be injected"
    );
}

#[test]
fn test_get_hat_backend_with_named_backend() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    backend: "claude"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    let hat_id = HatId::new("builder");
    let backend = event_loop.get_hat_backend(&hat_id);

    assert!(backend.is_some());
    match backend.unwrap() {
        HatBackend::Named(name) => assert_eq!(name, "claude"),
        _ => panic!("Expected Named backend"),
    }
}

#[test]
fn test_get_hat_backend_with_kiro_agent() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    backend:
      type: "kiro"
      agent: "my-agent"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    let hat_id = HatId::new("builder");
    let backend = event_loop.get_hat_backend(&hat_id);

    assert!(backend.is_some());
    match backend.unwrap() {
        HatBackend::KiroAgent { agent, .. } => assert_eq!(agent, "my-agent"),
        _ => panic!("Expected KiroAgent backend"),
    }
}

#[test]
fn test_get_hat_backend_inherits_global() {
    let yaml = r#"
cli:
  backend: "gemini"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    let hat_id = HatId::new("builder");
    let backend = event_loop.get_hat_backend(&hat_id);

    // Hat has no backend configured, should return None (inherit global)
    assert!(backend.is_none());
}

#[test]
fn test_hatless_mode_registers_ralph_catch_all() {
    // When no hats are configured, "ralph" should be registered as catch-all
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    // Registry should contain the builtin "ralph" hat (from_runtime_config)
    assert!(
        !event_loop.registry().is_empty(),
        "Registry should contain builtin ralph"
    );
    assert!(
        event_loop.registry().get(&HatId::new("ralph")).is_some(),
        "Registry should have ralph registered"
    );

    // No user-defined hats (EventLoop::default creates RalphConfig with empty hats)
    assert_eq!(
        event_loop.config().hats.len(),
        0,
        "No custom hats configured"
    );

    // When we initialize, task.start should route to "ralph"
    event_loop.initialize("Test prompt");

    // "ralph" should have pending events
    let next_hat = event_loop.next_hat();
    assert!(next_hat.is_some(), "Should have pending events for ralph");
    assert_eq!(next_hat.unwrap().as_str(), "ralph");
}

#[test]
fn test_hatless_mode_builds_ralph_prompt() {
    // In hatless mode, build_prompt for "ralph" should return HatlessRalph prompt
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let ralph_id = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph_id);

    assert!(prompt.is_some(), "Should build prompt for ralph");
    let prompt = prompt.unwrap();

    // Should contain RFC2119-style Ralph identity (uses "You are Ralph")
    assert!(
        prompt.contains("You are Ralph"),
        "Should identify as Ralph with RFC2119 style"
    );
    assert!(
        prompt.contains("## WORKFLOW"),
        "Should have workflow section"
    );
    assert!(
        prompt.contains("## EVENT WRITING"),
        "Should have event writing section"
    );
    assert!(
        prompt.contains("LOOP_COMPLETE"),
        "Should reference completion event"
    );
}

// === "Always Hatless Iteration" Architecture Tests ===
// These tests verify the core invariants of the Hatless Ralph architecture:
// - Ralph is always the sole executor when custom hats are defined
// - Custom hats define topology (pub/sub contracts) for coordination context
// - Ralph's prompt includes the ## HATS section documenting the topology

#[test]
fn test_always_hatless_ralph_executes_all_iterations() {
    // Per acceptance criteria #1: Ralph executes all iterations with custom hats
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.start", "build.done"]
    publishes: ["build.task"]
  builder:
    name: "Builder"
    triggers: ["build.task"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Simulate the workflow: task.start → planner (conceptually)
    event_loop.initialize("Implement feature X");
    assert_eq!(event_loop.next_hat().unwrap().as_str(), "ralph");

    // Simulate build.task → builder (conceptually)
    event_loop.build_prompt(&HatId::new("ralph")); // Consume task.start
    event_loop
        .bus
        .publish(Event::new("build.task", "Build feature X"));
    assert_eq!(
        event_loop.next_hat().unwrap().as_str(),
        "ralph",
        "build.task should route to Ralph"
    );

    // Simulate build.done → planner (conceptually)
    event_loop.build_prompt(&HatId::new("ralph")); // Consume build.task
    event_loop
        .bus
        .publish(Event::new("build.done", "Feature X complete"));
    assert_eq!(
        event_loop.next_hat().unwrap().as_str(),
        "ralph",
        "build.done should route to Ralph"
    );
}

#[test]
fn test_wave_results_activate_synthesizer() {
    // Simulates the wave review scenario:
    // 1. Coordinator dispatches review.perspective (wave events)
    // 2. After wave, review.done events are published to bus
    // 3. On next iteration, synthesizer should be the active hat
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["review.start"]
    publishes: ["review.perspective"]
    instructions: "Dispatch reviewers as a wave."
  reviewer:
    name: "Reviewer"
    triggers: ["review.perspective"]
    publishes: ["review.done"]
    concurrency: 3
    instructions: "Review code from your specialty."
  synthesizer:
    name: "Synthesizer"
    triggers: ["review.done"]
    publishes: ["review.complete"]
    instructions: "SYNTHESIZER MODE - Aggregate all review.done findings into a report."
    aggregate:
      mode: wait_for_all
      timeout: 300
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Step 1: Initialize with review.start — coordinator activates
    event_loop.initialize("Review the code");
    assert_eq!(event_loop.next_hat().unwrap().as_str(), "ralph");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        prompt.contains("Coordinator"),
        "Should activate coordinator for review.start"
    );

    // Step 2: Simulate wave results — publish review.done events directly to bus
    // (this is what loop_runner does after merge_wave_results_to_events_file + re-read)
    event_loop
        .bus
        .publish(Event::new("review.done", "Rust review findings"));
    event_loop
        .bus
        .publish(Event::new("review.done", "Frontend review findings"));
    event_loop
        .bus
        .publish(Event::new("review.done", "Docs review findings"));

    // Step 3: next_hat should find pending events and return ralph
    assert!(
        event_loop.next_hat().is_some(),
        "Should have pending events for next hat"
    );

    // Step 4: build_prompt should activate synthesizer (not coordinator)
    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        prompt.contains("SYNTHESIZER MODE"),
        "Should activate synthesizer for review.done events"
    );
    assert!(
        !prompt.contains("Dispatch reviewers"),
        "Should NOT have coordinator instructions"
    );
    assert!(
        prompt.contains("review.done"),
        "Should contain review.done events in context"
    );
}

#[test]
fn test_always_hatless_solo_mode_unchanged() {
    // Per acceptance criteria #3: Solo mode (no hats) operates as before
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    // Registry should contain builtin ralph, but no custom hats
    assert!(
        !event_loop.registry().is_empty(),
        "Registry should contain builtin ralph"
    );
    assert!(
        event_loop.registry().get(&HatId::new("ralph")).is_some(),
        "Registry should have ralph registered"
    );
    assert!(
        event_loop.config().hats.is_empty(),
        "No custom hats in config"
    );

    event_loop.initialize("Do something");
    assert_eq!(event_loop.next_hat().unwrap().as_str(), "ralph");

    // Solo mode prompt should NOT have ## HATS section
    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        !prompt.contains("## HATS"),
        "Solo mode should not have HATS section"
    );
}

#[test]
fn test_active_hat_gets_publishing_guide_not_topology() {
    // When a hat is triggered, show its instructions + Event Publishing Guide
    // Skip the topology table/Mermaid to reduce token usage
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.start", "build.done", "build.blocked"]
    publishes: ["build.task"]
  builder:
    name: "Builder"
    description: "Builds code"
    triggers: ["build.task"]
    publishes: ["build.done", "build.blocked"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test"); // Publishes task.start which triggers Planner

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    // Planner is active (triggered by task.start), so we get ACTIVE HAT section
    assert!(
        prompt.contains("## ACTIVE HAT"),
        "Should have ACTIVE HAT section when hat is triggered"
    );

    // Should NOT have topology table when a hat is active
    assert!(
        !prompt.contains("| Hat | Triggers On | Publishes |"),
        "Should NOT have topology table when hat is active"
    );
    assert!(
        !prompt.contains("```mermaid"),
        "Should NOT have Mermaid diagram when hat is active"
    );

    // Should have Event Publishing Guide showing who receives build.task
    assert!(
        prompt.contains("### Event Publishing Guide"),
        "Should have Event Publishing Guide for active hat"
    );
    assert!(
        prompt.contains("`build.task` → Received by: Builder"),
        "Should show Builder receives build.task"
    );
}

#[test]
fn test_always_hatless_no_backend_delegation() {
    // Per acceptance criteria #5: Custom hat backends are NOT used
    // This is architectural - the EventLoop.next_hat() always returns "ralph"
    // so per-hat backends (if configured) are never invoked
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    backend: "gemini"  # This backend should NEVER be used
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.bus.publish(Event::new("build.task", "Test"));

    // Despite builder having a specific backend, Ralph handles the iteration
    let next = event_loop.next_hat();
    assert_eq!(
        next.unwrap().as_str(),
        "ralph",
        "Ralph handles all iterations"
    );

    // The backend delegation would happen in main.rs, but since we always
    // return "ralph" from next_hat(), the gemini backend is never selected
}

#[test]
fn test_always_hatless_collects_all_pending_events() {
    // Verify Ralph's prompt includes downstream events from all hats when in multi-hat mode.
    // Kickoff events like task.start should drop out once a more specific downstream event exists.
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.start"]
  builder:
    name: "Builder"
    triggers: ["build.task"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Publish events that would go to different hats
    event_loop
        .bus
        .publish(Event::new("task.start", "Start task"));
    event_loop
        .bus
        .publish(Event::new("build.task", "Build something"));

    // Ralph should collect ALL pending events
    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    // The downstream event should be in Ralph's context, and the kickoff event
    // should not dominate once downstream work is pending.
    assert!(
        !prompt.contains("task.start"),
        "task.start should be filtered once a downstream event is pending"
    );
    assert!(
        prompt.contains("build.task"),
        "Should include build.task event"
    );
}

// === Phase 2: Active Hat Detection Tests ===

#[test]
fn test_determine_active_hats() {
    // Create EventLoop with 3 hats (security_reviewer, architecture_reviewer, correctness_reviewer)
    let yaml = r#"
hats:
  security_reviewer:
    name: "Security Reviewer"
    triggers: ["review.security"]
  architecture_reviewer:
    name: "Architecture Reviewer"
    triggers: ["review.architecture"]
  correctness_reviewer:
    name: "Correctness Reviewer"
    triggers: ["review.correctness"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    // Create events: [Event("review.security", "..."), Event("review.architecture", "...")]
    let events = vec![
        Event::new("review.security", "Check for vulnerabilities"),
        Event::new("review.architecture", "Review design patterns"),
    ];

    // Call determine_active_hats(&events)
    let active_hats = event_loop.determine_active_hats(&events);

    // Assert: Returns Vec with exactly security_reviewer and architecture_reviewer Hats
    assert_eq!(active_hats.len(), 2, "Should return exactly 2 active hats");

    let hat_ids: Vec<&str> = active_hats.iter().map(|h| h.id.as_str()).collect();
    assert!(
        hat_ids.contains(&"security_reviewer"),
        "Should include security_reviewer"
    );
    assert!(
        hat_ids.contains(&"architecture_reviewer"),
        "Should include architecture_reviewer"
    );
    assert!(
        !hat_ids.contains(&"correctness_reviewer"),
        "Should NOT include correctness_reviewer"
    );
}

#[test]
fn test_get_active_hat_id_with_pending_event() {
    // Create EventLoop with security_reviewer hat
    let yaml = r#"
hats:
  security_reviewer:
    name: "Security Reviewer"
    triggers: ["review.security"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Publish Event("review.security", "...")
    event_loop
        .bus
        .publish(Event::new("review.security", "Check authentication"));

    // Call get_active_hat_id()
    let active_hat_id = event_loop.get_active_hat_id();

    // Assert: Returns HatId("security_reviewer"), NOT "ralph"
    assert_eq!(
        active_hat_id.as_str(),
        "security_reviewer",
        "Should return security_reviewer, not ralph"
    );
}

#[test]
fn test_get_active_hat_id_no_pending_returns_ralph() {
    // Create EventLoop with hats but NO pending events
    let yaml = r#"
hats:
  security_reviewer:
    name: "Security Reviewer"
    triggers: ["review.security"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    // Call get_active_hat_id() - no pending events
    let active_hat_id = event_loop.get_active_hat_id();

    // Assert: Returns HatId("ralph")
    assert_eq!(
        active_hat_id.as_str(),
        "ralph",
        "Should return ralph when no pending events"
    );
}

#[test]
fn test_get_active_hat_id_deterministic_with_multiple_pending() {
    // Two hats with pending events → get_active_hat_id returns alphabetically first matching hat
    let yaml = r#"
hats:
  zebra_hat:
    name: "Zebra"
    triggers: ["work.*"]
  alpha_hat:
    name: "Alpha"
    triggers: ["work.*"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Publish event that both hats subscribe to
    event_loop
        .bus
        .publish(Event::new("work.start", "Begin work"));

    // Should deterministically return "alpha_hat" (alphabetically first)
    let active = event_loop.get_active_hat_id();
    assert_eq!(
        active.as_str(),
        "alpha_hat",
        "get_active_hat_id should return alphabetically first matching hat"
    );

    // Run multiple times to confirm determinism
    for _ in 0..100 {
        let active = event_loop.get_active_hat_id();
        assert_eq!(active.as_str(), "alpha_hat");
    }
}

#[test]
fn test_get_active_hat_id_matches_prompt_active_hat_selection() {
    let yaml = r#"
hats:
  investigator:
    name: "Investigator"
    triggers: ["debug.start", "hypothesis.confirmed"]
  tester:
    name: "Tester"
    triggers: ["hypothesis.test"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("debug.start", "Investigate a bug"));
    event_loop
        .bus
        .publish(Event::new("hypothesis.test", "Test the hypothesis"));

    let preview_active_hat = event_loop.get_active_hat_id();

    event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt should build");

    let built_active_hat = event_loop
        .state
        .last_active_hat_ids
        .first()
        .expect("build_prompt should set active hats")
        .clone();

    assert_eq!(
        preview_active_hat.as_str(),
        "tester",
        "downstream hypothesis.test should outrank kickoff debug.start in preview selection"
    );
    assert_eq!(
        built_active_hat.as_str(),
        "tester",
        "build_prompt should select tester when debug.start and hypothesis.test are both pending"
    );
    assert_eq!(
        preview_active_hat, built_active_hat,
        "display hat preview should match prompt-selected active hat"
    );
}

#[test]
fn test_get_active_hat_id_prefers_semantic_event_over_targeted_task_resume() {
    let yaml = r#"
hats:
  investigator:
    name: "Investigator"
    triggers: ["task.resume", "debug.start", "hypothesis.confirmed"]
  tester:
    name: "Tester"
    triggers: ["hypothesis.test"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("task.resume", "Recovery").with_target("investigator"));
    event_loop
        .bus
        .publish(Event::new("hypothesis.test", "Test the hypothesis"));

    let preview_active_hat = event_loop.get_active_hat_id();
    assert_eq!(
        preview_active_hat.as_str(),
        "tester",
        "semantic downstream work should outrank fallback task.resume for display selection"
    );

    event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt should build");

    let built_active_hat = event_loop
        .state
        .last_active_hat_ids
        .first()
        .expect("build_prompt should set active hats")
        .clone();
    assert_eq!(
        built_active_hat.as_str(),
        "tester",
        "prompt-selected active hat should ignore fallback task.resume when real work is pending"
    );
}

#[test]
fn test_get_active_hat_id_honors_direct_target_before_topic_lookup() {
    let yaml = r#"
hats:
  alpha_hat:
    name: "Alpha"
    triggers: ["task.resume"]
  zebra_hat:
    name: "Zebra"
    triggers: ["task.resume"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("task.resume", "Recovery").with_target("zebra_hat"));

    let active_hat_id = event_loop.get_active_hat_id();
    assert_eq!(
        active_hat_id.as_str(),
        "zebra_hat",
        "direct event targets should override generic topic subscriber ordering"
    );
}

#[test]
fn test_determine_active_hat_ids_excludes_entrypoint_hats_when_progressed_events_exist() {
    // When both a stale entrypoint event (review.start) and a progressed event
    // (review.done) are pending, only the downstream hat should be activated —
    // not the entrypoint hat. This prevents the coordinator from being
    // re-included alongside the synthesizer after wave workers complete.
    let yaml = r#"
event_loop:
  starting_event: "review.start"
  completion_promise: "review.complete"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["review.start"]
    publishes: ["review.perspective"]
  synthesizer:
    name: "Synthesizer"
    triggers: ["review.done"]
    publishes: ["review.complete"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Review the auth module");

    // Simulate state after wave workers complete: stale review.start +
    // new review.done events are both pending.
    let events = vec![
        Event::new("review.start", "Review the auth module"),
        Event::new("review.done", "## Rust Review\n..."),
        Event::new("review.done", "## Frontend Review\n..."),
    ];

    let active = event_loop.determine_active_hat_ids(&events);
    assert_eq!(
        active.len(),
        1,
        "Only the synthesizer should be active, not coordinator + synthesizer"
    );
    assert_eq!(
        active[0].as_str(),
        "synthesizer",
        "The synthesizer (triggered by review.done) should be selected over the coordinator (triggered by stale review.start)"
    );
}

#[test]
fn test_determine_active_hat_ids_falls_back_to_entrypoint_when_no_progressed_events() {
    // When only entrypoint events are pending, the entrypoint hat should be activated.
    let yaml = r#"
event_loop:
  starting_event: "review.start"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["review.start"]
    publishes: ["review.perspective"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Review the auth module");

    let events = vec![Event::new("review.start", "Review the auth module")];

    let active = event_loop.determine_active_hat_ids(&events);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].as_str(), "coordinator");
}

#[test]
fn test_check_for_user_prompt_detects_user_prompt_event() {
    // Create EventLoop
    let config: RalphConfig = serde_yaml::from_str("hats: {}").unwrap();
    let event_loop = EventLoop::new(config);

    // Create events with a user.prompt event
    // The id is embedded in the XML payload
    let events = vec![
        Event::new("build.task", "Some task"),
        Event::new(
            "user.prompt",
            r#"<event topic="user.prompt" id="q1">What is the feature name?</event>"#,
        ),
        Event::new("other.event", "Other"),
    ];

    // Check for user prompt
    let user_prompt = event_loop.check_for_user_prompt(&events);

    assert!(user_prompt.is_some(), "Should detect user.prompt event");
    assert_eq!(user_prompt.unwrap().id, "q1");
}

#[test]
fn test_check_for_user_prompt_returns_none_when_no_user_prompt() {
    // Create EventLoop
    let config: RalphConfig = serde_yaml::from_str("hats: {}").unwrap();
    let event_loop = EventLoop::new(config);

    // Create events WITHOUT a user.prompt event
    let events = vec![
        Event::new("build.task", "Some task"),
        Event::new("build.done", "Task completed"),
    ];

    // Check for user prompt
    let user_prompt = event_loop.check_for_user_prompt(&events);

    assert!(
        user_prompt.is_none(),
        "Should not detect user.prompt when not present"
    );
}

#[test]
fn test_extract_prompt_id_from_xml_format() {
    // Create EventLoop
    let config: RalphConfig = serde_yaml::from_str("hats: {}").unwrap();
    let event_loop = EventLoop::new(config);

    // Create event with XML attribute format
    let event = Event::new(
        "user.prompt",
        r#"<event topic="user.prompt" id="q42">What's the deadline?</event>"#,
    );
    let events = vec![event];

    let user_prompt = event_loop.check_for_user_prompt(&events).unwrap();
    assert_eq!(user_prompt.id, "q42");
}

// Note: Orphan event detection is now handled in loop_runner.rs::log_events_from_output()
// which logs to events.jsonl. The `event.orphaned` system event appears in the events file
// when an event has no subscriber hat, making it visible via `ralph events`.

// === Objective Persistence Tests ===

#[test]
fn test_initialize_stores_objective_in_ralph() {
    // initialize() should store the prompt as the objective in HatlessRalph
    // so that subsequent iterations always see it, even after bus.take_pending() consumes the start event.
    let yaml = r#"
hats:
  test_writer:
    name: "Test Writer"
    triggers: ["tdd.start"]
    publishes: ["test.written"]
    instructions: "Write failing tests."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.initialize("Implement a binary search tree with insert and search");

    // Consume the start event (simulates iteration 1 completing)
    let ralph_id = HatId::new("ralph");
    let prompt1 = event_loop.build_prompt(&ralph_id).unwrap();
    assert!(
        prompt1.contains("## OBJECTIVE"),
        "Iteration 1 should have OBJECTIVE section"
    );
    assert!(
        prompt1.contains("Implement a binary search tree"),
        "Iteration 1 should show the objective"
    );

    // Simulate iteration 2: hat publishes an event, start event is gone
    event_loop
        .bus
        .publish(Event::new("test.written", "tests/bst_test.rs"));

    let prompt2 = event_loop.build_prompt(&ralph_id).unwrap();

    // Objective should STILL be present even though task.start was consumed
    assert!(
        prompt2.contains("## OBJECTIVE"),
        "Iteration 2+ should still have OBJECTIVE section"
    );
    assert!(
        prompt2.contains("Implement a binary search tree"),
        "Objective should persist across iterations"
    );
}

#[test]
fn test_done_section_suppressed_for_active_hat_via_event_loop() {
    // When a hat is active (triggered by an event), the DONE section should NOT appear.
    // This prevents intermediate hats from seeing completion instructions.
    let yaml = r#"
hats:
  implementer:
    name: "Implementer"
    triggers: ["test.written"]
    publishes: ["test.passing"]
    instructions: "Make the failing test pass."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Build a calculator");

    // Consume the start event
    let ralph_id = HatId::new("ralph");
    let _ = event_loop.build_prompt(&ralph_id);

    // Simulate implementer being triggered
    event_loop
        .bus
        .publish(Event::new("test.written", "tests/calc_test.rs"));

    let prompt = event_loop.build_prompt(&ralph_id).unwrap();

    // Implementer hat is active — DONE section should be suppressed
    assert!(
        !prompt.contains("## DONE"),
        "DONE section should be suppressed when a hat is active"
    );
    assert!(
        !prompt.contains("You MUST emit a completion event"),
        "Completion instruction should not appear for active hat"
    );

    // But the objective should still be visible
    assert!(
        prompt.contains("## OBJECTIVE"),
        "OBJECTIVE should still be visible to active hat"
    );
    assert!(
        prompt.contains("Build a calculator"),
        "Objective content should be visible"
    );
}

// === Mutant-killing tests ===

#[test]
fn test_consecutive_failures_increments_on_failed_output() {
    // Kills: line 928 `+= 1` → `-=` / `*=`
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let ralph = HatId::new("ralph");

    event_loop.process_output(&ralph, "output", false);
    assert_eq!(event_loop.state.consecutive_failures, 1);

    event_loop.process_output(&ralph, "output", false);
    assert_eq!(event_loop.state.consecutive_failures, 2);
}

#[test]
fn test_consecutive_failures_resets_on_success() {
    // Kills: line 926 reset branch
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let ralph = HatId::new("ralph");

    event_loop.process_output(&ralph, "output", false);
    assert_eq!(event_loop.state.consecutive_failures, 1);

    event_loop.process_output(&ralph, "output", true);
    assert_eq!(event_loop.state.consecutive_failures, 0);
}

#[test]
fn test_cost_based_termination() {
    // Kills: line 383 `>=` → `<`, lines 987 `add_cost` noop / `-=` / `*=`
    let yaml = r"
event_loop:
  max_cost_usd: 10.0
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.add_cost(9.99);
    assert_eq!(
        event_loop.check_termination(),
        None,
        "Should NOT terminate below max cost"
    );

    event_loop.add_cost(0.01);
    assert_eq!(
        event_loop.check_termination(),
        Some(TerminationReason::MaxCost),
        "Should terminate at exactly max cost"
    );
}

#[test]
fn test_malformed_events_increment_counter() {
    // Kills: line 1063 `+= 1` → `-=` / `*=`
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    // Write invalid JSONL
    std::fs::write(&events_path, "not valid json\n").unwrap();
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop.state.consecutive_malformed_events, 1,
        "First malformed line should set counter to 1"
    );

    // Write another invalid line (append)
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&events_path)
        .unwrap();
    writeln!(file, "also not json").unwrap();
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop.state.consecutive_malformed_events, 2,
        "Second malformed line should set counter to 2"
    );
}

#[test]
fn test_malformed_counter_resets_on_valid_event() {
    // Kills: line 1072 `!` deletion
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.initialize("Test");

    // Write invalid JSONL
    std::fs::write(&events_path, "not valid json\n").unwrap();
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(event_loop.state.consecutive_malformed_events, 1);

    // Write a valid event
    write_event_to_jsonl(&events_path, "build.done", "success");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop.state.consecutive_malformed_events, 0,
        "Counter should reset when valid events are parsed"
    );
}

#[test]
fn test_validation_failure_termination_at_threshold() {
    // Kills: line 1165 `>=` → `<` and `&&` → `||`
    // (Note: line 1165 refers to validation threshold at line 398)
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    event_loop.state.consecutive_malformed_events = 2;
    assert_eq!(
        event_loop.check_termination(),
        None,
        "Should NOT terminate at 2 malformed events (threshold is 3)"
    );

    event_loop.state.consecutive_malformed_events = 3;
    assert_eq!(
        event_loop.check_termination(),
        Some(TerminationReason::ValidationFailure),
        "Should terminate at 3 malformed events"
    );
}

#[test]
fn test_stop_requested_termination_clears_signal() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let event_loop = EventLoop::new(config);

    let stop_path = temp_dir.path().join(".ralph/stop-requested");
    std::fs::create_dir_all(stop_path.parent().unwrap()).unwrap();
    std::fs::write(&stop_path, "").unwrap();

    assert_eq!(
        event_loop.check_termination(),
        Some(TerminationReason::Stopped),
        "Should terminate when stop requested signal exists"
    );
    assert!(
        !stop_path.exists(),
        "Stop signal should be removed after detection"
    );
}

#[test]
fn test_format_event_wraps_top_level_prompts() {
    // Kills: line 761 `==` → `!=` and `||` → `&&`
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Build a web server");

    let ralph = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph).unwrap();

    // task.start event should be wrapped in <top-level-prompt>
    assert!(
        prompt.contains("<top-level-prompt>"),
        "task.start events should be wrapped in <top-level-prompt> tags"
    );

    // Consume the start event, publish a non-top-level event
    event_loop
        .bus
        .publish(Event::new("build.done", "completed"));
    let prompt2 = event_loop.build_prompt(&ralph).unwrap();

    // build.done is NOT a top-level prompt, should NOT have the tag
    assert!(
        !prompt2.contains("<top-level-prompt>"),
        "Non-top-level events should NOT be wrapped in <top-level-prompt> tags"
    );
}

#[test]
fn test_check_ralph_completion_detection() {
    // Kills: line 1241 return `true` / `false`
    let config = RalphConfig::default();
    let event_loop = EventLoop::new(config);

    assert!(
        event_loop.check_ralph_completion(r#"<event topic="LOOP_COMPLETE">done</event>"#),
        "Should detect completion event"
    );
    assert!(
        !event_loop.check_ralph_completion("LOOP_COMPLETE\nMore text"),
        "Completion requires emitted event, not plain text"
    );
    assert!(
        !event_loop.check_ralph_completion("no match here"),
        "Should not detect completion in unrelated text"
    );
}

#[test]
fn test_scratchpad_injection_with_content() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let scratchpad_path = temp_dir.path().join(".ralph/agent/scratchpad.md");
    std::fs::create_dir_all(scratchpad_path.parent().unwrap()).unwrap();
    std::fs::write(
        &scratchpad_path,
        "## Progress\n- [x] Step 1\n- [ ] Step 2\n",
    )
    .unwrap();

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        prompt.contains("<scratchpad"),
        "Prompt should contain scratchpad header"
    );
    assert!(
        prompt.contains("Step 1"),
        "Prompt should contain scratchpad content"
    );
    assert!(
        prompt.contains("Step 2"),
        "Prompt should contain scratchpad content"
    );
}

#[test]
fn test_scratchpad_injection_no_file() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    // Do NOT create scratchpad file

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        !prompt.contains("<scratchpad path="),
        "Prompt should NOT contain scratchpad injection when file doesn't exist"
    );
}

#[test]
fn test_scratchpad_injection_empty_file() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let scratchpad_path = temp_dir.path().join(".ralph/agent/scratchpad.md");
    std::fs::create_dir_all(scratchpad_path.parent().unwrap()).unwrap();
    std::fs::write(&scratchpad_path, "   \n\n  ").unwrap();

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        !prompt.contains("<scratchpad path="),
        "Prompt should NOT contain scratchpad injection when file is empty/whitespace"
    );
}

#[test]
fn test_scratchpad_injection_ordering() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let scratchpad_path = temp_dir.path().join(".ralph/agent/scratchpad.md");
    std::fs::create_dir_all(scratchpad_path.parent().unwrap()).unwrap();
    std::fs::write(&scratchpad_path, "scratchpad marker content").unwrap();

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    let scratchpad_pos = prompt
        .find("<scratchpad")
        .expect("Should contain scratchpad");
    let orientation_pos = prompt
        .find("### 0a. ORIENTATION")
        .expect("Should contain orientation");

    assert!(
        scratchpad_pos < orientation_pos,
        "Scratchpad should appear before ORIENTATION in the prompt"
    );
}

#[test]
fn test_scratchpad_injection_tail_truncation() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let scratchpad_path = temp_dir.path().join(".ralph/agent/scratchpad.md");
    std::fs::create_dir_all(scratchpad_path.parent().unwrap()).unwrap();

    // Create content exceeding 16000 chars (4000 tokens * 4 chars/token)
    // Include markdown headings so truncation summary captures them
    let mut large_content = String::new();
    large_content.push_str("### Initial Analysis\n\n");
    for i in 0..500 {
        large_content.push_str(&format!("Line {}: some padding content here\n", i));
    }
    large_content.push_str("### Research Phase\n\n");
    for i in 500..1000 {
        large_content.push_str(&format!("Line {}: some padding content here\n", i));
    }
    large_content.push_str("### Implementation Notes\n\n");
    for i in 1000..2000 {
        large_content.push_str(&format!("Line {}: some padding content here\n", i));
    }
    assert!(
        large_content.len() > 16000,
        "Test content should exceed budget"
    );
    std::fs::write(&scratchpad_path, &large_content).unwrap();

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        prompt.contains("<scratchpad"),
        "Prompt should contain scratchpad header even when truncated"
    );
    assert!(
        prompt.contains("earlier content truncated"),
        "Prompt should indicate truncation occurred"
    );
    // Discarded headings should be summarized
    assert!(
        prompt.contains("discarded sections:"),
        "Prompt should summarize discarded section headings"
    );
    assert!(
        prompt.contains("### Initial Analysis"),
        "Prompt should list the discarded heading"
    );
    // The tail (most recent lines) should be kept
    assert!(
        prompt.contains("Line 1999"),
        "Last line should be preserved (tail kept)"
    );
    // Early lines should be truncated
    assert!(
        !prompt.contains("Line 0:"),
        "First line should be truncated (head removed)"
    );
}

#[test]
fn test_build_done_backpressure_accepts_mutants_warning() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: pass\nperformance: pass\nmutants: warn (65%)";
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"build.done".to_string()),
        "build.done with mutants warning should pass through. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"build.blocked".to_string()),
        "build.done should not be blocked by mutation warnings"
    );
}

#[test]
fn test_build_done_backpressure_rejects_high_complexity() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 12\nduplication: pass";
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"build.blocked".to_string()),
        "build.done with high complexity should be blocked. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"build.done".to_string()),
        "build.done should not pass through when complexity is too high"
    );
}

#[test]
fn test_build_done_backpressure_rejects_duplication() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: fail";
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"build.blocked".to_string()),
        "build.done with duplication should be blocked. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"build.done".to_string()),
        "build.done should not pass through when duplication fails"
    );
}

#[test]
fn test_build_done_backpressure_rejects_performance_regression() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 7\nduplication: pass\nperformance: regression";
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"build.blocked".to_string()),
        "build.done with performance regression should be blocked. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"build.done".to_string()),
        "build.done should not pass through when performance regresses"
    );
}

#[test]
fn test_review_done_backpressure_accepts_verified() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Write a review.done event WITH verification evidence
    write_event_to_jsonl(&events_path, "review.done", "tests: pass\nbuild: pass");
    let _ = event_loop.process_events_from_jsonl();

    // Should pass through as review.done (not blocked)
    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"review.done".to_string()),
        "Verified review.done should pass through. Got: {:?}",
        pending_topics
    );
}

#[test]
fn test_review_done_backpressure_rejects_unverified() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Write a review.done event WITHOUT verification evidence
    write_event_to_jsonl(&events_path, "review.done", "Looks good, approved!");
    let _ = event_loop.process_events_from_jsonl();

    // Should be transformed into review.blocked
    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"review.blocked".to_string()),
        "Unverified review.done should be blocked. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"review.done".to_string()),
        "review.done should not pass through without evidence"
    );
}

#[test]
fn test_review_done_backpressure_rejects_failed_checks() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Write a review.done event with failed checks
    write_event_to_jsonl(&events_path, "review.done", "tests: fail\nbuild: pass");
    let _ = event_loop.process_events_from_jsonl();

    // Should be transformed into review.blocked
    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"review.blocked".to_string()),
        "review.done with failed tests should be blocked. Got: {:?}",
        pending_topics
    );
}

#[test]
fn test_verify_passed_backpressure_accepts_quality_report() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = "quality.tests: pass\nquality.coverage: 82%\nquality.lint: pass\nquality.audit: pass\nquality.mutation: 72%\nquality.complexity: 7";
    write_event_to_jsonl(&events_path, "verify.passed", payload);
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"verify.passed".to_string()),
        "verify.passed with quality report should pass through. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"verify.failed".to_string()),
        "verify.passed should not be blocked by quality report"
    );
}

#[test]
fn test_verify_passed_backpressure_rejects_missing_quality_report() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "verify.passed", "All good");
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"verify.failed".to_string()),
        "verify.passed without quality report should be blocked. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"verify.passed".to_string()),
        "verify.passed should not pass through without quality report"
    );
}

#[test]
fn test_verify_passed_backpressure_rejects_failed_thresholds() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = "quality.tests: pass\nquality.coverage: 60%\nquality.lint: pass\nquality.audit: pass\nquality.mutation: 50%\nquality.complexity: 12";
    write_event_to_jsonl(&events_path, "verify.passed", payload);
    let _ = event_loop.process_events_from_jsonl();

    let empty = Vec::new();
    let pending_topics: Vec<String> = event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(
        pending_topics.contains(&"verify.failed".to_string()),
        "verify.passed with failing thresholds should be blocked. Got: {:?}",
        pending_topics
    );
    assert!(
        !pending_topics.contains(&"verify.passed".to_string()),
        "verify.passed should not pass through with failing thresholds"
    );
}

// === RObot Interaction Skill Injection Tests ===

#[test]
fn test_inject_robot_skill_when_enabled() {
    let yaml = r#"
RObot:
  enabled: true
  telegram:
    bot_token: "fake-token"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        prompt.contains("<robot-skill>"),
        "Prompt should contain <robot-skill> when RObot is enabled"
    );
    assert!(
        prompt.contains("human.interact"),
        "Robot skill should mention human.interact"
    );
    assert!(
        prompt.contains("</robot-skill>"),
        "Robot skill should have closing tag"
    );
}

#[test]
fn test_inject_robot_skill_skipped_when_disabled() {
    let config = RalphConfig::default(); // RObot disabled by default
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test prompt");

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();

    assert!(
        !prompt.contains("<robot-skill>"),
        "Prompt should NOT contain <robot-skill> when RObot is disabled"
    );
}

#[test]
fn test_persistent_mode_suppresses_loop_complete() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(&scratchpad_path, "## Tasks\n- [x] All done\n").unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    config.event_loop.persistent = true;
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // LOOP_COMPLETE should NOT terminate in persistent mode
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "Persistent mode should suppress LOOP_COMPLETE termination"
    );

    // Verify a task.resume event was injected so the loop continues
    let ralph_id = HatId::new("ralph");
    let pending = event_loop.bus.peek_pending(&ralph_id);
    assert!(
        pending.is_some_and(|events| events
            .iter()
            .any(|e| e.topic.as_str() == "task.resume" && e.payload.contains("Persistent mode"))),
        "A task.resume event should be injected after suppressed LOOP_COMPLETE"
    );
}

#[test]
fn test_non_persistent_mode_terminates_on_loop_complete() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(&scratchpad_path, "## Tasks\n- [x] All done\n").unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    // persistent defaults to false, but be explicit
    config.event_loop.persistent = false;
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // LOOP_COMPLETE should terminate normally when not persistent
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Non-persistent mode should terminate on LOOP_COMPLETE"
    );
}

#[test]
fn test_persistent_mode_still_respects_hard_limits() {
    let yaml = r"
event_loop:
  max_iterations: 2
  persistent: true
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.state.iteration = 2;

    // Hard limits should still terminate even in persistent mode
    assert_eq!(
        event_loop.check_termination(),
        Some(TerminationReason::MaxIterations),
        "Persistent mode should still respect max_iterations"
    );
}

#[test]
fn test_termination_reason_mappings() {
    let cases = vec![
        (TerminationReason::CompletionPromise, "completed", 0, true),
        (TerminationReason::MaxIterations, "max_iterations", 2, false),
        (TerminationReason::MaxRuntime, "max_runtime", 2, false),
        (TerminationReason::MaxCost, "max_cost", 2, false),
        (
            TerminationReason::ConsecutiveFailures,
            "consecutive_failures",
            1,
            false,
        ),
        (TerminationReason::LoopThrashing, "loop_thrashing", 1, false),
        (
            TerminationReason::ValidationFailure,
            "validation_failure",
            1,
            false,
        ),
        (TerminationReason::Stopped, "stopped", 1, false),
        (TerminationReason::Interrupted, "interrupted", 130, false),
        (
            TerminationReason::RestartRequested,
            "restart_requested",
            3,
            false,
        ),
    ];

    for (reason, expected_str, expected_code, is_success) in cases {
        assert_eq!(reason.as_str(), expected_str);
        assert_eq!(reason.exit_code(), expected_code);
        assert_eq!(reason.is_success(), is_success);
    }
}

#[test]
fn test_termination_status_texts() {
    let cases = vec![
        (
            TerminationReason::CompletionPromise,
            "All tasks completed successfully.",
        ),
        (
            TerminationReason::MaxIterations,
            "Stopped at iteration limit.",
        ),
        (TerminationReason::MaxRuntime, "Stopped at runtime limit."),
        (TerminationReason::MaxCost, "Stopped at cost limit."),
        (
            TerminationReason::ConsecutiveFailures,
            "Too many consecutive failures.",
        ),
        (
            TerminationReason::LoopThrashing,
            "Loop thrashing detected - same hat repeatedly blocked.",
        ),
        (
            TerminationReason::ValidationFailure,
            "Too many consecutive malformed JSONL events.",
        ),
        (TerminationReason::Stopped, "Manually stopped."),
        (TerminationReason::Interrupted, "Interrupted by signal."),
        (
            TerminationReason::RestartRequested,
            "Restarting by human request.",
        ),
    ];

    for (reason, expected) in cases {
        assert_eq!(termination_status_text(&reason), expected);
    }
}

#[test]
fn test_format_duration_variants() {
    use std::time::Duration;

    assert_eq!(format_duration(Duration::from_secs(45)), "45s");
    assert_eq!(format_duration(Duration::from_secs(61)), "1m 1s");
    assert_eq!(format_duration(Duration::from_hours(1)), "1h 0m 0s");
    assert_eq!(format_duration(Duration::from_secs(3661)), "1h 1m 1s");
}

#[test]
fn test_extract_task_id_first_line_and_default() {
    assert_eq!(
        EventLoop::extract_task_id(" task-123 \nMore details"),
        "task-123"
    );
    assert_eq!(EventLoop::extract_task_id(""), "unknown");
}

#[test]
fn test_mutation_warning_reason_variants() {
    let fail = MutationEvidence {
        status: MutationStatus::Fail,
        score_percent: Some(12.5),
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&fail, Some(80.0)).unwrap(),
        "mutation testing failed"
    );

    let warn = MutationEvidence {
        status: MutationStatus::Warn,
        score_percent: Some(65.5),
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&warn, Some(80.0)).unwrap(),
        "mutation score below threshold (65.50%)"
    );

    let unknown = MutationEvidence {
        status: MutationStatus::Unknown,
        score_percent: None,
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&unknown, Some(80.0)).unwrap(),
        "mutation testing status unknown"
    );

    let pass_low = MutationEvidence {
        status: MutationStatus::Pass,
        score_percent: Some(70.0),
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&pass_low, Some(80.0)).unwrap(),
        "mutation score 70.00% below threshold 80.00%"
    );

    let pass_missing = MutationEvidence {
        status: MutationStatus::Pass,
        score_percent: None,
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&pass_missing, Some(80.0)).unwrap(),
        "mutation score missing (threshold 80.00%)"
    );

    let pass_high = MutationEvidence {
        status: MutationStatus::Pass,
        score_percent: Some(95.0),
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&pass_high, Some(80.0)),
        None
    );

    let pass_no_threshold = MutationEvidence {
        status: MutationStatus::Pass,
        score_percent: Some(10.0),
    };
    assert_eq!(
        EventLoop::mutation_warning_reason(&pass_no_threshold, None),
        None
    );
}

#[test]
fn test_extract_prompt_id_prefers_xml_id() {
    let payload = r#"<event topic="user.prompt" id="q42">Question?</event>"#;
    assert_eq!(EventLoop::extract_prompt_id(payload), "q42");
}

#[test]
fn test_extract_prompt_id_fallback_prefix() {
    let id = EventLoop::extract_prompt_id("Plain question");
    assert!(id.starts_with('q'));
    assert!(id.len() > 1);
}

#[test]
fn test_check_for_user_prompt_extracts_id_and_text() {
    let event_loop = EventLoop::new(RalphConfig::default());
    let payload = r#"<event topic="user.prompt" id="q7">Need input</event>"#;
    let events = vec![
        Event::new("build.done", "ok"),
        Event::new("user.prompt", payload),
    ];

    let prompt = event_loop.check_for_user_prompt(&events).expect("prompt");
    assert_eq!(prompt.id, "q7");
    assert_eq!(prompt.text, payload);
}

#[test]
fn test_task_counts_and_open_task_list() {
    use crate::loop_context::LoopContext;
    use crate::task::{Task, TaskStatus};
    use crate::task_store::TaskStore;

    let temp_dir = tempfile::tempdir().unwrap();
    let loop_context = LoopContext::primary(temp_dir.path().to_path_buf());
    let event_loop = EventLoop::with_context(RalphConfig::default(), loop_context);

    let tasks_path = temp_dir.path().join(".ralph/agent/tasks.jsonl");
    let mut store = TaskStore::load(&tasks_path).unwrap();
    let mut closed = Task::new("Closed task".to_string(), 1);
    closed.status = TaskStatus::Closed;
    let open = Task::new("Open task".to_string(), 1);
    let open_id = open.id.clone();
    store.add(closed);
    store.add(open);
    store.save().unwrap();

    let (open_count, closed_count) = event_loop.count_tasks();
    assert_eq!(open_count, 1);
    assert_eq!(closed_count, 1);

    let open_list = event_loop.get_open_task_list();
    assert_eq!(open_list.len(), 1);
    assert!(open_list[0].contains(&open_id));
    assert!(open_list[0].contains("Open task"));
}

#[test]
fn test_verify_tasks_complete_missing_and_pending() {
    use crate::loop_context::LoopContext;
    use crate::task::Task;
    use crate::task_store::TaskStore;

    let temp_dir = tempfile::tempdir().unwrap();
    let loop_context = LoopContext::primary(temp_dir.path().to_path_buf());
    let event_loop = EventLoop::with_context(RalphConfig::default(), loop_context);

    // Missing tasks file should be treated as complete.
    assert!(event_loop.verify_tasks_complete().unwrap());

    let tasks_path = temp_dir.path().join(".ralph/agent/tasks.jsonl");
    let mut store = TaskStore::load(&tasks_path).unwrap();
    store.add(Task::new("Open task".to_string(), 1));
    store.save().unwrap();

    assert!(!event_loop.verify_tasks_complete().unwrap());
}

#[test]
fn test_verify_scratchpad_complete_variants() {
    use crate::loop_context::LoopContext;
    use std::fs;

    let temp_dir = tempfile::tempdir().unwrap();
    let loop_context = LoopContext::primary(temp_dir.path().to_path_buf());
    let event_loop = EventLoop::with_context(RalphConfig::default(), loop_context);

    assert!(event_loop.verify_scratchpad_complete().is_err());

    let scratchpad_path = temp_dir.path().join(".ralph/agent/scratchpad.md");
    fs::create_dir_all(scratchpad_path.parent().unwrap()).unwrap();
    fs::write(&scratchpad_path, "## Tasks\n- [ ] Pending\n").unwrap();
    assert!(!event_loop.verify_scratchpad_complete().unwrap());

    fs::write(&scratchpad_path, "## Tasks\n- [x] Done\n- [~] Cancelled\n").unwrap();
    assert!(event_loop.verify_scratchpad_complete().unwrap());
}

#[test]
fn test_termination_reason_exit_codes() {
    let cases = [
        (TerminationReason::CompletionPromise, 0),
        (TerminationReason::ConsecutiveFailures, 1),
        (TerminationReason::LoopThrashing, 1),
        (TerminationReason::ValidationFailure, 1),
        (TerminationReason::Stopped, 1),
        (TerminationReason::MaxIterations, 2),
        (TerminationReason::MaxRuntime, 2),
        (TerminationReason::MaxCost, 2),
        (TerminationReason::Interrupted, 130),
        (TerminationReason::RestartRequested, 3),
    ];

    for (reason, code) in cases {
        assert_eq!(reason.exit_code(), code, "{reason:?} exit code mismatch");
    }
}

#[test]
fn test_termination_reason_strings_and_flags() {
    let cases = [
        (TerminationReason::CompletionPromise, "completed", true),
        (TerminationReason::MaxIterations, "max_iterations", false),
        (TerminationReason::MaxRuntime, "max_runtime", false),
        (TerminationReason::MaxCost, "max_cost", false),
        (
            TerminationReason::ConsecutiveFailures,
            "consecutive_failures",
            false,
        ),
        (TerminationReason::LoopThrashing, "loop_thrashing", false),
        (
            TerminationReason::ValidationFailure,
            "validation_failure",
            false,
        ),
        (TerminationReason::Stopped, "stopped", false),
        (TerminationReason::Interrupted, "interrupted", false),
        (
            TerminationReason::RestartRequested,
            "restart_requested",
            false,
        ),
    ];

    for (reason, expected_str, is_success) in cases {
        assert_eq!(reason.as_str(), expected_str, "{reason:?} as_str mismatch");
        assert_eq!(
            reason.is_success(),
            is_success,
            "{reason:?} success mismatch"
        );
    }
}

#[test]
fn test_has_pending_human_events_detects_guidance() {
    let mut event_loop = EventLoop::new(RalphConfig::default());
    event_loop
        .bus
        .publish(Event::new("human.guidance", "Please focus on tests"));

    assert!(event_loop.has_pending_human_events());
}

#[test]
fn test_has_pending_human_events_ignores_non_human() {
    let mut event_loop = EventLoop::new(RalphConfig::default());
    event_loop.bus.publish(Event::new("task.start", "Do work"));

    assert!(!event_loop.has_pending_human_events());
}

#[test]
fn test_get_hat_publishes_returns_configured_topics() {
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.start"]
    publishes: ["task.plan", "build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    let publishes = event_loop.get_hat_publishes(&HatId::new("planner"));
    assert_eq!(
        publishes,
        vec!["task.plan".to_string(), "build.done".to_string()]
    );

    let missing = event_loop.get_hat_publishes(&HatId::new("missing"));
    assert!(missing.is_empty());
}

#[test]
fn test_inject_fallback_event_targets_last_hat() {
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    triggers: ["task.resume"]
    publishes: ["task.plan"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let planner_id = HatId::new("planner");

    event_loop.state.last_hat = Some(planner_id.clone());
    assert!(event_loop.inject_fallback_event());

    let pending = event_loop
        .bus
        .peek_pending(&planner_id)
        .expect("planner pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].topic.as_str(), "task.resume");
    assert_eq!(
        pending[0].target.as_ref().map(|id| id.as_str()),
        Some("planner")
    );
    assert!(
        pending[0]
            .payload
            .contains("Previous iteration by hat `planner` did not publish an event"),
        "Fallback payload should name the stalled hat"
    );
    assert!(
        pending[0].payload.contains("Allowed topics: `task.plan`"),
        "Fallback payload should list allowed publish topics"
    );

    let ralph_id = HatId::new("ralph");
    let ralph_pending = event_loop.bus.peek_pending(&ralph_id);
    assert!(ralph_pending.is_none_or(|events| events.is_empty()));
}

#[test]
fn test_inject_fallback_event_defaults_to_ralph() {
    let mut event_loop = EventLoop::new(RalphConfig::default());
    event_loop.state.last_hat = None;

    assert!(event_loop.inject_fallback_event());

    let ralph_id = HatId::new("ralph");
    let pending = event_loop
        .bus
        .peek_pending(&ralph_id)
        .expect("ralph pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].topic.as_str(), "task.resume");
    assert!(pending[0].target.is_none());
    assert!(pending[0].payload.contains("Review the scratchpad"));
}

#[test]
fn test_paths_use_loop_context_when_present() {
    use crate::loop_context::LoopContext;

    let temp_dir = tempfile::tempdir().unwrap();
    let loop_context = LoopContext::primary(temp_dir.path().to_path_buf());
    let event_loop = EventLoop::with_context(RalphConfig::default(), loop_context);

    assert_eq!(
        event_loop.tasks_path(),
        temp_dir.path().join(".ralph/agent/tasks.jsonl")
    );
    assert_eq!(
        event_loop.scratchpad_path(),
        temp_dir.path().join(".ralph/agent/scratchpad.md")
    );
}

#[test]
fn test_custom_scratchpad_overrides_loop_context() {
    use crate::loop_context::LoopContext;

    let temp_dir = tempfile::tempdir().unwrap();
    let loop_context = LoopContext::primary(temp_dir.path().to_path_buf());
    let mut config = RalphConfig::default();
    config.core.scratchpad.path = ".ralph/debug/global.md".to_string();

    let event_loop = EventLoop::with_context(config, loop_context);

    // Custom scratchpad path should be resolved relative to loop context workspace
    assert_eq!(
        event_loop.scratchpad_path(),
        temp_dir.path().join(".ralph/debug/global.md"),
        "Custom scratchpad in config should be resolved relative to workspace"
    );
}

#[test]
fn test_paths_fallback_to_config_when_no_context() {
    let temp_dir = tempfile::tempdir().unwrap();
    let scratchpad_path = temp_dir.path().join("scratchpad.md");
    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();

    let event_loop = EventLoop::new(config);

    assert_eq!(
        event_loop.tasks_path(),
        std::path::PathBuf::from(".ralph/agent/tasks.jsonl")
    );
    assert_eq!(event_loop.scratchpad_path(), scratchpad_path);
}

#[test]
fn test_record_hat_activations_increments_counts() {
    let mut event_loop = EventLoop::new(RalphConfig::default());
    let planner = HatId::new("planner");
    let reviewer = HatId::new("reviewer");

    event_loop.record_hat_activations(&[planner.clone(), reviewer.clone()]);
    event_loop.record_hat_activations(std::slice::from_ref(&planner));

    assert_eq!(
        event_loop.state.hat_activation_counts.get(&planner),
        Some(&2)
    );
    assert_eq!(
        event_loop.state.hat_activation_counts.get(&reviewer),
        Some(&1)
    );
}

#[test]
fn test_check_hat_exhaustion_emits_once_at_limit() {
    let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.done"]
    publishes: ["review.blocked"]
    max_activations: 2
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let hat_id = HatId::new("reviewer");
    let dropped = vec![
        Event::new("review.done", "ok"),
        Event::new("build.done", "ok"),
    ];

    event_loop
        .state
        .hat_activation_counts
        .insert(hat_id.clone(), 1);
    let (drop, event) = event_loop.check_hat_exhaustion(&hat_id, &dropped);
    assert!(!drop);
    assert!(event.is_none());

    event_loop
        .state
        .hat_activation_counts
        .insert(hat_id.clone(), 2);
    let (drop, event) = event_loop.check_hat_exhaustion(&hat_id, &dropped);
    assert!(drop);
    let exhausted = event.expect("exhausted event");
    assert_eq!(exhausted.topic.as_str(), "reviewer.exhausted");
    assert!(exhausted.payload.contains("max_activations: 2"));
    assert!(exhausted.payload.contains("activations: 2"));

    let (drop_again, event_again) = event_loop.check_hat_exhaustion(&hat_id, &dropped);
    assert!(drop_again);
    assert!(event_again.is_none());
}

// ── Phase 1: Hat Scope Enforcement Tests ──────────────────────────────

#[test]
fn test_scope_enforcement_drops_unauthorized_event() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  enforce_hat_scope: true
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Set builder as the active hat
    event_loop.state.last_active_hat_ids = vec![HatId::new("builder")];

    // Builder tries to emit LOOP_COMPLETE (not in its publishes)
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();

    // completion_requested should be false — the event was dropped by scope enforcement
    assert!(
        !event_loop.state.completion_requested,
        "LOOP_COMPLETE should be dropped when builder hat is active (not in publishes)"
    );
}

#[test]
fn test_scope_enforcement_allows_authorized_event() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  enforce_hat_scope: true
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done", "build.blocked"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Set builder as the active hat
    event_loop.state.last_active_hat_ids = vec![HatId::new("builder")];

    // Builder emits build.done (in its publishes) — should pass through
    write_event_to_jsonl(
        &events_path,
        "build.done",
        "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass",
    );
    let _ = event_loop.process_events_from_jsonl();

    // The event should have been published to the bus (not dropped)
    assert!(
        event_loop.has_pending_events(),
        "build.done should pass scope enforcement when builder is active"
    );
}

#[test]
fn test_scope_enforcement_allows_only_control_when_no_active_hats() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  enforce_hat_scope: true
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // No active hats (Ralph coordinating)
    event_loop.state.last_active_hat_ids = vec![];

    // LOOP_COMPLETE should pass through when Ralph is coordinating
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();

    assert!(
        event_loop.state.completion_requested,
        "LOOP_COMPLETE should be accepted when no active hats (Ralph coordinating)"
    );

    write_event_to_jsonl(&events_path, "build.done", "fake business event");
    let processed = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !processed
            .accepted_events
            .iter()
            .any(|event| event.topic.as_str() == "build.done"),
        "business events without an active hat should be rejected in hat-based mode"
    );
}

#[test]
fn test_scope_violation_event_published() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  enforce_hat_scope: true
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Set builder as the active hat
    event_loop.state.last_active_hat_ids = vec![HatId::new("builder")];

    // Builder tries to emit plan.approved (not in its publishes)
    write_event_to_jsonl(&events_path, "plan.approved", "Auto-approved");
    let _ = event_loop.process_events_from_jsonl();

    // A scope_violation event should have been published to the bus
    assert!(
        event_loop.has_pending_events(),
        "Scope violation event should be published to the bus"
    );
}

#[test]
fn test_ce_executor_shipper_cannot_publish_build_done() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  enforce_hat_scope: true
  completion_promise: LOOP_COMPLETE
hats:
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked", "fix.exhausted"]
    publishes: ["REVIEW_COMPLETE"]
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    event_loop.state.last_active_hat_ids = vec![HatId::new("shipper")];

    write_event_with_hat_to_jsonl(&events_path, "build.done", r#"{"ok":true}"#, "shipper");
    let processed = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        !processed
            .accepted_events
            .iter()
            .any(|event| event.topic.as_str() == "build.done"),
        "shipper must not be allowed to publish build.done"
    );
    assert!(
        event_loop.has_pending_events(),
        "scope violation should be published for rejected shipper build.done"
    );
}

// ── Phase 2: Event Chain Validation + loop.cancel Tests ───────────────

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
fn test_chain_validation_injects_task_resume_on_rejection() {
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

    // A task.resume event should have been published to the bus
    assert!(
        event_loop.has_pending_events(),
        "task.resume should be published on rejection"
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

// ── Phase 3: Human Timeout Event Injection Tests ──────────────────────

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

struct MockRobotService {
    timeout: u64,
    should_timeout: bool,
}

impl ralph_proto::RobotService for MockRobotService {
    fn send_question(&self, _payload: &str) -> anyhow::Result<i32> {
        Ok(1)
    }
    fn wait_for_response(&self, _events_path: &Path) -> anyhow::Result<Option<String>> {
        if self.should_timeout {
            Ok(None)
        } else {
            Ok(Some("approved".to_string()))
        }
    }
    fn send_checkin(
        &self,
        _: u32,
        _: Duration,
        _: Option<&ralph_proto::CheckinContext>,
    ) -> anyhow::Result<i32> {
        Ok(0)
    }
    fn timeout_secs(&self) -> u64 {
        self.timeout
    }
    fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }
    fn stop(self: Box<Self>) {}
}

struct RestartRequestRobotService;

impl ralph_proto::RobotService for RestartRequestRobotService {
    fn send_question(&self, _payload: &str) -> anyhow::Result<i32> {
        Ok(1)
    }

    fn wait_for_response(&self, _events_path: &Path) -> anyhow::Result<Option<String>> {
        Ok(Some("Please restart yourself now".to_string()))
    }

    fn send_checkin(
        &self,
        _: u32,
        _: Duration,
        _: Option<&ralph_proto::CheckinContext>,
    ) -> anyhow::Result<i32> {
        Ok(0)
    }

    fn timeout_secs(&self) -> u64 {
        5
    }

    fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn stop(self: Box<Self>) {}
}

#[test]
fn test_human_timeout_injects_timeout_event() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.set_robot_service(Box::new(MockRobotService {
        timeout: 5,
        should_timeout: true,
    }));

    // Write a human.interact event
    write_event_to_jsonl(&events_path, "human.interact", "Please review this plan");
    let _ = event_loop.process_events_from_jsonl();

    // The bus should have a human.timeout event (from the mock timeout)
    assert!(
        event_loop.has_pending_events(),
        "human.timeout event should be published on timeout"
    );
}

#[test]
fn test_human_response_still_works() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.set_robot_service(Box::new(MockRobotService {
        timeout: 5,
        should_timeout: false,
    }));

    // Write a human.interact event — mock returns "approved"
    write_event_to_jsonl(&events_path, "human.interact", "Please review this plan");
    let _ = event_loop.process_events_from_jsonl();

    // The bus should have a human.response event
    assert!(
        event_loop.has_pending_events(),
        "human.response event should be published when response received"
    );
}

/// Regression: start event written to JSONL by EventLogger must not be
/// re-read by `process_events_from_jsonl`, which would cause double-delivery.
/// The fix is to call `sync_event_reader_to_file_end()` after writing the
/// start event so the reader skips past it.
#[test]
fn test_sync_event_reader_prevents_start_event_double_delivery() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.starting_event = Some("work.start".to_string());

    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // 1. Initialize publishes start event to the bus (in-memory).
    event_loop.initialize("Run the test");

    // 2. Simulate EventLogger writing the same start event to the JSONL file.
    write_event_to_jsonl(&events_path, "work.start", "Run the test");

    // 3. Advance the reader past the logged entry.
    event_loop.sync_event_reader_to_file_end();

    // 4. Simulate an agent emitting a new event via `ralph emit`.
    write_event_to_jsonl(&events_path, "seed.ready", "initialized");

    // 5. process_events_from_jsonl should pick up ONLY seed.ready,
    //    not the already-published work.start.
    let processed = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        processed.had_events,
        "seed.ready should have been processed"
    );

    // Drain the bus and verify work.start appears exactly once (from initialize),
    // not twice (which would happen without the sync).
    let ralph_id = ralph_proto::HatId::new("ralph");
    let pending = event_loop.bus.take_pending(&ralph_id);
    let work_start_count = pending
        .iter()
        .filter(|e| e.topic.as_str() == "work.start")
        .count();
    assert_eq!(
        work_start_count, 1,
        "work.start must appear exactly once (from initialize), got {work_start_count}"
    );
    let seed_ready_count = pending
        .iter()
        .filter(|e| e.topic.as_str() == "seed.ready")
        .count();
    assert_eq!(
        seed_ready_count, 1,
        "seed.ready must appear exactly once (from JSONL), got {seed_ready_count}"
    );
}

#[test]
fn test_user_prompt_restart_request_creates_restart_signal_file() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "user.prompt", "Please restart yourself");
    let _ = event_loop.process_events_from_jsonl();

    assert!(
        temp_dir.path().join(".ralph/restart-requested").exists(),
        "user.prompt restart request should create restart signal file"
    );
}

#[test]
fn test_human_response_restart_request_creates_restart_signal_file() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.set_robot_service(Box::new(RestartRequestRobotService));

    write_event_to_jsonl(&events_path, "human.interact", "Need approval");
    let _ = event_loop.process_events_from_jsonl();

    assert!(
        temp_dir.path().join(".ralph/restart-requested").exists(),
        "human.response restart request should create restart signal file"
    );
}

// ─── Text fallback completion tests ─────────────────────────────────────────

#[test]
fn test_text_fallback_completions_respects_persistent_mode() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(&scratchpad_path, "## Tasks\n- [x] All done\n").unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    config.event_loop.persistent = true;
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    event_loop.request_completion_from_text_fallback();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "Text fallback completion should be suppressed in persistent mode"
    );
}

#[test]
fn test_text_fallback_completions_with_open_runtime_tasks() {
    use crate::loop_context::LoopContext;
    use crate::task::Task;
    use crate::task_store::TaskStore;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let tasks_path = temp_dir.path().join(".ralph/agent/tasks.jsonl");

    let mut store = TaskStore::load(&tasks_path).unwrap();
    let task1 = Task::new("Open task".to_string(), 1);
    store.add(task1);
    store.save().unwrap();

    let mut config = RalphConfig::default();
    config.memories.enabled = true;
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let loop_context = LoopContext::primary(temp_dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, loop_context);
    event_loop.initialize("Test");

    event_loop.request_completion_from_text_fallback();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "Text fallback completion should be rejected with open runtime tasks"
    );
    assert!(
        event_loop.has_pending_events(),
        "Rejecting completion should inject task.resume so the loop continues"
    );
}

#[test]
fn test_text_fallback_completions_with_missing_required_events() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    let agent_dir = temp_dir.path().join(".agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    std::fs::write(&scratchpad_path, "## Tasks\n- [x] All done\n").unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    config.event_loop.required_events = vec!["review.passed".to_string()];
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    event_loop.request_completion_from_text_fallback();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "Text fallback completion should be rejected when required events are missing"
    );
    // completion_requested should be reset after rejection
    assert!(
        !event_loop.state().completion_requested,
        "completion_requested should be reset after required-events rejection"
    );
    assert!(
        event_loop.has_pending_events(),
        "Rejecting completion should inject task.resume so the loop continues"
    );
}

#[test]
fn test_text_fallback_completions_succeeds_when_all_checks_pass() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(&scratchpad_path, "## Tasks\n- [x] Task 1 done\n").unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    event_loop.request_completion_from_text_fallback();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Text fallback completion should succeed when all safety checks pass"
    );
}

// ─── FR-1: Hat-level event allowlist filtering tests ────────────────────────

#[test]
fn test_event_filter_no_filter_sees_full_history() {
    // Regression: hat without event_filter sees all events in prompt.
    let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.trigger"]
    publishes: ["review.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("review.trigger", "trigger payload"));
    event_loop
        .bus
        .publish(Event::new("other.event", "other payload"));

    let ralph_id = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph_id).unwrap();

    assert!(
        prompt.contains("review.trigger"),
        "Prompt should contain review.trigger"
    );
    assert!(
        prompt.contains("other.event"),
        "Prompt should contain other.event when no filter is set"
    );
}

#[test]
fn test_event_filter_allowlist_filters_events() {
    // Only allowlisted events appear in the prompt.
    let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.trigger"]
    publishes: ["review.done"]
    event_filter:
      enabled: true
      events: ["review.trigger", "review.file"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("review.trigger", "trigger payload"));
    event_loop
        .bus
        .publish(Event::new("other.event", "other payload"));

    let ralph_id = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph_id).unwrap();

    assert!(
        prompt.contains("review.trigger"),
        "Prompt should contain allowlisted review.trigger"
    );
    assert!(
        !prompt.contains("other.event"),
        "Prompt should NOT contain non-allowlisted other.event"
    );
}

#[test]
fn test_event_filter_trigger_auto_included() {
    // Trigger events are automatically added to the allowlist.
    let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.trigger"]
    publishes: ["review.done"]
    event_filter:
      enabled: true
      events: ["review.file"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop
        .bus
        .publish(Event::new("review.trigger", "trigger payload"));
    event_loop
        .bus
        .publish(Event::new("review.file", "file payload"));
    event_loop
        .bus
        .publish(Event::new("other.event", "other payload"));

    let ralph_id = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph_id).unwrap();

    assert!(
        prompt.contains("review.trigger"),
        "Trigger event should be auto-included in prompt"
    );
    assert!(
        prompt.contains("review.file"),
        "Explicitly allowlisted event should be included"
    );
    assert!(
        !prompt.contains("other.event"),
        "Non-allowlisted event should be excluded"
    );
}

#[test]
fn test_event_filter_multi_hat_union_allowlist() {
    // When multiple active hats have filters, the allowlist is the union.
    let yaml = r#"
hats:
  hat_a:
    name: "Hat A"
    triggers: ["event.a"]
    publishes: ["done.a"]
    event_filter:
      enabled: true
      events: ["event.a"]
  hat_b:
    name: "Hat B"
    triggers: ["event.b"]
    publishes: ["done.b"]
    event_filter:
      enabled: true
      events: ["event.b"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.bus.publish(Event::new("event.a", "payload a"));
    event_loop.bus.publish(Event::new("event.b", "payload b"));
    event_loop.bus.publish(Event::new("event.c", "payload c"));

    let ralph_id = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph_id).unwrap();

    assert!(
        prompt.contains("event.a"),
        "Union allowlist should include event.a"
    );
    assert!(
        prompt.contains("event.b"),
        "Union allowlist should include event.b"
    );
    assert!(
        !prompt.contains("event.c"),
        "Union allowlist should exclude event.c"
    );
}

// ── Workflow Guard Integration Tests (Unit 7) ──

#[test]
fn test_workflow_guard_rejects_evaluated_before_scored() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // Configure workflow guard for AutoResearch experiment chain
    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Chain: planned -> ready -> measured
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Now try to skip scoring and go directly to evaluated - should be rejected
    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // evaluated should NOT be recorded as seen in workflow progress
    // Get phase AFTER processing to avoid borrow conflict
    // No correlation config → global instance (None key)
    let phase = event_loop
        .state
        .workflow_progress
        .get_phase("experiment", None);
    assert_eq!(
        phase,
        Some(2), // Still at measured (phase 2), not advanced to evaluated (phase 4)
        "experiment.evaluated before experiment.scored should not advance workflow"
    );
}

#[test]
fn test_workflow_guard_accepts_evaluated_after_scored() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Full chain in order
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.scored",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // After scoring, evaluated should advance the workflow
    // No correlation config → global instance (None key)
    let progress = &event_loop.state.workflow_progress;
    assert_eq!(
        progress.get_phase("experiment", None),
        Some(4), // Reached evaluated (phase 4)
        "experiment.evaluated after experiment.scored should advance workflow"
    );
}

#[test]
fn test_workflow_guard_periodic_review_does_not_advance_chain() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Chain: planned -> ready -> measured
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Interleave periodic.review - this should NOT advance the experiment chain
    write_event_to_jsonl(&events_path, "periodic.review", r#"{"status": "progress"}"#);
    let _ = event_loop.process_events_from_jsonl();

    // Now try to evaluate before scoring - still rejected
    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // periodic.review is not in the experiment chain, so workflow should still be at measured
    // evaluated was rejected because scored was never emitted
    // Get phase AFTER processing to avoid borrow conflict
    // No correlation config → global instance (None key)
    let phase = event_loop
        .state
        .workflow_progress
        .get_phase("experiment", None);
    assert_eq!(
        phase,
        Some(2), // Still at measured (phase 2) - evaluated was rejected
        "evaluated should still be rejected after periodic.review"
    );
}

#[test]
fn test_workflow_guard_completion_rejected_when_chain_incomplete() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Chain: planned -> ready -> measured (missing scored and evaluated)
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Try LOOP_COMPLETE before chain is complete
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();

    assert_eq!(
        reason, None,
        "LOOP_COMPLETE should be rejected when experiment chain is incomplete"
    );
}

#[test]
fn test_workflow_guard_completion_accepted_when_chain_complete() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Complete chain: planned -> ready -> measured -> scored -> evaluated
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.scored",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // LOOP_COMPLETE should now be accepted
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();

    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "LOOP_COMPLETE should be accepted when experiment chain is complete"
    );
}

#[test]
fn test_workflow_guard_instance_isolation_two_experiments() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
        correlation:
          from_payload: experiment_id
          from_topic: experiment.planned
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Experiment 1: fully complete
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.scored",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Experiment 2: only at measured
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-2"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-2"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-2"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    let progress = &event_loop.state.workflow_progress;

    // exp-1 should be at phase 4 (complete)
    assert_eq!(
        progress.get_phase("experiment", Some("exp-1")),
        Some(4),
        "exp-1 should be complete"
    );

    // exp-2 should be at phase 2 (measured)
    assert_eq!(
        progress.get_phase("experiment", Some("exp-2")),
        Some(2),
        "exp-2 should be at measured"
    );

    // Cannot evaluate exp-2 yet (needs scored first)
    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-2"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Get phase AFTER processing to verify evaluated was rejected
    let exp2_phase = event_loop
        .state
        .workflow_progress
        .get_phase("experiment", Some("exp-2"));
    assert_eq!(
        exp2_phase,
        Some(2), // Still at measured - evaluated was rejected until exp-2 is scored
        "exp-2 evaluated should be rejected until exp-2 is scored"
    );
}

#[test]
fn test_workflow_guard_rejection_publishes_task_resume_with_context() {
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Capture all published events via observer
    let captured_events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = captured_events.clone();
    event_loop.bus.add_observer(move |event: &Event| {
        captured.lock().unwrap().push(Event {
            topic: event.topic.clone(),
            payload: event.payload.clone(),
            source: event.source.clone(),
            target: event.target.clone(),
            wave_id: event.wave_id.clone(),
            wave_index: event.wave_index,
            wave_total: event.wave_total,
        });
    });

    // Advance to phase 2 (measured)
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.ready",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(
        &events_path,
        "experiment.measured",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Now try to skip scoring and go directly to evaluated - should be rejected
    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // Verify that a task.resume event was published with actionable context
    let events = captured_events.lock().unwrap();
    let resume_events: Vec<&Event> = events
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .collect();

    assert!(
        !resume_events.is_empty(),
        "A task.resume event should be published when a workflow guard rejects an event"
    );

    let last_resume = resume_events.last().unwrap();
    assert!(
        last_resume.payload.contains("WORKFLOW_GUARD_REJECTED"),
        "task.resume payload should indicate workflow guard rejection"
    );
    assert!(
        last_resume.payload.contains("next expected="),
        "task.resume payload should contain actionable next-expected context"
    );
    assert!(
        last_resume.payload.contains("experiment.scored"),
        "task.resume payload should mention the next expected topic"
    );
}

#[test]
fn test_workflow_guard_advisory_mode_accepts_out_of_order() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: advisory
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Skip ahead to evaluated without scoring — advisory should accept it
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(
        &events_path,
        "experiment.evaluated",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();

    // evaluated should be recorded as seen (in seen_topics)
    assert!(
        event_loop
            .state
            .seen_topics
            .contains("experiment.evaluated"),
        "Advisory mode should accept out-of-order events and record them as seen"
    );

    // Workflow progress should NOT advance for the skipped phase (advisory only advances valid phases)
    let phase = event_loop
        .state
        .workflow_progress
        .get_phase("experiment", None);
    assert_eq!(
        phase,
        Some(0),
        "Advisory mode should not advance progress for out-of-order events"
    );

    // LOOP_COMPLETE should NOT be blocked by advisory chains
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "LOOP_COMPLETE should be accepted when only advisory chains are incomplete"
    );
}

#[test]
fn test_workflow_guard_correlation_extraction_failure_rejects_event() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.scored
          - experiment.evaluated
        mode: strict
        correlation:
          from_payload: experiment_id
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Count task.resume events via lightweight observer
    let resume_count = Arc::new(AtomicUsize::new(0));
    let count = resume_count.clone();
    event_loop.bus.add_observer(move |event: &Event| {
        if event.topic.as_str() == "task.resume" {
            count.fetch_add(1, Ordering::SeqCst);
        }
    });

    // Valid first event with correlation key
    write_event_to_jsonl(
        &events_path,
        "experiment.planned",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", Some("exp-1")),
        Some(0)
    );

    // Event with malformed JSON payload should be rejected
    write_event_to_jsonl(&events_path, "experiment.scored", r"not-json-at-all");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", Some("exp-1")),
        Some(0),
        "Event with malformed JSON should not advance workflow"
    );

    // Event with missing correlation key should be rejected
    write_event_to_jsonl(
        &events_path,
        "experiment.scored",
        r#"{"other_field": "value"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", Some("exp-1")),
        Some(0),
        "Event with missing correlation key should not advance workflow"
    );

    // Event with non-string correlation value should be rejected
    write_event_to_jsonl(
        &events_path,
        "experiment.scored",
        r#"{"experiment_id": 123}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", Some("exp-1")),
        Some(0),
        "Event with non-string correlation value should not advance workflow"
    );

    // Verify recovery events were published for each rejection
    let observed = resume_count.load(Ordering::SeqCst);
    assert!(
        observed >= 3,
        "Should have published task.resume for each correlation extraction failure, got {}",
        observed
    );

    // Finally, a valid event should be accepted and allow recovery
    write_event_to_jsonl(
        &events_path,
        "experiment.scored",
        r#"{"experiment_id": "exp-1"}"#,
    );
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", Some("exp-1")),
        Some(1),
        "Valid event after rejections should advance workflow"
    );
}

#[test]
fn test_workflow_guard_recovery_after_rejection() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Advance normally to measured
    write_event_to_jsonl(&events_path, "experiment.planned", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(&events_path, "experiment.ready", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(&events_path, "experiment.measured", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(2)
    );

    // Try to skip scoring — rejected
    write_event_to_jsonl(&events_path, "experiment.evaluated", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(2),
        "Progress should remain at measured after rejected evaluated"
    );
    assert!(
        !event_loop
            .state
            .seen_topics
            .contains("experiment.evaluated"),
        "Rejected event should not be recorded as seen"
    );

    // Recovery: emit the correct next event (scored)
    write_event_to_jsonl(&events_path, "experiment.scored", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(3),
        "Scored should advance progress after recovery"
    );

    // Now evaluated should be accepted
    write_event_to_jsonl(&events_path, "experiment.evaluated", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(4),
        "Evaluated should be accepted after scoring"
    );

    // LOOP_COMPLETE should now succeed
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "LOOP_COMPLETE should succeed after recovery and full chain"
    );
}

#[test]
fn test_workflow_guard_rejects_old_phase_after_advance() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r"
event_loop:
  max_iterations: 10
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
        mode: strict
";
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Advance to ready (phase 1)
    write_event_to_jsonl(&events_path, "experiment.planned", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    write_event_to_jsonl(&events_path, "experiment.ready", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(1)
    );

    // Re-emit planned (phase 0) — should be accepted idempotently, no regression
    write_event_to_jsonl(&events_path, "experiment.planned", r"{}");
    let _ = event_loop.process_events_from_jsonl();
    assert_eq!(
        event_loop
            .state
            .workflow_progress
            .get_phase("experiment", None),
        Some(1),
        "Re-emitting old phase should not regress progress"
    );
}

// ---------------------------------------------------------------------------
// Isolated mode tests (U7)
// ---------------------------------------------------------------------------

#[test]
fn test_next_hat_isolated_returns_concrete_hat() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  strategist:
    name: "Strategist"
    description: "Plans"
    triggers: ["task.start"]
    publishes: ["experiment.planned"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    // After initialize, task.start is pending for strategist
    let next = event_loop.next_hat();
    assert!(next.is_some());
    assert_eq!(next.unwrap().as_str(), "strategist");
}

#[test]
fn test_next_hat_coordinator_returns_ralph() {
    let yaml = r#"
hats:
  strategist:
    name: "Strategist"
    description: "Plans"
    triggers: ["task.start"]
    publishes: ["experiment.planned"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    let next = event_loop.next_hat();
    assert!(next.is_some());
    assert_eq!(next.unwrap().as_str(), "ralph");
}

#[test]
fn test_isolated_prompt_contains_only_target_hat_instructions() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  implementer:
    name: "Implementer"
    description: "Implements"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
    instructions: "You are the implementer."
  reviewer:
    name: "Reviewer"
    description: "Reviews"
    triggers: ["experiment.ready"]
    publishes: ["review.done"]
    instructions: "You are the reviewer."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop
        .bus
        .publish(Event::new("experiment.planned", "Plan ready"));

    let next = event_loop.next_hat().unwrap().clone();
    assert_eq!(next.as_str(), "implementer");

    let prompt = event_loop.build_prompt(&next).unwrap();
    assert!(
        prompt.contains("You are the implementer."),
        "Prompt should contain implementer instructions"
    );
    assert!(
        !prompt.contains("You are the reviewer."),
        "Prompt should NOT contain reviewer instructions"
    );
    assert!(
        !prompt.contains("## HATS"),
        "Isolated prompt should not contain HATS section"
    );
}

#[test]
fn test_isolated_mode_accepts_only_first_business_event() {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  strategist:
    name: "Strategist"
    description: "Plans"
    triggers: ["task.start"]
    publishes: ["experiment.planned", "experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    // Simulate process_output to set current_isolated_hat
    event_loop.process_output(&HatId::new("strategist"), "output", true);

    // Simulate strategist emitting two business events
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    write_event_with_hat_to_jsonl(&events_path, "experiment.planned", "plan1", "strategist");
    write_event_with_hat_to_jsonl(&events_path, "experiment.ready", "ready1", "strategist");

    // Replace event_reader with our test file
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    // Only experiment.planned should be in seen_topics
    assert!(
        event_loop
            .state()
            .seen_topics
            .contains("experiment.planned"),
        "First business event should be accepted"
    );
    // experiment.ready should NOT be accepted
    assert!(
        !event_loop.state().seen_topics.contains("experiment.ready"),
        "Second business event should be dropped in isolated mode"
    );
}

// ── Characterization tests for existing event behavior (Unit 1) ──

#[test]
fn test_string_payload_events_pass_through_normally() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    write_event_to_jsonl(&events_path, "work.start", "Begin task");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    assert!(
        event_loop.state().seen_topics.contains("work.start"),
        "String payload event should pass through normally"
    );
}

/// Helper to write an event with an object payload to a JSONL file.
fn write_object_event_to_jsonl(path: &std::path::Path, topic: &str, payload: serde_json::Value) {
    use std::io::Write;
    let ts = chrono::Utc::now().to_rfc3339();
    let event_json = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": ts
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{}", event_json).unwrap();
}

#[test]
fn test_object_payload_events_from_jsonl_converted_to_string() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let object_payload = serde_json::json!({"status": "ok", "count": 42});
    write_object_event_to_jsonl(&events_path, "task.done", object_payload);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    assert!(
        event_loop.state().seen_topics.contains("task.done"),
        "Object payload event should be accepted after conversion"
    );

    // Verify the event on the bus has a string payload (serialized JSON)
    let ralph_id = HatId::new("ralph");
    let pending = event_loop.bus.peek_pending(&ralph_id);
    assert!(pending.is_some(), "Event should be on the bus");
    let events = pending.unwrap();
    let event = events.iter().find(|e| e.topic.as_str() == "task.done");
    assert!(event.is_some(), "build.done event should exist on bus");
    let payload = &event.unwrap().payload;
    assert!(
        payload.contains("status") && payload.contains("ok"),
        "Object payload should be converted to JSON string, got: {}",
        payload
    );
}

// ── Origin guard integration tests (Unit 3+4) ──

#[test]
fn test_origin_guard_accepts_valid_hat_event() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_with_hat_to_jsonl(&events_path, "build.done", "done", "builder");
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        result.had_events,
        "Valid hat + scope event should be accepted"
    );
}

#[test]
fn test_origin_guard_rejects_unknown_hat_event() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Event from an unknown hat (strategist is not registered)
    write_event_with_hat_to_jsonl(&events_path, "experiment.planned", "plan1", "strategist");
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !result.had_events,
        "Unknown hat event should be rejected by origin guard"
    );
}

#[test]
fn test_origin_guard_rejects_out_of_scope_event() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // builder does not publish plan.approved
    write_event_with_hat_to_jsonl(&events_path, "plan.approved", "approved", "builder");
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !result.had_events,
        "Out-of-scope event from registered hat should be rejected"
    );
}

#[test]
fn test_origin_guard_wave_event_unknown_hat_rejected() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "review.file"
    business_topics:
      - "task.done"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 2
    instructions: "Review."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Wave dispatch event from unknown hat
    {
        use std::io::Write;
        let event = serde_json::json!({
            "topic": "review.file",
            "payload": "src/main.rs",
            "ts": chrono::Utc::now().to_rfc3339(),
            "hat": "strategist",
            "wave_id": "w-1",
            "wave_index": 0,
            "wave_total": 1,
        });
        writeln!(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path)
                .unwrap(),
            "{}",
            event
        )
        .unwrap();
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();
    assert_eq!(
        result.wave_events.len(),
        0,
        "Wave event from unknown hat should be rejected by origin guard"
    );
}

#[test]
fn test_origin_guard_wave_event_valid_hat_accepted() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "review.file"
    business_topics:
      - "task.done"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.start"]
    publishes: ["review.file"]
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 2
    instructions: "Review."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Wave dispatch event from registered hat (coordinator publishes review.file)
    // The coordinator dispatches the wave, and reviewer receives it (concurrency > 1)
    {
        use std::io::Write;
        let event = serde_json::json!({
            "topic": "review.file",
            "payload": "src/main.rs",
            "ts": chrono::Utc::now().to_rfc3339(),
            "hat": "coordinator",
            "wave_id": "w-1",
            "wave_index": 0,
            "wave_total": 1,
        });
        writeln!(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path)
                .unwrap(),
            "{}",
            event
        )
        .unwrap();
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();
    assert_eq!(
        result.wave_events.len(),
        1,
        "Wave event from valid hat should be accepted by origin guard"
    );
}

#[test]
fn test_origin_guard_control_topic_without_hat_accepted() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // human.interact without hat should still work
    write_event_to_jsonl(&events_path, "human.interact", "What now?");
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        result.had_events,
        "Control topic without hat should be accepted"
    );
    assert!(
        result.human_interact_context.is_some(),
        "human.interact should produce interaction context"
    );
}

#[test]
fn test_origin_guard_mixed_batch_drops_invalid_only() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Write a mix: valid event, unknown hat event, valid event
    write_event_with_hat_to_jsonl(&events_path, "build.done", "first", "builder");
    write_event_with_hat_to_jsonl(&events_path, "plan.approved", "bad", "strategist");
    write_event_with_hat_to_jsonl(&events_path, "build.done", "second", "builder");

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        result.had_events,
        "Batch with at least one valid event should have had_events"
    );
}

// ── End origin guard integration tests ──

#[test]
fn test_workflow_guards_absent_means_no_chain_validation() {
    let yaml = r#"
event_loop:
  workflow_guards:
    chains: []
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // Emit out-of-order events that would violate a chain if one existed
    write_event_to_jsonl(&events_path, "experiment.evaluated", "done");
    write_event_to_jsonl(&events_path, "experiment.planned", "plan");

    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    assert!(
        event_loop
            .state()
            .seen_topics
            .contains("experiment.evaluated"),
        "Events should pass through when workflow_guards has empty chains"
    );
    assert!(
        event_loop
            .state()
            .seen_topics
            .contains("experiment.planned"),
        "Events should pass through when workflow_guards has empty chains"
    );
}

#[test]
fn test_empty_required_events_allows_completion() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    // Create scratchpad with all tasks completed
    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(&scratchpad_path, "## Tasks\n- [x] Task 1 done\n").unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    // required_events is empty by default
    assert!(config.event_loop.required_events.is_empty());

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Empty required_events should allow completion"
    );
}

#[test]
fn test_completion_promise_behavior_unchanged_without_event_policy() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();

    let agent_dir = temp_dir.path().join(".agent");
    fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    fs::write(&scratchpad_path, "## Tasks\n- [x] All done\n").unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    // Ensure no event_policy is configured (default)
    assert!(config.event_loop.event_policy.is_none());

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Finished");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "completion_promise behavior should be unchanged when event_policy is absent"
    );
}

// ── Event policy integration tests (Unit 4) ──

#[test]
fn test_event_policy_observe_mode_allows_bad_events_with_diagnostics() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: observe
    on_violation: warn
    schemas:
      test.topic:
        payload: json_object
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // String payload when JSON object required
    write_event_to_jsonl(&events_path, "test.topic", "plain string");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    // Event should still be on bus (observe mode)
    let ralph_id = HatId::new("ralph");
    let pending = event_loop.bus.peek_pending(&ralph_id);
    assert!(pending.is_some());
}

#[test]
fn test_event_policy_enforce_reject_replaces_with_task_resume() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      test.topic:
        payload: json_object
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    write_event_to_jsonl(&events_path, "test.topic", "plain string");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    // When event is rejected, validated_events is empty, so had_events is false
    // But task.resume is published directly to bus during policy validation
    assert!(
        !result.had_events,
        "Rejected events should not count as had_events"
    );
    // Bad event should NOT be on bus, but task.resume should be
    let ralph_id = HatId::new("ralph");
    let pending = event_loop.bus.peek_pending(&ralph_id);
    assert!(pending.is_some());
    let events = pending.unwrap();
    assert!(
        events.iter().any(|e| e.topic.as_str() == "task.resume"),
        "task.resume should be published for policy rejection"
    );
    assert!(
        !events.iter().any(|e| e.topic.as_str() == "test.topic"),
        "Bad event should NOT be on bus"
    );
}

#[test]
fn test_no_event_policy_skips_validation() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // Use a plain topic (not build.done which has special backpressure handling)
    write_event_to_jsonl(&events_path, "test.event", "Test payload");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(result.had_events);
    // Without event_policy, behavior should be unchanged
    assert!(event_loop.state().seen_topics.contains("test.event"));
}

#[test]
fn test_wave_events_update_terminal_observed_for_policy() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "review.file"
    business_topics:
      - "task.update"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 2
    instructions: "Review code."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Write a wave event with a terminal topic
    {
        use std::io::Write;
        let event = serde_json::json!({
            "topic": "review.file",
            "payload": "src/main.rs",
            "ts": chrono::Utc::now().to_rfc3339(),
            "wave_id": "w-1",
            "wave_index": 0,
            "wave_total": 1,
        });
        writeln!(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path)
                .unwrap(),
            "{}",
            event
        )
        .unwrap();
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();
    assert_eq!(
        result.wave_events.len(),
        1,
        "Wave event should be partitioned"
    );
    assert_eq!(result.wave_events[0].topic, "review.file");

    // Write a business event after the terminal topic
    write_event_to_jsonl(&events_path, "task.update", "update");

    let result = event_loop.process_events_from_jsonl().unwrap();
    // Business event after terminal should be rejected, so had_events is false
    assert!(
        !result.had_events,
        "Business event after terminal should be rejected"
    );

    // task.resume should be on the bus due to monotonicity violation
    let ralph_id = HatId::new("ralph");
    let pending = event_loop.bus.peek_pending(&ralph_id);
    assert!(pending.is_some());
    let events = pending.unwrap();
    assert!(
        events.iter().any(|e| e.topic.as_str() == "task.resume"),
        "task.resume should be published for terminal monotonicity violation"
    );
    assert!(
        events.iter().any(|e| e.payload.contains("monotonicity")),
        "Violation message should mention monotonicity"
    );
}

#[test]
fn test_check_completion_event_is_idempotent() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let ralph_id = HatId::new("ralph");

    // Set up completion request
    event_loop.state.completion_requested = true;

    // First call should succeed and set completion_honored
    let result1 = event_loop.check_completion_event();
    assert_eq!(result1, Some(TerminationReason::CompletionPromise));
    assert!(event_loop.state.completion_honored);

    // Capture bus state after first call
    let pending_after_first = event_loop
        .bus
        .peek_pending(&ralph_id)
        .map(|v| v.len())
        .unwrap_or(0);

    // Second call should return the same conclusion without side effects
    let result2 = event_loop.check_completion_event();
    assert_eq!(result2, Some(TerminationReason::CompletionPromise));

    // Bus event count should not have increased
    let pending_after_second = event_loop
        .bus
        .peek_pending(&ralph_id)
        .map(|v| v.len())
        .unwrap_or(0);
    assert_eq!(
        pending_after_second, pending_after_first,
        "Second check_completion_event call should not publish extra bus events"
    );
}

#[test]
fn test_request_completion_fallback_ignored_when_already_handled() {
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);

    // Simulate that completion was already handled
    event_loop.state.completion_honored = true;
    event_loop.state.completion_requested = false;

    // Text fallback request should be ignored
    event_loop.request_completion_from_text_fallback();
    assert!(
        !event_loop.state.completion_requested,
        "completion_requested should remain false when completion is already handled"
    );
}

#[test]
fn test_parsed_completion_event_ignored_when_already_handled() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Simulate that completion was already handled
    event_loop.state.completion_honored = true;
    event_loop.state.completion_requested = false;

    // Write a completion event to JSONL
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");

    // Process events — the parsed completion event should be ignored
    let _ = event_loop.process_events_from_jsonl();
    assert!(
        !event_loop.state.completion_requested,
        "Parsed completion event should be ignored when completion_honored is true"
    );
}

// ── Completion honored state tests (Unit 5) ──

#[test]
fn test_completion_honored_same_batch_duplicate_terminal_does_not_route() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: ignore
      business_after_completion: ignore
      write_diagnostic_event: false
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Batch: LOOP_COMPLETE, LOOP_COMPLETE
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Retry");

    let result = event_loop.process_events_from_jsonl().unwrap();
    // First LOOP_COMPLETE sets completion_requested, second is filtered by
    // same-batch completion guard (ignore) and does not produce had_events.
    assert!(
        event_loop.state.completion_requested,
        "First LOOP_COMPLETE should request completion"
    );

    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Completion should be accepted"
    );

    // No business events should have been published
    assert!(
        !result.had_plan_events,
        "Duplicate terminal should not produce plan events"
    );
}

#[test]
fn test_completion_honored_same_batch_business_event_does_not_route() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: ignore
      business_after_completion: ignore
      write_diagnostic_event: false
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Batch: LOOP_COMPLETE, experiment.planned
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    write_event_to_jsonl(&events_path, "experiment.planned", "{\"task_key\":\"a\"}");

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        event_loop.state.completion_requested,
        "LOOP_COMPLETE should request completion"
    );
    // experiment.planned is filtered by same-batch completion guard
    assert!(
        !result.had_plan_events,
        "Business event after completion in same batch should not trigger plan events"
    );
    assert!(
        !result.had_events,
        "Business event after completion in same batch should not produce any events"
    );

    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Completion should be accepted"
    );

    // Strategist should not have pending events from experiment.planned
    let strategist_pending = event_loop.bus.peek_pending(&HatId::new("strategist"));
    assert!(
        strategist_pending.map(|v| v.is_empty()).unwrap_or(true),
        "Business event after completion should not trigger strategist"
    );
}

#[test]
fn test_completion_honored_next_iteration_business_event_blocked() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: ignore
      business_after_completion: ignore
      write_diagnostic_event: false
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // First iteration: LOOP_COMPLETE
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, Some(TerminationReason::CompletionPromise));
    assert!(event_loop.state.completion_honored);

    // Second iteration: experiment.planned after completion honored
    // Need to reset event reader position and write new events
    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(&events_path2, "experiment.planned", "{\"task_key\":\"b\"}");

    let result = event_loop.process_events_from_jsonl().unwrap();
    // Business event should be blocked by completion-honored guard
    assert!(
        !result.had_events,
        "Business event after completion honored should not be published"
    );

    let strategist_pending = event_loop.bus.peek_pending(&HatId::new("strategist"));
    assert!(
        strategist_pending.map(|v| v.is_empty()).unwrap_or(true),
        "Business event after completion honored should not trigger strategist"
    );
}

#[test]
fn test_completion_honored_next_iteration_duplicate_terminal_blocked() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: ignore
      business_after_completion: ignore
      write_diagnostic_event: false
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // First iteration: LOOP_COMPLETE
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, Some(TerminationReason::CompletionPromise));
    assert!(event_loop.state.completion_honored);

    // Second iteration: duplicate LOOP_COMPLETE after completion honored
    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(&events_path2, "LOOP_COMPLETE", "Retry");

    let result = event_loop.process_events_from_jsonl().unwrap();
    // Duplicate terminal should be blocked by completion-honored guard
    assert!(
        !result.had_events,
        "Duplicate terminal after completion honored should not be published"
    );
    assert!(
        !result.had_plan_events,
        "Duplicate terminal after completion honored should not trigger plan events"
    );

    let strategist_pending = event_loop.bus.peek_pending(&HatId::new("strategist"));
    assert!(
        strategist_pending.map(|v| v.is_empty()).unwrap_or(true),
        "Duplicate terminal after completion honored should not trigger strategist"
    );
}

#[test]
fn test_completion_honored_loop_exit_reason_is_completion() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: ignore
      business_after_completion: ignore
      write_diagnostic_event: false
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Accept completion
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason1 = event_loop.check_completion_event();
    assert_eq!(reason1, Some(TerminationReason::CompletionPromise));

    // Even if more events arrive after completion is honored, termination reason
    // should still be CompletionPromise.
    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(&events_path2, "experiment.planned", "{\"task_key\":\"c\"}");
    let _ = event_loop.process_events_from_jsonl();

    let reason2 = event_loop.check_completion_event();
    assert_eq!(
        reason2,
        Some(TerminationReason::CompletionPromise),
        "Termination reason must remain CompletionPromise after honored completion"
    );
}

#[test]
fn test_completion_honored_old_config_without_completion_guard_behaves_as_before() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // Config with event_policy enabled but NO completion_after_terminal specified
    // (uses defaults: warn/warn/false)
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Old behavior: LOOP_COMPLETE should still work normally
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Old config should still accept completion normally"
    );

    // Old config default is Warn: business events after completion should still route
    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(
        &events_path2,
        "experiment.planned",
        "{\"task_key\":\"post-completion\"}",
    );

    let result = event_loop.process_events_from_jsonl().unwrap();
    eprintln!(
        "DEBUG: had_events={}, had_plan_events={}, completion_honored={}",
        result.had_events, result.had_plan_events, event_loop.state.completion_honored
    );
    eprintln!(
        "DEBUG: bus hats: {:?}",
        event_loop.bus.hat_ids().collect::<Vec<_>>()
    );
    assert!(
        result.had_events,
        "Old config with Warn default should allow business events after completion"
    );

    let strategist_pending = event_loop.bus.peek_pending(&HatId::new("strategist"));
    eprintln!("DEBUG: strategist_pending={:?}", strategist_pending);
    assert!(
        strategist_pending.map(|v| !v.is_empty()).unwrap_or(false),
        "Old config should trigger strategist for business events after completion"
    );
}

#[test]
fn test_completion_honored_reject_action_blocks_events() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: reject
      business_after_completion: reject
      write_diagnostic_event: true
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    use std::sync::Mutex;
    // Capture diagnostic events via observer
    let captured_events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = captured_events.clone();
    event_loop.bus.add_observer(move |event: &Event| {
        captured.lock().unwrap().push(Event {
            topic: event.topic.clone(),
            payload: event.payload.clone(),
            source: event.source.clone(),
            target: event.target.clone(),
            wave_id: event.wave_id.clone(),
            wave_index: event.wave_index,
            wave_total: event.wave_total,
        });
    });

    // First accept completion
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let _ = event_loop.check_completion_event();
    assert!(event_loop.state.completion_honored);

    // Next batch: business event after completion
    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(&events_path2, "experiment.planned", "{\"task_key\":\"d\"}");

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !result.had_events,
        "Reject action should block business event after completion"
    );

    // Diagnostic event should be published when write_diagnostic_event is true
    let has_diagnostic = captured_events
        .lock()
        .unwrap()
        .iter()
        .any(|e| e.topic.as_str() == "event.completion.blocked");
    assert!(
        has_diagnostic,
        "Reject action with write_diagnostic_event=true should publish event.completion.blocked"
    );
}

#[test]
fn test_completion_honored_warn_action_allows_event_with_warning() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "experiment.planned"
    completion_after_terminal:
      duplicate_terminal: warn
      business_after_completion: warn
      write_diagnostic_event: false
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // First accept completion
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let _ = event_loop.check_completion_event();
    assert!(event_loop.state.completion_honored);

    // Next batch: business event after completion with warn action
    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(&events_path2, "experiment.planned", "{\"task_key\":\"e\"}");

    let result = event_loop.process_events_from_jsonl().unwrap();
    // Warn action allows the event through (with a warning diagnostic)
    assert!(
        result.had_events,
        "Warn action should allow business event after completion"
    );
}

#[test]
fn test_state_machine_terminal_rejected_by_open_tasks_does_not_honor_terminal() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  required_events: [never.seen]
  state_machine:
    enabled: true
    instance_key:
      from_payload: task_key
      required_for: [experiment.planned]
    terminal_topics: [LOOP_COMPLETE]
    business_topics: [experiment.planned]
    terminal_guard:
      require_no_open_instances: false
    transitions:
      - topic: experiment.planned
        from: [idle]
        to: planned
        opens_instance: true
hats:
  strategist:
    name: "Strategist"
    triggers: ["experiment.planned"]
    publishes: ["experiment.planned"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();

    assert_eq!(reason, None);
    assert!(
        !event_loop
            .state
            .state_machine_runtime_state
            .as_ref()
            .unwrap()
            .is_terminal_honored(),
        "state machine terminal should not be honored until loop completion is honored"
    );

    let events_path2 = temp_dir.path().join("events2.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path2);
    write_event_to_jsonl(&events_path2, "experiment.planned", r#"{"task_key":"t1"}"#);
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        result.had_events,
        "business event should still be accepted after completion was rejected by open runtime tasks"
    );
}

#[test]
fn test_state_machine_branch_close_runs_before_workflow_guard() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.evaluated
        correlation:
          from_payload: task_key
  state_machine:
    enabled: true
    instance_key:
      from_payload: task_key
      required_for: [experiment.planned, experiment.blocked]
    terminal_topics: [LOOP_COMPLETE]
    business_topics: [experiment.planned, experiment.blocked]
    transitions:
      - topic: experiment.planned
        from: [idle]
        to: planned
        opens_instance: true
      - topic: experiment.blocked
        from: [planned]
        to: blocked
        closes_instance: true
hats:
  observer:
    name: "Observer"
    triggers: ["experiment.blocked"]
    publishes: ["LOOP_COMPLETE"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "experiment.planned", r#"{"task_key":"t1"}"#);
    write_event_to_jsonl(&events_path, "experiment.blocked", r#"{"task_key":"t1"}"#);
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        result.had_events,
        "state machine branch-close event should be accepted instead of rejected by linear workflow guards"
    );
    assert!(
        event_loop
            .state
            .state_machine_runtime_state
            .as_ref()
            .unwrap()
            .closed_instances_snapshot()
            .contains_key("t1"),
        "blocked should close the instance"
    );
}

#[test]
fn test_state_machine_processed_events_reports_only_accepted_events() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  state_machine:
    enabled: true
    instance_key:
      from_payload: task_key
      required_for: [experiment.planned, experiment.ready, experiment.blocked]
    terminal_topics: [LOOP_COMPLETE]
    business_topics: [experiment.planned, experiment.ready, experiment.blocked]
    transitions:
      - topic: experiment.planned
        from: [idle]
        to: planned
        opens_instance: true
      - topic: experiment.blocked
        from: [planned]
        to: blocked
        closes_instance: true
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "experiment.ready", r#"{"task_key":"bad"}"#);
    write_event_to_jsonl(&events_path, "experiment.planned", r#"{"task_key":"t1"}"#);
    write_event_to_jsonl(&events_path, "experiment.blocked", r#"{"task_key":"t1"}"#);
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");

    let result = event_loop.process_events_from_jsonl().unwrap();
    let topics: Vec<_> = result
        .accepted_events
        .iter()
        .map(|event| event.topic.as_str())
        .collect();

    assert_eq!(
        topics,
        vec!["experiment.planned", "experiment.blocked", "LOOP_COMPLETE"],
        "accepted event summary should omit rejected candidates and include accepted terminal"
    );
}

#[test]
fn test_verdict_gate_rejects_loop_complete_when_payload_is_fail() {
    use crate::config::VerdictGateConfig;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut config = RalphConfig::default();
    config.event_loop.verdict_gate = Some(VerdictGateConfig {
        topic: "review.complete".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
    });
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(
        &events_path,
        "review.complete",
        r#"{"pass_or_fail":"fail","verdict":"fail","final_findings_count":2}"#,
    );
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "verdict gate should reject LOOP_COMPLETE when review.complete carries pass_or_fail=fail"
    );
    assert!(
        event_loop.has_pending_events(),
        "Rejecting completion should inject task.resume so the loop continues"
    );
}

#[test]
fn test_verdict_gate_accepts_loop_complete_when_payload_is_pass() {
    use crate::config::VerdictGateConfig;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let mut config = RalphConfig::default();
    config.event_loop.verdict_gate = Some(VerdictGateConfig {
        topic: "review.complete".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
    });
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(
        &events_path,
        "review.complete",
        r#"{"pass_or_fail":"pass","verdict":"pass_with_residuals","final_findings_count":2}"#,
    );
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "verdict gate should accept LOOP_COMPLETE when review.complete carries pass_or_fail=pass"
    );
}

#[test]
fn test_no_verdict_gate_config_preserves_completion_behavior() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let config = RalphConfig::default();
    assert!(
        config.event_loop.verdict_gate.is_none(),
        "verdict_gate must default to None for backward compatibility"
    );
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Even if a review.complete event with pass_or_fail=fail is published, no verdict_gate
    // means the loop ignores it and accepts LOOP_COMPLETE (backward-compatible default).
    write_event_to_jsonl(
        &events_path,
        "review.complete",
        r#"{"pass_or_fail":"fail"}"#,
    );
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "without verdict_gate configured, LOOP_COMPLETE should be honored as before"
    );
}

// ---- P4: structured JSON evidence runtime integration tests ----

use crate::config::CoreConfig;

/// Helper: collect all topics from the event bus after processing.
fn collect_pending_topics(event_loop: &EventLoop) -> Vec<String> {
    let empty = Vec::new();
    event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn test_structured_build_done_json_pass_accepted() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = r#"{"checks":{"tests":"pass","lint":"pass","typecheck":"pass"}}"#;
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    assert!(
        topics.contains(&"build.done".to_string()),
        "structured pass should propagate build.done. Got: {topics:?}"
    );
    assert!(
        !topics.contains(&"build.blocked".to_string()),
        "structured pass must not emit build.blocked. Got: {topics:?}"
    );
}

#[test]
fn test_structured_build_done_json_missing_lint_blocks() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = r#"{"checks":{"tests":"pass","typecheck":"pass"}}"#;
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    assert!(
        topics.contains(&"build.blocked".to_string()),
        "missing lint should emit build.blocked. Got: {topics:?}"
    );
    assert!(
        !topics.contains(&"build.done".to_string()),
        "missing lint must not propagate build.done. Got: {topics:?}"
    );
}

#[test]
fn test_structured_build_done_json_missing_evidence_file_blocks() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = r#"{"checks":{"tests":"pass","lint":"pass","typecheck":"pass"},"evidence_files":["missing/never-created.log"]}"#;
    write_event_to_jsonl(&events_path, "build.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    assert!(
        topics.contains(&"build.blocked".to_string()),
        "missing evidence file should emit build.blocked. Got: {topics:?}"
    );
}

#[test]
fn test_legacy_text_build_done_still_passes() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Legacy text evidence uses the same evidence format that the JSON
    // path also requires: every required check + duplication. The
    // legacy parser returns duplication_passed=false when the field is
    // missing, so we must include it explicitly.
    write_event_to_jsonl(
        &events_path,
        "build.done",
        "tests: pass\nlint: pass\ntypecheck: pass\naudit: pass\ncoverage: pass\ncomplexity: 5\nduplication: pass",
    );
    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    assert!(
        topics.contains(&"build.done".to_string()),
        "legacy text evidence should still pass. Got: {topics:?}"
    );
}

#[test]
fn test_structured_review_done_json_pass_accepted() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = r#"{"checks":{"tests":"pass","build":"pass"}}"#;
    write_event_to_jsonl(&events_path, "review.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    assert!(
        topics.contains(&"review.done".to_string()),
        "structured review pass should propagate. Got: {topics:?}"
    );
    assert!(
        !topics.contains(&"review.blocked".to_string()),
        "structured review pass must not block. Got: {topics:?}"
    );
}

#[test]
fn test_structured_review_done_json_missing_build_blocks() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    let payload = r#"{"checks":{"tests":"pass"}}"#;
    write_event_to_jsonl(&events_path, "review.done", payload);
    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    assert!(
        topics.contains(&"review.blocked".to_string()),
        "missing build check should emit review.blocked. Got: {topics:?}"
    );
}

#[test]
fn test_structured_wave_review_done_exempt_from_blocking() {
    use std::io::Write;
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let config = RalphConfig {
        core: CoreConfig {
            workspace_root: temp_dir.path().to_path_buf(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut event_loop = EventLoop::new(config);
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Wave metadata must be at the event top-level (not in payload) so
    // the EventRecord picks it up and the event loop treats it as a
    // wave event.
    let wave_event = serde_json::json!({
        "topic": "review.done",
        "payload": r#"{"checks":{"tests":"pass","build":"pass"}}"#,
        "ts": chrono::Utc::now().to_rfc3339(),
        "wave_id": "w-1",
        "wave_index": 0,
        "wave_total": 2,
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .unwrap();
    writeln!(file, "{wave_event}").unwrap();

    let _ = event_loop.process_events_from_jsonl();

    let topics = collect_pending_topics(&event_loop);
    // Wave result events are exempt; the loop checks `!event.is_wave_event()`
    // before applying the structured JSON path, so we should NOT see
    // review.blocked even though the payload itself is structured.
    assert!(
        !topics.contains(&"review.blocked".to_string()),
        "wave review.done must not be blocked. Got: {topics:?}"
    );
}

// ===========================================================================
// Unit 6: Completion rejection stale-breaker progress tracking tests
// ===========================================================================

/// Helper to set up an event loop with required_events configured.
fn setup_loop_with_required_events(required: Vec<String>) -> EventLoop {
    let yaml = format!(
        r#"
event_loop:
  required_events:
{}
"#,
        required
            .iter()
            .map(|t| format!("    - \"{}\"", t))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
    EventLoop::new(config)
}

/// Helper to set up an event loop with memories enabled and a task store.
fn setup_loop_with_tasks(temp_dir: &std::path::Path) -> EventLoop {
    use crate::loop_context::LoopContext;
    use crate::task::Task;
    use crate::task_store::TaskStore;

    let tasks_path = temp_dir.join(".ralph/agent/tasks.jsonl");
    std::fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();

    // Create task store with one open task
    let mut store = TaskStore::load(&tasks_path).unwrap();
    let task = Task::new("Open task".to_string(), 1);
    store.add(task);
    store.save().unwrap();

    let mut config = RalphConfig::default();
    config.memories.enabled = true;
    config.core.workspace_root = temp_dir.to_path_buf();

    let loop_context = LoopContext::primary(temp_dir.to_path_buf());
    EventLoop::with_context(config, loop_context)
}

/// Helper to set up an event loop with workflow guards.
fn setup_loop_with_workflow_guards() -> EventLoop {
    use crate::config::{WorkflowChain, WorkflowChainMode, WorkflowGuardsConfig};

    let mut config = RalphConfig::default();
    config.event_loop.workflow_guards = Some(WorkflowGuardsConfig {
        chains: vec![WorkflowChain {
            name: "experiment".to_string(),
            topics: vec![
                "experiment.planned".to_string(),
                "experiment.ready".to_string(),
                "experiment.measured".to_string(),
                "experiment.scored".to_string(),
            ],
            mode: WorkflowChainMode::Strict,
            correlation: None,
        }],
    });

    EventLoop::new(config)
}

#[test]
fn test_stale_breaker_first_rejection_injects_resume() {
    // First rejection: should inject task.resume and NOT terminate
    let mut event_loop = setup_loop_with_required_events(vec!["review.done".to_string()]);
    event_loop.initialize("Test");

    // Simulate completion requested without required events
    event_loop.state.completion_requested = true;

    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "First rejection should not terminate");
    assert!(event_loop.has_pending_events(), "Should inject task.resume");
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 1,
        "Should have 1 consecutive rejection"
    );
}

#[test]
fn test_stale_breaker_second_rejection_still_no_terminate() {
    // Second same rejection: should still NOT terminate
    let mut event_loop = setup_loop_with_required_events(vec!["review.done".to_string()]);
    event_loop.initialize("Test");

    // First rejection
    event_loop.state.completion_requested = true;
    let _ = event_loop.check_completion_event();
    // Consume the task.resume event
    event_loop.build_prompt(&HatId::new("ralph"));
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);

    // Second rejection (same signature, no progress)
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Second same rejection should not terminate");
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 2,
        "Should have 2 consecutive rejections"
    );
}

#[test]
fn test_stale_breaker_third_rejection_returns_loop_stale() {
    // Third same rejection with no progress: should return LoopStale
    let mut event_loop = setup_loop_with_required_events(vec!["review.done".to_string()]);
    event_loop.initialize("Test");

    // First rejection
    event_loop.state.completion_requested = true;
    let _ = event_loop.check_completion_event();
    event_loop.build_prompt(&HatId::new("ralph"));

    // Second rejection
    event_loop.state.completion_requested = true;
    let _ = event_loop.check_completion_event();
    event_loop.build_prompt(&HatId::new("ralph"));

    // Third rejection
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::LoopStale),
        "Third same rejection with no progress should return LoopStale"
    );
}

#[test]
fn test_stale_breaker_open_tasks_three_rejections() {
    // Open runtime tasks causing completion rejection 3 times returns LoopStale
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let mut event_loop = setup_loop_with_tasks(temp_dir.path());
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // First rejection
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "First rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);

    // Second rejection
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Second rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 2);

    // Third rejection
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::LoopStale),
        "Third rejection with open tasks should return LoopStale"
    );
}

#[test]
fn test_stale_breaker_workflow_guard_three_rejections() {
    // Workflow guard incomplete 3 times returns LoopStale
    let mut event_loop = setup_loop_with_workflow_guards();
    event_loop.initialize("Test");

    // Start a workflow instance at phase 0 (simulates experiment.planned)
    // We need to advance the workflow progress directly since publishing to bus
    // doesn't go through workflow guard validation.
    event_loop
        .state
        .workflow_progress
        .advance("experiment", None, 0);

    // First rejection
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "First rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);

    // Second rejection
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Second rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 2);

    // Third rejection
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::LoopStale),
        "Third workflow guard rejection should return LoopStale"
    );
}

#[test]
fn test_stale_breaker_business_event_resets_counter() {
    // Two rejections with an accepted business event in between: counter resets
    let mut event_loop = setup_loop_with_required_events(vec!["review.done".to_string()]);
    event_loop.initialize("Test");

    // First rejection
    event_loop.state.completion_requested = true;
    let _ = event_loop.check_completion_event();
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);

    // Simulate accepted business event (adds to seen_topics as a business topic)
    event_loop
        .state
        .seen_topics
        .insert("build.task".to_string());

    // Second rejection (should see progress and reset counter)
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Should not terminate with progress");
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 1,
        "Counter should reset to 1 after business event progress"
    );
}

#[test]
fn test_stale_breaker_task_state_change_resets_counter() {
    // Two rejections with task state change in between: counter resets.
    // After closing the task, completion should be accepted (not rejected).
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let mut event_loop = setup_loop_with_tasks(temp_dir.path());
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // First rejection (task is open)
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "First rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);

    // Close the open task (simulates task state change)
    use crate::task_store::TaskStore;
    let tasks_path = temp_dir.path().join(".ralph/agent/tasks.jsonl");
    let mut store = TaskStore::load(&tasks_path).unwrap();
    let task_id = store.open().first().map(|t| t.id.clone());
    if let Some(id) = task_id {
        store.close(&id);
    }
    store.save().unwrap();

    // Second attempt: task is now closed, completion should be accepted
    // (progress was made, counter reset, and completion is now valid)
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "After task closed, completion should be accepted"
    );
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 0,
        "Counter should be 0 after acceptance"
    );
}

#[test]
fn test_stale_breaker_system_event_does_not_reset() {
    // Two rejections with only system/diagnostic events in between: no reset
    let mut event_loop = setup_loop_with_required_events(vec!["review.done".to_string()]);
    event_loop.initialize("Test");

    // First rejection
    event_loop.state.completion_requested = true;
    let _ = event_loop.check_completion_event();
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);

    // Add only system topics (should NOT count as progress)
    event_loop
        .state
        .seen_topics
        .insert("event.malformed".to_string());
    event_loop
        .state
        .seen_topics
        .insert("human.guidance".to_string());
    event_loop
        .state
        .seen_topics
        .insert("task.resume".to_string());

    // Second rejection (should NOT see progress from system topics)
    event_loop.state.completion_requested = true;
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Should not terminate yet");
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 2,
        "Counter should NOT reset for system topics"
    );
}

#[test]
fn test_stale_breaker_normal_completion_still_works() {
    // Normal accepted completion still returns CompletionPromise
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let agent_dir = temp_dir.path().join(".agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let scratchpad_path = agent_dir.join("scratchpad.md");
    std::fs::write(
        &scratchpad_path,
        "## Tasks\n- [x] Task 1 done\n- [x] Task 2 done\n",
    )
    .unwrap();

    let mut config = RalphConfig::default();
    config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Completion event with all tasks done
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Normal completion should still return CompletionPromise"
    );
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 0,
        "Rejection counter should be 0 after acceptance"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Unit 7: ce-executor completion chain verification (replay-light)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_report_done_satisfies_ce_executor_completion_gate() {
    // Regression: `report.done` must satisfy the ce-executor completion gate.
    // This mirrors the production flow where reporter emits report.done, then
    // LOOP_COMPLETE terminates.
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = setup_loop_with_required_events(vec!["report.done".to_string()]);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Simulate report.done being seen (this is the required event)
    event_loop
        .state
        .seen_topics
        .insert("report.done".to_string());

    // Write LOOP_COMPLETE to JSONL
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();

    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "report.done + LOOP_COMPLETE should terminate as CompletionPromise"
    );
}

#[test]
fn test_post_completion_business_events_do_not_reset_stale_breaker() {
    // Regression: after completion is accepted, post-completion fake/raw business
    // events must not reset the stale-breaker counter or trigger re-processing.
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = setup_loop_with_required_events(vec!["report.done".to_string()]);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Completion with report.done satisfied
    event_loop
        .state
        .seen_topics
        .insert("report.done".to_string());
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "First completion should succeed"
    );

    // After completion is honored, inject a fake post-completion business event
    // (simulating raw output parsing leaking into the events file)
    write_event_to_jsonl(&events_path, "experiment.planned", r#"{"task_key":"x"}"#);
    let _ = event_loop.process_events_from_jsonl();

    // The stale-breaker counter should remain 0 since completion was already honored
    assert_eq!(
        event_loop.state.consecutive_completion_rejections, 0,
        "Post-completion events should not reset stale-breaker after honored completion"
    );
    assert!(
        event_loop.state.completion_honored,
        "completion_honored should remain true after post-completion events"
    );
}

#[test]
fn test_old_bad_required_events_stale_breaks() {
    // Regression: old bad config with mutually exclusive required events
    // (e.g. review.passed + review.complete as required) should stale-break
    // instead of infinitely retrying.
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = setup_loop_with_required_events(vec![
        "review.passed".to_string(),
        "review.complete".to_string(),
    ]);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Simulate: only review.complete appears (not review.passed)
    event_loop
        .state
        .seen_topics
        .insert("review.complete".to_string());

    // First rejection
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "First rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 1);
    // Consume the task.resume event that was injected
    event_loop.build_prompt(&HatId::new("ralph"));

    // Second rejection (same signature, no progress)
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Second rejection should not terminate");
    assert_eq!(event_loop.state.consecutive_completion_rejections, 2);

    // Third rejection: stale-break
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::LoopStale),
        "Third rejection with old bad required-events should stale-break"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Unit 8: ce-executor plan-gate behavioral regression (replay-light)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_ce_executor_review_passed_routes_to_plan_gate_not_shipper() {
    // R11 regression: After review.passed, plan-gate must be the active hat.
    // Shipper must NOT activate on review.passed. Reporter must NOT activate.
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready", "queue.advance"]
    publishes: ["work.done"]
    instructions: "EXECUTOR MODE — Implement the task."
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["work.done"]
    publishes: ["review.passed", "review.complete"]
    instructions: "SYNTHESIZER MODE — Merge findings."
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed", "review.complete"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    default_publishes: "plan.blocked"
    instructions: "PLAN GATE MODE — Decide queue.advance vs plan.complete."
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked", "fix.exhausted"]
    publishes: ["REVIEW_COMPLETE"]
    default_publishes: "REVIEW_COMPLETE"
    instructions: "SHIPPER MODE — Final validation and commit."
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"
    instructions: "REPORTER MODE — Generate manager report."
event_loop:
  starting_event: "work.ready"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Simulate review.passed arriving at the bus (as if review-synthesizer emitted it)
    event_loop.bus.publish(Event::new(
        "review.passed",
        r#"{"plan_name":"test","task_id":"t1","task_key":"k1","step":"1"}"#,
    ));

    // next_hat should return ralph (the constant coordinator)
    assert_eq!(
        event_loop.next_hat().unwrap().as_str(),
        "ralph",
        "ralph should coordinate after review.passed"
    );

    // build_prompt should activate plan-gate, NOT shipper or reporter
    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        prompt.contains("PLAN GATE MODE"),
        "review.passed should route to plan-gate. prompt did not contain 'PLAN GATE MODE'"
    );
    assert!(
        !prompt.contains("SHIPPER MODE"),
        "review.passed should NOT route to shipper. prompt contained 'SHIPPER MODE'"
    );
    assert!(
        !prompt.contains("REPORTER MODE"),
        "review.passed should NOT route to reporter. prompt contained 'REPORTER MODE'"
    );
}

#[test]
fn test_ce_executor_queue_advance_routes_to_executor_not_reporter() {
    // R11 regression: After plan-gate emits queue.advance, executor must activate.
    // Reporter/shipper must NOT activate on queue.advance.
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready", "queue.advance"]
    publishes: ["work.done"]
    instructions: "EXECUTOR MODE — Implement the task."
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    default_publishes: "plan.blocked"
    instructions: "PLAN GATE MODE — Decide queue.advance vs plan.complete."
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked"]
    publishes: ["REVIEW_COMPLETE"]
    default_publishes: "REVIEW_COMPLETE"
    instructions: "SHIPPER MODE — Final validation and commit."
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"
    instructions: "REPORTER MODE — Generate manager report."
event_loop:
  starting_event: "work.ready"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    // Simulate plan-gate emitting queue.advance
    event_loop.bus.publish(Event::new(
        "queue.advance",
        r#"{"plan_name":"test","next_step":"2","reviewed_task_id":"t1","reviewed_task_key":"k1"}"#,
    ));

    assert_eq!(
        event_loop.next_hat().unwrap().as_str(),
        "ralph",
        "ralph should coordinate after queue.advance"
    );

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        prompt.contains("EXECUTOR MODE"),
        "queue.advance should route to executor. prompt did not contain 'EXECUTOR MODE'"
    );
    assert!(
        !prompt.contains("SHIPPER MODE"),
        "queue.advance should NOT route to shipper. prompt contained 'SHIPPER MODE'"
    );
    assert!(
        !prompt.contains("REPORTER MODE"),
        "queue.advance should NOT route to reporter. prompt contained 'REPORTER MODE'"
    );
}

#[test]
fn test_ce_executor_review_complete_fail_routes_to_plan_gate_not_shipper() {
    // R11 regression: review.complete with verdict=fail must route to plan-gate,
    // NOT directly to shipper. Shipper only activates on plan.blocked/plan.complete.
    let yaml = r#"
hats:
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["work.done"]
    publishes: ["review.passed", "review.failed", "review.complete"]
    instructions: "SYNTHESIZER MODE — Merge findings."
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.passed", "review.complete"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    default_publishes: "plan.blocked"
    instructions: "PLAN GATE MODE — Decide queue.advance vs plan.complete."
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked", "fix.exhausted"]
    publishes: ["REVIEW_COMPLETE"]
    default_publishes: "REVIEW_COMPLETE"
    instructions: "SHIPPER MODE — Final validation and commit."
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"
    instructions: "REPORTER MODE — Generate manager report."
event_loop:
  starting_event: "work.ready"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.bus.publish(Event::new(
        "review.complete",
        r#"{"plan_name":"test","verdict":"fail","task_id":"t1","task_key":"k1","step":"1"}"#,
    ));

    assert_eq!(
        event_loop.next_hat().unwrap().as_str(),
        "ralph",
        "ralph should coordinate after review.complete"
    );

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        prompt.contains("PLAN GATE MODE"),
        "review.complete(verdict=fail) should route to plan-gate. prompt did not contain 'PLAN GATE MODE'"
    );
    assert!(
        !prompt.contains("SHIPPER MODE"),
        "review.complete should NOT route directly to shipper. prompt contained 'SHIPPER MODE'"
    );
    assert!(
        !prompt.contains("REPORTER MODE"),
        "review.complete should NOT route to reporter. prompt contained 'REPORTER MODE'"
    );
}

#[test]
fn test_ce_executor_plan_blocked_routes_to_shipper_not_reporter() {
    // R11 regression: plan.blocked must route to shipper, which publishes REVIEW_COMPLETE.
    // Reporter must NOT activate on plan.blocked directly.
    let yaml = r#"
hats:
  plan-gate:
    name: "Plan Gate"
    triggers: ["review.complete"]
    publishes: ["queue.advance", "plan.complete", "plan.blocked"]
    default_publishes: "plan.blocked"
    instructions: "PLAN GATE MODE — Decide queue.advance vs plan.complete."
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked", "fix.exhausted"]
    publishes: ["REVIEW_COMPLETE"]
    default_publishes: "REVIEW_COMPLETE"
    instructions: "SHIPPER MODE — Final validation and commit."
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
    default_publishes: "report.done"
    instructions: "REPORTER MODE — Generate manager report."
event_loop:
  starting_event: "work.ready"
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.bus.publish(Event::new(
        "plan.blocked",
        r#"{"plan_name":"test","reason":"state mismatch","task_id":"t1","task_key":"k1"}"#,
    ));

    assert_eq!(
        event_loop.next_hat().unwrap().as_str(),
        "ralph",
        "ralph should coordinate after plan.blocked"
    );

    let prompt = event_loop.build_prompt(&HatId::new("ralph")).unwrap();
    assert!(
        prompt.contains("SHIPPER MODE"),
        "plan.blocked should route to shipper. prompt did not contain 'SHIPPER MODE'"
    );
    assert!(
        !prompt.contains("REPORTER MODE"),
        "plan.blocked should NOT route directly to reporter. prompt contained 'REPORTER MODE'"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// U8: Execution Contract Integration Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_execution_contract_rejects_work_done_with_missing_payload() {
    // Test that work.done without required payload fields is rejected
    // This tests the execution contract validator directly
    use crate::config::{
        ContractRejectConfig, ExecutionContractRule, GitChangeRequirement,
        TaskCompletionRequirement, TestEvidenceRequirement,
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

// ─────────────────────────────────────────────────────────────────────────────
// U5: Contract Rejection Interactions (supplement to U8 plan)
// ─────────────────────────────────────────────────────────────────────────────
//
// These tests verify that when an agent emits a `work.done` event but the
// execution contract rejects it, the event loop:
//   1. Does NOT publish the original `work.done` to subscribers (so downstream
//      hats like `review` are not triggered).
//   2. DOES publish the structured `event.execution_contract.rejected` diagnostic
//      and a `human.guidance` event.
//   3. Reports `had_rejected_events = true` and `had_raw_events = true` so the
//      loop runner's missing-event gate does not fire for "tried but invalid"
//      attempts (only for "did not try at all").

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
    // When execution contracts are disabled, no rejections occur and the
    // flags should reflect the default. This is a regression guard for the
    // flag semantics: `had_rejected_events` is exclusively about contract
    // rejections, not about malformed events or other failures.
    let yaml = r#"
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

// ─────────────────────────────────────────────────────────────────────────────
// 2026-06-04: Contract rejection recovery routing characterization tests
// (docs/plans/2026-06-04-001-fix-contract-rejection-hat-retry-plan.md)
//
// These tests characterize the gap: a rejected `work.done` must produce a
// targeted recovery event for the source hat so the next prompt activates
// the source hat, not the Ralph fallback.
// ─────────────────────────────────────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────────────────────
// U8: Replay-light integration tests (deterministic event loop paths)
// ─────────────────────────────────────────────────────────────────────────────
//
// These tests construct a real git repository and task store in a tempdir,
// then drive the event loop's `process_parse_result` to verify the
// execution-contract gate behaves correctly across the full pipeline:
//   - No events at all → hard gate (no `work.done` synthesized from defaults)
//   - Open task + work.done → rejected (TaskNotTerminal)
//   - Closed task + diff → accepted
//   - Closed task + clean + new commit (vs. loop start SHA) → accepted
//   - Closed task + clean + no new commits → rejected (NoGitEvidence)
//
// They run the real `DefaultGitEvidenceProvider` against a real git repo so
// the integration path is exercised end-to-end. The previous
// `test_execution_contract_validates_task_status` only covered config
// parsing; these tests cover the real event loop pipeline.

mod replay_light_integration {
    use crate::config::RalphConfig;
    use crate::event_loop::EventLoop;
    use crate::event_reader::ParseResult;
    use crate::task::{Task, TaskStatus};
    use crate::task_store::TaskStore;
    use ralph_proto::Event;
    use std::process::Command;
    use tempfile::TempDir;

    fn init_git_repo(dir: &std::path::Path) {
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap_or_else(|e| {
                    panic!("git {:?} failed: {}", args, e);
                })
        };
        run(&["init", "--initial-branch=main"]);
        run(&["config", "user.email", "test@test.local"]);
        run(&["config", "user.name", "Test User"]);
        // Ignore the .ralph state directory so it does not show up as
        // untracked changes when we later assert the worktree is clean.
        std::fs::write(dir.join(".gitignore"), ".ralph/\n").unwrap();
        std::fs::write(dir.join("README.md"), "# Test\n").unwrap();
        run(&["add", ".gitignore", "README.md"]);
        run(&["commit", "-m", "Initial commit"]);
    }

    fn git_head_sha(dir: &std::path::Path) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn build_test_config(workspace_root: &std::path::Path) -> RalphConfig {
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
        let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        config.core.workspace_root = workspace_root.to_path_buf();
        config
    }

    fn write_task(tasks_path: &std::path::Path, task_id: &str, status: TaskStatus) {
        let parent = tasks_path.parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        let mut store = TaskStore::load(tasks_path).unwrap();
        let mut task = Task::new("Test task".to_string(), 1);
        task.id = task_id.to_string();
        task.key = Some("k1".to_string());
        task.status = status;
        store.add(task);
        store.save().unwrap();
    }

    fn work_done_event(task_id: &str) -> crate::event_reader::Event {
        crate::event_reader::Event {
            topic: "work.done".to_string(),
            payload: Some(format!(
                r#"{{"plan_name":"p","plan_path":"/p","task_id":"{}","task_key":"k1","step":"step-01"}}"#,
                task_id
            )),
            ts: "2024-01-01T00:00:00Z".to_string(),
            wave_id: None,
            hat: Some("executor".to_string()),
            triggered: None,
            source: None,
            wave_index: None,
            wave_total: None,
        }
    }

    fn make_event_loop(config: RalphConfig) -> EventLoop {
        // Use `with_context` so `tasks_path()` resolves to the test
        // workspace's `.ralph/agent/tasks.jsonl`. `EventLoop::new` falls
        // back to a path relative to the current working directory, which
        // would point at the repo's own task store and never see the test
        // task that `write_task` just saved.
        let workspace = config.core.workspace_root.clone();
        let ctx = crate::loop_context::LoopContext::primary(workspace);
        EventLoop::with_context(config, ctx)
    }

    fn contract_disabled_config(workspace_root: &std::path::Path) -> RalphConfig {
        let mut config = build_test_config(workspace_root);
        if let Some(ref mut contracts) = config.event_loop.execution_contracts {
            contracts.enabled = false;
        }
        config
    }

    fn process_events(
        events: Vec<crate::event_reader::Event>,
        event_loop: &mut EventLoop,
    ) -> crate::ProcessedEvents {
        event_loop
            .process_parse_result(ParseResult {
                events,
                malformed: vec![],
            })
            .expect("process_parse_result should succeed")
    }

    #[test]
    fn test_no_events_triggers_hard_gate_at_event_loop_layer() {
        // The event loop layer must NOT synthesize a default `work.done`.
        // When the agent writes no events at all, the bus sees nothing and
        // the loop runner's missing-event gate is what should fire later.
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);

        let config = contract_disabled_config(workspace);
        let mut event_loop = make_event_loop(config);

        let result = process_events(vec![], &mut event_loop);

        // No events at the event loop layer.
        assert!(!result.had_events);
        assert!(!result.had_raw_events);
        assert!(!result.had_rejected_events);
        assert!(result.accepted_events.is_empty());
        assert!(result.contract_rejections.is_empty());
    }

    #[test]
    fn test_open_task_work_done_rejected() {
        // task status = open, payload complete → contract rejects with
        // TaskNotTerminal. The work.done must NOT be published.
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);
        let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
        write_task(&tasks_path, "test-id-1", TaskStatus::Open);

        let config = build_test_config(workspace);
        let mut event_loop = make_event_loop(config);

        let result = process_events(vec![work_done_event("test-id-1")], &mut event_loop);

        // The contract rejected the event.
        assert!(
            !result.contract_rejections.is_empty(),
            "Contract should reject open task"
        );
        assert!(result.had_rejected_events);
        assert!(
            !result.had_events,
            "Original work.done must not be accepted"
        );
        // No `work.done` in accepted events.
        assert!(
            !result
                .accepted_events
                .iter()
                .any(|e| e.topic.as_str() == "work.done")
        );
    }

    #[test]
    fn test_closed_task_work_done_with_diff_accepted() {
        // task status = closed + git has uncommitted diff → contract accepts.
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);
        let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
        write_task(&tasks_path, "test-id-1", TaskStatus::Closed);

        // Modify a tracked file so `git diff --quiet` exits 1 (has diff).
        // Modifying a tracked file produces an unstaged change, which is
        // what `DefaultGitEvidenceProvider::has_uncommitted_changes` checks.
        std::fs::write(
            workspace.join("README.md"),
            "# Test\nagent change for diff\n",
        )
        .unwrap();

        let config = build_test_config(workspace);
        let mut event_loop = make_event_loop(config);

        let result = process_events(vec![work_done_event("test-id-1")], &mut event_loop);

        // Contract accepts the work.done.
        assert!(
            result.contract_rejections.is_empty(),
            "Contract should accept closed task with diff, got: {:?}",
            result.contract_rejections
        );
        assert!(!result.had_rejected_events);
        assert!(result.had_events);
        // The original work.done is in accepted events.
        assert!(
            result
                .accepted_events
                .iter()
                .any(|e| e.topic.as_str() == "work.done")
        );
    }

    #[test]
    fn test_git_evidence_rejection_no_diff_no_commit() {
        // task status = closed + git has no uncommitted changes AND no new
        // commits since the loop start → contract rejects with NoGitEvidence.
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);
        let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
        write_task(&tasks_path, "test-id-1", TaskStatus::Closed);

        // Record the loop start SHA (no commits after this).
        let start_sha = git_head_sha(workspace);

        let config = build_test_config(workspace);
        let mut event_loop = make_event_loop(config);
        event_loop.set_loop_start_sha(Some(start_sha));

        let result = process_events(vec![work_done_event("test-id-1")], &mut event_loop);

        // No git evidence → contract rejected.
        assert!(
            !result.contract_rejections.is_empty(),
            "Contract should reject when no diff and no new commits"
        );
        let has_no_git_evidence = result.contract_rejections.iter().any(|f| {
            matches!(
                f.kind,
                crate::execution_contract::ExecutionContractViolationKind::NoGitEvidence { .. }
            )
        });
        assert!(
            has_no_git_evidence,
            "Expected NoGitEvidence finding, got: {:?}",
            result.contract_rejections
        );
        assert!(result.had_rejected_events);
        assert!(!result.had_events);
    }

    #[test]
    fn test_git_evidence_accepted_with_new_commit_after_loop_start() {
        // U4 regression: previously the validator passed `None` as the
        // baseline SHA, so `has_new_commits_since` always returned `false`.
        // After a commit lands and the worktree is clean, the agent should
        // still be able to declare `work.done`. This test pins that the
        // `set_loop_start_sha(Some(baseline))` path is what makes
        // commit-only evidence work.
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);
        let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
        write_task(&tasks_path, "test-id-1", TaskStatus::Closed);

        // Record the loop start SHA BEFORE making the agent's commit.
        let start_sha = git_head_sha(workspace);

        // Simulate the agent making a commit and leaving a clean worktree.
        std::fs::write(workspace.join("agent-change.txt"), "agent commit\n").unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(workspace)
                .output()
                .unwrap()
        };
        git(&["add", "agent-change.txt"]);
        git(&["commit", "-m", "Agent work"]);

        // Worktree should be clean.
        let status_out = git(&["status", "--porcelain"]);
        assert!(
            status_out.stdout.is_empty(),
            "Worktree should be clean after commit, got: {}",
            String::from_utf8_lossy(&status_out.stdout)
        );

        let config = build_test_config(workspace);
        let mut event_loop = make_event_loop(config);
        event_loop.set_loop_start_sha(Some(start_sha));

        let result = process_events(vec![work_done_event("test-id-1")], &mut event_loop);

        // The agent's commit counts as git evidence → contract accepts.
        assert!(
            result.contract_rejections.is_empty(),
            "Contract should accept closed task with new commit, got: {:?}",
            result.contract_rejections
        );
        assert!(result.had_events);
        assert!(
            result
                .accepted_events
                .iter()
                .any(|e| e.topic.as_str() == "work.done")
        );
    }

    #[test]
    fn test_trivial_step_accepted_without_git_evidence() {
        // The `trivial` step is in `allow_empty_for_steps` so the git
        // evidence check is skipped. With no diff and no new commits, but
        // a closed task and a trivial step, the contract should accept.
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);
        let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
        write_task(&tasks_path, "test-id-1", TaskStatus::Closed);

        let start_sha = git_head_sha(workspace);

        let config = build_test_config(workspace);
        let mut event_loop = make_event_loop(config);
        event_loop.set_loop_start_sha(Some(start_sha));

        let mut event = work_done_event("test-id-1");
        event.payload = Some(
            r#"{"plan_name":"p","plan_path":"/p","task_id":"test-id-1","task_key":"k1","step":"trivial"}"#
                .to_string(),
        );

        let result = process_events(vec![event], &mut event_loop);

        assert!(
            result.contract_rejections.is_empty(),
            "Trivial step should skip git evidence check, got: {:?}",
            result.contract_rejections
        );
        assert!(result.had_events);
    }

    /// Regression guard: the EventLoop's bus observer sees the structured
    /// diagnostic and human.guidance when contract rejection happens.
    #[test]
    fn test_rejection_publishes_diagnostic_and_guidance_to_bus() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);
        let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
        write_task(&tasks_path, "test-id-1", TaskStatus::Open); // open → will be rejected

        let config = build_test_config(workspace);
        let mut event_loop = make_event_loop(config);

        let observed: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_clone = std::sync::Arc::clone(&observed);
        event_loop.bus().add_observer(move |event: &Event| {
            observed_clone
                .lock()
                .unwrap()
                .push(event.topic.as_str().to_string());
        });

        let result = process_events(vec![work_done_event("test-id-1")], &mut event_loop);
        assert!(!result.contract_rejections.is_empty());

        let observed_topics = observed.lock().unwrap().clone();
        assert!(
            observed_topics
                .iter()
                .any(|t| t == "event.execution_contract.rejected"),
            "Diagnostic event should be published, observed: {:?}",
            observed_topics
        );
        assert!(
            observed_topics.iter().any(|t| t == "human.guidance"),
            "Guidance event should be published, observed: {:?}",
            observed_topics
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // 2026-06-04 plan U6: Accepted and rejected end-to-end event-loop tests
    // ─────────────────────────────────────────────────────────────────────────
    //
    // These tests exercise the full pipeline through real `EventLoop` +
    // `EventBus` + `HatRegistry` + task store to prove the contract rejection
    // recovery path works as a single integrated flow (not just isolated
    // unit assertions). They cover R10/R11/R12/R14.

    /// Accepted path: closed task + complete payload + diff → work.done
    /// is published to the bus and reviewer becomes the next active hat.
    #[test]
    fn test_accepted_work_done_routes_to_reviewer() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);
        let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
        write_task(&tasks_path, "test-id-1", TaskStatus::Closed);

        // Provide git evidence: modify a tracked file.
        std::fs::write(
            workspace.join("README.md"),
            "# Test\nagent change for diff\n",
        )
        .unwrap();

        let config = build_test_config(workspace);
        let mut event_loop = make_event_loop(config);

        let result = process_events(vec![work_done_event("test-id-1")], &mut event_loop);

        assert!(
            result.contract_rejections.is_empty(),
            "Closed task + diff should be accepted, got: {:?}",
            result.contract_rejections
        );
        assert!(result.had_events);
        assert!(
            result
                .accepted_events
                .iter()
                .any(|e| e.topic.as_str() == "work.done"),
            "Original work.done must be in accepted events"
        );

        // Reviewer's pending queue should contain the work.done event.
        let reviewer_id = ralph_proto::HatId::new("reviewer");
        let reviewer_pending = event_loop
            .bus
            .peek_pending(&reviewer_id)
            .cloned()
            .unwrap_or_default();
        assert!(
            reviewer_pending
                .iter()
                .any(|e| e.topic.as_str() == "work.done"),
            "Reviewer must receive the accepted work.done. Pending: {:?}",
            reviewer_pending
                .iter()
                .map(|e| e.topic.as_str())
                .collect::<Vec<_>>()
        );

        // The next active hat should be the reviewer (downstream of work.done).
        let active_hat_id = event_loop.get_active_hat_id();
        assert_eq!(
            active_hat_id.as_str(),
            "reviewer",
            "Accepted work.done must activate reviewer as the next hat"
        );
    }

    /// Rejected path: open task → work.done is dropped, executor receives
    /// a targeted `task.resume` retry event, reviewer stays inactive.
    #[test]
    fn test_rejected_open_task_routes_retry_to_executor_not_reviewer() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);
        let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
        write_task(&tasks_path, "test-id-1", TaskStatus::Open);

        let config = build_test_config(workspace);
        let mut event_loop = make_event_loop(config);

        let result = process_events(vec![work_done_event("test-id-1")], &mut event_loop);

        // Contract rejected the open task.
        assert!(
            !result.contract_rejections.is_empty(),
            "Open task should be rejected"
        );
        let has_task_not_terminal = result.contract_rejections.iter().any(|f| {
            matches!(
                f.kind,
                crate::execution_contract::ExecutionContractViolationKind::TaskNotTerminal { .. }
            )
        });
        assert!(
            has_task_not_terminal,
            "Expected TaskNotTerminal finding, got: {:?}",
            result.contract_rejections
        );

        // Original work.done is not in accepted events.
        assert!(
            !result
                .accepted_events
                .iter()
                .any(|e| e.topic.as_str() == "work.done"),
            "Rejected work.done must not be in accepted events"
        );

        // Reviewer must not have work.done in its queue.
        let reviewer_id = ralph_proto::HatId::new("reviewer");
        let reviewer_pending = event_loop
            .bus
            .peek_pending(&reviewer_id)
            .cloned()
            .unwrap_or_default();
        assert!(
            !reviewer_pending
                .iter()
                .any(|e| e.topic.as_str() == "work.done"),
            "Reviewer must not see rejected work.done"
        );

        // Executor must receive a targeted retry event (not just human.guidance).
        let executor_id = ralph_proto::HatId::new("executor");
        let executor_pending = event_loop
            .bus
            .peek_pending(&executor_id)
            .cloned()
            .unwrap_or_default();
        let targeted_retry = executor_pending.iter().find(|e| {
            e.topic.as_str() != "human.guidance"
                && e.target.as_ref().map(|t| t.as_str()) == Some("executor")
        });
        assert!(
            targeted_retry.is_some(),
            "Executor must receive a targeted retry for rejected work.done. \
             Pending: {:?}",
            executor_pending
                .iter()
                .map(|e| (e.topic.as_str(), e.target.as_ref().map(|t| t.as_str())))
                .collect::<Vec<_>>()
        );

        // Next active hat must be executor, not reviewer or ralph.
        let active_hat_id = event_loop.get_active_hat_id();
        assert_eq!(
            active_hat_id.as_str(),
            "executor",
            "After rejected work.done, the next active hat must be executor via \
             targeted retry, not reviewer/ralph. Got: {}",
            active_hat_id.as_str()
        );
    }

    /// Rejected path: missing `plan_path` in payload → finding names the
    /// missing field, retry target remains executor.
    #[test]
    fn test_rejected_missing_plan_path_names_finding_and_routes_retry() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);
        let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
        write_task(&tasks_path, "test-id-1", TaskStatus::Closed);

        std::fs::write(
            workspace.join("README.md"),
            "# Test\nagent change for diff\n",
        )
        .unwrap();

        let config = build_test_config(workspace);
        let mut event_loop = make_event_loop(config);

        // Build event WITHOUT plan_path.
        let mut event = work_done_event("test-id-1");
        event.payload = Some(
            r#"{"plan_name":"p","task_id":"test-id-1","task_key":"k1","step":"step-01"}"#
                .to_string(),
        );

        let result = process_events(vec![event], &mut event_loop);

        assert!(
            !result.contract_rejections.is_empty(),
            "Missing plan_path should reject"
        );
        let has_missing_plan_path = result.contract_rejections.iter().any(|f| {
            matches!(
                f.kind,
                crate::execution_contract::ExecutionContractViolationKind::MissingPayloadField { ref field }
                    if field == "plan_path"
            )
        });
        assert!(
            has_missing_plan_path,
            "Expected MissingPayloadField(plan_path) finding, got: {:?}",
            result.contract_rejections
        );

        // Retry target remains executor.
        let executor_id = ralph_proto::HatId::new("executor");
        let executor_pending = event_loop
            .bus
            .peek_pending(&executor_id)
            .cloned()
            .unwrap_or_default();
        let targeted_retry = executor_pending.iter().find(|e| {
            e.target.as_ref().map(|t| t.as_str()) == Some("executor")
                && e.topic.as_str() != "human.guidance"
        });
        assert!(
            targeted_retry.is_some(),
            "Even with missing plan_path, retry target must be executor"
        );
    }

    #[test]
    fn test_rejected_work_done_retry_payload_reaches_executor_prompt() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);
        let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
        write_task(&tasks_path, "test-id-1", TaskStatus::Closed);

        std::fs::write(
            workspace.join("README.md"),
            "# Test\nagent change for diff\n",
        )
        .unwrap();

        let config = build_test_config(workspace);
        let mut event_loop = make_event_loop(config);

        let mut event = work_done_event("test-id-1");
        event.payload = Some(
            r#"{"plan_name":"p","task_id":"test-id-1","task_key":"k1","step":"step-01"}"#
                .to_string(),
        );

        let result = process_events(vec![event], &mut event_loop);

        assert!(
            !result.contract_rejections.is_empty(),
            "Missing plan_path should reject"
        );

        let prompt = event_loop
            .build_prompt(&ralph_proto::HatId::new("ralph"))
            .expect("contract rejection retry should build a prompt");

        assert_eq!(
            event_loop
                .state
                .last_active_hat_ids
                .first()
                .map(|id| id.as_str()),
            Some("executor"),
            "Retry prompt should activate executor"
        );
        assert!(
            prompt.contains("rejected_topic") && prompt.contains("work.done"),
            "Retry prompt must include structured rejected topic context. Prompt:\n{}",
            prompt
        );
        assert!(
            prompt.contains("original_payload") && prompt.contains("plan_path"),
            "Retry prompt must include original payload and finding context. Prompt:\n{}",
            prompt
        );
    }

    /// Retry path: after a targeted retry, executor closes the task and
    /// re-emits valid work.done. Reviewer activates on the corrected event.
    #[test]
    fn test_retry_path_corrected_work_done_activates_reviewer() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);
        let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
        write_task(&tasks_path, "test-id-1", TaskStatus::Open);

        let config = build_test_config(workspace);
        let mut event_loop = make_event_loop(config);

        // Step 1: Reject open task → executor gets retry.
        let result = process_events(vec![work_done_event("test-id-1")], &mut event_loop);
        assert!(
            !result.contract_rejections.is_empty(),
            "First work.done should be rejected (open task)"
        );
        let executor_id = ralph_proto::HatId::new("executor");
        assert!(
            event_loop
                .bus
                .peek_pending(&executor_id)
                .map(|p| p
                    .iter()
                    .any(|e| e.target.as_ref().map(|t| t.as_str()) == Some("executor")))
                .unwrap_or(false),
            "Executor must receive retry event after rejection"
        );

        // Step 2: Simulate executor closing the task and re-emitting work.done.
        let mut store = TaskStore::load(&tasks_path).unwrap();
        if let Some(t) = store.get_mut("test-id-1") {
            t.status = TaskStatus::Closed;
        }
        store.save().unwrap();

        // Add git evidence (modify a tracked file) so the contract accepts
        // the corrected work.done. The retry guidance told executor to
        // complete the work; the simulation is that executor commits the
        // change before re-emitting.
        std::fs::write(
            workspace.join("README.md"),
            "# Test\nexecutor fix on retry\n",
        )
        .unwrap();

        // Drain the bus so we can observe the second round cleanly.
        event_loop.bus().take_pending(&executor_id);
        let _ = event_loop
            .bus()
            .take_pending(&ralph_proto::HatId::new("reviewer"));

        let result2 = process_events(vec![work_done_event("test-id-1")], &mut event_loop);
        assert!(
            result2.contract_rejections.is_empty(),
            "Second work.done (after closing task) should be accepted, got: {:?}",
            result2.contract_rejections
        );
        assert!(
            result2
                .accepted_events
                .iter()
                .any(|e| e.topic.as_str() == "work.done"),
            "Corrected work.done must be accepted"
        );

        // Reviewer activates.
        let reviewer_id = ralph_proto::HatId::new("reviewer");
        let reviewer_pending = event_loop
            .bus
            .peek_pending(&reviewer_id)
            .cloned()
            .unwrap_or_default();
        assert!(
            reviewer_pending
                .iter()
                .any(|e| e.topic.as_str() == "work.done"),
            "Reviewer must receive the corrected work.done"
        );
        let active_hat_id = event_loop.get_active_hat_id();
        assert_eq!(
            active_hat_id.as_str(),
            "reviewer",
            "After corrected work.done, reviewer must be the next active hat"
        );
    }

    /// Safety path: a forged `hat=ralph` work.done must NOT generate a
    /// targeted retry to ralph (which is a generic executor, not a real
    /// producer). The diagnostic still fires but with no retry target.
    #[test]
    fn test_forged_ralph_work_done_does_not_create_retry_to_ralph() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path();
        init_git_repo(workspace);
        let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
        write_task(&tasks_path, "test-id-1", TaskStatus::Open);

        let config = build_test_config(workspace);
        let mut event_loop = make_event_loop(config);

        // Build event with hat=ralph (forged attribution).
        let mut event = work_done_event("test-id-1");
        event.hat = Some("ralph".to_string());

        let result = process_events(vec![event], &mut event_loop);
        assert!(
            !result.contract_rejections.is_empty(),
            "Open task should still reject"
        );

        // No targeted retry should be published, because ralph is the generic
        // fallback and is not a safe retry target in multi-hat mode.
        let ralph_pending = event_loop
            .bus
            .peek_pending(&ralph_proto::HatId::new("ralph"))
            .cloned()
            .unwrap_or_default();
        let targeted_to_ralph = ralph_pending.iter().find(|e| {
            e.topic.as_str() != "human.guidance"
                && e.target.as_ref().map(|t| t.as_str()) == Some("ralph")
        });
        assert!(
            targeted_to_ralph.is_none(),
            "Forged hat=ralph must NOT generate a targeted retry to ralph. \
             Ralph is a generic executor, not a real work.done producer. \
             Pending: {:?}",
            ralph_pending
                .iter()
                .map(|e| (e.topic.as_str(), e.target.as_ref().map(|t| t.as_str())))
                .collect::<Vec<_>>()
        );
        // Executor (the real producer in this preset) must NOT get a retry
        // either, because the source attribution was forged to ralph.
        let executor_pending = event_loop
            .bus
            .peek_pending(&ralph_proto::HatId::new("executor"))
            .cloned()
            .unwrap_or_default();
        let targeted_to_executor = executor_pending.iter().find(|e| {
            e.topic.as_str() != "human.guidance"
                && e.target.as_ref().map(|t| t.as_str()) == Some("executor")
        });
        assert!(
            targeted_to_executor.is_none(),
            "Forged hat=ralph must NOT redirect retry to executor either. \
             The source attribution is untrusted; fall back to diagnostic only."
        );
    }

    // === Primary-loop current_loop_id() regression tests ===
    //
    // Background: `LoopContext::primary()` keeps `loop_id: None` (loop_context.rs:89),
    // and primary loops identify themselves via the `.ralph/current-loop-id` marker
    // that `LoopRunner::resolve_loop_id` writes (loop_runner.rs:183-203).
    // `EventLoop::current_loop_id()` is the helper that reads the marker; the
    // execution-contract call site at event_loop/mod.rs:3590 must use this helper
    // (not a hand-rolled `ctx.loop_id()` lookup) so primary-loop tasks are not
    // misclassified as belonging to a non-existent "default" loop.

    #[test]
    fn test_current_loop_id_reads_marker_for_primary_loop() {
        use crate::loop_context::LoopContext;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let ctx = LoopContext::primary(temp.path().to_path_buf());
        std::fs::create_dir_all(ctx.ralph_dir()).unwrap();
        std::fs::write(
            ctx.ralph_dir().join("current-loop-id"),
            "primary-20260604-091852\n",
        )
        .unwrap();

        let mut config = RalphConfig::default();
        config.core.workspace_root = temp.path().to_path_buf();
        let event_loop = EventLoop::with_context(config, ctx);

        assert_eq!(
            event_loop.current_loop_id(),
            Some("primary-20260604-091852".to_string()),
            "Primary loop must resolve its loop_id from the marker file"
        );
    }

    #[test]
    fn test_current_loop_id_returns_none_when_marker_missing_for_primary() {
        use crate::loop_context::LoopContext;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let ctx = LoopContext::primary(temp.path().to_path_buf());
        // Deliberately do not write the marker.

        let mut config = RalphConfig::default();
        config.core.workspace_root = temp.path().to_path_buf();
        let event_loop = EventLoop::with_context(config, ctx);

        assert_eq!(
            event_loop.current_loop_id(),
            None,
            "Primary loop with no marker should return None (caller decides fallback)"
        );
    }

    #[test]
    fn test_current_loop_id_for_contract_uses_marker_for_primary_loop() {
        use crate::loop_context::LoopContext;
        use tempfile::TempDir;

        // Regression for the `event_loop/mod.rs:3590` call site that previously
        // resolved `current_loop_id` from `LoopContext::loop_id()` (which is
        // always `None` for primary loops) and fell back to the literal
        // "default", causing every primary-loop task to be misclassified as
        // belonging to a non-existent "default" loop and rejected with
        // `TaskWrongLoop`.
        let temp = TempDir::new().unwrap();
        let ctx = LoopContext::primary(temp.path().to_path_buf());
        std::fs::create_dir_all(ctx.ralph_dir()).unwrap();
        std::fs::write(
            ctx.ralph_dir().join("current-loop-id"),
            "primary-20260604-091852\n",
        )
        .unwrap();

        let mut config = RalphConfig::default();
        config.core.workspace_root = temp.path().to_path_buf();
        let event_loop = EventLoop::with_context(config, ctx);

        assert_eq!(
            event_loop.current_loop_id_for_contract(),
            "primary-20260604-091852",
            "Contract check must see the marker value, not a hard-coded \"default\""
        );
    }

    #[test]
    fn test_current_loop_id_for_contract_falls_back_to_default_when_marker_missing() {
        use crate::loop_context::LoopContext;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let ctx = LoopContext::primary(temp.path().to_path_buf());
        // Deliberately do not write the marker.

        let mut config = RalphConfig::default();
        config.core.workspace_root = temp.path().to_path_buf();
        let event_loop = EventLoop::with_context(config, ctx);

        assert_eq!(
            event_loop.current_loop_id_for_contract(),
            "default",
            "When the marker is missing, the contract check should fall back to \"default\""
        );
    }
}
