//! 2026-07-24-003 plan Unit 6: ticket prepared → claimed → consumed.
//!
//! Validates the OPAC ticket state machine for `ralph wave emit`:
//!
//! - **Prepare** is the `ralph wave verify` side-effect (the
//!   existing wave_verify_gate keeps writing the on-disk ticket).
//! - **Claim** happens at the top of `ralph wave emit`: the
//!   CLI takes ownership of the ticket without deleting it.
//! - **Apply** runs the store-backed emission; on success the
//!   CLI **consumes** the ticket (deletes both the ticket and the
//!   claim marker).
//! - **Failure before Apply** (e.g. write failure, integration
//!   fault injection) restores the ticket to `prepared` so the
//!   next attempt can retry without re-running `ralph wave
//!   verify`.
//! - **Mismatch** at claim time leaves the ticket in `prepared`
//!   so a clean retry with the right payloads can succeed.
//! - **Cleanup failure** (ticket delete fails after Apply) does
//!   NOT roll back the emission; the response carries
//!   `applied_cleanup_pending: true` and points the agent at
//!   `ralph wave inspect <wave_id>`.
//!
//! Agent-context env scrubs are mandatory (HARD RULE 5).

use crate::common::{ralph_bin, scrub_agent_runtime_env};
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

#[path = "common/mod.rs"]
mod common;

/// Minimal ralph.yml: ACL allows `coordinator` hat to publish
/// `review.wave.ready`, policy is not enforcing.
fn write_minimal_ralph_yml(workspace: &Path) {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: false
    mode: enforce
hats:
  coordinator:
    name: "Coordinator"
    publishes:
      - review.wave.ready
"#;
    std::fs::write(workspace.join("ralph.yml"), yaml).unwrap();
    std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
}

fn run_ralph(
    workspace: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
    stdin_file: Option<&Path>,
) -> (i32, String, String) {
    let store_path = workspace.join(".ralph/test-store.db");
    let store_path_str = store_path.to_string_lossy().into_owned();
    let mut store_env: Vec<(&str, &str)> = vec![("RALPH_EMISSION_STORE_PATH", &store_path_str)];
    for (k, v) in extra_env.iter() {
        store_env.push((*k, *v));
    }

    let mut cmd = ralph_bin();
    cmd.current_dir(workspace);
    cmd.args(args);
    scrub_agent_runtime_env(&mut cmd);
    for (k, v) in &store_env {
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

fn write_payloads(workspace: &Path, payloads: &[&str]) -> std::path::PathBuf {
    let path = workspace.join("payloads.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    for p in payloads {
        writeln!(f, "{}", p).unwrap();
    }
    path
}

fn ticket_path(workspace: &Path) -> std::path::PathBuf {
    workspace.join(".ralph/agent/.ralph-wave-verify-ticket")
}

fn json_field<'a>(stdout: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    let start = stdout.find(&needle)? + needle.len();
    let rest = stdout[start..].trim_start();
    let bytes = rest.as_bytes();
    if bytes.first() == Some(&b'"') {
        let body = &rest[1..];
        let end = body.find('"')?;
        Some(&body[..end])
    } else {
        let end = rest
            .find(|c: char| c == ',' || c == '}')
            .unwrap_or(rest.len());
        Some(rest[..end].trim())
    }
}

// =============================================================================
// S5 + U6: Apply-before-failure leaves the ticket on disk and the
// same payload retry consumes it exactly once.
// =============================================================================

#[test]
fn u6_apply_before_failure_restores_ticket_for_retry() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#, r#"{"dim":"b"}"#]);

    let env = [
        ("RALPH_CURRENT_HAT", "coordinator"),
        ("RALPH_CURRENT_LOOP_ID", "loop-u6"),
    ];

    // 1. Verify → ticket is prepared on disk.
    let (v_code, _v_stdout, _) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &env,
        Some(&payloads),
    );
    assert_eq!(v_code, 0, "verify must succeed");
    let ticket = ticket_path(ws);
    assert!(
        ticket.exists(),
        "verify must leave the ticket in prepared state"
    );

    // 2. First emit attempt with fault injection BEFORE the apply
    //    write — the CLI must restore the ticket so the next
    //    attempt can claim it again.
    let mut env_with_inject = env.to_vec();
    env_with_inject.push(("RALPH_WAVE_EMIT_FAIL_AT", "apply_before_write"));
    let (f_code, _f_stdout, _f_err) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            "u6-restore-key",
            "--output",
            "json",
        ],
        &env_with_inject,
        Some(&payloads),
    );
    assert_ne!(f_code, 0, "injected failure must surface a non-zero exit");
    assert!(
        ticket.exists(),
        "U6 Apply-before-failure must restore the ticket (got removed)"
    );

    // 3. Retry with the SAME ticket + SAME payloads + SAME key.
    //    The retried emit must claim and consume; the ticket
    //    must end up consumed (deleted).
    let (r_code, r_stdout, _) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            "u6-restore-key",
            "--output",
            "json",
        ],
        &env,
        Some(&payloads),
    );
    assert_eq!(r_code, 0, "retry after restore must succeed; stdout={r_stdout}");
    assert!(
        !ticket.exists(),
        "successful retry must consume the ticket"
    );

    // 4. No extra events appended: the retry reuses the
    //    AlreadyApplied state, the on-disk batch is exactly 2.
    let body = std::fs::read_to_string(ws.join(".ralph/events.jsonl")).unwrap();
    let line_count = body.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        line_count, 2,
        "retry must not double-write events (got {line_count} lines)"
    );
}

// =============================================================================
// S6 + U6: identity mismatch (wrong fingerprint / topic / loop /
// hat) does NOT consume the ticket; a clean retry with matching
// inputs must succeed.
// =============================================================================

#[test]
fn u6_fingerprint_mismatch_leaves_ticket_prepared() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payload_a = ws.join("payloads-a.jsonl");
    let payload_b = ws.join("payloads-b.jsonl");
    std::fs::write(&payload_a, "{\"dim\":\"a\"}\n").unwrap();
    std::fs::write(&payload_b, "{\"dim\":\"b\"}\n").unwrap();

    let env = [
        ("RALPH_CURRENT_HAT", "coordinator"),
        ("RALPH_CURRENT_LOOP_ID", "loop-u6"),
    ];

    // Verify with payload A.
    let (v_code, _, _) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &env,
        Some(&payload_a),
    );
    assert_eq!(v_code, 0);
    let ticket = ticket_path(ws);
    assert!(ticket.exists());

    // Emit with payload B → mismatch. Ticket MUST stay prepared.
    let (e_code, _, e_err) = run_ralph(
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
        Some(&payload_b),
    );
    assert_ne!(e_code, 0, "mismatch must deny");
    assert!(
        e_err.contains("fingerprint mismatch"),
        "expected fingerprint mismatch, got: {e_err}"
    );
    assert!(
        ticket.exists(),
        "U6 mismatch must leave the ticket prepared (got consumed)"
    );

    // Re-verify with payload A → ticket refreshed.
    let (v2_code, _, _) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &env,
        Some(&payload_a),
    );
    assert_eq!(v2_code, 0);

    // Emit with payload A → succeeds, ticket consumed.
    let (e2_code, e2_stdout, _) = run_ralph(
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
        Some(&payload_a),
    );
    assert_eq!(e2_code, 0, "matching retry must succeed; stdout={e2_stdout}");
    assert!(
        !ticket.exists(),
        "successful matching emit must consume the ticket"
    );
}

// =============================================================================
// S7 + U6: cleanup (ticket delete) failure does NOT roll back the
// emission. The response carries `applied_cleanup_pending: true`
// and points at `ralph wave inspect <wave_id>`; a retry does not
// double-write events (the store's AlreadyApplied kicks in).
// =============================================================================

#[test]
fn u6_cleanup_failure_reports_applied_cleanup_pending() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#]);

    let env = [
        ("RALPH_CURRENT_HAT", "coordinator"),
        ("RALPH_CURRENT_LOOP_ID", "loop-u6"),
    ];

    // Verify → ticket prepared.
    let (v_code, _, _) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &env,
        Some(&payloads),
    );
    assert_eq!(v_code, 0);

    // Emit with cleanup fault injection. The store MUST be
    // updated to Applied even when the ticket delete fails —
    // that is the whole point of U6: failure of the *cleanup*
    // step must not produce a phantom emission.
    let mut env_with_inject = env.to_vec();
    env_with_inject.push(("RALPH_WAVE_EMIT_FAIL_AT", "cleanup_ticket"));
    let (code, stdout, stderr) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            "u6-cleanup-failure",
            "--output",
            "json",
        ],
        &env_with_inject,
        Some(&payloads),
    );
    assert_eq!(
        code, 0,
        "cleanup failure must NOT fail the emit; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(
        json_field(&stdout, "applied_cleanup_pending"),
        Some("true"),
        "U6 cleanup-failure response must surface applied_cleanup_pending: {stdout}"
    );
    assert_eq!(
        json_field(&stdout, "applied"),
        Some("true"),
        "U6 cleanup-failure response must still mark applied: {stdout}"
    );
    let wave_id = json_field(&stdout, "wave_id")
        .expect("wave_id present")
        .to_string();

    // Retry with the same key: the store's AlreadyApplied must
    // prevent any second event write.
    let body = std::fs::read_to_string(ws.join(".ralph/events.jsonl")).unwrap();
    let line_count = body.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        line_count, 1,
        "cleanup-failure path must still write the batch (got {line_count} lines)"
    );

    let (r_code, r_stdout, r_err) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--idempotency-key",
            "u6-cleanup-failure",
            "--output",
            "json",
        ],
        &env,
        Some(&payloads),
    );
    eprintln!("retry exit={r_code} stdout={r_stdout} stderr={r_err}");
    let body = std::fs::read_to_string(ws.join(".ralph/events.jsonl")).unwrap();
    let line_count_after = body.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        line_count_after, 1,
        "post-cleanup-failure retry must not append events (got {line_count_after} lines)"
    );
    assert_eq!(
        json_field(&r_stdout, "wave_id"),
        Some(wave_id.as_str()),
        "post-cleanup-failure retry must report the same wave_id; stderr={r_err}"
    );
    assert_eq!(
        json_field(&r_stdout, "deduplicated"),
        Some("true"),
        "post-cleanup-failure retry must be deduplicated: {r_stdout}"
    );
}

// =============================================================================
// Human CLI (no ticket gate) still bypasses the claim/consume
// flow — operators must not be locked out.
// =============================================================================

#[test]
fn u6_human_cli_bypasses_ticket_state_machine() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#]);

    let (code, _stdout, _) = run_ralph(
        ws,
        &["wave", "emit", "review.wave.ready", "--payloads-stdin", "--output", "json"],
        &[],
        Some(&payloads),
    );
    assert_eq!(code, 0, "human CLI must succeed without verify");

    // No ticket was ever written by the verify path → still
    // absent.
    assert!(
        !ticket_path(ws).exists(),
        "human CLI must not require a ticket on disk"
    );
}