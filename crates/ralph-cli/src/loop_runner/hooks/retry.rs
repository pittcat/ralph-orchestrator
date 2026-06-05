use super::super::*;
pub fn dispatch_retry_backoff_suspend_policy(
    event_loop: &EventLoop,
    hook_executor: &HookExecutor,
    loop_id: &str,
    phase_event_key: &str,
    phase_event: HookPhaseEvent,
    hook_name: &str,
    on_error: HookOnError,
    suspend_mode: HookSuspendMode,
    mutate: &HookMutationConfig,
    retry_max_attempts: u32,
    request: &HookRunRequest,
    outcome: HookDispatchOutcome,
) -> HookDispatchOutcome {
    run_retry_backoff_policy(
        phase_event_key,
        hook_name,
        &RETRY_BACKOFF_DELAYS_MS,
        |backoff_delay, _retry_attempt| {
            wait_for_retry_backoff_delay_with_signal_poll(
                request.workspace_root.as_path(),
                backoff_delay,
            )
        },
        |retry_attempt| {
            execute_hook_attempt(
                event_loop,
                hook_executor,
                loop_id,
                phase_event_key,
                phase_event,
                hook_name,
                on_error,
                suspend_mode,
                mutate,
                retry_attempt,
                retry_max_attempts,
                request,
            )
        },
        outcome,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn dispatch_wait_then_retry_suspend_policy(
    event_loop: &EventLoop,
    hook_executor: &HookExecutor,
    loop_id: &str,
    phase_event_key: &str,
    phase_event: HookPhaseEvent,
    hook_name: &str,
    on_error: HookOnError,
    suspend_mode: HookSuspendMode,
    mutate: &HookMutationConfig,
    retry_max_attempts: u32,
    request: &HookRunRequest,
    outcome: HookDispatchOutcome,
) -> HookDispatchOutcome {
    let suspend_state_store = SuspendStateStore::new(&request.workspace_root);
    let reason = format_suspending_hook_reason(&outcome);
    let suspend_state = SuspendStateRecord::new(
        loop_id,
        phase_event,
        hook_name,
        reason,
        suspend_mode,
        chrono::Utc::now(),
    );

    if let Err(error) = suspend_state_store.write_suspend_state(&suspend_state) {
        warn!(
            phase_event = %phase_event_key,
            hook_name = %hook_name,
            error = %error,
            "Failed to persist suspend-state for wait_then_retry; deferring to standard suspend handling"
        );
        return outcome;
    }

    warn!(
        phase_event = %phase_event_key,
        hook_name = %hook_name,
        "Lifecycle hook requested suspend(wait_then_retry); entering wait-for-resume gate before single retry"
    );

    run_wait_then_retry_policy(
        phase_event_key,
        hook_name,
        || wait_for_suspend_signal_with_poll(&suspend_state_store),
        || {
            suspend_state_store
                .clear_suspend_state()
                .context("Failed to clear wait_then_retry suspend-state after resume")?;
            Ok(())
        },
        || {
            execute_hook_attempt(
                event_loop,
                hook_executor,
                loop_id,
                phase_event_key,
                phase_event,
                hook_name,
                on_error,
                suspend_mode,
                mutate,
                2,
                retry_max_attempts,
                request,
            )
        },
        outcome,
    )
}

pub fn run_retry_backoff_policy<FWaitForDelay, FRunRetryAttempt>(
    phase_event_key: &str,
    hook_name: &str,
    backoff_delays_ms: &[u64],
    mut wait_for_delay: FWaitForDelay,
    mut run_retry_attempt: FRunRetryAttempt,
    mut outcome: HookDispatchOutcome,
) -> HookDispatchOutcome
where
    FWaitForDelay: FnMut(Duration, usize) -> RetryBackoffDelayOutcome,
    FRunRetryAttempt: FnMut(u32) -> HookDispatchOutcome,
{
    for (retry_attempt, backoff_delay_ms) in backoff_delays_ms.iter().copied().enumerate() {
        match wait_for_delay(Duration::from_millis(backoff_delay_ms), retry_attempt + 1) {
            RetryBackoffDelayOutcome::Elapsed => {}
            RetryBackoffDelayOutcome::StopRequested => {
                info!(
                    phase_event = %phase_event_key,
                    hook_name = %hook_name,
                    retry_attempt = retry_attempt + 1,
                    "Stop requested while waiting for retry_backoff retry; deferring to suspend termination handling"
                );
                break;
            }
            RetryBackoffDelayOutcome::RestartRequested => {
                info!(
                    phase_event = %phase_event_key,
                    hook_name = %hook_name,
                    retry_attempt = retry_attempt + 1,
                    "Restart requested while waiting for retry_backoff retry; deferring to suspend termination handling"
                );
                break;
            }
        }

        outcome = run_retry_attempt(retry_attempt as u32 + 2);

        if outcome.disposition == HookDisposition::Pass {
            info!(
                phase_event = %phase_event_key,
                hook_name = %hook_name,
                retry_attempt = retry_attempt + 1,
                "Lifecycle hook recovered under retry_backoff"
            );
            return outcome;
        }

        if outcome.disposition != HookDisposition::Suspend {
            return outcome;
        }
    }

    warn!(
        phase_event = %phase_event_key,
        hook_name = %hook_name,
        retry_attempts = backoff_delays_ms.len(),
        "Lifecycle hook retry_backoff policy exhausted; entering suspended wait_for_resume fallback"
    );

    outcome
}

pub fn run_wait_then_retry_policy<FWaitForSignal, FClearSuspendState, FRunRetryAttempt>(
    phase_event_key: &str,
    hook_name: &str,
    mut wait_for_signal: FWaitForSignal,
    mut clear_suspend_state: FClearSuspendState,
    mut run_retry_attempt: FRunRetryAttempt,
    outcome: HookDispatchOutcome,
) -> HookDispatchOutcome
where
    FWaitForSignal: FnMut() -> Result<SuspendWaitOutcome>,
    FClearSuspendState: FnMut() -> Result<()>,
    FRunRetryAttempt: FnMut() -> HookDispatchOutcome,
{
    let wait_outcome = match wait_for_signal() {
        Ok(wait_outcome) => wait_outcome,
        Err(error) => {
            warn!(
                phase_event = %phase_event_key,
                hook_name = %hook_name,
                error = %error,
                "wait_then_retry gate failed while polling suspend signals; deferring to standard suspend handling"
            );
            return outcome;
        }
    };

    match wait_outcome {
        SuspendWaitOutcome::Stop => {
            info!(
                phase_event = %phase_event_key,
                hook_name = %hook_name,
                "Stop requested while waiting under wait_then_retry; deferring to suspend termination handling"
            );
            outcome
        }
        SuspendWaitOutcome::Restart => {
            info!(
                phase_event = %phase_event_key,
                hook_name = %hook_name,
                "Restart requested while waiting under wait_then_retry; deferring to suspend termination handling"
            );
            outcome
        }
        SuspendWaitOutcome::Resume => {
            if let Err(error) = clear_suspend_state() {
                warn!(
                    phase_event = %phase_event_key,
                    hook_name = %hook_name,
                    error = %error,
                    "Failed to clear wait_then_retry suspend-state after resume; deferring to standard suspend handling"
                );
                return outcome;
            }

            let retry_outcome = run_retry_attempt();

            if retry_outcome.disposition == HookDisposition::Pass {
                info!(
                    phase_event = %phase_event_key,
                    hook_name = %hook_name,
                    "Lifecycle hook recovered under wait_then_retry"
                );
            }

            retry_outcome
        }
    }
}
