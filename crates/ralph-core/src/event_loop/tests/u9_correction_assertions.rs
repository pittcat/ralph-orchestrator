//! U9 (plan 2026-06-21-002) — test migration pinning the
//! unified `correction_context` / `loop.resume` surface.
//!
//! These tests pin the contracts that the U9 plan requires
//! to migrate from `task.resume` assertions:
//!
//! 1. `CorrectionContext::render_block` carries the unified
//!    `reason_code` prefix that the runtime validation
//!    pipeline emits (`origin:`, `policy:`, `execution_contract:`,
//!    `hat_handoff:`, `step_handoff:`, `workflow_guard:`).
//! 2. `ResumeContext::render_block` carries the loop metadata
//!    that `loop.resume` replaces from the legacy `task.resume`
//!    payload.
//! 3. `PromptContext::render_all_blocks` emits the
//!    `## ORCHESTRATOR CORRECTION` and `## LOOP RESUME CONTEXT`
//!    headings the next prompt consumes.
//! 4. `derive_retry_key` matches `Rejection::compute_retry_key`
//!    so the ledger-side rejection records carry the same
//!    `retry_key` as the legacy bus-side path.
//! 5. `CorrectionContext::from_lint_resume_hint` produces a
//!    correction with `stage = "policy"` and reason_code
//!    `lint:*` — the same prefix the runtime's
//!    engine-gate-driven rejection path emits.
//!
//! The legacy `task.resume` injection path is preserved and
//! the original tests in
//! `crates/ralph-core/src/event_loop/tests/task_resume_ttl.rs`
//! continue to pass without any of these new assertions.

use super::*;
use crate::correction::{
    self, CorrectionContext, PromptContext, ResumeContext,
};
use crate::event_loop::rejection::Rejection;

/// Helper: build a deterministic origin rejection.
fn rejection_with_origin(
    hat: &str,
    topic: &str,
    violation: &str,
) -> Rejection {
    Rejection::from_origin(Some(hat.into()), topic.into(), violation)
}

/// U9 #1: `CorrectionContext::from_rejection` populates the
/// stable `reason_code` prefix that the unified validation
/// pipeline emits. This is the migration target of
/// `task.resume` payloads' `reason` field.
#[test]
fn u9_correction_context_reason_code_matches_validation_stages() {
    // The runtime validation pipeline produces rejections
    // with stage prefixes `origin:`, `policy:`,
    // `execution_contract:`, `hat_handoff:`. The `CorrectionContext`
    // carries these prefixes verbatim so `ralph emit
    // --policy-check` (CLI) and `validate_event` (runtime)
    // output the same `reason_code` string.
    let origin_rejection =
        Rejection::from_origin(Some("executor".into()), "work.done".into(), "test");
    let origin_ctx = CorrectionContext::from_rejection(&origin_rejection, 1);
    assert_eq!(origin_ctx.stage, "origin");
    assert!(
        origin_ctx.reason_code.starts_with("origin:"),
        "reason_code = {}",
        origin_ctx.reason_code
    );

    let exec_finding = crate::execution_contract::ExecutionContractFinding {
        kind: crate::execution_contract::ExecutionContractViolationKind::MissingPayloadField {
            field: "plan_path".into(),
        },
        message: "missing plan_path".into(),
        topic: "work.done".into(),
        source_hat: Some("executor".into()),
    };
    let exec_rejection = Rejection::from_execution_contract(
        &exec_finding,
        Some("executor".into()),
        Some("executor".into()),
    );
    let exec_ctx = CorrectionContext::from_rejection(&exec_rejection, 1);
    assert_eq!(exec_ctx.stage, "execution_contract");
    assert!(
        exec_ctx.reason_code.starts_with("execution_contract:"),
        "reason_code = {}",
        exec_ctx.reason_code
    );
}

/// U9 #2: `ResumeContext::render_block` carries the loop
/// metadata that the `loop.resume` control topic replaces
/// from the legacy `task.resume` payload. The legacy
/// `task.resume` event's payload did NOT carry a loop_id,
/// closed_tasks count, or progress summary — `loop.resume`
/// adds them so the resumed hat can reason about session
/// continuity.
#[test]
fn u9_resume_context_block_carries_loop_metadata() {
    let rc = ResumeContext::new(
        "loop-u9-001",
        7,
        "7/12 steps complete",
        42,
        "scout -> plan -> implement",
    );
    let block = rc.render_block();

    // All five fields land in the rendered block — these
    // are the new affordances `loop.resume` provides over
    // the legacy `task.resume` topic.
    assert!(block.contains("Loop ID: loop-u9-001"));
    assert!(block.contains("Closed tasks: 7"));
    assert!(block.contains("Last iteration: 42"));
    assert!(block.contains("Progress summary: 7/12 steps complete"));
    assert!(
        block.contains("Scratchpad headline: scout -> plan -> implement"),
        "block = {block}"
    );
}

/// U9 #3: `PromptContext::render_all_blocks` emits the
/// `## ORCHESTRATOR CORRECTION` heading when a correction
/// block is queued, and the `## LOOP RESUME CONTEXT` heading
/// when a resume block is queued. The runtime's
/// `prepend_correction_and_resume` consumes this output and
/// prepends it to the next prompt — these headings are what
/// the agent searches for in the prompt to know it has been
/// corrected / resumed.
#[test]
fn u9_prompt_context_renders_correction_and_resume_headings() {
    let mut pc = PromptContext::default();

    // Correction block (U7a path).
    let r = rejection_with_origin("executor", "work.done", "missing plan_path");
    pc.push_correction(CorrectionContext::from_rejection(&r, 1));

    // Resume block (U7b path).
    pc.resume_blocks
        .push(ResumeContext::new("loop-u9", 0, "", 5, ""));

    let out = pc.render_all_blocks();
    assert!(
        out.contains("## ORCHESTRATOR CORRECTION"),
        "render_all_blocks output missing ## ORCHESTRATOR CORRECTION heading: {out}"
    );
    assert!(
        out.contains("## LOOP RESUME CONTEXT"),
        "render_all_blocks output missing ## LOOP RESUME CONTEXT heading: {out}"
    );
    // The correction block carries the reason_code line —
    // agents grep the prompt for `### Reason:` to find the
    // structured rejection.
    assert!(out.contains("### Reason:"));
}

/// U9 #4: `correction::derive_retry_key` matches
/// `Rejection::compute_retry_key` shape so the ledger-side
/// rejection records carry the same retry counter the legacy
/// bus-side path used. Without this stability, the per-key
/// retry counter that drives the R11 escalation tripwire
/// would split between the bus and the ledger.
#[test]
fn u9_correction_retry_key_matches_rejection_shape() {
    let r = rejection_with_origin("executor", "work.done", "missing plan_path");
    let stage = r.stage;
    let derived = correction::derive_retry_key(
        stage,
        "executor",
        "work.done",
        &r.violation,
    );
    assert_eq!(derived, r.retry_key);
}

/// U9 #5: `CorrectionContext::from_lint_resume_hint` produces
/// a correction with `stage = "policy"` and `reason_code`
/// `lint:*`. This is the U4b-lint-failure migration target —
/// the runtime engine gate rejection that used to push a
/// `task.resume` now produces a `CorrectionContext` with the
/// same stage prefix the CLI emit side reports.
#[test]
fn u9_correction_from_lint_hint_carries_policy_stage() {
    use crate::preset::engine::LintResumeHint;

    let hint = LintResumeHint::from_reason("work.done", "missing required fields");
    let ctx = CorrectionContext::from_lint_resume_hint(&hint, 1);

    assert_eq!(ctx.stage, "policy");
    let reason_code = ctx.reason_code.clone();
    assert!(
        reason_code.starts_with("lint:"),
        "reason_code = {reason_code}"
    );
    assert_eq!(ctx.topic, "work.done");
}

/// U9 #6: `emit_correction_context` writes a `RejectionRecord`
/// to `.ralph/recovery.jsonl` with the unified
/// `{ts, hat, topic, reason_code, retry_count,
/// terminal_reason}` shape. This is the runtime surface that
/// `ralph diagnose --from-ledger` reads (T8.1 in
/// `crates/ralph-cli/tests/diagnose.rs`).
#[test]
fn u9_emit_correction_writes_recovery_jsonl_with_unified_schema() {
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let r = rejection_with_origin("executor", "work.done", "missing plan_path");
    let mut pc = PromptContext::default();
    let ctx = correction::emit_correction_context(&r, 1, Some(temp.path()), &mut pc);

    let path = crate::state::recovery_log_path(temp.path());
    assert!(path.exists(), "recovery.jsonl must be created");
    let content = std::fs::read_to_string(&path).unwrap();
    // The schema is the U7a RejectionRecord shape — every
    // CLI/runtime consumer parses these fields.
    assert!(content.contains("\"hat\":\"executor\""));
    assert!(content.contains("\"topic\":\"work.done\""));
    assert!(content.contains("\"reason_code\""));
    assert!(content.contains(&ctx.reason_code));
    assert!(content.contains("\"retry_count\":1"));
}

/// U9 #7: R11 tripwire — `CorrectionContext::needs_escalation`
/// flips at retry_count=3. The runtime's
/// `maybe_escalate_to_human_guidance` publishes a
/// `human.guidance` event at this threshold. This is the
/// deterministic counterpart to the legacy
/// `consecutive_rejections >= 3 → human.guidance` ladder.
#[test]
fn u9_correction_needs_escalation_flips_at_threshold() {
    let r = rejection_with_origin("executor", "work.done", "missing plan_path");
    assert!(!CorrectionContext::from_rejection(&r, 0).needs_escalation);
    assert!(!CorrectionContext::from_rejection(&r, 2).needs_escalation);
    assert!(CorrectionContext::from_rejection(&r, 3).needs_escalation);
    assert!(CorrectionContext::from_rejection(&r, 5).needs_escalation);
}

/// U9 #8: `maybe_escalate_to_human_guidance` publishes a
/// `human.guidance` event when `needs_escalation` is true,
/// and stays silent below the threshold.
#[test]
fn u9_escalation_helper_publishes_human_guidance_at_threshold() {
    let mut bus = ralph_proto::EventBus::new();
    let r = rejection_with_origin("executor", "work.done", "missing plan_path");

    let below = CorrectionContext::from_rejection(&r, 2);
    let fired_below = correction::maybe_escalate_to_human_guidance(&mut bus, &below);
    assert!(!fired_below);
    assert!(bus.take_human_pending().is_empty());

    let above = CorrectionContext::from_rejection(&r, 3);
    let fired_above = correction::maybe_escalate_to_human_guidance(&mut bus, &above);
    assert!(fired_above);
    let human = bus.take_human_pending();
    let guidance_count = human
        .iter()
        .filter(|e| e.topic.as_str() == ralph_proto::HUMAN_GUIDANCE)
        .count();
    assert_eq!(guidance_count, 1, "exactly one human.guidance event");
}

/// U9 #9: `RetryCounter` increments and trips the escalation
/// threshold. This is the runtime's per-`retry_key` counter
/// that drives the R11 tripwire in production.
#[test]
fn u9_retry_counter_increments_and_trips_at_threshold() {
    let mut counter = correction::RetryCounter::default();
    assert_eq!(counter.increment("a"), 1);
    assert_eq!(counter.increment("a"), 2);
    assert_eq!(counter.increment("a"), 3);
    assert!(counter.needs_escalation("a", 3));
    assert!(!counter.needs_escalation("b", 3));
    counter.reset("a");
    assert_eq!(counter.get("a"), 0);
}

/// U9 #10: `RecoveryAction::to_correction_context` produces a
/// correction with `stage = "drift"` — the drift engine's
/// hard-escalation hook. This is the U7b drift counterpart
/// to the policy correction pipeline.
#[test]
fn u9_drift_recovery_action_converts_to_correction_context() {
    use crate::diagnosis::{DiagnosisSeverity, RecoveryAction};
    let action = RecoveryAction {
        retry_key: "policy:executor:work.done:missing_field".into(),
        target_hat: ralph_proto::HatId::new("executor"),
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

/// U9 #11: Topic constants are stable across the
/// legacy/new control topic boundary. Both `task.resume`
/// (legacy) and `loop.resume` (U7b) are recognised as
/// orchestrator control topics; `loop.resume` is what the
/// new `--continue` path emits when the
/// `UNIFIED_DETERMINISTIC_CORRECTION=1` flag is on.
#[test]
fn u9_loop_resume_topic_constant_is_stable() {
    assert_eq!(ralph_proto::LOOP_RESUME, "loop.resume");
    assert_eq!(ralph_proto::TASK_RESUME, "task.resume");
    assert!(ralph_proto::is_orchestrator_control(ralph_proto::LOOP_RESUME));
    assert!(ralph_proto::is_orchestrator_control(ralph_proto::TASK_RESUME));
    // Sanity: business topics are NOT control topics.
    assert!(!ralph_proto::is_orchestrator_control("work.done"));
}

/// U9 #12: `correction::is_correction_enabled` is callable
/// without panicking and returns a `bool`. The helper
/// consults the `UNIFIED_DETERMINISTIC_CORRECTION` env var;
/// production code paths branch on the result to decide
/// between the legacy `task.resume` injection and the new
/// deterministic-correction path.
#[test]
fn u9_is_correction_enabled_returns_bool_without_panic() {
    let _ = correction::is_correction_enabled();
}