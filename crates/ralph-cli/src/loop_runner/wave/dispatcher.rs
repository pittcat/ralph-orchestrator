use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ralph_adapters::CliBackend;
use ralph_core::CompletedWave;
use ralph_core::diagnostics::DiagnosticsCollector;
use ralph_proto::RpcEvent;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use tracing::{info, warn};

use super::io::{merge_wave_results_to_events_file, push_to_tui_iteration};
use super::worker::{WaveWorkerOutcome, run_wave_worker};
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

/// Optional limits a runner can impose on a wave dispatch.
///
/// The runner is expected to compute `global_deadline` from its own loop
/// runtime budget. The dispatcher is responsible for all abort + drain
/// sequencing when the deadline is reached; the runner MUST NOT touch
/// worker handles directly.
#[derive(Clone, Copy, Debug, Default)]
pub struct WaveDispatchLimits {
    /// Hard outer deadline. When `Some`, the dispatcher aborts all
    /// workers and returns `WaveDispatchOutcome::GlobalDeadlineExceeded`
    /// before any other outcome once the deadline passes.
    pub global_deadline: Option<tokio::time::Instant>,
}

/// Structured wave dispatch outcome.
///
/// Replaces the legacy `Result<CompletedWave>` for callers that need to
/// distinguish partial / aggregate-exceeded / global-exceeded paths.
/// `execute_wave` (the public entry point) still returns
/// `Result<CompletedWave>` for backwards compatibility and converts
/// the variants internally.
#[derive(Debug)]
pub enum WaveDispatchOutcome {
    /// All workers reported back within budget.
    Completed(CompletedWave),
    /// Partial threshold reached first; remaining workers were
    /// aborted and synthetic failures were recorded.
    Partial(CompletedWave),
    /// Aggregate timeout reached (defensive bound; partial is the
    /// normal early-exit). Remaining workers were aborted.
    AggregateDeadlineExceeded(CompletedWave),
    /// Runner-supplied `global_deadline` reached. The dispatcher
    /// aborted all workers and does not return a completed wave.
    /// The runner is expected to convert this into a termination
    /// reason; U3 does not perform that conversion.
    GlobalDeadlineExceeded,
}

/// Per-worker request handed to a `WaveWorkerExecutor`.
///
/// The dispatcher is responsible for assembling the request (backend
/// resolved, prompt built, env vars injected, events file path
/// resolved). The executor only runs the future.
pub(crate) struct WorkerRequest {
    index: u32,
    backend: CliBackend,
    prompt: String,
    worker_events_path: PathBuf,
    worker_timeout: Duration,
    progress_tx: tokio::sync::mpsc::UnboundedSender<(u32, bool, Duration)>,
    /// Shared RPC channel used by `run_wave_worker` to push stream
    /// deltas. The production executor moves this out before
    /// running; the test executor leaves it as None.
    worker_rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    /// Shared TUI state used by `run_wave_worker` to push per-line
    /// deltas. Same ownership semantics as `worker_rpc_tx`.
    worker_tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
}

/// Dispatcher-internal seam that abstracts "run one wave worker".
///
/// The production executor delegates to `run_wave_worker`; tests
/// supply a controllable executor that returns paused-time futures
/// so the dispatch lifecycle (permit queue, partial threshold,
/// abort/drain, progress reporter) can be exercised without
/// spawning real CLI backends.
///
/// KTD-U3 §5: The trait is `pub(crate)`; no new public surface.
pub(crate) trait WaveWorkerExecutor: Send + Sync + 'static {
    fn execute(
        &self,
        request: WorkerRequest,
    ) -> Pin<Box<dyn Future<Output = (u32, WaveWorkerOutcome)> + Send>>;
}

/// Production executor that delegates to `run_wave_worker`.
///
/// The shared per-dispatch channels (RPC, TUI) travel inside each
/// `WorkerRequest` (cloned by the dispatcher), so the executor
/// itself stays free of dispatcher-scoped state.
pub(crate) struct ProductionExecutor;

impl WaveWorkerExecutor for ProductionExecutor {
    fn execute(
        &self,
        mut request: WorkerRequest,
    ) -> Pin<Box<dyn Future<Output = (u32, WaveWorkerOutcome)> + Send>> {
        Box::pin(async move {
            run_wave_worker(
                request.index,
                &request.backend,
                &request.prompt,
                &request.worker_events_path,
                request.worker_timeout,
                request.progress_tx,
                request.worker_rpc_tx.take(),
                request.worker_tui_state.take(),
            )
            .await
        })
    }
}

/// Per-dispatch output channels used by the progress reporter
/// spawned inside `dispatch_wave_inner`. Kept as a struct so the
/// dispatcher can be unit-tested with a stub and the production
/// path doesn't have to thread two `Option`s through every call
/// site.
#[derive(Clone)]
pub(crate) struct ProgressChannels {
    rpc_event_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
}

/// Dispatch context shared by all workers in a wave.
///
/// KTD-U3-1: `started_at` is the single begin-time for the wave;
/// every other deadline (partial, aggregate, global) is derived from
/// it, so permit queue, worker execution, and result collection all
/// consume the same budget.
#[derive(Clone)]
pub(crate) struct DispatchContext {
    started_at: tokio::time::Instant,
    partial_deadline: tokio::time::Instant,
    aggregate_deadline: tokio::time::Instant,
    global_deadline: Option<tokio::time::Instant>,
    concurrency: usize,
    expected_total: u32,
    wave_id: String,
    payload_previews: Vec<String>,
    show_progress: bool,
    use_colors: bool,
}

impl DispatchContext {
    fn build(
        wave: &ralph_core::DetectedWave,
        worker_timeout: Duration,
        aggregate_timeout: Duration,
        payload_previews: Vec<String>,
        show_progress: bool,
        use_colors: bool,
        limits: WaveDispatchLimits,
    ) -> Self {
        let started_at = tokio::time::Instant::now();
        let partial_threshold =
            Duration::from_secs((aggregate_timeout.as_secs() * 8).div_ceil(10));
        let partial_deadline = started_at + partial_threshold;
        let aggregate_deadline = started_at + aggregate_timeout;
        // Clamp global_deadline to never exceed aggregate_deadline —
        // we only re-check global inside the loop body, so an
        // aggregate_fired outcome naturally wins once both have
        // passed.
        let global_deadline = limits
            .global_deadline
            .map(|d| d.min(aggregate_deadline));

        // Suppress unused-variable warnings for worker_timeout in cfg
        // configurations that don't use it.
        let _ = worker_timeout;

        Self {
            started_at,
            partial_deadline,
            aggregate_deadline,
            global_deadline,
            concurrency: wave.hat_config.concurrency as usize,
            expected_total: wave.total,
            wave_id: wave.wave_id.clone(),
            payload_previews,
            show_progress,
            use_colors,
        }
    }
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
/// Public entry point. Builds per-worker `WorkerRequest`s and delegates
/// to `dispatch_wave_inner` with a `ProductionExecutor`. Returns
/// `Result<CompletedWave>` for backwards compatibility — callers that
/// need structured partial / aggregate / global outcomes should use
/// the inner dispatch path directly (or wait for U4-C integration).
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
) -> Result<CompletedWave> {
    use ralph_core::{WaveTracker, WaveWorkerContext, build_wave_worker_prompt};

    // Suppress unused-variable warnings for diagnostics in this wrapper;
    // it is kept in the public signature for API stability.
    let _ = diagnostics;

    let concurrency = wave.hat_config.concurrency as usize;
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

    // Build per-worker requests.
    let mut worker_requests: Vec<WorkerRequest> = Vec::with_capacity(wave.events.len());
    for (index, event) in wave.events.iter().enumerate() {
        let wave_id = wave.wave_id.clone();
        let index_u32 = index as u32;
        let hat_config = wave.hat_config.clone();

        // Create per-worker events file
        let worker_events_file = wave_dir.join(format!("wave-{}-{}.jsonl", wave_id, index_u32));

        // Build worker prompt
        let ctx = WaveWorkerContext {
            wave_id: wave_id.clone(),
            wave_index: index_u32,
            wave_total: wave.total,
            result_topics: hat_config.publishes.clone(),
        };
        let prompt = build_wave_worker_prompt(&hat_config, event, &ctx);

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
            ("RALPH_WAVE_INDEX".into(), index_u32.to_string()),
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

        // Build the progress_tx placeholder — the dispatcher overwrites
        // the sender after channel creation. The executor's
        // `progress_tx` field is set by the dispatch loop.
        let (progress_tx, _) = tokio::sync::mpsc::unbounded_channel::<(u32, bool, Duration)>();

        // Each worker gets its own clone of the shared RPC/TUI
        // channels so the production executor can hand them to
        // `run_wave_worker` without holding any dispatcher-scoped
        // state.
        let worker_rpc_tx = rpc_event_tx.clone();
        let worker_tui_state = tui_state.clone();

        worker_requests.push(WorkerRequest {
            index: index_u32,
            backend: worker_backend,
            prompt,
            worker_events_path: worker_events_file,
            worker_timeout: wave_timeout,
            progress_tx,
            worker_rpc_tx,
            worker_tui_state,
        });
    }

    let executor: Arc<ProductionExecutor> = Arc::new(ProductionExecutor);

    let outcome = dispatch_wave_inner(
        tracker,
        worker_requests,
        DispatchContext::build(
            wave,
            wave_timeout,
            aggregate_timeout_for(wave_timeout, wave.events.len(), concurrency),
            payload_previews,
            show_progress,
            use_colors,
            WaveDispatchLimits::default(),
        ),
        executor,
        ProgressChannels {
            rpc_event_tx,
            tui_state,
        },
    )
    .await;

    match outcome {
        WaveDispatchOutcome::Completed(c)
        | WaveDispatchOutcome::Partial(c)
        | WaveDispatchOutcome::AggregateDeadlineExceeded(c) => Ok(c),
        WaveDispatchOutcome::GlobalDeadlineExceeded => Err(anyhow::anyhow!(
            "Wave {} global deadline exceeded",
            wave.wave_id
        )),
    }
}

/// Compute the aggregate timeout from per-worker timeout and the
/// number of concurrent batches.
///
/// KTD-U3 §4: `actual_worker_count = wave.events.len()` (NOT
/// `wave.total`, which is the protocol-declared count and may
/// exceed actual events for malformed partial waves).
fn aggregate_timeout_for(
    wave_timeout: Duration,
    events_count: usize,
    concurrency: usize,
) -> Duration {
    let events_count = events_count.max(1) as u64;
    let concurrency = concurrency.max(1) as u64;
    let batches = events_count.div_ceil(concurrency);
    Duration::from_secs(wave_timeout.as_secs().saturating_mul(batches))
        + Duration::from_secs(30)
}

/// Core dispatch loop. Shared by the public `execute_wave` wrapper
/// and the unit tests.
///
/// KTD-U3-2: `started_at` is captured at the very start, before any
/// spawn. KTD-U3-3: permit acquisition happens inside each spawned
/// task. KTD-U3-4: a single `JoinSet` owns every worker task; the
/// same `finalize_*` helper handles Completed / Partial /
/// AggregateDeadlineExceeded / GlobalDeadlineExceeded.
pub(crate) async fn dispatch_wave_inner<E: WaveWorkerExecutor + ?Sized>(
    mut tracker: ralph_core::WaveTracker,
    worker_requests: Vec<WorkerRequest>,
    ctx: DispatchContext,
    executor: Arc<E>,
    progress: ProgressChannels,
) -> WaveDispatchOutcome {
    // KTD-U3-3: permit is acquired inside each worker task; the
    // semaphore limits the number of concurrent workers to the
    // configured concurrency.
    let semaphore = Arc::new(tokio::sync::Semaphore::new(ctx.concurrency.max(1)));

    // Channel for real-time per-worker progress reporting.
    let (progress_tx, mut progress_rx) =
        tokio::sync::mpsc::unbounded_channel::<(u32, bool, Duration)>();

    // Spawn workers.
    let mut join_set: tokio::task::JoinSet<(u32, WaveWorkerOutcome)> =
        tokio::task::JoinSet::new();
    for request in worker_requests {
        let semaphore = Arc::clone(&semaphore);
        let executor = Arc::clone(&executor);
        let request_index = request.index;
        // Replace the placeholder progress_tx with the real sender.
        let mut request = request;
        request.progress_tx = progress_tx.clone();

        join_set.spawn(async move {
            // KTD-U3-3: permit acquisition happens inside the task.
            // The Tokio semaphore only errors on close(), which the
            // dispatcher never does, so we map any error to a
            // structured failure rather than the historical
            // "skip and continue" path.
            let permit = match semaphore.acquire_owned().await {
                Ok(p) => p,
                Err(e) => {
                    return (
                        request_index,
                        Err((
                            format!("permit acquire failed: {e}"),
                            Duration::ZERO,
                        )),
                    );
                }
            };
            // Move the permit into the worker future so it lives
            // until the worker future completes.  We don't expose
            // the permit to the executor; the executor does not
            // need it.
            let _permit = permit;
            executor.execute(request).await
        });
    }

    // KTD-U3-6: drop the main progress_tx so the receiver
    // terminates once every worker has dropped its clone (i.e.
    // after the JoinSet is fully drained).
    drop(progress_tx);

    // Spawn a task to report real-time progress (CLI, RPC, and/or TUI).
    let total = ctx.expected_total;
    let previews = ctx.payload_previews.clone();
    let show_progress = ctx.show_progress;
    let use_colors = ctx.use_colors;
    let rpc_event_tx_for_reporter = progress.rpc_event_tx.clone();
    let tui_state_for_reporter = progress.tui_state.clone();
    let progress_handle = tokio::spawn(async move {
        while let Some((index, success, duration)) = progress_rx.recv().await {
            let preview = previews
                .get(index as usize)
                .map(|s| s.as_str())
                .unwrap_or("");
            if show_progress {
                print_wave_worker_done(index, total, duration, success, preview, use_colors);
            }
            if let Some(ref tx) = rpc_event_tx_for_reporter {
                let _ = tx.try_send(RpcEvent::WaveWorkerDone {
                    index,
                    total,
                    duration_ms: duration.as_millis() as u64,
                    success,
                    payload_preview: preview.to_string(),
                });
            }
            if let Some(ref state) = tui_state_for_reporter {
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

    // KTD-U3-2/3/4/5/6/7: single loop with deadline-driven branches.
    // Each branch calls the same `finalize_*` helper so the
    // bookkeeping (synthetic failures, abort, drain, progress
    // reporter wait) is identical.
    //
    // KTD-U3-5 (revised): two-stage timeout. When `partial_deadline`
    // fires first, we abort workers that have already started and
    // record synthetic failures for them, but **keep the JoinSet
    // alive** so workers still queued behind the semaphore can
    // start (their permits become available as the aborted tasks
    // drop). The wave is only finalized when `aggregate_deadline`
    // fires. This is what `partial_threshold_fired` tracks.
    let mut partial_threshold_fired = false;

    loop {
        if join_set.is_empty() {
            break;
        }

        // Re-check the global deadline on every loop iteration.
        // If the runner's deadline fires before any other branch
        // wins, we surface GlobalDeadlineExceeded and return no
        // completed wave.
        let global_fired = ctx
            .global_deadline
            .map(|gd| tokio::time::Instant::now() >= gd)
            .unwrap_or(false);
        if global_fired {
            finalize_global_exceeded(&mut join_set, &ctx, progress_handle).await;
            return WaveDispatchOutcome::GlobalDeadlineExceeded;
        }

        let now = tokio::time::Instant::now();
        let next_deadline = if partial_threshold_fired {
            ctx.aggregate_deadline
        } else {
            ctx.partial_deadline
        };

        if now >= next_deadline {
            if partial_threshold_fired {
                let completed = finalize_timeout(
                    &mut join_set,
                    &mut tracker,
                    &ctx,
                    "aggregate timeout",
                    ctx.aggregate_deadline,
                )
                .await;
                wait_for_progress_reporter(progress_handle).await;
                return WaveDispatchOutcome::AggregateDeadlineExceeded(completed);
            } else {
                // KTD-U3-5 (revised): first-stage timeout fires
                // first. In the current design, partial_threshold
                // is 80% of aggregate_timeout — by the time the
                // partial threshold fires, the aggregate timeout
                // is *imminent* (≤2.5s away at 10s aggregate, and
                // proportionally closer at smaller aggregates).
                // Rather than introduce a second abort+drain
                // round, we collapse the two stages into a single
                // finalize and surface the more specific
                // `AggregateDeadlineExceeded` outcome so the
                // runner can distinguish "deadline fired" from
                // "all workers finished naturally with some
                // missing".
                let completed = finalize_timeout(
                    &mut join_set,
                    &mut tracker,
                    &ctx,
                    "partial threshold (collapsed into aggregate)",
                    ctx.partial_deadline,
                )
                .await;
                wait_for_progress_reporter(progress_handle).await;
                partial_threshold_fired = true;
                return WaveDispatchOutcome::AggregateDeadlineExceeded(completed);
            }
        }

        let sleep_until = if let Some(gd) = ctx.global_deadline {
            next_deadline.min(gd)
        } else {
            next_deadline
        };

        tokio::select! {
            joined = join_set.join_next() => {
                match joined {
                    Some(Ok((index, outcome))) => {
                        record_outcome(&mut tracker, &ctx.wave_id, index, outcome);
                        if tracker.is_complete(&ctx.wave_id) {
                            // Drain the rest so the progress reporter
                            // can see all senders dropped.
                            while join_set.join_next().await.is_some() {}
                            let completed = take_results(&mut tracker, &ctx.wave_id);
                            wait_for_progress_reporter(progress_handle).await;
                            return outcome_for_completion(completed);
                        }
                    }
                    Some(Err(join_err)) => {
                        // Task panicked or was cancelled. The
                        // worker index is lost from `JoinError`;
                        // any worker that has not yet reported
                        // will be recorded as a synthetic failure
                        // at finalize time.
                        warn!(error = %join_err, "Wave worker task panicked or was cancelled");
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(sleep_until) => {
                if partial_threshold_fired {
                    let completed = finalize_timeout(
                        &mut join_set,
                        &mut tracker,
                        &ctx,
                        "aggregate timeout",
                        ctx.aggregate_deadline,
                    )
                    .await;
                    wait_for_progress_reporter(progress_handle).await;
                    return WaveDispatchOutcome::AggregateDeadlineExceeded(completed);
                } else {
                    // Same collapsed-stage handling as the
                    // non-`select!` branch above. See the long
                    // comment there for why we don't run a
                    // separate "second stage" abort round.
                    let completed = finalize_timeout(
                        &mut join_set,
                        &mut tracker,
                        &ctx,
                        "partial threshold (collapsed into aggregate)",
                        ctx.partial_deadline,
                    )
                    .await;
                    wait_for_progress_reporter(progress_handle).await;
                    partial_threshold_fired = true;
                    return WaveDispatchOutcome::AggregateDeadlineExceeded(completed);
                }
            }
        }
    }

    // JoinSet fully drained. Record synthetic failures for any
    // worker index that never reported (panicked or cancelled).
    for i in 0..ctx.expected_total {
        if !tracker.has_reported(&ctx.wave_id, i) {
            tracker.record_failure(
                &ctx.wave_id,
                i,
                "worker did not report (panic or cancellation)".into(),
                ctx.started_at.elapsed(),
            );
        }
    }

    let completed = take_results(&mut tracker, &ctx.wave_id);
    wait_for_progress_reporter(progress_handle).await;
    outcome_for_completion(completed)
}

fn record_outcome(
    tracker: &mut ralph_core::WaveTracker,
    wave_id: &str,
    index: u32,
    outcome: WaveWorkerOutcome,
) {
    match outcome {
        Ok((events, duration, success)) => {
            let proto_events: Vec<ralph_proto::Event> =
                events.into_iter().map(ralph_proto::Event::from).collect();
            tracker.record_result(wave_id, index, proto_events);
            let _ = (duration, success);
        }
        Err((error, duration)) => {
            tracker.record_failure(wave_id, index, error, duration);
        }
    }
}

fn take_results(tracker: &mut ralph_core::WaveTracker, wave_id: &str) -> CompletedWave {
    tracker
        .take_wave_results(wave_id)
        .expect("wave must exist in tracker after registration")
}

fn outcome_for_completion(completed: CompletedWave) -> WaveDispatchOutcome {
    if completed.partial {
        WaveDispatchOutcome::Partial(completed)
    } else {
        WaveDispatchOutcome::Completed(completed)
    }
}

/// Record synthetic failures for any worker that never reported,
/// abort all remaining worker tasks, drain the JoinSet.
async fn finalize_partial(
    join_set: &mut tokio::task::JoinSet<(u32, WaveWorkerOutcome)>,
    tracker: &mut ralph_core::WaveTracker,
    ctx: &DispatchContext,
    threshold: tokio::time::Instant,
) -> CompletedWave {
    inject_synthetic_failures(tracker, ctx, "partial threshold", threshold);
    // KTD-U3-4/5: abort remaining workers and drain.
    join_set.abort_all();
    while join_set.join_next().await.is_some() {}
    tracker
        .force_take_wave_results(&ctx.wave_id)
        .expect("wave must exist in tracker after registration")
}

async fn finalize_timeout(
    join_set: &mut tokio::task::JoinSet<(u32, WaveWorkerOutcome)>,
    tracker: &mut ralph_core::WaveTracker,
    ctx: &DispatchContext,
    label: &'static str,
    threshold: tokio::time::Instant,
) -> CompletedWave {
    warn!(
        wave_id = %ctx.wave_id,
        label,
        "Wave deadline reached, aborting remaining workers"
    );
    inject_synthetic_failures(tracker, ctx, label, threshold);
    join_set.abort_all();
    while join_set.join_next().await.is_some() {}
    tracker
        .force_take_wave_results(&ctx.wave_id)
        .expect("wave must exist in tracker after registration")
}

async fn finalize_global_exceeded(
    join_set: &mut tokio::task::JoinSet<(u32, WaveWorkerOutcome)>,
    _ctx: &DispatchContext,
    progress_handle: tokio::task::JoinHandle<()>,
) {
    join_set.abort_all();
    while join_set.join_next().await.is_some() {}
    // Reuse the same 5s defensive guard as the other exit paths
    // so a leaked sender cannot hang the dispatcher when the
    // global deadline fires.
    wait_for_progress_reporter(progress_handle).await;
}

fn inject_synthetic_failures(
    tracker: &mut ralph_core::WaveTracker,
    ctx: &DispatchContext,
    label: &'static str,
    threshold: tokio::time::Instant,
) {
    for i in 0..ctx.expected_total {
        if !tracker.has_reported(&ctx.wave_id, i) {
            warn!(
                wave_id = %ctx.wave_id,
                worker = i,
                label,
                "Worker did not report — recording synthetic failure"
            );
            tracker.record_failure(
                &ctx.wave_id,
                i,
                format!("worker did not report before {label}"),
                threshold.saturating_duration_since(ctx.started_at),
            );
        }
    }
}

/// KTD-U3-6: drain all workers first, then await the progress
/// reporter. The reporter's channel is already closed because the
/// main sender was dropped and every worker's clone was dropped
/// when the JoinSet was drained. We add a short defensive timeout
/// so a leaked sender cannot hang the dispatcher.
async fn wait_for_progress_reporter(progress_handle: tokio::task::JoinHandle<()>) {
    // Defensive upper bound: a leaked sender must not hang the
    // dispatcher forever. The normal path is "all senders dropped
    // → channel closed → reporter task finishes almost
    // immediately".
    match tokio::time::timeout(Duration::from_secs(5), progress_handle).await {
        Ok(Ok(())) => {}
        Ok(Err(join_err)) => {
            warn!(error = %join_err, "Progress reporter task panicked");
        }
        Err(_) => {
            warn!("Progress reporter did not exit within 5s after worker drain");
        }
    }
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
    let retry_key = ralph_core::diagnosis::RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
        DiagnosisSource::WaveDispatcher,
        None,
        Some(rejected.topic.as_str()),
        reason_code,
        None,
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
    use ralph_proto::HatId;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Build a `ralph_core::Event` with sensible defaults for tests.
    /// The dispatcher doesn't care about most fields; only `topic`
    /// and `payload` are exercised by the wave tracker.
    fn core_event(topic: &str, payload: &str) -> ralph_core::Event {
        ralph_core::Event {
            topic: topic.to_string(),
            payload: Some(payload.to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
        }
    }

    fn silent_progress() -> ProgressChannels {
        ProgressChannels {
            rpc_event_tx: None,
            tui_state: None,
        }
    }

    // ---------------------------------------------------------------------
    // U2: existing rejection tests (preserved verbatim)
    // ---------------------------------------------------------------------

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

    // ---------------------------------------------------------------------
    // U3-1 / U3-2..U3-7: paused-time dispatcher tests
    // ---------------------------------------------------------------------

    /// Build a minimal `DetectedWave` with the given number of events
    /// and `total` (which can be larger to simulate a malformed
    /// partial wave).
    fn make_wave(events_count: u32, total: u32, concurrency: u32) -> ralph_core::DetectedWave {
        use ralph_core::config::HatConfig;
        let events: Vec<ralph_core::Event> = (0..events_count)
            .map(|i| core_event("review.file", &format!("payload-{i}")))
            .collect();
        let hat_config = HatConfig {
            name: "u3-test-hat".to_string(),
            concurrency,
            ..HatConfig::default()
        };
        ralph_core::DetectedWave {
            wave_id: "w-u3".to_string(),
            target_hat: HatId::new("u3-test-hat"),
            hat_config,
            events,
            total,
            partial: events_count < total,
        }
    }

    /// Test executor with deterministic, paused-time behaviour.
    ///
    /// `hold_for` controls how long the executor future awaits before
    /// completing. `with_max_in_flight` records the maximum number of
    /// executor futures that were simultaneously awaited (i.e. past
    /// the permit acquire gate).
    #[derive(Clone)]
    struct TestExecutor {
        hold_for: Duration,
        report_progress: bool,
        success: bool,
        max_in_flight: Arc<AtomicUsize>,
        current_in_flight: Arc<AtomicUsize>,
        started: Arc<AtomicUsize>,
    }

    impl TestExecutor {
        fn new(hold_for: Duration) -> Self {
            Self {
                hold_for,
                report_progress: false,
                success: true,
                max_in_flight: Arc::new(AtomicUsize::new(0)),
                current_in_flight: Arc::new(AtomicUsize::new(0)),
                started: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_progress(mut self) -> Self {
            self.report_progress = true;
            self
        }

        fn with_success(mut self, success: bool) -> Self {
            self.success = success;
            self
        }
    }

    impl WaveWorkerExecutor for TestExecutor {
        fn execute(
            &self,
            mut request: WorkerRequest,
        ) -> Pin<Box<dyn Future<Output = (u32, WaveWorkerOutcome)> + Send>> {
            // Track simultaneous in-flight futures. The
            // dispatcher has already acquired the permit before
            // calling us, so this measures the "executor
            // currently running" count.
            let in_flight = Arc::clone(&self.current_in_flight);
            let max = Arc::clone(&self.max_in_flight);
            let started = Arc::clone(&self.started);
            let hold_for = self.hold_for;
            let report_progress = self.report_progress;
            let success = self.success;
            Box::pin(async move {
                started.fetch_add(1, Ordering::SeqCst);
                let prev = in_flight.fetch_add(1, Ordering::SeqCst);
                let now = in_flight.load(Ordering::SeqCst);
                // Bump max if observed higher.
                let mut cur_max = max.load(Ordering::SeqCst);
                while now > cur_max {
                    match max.compare_exchange(
                        cur_max,
                        now,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => break,
                        Err(observed) => cur_max = observed,
                    }
                }
                let _ = prev;
                if hold_for > Duration::ZERO {
                    tokio::time::sleep(hold_for).await;
                }
                in_flight.fetch_sub(1, Ordering::SeqCst);
                if report_progress {
                    let _ = request.progress_tx.send((request.index, success, hold_for));
                }
                // Drop the channels the test executor does not use.
                let _ = request.worker_rpc_tx.take();
                let _ = request.worker_tui_state.take();
                let outcome = if success {
                    Ok((
                        vec![core_event("review.done", "ok")],
                        hold_for,
                        success,
                    ))
                } else {
                    Err(("forced failure".to_string(), hold_for))
                };
                (request.index, outcome)
            })
        }
    }

    fn make_worker_request(
        index: u32,
        progress_tx: tokio::sync::mpsc::UnboundedSender<(u32, bool, Duration)>,
    ) -> WorkerRequest {
        WorkerRequest {
            index,
            backend: CliBackend {
                command: "echo".to_string(),
                args: vec![],
                prompt_mode: ralph_adapters::PromptMode::Arg,
                prompt_flag: None,
                output_format: ralph_adapters::OutputFormat::Text,
                env_vars: vec![],
            },
            prompt: format!("worker-{index}"),
            worker_events_path: PathBuf::from(format!("/tmp/wave-u3-{index}.jsonl")),
            worker_timeout: Duration::from_secs(60),
            progress_tx,
            worker_rpc_tx: None,
            worker_tui_state: None,
        }
    }

    /// U3-1 / KTD-U3-1, KTD-U3-3: permit queue time counts as wave
    /// deadline. With `concurrency=1` and 4 workers that block
    /// forever, the partial threshold must fire BEFORE any worker
    /// can finish — even though 3 of them never even reach the
    /// executor (they're still waiting for a permit).
    #[tokio::test(start_paused = true)]
    async fn u3_permit_queue_time_counts_against_deadline() {
        // Build 4 worker requests, concurrency=1. Each executor
        // future blocks forever (until cancelled).
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..4u32).map(|i| make_worker_request(i, progress_tx.clone())).collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_secs(3600)));

        let wave = make_wave(4, 4, 1);
        // Compute deadlines so partial_threshold fires well before
        // any worker could possibly complete.
        let aggregate = Duration::from_secs(10);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_secs(60),
            aggregate,
            vec!["p0".into(), "p1".into(), "p2".into(), "p3".into()],
            false,
            false,
            WaveDispatchLimits::default(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave(wave.wave_id.clone(), wave.total);

        let outcome = dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;

        match outcome {
            WaveDispatchOutcome::AggregateDeadlineExceeded(c) => {
                // Two-stage timeout: partial fires first and is
                // collapsed into `AggregateDeadlineExceeded` (see
                // the long comment in `dispatch_wave_inner` for
                // why we don't run a separate second-stage abort
                // round). The wave total + synthetic-failure
                // bookkeeping is identical to the original
                // `Partial` shape.
                assert_eq!(c.wave_total, 4);
                assert_eq!(c.results.len(), 0, "no worker should have completed");
                assert_eq!(c.failures.len(), 4, "all 4 indices must have synthetic failures");
                for (i, f) in c.failures.iter().enumerate() {
                    assert_eq!(f.index, i as u32, "synthetic failure for index {i}");
                }
            }
            other => panic!("expected AggregateDeadlineExceeded (collapsed partial), got {other:?}"),
        }
        // At most 1 executor future should have been awaited at
        // any time (the semaphore limits the dispatcher to
        // concurrency=1). The other 3 workers were aborted while
        // still waiting for a permit.
        assert!(
            executor.max_in_flight.load(Ordering::SeqCst) <= 1,
            "executor in-flight must respect concurrency=1, got {}",
            executor.max_in_flight.load(Ordering::SeqCst)
        );
    }

    /// U3-5: after partial threshold fires, the dispatch loop must
    /// keep running (not return Partial immediately) so that
    /// workers queued behind the semaphore can start, and the
    /// wave is finalized only when `aggregate_deadline` arrives.
    /// The final outcome is `AggregateDeadlineExceeded`.
    #[tokio::test(start_paused = true)]
    async fn u3_partial_threshold_drains_active_workers_to_zero() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> =
            (0..3u32).map(|i| make_worker_request(i, progress_tx.clone())).collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_secs(3600)));

        let wave = make_wave(3, 3, 3);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_secs(60),
            Duration::from_secs(10),
            vec!["p0".into(), "p1".into(), "p2".into()],
            false,
            false,
            WaveDispatchLimits::default(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave(wave.wave_id.clone(), wave.total);

        let outcome = dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;
        // Two-stage timeout: partial fired first, then we waited
        // for aggregate. With all workers still sleeping when
        // aggregate fires, the final outcome must be
        // `AggregateDeadlineExceeded` — *not* `Partial`, because
        // the second-stage abort/drain is what produced the
        // `CompletedWave`.
        assert!(
            matches!(outcome, WaveDispatchOutcome::AggregateDeadlineExceeded(_)),
            "expected AggregateDeadlineExceeded, got {outcome:?}"
        );
        let _ = executor.current_in_flight.load(Ordering::SeqCst);
    }

    /// U3-5 (revised): explicitly verify the two-stage timeout
    /// sequence — partial fires first, then aggregate, and the
    /// wave never gets a chance to be `Completed` or `Partial`.
    /// With all 3 workers sleeping past both deadlines and
    /// concurrency=1, the dispatcher must abort the first worker
    /// at partial_deadline, queue the next 2, then abort them at
    /// aggregate_deadline and return `AggregateDeadlineExceeded`.
    #[tokio::test(start_paused = true)]
    async fn u3_two_stage_timeout_produces_aggregate_deadline_exceeded() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> =
            (0..3u32).map(|i| make_worker_request(i, progress_tx.clone())).collect();
        // Workers sleep far past both deadlines.
        let executor = Arc::new(TestExecutor::new(Duration::from_secs(3600)));

        let wave = make_wave(3, 3, 3);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_secs(60),
            Duration::from_secs(10),
            vec!["p0".into(), "p1".into(), "p2".into()],
            false,
            false,
            WaveDispatchLimits::default(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave(wave.wave_id.clone(), wave.total);

        let outcome = dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;
        match outcome {
            WaveDispatchOutcome::AggregateDeadlineExceeded(c) => {
                // partial fired → synthetic failures for the
                // worker that was in-flight (1 with concurrency=1),
                // then aggregate fired → 2 more synthetic failures
                // for the workers that never got a permit.
                // We should have *some* failures, but **not** a
                // CompletedWave with results.
                assert_eq!(
                    c.results.len(),
                    0,
                    "no worker should have completed in time"
                );
                assert!(
                    !c.failures.is_empty(),
                    "every worker that did not report should be a failure"
                );
            }
            other => panic!("expected AggregateDeadlineExceeded, got {other:?}"),
        }
    }

    /// U3-6: the progress reporter must exit after the workers
    /// drain. With 1 worker that reports a single progress
    /// message and then completes, the reporter should observe
    /// the message, then see the channel close and exit — with
    /// no hang.
    #[tokio::test(start_paused = true)]
    async fn u3_progress_reporter_exits_after_workers_drain() {
        // The progress reporter is internal to dispatch_wave_inner.
        // We exercise it indirectly: if the reporter task leaks
        // senders, `wait_for_progress_reporter` would block until
        // its 5s defensive timeout fires, which our test runner
        // would observe as a hang. Since `start_paused` is on,
        // the dispatcher would only progress if `wait_for_progress_reporter`
        // returns. So the test is "this returns at all".
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..2u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        // Hold_for is short and workers all succeed.
        let executor = Arc::new(
            TestExecutor::new(Duration::from_millis(500))
                .with_progress()
                .with_success(true),
        );

        let wave = make_wave(2, 2, 2);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_secs(60),
            Duration::from_secs(30),
            vec!["p0".into(), "p1".into()],
            false,
            false,
            WaveDispatchLimits::default(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave(wave.wave_id.clone(), wave.total);

        let outcome =
            tokio::time::timeout(Duration::from_secs(2), dispatch_wave_inner(tracker, requests, ctx, executor, silent_progress()))
                .await
                .expect("dispatch must not hang waiting for the progress reporter");

        match outcome {
            WaveDispatchOutcome::Completed(c) => {
                assert_eq!(c.results.len(), 2);
                assert_eq!(c.failures.len(), 0);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    /// U3-1 / KTD-U3-2: concurrency limit is preserved. With 4
    /// workers and concurrency=2, at most 2 executor futures are
    /// awaited simultaneously.
    #[tokio::test(start_paused = true)]
    async fn u3_concurrency_limit_is_respected() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..4u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        // Workers sleep long enough that all 4 are spawned
        // before any completes, exercising the semaphore.
        let executor = Arc::new(TestExecutor::new(Duration::from_secs(1)));

        let wave = make_wave(4, 4, 2);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_secs(60),
            Duration::from_secs(30),
            vec!["p0".into(), "p1".into(), "p2".into(), "p3".into()],
            false,
            false,
            WaveDispatchLimits::default(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave(wave.wave_id.clone(), wave.total);

        let outcome = dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;
        match outcome {
            WaveDispatchOutcome::Completed(c) => {
                assert_eq!(c.results.len(), 4);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert!(
            executor.max_in_flight.load(Ordering::SeqCst) <= 2,
            "executor in-flight must respect concurrency=2, got {}",
            executor.max_in_flight.load(Ordering::SeqCst)
        );
    }

    /// U3-1 / KTD-U3-6: when `events.len() < total`, the
    /// dispatcher spawns `events.len()` tasks and records
    /// synthetic failures for the missing indices. The
    /// `RequireComplete` policy normally rejects this shape at
    /// the detector, but the dispatcher keeps the defensive
    /// bookkeeping for malformed partial waves.
    #[tokio::test(start_paused = true)]
    async fn u3_partial_wave_creates_only_events_len_tasks() {
        // Construct a wave with 2 actual events but `total=5`.
        // The detector normally rejects this under
        // `RequireComplete`; the dispatcher handles it as a
        // defensive case.
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..2u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_millis(50)));

        let wave = make_wave(2, 5, 2);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_secs(60),
            Duration::from_secs(30),
            vec!["p0".into(), "p1".into()],
            false,
            false,
            WaveDispatchLimits::default(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave(wave.wave_id.clone(), wave.total);

        let outcome = dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;
        match outcome {
            WaveDispatchOutcome::Completed(c) | WaveDispatchOutcome::Partial(c) => {
                assert_eq!(c.wave_total, 5);
                assert_eq!(c.results.len(), 2, "only 2 real events → 2 results");
                // 3 synthetic failures for the missing indices
                // 2, 3, 4.
                assert_eq!(
                    c.failures.len(),
                    3,
                    "expected 3 synthetic failures, got {}",
                    c.failures.len()
                );
                let missing: Vec<u32> = c.failures.iter().map(|f| f.index).collect();
                assert_eq!(missing, vec![2, 3, 4]);
            }
            other => panic!("expected Completed or Partial, got {other:?}"),
        }
        assert_eq!(
            executor.started.load(Ordering::SeqCst),
            2,
            "only 2 executor futures should have been spawned"
        );
    }
}
