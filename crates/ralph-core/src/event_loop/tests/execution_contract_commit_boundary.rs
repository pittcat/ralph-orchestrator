//! 2026-07-07-002 plan Unit 2: execution contract commit boundary wiring.
//!
//! Characterization tests proving contract-rejected `work.done` never enters
//! accepted events or pre-commit state (`work_done_seen_tasks`).

use crate::config::RalphConfig;
use crate::event_loop::EventLoop;
use crate::event_reader::ParseResult;
use crate::execution_contract::ExecutionContractViolationKind;
use crate::task::{Task, TaskStatus};
use crate::task_store::TaskStore;
use ralph_proto::HatId;
use std::process::Command;
use tempfile::TempDir;

fn init_git_repo(dir: &std::path::Path) {
    let run = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| panic!("git {:?} failed: {e}", args));
    };
    run(&["init", "--initial-branch=main"]);
    run(&["config", "user.email", "test@test.local"]);
    run(&["config", "user.name", "Test User"]);
    std::fs::write(dir.join(".gitignore"), ".ralph/\n").unwrap();
    std::fs::write(dir.join("README.md"), "# Test\n").unwrap();
    run(&["add", ".gitignore", "README.md"]);
    run(&["commit", "-m", "Initial commit"]);
}

fn build_test_config(workspace_root: &std::path::Path) -> RalphConfig {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*", "work.ready"]
    publishes: ["work.done", "work.failed"]
  validator:
    name: "Validator"
    triggers: ["work.done"]
    publishes: ["test.passed", "test.failed"]
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

fn write_open_task(tasks_path: &std::path::Path, task_id: &str) {
    let parent = tasks_path.parent().unwrap();
    std::fs::create_dir_all(parent).unwrap();
    let mut store = TaskStore::load(tasks_path).unwrap();
    let mut task = Task::new("Step 01".to_string(), 1);
    task.id = task_id.to_string();
    task.key = Some("k1".to_string());
    task.status = TaskStatus::Open;
    store.add(task);
    store.save().unwrap();
}

fn write_closed_task(tasks_path: &std::path::Path, task_id: &str) {
    let parent = tasks_path.parent().unwrap();
    std::fs::create_dir_all(parent).unwrap();
    let mut store = TaskStore::load(tasks_path).unwrap();
    let mut task = Task::new("Step 01".to_string(), 1);
    task.id = task_id.to_string();
    task.key = Some("k1".to_string());
    task.status = TaskStatus::Closed;
    store.add(task);
    store.save().unwrap();
}

fn work_done_event(task_id: &str) -> crate::event_reader::Event {
    crate::event_reader::Event {
        topic: "work.done".to_string(),
        payload: Some(format!(
            r#"{{"plan_name":"p","plan_path":"/p","task_id":"{task_id}","task_key":"k1","step":"step-01"}}"#
        )),
        ts: "2024-01-01T00:00:00Z".to_string(),
        wave_id: None,
        hat: Some("executor".to_string()),
        triggered: None,
        source: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    }
}

#[test]
fn test_open_task_work_done_not_in_accepted_events() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_repo(workspace);
    let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
    write_open_task(&tasks_path, "live-task-1");

    let config = build_test_config(workspace);
    let ctx = crate::loop_context::LoopContext::primary(workspace.to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx);

    let result = event_loop
        .process_parse_result(ParseResult {
            events: vec![work_done_event("live-task-1")],
            malformed: vec![],
        })
        .expect("process_parse_result");

    assert!(
        !result.contract_rejections.is_empty(),
        "expected TaskNotTerminal rejection"
    );
    assert!(
        result.contract_rejections.iter().any(|f| matches!(
            f.kind,
            ExecutionContractViolationKind::TaskNotTerminal { .. }
        )),
        "expected TaskNotTerminal, got {:?}",
        result.contract_rejections
    );
    assert!(
        !result
            .accepted_events
            .iter()
            .any(|e| e.topic.as_str() == "work.done"),
        "rejected work.done must not appear in accepted_events"
    );
}

#[test]
fn test_open_task_work_done_does_not_update_work_done_seen_tasks() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_repo(workspace);
    let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
    write_open_task(&tasks_path, "live-task-1");

    let config = build_test_config(workspace);
    let ctx = crate::loop_context::LoopContext::primary(workspace.to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx);

    event_loop
        .process_parse_result(ParseResult {
            events: vec![work_done_event("live-task-1")],
            malformed: vec![],
        })
        .expect("process_parse_result");

    assert!(
        event_loop.state.work_done_seen_tasks.is_empty(),
        "contract-rejected work.done must not populate work_done_seen_tasks"
    );
}

#[test]
fn test_closed_task_work_done_accepted_and_updates_work_done_seen_tasks() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_repo(workspace);
    let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
    write_closed_task(&tasks_path, "live-task-1");
    std::fs::write(workspace.join("README.md"), "# Test\nagent diff\n").unwrap();

    let config = build_test_config(workspace);
    let ctx = crate::loop_context::LoopContext::primary(workspace.to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx);

    let result = event_loop
        .process_parse_result(ParseResult {
            events: vec![work_done_event("live-task-1")],
            malformed: vec![],
        })
        .expect("process_parse_result");

    assert!(
        result.contract_rejections.is_empty(),
        "closed task with diff should pass contract"
    );
    assert!(
        result
            .accepted_events
            .iter()
            .any(|e| e.topic.as_str() == "work.done"),
        "accepted work.done must appear in accepted_events"
    );
    assert!(
        event_loop
            .state
            .work_done_seen_tasks
            .contains("p::step-01::live-task-1"),
        "accepted work.done must update work_done_seen_tasks"
    );
}

#[test]
fn test_contract_rejection_still_emits_recovery_envelope_with_safe_target() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_repo(workspace);
    let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
    write_open_task(&tasks_path, "live-task-1");

    let config = build_test_config(workspace);
    let ctx = crate::loop_context::LoopContext::primary(workspace.to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx);

    let result = event_loop
        .process_parse_result(ParseResult {
            events: vec![work_done_event("live-task-1")],
            malformed: vec![],
        })
        .expect("process_parse_result");

    assert!(!result.contract_rejections.is_empty());

    let executor_id = HatId::new("executor");
    let pending = event_loop
        .bus
        .peek_pending(&executor_id)
        .cloned()
        .unwrap_or_default();
    assert!(
        pending.iter().any(|e| e.topic.as_str() == "task.resume"),
        "rejected work.done must still produce targeted task.resume recovery"
    );
}
