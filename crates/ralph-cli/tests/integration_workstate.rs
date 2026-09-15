//! Integration tests for `ralph tools workstate` CLI commands.
//!
//! Workstate is a loop-scoped key-value store: agent context reads/writes
//! the current loop's scope, the human CLI operates on the loop-less scope.

mod common;

use anyhow::Result;
use tempfile::TempDir;

// ─────────────────────────────────────────────────────────────────────────────
// Helper Functions
// ─────────────────────────────────────────────────────────────────────────────

/// Run ralph tools workstate command with given args in the temp directory.
fn ralph_workstate(temp_path: &std::path::Path, args: &[&str]) -> std::process::Output {
    common::ralph_bin()
        .arg("tools")
        .arg("workstate")
        .args(args)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph command")
}

/// Run ralph tools workstate command and assert success.
fn ralph_workstate_ok(temp_path: &std::path::Path, args: &[&str]) -> String {
    let output = ralph_workstate(temp_path, args);
    assert!(
        output.status.success(),
        "Command 'ralph tools workstate {}' failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Run ralph tools workstate as an agent of the given loop (HARD RULE 5:
/// scrub first via common::ralph_bin, then overlay the agent env explicitly).
fn ralph_workstate_as_agent(
    temp_path: &std::path::Path,
    loop_id: &str,
    args: &[&str],
) -> std::process::Output {
    common::ralph_bin()
        .arg("tools")
        .arg("workstate")
        .args(args)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path)
        .env("RALPH_CURRENT_HAT", "executor")
        .env("RALPH_CURRENT_LOOP_ID", loop_id)
        .output()
        .expect("Failed to execute ralph command")
}

fn workstate_path(temp_path: &std::path::Path) -> std::path::PathBuf {
    temp_path.join(".ralph/agent/workstate.jsonl")
}

// ─────────────────────────────────────────────────────────────────────────────
// Set / Get / List / Delete
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn workstate_cli_set_get_roundtrip() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    let set_out = ralph_workstate(temp_path, &["set", "draft-conclusion", "方案A待验证"]);
    assert!(
        set_out.status.success(),
        "set failed: {}",
        String::from_utf8_lossy(&set_out.stderr)
    );
    assert!(
        workstate_path(temp_path).exists(),
        "workstate.jsonl should be created"
    );

    let stdout = ralph_workstate_ok(temp_path, &["get", "draft-conclusion"]);
    assert_eq!(stdout.trim_end(), "方案A待验证");

    let list = ralph_workstate_ok(temp_path, &["list"]);
    assert!(
        list.lines()
            .any(|line| line.starts_with("draft-conclusion\t")),
        "list should contain 'draft-conclusion<TAB>updated_at': {list}"
    );

    let del = ralph_workstate(temp_path, &["delete", "draft-conclusion"]);
    assert!(
        del.status.success(),
        "delete failed: {}",
        String::from_utf8_lossy(&del.stderr)
    );

    let get_after = ralph_workstate(temp_path, &["get", "draft-conclusion"]);
    assert!(!get_after.status.success(), "get after delete should fail");

    Ok(())
}

#[test]
fn workstate_cli_upsert_last_write_wins() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    ralph_workstate_ok(temp_path, &["set", "k", "first"]);
    ralph_workstate_ok(temp_path, &["set", "k", "second"]);

    let stdout = ralph_workstate_ok(temp_path, &["get", "k"]);
    assert_eq!(stdout.trim_end(), "second");

    let list = ralph_workstate_ok(temp_path, &["list"]);
    assert_eq!(
        list.lines().filter(|l| l.starts_with("k\t")).count(),
        1,
        "upserted key should appear exactly once in list: {list}"
    );

    Ok(())
}

#[test]
fn workstate_cli_cross_loop_invisible() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // loop L1 (agent context) writes a key.
    let set = ralph_workstate_as_agent(temp_path, "loop-1", &["set", "draft-conclusion", "v1"]);
    assert!(
        set.status.success(),
        "agent set failed: {}",
        String::from_utf8_lossy(&set.stderr)
    );

    // loop L1 sees it.
    let l1 = ralph_workstate_as_agent(temp_path, "loop-1", &["list"]);
    assert!(l1.status.success());
    assert!(String::from_utf8_lossy(&l1.stdout).contains("draft-conclusion"));

    // loop L2 does not.
    let l2 = ralph_workstate_as_agent(temp_path, "loop-2", &["list"]);
    assert!(l2.status.success());
    assert!(
        !String::from_utf8_lossy(&l2.stdout).contains("draft-conclusion"),
        "loop-2 must not see loop-1 entries: {}",
        String::from_utf8_lossy(&l2.stdout)
    );

    // Human (loop-less) scope does not either.
    let human = ralph_workstate_ok(temp_path, &["list"]);
    assert!(
        !human.contains("draft-conclusion"),
        "human scope must not see loop-1 entries: {human}"
    );

    Ok(())
}

#[test]
fn workstate_cli_human_can_select_a_loop_scope() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    for (loop_id, key, value) in [
        ("loop-1", "draft-conclusion", "first"),
        ("loop-2", "other-conclusion", "second"),
    ] {
        let set = ralph_workstate_as_agent(temp_path, loop_id, &["set", key, value]);
        assert!(
            set.status.success(),
            "agent set failed: {}",
            String::from_utf8_lossy(&set.stderr)
        );
    }

    let default_scope = ralph_workstate_ok(temp_path, &["list"]);
    assert!(
        default_scope.is_empty(),
        "default human scope stays loop-less"
    );

    let selected_scope = ralph_workstate_ok(temp_path, &["list", "--loop-id", "loop-1"]);
    assert!(selected_scope.contains("draft-conclusion"));
    assert!(!selected_scope.contains("other-conclusion"));

    ralph_workstate_ok(
        temp_path,
        &["set", "draft-conclusion", "updated", "--loop-id", "loop-1"],
    );
    let updated = ralph_workstate_ok(
        temp_path,
        &["get", "draft-conclusion", "--loop-id", "loop-1"],
    );
    assert_eq!(updated.trim_end(), "updated");

    Ok(())
}

#[test]
fn workstate_cli_agent_cannot_select_a_loop_scope() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let output =
        ralph_workstate_as_agent(temp_dir.path(), "loop-1", &["list", "--loop-id", "loop-2"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot select a loop id"));
    Ok(())
}

#[test]
fn workstate_cli_rejects_invalid_input() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    let cases: Vec<Vec<String>> = vec![
        vec!["set".into(), "".into(), "v".into()],
        vec!["set".into(), "with space".into(), "v".into()],
        vec!["set".into(), "with\ttab".into(), "v".into()],
        vec!["set".into(), "k".into(), "x".repeat(10_001)],
    ];

    for args in &cases {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = ralph_workstate(temp_path, &arg_refs);
        assert!(
            !output.status.success(),
            "invalid input {args:?} should be rejected"
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).is_empty(),
            "stderr should explain the rejection for {args:?}"
        );
    }

    assert!(
        !workstate_path(temp_path).exists(),
        "no lines should be written for rejected input"
    );

    Ok(())
}

#[test]
fn workstate_cli_agent_context_requires_loop_id() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path();

    // Agent context (hat set) without any loop id must fail closed.
    let output = common::ralph_bin()
        .arg("tools")
        .arg("workstate")
        .args(["set", "k", "v"])
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path)
        .env("RALPH_CURRENT_HAT", "executor")
        .output()?;

    assert!(
        !output.status.success(),
        "agent context without loop id must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("loop"),
        "stderr should mention the missing loop id: {stderr}"
    );
    assert!(!workstate_path(temp_path).exists());

    Ok(())
}
