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
/// U3 of plan 2026-07-02-005: maintain `current_step` pointer
/// after marking. The agent has finished this step; the
/// `current_step` heading should NOT keep pointing at the
/// just-closed step (that produced a "shadow" duplicate). Set
/// it to `None` so the writer falls back to the `(none)`
/// placeholder; the next `queue.advance` will repopulate with
/// the next step.
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
    // U1 of plan 2026-07-05-005 (KTD-1): the `current_step` field is
    // no longer the source of truth — it is derived from
    // `completed_steps.last()` via `ProgressSnapshot::current_step()`.
    // We deliberately do NOT reset `current_step` to `None` here;
    // pushing the just-closed step into `completed_steps` is enough
    // to make the derived view advance. Touching the field is a
    // KTD-1 violation; the field is read-time-ignored.
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
        // U1 of plan 2026-07-05-005 (KTD-1): do NOT touch the
        // `current_step` field — the derived view (last completed
        // step) is the single source of truth for the markdown
        // heading. The next read will see the just-pushed step as
        // the current step.
    }
    // Close every still-open AND started task so the ledger
    // matches the plan-complete state. We do NOT re-open tasks
    // that were already closed/failed — `is_terminal` is the
    // source of truth. The single `save` makes the close atomic
    // on disk.
    //
    // 2026-06-30-001 P0-4 (primary-20260630-032648 diagnosis):
    // skipping never-started rows (started.is_none()) prevents
    // `tasks.jsonl` from accumulating orphan closed tasks with
    // `key=null, started_at=null, closed=<now>` rows that the
    // validator's `open_tasks` view treats as "executor did
    // work". TaskStore::close / close_by_key also gate the
    // same condition as a defence-in-depth; the projector
    // additionally filters here so the diagnostic
    // `closed_count` reflects only legitimate closes.
    let mut store = crate::task_store::TaskStore::load(&ctx.tasks_path)
        .map_err(|e| format!("tasks_load: {e}"))?;
    let mut closed = 0usize;
    for task in store.all().to_vec() {
        if task.status.is_terminal() {
            continue;
        }
        if task.started.is_none() {
            debug!(
                task_id = %task.id,
                task_key = ?task.key,
                "state projection: plan.complete skipped never-started task"
            );
            continue;
        }
        store.close(&task.id);
        closed += 1;
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
    // U1 of plan 2026-07-05-005 (KTD-1): render `## Current Step`
    // from the derived view (`completed_steps.last()`), NOT from the
    // deprecated `snap.current_step` field. The field is intentionally
    // left untouched by this writer so the on-disk markdown and the
    // in-memory `ProgressSnapshot` stay consistent without a second
    // source of truth to keep in sync.
    match snap.current_step() {
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
        // U1 of plan 2026-07-05-005 (KTD-1): the heading is derived
        // from `completed_steps.last()`, not from the `current_step`
        // field. With `completed_steps = [step-01]` the derived
        // value is "step-01", so the placeholder path is only hit
        // when the list is empty (next test).
        assert!(
            body.contains("## Current Step\nstep-01\n"),
            "derived current_step = step-01 must render that value, got:\n{body}"
        );
        assert!(
            body.contains("- step-01"),
            "completed_steps must keep their list-rendering, got:\n{body}"
        );
    }

    #[test]
    fn write_progress_derived_current_step_from_completed_list() {
        // U1 of plan 2026-07-05-005 (KTD-1): even when the legacy
        // `current_step` field is set to a different value, the
        // rendered heading MUST come from `completed_steps.last()`.
        let dir = tempdir().unwrap();
        let path = dir.path().join("progress.md");
        let snap = ProgressSnapshot {
            current_step: Some("STALE_FIELD_VALUE".to_string()),
            completed_steps: vec!["step-01".to_string(), "step-02".to_string()],
            empty_headings: false,
        };
        write_progress(&path, &snap).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("## Current Step\nstep-02\n"),
            "derived current_step must equal completed_steps.last(), got:\n{body}"
        );
        assert!(
            !body.contains("STALE_FIELD_VALUE"),
            "the deprecated `current_step` field must NOT influence the rendered markdown, got:\n{body}"
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

    // U3 of plan 2026-07-02-005: `project_mark_step_completed`
    // must clear `current_step` after pushing the closed step,
    // so the markdown `## Current Step` heading does not point
    // at the just-closed step. The agent's next queue.advance
    // will repopulate the pointer.

    fn build_ctx_for_test(dir: &std::path::Path) -> crate::state_projector::ProjectionContext {
        use crate::state_projector::ProjectionContext;
        let progress_path = dir.join("progress.md");
        let tasks_path = dir.join("tasks.jsonl");
        std::fs::write(&progress_path, "# Progress\n\n## Current Step\nstep-01\n").unwrap();
        let mut ctx =
            ProjectionContext::new(dir, crate::config::StateProjectionConfig::default(), false);
        ctx.progress_cache =
            ProgressSnapshot::parse(&std::fs::read_to_string(&progress_path).unwrap());
        ctx
    }

    #[test]
    fn u1_mark_step_completed_advances_derived_current_step() {
        // U1 of plan 2026-07-05-005 (KTD-1): `project_mark_step_completed`
        // no longer touches the deprecated `current_step` field. After
        // marking step-01 complete, the DERIVED view
        // (`completed_steps.last()`) returns step-01 — both the
        // rendered markdown and the in-memory snapshot agree.
        use crate::state_projector::progress::project_mark_step_completed;
        let dir = tempdir().unwrap();
        let mut ctx = build_ctx_for_test(dir.path());
        // Pre-condition: context picked up `step-01` from disk (legacy
        // field; derived view also points at the just-loaded step).
        assert_eq!(ctx.progress_cache.current_step.as_deref(), Some("step-01"));

        let payload = serde_json::json!({"step": "step-01"});
        project_mark_step_completed(&mut ctx, &payload, None).unwrap();

        assert!(
            ctx.progress_cache.is_step_completed("step-01"),
            "step-01 must be listed under Completed Steps"
        );
        // KTD-1: the derived view must reflect the just-marked step.
        assert_eq!(
            ctx.progress_cache.current_step(),
            Some("step-01"),
            "U1: derived current_step must equal completed_steps.last()"
        );
        let body = std::fs::read_to_string(&ctx.progress_path).unwrap();
        assert!(
            body.contains("## Current Step\nstep-01\n"),
            "rendered progress.md must show the just-marked step as current, got:\n{body}"
        );
    }

    #[test]
    fn u3_mark_step_completed_missing_step_pointer_returns_err() {
        use crate::state_projector::progress::project_mark_step_completed;
        let dir = tempdir().unwrap();
        let mut ctx = build_ctx_for_test(dir.path());
        // Empty payload → no `step` field → error.
        let payload = serde_json::json!({});
        let err = project_mark_step_completed(&mut ctx, &payload, None).unwrap_err();
        assert!(
            err.contains("missing pointer 'step'"),
            "error must mention missing pointer, got: {err}"
        );
    }

    #[test]
    fn u1_mark_step_completed_consecutive_advances_derived_step() {
        // U1 of plan 2026-07-05-005 (KTD-1): consecutive marks must
        // advance the derived `current_step()` pointer along with
        // the completed list. After step-01 then step-02 the derived
        // view is `step-02`.
        use crate::state_projector::progress::project_mark_step_completed;
        let dir = tempdir().unwrap();
        let mut ctx = build_ctx_for_test(dir.path());
        // First mark.
        project_mark_step_completed(&mut ctx, &serde_json::json!({"step": "step-01"}), None)
            .unwrap();
        // Second mark of a DIFFERENT step — derived current_step
        // advances to step-02.
        project_mark_step_completed(&mut ctx, &serde_json::json!({"step": "step-02"}), None)
            .unwrap();
        assert!(ctx.progress_cache.is_step_completed("step-01"));
        assert!(ctx.progress_cache.is_step_completed("step-02"));
        assert_eq!(
            ctx.progress_cache.current_step(),
            Some("step-02"),
            "U1: derived current_step must track completed_steps.last()"
        );
    }
}
