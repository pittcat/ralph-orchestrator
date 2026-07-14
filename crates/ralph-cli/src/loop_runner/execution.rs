use super::*;

/// Outcome of executing a prompt via PTY or CLI executor.
pub(crate) struct ExecutionOutcome {
    pub output: String,
    pub success: bool,
    pub termination: Option<TerminationReason>,
    /// Whether the backend was terminated by a watchdog timeout
    /// (PTY `IdleTimeout` or `CliExecutor` `timed_out`).
    ///
    /// Unit 3 of plan 2026-06-06-001: watchdog timeout is a backend-call end,
    /// NOT a loop terminate. The runner uses this flag to surface the cause
    /// in logs while still letting `process_output` / `process_events_from_jsonl`
    /// drive the partial-event / missing-event / hard-gate pipeline. If no
    /// partial events arrived, the existing missing-event hard gate / fallback
    /// path will fire on the next iteration.
    ///
    /// # Implementation notes
    ///
    /// The three executor paths compute this flag differently:
    ///
    /// - **PTY path** (`execute_pty`): set when
    ///   `pty_result.termination == TerminationType::IdleTimeout`. The PTY
    ///   executor has no `post_event_timed_out` equivalent — `IdleTimeout` is
    ///   the only watchdog concept on this path.
    /// - **CliExecutor path** (non-PTY branch in `runner.rs`): set when
    ///   `result.timed_out && !result.post_event_timed_out`. `timed_out` is
    ///   the raw inactivity-timeout fire; `post_event_timed_out` is a
    ///   CliExecutor-only "soft wrap-up" signal meaning the backend emitted
    ///   an event and then hung during the post-event grace window. Soft
    ///   wrap-ups are treated as a normal successful backend call
    ///   (`success = true`) and must *not* light up this flag, so the
    ///   `&& !result.post_event_timed_out` guard is load-bearing.
    /// - **ACP path** (`execute_acp`): always `false` — ACP currently has no
    ///   watchdog concept. If one is added, this field must be updated in
    ///   lockstep with the corresponding test in `tests.rs`.
    pub watchdog_timeout: bool,
    pub total_cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}
/// Injects Ralph hat execution context environment variables into a backend.
/// Overwrites any existing Ralph reserved variables.
///
/// `hats_source_label` carries the preset label (e.g. `builtin:ce-executor-pipeline`)
/// that the loop was started with. We propagate it as `RALPH_HATS_SOURCE` so that
/// any in-process CLI invocation (notably `ralph emit` and `ralph wave emit`)
/// inherits the same `event_policy.schemas` the loop runner sees, even when the
/// agent never passes `-H builtin:...`. Plan 001 §4.3 C1.
///
/// When `None`, the function falls back to `RALPH_HATS_SOURCE` from the parent
/// process env so call sites that don't yet thread the explicit label still
/// forward whatever the launcher set. Call sites with the explicit value
/// always win.
///
/// `config_path` carries the resolved project config file path. The retain
/// list strips any stale `RALPH_CONFIG` the parent process may have leaked.
/// 2026-07-13-001 plan U2 + review #C5: align `RALPH_CONFIG` with the
/// `RALPH_HATS_SOURCE` fallback semantics. When the caller has no
/// explicit config path (legacy `execute_wave` wrapper, defaults-only
/// runs, ...), keep the parent env value so the hat subprocess
/// inherits whatever the launcher set. When the caller does pass a
/// path, it always wins.
pub fn inject_hat_execution_env(
    backend: &mut CliBackend,
    current_hat: &str,
    loop_id: &str,
    events_file: &std::path::Path,
    triggered_hat: Option<&str>,
    hats_source_label: Option<&str>,
    config_path: Option<&std::path::Path>,
) {
    let resolved_label = hats_source_label
        .map(|s| s.to_string())
        .or_else(|| std::env::var("RALPH_HATS_SOURCE").ok())
        .filter(|s| !s.is_empty());
    let resolved_config = config_path
        .map(|p| p.display().to_string())
        .or_else(|| {
            std::env::var("RALPH_CONFIG")
                .ok()
                .filter(|v| !v.trim().is_empty())
        });
    backend.env_vars.retain(|(k, _)| {
        !matches!(
            k.as_str(),
            "RALPH_CURRENT_HAT"
                | "RALPH_CURRENT_LOOP_ID"
                | "RALPH_EVENTS_FILE"
                | "RALPH_TRIGGERED_HAT"
                | "RALPH_HATS_SOURCE"
                | "RALPH_CONFIG"
        )
    });
    backend
        .env_vars
        .push(("RALPH_CURRENT_HAT".into(), current_hat.into()));
    backend
        .env_vars
        .push(("RALPH_CURRENT_LOOP_ID".into(), loop_id.into()));
    backend.env_vars.push((
        "RALPH_EVENTS_FILE".into(),
        events_file.display().to_string(),
    ));
    if let Some(triggered) = triggered_hat {
        backend
            .env_vars
            .push(("RALPH_TRIGGERED_HAT".into(), triggered.into()));
    }
    if let Some(label) = resolved_label {
        backend.env_vars.push(("RALPH_HATS_SOURCE".into(), label));
    }
    if let Some(path) = resolved_config {
        backend.env_vars.push(("RALPH_CONFIG".into(), path));
    }
}

pub fn prepare_tui_iteration(
    tui_state: &Arc<std::sync::Mutex<ralph_tui::TuiState>>,
    hat_display: String,
    backend: String,
    max_iterations: u32,
) -> Option<Arc<std::sync::Mutex<Vec<ratatui::text::Line<'static>>>>> {
    let Ok(mut state) = tui_state.lock() else {
        return None;
    };
    // Ensure max_iterations is always available for header display, even if
    // state was reset by earlier events.
    state.max_iterations = Some(max_iterations);
    state.start_new_iteration_with_metadata(Some(hat_display), Some(backend));
    state.latest_iteration_lines_handle()
}

/// Execute a prompt via ACP (Agent Client Protocol) for kiro-acp backend.
pub async fn execute_acp(
    backend: &CliBackend,
    config: &RalphConfig,
    prompt: &str,
    verbosity: Verbosity,
    tui_lines: Option<Arc<std::sync::Mutex<Vec<ratatui::text::Line<'static>>>>>,
    rpc_stdout: Option<Arc<std::sync::Mutex<std::io::Stdout>>>,
    iteration: u32,
    hat: &str,
    backend_name: &str,
) -> Result<ExecutionOutcome> {
    let executor = AcpExecutor::new(backend.clone(), config.core.workspace_root.clone());

    let pty_result = if let Some(lines) = tui_lines {
        let mut handler = TuiStreamHandler::with_lines(verbosity == Verbosity::Verbose, lines);
        executor.execute(prompt, &mut handler).await?
    } else if let Some(stdout_writer) = rpc_stdout {
        let mut handler = JsonRpcStreamHandler::new(
            stdout_writer,
            iteration,
            Some(hat.to_string()),
            Some(backend_name.to_string()),
        );
        executor.execute(prompt, &mut handler).await?
    } else {
        match verbosity {
            Verbosity::Quiet => {
                let mut handler = QuietStreamHandler;
                executor.execute(prompt, &mut handler).await?
            }
            Verbosity::Normal => {
                let mut handler = ConsoleStreamHandler::new(false);
                executor.execute(prompt, &mut handler).await?
            }
            Verbosity::Verbose => {
                let mut handler = ConsoleStreamHandler::new(true);
                executor.execute(prompt, &mut handler).await?
            }
        }
    };

    let output = if pty_result.extracted_text.is_empty() {
        pty_result.stripped_output
    } else {
        pty_result.extracted_text
    };

    Ok(ExecutionOutcome {
        output,
        success: pty_result.success,
        termination: None,
        watchdog_timeout: false,
        total_cost_usd: pty_result.total_cost_usd,
        input_tokens: pty_result.input_tokens,
        output_tokens: pty_result.output_tokens,
        cache_read_tokens: pty_result.cache_read_tokens,
        cache_write_tokens: pty_result.cache_write_tokens,
    })
}

pub async fn execute_pty(
    executor: Option<&mut PtyExecutor>,
    backend: &CliBackend,
    config: &RalphConfig,
    prompt: &str,
    interactive: bool,
    interrupt_rx: tokio::sync::watch::Receiver<bool>,
    verbosity: Verbosity,
    tui_lines: Option<Arc<std::sync::Mutex<Vec<ratatui::text::Line<'static>>>>>,
    rpc_stdout: Option<Arc<std::sync::Mutex<std::io::Stdout>>>,
    iteration: u32,
    hat: &str,
    backend_name: &str,
) -> Result<ExecutionOutcome> {
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    // Use provided executor or create a new one
    // If executor is provided, TUI is connected and owns raw mode management
    let tui_connected = executor.is_some();
    let mut temp_executor;
    let exec = if let Some(e) = executor {
        // Update the executor's backend to use hat-level configuration
        // This is critical for hat-level backend support - without this update,
        // the executor would continue using the global backend it was created with
        e.set_backend(backend.clone());
        let idle_timeout_secs: u64 = if interactive {
            u64::from(config.cli.idle_timeout_secs)
        } else {
            config.autonomous_idle_timeout_secs(backend_name)
        };
        e.set_idle_timeout_secs(u32::try_from(idle_timeout_secs).unwrap_or(u32::MAX));
        e
    } else {
        // Interactive mode uses the user-facing 30s default; autonomous
        // (RPC / worktree / TUI-observation) uses the resolver. The previous
        // hard-coded `0` for the autonomous branch disabled the watchdog
        // entirely and hung the outer loop on silent, non-exiting backends
        // (see pty_executor.rs and plan 2026-06-06-001).
        let idle_timeout_secs: u64 = if interactive {
            u64::from(config.cli.idle_timeout_secs)
        } else {
            config.autonomous_idle_timeout_secs(backend_name)
        };
        let pty_config = PtyConfig {
            interactive,
            idle_timeout_secs: u32::try_from(idle_timeout_secs).unwrap_or(u32::MAX),
            workspace_root: config.core.workspace_root.clone(),
            ..PtyConfig::from_env()
        };
        temp_executor = PtyExecutor::new(backend.clone(), pty_config);
        &mut temp_executor
    };

    // Set TUI mode flag when TUI is connected (tui_lines is Some)
    // This replaces the broken output_rx.is_none() detection in PtyExecutor
    if tui_lines.is_some() {
        exec.set_tui_mode(true);
    }

    // Enter raw mode for interactive mode to capture keystrokes
    // Skip if TUI is connected - TUI owns raw mode and will manage it
    if interactive && !tui_connected {
        enable_raw_mode().context("Failed to enable raw mode")?;
    }

    // Use scopeguard to ensure raw mode is restored on any exit path
    // Skip if TUI is connected - TUI owns raw mode
    let _guard = scopeguard::guard((interactive, tui_connected), |(is_interactive, tui)| {
        if is_interactive && !tui {
            let _ = disable_raw_mode();
        }
    });

    // Run PTY executor with shared interrupt channel
    let result = if interactive && tui_lines.is_none() && rpc_stdout.is_none() {
        // Raw interactive mode only when not using TUI or RPC (TUI/RPC handle their own I/O)
        exec.run_interactive(prompt, interrupt_rx).await
    } else if let Some(lines) = tui_lines {
        // TUI mode: use TuiStreamHandler to capture output for TUI display
        let verbose = verbosity == Verbosity::Verbose;
        let mut handler = TuiStreamHandler::with_lines(verbose, lines);
        exec.run_observe_streaming(prompt, interrupt_rx, &mut handler)
            .await
    } else if let Some(stdout_writer) = rpc_stdout {
        // RPC mode: use JsonRpcStreamHandler for JSON-lines output
        let mut handler = JsonRpcStreamHandler::new(
            stdout_writer,
            iteration,
            Some(hat.to_string()),
            Some(backend_name.to_string()),
        );
        exec.run_observe_streaming(prompt, interrupt_rx, &mut handler)
            .await
    } else {
        // Use streaming handler for non-interactive mode (respects verbosity)
        // Use PrettyStreamHandler for StreamJson backends (Claude) on TTY for markdown rendering
        // Use ConsoleStreamHandler for Text format backends (Kiro, Gemini, etc.) for immediate output
        let use_pretty =
            backend.output_format == BackendOutputFormat::StreamJson && stdout().is_terminal();

        match verbosity {
            Verbosity::Quiet => {
                let mut handler = QuietStreamHandler;
                exec.run_observe_streaming(prompt, interrupt_rx, &mut handler)
                    .await
            }
            Verbosity::Normal => {
                if use_pretty {
                    let mut handler = PrettyStreamHandler::new(false);
                    exec.run_observe_streaming(prompt, interrupt_rx, &mut handler)
                        .await
                } else {
                    let mut handler = ConsoleStreamHandler::new(false);
                    exec.run_observe_streaming(prompt, interrupt_rx, &mut handler)
                        .await
                }
            }
            Verbosity::Verbose => {
                if use_pretty {
                    let mut handler = PrettyStreamHandler::new(true);
                    exec.run_observe_streaming(prompt, interrupt_rx, &mut handler)
                        .await
                } else {
                    let mut handler = ConsoleStreamHandler::new(true);
                    exec.run_observe_streaming(prompt, interrupt_rx, &mut handler)
                        .await
                }
            }
        }
    };

    match result {
        Ok(pty_result) => {
            let watchdog_timeout = matches!(
                pty_result.termination,
                ralph_adapters::TerminationType::IdleTimeout
            );
            let termination = convert_termination_type(pty_result.termination, interactive);

            // Use extracted_text for event parsing when available (NDJSON backends like Claude),
            // otherwise fall back to stripped_output (non-JSON backends or interactive mode).
            // This fixes event parsing for Claude's stream-json output where event tags like
            // <event topic="..."> are inside JSON string values and not directly visible.
            let output_for_parsing = if pty_result.extracted_text.is_empty() {
                pty_result.stripped_output
            } else {
                pty_result.extracted_text
            };
            Ok(ExecutionOutcome {
                output: output_for_parsing,
                success: pty_result.success,
                termination,
                watchdog_timeout,
                total_cost_usd: pty_result.total_cost_usd,
                input_tokens: pty_result.input_tokens,
                output_tokens: pty_result.output_tokens,
                cache_read_tokens: pty_result.cache_read_tokens,
                cache_write_tokens: pty_result.cache_write_tokens,
            })
        }
        Err(e) => {
            // PTY allocation may have failed - log and continue with error
            warn!("PTY execution failed: {}, continuing with error status", e);
            Err(anyhow::Error::new(e))
        }
    }
}
