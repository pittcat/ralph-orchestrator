use super::*;

/// Start a loop from an external caller (e.g., a daemon integration).
///
/// Loads config from `ralph.yml`, applies the given prompt, acquires the
/// loop lock, and runs the orchestration loop headlessly.
///
/// Returns `Ok(TerminationReason)` on completion or `Err` on fatal errors.
pub async fn start_loop(
    prompt: String,
    workspace_root: PathBuf,
    config_path: Option<PathBuf>,
) -> Result<TerminationReason> {
    use crate::{ColorMode, ConfigSource, load_config_with_overrides};

    // Load config from file or defaults
    let config_source = config_path.unwrap_or_else(|| workspace_root.join("ralph.yml"));
    let sources = vec![ConfigSource::File(config_source)];
    let mut config = load_config_with_overrides(&sources)?;

    // Set workspace root to the provided path
    config.core.workspace_root = workspace_root.clone();

    // Apply the prompt
    config.event_loop.prompt = Some(prompt);
    config.event_loop.prompt_file = String::new();

    // Force autonomous headless mode (no TUI, no interactive)
    config.cli.default_mode = "autonomous".to_string();

    // Normalize and validate
    config.normalize();
    let warnings = config
        .validate()
        .context("Configuration validation failed")?;
    for warning in &warnings {
        tracing::warn!("{}", warning);
    }

    // Auto-detect backend if needed
    if config.cli.backend == "auto" {
        let priority = config.get_agent_priority();
        let detected = ralph_adapters::detect_backend(&priority, |backend| {
            config.adapter_settings(backend).enabled
        });
        match detected {
            Ok(backend) => {
                info!("Auto-detected backend: {}", backend);
                config.cli.backend = backend;
            }
            Err(e) => return Err(anyhow::Error::new(e)),
        }
    }

    // Ensure scratchpad directory exists
    crate::ensure_scratchpad_directory(&config)?;

    // Acquire the loop lock (primary loop)
    let prompt_summary = config.event_loop.prompt.as_deref().unwrap_or("[daemon]");
    let prompt_summary = ralph_core::truncate_with_ellipsis(prompt_summary, 100);

    let _lock_guard = ralph_core::LoopLock::try_acquire(&workspace_root, &prompt_summary)
        .context("Failed to acquire loop lock — another loop may be running")?;

    let loop_context = ralph_core::LoopContext::primary(workspace_root);

    // Run the loop headlessly
    run_loop_impl(
        config,
        ColorMode::Never,
        false, // not resume
        false, // no TUI
        false, // no RPC
        Verbosity::Normal,
        None,               // no session recording
        Some(loop_context), // loop context
        Vec::new(),         // no custom args
        None,               // default auto-merge
        None,               // no explicit loop ID
        false,              // warmup_only (daemon mode uses normal flow)
        false,              // force_warmup (daemon mode uses normal flow)
        None,               // U0: no prebuilt diagnostics; EventLoop builds its own
        false,              // no_sync_agent_docs (daemon uses config default)
        false, // source_is_builtin_embedded (daemon re-resolves builtin via its own path)
        None,  // hats_source_label (daemon re-resolves builtin via its own path)
    )
    .await
    .map_err(|e| {
        // B1-边界: `start_loop` is the daemon entry point. Without this
        // mapping, `agent_doc_sync` strict-mode failures and preset-lint
        // errors would surface as plain `anyhow::Error` to the caller
        // and the process would exit 1 — swallowing the 78 / 2 contract
        // that `commands/run.rs` already preserves via
        // `run_loop_result_exit_code`. Doing it here is idempotent
        // because both call sites use `std::process::exit` and the
        // second call is unreachable.
        if let Some(code) = crate::commands::run::run_loop_result_exit_code(&e) {
            std::process::exit(code);
        }
        e
    })
}
