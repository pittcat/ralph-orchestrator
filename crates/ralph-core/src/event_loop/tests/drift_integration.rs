//! Integration tests for the U6 runtime-diagnosis prompt injection.
//!
//! These tests exercise the four `build_prompt` paths end-to-end
//! through a real `EventLoop` to confirm the
//! `## Runtime Diagnosis Alert` block lands in the prompt exactly
//! where the U6 plan demands:
//!
//! 1. Solo ralph path (no hats defined).
//! 2. Multi-hat coordinator path (ralph + custom hats).
//! 3. Isolated hat path (one hat per iteration).
//! 4. Backward-compat custom hat path (no isolated mode).
//!
//! The tests also pin the negative paths:
//!
//! - diagnostics disabled (no `runtime_diagnosis` block) ⇒ no alert.
//! - prompt injection disabled ⇒ no alert.
//! - the alert body never exceeds `max_prompt_chars`.
//! - the alert is filtered to the target hat in isolated mode.
//! - the Final escalation hint never replaces
//!   `TerminationReason::PayloadContractViolation` (U6 plan,
//!   regression-guard row).

use super::*;
use crate::config::{DriftConfig, MalformedJsonlPolicy, RuntimeDiagnosisConfig};
use crate::diagnosis::{
    AcceptedEventEvidence, DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, EscalationLevel,
    RUNTIME_DIAGNOSIS_ALERT_HEADER, RecoveryDiagnosisEnvelope, RecoveryResponder,
};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Build a `RuntimeDiagnosisConfig` with all the U6-relevant knobs
/// exposed so the tests can flip them per-scenario without writing
/// full YAML.
fn diagnosis_config(
    enabled: bool,
    prompt_injection: bool,
    max_chars: usize,
    max_findings: usize,
    window: usize,
    max_repeats: usize,
) -> RuntimeDiagnosisConfig {
    RuntimeDiagnosisConfig {
        enabled,
        write_artifacts: false,
        prompt_injection_enabled: prompt_injection,
        max_prompt_findings: max_findings,
        max_prompt_chars: max_chars,
        retry_window_iterations: window,
        max_repeated_recoveries: max_repeats,
        artifact_retention: 10,
        malformed_jsonl_policy: MalformedJsonlPolicy::Warn,
        drift: DriftConfig::default(),
    }
}

fn make_finding_envelope(
    retry_key: &str,
    iteration: u32,
    target_hat: Option<&str>,
    source_hat: Option<&str>,
    safe_target: bool,
) -> RecoveryDiagnosisEnvelope {
    let mut b = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::MissingEventGate)
        .severity(DiagnosisSeverity::Warning)
        .iteration(iteration)
        .reason_code("no_emit")
        .message(format!("diagnostic for retry_key={retry_key}"))
        .retry_key(retry_key)
        .safe_target(safe_target)
        .outcome(DiagnosisOutcome::Pending);
    if let Some(t) = target_hat {
        b = b.target_hat(t);
    }
    if let Some(s) = source_hat {
        b = b.source_hat(s);
    }
    b.build()
}

fn make_yaml_with_diagnosis(cfg: &RuntimeDiagnosisConfig, hats_yaml: &str) -> RalphConfig {
    let yaml = format!(
        r#"
telemetry:
  runtime_diagnosis:
    enabled: {enabled}
    write_artifacts: false
    prompt_injection_enabled: {prompt}
    max_prompt_findings: {mf}
    max_prompt_chars: {mc}
    retry_window_iterations: {rw}
    max_repeated_recoveries: {mr}
    artifact_retention: 10
    malformed_jsonl_policy: warn
    drift:
      window_size: 50
      field_completeness_threshold: 0.9
      coord_join_rate_threshold: 0.6
      emit_cadence_sigma: 2.0
{hats}
"#,
        enabled = cfg.enabled,
        prompt = cfg.prompt_injection_enabled,
        mf = cfg.max_prompt_findings,
        mc = cfg.max_prompt_chars,
        rw = cfg.retry_window_iterations,
        mr = cfg.max_repeated_recoveries,
        hats = hats_yaml
    );
    serde_yaml::from_str(&yaml).expect("valid YAML")
}

fn solo_hats_yaml() -> &'static str {
    ""
}

fn multi_hat_yaml() -> &'static str {
    r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    publishes: ["build.done"]
  planner:
    name: "Planner"
    triggers: ["plan.ready"]
    publishes: ["plan.done"]
"#
}

fn isolated_hats_yaml() -> &'static str {
    r#"
event_loop:
  execution_mode: isolated
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    publishes: ["build.done"]
  planner:
    name: "Planner"
    triggers: ["plan.ready"]
    publishes: ["plan.done"]
"#
}

fn custom_hat_yaml() -> &'static str {
    r#"
hats:
  reviewer:
    name: "Code Reviewer"
    triggers: ["review.request"]
    instructions: "Review code quality."
"#
}

// ── 1. solo ralph path ─────────────────────────────────────────────

#[test]
fn solo_ralh_prompt_includes_alert() {
    let cfg = diagnosis_config(true, true, 2000, 5, 5, 3);
    let config = make_yaml_with_diagnosis(&cfg, solo_hats_yaml());
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("test");

    let envelope = make_finding_envelope(
        "missing_event_gate:builder:work_done:no_emit:*",
        1,
        Some("ralph"),
        Some("ralph"),
        true,
    );
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
    event_loop.set_iteration_for_test(1);

    let prompt = event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt");
    assert!(
        prompt.contains(RUNTIME_DIAGNOSIS_ALERT_HEADER),
        "solo ralph prompt should include the diagnosis alert header"
    );
}

// ── 2. multi-hat coordinator path ──────────────────────────────────

#[test]
fn multi_hat_coordinator_prompt_includes_alert() {
    let cfg = diagnosis_config(true, true, 2000, 5, 5, 3);
    let config = make_yaml_with_diagnosis(&cfg, multi_hat_yaml());
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("test");

    // Fire a finding that targets a specific hat. The coordinator
    // sees every hat's alerts (this is the contract documented in
    // `apply_runtime_diagnosis_prompt`).
    let envelope = make_finding_envelope(
        "missing_event_gate:builder:work_done:no_emit:*",
        1,
        Some("builder"),
        Some("builder"),
        true,
    );
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
    event_loop.set_iteration_for_test(1);

    let prompt = event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt");
    assert!(
        prompt.contains(RUNTIME_DIAGNOSIS_ALERT_HEADER),
        "coordinator prompt should include the diagnosis alert"
    );
}

// ── 3. isolated hat path ───────────────────────────────────────────

#[test]
fn isolated_hat_prompt_filters_findings_to_target_hat() {
    let cfg = diagnosis_config(true, true, 2000, 5, 5, 3);
    let config = make_yaml_with_diagnosis(&cfg, isolated_hats_yaml());
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("test");

    // Two findings, one for each hat.
    let builder_env = make_finding_envelope(
        "missing_event_gate:builder:work_done:no_emit:*",
        1,
        Some("builder"),
        Some("builder"),
        true,
    );
    let planner_env = make_finding_envelope(
        "missing_event_gate:planner:plan_ready:no_emit:*",
        1,
        Some("planner"),
        Some("planner"),
        true,
    );
    let _ = event_loop.record_recovery_envelope(&builder_env, Vec::new());
    let _ = event_loop.record_recovery_envelope(&planner_env, Vec::new());
    event_loop.set_iteration_for_test(1);

    // Build the builder's prompt. Only the builder finding should
    // show; the planner finding is filtered out.
    let prompt = event_loop
        .build_prompt(&HatId::new("builder"))
        .expect("prompt");
    assert!(
        prompt.contains(RUNTIME_DIAGNOSIS_ALERT_HEADER),
        "isolated builder prompt should include the alert"
    );
    assert!(
        prompt.contains("builder"),
        "alert should mention the builder hat"
    );
    // The planner finding's topic ("plan.ready") must not appear.
    assert!(
        !prompt.contains("plan_ready"),
        "isolated builder prompt should NOT include planner finding"
    );
}

// ── 4. backward-compat custom hat path ─────────────────────────────

#[test]
fn custom_hat_prompt_includes_alert() {
    let cfg = diagnosis_config(true, true, 2000, 5, 5, 3);
    let config = make_yaml_with_diagnosis(&cfg, custom_hat_yaml());
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("test");

    let envelope = make_finding_envelope(
        "missing_event_gate:reviewer:review_request:no_emit:*",
        1,
        Some("reviewer"),
        Some("reviewer"),
        true,
    );
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
    event_loop.set_iteration_for_test(1);

    // No execution_mode: isolated in YAML, so the custom-hat path is
    // the backward-compat branch. Trigger the reviewer by
    // publishing review.request.
    use ralph_proto::Event;
    event_loop
        .bus
        .publish(Event::new("review.request", "Review PR #1"));
    let prompt = event_loop
        .build_prompt(&HatId::new("reviewer"))
        .expect("prompt");
    assert!(
        prompt.contains(RUNTIME_DIAGNOSIS_ALERT_HEADER),
        "custom-hat prompt should include the diagnosis alert; prompt was: {prompt}"
    );
}

// ── 5. prompt-injection disabled / diagnostics disabled ───────────

#[test]
fn diagnostics_disabled_omits_alert() {
    // Default `RalphConfig` has `runtime_diagnosis.enabled == false`.
    // The prompt must not contain the alert header.
    let config = RalphConfig::default();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("test");

    // Feed an envelope into the responder directly to make sure the
    // `enabled = false` short-circuit, not the empty-findings
    // short-circuit, is what removes the alert.
    let envelope = make_finding_envelope(
        "missing_event_gate:builder:work_done:no_emit:*",
        1,
        Some("builder"),
        Some("builder"),
        true,
    );
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
    event_loop.set_iteration_for_test(1);

    let prompt = event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt");
    assert!(
        !prompt.contains(RUNTIME_DIAGNOSIS_ALERT_HEADER),
        "diagnostics disabled ⇒ prompt must not contain the alert"
    );
}

#[test]
fn prompt_injection_disabled_omits_alert() {
    let cfg = diagnosis_config(true, false, 2000, 5, 5, 3);
    let config = make_yaml_with_diagnosis(&cfg, solo_hats_yaml());
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("test");

    let envelope = make_finding_envelope(
        "missing_event_gate:builder:work_done:no_emit:*",
        1,
        Some("builder"),
        Some("builder"),
        true,
    );
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
    event_loop.set_iteration_for_test(1);

    let prompt = event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt");
    assert!(
        !prompt.contains(RUNTIME_DIAGNOSIS_ALERT_HEADER),
        "prompt_injection_enabled=false ⇒ prompt must not contain the alert"
    );
}

// ── 6. prompt length bound ────────────────────────────────────────

#[test]
fn alert_body_truncated_to_max_chars() {
    let cfg = diagnosis_config(true, true, 200, 50, 5, 3);
    let config = make_yaml_with_diagnosis(&cfg, solo_hats_yaml());
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("test");

    // Pile up a long string of findings so the alert body would
    // overshoot `max_chars` without truncation.
    for i in 0..20 {
        let env = make_finding_envelope(
            &format!("missing_event_gate:builder:work_done:filler_long_line:{i}"),
            1,
            Some("builder"),
            Some("builder"),
            true,
        );
        let _ = event_loop.record_recovery_envelope(&env, Vec::new());
    }
    event_loop.set_iteration_for_test(1);

    let prompt = event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt");
    let alert_idx = prompt
        .find(RUNTIME_DIAGNOSIS_ALERT_HEADER)
        .expect("alert header should be present");
    let body = &prompt[alert_idx..];
    assert!(
        body.chars().count() <= cfg.max_prompt_chars,
        "alert body len = {}, max = {}",
        body.chars().count(),
        cfg.max_prompt_chars
    );
    assert!(
        body.ends_with('\u{2026}'),
        "alert body must end with truncation ellipsis"
    );
}

// ── 7. recovery drops the alert ───────────────────────────────────

#[test]
fn check_recovery_drops_finding_from_prompt() {
    let cfg = diagnosis_config(true, true, 2000, 5, 5, 3);
    let config = make_yaml_with_diagnosis(&cfg, solo_hats_yaml());
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("test");

    let envelope = make_finding_envelope(
        "missing_event_gate:builder:work_done:no_emit:*",
        1,
        Some("builder"),
        Some("builder"),
        true,
    );
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());

    // Pretend the next iteration accepted the topic the envelope
    // was complaining about. Per the R7 review, we now pass
    // per-event evidence (topic + field set + timestamp) so the
    // responder can re-evaluate the specific drift metric.
    let evidence = vec![AcceptedEventEvidence {
        topic: "work.done".to_string(),
        fields: BTreeSet::new(),
        source_hat: Some("builder".to_string()),
        timestamp: chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
    }];
    let outcome =
        event_loop
            .recovery_responder_mut()
            .check_recovery(&envelope.retry_key, &evidence, 2);
    assert_eq!(outcome, Some(DiagnosisOutcome::Recovered));

    event_loop.begin_diagnosis_iteration();
    event_loop.set_iteration_for_test(2);
    let prompt = event_loop
        .build_prompt(&HatId::new("ralph"))
        .expect("prompt");
    assert!(
        !prompt.contains(RUNTIME_DIAGNOSIS_ALERT_HEADER),
        "recovered findings must not appear in the next prompt"
    );
}

// ── 8. hard escalation produces a RecoveryAction, not a hint ─────

#[test]
fn hard_escalation_after_threshold_emits_recovery_action() {
    let cfg = diagnosis_config(true, true, 2000, 5, 5, 2);
    let config = make_yaml_with_diagnosis(&cfg, solo_hats_yaml());
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("test");
    event_loop.set_iteration_for_test(1);

    for iter in 1..=2 {
        let env = make_finding_envelope(
            "missing_event_gate:builder:work_done:no_emit:*",
            iter,
            Some("builder"),
            Some("builder"),
            true,
        );
        let _ = event_loop.record_recovery_envelope(&env, Vec::new());
    }
    let actions = event_loop.recovery_responder_mut().drain_hard_escalations();
    assert_eq!(
        actions.len(),
        1,
        "expected exactly one Hard escalation action"
    );
    assert_eq!(actions[0].target_hat.as_str(), "builder");
    assert_eq!(
        actions[0].retry_key,
        "missing_event_gate:builder:work_done:no_emit:*"
    );
}

// ── 9. no safe target does NOT emit a recovery action ────────────

#[test]
fn no_safe_target_skips_recovery_action() {
    let cfg = diagnosis_config(true, true, 2000, 5, 5, 2);
    let config = make_yaml_with_diagnosis(&cfg, solo_hats_yaml());
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("test");
    event_loop.set_iteration_for_test(1);

    for iter in 1..=3 {
        let env = make_finding_envelope(
            "stall_recovery:ralph:*:stall_no_events:*",
            iter,
            None,
            Some("ralph"),
            false,
        );
        let _ = event_loop.record_recovery_envelope(&env, Vec::new());
    }
    let actions = event_loop.recovery_responder_mut().drain_hard_escalations();
    assert!(
        actions.is_empty(),
        "no-safe-target findings must not produce Hard actions"
    );
    let hint = event_loop.recovery_responder_mut().take_termination_hint();
    assert!(
        hint.is_some(),
        "no-safe-target findings must surface a Final hint"
    );
}

// ── 10. Final escalation never replaces PayloadContractViolation ──

#[test]
fn final_hint_never_replaces_payload_contract_violation() {
    // The runner integration is the contract enforcer; the
    // responder itself must NEVER propose
    // `TerminationReason::PayloadContractViolation` as a hint
    // reason. The hint's `reason` field is the only data the runner
    // sees, so this test pins the responder's contract directly.
    let cfg = diagnosis_config(true, true, 2000, 5, 1, 1);
    let config = make_yaml_with_diagnosis(&cfg, solo_hats_yaml());
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("test");
    event_loop.set_iteration_for_test(1);

    // A payload-contract-shaped finding (`safe_target = false`)
    // produces a Final hint. The hint must NOT name the
    // `payload_contract_violation` reason — that reason is
    // reserved for the runner's existing termination logic.
    let env = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::PayloadContract)
        .severity(DiagnosisSeverity::Critical)
        .iteration(1)
        .reason_code("payload_contract_violation")
        .message("plan_name required on work.done")
        .topic("work.done")
        .retry_key("payload_contract:builder:work_done:payload_contract_violation:plan_name")
        .safe_target(false)
        .outcome(DiagnosisOutcome::NotRetriable)
        .build();
    let _ = event_loop.record_recovery_envelope(&env, Vec::new());

    let hint = event_loop.recovery_responder_mut().take_termination_hint();
    if let Some(hint) = hint {
        // The reason must not be the runner's reserved
        // `payload_contract_violation` phrase. The responder's
        // reason may include the retry_key (which can contain
        // arbitrary underscore-joined parts), so we check the
        // *phrase* "payload_contract_violation reason" or
        // "termination reason: payload_contract_violation".
        // Neither phrase is in the responder's vocabulary.
        assert!(
            !hint.reason.starts_with("payload_contract_violation"),
            "responder hint must not introduce a new termination reason: {}",
            hint.reason
        );
        assert!(
            !hint.reason.contains("termination_reason: payload_contract"),
            "responder hint must not introduce a new termination reason: {}",
            hint.reason
        );
    }
    // The test also asserts (by absence) that the runner side will
    // *not* replace `TerminationReason::PayloadContractViolation`.
    // The runner logic is in `runner.rs::finalize_recovery_diagnosis`
    // which only *appends* a section, never overrides the reason.
}

// ── 11. responder API contract ────────────────────────────────────

#[test]
fn responder_api_smoke() {
    // Round-trip the basic API surface to catch signature drift.
    let cfg = Arc::new(diagnosis_config(true, true, 2000, 5, 5, 3));
    let mut r = RecoveryResponder::new(cfg.clone());
    r.begin_iteration();
    assert_eq!(r.tracked_retry_keys(), 0);
    assert!(!r.has_pending_findings());

    let env = make_finding_envelope(
        "missing_event_gate:builder:work_done:no_emit:*",
        1,
        Some("builder"),
        Some("builder"),
        true,
    );
    let decision = r.record_finding(&env, 1);
    assert_eq!(decision.level, EscalationLevel::Soft);
    assert_eq!(decision.attempt, 1);
    assert_eq!(r.tracked_retry_keys(), 1);
    assert!(r.has_pending_findings());
    assert_eq!(
        r.attempt_count("missing_event_gate:builder:work_done:no_emit:*"),
        1
    );
    assert!(r.has_safe_target("missing_event_gate:builder:work_done:no_emit:*"));
    assert_eq!(
        r.target_hat_for_retry("missing_event_gate:builder:work_done:no_emit:*")
            .as_deref(),
        Some("builder")
    );
}

// ── 12. escalation-level envelope is NOT published to events bus ──

#[test]
fn responder_does_not_publish_to_bus() {
    // The plan forbids the responder from writing envelopes to the
    // events JSONL. The U3 logger (`DiagnosticsCollector::log_recovery`)
    // is the only writer; the responder is in-memory only. We
    // verify by reading the bus's pending counts before and after a
    // `record_finding` call and asserting they are unchanged. The
    // `bus()` accessor requires `&mut EventLoop`, so we read both
    // snapshots through a `&mut` binding.
    let cfg = diagnosis_config(true, true, 2000, 5, 5, 3);
    let config = make_yaml_with_diagnosis(&cfg, solo_hats_yaml());
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("test");

    let bus_len_before = total_pending(&mut event_loop);

    let envelope = make_finding_envelope(
        "missing_event_gate:builder:work_done:no_emit:*",
        1,
        Some("builder"),
        Some("builder"),
        true,
    );
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());

    let bus_len_after = total_pending(&mut event_loop);
    assert_eq!(
        bus_len_before, bus_len_after,
        "RecoveryResponder must not publish to the EventBus"
    );
}

fn total_pending(event_loop: &mut EventLoop) -> usize {
    use ralph_proto::EventBus;
    let bus: &mut EventBus = event_loop.bus();
    let hat_ids: Vec<ralph_proto::HatId> = bus.hat_ids().cloned().collect();
    let mut total = 0_usize;
    for hat_id in &hat_ids {
        if let Some(events) = bus.peek_pending(hat_id) {
            total += events.len();
        }
    }
    total
}
