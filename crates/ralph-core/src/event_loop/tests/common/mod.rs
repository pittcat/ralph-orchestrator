//! Shared test helpers and mock services for event_loop tests.

use super::*;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// P2 finding #8: shared `init_git_workspace` helper. Both
/// `isolated_complex_regression.rs` and the ralph-cli `loop_runner`
/// tests had near-identical copies of this routine; consolidating
/// here avoids drift. The function takes a writable directory
/// (typically a `tempfile::TempDir` path) and turns it into a
/// minimal git repo with one commit, so the loop's
/// `check_termination()` workspace-validity path returns OK.
pub(super) fn init_git_workspace(workspace: &Path) {
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

/// Helper to write an event to a JSONL file for testing.
pub(super) fn write_event_to_jsonl(path: &std::path::Path, topic: &str, payload: &str) {
    use std::io::Write;
    let ts = chrono::Utc::now().to_rfc3339();
    let event_json = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": ts
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{}", event_json).unwrap();
}

/// Like [`write_event_to_jsonl`] but includes hat provenance for origin guard compatibility.
pub(super) fn write_event_with_hat_to_jsonl(
    path: &std::path::Path,
    topic: &str,
    payload: &str,
    hat: &str,
) {
    use std::io::Write;
    let ts = chrono::Utc::now().to_rfc3339();
    let event_json = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": ts,
        "hat": hat,
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{}", event_json).unwrap();
}

/// Helper to write an event with an object payload to a JSONL file.
pub(super) fn write_object_event_to_jsonl(
    path: &std::path::Path,
    topic: &str,
    payload: serde_json::Value,
) {
    use std::io::Write;
    let ts = chrono::Utc::now().to_rfc3339();
    let event_json = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": ts
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{}", event_json).unwrap();
}

/// Helper: collect all topics from the event bus after processing.
pub(super) fn collect_pending_topics(event_loop: &EventLoop) -> Vec<String> {
    let empty = Vec::new();
    event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Helper to set up an event loop with required_events configured.
pub(super) fn setup_loop_with_required_events(required: Vec<String>) -> EventLoop {
    let yaml = format!(
        r#"
event_loop:
  required_events:
{}
"#,
        required
            .iter()
            .map(|t| format!("    - \"{}\"", t))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
    EventLoop::new(config)
}

/// Helper to set up an event loop with memories enabled and a task store.
pub(super) fn setup_loop_with_tasks(temp_dir: &std::path::Path) -> EventLoop {
    use crate::loop_context::LoopContext;
    use crate::task::Task;
    use crate::task_store::TaskStore;

    let tasks_path = temp_dir.join(".ralph/agent/tasks.jsonl");
    std::fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();

    // Create task store with one open task
    let mut store = TaskStore::load(&tasks_path).unwrap();
    let task = Task::new("Open task".to_string(), 1);
    store.add(task);
    store.save().unwrap();

    let mut config = RalphConfig::default();
    config.memories.enabled = true;
    config.core.workspace_root = temp_dir.to_path_buf();

    let loop_context = LoopContext::primary(temp_dir.to_path_buf());
    EventLoop::with_context(config, loop_context)
}

/// Helper to set up an event loop with workflow guards.
pub(super) fn setup_loop_with_workflow_guards() -> EventLoop {
    use crate::config::{WorkflowChain, WorkflowChainMode, WorkflowGuardsConfig};

    let mut config = RalphConfig::default();
    config.event_loop.workflow_guards = Some(WorkflowGuardsConfig {
        chains: vec![WorkflowChain {
            name: "experiment".to_string(),
            topics: vec![
                "experiment.planned".to_string(),
                "experiment.ready".to_string(),
                "experiment.measured".to_string(),
                "experiment.scored".to_string(),
            ],
            mode: WorkflowChainMode::Strict,
            correlation: None,
        }],
    });

    EventLoop::new(config)
}

pub(super) struct MockRobotService {
    pub timeout: u64,
    pub should_timeout: bool,
}

impl ralph_proto::RobotService for MockRobotService {
    fn send_question(&self, _payload: &str) -> anyhow::Result<i32> {
        Ok(1)
    }
    fn wait_for_response(&self, _events_path: &Path) -> anyhow::Result<Option<String>> {
        if self.should_timeout {
            Ok(None)
        } else {
            Ok(Some("approved".to_string()))
        }
    }
    fn send_checkin(
        &self,
        _: u32,
        _: Duration,
        _: Option<&ralph_proto::CheckinContext>,
    ) -> anyhow::Result<i32> {
        Ok(0)
    }
    fn timeout_secs(&self) -> u64 {
        self.timeout
    }
    fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }
    fn stop(self: Box<Self>) {}
}

pub(super) struct RestartRequestRobotService;

impl ralph_proto::RobotService for RestartRequestRobotService {
    fn send_question(&self, _payload: &str) -> anyhow::Result<i32> {
        Ok(1)
    }

    fn wait_for_response(&self, _events_path: &Path) -> anyhow::Result<Option<String>> {
        Ok(Some("Please restart yourself now".to_string()))
    }

    fn send_checkin(
        &self,
        _: u32,
        _: Duration,
        _: Option<&ralph_proto::CheckinContext>,
    ) -> anyhow::Result<i32> {
        Ok(0)
    }

    fn timeout_secs(&self) -> u64 {
        5
    }

    fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn stop(self: Box<Self>) {}
}
