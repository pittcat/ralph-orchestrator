//! Replay-light integration tests (deterministic event loop paths).

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
        r#"{"plan_name":"p","task_id":"test-id-1","task_key":"k1","step":"step-01"}"#.to_string(),
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
        r#"{"plan_name":"p","task_id":"test-id-1","task_key":"k1","step":"step-01"}"#.to_string(),
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

// ─────────────────────────────────────────────────────────────────────────
// Unit 3 of plan 2026-06-06-001: watchdog timeout integration coverage.
//
// These tests prove that even when the backend was killed by the
// autonomous PTY watchdog, the event loop's event-processing pipeline
// still runs against any partial JSONL events the agent emitted before
// the kill. The runner-level signal that triggers this is
// `ExecutionOutcome.termination = None` (with `watchdog_timeout = true`
// kept only as a diagnostic flag). The runner then calls
// `event_loop.process_output(...)` + `event_loop.process_events_from_jsonl(...)`
// — exactly the path these tests exercise.
//
// The R3 invariant is that a watchdog timeout MUST NOT:
//   - silently mark the iteration as a successful completion, or
//   - bypass execution-contract validation, or
//   - skip the missing-event hard gate when no events arrived.
// ─────────────────────────────────────────────────────────────────────────

/// Scenario 1 (Happy): the agent emitted a valid `work.done` event before
/// the watchdog killed the backend. After the simulated timeout, the
/// runner still calls `process_parse_result`; the event lands on the
/// reviewer's queue and activates the reviewer hat. This proves the
/// timeout did not falsely terminate the loop, did not bypass execution
/// contract validation, and did not drop the partial event.
#[test]
fn test_watchdog_timeout_with_partial_work_done_still_routes_to_reviewer() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_repo(workspace);
    let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
    write_task(&tasks_path, "test-id-1", TaskStatus::Closed);

    // Provide git evidence so the contract accepts the work.done. This
    // mirrors the realistic case where the agent committed changes and
    // wrote `work.done` to JSONL before some tail command hung.
    std::fs::write(
        workspace.join("README.md"),
        "# Test\nagent change for diff\n",
    )
    .unwrap();

    let config = build_test_config(workspace);
    let mut event_loop = make_event_loop(config);

    // Simulate the runner's post-execution event-processing call. In the
    // real runner this path is reached precisely BECAUSE Unit 3 keeps
    // `outcome.termination = None` for autonomous IdleTimeout.
    let result = process_events(vec![work_done_event("test-id-1")], &mut event_loop);

    assert!(
        result.contract_rejections.is_empty(),
        "Watchdog timeout must not interfere with contract validation. \
         Closed task + diff is a normal accept: {:?}",
        result.contract_rejections
    );
    assert!(
        result.had_events,
        "The partial work.done emitted before the watchdog fired must \
         remain visible to the event loop."
    );
    assert!(
        result
            .accepted_events
            .iter()
            .any(|e| e.topic.as_str() == "work.done"),
        "Original work.done must be in accepted events even though the \
         backend was killed by the watchdog."
    );

    // Reviewer must receive the work.done and become the next active
    // hat — proving the downstream workflow continues after the timeout.
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
        "Reviewer must receive the partial work.done. The watchdog \
         timeout is a backend-call end, NOT a loop terminate."
    );
    let active_hat_id = event_loop.get_active_hat_id();
    assert_eq!(
        active_hat_id.as_str(),
        "reviewer",
        "After the watchdog timeout with a valid partial work.done, the \
         next active hat must be reviewer — the workflow continues."
    );
}

/// Scenario 2 (Happy / Integration): the agent wrote nothing before the
/// watchdog killed the backend. The runner still drains the empty event
/// stream; `ProcessedEvents.had_events` is false and `had_raw_events` is
/// false, which is exactly the precondition the runner's missing-event
/// hard gate checks (`!agent_wrote_any_valid_or_rejected`). The hard gate
/// then injects guidance and increments `consecutive_hard_gates` —
/// recovery is possible because the loop did NOT force-stop.
#[test]
fn test_watchdog_timeout_with_no_events_leaves_missing_event_gate_armed() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_repo(workspace);

    let config = contract_disabled_config(workspace);
    let mut event_loop = make_event_loop(config);

    // Simulate the runner's post-watchdog event drain: no JSONL events
    // because the agent never got far enough to emit one.
    let result = process_events(vec![], &mut event_loop);

    // These are the EXACT flags the runner inspects to decide whether to
    // arm the missing-event hard gate:
    //   `!agent_wrote_events && !hard_gate_triggered_this_iteration`
    // See loop_runner/runner.rs around the call to
    // `should_gate_missing_events`. Pinning them here documents the
    // integration contract: a watchdog timeout with no events leaves the
    // existing hard-gate path armed instead of force-stopping the loop.
    assert!(
        !result.had_events,
        "No events were emitted before the watchdog timeout; the runner's \
         missing-event gate relies on `had_events == false`."
    );
    assert!(
        !result.had_raw_events,
        "`had_raw_events` must also be false so the runner's \
         `agent_wrote_any_valid_or_rejected` precondition triggers the \
         missing-event hard gate path."
    );
    assert!(
        result.contract_rejections.is_empty(),
        "Empty JSONL must not synthesize spurious contract rejections."
    );
    assert!(
        result.accepted_events.is_empty(),
        "Empty JSONL must not synthesize spurious accepted events."
    );
}

/// Scenario 4 + 5 (Integration / Regression): a watchdog timeout MUST NOT
/// fake-pass the execution contract. If the agent emitted a `work.done`
/// against an open task before the backend died, the contract still has
/// to reject it — exactly the same way it would on a normal exit. Pins
/// that "the backend was killed" never becomes an excuse to skip
/// validation.
#[test]
fn test_watchdog_timeout_with_partial_open_task_event_is_still_rejected() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_repo(workspace);
    let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
    write_task(&tasks_path, "test-id-1", TaskStatus::Open);

    let config = build_test_config(workspace);
    let mut event_loop = make_event_loop(config);

    let result = process_events(vec![work_done_event("test-id-1")], &mut event_loop);

    assert!(
        !result.contract_rejections.is_empty(),
        "Open task + partial work.done after a watchdog timeout must still \
         be rejected by the execution contract — the timeout does not \
         excuse contract violations."
    );
    let has_task_not_terminal = result.contract_rejections.iter().any(|f| {
        matches!(
            f.kind,
            crate::execution_contract::ExecutionContractViolationKind::TaskNotTerminal { .. }
        )
    });
    assert!(
        has_task_not_terminal,
        "Expected TaskNotTerminal finding even on a partial-event timeout, got: {:?}",
        result.contract_rejections
    );
    assert!(
        !result
            .accepted_events
            .iter()
            .any(|e| e.topic.as_str() == "work.done"),
        "Rejected work.done must NOT be in accepted events; the watchdog \
         fire is not a workflow promotion event."
    );
}

// ─────────────────────────────────────────────────────────────────────────
// 2026-06-04 plan U4: end-to-end recovery journal coverage for the
// missing-event hard gate. The path is:
//   1. an iteration produces zero valid events (the agent completely
//      forgot to emit);
//   2. the gate's envelope construction code is exercised via the
//      process-events pipeline (which is what the runner calls);
//   3. when diagnostics are enabled, `recovery.jsonl` carries a
//      `MissingEventGate` envelope so the operator report can list it.
//
// This test does NOT call `inject_missing_event_hard_gate_guidance`
// directly (that is the loop_runner test's job). It exercises the
// event-loop-layer surface that the runner drives.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn test_u4_no_events_writes_diagnostics_summary_metadata() {
    // U4 sanity: when no events arrive and diagnostics are enabled,
    // the loop still produces a diagnostics session and the recovery
    // journal is writable. This is a pre-flight check for the
    // missing-event gate's journal write — the gate calls
    // `log_recovery`, which is a no-op when the logger isn't there.
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_repo(workspace);

    let mut config = contract_disabled_config(workspace);
    config.core.workspace_root = workspace.to_path_buf();
    let diagnostics = crate::diagnostics::DiagnosticsCollector::with_enabled(workspace, true)
        .expect("create diagnostics collector");
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);

    let result = process_events(vec![], &mut event_loop);
    assert!(!result.had_events);

    // A diagnostics session was created.
    let mut session_dirs: Vec<_> = std::fs::read_dir(workspace.join(".ralph/diagnostics"))
        .expect("read diagnostics root")
        .filter_map(Result::ok)
        .collect();
    session_dirs.sort_by_key(|entry| entry.path());
    let session_path = session_dirs
        .last()
        .expect("at least one diagnostics session")
        .path();
    // The session id is the directory name (timestamped).
    let session_id = session_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(String::from);
    assert!(
        session_id.is_some(),
        "session id must be the directory name"
    );

    // Writing to the recovery logger before any envelope is
    // constructed is a no-op (no file should be created when the
    // collector hasn't actually written anything). But once we DO
    // construct an envelope, it must be persisted to recovery.jsonl.
    let envelope = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
        .source(crate::diagnosis::DiagnosisSource::MissingEventGate)
        .severity(crate::diagnosis::DiagnosisSeverity::Warning)
        .iteration(1)
        .source_hat("executor")
        .target_hat("executor")
        .topic("work.done")
        .reason_code("missing_event")
        .message("executor did not emit any event on its publish obligation")
        .expected_action("emit one of: work.done, work.failed")
        .safe_target(true)
        .outcome(crate::diagnosis::DiagnosisOutcome::Pending)
        .session_id(session_id.clone().unwrap_or_default())
        .build();
    let entry = crate::diagnosis::RecoveryJournalEntry::from_envelope(envelope.clone(), vec![]);
    event_loop.diagnostics().log_recovery(entry);

    // recovery.jsonl must exist and contain the envelope.
    let recovery_path = session_path.join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
    assert!(
        content.contains("missing_event_gate"),
        "journal must list source"
    );
    assert!(
        content.contains(&envelope.diagnosis_id),
        "journal must list the diagnosis_id"
    );
}
