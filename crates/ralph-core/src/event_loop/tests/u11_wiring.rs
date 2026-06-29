//! U11 (2026-06-27 mechanism foundation) wiring tests:
//! `archive_state_for_loop` must run once at loop start, called
//! from `EventLoop::with_context_and_diagnostics` before
//! `IdempotentLog::open` writes the new `loop-version.json`.
//!
//! The unit tests in `archive_version_stage/tests.rs` cover the
//! archive function itself; the integration tests here pin the
//! wiring contract:
//!
//! 1. Worktree loop reuse with a different loop_id archives the
//!    previous `.ralph/*.jsonl` files into
//!    `.ralph/archive/{old_loop_id}.{ISO8601}/`.
//! 2. Resume on the same loop_id is a no-op (no archive dir
//!    created).
//! 3. Primary loops (no loop_id) skip archive entirely.
//! 4. First run in a worktree (no `loop-version.json` yet) is a
//!    no-op (no archive dir created).
//!
//! Each test calls `EventLoop::with_context_and_diagnostics`
//! directly so the wiring is exercised without going through the
//! CLI / `EventLoop::new` chain.

use crate::config::RalphConfig;
use crate::event_loop::EventLoop;
use crate::event_loop::tests::common::init_git_workspace;
use crate::loop_context::LoopContext;
use std::path::Path;

/// Build a minimal solo-mode `RalphConfig` rooted at `workspace`.
fn solo_config(workspace: &Path) -> RalphConfig {
    let mut config = RalphConfig::default();
    config.core.workspace_root = workspace.to_path_buf();
    config
}

/// U11 happy path: when a worktree loop is created and the
/// previous `.ralph/loop-version.json` says a different loop_id,
/// all `.jsonl` files in `.ralph/` are moved into a fresh
/// `archive/{old_loop_id}.{ISO8601}/` directory. The new loop
/// starts with a clean `.ralph/` (except for the unchanged
/// `loop-version.json`, which `IdempotentLog::open` will
/// overwrite).
#[test]
fn event_loop_new_archives_previous_loop_state_on_worktree_reuse() {
    let dir = tempfile::tempdir().unwrap();
    init_git_workspace(dir.path());

    // Lay down the prior loop's state in `.ralph/`.
    let ralph_dir = dir.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    std::fs::write(
        ralph_dir.join("loop-version.json"),
        r#"{"loop_id":"loop-old","version":3}"#,
    )
    .unwrap();
    std::fs::write(ralph_dir.join("tasks.jsonl"), "{}\n").unwrap();
    std::fs::write(ralph_dir.join("recovery.jsonl"), "{}\n").unwrap();

    // Construct a worktree loop with a NEW loop_id.
    let config = solo_config(dir.path());
    let ctx = LoopContext::worktree(
        "loop-new",
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    let _event_loop = EventLoop::with_context(config, ctx);

    // Pin: both .jsonl files were moved out of `.ralph/` into
    // a fresh archive subdir whose name starts with
    // `loop-old.`.
    assert!(
        !ralph_dir.join("tasks.jsonl").exists(),
        "U11: tasks.jsonl should have been moved to archive"
    );
    assert!(
        !ralph_dir.join("recovery.jsonl").exists(),
        "U11: recovery.jsonl should have been moved to archive"
    );
    assert!(
        ralph_dir.join("loop-version.json").exists(),
        "U11: loop-version.json must remain in place (IdempotentLog overwrites it)"
    );

    let archive_root = ralph_dir.join("archive");
    assert!(
        archive_root.is_dir(),
        "U11: archive dir must exist after reuse: {}",
        archive_root.display()
    );
    let mut archive_entries: Vec<_> = std::fs::read_dir(&archive_root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("loop-old."))
        .collect();
    assert_eq!(
        archive_entries.len(),
        1,
        "U11: exactly one archive subdir starting with 'loop-old.'"
    );
    let archived = archive_entries.pop().unwrap().path();
    assert!(
        archived.join("tasks.jsonl").exists(),
        "U11: archived tasks.jsonl missing"
    );
    assert!(
        archived.join("recovery.jsonl").exists(),
        "U11: archived recovery.jsonl missing"
    );
}

/// U11 resume case: when a worktree loop is created and the
/// persisted `loop_id` equals the new `loop_id`, archive must be
/// a no-op. Files stay in place.
#[test]
fn event_loop_new_is_noop_when_loop_id_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    init_git_workspace(dir.path());

    let ralph_dir = dir.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    std::fs::write(
        ralph_dir.join("loop-version.json"),
        r#"{"loop_id":"loop-same","version":7}"#,
    )
    .unwrap();
    std::fs::write(ralph_dir.join("tasks.jsonl"), "{}\n").unwrap();

    let config = solo_config(dir.path());
    let ctx = LoopContext::worktree(
        "loop-same",
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    let _event_loop = EventLoop::with_context(config, ctx);

    // Pin: tasks.jsonl stayed in place; no archive dir created.
    assert!(
        ralph_dir.join("tasks.jsonl").exists(),
        "U11: tasks.jsonl must remain in place on resume"
    );
    assert!(
        !ralph_dir.join("archive").exists(),
        "U11: no archive dir on resume (loop_id unchanged)"
    );
}

/// U11 primary-loop case: `LoopContext::primary` has `loop_id =
/// None`, so archive must not run. The wiring guard
/// (`context.loop_id()`) is the gate; this test pins it.
#[test]
fn event_loop_new_skips_archive_for_primary_loop() {
    let dir = tempfile::tempdir().unwrap();
    init_git_workspace(dir.path());

    // Drop a `loop-version.json` with a non-empty loop_id so we
    // can detect any accidental archive attempt.
    let ralph_dir = dir.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    std::fs::write(
        ralph_dir.join("loop-version.json"),
        r#"{"loop_id":"should-not-be-touched","version":1}"#,
    )
    .unwrap();
    std::fs::write(ralph_dir.join("tasks.jsonl"), "{}\n").unwrap();

    let config = solo_config(dir.path());
    let ctx = LoopContext::primary(dir.path().to_path_buf());
    let _event_loop = EventLoop::with_context(config, ctx);

    // Pin: nothing was archived, and the persisted loop_id
    // remained unchanged (primary loops never archive).
    assert!(
        ralph_dir.join("tasks.jsonl").exists(),
        "U11: primary loop must not archive"
    );
    assert!(
        !ralph_dir.join("archive").exists(),
        "U11: primary loop must not create an archive dir"
    );
    let raw = std::fs::read_to_string(ralph_dir.join("loop-version.json")).unwrap();
    assert!(
        raw.contains("should-not-be-touched"),
        "U11: primary loop must not rewrite loop-version.json via archive path"
    );
}

/// U11 first-run case: a brand-new worktree (no
/// `loop-version.json`) must not create an archive dir. The
/// archive function returns `Ok(None)` and the wiring is a
/// debug-level no-op.
#[test]
fn event_loop_new_handles_first_run_in_worktree() {
    let dir = tempfile::tempdir().unwrap();
    init_git_workspace(dir.path());

    // No `.ralph/` at all — fresh worktree.
    let config = solo_config(dir.path());
    let ctx = LoopContext::worktree(
        "loop-first",
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
    );
    let _event_loop = EventLoop::with_context(config, ctx);

    // Pin: no archive dir created anywhere under the worktree.
    assert!(
        !dir.path().join(".ralph").join("archive").exists(),
        "U11: first run in worktree must not create an archive dir"
    );
}
