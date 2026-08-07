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
    fn test_matches_topic_rule_exact_and_glob() {
        assert!(matches_topic_rule("build.done", "build.done"));
        assert!(!matches_topic_rule("build.done", "build.done.rejected"));
        assert!(matches_topic_rule("debug.*", "debug.step"));
        assert!(matches_topic_rule("debug.*", "debug.done"));
        assert!(!matches_topic_rule("debug.*", "build.done"));
        assert!(matches_topic_rule("*", "anything.at.all"));
    }

    #[test]
    fn test_check_topic_deny_rules_uses_shared_glob_matcher() {
        let mut config = test_config();
        config.topic_deny_rules.push(TopicDenyRule {
            hat_id: "validator".to_string(),
            topic: "debug.*".to_string(),
        });
        let decision = check_topic_deny_rules(Some("validator"), "debug.step", &config);
        assert!(
            matches!(decision, Some(PolicyDecision::RejectWithResume(_))),
            "glob deny must surface RejectWithResume in Enforce mode; got {decision:?}"
        );
        let miss = check_topic_deny_rules(Some("validator"), "build.done", &config);
        assert!(miss.is_none(), "non-matching topic must not be denied");
    }

    #[test]
    fn test_accept_when_disabled() {
        let mut config = test_config();
        config.enabled = false;
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some("{}"), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_accept_valid_json_object() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some(r#"{"key": "value"}"#), &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_string_payload_when_json_object_required() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some("plain string"), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_missing_required_field() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec!["task_key".to_string()],

            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some(r#"{"other": "value"}"#), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_invalid_allowed_value() {
        let mut config = test_config();
        let mut schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        schema.allowed_values.insert(
            "decision".to_string(),
            vec![
                Value::String("keep".to_string()),
                Value::String("discard".to_string()),
            ],
        );
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event(
            "test",
            Some(r#"{"decision": "blocked"}"#),
            &config,
            &mut state,
        );
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_terminal_then_business_violation() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        // validate_event no longer mutates terminal_observed; caller applies it
        // after all validation layers have passed. We simulate that here.
        state.terminal_observed = true;
        let decision = validate_event("experiment.planned", Some("{}"), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_observe_mode_does_not_reject() {
        let mut config = test_config();
        config.mode = EventPolicyMode::Observe;
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some("plain string"), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::Warn(_)));
    }

    #[test]
    fn test_enforce_reject_with_resume() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],

            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", Some("plain string"), &config, &mut state);
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_nested_field_extraction() {
        let value = serde_json::json!({"evaluation": {"decision": "keep"}});
        let result = extract_json_field(&value, "evaluation.decision");
        assert_eq!(result, Some(Value::String("keep".to_string())));
    }

    #[test]
    fn test_extract_json_field_nonexistent_path() {
        let value = serde_json::json!({"a": {"b": 1}});
        assert_eq!(extract_json_field(&value, "a.c"), None);
        assert_eq!(extract_json_field(&value, "x.y"), None);
        assert_eq!(extract_json_field(&value, ""), None);
    }

    #[test]
    fn test_extract_json_field_intermediate_non_object() {
        let value = serde_json::json!({"a": [1, 2, 3]});
        assert_eq!(extract_json_field(&value, "a.b"), None);
        let value2 = serde_json::json!({"a": "string"});
        assert_eq!(extract_json_field(&value2, "a.b"), None);
    }

    #[test]
    fn test_required_fields_when_payload_missing() {
        let mut config = test_config();
        let schema = EventSchema {
            payload: None,
            required_fields: vec!["task_key".to_string()],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();
        let decision = validate_event("test", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::RejectWithResume(_)),
            "Missing payload with required fields should be rejected"
        );
    }

    #[test]
    fn test_nested_allowed_values_validation() {
        let mut config = test_config();
        let mut schema = EventSchema {
            payload: Some(PayloadType::JsonObject),
            required_fields: vec![],
            allowed_values: HashMap::new(),
            hat_allowed_values: HashMap::new(),
            ..Default::default()
        };
        schema.allowed_values.insert(
            "evaluation.decision".to_string(),
            vec![
                Value::String("keep".to_string()),
                Value::String("discard".to_string()),
            ],
        );
        config.schemas.insert("test".to_string(), schema);
        let mut state = PolicyRuntimeState::default();

        // Valid nested value
        let decision = validate_event(
            "test",
            Some(r#"{"evaluation": {"decision": "keep"}}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);

        // Invalid nested value
        let decision = validate_event(
            "test",
            Some(r#"{"evaluation": {"decision": "blocked"}}"#),
            &config,
            &mut state,
        );
        assert!(matches!(decision, PolicyDecision::RejectWithResume(_)));
    }

    #[test]
    fn test_duplicate_terminal_event_violation() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        // Caller sets terminal_observed after the first terminal event passes validation
        state.terminal_observed = true;
        let decision = validate_event("LOOP_COMPLETE", None, &config, &mut state);
        assert!(
            matches!(
                decision,
                PolicyDecision::RejectWithResume(PolicyFinding {
                    violation_type: ViolationType::DuplicateTerminalEvent { ref topic },
                    evidence: None, .. }) if topic == "LOOP_COMPLETE"
            ),
            "Expected DuplicateTerminalEvent violation, got {:?}",
            decision
        );
    }

    #[test]
    fn test_duplicate_terminal_accepted_when_disabled() {
        let mut config = test_config();
        config.enabled = false;
        let mut state = PolicyRuntimeState::default();
        state.terminal_observed = true;
        let decision = validate_event("LOOP_COMPLETE", None, &config, &mut state);
        assert_eq!(decision, PolicyDecision::Accept);
    }

    #[test]
    fn test_duplicate_terminal_observe_mode_warns() {
        let mut config = test_config();
        config.mode = EventPolicyMode::Observe;
        let mut state = PolicyRuntimeState::default();
        state.terminal_observed = true;
        let decision = validate_event("LOOP_COMPLETE", None, &config, &mut state);
        assert!(
            matches!(decision, PolicyDecision::Warn(ref findings) if findings.iter().any(|f| matches!(f.violation_type, ViolationType::DuplicateTerminalEvent { .. }))),
            "Expected Warn with DuplicateTerminalEvent, got {:?}",
            decision
        );
    }

    #[test]
    fn test_from_events_replays_terminal_and_business() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"experiment.planned","payload":"{{}}","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let config = test_config();
        let state = PolicyRuntimeState::from_events(file.path(), &config).unwrap();

        assert!(state.terminal_observed);
        assert!(state.observed_topics.contains("experiment.planned"));
        assert!(state.observed_topics.contains("LOOP_COMPLETE"));
    }

    #[test]
    fn test_from_events_payload_compatibility() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().unwrap();
        // String payload
        writeln!(
            file,
            r#"{{"topic":"task.start","payload":"Start work","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        // Object payload
        writeln!(
            file,
            r#"{{"topic":"task.done","payload":{{"result":"success"}},"ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        // Null payload
        writeln!(
            file,
            r#"{{"topic":"heartbeat","payload":null,"ts":"2024-01-01T00:00:02Z"}}"#
        )
        .unwrap();
        // Missing payload
        writeln!(file, r#"{{"topic":"noop","ts":"2024-01-01T00:00:03Z"}}"#).unwrap();
        file.flush().unwrap();

        let config = test_config();
        let state = PolicyRuntimeState::from_events(file.path(), &config).unwrap();

        assert_eq!(state.observed_topics.len(), 4);
        assert!(state.observed_topics.contains("task.start"));
        assert!(state.observed_topics.contains("task.done"));
        assert!(state.observed_topics.contains("heartbeat"));
        assert!(state.observed_topics.contains("noop"));
    }

    #[test]
    fn test_from_events_missing_file() {
        let config = test_config();
        let state = PolicyRuntimeState::from_events("/nonexistent/events.jsonl", &config).unwrap();
        assert!(!state.terminal_observed);
        assert!(state.observed_topics.is_empty());
    }

    #[test]
    fn test_from_events_skips_malformed_lines() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"experiment.planned","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(file, "this is not json").unwrap();
        writeln!(
            file,
            r#"{{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        file.flush().unwrap();

        let config = test_config();
        let state = PolicyRuntimeState::from_events(file.path(), &config).unwrap();

        assert!(state.terminal_observed);
        assert!(state.observed_topics.contains("experiment.planned"));
        assert!(state.observed_topics.contains("LOOP_COMPLETE"));
    }

    #[test]
    fn test_check_completion_honored_inactive_returns_none() {
        let config = test_config();
        let state = PolicyRuntimeState::default();
        assert_eq!(
            check_completion_honored("LOOP_COMPLETE", &config, &state),
            None
        );
        assert_eq!(
            check_completion_honored("experiment.planned", &config, &state),
            None
        );
    }

    #[test]
    fn test_check_completion_honored_warns_duplicate_terminal_by_default() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        let decision = check_completion_honored("LOOP_COMPLETE", &config, &state);
        assert!(
            matches!(decision, Some(PolicyDecision::Warn(_))),
            "Expected Warn for duplicate terminal by default, got {:?}",
            decision
        );
    }

    #[test]
    fn test_check_completion_honored_warns_business_after_completion_by_default() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        let decision = check_completion_honored("experiment.planned", &config, &state);
        assert!(
            matches!(decision, Some(PolicyDecision::Warn(_))),
            "Expected Warn for business after completion by default, got {:?}",
            decision
        );
    }

    #[test]
    fn test_check_completion_honored_allows_unrelated_events() {
        let config = test_config();
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        assert_eq!(
            check_completion_honored("task.resume", &config, &state),
            None
        );
    }

    #[test]
    fn test_check_completion_honored_ignore_action() {
        let mut config = test_config();
        config.completion_after_terminal.duplicate_terminal = CompletionAfterTerminalAction::Ignore;
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        let decision = check_completion_honored("LOOP_COMPLETE", &config, &state);
        assert!(
            matches!(decision, Some(PolicyDecision::Ignore(_))),
            "Expected Ignore, got {:?}",
            decision
        );
    }

    #[test]
    fn test_check_completion_honored_warn_action() {
        let mut config = test_config();
        config.completion_after_terminal.business_after_completion =
            CompletionAfterTerminalAction::Warn;
        let mut state = PolicyRuntimeState::default();
        state.completion_honored = true;
        let decision = check_completion_honored("experiment.planned", &config, &state);
        assert!(
            matches!(decision, Some(PolicyDecision::Warn(_))),
            "Expected Warn, got {:?}",
            decision
        );
    }

    #[test]
    fn test_check_completion_guard_respects_guard_active_flag() {
        let config = test_config();
        assert_eq!(
            check_completion_guard("LOOP_COMPLETE", &config, false),
            None
        );
        assert!(matches!(
            check_completion_guard("LOOP_COMPLETE", &config, true),
            Some(PolicyDecision::Warn(_))
        ));
    }

    #[test]
    fn test_fixture_valid_chain_accepted() {
        let (_, decision) = replay_and_validate(FIXTURE_VALID_CHAIN);
        assert!(
            is_accept(&decision),
            "Expected Accept for valid chain, got {:?}",
            decision
        );
    }

    #[test]
    fn test_fixture_duplicate_terminal_rejected_or_ignored() {
        let (_, decision) = replay_and_validate(FIXTURE_DUPLICATE_TERMINAL);
        assert!(
            !is_accept(&decision),
            "Expected reject/ignore for duplicate terminal, got {:?}",
            decision
        );
    }

    #[test]
    fn test_fixture_business_after_terminal_rejected_or_ignored() {
        let (_, decision) = replay_and_validate(FIXTURE_BUSINESS_AFTER_TERMINAL);
        assert!(
            !is_accept(&decision),
            "Expected reject/ignore for business after terminal, got {:?}",
            decision
        );
    }

    #[test]
    fn test_fixture_missing_required_fields_rejected_when_strict() {
        let config = fixture_config();
        let mut state =
            PolicyRuntimeState::from_events("/nonexistent/events.jsonl", &config).unwrap();
        let (topic, payload) = parse_fixture_line(FIXTURE_MISSING_REQUIRED_FIELDS);
        let decision = validate_event(&topic, payload.as_deref(), &config, &mut state);
        assert!(
            !is_accept(&decision),
            "Expected reject for missing provenance under strict config, got {:?}",
            decision
        );
    }

    #[test]
    fn test_provenance_fields_preserved_by_reader() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"experiment.planned","payload":{{"task_key":"x"}},"ts":"2024-01-01T00:00:00Z","hat":"strategist","triggered":"implementer","source":"cli"}}"#
        ).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();
        assert_eq!(result.events.len(), 1);
        let event = &result.events[0];
        assert_eq!(event.hat, Some("strategist".to_string()));
        assert_eq!(event.triggered, Some("implementer".to_string()));
        assert_eq!(event.source, Some("cli".to_string()));
    }

    #[test]
    fn test_old_simple_event_fixtures_still_parse() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"topic":"task.start","payload":"Start work","ts":"2024-01-01T00:00:00Z"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"topic":"task.done","payload":null,"ts":"2024-01-01T00:00:01Z"}}"#
        )
        .unwrap();
        writeln!(file, r#"{{"topic":"noop","ts":"2024-01-01T00:00:02Z"}}"#).unwrap();
        file.flush().unwrap();

        let mut reader = EventReader::new(file.path());
        let result = reader.read_new_events().unwrap();
        assert_eq!(result.events.len(), 3);
        assert_eq!(result.events[0].topic, "task.start");
        assert_eq!(result.events[0].payload, Some("Start work".to_string()));
        assert!(result.events[1].payload.is_none());
        assert!(result.events[2].payload.is_none());
    }

    #[test]
    fn test_check_topic_format_accepts_whitelisted_topic() {
        let mut allowed = HashSet::new();
        allowed.insert("work.done".to_string());
        allowed.insert("review.passed".to_string());
        assert_eq!(check_topic_format("work.done", &allowed), None);
        assert_eq!(check_topic_format("review.passed", &allowed), None);
    }

    #[test]
    fn test_check_topic_format_rejects_unknown_topic() {
        let mut allowed = HashSet::new();
        allowed.insert("work.done".to_string());
        let result = check_topic_format("REVIEW_COMPLETE", &allowed);
        assert!(result.is_some());
        let decision = result.unwrap();
        assert!(matches!(decision, PolicyDecision::Block(_)));
    }

    #[test]
    fn test_check_topic_format_rejects_uppercase_topic() {
        let mut allowed = HashSet::new();
        allowed.insert("work.done".to_string());
        // AE2: uppercase topic is rejected
        let result = check_topic_format("LOOP_COMPLETE", &allowed);
        assert!(result.is_some());
        let decision = result.unwrap();
        match decision {
            PolicyDecision::Block(finding) => {
                assert!(matches!(
                    finding.violation_type,
                    ViolationType::InvalidTopicFormat { .. }
                ));
            }
            _ => panic!("Expected Block decision"),
        }
    }

    #[test]
    fn test_check_topic_format_accepts_loop_complete_when_whitelisted() {
        // AE5: whitelisted completion token is accepted
        let mut allowed = HashSet::new();
        allowed.insert("LOOP_COMPLETE".to_string());
        assert_eq!(check_topic_format("LOOP_COMPLETE", &allowed), None);
    }

    #[test]
    fn test_is_system_topic_event_prefix() {
        assert!(is_system_topic("event.malformed"));
        assert!(is_system_topic("event.scope_violation"));
        assert!(is_system_topic("event.policy_warning"));
        assert!(!is_system_topic("work.done"));
        assert!(!is_system_topic("review.passed"));
    }

    #[test]
    fn test_is_system_topic_human_prefix() {
        assert!(is_system_topic("human.guidance"));
        assert!(!is_system_topic("humanx.guidance")); // no dot after prefix
    }

    #[test]
    fn test_build_allowed_topics_includes_hat_publishes() {
        let mut hats = std::collections::HashMap::new();
        let mut hat_config = crate::config::HatConfig::default();
        hat_config.publishes = vec!["work.done".to_string(), "review.passed".to_string()];
        hats.insert("executor".to_string(), hat_config);

        let allowed = build_allowed_topics(&hats, "LOOP_COMPLETE", None);
        assert!(allowed.contains("work.done"));
        assert!(allowed.contains("review.passed"));
        assert!(allowed.contains("LOOP_COMPLETE"));
        assert!(allowed.contains("loop.cancel"));
        assert!(allowed.contains("task.resume"));
        assert!(allowed.contains("build.task.abandoned"));
    }

    #[test]
    fn test_build_allowed_topics_empty_hats() {
        let hats = std::collections::HashMap::new();
        let allowed = build_allowed_topics(&hats, "LOOP_COMPLETE", None);
        // Only system topics
        assert!(allowed.contains("LOOP_COMPLETE"));
        assert!(allowed.contains("loop.cancel"));
        assert!(allowed.contains("task.resume"));
        assert!(allowed.contains("build.task.abandoned"));
        assert!(!allowed.contains("work.done"));
    }

    #[test]
    fn test_build_allowed_topics_includes_event_policy_topics() {
        let hats = std::collections::HashMap::new();
        let policy = EventPolicyConfig {
            terminal_topics: vec!["review.file".to_string()],
            business_topics: vec!["task.update".to_string()],
            ..Default::default()
        };
        let allowed = build_allowed_topics(&hats, "LOOP_COMPLETE", Some(&policy));
        assert!(allowed.contains("review.file"));
        assert!(allowed.contains("task.update"));
        assert!(allowed.contains("LOOP_COMPLETE"));
    }

    #[test]
    fn system_topic_short_circuit_runs_before_format_check() {
        // Empty whitelist — `check_topic_format` would reject ANY non-empty
        // topic that is not in the whitelist.
        let allowed = build_allowed_topics(&HashMap::new(), "LOOP_COMPLETE", None);

        // A topic that:
        //   - has uppercase letters → would normally fail format checks
        //   - is an `event.*` topic → admitted by `is_system_topic`
        //   - is NOT in the whitelist (and never will be, by U3 design)
        let rogue_system_topic = "event.foo.BAR";

        // Sanity: the system-topic short-circuit admits it.
        assert!(
            is_system_topic(rogue_system_topic),
            "test premise: '{rogue_system_topic}' must satisfy is_system_topic"
        );

        // Sanity: `check_topic_format` would reject it on its own — this
        // is the whole reason we need the short-circuit.
        assert!(
            check_topic_format(rogue_system_topic, &allowed).is_some(),
            "test premise: '{rogue_system_topic}' must be rejected by check_topic_format \
             when called in isolation, so that the short-circuit is load-bearing"
        );

        // Now compose the two checks in the documented order
        // (`is_system_topic` → `check_topic_format`). The composed
        // operation MUST accept the system topic even though
        // `check_topic_format` alone would reject it.
        let composed_admits = |topic: &str| -> bool {
            if is_system_topic(topic) {
                return true;
            }
            check_topic_format(topic, &allowed).is_none()
        };
        assert!(
            composed_admits(rogue_system_topic),
            "composed validation (is_system_topic → check_topic_format) must admit \
             '{rogue_system_topic}' — this is the order documented in build_allowed_topics"
        );

        // A non-system rogue topic (uppercase business topic) must STILL
        // be rejected by the composed operation — proving we did not
        // accidentally turn the short-circuit into a blanket bypass.
        let rogue_business_topic = "WORK.DONE.WITH_UPPERCASE";
        assert!(!is_system_topic(rogue_business_topic));
        assert!(
            !composed_admits(rogue_business_topic),
            "composed validation must still reject unknown business topics; \
             the short-circuit is for system topics only"
        );

        // And a well-formed business topic that's in the whitelist must
        // still be admitted — proving `check_topic_format` is still
        // doing its real job on the non-system side. Add "work.done"
        // to the whitelist to exercise the admit path explicitly.
        let mut allowed_with_work = allowed.clone();
        allowed_with_work.insert("work.done".to_string());
        let composed_admits_work = |topic: &str| -> bool {
            if is_system_topic(topic) {
                return true;
            }
            check_topic_format(topic, &allowed_with_work).is_none()
        };
        assert!(
            composed_admits_work("work.done"),
            "composed validation must admit whitelisted business topics"
        );
    }

    #[test]
    fn test_topic_deny_rules_match_rejected() {
        // Matching deny rule → Block when mode=Enforce
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "executor".to_string(),
                topic: "build.done".to_string(),
            }],
            ..Default::default()
        };
        let decision = check_topic_deny_rules(Some("executor"), "build.done", &config);
        assert!(matches!(decision, Some(PolicyDecision::Block(_))));
    }

    #[test]
    fn test_topic_deny_rules_non_matching_accepted() {
        // Non-matching hat_id → None (allowed)
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "executor".to_string(),
                topic: "build.done".to_string(),
            }],
            ..Default::default()
        };
        // Different hat, same topic → no match
        assert!(check_topic_deny_rules(Some("reviewer"), "build.done", &config).is_none());
        // Same hat, different topic → no match
        assert!(check_topic_deny_rules(Some("executor"), "work.done", &config).is_none());
        // No hat → no match (empty string not matched)
        assert!(check_topic_deny_rules(None, "build.done", &config).is_none());
    }

    #[test]
    fn test_topic_deny_rules_observe_mode_warns() {
        // Observe mode → Warn even when rule matches
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Observe,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "executor".to_string(),
                topic: "build.done".to_string(),
            }],
            ..Default::default()
        };
        let decision = check_topic_deny_rules(Some("executor"), "build.done", &config);
        assert!(matches!(decision, Some(PolicyDecision::Warn(_))));
    }

    #[test]
    fn test_topic_deny_rules_glob_pattern_matches() {
        // Glob pattern `debug.*` matches `debug.step`, `debug.done`, etc.
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::RejectWithResume,
            topic_deny_rules: vec![TopicDenyRule {
                hat_id: "coordinator".to_string(),
                topic: "debug.*".to_string(),
            }],
            ..Default::default()
        };
        // Segment wildcard matches
        assert!(matches!(
            check_topic_deny_rules(Some("coordinator"), "debug.step", &config),
            Some(PolicyDecision::RejectWithResume(_))
        ));
        assert!(matches!(
            check_topic_deny_rules(Some("coordinator"), "debug.done", &config),
            Some(PolicyDecision::RejectWithResume(_))
        ));
        // Non-matching topic not matched
        assert!(check_topic_deny_rules(Some("coordinator"), "debug", &config).is_none());
        // Non-matching hat not matched
        assert!(check_topic_deny_rules(Some("executor"), "debug.step", &config).is_none());
    }

    #[test]
    fn test_topic_deny_rules_glob_exact_overlap() {
        // When glob and exact rule both exist for same hat, first match wins.
        // Exact rule for `build.done` and glob rule for `debug.*` on coordinator.
        let config = EventPolicyConfig {
            enabled: true,
            mode: EventPolicyMode::Enforce,
            on_violation: ViolationAction::Block,
            topic_deny_rules: vec![
                TopicDenyRule {
                    hat_id: "coordinator".to_string(),
                    topic: "build.done".to_string(),
                },
                TopicDenyRule {
                    hat_id: "coordinator".to_string(),
                    topic: "debug.*".to_string(),
                },
            ],
            ..Default::default()
        };
        let decision = check_topic_deny_rules(Some("coordinator"), "build.done", &config);
        // Exact match found first (Block, not RejectWithResume from glob)
        assert!(matches!(decision, Some(PolicyDecision::Block(_))));
    }

    #[test]
    fn test_plan_name_equality_matches_accepted() {
        // work.ready with plan_name=A → work.done with plan_name=A → Accept
        let mut config = test_config();
        config.plan_name_equality_required = true;
        let mut state = PolicyRuntimeState::default();
        state.current_plan_name = Some("plan-x".to_string());

        let decision = validate_event(
            "work.done",
            Some(r#"{"plan_name": "plan-x"}"#),
            &config,
            &mut state,
        );
        assert_eq!(decision, PolicyDecision::Accept);
    }
}
