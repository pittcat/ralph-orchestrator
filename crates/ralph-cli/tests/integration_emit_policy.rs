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

mod common;

use tempfile::TempDir;

#[test]
fn test_emit_isolated_mode_rejects_conflicting_hat_override() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

    let output = common::ralph_bin()
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

    let output = common::ralph_bin()
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

    let output = common::ralph_bin()
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

/// 2026-07-27: when an outer hat leaks `RALPH_EVENTS_FILE` into a
/// human-CLI invocation, `ralph emit` rejects with
/// `path_resolution_failed` (allowlist mismatch). The error message
/// on its own does not explain that hat env leakage is the likely
/// cause — the user / agent has to guess. We emit an extra stderr
/// hint naming the leak and listing the unset command.
///
/// This test pins both behaviours:
/// - non-allowlisted `RALPH_EVENTS_FILE` is still rejected (R5
///   stdout summary stays stable).
/// - the stderr hint names `unset RALPH_*` and the
///   `scrub_agent_runtime_env()` helper.
#[test]
fn test_emit_rejected_env_events_file_prints_outer_hat_leak_hint() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();
    // `current-events` pins the allowlist to a single canonical target
    // — anything else, including the leaked env value, will be rejected.
    std::fs::write(
        temp_path.join(".ralph/current-events"),
        ".ralph/events-20260101-000000.jsonl",
    )
    .unwrap();

    let output = common::ralph_bin()
        .args(["emit", "review.unit.done", "{}", "--hat", "executor"])
        .env(
            "RALPH_EVENTS_FILE",
            temp_path.join(".ralph/events-other.jsonl"),
        )
        .current_dir(temp_path)
        .output()
        .expect("failed to execute ralph emit");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "non-allowlisted RALPH_EVENTS_FILE must be rejected: stdout={stdout} stderr={stderr}"
    );
    // R5 contract: stdout summary stays stable.
    assert!(
        stdout.contains("emit rejected [path_resolution_failed]"),
        "stdout must carry the stable reject summary: {stdout}"
    );
    // New behaviour: stderr hint names the cause and the fix.
    assert!(
        stderr.contains("hint:"),
        "stderr must carry a hint when env_events_file is the source: {stderr}"
    );
    assert!(
        stderr.contains("unset RALPH_CURRENT_HAT"),
        "hint must list the unset command: {stderr}"
    );
    assert!(
        stderr.contains("scrub_agent_runtime_env"),
        "hint must point at the helper: {stderr}"
    );
}

/// Closure for ec636dc4: runner-injected `RALPH_CONFIG=ralph.yml` plus
/// `RALPH_HATS_SOURCE` must NOT re-fire
/// `Config file "ralph.yml" not found, using defaults` on every in-loop
/// emit. The workflow comes from the hats source; missing project
/// ralph.yml is the expected default core layer.
#[test]
fn test_emit_hats_source_with_ralph_config_suppresses_missing_yml_warn() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();
    // Deliberately NO ralph.yml — mirrors hat cwd / thin worktree.

    let output = common::ralph_bin()
        .args([
            "emit",
            "debug.step",
            "task_id=demo",
            "--policy-check",
            "--hat",
            "executor",
        ])
        .current_dir(temp_path)
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-pipeline")
        .env("RALPH_CONFIG", "ralph.yml")
        .env("RALPH_CURRENT_HAT", "executor")
        .env(
            "RALPH_EVENTS_FILE",
            temp_path.join(".ralph/events.jsonl"),
        )
        .output()
        .expect("failed to execute ralph emit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("not found, using defaults"),
        "runner-injected RALPH_CONFIG + hats source must suppress missing-default warn; stderr={stderr}"
    );
}

/// `--policy-check` must reject non-allowlisted `RALPH_EVENTS_FILE`
/// the same way as formal emit (no false green then exit 1 on apply).
#[test]
fn test_emit_policy_check_rejects_non_allowlisted_events_file() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();
    std::fs::write(
        temp_path.join(".ralph/current-events"),
        ".ralph/events-20260101-000000.jsonl",
    )
    .unwrap();

    let output = common::ralph_bin()
        .args([
            "emit",
            "review.unit.done",
            "{}",
            "--policy-check",
            "--hat",
            "executor",
        ])
        .env(
            "RALPH_EVENTS_FILE",
            temp_path.join(".ralph/events-other.jsonl"),
        )
        .current_dir(temp_path)
        .output()
        .expect("failed to execute ralph emit --policy-check");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "--policy-check must reject non-allowlisted RALPH_EVENTS_FILE: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("emit rejected [path_resolution_failed]"),
        "stdout must carry the stable reject summary: {stdout}"
    );
    assert!(
        stderr.contains("hint:"),
        "stderr must carry the outer-hat leak hint: {stderr}"
    );
}
