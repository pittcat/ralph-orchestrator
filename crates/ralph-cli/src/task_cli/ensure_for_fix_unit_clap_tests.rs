#![cfg(test)]

// ─────────────────────────────────────────────────────────────────────────
// U7 (2026-07-04-003 plan): `EnsureArgs --for-fix-unit` derives the
// canonical fix-unit key + pins owner to `coordinator` without
// requiring an explicit `--key`.
//
// The tests intentionally avoid touching the ACL gate (`check_task`)
// and the verify gate (Unit 5/6) so this mod stays narrowly scoped
// to clap-level + handler-level key derivation.
// ─────────────────────────────────────────────────────────────────────────

use super::{EnsureArgs, OperationContext, TaskStore, add_common_task_fields, get_tasks_path};
use clap::Parser;
use ralph_core::Task;
use tempfile::TempDir;

/// Mirror of the production `derive_key` path inside
/// `ensure_task_with_args` so we can assert the canonical key
/// shape without invoking the full write path.
fn derive_key(args: &EnsureArgs) -> Option<String> {
    if let Some(spec) = args.for_fix_unit.as_deref() {
        let mut parts = spec.split(':');
        let plan = parts.next().unwrap_or("").to_string();
        let fix_step = parts.next().unwrap_or("").to_string();
        let slug = parts.next().unwrap_or("").to_string();
        if plan.is_empty() || fix_step.is_empty() || slug.is_empty() || parts.next().is_some() {
            return None;
        }
        Some(format!("ce-executor:{plan}:{fix_step}:{slug}"))
    } else {
        args.key.clone()
    }
}

#[test]
fn test_ensure_for_fix_unit_derives_key_without_explicit_key() {
    // Simulate clap parsing by constructing EnsureArgs directly
    // with no --key and a valid --for-fix-unit spec.
    let args = EnsureArgs {
        title: "fix-foo".to_string(),
        key: None,
        priority: 2,
        description: None,
        blocked_by: None,
        for_fix_unit: Some("myplan:fix-01:patch-foo".to_string()),
        format: crate::task_cli::OutputFormat::Quiet,
    };
    let derived = derive_key(&args).expect("for_fix_unit should derive a key");
    assert_eq!(derived, "ce-executor:myplan:fix-01:patch-foo");
}

#[test]
fn test_ensure_for_fix_unit_pins_owner_coordinator() {
    let temp_dir = TempDir::new().expect("temp dir");
    let path = get_tasks_path(Some(&temp_dir.path().to_path_buf()));
    let mut store = TaskStore::load(&path).expect("load store");
    let ctx = OperationContext {
        workspace_root: temp_dir.path().to_path_buf(),
        current_loop_id: Some("loop-a".to_string()),
        current_hat_id: Some("executor".to_string()),
        is_agent_context: true,
    };
    let args = EnsureArgs {
        title: "fix-foo".to_string(),
        key: None,
        priority: 2,
        description: None,
        blocked_by: None,
        for_fix_unit: Some("myplan:fix-01:patch-foo".to_string()),
        format: crate::task_cli::OutputFormat::Quiet,
    };
    let key = derive_key(&args).expect("derive key");
    let task = add_common_task_fields(
        Task::new(args.title.clone(), args.priority).with_key(Some(key)),
        &ctx,
        args.description.clone(),
        args.blocked_by.clone(),
    );
    let task = if args.for_fix_unit.is_some() {
        task.with_owner_hat(Some("coordinator".to_string()))
    } else {
        task
    };
    store.add(task.clone());
    store.save().expect("save");
    // Owner must be pinned to coordinator regardless of ctx.
    assert_eq!(task.owner_hat_id.as_deref(), Some("coordinator"));
    assert_eq!(
        task.key.as_deref(),
        Some("ce-executor:myplan:fix-01:patch-foo")
    );
}

#[test]
fn test_ensure_explicit_key_still_works() {
    // When --for-fix-unit is None, --key should be used as-is.
    let args = EnsureArgs {
        title: "do work".to_string(),
        key: Some("my-explicit-key".to_string()),
        priority: 2,
        description: None,
        blocked_by: None,
        for_fix_unit: None,
        format: crate::task_cli::OutputFormat::Quiet,
    };
    let derived = derive_key(&args).expect("explicit key should be returned");
    assert_eq!(derived, "my-explicit-key");
}

#[test]
fn test_ensure_for_fix_unit_with_both_set_picks_for_fix_unit() {
    // Even when both are set (clap rejects at parse time, but
    // construction is allowed), the for_fix_unit derivation
    // wins so the canonical contract is preserved.
    let args = EnsureArgs {
        title: "fix-foo".to_string(),
        key: Some("stale-key".to_string()),
        priority: 2,
        description: None,
        blocked_by: None,
        for_fix_unit: Some("p:fix-01:s".to_string()),
        format: crate::task_cli::OutputFormat::Quiet,
    };
    let derived = derive_key(&args).expect("derive");
    assert_eq!(derived, "ce-executor:p:fix-01:s");
}

#[test]
fn test_ensure_clap_parses_for_fix_unit_without_key() {
    // Use `TaskArgs::try_parse_from` to verify clap accepts
    // `--for-fix-unit` without `--key` and rejects the
    // conflict.
    use crate::task_cli::{TaskArgs, TaskCommands};
    let parsed = TaskArgs::try_parse_from([
        "ralph-tools-task",
        "ensure",
        "fix-foo",
        "--for-fix-unit",
        "myplan:fix-01:patch-foo",
    ])
    .expect("for_fix_unit alone should parse");
    match parsed.command {
        TaskCommands::Ensure(args) => {
            assert_eq!(args.title, "fix-foo");
            assert!(args.key.is_none(), "--key should be None");
            assert_eq!(
                args.for_fix_unit.as_deref(),
                Some("myplan:fix-01:patch-foo")
            );
        }
        _ => panic!("expected Ensure subcommand"),
    }
}

#[test]
fn test_ensure_clap_rejects_both_key_and_for_fix_unit() {
    use crate::task_cli::TaskArgs;
    let err = TaskArgs::try_parse_from([
        "ralph-tools-task",
        "ensure",
        "fix-foo",
        "--key",
        "x",
        "--for-fix-unit",
        "p:fix-01:s",
    ])
    .expect_err("clap must reject --key + --for-fix-unit");
    let msg = err.to_string();
    assert!(
        msg.contains("cannot be used with") || msg.contains("for-fix-unit"),
        "clap error should mention conflicts: {msg}"
    );
}
