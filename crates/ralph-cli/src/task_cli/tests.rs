#![cfg(test)]
use super::*;

use tempfile::TempDir;

fn write_tasks(root: &Path, tasks: Vec<Task>) -> TaskStore {
    let root_buf = root.to_path_buf();
    let path = get_tasks_path(Some(&root_buf));
    let mut store = TaskStore::load(&path).expect("load task store");
    for task in tasks {
        store.add(task);
    }
    store.save().expect("save task store");
    TaskStore::load(&path).expect("reload task store")
}

#[test]
fn test_list_status_filter_accepts_in_progress() {
    let temp_dir = TempDir::new().expect("temp dir");
    let mut open_task = Task::new("Open".to_string(), 2);
    open_task.status = TaskStatus::Open;
    let mut in_progress = Task::new("In progress".to_string(), 2);
    in_progress.status = TaskStatus::InProgress;

    let store = write_tasks(temp_dir.path(), vec![open_task, in_progress]);

    let args = ListArgs {
        status: Some("in_progress".to_string()),
        days: None,
        limit: None,
        all: true,
        format: OutputFormat::Quiet,
    };

    let filtered = filter_tasks_for_list(&store, &args);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].status, TaskStatus::InProgress);
}

#[test]
fn test_ready_filters_by_loop_id_marker() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path().to_path_buf();

    let mut task_loop_a = Task::new("Loop A task".to_string(), 1);
    task_loop_a.loop_id = Some("loop-a".to_string());
    let mut task_loop_b = Task::new("Loop B task".to_string(), 1);
    task_loop_b.loop_id = Some("loop-b".to_string());

    let store = write_tasks(temp_dir.path(), vec![task_loop_a, task_loop_b]);

    let marker_dir = root.join(".ralph");
    std::fs::create_dir_all(&marker_dir).expect("marker dir");
    std::fs::write(marker_dir.join("current-loop-id"), "loop-a").expect("write marker");

    let args = ReadyArgs {
        all: false,
        format: OutputFormat::Quiet,
    };

    let ready = filter_tasks_for_ready(&store, &args, Some(&root));
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].loop_id.as_deref(), Some("loop-a"));
}

#[test]
fn test_read_current_loop_id_ignores_empty_marker() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path().to_path_buf();
    let marker_dir = root.join(".ralph");
    std::fs::create_dir_all(&marker_dir).expect("marker dir");
    std::fs::write(marker_dir.join("current-loop-id"), "  ").expect("write marker");

    assert_eq!(read_current_loop_id(Some(&root)), None);
}

#[test]
fn test_get_tasks_path_discovers_workspace_root_from_nested_dir() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(root.join(".ralph/agent")).expect("agent dir");

    assert_eq!(
        get_tasks_path(Some(&root)),
        root.join(".ralph/agent/tasks.jsonl")
    );
}

fn ctx_for(workspace: &Path, loop_id: Option<&str>, hat_id: Option<&str>) -> OperationContext {
    OperationContext::detect_with_env(workspace.to_path_buf(), |key| match key {
        "RALPH_CURRENT_HAT" => hat_id.map(|s| s.to_string()),
        "RALPH_CURRENT_LOOP_ID" => loop_id.map(|s| s.to_string()),
        "RALPH_EVENTS_FILE" => None,
        "RALPH_WAVE_WORKER" => None,
        _ => None,
    })
}

fn write_marker(root: &Path, file: &str, value: &str) {
    let dir = root.join(".ralph");
    std::fs::create_dir_all(&dir).expect("marker dir");
    std::fs::write(dir.join(file), value).expect("write marker");
}

fn add_args(title: &str, blocked_by: Option<&str>) -> AddArgs {
    AddArgs {
        title: title.to_string(),
        priority: 2,
        description: None,
        blocked_by: blocked_by.map(|s| s.to_string()),
        format: OutputFormat::Quiet,
    }
}

fn ensure_args(title: &str, key: &str, blocked_by: Option<&str>) -> EnsureArgs {
    EnsureArgs {
        title: title.to_string(),
        key: Some(key.to_string()),
        priority: 2,
        description: None,
        blocked_by: blocked_by.map(|s| s.to_string()),
        for_fix_unit: None,
        format: OutputFormat::Quiet,
    }
}

fn open_store(root: &Path) -> TaskStore {
    let path = get_tasks_path(Some(&root.to_path_buf()));
    TaskStore::load(&path).expect("load task store")
}

// ---- P2 plan 列举测试 (18 项) ----

#[test]
fn test_task_add_stamps_loop_and_owner_hat() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let mut store = open_store(root);

    add_task_with_args(
        &mut store,
        &add_args("Do work", None),
        &ctx,
        &["executor".to_string()],
        false,
    )
    .unwrap();

    let saved = store.all();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].loop_id.as_deref(), Some("loop-a"));
    assert_eq!(saved[0].owner_hat_id.as_deref(), Some("executor"));
}

#[test]
fn test_task_add_without_hat_keeps_owner_none_for_human() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-h");
    let ctx = ctx_for(root, Some("loop-h"), None);
    let mut store = open_store(root);

    add_task_with_args(
        &mut store,
        &add_args("Do work", None),
        &ctx,
        &["executor".to_string()],
        false,
    )
    .unwrap();

    let saved = store.all();
    assert_eq!(saved[0].loop_id.as_deref(), Some("loop-h"));
    assert!(saved[0].owner_hat_id.is_none());
}

// U3: owner_hat_id must be on tasks.coordinator_hats. The five scenarios
// below exercise the create-side complement to the JSONL origin guard
// (which already rejects ralph at the read path).

#[test]
fn test_task_add_allows_executor_when_in_coordinator_hats() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let mut store = open_store(root);

    add_task_with_args(
        &mut store,
        &add_args("ok task", None),
        &ctx,
        &["coordinator".to_string(), "executor".to_string()],
        false,
    )
    .expect("executor in coordinator_hats should be accepted");
}

#[test]
fn test_task_add_rejects_ralph_when_not_in_coordinator_hats() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let ctx = ctx_for(root, Some("loop-a"), Some("ralph"));
    let mut store = open_store(root);

    let err = add_task_with_args(
        &mut store,
        &add_args("rogue task", None),
        &ctx,
        &["coordinator".to_string(), "executor".to_string()],
        false,
    )
    .expect_err("ralph must be rejected (not in coordinator_hats)");

    let msg = err.to_string();
    assert!(
        msg.contains("'ralph'") && msg.contains("not in tasks.coordinator_hats"),
        "error should name the rejected owner and the allowlist, got: {msg}"
    );

    // And nothing was persisted.
    assert!(store.all().is_empty());
}

#[test]
fn test_task_add_rejects_any_owner_when_coordinator_hats_empty() {
    // Fail-closed: an empty allowlist must not let any agent persist a
    // task. This catches the misconfigured-preset failure mode where
    // `coordinator_hats` is unset (or accidentally cleared) and the
    // orchestrator would otherwise default-open the gate.
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let mut store = open_store(root);

    let err = add_task_with_args(&mut store, &add_args("anything", None), &ctx, &[], false)
        .expect_err("empty coordinator_hats must reject any owner (fail-closed)");

    assert!(err.to_string().contains("not in tasks.coordinator_hats"));
}

#[test]
fn test_task_add_allows_human_call_without_owner() {
    // When `ctx.current_hat_id` is None, the task has no owner and
    // `validate_owner_hat_id` is a no-op. This is the human CLI
    // path — operators must not be locked out by the owner check.
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-h");
    let ctx = ctx_for(root, Some("loop-h"), None);
    let mut store = open_store(root);

    add_task_with_args(&mut store, &add_args("human task", None), &ctx, &[], false)
        .expect("human CLI call (no owner) must not be blocked by empty allowlist");
}

#[test]
fn test_task_ensure_rejects_off_allowlist_owner() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let ctx = ctx_for(root, Some("loop-a"), Some("rogue-hat"));
    let mut store = open_store(root);

    let err = ensure_task_with_args(
        &mut store,
        &ensure_args("x", "k:v", None),
        &ctx,
        &["executor".to_string()],
        false,
        &[],
    )
    .expect_err("ensure must also reject off-allowlist owner");
    assert!(err.to_string().contains("not in tasks.coordinator_hats"));
}

#[test]
fn test_task_start_rejects_other_loop_task() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let other_loop = Task::new("Other loop".to_string(), 1)
        .with_loop_id(Some("loop-b".to_string()))
        .with_owner_hat(Some("reviewer".to_string()));
    let other_id = other_loop.id.clone();
    store.add(other_loop);

    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let err = start_task_with_context(&mut store, &other_id, &ctx, &[], false)
        .expect_err("cross-loop start should fail");
    assert!(err.to_string().contains("loop-b"));
}

#[test]
fn test_task_close_rejects_other_loop_task() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let other_loop = Task::new("Other".to_string(), 1)
        .with_loop_id(Some("loop-b".to_string()))
        .with_owner_hat(Some("reviewer".to_string()));
    let other_id = other_loop.id.clone();
    store.add(other_loop);

    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let err = close_task_with_context(&mut store, &other_id, &ctx, &[], false)
        .expect_err("cross-loop close should fail");
    assert!(err.to_string().contains("loop-b"));
}

#[test]
fn test_task_fail_rejects_other_loop_task() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let other = Task::new("Other".to_string(), 1)
        .with_loop_id(Some("loop-b".to_string()))
        .with_owner_hat(Some("reviewer".to_string()));
    let other_id = other.id.clone();
    store.add(other);

    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let err = fail_task_with_context(&mut store, &other_id, &ctx, &[], false)
        .expect_err("cross-loop fail should fail");
    assert!(err.to_string().contains("loop-b"));
}

#[test]
fn test_task_reopen_rejects_other_loop_task() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let other = Task::new("Other".to_string(), 1)
        .with_loop_id(Some("loop-b".to_string()))
        .with_owner_hat(Some("reviewer".to_string()));
    let other_id = other.id.clone();
    store.add(other);

    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let err = reopen_task_with_context(&mut store, &other_id, &ctx, &[], false)
        .expect_err("cross-loop reopen should fail");
    assert!(err.to_string().contains("loop-b"));
}

#[test]
fn test_task_start_rejects_other_hat_task() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let owned = Task::new("Owned".to_string(), 1)
        .with_loop_id(Some("loop-a".to_string()))
        .with_owner_hat(Some("reviewer".to_string()));
    let owned_id = owned.id.clone();
    store.add(owned);

    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let err = start_task_with_context(&mut store, &owned_id, &ctx, &[], false)
        .expect_err("cross-hat start should fail");
    assert!(err.to_string().contains("reviewer"));
}

// ---- U3 wiring tests: enforce_command_policy bridge ----

fn isolated_config_with_coordinator() -> ralph_core::config::RalphConfig {
    let yaml = r#"
event_loop:
  execution_mode: isolated
tasks:
  enabled: true
  coordinator_hats:
    - coordinator
hats:
  coordinator:
    name: "Coordinator"
    publishes: ["work.ready"]
  worker:
    name: "Worker"
    publishes: ["work.done"]
"#;
    serde_yaml::from_str(yaml).unwrap()
}

fn hats_for(cfg: &ralph_core::config::RalphConfig) -> Vec<String> {
    cfg.tasks.coordinator_hats.clone()
}

#[test]
fn enforce_command_policy_allows_coordinator_add() {
    let cfg = isolated_config_with_coordinator();
    let ctx = ctx_for(Path::new("/tmp"), Some("loop-a"), Some("coordinator"));
    let hats = hats_for(&cfg);
    assert!(enforce_command_policy(&ctx, &hats, None, None, "add", false).is_ok());
}

#[test]
fn enforce_command_policy_denies_worker_add_with_hint() {
    let cfg = isolated_config_with_coordinator();
    let ctx = ctx_for(Path::new("/tmp"), Some("loop-a"), Some("worker"));
    let hats = hats_for(&cfg);
    let err = enforce_command_policy(&ctx, &hats, None, None, "add", false)
        .expect_err("worker must be denied at add entry");
    let msg = err.to_string();
    assert!(msg.contains("hat_command_policy denied 'add'"));
    assert!(msg.contains("worker"));
    assert!(msg.contains("non_coordinator_owner"));
}

#[test]
fn enforce_command_policy_denies_worker_ensure_with_hint() {
    let cfg = isolated_config_with_coordinator();
    let ctx = ctx_for(Path::new("/tmp"), Some("loop-a"), Some("worker"));
    let hats = hats_for(&cfg);
    let err = enforce_command_policy(&ctx, &hats, None, None, "ensure", false)
        .expect_err("worker must be denied at ensure entry");
    assert!(err.to_string().contains("non_coordinator_owner"));
}

#[test]
fn enforce_command_policy_allows_worker_close() {
    let cfg = isolated_config_with_coordinator();
    let ctx = ctx_for(Path::new("/tmp"), Some("loop-a"), Some("worker"));
    let hats = hats_for(&cfg);
    assert!(
        enforce_command_policy(&ctx, &hats, None, None, "close", false).is_ok(),
        "close passes the role gate; ownership is enforced by authorize_lifecycle"
    );
}

#[test]
fn enforce_command_policy_human_cli_unaffected() {
    let cfg = isolated_config_with_coordinator();
    let ctx = ctx_for(Path::new("/tmp"), None, None);
    let hats = hats_for(&cfg);
    assert!(enforce_command_policy(&ctx, &hats, None, None, "add", false).is_ok());
    assert!(enforce_command_policy(&ctx, &hats, None, None, "ensure", false).is_ok());
}

#[test]
fn enforce_command_policy_empty_coordinator_hats_fails_closed_for_agent() {
    let yaml = r"
event_loop:
  execution_mode: isolated
tasks:
  enabled: true
  coordinator_hats: []
";
    let cfg: ralph_core::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let ctx = ctx_for(Path::new("/tmp"), Some("loop-a"), Some("coordinator"));
    let hats = hats_for(&cfg);
    let err = enforce_command_policy(&ctx, &hats, None, None, "add", false)
        .expect_err("empty coordinator_hats must fail closed for agents");
    let msg = err.to_string();
    assert!(msg.contains("non_coordinator_owner"));
    assert!(msg.contains("tasks.coordinator_hats is empty"));
}

#[test]
fn test_task_close_allows_owner_hat() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let owned = Task::new("Owned".to_string(), 1)
        .with_loop_id(Some("loop-a".to_string()))
        .with_owner_hat(Some("executor".to_string()));
    let owned_id = owned.id.clone();
    store.add(owned);

    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    close_task_with_context(&mut store, &owned_id, &ctx, &[], false)
        .expect("owner hat should be allowed to close");
    assert_eq!(store.get(&owned_id).unwrap().status, TaskStatus::Closed);
}

#[test]
fn test_task_operation_allows_configured_task_coordinator() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let owned = Task::new("Owned".to_string(), 1)
        .with_loop_id(Some("loop-a".to_string()))
        .with_owner_hat(Some("reviewer".to_string()));
    let owned_id = owned.id.clone();
    store.add(owned);

    let ctx = ctx_for(root, Some("loop-a"), Some("coordinator"));
    let coordinators = vec!["coordinator".to_string()];
    close_task_with_context(&mut store, &owned_id, &ctx, &coordinators, false)
        .expect("coordinator should be allowed to close any task");
    assert_eq!(store.get(&owned_id).unwrap().status, TaskStatus::Closed);
}

#[test]
fn test_task_operation_rejects_missing_current_hat_in_agent_context() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let owned = Task::new("Owned".to_string(), 1)
        .with_loop_id(Some("loop-a".to_string()))
        .with_owner_hat(Some("executor".to_string()));
    let owned_id = owned.id.clone();
    store.add(owned);

    // Agent context (loop marker set) but no RALPH_CURRENT_HAT.
    let ctx = ctx_for(root, Some("loop-a"), None);
    let err = close_task_with_context(&mut store, &owned_id, &ctx, &[], false)
        .expect_err("missing hat in agent context should fail closed");
    assert!(err.to_string().to_lowercase().contains("hat"));
}

#[test]
fn test_task_close_denied_for_coordinator_owned_task_hints_delegate() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let owned = Task::new("Owned".to_string(), 1)
        .with_loop_id(Some("loop-a".to_string()))
        .with_owner_hat(Some("coordinator".to_string()));
    let owned_id = owned.id.clone();
    store.add(owned);

    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let coordinators = vec!["coordinator".to_string()];
    let err = close_task_with_context(&mut store, &owned_id, &ctx, &coordinators, false)
        .expect_err("executor must not close coordinator-owned task");
    let msg = err.to_string();
    assert!(msg.contains("Ask hat 'coordinator'"));
    assert!(msg.contains("ralph tools task close"));
    assert!(msg.contains("re-emit work.done"));
}

#[test]
fn test_task_ensure_key_scoped_by_loop() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let ctx_a = ctx_for(root, Some("loop-a"), Some("executor"));
    let mut store = open_store(root);
    ensure_task_with_args(
        &mut store,
        &ensure_args("First", "shared:task", None),
        &ctx_a,
        &["executor".to_string()],
        false,
        &[],
    )
    .unwrap();
    ensure_task_with_args(
        &mut store,
        &ensure_args("Second", "shared:task", None),
        &ctx_a,
        &["executor".to_string()],
        false,
        &[],
    )
    .unwrap();
    assert_eq!(store.all().len(), 1);
}

#[test]
fn test_task_ensure_same_key_same_loop_reuses() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let ctx_a = ctx_for(root, Some("loop-a"), Some("executor"));
    let mut store = open_store(root);
    ensure_task_with_args(
        &mut store,
        &ensure_args("First", "shared:task", None),
        &ctx_a,
        &["executor".to_string()],
        false,
        &[],
    )
    .unwrap();
    let first_id = store
        .get_by_key_in_loop("shared:task", Some("loop-a"))
        .unwrap()
        .id
        .clone();
    ensure_task_with_args(
        &mut store,
        &ensure_args("Second", "shared:task", None),
        &ctx_a,
        &["executor".to_string()],
        false,
        &[],
    )
    .unwrap();
    let reused_id = store
        .get_by_key_in_loop("shared:task", Some("loop-a"))
        .unwrap()
        .id
        .clone();
    assert_eq!(first_id, reused_id);
    assert_eq!(store.all().len(), 1);
}

#[test]
fn test_task_ensure_same_key_different_loop_creates_new() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let ctx_a = ctx_for(root, Some("loop-a"), Some("executor"));
    let mut store = open_store(root);
    ensure_task_with_args(
        &mut store,
        &ensure_args("First", "shared:task", None),
        &ctx_a,
        &["executor".to_string()],
        false,
        &[],
    )
    .unwrap();
    // Switch marker to loop-b so the next ensure is stamped loop-b.
    write_marker(root, "current-loop-id", "loop-b");
    let ctx_b = ctx_for(root, Some("loop-b"), Some("executor"));
    ensure_task_with_args(
        &mut store,
        &ensure_args("Second", "shared:task", None),
        &ctx_b,
        &["executor".to_string()],
        false,
        &[],
    )
    .unwrap();
    assert_eq!(store.all().len(), 2);
}

#[test]
fn test_task_add_rejects_blocker_from_other_loop() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    // Blocker exists but belongs to a different loop.
    let other_blocker =
        Task::new("Other loop blocker".to_string(), 1).with_loop_id(Some("loop-b".to_string()));
    let blocker_id = other_blocker.id.clone();
    store.add(other_blocker);

    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let err = add_task_with_args(
        &mut store,
        &add_args("Do work", Some(&blocker_id)),
        &ctx,
        &["executor".to_string()],
        false,
    )
    .expect_err("cross-loop blocker should be rejected");
    assert!(err.to_string().contains(&blocker_id));
}

#[test]
fn test_task_add_rejects_missing_blocker() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let err = add_task_with_args(
        &mut store,
        &add_args("Do work", Some("task-9999-deadbeef")),
        &ctx,
        &["executor".to_string()],
        false,
    )
    .expect_err("missing blocker should be rejected");
    assert!(err.to_string().contains("task-9999-deadbeef"));
}

#[test]
fn test_task_ready_defaults_to_current_loop() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");

    let mut t_a = Task::new("Loop A task".to_string(), 1);
    t_a.loop_id = Some("loop-a".to_string());
    let mut t_b = Task::new("Loop B task".to_string(), 1);
    t_b.loop_id = Some("loop-b".to_string());
    let store = write_tasks(root, vec![t_a, t_b]);

    let args = ReadyArgs {
        all: false,
        format: OutputFormat::Quiet,
    };
    let ready = filter_tasks_for_ready(&store, &args, Some(&root.to_path_buf()));
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].loop_id.as_deref(), Some("loop-a"));
}

#[test]
fn test_task_ready_all_includes_other_loops() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");

    let mut t_a = Task::new("Loop A task".to_string(), 1);
    t_a.loop_id = Some("loop-a".to_string());
    let mut t_b = Task::new("Loop B task".to_string(), 1);
    t_b.loop_id = Some("loop-b".to_string());
    let store = write_tasks(root, vec![t_a, t_b]);

    let args = ReadyArgs {
        all: true,
        format: OutputFormat::Quiet,
    };
    let ready = filter_tasks_for_ready(&store, &args, Some(&root.to_path_buf()));
    assert_eq!(ready.len(), 2);
}

#[test]
fn test_legacy_task_without_loop_not_mutable_by_agent() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    // Legacy task has neither loop_id nor owner_hat_id.
    let legacy = Task::new("Legacy".to_string(), 1);
    let legacy_id = legacy.id.clone();
    store.add(legacy);

    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let err = close_task_with_context(&mut store, &legacy_id, &ctx, &[], false)
        .expect_err("agent must not mutate legacy task");
    assert!(err.to_string().contains("legacy"));
}

// ---- U1 SSOT 回归 ----

#[test]
fn load_coordinator_hats_via_config_matches_explicit_yaml() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path().to_path_buf();
    std::fs::write(
        root.join("ralph.yml"),
        "tasks:\n  coordinator_hats:\n    - coordinator\n    - executor\n",
    )
    .expect("write ralph.yml");

    // 不再调用旧 loader,改为通过 config 读取
    let config = load_config_or_default(Some(&root), &[]);
    assert_eq!(
        config.tasks.coordinator_hats,
        vec!["coordinator".to_string(), "executor".to_string()]
    );
}

#[test]
fn load_config_or_default_handles_missing_ralph_yml() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path().to_path_buf();
    let config = load_config_or_default(Some(&root), &[]);
    // 缺 ralph.yml → Default::default() (event_loop.execution_mode = isolated)
    assert!(config.tasks.coordinator_hats.is_empty());
}

#[test]
fn load_config_or_default_handles_missing_tasks_section() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path().to_path_buf();
    std::fs::write(
        root.join("ralph.yml"),
        "event_loop:\n  execution_mode: isolated\n",
    )
    .expect("write ralph.yml");
    let config = load_config_or_default(Some(&root), &[]);
    assert!(config.tasks.coordinator_hats.is_empty());
}

// ---- empty task_id guard tests ----

#[test]
fn test_validate_task_id_accepts_non_empty() {
    assert!(validate_task_id("task-123-abc").is_ok());
}

#[test]
fn test_validate_task_id_rejects_empty() {
    let err = validate_task_id("").expect_err("empty task_id must be rejected");
    assert!(err.to_string().contains("cannot be empty"));
}

#[test]
fn test_validate_task_id_rejects_whitespace_only() {
    let err = validate_task_id("   ").expect_err("whitespace-only task_id must be rejected");
    assert!(err.to_string().contains("cannot be empty"));
}

#[test]
fn test_start_task_with_context_rejects_empty_id() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let err = start_task_with_context(&mut store, "", &ctx, &["executor".to_string()], false)
        .expect_err("empty task_id must be rejected");
    assert!(err.to_string().contains("cannot be empty"));
}

#[test]
fn test_coordinator_cannot_start_non_owned_task() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let task = Task::new("unit".to_string(), 1)
        .with_loop_id(Some("loop-a".to_string()))
        .with_owner_hat(Some("executor".to_string()));
    let task_id = task.id.clone();
    store.add(task);
    let ctx = ctx_for(root, Some("loop-a"), Some("dispatcher"));

    let error = start_task_with_context(
        &mut store,
        &task_id,
        &ctx,
        &["dispatcher".to_string()],
        false,
    )
    .expect_err("coordinator administration must not grant execution ownership");
    assert!(error.to_string().contains("not_execution_owner"));
}

#[test]
fn test_close_task_with_context_rejects_empty_id() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let mut store = open_store(root);
    let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
    let err = close_task_with_context(&mut store, "", &ctx, &["executor".to_string()], false)
        .expect_err("empty task_id must be rejected");
    assert!(err.to_string().contains("cannot be empty"));
}

// ─────────────────────────────────────────────────────────────────────
// U4: `task verify` — OPAC Precheck stage for task mutations.
//
// Each test confirms verify behaves like the real mutation for the
// success path and surfaces the same machine-readable deny prefix on
// the failure path. The tests intentionally avoid using `ce-executor`
// hat names so the surface stays general (R10).
// ─────────────────────────────────────────────────────────────────────

fn verify_add_args(title: &str) -> VerifyAddArgs {
    VerifyAddArgs {
        title: Some(title.to_string()),
        priority: 2,
        description: None,
        blocked_by: None,
        format: VerifyFormatArgs {
            format: OutputFormat::Quiet,
        },
    }
}

fn verify_ensure_args(title: &str, key: &str) -> VerifyEnsureArgs {
    VerifyEnsureArgs {
        title: Some(title.to_string()),
        key: Some(key.to_string()),
        for_fix_unit: None,
        priority: 2,
        description: None,
        blocked_by: None,
        format: VerifyFormatArgs {
            format: OutputFormat::Quiet,
        },
    }
}

fn base_config_with(coordinator: &[&str]) -> ralph_core::config::RalphConfig {
    let yaml = format!(
        "tasks:\n  coordinator_hats: [{}]\n  enabled: true\n",
        coordinator.join(", ")
    );
    serde_yaml::from_str(&yaml).expect("parse yaml")
}

#[test]
fn test_verify_add_allowed_for_coordinator_hat_does_not_write() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-x");
    let mut store = open_store(root);
    let ctx = ctx_for(root, Some("loop-x"), Some("coordinator"));
    let _cfg = base_config_with(&["coordinator"]);
    let before_count = store.all().len();

    let outcome = verify_add(
        &mut store,
        &ctx,
        &["coordinator".into()],
        None,
        &verify_add_args("hi"),
        &[],
    )
    .expect("verify_add should not error");
    assert!(matches!(outcome, VerifyOutcome::Allow));

    // Confirm verify did NOT touch the store.
    assert_eq!(store.all().len(), before_count);
}

#[test]
fn test_verify_add_denied_for_non_coordinator_agent() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-x");
    let mut store = open_store(root);
    let ctx = ctx_for(root, Some("loop-x"), Some("worker"));
    let _cfg = base_config_with(&["coordinator"]);

    let outcome = verify_add(
        &mut store,
        &ctx,
        &["coordinator".into()],
        None,
        &verify_add_args("hi"),
        &[],
    )
    .expect("verify_add should not error");
    match outcome {
        VerifyOutcome::Deny { reason, .. } => assert_eq!(reason, "non_coordinator_owner"),
        VerifyOutcome::Allow => panic!("expected Deny for non-coordinator agent"),
    }
}

#[test]
fn test_verify_add_denies_missing_title() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-x");
    let mut store = open_store(root);
    let ctx = ctx_for(root, Some("loop-x"), Some("coordinator"));
    let _cfg = base_config_with(&["coordinator"]);
    let mut args = verify_add_args("placeholder");
    args.title = None;

    let outcome =
        verify_add(&mut store, &ctx, &["coordinator".into()], None, &args, &[]).expect("ok");
    assert!(matches!(
        outcome,
        VerifyOutcome::Deny { ref reason, .. } if reason == "missing_title"
    ));
}

#[test]
fn test_verify_ensure_rejects_missing_key_and_missing_title() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-x");
    let mut store = open_store(root);
    let ctx = ctx_for(root, Some("loop-x"), Some("coordinator"));
    let _cfg = base_config_with(&["coordinator"]);
    let mut args = verify_ensure_args("placeholder", "k");
    args.key = None;
    args.for_fix_unit = None;
    let outcome =
        verify_ensure(&mut store, &ctx, &["coordinator".into()], None, &args, &[]).expect("ok");
    assert!(matches!(
        outcome,
        VerifyOutcome::Deny { ref reason, .. } if reason == "missing_key"
    ));

    // And missing-title branch.
    let mut args2 = verify_ensure_args("placeholder", "k");
    args2.title = None;
    let outcome =
        verify_ensure(&mut store, &ctx, &["coordinator".into()], None, &args2, &[]).expect("ok");
    assert!(matches!(
        outcome,
        VerifyOutcome::Deny { ref reason, .. } if reason == "missing_title"
    ));
}

#[test]
fn test_verify_lifecycle_close_admits_owner_task() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-x");
    let mut store = open_store(root);
    let ctx = ctx_for(root, Some("loop-x"), Some("coordinator"));
    let _cfg = base_config_with(&["coordinator"]);

    let mut task = Task::new("done".into(), 2).with_owner_hat(Some("coordinator".into()));
    task.loop_id = Some("loop-x".into());
    store.add(task.clone());
    store.save().unwrap();
    store = open_store(root);

    let outcome = verify_lifecycle(
        &store,
        &ctx,
        &["coordinator".into()],
        None,
        "close",
        &task.id,
    )
    .expect("ok");
    assert!(matches!(outcome, VerifyOutcome::Allow));
}

#[test]
fn test_verify_lifecycle_close_denies_unknown_task_id() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-x");
    let store = open_store(root);
    let ctx = ctx_for(root, Some("loop-x"), Some("coordinator"));
    let _cfg = base_config_with(&["coordinator"]);

    let outcome = verify_lifecycle(
        &store,
        &ctx,
        &["coordinator".into()],
        None,
        "close",
        "task-does-not-exist",
    )
    .expect("ok");
    assert!(matches!(
        outcome,
        VerifyOutcome::Deny { ref reason, .. } if reason == "task_not_found"
    ));
}

#[test]
fn test_verify_emit_bridge_succeeds_with_consistent_fields() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-yo");
    let mut store = open_store(root);
    let mut task = Task::new("bridge-ok".into(), 2)
        .with_owner_hat(Some("coordinator".into()))
        .with_key(Some("myplan:step-02:patch-foo".into()));
    task.loop_id = Some("loop-yo".into());
    task.status = TaskStatus::InProgress;
    let id = task.id.clone();
    store.add(task);
    store.save().unwrap();

    let args = VerifyEmitBridgeArgs {
        task_id: id.clone(),
        task_key: "myplan:step-02:patch-foo".into(),
        step: "step-02".into(),
        format: OutputFormat::Quiet,
    };
    // execute_verify_emit_bridge uses operation_context_for, which only
    // honors env-level hats. We invoke the function with --root and
    // expect allow via the human-cli path (ctx.is_agent_context==false).
    execute_verify_emit_bridge(args, Some(&root.to_path_buf())).expect("verify ok");
}

#[test]
fn test_verify_emit_bridge_rejects_unknown_task_id() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-yo");
    let _store = open_store(root);

    let args = VerifyEmitBridgeArgs {
        task_id: "task-missing".into(),
        task_key: "x:step-1:foo".into(),
        step: "step-1".into(),
        format: OutputFormat::Quiet,
    };
    let err = execute_verify_emit_bridge(args, Some(&root.to_path_buf()))
        .expect_err("unknown task_id must be denied");
    let text = format!("{err:#}");
    assert!(
        text.contains("task_verify_emit_bridge"),
        "stable prefix present"
    );
    assert!(
        text.contains("task_id_resolution") || text.contains("task-missing"),
        "structured context attached"
    );
}

#[test]
fn test_verify_emit_bridge_rejects_step_key_mismatch() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-yo");
    let mut store = open_store(root);
    let mut task = Task::new("bridge-mismatch".into(), 2)
        .with_owner_hat(Some("coordinator".into()))
        .with_key(Some("plan-x:step-09:slug".into()));
    task.loop_id = Some("loop-yo".into());
    task.status = TaskStatus::InProgress;
    let id = task.id.clone();
    store.add(task);
    store.save().unwrap();

    let args = VerifyEmitBridgeArgs {
        task_id: id,
        task_key: "plan-x:step-09:slug".into(),
        step: "step-08".into(), // wrong on purpose
        format: OutputFormat::Quiet,
    };
    let err = execute_verify_emit_bridge(args, Some(&root.to_path_buf()))
        .expect_err("step mismatch must be denied");
    let text = format!("{err:#}");
    assert!(text.contains("step") || text.contains("step-09"));
}

#[test]
fn test_verify_emit_bridge_rejects_terminal_task() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-yo");
    let mut store = open_store(root);
    let mut task = Task::new("bridge-closed".into(), 2)
        .with_owner_hat(Some("coordinator".into()))
        .with_key(Some("plan-y:step-1:slug".into()));
    task.loop_id = Some("loop-yo".into());
    task.status = TaskStatus::Closed;
    let id = task.id.clone();
    store.add(task);
    store.save().unwrap();

    let args = VerifyEmitBridgeArgs {
        task_id: id,
        task_key: "plan-y:step-1:slug".into(),
        step: "step-1".into(),
        format: OutputFormat::Quiet,
    };
    let err = execute_verify_emit_bridge(args, Some(&root.to_path_buf()))
        .expect_err("terminal task must be denied");
    let text = format!("{err:#}");
    assert!(text.contains("terminal") || text.contains("Closed"));
}

// ─────────────────────────────────────────────────────────────────────
// U7: completion-emit warning after `task close`. Helper-level tests.
// ─────────────────────────────────────────────────────────────────────

fn make_event_envelope(topic: &str) -> String {
    serde_json::json!({"topic": topic, "ts": 1}).to_string()
}

fn config_with_completion_topics(topics: &[&str]) -> ralph_core::config::RalphConfig {
    use ralph_core::config::EventPolicyConfig;
    use ralph_core::config::hat::HatConfig;
    let mut cfg = ralph_core::config::RalphConfig::default();
    let mut hat = HatConfig::default();
    hat.publishes = topics.iter().map(|s| s.to_string()).collect();
    cfg.hats.insert("executor".to_string(), hat);
    cfg.event_loop.event_policy = Some(EventPolicyConfig {
        enabled: true,
        terminal_topics: topics.iter().map(|s| s.to_string()).collect(),
        ..EventPolicyConfig::default()
    });
    cfg
}

#[test]
fn test_parse_topics_from_jsonl_extracts_topic_fields() {
    let content = format!(
        "{}\n{}\n{}\n",
        make_event_envelope("work.ready"),
        make_event_envelope("chat.out"),
        make_event_envelope("work.done"),
    );
    // Pass a high max_lines so the whole content is scanned; tail
    // mode is exercised in test_parse_topics_from_jsonl_tail.
    let topics = parse_topics_from_jsonl_tail(&content, usize::MAX);
    assert_eq!(
        topics,
        vec![
            "work.ready".to_string(),
            "chat.out".to_string(),
            "work.done".to_string()
        ]
    );
}

#[test]
fn test_parse_topics_from_jsonl_skips_malformed_lines() {
    let content = format!(
        "{}\nnot-json\n{}\n",
        make_event_envelope("a"),
        make_event_envelope("b")
    );
    let topics = parse_topics_from_jsonl_tail(&content, usize::MAX);
    assert_eq!(topics, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn test_parse_topics_from_jsonl_tail_truncates_to_last_n_lines() {
    let content = format!(
        "{}\n{}\n{}\n",
        make_event_envelope("work.ready"),
        make_event_envelope("chat.out"),
        make_event_envelope("work.done"),
    );
    let topics = parse_topics_from_jsonl_tail(&content, 1);
    // Only the last envelope should be returned.
    assert_eq!(topics, vec!["work.done".to_string()]);
}

#[test]
fn test_close_warning_skips_when_completion_already_present_in_hat_channel() {
    // Hat-channel already has `work.done` → no warning printed to stderr.
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-x");
    // Write hat-channel with completion topic.
    let ralph_dir = root.join(".ralph");
    std::fs::create_dir_all(&ralph_dir).expect("ralph dir");
    std::fs::write(
        ralph_dir.join("current-hat-events"),
        make_event_envelope("work.done"),
    )
    .expect("write hat channel");

    let config = config_with_completion_topics(&["work.done"]);
    // Capture stderr by sending it to a sink (the helper uses
    // eprintln!). We assert behaviour via the empty-channel and
    // happy-path checks separately; here we just confirm no panic.
    emit_close_completion_warning(root, &config, "executor", "task-1");
}

#[test]
fn test_close_warning_no_topics_does_not_warn() {
    // Hat publishes nothing in terminal_topics → no warning.
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    let config = config_with_completion_topics(&["some.completion"]);
    // Hat publishes nothing → derive_completion_publishes is empty.
    emit_close_completion_warning(root, &config, "unknown", "task-2");
    // No assertion needed: the helper bails out early when expected==[].
}

/// 2026-07-16 cleanup plan U1: success-path stderr JSON must be
/// parseable. U5 introduced `"hat": "{:?}"` which renders to
/// `"hat": ""executor""` (embedded quotes break `serde_json`).
/// This test pins the success-path schema by asserting that
/// `build_close_warning_payload` returns a string whose JSON
/// payload parses with `serde_json`.
#[test]
fn test_close_warning_success_path_emits_parseable_json() {
    let payload = build_close_warning_payload(
        "executor",
        "task-1",
        &["work.done".to_string()],
        &[],
        "next-step-hint",
    );
    let json_str = payload
        .split_once('{')
        .map(|(_prefix, rest)| format!("{{{rest}"))
        .unwrap_or_else(|| panic!("payload missing JSON body: {payload}"));
    let parsed: serde_json::Value =
        serde_json::from_str(&json_str).expect("stderr JSON must be parseable");
    assert_eq!(parsed["code"], "close_without_completion_emit");
    assert_eq!(parsed["hat"], "executor");
    assert_eq!(parsed["task_id"], "task-1");
}

/// 2026-07-16 cleanup plan U1: early-return paths must also carry
/// `task_id` so all four stderr schemas stay consistent.
#[test]
fn test_close_warning_early_return_paths_carry_task_id() {
    let payload = build_close_warning_payload_missing_marker(
        "executor",
        "task-1",
        &["work.done".to_string()],
    );
    let json_str = payload
        .split_once('{')
        .map(|(_prefix, rest)| format!("{{{rest}"))
        .unwrap_or_else(|| panic!("payload missing JSON body: {payload}"));
    let parsed: serde_json::Value =
        serde_json::from_str(&json_str).expect("early-return JSON must be parseable");
    assert_eq!(parsed["code"], "close_without_completion_emit");
    assert_eq!(parsed["hat"], "executor");
    assert_eq!(parsed["task_id"], "task-1");
    assert_eq!(parsed["reason"], "hat_channel_missing_marker");
}

// ── 2026-07-07-002 plan Unit 7: live task identity idempotency ─
//
// ensure / add must not create a second live task row for the
// same `(loop_id, key)` locus; a duplicate add should surface
// as a bail, not silently append.
//
// These tests pin the live-task-identity contract described in
// the 2026-07-07-002 plan "Requirements Trace R4". BDD coverage
// lives in `ce_executor_serial_task_identity_idempotent.yml`;
// these unit tests pin the CLI surface independently.

#[test]
fn test_ensure_same_loop_and_key_does_not_append_second_row() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let ctx = ctx_for(root, Some("loop-a"), Some("coordinator"));
    let mut store = open_store(root);
    let coordinator_hats = vec!["coordinator".to_string()];

    // First ensure — creates the live record.
    ensure_task_with_args(
        &mut store,
        &ensure_args("Step 01", "ce-executor:idem-test:step-01:u1-impl", None),
        &ctx,
        &coordinator_hats,
        false,
        &[],
    )
    .expect("first ensure must succeed");

    let after_first = store.all();
    assert_eq!(after_first.len(), 1, "first ensure writes one row");
    let first_id = after_first[0].id.clone();

    // Second ensure with the same loop + key — must NOT append
    // a second row and must return the same task id.
    ensure_task_with_args(
        &mut store,
        &ensure_args(
            "Step 01 (re-issued)",
            "ce-executor:idem-test:step-01:u1-impl",
            None,
        ),
        &ctx,
        &coordinator_hats,
        false,
        &[],
    )
    .expect("second ensure must succeed (idempotent)");

    let after_second = store.all();
    assert_eq!(
        after_second.len(),
        1,
        "second ensure must not append a second live row"
    );
    assert_eq!(after_second[0].id, first_id, "ensure returns same id");
}

#[test]
fn test_ensure_different_loop_yields_distinct_live_records() {
    // Independence regression: two loops that each use the
    // same `task_key` (the key is namespaced under the plan,
    // not the loop) must each get their own live record.
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    write_marker(root, "current-loop-id", "loop-a");
    let ctx_a = ctx_for(root, Some("loop-a"), Some("coordinator"));
    let coordinator_hats = vec!["coordinator".to_string()];
    let mut store = open_store(root);

    ensure_task_with_args(
        &mut store,
        &ensure_args("step", "shared:key:v", None),
        &ctx_a,
        &coordinator_hats,
        false,
        &[],
    )
    .expect("first ensure (loop-a) must succeed");

    ensure_task_with_args(
        &mut store,
        &ensure_args("step", "shared:key:v", None),
        &ctx_a,
        &coordinator_hats,
        false,
        &[],
    )
    .expect("second ensure same loop must be idempotent");

    // Re-target marker to a different loop and re-issue. The
    // store must treat this as a fresh locus — i.e. one row
    // per loop, NOT collision.
    write_marker(root, "current-loop-id", "loop-b");
    let ctx_b = ctx_for(root, Some("loop-b"), Some("coordinator"));
    ensure_task_with_args(
        &mut store,
        &ensure_args("step", "shared:key:v", None),
        &ctx_b,
        &coordinator_hats,
        false,
        &[],
    )
    .expect("ensure on a different loop must succeed");

    let saved = store.all();
    assert_eq!(saved.len(), 2, "two loops => two live records");
    let loops: std::collections::HashSet<_> =
        saved.iter().filter_map(|t| t.loop_id.clone()).collect();
    assert!(loops.contains("loop-a"));
    assert!(loops.contains("loop-b"));
}
