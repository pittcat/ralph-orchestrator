//! Integration tests for agent reference CLI commands.
//!
//! Tests cover the actual `ralph` binary behavior for:
//!   - `ralph tools skill list` and `load` for agent-reference builtin skills
//!   - `ralph emit` with JSON payload
//!   - Agent context (RALPH_CURRENT_HAT) vs human CLI context
//!
//! These tests use CARGO_BIN_EXE_ralph, which requires building the binary first.
//! They're in the cli-serial group (see .config/nextest.toml).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

/// Run `ralph tools skill` with an explicit --root for isolation.
fn ralph_skill(temp_path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ralph"))
        .arg("tools")
        .arg("skill")
        .args(args)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph tools skill command")
}

/// Run `ralph tools skill` with custom env vars for agent context simulation.
fn ralph_skill_with_env(temp_path: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ralph"));
    cmd.arg("tools")
        .arg("skill")
        .args(args)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path);
    // Drop any RALPH_* agent env from the outer test runner
    cmd.env_remove("RALPH_CURRENT_HAT");
    cmd.env_remove("RALPH_CURRENT_LOOP_ID");
    cmd.env_remove("RALPH_EVENTS_FILE");
    cmd.env_remove("RALPH_WAVE_WORKER");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output()
        .expect("Failed to execute ralph tools skill command")
}

/// Run `ralph emit` in an isolated temp workspace.
fn _ralph_emit(temp_path: &Path, args: &[&str]) -> Output {
    // Create .ralph/ directory and events marker so emit has a target
    let ralph_dir = temp_path.join(".ralph");
    fs::create_dir_all(&ralph_dir).expect("create .ralph dir");
    let events_path = ralph_dir.join("events.jsonl");
    // Touch events file so it exists
    fs::write(&events_path, "").expect("create events file");

    // Create current-events marker pointing to our events file
    fs::write(ralph_dir.join("current-events"), ".ralph/events.jsonl")
        .expect("write current-events marker");

    Command::new(env!("CARGO_BIN_EXE_ralph"))
        .arg("emit")
        .args(args)
        .arg("--root")
        .arg(temp_path)
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit command")
}

fn ralph_skill_ok(temp_path: &Path, args: &[&str]) -> String {
    let output = ralph_skill(temp_path, args);
    assert!(
        output.status.success(),
        "Command 'ralph tools skill {}' failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

// ---- Agent reference builtin skills ----

#[test]
fn test_agent_reference_skill_list_contains_emit() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let stdout = ralph_skill_ok(temp_path, &["list", "--format", "quiet"]);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        lines.contains(&"ralph-tools-emit"),
        "skill list must contain ralph-tools-emit; got: {stdout}"
    );
    assert!(
        lines.contains(&"ralph-tools-wave"),
        "skill list must contain ralph-tools-wave; got: {stdout}"
    );
    assert!(
        lines.contains(&"ralph-tools-cmdref"),
        "skill list must contain ralph-tools-cmdref; got: {stdout}"
    );
}

#[test]
fn test_agent_reference_skill_load_emit_shows_error_recovery() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let stdout = ralph_skill_ok(temp_path, &["load", "ralph-tools-emit"]);
    // Must contain the error recovery table from ralph-tools-emit.md
    assert!(
        stdout.contains("Invalid JSON payload"),
        "ralph-tools-emit must contain 'Invalid JSON payload'; got: {stdout}"
    );
    // Must mention event file resolution priority
    assert!(
        stdout.contains("事件文件解析优先级") || stdout.contains("events file not in allowlist"),
        "ralph-tools-emit must mention event file resolution; got: {stdout}"
    );
}

#[test]
fn test_agent_reference_skill_load_cmdref_shows_interact_ref() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let stdout = ralph_skill_ok(temp_path, &["load", "ralph-tools-cmdref"]);
    // Must mention ralph tools interact progress reference
    assert!(
        stdout.contains("ralph tools interact progress") || stdout.contains("ralph tools interact"),
        "ralph-tools-cmdref must mention interact command; got: {stdout}"
    );
}

#[test]
fn test_agent_reference_skill_load_all_three_refs_works() {
    // Load all three skill refs individually in a clean workspace
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    for name in &["ralph-tools-emit", "ralph-tools-wave", "ralph-tools-cmdref"] {
        let output = ralph_skill(temp_path, &["load", name]);
        assert!(
            output.status.success(),
            "skill load {} failed: {}",
            name,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output.stdout.is_empty(),
            "skill load {} produced empty output",
            name
        );
    }
}

// ---- ralph emit integration ----

#[test]
fn test_agent_reference_emit_writes_event_file() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    let ralph_dir = temp_path.join(".ralph");
    let events_path = ralph_dir.join("events.jsonl");

    // Set up marker so ralph emit knows where to write
    fs::create_dir_all(&ralph_dir).expect("create .ralph dir");
    fs::write(&events_path, "").expect("create events file");
    fs::write(ralph_dir.join("current-events"), ".ralph/events.jsonl")
        .expect("write current-events marker");

    let output = Command::new(env!("CARGO_BIN_EXE_ralph"))
        .arg("emit")
        .arg("build.done")
        .arg(r#"{"ok":true}"#)
        .arg("-j")
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit");

    assert!(
        output.status.success(),
        "ralph emit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the event was written to the events file
    let content = fs::read_to_string(&events_path).expect("read events file");
    assert!(
        content.contains("build.done"),
        "events file must contain 'build.done'; got: {content}"
    );
    assert!(
        content.contains("ok") && content.contains("true"),
        "events file must contain JSON payload; got: {content}"
    );
}

// ---- Agent context enforcement ----

#[test]
fn test_same_agent_context_without_hat_fails_closed() {
    // Agent context (RALPH_EVENTS_FILE set) but no RALPH_CURRENT_HAT
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let output = ralph_skill_with_env(
        temp_path,
        &[("RALPH_EVENTS_FILE", "/tmp/test-events.jsonl")],
        &["load", "ralph-tools-emit"],
    );
    assert!(
        !output.status.success(),
        "agent without RALPH_CURRENT_HAT must fail closed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("RALPH_CURRENT_HAT") || stderr.contains("not visible"),
        "stderr should mention RALPH_CURRENT_HAT or visibility; got: {stderr}"
    );
}

#[test]
fn test_human_cli_context_can_load_skill_without_hat() {
    // Human CLI context: no env vars → should work for list/load
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let output = ralph_skill(temp_path, &["load", "ralph-tools-emit"]);
    assert!(
        output.status.success(),
        "human CLI must load builtin skills without RALPH_CURRENT_HAT; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---- Smoke: skill list shows fewer when hat-specific ----

#[test]
fn test_agent_reference_hat_filter_does_not_block_builtins() {
    // Builtin skills have no hat restriction, so agent with ANY hat should see them
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let output = ralph_skill_with_env(
        temp_path,
        &[("RALPH_CURRENT_HAT", "executor")],
        &["list", "--format", "quiet"],
    );
    assert!(
        output.status.success(),
        "agent with executor hat must list skills; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Builtin skills (no hat filter) should be visible
    assert!(
        stdout.lines().any(|l| l == "ralph-tools-emit"),
        "executor hat must see ralph-tools-emit; got: {stdout}"
    );
}

#[test]
fn test_agent_reference_skill_load_ralph_tools_contains_task_resume_anchor() {
    // plan 004 U1 / AE0 / SC1: ralph-tools (auto-injected) must carry
    // the R0 anchor strings (收到 task.resume 时, required_fields,
    // --policy-check) and must NOT steer agents toward --unsafe-no-policy-check
    // as a recommended fix.
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let stdout = ralph_skill_ok(temp_path, &["load", "ralph-tools"]);
    assert!(
        stdout.contains("收到 `task.resume` 时"),
        "ralph-tools must include the R0 section header '收到 task.resume 时'"
    );
    assert!(
        stdout.contains("required_fields"),
        "ralph-tools R0 must mention required_fields"
    );
    assert!(
        stdout.contains("--policy-check"),
        "ralph-tools R0 must mention --policy-check"
    );
    assert!(
        !stdout.contains("确认配置允许 `--unsafe-no-policy-check`"),
        "ralph-tools R0b fix: must NOT recommend --unsafe-no-policy-check as a default recovery"
    );
}
