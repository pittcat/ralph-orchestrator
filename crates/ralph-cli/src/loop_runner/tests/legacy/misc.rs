// Auto-extracted from the legacy loop-runner regression suite. Tests in this
// module remain part of the loop_runner::tests::legacy surface; only the file
// layout changed (mechanical split per plan 2026-08-07-005). Behavior,
// assertions, fixtures, and process environment semantics are unchanged.
//
// The full original `legacy.rs` import set is reproduced verbatim per bucket so
// that every existing test compiles without rewriting call sites. Splits may
// leave some imports unused in a given bucket; this is a mechanical artifact,
// not dead code (the same items remain used by sibling buckets).

#![allow(unused_imports)]

use super::super::super::*;
use super::super::common::*;
use super::super::fake_path::*;
use super::helpers::*;
use crate::test_support::CwdGuard;
use ralph_core::HatRegistry;
use ralph_core::planning_session::{ConversationEntry, ConversationType};
use ralph_proto::{Hat, Topic};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::sync::{Arc, Mutex};

// Test: test_resolve_loop_id_fresh_generates_new
#[test]
fn test_resolve_loop_id_fresh_generates_new() {
    let temp = tempfile::TempDir::new().unwrap();
    let ctx = ralph_core::LoopContext::primary(temp.path().to_path_buf());
    ctx.ensure_ralph_dir().unwrap();

    let id = resolve_loop_id(&ctx, false, None);
    assert!(
        id.starts_with("primary-"),
        "Fresh run should generate primary-{{timestamp}}, got: {}",
        id
    );
}

// Test: test_resolve_loop_id_continue_reuses_marker
#[test]
fn test_resolve_loop_id_continue_reuses_marker() {
    let temp = tempfile::TempDir::new().unwrap();
    let ctx = ralph_core::LoopContext::primary(temp.path().to_path_buf());
    ctx.ensure_ralph_dir().unwrap();

    // Write a marker from a "previous run"
    std::fs::write(
        ctx.ralph_dir().join("current-loop-id"),
        "primary-20260303-100000",
    )
    .unwrap();

    let id = resolve_loop_id(&ctx, true, None);
    assert_eq!(
        id, "primary-20260303-100000",
        "--continue should reuse existing loop ID"
    );
}

// Test: test_resolve_loop_id_continue_explicit_overrides_marker
#[test]
fn test_resolve_loop_id_continue_explicit_overrides_marker() {
    let temp = tempfile::TempDir::new().unwrap();
    let ctx = ralph_core::LoopContext::primary(temp.path().to_path_buf());
    ctx.ensure_ralph_dir().unwrap();

    std::fs::write(
        ctx.ralph_dir().join("current-loop-id"),
        "primary-20260303-100000",
    )
    .unwrap();

    let id = resolve_loop_id(&ctx, true, Some("custom-loop-42"));
    assert_eq!(
        id, "custom-loop-42",
        "--loop-id should override the marker file"
    );
}

// Test: test_resolve_loop_id_continue_no_marker_generates_new
#[test]
fn test_resolve_loop_id_continue_no_marker_generates_new() {
    let temp = tempfile::TempDir::new().unwrap();
    let ctx = ralph_core::LoopContext::primary(temp.path().to_path_buf());
    ctx.ensure_ralph_dir().unwrap();

    // No marker file exists
    let id = resolve_loop_id(&ctx, true, None);
    assert!(
        id.starts_with("primary-"),
        "--continue without marker should fall back to generating new ID, got: {}",
        id
    );
}

// Test: test_get_last_commit_info_returns_none_without_git
#[cfg(unix)]
#[test]
fn test_get_last_commit_info_returns_none_without_git() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let _cwd = CwdGuard::set(temp_dir.path());
    let missing_git = temp_dir.path().join("git");
    assert!(get_last_commit_info_with_cmd(missing_git.as_os_str()).is_none());
}

// Test: test_get_last_commit_info_reads_last_commit
#[cfg(unix)]
#[test]
fn test_get_last_commit_info_reads_last_commit() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let repo_root = temp_dir.path();

    Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo_root)
        .status()
        .expect("git init");

    std::fs::write(repo_root.join("README.md"), "hello").expect("write file");
    Command::new("git")
        .args(["add", "."])
        .current_dir(repo_root)
        .status()
        .expect("git add");

    Command::new("git")
        .args([
            "-c",
            "user.name=Test User",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "Initial commit",
            "--quiet",
        ])
        .current_dir(repo_root)
        .status()
        .expect("git commit");

    let _cwd = CwdGuard::set(repo_root);
    let info = get_last_commit_info_with_cmd(OsStr::new("git")).expect("commit info");
    assert!(
        info.contains("Initial commit"),
        "unexpected commit info: {info}"
    );
}

// Test: test_u5_r5_last_reviewed_sha_written_when_wave_fully_closed_and_passed
#[test]
fn test_u5_r5_last_reviewed_sha_written_when_wave_fully_closed_and_passed() {
    use ralph_core::Event as JsonlEvent;
    use ralph_core::event_loop::review_step_state::ReviewStepTracker;

    // Happy path: wave fully closed + review.passed → SHA write allowed.
    let mut tracker = ReviewStepTracker::default();

    let wave = JsonlEvent {
        topic: "review.wave.ready".to_string(),
        payload: Some(
            r#"{"plan_name":"u5-plan","task_id":"t1","task_key":"k1","step":"1","dimension":"sec"}"#
                .to_string(),
        ),
        ts: String::new(),
        hat: Some("review-coordinator".to_string()),
        triggered: None,
        source: None,
        wave_id: Some("w-1".to_string()),
        wave_index: None,
        wave_total: Some(2),
        system_injected: None,
    };
    tracker.observe_accepted(&wave);

    // All dimensions received.
    for dim in ["sec", "rel"] {
        let mut d = wave.clone();
        d.topic = "review.dimension.done".to_string();
        d.hat = Some("dimension-reviewer".to_string());
        d.payload = Some(format!(
            r#"{{"plan_name":"u5-plan","task_id":"t1","task_key":"k1","step":"1","dimension":"{dim}","findings_count":0,"findings_file":"f.json"}}"#
        ));
        tracker.observe_accepted(&d);
    }

    // Verdict terminal.
    let passed = JsonlEvent {
        topic: "review.passed".to_string(),
        payload: Some(
            r#"{"plan_name":"u5-plan","task_id":"t1","task_key":"k1","step":"1","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#
                .to_string(),
        ),
        ts: String::new(),
        hat: Some("review-synthesizer".to_string()),
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    tracker.observe_accepted(&passed);

    assert!(
        tracker.is_wave_closed("u5-plan", "t1", "1"),
        "U5: happy path — wave fully closed + verdict seen → SHA write allowed"
    );
}

// Test: test_u5_r5_last_reviewed_sha_blocked_when_wave_open_4_of_11
#[test]
fn test_u5_r5_last_reviewed_sha_blocked_when_wave_open_4_of_11() {
    use ralph_core::Event as JsonlEvent;
    use ralph_core::event_loop::review_step_state::ReviewStepTracker;

    // Error path: wave ready + only 4/11 dimensions → SHA write MUST be blocked.
    // This is the zippy-sparrow stall scenario: a premature SHA would let
    // DEC-002 empty_diff claim an empty review when in fact 7 dimensions
    // never received.
    let mut tracker = ReviewStepTracker::default();

    let wave = JsonlEvent {
        topic: "review.wave.ready".to_string(),
        payload: Some(
            r#"{"plan_name":"zippy-plan","task_id":"t-4of11","task_key":"k-4of11","step":"1","dimension":"sec"}"#
                .to_string(),
        ),
        ts: String::new(),
        hat: Some("review-coordinator".to_string()),
        triggered: None,
        source: None,
        wave_id: Some("w-stall".to_string()),
        wave_index: None,
        wave_total: Some(11),
        system_injected: None,
    };
    tracker.observe_accepted(&wave);

    // Only 4 unique dimensions received.
    for dim in ["sec", "rel", "perf", "a11y"] {
        let mut d = wave.clone();
        d.topic = "review.dimension.done".to_string();
        d.hat = Some("dimension-reviewer".to_string());
        d.payload = Some(format!(
            r#"{{"plan_name":"zippy-plan","task_id":"t-4of11","task_key":"k-4of11","step":"1","dimension":"{dim}","findings_count":0,"findings_file":"f.json"}}"#
        ));
        tracker.observe_accepted(&d);
    }

    assert!(
        !tracker.is_wave_closed("zippy-plan", "t-4of11", "1"),
        "U5: error path — 4/11 dimensions, wave open → SHA write MUST be blocked \
         (this kills DEC-002 empty_diff fuel)"
    );
}

// Test: test_u5_r5_last_reviewed_sha_written_for_real_empty_diff
#[test]
fn test_u5_r5_last_reviewed_sha_written_for_real_empty_diff() {
    use ralph_core::event_loop::review_step_state::ReviewStepTracker;

    // Regression: real empty diff (no wave, no commit, just verdict)
    // → SHA write is safe. The `is_wave_closed` gate returns true for
    // steps with no tracker entry, which is the correct behavior for
    // empty_diff fast-path (the DEC-002 attack vector is only when a
    // wave IS open but verdict is being emitted prematurely).
    let tracker = ReviewStepTracker::default();
    assert!(
        tracker.is_wave_closed("u5-plan", "never-touched", "1"),
        "U5: regression — step with no wave ever opened, empty_diff is safe"
    );
}
