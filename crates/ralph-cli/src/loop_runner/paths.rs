use super::*;

pub fn resolve_current_events_path(ctx: &LoopContext) -> PathBuf {
    ctx.resolve_events_path()
}

pub fn current_candidate_events_marker(ctx: &LoopContext) -> PathBuf {
    ctx.ralph_dir().join("current-candidate-events")
}

pub fn resolve_candidate_events_path(ctx: &LoopContext) -> PathBuf {
    fs::read_to_string(current_candidate_events_marker(ctx))
        .ok()
        .map(|relative| {
            let relative = relative.trim().to_string();
            if std::path::Path::new(&relative).is_relative() {
                ctx.workspace().join(relative)
            } else {
                PathBuf::from(relative)
            }
        })
        .unwrap_or_else(|| ctx.ralph_dir().join("event-candidates.jsonl"))
}

pub fn resolve_emit_events_path(ctx: &LoopContext, state_machine_enabled: bool) -> PathBuf {
    if state_machine_enabled {
        resolve_candidate_events_path(ctx)
    } else {
        resolve_current_events_path(ctx)
    }
}

pub fn config_state_machine_enabled(config: &RalphConfig) -> bool {
    config
        .event_loop
        .state_machine
        .as_ref()
        .is_some_and(|sm| sm.enabled)
}

/// Path to the per-hat channel marker file.
///
/// In isolated mode the runner writes this marker before spawning a hat
/// so that `ralph emit` is allowed to write to the dedicated channel file.
pub fn current_hat_events_marker(ctx: &LoopContext) -> PathBuf {
    ctx.ralph_dir().join("current-hat-events")
}

/// Resolve the per-hat channel events file from the marker, if present.
pub fn resolve_hat_channel_events_path(ctx: &LoopContext) -> Option<PathBuf> {
    fs::read_to_string(current_hat_events_marker(ctx))
        .ok()
        .and_then(|value| {
            let relative = value.trim();
            if relative.is_empty() {
                return None;
            }
            let path = std::path::Path::new(relative);
            Some(if path.is_relative() {
                ctx.workspace().join(path)
            } else {
                path.to_path_buf()
            })
        })
}

/// Build a dedicated write-channel path for a hat activation.
pub fn hat_channel_events_path(
    ctx: &LoopContext,
    hat_id: &str,
    loop_id: &str,
    iteration: u32,
) -> PathBuf {
    ctx.agent_dir()
        .join(format!("events-hat-{hat_id}-{loop_id}-{iteration}.jsonl"))
}

/// True when the loaded config is in isolated execution mode.
pub fn is_isolated_mode(config: &RalphConfig) -> bool {
    config.event_loop.execution_mode == ralph_core::config::HatExecutionMode::Isolated
}
