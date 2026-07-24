//! 2026-07-22-001 plan U9: end-to-end BDD scenarios for the
//! default-path wave protocol suite.
//!
//! This file is the U9 delivery side of the plan's
//! `Verification Contract`. It exercises the
//! `wave_verify_gate` ticket gate, the lazy supervisor-store
//! bridge, and the public-only Confirm path through the real
//! `ralph` binary so the OPAC ticket gate is proven end-to-end
//! (not just at the unit level). All tests scrub agent-context
//! env (HARD RULE 5) before invoking `ralph` and use the
//! `common::ralph_bin()` helper so an outer hat cannot poison
//! the fixtures.

use crate::common::{ralph_bin, scrub_agent_runtime_env};
use std::io::Write;
use tempfile::TempDir;

#[path = "common/mod.rs"]
mod common;

/// Render a minimal ralph.yml under the workspace so
/// `load_policy_config_for_cli_emit` finds a known schema and
/// the wave emit path's precheck agrees with the test's payload
/// shape.
fn write_minimal_ralph_yml(workspace: &std::path::Path) {
    let yaml = r"
event_loop:
  event_policy:
    enabled: false
";
    std::fs::write(workspace.join("ralph.yml"), yaml).unwrap();
    std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
}

/// Run `ralph` against a temp workspace, capture stdout / stderr
/// / exit code. Always scrubs agent-context env so the spawned
/// binary sees a human CLI (the tests explicitly opt into agent
/// mode with `.env(...)` when they need it). When `stdin_file`
/// is `Some`, the helper wires the file as the child's stdin
/// so `--payloads-stdin` receives the supplied payload bytes.
fn run_ralph(
    workspace: &std::path::Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
    stdin_file: Option<&std::path::Path>,
) -> (i32, String, String) {
    let mut cmd = ralph_bin();
    cmd.current_dir(workspace);
    cmd.args(args);
    scrub_agent_runtime_env(&mut cmd);
    for (k, v) in extra_env {
        cmd.env(*k, *v);
    }
    if let Some(p) = stdin_file {
        let f = std::fs::File::open(p).expect("stdin payload file");
        cmd.stdin(f);
    }
    let output = cmd.output().expect("ralph invocation must succeed");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Write a payload file with the supplied JSON lines (one per line).
fn write_payloads(workspace: &std::path::Path, payloads: &[&str]) -> std::path::PathBuf {
    let path = workspace.join("payloads.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    for p in payloads {
        writeln!(f, "{}", p).unwrap();
    }
    path
}

// =============================================================================
// U9 / Scenario 1: OPAC ticket gate end-to-end.
// Agent emits without first verifying → deny + stable prefix.
// =============================================================================

#[test]
fn u9_opac_no_ticket_denies_wave_emit() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#, r#"{"dim":"b"}"#]);

    // Simulate the agent context: RALPH_CURRENT_HAT makes
    // OperationContext::detect is_agent_context = true so the
    // ticket gate engages.
    let (code, _stdout, stderr) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--output",
            "json",
        ],
        &[
            ("RALPH_CURRENT_HAT", "coordinator"),
            ("RALPH_CURRENT_LOOP_ID", "loop-u9"),
        ],
        Some(&payloads),
    );
    let stderr_combined = stderr;
    assert_ne!(code, 0, "agent emit without verify must fail");
    assert!(
        stderr_combined.contains("wave_verify_gate denied")
            || stderr_combined.contains("wave_verify_gate"),
        "expected stable deny prefix, got: {stderr_combined}"
    );
}

// =============================================================================
// U9 / Scenario 2: Verify → Emit happy path.
// Same payload file piped twice; second invocation succeeds and
// the events file receives the wave.
// =============================================================================

#[test]
fn u9_opac_verify_then_emit_succeeds_in_default_path() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#, r#"{"dim":"b"}"#]);

    let env = [
        ("RALPH_CURRENT_HAT", "coordinator"),
        ("RALPH_CURRENT_LOOP_ID", "loop-u9"),
    ];

    // 1. Verify
    let (v_code, v_stdout, _v_stderr) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &env,
        Some(&payloads),
    );
    assert_eq!(v_code, 0, "verify must succeed; stdout={v_stdout}");

    // 2. Emit (same payload file → same fingerprint)
    let (e_code, e_stdout, e_stderr) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--output",
            "json",
        ],
        &env,
        Some(&payloads),
    );
    assert_eq!(
        e_code, 0,
        "verify-then-emit must succeed; stdout={e_stdout} stderr={e_stderr}"
    );

    // 3. Confirm: events file must contain 2 records for the
    //    emitted wave_id. (The event file is the default
    //    `.ralph/events.jsonl` since we did not override
    //    RALPH_EVENTS_FILE.)
    let events_path = ws.join(".ralph/events.jsonl");
    let body = std::fs::read_to_string(&events_path).unwrap_or_default();
    let count = body.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(count, 2, "two payloads must produce two JSONL lines");
}

// =============================================================================
// U9 / Scenario 3: human CLI bypass.
// Without RALPH_CURRENT_HAT, emit works without a verify ticket.
// =============================================================================

#[test]
fn u9_human_cli_bypasses_ticket_gate() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"x"}"#]);

    // No RALPH_CURRENT_HAT → OperationContext.is_agent_context
    // is false → require_ticket returns Ok early.
    let (code, _stdout, _stderr) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--output",
            "json",
        ],
        &[],
        Some(&payloads),
    );
    assert_eq!(code, 0, "human CLI emit must succeed without verify");
}

// =============================================================================
// U9 / Scenario 4: fingerprint mismatch.
// Verify with payload A, then emit with payload B → deny.
// =============================================================================

#[test]
fn u9_opac_fingerprint_drift_denies_emit() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payload_a = ws.join("payloads-a.jsonl");
    let payload_b = ws.join("payloads-b.jsonl");
    std::fs::write(&payload_a, "{\"dim\":\"a\"}\n").unwrap();
    std::fs::write(&payload_b, "{\"dim\":\"b\"}\n").unwrap();

    let env = [
        ("RALPH_CURRENT_HAT", "coordinator"),
        ("RALPH_CURRENT_LOOP_ID", "loop-u9"),
    ];

    // Verify with payload A
    let (v_code, _v_stdout, _v_stderr) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &env,
        Some(&payload_a),
    );
    assert_eq!(v_code, 0, "verify with payload A must succeed");

    // Emit with payload B → must deny with fingerprint mismatch
    let (e_code, _e_stdout, e_stderr) = run_ralph(
        ws,
        &["wave", "emit", "review.wave.ready", "--payloads-stdin"],
        &env,
        Some(&payload_b),
    );
    assert_ne!(e_code, 0, "drift between verify and emit must deny");
    assert!(
        e_stderr.contains("fingerprint mismatch"),
        "expected fingerprint mismatch deny, got: {e_stderr}"
    );
}
