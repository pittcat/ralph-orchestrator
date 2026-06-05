use super::super::*;
pub fn dispatch_phase_event_hooks(
    event_loop: &EventLoop,
    hooks_enabled: bool,
    loop_id: &str,
    hook_engine: &HookEngine,
    hook_executor: &HookExecutor,
    phase_event: HookPhaseEvent,
    payload_input: HookPayloadBuilderInput,
) -> Vec<HookDispatchOutcome> {
    if !hooks_enabled {
        return Vec::new();
    }

    let resolved_hooks = hook_engine.resolve_phase_event(phase_event);
    if resolved_hooks.is_empty() {
        return Vec::new();
    }

    let workspace_root = payload_input.workspace.clone();
    let payload = hook_engine.build_payload(phase_event, payload_input);
    let stdin_payload = match serde_json::to_value(&payload) {
        Ok(value) => value,
        Err(error) => {
            warn!(
                phase_event = %phase_event,
                error = %error,
                "Failed to serialize lifecycle hook payload; skipping phase-event dispatch"
            );
            return Vec::new();
        }
    };

    let mut outcomes = Vec::with_capacity(resolved_hooks.len());

    for hook in resolved_hooks {
        let hook_name = hook.name.clone();
        let phase_event_key = hook.phase_event.as_str().to_string();

        let request = HookRunRequest {
            phase_event: phase_event_key.clone(),
            hook_name: hook_name.clone(),
            command: hook.command.clone(),
            workspace_root: workspace_root.clone(),
            cwd: hook.cwd.clone(),
            env: hook.env.clone(),
            timeout_seconds: hook.timeout_seconds,
            max_output_bytes: hook.max_output_bytes,
            stdin_payload: stdin_payload.clone(),
        };

        let outcome = dispatch_hook_with_suspend_policy(
            event_loop,
            hook_executor,
            loop_id,
            &phase_event_key,
            hook.phase_event,
            &hook_name,
            hook.on_error,
            hook.suspend_mode,
            &hook.mutate,
            &request,
        );
        outcomes.push(outcome);
    }

    outcomes
}

#[allow(clippy::too_many_arguments)]
pub fn dispatch_hook_with_suspend_policy(
    event_loop: &EventLoop,
    hook_executor: &HookExecutor,
    loop_id: &str,
    phase_event_key: &str,
    phase_event: HookPhaseEvent,
    hook_name: &str,
    on_error: HookOnError,
    suspend_mode: HookSuspendMode,
    mutate: &HookMutationConfig,
    request: &HookRunRequest,
) -> HookDispatchOutcome {
    let retry_max_attempts = max_retry_attempts_for_suspend_mode(suspend_mode);
    let outcome = execute_hook_attempt(
        event_loop,
        hook_executor,
        loop_id,
        phase_event_key,
        phase_event,
        hook_name,
        on_error,
        suspend_mode,
        mutate,
        1,
        retry_max_attempts,
        request,
    );

    if outcome.disposition != HookDisposition::Suspend {
        return outcome;
    }

    match suspend_mode {
        HookSuspendMode::WaitForResume => outcome,
        HookSuspendMode::RetryBackoff => dispatch_retry_backoff_suspend_policy(
            event_loop,
            hook_executor,
            loop_id,
            phase_event_key,
            phase_event,
            hook_name,
            on_error,
            suspend_mode,
            mutate,
            retry_max_attempts,
            request,
            outcome,
        ),
        HookSuspendMode::WaitThenRetry => dispatch_wait_then_retry_suspend_policy(
            event_loop,
            hook_executor,
            loop_id,
            phase_event_key,
            phase_event,
            hook_name,
            on_error,
            suspend_mode,
            mutate,
            retry_max_attempts,
            request,
            outcome,
        ),
    }
}
pub fn execute_hook_attempt(
    event_loop: &EventLoop,
    hook_executor: &HookExecutor,
    loop_id: &str,
    phase_event_key: &str,
    phase_event: HookPhaseEvent,
    hook_name: &str,
    on_error: HookOnError,
    suspend_mode: HookSuspendMode,
    mutate: &HookMutationConfig,
    retry_attempt: u32,
    retry_max_attempts: u32,
    request: &HookRunRequest,
) -> HookDispatchOutcome {
    match hook_executor.run(request.clone()) {
        Ok(run_result) => {
            let run_disposition = classify_hook_disposition(on_error, &run_result);
            let mutation_parse_outcome =
                parse_hook_mutation_stdout(mutate, hook_name, &run_result.stdout.content);
            let mutation_failure = if run_disposition == HookDisposition::Pass {
                mutation_parse_failure(&mutation_parse_outcome)
            } else {
                None
            };

            let disposition = if mutation_failure.is_some() {
                disposition_from_on_error(on_error)
            } else {
                run_disposition
            };

            let failure = if let Some(mutation_failure) = mutation_failure {
                Some(mutation_failure)
            } else if run_disposition == HookDisposition::Pass {
                None
            } else {
                Some(HookDispatchFailure::HookRunFailed {
                    exit_code: run_result.exit_code,
                    timed_out: run_result.timed_out,
                })
            };

            event_loop.log_hook_run_telemetry(HookRunTelemetryEntry::from_run_result(
                loop_id,
                phase_event_key,
                hook_name,
                disposition,
                suspend_mode,
                retry_attempt,
                retry_max_attempts,
                &run_result,
            ));

            if disposition == HookDisposition::Pass {
                debug!(
                    phase_event = %phase_event_key,
                    hook_name = %hook_name,
                    duration_ms = run_result.duration_ms,
                    "Lifecycle hook executed successfully"
                );
            } else {
                let failure_detail = format_hook_failure_detail(failure.as_ref());
                warn!(
                    phase_event = %phase_event_key,
                    hook_name = %hook_name,
                    disposition = ?disposition,
                    exit_code = ?run_result.exit_code,
                    timed_out = run_result.timed_out,
                    failure = %failure_detail,
                    "Lifecycle hook returned non-pass disposition; continuing"
                );
            }

            HookDispatchOutcome {
                phase_event,
                hook_name: hook_name.to_string(),
                disposition,
                suspend_mode,
                failure,
                mutation_parse_outcome,
            }
        }
        Err(error) => {
            let disposition = disposition_from_on_error(on_error);

            warn!(
                phase_event = %phase_event_key,
                hook_name = %hook_name,
                disposition = ?disposition,
                error = %error,
                "Lifecycle hook execution failed; continuing"
            );

            HookDispatchOutcome {
                phase_event,
                hook_name: hook_name.to_string(),
                disposition,
                suspend_mode,
                failure: Some(HookDispatchFailure::HookExecutionError {
                    message: error.to_string(),
                }),
                mutation_parse_outcome: HookMutationParseOutcome::Disabled,
            }
        }
    }
}
