use super::super::*;
use super::super::payload_inputs;
pub fn loop_termination_phase_events(
    reason: &TerminationReason,
) -> (HookPhaseEvent, HookPhaseEvent) {
    if reason.is_success() {
        (
            HookPhaseEvent::PreLoopComplete,
            HookPhaseEvent::PostLoopComplete,
        )
    } else {
        (HookPhaseEvent::PreLoopError, HookPhaseEvent::PostLoopError)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn dispatch_pre_loop_termination_hooks(
    event_loop: &EventLoop,
    hooks_dispatch_enabled: bool,
    loop_id: &str,
    hook_engine: &HookEngine,
    hook_executor: &HookExecutor,
    suspend_state_store: &SuspendStateStore,
    ctx: &LoopContext,
    max_iterations: u32,
    accumulated_hook_metadata: &mut serde_json::Map<String, serde_json::Value>,
    reason: TerminationReason,
) -> impl std::future::Future<Output = Result<TerminationReason>> + Send {
    let outcomes = collect_loop_termination_hook_outcomes(
        event_loop,
        hooks_dispatch_enabled,
        loop_id,
        hook_engine,
        hook_executor,
        ctx,
        max_iterations,
        accumulated_hook_metadata,
        &reason,
        true,
    );
    let loop_id = loop_id.to_string();
    let suspend_state_store = suspend_state_store.clone();

    async move {
        resolve_loop_termination_hook_outcomes(&outcomes, &loop_id, &suspend_state_store, reason)
            .await
    }
}

#[allow(clippy::too_many_arguments)]
pub fn dispatch_post_loop_termination_hooks(
    event_loop: &EventLoop,
    hooks_dispatch_enabled: bool,
    loop_id: &str,
    hook_engine: &HookEngine,
    hook_executor: &HookExecutor,
    suspend_state_store: &SuspendStateStore,
    ctx: &LoopContext,
    max_iterations: u32,
    accumulated_hook_metadata: &mut serde_json::Map<String, serde_json::Value>,
    reason: TerminationReason,
) -> impl std::future::Future<Output = Result<TerminationReason>> + Send {
    let outcomes = collect_loop_termination_hook_outcomes(
        event_loop,
        hooks_dispatch_enabled,
        loop_id,
        hook_engine,
        hook_executor,
        ctx,
        max_iterations,
        accumulated_hook_metadata,
        &reason,
        false,
    );
    let loop_id = loop_id.to_string();
    let suspend_state_store = suspend_state_store.clone();

    async move {
        resolve_loop_termination_hook_outcomes(&outcomes, &loop_id, &suspend_state_store, reason)
            .await
    }
}

#[allow(clippy::too_many_arguments)]
pub fn collect_loop_termination_hook_outcomes(
    event_loop: &EventLoop,
    hooks_dispatch_enabled: bool,
    loop_id: &str,
    hook_engine: &HookEngine,
    hook_executor: &HookExecutor,
    ctx: &LoopContext,
    max_iterations: u32,
    accumulated_hook_metadata: &mut serde_json::Map<String, serde_json::Value>,
    reason: &TerminationReason,
    is_pre_phase: bool,
) -> Vec<HookDispatchOutcome> {
    let (pre_phase_event, post_phase_event) = loop_termination_phase_events(reason);
    let phase_event = if is_pre_phase {
        pre_phase_event
    } else {
        post_phase_event
    };

    let active_hat = event_loop.get_active_hat_id().as_str().to_string();
    let outcomes = dispatch_phase_event_hooks(
        event_loop,
        hooks_dispatch_enabled,
        loop_id,
        hook_engine,
        hook_executor,
        phase_event,
        payload_inputs::build_loop_termination_payload_input(
            loop_id,
            ctx,
            max_iterations,
            event_loop.state().iteration,
            Some(active_hat.clone()),
            Some(active_hat),
            None,
            reason,
            accumulated_hook_metadata,
        ),
    );
    merge_accumulated_hook_metadata_from_outcomes(accumulated_hook_metadata, &outcomes);
    outcomes
}

pub async fn resolve_loop_termination_hook_outcomes(
    outcomes: &[HookDispatchOutcome],
    loop_id: &str,
    suspend_state_store: &SuspendStateStore,
    reason: TerminationReason,
) -> Result<TerminationReason> {
    fail_if_blocking_loop_termination_outcomes(outcomes)?;

    if let Some(termination_reason) =
        wait_for_resume_if_suspended(outcomes, loop_id, suspend_state_store).await?
    {
        return Ok(termination_reason);
    }

    Ok(reason)
}
