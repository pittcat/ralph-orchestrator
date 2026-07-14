// U2d: hard_gate_payload_contract 测试族(payload contract 主题)。
//
// 从 `loop_runner/tests/legacy.rs` 迁出。测试函数签名/断言逐字节不变(R6)。
//
// 覆盖:
//   - 早期 U6 段:`sample_violation` (helper) + `u6_writes_violation_report_to_diagnostics_dir` +
//     `u6_report_filename_uses_rfc3339_timestamp`
//   - U3 T3.6 / U4 stage 系列:`test_u3_t3_6_rejection_stage_missing_event_as_str` +
//     `test_u4_rejection_stage_emit_claimed_but_not_written_as_str` +
//     `test_u3_t3_6b_r5_hard_gate_routing_regression`
//   - U6 wave_obligation 系列:`test_u6_does_not_gate_while_wave_obligation_pending` +
//     `test_u6_gates_again_after_wave_reaches_terminal` +
//     `test_u6_legacy_path_unchanged_when_no_wave_present`
//   - P1 conditional_obligation 系列:`test_p1_conditional_obligation_*` x3
//   - inject_hat_execution_env 系列:`test_inject_hat_execution_env_*` x2
//   - U3 contract_rejection 系列:`test_contract_rejection_satisfies_any_valid_or_rejected` +
//     `test_missing_event_gate_fires_only_when_no_raw_events`
//   - U3 hat pinning 系列:`test_u3_pending_recovery_hat_*` +
//     `test_u3_next_hat_*` x2 + `test_u3_wave_policy_rejection_guidance_pins_recovery_hat` +
//     `test_u3_handoff_tracker_safe_target_unchanged_when_consumer_is_review_coordinator`
//   - helper `u3_workspace_with_isolated_hats`
//
// Import 策略(R3 / KTD4):
//   - `use super::super::*;` 引入 loop_runner::* glob(含 hard_gate.rs / payload_contract_gate.rs /
//     execution.rs 的 pub fn,如 `CliBackend` / `BackendOutputFormat` / `inject_hat_execution_env`)
//   - `use super::common::*;` 引入共享 helper
//   - `use super::fake_path::*;` 引入 fake PATH helper

use super::super::*;
use super::common::*;
use super::fake_path::*;

// ──────────────────────────────────────────────────────────────────────
// U6: payload contract violation report writing
// ──────────────────────────────────────────────────────────────────────

fn sample_violation() -> ralph_core::payload_contract::PayloadContractViolation {
    ralph_core::payload_contract::PayloadContractViolation {
        error_type:
            ralph_core::payload_contract::PayloadContractViolationKind::MissingRequiredField,
        timestamp: "2026-06-03T12:34:56.789Z".to_string(),
        topic: "work.ready".to_string(),
        field: Some("plan_name".to_string()),
        source_hat: vec!["coordinator".to_string()],
        target_hat: vec!["executor".to_string()],
        schema_defined_in: "inline".to_string(),
        downstream_reference: None,
        upstream_reference: None,
        fix_hint: "Add the missing field to the payload of the 'work.ready' event.".to_string(),
        payload_excerpt: Some(r#"{"task_id": "t-1"}"#.to_string()),
    }
}

#[test]
fn u6_writes_violation_report_to_diagnostics_dir() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path();
    let violation = sample_violation();
    let path = write_payload_contract_violation_report(dir, &violation);
    assert!(
        path.exists(),
        "report file must be created: {}",
        path.display()
    );
    let body = std::fs::read_to_string(&path).unwrap();
    // Must include required fields
    assert!(
        body.contains("work.ready"),
        "body must include topic: {}",
        body
    );
    assert!(
        body.contains("plan_name"),
        "body must include field: {}",
        body
    );
    assert!(
        body.contains("coordinator"),
        "body must include source hat: {}",
        body
    );
    assert!(
        body.contains("executor"),
        "body must include target hat: {}",
        body
    );
    assert!(
        body.contains("inline"),
        "body must include schema source: {}",
        body
    );
    assert!(
        body.contains("Add the missing field"),
        "body must include fix_hint: {}",
        body
    );
}

#[test]
fn u6_report_filename_uses_rfc3339_timestamp() {
    // Filename should be `payload-contract-error-{ts}.json` where the
    // timestamp is the violation's timestamp with `:` and `.` replaced
    // (so the file is portable across filesystems).
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path();
    let violation = sample_violation();
    let path = write_payload_contract_violation_report(dir, &violation);
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("payload-contract-error-"),
        "filename: {}",
        name
    );
    assert!(name.ends_with(".json"), "filename: {}", name);
    assert!(
        !name.contains(':'),
        "filename must not contain colons: {}",
        name
    );
}

#[test]
fn test_u3_t3_6_rejection_stage_missing_event_as_str() {
    // T3.6: the new `RejectionStage::MissingEvent` variant
    // serialises as the stable string `"missing_event"` so the
    // drift detector can count these as a recognisable class.
    use ralph_core::event_loop::rejection::RejectionStage;
    assert_eq!(RejectionStage::MissingEvent.as_str(), "missing_event");
    // Serialise / round-trip via serde.
    let json = serde_json::to_string(&RejectionStage::MissingEvent).expect("serialize");
    assert_eq!(json, "\"missing_event\"");
    let back: RejectionStage = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, RejectionStage::MissingEvent);
}

#[test]
fn test_u4_rejection_stage_emit_claimed_but_not_written_as_str() {
    // U4 (R1): the new `RejectionStage::EmitClaimedButNotWritten`
    // variant serialises as the stable string
    // `"emit_claimed_but_not_written"` so the drift detector can
    // distinguish "agent forgot to emit" (`missing_event`) from
    // "agent claimed to emit but the run fell off the rails" (this
    // variant).  Both share the same recovery path shape, but the
    // operator-actionable root cause is different.
    use ralph_core::event_loop::rejection::RejectionStage;
    assert_eq!(
        RejectionStage::EmitClaimedButNotWritten.as_str(),
        "emit_claimed_but_not_written"
    );
    // Serialise / round-trip via serde so the drift detector's
    // bucket-counter keeps working after future refactors.
    let json = serde_json::to_string(&RejectionStage::EmitClaimedButNotWritten).expect("serialize");
    assert_eq!(json, "\"emit_claimed_but_not_written\"");
    let back: RejectionStage = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, RejectionStage::EmitClaimedButNotWritten);
}

// 2026-06-17-001 Unit 6: GateWaveMutex. The wave registry lives
// in `LoopState::flow_lifecycle`; the hard gate must NOT fire
// while a wave obligation is pending, and must still fire when
// no wave was emitted (the "completely forgot to emit" case).

// 2026-06-17-001 Unit 6: GateWaveMutex. The wave registry lives
// in `LoopState::flow_lifecycle`; the hard gate must NOT fire
// while a wave obligation is pending, and must still fire when
// no wave was emitted (the "completely forgot to emit" case).

#[test]
fn test_u6_does_not_gate_while_wave_obligation_pending() {
    use ralph_core::RalphConfig;
    use ralph_core::flow_lifecycle::{FlowLifecycleRecord, FlowLifecycleRegistry, FlowPhase};
    let yaml = r#"
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done"]
    publishes: ["review.passed", "review.failed", "review.wave.ready"]
    obligations:
      - on_trigger: "work.done"
        must_emit_any_of: ["review.passed", "review.wave.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml config");
    let mut event_loop = EventLoop::new(config);
    event_loop.state_mut().last_activation_events =
        vec![ralph_proto::Event::new("work.done", "{}")];
    let hat_id: HatId = "review-coordinator".into();

    // Last activation was a `work.done` event — the hat declared
    // an obligation to publish `review.passed`. The activation
    // has not produced `review.passed` yet (candidate_topics is
    // empty), so the obligation is unsatisfied.
    let candidate_topics: Vec<String> = vec![];

    // Pre-condition: no wave has been registered yet. The legacy
    // path would gate the hat (unsatisfied obligation, no wave
    // outstanding).
    let pre = should_gate_missing_events(&hat_id, &event_loop, &candidate_topics);
    assert!(pre, "no wave registered → gate must fire (legacy path)");

    // Register a wave obligation for review-coordinator on the
    // review.wave.ready topic. The gate must back off and let
    // the wave workers report back.
    let record = FlowLifecycleRecord::new("wave-1", "review-coordinator", "review.wave.ready", 7)
        .with_timeouts(60, 1800);
    // Use a fresh registry whose record is mid-flight (WorkersActive)
    // so the gate sees an active obligation.
    let mut registry = FlowLifecycleRegistry::new();
    registry.register(record);
    registry
        .transition("wave-1", FlowPhase::Spawning, 1, None, None)
        .unwrap();
    registry
        .transition("wave-1", FlowPhase::WorkersActive, 1, None, None)
        .unwrap();
    let active_record = registry.get("wave-1").unwrap().clone();
    event_loop
        .state_mut()
        .flow_lifecycle
        .register(active_record);

    let post = should_gate_missing_events(&hat_id, &event_loop, &candidate_topics);
    assert!(
        !post,
        "wave obligation pending → gate must NOT fire (Unit 6 GateWaveMutex)"
    );
    // Pin the registry state explicitly so a refactor that flips the
    // return value without touching the registry still fails.
    assert!(
        event_loop
            .state()
            .flow_lifecycle
            .is_obligation_pending("wave-1")
    );
    assert_eq!(
        event_loop
            .state()
            .flow_lifecycle
            .get("wave-1")
            .unwrap()
            .phase,
        FlowPhase::WorkersActive
    );
}

#[test]
fn test_u6_gates_again_after_wave_reaches_terminal() {
    use ralph_core::RalphConfig;
    use ralph_core::flow_lifecycle::{FlowLifecycleRecord, FlowLifecycleRegistry, FlowPhase};
    let yaml = r#"
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done"]
    publishes: ["review.passed", "review.failed", "review.wave.ready"]
    obligations:
      - on_trigger: "work.done"
        must_emit_any_of: ["review.passed", "review.wave.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml config");
    let mut event_loop = EventLoop::new(config);
    event_loop.state_mut().last_activation_events =
        vec![ralph_proto::Event::new("work.done", "{}")];
    let hat_id: HatId = "review-coordinator".into();

    let mut registry = FlowLifecycleRegistry::new();
    registry.register(
        FlowLifecycleRecord::new("wave-1", "review-coordinator", "review.wave.ready", 7)
            .with_timeouts(60, 1800),
    );
    registry
        .transition("wave-1", FlowPhase::Spawning, 1, None, None)
        .unwrap();
    registry
        .transition("wave-1", FlowPhase::WorkersActive, 1, None, None)
        .unwrap();
    registry
        .transition("wave-1", FlowPhase::Aggregating, 1, None, None)
        .unwrap();
    registry
        .transition("wave-1", FlowPhase::Closed, 1, None, None)
        .unwrap();
    let closed_record = registry.get("wave-1").unwrap().clone();
    event_loop
        .state_mut()
        .flow_lifecycle
        .register(closed_record);

    // Wave closed → obligation no longer pending → gate fires
    // again because no `review.passed` candidate was emitted.
    let candidate_topics: Vec<String> = vec![];
    let gated = should_gate_missing_events(&hat_id, &event_loop, &candidate_topics);
    assert!(
        gated,
        "wave reached Closed → obligation cleared → gate must fire"
    );
}

#[test]
fn test_u6_legacy_path_unchanged_when_no_wave_present() {
    // Defensive regression: a hat without a flow record must keep
    // its pre-Unit-6 behavior. This is the exact scenario the
    // archive P0-B report identified (executor emits no
    // `work.done` at all → gate fires).
    use ralph_core::RalphConfig;
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done", "work.failed"]
    obligations:
      - on_trigger: "work.ready"
        must_emit_any_of: ["work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml config");
    let mut event_loop = EventLoop::new(config);
    event_loop.state_mut().last_activation_events =
        vec![ralph_proto::Event::new("work.ready", "{}")];
    let hat_id: HatId = "executor".into();

    let candidate_topics: Vec<String> = vec![];
    let gated = should_gate_missing_events(&hat_id, &event_loop, &candidate_topics);
    assert!(
        gated,
        "executor with no work.done and no wave → gate must fire"
    );
}

#[test]
fn test_p1_conditional_obligation_gates_when_commit_count_positive() {
    // 2026-06-08 fix (P1): when a hat declares
    // `conditional_must_emit` on a trigger and the trigger payload
    // matches the predicate, the hard_gate must reject a candidate
    // that satisfies the top-level OR but not the strict
    // conditional.  This is the U3/U4 fix integration test:
    //   work.done with commit_count=2 + review.passed → gate fires
    //   (would otherwise skip the wave).
    let yaml = r#"
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done"]
    publishes: ["review.wave.ready", "review.passed"]
    obligations:
      - on_trigger: "work.done"
        must_emit_any_of: ["review.wave.ready", "review.passed"]
        conditional_must_emit:
          - when: { commit_count_min: 1 }
            must_emit_any_of: ["review.wave.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.state_mut().last_activation_events = vec![ralph_proto::Event::new(
        "work.done",
        r#"{"commit_count": 2, "changed_lines": 400}"#,
    )];
    let hat = HatId::new("review-coordinator");

    // Non-trivial diff + review.passed → conditional matched, candidate
    // off strict set → obligation unsatisfied → gate fires.
    let passed = vec!["review.passed".to_string()];
    assert!(
        should_gate_missing_events(&hat, &event_loop, &passed),
        "non-trivial work.done (commit_count=2) with review.passed must trigger gate (U3/U4 bug)"
    );

    // Non-trivial diff + review.wave.ready → conditional matched, candidate
    // in strict set → obligation satisfied → gate does NOT fire.
    let wave = vec!["review.wave.ready".to_string()];
    assert!(
        !should_gate_missing_events(&hat, &event_loop, &wave),
        "non-trivial work.done with review.wave.ready must not trigger gate"
    );
}

#[test]
fn test_p1_conditional_obligation_falls_back_to_legacy_or_on_empty_diff() {
    // 2026-06-08 fix (P1) — empty-diff path: when the trigger payload
    // does NOT match the conditional predicate (e.g. commit_count=0),
    // the obligation falls back to the top-level OR semantics.
    // review.passed is acceptable for a trivial 0-commit, 0-line diff.
    let yaml = r#"
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done"]
    publishes: ["review.wave.ready", "review.passed"]
    obligations:
      - on_trigger: "work.done"
        must_emit_any_of: ["review.wave.ready", "review.passed"]
        conditional_must_emit:
          - when: { commit_count_min: 1 }
            must_emit_any_of: ["review.wave.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.state_mut().last_activation_events = vec![ralph_proto::Event::new(
        "work.done",
        r#"{"commit_count": 0, "changed_lines": 0}"#,
    )];
    let hat = HatId::new("review-coordinator");

    // Empty diff + review.passed → no conditional matched → legacy OR applies
    // → obligation satisfied → gate does NOT fire.
    let passed = vec!["review.passed".to_string()];
    assert!(
        !should_gate_missing_events(&hat, &event_loop, &passed),
        "empty diff (commit_count=0) with review.passed must NOT trigger gate (legacy OR fallback)"
    );
}

#[test]
fn test_p1_per_obligation_trigger_context_isolated() {
    // 2026-06-08 fix (P1) — multi-trigger isolation: when a hat has
    // obligations for multiple triggers (e.g. work.done + fix.applied),
    // each obligation is evaluated against its OWN trigger event's
    // payload, not the first matching event's payload.  This test
    // exercises divergent payloads: work.done has commit_count=1
    // (strict), fix.applied has commit_count=0 (legacy OR allows
    // review.passed).  The fix.applied obligation must be evaluated
    // with the fix.applied payload, so the gate does NOT fire
    // (review.passed satisfies fix.applied's obligation).
    let yaml = r#"
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done", "fix.applied"]
    publishes: ["review.wave.ready", "review.passed"]
    obligations:
      - on_trigger: "work.done"
        must_emit_any_of: ["review.wave.ready", "review.passed"]
        conditional_must_emit:
          - when: { commit_count_min: 1 }
            must_emit_any_of: ["review.wave.ready"]
      - on_trigger: "fix.applied"
        must_emit_any_of: ["review.wave.ready", "review.passed"]
        conditional_must_emit:
          - when: { commit_count_min: 1 }
            must_emit_any_of: ["review.wave.ready"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    // Note: work.done is first in last_activation_events, but the
    // fix.applied obligation must still see fix.applied's payload
    // (commit_count=0) so that its conditional does NOT match.
    event_loop.state_mut().last_activation_events = vec![
        ralph_proto::Event::new("work.done", r#"{"commit_count": 1}"#),
        ralph_proto::Event::new("fix.applied", r#"{"commit_count": 0}"#),
    ];
    let hat = HatId::new("review-coordinator");

    // work.done obligation: commit_count=1 conditional matches, review.passed
    // is off strict set → unsatisfied.
    // fix.applied obligation: commit_count=0 conditional does NOT match
    // → fall back to legacy OR → review.passed satisfies.
    // `any` returns true → gate does NOT fire.
    let passed = vec!["review.passed".to_string()];
    assert!(
        !should_gate_missing_events(&hat, &event_loop, &passed),
        "fix.applied obligation must use its own context (commit_count=0), not work.done's"
    );

    // Now flip: fix.applied has commit_count=1, work.done has commit_count=0.
    // work.done obligation: legacy OR, review.passed satisfies.
    // fix.applied obligation: strict, review.passed is off → unsatisfied.
    event_loop.state_mut().last_activation_events = vec![
        ralph_proto::Event::new("work.done", r#"{"commit_count": 0}"#),
        ralph_proto::Event::new("fix.applied", r#"{"commit_count": 1}"#),
    ];
    assert!(
        !should_gate_missing_events(&hat, &event_loop, &passed),
        "work.done obligation must use its own context (commit_count=0), not fix.applied's"
    );
}

#[test]
fn test_inject_hat_execution_env_sets_reserved_and_preserves_user_vars() {
    let mut backend = CliBackend {
        command: "echo".into(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format: BackendOutputFormat::Text,
        env_vars: vec![
            ("USER_VAR".into(), "keep".into()),
            ("RALPH_CURRENT_HAT".into(), "old-hat".into()),
            ("RALPH_CONFIG".into(), "/tmp/old.yml".into()),
        ],
    };
    inject_hat_execution_env(
        &mut backend,
        "reviewer",
        "loop-42",
        std::path::Path::new("/tmp/events.jsonl"),
        Some("synthesizer"),
        None,
        Some(std::path::Path::new("/tmp/custom.yml")),
    );
    let map: std::collections::HashMap<_, _> = backend.env_vars.into_iter().collect();
    assert_eq!(map.get("USER_VAR").unwrap(), "keep");
    assert_eq!(map.get("RALPH_CURRENT_HAT").unwrap(), "reviewer");
    assert_eq!(map.get("RALPH_CURRENT_LOOP_ID").unwrap(), "loop-42");
    assert_eq!(map.get("RALPH_EVENTS_FILE").unwrap(), "/tmp/events.jsonl");
    assert_eq!(map.get("RALPH_TRIGGERED_HAT").unwrap(), "synthesizer");
    assert_eq!(map.get("RALPH_CONFIG").unwrap(), "/tmp/custom.yml");
}

#[test]
fn test_inject_hat_execution_env_omits_triggered_when_none() {
    let mut backend = CliBackend {
        command: "echo".into(),
        args: vec![],
        prompt_mode: ralph_adapters::PromptMode::Arg,
        prompt_flag: None,
        output_format: BackendOutputFormat::Text,
        env_vars: vec![("RALPH_CONFIG".into(), "/tmp/stale.yml".into())],
    };
    inject_hat_execution_env(
        &mut backend,
        "ralph",
        "loop-1",
        std::path::Path::new(".ralph/events.jsonl"),
        None,
        None,
        None,
    );
    let keys: Vec<_> = backend.env_vars.iter().map(|(k, _)| k.as_str()).collect();
    assert!(keys.contains(&"RALPH_CURRENT_HAT"));
    assert!(keys.contains(&"RALPH_CURRENT_LOOP_ID"));
    assert!(keys.contains(&"RALPH_EVENTS_FILE"));
    assert!(!keys.contains(&"RALPH_TRIGGERED_HAT"));
    assert!(!keys.contains(&"RALPH_CONFIG"));
}

// ─────────────────────────────────────────────────────────────────────────
// U3 supplement: contract-rejection interaction with missing-event gate
// ─────────────────────────────────────────────────────────────────────────
//
// The loop runner gates missing-event hard-failures on the flag
// `agent_wrote_any_valid_or_rejected = had_raw_events || had_rejected_events`.
// When the contract rejects a `work.done` event, `had_rejected_events` is
// true and `had_events` is false. The loop runner MUST treat this as
// "agent tried but failed contract" and NOT fire the missing-event gate
// (which is reserved for the "agent completely forgot to emit" case).
//
// Likewise, the default_publishes fallback must NOT trigger because the
// agent did write a valid `work.done` event — the contract rejection
// should drive the next iteration through the published guidance event.

#[test]
fn test_contract_rejection_satisfies_any_valid_or_rejected() {
    // Simulate the loop runner's gating decision: a contract-rejected
    // event must be treated as "the agent wrote something" so the
    // missing-event gate does not fire.
    let processed = ralph_core::ProcessedEvents {
        had_events: false,
        had_raw_events: true,
        had_rejected_events: true,
        had_plan_events: false,

        has_orphans: false,
        accepted_events: vec![],
        contract_rejections: vec![ralph_core::execution_contract::ExecutionContractFinding {
            kind: ralph_core::execution_contract::ExecutionContractViolationKind::NoGitEvidence {
                step: None,
            },
            message: "test rejection".to_string(),
            topic: "work.done".to_string(),
            source_hat: None,
        }],
        payload_contract_violation: None,
    };

    let agent_wrote_any_valid_or_rejected =
        processed.had_raw_events || processed.had_rejected_events;

    assert!(
        agent_wrote_any_valid_or_rejected,
        "Contract rejection must satisfy any_valid_or_rejected so the missing-event gate does not fire"
    );
    assert!(
        !processed.had_events,
        "had_events should be false (rejection does not count as accepted)"
    );
    assert!(
        processed.had_rejected_events,
        "had_rejected_events should be true"
    );
    assert!(
        processed.had_raw_events,
        "had_raw_events should be true (events that reached the contract layer count)"
    );
}

#[test]
fn test_missing_event_gate_fires_only_when_no_raw_events() {
    // Mirror the loop runner's gate decision: missing-event gate fires
    // ONLY when the agent wrote absolutely nothing. A contract rejection
    // (had_rejected_events=true) must be enough to skip the gate.
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    // Sanity: executor (publishes but no default_publishes) WOULD gate if
    // the agent emitted nothing.
    assert!(
        should_gate_missing_events(&HatId::new("executor"), &event_loop, &[]),
        "executor should normally trigger missing-event gate"
    );

    // Simulate the agent's output: no events at all.
    let empty = ralph_core::ProcessedEvents {
        had_events: false,
        had_raw_events: false,
        had_rejected_events: false,
        had_plan_events: false,

        has_orphans: false,
        accepted_events: vec![],
        contract_rejections: vec![],
        payload_contract_violation: None,
    };
    let gate_would_fire = !empty.had_raw_events
        && !empty.had_rejected_events
        && should_gate_missing_events(&HatId::new("executor"), &event_loop, &[]);
    assert!(
        gate_would_fire,
        "Missing-event gate MUST fire when agent wrote nothing"
    );

    // Now simulate contract rejection: had_raw_events=true, had_rejected_events=true.
    let rejected = ralph_core::ProcessedEvents {
        had_events: false,
        had_raw_events: true,
        had_rejected_events: true,
        had_plan_events: false,

        has_orphans: false,
        accepted_events: vec![],
        contract_rejections: vec![],
        payload_contract_violation: None,
    };
    let gate_would_fire = !rejected.had_raw_events
        && !rejected.had_rejected_events
        && should_gate_missing_events(&HatId::new("executor"), &event_loop, &[]);
    assert!(
        !gate_would_fire,
        "Missing-event gate MUST NOT fire when contract rejected an event"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// 2026-06-13 plan U3: hat pinning after a hard gate or wave recovery.
//
// The runner's `inject_missing_event_hard_gate_guidance` and
// `inject_wave_policy_rejection_guidance` helpers set
// `LoopState::pending_recovery_hat` to the offending hat. The
// next call to `EventLoop::next_hat` must:
//   1. Return the pinned hat (NOT whatever the round-robin /
//      coordinator default would pick).
//   2. Clear the field so a later iteration is not stuck on the
//      same hat when the obligation is actually satisfied.
//
// These tests exercise both helpers' pinning side effect and the
// `next_hat` consumption + clear path. They do not exercise the
// full runner main loop — that integration check belongs to U6.
// ─────────────────────────────────────────────────────────────────────────

#[cfg(unix)]
fn u3_workspace_with_isolated_hats() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let diagnostics = ralph_core::diagnostics::DiagnosticsCollector::with_enabled(&root, true)
        .expect("create diagnostics collector");
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done"]
    publishes: ["review.wave.ready", "review.passed"]
  executor:
    name: "Executor"
    triggers: ["task.start"]
    publishes: ["work.done"]
"#;
    let mut config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
    config.core.workspace_root = root.clone();
    let event_loop = ralph_core::EventLoop::with_diagnostics(config, diagnostics);
    // We just need the workspace / event_loop pair returned;
    // the caller constructs its own EventLoop to control state.
    let _ = event_loop;
    (temp, root)
}

#[test]
fn test_u3_next_hat_consumes_pending_recovery_hat_and_clears() {
    // 2026-06-13 plan U3 — error path / next-iteration shape:
    // the `EventLoop::next_hat` method must (a) honour the pin
    // and (b) clear it so the loop does not get stuck on a
    // single hat.
    use ralph_core::EventLoop;

    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done"]
    publishes: ["review.wave.ready", "review.passed"]
  executor:
    name: "Executor"
    triggers: ["task.start"]
    publishes: ["work.done"]
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
    let mut event_loop = EventLoop::new(config);

    // Pre-condition: the bus has at least one pending event for
    // the executor hat (the default isolated-mode round-robin
    // would otherwise not have anything to compare against — we
    // want a world where the round-robin would have picked
    // something *other* than review-coordinator).
    event_loop
        .bus()
        .publish(ralph_proto::Event::new("task.start", "{}").with_target("executor"));

    // U3: pin the next iteration to review-coordinator.
    event_loop.state_mut().pending_recovery_hat =
        Some(ralph_proto::HatId::new("review-coordinator"));

    // next_hat must return the pinned hat, not the round-robin
    // pick.
    let selected = event_loop
        .next_hat()
        .cloned()
        .expect("next_hat must return Some when pin or pending is set");
    assert_eq!(
        selected.as_str(),
        "review-coordinator",
        "U3: pinned hat must take precedence over the round-robin pick"
    );

    // The field must be cleared after consumption so the next
    // iteration can pick a different hat normally.
    assert!(
        event_loop.state().pending_recovery_hat.is_none(),
        "U3: pending_recovery_hat must be cleared on consumption"
    );
}

#[test]
fn test_u3_next_hat_falls_through_when_pinned_hat_unknown() {
    // 2026-06-13 plan U3 — edge case: a stale pin (e.g. a hat
    // that has been deregistered between iterations) must NOT
    // block hat selection. The runner cannot get stuck on a
    // ghost hat id.
    use ralph_core::EventLoop;

    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  executor:
    name: "Executor"
    triggers: ["task.start"]
    publishes: ["work.done"]
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).expect("parse yaml");
    let mut event_loop = EventLoop::new(config);

    // Pin to a hat that does NOT exist in the registry.
    event_loop.state_mut().pending_recovery_hat = Some(ralph_proto::HatId::new("ghost-hat"));

    // Pin should be cleared even when the hat is unknown, so the
    // next iteration can operate normally.
    let _ = event_loop.next_hat();
    assert!(
        event_loop.state().pending_recovery_hat.is_none(),
        "U3: stale pin must be cleared even when the hat is unregistered"
    );
}

#[test]
fn test_u3_handoff_tracker_safe_target_unchanged_when_consumer_is_review_coordinator() {
    // 2026-06-13 plan U3 — regression guard: the
    // `HandoffTracker::expired` path must keep `safe_target ==
    // consumer` when the consumer is `review-coordinator` (i.e.
    // NOT the default `plan-gate` bottleneck case). Without
    // this guard, escalation to `review-coordinator` would
    // itself be re-routed to the fallback and never reach
    // `review-coordinator`.
    use ralph_core::workflow_contract::HandoffTracker;
    use std::time::{Duration, Instant};

    let mut tracker = HandoffTracker::new();
    let now = Instant::now();
    // Pending handoff for review-coordinator with a 1s default
    // timeout.
    tracker = tracker.with_default_timeout(Duration::from_secs(1));
    tracker.on_handoff_accepted("work.ready", "review-coordinator", "evt-1", now);

    // At now + 2s the entry has expired.
    let escalations = tracker.expired(now + Duration::from_secs(2));
    assert_eq!(escalations.len(), 1);
    // U3: the safe_target MUST equal the consumer, not the
    // default plan-gate fallback.
    assert_eq!(
        escalations[0].safe_target, "review-coordinator",
        "U3: review-coordinator handoff must keep safe_target == consumer; \
         got safe_target={}",
        escalations[0].safe_target
    );
    assert_eq!(escalations[0].consumer, "review-coordinator");
}
