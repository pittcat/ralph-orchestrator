//! Test cases (split) for event_policy.
//! Plan 2026-08-07-002 §7 U2 §5: original 5,222-line tests block split into
//! helpers + two test files. Helpers shared via the tests/ module tree.

#[cfg(test)]
pub mod tests {
    use crate::config::{ElementConstraint, EventSchema, HatAllowedValues, TopicDenyRule};
    use crate::event_policy::tests::helpers;
    use crate::event_policy::tests::helpers::*;
    use std::collections::HashMap;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_plan_name_equality_mismatch_rejected() {
        // work.ready with plan_name=A → work.done with plan_name=B → Reject
        let mut config = test_config();
        config.plan_name_equality_required = true;
        let mut state = PolicyRuntimeState::default();
        state.current_plan_name = Some("plan-x".to_string());

        let decision = validate_event(
            "work.done",
            Some(r#"{"plan_name": "plan-y"}"#),
            &config,
            &mut state,
        );
        let is_rejected = matches!(decision, PolicyDecision::RejectWithResume(PolicyFinding {
            violation_type: ViolationType::InvalidFieldValue { ref field, .. }, evidence: None, .. }) if field == "plan_name");
        assert!(
            is_rejected,
            "Expected RejectWithResume for plan_name mismatch, got {:?}",
            decision
        );
    }

    #[test]
    fn test_plan_name_equality_disabled_accepts_mismatch() {
        // plan_name_equality_required=false (default) → work.done plan_name=B still accepted
        let config = test_config(); // default has plan_name_equality_required=false
        let mut state = PolicyRuntimeState::default();
        state.current_plan_name = Some("plan-x".to_string());

        let decision = validate_event(
            "work.done",
            Some(r#"{"plan_name": "plan-y"}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_plan_name_equality_no_work_ready_skips_check() {
        // No work.ready → current_plan_name is None → skip check
        let mut config = test_config();
        config.plan_name_equality_required = true;
        let mut state = PolicyRuntimeState::default();
        // current_plan_name is None (no work.ready received)

        let decision = validate_event(
            "work.done",
            Some(r#"{"plan_name": "anything"}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn review_dimension_ready_dedup_first_accepted() {
        // Happy path: first `review.dimension.ready` for a
        // (plan, step, task, dimension) tuple is accepted.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let decision = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "First review.dimension.ready for a new key must be accepted"
        );
        assert!(
            state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness")
        );
    }

    #[test]
    fn review_dimension_ready_dedup_rejects_second_emit() {
        // Error path: 2nd `review.dimension.ready` with the
        // same (plan, step, task, dimension) tuple is rejected
        // with `RejectWithResume`.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");

        let first = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(
                second,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    evidence: None, .. }) if key == "p1::step-01::t1::correctness"
            ),
            "Second review.dimension.ready for same key must be rejected with DuplicateWorkDone, got {:?}",
            second
        );
    }

    #[test]
    fn review_dimension_ready_dedup_different_dimensions_both_accepted() {
        // Edge case: same (plan, step, task) but different
        // `dimension` → both accepted (serial walk through
        // review dimensions).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        let p1 = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let first = validate_event("review.dimension.ready", Some(&p1), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let p2 = review_dimension_ready_payload("p1", "step-01", "t1", "security");
        let second = validate_event("review.dimension.ready", Some(&p2), &config, &mut state);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "review.dimension.ready for same task but different dimension must be accepted, got {:?}",
            second
        );
    }

    #[test]
    fn review_dimension_ready_dedup_different_step_accepted() {
        // Edge case: same (plan, task, dimension) but different
        // `step` → still accepted (key includes step).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();

        let p1 = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let first = validate_event("review.dimension.ready", Some(&p1), &config, &mut state);
        assert_eq!(first, PolicyDecision::Accept);

        let p2 = review_dimension_ready_payload("p1", "step-02", "t1", "correctness");
        let second = validate_event("review.dimension.ready", Some(&p2), &config, &mut state);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "review.dimension.ready for same dim but different step must be accepted, got {:?}",
            second
        );
    }

    #[test]
    fn review_dimension_ready_dedup_is_recoverable() {
        // The DuplicateWorkDone violation (reused for
        // review.dimension.ready) must map to the recoverable
        // bucket so the runner publishes a `task.resume` with
        // a fix_hint.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
        let first = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);

        let second = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        let finding = match second {
            PolicyDecision::RejectWithResume(f) => f,
            other => panic!("expected RejectWithResume, got {:?}", other),
        };
        let class = is_recoverable_policy_finding(&finding);
        assert_eq!(
            class,
            Some(ReasonClass::DuplicateWorkDone),
            "review.dimension.ready dup must map to recoverable bucket, got {:?}",
            class
        );
    }

    #[test]
    fn review_dimension_ready_dedup_disabled_policy_accepts_all() {
        // When event policy is disabled, the dedup check must
        // be skipped.
        let mut config = test_config();
        config.enabled = false;
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");

        let first = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        let second = validate_event(
            "review.dimension.ready",
            Some(&payload),
            &config,
            &mut state,
        );
        assert_eq!(first, PolicyDecision::Accept);
        assert_eq!(
            second,
            PolicyDecision::Accept,
            "disabled policy must not dedup review.dimension.ready, got {:?}",
            second
        );
    }

    #[test]
    fn review_dimension_ready_dedup_missing_fields_skips_dedup() {
        // If payload is missing any of the dedup fields, the
        // dedup check cannot run — fall through to other
        // policy layers. The DuplicateWorkDone variant must
        // NOT appear.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"dimension":"correctness"}"#; // missing plan_name/step/task_id
        let decision = validate_event("review.dimension.ready", Some(payload), &config, &mut state);
        if let PolicyDecision::RejectWithResume(f) = &decision {
            assert!(
                !matches!(f.violation_type, ViolationType::DuplicateWorkDone { .. }),
                "missing-fields payload must not trigger DuplicateWorkDone, got {:?}",
                f.violation_type
            );
        }
    }

    #[test]
    fn review_dimension_failed_unknown_dimension_rejected() {
        // The 2026-07-01 ralph-e2e run emitted
        // `review.dimension.failed(dimension=unknown)` from a
        // dimension-reviewer payload that lost its
        // `original_dimension` field. The P1-A gate must
        // reject the unknown value with InvalidFieldValue
        // BEFORE the flow-scope stage can surface it as
        // `flow_unknown_emit`.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_failed_payload(Some("unknown"));
        let decision = validate_event(
            "review.dimension.failed",
            Some(&payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(
                decision,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::InvalidFieldValue { ref field, .. },
                    evidence: None, .. }) if field == "dimension"
            ),
            "unknown dimension must be rejected with InvalidFieldValue, got {:?}",
            decision
        );
    }

    #[test]
    fn review_dimension_failed_missing_dimension_rejected() {
        // The P1-A gate must also catch payloads that omit
        // the `dimension` field entirely. This is the
        // `MissingRequiredField` arm (not InvalidFieldValue).
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_dimension_failed_payload(None);
        let decision = validate_event(
            "review.dimension.failed",
            Some(&payload),
            &config,
            &mut state,
        );
        assert!(
            matches!(
                decision,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::MissingRequiredField { ref field },
                    evidence: None, .. }) if field == "dimension"
            ),
            "missing dimension must be rejected with MissingRequiredField, got {:?}",
            decision
        );
    }

    #[test]
    fn review_dimension_failed_whitelisted_dimension_accepted() {
        // Happy path: any of the 6 known dimensions is
        // accepted by the P1-A gate.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        for dim in &[
            "goal-alignment",
            "correctness",
            "testing",
            "maintainability",
            "project-standards",
            "adversarial",
        ] {
            let payload = review_dimension_failed_payload(Some(dim));
            let decision = validate_event(
                "review.dimension.failed",
                Some(&payload),
                &config,
                &mut state,
            );
            assert_eq!(
                decision,
                PolicyDecision::Accept,
                "whitelisted dimension {dim} must be accepted, got {:?}",
                decision
            );
        }
    }

    #[test]
    fn review_dimension_failed_missing_payload_falls_through() {
        // If the event has no payload (legacy / synthetic),
        // the P1-A gate cannot decode the dimension. The
        // check must fall through (no rejection from this
        // layer) so downstream schema/terminal layers can
        // surface their own precise error.
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("review.dimension.failed", None, &config, &mut state);
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "missing-payload event must fall through, got {:?}",
            decision
        );
    }

    #[test]
    fn review_dimension_ready_replay_from_events_populates_seen_keys() {
        // `PolicyRuntimeState::from_events` must populate the
        // dedup set from any prior `review.dimension.ready`
        // events in the JSONL so cross-batch replay is
        // honored.
        use std::io::Write;

        let jsonl = r#"{"topic":"review.dimension.ready","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
{"topic":"review.dimension.done","hat":"dimension-reviewer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            state
                .review_dimension_ready_seen_keys
                .contains("p1::step-01::t1::correctness"),
            "from_events must populate dedup set from prior review.dimension.ready, got {:?}",
            state.review_dimension_ready_seen_keys
        );
    }

    #[test]
    fn forge_wave_verified_duplicate_is_rejected() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_key":"pf-1","wave_id":"wave-1","candidate_commit_sha":"abc123"}"#;

        assert_eq!(
            validate_event("forge.wave.verified", Some(payload), &config, &mut state),
            PolicyDecision::Accept
        );
        assert!(
            state
                .forge_wave_verified_seen_keys
                .contains("pf-1::wave-1::abc123")
        );

        let second = validate_event("forge.wave.verified", Some(payload), &config, &mut state);
        assert!(
            matches!(
                second,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    ref topic,
                    violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                    evidence: None, .. }) if topic == "forge.wave.verified" && key == "pf-1::wave-1::abc123"
            ),
            "duplicate forge.wave.verified must be rejected, got {second:?}"
        );
    }

    #[test]
    fn forge_wave_verified_replay_populates_seen_keys() {
        use std::io::Write;

        let mut tmp = NamedTempFile::new().unwrap();
        writeln!(
            tmp,
            r#"{{"topic":"forge.wave.verified","payload":"{{\"plan_key\":\"pf-1\",\"wave_id\":\"wave-1\",\"candidate_commit_sha\":\"abc123\"}}"}}"#
        )
        .unwrap();
        tmp.flush().unwrap();

        let mut state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        let payload = r#"{"plan_key":"pf-1","wave_id":"wave-1","candidate_commit_sha":"abc123"}"#;
        let decision = validate_event(
            "forge.wave.verified",
            Some(payload),
            &test_config(),
            &mut state,
        );
        assert!(matches!(
            decision,
            PolicyDecision::RejectWithResume(PolicyFinding {
                violation_type: ViolationType::DuplicateWorkDone { .. },
                evidence: None,
                ..
            })
        ));
    }

    #[test]
    fn review_start_dedup_first_accepted() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_start_payload("p1", Some("step-01"), "t1");
        let decision = validate_event("review.start", Some(&payload), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
        assert!(state.review_start_seen_keys.contains("p1::t1::step-01"));
    }

    #[test]
    fn review_start_dedup_duplicate_rejected() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = review_start_payload("p1", Some("step-01"), "t1");
        assert_eq!(
            validate_event("review.start", Some(&payload), &config, &mut state),
            PolicyDecision::Accept
        );
        let second = validate_event("review.start", Some(&payload), &config, &mut state);
        assert!(
            matches!(second, PolicyDecision::RejectWithResume(_)),
            "duplicate review.start must be rejected, got {:?}",
            second
        );
    }

    #[test]
    fn review_start_dedup_different_task_accepted() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let p1 = review_start_payload("p1", Some("step-01"), "t1");
        let p2 = review_start_payload("p1", Some("step-01"), "t2");
        assert_eq!(
            validate_event("review.start", Some(&p1), &config, &mut state),
            PolicyDecision::Accept
        );
        assert_eq!(
            validate_event("review.start", Some(&p2), &config, &mut state),
            PolicyDecision::Accept
        );
    }

    #[test]
    fn review_start_dedup_missing_task_id_skips_dedup() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p1","step":"step-01"}"#; // missing task_id
        let decision = validate_event("review.start", Some(payload), &config, &mut state);
        if let PolicyDecision::RejectWithResume(f) = &decision {
            assert!(
                !matches!(f.violation_type, ViolationType::DuplicateWorkDone { .. }),
                "missing task_id must not trigger DuplicateWorkDone, got {:?}",
                f.violation_type
            );
        }
    }

    #[test]
    fn review_start_dedup_step_in_key() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        let without_step = review_start_payload("p1", None, "t1");
        let with_step = review_start_payload("p1", Some("step-01"), "t1");
        assert_eq!(
            validate_event("review.start", Some(&without_step), &config, &mut state),
            PolicyDecision::Accept
        );
        // Same plan/task but now with a step is a different key, so accepted.
        assert_eq!(
            validate_event("review.start", Some(&with_step), &config, &mut state),
            PolicyDecision::Accept
        );
        // Re-emitting the no-step payload should still be rejected.
        assert!(
            matches!(
                validate_event("review.start", Some(&without_step), &config, &mut state),
                PolicyDecision::RejectWithResume(_)
            ),
            "re-emitting no-step review.start must be rejected"
        );
    }

    #[test]
    fn review_start_replay_from_events_populates_seen_keys() {
        use std::io::Write;
        let jsonl = r#"{"topic":"review.start","hat":"coordinator","payload":"{\"plan_name\":\"p1\",\"task_id\":\"t1\",\"task_key\":\"k1\"}"}"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            state.review_start_seen_keys.contains("p1::t1"),
            "from_events must populate review.start dedup set, got {:?}",
            state.review_start_seen_keys
        );
    }

    #[test]
    fn review_start_replay_from_events_with_step_populates_seen_keys() {
        use std::io::Write;
        let jsonl = r#"{"topic":"review.start","hat":"coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"task_key\":\"k1\"}"}"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            state.review_start_seen_keys.contains("p1::t1::step-01"),
            "from_events must include step in review.start key, got {:?}",
            state.review_start_seen_keys
        );
    }

    #[test]
    fn review_start_prune_on_fix_applied_from_events() {
        use std::io::Write;
        let jsonl = r#"{"topic":"review.start","hat":"coordinator","payload":"{\"plan_name\":\"p1\",\"task_id\":\"t1\",\"task_key\":\"k1\"}"}
{"topic":"fix.applied","hat":"fixer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"task_key\":\"k1\"}"}"#;
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(jsonl.as_bytes()).unwrap();
        tmp.flush().unwrap();

        let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
        assert!(
            !state.review_start_seen_keys.contains("p1::t1"),
            "fix.applied replay must prune review.start dedup set, got {:?}",
            state.review_start_seen_keys
        );
    }

    #[test]
    fn review_start_prune_bucket_manual() {
        let mut state = PolicyRuntimeState::default();
        state.review_start_seen_keys.insert("p1::t1".into());
        state
            .review_start_seen_keys
            .insert("p1::t1::step-01".into());
        state.review_start_seen_keys.insert("p2::t1".into());

        state.prune_review_start_bucket("p1", "t1");

        assert!(!state.review_start_seen_keys.contains("p1::t1"));
        assert!(!state.review_start_seen_keys.contains("p1::t1::step-01"));
        assert!(state.review_start_seen_keys.contains("p2::t1"));
    }

    #[test]
    fn test_policy_decision_has_acknowledge_and_forward_variant() {
        // (a) AcknowledgeAndForward is constructible with a PolicyFinding.
        let finding = PolicyFinding {
            topic: "review.dimensions.complete".to_string(),
            violation_type: ViolationType::DuplicateWorkDone {
                key: "k".to_string(),
                hint: DuplicateWorkDoneHint::ReviewDimensionsComplete,
                seen_count: None,
            },
            message: "test".to_string(),
            evidence: None,
        };
        let decision = PolicyDecision::AcknowledgeAndForward(finding.clone());
        match decision {
            PolicyDecision::AcknowledgeAndForward(f) => {
                assert_eq!(f.topic, "review.dimensions.complete");
                // reason_code is derived from `violation_type` (per
                // `ViolationType::reason_code()`); verify the
                // `ReviewDimensionsComplete` hint mapping here.
                assert_eq!(
                    f.violation_type.reason_code(),
                    "duplicate_review_dimensions_complete"
                );
            }
            other => panic!("expected AcknowledgeAndForward, got {other:?}"),
        }

        // (b) Total enum variant count is 7 (Accept / Warn /
        // RejectWithResume / Hold / AcknowledgeAndForward / Block /
        // Ignore). If a future Unit adds another variant this
        // assertion fails fast and the author is forced to re-pin
        // the contract here.
        let all = [
            std::mem::discriminant(&PolicyDecision::Accept),
            std::mem::discriminant(&PolicyDecision::Warn(vec![])),
            std::mem::discriminant(&PolicyDecision::RejectWithResume(finding.clone())),
            std::mem::discriminant(&PolicyDecision::Hold(finding.clone())),
            std::mem::discriminant(&PolicyDecision::AcknowledgeAndForward(finding.clone())),
            std::mem::discriminant(&PolicyDecision::Block(finding.clone())),
            std::mem::discriminant(&PolicyDecision::Ignore(finding)),
        ];
        let unique: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(
            unique.len(),
            7,
            "PolicyDecision must have 7 distinct variants after U2"
        );
    }

    #[test]
    fn test_review_dimensions_complete_dedup_hit_returns_acknowledge_and_forward() {
        let mut config = test_config();
        // Allow the topic + fields used by `review.dimensions.complete`.
        config.schemas.insert(
            "review.dimensions.complete".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![
                    "plan_name".to_string(),
                    "step".to_string(),
                    "task_id".to_string(),
                    "fix_round".to_string(),
                ],
                ..Default::default()
            },
        );
        let mut state = PolicyRuntimeState::default();

        let payload = r#"{"plan_name":"p","step":"s","task_id":"t","fix_round":1}"#;
        // First emit is accepted.
        let first = validate_event_with_hat(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
            None,
        );
        assert_eq!(first, PolicyDecision::Accept);

        // Second emit with the same key returns AcknowledgeAndForward.
        let second = validate_event_with_hat(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
            None,
        );
        match second {
            PolicyDecision::AcknowledgeAndForward(finding) => {
                assert_eq!(finding.topic, "review.dimensions.complete");
                assert_eq!(
                    finding.violation_type.reason_code(),
                    "duplicate_review_dimensions_complete"
                );
            }
            other => panic!(
                "expected AcknowledgeAndForward, got {other:?}; \
                 the U2 silent-success carve-out must apply to dedup hits"
            ),
        }
    }

    #[test]
    fn test_review_dimensions_complete_first_emit_still_accepts() {
        let mut config = test_config();
        config.schemas.insert(
            "review.dimensions.complete".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![
                    "plan_name".to_string(),
                    "step".to_string(),
                    "task_id".to_string(),
                    "fix_round".to_string(),
                ],
                ..Default::default()
            },
        );
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","step":"s","task_id":"t","fix_round":7}"#;
        let decision = validate_event_with_hat(
            "review.dimensions.complete",
            Some(payload),
            &config,
            &mut state,
            None,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_other_topic_dedup_still_rejects_with_resume() {
        let mut config = test_config();
        config.schemas.insert(
            "work.done".to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: vec![
                    "plan_name".to_string(),
                    "step".to_string(),
                    "task_id".to_string(),
                ],
                ..Default::default()
            },
        );
        let mut state = PolicyRuntimeState::default();
        let payload = r#"{"plan_name":"p","step":"s","task_id":"t"}"#;
        // First emit accepted.
        let first = validate_event_with_hat("work.done", Some(payload), &config, &mut state, None);
        assert_eq!(first, PolicyDecision::Accept);
        // Second emit returns RejectWithResume (unchanged behaviour).
        let second = validate_event_with_hat("work.done", Some(payload), &config, &mut state, None);
        match second {
            PolicyDecision::RejectWithResume(finding) => {
                assert_eq!(finding.topic, "work.done");
            }
            other => panic!(
                "expected RejectWithResume for work.done dedup, got {other:?}; \
                 the U2 carve-out must NOT extend to work.done"
            ),
        }
    }

    #[test]
    fn test_duplicate_work_done_hint_has_review_dimensions_complete_variant() {
        let all = [
            DuplicateWorkDoneHint::DuplicateStallBypass,
            DuplicateWorkDoneHint::DuplicateSameStep,
            DuplicateWorkDoneHint::ReviewDimensionDuplicate,
            DuplicateWorkDoneHint::ReviewDimensionsComplete,
        ];
        let unique: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(
            unique.len(),
            4,
            "DuplicateWorkDoneHint must have 4 distinct variants after U6"
        );
    }

    #[test]
    fn test_review_dimensions_complete_duplicate_emits_distinct_reason_code() {
        let finding = PolicyFinding {
            topic: "review.dimensions.complete".to_string(),
            violation_type: ViolationType::DuplicateWorkDone {
                key: "p::s::t::0".to_string(),
                hint: DuplicateWorkDoneHint::ReviewDimensionsComplete,
                seen_count: None,
            },
            message: "test".to_string(),
            evidence: None,
        };
        assert_eq!(
            finding.violation_type.reason_code(),
            "duplicate_review_dimensions_complete",
            "ReviewDimensionsComplete hint MUST map to its own distinct reason_code"
        );
    }

    #[test]
    fn test_review_dimension_ready_duplicate_still_uses_review_dimension_duplicate() {
        let finding = PolicyFinding {
            topic: "review.dimension.ready".to_string(),
            violation_type: ViolationType::DuplicateWorkDone {
                key: "p::s::t::d".to_string(),
                hint: DuplicateWorkDoneHint::ReviewDimensionDuplicate,
                seen_count: None,
            },
            message: "test".to_string(),
            evidence: None,
        };
        assert_eq!(
            finding.violation_type.reason_code(),
            "duplicate_review_dimension_ready",
            "ReviewDimensionDuplicate hint must keep its distinct code (regression guard)"
        );
    }

    #[test]
    fn test_other_topics_dedup_hint_carries_in_envelope() {
        // U3 of plan 2026-07-05-005 (R3, R9): restore the stable
        // external contract per KTD-3 — single `duplicate_work_done`
        // reason_code for the `DuplicateSameStep` and
        // `DuplicateStallBypass` variants. The `hint` field on
        // `RecoveryDiagnosisEnvelope` carries the discriminator
        // (`duplicate_work_done_same_step` /
        // `duplicate_work_done_stall_bypass`) so post-mortem
        // tooling can still distinguish the two paths. This test
        // pins both surfaces.
        let cases = [
            (
                DuplicateWorkDoneHint::DuplicateStallBypass,
                "duplicate_work_done",
                "duplicate_work_done_stall_bypass",
            ),
            (
                DuplicateWorkDoneHint::DuplicateSameStep,
                "duplicate_work_done",
                "duplicate_work_done_same_step",
            ),
            (
                DuplicateWorkDoneHint::ReviewDimensionDuplicate,
                "duplicate_review_dimension_ready",
                "duplicate_review_dimension_ready",
            ),
            (
                DuplicateWorkDoneHint::ReviewDimensionsComplete,
                "duplicate_review_dimensions_complete",
                "duplicate_review_dimensions_complete",
            ),
        ];
        for (hint, expected_code, expected_hint) in cases {
            let finding = PolicyFinding {
                topic: "work.done".to_string(),
                violation_type: ViolationType::DuplicateWorkDone {
                    key: "p::s::t".to_string(),
                    hint,
                    seen_count: None,
                },
                message: "test".to_string(),
                evidence: None,
            };
            assert_eq!(
                finding.violation_type.reason_code(),
                expected_code,
                "{hint:?} must surface its stable reason_code"
            );
            assert_eq!(
                hint.as_hint_str(),
                expected_hint,
                "{hint:?} must surface its stable hint string"
            );
        }
    }

    #[test]
    fn test_distinct_reason_codes_invariant() {
        let codes = [
            (
                "DuplicateStallBypass",
                ViolationType::DuplicateWorkDone {
                    key: "k".to_string(),
                    hint: DuplicateWorkDoneHint::DuplicateStallBypass,
                    seen_count: None,
                }
                .reason_code(),
            ),
            (
                "DuplicateSameStep",
                ViolationType::DuplicateWorkDone {
                    key: "k".to_string(),
                    hint: DuplicateWorkDoneHint::DuplicateSameStep,
                    seen_count: None,
                }
                .reason_code(),
            ),
            (
                "ReviewDimensionDuplicate",
                ViolationType::DuplicateWorkDone {
                    key: "k".to_string(),
                    hint: DuplicateWorkDoneHint::ReviewDimensionDuplicate,
                    seen_count: None,
                }
                .reason_code(),
            ),
            (
                "ReviewDimensionsComplete",
                ViolationType::DuplicateWorkDone {
                    key: "k".to_string(),
                    hint: DuplicateWorkDoneHint::ReviewDimensionsComplete,
                    seen_count: None,
                }
                .reason_code(),
            ),
        ];
        let mut unique = std::collections::HashSet::new();
        for (_, code) in &codes {
            unique.insert(*code);
        }
        // U3 of plan 2026-07-05-005 (fix-plan §R3 / KTD-3): StallBypass
        // and SameStep share the legacy `duplicate_work_done`
        // reason_code per the stable external contract; the
        // disambiguation hint string travels on
        // `RecoveryDiagnosisEnvelope.hint`. The three "named"
        // lanes (StallBypass+SameStep collapsed, plus
        // ReviewDimensionDuplicate and ReviewDimensionsComplete)
        // must produce **3 distinct** codes. Merging any of
        // them would re-introduce the silent-success
        // misclassification — fail fast.
        assert_eq!(
            unique.len(),
            3,
            "expected 3 distinct reason codes (StallBypass+SameStep collapsed → \
             duplicate_work_done, plus ReviewDimensionDuplicate and ReviewDimensionsComplete); \
             got {codes:?}"
        );
        assert!(
            unique.contains("duplicate_work_done"),
            "DuplicateSameStep+DuplicateStallBypass must collapse to duplicate_work_done under U3"
        );
        assert!(
            unique.contains("duplicate_review_dimension_ready"),
            "ReviewDimensionDuplicate keeps its distinct code under U3"
        );
        assert!(
            unique.contains("duplicate_review_dimensions_complete"),
            "ReviewDimensionsComplete keeps its distinct code under U3"
        );
    }

    #[test]
    fn policy_check_does_not_require_handoff_envelope_when_disabled() {
        // Default-closed: no flags set, no envelope required.
        let policy = policy_minimal();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_options(
            "work.done",
            Some(&serde_json::to_string(&full_payload()).unwrap()),
            &policy,
            &mut state,
            None,
            &StubHandoff {
                enabled: false,
                validate_payload: false,
            },
        );
        assert!(
            matches!(decision, PolicyDecision::Accept),
            "disabled flag must not gate; got {:?}",
            decision
        );
    }

    #[test]
    fn policy_check_rejects_missing_handoff_envelope_when_validation_enabled() {
        let mut payload = full_payload();
        payload.as_object_mut().unwrap().remove("handoff_envelope");

        // U1: now actually wired — uses the original
        // `policy_minimal()` (no schema declared) and asserts
        // the `check_handoff_envelope` validator fires.
        let policy = policy_minimal();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_options(
            "work.done",
            Some(&serde_json::to_string(&payload).unwrap()),
            &policy,
            &mut state,
            None,
            &StubHandoff {
                enabled: true,
                validate_payload: true,
            },
        );
        match decision {
            PolicyDecision::RejectWithResume(f)
            | PolicyDecision::Hold(f)
            | PolicyDecision::AcknowledgeAndForward(f) => {
                assert!(
                    f.message.contains("handoff_envelope_missing"),
                    "missing envelope must surface; got finding: {:?}",
                    f
                );
            }
            other => panic!("expected rejection, got {:?}", other),
        }
    }

    #[test]
    fn policy_check_rejects_invalid_handoff_envelope_when_validation_enabled() {
        let mut payload = full_payload();
        payload["handoff_envelope"]["schema_version"] = serde_json::json!("wrong");
        let policy = policy_minimal();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_options(
            "work.done",
            Some(&serde_json::to_string(&payload).unwrap()),
            &policy,
            &mut state,
            None,
            &StubHandoff {
                enabled: true,
                validate_payload: true,
            },
        );
        match decision {
            PolicyDecision::RejectWithResume(f)
            | PolicyDecision::Hold(f)
            | PolicyDecision::AcknowledgeAndForward(f) => {
                assert!(
                    f.message
                        .contains("handoff_envelope_invalid_schema_version"),
                    "invalid schema version must surface; got finding: {:?}",
                    f
                );
            }
            other => panic!("expected rejection, got {:?}", other),
        }
    }

    #[test]
    fn policy_check_accepts_valid_handoff_envelope_when_validation_enabled() {
        let policy = policy_minimal();
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_options(
            "work.done",
            Some(&serde_json::to_string(&full_payload()).unwrap()),
            &policy,
            &mut state,
            None,
            &StubHandoff {
                enabled: true,
                validate_payload: true,
            },
        );
        assert!(
            matches!(decision, PolicyDecision::Accept),
            "valid envelope must accept; got {:?}",
            decision
        );
    }

    #[test]
    fn handoff_envelope_validation_enabled_gate_is_correct() {
        let cfg_disabled = StubHandoff {
            enabled: false,
            validate_payload: true,
        };
        let cfg_validate_only = StubHandoff {
            enabled: true,
            validate_payload: false,
        };
        let cfg_full = StubHandoff {
            enabled: true,
            validate_payload: true,
        };
        assert!(!handoff_envelope_validation_enabled(
            Some("{}"),
            &cfg_disabled
        ));
        assert!(!handoff_envelope_validation_enabled(
            Some("{}"),
            &cfg_validate_only
        ));
        assert!(handoff_envelope_validation_enabled(Some("{}"), &cfg_full));
        assert!(!handoff_envelope_validation_enabled(None, &cfg_full));
    }

    #[test]
    fn event_loop_handoff_config_adapter_projects_typed_config() {
        let cfg = HandoffEnvelopeConfig {
            enabled: true,
            prompt_injection: true,
            validate_payload: true,
            emit_result_summary: false,
        };
        let adapter = EventLoopHandoffConfig {
            handoff_envelope: &cfg,
        };
        assert!(adapter.handoff_envelope_enabled());
        assert!(adapter.handoff_envelope_validate_payload());

        let cfg_off = HandoffEnvelopeConfig::default();
        let adapter = EventLoopHandoffConfig {
            handoff_envelope: &cfg_off,
        };
        assert!(!adapter.handoff_envelope_enabled());
        assert!(!adapter.handoff_envelope_validate_payload());
    }

    #[test]
    fn payload_consistency_happy_payload_not_matching_rule_is_accepted() {
        // S1: payload does NOT satisfy the rule's `when` → Accept, no finding.
        let config = consistency_config(true, vec![fix_done_contradiction_rule()]);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event(
            "fix.done",
            Some(r#"{"review_verdict":"passed","fixes_applied":2,"fix_status":"applied"}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn payload_consistency_hitting_payload_is_rejected_with_semantic_gate() {
        // S2: payload satisfies `when` → RejectWithResume carrying a
        // SemanticGateViolation whose gate is `payload_consistency:<id>`.
        let config = consistency_config(true, vec![fix_done_contradiction_rule()]);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(HITTING_PAYLOAD), &config, &mut state);

        let PolicyDecision::RejectWithResume(finding) = decision else {
            panic!("expected RejectWithResume, got {decision:?}");
        };
        let ViolationType::SemanticGateViolation { gate, context, .. } = &finding.violation_type
        else {
            panic!(
                "expected SemanticGateViolation, got {:?}",
                finding.violation_type
            );
        };
        assert!(
            gate.starts_with("payload_consistency:"),
            "gate must start with payload_consistency: prefix, got {gate}"
        );
        assert!(
            gate.contains("fix-done-no-fixes"),
            "gate must contain the rule id, got {gate}"
        );
        assert_eq!(gate, "payload_consistency:fix-done-no-fixes");
        // context should carry the rule's actionable message.
        assert!(
            context.contains("no fixes were applied"),
            "context should reflect rule message, got {context}"
        );
    }

    #[test]
    fn payload_consistency_disabled_does_not_fire() {
        // S4: enabled=false with a hitting payload → Accept (gate off by default).
        let config = consistency_config(false, vec![fix_done_contradiction_rule()]);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(HITTING_PAYLOAD), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn payload_consistency_rule_is_scoped_to_its_topic() {
        // Topic filter: rule declared for fix.done, emit work.done with a
        // hitting payload → Accept (rule must not fire for other topics).
        let config = consistency_config(true, vec![fix_done_contradiction_rule()]);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("work.done", Some(HITTING_PAYLOAD), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn payload_consistency_first_hit_in_declaration_order_wins() {
        // Two rules both hit; the surfaced finding carries the FIRST
        // rule's id (stable declaration order).
        let first = consistency_rule(
            "first-rule",
            "fix.done",
            serde_json::json!({"field": "review_verdict", "eq": "blocked"}),
            "first rule message",
        );
        let second = consistency_rule(
            "second-rule",
            "fix.done",
            serde_json::json!({"field": "fix_status", "eq": "applied"}),
            "second rule message",
        );
        let config = consistency_config(true, vec![first, second]);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(HITTING_PAYLOAD), &config, &mut state);

        let PolicyDecision::RejectWithResume(finding) = decision else {
            panic!("expected RejectWithResume, got {decision:?}");
        };
        let ViolationType::SemanticGateViolation { gate, .. } = &finding.violation_type else {
            panic!(
                "expected SemanticGateViolation, got {:?}",
                finding.violation_type
            );
        };
        assert_eq!(gate, "payload_consistency:first-rule");
    }

    #[test]
    fn payload_consistency_rule_message_is_surfaced_to_agent() {
        // Message passthrough: the finding message/context reflects the
        // rule's `message` so the agent gets actionable guidance.
        let rule = consistency_rule(
            "msg-rule",
            "fix.done",
            serde_json::json!({"field": "review_verdict", "eq": "blocked"}),
            "ACTIONABLE: re-run the fixer before claiming fix.done",
        );
        let config = consistency_config(true, vec![rule]);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("fix.done", Some(HITTING_PAYLOAD), &config, &mut state);

        let PolicyDecision::RejectWithResume(finding) = decision else {
            panic!("expected RejectWithResume, got {decision:?}");
        };
        let ViolationType::SemanticGateViolation { context, .. } = &finding.violation_type else {
            panic!(
                "expected SemanticGateViolation, got {:?}",
                finding.violation_type
            );
        };
        assert!(
            context.contains("ACTIONABLE: re-run the fixer"),
            "context must carry the rule message, got {context}"
        );
        assert!(
            finding.message.contains("ACTIONABLE: re-run the fixer"),
            "finding.message must carry the rule message, got {}",
            finding.message
        );
        assert!(
            finding.message.contains("payload_consistency:msg-rule"),
            "finding.message must name the gate, got {}",
            finding.message
        );
    }

    #[test]
    fn evaluate_candidate_emit_accepts_valid_payload() {
        let config = candidate_emit_test_config();
        let hat_id = ralph_proto::HatId::new("worker");
        let payload = r#"{"task_key": "task-123"}"#;

        let result = evaluate_candidate_emit(&config, &hat_id, "work.ready", payload, None)
            .expect("evaluation should succeed");
        assert_eq!(result.policy_decision, "accept");
        assert!(
            result.reasons.is_empty(),
            "accepted emit should have no reasons, got {:?}",
            result.reasons
        );
        assert_eq!(
            result.next_hat_candidates,
            NextHatCandidates::Verified { hats: Vec::new() },
            "no subscribers should yield a verified empty routing set"
        );
    }

    #[test]
    fn evaluate_candidate_emit_rejects_missing_required_field() {
        let config = candidate_emit_test_config();
        let hat_id = ralph_proto::HatId::new("worker");
        // Missing the required `task_key` field.
        let payload = r#"{"other_field": "value"}"#;

        let result = evaluate_candidate_emit(&config, &hat_id, "work.ready", payload, None)
            .expect("evaluation should succeed");
        assert_eq!(result.policy_decision, "reject");
        assert!(
            !result.reasons.is_empty(),
            "rejected emit should have at least one reason"
        );
        // The reason should mention the missing field (exact gate label
        // depends on the validation path; check that at least one reason
        // exists).
        assert_eq!(
            result.reasons[0].reason_code, "missing_required_field",
            "expected missing_required_field reason, got {:?}",
            result.reasons[0]
        );
        assert_eq!(
            result.next_hat_candidates,
            NextHatCandidates::Verified { hats: Vec::new() },
            "rejected emit should not surface downstream routing candidates"
        );
        assert!(
            result.projection.is_none(),
            "rejected emit should not surface a projection preview"
        );
    }

    #[test]
    fn evaluate_candidate_emit_equivalence_with_validate() {
        let config = candidate_emit_test_config();
        let hat_id = ralph_proto::HatId::new("worker");
        let policy_config = config.event_loop.event_policy.as_ref().unwrap();

        // Same valid payload: both paths should accept.
        let valid_payload = r#"{"task_key": "abc"}"#;
        let candidate =
            evaluate_candidate_emit(&config, &hat_id, "work.ready", valid_payload, None)
                .expect("evaluation");

        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_hat(
            "work.ready",
            Some(valid_payload),
            policy_config,
            &mut state,
            Some("worker"),
        );

        // evaluate_candidate_emit should say accept when validate_event_with_hat
        // says Accept or Warn.
        assert_eq!(
            candidate.policy_decision, "accept",
            "evaluate_candidate_emit must accept when validate_event_with_hat is {:?}",
            decision
        );
        assert_eq!(
            candidate.next_hat_candidates,
            NextHatCandidates::Verified { hats: Vec::new() }
        );

        // Same invalid payload (missing field): both paths should reject.
        let invalid_payload = r#"{}"#;
        let candidate2 =
            evaluate_candidate_emit(&config, &hat_id, "work.ready", invalid_payload, None)
                .expect("evaluation");
        assert_eq!(candidate2.policy_decision, "reject");
        assert_eq!(
            candidate2.next_hat_candidates,
            NextHatCandidates::Verified { hats: Vec::new() }
        );
        assert!(candidate2.projection.is_none());
    }

    #[test]
    fn evaluate_candidate_emit_accepted_includes_projection() {
        // RED phase: build_projection_preview currently returns None unconditionally.
        // After U3 GREEN, accepted events must include a projection with state_changes.
        let config = projection_test_config();
        let hat_id = ralph_proto::HatId::new("reviewer");
        let payload = serde_json::json!({
            "plan_name": "myplan",
            "task_id": "task-1"
        });

        let result =
            evaluate_candidate_emit(&config, &hat_id, "review.start", &payload.to_string(), None)
                .expect("evaluation should succeed");

        assert_eq!(
            result.policy_decision, "accept",
            "review.start with plan_name and task_id must be accepted"
        );
        assert!(
            result.projection.is_some(),
            "accepted event MUST include projection with state_changes, got None"
        );
        assert_eq!(
            result.next_hat_candidates,
            NextHatCandidates::Verified { hats: Vec::new() }
        );
        let preview = result.projection.unwrap();
        assert!(
            !preview.state_changes.is_empty(),
            "accepted event projection state_changes must not be empty"
        );
    }

    #[test]
    fn evaluate_candidate_emit_rejected_has_no_projection() {
        // Rejected events must NOT include a projection.
        let config = projection_test_config();
        let hat_id = ralph_proto::HatId::new("reviewer");
        // Missing required plan_name and task_id.
        let payload = serde_json::json!({});

        let result =
            evaluate_candidate_emit(&config, &hat_id, "review.start", &payload.to_string(), None)
                .expect("evaluation should succeed");

        assert_eq!(
            result.policy_decision, "reject",
            "review.start without required fields must be rejected"
        );
        assert!(
            result.projection.is_none(),
            "rejected event must NOT include projection, got {:?}",
            result.projection
        );
        assert_eq!(
            result.next_hat_candidates,
            NextHatCandidates::Verified { hats: Vec::new() }
        );
    }
}
