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
    if let Some(provided_id) = json_pointer(payload, "task_id") {
        task.id = provided_id.to_string();
    }
    if let Some(plan_name) = json_pointer(payload, "plan_name") {
        task = task.with_description(Some(format!("plan: {plan_name}")));
    }
    if let Some(loop_id) = ctx_loop_id(payload) {
        task = task.with_loop_id(Some(loop_id.to_string()));
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
    let closed = store.close(&task_id);
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
