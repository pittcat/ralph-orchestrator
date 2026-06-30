//! Progress ledger projection.
//!
//! Plan ref: U3 of
//! `docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md`.
//!
//! Progress writes go through [`write_progress`] which preserves
//! the legacy markdown shape (the `progress_task_gate` parser is
//! the source of truth for the dialect — we round-trip it instead
//! of inventing a new one).

// This module **owns** the deprecated `progress_cache` mirror:
// `push_completed` and `write_progress` mutate it as a
// write-through of the on-disk ledger. The mirror is kept in
// sync with the canonical `LedgerSnapshot` (via
// `sync_to_ledger_snapshot` and `project_ledger_snapshot`) so
// legacy callers continue to observe the same view, while the
// U2 path reads from the snapshot directly. Touching the
// field here is therefore intentional and unavoidable.
#![allow(deprecated)]

use std::path::Path;

use serde_json::Value;

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
    // 2026-06-30 P0-1 (primary-20260629-170451 diagnosis):
    // The pre-fix projector rewrote `progress.md` with
    // empty headings on every close event when `snap` had no
    // `current_step`; that produced the
    // `WARN: progress.md written with empty headings` log
    // line that the validator's prompt picks up as
    // "0 ready / 0 open / N closed". Falling back to a
    // `(none)` placeholder is friendlier than emitting a
    // heading-only document the `progress_task_gate`
    // consumer interprets as a fresh empty state.
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
    // 2026-06-30 P0-1: do NOT log `warn!` for empty headings
    // when the projector just rounded out a closing step
    // cleanly — `current_step = Some(...) && completed_steps
    // nonempty` is the steady-state shape that the previous
    // projector incorrectly labelled "empty". Only log
    // when BOTH headings are empty, which still surfaces the
    // diagnostic noise about a freshly-bootstrapped loop
    // that has not yet observed its first close event.
    if snap.empty_headings {
        debug!("progress.md written with no current_step and no completed_steps");
    }
    Ok(())
}

/// U2 (plan 2026-06-21-002): publicly callable variant of
/// [`write_progress`]. The projector module exposes this so
/// [`super::StateProjector::apply_from_ledger`] /
/// [`super::StateProjector::project_ledger_snapshot`] can
/// re-emit the progress file from a [`crate::state::LedgerSnapshot`]
/// without going through the event-driven path.
pub(crate) fn write_progress_external(path: &Path, snap: &ProgressSnapshot) -> Result<(), String> {
    write_progress(path, snap)
}

#[cfg(test)]
mod tests {
    //! 2026-06-30 P0-1 (primary-20260629-170451 diagnosis):
    //! Regression tests for `write_progress`. The pre-fix
    //! projector emitted `WARN: progress.md written with empty
    //! headings` whenever a `current_step` flip pushed the
    //! snapshot into the `(None, _)` shape mid-phase — the
    //! validator's prompt then saw "0 ready / 0 open / N
    //! closed" and aborted fix-02 with no `test.passed`.
    //! The post-fix writer falls back to `(none)` placeholders
    //! when `current_step` is `None`, and only logs the
    //! "fresh bootstrap" path at `debug!` level. These tests
    //! pin both behaviours so the validator keeps seeing a
    //! readable `progress.md` until the very first close
    //! event lands.

    use super::*;
    use crate::step_handoff::ProgressSnapshot;
    use tempfile::tempdir;

    #[test]
    fn write_progress_uses_none_placeholder_when_current_step_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("progress.md");
        let snap = ProgressSnapshot {
            current_step: None,
            completed_steps: vec!["step-01".to_string()],
            empty_headings: false,
        };
        write_progress(&path, &snap).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("## Current Step\n(none)\n"),
            "current_step = None must render as `(none)` placeholder, got:\n{body}"
        );
        assert!(
            body.contains("- step-01"),
            "completed_steps must keep their list-rendering, got:\n{body}"
        );
    }

    #[test]
    fn write_progress_logs_only_at_debug_for_completely_empty_snapshot() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("progress.md");
        let snap = ProgressSnapshot {
            current_step: None,
            completed_steps: Vec::new(),
            empty_headings: true,
        };
        // No assertion on log emission here; we only check
        // that the writer does not panic and produces the
        // placeholder document.
        write_progress(&path, &snap).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("(none)"),
            "empty snapshot must still produce a placeholder body, got:\n{body}"
        );
    }
}
