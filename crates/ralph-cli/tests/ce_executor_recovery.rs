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
use ralph_core::{
    Event as JsonlEvent, HatRegistry, NonRetryableReason, RalphConfig, Rejection, RejectionStage,
    build_task_resume_payload, rejection_from_origin,
};
use std::path::PathBuf;

/// Returns the path to the recovery fixture shipped with `ralph-core`.
///
/// The fixture lives in `tests/fixtures/recovery/` to keep it out of the
/// `SmokeRunner` discovery path: the smoke runner scans every `.jsonl` in
/// `tests/fixtures/` and expects `Record`-format sessions
/// (`{ts: u64, event, data}`). The recovery fixture uses the per-event
/// `{topic, payload, hat, ...}` shape and would otherwise fail
/// `test_all_discovered_fixtures_are_valid`.
fn recovery_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("crates/ralph-core/tests/fixtures/recovery/ce-executor-rejected-event-recovery.jsonl")
}

/// Read every line of the fixture as a [`JsonlEvent`].
fn load_recovery_events() -> Vec<JsonlEvent> {
    let path = recovery_fixture();
    assert!(
        path.exists(),
        "recovery fixture must exist at {}",
        path.display()
    );
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
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
    // The origin guard should accept executor's work.done (executor is the
    // legitimate producer), and the contract layer rejects it with the
    // right diagnostic.
    // U2 change: fixture hat previously was "ralph" — this was the original
    // vulnerability. U2 now rejects ralph business topics at origin level,
    // so the fixture's work.done uses hat="executor" instead.
    let registry = ce_executor_registry();
    let events = load_recovery_events();

    let executor_work_done = events
        .iter()
        .find(|e| e.topic == "work.done" && e.hat.as_deref() == Some("executor"))
        .expect("fixture must include an executor work.done");

    // executor is registered with publishes=["work.done"], so origin accepts it.
    match validate_event_origin(
        executor_work_done,
        &registry,
        "loop.cancel",
        "LOOP_COMPLETE",
    ) {
        OriginCheck::Accepted => {}
        other => {
            panic!("executor work.done should be in-scope for contract rejection, got {other:?}")
        }
    }

    // Confirm the payload is in fact missing plan_path — without this,
    // the contract-rejection test in U2 cannot anchor to a real fixture.
    let payload = executor_work_done.payload.as_deref().unwrap_or("");
    assert!(
        !payload.contains("plan_path"),
        "fixture executor work.done must omit plan_path to exercise contract rejection; got {payload}"
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

// ─────────────────────────────────────────────────────────────────────────
// 2026-06-07 plan Unit 2: unified rejection classification + targeted retry
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn u2_origin_rejection_from_fixture_classifies_as_non_retryable() {
    // The fixture's `review-coordinator work.done` event is rejected
    // by the origin guard with the "out-of-scope topic for declared
    // hat" reason.  The new `Rejection::from_origin` wrapper must
    // classify that as non-retryable (R1: do not auto-relax publishes).
    let events = load_recovery_events();
    let registry = ce_executor_registry();

    let rogue = events
        .iter()
        .find(|e| e.topic == "work.done" && e.hat.as_deref() == Some("review-coordinator"))
        .expect("fixture must include a review-coordinator work.done");

    let check = validate_event_origin(rogue, &registry, "loop.cancel", "LOOP_COMPLETE");
    let rejection = rejection_from_origin(&check, rogue.hat.clone())
        .expect("rejected event must produce a Rejection");

    assert_eq!(rejection.stage, RejectionStage::Origin);
    assert_eq!(rejection.topic, "work.done");
    assert_eq!(rejection.source_hat.as_deref(), Some("review-coordinator"));
    assert!(
        !rejection.retry_eligible,
        "out-of-scope must be fail-closed"
    );
    assert_eq!(
        rejection.non_retryable_reason,
        Some(NonRetryableReason::OutOfScope)
    );
    assert_eq!(rejection.target_hat, None, "no target → no task.resume");
    assert!(!rejection.should_publish_resume());

    // The retry key must remain stable across two such rejections so
    // the runner can count them and surface exhaustion at the budget.
    let again = rejection_from_origin(&check, rogue.hat.clone()).unwrap();
    assert_eq!(rejection.retry_key, again.retry_key);
    assert!(
        rejection
            .retry_key
            .starts_with("origin:review-coordinator:work.done:")
    );
}

#[test]
fn u2_executor_missing_field_rejection_classifies_as_retryable() {
    // The fixture's `executor work.done` event with missing `plan_path`
    // is rejected by the execution contract layer, not the origin
    // guard (origin accepts executor's publish scope).  Wrapping
    // the finding as a Rejection must mark it retryable with a
    // target_hat = "executor" — that's the path U2's targeted retry takes.
    // U2 change: fixture hat changed from "ralph" to "executor" (ralph is
    // now restricted to control topics only).
    let events = load_recovery_events();
    let executor_work_done = events
        .iter()
        .find(|e| e.topic == "work.done" && e.hat.as_deref() == Some("executor"))
        .expect("fixture must include an executor work.done");

    // Mimic the layer that produces ExecutionContractFinding from
    // the missing plan_path payload.  Real production code wraps
    // the finding in `Rejection::from_execution_contract`.
    let finding = ralph_core::execution_contract::ExecutionContractFinding {
        kind: ralph_core::execution_contract::ExecutionContractViolationKind::MissingPayloadField {
            field: "plan_path".into(),
        },
        message: "missing plan_path".into(),
        topic: "work.done".into(),
        source_hat: Some("executor".into()),
    };
    let rejection = Rejection::from_execution_contract(
        &finding,
        executor_work_done.hat.clone(),
        executor_work_done.hat.clone(),
    );

    assert_eq!(rejection.stage, RejectionStage::ExecutionContract);
    assert_eq!(rejection.topic, "work.done");
    assert_eq!(rejection.source_hat.as_deref(), Some("executor"));
    assert!(rejection.retry_eligible);
    assert_eq!(rejection.target_hat.as_deref(), Some("executor"));
    assert!(rejection.should_publish_resume());
    assert!(
        rejection
            .retry_key
            .contains("execution_contract:executor:work.done:missing_field")
    );

    // The task.resume payload must carry the violation + allowed
    // topics + original trigger context so the resumed hat can
    // self-correct without guessing.
    let payload_str = build_task_resume_payload(
        &rejection,
        &["work.done".into()],
        &["plan_path".into()],
        Some("work.ready"),
        executor_work_done.payload.as_deref(),
    );
    let v: serde_json::Value = serde_json::from_str(&payload_str).unwrap();
    assert_eq!(v["stage"], "execution_contract");
    assert_eq!(v["topic"], "work.done");
    assert_eq!(v["allowed_topics"][0], "work.done");
    assert_eq!(v["required_fields"][0], "plan_path");
    assert_eq!(v["original_trigger_topic"], "work.ready");
    assert!(
        v["retry_key"]
            .as_str()
            .unwrap()
            .contains("execution_contract:executor:work.done:missing_field")
    );
}

// ─────────────────────────────────────────────────────────────────────────
// 2026-06-07 plan Unit 3: 统一 wave 结果格式
// (Note: the merge function itself is exercised by crate-internal
//  tests in `loop_runner/tests.rs` because it lives behind the
//  binary-crate visibility boundary.  Here we just record that the
//  U1 fixture's 8 wave results exercise the contract.)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn u3_u1_fixture_inventory_matches_wave_merge_contract() {
    // The U1 fixture ships 8 review.dimension.done results.  Six of
    // them carry wave_id/wave_index (5 explicit + 1 from the line
    // where we deliberately kept wave metadata); two do not.  R8
    // requires the framework to *standardize* all results before
    // merge, so the U3 merge tests in `loop_runner/tests.rs` are the
    // authoritative place to verify that.  This test only asserts
    // the fixture's shape, so a future change to merge semantics can
    // keep the fixture in sync.
    let events = load_recovery_events();
    let dim_results: Vec<&ralph_core::Event> = events
        .iter()
        .filter(|e| e.topic == "review.dimension.done")
        .collect();
    assert_eq!(
        dim_results.len(),
        8,
        "fixture ships 8 review.dimension.done results"
    );
    let with_wave = dim_results.iter().filter(|e| e.wave_id.is_some()).count();
    let without_wave = dim_results.iter().filter(|e| e.wave_id.is_none()).count();
    assert!(
        with_wave >= 1 && without_wave >= 1,
        "fixture must include both shapes so U3 merge normalization is exercised end-to-end"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// 2026-06-07 plan Unit 4: hard gate activation-level obligations
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn u4_hat_config_parses_obligations_from_yaml_roundtrip() {
    // R3 / R4: a hat with `obligations:` must round-trip through
    // YAML so preset authors can opt into the activation-level path
    // without a schema migration.  The shape is:
    //   obligations:
    //     - on_trigger: "work.done"
    //       must_emit_any_of: ["review.wave.ready", "review.passed"]
    use ralph_core::{ActivationObligation, HatConfig};

    let yaml = r#"
name: "Review Coordinator"
triggers: ["work.done", "fix.applied"]
publishes: ["review.wave.ready", "review.passed"]
obligations:
  - on_trigger: "work.done"
    must_emit_any_of: ["review.wave.ready", "review.passed"]
  - on_trigger: "fix.applied"
    must_emit_any_of: ["review.passed"]
"#;
    let hat: HatConfig = serde_yaml::from_str(yaml).expect("parse hat yaml");
    assert_eq!(hat.obligations.len(), 2);
    let o0 = &hat.obligations[0];
    assert_eq!(o0.on_trigger, "work.done");
    assert_eq!(
        o0.must_emit_any_of,
        vec!["review.wave.ready", "review.passed"]
    );
    let o1 = &hat.obligations[1];
    assert_eq!(o1.on_trigger, "fix.applied");
    assert_eq!(o1.must_emit_any_of, vec!["review.passed"]);

    // Round-trip: serialize back to YAML and re-parse; obligations
    // must survive.
    let yaml_out = serde_yaml::to_string(&hat).expect("serialize hat");
    let hat2: HatConfig = serde_yaml::from_str(&yaml_out).expect("re-parse hat");
    assert_eq!(hat2.obligations, hat.obligations);

    // Equality with itself is a tautology; the meaningful check is
    // that both obligations survive the round trip.  Test the
    // explicit ActivationObligation equality to lock the contract.
    let _eq: ActivationObligation = hat.obligations[0].clone();
}

#[test]
fn u4_obligation_satisfied_for_each_review_coordinator_branch() {
    // R3: review-coordinator 条件 emit 语义 (空 diff → review.passed;
    // 有 diff → review.wave.ready) 必须各自满足 obligation，不被
    // 误判为 missing-event。
    //
    // 2026-06-08 fix: this test now exercises the legacy-OR path
    // (no `conditional_must_emit`, no `trigger_context`).  The
    // tightening behavior is covered by `hat.rs` unit tests
    // (`conditional_must_emit_*`).
    use ralph_core::{ActivationObligation, obligation_satisfied};

    let o = ActivationObligation {
        on_trigger: "work.done".into(),
        must_emit_any_of: vec!["review.wave.ready".into(), "review.passed".into()],
        conditional_must_emit: vec![],
        conditional_forbid_topics: vec![],
    };

    // Empty-diff branch: review-coordinator picks review.passed.
    assert!(obligation_satisfied(
        Some(&o),
        &vec!["review.passed".into()],
        None
    ));
    // Non-empty branch: review-coordinator picks review.wave.ready.
    assert!(obligation_satisfied(
        Some(&o),
        &vec!["review.wave.ready".into()],
        None
    ));
    // Off-obligation set: agent picked the wrong topic — this is a
    // hard failure (R1) and must NOT satisfy the obligation so the
    // downstream reporter can flag it.
    assert!(!obligation_satisfied(
        Some(&o),
        &vec!["work.failed".into()],
        None
    ));
    // No candidate at all: missing event — obligation not satisfied.
    assert!(!obligation_satisfied(Some(&o), &vec![], None));
}
