//! 2026-07-24-003 plan Unit 1: Characterization baseline.
//!
//! This file **freezes the current behavior** of the wave protocol
//! before any fixes land. Every assertion below must hold against the
//! unmodified code at plan baseline. Subsequent Units (U3, U5, U6)
//! are expected to *invert* these assertions as the protocol
//! changes; the flips are tracked in this file's header comment so
//! the regressions can be audited line-by-line.
//!
//! ## Inversion map (assertion → owning Unit)
//!
//! - `baseline_emit_json_includes_events_file`
//!     Flipped by U5 (`wave emit` JSON loses `events_file` once the
//!     emission authority moves to the supervisor store).
//! - `baseline_idempotency_writes_sidecar`
//!     Flipped by U5 (Store-cutover path: no sidecar read or write
//!     on the happy path; legacy sidecar imported once when
//!     Store has no emission for that scope).
//! - `baseline_ticket_removed_before_event_write_on_io_failure`
//!     Flipped by U6 (`require_ticket` does not delete the ticket
//!     until the Apply step succeeds; mismatch / IO-failure
//!     paths leave the ticket on disk for retry).
//! - `baseline_inspect_loop_swallows_corrupt_store`
//!     **Flipped by U3** — the assertion now pins the post-flip
//!     contract (`availability = unavailable` with sanitised reason)
//!     because the U3 implementation landed in the same commit
//!     series as this file. Reverting this test would be a U3
//!     regression.
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
// Baseline 1: emit JSON includes `events_file`.
// Flipped by U5 — once the supervisor store owns emission, the CLI
// stops echoing the events file path in the success response.
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
    // Pin the field that U5 plans to remove.
    assert!(
        parsed.get("events_file").is_some(),
        "baseline must include `events_file` (U5 inverts this): {parsed}"
    );
    assert!(
        parsed["events_file"].is_string(),
        "events_file must be a string: {parsed}"
    );
}

// =============================================================================
// Baseline 2: idempotency writes the legacy sidecar.
// Flipped by U5 — Store-cutover path does not write the sidecar on
// happy paths; the sidecar remains as a one-shot legacy importer
// for Store misses.
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

    // The events file path is `.ralph/events.jsonl` (default).
    let events_file = ws.join(".ralph/events.jsonl");
    let parent = events_file.parent().expect("parent");
    let file_name = events_file.file_name().expect("file_name");
    let sidecar = parent.join(format!(".{}.idempotency.jsonl", file_name.to_string_lossy()));
    assert!(
        sidecar.exists(),
        "baseline must write .idempotency.jsonl next to events file (U5 inverts this): {sidecar:?}"
    );
    let body = std::fs::read_to_string(&sidecar).unwrap();
    assert!(
        body.contains(key),
        "sidecar must record the key we passed: {body}"
    );
}

// =============================================================================
// Baseline 3: ticket is consumed before the event write is attempted.
// `read_and_consume_ticket` deletes the file before the caller runs
// `write_wave_events_with_provenance`. When the events file cannot be
// opened, the ticket is already gone — the agent has no ticket left
// to retry with.
//
// Flipped by U6: ticket transitions through
// `prepared → claimed → consumed`. IO failure between claim and
// apply restores the ticket so the agent can retry without
// re-running `ralph wave verify`.
//
// We simulate IO failure by pointing RALPH_EVENTS_FILE at a path
// whose parent cannot be created (a *file* path used as a parent
// directory is rejected by `create_dir_all`). The agent is in
// agent-context (RALPH_CURRENT_HAT set) and has a verified ticket
// for a 1-payload batch.
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
    assert!(
        ticket_path.exists(),
        "verify must have recorded a ticket: {ticket_path:?}"
    );

    // 2. Force IO failure on the events-file write by pointing
    // RALPH_EVENTS_FILE at a path whose "parent" is a regular file
    // (create_dir_all on a path that already exists as a file is an
    // IO error on all unix platforms we support).
    let blocker = ws.join("blocker");
    std::fs::write(&blocker, b"not a directory").unwrap();
    let blocked_events = blocker.join("events.jsonl");

    let (e_code, _stdout, e_stderr) = run_ralph(
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
            ("RALPH_CURRENT_LOOP_ID", "loop-baseline-3"),
            ("RALPH_EVENTS_FILE", blocked_events.to_str().unwrap()),
        ],
        Some(&payloads),
    );
    assert_ne!(e_code, 0, "emit must fail under IO failure; stderr={e_stderr}");

    // Baseline invariant: `require_ticket` consumes the ticket *before*
    // the IO failure is observed, so the agent has no ticket left.
    assert!(
        !ticket_path.exists(),
        "baseline: ticket must be gone after the failed emit (U6 restores it): {ticket_path:?}"
    );
}

// =============================================================================
// Baseline 4: `inspect loop` swallows a corrupt supervisor store and
// returns a default-shape summary with no `availability` signal.
//
// Flipped by U3 — `wave inspect` and `inspect loop` surface
// `availability` so the agent can distinguish a healthy empty store
// from a corrupt / unavailable one.
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
    let (code, stdout, stderr) = run_ralph(
        ws,
        &["inspect", "loop", "--format", "json"],
        &[],
        None,
    );
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