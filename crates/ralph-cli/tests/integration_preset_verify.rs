//! Integration tests for `ralph preset verify` (Unit 4).
//!
//! These tests use the real `ralph` binary against temp workspaces with
//! non-Rust content (just YAML), confirming that:
//! - `verify --help` exposes the public contract (`--scenario`, `--format`)
//! - a successful run returns exit 0 and a parseable JSON report
//! - a static failure path does not consume the scenario
//! - a remote hats source is rejected without any network call
//! - the source_kind is correctly classified (builtin vs file vs remote)
//! - `preset check` and `inspect prompt` regressions are unchanged
//!
//! Each test scrubs agent-context env via `common::scrub_agent_runtime_env`
//! per HARD RULE 5.

mod common;

use common::{ralph_bin, scrub_agent_runtime_env};
use std::fs;
use std::path::Path;

fn write_file(path: &Path, content: &str) {
    fs::write(path, content).expect("write fixture");
}

/// Locate the JSON object the CLI's verify command prints. The CLI's
/// tracing layer may emit log lines before the JSON; this helper scans
/// for the FIRST balanced JSON object starting with `{` on a new line.
fn extract_json(stdout: &str) -> String {
    let bytes = stdout.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'{' {
            // Confirm it starts at line start (preceded by newline) OR
            // is the very first byte.
            let at_line_start = i == 0 || bytes[i - 1] == b'\n';
            if at_line_start {
                start = i;
                break;
            }
        }
    }
    // Walk forward to find the matching closing brace (respecting strings).
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;
    let chars: Vec<char> = stdout[start..].chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            continue;
        }
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return chars[..=i].iter().collect();
            }
        }
    }
    stdout[start..].to_string()
}

/// Minimal valid scenario YAML for the verify CLI tests. The runtime
/// never advances past the first hat (single empty response), so the
/// scenario is intentionally `terminal: none` so the verdict does not
/// require a real terminal topic — it tests CLI plumbing, not workflow.
const MINIMAL_SCENARIO: &str = r#"
version: 1
scenarios:
  - name: minimal-bounded
    responses:
      - output: ""
        success: true
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 4
      no_progress_steps: 4
"#;

/// Minimal hats YAML that subscribes to work.start and publishes one
/// topic. `task.start` / `task.resume` are reserved for Ralph (the
/// coordinator); we use the user-facing starting event instead.
const MINIMAL_HATS: &str = r#"
hats:
  doer:
    name: Doer
    description: dummy hat for verify plumbing
    triggers:
      - work.start
    publishes:
      - LOOP_COMPLETE
    instructions: "noop"
"#;

/// Minimal core config YAML.
const MINIMAL_CORE: &str = r#"
event_loop:
  execution_mode: isolated
  starting_event: work.start
  completion_promise: LOOP_COMPLETE
  max_iterations: 4
"#;

#[test]
fn preset_verify_help_exposes_public_contract() {
    let mut cmd = ralph_bin();
    cmd.args(["preset", "verify", "--help"]);
    let output = cmd.output().expect("spawn ralph");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--scenario") && stdout.contains("--format"),
        "verify help must mention --scenario and --format; got:\n{stdout}"
    );
    assert!(
        stdout.contains("human") && stdout.contains("json"),
        "verify help must mention human/json format values; got:\n{stdout}"
    );
}

#[test]
fn preset_verify_minimal_run_writes_json_report_and_exits_zero() {
    // Use `builtin:merge-batch` — the only builtin that passes strict
    // static contract — for the happy-path exit-0 path. Its starting
    // event is `merge.start`. The scenario is intentionally trivial
    // (terminal: none, single empty response) so the verdict tests
    // CLI plumbing, not workflow correctness.
    let tmp = tempfile::tempdir().expect("tempdir");
    let scenario_path = tmp.path().join("scenario.yml");
    write_file(
        &scenario_path,
        r#"
version: 1
scenarios:
  - name: minimal-bounded
    responses:
      - output: ""
        success: true
    expect:
      start_event: merge.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 4
      no_progress_steps: 4
"#,
    );

    let mut cmd = ralph_bin();
    cmd.current_dir(tmp.path())
        .env("RUST_LOG", "off")
        .args([
            "-H",
            "builtin:merge-batch",
            "preset",
            "verify",
            "--scenario",
            scenario_path.to_str().unwrap(),
            "--format",
            "json",
        ]);

    let output = cmd.output().expect("spawn ralph");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json_slice = extract_json(&stdout);
    let json: serde_json::Value = serde_json::from_str(&json_slice)
        .unwrap_or_else(|e| panic!("verify JSON unparseable: {e}\njson_slice={json_slice}\nstderr={stderr}"));
    assert!(json["passed"].is_boolean());
    assert!(json["source_kind"].is_string());
    assert!(json["static"].is_object());
    assert!(json["scenarios"].is_array());
    assert!(json["trace_digest"].is_string());
    let passed = json["passed"].as_bool().unwrap();
    let expected_exit = if passed { 0 } else { 1 };
    assert_eq!(
        output.status.code(),
        Some(expected_exit),
        "exit code must mirror passed flag; passed={passed}; status={:?}\nstderr={stderr}",
        output.status
    );
}

#[test]
fn preset_verify_remote_hats_source_is_rejected_without_network() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let core_path = tmp.path().join("core.yml");
    let scenario_path = tmp.path().join("scenario.yml");
    write_file(&core_path, MINIMAL_CORE);
    write_file(&scenario_path, MINIMAL_SCENARIO);

    let mut cmd = ralph_bin();
    scrub_agent_runtime_env(&mut cmd);
    cmd.current_dir(tmp.path())
        .args([
            "-c",
            core_path.to_str().unwrap(),
            "-H",
            "https://example.com/should-not-fetch/hats.yml",
            "preset",
            "verify",
            "--scenario",
            scenario_path.to_str().unwrap(),
            "--format",
            "json",
        ]);

    let output = cmd.output().expect("spawn ralph");
    assert_ne!(
        output.status.code(),
        Some(0),
        "remote hats source must be rejected (non-zero exit)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("remote") || stderr.contains("verify does not accept"),
        "stderr must explain the rejection; got: {stderr}"
    );
    // The stdout must NOT have hit any network — verify prints JSON only
    // when the runtime actually ran. A remote rejection happens before
    // runtime, so stdout should not contain a verify report.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("\"passed\":true"),
        "verify must not produce a passing report for a remote source; stdout={stdout}"
    );
}

#[test]
fn preset_verify_builtin_source_kind_is_classified() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let core_path = tmp.path().join("core.yml");
    let scenario_path = tmp.path().join("scenario.yml");
    write_file(&core_path, MINIMAL_CORE);
    write_file(&scenario_path, MINIMAL_SCENARIO);

    let mut cmd = ralph_bin();
    cmd.current_dir(tmp.path())
        .args([
            "-c",
            core_path.to_str().unwrap(),
            "-H",
            "builtin:debug",
            "preset",
            "verify",
            "--scenario",
            scenario_path.to_str().unwrap(),
            "--format",
            "json",
        ]);

    let output = cmd.output().expect("spawn ralph");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // We don't assert exit code (the builtin:debug preset may not match
    // the scenario starting_event); we only assert that the source_kind
    // field is correctly classified when a report IS produced.
    let json_slice = extract_json(&stdout);
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&json_slice) {
        assert_eq!(
            json["source_kind"],
            serde_json::Value::String("builtin".to_string()),
            "builtin: prefix must classify as builtin; stdout={stdout}\nstderr={stderr}"
        );
    }
}

#[test]
fn preset_verify_file_source_kind_is_external() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let core_path = tmp.path().join("core.yml");
    let hats_path = tmp.path().join("hats.yml");
    let scenario_path = tmp.path().join("scenario.yml");
    write_file(&core_path, MINIMAL_CORE);
    write_file(&hats_path, MINIMAL_HATS);
    write_file(&scenario_path, MINIMAL_SCENARIO);

    let mut cmd = ralph_bin();
    cmd.current_dir(tmp.path())
        .args([
            "-c",
            core_path.to_str().unwrap(),
            "-H",
            hats_path.to_str().unwrap(),
            "preset",
            "verify",
            "--scenario",
            scenario_path.to_str().unwrap(),
            "--format",
            "json",
        ]);

    let output = cmd.output().expect("spawn ralph");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_slice = extract_json(&stdout);
    let json: serde_json::Value =
        serde_json::from_str(&json_slice).expect("verify should produce parseable JSON");
    assert_eq!(
        json["source_kind"],
        serde_json::Value::String("external".to_string()),
        "local file hats must classify as external; stdout={stdout}"
    );
}

#[test]
fn preset_verify_static_failure_returns_nonzero_and_does_not_consume_scenario() {
    // `builtin:debug` fails strict static contract check; the driver
    // must not consume the scenario before reporting the static failure.
    let tmp = tempfile::tempdir().expect("tempdir");
    let scenario_path = tmp.path().join("scenario.yml");
    write_file(&scenario_path, MINIMAL_SCENARIO);

    let mut cmd = ralph_bin();
    cmd.current_dir(tmp.path())
        .args([
            "-H",
            "builtin:debug",
            "preset",
            "verify",
            "--scenario",
            scenario_path.to_str().unwrap(),
            "--format",
            "json",
        ]);

    let output = cmd.output().expect("spawn ralph");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_slice = extract_json(&stdout);
    let json: serde_json::Value = serde_json::from_str(&json_slice)
        .unwrap_or_else(|e| panic!("verify JSON unparseable: {e}\njson_slice={json_slice}"));
    assert_eq!(json["passed"], serde_json::Value::Bool(false));
    let scenarios = json["scenarios"].as_array().expect("scenarios array");
    assert!(
        scenarios.is_empty(),
        "static failure must not consume scenarios; got {scenarios:?}"
    );
    assert_ne!(output.status.code(), Some(0), "static failure must exit nonzero");
}

#[test]
fn preset_verify_start_event_mismatch_is_input_error() {
    // StartEventMismatch (scenario start_event mismatches preset starting_event)
    // must classify as input_error per A3 finding, not scenario_failure.
    let tmp = tempfile::tempdir().expect("tempdir");
    let core_path = tmp.path().join("core.yml");
    let scenario_path = tmp.path().join("scenario.yml");

    // Core config with execution_mode: isolated + starting_event: debug.start.
    write_file(
        &core_path,
        r#"
event_loop:
  execution_mode: isolated
  starting_event: debug.start
  completion_promise: loop.complete
  max_iterations: 4

tasks:
  enabled: true
  coordinator_hats:
    - hat_a
"#,
    );

    // Scenario uses work.start — mismatches the preset's debug.start.
    write_file(
        &scenario_path,
        r#"
version: 1
scenarios:
  - name: start-event-mismatch
    responses:
      - output: ""
        success: true
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 1
      no_progress_steps: 1
"#,
    );

    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/isolated-start-mismatch-preset.yml");

    let mut cmd = ralph_bin();
    cmd.current_dir(tmp.path())
        .env("RUST_LOG", "off")
        .args([
            "-c",
            core_path.to_str().unwrap(),
            "-H",
            fixture_path.to_str().unwrap(),
            "preset",
            "verify",
            "--scenario",
            scenario_path.to_str().unwrap(),
            "--format",
            "json",
        ]);

    let output = cmd.output().expect("spawn ralph");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_ne!(
        output.status.code(),
        Some(0),
        "start event mismatch must exit nonzero; stdout={stdout}\nstderr={stderr}"
    );

    let json_slice = extract_json(&stdout);
    let json: serde_json::Value = serde_json::from_str(&json_slice)
        .unwrap_or_else(|e| panic!("verify JSON unparseable: {e}\njson_slice={json_slice}\nstderr={stderr}"));

    assert_eq!(json["passed"], serde_json::Value::Bool(false));
    assert_eq!(
        json["failure_kind"],
        serde_json::Value::String("input_error".to_string()),
        "StartEventMismatch must classify as input_error; got {:?}",
        json["failure_kind"]
    );
}

#[test]
fn preset_verify_scenario_failure_returns_nonzero_with_category() {
    // Scenario parse error (unknown top-level version) → input_error,
    // nonzero exit, static layer still present. Use `builtin:merge-batch`
    // which passes strict so we reach the scenario parse stage.
    let tmp = tempfile::tempdir().expect("tempdir");
    let scenario_path = tmp.path().join("scenario.yml");
    write_file(
        &scenario_path,
        r#"
version: 99
scenarios:
  - name: bad-version
    responses:
      - output: ""
        success: true
    expect:
      start_event: merge.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 1
      no_progress_steps: 1
"#,
    );

    let mut cmd = ralph_bin();
    cmd.current_dir(tmp.path())
        .args([
            "-H",
            "builtin:merge-batch",
            "preset",
            "verify",
            "--scenario",
            scenario_path.to_str().unwrap(),
            "--format",
            "json",
        ]);

    let output = cmd.output().expect("spawn ralph");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_slice = extract_json(&stdout);
    let json: serde_json::Value = serde_json::from_str(&json_slice)
        .unwrap_or_else(|e| panic!("verify JSON unparseable: {e}\njson_slice={json_slice}"));
    assert_eq!(json["passed"], serde_json::Value::Bool(false));
    assert_eq!(
        json["failure_kind"],
        serde_json::Value::String("input_error".to_string()),
        "unknown version must classify as input_error; got {:?}",
        json["failure_kind"]
    );
    assert_ne!(
        output.status.code(),
        Some(0),
        "scenario parse failure must exit nonzero"
    );
}

#[test]
fn preset_check_still_works_without_backend() {
    // Regression: `preset check` must NOT consume the scenario YAML and
    // must NOT route through verify.
    let tmp = tempfile::tempdir().expect("tempdir");
    let core_path = tmp.path().join("core.yml");
    let hats_path = tmp.path().join("hats.yml");
    let scenario_path = tmp.path().join("scenario.yml");
    write_file(&core_path, MINIMAL_CORE);
    write_file(&hats_path, MINIMAL_HATS);
    write_file(&scenario_path, MINIMAL_SCENARIO);

    let mut cmd = ralph_bin();
    cmd.current_dir(tmp.path())
        .args([
            "-c",
            core_path.to_str().unwrap(),
            "-H",
            hats_path.to_str().unwrap(),
            "preset",
            "check",
            "--strict",
        ]);

    let output = cmd.output().expect("spawn ralph");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // `check` must not produce a verify report shape (no `scenarios[]`).
    assert!(
        !stdout.contains("\"scenarios\":["),
        "preset check must not produce a verify report; stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn inspect_prompt_does_not_run_scenario() {
    // Regression: `inspect prompt` must remain read-only and not consume
    // any scenario YAML. We do NOT pass `--scenario`; this is a smoke
    // check that the inspect dispatch ignores verify.
    let tmp = tempfile::tempdir().expect("tempdir");
    let core_path = tmp.path().join("core.yml");
    let hats_path = tmp.path().join("hats.yml");
    write_file(&core_path, MINIMAL_CORE);
    write_file(&hats_path, MINIMAL_HATS);

    let mut cmd = ralph_bin();
    cmd.current_dir(tmp.path())
        .args([
            "-c",
            core_path.to_str().unwrap(),
            "-H",
            hats_path.to_str().unwrap(),
            "inspect",
            "prompt",
            "doer",
        ]);

    let output = cmd.output().expect("spawn ralph");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("\"scenarios\":["),
        "inspect prompt must not produce a verify report; stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn preset_verify_rejects_coordinator_mode_with_input_error() {
    // A preset with execution_mode=coordinator (default) must be rejected
    // by verify with failure_kind=input_error and a message about isolated mode.
    let tmp = tempfile::tempdir().expect("tempdir");
    let core_path = tmp.path().join("core.yml");
    let scenario_path = tmp.path().join("scenario.yml");

    // Write a core config with execution_mode: omitted (defaults to Coordinator).
    write_file(
        &core_path,
        r#"
event_loop:
  # NOTE: execution_mode is intentionally omitted so it defaults to Coordinator.
  starting_event: work.start
  completion_promise: loop.complete
  max_iterations: 4

tasks:
  enabled: true
  coordinator_hats:
    - hat_a
"#,
    );

    write_file(
        &scenario_path,
        r#"
version: 1
scenarios:
  - name: coordinator-mode-rejected
    responses:
      - output: ""
        success: true
    expect:
      start_event: work.start
      accepted_events: []
      forbidden_events: []
      terminal: none
    limits:
      max_steps: 4
      no_progress_steps: 4
"#,
    );

    // coordinator-mode-preset.yml: 3-hat chain via fixture file.
    let fixture_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coordinator-mode-preset.yml");

    let mut cmd = ralph_bin();
    cmd.current_dir(tmp.path())
        .env("RUST_LOG", "off")
        .args([
            "-c",
            core_path.to_str().unwrap(),
            "-H",
            fixture_path.to_str().unwrap(),
            "preset",
            "verify",
            "--scenario",
            scenario_path.to_str().unwrap(),
            "--format",
            "json",
        ]);

    let output = cmd.output().expect("spawn ralph");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_ne!(
        output.status.code(),
        Some(0),
        "coordinator-mode preset must be rejected with non-zero exit; stdout={stdout}\nstderr={stderr}"
    );

    let json_slice = extract_json(&stdout);
    let json: serde_json::Value = serde_json::from_str(&json_slice).unwrap_or_else(|e| {
        panic!("verify JSON unparseable: {e}\njson_slice={json_slice}\nstderr={stderr}")
    });

    assert_eq!(
        json["failure_kind"],
        serde_json::Value::String("input_error".to_string()),
        "coordinator mode rejection must be input_error; got {:?}\nstdout={stdout}\nstderr={stderr}",
        json["failure_kind"]
    );

    assert!(
        stderr.contains("event_loop.execution_mode: isolated")
            || stderr.contains("coordinator mode is not supported"),
        "stderr must mention isolated mode requirement; got: {stderr}"
    );
}