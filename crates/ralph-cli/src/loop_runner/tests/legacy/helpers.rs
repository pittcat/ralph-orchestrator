// Auto-extracted helpers shared across legacy submodules. The original
// legacy.rs defined these inline next to their callers; the split preserves
// call-site transparency by re-exporting every helper here.
//
// The full original import set is reproduced verbatim so that helpers can
// reference every item callers might need.

#![allow(unused_imports)]

use super::super::super::*;
use super::super::common::*;
use super::super::fake_path::*;
use crate::test_support::CwdGuard;
use ralph_core::HatRegistry;
use ralph_core::planning_session::{ConversationEntry, ConversationType};
use ralph_proto::{Hat, Topic};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::sync::{Arc, Mutex};

// Helper: fn make_event_loop_for_recovery_test
pub(crate) fn make_event_loop_for_recovery_test() -> EventLoop {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["task.*"]
    publishes: ["work.done", "work.failed"]
  reviewer:
    name: "Reviewer"
    triggers: ["work.done"]
    publishes: ["review.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    EventLoop::new(config)
}

// Helper: fn u4_workspace
#[cfg(unix)]
pub(crate) fn u4_workspace() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().to_path_buf();
    (temp, root)
}

// Helper: fn u4_session_dir
#[cfg(unix)]
pub(crate) fn u4_session_dir(workspace_root: &Path) -> std::path::PathBuf {
    let mut session_dirs: Vec<_> = std::fs::read_dir(workspace_root.join(".ralph/diagnostics"))
        .expect("read diagnostics dir")
        .filter_map(Result::ok)
        .collect();
    session_dirs.sort_by_key(|entry| entry.path());
    session_dirs
        .last()
        .expect("at least one diagnostics session should exist")
        .path()
}

// Helper: fn u4_recovery_journal
#[cfg(unix)]
pub(crate) fn u4_recovery_journal(
    workspace_root: &Path,
) -> Vec<ralph_core::diagnosis::RecoveryJournalEntry> {
    let path = u4_session_dir(workspace_root).join("recovery.jsonl");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: path={}", path.display()));
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse recovery.jsonl line"))
        .collect()
}

// Helper: fn u4_orchestration_log
#[cfg(unix)]
pub(crate) fn u4_orchestration_log(workspace_root: &Path) -> std::path::PathBuf {
    u4_session_dir(workspace_root).join("orchestration.jsonl")
}

// Helper: fn u4_orchestration_has_recovery_diagnosed
#[cfg(unix)]
pub(crate) fn u4_orchestration_has_recovery_diagnosed(
    workspace_root: &Path,
    diagnosis_id: &str,
) -> bool {
    let path = u4_orchestration_log(workspace_root);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    content
        .lines()
        .any(|line| line.contains("\"type\":\"recovery_diagnosed\"") && line.contains(diagnosis_id))
}

// Helper: fn build_u8_event_loop
pub(crate) fn build_u8_event_loop(
    workspace: std::path::PathBuf,
    diagnostics_enabled: bool,
) -> ralph_core::EventLoop {
    let config = ralph_core::RalphConfig::default();
    let ctx = ralph_core::LoopContext::primary(workspace);
    let collector = if diagnostics_enabled {
        // Bypass `RALPH_DIAGNOSTICS` env so the test is hermetic;
        // `with_enabled(_, true)` is the same path U0 takes when the
        // operator sets the env var.
        ralph_core::diagnostics::DiagnosticsCollector::with_enabled(
            &ctx.workspace().join(".ralph"),
            true,
        )
        .expect("diagnostics collector must initialize in tmpdir")
    } else {
        ralph_core::diagnostics::DiagnosticsCollector::disabled()
    };
    ralph_core::EventLoop::with_context_and_diagnostics(config, ctx, collector)
        .expect("U13: archive must succeed for fresh-loop tests")
}

// Helper: fn u2_make_n_hat_config
pub(crate) fn u2_make_n_hat_config(n: usize, mode_yaml: &str) -> ralph_core::RalphConfig {
    let mut hats_yaml = String::new();
    for i in 0..n {
        if i > 0 {
            hats_yaml.push('\n');
        }
        // Last hat publishes the completion promise directly so
        // its R3 egress closes. Earlier hats publish to the
        // *next* hat's trigger so the chain handoff fires.
        let publishes = if i + 1 == n {
            "\"work.done\"".to_string()
        } else {
            format!("\"handoff.to.h{}\"", i + 1)
        };
        let triggers = if i == 0 {
            "[\"work.start\"]".to_string()
        } else {
            format!("[\"handoff.to.h{i}\"]")
        };
        hats_yaml.push_str(&format!(
            "  h{i}:\n    name: \"H{i}\"\n    description: \"Hat {i}\"\n    triggers: {triggers}\n    publishes: [{publishes}]\n    instructions: \"Do hat {i}.\""
        ));
    }
    let yaml = format!(
        r#"
hats:
{hats_yaml}
event_loop:
  starting_event: "work.start"
  completion_promise: "work.done"
  {mode_yaml}
tasks:
  enabled: false
"#
    );
    serde_yaml::from_str(&yaml).expect("parse test config")
}

// Helper: fn make_wave_aggregator_topology
#[cfg(unix)]
pub(crate) fn make_wave_aggregator_topology() -> ralph_core::RalphConfig {
    // Two-hat topology, both non-isolated so the test focuses on
    // wait_for_all semantics:
    //   - `dispatcher` triggers `review.start` and publishes
    //     `review.perspective` (a wave trigger).
    //   - `worker` (concurrency: 2) is the wave target hat, triggered
    //     by `review.perspective`, publishes `review.done` — the
    //     aggregator trigger.
    //   - `aggregator` (wait_for_all) collects `review.done` events.
    let yaml = r#"
hats:
  dispatcher:
    name: "Dispatcher"
    triggers: ["review.start"]
    publishes: ["review.perspective"]
    instructions: "Dispatch wave."
  worker:
    name: "Worker"
    triggers: ["review.perspective"]
    publishes: ["review.done"]
    concurrency: 2
    instructions: "Emit review.done."
  aggregator:
    name: "Aggregator"
    triggers: ["review.done"]
    publishes: ["aggregate.complete"]
    instructions: "AGGREGATOR MODE - aggregate all review.done."
    aggregate:
      mode: wait_for_all
      timeout: 60
"#;
    serde_yaml::from_str(yaml).expect("aggregator topology yaml should parse")
}

// Helper: fn make_wave_with_count
#[cfg(unix)]
pub(crate) fn make_wave_with_count(
    wave_id: &str,
    total: u32,
    publishes: Vec<String>,
) -> ralph_core::DetectedWave {
    use ralph_core::Event;
    let events: Vec<Event> = (0..total)
        .map(|i| Event {
            topic: "review.perspective".to_string(),
            payload: Some(format!("dimension-{i}")),
            ts: "2026-01-01T00:00:00Z".to_string(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: Some(wave_id.to_string()),
            wave_index: Some(i),
            wave_total: Some(total),
            system_injected: None,
        })
        .collect();
    ralph_core::DetectedWave {
        wave_id: wave_id.to_string(),
        target_hat: "worker".into(),
        hat_config: ralph_core::HatConfig {
            name: "Worker".to_string(),
            description: Some("Wave worker".to_string()),
            triggers: vec!["review.perspective".to_string()],
            publishes,
            terminal_events: vec![],
            instructions: "Emit review.done when finished.".to_string(),
            extra_instructions: vec![],
            backend: None,
            backend_args: None,
            default_publishes: None,
            max_activations: None,
            disallowed_tools: vec![],
            timeout: Some(30),
            // 2026-07-25-006 U4 (R2/R3): idle heartbeat fields
            // stay `None` here so the legacy timeout shape is
            // not accidentally reinterpreted as lease-enabled.
            idle_heartbeat_secs: None,
            idle_weak_signal_cap: None,
            // 2026-07-28-003 plan U3 (R1): default None keeps
            // the existing legacy-wave fixtures bit-for-bit
            // identical to the pre-U3 behaviour.
            startup_grace_secs: None,
            // 2026-06-17-004 U2 (R3): explicit `None` for new
            // field keeps the test helper aligned with
            // `HatConfig::default()`.
            missing_event_grace_secs: None,
            concurrency: 2,
            aggregate: None,
            scratchpad: None,
            event_filter: None,
            // 2026-06-26 plan U2: test fixture does not exercise
            // the exempt list; default empty.
            exempt_topics: vec![],
            // 2026-06-29-007 plan U5a: test fixture does not
            // exercise write paths; default `None` mirrors
            // production default.
            allowed_write_paths: None,
            phase_triggers: None,
            ignore_payload_fields: vec![],
            obligations: vec![],
            trigger_multi_consumer_topics: HashSet::new(),
        },
        events,
        total,
        partial: false,
        consumer_aggregate_timeout: None,
    }
}

// Helper: fn install_simple_worker_backend
#[cfg(unix)]
pub(crate) fn install_simple_worker_backend(temp_dir: &std::path::Path) -> std::path::PathBuf {
    // P2 finding #7: reuse `write_fake_executable` so the U3 worker
    // backend installs the same way as the legacy fake backends.
    // The script body is a single self-contained bash heredoc; the
    // fake_executable wrapper adds the shebang and chmod.  We keep
    // the bin/ subdirectory the original code created so the
    // per-test layout is unchanged.
    let bin_dir = temp_dir.join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let body = r#"set -u
if [ -z "${RALPH_EVENTS_FILE:-}" ]; then
  echo 'no RALPH_EVENTS_FILE' >&2
  exit 2
fi
cat > "$RALPH_EVENTS_FILE" <<PEOF
{"topic":"review.done","payload":"dim-${RALPH_WAVE_INDEX:-0}-result","ts":"2026-01-01T00:00:00Z","wave_id":"${RALPH_WAVE_ID:-w-default}","wave_index":${RALPH_WAVE_INDEX:-0},"wave_total":${RALPH_WAVE_TOTAL:-0},"hat":"${RALPH_CURRENT_HAT:-}","source":"${RALPH_CURRENT_HAT:-}"}
PEOF
exit 0
"#;
    write_fake_executable(&bin_dir, "wave-worker", body)
}

// Helper: struct WaveTestSetup
#[cfg(unix)]
pub(crate) struct WaveTestSetup {
    pub(crate) _temp: tempfile::TempDir,
    pub(crate) workspace: std::path::PathBuf,
    pub(crate) event_loop: ralph_core::EventLoop,
    pub(crate) events_file: std::path::PathBuf,
    pub(crate) backend: ralph_adapters::CliBackend,
}

// Helper: fn setup_wave_test
#[cfg(unix)]
pub(crate) fn setup_wave_test() -> WaveTestSetup {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().to_path_buf();
    init_git_workspace(&workspace);

    let config = make_wave_aggregator_topology();
    let loop_ctx = ralph_core::LoopContext::primary(workspace.clone());
    let event_loop = ralph_core::EventLoop::with_context(config, loop_ctx);

    let events_dir = workspace.join(".ralph");
    std::fs::create_dir_all(&events_dir).expect("ralph dir");
    let events_file = events_dir.join("events.jsonl");
    std::fs::write(&events_file, "").expect("empty events");

    let worker_path = install_simple_worker_backend(&workspace);
    let backend = ralph_adapters::CliBackend {
        command: worker_path.display().to_string(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format: ralph_adapters::OutputFormat::Text,
        env_vars: vec![],
    };

    WaveTestSetup {
        _temp: temp,
        workspace,
        event_loop,
        events_file,
        backend,
    }
}

// Helper: fn init_git_workspace
#[cfg(unix)]
pub(crate) fn init_git_workspace(workspace: &std::path::Path) {
    use std::process::Command;
    let run = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .unwrap_or_else(|e| panic!("git {args:?} failed: {e}"))
    };
    run(&["init", "--initial-branch=main"]);
    run(&["config", "user.email", "test@test.local"]);
    run(&["config", "user.name", "Test User"]);
    std::fs::write(workspace.join(".gitignore"), ".ralph/\n").unwrap();
    std::fs::write(workspace.join("README.md"), "# Test\n").unwrap();
    run(&["add", ".gitignore", "README.md"]);
    run(&["commit", "-m", "init"]);
}

// Helper: fn u5_stage_events_file
pub(crate) fn u5_stage_events_file(workspace: &Path, file_name: &str) -> (LoopContext, PathBuf) {
    let ctx = LoopContext::primary(workspace.to_path_buf());
    let ralph_dir = ctx.ralph_dir();
    std::fs::create_dir_all(&ralph_dir).expect("create .ralph dir");
    let relative = format!(".ralph/{file_name}");
    std::fs::write(ctx.current_events_marker(), &relative).expect("write marker");
    let absolute = ctx.workspace().join(&relative);
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent).expect("create events parent");
    }
    (ctx, absolute)
}

// Helper: fn build_isolated_event_loop
pub(crate) fn build_isolated_event_loop(
    config: ralph_core::RalphConfig,
    hat_label: Option<&str>,
) -> EventLoop {
    let mut el = EventLoop::new(config);
    if let Some(label) = hat_label {
        el.state_mut().last_hat = Some(HatId::new(label));
    }
    el
}

// Helper: fn seed_hat_channel
pub(crate) fn seed_hat_channel(
    ctx: &ralph_core::LoopContext,
    hat: &str,
    loop_id: &str,
    iteration: u32,
    contents: &str,
) -> std::path::PathBuf {
    let channel_path =
        crate::loop_runner::paths::hat_channel_events_path(ctx, hat, loop_id, iteration);
    if let Some(parent) = channel_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&channel_path, contents).unwrap();
    // Marker format matches `prepare_hat_channel`: a workspace-relative path.
    let relative = format!(".ralph/agent/events-hat-{hat}-{loop_id}-{iteration}.jsonl");
    std::fs::write(
        crate::loop_runner::paths::current_hat_events_marker(ctx),
        relative,
    )
    .unwrap();
    channel_path
}
