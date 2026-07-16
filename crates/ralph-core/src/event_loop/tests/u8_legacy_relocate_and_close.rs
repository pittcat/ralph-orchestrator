//! U8 (2026-06-27 mechanism foundation completion):
//! `EventLoop::with_context_and_diagnostics` invokes
//! `relocate_legacy_tasks` on every start so legacy
//! `tasks.jsonl` records (with `loop_id == null`)
//! inherit the active loop id. The corresponding
//! `LoopState::on_repair_close` clears the per-task
//! `stall_recovery_counts` entry when `repair.close`
//! arrives.
//!
//! Pinned contracts:
//! 1. Two legacy + one with loop_id → backfill 2 on
//!    first start.
//! 2. `repair.close(task_key=k)` clears
//!    `stall_recovery_counts["stall:k"]` and returns
//!    `true`; second call returns `false`.
//! 3. Missing `tasks.jsonl` does not panic.
//! 4. Idempotency: a second start with the same
//!    `loop_id` backfills 0.

use super::*;
use crate::event_loop::legacy_task_relocate::relocate_legacy_tasks;
use std::io::Write;

fn write_jsonl_tasks(path: &std::path::Path, lines: &[&str]) {
    let mut f = std::fs::File::create(path).expect("create tasks.jsonl");
    for line in lines {
        writeln!(f, "{line}").unwrap();
    }
}

#[test]
fn u8_relocate_legacy_tasks_backfills_two_records() {
    let temp = tempfile::tempdir().unwrap();
    let tasks_path = temp.path().join("tasks.jsonl");
    write_jsonl_tasks(
        &tasks_path,
        &[
            r#"{"task_key":"legacy-1","loop_id":null,"status":"open"}"#,
            r#"{"task_key":"legacy-2","loop_id":"","status":"open"}"#,
            r#"{"task_key":"task-with-id","loop_id":"existing","status":"open"}"#,
        ],
    );
    let backfilled = relocate_legacy_tasks(&tasks_path, "loop-u8-1").expect("relocate succeeds");
    assert_eq!(backfilled, 2, "two legacy records must be backfilled");
}

#[test]
fn u8_relocate_legacy_tasks_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let tasks_path = temp.path().join("tasks.jsonl");
    write_jsonl_tasks(
        &tasks_path,
        &[
            r#"{"task_key":"legacy-1","loop_id":null,"status":"open"}"#,
            r#"{"task_key":"legacy-2","loop_id":"","status":"open"}"#,
        ],
    );
    let first = relocate_legacy_tasks(&tasks_path, "loop-u8-2").unwrap();
    assert_eq!(first, 2);
    let second = relocate_legacy_tasks(&tasks_path, "loop-u8-2").unwrap();
    assert_eq!(second, 0, "second start with same loop_id must backfill 0");
}

#[test]
fn u8_relocate_legacy_tasks_missing_file_returns_err() {
    let temp = tempfile::tempdir().unwrap();
    let tasks_path = temp.path().join("nonexistent.jsonl");
    let result = relocate_legacy_tasks(&tasks_path, "loop-u8-3");
    // The current implementation surfaces a
    // `RelocateError::Io` for a missing tasks file.
    // `with_context_and_diagnostics` logs the error at
    // WARN level and continues — the loop must not
    // crash. The test pins the documented contract.
    assert!(
        result.is_err(),
        "missing tasks file must return Err so the loop start logs it"
    );
}

#[test]
fn u8_on_repair_close_clears_stall_recovery_count() {
    let mut state = LoopState::new();
    // Simulate a per-task stall counter.
    state
        .stall_recovery_counts
        .insert("stall:task-1".to_string(), 5);
    assert!(
        state.on_repair_close("task-1"),
        "first close must remove the entry"
    );
    assert!(
        !state.stall_recovery_counts.contains_key("stall:task-1"),
        "entry must be removed"
    );
    // Idempotent: second close returns false.
    assert!(
        !state.on_repair_close("task-1"),
        "second close must report no entry"
    );
}

#[test]
fn u8_on_repair_close_unknown_task_is_noop() {
    let mut state = LoopState::new();
    assert!(
        !state.on_repair_close("never-stalled"),
        "unknown task must report no entry"
    );
    assert!(state.stall_recovery_counts.is_empty());
}
