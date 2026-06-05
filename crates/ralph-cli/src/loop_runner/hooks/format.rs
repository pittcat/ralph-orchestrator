use super::super::*;
pub fn fail_if_blocking_loop_start_outcomes(outcomes: &[HookDispatchOutcome]) -> Result<()> {
    let Some(blocking_outcome) = outcomes
        .iter()
        .find(|outcome| outcome.disposition == HookDisposition::Block)
    else {
        return Ok(());
    };

    let reason = format_blocking_hook_reason(blocking_outcome);
    error!(
        phase_event = %blocking_outcome.phase_event,
        hook_name = %blocking_outcome.hook_name,
        reason = %reason,
        "Lifecycle hook blocked loop.start boundary"
    );

    Err(anyhow::anyhow!(reason))
}

pub fn fail_if_blocking_iteration_start_outcomes(outcomes: &[HookDispatchOutcome]) -> Result<()> {
    let Some(blocking_outcome) = outcomes
        .iter()
        .find(|outcome| outcome.disposition == HookDisposition::Block)
    else {
        return Ok(());
    };

    let reason = format_blocking_hook_reason(blocking_outcome);
    error!(
        phase_event = %blocking_outcome.phase_event,
        hook_name = %blocking_outcome.hook_name,
        reason = %reason,
        "Lifecycle hook blocked iteration.start boundary"
    );

    Err(anyhow::anyhow!(reason))
}

pub fn fail_if_blocking_plan_created_outcomes(outcomes: &[HookDispatchOutcome]) -> Result<()> {
    let Some(blocking_outcome) = outcomes
        .iter()
        .find(|outcome| outcome.disposition == HookDisposition::Block)
    else {
        return Ok(());
    };

    let reason = format_blocking_hook_reason(blocking_outcome);
    error!(
        phase_event = %blocking_outcome.phase_event,
        hook_name = %blocking_outcome.hook_name,
        reason = %reason,
        "Lifecycle hook blocked plan.created boundary"
    );

    Err(anyhow::anyhow!(reason))
}

pub fn fail_if_blocking_human_interact_outcomes(outcomes: &[HookDispatchOutcome]) -> Result<()> {
    let Some(blocking_outcome) = outcomes
        .iter()
        .find(|outcome| outcome.disposition == HookDisposition::Block)
    else {
        return Ok(());
    };

    let reason = format_blocking_hook_reason(blocking_outcome);
    error!(
        phase_event = %blocking_outcome.phase_event,
        hook_name = %blocking_outcome.hook_name,
        reason = %reason,
        "Lifecycle hook blocked human.interact boundary"
    );

    Err(anyhow::anyhow!(reason))
}

pub fn fail_if_blocking_loop_termination_outcomes(outcomes: &[HookDispatchOutcome]) -> Result<()> {
    let Some(blocking_outcome) = outcomes
        .iter()
        .find(|outcome| outcome.disposition == HookDisposition::Block)
    else {
        return Ok(());
    };

    let reason = format_blocking_hook_reason(blocking_outcome);
    error!(
        phase_event = %blocking_outcome.phase_event,
        hook_name = %blocking_outcome.hook_name,
        reason = %reason,
        "Lifecycle hook blocked loop termination boundary"
    );

    Err(anyhow::anyhow!(reason))
}

pub fn format_suspending_hook_reason(outcome: &HookDispatchOutcome) -> String {
    format!(
        "Lifecycle hook '{}' suspended orchestration at '{}': {}",
        outcome.hook_name,
        outcome.phase_event.as_str(),
        format_hook_failure_detail(outcome.failure.as_ref())
    )
}

pub fn format_blocking_hook_reason(outcome: &HookDispatchOutcome) -> String {
    format!(
        "Lifecycle hook '{}' blocked orchestration at '{}': {}",
        outcome.hook_name,
        outcome.phase_event.as_str(),
        format_hook_failure_detail(outcome.failure.as_ref())
    )
}

pub fn format_hook_failure_detail(failure: Option<&HookDispatchFailure>) -> String {
    match failure {
        Some(HookDispatchFailure::HookRunFailed {
            exit_code,
            timed_out,
        }) => {
            if *timed_out {
                "hook timed out".to_string()
            } else if let Some(code) = exit_code {
                format!("hook exited with code {code}")
            } else {
                "hook exited unsuccessfully".to_string()
            }
        }
        Some(HookDispatchFailure::HookExecutionError { message }) => {
            format!("hook execution failed: {message}")
        }
        Some(HookDispatchFailure::InvalidMutationOutput { message }) => {
            format!("invalid mutation output: {message}")
        }
        None => "hook failed without failure details".to_string(),
    }
}

pub fn classify_hook_disposition(
    on_error: HookOnError,
    run_result: &HookRunResult,
) -> HookDisposition {
    if !run_result.timed_out && run_result.exit_code == Some(0) {
        HookDisposition::Pass
    } else {
        disposition_from_on_error(on_error)
    }
}

pub fn disposition_from_on_error(on_error: HookOnError) -> HookDisposition {
    match on_error {
        HookOnError::Warn => HookDisposition::Warn,
        HookOnError::Block => HookDisposition::Block,
        HookOnError::Suspend => HookDisposition::Suspend,
    }
}

/// Executes a prompt in PTY mode with raw terminal handling.
/// Converts PTY termination type to loop termination reason.
///
/// In interactive mode, idle timeout signals "iteration complete" rather than
/// "loop stopped", allowing the event loop to process output and continue.
///
/// # Arguments
/// * `termination_type` - The PTY executor's termination type
/// * `interactive` - Whether running in interactive mode
///
/// # Returns
/// * `None` - Continue processing (iteration complete)
/// * `Some(TerminationReason)` - Stop the loop
pub fn convert_termination_type(
    termination_type: ralph_adapters::TerminationType,
    interactive: bool,
) -> Option<TerminationReason> {
    match termination_type {
        ralph_adapters::TerminationType::Natural => None,
        ralph_adapters::TerminationType::IdleTimeout => {
            if interactive {
                // In interactive mode, idle timeout signals iteration complete,
                // not loop termination. Let output be processed for events.
                info!("PTY idle timeout in interactive mode, iteration complete");
                None
            } else {
                warn!("PTY idle timeout reached, terminating loop");
                Some(TerminationReason::Stopped)
            }
        }
        ralph_adapters::TerminationType::UserInterrupt
        | ralph_adapters::TerminationType::ForceKill => Some(TerminationReason::Interrupted),
    }
}
