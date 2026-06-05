use super::*;

pub const RETRY_BACKOFF_DELAYS_MS: [u64; 3] = [100, 200, 400];
pub const RETRY_BACKOFF_SIGNAL_POLL_INTERVAL_MS: u64 = 100;
pub const SUSPEND_WAIT_SIGNAL_POLL_INTERVAL_MS: u64 = 250;

pub fn max_retry_attempts_for_suspend_mode(suspend_mode: HookSuspendMode) -> u32 {
    match suspend_mode {
        HookSuspendMode::WaitForResume => 1,
        HookSuspendMode::RetryBackoff => RETRY_BACKOFF_DELAYS_MS.len() as u32 + 1,
        HookSuspendMode::WaitThenRetry => 2,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuspendWaitOutcome {
    Resume,
    Stop,
    Restart,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookDispatchOutcome {
    pub phase_event: HookPhaseEvent,
    pub hook_name: String,
    pub disposition: HookDisposition,
    pub suspend_mode: HookSuspendMode,
    pub failure: Option<HookDispatchFailure>,
    pub mutation_parse_outcome: HookMutationParseOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookDispatchFailure {
    HookRunFailed {
        exit_code: Option<i32>,
        timed_out: bool,
    },
    HookExecutionError {
        message: String,
    },
    InvalidMutationOutput {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryBackoffDelayOutcome {
    Elapsed,
    StopRequested,
    RestartRequested,
}

pub fn wait_for_retry_backoff_delay_with_signal_poll(
    workspace_root: &Path,
    backoff_delay: Duration,
) -> RetryBackoffDelayOutcome {
    if backoff_delay.is_zero() {
        return RetryBackoffDelayOutcome::Elapsed;
    }

    let poll_interval = Duration::from_millis(RETRY_BACKOFF_SIGNAL_POLL_INTERVAL_MS);
    let sleep_started_at = std::time::Instant::now();

    loop {
        if is_stop_requested(workspace_root) {
            return RetryBackoffDelayOutcome::StopRequested;
        }

        if is_restart_requested(workspace_root) {
            return RetryBackoffDelayOutcome::RestartRequested;
        }

        let elapsed = sleep_started_at.elapsed();
        if elapsed >= backoff_delay {
            return RetryBackoffDelayOutcome::Elapsed;
        }

        let remaining = backoff_delay.saturating_sub(elapsed);
        std::thread::sleep(std::cmp::min(remaining, poll_interval));
    }
}

pub fn wait_for_suspend_signal_with_poll(
    suspend_state_store: &SuspendStateStore,
) -> Result<SuspendWaitOutcome> {
    let poll_interval = Duration::from_millis(SUSPEND_WAIT_SIGNAL_POLL_INTERVAL_MS);

    loop {
        if is_stop_requested(suspend_state_store.workspace_root()) {
            return Ok(SuspendWaitOutcome::Stop);
        }

        if is_restart_requested(suspend_state_store.workspace_root()) {
            return Ok(SuspendWaitOutcome::Restart);
        }

        if suspend_state_store
            .consume_resume_requested()
            .context("Failed to consume resume signal while suspended")?
        {
            return Ok(SuspendWaitOutcome::Resume);
        }

        std::thread::sleep(poll_interval);
    }
}

pub async fn wait_for_resume_if_suspended(
    outcomes: &[HookDispatchOutcome],
    loop_id: &str,
    suspend_state_store: &SuspendStateStore,
) -> Result<Option<TerminationReason>> {
    let Some(suspending_outcome) = outcomes
        .iter()
        .find(|outcome| outcome.disposition == HookDisposition::Suspend)
    else {
        return Ok(None);
    };

    let reason = format_suspending_hook_reason(suspending_outcome);
    let suspend_state = SuspendStateRecord::new(
        loop_id,
        suspending_outcome.phase_event,
        &suspending_outcome.hook_name,
        &reason,
        suspending_outcome.suspend_mode,
        chrono::Utc::now(),
    );

    suspend_state_store
        .write_suspend_state(&suspend_state)
        .with_context(|| {
            format!(
                "Failed to persist suspend-state for hook '{}' at '{}'",
                suspending_outcome.hook_name,
                suspending_outcome.phase_event.as_str()
            )
        })?;

    warn!(
        phase_event = %suspending_outcome.phase_event,
        hook_name = %suspending_outcome.hook_name,
        suspend_mode = ?suspending_outcome.suspend_mode,
        reason = %reason,
        "Lifecycle hook requested suspend; entering wait_for_resume gate"
    );

    loop {
        if consume_stop_requested_signal(suspend_state_store.workspace_root())? {
            clear_suspend_wait_artifacts(suspend_state_store)?;
            info!(
                phase_event = %suspending_outcome.phase_event,
                hook_name = %suspending_outcome.hook_name,
                "Stop requested while suspended; terminating loop"
            );
            return Ok(Some(TerminationReason::Stopped));
        }

        if is_restart_requested(suspend_state_store.workspace_root()) {
            clear_suspend_wait_artifacts(suspend_state_store)?;
            info!(
                phase_event = %suspending_outcome.phase_event,
                hook_name = %suspending_outcome.hook_name,
                "Restart requested while suspended; terminating loop for restart"
            );
            return Ok(Some(TerminationReason::RestartRequested));
        }

        if suspend_state_store
            .consume_resume_requested()
            .context("Failed to consume resume signal while suspended")?
        {
            suspend_state_store
                .clear_suspend_state()
                .context("Failed to clear suspend-state after resume signal")?;

            info!(
                phase_event = %suspending_outcome.phase_event,
                hook_name = %suspending_outcome.hook_name,
                "Resume signal consumed; leaving suspended wait_for_resume state"
            );
            return Ok(None);
        }

        tokio::time::sleep(Duration::from_millis(SUSPEND_WAIT_SIGNAL_POLL_INTERVAL_MS)).await;
    }
}

pub fn clear_suspend_wait_artifacts(suspend_state_store: &SuspendStateStore) -> Result<()> {
    suspend_state_store
        .clear_suspend_state()
        .context("Failed to clear suspend-state artifact")?;
    suspend_state_store
        .consume_resume_requested()
        .context("Failed to clear stale resume signal")?;
    Ok(())
}

pub fn is_stop_requested(workspace_root: &Path) -> bool {
    workspace_root.join(".ralph/stop-requested").exists()
}

pub fn consume_stop_requested_signal(workspace_root: &Path) -> Result<bool> {
    let stop_path = workspace_root.join(".ralph/stop-requested");
    match fs::remove_file(&stop_path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(anyhow::Error::new(error)).with_context(|| {
            format!(
                "Failed to consume stop signal while suspended: {}",
                stop_path.display()
            )
        }),
    }
}

pub fn is_restart_requested(workspace_root: &Path) -> bool {
    workspace_root.join(".ralph/restart-requested").exists()
}
