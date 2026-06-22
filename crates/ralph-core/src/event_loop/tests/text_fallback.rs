//! Tests for text_fallback.

use super::*;

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
