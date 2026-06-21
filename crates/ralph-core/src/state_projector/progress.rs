//! Progress ledger projection.
//!
//! Plan ref: U3 of
//! `docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md`.
//!
//! Progress writes go through [`write_progress`] which preserves
//! the legacy markdown shape (the `progress_task_gate` parser is
//! the source of truth for the dialect — we round-trip it instead
//! of inventing a new one).

use std::path::Path;

use serde_json::Value;
use tracing::warn;

use crate::state_projector::ProjectionContext;
use crate::state_projector::json_pointer;
use crate::step_handoff::ProgressSnapshot;
use tracing::debug;

const PROGRESS_HEADER: &str = "# Progress\n\n";
const CURRENT_STEP_HEADING: &str = "## Current Step\n";
const COMPLETED_STEPS_HEADING: &str = "## Completed Steps\n";

/// Advance the progress file. Called from `queue.advance`.
pub(crate) fn project_advance_step(
    ctx: &mut ProjectionContext,
    payload: &Value,
    current_step_pointer: Option<&str>,
    completed_step_pointer: Option<&str>,
) -> Result<(), String> {
    let current_ptr = current_step_pointer.unwrap_or("step");
    let new_current = json_pointer(payload, current_ptr)
        .ok_or_else(|| format!("missing pointer '{current_ptr}'"))?
        .to_string();

    let completed_ptr = completed_step_pointer.unwrap_or("completed_step");
    if let Some(done) = json_pointer(payload, completed_ptr) {
        push_completed(&mut ctx.progress_cache, done);
    } else if json_pointer(payload, "step").is_some() {
        // Fallback: if the event only carries `step` we still try
        // to record the *previous* current step as completed.
        // Clone the current step up front so the immutable borrow
        // ends before we take `&mut ctx.progress_cache` for the
        // push.
        if let Some(prev) = ctx.progress_cache.current_step.clone() {
            push_completed(&mut ctx.progress_cache, &prev);
        }
    }
    ctx.progress_cache.current_step = Some(new_current);
    write_progress(&ctx.progress_path, &ctx.progress_cache)
}

/// Append a completed step to the progress file. Called from
/// `work.done` (via the task sub-module).
pub(crate) fn project_close_step(ctx: &mut ProjectionContext, step: &str) -> Result<(), String> {
    push_completed(&mut ctx.progress_cache, step);
    write_progress(&ctx.progress_path, &ctx.progress_cache)
}

/// Append a step to the `## Completed Steps` heading. Idempotent:
/// re-pinning the same step is a no-op. Distinct from
/// `project_close_step` only in the call site — both write the
/// same heading. Kept as a separate function so the
/// `state_projection.actions.work.done` chain can express the
/// `close_task` → `mark_step_completed` order explicitly (R4,
/// KTD-3).
///
/// Plan ref: U3a of
/// `docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md`.
pub(crate) fn project_mark_step_completed(
    ctx: &mut ProjectionContext,
    payload: &serde_json::Value,
    step_pointer: Option<&str>,
) -> Result<(), String> {
    let pointer = step_pointer.unwrap_or("step");
    let step = crate::state_projector::json_pointer(payload, pointer)
        .ok_or_else(|| format!("mark_step_completed: missing pointer '{pointer}'"))?
        .to_string();
    push_completed(&mut ctx.progress_cache, &step);
    write_progress(&ctx.progress_path, &ctx.progress_cache)
}

/// Finalize the plan. Closes any open tasks in `tasks.jsonl`
/// and updates the progress banner. P1 fix (review 2026-06-17-003):
/// the docstring previously promised "closes all open tasks" but
/// only the progress file was touched. Without closing the
/// tasks, `tasks.jsonl` would carry stale open rows and the U4
/// `progress_task_gate` would reject the next `queue.advance`
/// for any new step that the agent tries to run. The closing
/// loop is best-effort: a save error fails the whole
/// projection (fail-closed) so the diagnostic surfaces a
/// `plan.blocked` rather than a silent task leak.
pub(crate) fn project_plan_complete(
    ctx: &mut ProjectionContext,
    payload: &Value,
    final_step_pointer: Option<&str>,
) -> Result<(), String> {
    let final_ptr = final_step_pointer.unwrap_or("step");
    if let Some(step) = json_pointer(payload, final_ptr) {
        push_completed(&mut ctx.progress_cache, step);
        ctx.progress_cache.current_step = Some(step.to_string());
    }
    // Close every still-open task so the ledger matches the
    // plan-complete state. We do NOT re-open tasks that were
    // already closed/failed — `is_terminal` is the source of
    // truth. The single `save` makes the close atomic on disk.
    let mut store = crate::task_store::TaskStore::load(&ctx.tasks_path)
        .map_err(|e| format!("tasks_load: {e}"))?;
    let mut closed = 0usize;
    for task in store.all().to_vec() {
        if !task.status.is_terminal() {
            store.close(&task.id);
            closed += 1;
        }
    }
    if closed > 0 {
        store.save().map_err(|e| format!("tasks_save: {e}"))?;
        ctx.tasks_cache = store.all().to_vec();
        debug!(
            closed_count = closed,
            "state projection: plan.complete closed remaining open tasks"
        );
    }
    write_progress(&ctx.progress_path, &ctx.progress_cache)
}

fn push_completed(snap: &mut ProgressSnapshot, step: &str) {
    let trimmed = step.trim();
    if trimmed.is_empty() {
        return;
    }
    if !snap.completed_steps.iter().any(|s| s == trimmed) {
        snap.completed_steps.push(trimmed.to_string());
    }
}

fn write_progress(path: &Path, snap: &ProgressSnapshot) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("progress_mkdir: {e}"))?;
    }
    let mut buf = String::from(PROGRESS_HEADER);
    match &snap.current_step {
        Some(step) => {
            buf.push_str(CURRENT_STEP_HEADING);
            buf.push_str(step);
            buf.push_str("\n\n");
        }
        None => {
            buf.push_str(CURRENT_STEP_HEADING);
            buf.push_str("(none)\n\n");
        }
    }
    buf.push_str(COMPLETED_STEPS_HEADING);
    if snap.completed_steps.is_empty() {
        buf.push_str("(none)\n");
    } else {
        for step in &snap.completed_steps {
            buf.push_str("- ");
            buf.push_str(step);
            buf.push('\n');
        }
    }
    // Atomic write: write to temp + rename. Avoids leaving a
    // half-written file when the loop is interrupted.
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, &buf).map_err(|e| format!("progress_write: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("progress_rename: {e}"))?;
    if snap.empty_headings {
        warn!("progress.md written with empty headings");
    }
    Ok(())
}

/// U2 (plan 2026-06-21-002): publicly callable variant of
/// [`write_progress`]. The projector module exposes this so
/// [`super::StateProjector::apply_from_ledger`] /
/// [`super::StateProjector::project_ledger_snapshot`] can
/// re-emit the progress file from a [`crate::state::LedgerSnapshot`]
/// without going through the event-driven path.
pub(crate) fn write_progress_external(
    path: &Path,
    snap: &ProgressSnapshot,
) -> Result<(), String> {
    write_progress(path, snap)
}
