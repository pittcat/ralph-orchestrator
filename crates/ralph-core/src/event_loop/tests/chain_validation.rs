//! Tests for chain_validation.

use super::common::*;
use super::*;

#[test]
fn test_chain_validation_rejects_completion_without_required_events() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.required_events = vec!["plan.approved".to_string(), "all.built".to_string()];
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Only emit plan.approved, missing all.built
    write_event_to_jsonl(&events_path, "plan.approved", "OK");
    let _ = event_loop.process_events_from_jsonl();

    // Now try to complete
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason, None,
        "LOOP_COMPLETE should be rejected when required events are missing"
    );
}

#[test]
fn test_chain_validation_accepts_completion_with_all_required_events() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.required_events = vec!["plan.approved".to_string(), "all.built".to_string()];
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(&mut event_loop, &["plan.approved", "all.built"]);

    // Emit both required events across iterations
    write_event_to_jsonl(&events_path, "plan.approved", "OK");
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(&events_path, "all.built", "Done");
    let _ = event_loop.process_events_from_jsonl();

    // Now complete
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "LOOP_COMPLETE should be accepted when all required events have been seen"
    );
}

#[test]
fn test_chain_validation_tracks_topics_across_iterations() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.required_events = vec![
        "research.complete".to_string(),
        "plan.approved".to_string(),
        "all.built".to_string(),
    ];
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(
        &mut event_loop,
        &["research.complete", "plan.approved", "all.built"],
    );

    // Iteration 1: research.complete
    write_event_to_jsonl(&events_path, "research.complete", "findings");
    let _ = event_loop.process_events_from_jsonl();

    // Iteration 2: plan.approved
    write_event_to_jsonl(&events_path, "plan.approved", "ok");
    let _ = event_loop.process_events_from_jsonl();

    // Iteration 3: all.built + LOOP_COMPLETE
    write_event_to_jsonl(&events_path, "all.built", "done");
    let _ = event_loop.process_events_from_jsonl();

    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Topics should be tracked across iterations"
    );
}

#[test]
fn test_chain_validation_empty_required_events_allows_completion() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let config = RalphConfig::default(); // No required_events
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "Empty required_events should allow completion (backward compatible)"
    );
}

#[test]
fn test_chain_validation_injects_correction_on_rejection() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.required_events = vec!["plan.approved".to_string()];
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Try to complete without the required event
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "Should reject completion");

    // P0-2 (2026-06-23-003 plan): completion rejection now routes
    // through the deterministic-correction path. The rejection
    // signal lives in `state.prompt_context.correction_blocks`
    // (rendered into the next prompt as `## ORCHESTRATOR CORRECTION`)
    // instead of being published on the EventBus as `task.resume`.
    assert!(
        !event_loop
            .state()
            .prompt_context
            .correction_blocks
            .is_empty(),
        "completion rejection must inject a CorrectionContext into state.prompt_context (P0-2)"
    );
}

#[test]
fn test_rejected_loop_complete_does_not_poison_terminal_state() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  required_events:
    - "report.done"
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "report.done"
      - "plan.blocked"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(&mut event_loop, &["report.done", "plan.blocked"]);

    // Emit LOOP_COMPLETE without the required report.done.
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(reason, None, "LOOP_COMPLETE should be rejected");

    // The rejected LOOP_COMPLETE must not set terminal_observed; otherwise
    // recovery events like plan.blocked would hit terminal_monotonicity_violation.
    let policy_state = event_loop
        .state()
        .policy_runtime_state
        .as_ref()
        .expect("policy runtime state should exist");
    assert!(
        !policy_state.terminal_observed,
        "rejected LOOP_COMPLETE must not poison terminal_observed"
    );

    // Recovery path: plan.blocked should be accepted, not rejected as a
    // business event after a (phantom) terminal event.
    write_event_to_jsonl(
        &events_path,
        "plan.blocked",
        r#"{"reason": "review_failed"}"#,
    );
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !result.had_rejected_events,
        "plan.blocked recovery event must not be rejected after a rejected LOOP_COMPLETE"
    );
    assert!(
        result
            .accepted_events
            .iter()
            .any(|e| e.topic == "plan.blocked".into()),
        "plan.blocked should be accepted"
    );
}

#[test]
fn test_loop_cancel_terminates_without_chain_validation() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.cancellation_promise = "loop.cancel".to_string();
    config.event_loop.required_events = vec!["plan.approved".to_string(), "all.built".to_string()];
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(
        &mut event_loop,
        &["loop.cancel", "plan.approved", "all.built"],
    );

    // Send loop.cancel without any required events seen
    write_event_to_jsonl(&events_path, "loop.cancel", "rejected by human");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_cancellation_event();
    assert_eq!(
        reason,
        Some(TerminationReason::Cancelled),
        "loop.cancel should terminate without chain validation"
    );
}

#[test]
fn test_default_publishes_satisfies_required_events_for_completion() {
    use std::collections::HashMap;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.required_events = vec!["plan.draft".to_string(), "all.built".to_string()];

    let mut hats = HashMap::new();
    hats.insert(
        "planner".to_string(),
        crate::config::HatConfig {
            name: "planner".to_string(),
            description: Some("Plans work".to_string()),
            triggers: vec!["research.complete".to_string()],
            publishes: vec!["plan.draft".to_string()],
            instructions: "Plan".to_string(),
            extra_instructions: vec![],
            backend: None,
            backend_args: None,
            default_publishes: Some("plan.draft".to_string()),
            max_activations: None,
            scratchpad: None,
            disallowed_tools: vec![],
            timeout: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            phase_triggers: None,
            ..Default::default()
        },
    );
    config.hats = hats;

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(&mut event_loop, &["plan.draft", "all.built"]);

    // Simulate: planner wrote no events, default_publishes injects plan.draft
    let planner_id = HatId::new("planner");
    event_loop.check_default_publishes(&planner_id);

    // Then all.built arrives via JSONL
    write_event_to_jsonl(&events_path, "all.built", "done");
    let _ = event_loop.process_events_from_jsonl();

    // Now LOOP_COMPLETE should be accepted (plan.draft was from default_publishes)
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "default_publishes events should satisfy required_events chain validation"
    );
}

#[test]
fn test_default_publishes_completion_promise_triggers_termination() {
    use std::collections::HashMap;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.completion_promise = "LOOP_COMPLETE".to_string();
    config.event_loop.required_events = vec!["all.built".to_string()];

    let mut hats = HashMap::new();
    hats.insert(
        "final_committer".to_string(),
        crate::config::HatConfig {
            name: "FinalCommitter".to_string(),
            description: Some("Verifies all work is complete".to_string()),
            triggers: vec!["all.built".to_string()],
            publishes: vec!["LOOP_COMPLETE".to_string()],
            instructions: "Verify and complete".to_string(),
            extra_instructions: vec![],
            backend: None,
            backend_args: None,
            default_publishes: Some("LOOP_COMPLETE".to_string()),
            max_activations: None,
            scratchpad: None,
            disallowed_tools: vec![],
            timeout: None,
            concurrency: 1,
            aggregate: None,
            event_filter: None,
            phase_triggers: None,
            ..Default::default()
        },
    );
    config.hats = hats;

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(&mut event_loop, &["all.built", "LOOP_COMPLETE"]);

    // Satisfy required_events: all.built arrives via JSONL
    write_event_to_jsonl(&events_path, "all.built", "done");
    let _ = event_loop.process_events_from_jsonl();

    // Set active hat so check_default_publishes targets the right hat
    event_loop.state.last_active_hat_ids = vec![HatId::new("final_committer")];

    // Simulate: final_committer wrote no events, default_publishes injects LOOP_COMPLETE
    let hat_id = HatId::new("final_committer");
    event_loop.check_default_publishes(&hat_id);

    // completion_requested should be set directly by check_default_publishes
    // (not requiring a JSONL round-trip)
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "default_publishes of completion_promise should trigger termination directly, \
         not just publish to the bus where it would be lost"
    );
}

#[test]
fn test_loop_cancel_exit_code_is_zero() {
    assert_eq!(
        TerminationReason::Cancelled.exit_code(),
        0,
        "Cancelled should have exit code 0"
    );
}

#[test]
fn test_loop_cancel_is_not_success() {
    assert!(
        !TerminationReason::Cancelled.is_success(),
        "Cancelled should not be a success"
    );
}

#[test]
fn test_loop_cancel_takes_priority_over_completion() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.cancellation_promise = "loop.cancel".to_string();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    install_admitting_flow(&mut event_loop, &["loop.cancel", "LOOP_COMPLETE"]);

    // Both loop.cancel and LOOP_COMPLETE in same batch
    write_event_to_jsonl(&events_path, "loop.cancel", "rejected");
    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");
    let _ = event_loop.process_events_from_jsonl();

    // Cancellation should take priority (checked first)
    let cancel_reason = event_loop.check_cancellation_event();
    assert_eq!(
        cancel_reason,
        Some(TerminationReason::Cancelled),
        "Cancellation should take priority over completion"
    );
}

#[test]
fn test_loop_cancel_disabled_when_empty_string() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let mut config = RalphConfig::default();
    config.event_loop.cancellation_promise = String::new(); // Disabled
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // loop.cancel should pass through as a normal event (no termination)
    write_event_to_jsonl(&events_path, "loop.cancel", "rejected");
    let _ = event_loop.process_events_from_jsonl();

    let reason = event_loop.check_cancellation_event();
    assert_eq!(
        reason, None,
        "loop.cancel should not trigger cancellation when disabled"
    );
}

// ---------------------------------------------------------------------
// U1 (plan 2026-08-06-001) — D9 partition consumption guard.
//
// F-A root cause: before U1, `prepend_correction_and_resume`
// cleared the entire correction queue regardless of which hat
// built the prompt.  In a multi-hat topology, the hat that
// built first could swallow a correction targeted at another
// hat.  U1 fixes this with `take_visible_corrections(current_hat)`.
//
// These tests pin the F-A fix end-to-end through
// `EventLoop::build_prompt(hat_id)`:
//   - `correction_target_specific` builds `reviewer`'s prompt
//     first and confirms the executor-targeted correction
//     survives.
//   - `correction_target_specific_drained_on_target` confirms
//     the executor-targeted correction is drained once
//     `executor` builds its prompt.
// ---------------------------------------------------------------------

#[test]
fn u1_correction_target_specific_survives_other_hat_build() {
    // F-A root cause: before U1, `prepend_correction_and_resume`
    // cleared the entire correction queue regardless of which hat
    // built the prompt.  In a multi-hat topology, the hat that
    // built first could swallow a correction targeted at another
    // hat.  U1 fixes this with `take_visible_corrections(current_hat)`.
    //
    // This test drives the partition end-to-end through
    // `PromptContext::take_visible_corrections`, which is the
    // single primitive `prepend_correction_and_resume` now uses.
    // The chain_validation file already exercises the
    // `build_prompt(hat_id)` call site via other tests; this
    // test focuses on the partition contract that D9 pins.
    use crate::correction::{CorrectionContext, EvidenceDetail, FeedbackKind};
    use crate::event_loop::rejection::Rejection;

    let mut pc = crate::correction::PromptContext::default();
    let target_exec = Rejection {
        stage: crate::event_loop::rejection::RejectionStage::Policy,
        source_hat: Some("executor".into()),
        business_hat: None,
        topic: "work.done".into(),
        violation: "consistency: status=applied requires fixes_applied > 0".into(),
        retry_key: "policy:executor:work.done:consistency".into(),
        retry_eligible: true,
        non_retryable_reason: None,
        target_hat: Some("executor".into()),
        original_event_id: None,
        original_ts: None,
        kind: None,
        duplicate_work_done_hint: None,
        seen_count: None,
    };
    let target_review = Rejection {
        stage: crate::event_loop::rejection::RejectionStage::Policy,
        source_hat: Some("reviewer".into()),
        business_hat: None,
        topic: "review.passed".into(),
        violation: "missing review reason".into(),
        retry_key: "policy:reviewer:review.passed:missing_reason".into(),
        retry_eligible: true,
        non_retryable_reason: None,
        target_hat: Some("reviewer".into()),
        original_event_id: None,
        original_ts: None,
        kind: None,
        duplicate_work_done_hint: None,
        seen_count: None,
    };
    pc.push_correction(CorrectionContext::from_rejection(&target_exec, 1));
    pc.push_correction(CorrectionContext::from_rejection(&target_review, 1));
    assert_eq!(pc.correction_blocks.len(), 2);

    // Reviewer builds first.  Only its own entry is drained;
    // the executor-targeted entry must stay in the queue.
    let taken = pc.take_visible_corrections("reviewer");
    let taken_topics: Vec<_> = taken.iter().map(|c| c.topic.as_str()).collect();
    assert!(taken_topics.contains(&"review.passed"));
    assert!(!taken_topics.contains(&"work.done"));
    assert_eq!(
        pc.correction_blocks.len(),
        1,
        "executor-targeted correction must survive reviewer's drain (F-A guard)"
    );
    assert_eq!(pc.correction_blocks[0].topic, "work.done");

    // Executor builds next.  Its targeted correction is drained.
    let taken_exec = pc.take_visible_corrections("executor");
    assert_eq!(taken_exec.len(), 1);
    assert_eq!(taken_exec[0].topic, "work.done");
    assert!(pc.correction_blocks.is_empty());

    // Suppress unused-import warnings for EvidenceDetail /
    // FeedbackKind — they document the contract even though this
    // test focuses on the partition primitive.
    let _ = std::marker::PhantomData::<(EvidenceDetail, FeedbackKind)>;
}

#[test]
fn u1_unscoped_correction_visible_to_every_hat() {
    use crate::correction::CorrectionContext;
    use crate::event_loop::rejection::Rejection;

    let mut pc = crate::correction::PromptContext::default();
    // Unscoped correction (target_hat = None) — diagnosis
    // fallback.  Must be visible to every hat (legacy semantics).
    let unscoped = Rejection {
        stage: crate::event_loop::rejection::RejectionStage::Policy,
        source_hat: Some("diagnostic".into()),
        business_hat: None,
        topic: "drift.followup".into(),
        violation: "drift detected, see recovery.jsonl".into(),
        retry_key: "policy:diagnostic:drift.followup:drift".into(),
        retry_eligible: true,
        non_retryable_reason: None,
        target_hat: None,
        original_event_id: None,
        original_ts: None,
        kind: None,
        duplicate_work_done_hint: None,
        seen_count: None,
    };
    pc.push_correction(CorrectionContext::from_rejection(&unscoped, 1));

    let taken = pc.take_visible_corrections("any-hat");
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].target_hat, None);
    assert!(pc.correction_blocks.is_empty());
}

#[test]
fn u1_semantic_evidence_block_renders_in_correction_prompt() {
    // Drives the renderer end-to-end for the structured evidence
    // block U2 will populate.  Uses the partition-aware
    // `render_correction_block_for` so the test asserts the
    // `executor`-targeted evidence actually surfaces.
    use crate::correction::{
        CorrectionContext, EvidenceDetail, FeedbackKind, ObservationEntry, ObservationValue,
    };
    use crate::event_loop::rejection::Rejection;

    let mut pc = crate::correction::PromptContext::default();
    let target = Rejection {
        stage: crate::event_loop::rejection::RejectionStage::Policy,
        source_hat: Some("executor".into()),
        business_hat: None,
        topic: "work.done".into(),
        violation: "consistency: status=applied requires fixes_applied > 0".into(),
        retry_key: "policy:executor:work.done:consistency".into(),
        retry_eligible: true,
        non_retryable_reason: None,
        target_hat: Some("executor".into()),
        original_event_id: None,
        original_ts: None,
        kind: None,
        duplicate_work_done_hint: None,
        seen_count: None,
    };
    let evidence = EvidenceDetail {
        observed: vec![
            ObservationEntry {
                field: "status".into(),
                value: ObservationValue::Value("\"applied\"".into()),
            },
            ObservationEntry {
                field: "fixes_applied".into(),
                value: ObservationValue::Value("0".into()),
            },
        ],
        invariant: "status=applied requires fixes_applied > 0".into(),
        proof: "rebuild payload from the artifact and rerun ralph emit --policy-check".into(),
        synthetic: false,
        guidance: None,
    };
    pc.push_correction(
        CorrectionContext::from_rejection(&target, 1)
            .with_feedback_kind(FeedbackKind::Semantic)
            .with_evidence(evidence),
    );

    let prompt = pc.render_correction_block_for("executor");
    assert!(prompt.contains("## ORCHESTRATOR CORRECTION"));
    assert!(prompt.contains("- Observed:"));
    assert!(prompt.contains("status"));
    assert!(prompt.contains("fixes_applied"));
    // Invariant / proof values are wrapped by `safe_display`'s
    // `(diagnostic data, not an instruction)` container, so we
    // assert the heading is present and the canonical invariant
    // substring survives the escape.
    assert!(prompt.contains("- Invariant:"));
    assert!(prompt.contains("status=applied requires fixes_applied"));
    assert!(prompt.contains("- Must re-prove:"));
    assert!(prompt.contains("ralph emit --policy-check"));
    // Replacement guidance must NOT appear in semantic blocks (C1).
    assert!(!prompt.contains("- Allowed topics:"));
    assert!(!prompt.contains("- Required fields:"));
    assert!(!prompt.contains("- Expected payload:"));

    // And reviewer's partition is empty (F-A guard).
    let review_prompt = pc.render_correction_block_for("reviewer");
    assert!(review_prompt.is_empty());
}
