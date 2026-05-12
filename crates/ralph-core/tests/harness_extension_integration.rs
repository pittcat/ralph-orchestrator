//! Integration tests for harness extensions (FR-1 through FR-4).
//!
//! Validates that event filtering, event projection, state file injection,
//! and preflight extension hooks work independently, in combination, and
//! without regressing baseline behavior.

use ralph_core::{EventLoop, RalphConfig};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tempfile::TempDir;

fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

fn safe_current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| {
        let fallback = std::env::temp_dir();
        std::env::set_current_dir(&fallback).expect("set fallback cwd");
        fallback
    })
}

struct CwdGuard {
    _lock: MutexGuard<'static, ()>,
    original: PathBuf,
}

impl CwdGuard {
    fn set(path: &Path) -> Self {
        let lock = test_lock();
        let original = safe_current_dir();
        std::env::set_current_dir(path).expect("set current dir");
        Self {
            _lock: lock,
            original,
        }
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
    }
}

// =============================================================================
// 1. Baseline regression test
// =============================================================================

#[test]
fn test_baseline_no_extensions() {
    let temp_dir = TempDir::new().unwrap();
    let _cwd = CwdGuard::set(temp_dir.path());

    let yaml = r#"
core:
  scratchpad: ".ralph/agent/scratchpad.md"
  specs_dir: "./specs"
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;

    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);

    event_loop.initialize("test objective");
    let prompt = event_loop.build_prompt(&ralph_proto::HatId::new("ralph")).unwrap();

    assert!(prompt.contains("You are Ralph"), "Prompt should identify Ralph");
    assert!(
        prompt.contains("LOOP_COMPLETE"),
        "Prompt should contain completion promise"
    );
}

// =============================================================================
// 2. FR-1: Event filter integration test
// =============================================================================

#[test]
fn test_fr1_event_filter_prompt_content() {
    let temp_dir = TempDir::new().unwrap();
    let ralph_dir = temp_dir.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).unwrap();

    let events_file = ralph_dir.join("events.jsonl");
    fs::write(
        &events_file,
        r#"{"topic":"review.file","payload":"src/main.rs","ts":"2026-01-14T12:00:00Z"}
{"topic":"build.done","payload":"Build passed","ts":"2026-01-14T12:00:01Z"}
{"topic":"review.complete","payload":"Review finished","ts":"2026-01-14T12:00:02Z"}
"#,
    )
    .unwrap();

    let yaml = r#"
core:
  scratchpad: ".ralph/agent/scratchpad.md"
  specs_dir: "./specs"
hats:
  reviewer:
    name: "Reviewer"
    description: "Reviews code"
    triggers: ["review.file"]
    publishes: ["review.complete"]
    event_filter:
      enabled: true
      mode: allowlist
      events:
        - "review.file"
        - "review.complete"
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;

    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let _cwd = CwdGuard::set(temp_dir.path());

    event_loop.process_events_from_jsonl().unwrap();
    event_loop.initialize("test objective");

    let prompt = event_loop
        .build_prompt(&ralph_proto::HatId::new("ralph"))
        .unwrap();

    // Allowlisted events should appear in the prompt context.
    assert!(
        prompt.contains("Event: review.file"),
        "Prompt should contain allowlisted review.file event"
    );
    assert!(
        prompt.contains("Event: review.complete"),
        "Prompt should contain allowlisted review.complete event"
    );

    // Non-allowlisted event should be filtered out.
    assert!(
        !prompt.contains("Event: build.done"),
        "Prompt should NOT contain non-allowlisted build.done event"
    );
}

// =============================================================================
// 3. FR-2: Event projection integration test
// =============================================================================

#[test]
fn test_fr2_event_projection_creates_jsonl() {
    let temp_dir = TempDir::new().unwrap();
    let ralph_dir = temp_dir.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).unwrap();

    let events_file = ralph_dir.join("events.jsonl");
    fs::write(
        &events_file,
        r#"{"topic":"experiment.done","payload":"{\"result\":\"success\"}","ts":"2026-01-14T12:00:00Z"}
"#,
    )
    .unwrap();

    let yaml = r#"
core:
  scratchpad: ".ralph/agent/scratchpad.md"
  specs_dir: "./specs"
  event_projection:
    enabled: true
    rules:
      - name: "experiment-log"
        trigger_events: ["experiment.done"]
        fields: ["topic", "payload"]
        target_file: ".ralph/experiments.jsonl"
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;

    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    let _cwd = CwdGuard::set(temp_dir.path());

    event_loop.process_events_from_jsonl().unwrap();

    let projection_path = temp_dir.path().join(".ralph/experiments.jsonl");
    assert!(
        projection_path.exists(),
        "Projection file should be created"
    );

    let content = fs::read_to_string(&projection_path).unwrap();
    let line: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    assert_eq!(line["topic"], "experiment.done");
    assert_eq!(line["payload"], r#"{"result":"success"}"#);
}

// =============================================================================
// 4. FR-3: State file injection integration test
// =============================================================================

#[test]
fn test_fr3_state_file_injected_into_prompt() {
    let temp_dir = TempDir::new().unwrap();
    let ralph_dir = temp_dir.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).unwrap();

    let state_file = ralph_dir.join("state.json");
    fs::write(&state_file, r#"{"status":"ok","count":42}"#).unwrap();

    let yaml = r#"
core:
  scratchpad: ".ralph/agent/scratchpad.md"
  specs_dir: "./specs"
  state_files:
    enabled: true
    files:
      - path: ".ralph/state.json"
        format: json
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;

    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    let _cwd = CwdGuard::set(temp_dir.path());

    event_loop.initialize("test objective");
    let prompt = event_loop
        .build_prompt(&ralph_proto::HatId::new("ralph"))
        .unwrap();

    assert!(
        prompt.contains(r#"<state-file name=".ralph/state.json" format="json">"#),
        "Prompt should contain state-file XML opening tag"
    );
    assert!(
        prompt.contains(r#""status":"ok""#),
        "Prompt should contain state file content"
    );
    assert!(
        prompt.contains("</state-file>"),
        "Prompt should contain state-file closing tag"
    );
}

// =============================================================================
// 5. FR-4: Preflight extension hook integration test
// =============================================================================

#[tokio::test]
async fn test_fr4_preflight_hook_executes() {
    let temp_dir = TempDir::new().unwrap();

    let yaml = r#"
core:
  scratchpad: ".ralph/agent/scratchpad.md"
  specs_dir: "./specs"
  preflight_extensions:
    enabled: true
    hooks:
      - name: "hello-hook"
        command: "echo 'hello from hook'"
        stage: after_native
        fail_on_error: false
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;

    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = temp_dir.path().to_path_buf();

    let runner = ralph_core::PreflightRunner::default_checks(&config);
    let report = runner.run_all(&config).await;

    let hook_result = report
        .checks
        .iter()
        .find(|c| c.name == "hello-hook")
        .expect("Hook result should be present in report");

    assert_eq!(
        hook_result.status,
        ralph_core::CheckStatus::Pass,
        "Hook should pass: {:?}",
        hook_result.message
    );
    assert!(
        hook_result.label.contains("Hook 'hello-hook' passed"),
        "Hook label should indicate success: {}",
        hook_result.label
    );
}

// =============================================================================
// 6. Combination test: all extensions enabled together
// =============================================================================

#[test]
fn test_all_extensions_enabled_together() {
    let temp_dir = TempDir::new().unwrap();
    let ralph_dir = temp_dir.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).unwrap();

    let state_file = ralph_dir.join("state.json");
    fs::write(&state_file, r#"{"mode":"active"}"#).unwrap();

    let events_file = ralph_dir.join("events.jsonl");
    fs::write(
        &events_file,
        r#"{"topic":"experiment.done","payload":"done","ts":"2026-01-14T12:00:00Z"}
{"topic":"review.file","payload":"src/lib.rs","ts":"2026-01-14T12:00:01Z"}
"#,
    )
    .unwrap();

    let yaml = r#"
core:
  scratchpad: ".ralph/agent/scratchpad.md"
  specs_dir: "./specs"
  event_projection:
    enabled: true
    rules:
      - name: "experiment-log"
        trigger_events: ["experiment.done"]
        fields: ["topic"]
        target_file: ".ralph/experiments.jsonl"
  state_files:
    enabled: true
    files:
      - path: ".ralph/state.json"
        format: json
  preflight_extensions:
    enabled: true
    hooks:
      - name: "noop"
        command: "true"
        stage: before_native
        fail_on_error: false
hats:
  reviewer:
    name: "Reviewer"
    description: "Reviews code"
    triggers: ["review.file"]
    publishes: ["review.done"]
    event_filter:
      enabled: true
      mode: allowlist
      events:
        - "review.file"
        - "review.done"
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;

    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    let _cwd = CwdGuard::set(temp_dir.path());

    // Should not panic during event processing.
    event_loop.process_events_from_jsonl().unwrap();

    // Should not panic during prompt build.
    event_loop.initialize("test objective");
    let prompt = event_loop
        .build_prompt(&ralph_proto::HatId::new("ralph"))
        .unwrap();

    // State file should be injected.
    assert!(
        prompt.contains("<state-file"),
        "Prompt should contain injected state files"
    );

    // Projection should have created the target file.
    assert!(
        temp_dir.path().join(".ralph/experiments.jsonl").exists(),
        "Projection file should be created"
    );
}

// =============================================================================
// 7. FR-2 + FR-3 closed-loop test
// =============================================================================

#[test]
fn test_fr2_projection_and_fr3_injection_loop() {
    let temp_dir = TempDir::new().unwrap();
    let ralph_dir = temp_dir.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).unwrap();

    let events_file = ralph_dir.join("events.jsonl");
    fs::write(
        &events_file,
        r#"{"topic":"experiment.done","payload":"success","ts":"2026-01-14T12:00:00Z"}
"#,
    )
    .unwrap();

    let yaml = r#"
core:
  scratchpad: ".ralph/agent/scratchpad.md"
  specs_dir: "./specs"
  event_projection:
    enabled: true
    rules:
      - name: "experiment-log"
        trigger_events: ["experiment.done"]
        fields: ["topic", "payload"]
        target_file: ".ralph/experiments.jsonl"
  state_files:
    enabled: true
    files:
      - path: ".ralph/experiments.jsonl"
        format: jsonl
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;

    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = temp_dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    let _cwd = CwdGuard::set(temp_dir.path());

    event_loop.process_events_from_jsonl().unwrap();
    event_loop.initialize("test objective");
    let prompt = event_loop
        .build_prompt(&ralph_proto::HatId::new("ralph"))
        .unwrap();

    // The projected content should be injected back into the prompt as a state file.
    assert!(
        prompt.contains(r#"<state-file name=".ralph/experiments.jsonl" format="jsonl">"#),
        "Prompt should contain projected file as state-file"
    );
    assert!(
        prompt.contains("success"),
        "Prompt should contain projected data"
    );
    assert!(
        prompt.contains("experiment.done"),
        "Prompt should contain projected event topic"
    );
}
