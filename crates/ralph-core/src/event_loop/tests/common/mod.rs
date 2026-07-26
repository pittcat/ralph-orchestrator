//! Shared test helpers and mock services for event_loop tests.

use super::*;

use std::path::Path;

/// U3 helper: build a `RalphConfig` for the minimal isolated
/// preset used by `preview_api` and `preview_characterization`
/// tests. Single source of truth — replaces 5+ inline YAML blocks
/// that previously drifted between test files.
///
/// `memories` / `tasks` toggle the `memories.enabled` /
/// `tasks.enabled` flags; `inject: auto` is preserved when
/// memories is enabled so the auto-inject path is exercised.
pub(super) fn minimal_isolated_config(memories: bool, tasks: bool) -> RalphConfig {
    let yaml = format!(
        r#"
event_loop:
  execution_mode: isolated
hats:
  builder:
    name: "Builder"
    triggers: ["work.start"]
    publishes: ["work.done"]
memories:
  enabled: {memories}
  inject: auto
tasks:
  enabled: {tasks}
"#
    );
    serde_yaml::from_str(&yaml).expect("minimal_isolated_config YAML must parse")
}

/// U3 helper: build a `RalphConfig` with multiple hats sharing
/// the same builder topology. Used by per-hat filter
/// characterization tests where a single builder YAML is too
/// narrow to exercise the registry's `is_hat_eligible` path.
///
/// `hats` is a slice of `(hat_id, display_name)` tuples that get
/// rendered into the YAML `hats:` block. The default gate is
/// opened (memories=true, tasks=true) so the test focuses on the
/// per-hat filter, not on gating branches.
pub(super) fn per_hat_isolated_config(hats: &[(&str, &str)]) -> RalphConfig {
    let hats_yaml: String = hats
        .iter()
        .map(|(id, name)| {
            format!(
                "  {id}:\n    name: \"{name}\"\n    triggers: [\"work.start\"]\n    publishes: [\"work.done\"]\n"
            )
        })
        .collect();
    let yaml = format!(
        r#"
event_loop:
  execution_mode: isolated
hats:
{hats_yaml}memories:
  enabled: true
  inject: auto
tasks:
  enabled: true
"#
    );
    serde_yaml::from_str(&yaml).expect("per_hat_isolated_config YAML must parse")
}

/// U3 helper: a reverse-case fixture used to verify that
/// `SkillInjector::plan_auto_inject` returns empty when
/// `skills.enabled = false` even though memories and tasks are
/// enabled. The `plan_auto_inject_with_disabled_skills` test in
/// `preview_api.rs` consumes this.
///
/// Future maintainers who tweak the global gate must keep this
/// fixture returning empty (skills.enabled = false must short-
/// circuit regardless of memories/tasks flags).
pub(super) fn fixture_with_disabled_skills() -> RalphConfig {
    let yaml = r#"
event_loop:
  execution_mode: isolated
skills:
  enabled: false
hats:
  builder:
    name: "Builder"
    triggers: ["work.start"]
    publishes: ["work.done"]
memories:
  enabled: true
  inject: auto
tasks:
  enabled: true
"#;
    serde_yaml::from_str(yaml).expect("fixture_with_disabled_skills YAML must parse")
}

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
        r"
event_loop:
  required_events:
{}
",
        required
            .iter()
            .map(|t| format!("    - \"{}\"", t))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
    // U11 fail-closed: required_events triggers a JSONL ingest
    // path that the stage pipeline gates. Inject a flow that
    // admits the required topics (and LOOP_COMPLETE) so the
    // chain-validation test stays focused on `required_events`,
    // not on FlowStepScope reject semantics.
    let topics_yaml = required
        .iter()
        .map(|t| format!("        - {t}"))
        .chain(std::iter::once("        - LOOP_COMPLETE".to_string()))
        .collect::<Vec<_>>()
        .join("\n");
    let flow_yaml = format!(
        "mechanism:\n  flow:\n    type: declared\n    version: 1\n    terminal_emits: [LOOP_COMPLETE]\n    steps:\n      - id: unit_loop\n        kind: foreach\n        allowed_emits:\n{topics_yaml}\n        terminal_when: all_done\n"
    );
    let flow = crate::event_loop::flow_declaration::FlowDeclaration::from_yaml(&flow_yaml)
        .expect("test flow YAML must parse");
    let mut event_loop = EventLoop::new(config);
    event_loop.stage_pipeline =
        crate::event_loop::stage_pipeline::StagePipeline::with_default_stages(flow);
    event_loop
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

/// U11 fail-closed helper: build an `EventLoop` whose
/// `FlowStepScope` admits the given topics under the
/// U11 fail-closed helper: install a `FlowStepScope`-admitting
/// `StagePipeline` on an existing `EventLoop`. Used by tests
/// that build their own `EventLoop` (e.g. with custom
/// `required_events` or hat configs) and then need the
/// stage pipeline to permit the JSONL topics they emit.
/// `LOOP_COMPLETE` is always included in the `unit_loop`
/// step's `allowed_emits` and the flow's `terminal_emits`
/// so the dispatcher topic also passes the gate.
pub(super) fn install_admitting_flow(event_loop: &mut EventLoop, allowed: &[&str]) {
    use crate::event_loop::flow_declaration::FlowDeclaration;
    use crate::event_loop::stage_pipeline::StagePipeline;

    let mut all_topics: Vec<String> = allowed.iter().map(|s| s.to_string()).collect();
    if !all_topics.iter().any(|t| t == "LOOP_COMPLETE") {
        all_topics.push("LOOP_COMPLETE".to_string());
    }
    let topics_yaml = all_topics
        .iter()
        .map(|t| format!("        - {t}"))
        .collect::<Vec<_>>()
        .join("\n");
    let yaml = format!(
        "mechanism:\n  flow:\n    type: declared\n    version: 1\n    terminal_emits: [LOOP_COMPLETE]\n    steps:\n      - id: unit_loop\n        kind: foreach\n        allowed_emits:\n{topics_yaml}\n        terminal_when: all_done\n"
    );
    let flow = FlowDeclaration::from_yaml(&yaml).expect("test flow YAML must parse");
    event_loop.stage_pipeline = StagePipeline::with_default_stages(flow);
}
