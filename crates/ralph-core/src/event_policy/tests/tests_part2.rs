// Test cases (split) for event_policy.
// Plan 2026-08-07-002 §7 U2 §5: original 5,222-line tests block split into
// helpers + two test files. Helpers shared via the tests/ module tree.

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
// Restored regression tests omitted during the module split.
#[test]
fn c7_review_dimensions_complete_accepts_skipped_with_null_findings() {
    let mut config = test_config_with_enforce_and_resume();
    let mut rw = HashMap::new();
    rw.insert("status".to_string(), serde_json::json!("done"));
    insert_review_dimensions_schema(&mut config, "findings_file", true, Vec::new(), rw, true);
    let mut state = PolicyRuntimeState::default();
    // `status: skipped` with null findings_file must be accepted.
    let payload = r#"{
        "dimensions": [
            {"dimension":"goal-alignment","status":"done","findings_file":"/tmp/ga.md"},
            {"dimension":"correctness","status":"skipped","findings_file":null}
        ]
    }"#;
    let decision = validate_event(
        "review.dimensions.complete",
        Some(payload),
        &config,
        &mut state,
    );
    assert_eq!(
        decision,
        PolicyDecision::Accept,
        "C7 positive: status=skipped with null findings_file is allowed, got {:?}",
        decision
    );
}

fn redteam_queue_config() -> EventPolicyConfig {
    let mut config = test_config();
    let schemas = [
        (
            "redteam.attack.mapped",
            vec!["experiment_count"],
        ),
        (
            "redteam.experiment.done",
            vec!["experiment_id"],
        ),
        (
            "redteam.experiment.next",
            vec![
                "next_experiment_id",
                "completed_count",
                "remaining_count",
                "accepted_count",
                "rejected_count",
                "evidence_board_path",
            ],
        ),
        (
            "redteam.evidence.gated",
            vec![
                "qualified_experiment_ids",
                "qualified_experiment_count",
                "rejected_experiment_count",
                "total_experiment_count",
            ],
        ),
    ];
    for (topic, required_fields) in schemas {
        config.schemas.insert(
            topic.to_string(),
            EventSchema {
                payload: Some(PayloadType::JsonObject),
                required_fields: required_fields
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                ..Default::default()
            },
        );
    }
    config
}

fn redteam_done(experiment_id: &str) -> String {
    serde_json::json!({"experiment_id": experiment_id}).to_string()
}

fn redteam_next(next_id: &str, completed: u64, remaining: u64, accepted: u64, rejected: u64) -> String {
    serde_json::json!({
        "next_experiment_id": next_id,
        "completed_count": completed,
        "remaining_count": remaining,
        "accepted_count": accepted,
        "rejected_count": rejected,
        "evidence_board_path": ".ralph/red-team/07-evidence-board.md"
    })
    .to_string()
}

#[test]
fn redteam_queue_rejects_duplicate_attack_mapping_in_same_ledger() {
    let config = redteam_queue_config();
    let mut state = PolicyRuntimeState::default();

    assert_eq!(
        validate_event(
            "redteam.attack.mapped",
            Some(r#"{"experiment_count":2}"#),
            &config,
            &mut state,
        ),
        PolicyDecision::Accept
    );

    let duplicate = validate_event(
        "redteam.attack.mapped",
        Some(r#"{"experiment_count":2}"#),
        &config,
        &mut state,
    );
    assert!(
        matches!(duplicate, PolicyDecision::RejectWithResume(PolicyFinding {
            violation_type: ViolationType::SemanticGateViolation { ref gate, .. },
            ..
        }) if gate == "redteam_experiment_queue_consistency"),
        "duplicate attack mapping in one accepted ledger must be rejected, got {duplicate:?}"
    );
}

#[test]
fn redteam_queue_rejects_counter_drift_and_duplicate_handoff() {
    let config = redteam_queue_config();
    let mut state = PolicyRuntimeState::default();

    assert_eq!(
        validate_event(
            "redteam.attack.mapped",
            Some(r#"{"experiment_count":2}"#),
            &config,
            &mut state,
        ),
        PolicyDecision::Accept
    );
    assert_eq!(
        validate_event(
            "redteam.experiment.done",
            Some(&redteam_done("RTE-001")),
            &config,
            &mut state,
        ),
        PolicyDecision::Accept
    );

    let drifted = redteam_next("RTE-002", 1, 1, 1, 1);
    let drift = validate_event("redteam.experiment.next", Some(&drifted), &config, &mut state);
    assert!(
        matches!(drift, PolicyDecision::RejectWithResume(PolicyFinding {
            violation_type: ViolationType::SemanticGateViolation { ref gate, .. },
            ..
        }) if gate == "redteam_experiment_queue_consistency"),
        "counter drift must be rejected, got {drift:?}"
    );

    let valid = redteam_next("RTE-002", 1, 1, 1, 0);
    assert_eq!(
        validate_event("redteam.experiment.next", Some(&valid), &config, &mut state),
        PolicyDecision::Accept
    );
    let duplicate = validate_event("redteam.experiment.next", Some(&valid), &config, &mut state);
    assert!(
        matches!(duplicate, PolicyDecision::RejectWithResume(PolicyFinding {
            violation_type: ViolationType::SemanticGateViolation { ref gate, .. },
            ..
        }) if gate == "redteam_experiment_queue_consistency"),
        "same queue handoff must be rejected, got {duplicate:?}"
    );
}

#[test]
fn redteam_queue_rejects_skip_and_final_aggregate_drift() {
    let config = redteam_queue_config();
    let mut state = PolicyRuntimeState::default();
    assert_eq!(
        validate_event(
            "redteam.attack.mapped",
            Some(r#"{"experiment_count":2}"#),
            &config,
            &mut state,
        ),
        PolicyDecision::Accept
    );
    assert_eq!(
        validate_event(
            "redteam.experiment.done",
            Some(&redteam_done("RTE-001")),
            &config,
            &mut state,
        ),
        PolicyDecision::Accept
    );
    assert_eq!(
        validate_event(
            "redteam.experiment.next",
            Some(&redteam_next("RTE-002", 1, 1, 0, 1)),
            &config,
            &mut state,
        ),
        PolicyDecision::Accept
    );

    let skipped = validate_event(
        "redteam.experiment.done",
        Some(&redteam_done("RTE-003")),
        &config,
        &mut state,
    );
    assert!(
        matches!(skipped, PolicyDecision::RejectWithResume(_)),
        "done for an ID other than the pending handoff must be rejected, got {skipped:?}"
    );

    assert_eq!(
        validate_event(
            "redteam.experiment.done",
            Some(&redteam_done("RTE-002")),
            &config,
            &mut state,
        ),
        PolicyDecision::Accept
    );
    let aggregate_drift = serde_json::json!({
        "qualified_experiment_ids": ["RTE-001"],
        "qualified_experiment_count": 1,
        "rejected_experiment_count": 0,
        "total_experiment_count": 2
    })
    .to_string();
    let final_decision = validate_event(
        "redteam.evidence.gated",
        Some(&aggregate_drift),
        &config,
        &mut state,
    );
    assert!(
        matches!(final_decision, PolicyDecision::RejectWithResume(_)),
        "final aggregate drift must be rejected, got {final_decision:?}"
    );

    let null_ids = serde_json::json!({
        "qualified_experiment_ids": null,
        "qualified_experiment_count": 1,
        "rejected_experiment_count": 1,
        "total_experiment_count": 2
    })
    .to_string();
    let null_ids_decision = validate_event(
        "redteam.evidence.gated",
        Some(&null_ids),
        &config,
        &mut state,
    );
    assert!(
        matches!(null_ids_decision, PolicyDecision::RejectWithResume(_)),
        "null qualified IDs must not pass final aggregate validation, got {null_ids_decision:?}"
    );
}

#[test]
fn redteam_queue_replay_restores_handoff_dedup_state() {
    let config = redteam_queue_config();
    let mut events = NamedTempFile::new().unwrap();
    let lines = [
        serde_json::json!({
            "topic": "redteam.attack.mapped",
            "payload": {"experiment_count": 2}
        }),
        serde_json::json!({
            "topic": "redteam.experiment.done",
            "payload": {"experiment_id": "RTE-001"}
        }),
        serde_json::json!({
            "topic": "redteam.experiment.next",
            "payload": serde_json::from_str::<Value>(&redteam_next("RTE-002", 1, 1, 1, 0)).unwrap()
        }),
    ];
    for line in lines {
        writeln!(events, "{line}").unwrap();
    }
    events.flush().unwrap();

    let mut state = PolicyRuntimeState::from_events(events.path(), &config).unwrap();
    assert_eq!(state.redteam_experiment_total, Some(2));
    assert_eq!(state.redteam_experiment_done_count, 1);
    let duplicate = validate_event(
        "redteam.experiment.next",
        Some(&redteam_next("RTE-002", 1, 1, 1, 0)),
        &config,
        &mut state,
    );
    assert!(
        matches!(duplicate, PolicyDecision::RejectWithResume(_)),
        "replayed queue state must reject duplicate handoff, got {duplicate:?}"
    );
}

#[test]
fn c7_review_dimensions_complete_allowed_values_on_status() {
    let mut config = test_config_with_enforce_and_resume();
    insert_review_dimensions_schema(
        &mut config,
        "status",
        true,
        vec![
            serde_json::json!("done"),
            serde_json::json!("skipped"),
            serde_json::json!("failed"),
        ],
        HashMap::new(),
        false,
    );
    let mut state = PolicyRuntimeState::default();
    let payload = r#"{
        "dimensions": [
            {"dimension":"goal-alignment","status":"bogus"}
        ]
    }"#;
    let decision = validate_event(
        "review.dimensions.complete",
        Some(payload),
        &config,
        &mut state,
    );
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "C7 allowed_values: status='bogus' MUST be rejected, got {:?}",
        decision
    );
}

#[test]
fn c7_review_dimensions_complete_missing_required_field() {
    let mut config = test_config_with_enforce_and_resume();
    insert_review_dimensions_schema(
        &mut config,
        "findings_file",
        true,
        Vec::new(),
        HashMap::new(),
        false,
    );
    let mut state = PolicyRuntimeState::default();
    let payload = r#"{
        "dimensions": [
            {"dimension":"goal-alignment"}
        ]
    }"#;
    let decision = validate_event(
        "review.dimensions.complete",
        Some(payload),
        &config,
        &mut state,
    );
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "C7 required: missing findings_file MUST be rejected, got {:?}",
        decision
    );
}

/// T-U7-02 / R11: a string payload that is a parseable JSON
/// object is normalized to the object form and accepted.
/// Required-field validation runs against the normalized
/// object, not the original string.

#[test]
fn c7_review_dimensions_complete_silent_drop_done_with_null_findings() {
    let mut config = test_config_with_enforce_and_resume();
    let mut rw = HashMap::new();
    rw.insert("status".to_string(), serde_json::json!("done"));
    insert_review_dimensions_schema(&mut config, "findings_file", true, Vec::new(), rw, true);
    let mut state = PolicyRuntimeState::default();
    // 6 dimensions, last 4 are fake `status: done, findings_file: null`.
    let payload = r#"{
        "dimensions": [
            {"dimension":"goal-alignment","status":"done","findings_file":"/tmp/ga.md"},
            {"dimension":"correctness","status":"done","findings_file":"/tmp/co.md"},
            {"dimension":"testing","status":"done","findings_file":null},
            {"dimension":"maintainability","status":"done","findings_file":null},
            {"dimension":"project-standards","status":"done","findings_file":null},
            {"dimension":"adversarial","status":"done","findings_file":null}
        ]
    }"#;
    let decision = validate_event(
        "review.dimensions.complete",
        Some(payload),
        &config,
        &mut state,
    );
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "C7: status=done with null findings_file MUST be rejected, got {:?}",
        decision
    );
}

#[test]
fn p0d_review_complete_fix_plan_file_null_literal_is_rejected() {
    let mut config = test_config_with_enforce_and_resume();
    config.schemas.insert(
        "review.complete".to_string(),
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![
                "plan_name".to_string(),
                "fix_round".to_string(),
                "fix_plan_file".to_string(),
                "verdict".to_string(),
                "residual_findings_count".to_string(),
                "findings_summary".to_string(),
                "task_id".to_string(),
                "task_key".to_string(),
                "step".to_string(),
                "findings_count".to_string(),
            ],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        },
    );
    let mut state = PolicyRuntimeState::default();
    // Same payload the ralph-e2e run emitted (note: `fix_plan_file: null`
    // is JSON null, not the string `"null"`).
    let payload = r#"{"plan_name":"python-sort-algorithms","fix_round":1,"fix_plan_file":null,"verdict":"pass","residual_findings_count":0,"findings_summary":"no findings","task_id":"task-1782311559-5071","task_key":"ce-executor:python-sort-algorithms:fix-01:u1-sorted-comparison-impl","step":"fix-01","findings_count":0}"#;
    let decision = validate_event("review.complete", Some(payload), &config, &mut state);
    match decision {
        PolicyDecision::RejectWithResume(finding) => {
            assert!(
                matches!(
                    finding.violation_type,
                    ViolationType::PayloadTypeMismatch { ref expected, ref actual }
                    if expected == "string" && actual == "null"
                ),
                "P0-D: expected PayloadTypeMismatch(string, null), got {:?}",
                finding
            );
        }
        other => panic!("P0-D: expected RejectWithResume, got {:?}", other),
    }
}

/// 2026-06-24 P0-D positive case: `fix_plan_file` as the
/// schema-required string `"null"` (no fix plan) must be
/// accepted when all required fields are present.

#[test]
fn p0d_review_complete_fix_plan_file_string_null_is_accepted() {
    let mut config = test_config_with_enforce_and_resume();
    config.schemas.insert(
        "review.complete".to_string(),
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![
                "plan_name".to_string(),
                "fix_round".to_string(),
                "fix_plan_file".to_string(),
                "verdict".to_string(),
                "residual_findings_count".to_string(),
                "findings_summary".to_string(),
                "task_id".to_string(),
                "task_key".to_string(),
                "step".to_string(),
                "findings_count".to_string(),
            ],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        },
    );
    let mut state = PolicyRuntimeState::default();
    // `fix_plan_file` is the literal string `"null"` (note the
    // escaped quotes inside the JSON string).
    let payload = r#"{"plan_name":"python-sort-algorithms","fix_round":0,"fix_plan_file":"null","verdict":"pass","residual_findings_count":0,"findings_summary":"no findings","task_id":"task-1782310833-0494","task_key":"ce-executor:python-sort-algorithms:step-02:u0-quick-sort-impl","step":"step-02","findings_count":0}"#;
    let decision = validate_event("review.complete", Some(payload), &config, &mut state);
    assert_eq!(
        decision,
        PolicyDecision::Accept,
        "P0-D positive: string \"null\" must be accepted, got {:?}",
        decision
    );
}

// 2026-07-03-005 plan (P0 fix C7): element_constraints rejects
// `review.dimensions.complete` with `status: done` and a null
// `findings_file`. Without this, the agent fabricates 4 of 6
// dimensions as `status: done, findings_file: null` and the shipper
// walks `pass_with_residuals` based on the inflated summary.

fn insert_review_dimensions_schema(
    config: &mut EventPolicyConfig,
    field: &str,
    required: bool,
    allowed: Vec<serde_json::Value>,
    required_when: HashMap<String, serde_json::Value>,
    forbid_null: bool,
) {
    let constraint = ElementConstraint {
        field: field.to_string(),
        required,
        allowed_values: allowed,
        required_when,
        forbid_null_when_required: forbid_null,
    };
    let mut ec = HashMap::new();
    ec.insert("dimensions".to_string(), constraint);
    config.schemas.insert(
        "review.dimensions.complete".to_string(),
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec!["dimensions".to_string()],
            element_constraints: ec,
            ..Default::default()
        },
    );
}

#[test]
fn p1_3_duplicate_test_failed_same_fix_round_rejected() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = test_result_payload("p1", "step-01", "t1", 0);

    let first = validate_event("test.failed", Some(&payload), &config, &mut state);
    assert_eq!(first, PolicyDecision::Accept);

    let second = validate_event("test.failed", Some(&payload), &config, &mut state);
    assert!(
        matches!(
            second,
            PolicyDecision::RejectWithResume(PolicyFinding {
                violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                evidence: None, .. }) if key == "p1::step-01::t1::0"
        ),
        "Second test.failed for same fix_round must be rejected, got {:?}",
        second
    );
}

#[test]
fn p1_3_duplicate_test_passed_same_fix_round_rejected() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = test_result_payload("p1", "step-01", "t1", 0);

    let first = validate_event("test.passed", Some(&payload), &config, &mut state);
    assert_eq!(first, PolicyDecision::Accept);

    let second = validate_event("test.passed", Some(&payload), &config, &mut state);
    assert!(
        matches!(
            second,
            PolicyDecision::RejectWithResume(PolicyFinding {
                violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                evidence: None, .. }) if key == "p1::step-01::t1::0"
        ),
        "Second test.passed for same fix_round must be rejected, got {:?}",
        second
    );
}

#[test]
fn p1_3_duplicate_work_ready_different_step_accepted() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();

    let p1 = work_ready_payload("p1", "step-01", "t1");
    let first = validate_event("work.ready", Some(&p1), &config, &mut state);
    assert_eq!(first, PolicyDecision::Accept);

    let p2 = work_ready_payload("p1", "step-02", "t1");
    let second = validate_event("work.ready", Some(&p2), &config, &mut state);
    assert_eq!(
        second,
        PolicyDecision::Accept,
        "work.ready for a different step must be accepted"
    );
}

#[test]
fn p1_3_duplicate_work_ready_first_accepted() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = work_ready_payload("p1", "step-01", "t1");
    let decision = validate_event("work.ready", Some(&payload), &config, &mut state);
    assert_eq!(
        decision,
        PolicyDecision::Accept,
        "First work.ready for a new (plan, step, task) tuple must be accepted"
    );
    assert!(state.work_ready_seen_keys.contains_key("p1::step-01::t1"));
    assert_eq!(state.work_ready_seen_keys["p1::step-01::t1"], 1);
}

#[test]
fn p1_3_duplicate_work_ready_second_rejected() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = work_ready_payload("p1", "step-01", "t1");

    let first = validate_event("work.ready", Some(&payload), &config, &mut state);
    assert_eq!(first, PolicyDecision::Accept);

    let second = validate_event("work.ready", Some(&payload), &config, &mut state);
    assert!(
        matches!(
            second,
            PolicyDecision::RejectWithResume(PolicyFinding {
                violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                evidence: None, .. }) if key == "p1::step-01::t1"
        ),
        "Second work.ready for same key must be rejected with DuplicateWorkDone, got {:?}",
        second
    );
}

#[test]
fn p1_3_fix_applied_prunes_test_result_buckets() {
    let mut state = PolicyRuntimeState::default();
    state
        .test_passed_seen_keys
        .insert("p1::step-01::t1::0".into());
    state
        .test_failed_seen_keys
        .insert("p1::step-01::t1::0".into());
    state
        .test_passed_seen_keys
        .insert("p1::step-01::t2::0".into());

    state.prune_test_result_buckets("p1", "step-01", "t1");

    assert!(!state.test_passed_seen_keys.contains("p1::step-01::t1::0"));
    assert!(!state.test_failed_seen_keys.contains("p1::step-01::t1::0"));
    // Sibling task t2 is preserved
    assert!(state.test_passed_seen_keys.contains("p1::step-01::t2::0"));
}

#[test]
fn p1_3_fix_applied_prunes_work_ready_bucket() {
    // U5 of plan 2026-07-05-005 (fix-plan §R8): the dedup
    // counter is observation, not dedup state. The bucket
    // classification moves to `pruned_work_ready_buckets`,
    // but the dedup entries (and their counts) survive the
    // prune. Update the assertion accordingly: keys under
    // the pruned bucket stay in `work_ready_seen_keys`, and
    // keys outside it are untouched.
    let mut state = PolicyRuntimeState::default();
    state
        .work_ready_seen_keys
        .insert("p1::step-01::t1".into(), 1);
    state
        .work_ready_seen_keys
        .insert("p1::step-01::t2".into(), 1);
    state
        .work_ready_seen_keys
        .insert("p1::step-02::t1".into(), 1);

    state.prune_work_ready_bucket("p1", "step-01");

    // Pruned keys survive with their counts intact.
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
        Some(1)
    );
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-01::t2").copied(),
        Some(1)
    );
    // Different step preserved.
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-02::t1").copied(),
        Some(1)
    );
    // Bucket side-table records the prune.
    assert!(state.pruned_work_ready_buckets.contains("p1::step-01::t1"));
    assert!(state.pruned_work_ready_buckets.contains("p1::step-01::t2"));
    assert!(!state.pruned_work_ready_buckets.contains("p1::step-02::t1"));
}

// 2026-07-02-004 U7: precheck `<X>.proposed` dedup (R6).

// ─────────────────────────────────────────────────────────────────
// U5 of plan 2026-07-05-005 (R8): work_ready_seen_keys is now a
// HashMap<String, u32> so post-mortem tooling can distinguish a
// single duplicate from a "dup storm". Only the work.ready
// bucket is instrumented; the other 7 seen_keys fields stay as
// HashSet to keep the change blast radius small.
// ─────────────────────────────────────────────────────────────────

#[test]
fn p1_3_test_passed_different_fix_round_accepted() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();

    let p1 = test_result_payload("p1", "step-01", "t1", 0);
    let first = validate_event("test.passed", Some(&p1), &config, &mut state);
    assert_eq!(first, PolicyDecision::Accept);

    let p2 = test_result_payload("p1", "step-01", "t1", 1);
    let second = validate_event("test.passed", Some(&p2), &config, &mut state);
    assert_eq!(
        second,
        PolicyDecision::Accept,
        "test.passed with a different fix_round must be accepted"
    );
}

#[test]
fn p1_3_test_passed_missing_fix_round_skips_dedup() {
    // Mirrors U6 KTD4: missing `fix_round` falls through so
    // the schema validator reports `missing_required_field`.
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = r#"{"plan_name":"p1","step":"step-01","task_id":"t1","tests_run":10,"tests_passed":10}"#;

    let first = validate_event("test.passed", Some(payload), &config, &mut state);
    assert_eq!(
        first,
        PolicyDecision::Accept,
        "Missing fix_round must NOT be dedup-rejected"
    );
    assert!(
        state.test_passed_seen_keys.is_empty(),
        "Missing fix_round must NOT populate the dedup mirror"
    );
}

#[test]
fn step_handoff_u5_plan_blocked_null_payload_is_rejected() {
    let mut config = test_config_with_enforce_and_resume();
    config.mode = EventPolicyMode::Observe;
    let mut state = PolicyRuntimeState::default();
    let decision = validate_event("plan.blocked", None, &config, &mut state);
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "U5: null plan.blocked must RejectWithResume even in Observe, got {:?}",
        decision
    );
}

/// 2026-06-24 P0-D regression guard: a `review.complete` whose
/// `fix_plan_file` is a JSON `null` literal (instead of the
/// schema-required string `"null"`) must be rejected with a
/// `PayloadTypeMismatch` finding. The check must run regardless
/// of `EventPolicyMode` (defense-in-depth mirrors the U5
/// null-payload hard-reject list).
///
/// Background: the ralph-e2e python-sort-algorithms run shipped
/// `fix_plan_file: null` (JSON literal) for the fix-01 review
/// round, the runtime accepted it, and the downstream
/// coordinator's `fix_plan_file == "null"` string equality check
/// failed — leaving `plan.complete` un-emitted.

#[test]
fn step_handoff_u5_plan_complete_null_payload_is_rejected() {
    let mut config = test_config_with_enforce_and_resume();
    config.mode = EventPolicyMode::Observe;
    let mut state = PolicyRuntimeState::default();
    let decision = validate_event("plan.complete", None, &config, &mut state);
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "U5: null plan.complete must RejectWithResume even in Observe, got {:?}",
        decision
    );
}

/// Step-handoff U5: a null `plan.blocked` payload is
/// hard-rejected even in Observe mode.

#[test]
fn step_handoff_u5_whitelist_membership_pinned() {
    let expected = [
        "review.passed",
        "review.failed",
        "review.complete",
        "work.done",
        "queue.advance",
        "review.wave.ready",
        "work.ready",
        "plan.complete",
        "plan.blocked",
    ];
    assert_eq!(NULL_PAYLOAD_REJECT_TOPICS, expected);
    for topic in expected {
        assert!(
            is_null_payload_rejected_topic(topic),
            "is_null_payload_rejected_topic must accept `{topic}`"
        );
    }
    // A non-whitelist topic is unaffected.
    assert!(!is_null_payload_rejected_topic("human.guidance"));
}

/// Step-handoff U5: a null `work.ready` payload is
/// hard-rejected even in Observe mode. This is the per-topic
/// pin for `work.ready` after the list extension.

#[test]
fn step_handoff_u5_work_ready_null_payload_is_rejected() {
    let mut config = test_config_with_enforce_and_resume();
    config.mode = EventPolicyMode::Observe;
    let mut state = PolicyRuntimeState::default();
    let decision = validate_event("work.ready", None, &config, &mut state);
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "U5: null work.ready must RejectWithResume even in Observe, got {:?}",
        decision
    );
}

/// Step-handoff U5: a null `plan.complete` payload is
/// hard-rejected even in Observe mode.

#[test]
fn test_p0_2_system_control_topics_short_circuit_deny_rules() {
    let config = EventPolicyConfig {
        enabled: true,
        mode: EventPolicyMode::Enforce,
        on_violation: ViolationAction::Block,
        topic_deny_rules: vec![
            TopicDenyRule {
                hat_id: "validator".to_string(),
                topic: "task.resume".to_string(),
            },
            TopicDenyRule {
                hat_id: "executor".to_string(),
                topic: "task.resume".to_string(),
            },
            TopicDenyRule {
                hat_id: "ralph".to_string(),
                topic: "loop.cancel".to_string(),
            },
            TopicDenyRule {
                hat_id: "shipper".to_string(),
                topic: "build.task.abandoned".to_string(),
            },
        ],
        ..Default::default()
    };
    assert!(
        check_topic_deny_rules(Some("validator"), "task.resume", &config).is_none(),
        "P0-2: task.resume must be admitted for every hat — runner injection"
    );
    assert!(
        check_topic_deny_rules(Some("executor"), "task.resume", &config).is_none(),
        "P0-2: task.resume short-circuit is independent of originating hat"
    );
    assert!(
        check_topic_deny_rules(Some("ralph"), "loop.cancel", &config).is_none(),
        "P0-2: loop.cancel short-circuit must preempt the ralph deny rule"
    );
    assert!(
        check_topic_deny_rules(Some("shipper"), "build.task.abandoned", &config).is_none(),
        "P0-2: build.task.abandoned short-circuit must preempt the shipper deny rule"
    );
    // Sanity: the short-circuit is precisely scoped.
    // A business topic still matches its deny rule.
    let config_with_business_block = EventPolicyConfig {
        enabled: true,
        mode: EventPolicyMode::Enforce,
        on_violation: ViolationAction::Block,
        topic_deny_rules: vec![TopicDenyRule {
            hat_id: "executor".to_string(),
            topic: "build.done".to_string(),
        }],
        ..Default::default()
    };
    assert!(
        check_topic_deny_rules(Some("executor"), "build.done", &config_with_business_block)
            .is_some(),
        "deny rules still fire for business topics"
    );
}

#[test]
fn test_u4_duplicate_work_done_different_step_accepted() {
    // Edge case: same (plan, task_id) but different `step` key →
    // still accepted (key includes step).
    let config = test_config();
    let mut state = PolicyRuntimeState::default();

    // step-01 emit
    let p1 = work_done_payload("p1", "step-01", "t1");
    let first = validate_event("work.done", Some(&p1), &config, &mut state);
    assert_eq!(first, PolicyDecision::Accept);

    // Same task, different step: accepted
    let p2 = work_done_payload("p1", "step-02", "t1");
    let second = validate_event("work.done", Some(&p2), &config, &mut state);
    assert_eq!(
        second,
        PolicyDecision::Accept,
        "work.done for same task but different step must be accepted, got {:?}",
        second
    );
}

#[test]
fn test_u4_duplicate_work_done_different_task_accepted() {
    // Edge case: same (plan, step) but different `task_id` →
    // still accepted (key includes task_id).
    let config = test_config();
    let mut state = PolicyRuntimeState::default();

    let p1 = work_done_payload("p1", "step-01", "t1");
    let first = validate_event("work.done", Some(&p1), &config, &mut state);
    assert_eq!(first, PolicyDecision::Accept);

    let p2 = work_done_payload("p1", "step-01", "t2");
    let second = validate_event("work.done", Some(&p2), &config, &mut state);
    assert_eq!(
        second,
        PolicyDecision::Accept,
        "work.done for same step but different task must be accepted, got {:?}",
        second
    );
}

#[test]
fn test_u4_duplicate_work_done_disabled_policy_accepts_all() {
    // When event policy is disabled, the dedup check must be
    // skipped (mirrors all other policy checks).
    let mut config = test_config();
    config.enabled = false;
    let mut state = PolicyRuntimeState::default();
    let payload = work_done_payload("p1", "step-01", "t1");

    let first = validate_event("work.done", Some(&payload), &config, &mut state);
    let second = validate_event("work.done", Some(&payload), &config, &mut state);
    assert_eq!(first, PolicyDecision::Accept);
    assert_eq!(
        second,
        PolicyDecision::Accept,
        "disabled policy must not dedup, got {:?}",
        second
    );
}

#[test]
fn test_u4_duplicate_work_done_first_accepted() {
    // Happy path: first `work.done` for a (plan, step, task) tuple is accepted.
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = work_done_payload("p1", "step-01", "t1");
    let decision = validate_event("work.done", Some(&payload), &config, &mut state);
    assert_eq!(
        decision,
        PolicyDecision::Accept,
        "First work.done for a new (plan, step, task) tuple must be accepted"
    );
    // The dedup key should now be in the per-batch set
    assert!(state.work_done_seen_keys.contains("p1::step-01::t1"));
}

#[test]
fn test_u4_duplicate_work_done_hint_distinct() {
    // The two hints are distinct enum values so the runtime can
    // branch on them.
    assert_ne!(
        DuplicateWorkDoneHint::DuplicateSameStep,
        DuplicateWorkDoneHint::DuplicateStallBypass
    );
}

// -------------------------------------------------------------------------
// U5 (2026-06-17-003 plan, R6): `review.dimension.ready` dedup
//
// Mirrors the U4 work.done dedup pattern. Key is
// `(plan_name, step, task_id, dimension)`. A 2nd emit with
// the same key is rejected as `DuplicateWorkDone` (variant
// reused for retry-key parity).
// -------------------------------------------------------------------------

fn review_dimension_ready_payload(plan: &str, step: &str, task: &str, dim: &str) -> String {
    format!(
        r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","dimension":"{dim}","wave_id":"w1"}}"#
    )
}

fn review_start_payload(plan: &str, step: Option<&str>, task: &str) -> String {
    if let Some(st) = step {
        format!(
            r#"{{"plan_name":"{plan}","step":"{st}","task_id":"{task}","task_key":"k-{task}"}}"#
        )
    } else {
        format!(r#"{{"plan_name":"{plan}","task_id":"{task}","task_key":"k-{task}"}}"#)
    }
}

#[test]
fn test_u4_duplicate_work_done_hint_mapped_to_reason_code() {
    // U3 of plan 2026-07-05-005 (fix-plan §R3): restore the
    // single `duplicate_work_done` reason_code for both
    // `DuplicateSameStep` and `DuplicateStallBypass` per KTD-3.
    // The `hint` field on `RecoveryDiagnosisEnvelope` carries
    // the disambiguation so post-mortem tooling can still
    // distinguish the two paths.
    let same_step = PolicyFinding {
        topic: "work.done".to_string(),
        violation_type: ViolationType::DuplicateWorkDone {
            key: "p::s::t".to_string(),
            hint: DuplicateWorkDoneHint::DuplicateSameStep,
            seen_count: None,
        },
        message: "test".to_string(),
        evidence: None,
    };
    assert_eq!(
        same_step.violation_type.reason_code(),
        "duplicate_work_done",
        "U3: DuplicateSameStep must surface as duplicate_work_done (hint carries the discriminator)"
    );
    assert_eq!(
        DuplicateWorkDoneHint::DuplicateSameStep.as_hint_str(),
        "duplicate_work_done_same_step",
        "U3: DuplicateSameStep hint string stays stable for recovery envelope"
    );
    let stall = PolicyFinding {
        topic: "work.done".to_string(),
        violation_type: ViolationType::DuplicateWorkDone {
            key: "p::s::t".to_string(),
            hint: DuplicateWorkDoneHint::DuplicateStallBypass,
            seen_count: None,
        },
        message: "test".to_string(),
        evidence: None,
    };
    assert_eq!(
        stall.violation_type.reason_code(),
        "duplicate_work_done",
        "U3: DuplicateStallBypass must surface as duplicate_work_done (hint carries the discriminator)"
    );
    assert_eq!(
        DuplicateWorkDoneHint::DuplicateStallBypass.as_hint_str(),
        "duplicate_work_done_stall_bypass",
        "U3: DuplicateStallBypass hint string stays stable for recovery envelope"
    );
}

#[test]
fn test_u4_duplicate_work_done_is_recoverable() {
    // The DuplicateWorkDone violation must be in the recoverable
    // bucket (R-B1) so the runner publishes a `task.resume` with
    // `fix_hint` instead of the U6 fast-fail.
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = work_done_payload("p1", "step-01", "t1");
    let first = validate_event("work.done", Some(&payload), &config, &mut state);
    assert_eq!(first, PolicyDecision::Accept);

    let second = validate_event("work.done", Some(&payload), &config, &mut state);
    let finding = match second {
        PolicyDecision::RejectWithResume(f) => f,
        other => panic!("expected RejectWithResume, got {:?}", other),
    };
    let class = is_recoverable_policy_finding(&finding);
    assert_eq!(
        class,
        Some(ReasonClass::DuplicateWorkDone),
        "DuplicateWorkDone must map to the recoverable bucket, got {:?}",
        class
    );
}

#[test]
fn test_u4_duplicate_work_done_missing_fields_skips_dedup() {
    // If the payload is missing plan_name/step/task_id, the dedup
    // check cannot run — fall through to other policy layers.
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = r#"{"task_key":"k"}"#; // missing plan_name/step/task_id
    let first = validate_event("work.done", Some(payload), &config, &mut state);
    // First emit: not rejected by dedup (no key to compare).
    // May be rejected by other policies (e.g. required fields), but
    // the dedup violation type must NOT appear.
    if let PolicyDecision::RejectWithResume(f) = &first {
        assert!(
            !matches!(f.violation_type, ViolationType::DuplicateWorkDone { .. }),
            "missing-fields payload must not trigger DuplicateWorkDone, got {:?}",
            f.violation_type
        );
    }
}

#[test]
fn test_u4_duplicate_work_done_second_rejected() {
    // Error path: 2nd `work.done` with the same (plan, step, task)
    // tuple is rejected with `RejectWithResume` (RecoverableRejection
    // — the policy validator routes it through the recoverable bucket).
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = work_done_payload("p1", "step-01", "t1");

    // First emit: accepted
    let first = validate_event("work.done", Some(&payload), &config, &mut state);
    assert_eq!(first, PolicyDecision::Accept);

    // Second emit (same key, same batch): rejected
    let second = validate_event("work.done", Some(&payload), &config, &mut state);
    assert!(
        matches!(
            second,
            PolicyDecision::RejectWithResume(PolicyFinding {
                violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                evidence: None, .. }) if key == "p1::step-01::t1"
        ),
        "Second work.done for same key must be rejected with DuplicateWorkDone, got {:?}",
        second
    );
}

#[test]
fn test_u4_review_passed_skip_reason_allowlist_accepts_legal_values() {
    let config = review_passed_allowlist_config();
    // Each legal value is paired with a hat that allows it (per the
    // hat_allowed_values below). `trivial_step` is allowed by the
    // global allowlist only — no hat-specific entry — so the
    // hat-aware check skips and the value passes through the
    // global allowlist. With a hat, the check passes when the
    // value is either: (a) in the hat's per-hat list, or (b) not
    // restricted per-hat (i.e. the schema has no entry for that
    // hat, in which case only the global allowlist applies).
    let cases: &[(&str, &str)] = &[
        ("empty_diff", "review-coordinator"),
        ("aggregate_timeout", "review-synthesizer"),
        // trivial_step is in the global allowlist but not in any
        // hat-specific entry. The hat-aware block only fires when
        // the schema has a rule for the emitting hat; pick a hat
        // without a per-hat rule (the schema only has rules for
        // review-coordinator / review-synthesizer, so use any
        // other hat id to exercise the "no rule → skip" branch).
        ("trivial_step", "executor"),
    ];
    for (legal, hat_id) in cases {
        let payload = format!(
            r#"{{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"{legal}"}}"#
        );
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_hat(
            "review.passed",
            Some(&payload),
            &config,
            &mut state,
            Some(hat_id),
        );
        assert_eq!(
            decision,
            PolicyDecision::Accept,
            "skip_reason='{legal}' with hat='{hat_id}' should be accepted by the allowlist, got {:?}",
            decision
        );
    }
}

#[test]
fn test_u4_review_passed_skip_reason_allowlist_rejects_empty_string() {
    let config = review_passed_allowlist_config();
    let mut state = PolicyRuntimeState::default();
    let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":""}"#;
    let decision = validate_event("review.passed", Some(payload), &config, &mut state);
    assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
}

#[test]
fn test_u4_review_passed_skip_reason_allowlist_rejects_fabricated() {
    // The P1 root cause: review-synthesizer invented
    // `dimension_reviewer_no_response` as a skip_reason when the
    // aggregate timeout fired. Without the allowlist this passes
    // the required_fields gate. U4 closes that hole.
    let config = review_passed_allowlist_config();
    let mut state = PolicyRuntimeState::default();
    let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"dimension_reviewer_no_response"}"#;
    let decision = validate_event("review.passed", Some(payload), &config, &mut state);
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "fabricated skip_reason must be rejected, got {:?}",
        decision
    );
}

#[test]
fn test_u4_topic_deny_rules_executor_build_done_preserved() {
    // Regression: the original `executor → build.done` deny rule must
    // still fire after the U4 additions. Otherwise a worktree-loop
    // executor could impersonate the review-synthesizer again.
    let config = EventPolicyConfig {
        enabled: true,
        mode: EventPolicyMode::Enforce,
        on_violation: ViolationAction::Block,
        topic_deny_rules: vec![
            TopicDenyRule {
                hat_id: "executor".to_string(),
                topic: "build.done".to_string(),
            },
            TopicDenyRule {
                hat_id: "ralph".to_string(),
                topic: "review.passed".to_string(),
            },
        ],
        ..Default::default()
    };
    assert!(matches!(
        check_topic_deny_rules(Some("executor"), "build.done", &config),
        Some(PolicyDecision::Block(_))
    ));
    // And the new ralph rule still fires.
    assert!(matches!(
        check_topic_deny_rules(Some("ralph"), "review.passed", &config),
        Some(PolicyDecision::Block(_))
    ));
}

#[test]
fn test_u4_topic_deny_rules_ralph_blocked_from_workflow_topics() {
    // Mirrors the five new deny rules in ce-executor.yml:
    //   {hat_id: ralph, topic: review.wave.ready / review.passed /
    //    queue.advance / plan.complete / plan.blocked}
    let config = EventPolicyConfig {
        enabled: true,
        mode: EventPolicyMode::Enforce,
        on_violation: ViolationAction::Block,
        topic_deny_rules: vec![
            TopicDenyRule {
                hat_id: "ralph".to_string(),
                topic: "review.wave.ready".to_string(),
            },
            TopicDenyRule {
                hat_id: "ralph".to_string(),
                topic: "review.passed".to_string(),
            },
            TopicDenyRule {
                hat_id: "ralph".to_string(),
                topic: "queue.advance".to_string(),
            },
            TopicDenyRule {
                hat_id: "ralph".to_string(),
                topic: "plan.complete".to_string(),
            },
            TopicDenyRule {
                hat_id: "ralph".to_string(),
                topic: "plan.blocked".to_string(),
            },
        ],
        ..Default::default()
    };
    for topic in [
        "review.wave.ready",
        "review.passed",
        "queue.advance",
        "plan.complete",
        "plan.blocked",
    ] {
        let decision = check_topic_deny_rules(Some("ralph"), topic, &config);
        assert!(
            matches!(decision, Some(PolicyDecision::Block(_))),
            "ralph must be blocked from '{topic}', got {:?}",
            decision
        );
    }
}

#[test]
fn test_u4_topic_deny_rules_ralph_unchanged_for_control_topics() {
    // Control topics (e.g. task.resume, LOOP_COMPLETE) must NOT be
    // blocked for ralph — they are ralph's legitimate surface.
    // The ralph deny list only covers business topics.
    let config = EventPolicyConfig {
        enabled: true,
        mode: EventPolicyMode::Enforce,
        topic_deny_rules: vec![
            TopicDenyRule {
                hat_id: "ralph".to_string(),
                topic: "review.wave.ready".to_string(),
            },
            TopicDenyRule {
                hat_id: "ralph".to_string(),
                topic: "review.passed".to_string(),
            },
            TopicDenyRule {
                hat_id: "ralph".to_string(),
                topic: "queue.advance".to_string(),
            },
        ],
        ..Default::default()
    };
    assert!(check_topic_deny_rules(Some("ralph"), "task.resume", &config).is_none());
    assert!(check_topic_deny_rules(Some("ralph"), "LOOP_COMPLETE", &config).is_none());
    assert!(check_topic_deny_rules(Some("ralph"), "human.guidance", &config).is_none());
}

#[test]
fn test_u8_review_passed_hat_aware_allowed_values() {
    let config = review_passed_allowlist_config();
    let payload = r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"aggregate_timeout"}"#;

    // review-coordinator may only use skip_reason='empty_diff'.
    let mut state = PolicyRuntimeState::default();
    let decision = validate_event_with_hat(
        "review.passed",
        Some(payload),
        &config,
        &mut state,
        Some("review-coordinator"),
    );
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "review-coordinator emitting review.passed(skip_reason=aggregate_timeout) must be rejected, got {:?}",
        decision
    );

    // review-synthesizer may use skip_reason='aggregate_timeout'.
    let mut state = PolicyRuntimeState::default();
    let decision = validate_event_with_hat(
        "review.passed",
        Some(payload),
        &config,
        &mut state,
        Some("review-synthesizer"),
    );
    assert_eq!(
        decision,
        PolicyDecision::Accept,
        "review-synthesizer emitting review.passed(skip_reason=aggregate_timeout) must be accepted, got {:?}",
        decision
    );

    // U1 (2026-06-17-004 plan, R2): no hat provided + schema has
    // hat_allowed_values → fail-closed with a MissingRequiredField
    // finding. The CLI emit pipeline's `check_emit_provenance` gate
    // rejects hat-less business-topic emits earlier; this test pins
    // the programmatic-caller contract (validate_event / API
    // server path) so the old "skip hat-aware when None" behavior
    // cannot silently re-appear. The `validate_event` convenience
    // wraps `validate_event_with_hat(..., None)` so it inherits
    // the same fail-closed semantics.
    let mut state = PolicyRuntimeState::default();
    let decision =
        validate_event_with_hat("review.passed", Some(payload), &config, &mut state, None);
    match decision {
        PolicyDecision::RejectWithResume(finding) => {
            assert!(
                matches!(
                    finding.violation_type,
                    ViolationType::MissingRequiredField { .. }
                ),
                "no-hat + hat_allowed_values must yield MissingRequiredField, got {:?}",
                finding.violation_type
            );
            assert!(
                finding.message.contains("hat-specific allowed values"),
                "message must explain the provenance requirement, got: {}",
                finding.message
            );
        }
        other => panic!(
            "no-hat + hat_allowed_values must be rejected (fail-closed), got {:?}",
            other
        ),
    }
}

#[test]
fn u1_dedup_helper_prunes_allow_fix_round_rereview() {
    // End-to-end happy path: first ready accept, second
    // emit blocked, fix.applied prune, third emit (re-review)
    // accepted.
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
    assert!(matches!(
        second,
        PolicyDecision::RejectWithResume(PolicyFinding {
            violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
            evidence: None, .. }) if key == "p1::step-01::t1::correctness"
    ));

    // fix.applied accept path runs prune.
    state.prune_review_dimension_ready_bucket("p1", "step-01", "t1");

    let third = validate_event(
        "review.dimension.ready",
        Some(&payload),
        &config,
        &mut state,
    );
    assert_eq!(
        third,
        PolicyDecision::Accept,
        "after fix.applied prune the re-review ready must be accepted, got {:?}",
        third
    );
}

#[test]
fn u1_fix_applied_prune_helper_keeps_other_task_keys() {
    let mut state = PolicyRuntimeState::default();
    state
        .review_dimension_ready_seen_keys
        .insert("p1::step-01::t1::correctness".into());
    state
        .review_dimension_ready_seen_keys
        .insert("p1::step-01::t2::correctness".into());

    state.prune_review_dimension_ready_bucket("p1", "step-01", "t1");

    assert!(
        !state
            .review_dimension_ready_seen_keys
            .contains("p1::step-01::t1::correctness")
    );
    assert!(
        state
            .review_dimension_ready_seen_keys
            .contains("p1::step-01::t2::correctness")
    );
}

#[test]
fn u1_fix_applied_replay_does_not_prune_other_task_dimension_ready() {
    // Defensive: `fix.applied` payload's task_id bounds the
    // prune scope. A sibling task in the same (plan, step)
    // must keep its dedup key.
    use std::io::Write;

    let jsonl = r#"{"topic":"review.dimension.ready","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
{"topic":"review.dimension.ready","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t2\",\"dimension\":\"correctness\"}"}
{"topic":"fix.applied","hat":"fixer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":1,\"applied_count\":8,\"failed_count\":0,\"commit_count\":1,\"changed_lines\":96}"}
"#;
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(jsonl.as_bytes()).unwrap();
    tmp.flush().unwrap();

    let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
    assert!(
        !state
            .review_dimension_ready_seen_keys
            .contains("p1::step-01::t1::correctness"),
        "fix.applied on t1 must prune t1 bucket, got {:?}",
        state.review_dimension_ready_seen_keys
    );
    assert!(
        state
            .review_dimension_ready_seen_keys
            .contains("p1::step-01::t2::correctness"),
        "fix.applied on t1 must NOT prune t2 bucket, got {:?}",
        state.review_dimension_ready_seen_keys
    );
}

#[test]
fn u1_fix_applied_replay_populates_work_done_seen_keys_for_prior_work_done() {
    use std::io::Write;

    let jsonl = r#"{"topic":"work.done","hat":"executor","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"commit_count\":1}"}
"#;
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(jsonl.as_bytes()).unwrap();
    tmp.flush().unwrap();

    let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
    assert!(
        state.work_done_seen_keys.contains("p1::step-01::t1"),
        "from_events must mirror prior work.done into work_done_seen_keys, got {:?}",
        state.work_done_seen_keys
    );
}

#[test]
fn u1_fix_applied_replay_prunes_dimension_ready_keys() {
    use std::io::Write;

    let jsonl = r#"{"topic":"review.dimension.ready","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
{"topic":"review.dimension.done","hat":"dimension-reviewer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
{"topic":"fix.applied","hat":"fixer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":1,\"applied_count\":8,\"failed_count\":0,\"commit_count\":1,\"changed_lines\":96}"}
"#;
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(jsonl.as_bytes()).unwrap();
    tmp.flush().unwrap();

    let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
    assert!(
        !state
            .review_dimension_ready_seen_keys
            .contains("p1::step-01::t1::correctness"),
        "from_events replay of fix.applied must prune the bucket, got {:?}",
        state.review_dimension_ready_seen_keys
    );
}

#[test]
fn u1_fix_applied_replay_then_rereview_ready_accepted() {
    use std::io::Write;

    let jsonl = r#"{"topic":"review.dimension.ready","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"dimension\":\"correctness\"}"}
{"topic":"fix.applied","hat":"fixer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":1,\"applied_count\":8,\"failed_count\":0,\"commit_count\":1,\"changed_lines\":96}"}
"#;
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(jsonl.as_bytes()).unwrap();
    tmp.flush().unwrap();

    let mut state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
    let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");
    let decision = validate_event(
        "review.dimension.ready",
        Some(&payload),
        &test_config(),
        &mut state,
    );
    assert_eq!(
        decision,
        PolicyDecision::Accept,
        "Re-review ready after fix.applied must be accepted, got {:?}",
        decision
    );
}

#[test]
fn u1_prune_review_dimension_ready_bucket_clears_matching_prefix() {
    let mut state = PolicyRuntimeState::default();
    state
        .review_dimension_ready_seen_keys
        .insert("p1::step-01::t1::correctness".into());
    state
        .review_dimension_ready_seen_keys
        .insert("p1::step-01::t1::testing".into());
    state
        .review_dimension_ready_seen_keys
        .insert("p1::step-02::t1::correctness".into());

    state.prune_review_dimension_ready_bucket("p1", "step-01", "t1");

    assert!(
        !state
            .review_dimension_ready_seen_keys
            .contains("p1::step-01::t1::correctness"),
        "matching-prefix entry should be pruned, got {:?}",
        state.review_dimension_ready_seen_keys
    );
    assert!(
        !state
            .review_dimension_ready_seen_keys
            .contains("p1::step-01::t1::testing"),
        "matching-prefix entry should be pruned, got {:?}",
        state.review_dimension_ready_seen_keys
    );
    assert!(
        state
            .review_dimension_ready_seen_keys
            .contains("p1::step-02::t1::correctness"),
        "non-matching-prefix entry should remain, got {:?}",
        state.review_dimension_ready_seen_keys
    );
}

#[test]
fn u1_prune_review_dimension_ready_does_not_affect_other_steps() {
    // Defensive: prune is scoped to (plan, step, task). A
    // different step in the same plan must keep its key.
    let mut state = PolicyRuntimeState::default();
    state
        .review_dimension_ready_seen_keys
        .insert("p1::step-01::t1::correctness".into());
    state
        .review_dimension_ready_seen_keys
        .insert("p1::step-02::t1::correctness".into());

    state.prune_review_dimension_ready_bucket("p1", "step-01", "t1");

    assert!(
        !state
            .review_dimension_ready_seen_keys
            .contains("p1::step-01::t1::correctness")
    );
    assert!(
        state
            .review_dimension_ready_seen_keys
            .contains("p1::step-02::t1::correctness")
    );
}

#[test]
fn u1_prune_work_done_bucket_mirror_clears_matching_prefix() {
    let mut state = PolicyRuntimeState::default();
    state.work_done_seen_keys.insert("p1::step-01::t1".into());
    state.work_done_seen_keys.insert("p1::step-02::t1".into());
    state.work_done_seen_keys.insert("p2::step-01::t1".into());

    state.prune_work_done_bucket("p1", "step-01");

    assert!(!state.work_done_seen_keys.contains("p1::step-01::t1"));
    assert!(state.work_done_seen_keys.contains("p1::step-02::t1"));
    assert!(state.work_done_seen_keys.contains("p2::step-01::t1"));
}

#[test]
fn u1_semantic_gate_is_recoverable_implies_not_fatal() {
    let finding = PolicyFinding {
        topic: "review.passed".to_string(),
        violation_type: ViolationType::SemanticGateViolation {
            gate: "review_passed_while_wave_open".to_string(),
            context: "wave='w-1' received=0/3 expected".to_string(),
            referenced_fields: Vec::new(),
        },
        message: "review-coordinator must not emit review.passed while wave is incomplete"
            .to_string(),
        evidence: None,
    };
    // Recoverable → never feeds the U6 fast-fail
    // (`capture_violation` in `event_loop/mod.rs` early-returns
    // when this returns `Some`).
    assert!(is_recoverable_policy_finding(&finding).is_some());
    // And the bridge arms for `AllowedValueMismatch` /
    // `MissingRequiredField` / `PayloadTypeMismatch` only —
    // `SemanticGateViolation` falls through to `return None`.
    // We re-state the arms here so a future enum expansion
    // that accidentally adds a new fatal mapping is caught
    // by this test.
    assert!(matches!(
        finding.violation_type,
        ViolationType::SemanticGateViolation { .. }
    ));
}

// -------------------------------------------------------------------------
// 2026-06-24 P1-3: `work.ready` / `test.passed` / `test.failed` dedup
//
// Mirrors the U4 `work.done` and U5 `review.dimensions.complete`
// dedup patterns. `work.ready` key is `(plan, step, task_id)`;
// `test.passed` / `test.failed` key is
// `(plan, step, task_id, fix_round)`.
// -------------------------------------------------------------------------

fn work_ready_payload(plan: &str, step: &str, task: &str) -> String {
    format!(r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","task_key":"k"}}"#)
}

fn test_result_payload(plan: &str, step: &str, task: &str, fix_round: u64) -> String {
    format!(
        r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","fix_round":{fix_round},"tests_run":10,"tests_passed":10}}"#
    )
}

#[test]
fn u1_semantic_gate_violation_does_not_perturb_other_buckets() {
    assert_eq!(
        ReasonClass::PayloadTypeMismatch.as_str(),
        "payload_type_mismatch"
    );
    assert_eq!(
        ReasonClass::MissingRequiredField.as_str(),
        "missing_required_field"
    );
    assert_eq!(ReasonClass::TopicDenied.as_str(), "topic_denied");
    // And the non-recoverable ones stay non-recoverable.
    let finding = PolicyFinding {
        topic: "review.passed".to_string(),
        violation_type: ViolationType::TerminalMonotonicityViolation {
            terminal_topic: "plan.complete".to_string(),
            business_topic: "review.passed".to_string(),
        },
        message: "terminal monotonicity".to_string(),
        evidence: None,
    };
    assert!(is_recoverable_policy_finding(&finding).is_none());
}

// U1 (2026-06-17-003 plan): the `finding_to_payload_contract_violation`
// bridge (in `event_loop/mod.rs`) maps a `PolicyFinding` to a
// `PayloadContractViolation` only when the violation is
// schema-derived. `SemanticGateViolation` must NOT be in that
// set so the runner's `PayloadContractViolation` fatal branch
// never fires for `review_passed_while_wave_open`. We re-test
// here at the policy layer because the bridge is in `mod.rs`
// and not exposed for direct unit testing without spinning up
// an `EventLoop`. The downstream guarantee is:
//   is_recoverable_policy_finding == Some(SemanticGateViolation)
//   → bridge returns None → runner skips the fatal branch.

#[test]
fn u1_semantic_gate_violation_is_recoverable_with_own_bucket() {
    let finding = PolicyFinding {
        topic: "review.passed".to_string(),
        violation_type: ViolationType::SemanticGateViolation {
            gate: "review_passed_while_wave_open".to_string(),
            context: "wave='w-1' received=0/3 expected".to_string(),
            referenced_fields: Vec::new(),
        },
        message: "review-coordinator must not emit review.passed while wave is incomplete"
            .to_string(),
        evidence: None,
    };
    let class = is_recoverable_policy_finding(&finding)
        .expect("SemanticGateViolation must be in the recoverable set");
    assert_eq!(class, ReasonClass::SemanticGateViolation);
    assert_eq!(class.as_str(), "semantic_gate_violation");
    assert_eq!(
        finding.violation_type.reason_code(),
        "semantic_gate_violation"
    );
    // field() returns None — semantic-gate violations are
    // state-scoped, not field-scoped.
    assert!(finding.violation_type.field().is_none());
}

// U1 (2026-06-17-003 plan): the four existing recoverable
// buckets must keep their stable labels — adding
// `SemanticGateViolation` to the enum must not shift them.

#[test]
fn u4_no_prune_blocks_re_review_ready() {
    // 2026-06-18-006 plan U4 (R4): negative counterpart of
    // `u1_dedup_helper_prunes_allow_fix_round_rereview`.
    // Without the U1 prune (which is triggered when `fix.applied`
    // is accepted via `prune_review_dimension_ready_bucket`),
    // re-emitting a `review.dimension.ready` for the same
    // `(plan, step, task, dimension)` MUST be rejected as
    // `DuplicateWorkDone` — the dedup mirror still holds the
    // round-0 key. This pins that U1's prune is the load-bearing
    // step that lets the re-review round walk. The
    // `review_dimension_ready_dedup_*` cluster above already
    // covers the first/second emit round-trip on a fresh
    // state; this test isolates the specific post-accept
    // failure mode (round 1 emit blocked because round 0
    // still lingers).
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = review_dimension_ready_payload("p1", "step-01", "t1", "correctness");

    // Round 0: accept once so the dedup mirror learns the key.
    let first = validate_event(
        "review.dimension.ready",
        Some(&payload),
        &config,
        &mut state,
    );
    assert_eq!(
        first,
        PolicyDecision::Accept,
        "round 0 review.dimension.ready must be accepted, got {:?}",
        first
    );
    assert!(
        state
            .review_dimension_ready_seen_keys
            .contains("p1::step-01::t1::correctness"),
        "dedup mirror must hold round 0 key after accept, got {:?}",
        state.review_dimension_ready_seen_keys
    );

    // Intentionally DO NOT call
    // `state.prune_review_dimension_ready_bucket("p1", "step-01", "t1")`.
    // This simulates the bug scenario where the `fix.applied`
    // acceptance path (which normally prunes the bucket) is
    // missing — the in-batch mirror still holds the round-0 key.

    // Round 1 (re-review): re-emit the same ready. Without
    // the prune, this must be rejected as `DuplicateWorkDone`.
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
        "without U1 prune, re-review ready must be rejected as DuplicateWorkDone, got {:?}",
        second
    );

    // The dedup mirror must STILL hold the round-0 key after
    // the rejection — that's exactly the load the prune is
    // meant to lift. Pinning this prevents a future "helpful"
    // edit from clearing the mirror on rejection and silently
    // re-enabling duplicate work-done emits.
    assert!(
        state
            .review_dimension_ready_seen_keys
            .contains("p1::step-01::t1::correctness"),
        "dedup mirror must keep round-0 key when prune is skipped, got {:?}",
        state.review_dimension_ready_seen_keys
    );
}

// -------------------------------------------------------------------------
// U5 (2026-06-18-004 plan, R4, KTD3):
// `review.dimensions.complete` dedup keyed on
// `(plan_name, step, task_id, fix_round)`. A 2nd emit with
// the same key is rejected as `DuplicateWorkDone`. After
// `fix.applied` the bucket is pruned so the next round's
// `review.dimensions.complete` (with `fix_round=N+1`) can
// land without colliding with the prior round's entry.
//
// Mirrors the U5 `review.dimension.ready` test cluster
// above. Together they pin the re-review dedup contract
// end-to-end.
// -------------------------------------------------------------------------

fn review_dimensions_complete_payload(
    plan: &str,
    step: &str,
    task: &str,
    fix_round: u32,
) -> String {
    format!(
        r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","fix_round":{fix_round},"dimensions":[]}}"#
    )
}

#[test]
fn u5_fix_applied_replay_prunes_dimensions_complete_keys() {
    // KTD1 symmetry for the complete dedup: replay of
    // `fix.applied` MUST also clear the
    // `review.dimensions.complete` bucket so the next
    // round's complete (fix_round=N+1) does not collide
    // with the prior round's entry on rehydrate.
    use std::io::Write;

    let jsonl = r#"{"topic":"review.dimensions.complete","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":0,\"dimensions\":[]}"}
{"topic":"fix.applied","hat":"fixer","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":1,\"applied_count\":1,\"failed_count\":0,\"commit_count\":1,\"changed_lines\":5}"}
"#;
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(jsonl.as_bytes()).unwrap();
    tmp.flush().unwrap();

    let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
    assert!(
        !state
            .review_dimensions_complete_seen_keys
            .contains("p1::step-01::t1::0"),
        "from_events replay of fix.applied MUST prune the complete bucket, got {:?}",
        state.review_dimensions_complete_seen_keys
    );
}

// ── WAC-U7 (2026-06-12-002): payload hard gate ──

/// T-U7-01 / R10: null `review.passed` payload is hard-rejected
/// even when `EventPolicyMode::Observe` is configured.

#[test]
fn u5_other_seen_keys_still_hashset() {
    // Anti-regression: the other 7 seen_keys fields MUST
    // remain HashSet<String>; only work_ready_seen_keys was
    // widened to HashMap<String, u32>.
    //
    // Type-system guard: this test is a tautology in the
    // sense that converting any of these fields from
    // `HashSet<String>` to `HashMap<String, _>` would be a
    // compile error (the field types are pinned by the
    // struct definition). The assert is a sanity belt-and-
    // suspenders check; the real protection is the type
    // system. If you see this test "failing" because of a
    // future refactor, the right answer is to widen
    // work_ready_seen_keys's pattern to a sibling field
    // deliberately — not to weaken this assertion.
    use std::collections::HashSet;
    let mut state = PolicyRuntimeState::default();
    let work_done_keys: HashSet<String> = HashSet::new();
    state.work_done_seen_keys = work_done_keys;
    let dim_ready_keys: HashSet<String> = HashSet::new();
    state.review_dimension_ready_seen_keys = dim_ready_keys;
    let dim_complete_keys: HashSet<String> = HashSet::new();
    state.review_dimensions_complete_seen_keys = dim_complete_keys;
    let passed_keys: HashSet<String> = HashSet::new();
    state.test_passed_seen_keys = passed_keys;
    let failed_keys: HashSet<String> = HashSet::new();
    state.test_failed_seen_keys = failed_keys;
    let review_start_keys: HashSet<String> = HashSet::new();
    state.review_start_seen_keys = review_start_keys;
    assert!(state.work_ready_seen_keys.is_empty());
}

#[test]
fn u5_prune_helper_keeps_other_task_keys() {
    // Defensive: prune is scoped to (plan, step, task). A
    // sibling task in the same (plan, step) must keep its
    // dedup key.
    let mut state = PolicyRuntimeState::default();
    state
        .review_dimensions_complete_seen_keys
        .insert("p1::step-01::t1::0".into());
    state
        .review_dimensions_complete_seen_keys
        .insert("p1::step-01::t2::0".into());

    state.prune_review_dimensions_complete_bucket("p1", "step-01", "t1");

    assert!(
        !state
            .review_dimensions_complete_seen_keys
            .contains("p1::step-01::t1::0")
    );
    assert!(
        state
            .review_dimensions_complete_seen_keys
            .contains("p1::step-01::t2::0")
    );
}

#[test]
fn u5_review_dimensions_complete_dedup_different_fix_round_accepted() {
    // Edge case: 1st round (fix_round=0) accepted, 2nd
    // round (fix_round=1) accepted (after fix.applied
    // prune) — the fix_round segment keeps them distinct.
    let config = test_config();
    let mut state = PolicyRuntimeState::default();

    let first = review_dimensions_complete_payload("p1", "step-01", "t1", 0);
    let first_decision = validate_event(
        "review.dimensions.complete",
        Some(&first),
        &config,
        &mut state,
    );
    assert_eq!(first_decision, PolicyDecision::Accept);

    // Simulate fix.applied accept site (U1 path).
    state.prune_review_dimensions_complete_bucket("p1", "step-01", "t1");

    let second = review_dimensions_complete_payload("p1", "step-01", "t1", 1);
    let second_decision = validate_event(
        "review.dimensions.complete",
        Some(&second),
        &config,
        &mut state,
    );
    assert_eq!(
        second_decision,
        PolicyDecision::Accept,
        "fix_round=1 must be accepted after fix.applied prune, got {:?}",
        second_decision
    );
}

#[test]
fn u5_review_dimensions_complete_dedup_disabled_policy_accepts_all() {
    // When policy is disabled, dedup must NOT fire.
    let mut config = test_config();
    config.enabled = false;
    let mut state = PolicyRuntimeState::default();
    let payload = review_dimensions_complete_payload("p1", "step-01", "t1", 0);

    let first = validate_event(
        "review.dimensions.complete",
        Some(&payload),
        &config,
        &mut state,
    );
    let second = validate_event(
        "review.dimensions.complete",
        Some(&payload),
        &config,
        &mut state,
    );
    assert_eq!(first, PolicyDecision::Accept);
    assert_eq!(
        second,
        PolicyDecision::Accept,
        "disabled policy must NOT dedup review.dimensions.complete, got {:?}",
        second
    );
}

#[test]
fn u5_review_dimensions_complete_dedup_first_accepted() {
    // Happy path: first emit with `fix_round=0` is accepted.
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = review_dimensions_complete_payload("p1", "step-01", "t1", 0);
    let decision = validate_event(
        "review.dimensions.complete",
        Some(&payload),
        &config,
        &mut state,
    );
    assert_eq!(
        decision,
        PolicyDecision::Accept,
        "first review.dimensions.complete must be accepted, got {:?}",
        decision
    );
    assert!(
        state
            .review_dimensions_complete_seen_keys
            .contains("p1::step-01::t1::0"),
        "seen_keys must include the round-0 entry, got {:?}",
        state.review_dimensions_complete_seen_keys
    );
}

#[test]
fn u5_review_dimensions_complete_dedup_rejects_second_emit_same_round() {
    // Error path: 2nd emit with the same `fix_round` is
    // acknowledged + forwarded (U2 carve-out: silent-success
    // lane) instead of being rejected as `DuplicateWorkDone`.
    // The 4× duplicate `review.dimensions.complete` events
    // from the perky-maple P2-1 run are now silently accepted
    // by policy and forwarded to the bus; downstream code
    // observes the dedup hint via the carried `PolicyFinding`.
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = review_dimensions_complete_payload("p1", "step-01", "t1", 0);

    let first = validate_event(
        "review.dimensions.complete",
        Some(&payload),
        &config,
        &mut state,
    );
    assert_eq!(first, PolicyDecision::Accept);

    let second = validate_event(
        "review.dimensions.complete",
        Some(&payload),
        &config,
        &mut state,
    );
    assert!(
        matches!(
            second,
            PolicyDecision::AcknowledgeAndForward(PolicyFinding {
                violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                evidence: None, .. }) if key == "p1::step-01::t1::0"
        ),
        "2nd review.dimensions.complete same round must be AcknowledgeAndForward per U2, got {:?}",
        second
    );
}

#[test]
fn u5_review_dimensions_complete_replay_populates_seen_keys() {
    // KTD3 / KTD1 symmetry: `from_events` mirrors the
    // dedup set from prior `review.dimensions.complete`
    // events so loop rehydrate does not accept a
    // duplicate.
    use std::io::Write;

    let jsonl = r#"{"topic":"review.dimensions.complete","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":0,\"dimensions\":[]}"}
{"topic":"review.dimensions.complete","hat":"review-coordinator","payload":"{\"plan_name\":\"p1\",\"step\":\"step-01\",\"task_id\":\"t1\",\"fix_round\":1,\"dimensions\":[]}"}
"#;
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(jsonl.as_bytes()).unwrap();
    tmp.flush().unwrap();

    let state = PolicyRuntimeState::from_events(tmp.path(), &test_config()).unwrap();
    assert!(
        state
            .review_dimensions_complete_seen_keys
            .contains("p1::step-01::t1::0"),
        "from_events must mirror round-0 complete into the dedup set, got {:?}",
        state.review_dimensions_complete_seen_keys
    );
    assert!(
        state
            .review_dimensions_complete_seen_keys
            .contains("p1::step-01::t1::1"),
        "from_events must mirror round-1 complete into the dedup set, got {:?}",
        state.review_dimensions_complete_seen_keys
    );
}

#[test]
fn u5_work_ready_dedup_counter_first_hit_is_one() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = work_ready_payload("p1", "step-01", "t1");
    let decision = validate_event("work.ready", Some(&payload), &config, &mut state);
    assert_eq!(decision, PolicyDecision::Accept);
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
        Some(1),
        "U5: first work.ready hit must seed the counter at 1"
    );
}

#[test]
fn u5_work_ready_dedup_counter_increments_on_repeat() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = work_ready_payload("p1", "step-01", "t1");

    validate_event("work.ready", Some(&payload), &config, &mut state);
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
        Some(1)
    );

    let second = validate_event("work.ready", Some(&payload), &config, &mut state);
    assert!(matches!(second, PolicyDecision::RejectWithResume(_)));
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
        Some(2),
        "U5: counter must bump on every observed hit"
    );

    let third = validate_event("work.ready", Some(&payload), &config, &mut state);
    assert!(matches!(third, PolicyDecision::RejectWithResume(_)));
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
        Some(3)
    );
}

#[test]
fn u5_work_ready_prune_preserves_counter_on_pruned_key() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = r#"{"plan_name":"p1","step":"step-01","task_id":"t1"}"#;

    // First emit accepted → seed count=1.
    let first = validate_event("work.ready", Some(payload), &config, &mut state);
    assert!(matches!(first, PolicyDecision::Accept));
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
        Some(1)
    );

    // Second emit (without prune) → RejectWithResume, count=2.
    let second = validate_event("work.ready", Some(payload), &config, &mut state);
    assert!(matches!(second, PolicyDecision::RejectWithResume(_)));
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
        Some(2)
    );

    // Prune the bucket; dedup entry must survive in the
    // counter map; the bucket side-table records the prune.
    state.prune_work_ready_bucket("p1", "step-01");
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
        Some(2),
        "U5: count survives the prune"
    );
    assert!(state.pruned_work_ready_buckets.contains("p1::step-01::t1"));

    // Third emit (post-prune) → Accept, count increments to 3.
    let third = validate_event("work.ready", Some(payload), &config, &mut state);
    assert!(
        matches!(third, PolicyDecision::Accept),
        "U5: post-prune re-emit must accept, got {third:?}"
    );
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
        Some(3),
        "U5: count is incremented, not reset to 1"
    );
}

#[test]
fn u5_work_ready_prune_preserves_counter_on_remaining_keys() {
    let mut state = PolicyRuntimeState::default();
    state
        .work_ready_seen_keys
        .insert("p1::step-01::t1".into(), 7);
    state
        .work_ready_seen_keys
        .insert("p1::step-02::t2".into(), 3);

    state.prune_work_ready_bucket("p1", "step-01");

    // U5 of plan 2026-07-05-005 (fix-plan §R8): the dedup
    // hit counter is observation, not dedup state. The
    // pruned bucket's entry MUST survive (its count must
    // survive), only the bucket classification moves to the
    // side-table `pruned_work_ready_buckets`. Keys outside
    // the pruned bucket are untouched (counter preserved).
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-01::t1").copied(),
        Some(7),
        "U5: pruned key's counter is preserved across pruning"
    );
    assert!(state.pruned_work_ready_buckets.contains("p1::step-01::t1"));
    assert_eq!(
        state.work_ready_seen_keys.get("p1::step-02::t2").copied(),
        Some(3),
        "U5: counter is observation, not dedup state — pruning \
         other buckets does not reset surviving keys' counts"
    );
    assert!(!state.pruned_work_ready_buckets.contains("p1::step-02::t2"));
}

/// U5 of plan 2026-07-05-005 (fix-plan §R8): after a bucket
/// prune, a re-emit with the same `(plan_name, step, task_id)`
/// lands as Accept (the bucket classification is cleared), and
/// the existing counter is incremented — **not** reset to 1.

#[test]
fn u6_review_dimensions_complete_missing_fix_round_skips_dedup() {
    // U6 (2026-06-18-006 plan, R6, KTD4): missing `fix_round`
    // no longer silently defaults to `0`. The dedup layer
    // must skip recording the key so the schema validator
    // (downstream of `validate_event`) reports the precise
    // `missing_required_field` error to the agent, rather
    // than the dedup layer hiding the failure behind a
    // misleading `DuplicateWorkDone` rejection.
    //
    // This test replaces the prior U5 assertion that
    // expected missing `fix_round` to default to `0` and
    // dedup against the round-0 key. U6 reverses that
    // behavior now that `fix_round` is a required schema
    // field (2026-06-18-004 plan U0).
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    // Intentionally omit `fix_round` — schema invalid.
    let payload = r#"{"plan_name":"p1","step":"step-01","task_id":"t1","dimensions":[]}"#;

    let first = validate_event(
        "review.dimensions.complete",
        Some(payload),
        &config,
        &mut state,
    );
    // First emit also doesn't get a dedup key written —
    // dedup layer is silent on schema-invalid emits.
    // (Schema validation is downstream; we assert the dedup
    // layer's contract here: it does NOT insert a key.)
    assert_eq!(
        first,
        PolicyDecision::Accept,
        "missing fix_round must NOT be dedup-rejected by the policy layer (schema layer reports the real error), got {:?}",
        first
    );
    assert!(
        state.review_dimensions_complete_seen_keys.is_empty(),
        "missing fix_round must NOT populate the dedup mirror, got {:?}",
        state.review_dimensions_complete_seen_keys
    );

    let second = validate_event(
        "review.dimensions.complete",
        Some(payload),
        &config,
        &mut state,
    );
    // 2nd emit with the same invalid payload: still no
    // dedup, no `DuplicateWorkDone`. The dedup layer's
    // contract is to stay out of the way when the event is
    // schema-invalid.
    assert!(
        !matches!(second, PolicyDecision::RejectWithResume(_)),
        "missing fix_round must NOT trigger DuplicateWorkDone on a 2nd emit — schema layer owns the error, got {:?}",
        second
    );
    assert!(
        state.review_dimensions_complete_seen_keys.is_empty(),
        "seen_keys must still be empty after 2nd schema-invalid emit, got {:?}",
        state.review_dimensions_complete_seen_keys
    );
}

#[test]
fn u6_review_dimensions_complete_same_fix_round_still_dedups() {
    // U6 regression guard: the KTD4 change must NOT break
    // the round-0 dedup contract. Two emits both carrying
    // `fix_round=0` for the same `(plan, step, task)` are
    // still dedup-handled — U2 changes the *decision* from
    // `RejectWithResume` to `AcknowledgeAndForward` so the
    // silent-success run does not produce `task.resume`
    // storms, but the dedup invariant (mirror is populated,
    // second emit carries a `DuplicateWorkDone` finding) is
    // intact. Only schema-invalid emits are exempted.
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = review_dimensions_complete_payload("p1", "step-01", "t1", 0);

    let first = validate_event(
        "review.dimensions.complete",
        Some(&payload),
        &config,
        &mut state,
    );
    assert_eq!(first, PolicyDecision::Accept);
    assert!(
        state
            .review_dimensions_complete_seen_keys
            .contains("p1::step-01::t1::0"),
        "round-0 emit must populate the dedup mirror, got {:?}",
        state.review_dimensions_complete_seen_keys
    );

    let second = validate_event(
        "review.dimensions.complete",
        Some(&payload),
        &config,
        &mut state,
    );
    assert!(
        matches!(
            second,
            PolicyDecision::AcknowledgeAndForward(PolicyFinding {
                violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                evidence: None, .. }) if key == "p1::step-01::t1::0"
        ),
        "2nd round-0 emit must STILL be dedup-handled per U2 (AcknowledgeAndForward), got {:?}",
        second
    );
}

#[test]
fn u6_review_dimensions_complete_string_fix_round_skips_dedup() {
    // U6 (KTD4): non-numeric `fix_round` (e.g. string `"1"`)
    // is also treated as schema-invalid. The dedup layer
    // must not write a key for it, leaving the schema
    // validator free to report `type_mismatch`. This is the
    // same root-cause class as missing `fix_round` — a
    // schema-level error that must not be hidden behind
    // `DuplicateWorkDone`.
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload =
        r#"{"plan_name":"p1","step":"step-01","task_id":"t1","fix_round":"1","dimensions":[]}"#;

    let first = validate_event(
        "review.dimensions.complete",
        Some(payload),
        &config,
        &mut state,
    );
    assert_eq!(
        first,
        PolicyDecision::Accept,
        "string fix_round must NOT be dedup-rejected (schema layer reports type_mismatch), got {:?}",
        first
    );
    assert!(
        state.review_dimensions_complete_seen_keys.is_empty(),
        "string fix_round must NOT populate the dedup mirror, got {:?}",
        state.review_dimensions_complete_seen_keys
    );

    let second = validate_event(
        "review.dimensions.complete",
        Some(payload),
        &config,
        &mut state,
    );
    assert!(
        !matches!(second, PolicyDecision::RejectWithResume(_)),
        "string fix_round must NOT trigger DuplicateWorkDone on 2nd emit, got {:?}",
        second
    );
    assert!(
        state.review_dimensions_complete_seen_keys.is_empty(),
        "seen_keys must still be empty after 2nd string fix_round emit, got {:?}",
        state.review_dimensions_complete_seen_keys
    );
}

#[test]
fn u7_build_allowed_topics_includes_precheck_derived_topics() {
    use crate::config::RalphConfig;
    let yaml = r#"
event_loop:
  precheck:
    enabled: true
    rules:
      work.done:
        prompt: ["ok"]
        on_fail:
          target: executor
hats:
  executor:
    name: "Executor"
    triggers: ["task.start"]
    publishes: ["work.done"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.normalize();
    let allowed = build_allowed_topics(
        &config.hats,
        "LOOP_COMPLETE",
        config.event_loop.event_policy.as_ref(),
    );
    assert!(allowed.contains("work.done.proposed"));
    assert!(allowed.contains("work.done.rejected"));
    assert!(allowed.contains("work.done"));
}

// ─────────────────────────────────────────────────────────────────────
// U2 + U6 (plan 2026-07-04-004): `PolicyDecision::AcknowledgeAndForward`
// + the `ReviewDimensionsComplete` hint mapping. Together they
// carve out the silent-success lane so `review.dimensions.complete`
// re-emits do not trigger `task.resume` storms, while keeping the
// dedup invariant intact.
// ─────────────────────────────────────────────────────────────────────

/// `PolicyDecision` now exposes a 7th variant: `AcknowledgeAndForward`.
/// Pin the variant count + the new variant's existence so static
/// assertions across the workspace stay in sync (the project
/// sealed-style helper `ensure_sealed_enum()` no longer compiles
/// when a new variant is added without updating the call sites
/// listed in `find_referencing_symbols`).

#[test]
fn u7_precheck_proposed_cleared_on_rejected_allows_retry() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = r#"{"step":"s1"}"#;

    assert_eq!(
        validate_event("work.done.proposed", Some(payload), &config, &mut state),
        PolicyDecision::Accept
    );
    assert_eq!(
        validate_event(
            "work.done.rejected",
            Some(r#"{"failed_checks":[1],"reason":"no","synthetic":false}"#),
            &config,
            &mut state
        ),
        PolicyDecision::Accept
    );
    assert_eq!(
        validate_event("work.done.proposed", Some(payload), &config, &mut state),
        PolicyDecision::Accept,
        "after gate rejection the same candidate may be re-proposed"
    );
}

#[test]
fn u7_precheck_proposed_dedup_rejects_duplicate_candidate() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = r#"{"step":"s1"}"#;

    let first = validate_event("work.done.proposed", Some(payload), &config, &mut state);
    assert_eq!(first, PolicyDecision::Accept);

    let second = validate_event("work.done.proposed", Some(payload), &config, &mut state);
    assert!(
        matches!(
            second,
            PolicyDecision::RejectWithResume(PolicyFinding {
                violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                evidence: None, .. }) if key == "work.done::{\"step\":\"s1\"}"
        ),
        "duplicate work.done.proposed must be rejected, got {:?}",
        second
    );
}

#[test]
fn u7_precheck_proposed_remains_deduplicated_after_pass() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let payload = r#"{"step":"s1"}"#;

    assert_eq!(
        validate_event("work.done.proposed", Some(payload), &config, &mut state),
        PolicyDecision::Accept
    );
    assert_eq!(
        validate_event("work.done", Some(payload), &config, &mut state),
        PolicyDecision::Accept
    );

    let duplicate = validate_event("work.done.proposed", Some(payload), &config, &mut state);
    assert!(
        matches!(
            duplicate,
            PolicyDecision::RejectWithResume(PolicyFinding {
                violation_type: ViolationType::DuplicateWorkDone { ref key, .. },
                evidence: None, .. }) if key == "work.done::{\"step\":\"s1\"}"
        ),
        "same candidate must remain deduplicated after gate pass, got {:?}",
        duplicate
    );

    assert_eq!(
        validate_event(
            "work.done.proposed",
            Some(r#"{"step":"s2"}"#),
            &config,
            &mut state
        ),
        PolicyDecision::Accept,
        "a new payload remains a valid candidate after a prior pass"
    );
}

#[test]
fn u8_review_start_different_total_units_allowed() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let first = r#"{"plan_name":"p1","task_id":"t1","task_key":"k1","fix_round":0,"total_units":11,"triggered":"ralph"}"#;
    let second = r#"{"plan_name":"p1","task_id":"t1","task_key":"k1","fix_round":0,"total_units":7,"triggered":"ralph"}"#;
    assert_eq!(
        validate_event("review.start", Some(first), &config, &mut state),
        PolicyDecision::Accept
    );
    // Different total_units is a different semantic key, so
    // accepted (e.g. plan was re-planned mid-review).
    assert_eq!(
        validate_event("review.start", Some(second), &config, &mut state),
        PolicyDecision::Accept
    );
}

#[test]
fn u8_review_start_legacy_fallback_when_fix_round_missing() {
    // Pre-U8 emits that don't carry `fix_round` /
    // `total_units` must still use the legacy
    // `(plan_name, task_id [, step])` key — backward
    // compatibility for older recovery journals.
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    let first = r#"{"plan_name":"p1","task_id":"t1","task_key":"k1"}"#;
    let second =
        r#"{"plan_name":"p1","task_id":"t1","task_key":"k1","triggered":"review-coordinator"}"#;
    assert_eq!(
        validate_event("review.start", Some(first), &config, &mut state),
        PolicyDecision::Accept
    );
    // Without `fix_round` / `total_units`, the legacy key
    // `p1::t1` matches.
    assert!(
        matches!(
            validate_event("review.start", Some(second), &config, &mut state),
            PolicyDecision::RejectWithResume(_)
        ),
        "U8: legacy fallback (no fix_round / total_units) must still dedup"
    );
}

#[test]
fn u8_review_start_semantic_dedup_ignores_triggered_field() {
    let config = test_config();
    let mut state = PolicyRuntimeState::default();
    // 1st emit: triggered=ralph, fix_round=0, total_units=11.
    let first = r#"{"plan_name":"p1","task_id":"t1","task_key":"k1","fix_round":0,"total_units":11,"triggered":"ralph"}"#;
    assert_eq!(
        validate_event("review.start", Some(first), &config, &mut state),
        PolicyDecision::Accept
    );
    // 2nd emit: triggered=review-coordinator, identical
    // (plan_name, task_id, fix_round, total_units). 175407
    // root cause: this slipped through before U8. After U8,
    // the dedup key is `p1::fr=0::tu=11` and matches.
    let second = r#"{"plan_name":"p1","task_id":"t1","task_key":"k1","fix_round":0,"total_units":11,"triggered":"review-coordinator"}"#;
    assert!(
        matches!(
            validate_event("review.start", Some(second), &config, &mut state),
            PolicyDecision::RejectWithResume(_)
        ),
        "U8: 2nd review.start with identical fix_round+total_units must be rejected \
         regardless of `triggered`"
    );
}

#[test]
fn u_fixes_2026_07_04_task_id_task_key_mismatch_surfaces_invalid_field_value() {
    // Regression test: agent emits work.done with same
    // task_id as the prior accepted event but a different
    // task_key. Must be rejected as InvalidFieldValue
    // (`task_id_task_key_mismatch`), NOT DuplicateWorkDone,
    // so the resume hint is actionable.
    let mut state = PolicyRuntimeState::default();
    let config = test_config();
    let payload1 = serde_json::json!({
        "plan_name": "p1",
        "step": "step-01",
        "task_id": "t1",
        "task_key": "ce-executor:p1:step-01:u1-skeleton",
        "commit_count": 1,
        "changed_lines": 10,
    })
    .to_string();
    let payload2 = serde_json::json!({
        "plan_name": "p1",
        "step": "step-01",
        "task_id": "t1",
        "task_key": "ce-executor:p1:step-01:u0-impl",
        "commit_count": 1,
        "changed_lines": 10,
    })
    .to_string();
    let first = super::validate_event("work.done", Some(&payload1), &config, &mut state);
    assert!(matches!(first, super::PolicyDecision::Accept));
    let second = super::validate_event("work.done", Some(&payload2), &config, &mut state);
    match second {
        super::PolicyDecision::RejectWithResume(finding) => {
            assert!(
                matches!(
                    finding.violation_type,
                    super::ViolationType::InvalidFieldValue { .. }
                ),
                "expected InvalidFieldValue(task_id_task_key_mismatch), got {:?}",
                finding.violation_type
            );
            assert!(
                finding.message.contains("task_id_task_key_mismatch"),
                "message should name the failure mode, got: {}",
                finding.message
            );
        }
        other => panic!("expected RejectWithResume, got {other:?}"),
    }
}

#[test]
fn wac_r10_null_payload_on_whitelist_topic_is_rejected() {
    let mut config = test_config_with_enforce_and_resume();
    // Switch the policy into Observe mode to confirm the R10
    // gate is mode-agnostic.
    config.mode = EventPolicyMode::Observe;
    let mut state = PolicyRuntimeState::default();
    let decision = validate_event("review.passed", None, &config, &mut state);
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "R10 must RejectWithResume even in Observe mode, got {:?}",
        decision
    );
}

/// R10 also covers the other whitelist topics:
/// `work.done`, `queue.advance`, `review.wave.ready`, etc.
/// Step-handoff (2026-06-17-002) U5 extends the whitelist with
/// the handoff/terminal topics `work.ready`, `plan.complete`,
/// `plan.blocked` so the hard gate uniformly covers every
/// handoff/terminal topic in the ce-executor step chain.

#[test]
fn wac_r10_null_payload_rejects_every_whitelist_topic() {
    let config = test_config_with_enforce_and_resume();
    for topic in [
        "review.passed",
        "review.failed",
        "review.complete",
        "work.done",
        "work.ready",
        "queue.advance",
        "review.wave.ready",
        "plan.complete",
        "plan.blocked",
    ] {
        let mut s = PolicyRuntimeState::default();
        let decision = validate_event(topic, None, &config, &mut s);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "R10 must reject null payload on `{topic}`, got {:?}",
            decision
        );
    }
}

/// Step-handoff (2026-06-17-002) U5: `is_null_payload_rejected_topic` is the
/// single source of truth for the whitelist. Pin the exact
/// membership (original 6 + 3 U5 additions appended in place) so
/// future edits cannot silently drop a topic from the hard gate.

#[test]
fn wac_r10_overrides_observe_mode_for_null_whitelist_payload() {
    let mut config = test_config_with_enforce_and_resume();
    config.mode = EventPolicyMode::Observe;
    let mut state = PolicyRuntimeState::default();
    let decision = validate_event("work.done", None, &config, &mut state);
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "R10 must not downgraded by Observe mode, got {:?}",
        decision
    );
}

/// Helper: build a minimal `EventPolicyConfig` with Enforce
/// mode + RejectWithResume. Reused by the WAC tests above.
fn test_config_with_enforce_and_resume() -> EventPolicyConfig {
    let mut config = EventPolicyConfig::default();
    config.enabled = true;
    config.mode = EventPolicyMode::Enforce;
    config.on_violation = ViolationAction::RejectWithResume;
    config
}

// U1 (2026-06-17-003 plan): the new `SemanticGateViolation`
// variant must be in the recoverable set with its own bucket
// — and its reason_code must NOT collide with the schema-level
// `invalid_field_value` so diagnostics stay unambiguous.

#[test]
fn wac_r11_string_payload_normalizes_to_object() {
    let mut config = test_config_with_enforce_and_resume();
    config.schemas.insert(
        "review.wave.ready".to_string(),
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec!["dimension".to_string(), "plan_name".to_string()],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        },
    );
    let mut state = PolicyRuntimeState::default();
    // The payload is a JSON-string-of-an-object.
    let payload = r#"{"dimension":"code-quality","plan_name":"p1"}"#;
    let decision = validate_event("review.wave.ready", Some(payload), &config, &mut state);
    assert!(
        matches!(decision, PolicyDecision::Accept),
        "string-as-object must normalize and accept, got {:?}",
        decision
    );
}

/// T-U7-03 / R11: a string payload that is NOT a valid JSON
/// object is rejected (cannot be normalized).

#[test]
fn wac_r11_string_payload_not_json_is_rejected() {
    let mut config = test_config_with_enforce_and_resume();
    config.schemas.insert(
        "review.wave.ready".to_string(),
        EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec!["dimension".to_string()],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        },
    );
    let mut state = PolicyRuntimeState::default();
    let decision = validate_event("review.wave.ready", Some("not-a-json"), &config, &mut state);
    assert!(
        matches!(decision, PolicyDecision::RejectWithResume(_)),
        "non-JSON string must be rejected, got {:?}",
        decision
    );
}

/// T-U7-07: R10 hard-rejects null payloads even when the
/// rest of the policy is in `Observe` mode. The other
/// findings (terminal monotonicity, etc.) still fall through
/// to `Warn` per the existing behaviour, but R10 specifically
/// escalates to `RejectWithResume`.

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

// Shared fixtures restored with the event-policy regression tests.
fn review_passed_allowlist_config() -> EventPolicyConfig {
    let mut config = test_config();
    let mut schema = EventSchema {
        payload: Some(PayloadType::JsonObject),
        required_fields: vec![
            "plan_name".into(),
            "task_id".into(),
            "task_key".into(),
            "step".into(),
            "findings_count".into(),
            "fix_round".into(),
            "verdict".into(),
            "skip_reason".into(),
        ],
        allowed_values: HashMap::new(),
        hat_allowed_values: HashMap::new(),
        ..Default::default()
    };
    // Mirror the ce-executor.yml U4 allowlist exactly.
    schema.allowed_values.insert(
        "skip_reason".to_string(),
        vec![
            Value::String("empty_diff".to_string()),
            Value::String("trivial_step".to_string()),
            Value::String("aggregate_timeout".to_string()),
        ],
    );
    // U8: hat-aware restrictions mirror the preset.
    schema.hat_allowed_values.insert(
        "skip_reason".to_string(),
        vec![
            HatAllowedValues {
                hat_id: "review-coordinator".to_string(),
                values: vec![Value::String("empty_diff".to_string())],
            },
            HatAllowedValues {
                hat_id: "review-synthesizer".to_string(),
                values: vec![Value::String("aggregate_timeout".to_string())],
            },
        ],
    );
    config.schemas.insert("review.passed".to_string(), schema);
    config
}

fn work_done_payload(plan: &str, step: &str, task: &str) -> String {
    format!(r#"{{"plan_name":"{plan}","step":"{step}","task_id":"{task}","task_key":"k"}}"#)
}
