use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ralph_adapters::CliBackend;
use ralph_proto::RpcEvent;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use tracing::{info, warn};

use super::io::{merge_wave_results_to_events_file, push_to_tui_iteration};
use super::worker::run_wave_worker;
use crate::display::{print_wave_header, print_wave_summary, print_wave_worker_done};
use crate::loop_runner::execution::inject_hat_execution_env;
use crate::loop_runner::paths::{config_state_machine_enabled, resolve_emit_events_path};

/// Bundled output channels for wave progress reporting.
pub struct WaveOutputs<'a> {
    pub use_colors: bool,
    /// Show wave progress on CLI (no TUI, no RPC).
    pub show_cli: bool,
    pub rpc_tx: Option<&'a tokio::sync::mpsc::Sender<RpcEvent>>,
    pub tui: Option<&'a Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
}

/// Handle wave events: detect, execute, merge results, and update UI.
///
/// Orchestrates the full wave lifecycle — detection, parallel execution,
/// result merging back to the events file, and re-reading for aggregator
/// activation. Updates CLI, RPC, and TUI outputs as appropriate.
///
/// v2: Processes **all** valid waves detected in the batch, not just the
/// lexicographically first one.  This prevents silent drops when a hat
/// emits multiple waves in a single iteration (e.g. review-coordinator
/// retrying after an empty payload).
pub async fn handle_wave_events(
    wave_events: &[ralph_core::Event],
    event_loop: &mut ralph_core::EventLoop,
    backend: &CliBackend,
    ctx: &ralph_core::LoopContext,
    use_colors: bool,
    enable_rpc: bool,
    rpc_event_tx: Option<&tokio::sync::mpsc::Sender<RpcEvent>>,
    tui_state: Option<&Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
    loop_id: &str,
) {
    let waves = ralph_core::detect_all_wave_events(wave_events, event_loop.registry());
    if waves.is_empty() {
        return;
    }

    info!(
        wave_count = waves.len(),
        "Detected multiple waves in single iteration, executing all"
    );

    let out = WaveOutputs {
        use_colors,
        show_cli: tui_state.is_none() && !enable_rpc,
        rpc_tx: rpc_event_tx,
        tui: tui_state,
    };

    let main_events_file =
        resolve_emit_events_path(ctx, config_state_machine_enabled(event_loop.config()));

    let mut any_success = false;

    for detected in waves {
        let wave_timeout_secs = detected.timeout_secs();

        info!(
            wave_id = %detected.wave_id,
            total = detected.total,
            hat = %detected.target_hat,
            concurrency = detected.hat_config.concurrency,
            "Wave detected, executing parallel workers"
        );

        // Announce wave start to CLI / RPC / TUI
        if out.show_cli {
            print_wave_header(
                &detected.hat_config.name,
                detected.total as usize,
                wave_timeout_secs,
                out.use_colors,
            );
        }
        if let Some(tx) = out.rpc_tx {
            let _ = tx.try_send(RpcEvent::WaveStarted {
                hat_name: detected.hat_config.name.clone(),
                worker_count: detected.total,
                timeout_secs: wave_timeout_secs,
            });
        }
        if let Some(state) = out.tui {
            if let Ok(mut s) = state.lock() {
                info!(
                    hat = %detected.hat_config.name,
                    workers = detected.total,
                    "Setting wave_active on TUI state"
                );
                s.wave_active = Some(ralph_tui::state::WaveInfo::new(
                    detected.hat_config.name.clone(),
                    detected.total,
                ));
                s.wave_active_iteration_idx = Some(s.iterations.len().saturating_sub(1));
                if let Some(ref wave) = s.wave_active {
                    for (i, buffer) in wave.worker_buffers.iter().enumerate() {
                        if let Ok(mut buf_lines) = buffer.lines_handle().lock() {
                            buf_lines.push(Line::from(Span::styled(
                                format!("Worker {}/{}: launching...", i + 1, detected.total),
                                Style::default().fg(Color::DarkGray),
                            )));
                        }
                    }
                }
            }
            let header_line = Line::from(vec![
                Span::styled("── WAVE: ", Style::default().fg(Color::Magenta)),
                Span::styled(
                    format!(
                        "{} | {} workers | timeout {}s",
                        detected.hat_config.name, detected.total, wave_timeout_secs
                    ),
                    Style::default().fg(Color::Magenta),
                ),
                Span::styled(
                    " ──────────────────────",
                    Style::default().fg(Color::Magenta),
                ),
            ]);
            push_to_tui_iteration(state, header_line);
        }

        let wave_result = execute_wave(
            &detected,
            backend,
            &main_events_file,
            out.show_cli,
            out.use_colors,
            out.rpc_tx.cloned(),
            out.tui.map(Arc::clone),
            loop_id,
        )
        .await;

        match wave_result {
            Ok(completed) => {
                any_success = true;

                // Report completion to CLI / RPC / TUI
                if out.show_cli {
                    print_wave_summary(
                        completed.results.len(),
                        completed.failures.len(),
                        completed.duration,
                        out.use_colors,
                    );
                }
                if let Some(tx) = out.rpc_tx {
                    let _ = tx.try_send(RpcEvent::WaveCompleted {
                        succeeded: completed.results.len(),
                        failed: completed.failures.len(),
                        duration_ms: completed.duration.as_millis() as u64,
                    });
                }
                if let Some(state) = out.tui {
                    if let Ok(mut s) = state.lock() {
                        let wave_iter_idx = s.wave_active_iteration_idx.take();
                        if let Some(wave) = s.wave_active.take() {
                            let target_idx =
                                wave_iter_idx.unwrap_or(s.iterations.len().saturating_sub(1));
                            if let Some(buf) = s.iterations.get_mut(target_idx) {
                                buf.wave_info = Some(wave);
                            }
                        }
                    }
                    let secs = completed.duration.as_secs();
                    let color = if completed.failures.is_empty() {
                        Color::Green
                    } else {
                        Color::Yellow
                    };
                    let line = Line::from(Span::styled(
                        format!(
                            "── Wave complete: {} succeeded, {} failed ({}s) ──────────────────────",
                            completed.results.len(),
                            completed.failures.len(),
                            secs,
                        ),
                        Style::default().fg(color),
                    ));
                    push_to_tui_iteration(state, line);
                }

                info!(
                    wave_id = %completed.wave_id,
                    results = completed.results.len(),
                    failures = completed.failures.len(),
                    duration_ms = completed.duration.as_millis() as u64,
                    "Wave completed"
                );

                // Merge result events into main events file so aggregator hat picks them up
                if let Err(e) = merge_wave_results_to_events_file(
                    &completed,
                    &main_events_file,
                    &detected.hat_config.publishes,
                ) {
                    warn!(error = %e, "Failed to merge wave results to events file");
                }
            }
            Err(e) => {
                warn!(error = %e, "Wave execution failed");
            }
        }
    }

    // Re-read events file once after all waves have been merged so that
    // every wave result is published to the bus.  The EventReader's position
    // was before any merge, so it picks up all newly appended events.
    if any_success {
        if let Ok(reread) = event_loop.process_events_from_jsonl_with_waves()
            && reread.processed.had_events
        {
            info!("Published wave result events to bus for aggregator");
            // Wave results legitimately share the same topic (e.g.
            // 3x review.done). Reset the stale-loop counter so
            // this batch doesn't trigger LoopStale termination.
            event_loop.reset_stale_topic_counter();
        }
    }
}

/// Execute a detected wave by spawning parallel backend instances.
///
/// Creates per-worker event files, spawns workers with concurrency-limited
/// semaphore, collects results, and returns a `CompletedWave`.
pub async fn execute_wave(
    wave: &ralph_core::DetectedWave,
    global_backend: &CliBackend,
    main_events_file: &Path,
    show_progress: bool,
    use_colors: bool,
    rpc_event_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
    loop_id: &str,
) -> Result<ralph_core::CompletedWave> {
    use ralph_core::{WaveTracker, WaveWorkerContext, build_wave_worker_prompt};

    let concurrency = wave.hat_config.concurrency as usize;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));

    let wave_timeout = Duration::from_secs(wave.timeout_secs());

    // Register wave in tracker
    let mut tracker = WaveTracker::new();
    tracker.register_wave(wave.wave_id.clone(), wave.total);

    // Resolve per-worker events directory
    let wave_dir = main_events_file
        .parent()
        .unwrap_or(Path::new(".ralph"))
        .to_path_buf();

    // Build payload previews for display (first ~60 chars of each event payload)
    let payload_previews: Vec<String> = wave
        .events
        .iter()
        .map(|e| e.payload.as_deref().unwrap_or("").replace('\n', " "))
        .collect();

    // Channel for real-time per-worker progress reporting
    let (progress_tx, mut progress_rx) =
        tokio::sync::mpsc::unbounded_channel::<(u32, bool, Duration)>();

    // Spawn workers
    let mut handles = Vec::new();
    for (index, event) in wave.events.iter().enumerate() {
        let permit = semaphore.clone().acquire_owned().await?;
        let wave_id = wave.wave_id.clone();
        let index = index as u32;
        let event = event.clone();
        let hat_config = wave.hat_config.clone();

        // Create per-worker events file
        let worker_events_file = wave_dir.join(format!("wave-{}-{}.jsonl", wave_id, index));

        // Build worker prompt
        let ctx = WaveWorkerContext {
            wave_id: wave_id.clone(),
            wave_index: index,
            wave_total: wave.total,
            result_topics: hat_config.publishes.clone(),
        };
        let prompt = build_wave_worker_prompt(&hat_config, &event, &ctx);

        // Resolve backend for this worker
        let mut worker_backend = if let Some(ref hat_backend) = hat_config.backend {
            CliBackend::from_hat_backend(hat_backend).unwrap_or_else(|_| global_backend.clone())
        } else {
            global_backend.clone()
        };

        #[cfg(test)]
        {
            // Test-only: allow fake backend PATH injection to flow through the global backend
            // when a wave worker resolves its command from a hat-specific backend.
            for (key, value) in &global_backend.env_vars {
                if !worker_backend
                    .env_vars
                    .iter()
                    .any(|(existing, _)| existing == key)
                {
                    worker_backend.env_vars.push((key.clone(), value.clone()));
                }
            }
        }

        // Inject wave env vars
        worker_backend.env_vars.extend([
            ("RALPH_WAVE_WORKER".into(), "1".into()),
            ("RALPH_WAVE_ID".into(), wave_id.clone()),
            ("RALPH_WAVE_INDEX".into(), index.to_string()),
            (
                "RALPH_EVENTS_FILE".into(),
                worker_events_file.display().to_string(),
            ),
        ]);

        // Inject hat execution context for wave worker
        inject_hat_execution_env(
            &mut worker_backend,
            wave.target_hat.as_str(),
            loop_id,
            &worker_events_file,
            None,
        );

        // Apply hat backend args
        if let Some(ref args) = hat_config.backend_args {
            worker_backend.args.extend(args.iter().cloned());
        }

        let worker_events_path = worker_events_file.clone();
        let tx = progress_tx.clone();
        let worker_rpc_tx = rpc_event_tx.clone();
        let worker_tui_state = tui_state.clone();

        let handle = tokio::spawn(async move {
            let _permit = permit; // Hold permit for concurrency limiting
            run_wave_worker(
                index,
                &worker_backend,
                &prompt,
                &worker_events_path,
                wave_timeout,
                tx,
                worker_rpc_tx,
                worker_tui_state,
            )
            .await
        });

        handles.push(handle);
    }

    // Drop our sender so the receiver terminates when all workers finish
    drop(progress_tx);

    // Spawn a task to report real-time progress (CLI, RPC, and/or TUI)
    let total = wave.total;
    let previews = payload_previews;
    let progress_handle = tokio::spawn(async move {
        while let Some((index, success, duration)) = progress_rx.recv().await {
            let preview = previews
                .get(index as usize)
                .map(|s| s.as_str())
                .unwrap_or("");
            if show_progress {
                print_wave_worker_done(index, total, duration, success, preview, use_colors);
            }
            if let Some(ref tx) = rpc_event_tx {
                let _ = tx.try_send(RpcEvent::WaveWorkerDone {
                    index,
                    total,
                    duration_ms: duration.as_millis() as u64,
                    success,
                    payload_preview: preview.to_string(),
                });
            }
            if let Some(ref state) = tui_state {
                if let Ok(mut s) = state.lock()
                    && let Some(ref mut wave) = s.wave_active
                {
                    wave.completed += 1;
                }
                let secs = duration.as_secs();
                let (icon, color) = if success {
                    ("\u{2713}", Color::Green)
                } else {
                    ("\u{2717}", Color::Red)
                };
                let status_word = if success { "done" } else { "failed" };
                let truncated_preview = if preview.len() > 60 {
                    &preview[..ralph_core::floor_char_boundary(preview, 60)]
                } else {
                    preview
                };
                let line = Line::from(vec![
                    Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                    Span::raw(format!(
                        "Worker {}/{} {} ({}s) — {}",
                        index + 1,
                        total,
                        status_word,
                        secs,
                        truncated_preview
                    )),
                ]);
                push_to_tui_iteration(state, line);
            }
        }
    });

    // Compute aggregate timeout: enough wall-clock time for all batches + buffer.
    // With semaphore-based concurrency limiting, total time is bounded by
    // ceil(total / concurrency) * per_worker_timeout.
    let batches = u64::from(wave.total).div_ceil(concurrency as u64);
    let aggregate_timeout = Duration::from_secs(wave_timeout.as_secs().saturating_mul(batches))
        + Duration::from_secs(30);

    // Collect results with aggregate timeout to prevent indefinite hangs
    let results =
        match tokio::time::timeout(aggregate_timeout, futures::future::join_all(handles)).await {
            Ok(results) => results,
            Err(_) => {
                warn!(
                    timeout_secs = aggregate_timeout.as_secs(),
                    "Wave aggregate timeout reached, cancelling remaining workers"
                );
                Vec::new()
            }
        };

    let mut reported_indices = std::collections::HashSet::new();

    for result in results {
        match result {
            Ok((index, Ok((events, _duration, _success)))) => {
                reported_indices.insert(index);
                let proto_events: Vec<ralph_proto::Event> =
                    events.into_iter().map(ralph_proto::Event::from).collect();
                tracker.record_result(&wave.wave_id, index, proto_events);
            }
            Ok((index, Err((error, duration)))) => {
                reported_indices.insert(index);
                tracker.record_failure(&wave.wave_id, index, error, duration);
            }
            Err(join_err) => {
                // Task panicked or was cancelled — index is lost from JoinError.
                // The missing-index sweep below will record the failure.
                warn!(error = %join_err, "Wave worker task panicked");
            }
        }
    }

    // Wait for progress reporter after consuming join results. Successful
    // worker outcomes still own a sender until the result is dropped here.
    let _ = progress_handle.await;

    // Record failures for any workers that didn't report back (panicked or timed out).
    for i in 0..wave.total {
        if !reported_indices.contains(&i) {
            warn!(
                worker = i,
                wave_id = %wave.wave_id,
                "Worker did not report — recording synthetic failure"
            );
            tracker.record_failure(
                &wave.wave_id,
                i,
                "Worker panicked or was cancelled by aggregate timeout".into(),
                aggregate_timeout,
            );
        }
    }

    // Take completed wave results
    tracker
        .take_wave_results(&wave.wave_id)
        .ok_or_else(|| anyhow::anyhow!("Wave {} not found in tracker", wave.wave_id))
}
