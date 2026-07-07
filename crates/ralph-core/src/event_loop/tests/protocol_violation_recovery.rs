//! 2026-07-07-002 plan Unit 8: protocol-violation bounded-retry + fail-close tests.
//!
//! These tests pin the U8 contract that emerged from the
//! post-`fix(U10)` regression:
//!
//! * `TaskNotTerminal` on the same `(hat, topic, task_key, step)`
//!   signature must accumulate counts in the protocol budget,
//!   NOT in the legacy `workflow_guard:` scope-violation budget.
//! * Once the count strictly exceeds `U2_REJECTION_RETRY_LIMIT`
//!   the runtime must fall through to `plan.blocked` rather than
//!   emit another `task.resume` retry event.
//! * The `U2_REJECTION_RETRY_LIMIT` boundary (3) allows 3 retries
//!   then fail-closes on the 4th.

use super::*;

fn protocol_key(hat: &str, topic: &str, task_key: &str, step: &str, code: &str) -> String {
    format!("protocol:{hat}:{topic}:{task_key}:{step}:{code}")
}

/// U8 (DEV-006 carve-out): the protocol-violation retry budget
/// must accumulate across repeated `TaskNotTerminal` rejections
/// of the same `(hat, topic, task_key, step)` signature, so the
/// bounded-budget fail-close actually fires.
///
/// Regression guard for the 2026-07-07-002 plan: prior to the
/// `clear_rejection_keys_for_hat` carve-out, the budget count
/// was reset to 1 on every legal `work.ready` emit and the
/// fail-close `plan.blocked` was never produced.
#[test]
fn test_protocol_violation_budget_accumulates_across_retries() {
    let mut state = LoopState::default();
    let hat = "executor";
    let topic = "work.done";
    let task_key = "k1";
    let step = "step-01";
    let code = "task_not_terminal";

    let mut counts = Vec::new();
    for _ in 0..(U2_REJECTION_RETRY_LIMIT + 1) {
        let (count, exhausted) =
            state.record_protocol_violation_signature(hat, topic, task_key, step, code);
        counts.push(count);
        assert!(!exhausted || count > U2_REJECTION_RETRY_LIMIT);
    }
    let key = protocol_key(hat, topic, task_key, step, code);
    // After 1 + LIMIT consecutive signatures the count is strictly
    // greater than the limit; the (LIMIT+1)-th call returns
    // exhausted=true so the runtime falls through to fail-close.
    assert_eq!(counts.last().copied(), Some(U2_REJECTION_RETRY_LIMIT + 1));
    assert!(state.rejection_key_is_exhausted(&key));
}

/// U8 sanity: two parallel retry signatures must not share budget.
/// `executor + work.done + step-01` and `executor + work.done +
/// step-02` each get their own counter — a fix on step-02 does
/// not reset the step-01 budget.
#[test]
fn test_protocol_violation_signature_isolates_by_step() {
    let mut state = LoopState::default();
    let (_c1, ex1) =
        state.record_protocol_violation_signature("executor", "work.done", "k", "step-01", "task_not_terminal");
    let (_c2, ex2) =
        state.record_protocol_violation_signature("executor", "work.done", "k", "step-02", "task_not_terminal");
    assert!(!ex1 && !ex2);
    // Exhaust step-01 by adding LIMIT more signatures after the
    // initial one above (4 total on step-01), then verify the
    // (LIMIT+2)-th signature trips the fail-close flag.
    for _ in 0..U2_REJECTION_RETRY_LIMIT {
        state.record_protocol_violation_signature("executor", "work.done", "k", "step-01", "task_not_terminal");
    }
    let (c_step1, ex_step1) =
        state.record_protocol_violation_signature("executor", "work.done", "k", "step-01", "task_not_terminal");
    let (_, ex_step2) =
        state.record_protocol_violation_signature("executor", "work.done", "k", "step-02", "task_not_terminal");
    assert_eq!(c_step1, U2_REJECTION_RETRY_LIMIT + 2);
    assert!(ex_step1);
    // step-02 budget is independent and still well below the limit
    // (LIMIT+2 fails, but step-02 only has 2 signatures so far).
    assert!(!ex_step2);
}

/// U8 (DEV-006 carve-out): the protocol budget is **not**
/// cleared by `clear_rejection_keys_for_hat`. The legacy
/// 2026-06-14-004 U2 clear was scoped to scope-violation counts
/// (workflow_guard prefix); clearing protocol keys would
/// silently reset the bounded retry and prevent fail-close.
#[test]
fn test_clear_rejection_keys_keeps_protocol_budget() {
    let mut state = LoopState::default();
    state.record_protocol_violation_signature(
        "executor",
        "work.done",
        "k",
        "step-01",
        "task_not_terminal",
    );
    state.record_protocol_violation_signature(
        "executor",
        "work.done",
        "k",
        "step-01",
        "task_not_terminal",
    );
    let key = protocol_key("executor", "work.done", "k", "step-01", "task_not_terminal");
    assert_eq!(state.rejection_retry_count(&key), 2);

    state.clear_rejection_keys_for_hat("executor");

    // protocol prefix is preserved; the budget continues to track.
    assert_eq!(state.rejection_retry_count(&key), 2);
}

/// U8 boundary: `U2_REJECTION_RETRY_LIMIT` is intentionally 3;
/// plan/doc references use 3 then fail-close on the 4th. This
/// regression guard pins the budget constant so future drift
/// cannot silently break the BDD scenario's expectations.
#[test]
fn test_u2_rejection_retry_limit_is_three() {
    assert_eq!(U2_REJECTION_RETRY_LIMIT, 3);
}
