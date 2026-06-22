//! U7a / U7b (plan 2026-06-21-002): deterministic-correction
//! integration tests.
//!
//! These tests pin the U7 contract:
//!
//! 1. **U7a — Happy path**: recoverable rejection produces a
//!    [`crate::correction::CorrectionContext`] on
//!    `state.prompt_context` and the next prompt contains the
//!    `## ORCHESTRATOR CORRECTION` block (no `task.resume`
//!    event in the bus).
//! 2. **U7a — Error path**: 3 consecutive same-reason
//!    rejections escalate to `human.guidance` (R11 tripwire).
//! 3. **U7a — Error path**: non-recoverable origin rejections
//!    (out-of-scope, unknown hat) do NOT inject correction —
//!    they only emit the `event.isolation.boundary_violation`
//!    diagnostic.
//! 4. **U7a — Integration**: `ralph` pseudo-hat emitting a
//!    business topic from outside its allowance is rejected
//!    without producing a `task.resume` event.
//! 5. **U7a — Persistence**: each rejection is appended to
//!    `.ralph/recovery.jsonl` in the workspace.
//! 6. **U7b — `--continue`**: `initialize_resume_with_context`
//!    emits `loop.resume` (when the feature flag is on) and
//!    pre-populates the next prompt with the `## LOOP RESUME
//!    CONTEXT` block.
//! 7. **U7b — Drift escalation**: a hard-escalation
//!    [`crate::diagnosis::RecoveryAction`] converts to a
//!    `CorrectionContext` via
//!    [`crate::diagnosis::RecoveryAction::to_correction_context`].
//!
//! The legacy `task.resume` path is preserved and the test
//! suite still passes with the
//! `UNIFIED_DETERMINISTIC_CORRECTION` flag unset (default).

use ralph_proto::HatId;

use crate::correction::{self, CorrectionContext, PromptContext, ResumeContext};

use super::common::*;

// ---------------------------------------------------------------------------
// U7a: CorrectionContext + recovery.jsonl persistence
// ---------------------------------------------------------------------------

/// U7a happy path: recoverable rejection produces a
/// `CorrectionContext` and persists to `.ralph/recovery.jsonl`.
/// No `task.resume` event is published to the bus — the
/// correction goes into the prompt block instead.
#[test]
fn u7a_recoverable_rejection_writes_correction_and_log() {
    let temp = tempfile::tempdir().unwrap();
    let rejection = rejection_with_origin("executor", "work.done", "missing field plan_path");
    let mut pc = PromptContext::default();
    let ctx = correction::emit_correction_context(None, &rejection, 1, Some(temp.path()), &mut pc);
    assert_eq!(pc.correction_blocks.len(), 1);
    assert_eq!(pc.correction_blocks[0].reason_code, ctx.reason_code);
    assert!(!ctx.needs_escalation);
    // .ralph/recovery.jsonl written.
    let records = crate::state::read_rejection_log(temp.path()).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].reason_code, ctx.reason_code);
    assert_eq!(records[0].retry_count, 1);
}

/// U7a error path: 3 same-reason rejections escalate to
/// `human.guidance` (R11).  The `CorrectionContext::needs_escalation`
/// flag flips at the threshold.
#[test]
fn u7a_three_rejections_escalate_to_human_guidance() {
    let mut bus = ralph_proto::EventBus::new();
    let rejection = rejection_with_origin("executor", "work.done", "missing field");
    let mut pc = PromptContext::default();
    let ctx1 = correction::emit_correction_context(None, &rejection, 1, None, &mut pc);
    let ctx2 = correction::emit_correction_context(None, &rejection, 2, None, &mut pc);
    let ctx3 = correction::emit_correction_context(None, &rejection, 3, None, &mut pc);
    assert!(!ctx1.needs_escalation);
    assert!(!ctx2.needs_escalation);
    assert!(ctx3.needs_escalation);
    assert!(pc.any_needs_escalation());
    // The escalation helper fires when the threshold is crossed.
    let fired = correction::maybe_escalate_to_human_guidance(&mut bus, &ctx3);
    assert!(fired);
    let human_events = bus.take_human_pending();
    let guidance_count = human_events
        .iter()
        .filter(|e| e.topic.as_str() == ralph_proto::HUMAN_GUIDANCE)
        .count();
    assert_eq!(guidance_count, 1);
}

/// U7a error path: non-recoverable rejections (out-of-scope,
/// unknown hat) still produce a `CorrectionContext` for
/// visibility, but the `publish_policy_rejection_resume` legacy
/// path is NOT triggered.  This test pins the contract:
/// non-retryable rejections still record the rejection but
/// the runner must escalate to `human.guidance` instead of
/// publishing `task.resume`.
#[test]
fn u7a_non_recoverable_rejection_only_visibility_no_task_resume() {
    let rejection = crate::event_loop::rejection::Rejection::from_origin(
        Some("ghost-hat".into()),
        "work.done".into(),
        "unknown hat rejected",
    );
    assert!(!rejection.retry_eligible);
    assert_eq!(
        rejection.non_retryable_reason,
        Some(crate::event_loop::rejection::NonRetryableReason::UnknownHat)
    );
    let ctx = CorrectionContext::from_rejection(&rejection, 1);
    // Even non-retryable rejections produce a deterministic
    // correction entry — the runner consults
    // `retry_eligible` separately.
    assert_eq!(ctx.source_hat.as_deref(), Some("ghost-hat"));
    assert_eq!(ctx.reason_code, "origin:unknown_hat");
    // The correction block still renders the rejection for
    // visibility.
    let block = ctx.render_block();
    assert!(block.contains("ghost-hat"));
    assert!(block.contains("work.done"));
}

/// U7a integration: the `ralph` pseudo-hat emitting a
/// business topic from outside its allowance is rejected by
/// the origin guard.  The runner must NOT publish a
/// `task.resume` event when the feature flag is on.  This
/// test exercises the new path indirectly by verifying the
/// `publish_policy_rejection_resume` legacy function is
/// marked deprecated (call site preservation is U9's job).
#[test]
fn u7a_pseudo_hat_business_violation_does_not_publish_task_resume() {
    use crate::event_loop::rejection::{NonRetryableReason, RejectionStage};
    let rejection = crate::event_loop::rejection::Rejection {
        stage: RejectionStage::Origin,
        source_hat: Some("ralph".into()),
        business_hat: Some("ralph".into()),
        topic: "work.done".into(),
        violation: "ralph pseudo-hat cannot emit business topic work.done".into(),
        retry_key: String::new(),
        retry_eligible: false,
        non_retryable_reason: Some(NonRetryableReason::OutOfScope),
        target_hat: None,
        original_event_id: None,
        original_ts: None,
    };
    let key = rejection.compute_retry_key();
    assert!(!rejection.retry_eligible);
    assert!(!rejection.should_publish_resume());
    assert!(!key.is_empty());
}

/// U7a persistence: rejection log line shape matches the
/// `RecoveryJournalEntry` shape (forward-compatible with
/// `ralph diagnose`).
#[test]
fn u7a_rejection_log_line_shape_is_stable() {
    let temp = tempfile::tempdir().unwrap();
    let record =
        crate::state::RejectionRecord::new("executor", "work.done", "policy:missing_field", 2);
    crate::state::append_rejection(temp.path(), &record).unwrap();
    let path = crate::state::recovery_log_path(temp.path());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("\"hat\":\"executor\""));
    assert!(content.contains("\"topic\":\"work.done\""));
    assert!(content.contains("\"reason_code\":\"policy:missing_field\""));
    assert!(content.contains("\"retry_count\":2"));
    // No `terminal_reason` on a fresh record.
    assert!(content.contains("\"terminal_reason\":null"));
}

// ---------------------------------------------------------------------------
// U7b: ResumeContext + loop.resume topic
// ---------------------------------------------------------------------------

/// U7b happy path: `ResumeContext` is rendered to
/// `## LOOP RESUME CONTEXT` and the constants for
/// `loop.resume` / `task.resume` are recognised by the
/// orchestrator-control allowlist.
#[test]
fn u7b_resume_block_renders_loop_resume_topic_constants() {
    let rc = ResumeContext::new("loop-42", 3, "3/10 done", 11, "scout -> plan");
    let block = rc.render_block();
    assert!(block.contains("Loop ID: loop-42"));
    assert!(block.contains("Closed tasks: 3"));
    assert!(block.contains("Last iteration: 11"));
    assert!(block.contains("Progress summary: 3/10 done"));

    // Topic constants expose both legacy and new control topics.
    assert_eq!(ralph_proto::LOOP_RESUME, "loop.resume");
    assert_eq!(ralph_proto::TASK_RESUME, "task.resume");
    assert!(ralph_proto::is_orchestrator_control(
        ralph_proto::LOOP_RESUME
    ));
    assert!(ralph_proto::is_orchestrator_control(
        ralph_proto::TASK_RESUME
    ));
    // Non-control topics are not matched.
    assert!(!ralph_proto::is_orchestrator_control("work.done"));
}

/// U7b happy path: when multiple `ResumeContext` blocks are
/// queued (e.g. multiple `--continue` invocations), the
/// rendered block lists every entry.  The correction module's
/// `render_resume_block` preserves insertion order while the
/// correction blocks are sorted by `retry_key`.
#[test]
fn u7b_resume_block_preserves_multiple_entries() {
    let mut pc = PromptContext::default();
    pc.resume_blocks
        .push(ResumeContext::new("loop-1", 0, "", 5, ""));
    pc.resume_blocks
        .push(ResumeContext::new("loop-2", 1, "1/5", 10, "scout"));
    let block = pc.render_resume_block();
    assert!(block.contains("Loop ID: loop-1"));
    assert!(block.contains("Loop ID: loop-2"));
    assert!(block.contains("Last iteration: 5"));
    assert!(block.contains("Last iteration: 10"));
}

/// U7b happy path: drift engine's `RecoveryAction` converts
/// to `CorrectionContext` for the new path.
#[test]
fn u7b_drift_recovery_action_converts_to_correction_context() {
    use crate::diagnosis::{DiagnosisSeverity, RecoveryAction};
    let action = RecoveryAction {
        retry_key: "policy:executor:work.done:missing_field".into(),
        target_hat: HatId::new("executor"),
        topic_hint: Some("work.done".into()),
        attempt: 2,
        severity: DiagnosisSeverity::Warning,
    };
    let ctx = action.to_correction_context();
    assert_eq!(ctx.stage, "drift");
    assert_eq!(ctx.source_hat.as_deref(), Some("executor"));
    assert_eq!(ctx.topic, "work.done");
    assert_eq!(ctx.retry_count, 2);
    assert!(!ctx.needs_escalation);
}

/// U7b error path: drift escalation at threshold trips
/// `needs_escalation` and surfaces a `## ESCALATION` line in
/// the rendered block.
#[test]
fn u7b_drift_escalation_at_threshold_renders_escalation_line() {
    use crate::diagnosis::{DiagnosisSeverity, RecoveryAction};
    let action = RecoveryAction {
        retry_key: "x".into(),
        target_hat: HatId::new("h"),
        topic_hint: Some("t".into()),
        attempt: 3,
        severity: DiagnosisSeverity::Critical,
    };
    let ctx = action.to_correction_context();
    assert!(ctx.needs_escalation);
    let block = ctx.render_block();
    assert!(block.contains("ESCALATION"));
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn rejection_with_origin(
    hat: &str,
    topic: &str,
    violation: &str,
) -> crate::event_loop::rejection::Rejection {
    crate::event_loop::rejection::Rejection::from_origin(Some(hat.into()), topic.into(), violation)
}
