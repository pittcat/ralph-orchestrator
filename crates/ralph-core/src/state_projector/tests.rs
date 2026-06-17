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
        StateProjectionAction::PlanComplete {
            final_step: Some("step".to_string()),
        },
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
    let event = make_event(
        "work.ready",
        json!({"task_key": "x", "step": "step-01"}).to_string(),
    );
    let report = proj.apply(&[event]);
    assert_eq!(report.applied, 0);
    assert_eq!(report.rejected, 0);
    assert!(!tmp.path().join(".ralph/agent/tasks.jsonl").exists());
}

#[test]
fn empty_actions_map_is_a_noop() {
    let tmp = workspace();
    let cfg = StateProjectionConfig {
        enabled: true,
        actions: Default::default(),
    };
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
    let content = std::fs::read_to_string(tmp.path().join(".ralph/agent/tasks.jsonl")).unwrap();
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

// P0 regression (review 2026-06-17-003): when two events of
// the same topic appear in a single batch and one fails the
// projector, only the matching event must be dropped — the
// other one (which the projector would have accepted) must
// survive. The previous implementation matched by topic name
// and dropped every event of the rejected topic, wiping out
// valid sibling events in a single batch.
#[test]
fn p0_retain_drops_only_matching_payload_in_batch() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new(tmp.path(), make_config()));
    // Pre-populate the ledger with the task that the OK event
    // will close, so the OK event passes the projector.
    let ready = make_event(
        "work.ready",
        json!({
            "task_id": "task-A",
            "task_key": "ce-executor:p:step-01:u1-impl",
            "plan_name": "p",
            "step": "step-01"
        })
        .to_string(),
    );
    proj.apply(&[ready]);
    let id_a = proj
        .context()
        .tasks_cache
        .first()
        .map(|t| t.id.clone())
        .expect("ensure produced a task");

    // Build a batch with three work.done events:
    //   - index 0: closes task-A (will succeed once we fix the
    //     payload to use the real id; here it uses the wrong id
    //     and fails with task_not_found).
    //   - index 1: missing task_id pointer (fails payload_parse
    //     is not right; fails because of missing pointer).
    //   - index 2: closes task-A with the right id (would
    //     succeed on its own).
    //
    // We craft index 0 to fail (wrong id), index 1 to fail
    // (missing field), and index 2 to succeed. After apply, the
    // rejection set must identify (0, payload0) and (1, payload1)
    // as the dropped ones, leaving index 2 intact in the loop's
    // event batch.
    let bad_id_payload =
        json!({"task_id": "wrong-id", "task_key": "ce-executor:p:step-01:u1-impl", "step": "step-01"})
            .to_string();
    let missing_field_payload = json!({}).to_string(); // no task_id
    let good_payload = json!({
        "task_id": id_a,
        "task_key": "ce-executor:p:step-01:u1-impl",
        "step": "step-01"
    })
    .to_string();
    let good_event = make_event("work.done", good_payload.clone());
    let bad_event = make_event("work.done", bad_id_payload.clone());
    let missing_event = make_event("work.done", missing_field_payload.clone());

    // Simulate the same hook the event loop uses to drop
    // rejected events from a batch. The hook should drop
    // bad_event and missing_event but keep good_event.
    let report = proj.apply(&[bad_event.clone(), missing_event.clone(), good_event.clone()]);
    assert_eq!(report.rejected, 2, "two events should be rejected");
    assert_eq!(report.applied, 1, "good event should still apply");
    assert_eq!(report.rejections.len(), 2);
    assert!(
        report.rejections.iter().all(|r| r.payload.is_some()),
        "rejections should carry payload"
    );

    // Mirror the event_loop retain logic.
    let mut seen_no_payload = std::collections::HashMap::new();
    let mut need_no_payload = std::collections::HashMap::new();
    for r in &report.rejections {
        if r.payload.is_none() {
            *need_no_payload.entry(r.topic.clone()).or_insert(0) += 1;
        }
    }
    let rejected_with_payload: std::collections::HashSet<(String, String)> = report
        .rejections
        .iter()
        .filter_map(|r| {
            let p = r.payload.as_ref()?;
            Some((r.topic.clone(), p.clone()))
        })
        .collect();
    let mut batch = vec![bad_event, missing_event, good_event];
    batch.retain(|e| {
        if let Some(p) = e.payload.as_ref() {
            !rejected_with_payload.contains(&(e.topic.clone(), p.clone()))
        } else {
            let seen = seen_no_payload.entry(e.topic.clone()).or_insert(0);
            let needed = need_no_payload.get(&e.topic).copied().unwrap_or(0);
            let drop = *seen < needed;
            *seen += 1;
            !drop
        }
    });
    assert_eq!(
        batch.len(),
        1,
        "P0 fix: only matching events dropped, sibling kept"
    );
    assert_eq!(batch[0].topic, "work.done");
    assert_eq!(batch[0].payload.as_deref(), Some(good_payload.as_str()));
}

// P1 fix (review 2026-06-17-003): `project_plan_complete` must
// close any open tasks in tasks.jsonl, not just touch
// progress.md. Without this, the next `queue.advance` would
// fail the U4 `progress_task_gate` because tasks.jsonl still
// carried stale open rows.
#[test]
fn p1_plan_complete_closes_open_tasks() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new(tmp.path(), make_config()));
    // Create two open tasks via the projector so the ledger is
    // // populated the same way the loop would do it.
    let ready1 = make_event(
        "work.ready",
        json!({
            "task_id": "task-A",
            "task_key": "ce-executor:p:step-01:u1-impl",
            "plan_name": "p",
            "step": "step-01"
        })
        .to_string(),
    );
    let ready2 = make_event(
        "work.ready",
        json!({
            "task_id": "task-B",
            "task_key": "ce-executor:p:step-02:u1-impl",
            "plan_name": "p",
            "step": "step-02"
        })
        .to_string(),
    );
    proj.apply(&[ready1, ready2]);
    assert_eq!(proj.context().tasks_cache.len(), 2);
    assert!(
        proj.context()
            .tasks_cache
            .iter()
            .all(|t| !t.status.is_terminal())
    );

    // Fire plan.complete. After this, every task must be terminal.
    let plan_complete = make_event("plan.complete", json!({"step": "step-final"}).to_string());
    let report = proj.apply(&[plan_complete]);
    assert_eq!(report.applied, 1, "plan.complete should apply");
    let tasks_path = tmp.path().join(".ralph").join("agent").join("tasks.jsonl");
    let content = std::fs::read_to_string(&tasks_path).unwrap();
    assert!(
        content.contains("\"closed\""),
        "every task must end up closed after plan.complete; ledger: {content}"
    );
    let reopened: Vec<_> = proj
        .context()
        .tasks_cache
        .iter()
        .filter(|t| !t.status.is_terminal())
        .collect();
    assert!(
        reopened.is_empty(),
        "no task may remain open after plan.complete: {reopened:?}"
    );
}

// P2 (review 2026-06-17-003): `bootstrap_from_disk` must
// gracefully handle the cold-start cases — missing files,
// malformed JSON lines, empty progress headings. The projector
// is the canonical writer in Phase 1, but the resume path
// (Unit 6) reads ledgers written by previous runs that may
// have been hand-edited.
#[test]
fn p2_bootstrap_handles_missing_files() {
    let tmp = tempfile::tempdir().unwrap();
    // No `.ralph/agent/` dir at all — bootstrap should not panic.
    let mut proj = StateProjector::new(ProjectionContext::new(
        tmp.path(),
        StateProjectionConfig::default(),
    ));
    proj.bootstrap_from_disk().unwrap();
    assert!(proj.context().tasks_cache.is_empty());
    let snap = &proj.context().progress_cache;
    assert!(snap.current_step.is_none());
    assert!(snap.completed_steps.is_empty());
}

#[test]
fn p2_bootstrap_handles_malformed_task_lines() {
    let tmp = workspace();
    let tasks_path = tmp.path().join(".ralph").join("agent").join("tasks.jsonl");
    // A hand-written ledger with a bad line and a good line.
    // The good line carries the canonical fields so the
    // `Task` deserializer accepts it. The bad line is a
    // syntax error that `TaskStore::load` must skip.
    std::fs::write(
        &tasks_path,
        "{this is not json}\n\
         {\"id\":\"task-good\",\"title\":\"t\",\"status\":\"open\",\"priority\":1,\
          \"blocked_by\":[],\"created\":\"2026-01-01T00:00:00Z\"}\n",
    )
    .unwrap();
    let mut proj = StateProjector::new(ProjectionContext::new(
        tmp.path(),
        StateProjectionConfig::default(),
    ));
    // Bad lines are skipped (TaskStore::load contract); the
    // good line must survive.
    proj.bootstrap_from_disk().unwrap();
    assert_eq!(proj.context().tasks_cache.len(), 1);
    assert_eq!(proj.context().tasks_cache[0].id, "task-good");
}

#[test]
fn p2_bootstrap_handles_empty_progress_headings() {
    let tmp = workspace();
    let progress_path = tmp.path().join(".ralph").join("agent").join("progress.md");
    std::fs::write(&progress_path, "# nothing here\n\n").unwrap();
    let mut proj = StateProjector::new(ProjectionContext::new(
        tmp.path(),
        StateProjectionConfig::default(),
    ));
    proj.bootstrap_from_disk().unwrap();
    let snap = &proj.context().progress_cache;
    assert!(snap.current_step.is_none());
    assert!(snap.completed_steps.is_empty());
    assert!(snap.empty_headings);
}

#[test]
fn p2_repeated_apply_keeps_cache_warm() {
    // Cold-start only triggers on the first call. The
    // projector sub-modules (`project_ensure_task` etc.)
    // re-read disk on every call as a deliberate safety net
    // (so cross-loop writes are visible immediately), so the
    // observable contract is: after each apply, the cache is
    // in sync with the on-disk ledger. This test asserts that
    // invariant rather than counting cache entries.
    let tmp = workspace();
    let tasks_path = tmp.path().join(".ralph").join("agent").join("tasks.jsonl");
    std::fs::write(
        &tasks_path,
        "{\"id\":\"seed\",\"title\":\"t\",\"status\":\"open\",\"priority\":1,\"blocked_by\":[],\"created\":\"2026-01-01T00:00:00Z\"}\n",
    )
    .unwrap();
    let mut proj = StateProjector::new(ProjectionContext::new(tmp.path(), make_config()));
    proj.apply(&[make_event(
        "work.ready",
        json!({"task_key": "k1", "step": "step-01"}).to_string(),
    )]);
    let cache_after_first = proj.context().tasks_cache.clone();
    proj.apply(&[make_event(
        "work.ready",
        json!({"task_key": "k2", "step": "step-02"}).to_string(),
    )]);
    let cache_after_second = proj.context().tasks_cache.clone();

    // Each apply must end with a cache that mirrors the disk.
    let disk = std::fs::read_to_string(&tasks_path).unwrap();
    let disk_task_count = disk.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        cache_after_second.len(),
        disk_task_count,
        "cache and disk must agree after every apply (cache={}, disk lines={})",
        cache_after_second.len(),
        disk_task_count
    );
    assert_eq!(
        cache_after_first.len(),
        disk_task_count - 1,
        "the first apply must have added exactly one task to disk"
    );
}
