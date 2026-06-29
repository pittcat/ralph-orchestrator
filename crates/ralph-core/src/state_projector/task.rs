//! Task ledger projection.
//!
//! Plan ref: U2 of
//! `docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md`.
//!
//! These two helpers write `.ralph/agent/tasks.jsonl` from the
//! projector. The projector is the **sole** writer in Phase 1
//! (preset instructions ban the agent from calling
//! `ralph tools task ensure|start|close|fail`).
//!
//! Failures bubble up as `Err(reason)`; the caller converts them
//! into a [`super::Rejection`] and the bus emits
//! `event.state_projection.rejected`.

// This module **owns** the deprecated `tasks_cache` mirror:
// `persist` updates it as a write-through of the on-disk ledger.
// The mirror is kept in sync with the canonical
// `LedgerSnapshot` (via `sync_to_ledger_snapshot` and
// `project_ledger_snapshot`) so legacy callers continue to
// observe the same view, while the U2 path reads from the
// snapshot directly. Touching the field here is therefore
// intentional and unavoidable.
#![allow(deprecated)]

use std::path::Path;

use serde_json::Value;

use crate::state_projector::ProjectionContext;
use crate::state_projector::json_pointer;
use crate::task::Task;
use crate::task_store::TaskStore;

/// Project a `work.ready` event into the task ledger.
///
/// `key_pointer` is a JSON pointer into the payload (e.g.
/// `"task_key"`, `"step"`); the resolved value becomes the task's
/// stable `key`. `title_pointer` is optional and defaults to `key`.
pub(crate) fn project_ensure_task(
    ctx: &mut ProjectionContext,
    payload: &Value,
    key_pointer: &str,
    title_pointer: Option<&str>,
) -> Result<(), String> {
    let key = json_pointer(payload, key_pointer)
        .ok_or_else(|| format!("missing required pointer '{key_pointer}'"))?
        .to_string();
    let title_source = title_pointer.unwrap_or(key_pointer);
    let title = json_pointer(payload, title_source)
        .unwrap_or(&key)
        .to_string();

    // Apply the change to the in-memory cache first, then persist.
    // We use `with_exclusive_lock` to honour cross-loop writes
    // (rare in Phase 1 but cheap and safe) and to keep the same
    // locking discipline the rest of the orchestrator uses.
    let mut store = TaskStore::load(&ctx.tasks_path).map_err(|e| format!("tasks_load: {e}"))?;
    // Honour the loop's R4 setting from `ProjectionContext` so the
    // projector matches loop behaviour. Previously this hard-coded
    // `false` and silently disabled the R4 gate inside the
    // projector; that bypassed the preset contract — see R1 in
    // docs/plans/2026-06-17-005-fix-state-projection-phase1-review-findings-plan.md.
    store.set_enforce_current_unit(ctx.enforce_current_unit);
    let mut task = Task::new(title, 1).with_key(Some(key.clone()));
    // Honour the payload's `task_id` when the agent supplies one
    // (ce-executor presets always do). Without this the loop
    // would round-trip through a generated id that the agent
    // can never reproduce, breaking the subsequent `work.done`.
    //
    // Fix-1 (2026-06-29 primary-072512 P0): the prior 2026-06-28
    // plan U5 fallback that synthesized `from_key:{key}` when
    // the payload's `task_id` was empty was the root cause of the
    // 4th recurrence of the same pattern group. The synthetic id
    // (a) silently swallowed a contract violation, (b) could not
    // be matched by the subsequent `work.done`, (c) triggered
    // hard-gate exhaustion, and (d) blocked the review/ship/
    // report chain. The preset line 1179 forbids empty
    // `task_id`; we now fail-closed instead of papering over the
    // mistake. The recovery path is the runner injecting
    // schema-level guidance (Fix-2 in `loop_runner/runner.rs`).
    if let Some(provided_id) = json_pointer(payload, "task_id") {
        if provided_id.is_empty() {
            return Err("empty_task_id_in_work_ready: coordinator must embed the \
                 projector-derived id (see preset ce-executor-serial line 1179)"
                .to_string());
        }
        task.id = provided_id.to_string();
    }
    if let Some(plan_name) = json_pointer(payload, "plan_name") {
        task = task.with_description(Some(format!("plan: {plan_name}")));
    }
    // P0-2 (plan 2026-06-29-006): prefer the payload's `loop_id`
    // when present, otherwise fall back to the loop marker
    // threaded in via `ProjectionContext::current_loop_id`. Without
    // this fallback, executor re-emissions that ship
    // `task_id="from_key:..."` or `task_id=""` produce a task
    // whose `loop_id` is `None`, which is then hard-rejected by
    // `validate_task` as `TaskWrongLoop { actual_loop: None }`
    // (see 2026-06-29-ce-executor-serial-primary-172725 §F3).
    if let Some(loop_id) = ctx_loop_id(payload) {
        task = task.with_loop_id(Some(loop_id.to_string()));
    } else if let Some(loop_id) = ctx.current_loop_id.as_ref() {
        task = task.with_loop_id(Some(loop_id.clone()));
    }
    // R1 (2026-06-17-005 fix plan): when the loop enables R4, the
    // projector must surface single-U collisions as `Err` so the
    // hook can drop the offending event and emit
    // `event.state_projection.rejected`. `TaskStore::ensure`
    // silently returns the existing task on collision (legacy
    // contract); we pre-check so the reject is loud.
    if let Some(collision_idx) = ctx
        .enforce_current_unit
        .then(|| store.find_unit_collision_idx(&task))
        .flatten()
    {
        let sibling = store.all().get(collision_idx);
        let sibling_key = sibling
            .and_then(|t| t.key.as_deref())
            .unwrap_or("<unknown>");
        let sibling_id = sibling.map(|t| t.id.as_str()).unwrap_or("<unknown>");
        return Err(format!(
            "r4_unit_collision: refusing work.ready for task_key='{}' \
             (R4 enforce_current_unit is active; sibling task already open in \
             the same step: sibling_task_id='{}' sibling_task_key='{}')",
            task.key.as_deref().unwrap_or("<no-key>"),
            sibling_id,
            sibling_key,
        ));
    }
    store.ensure(task);
    persist(&ctx.tasks_path, &store, &mut ctx.tasks_cache)
}

pub(crate) fn project_close_task(
    ctx: &mut ProjectionContext,
    payload: &Value,
    task_id_pointer: &str,
    step_pointer: Option<&str>,
) -> Result<(), String> {
    let task_id = json_pointer(payload, task_id_pointer)
        .ok_or_else(|| format!("missing required pointer '{task_id_pointer}'"))?
        .to_string();

    let mut store = TaskStore::load(&ctx.tasks_path).map_err(|e| format!("tasks_load: {e}"))?;
    // 2026-06-30 P0-2 (primary-153653): when multiple tasks share
    // the same task_id (coordinator emits the same id for fix-01
    // and fix-02 because the agent's prompt template reuses it),
    // `store.close(&task_id)` only closes the **first** row with
    // that id (`TaskStore::get_mut` returns the first match) —
    // later fix-unit rows stay `open` forever, producing the
    // P0-3 tasks.jsonl ↔ progress.md drift. When the payload also
    // carries a `task_key` (always present in ce-executor-serial
    // payloads), look up by key first; the key is unique per
    // step including fix-NN, so the close targets the right row.
    // Fall back to id lookup when no key is available so legacy
    // payloads keep working.
    let closed = if let Some(task_key) = json_pointer(payload, "task_key") {
        if store.get_by_key_mut(task_key).is_some() {
            store.close_by_key(task_key)
        } else {
            // Key mismatch — fall back to id lookup so we don't
            // silently no-op on malformed payloads.
            store.close(&task_id)
        }
    } else {
        store.close(&task_id)
    };
    if closed.is_none() {
        // Fail-closed: a `work.done` referencing a task_id that
        // is not in the ledger is a contract violation. The
        // existing `progress_task_gate` will also reject it; we
        // refuse to silently no-op.
        return Err(format!("task_not_found: {task_id}"));
    }
    persist(&ctx.tasks_path, &store, &mut ctx.tasks_cache)?;

    // If the event also carries a `step`, advance the progress
    // ledger. We delegate the progress write to the progress
    // sub-module to keep one source of truth.
    if let Some(step) = step_pointer
        .and_then(|p| json_pointer(payload, p))
        .map(|s| s.to_string())
    {
        crate::state_projector::progress::project_close_step(ctx, &step)
    } else {
        Ok(())
    }
}

fn persist(_path: &Path, store: &TaskStore, cache: &mut Vec<Task>) -> Result<(), String> {
    // `_path` is reserved for a future diagnostic event that
    // records the on-disk write site (e.g. via the
    // `ralph_diagnostics` collector). The path lives on the
    // context already; carrying it through `persist` keeps the
    // signature stable for that extension.
    store.save().map_err(|e| format!("tasks_save: {e}"))?;
    *cache = store.all().to_vec();
    Ok(())
}

fn ctx_loop_id(payload: &Value) -> Option<&str> {
    json_pointer(payload, "loop_id")
}

#[cfg(test)]
mod tests {
    //! P0-2 (plan 2026-06-29-006): the projector must fall back
    //! to `ProjectionContext::current_loop_id` when an event
    //! payload omits the `loop_id` field. Without the fallback,
    //! tasks projected from `task_id="from_key:..."` legacy
    //! emissions stay `loop_id=None` and trigger
    //! `TaskWrongLoop { actual_loop: None }` in `validate_task`.

    use super::*;
    use crate::state_projector::StateProjectionConfig;
    use tempfile::tempdir;

    fn ctx_with_loop_marker(workspace: &Path, loop_id: &str) -> ProjectionContext {
        ProjectionContext::new(workspace, StateProjectionConfig::default(), false)
            .with_current_loop_id(loop_id)
    }

    fn payload_with_task_id(task_id: &str, task_key: &str) -> Value {
        serde_json::json!({
            "task_id": task_id,
            "task_key": task_key,
            "plan_name": "test-plan",
        })
    }

    #[test]
    fn project_task_with_payload_loop_id_uses_payload() {
        let dir = tempdir().unwrap();
        let mut ctx = ctx_with_loop_marker(dir.path(), "loop-A");
        let payload = payload_with_task_id("task-1", "k-1");
        let payload_with_loop = serde_json::json!({
            "task_id": "task-1",
            "task_key": "k-1",
            "plan_name": "test-plan",
            "loop_id": "loop-B",
        });
        project_ensure_task(&mut ctx, &payload_with_loop, "task_key", None).unwrap();
        let tasks = ctx.task_snapshot().0;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].loop_id.as_deref(), Some("loop-B"));
        // Reference unused variables to silence lints.
        let _ = payload;
    }

    #[test]
    fn project_task_falls_back_to_ctx_loop_id_when_payload_missing() {
        let dir = tempdir().unwrap();
        let mut ctx = ctx_with_loop_marker(dir.path(), "loop-A");
        let payload = payload_with_task_id("task-1", "k-1");
        project_ensure_task(&mut ctx, &payload, "task_key", None).unwrap();
        let tasks = ctx.task_snapshot().0;
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].loop_id.as_deref(),
            Some("loop-A"),
            "P0-2: when payload has no `loop_id`, projector must use the loop marker"
        );
    }

    #[test]
    fn project_task_with_no_loop_id_anywhere_stays_none() {
        let dir = tempdir().unwrap();
        // No loop marker in ctx, no loop_id in payload.
        let mut ctx = ProjectionContext::new(dir.path(), StateProjectionConfig::default(), false);
        let payload = payload_with_task_id("task-1", "k-1");
        project_ensure_task(&mut ctx, &payload, "task_key", None).unwrap();
        let tasks = ctx.task_snapshot().0;
        assert_eq!(tasks.len(), 1);
        // Loop_id is None — preserve pre-fix behaviour for
        // non-loop-scoped presets that don't set a marker.
        assert_eq!(tasks[0].loop_id, None);
    }

    // Fix-1 (2026-06-29 primary-072512 P0): empty task_id must
    // be rejected by the projector instead of silently falling
    // back to `from_key:{key}`. The 4th recurrence of the same
    // pattern group caused the hard gate to exhaust on step-04
    // because `work.ready(task_id="")` produced a synthetic
    // task that the subsequent `work.done` could not match.
    #[test]
    fn project_task_rejects_empty_task_id() {
        let dir = tempdir().unwrap();
        let mut ctx = ctx_with_loop_marker(dir.path(), "loop-A");
        let payload = serde_json::json!({
            "task_id": "",
            "task_key": "ce-executor:test:step-01:u0-impl",
            "plan_name": "test-plan",
        });
        let result = project_ensure_task(&mut ctx, &payload, "task_key", None);
        assert!(
            result.is_err(),
            "Fix-1: empty task_id must fail-closed (was: silently fell back to from_key:)"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("empty_task_id"),
            "error must name the root cause: got {err}"
        );
        // No task should have been written to the ledger.
        let tasks = ctx.task_snapshot().0;
        assert!(
            tasks.is_empty(),
            "Fix-1: rejected event must not write to ledger (was: wrote synthetic from_key: task)"
        );
    }

    // 2026-06-30 P0-2 (primary-153653): fix-01 and fix-02
    // shared the same task_id in the JSONL because the
    // coordinator's fix-unit prompt template copied it. Closing
    // by id alone only closed the first row, leaving the rest
    // `open` forever and producing the P0-3 tasks.jsonl ↔
    // progress.md drift. The projector now prefers `task_key`
    // when present, then falls back to id. Regression test:
    // ensure two fix-units with the same task_id but different
    // task_keys, close each via work.done carrying the right
    // task_key, and verify both rows end up closed.
    #[test]
    fn p0_2_fix_units_share_task_id_close_by_key_independently() {
        use crate::state_projector::ProjectionContext;
        use crate::state_projector::StateProjectionConfig;

        let dir = tempdir().unwrap();
        let mut ctx = ProjectionContext::new(dir.path(), StateProjectionConfig::default(), false)
            .with_current_loop_id("loop-A");
        // Two fix-units, same task_id (the bug condition), different
        // task_keys (the discriminator the fix relies on).
        let fix01_ensure = serde_json::json!({
            "task_id": "task-shared",
            "task_key": "ce-executor:p:fix-01:u1",
            "plan_name": "p",
        });
        let fix02_ensure = serde_json::json!({
            "task_id": "task-shared",
            "task_key": "ce-executor:p:fix-02:u2",
            "plan_name": "p",
        });
        project_ensure_task(&mut ctx, &fix01_ensure, "task_key", None).unwrap();
        project_ensure_task(&mut ctx, &fix02_ensure, "task_key", None).unwrap();

        let fix01_close = serde_json::json!({
            "task_id": "task-shared",
            "task_key": "ce-executor:p:fix-01:u1",
            "step": "fix-01",
        });
        let fix02_close = serde_json::json!({
            "task_id": "task-shared",
            "task_key": "ce-executor:p:fix-02:u2",
            "step": "fix-02",
        });
        project_close_task(&mut ctx, &fix01_close, "task_id", Some("step")).unwrap();
        project_close_task(&mut ctx, &fix02_close, "task_id", Some("step")).unwrap();

        let tasks = ctx.task_snapshot().0;
        assert_eq!(tasks.len(), 2, "two fix-units must both be persisted");
        let closed_keys: Vec<&str> = tasks
            .iter()
            .filter(|t| t.status == crate::task::TaskStatus::Closed)
            .filter_map(|t| t.key.as_deref())
            .collect();
        assert_eq!(
            closed_keys.len(),
            2,
            "P0-2: both fix-units must close when task_key differs (was: only first closes)"
        );
    }
}
