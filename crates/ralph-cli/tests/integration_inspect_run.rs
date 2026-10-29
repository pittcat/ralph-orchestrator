//! Integration tests for `ralph inspect run` (run-observation/v1)
//! and the run manifest evidence file
//! (`.ralph/runs/<run_id>.manifest.json`).
//!
//! feat(monitor) foundation plan (2026-10-28-1949) Unit 1.

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

fn write_run_fixture(temp_path: &std::path::Path, home_path: &std::path::Path, sleep_secs: u64) {
    let user_config_dir = home_path.join(".ralph");
    fs::create_dir_all(&user_config_dir).expect("create user config dir");
    fs::write(
        user_config_dir.join("config.yml"),
        "cli:\n  backend: custom\n",
    )
    .expect("write user config");

    let backend_script = temp_path.join("backend.sh");
    fs::write(
        &backend_script,
        format!(
            "#!/bin/sh\ncat >/dev/null\nsleep {sleep_secs}\n\"{}\" emit LOOP_COMPLETE inspect-done\n",
            env!("CARGO_BIN_EXE_ralph")
        ),
    )
    .expect("write backend script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&backend_script)
            .expect("metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&backend_script, permissions).expect("set executable permissions");
    }

    fs::write(
        temp_path.join("ralph.yml"),
        r#"
cli:
  backend: custom
  command: "./backend.sh"
  prompt_mode: stdin
event_loop:
  max_iterations: 3
  max_runtime_seconds: 30
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: false
"#,
    )
    .expect("write local config");
}

fn find_manifest_file(temp_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let runs_dir = temp_path.join(".ralph/runs");
    fs::read_dir(&runs_dir).ok()?.find_map(|entry| {
        let path = entry.ok()?.path();
        let is_manifest = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".manifest.json"))
            .unwrap_or(false);
        is_manifest.then_some(path)
    })
}

#[test]
fn test_inspect_run_empty_workspace() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let json = inspect_run_json(temp_path);
    assert_eq!(json["schema_version"], "run-observation/v1");
    assert_eq!(
        json["workspace"].as_str().unwrap(),
        fs::canonicalize(temp_path).unwrap().to_string_lossy()
    );
    assert_eq!(json["runs"], Value::Array(vec![]));
}

#[test]
fn test_run_writes_manifest_then_inspect_reports_running_and_exited() {
    let temp_dir = TempDir::new().expect("temp dir");
    let home_dir = TempDir::new().expect("temp home");
    let temp_path = temp_dir.path();
    write_run_fixture(temp_path, home_dir.path(), 5);

    // Spawn `ralph run` in the background so we can inspect mid-run.
    let mut child = common::ralph_bin()
        .args(["run", "--no-tui", "--prompt", "manifest evidence prompt"])
        .current_dir(temp_path)
        .env("HOME", home_dir.path())
        .env("USERPROFILE", home_dir.path())
        .spawn()
        .expect("spawn ralph run");

    // Wait for the manifest to appear (poll, bounded).
    let mut manifest_path = None;
    for _ in 0..100 {
        if let Some(p) = find_manifest_file(temp_path) {
            manifest_path = Some(p);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let manifest_path = manifest_path.expect("manifest written while loop is starting");

    // Manifest schema + identity invariants.
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).expect("read manifest"))
            .expect("manifest is valid JSON");
    assert_eq!(manifest["schema_version"], "run-manifest/v1");
    let run_id = manifest["run_id"].as_str().unwrap().to_string();
    assert!(
        run_id.starts_with("primary-"),
        "primary loop must use stable primary-* run_id, got {run_id}"
    );
    assert_eq!(manifest["kind"], "primary");
    assert_eq!(manifest["pid"].as_u64().unwrap() as u32, child.id());
    assert_eq!(manifest["revision"], 1);
    assert_eq!(manifest["prompt_preview"], "manifest evidence prompt");
    assert!(
        manifest["prompt_sha256"].as_str().unwrap().len() == 64,
        "prompt_sha256 must be a sha256 hex digest"
    );
    assert!(
        manifest["events_file"].as_str().is_some(),
        "manifest records the events file path"
    );
    assert!(manifest["terminal"].is_null());

    // Mid-run observation: state=running, terminal absent.
    let running = inspect_run_json(temp_path);
    let run = &running["runs"][0];
    assert_eq!(run["run_id"], run_id);
    assert_eq!(run["kind"], "primary");
    assert_eq!(run["state"], "running");
    assert_eq!(run["evidence"]["manifest_present"], true);
    assert!(run["terminal"].is_null() || run.get("terminal").is_none());

    let status = child.wait().expect("wait for ralph run");
    assert!(
        status.success(),
        "ralph run should exit 0 after LOOP_COMPLETE"
    );

    // Post-run observation: manifest now carries terminal evidence.
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).expect("read manifest"))
            .expect("manifest is valid JSON");
    let terminal = &manifest["terminal"];
    assert_eq!(terminal["reason"], "completed");
    assert!(terminal["at"].as_str().is_some());
    assert_eq!(manifest["revision"], 2);

    let exited = inspect_run_json(temp_path);
    let run = &exited["runs"][0];
    assert_eq!(run["run_id"], run_id);
    assert_eq!(run["state"], "exited");
    assert_eq!(run["terminal"]["reason"], "completed");
    assert!(run["started_at"].as_str().is_some());
    assert!(run["manifest"].as_str().unwrap().contains(".manifest.json"));
}

#[test]
fn test_inspect_run_legacy_lock_without_manifest_reports_unknown() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    // Fabricate a legacy workspace: dead-pid loop.lock + current-loop-id,
    // but no manifest — pre-manifest runs or crash leftovers.
    let ralph_dir = temp_path.join(".ralph");
    fs::create_dir_all(&ralph_dir).expect("create .ralph");
    fs::write(
        ralph_dir.join("loop.lock"),
        r#"{"pid": 99999999, "started": "2026-10-28T10:00:00Z", "prompt": "legacy prompt"}"#,
    )
    .expect("write loop.lock");
    fs::write(ralph_dir.join("current-loop-id"), "primary-20261028-100000")
        .expect("write current-loop-id");

    let json = inspect_run_json(temp_path);
    let runs = json["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run["run_id"], "primary-20261028-100000");
    assert_eq!(run["kind"], "primary");
    assert_eq!(run["state"], "unknown");
    assert_eq!(run["pid"], 99999999);
    assert_eq!(run["prompt_preview"], "legacy prompt");
    assert!(run["evidence"]["lock_present"].as_bool().unwrap());
    assert!(
        run["evidence"]["manifest_present"].as_bool() == Some(false)
            || run["evidence"]["manifest_present"].is_null()
    );
}

#[test]
fn test_inspect_run_corrupt_manifest_flagged_in_evidence() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let runs_dir = temp_path.join(".ralph/runs");
    fs::create_dir_all(&runs_dir).expect("create runs dir");
    fs::write(
        runs_dir.join("loop-broken.manifest.json"),
        "{not valid json",
    )
    .expect("write corrupt manifest");

    let json = inspect_run_json(temp_path);
    let runs = json["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run["run_id"], "loop-broken");
    assert_eq!(run["state"], "unknown");
    assert_eq!(run["evidence"]["manifest_corrupt"], true);
}

#[test]
fn test_inspect_run_human_format_lists_runs() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();

    let ralph_dir = temp_path.join(".ralph");
    fs::create_dir_all(&ralph_dir).expect("create .ralph");
    fs::write(ralph_dir.join("current-loop-id"), "primary-20261028-100000")
        .expect("write current-loop-id");

    let output = run_ralph(temp_path, &["inspect", "run"]);
    assert!(
        output.status.success(),
        "inspect run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("primary-20261028-100000"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("unknown"), "stdout: {stdout}");
}

fn write_manifest(temp_path: &std::path::Path, run_id: &str, preset: Option<&str>) {
    let runs_dir = temp_path.join(".ralph/runs");
    fs::create_dir_all(&runs_dir).expect("create runs dir");
    let preset_field = preset
        .map(|p| format!(r#", "preset": "{p}""#))
        .unwrap_or_default();
    fs::write(
        runs_dir.join(format!("{run_id}.manifest.json")),
        format!(
            r#"{{"schema_version": "run-manifest/v1",
  "run_id": "{run_id}",
  "kind": "primary",
  "pid": 99999999,
  "cwd": "{}",
  "workspace": "{}",
  "repo_root": "{}",
  "prompt_preview": "descriptor test",
  "prompt_sha256": "{:0>64}",
  "started_at": "2026-10-28T10:00:00Z",
  "revision": 2{preset_field},
  "terminal": {{"reason": "completed", "at": "2026-10-28T11:00:00Z"}}}}"#,
            temp_path.display(),
            temp_path.display(),
            temp_path.display(),
            "0"
        ),
    )
    .expect("write manifest");
}

#[test]
fn test_inspect_run_builtin_preset_descriptor_resolves() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_manifest(temp_path, "primary-desc", Some("ce-executor-pipeline"));

    let json = inspect_run_json(temp_path);
    let run = &json["runs"][0];
    let desc = &run["preset_descriptor"];
    assert!(
        desc.is_object(),
        "expected preset_descriptor object, got {desc}"
    );
    assert_eq!(desc["schema_version"], "monitor-descriptor/v1");
    assert_eq!(desc["resolved_from"], "builtin");
    assert_eq!(desc["preset"], "ce-executor-pipeline");
    assert_eq!(desc["stages"].as_array().unwrap().len(), 8);
    assert_eq!(desc["stages"][0]["id"], "plan");
    assert_eq!(desc["progress"]["denominator"], "stages");
    assert_eq!(desc["dag"]["enabled"], false);
    assert_eq!(run["preset_descriptor_from"], "builtin");
}

#[test]
fn test_inspect_run_unknown_preset_descriptor_null() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_manifest(temp_path, "primary-nodesc", Some("no-such-preset"));

    let json = inspect_run_json(temp_path);
    let run = &json["runs"][0];
    assert!(run["preset_descriptor"].is_null());
    assert_eq!(run["preset_descriptor_from"], "none");
}

#[test]
fn test_inspect_run_workspace_descriptor_override() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_manifest(temp_path, "primary-custom", Some("custom-flow"));
    let monitor_dir = temp_path.join(".ralph/monitor");
    fs::create_dir_all(&monitor_dir).expect("create monitor dir");
    fs::write(
        monitor_dir.join("custom-flow.yml"),
        r#"schema_version: monitor-descriptor/v1
preset: custom-flow
stages:
  - id: only
    label: Only
    topics: [t.a]
progress:
  denominator: stages
artifacts: []
dag:
  enabled: false
"#,
    )
    .expect("write custom descriptor");

    let json = inspect_run_json(temp_path);
    let run = &json["runs"][0];
    assert_eq!(run["preset_descriptor"]["resolved_from"], "workspace");
    assert_eq!(run["preset_descriptor"]["stages"][0]["id"], "only");
    assert_eq!(run["preset_descriptor_from"], "workspace");
}

#[test]
fn test_inspect_run_no_dag_db_omits_scheduler_block() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_manifest(temp_path, "primary-wave", Some("autoresearch"));

    let json = inspect_run_json(temp_path);
    let run = &json["runs"][0];
    assert!(
        run.get("scheduler").is_none(),
        "wave runs must not emit a scheduler key, got {run}"
    );
}

#[test]
fn test_inspect_run_dag_db_present_emits_scheduler_mode_marker() {
    let temp_dir = TempDir::new().expect("temp dir");
    let temp_path = temp_dir.path();
    write_manifest(temp_path, "primary-dag", Some("parallel-forge"));
    // dag.db presence alone is the non-wave signal; an unreadable /
    // schema-less store still surfaces the mode marker (fail-soft
    // passthrough — never aborts the read-only inspect).
    let ralph_dir = temp_path.join(".ralph");
    fs::create_dir_all(&ralph_dir).expect("create .ralph");
    fs::write(ralph_dir.join("dag.db"), b"not-a-database").expect("dag.db");

    let json = inspect_run_json(temp_path);
    let run = &json["runs"][0];
    assert_eq!(run["scheduler"]["scheduler_mode"], "dag");
}
