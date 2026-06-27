//! State isolation tests (U11 follow-up to commit 9f5abcfc).
//!
//! Verifies that `archive_state_for_loop` and `IdempotentLog::open`
//! cooperate so two sequential loop runs in the same workspace
//! do NOT cross-contaminate. This is the Plan 2026-06-27 v1.1
//! `worktree_reuse_state_isolation` scenario, implemented as a
//! direct integration test because the BDD runner's
//! `process_events_from_jsonl` path does not exercise the
//! archive hook (which lives in `EventLoop::new`).
//!
//! Each test creates a fresh workspace under
//! `std::env::temp_dir()`, runs the helpers, then asserts the
//! resulting `.ralph/` (and its `archives/` subdir) shape.

use std::fs;
use std::path::{Path, PathBuf};

use ralph_core::event_loop::stages::archive_version_stage::archive_state_for_loop;
use ralph_core::state::idempotent_log::{IdempotentLog, IdempotentRecord};

fn fresh_workspace(label: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir()
        .join(format!("ralph_state_isolation_{label}_{pid}_{nanos}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create workspace");
    dir
}

fn write_loop_version(workspace: &Path, loop_id: &str, version: u64) {
    let payload = serde_json::json!({
        "loop_id": loop_id,
        "version": version,
    });
    fs::write(
        workspace.join("loop-version.json"),
        serde_json::to_string_pretty(&payload).unwrap(),
    )
    .expect("write loop-version.json");
}

fn seed_recovery(workspace: &Path, loop_id: &str, n: usize) {
    let path = workspace.join("recovery.jsonl");
    let mut buf = String::new();
    for i in 0..n {
        let rec = serde_json::json!({
            "_idempotency_key": format!("recovery:r{i}:loop:{loop_id}"),
            "_version": 1,
            "_final": true,
            "reason_code": format!("seed_reason_{i}"),
            "ts": "2024-01-01T00:00:00Z",
        });
        buf.push_str(&serde_json::to_string(&rec).unwrap());
        buf.push('\n');
    }
    fs::write(&path, buf).expect("write recovery.jsonl");
}

#[test]
fn state_isolation_archive_moves_recovery_under_new_loop_id() {
    let workspace = fresh_workspace("archive_moves");

    // First run: no loop-version.json → no archive.
    let result = archive_state_for_loop(&workspace, "loop-A").expect("first archive");
    assert!(
        result.is_none(),
        "first run with no prior loop-version.json must not archive anything"
    );

    // Pretend loop-A completed: write its loop-version + recovery.
    write_loop_version(&workspace, "loop-A", 1);
    seed_recovery(&workspace, "loop-A", 3);
    assert!(workspace.join("recovery.jsonl").exists());

    // Second run with a different loop_id → archive loop-A.
    let archived =
        archive_state_for_loop(&workspace, "loop-B").expect("second archive");
    let archive_dir = archived.expect("archive_dir returned");
    assert!(
        archive_dir.exists(),
        "archive dir must exist on disk: {archive_dir:?}"
    );
    let archived_recovery = archive_dir.join("recovery.jsonl");
    assert!(
        archived_recovery.exists(),
        "loop-A's recovery.jsonl must be moved into the archive: {archived_recovery:?}"
    );
    assert!(
        !workspace.join("recovery.jsonl").exists(),
        "root recovery.jsonl must be gone after archive"
    );
}

#[test]
fn state_isolation_same_loop_id_does_not_archive() {
    let workspace = fresh_workspace("same_loop_no_archive");

    write_loop_version(&workspace, "loop-resume", 5);
    seed_recovery(&workspace, "loop-resume", 2);

    // Same loop_id → resume case → no archive.
    let result =
        archive_state_for_loop(&workspace, "loop-resume").expect("resume archive");
    assert!(
        result.is_none(),
        "resume on the same loop_id must be a no-op"
    );
    assert!(
        workspace.join("recovery.jsonl").exists(),
        "resume must leave recovery.jsonl in place"
    );
}

#[test]
fn state_isolation_idempotent_log_opens_after_archive() {
    let workspace = fresh_workspace("idempotent_after_archive");

    // Stage loop-A state.
    write_loop_version(&workspace, "loop-A", 1);
    seed_recovery(&workspace, "loop-A", 2);

    // Archive loop-A and immediately open IdempotentLog for loop-B.
    let _ = archive_state_for_loop(&workspace, "loop-B").expect("archive loop-A");
    let mut log =
        IdempotentLog::open(&workspace, "loop-B").expect("IdempotentLog::open for loop-B");

    // Disabled-stub short-circuit must NOT fire because we opened
    // with a real workspace path. We assert by observing that
    // loop_id() / version() return real values rather than the
    // disabled-stub defaults ("", 0).
    assert_eq!(log.loop_id(), "loop-B", "loop_id must reflect open() arg");
    assert!(
        log.version() >= 2,
        "version must be bumped past loop-A's persisted version (1); got {}",
        log.version()
    );

    // Append a non-final record.
    let key = "recovery:r-new:loop:loop-B";
    log.append(
        IdempotentRecord::new(key).with_payload(serde_json::json!({"seed": true})),
    )
    .expect("append non-final record");
    assert_eq!(
        log.final_count(),
        0,
        "no _final=true record yet; final_count must be 0"
    );

    // Finalise the same key.
    log.append(
        IdempotentRecord::new(key)
            .with_payload(serde_json::json!({"seed": "final"}))
            .with_final(true),
    )
    .expect("append final record");
    assert_eq!(
        log.final_count(),
        1,
        "after final append, final_count must reflect the single _final=true record"
    );
}

#[test]
fn state_isolation_archive_rejects_relative_workspace() {
    // The archive helper refuses to operate on relative paths
    // because the moved `.jsonl` files would otherwise land in
    // an unpredictable directory. Pass a synthetic relative
    // path and assert the call returns an error.
    let relative = Path::new("relative-workspace-for-archive-test");
    let result = archive_state_for_loop(relative, "loop-anything");
    assert!(
        result.is_err(),
        "relative workspace must be rejected by archive_state_for_loop"
    );
}