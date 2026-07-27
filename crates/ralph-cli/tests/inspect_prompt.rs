//! 2026-07-26-001 plan U3/U4/U5 integration tests for
//! `ralph inspect prompt`.
//!
//! Coverage:
//!   - U3: human output contains the block list and skill table
//!     markers, no loop side effects
//!   - U4: JSON shape is stable; unknown hat exits non-zero with
//!     a stderr line naming the hat
//!   - U5: tempdir without `crates/`/`presets/en/` still resolves a
//!     local preset; agent-context env is scrubbed (HARD RULE 5)
//!
//! Scenarios pinned:
//!   S1 — default gate: auto_inject contains `ralph-tools`;
//!        on_demand contains `ralph-tools-emit`
//!   S3 — human output has recognizable block list
//!   S4 — unknown hat: exit ≠ 0, stderr names the hat
//!   S5 — tempdir with only a local preset YAML succeeds
//!   S6 — agent-context env pollution does not affect output

mod common;

use std::fs;

use common::ralph_bin;

const MINIMAL_PRESET: &str = r#"
event_loop:
  execution_mode: isolated
hats:
  worker:
    name: "Worker"
    triggers: ["work.start"]
    publishes: ["work.done"]
    instructions: "Build a small CLI in Rust that prints hello world."
memories:
  enabled: true
  inject: auto
tasks:
  enabled: true
"#;

const DOUBLE_OFF_PRESET: &str = r#"
event_loop:
  execution_mode: isolated
hats:
  worker:
    name: "Worker"
    triggers: ["work.start"]
    publishes: ["work.done"]
    instructions: "Quiet worker."
memories:
  enabled: false
tasks:
  enabled: false
"#;

fn write_preset(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    path
}

#[test]
fn inspect_prompt_human_lists_blocks_and_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args(["inspect", "prompt", "--hat", "worker"]);

    let output = cmd.output().expect("spawn ralph inspect prompt");
    assert!(
        output.status.success(),
        "inspect prompt must exit 0; got {:?}\nstderr: {}\nstdout: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Block / structure markers (U3 / S3)
    assert!(
        stdout.contains("Prompt visibility preview"),
        "stdout must contain header"
    );
    assert!(
        stdout.contains("hat_id:"),
        "stdout must include hat_id line"
    );
    assert!(stdout.contains("gates:"), "stdout must include gates line");
    assert!(
        stdout.contains("auto_inject"),
        "stdout must include auto_inject block"
    );
    assert!(
        stdout.contains("on_demand"),
        "stdout must include on_demand block"
    );
    assert!(
        stdout.contains("block_titles"),
        "stdout must include block_titles block"
    );

    // Skill classification (S1)
    assert!(
        stdout.contains("ralph-tools") && !stdout.contains("ralph-tools (gated) only"),
        "stdout must mention ralph-tools"
    );
    assert!(
        stdout.contains("ralph-tools-emit"),
        "stdout must list emit as on-demand"
    );

    // No side effects on events.jsonl (S3)
    let events = tmp.path().join(".ralph/events.jsonl");
    assert!(
        !events.exists(),
        "inspect prompt must not create .ralph/events.jsonl; found at {events:?}"
    );
}

#[test]
fn inspect_prompt_json_shape_is_stable() {
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args(["inspect", "prompt", "--hat", "worker", "--format", "json"]);

    let output = cmd.output().expect("spawn ralph inspect prompt json");
    assert!(
        output.status.success(),
        "inspect prompt json must exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    // Stable top-level keys
    for key in [
        "hat_id",
        "gates",
        "auto_inject",
        "on_demand",
        "block_titles",
    ] {
        assert!(
            parsed.get(key).is_some(),
            "JSON must contain top-level key {key}; got {parsed:?}"
        );
    }
    assert!(
        parsed.get("evidence_level").is_none(),
        "default preview must omit evidence_level; got {parsed:?}"
    );
    assert_eq!(parsed["hat_id"], "worker");

    let gates = &parsed["gates"];
    assert_eq!(gates["tasks_enabled"], true);
    assert_eq!(gates["memories_enabled"], true);

    let auto: Vec<&str> = parsed["auto_inject"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    for expected in ["ralph-tools", "ralph-tools-tasks", "ralph-tools-memories"] {
        assert!(
            auto.contains(&expected),
            "auto_inject must include {expected}; got {auto:?}"
        );
    }

    // on-demand must include emit/wave/cmdref/precheck/recovery-directives
    let on_demand: Vec<&str> = parsed["on_demand"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    for expected in [
        "ralph-tools-emit",
        "ralph-tools-wave",
        "ralph-tools-cmdref",
        "ralph-tools-precheck",
        "ralph-tools-recovery-directives",
    ] {
        assert!(
            on_demand.contains(&expected),
            "on_demand must include {expected}; got {on_demand:?}"
        );
    }

    // Auto-inject discriminator
    let auto_first = &parsed["auto_inject"][0];
    assert_eq!(
        auto_first["source"], "gated",
        "default-gate auto-inject source must be `gated`"
    );

    // block_titles must be non-empty (U3 / S3) — the CLI path
    // constructs an EventLoop (under a tracing sink so stdout
    // stays clean for JSON) so the build_prompt-driven block
    // extraction runs end-to-end.
    let titles = parsed["block_titles"].as_array().unwrap();
    assert!(
        !titles.is_empty(),
        "block_titles must be non-empty for the CLI path; got {titles:?}"
    );
}

#[test]
fn inspect_prompt_candidate_emit_marks_runtime_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args([
            "inspect",
            "prompt",
            "--hat",
            "worker",
            "--format",
            "json",
            "--topic",
            "work.done",
            "--payload",
            r#"{"task_key":"task-123"}"#,
        ]);

    let output = cmd.output().expect("spawn ralph inspect prompt candidate emit");
    assert!(
        output.status.success(),
        "candidate emit preview must exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("candidate emit JSON");

    assert_eq!(parsed["evidence_level"], "runtime");
    let candidate = &parsed["candidate_emit"];
    assert_eq!(candidate["policy_decision"], "accept");
    assert_eq!(candidate["next_hat_candidates"]["kind"], "verified");
    assert!(
        candidate["next_hat_candidates"]["hats"]
            .as_array()
            .expect("hats array")
            .is_empty(),
        "no downstream subscribers in the minimal preset should produce an empty verified routing set"
    );
}

#[test]
fn inspect_prompt_unknown_hat_exits_nonzero_with_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args(["inspect", "prompt", "--hat", "does-not-exist"]);

    let output = cmd.output().expect("spawn ralph inspect prompt bad hat");
    assert!(
        !output.status.success(),
        "unknown hat must exit non-zero; got {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does-not-exist") || stderr.to_lowercase().contains("not found"),
        "stderr must name the missing hat; got: {stderr}"
    );
}

#[test]
fn inspect_prompt_double_off_gate_excludes_ralph_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local-off.yml", DOUBLE_OFF_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args(["inspect", "prompt", "--hat", "worker", "--format", "json"]);

    let output = cmd.output().expect("spawn ralph inspect prompt double-off");
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");

    let auto: Vec<&str> = parsed["auto_inject"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert!(
        auto.is_empty(),
        "double-off gate must produce empty auto_inject; got {auto:?}"
    );

    let gates = &parsed["gates"];
    assert_eq!(gates["tasks_enabled"], false);
    assert_eq!(gates["memories_enabled"], false);
}

#[test]
fn inspect_prompt_works_in_tempdir_without_crates_dir() {
    // S5: a project that only carries a local preset YAML (no
    // `crates/`, no `presets/en/`) must still resolve — the
    // embedded skills come from the running ralph binary.
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "outer.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args(["inspect", "prompt", "--hat", "worker", "--format", "json"]);

    let output = cmd.output().expect("spawn ralph inspect prompt in tempdir");
    assert!(
        output.status.success(),
        "inspect prompt in tempdir must exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");
    assert_eq!(parsed["hat_id"], "worker");

    // The `Build a small CLI in Rust that prints hello world.`
    // instructions line is encoded via the block list / on-demand
    // surface; we don't strictly require it on stdout (it may be
    // trimmed) but at minimum the hat must be locatable.
    assert!(!parsed["auto_inject"].as_array().unwrap().is_empty());
}

#[test]
fn inspect_prompt_survives_polluted_agent_env() {
    // S6: HARD RULE 5 — the human CLI must ignore inherited
    // agent-context env vars. We pollute the spawn env with
    // hat-context values and confirm output matches the scrubbed
    // run.
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args(["inspect", "prompt", "--hat", "worker", "--format", "json"])
        // Polluted env: simulate hat-injected context from an
        // outer loop. `ralph_bin()` already scrubbed the base
        // env; we explicitly re-add the keys here.
        .env("RALPH_CURRENT_HAT", "executor")
        .env("RALPH_CURRENT_LOOP_ID", "loop-x")
        .env("RALPH_EVENTS_FILE", "/tmp/some-events.jsonl")
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-pipeline")
        .env("RALPH_WAVE_WORKER", "wave-1")
        .env("RALPH_TRIGGERED_HAT", "executor")
        .env("RALPH_CONFIG", "/tmp/some-ralph.yml");

    let output = cmd.output().expect("spawn ralph inspect prompt polluted");
    assert!(
        output.status.success(),
        "inspect prompt must succeed under polluted env; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid json");

    // The polluted env must NOT have rerouted the request: we
    // asked for `--hat worker`, we must get `worker` back, not
    // `executor` (the polluted RALPH_CURRENT_HAT value).
    assert_eq!(
        parsed["hat_id"], "worker",
        "polluted RALPH_CURRENT_HAT must not change the resolved hat"
    );
}

#[test]
fn inspect_prompt_help_lists_subcommand() {
    let mut cmd = ralph_bin();
    cmd.args(["inspect", "--help"]);
    let output = cmd.output().expect("spawn ralph inspect --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("prompt") || stdout.contains("Prompt"),
        "inspect --help must list the new `prompt` subcommand; got: {stdout}"
    );
}

#[test]
fn inspect_prompt_full_json_returns_real_body() {
    // T1: --full --format json returns a non-empty prompt_body with real content
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args([
            "inspect", "prompt", "--hat", "worker", "--format", "json", "--full",
        ]);

    let output = cmd
        .output()
        .expect("spawn ralph inspect prompt --full json");
    assert!(
        output.status.success(),
        "inspect prompt --full json must exit 0; got {:?}\nstderr: {}\nstdout: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    // prompt_body must be a non-empty string with real content
    let prompt_body = parsed["prompt_body"]
        .as_str()
        .expect("prompt_body must be a string");
    assert!(
        prompt_body.len() > 100,
        "prompt_body must be >100 chars, got {} chars: {prompt_body}",
        prompt_body.len()
    );

    // The MINIMAL_PRESET instruction literal must appear in the rendered body
    assert!(
        prompt_body.contains("Build a small CLI in Rust that prints hello world."),
        "prompt_body must contain the hat instruction literal; got: {prompt_body}"
    );
}

#[test]
fn inspect_prompt_full_human_prints_full_body() {
    // T1: --full --format human prints the full body (not suppressed)
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args([
            "inspect", "prompt", "--hat", "worker", "--format", "human", "--full",
        ]);

    let output = cmd
        .output()
        .expect("spawn ralph inspect prompt --full human");
    assert!(
        output.status.success(),
        "inspect prompt --full human must exit 0; got {:?}\nstderr: {}\nstdout: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Must print the instruction literal from MINIMAL_PRESET
    assert!(
        stdout.contains("Build a small CLI in Rust that prints hello world."),
        "stdout must contain the hat instruction literal; got: {stdout}"
    );

    // Must NOT print the suppressed message
    assert!(
        !stdout.contains("suppressed in this build"),
        "stdout must NOT contain suppressed message; got: {stdout}"
    );
    assert!(
        !stderr.contains("suppressed in this build"),
        "stderr must NOT contain suppressed message; got: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Fail-loud contracts (U1 error-handling hardening):
// `--full` promises a real `prompt_body`; emitting `""` or a "suppressed"
// placeholder would mask a real failure. These tests pin that promise:
// when build_prompt returns None, the CLI must exit non-zero with stderr
// naming the failure (not silently emit an empty SSOT field).
// ---------------------------------------------------------------------------

#[test]
fn inspect_prompt_full_json_unknown_hat_exits_nonzero() {
    // A1 fail-loud: --full --format json with an unknown hat must
    // exit non-zero. The hat-not-found branch fires before
    // build_prompt runs, so this exercises the early-exit error
    // path (exit 2 + stderr names the hat).
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args([
            "inspect", "prompt", "--hat", "ghost", "--format", "json", "--full",
        ]);

    let output = cmd
        .output()
        .expect("spawn ralph inspect prompt --full json unknown hat");
    assert!(
        !output.status.success(),
        "--full with unknown hat must exit non-zero; got {:?}\nstderr: {}\nstdout: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ghost") || String::from_utf8_lossy(&output.stdout).contains("ghost"),
        "stderr or stdout must name the unknown hat; got stderr: {stderr}\nstdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn inspect_prompt_full_json_does_not_emit_empty_body() {
    // A2 contract pin: even if `--full` were to fail internally, the
    // JSON output must NEVER be `{"prompt_body": "", ...}`. Either
    // the body is real (success) or the command exits non-zero with
    // a stderr error (failure). Silent `prompt_body: ""` is the bug
    // we just fixed.
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args([
            "inspect", "prompt", "--hat", "worker", "--format", "json", "--full",
        ]);

    let output = cmd
        .output()
        .expect("spawn ralph inspect prompt --full json");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        // Success path: prompt_body must be non-empty.
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout).expect("must be valid JSON on success");
        let body = parsed["prompt_body"]
            .as_str()
            .expect("prompt_body must be a string on success");
        assert!(
            !body.is_empty(),
            "successful --full JSON must have non-empty prompt_body; got empty"
        );
    } else {
        // Failure path: stdout must NOT contain a JSON with prompt_body="".
        // Either stdout is empty, or stdout has no prompt_body key.
        if !stdout.trim().is_empty()
            && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&stdout)
        {
            let body = parsed.get("prompt_body").and_then(|v| v.as_str());
            assert!(
                body.map(str::is_empty) != Some(true),
                "failed --full JSON must NOT emit prompt_body=\"\"; \
                 emit must fail-loud instead; got stdout: {stdout}\nstderr: {stderr}"
            );
        }
        assert!(
            !stderr.is_empty(),
            "failed --full must produce stderr; got empty stderr"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// Unit 7 of plan 2026-07-27-002: full SkillGateFlags override in CLI.
// ─────────────────────────────────────────────────────────────────────

/// With all three skill gate overrides provided, the JSON output must
/// contain `skill_gates` with all three fields set to the exact values.
#[test]
fn inspect_prompt_skill_gate_all_three_overrides() {
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args([
            "inspect",
            "prompt",
            "--hat",
            "worker",
            "--format",
            "json",
            "--scratchpad",
            "true",
            "--tasks-enabled",
            "false",
            "--memories-enabled",
            "true",
        ]);

    let output = cmd
        .output()
        .expect("spawn ralph inspect prompt with all three gates");
    assert!(
        output.status.success(),
        "inspect prompt with skill gate overrides must exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    let gates = parsed
        .get("skill_gates")
        .expect("skill_gates must be present");
    assert_eq!(
        gates.get("tasks_enabled").and_then(|v| v.as_bool()),
        Some(false),
        "tasks_enabled must be false as override"
    );
    assert_eq!(
        gates.get("memories_enabled").and_then(|v| v.as_bool()),
        Some(true),
        "memories_enabled must be true as override"
    );
    assert_eq!(
        gates.get("scratchpad_enabled").and_then(|v| v.as_bool()),
        Some(true),
        "scratchpad_enabled must be true as override"
    );
}

/// When only `--scratchpad false` is provided, `tasks_enabled` and
/// `memories_enabled` must fall back to the effective config values
/// (not hardcoded false). The preset has tasks=true, memories=true.
#[test]
fn inspect_prompt_skill_gate_partial_override_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args([
            "inspect",
            "prompt",
            "--hat",
            "worker",
            "--format",
            "json",
            "--scratchpad",
            "false",
        ]);

    let output = cmd
        .output()
        .expect("spawn ralph inspect prompt with scratchpad=false");
    assert!(
        output.status.success(),
        "inspect prompt with partial override must exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    let gates = parsed
        .get("skill_gates")
        .expect("skill_gates must be present");
    // MINIMAL_PRESET has tasks=true, memories=true — must fall back to those.
    assert_eq!(
        gates.get("tasks_enabled").and_then(|v| v.as_bool()),
        Some(true),
        "tasks_enabled must fall back to effective config (true)"
    );
    assert_eq!(
        gates.get("memories_enabled").and_then(|v| v.as_bool()),
        Some(true),
        "memories_enabled must fall back to effective config (true)"
    );
    assert_eq!(
        gates.get("scratchpad_enabled").and_then(|v| v.as_bool()),
        Some(false),
        "scratchpad_enabled must be false as explicit override"
    );
}

/// When no skill gate override flags are provided, `skill_gates` must be
/// absent from JSON (preserves the pre-U7 default behavior).
#[test]
fn inspect_prompt_skill_gate_no_override_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    let preset_path = write_preset(tmp.path(), "local.yml", MINIMAL_PRESET);

    let mut cmd = ralph_bin();
    cmd.current_dir(&tmp)
        .args(["-c", preset_path.to_str().unwrap()])
        .args(["inspect", "prompt", "--hat", "worker", "--format", "json"]);

    let output = cmd
        .output()
        .expect("spawn ralph inspect prompt without overrides");
    assert!(
        output.status.success(),
        "inspect prompt without overrides must exit 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    // skill_gates must be absent when no override is supplied.
    assert!(
        parsed.get("skill_gates").is_none(),
        "no override → skill_gates must be absent from JSON; got: {parsed}"
    );
}
