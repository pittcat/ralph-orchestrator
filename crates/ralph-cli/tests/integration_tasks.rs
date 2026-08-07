//! Integration tests for `ralph tools task` CLI commands.

mod common;

use ralph_core::{Task, TaskStatus};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::TempDir;

fn ralph_task(temp_path: &std::path::Path, args: &[&str]) -> std::process::Output {
    common::ralph_bin()
        .arg("tools")
        .arg("task")
        .args(args)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph tools task command")
}

fn ralph_task_ok(temp_path: &std::path::Path, args: &[&str]) -> String {
    let output = ralph_task(temp_path, args);
    assert!(
        output.status.success(),
        "Command 'ralph tools task {}' failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn list_tasks(temp_path: &std::path::Path, extra_args: &[&str]) -> Vec<Task> {
    let mut args = vec!["list", "--format", "json"];
    args.extend_from_slice(extra_args);
    let stdout = ralph_task_ok(temp_path, &args);
    serde_json::from_str(&stdout).expect("Failed to parse task list JSON")
}

/// Regression: outer `ralph run` hat env must not poison human-CLI task helpers.
///
/// Simulates inherited `RALPH_CURRENT_HAT` / `EVENTS_FILE` / `LOOP_ID` on the
/// Command, then scrubs via `common::scrub_agent_runtime_env` before invoke.
#[test]
fn test_task_add_succeeds_after_scrubbing_simulated_hat_env() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    // Start from a raw Command so we can attach inherited-style pollution,
    // then prove scrub clears it before invoke (human-CLI semantics).
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ralph"));
    cmd.env("RALPH_CURRENT_HAT", "executor");
    cmd.env("RALPH_CURRENT_LOOP_ID", "loop-pollution");
    cmd.env("RALPH_EVENTS_FILE", "/tmp/should-not-be-used.jsonl");
    common::scrub_agent_runtime_env(&mut cmd);

    let output = cmd
        .args(["tools", "task", "add", "Under pollution", "--root"])
        .arg(temp_path)
        .current_dir(temp_path)
        .output()
        .expect("spawn ralph tools task add");

    assert!(
        output.status.success(),
        "task add must succeed after scrubbing hat env; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_task_add_and_list_json() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    ralph_task_ok(
        temp_path,
        &["add", "First task", "-p", "2", "-d", "Test description"],
    );

    let tasks = list_tasks(temp_path, &["--all"]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "First task");
    assert_eq!(tasks[0].priority, 2);
    assert_eq!(tasks[0].description.as_deref(), Some("Test description"));
}

#[test]
fn test_task_add_quiet_outputs_id() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let stdout = ralph_task_ok(temp_path, &["add", "Quiet task", "--format", "quiet"]);
    let id = stdout.trim();
    assert!(id.starts_with("task-"), "Expected task id, got: {}", id);

    let tasks = list_tasks(temp_path, &["--all"]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, id);
}

#[test]
fn test_task_ready_filters_by_loop_id() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let ralph_dir = temp_path.join(".ralph");
    std::fs::create_dir_all(&ralph_dir).expect("create .ralph");

    std::fs::write(ralph_dir.join("current-loop-id"), "loop-a").expect("write loop a");
    ralph_task_ok(temp_path, &["add", "Task A"]);

    std::fs::write(ralph_dir.join("current-loop-id"), "loop-b").expect("write loop b");
    ralph_task_ok(temp_path, &["add", "Task B"]);

    let stdout = ralph_task_ok(temp_path, &["ready", "--format", "json"]);
    let tasks: Vec<Task> = serde_json::from_str(&stdout).expect("parse ready JSON");

    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "Task B");
    assert_eq!(tasks[0].loop_id.as_deref(), Some("loop-b"));
}

#[test]
fn test_task_ready_respects_blockers() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    ralph_task_ok(temp_path, &["add", "Blocker"]);
    let tasks = list_tasks(temp_path, &["--all"]);
    let blocker_id = tasks[0].id.clone();

    ralph_task_ok(temp_path, &["add", "Blocked", "--blocked-by", &blocker_id]);

    let stdout = ralph_task_ok(temp_path, &["ready", "--format", "json"]);
    let ready: Vec<Task> = serde_json::from_str(&stdout).expect("parse ready JSON");
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].title, "Blocker");

    ralph_task_ok(temp_path, &["close", &blocker_id]);

    let stdout = ralph_task_ok(temp_path, &["ready", "--format", "json"]);
    let ready: Vec<Task> = serde_json::from_str(&stdout).expect("parse ready JSON");
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].title, "Blocked");
}

#[test]
fn test_task_close_and_fail_update_status() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    ralph_task_ok(temp_path, &["add", "Close me"]);
    ralph_task_ok(temp_path, &["add", "Fail me"]);

    let tasks = list_tasks(temp_path, &["--all"]);
    let close_id = tasks[0].id.clone();
    let fail_id = tasks[1].id.clone();

    ralph_task_ok(temp_path, &["close", &close_id]);
    ralph_task_ok(temp_path, &["fail", &fail_id]);

    let tasks = list_tasks(temp_path, &["--all"]);
    let status_by_id: std::collections::HashMap<String, TaskStatus> =
        tasks.into_iter().map(|t| (t.id, t.status)).collect();

    assert_eq!(status_by_id.get(&close_id), Some(&TaskStatus::Closed));
    assert_eq!(status_by_id.get(&fail_id), Some(&TaskStatus::Failed));
}

#[test]
fn test_task_ready_all_shows_tasks_from_all_loops() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let ralph_dir = temp_path.join(".ralph");
    std::fs::create_dir_all(&ralph_dir).expect("create .ralph");

    // Create tasks under different loop IDs (simulating sequential runs)
    std::fs::write(ralph_dir.join("current-loop-id"), "primary-run-1").expect("write run 1");
    ralph_task_ok(temp_path, &["add", "Task from run 1"]);

    std::fs::write(ralph_dir.join("current-loop-id"), "primary-run-2").expect("write run 2");
    ralph_task_ok(temp_path, &["add", "Task from run 2"]);

    // Without --all, only run-2 tasks are visible
    let stdout = ralph_task_ok(temp_path, &["ready", "--format", "json"]);
    let filtered: Vec<Task> = serde_json::from_str(&stdout).expect("parse");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].title, "Task from run 2");

    // With --all, both tasks are visible
    let stdout = ralph_task_ok(temp_path, &["ready", "--all", "--format", "json"]);
    let all: Vec<Task> = serde_json::from_str(&stdout).expect("parse");
    assert_eq!(all.len(), 2);
}

#[test]
fn test_task_show_json() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    ralph_task_ok(temp_path, &["add", "Show me"]);
    let tasks = list_tasks(temp_path, &["--all"]);
    let task_id = tasks[0].id.clone();

    let stdout = ralph_task_ok(temp_path, &["show", &task_id, "--format", "json"]);
    let task: Task = serde_json::from_str(&stdout).expect("parse task JSON");
    assert_eq!(task.id, task_id);
    assert_eq!(task.title, "Show me");
}

#[test]
fn test_task_ensure_deduplicates_by_key_and_updates_metadata() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let first_id = ralph_task_ok(
        temp_path,
        &[
            "ensure",
            "Initial title",
            "--key",
            "impl:task-01",
            "-p",
            "2",
            "--format",
            "quiet",
        ],
    )
    .trim()
    .to_string();

    let second_id = ralph_task_ok(
        temp_path,
        &[
            "ensure",
            "Updated title",
            "--key",
            "impl:task-01",
            "-p",
            "1",
            "--format",
            "quiet",
        ],
    )
    .trim()
    .to_string();

    assert_eq!(first_id, second_id);

    let tasks = list_tasks(temp_path, &["--all"]);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, first_id);
    assert_eq!(tasks[0].title, "Updated title");
    assert_eq!(tasks[0].priority, 1);
    assert_eq!(tasks[0].key.as_deref(), Some("impl:task-01"));
}

#[test]
fn test_task_start_and_reopen_update_lifecycle_fields() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let task_id = ralph_task_ok(temp_path, &["add", "Lifecycle task", "--format", "quiet"])
        .trim()
        .to_string();

    ralph_task_ok(temp_path, &["start", &task_id]);
    let stdout = ralph_task_ok(temp_path, &["show", &task_id, "--format", "json"]);
    let task: Task = serde_json::from_str(&stdout).expect("parse started task");
    assert_eq!(task.status, TaskStatus::InProgress);
    assert!(task.started.is_some());

    ralph_task_ok(temp_path, &["close", &task_id]);
    ralph_task_ok(temp_path, &["reopen", &task_id]);

    let stdout = ralph_task_ok(temp_path, &["show", &task_id, "--format", "json"]);
    let task: Task = serde_json::from_str(&stdout).expect("parse reopened task");
    assert_eq!(task.status, TaskStatus::Open);
    assert!(task.started.is_some());
    assert!(task.closed.is_none());
}

// ─────────────────────────────────────────────────────────────────────────
// U1 (2026-08-03-001-fix-opac-high-confidence-gates-plan): race-safe
// verify-then-apply claim lifecycle at the subprocess level.
// ─────────────────────────────────────────────────────────────────────────

/// Write a minimal `ralph.yml` that enables the task verify gate
/// for agents and pins `coordinator_hats` to `coordinator` so the
/// subprocess task policy allow-lists the simulated hat.
fn write_agent_gate_preset(temp_path: &std::path::Path) {
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();
    std::fs::write(
        temp_path.join("ralph.yml"),
        r#"
tasks:
  enabled: true
  require_verify_for_cli_mutate: true
  allow_unsafe_task_mutate: false
  coordinator_hats:
    - coordinator
event_loop:
  execution_mode: isolated
"#,
    )
    .unwrap();
}

/// Variant of [`write_agent_gate_preset`] whose `coordinator_hats`
/// allowlist carries `hats` verbatim. Used by the cross-scope
/// confirmation tests that need two distinct hats inside one loop;
/// the single-hat fixture above stays untouched for the existing
/// suite.
fn write_agent_gate_preset_with_hats(temp_path: &std::path::Path, hats: &[&str]) {
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();
    let list: Vec<String> = hats.iter().map(|h| format!("    - {h}")).collect();
    std::fs::write(
        temp_path.join("ralph.yml"),
        format!(
            "tasks:\n  enabled: true\n  require_verify_for_cli_mutate: true\n  \
             allow_unsafe_task_mutate: false\n  coordinator_hats:\n{list}\n\
             event_loop:\n  execution_mode: isolated\n",
            list = list.join("\n")
        ),
    )
    .unwrap();
}

/// Spawn `ralph tools task <args...>` as the simulated hat `hat`
/// inside `loop_id` against the gate-enabled preset. Scrubs inherited
/// hat env first per HARD RULE 5, then re-installs the simulated
/// agent context explicitly.
fn spawn_task_as(
    temp_path: &std::path::Path,
    hat: &str,
    loop_id: &str,
    args: &[&str],
) -> std::process::Output {
    let mut cmd = common::ralph_bin();
    cmd.env("RALPH_CURRENT_HAT", hat)
        .env("RALPH_CURRENT_LOOP_ID", loop_id)
        .arg("tools")
        .arg("task")
        .args(args)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path);
    cmd.output()
        .expect("Failed to execute ralph tools task command")
}

/// Locate the row carrying `key` in `tasks.jsonl` and return the raw
/// JSON value. Panics if the row is missing.
fn row_by_key(temp_path: &std::path::Path, key: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(temp_path.join(".ralph/agent/tasks.jsonl"))
        .expect("read tasks.jsonl");
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).expect("row parses as JSON");
        if v.get("key").and_then(|k| k.as_str()) == Some(key) {
            return v;
        }
    }
    panic!("row with key '{key}' not found in tasks.jsonl");
}

/// Extract `(state, loop_id, hat_id)` from a row's confirmation JSON.
fn confirmation_scope(cfm: &serde_json::Value) -> (String, String, String) {
    (
        cfm.get("state")
            .and_then(|s| s.as_str())
            .expect("confirmation.state")
            .to_string(),
        cfm.get("loop_id")
            .and_then(|s| s.as_str())
            .expect("confirmation.loop_id")
            .to_string(),
        cfm.get("hat_id")
            .and_then(|s| s.as_str())
            .expect("confirmation.hat_id")
            .to_string(),
    )
}

/// Spawn a `ralph tools task verify <verb>` subprocess that runs
/// as the simulated `coordinator` hat inside the gate-enabled
/// preset. Scrubs inherited hat env first per HARD RULE 5.
fn spawn_verify_add(temp_path: &std::path::Path, title: &str) -> std::process::Output {
    let mut cmd = common::ralph_bin();
    cmd.env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_CURRENT_LOOP_ID", "loop-u1")
        .arg("tools")
        .arg("task")
        .arg("verify")
        .arg("add")
        .arg(title)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path);
    cmd.output().expect("spawn ralph tools task verify add")
}

/// Spawn a `ralph tools task add <title>` subprocess that runs
/// as the simulated `coordinator` hat inside the gate-enabled
/// preset.
fn spawn_apply_add(temp_path: &std::path::Path, title: &str) -> std::process::Output {
    let mut cmd = common::ralph_bin();
    cmd.env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_CURRENT_LOOP_ID", "loop-u1")
        .arg("tools")
        .arg("task")
        .arg("add")
        .arg(title)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path);
    cmd.output().expect("spawn ralph tools task add")
}

/// Spawn a `ralph tools task verify add <title> <extra_args...>`
/// subprocess as the simulated `coordinator` hat (loop-u1). Mirrors
/// `spawn_verify_add` but parametrises the trailing args so the
/// verified payload can include e.g. `--blocked-by`.
fn spawn_verify_add_with_args(
    temp_path: &std::path::Path,
    title: &str,
    extra_args: &[&str],
) -> std::process::Output {
    let mut cmd = common::ralph_bin();
    cmd.env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_CURRENT_LOOP_ID", "loop-u1")
        .arg("tools")
        .arg("task")
        .arg("verify")
        .arg("add")
        .arg(title)
        .args(extra_args)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path);
    cmd.output().expect("spawn ralph tools task verify add")
}

/// Spawn a `ralph tools task add <title> <extra_args...>` subprocess
/// as the simulated `coordinator` hat (loop-u1). Mirrors
/// `spawn_apply_add` but parametrises the trailing args so the Apply
/// payload can include e.g. `--blocked-by`.
fn spawn_apply_add_with_args(
    temp_path: &std::path::Path,
    title: &str,
    extra_args: &[&str],
) -> std::process::Output {
    let mut cmd = common::ralph_bin();
    cmd.env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_CURRENT_LOOP_ID", "loop-u1")
        .arg("tools")
        .arg("task")
        .arg("add")
        .arg(title)
        .args(extra_args)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path);
    cmd.output().expect("spawn ralph tools task add")
}

/// U1: two real `ralph tools task add` subprocesses racing the
/// same prepared ticket must produce exactly one winner. The
/// loser must receive `task_verify_gate denied` and the task
/// store must record at most one Apply.
#[test]
fn test_task_verify_concurrent_apply_claims_once() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    // Record a single matching prepared ticket.
    let verify = spawn_verify_add(temp_path, "Concurrent target");
    assert!(
        verify.status.success(),
        "verify must succeed before concurrent Apply; stderr={}",
        String::from_utf8_lossy(&verify.stderr)
    );

    // Race two Apply processes against the same prepared ticket.
    let barrier = Arc::new(Barrier::new(2));
    let temp_path_a = temp_path.to_path_buf();
    let temp_path_b = temp_path.to_path_buf();
    let barrier_a = barrier.clone();
    let barrier_b = barrier.clone();

    let handle_a = thread::spawn(move || {
        barrier_a.wait();
        spawn_apply_add(&temp_path_a, "Concurrent target")
    });
    let handle_b = thread::spawn(move || {
        barrier_b.wait();
        spawn_apply_add(&temp_path_b, "Concurrent target")
    });
    let result_a = handle_a.join().expect("join a");
    let result_b = handle_b.join().expect("join b");

    let oks = [&result_a, &result_b]
        .iter()
        .filter(|o| o.status.success())
        .count();
    let denials = [&result_a, &result_b]
        .iter()
        .filter(|o| {
            !o.status.success()
                && String::from_utf8_lossy(&o.stderr).contains("task_verify_gate denied")
        })
        .count();

    assert_eq!(
        oks,
        1,
        "exactly one Apply must win: a.success={} b.success={}; a.stderr={} b.stderr={}",
        result_a.status.success(),
        result_b.status.success(),
        String::from_utf8_lossy(&result_a.stderr),
        String::from_utf8_lossy(&result_b.stderr)
    );
    assert_eq!(
        denials,
        1,
        "the loser must be denied with task_verify_gate denied prefix; \
         a.stderr={} b.stderr={}",
        String::from_utf8_lossy(&result_a.stderr),
        String::from_utf8_lossy(&result_b.stderr)
    );

    // The task store must record exactly one task — at most one
    // Apply actually wrote.
    let tasks = list_tasks(temp_path, &[]);
    let concurrent_tasks: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.title == "Concurrent target")
        .collect();
    assert_eq!(
        concurrent_tasks.len(),
        1,
        "exactly one task must be written: tasks={:?}",
        tasks
    );
}

/// U1: a fingerprint mismatch (recorded for one title, Apply
/// uses another) must leave the prepared record on disk so a
/// corrected retry succeeds without re-running verify.
#[test]
fn test_task_verify_mismatch_preserves_prepared_record() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    // Verify a ticket for "Original title".
    let verify = spawn_verify_add(temp_path, "Original title");
    assert!(
        verify.status.success(),
        "verify must succeed; stderr={}",
        String::from_utf8_lossy(&verify.stderr)
    );

    // Apply with a different title. Under U2's per-intent
    // namespacing, the apply path is derived from the (verb,
    // payload, loop, hat) tuple, so the apply for "Different
    // title" looks for a different on-disk file than the one
    // verify wrote for "Original title". The gate denies with
    // `task_verify_gate denied` and explains the missing ticket;
    // the prepared record for "Original title" must remain on
    // disk untouched.
    let mismatched = spawn_apply_add(temp_path, "Different title");
    let mismatched_stderr = String::from_utf8_lossy(&mismatched.stderr);
    assert!(
        !mismatched.status.success(),
        "mismatch must deny; stderr={}",
        mismatched_stderr
    );
    assert!(
        mismatched_stderr.contains("task_verify_gate denied"),
        "denial must carry stable prefix; stderr={}",
        mismatched_stderr
    );

    // The prepared ticket for "Original title" must still be on
    // disk under the per-intent namespace.
    let ticket_dir = temp_path.join(".ralph/agent/task-tickets");
    assert!(ticket_dir.is_dir(), "scoped ticket directory must exist");
    let ticket_count = std::fs::read_dir(&ticket_dir)
        .expect("read ticket dir")
        .filter_map(|e| e.ok())
        .count();
    assert!(
        ticket_count >= 1,
        "mismatch must leave the prepared record on disk (found {ticket_count} entries)"
    );

    // A corrected Apply against the original title must now
    // succeed without a fresh verify.
    let corrected = spawn_apply_add(temp_path, "Original title");
    let corrected_stderr = String::from_utf8_lossy(&corrected.stderr);
    assert!(
        corrected.status.success(),
        "corrected Apply must succeed without re-verify; stderr={}",
        corrected_stderr
    );

    let tasks = list_tasks(temp_path, &[]);
    assert_eq!(
        tasks.iter().filter(|t| t.title == "Original title").count(),
        1,
        "exactly one Original-title task must exist"
    );
    assert_eq!(
        tasks
            .iter()
            .filter(|t| t.title == "Different title")
            .count(),
        0,
        "mismatch Apply must not create any task"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// U2 (2026-08-03-001-fix-opac-high-confidence-gates-plan): per-operation
// ticket namespace at the subprocess level.
// ─────────────────────────────────────────────────────────────────────────

/// Spawn a `ralph tools task verify <verb>` subprocess for any
/// task verb (`add` or `ensure`). Mirrors `spawn_verify_add` but
/// parametrises the verb so we can drive the add/ensure
/// coexistence scenarios.
fn spawn_verify(temp_path: &std::path::Path, verb: &str, args: &[&str]) -> std::process::Output {
    let mut cmd = common::ralph_bin();
    cmd.env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_CURRENT_LOOP_ID", "loop-u2")
        .arg("tools")
        .arg("task")
        .arg("verify")
        .arg(verb)
        .args(args)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path);
    cmd.output().expect("spawn ralph tools task verify")
}

/// Spawn a `ralph tools task <verb>` subprocess for any verb.
fn spawn_apply(temp_path: &std::path::Path, verb: &str, args: &[&str]) -> std::process::Output {
    let mut cmd = common::ralph_bin();
    cmd.env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_CURRENT_LOOP_ID", "loop-u2")
        .arg("tools")
        .arg("task")
        .arg(verb)
        .args(args)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path);
    cmd.output().expect("spawn ralph tools task apply")
}

/// U2: `verify add A` and `verify ensure B` must each write to
/// independent ticket files; both subsequent Apply invocations
/// must succeed (one task each) and neither verify must
/// overwrite the other.
#[test]
fn test_task_verify_add_and_ensure_tickets_coexist() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    // Step 1: verify both intents in the same workspace.
    let verify_add = spawn_verify(temp_path, "add", &["Scoped add target"]);
    let verify_ensure = spawn_verify(
        temp_path,
        "ensure",
        &["Scoped ensure target", "--key", "scoped:ensure"],
    );
    assert!(
        verify_add.status.success(),
        "verify add must succeed; stderr={}",
        String::from_utf8_lossy(&verify_add.stderr)
    );
    assert!(
        verify_ensure.status.success(),
        "verify ensure must succeed; stderr={}",
        String::from_utf8_lossy(&verify_ensure.stderr)
    );

    // Both ticket files exist independently under the namespace.
    let ticket_dir = temp_path.join(".ralph/agent/task-tickets");
    assert!(
        ticket_dir.is_dir(),
        "scoped ticket directory must exist: {}",
        ticket_dir.display()
    );
    let entries: Vec<_> = std::fs::read_dir(&ticket_dir)
        .expect("read ticket dir")
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        entries.len() >= 2,
        "both scoped tickets must exist: entries={:?}",
        entries
    );

    // Step 2: apply the add intent — must succeed without
    // disturbing the ensure ticket.
    let apply_add = spawn_apply(temp_path, "add", &["Scoped add target"]);
    let apply_add_stderr = String::from_utf8_lossy(&apply_add.stderr);
    assert!(
        apply_add.status.success(),
        "apply add must succeed; stderr={}",
        apply_add_stderr
    );
    assert!(
        !apply_add_stderr.contains("task_verify_gate denied"),
        "apply add must not be denied; stderr={}",
        apply_add_stderr
    );

    // Unit 1 confirmation contract: the successful add Apply recorded a
    // pending confirmation on its row; consume it so the next same-scope
    // protected mutation passes the gate. (Added step — the ticket
    // independence assertions below are unchanged.)
    let (add_id, add_ref, add_digest) = confirmation_of_task_titled(temp_path, "Scoped add target");
    let confirm_add = spawn_confirm(temp_path, "loop-u2", &add_id, &add_ref, &add_digest);
    assert!(
        confirm_add.status.success(),
        "confirm of the add task must succeed; stderr={}",
        String::from_utf8_lossy(&confirm_add.stderr)
    );

    // Step 3: apply the ensure intent — must still succeed (its
    // ticket was not consumed by the add apply).
    let apply_ensure = spawn_apply(
        temp_path,
        "ensure",
        &["Scoped ensure target", "--key", "scoped:ensure"],
    );
    let apply_ensure_stderr = String::from_utf8_lossy(&apply_ensure.stderr);
    assert!(
        apply_ensure.status.success(),
        "apply ensure must succeed after add apply; stderr={}",
        apply_ensure_stderr
    );

    // Both tasks must be present in the store.
    let tasks = list_tasks(temp_path, &[]);
    assert!(
        tasks.iter().any(|t| t.title == "Scoped add target"),
        "add task must be present"
    );
    assert!(
        tasks.iter().any(|t| t.title == "Scoped ensure target"),
        "ensure task must be present"
    );
}

/// U2: a fresh verify for a different intent must not
/// invalidate a previously-pending, yet-unconsumed ticket.
#[test]
fn test_task_later_verify_does_not_invalidate_prior_intent() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    // Verify intent A.
    let verify_a = spawn_verify(temp_path, "add", &["Intent A"]);
    assert!(verify_a.status.success());

    // Verify intent B — must not invalidate A.
    let verify_b = spawn_verify(temp_path, "add", &["Intent B"]);
    assert!(verify_b.status.success());

    // Both tickets are independently present.
    let tickets: Vec<_> = std::fs::read_dir(temp_path.join(".ralph/agent/task-tickets"))
        .expect("read ticket dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .collect();
    assert!(
        tickets.len() >= 2,
        "both scoped tickets must coexist: tickets={:?}",
        tickets
    );

    // Apply A succeeds; B's ticket is untouched.
    let apply_a = spawn_apply(temp_path, "add", &["Intent A"]);
    assert!(
        apply_a.status.success(),
        "apply A must succeed; stderr={}",
        String::from_utf8_lossy(&apply_a.stderr)
    );

    // Unit 1 confirmation contract: A's Apply recorded a pending
    // confirmation; consume it so B's protected mutation passes the
    // gate. (Added step — the ticket-coexistence assertions above and
    // below are unchanged.)
    let (a_id, a_ref, a_digest) = confirmation_of_task_titled(temp_path, "Intent A");
    let confirm_a = spawn_confirm(temp_path, "loop-u2", &a_id, &a_ref, &a_digest);
    assert!(
        confirm_a.status.success(),
        "confirm of Intent A must succeed; stderr={}",
        String::from_utf8_lossy(&confirm_a.stderr)
    );

    // Apply B also succeeds; both tasks recorded.
    let apply_b = spawn_apply(temp_path, "add", &["Intent B"]);
    assert!(
        apply_b.status.success(),
        "apply B must succeed after A; stderr={}",
        String::from_utf8_lossy(&apply_b.stderr)
    );

    let tasks = list_tasks(temp_path, &[]);
    assert!(tasks.iter().any(|t| t.title == "Intent A"));
    assert!(tasks.iter().any(|t| t.title == "Intent B"));
}

// ─────────────────────────────────────────────────────────────────────────
// U1 contract (2026-08-03-001-fix-opac-high-confidence-gates-plan):
// only a successful Apply may consume the ticket. A post-gate store
// failure must restore the ticket so the agent can retry without
// re-running verify.
// ─────────────────────────────────────────────────────────────────────────

/// Remove the task with `task_id` from the TempDir fixture's
/// `tasks.jsonl` and return the removed JSONL line so the test can
/// restore it later. Simulates the blocker disappearing from the
/// store between verify and Apply (the CLI has no delete verb).
fn remove_task_line_from_store(temp_path: &std::path::Path, task_id: &str) -> String {
    let store_path = temp_path.join(".ralph/agent/tasks.jsonl");
    let raw = std::fs::read_to_string(&store_path).expect("read tasks.jsonl fixture");
    let mut removed = None;
    let mut kept = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let is_target = serde_json::from_str::<serde_json::Value>(line)
            .map(|v| v.get("id").and_then(|i| i.as_str()) == Some(task_id))
            .unwrap_or(false);
        if is_target && removed.is_none() {
            removed = Some(line.to_string());
        } else {
            kept.push(line.to_string());
        }
    }
    let removed = removed.expect("blocker line must exist in tasks.jsonl fixture");
    let body = if kept.is_empty() {
        String::new()
    } else {
        kept.join("\n") + "\n"
    };
    std::fs::write(&store_path, body).expect("rewrite tasks.jsonl fixture");
    removed
}

/// Append a previously removed JSONL line back to the fixture store
/// ("the agent fixed the cause of the failed Apply").
fn append_task_line_to_store(temp_path: &std::path::Path, line: &str) {
    let store_path = temp_path.join(".ralph/agent/tasks.jsonl");
    let mut body = std::fs::read_to_string(&store_path).unwrap_or_default();
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(line);
    body.push('\n');
    std::fs::write(&store_path, body).expect("append to tasks.jsonl fixture");
}

/// U1: when the gate passes but the store mutation fails afterwards,
/// the ticket must be restored so a retry succeeds without re-verify.
///
/// Trigger note: the failure must happen AFTER the gate matched, so the
/// Apply payload must be identical to the verified payload — U2
/// namespaces tickets by an intent digest that includes `blocked_by`
/// (`canonical_add_payload`), so verifying title-only and applying with
/// an extra `--blocked-by` is denied at the gate (different intent
/// path) and never reaches the store; and `task verify add` itself
/// refuses to record a ticket for a nonexistent blocker. This test
/// therefore verifies `add "Retry target" --blocked-by <real blocker>`,
/// drops the blocker from the fixture store, then applies the identical
/// payload: the gate passes and blocked_by validation fails.
///
/// Pre-fix, `execute_add` consumes the ticket at claim time (before the
/// mutation), so the ticket-survival assertion fails; post-fix the
/// ticket is restored and the same-payload retry succeeds.
#[test]
fn test_task_add_failure_after_claim_restores_ticket_for_retry() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    // Seed a blocker task into loop-u1 through the normal
    // verify → apply path (its own ticket is consumed on success —
    // the happy path works both pre- and post-fix).
    let verify_blocker = spawn_verify_add(temp_path, "Blocker task");
    assert!(
        verify_blocker.status.success(),
        "verify blocker must succeed; stderr={}",
        String::from_utf8_lossy(&verify_blocker.stderr)
    );
    let apply_blocker = spawn_apply_add(temp_path, "Blocker task");
    assert!(
        apply_blocker.status.success(),
        "apply blocker must succeed; stderr={}",
        String::from_utf8_lossy(&apply_blocker.stderr)
    );
    let blocker_id = list_tasks(temp_path, &[])
        .iter()
        .find(|t| t.title == "Blocker task")
        .map(|t| t.id.clone())
        .expect("blocker task must be listed");

    // Unit 1 confirmation contract: the blocker's protected Apply
    // recorded a pending confirmation; consume it now so the later
    // Retry-target mutations are judged on their own gate state. The
    // confirmed state travels with the row through the remove/restore
    // fixture steps below. (Added step — original assertions unchanged.)
    let (blocker_task_id, blocker_ref, blocker_digest) =
        confirmation_of_task_titled(temp_path, "Blocker task");
    assert_eq!(blocker_task_id, blocker_id, "blocker identity consistent");
    let confirm_blocker = spawn_confirm(
        temp_path,
        "loop-u1",
        &blocker_task_id,
        &blocker_ref,
        &blocker_digest,
    );
    assert!(
        confirm_blocker.status.success(),
        "confirm blocker must succeed; stderr={}",
        String::from_utf8_lossy(&confirm_blocker.stderr)
    );

    // Verify the real target with the blocker inside the payload.
    let blocked_by_args = ["--blocked-by", blocker_id.as_str()];
    let verify = spawn_verify_add_with_args(temp_path, "Retry target", &blocked_by_args);
    assert!(
        verify.status.success(),
        "verify must succeed; stderr={}",
        String::from_utf8_lossy(&verify.stderr)
    );

    // Drop the blocker from the fixture store so the identical Apply
    // payload now fails blocked_by validation after the gate passes.
    let blocker_line = remove_task_line_from_store(temp_path, &blocker_id);

    // Apply the verified (identical) payload: the gate passes, the
    // store mutation fails on blocked_by validation.
    let failed_apply = spawn_apply_add_with_args(temp_path, "Retry target", &blocked_by_args);
    let failed_stderr = String::from_utf8_lossy(&failed_apply.stderr);
    assert!(
        !failed_apply.status.success(),
        "apply with a vanished blocker must fail; stderr={failed_stderr}"
    );
    assert!(
        !failed_stderr.contains("task_verify_gate denied"),
        "failure must come from blocked_by validation after a passed gate, \
         not from the gate itself; stderr={failed_stderr}"
    );
    assert!(
        failed_stderr.contains("blocked_by"),
        "stderr must surface the blocked_by validation failure; stderr={failed_stderr}"
    );
    let tasks = list_tasks(temp_path, &[]);
    assert!(
        tasks.is_empty(),
        "failed Apply must not write to the store; tasks={tasks:?}"
    );

    // U1 contract: the ticket must survive the failed Apply — a
    // prepared `.ticket` file exists and no `.ticket.claimed` marker
    // lingers. (`.ticket.lock` siblings from the gate's FileLock are
    // excluded by the exact `.ticket` suffix match.)
    let ticket_dir = temp_path.join(".ralph/agent/task-tickets");
    let entries: Vec<String> = std::fs::read_dir(&ticket_dir)
        .expect("read ticket dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let prepared = entries.iter().filter(|n| n.ends_with(".ticket")).count();
    let claimed = entries
        .iter()
        .filter(|n| n.ends_with(".ticket.claimed"))
        .count();
    assert!(
        prepared >= 1,
        "failed Apply must leave a prepared ticket for retry; entries={entries:?}"
    );
    assert_eq!(
        claimed, 0,
        "no claim marker may linger after a failed Apply; entries={entries:?}"
    );

    // Fix the failure cause (blocker is back in the store) and retry
    // the identical payload — must succeed without re-running verify.
    append_task_line_to_store(temp_path, &blocker_line);
    let retry = spawn_apply_add_with_args(temp_path, "Retry target", &blocked_by_args);
    assert!(
        retry.status.success(),
        "retry Apply must succeed without re-verify; stderr={}",
        String::from_utf8_lossy(&retry.stderr)
    );

    let tasks = list_tasks(temp_path, &[]);
    assert_eq!(
        tasks.iter().filter(|t| t.title == "Retry target").count(),
        1,
        "exactly one Retry target task must exist; tasks={tasks:?}"
    );
}

/// U2 behavior lock: the legacy fixed-path plaintext ticket
/// (`.ralph/agent/.ralph-task-verify-ticket`) must never satisfy the
/// scoped gate. A planted legacy file is ignored, the Apply is denied
/// with the stable prefix, the file stays on disk byte-identical
/// (never consumed or accepted), and the store stays empty.
#[test]
fn test_legacy_plaintext_ticket_is_not_trusted() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    // Plant a legacy plaintext ticket at the old fixed path.
    let legacy_path = temp_path.join(".ralph/agent/.ralph-task-verify-ticket");
    std::fs::create_dir_all(legacy_path.parent().expect("legacy parent dir"))
        .expect("create .ralph/agent");
    let legacy_content = "legacy-ticket-v0 garbage";
    std::fs::write(&legacy_path, legacy_content).expect("write legacy ticket fixture");

    let apply = spawn_apply_add(temp_path, "Legacy bait");
    let stderr = String::from_utf8_lossy(&apply.stderr);
    assert!(
        !apply.status.success(),
        "legacy plaintext ticket must not satisfy the gate; stderr={stderr}"
    );
    assert!(
        stderr.contains("task_verify_gate denied"),
        "denial must carry the stable prefix; stderr={stderr}"
    );

    // The legacy file must remain in place, byte-identical.
    let after = std::fs::read(&legacy_path).expect("legacy ticket must still exist");
    assert_eq!(
        after,
        legacy_content.as_bytes(),
        "legacy ticket must be untouched (not consumed or accepted)"
    );

    let tasks = list_tasks(temp_path, &[]);
    assert!(tasks.is_empty(), "store must stay empty; tasks={tasks:?}");
}

// ─────────────────────────────────────────────────────────────────────────
// Unit 1 (task confirmation 最小纵向切片): a successful protected
// Apply records a pending confirmation that the same loop/hat must
// consume via `ralph tools task confirm` before the next protected
// mutation passes the gate.
// ─────────────────────────────────────────────────────────────────────────

/// Spawn `ralph tools task confirm <task_id> --reference <ref> --digest
/// <digest>` as the simulated `coordinator` hat inside `loop_id`.
fn spawn_confirm(
    temp_path: &std::path::Path,
    loop_id: &str,
    task_id: &str,
    reference: &str,
    digest: &str,
) -> std::process::Output {
    let mut cmd = common::ralph_bin();
    cmd.env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_CURRENT_LOOP_ID", loop_id)
        .arg("tools")
        .arg("task")
        .arg("confirm")
        .arg(task_id)
        .arg("--reference")
        .arg(reference)
        .arg("--digest")
        .arg(digest)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path);
    cmd.output().expect("spawn ralph tools task confirm")
}

/// Locate the task titled `title` in `tasks.jsonl` and return its
/// `(task_id, confirmation.reference, confirmation.digest)`. Panics if
/// the row or its confirmation is missing.
fn confirmation_of_task_titled(
    temp_path: &std::path::Path,
    title: &str,
) -> (String, String, String) {
    let raw = std::fs::read_to_string(temp_path.join(".ralph/agent/tasks.jsonl"))
        .expect("read tasks.jsonl");
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).expect("row parses as JSON");
        if v.get("title").and_then(|t| t.as_str()) == Some(title) {
            let cfm = v
                .get("confirmation")
                .unwrap_or_else(|| panic!("task '{title}' row must carry confirmation: {line}"));
            return (
                v.get("id")
                    .and_then(|i| i.as_str())
                    .expect("row id")
                    .to_string(),
                cfm.get("reference")
                    .and_then(|r| r.as_str())
                    .expect("confirmation.reference")
                    .to_string(),
                cfm.get("digest")
                    .and_then(|d| d.as_str())
                    .expect("confirmation.digest")
                    .to_string(),
            );
        }
    }
    panic!("task titled '{title}' not found in tasks.jsonl");
}

/// S2: a matching `task confirm` from the same loop/hat transitions the
/// pending confirmation to `confirmed`, is idempotent on repeat (no new
/// row, no rewrite), and unblocks the next same-scope protected
/// mutation.
#[test]
fn test_task_confirmation_confirm_transitions_and_unblocks_next_mutation() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    // Protected Apply records a pending confirmation (S1 contract).
    let verify = spawn_verify_add(temp_path, "Confirm me");
    assert!(
        verify.status.success(),
        "verify must succeed; stderr={}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let apply = spawn_apply_add_with_args(temp_path, "Confirm me", &["--format", "json"]);
    let apply_stderr = String::from_utf8_lossy(&apply.stderr);
    assert!(
        apply.status.success(),
        "apply must succeed; stderr={apply_stderr}"
    );
    let stdout = String::from_utf8_lossy(&apply.stdout);
    let applied: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("apply stdout must be task JSON");
    let task_id = applied
        .get("id")
        .and_then(|v| v.as_str())
        .expect("task JSON must carry id")
        .to_string();
    let cfm = applied
        .get("confirmation")
        .expect("apply JSON must carry confirmation");
    let reference = cfm
        .get("reference")
        .and_then(|v| v.as_str())
        .expect("reference")
        .to_string();
    let digest = cfm
        .get("digest")
        .and_then(|v| v.as_str())
        .expect("digest")
        .to_string();

    // Confirm from the same loop/hat → exit 0, state becomes confirmed.
    let confirm = spawn_confirm(temp_path, "loop-u1", &task_id, &reference, &digest);
    let confirm_stderr = String::from_utf8_lossy(&confirm.stderr);
    assert!(
        confirm.status.success(),
        "confirm must succeed; stderr={confirm_stderr}"
    );

    let show = ralph_task_ok(temp_path, &["show", &task_id, "--format", "json"]);
    let shown: serde_json::Value = serde_json::from_str(&show).expect("show JSON parses");
    assert_eq!(
        shown
            .get("confirmation")
            .and_then(|c| c.get("state"))
            .and_then(|s| s.as_str()),
        Some("confirmed"),
        "state must be confirmed after task confirm; show={show}"
    );

    // Idempotent re-confirm: exit 0, no new row, store byte-identical.
    let store_path = temp_path.join(".ralph/agent/tasks.jsonl");
    let before = std::fs::read(&store_path).expect("read tasks.jsonl");
    let confirm_again = spawn_confirm(temp_path, "loop-u1", &task_id, &reference, &digest);
    assert!(
        confirm_again.status.success(),
        "repeat confirm must be idempotent (exit 0); stderr={}",
        String::from_utf8_lossy(&confirm_again.stderr)
    );
    let after = std::fs::read(&store_path).expect("read tasks.jsonl");
    assert_eq!(
        before, after,
        "idempotent confirm must not rewrite or append rows"
    );

    // The next same-scope protected mutation passes after the confirm.
    let verify_next = spawn_verify_add(temp_path, "Next mutation");
    assert!(
        verify_next.status.success(),
        "verify of next mutation must succeed; stderr={}",
        String::from_utf8_lossy(&verify_next.stderr)
    );
    let apply_next = spawn_apply_add(temp_path, "Next mutation");
    assert!(
        apply_next.status.success(),
        "post-confirm protected mutation must pass; stderr={}",
        String::from_utf8_lossy(&apply_next.stderr)
    );

    let tasks = list_tasks(temp_path, &[]);
    assert_eq!(
        tasks.iter().filter(|t| t.title == "Next mutation").count(),
        1,
        "exactly one Next-mutation task must exist; tasks={tasks:?}"
    );
}

/// S3: while a confirmation is pending, the next same-scope protected
/// mutation must be denied with `confirmation_required` (under the
/// stable `task_verify_gate denied` prefix), leave `tasks.jsonl`
/// untouched, create no second task, and preserve the prepared ticket
/// so the retry after `task confirm` succeeds without a fresh verify.
#[test]
fn test_task_confirmation_pending_blocks_next_mutation_until_confirmed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    // First protected mutation lands with a pending confirmation.
    let verify_first = spawn_verify_add(temp_path, "First mutation");
    assert!(
        verify_first.status.success(),
        "verify first must succeed; stderr={}",
        String::from_utf8_lossy(&verify_first.stderr)
    );
    let apply_first = spawn_apply_add(temp_path, "First mutation");
    assert!(
        apply_first.status.success(),
        "apply first must succeed; stderr={}",
        String::from_utf8_lossy(&apply_first.stderr)
    );

    // Prepare the second mutation (verify writes its ticket).
    let verify_second = spawn_verify_add(temp_path, "Second mutation");
    assert!(
        verify_second.status.success(),
        "verify second must succeed even while first is pending; stderr={}",
        String::from_utf8_lossy(&verify_second.stderr)
    );

    let store_path = temp_path.join(".ralph/agent/tasks.jsonl");
    let before = std::fs::read(&store_path).expect("read tasks.jsonl");
    // BDD "事件无变化" clause: capture the event ledger (may not exist
    // at all — `None`) and require it byte-identical after the denial.
    let events_path = temp_path.join(".ralph/events.jsonl");
    let events_before = std::fs::read(&events_path).ok();

    // Apply second WITHOUT confirming first → denied.
    let denied = spawn_apply_add(temp_path, "Second mutation");
    let denied_stderr = String::from_utf8_lossy(&denied.stderr);
    assert!(
        !denied.status.success(),
        "unconfirmed pending must block the next protected mutation; stderr={denied_stderr}"
    );
    assert!(
        denied_stderr.contains("task_verify_gate denied"),
        "denial must carry the stable gate prefix; stderr={denied_stderr}"
    );
    assert!(
        denied_stderr.contains("confirmation_required"),
        "denial must carry the stable confirmation_required token; stderr={denied_stderr}"
    );

    // No side effects: store byte-identical, no second task.
    let after = std::fs::read(&store_path).expect("read tasks.jsonl");
    assert_eq!(before, after, "denied mutation must not touch tasks.jsonl");
    let events_after = std::fs::read(&events_path).ok();
    assert_eq!(
        events_before, events_after,
        "denied mutation must not touch .ralph/events.jsonl \
         (absent stays absent, present stays byte-identical)"
    );
    let tasks = list_tasks(temp_path, &[]);
    assert_eq!(
        tasks
            .iter()
            .filter(|t| t.title == "Second mutation")
            .count(),
        0,
        "denied mutation must not create a task; tasks={tasks:?}"
    );

    // Prepared ticket survives the denial (retry after confirm must not
    // need a fresh verify).
    let ticket_dir = temp_path.join(".ralph/agent/task-tickets");
    let entries: Vec<String> = std::fs::read_dir(&ticket_dir)
        .expect("read ticket dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let prepared = entries.iter().filter(|n| n.ends_with(".ticket")).count();
    let claimed = entries
        .iter()
        .filter(|n| n.ends_with(".ticket.claimed"))
        .count();
    assert!(
        prepared >= 1,
        "denied mutation must preserve the prepared ticket; entries={entries:?}"
    );
    assert_eq!(
        claimed, 0,
        "denied mutation must not leave a claim marker; entries={entries:?}"
    );

    // Confirm first, then the preserved ticket lets second through.
    let (first_id, first_ref, first_digest) =
        confirmation_of_task_titled(temp_path, "First mutation");
    let confirm = spawn_confirm(temp_path, "loop-u1", &first_id, &first_ref, &first_digest);
    assert!(
        confirm.status.success(),
        "confirm first must succeed; stderr={}",
        String::from_utf8_lossy(&confirm.stderr)
    );
    let apply_second = spawn_apply_add(temp_path, "Second mutation");
    assert!(
        apply_second.status.success(),
        "post-confirm retry must pass without re-verify; stderr={}",
        String::from_utf8_lossy(&apply_second.stderr)
    );
    let tasks = list_tasks(temp_path, &[]);
    assert_eq!(
        tasks
            .iter()
            .filter(|t| t.title == "Second mutation")
            .count(),
        1,
        "exactly one Second-mutation task after confirm; tasks={tasks:?}"
    );
}

/// S4: confirm with a wrong digest, a wrong reference, or a different
/// loop must fail with the stable reason tokens, leave the state
/// `pending`, and a later correct confirm must still succeed.
#[test]
fn test_task_confirmation_mismatch_keeps_state_pending() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    let verify = spawn_verify_add(temp_path, "Mismatch target");
    assert!(verify.status.success(), "verify must succeed");
    let apply = spawn_apply_add_with_args(temp_path, "Mismatch target", &["--format", "json"]);
    assert!(
        apply.status.success(),
        "apply must succeed; stderr={}",
        String::from_utf8_lossy(&apply.stderr)
    );
    let (task_id, reference, digest) = confirmation_of_task_titled(temp_path, "Mismatch target");

    let state_of = || {
        let raw = std::fs::read_to_string(temp_path.join(".ralph/agent/tasks.jsonl"))
            .expect("read tasks.jsonl");
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .find_map(|line| {
                let v: serde_json::Value = serde_json::from_str(line).ok()?;
                if v.get("id").and_then(|i| i.as_str()) == Some(task_id.as_str()) {
                    v.get("confirmation")
                        .and_then(|c| c.get("state"))
                        .and_then(|s| s.as_str())
                        .map(str::to_string)
                } else {
                    None
                }
            })
            .expect("task row with confirmation must exist")
    };

    // Wrong digest, right reference → confirmation_mismatch, stays pending.
    let bad_digest = spawn_confirm(temp_path, "loop-u1", &task_id, &reference, "deadbeef");
    let bad_digest_stderr = String::from_utf8_lossy(&bad_digest.stderr);
    assert!(
        !bad_digest.status.success(),
        "wrong digest must fail; stderr={bad_digest_stderr}"
    );
    assert!(
        bad_digest_stderr.contains("confirmation_mismatch"),
        "wrong digest must surface confirmation_mismatch; stderr={bad_digest_stderr}"
    );
    assert_eq!(
        state_of(),
        "pending",
        "state must stay pending after wrong digest"
    );

    // Wrong reference → confirmation_unavailable, stays pending.
    let bad_reference = spawn_confirm(
        temp_path,
        "loop-u1",
        &task_id,
        "cfm-00000000000000000000000000000000",
        &digest,
    );
    let bad_reference_stderr = String::from_utf8_lossy(&bad_reference.stderr);
    assert!(
        !bad_reference.status.success(),
        "wrong reference must fail; stderr={bad_reference_stderr}"
    );
    assert!(
        bad_reference_stderr.contains("confirmation_unavailable"),
        "wrong reference must surface confirmation_unavailable; stderr={bad_reference_stderr}"
    );
    assert_eq!(
        state_of(),
        "pending",
        "state must stay pending after wrong reference"
    );

    // Different loop (same reference + digest) → scope mismatch, stays pending.
    let wrong_loop = spawn_confirm(temp_path, "loop-other", &task_id, &reference, &digest);
    let wrong_loop_stderr = String::from_utf8_lossy(&wrong_loop.stderr);
    assert!(
        !wrong_loop.status.success(),
        "different loop must fail; stderr={wrong_loop_stderr}"
    );
    assert!(
        wrong_loop_stderr.contains("confirmation_mismatch"),
        "different loop must surface confirmation_mismatch; stderr={wrong_loop_stderr}"
    );
    assert_eq!(
        state_of(),
        "pending",
        "state must stay pending after wrong loop"
    );

    // The correct confirm still works afterwards (state was never consumed).
    let ok = spawn_confirm(temp_path, "loop-u1", &task_id, &reference, &digest);
    assert!(
        ok.status.success(),
        "correct confirm must still succeed after mismatches; stderr={}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert_eq!(
        state_of(),
        "confirmed",
        "correct confirm transitions to confirmed"
    );
}

/// S4: legacy JSONL rows without a `confirmation` field must keep
/// listing/showing normally, never parse as confirmed, and never block
/// a new protected mutation.
#[test]
fn test_task_confirmation_legacy_rows_do_not_block() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    // Seed a legacy row (no confirmation field) directly into the store.
    let store_path = temp_path.join(".ralph/agent/tasks.jsonl");
    std::fs::create_dir_all(store_path.parent().expect("store parent")).expect("create dir");
    let legacy_line = r#"{"id":"task-1000-legacy","title":"Legacy row","status":"open","priority":3,"created":"2026-01-01T00:00:00Z","loop_id":"loop-u1"}"#;
    std::fs::write(&store_path, format!("{legacy_line}\n")).expect("seed legacy row");

    // Legacy row lists and shows normally; confirmation is absent (never
    // parsed as confirmed).
    let tasks = list_tasks(temp_path, &["--all"]);
    assert_eq!(tasks.len(), 1, "legacy row must list; tasks={tasks:?}");
    assert!(
        tasks[0].confirmation.is_none(),
        "legacy row must deserialize without confirmation"
    );
    let show = ralph_task_ok(temp_path, &["show", "task-1000-legacy", "--format", "json"]);
    let shown: serde_json::Value = serde_json::from_str(&show).expect("show JSON parses");
    assert!(
        shown.get("confirmation").is_none(),
        "legacy row must not grow a confirmation via show; show={show}"
    );

    // A new protected mutation passes the pending gate despite the
    // legacy row sharing the loop.
    let verify = spawn_verify_add(temp_path, "Fresh mutation");
    assert!(
        verify.status.success(),
        "verify must succeed alongside legacy row; stderr={}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let apply = spawn_apply_add(temp_path, "Fresh mutation");
    assert!(
        apply.status.success(),
        "legacy row must not block a new protected mutation; stderr={}",
        String::from_utf8_lossy(&apply.stderr)
    );

    // The rewritten store keeps the legacy row confirmation-free.
    let raw = std::fs::read_to_string(&store_path).expect("read tasks.jsonl");
    let mut legacy_confirmations = 0usize;
    let mut rows = 0usize;
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        rows += 1;
        let v: serde_json::Value = serde_json::from_str(line).expect("row parses");
        if v.get("id").and_then(|i| i.as_str()) == Some("task-1000-legacy") {
            assert!(
                v.get("confirmation").is_none(),
                "legacy row must stay confirmation-free after rewrite; line={line}"
            );
            legacy_confirmations += 1;
        }
    }
    assert_eq!(rows, 2, "legacy row + fresh task expected; raw={raw}");
    assert_eq!(
        legacy_confirmations, 1,
        "legacy row must survive the rewrite"
    );
}

/// Compatibility contract: the three gate bypass paths (human CLI,
/// gate off, unsafe hatch) must behave exactly as before — no
/// confirmation is ever recorded and no pending gate applies.
#[test]
fn test_task_confirmation_bypass_paths_do_not_record_confirmation() {
    // ── 1. Human CLI under a gate-enabled preset ──────────────────
    let human_dir = TempDir::new().expect("temp dir");
    let human_path = human_dir.path();
    write_agent_gate_preset(human_path);

    // Humans bypass the gate entirely: add without any verify.
    ralph_task_ok(human_path, &["add", "Human task"]);
    ralph_task_ok(human_path, &["add", "Second human task"]);
    let raw = std::fs::read_to_string(human_path.join(".ralph/agent/tasks.jsonl"))
        .expect("read tasks.jsonl");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "both human adds must land; raw={raw}");
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("row parses");
        assert!(
            v.get("confirmation").is_none(),
            "human CLI rows must never carry a confirmation; line={line}"
        );
    }

    // ── 2. Agent with the gate OFF (no verify needed) ─────────────
    let gate_off_dir = TempDir::new().expect("temp dir");
    let gate_off_path = gate_off_dir.path();
    std::fs::create_dir_all(gate_off_path.join(".ralph")).unwrap();
    std::fs::write(
        gate_off_path.join("ralph.yml"),
        r#"
tasks:
  enabled: true
  require_verify_for_cli_mutate: false
  allow_unsafe_task_mutate: false
  coordinator_hats:
    - coordinator
event_loop:
  execution_mode: isolated
"#,
    )
    .unwrap();
    let gate_off_apply = spawn_apply_add(gate_off_path, "Gate-off task");
    assert!(
        gate_off_apply.status.success(),
        "gate-off agent add must succeed without verify; stderr={}",
        String::from_utf8_lossy(&gate_off_apply.stderr)
    );
    let raw = std::fs::read_to_string(gate_off_path.join(".ralph/agent/tasks.jsonl"))
        .expect("read tasks.jsonl");
    let v: serde_json::Value =
        serde_json::from_str(raw.lines().next().expect("one row")).expect("row parses");
    assert!(
        v.get("confirmation").is_none(),
        "gate-off rows must not carry a confirmation; raw={raw}"
    );

    // ── 3. Agent with the unsafe escape hatch ──────────────────────
    let unsafe_dir = TempDir::new().expect("temp dir");
    let unsafe_path = unsafe_dir.path();
    std::fs::create_dir_all(unsafe_path.join(".ralph")).unwrap();
    std::fs::write(
        unsafe_path.join("ralph.yml"),
        r#"
tasks:
  enabled: true
  require_verify_for_cli_mutate: true
  allow_unsafe_task_mutate: true
  coordinator_hats:
    - coordinator
event_loop:
  execution_mode: isolated
"#,
    )
    .unwrap();
    let unsafe_apply = spawn_apply_add(unsafe_path, "Unsafe hatch task");
    assert!(
        unsafe_apply.status.success(),
        "unsafe-hatch agent add must succeed without verify; stderr={}",
        String::from_utf8_lossy(&unsafe_apply.stderr)
    );
    let raw = std::fs::read_to_string(unsafe_path.join(".ralph/agent/tasks.jsonl"))
        .expect("read tasks.jsonl");
    let v: serde_json::Value =
        serde_json::from_str(raw.lines().next().expect("one row")).expect("row parses");
    assert!(
        v.get("confirmation").is_none(),
        "unsafe-hatch rows must not carry a confirmation; raw={raw}"
    );
}

/// S1: a successful protected Apply must print a task JSON carrying a
/// fresh, unique confirmation reference in state `pending`, and
/// `tasks.jsonl` must hold exactly one business row whose confirmation
/// fields match the printed ones.
#[test]
fn test_task_confirmation_apply_emits_pending_reference() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    let verify = spawn_verify_add(temp_path, "Confirmation target");
    assert!(
        verify.status.success(),
        "verify must succeed before Apply; stderr={}",
        String::from_utf8_lossy(&verify.stderr)
    );

    let apply = spawn_apply_add_with_args(temp_path, "Confirmation target", &["--format", "json"]);
    let apply_stderr = String::from_utf8_lossy(&apply.stderr);
    assert!(
        apply.status.success(),
        "apply must succeed; stderr={apply_stderr}"
    );

    // The Apply stdout JSON must carry a non-empty pending confirmation.
    let stdout = String::from_utf8_lossy(&apply.stdout);
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("apply stdout must be task JSON");
    let cfm = value
        .get("confirmation")
        .expect("task JSON must carry a confirmation field");
    let reference = cfm
        .get("reference")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        !reference.is_empty(),
        "confirmation.reference must be non-empty; stdout={stdout}"
    );
    assert_eq!(
        cfm.get("state").and_then(|v| v.as_str()),
        Some("pending"),
        "confirmation.state must be 'pending' right after Apply; stdout={stdout}"
    );

    // tasks.jsonl must hold exactly one business row carrying the same
    // confirmation (written in the same atomic save as the task row).
    let raw = std::fs::read_to_string(temp_path.join(".ralph/agent/tasks.jsonl"))
        .expect("tasks.jsonl must exist after Apply");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one business row expected; tasks.jsonl={raw}"
    );
    let row: serde_json::Value = serde_json::from_str(lines[0]).expect("row parses as JSON");
    let row_cfm = row
        .get("confirmation")
        .expect("jsonl row must carry confirmation");
    assert_eq!(
        row_cfm.get("reference").and_then(|v| v.as_str()),
        Some(reference),
        "jsonl confirmation.reference must match the printed one; row={lines:?}"
    );
    assert_eq!(
        row_cfm.get("state").and_then(|v| v.as_str()),
        Some("pending"),
        "jsonl confirmation.state must be pending; row={lines:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Unit 1 follow-up (cross-scope confirmation hardening): the pending
// record is owned by the loop/hat that recorded it. Cross-scope mint
// overwrites and cross-scope confirms are rejected with stable tokens.
// ─────────────────────────────────────────────────────────────────────────

/// Confirm against a task id that does not exist in the store must
/// exit non-zero with the stable `confirmation_unavailable` token.
#[test]
fn test_task_confirm_unknown_task_id_is_unavailable() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    let out = spawn_task_as(
        temp_path,
        "coordinator",
        "loop-u1",
        &[
            "confirm",
            "task-9999999999-ffff",
            "--reference",
            "cfm-does-not-exist",
            "--digest",
            "digest-x",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "confirm of an unknown task id must exit non-zero; stderr={stderr}"
    );
    assert!(
        stderr.contains("confirmation_unavailable"),
        "unknown task id must surface confirmation_unavailable; stderr={stderr}"
    );
}

/// Confirm against a human/legacy row that carries no confirmation
/// record must exit non-zero with `confirmation_unavailable` — never
/// invent a transition on a confirmation-free row.
#[test]
fn test_task_confirm_row_without_confirmation_is_unavailable() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    // Seed a legacy row (no confirmation field) directly into the store.
    let store_path = temp_path.join(".ralph/agent/tasks.jsonl");
    std::fs::create_dir_all(store_path.parent().expect("store parent")).expect("create dir");
    let legacy_line = r#"{"id":"task-1000-legacy","title":"Legacy row","status":"open","priority":3,"created":"2026-01-01T00:00:00Z","loop_id":"loop-u1"}"#;
    std::fs::write(&store_path, format!("{legacy_line}\n")).expect("seed legacy row");

    let out = spawn_task_as(
        temp_path,
        "coordinator",
        "loop-u1",
        &[
            "confirm",
            "task-1000-legacy",
            "--reference",
            "cfm-any",
            "--digest",
            "digest-any",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "confirm of a confirmation-free row must exit non-zero; stderr={stderr}"
    );
    assert!(
        stderr.contains("confirmation_unavailable"),
        "confirmation-free row must surface confirmation_unavailable; stderr={stderr}"
    );
}

/// Ensure-path symmetric of S1/S2: a gate-active `task ensure` mints a
/// pending confirmation (reference/digest non-empty, scope stamped) on
/// the ensured row; `task confirm` from the same loop/hat transitions
/// it to `confirmed`.
#[test]
fn test_task_confirmation_ensure_path_mints_and_confirms() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset(temp_path);

    let key = "scope:ensure-1";
    let verify = spawn_task_as(
        temp_path,
        "coordinator",
        "loop-u1",
        &["verify", "ensure", "Ensure me", "--key", key],
    );
    assert!(
        verify.status.success(),
        "verify ensure must succeed; stderr={}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let apply = spawn_task_as(
        temp_path,
        "coordinator",
        "loop-u1",
        &["ensure", "Ensure me", "--key", key, "--format", "json"],
    );
    let apply_stderr = String::from_utf8_lossy(&apply.stderr);
    assert!(
        apply.status.success(),
        "apply ensure must succeed; stderr={apply_stderr}"
    );

    // The Apply stdout JSON carries a non-empty pending confirmation.
    let stdout = String::from_utf8_lossy(&apply.stdout);
    let applied: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("apply stdout must be task JSON");
    let cfm = applied
        .get("confirmation")
        .expect("ensure JSON must carry confirmation");
    assert_eq!(
        cfm.get("state").and_then(|v| v.as_str()),
        Some("pending"),
        "ensure confirmation must be pending right after Apply; stdout={stdout}"
    );
    let reference = cfm
        .get("reference")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let digest = cfm
        .get("digest")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        !reference.is_empty(),
        "reference must be non-empty; stdout={stdout}"
    );
    assert!(
        !digest.is_empty(),
        "digest must be non-empty; stdout={stdout}"
    );

    // The disk row matches the printed confirmation and is scoped to
    // the recording loop/hat.
    let row = row_by_key(temp_path, key);
    let row_cfm = row
        .get("confirmation")
        .expect("jsonl row must carry confirmation");
    assert_eq!(
        row_cfm.get("reference").and_then(|v| v.as_str()),
        Some(reference.as_str()),
        "jsonl confirmation.reference must match the printed one"
    );
    let (state, cfm_loop, cfm_hat) = confirmation_scope(row_cfm);
    assert_eq!(state, "pending");
    assert_eq!(cfm_loop, "loop-u1");
    assert_eq!(cfm_hat, "coordinator");

    // Confirm from the same loop/hat transitions to confirmed.
    let task_id = row
        .get("id")
        .and_then(|v| v.as_str())
        .expect("row id")
        .to_string();
    let confirm = spawn_task_as(
        temp_path,
        "coordinator",
        "loop-u1",
        &[
            "confirm",
            &task_id,
            "--reference",
            &reference,
            "--digest",
            &digest,
        ],
    );
    assert!(
        confirm.status.success(),
        "same-scope confirm must succeed; stderr={}",
        String::from_utf8_lossy(&confirm.stderr)
    );
    let row = row_by_key(temp_path, key);
    let (state, _, _) = confirmation_scope(
        row.get("confirmation")
            .expect("jsonl row must carry confirmation"),
    );
    assert_eq!(state, "confirmed", "confirm must transition the ensure row");
}

/// Cross-scope overwrite hole: hat A's pending confirmation on a keyed
/// row must block hat B (same loop) from minting over it via
/// `ensure` — denial carries `confirmation_scope_conflict`, the row
/// keeps scope-A pending, B's prepared ticket survives for retry, and
/// after A consumes its confirmation B's retry mints a fresh scope-B
/// pending record.
#[test]
fn test_task_confirmation_cross_scope_ensure_overwrite_is_rejected() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset_with_hats(temp_path, &["coordinator", "reviewer"]);

    let key = "cross:scope-1";

    // Hat A (coordinator) verify + ensure → pending scope A.
    let verify_a = spawn_task_as(
        temp_path,
        "coordinator",
        "loop-x",
        &["verify", "ensure", "Cross scope", "--key", key],
    );
    assert!(
        verify_a.status.success(),
        "verify A must succeed; stderr={}",
        String::from_utf8_lossy(&verify_a.stderr)
    );
    let apply_a = spawn_task_as(
        temp_path,
        "coordinator",
        "loop-x",
        &["ensure", "Cross scope", "--key", key],
    );
    assert!(
        apply_a.status.success(),
        "apply A must succeed; stderr={}",
        String::from_utf8_lossy(&apply_a.stderr)
    );
    let row = row_by_key(temp_path, key);
    let row_cfm = row
        .get("confirmation")
        .expect("row must carry confirmation");
    let (state, cfm_loop, cfm_hat) = confirmation_scope(row_cfm);
    assert_eq!(
        (state.as_str(), cfm_loop.as_str(), cfm_hat.as_str()),
        ("pending", "loop-x", "coordinator")
    );
    let task_id = row
        .get("id")
        .and_then(|v| v.as_str())
        .expect("row id")
        .to_string();
    let reference_a = row_cfm
        .get("reference")
        .and_then(|v| v.as_str())
        .expect("reference")
        .to_string();
    let digest_a = row_cfm
        .get("digest")
        .and_then(|v| v.as_str())
        .expect("digest")
        .to_string();

    // Hat B (reviewer) verifies its own intent (scoped ticket) fine...
    let verify_b = spawn_task_as(
        temp_path,
        "reviewer",
        "loop-x",
        &["verify", "ensure", "Cross scope B", "--key", key],
    );
    assert!(
        verify_b.status.success(),
        "verify B must succeed; stderr={}",
        String::from_utf8_lossy(&verify_b.stderr)
    );

    // ...but the ensure itself is rejected: A's pending record is
    // owned by scope A and B must not silently release it.
    let apply_b = spawn_task_as(
        temp_path,
        "reviewer",
        "loop-x",
        &["ensure", "Cross scope B", "--key", key],
    );
    let apply_b_stderr = String::from_utf8_lossy(&apply_b.stderr);
    assert!(
        !apply_b.status.success(),
        "cross-scope ensure overwrite must exit non-zero; stderr={apply_b_stderr}"
    );
    assert!(
        apply_b_stderr.contains("confirmation_scope_conflict"),
        "denial must carry the confirmation_scope_conflict token; stderr={apply_b_stderr}"
    );
    assert!(
        apply_b_stderr.contains("ralph tools task confirm"),
        "denial must point at the recorder's confirm path; stderr={apply_b_stderr}"
    );

    // The row is untouched: still scope-A pending, same reference,
    // A's title not overwritten.
    let row = row_by_key(temp_path, key);
    assert_eq!(
        row.get("title").and_then(|v| v.as_str()),
        Some("Cross scope"),
        "rejected overwrite must not touch row metadata"
    );
    let row_cfm = row
        .get("confirmation")
        .expect("row must still carry confirmation");
    let (state, cfm_loop, cfm_hat) = confirmation_scope(row_cfm);
    assert_eq!(
        (state.as_str(), cfm_loop.as_str(), cfm_hat.as_str()),
        ("pending", "loop-x", "coordinator")
    );
    assert_eq!(
        row_cfm.get("reference").and_then(|v| v.as_str()),
        Some(reference_a.as_str()),
        "rejected overwrite must keep A's reference"
    );

    // B's prepared ticket survives the denial (restored for retry).
    let ticket_dir = temp_path.join(".ralph/agent/task-tickets");
    let entries: Vec<String> = std::fs::read_dir(&ticket_dir)
        .expect("read ticket dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let prepared = entries.iter().filter(|n| n.ends_with(".ticket")).count();
    let claimed = entries
        .iter()
        .filter(|n| n.ends_with(".ticket.claimed"))
        .count();
    assert!(
        prepared >= 1,
        "denied overwrite must preserve B's prepared ticket; entries={entries:?}"
    );
    assert_eq!(
        claimed, 0,
        "denied overwrite must not leave a claim marker; entries={entries:?}"
    );

    // Hat A consumes its own confirmation.
    let confirm_a = spawn_task_as(
        temp_path,
        "coordinator",
        "loop-x",
        &[
            "confirm",
            &task_id,
            "--reference",
            &reference_a,
            "--digest",
            &digest_a,
        ],
    );
    assert!(
        confirm_a.status.success(),
        "recorder's confirm must succeed; stderr={}",
        String::from_utf8_lossy(&confirm_a.stderr)
    );

    // Hat B retries the same payload — the restored ticket suffices
    // (no fresh verify) and mints a fresh pending scope B.
    let apply_b2 = spawn_task_as(
        temp_path,
        "reviewer",
        "loop-x",
        &["ensure", "Cross scope B", "--key", key],
    );
    assert!(
        apply_b2.status.success(),
        "post-confirm retry with restored ticket must succeed; stderr={}",
        String::from_utf8_lossy(&apply_b2.stderr)
    );
    let row = row_by_key(temp_path, key);
    let row_cfm = row
        .get("confirmation")
        .expect("row must carry B's confirmation");
    let (state, cfm_loop, cfm_hat) = confirmation_scope(row_cfm);
    assert_eq!(
        (state.as_str(), cfm_loop.as_str(), cfm_hat.as_str()),
        ("pending", "loop-x", "reviewer")
    );
    assert_ne!(
        row_cfm.get("reference").and_then(|v| v.as_str()),
        Some(reference_a.as_str()),
        "B's mint must carry a fresh reference, not A's"
    );
}

/// Cross-scope confirm against an already-Confirmed record must exit
/// non-zero with `confirmation_mismatch` (the idempotent repeat is
/// reserved for the recording loop/hat); the state stays confirmed and
/// the recorder's own idempotent repeat still exits 0.
#[test]
fn test_task_confirmation_cross_scope_confirm_on_confirmed_is_mismatch() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_agent_gate_preset_with_hats(temp_path, &["coordinator", "reviewer"]);

    // Hat A records a confirmation and consumes it.
    let verify = spawn_task_as(
        temp_path,
        "coordinator",
        "loop-x",
        &["verify", "add", "Confirmed target"],
    );
    assert!(
        verify.status.success(),
        "verify must succeed; stderr={}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let apply = spawn_task_as(
        temp_path,
        "coordinator",
        "loop-x",
        &["add", "Confirmed target", "--format", "json"],
    );
    assert!(
        apply.status.success(),
        "apply must succeed; stderr={}",
        String::from_utf8_lossy(&apply.stderr)
    );
    let (task_id, reference, digest) = confirmation_of_task_titled(temp_path, "Confirmed target");
    let confirm_a = spawn_task_as(
        temp_path,
        "coordinator",
        "loop-x",
        &[
            "confirm",
            &task_id,
            "--reference",
            &reference,
            "--digest",
            &digest,
        ],
    );
    assert!(
        confirm_a.status.success(),
        "recorder's confirm must succeed; stderr={}",
        String::from_utf8_lossy(&confirm_a.stderr)
    );

    // Hat B presents the exact reference/digest from a different hat
    // scope → mismatch, not idempotent success.
    let confirm_b = spawn_task_as(
        temp_path,
        "reviewer",
        "loop-x",
        &[
            "confirm",
            &task_id,
            "--reference",
            &reference,
            "--digest",
            &digest,
        ],
    );
    let confirm_b_stderr = String::from_utf8_lossy(&confirm_b.stderr);
    assert!(
        !confirm_b.status.success(),
        "cross-scope confirm on a confirmed record must exit non-zero; stderr={confirm_b_stderr}"
    );
    assert!(
        confirm_b_stderr.contains("confirmation_mismatch"),
        "cross-scope repeat must surface confirmation_mismatch; stderr={confirm_b_stderr}"
    );

    // The state stays confirmed; the recorder's idempotent repeat is
    // unaffected (exit 0).
    let raw = std::fs::read_to_string(temp_path.join(".ralph/agent/tasks.jsonl"))
        .expect("read tasks.jsonl");
    let state = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .find_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            if v.get("id").and_then(|i| i.as_str()) == Some(task_id.as_str()) {
                v.get("confirmation")
                    .and_then(|c| c.get("state"))
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            } else {
                None
            }
        })
        .expect("confirmed row must exist");
    assert_eq!(state, "confirmed", "state must stay confirmed");

    let confirm_again = spawn_task_as(
        temp_path,
        "coordinator",
        "loop-x",
        &[
            "confirm",
            &task_id,
            "--reference",
            &reference,
            "--digest",
            &digest,
        ],
    );
    assert!(
        confirm_again.status.success(),
        "same-scope idempotent repeat must still exit 0; stderr={}",
        String::from_utf8_lossy(&confirm_again.stderr)
    );
}
