// Hooks tests live in this module. They exercise phase dispatch, lifecycle
// hooks, mutation namespaces, and the retry/backoff outcomes against the
// real loop-runner event loop. Shared helpers come from the sibling modules.

use super::super::*;
use super::common;
use super::common::*;

#[cfg(unix)]
fn hook_spec_with_command_and_on_error_and_suspend_mode(
    name: &str,
    command: Vec<String>,
    on_error: HookOnError,
    suspend_mode: Option<HookSuspendMode>,
) -> ralph_core::HookSpec {
    ralph_core::HookSpec {
        name: name.to_string(),
        command,
        cwd: None,
        env: std::collections::HashMap::new(),
        timeout_seconds: None,
        max_output_bytes: None,
        on_error: Some(on_error),
        suspend_mode,
        mutate: ralph_core::HookMutationConfig::default(),
        extra: std::collections::HashMap::new(),
    }
}

#[cfg(unix)]
fn hook_spec_with_command_and_on_error(
    name: &str,
    command: Vec<String>,
    on_error: HookOnError,
) -> ralph_core::HookSpec {
    hook_spec_with_command_and_on_error_and_suspend_mode(name, command, on_error, None)
}

#[cfg(unix)]
fn hook_spec_with_command(name: &str, command: Vec<String>) -> ralph_core::HookSpec {
    hook_spec_with_command_and_on_error(name, command, HookOnError::Warn)
}

#[cfg(unix)]
fn recording_hook(name: &str, log_path: &Path) -> ralph_core::HookSpec {
    hook_spec_with_command(
        name,
        vec![
            "sh".to_string(),
            "-c".to_string(),
            r#"payload="$(cat)"
phase="$(printf '%s' "$payload" | grep -o '"phase_event":"[^"]*"' | cut -d'"' -f4)"
printf '%s|%s\n' "$1" "$phase" >> "$2""#
                .to_string(),
            "hook-recorder".to_string(),
            name.to_string(),
            log_path.to_string_lossy().into_owned(),
        ],
    )
}

#[cfg(unix)]
fn payload_recording_hook(name: &str, log_path: &Path) -> ralph_core::HookSpec {
    hook_spec_with_command(
        name,
        vec![
            "sh".to_string(),
            "-c".to_string(),
            r#"payload="$(cat)"
printf '%s\n' "$payload" >> "$1""#
                .to_string(),
            "hook-payload-recorder".to_string(),
            log_path.to_string_lossy().into_owned(),
        ],
    )
}

#[cfg(unix)]
fn hook_engine_with_events(
    events: std::collections::HashMap<HookPhaseEvent, Vec<ralph_core::HookSpec>>,
) -> HookEngine {
    let hooks_config = ralph_core::HooksConfig {
        enabled: true,
        events,
        ..ralph_core::HooksConfig::default()
    };
    HookEngine::new(&hooks_config)
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_routes_by_phase_and_preserves_order() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let log_path = temp_dir.path().join("hook-dispatch.log");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreIterationStart,
        vec![
            recording_hook("pre-iteration-first", &log_path),
            recording_hook("pre-iteration-second", &log_path),
        ],
    );
    events.insert(
        HookPhaseEvent::PostLoopStart,
        vec![recording_hook("post-loop-only", &log_path)],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("ralph".to_string()),
            Some("builder".to_string()),
            Some("task-123".to_string()),
        ),
    );

    assert_eq!(
        read_hook_log(&log_path),
        vec![
            "pre-iteration-first|pre.iteration.start".to_string(),
            "pre-iteration-second|pre.iteration.start".to_string(),
        ]
    );

    dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert_eq!(
        read_hook_log(&log_path),
        vec![
            "pre-iteration-first|pre.iteration.start".to_string(),
            "pre-iteration-second|pre.iteration.start".to_string(),
            "post-loop-only|post.loop.start".to_string(),
        ]
    );
}

#[cfg(unix)]
#[test]
fn test_ac13_mutation_disabled_json_output_is_inert_for_accumulator_and_downstream_payloads() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let payload_log_path = temp_dir
        .path()
        .join("hook-metadata-disabled-payloads.jsonl");

    let mut disabled_mutation_spec = hook_spec_with_command(
        "metadata-emitter",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' '{\"metadata\":{\"risk_score\":0.72}}'".to_string(),
        ],
    );
    disabled_mutation_spec.mutate = hook_mutation_config(false);

    let mut events = std::collections::HashMap::new();
    events.insert(HookPhaseEvent::PreLoopStart, vec![disabled_mutation_spec]);
    events.insert(
        HookPhaseEvent::PostLoopStart,
        vec![payload_recording_hook(
            "payload-recorder",
            &payload_log_path,
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let mut accumulated_hook_metadata = serde_json::Map::new();
    accumulated_hook_metadata.insert("upstream".to_string(), serde_json::json!("preserved"));

    let pre_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        super::super::payload_inputs::build_loop_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            0,
            Some("planner".to_string()),
            &accumulated_hook_metadata,
        ),
    );

    assert_eq!(pre_outcomes.len(), 1);
    assert_eq!(pre_outcomes[0].disposition, HookDisposition::Pass);
    assert_eq!(pre_outcomes[0].failure, None);
    assert_eq!(
        pre_outcomes[0].mutation_parse_outcome,
        HookMutationParseOutcome::Disabled
    );

    let metadata_before_merge = accumulated_hook_metadata.clone();
    merge_accumulated_hook_metadata_from_outcomes(&mut accumulated_hook_metadata, &pre_outcomes);
    assert_eq!(accumulated_hook_metadata, metadata_before_merge);

    let post_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostLoopStart,
        super::super::payload_inputs::build_loop_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            0,
            Some("planner".to_string()),
            &accumulated_hook_metadata,
        ),
    );
    merge_accumulated_hook_metadata_from_outcomes(&mut accumulated_hook_metadata, &post_outcomes);

    let payloads = read_hook_payload_log(&payload_log_path);
    assert_eq!(payloads.len(), 1);
    assert_eq!(
        payloads[0]["metadata"]["accumulated"],
        serde_json::json!({"upstream":"preserved"})
    );

    let payload_accumulated = payloads[0]["metadata"]["accumulated"]
        .as_object()
        .expect("metadata.accumulated object");
    assert!(!payload_accumulated.contains_key("hook_metadata"));
}

#[cfg(unix)]
#[test]
fn test_ac14_mutation_enabled_updates_only_namespaced_metadata_in_downstream_payloads() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let payload_log_path = temp_dir.path().join("hook-metadata-enabled-payloads.jsonl");

    let mut mutation_spec = hook_spec_with_command(
        "metadata-emitter",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' '{\"metadata\":{\"risk_score\":0.72,\"gates\":[\"policy_check\"]}}'"
                .to_string(),
        ],
    );
    mutation_spec.mutate = hook_mutation_config(true);

    let mut events = std::collections::HashMap::new();
    events.insert(HookPhaseEvent::PreLoopStart, vec![mutation_spec]);
    events.insert(
        HookPhaseEvent::PostLoopStart,
        vec![payload_recording_hook(
            "payload-recorder",
            &payload_log_path,
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let mut accumulated_hook_metadata = serde_json::Map::new();
    accumulated_hook_metadata.insert("upstream".to_string(), serde_json::json!("preserved"));

    let pre_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        super::super::payload_inputs::build_loop_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            0,
            Some("planner".to_string()),
            &accumulated_hook_metadata,
        ),
    );
    assert!(matches!(
        pre_outcomes[0].mutation_parse_outcome,
        HookMutationParseOutcome::Parsed { .. }
    ));

    merge_accumulated_hook_metadata_from_outcomes(&mut accumulated_hook_metadata, &pre_outcomes);
    assert_eq!(
        serde_json::Value::Object(accumulated_hook_metadata.clone()),
        serde_json::json!({
            "upstream": "preserved",
            "hook_metadata": {
                "metadata-emitter": {
                    "risk_score": 0.72,
                    "gates": ["policy_check"]
                }
            }
        })
    );

    let post_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostLoopStart,
        super::super::payload_inputs::build_loop_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            0,
            Some("planner".to_string()),
            &accumulated_hook_metadata,
        ),
    );
    merge_accumulated_hook_metadata_from_outcomes(&mut accumulated_hook_metadata, &post_outcomes);

    let payloads = read_hook_payload_log(&payload_log_path);
    assert_eq!(payloads.len(), 1);
    let payload = &payloads[0];

    assert_eq!(payload["phase_event"], serde_json::json!("post.loop.start"));
    assert_eq!(
        payload["context"]["active_hat"],
        serde_json::json!("planner")
    );
    assert_eq!(
        payload["metadata"]["accumulated"],
        serde_json::json!({
            "upstream": "preserved",
            "hook_metadata": {
                "metadata-emitter": {
                    "risk_score": 0.72,
                    "gates": ["policy_check"]
                }
            }
        })
    );

    let payload_object = payload.as_object().expect("payload object");
    assert!(!payload_object.contains_key("prompt"));
    assert!(!payload_object.contains_key("events"));
    assert!(!payload_object.contains_key("config"));

    let context = payload["context"]
        .as_object()
        .expect("payload context object");
    assert!(!context.contains_key("prompt"));
    assert!(!context.contains_key("events"));
    assert!(!context.contains_key("config"));

    let payload_accumulated = payload["metadata"]["accumulated"]
        .as_object()
        .expect("metadata.accumulated object");
    assert!(!payload_accumulated.contains_key("risk_score"));
    assert!(!payload_accumulated.contains_key("gates"));
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_noop_when_disabled_or_unconfigured() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let log_path = temp_dir.path().join("hook-noop.log");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreIterationStart,
        vec![recording_hook("should-not-run", &log_path)],
    );

    let hook_engine = hook_engine_with_events(events);
    let empty_engine = hook_engine_with_events(std::collections::HashMap::new());
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let disabled_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        false,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("ralph".to_string()),
            Some("builder".to_string()),
            Some("task-123".to_string()),
        ),
    );

    let empty_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &empty_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("ralph".to_string()),
            Some("builder".to_string()),
            Some("task-123".to_string()),
        ),
    );

    let mismatched_phase_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert!(
        disabled_outcomes.is_empty(),
        "disabled hooks must be a no-op"
    );
    assert!(
        empty_outcomes.is_empty(),
        "empty hooks config must be a no-op"
    );
    assert!(
        mismatched_phase_outcomes.is_empty(),
        "dispatching a phase without hooks must be a no-op"
    );
    assert!(
        !log_path.exists(),
        "hook log should not be created on no-op paths"
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_returns_dispositions_and_failure_context() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreLoopStart,
        vec![
            hook_spec_with_command(
                "hook-pass",
                vec!["sh".to_string(), "-c".to_string(), "exit 0".to_string()],
            ),
            hook_spec_with_command(
                "hook-warn",
                vec!["sh".to_string(), "-c".to_string(), "exit 7".to_string()],
            ),
            hook_spec_with_command_and_on_error(
                "hook-block",
                vec!["sh".to_string(), "-c".to_string(), "exit 23".to_string()],
                HookOnError::Block,
            ),
        ],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert_eq!(outcomes.len(), 3);

    assert_eq!(outcomes[0].hook_name, "hook-pass");
    assert_eq!(outcomes[0].phase_event, HookPhaseEvent::PreLoopStart);
    assert_eq!(outcomes[0].disposition, HookDisposition::Pass);
    assert!(outcomes[0].failure.is_none());

    assert_eq!(outcomes[1].hook_name, "hook-warn");
    assert_eq!(outcomes[1].disposition, HookDisposition::Warn);
    assert_eq!(
        outcomes[1].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(7),
            timed_out: false,
        })
    );

    assert_eq!(outcomes[2].hook_name, "hook-block");
    assert_eq!(outcomes[2].disposition, HookDisposition::Block);
    assert_eq!(
        outcomes[2].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(23),
            timed_out: false,
        })
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_maps_executor_failures_to_on_error_disposition() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreLoopStart,
        vec![
            hook_spec_with_command(
                "warn-exec-error",
                vec!["definitely-not-a-real-exec-warn".to_string()],
            ),
            hook_spec_with_command_and_on_error(
                "block-exec-error",
                vec!["definitely-not-a-real-exec-block".to_string()],
                HookOnError::Block,
            ),
        ],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[0].hook_name, "warn-exec-error");
    assert_eq!(outcomes[0].disposition, HookDisposition::Warn);
    match &outcomes[0].failure {
        Some(HookDispatchFailure::HookExecutionError { message }) => {
            assert!(
                message.contains("definitely-not-a-real-exec-warn"),
                "executor failure context should include missing command"
            );
        }
        other => panic!("expected execution error failure context, got {other:?}"),
    }

    assert_eq!(outcomes[1].hook_name, "block-exec-error");
    assert_eq!(outcomes[1].disposition, HookDisposition::Block);
    match &outcomes[1].failure {
        Some(HookDispatchFailure::HookExecutionError { message }) => {
            assert!(
                message.contains("definitely-not-a-real-exec-block"),
                "executor failure context should include missing command"
            );
        }
        other => panic!("expected execution error failure context, got {other:?}"),
    }
}

// AC-15: JSON-only mutation format errors must flow through lifecycle on_error dispositions.
#[cfg(unix)]
#[test]
fn test_ac15_dispatch_phase_event_hooks_non_json_mutation_warn_continues_through_block_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut warn_hook = hook_spec_with_command_and_on_error(
        "warn-invalid-mutation",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' 'oops'".to_string(),
        ],
        HookOnError::Warn,
    );
    warn_hook.mutate = hook_mutation_config(true);

    let mut events = std::collections::HashMap::new();
    events.insert(HookPhaseEvent::PreLoopStart, vec![warn_hook]);

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Warn);
    assert!(matches!(
        outcomes[0].mutation_parse_outcome,
        HookMutationParseOutcome::Invalid(_)
    ));
    assert!(matches!(
        &outcomes[0].failure,
        Some(HookDispatchFailure::InvalidMutationOutput { message })
        if message.contains("not valid JSON")
    ));
    assert!(fail_if_blocking_loop_start_outcomes(&outcomes).is_ok());
}

#[cfg(unix)]
#[test]
fn test_ac15_dispatch_phase_event_hooks_non_json_mutation_block_surfaces_invalid_output_reason() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut block_hook = hook_spec_with_command_and_on_error(
        "block-invalid-mutation",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' 'oops'".to_string(),
        ],
        HookOnError::Block,
    );
    block_hook.mutate = hook_mutation_config(true);

    let mut events = std::collections::HashMap::new();
    events.insert(HookPhaseEvent::PreLoopStart, vec![block_hook]);

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Block);
    assert!(matches!(
        &outcomes[0].failure,
        Some(HookDispatchFailure::InvalidMutationOutput { message })
        if message.contains("not valid JSON")
    ));

    let block_error = fail_if_blocking_loop_start_outcomes(&outcomes)
        .expect_err("block disposition should fail loop.start boundary");
    let block_message = block_error.to_string();
    assert!(block_message.contains("block-invalid-mutation"));
    assert!(block_message.contains("pre.loop.start"));
    assert!(block_message.contains("invalid mutation output"));
    assert!(block_message.contains("not valid JSON"));
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_runtime_failure_takes_precedence_over_mutation_parse_error() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut block_hook = hook_spec_with_command_and_on_error(
        "block-runtime-failure",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' 'oops'; exit 23".to_string(),
        ],
        HookOnError::Block,
    );
    block_hook.mutate = hook_mutation_config(true);

    let mut events = std::collections::HashMap::new();
    events.insert(HookPhaseEvent::PreLoopStart, vec![block_hook]);

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Block);
    assert_eq!(
        outcomes[0].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(23),
            timed_out: false,
        })
    );
    assert!(matches!(
        outcomes[0].mutation_parse_outcome,
        HookMutationParseOutcome::Invalid(_)
    ));

    let block_error = fail_if_blocking_loop_start_outcomes(&outcomes)
        .expect_err("block disposition should fail loop.start boundary");
    let block_message = block_error.to_string();
    assert!(block_message.contains("hook exited with code 23"));
    assert!(!block_message.contains("invalid mutation output"));
}

#[cfg(unix)]
#[test]
fn test_ac15_dispatch_phase_event_hooks_non_json_mutation_suspend_uses_wait_for_resume_gate() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut suspend_hook = hook_spec_with_command_and_on_error(
        "suspend-invalid-mutation",
        vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '%s' 'oops'".to_string(),
        ],
        HookOnError::Suspend,
    );
    suspend_hook.mutate = hook_mutation_config(true);

    let mut events = std::collections::HashMap::new();
    events.insert(HookPhaseEvent::PreIterationStart, vec![suspend_hook]);

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            None,
            None,
        ),
    );

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Suspend);
    assert!(matches!(
        &outcomes[0].failure,
        Some(HookDispatchFailure::InvalidMutationOutput { message })
        if message.contains("not valid JSON")
    ));
    assert!(fail_if_blocking_iteration_start_outcomes(&outcomes).is_ok());

    let resume_store = suspend_state_store.clone();
    let resume_handle = std::thread::spawn(move || {
        let wait_started_at = std::time::Instant::now();
        while !resume_store.suspend_state_path().exists() {
            assert!(
                wait_started_at.elapsed() < Duration::from_secs(2),
                "suspend-state should be written before resume"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let suspend_state = resume_store
            .read_suspend_state()
            .expect("read suspend-state")
            .expect("suspend-state should exist while waiting");
        assert!(suspend_state.reason.contains("invalid mutation output"));
        assert!(suspend_state.reason.contains("not valid JSON"));

        resume_store
            .write_resume_requested()
            .expect("write resume signal");
    });

    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    resume_handle
        .join()
        .expect("resume helper thread should not panic");

    assert_eq!(wait_result, None);
    assert!(
        suspend_state_store
            .read_suspend_state()
            .expect("read suspend-state after resume")
            .is_none(),
        "suspend-state should be cleared after resume"
    );
    assert!(
        !suspend_state_store.resume_requested_path().exists(),
        "resume-requested should be consumed after resume"
    );
}

#[cfg(unix)]
#[test]
fn test_loop_start_dispatch_warn_continues_and_block_aborts() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreLoopStart,
        vec![hook_spec_with_command_and_on_error(
            "warn-pre-loop-start",
            vec!["sh".to_string(), "-c".to_string(), "exit 17".to_string()],
            HookOnError::Warn,
        )],
    );
    events.insert(
        HookPhaseEvent::PostLoopStart,
        vec![hook_spec_with_command_and_on_error(
            "block-post-loop-start",
            vec!["sh".to_string(), "-c".to_string(), "exit 29".to_string()],
            HookOnError::Block,
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let pre_loop_start_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 0, None),
    );

    assert_eq!(pre_loop_start_outcomes.len(), 1);
    assert_eq!(
        pre_loop_start_outcomes[0].disposition,
        HookDisposition::Warn
    );
    assert_eq!(
        pre_loop_start_outcomes[0].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(17),
            timed_out: false,
        })
    );
    assert!(
        fail_if_blocking_loop_start_outcomes(&pre_loop_start_outcomes).is_ok(),
        "warn disposition should continue across loop.start boundary"
    );

    let post_loop_start_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 0, Some("planner".to_string())),
    );

    assert_eq!(post_loop_start_outcomes.len(), 1);
    assert_eq!(
        post_loop_start_outcomes[0].disposition,
        HookDisposition::Block
    );
    let post_loop_start_error = fail_if_blocking_loop_start_outcomes(&post_loop_start_outcomes)
        .expect_err("block disposition should abort loop.start boundary");
    let post_loop_start_message = post_loop_start_error.to_string();
    assert!(post_loop_start_message.contains("block-post-loop-start"));
    assert!(post_loop_start_message.contains("post.loop.start"));
    assert!(post_loop_start_message.contains("hook exited with code 29"));
}

#[cfg(unix)]
#[test]
fn test_iteration_start_dispatch_warn_continues_and_block_aborts() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreIterationStart,
        vec![hook_spec_with_command_and_on_error(
            "warn-pre-iteration-start",
            vec!["sh".to_string(), "-c".to_string(), "exit 19".to_string()],
            HookOnError::Warn,
        )],
    );
    events.insert(
        HookPhaseEvent::PostIterationStart,
        vec![hook_spec_with_command_and_on_error(
            "block-post-iteration-start",
            vec!["sh".to_string(), "-c".to_string(), "exit 31".to_string()],
            HookOnError::Block,
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let pre_iteration_start_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            None,
            None,
        ),
    );

    assert_eq!(pre_iteration_start_outcomes.len(), 1);
    assert_eq!(
        pre_iteration_start_outcomes[0].disposition,
        HookDisposition::Warn
    );
    assert_eq!(
        pre_iteration_start_outcomes[0].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(19),
            timed_out: false,
        })
    );
    assert!(
        fail_if_blocking_iteration_start_outcomes(&pre_iteration_start_outcomes).is_ok(),
        "warn disposition should continue across iteration.start boundary"
    );

    let post_iteration_start_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            Some("builder".to_string()),
            Some("task-123".to_string()),
        ),
    );

    assert_eq!(post_iteration_start_outcomes.len(), 1);
    assert_eq!(
        post_iteration_start_outcomes[0].disposition,
        HookDisposition::Block
    );
    let post_iteration_start_error =
        fail_if_blocking_iteration_start_outcomes(&post_iteration_start_outcomes)
            .expect_err("block disposition should abort iteration.start boundary");
    let post_iteration_start_message = post_iteration_start_error.to_string();
    assert!(post_iteration_start_message.contains("block-post-iteration-start"));
    assert!(post_iteration_start_message.contains("post.iteration.start"));
    assert!(post_iteration_start_message.contains("hook exited with code 31"));
}

#[cfg(unix)]
#[test]
fn test_plan_created_lifecycle_hooks_dispatch_only_for_semantic_plan_batches() {
    use std::io::Write;

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (mut event_loop, loop_ctx) = dispatch_test_event_loop_with_context(temp_dir.path());
    let events_path = loop_ctx.events_path();
    std::fs::create_dir_all(events_path.parent().expect("events path parent"))
        .expect("create events directory");

    let mut events_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .expect("open events file");
    writeln!(
        events_file,
        r#"{{"topic":"task.start","payload":"noop","ts":"2024-01-01T00:00:00Z"}}"#
    )
    .expect("write non-plan event");
    events_file.flush().expect("flush non-plan event");

    let log_path = temp_dir.path().join("plan-created-hook-payloads.jsonl");
    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PrePlanCreated,
        vec![payload_recording_hook("pre-plan-created", &log_path)],
    );
    events.insert(
        HookPhaseEvent::PostPlanCreated,
        vec![payload_recording_hook("post-plan-created", &log_path)],
    );
    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();

    assert!(
        !event_loop
            .has_pending_plan_events_in_jsonl()
            .expect("peek non-plan events"),
        "non-plan batches must not trigger pre.plan.created"
    );

    let processed_non_plan = event_loop
        .process_events_from_jsonl()
        .expect("process non-plan batch");
    assert!(processed_non_plan.had_events);
    assert!(
        !processed_non_plan.had_plan_events,
        "non-plan batches must not trigger post.plan.created"
    );
    assert!(
        !log_path.exists(),
        "plan.created hooks should not run for non-plan batches"
    );

    writeln!(
        events_file,
        r#"{{"topic":"plan.created","payload":"ready","ts":"2024-01-01T00:00:01Z"}}"#
    )
    .expect("write plan event");
    events_file.flush().expect("flush plan event");

    assert!(
        event_loop
            .has_pending_plan_events_in_jsonl()
            .expect("peek plan events"),
        "plan.* batches should trigger pre.plan.created"
    );

    let pre_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PrePlanCreated,
        build_plan_created_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            event_loop.state().iteration,
            Some("planner".to_string()),
            Some("planner".to_string()),
            None,
        ),
    );
    assert!(fail_if_blocking_plan_created_outcomes(&pre_outcomes).is_ok());

    let processed_plan = event_loop
        .process_events_from_jsonl()
        .expect("process plan batch");
    assert!(processed_plan.had_events);
    assert!(
        processed_plan.had_plan_events,
        "plan.* batches should trigger post.plan.created"
    );

    let post_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostPlanCreated,
        build_plan_created_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            event_loop.state().iteration,
            Some("planner".to_string()),
            Some("planner".to_string()),
            None,
        ),
    );
    assert!(fail_if_blocking_plan_created_outcomes(&post_outcomes).is_ok());

    let payloads = read_hook_payload_log(&log_path);
    let observed_phases: Vec<&str> = payloads
        .iter()
        .map(|payload| {
            payload["phase_event"]
                .as_str()
                .expect("phase_event should be present")
        })
        .collect();

    assert_eq!(
        observed_phases,
        vec!["pre.plan.created", "post.plan.created"],
        "plan.created hooks should dispatch exactly once around semantic plan batches"
    );
}

#[cfg(unix)]
#[test]
fn test_loop_termination_lifecycle_hooks_dispatch_complete_and_error_boundaries() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let log_path = temp_dir.path().join("loop-termination-hook-payloads.jsonl");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreLoopComplete,
        vec![payload_recording_hook("pre-loop-complete", &log_path)],
    );
    events.insert(
        HookPhaseEvent::PostLoopComplete,
        vec![payload_recording_hook("post-loop-complete", &log_path)],
    );
    events.insert(
        HookPhaseEvent::PreLoopError,
        vec![payload_recording_hook("pre-loop-error", &log_path)],
    );
    events.insert(
        HookPhaseEvent::PostLoopError,
        vec![payload_recording_hook("post-loop-error", &log_path)],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let completed_reason = block_on_test_future(common::dispatch_pre_loop_termination_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        &suspend_state_store,
        &loop_ctx,
        5,
        TerminationReason::CompletionPromise,
    ))
    .expect("pre.loop.complete dispatch should succeed");
    let completed_reason = block_on_test_future(common::dispatch_post_loop_termination_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        &suspend_state_store,
        &loop_ctx,
        5,
        completed_reason,
    ))
    .expect("post.loop.complete dispatch should succeed");
    assert_eq!(completed_reason, TerminationReason::CompletionPromise);

    let error_reason = block_on_test_future(common::dispatch_pre_loop_termination_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        &suspend_state_store,
        &loop_ctx,
        5,
        TerminationReason::MaxRuntime,
    ))
    .expect("pre.loop.error dispatch should succeed");
    let error_reason = block_on_test_future(common::dispatch_post_loop_termination_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        &suspend_state_store,
        &loop_ctx,
        5,
        error_reason,
    ))
    .expect("post.loop.error dispatch should succeed");
    assert_eq!(error_reason, TerminationReason::MaxRuntime);

    let payloads = read_hook_payload_log(&log_path);
    let phases: Vec<&str> = payloads
        .iter()
        .map(|payload| {
            payload["phase_event"]
                .as_str()
                .expect("phase_event should be present")
        })
        .collect();
    let reasons: Vec<&str> = payloads
        .iter()
        .map(|payload| {
            payload["context"]["termination_reason"]
                .as_str()
                .expect("termination_reason should be present")
        })
        .collect();

    assert_eq!(
        phases,
        vec![
            "pre.loop.complete",
            "post.loop.complete",
            "pre.loop.error",
            "post.loop.error"
        ]
    );
    assert_eq!(
        reasons,
        vec!["completed", "completed", "max_runtime", "max_runtime"]
    );
}

#[cfg(unix)]
#[test]
fn test_iteration_start_suspend_waits_for_resume_and_clears_artifacts_before_continuing() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreIterationStart,
        vec![hook_spec_with_command_and_on_error(
            "suspend-pre-iteration-start",
            vec!["sh".to_string(), "-c".to_string(), "exit 41".to_string()],
            HookOnError::Suspend,
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let pre_iteration_start_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            None,
            None,
        ),
    );

    assert_eq!(pre_iteration_start_outcomes.len(), 1);
    assert_eq!(
        pre_iteration_start_outcomes[0].disposition,
        HookDisposition::Suspend
    );
    assert_eq!(
        pre_iteration_start_outcomes[0].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(41),
            timed_out: false,
        })
    );
    assert!(
        fail_if_blocking_iteration_start_outcomes(&pre_iteration_start_outcomes).is_ok(),
        "suspend disposition should not block iteration.start boundary"
    );

    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let wait_result = block_on_test_future(async {
        let wait_outcomes = pre_iteration_start_outcomes.clone();
        let wait_store = suspend_state_store.clone();
        let wait_handle = tokio::spawn(async move {
            wait_for_resume_if_suspended(&wait_outcomes, "loop-test", &wait_store).await
        });

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if suspend_state_store.suspend_state_path().exists() {
                    break;
                }

                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("suspend-state should be written before resume");

        let suspend_state = suspend_state_store
            .read_suspend_state()
            .expect("read suspend-state")
            .expect("suspend-state should exist while waiting for resume");

        assert_eq!(suspend_state.loop_id, "loop-test");
        assert_eq!(suspend_state.phase_event, HookPhaseEvent::PreIterationStart);
        assert_eq!(suspend_state.hook_name, "suspend-pre-iteration-start");
        assert_eq!(suspend_state.suspend_mode, HookSuspendMode::WaitForResume);
        assert!(!suspend_state_store.resume_requested_path().exists());

        suspend_state_store
            .write_resume_requested()
            .expect("write resume signal");

        tokio::time::timeout(Duration::from_secs(2), wait_handle)
            .await
            .expect("wait_for_resume helper should complete after resume signal")
            .expect("wait_for_resume task should not panic")
    })
    .expect("wait helper should succeed");

    assert_eq!(wait_result, None);
    assert!(
        suspend_state_store
            .read_suspend_state()
            .expect("read suspend-state after resume")
            .is_none(),
        "suspend-state should be cleared after resume"
    );
    assert!(
        !suspend_state_store.resume_requested_path().exists(),
        "resume-requested should be consumed after resume"
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_retry_backoff_recovers_before_exhaustion() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let attempts_path = temp_dir.path().join("retry-backoff-attempts.txt");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreIterationStart,
        vec![hook_spec_with_command_and_on_error_and_suspend_mode(
            "retry-backoff-pre-iteration-start",
            vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"attempts_file="$1"
attempt=0
if [ -f "$attempts_file" ]; then
  attempt="$(cat "$attempts_file")"
fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$attempts_file"
if [ "$attempt" -lt 3 ]; then
  exit 41
fi
exit 0"#
                    .to_string(),
                "retry-backoff-hook".to_string(),
                attempts_path.to_string_lossy().into_owned(),
            ],
            HookOnError::Suspend,
            Some(HookSuspendMode::RetryBackoff),
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop_with_diagnostics(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            None,
            None,
        ),
    );

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Pass);
    assert_eq!(outcomes[0].suspend_mode, HookSuspendMode::RetryBackoff);
    assert_eq!(outcomes[0].failure, None);

    let attempts = std::fs::read_to_string(&attempts_path).expect("read attempts");
    assert_eq!(attempts.trim(), "3", "hook should recover on third attempt");

    let telemetry_entries = read_hook_run_telemetry_entries(temp_dir.path());
    assert_eq!(telemetry_entries.len(), 3);
    assert_eq!(
        telemetry_entries
            .iter()
            .map(|entry| entry.retry_attempt)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(
        telemetry_entries
            .iter()
            .all(|entry| entry.retry_max_attempts == 4)
    );
    assert!(
        telemetry_entries
            .iter()
            .all(|entry| entry.suspend_mode == HookSuspendMode::RetryBackoff)
    );
    assert_eq!(
        telemetry_entries
            .iter()
            .map(|entry| entry.disposition)
            .collect::<Vec<_>>(),
        vec![
            HookDisposition::Suspend,
            HookDisposition::Suspend,
            HookDisposition::Pass,
        ]
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_retry_backoff_exhausts_to_suspend() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let attempts_path = temp_dir.path().join("retry-backoff-attempts.txt");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PostIterationStart,
        vec![hook_spec_with_command_and_on_error_and_suspend_mode(
            "retry-backoff-post-iteration-start",
            vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"attempts_file="$1"
attempt=0
if [ -f "$attempts_file" ]; then
  attempt="$(cat "$attempts_file")"
fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$attempts_file"
exit 51"#
                    .to_string(),
                "retry-backoff-hook".to_string(),
                attempts_path.to_string_lossy().into_owned(),
            ],
            HookOnError::Suspend,
            Some(HookSuspendMode::RetryBackoff),
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            Some("builder".to_string()),
            Some("task-123".to_string()),
        ),
    );

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Suspend);
    assert_eq!(outcomes[0].suspend_mode, HookSuspendMode::RetryBackoff);
    assert_eq!(
        outcomes[0].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(51),
            timed_out: false,
        })
    );

    let attempts: usize = std::fs::read_to_string(&attempts_path)
        .expect("read attempts")
        .trim()
        .parse()
        .expect("parse attempts");
    assert_eq!(
        attempts,
        RETRY_BACKOFF_DELAYS_MS.len() + 1,
        "retry_backoff should cap retries at the configured schedule"
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_retry_backoff_yields_to_stop_signal() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let attempts_path = temp_dir.path().join("retry-backoff-attempts.txt");
    std::fs::create_dir_all(temp_dir.path().join(".ralph")).expect("create .ralph");
    std::fs::write(temp_dir.path().join(".ralph/stop-requested"), "").expect("write stop signal");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreLoopStart,
        vec![hook_spec_with_command_and_on_error_and_suspend_mode(
            "retry-backoff-pre-loop-start",
            vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"attempts_file="$1"
attempt=0
if [ -f "$attempts_file" ]; then
  attempt="$(cat "$attempts_file")"
fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$attempts_file"
exit 61"#
                    .to_string(),
                "retry-backoff-hook".to_string(),
                attempts_path.to_string_lossy().into_owned(),
            ],
            HookOnError::Suspend,
            Some(HookSuspendMode::RetryBackoff),
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    let attempts = std::fs::read_to_string(&attempts_path).expect("read attempts");
    assert_eq!(
        attempts.trim(),
        "1",
        "stop signal should short-circuit retry_backoff retries"
    );

    let suspend_state_store = SuspendStateStore::new(temp_dir.path());
    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    assert_eq!(wait_result, Some(TerminationReason::Stopped));
    assert!(!temp_dir.path().join(".ralph/stop-requested").exists());
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_wait_then_retry_recovers_after_resume() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let attempts_path = temp_dir.path().join("wait-then-retry-attempts.txt");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreIterationStart,
        vec![hook_spec_with_command_and_on_error_and_suspend_mode(
            "wait-then-retry-pre-iteration-start",
            vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"attempts_file="$1"
attempt=0
if [ -f "$attempts_file" ]; then
  attempt="$(cat "$attempts_file")"
fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$attempts_file"
if [ "$attempt" -lt 2 ]; then
  exit 71
fi
exit 0"#
                    .to_string(),
                "wait-then-retry-hook".to_string(),
                attempts_path.to_string_lossy().into_owned(),
            ],
            HookOnError::Suspend,
            Some(HookSuspendMode::WaitThenRetry),
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop_with_diagnostics(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let resume_store = suspend_state_store.clone();
    let resume_handle = std::thread::spawn(move || {
        let wait_started_at = std::time::Instant::now();
        while !resume_store.suspend_state_path().exists() {
            assert!(
                wait_started_at.elapsed() < Duration::from_secs(2),
                "wait_then_retry should persist suspend-state before waiting"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        resume_store
            .write_resume_requested()
            .expect("write resume signal");
    });

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            None,
            None,
        ),
    );

    resume_handle
        .join()
        .expect("resume helper thread should not panic");

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Pass);
    assert_eq!(outcomes[0].suspend_mode, HookSuspendMode::WaitThenRetry);
    assert_eq!(outcomes[0].failure, None);

    let attempts = std::fs::read_to_string(&attempts_path).expect("read attempts");
    assert_eq!(
        attempts.trim(),
        "2",
        "wait_then_retry should run exactly one retry after resume"
    );
    assert!(
        suspend_state_store
            .read_suspend_state()
            .expect("read suspend-state after wait_then_retry")
            .is_none(),
        "suspend-state should be cleared after wait_then_retry resume"
    );
    assert!(
        !suspend_state_store.resume_requested_path().exists(),
        "resume signal should be consumed under wait_then_retry"
    );

    let telemetry_entries = read_hook_run_telemetry_entries(temp_dir.path());
    assert_eq!(telemetry_entries.len(), 2);
    assert_eq!(
        telemetry_entries
            .iter()
            .map(|entry| entry.retry_attempt)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(
        telemetry_entries
            .iter()
            .all(|entry| entry.retry_max_attempts == 2)
    );
    assert!(
        telemetry_entries
            .iter()
            .all(|entry| entry.suspend_mode == HookSuspendMode::WaitThenRetry)
    );
    assert_eq!(
        telemetry_entries
            .iter()
            .map(|entry| entry.disposition)
            .collect::<Vec<_>>(),
        vec![HookDisposition::Suspend, HookDisposition::Pass]
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_wait_then_retry_retry_failure_remains_suspended() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let attempts_path = temp_dir.path().join("wait-then-retry-attempts.txt");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PostIterationStart,
        vec![hook_spec_with_command_and_on_error_and_suspend_mode(
            "wait-then-retry-post-iteration-start",
            vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"attempts_file="$1"
attempt=0
if [ -f "$attempts_file" ]; then
  attempt="$(cat "$attempts_file")"
fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$attempts_file"
exit 72"#
                    .to_string(),
                "wait-then-retry-hook".to_string(),
                attempts_path.to_string_lossy().into_owned(),
            ],
            HookOnError::Suspend,
            Some(HookSuspendMode::WaitThenRetry),
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let resume_store = suspend_state_store.clone();
    let resume_handle = std::thread::spawn(move || {
        let wait_started_at = std::time::Instant::now();
        while !resume_store.suspend_state_path().exists() {
            assert!(
                wait_started_at.elapsed() < Duration::from_secs(2),
                "wait_then_retry should persist suspend-state before waiting"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        resume_store
            .write_resume_requested()
            .expect("write resume signal");
    });

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PostIterationStart,
        build_iteration_start_payload_input(
            "loop-test",
            &loop_ctx,
            5,
            1,
            Some("planner".to_string()),
            Some("builder".to_string()),
            Some("task-123".to_string()),
        ),
    );

    resume_handle
        .join()
        .expect("resume helper thread should not panic");

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].disposition, HookDisposition::Suspend);
    assert_eq!(outcomes[0].suspend_mode, HookSuspendMode::WaitThenRetry);
    assert_eq!(
        outcomes[0].failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(72),
            timed_out: false,
        })
    );

    let attempts = std::fs::read_to_string(&attempts_path).expect("read attempts");
    assert_eq!(
        attempts.trim(),
        "2",
        "wait_then_retry should run a single retry attempt after resume"
    );
    assert!(
        suspend_state_store
            .read_suspend_state()
            .expect("read suspend-state after wait_then_retry")
            .is_none(),
        "first wait_then_retry suspend-state should be cleared after resume"
    );
    assert!(
        !suspend_state_store.resume_requested_path().exists(),
        "resume signal should be consumed after wait_then_retry"
    );
}

#[cfg(unix)]
#[test]
fn test_dispatch_phase_event_hooks_wait_then_retry_prioritizes_stop_over_resume() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let attempts_path = temp_dir.path().join("wait-then-retry-attempts.txt");
    std::fs::create_dir_all(temp_dir.path().join(".ralph")).expect("create .ralph");
    std::fs::write(temp_dir.path().join(".ralph/stop-requested"), "").expect("write stop signal");
    std::fs::write(temp_dir.path().join(".ralph/resume-requested"), "")
        .expect("write resume signal");

    let mut events = std::collections::HashMap::new();
    events.insert(
        HookPhaseEvent::PreLoopStart,
        vec![hook_spec_with_command_and_on_error_and_suspend_mode(
            "wait-then-retry-pre-loop-start",
            vec![
                "sh".to_string(),
                "-c".to_string(),
                r#"attempts_file="$1"
attempt=0
if [ -f "$attempts_file" ]; then
  attempt="$(cat "$attempts_file")"
fi
attempt=$((attempt + 1))
printf '%s' "$attempt" > "$attempts_file"
exit 73"#
                    .to_string(),
                "wait-then-retry-hook".to_string(),
                attempts_path.to_string_lossy().into_owned(),
            ],
            HookOnError::Suspend,
            Some(HookSuspendMode::WaitThenRetry),
        )],
    );

    let hook_engine = hook_engine_with_events(events);
    let hook_executor = HookExecutor::new();
    let event_loop = dispatch_test_event_loop(temp_dir.path());
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());
    let suspend_state_store = SuspendStateStore::new(temp_dir.path());

    let outcomes = dispatch_phase_event_hooks(
        &event_loop,
        true,
        "loop-test",
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input("loop-test", &loop_ctx, 5, 1, Some("ralph".to_string())),
    );

    let attempts = std::fs::read_to_string(&attempts_path).expect("read attempts");
    assert_eq!(
        attempts.trim(),
        "1",
        "stop signal should prevent wait_then_retry from running the retry"
    );

    let wait_result = block_on_test_future(wait_for_resume_if_suspended(
        &outcomes,
        "loop-test",
        &suspend_state_store,
    ))
    .expect("wait helper should succeed");

    assert_eq!(wait_result, Some(TerminationReason::Stopped));
    assert!(!temp_dir.path().join(".ralph/stop-requested").exists());
    assert!(!suspend_state_store.resume_requested_path().exists());
}

#[test]
fn test_run_retry_backoff_policy_replays_configured_schedule_deterministically() {
    let mut observed_delays_ms = Vec::new();
    let mut observed_retry_attempts = Vec::new();

    let outcome = run_retry_backoff_policy(
        "pre.iteration.start",
        "retry-hook",
        &[3, 5, 8],
        |delay, retry_attempt| {
            observed_delays_ms.push(delay.as_millis() as u64);
            assert_eq!(retry_attempt, observed_delays_ms.len());
            RetryBackoffDelayOutcome::Elapsed
        },
        |retry_attempt| {
            observed_retry_attempts.push(retry_attempt);
            if retry_attempt == 4 {
                HookDispatchOutcome {
                    phase_event: HookPhaseEvent::PreIterationStart,
                    hook_name: "retry-hook".to_string(),
                    disposition: HookDisposition::Pass,
                    suspend_mode: HookSuspendMode::RetryBackoff,
                    failure: None,

                    mutation_parse_outcome: HookMutationParseOutcome::Disabled,
                }
            } else {
                suspend_outcome_with_mode(
                    HookPhaseEvent::PreIterationStart,
                    "retry-hook",
                    HookSuspendMode::RetryBackoff,
                )
            }
        },
        suspend_outcome_with_mode(
            HookPhaseEvent::PreIterationStart,
            "retry-hook",
            HookSuspendMode::RetryBackoff,
        ),
    );

    assert_eq!(observed_delays_ms, vec![3, 5, 8]);
    assert_eq!(observed_retry_attempts, vec![2, 3, 4]);
    assert_eq!(outcome.disposition, HookDisposition::Pass);
    assert_eq!(outcome.failure, None);
}

#[test]
fn test_run_retry_backoff_policy_exhausts_after_last_configured_delay() {
    let mut observed_retry_attempts = Vec::new();

    let outcome = run_retry_backoff_policy(
        "post.iteration.start",
        "retry-hook",
        &[11, 13],
        |_delay, _retry_attempt| RetryBackoffDelayOutcome::Elapsed,
        |retry_attempt| {
            observed_retry_attempts.push(retry_attempt);
            suspend_outcome_with_mode(
                HookPhaseEvent::PostIterationStart,
                "retry-hook",
                HookSuspendMode::RetryBackoff,
            )
        },
        suspend_outcome_with_mode(
            HookPhaseEvent::PostIterationStart,
            "retry-hook",
            HookSuspendMode::RetryBackoff,
        ),
    );

    assert_eq!(observed_retry_attempts, vec![2, 3]);
    assert_eq!(outcome.disposition, HookDisposition::Suspend);
    assert_eq!(
        outcome.failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(41),
            timed_out: false,
        })
    );
}

#[test]
fn test_run_retry_backoff_policy_stop_signal_short_circuits_before_retry_attempt() {
    let initial_outcome = suspend_outcome_with_mode(
        HookPhaseEvent::PreLoopStart,
        "retry-hook",
        HookSuspendMode::RetryBackoff,
    );
    let mut retry_attempt_called = false;

    let outcome = run_retry_backoff_policy(
        "pre.loop.start",
        "retry-hook",
        &[21, 34],
        |_delay, _retry_attempt| RetryBackoffDelayOutcome::StopRequested,
        |_retry_attempt| {
            retry_attempt_called = true;
            initial_outcome.clone()
        },
        initial_outcome.clone(),
    );

    assert!(!retry_attempt_called);
    assert_eq!(outcome, initial_outcome);
}

#[test]
fn test_run_wait_then_retry_policy_resume_retries_once_and_returns_retry_result() {
    let mut clear_suspend_calls = 0usize;
    let mut retry_calls = 0usize;

    let outcome = run_wait_then_retry_policy(
        "pre.iteration.start",
        "wait-hook",
        || Ok(SuspendWaitOutcome::Resume),
        || {
            clear_suspend_calls += 1;
            Ok(())
        },
        || {
            retry_calls += 1;
            HookDispatchOutcome {
                phase_event: HookPhaseEvent::PreIterationStart,
                hook_name: "wait-hook".to_string(),
                disposition: HookDisposition::Pass,
                suspend_mode: HookSuspendMode::WaitThenRetry,
                failure: None,

                mutation_parse_outcome: HookMutationParseOutcome::Disabled,
            }
        },
        suspend_outcome_with_mode(
            HookPhaseEvent::PreIterationStart,
            "wait-hook",
            HookSuspendMode::WaitThenRetry,
        ),
    );

    assert_eq!(clear_suspend_calls, 1);
    assert_eq!(retry_calls, 1);
    assert_eq!(outcome.disposition, HookDisposition::Pass);
    assert_eq!(outcome.failure, None);
}

#[test]
fn test_run_wait_then_retry_policy_retry_failure_returns_suspend() {
    let mut clear_suspend_calls = 0usize;
    let mut retry_calls = 0usize;

    let outcome = run_wait_then_retry_policy(
        "post.iteration.start",
        "wait-hook",
        || Ok(SuspendWaitOutcome::Resume),
        || {
            clear_suspend_calls += 1;
            Ok(())
        },
        || {
            retry_calls += 1;
            suspend_outcome_with_mode(
                HookPhaseEvent::PostIterationStart,
                "wait-hook",
                HookSuspendMode::WaitThenRetry,
            )
        },
        suspend_outcome_with_mode(
            HookPhaseEvent::PostIterationStart,
            "wait-hook",
            HookSuspendMode::WaitThenRetry,
        ),
    );

    assert_eq!(clear_suspend_calls, 1);
    assert_eq!(retry_calls, 1);
    assert_eq!(outcome.disposition, HookDisposition::Suspend);
    assert_eq!(
        outcome.failure,
        Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(41),
            timed_out: false,
        })
    );
}

#[test]
fn test_run_wait_then_retry_policy_stop_skips_retry_path() {
    let initial_outcome = suspend_outcome_with_mode(
        HookPhaseEvent::PreLoopStart,
        "wait-hook",
        HookSuspendMode::WaitThenRetry,
    );
    let mut clear_suspend_called = false;
    let mut retry_called = false;

    let outcome = run_wait_then_retry_policy(
        "pre.loop.start",
        "wait-hook",
        || Ok(SuspendWaitOutcome::Stop),
        || {
            clear_suspend_called = true;
            Ok(())
        },
        || {
            retry_called = true;
            HookDispatchOutcome {
                phase_event: HookPhaseEvent::PreLoopStart,
                hook_name: "wait-hook".to_string(),
                disposition: HookDisposition::Pass,
                suspend_mode: HookSuspendMode::WaitThenRetry,
                failure: None,

                mutation_parse_outcome: HookMutationParseOutcome::Disabled,
            }
        },
        initial_outcome.clone(),
    );

    assert!(!clear_suspend_called);
    assert!(!retry_called);
    assert_eq!(outcome, initial_outcome);
}

#[test]
fn test_fail_if_blocking_loop_start_outcomes_allows_non_blocking_dispositions() {
    let outcomes = vec![
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PreLoopStart,
            hook_name: "warn-hook".to_string(),
            disposition: HookDisposition::Warn,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: Some(HookDispatchFailure::HookRunFailed {
                exit_code: Some(7),
                timed_out: false,
            }),

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PostLoopStart,
            hook_name: "pass-hook".to_string(),
            disposition: HookDisposition::Pass,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: None,

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
    ];

    assert!(fail_if_blocking_loop_start_outcomes(&outcomes).is_ok());
}

#[test]
fn test_fail_if_blocking_loop_start_outcomes_surfaces_failure_context() {
    let blocked_exit_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PostLoopStart,
        hook_name: "block-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(42),
            timed_out: false,
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_exit_error = fail_if_blocking_loop_start_outcomes(&blocked_exit_outcomes)
        .expect_err("block disposition should fail loop.start boundary");
    let blocked_exit_message = blocked_exit_error.to_string();
    assert!(blocked_exit_message.contains("block-hook"));
    assert!(blocked_exit_message.contains("post.loop.start"));
    assert!(blocked_exit_message.contains("hook exited with code 42"));

    let blocked_exec_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PreLoopStart,
        hook_name: "block-exec-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookExecutionError {
            message: "spawn failed".to_string(),
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_exec_error = fail_if_blocking_loop_start_outcomes(&blocked_exec_outcomes)
        .expect_err("block disposition should fail loop.start boundary");
    let blocked_exec_message = blocked_exec_error.to_string();
    assert!(blocked_exec_message.contains("block-exec-hook"));
    assert!(blocked_exec_message.contains("pre.loop.start"));
    assert!(blocked_exec_message.contains("hook execution failed: spawn failed"));
}

#[test]
fn test_fail_if_blocking_iteration_start_outcomes_allows_non_blocking_dispositions() {
    let outcomes = vec![
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PreIterationStart,
            hook_name: "warn-hook".to_string(),
            disposition: HookDisposition::Warn,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: Some(HookDispatchFailure::HookRunFailed {
                exit_code: Some(9),
                timed_out: false,
            }),

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
        HookDispatchOutcome {
            phase_event: HookPhaseEvent::PostIterationStart,
            hook_name: "pass-hook".to_string(),
            disposition: HookDisposition::Pass,
            suspend_mode: HookSuspendMode::WaitForResume,
            failure: None,

            mutation_parse_outcome: HookMutationParseOutcome::Disabled,
        },
    ];

    assert!(fail_if_blocking_iteration_start_outcomes(&outcomes).is_ok());
}

#[test]
fn test_fail_if_blocking_iteration_start_outcomes_surfaces_failure_context() {
    let blocked_timeout_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PreIterationStart,
        hook_name: "block-timeout-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookRunFailed {
            exit_code: None,
            timed_out: true,
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_timeout_error =
        fail_if_blocking_iteration_start_outcomes(&blocked_timeout_outcomes)
            .expect_err("block disposition should fail iteration.start boundary");
    let blocked_timeout_message = blocked_timeout_error.to_string();
    assert!(blocked_timeout_message.contains("block-timeout-hook"));
    assert!(blocked_timeout_message.contains("pre.iteration.start"));
    assert!(blocked_timeout_message.contains("hook timed out"));

    let blocked_exec_outcomes = vec![HookDispatchOutcome {
        phase_event: HookPhaseEvent::PostIterationStart,
        hook_name: "block-exec-hook".to_string(),
        disposition: HookDisposition::Block,
        suspend_mode: HookSuspendMode::WaitForResume,
        failure: Some(HookDispatchFailure::HookExecutionError {
            message: "spawn failed".to_string(),
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }];

    let blocked_exec_error = fail_if_blocking_iteration_start_outcomes(&blocked_exec_outcomes)
        .expect_err("block disposition should fail iteration.start boundary");
    let blocked_exec_message = blocked_exec_error.to_string();
    assert!(blocked_exec_message.contains("block-exec-hook"));
    assert!(blocked_exec_message.contains("post.iteration.start"));
    assert!(blocked_exec_message.contains("hook execution failed: spawn failed"));
}

#[test]
fn test_loop_termination_phase_events_maps_success_and_error_reasons() {
    assert_eq!(
        loop_termination_phase_events(&TerminationReason::CompletionPromise),
        (
            HookPhaseEvent::PreLoopComplete,
            HookPhaseEvent::PostLoopComplete
        )
    );
    assert_eq!(
        loop_termination_phase_events(&TerminationReason::MaxRuntime),
        (HookPhaseEvent::PreLoopError, HookPhaseEvent::PostLoopError)
    );
}

#[test]
fn test_build_loop_termination_payload_input_sets_termination_reason_context() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let loop_ctx = LoopContext::primary(temp_dir.path().to_path_buf());

    let payload_input = build_loop_termination_payload_input(
        "loop-test",
        &loop_ctx,
        42,
        7,
        Some("planner".to_string()),
        Some("builder".to_string()),
        Some("task-123".to_string()),
        &TerminationReason::RestartRequested,
    );

    assert_eq!(
        payload_input.context.termination_reason.as_deref(),
        Some("restart_requested")
    );
    assert_eq!(payload_input.context.active_hat.as_deref(), Some("planner"));
    assert_eq!(
        payload_input.context.selected_hat.as_deref(),
        Some("builder")
    );
    assert_eq!(
        payload_input.context.selected_task.as_deref(),
        Some("task-123")
    );
}

fn hook_mutation_config(enabled: bool) -> HookMutationConfig {
    HookMutationConfig {
        enabled,
        format: Some("json".to_string()),
        extra: std::collections::HashMap::new(),
    }
}

fn json_object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    value.as_object().cloned().expect("json object")
}

#[test]
fn test_parse_hook_mutation_stdout_skips_when_disabled() {
    let outcome =
        parse_hook_mutation_stdout(&HookMutationConfig::default(), "env-guard", "not-json");

    assert_eq!(outcome, HookMutationParseOutcome::Disabled);
}

#[test]
fn test_parse_hook_mutation_stdout_accepts_metadata_only_payload_and_namespaces_by_hook() {
    let outcome = parse_hook_mutation_stdout(
        &hook_mutation_config(true),
        "env-guard",
        r#"{"metadata":{"risk_score":0.72,"gates":["policy_check"]}}"#,
    );

    let HookMutationParseOutcome::Parsed {
        namespaced_metadata,
    } = outcome
    else {
        panic!("expected parsed mutation payload");
    };

    assert_eq!(
        serde_json::Value::Object(namespaced_metadata),
        serde_json::json!({
            "hook_metadata": {
                "env-guard": {
                    "risk_score": 0.72,
                    "gates": ["policy_check"]
                }
            }
        })
    );
}

#[test]
fn test_parse_hook_mutation_stdout_rejects_non_json_payload_when_enabled() {
    let outcome = parse_hook_mutation_stdout(&hook_mutation_config(true), "env-guard", "oops");

    let HookMutationParseOutcome::Invalid(HookMutationParseError::InvalidJson { message }) =
        outcome
    else {
        panic!("expected invalid-json mutation parse outcome");
    };

    assert!(message.contains("valid JSON"));
}

#[test]
fn test_parse_hook_mutation_stdout_rejects_non_metadata_payload_shape() {
    let outcome = parse_hook_mutation_stdout(
        &hook_mutation_config(true),
        "env-guard",
        r#"{"metadata":{"risk_score":0.72},"prompt":"inject"}"#,
    );

    let HookMutationParseOutcome::Invalid(HookMutationParseError::InvalidSchema { message }) =
        outcome
    else {
        panic!("expected invalid-schema mutation parse outcome");
    };

    assert!(message.contains("supports only"));
}

#[test]
fn test_merge_hook_metadata_namespace_merges_multiple_hook_entries() {
    let mut accumulated_metadata = serde_json::Map::new();
    accumulated_metadata.insert("upstream".to_string(), serde_json::json!("preserved"));

    merge_hook_metadata_namespace(
        &mut accumulated_metadata,
        "env-guard",
        json_object(serde_json::json!({"risk_score": 0.72})),
    )
    .expect("merge env-guard metadata");

    merge_hook_metadata_namespace(
        &mut accumulated_metadata,
        "policy-gate",
        json_object(serde_json::json!({"status": "pass"})),
    )
    .expect("merge policy-gate metadata");

    assert_eq!(
        accumulated_metadata["upstream"],
        serde_json::json!("preserved")
    );
    assert_eq!(
        accumulated_metadata["hook_metadata"]["env-guard"]["risk_score"],
        serde_json::json!(0.72)
    );
    assert_eq!(
        accumulated_metadata["hook_metadata"]["policy-gate"]["status"],
        serde_json::json!("pass")
    );
}

#[test]
fn test_merge_hook_metadata_namespace_rejects_non_object_namespace_value() {
    let mut accumulated_metadata = serde_json::Map::new();
    accumulated_metadata.insert(
        "hook_metadata".to_string(),
        serde_json::Value::String("invalid".to_string()),
    );

    let merge_result = merge_hook_metadata_namespace(
        &mut accumulated_metadata,
        "env-guard",
        json_object(serde_json::json!({"risk_score": 0.72})),
    );

    assert!(matches!(
        merge_result,
        Err(HookMutationParseError::InvalidSchema { message })
        if message.contains("must be a JSON object")
    ));
}
