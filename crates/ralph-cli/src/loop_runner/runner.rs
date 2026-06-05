use super::*;

pub struct RpcSharedState {
    iteration: Arc<std::sync::atomic::AtomicU32>,
    /// Current (hat id, hat display name) pair.
    hat: Arc<std::sync::Mutex<(String, String)>>,
    completed: Arc<std::sync::atomic::AtomicBool>,
    total_cost_usd: Arc<std::sync::Mutex<f64>>,
}

/// Resolves the loop ID for task ownership tracking.
///
/// - Worktree loops: use the loop_id from the LoopContext.
/// - Primary loops (fresh): generate a new `primary-{timestamp}` ID.
/// - Primary loops (--continue): reuse the existing `current-loop-id` marker,
///   or use an explicit `--loop-id` if provided.
pub fn resolve_loop_id(
    ctx: &ralph_core::LoopContext,
    resume: bool,
    explicit_loop_id: Option<&str>,
) -> String {
    ctx.loop_id().map(|s| s.to_string()).unwrap_or_else(|| {
        if resume {
            if let Some(explicit_id) = explicit_loop_id {
                return explicit_id.to_string();
            }
            let marker = ctx.ralph_dir().join("current-loop-id");
            if let Ok(existing) = std::fs::read_to_string(&marker) {
                let existing = existing.trim().to_string();
                if !existing.is_empty() {
                    return existing;
                }
            }
        }
        // Fresh run: generate a new timestamped ID
        format!("primary-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
    })
}

/// Core loop implementation supporting both fresh start and continue modes.
///
/// # Arguments
///
/// * `resume` - If true, publishes `task.resume` instead of `task.start`,
///   signaling the planner to read existing scratchpad rather than doing fresh gap analysis.
/// * `record_session` - If provided, records all events to the specified JSONL file for replay testing.
/// * `auto_merge_override` - Explicit auto-merge setting. If `Some(false)`, disables auto-merge
///   (equivalent to `--no-auto-merge`). If `None`, uses `config.features.auto_merge`.
/// * `resume_loop_id` - Explicit loop ID to use when resuming (`--loop-id`).
///   If `None` and `resume` is true, reuses the existing `current-loop-id` marker.
pub async fn run_loop_impl(
    mut config: RalphConfig,
    color_mode: ColorMode,
    resume: bool,
    enable_tui: bool,
    enable_rpc: bool,
    verbosity: Verbosity,
    record_session: Option<PathBuf>,
    loop_context: Option<LoopContext>,
    custom_args: Vec<String>,
    auto_merge_override: Option<bool>,
    resume_loop_id: Option<String>,
    warmup_only: bool,
    force_warmup: bool,
) -> Result<TerminationReason> {
    // U5: Payload contract hard gate. Runs BEFORE any backend is spawned.
    // In strict mode (always on for `ralph run`), any payload contract
    // error is fatal: the agent must not be started. There is no skip flag.
    enforce_payload_contract_gate(&config)?;

    // Set up process group leadership per spec
    // "The orchestrator must run as a process group leader"
    process_management::setup_process_group();

    let use_colors = color_mode.should_use_colors();

    // Determine effective execution mode (with fallback logic)
    // Per spec: Claude backend requires PTY mode to avoid hangs
    // TUI mode is observation-only - uses streaming mode, not interactive
    let interactive_requested = config.cli.default_mode == "interactive" && !enable_tui;
    let user_interactive = if interactive_requested {
        if stdout().is_terminal() {
            true
        } else {
            warn!("Interactive mode requested but stdout is not a TTY, falling back to autonomous");
            false
        }
    } else {
        false
    };
    // PTY is required for TUI/RPC observation and true interactive sessions.
    // Headless `ralph run --no-tui` should use CliExecutor so backends get their
    // non-interactive prompt forms (for example `claude -p` or `codex exec`).
    let use_pty = enable_tui || enable_rpc || user_interactive;

    // Set up interrupt channel for signal handling
    // Per spec:
    // - SIGINT (Ctrl+C): Immediately terminate child process (SIGTERM -> 5s grace -> SIGKILL), exit with code 130
    // - SIGTERM: Same as SIGINT
    // - SIGHUP: Same as SIGINT
    //
    // Use watch channel for interrupt notification so we can race execution vs interrupt
    // Note: Signal handlers are spawned AFTER TUI initialization to avoid deadlock
    let (interrupt_tx, interrupt_rx) = tokio::sync::watch::channel(false);

    // Resolve prompt content with precedence:
    // 1. CLI -p (inline text)
    // 2. CLI -P (file path)
    // 3. Config prompt (inline text)
    // 4. Config prompt_file (file path)
    // 5. Default PROMPT.md
    let prompt_content = resolve_prompt_content(&config.event_loop)?;

    // Create or use provided loop context for path resolution
    // This ensures events are written to the correct location for worktree loops
    let ctx = loop_context
        .clone()
        .unwrap_or_else(|| LoopContext::primary(config.core.workspace_root.clone()));
    let urgent_steer_path = ctx.urgent_steer_path();
    let urgent_steer_store = UrgentSteerStore::new(urgent_steer_path.clone());
    urgent_steer_store
        .clear()
        .context("Failed to clear stale urgent-steer marker")?;
    let _urgent_steer_cleanup = scopeguard::guard(urgent_steer_path.clone(), |path| {
        let _ = UrgentSteerStore::new(path).clear();
    });

    // Write loop ID to marker file for task ownership tracking.
    // For worktree loops, use the loop_id; for primary loops, generate one.
    // This file is read by `ralph tools task add` to tag new tasks.
    //
    // In --continue mode, reuse the existing loop ID so that tasks from the
    // previous run remain visible to `ralph tools task ready`. An explicit
    // --loop-id takes priority over the marker file.
    let loop_id = resolve_loop_id(&ctx, resume, resume_loop_id.as_deref());
    let loop_id_marker = ctx.ralph_dir().join("current-loop-id");
    fs::write(&loop_id_marker, &loop_id).context("Failed to write current-loop-id marker")?;
    debug!(loop_id = %loop_id, marker = ?loop_id_marker, "Wrote loop ID marker file");

    // R3: stamp an owner hat on the registry entry so loop authorization
    // helpers can gate cross-loop operations. Agent-owned loops get the
    // current hat; human CLI invocations stay `None` so any operator can
    // still interact with them.
    register_loop_owner(&loop_id, &config, resume);

    let state_machine_enabled = config
        .event_loop
        .state_machine
        .as_ref()
        .is_some_and(|sm| sm.enabled);

    // For fresh runs (not resume), generate a unique timestamped events file
    // This prevents stale events from previous runs polluting new runs (issue #82)
    // The marker file `.ralph/current-events` coordinates path between Ralph and agents
    if !resume {
        let run_id = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
        // Use relative path in marker file for portability across agents
        // The actual file is at ctx.ralph_dir()/events-{run_id}.jsonl
        let relative_events_path = format!(".ralph/events-{}.jsonl", run_id);

        fs::create_dir_all(ctx.ralph_dir()).context("Failed to create .ralph directory")?;
        fs::write(ctx.current_events_marker(), &relative_events_path)
            .context("Failed to write current-events marker file")?;

        debug!("Created events file for this run: {}", relative_events_path);

        if state_machine_enabled {
            let relative_candidate_events_path =
                format!(".ralph/event-candidates-{}.jsonl", run_id);
            fs::write(
                current_candidate_events_marker(&ctx),
                &relative_candidate_events_path,
            )
            .context("Failed to write current-candidate-events marker file")?;
            debug!(
                "Created candidate events file for this run: {}",
                relative_candidate_events_path
            );
        } else {
            let _ = fs::remove_file(current_candidate_events_marker(&ctx));
        }

        // Clear scratchpads for fresh objective start
        // Stale content from previous runs can confuse the agent about current task state
        // Clear global scratchpad and all per-hat scratchpad overrides
        let mut scratchpad_paths: Vec<PathBuf> =
            vec![ctx.workspace().join(&config.core.scratchpad.path)];
        for hat in config.hats.values() {
            if let Some(ref sc) = hat.scratchpad
                && sc.enabled
            {
                let hat_path = ctx.workspace().join(&sc.path);
                if !scratchpad_paths.contains(&hat_path) {
                    scratchpad_paths.push(hat_path);
                }
            }
        }
        for scratchpad_path in &scratchpad_paths {
            if scratchpad_path.exists() {
                fs::remove_file(scratchpad_path).with_context(|| {
                    format!("Failed to clear scratchpad: {:?}", scratchpad_path)
                })?;
                debug!(
                    "Cleared scratchpad for fresh objective: {:?}",
                    scratchpad_path
                );
            }
        }
    }

    // Initialize event loop with context for proper path resolution
    let mut event_loop = EventLoop::with_context(config.clone(), ctx.clone());
    if state_machine_enabled {
        event_loop.set_event_reader_path(resolve_candidate_events_path(&ctx));
    }

    // Inject robot service (Telegram) for human-in-the-loop communication
    if config.robot.enabled
        && ctx.is_primary()
        && let Some(service) = create_robot_service(&config, &ctx)
    {
        event_loop.set_robot_service(service);
    }

    // Capture the robot service shutdown flag so signal handlers can interrupt wait_for_response()
    let robot_shutdown = event_loop.robot_shutdown_flag();

    let hooks_dispatch_enabled = config.hooks.enabled && !config.hooks.events.is_empty();
    let hook_engine = HookEngine::new(&config.hooks);
    let hook_executor = HookExecutor::new();
    let suspend_state_store = SuspendStateStore::new(ctx.workspace());
    let mut accumulated_hook_metadata = serde_json::Map::new();

    let pre_loop_start_outcomes = dispatch_phase_event_hooks(
        &event_loop,
        hooks_dispatch_enabled,
        &loop_id,
        &hook_engine,
        &hook_executor,
        HookPhaseEvent::PreLoopStart,
        build_loop_start_payload_input(
            &loop_id,
            &ctx,
            config.event_loop.max_iterations,
            event_loop.state().iteration,
            None,
            &accumulated_hook_metadata,
        ),
    );
    merge_accumulated_hook_metadata_from_outcomes(
        &mut accumulated_hook_metadata,
        &pre_loop_start_outcomes,
    );
    fail_if_blocking_loop_start_outcomes(&pre_loop_start_outcomes)?;
    let mut pending_suspend_termination_reason =
        wait_for_resume_if_suspended(&pre_loop_start_outcomes, &loop_id, &suspend_state_store)
            .await?;

    if pending_suspend_termination_reason.is_none() {
        // For resume mode, we initialize with a different event topic
        // This tells the planner to read existing scratchpad rather than creating a new one
        if resume {
            event_loop.initialize_resume(&prompt_content);
        } else {
            event_loop.initialize(&prompt_content);
        }

        let post_loop_start_outcomes = dispatch_phase_event_hooks(
            &event_loop,
            hooks_dispatch_enabled,
            &loop_id,
            &hook_engine,
            &hook_executor,
            HookPhaseEvent::PostLoopStart,
            build_loop_start_payload_input(
                &loop_id,
                &ctx,
                config.event_loop.max_iterations,
                event_loop.state().iteration,
                Some(event_loop.get_active_hat_id().as_str().to_string()),
                &accumulated_hook_metadata,
            ),
        );
        merge_accumulated_hook_metadata_from_outcomes(
            &mut accumulated_hook_metadata,
            &post_loop_start_outcomes,
        );
        fail_if_blocking_loop_start_outcomes(&post_loop_start_outcomes)?;
        pending_suspend_termination_reason =
            wait_for_resume_if_suspended(&post_loop_start_outcomes, &loop_id, &suspend_state_store)
                .await?;
    }

    // Set up session recording if requested
    // This records all events to a JSONL file for replay testing
    let _session_recorder: Option<Arc<SessionRecorder<BufWriter<File>>>> =
        if let Some(record_path) = record_session {
            let file = File::create(&record_path).with_context(|| {
                format!("Failed to create session recording file: {:?}", record_path)
            })?;
            let recorder = Arc::new(SessionRecorder::new(BufWriter::new(file)));

            // Record metadata for the session
            recorder.record_meta(Record::meta_loop_start(
                &config.event_loop.prompt_file,
                config.event_loop.max_iterations,
                if enable_tui { Some("tui") } else { Some("cli") },
            ));

            // Wire observer to EventBus so events are recorded
            let observer = SessionRecorder::make_observer(Arc::clone(&recorder));
            event_loop.add_observer(observer);

            info!("Session recording enabled: {:?}", record_path);
            Some(recorder)
        } else {
            None
        };

    // ── Phase Initialization (Warmup/Production Two-Phase Loop) ───────────────
    // Determine starting phase based on CLI flags and phase.json state
    let phase_json_path = ctx.ralph_dir().join("agent").join("phase.json");
    let current_phase = if force_warmup {
        info!("Force warmup enabled — starting in warmup phase");
        Phase::Warmup
    } else if phase_json_path.exists() {
        // Read existing phase.json to check warmup_completed marker
        match fs::read_to_string(&phase_json_path) {
            Ok(content) => {
                // Parse phase.json to check warmup_completed field
                if let Ok(phase_data) = serde_json::from_str::<serde_json::Value>(&content) {
                    let warmup_completed = phase_data
                        .get("warmup_completed")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if warmup_completed && !warmup_only {
                        info!("Warmup previously completed — skipping to production phase");
                        Phase::Production
                    } else {
                        Phase::Warmup
                    }
                } else {
                    Phase::Warmup
                }
            }
            Err(_) => Phase::Warmup,
        }
    } else {
        // No phase.json exists — start in warmup phase
        Phase::Warmup
    };

    // Set phase on registry for phase-aware hat triggering
    event_loop.registry_mut().set_phase(current_phase.clone());

    // Apply warmup_only / stop_on_exit from CLI flag to config
    // WarmupConfig is nested under phase_config, not directly on EventLoopConfig
    let stop_on_exit = if warmup_only {
        info!("Warmup-only mode enabled — loop will exit after warmup completes");
        // Ensure phase_config exists with warmup_config
        if config.event_loop.phase_config.is_none() {
            config.event_loop.phase_config = Some(PhaseConfig {
                initial: Phase::Warmup,
                transition_event: "phase.transition".to_string(),
                warmup_config: None,
            });
        }
        if let Some(ref mut phase_config) = config.event_loop.phase_config {
            if phase_config.warmup_config.is_none() {
                phase_config.warmup_config = Some(WarmupConfig::default());
            }
            if let Some(ref mut warmup) = phase_config.warmup_config {
                warmup.stop_on_exit = true;
            }
        }
        true
    } else {
        config
            .event_loop
            .phase_config
            .as_ref()
            .and_then(|p| p.warmup_config.as_ref())
            .map(|w| w.stop_on_exit)
            .unwrap_or(false)
    };

    // Initialize event logger for history/observability (uses context for path resolution).
    // This writes to the history file, NOT the trusted events file consumed by EventReader.
    // Raw output parsing, orphan events, and terminate events all go here.
    let mut event_logger = EventLogger::history_from_context(&ctx);

    // Log initial event (use configured starting_event or default to task.start/task.resume)
    let default_start_topic = if resume { "task.resume" } else { "task.start" };
    let start_topic = config
        .event_loop
        .starting_event
        .as_deref()
        .unwrap_or(default_start_topic);
    let start_triggered = "planner"; // Default triggered hat for backward compat
    let start_event = Event::new(start_topic, &prompt_content);
    let start_record = EventRecord::new(
        0,
        "loop",
        &start_event,
        Some(&HatId::new(start_triggered)),
        Some(current_phase.to_string()),
    );
    if let Err(e) = event_logger.log(&start_record) {
        warn!("Failed to log start event: {}", e);
    }
    // NOTE: No sync_event_reader_to_file_end() needed here because the history
    // logger writes to a separate file from the trusted events file consumed
    // by EventReader. The start event only appears in history, not in the
    // trusted event stream.

    // Create backend from config - TUI mode uses the same backend as non-TUI
    // The TUI is an observation layer that displays output, not a different mode
    let mut backend = CliBackend::from_config(&config.cli).map_err(|e| anyhow::Error::new(e))?;

    // Append custom args from CLI if provided (e.g., `ralph run -b opencode -- --model="some-model"`)
    if !custom_args.is_empty() {
        backend.args.extend(custom_args);
    }

    // Create PTY executor if using interactive mode
    let mut pty_executor = if use_pty {
        // The watchdog value in seconds. Interactive mode uses the user-facing
        // 30s default; autonomous / RPC / worktree mode uses the resolver
        // below (explicit override or per-adapter timeout, default 300s).
        // Hard-coding 0 for autonomous used to silently disable the watchdog
        // and hang the outer loop on a silent, non-exiting backend — see
        // pty_executor.rs and plan 2026-06-06-001.
        let idle_timeout_secs: u64 = if user_interactive {
            u64::from(config.cli.idle_timeout_secs)
        } else {
            config.autonomous_idle_timeout_secs(&config.cli.backend)
        };
        // In autonomous (non-interactive) mode, use a very wide PTY to prevent
        // line wrapping of long NDJSON output (Pi emits 800+ char JSON lines that
        // get garbled when the PTY wraps at 80 columns).
        let cols = if user_interactive {
            PtyConfig::from_env().cols
        } else {
            32768
        };
        // The watchdog u64 is bounded to u32::MAX so PtyConfig's u32 field
        // can hold it without silent truncation; the realistic value (300s
        // default) fits trivially.
        let pty_config = PtyConfig {
            interactive: user_interactive,
            idle_timeout_secs: u32::try_from(idle_timeout_secs).unwrap_or(u32::MAX),
            cols,
            workspace_root: config.core.workspace_root.clone(),
            ..PtyConfig::from_env()
        };
        Some(PtyExecutor::new(backend.clone(), pty_config))
    } else {
        None
    };

    // Create termination signal for TUI shutdown
    let (terminated_tx, terminated_rx) = tokio::sync::watch::channel(false);

    // Wire TUI with termination signal and shared state
    // TUI is observation-only - works in both interactive and autonomous modes
    // Requirements: both stdin and stdout must be terminals for TUI
    // (Crossterm requires stdin for keyboard input, stdout for rendering)
    let enable_tui = enable_tui && !enable_rpc && stdin().is_terminal() && stdout().is_terminal();

    // RPC mode state: channels for stdin commands and stdout events
    let (rpc_event_tx, rpc_event_rx) = if enable_rpc {
        let (tx, rx) = tokio::sync::mpsc::channel::<RpcEvent>(256);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let (rpc_guidance_tx, mut rpc_guidance_rx) = if enable_rpc {
        let (tx, rx) = tokio::sync::mpsc::channel::<GuidanceMessage>(64);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    // Shared stdout writer for RPC mode (thread-safe for JsonRpcStreamHandler)
    let rpc_stdout: Option<Arc<std::sync::Mutex<std::io::Stdout>>> = if enable_rpc {
        Some(Arc::new(std::sync::Mutex::new(std::io::stdout())))
    } else {
        None
    };

    // RPC mode: spawn stdin reader and stdout emitter tasks
    let rpc_dispatcher_started = if enable_rpc {
        let backend_name = config.cli.backend.clone();
        let max_iters = config.event_loop.max_iterations;

        // Create shared state for get_state responses
        let rpc_state_iteration = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let rpc_state_hat: Arc<std::sync::Mutex<(String, String)>> = Arc::new(
            std::sync::Mutex::new(("unknown".to_string(), "Unknown".to_string())),
        );
        let rpc_state_completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let rpc_state_total_cost: Arc<std::sync::Mutex<f64>> = Arc::new(std::sync::Mutex::new(0.0));

        let rpc_state_iteration_clone = rpc_state_iteration.clone();
        let rpc_state_hat_clone = rpc_state_hat.clone();
        let rpc_state_completed_clone = rpc_state_completed.clone();
        let rpc_state_total_cost_clone = rpc_state_total_cost.clone();
        let rpc_state_started_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let state_fn = move || {
            let (hat, hat_display) = rpc_state_hat_clone
                .lock()
                .map(|g| g.clone())
                .unwrap_or_else(|_| ("unknown".to_string(), "Unknown".to_string()));
            let total_cost_usd = rpc_state_total_cost_clone.lock().map(|g| *g).unwrap_or(0.0);
            RpcState {
                iteration: rpc_state_iteration_clone.load(std::sync::atomic::Ordering::Relaxed),
                max_iterations: Some(max_iters),
                hat,
                hat_display,
                backend: backend_name.clone(),
                completed: rpc_state_completed_clone.load(std::sync::atomic::Ordering::Relaxed),
                started_at: rpc_state_started_at,
                iteration_started_at: None,
                task_counts: RpcTaskCounts::default(),
                active_task: None,
                total_cost_usd,
            }
        };

        let dispatcher = RpcDispatcher::new(
            interrupt_tx.clone(),
            rpc_guidance_tx
                .clone()
                .expect("RPC guidance tx should exist"),
            rpc_event_tx.clone().expect("RPC event tx should exist"),
            Some(urgent_steer_path.clone()),
            state_fn,
        );

        // Mark loop as started
        dispatcher.mark_loop_started();

        // Spawn stdin reader
        tokio::spawn(async move {
            run_stdin_reader(dispatcher, tokio::io::stdin()).await;
        });

        // Spawn stdout emitter
        let rx = rpc_event_rx.expect("RPC event rx should exist");
        tokio::spawn(async move {
            run_stdout_emitter(rx).await;
        });

        // Emit loop_started event
        if let Some(ref tx) = rpc_event_tx {
            let started_event = RpcEvent::LoopStarted {
                prompt: prompt_content.clone(),
                max_iterations: Some(config.event_loop.max_iterations),
                backend: config.cli.backend.clone(),
                started_at: rpc_state_started_at,
            };
            let _ = tx.try_send(started_event);
        }

        Some(RpcSharedState {
            iteration: rpc_state_iteration,
            hat: rpc_state_hat,
            completed: rpc_state_completed,
            total_cost_usd: rpc_state_total_cost,
        })
    } else {
        None
    };

    let (mut tui_handle, tui_state, guidance_next_queue) = if enable_tui {
        // Build hat map for dynamic topic-to-hat resolution
        // This allows TUI to display custom hats (e.g., "Security Reviewer")
        // instead of generic "ralph" for all events
        let hat_map = build_tui_hat_map(event_loop.registry());
        let tui = Tui::new()
            .with_hat_map(hat_map)
            .with_termination_signal(terminated_rx)
            .with_events_path(resolve_current_events_path(&ctx))
            .with_urgent_steer_path(urgent_steer_path.clone());

        // Get shared state and guidance queue before spawning (for content streaming)
        let state = tui.state();
        let guidance_queue = tui.guidance_next_queue();

        // Wire interrupt channel so TUI can signal main loop on Ctrl+C
        // (raw mode prevents SIGINT from being generated by the OS)
        let tui = tui.with_interrupt_tx(interrupt_tx.clone());

        let observer = tui.observer();
        event_loop.add_observer(observer);
        (
            Some(tokio::spawn(async move { tui.run().await })),
            Some(state),
            Some(guidance_queue),
        )
    } else {
        (None, None, None)
    };

    // Add RPC EventBus observer to map ralph_proto::Event topics to RpcEvent variants
    // Per Task 04 requirement #4: "Add an EventBus observer that serializes Event → RpcEvent"
    if let Some(ref tx) = rpc_event_tx {
        let tx_clone = tx.clone();
        event_loop.add_observer(move |event: &Event| {
            // Map all event topics to RpcEvent::OrchestrationEvent
            // This provides observability for: build.task, build.done, loop.terminate,
            // task.start, task.resume, and any custom hat events
            let rpc_event = RpcEvent::OrchestrationEvent {
                topic: event.topic.as_str().to_string(),
                payload: event.payload.clone(),
                source: event.source.as_ref().map(|h| h.as_str().to_string()),
                target: event.target.as_ref().map(|h| h.as_str().to_string()),
            };
            let _ = tx_clone.try_send(rpc_event);
        });
    }

    // Give TUI task time to initialize (enter alternate screen, enable raw mode)
    // before the main loop starts doing work
    if tui_handle.is_some() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Seed max_iterations into TUI state for accurate iteration display.
    if let Some(mut s) = tui_state.as_ref().and_then(|state| state.lock().ok()) {
        s.max_iterations = Some(config.event_loop.max_iterations);
    }

    // Spawn signal handlers AFTER TUI initialization to avoid deadlock
    // (TUI must enter raw mode and create EventStream before signal handlers are registered)

    // Spawn task to listen for SIGINT (Ctrl+C)
    let interrupt_tx_sigint = interrupt_tx.clone();
    let robot_shutdown_sigint = robot_shutdown.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            debug!("Interrupt received (SIGINT), terminating immediately...");
            if let Some(ref flag) = robot_shutdown_sigint {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let _ = interrupt_tx_sigint.send(true);
        }
    });

    // Spawn task to listen for SIGTERM (Unix only)
    #[cfg(unix)]
    {
        let interrupt_tx_sigterm = interrupt_tx.clone();
        let robot_shutdown_sigterm = robot_shutdown.clone();
        tokio::spawn(async move {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("Failed to register SIGTERM handler");
            sigterm.recv().await;
            debug!("SIGTERM received, terminating immediately...");
            if let Some(ref flag) = robot_shutdown_sigterm {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let _ = interrupt_tx_sigterm.send(true);
        });
    }

    // Spawn task to listen for SIGHUP (Unix only)
    #[cfg(unix)]
    {
        let interrupt_tx_sighup = interrupt_tx.clone();
        let robot_shutdown_sighup = robot_shutdown.clone();
        tokio::spawn(async move {
            let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                .expect("Failed to register SIGHUP handler");
            sighup.recv().await;
            warn!("SIGHUP received (terminal closed), terminating immediately...");
            if let Some(ref flag) = robot_shutdown_sighup {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let _ = interrupt_tx_sighup.send(true);
        });
    }

    // Log execution mode - hat info already logged by initialize()
    let exec_mode = if user_interactive {
        "interactive"
    } else {
        "autonomous"
    };
    debug!(execution_mode = %exec_mode, "Execution mode configured");

    // Track the last hat to detect hat changes for logging
    let mut last_hat: Option<HatId> = None;

    // Track consecutive fallback attempts to prevent infinite loops
    let mut consecutive_fallbacks: u32 = 0;
    const MAX_FALLBACK_ATTEMPTS: u32 = 3;

    // Initialize loop history if we have a loop context
    let loop_history = loop_context
        .as_ref()
        .map(|ctx| LoopHistory::from_context(ctx));

    // Record loop start in history
    if let Some(ref history) = loop_history
        && let Err(e) = history.record_started(&prompt_content)
    {
        warn!("Failed to record loop start in history: {}", e);
    }

    // Auto-merge setting: CLI override > config > default (false for safety)
    let auto_merge = auto_merge_override.unwrap_or(config.features.auto_merge);

    // Detect merge loop on startup via RALPH_MERGE_LOOP_ID env var
    // Per spec: If set, mark entry as "merging" with current PID
    let merge_loop_id: Option<String> = std::env::var("RALPH_MERGE_LOOP_ID").ok();
    if let Some(ref loop_id) = merge_loop_id {
        let repo_root = loop_context
            .as_ref()
            .map(|ctx| ctx.repo_root().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let queue = MergeQueue::new(&repo_root);
        let pid = std::process::id();

        match queue.mark_merging(loop_id, pid) {
            Ok(()) => {
                info!(loop_id = %loop_id, pid = pid, "Merge loop started, marked as merging");
            }
            Err(ralph_core::MergeQueueError::NotFound(_)) => {
                warn!(loop_id = %loop_id, "Merge loop started but no queue entry found");
            }
            Err(ralph_core::MergeQueueError::InvalidTransition(_, from, _)) => {
                // Entry is already merging/merged/discarded, skip update
                debug!(loop_id = %loop_id, state = ?from, "Merge queue entry already in terminal state, skipping");
            }
            Err(e) => {
                warn!(loop_id = %loop_id, error = %e, "Failed to mark merge loop as merging");
            }
        }
    }

    // Record base commit at loop start for accurate handoff scope (base..HEAD)
    let base_commit = ralph_core::get_head_sha(&ctx.workspace()).ok();

    // Record the same baseline in the event loop state so execution-contract
    // validation can detect commits produced during this loop. Without this,
    // `diff_or_commit` cannot distinguish "loop produced a new commit" from
    // "the repository merely has commits from prior history".
    event_loop.set_loop_start_sha(base_commit.clone());

    // Helper closure to handle termination (writes summary, prints status, records history)
    let handle_termination = |reason: &TerminationReason,
                              state: &ralph_core::LoopState,
                              scratchpad: &str,
                              history: &Option<LoopHistory>,
                              context: &Option<LoopContext>,
                              auto_merge: bool,
                              prompt: &str| {
        // Per spec: Write summary file on termination
        let summary_writer = if let Some(ctx) = context {
            SummaryWriter::from_context(ctx)
        } else {
            SummaryWriter::default()
        };
        let scratchpad_path = if let Some(ctx) = context {
            ctx.scratchpad_path()
        } else {
            PathBuf::from(scratchpad)
        };
        let scratchpad_opt = if scratchpad_path.exists() {
            Some(scratchpad_path.as_path())
        } else {
            None
        };

        // Get final commit SHA if available
        let final_commit = get_last_commit_info();

        if let Err(e) = summary_writer.write(reason, state, scratchpad_opt, final_commit.as_deref())
        {
            warn!("Failed to write summary file: {}", e);
        }

        // Record termination in history
        if let Some(hist) = history {
            let reason_str = match reason {
                TerminationReason::CompletionPromise => "completion_promise",
                TerminationReason::MaxIterations => "max_iterations",
                TerminationReason::MaxRuntime => "max_runtime",
                TerminationReason::MaxCost => "max_cost",
                TerminationReason::ConsecutiveFailures => "consecutive_failures",
                TerminationReason::LoopThrashing => "loop_thrashing",
                TerminationReason::LoopStale => "loop_stale",
                TerminationReason::ValidationFailure => "validation_failure",
                TerminationReason::Stopped => "stopped",
                TerminationReason::Interrupted => "interrupted",
                TerminationReason::RestartRequested => "restart_requested",
                TerminationReason::WorkspaceGone => "workspace_gone",
                TerminationReason::Cancelled => "cancelled",
                TerminationReason::PayloadContractViolation => "payload_contract_violation",
            };

            if matches!(reason, TerminationReason::Interrupted) {
                if let Err(e) = hist.record_terminated("SIGTERM") {
                    warn!("Failed to record termination in history: {}", e);
                }
            } else if let Err(e) = hist.record_completed(reason_str) {
                warn!("Failed to record completion in history: {}", e);
            }
        }

        // Handle merge queue state transitions for merge loops
        // Per spec: CompletionPromise → merged, other → needs-review
        if let Some(ref loop_id) = merge_loop_id {
            let repo_root = context
                .as_ref()
                .map(|ctx| ctx.repo_root().to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            let queue = MergeQueue::new(&repo_root);

            if matches!(reason, TerminationReason::CompletionPromise) {
                // Get commit SHA from git rev-parse HEAD
                let commit = Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .ok()
                    .and_then(|output| {
                        if output.status.success() {
                            String::from_utf8(output.stdout)
                                .ok()
                                .map(|s| s.trim().to_string())
                        } else {
                            None
                        }
                    });

                match commit {
                    Some(sha) => {
                        if let Err(e) = queue.mark_merged(loop_id, &sha) {
                            warn!(loop_id = %loop_id, error = %e, "Failed to mark merge as completed");
                        } else {
                            info!(loop_id = %loop_id, commit = %sha, "Merge completed successfully");
                        }
                    }
                    None => {
                        // Per spec: "If commit SHA cannot be resolved, mark as needs-review"
                        if let Err(e) =
                            queue.mark_needs_review(loop_id, "merge complete but commit not found")
                        {
                            warn!(loop_id = %loop_id, error = %e, "Failed to mark merge as needs-review");
                        } else {
                            warn!(loop_id = %loop_id, "Merge completed but could not resolve commit SHA");
                        }
                    }
                }
            } else {
                // Any non-CompletionPromise termination → needs-review
                let reason_str = match reason {
                    TerminationReason::MaxIterations => "max iterations reached",
                    TerminationReason::MaxRuntime => "max runtime exceeded",
                    TerminationReason::MaxCost => "max cost exceeded",
                    TerminationReason::ConsecutiveFailures => "consecutive failures",
                    TerminationReason::LoopThrashing => "loop thrashing detected",
                    TerminationReason::LoopStale => "stale loop detected",
                    TerminationReason::ValidationFailure => "validation failure",
                    TerminationReason::Stopped => "manually stopped",
                    TerminationReason::Interrupted => "interrupted by signal",
                    TerminationReason::CompletionPromise => unreachable!(),
                    TerminationReason::RestartRequested => "restart requested",
                    TerminationReason::WorkspaceGone => "workspace directory removed",
                    TerminationReason::Cancelled => "cancelled by human",
                    TerminationReason::PayloadContractViolation => "payload contract violation",
                };
                if let Err(e) = queue.mark_needs_review(loop_id, reason_str) {
                    warn!(loop_id = %loop_id, error = %e, "Failed to mark merge as needs-review");
                } else {
                    info!(loop_id = %loop_id, reason = reason_str, "Merge marked as needs-review");
                }
            }
        }

        // Handle completion for all loops (landing + merge queue for worktrees)
        // Per spec: merge loops do NOT enqueue themselves, even if run in worktree context
        if let Some(ctx) = context {
            if merge_loop_id.is_none() && matches!(reason, TerminationReason::CompletionPromise) {
                let handler = LoopCompletionHandler::new(auto_merge);
                match handler.handle_completion(ctx, prompt, base_commit.as_deref()) {
                    Ok(CompletionAction::None) => {
                        debug!("Loop completed, no action needed");
                    }
                    Ok(CompletionAction::Landed { landing }) => {
                        info!(
                            committed = landing.committed,
                            handoff = %landing.handoff_path,
                            open_tasks = landing.open_task_count,
                            "Primary loop landed successfully"
                        );
                    }
                    Ok(CompletionAction::Enqueued { loop_id, landing }) => {
                        info!(loop_id = %loop_id, "Loop queued for auto-merge");
                        if let Some(ref l) = landing {
                            debug!(
                                committed = l.committed,
                                handoff = %l.handoff_path,
                                "Landing completed before enqueue"
                            );
                        }
                        if let Some(hist) = history {
                            let _ = hist.record_merge_queued();
                        }
                        // Worktree loop exits cleanly; merge will be processed
                        // when the primary loop completes and checks the queue
                    }
                    Ok(CompletionAction::ManualMerge {
                        loop_id,
                        worktree_path,
                        landing,
                    }) => {
                        info!(
                            loop_id = %loop_id,
                            "Loop completed. To merge manually: cd {} && git merge",
                            worktree_path
                        );
                        if let Some(ref l) = landing {
                            debug!(
                                committed = l.committed,
                                handoff = %l.handoff_path,
                                "Landing completed (manual merge mode)"
                            );
                        }
                    }
                    Err(e) => {
                        warn!("Completion handler failed: {}", e);
                    }
                }
            }

            // Handle merge queue processing for primary loop completion
            if ctx.is_primary() && matches!(reason, TerminationReason::CompletionPromise) {
                process_pending_merges(ctx.repo_root());
            }

            // Always deregister from registry — process is exiting regardless of reason.
            // CompletionPromise loops are tracked by the merge queue from here on.
            let registry = LoopRegistry::new(ctx.repo_root());
            if let Err(e) = registry.deregister_current_process() {
                warn!("Failed to deregister loop from registry: {}", e);
            }
        }

        // Print termination info to console (skip in TUI mode - TUI handles display)
        // Skip in RPC mode - JSON events replace console output
        if !enable_tui && !enable_rpc {
            print_termination(reason, state, use_colors, Some(&loop_id));
        }

        // Mark RPC state as completed so get_state reflects termination
        if let Some(ref shared) = rpc_dispatcher_started {
            shared
                .completed
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }

        // Emit RPC loop_terminated event
        if let Some(ref tx) = rpc_event_tx {
            let terminated_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            let rpc_reason = match reason {
                TerminationReason::CompletionPromise => {
                    ralph_proto::json_rpc::TerminationReason::Completed
                }
                TerminationReason::MaxIterations => {
                    ralph_proto::json_rpc::TerminationReason::MaxIterations
                }
                TerminationReason::Interrupted | TerminationReason::Stopped => {
                    ralph_proto::json_rpc::TerminationReason::Interrupted
                }
                _ => ralph_proto::json_rpc::TerminationReason::Error,
            };

            let accumulated_cost = rpc_dispatcher_started
                .as_ref()
                .and_then(|s| s.total_cost_usd.lock().ok().map(|g| *g))
                .unwrap_or(0.0);

            let terminate_event = RpcEvent::LoopTerminated {
                reason: rpc_reason,
                total_iterations: state.iteration,
                duration_ms: state.elapsed().as_millis() as u64,
                total_cost_usd: accumulated_cost,
                terminated_at,
            };
            let _ = tx.try_send(terminate_event);
        }
    };

    if let Some(reason) = pending_suspend_termination_reason.take() {
        let reason = dispatch_pre_loop_termination_hooks(
            &event_loop,
            hooks_dispatch_enabled,
            &loop_id,
            &hook_engine,
            &hook_executor,
            &suspend_state_store,
            &ctx,
            config.event_loop.max_iterations,
            &mut accumulated_hook_metadata,
            reason,
        )
        .await?;

        let terminate_event = event_loop.publish_terminate_event(&reason);
        log_terminate_event(
            &mut event_logger,
            event_loop.state().iteration,
            &terminate_event,
            Some(event_loop.registry().current_phase().to_string()),
        );

        let reason = dispatch_post_loop_termination_hooks(
            &event_loop,
            hooks_dispatch_enabled,
            &loop_id,
            &hook_engine,
            &hook_executor,
            &suspend_state_store,
            &ctx,
            config.event_loop.max_iterations,
            &mut accumulated_hook_metadata,
            reason,
        )
        .await?;

        handle_termination(
            &reason,
            event_loop.state(),
            &config.core.scratchpad.path,
            &loop_history,
            &loop_context,
            auto_merge,
            &prompt_content,
        );

        // Wait for user to exit TUI (press 'q') on natural completion
        if let Some(handle) = tui_handle.take() {
            let _ = handle.await;
        }

        return Ok(reason);
    }

    // Print startup banner for --no-tui runs. Gives agents/humans tailing
    // the stream the loop-id, key state files, and tail/resume commands up
    // front so they don't have to reverse-engineer them from scrollback.
    if !enable_tui && !enable_rpc {
        let events_path = resolve_current_events_path(&ctx);
        let scratchpad_path = ctx.workspace().join(&config.core.scratchpad.path);
        print_loop_banner(
            &loop_id,
            &config.cli.backend,
            std::path::Path::new(&config.event_loop.prompt_file),
            &events_path,
            &scratchpad_path,
            config.event_loop.max_iterations,
            resume,
            use_colors,
        );
    }

    // Main orchestration loop
    loop {
        // Check for interrupt signal at start of each iteration
        // This catches TUI Ctrl+C (via interrupt_tx) before printing iteration separator
        if *interrupt_rx.borrow() {
            #[cfg(unix)]
            {
                use nix::sys::signal::{Signal, killpg};
                use nix::unistd::getpgrp;
                let pgid = getpgrp();
                debug!(
                    "Interrupt detected at loop start, sending SIGTERM to process group {}",
                    pgid
                );
                let _ = killpg(pgid, Signal::SIGTERM);
                tokio::time::sleep(Duration::from_millis(250)).await;
                let _ = killpg(pgid, Signal::SIGKILL);
            }
            let reason = dispatch_pre_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                TerminationReason::Interrupted,
            )
            .await?;

            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );

            let reason = dispatch_post_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
            );
            // Signal TUI to exit immediately on interrupt
            let _ = terminated_tx.send(true);
            return Ok(reason);
        }

        // Drain next-loop guidance queue and write as human.guidance events.
        // These will be picked up by process_events_from_jsonl() during build_prompt().
        // Handle both TUI guidance queue and RPC guidance channel.
        let mut guidance_messages: Vec<String> = Vec::new();

        // Drain TUI guidance queue
        if let Some(ref queue) = guidance_next_queue {
            let messages: Vec<String> = {
                let mut q = queue.lock().unwrap();
                q.drain(..).collect()
            };
            guidance_messages.extend(messages);
        }

        // Drain RPC guidance channel (non-blocking)
        if let Some(ref mut rx) = rpc_guidance_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg.target {
                    GuidanceTarget::Current => {
                        debug!("Received RPC steer(current); applying at next prompt boundary");
                        guidance_messages.push(msg.message);
                    }
                    GuidanceTarget::Next => guidance_messages.push(msg.message),
                }
            }
        }

        if !guidance_messages.is_empty() {
            let events_path = resolve_current_events_path(&ctx);

            use std::io::Write;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path);

            let mut writer = match file {
                Ok(f) => std::io::BufWriter::new(f),
                Err(e) => {
                    warn!(error = %e, path = ?events_path, "Failed to open events file for guidance flush");
                    // Skip flushing - keep loop running
                    continue;
                }
            };

            for msg in &guidance_messages {
                let timestamp = chrono::Utc::now().to_rfc3339();
                let event = serde_json::json!({
                    "topic": "human.guidance",
                    "payload": msg,
                    "ts": timestamp,
                });

                match serde_json::to_string(&event) {
                    Ok(line) => {
                        if writeln!(writer, "{}", line).is_err() {
                            warn!(path = ?events_path, "Failed writing guidance event line");
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed serializing guidance event");
                    }
                }
            }
            info!(
                count = guidance_messages.len(),
                "Wrote guidance events to events.jsonl"
            );
        }

        // Check termination before execution
        if let Some(reason) = event_loop.check_termination() {
            let reason = dispatch_pre_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            // Per spec: Publish loop.terminate event to observers
            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );

            let reason = dispatch_post_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
            );
            // Wait for user to exit TUI (press 'q') on natural completion
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            return Ok(reason);
        }

        let iteration = event_loop.state().iteration + 1;

        if event_loop.has_pending_events() {
            let pre_iteration_start_outcomes = dispatch_phase_event_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                HookPhaseEvent::PreIterationStart,
                build_iteration_start_payload_input(
                    &loop_id,
                    &ctx,
                    config.event_loop.max_iterations,
                    iteration,
                    Some(event_loop.get_active_hat_id().as_str().to_string()),
                    None,
                    None,
                    &accumulated_hook_metadata,
                ),
            );
            merge_accumulated_hook_metadata_from_outcomes(
                &mut accumulated_hook_metadata,
                &pre_iteration_start_outcomes,
            );
            fail_if_blocking_iteration_start_outcomes(&pre_iteration_start_outcomes)?;

            if let Some(reason) = wait_for_resume_if_suspended(
                &pre_iteration_start_outcomes,
                &loop_id,
                &suspend_state_store,
            )
            .await?
            {
                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                );
                // Wait for user to exit TUI (press 'q') on natural completion
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                return Ok(reason);
            }
        }

        // Get next hat to execute, with fallback recovery if no pending events
        let hat_id = match event_loop.next_hat() {
            Some(id) => {
                // Reset fallback counter on successful event routing
                consecutive_fallbacks = 0;
                id.clone()
            }
            None => {
                match recover_late_events_before_fallback(&mut event_loop)
                    .inspect_err(
                        |e| warn!(error = %e, "Failed to drain late JSONL events before fallback"),
                    )
                    .ok()
                {
                    Some(LateEventRecovery::PendingWork) => {
                        debug!(
                            "Recovered late JSONL events before fallback; retrying hat selection"
                        );
                        consecutive_fallbacks = 0;
                        continue;
                    }
                    Some(LateEventRecovery::Terminate(reason)) => {
                        let reason = dispatch_pre_loop_termination_hooks(
                            &event_loop,
                            hooks_dispatch_enabled,
                            &loop_id,
                            &hook_engine,
                            &hook_executor,
                            &suspend_state_store,
                            &ctx,
                            config.event_loop.max_iterations,
                            &mut accumulated_hook_metadata,
                            reason,
                        )
                        .await?;

                        let terminate_event = event_loop.publish_terminate_event(&reason);
                        log_terminate_event(
                            &mut event_logger,
                            event_loop.state().iteration,
                            &terminate_event,
                            Some(event_loop.registry().current_phase().to_string()),
                        );

                        let reason = dispatch_post_loop_termination_hooks(
                            &event_loop,
                            hooks_dispatch_enabled,
                            &loop_id,
                            &hook_engine,
                            &hook_executor,
                            &suspend_state_store,
                            &ctx,
                            config.event_loop.max_iterations,
                            &mut accumulated_hook_metadata,
                            reason,
                        )
                        .await?;

                        handle_termination(
                            &reason,
                            event_loop.state(),
                            &config.core.scratchpad.path,
                            &loop_history,
                            &loop_context,
                            auto_merge,
                            &prompt_content,
                        );
                        if let Some(handle) = tui_handle.take() {
                            let _ = handle.await;
                        }
                        return Ok(reason);
                    }
                    Some(LateEventRecovery::NoLateEvents) | None => {}
                }

                // No pending events - try to recover by injecting a fallback event
                // This triggers the built-in planner to assess the situation
                consecutive_fallbacks += 1;

                if consecutive_fallbacks > MAX_FALLBACK_ATTEMPTS {
                    warn!(
                        attempts = consecutive_fallbacks,
                        "Fallback recovery exhausted after {} attempts, terminating",
                        MAX_FALLBACK_ATTEMPTS
                    );
                    let reason = dispatch_pre_loop_termination_hooks(
                        &event_loop,
                        hooks_dispatch_enabled,
                        &loop_id,
                        &hook_engine,
                        &hook_executor,
                        &suspend_state_store,
                        &ctx,
                        config.event_loop.max_iterations,
                        &mut accumulated_hook_metadata,
                        TerminationReason::Stopped,
                    )
                    .await?;

                    let terminate_event = event_loop.publish_terminate_event(&reason);
                    log_terminate_event(
                        &mut event_logger,
                        event_loop.state().iteration,
                        &terminate_event,
                        Some(event_loop.registry().current_phase().to_string()),
                    );

                    let reason = dispatch_post_loop_termination_hooks(
                        &event_loop,
                        hooks_dispatch_enabled,
                        &loop_id,
                        &hook_engine,
                        &hook_executor,
                        &suspend_state_store,
                        &ctx,
                        config.event_loop.max_iterations,
                        &mut accumulated_hook_metadata,
                        reason,
                    )
                    .await?;

                    handle_termination(
                        &reason,
                        event_loop.state(),
                        &config.core.scratchpad.path,
                        &loop_history,
                        &loop_context,
                        auto_merge,
                        &prompt_content,
                    );
                    // Wait for user to exit TUI (press 'q') on natural completion
                    if let Some(handle) = tui_handle.take() {
                        let _ = handle.await;
                    }
                    return Ok(reason);
                }

                if event_loop.inject_fallback_event() {
                    // Fallback injected successfully, continue to next iteration
                    // The planner will be triggered and can either:
                    // - Dispatch more work if tasks remain
                    // - Output LOOP_COMPLETE if done
                    // - Determine what went wrong and recover
                    continue;
                }

                // Fallback not possible (no planner hat or doesn't subscribe to task.resume)
                warn!("No hats with pending events and fallback not available, terminating");
                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    TerminationReason::Stopped,
                )
                .await?;

                // Per spec: Publish loop.terminate event to observers
                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                );
                // Wait for user to exit TUI (press 'q') on natural completion
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                return Ok(reason);
            }
        };

        // Update RPC state iteration counter
        if let Some(ref shared) = rpc_dispatcher_started {
            shared
                .iteration
                .store(iteration, std::sync::atomic::Ordering::Relaxed);
        }

        // Determine which hat to display in iteration separator
        // When Ralph is coordinating (hat_id == "ralph"), show the active hat being worked on
        let preview_display_hat = if hat_id.as_str() == "ralph" {
            event_loop.get_active_hat_id()
        } else {
            hat_id.clone()
        };

        let post_iteration_start_outcomes = dispatch_phase_event_hooks(
            &event_loop,
            hooks_dispatch_enabled,
            &loop_id,
            &hook_engine,
            &hook_executor,
            HookPhaseEvent::PostIterationStart,
            build_iteration_start_payload_input(
                &loop_id,
                &ctx,
                config.event_loop.max_iterations,
                iteration,
                Some(preview_display_hat.as_str().to_string()),
                Some(preview_display_hat.as_str().to_string()),
                None,
                &accumulated_hook_metadata,
            ),
        );
        merge_accumulated_hook_metadata_from_outcomes(
            &mut accumulated_hook_metadata,
            &post_iteration_start_outcomes,
        );
        fail_if_blocking_iteration_start_outcomes(&post_iteration_start_outcomes)?;

        if let Some(reason) = wait_for_resume_if_suspended(
            &post_iteration_start_outcomes,
            &loop_id,
            &suspend_state_store,
        )
        .await?
        {
            let reason = dispatch_pre_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );

            let reason = dispatch_post_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
            );
            // Wait for user to exit TUI (press 'q') on natural completion
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            return Ok(reason);
        }

        // Log hat changes with appropriate messaging
        // Skip in TUI mode - TUI shows hat info in header, and stdout would corrupt display
        // Skip in RPC mode - JSON events replace console output
        if last_hat.as_ref() != Some(&hat_id) {
            if tui_state.is_none() && !enable_rpc {
                if hat_id.as_str() == "ralph" {
                    info!("I'm Ralph. Let's do this.");
                } else {
                    info!("Putting on my {} hat.", hat_id);
                }
            }
            last_hat = Some(hat_id.clone());
        }
        debug!(
            "Iteration {}/{} - {} active",
            iteration, config.event_loop.max_iterations, hat_id
        );

        // Build prompt for this hat
        let prompt = match event_loop.build_prompt(&hat_id) {
            Some(p) => p,
            None => {
                error!("Failed to build prompt for hat '{}'", hat_id);
                continue;
            }
        };

        let display_hat =
            resolve_display_hat_for_execution(&event_loop, &hat_id, &preview_display_hat);

        // Log full prompt to diagnostics (RALPH_DIAGNOSTICS=1)
        event_loop.log_prompt(iteration, display_hat.as_str(), &prompt);

        let hat_display = event_loop
            .registry()
            .get(&display_hat)
            .map(|hat| hat.name.clone())
            .unwrap_or_else(|| display_hat.as_str().to_string());

        // Update RPC shared hat state so get_state reflects the current iteration's hat.
        if let Some(ref shared) = rpc_dispatcher_started
            && let Ok(mut guard) = shared.hat.lock()
        {
            *guard = (display_hat.as_str().to_string(), hat_display.clone());
        }

        // Track iteration start time for RPC iteration_end duration calculation
        // (cheap to create even when not in RPC mode)
        let iteration_started_at = std::time::Instant::now();

        // Emit RPC iteration_start event after prompt construction so the displayed
        // hat matches the one actually selected for execution.
        if let Some(ref tx) = rpc_event_tx {
            let started_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let start_event = RpcEvent::IterationStart {
                iteration,
                max_iterations: Some(config.event_loop.max_iterations),
                hat: display_hat.as_str().to_string(),
                hat_display: hat_display.clone(),
                backend: config.cli.backend.clone(),
                started_at,
            };
            let _ = tx.try_send(start_event);
        }

        // Per spec: Print iteration demarcation separator
        // "Each iteration must be clearly demarcated in the output so users can
        // visually distinguish where one iteration ends and another begins."
        // Skip when TUI is enabled - TUI has its own header showing iteration info
        // Skip in RPC mode - JSON events replace console output
        if tui_state.is_none() && !enable_rpc {
            print_iteration_separator(
                iteration,
                display_hat.as_str(),
                event_loop.state().elapsed(),
                config.event_loop.max_iterations,
                use_colors,
            );
        }

        // In verbose mode, print the full prompt before execution
        if verbosity == Verbosity::Verbose {
            eprintln!("\n{}", "=".repeat(80));
            eprintln!("PROMPT FOR {} (iteration {})", hat_id, iteration);
            eprintln!("{}", "-".repeat(80));
            eprintln!("{}", prompt);
            eprintln!("{}\n", "=".repeat(80));
        }

        // Execute the prompt (interactive or autonomous mode)
        // Determine which backend to use for this hat and the appropriate timeout
        // Hat-level backend configuration takes precedence over global cli.backend

        // Step 1: Get hat backend configuration for the active hat
        // Use display_hat (the active hat) instead of hat_id ("ralph" in multi-hat mode)
        let hat_config_opt = event_loop.registry().get_config(&display_hat);
        let hat_backend_opt = hat_config_opt.and_then(|c| c.backend.as_ref());
        let hat_backend_args = hat_config_opt.and_then(|c| c.backend_args.clone());

        // Step 2: Resolve effective backend and determine backend name for timeout
        // Note: backend_name_for_timeout is owned String to avoid lifetime issues with hat_backend reference
        let (mut effective_backend, backend_name_for_timeout): (CliBackend, String) =
            match hat_backend_opt {
                Some(hat_backend) => {
                    // Hat has custom backend configuration
                    match CliBackend::from_hat_backend(hat_backend) {
                        Ok(hat_backend_instance) => {
                            debug!(
                                "Using hat-level backend for '{}': {:?}",
                                display_hat, hat_backend
                            );

                            // Determine backend name for timeout based on hat backend type
                            // Use owned String to avoid borrowing issues and improve code clarity
                            let backend_name = match hat_backend {
                                ralph_core::HatBackend::Named(name) => name.clone(),
                                ralph_core::HatBackend::NamedWithArgs { backend_type, .. } => {
                                    backend_type.clone()
                                }
                                ralph_core::HatBackend::KiroAgent { backend_type, .. } => {
                                    backend_type.clone()
                                }
                                // For Custom backends, extract command name from path
                                // Handles both Unix ("/usr/bin/codex") and commands with args ("ollama run llama3")
                                ralph_core::HatBackend::Custom { command, .. } => {
                                    // First split by whitespace to handle commands with arguments
                                    // e.g., "ollama run llama3" -> "ollama"
                                    let base_command =
                                        command.split_whitespace().next().unwrap_or(command);
                                    // Then extract filename from path
                                    // e.g., "/usr/bin/codex" -> "codex"
                                    std::path::Path::new(base_command)
                                        .file_name()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("custom")
                                        .to_string()
                                }
                            };

                            (hat_backend_instance, backend_name)
                        }
                        Err(e) => {
                            // Failed to create backend from hat config - fall back to global
                            warn!(
                                "Failed to create backend from hat configuration for '{}': {}. Falling back to global backend.",
                                display_hat, e
                            );
                            // IMPORTANT: Use global backend name for timeout since we're using global backend
                            (backend.clone(), config.cli.backend.clone())
                        }
                    }
                }
                None => {
                    // No custom backend - use global configuration
                    debug!(
                        "Using global backend for '{}': {}",
                        display_hat, config.cli.backend
                    );
                    (backend.clone(), config.cli.backend.clone())
                }
            };

        // Step 2.5: Apply custom hat backend args if configured
        if let Some(args) = hat_backend_args {
            effective_backend.args.extend(args);
        }

        // Inject hat execution context into backend environment
        let events_path = resolve_emit_events_path(&ctx, state_machine_enabled);
        let triggered_hat = event_loop.triggered_hat().map(|h| h.as_str().to_string());
        inject_hat_execution_env(
            &mut effective_backend,
            display_hat.as_str(),
            &loop_id,
            &events_path,
            triggered_hat.as_deref(),
        );

        // Step 3: Get timeout from config based on actual backend being used
        let timeout_secs = config.adapter_settings(&backend_name_for_timeout).timeout;
        let timeout = Some(Duration::from_secs(timeout_secs));

        // For TUI mode, get the shared lines buffer for this iteration.
        // The buffer is owned by TuiState's IterationBuffer, so writes from
        // TuiStreamHandler appear immediately in the TUI (real-time streaming).
        let tui_lines: Option<Arc<std::sync::Mutex<Vec<ratatui::text::Line<'static>>>>> =
            if let Some(ref state) = tui_state {
                // Start new iteration and get handle to the LATEST iteration's lines buffer.
                // We must use latest_iteration_lines_handle() instead of current_iteration_lines_handle()
                // because the user may be viewing an older iteration while a new one executes.
                prepare_tui_iteration(
                    state,
                    hat_display.clone(),
                    backend_name_for_timeout.clone(),
                    config.event_loop.max_iterations,
                )
            } else {
                None
            };

        // Race execution against interrupt signal for immediate termination on Ctrl+C
        let mut interrupt_rx_clone = interrupt_rx.clone();
        let interrupt_rx_for_pty = interrupt_rx.clone();
        let tui_lines_for_pty = tui_lines.clone();
        let rpc_stdout_for_pty = rpc_stdout.clone();
        let execute_future = async {
            if effective_backend.output_format == BackendOutputFormat::Acp {
                execute_acp(
                    &effective_backend,
                    &config,
                    &prompt,
                    verbosity,
                    tui_lines_for_pty,
                    rpc_stdout_for_pty,
                    iteration,
                    display_hat.as_str(),
                    &backend_name_for_timeout,
                )
                .await
            } else if use_pty {
                execute_pty(
                    pty_executor.as_mut(),
                    &effective_backend,
                    &config,
                    &prompt,
                    user_interactive,
                    interrupt_rx_for_pty,
                    verbosity,
                    tui_lines_for_pty,
                    rpc_stdout_for_pty,
                    iteration,
                    display_hat.as_str(),
                    &backend_name_for_timeout,
                )
                .await
            } else {
                let executor = CliExecutor::new(effective_backend.clone());
                let result = executor
                    .execute(&prompt, stdout(), timeout, verbosity == Verbosity::Verbose)
                    .await?;
                Ok(ExecutionOutcome {
                    output: normalize_cli_output_for_parsing(
                        effective_backend.output_format,
                        &result.output,
                    ),
                    success: result.success,
                    termination: None,
                    total_cost_usd: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                })
            }
        };

        let outcome = tokio::select! {
            result = execute_future => result?,
            _ = interrupt_rx_clone.changed() => {
                // Immediately terminate children via process group signal
                #[cfg(unix)]
                {
                    use nix::sys::signal::{killpg, Signal};
                    use nix::unistd::getpgrp;
                    let pgid = getpgrp();
                    debug!("Sending SIGTERM to process group {}", pgid);
                    let _ = killpg(pgid, Signal::SIGTERM);

                    // Wait briefly for graceful exit, then SIGKILL
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let _ = killpg(pgid, Signal::SIGKILL);
                }

                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    TerminationReason::Interrupted,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(&mut event_logger, event_loop.state().iteration, &terminate_event, Some(event_loop.registry().current_phase().to_string()));

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(&reason, event_loop.state(), &config.core.scratchpad.path, &loop_history, &loop_context, auto_merge, &prompt_content);
                // Signal TUI to exit immediately on interrupt
                let _ = terminated_tx.send(true);
                return Ok(reason);
            }
        };

        if let Some(reason) = outcome.termination {
            let reason = dispatch_pre_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );

            let reason = dispatch_post_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
            );
            // Wait for user to exit TUI (press 'q') on natural completion
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            return Ok(reason);
        }

        let output = outcome.output;
        let success = outcome.success;
        let output_hat_id = resolve_hat_for_output_processing(&hat_id, &display_hat);

        // Note: TUI lines are now written directly to IterationBuffer during streaming,
        // so no post-execution transfer is needed.
        if let Some(mut s) = tui_state.as_ref().and_then(|state| state.lock().ok()) {
            s.finish_latest_iteration();
        }

        // Emit RPC iteration_end event
        if let Some(ref tx) = rpc_event_tx {
            let duration_ms = iteration_started_at.elapsed().as_millis() as u64;
            // Check if this iteration's output contains LOOP_COMPLETE
            let loop_complete_triggered = output.contains(&config.event_loop.completion_promise);
            let iteration_cost_usd = outcome.total_cost_usd;
            if let Some(ref shared) = rpc_dispatcher_started
                && let Ok(mut guard) = shared.total_cost_usd.lock()
            {
                *guard += iteration_cost_usd;
            }
            let end_event = RpcEvent::IterationEnd {
                iteration,
                duration_ms,
                cost_usd: iteration_cost_usd,
                input_tokens: outcome.input_tokens,
                output_tokens: outcome.output_tokens,
                cache_read_tokens: outcome.cache_read_tokens,
                cache_write_tokens: outcome.cache_write_tokens,
                loop_complete_triggered,
            };
            let _ = tx.try_send(end_event);
        }

        // Per-iteration footer for --no-tui: one line with budget/cost/elapsed
        // so tailing agents can catch runaway loops without parsing events.
        if tui_state.is_none() && !enable_rpc {
            let iter_duration = iteration_started_at.elapsed();
            print_iteration_footer(
                iteration,
                config.event_loop.max_iterations,
                iter_duration,
                event_loop.state().elapsed(),
                outcome.total_cost_usd,
                event_loop.state().cumulative_cost,
                use_colors,
            );
        }

        // Legacy configs log candidate events from backend output. State-machine
        // configs use accepted-only logging after runtime validation.
        let raw_output_logging_enabled = !config
            .event_loop
            .state_machine
            .as_ref()
            .is_some_and(|sm| sm.enabled);
        log_events_from_output(
            &mut event_logger,
            iteration,
            &output_hat_id,
            &output,
            event_loop.registry(),
            raw_output_logging_enabled,
        );

        // Process output
        if let Some(reason) = event_loop.process_output(&output_hat_id, &output, success) {
            // Per spec: Log "All done! {promise} detected." when completion promise found
            if reason == TerminationReason::CompletionPromise {
                info!(
                    "All done! {} detected.",
                    config.event_loop.completion_promise
                );
            }

            let reason = dispatch_pre_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            // Per spec: Publish loop.terminate event to observers
            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );

            let reason = dispatch_post_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
            );
            // Wait for user to exit TUI (press 'q') on natural completion
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            return Ok(reason);
        }

        // Check for planning session user responses (if in planning mode)
        if let Err(e) = check_planning_session_responses(&mut event_loop) {
            warn!(error = %e, "Failed to check planning session responses");
        }

        let should_dispatch_plan_created_hooks = event_loop
            .has_pending_plan_events_in_jsonl()
            .inspect_err(|e| {
                warn!(
                    error = %e,
                    "Failed to inspect unread JSONL events for semantic plan.* topics"
                )
            })
            .unwrap_or(false);

        if should_dispatch_plan_created_hooks {
            let pre_plan_created_outcomes = dispatch_phase_event_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                HookPhaseEvent::PrePlanCreated,
                build_plan_created_payload_input(
                    &loop_id,
                    &ctx,
                    config.event_loop.max_iterations,
                    event_loop.state().iteration,
                    Some(display_hat.as_str().to_string()),
                    Some(display_hat.as_str().to_string()),
                    None,
                    &accumulated_hook_metadata,
                ),
            );
            merge_accumulated_hook_metadata_from_outcomes(
                &mut accumulated_hook_metadata,
                &pre_plan_created_outcomes,
            );
            fail_if_blocking_plan_created_outcomes(&pre_plan_created_outcomes)?;

            if let Some(reason) = wait_for_resume_if_suspended(
                &pre_plan_created_outcomes,
                &loop_id,
                &suspend_state_store,
            )
            .await?
            {
                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                );
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                return Ok(reason);
            }
        }

        let pending_human_interact_context = event_loop
            .pending_human_interact_context_in_jsonl()
            .inspect_err(|e| {
                warn!(
                    error = %e,
                    "Failed to inspect unread JSONL events for human.interact boundary"
                )
            })
            .ok()
            .flatten();

        if let Some(human_interact_context) = pending_human_interact_context {
            let pre_human_interact_outcomes = dispatch_phase_event_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                HookPhaseEvent::PreHumanInteract,
                build_human_interact_payload_input(
                    &loop_id,
                    &ctx,
                    config.event_loop.max_iterations,
                    event_loop.state().iteration,
                    Some(display_hat.as_str().to_string()),
                    Some(display_hat.as_str().to_string()),
                    None,
                    Some(human_interact_context),
                    &accumulated_hook_metadata,
                ),
            );
            merge_accumulated_hook_metadata_from_outcomes(
                &mut accumulated_hook_metadata,
                &pre_human_interact_outcomes,
            );
            fail_if_blocking_human_interact_outcomes(&pre_human_interact_outcomes)?;

            if let Some(reason) = wait_for_resume_if_suspended(
                &pre_human_interact_outcomes,
                &loop_id,
                &suspend_state_store,
            )
            .await?
            {
                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                );
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                return Ok(reason);
            }
        }

        // Read events from JSONL, partitioning wave events from regular events
        let (processed_events, wave_events) =
            match event_loop.process_events_from_jsonl_with_waves() {
                Ok(result) => (Some(result.processed), result.wave_events),
                Err(e) => {
                    warn!(error = %e, "Failed to read events from JSONL");
                    (None, Vec::new())
                }
            };

        if let Some(processed) = processed_events.as_ref()
            && !raw_output_logging_enabled
        {
            log_accepted_events(
                &mut event_logger,
                event_loop.state().iteration,
                &hat_id,
                &processed.accepted_events,
                event_loop.registry(),
            );
        }

        // ── U6: Handle execution contract rejections ─────────────────────────
        // Log contract rejections for operator visibility and diagnostics.
        // Rejections do NOT terminate the loop — guidance drives the next iteration.
        if let Some(processed) = processed_events.as_ref() {
            handle_execution_contract_rejections(processed, &mut event_loop, &display_hat);
        }

        // ── U6: Handle payload contract violations ───────────────────────────
        // Unlike execution contract rejections (which drive recovery via
        // human.guidance), payload contract violations pause the loop and
        // emit a structured diagnostic. The non-regression contract is:
        //   - the diagnostic file MUST be written
        //   - the loop MUST terminate with PayloadContractViolation
        //   - the diagnostic must surface on stderr even if file write fails
        if let Some(processed) = processed_events.as_ref()
            && let Some(violation) = &processed.payload_contract_violation
        {
            // U6: write diagnostic and terminate. Default location is
            // `<workspace>/.ralph/diagnostics`; the loop context is the
            // source of truth for the workspace.
            let diagnostics_dir = event_loop
                .loop_context()
                .map(|c| c.workspace().join(".ralph").join("diagnostics"))
                .unwrap_or_else(|| std::path::PathBuf::from(".ralph/diagnostics"));
            write_payload_contract_violation_report(&diagnostics_dir, violation);
            return Ok(TerminationReason::PayloadContractViolation);
        }

        // ── PhaseWatcher: Check for experiment.evaluated during warmup ───────
        // PhaseWatcher monitors accepted events and triggers phase transitions
        // when the configured exit condition is met (e.g., experiment.evaluated)
        let phase_transition_reason = if current_phase == Phase::Warmup {
            // Check if any accepted event is experiment.evaluated
            let has_experiment_evaluated = processed_events
                .as_ref()
                .map(|events| {
                    events
                        .accepted_events
                        .iter()
                        .any(|e| e.topic.as_str() == "experiment.evaluated")
                })
                .unwrap_or(false);

            if has_experiment_evaluated {
                info!("experiment.evaluated detected in warmup phase — checking exit conditions");
                // Run the check exit conditions script
                match run_check_exit_conditions(&ctx).await {
                    Ok(CheckExitResult::Ready) => {
                        info!("Exit conditions satisfied — initiating phase transition");
                        if stop_on_exit {
                            Some("warmup_complete")
                        } else {
                            Some("phase_transition")
                        }
                    }
                    Ok(CheckExitResult::NotReady { unmet_conditions }) => {
                        debug!(
                            ?unmet_conditions,
                            "Exit conditions not yet satisfied — continuing warmup"
                        );
                        None
                    }
                    Ok(CheckExitResult::DrainRequired { pending_count }) => {
                        info!(
                            pending_count,
                            "Drain required — waiting for in-flight experiments to complete"
                        );
                        None
                    }
                    Err(e) => {
                        warn!(error = %e, "Check exit conditions failed — continuing warmup");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some(phase_reason) = phase_transition_reason {
            // Phase transition triggered by PhaseWatcher
            info!(
                "Phase transition triggered: {} (stop_on_exit: {})",
                phase_reason, stop_on_exit
            );

            // Run transition script and update phase
            match run_transition_script(&ctx, stop_on_exit).await {
                Ok(_) => {
                    // Update phase to production
                    let new_phase = Phase::Production;
                    event_loop.registry_mut().set_phase(new_phase.clone());

                    // Update phase.json
                    let phase_json = serde_json::json!({
                        "phase": new_phase.to_string(),
                        "warmup_completed": stop_on_exit,
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    });
                    let agent_dir = ctx.ralph_dir().join("agent");
                    fs::create_dir_all(&agent_dir).ok();
                    let phase_path = agent_dir.join("phase.json");
                    if let Err(e) = fs::write(
                        &phase_path,
                        serde_json::to_string_pretty(&phase_json).unwrap(),
                    ) {
                        warn!(error = %e, "Failed to write phase.json");
                    }

                    // Publish phase transition event
                    let transition_topic = if stop_on_exit {
                        "warmup.complete"
                    } else {
                        "phase.transition"
                    };
                    let transition_payload = serde_json::json!({
                        "phase": new_phase.to_string(),
                        "reason": phase_reason,
                        "warmup_completed": stop_on_exit,
                    });
                    let transition_event =
                        Event::new(transition_topic, &transition_payload.to_string());
                    event_loop.publish_event(transition_event);

                    // If warmup_only mode, terminate the loop
                    if stop_on_exit {
                        let reason = TerminationReason::CompletionPromise;
                        handle_termination(
                            &reason,
                            event_loop.state(),
                            &config.core.scratchpad.path,
                            &loop_history,
                            &loop_context,
                            auto_merge,
                            &prompt_content,
                        );
                        if let Some(handle) = tui_handle.take() {
                            let _ = handle.await;
                        }
                        return Ok(reason);
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Phase transition script failed — continuing in warmup");
                }
            }
        }

        if let Some(human_interact_context) = processed_events
            .as_ref()
            .and_then(|events| events.human_interact_context.clone())
        {
            let post_human_interact_outcomes = dispatch_phase_event_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                HookPhaseEvent::PostHumanInteract,
                build_human_interact_payload_input(
                    &loop_id,
                    &ctx,
                    config.event_loop.max_iterations,
                    event_loop.state().iteration,
                    Some(display_hat.as_str().to_string()),
                    Some(display_hat.as_str().to_string()),
                    None,
                    Some(human_interact_context),
                    &accumulated_hook_metadata,
                ),
            );
            merge_accumulated_hook_metadata_from_outcomes(
                &mut accumulated_hook_metadata,
                &post_human_interact_outcomes,
            );
            fail_if_blocking_human_interact_outcomes(&post_human_interact_outcomes)?;

            if let Some(reason) = wait_for_resume_if_suspended(
                &post_human_interact_outcomes,
                &loop_id,
                &suspend_state_store,
            )
            .await?
            {
                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                );
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                return Ok(reason);
            }
        }

        if processed_events
            .as_ref()
            .map(|events| events.had_plan_events)
            .unwrap_or(false)
        {
            let post_plan_created_outcomes = dispatch_phase_event_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                HookPhaseEvent::PostPlanCreated,
                build_plan_created_payload_input(
                    &loop_id,
                    &ctx,
                    config.event_loop.max_iterations,
                    event_loop.state().iteration,
                    Some(display_hat.as_str().to_string()),
                    Some(display_hat.as_str().to_string()),
                    None,
                    &accumulated_hook_metadata,
                ),
            );
            merge_accumulated_hook_metadata_from_outcomes(
                &mut accumulated_hook_metadata,
                &post_plan_created_outcomes,
            );
            fail_if_blocking_plan_created_outcomes(&post_plan_created_outcomes)?;

            if let Some(reason) = wait_for_resume_if_suspended(
                &post_plan_created_outcomes,
                &loop_id,
                &suspend_state_store,
            )
            .await?
            {
                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                );
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                return Ok(reason);
            }
        }

        let mut agent_wrote_events = processed_events
            .as_ref()
            .map(|events| events.had_events)
            .unwrap_or(false);

        // Agent wrote any valid or rejected events — used for missing-event gate
        let agent_wrote_any_valid_or_rejected = processed_events
            .as_ref()
            .map(|events| events.had_raw_events || events.had_rejected_events)
            .unwrap_or(false);

        let mut late_termination_reason: Option<TerminationReason> = None;
        let mut hard_gate_triggered_this_iteration = false;
        if !agent_wrote_events && output_mentions_ralph_emit(&output) {
            match recover_expected_emit_after_output(&mut event_loop)
                .inspect_err(|e| warn!(error = %e, "Failed to recover expected emit events"))
                .ok()
            {
                Some(LateEventRecovery::PendingWork) => {
                    agent_wrote_events = true;
                    event_loop.reset_hard_gate_count();
                }
                Some(LateEventRecovery::Terminate(reason)) => {
                    agent_wrote_events = true;
                    event_loop.reset_hard_gate_count();
                    late_termination_reason = Some(reason);
                }
                Some(LateEventRecovery::NoLateEvents) | None => {
                    if should_hard_gate(&display_hat, &event_loop) {
                        hard_gate_triggered_this_iteration = true;
                        event_loop.increment_hard_gate_count();
                        inject_hard_gate_guidance(
                            &ctx,
                            &display_hat,
                            &event_loop.get_hat_publishes(&display_hat),
                        );
                        info!(
                            hat = %display_hat.as_str(),
                            consecutive = event_loop.state().consecutive_hard_gates,
                            "Hard gate triggered: agent claimed emit but no event written"
                        );
                    } else {
                        event_loop.reset_hard_gate_count();
                        warn!(
                            hat = %hat_id.as_str(),
                            "Output indicated `ralph emit`, but no event became readable before fallback logic"
                        );
                    }
                }
            }
        }

        // Execute wave if wave events detected
        if !wave_events.is_empty() {
            handle_wave_events(
                &wave_events,
                &mut event_loop,
                &backend,
                &ctx,
                use_colors,
                enable_rpc,
                rpc_event_tx.as_ref(),
                tui_state.as_ref(),
                &loop_id,
            )
            .await;
        }

        // Inject default_publishes for active hats only when agent wrote no events.
        // Skip default_publishes when hard gate triggered — the agent explicitly
        // claimed to emit and we want it to learn to do so, not be bailed out.
        // Prefer the displayed execution hat first so a non-emitting turn still
        // falls back to the hat the user actually saw in the banner.
        //
        // MISSING-EVENT GATE (U1): Regardless of whether output mentioned `ralph emit`,
        // if the hat has a publish obligation but no default_publishes fallback,
        // hard gate on missing events. This catches the "completely forgot" case.
        // Contract rejection does NOT trigger this gate because the agent DID try to emit.
        if !agent_wrote_any_valid_or_rejected
            && wave_events.is_empty()
            && !hard_gate_triggered_this_iteration
            && should_gate_missing_events(&display_hat, &event_loop)
        {
            event_loop.increment_hard_gate_count();
            inject_missing_event_hard_gate_guidance(
                &ctx,
                &display_hat,
                &event_loop.get_hat_publishes(&display_hat),
            );
            info!(
                hat = %display_hat.as_str(),
                consecutive = event_loop.state().consecutive_hard_gates,
                "Hard gate triggered: hat has publish obligation but emitted no event"
            );
        } else if !agent_wrote_any_valid_or_rejected
            && wave_events.is_empty()
            && !hard_gate_triggered_this_iteration
        {
            let mut fallback_hats = Vec::new();
            if display_hat.as_str() != "ralph" {
                fallback_hats.push(display_hat.clone());
            }
            for active_hat_id in event_loop.state().last_active_hat_ids.clone() {
                if !fallback_hats.contains(&active_hat_id) {
                    fallback_hats.push(active_hat_id);
                }
            }

            for active_hat_id in &fallback_hats {
                event_loop.check_default_publishes(active_hat_id);
                if event_loop.has_pending_events() {
                    break; // One default is sufficient
                }
            }
        }

        // Check cancellation first (no chain validation) — takes priority over completion
        if let Some(reason) = event_loop.check_cancellation_event() {
            info!("Loop cancelled gracefully via loop.cancel event.");

            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );
            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
            );
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            return Ok(reason);
        }

        if let Some(reason) =
            late_termination_reason.or_else(|| event_loop.check_completion_event())
        {
            info!(
                "Completion event {} detected.",
                config.event_loop.completion_promise
            );

            let reason = dispatch_pre_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            let terminate_event = event_loop.publish_terminate_event(&reason);
            log_terminate_event(
                &mut event_logger,
                event_loop.state().iteration,
                &terminate_event,
                Some(event_loop.registry().current_phase().to_string()),
            );

            let reason = dispatch_post_loop_termination_hooks(
                &event_loop,
                hooks_dispatch_enabled,
                &loop_id,
                &hook_engine,
                &hook_executor,
                &suspend_state_store,
                &ctx,
                config.event_loop.max_iterations,
                &mut accumulated_hook_metadata,
                reason,
            )
            .await?;

            handle_termination(
                &reason,
                event_loop.state(),
                &config.core.scratchpad.path,
                &loop_history,
                &loop_context,
                auto_merge,
                &prompt_content,
            );
            if let Some(handle) = tui_handle.take() {
                let _ = handle.await;
            }
            return Ok(reason);
        }

        // Fallback: detect completion promise in output text.
        // Primary path is JSONL events (check_completion_event above).
        // This catches backends that output LOOP_COMPLETE as text — either
        // without `ralph emit` (e.g. kiro-cli) or alongside it (e.g. OpenCode
        // which writes both a JSONL event and prints "Event emitted:" to stdout).
        //
        // We route through check_completion_event() to ensure all safety checks
        // are applied (persistent mode suppression, required_events validation,
        // runtime task verification). No parallel termination path.
        if EventParser::contains_promise(&output, &config.event_loop.completion_promise) {
            event_loop.request_completion_from_text_fallback();
            if let Some(reason) = event_loop.check_completion_event() {
                info!(
                    "Completion promise {} detected in output text.",
                    config.event_loop.completion_promise
                );

                let reason = dispatch_pre_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                let terminate_event = event_loop.publish_terminate_event(&reason);
                log_terminate_event(
                    &mut event_logger,
                    event_loop.state().iteration,
                    &terminate_event,
                    Some(event_loop.registry().current_phase().to_string()),
                );

                let reason = dispatch_post_loop_termination_hooks(
                    &event_loop,
                    hooks_dispatch_enabled,
                    &loop_id,
                    &hook_engine,
                    &hook_executor,
                    &suspend_state_store,
                    &ctx,
                    config.event_loop.max_iterations,
                    &mut accumulated_hook_metadata,
                    reason,
                )
                .await?;

                handle_termination(
                    &reason,
                    event_loop.state(),
                    &config.core.scratchpad.path,
                    &loop_history,
                    &loop_context,
                    auto_merge,
                    &prompt_content,
                );
                if let Some(handle) = tui_handle.take() {
                    let _ = handle.await;
                }
                return Ok(reason);
            }
            // Safety check rejected completion (persistent mode, missing required
            // events, open tasks, etc.) — continue the loop normally.
        }

        // Precheck validation: Warn if no pending events after processing output
        // Per EventLoop doc: "Use has_pending_events after process_output to detect
        // if the LLM failed to publish an event."
        if !event_loop.has_pending_events() {
            let expected = event_loop.get_hat_publishes(&hat_id);
            debug!(
                hat = %hat_id.as_str(),
                expected_topics = ?expected,
                "No pending events after iteration. Agent may have failed to publish a valid event. \
                 Expected one of: {:?}. Loop will terminate on next iteration.",
                expected
            );
        }

        // Cooldown delay between iterations (skip for human events)
        let cooldown = config.event_loop.cooldown_delay_seconds;
        if cooldown > 0 && !event_loop.has_pending_human_events() {
            debug!(
                delay_seconds = cooldown,
                "Cooldown delay before next iteration"
            );
            tokio::time::sleep(Duration::from_secs(cooldown)).await;
        }
    }
}
