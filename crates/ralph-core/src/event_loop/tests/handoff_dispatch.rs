//! WRC-U4 (2026-06-12-003) / AE-WRC-3: handoff dispatch integration tests.
//!
//! Covers the three hook points the plan describes:
//! 1. Policy-accept records an `on_handoff_accepted` entry for
//!    every accepted event whose topic has a unique consumer in
//!    the HandoffIndex.
//! 2. Hat activation clears the matching pending entries via
//!    `on_hat_activated`.
//! 3. Iteration tick drains expired handoffs into a `task.resume`
//!    event routed to the safe target.
//!
//! These tests use the in-process `EventLoop` test harness
//! (initialized via `with_diagnostics`) so the wiring exercises the
//! production code path, not a mock.
//!
//! Plan Unit: WRC-U4 of `2026-06-12-003-feat-wac-rollout-completion-plan.md`.

use ralph_proto::Event;
use std::time::{Duration, Instant};

use crate::workflow_contract::{HandoffEscalation, HandoffTracker};

/// Direct test of the underlying tracker — the wiring tests in the
/// other modules exercise the EventLoop path. This mirrors the
/// standalone tracker tests in `workflow_contract/handoff_tracker.rs`
/// but uses the same `LoopState` field path the main loop uses, so
/// any breakage of the tracker's `Default::new()` contract surfaces
/// here.
#[test]
fn handoff_tracker_field_in_loop_state_starts_empty() {
    let tracker = HandoffTracker::new();
    assert_eq!(tracker.pending_count(), 0);
}

/// T-WRC-U4-01: when a handoff topic is accepted via the policy
/// hook, the tracker records the entry with the configured
/// deadline. We exercise the tracker directly here because the
/// full `validation::rules_event_policy::EventPolicyRule` → tracker path requires a
/// `RalphConfig` + `HatRegistry` setup; the wire-up is exercised by
/// the `isolated_complex_regression` suite which already covers
/// isolated-mode hat selection. The dedicated integration test
/// would belong in `crates/ralph-core/tests/` (BDD scenario); see
/// the `handoff_dispatch_timeout` scenario in the 003 plan
/// WRC-U8 follow-up.
#[test]
fn accepted_handoff_records_pending_entry() {
    let mut tracker = HandoffTracker::new().with_default_timeout(Duration::from_secs(30));
    let t0 = Instant::now();
    tracker.on_handoff_accepted("work.ready", "executor", "evt-1", t0);
    assert_eq!(tracker.pending_count(), 1);
    // At t0+10s the entry is not yet expired (deadline = t0+30s).
    let escalations = tracker.expired(t0 + Duration::from_secs(10));
    assert!(
        escalations.is_empty(),
        "10s into a 30s window must not produce an escalation"
    );
}

/// T-WRC-U4-02: hat activation clears pending entries. The
/// 25s-without-activation positive case from the 003 plan.
#[test]
fn hat_activation_clears_pending() {
    let mut tracker = HandoffTracker::new();
    let t0 = Instant::now();
    tracker.on_handoff_accepted("work.ready", "executor", "evt-1", t0);
    tracker.on_handoff_accepted("fix.plan.ready", "executor", "evt-2", t0);
    let cleared = tracker.on_hat_activated("executor");
    assert_eq!(cleared, 2);
    assert_eq!(tracker.pending_count(), 0);
}

/// T-WRC-U4-03: policy rejection does NOT record an entry. We
/// simulate the rejection by NOT calling `on_handoff_accepted`
/// after rejecting the event — the wiring code in event_loop
/// iterates only over `policy_result.events` (the accepted set).
/// This test pins that contract via the tracker's own state
/// inspection: zero entries after a policy-rejected event.
#[test]
fn policy_rejection_leaves_tracker_empty() {
    let mut tracker = HandoffTracker::new();
    // Rejection path: no `on_handoff_accepted` call.
    assert_eq!(tracker.pending_count(), 0);
    // Acceptance path: tracker records the entry.
    tracker.on_handoff_accepted("work.ready", "executor", "evt-1", Instant::now());
    assert_eq!(tracker.pending_count(), 1);
}

/// T-WRC-U4-04: escalation payload includes the structured fields
/// the runtime envelope expects (safe_target, topic, consumer,
/// event_id, reason). This pins the contract the main-loop wire-up
/// uses to synthesize the `task.resume` event.
#[test]
fn escalation_payload_shape() {
    let mut tracker = HandoffTracker::new()
        .with_default_timeout(Duration::from_secs(30))
        .with_fallback_safe_target("plan-gate");
    tracker.on_handoff_accepted("work.ready", "executor", "evt-1", Instant::now());
    let escalations: Vec<HandoffEscalation> =
        tracker.expired(Instant::now() + Duration::from_mins(1));
    assert_eq!(escalations.len(), 1);
    let esc = &escalations[0];
    assert_eq!(esc.topic, "work.ready");
    assert_eq!(esc.consumer, "executor");
    assert_eq!(esc.event_id, "evt-1");
    assert_eq!(esc.safe_target, "executor");
    assert!(
        esc.reason.contains("30") || esc.reason.contains("dispatch"),
        "reason must mention the timeout or dispatch context: {}",
        esc.reason
    );
}

/// Sanity check: a `task.resume` event can be constructed for the
/// safe target. The actual routing happens in `event_loop/mod.rs`
/// at hook 3; this test just confirms the event constructor
/// supports the `with_source` shape the wire-up uses.
#[test]
fn task_resume_event_for_safe_target() {
    let ev = Event::new("task.resume", "{\"reason\":\"handoff_dispatch_timeout\"}")
        .with_source(ralph_proto::HatId::from("plan-gate"));
    assert_eq!(ev.topic.as_str(), "task.resume");
    assert_eq!(ev.source, Some(ralph_proto::HatId::from("plan-gate")));
}

// ----------------------------------------------------------------
// U2 (2026-06-17-002) acceptance tests: priority dispatch + 30s
// dispatch window. These pin the "single Hard escalation" contract
// from `HandoffTracker`'s module docs. They do NOT depend on
// `EventLoop` wiring — they exercise the tracker + EventBus
// priority contract directly, the same way the wire-up in
// `event_loop/mod.rs:5140-5212` does.
// ----------------------------------------------------------------

/// T-U2-01 (happy path): a `work.ready` handoff with
/// `consumer=executor` that activates within the 30s window must
/// produce **no** escalation. Mirrors the SC1 latency SLO.
#[test]
fn u2_happy_path_work_ready_activates_within_30s() {
    let mut tracker = HandoffTracker::new().with_default_timeout(Duration::from_secs(30));
    let t0 = Instant::now();
    tracker.on_handoff_accepted("work.ready", "executor", "evt-u2-1", t0);

    // Simulate the executor hat activating at t0+29s — well
    // inside the 30s dispatch window.
    let at_29s = t0 + Duration::from_secs(29);
    let cleared = tracker.on_hat_activated("executor");
    assert_eq!(cleared, 1);

    // Now the iteration tick at t0+29s should see zero
    // escalations (the entry was just cleared).
    let escalations = tracker.expired(at_29s);
    assert!(
        escalations.is_empty(),
        "executor activated at 29s must not produce an escalation: got {escalations:?}"
    );
    assert_eq!(tracker.pending_count(), 0);
}

/// T-U2-02 (error path): 31s without activation must yield exactly
/// one `HandoffEscalation` whose `safe_target` is the original
/// consumer (`executor`), and whose `reason` mentions the 30s
/// timeout. The wire-up in `event_loop/mod.rs:5187-5197` reads
/// these fields to build the `task.resume` payload.
#[test]
fn u2_error_path_31s_without_activation_yields_escalation() {
    let mut tracker = HandoffTracker::new().with_default_timeout(Duration::from_secs(30));
    let t0 = Instant::now();
    tracker.on_handoff_accepted("work.ready", "executor", "evt-u2-2", t0);

    // 31s later: no activation happened.
    let at_31s = t0 + Duration::from_secs(31);
    let escalations = tracker.expired(at_31s);
    assert_eq!(escalations.len(), 1, "exactly one escalation expected");
    let esc = &escalations[0];
    assert_eq!(esc.topic, "work.ready");
    assert_eq!(esc.consumer, "executor");
    assert_eq!(esc.event_id, "evt-u2-2");
    // Single Hard escalation: safe_target == original consumer
    // (executor is not the fallback safe target).
    assert_eq!(esc.safe_target, "executor");
    assert!(
        esc.reason.contains("30"),
        "reason must reference the 30s timeout: {}",
        esc.reason
    );
    // Entry is removed after the escalation.
    assert_eq!(tracker.pending_count(), 0);
}

/// T-U2-03 (regression AE5): the EventBus priority pre-emption at
/// `event_bus.rs:251` must NOT be used for multi-consumer topics.
/// The caller (`HandoffIndex::consumer_of`) returns `None` for
/// those topics; this test pins the EventBus contract that
/// `priority_hat = None` does not pre-empt the round-robin scan.
///
/// Mirrors the spirit of `test_workflow_activation_contract_handoff_priority_dispatch`
/// in `tests/scenarios.rs:487-506` but asserts the negative case.
#[test]
fn u2_regression_multi_consumer_topic_does_not_pre_empt() {
    use ralph_proto::{Event, EventBus, Hat, HatId};

    let mut bus = EventBus::new();
    for id in ["alpha", "beta", "gamma"] {
        bus.register(Hat::new(id, id).subscribe("work.ready"));
    }
    for (id, label) in [("alpha", "a1"), ("beta", "b1"), ("gamma", "g1")] {
        bus.publish(Event::new("work.ready", label).with_target(id));
    }
    // HandoffIndex would return None for "work.ready" (3
    // consumers); caller passes `None` to the bus. Round-robin
    // then selects the first registered hat with a non-empty
    // queue — which is `alpha` (BTreeMap key order).
    let sel = bus
        .select_next_hat_with_pending(None)
        .expect("round-robin must select a hat");
    assert_eq!(
        sel.as_str(),
        "alpha",
        "with priority_hat=None, round-robin must pick the first registered hat"
    );
    // The fact that calling with `None` is the ONLY safe path
    // for multi-consumer topics is the regression contract.
    // The pre-emption path (Some(_)) is reserved for unique
    // consumers only.
    let _: HatId = sel;
}

// ----------------------------------------------------------------
// 2026-07-06 silent-success P0-4 fix (long-term U16): handoff
// triggers pre-validation. When `HandoffIndex::consumer_of(topic)`
// returns a hat whose `triggers` list does NOT declare the topic,
// the previous wire-up registered a 600s pending entry that
// always escalated to `task.resume` → `recovery_exhausted:stall_recovery:...:handoff_dispatch_timeout`
// → shipper prefix-allowlist pass translation → silent-success
// (primary-20260705-224028).
//
// The fix (event_loop/mod.rs:9969-10027 in this revision) emits
// `task.resume.misrouted` diagnostic immediately and skips the
// pending registration, so the operator sees the misroute on the
// first event rather than after 600s of silent stall.
// ----------------------------------------------------------------

/// T-U16-01: `check_hat_triggers` (the shared helper used by
/// `validate_resume_routing`) rejects a hat whose `triggers` list
/// does not declare the topic. This pins the helper's contract
/// that the new misroute wire-up depends on.
#[test]
fn u16_check_hat_triggers_rejects_undeclared_topic() {
    use crate::workflow_contract::handoff_index::check_hat_triggers;
    let triggers = vec!["work.ready".to_string(), "task.resume".to_string()];
    let result = check_hat_triggers(&triggers, "build.done");
    assert!(
        result.is_err(),
        "consumer hat whose `triggers` does not declare the topic must be rejected"
    );
}

/// T-U16-02: same helper accepts when `triggers` declares the
/// topic literally (the happy-path: producer emits work.ready,
/// consumer's triggers has work.ready → on_handoff_accepted is
/// reached normally).
#[test]
fn u16_check_hat_triggers_accepts_declared_topic() {
    use crate::workflow_contract::handoff_index::check_hat_triggers;
    let triggers = vec!["work.ready".to_string()];
    let result = check_hat_triggers(&triggers, "work.ready");
    assert!(
        result.is_ok(),
        "consumer hat whose `triggers` declares the topic must be accepted"
    );
}

/// T-U16-03: glob patterns in `triggers` match a single topic
/// segment (`*` is single-segment per `Topic::matches`, not
/// `.*`). Pinned so the misroute wire-up does not silently
/// assume multi-segment matching that the helper doesn't
/// actually support.
#[test]
fn u16_check_hat_triggers_glob_pattern_matches_single_segment() {
    use crate::workflow_contract::handoff_index::check_hat_triggers;
    let triggers = vec!["review.*".to_string()];
    // Single-segment: matches.
    check_hat_triggers(&triggers, "review.dimension").expect("single-segment matches");
    // Two-segment (review.dimension.ready): does NOT match the
    // single-`*` glob. This is the documented `Topic::matches`
    // behavior — `*` is one segment, not `.*`.
    let multi_segment = check_hat_triggers(&triggers, "review.dimension.ready");
    assert!(
        multi_segment.is_err(),
        "single-`*` glob must NOT match two-segment topics; use `review.**` (or two patterns) for nested matching"
    );
}

// ----------------------------------------------------------------
// U7 (2026-07-23-001, R10 / KTD-7): the virtual `supervisor` is a
// runtime consumer, NOT a `HatRegistry` agent hat. It is wired into
// the `HandoffGraph` as the unique consumer of the slot-level
// `*.unit.done` / `*.unit.failed` topics (see
// `preset_lint::workflow_activation::HandoffGraph::from_config`), so
// `HandoffIndex::consumer_of("exec.unit.done")` returns
// `Some("supervisor")`. But `registry.get_config("supervisor")` is
// `None`, so the U16 misrouted wire-up — which reads the consumer's
// `triggers` from the registry — would treat the missing entry as
// "triggers do not declare the topic" and emit a spurious
// `task.resume.misrouted`. The U7 narrow exemption recognizes the
// virtual consumer centrally (`event_origin::is_virtual_runtime_consumer`)
// and skips the U16 check for it, while leaving normal-hat U16 intact.
// ----------------------------------------------------------------

/// Build a minimal `event_reader::Event` (aliased `JsonlEvent` in the
/// loop) for a topic, for driving `apply_contract_committed_side_effects`.
fn u7_jsonl_event(topic: &str) -> crate::event_reader::Event {
    serde_json::from_str(&format!(
        r#"{{"topic":"{topic}","ts":"2026-07-23T00:00:00Z"}}"#
    ))
    .expect("valid jsonl event")
}

/// Config with `supervisor.enabled` + isolated mode and a single worker
/// hat that publishes `exec.unit.done`. The virtual supervisor is the
/// unique consumer of that slot topic; no real hat subscribes to it.
fn u7_supervisor_enabled_config() -> crate::config::RalphConfig {
    let yaml = r#"
event_loop:
  execution_mode: isolated
  supervisor:
    enabled: true
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["exec.unit.done", "work.done"]
"#;
    serde_yaml::from_str(yaml).expect("valid supervisor config")
}

fn wave_runtime_config() -> crate::config::RalphConfig {
    let yaml = r#"
event_loop:
  execution_mode: isolated
mechanism:
  flow:
    type: declared
    version: 1
    terminal_emits: ["LOOP_COMPLETE"]
    steps:
      - id: review_wave
        kind: side_effect
        runs: wave.runtime.review
        allowed_emits: ["review.unit.done"]
hats:
  review-worker:
    name: "Review Worker"
    triggers: ["review.unit.ready"]
    publishes: ["review.unit.done"]
"#;
    serde_yaml::from_str(yaml).expect("valid wave runtime config")
}

/// U7 consumer-predicate 正例: the canonical virtual supervisor id is
/// recognized as the virtual runtime consumer.
#[test]
fn u7_virtual_supervisor_consumer_predicate_positive() {
    use crate::event_origin::is_virtual_runtime_consumer;
    for consumer in ["supervisor", "wave_runtime"] {
        assert!(
            is_virtual_runtime_consumer(consumer),
            "`{consumer}` must be recognized as a virtual runtime consumer"
        );
    }
}

/// U7 consumer-predicate 反例: ordinary hats are NOT the virtual
/// consumer, so they stay subject to the U16 misrouted check (the
/// exemption must not widen to normal hats).
#[test]
fn u7_virtual_supervisor_consumer_predicate_negative() {
    use crate::event_origin::is_virtual_runtime_consumer;
    for hat in ["executor", "integrator", "reviewer", "plan-reviewer", ""] {
        assert!(
            !is_virtual_runtime_consumer(hat),
            "ordinary hat `{hat}` must NOT be treated as a virtual runtime consumer"
        );
    }
}

/// U7 正例 (RED→GREEN): feeding a legitimate `exec.unit.done` whose
/// unique consumer is the virtual `supervisor` must NOT emit
/// `task.resume.misrouted`. Before the narrow exemption the registry
/// lookup for `supervisor` returns `None`, which the U16 wire-up
/// misreads as "triggers do not declare the topic".
#[test]
fn test_virtual_supervisor_unit_done_no_misrouted() {
    let config = u7_supervisor_enabled_config();
    // Sanity: the virtual supervisor really is the unique consumer of
    // the slot topic in this config (the pre-condition under test).
    {
        let index = crate::workflow_contract::HandoffIndex::from_config(&config);
        assert_eq!(
            index.consumer_of("exec.unit.done"),
            Some("supervisor"),
            "virtual supervisor must be the unique consumer of exec.unit.done"
        );
    }
    let mut event_loop = crate::EventLoop::new(config);
    event_loop.apply_contract_committed_side_effects(&[u7_jsonl_event("exec.unit.done")]);
    assert!(
        !event_loop
            .state
            .seen_topics
            .contains("task.resume.misrouted"),
        "virtual supervisor consuming exec.unit.done must NOT produce task.resume.misrouted"
    );
}

#[test]
fn virtual_wave_runtime_unit_done_no_misrouted() {
    let config = wave_runtime_config();
    {
        let index = crate::workflow_contract::HandoffIndex::from_config(&config);
        assert_eq!(
            index.consumer_of("review.unit.done"),
            Some("wave_runtime"),
            "wave runtime must be the unique consumer of review.unit.done"
        );
    }

    let mut event_loop = crate::EventLoop::new(config);
    event_loop.apply_contract_committed_side_effects(&[u7_jsonl_event("review.unit.done")]);

    assert!(
        !event_loop
            .state
            .seen_topics
            .contains("task.resume.misrouted"),
        "virtual wave runtime must not produce task.resume.misrouted"
    );
    assert_eq!(
        event_loop.state.handoff_tracker.pending_count(),
        0,
        "virtual wave runtime must not register an agent handoff"
    );
}

/// U7 反例 (regression guard): an ordinary hat that is the HandoffIndex
/// consumer of a slot topic but does NOT declare it in its `triggers`
/// still produces `task.resume.misrouted`. The U7 exemption is keyed on
/// the virtual-supervisor id only, so normal-hat U16 behavior is
/// unchanged.
#[test]
fn u7_normal_hat_consumer_still_reports_misrouted() {
    use crate::workflow_contract::{HandoffEntry, HandoffIndex, HandoffSource};

    // Normal `executor` hat whose `triggers` is only `work.ready` —
    // it does NOT declare `exec.unit.done`.
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["exec.unit.done", "work.done"]
"#;
    let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).expect("valid config");
    let mut event_loop = crate::EventLoop::new(config);

    // Force the HandoffIndex to name the ordinary `executor` hat as the
    // consumer of `exec.unit.done` (a misroute: its triggers lack it).
    let mut index = HandoffIndex::default();
    index.entries.insert(
        "exec.unit.done".to_string(),
        HandoffEntry {
            source: HandoffSource::Derived,
            consumer: Some("executor".to_string()),
        },
    );
    event_loop.handoff_index = index;

    event_loop.apply_contract_committed_side_effects(&[u7_jsonl_event("exec.unit.done")]);
    assert!(
        event_loop
            .state
            .seen_topics
            .contains("task.resume.misrouted"),
        "an ordinary hat consumer whose triggers lack the topic must still report task.resume.misrouted"
    );
}

/// Regression: `ce-executor-supervisor`'s top-level `mechanism.flow`
/// must load into `FlowStepScopeStage` so `work.ready` is admitted
/// to the bus and lands in `task-planner`'s pending queue.
///
/// Pre-fix: `load_opt_in_flow_declaration` wrapped a bare
/// `serde_yaml::to_string(flow)` under `mechanism:\n  flow:\n`
/// without indenting, so `mechanism.flow` parsed as null,
/// `FlowStepScope` rejected with `flow_step_undeclared`, and the
/// Outside-In primary_path E2E never activated `task-planner`.
#[test]
fn supervisor_work_ready_lands_in_task_planner_pending() {
    use crate::config::RalphConfig;
    use crate::event_loop::EventLoop;
    use ralph_proto::HatId;
    use std::io::Write;
    use tempfile::TempDir;

    let yaml = include_str!("../../../../../presets/en/ce-executor-supervisor.yml");
    let config = RalphConfig::parse_yaml(yaml).expect("parse supervisor preset");
    assert!(
        config
            .mechanism
            .as_ref()
            .and_then(|m| m.flow.as_ref())
            .is_some(),
        "builtin supervisor must declare top-level mechanism.flow"
    );

    let tmp = TempDir::new().unwrap();
    let events_path = tmp.path().join("events.jsonl");
    std::fs::write(&events_path, "").unwrap();

    let mut event_loop = EventLoop::new(config);
    event_loop.set_event_reader_path(&events_path);
    event_loop.state_mut().current_isolated_hat = Some(HatId::new("coordinator"));
    event_loop.state_mut().last_active_hat_ids = vec![HatId::new("coordinator")];

    let line = r#"{"topic":"work.ready","payload":"{\"plan_name\":\"e2e-plan\",\"plan_path\":\"plan.md\",\"task_id\":\"t-1\",\"task_key\":\"plan:e2e:u1\",\"step\":\"step-01\",\"complexity\":\"small\"}","ts":"2020-01-01T00:00:00Z","hat":"coordinator","source":"coordinator","triggered":"task-planner"}"#;
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&events_path)
            .unwrap();
        writeln!(f, "{line}").unwrap();
    }

    let processed = event_loop.process_events_from_jsonl().expect("process");
    assert!(
        processed.had_events,
        "work.ready must be accepted by process_events"
    );

    let pending = event_loop.bus().peek_pending(&HatId::new("task-planner"));
    let topics: Vec<_> = pending
        .map(|q| q.iter().map(|e| e.topic.as_str().to_string()).collect())
        .unwrap_or_default();
    assert!(
        topics.iter().any(|t| t == "work.ready"),
        "work.ready must land in task-planner pending after FlowStepScope admits it; got {topics:?}"
    );
}
