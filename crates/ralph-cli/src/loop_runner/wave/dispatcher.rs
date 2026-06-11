use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures::StreamExt;
use ralph_adapters::CliBackend;
use ralph_core::diagnosis::{
    DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope, RecoveryJournalEntry,
};
use ralph_core::diagnostics::DiagnosticsCollector;
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
    diagnostics: Option<&Arc<DiagnosticsCollector>>,
) {
    let max_wave_total = event_loop.config().event_loop.max_wave_total;
    let outcome = ralph_core::detect_all_wave_events_capped(
        wave_events,
        event_loop.registry(),
        ralph_core::PartialWavePolicy::RequireComplete,
        max_wave_total,
    );

    let out = WaveOutputs {
        use_colors,
        show_cli: tui_state.is_none() && !enable_rpc,
        rpc_tx: rpc_event_tx,
        tui: tui_state,
    };

    // U2: emit a single structured `plan.blocked` per rejected wave and
    // record a recovery envelope BEFORE any TUI / backend side-effects.
    for rejected in &outcome.rejected {
        if let Err(err) =
            handle_wave_rejection(rejected, event_loop, &out, diagnostics, loop_id, max_wave_total)
                .await
        {
            warn!(?err, "failed to handle wave rejection");
        }
    }

    let waves = outcome.accepted;
    if waves.is_empty() {
        return;
    }

    info!(
        wave_count = waves.len(),
        "Detected multiple waves in single iteration, executing all"
    );

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
            diagnostics,
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
    diagnostics: Option<&Arc<DiagnosticsCollector>>,
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
        // U2: surface worker-spawn failures as recovery envelopes.
        //
        // Previously `acquire_owned().await?` propagated the semaphore error
        // out of `execute_wave` and aborted the entire wave. From the
        // orchestrator's perspective this looked like a 0/N `dimension.done`
        // rate with no observable cause. Per plan U2 we now skip the
        // affected worker, log a `WaveDispatcher` recovery envelope
        // (when diagnostics are enabled), and let the wave proceed with
        // the rest of the workers. The missing worker is later recorded
        // as a synthetic failure by the "didn't report back" sweep below
        // — so the wave still reports its full count of failures, just
        // now with an attributable root cause in `recovery.jsonl`.
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    wave_id = %wave.wave_id,
                    worker = index,
                    error = %e,
                    "Wave worker semaphore acquire failed; recording recovery envelope and skipping worker"
                );
                if let Some(collector) = diagnostics {
                    let env = RecoveryDiagnosisEnvelope::builder()
                        .source(DiagnosisSource::WaveDispatcher)
                        .severity(DiagnosisSeverity::Error)
                        .reason_code("worker_spawn_failed")
                        .message(format!(
                            "Failed to acquire wave worker permit {}: {}",
                            index, e
                        ))
                        .source_hat(wave.target_hat.as_str())
                        .safe_target(false)
                        .build();
                    collector.log_recovery(RecoveryJournalEntry::from_envelope(env, vec![]));
                }
                continue;
            }
        };
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

    // U1: partial threshold at 80% of aggregate_timeout.
    // When some workers haven't reported by this time, force-dispatch
    // whatever results have arrived so far as a partial wave.
    let partial_threshold = Duration::from_secs(
        (aggregate_timeout.as_secs() * 8).div_ceil(10), // × 0.8, rounded up
    );

    // Collect results using FuturesUnordered so we can record each result
    // to the tracker as it arrives, enabling accurate partial-threshold checks.
    let mut pending = futures::stream::FuturesUnordered::new();
    for handle in handles {
        pending.push(handle);
    }

    let mut partial_threshold_fired = false;
    let partial_deadline = tokio::time::Instant::now() + partial_threshold;
    let aggregate_deadline = tokio::time::Instant::now() + aggregate_timeout;

    loop {
        if pending.is_empty() {
            break;
        }

        let remaining_timeout = if partial_threshold_fired {
            aggregate_deadline.saturating_duration_since(tokio::time::Instant::now())
        } else {
            partial_deadline.saturating_duration_since(tokio::time::Instant::now())
        };

        tokio::select! {
            result = pending.next() => {
                let Some(result) = result else { break };

                match result {
                    Ok((index, Ok((events, _duration, _success)))) => {
                        let proto_events: Vec<ralph_proto::Event> =
                            events.into_iter().map(ralph_proto::Event::from).collect();
                        tracker.record_result(&wave.wave_id, index, proto_events);
                    }
                    Ok((index, Err((error, duration)))) => {
                        tracker.record_failure(&wave.wave_id, index, error, duration);
                    }
                    Err(join_err) => {
                        // Task panicked or was cancelled — index is lost from JoinError.
                        // The missing-index sweep below will record the failure.
                        warn!(error = %join_err, "Wave worker task panicked");
                    }
                }

                // If wave is now complete, no need to wait further
                if tracker.is_complete(&wave.wave_id) {
                    break;
                }
            }
            _ = tokio::time::sleep(remaining_timeout), if !partial_threshold_fired => {
                // Partial threshold reached — check if wave is already complete
                if tracker.is_complete(&wave.wave_id) {
                    break;
                }

                // Force-dispatch partial wave
                warn!(
                    wave_id = %wave.wave_id,
                    threshold_secs = partial_threshold.as_secs(),
                    "Partial threshold reached, force-dispatching available results"
                );
                // Inject synthetic failures for workers that haven't reported yet
                for i in 0..wave.total {
                    if !tracker.has_reported(&wave.wave_id, i) {
                        tracker.record_failure(
                            &wave.wave_id,
                            i,
                            format!(
                                "Worker {} did not report before partial threshold ({}s)",
                                i, partial_threshold.as_secs()
                            ),
                            partial_threshold,
                        );
                    }
                }
                // Force-take whatever we have
                let completed = tracker
                    .force_take_wave_results(&wave.wave_id)
                    .expect("wave must exist in tracker after registration");
                // Cancel remaining worker tasks — they'll be dropped when
                // the FuturesUnordered goes out of scope.
                let _ = progress_handle.await;
                return Ok(completed);
            }
            _ = tokio::time::sleep_until(aggregate_deadline), if partial_threshold_fired => {
                // Aggregate timeout reached after partial threshold
                warn!(
                    timeout_secs = aggregate_timeout.as_secs(),
                    "Wave aggregate timeout reached, cancelling remaining workers"
                );
                break;
            }
        }

        // If we just passed the partial threshold point without the select
        // firing (because workers were completing), mark it as fired.
        if !partial_threshold_fired && tokio::time::Instant::now() >= partial_deadline {
            partial_threshold_fired = true;
        }
    }

    // Wait for progress reporter after consuming join results.
    let _ = progress_handle.await;

    // Record failures for any workers that didn't report back (panicked or timed out).
    for i in 0..wave.total {
        if !tracker.has_reported(&wave.wave_id, i) {
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

/// U2: Handle a single rejected wave.
///
/// Emits a structured `plan.blocked` event with the typed reason and
/// records a `RecoveryDiagnosisEnvelope` so the responder can escalate
/// after a stable retry window. **No** worker, TUI update, or backend
/// call is performed — the wave is short-circuited before any of those
/// side-effects.
async fn handle_wave_rejection(
    rejected: &ralph_core::RejectedWave,
    event_loop: &mut ralph_core::EventLoop,
    out: &WaveOutputs<'_>,
    diagnostics: Option<&Arc<DiagnosticsCollector>>,
    loop_id: &str,
    max_wave_total: u32,
) -> Result<()> {
    use ralph_core::diagnosis::{DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource};

    let (reason_code, structured_reason) = match &rejected.reason {
        ralph_core::WaveRejection::TotalExceedsCap { actual, cap } => (
            "wave_total_exceeds_cap",
            serde_json::json!({
                "reason": "wave_total_exceeds_cap",
                "wave_id": rejected.wave_id,
                "topic": rejected.topic,
                "actual": actual,
                "cap": cap,
            }),
        ),
        ralph_core::WaveRejection::ZeroTotal => (
            "wave_total_zero",
            serde_json::json!({
                "reason": "wave_total_zero",
                "wave_id": rejected.wave_id,
                "topic": rejected.topic,
            }),
        ),
        ralph_core::WaveRejection::InconsistentTopic => (
            "wave_inconsistent_topic",
            serde_json::json!({
                "reason": "wave_inconsistent_topic",
                "wave_id": rejected.wave_id,
                "topic": rejected.topic,
            }),
        ),
        ralph_core::WaveRejection::InconsistentTotal => (
            "wave_inconsistent_total",
            serde_json::json!({
                "reason": "wave_inconsistent_total",
                "wave_id": rejected.wave_id,
                "topic": rejected.topic,
                "actual": rejected.actual,
            }),
        ),
        ralph_core::WaveRejection::MissingIndex => (
            "wave_missing_index",
            serde_json::json!({
                "reason": "wave_missing_index",
                "wave_id": rejected.wave_id,
                "topic": rejected.topic,
            }),
        ),
        ralph_core::WaveRejection::IndexOutOfRange => (
            "wave_index_out_of_range",
            serde_json::json!({
                "reason": "wave_index_out_of_range",
                "wave_id": rejected.wave_id,
                "topic": rejected.topic,
                "actual": rejected.actual,
            }),
        ),
        ralph_core::WaveRejection::NoTargetHat => (
            "wave_no_target_hat",
            serde_json::json!({
                "reason": "wave_no_target_hat",
                "wave_id": rejected.wave_id,
                "topic": rejected.topic,
            }),
        ),
        ralph_core::WaveRejection::SequentialTarget => (
            "wave_sequential_target",
            serde_json::json!({
                "reason": "wave_sequential_target",
                "wave_id": rejected.wave_id,
                "topic": rejected.topic,
            }),
        ),
        ralph_core::WaveRejection::IsolatedScopeViolation { topic, isolated_hat, .. } => (
            "wave_isolated_scope_violation",
            serde_json::json!({
                "reason": "wave_isolated_scope_violation",
                "wave_id": rejected.wave_id,
                "topic": topic,
                "isolated_hat": isolated_hat,
            }),
        ),
        ralph_core::WaveRejection::IsolatedMultipleBusinessEmissions { isolated_hat, .. } => (
            "wave_isolated_multiple_business_emissions",
            serde_json::json!({
                "reason": "wave_isolated_multiple_business_emissions",
                "wave_id": rejected.wave_id,
                "topic": rejected.topic,
                "isolated_hat": isolated_hat,
            }),
        ),
    };

    // Surface to CLI for dogfooding visibility.
    if out.show_cli {
        eprintln!(
            "{} wave {} rejected ({}): {}",
            if out.use_colors { "\x1b[31m" } else { "" },
            rejected.wave_id,
            reason_code,
            structured_reason
        );
    }

    // Build the recovery envelope so the responder can escalate.
    // U4-B1: the Wave-specific retry key namespaced by `wave_id` so
    // that different rejected waves do not collapse into a single
    // finding. See plan §3 KTD-U4-3 / §5 B1.
    let retry_key = ralph_core::diagnosis::RecoveryDiagnosisEnvelopeBuilder::wave_retry_key(
        &rejected.wave_id,
        reason_code,
    );
    let envelope = ralph_core::diagnosis::RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::WaveDispatcher)
        .severity(DiagnosisSeverity::Error)
        .reason_code(reason_code)
        .message(format!(
            "Wave {} rejected before dispatch: {} (topic={}, actual={}, loop_id={})",
            rejected.wave_id, reason_code, rejected.topic, rejected.actual, loop_id
        ))
        .expected_action("Reduce wave fan-out or fix payload emission; see plan.blocked payload.")
        .topic(rejected.topic.clone())
        .retry_attempt(0)
        .safe_target(false)
        .outcome(DiagnosisOutcome::NotRetriable)
        .retry_key(retry_key)
        .build();
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());

    if let Some(diag) = diagnostics {
        diag.log_error(
            event_loop.state().iteration,
            "ralph-cli/wave-rejection",
            ralph_core::diagnostics::DiagnosticError::BackendError {
                backend: "ralph-cli/wave-rejection".to_string(),
                message: format!(
                    "wave {} rejected: {} (actual={}, cap={})",
                    rejected.wave_id, reason_code, rejected.actual, max_wave_total
                ),
            },
        );
    }

    // plan.blocked emission is deferred to U4: the recovery envelope
    // already captures the diagnostic; a future change can route the
    // blocked signal through EventLoop::publish_event with a
    // ralph_proto::Event once the U4 isolated-scope plumbing is in.

    // KTD-4 / §6 U2: publish a structured `plan.blocked` event ONLY
    // for `TotalExceedsCap` (the cap-overshoot path is the one that
    // must trigger shipper/reporter escalation). Other malformed
    // rejections are surfaced via the recovery envelope + diagnostics
    // only — they do not block the plan, just the malformed wave.
    if matches!(
        rejected.reason,
        ralph_core::WaveRejection::TotalExceedsCap { .. }
    ) {
        let plan_blocked_payload = structured_reason.to_string();
        let plan_blocked_event =
            ralph_proto::Event::new("plan.blocked", plan_blocked_payload);
        event_loop.publish_event(plan_blocked_event);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::EventLoop;
    use ralph_core::config::RalphConfig;
    use std::sync::{Arc, Mutex};

    fn build_event_loop() -> EventLoop {
        let yaml = r#"
hats: {}
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml parse");
        let mut el = EventLoop::new(config);
        el.initialize("u2-rejection-test");
        el
    }

    fn build_outputs_silent() -> WaveOutputs<'static> {
        // Show CLI off so the test does not pollute stderr with the
        // human-readable rejection notice.
        WaveOutputs {
            use_colors: false,
            show_cli: false,
            rpc_tx: None,
            tui: None,
        }
    }

    fn make_rejected(reason: ralph_core::WaveRejection) -> ralph_core::RejectedWave {
        ralph_core::RejectedWave {
            wave_id: "w-test-001".to_string(),
            topic: "review.wave.ready".to_string(),
            actual: 335,
            reason,
        }
    }

    /// KTD-4 / §6 U2: when a wave is rejected for exceeding the cap,
    /// the dispatcher MUST publish a structured `plan.blocked` event
    /// so the shipper/reporter hat can route the failure. One event
    /// per rejection — N events of the same wave produce one plan.blocked.
    #[tokio::test]
    async fn u2_total_exceeds_cap_publishes_plan_blocked() {
        let mut el = build_event_loop();

        // Observer captures everything published to the bus.
        let captured: Arc<Mutex<Vec<ralph_proto::Event>>> = Arc::new(Mutex::new(Vec::new()));
        let cap_clone = Arc::clone(&captured);
        el.add_observer(move |event: &ralph_proto::Event| {
            cap_clone.lock().unwrap().push(event.clone());
        });

        let rejected = make_rejected(ralph_core::WaveRejection::TotalExceedsCap {
            actual: 335,
            cap: 64,
        });
        let out = build_outputs_silent();

        handle_wave_rejection(&rejected, &mut el, &out, None, "test-loop", 64)
            .await
            .expect("rejection should not error");

        let blocked_events: Vec<_> = {
            let guard = captured.lock().unwrap();
            guard
                .iter()
                .filter(|e| e.topic.as_str() == "plan.blocked")
                .cloned()
                .collect()
        };
        assert_eq!(
            blocked_events.len(),
            1,
            "U2: TotalExceedsCap must publish exactly one plan.blocked, got {}",
            blocked_events.len()
        );

        // Payload must be a structured JSON object carrying the
        // typed reason — shipper/reporter route on these fields.
        let payload_str = blocked_events[0].payload.as_str();
        let payload: serde_json::Value =
            serde_json::from_str(payload_str).expect("plan.blocked payload must be JSON object");
        assert_eq!(payload["reason"], "wave_total_exceeds_cap");
        assert_eq!(payload["wave_id"], "w-test-001");
        assert_eq!(payload["topic"], "review.wave.ready");
        assert_eq!(payload["actual"], 335);
        assert_eq!(payload["cap"], 64);
    }

    /// KTD-4 / §6 U2: only `TotalExceedsCap` escalates to plan.blocked.
    /// Other malformed rejections (e.g. `ZeroTotal`, `InconsistentTopic`)
    /// only surface via the recovery envelope + diagnostics, so they
    /// do not block unrelated workflows.
    #[tokio::test]
    async fn u2_non_cap_rejections_do_not_publish_plan_blocked() {
        let cases = [
            ("ZeroTotal", ralph_core::WaveRejection::ZeroTotal),
            (
                "InconsistentTopic",
                ralph_core::WaveRejection::InconsistentTopic,
            ),
            (
                "NoTargetHat",
                ralph_core::WaveRejection::NoTargetHat,
            ),
        ];
        let out = build_outputs_silent();

        for (label, reason) in cases {
            let mut el = build_event_loop();
            let captured: Arc<Mutex<Vec<ralph_proto::Event>>> =
                Arc::new(Mutex::new(Vec::new()));
            let cap_clone = Arc::clone(&captured);
            el.add_observer(move |event: &ralph_proto::Event| {
                cap_clone.lock().unwrap().push(event.clone());
            });

            let rejected = make_rejected(reason);
            handle_wave_rejection(&rejected, &mut el, &out, None, "test-loop", 64)
                .await
                .unwrap_or_else(|e| panic!("rejection for {label} errored: {e}"));

            let blocked = captured
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.topic.as_str() == "plan.blocked");
            assert!(
                !blocked,
                "U2: {label} must NOT publish plan.blocked (only TotalExceedsCap does)"
            );
        }
    }

    /// U4-B1 / KTD-U4-3: end-to-end check that the recovery envelope
    /// recorded by `handle_wave_rejection` actually carries a
    /// wave-scoped retry key. Different `wave_id`s MUST produce
    /// different keys, even when the rejection reason is identical.
    #[tokio::test]
    async fn u4_b1_retry_key_is_wave_scoped() {
        use ralph_core::diagnostics::DiagnosticsCollector;

        let temp = tempfile::tempdir().expect("tempdir");
        let diagnostics_root = temp.path().to_path_buf();

        let yaml = r#"
hats: {}
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("yaml parse");
        let diagnostics = DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("diagnostics enabled");
        let mut el = EventLoop::with_diagnostics(config, diagnostics);
        el.initialize("u4-b1-retry-key");
        let out = build_outputs_silent();

        // Two distinct waves with the SAME rejection reason.
        let rejected_a = ralph_core::RejectedWave {
            wave_id: "w-A".to_string(),
            topic: "review.wave.ready".to_string(),
            actual: 335,
            reason: ralph_core::WaveRejection::TotalExceedsCap {
                actual: 335,
                cap: 64,
            },
        };
        let rejected_b = ralph_core::RejectedWave {
            wave_id: "w-B".to_string(),
            topic: "review.wave.ready".to_string(),
            actual: 335,
            reason: ralph_core::WaveRejection::TotalExceedsCap {
                actual: 335,
                cap: 64,
            },
        };

        handle_wave_rejection(&rejected_a, &mut el, &out, None, "test-loop", 64)
            .await
            .expect("rejection a");
        handle_wave_rejection(&rejected_b, &mut el, &out, None, "test-loop", 64)
            .await
            .expect("rejection b");

        // Read recovery.jsonl from the diagnostics session dir.
        let mut session_dirs: Vec<_> = std::fs::read_dir(
            diagnostics_root.join(".ralph/diagnostics"),
        )
        .expect("read diagnostics dir")
        .filter_map(Result::ok)
        .collect();
        session_dirs.sort_by_key(|entry| entry.path());
        let session_path = session_dirs
            .last()
            .expect("at least one diagnostics session")
            .path();
        let recovery_path = session_path.join("recovery.jsonl");
        let content = std::fs::read_to_string(&recovery_path)
            .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
        let entries: Vec<ralph_core::diagnosis::RecoveryJournalEntry> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("parse recovery entry"))
            .collect();

        assert_eq!(
            entries.len(),
            2,
            "two distinct rejections must produce two recovery entries"
        );

        let retry_keys: std::collections::HashSet<String> = entries
            .iter()
            .map(|e| e.envelope.retry_key.clone())
            .collect();
        assert_eq!(
            retry_keys.len(),
            2,
            "different wave_ids must produce different retry keys, got {:?}",
            retry_keys
        );
        for k in &retry_keys {
            assert!(
                k.starts_with("wave_dispatcher:"),
                "retry key must use the wave_dispatcher namespace, got: {k}"
            );
            assert!(
                k.ends_with(":wave_total_exceeds_cap"),
                "retry key must end with the reason code, got: {k}"
            );
        }
        // And each key must contain its own wave_id.
        let key_for_a = entries
            .iter()
            .find(|e| e.envelope.message.contains("Wave w-A rejected"))
            .expect("entry for w-A")
            .envelope
            .retry_key
            .clone();
        let key_for_b = entries
            .iter()
            .find(|e| e.envelope.message.contains("Wave w-B rejected"))
            .expect("entry for w-B")
            .envelope
            .retry_key
            .clone();
        assert!(
            key_for_a.contains("w_a"),
            "w-A key must contain normalized w-A, got: {key_for_a}"
        );
        assert!(
            key_for_b.contains("w_b"),
            "w-B key must contain normalized w-B, got: {key_for_b}"
        );
    }
}
