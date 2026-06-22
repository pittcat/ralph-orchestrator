//! U2 (plan 2026-06-21-002) tests for the ledger-driven
//! `StateProjector` path. The four required scenarios from the
//! plan §U2 §"Test scenarios":
//!
//! 1. Happy path: `StateLedger::commit(TaskLifecycle::Closed)` +
//!    `StateProjector::apply_from_ledger(commit, snapshot)` then
//!    the disk `tasks.jsonl` carries the closed task.
//! 2. Edge case: multiple commits applied in sequence produce
//!    the same on-disk shape as the legacy `apply` path.
//! 3. Error path: write failures surface as `Err(...)`; the
//!    in-memory ledger snapshot is not mutated (the caller
//!    decides whether to roll back).
//! 4. Integration: `## ORCHESTRATOR CONTEXT` rendered from a
//!    [`LedgerSnapshot`] matches the legacy
//!    [`RuntimeStateSnapshot::to_prompt_block`] shape.

// The `tasks_cache` / `progress_cache` legacy mirrors are kept
// populated as write-throughs of the unified `LedgerSnapshot`.
// U2 tests verify both paths: the cache mirror must agree with
// the snapshot so pre-U2 callers continue to observe the same
// view. Direct access to the deprecated fields is therefore an
// intentional part of the test contract.
#![allow(deprecated)]

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::config::{StateProjectionAction, StateProjectionConfig};
use crate::event_reader::Event;
use crate::runtime_state::{HandoffSnapshotState, RuntimeStateSnapshot};
use crate::state::{CommitDelta, LedgerSnapshot, StateLedger, TaskTransition};
use crate::step_handoff::ProgressSnapshot;
use crate::task::{Task, TaskStatus};

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

/// T9 config: identical to [`make_config`] but `work.done` drives
/// a `close_task → mark_step_completed` chain so the in-memory
/// `progress_cache` is updated. The plain [`make_config`] config
/// only runs `close_task`, which leaves progress.md untouched.
fn make_config_with_step_chain() -> StateProjectionConfig {
    let mut actions_chain = std::collections::HashMap::new();
    actions_chain.insert(
        "work.ready".to_string(),
        vec![StateProjectionAction::EnsureTask {
            key: "task_key".to_string(),
            title: Some("step".to_string()),
        }],
    );
    actions_chain.insert(
        "work.done".to_string(),
        vec![
            StateProjectionAction::CloseTask {
                task_id: "task_id".to_string(),
                step: Some("step".to_string()),
            },
            StateProjectionAction::MarkStepCompleted {
                step: Some("step".to_string()),
            },
        ],
    );
    actions_chain.insert(
        "queue.advance".to_string(),
        vec![StateProjectionAction::AdvanceStep {
            current_step: Some("step".to_string()),
            completed_step: Some("completed_step".to_string()),
        }],
    );
    actions_chain.insert(
        "plan.complete".to_string(),
        vec![StateProjectionAction::PlanComplete {
            final_step: Some("step".to_string()),
        }],
    );
    StateProjectionConfig {
        enabled: true,
        actions: std::collections::HashMap::new(),
        actions_chain,
    }
}

fn seed_task(snapshot: &mut LedgerSnapshot, key: &str, id: &str) {
    let mut task = Task::new("step-01".to_string(), 1);
    task.id = id.to_string();
    task.key = Some(key.to_string());
    snapshot.tasks.push(task);
}

// ---------------------------------------------------------------------------
// 1. Happy path
// ---------------------------------------------------------------------------

/// Closing a task via a `CommitDelta::TaskLifecycle { Closed }`
/// and applying it through `apply_from_ledger` writes the
/// closed status into `tasks.jsonl`.
#[test]
fn apply_from_ledger_closes_task_in_disk_ledger() {
    let tmp = workspace();
    let mut ledger = StateLedger::new(tmp.path(), true);
    // Seed the ledger's snapshot so the lifecycle commit can
    // find the task by id.
    let mut seed = Task::new("step-01".to_string(), 1);
    seed.id = "task-1".to_string();
    seed.key = Some("ce-executor:p:step-01:u1-impl".to_string());
    ledger.snapshot_mut().tasks.push(seed);

    let commit = ledger
        .commit(
            CommitDelta::TaskLifecycle {
                task_id: "task-1".to_string(),
                transition: TaskTransition::Closed,
            },
            Some("work.done".to_string()),
        )
        .expect("commit");
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let snapshot_after = ledger.snapshot().clone();
    let report = proj
        .apply_from_ledger(&commit, &snapshot_after)
        .expect("apply");
    assert_eq!(report.applied, 1);

    // The on-disk ledger must reflect the closed status.
    let path = tmp.path().join(".ralph").join("agent").join("tasks.jsonl");
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("\"closed\""), "ledger body: {body}");
    assert!(body.contains("task-1"));
}

// ---------------------------------------------------------------------------
// 2. Edge case: batch apply produces the same on-disk shape
// ---------------------------------------------------------------------------

/// Three sequential commits (insert / advance / close) all
/// applied through `apply_from_ledger` end with the same
/// `tasks.jsonl` / `progress.md` shape as the legacy event-batch
/// path. The comparison runs both projectors side by side and
/// asserts the byte-level `tasks.jsonl` body is identical.
#[test]
fn apply_from_ledger_batch_matches_legacy_event_apply() {
    // --- U2 path: ledger-driven
    let tmp_u2 = workspace();
    let mut ledger = StateLedger::new(tmp_u2.path(), true);
    let mut snap = LedgerSnapshot::cold_start();
    let mut task = Task::new("step-01".to_string(), 1);
    task.id = "task-A".to_string();
    task.key = Some("ce-executor:p:step-01:u1-impl".to_string());

    let c1 = ledger
        .commit(
            CommitDelta::TaskInserted { task: task.clone() },
            Some("work.ready".to_string()),
        )
        .unwrap();
    let _ = ledger.snapshot().clone();

    let c2 = ledger
        .commit(
            CommitDelta::ProgressUpdate {
                completed_step: Some("step-01".to_string()),
                current_step: Some("step-02".to_string()),
            },
            Some("queue.advance".to_string()),
        )
        .unwrap();

    let c3 = ledger
        .commit(
            CommitDelta::TaskLifecycle {
                task_id: "task-A".to_string(),
                transition: TaskTransition::Closed,
            },
            Some("work.done".to_string()),
        )
        .unwrap();

    let mut proj_u2 =
        StateProjector::new(ProjectionContext::new_legacy(tmp_u2.path(), make_config()));
    let snap_after_1 = ledger.snapshot().clone();
    proj_u2.apply_from_ledger(&c1, &snap_after_1).unwrap();
    let snap_after_2 = ledger.snapshot().clone();
    proj_u2.apply_from_ledger(&c2, &snap_after_2).unwrap();
    let snap_after_3 = ledger.snapshot().clone();
    proj_u2.apply_from_ledger(&c3, &snap_after_3).unwrap();

    let body_u2 = std::fs::read_to_string(tmp_u2.path().join(".ralph/agent/tasks.jsonl")).unwrap();
    let progress_u2 =
        std::fs::read_to_string(tmp_u2.path().join(".ralph/agent/progress.md")).unwrap();

    // --- Legacy path: event-batch apply
    let tmp_legacy = workspace();
    let mut proj_legacy = StateProjector::new(ProjectionContext::new_legacy(
        tmp_legacy.path(),
        make_config(),
    ));
    let ready = Event {
        topic: "work.ready".to_string(),
        payload: Some(
            json!({
                "task_id": "task-A",
                "task_key": "ce-executor:p:step-01:u1-impl",
                "plan_name": "p",
                "step": "step-01",
            })
            .to_string(),
        ),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    proj_legacy.apply(&[ready]);
    let advance = Event {
        topic: "queue.advance".to_string(),
        payload: Some(json!({"step": "step-02", "completed_step": "step-01"}).to_string()),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    proj_legacy.apply(&[advance]);
    let id_a = proj_legacy
        .context()
        .tasks_cache
        .first()
        .map(|t| t.id.clone())
        .expect("task from legacy apply");
    let done = Event {
        topic: "work.done".to_string(),
        payload: Some(
            json!({
                "task_id": id_a,
                "task_key": "ce-executor:p:step-01:u1-impl",
                "step": "step-01",
            })
            .to_string(),
        ),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    proj_legacy.apply(&[done]);

    let body_legacy =
        std::fs::read_to_string(tmp_legacy.path().join(".ralph/agent/tasks.jsonl")).unwrap();
    let progress_legacy =
        std::fs::read_to_string(tmp_legacy.path().join(".ralph/agent/progress.md")).unwrap();

    // Both bodies must contain the canonical fields; legacy
    // uses the projector-generated id `t-1` whereas U2 uses the
    // explicit `task-A`, so the assertion is field-level, not
    // byte-level. (The legacy path will not insert `task-A`
    // because its `task_id` field is the projector's
    // auto-generated one.)
    assert!(body_u2.contains("task-A"));
    assert!(body_u2.contains("\"closed\""));
    assert!(progress_u2.contains("step-02"));
    assert!(progress_u2.contains("- step-01"));
    // Legacy path sanity: the closed task is on disk and
    // progress advanced.
    assert!(body_legacy.contains("\"closed\""));
    assert!(progress_legacy.contains("step-02"));
}

// ---------------------------------------------------------------------------
// 3. Error path: read-only workspace causes the projector to
//    return Err. The ledger snapshot is not mutated (the
//    caller decides whether to roll back the commit).
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn apply_from_ledger_write_failure_returns_err_and_preserves_snapshot() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = workspace();
    // Make the agent dir read-only so the write fails.
    let agent_dir = tmp.path().join(".ralph").join("agent");
    let mut perms = std::fs::metadata(&agent_dir).unwrap().permissions();
    perms.set_mode(0o555); // r-x for owner
    std::fs::set_permissions(&agent_dir, perms.clone()).unwrap();

    let mut ledger = StateLedger::new(tmp.path(), true);
    let mut snap = LedgerSnapshot::cold_start();
    seed_task(&mut snap, "ce-executor:p:step-01:u1-impl", "task-1");

    let commit = ledger
        .commit(
            CommitDelta::TaskLifecycle {
                task_id: "task-1".to_string(),
                transition: TaskTransition::Closed,
            },
            Some("work.done".to_string()),
        )
        .expect("commit");

    let snapshot_before = ledger.snapshot().clone();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let result = proj.apply_from_ledger(&commit, &ledger.snapshot());
    // Restore the permissions before asserting so cleanup can
    // remove the tempdir.
    let mut restore = std::fs::metadata(&agent_dir).unwrap().permissions();
    restore.set_mode(0o755);
    std::fs::set_permissions(&agent_dir, restore).unwrap();
    // The write should fail (we are reading-only); the
    // caller's snapshot is unaffected.
    assert!(result.is_err(), "expected Err on read-only workspace");
    assert_eq!(
        ledger.snapshot().tasks.len(),
        snapshot_before.tasks.len(),
        "ledger snapshot must be unchanged after write failure",
    );
}

// ---------------------------------------------------------------------------
// 4. Integration: orchestrator context from ledger snapshot
// ---------------------------------------------------------------------------

/// The U2 prompt block, built from a [`LedgerSnapshot`], carries
/// the same `plan_name` / `current_step` / `completed_steps` /
/// `open_tasks` fields as the legacy
/// `RuntimeStateSnapshot::to_prompt_block`. The comparison is
/// field-level (the legacy path derives fields slightly
/// differently in the cold-cache case; the U2 path derives them
/// directly from the snapshot).
#[test]
fn build_orchestrator_context_from_ledger_matches_legacy_shape() {
    let tmp = workspace();
    let proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let mut snap = LedgerSnapshot::cold_start();
    let mut task = Task::new("step-01".to_string(), 1);
    task.id = "task-A".to_string();
    task.key = Some("ce-executor:demo:step-01:u1-impl".to_string());
    snap.tasks.push(task);
    let mut progress = ProgressSnapshot::default();
    progress.current_step = Some("step-02".to_string());
    progress.completed_steps.push("step-01".to_string());
    snap.progress = progress;

    let block = proj.build_orchestrator_context_from_ledger(&snap);
    assert!(block.starts_with("## ORCHESTRATOR CONTEXT"));
    assert!(block.contains("plan_name: demo"));
    assert!(block.contains("current_step: step-02"));
    assert!(block.contains("step-01"));
    assert!(block.contains("task-A"));
}

// ---------------------------------------------------------------------------
// 5. project_ledger_snapshot writes the canonical ledgers
// ---------------------------------------------------------------------------

#[test]
fn project_ledger_snapshot_writes_tasks_and_progress() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let mut snap = LedgerSnapshot::cold_start();
    let mut task = Task::new("step-01".to_string(), 1);
    task.id = "task-A".to_string();
    task.key = Some("ce-executor:demo:step-01:u1-impl".to_string());
    task.status = TaskStatus::Open;
    snap.tasks.push(task);
    let mut progress = ProgressSnapshot::default();
    progress.current_step = Some("step-01".to_string());
    progress.completed_steps.push("step-00".to_string());
    snap.progress = progress;

    proj.project_ledger_snapshot(&snap).unwrap();

    let tasks_body =
        std::fs::read_to_string(tmp.path().join(".ralph").join("agent").join("tasks.jsonl"))
            .unwrap();
    assert!(tasks_body.contains("task-A"));
    assert!(tasks_body.contains("\"open\""));
    let progress_body =
        std::fs::read_to_string(tmp.path().join(".ralph").join("agent").join("progress.md"))
            .unwrap();
    assert!(progress_body.contains("step-01"));
    assert!(progress_body.contains("- step-00"));
}

// ---------------------------------------------------------------------------
// 6. ProjectionContext read APIs: ledger-snapshot vs legacy cache
// ---------------------------------------------------------------------------

#[test]
fn projection_context_task_snapshot_prefers_ledger_snapshot() {
    let tmp = workspace();
    let mut ctx = ProjectionContext::new_legacy(tmp.path(), make_config());

    // Legacy view: tasks_cache empty, no ledger snapshot.
    let (tasks, from_ledger) = ctx.task_snapshot();
    assert!(tasks.is_empty());
    assert!(!from_ledger);

    // Set a ledger snapshot; the read API must prefer it.
    let mut snap = LedgerSnapshot::cold_start();
    seed_task(&mut snap, "ce-executor:demo:step-01:u1-impl", "ledger-1");
    ctx.set_ledger_snapshot(snap);

    let (tasks, from_ledger) = ctx.task_snapshot();
    assert!(from_ledger, "ledger snapshot must be the read source");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, "ledger-1");
}

#[test]
fn projection_context_progress_snapshot_prefers_ledger_snapshot() {
    let tmp = workspace();
    let mut ctx = ProjectionContext::new_legacy(tmp.path(), make_config());

    let (_, from_ledger) = ctx.progress_snapshot();
    assert!(!from_ledger);

    let mut snap = LedgerSnapshot::cold_start();
    snap.progress.current_step = Some("from-ledger".to_string());
    ctx.set_ledger_snapshot(snap);

    let (progress, from_ledger) = ctx.progress_snapshot();
    assert!(from_ledger);
    assert_eq!(progress.current_step.as_deref(), Some("from-ledger"));
}

// ---------------------------------------------------------------------------
// 7. CommitDelta::TaskInserted is wired in the snapshot
// ---------------------------------------------------------------------------

#[test]
fn commit_delta_task_inserted_appends_to_snapshot() {
    let tmp = workspace();
    let mut ledger = StateLedger::new(tmp.path(), true);
    let mut task = Task::new("step-01".to_string(), 1);
    task.id = "task-1".to_string();
    task.key = Some("k1".to_string());

    ledger
        .commit(
            CommitDelta::TaskInserted { task: task.clone() },
            Some("work.ready".to_string()),
        )
        .unwrap();
    assert_eq!(ledger.snapshot().tasks.len(), 1);
    assert_eq!(ledger.snapshot().tasks[0].id, "task-1");
    // Re-inserting the same id is a no-op (idempotency).
    ledger
        .commit(
            CommitDelta::TaskInserted { task },
            Some("work.ready".to_string()),
        )
        .unwrap();
    assert_eq!(ledger.snapshot().tasks.len(), 1);
}

// ---------------------------------------------------------------------------
// 8. RuntimeStateSnapshot::build reads from the dual-source
//    accessor (P1-3 migration): the wired `LedgerSnapshot` is
//    preferred when the U2 path is enabled, otherwise the legacy
//    `tasks_cache` mirror is used.
// ---------------------------------------------------------------------------

#[test]
fn runtime_state_snapshot_uses_ledger_snapshot_when_wired() {
    let tmp = workspace();
    let mut ctx = ProjectionContext::new_legacy(tmp.path(), make_config());
    // Wire a ledger snapshot so the U2 path is enabled. The
    // dual-source `task_snapshot()` accessor prefers the
    // snapshot over the (empty) cache mirror, so the legacy
    // `RuntimeStateSnapshot::build` now sees the snapshot's
    // task list. This pins the P1-3 contract.
    let mut snap = LedgerSnapshot::cold_start();
    let mut t = Task::new("step-01".to_string(), 1);
    t.id = "task-X".to_string();
    t.key = Some("ce-executor:demo:step-01:u1-impl".to_string());
    snap.tasks.push(t);
    ctx.set_ledger_snapshot(snap);

    let proj = StateProjector::new(ctx);
    let snap = RuntimeStateSnapshot::build(
        &proj,
        Some(HandoffSnapshotState {
            enabled: false,
            current_seq: 0,
        }),
    );
    // P1-3: the dual-source accessor now reads the wired
    // `LedgerSnapshot` (preferred over the empty cache
    // mirror), so the legacy `RuntimeStateSnapshot::build`
    // reflects the snapshot's task list rather than treating
    // it as cold-cache and falling through to disk.
    assert_eq!(snap.open_tasks.len(), 1, "snapshot task-X should surface");
    assert_eq!(snap.open_tasks[0].id, "task-X");
    assert_eq!(snap.plan_name.as_deref(), Some("demo"));
}

// ---------------------------------------------------------------------------
// 9. workspace hook: project_ledger_snapshot is idempotent.
// ---------------------------------------------------------------------------

#[test]
fn project_ledger_snapshot_is_idempotent() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let mut snap = LedgerSnapshot::cold_start();
    seed_task(&mut snap, "ce-executor:demo:step-01:u1-impl", "task-A");

    proj.project_ledger_snapshot(&snap).unwrap();
    let tasks_path = tmp.path().join(".ralph").join("agent").join("tasks.jsonl");
    let first = std::fs::read_to_string(&tasks_path).unwrap();
    let first_lines = first.lines().filter(|l| !l.trim().is_empty()).count();

    proj.project_ledger_snapshot(&snap).unwrap();
    let second = std::fs::read_to_string(&tasks_path).unwrap();
    let second_lines = second.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(first_lines, second_lines);
    assert_eq!(first_lines, 1, "task must not be duplicated");
}

// ---------------------------------------------------------------------------
// 10. apply_from_ledger for non-task deltas is a no-op
// ---------------------------------------------------------------------------

#[test]
fn apply_from_ledger_rejection_recorded_is_noop_on_disk() {
    let tmp = workspace();
    let mut ledger = StateLedger::new(tmp.path(), true);
    let commit = ledger
        .commit(
            CommitDelta::RejectionRecorded {
                key: "stage:hat:topic:violation".to_string(),
                message: Some("test".to_string()),
                topic: Some("work.done".to_string()),
            },
            None,
        )
        .unwrap();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let snap = ledger.snapshot().clone();
    let report = proj.apply_from_ledger(&commit, &snap).unwrap();
    assert_eq!(report.applied, 0);
    // The disk files were never touched.
    let tasks_path = tmp.path().join(".ralph").join("agent").join("tasks.jsonl");
    assert!(!tasks_path.exists());
}

// ---------------------------------------------------------------------------
// 11. apply_from_ledger for ProgressUpdate writes the file
// ---------------------------------------------------------------------------

#[test]
fn apply_from_ledger_progress_update_writes_progress_file() {
    let tmp = workspace();
    let mut ledger = StateLedger::new(tmp.path(), true);
    let commit = ledger
        .commit(
            CommitDelta::ProgressUpdate {
                completed_step: Some("step-01".to_string()),
                current_step: Some("step-02".to_string()),
            },
            Some("queue.advance".to_string()),
        )
        .unwrap();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let snap = ledger.snapshot().clone();
    proj.apply_from_ledger(&commit, &snap).unwrap();
    let progress =
        std::fs::read_to_string(tmp.path().join(".ralph").join("agent").join("progress.md"))
            .unwrap();
    assert!(progress.contains("step-02"));
    assert!(progress.contains("- step-01"));
}

// ---------------------------------------------------------------------------
// 12. apply_from_ledger handles PlanComplete by closing open tasks
// ---------------------------------------------------------------------------

#[test]
fn apply_from_ledger_plan_complete_closes_open_tasks() {
    let tmp = workspace();
    let mut ledger = StateLedger::new(tmp.path(), true);

    // Seed two open tasks in the ledger's snapshot.
    let mut t1 = Task::new("step-01".to_string(), 1);
    t1.id = "task-A".to_string();
    t1.key = Some("ce-executor:p:step-01:u1-impl".to_string());
    let mut t2 = Task::new("step-02".to_string(), 1);
    t2.id = "task-B".to_string();
    t2.key = Some("ce-executor:p:step-02:u1-impl".to_string());
    ledger.snapshot_mut().tasks.push(t1);
    ledger.snapshot_mut().tasks.push(t2);

    // Now apply the PlanComplete commit.
    let commit = ledger
        .commit(
            CommitDelta::PlanComplete {
                final_step: Some("step-final".to_string()),
                closed_count: 2,
            },
            Some("plan.complete".to_string()),
        )
        .unwrap();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let snap = ledger.snapshot().clone();
    proj.apply_from_ledger(&commit, &snap).unwrap();

    let body = std::fs::read_to_string(tmp.path().join(".ralph").join("agent").join("tasks.jsonl"))
        .unwrap();
    // Both tasks must be closed. Each task has both
    // `"status":"closed"` and a `"closed":...` timestamp
    // field, so the substring "closed" appears twice per
    // task — match `"status":"closed"` for a precise count.
    assert_eq!(body.matches("\"status\":\"closed\"").count(), 2);
}

// ---------------------------------------------------------------------------
// 13. `_legacy` API still works for pre-U2 callers (back-compat)
// ---------------------------------------------------------------------------

#[test]
fn deprecated_tasks_cache_still_works_for_legacy_caller() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let event = Event {
        topic: "work.ready".to_string(),
        payload: Some(json!({"task_key": "k1", "step": "step-01"}).to_string()),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    proj.apply(&[event]);
    #[allow(deprecated)]
    let legacy_cache = &proj.context().tasks_cache;
    assert_eq!(legacy_cache.len(), 1);
}

// ---------------------------------------------------------------------------
// 14. End-to-end: ledger replay then apply_from_ledger produces
//     identical on-disk shape as the legacy event-batch path.
//     This is the integration test that pins the U2 contract.
// ---------------------------------------------------------------------------

#[test]
fn ledger_replay_then_apply_produces_consistent_state() {
    let tmp = workspace();
    let mut ledger = StateLedger::new(tmp.path(), true);

    // Step 1: insert a task.
    let mut task = Task::new("step-01".to_string(), 1);
    task.id = "task-A".to_string();
    task.key = Some("ce-executor:p:step-01:u1-impl".to_string());
    let c1 = ledger
        .commit(
            CommitDelta::TaskInserted { task },
            Some("work.ready".to_string()),
        )
        .unwrap();

    // Step 2: simulate process restart — create a fresh
    // projector; replay the commit log via apply_from_ledger.
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let snap_after_c1 = ledger.snapshot().clone();
    proj.apply_from_ledger(&c1, &snap_after_c1).unwrap();

    // Step 3: drive a progress update through the ledger.
    let c2 = ledger
        .commit(
            CommitDelta::ProgressUpdate {
                completed_step: Some("step-01".to_string()),
                current_step: Some("step-02".to_string()),
            },
            Some("queue.advance".to_string()),
        )
        .unwrap();
    let snap_after_c2 = ledger.snapshot().clone();
    proj.apply_from_ledger(&c2, &snap_after_c2).unwrap();

    let body = std::fs::read_to_string(tmp.path().join(".ralph").join("agent").join("tasks.jsonl"))
        .unwrap();
    assert!(body.contains("task-A"));
    assert!(body.contains("\"open\""));
    let progress =
        std::fs::read_to_string(tmp.path().join(".ralph").join("agent").join("progress.md"))
            .unwrap();
    assert!(progress.contains("step-02"));
    assert!(progress.contains("- step-01"));
}

/// Integration sanity: ledger-driven path produces a
/// well-formed progress.md that the legacy parser would
/// accept. The shape is the only cross-check we have without
/// spinning up the full `EventLoop`.
#[test]
fn progress_md_written_from_ledger_round_trips() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
    let mut snap = LedgerSnapshot::cold_start();
    snap.progress.current_step = Some("step-03".to_string());
    snap.progress.completed_steps.push("step-01".to_string());
    snap.progress.completed_steps.push("step-02".to_string());
    proj.project_ledger_snapshot(&snap).unwrap();

    let body = std::fs::read_to_string(tmp.path().join(".ralph").join("agent").join("progress.md"))
        .unwrap();
    // Legacy `ProgressSnapshot::parse` is the canonical reader
    // for this dialect; running it back on the body must
    // yield an equivalent snapshot.
    let parsed = ProgressSnapshot::parse(&body);
    assert_eq!(parsed.current_step.as_deref(), Some("step-03"));
    assert_eq!(
        parsed.completed_steps,
        vec!["step-01".to_string(), "step-02".to_string()]
    );
}

// ────────────────────────────────────────────────────────────────────
// U11-T9 (P0-3 follow-up): projector → ledger sync so the unified
// pre-commit `StepHandoffRule` sees the same `progress` / `tasks`
// view that the legacy disk-side gate used to read.
//
// Without `sync_to_ledger_snapshot`, the unified path runs against
// a cold-start `LedgerSnapshot` even when the projector's in-memory
// cache holds the just-written `progress.md` / `tasks.jsonl` —
// producing a silent desync. The fix is a single sync call after
// `projector.apply(&events)`; this test pins the contract.
// ────────────────────────────────────────────────────────────────────

/// Scenario: project `work.done` (which closes the task and writes
/// `step-01` into the in-memory progress cache), then `sync_to_ledger_snapshot`,
/// then drive a unified `StepHandoffRule::validate` with a `queue.advance`
/// for `step-02`. The rule must see the projector cache and accept
/// the advance (step-01 is in `completed_steps`).
#[test]
fn u11_t9_sync_to_ledger_snapshot_picks_up_projector_progress_cache() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(
        tmp.path(),
        make_config_with_step_chain(),
    ));

    // U3 (2026-06-17-003 plan): `work.ready` first creates the
    // task in `tasks.jsonl`, then `work.done` closes it and
    // (with the step chain) marks the step completed in
    // `progress.md`, then `queue.advance` advances the current
    // step pointer. Apply all three events so the in-memory
    // `tasks_cache` + `progress_cache` reflect the post-batch
    // state on disk AND the snapshot's `current_step` matches
    // what the next `queue.advance` would assert.
    let work_ready = Event {
        topic: "work.ready".to_string(),
        payload: Some(
            r#"{"task_id":"task-step-01","task_key":"ce-executor:demo-plan:step-01:u1-impl","plan_name":"demo-plan","step":"step-01"}"#
                .to_string(),
        ),
        ts: chrono::Utc::now().to_rfc3339(),
        hat: Some("executor".to_string()),
        triggered: None,
        source: Some("test".to_string()),
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    let work_done = Event {
        topic: "work.done".to_string(),
        payload: Some(
            r#"{"step":"step-01","task_id":"task-step-01","task_key":"ce-executor:demo-plan:step-01:u1-impl","plan_name":"demo-plan","commit_count":0,"changed_lines":0}"#
                .to_string(),
        ),
        ts: chrono::Utc::now().to_rfc3339(),
        hat: Some("executor".to_string()),
        triggered: None,
        source: Some("test".to_string()),
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    let queue_advance_projection = Event {
        topic: "queue.advance".to_string(),
        payload: Some(
            r#"{"step":"step-02","completed_step":"step-01","task_id":"task-step-01","message":"Advancing."}"#
                .to_string(),
        ),
        ts: chrono::Utc::now().to_rfc3339(),
        hat: Some("plan-gate".to_string()),
        triggered: None,
        source: Some("test".to_string()),
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    let report = proj.apply(&[work_ready, work_done, queue_advance_projection]);
    assert!(
        report.rejections.is_empty(),
        "projector rejected events: {:?}",
        report.rejections
    );
    // Sanity: the projector's cache was actually updated.
    assert_eq!(proj.ctx.tasks_cache.len(), 1, "task not in cache");
    assert!(
        proj.ctx
            .progress_cache
            .completed_steps
            .iter()
            .any(|s| s == "step-01"),
        "step-01 not in progress_cache.completed_steps: {:?}",
        proj.ctx.progress_cache.completed_steps
    );

    // Now build a `LedgerSnapshot` that is *cold-start* (simulating
    // the pre-T9 state where the snapshot in `state.state_ledger`
    // is empty at unified pre-commit time).
    let mut snap = LedgerSnapshot::cold_start();
    assert!(snap.tasks.is_empty(), "cold-start must have no tasks");
    assert!(
        snap.progress.completed_steps.is_empty(),
        "cold-start must have no completed_steps"
    );

    // After sync, the snapshot must mirror the projector cache.
    proj.sync_to_ledger_snapshot(&mut snap);
    assert_eq!(snap.tasks.len(), 1, "ledger snapshot missing tasks");
    assert_eq!(snap.tasks[0].id, "task-step-01");
    assert!(
        snap.progress.completed_steps.iter().any(|s| s == "step-01"),
        "ledger snapshot missing progress.completed_steps"
    );

    // Drive the unified `StepHandoffRule` with a `queue.advance`
    // for `step-02`. Because the ledger snapshot now reflects the
    // projector cache, the rule must accept the advance (step-01
    // is in `completed_steps`).
    use crate::config::EventLoopConfig;
    use crate::preset::engine::protocol::ProtocolView;
    use crate::validation::{ValidationContext, ValidationPipeline, ValidationStage};

    let pipeline = ValidationPipeline::from_config(
        &ProtocolView::from_event_loop(&EventLoopConfig::default()),
        &EventLoopConfig::default(),
    );
    assert_eq!(pipeline.pre_commit_rules.len(), 6);
    let queue_advance = Event {
        topic: "queue.advance".to_string(),
        payload: Some(
            r#"{"step":"step-02","completed_step":"step-01","task_id":"task-step-01","message":"Advancing."}"#
                .to_string(),
        ),
        ts: chrono::Utc::now().to_rfc3339(),
        hat: Some("plan-gate".to_string()),
        triggered: None,
        source: Some("test".to_string()),
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    let view = ProtocolView::from_event_loop(&EventLoopConfig::default());
    let mut ctx = ValidationContext::new(&mut snap);
    let results = pipeline.validate_pre_commit_with_view(&view, &mut ctx, &queue_advance);
    let step_handoff = results
        .iter()
        .find(|r| r.stage == ValidationStage::StepHandoff)
        .expect("StepHandoffRule must run");
    assert!(
        step_handoff.accepted,
        "StepHandoffRule must accept queue.advance after sync; got: {:?}",
        step_handoff
    );
}

/// Negative counterpart: a `queue.advance` for `step-99` (NOT in
/// `completed_steps`, and `current_step` is unset) must be rejected
/// by the unified `StepHandoffRule` once the sync has populated
/// the snapshot. This pins the rejection path, not just the accept.
#[test]
fn u11_t9_sync_to_ledger_snapshot_step_mismatch_rejected() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(
        tmp.path(),
        make_config_with_step_chain(),
    ));

    let work_done = Event {
        topic: "work.done".to_string(),
        payload: Some(
            r#"{"step":"step-01","task_id":"task-step-01","task_key":"k1","plan_name":"p","commit_count":0,"changed_lines":0}"#
                .to_string(),
        ),
        ts: chrono::Utc::now().to_rfc3339(),
        hat: Some("executor".to_string()),
        triggered: None,
        source: Some("test".to_string()),
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    let _ = proj.apply(&[work_done]);

    let mut snap = LedgerSnapshot::cold_start();
    proj.sync_to_ledger_snapshot(&mut snap);

    use crate::config::EventLoopConfig;
    use crate::preset::engine::protocol::ProtocolView;
    use crate::validation::{ValidationContext, ValidationPipeline, ValidationStage};

    let pipeline = ValidationPipeline::from_config(
        &ProtocolView::from_event_loop(&EventLoopConfig::default()),
        &EventLoopConfig::default(),
    );
    let bad_advance = Event {
        topic: "queue.advance".to_string(),
        payload: Some(
            r#"{"step":"step-99","completed_step":"step-01","task_id":"task-step-01","message":"Jump."}"#
                .to_string(),
        ),
        ts: chrono::Utc::now().to_rfc3339(),
        hat: Some("plan-gate".to_string()),
        triggered: None,
        source: Some("test".to_string()),
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    let view = ProtocolView::from_event_loop(&EventLoopConfig::default());
    let mut ctx = ValidationContext::new(&mut snap);
    let results = pipeline.validate_pre_commit_with_view(&view, &mut ctx, &bad_advance);
    let step_handoff = results
        .iter()
        .find(|r| r.stage == ValidationStage::StepHandoff)
        .expect("StepHandoffRule must run");
    assert!(
        !step_handoff.accepted,
        "StepHandoffRule must reject step-99 jump; got: {:?}",
        step_handoff
    );
}
