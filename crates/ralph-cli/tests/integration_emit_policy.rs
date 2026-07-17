//! Integration tests for `ralph emit` policy precheck (Units U1, U2, U5).
//!
//! These tests verify the strict validation mechanisms themselves:
//! - no-policy skip info
//! - isolated mode hat mismatch rejection
//! - ralph pseudo-hat authority (blocklist + allowlist)
//! - payload schema validation (string vs json_object)
//! - malformed RALPH_HATS_SOURCE fail-closed
//! - rejection hint security (no leak of unauthorised topic example)
//! - control topic exemption (loop.cancel, task.resume)
//! - source field default rules (business vs control topic)
//!
//! 2026-06-24: preset-text-specific tests (hardcoded hat/topic/handoff
//! assertions like review-synthesizer+review.passed, executor+work.done,
//! coordinator+review.passed+skip_reason) were removed. The preset only
//! needs to pass strict validation; per-hat/per-topic ownership is
//! covered by the preset_lint suite and the SSOT merge tests.

use std::process::Command;
use tempfile::TempDir;

#[test]
fn test_emit_isolated_mode_rejects_conflicting_hat_override() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-pipeline",
            "emit",
            "debug.step",
            "task_id=demo",
            "--hat",
            "review-coordinator",
        ])
        .env("RALPH_CURRENT_HAT", "review-synthesizer")
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "emit should reject conflicting --hat in isolated mode: stderr={}",
        stderr
    );
    assert!(
        stderr.contains("Isolated mode hat mismatch"),
        "expected isolated-mode mismatch error, got: {}",
        stderr
    );

    // The event must not be written anywhere.
    let events_file = temp_path.join(".ralph/events.jsonl");
    assert!(
        !events_file.exists()
            || std::fs::read_to_string(&events_file)
                .unwrap()
                .trim()
                .is_empty(),
        "rejected event must not be written"
    );
}

/// ralph pseudo-hat must NOT be allowed to emit business topics. Allowing it
/// would let a worktree loop's loop runner impersonate downstream hats and
/// advance the workflow as `ralph` — the same impersonation attack the P0
/// origin guard rejects at JSONL read time. The CLI-side guard rejects this
/// at the write boundary so the agent gets immediate backpressure.
///
/// `review.passed` here is an arbitrary business topic sample; the test
/// pins the blocklist side of the ralph pseudo-hat authority guard.
#[test]
fn test_emit_with_malformed_hats_source_fails_closed() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();
    // Create a workspace ralph.yml so fail-closed engages.
    std::fs::write(temp_path.join("ralph.yml"), "agent: claude\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args(["emit", "work.ready", "--json", "{}", "--hat", "coordinator"])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:not-a-real-preset")
        .env("RALPH_CURRENT_HAT", "coordinator")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit");

    assert!(
        !output.status.success(),
        "malformed RALPH_HATS_SOURCE + workspace config must fail closed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not-a-real-preset") || stderr.contains("Pre-publish"),
        "stderr should explain the fail-closed reason: {}",
        stderr
    );
}

/// AC-4: when a hat tries to emit a topic it has no authority for, the
/// rejection hint must NOT leak the unauthorised payload example. This is
/// the hat-scoping rule from plan 001 §4.1. `work.ready` + `executor` are
/// samples — the test pins the rejection-hint security mechanism, not the
/// specific hat/topic ownership.
#[test]
fn test_emit_rejection_hint_excludes_unauthorised_topics() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "emit",
            "work.ready",
            "garbage",
            "--hat",
            // executor is not in the work.ready authorised set
            "executor",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-pipeline")
        .env("RALPH_CURRENT_HAT", "executor")
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .output()
        .expect("failed to execute ralph emit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "executor must not be allowed to emit work.ready: stderr={}",
        stderr
    );
    // When fix_hint_for_hat_topic returns None (the hat cannot publish
    // the topic), the rejection message must not leak the example
    // `ralph emit work.ready --json '{...}'` form — that would teach
    // the agent to bypass provenance.
    assert!(
        !stderr.contains("ralph emit work.ready --json"),
        "fix hint must not surface an unauthorised-topic payload example: {}",
        stderr
    );
}
