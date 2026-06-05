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
