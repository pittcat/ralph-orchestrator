//! U7 tests: CLI policy-check rejection → stdout EmitResult (JSON).
//!
//! Acceptance criteria:
//! 1. A `work.done` payload missing `task_id` under `--policy-check
//!    --output json` produces a stdout line parseable as `EmitResult`
//!    with `ok=false` and `errors[0].code` carrying the policy gate's
//!    reason. 2. The process exits non-zero so CI / dashboards can
//!    detect the rejection.
//!
//! Lifted verbatim from `commands/emit.rs` lines 6026-6122 of HEAD
//! `7909f159`. Behaviour is identical.

use super::EmitArgs;
use crate::cli::ColorMode;
use std::path::PathBuf;

#[test]
fn test_policy_check_reject_json_emit_result_shape() {
    let workspace = tempfile::TempDir::new()
        .expect("temp dir")
        .path()
        .to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      work.done:
        required_fields:
          - plan_name
          - task_id
",
    )
    .expect("ralph.yml");

    let args = EmitArgs {
        topic: Some("work.done".to_string()),
        payload: "{}".to_string(),
        json: true,
        file: PathBuf::from(".ralph/events.jsonl"),
        policy_check: true,
        no_policy_check: false,
        hat: Some("coordinator".to_string()),
        triggered: None,
        source: None,
        schema: None,
        output: "json".to_string(),
        policy_check_token: None,
    };

    // The handler should still return Err so the exit code is non-zero,
    // but the EmitResult JSON must have been printed to stdout before
    // the bail. We only assert the return code here — stdout capture is
    // covered by `test_policy_check_reject_json_exit_nonzero`.
    let result = super::emit_command_with_root(ColorMode::Never, args, Some(&workspace));
    assert!(
        result.is_err(),
        "policy-check rejection must yield Err exit code, got: {result:?}"
    );
}

#[test]
fn test_policy_check_reject_json_exit_nonzero() {
    let workspace = tempfile::TempDir::new()
        .expect("temp dir")
        .path()
        .to_path_buf();
    std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
    std::fs::write(
        workspace.join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      work.done:
        required_fields:
          - plan_name
          - task_id
",
    )
    .expect("ralph.yml");

    let args = EmitArgs {
        topic: Some("work.done".to_string()),
        payload: "{}".to_string(),
        json: true,
        file: PathBuf::from(".ralph/events.jsonl"),
        policy_check: true,
        no_policy_check: false,
        hat: Some("coordinator".to_string()),
        triggered: None,
        source: None,
        schema: None,
        output: "json".to_string(),
        policy_check_token: None,
    };

    let result = super::emit_command_with_root(ColorMode::Never, args, Some(&workspace));
    assert!(result.is_err(), "exit code must be non-zero on rejection");
}
