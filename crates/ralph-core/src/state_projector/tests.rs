//! State projector unit tests.
//!
//! Plan ref: U1 of
//! `docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md`.

// Allow direct `tasks_cache` / `progress_cache` access in this
// module: the legacy mirror is the unit under test (every legacy
// caller and ~150 pre-U2 tests still read from these fields). The
// new `LedgerSnapshot` path is covered by `u2_tests.rs`. The
// deprecation is intentional — it tracks the migration without
// forcing a refactor of every test in this module.
#![allow(deprecated)]

use std::path::Path;

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::config::{StateProjectionAction, StateProjectionConfig};

#[test]
fn u2_quick_diag() {
    let _ = 42;
}

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
        system_injected: None,
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
        actions_chain: std::collections::HashMap::new(),
    }
}

#[test]
fn configured_custom_topic_projects_task_batch() {
    let tmp = workspace();
    let config: StateProjectionConfig = serde_yaml::from_str(
        r#"
enabled: true
actions:
  custom.plan.ready:
    kind: ensure_task_batch
    items: unit_tasks
    count: unit_count
    key: task_key
    title: title
    blocked_by_keys: depends_on_task_keys
"#,
    )
    .expect("batch action config must parse");
    let mut projector = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), config));
    let event = make_event(
        "custom.plan.ready",
        json!({
            "unit_count": 2,
            "unit_tasks": [
                {
                    "task_key": "custom:p:U1",
                    "title": "First",
                    "depends_on_task_keys": []
                },
                {
                    "task_key": "custom:p:U2",
                    "title": "Second",
                    "depends_on_task_keys": ["custom:p:U1"]
                }
            ]
        })
        .to_string(),
    );

    let report = projector.apply(&[event]);

    assert_eq!(report.applied, 1);
    assert_eq!(report.rejected, 0, "{:?}", report.rejections);
    let store = crate::task_store::TaskStore::load(&tasks_path(tmp.path())).unwrap();
    assert_eq!(store.all().len(), 2);
    assert_eq!(store.all()[1].blocked_by, vec![store.all()[0].id.clone()]);
}

#[test]
fn unconfigured_custom_topic_remains_inert() {
    let tmp = workspace();
    let mut projector =
        StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let event = make_event("custom.plan.ready", json!({}).to_string());

    let report = projector.apply(&[event]);

    assert_eq!(report.applied, 0);
    assert_eq!(report.rejected, 0);
    assert!(!tasks_path(tmp.path()).exists());
}

#[test]
fn batch_rejection_is_atomic_and_replay_reuses_ids() {
    let tmp = workspace();
    let config: StateProjectionConfig = serde_yaml::from_str(
        r#"
enabled: true
actions:
  custom.plan.ready:
    kind: ensure_task_batch
    items: unit_tasks
    count: unit_count
    key: task_key
    title: title
    blocked_by_keys: depends_on_task_keys
"#,
    )
    .unwrap();
    let mut projector = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), config));
    let event = |payload: serde_json::Value| make_event("custom.plan.ready", payload.to_string());
    let invalid = event(json!({
        "unit_count": 2,
        "unit_tasks": [{
            "task_key": "custom:p:U1",
            "title": "First",
            "depends_on_task_keys": ["missing"]
        }]
    }));
    let path = tasks_path(tmp.path());
    crate::task_store::reset_successful_persist_count(&path);
    let before = std::fs::read(&path).unwrap_or_default();
    let rejected = projector.apply(&[invalid]);
    assert_eq!(rejected.applied, 0);
    assert_eq!(rejected.rejected, 1);
    assert_eq!(std::fs::read(&path).unwrap_or_default(), before);
    assert_eq!(crate::task_store::successful_persist_count(&path), 0);

    let valid = event(json!({
        "unit_count": 2,
        "unit_tasks": [
            {"task_key": "custom:p:U1", "title": "First", "depends_on_task_keys": []},
            {"task_key": "custom:p:U2", "title": "Second", "depends_on_task_keys": ["custom:p:U1"]}
        ]
    }));
    let accepted = projector.apply(std::slice::from_ref(&valid));
    assert_eq!(accepted.applied, 1);
    assert_eq!(crate::task_store::successful_persist_count(&path), 1);
    let first = crate::task_store::TaskStore::load(&path).unwrap();
    let ids = first
        .all()
        .iter()
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    let accepted_again = projector.apply(&[valid]);
    assert_eq!(accepted_again.applied, 1);
    let replay = crate::task_store::TaskStore::load(&path).unwrap();
    assert_eq!(replay.all().len(), 2);
    assert_eq!(
        replay
            .all()
            .iter()
            .map(|task| task.id.clone())
            .collect::<Vec<_>>(),
        ids
    );
}

#[test]
fn batch_of_64_items_persists_once() {
    let tmp = workspace();
    let config: StateProjectionConfig = serde_yaml::from_str(
        r#"
enabled: true
actions:
  custom.plan.ready:
    kind: ensure_task_batch
    items: unit_tasks
    count: unit_count
    key: task_key
    title: title
    blocked_by_keys: depends_on_task_keys
"#,
    )
    .unwrap();
    let mut projector = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), config));
    let unit_tasks = (1..=64)
        .map(|index| json!({
            "task_key": format!("custom:p:U{index}"),
            "title": format!("Unit {index}"),
            "depends_on_task_keys": if index == 1 { vec![] } else { vec![format!("custom:p:U{}", index - 1)] }
        }))
        .collect::<Vec<_>>();
    let path = tasks_path(tmp.path());
    crate::task_store::reset_successful_persist_count(&path);
    let report = projector.apply(&[make_event(
        "custom.plan.ready",
        json!({"unit_count": 64, "unit_tasks": unit_tasks}).to_string(),
    )]);
    assert_eq!(report.rejected, 0, "{:?}", report.rejections);
    assert_eq!(crate::task_store::successful_persist_count(&path), 1);
    let store = crate::task_store::TaskStore::load(&path).unwrap();
    assert_eq!(store.all().len(), 64);
    for pair in store.all().windows(2) {
        assert_eq!(pair[1].blocked_by, vec![pair[0].id.clone()]);
    }
}

#[test]
fn disabled_config_is_a_noop() {
    let tmp = workspace();
    let cfg = StateProjectionConfig::default();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
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
        actions: std::collections::HashMap::default(),
        actions_chain: std::collections::HashMap::default(),
    };
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
    let event = make_event("work.ready", json!({"task_key": "x"}).to_string());
    let report = proj.apply(&[event]);
    assert_eq!(report.applied, 0);
    assert!(!tmp.path().join(".ralph/agent/tasks.jsonl").exists());
}

#[test]
fn happy_path_ensure_task_writes_ledger() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
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
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
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
fn happy_path_queue_advance_advances_derived_current_step() {
    // U1 of plan 2026-07-05-005 (KTD-1): the markdown `## Current
    // Step` heading is now derived from `completed_steps.last()`.
    // After `queue.advance` records `completed_step: step-01`, the
    // derived current step is `step-01` (the just-finished step).
    // The inbound `step: step-02` field is still useful for the
    // event-driven handoff, but it is no longer the source of
    // truth for the rendered heading.
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let event = make_event(
        "queue.advance",
        json!({"step": "step-02", "completed_step": "step-01"}).to_string(),
    );
    let report = proj.apply(&[event]);
    assert_eq!(report.applied, 1);
    let progress = std::fs::read_to_string(tmp.path().join(".ralph/agent/progress.md")).unwrap();
    assert!(
        progress.contains("## Current Step\nstep-01\n"),
        "derived current_step must equal completed_steps.last() = step-01, got:\n{progress}"
    );
    assert!(progress.contains("- step-01"));
}

#[test]
fn rejected_event_returns_reason() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
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
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let event = make_event("build.done", json!({}).to_string());
    let report = proj.apply(&[event]);
    assert_eq!(report.applied, 0);
    assert_eq!(report.rejected, 0);
}

// P0 regression (2026-06-18, ce-executor-serial step_handoff):
// the serial preset's plan-gate emits `queue.advance` with
// `{plan_name, completed_step, next_step, reviewed_task_id, reviewed_task_key}`
// (no `step` field). When `state_projection.actions.queue.advance.current_step`
// points at `step`, every queue.advance is dropped with
// `event.state_projection.rejected` and `progress.md` stops advancing.
// This test pins the fix: a pointer override to `next_step` lets the
// serial emit shape actually project. If anyone reverts
// `current_step: "step"` in `presets/en/ce-executor-serial.yml`,
// this test still passes (it uses its own config), but the
// companion preset-lint test in `crates/ralph-cli/src/presets.rs`
// catches the preset regression directly.
#[test]
fn serial_preset_queue_advance_payload_drives_progress_with_next_step_pointer() {
    let tmp = workspace();
    let mut actions = std::collections::HashMap::new();
    // Mirror the post-fix ce-executor-serial state_projection block.
    actions.insert(
        "queue.advance".to_string(),
        StateProjectionAction::AdvanceStep {
            current_step: Some("next_step".to_string()),
            completed_step: Some("completed_step".to_string()),
        },
    );
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(
        tmp.path(),
        StateProjectionConfig {
            enabled: true,
            actions,
            actions_chain: std::collections::HashMap::default(),
        },
    ));

    // Real serial preset payload shape (no `step` field).
    let event = make_event(
        "queue.advance",
        json!({
            "plan_name": "demo",
            "completed_step": "step-01",
            "next_step": "step-02",
            "reviewed_task_id": "task-1",
            "reviewed_task_key": "ce-executor:demo:step-01:u1-impl"
        })
        .to_string(),
    );
    let report = proj.apply(&[event]);
    assert_eq!(
        report.rejected, 0,
        "queue.advance must not be rejected under serial payload shape: {:?}",
        report.rejections
    );
    assert_eq!(report.applied, 1);

    let progress = std::fs::read_to_string(tmp.path().join(".ralph/agent/progress.md")).unwrap();
    // U1 of plan 2026-07-05-005 (KTD-1): the rendered `## Current
    // Step` heading is now derived from `completed_steps.last()`,
    // so it equals the just-completed step (`step-01`), not the
    // upcoming `next_step` field. The agent still sees the
    // completed step as the heading; downstream consumers use the
    // derived accessor for "what is the current step?".
    assert!(
        progress.contains("## Current Step\nstep-01\n"),
        "derived Current Step must equal completed_steps.last() = step-01, got:\n{progress}"
    );
    assert!(
        progress.contains("- step-01"),
        "Completed Steps must include completed_step value, got:\n{progress}"
    );
}

#[test]
fn unconfigured_topic_is_inert_per_action_key_authority() {
    // Plan 2026-07-28-001 R11 / S9 (4.2 action-key migration):
    // after the legacy `PROJECTED_TOPICS` allow-list was removed,
    // the only activation trigger is the configured action key on
    // `StateProjectionConfig`. A topic with no matching key is
    // literally inert: `apply()` walks past it without bumping
    // the rejection counter.
    use crate::config::{StateProjectionAction, StateProjectionConfig};
    use crate::event_reader::Event;
    use crate::state_projector::{ProjectionContext, StateProjector};
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let workspace = temp.path();

    let mut config = StateProjectionConfig::default();
    config.enabled = true;
    config.actions.insert(
        "configured.topic".to_string(),
        StateProjectionAction::EnsureTask {
            key: "k".to_string(),
            title: None,
        },
    );
    let ctx = ProjectionContext::new_legacy(workspace, config);
    let mut projector = StateProjector::new(ctx);
    let events = vec![Event {
        topic: "forbidden.topic".to_string(),
        payload: Some("{}".to_string()),
        ts: "2024-01-01T00:00:00Z".to_string(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    }];
    let report = projector.apply(events.as_slice());
    // The unconfigured topic must stay silent (no rejection, no
    // error) because the action-key gate filters it before the
    // dispatch arm ever sees it.
    assert_eq!(
        report.rejected, 0,
        "action-key gate must skip inert topics without counting them"
    );
    assert!(
        report.rejections.is_empty(),
        "inert topics must not produce rejection diagnostics: {:?}",
        report.rejections
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

    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
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
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
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
    //   - index 0: closes task-A with a wrong id AND wrong key
    //     (fails task_not_found; the P0-2 fallback path looks up
    //     by `task_key` first when present, so the wrong-id path
    //     only fails when neither id nor key match).
    //   - index 1: missing task_id pointer (fails because of
    //     missing pointer).
    //   - index 2: closes task-A with the right id (would
    //     succeed on its own).
    //
    // We craft index 0 to fail (wrong id + wrong key), index 1
    // to fail (missing field), and index 2 to succeed. After
    // apply, the rejection set must identify (0, payload0) and
    // (1, payload1) as the dropped ones, leaving index 2 intact
    // in the loop's event batch.
    let bad_id_payload =
        json!({"task_id": "wrong-id", "task_key": "wrong-key", "step": "step-01"}).to_string();
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
//
// 2026-06-30-001 P0-4 (primary-20260630-032648 diagnosis):
// `project_plan_complete` MUST skip never-started rows
// (started.is_none()) to prevent orphan closed tasks in
// tasks.jsonl. Started tasks still close normally.
#[test]
fn p1_plan_complete_closes_open_tasks() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    // Create two open tasks via the projector so the ledger is
    // populated the same way the loop would do it. task-A is
    // marked started (simulating a real work.start); task-B
    // is left unstarted (simulating a placeholder row that
    // the runtime must skip on plan.complete per P0-4).
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

    // Persist task-A as started so project_plan_complete
    // sees started.is_some() and closes it.
    use crate::task_store::TaskStore;
    let store_path = tmp.path().join(".ralph").join("agent").join("tasks.jsonl");
    let mut store = TaskStore::load(&store_path).unwrap();
    let id_a = store
        .all()
        .iter()
        .find(|t| t.key.as_deref() == Some("ce-executor:p:step-01:u1-impl"))
        .map(|t| t.id.clone())
        .expect("task-A should be in store");
    if let Some(row) = store.get_mut(&id_a) {
        row.started = Some("2026-06-30T06:00:00Z".to_string());
    }
    store.save().unwrap();

    // Fire plan.complete. Started task-A must close;
    // unstarted task-B must remain open (P0-4).
    let plan_complete = make_event("plan.complete", json!({"step": "step-final"}).to_string());
    let report = proj.apply(&[plan_complete]);
    assert_eq!(report.applied, 1, "plan.complete should apply");
    let content = std::fs::read_to_string(&store_path).unwrap();
    assert!(
        content.contains("\"closed\""),
        "started task must end up closed after plan.complete; ledger: {content}"
    );
    let reopened: Vec<_> = proj
        .context()
        .tasks_cache
        .iter()
        .filter(|t| !t.status.is_terminal())
        .collect();
    assert_eq!(
        reopened.len(),
        1,
        "P0-4: only the never-started task should remain open; ledger: {reopened:?}"
    );
    assert!(
        reopened[0].id == "task-B",
        "P0-4: task-B must remain open (started.is_none())"
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
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(
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
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(
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
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(
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
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
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

// ---------------------------------------------------------------------------
// R1 regression matrix (2026-06-17-005 fix plan): the projector must
// honour `EventLoopConfig.enforce_current_unit` rather than hard-coding
// the R4 gate off. These tests pin the contract so a future refactor
// cannot silently re-introduce the P0 regression.
// ---------------------------------------------------------------------------

fn make_projector_with_r4(tmp: &TempDir, enforce_current_unit: bool) -> StateProjector {
    StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()))
        // Mirror what `EventLoop` does on first use: thread the loop's
        // R4 setting into the projector. The new path is exercised in
        // `event_loop` integration tests; here we toggle the field
        // directly so the unit test stays focused on the projector.
        .with_enforce_current_unit(enforce_current_unit)
}

/// R1 happy path — `enforce_current_unit=true`. Two `work.ready`
/// events for the same step but different units: the first is
/// accepted, the second is rejected (loud, not silent).
#[test]
fn r1_enforce_current_unit_true_rejects_sibling_unit() {
    let tmp = workspace();
    let mut proj = make_projector_with_r4(&tmp, true);

    let first = make_event(
        "work.ready",
        json!({"task_key": "ce-executor:p:step-01:u1-impl", "step": "step-01"}).to_string(),
    );
    let second = make_event(
        "work.ready",
        json!({"task_key": "ce-executor:p:step-01:u2-impl", "step": "step-01"}).to_string(),
    );

    let first_report = proj.apply(&[first]);
    assert_eq!(first_report.applied, 1);
    assert_eq!(first_report.rejected, 0);

    let second_report = proj.apply(&[second]);
    assert_eq!(second_report.applied, 0);
    assert_eq!(second_report.rejected, 1);
    let rejection = &second_report.rejections[0];
    assert_eq!(rejection.topic, "work.ready");
    assert!(
        rejection.reason.contains("r4_unit_collision"),
        "R4 reject must be loud; got: {}",
        rejection.reason,
    );
    // The reject reason must surface the sibling's key + id so
    // an operator (or agent) can locate the existing task
    // without grepping the ledger.
    assert!(
        rejection.reason.contains("sibling_task_key=")
            && rejection.reason.contains("sibling_task_id="),
        "R4 reject must include sibling_task_key and sibling_task_id for debug; got: {}",
        rejection.reason,
    );
    assert!(
        rejection.reason.contains("ce-executor:p:step-01:u1-impl"),
        "R4 reject must surface the existing sibling's key (u1-impl); got: {}",
        rejection.reason,
    );
    // The sibling event's payload must travel with the rejection
    // so the hook can drop it by `(topic, payload)` (P0 fix from
    // commit 0e6e9cc9).
    let payload_text = rejection
        .payload
        .as_deref()
        .expect("R1 reject must carry the offending event's payload snapshot");
    assert!(
        payload_text.contains("u2-impl"),
        "rejection payload must include the offending event's payload; got: {:?}",
        payload_text,
    );

    // Only the first task should be on disk.
    let disk = std::fs::read_to_string(tmp.path().join(".ralph/agent/tasks.jsonl")).unwrap();
    let line_count = disk.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        line_count, 1,
        "R4 reject must not have created the second task"
    );
}

/// R1 happy path — `enforce_current_unit=false`. The pre-Phase-1
/// behaviour is preserved: two sibling-unit `work.ready` events
/// both create tasks.
#[test]
fn r1_enforce_current_unit_false_allows_sibling_units() {
    let tmp = workspace();
    let mut proj = make_projector_with_r4(&tmp, false);

    let first = make_event(
        "work.ready",
        json!({"task_key": "ce-executor:p:step-01:u1-impl", "step": "step-01"}).to_string(),
    );
    let second = make_event(
        "work.ready",
        json!({"task_key": "ce-executor:p:step-01:u2-impl", "step": "step-01"}).to_string(),
    );

    let report = proj.apply(&[first, second]);
    assert_eq!(report.applied, 2);
    assert_eq!(report.rejected, 0);

    let disk = std::fs::read_to_string(tmp.path().join(".ralph/agent/tasks.jsonl")).unwrap();
    let line_count = disk.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(line_count, 2, "both sibling tasks must be on disk");
}

/// R1 edge case — same unit, second `work.ready` is a refresh, not
/// a collision. R4 must allow refreshes (`u1` == `u1`).
#[test]
fn r1_enforce_current_unit_true_allows_same_unit_refresh() {
    let tmp = workspace();
    let mut proj = make_projector_with_r4(&tmp, true);

    let first = make_event(
        "work.ready",
        json!({"task_key": "ce-executor:p:step-01:u1-impl", "step": "step-01"}).to_string(),
    );
    let second = make_event(
        "work.ready",
        json!({"task_key": "ce-executor:p:step-01:u1-review", "step": "step-01"}).to_string(),
    );

    let report = proj.apply(&[first, second]);
    assert_eq!(report.applied, 2);
    assert_eq!(report.rejected, 0);
}

/// R1 edge case — `enforce_current_unit` is not on the YAML. The
/// default is `false`, so the projector's default behaviour is
/// unchanged (legacy semantics preserved).
#[test]
fn r1_default_enforce_current_unit_is_false() {
    assert!(
        !ProjectionContext::new_legacy(
            std::env::temp_dir().as_path(),
            StateProjectionConfig::default(),
        )
        .enforce_current_unit
    );
}

/// R1 boundary — R4 must not collide across `loop_id` boundaries.
/// `find_unit_collision_idx` (task_store.rs) skips tasks whose
/// `loop_id` differs from the candidate's. This pins the
/// contract so a future refactor cannot silently start
/// cross-loop rejecting.
#[test]
fn r1_enforce_current_unit_true_different_loop_id_no_collision() {
    let tmp = workspace();
    let mut proj = make_projector_with_r4(&tmp, true);

    // Seed an open task for `ce-executor:p:step-01:u1-impl`
    // under one loop. We do not have a way to set
    // `loop_id` through the projector path (it is sourced
    // from the event payload's `loop_id` field), so we write
    // the tasks.jsonl row directly to model a foreign-loop
    // sibling.
    let tasks_path = tmp.path().join(".ralph").join("agent").join("tasks.jsonl");
    let foreign_row = json!({
        "id": "foreign-1",
        "title": "u1-impl",
        "key": "ce-executor:p:step-01:u1-impl",
        "status": "open",
        "priority": 1,
        "blocked_by": [],
        "created": "2026-01-01T00:00:00Z",
        "loop_id": "loop-other",
    });
    std::fs::write(&tasks_path, format!("{foreign_row}\n")).unwrap();
    // Bootstrap so the cache mirrors the seeded row.
    let _ = proj.bootstrap_from_disk();

    // A work.ready for the same key+unit+step under a
    // different loop_id (we pass `loop_id` in the payload
    // to distinguish it from the seeded row).
    let event = make_event(
        "work.ready",
        json!({
            "task_key": "ce-executor:p:step-01:u1-impl",
            "task_id": "t-self",
            "step": "step-01",
            "loop_id": "loop-self",
        })
        .to_string(),
    );
    let report = proj.apply(&[event]);
    assert_eq!(
        report.rejected, 0,
        "R4 must not collide across loop_id boundaries; the foreign \
         sibling (loop_id=loop-other) must be invisible to the candidate \
         (loop_id=loop-self). rejections={:?}",
        report.rejections,
    );
    assert_eq!(
        report.applied, 1,
        "the candidate's task must be created despite the foreign sibling"
    );
}

/// R1 boundary — R4 must not collide across `step` boundaries.
/// `task_locus` extracts the `{plan}:{step}` middle portion
/// of the canonical key. Two tasks with the same unit but
/// different steps are NOT collisions.
#[test]
fn r1_enforce_current_unit_true_different_step_no_collision() {
    let tmp = workspace();
    let mut proj = make_projector_with_r4(&tmp, true);

    let first = make_event(
        "work.ready",
        json!({"task_key": "ce-executor:p:step-01:u1-impl", "step": "step-01"}).to_string(),
    );
    let second = make_event(
        "work.ready",
        json!({"task_key": "ce-executor:p:step-02:u1-impl", "step": "step-02"}).to_string(),
    );

    let first_report = proj.apply(&[first]);
    assert_eq!(first_report.applied, 1);
    assert_eq!(first_report.rejected, 0);

    let second_report = proj.apply(&[second]);
    assert_eq!(second_report.applied, 1);
    assert_eq!(
        second_report.rejected, 0,
        "R4 must not collide across step boundaries; u1-impl in step-01 \
         and u1-impl in step-02 are different loci"
    );

    // Both tasks should be on disk (different steps).
    let disk = std::fs::read_to_string(tmp.path().join(".ralph/agent/tasks.jsonl")).unwrap();
    let line_count = disk.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        line_count, 2,
        "both sibling tasks (step-01, step-02) must be on disk"
    );
}

// Plan 2026-07-29-002 U1 (R1 / S1): an accepted `exec.unit.done`
// closes exactly the live task addressed by `payload.task_id` and
// leaves siblings untouched. The projector never emits a
// "close everything" side effect.
#[test]
fn accepted_exec_unit_done_closes_exact_task() {
    let tmp = workspace();
    let config: StateProjectionConfig = serde_yaml::from_str(
        r#"
enabled: true
actions:
  forge.plan.ready:
    kind: ensure_task_batch
    items: unit_tasks
    count: unit_count
    key: task_key
    title: title
    blocked_by_keys: depends_on_task_keys
  exec.unit.done:
    kind: close_task
    task_id: task_id
"#,
    )
    .unwrap();
    let mut projector = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), config));
    // Seed two tasks via the same planner event the preset uses.
    let ready = make_event(
        "forge.plan.ready",
        json!({
            "unit_count": 2,
            "unit_tasks": [
                {"task_key": "forge:p:U1", "title": "U1", "depends_on_task_keys": []},
                {"task_key": "forge:p:U2", "title": "U2", "depends_on_task_keys": []}
            ]
        })
        .to_string(),
    );
    let ready_report = projector.apply(&[ready]);
    assert_eq!(ready_report.applied, 1);
    let ids: Vec<String> = projector
        .context()
        .tasks_cache
        .iter()
        .map(|t| t.id.clone())
        .collect();
    assert_eq!(ids.len(), 2);

    // Accepted exec.unit.done for task[0] only.
    let done = make_event(
        "exec.unit.done",
        json!({
            "wave_id": "w1",
            "slot_index": 0,
            "task_id": ids[0],
            "task_key": "forge:p:U1",
            "content_hash": "abc",
            "unit_id": "U1",
            "unit_report_path": "u.md",
            "plan_key": "p"
        })
        .to_string(),
    );
    let report = projector.apply(&[done]);
    assert_eq!(report.applied, 1, "{:?}", report.rejections);
    assert_eq!(report.rejected, 0);

    let store = crate::task_store::TaskStore::load(&tasks_path(tmp.path())).unwrap();
    let by_id: std::collections::HashMap<_, _> = store
        .all()
        .iter()
        .map(|t| (t.id.clone(), t.status))
        .collect();
    assert!(
        matches!(by_id.get(&ids[0]), Some(crate::task::TaskStatus::Closed)),
        "target task must be closed"
    );
    assert!(
        matches!(by_id.get(&ids[1]), Some(crate::task::TaskStatus::Open)),
        "sibling task must remain open"
    );
}

// Plan 2026-07-29-002 U1 (R1 / S3): an `exec.unit.done` whose
// `task_id` does not match any live task is rejected, and the
// projection produces zero task-state side effects.
#[test]
fn exec_unit_done_unknown_task_does_not_close_any_task() {
    let tmp = workspace();
    let config: StateProjectionConfig = serde_yaml::from_str(
        r#"
enabled: true
actions:
  forge.plan.ready:
    kind: ensure_task_batch
    items: unit_tasks
    count: unit_count
    key: task_key
    title: title
    blocked_by_keys: depends_on_task_keys
  exec.unit.done:
    kind: close_task
    task_id: task_id
"#,
    )
    .unwrap();
    let mut projector = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), config));
    let ready = make_event(
        "forge.plan.ready",
        json!({
            "unit_count": 1,
            "unit_tasks": [
                {"task_key": "forge:p:U1", "title": "U1", "depends_on_task_keys": []}
            ]
        })
        .to_string(),
    );
    projector.apply(&[ready]);

    let done = make_event(
        "exec.unit.done",
        json!({
            "wave_id": "w1",
            "slot_index": 0,
            "task_id": "no-such-task",
            "task_key": "forge:p:no-such-key",
            "content_hash": "abc",
            "unit_id": "U1",
            "unit_report_path": "u.md",
            "plan_key": "p"
        })
        .to_string(),
    );
    let report = projector.apply(&[done]);
    assert_eq!(report.applied, 0);
    assert_eq!(report.rejected, 1);

    let store = crate::task_store::TaskStore::load(&tasks_path(tmp.path())).unwrap();
    assert!(
        store
            .all()
            .iter()
            .all(|t| matches!(t.status, crate::task::TaskStatus::Open)),
        "no task may be closed by an unknown-id event"
    );
}

// (U2 schedule validation tests follow in the inner module below.)

#[test]
fn u2_diagnostic_smoke() {
    // If this test runs, the parent tests module is being scanned
    // correctly. Remove once the schedule_* tests are visible.
}

mod u2_schedule_validation {
    //! U2 of plan 2026-07-29-001
    //! (`fix-parallel-forge-static-wave-settlement-plan`).
    //!
    //! Table-driven cases for the static `execution_wave` /
    //! `integration_order` schedule validation inside
    //! `EnsureTaskBatch`. Each case builds a real
    //! `StateProjector::apply` with the new optional pointers and
    //! asserts either acceptance (write to ledger) or rejection
    //! (no side effect, structured reason).
    //!
    //! Cases:
    //! 1. AE1 longest-path DAG: 4 layers with 8 units.
    //! 2. wave 缺号 (gap).
    //! 3. 同 wave 依赖 (same-wave edge rejected).
    //! 4. 后 wave 依赖 (dep pointing to a larger wave rejected).
    //! 5. 重复 integration_order (duplicate).
    //! 6. order 逆依赖 (dep.order >= unit.order rejected).
    //! 7. digest 不匹配被拒 (empty digest rejected).
    //! 8. replay 幂等 (same payload accepted twice).
    //! 9. legacy path (no pointers) still accepted.
    //! 10. only one pointer declared is rejected.

    use super::*;
    use serde_json::json;

    fn workspace() -> TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".ralph").join("agent")).unwrap();
        tmp
    }

    fn batch_config(
        execution_wave: Option<&'static str>,
        integration_order: Option<&'static str>,
        execution_plan_digest: Option<&'static str>,
    ) -> StateProjectionConfig {
        let yaml = format!(
            r#"
enabled: true
actions:
  forge.plan.ready:
    kind: ensure_task_batch
    items: unit_tasks
    count: unit_count
    key: task_key
    title: title
    blocked_by_keys: depends_on_task_keys
    execution_wave: {}
    integration_order: {}
    execution_plan_digest: {}
"#,
            execution_wave
                .map(|s| format!("\"{s}\""))
                .unwrap_or_else(|| "~".to_string()),
            integration_order
                .map(|s| format!("\"{s}\""))
                .unwrap_or_else(|| "~".to_string()),
            execution_plan_digest
                .map(|s| format!("\"{s}\""))
                .unwrap_or_else(|| "~".to_string()),
        );
        serde_yaml::from_str(&yaml).expect("batch action config must parse")
    }

    fn apply_batch(
        projector: &mut StateProjector,
        payload: serde_json::Value,
    ) -> crate::state_projector::ApplyReport {
        projector.apply(&[make_event("forge.plan.ready", payload.to_string())])
    }

    /// AE1: longest-path DAG.
    ///   A,B                  -> wave 1
    ///   C(A),D(A,B),E(B)     -> wave 2
    ///   F(C,D),G(D,E)        -> wave 3
    ///   H(F,G)               -> wave 4
    /// integration_order [1..8] all unique, all deps respected.
    #[test]
    fn schedule_ae1_longest_path_dag_is_accepted() {
        let tmp = workspace();
        let cfg = batch_config(Some("execution_wave"), Some("integration_order"), None);
        let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));

        let payload = json!({
            "unit_count": 8,
            "unit_tasks": [
                {"task_key": "forge:p:A", "title": "A", "depends_on_task_keys": [], "execution_wave": 1, "integration_order": 1},
                {"task_key": "forge:p:B", "title": "B", "depends_on_task_keys": [], "execution_wave": 1, "integration_order": 2},
                {"task_key": "forge:p:C", "title": "C", "depends_on_task_keys": ["forge:p:A"], "execution_wave": 2, "integration_order": 3},
                {"task_key": "forge:p:D", "title": "D", "depends_on_task_keys": ["forge:p:A", "forge:p:B"], "execution_wave": 2, "integration_order": 4},
                {"task_key": "forge:p:E", "title": "E", "depends_on_task_keys": ["forge:p:B"], "execution_wave": 2, "integration_order": 5},
                {"task_key": "forge:p:F", "title": "F", "depends_on_task_keys": ["forge:p:C", "forge:p:D"], "execution_wave": 3, "integration_order": 6},
                {"task_key": "forge:p:G", "title": "G", "depends_on_task_keys": ["forge:p:D", "forge:p:E"], "execution_wave": 3, "integration_order": 7},
                {"task_key": "forge:p:H", "title": "H", "depends_on_task_keys": ["forge:p:F", "forge:p:G"], "execution_wave": 4, "integration_order": 8}
            ]
        });

        let report = apply_batch(&mut proj, payload);
        assert_eq!(
            report.applied, 1,
            "AE1 schedule must be accepted; rejections={:?}",
            report.rejections
        );
        let store = crate::task_store::TaskStore::load(&tasks_path(tmp.path())).unwrap();
        assert_eq!(store.all().len(), 8, "AE1 must write 8 tasks");
    }

    /// Wave gap: declared waves [1, 3] (missing 2) is rejected.
    #[test]
    fn schedule_wave_gap_is_rejected() {
        let tmp = workspace();
        let cfg = batch_config(Some("execution_wave"), Some("integration_order"), None);
        let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let payload = json!({
            "unit_count": 2,
            "unit_tasks": [
                {"task_key": "forge:p:A", "title": "A", "depends_on_task_keys": [], "execution_wave": 1, "integration_order": 1},
                {"task_key": "forge:p:B", "title": "B", "depends_on_task_keys": ["forge:p:A"], "execution_wave": 3, "integration_order": 2}
            ]
        });
        let report = apply_batch(&mut proj, payload);
        assert_eq!(report.applied, 0, "wave gap must reject the batch");
        assert_eq!(report.rejected, 1);
        let reason = &report.rejections[0].reason;
        assert!(
            reason.contains("wave") || reason.contains("consecutive"),
            "rejection reason must name the wave rule: got {reason}"
        );
        assert!(
            !tasks_path(tmp.path()).exists(),
            "rejected schedule must not write tasks.jsonl"
        );
    }

    /// Same-wave edge (A and B both wave=1, but B depends on A) is rejected.
    #[test]
    fn schedule_same_wave_edge_is_rejected() {
        let tmp = workspace();
        let cfg = batch_config(Some("execution_wave"), Some("integration_order"), None);
        let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let payload = json!({
            "unit_count": 2,
            "unit_tasks": [
                {"task_key": "forge:p:A", "title": "A", "depends_on_task_keys": [], "execution_wave": 1, "integration_order": 1},
                {"task_key": "forge:p:B", "title": "B", "depends_on_task_keys": ["forge:p:A"], "execution_wave": 1, "integration_order": 2}
            ]
        });
        let report = apply_batch(&mut proj, payload);
        assert_eq!(report.applied, 0, "same-wave edge must reject");
        assert_eq!(report.rejected, 1);
        let reason = &report.rejections[0].reason;
        assert!(
            reason.contains("wave"),
            "rejection must reference the wave rule: got {reason}"
        );
        assert!(!tasks_path(tmp.path()).exists());
    }

    /// Dep pointing to a larger wave than the dependent is rejected
    /// (wave(B) > wave(A) where A depends on B).
    #[test]
    fn schedule_later_wave_dependency_is_rejected() {
        let tmp = workspace();
        let cfg = batch_config(Some("execution_wave"), Some("integration_order"), None);
        let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        // B is wave 1 but depends on A (wave 2): illegal direction.
        let payload = json!({
            "unit_count": 2,
            "unit_tasks": [
                {"task_key": "forge:p:A", "title": "A", "depends_on_task_keys": [], "execution_wave": 2, "integration_order": 2},
                {"task_key": "forge:p:B", "title": "B", "depends_on_task_keys": ["forge:p:A"], "execution_wave": 1, "integration_order": 1}
            ]
        });
        let report = apply_batch(&mut proj, payload);
        assert_eq!(report.applied, 0, "later-wave dep must reject");
        assert_eq!(report.rejected, 1);
        let reason = &report.rejections[0].reason;
        assert!(
            reason.contains("wave"),
            "rejection must name the wave rule: got {reason}"
        );
        assert!(!tasks_path(tmp.path()).exists());
    }

    /// Duplicate `integration_order` is rejected.
    #[test]
    fn schedule_duplicate_integration_order_is_rejected() {
        let tmp = workspace();
        let cfg = batch_config(Some("execution_wave"), Some("integration_order"), None);
        let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let payload = json!({
            "unit_count": 3,
            "unit_tasks": [
                {"task_key": "forge:p:A", "title": "A", "depends_on_task_keys": [], "execution_wave": 1, "integration_order": 1},
                {"task_key": "forge:p:B", "title": "B", "depends_on_task_keys": [], "execution_wave": 1, "integration_order": 1},
                {"task_key": "forge:p:C", "title": "C", "depends_on_task_keys": ["forge:p:A"], "execution_wave": 2, "integration_order": 3}
            ]
        });
        let report = apply_batch(&mut proj, payload);
        assert_eq!(report.applied, 0, "duplicate order must reject");
        assert_eq!(report.rejected, 1);
        let reason = &report.rejections[0].reason;
        assert!(
            reason.contains("integration_order") || reason.contains("order"),
            "rejection must name the order rule: got {reason}"
        );
        assert!(!tasks_path(tmp.path()).exists());
    }

    /// `integration_order` of a dep must be < that of the unit
    /// (i.e. order on the dep edge must point earlier in the
    /// topological sequence).
    #[test]
    fn schedule_inverse_integration_order_is_rejected() {
        let tmp = workspace();
        let cfg = batch_config(Some("execution_wave"), Some("integration_order"), None);
        let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        // A has order 2; B depends on A but has order 1. dep.order(2) > unit.order(1).
        let payload = json!({
            "unit_count": 2,
            "unit_tasks": [
                {"task_key": "forge:p:A", "title": "A", "depends_on_task_keys": [], "execution_wave": 1, "integration_order": 2},
                {"task_key": "forge:p:B", "title": "B", "depends_on_task_keys": ["forge:p:A"], "execution_wave": 2, "integration_order": 1}
            ]
        });
        let report = apply_batch(&mut proj, payload);
        assert_eq!(report.applied, 0, "inverse order must reject");
        assert_eq!(report.rejected, 1);
        let reason = &report.rejections[0].reason;
        assert!(
            reason.contains("integration_order") || reason.contains("order"),
            "rejection must name the order rule: got {reason}"
        );
        assert!(!tasks_path(tmp.path()).exists());
    }

    /// Empty digest rejected (digest pointer present but value is empty).
    #[test]
    fn schedule_empty_digest_is_rejected() {
        let tmp = workspace();
        let cfg = batch_config(
            Some("execution_wave"),
            Some("integration_order"),
            Some("execution_plan_digest"),
        );
        let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let payload_empty = json!({
            "execution_plan_digest": "",
            "unit_count": 2,
            "unit_tasks": [
                {"task_key": "forge:p:A", "title": "A", "depends_on_task_keys": [], "execution_wave": 1, "integration_order": 1},
                {"task_key": "forge:p:B", "title": "B", "depends_on_task_keys": ["forge:p:A"], "execution_wave": 2, "integration_order": 2}
            ]
        });
        let report = apply_batch(&mut proj, payload_empty);
        assert_eq!(report.applied, 0, "empty digest must reject");
        assert_eq!(report.rejected, 1);
        let reason = &report.rejections[0].reason;
        assert!(
            reason.contains("digest"),
            "rejection must name the digest rule: got {reason}"
        );
    }

    /// Non-empty digest accepted (digest pointer present and value is non-empty).
    #[test]
    fn schedule_non_empty_digest_is_accepted() {
        let tmp = workspace();
        let cfg = batch_config(
            Some("execution_wave"),
            Some("integration_order"),
            Some("execution_plan_digest"),
        );
        let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let payload = json!({
            "execution_plan_digest": "deadbeef",
            "unit_count": 2,
            "unit_tasks": [
                {"task_key": "forge:p:A", "title": "A", "depends_on_task_keys": [], "execution_wave": 1, "integration_order": 1},
                {"task_key": "forge:p:B", "title": "B", "depends_on_task_keys": ["forge:p:A"], "execution_wave": 2, "integration_order": 2}
            ]
        });
        let report = apply_batch(&mut proj, payload);
        assert_eq!(
            report.applied, 1,
            "non-empty digest must be accepted: rejections={:?}",
            report.rejections
        );
    }

    /// Replaying the same accepted payload must be idempotent (no
    /// duplicate tasks, no rejection).
    #[test]
    fn schedule_replay_is_idempotent() {
        let tmp = workspace();
        let cfg = batch_config(Some("execution_wave"), Some("integration_order"), None);
        let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let payload = json!({
            "unit_count": 3,
            "unit_tasks": [
                {"task_key": "forge:p:A", "title": "A", "depends_on_task_keys": [], "execution_wave": 1, "integration_order": 1},
                {"task_key": "forge:p:B", "title": "B", "depends_on_task_keys": ["forge:p:A"], "execution_wave": 2, "integration_order": 2},
                {"task_key": "forge:p:C", "title": "C", "depends_on_task_keys": ["forge:p:B"], "execution_wave": 3, "integration_order": 3}
            ]
        });

        let first = apply_batch(&mut proj, payload.clone());
        assert_eq!(first.applied, 1, "first accept must succeed");
        let store = crate::task_store::TaskStore::load(&tasks_path(tmp.path())).unwrap();
        let ids_first: Vec<String> = store.all().iter().map(|t| t.id.clone()).collect();

        let second = apply_batch(&mut proj, payload);
        assert_eq!(
            second.applied, 1,
            "replay must still succeed (idempotent): rejections={:?}",
            second.rejections
        );
        let store2 = crate::task_store::TaskStore::load(&tasks_path(tmp.path())).unwrap();
        assert_eq!(
            store2.all().len(),
            3,
            "replay must not duplicate rows: got {}",
            store2.all().len()
        );
        let ids_second: Vec<String> = store2.all().iter().map(|t| t.id.clone()).collect();
        let mut ids_first_sorted = ids_first.clone();
        ids_first_sorted.sort();
        let mut ids_second_sorted = ids_second.clone();
        ids_second_sorted.sort();
        assert_eq!(
            ids_first_sorted, ids_second_sorted,
            "replay must reuse existing task ids"
        );
    }

    /// Legacy compatibility: when neither pointer is supplied, the
    /// projector skips wave/order validation (unchanged behaviour).
    #[test]
    fn schedule_legacy_no_pointers_is_accepted() {
        let tmp = workspace();
        let cfg = batch_config(None, None, None);
        let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let payload = json!({
            "unit_count": 2,
            "unit_tasks": [
                {"task_key": "custom:p:U1", "title": "First", "depends_on_task_keys": []},
                {"task_key": "custom:p:U2", "title": "Second", "depends_on_task_keys": ["custom:p:U1"]}
            ]
        });
        let report = apply_batch(&mut proj, payload);
        assert_eq!(
            report.applied, 1,
            "legacy path without pointers must be accepted: rejections={:?}",
            report.rejections
        );
        let store = crate::task_store::TaskStore::load(&tasks_path(tmp.path())).unwrap();
        assert_eq!(store.all().len(), 2);
    }

    /// Only one pointer declared -> reject (both pointers must be
    /// present together for schedule validation to be meaningful).
    #[test]
    fn schedule_only_one_pointer_declared_is_rejected() {
        let tmp = workspace();
        let cfg = batch_config(Some("execution_wave"), None, None);
        let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let payload = json!({
            "unit_count": 2,
            "unit_tasks": [
                {"task_key": "forge:p:A", "title": "A", "depends_on_task_keys": [], "execution_wave": 1},
                {"task_key": "forge:p:B", "title": "B", "depends_on_task_keys": ["forge:p:A"], "execution_wave": 2}
            ]
        });
        let report = apply_batch(&mut proj, payload);
        assert_eq!(
            report.applied, 0,
            "single-pointer config must reject (validation needs both)"
        );
        assert_eq!(report.rejected, 1);
        let reason = &report.rejections[0].reason;
        assert!(
            reason.contains("pointer") || reason.contains("integration_order"),
            "rejection must name the missing pointer: got {reason}"
        );
    }
}

// Plan 2026-07-29-001 U3 acceptance tests — see §11 unit-test
// split (1..6). The projector now exposes a `CloseTaskBatch`
// action that closes a wave's tasks atomically when the
// `forge.wave.settled` event arrives, while leaving each per-unit
// `exec.unit.done` event strictly read-only with respect to the
// task ledger. The tests below drive the run-down sequence the
// spec promises:
//
//   1. exec done inert to tasks.
//   2. settlement close exact batch.
//   3. future wave untouched.
//   4. duplicate IDs fail-close.
//   5. one unknown ID causes zero close.
//   6. replay duplicate不产生额外状态。
mod u3_close_task_batch {
    use super::*;

    fn config_with_batch_close() -> StateProjectionConfig {
        // The preset's actual state_projection.actions map for
        // parallel-forge. `exec.unit.done` is inert on purpose (it
        // maps to nothing); settlement is the only path that
        // closes tasks.
        serde_yaml::from_str(
            r#"
enabled: true
actions:
  forge.plan.ready:
    kind: ensure_task_batch
    items: unit_tasks
    count: unit_count
    key: task_key
    title: title
    blocked_by_keys: depends_on_task_keys
  forge.wave.settled:
    kind: close_task_batch
    task_ids: settled_task_ids
"#,
        )
        .unwrap()
    }

    fn seed_two_waves(projector: &mut StateProjector) -> (Vec<String>, Vec<String>) {
        // wave 1: U1, U2 (no deps); wave 2: U3, U4 (each depend
        // on wave 1). Two distinct units-per-wave arrays so we can
        // later prove U3+U4 keep their open state when only
        // wave 1 settles.
        let ready = make_event(
            "forge.plan.ready",
            json!({
                "unit_count": 4,
                "unit_tasks": [
                    {"task_key": "forge:p:U1", "title": "U1", "depends_on_task_keys": []},
                    {"task_key": "forge:p:U2", "title": "U2", "depends_on_task_keys": []},
                    {"task_key": "forge:p:U3", "title": "U3", "depends_on_task_keys": ["forge:p:U1", "forge:p:U2"]},
                    {"task_key": "forge:p:U4", "title": "U4", "depends_on_task_keys": ["forge:p:U1", "forge:p:U2"]}
                ]
            })
            .to_string(),
        );
        let report = projector.apply(&[ready]);
        assert_eq!(report.applied, 1, "planner event must apply");

        let ids: Vec<String> = projector
            .context()
            .tasks_cache
            .iter()
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(ids.len(), 4);

        // Pairs are stable by task_key order:
        //   U1, U2 → wave 1
        //   U3, U4 → wave 2
        let mut wave1: Vec<String> = Vec::new();
        let mut wave2: Vec<String> = Vec::new();
        for t in projector.context().tasks_cache.iter() {
            let key = t.key.clone().unwrap_or_default();
            match key.as_str() {
                "forge:p:U1" => wave1.push(t.id.clone()),
                "forge:p:U2" => wave1.push(t.id.clone()),
                "forge:p:U3" => wave2.push(t.id.clone()),
                "forge:p:U4" => wave2.push(t.id.clone()),
                _ => {}
            }
        }
        wave1.sort();
        wave2.sort();
        let _ = ids;
        (wave1, wave2)
    }

    // Unit-test split §11.1: exec.unit.done must be inert with
    // respect to the task ledger. The action map does not even
    // register `exec.unit.done`, so the projector must report
    // it as a no-op (`applied == 0`, `rejected == 0`).
    #[test]
    fn exec_unit_done_does_not_close_task() {
        let tmp = workspace();
        let cfg = config_with_batch_close();
        let mut projector = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let (wave1, _wave2) = seed_two_waves(&mut projector);

        let done = make_event(
            "exec.unit.done",
            json!({
                "wave_id": "w1",
                "slot_index": 0,
                "task_id": wave1[0],
                "task_key": "forge:p:U1",
                "content_hash": "abc",
                "unit_id": "U1",
                "unit_report_path": "u.md",
                "plan_key": "p"
            })
            .to_string(),
        );
        let report = projector.apply(&[done]);
        assert_eq!(report.applied, 0, "exec.unit.done must be inert");
        assert_eq!(report.rejected, 0);

        let store = crate::task_store::TaskStore::load(&tasks_path(tmp.path())).unwrap();
        for row in store.all() {
            assert!(
                matches!(row.status, crate::task::TaskStatus::Open),
                "task {} must remain open after exec.unit.done",
                row.id
            );
        }
    }

    // Unit-test split §11.2 + §11.3: a `forge.wave.settled`
    // payload closes exactly the named batch, and future wave
    // tasks are untouched.
    #[test]
    fn settlement_closes_exact_batch_and_leaves_future_wave_open() {
        let tmp = workspace();
        let cfg = config_with_batch_close();
        let mut projector = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let (wave1, wave2) = seed_two_waves(&mut projector);

        let settled = make_event(
            "forge.wave.settled",
            json!({
                "wave_id": "w1",
                "wave_index": 1,
                "settled_task_ids": wave1,
                "settled_unit_ids": ["U1", "U2"],
                "verified_base_commit": "deadbeef"
            })
            .to_string(),
        );
        let report = projector.apply(&[settled]);
        assert_eq!(report.applied, 1, "settlement must apply: {:?}", report.rejections);
        assert_eq!(report.rejected, 0);

        let store = crate::task_store::TaskStore::load(&tasks_path(tmp.path())).unwrap();
        let by_id: std::collections::HashMap<_, _> = store
            .all()
            .iter()
            .map(|t| (t.id.clone(), t.status.clone()))
            .collect();
        for id in &wave1 {
            assert!(
                matches!(by_id.get(id), Some(crate::task::TaskStatus::Closed)),
                "wave1 task {id} must be closed"
            );
        }
        for id in &wave2 {
            assert!(
                matches!(by_id.get(id), Some(crate::task::TaskStatus::Open)),
                "wave2 task {id} must remain open"
            );
        }
    }

    // Unit-test split §11.4: duplicate task_ids in the payload
    // fail-close with zero side effects.
    #[test]
    fn settlement_with_duplicate_ids_rejects_with_zero_side_effect() {
        let tmp = workspace();
        let cfg = config_with_batch_close();
        let mut projector = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let (wave1, _wave2) = seed_two_waves(&mut projector);

        let mut dup = wave1.clone();
        dup.push(wave1[0].clone());
        let settled = make_event(
            "forge.wave.settled",
            json!({
                "wave_id": "w1",
                "wave_index": 1,
                "settled_task_ids": dup,
                "settled_unit_ids": ["U1", "U2"],
                "verified_base_commit": "deadbeef"
            })
            .to_string(),
        );
        let report = projector.apply(&[settled]);
        assert_eq!(report.rejected, 1, "duplicate batch must be rejected");
        assert!(report.rejections[0].reason.contains("duplicate"));

        let store = crate::task_store::TaskStore::load(&tasks_path(tmp.path())).unwrap();
        for row in store.all() {
            assert!(
                matches!(row.status, crate::task::TaskStatus::Open),
                "duplicate batch must not close any task (saw {} closed)",
                row.id
            );
        }
    }

    // Unit-test split §11.5: one unknown task_id in the batch
    // closes zero tasks (atomicity).
    #[test]
    fn settlement_with_one_unknown_id_closes_nothing() {
        let tmp = workspace();
        let cfg = config_with_batch_close();
        let mut projector = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let (mut wave1, _wave2) = seed_two_waves(&mut projector);
        wave1.push("no-such-task".to_string());

        let settled = make_event(
            "forge.wave.settled",
            json!({
                "wave_id": "w1",
                "wave_index": 1,
                "settled_task_ids": wave1,
                "settled_unit_ids": ["U1", "U2"],
                "verified_base_commit": "deadbeef"
            })
            .to_string(),
        );
        let report = projector.apply(&[settled]);
        assert_eq!(report.rejected, 1, "unknown id must reject");
        assert!(report.rejections[0].reason.contains("unknown"));

        let store = crate::task_store::TaskStore::load(&tasks_path(tmp.path())).unwrap();
        for row in store.all() {
            assert!(
                matches!(row.status, crate::task::TaskStatus::Open),
                "unknown id must not close any task"
            );
        }
    }

    // Unit-test split §11.6: replaying the same already-closed
    // settlement is an idempotent no-op (matches U2's
    // `EnsureTaskBatch` replay contract).
    #[test]
    fn settlement_replay_is_idempotent_noop() {
        let tmp = workspace();
        let cfg = config_with_batch_close();
        let mut projector = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let (wave1, _wave2) = seed_two_waves(&mut projector);

        let payload = json!({
            "wave_id": "w1",
            "wave_index": 1,
            "settled_task_ids": wave1,
            "settled_unit_ids": ["U1", "U2"],
            "verified_base_commit": "deadbeef"
        })
        .to_string();

        let first = projector.apply(&[make_event("forge.wave.settled", payload.clone())]);
        assert_eq!(first.applied, 1);

        let replay = projector.apply(&[make_event("forge.wave.settled", payload)]);
        assert_eq!(replay.applied, 1, "replay must still apply (idempotent no-op)");
        assert_eq!(replay.rejected, 0);

        let store = crate::task_store::TaskStore::load(&tasks_path(tmp.path())).unwrap();
        let by_id: std::collections::HashMap<_, _> = store
            .all()
            .iter()
            .map(|t| (t.id.clone(), t.status.clone()))
            .collect();
        for id in &wave1 {
            assert!(
                matches!(by_id.get(id), Some(crate::task::TaskStatus::Closed)),
                "wave1 task {} should still be closed after replay",
                id
            );
        }
    }

    // Negative companion: an empty `settled_task_ids` array is
    // a contract violation (no task to close) and must be
    // rejected rather than silently no-op.
    #[test]
    fn settlement_with_empty_ids_is_rejected() {
        let tmp = workspace();
        let cfg = config_with_batch_close();
        let mut projector = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let _ = seed_two_waves(&mut projector);

        let settled = make_event(
            "forge.wave.settled",
            json!({
                "wave_id": "w1",
                "wave_index": 1,
                "settled_task_ids": [],
                "settled_unit_ids": [],
                "verified_base_commit": "deadbeef"
            })
            .to_string(),
        );
        let report = projector.apply(&[settled]);
        assert_eq!(report.rejected, 1);
        assert!(report.rejections[0].reason.contains("empty"));
    }

    // Negative companion: a payload that mixes open and closed
    // rows in the same batch is identity drift and must fail
    // (no silent partial close).
    #[test]
    fn settlement_with_mixed_open_closed_ids_is_rejected() {
        let tmp = workspace();
        let cfg = config_with_batch_close();
        let mut projector = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let (wave1, _wave2) = seed_two_waves(&mut projector);

        // Pre-close wave1[0] via a direct settlement.
        let first = make_event(
            "forge.wave.settled",
            json!({
                "wave_id": "w1",
                "wave_index": 1,
                "settled_task_ids": vec![wave1[0].clone()],
                "settled_unit_ids": ["U1"],
                "verified_base_commit": "deadbeef"
            })
            .to_string(),
        );
        let pre = projector.apply(&[first]);
        assert_eq!(pre.applied, 1);

        // Now mix closed batch (wave1[0]) with an open one
        // (wave1[1]). Must reject.
        let mixed = make_event(
            "forge.wave.settled",
            json!({
                "wave_id": "w1",
                "wave_index": 1,
                "settled_task_ids": wave1,
                "settled_unit_ids": ["U1", "U2"],
                "verified_base_commit": "deadbeef"
            })
            .to_string(),
        );
        let report = projector.apply(&[mixed]);
        assert_eq!(report.rejected, 1, "mixed batch must reject");
        assert!(
            report.rejections[0].reason.contains("mixes open") ||
                report.rejections[0].reason.contains("identity drift"),
            "rejection must call out identity drift: got {}",
            report.rejections[0].reason
        );
    }
}
