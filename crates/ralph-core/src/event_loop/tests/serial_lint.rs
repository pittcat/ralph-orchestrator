//! Unit tests for the in-loop lint feedback path (U4b / R13).
//!
//! 2026-06-20-001 review v2 F1-F3: the BDD scenarios in
//! `tests/scenarios/serial_lint/` were dropped because the
//! scenario runner's per-iteration `next_hat()` returns the
//! fallback `ralph` hat on iteration 2 (after the executor's
//! rejected event was dropped), making `## LINT MIRROR`
//! injection unreachable from the SourceHat routing check.
//!
//! These unit tests exercise the in-loop feedback path
//! directly: `process_events_from_jsonl` populates
//! `state.pending_lint_resume`, then `inject_pending_lint_resume`
//! consumes it on the next `build_prompt` call. The test
//! fixture uses `write_object_event_to_jsonl` so the payload is
//! a real JSON object (the engine gate's required-field check
//! only operates on JSON objects — non-object payloads are
//! treated as missing-fields by the fail-closed `missing_fields`
//! helper).

use super::*;
use crate::event_loop::tests::common::write_object_event_to_jsonl;
use crate::preset::engine::LintResumeHint;

fn serial_lint_config() -> RalphConfig {
    let yaml = r#"
prompt_file: PROMPT.md
hats:
  executor:
    name: "Executor"
    subscribes_to: ["task.start"]
    publishes: ["work.done"]
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
  starting_event: "task.start"
  hat_handoff:
    enabled: true
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      work.done:
        required_fields:
          - plan_name
          - step
          - commit_count
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).expect("config parses");
    config.core.workspace_root = std::env::temp_dir();
    // Disable TTL so fixture timestamps don't trip freshness filter.
    config.event_loop.task_resume_ttl_seconds = Some(0);
    config
}

#[test]
fn u2_engine_gate_rejection_seeds_pending_lint_resume() {
    // R13 / R7-4 / review P0 #4: a missing-required-field
    // rejection must populate `state.pending_lint_resume` so
    // the agent's next prompt sees `## LINT RESUME REQUIRED`.
    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let mut config = serial_lint_config();
    config.core.workspace_root = temp.path().to_path_buf();
    let mut event_loop = EventLoop::with_context(
        config,
        crate::loop_context::LoopContext::primary(temp.path().to_path_buf()),
    );
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Emit `work.done` missing `plan_name` + `step` (only
    // `commit_count`). Engine gate should reject and seed the
    // hint.
    write_object_event_to_jsonl(
        &events_path,
        "work.done",
        serde_json::json!({"commit_count": 1}),
    );
    let _result = event_loop.process_events_from_jsonl().unwrap();

    // The hint must be populated (review P0 #4 — previous code
    // path did not seed the hint, leaving the agent without
    // feedback).
    let hint = event_loop
        .state
        .pending_lint_resume
        .as_ref()
        .expect("pending_lint_resume must be set after engine rejection");
    assert_eq!(hint.topic, "work.done");
    assert!(
        hint.reason.contains("missing required fields"),
        "reason must describe missing fields, got {:?}",
        hint.reason
    );
}

#[test]
fn u2_engine_gate_acceptance_does_not_seed_hint() {
    // Symmetric to the rejection case: an accepted event
    // must NOT populate `pending_lint_resume`. Otherwise the
    // next prompt would falsely claim a lint failure.
    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let mut config = serial_lint_config();
    config.core.workspace_root = temp.path().to_path_buf();
    let mut event_loop = EventLoop::with_context(
        config,
        crate::loop_context::LoopContext::primary(temp.path().to_path_buf()),
    );
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_object_event_to_jsonl(
        &events_path,
        "work.done",
        serde_json::json!({
            "plan_name": "feat-x",
            "step": "step-1",
            "commit_count": 1,
        }),
    );
    let _result = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        event_loop.state.pending_lint_resume.is_none(),
        "pending_lint_resume must stay None on accept"
    );
}

#[test]
fn u2_inject_consumes_pending_lint_resume() {
    // U4b: when `state.pending_lint_resume` is Some,
    // `build_prompt` injects `## LINT MIRROR` + `## LINT RESUME
    // REQUIRED` at the head of the prompt, then clears the
    // slot (consume-on-use).
    let temp = tempfile::tempdir().unwrap();
    let mut config = serial_lint_config();
    config.core.workspace_root = temp.path().to_path_buf();
    let mut event_loop = EventLoop::with_context(
        config,
        crate::loop_context::LoopContext::primary(temp.path().to_path_buf()),
    );
    event_loop.initialize("Test");

    // Seed a hint manually.
    event_loop.state.pending_lint_resume = Some(LintResumeHint::from_reason(
        "work.done",
        "missing required fields: plan_name,step",
    ));

    let hat_id = ralph_proto::HatId::new("executor");
    let prompt = event_loop
        .build_prompt(&hat_id)
        .expect("build_prompt returns Some");

    assert!(
        prompt.contains("## LINT MIRROR"),
        "prompt must include LINT MIRROR block, head: {:?}",
        &prompt[..prompt.len().min(200)]
    );
    assert!(
        prompt.contains("## LINT RESUME REQUIRED"),
        "prompt must include LINT RESUME REQUIRED block"
    );
    assert!(
        prompt.contains("missing required fields"),
        "prompt must include the rejection reason"
    );

    // Consume-on-use: the slot is now None.
    assert!(
        event_loop.state.pending_lint_resume.is_none(),
        "pending_lint_resume must be cleared after consume"
    );
}

#[test]
fn u2_inject_no_op_when_hint_slot_empty() {
    // Symmetric: when `state.pending_lint_resume` is None,
    // `build_prompt` must NOT inject anything. Otherwise we
    // would accumulate stale resume blocks.
    let temp = tempfile::tempdir().unwrap();
    let mut config = serial_lint_config();
    config.core.workspace_root = temp.path().to_path_buf();
    let mut event_loop = EventLoop::with_context(
        config,
        crate::loop_context::LoopContext::primary(temp.path().to_path_buf()),
    );
    event_loop.initialize("Test");
    event_loop.state.pending_lint_resume = None;

    let hat_id = ralph_proto::HatId::new("executor");
    let prompt = event_loop
        .build_prompt(&hat_id)
        .expect("build_prompt returns Some");

    assert!(
        !prompt.contains("## LINT MIRROR"),
        "prompt must NOT include LINT MIRROR when no hint"
    );
    assert!(
        !prompt.contains("## LINT RESUME REQUIRED"),
        "prompt must NOT include LINT RESUME REQUIRED when no hint"
    );
}

#[test]
fn u2_inject_misrouted_hat_restores_hint() {
    // U4b routing: in isolated mode, if the active hat does
    // not own the hint's topic (per its `publishes` list),
    // the hint is restored (not consumed) so the right hat's
    // next prompt can use it.
    let temp = tempfile::tempdir().unwrap();
    let mut config = serial_lint_config();
    config.core.workspace_root = temp.path().to_path_buf();
    let mut event_loop = EventLoop::with_context(
        config,
        crate::loop_context::LoopContext::primary(temp.path().to_path_buf()),
    );
    event_loop.initialize("Test");

    // The executor hat publishes `work.done` (per
    // serial_lint_config). Seed a hint on a topic the
    // executor does NOT publish (`debug.exhausted`) so
    // routing rejects and the hint is restored.
    event_loop.state.pending_lint_resume = Some(LintResumeHint::from_reason(
        "debug.exhausted",
        "missing required fields",
    ));
    let executor_id = ralph_proto::HatId::new("executor");
    let prompt = event_loop.build_prompt(&executor_id);
    assert!(prompt.is_some());

    // Hint must still be set (restored, not consumed).
    assert!(
        event_loop.state.pending_lint_resume.is_some(),
        "pending_lint_resume must be restored when current hat does not own hint topic"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Plan 2026-06-20-001 KTD-7 / RISK-6: lint circuit breaker
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn u2_circuit_breaker_trips_after_consecutive_rejections() {
    // After `LINT_CIRCUIT_BREAKER_LIMIT` (2) consecutive
    // iterations in which the engine gate rejects every
    // event, `lint_circuit_breaker_tripped` must latch. The
    // d623c09 runtime gates keep running (termination is
    // the existing `consecutive_malformed_events >= 3`
    // check, which is a different backstop).
    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let mut config = serial_lint_config();
    config.core.workspace_root = temp.path().to_path_buf();
    let mut event_loop = EventLoop::with_context(
        config,
        crate::loop_context::LoopContext::primary(temp.path().to_path_buf()),
    );
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Iter 1: one rejection. Counter goes 0 → 1 (not yet at
    // the limit). Breaker must NOT have tripped.
    write_object_event_to_jsonl(
        &events_path,
        "work.done",
        serde_json::json!({"commit_count": 1}),
    );
    let _ = event_loop.process_events_from_jsonl().unwrap();
    assert_eq!(
        event_loop.state.consecutive_engine_gate_rejections, 1,
        "first full-rejection iter must set counter to 1"
    );
    assert!(
        !event_loop.state.lint_circuit_breaker_tripped,
        "breaker must NOT trip on a single rejection"
    );

    // Iter 2: another rejection. Counter goes 1 → 2, hits
    // the limit, breaker trips.
    write_object_event_to_jsonl(
        &events_path,
        "work.done",
        serde_json::json!({"commit_count": 2}),
    );
    let _ = event_loop.process_events_from_jsonl().unwrap();
    assert_eq!(
        event_loop.state.consecutive_engine_gate_rejections, 2,
        "second full-rejection iter must set counter to 2"
    );
    assert!(
        event_loop.state.lint_circuit_breaker_tripped,
        "breaker MUST trip at LINT_CIRCUIT_BREAKER_LIMIT=2"
    );

    // Iter 3: a legal event arrives. The breaker has latched
    // so the engine gate short-circuits. d623c09 still runs
    // and may filter further; what matters for KTD-7 is
    // (a) the engine gate did NOT bump the rejection counter
    //     past 2 (still 2),
    // (b) the breaker is still latched (one-way latch until
    //     loop restart / RALPH_SERIAL_LINT_MODE=off), and
    // (c) the breaker remained "dormant" — it did not
    //     accumulate a third rejection for a legal event.
    write_object_event_to_jsonl(
        &events_path,
        "work.done",
        serde_json::json!({
            "plan_name": "feat-x",
            "step": "step-1",
            "commit_count": 3,
        }),
    );
    let _ = event_loop.process_events_from_jsonl().unwrap();
    assert_eq!(
        event_loop.state.consecutive_engine_gate_rejections, 2,
        "counter must NOT advance on a legal event after the breaker tripped"
    );
    assert!(
        event_loop.state.lint_circuit_breaker_tripped,
        "breaker must stay latched after the legal event"
    );
}

#[test]
fn u2_circuit_breaker_resets_on_acceptance() {
    // When the engine gate accepts at least one event, the
    // counter resets to 0 — the gate is still useful, so
    // the breaker does not trip even after multiple full
    // rejections in a row, as long as they are interleaved
    // with accepts.
    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let mut config = serial_lint_config();
    config.core.workspace_root = temp.path().to_path_buf();
    let mut event_loop = EventLoop::with_context(
        config,
        crate::loop_context::LoopContext::primary(temp.path().to_path_buf()),
    );
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Iter 1: rejection. Counter → 1.
    write_object_event_to_jsonl(
        &events_path,
        "work.done",
        serde_json::json!({"commit_count": 1}),
    );
    let _ = event_loop.process_events_from_jsonl().unwrap();
    assert_eq!(event_loop.state.consecutive_engine_gate_rejections, 1);

    // Iter 2: ACCEPT. Counter resets to 0.
    write_object_event_to_jsonl(
        &events_path,
        "work.done",
        serde_json::json!({
            "plan_name": "feat-x",
            "step": "step-1",
            "commit_count": 2,
        }),
    );
    let _ = event_loop.process_events_from_jsonl().unwrap();
    assert_eq!(
        event_loop.state.consecutive_engine_gate_rejections, 0,
        "any accept must reset the counter"
    );
    assert!(
        !event_loop.state.lint_circuit_breaker_tripped,
        "breaker must NOT trip when counter resets on accepts"
    );
}
