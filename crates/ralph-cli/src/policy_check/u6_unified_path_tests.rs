use super::*;
use ralph_core::validation::{ReasonCode, ValidationResult, ValidationStage};
use tempfile::TempDir;

fn fake_report(
    pre: Vec<ValidationResult>,
    post: Vec<ValidationResult>,
) -> ralph_core::validation::ValidationReport {
    let accepted = pre.iter().all(|r| r.accepted) && post.iter().all(|r| r.accepted);
    let post_commit_rejected = post.iter().any(|r| !r.accepted);
    ralph_core::validation::ValidationReport {
        pre_commit: pre,
        post_commit: post,
        accepted,
        post_commit_rejected,
    }
}

#[test]
fn report_from_validation_accepts_collects_no_reasons() {
    let report = fake_report(
        vec![ValidationResult::accept(), ValidationResult::accept()],
        vec![ValidationResult::accept()],
    );
    let workspace = std::path::PathBuf::from("/tmp/workspace");
    let out = report_from_validation(&report, "work.done", Some("executor"), &workspace);
    assert!(out.accepted);
    assert!(out.reason_codes.is_empty());
    assert!(out.suggestions.is_empty());
    assert!(!out.post_commit_rejected);
    assert_eq!(out.topic, "work.done");
    assert_eq!(out.hat.as_deref(), Some("executor"));
}

#[test]
fn report_from_validation_pre_commit_collects_reason_code_and_hint() {
    // The unified pipeline emits `step_handoff::<reason>` (the
    // `STEP_HANDOFF_MISMATCH_PREFIX` constant already ends in
    // `:`, and the rule appends `<reason>` after another `:`).
    // Pin the exact shape so the loop ↔ CLI vocabulary stays
    // in lockstep.
    let report = fake_report(
        vec![
            ValidationResult::accept(),
            ValidationResult::reject(
                ValidationStage::StepHandoff,
                format!(
                    "{}:task_closed_but_progress_missing",
                    ReasonCode::STEP_HANDOFF_MISMATCH_PREFIX
                ),
                Some("add progress entry for step".to_string()),
                true,
            ),
        ],
        vec![ValidationResult::accept()],
    );
    let workspace = std::path::PathBuf::from("/tmp/workspace");
    let out = report_from_validation(&report, "queue.advance", None, &workspace);
    assert!(!out.accepted);
    assert_eq!(out.reason_codes.len(), 1);
    assert_eq!(
        out.reason_codes[0],
        "step_handoff::task_closed_but_progress_missing"
    );
    assert_eq!(out.suggestions[0], "add progress entry for step");
    assert!(!out.post_commit_rejected);
}

#[test]
fn report_from_validation_post_commit_rejection_sets_flag() {
    use ralph_core::validation::RejectionHint;
    let report = fake_report(
        vec![ValidationResult::accept()],
        vec![ValidationResult::reject(
            ValidationStage::ExecutionContract,
            ReasonCode::CONTRACT_MISSING_TASK_ID,
            Some(RejectionHint::missing_task_id("task_id")),
            true,
        )],
    );
    let workspace = std::path::PathBuf::from("/tmp/workspace");
    let out = report_from_validation(&report, "work.done", Some("executor"), &workspace);
    assert!(!out.accepted);
    assert!(out.post_commit_rejected);
    assert_eq!(out.reason_codes.len(), 1);
    assert_eq!(out.reason_codes[0], ReasonCode::CONTRACT_MISSING_TASK_ID);
}

#[test]
fn report_to_json_value_contains_reason_codes_array() {
    let report = fake_report(
        vec![ValidationResult::reject(
            ValidationStage::Origin,
            ReasonCode::RALPH_CONTROL_ONLY.to_string(),
            None,
            true,
        )],
        vec![],
    );
    let workspace = std::path::PathBuf::from("/tmp/workspace");
    let out = report_from_validation(&report, "review.passed", Some("ralph"), &workspace);
    let json = out.to_json_value().unwrap();
    assert_eq!(json["topic"], "review.passed");
    assert_eq!(json["hat"], "ralph");
    assert_eq!(json["accepted"], false);
    assert_eq!(
        json["reason_codes"][0],
        ReasonCode::RALPH_CONTROL_ONLY.to_string()
    );
    assert_eq!(
        json["suggestions"][0],
        serde_json::Value::String(String::new())
    );
}

#[test]
fn run_policy_check_unified_accepts_no_required_fields() {
    // Empty workspace + no config → cold-start view with no
    // required fields. The pipeline accepts every event, the
    // report mirrors the loop's accept verdict.
    let tmp = TempDir::new().unwrap();
    let report =
        run_policy_check_unified("debug.step", Some("task_id=demo"), None, None, tmp.path())
            .expect("unified check should succeed on empty workspace");
    assert!(report.accepted, "report: {report:?}");
    assert!(report.reason_codes.is_empty());
    assert_eq!(report.topic, "debug.step");
}

#[test]
fn unified_policy_check_uses_caller_resolved_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config: RalphConfig = serde_yaml::from_str(
        r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
hats:
  review-worker:
    name: "Review Worker"
    triggers: ["review.unit.ready"]
    publishes: ["review.unit.done"]
"#,
    )
    .expect("valid config");

    let report = run_policy_check_unified_with_config(
        "review.unit.done",
        Some("{}"),
        Some("review-worker"),
        None,
        temp.path(),
        Some(&config),
    )
    .expect("policy check");

    assert!(
        !report
            .reason_codes
            .iter()
            .any(|code| code == "origin:unknown_hat"),
        "the caller-resolved hat registry must be reused: {report:?}"
    );
}

#[test]
fn run_policy_check_unified_rejects_missing_required_field() {
    // Build a workspace whose ralph.yml declares a required
    // field for `experiment.planned`. The payload omits it →
    // the unified pipeline must surface a structured
    // `engine_rejected:required_field:<name>` reason code.
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".ralph")).unwrap();
    std::fs::write(
        tmp.path().join("ralph.yml"),
        r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      experiment.planned:
        required_fields:
          - task_key
",
    )
    .unwrap();
    let report = run_policy_check_unified(
        "experiment.planned",
        Some(r#"{"foo":"bar"}"#),
        None,
        None,
        tmp.path(),
    )
    .expect("unified check should return a report");
    assert!(
        !report.accepted,
        "missing required field must reject: {report:?}"
    );
    assert!(
        report
            .reason_codes
            .iter()
            .any(|c| c.starts_with("engine_rejected:required_field")),
        "expected engine_rejected:required_field reason, got: {:?}",
        report.reason_codes
    );
}

#[test]
#[allow(deprecated)]
fn run_policy_check_unified_misaligned_queue_advance_rejected() {
    // Step-handoff gate (U1 2026-06-17-005 plan): when
    // progress.md and tasks.jsonl disagree, the loop rejects
    // `queue.advance` with `progress_task_mismatch`. The
    // unified pipeline must surface the same reason_code so
    // the CLI and loop never disagree.
    let tmp = TempDir::new().unwrap();
    let ralph_dir = tmp.path().join(".ralph");
    std::fs::create_dir_all(&ralph_dir).unwrap();
    let agent_dir = ralph_dir.join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();

    // Closed task for step-01.
    let task = serde_json::json!({
        "id": "task-1",
        "title": "step-01",
        "status": "closed",
        "priority": 3,
    });
    let tasks_path = agent_dir.join("tasks.jsonl");
    std::fs::write(&tasks_path, format!("{task}\n")).unwrap();

    // progress.md does NOT list step-01 → mismatch.
    let progress = "## Current Step\nstep-02\n\n## Completed Steps\n- step-02\n";
    std::fs::write(agent_dir.join("progress.md"), progress).unwrap();

    let report = run_policy_check_unified(
        "queue.advance",
        Some(r#"{"step":"step-02","task_id":"task-1"}"#),
        None,
        None,
        tmp.path(),
    )
    .expect("unified check should return a report");
    assert!(
        !report.accepted,
        "misaligned progress must produce a non-accepting report: {report:?}"
    );
    assert!(
        report
            .reason_codes
            .iter()
            .any(|c| c.starts_with("step_handoff:")),
        "expected step_handoff reason code, got: {:?}",
        report.reason_codes
    );
}

// The legacy `check_progress_task_alignment` call below is
// intentional: this test pins the contract that the disk-read
// legacy gate and the unified snapshot pipeline both reject a
// misaligned `queue.advance`. The `#[allow(deprecated)]` only
// sits on the function because Rust does not propagate allow
// attributes from a `use` statement into the function body.
// Do not migrate this call to `check_alignment_with_snapshot` —
// it would defeat the legacy-vs-unified parity this test
// verifies.
#[test]
#[allow(deprecated)]
fn run_policy_check_unified_and_loop_agree_on_misaligned_progress() {
    // U6 plan §"Test scenarios" Error path: `--policy-check` and
    // the loop must produce matching verdicts (both reject) for a
    // misaligned `queue.advance`. The exact reason string may
    // differ — the unified pipeline runs against a cold-start
    // `LedgerSnapshot` (CLI emit runs ahead of the loop, not
    // against its in-memory state), while the legacy gate reads
    // `tasks.jsonl` directly. The contract we pin is that both
    // paths surface `step_handoff:` reason codes so downstream
    // tooling can route on the prefix; the unified pipeline also
    // emits a `step_handoff::` shape that the CLI precheck can
    // intersect with the loop's `progress_task_mismatch` family.
    #[allow(deprecated)]
    use ralph_core::step_handoff::progress_task_gate::{
        GateDecision, check_progress_task_alignment,
    };

    let tmp = TempDir::new().unwrap();
    let ralph_dir = tmp.path().join(".ralph");
    std::fs::create_dir_all(ralph_dir.join("agent")).unwrap();

    let task = serde_json::json!({
        "id": "task-1",
        "title": "step-01",
        "status": "closed",
        "priority": 3,
    });
    std::fs::write(ralph_dir.join("agent/tasks.jsonl"), format!("{task}\n")).unwrap();
    std::fs::write(
        ralph_dir.join("agent/progress.md"),
        "## Current Step\nstep-02\n\n## Completed Steps\n- step-02\n",
    )
    .unwrap();

    // Loop-side gate (the existing precheck helper
    // `check_progress_task_alignment` mirrors the loop's
    // `apply_step_handoff_gate`).
    let decision =
        check_progress_task_alignment("queue.advance", Some("step-02"), Some("task-1"), tmp.path());
    let loop_rejects = matches!(decision, GateDecision::Mismatch(_));
    assert!(
        loop_rejects,
        "loop gate must reject the misaligned progress"
    );

    // Unified-pipeline side: must also reject and emit a
    // `step_handoff:` reason code. The exact suffix differs
    // because the unified pipeline runs against a cold-start
    // snapshot (no tasks loaded), but the shared prefix is
    // what callers match on.
    let report = run_policy_check_unified(
        "queue.advance",
        Some(r#"{"step":"step-02","task_id":"task-1"}"#),
        None,
        None,
        tmp.path(),
    )
    .expect("unified check should return a report");
    assert!(!report.accepted, "unified pipeline must reject");
    assert!(
        report
            .reason_codes
            .iter()
            .any(|c| c.starts_with("step_handoff:")),
        "unified pipeline must surface step_handoff reason code; got: {:?}",
        report.reason_codes
    );
}

// ─────────────────────────────────────────────────────────────────
// 2026-07-07-001 plan U1: CLI `run_policy_check_unified` must
// wire the runtime `HatRegistry` from the loaded config so
// an envelope addressed to an unknown `receiver_contract.to_hat`
// is rejected (the same wire shape as the runtime pipeline).
// ─────────────────────────────────────────────────────────────────

/// Build a workspace whose `ralph.yml` declares a small set of
/// hats, an envelope-required schema on `work.done`, and a
/// `handoff_envelope` policy block that asks the validator to
/// run. Used by the U1 RED tests below.
fn workspace_with_serial_handoff_policy() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let yaml = r#"
hats:
  executor:
    name: Executor
    triggers: ["work.ready"]
    publishes: ["work.done", "work.failed"]
    instructions: "execute"
  reviewer:
    name: Reviewer
    triggers: ["work.done"]
    publishes: ["review.passed"]
    instructions: "review"
event_loop:
  handoff_envelope:
    enabled: true
    validate_payload: true
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      work.done:
        required_fields: ["handoff_envelope", "task_id", "step"]
"#;
    std::fs::write(tmp.path().join("ralph.yml"), yaml).unwrap();
    tmp
}

/// Local copy of the U10 (2026-07-06-004) envelope fixture
/// (defined inside `tests` module below) so the U1 tests can
/// stand on their own without depending on the test-module
/// access boundary.
fn u1_full_envelope(to_hat: &str) -> String {
    format!(
        r#"{{
  "plan_name":"p1",
  "task_id":"t1",
  "task_key":"p1:step-2:implement",
  "step":"step-2",
  "commit_count":1,
  "changed_lines":10,
  "handoff_envelope":{{
    "schema_version":"handoff-envelope.v1",
    "root_goal":"ship without regressions",
    "plan":{{
      "name":"p1",
      "path":"docs/plans/p1.md",
      "current_step":"step-2",
      "completed_steps":["step-1"]
    }},
    "state":{{
      "current_status":"ready_for_review",
      "last_signal":"work.done",
      "blocking_reason":null
    }},
    "receiver_contract":{{
      "to_hat":"{to_hat}",
      "must_do":["review step-2"],
      "must_not_do":["regress step-1"],
      "success_signal":"work.done",
      "failure_signal":"work.failed"
    }}
  }}
}}"#
    )
}

#[test]
fn policy_check_rejects_unknown_handoff_to_hat_from_builtin_serial_config() {
    // 2026-07-07-001 plan U1 (P1): CLI --policy-check must
    // reject an envelope addressed to an unknown to_hat when
    // the workspace config declares a real hat set. The
    // unified pipeline inside `run_policy_check_unified` must
    // be wired with the runtime `HatRegistry` so the
    // `EventPolicyRule` registry check actually fires.
    let tmp = workspace_with_serial_handoff_policy();
    let payload = u1_full_envelope("ghost-hat");
    let report = run_policy_check_unified(
        "work.done",
        Some(&payload),
        Some("executor"),
        None,
        tmp.path(),
    )
    .expect("unified check should return a report");
    assert!(
        !report.accepted,
        "unknown to_hat must produce a non-accepting report: {report:?}"
    );
    // The CLI report surfaces a stable reason prefix; the
    // envelope validator's `unknown_to_hat` code must reach
    // either `reason_codes` or `suggestions` so downstream
    // tooling can route on it.
    let reason_blob = format!("{:?}\n{:?}", report.reason_codes, report.suggestions);
    assert!(
        reason_blob.contains("handoff_envelope_unknown_to_hat"),
        "report must surface the stable unknown-to_hat code; got: {reason_blob}"
    );
    assert!(
        reason_blob.contains("ghost-hat"),
        "report must name the offending to_hat id; got: {reason_blob}"
    );
}

#[test]
fn policy_check_accepts_known_handoff_to_hat_from_builtin_serial_config() {
    // Symmetric happy-path: known to_hat must still pass.
    let tmp = workspace_with_serial_handoff_policy();
    let payload = u1_full_envelope("reviewer");
    let report = run_policy_check_unified(
        "work.done",
        Some(&payload),
        Some("executor"),
        None,
        tmp.path(),
    )
    .expect("unified check should return a report");
    assert!(
        report.accepted,
        "known to_hat must pass; report: {report:?}"
    );
}

// ─────────────────────────────────────────────────────────────────
// U7 of plan 2026-07-05-005 (R6, R12): envelope-layer
// `triggered` validator. Both the apply path and the
// `--policy-check` path share `check_envelope_triggered`;
// tests cover the standalone helper and the unified
// entry-point surface.
// ─────────────────────────────────────────────────────────────────

fn cfg_with_hats(ids: &[&str]) -> RalphConfig {
    // RalphConfig.hats is HashMap<String, HatConfig>; build
    // it as a YAML mapping keyed by hat id.
    let mut hat_blocks = String::new();
    for id in ids {
        hat_blocks.push_str(&format!(
            "  {id}:\n    name: {id}\n    triggers: []\n    publishes: []\n"
        ));
    }
    let yaml = format!("hats:\n{hat_blocks}");
    serde_yaml::from_str(&yaml).expect("synthetic RalphConfig yaml")
}

#[test]
fn u7_check_envelope_triggered_in_topology_allowed() {
    let cfg = cfg_with_hats(&["review-synthesizer"]);
    // U7 of 2026-07-05-005: business-topic path; declared
    // hat must be accepted.
    check_envelope_triggered("work.done", None, Some("review-synthesizer"), &cfg)
        .expect("declared hat must be accepted");
}

#[test]
fn u7_check_envelope_triggered_rejects_isolated_self_target() {
    let mut cfg = cfg_with_hats(&["goal-alignment", "correctness"]);
    cfg.event_loop.execution_mode = ralph_core::config::HatExecutionMode::Isolated;

    let err = check_envelope_triggered(
        "review.goalalign.done",
        Some("goal-alignment"),
        Some("goal-alignment"),
        &cfg,
    )
    .expect_err("isolated business self-target must fail closed");

    assert_eq!(err.reason_code, "triggered_self_target");
    assert!(err.message.contains("publishing hat"));
}

#[test]
fn u7_check_envelope_triggered_missing_allowed() {
    let cfg = cfg_with_hats(&["review-synthesizer"]);
    // R12: missing triggered is allowed.
    check_envelope_triggered("work.done", None, None, &cfg).expect("missing triggered is allowed");
    check_envelope_triggered("work.done", None, Some(""), &cfg)
        .expect("empty triggered is allowed");
}

#[test]
fn u7_check_envelope_triggered_not_in_topology_rejected() {
    let cfg = cfg_with_hats(&["review-synthesizer"]);
    let err = check_envelope_triggered("work.done", None, Some("planner"), &cfg).unwrap_err();
    assert_eq!(err.reason_code, "triggered_not_in_topology");
    assert!(err.message.contains("planner"));
    assert!(err.message.contains("review-synthesizer"));
}

/// U7 of plan 2026-07-05-005 (fix-plan §R11): a ralph-control
/// topic carrying `triggered="ralph"` (the runtime pseudo-hat)
/// is accepted even when `ralph` is not in the preset hats[].
#[test]
fn u7_check_envelope_triggered_ralph_control_topic_accepts_pseudo_hat() {
    let cfg = cfg_with_hats(&["review-synthesizer"]);
    // task.resume is a ralph-control topic; `ralph` is the
    // runtime pseudo-hat that injects recovery events.
    check_envelope_triggered("task.resume", None, Some("ralph"), &cfg)
        .expect("ralph-control topic + triggered=ralph must accept");
}

/// U7 of plan 2026-07-05-005 (fix-plan §R11): an
/// orchestrator-diagnostic topic carrying an unknown `triggered`
/// is accepted (the runtime origin guard handles it).
#[test]
fn u7_check_envelope_triggered_diagnostic_topic_accepts_unknown() {
    let cfg = cfg_with_hats(&["review-synthesizer"]);
    check_envelope_triggered("event.malformed", None, Some("ralph-runner"), &cfg)
        .expect("diagnostic topic + unknown triggered must accept");
}

/// U7 of plan 2026-07-05-005 (fix-plan §R11): a business
/// topic carrying `triggered="ralph"` (the pseudo-hat, which
/// is not in preset hats[]) MUST be rejected — the strict
/// business-topic layer applies regardless of the value.
#[test]
fn u7_check_envelope_triggered_business_topic_rejects_pseudo_hat() {
    let cfg = cfg_with_hats(&["review-synthesizer"]);
    let err = check_envelope_triggered("work.done", None, Some("ralph"), &cfg).unwrap_err();
    assert_eq!(err.reason_code, "triggered_not_in_topology");
    assert!(err.message.contains("ralph"));
}

#[test]
fn u7_policy_check_unified_surfaces_triggered_violation() {
    // Build a workspace with a ralph.yml that declares
    // exactly one hat, then call run_policy_check_unified
    // with an unknown `triggered` value. The report must
    // surface `triggered_not_in_topology` so the agent can
    // see the violation without writing to disk.
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".ralph")).unwrap();
    std::fs::write(
        tmp.path().join("ralph.yml"),
        r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
hats:
  review-synthesizer:
    name: review-synthesizer
    triggers: ["work.done"]
    publishes: ["review.dimensions.complete"]
"#,
    )
    .unwrap();
    let report = run_policy_check_unified(
        "work.done",
        Some(r#"{"task_id":"t1","task_key":"k1","step":"s1"}"#),
        None,
        Some("planner"), // unknown hat id
        tmp.path(),
    )
    .expect("report");
    assert!(!report.accepted, "unknown triggered must reject");
    assert!(
        report
            .reason_codes
            .iter()
            .any(|c| c == "triggered_not_in_topology"),
        "expected triggered_not_in_topology in {:?}",
        report.reason_codes
    );
}
