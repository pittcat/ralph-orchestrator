//! 2026-09-03-0959 plan U5 (R11/R12; S13/S14; D1/D2/D13; E6/E7/E13/E16):
//! CLI-side driver closure integration tests.
//!
//! Scope: the **outside-in** surface the agent / operator can poke:
//!
//! - `ralph inspect loop --format json` exposes a new `scheduler` key when
//!   `event_loop.supervisor.scheduler_mode != wave`. When `scheduler_mode`
//!   is `wave` (the legacy default), the key is **absent** so existing
//!   `loop_inspect.v2` consumers see no regression (R3).
//! - The JSON is sanitized (R11 / E16): no raw `payload`, no DB path,
//   no `Bearer ` / `token=` / `.jsonl` / `.db` substrings. Only the
//!   bounded counts + `plan_keys` identifiers surface.
//! - The CLI **never** aborts with a non-zero exit because of an empty
//!   shadow sink — the inspect command is read-only and best-effort.
//! - The CLI binary still launches under the legacy supervisor path
//!   (no `event_loop.supervisor.scheduler_mode` override) without
//!   crashing — regression guard for the legacy wave loop.
//!
//! These tests intentionally **avoid** spawning a real `ralph run` —
//! the driver seam is wired in U5, but the full DAG authority lands in
//! U6. The tests instead cover the inspect surface, which is the
//! only thing a CLI invocation can observe in U5.

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

fn write_dag_shadow_ralph_yml(workspace: &std::path::Path) {
    // `scheduler_mode: dag_shadow` requires both
    // `event_loop.supervisor.enabled = true` AND
    // `event_loop.execution_mode: isolated` (U1 fail-closed contract,
    // E12 / S1 / S2). The test config exercises the validation happy
    // path so the inspect command reaches the `scheduler` summary
    // branch.
    let yaml = r#"
event_loop:
  event_policy:
    enabled: false
    mode: enforce
  execution_mode: isolated
  supervisor:
    enabled: true
    scheduler_mode: dag_shadow
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
// Legacy wave mode: `scheduler` key MUST be absent (R3 regression guard).
// =============================================================================

#[test]
fn inspect_loop_wave_mode_omits_scheduler_key() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_minimal_ralph_yml(ws);
    let (code, stdout, _stderr) = run_ralph(ws, &["inspect", "loop", "--format", "json"], &[]);
    assert_eq!(code, 0, "inspect loop must succeed under Wave mode");
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("inspect loop --format json must produce JSON");
    assert!(
        json.get("scheduler").is_none(),
        "Wave mode must NOT surface a `scheduler` block in inspect JSON: {json}"
    );
    // Schema parity: the existing `supervisor` key behaviour is
    // independent (Wave + supervisor disabled → no `supervisor` key).
    assert!(
        json.get("supervisor").is_none(),
        "Wave + supervisor disabled must not surface supervisor: {json}"
    );
}

// =============================================================================
// DagShadow mode: `scheduler` key MUST be present, sanitized, and
// well-formed even when zero driver runs have happened (best-effort
// read-only surface).
// =============================================================================

#[test]
fn inspect_loop_dag_shadow_mode_surfaces_empty_scheduler_block() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_dag_shadow_ralph_yml(ws);
    let (code, stdout, _stderr) = run_ralph(ws, &["inspect", "loop", "--format", "json"], &[]);
    assert_eq!(
        code, 0,
        "inspect loop must succeed even with zero driver observations"
    );
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("inspect loop --format json must produce JSON");
    let scheduler = json
        .get("scheduler")
        .unwrap_or_else(|| panic!("DagShadow mode must surface a `scheduler` block: {json}"));
    // Mode label is the tri-state string form, not a payload-bearing string.
    assert_eq!(
        scheduler["scheduler_mode"], "dag_shadow",
        "scheduler.scheduler_mode must reflect the configured mode"
    );
    // Empty sink → zero counters.
    assert_eq!(scheduler["total_observations"], 0u64);
    assert_eq!(scheduler["admitted_total"], 0u64);
    assert_eq!(scheduler["blocked_total"], 0u64);
    // No plan keys observed yet.
    let plan_keys = scheduler["plan_keys"]
        .as_array()
        .expect("plan_keys must be an array");
    assert!(
        plan_keys.is_empty(),
        "plan_keys must be empty for an empty sink"
    );
}

// =============================================================================
// Sanitization contract: forbidden substrings MUST never appear in the
// `scheduler` JSON, even with adversarial inputs (R11 / E16).
// =============================================================================

#[test]
fn inspect_loop_dag_shadow_scheduler_json_is_sanitized() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_dag_shadow_ralph_yml(ws);
    let (_code, stdout, _stderr) = run_ralph(ws, &["inspect", "loop", "--format", "json"], &[]);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("inspect loop --format json must produce JSON");
    let scheduler = json
        .get("scheduler")
        .expect("DagShadow mode must surface a `scheduler` block");
    // We render the scheduler block as compact JSON to assert against it
    // as a string (the slice that actually reaches the operator / agent).
    let rendered = serde_json::to_string(scheduler).expect("scheduler must serialize");
    for forbidden in [
        "payload",
        "secret",
        "password",
        "/home/",
        "/root/",
        "/tmp/",
        "/var/",
        "token=",
        "Bearer ",
        "fn ",
        "use ",
        ".jsonl",
        ".db",
        "events.jsonl",
        "supervisor.db",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "scheduler JSON must not contain forbidden substring {forbidden:?}: {rendered}"
        );
    }
    // Positive controls: scheduler_mode + totals keys surface.
    assert!(rendered.contains("\"scheduler_mode\":\"dag_shadow\""));
    assert!(rendered.contains("\"total_observations\":0"));
    assert!(rendered.contains("\"admitted_total\":0"));
    assert!(rendered.contains("\"blocked_total\":0"));
}

// =============================================================================
// Schema version contract: when a `scheduler` key is present the
// `schema_version` MUST still be the existing `loop_inspect.v2`
// (no bump for U5 — this is a forward-compatible additive field).
// =============================================================================

#[test]
fn inspect_loop_dag_shadow_keeps_loop_inspect_v2_schema_version() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_dag_shadow_ralph_yml(ws);
    let (_code, stdout, _stderr) = run_ralph(ws, &["inspect", "loop", "--format", "json"], &[]);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("inspect loop --format json must produce JSON");
    assert_eq!(
        json["schema_version"], "loop_inspect.v2",
        "U5 is forward-compatible — schema_version must not bump"
    );
}

// =============================================================================
// Agent-context regression (HARD RULE 5): even when the test harness
// inherits `RALPH_CURRENT_HAT` / `RALPH_CURRENT_LOOP_ID` /
// `RALPH_EVENTS_FILE` / `RALPH_WAVE_WORKER` / `RALPH_TRIGGERED_HAT` /
// `RALPH_HATS_SOURCE` (the wave-worker hat-activation env), the inspect
// command must still:
//   1. exit 0,
//   2. produce a parseable JSON object on stdout (no preflight WARN
//      mixed in — `RALPH_CONFIG` is intentionally NOT re-injected here,
//      because that is a config-discovery concern separate from agent
//      visibility), and
//   3. surface the local ralph.yml's `scheduler` block (since the
//      scheduler summary is gated on `event_loop.supervisor.scheduler_mode`,
//      which is config, not agent, visibility).
//
// Note: `loop_id` / `current_hat` legitimately reflect the polluted
// env vars (they are the inspect surface for that env). The
// forbidden-substring check is intentionally scoped to the
// `scheduler` block, where they must NOT surface (R11 / E16).
// =============================================================================

#[test]
fn inspect_loop_with_polluted_agent_env_still_returns_clean_scheduler_block() {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    write_dag_shadow_ralph_yml(ws);
    // Only inject agent-context env vars. `RALPH_CONFIG` is NOT
    // re-injected — `scrub_agent_runtime_env` stripped it and we want
    // the local ralph.yml (with `scheduler_mode: dag_shadow`) to be
    // resolved normally. A test that mixes RALPH_CONFIG pollution into
    // this regression would conflate two orthogonal concerns: agent
    // visibility vs. config discovery.
    let polluted_env: Vec<(&str, &str)> = vec![
        ("RALPH_CURRENT_HAT", "executor"),
        ("RALPH_CURRENT_LOOP_ID", "loop-polluted"),
        ("RALPH_EVENTS_FILE", "/tmp/polluted-events.jsonl"),
        ("RALPH_WAVE_WORKER", "wave-worker-x"),
        ("RALPH_TRIGGERED_HAT", "executor"),
        ("RALPH_HATS_SOURCE", "/tmp/polluted-hats-source"),
    ];
    let (code, stdout, _stderr) =
        run_ralph(ws, &["inspect", "loop", "--format", "json"], &polluted_env);
    assert_eq!(
        code, 0,
        "inspect loop must succeed even under polluted agent env (HARD RULE 5)"
    );
    // Stdout must be a clean JSON document (no preflight WARN prepended
    // that would break a downstream `serde_json::from_str` consumer).
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("inspect loop --format json must produce JSON");
    // The scheduler block surfaces — local ralph.yml is respected
    // because we did NOT re-inject `RALPH_CONFIG`.
    let scheduler = json
        .get("scheduler")
        .unwrap_or_else(|| panic!("DagShadow mode must surface a `scheduler` block: {json}"));
    assert_eq!(scheduler["scheduler_mode"], "dag_shadow");
    // Sanitization contract on the scheduler block itself (R11 / E16):
    // polluted agent env values MUST NOT leak into the sanitized
    // operator-facing scheduler summary. `loop_id` and `current_hat`
    // are LEGITIMATE reflections of the polluted env at the OUTER
    // view; this assertion is scoped to the scheduler block only.
    let scheduler_rendered = serde_json::to_string(scheduler).expect("scheduler must serialize");
    for forbidden in [
        "/tmp/polluted-events.jsonl",
        "/tmp/polluted-hats-source",
        "loop-polluted",
        "wave-worker-x",
    ] {
        assert!(
            !scheduler_rendered.contains(forbidden),
            "polluted agent env value {forbidden:?} leaked into scheduler block: {scheduler_rendered}"
        );
    }
}
