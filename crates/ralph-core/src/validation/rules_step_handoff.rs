//! U4c: `StepHandoffRule` — wraps `step_handoff::progress_task_gate`.
//!
//! Pre-commit phase. The rule inspects `queue.advance` and
//! `plan.complete` events against the snapshot's
//! `progress` + `tasks` views (lifted from the legacy disk-read
//! `check_progress_task_alignment` into a pure-function shape
//! that takes `&ProgressSnapshot + &[Task]` directly).
//!
//! The rule preserves the legacy `reason` strings from
//! `ProgressTaskMismatch` so existing tests can match the
//! stable surface.

use crate::event_reader::Event;
use crate::preset::engine::protocol::ProtocolView;
use crate::step_handoff::progress_task_gate::{
    GATED_TOPICS, TaskProgressDecision, check_alignment_with_snapshot,
    refresh_progress_snapshot_if_stale,
};
use crate::task::Task;
use crate::task_store::resolve_task_for_gate;
use ralph_proto::HatId;

use super::context::ValidationContext;
use super::pipeline::{RulePhase, ValidationRule};
use super::result::{ReasonCode, ValidationResult, ValidationStage};

/// `StepHandoffRule` — pre-commit step-handoff gate.
pub struct StepHandoffRule;

impl ValidationRule for StepHandoffRule {
    fn name(&self) -> &'static str {
        ValidationStage::StepHandoff.as_str()
    }

    fn applies_to(&self) -> RulePhase {
        RulePhase::PreCommit
    }

    fn validate(
        &self,
        _protocol_view: &ProtocolView,
        ctx: &mut ValidationContext<'_>,
        event: &Event,
    ) -> ValidationResult {
        if !GATED_TOPICS.contains(&event.topic.as_str()) {
            return ValidationResult::accept_with(ValidationStage::StepHandoff);
        }
        let (step, task_id) = extract_step_task(event);
        // U1 of plan 2026-07-02-005: extract the payload
        // `completed_steps` array so the gate can use it for
        // `plan.complete` under `Current Step=None` branches.
        let payload_completed_steps = extract_completed_steps_array(event);

        // P1-#9 (002-adversarial-review): borrow progress +
        // tasks from the snapshot instead of cloning them on
        // every validation. `check_alignment_with_snapshot`
        // already takes `&ProgressSnapshot` and `&[Task]`, so
        // the legacy clone was pure waste. The borrow checker
        // is happy because `extract_step_task` consumes only
        // `&Event` (no overlap with the snapshot accessors).
        let snapshot = ctx.snapshot();

        // U6 of plan 2026-07-02-005: best-effort reconciliation
        // of the in-memory `LedgerSnapshot.progress` mirror with
        // the on-disk `progress.md` (175407 root cause). When
        // the runtime's mirror is stale (e.g. projector flushed
        // to disk but the snapshot kept the pre-flush view), the
        // gate would otherwise emit a `progress_missing_current_step`
        // mismatch on a perfectly valid event. We reconcile the
        // mirror in-place via `refresh_progress_snapshot_if_stale`
        // BEFORE running the gate check.
        //
        // The refresh needs the progress.md path. Use the
        // workspace_root-derived path: the runtime always places
        // the ledger at `<workspace>/.ralph/agent/progress.md`.
        // We re-derive it from the task path's parent (parent's
        // parent) so we don't need a separate `progress_path` knob
        // on the context — the path layout is a hard invariant.
        //
        // We use a local `progress` variable rather than mutating
        // the snapshot mirror in-place: the snapshot accessor
        // returns `&LedgerSnapshot`, and mutating it from the
        // rule would require a deeper API change. The next
        // projector flush will reconcile the mirror naturally.
        let progress = match ctx.tasks_path() {
            Some(tasks_path) => {
                let progress_path = tasks_path
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.join("agent").join("progress.md"));
                match progress_path {
                    Some(p) => {
                        let mut local = snapshot.progress.clone();
                        refresh_progress_snapshot_if_stale(&p, &mut local);
                        local
                    }
                    None => snapshot.progress.clone(),
                }
            }
            None => snapshot.progress.clone(),
        };

        // U5 of plan 2026-07-02-005: when the gate needs to
        // check a specific `task_id` AND the in-memory snapshot
        // is missing it, fall back to a best-effort disk reload
        // via `resolve_task_for_gate`. This eliminates the
        // 140149 / 175407 false-positive `task_not_found`
        // rejection when the runtime in-memory view is stale.
        // The fallback only runs when:
        //   1. The event carries a `task_id` field.
        //   2. The snapshot.tasks slice lacks that id.
        //   3. The caller wired a `tasks_path` via
        //      `ValidationContext::with_tasks_path`.
        // If any of those are missing, the legacy path (in-memory
        // only) runs unchanged.
        let resolved_task: Option<Task> = match (task_id.as_deref(), ctx.tasks_path()) {
            (Some(tid), Some(path)) => match resolve_task_for_gate(&snapshot.tasks, path, tid) {
                Ok(t) => t,
                Err(_) => None, // Treat reload failure as a clean miss;
                                // gate's downstream check still emits
                                // the right `task_not_found` finding.
            },
            _ => None,
        };

        let phase_id = ctx.workflow_phase_id();

        let decision = check_alignment_with_snapshot(
            &progress,
            &snapshot.tasks,
            event.topic.as_str(),
            step.as_deref(),
            task_id.as_deref(),
            payload_completed_steps.as_deref(),
            phase_id.as_deref(),
        );

        // U5: if the in-memory view missed the task but the disk
        // reload found it, we have a single fresh row but the
        // gate's signature takes `&[Task]`. Build a one-row
        // shadow slice **only** when the gate rejected on
        // `task_not_found` (the case U5 was designed for). This
        // keeps the change surgical: the legacy accept path is
        // untouched.
        let decision = if let TaskProgressDecision::Mismatch(m) = &decision
            && m.reason == "task_not_found"
            && let Some(ref found) = resolved_task
        {
            // Re-run with the disk-reloaded row appended.
            let mut extended: Vec<Task> = snapshot.tasks.clone();
            if !extended.iter().any(|t| t.id == found.id) {
                extended.push(found.clone());
            }
            check_alignment_with_snapshot(
                &progress,
                &extended,
                event.topic.as_str(),
                step.as_deref(),
                task_id.as_deref(),
                payload_completed_steps.as_deref(),
                phase_id.as_deref(),
            )
        } else {
            decision
        };

        match decision {
            TaskProgressDecision::Inert | TaskProgressDecision::Aligned => {
                ValidationResult::accept_with(ValidationStage::StepHandoff)
            }
            TaskProgressDecision::Mismatch(m) => {
                let code = format!("{}:{}", ReasonCode::STEP_HANDOFF_MISMATCH_PREFIX, m.reason);
                let hint = m.detail.clone();
                ValidationResult::reject(ValidationStage::StepHandoff, code, Some(hint), true)
            }
        }
    }
}

/// Extract `(step, task_id)` from the event payload. The
/// `progress_task_gate` legacy function accepts `Option<&str>`
/// values; this helper returns them as owned strings so the
/// borrow checker stays happy.
fn extract_step_task(event: &Event) -> (Option<String>, Option<String>) {
    let payload = event.payload.as_deref().unwrap_or("");
    let parsed: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let step = parsed
        .get("step")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let task_id = parsed
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (step, task_id)
}

/// U1 of plan 2026-07-02-005: extract `completed_steps` array from
/// the event payload. Returns `Some(vec)` when the payload has a
/// `completed_steps` JSON array, `None` otherwise (incl. non-array
/// shapes). Step may be either a string or an object with `id`
/// (coordinator rewrites `step` but not `completed_steps`), so the
/// array shape is left as-is.
fn extract_completed_steps_array(event: &Event) -> Option<Vec<String>> {
    let payload = event.payload.as_deref()?;
    let parsed: serde_json::Value = serde_json::from_str(payload).ok()?;
    let arr = parsed.get("completed_steps")?.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        // Accept only string entries; mixed-shape arrays are ignored.
        if let Some(s) = v.as_str() {
            out.push(s.to_string());
        }
    }
    Some(out)
}

// Keep the unused HatId import out of warnings.
#[allow(dead_code)]
fn _hat_id_marker(_: HatId) {}
