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
        actions: Default::default(),
        actions_chain: Default::default(),
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
fn happy_path_queue_advance_advances_current_step() {
    let tmp = workspace();
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
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
            actions_chain: Default::default(),
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
    assert!(
        progress.contains("step-02"),
        "Current Step must advance to next_step value, got:\n{progress}"
    );
    assert!(
        progress.contains("- step-01"),
        "Completed Steps must include completed_step value, got:\n{progress}"
    );
}

#[test]
fn projected_topics_list_is_locked() {
    // R6 (2026-06-17-005 fix plan): review/plan-blocked topics
    // removed; declared surface must match implementation.
    //
    // The assert_eq! below doubles as the "removed topics stay
    // out" check: any future re-add of `review.passed` /
    // `review.failed` / `plan.blocked` must come with a
    // matching `StateProjectionAction` variant and an
    // explanation in commit / plan / docs. See the comment on
    // `PROJECTED_TOPICS` in `state_projector/mod.rs` for the
    // Phase 2 re-introduction protocol.
    assert_eq!(
        PROJECTED_TOPICS,
        &["work.ready", "work.done", "queue.advance", "plan.complete",]
    );
    // Belt-and-suspenders reverse check: if a future refactor
    // ever widens the list without the corresponding
    // `StateProjectionAction` mapping, this catches the
    // mismatch before it ships.
    for forbidden in ["review.passed", "review.failed", "plan.blocked"] {
        assert!(
            !PROJECTED_TOPICS.contains(&forbidden),
            "PROJECTED_TOPICS must not contain `{forbidden}`; \
             re-introducing it requires a matching StateProjectionAction variant \
             and a plan / commit message explaining why (R6 of 2026-06-17-005)"
        );
    }
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
    let mut proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), make_config()));
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
