//! U12 (2026-06-27 mechanism foundation completion):
//! tests for `step_close_obligation`. The plan pins
//! four scenarios; we cover the happy path, the
//! skip-emit case, and the partial-progress obligation.

use super::*;

#[test]
fn u12_required_emit_returns_none_when_complete() {
    let progress = StepProgress { done: 8, total: 8 };
    let on_partial = std::collections::BTreeMap::new();
    let obligation = required_emit(progress, &on_partial);
    assert_eq!(obligation, Obligation::None);
}

#[test]
fn u12_required_emit_returns_none_when_zero_progress() {
    // No progress yet — no obligation.
    let progress = StepProgress { done: 0, total: 8 };
    let mut on_partial = std::collections::BTreeMap::new();
    on_partial.insert(
        "partial_units_done".to_string(),
        "plan.blocked(reason=\"4_of_8_partial\")".to_string(),
    );
    let obligation = required_emit(progress, &on_partial);
    assert_eq!(obligation, Obligation::None);
}

#[test]
fn u12_required_emit_returns_pending_when_partial() {
    let progress = StepProgress { done: 4, total: 8 };
    let mut on_partial = std::collections::BTreeMap::new();
    on_partial.insert("all_done".to_string(), "plan.complete".to_string());
    on_partial.insert(
        "any_failed".to_string(),
        "plan.blocked(reason=\"unit_failed\")".to_string(),
    );
    on_partial.insert(
        "partial_units_done".to_string(),
        "plan.blocked(reason=\"4_of_8_partial\")".to_string(),
    );
    let obligation = required_emit(progress, &on_partial);
    match obligation {
        Obligation::Pending(branches) => {
            assert_eq!(branches.len(), 3);
            let topics: Vec<&str> = branches.iter().map(|b| b.expected_topic.as_str()).collect();
            assert!(topics.contains(&"plan.complete"));
            assert!(topics.contains(&"plan.blocked"));
        }
        other => panic!("expected Pending, got {other:?}"),
    }
}

#[test]
fn u12_required_emit_returns_none_when_no_partial_branches() {
    // Partial progress but `on_partial` is empty:
    // no obligation can be enforced.
    let progress = StepProgress { done: 4, total: 8 };
    let on_partial = std::collections::BTreeMap::new();
    let obligation = required_emit(progress, &on_partial);
    assert_eq!(obligation, Obligation::None);
}

#[test]
fn u12_emit_satisfies_partial_obligation() {
    let progress = StepProgress { done: 4, total: 8 };
    let mut on_partial = std::collections::BTreeMap::new();
    on_partial.insert(
        "partial_units_done".to_string(),
        "plan.blocked(reason=\"4_of_8_partial\")".to_string(),
    );
    let obligation = required_emit(progress, &on_partial);
    // Happy path: a partial emit that names `partial`
    // in the reason matches the obligation.
    assert!(emit_satisfies_obligation(
        &obligation,
        "plan.blocked",
        r#"{"reason":"4_of_8_partial_continue_to_review"}"#,
    ));
}

#[test]
fn u12_emit_violates_partial_obligation_when_skipping_review() {
    let progress = StepProgress { done: 4, total: 8 };
    let mut on_partial = std::collections::BTreeMap::new();
    on_partial.insert(
        "partial_units_done".to_string(),
        "plan.blocked(reason=\"partial\")".to_string(),
    );
    let obligation = required_emit(progress, &on_partial);
    // Error path: emitting `review.complete` (which is
    // not in `allowed_emits` for `plan_end`) violates
    // the obligation. `emit_satisfies_obligation`
    // returns `false` so the dispatcher can reject.
    assert!(!emit_satisfies_obligation(
        &obligation,
        "review.complete",
        r#"{"verdict":"pass"}"#,
    ));
}

#[test]
fn u12_emit_satisfies_no_obligation_returns_true() {
    // An empty obligation accepts any emit.
    let obligation = Obligation::None;
    assert!(emit_satisfies_obligation(&obligation, "anything", "{}",));
}

#[test]
fn u12_parse_directive_extracts_topic_and_reason() {
    let directive = "plan.blocked(reason=\"4_of_8_partial\")";
    let obligation = parse_directive("partial_units_done", directive);
    assert_eq!(obligation.branch, "partial_units_done");
    assert_eq!(obligation.expected_topic, "plan.blocked");
    assert_eq!(obligation.expected_reason_pattern, "4_of_8_partial");
}

#[test]
fn u12_parse_directive_handles_plain_topic() {
    let directive = "plan.complete";
    let obligation = parse_directive("all_done", directive);
    assert_eq!(obligation.branch, "all_done");
    assert_eq!(obligation.expected_topic, "plan.complete");
    assert_eq!(obligation.expected_reason_pattern, "");
}
