//! Tests for loop_context.

use super::common::*;
use super::*;

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
fn test_has_pending_human_events_detects_guidance() {
    // 2026-06-28-005: human.guidance topic was deleted; the
    // has_pending_human_events stub always returns false now
    // (the dedicated human_pending queue was removed together
    // with the topic). Pin that here so the stub contract is
    // visible from tests.
    let event_loop = EventLoop::new(RalphConfig::default());
    assert!(!event_loop.has_pending_human_events());
}

#[test]
fn test_has_pending_human_events_ignores_non_human() {
    let event_loop = EventLoop::new(RalphConfig::default());
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
fn test_missing_terminal_emit_recovery_targets_same_hat_with_typed_resume() {
    let yaml = r#"
hats:
  goal-alignment:
    name: "Goal alignment"
    triggers: ["review.dimensions.done"]
    publishes: ["review.goalalign.done", "review.goalalign.failed"]
    terminal_events: ["review.goalalign.done", "review.goalalign.failed"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let hat_id = HatId::new("goal-alignment");
    event_loop.state.last_activation_events = vec![ralph_proto::Event::new(
        "review.dimensions.done",
        "{\"iteration\":2}",
    )];

    assert!(event_loop.inject_missing_terminal_emit_recovery(
        &hat_id,
        &[
            "review.goalalign.done".to_string(),
            "review.goalalign.failed".to_string(),
        ],
    ));

    let pending = event_loop
        .bus
        .peek_pending(&hat_id)
        .expect("targeted resume");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].topic.as_str(), "task.resume");
    assert_eq!(pending[0].target.as_ref(), Some(&hat_id));
    assert!(pending[0].payload.contains("missing_event_gate"));
    assert!(pending[0].payload.contains("review.goalalign.done"));
    assert!(pending[0].payload.contains("review.dimensions.done"));
    assert!(pending[0].payload.contains("original_trigger_payload"));
}

#[test]
fn test_missing_terminal_emit_recovery_blocks_after_bounded_retries() {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
    terminal_events: ["work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let hat_id = HatId::new("executor");
    event_loop.state.last_activation_events = vec![ralph_proto::Event::new("work.ready", "{}")];

    for _ in 0..=crate::event_loop::loop_state::U2_REJECTION_RETRY_LIMIT {
        let _ =
            event_loop.inject_missing_terminal_emit_recovery(&hat_id, &["work.done".to_string()]);
    }

    let pending = event_loop.bus.peek_pending(&hat_id).expect("resume queue");
    assert_eq!(
        pending.len(),
        crate::event_loop::loop_state::U2_REJECTION_RETRY_LIMIT as usize
    );
    assert!(
        pending
            .iter()
            .all(|event| event.topic.as_str() == "task.resume")
    );
    assert!(event_loop.terminal_event_emitted);
    assert!(
        event_loop
            .bus
            .peek_pending(&hat_id)
            .is_none_or(|events| events.len()
                == crate::event_loop::loop_state::U2_REJECTION_RETRY_LIMIT as usize)
    );
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
    // Plan 2026-08-10-001 U1 R1: the Ralph untargeted
    // fallback (target-less `Event::new("task.resume", ...)`)
    // was dropped in favour of fail-closed `Block
    // { MissingTarget }` semantics for the case when
    // `last_hat` is `None` or `Some("ralph")`. Surfaces via
    // `loop.stalled` only when `progress_steward.enabled`
    // is set; otherwise nothing is published and the
    // up-stream plan.blocked ladder takes over.
    let mut event_loop = EventLoop::new(RalphConfig::default());
    event_loop.state.last_hat = None;
    // Disable progress_steward so the helper does not emit
    // `loop.stalled`; mirrors the ce-executor pipeline.
    event_loop.config.event_loop.progress_steward.enabled = false;

    assert!(event_loop.inject_fallback_event());

    let ralph_id = HatId::new("ralph");
    let pending = event_loop.bus.peek_pending(&ralph_id);
    assert!(
        pending.is_none_or(|events| events.is_empty()),
        "no task.resume must be published to ralph when last_hat is None and steward is disabled"
    );
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
