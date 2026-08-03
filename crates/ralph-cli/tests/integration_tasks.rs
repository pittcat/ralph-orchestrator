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
        oks, 1,
        "exactly one Apply must win: a.success={} b.success={}; a.stderr={} b.stderr={}",
        result_a.status.success(),
        result_b.status.success(),
        String::from_utf8_lossy(&result_a.stderr),
        String::from_utf8_lossy(&result_b.stderr)
    );
    assert_eq!(
        denials, 1,
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
    assert!(
        ticket_dir.is_dir(),
        "scoped ticket directory must exist"
    );
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
    let verify_add =
        spawn_verify(temp_path, "add", &["Scoped add target"]);
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
        apply_add_stderr.contains("task_verify_gate denied") == false,
        "apply add must not be denied; stderr={}",
        apply_add_stderr
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
