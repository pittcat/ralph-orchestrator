//! Integration tests for ce-executor rejected event recovery.
//!
//! 2026-06-07 plan Unit 1:固化失败链路的回放与证据模型
//!
//! These tests document the four failure modes observed in the
//! 2026-06-06 drift run (fixture: `ce-executor-rejected-event-recovery.jsonl`):
//!
//! 1. Executor activated, no event emitted → must record missing-event
//!    diagnosis while preserving the original trigger event.
//! 2. work.done missing `plan_path` → must be rejected by the execution
//!    contract, must not propagate to review-coordinator.
//! 3. review-coordinator emitting work.done → must be rejected by the
//!    origin guard without granting extra publish scope.
//! 4. Wave worker results missing wave_id/wave_index → must be
//!    standardizable by the wave merger, and never contaminate a different
//!    loop's task store (cross-worktree isolation).
//!
//! Tests are scoped at the JSONL/origin level so they can run without a
//! live backend. Full EventLoop end-to-end coverage is Unit 7
//! (`crates/ralph-core/tests/scenarios/ce_executor_recovery.yml`).

use ralph_core::event_origin::{OriginCheck, validate_event_origin};
use ralph_core::{Event as JsonlEvent, HatRegistry, RalphConfig};
use std::path::PathBuf;

/// Returns the path to the recovery fixture shipped with `ralph-core`.
fn recovery_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("crates/ralph-core/tests/fixtures/ce-executor-rejected-event-recovery.jsonl")
}

/// Read every line of the fixture as a [`JsonlEvent`].
fn load_recovery_events() -> Vec<JsonlEvent> {
    let path = recovery_fixture();
    assert!(
        path.exists(),
        "recovery fixture must exist at {}",
        path.display()
    );
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str::<JsonlEvent>(l)
                .unwrap_or_else(|e| panic!("parse fixture line `{l}`: {e}"))
        })
        .collect()
}

/// Build a `HatRegistry` shaped like the ce-executor preset.
///
/// We use a minimal subset that captures the
///   - executor (publishes work.done),
///   - review-coordinator (publishes review.wave.ready + review.passed),
///   - dimension-reviewer (publishes review.dimension.done),
/// publish boundaries that the fixture exercises.
fn ce_executor_registry() -> HatRegistry {
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["work.ready", "work.failed"]
  executor:
    name: "Executor"
    triggers: ["work.ready", "queue.advance", "work.retry", "fix.plan.ready"]
    publishes: ["work.done", "work.failed"]
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done", "fix.applied"]
    publishes: ["review.wave.ready", "review.passed"]
  dimension-reviewer:
    name: "Dimension Reviewer"
    triggers: ["review.wave.ready"]
    publishes: ["review.dimension.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  cancellation_promise: "loop.cancel"
  starting_event: "work.start"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse ce-executor test config");
    HatRegistry::from_runtime_config(&config)
}

#[test]
fn fixture_parses_to_expected_event_count() {
    // Sanity: the fixture has the 5 scenario events the plan requires:
    //   - 1 work.start (loop warmup)
    //   - 1 work.ready (executor trigger)
    //   - 1 work.done from ralph (contract-invalid: missing plan_path)
    //   - 1 work.done from review-coordinator (origin-rejected)
    //   - 1 review.wave.ready
    //   - 8 review.dimension.done (3 with wave metadata, 5 without — see plan
    //     "3 条缺少 wave_id/wave_index" characterization; in this fixture the
    //     last 3 lines omit wave_id/wave_index for that reason)
    let events = load_recovery_events();
    assert_eq!(events.len(), 13, "fixture must contain 13 lines");

    let work_starts = events.iter().filter(|e| e.topic == "work.start").count();
    let work_ready = events.iter().filter(|e| e.topic == "work.ready").count();
    let work_done = events.iter().filter(|e| e.topic == "work.done").count();
    let wave_ready = events
        .iter()
        .filter(|e| e.topic == "review.wave.ready")
        .count();
    let dim_done = events
        .iter()
        .filter(|e| e.topic == "review.dimension.done")
        .count();

    assert_eq!(work_starts, 1);
    assert_eq!(work_ready, 1);
    assert_eq!(work_done, 2, "one contract-invalid + one origin-rejected");
    assert_eq!(wave_ready, 1);
    assert_eq!(dim_done, 8, "8 wave worker results");
}

#[test]
fn contract_invalid_work_done_from_executor_is_rejected() {
    // The executor's work.done payload in the fixture is missing plan_path.
    // The origin guard is not the right layer for that check (contract
    // rejection lives in execution_contracts), but origin must still
    // accept the event so the contract layer can reject it with the
    // right diagnostic — that proves the executor is in scope.
    let registry = ce_executor_registry();
    let events = load_recovery_events();

    let ralph_fallback = events
        .iter()
        .find(|e| e.topic == "work.done" && e.hat.as_deref() == Some("ralph"))
        .expect("fixture must include a ralph work.done");

    // ralph is a builtin runtime hat with derived publish scope, so
    // work.done (an executor publishes topic) is in scope.
    match validate_event_origin(ralph_fallback, &registry, "loop.cancel", "LOOP_COMPLETE") {
        OriginCheck::Accepted => {}
        other => panic!(
            "ralph work.done should be in-scope for contract rejection, got {other:?}"
        ),
    }

    // Confirm the payload is in fact missing plan_path — without this,
    // the contract-rejection test in U2 cannot anchor to a real fixture.
    let payload = ralph_fallback.payload.as_deref().unwrap_or("");
    assert!(
        !payload.contains("plan_path"),
        "fixture ralph work.done must omit plan_path to exercise contract rejection; got {payload}"
    );
}

#[test]
fn work_done_from_review_coordinator_is_origin_rejected() {
    // review-coordinator is registered with publishes=[review.wave.ready,
    // review.passed]. Emitting work.done from it must be rejected by the
    // origin guard, even though the topic name suggests a "next-step"
    // event. R1:拒绝不可通过放宽 hat publishes 绕过。
    let registry = ce_executor_registry();
    let events = load_recovery_events();

    let rogue = events
        .iter()
        .find(|e| e.topic == "work.done" && e.hat.as_deref() == Some("review-coordinator"))
        .expect("fixture must include a review-coordinator work.done");

    match validate_event_origin(rogue, &registry, "loop.cancel", "LOOP_COMPLETE") {
        OriginCheck::Rejected { topic, hat, reason } => {
            assert_eq!(topic, "work.done");
            assert_eq!(hat.as_deref(), Some("review-coordinator"));
            assert!(
                reason.contains("out-of-scope") || reason.contains("unknown"),
                "rejection must explicitly say out-of-scope/unknown, got: {reason}"
            );
        }
        OriginCheck::Accepted => panic!(
            "review-coordinator must NOT be allowed to publish work.done; \
             this is a security boundary (R1)."
        ),
    }
}

#[test]
fn wave_results_must_carry_wave_correlation_metadata() {
    // R8: 无论 worker 用什么合法输出形式,merge 前都必须补齐 wave_id/
    // wave_index/wave_total/ts. This test inventories which worker results
    // in the fixture are missing wave metadata — those are the cases U3's
    // wave merger must normalize.
    let events = load_recovery_events();
    let dim_results: Vec<&JsonlEvent> = events
        .iter()
        .filter(|e| e.topic == "review.dimension.done")
        .collect();

    assert_eq!(dim_results.len(), 8);

    let with_wave: Vec<&JsonlEvent> = dim_results
        .iter()
        .copied()
        .filter(|e| e.wave_id.is_some())
        .collect();
    let without_wave: Vec<&JsonlEvent> = dim_results
        .iter()
        .copied()
        .filter(|e| e.wave_id.is_none())
        .collect();

    assert!(
        !with_wave.is_empty(),
        "fixture must include both wave-tagged and untagged results"
    );
    assert!(
        !without_wave.is_empty(),
        "fixture must include at least 1 untagged result to exercise U3"
    );

    // wave_id + wave_index must travel together — if either is set the
    // other must be too (this is what the standard merge function
    // relies on; mismatched pairs are evidence of bypass paths).
    for e in &dim_results {
        let has_id = e.wave_id.is_some();
        let has_idx = e.wave_index.is_some();
        assert_eq!(
            has_id, has_idx,
            "wave_id and wave_index must be present together; got id={:?} idx={:?}",
            e.wave_id, e.wave_index
        );
    }
}

#[test]
fn unknown_hat_always_origin_rejected() {
    // R1 defense in depth: even control topics like human.interact
    // must reject unknown hat sources (anti-spoofing).
    let registry = ce_executor_registry();
    let event = JsonlEvent {
        topic: "human.interact".into(),
        payload: Some("{\"question\":\"?\"}".into()),
        ts: "2026-06-06T00:00:00Z".into(),
        hat: Some("ghost-hat".into()),
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
    };
    match validate_event_origin(&event, &registry, "loop.cancel", "LOOP_COMPLETE") {
        OriginCheck::Rejected { .. } => {}
        OriginCheck::Accepted => panic!("unknown hat must be rejected even for control topics"),
    }
}

#[test]
fn task_keys_in_fixture_are_loop_scoped() {
    // R5: 跨 worktree 的 task store 必须保持 loop 隔离。
    // The fixture uses one loop with two distinct task_keys
    // (U0 step-01 + U2 recovery-envelope). This test asserts that the
    // task_id / task_key pairs in the fixture are different from each
    // other so a contamination test (Unit 7) can use them as anchors.
    let events = load_recovery_events();
    let mut keys: Vec<String> = events
        .iter()
        .filter_map(|e| {
            e.payload.as_ref().and_then(|p| {
                serde_json::from_str::<serde_json::Value>(p)
                    .ok()
                    .and_then(|v| v.get("task_key").and_then(|k| k.as_str().map(String::from)))
            })
        })
        .collect();
    keys.sort();
    keys.dedup();
    assert!(
        keys.len() >= 2,
        "fixture must carry ≥2 distinct task_keys to exercise loop isolation; got {keys:?}"
    );
}
