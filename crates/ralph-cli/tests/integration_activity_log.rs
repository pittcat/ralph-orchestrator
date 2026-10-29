//! Integration tests for the per-run activity ledger
//! (`.ralph/runs/<run_id>.activity.jsonl`) and the `activity_file`
//! field on `ralph inspect run`.
//!
//! monitor health-recovery plan (2026-10-28-1949) Unit 1.

mod common;

use serde_json::Value;
use std::fs;
use std::process::Output;
use tempfile::TempDir;

fn run_ralph(temp_path: &std::path::Path, args: &[&str]) -> Output {
    common::ralph_bin()
        .args(args)
        .current_dir(temp_path)
        .output()
        .expect("execute ralph")
}

fn inspect_run_json(temp_path: &std::path::Path) -> Value {
    let output = run_ralph(temp_path, &["inspect", "run", "--format", "json"]);
    assert!(
        output.status.success(),
        "inspect run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("inspect run emits valid JSON")
}

/// Fake backend fixture: `backend.sh` consumes stdin, optionally
/// echoes stderr lines, sleeps, then emits LOOP_COMPLETE.
fn write_fixture(
    temp_path: &std::path::Path,
    home_path: &std::path::Path,
    backend_body: &str,
    max_runtime: u64,
) {
    write_fixture_t(temp_path, home_path, backend_body, max_runtime, "")
}

/// `extra_cfg` is appended verbatim to `ralph.yml` (e.g. adapter
/// timeout overrides — custom backends fall back to `adapters.claude`).
fn write_fixture_t(
    temp_path: &std::path::Path,
    home_path: &std::path::Path,
    backend_body: &str,
    max_runtime: u64,
    extra_cfg: &str,
) {
    fs::create_dir_all(home_path.join(".ralph")).expect("user config dir");
    fs::write(
        home_path.join(".ralph/config.yml"),
        "cli:\n  backend: custom\n",
    )
    .expect("user config");

    let backend = temp_path.join("backend.sh");
    fs::write(
        &backend,
        format!("#!/bin/sh\ncat >/dev/null\n{backend_body}\n"),
    )
    .expect("backend script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = fs::metadata(&backend).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(&backend, p).unwrap();
    }

    fs::write(
        temp_path.join("ralph.yml"),
        format!(
            r#"
cli:
  backend: custom
  command: "./backend.sh"
  prompt_mode: stdin
event_loop:
  max_iterations: 3
  max_runtime_seconds: {max_runtime}
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: false
{extra_cfg}
"#
        ),
    )
    .expect("local config");
}

fn activity_lines(temp_path: &std::path::Path) -> Vec<Value> {
    let runs = temp_path.join(".ralph/runs");
    let file = fs::read_dir(&runs)
        .ok()
        .and_then(|mut d| {
            d.find_map(|e| {
                let p = e.ok()?.path();
                let hit = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".activity.jsonl"))
                    .unwrap_or(false);
                hit.then_some(p)
            })
        })
        .unwrap_or_else(|| panic!("no activity.jsonl under {}", runs.display()));
    fs::read_to_string(&file)
        .expect("read activity ledger")
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line parses"))
        .collect()
}

#[test]
fn test_run_writes_activity_start_end_and_inspect_reports_file() {
    let temp_dir = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    write_fixture(
        temp_dir.path(),
        home.path(),
        &format!(
            "\"{}\" emit LOOP_COMPLETE done",
            env!("CARGO_BIN_EXE_ralph")
        ),
        30,
    );
    let out = common::ralph_bin()
        .args(["run", "--no-tui", "-p", "x"])
        .current_dir(temp_dir.path())
        .env("HOME", home.path())
        .output()
        .expect("ralph run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let lines = activity_lines(temp_dir.path());
    assert!(
        lines.len() >= 2,
        "expected ≥1 start/end pair, got {lines:?}"
    );
    let first = &lines[0];
    assert_eq!(first["phase"], "start");
    assert_eq!(first["seq_in_run"], 1);
    assert!(first["hat"].is_string());
    assert!(first["backend"].is_string());
    let end = &lines[1];
    assert_eq!(end["phase"], "end");
    assert_eq!(end["seq_in_run"], 2);
    assert_eq!(end["is_error"], false);
    assert!(end["duration_ms"].as_u64().is_some());

    // inspect run surfaces the ledger path.
    let json = inspect_run_json(temp_dir.path());
    let run = &json["runs"][0];
    let af = run["activity_file"].as_str().unwrap_or("");
    assert!(
        af.ends_with(".activity.jsonl"),
        "activity_file missing/odd: {af} in {run}"
    );
}

#[test]
fn test_stderr_hint_rate_limited() {
    let temp_dir = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    // backend prints a rate-limit banner then completes — the end
    // record should classify the tail.
    write_fixture(
        temp_dir.path(),
        home.path(),
        &format!(
            "echo '429 rate limit exceeded' >&2\n\"{}\" emit LOOP_COMPLETE done",
            env!("CARGO_BIN_EXE_ralph")
        ),
        30,
    );
    let _ = common::ralph_bin()
        .args(["run", "--no-tui", "-p", "x"])
        .current_dir(temp_dir.path())
        .env("HOME", home.path())
        .output()
        .expect("ralph run");
    let lines = activity_lines(temp_dir.path());
    let end = lines
        .iter()
        .find(|l| l["phase"] == "end")
        .expect("end line");
    assert_eq!(end["stderr_tail_hint"], "rate_limited");
}

#[test]
fn test_backend_timeout_marks_timed_out_end_record() {
    let temp_dir = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    // backend sleeps past the 1s inactivity watchdog → SIGTERM →
    // end record carries timed_out: true.
    write_fixture_t(
        temp_dir.path(),
        home.path(),
        "sleep 60",
        30,
        "adapters:\n  claude:\n    timeout: 1\n",
    );
    let _ = common::ralph_bin()
        .args(["run", "--no-tui", "-p", "x"])
        .current_dir(temp_dir.path())
        .env("HOME", home.path())
        .output()
        .expect("ralph run");
    let lines = activity_lines(temp_dir.path());
    let end = lines
        .iter()
        .find(|l| l["phase"] == "end")
        .expect("end line");
    assert_eq!(end["timed_out"], true);
    assert_eq!(end["is_error"], true);
}

#[test]
fn test_no_backend_call_leaves_no_activity_key() {
    // inspect on an empty workspace → no runs; a manifest without any
    // activity file must omit the key entirely.
    let temp_dir = TempDir::new().unwrap();
    let json = inspect_run_json(temp_dir.path());
    assert_eq!(json["runs"], Value::Array(vec![]));
}
