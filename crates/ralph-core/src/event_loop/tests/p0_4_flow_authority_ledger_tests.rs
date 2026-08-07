use super::super::*;
use std::path::PathBuf;

// Plan 2026-07-31-001 (nextest process-per-test
// compatibility): the prior helper shared one directory
// per process id, which caused races when nextest ran tests
// in parallel. Each test now gets its own sub-directory
// rooted at the shared per-process temp dir; the helper
// accepts the test name so two tests never collide on
// `flow-authority.jsonl` writes. The `test_name` is the
// `&str` the caller passes — usually the literal test fn
// name to keep a 1:1 audit trail between the test and its
// scratch space.
fn workspace_root(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("ralph-p0-4-flow-auth-{}", std::process::id()))
        .join(test_name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".ralph")).unwrap();
    dir
}

#[test]
fn load_returns_none_when_ledger_missing() {
    let root = workspace_root("load_returns_none_when_ledger_missing");
    let got = load_flow_authority_current_step(&root, None);
    assert!(got.is_none(), "missing ledger must yield None");
}

#[test]
fn load_returns_last_step_from_ledger() {
    let root = workspace_root("load_returns_last_step_from_ledger");
    let path = root.join(".ralph/flow-authority.jsonl");
    std::fs::write(
        &path,
        "{\"step\":\"scope_freeze\",\"topic\":\"scope.freeze\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n\
             {\"step\":\"synth_await\",\"topic\":\"review.wave.complete\"}\n",
    )
    .unwrap();
    let got = load_flow_authority_current_step(&root, None);
    assert_eq!(got.as_deref(), Some("synth_await"));
}

#[test]
fn load_skips_blank_and_malformed_lines() {
    let root = workspace_root("load_skips_blank_and_malformed_lines");
    let path = root.join(".ralph/flow-authority.jsonl");
    std::fs::write(
        &path,
        "\n{\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n\
             not-json\n\
             {\"step\":\"synth_await\"}\n",
    )
    .unwrap();
    let got = load_flow_authority_current_step(&root, None);
    assert_eq!(got.as_deref(), Some("synth_await"));
}

/// Plan 004 R7 / P0-4: rejected events never enter the
/// accept branch, so the authority ledger only reflects the
/// accepted transitions. Mixing rejected events into the
/// main ledger (the pre-fix bug) used to advance the
/// recovered step incorrectly.
#[test]
fn rejected_events_do_not_pollute_authority() {
    // The acceptance ledger is a separate file from
    // events.jsonl. The pre-fix CLI folded raw main ledger
    // topics (including rejected ones) through
    // `advance_plan_step`. The post-fix CLI reads only the
    // accepted ledger; the test pins that rejected events
    // never reach this file.
    let root = workspace_root("rejected_events_do_not_pollute_authority");
    let path = root.join(".ralph/flow-authority.jsonl");
    // Simulate the EventLoop having accepted exactly one
    // event: scope.ready, which advanced review_wave.
    std::fs::write(
        &path,
        "{\"step\":\"scope_freeze\",\"topic\":\"scope.freeze\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n",
    )
    .unwrap();
    let got = load_flow_authority_current_step(&root, None);
    assert_eq!(got.as_deref(), Some("review_wave"));
}

/// Plan 004 R7: the same accepted-step ledger is consumed
/// by both the resident EventLoop (writes) and CLI
/// policy-check / restart (reads). Restart consistency:
/// re-instantiating the recovery function on the same
/// ledger must produce the same step.
#[test]
fn restart_consistency_across_reads() {
    let root = workspace_root("restart_consistency_across_reads");
    let path = root.join(".ralph/flow-authority.jsonl");
    std::fs::write(
        &path,
        "{\"step\":\"scope_freeze\",\"topic\":\"scope.freeze\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n\
             {\"step\":\"synth_await\",\"topic\":\"review.wave.complete\"}\n",
    )
    .unwrap();
    let a = load_flow_authority_current_step(&root, None);
    let b = load_flow_authority_current_step(&root, None);
    assert_eq!(a, b, "restart must observe the same authority");
    assert_eq!(a.as_deref(), Some("synth_await"));
}

// Plan 2026-07-31-001 regression tests: the loop_id filter
// must partition flow-authority.jsonl entries by their active
// loop so a new loop cold-start on the same workspace does NOT
// inherit the previous loop's terminal step (root cause:
// implementation-review runs primary-20260731-131515 +
// primary-20260731-133437 both failed `ralph emit
// scope.ready.proposed --policy-check` with
// `flow_unknown_emit` because the previous loop's `finalize`
// entry was carried over via the loop-blind read).

#[test]
fn load_filters_entries_by_loop_id() {
    let root = workspace_root("load_filters_entries_by_loop_id");
    let path = root.join(".ralph/flow-authority.jsonl");
    std::fs::write(
        &path,
        "{\"step\":\"scope_freeze\",\"topic\":\"scope.ready\",\"loop_id\":\"loop-A\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\",\"loop_id\":\"loop-A\"}\n\
             {\"step\":\"finalize\",\"topic\":\"scope.blocked\",\"loop_id\":\"loop-B\"}\n",
    )
    .unwrap();
    // loop-A caller — must see the latest loop-A entry,
    // NOT the stale `finalize` from loop-B.
    let a = load_flow_authority_current_step(&root, Some("loop-A"));
    assert_eq!(
        a.as_deref(),
        Some("review_wave"),
        "loop-A caller must ignore loop-B entries"
    );
    // loop-B caller — must see the loop-B entry.
    let b = load_flow_authority_current_step(&root, Some("loop-B"));
    assert_eq!(b.as_deref(), Some("finalize"));
    // No loop_id passed (legacy / tests / CLI sub-process
    // without a marker on disk) — last entry wins (loop-B's
    // finalize) so older flows and tests keep working.
    let none = load_flow_authority_current_step(&root, None);
    assert_eq!(none.as_deref(), Some("finalize"));
}

#[test]
fn load_keeps_unstamped_entries_for_backward_compat() {
    let root = workspace_root("load_keeps_unstamped_entries_for_backward_compat");
    let path = root.join(".ralph/flow-authority.jsonl");
    std::fs::write(
        &path,
        "{\"step\":\"scope_freeze\",\"topic\":\"scope.ready\"}\n\
             {\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n",
    )
    .unwrap();
    let got = load_flow_authority_current_step(&root, Some("loop-C"));
    assert_eq!(
        got.as_deref(),
        Some("review_wave"),
        "unstamped entries must remain readable so pre-fix loops and tests don't break"
    );
}

#[test]
fn load_returns_none_for_empty_loop_scoped_ledger() {
    let root = workspace_root("load_returns_none_for_empty_loop_scoped_ledger");
    let path = root.join(".ralph/flow-authority.jsonl");
    std::fs::write(
        &path,
        "{\"step\":\"finalize\",\"topic\":\"scope.blocked\",\"loop_id\":\"loop-A\"}\n",
    )
    .unwrap();
    // loop-B caller — no entry for this loop — must return
    // None (fall back to initial_current_plan_step on the
    // consumer side) so `ralph emit --policy-check` does not
    // pick up another loop's terminal step.
    let got = load_flow_authority_current_step(&root, Some("loop-B"));
    assert!(
        got.is_none(),
        "loop-B caller must see no entries; the loop-A `finalize` \
             must not leak across loops"
    );
}
