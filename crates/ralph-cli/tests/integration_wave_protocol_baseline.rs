//! 2026-07-24-003 plan Unit 1: Characterization baseline (post-flip).
//!
//! Assertions below pin the **post U3/U5/U6** contracts. Historical
//! pre-fix shapes lived in earlier revisions of this file.
//!
//! ## Contract map
//!
//! - `baseline_emit_json_includes_events_file` — U5: success JSON
//!   omits `events_file`.
//! - `baseline_idempotency_writes_sidecar` — U5: keyed emit does not
//!   write the legacy sidecar.
//! - `baseline_ticket_removed_before_event_write_on_io_failure` — U6:
//!   Apply-before failure restores the ticket (prepared).
//! - `baseline_inspect_loop_swallows_corrupt_store` — U3: corrupt
//!   store surfaces `availability=unavailable`.
//!
//! ## Agent-context scrubbing (HARD RULE 5)
//!
//! All tests use [`common::ralph_bin`] so any outer hat env that
//! inherits `RALPH_CURRENT_HAT` / `RALPH_CURRENT_LOOP_ID` /
//! `RALPH_EVENTS_FILE` etc. cannot turn these fixtures into
//! agent-context invocations. The agent-context scenarios here
//! opt back in with explicit `.env(...)` overlays after the
//! helper scrubs them.

use crate::common::ralph_bin;
use std::io::Write;
use tempfile::TempDir;

#[path = "common/mod.rs"]
mod common;

/// Render a minimal `ralph.yml` so `load_policy_config_for_cli_emit`
/// finds a known shape and the wave emit path does not bail on a
/// missing config (the supervisor preset must explicitly opt-in via
/// `--hats-source` / supervisor.enabled — out of scope here).
///
/// `event_loop.event_policy.mode` is required by the YAML schema
/// (`mode: enforce`); `mode` is not consulted here because the policy
/// is `enabled: false`. This matches the minimal-render pattern used
/// in `integration_run.rs:638-641`.
///
/// The `coordinator` hat is declared with `publishes: [review.wave.ready]`
/// so the U23 wave-dispatcher ACL allows `ralph wave verify` /
/// `ralph wave emit` from agent context (mirrors what builtin
/// presets do). The hat entry is required by `baseline_ticket_*` —
/// without it the ACL rejects non-dispatcher hats before the
/// ticket gate is even reached.
fn write_minimal_ralph_yml(workspace: &std::path::Path) {
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

/// Run `ralph` against a temp workspace, capturing stdout/stderr
/// and the exit code. Always scrubs agent-context env (HARD RULE 5).
fn run_ralph(
    workspace: &std::path::Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
    stdin_file: Option<&std::path::Path>,
) -> (i32, String, String) {
    let mut cmd = ralph_bin();
    cmd.current_dir(workspace);
    cmd.args(args);
    common::scrub_agent_runtime_env(&mut cmd);
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
// Baseline 1 (FLIPPED by U5): emit JSON must NOT include `events_file`.
// Agents Confirm via `ralph wave inspect`, not ledger paths.
// =============================================================================

#[test]
fn baseline_emit_json_includes_events_file() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"a"}"#, r#"{"dim":"b"}"#]);

    // Human CLI (no RALPH_CURRENT_HAT) → no ticket gate, no ACL gating.
    let (code, stdout, stderr) = run_ralph(
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
    assert_eq!(code, 0, "wave emit must succeed; stderr={stderr}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be JSON; raw={stdout:?}; err={e}"));
    assert_eq!(parsed["ok"], serde_json::json!(true));
    assert_eq!(parsed["topic"], serde_json::json!("review.wave.ready"));
    assert_eq!(parsed["count"], serde_json::json!(2));
    assert_eq!(parsed["deduplicated"], serde_json::json!(false));
    // U5 contract: agents must not see internal ledger paths.
    assert!(
        parsed.get("events_file").is_none(),
        "U5: success JSON must omit `events_file`: {parsed}"
    );
    assert!(
        parsed.get("wave_id").and_then(|v| v.as_str()).is_some(),
        "success JSON must carry public wave_id: {parsed}"
    );
}

// =============================================================================
// Baseline 2 (FLIPPED by U5): keyed emit must NOT write the legacy sidecar.
// =============================================================================

#[test]
fn baseline_idempotency_writes_sidecar() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"x"}"#]);

    let key = "ce-baseline:sidecar-writes";
    let (code, _stdout, stderr) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--output",
            "json",
            "--idempotency-key",
            key,
        ],
        &[],
        Some(&payloads),
    );
    assert_eq!(code, 0, "first emit must succeed; stderr={stderr}");

    let events_file = ws.join(".ralph/events.jsonl");
    let parent = events_file.parent().expect("parent");
    let file_name = events_file.file_name().expect("file_name");
    let sidecar = parent.join(format!(
        ".{}.idempotency.jsonl",
        file_name.to_string_lossy()
    ));
    assert!(
        !sidecar.exists(),
        "U5: keyed emit must not write .idempotency.jsonl (got {sidecar:?})"
    );
}

// =============================================================================
// Baseline 3 (FLIPPED by U6): Apply-before-write failure restores ticket
// (prepared → claimed → restore → prepared). Agent retries without re-verify.
// =============================================================================

#[test]
fn baseline_ticket_removed_before_event_write_on_io_failure() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let payloads = write_payloads(ws, &[r#"{"dim":"x"}"#]);

    let env = [
        ("RALPH_CURRENT_HAT", "coordinator"),
        ("RALPH_CURRENT_LOOP_ID", "loop-baseline-3"),
        ("RALPH_WAVE_EMIT_FAIL_AT", "apply_before_write"),
    ];

    // 1. Verify records a fresh ticket.
    let (v_code, _v_stdout, v_stderr) = run_ralph(
        ws,
        &["wave", "verify", "review.wave.ready", "--payloads-stdin"],
        &env,
        Some(&payloads),
    );
    assert_eq!(v_code, 0, "verify must succeed; stderr={v_stderr}");

    let ticket_path = ws.join(".ralph/agent/.ralph-wave-verify-ticket");
    let claim_path = ws.join(".ralph/agent/.ralph-wave-verify-ticket.claim");
    assert!(
        ticket_path.exists(),
        "verify must have recorded a ticket: {ticket_path:?}"
    );

    // 2. Emit with fault injection before events write.
    let (e_code, _stdout, e_stderr) = run_ralph(
        ws,
        &[
            "wave",
            "emit",
            "review.wave.ready",
            "--payloads-stdin",
            "--output",
            "json",
            "--idempotency-key",
            "baseline-ticket-restore",
        ],
        &env,
        Some(&payloads),
    );
    assert_ne!(
        e_code, 0,
        "emit must fail under apply_before_write; stderr={e_stderr}"
    );

    // U6: ticket restored to prepared; claim cleared.
    assert!(
        ticket_path.exists(),
        "U6: ticket must be restored after Apply-before failure: {ticket_path:?}"
    );
    assert!(
        !claim_path.exists(),
        "U6: claim marker must be cleared after restore: {claim_path:?}"
    );
}

// =============================================================================
// Baseline 4: `inspect loop` surfaces corrupt store as unavailable (U3 flip).
// =============================================================================

#[test]
fn baseline_inspect_loop_swallows_corrupt_store() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);

    // Seed a non-sqlite file where supervisor.db would live so the
    // rusqlite open fails fast and `summarize` falls through to
    // `SupervisorInspectSummary::default()`.
    let ralph_dir = ws.join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    let db_path = ralph_dir.join("supervisor.db");
    std::fs::write(&db_path, b"not a sqlite database\n").unwrap();

    // `inspect loop` must succeed (read-only / best-effort contract).
    let (code, stdout, stderr) = run_ralph(ws, &["inspect", "loop", "--format", "json"], &[], None);
    assert_eq!(
        code, 0,
        "inspect loop must remain best-effort even with corrupt store; stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be JSON; raw={stdout:?}; err={e}"));

    // 2026-07-24-003 plan U3 — FLIPPED baseline (was U1 invariant).
    // The pre-U3 behaviour (`summarize` → `default()` with no
    // availability signal) is the failure mode we just closed; the
    // post-U3 behaviour (corrupt store surfaces `availability =
    // unavailable` with a sanitised reason) is the new contract.
    //
    // This test is the **post-U3** assertion. The U1-frozen
    // `unavailability must be absent` invariant now lives in the
    // git history of this file before the U3 edit; do not
    // re-introduce it. The contract under test is the S13
    // `unknown ≠ unavailable` distinction:
    let supervisor = parsed
        .get("supervisor")
        .unwrap_or_else(|| panic!("inspect loop must surface a supervisor block: {parsed}"));
    assert_eq!(
        supervisor["availability"],
        serde_json::json!("unavailable"),
        "U3 contract: corrupt store MUST surface availability=unavailable"
    );
    assert_eq!(
        supervisor["active_waves"],
        serde_json::json!([]),
        "corrupt store cannot prove active waves; the field stays empty"
    );
}
