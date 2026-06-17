//! State projector unit tests.
//!
//! Plan ref: U1 of
//! `docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md`.

use std::path::Path;

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::config::{StateProjectionAction, StateProjectionConfig};
use crate::event_reader::Event;

fn make_event(topic: &str, payload: impl Into<String>) -> Event {
    Event {
        topic: topic.to_string(),
        payload: Some(payload.into()),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
    }
}

fn workspace() -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".ralph").join("agent")).unwrap();
    tmp
}

fn make_config() -> StateProjectionConfig {
    let mut actions = std::collections::HashMap::new();
    actions.insert(
        "work.ready".to_string(),
        StateProjectionAction::EnsureTask {
            key: "task_key".to_string(),
            title: Some("step".to_string()),
        },
    );
    actions.insert(
        "work.done".to_string(),
        StateProjectionAction::CloseTask {
            task_id: "task_id".to_string(),
            step: Some("step".to_string()),
        },
    );
    actions.insert(
        "queue.advance".to_string(),
        StateProjectionAction::AdvanceStep {
            current_step: Some("step".to_string()),
            completed_step: Some("completed_step".to_string()),
        },
    );
    actions.insert(
        "plan.complete".to_string(),
        StateProjectionAction::PlanComplete { final_step: Some("step".to_string()) },
    );
    StateProjectionConfig {
        enabled: true,
        actions,
    }
}

#[test]
fn disabled_config_is_a_noop() {
    let tmp = workspace();
    let cfg = StateProjectionConfig::default();
    let mut proj = StateProjector::new(ProjectionContext::new(tmp.path(), cfg));
    let event = make_event("work.ready", json!({"task_key": "x", "step": "step-01"}).to_string());
    let report = proj.apply(&[event]);
    assert_eq!(report.applied, 0);
    assert_eq!(report.rejected, 0);
    assert!(!tmp.path().join(".ralph/agent/tasks.jsonl").exists());
}

#[test]
fn empty_actions_map_is_a_noop() {
    let tmp = workspace();
    let cfg = StateProjectionConfig { enabled: true, actions: Default::default() };
    let mut proj = StateProjector::new(ProjectionContext::new(tmp.path(), cfg));
    let event = make_event("work.ready", json!({"task_key": "x"}).to_string());
    let report = proj.apply(&[event]);
    assert_eq!(report.applied, 0);
    assert!(!tmp.path().join(".ralph/agent/tasks.jsonl").exists());
}

#[test]
fn happy_path_ensure_task_writes_ledger() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new(tmp.path(), make_config()));
    let event = make_event(
        "work.ready",
        json!({"task_key": "ce-executor:p:step-01:u1-impl", "step": "step-01"}).to_string(),
    );
    let report = proj.apply(&[event]);
    assert_eq!(report.applied, 1);
    assert_eq!(report.rejected, 0);
    let content =
        std::fs::read_to_string(tmp.path().join(".ralph/agent/tasks.jsonl")).unwrap();
    assert!(content.contains("\"step-01\""));
    assert!(content.contains("\"open\""));
}

#[test]
fn happy_path_close_task_updates_progress() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new(tmp.path(), make_config()));
    let ready = make_event(
        "work.ready",
        json!({"task_key": "ce-executor:p:step-01:u1-impl", "step": "step-01"}).to_string(),
    );
    // Apply the ready event first so the cache is populated
    // with the freshly-created task's auto-generated id. The
    // subsequent `work.done` event must reference that real id —
    // the projector fail-closes on unknown task_ids.
    let ready_report = proj.apply(&[ready]);
    assert_eq!(ready_report.applied, 1);
    let id = proj
        .context()
        .tasks_cache
        .first()
        .map(|t| t.id.clone())
        .expect("ensure produced a task");
    let done = make_event(
        "work.done",
        json!({
            "task_id": id,
            "task_key": "ce-executor:p:step-01:u1-impl",
            "step": "step-01"
        })
        .to_string(),
    );
    let report = proj.apply(&[done]);
    assert_eq!(report.applied, 1);
    let progress = std::fs::read_to_string(tmp.path().join(".ralph/agent/progress.md")).unwrap();
    assert!(progress.contains("step-01"));
}

#[test]
fn happy_path_queue_advance_advances_current_step() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new(tmp.path(), make_config()));
    let event = make_event(
        "queue.advance",
        json!({"step": "step-02", "completed_step": "step-01"}).to_string(),
    );
    let report = proj.apply(&[event]);
    assert_eq!(report.applied, 1);
    let progress = std::fs::read_to_string(tmp.path().join(".ralph/agent/progress.md")).unwrap();
    assert!(progress.contains("step-02"));
    assert!(progress.contains("- step-01"));
}

#[test]
fn rejected_event_returns_reason() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new(tmp.path(), make_config()));
    // Missing task_key pointer — fail-closed.
    let event = make_event("work.ready", json!({}).to_string());
    let report = proj.apply(&[event]);
    assert_eq!(report.rejected, 1);
    assert_eq!(report.rejections.len(), 1);
    assert!(report.rejections[0].reason.contains("task_key"));
}

#[test]
fn unprojected_topic_is_inert() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new(tmp.path(), make_config()));
    let event = make_event("build.done", json!({}).to_string());
    let report = proj.apply(&[event]);
    assert_eq!(report.applied, 0);
    assert_eq!(report.rejected, 0);
}

#[test]
fn projected_topics_list_is_locked() {
    assert_eq!(
        PROJECTED_TOPICS,
        &[
            "work.ready",
            "work.done",
            "queue.advance",
            "plan.complete",
            "review.passed",
            "review.failed",
            "plan.blocked",
        ]
    );
}

#[test]
fn json_pointer_reads_nested_keys() {
    let v = json!({"a": {"b": "hi"}});
    assert_eq!(json_pointer(&v, "a.b"), Some("hi"));
    assert_eq!(json_pointer(&v, "missing"), None);
    assert_eq!(json_pointer(&v, ""), None);
}

#[test]
fn bootstrap_from_disk_loads_existing_ledger() {
    let tmp = workspace();
    // Seed a task and progress file.
    let task_path = tmp.path().join(".ralph/agent/tasks.jsonl");
    let progress_path = tmp.path().join(".ralph/agent/progress.md");
    std::fs::write(&task_path, "{}\n").unwrap();
    std::fs::write(&progress_path, "## Current Step\nstep-07\n").unwrap();

    let mut proj = StateProjector::new(ProjectionContext::new(tmp.path(), make_config()));
    proj.bootstrap_from_disk().unwrap();
    let snap = &proj.context().progress_cache;
    assert_eq!(snap.current_step.as_deref(), Some("step-07"));
}

#[test]
fn progress_paths_match_canonical_layout() {
    let ws = Path::new("/tmp/example");
    assert_eq!(
        tasks_path(ws),
        PathBuf::from("/tmp/example/.ralph/agent/tasks.jsonl")
    );
    assert_eq!(
        progress_path(ws),
        PathBuf::from("/tmp/example/.ralph/agent/progress.md")
    );
}
