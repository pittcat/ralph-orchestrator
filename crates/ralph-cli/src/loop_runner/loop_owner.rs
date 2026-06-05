use super::*;

pub(crate) fn register_loop_owner(loop_id: &str, config: &RalphConfig, resume: bool) {
    let owner = std::env::var("RALPH_CURRENT_HAT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    register_loop_owner_with_hat(loop_id, config, resume, owner);
}

/// R3 (testable core): the same registration logic as
/// [`register_loop_owner`], but the owner hat id is passed explicitly
/// instead of read from process env. Tests use this variant to avoid
/// mutating process state.
pub(crate) fn register_loop_owner_with_hat(
    loop_id: &str,
    config: &RalphConfig,
    resume: bool,
    owner: Option<String>,
) {
    use ralph_core::loop_registry::LoopEntry;

    if resume {
        // Existing entry remains authoritative during resume.
        return;
    }

    let workspace = &config.core.workspace_root;
    let registry = LoopRegistry::new(workspace);
    // If a stale entry with our PID exists (crash recovery), reuse its
    // owner rather than overwriting it. This keeps ownership consistent
    // across crashes of an agent-owned loop.
    if let Ok(Some(existing)) = registry.get(loop_id) {
        if existing.pid == std::process::id() {
            debug!(
                loop_id = %loop_id,
                "Loop entry already registered for current PID; leaving owner untouched"
            );
            return;
        }
    }

    let prompt = config
        .event_loop
        .prompt
        .clone()
        .or_else(|| {
            if config.event_loop.prompt_file.is_empty() {
                None
            } else {
                Some(config.event_loop.prompt_file.clone())
            }
        })
        .unwrap_or_else(|| "[loop]".to_string());
    let worktree_path = std::env::current_dir()
        .ok()
        .map(|p| p.display().to_string());

    let entry = LoopEntry::with_id(
        loop_id,
        prompt,
        worktree_path,
        workspace.display().to_string(),
    )
    .with_owner_hat(owner.as_deref());

    if let Err(err) = registry.register(entry) {
        warn!(loop_id = %loop_id, error = %err, "Failed to register loop owner metadata");
    }
}
