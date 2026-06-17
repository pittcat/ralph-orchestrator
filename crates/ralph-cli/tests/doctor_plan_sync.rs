//! Integration tests for `ralph doctor plan-sync` (U5 / R7).

use std::process::Command;
use tempfile::TempDir;

fn ralph_doctor_plan_sync(temp_path: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args(args)
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph doctor plan-sync")
}

fn write_plan(dir: &std::path::Path, name: &str, status: &str, body: &str) -> std::path::PathBuf {
    let plan = dir.join(format!("{name}.md"));
    std::fs::write(
        &plan,
        format!("---\ntitle: t\nstatus: {status}\n---\n{body}\n"),
    )
    .expect("write plan");
    plan
}

fn write_tasks(dir: &std::path::Path, lines: &[&str]) -> std::path::PathBuf {
    let ralph = dir.join(".ralph").join("agent");
    std::fs::create_dir_all(&ralph).expect("mkdir ralph");
    let tasks = ralph.join("tasks.jsonl");
    std::fs::write(&tasks, lines.join("\n") + "\n").expect("write tasks");
    tasks
}

#[test]
fn t5_1_stalled_frontmatter_with_closed_task_fails() {
    let dir = TempDir::new().expect("temp dir");
    let plan_path = write_plan(dir.path(), "test-plan", "stalled-after-u1", "# body");
    let _tasks = write_tasks(
        dir.path(),
        &[
            r#"{"id":"t1","title":"u1 work","status":"closed","key":"ce-executor:test-plan:step-01:u1-impl","created":"2026-06-17T00:00:00Z"}"#,
        ],
    );
    // Note: doctor auto-discovers the newest plan under docs/plans/ or
    // docs/achieved/plan/, so we pass --plan explicitly here.
    let _ = plan_path; // path recorded for clarity
    let explicit = dir.path().join("test-plan.md");

    let output = ralph_doctor_plan_sync(
        dir.path(),
        &["doctor", "plan-sync", "--plan", explicit.to_str().unwrap()],
    );

    assert!(
        !output.status.success(),
        "expected failure, got success: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("FAIL") || stdout.contains("drift"),
        "stdout should mention drift detection: {}",
        stdout
    );
}

#[test]
fn t5_2_consistent_frontmatter_and_tasks_exit_zero() {
    let dir = TempDir::new().expect("temp dir");
    write_plan(dir.path(), "ok-plan", "active", "# body");
    write_tasks(
        dir.path(),
        &[
            r#"{"id":"t1","title":"u1 work","status":"open","key":"ce-executor:ok-plan:step-01:u1-impl","created":"2026-06-17T00:00:00Z"}"#,
        ],
    );
    let explicit = dir.path().join("ok-plan.md");

    let output = ralph_doctor_plan_sync(
        dir.path(),
        &["doctor", "plan-sync", "--plan", explicit.to_str().unwrap()],
    );

    assert!(
        output.status.success(),
        "expected success, got: stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("PASS"),
        "expected PASS in stdout: {}",
        stdout
    );
}

#[test]
fn t5_3_missing_plan_file_exits_nonzero_with_clear_error() {
    let dir = TempDir::new().expect("temp dir");
    let missing = dir.path().join("nonexistent.md");

    let output = ralph_doctor_plan_sync(
        dir.path(),
        &["doctor", "plan-sync", "--plan", missing.to_str().unwrap()],
    );

    assert!(!output.status.success(), "expected failure on missing plan");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("not found") || combined.contains("plan-sync error"),
        "expected clear error: {}",
        combined
    );
}

#[test]
fn t5_4_missing_tasks_jsonl_warns_but_does_not_crash() {
    let dir = TempDir::new().expect("temp dir");
    let plan = write_plan(dir.path(), "no-tasks", "active", "# body");
    // Deliberately do NOT create .ralph/agent/tasks.jsonl

    let output = ralph_doctor_plan_sync(
        dir.path(),
        &["doctor", "plan-sync", "--plan", plan.to_str().unwrap()],
    );

    // T5.4: warn does not crash; exit code is 0 (warn path returns Ok).
    assert!(
        output.status.success(),
        "expected success (warn) but got failure: stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("WARN") || stdout.contains("skipped") || stdout.contains("missing"),
        "expected WARN/missing indicator: {}",
        stdout
    );
}
