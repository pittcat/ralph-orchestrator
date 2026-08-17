use super::*;
use ralph_core::PolicyRuntimeState;
use tempfile::TempDir;

fn strict_policy_with_required(field: &str) -> EventPolicyConfig {
    let yaml = format!(
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: false
    schemas:
      review.wave.ready:
        required_fields:
          - {field}
      work.done:
        required_fields:
          - {field}
      work.ready:
        required_fields:
          - {field}
"
    );
    let cfg: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
    cfg.event_loop.event_policy.unwrap()
}

/// 2026-07-26-004 plan U7 (R7 / S7 / S8): the CLI `--policy-check`
/// flow-step decision MUST agree with the resident EventLoop. With no
/// accepted-state ledger the recovered step is the first step
/// (`scope_freeze`), so `review.unit.done` is rejected as
/// `flow_unknown_emit`; once the accepted-step ledger records
/// `scope.ready`, the recovered step is `review_wave` and the same
/// emit is admitted — no silent fall-back to raw topics (the
/// primary-20260726 flow drift).
#[test]
fn u7_cli_flow_step_scope_agrees_with_recovered_step() {
    use ralph_core::config::{
        EventLoopConfig, FlowDeclarationConfig, FlowStepConfig, MechanismConfig, RalphConfig,
    };
    let mk =
        |id: &str, allowed: Vec<&str>, on: Option<&str>, on_any_of: Vec<&str>| -> FlowStepConfig {
            FlowStepConfig {
                id: id.to_string(),
                kind: None,
                allowed_emits: allowed.into_iter().map(String::from).collect(),
                terminal_when: None,
                on_partial: std::collections::BTreeMap::new(),
                runs: None,
                on: on.map(String::from),
                on_any_of: on_any_of.into_iter().map(String::from).collect(),
                transition_emits: Vec::new(),
            }
        };
    let mut cfg = RalphConfig::default();
    cfg.mechanism = None;
    cfg.event_loop = EventLoopConfig {
        mechanism: Some(MechanismConfig {
            flow: Some(FlowDeclarationConfig {
                flow_type: "declared".to_string(),
                version: 1,
                terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                steps: vec![
                    mk("scope_freeze", vec!["scope.ready"], None, vec![]),
                    mk(
                        "review_wave",
                        vec!["review.unit.done"],
                        Some("scope.ready"),
                        vec![],
                    ),
                ],
                repair_budget: 3,
                enforce_schema: "hard".to_string(),
                state_idempotency: "required".to_string(),
            }),
            phase_authority: None,
        }),
        ..EventLoopConfig::default()
    };

    let ws = tempfile::tempdir().expect("tempdir");
    // Before scope.ready: review.unit.done at scope_freeze → rejected.
    let reject = check_cli_flow_step_scope(
        &cfg,
        ws.path(),
        None,
        "review.unit.done",
        Some("review-worker"),
        Some("{}"),
    );
    assert_eq!(
        reject.as_deref(),
        Some("flow_unknown_emit"),
        "review.unit.done at the first step must be flow_unknown_emit"
    );

    // Land scope.ready in the accepted-step ledger → recovered
    // step = review_wave.
    std::fs::create_dir_all(ws.path().join(".ralph")).expect("mkdir .ralph");
    std::fs::write(
        ws.path().join(".ralph/flow-authority.jsonl"),
        "{\"step\":\"review_wave\",\"topic\":\"scope.ready\"}\n",
    )
    .expect("write accepted-step ledger");

    // After scope.ready: the SAME emit is admitted (CLI agrees with
    // the recovered review_wave step the EventLoop would hold).
    let admit = check_cli_flow_step_scope(
        &cfg,
        ws.path(),
        None,
        "review.unit.done",
        Some("review-worker"),
        Some("{}"),
    );
    assert_eq!(
        admit, None,
        "CLI policy-check must agree with the recovered review_wave step"
    );
}

/// Regression for the active-main-ledger case: when `.ralph/current-events`
/// points at a timestamped main ledger, policy-check must recover the
/// current step from that accepted ledger instead of falling back to the
/// static `.ralph/events.jsonl` file.
#[test]
fn u7_policy_check_uses_current_events_marker_for_active_main_ledger() {
    use ralph_core::config::{
        EventLoopConfig, FlowDeclarationConfig, FlowStepConfig, MechanismConfig, RalphConfig,
    };
    let mk =
        |id: &str, allowed: Vec<&str>, on: Option<&str>, on_any_of: Vec<&str>| -> FlowStepConfig {
            FlowStepConfig {
                id: id.to_string(),
                kind: None,
                allowed_emits: allowed.into_iter().map(String::from).collect(),
                terminal_when: None,
                on_partial: std::collections::BTreeMap::new(),
                runs: None,
                on: on.map(String::from),
                on_any_of: on_any_of.into_iter().map(String::from).collect(),
                transition_emits: Vec::new(),
            }
        };
    let mut cfg = RalphConfig::default();
    cfg.event_loop = EventLoopConfig {
        mechanism: Some(MechanismConfig {
            flow: Some(FlowDeclarationConfig {
                flow_type: "declared".to_string(),
                version: 1,
                terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                steps: vec![
                    mk("scope_freeze", vec!["scope.ready"], None, vec![]),
                    mk(
                        "review_wave",
                        vec!["review.unit.done"],
                        Some("scope.ready"),
                        vec![],
                    ),
                ],
                repair_budget: 3,
                enforce_schema: "hard".to_string(),
                state_idempotency: "required".to_string(),
            }),
            phase_authority: None,
        }),
        ..EventLoopConfig::default()
    };

    let ws = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(ws.path().join(".ralph")).expect("mkdir .ralph");
    let active_events = ws.path().join(".ralph/events-20260727-023002.jsonl");
    std::fs::write(&active_events, "{\"topic\":\"scope.ready\"}\n")
        .expect("write active main ledger");
    std::fs::write(
        ws.path().join(".ralph/current-events"),
        " .ralph/events-20260727-023002.jsonl\n",
    )
    .expect("write current-events marker");

    let admit = check_cli_flow_step_scope(
        &cfg,
        ws.path(),
        None,
        "review.unit.done",
        Some("review-worker"),
        Some("{}"),
    );
    assert_eq!(
        admit, None,
        "policy-check must recover review_wave from the current-events marker"
    );
}

/// P1 regression: when the accepted-step ledger is missing, the
/// default workspace path must not infer the current step from raw
/// topic logs. A forged `review.wave.failed` in `events.jsonl`
/// must not advance the recovered step to `finalize` and allow
/// `review.blocked`.
#[test]
fn p1_8_policy_check_default_path_ignores_raw_topic_replay_without_acceptance_ledger() {
    use ralph_core::config::{
        EventLoopConfig, FlowDeclarationConfig, FlowStepConfig, MechanismConfig, RalphConfig,
    };
    let mk =
        |id: &str, allowed: Vec<&str>, on: Option<&str>, on_any_of: Vec<&str>| -> FlowStepConfig {
            FlowStepConfig {
                id: id.to_string(),
                kind: None,
                allowed_emits: allowed.into_iter().map(String::from).collect(),
                terminal_when: None,
                on_partial: std::collections::BTreeMap::new(),
                runs: None,
                on: on.map(String::from),
                on_any_of: on_any_of.into_iter().map(String::from).collect(),
                transition_emits: Vec::new(),
            }
        };

    let mut cfg = RalphConfig::default();
    cfg.event_loop = EventLoopConfig {
        mechanism: Some(MechanismConfig {
            flow: Some(FlowDeclarationConfig {
                flow_type: "declared".to_string(),
                version: 1,
                terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                steps: vec![
                    mk("scope_freeze", vec!["scope.ready"], None, vec![]),
                    mk(
                        "review_wave",
                        vec!["review.unit.done", "review.wave.failed"],
                        Some("scope.ready"),
                        vec!["finalize"],
                    ),
                    mk(
                        "finalize",
                        vec!["review.blocked"],
                        Some("review.wave.failed"),
                        vec![],
                    ),
                ],
                enforce_schema: "hard".to_string(),
                state_idempotency: "required".to_string(),
                repair_budget: Default::default(),
            }),
            phase_authority: None,
        }),
        ..EventLoopConfig::default()
    };

    let ws = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(ws.path().join(".ralph")).expect("mkdir .ralph");
    std::fs::write(
        ws.path().join(".ralph/events.jsonl"),
        "{\"topic\":\"scope.ready\"}\n{\"topic\":\"review.wave.failed\"}\n",
    )
    .expect("write raw ledger");

    let reject = check_cli_flow_step_scope(
        &cfg,
        ws.path(),
        None,
        "review.blocked",
        Some("finalizer"),
        Some("{}"),
    );
    assert_eq!(
        reject.as_deref(),
        Some("flow_unknown_emit"),
        "default-path policy-check must ignore raw topic replay when the accepted-step ledger is missing",
    );
}

/// Plan 004 P1-8: CLI policy-check honours a caller-provided
/// `events_file`. Two loops A and B in the same workspace
/// write different ledgers; policy-check on loop A must NOT
/// read loop B's ledger. We pin the contract here by feeding
/// each call a distinct ledger path and asserting the
/// recovered step matches that ledger's topic sequence —
/// not the workspace's default `.ralph/events.jsonl`.
#[test]
fn p1_8_policy_check_respects_caller_events_file() {
    use ralph_core::config::{
        EventLoopConfig, FlowDeclarationConfig, FlowStepConfig, MechanismConfig,
    };
    let cfg = RalphConfig {
        mechanism: None,
        event_loop: EventLoopConfig {
            mechanism: Some(MechanismConfig {
                flow: Some(FlowDeclarationConfig {
                    flow_type: "declared".to_string(),
                    version: 1,
                    terminal_emits: vec!["LOOP_COMPLETE".to_string()],
                    steps: vec![
                        FlowStepConfig {
                            id: "scope_freeze".to_string(),
                            kind: None,
                            allowed_emits: vec!["scope.ready".to_string()],
                            terminal_when: None,
                            on_partial: Default::default(),
                            runs: None,
                            on: None,
                            on_any_of: Vec::new(),
                            transition_emits: Vec::new(),
                        },
                        FlowStepConfig {
                            id: "review_wave".to_string(),
                            kind: None,
                            allowed_emits: vec!["review.unit.done".to_string()],
                            terminal_when: None,
                            on_partial: Default::default(),
                            runs: None,
                            on: None,
                            on_any_of: Vec::new(),
                            transition_emits: Vec::new(),
                        },
                    ],
                    enforce_schema: "hard".to_string(),
                    state_idempotency: "required".to_string(),
                    repair_budget: Default::default(),
                }),
                phase_authority: None,
            }),
            ..EventLoopConfig::default()
        },
        ..RalphConfig::default()
    };

    let ws = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(ws.path().join(".ralph")).expect("mkdir .ralph");
    // Two loops; two ledgers; two recovery targets.
    let loop_a = ws.path().join("loop-a.jsonl");
    let loop_b = ws.path().join("loop-b.jsonl");
    // Default ledger (workspace_root/.ralph/events.jsonl) is
    // intentionally left untouched — we exercise the
    // caller-provided path.
    std::fs::write(&loop_a, "{\"topic\":\"scope.ready\"}\n").expect("write A");
    std::fs::write(&loop_b, "{}\n").expect("write B");

    // Loop A has scope.ready → recovered review_wave →
    // review.unit.done admitted.
    let admit_a = check_cli_flow_step_scope(
        &cfg,
        ws.path(),
        Some(loop_a.as_path()),
        "review.unit.done",
        Some("review-worker"),
        Some("{}"),
    );
    assert!(
        admit_a.is_none(),
        "loop A must admit review.unit.done via its own ledger; got {admit_a:?}",
    );

    // Loop B has only an empty (malformed) line → recovered
    // step stays at scope_freeze → review.unit.done rejected.
    let reject_b = check_cli_flow_step_scope(
        &cfg,
        ws.path(),
        Some(loop_b.as_path()),
        "review.unit.done",
        Some("review-worker"),
        Some("{}"),
    );
    assert_eq!(
        reject_b.as_deref(),
        Some("flow_unknown_emit"),
        "loop B must reject review.unit.done because its ledger has no scope.ready",
    );
}

#[test]
fn test_resolve_policy_check_mode_explicit_wins() {
    let flags = PolicyCheckFlags {
        policy_check: true,
        no_policy_check: false,
    };
    assert_eq!(
        resolve_policy_check_mode(&flags, None),
        PolicyCheckMode::ExplicitCheck
    );
}

#[test]
fn test_resolve_policy_check_mode_strict_without_flags_is_enforce() {
    let policy = strict_policy_with_required("depth");
    let cfg = RalphConfig {
        event_loop: ralph_core::config::EventLoopConfig {
            event_policy: Some(policy),
            ..Default::default()
        },
        ..Default::default()
    };
    let flags = PolicyCheckFlags {
        policy_check: false,
        no_policy_check: false,
    };
    assert_eq!(
        resolve_policy_check_mode(&flags, Some(&cfg)),
        PolicyCheckMode::Enforce
    );
}

#[test]
fn test_resolve_policy_check_mode_unsafe_bypass_blocked_when_disallowed() {
    let mut policy = strict_policy_with_required("depth");
    policy.allow_unsafe_cli_emit = false;
    let cfg = RalphConfig {
        event_loop: ralph_core::config::EventLoopConfig {
            event_policy: Some(policy),
            ..Default::default()
        },
        ..Default::default()
    };
    let flags = PolicyCheckFlags {
        policy_check: false,
        no_policy_check: true,
    };
    assert_eq!(
        resolve_policy_check_mode(&flags, Some(&cfg)),
        PolicyCheckMode::Enforce
    );
}

#[test]
fn test_resolve_policy_check_mode_unsafe_bypass_allowed_when_config_permits() {
    let mut policy = strict_policy_with_required("depth");
    policy.allow_unsafe_cli_emit = true;
    let cfg = RalphConfig {
        event_loop: ralph_core::config::EventLoopConfig {
            event_policy: Some(policy),
            ..Default::default()
        },
        ..Default::default()
    };
    let flags = PolicyCheckFlags {
        policy_check: false,
        no_policy_check: true,
    };
    assert_eq!(
        resolve_policy_check_mode(&flags, Some(&cfg)),
        PolicyCheckMode::Skip
    );
}

#[test]
fn test_resolve_policy_check_mode_no_strict_no_flags_is_skip() {
    let cfg = RalphConfig::default();
    let flags = PolicyCheckFlags {
        policy_check: false,
        no_policy_check: false,
    };
    assert_eq!(
        resolve_policy_check_mode(&flags, Some(&cfg)),
        PolicyCheckMode::Skip
    );
}

// ─────────────────────────────────────────────────────────────────────
// U15: agent context defaults to strict policy-check.
// ─────────────────────────────────────────────────────────────────────

fn agent_ctx_config() -> RalphConfig {
    // Config without `require_policy_check_for_cli_emit` — agent
    // context should still default to strict via U15.
    let yaml = r"
event_loop:
  event_policy:
    enabled: false
    mode: enforce
    allow_unsafe_cli_emit: false
";
    serde_yaml::from_str(yaml).unwrap()
}

fn agent_ctx_config_optout() -> RalphConfig {
    // Preset author opts out for agent context.
    let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    allow_unsafe_cli_emit: true
";
    serde_yaml::from_str(yaml).unwrap()
}

#[test]
fn test_u15_agent_context_enforce_even_when_config_disabled() {
    let cfg = agent_ctx_config();
    let flags = PolicyCheckFlags {
        policy_check: false,
        no_policy_check: false,
    };
    // Human CLI: skip (config says no enforce).
    assert_eq!(
        resolve_policy_check_mode_with_ctx(&flags, Some(&cfg), false),
        PolicyCheckMode::Skip
    );
    // Agent CLI: enforce despite the disabled config (U15 default).
    assert_eq!(
        resolve_policy_check_mode_with_ctx(&flags, Some(&cfg), true),
        PolicyCheckMode::Enforce
    );
}

#[test]
fn test_u15_agent_context_explicit_check_wins() {
    let cfg = agent_ctx_config();
    let flags = PolicyCheckFlags {
        policy_check: true,
        no_policy_check: false,
    };
    assert_eq!(
        resolve_policy_check_mode_with_ctx(&flags, Some(&cfg), true),
        PolicyCheckMode::ExplicitCheck
    );
}

#[test]
fn test_u15_agent_unsafe_bypass_blocked_when_disallowed() {
    let cfg = agent_ctx_config();
    let flags = PolicyCheckFlags {
        policy_check: false,
        no_policy_check: true,
    };
    // Config has `allow_unsafe_cli_emit: false` so the bypass
    // flag is rejected with Enforce even in agent context.
    assert_eq!(
        resolve_policy_check_mode_with_ctx(&flags, Some(&cfg), true),
        PolicyCheckMode::Enforce
    );
}

#[test]
fn test_u15_agent_unsafe_bypass_allowed_when_optout_set() {
    let cfg = agent_ctx_config_optout();
    let flags = PolicyCheckFlags {
        policy_check: false,
        no_policy_check: true,
    };
    // Preset opts out → agent can bypass.
    assert_eq!(
        resolve_policy_check_mode_with_ctx(&flags, Some(&cfg), true),
        PolicyCheckMode::Skip
    );
}

#[test]
fn test_u15_human_cli_unchanged_by_agent_default() {
    // Backwards compat: human CLI uses the config flag verbatim.
    let cfg = agent_ctx_config();
    let flags = PolicyCheckFlags {
        policy_check: false,
        no_policy_check: false,
    };
    // Same call as legacy resolve_policy_check_mode.
    assert_eq!(
        resolve_policy_check_mode_with_ctx(&flags, Some(&cfg), false),
        resolve_policy_check_mode(&flags, Some(&cfg))
    );
}

#[test]
fn test_enabled_event_policy_returns_none_when_disabled() {
    let cfg = RalphConfig::default();
    assert!(enabled_event_policy(Some(&cfg)).is_none());
}

#[test]
fn test_enabled_event_policy_returns_policy_when_enabled() {
    let policy = strict_policy_with_required("depth");
    let cfg = RalphConfig {
        event_loop: ralph_core::config::EventLoopConfig {
            event_policy: Some(policy),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(enabled_event_policy(Some(&cfg)).is_some());
}

#[test]
fn test_validate_batch_against_config_reports_all_missing_depth_violations() {
    let policy = strict_policy_with_required("depth");
    let tmp = TempDir::new().unwrap();
    let events = tmp.path().join("events.jsonl");
    // All 7 payloads lack `depth` → expect 7 errors, one per index.
    let payloads: Vec<String> = (0..7).map(|i| format!(r#"{{"dim":"d{i}"}}"#)).collect();
    let batch =
        validate_batch_against_config("review.wave.ready", &payloads, &policy, &events).unwrap();
    assert_eq!(batch.errors.len(), 7);
    for (i, err) in batch.errors.iter().enumerate() {
        assert_eq!(err.payload_index, i);
        assert_eq!(err.field, "depth");
        assert_eq!(err.reason_code, "missing_required_field");
        assert!(err.message.contains("depth"));
    }
}

#[test]
fn test_validate_batch_against_config_passes_with_depth() {
    let policy = strict_policy_with_required("depth");
    let tmp = TempDir::new().unwrap();
    let events = tmp.path().join("events.jsonl");
    let payloads: Vec<String> = (0..3)
        .map(|i| format!(r#"{{"dim":"d{i}","depth":"standard"}}"#))
        .collect();
    let batch =
        validate_batch_against_config("review.wave.ready", &payloads, &policy, &events).unwrap();
    assert!(batch.is_ok(), "valid payloads should produce empty errors");
}

#[test]
fn test_validate_batch_against_config_atomic_partial_rejection() {
    // Index 3 missing depth, others valid → still at least one error,
    // and the entire batch is rejected (atomicity).
    let policy = strict_policy_with_required("depth");
    let tmp = TempDir::new().unwrap();
    let events = tmp.path().join("events.jsonl");
    let mut payloads: Vec<String> = (0..5)
        .map(|i| format!(r#"{{"dim":"d{i}","depth":"standard"}}"#))
        .collect();
    payloads[3] = r#"{"dim":"d3"}"#.to_string();
    let batch =
        validate_batch_against_config("review.wave.ready", &payloads, &policy, &events).unwrap();
    assert!(!batch.is_ok());
    assert!(
        batch
            .errors
            .iter()
            .any(|e| e.payload_index == 3 && e.field == "depth")
    );
}

#[test]
fn test_validate_topic_payload_against_config_returns_none_when_ok() {
    let policy = strict_policy_with_required("depth");
    let tmp = TempDir::new().unwrap();
    let events = tmp.path().join("events.jsonl");
    let err = validate_topic_payload_against_config(
        "review.wave.ready",
        r#"{"dim":"d","depth":"standard"}"#,
        &policy,
        &events,
    )
    .unwrap();
    assert!(err.is_none());
}

#[test]
fn test_validate_topic_payload_against_config_returns_error_when_missing() {
    let policy = strict_policy_with_required("depth");
    let tmp = TempDir::new().unwrap();
    let events = tmp.path().join("events.jsonl");
    let err = validate_topic_payload_against_config(
        "review.wave.ready",
        r#"{"dim":"d"}"#,
        &policy,
        &events,
    )
    .unwrap()
    .expect("missing depth should produce an error");
    assert_eq!(err.field, "depth");
    assert_eq!(err.reason_code, "missing_required_field");
}

// ------------------------------------------------------------------
// 2026-07-06-004 plan U10: end-to-end pipeline policy-check
// exercises the handoff envelope validator through the CLI
// entry point. The pipeline preset enables
// `validate_payload: true`, so a missing `handoff_envelope`
// must surface as a missing_required_field-style rejection.
// ------------------------------------------------------------------

fn serial_like_handoff_config() -> ralph_core::config::HandoffEnvelopeConfig {
    ralph_core::config::HandoffEnvelopeConfig {
        enabled: true,
        prompt_injection: true,
        validate_payload: true,
        emit_result_summary: true,
    }
}

fn full_handoff_envelope_payload() -> String {
    r#"{
            "plan_name":"p1",
            "task_id":"t1",
            "task_key":"p1:step-2:implement",
            "step":"step-2",
            "commit_count":1,
            "changed_lines":10,
            "handoff_envelope":{
                "schema_version":"handoff-envelope.v1",
                "root_goal":"ship without regressions",
                "plan":{
                    "name":"p1",
                    "path":"docs/plans/p1.md",
                    "current_step":"step-2",
                    "completed_steps":["step-1"]
                },
                "state":{
                    "current_status":"ready_for_review",
                    "last_signal":"work.done",
                    "blocking_reason":null
                },
                "receiver_contract":{
                    "to_hat":"reviewer",
                    "must_do":["review step-2"],
                    "must_not_do":["regress step-1"],
                    "success_signal":"work.done",
                    "failure_signal":"work.failed"
                }
            }
        }"#
    .to_string()
}

#[test]
fn ce_executor_serial_policy_check_accepts_valid_handoff_envelope_payload() {
    let tmp = TempDir::new().unwrap();
    let events = tmp.path().join("events.jsonl");
    let policy = strict_policy_with_required("handoff_envelope");
    let result = validate_topic_payload_with_handoff(
        "work.done",
        &full_handoff_envelope_payload(),
        &policy,
        &events,
        &serial_like_handoff_config(),
    )
    .unwrap();
    assert!(
        result.is_none(),
        "serial-like config must accept a valid envelope; got {:?}",
        result
    );
}

#[test]
fn ce_executor_serial_policy_check_rejects_missing_handoff_envelope_payload() {
    let tmp = TempDir::new().unwrap();
    let events = tmp.path().join("events.jsonl");
    let policy = strict_policy_with_required("handoff_envelope");
    let payload =
        full_handoff_envelope_payload().replace("\"handoff_envelope\":{", "\"__stripped__\":{");
    let err = validate_topic_payload_with_handoff(
        "work.done",
        &payload,
        &policy,
        &events,
        &serial_like_handoff_config(),
    )
    .unwrap()
    .expect("missing envelope must reject");
    // Either the schema-side (required_fields) check or the
    // handoff-envelope validator side will surface. We
    // accept either; the contract is "rejected".
    assert!(
        err.reason_code == "missing_required_field" || err.message.contains("handoff_envelope"),
        "rejection must trace to handoff; got {:?}",
        err
    );
}

#[test]
fn load_policy_config_for_cli_emit_rejects_invalid_hats_file() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let root_path = root.to_path_buf();
    std::fs::create_dir_all(root.join(".ralph")).unwrap();
    std::fs::write(
        root.join(".ralph/hats.yml"),
        r#"
hats:
  executor:
    name: 123
    triggers: ["work.ready"]
"#,
    )
    .unwrap();

    let err = load_policy_config_for_cli_emit(Some(&root_path), OnConfigError::Fail, &[])
        .expect_err("invalid hat config must reject");
    let msg = err.to_string();
    assert!(
        msg.contains("Failed to parse hat config for `executor`")
            || msg.contains("`.ralph/hats.yml`"),
        "unexpected error message: {msg}"
    );
}

#[test]
fn test_emit_policy_validation_failure_json_outputs_structured_payload() {
    let failure = ValidationFailure {
        ok: false,
        error: "policy_validation_failed",
        topic: "review.wave.ready".to_string(),
        validation_errors: vec![ValidationError {
            payload_index: 0,
            field: "depth".to_string(),
            reason_code: "missing_required_field".to_string(),
            message: "Missing required field: depth".to_string(),
            ..Default::default()
        }],
    };
    // We can't easily intercept stdout in this module; instead we
    // verify the JSON shape and the text summary separately.
    let json = serde_json::to_string(&failure).unwrap();
    assert!(json.contains("\"ok\":false"));
    assert!(json.contains("\"error\":\"policy_validation_failed\""));
    assert!(json.contains("\"topic\":\"review.wave.ready\""));
    assert!(json.contains("\"field\":\"depth\""));
    assert!(json.contains("\"reason_code\":\"missing_required_field\""));
}

#[test]
fn test_emit_policy_validation_failure_text_summary_mentions_count() {
    // Build a failure with 7 errors all on `depth`.
    let errors: Vec<ValidationError> = (0..7)
        .map(|i| ValidationError {
            payload_index: i,
            field: "depth".to_string(),
            reason_code: "missing_required_field".to_string(),
            message: "Missing required field: depth".to_string(),
            ..Default::default()
        })
        .collect();
    let failure = ValidationFailure {
        ok: false,
        error: "policy_validation_failed",
        topic: "review.wave.ready".to_string(),
        validation_errors: errors,
    };
    // We assert via the JSON shape (text goes to stderr, no
    // capture). Verify the field aggregation logic by inspecting
    // the validation_errors directly.
    assert_eq!(failure.validation_errors.len(), 7);
    let all_depth = failure.validation_errors.iter().all(|e| e.field == "depth");
    assert!(all_depth);
}

#[test]
fn test_render_validation_error_repair_block_includes_field_and_command() {
    let error = ValidationError {
        payload_index: 3,
        field: "task_id".to_string(),
        reason_code: "missing_required_field".to_string(),
        message: "missing required field: task_id".to_string(),
        expected: Some("task_id".to_string()),
        field_description: Some("live task id".to_string()),
        suggested_payload_shape: Some(serde_json::json!({"task_id": "<task_id>"})),
        suggested_command: Some(
            "ralph emit work.done --policy-check -j '{\"task_id\":\"<task_id>\"}'".to_string(),
        ),
        ..Default::default()
    };
    let block = render_validation_error_repair_block("work.done", &[error])
        .expect("repair block must be present");
    assert!(block.contains("Repair hints for topic `work.done`"));
    assert!(block.contains("field `task_id`"));
    assert!(block.contains("meaning:"));
    assert!(block.contains("suggested payload shape"));
    assert!(block.contains("--policy-check"));
}

/// U4 (plan 2026-08-06-001, R2/R3): the CLI
/// `ralph emit --policy-check --output json` JSON shape
/// must carry observed / invariant / required_proof for a
/// semantic finding so the agent sees the same source of
/// truth as the loop prompt.  Mechanical findings must
/// keep their existing shape (`suggested_*` present,
/// `observed` / `invariant` / `required_proof` absent).
#[test]
fn u4_semantic_finding_carries_observed_invariant_required_proof() {
    let finding = ralph_core::PolicyFinding {
        topic: "fix.done".to_string(),
        violation_type: ViolationType::SemanticGateViolation {
            gate: "payload_consistency:fix-done-blocked".to_string(),
            context: "status=blocked requires fixes_applied > 0".to_string(),
            referenced_fields: vec!["status".into(), "fixes_applied".into()],
        },
        message: "payload_consistency rule 'fix-done-blocked' violated".to_string(),
        evidence: Some(ralph_core::correction::EvidenceDetail {
            observed: vec![
                ralph_core::correction::ObservationEntry {
                    field: "status".into(),
                    value: ralph_core::correction::ObservationValue::Value("\"blocked\"".into()),
                },
                ralph_core::correction::ObservationEntry {
                    field: "fixes_applied".into(),
                    value: ralph_core::correction::ObservationValue::Value("0".into()),
                },
            ],
            invariant: "status=blocked requires fixes_applied > 0".into(),
            proof: "rebuild from artifact and rerun ralph emit --policy-check".into(),
            synthetic: false,
            guidance: None,
            failed_check_keys: None,
        }),
    };
    let error = finding_record(&finding);
    let json = serde_json::to_value(&error).expect("ValidationError serialises");
    assert_eq!(json["reason_code"], "semantic_gate_violation");
    assert_eq!(json["gate"], "payload_consistency:fix-done-blocked");
    assert_eq!(
        json["referenced_fields"],
        serde_json::json!(["status", "fixes_applied"])
    );
    // Evidence-bound feedback surface (R2).
    assert!(json["observed"].is_array());
    let observed = json["observed"].as_array().unwrap();
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0]["field"], "status");
    assert_eq!(observed[0]["value"], "\"blocked\"");
    assert_eq!(observed[1]["field"], "fixes_applied");
    assert_eq!(observed[1]["value"], "0");
    assert_eq!(
        json["invariant"],
        "status=blocked requires fixes_applied > 0"
    );
    assert_eq!(
        json["required_proof"],
        "rebuild from artifact and rerun ralph emit --policy-check"
    );
    // R3: semantic path must NOT carry replacement
    // guidance.
    assert!(json.get("suggested_payload_shape").is_none());
    assert!(json.get("suggested_command").is_none());
}

/// U4 (plan 2026-08-06-001, R3): mechanical findings keep
/// their replacement shape — the suggestion-omission guard
/// from F-B stays a characterization (E9a) and must not
/// regress.  Observed / invariant / required_proof stay
/// `None` because the mechanical path does not carry
/// evidence.
#[test]
fn u4_mechanical_finding_omits_evidence_fields_keeps_replacement() {
    let finding = ralph_core::PolicyFinding {
        topic: "work.done".to_string(),
        violation_type: ViolationType::MissingRequiredField {
            field: "task_id".to_string(),
        },
        message: "missing required field: task_id".to_string(),
        evidence: None,
    };
    let error = finding_record(&finding);
    let json = serde_json::to_value(&error).expect("ValidationError serialises");
    assert_eq!(json["reason_code"], "missing_required_field");
    assert_eq!(json["field"], "task_id");
    // No evidence fields for mechanical findings.
    assert!(json.get("observed").is_none());
    assert!(json.get("invariant").is_none());
    assert!(json.get("required_proof").is_none());
}

/// U4 (plan 2026-08-06-001, R2): the text projection
/// (`render_validation_error_repair_block`) renders the
/// structured evidence on its own lines so the human
/// operator sees the same source of truth as the agent.
#[test]
fn u4_text_projection_renders_observed_invariant_required_proof() {
    let error = ValidationError {
        payload_index: 0,
        field: String::new(),
        reason_code: "semantic_gate_violation".to_string(),
        message: "payload_consistency:fix-done-blocked violated".to_string(),
        gate: Some("payload_consistency:fix-done-blocked".to_string()),
        referenced_fields: Some(vec!["status".into(), "fixes_applied".into()]),
        observed: Some(vec![
            serde_json::json!({"field": "status", "value": "\"blocked\""}),
            serde_json::json!({"field": "fixes_applied", "value": "0"}),
        ]),
        invariant: Some("status=blocked requires fixes_applied > 0".to_string()),
        required_proof: Some("rebuild and rerun ralph emit --policy-check".to_string()),
        ..Default::default()
    };
    let block = render_validation_error_repair_block("fix.done", &[error]).expect("block present");
    assert!(block.contains("observed: status="), "block = {block}");
    assert!(block.contains("fixes_applied="), "block = {block}");
    assert!(block.contains("invariant: status=blocked requires fixes_applied > 0"));
    assert!(block.contains("must re-prove: rebuild and rerun ralph emit --policy-check"));
}

/// U4 (plan 2026-08-06-001, R5/F-E): synthetic precheck
/// evidence renders the `gate_silent_or_ambiguous` marker
/// on the CLI projection so the agent cannot mistake the
/// absence of observations for a clean pass.
#[test]
fn u4_synthetic_evidence_renders_marker_in_cli_projection() {
    let error = ValidationError {
        payload_index: 0,
        field: String::new(),
        reason_code: "semantic_gate_violation".to_string(),
        message: "precheck gate silent".to_string(),
        gate: Some("precheck:work.done".to_string()),
        observed: Some(vec![]),
        invariant: Some("precheck gate for `work.done` was silent or ambiguous".to_string()),
        required_proof: Some(
            "Reinvestigate the gate; do not assume any checklist item passed".to_string(),
        ),
        ..Default::default()
    };
    let block = render_validation_error_repair_block("work.done", &[error]).expect("block present");
    assert!(
        block.contains("(none — gate did not return any fact-checked observations)"),
        "empty observed list must surface the absence explicitly: {block}"
    );
    assert!(block.contains("silent or ambiguous"));
    assert!(block.contains("do not assume"));
}

#[test]
fn test_build_policy_state_falls_back_to_default_on_missing_file() {
    let policy = strict_policy_with_required("depth");
    let tmp = TempDir::new().unwrap();
    let events = tmp.path().join("missing.jsonl");
    let ctx = PolicyCheckContext {
        events_file: events,
    };
    let state = build_policy_state(&policy, &ctx);
    // Default state: no terminal observed, no observed topics.
    assert!(!state.terminal_observed);
    assert!(state.observed_topics.is_empty());
    // Suppress the "unused" warning on the policy parameter.
    let _ = &policy;
}

#[test]
fn test_load_workspace_config_returns_none_when_missing() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let cfg = load_workspace_config(Some(&root), OnConfigError::Tolerate, &[]).unwrap();
    assert!(cfg.is_none());
}

#[test]
fn test_validate_batch_replays_terminal_event() {
    // Pre-seed the events file with a terminal event. The strict
    // policy + business_after_completion: reject should now reject
    // any business topic that arrives after the terminal.
    let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: false
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
    completion_after_terminal:
      duplicate_terminal: reject
      business_after_completion: reject
";
    let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let policy = cfg.event_loop.event_policy.unwrap();

    let tmp = TempDir::new().unwrap();
    let events = tmp.path().join("events.jsonl");
    std::fs::write(
        &events,
        r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
    )
    .unwrap();

    let payloads = vec![r#"{"task_key":"x"}"#.to_string()];
    let batch =
        validate_batch_against_config("experiment.planned", &payloads, &policy, &events).unwrap();
    assert!(!batch.is_ok(), "business event after terminal must reject");
    assert!(
        batch
            .errors
            .iter()
            .any(|e| e.reason_code == "terminal_monotonicity_violation"
                || e.reason_code == "business_event_after_completion"
                || e.reason_code == "duplicate_terminal_event")
    );
}

#[test]
fn test_policy_runtime_state_can_be_replayed_in_loop() {
    // Sanity check: the loop and CLI must share the same replay
    // semantics. The loop uses `from_events` directly; the CLI
    // wraps it in `build_policy_state`. The two paths must
    // produce the same `terminal_observed` for a given fixture.
    let policy = strict_policy_with_required("depth");
    let tmp = TempDir::new().unwrap();
    let events = tmp.path().join("events.jsonl");
    std::fs::write(
        &events,
        r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    let direct = PolicyRuntimeState::from_events(&events, &policy).unwrap();
    let wrapped = build_policy_state(
        &policy,
        &PolicyCheckContext {
            events_file: events,
        },
    );
    assert_eq!(direct.terminal_observed, wrapped.terminal_observed);
}

// ── U1 / 2026-06-17-005 plan ─────────────────────────────────────
// CLI step-handoff progress-task gate precheck (`check_step_handoff_gate`).
// The helper must:
//   * pass through non-gated topics without invoking the gate
//   * pass when progress.md ↔ tasks.jsonl align
//   * return `progress_task_mismatch` when the agent's claim
//     disagrees with the ledger — the same reason the loop would
//     emit on the same fixture
//   * fail-closed on non-JSON / empty payloads for gated topics
//     (review finding #6)

fn workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".ralph").join("agent")).unwrap();
    tmp
}

fn write_progress(tmp: &tempfile::TempDir, body: &str) {
    let path = tmp.path().join(".ralph").join("agent").join("progress.md");
    std::fs::write(path, body).unwrap();
}

fn write_closed_task(tmp: &tempfile::TempDir, id: &str, title: &str) {
    use ralph_core::task::{Task, TaskStatus};
    let mut task = Task::new(title.to_string(), 3);
    task.id = id.to_string();
    task.status = TaskStatus::Closed;
    let line = serde_json::to_string(&task).unwrap();
    let path = tmp.path().join(".ralph").join("agent").join("tasks.jsonl");
    let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
    existing.push_str(&line);
    existing.push('\n');
    std::fs::write(&path, existing).unwrap();
}

/// Happy path: aligned progress + tasks, `queue.advance` policy-check passes.
///
/// U1 of 2026-07-05-005 (KTD-1): the derived `current_step` accessor
/// returns `completed_steps.last()`, so the fixture must mark the
/// inbound event's step as already completed (or this would be a
/// `step_mismatch`). We re-derive the test against the new rule:
/// `## Current Step\nstep-02` plus `completed_steps: [step-01, step-02]`
/// makes the derived `current_step() == Some("step-02")` and the
/// closed task for `step-02` confirms task-step consistency.
#[test]
fn u1_step_handoff_gate_happy_path_aligned_progress() {
    let tmp = workspace();
    write_closed_task(&tmp, "task-1", "step-02");
    write_progress(
        &tmp,
        "## Current Step\nstep-02\n\n## Completed Steps\n- step-01\n- step-02\n",
    );
    let payload = r#"{"step":"step-02","task_id":"task-1"}"#;
    let res = check_step_handoff_gate("queue.advance", payload, tmp.path());
    assert!(res.is_ok(), "aligned progress must pass: {res:?}");
}

/// Error path: deliberate progress drift → CLI gate returns the
/// same `progress_task_mismatch` reason the loop would emit.
#[test]
fn u1_step_handoff_gate_rejects_misaligned_progress() {
    let tmp = workspace();
    write_closed_task(&tmp, "task-1", "step-01");
    // progress.md does NOT list step-01 → mismatch.
    write_progress(
        &tmp,
        "## Current Step\nstep-02\n\n## Completed Steps\n- step-02\n",
    );
    let payload = r#"{"step":"step-02","task_id":"task-1"}"#;
    let err = check_step_handoff_gate("queue.advance", payload, tmp.path())
        .expect_err("mismatched progress must produce validation error");
    assert_eq!(err.reason_code, "progress_task_mismatch");
    assert!(err.message.contains("task_closed_but_progress_missing"));
}

/// Edge: topic not in `GATED_TOPICS` → no gate call, no error.
#[test]
fn u1_step_handoff_gate_skips_non_gated_topic() {
    let tmp = workspace();
    // No progress.md / tasks.jsonl — gate must short-circuit.
    let payload = r#"{"step":"step-01"}"#;
    let res = check_step_handoff_gate("review.dimension.done", payload, tmp.path());
    assert!(res.is_ok(), "non-gated topic must pass without gate call");
}

/// Fail-closed alignment with review finding #6: empty payload on
/// a gated topic must surface `progress_task_mismatch`, not
/// silently pass through.
#[test]
fn u1_step_handoff_gate_fail_closed_on_empty_payload() {
    let tmp = workspace();
    let err = check_step_handoff_gate("queue.advance", "", tmp.path())
        .expect_err("empty gated payload must fail closed");
    assert_eq!(err.reason_code, "progress_task_mismatch");
    assert!(err.message.contains("non-empty"));
}

/// Fail-closed alignment with finding #6: non-JSON payload on a
/// gated topic must surface `progress_task_mismatch` instead of
/// inert-passing.
#[test]
fn u1_step_handoff_gate_fail_closed_on_non_json_payload() {
    let tmp = workspace();
    let err = check_step_handoff_gate("plan.complete", "not json at all", tmp.path())
        .expect_err("non-JSON gated payload must fail closed");
    assert_eq!(err.reason_code, "progress_task_mismatch");
}

#[test]
fn test_check_wave_dimension_assignment_no_env_returns_ok() {
    // env unset, any topic
    assert!(
        check_wave_dimension_assignment_with_env(
            "review.dimension.done",
            r#"{"dimension":"testing"}"#,
            None
        )
        .is_ok()
    );
    assert!(
        check_wave_dimension_assignment_with_env("work.done", r#"{"dimension":"testing"}"#, None)
            .is_ok()
    );
}

#[test]
fn test_check_wave_dimension_assignment_match_returns_ok() {
    assert!(
        check_wave_dimension_assignment_with_env(
            "review.dimension.done",
            r#"{"dimension":"testing"}"#,
            Some("testing")
        )
        .is_ok()
    );
}

#[test]
fn test_check_wave_dimension_assignment_mismatch_returns_err() {
    let err = check_wave_dimension_assignment_with_env(
        "review.dimension.done",
        r#"{"dimension":"correctness"}"#,
        Some("testing"),
    )
    .unwrap_err();
    assert_eq!(err.reason_code, "dimension_mismatch");
    assert!(err.message.contains("expected_dimension=testing"));
    assert!(err.message.contains("actual_dimension=correctness"));
}

#[test]
fn test_check_wave_dimension_assignment_missing_field_returns_err() {
    let err = check_wave_dimension_assignment_with_env(
        "review.dimension.done",
        r#"{"findings_count":3}"#,
        Some("testing"),
    )
    .unwrap_err();
    assert_eq!(err.reason_code, "dimension_mismatch");
    assert!(err.message.contains("actual_dimension=<missing>"));
}

#[test]
fn test_check_wave_dimension_assignment_non_json_returns_err() {
    let err = check_wave_dimension_assignment_with_env(
        "review.dimension.done",
        "not json",
        Some("testing"),
    )
    .unwrap_err();
    assert_eq!(err.reason_code, "dimension_mismatch");
    assert!(err.message.contains("actual_dimension=<missing>"));
}

/// Integration: the CLI gate and the loop gate share
/// `check_progress_task_alignment` — they MUST produce the same
/// `Mismatch.reason` for the same fixture. Drift between the two
/// would re-introduce finding #21 (CLI passes / loop rejects).
#[test]
#[allow(deprecated)]
fn u1_step_handoff_gate_matches_loop_gate_reason() {
    let tmp = workspace();
    write_closed_task(&tmp, "task-1", "step-01");
    write_progress(
        &tmp,
        "## Current Step\nstep-02\n\n## Completed Steps\n- step-02\n",
    );

    let payload = r#"{"step":"step-02","task_id":"task-1"}"#;
    let cli_err = check_step_handoff_gate("queue.advance", payload, tmp.path())
        .expect_err("CLI gate must reject");

    let decision =
        check_progress_task_alignment("queue.advance", Some("step-02"), Some("task-1"), tmp.path());
    let loop_reason = match decision {
        GateDecision::Mismatch(m) => m.reason,
        other => panic!("expected Mismatch from loop gate, got {other:?}"),
    };
    assert!(
        cli_err.message.contains(&loop_reason),
        "CLI gate message must reference the loop-gate reason `{loop_reason}`; got: {}",
        cli_err.message
    );
}

// ── U1 / 2026-06-17-003 plan ─────────────────────────────────────
// CLI isolated-mode scope precheck (`check_isolated_scope`).
// The helper must:
//   * pass through coordinator mode without effect
//   * pass when hat is None (defer to origin guard)
//   * pass when hat is ralph and topic is in RALPH_CONTROL_TOPICS
//   * pass when hat is registered and topic is in hat.publishes
//   * reject with isolated_scope_violation when hat is registered
//     and topic is NOT in hat.publishes
//   * reject with the same reason when the ralph pseudo-hat tries
//     to publish a business topic (the existing ralph-guard at
//     emit.rs catches this earlier, but the isolated-scope check
//     must still agree so the two gates compose cleanly)

fn isolated_config_with_hats(yaml_hats: &str) -> RalphConfig {
    let yaml = format!(
        r"
event_loop:
  execution_mode: isolated
hats:
{yaml_hats}
"
    );
    serde_yaml::from_str(&yaml).unwrap()
}

#[test]
fn u1_isolated_scope_happy_path_executor_publishes_work_done() {
    let cfg = isolated_config_with_hats(
        r#"
  executor:
    name: "Executor"
    triggers: ["plan.advance"]
    publishes: ["work.done", "task.resume"]
"#,
    );
    assert!(
        check_isolated_scope(Some("executor"), "work.done", &cfg).is_ok(),
        "executor publishing work.done must be allowed in isolated mode"
    );
}

#[test]
fn u1_isolated_scope_error_path_executor_publishes_debug_step() {
    let cfg = isolated_config_with_hats(
        r#"
  executor:
    name: "Executor"
    triggers: ["plan.advance"]
    publishes: ["work.done", "task.resume"]
"#,
    );
    let err = check_isolated_scope(Some("executor"), "debug.step", &cfg)
        .expect_err("executor publishing debug.step must be rejected in isolated mode");
    assert_eq!(err.reason_code, "isolated_scope_violation");
    assert!(err.message.contains("executor"));
    assert!(err.message.contains("debug.step"));
    assert!(err.message.contains("work.done"));
}

#[test]
fn u1_isolated_scope_coordinator_mode_is_noop() {
    let yaml = r#"
event_loop:
  execution_mode: coordinator
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#;
    let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    // Any hat + any topic in coordinator mode → Ok.
    assert!(check_isolated_scope(Some("executor"), "debug.step", &cfg).is_ok());
    assert!(check_isolated_scope(Some("unknown-hat"), "anything", &cfg).is_ok());
}

#[test]
fn u1_isolated_scope_ralph_hat_control_topic_allowed() {
    let cfg = isolated_config_with_hats(
        r#"
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#,
    );
    // ralph + LOOP_COMPLETE → Ok (control topic whitelist).
    assert!(check_isolated_scope(Some("ralph"), "LOOP_COMPLETE", &cfg).is_ok());
    assert!(check_isolated_scope(Some("ralph"), "task.resume", &cfg).is_ok());
    // 2026-06-28-005: human.guidance is no longer a control topic.
    // Use plan.blocked instead — it is the structured terminal
    // orchestrator topic and ships with the same allowlist
    // semantics.
    assert!(check_isolated_scope(Some("ralph"), "plan.blocked", &cfg).is_ok());
}

#[test]
fn u1_isolated_scope_ralph_hat_business_topic_rejected() {
    let cfg = isolated_config_with_hats(
        r#"
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#,
    );
    // ralph + business topic → Err. The existing ralph-guard in
    // emit.rs already rejects this; the isolated-scope check
    // independently agrees (no double-reject at the gate level,
    // since emit.rs's ralph-guard runs first and bails before
    // reaching the isolated-scope check).
    let err = check_isolated_scope(Some("ralph"), "review.passed", &cfg)
        .expect_err("ralph + business topic must be rejected");
    assert_eq!(err.reason_code, "isolated_scope_violation");
}

#[test]
fn u1_isolated_scope_no_hat_passes() {
    let cfg = isolated_config_with_hats(
        r#"
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#,
    );
    // hat == None → defer to origin guard; check returns Ok.
    assert!(check_isolated_scope(None, "debug.step", &cfg).is_ok());
}

/// P1 (testing reviewer, plan §U1 test-scenarios Edge): an unknown
/// hat in isolated mode must be rejected with `isolated_scope_violation`
/// and `allowed: []` in the message. `HatRegistry::can_publish` is
/// fail-closed for unknown hats, so the new precheck agrees with the
/// loop's runtime scope guard. Without this test, the
/// `config.hats.get(hat_id).map(|c| c.publishes.clone()).unwrap_or_default()`
/// branch on line 444 is untested.
#[test]
fn u1_isolated_scope_unknown_hat_rejected() {
    let cfg = isolated_config_with_hats(
        r#"
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#,
    );
    let err = check_isolated_scope(Some("ghost-hat"), "work.done", &cfg)
        .expect_err("unknown hat in isolated mode must be rejected (fail-closed)");
    assert_eq!(err.reason_code, "isolated_scope_violation");
    assert!(
        err.message.contains("ghost-hat"),
        "message must name the offending hat, got: {}",
        err.message
    );
    assert!(
        err.message.contains("work.done"),
        "message must name the offending topic, got: {}",
        err.message
    );
    assert!(
        err.message.contains("[]"),
        "message must show empty allowed_publishes for unknown hat, got: {}",
        err.message
    );
}

// -----------------------------------------------------------------
// 2026-06-17-004 plan U1 (R1, R2): unit tests for check_emit_provenance.
// -----------------------------------------------------------------

fn isolated_config_minimal() -> RalphConfig {
    let yaml = r#"
event_loop:
  execution_mode: isolated
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#;
    serde_yaml::from_str(yaml).unwrap()
}

#[test]
fn u1_check_emit_provenance_no_hat_business_topic_rejected() {
    let cfg = isolated_config_minimal();
    let err = check_emit_provenance(None, "review.passed", &cfg)
        .expect_err("isolated + no hat + business topic must be rejected");
    assert_eq!(err.reason_code, "missing_provenance");
    assert!(err.message.contains("review.passed"));
    assert!(err.message.contains("--hat"));
}

#[test]
fn u1_check_emit_provenance_with_hat_passes() {
    let cfg = isolated_config_minimal();
    // Hat present → defer to scope guard, this gate returns Ok.
    assert!(check_emit_provenance(Some("executor"), "work.done", &cfg).is_ok());
    assert!(check_emit_provenance(Some("executor"), "review.passed", &cfg).is_ok());
}

#[test]
fn u1_check_emit_provenance_control_topics_allowed_without_hat() {
    let cfg = isolated_config_minimal();
    // Control topics are produced by the loop / runtime pseudo-hat.
    // 2026-06-28-005: human.guidance was removed from this
    // list; plan.blocked is the new structured terminal
    // orchestrator topic and ships with the same allowlist
    // semantics.
    for topic in [
        "LOOP_COMPLETE",
        "loop.cancel",
        "task.resume",
        "plan.blocked",
    ] {
        assert!(
            check_emit_provenance(None, topic, &cfg).is_ok(),
            "control topic '{topic}' must be allowed without hat"
        );
    }
}

#[test]
fn u1_check_emit_provenance_diagnostic_topics_allowed_without_hat() {
    let cfg = isolated_config_minimal();
    // Orchestrator diagnostics (event.* allowlist) are emitted by
    // the loop itself.
    for topic in [
        "event.malformed",
        "event.scope_violation",
        "event.isolation.boundary_violation",
        "event.payload_contract.rejected",
    ] {
        assert!(
            check_emit_provenance(None, topic, &cfg).is_ok(),
            "diagnostic topic '{topic}' must be allowed without hat"
        );
    }
}

#[test]
fn u1_check_emit_provenance_coordinator_mode_noop() {
    let yaml = r#"
event_loop:
  execution_mode: coordinator
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
"#;
    let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    // Coordinator mode does not require provenance — the existing
    // blanket check (L424-436 in emit.rs) governs.
    assert!(check_emit_provenance(None, "review.passed", &cfg).is_ok());
    assert!(check_emit_provenance(None, "debug.step", &cfg).is_ok());
}

// ──────────────────────────────────────────────────────────
// 2026-07-06-004 fix-plan U2: CLI integration test that
// `ralph emit --policy-check --output json` returns a JSON
// object whose `handoff_envelope` summary is populated when
// the typed `emit_result_summary` flag is on AND the payload
// contains a valid envelope. The summary carries 4 fields
// (schema_version / to_hat / success_signal / failure_signal)
// so the agent can confirm the envelope was recognised in
// the same CLI round-trip.
#[test]
fn build_emit_result_parts_attaches_handoff_envelope_summary_when_enabled() {
    use ralph_core::config::{EventLoopConfig, EventPolicyConfig, HandoffEnvelopeConfig};

    let envelope_payload = serde_json::json!({
        "handoff_envelope": {
            "schema_version": "handoff-envelope.v1",
            "root_goal": "ship step-01 cleanly",
            "plan": {
                "name": "plan-u2",
                "path": "docs/plans/u2.md",
                "current_step": "step-01",
                "completed_steps": []
            },
            "state": {
                "current_status": "ready_for_review",
                "last_signal": "work.done",
                "blocking_reason": null
            },
            "receiver_contract": {
                "to_hat": "validator",
                "must_do": ["run full suite"],
                "success_signal": "test.passed",
                "failure_signal": "test.failed"
            }
        }
    });
    let payload_str = envelope_payload.to_string();

    let cfg = RalphConfig {
        event_loop: EventLoopConfig {
            handoff_envelope: HandoffEnvelopeConfig {
                enabled: true,
                prompt_injection: true,
                validate_payload: true,
                emit_result_summary: true,
            },
            event_policy: Some(EventPolicyConfig::default()),
            ..EventLoopConfig::default()
        },
        ..RalphConfig::default()
    };

    let tmp = TempDir::new().unwrap();
    let parts = build_emit_result_parts(
        "work.done".to_string(),
        true,
        false,
        Vec::new(),
        Some(&cfg),
        tmp.path(),
        Some("coordinator"),
        None,
        Some(&payload_str),
    );

    // U2: when the typed `emit_result_summary` flag is on
    // AND the payload contains a valid envelope, the
    // summary must be populated with the 4 expected
    // fields.
    let summary = parts
        .handoff_envelope
        .as_ref()
        .expect("summary must be populated when emit_result_summary=true");
    assert_eq!(summary.schema_version, "handoff-envelope.v1");
    assert_eq!(summary.to_hat, "validator");
    assert_eq!(summary.success_signal, "test.passed");
    assert_eq!(summary.failure_signal, "test.failed");

    // Edge: rejected payload (ok=false) must surface as
    // `None` because the assemble layer's forced-clear
    // logic overwrites the field when `ok=false`.
    let parts_reject = build_emit_result_parts(
        "work.done".to_string(),
        false,
        false,
        vec![ralph_core::emit_result::EmitError {
            code: "x".to_string(),
            message: "y".to_string(),
            field: None,
            suggested_command: None,
            ..ralph_core::emit_result::EmitError::default()
        }],
        Some(&cfg),
        tmp.path(),
        Some("coordinator"),
        None,
        Some(&payload_str),
    );
    assert!(
        parts_reject.handoff_envelope.is_none(),
        "rejection paths must not surface a handoff_envelope summary: {:?}",
        parts_reject.handoff_envelope
    );

    // Edge: missing envelope payload → summary is None
    // even when the typed flag is on.
    let parts_no_env = build_emit_result_parts(
        "work.done".to_string(),
        true,
        false,
        Vec::new(),
        Some(&cfg),
        tmp.path(),
        Some("coordinator"),
        None,
        Some(r#"{"plan_name":"no-envelope"}"#),
    );
    assert!(
        parts_no_env.handoff_envelope.is_none(),
        "missing envelope payload must NOT populate summary: {:?}",
        parts_no_env.handoff_envelope
    );

    // Edge: typed flag off → summary is None even with a
    // valid envelope in the payload.
    let mut cfg_off = cfg.clone();
    cfg_off.event_loop.handoff_envelope.emit_result_summary = false;
    let parts_flag_off = build_emit_result_parts(
        "work.done".to_string(),
        true,
        false,
        Vec::new(),
        Some(&cfg_off),
        tmp.path(),
        Some("coordinator"),
        None,
        Some(&payload_str),
    );
    assert!(
        parts_flag_off.handoff_envelope.is_none(),
        "emit_result_summary=false must NOT populate summary: {:?}",
        parts_flag_off.handoff_envelope
    );
}

// 2026-07-09-001 plan (U3): enrichment helper tests.

/// U3 happy path: `missing_required_field` on a field that
/// has `field_docs.<f>.meaning` produces an enriched error
/// that includes the meaning and a `suggested_payload_shape`
/// — but no fabricated business value.
#[test]
fn u3_enriched_validation_error_missing_required_field_uses_field_doc() {
    let mut schema = EventSchema::default();
    schema.required_fields.push("task_id".to_string());
    schema.field_docs.insert(
        "task_id".to_string(),
        EventFieldDoc {
            meaning: "live task id from ralph tools task list".to_string(),
            source: "ralph tools task list".to_string(),
            fill_rule: "do NOT hand-write".to_string(),
        },
    );
    let err = ValidationError {
        payload_index: 0,
        field: "task_id".to_string(),
        reason_code: "missing_required_field".to_string(),
        message: "missing required field: task_id".to_string(),
        ..Default::default()
    };
    let enriched = enrich_validation_error_with_topic(
        err,
        "work.done",
        None,
        Some(&serde_json::json!({})),
        Some(&schema),
    );
    assert_eq!(enriched.expected.as_deref(), Some("task_id"));
    assert_eq!(
        enriched.field_description.as_deref(),
        Some("live task id from ralph tools task list")
    );
    let shape = enriched
        .suggested_payload_shape
        .as_ref()
        .expect("shape must be present for field-level missing");
    let s = shape.to_string();
    assert!(
        s.contains("task_id"),
        "shape must reference the field, got: {s}"
    );
    let cmd = enriched
        .suggested_command
        .as_deref()
        .expect("command must be present for field-level missing");
    assert!(cmd.contains("ralph emit work.done"));
    assert!(cmd.contains("--policy-check"));
}

/// U3 error path: `invalid_field_value` carries the
/// `allowed_values` list as `expected` and the actual
/// value as `actual`.
#[test]
fn u3_enriched_validation_error_invalid_value_uses_allowed_values() {
    let mut schema = EventSchema::default();
    schema.allowed_values.insert(
        "verdict".to_string(),
        vec![serde_json::json!("pass"), serde_json::json!("blocked")],
    );
    let err = ValidationError {
        payload_index: 0,
        field: "verdict".to_string(),
        reason_code: "invalid_field_value".to_string(),
        message: "verdict = 'bogus' not in allowed list".to_string(),
        ..Default::default()
    };
    let enriched = enrich_validation_error_with_topic(
        err,
        "review.accepted",
        None,
        Some(&serde_json::json!({"verdict": "bogus"})),
        Some(&schema),
    );
    let expected = enriched.expected.as_deref().expect("expected must be set");
    assert!(expected.contains("pass"));
    assert!(expected.contains("blocked"));
    assert_eq!(enriched.actual.as_deref(), Some("\"bogus\""));
}

/// U3 error path: when a matching `hat_allowed_values`
/// rule exists, enrichment prefers the hat-specific
/// allowed-value list over the generic `allowed_values`.
#[test]
fn u3_enriched_validation_error_uses_hat_allowed_values_when_present() {
    let mut schema = EventSchema::default();
    schema
        .allowed_values
        .insert("verdict".to_string(), vec![serde_json::json!("generic")]);
    schema.hat_allowed_values.insert(
        "verdict".to_string(),
        vec![ralph_core::config::HatAllowedValues {
            hat_id: "reviewer".to_string(),
            values: vec![serde_json::json!("pass"), serde_json::json!("blocked")],
        }],
    );
    let err = ValidationError {
        payload_index: 0,
        field: "verdict".to_string(),
        reason_code: "invalid_field_value".to_string(),
        message: "verdict = 'bogus' not in allowed list".to_string(),
        ..Default::default()
    };
    let enriched = enrich_validation_error_with_topic(
        err,
        "review.accepted",
        Some("reviewer"),
        Some(&serde_json::json!({"verdict": "bogus"})),
        Some(&schema),
    );
    let expected = enriched.expected.as_deref().expect("expected must be set");
    assert!(expected.contains("pass"));
    assert!(expected.contains("blocked"));
    assert!(!expected.contains("generic"));
}

/// U3 error path: `payload_type_mismatch` produces an
/// `expected` payload-type string and does NOT fabricate a
/// field description (no field to describe).
#[test]
fn u3_enriched_validation_error_payload_type_mismatch_has_no_field_doc() {
    use ralph_core::config::PayloadType;
    let schema = EventSchema {
        payload: Some(PayloadType::JsonObject),
        required_fields: vec!["task_id".to_string()],
        ..Default::default()
    };
    let err = ValidationError {
        payload_index: 0,
        field: String::new(),
        reason_code: "payload_type_mismatch".to_string(),
        message: "expected json_object".to_string(),
        ..Default::default()
    };
    let enriched = enrich_validation_error_with_topic(
        err,
        "work.done",
        None,
        Some(&serde_json::json!("a string")),
        Some(&schema),
    );
    assert!(
        enriched
            .expected
            .as_deref()
            .unwrap()
            .contains("json_object")
    );
    assert!(
        enriched.field_description.is_none(),
        "payload-level violation must not fabricate a field doc"
    );
}

/// U3 batch path: `payload_index` survives enrichment
/// unchanged.
#[test]
fn u3_enriched_validation_error_preserves_payload_index() {
    let err = ValidationError {
        payload_index: 3,
        field: "depth".to_string(),
        reason_code: "missing_required_field".to_string(),
        message: "missing depth".to_string(),
        ..Default::default()
    };
    let enriched =
        enrich_validation_error_with_topic(err, "review.dimensions.complete", None, None, None);
    assert_eq!(enriched.payload_index, 3);
}

/// U3 privacy: enriched JSON never includes the absolute
/// workspace path. We don't have access to the workspace
/// inside the helper, so this test pins the contract
/// indirectly: the helper has no Path argument, so a
/// workspace path cannot leak in.
#[test]
fn u3_enriched_validation_error_helper_has_no_workspace_arg() {
    // If the signature ever grows a workspace path,
    // this compile-time check will fail.
    let _signature: fn(
        ValidationError,
        &str,
        Option<&str>,
        Option<&serde_json::Value>,
        Option<&EventSchema>,
    ) -> ValidationError = enrich_validation_error_with_topic;
}

/// U3 backward compatibility: the old
/// `payload_index` / `field` / `reason_code` / `message`
/// fields are still serialised. We pin the JSON shape so
/// existing JSON consumers (ralph-bot, agent scripts)
/// keep working.
#[test]
fn u3_enriched_validation_error_preserves_legacy_json_shape() {
    let err = ValidationError {
        payload_index: 0,
        field: "task_id".to_string(),
        reason_code: "missing_required_field".to_string(),
        message: "missing required field: task_id".to_string(),
        ..Default::default()
    };
    let v = serde_json::to_value(&err).expect("serialize ValidationError");
    assert_eq!(v["payload_index"], serde_json::json!(0));
    assert_eq!(v["field"], serde_json::json!("task_id"));
    assert_eq!(
        v["reason_code"],
        serde_json::json!("missing_required_field")
    );
    assert_eq!(
        v["message"],
        serde_json::json!("missing required field: task_id")
    );
    // Optional fields are skipped when None.
    assert!(v.get("expected").is_none());
    assert!(v.get("actual").is_none());
    assert!(v.get("field_description").is_none());
    assert!(v.get("suggested_payload_shape").is_none());
    assert!(v.get("suggested_command").is_none());
}

// 2026-07-09-001 plan (U4): wiring tests.

/// U4 happy path: a `PolicyCheckReport` with one
/// `validation_errors` item gets enriched when the
/// caller passes a schema with `field_docs.task_id` and
/// the original payload. The first validation_error
/// becomes a `missing_required_field`-style error with
/// the meaning / suggested shape / suggested command
/// fields populated by `enrich_report_with_schema`.
#[test]
fn u4_enrich_report_with_schema_populates_missing_required_field() {
    use ralph_core::config::EventFieldDoc;
    let mut schema = EventSchema::default();
    schema.required_fields.push("task_id".to_string());
    schema.field_docs.insert(
        "task_id".to_string(),
        EventFieldDoc {
            meaning: "live task id".to_string(),
            source: "ralph tools task list".to_string(),
            fill_rule: String::new(),
        },
    );
    // The unified pipeline stamps a `missing_required_field`-like
    // error with `field` empty when running with the
    // engine result that the existing code path returns.
    // U4 wiring tests this by simulating the same shape.
    let mut report = PolicyCheckReport {
        topic: "work.done".to_string(),
        hat: None,
        workspace: std::path::PathBuf::from("/tmp"),
        accepted: false,
        reason_codes: vec!["missing_required_field".to_string()],
        suggestions: vec![String::new()],
        post_commit_rejected: false,
        validation_errors: vec![ValidationError {
            payload_index: 0,
            field: "task_id".to_string(),
            reason_code: "missing_required_field".to_string(),
            message: "missing required field: task_id".to_string(),
            ..Default::default()
        }],
    };
    report = enrich_report_with_schema(
        report,
        "work.done",
        None,
        Some(&serde_json::json!({})),
        Some(&schema),
    );
    let err = &report.validation_errors[0];
    assert_eq!(err.field, "task_id");
    assert_eq!(err.expected.as_deref(), Some("task_id"));
    assert_eq!(err.field_description.as_deref(), Some("live task id"));
    assert!(err.suggested_payload_shape.is_some());
    let cmd = err.suggested_command.as_deref().expect("cmd present");
    assert!(cmd.contains("ralph emit work.done"));
}

/// U4 backward compatibility: a `PolicyCheckReport`
/// produced before U4 (no `validation_errors`) flows
/// through `report_to_emit_result` using the legacy
/// `map_policy_report_to_errors` path. The legacy JSON
/// shape does not include the U3 enrichment fields.
#[test]
fn u4_report_to_emit_result_falls_back_to_legacy_path() {
    let report = PolicyCheckReport {
        topic: "work.done".to_string(),
        hat: None,
        workspace: std::path::PathBuf::from("/tmp"),
        accepted: false,
        reason_codes: vec!["missing_required_field".to_string()],
        suggestions: vec![String::new()],
        post_commit_rejected: false,
        validation_errors: vec![],
    };
    let result = report_to_emit_result(&report, None);
    assert!(!result.ok, "rejection must produce ok=false");
    let v = serde_json::to_value(&result).expect("serialize EmitResult");
    let errors = v["errors"].as_array().expect("errors must be array");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["code"], "missing_required_field");
}

/// U4 happy path: when `validation_errors` is populated,
/// `report_to_emit_result` uses them (not the legacy
/// `reason_codes` flattening) so the agent sees the
/// full U3 enrichment JSON (`field` / `expected` /
/// `actual` / `field_description` /
/// `suggested_payload_shape` / `suggested_command`).
///
/// 2026-07-09-001 plan (U4): after U3 extends
/// `EmitError`, this fixture populates every U3 field
/// so the additive invariant is no longer silent — a
/// future U3 regression would skip one of these
/// assertions, not just `code` / `suggested_command`.
#[test]
fn u4_report_to_emit_result_uses_validation_errors_when_present() {
    let report = PolicyCheckReport {
        topic: "work.done".to_string(),
        hat: None,
        workspace: std::path::PathBuf::from("/tmp"),
        accepted: false,
        // Legacy code + suggestion must be ignored.
        reason_codes: vec!["should_be_ignored".to_string()],
        suggestions: vec!["should_be_ignored".to_string()],
        post_commit_rejected: false,
        validation_errors: vec![ValidationError {
            payload_index: 0,
            field: "task_id".to_string(),
            reason_code: "missing_required_field".to_string(),
            message: "missing required field: task_id".to_string(),
            suggested_command: Some(
                "ralph emit work.done --policy-check -j '{\"task_id\":\"<task_id>\"}'".to_string(),
            ),
            expected: Some("task_id".to_string()),
            actual: None,
            field_description: Some("the live task id from `ralph tools task list`".to_string()),
            suggested_payload_shape: Some(serde_json::json!({"task_id": "<id>"})),
            ..Default::default()
        }],
    };
    let result = report_to_emit_result(&report, None);
    let v = serde_json::to_value(&result).expect("serialize EmitResult");
    let errors = v["errors"].as_array().expect("errors must be array");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["code"], "missing_required_field");
    assert_eq!(errors[0]["field"], "task_id");
    assert!(
        errors[0]["suggested_command"]
            .as_str()
            .unwrap()
            .contains("--policy-check")
    );
    // U4 (2026-07-09-001): U3 enrichment fields survive
    // the validation_errors_to_emit_errors round trip.
    assert_eq!(
        errors[0]["expected"],
        serde_json::json!("task_id"),
        "expected must propagate as a JSON string (ValidationError.String → Value::String)"
    );
    assert!(
        errors[0].get("actual").is_none(),
        "None actual must skip serialise (no `\"actual\": null`)"
    );
    assert_eq!(
        errors[0]["field_description"],
        "the live task id from `ralph tools task list`"
    );
    assert_eq!(
        errors[0]["suggested_payload_shape"],
        serde_json::json!({"task_id": "<id>"})
    );
}

/// U4 (2026-07-09-001): regression guard — when a
/// `ValidationError`'s U3 fields are `None`, the JSON
/// envelope must omit the JSON keys entirely (not
/// `"expected": null`). This pins the
/// skip-serializing-if-None invariant; a future caller
/// that reassigns `expected` to `Some(serde_json::Value::Null)`
/// would silently break agents reading JSON.
#[test]
fn u4_report_to_emit_result_omits_none_enrichment_fields() {
    let report = PolicyCheckReport {
        topic: "work.done".to_string(),
        hat: None,
        workspace: std::path::PathBuf::from("/tmp"),
        accepted: false,
        reason_codes: vec!["should_be_ignored".to_string()],
        suggestions: vec!["should_be_ignored".to_string()],
        post_commit_rejected: false,
        validation_errors: vec![ValidationError {
            payload_index: 0,
            field: "task_id".to_string(),
            reason_code: "missing_required_field".to_string(),
            message: "missing required field: task_id".to_string(),
            suggested_command: None,
            // All U3 enrichment fields default to None.
            ..Default::default()
        }],
    };
    let result = report_to_emit_result(&report, None);
    let v = serde_json::to_value(&result).expect("serialize EmitResult");
    let err_obj = &v["errors"][0];
    assert!(err_obj.get("expected").is_none());
    assert!(err_obj.get("actual").is_none());
    assert!(err_obj.get("field_description").is_none());
    assert!(err_obj.get("suggested_payload_shape").is_none());
}

// 2026-07-09-001 plan (U5): batch/wave enrichment tests.

/// U5 batch happy path: a `ValidationFailure::from_batch`
/// whose `validation_errors` carry a `payload_index` gets
/// matched to the index-matched payload when enriched.
/// The original 4-field shape (`field` / `reason_code` /
/// `message` / `payload_index`) survives; the new U3
/// fields (`expected` / `field_description` /
/// `suggested_payload_shape` / `suggested_command`) are
/// filled when the schema has field_docs.
#[test]
fn u5_enrich_with_schema_populates_per_payload_field_doc() {
    use ralph_core::config::EventFieldDoc;
    let mut schema = EventSchema::default();
    schema.required_fields.push("depth".to_string());
    schema.field_docs.insert(
        "depth".to_string(),
        EventFieldDoc {
            meaning: "review dimension depth".to_string(),
            source: "preset config".to_string(),
            fill_rule: String::new(),
        },
    );
    let failure = ValidationFailure {
        ok: false,
        error: "policy_validation_failed",
        topic: "review.dimensions.complete".to_string(),
        validation_errors: vec![
            ValidationError {
                payload_index: 0,
                field: "depth".to_string(),
                reason_code: "missing_required_field".to_string(),
                message: "missing depth".to_string(),
                ..Default::default()
            },
            ValidationError {
                payload_index: 3,
                field: "depth".to_string(),
                reason_code: "missing_required_field".to_string(),
                message: "missing depth at index 3".to_string(),
                ..Default::default()
            },
        ],
    };
    let payloads = vec![
        serde_json::json!({}),
        serde_json::json!({"x": 1}),
        serde_json::json!({"y": 2}),
        serde_json::json!({}),
    ];
    let failure =
        failure.enrich_with_schema("review.dimensions.complete", None, &payloads, Some(&schema));
    // payload_index is preserved (the U5 / SC2a contract).
    assert_eq!(failure.validation_errors[0].payload_index, 0);
    assert_eq!(failure.validation_errors[1].payload_index, 3);
    // Both items got enriched because both reference a field
    // with field_docs.
    assert_eq!(
        failure.validation_errors[0].field_description.as_deref(),
        Some("review dimension depth")
    );
    assert_eq!(
        failure.validation_errors[1].field_description.as_deref(),
        Some("review dimension depth")
    );
}

/// U5 privacy: enriched JSON never includes the absolute
/// workspace path. We don't have access to the workspace
/// inside the helper, so this test pins the contract
/// indirectly: the helper has no Path argument.
#[test]
fn u5_enrich_with_schema_has_no_workspace_arg() {
    let _signature: fn(
        ValidationFailure,
        &str,
        Option<&str>,
        &[serde_json::Value],
        Option<&EventSchema>,
    ) -> ValidationFailure = |f, t, hat, p, s| f.enrich_with_schema(t, hat, p, s);
}

/// U5 backward compatibility: enriching a
/// `ValidationFailure` whose schema has no `field_docs`
/// still produces a stable JSON shape (no panic, no
/// fabricated field description).
#[test]
fn u5_enrich_with_schema_no_field_docs_is_safe() {
    let schema = EventSchema::default();
    let failure = ValidationFailure {
        ok: false,
        error: "policy_validation_failed",
        topic: "work.done".to_string(),
        validation_errors: vec![ValidationError {
            payload_index: 0,
            field: "task_id".to_string(),
            reason_code: "missing_required_field".to_string(),
            message: "missing required field: task_id".to_string(),
            ..Default::default()
        }],
    };
    let payloads = vec![serde_json::json!({})];
    let failure = failure.enrich_with_schema("work.done", None, &payloads, Some(&schema));
    assert!(
        failure.validation_errors[0].field_description.is_none(),
        "no field_docs must produce no field_description"
    );
    // `expected` is still filled: that's the bare field
    // name, which is safe.
    assert_eq!(
        failure.validation_errors[0].expected.as_deref(),
        Some("task_id")
    );
}

/// U5 batch safety: when an error has an out-of-range
/// `payload_index` (e.g. a clock-skewed batch where the
/// operator passes `payloads.len() < expected`), the
/// helper must NOT panic. It silently uses `Value::Null`
/// for the payload lookup so the rest of the enrichment
/// still works.
#[test]
fn u5_enrich_with_schema_out_of_range_payload_index_is_safe() {
    use ralph_core::config::EventFieldDoc;
    let mut schema = EventSchema::default();
    schema.field_docs.insert(
        "task_id".to_string(),
        EventFieldDoc {
            meaning: "live id".to_string(),
            source: String::new(),
            fill_rule: String::new(),
        },
    );
    let failure = ValidationFailure {
        ok: false,
        error: "policy_validation_failed",
        topic: "work.done".to_string(),
        validation_errors: vec![ValidationError {
            payload_index: 99,
            field: "task_id".to_string(),
            reason_code: "missing_required_field".to_string(),
            message: "missing required field: task_id".to_string(),
            ..Default::default()
        }],
    };
    let payloads = vec![serde_json::json!({})];
    let failure = failure.enrich_with_schema("work.done", None, &payloads, Some(&schema));
    assert_eq!(failure.validation_errors[0].payload_index, 99);
    assert_eq!(
        failure.validation_errors[0].field_description.as_deref(),
        Some("live id"),
        "schema lookup must still succeed even when payload_index is OOR"
    );
}

// 2026-07-09-001 plan (U6 / C1+M1+T2):
// pin the dead-code removal in
// `enrich_validation_error`'s `missing_required_field`
// branch. The pre-fix code accidentally assigned
// suggested_command to an inline placeholder that
// contained `error.field` as the topic name and
// immediately re-assigned None. The behaviour was
// correct (None at the end) but the trap was easy to
// misread. These two tests pin:
//
// - the plain `enrich_validation_error` keeps
//   `suggested_command == None` after the field is
//   populated.
// - the wrapper
//   `enrich_validation_error_with_topic` regenerates
//   `suggested_command` from the real topic, so any
//   future regression that re-adds the inline
//   placeholder shows up as wrong-topic noise in the
//   command string.

fn schema_with_task_id_field() -> EventSchema {
    use ralph_core::config::EventFieldDoc;
    let mut schema = EventSchema::default();
    schema.required_fields.push("task_id".to_string());
    schema.field_docs.insert(
        "task_id".to_string(),
        EventFieldDoc {
            meaning: "live task id from `ralph tools task list`".to_string(),
            source: "preset".to_string(),
            fill_rule: "do NOT hand-write".to_string(),
        },
    );
    schema
}

#[test]
fn enrich_validation_error_keeps_suggested_command_none_missing_branch() {
    // U6 happy: enrich_validation_error (no topic
    // wrapper) must NOT touch `suggested_command` in
    // the `missing_required_field` branch — the
    // downstream `enrich_validation_error_with_topic`
    // wrapper owns that field. The pre-fix code path
    // (assignment-then-immediate-reassignment) also
    // produced None at the end; this test pins the
    // final invariant so a future refactor that
    // removes the immediate reassignment without
    // deleting the assignment fails CI rather than
    // shipping a misleading inline placeholder.
    let schema = schema_with_task_id_field();
    let err = ValidationError {
        payload_index: 0,
        field: "task_id".to_string(),
        reason_code: "missing_required_field".to_string(),
        message: "missing required field: task_id".to_string(),
        ..Default::default()
    };
    let enriched = enrich_validation_error(err, None, None, Some(&schema));
    assert!(
        enriched.suggested_command.is_none(),
        "enrich_validation_error (no topic wrapper) must keep suggested_command == None \
             for the missing_required_field branch; got {:?}",
        enriched.suggested_command
    );
    assert!(
        enriched.suggested_payload_shape.is_some(),
        "missing_required_field enrichment still surfaces the schema-aware payload shape"
    );
    assert_eq!(
        enriched.field_description.as_deref(),
        Some("live task id from `ralph tools task list`")
    );
}

#[test]
fn enrich_validation_error_with_topic_regenerates_suggested_command() {
    // U6 negative: the wrapper
    // `enrich_validation_error_with_topic` must
    // produce a `suggested_command` that uses the
    // real topic (e.g. `review.complete`), NOT the
    // inline field-name placeholder the pre-fix code
    // path would have produced. Guards against future
    // edits that resurrect the dead-code inline
    // placeholder.
    let schema = schema_with_task_id_field();
    let err = ValidationError {
        payload_index: 0,
        field: "task_id".to_string(),
        reason_code: "missing_required_field".to_string(),
        message: "missing required field: task_id".to_string(),
        ..Default::default()
    };
    let enriched = enrich_validation_error_with_topic(
        err,
        "review.complete",
        None,
        Some(&serde_json::json!({})),
        Some(&schema),
    );
    let suggested = enriched
        .suggested_command
        .as_deref()
        .expect("topic wrapper must populate suggested_command");
    assert!(
        suggested.contains("ralph emit review.complete"),
        "topic wrapper must use the real topic, not the field name. got: {suggested}"
    );
    assert!(
        !suggested.contains("ralph emit task_id "),
        "legacy inline-placeholder regression. got: {suggested}"
    );
    assert!(
        suggested.contains("--policy-check"),
        "suggested_command must advertise --policy-check so the agent re-prechecks"
    );
}
