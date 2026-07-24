//! 2026-07-24-003 plan Unit 2: outside-in surface for
//! `ralph wave inspect`.
//!
//! Unit 2 deliberately drives the surface *without* the supervisor
//! store (that is U3). The DTO, parser, and four-state view
//! (`Found` / `Unknown` / `Unavailable` / invalid args) must be
//! locked down here so U3 only wires the read model into the
//! existing seam.
//!
//! Negative-coverage contract (R11 / output safety):
//!
//! - `wave_id` is the only stable identifier echoed back.
//! - No `db_path`, `events_file`, internal `store_id`, `pid`,
//!   payload, ticket path, or events JSONL path may appear in the
//!   serialised view under any state (Found / Unknown /
//!   Unavailable).
//!
//! Unknown / Unavailable distinctions (S13):
//!
//! - Unknown wave_id → `registered: false` (the wave never made it
//!   into the supervisor store). `availability` must remain
//!   `available` because the store is reachable; only the lookup
//!   missed.
//! - Unavailable store → `registered: false` and
//!   `availability: "unavailable"` so the agent can distinguish
//!   "store said no" from "store unreachable".

use crate::common::ralph_bin;
use tempfile::TempDir;

#[path = "common/mod.rs"]
mod common;

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

fn run_ralph(
    workspace: &std::path::Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> (i32, String, String) {
    let mut cmd = ralph_bin();
    cmd.current_dir(workspace);
    cmd.args(args);
    common::scrub_agent_runtime_env(&mut cmd);
    for (k, v) in extra_env {
        cmd.env(*k, *v);
    }
    let output = cmd.output().expect("ralph invocation must succeed");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

// =============================================================================
// CLI parser: `ralph wave --help` lists `inspect` as a subcommand.
// =============================================================================

#[test]
fn inspect_help_lists_subcommand() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let (code, stdout, stderr) = run_ralph(ws, &["wave", "--help"], &[]);
    assert_eq!(code, 0, "wave --help must exit 0; stderr={stderr}");
    assert!(
        stdout.contains("inspect"),
        "wave --help must list the inspect subcommand: {stdout}"
    );
    // The other subcommands still appear (no surface regression).
    assert!(stdout.contains("emit"), "emit must still appear: {stdout}");
    assert!(
        stdout.contains("verify"),
        "verify must still appear: {stdout}"
    );
}

// =============================================================================
// Unknown wave_id returns `registered=false` with `availability=available`.
// =============================================================================

#[test]
fn inspect_unknown_wave_id_returns_not_registered() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    // No DB → no error, but the lookup misses.
    let (code, stdout, stderr) = run_ralph(
        ws,
        &["wave", "inspect", "w-does-not-exist", "--output", "json"],
        &[],
    );
    assert_eq!(code, 0, "inspect unknown must exit 0; stderr={stderr}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be JSON; raw={stdout:?}; err={e}"));
    assert_eq!(parsed["ok"], serde_json::json!(true));
    assert_eq!(parsed["wave_id"], serde_json::json!("w-does-not-exist"));
    assert_eq!(parsed["registered"], serde_json::json!(false));
    // S13: the lookup-miss case must NOT claim unavailable — only a
    // store-open failure does that.
    assert_eq!(
        parsed["availability"],
        serde_json::json!("available"),
        "lookup-miss must NOT collapse into unavailable (S13)"
    );
}

// =============================================================================
// Negative-output contract: the view NEVER leaks db paths / event
// files / ticket paths / payload contents under any of the four
// reachable states.
// =============================================================================

const FORBIDDEN_OUTPUT_FIELDS: &[&str] = &[
    "db_path",
    "events_file",
    "store_id",
    "pid",
    "payload",
    "ticket",
    "idempotency",
];

fn assert_no_forbidden_fields(view: &serde_json::Value, label: &str) {
    let obj = view.as_object().expect("view must be a JSON object");
    for forbidden in FORBIDDEN_OUTPUT_FIELDS {
        assert!(
            !obj.contains_key(*forbidden),
            "{label}: view must not contain `{forbidden}` field: {view}"
        );
    }
    // And no values that look like absolute paths to internal ledgers.
    let s = serde_json::to_string(view).expect("re-serialise view");
    for forbidden_fragment in [
        ".ralph/",
        "supervisor.db",
        "events.jsonl",
        ".ralph-wave-verify",
    ] {
        assert!(
            !s.contains(forbidden_fragment),
            "{label}: serialised view must not contain `{forbidden_fragment}`: {s}"
        );
    }
}

#[test]
fn inspect_unknown_view_omits_internal_fields() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let (_code, stdout, _stderr) = run_ralph(
        ws,
        &["wave", "inspect", "w-private-id", "--output", "json"],
        &[],
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_no_forbidden_fields(&parsed, "unknown wave");
}

#[test]
fn inspect_unavailable_store_view_omits_internal_fields() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);

    // Seed a non-sqlite file where supervisor.db would live so the
    // U3 store-open path fails fast. We pre-create the file so the
    // path resolves but the contents cannot be parsed.
    let ralph_dir = ws.join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    std::fs::write(ralph_dir.join("supervisor.db"), b"not a sqlite database\n").unwrap();

    let (_code, stdout, _stderr) =
        run_ralph(ws, &["wave", "inspect", "w-x", "--output", "json"], &[]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be JSON; raw={stdout:?}; err={e}"));
    assert_eq!(parsed["registered"], serde_json::json!(false));
    assert_eq!(
        parsed["availability"],
        serde_json::json!("unavailable"),
        "corrupt DB must surface as unavailable (S13)"
    );
    assert_no_forbidden_fields(&parsed, "unavailable store");
}

// =============================================================================
// Default text output is friendly and never silent — used by humans and
// as a CLI smoke check that the new subcommand wires up.
// =============================================================================

#[test]
fn inspect_unknown_text_output_is_human_friendly() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let (code, stdout, stderr) =
        run_ralph(ws, &["wave", "inspect", "w-none", "--output", "text"], &[]);
    assert_eq!(code, 0, "inspect unknown must exit 0; stderr={stderr}");
    let lower = stdout.to_lowercase();
    assert!(
        lower.contains("not registered") || lower.contains("unknown"),
        "text output must distinguish unknown: {stdout}"
    );
    assert!(stdout.contains("w-none"), "echoes wave_id: {stdout}");
}

// =============================================================================
// CLI parsing rejection: missing wave_id argument fails with a stable
// clap error rather than a panic.
// =============================================================================

#[test]
fn inspect_requires_wave_id_arg() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let (code, _stdout, stderr) = run_ralph(ws, &["wave", "inspect"], &[]);
    assert_ne!(code, 0, "missing wave_id must fail; stderr={stderr}");
    let lower = stderr.to_lowercase();
    assert!(
        lower.contains("usage") || lower.contains("required") || lower.contains("<wave_id>"),
        "clap error must mention the missing argument: {stderr}"
    );
}

// =============================================================================
// CLI parsing: --output rejects unknown values.
// =============================================================================

#[test]
fn inspect_rejects_unknown_output_format() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let (code, _stdout, stderr) =
        run_ralph(ws, &["wave", "inspect", "w-x", "--output", "xml"], &[]);
    assert_ne!(code, 0, "unknown --output must fail: {stderr}");
}

// =============================================================================
// Agent-context scrubbing (HARD RULE 5): `ralph wave inspect` runs in
// pure read-only diagnostic mode regardless of inherited hat env.
// The contract is that the command must NOT bail on a polluted env.
// =============================================================================

#[test]
fn inspect_tolerates_polluted_hat_env() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let (code, stdout, stderr) = run_ralph(
        ws,
        &["wave", "inspect", "w-polluted", "--output", "json"],
        &[
            ("RALPH_CURRENT_HAT", "executor"),
            ("RALPH_CURRENT_LOOP_ID", "loop-polluted"),
            ("RALPH_EVENTS_FILE", "/tmp/x.jsonl"),
            ("RALPH_WAVE_WORKER", "1"),
        ],
    );
    assert_eq!(
        code, 0,
        "wave inspect must remain read-only even with polluted hat env; stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be JSON; raw={stdout:?}; err={e}"));
    assert_eq!(parsed["registered"], serde_json::json!(false));
}

// =============================================================================
// U3: `inspect loop` flips U1's `baseline_inspect_loop_swallows_corrupt_store`.
// A corrupt supervisor db must surface `availability=unavailable` so the
// agent can distinguish a healthy empty store from a corrupt one
// (S13). The legacy "default empty summary" shape is no longer
// acceptable — the JSON must carry an `availability` block that
// the agent can grep.
// =============================================================================

#[test]
fn inspect_loop_corrupt_store_surfaces_unavailable() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let ralph_dir = ws.join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    std::fs::write(ralph_dir.join("supervisor.db"), b"not a sqlite database\n").unwrap();

    let (code, stdout, stderr) = run_ralph(ws, &["inspect", "loop", "--format", "json"], &[]);
    assert_eq!(
        code, 0,
        "inspect loop must remain best-effort; stderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be JSON; raw={stdout:?}; err={e}"));

    let supervisor = parsed
        .get("supervisor")
        .expect("inspect loop must surface a supervisor block");
    assert_eq!(
        supervisor["availability"],
        serde_json::json!("unavailable"),
        "corrupt DB must flip availability to unavailable (U3 closes U1 baseline)"
    );
    // The shape must NOT silently pass as a healthy empty store.
    assert_eq!(
        supervisor["active_waves"],
        serde_json::json!([]),
        "corrupt store cannot prove active waves; the field stays empty"
    );
}

// =============================================================================
// U3: store-open failure for `wave inspect` flips the same availability
// contract on the wave command surface. The unified shape lets agents
// pin one rule across both commands.
// =============================================================================

#[test]
fn wave_inspect_corrupt_store_surfaces_unavailable() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let ralph_dir = ws.join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    std::fs::write(ralph_dir.join("supervisor.db"), b"corrupted\n").unwrap();

    let (_code, stdout, _stderr) =
        run_ralph(ws, &["wave", "inspect", "w-x", "--output", "json"], &[]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        parsed["availability"],
        serde_json::json!("unavailable"),
        "corrupt store must surface availability=unavailable"
    );
    assert_eq!(parsed["registered"], serde_json::json!(false));
}

// =============================================================================
// U3: a missing supervisor db is a *lookup miss*, not unavailable. The
// store is reachable (no error opening it because there is no db to
// open); the wave never reached the store. This is the S13
// `unknown ≠ unavailable` distinction.
// =============================================================================

#[test]
fn wave_inspect_missing_store_is_known_unknown() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    // No supervisor.db on disk.

    let (_code, stdout, _stderr) =
        run_ralph(ws, &["wave", "inspect", "w-x", "--output", "json"], &[]);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["registered"], serde_json::json!(false));
    assert_eq!(
        parsed["availability"],
        serde_json::json!("available"),
        "missing db is a lookup miss, NOT unavailable"
    );
}
