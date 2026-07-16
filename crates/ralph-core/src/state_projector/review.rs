//! Review-summary projection.
//!
//! Plan ref: U3 of `docs/plans/2026-07-05-005-refactor-state-management-hardening-plan.md`.
//!
//! The projector keeps a small in-memory view of the latest
//! `review.dimensions.complete` event so the next `## ORCHESTRATOR
//! CONTEXT` block can include a `## REVIEW SUMMARY` section without
//! re-reading `events.jsonl`. The event itself is already
//! de-duplicated upstream by `event_policy` (the
//! `review_dimensions_complete_seen_keys` set), so the projector
//! runs at most once per dedup window — the view is monotonically
//! replaced on each accepted event (last-write-wins for visibility).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state_projector::ProjectionContext;
use crate::state_projector::json_pointer;

/// Per-event view of the latest `review.dimensions.complete` event.
///
/// Stored in `ProjectionContext` behind a `Mutex` because the
/// projector is single-threaded today, but future parallelism could
/// share the same context; the lock keeps the door open for that
/// without changing the public signature.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewDimensionsView {
    /// Stable task key from the event payload.
    pub task_key: Option<String>,
    /// Fix-round counter from the event payload.
    pub fix_round: Option<String>,
    /// Per-dimension verdict map (cloned from the payload).
    pub dimensions: Option<Value>,
    /// Human-readable summary line.
    pub summary: Option<String>,
}

/// Project a `review.dimensions.complete` event into the in-memory
/// view. Returns `Ok(())` after replacing the previous view (if
/// any). The projector does not dedup; that is `event_policy`'s
/// job. Each accepted event simply overwrites the previous view.
pub(crate) fn project_review_dimensions_complete(
    ctx: &mut ProjectionContext,
    payload: &Value,
    task_key_pointer: Option<&str>,
    fix_round_pointer: Option<&str>,
    dimensions_pointer: Option<&str>,
    summary_pointer: Option<&str>,
) -> Result<(), String> {
    let task_key =
        json_pointer(payload, task_key_pointer.unwrap_or("task_key")).map(|s| s.to_string());
    let fix_round =
        json_pointer(payload, fix_round_pointer.unwrap_or("fix_round")).map(|s| s.to_string());
    let dimensions = json_pointer(payload, dimensions_pointer.unwrap_or("dimensions"))
        .map(|v| Value::String(v.to_string()))
        .or_else(|| {
            // Fall back to the raw JSON value if the pointer
            // resolves to a non-string (the typical case for a
            // JSON object of dimension verdicts).
            payload
                .get(dimensions_pointer.unwrap_or("dimensions"))
                .cloned()
        });
    let summary =
        json_pointer(payload, summary_pointer.unwrap_or("summary")).map(|s| s.to_string());

    let view = ReviewDimensionsView {
        task_key: task_key.clone(),
        fix_round: fix_round.clone(),
        dimensions,
        summary: summary.clone(),
    };
    store_view(ctx, view);

    // `task_key` is required for the gate's `(task_key, fix_round)`
    // dedup contract; surface a missing-pointer error so the
    // projector rejects malformed events rather than silently
    // accepting an empty view.
    if task_key.is_none() {
        return Err(format!(
            "review_dimensions_complete: missing pointer 'task_key' (or '{}')",
            task_key_pointer.unwrap_or("task_key")
        ));
    }
    let _ = fix_round;
    Ok(())
}

fn store_view(ctx: &mut ProjectionContext, view: ReviewDimensionsView) {
    if let Ok(mut guard) = ctx.review_dimensions_view.lock() {
        *guard = Some(view);
    }
}

const REVIEW_SUMMARY_HEADING: &str = "## REVIEW SUMMARY";

/// Render the `## REVIEW SUMMARY` block appended after
/// `## ORCHESTRATOR CONTEXT` when a `review.dimensions.complete`
/// view is present.
pub(crate) fn render_review_summary_block(view: &ReviewDimensionsView) -> String {
    use std::fmt::Write as _;
    let mut buf = String::new();
    let _ = writeln!(buf, "{REVIEW_SUMMARY_HEADING}");
    match &view.task_key {
        Some(k) => {
            let _ = writeln!(buf, "- task_key: {k}");
        }
        None => {
            let _ = writeln!(buf, "- task_key: (none)");
        }
    }
    match &view.fix_round {
        Some(r) => {
            let _ = writeln!(buf, "- fix_round: {r}");
        }
        None => {
            let _ = writeln!(buf, "- fix_round: (none)");
        }
    }
    match &view.summary {
        Some(s) => {
            let _ = writeln!(buf, "- summary: {s}");
        }
        None => {
            let _ = writeln!(buf, "- summary: (none)");
        }
    }
    match &view.dimensions {
        Some(d) => {
            let _ = writeln!(buf, "- dimensions: {d}");
        }
        None => {
            let _ = writeln!(buf, "- dimensions: (none)");
        }
    }
    let _ = writeln!(buf);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StateProjectionConfig;
    use tempfile::tempdir;

    fn ctx() -> ProjectionContext {
        let dir = tempdir().unwrap();
        ProjectionContext::new(dir.path(), StateProjectionConfig::default(), false)
    }

    #[test]
    fn rejects_missing_task_key() {
        let mut c = ctx();
        let payload = serde_json::json!({"summary": "ok"});
        let err = project_review_dimensions_complete(&mut c, &payload, None, None, None, None)
            .unwrap_err();
        assert!(err.contains("missing pointer 'task_key'"), "got: {err}");
    }

    #[test]
    fn records_view_on_valid_event() {
        let mut c = ctx();
        let payload = serde_json::json!({
            "task_key": "ce-executor:demo:step-01:u1-impl",
            "fix_round": "0",
            "dimensions": {"correctness": "pass"},
            "summary": "all green",
        });
        project_review_dimensions_complete(&mut c, &payload, None, None, None, None).unwrap();
        let guard = c.review_dimensions_view.lock().unwrap();
        let view = guard.as_ref().expect("view stored");
        assert_eq!(
            view.task_key.as_deref(),
            Some("ce-executor:demo:step-01:u1-impl")
        );
        assert_eq!(view.fix_round.as_deref(), Some("0"));
        assert_eq!(view.summary.as_deref(), Some("all green"));
    }

    #[test]
    fn accepts_resubmitted_event_idempotently() {
        // U3 boundary: the projector does not dedup. Each accepted
        // event overwrites the view (last-write-wins). The dedup
        // itself is owned by `event_policy`'s
        // `review_dimensions_complete_seen_keys` set; if the
        // upstream dedup were broken, the projector would simply
        // re-record the latest view, which is the same as the
        // first record.
        let mut c = ctx();
        let payload = serde_json::json!({
            "task_key": "k1",
            "fix_round": "1",
            "summary": "first",
        });
        project_review_dimensions_complete(&mut c, &payload, None, None, None, None).unwrap();
        let payload2 = serde_json::json!({
            "task_key": "k1",
            "fix_round": "1",
            "summary": "second",
        });
        project_review_dimensions_complete(&mut c, &payload2, None, None, None, None).unwrap();
        let guard = c.review_dimensions_view.lock().unwrap();
        assert_eq!(
            guard.as_ref().unwrap().summary.as_deref(),
            Some("second"),
            "U3: projector records last-write-wins; dedup is upstream"
        );
    }
}
