// U2d: hard_gate 测试族(行为主题)。
//
// 从 `loop_runner/tests/legacy.rs` 迁出。测试函数签名/断言逐字节不变(R6)。
//
// 覆盖:
//   - 早期 U5 段:`hard_gate_passes_when_no_hats` / `hard_gate_passes_when_contracts_covered` /
//     `hard_gate_fails_when_field_missing_from_schema` / `hard_gate_fails_when_schema_missing_in_strict_mode` /
//     `hard_gate_message_is_actionable`
//   - 早期 U0 characterization:`u0_hard_gate_error_includes_source_hats` /
//     `u0_hard_gate_runs_independent_of_preflight_toggle` / `u0_hard_gate_solo_mode_is_pass_through`
//   - `test_should_hard_gate` / `test_missing_event_hard_gate`
//   - `test_u4_obligation_path_gates_when_no_candidate_topics`
//   - U2 grace window 系列:`test_u2_t2_1` ~ `test_u2_t2_5` + `test_u2_resolve_missing_event_grace_secs_helper`
//   - U3 replay / inject 系列:`test_u3_t3_1` ~ `test_u3_t3_4`
//   - U4 hard_gate_guidance 系列:`test_u4_t4_1` ~ `test_u4_t4_4`
//   - `test_u3_t3_5_enrich_with_stage_helper`
//
// Import 策略(R3 / KTD4):
//   - `use super::super::*;` 引入 loop_runner::* glob(含 hard_gate.rs / payload_contract_gate.rs 的 pub fn)
//   - `use super::common::*;` 引入共享 helper
//   - `use super::fake_path::*;` 引入 fake PATH helper

use super::super::*;
use super::common::*;
use super::fake_path::*;

// ──────────────────────────────────────────────────────────────────────
// 本地 helper: `u4_workspace` 在原 `legacy.rs` 中定义,被 `test_u4_t4_3` /
// `test_u4_t4_4` 调用。U2d 严格原子性约束禁止修改其他测试文件,故在此
// 复制一份同样签名的本地版本(实现与 legacy.rs 中等价)。`#[cfg(unix)]`
// 保留以匹配原 hard_gate.rs 用例的编译条件。
// ──────────────────────────────────────────────────────────────────────

#[cfg(unix)]
fn u4_workspace() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let root = temp.path().to_path_buf();
    (temp, root)
}

// ──────────────────────────────────────────────────────────────────────
// U5: payload contract hard gate
// ──────────────────────────────────────────────────────────────────────

#[test]
fn hard_gate_passes_when_no_hats() {
    // Hatless / solo mode: no contract to validate → pass.
    let config = ralph_core::RalphConfig::default();
    let result = enforce_payload_contract_gate(&config);
    assert!(result.is_ok(), "Hatless mode should pass: {:?}", result);
}

#[test]
fn hard_gate_passes_when_contracts_covered() {
    // All required fields are in the schema → pass.
    let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: observe
    schemas:
      work.ready:
        required_fields: ["task_id"]
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let result = enforce_payload_contract_gate(&config);
    assert!(
        result.is_ok(),
        "Covered contracts should pass: {:?}",
        result
    );
}

#[test]
fn hard_gate_fails_when_field_missing_from_schema() {
    // `plan_name` is referenced but not in required_fields → fatal error.
    let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: observe
    schemas:
      work.ready:
        required_fields: ["task_id"]
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let err = enforce_payload_contract_gate(&config).unwrap_err();
    let msg = format!("{}", err);
    assert!(msg.contains("Payload contract gate failed"), "msg: {}", msg);
    assert!(msg.contains("plan_name"), "msg must mention field: {}", msg);
    assert!(
        msg.contains("work.ready"),
        "msg must mention topic: {}",
        msg
    );
    assert!(
        msg.contains("FieldMissingFromSchema"),
        "msg must include kind: {}",
        msg
    );
}

#[test]
fn hard_gate_fails_when_schema_missing_in_strict_mode() {
    // Trigger topic has no schema → strict mode treats it as an error.
    let yaml = r#"
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let err = enforce_payload_contract_gate(&config).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("SchemaMissingForRequiredTopic"),
        "msg must mention kind: {}",
        msg
    );
}

#[test]
fn hard_gate_message_is_actionable() {
    // Error message must list all errors, mention source hats, schema
    // source, and provide a fix hint.
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: observe
    schemas:
      work.ready:
        required_fields: ["task_id"]
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let err = enforce_payload_contract_gate(&config).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("coordinator"),
        "msg must list source hat: {}",
        msg
    );
    assert!(msg.contains("Fix by"), "msg must include fix hint: {}", msg);
    assert!(
        msg.contains("event_policy.schemas"),
        "msg must point to fix location: {}",
        msg
    );
}

// ──────────────────────────────────────────────────────────────────────
// U0 characterization: lock in current `enforce_payload_contract_gate`
// behavior so U1/U2 shared contract layer cannot silently change the
// hard-gate semantics. The hard gate is a *non-skippable* invariant.
// ──────────────────────────────────────────────────────────────────────

/// U0 characterization: the hard gate error must list the source hats
/// (upstream publishers) of the offending trigger topic. This is critical
/// for users to debug "which hat is the upstream producer of the bad
/// field?" without running `ralph hats validate` separately.
///
/// **Why structural assertions on `validate_payload_contract`**: the
/// formatted `enforce_payload_contract_gate` error embeds source hats as
/// `source_hats=[<id>, <id>]` in a multi-line human-readable message.
/// Asserting on the literal `source_hats` label or the joined hat list
/// inside that string is brittle: any future refactor that promotes
/// `source_hats` to a structured `RuntimeContractFinding.details` field
/// (planned for U1/U2) would silently leave the label inside a JSON key
/// (e.g. `"source_hats": [...]`) and the test would pass for the wrong
/// reason. To pin the contract semantically, this test calls
/// `validate_payload_contract` directly and asserts on the structured
/// `PayloadContractError.source_hats` field. The user-facing message
/// is still exercised once, against the consumer-hat label, to backstop
/// the `enforce_payload_contract_gate` code path.
#[test]
fn u0_hard_gate_error_includes_source_hats() {
    // Two hats publish work.ready. The error must list BOTH in source_hats.
    let yaml = r#"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  alternate:
    name: "Alternate"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Also publish."
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: observe
    schemas:
      work.ready:
        required_fields: ["task_id"]
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let registry = ralph_core::HatRegistry::from_runtime_config(&config);

    // Structural path: invoke the validator directly so the test asserts on
    // the typed `source_hats` field, not on a substring of the formatted
    // error message.
    let result = ralph_core::payload_contract::validate_payload_contract(&config, &registry, true);
    assert!(
        !result.is_valid(),
        "fixture must produce a payload contract error: {:?}",
        result
    );
    let err = result
        .errors
        .iter()
        .find(|e| {
            e.hat_id == "executor"
                && e.topic == "work.ready"
                && e.field.as_deref() == Some("plan_name")
        })
        .expect("expected FieldMissingFromSchema error for executor/work.ready/plan_name");
    // source_hats must structurally include both upstream publishers.
    assert!(
        err.source_hats.contains(&"coordinator".to_string()),
        "source_hats must include 'coordinator': {:?}",
        err.source_hats
    );
    assert!(
        err.source_hats.contains(&"alternate".to_string()),
        "source_hats must include 'alternate': {:?}",
        err.source_hats
    );

    // Formatted-message backstop: the user-facing error from
    // `enforce_payload_contract_gate` must still surface the consumer hat
    // via the `hat=<id>` label. This guards the hard-gate code path
    // independently from the structured field above.
    let gate_err = enforce_payload_contract_gate(&config).unwrap_err();
    let msg = format!("{}", gate_err);
    assert!(
        msg.contains("hat=executor"),
        "msg must identify the consumer hat via the 'hat=<id>' label ('executor'): {}",
        msg
    );
}

/// U0 characterization: `enforce_payload_contract_gate` is independent of
/// `features.preflight.enabled` and `--skip-preflight`. Even if the user
/// has preflight disabled, the payload hard gate MUST still run before
/// backend spawn. This is a non-regression invariant: the gate is
/// intentionally not coupled to the preflight toggle.
#[test]
fn u0_hard_gate_runs_independent_of_preflight_toggle() {
    // Construct a config with a payload contract violation (plan_name
    // missing from required_fields). Pre-flight is disabled at the
    // config level. The hard gate must still fail.
    let yaml = r#"
features:
  preflight:
    enabled: false
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "Publish."
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["LOOP_COMPLETE"]
    instructions: |
      From event payload: task_id, plan_name
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: observe
    schemas:
      work.ready:
        required_fields: ["task_id"]
"#;
    let config: ralph_core::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    // Sanity: preflight is disabled in this config.
    assert!(
        !config.features.preflight.enabled,
        "test fixture must have preflight disabled"
    );
    // The hard gate must still fire.
    let err = enforce_payload_contract_gate(&config)
        .expect_err("hard gate must fire even when preflight.enabled=false");
    let msg = format!("{}", err);
    assert!(
        msg.contains("Payload contract gate failed"),
        "msg must indicate hard-gate failure regardless of preflight: {}",
        msg
    );
    assert!(
        msg.contains("plan_name"),
        "msg must name the offending field: {}",
        msg
    );
}

/// U0 characterization: hatless / solo mode (no custom hats) is the
/// pass-through. There is nothing to validate, so the hard gate must
/// succeed — even if preflight is otherwise disabled. This locks in the
/// baseline behavior so adding a runtime contract layer doesn't
/// accidentally start failing solo runs.
#[test]
fn u0_hard_gate_solo_mode_is_pass_through() {
    let mut config = ralph_core::RalphConfig::default();
    config.features.preflight.enabled = false;
    assert!(config.hats.is_empty(), "default config has no custom hats");
    let result = enforce_payload_contract_gate(&config);
    assert!(
        result.is_ok(),
        "hatless/solo mode must pass through the hard gate: {:?}",
        result
    );
}

#[test]
fn test_should_hard_gate() {
    let yaml = r#"
hats:
  reviewer:
    name: "Reviewer"
    publishes: ["review.passed"]
  coordinator:
    name: "Coordinator"
    publishes: ["work.ready"]
    default_publishes: "work.failed"
  silent:
    name: "Silent"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    assert!(
        should_hard_gate(&HatId::new("reviewer"), &event_loop),
        "hat with publishes and no default_publishes should hard gate"
    );
    assert!(
        !should_hard_gate(&HatId::new("coordinator"), &event_loop),
        "hat with default_publishes should NOT hard gate"
    );
    assert!(
        !should_hard_gate(&HatId::new("silent"), &event_loop),
        "hat with no publishes should NOT hard gate"
    );
    assert!(
        !should_hard_gate(&HatId::new("nonexistent"), &event_loop),
        "unknown hat should NOT hard gate"
    );
}

#[test]
fn test_missing_event_hard_gate() {
    // U1: Tests for should_gate_missing_events which catches the "completely forgot to emit" case
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    publishes: ["work.done", "work.failed"]
  reviewer:
    name: "Reviewer"
    publishes: ["review.passed"]
    default_publishes: "review.done"
  gate:
    name: "Gate"
    publishes: ["plan.blocked"]
    default_publishes: "plan.blocked"
  silent:
    name: "Silent"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let event_loop = EventLoop::new(config);

    // U4 (2026-06-07): should_gate_missing_events now takes the
    // candidate topic set so the activation-level obligation path can
    // distinguish "no event at all" from "agent emitted a topic
    // outside the obligation set".  Legacy hats without obligations
    // ignore the candidate list and follow the blanket rule.
    let no_candidates: Vec<String> = Vec::new();

    // Executor with publishes but no default_publishes -> should gate on missing events
    assert!(
        should_gate_missing_events(&HatId::new("executor"), &event_loop, &no_candidates),
        "executor with publishes and no default_publishes should gate missing events"
    );
    // Reviewer with default_publishes -> should NOT gate (has fallback)
    assert!(
        !should_gate_missing_events(&HatId::new("reviewer"), &event_loop, &no_candidates),
        "hat with default_publishes should NOT gate missing events"
    );
    // Gate with default_publishes (fail-closed) -> should NOT gate
    assert!(
        !should_gate_missing_events(&HatId::new("gate"), &event_loop, &no_candidates),
        "gate with default_publishes should NOT gate missing events"
    );
    // Silent hat with no publishes -> should NOT gate
    assert!(
        !should_gate_missing_events(&HatId::new("silent"), &event_loop, &no_candidates),
        "hat with no publishes should NOT gate missing events"
    );
    // Unknown hat -> should NOT gate
    assert!(
        !should_gate_missing_events(&HatId::new("nonexistent"), &event_loop, &no_candidates),
        "unknown hat should NOT gate missing events"
    );
}

#[test]
fn test_u4_obligation_path_gates_when_no_candidate_topics() {
    // U4 (2026-06-07): hats with explicit `obligations:` now go
    // through the activation-level path.  When the candidate topic
    // set is empty, the obligation is unsatisfied and the gate
    // MUST fire — the previous behaviour was to silently never gate,
    // which left the loop hanging when such a hat forgot to emit.
    let yaml = r#"
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done"]
    publishes: ["review.wave.ready", "review.passed"]
    obligations:
      - on_trigger: "work.done"
        must_emit_any_of: ["review.wave.ready", "review.passed"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.state_mut().last_activation_events =
        vec![ralph_proto::Event::new("work.done", "{}")];
    let hat = HatId::new("review-coordinator");

    // Empty candidates → obligation unsatisfied → gate fires.
    let empty: Vec<String> = Vec::new();
    assert!(
        should_gate_missing_events(&hat, &event_loop, &empty),
        "obligation-equipped hat with no candidates must trigger missing-event gate"
    );

    // Off-obligation candidates → obligation unsatisfied → gate fires.
    let off_obligation = vec!["work.failed".to_string()];
    assert!(
        should_gate_missing_events(&hat, &event_loop, &off_obligation),
        "off-obligation candidate must not satisfy the obligation"
    );

    // On-obligation candidates → obligation satisfied → gate does NOT fire.
    let on_obligation_wave = vec!["review.wave.ready".to_string()];
    assert!(
        !should_gate_missing_events(&hat, &event_loop, &on_obligation_wave),
        "matching candidate must satisfy the obligation"
    );
    let on_obligation_passed = vec!["review.passed".to_string()];
    assert!(
        !should_gate_missing_events(&hat, &event_loop, &on_obligation_passed),
        "second obligation branch must also satisfy"
    );
}

// 2026-06-17-004 U2 (R3): HatActivationClock + missing-event
// gate grace window.  These tests pin the new behaviour
// (T2.1-T2.5 in the plan).

#[test]
fn test_u2_t2_1_grace_within_window_suppresses_gate() {
    // T2.1: when the hat has been activated within the grace
    // window and produced no event, the gate must NOT fire.
    // Covers AE2 from the plan (long-running hat killed during
    // model warm-up).
    let yaml = r#"
hats:
  dimension-reviewer:
    name: "Dimension Reviewer"
    triggers: ["review.dimension.ready"]
    publishes: ["review.dimension.done", "review.dimension.failed"]
    missing_event_grace_secs: 540
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let hat = HatId::new("dimension-reviewer");
    // Record the activation *now* — elapsed is well under 540s.
    event_loop.state_mut().record_hat_activation(&hat);

    let no_candidates: Vec<String> = Vec::new();
    assert!(
        !should_gate_missing_events(&hat, &event_loop, &no_candidates),
        "gate must be suppressed within the per-hat grace window"
    );
}

#[test]
fn test_u2_t2_2_grace_boundary_just_inside_window_suppresses_gate() {
    // T2.2: a hat that was activated `grace - 1s` ago must still
    // be within the window.  We simulate this by manually
    // inserting an `Instant` in the past into the clock.
    use std::time::{Duration, Instant};
    let yaml = r#"
hats:
  dimension-reviewer:
    name: "Dimension Reviewer"
    triggers: ["review.dimension.ready"]
    publishes: ["review.dimension.done", "review.dimension.failed"]
    missing_event_grace_secs: 60
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let hat = HatId::new("dimension-reviewer");
    // Insert an activation that was 59s ago.  `Instant` does not
    // allow arbitrary backdating, so we use a tiny manual offset
    // by sleeping 50ms to ensure elapsed > 0.  The 60s grace
    // window is far larger than any test sleep, so the gate must
    // remain suppressed.
    event_loop.state_mut().record_hat_activation(&hat);
    std::thread::sleep(Duration::from_millis(50));
    let _ = Instant::now(); // ensure Instant is in scope

    let no_candidates: Vec<String> = Vec::new();
    assert!(
        !should_gate_missing_events(&hat, &event_loop, &no_candidates),
        "gate must be suppressed for any elapsed < grace_secs (60s grace, 50ms sleep)"
    );
}

#[test]
fn test_u2_t2_3_grace_boundary_just_outside_window_fires_gate() {
    // T2.3: a hat with `missing_event_grace_secs: 0` has no grace
    // window — the gate must fire on the very first iteration.
    // This is the "opt out" path.  We also verify the legacy
    // blanket-rule hat (no `obligations:` declared) still
    // respects the grace when one is set.
    let yaml_zero = r#"
hats:
  legacy:
    name: "Legacy"
    triggers: ["work.ready"]
    publishes: ["work.done"]
    missing_event_grace_secs: 0
"#;
    let config_zero: RalphConfig = serde_yaml::from_str(yaml_zero).unwrap();
    let mut event_loop_zero = EventLoop::new(config_zero);
    let hat_zero = HatId::new("legacy");
    event_loop_zero.state_mut().record_hat_activation(&hat_zero);

    let no_candidates: Vec<String> = Vec::new();
    assert!(
        should_gate_missing_events(&hat_zero, &event_loop_zero, &no_candidates),
        "missing_event_grace_secs: 0 disables the defer — gate fires immediately"
    );
}

#[test]
fn test_u2_t2_4_wave_obligation_still_suppresses_gate_during_grace() {
    // T2.4: regression for `test_wave_policy_rejection_skips_missing_event_gate`.
    // Even with the new grace logic, a wave obligation that is
    // still pending (no terminal phase) must continue to
    // suppress the gate.  The grace check is a *new* early-return
    // path; the wave-mutex / obligation check is layered on top.
    use ralph_core::flow_lifecycle::{FlowLifecycleRecord, FlowLifecycleRegistry, FlowPhase};
    let yaml = r#"
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["work.done"]
    publishes: ["review.wave.ready", "review.passed"]
    obligations:
      - on_trigger: "work.done"
        must_emit_any_of: ["review.wave.ready", "review.passed"]
    missing_event_grace_secs: 540
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.state_mut().last_activation_events =
        vec![ralph_proto::Event::new("work.done", "{}")];
    let hat = HatId::new("review-coordinator");
    // Record an open wave — this is the "waiting on workers" case.
    use std::time::Instant;
    let now = Instant::now();
    let mut wave_reg = FlowLifecycleRegistry::new();
    wave_reg.register(FlowLifecycleRecord {
        flow_unit_id: "w-001".into(),
        target_hat: hat.as_str().into(),
        source_topic: "review.wave.ready".into(),
        wave_total: 3,
        received_count: 0,
        missing_indices: vec![0, 1, 2],
        configured_aggregate_secs: 0,
        configured_worker_secs: 0,
        started_at: now,
        last_transition_at: now,
        phase: FlowPhase::WorkersActive,
        last_source_hat: None,
        last_reason_code: None,
    });
    event_loop.state_mut().flow_lifecycle = wave_reg;
    // Record activation just now to be inside grace.
    event_loop.state_mut().record_hat_activation(&hat);

    let no_candidates: Vec<String> = Vec::new();
    assert!(
        !should_gate_missing_events(&hat, &event_loop, &no_candidates),
        "wave obligation pending must continue to suppress the gate even within grace window"
    );
}

#[test]
fn test_u2_t2_5_agent_emit_rejected_still_suppresses_gate() {
    // T2.5: regression — when the agent DID try to emit
    // (contract-rejected topics land in `candidate_topics`),
    // the gate must continue to be suppressed.  The grace
    // window does not change this behaviour; it just adds an
    // additional early-return path.
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done", "work.failed"]
    missing_event_grace_secs: 540
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    let hat = HatId::new("executor");
    event_loop.state_mut().record_hat_activation(&hat);

    // Candidates include the contract-rejected `work.done` topic.
    let rejected_candidates = vec!["work.done".to_string()];
    assert!(
        !should_gate_missing_events(&hat, &event_loop, &rejected_candidates),
        "agent attempting to emit (even if contract-rejected) must continue to suppress the gate"
    );
}

#[test]
fn test_u2_resolve_missing_event_grace_secs_helper() {
    // Pin the resolution chain from KTD-4 in the plan:
    //   1. per-hat `missing_event_grace_secs` wins
    //   2. preset default
    //   3. min(adapter_idle * 0.3, 540)
    use ralph_core::config::hat::HatConfig;
    use ralph_core::config::hat::resolve_missing_event_grace_secs;

    // Case 1: per-hat override wins.
    let mut h = HatConfig::default();
    h.missing_event_grace_secs = Some(7);
    let g = resolve_missing_event_grace_secs(&h, Some(999), 1000);
    assert_eq!(g, 7, "per-hat override wins");

    // Case 2: per-hat is None, preset default wins.
    let mut h2 = HatConfig::default();
    h2.missing_event_grace_secs = None;
    let g2 = resolve_missing_event_grace_secs(&h2, Some(120), 1000);
    assert_eq!(g2, 120, "preset default wins when per-hat is None");

    // Case 3: per-hat None, preset default None, fallback applies.
    let mut h3 = HatConfig::default();
    h3.missing_event_grace_secs = None;
    // adapter_idle = 1000, expected: floor(1000 * 0.3) = 300.
    let g3 = resolve_missing_event_grace_secs(&h3, None, 1000);
    assert_eq!(g3, 300);

    // Case 4: cap at 540s.
    let mut h4 = HatConfig::default();
    h4.missing_event_grace_secs = None;
    // adapter_idle = 100_000, expected: min(floor(100_000 * 0.3), 540) = 540.
    let g4 = resolve_missing_event_grace_secs(&h4, None, 100_000);
    assert_eq!(g4, 540, "540s floor cap must apply");

    // Case 5: explicit Some(0) opts out (gate fires immediately).
    let mut h5 = HatConfig::default();
    h5.missing_event_grace_secs = Some(0);
    let g5 = resolve_missing_event_grace_secs(&h5, Some(999), 1000);
    assert_eq!(g5, 0, "Some(0) opt-out must be honoured");
}

// 2026-06-17-004 U3 (R4+R5): Recovery routing — target + trigger
// context replay.  These tests pin the new behaviour (T3.1-T3.6
// in the plan).

#[test]
fn test_u3_t3_1_replay_obligation_triggers_to_activation_state() {
    // T3.1 / T3.4: when the gate fires, the next activation must
    // see the original trigger topic in `last_activation_events`
    // (not the empty default).  Multi-trigger snapshots are
    // drained as-is (the obligation check filters by topic at
    // evaluation time).
    let mut state = ralph_core::event_loop::LoopState::new();
    let trigger =
        ralph_proto::Event::new("review.dimension.ready", r#"{"dimension":"correctness"}"#);
    state.pending_obligation_triggers = vec![trigger.clone()];

    let drained = state.replay_obligation_triggers_to_activation_state();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].topic.as_str(), "review.dimension.ready");
    assert_eq!(state.last_activation_events.len(), 1);
    assert_eq!(
        state.last_activation_events[0].topic.as_str(),
        "review.dimension.ready"
    );
    // Drain again — empty now.
    let drained2 = state.replay_obligation_triggers_to_activation_state();
    assert!(drained2.is_empty());
    // last_activation_events is preserved on a no-op replay.
    assert_eq!(state.last_activation_events.len(), 1);
}

#[test]
fn test_u3_t3_2_inject_missing_event_writes_target_field() {
    // T3.2: the resume JSONL line written by the missing-event
    // gate helper must include a top-level `target` field equal
    // to the offending hat id, and a `hat` field too (for U1
    // provenance allowlist compatibility).
    use std::io::Read;
    let tmp = tempfile::tempdir().expect("tempdir");
    // Create the .ralph/ directory so the current-events marker
    // file can be written.
    std::fs::create_dir_all(tmp.path().join(".ralph")).expect("create .ralph");
    let events_path = tmp.path().join(".ralph/events.jsonl");
    std::fs::write(&events_path, "").expect("seed events file");
    // Write the current-events marker so the helper resolves to
    // our chosen path.
    let ctx = ralph_core::loop_context::LoopContext::primary(tmp.path().to_path_buf());
    std::fs::write(
        ctx.current_events_marker(),
        events_path.to_string_lossy().to_string(),
    )
    .expect("write current-events marker");

    let hat = HatId::new("dimension-reviewer");
    let expected_topics = vec!["review.dimension.done".to_string()];

    inject_missing_event_hard_gate_guidance(&ctx, None, &hat, &expected_topics);

    let mut buf = String::new();
    std::fs::File::open(&events_path)
        .expect("open events file")
        .read_to_string(&mut buf)
        .expect("read events file");
    let line = buf.lines().last().expect("at least one line written");
    let value: serde_json::Value = serde_json::from_str(line).expect("valid JSONL");
    assert_eq!(value["topic"], "task.resume");
    assert_eq!(value["target"], "dimension-reviewer");
    assert_eq!(value["hat"], "dimension-reviewer");
    assert!(value["payload"].is_string());
}

#[test]
fn test_u3_t3_3_payload_has_stage_missing_event_and_target_hat() {
    // T3.3: the embedded payload (the `payload` field, which is
    // itself a JSON string) must contain `stage: missing_event`,
    // `target_hat: dimension-reviewer`, and `reason`.
    use std::io::Read;
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join(".ralph")).expect("create .ralph");
    let events_path = tmp.path().join(".ralph/events.jsonl");
    std::fs::write(&events_path, "").expect("seed events file");
    let ctx = ralph_core::loop_context::LoopContext::primary(tmp.path().to_path_buf());
    std::fs::write(
        ctx.current_events_marker(),
        events_path.to_string_lossy().to_string(),
    )
    .expect("write current-events marker");

    let hat = HatId::new("dimension-reviewer");
    let expected_topics = vec!["review.dimension.done".to_string()];

    inject_missing_event_hard_gate_guidance(&ctx, None, &hat, &expected_topics);

    let mut buf = String::new();
    std::fs::File::open(&events_path)
        .expect("open events file")
        .read_to_string(&mut buf)
        .expect("read events file");
    let line = buf.lines().last().expect("at least one line written");
    let value: serde_json::Value = serde_json::from_str(line).expect("valid JSONL");
    let payload_str = value["payload"].as_str().expect("payload is a string");
    let payload: serde_json::Value = serde_json::from_str(payload_str).expect("payload is JSON");
    assert_eq!(payload["stage"], "missing_event");
    assert_eq!(payload["target_hat"], "dimension-reviewer");
    assert_eq!(payload["reason"], "missing_field");
    assert!(
        payload["allowed_topics"]
            .as_array()
            .expect("allowed_topics array")
            .iter()
            .any(|v| v == "review.dimension.done")
    );
}

#[test]
fn test_u3_t3_4_triggers_embedded_in_resume_when_provided() {
    // T3.4: when the helper is called with the trigger-snapshot
    // extension, the first trigger's `topic` and `payload` are
    // embedded in the resume JSON as `original_trigger_topic`
    // and `original_trigger_payload`.
    use std::io::Read;
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join(".ralph")).expect("create .ralph");
    let events_path = tmp.path().join(".ralph/events.jsonl");
    std::fs::write(&events_path, "").expect("seed events file");
    let ctx = ralph_core::loop_context::LoopContext::primary(tmp.path().to_path_buf());
    std::fs::write(
        ctx.current_events_marker(),
        events_path.to_string_lossy().to_string(),
    )
    .expect("write current-events marker");

    let hat = HatId::new("dimension-reviewer");
    let expected_topics = vec!["review.dimension.done".to_string()];
    let trigger = ralph_proto::Event::new(
        "review.dimension.ready",
        r#"{"dimension":"correctness","depth":"standard"}"#,
    );

    inject_missing_event_hard_gate_guidance_with_triggers(
        &ctx,
        None,
        &hat,
        &expected_topics,
        &[trigger],
    );

    let mut buf = String::new();
    std::fs::File::open(&events_path)
        .expect("open events file")
        .read_to_string(&mut buf)
        .expect("read events file");
    let line = buf.lines().last().expect("at least one line written");
    let value: serde_json::Value = serde_json::from_str(line).expect("valid JSONL");
    let payload_str = value["payload"].as_str().expect("payload is a string");
    let payload: serde_json::Value = serde_json::from_str(payload_str).expect("payload is JSON");
    assert_eq!(payload["original_trigger_topic"], "review.dimension.ready");
    assert_eq!(
        payload["original_trigger_payload"]["dimension"],
        "correctness"
    );
    assert_eq!(payload["original_trigger_payload"]["depth"], "standard");
}

#[test]
fn test_u4_t4_1_hard_gate_guidance_embeds_original_trigger() {
    // U4 (R1) T4.1 — happy path: when the runner takes the
    // claim-but-no-write path with an obligation trigger snapshot,
    // the resume JSON must embed the original trigger topic +
    // payload and stamp `target` + `stage` so the recovery can
    // route the `dimension-reviewer` back to the right
    // `dimension`.  Mirrors `test_u3_t3_4_triggers_embedded_in_resume_when_provided`
    // but for the claim-but-no-write variant (the new
    // `inject_hard_gate_guidance_with_triggers` helper).  Also
    // pins the JSONL top-level shape: `target: dimension-reviewer`
    // (matches the missing-event path's `Event::with_target`).
    use std::io::Read;
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join(".ralph")).expect("create .ralph");
    let events_path = tmp.path().join(".ralph/events.jsonl");
    std::fs::write(&events_path, "").expect("seed events file");
    let ctx = ralph_core::loop_context::LoopContext::primary(tmp.path().to_path_buf());
    std::fs::write(
        ctx.current_events_marker(),
        events_path.to_string_lossy().to_string(),
    )
    .expect("write current-events marker");

    let hat = HatId::new("dimension-reviewer");
    let expected_topics = vec!["review.dimension.done".to_string()];
    let trigger = ralph_proto::Event::new(
        "review.dimension.ready",
        r#"{"dimension":"testing","depth":"standard","diff_base":"HEAD~1"}"#,
    );

    inject_hard_gate_guidance_with_triggers(&ctx, None, &hat, &expected_topics, &[trigger]);

    let mut buf = String::new();
    std::fs::File::open(&events_path)
        .expect("open events file")
        .read_to_string(&mut buf)
        .expect("read events file");
    let line = buf.lines().last().expect("at least one line written");
    let value: serde_json::Value = serde_json::from_str(line).expect("valid JSONL");
    // Top-level shape: `target: dimension-reviewer` so the
    // EventBus re-reader routes the resume to the gated hat
    // without parsing the payload.  The `hat` field is also
    // written for the U1 provenance allowlist.
    assert_eq!(value["topic"], "task.resume");
    assert_eq!(value["hat"], "dimension-reviewer");
    assert_eq!(value["target"], "dimension-reviewer");

    let payload_str = value["payload"].as_str().expect("payload is a string");
    let payload: serde_json::Value = serde_json::from_str(payload_str).expect("payload is JSON");
    // Schema-required fields preserved by the wrapper.
    assert_eq!(payload["reason"], "other"); // "emit_claimed_but_not_written" has no keyword in extract_reason_code
    assert_eq!(payload["target_hat"], "dimension-reviewer");
    // U4 stage: the new `emit_claimed_but_not_written` variant
    // distinguishes this from the missing-event gate's
    // `missing_event` stage.  Both share the recovery shape but
    // have different operator-actionable root causes.
    assert_eq!(payload["stage"], "emit_claimed_but_not_written");
    // U4 trigger embedding: the resume carries the original
    // `review.dimension.ready` topic + payload so the resumed
    // hat knows which dimension to review.
    assert_eq!(payload["original_trigger_topic"], "review.dimension.ready");
    assert_eq!(payload["original_trigger_payload"]["dimension"], "testing");
    assert_eq!(payload["original_trigger_payload"]["depth"], "standard");
    assert_eq!(payload["original_trigger_payload"]["diff_base"], "HEAD~1");
    // `allowed_topics` and `triggered` are still stamped so the
    // agent can read the recovery instructions from the event.
    let allowed = payload["allowed_topics"]
        .as_array()
        .expect("allowed_topics must be an array");
    assert!(allowed.iter().any(|v| v == "review.dimension.done"));
    assert_eq!(payload["triggered"], "dimension-reviewer");
}

#[test]
fn test_u4_t4_2_hard_gate_guidance_no_triggers_omits_fields() {
    // U4 (R1) T4.2 — edge case: when the claim-but-no-write
    // helper is called WITHOUT a trigger snapshot (legacy caller
    // path), the resume JSON remains valid and the schema-required
    // fields are still present, but the `original_trigger_*`
    // fields are simply OMITTED (rather than written as null /
    // empty), preserving the existing wire shape for any
    // downstream consumer that asserted on field presence.
    use std::io::Read;
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join(".ralph")).expect("create .ralph");
    let events_path = tmp.path().join(".ralph/events.jsonl");
    std::fs::write(&events_path, "").expect("seed events file");
    let ctx = ralph_core::loop_context::LoopContext::primary(tmp.path().to_path_buf());
    std::fs::write(
        ctx.current_events_marker(),
        events_path.to_string_lossy().to_string(),
    )
    .expect("write current-events marker");

    let hat = HatId::new("dimension-reviewer");
    let expected_topics = vec!["review.dimension.done".to_string()];

    // Legacy path: wrapper with empty trigger slice.
    inject_hard_gate_guidance(&ctx, None, &hat, &expected_topics);

    let mut buf = String::new();
    std::fs::File::open(&events_path)
        .expect("open events file")
        .read_to_string(&mut buf)
        .expect("read events file");
    let line = buf.lines().last().expect("at least one line written");
    let value: serde_json::Value = serde_json::from_str(line).expect("valid JSONL");
    let payload_str = value["payload"].as_str().expect("payload is a string");
    let payload: serde_json::Value = serde_json::from_str(payload_str).expect("payload is JSON");
    // Schema-required fields still present.
    assert_eq!(payload["target_hat"], "dimension-reviewer");
    // Stage is now stamped by the wrapper too (U4 unification).
    assert_eq!(payload["stage"], "emit_claimed_but_not_written");
    // `original_trigger_*` are absent when no triggers provided.
    assert!(
        payload.get("original_trigger_topic").is_none(),
        "legacy callers must NOT see original_trigger_topic"
    );
    assert!(
        payload.get("original_trigger_payload").is_none(),
        "legacy callers must NOT see original_trigger_payload"
    );
    // Top-level shape unchanged.
    assert_eq!(value["target"], "dimension-reviewer");
    assert_eq!(value["hat"], "dimension-reviewer");
}

#[test]
fn test_u4_t4_3_hard_gate_guidance_stashes_triggers_for_replay() {
    // U4 (R1) T4.3 — error path coverage: when the helper is
    // called WITH an event loop handle, it must stash the trigger
    // snapshot into `LoopState::pending_obligation_triggers` so
    // the runner's `replay_obligation_triggers_to_activation_state`
    // can drain it into `last_activation_events` for the next
    // activation.  Without the stash, the next activation's
    // `last_activation_events` would be empty and the obligation
    // check in `should_gate_missing_events` could not evaluate
    // the gate (silent DR would not be recognised on the second
    // turn and could keep gating forever).
    let (_temp, workspace) = u4_workspace();
    let diagnostics = ralph_core::diagnostics::DiagnosticsCollector::with_enabled(&workspace, true)
        .expect("create diagnostics collector");
    let config = ralph_core::RalphConfig::default();
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);

    let ctx = LoopContext::primary(workspace.clone());
    let hat = ralph_proto::HatId::new("dimension-reviewer");
    let expected_topics = vec!["review.dimension.done".to_string()];
    let trigger = ralph_proto::Event::new("review.dimension.ready", r#"{"dimension":"testing"}"#);

    // Sanity: snapshot is empty on a fresh loop.
    assert!(
        event_loop.state().pending_obligation_triggers.is_empty(),
        "fresh loop must have an empty trigger snapshot"
    );

    inject_hard_gate_guidance_with_triggers(
        &ctx,
        Some(&mut event_loop),
        &hat,
        &expected_topics,
        &[trigger.clone()],
    );

    // After inject: the snapshot must contain the trigger so
    // the runner can drain it.
    let snapshot = &event_loop.state().pending_obligation_triggers;
    assert_eq!(snapshot.len(), 1, "trigger snapshot must be populated");
    assert_eq!(snapshot[0].topic.as_str(), "review.dimension.ready");
    assert!(
        snapshot[0].payload.contains("\"dimension\":\"testing\""),
        "trigger payload must round-trip the original `dimension`"
    );
    // `pending_recovery_hat` is also pinned (matches the
    // pre-existing U3 P1 fix behaviour).
    assert_eq!(
        event_loop
            .state()
            .pending_recovery_hat
            .as_ref()
            .map(|h| h.as_str()),
        Some("dimension-reviewer"),
        "pending_recovery_hat must be pinned to the gated hat"
    );

    // Now drain via the same helper the runner calls.
    let drained = event_loop
        .state_mut()
        .replay_obligation_triggers_to_activation_state();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].topic.as_str(), "review.dimension.ready");
    // The next activation sees the original trigger in
    // `last_activation_events` so the obligation check has the
    // right context to evaluate the gate.
    assert_eq!(event_loop.state().last_activation_events.len(), 1);
    assert_eq!(
        event_loop.state().last_activation_events[0].topic.as_str(),
        "review.dimension.ready"
    );
}

#[test]
fn test_u4_t4_4_hard_gate_guidance_legacy_caller_does_not_stash() {
    // U4 (R1) T4.4 — defence: the legacy wrapper
    // `inject_hard_gate_guidance` (no trigger arg) must NOT
    // populate `pending_obligation_triggers` even when an
    // EventLoop handle is provided — the trigger slice is
    // always empty, so the stash is skipped (matches the
    // missing-event path's `if !obligation_triggers.is_empty()`
    // guard).  This prevents stale triggers from leaking across
    // iterations when a legacy caller fires the gate.
    let (_temp, workspace) = u4_workspace();
    let diagnostics = ralph_core::diagnostics::DiagnosticsCollector::with_enabled(&workspace, true)
        .expect("create diagnostics collector");
    let config = ralph_core::RalphConfig::default();
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);

    let ctx = LoopContext::primary(workspace.clone());
    let hat = ralph_proto::HatId::new("executor");
    let expected_topics = vec!["work.done".to_string()];

    inject_hard_gate_guidance(&ctx, Some(&mut event_loop), &hat, &expected_topics);

    // No triggers → snapshot stays empty (no stale data).
    assert!(
        event_loop.state().pending_obligation_triggers.is_empty(),
        "legacy wrapper must NOT populate pending_obligation_triggers"
    );
    // The pin is still set (P1 fix).
    assert_eq!(
        event_loop
            .state()
            .pending_recovery_hat
            .as_ref()
            .map(|h| h.as_str()),
        Some("executor")
    );
}

#[test]
fn test_u3_t3_5_enrich_with_stage_helper() {
    // T3.5: enrich_task_resume_payload_with_stage must add a
    // `stage` field when supplied, and must NOT add one when
    // the caller passes `None` (legacy behaviour).
    //
    // 2026-06-23-005 U1: also assert the new typed `kind` field.
    use ralph_core::RejectionKind;
    use ralph_core::event_loop::rejection::task_resume_payload_has_required_fields;
    use ralph_core::event_loop::rejection::{
        RejectionStage, enrich_task_resume_payload_with_stage,
    };

    // Case 1: explicit stage + typed kind → JSON has `stage` AND `kind` fields.
    let p1 = enrich_task_resume_payload_with_stage(
        "missing event",
        "hard_gate_missing_event",
        Some("dimension-reviewer"),
        Some(RejectionStage::MissingEvent),
        Some(RejectionKind::MissingEventGate),
    );
    assert!(task_resume_payload_has_required_fields(&p1));
    let v1: serde_json::Value = serde_json::from_str(&p1).expect("valid JSON");
    assert_eq!(v1["stage"], "missing_event");
    assert_eq!(v1["target_hat"], "dimension-reviewer");
    assert_eq!(v1["reason"], "missing_field");
    assert_eq!(v1["kind"], "missing_event_gate");

    // Case 2: None → JSON has no `stage` field. Kind None → falls back to reason.
    let p2 = enrich_task_resume_payload_with_stage(
        "missing event",
        "hard_gate_missing_event",
        Some("dimension-reviewer"),
        None,
        None,
    );
    let v2: serde_json::Value = serde_json::from_str(&p2).expect("valid JSON");
    assert!(
        v2.get("stage").is_none(),
        "legacy callers must not see a `stage` field"
    );
    assert_eq!(v2["target_hat"], "dimension-reviewer");
    assert_eq!(v2["kind"], "missing_field"); // fallback to reason_code
}
