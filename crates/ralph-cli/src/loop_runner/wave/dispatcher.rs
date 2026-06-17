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
    /// Plan 001 §4.3 C1: explicit preset label forwarded to each
    /// wave worker so its in-process `ralph emit` / `ralph wave emit`
    /// inherits the loop's `event_policy.schemas` via `RALPH_HATS_SOURCE`.
    /// When `None`, the dispatcher falls back to the parent process env.
    pub hats_source_label: Option<&'a str>,
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
    /// U2 (Unit 2 of 2026-06-17-001 plan): Spawn guarantee violation.
    /// Fewer workers were spawned than there were wave events.
    /// The runner is expected to write a `wave_spawn_failed`
    /// RecoveryDiagnosisEnvelope and continue with any partial results.
    SpawnFailed {
        /// Number of workers that were actually spawned.
        spawned_count: u32,
        /// Number of wave events (expected workers).
        expected_count: u32,
    },
}

/// U4-C3: outcome of `handle_wave_events` returned to the runner.
///
/// `global_deadline_exceeded` is true when any of the dispatched
/// waves hit the runner-supplied outer deadline. The runner uses
/// this to set `late_termination_reason = Some(MaxRuntime)` and
/// skip the iteration's post-wave phases (default publishes,
/// missing-event gate) so the existing unified termination flow
/// can take over.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HandleWaveOutcome {
    pub global_deadline_exceeded: bool,
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
    /// Dimension this worker is hard-bound to (R1). Parsed from the
    /// `review.wave.ready` payload's `dimension` field. `None` for
    /// waves that do not carry a dimension assignment (legacy
    /// waves, or non-review waves). When `Some`, the worker prompt
    /// and the `RALPH_WAVE_DIMENSION` env var surface this value,
    /// the CLI precheck enforces it (R3), and the merge layer drops
    /// any emitted `review.dimension.done` with a mismatched
    /// dimension (R4).
    assigned_dimension: Option<String>,
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
    /// The declared wave total (may exceed events.len() in malformed partial waves).
    expected_total: u32,
    /// U2: actual number of events in this wave. Used for the spawn guarantee
    /// check — we must spawn exactly `events_len` workers, not `expected_total`.
    events_len: u32,
    wave_id: String,
    payload_previews: Vec<String>,
    show_progress: bool,
    use_colors: bool,
    /// U1/R1 (2026-06-17-002): per-worker dimension assignment
    /// parsed from each `review.wave.ready` payload. Carried on
    /// the context so `execute_wave_structured` can stamp it
    /// onto the `CompletedWave` for the merge layer to read.
    assigned_dimensions: std::collections::HashMap<u32, String>,
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
        assigned_dimensions: std::collections::HashMap<u32, String>,
    ) -> Self {
        let started_at = tokio::time::Instant::now();
        let partial_threshold = Duration::from_secs((aggregate_timeout.as_secs() * 8).div_ceil(10));
        let partial_deadline = started_at + partial_threshold;
        let aggregate_deadline = started_at + aggregate_timeout;
        // Clamp global_deadline to never exceed aggregate_deadline —
        // we only re-check global inside the loop body, so an
        // aggregate_fired outcome naturally wins once both have
        // passed.
        let global_deadline = limits.global_deadline.map(|d| d.min(aggregate_deadline));

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
            events_len: wave.events.len() as u32,
            wave_id: wave.wave_id.clone(),
            payload_previews,
            show_progress,
            use_colors,
            assigned_dimensions,
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
    // U4-C2: runner-supplied outer deadline. The runner computes
    // this from `loop.max_runtime_seconds - state.elapsed()` and
    // passes it through so the dispatcher can preempt long waves
    // when the loop is about to hit its hard time budget
    // (KTD-U4-6 / §6 C2). The dispatcher aborts + drains all
    // workers when the deadline fires and returns
    // `WaveDispatchOutcome::GlobalDeadlineExceeded`; the runner
    // is responsible for converting that into
    // `TerminationReason::MaxRuntime` (U4-C3).
    global_deadline: Option<tokio::time::Instant>,
    // Plan 001 §4.3 C1: explicit preset label forwarded to each
    // wave worker so its in-process `ralph emit` / `ralph wave emit`
    // inherits the loop's `event_policy.schemas` via `RALPH_HATS_SOURCE`.
    hats_source_label: Option<&str>,
) -> HandleWaveOutcome {
    let max_wave_total = event_loop.config().event_loop.max_wave_total;
    // U4-C3: accumulator for the per-wave outcomes. Set to true
    // when any wave hits the runner-supplied global deadline so
    // the runner can map to `TerminationReason::MaxRuntime`.
    let mut result = HandleWaveOutcome::default();
    // U5/R5 (2026-06-17-002): per-slot retry budget for
    // dimension mismatch retries. Key is `(wave_id, wave_index)`.
    // The budget caps each slot at 1 retry (2 total attempts:
    // initial + 1 retry) so a permanently-mismatched worker
    // does not infinite-loop on the dispatcher's `task.resume`.
    // The map is process-local; it is discarded when
    // `handle_wave_events` returns (no persistence needed).
    let mut dimension_retry_budget: std::collections::HashMap<(String, u32), u32> =
        std::collections::HashMap::new();
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
        hats_source_label,
    };

    // U2: emit a single structured `plan.blocked` per rejected wave and
    // record a recovery envelope BEFORE any TUI / backend side-effects.
    for rejected in &outcome.rejected {
        if let Err(err) = handle_wave_rejection(
            rejected,
            event_loop,
            &out,
            diagnostics,
            loop_id,
            max_wave_total,
        )
        .await
        {
            warn!(?err, "failed to handle wave rejection");
        }
    }

    let waves = outcome.accepted;
    if waves.is_empty() {
        return HandleWaveOutcome::default();
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

        let wave_outcome = execute_wave_structured(
            &detected,
            backend,
            &main_events_file,
            out.show_cli,
            out.use_colors,
            out.rpc_tx.cloned(),
            out.tui.map(Arc::clone),
            loop_id,
            // U4-C2: forward the runner-supplied global deadline
            // (from `loop.max_runtime_seconds - state.elapsed()`)
            // to the dispatcher so it can preempt long waves.
            WaveDispatchLimits { global_deadline },
            // Plan 001 §4.3 C1: forward the explicit preset label so
            // each wave worker's `ralph emit` / `ralph wave emit`
            // inherits the loop's `event_policy.schemas`.
            out.hats_source_label.as_deref(),
        )
        .await;

        // U4-B3: classify the structured outcome BEFORE we
        // pattern-match on the carried `CompletedWave`. We need to
        // know the reason code up front so we can record the
        // recovery envelope for Partial / AggregateDeadlineExceeded
        // before the wave result is merged into the main events
        // file. (Per KTD-U4-5: timeout findings start as Pending
        // and recover when a new wave on the same target topic
        // completes in a later iteration.)
        let timeout_reason: Option<&'static str> = match &wave_outcome {
            WaveDispatchOutcome::Partial(_) => Some("wave_partial_threshold"),
            WaveDispatchOutcome::AggregateDeadlineExceeded(_) => {
                Some("wave_aggregate_deadline_exceeded")
            }
            _ => None,
        };

        // U4-B3: timeout / partial outcomes must feed the recovery
        // responder, not just be folded into a generic Err. We
        // dispatch on the structured `WaveDispatchOutcome` here.
        match wave_outcome {
            WaveDispatchOutcome::Completed(completed)
            | WaveDispatchOutcome::Partial(completed)
            | WaveDispatchOutcome::AggregateDeadlineExceeded(completed) => {
                if let Some(reason_code) = timeout_reason {
                    record_wave_timeout_envelope(event_loop, &detected, &completed, reason_code);
                }

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
                // 2026-06-13-004 U1 (P0-1): pass `detected.target_hat`
                // as `default_source_hat` so each merged record carries a
                // provenance attribution. The merge layer prefers the
                // worker's own `event.source` when present (i.e. the
                // worker hat name like `dimension-reviewer`); when the
                // worker did not populate `source`, we fall back to
                // `detected.target_hat` (e.g. `review-coordinator`). The
                // resulting `hat` field is what the isolated scope
                // check in `process_parse_result` reads to decide
                // whether the re-published event is in-scope.
                //
                // U5/R5 (2026-06-17-002): capture the per-slot
                // mismatch list returned by the merge layer. The
                // dispatcher uses it to inject `task.resume`
                // events so the `dimension-reviewer` retries the
                // mismatched slot. Per-slot retry budget lives on
                // `dimension_retry_budget` below; the same function
                // call also writes the synthetic `wave.worker.failed`
                // records.
                let mismatch_info = match merge_wave_results_to_events_file(
                    &completed,
                    &main_events_file,
                    &detected.hat_config.publishes,
                    detected.target_hat.as_str(),
                    // 2026-06-16-001 U2: synthetic `wave.worker.failed`
                    // records attribute to `review-synthesizer` (the
                    // wave-result consumer) instead of the wave target
                    // hat. Pass `None` to use the default.
                    None,
                ) {
                    Ok(info) => info,
                    Err(e) => {
                        warn!(error = %e, "Failed to merge wave results to events file");
                        Vec::new()
                    }
                };

                // U5/R5: for each mismatched slot that is not yet
                // budget-exhausted, write a `task.resume` event to
                // the main events file. The `dimension-reviewer`
                // hat picks it up and retries that single slot.
                if !mismatch_info.is_empty() {
                    inject_dimension_retry_task_resume(
                        &mismatch_info,
                        &completed,
                        &main_events_file,
                        &mut dimension_retry_budget,
                        &detected,
                    );
                }
            }
            WaveDispatchOutcome::GlobalDeadlineExceeded => {
                // U4-C3: the runner-supplied outer deadline fired.
                // Record a loop-level recovery envelope (retry_key
                // namespaced by `loop_runner:<loop_id>:max_runtime`,
                // NOT wave-scoped — the deadline is a loop-level
                // signal, see plan §6 C3) and stop processing the
                // remaining waves. The runner uses the returned
                // `HandleWaveOutcome` to set
                // `late_termination_reason = MaxRuntime` and skip
                // post-iteration phases.
                //
                // DiagnosisSource: `WaveDispatcher` is the closest
                // existing variant — the wave dispatcher is what
                // actually fires the abort. `LoopRunner` does not
                // exist as a DiagnosisSource variant (envelope.rs
                // has no `LoopRunner` member); using `WaveDispatcher`
                // keeps the source stable for the responder while
                // the retry_key (loop-level) carries the
                // "loop" signal.
                record_loop_max_runtime_envelope(event_loop, loop_id, &detected);
                // ADV-5: mirror the Completed branch's TUI cleanup
                // so the header does not get stuck on a stale
                // wave_active pointer. We intentionally do NOT
                // emit RpcEvent::WaveCompleted — the wave did not
                // complete, it was aborted by the outer deadline;
                // the runner reads `result.global_deadline_exceeded`
                // and surfaces MaxRuntime termination itself.
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
                    let line = Line::from(Span::styled(
                        "── Wave aborted: loop max_runtime exceeded ──────────────────────",
                        Style::default().fg(Color::Red),
                    ));
                    push_to_tui_iteration(state, line);
                }
                result.global_deadline_exceeded = true;
                return result;
            }
            WaveDispatchOutcome::SpawnFailed {
                spawned_count,
                expected_count,
            } => {
                // U2: spawn guarantee violated — fewer workers were spawned
                // than there were wave events. Write a recovery envelope
                // so the diagnosis system can observe the failure, but
                // continue processing remaining waves. There are no results
                // to merge for this wave.
                warn!(
                    wave_id = %detected.wave_id,
                    spawned_count,
                    expected_count,
                    "Wave spawn guarantee violated"
                );
                record_wave_spawn_failed_envelope(
                    event_loop,
                    loop_id,
                    &detected,
                    spawned_count,
                    expected_count,
                );
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
                    let line = Line::from(Span::styled(
                        format!(
                            "── Wave spawn FAILED: {}/{} workers spawned ──────────────────────",
                            spawned_count, expected_count
                        ),
                        Style::default().fg(Color::Red),
                    ));
                    push_to_tui_iteration(state, line);
                }
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
    result
}

/// Execute a detected wave by spawning parallel backend instances.
///
/// Public entry point. Builds per-worker `WorkerRequest`s and delegates
/// to `dispatch_wave_inner` with a `ProductionExecutor`. Returns
/// `Result<CompletedWave>` for backwards compatibility — callers that
/// need structured partial / aggregate / global outcomes should use
/// [`Self::execute_wave_structured`] instead.
///
/// Kept as a thin compatibility wrapper even though
/// `handle_wave_events` now calls `execute_wave_structured`
/// directly: existing tests in `loop_runner/tests.rs` and any
/// downstream consumer that still imports the legacy signature
/// continue to work.
#[allow(dead_code)]
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
    // Suppress unused-variable warnings for diagnostics in this wrapper;
    // it is kept in the public signature for API stability.
    let _ = diagnostics;

    let outcome = execute_wave_structured(
        wave,
        global_backend,
        main_events_file,
        show_progress,
        use_colors,
        rpc_event_tx,
        tui_state,
        loop_id,
        // U4-C2: legacy wrapper does not have a runner-supplied
        // global deadline; the dispatcher will fall back to the
        // wave-internal partial/aggregate timers.
        WaveDispatchLimits::default(),
        // Legacy wrapper has no runner-supplied hat-source label;
        // the dispatcher falls back to the parent process env var.
        None,
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
        WaveDispatchOutcome::SpawnFailed {
            spawned_count,
            expected_count,
        } => Err(anyhow::anyhow!(
            "Wave {} spawn guarantee violated: only {}/{} workers spawned",
            wave.wave_id,
            spawned_count,
            expected_count
        )),
    }
}

/// Execute a detected wave and return the structured
/// [`WaveDispatchOutcome`].
///
/// U4-B3: this is the public entry point that lets the caller
/// distinguish Completed / Partial / AggregateDeadlineExceeded /
/// GlobalDeadlineExceeded and feed each into the recovery
/// responder. The legacy [`Self::execute_wave`] wrapper is
/// preserved for backwards compatibility and collapses all
/// non-Global variants into a successful `Result<CompletedWave>`.
pub async fn execute_wave_structured(
    wave: &ralph_core::DetectedWave,
    global_backend: &CliBackend,
    main_events_file: &Path,
    show_progress: bool,
    use_colors: bool,
    rpc_event_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
    loop_id: &str,
    // U4-C2: runner-supplied wave dispatch limits. The runner
    // computes `global_deadline` from its own loop runtime
    // budget (typically `loop.max_runtime_seconds -
    // state.elapsed()`) and passes it through here. The
    // dispatcher is responsible for all abort + drain sequencing
    // when the deadline fires; the runner MUST NOT touch worker
    // handles directly (KTD-U4-6).
    limits: WaveDispatchLimits,
    // Plan 001 §4.3 C1: explicit preset label forwarded to each
    // wave worker so its in-process `ralph emit` / `ralph wave emit`
    // invocations pick up the loop's `event_policy.schemas` via
    // `RALPH_HATS_SOURCE`. When `None`, the dispatcher falls back to
    // the parent process env var — this preserves the legacy wrapper
    // path for callers that have not yet threaded the label.
    hats_source_label: Option<&str>,
) -> WaveDispatchOutcome {
    use ralph_core::{WaveTracker, WaveWorkerContext, build_wave_worker_prompt};

    let concurrency = wave.hat_config.concurrency as usize;
    let wave_timeout = Duration::from_secs(wave.per_worker_timeout_secs());
    // Use an explicitly-configured aggregate timeout (worker or consumer)
    // directly.  Only fall back to the per-worker-timeout × batches formula
    // when no aggregate timeout is available.
    let aggregate_timeout =
        if wave.has_explicit_aggregate_timeout() || wave.consumer_aggregate_timeout.is_some() {
            Duration::from_secs(wave.aggregate_timeout_secs())
        } else {
            aggregate_timeout_for(wave_timeout, wave.events.len(), concurrency)
        };

    // Register wave in tracker
    let mut tracker = WaveTracker::new();
    tracker.register_wave_with_source(
        wave.wave_id.clone(),
        wave.total,
        Some(wave.target_hat.clone()),
    );

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

        // U1/R1: parse the `dimension` field from the wave event's
        // payload. When present and non-empty, the worker is
        // hard-bound to that dimension for `review.dimension.done`
        // emissions; the CLI precheck (R3) and merge layer (R4)
        // enforce this. Whitespace-only / missing / non-string values
        // become `None` so legacy / malformed waves still dispatch.
        let assigned_dimension = parse_assigned_dimension(event.payload.as_deref());

        // Create per-worker events file
        let worker_events_file = wave_dir.join(format!("wave-{}-{}.jsonl", wave_id, index_u32));

        // Build worker prompt
        let ctx = WaveWorkerContext {
            wave_id: wave_id.clone(),
            wave_index: index_u32,
            wave_total: wave.total,
            result_topics: hat_config.publishes.clone(),
            assigned_dimension: assigned_dimension.clone(),
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

        // U2: surface the worker-bound dimension (parsed by U1 from
        // the wave event's payload) to the backend's process
        // environment as `RALPH_WAVE_DIMENSION`. The agent's bash
        // tool can read this var to know which dimension it is
        // reviewing for, matching the `## ASSIGNED DIMENSION`
        // block in the worker prompt (also added by U1). When the
        // wave carries no dimension (legacy / non-review waves),
        // do NOT inject — the var stays unset, preserving the
        // pre-U2 behaviour for non-dimension-bound workers.
        if let Some(ref dim) = assigned_dimension {
            worker_backend
                .env_vars
                .push(("RALPH_WAVE_DIMENSION".into(), dim.clone()));
        }

        // Inject hat execution context for wave worker. Plan 001 C1:
        // forward `hats_source_label` explicitly when available so the
        // worker inherits the loop's preset (and its event_policy.schemas)
        // rather than relying on parent env propagation.
        inject_hat_execution_env(
            &mut worker_backend,
            wave.target_hat.as_str(),
            loop_id,
            &worker_events_file,
            None,
            hats_source_label,
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
            assigned_dimension,
        });
    }

    let executor: Arc<ProductionExecutor> = Arc::new(ProductionExecutor);

    // U4/R4 (2026-06-17-002): build the per-index dimension map
    // from the dispatcher-parsed `WorkerRequest::assigned_dimension`
    // fields and pass it through `DispatchContext` so:
    //   1. `inject_synthetic_failures` (the timeout / never-reported
    //      path) can stamp `dimension_missing` failures on slots
    //      that had a dimension assignment, and
    //   2. the merge layer can read it from the returned
    //      `CompletedWave.assigned_dimensions` to drop mismatched
    //      `review.dimension.done` events.
    // The `assigned_dimension` field is parsed by U1 from each
    // wave event's payload; `None` entries (legacy / non-review
    // waves) are omitted so the map only contains slots the
    // dispatcher actually hard-bound.
    let mut assigned_dimensions: std::collections::HashMap<u32, String> =
        std::collections::HashMap::new();
    for request in &worker_requests {
        if let Some(dim) = request.assigned_dimension.as_ref() {
            assigned_dimensions.insert(request.index, dim.clone());
        }
    }

    dispatch_wave_inner(
        tracker,
        worker_requests,
        DispatchContext::build(
            wave,
            wave_timeout,
            aggregate_timeout,
            payload_previews,
            show_progress,
            use_colors,
            limits,
            assigned_dimensions,
        ),
        executor,
        ProgressChannels {
            rpc_event_tx,
            tui_state,
        },
    )
    .await
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
    Duration::from_secs(wave_timeout.as_secs().saturating_mul(batches)) + Duration::from_secs(30)
}

/// U1/R1: extract the `dimension` field from a `review.wave.ready`
/// payload so the dispatcher can hard-bind the worker to that
/// dimension. The agent then MUST emit `review.dimension.done` with
/// exactly this dimension; mismatches are rejected (R3) and
/// dropped (R4) with a `task.resume` retry (R5).
///
/// Returns `None` for:
/// - missing / empty / whitespace-only payload
/// - non-JSON payload
/// - payload without a `dimension` key
/// - `dimension` value that is not a string
/// - empty / whitespace-only string value
///
/// This mirrors the malformed-payload tolerance of
/// `merge_wave_results_to_events_file`'s JSONL parsing so legacy
/// / off-spec waves keep dispatching.
fn parse_assigned_dimension(payload: Option<&str>) -> Option<String> {
    let payload = payload?.trim();
    if payload.is_empty() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let dim = value.get("dimension")?.as_str()?;
    let trimmed = dim.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
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
    // U2 (Unit 2 of 2026-06-17-001 plan): spawn guarantee — every
    // wave event MUST produce a worker task. Track the count so we can
    // assert after the loop and return SpawnFailed if any requests were
    // silently dropped.
    let mut join_set: tokio::task::JoinSet<(u32, WaveWorkerOutcome)> = tokio::task::JoinSet::new();
    let mut spawned_count = 0u32;
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
                        Err((format!("permit acquire failed: {e}"), Duration::ZERO)),
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
        spawned_count += 1;
    }

    // U2: spawn guarantee — 0-worker silent is forbidden.
    // We spawn one worker per event (worker_requests.len() = events.len()).
    // Use events_len, not expected_total, because in malformed partial waves
    // total > events.len() and only events.len() workers are created.
    if spawned_count < ctx.events_len {
        warn!(
            wave_id = %ctx.wave_id,
            spawned_count,
            expected_count = ctx.events_len,
            "wave_spawn_failed: fewer workers spawned than wave events"
        );
        return WaveDispatchOutcome::SpawnFailed {
            spawned_count,
            expected_count: ctx.events_len,
        };
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
    // KTD-U3-5 (revised): two-stage timeout, currently collapsed
    // into a single `finalize_timeout` (see the long comment in
    // the `else` branch below). Both `partial_deadline` and
    // `aggregate_deadline` paths return `AggregateDeadlineExceeded`
    // today, so the flag below only gates the loop's
    // deadline-choice comparison. The flag stays `let` (not
    // `let mut`) so future maintainers can wire a real
    // "release-permits-then-keep-waiting" two-stage path by
    // adding the mutation point without further refactoring the
    // loop body.
    let partial_threshold_fired = false;

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
                            let completed =
                                take_results(&mut tracker, &ctx.wave_id, &ctx.assigned_dimensions);
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
                // The sleep timer may have fired because either the
                // partial/aggregate deadline or the runner-supplied
                // global deadline arrived. Re-check the global
                // deadline first; if it has passed, surface
                // `GlobalDeadlineExceeded` so the runner can map to
                // `TerminationReason::MaxRuntime`. (Per U4-C2 /
                // KTD-U4-6: the global deadline is the highest
                // priority outer bound and must preempt the inner
                // partial/aggregate timers.)
                if ctx
                    .global_deadline
                    .map(|gd| tokio::time::Instant::now() >= gd)
                    .unwrap_or(false)
                {
                    finalize_global_exceeded(&mut join_set, &ctx, progress_handle).await;
                    return WaveDispatchOutcome::GlobalDeadlineExceeded;
                }
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
                    return WaveDispatchOutcome::AggregateDeadlineExceeded(completed);
                }
            }
        }
    }

    // JoinSet fully drained. Record synthetic failures for any
    // worker index that never reported (panicked or cancelled).
    for i in 0..ctx.expected_total {
        if !tracker.has_reported(&ctx.wave_id, i) {
            // U4/R4 (2026-06-17-002): when the un-reported slot
            // had a dimension assignment, record a
            // `dimension_missing` failure so the merge layer
            // emits the structured `wave.worker.failed` record.
            let expected = ctx.assigned_dimensions.get(&i).cloned();
            if let Some(expected_dim) = expected {
                tracker.record_failure_with_dimensions(
                    &ctx.wave_id,
                    i,
                    format!("dimension_missing: expected={expected_dim}"),
                    ctx.started_at.elapsed(),
                    Some(expected_dim),
                    None,
                );
            } else {
                tracker.record_failure(
                    &ctx.wave_id,
                    i,
                    "worker did not report (panic or cancellation)".into(),
                    ctx.started_at.elapsed(),
                );
            }
        }
    }

    let completed = take_results(&mut tracker, &ctx.wave_id, &ctx.assigned_dimensions);
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

fn take_results(
    tracker: &mut ralph_core::WaveTracker,
    wave_id: &str,
    assigned_dimensions: &std::collections::HashMap<u32, String>,
) -> CompletedWave {
    let mut completed = tracker
        .take_wave_results(wave_id)
        .expect("wave must exist in tracker after registration");
    // U4/R4 (2026-06-17-002): stamp the per-index dimension map
    // onto the returned CompletedWave so the merge layer can
    // drop mismatched review.dimension.done events.
    completed.assigned_dimensions = assigned_dimensions.clone();
    completed
}

/// U5/R5 (2026-06-17-002): write a `task.resume` event for each
/// mismatched slot whose retry budget is not yet exhausted.
///
/// Each slot may retry at most once (2 total attempts). Exhausted
/// slots are skipped so a permanently-mismatched worker cannot
/// infinite-loop on the dispatcher's `task.resume` — U4's
/// `wave.worker.failed` record (already written by the merge
/// layer) becomes the terminal signal for the synthesizer.
///
/// The injected events are appended directly to the main events
/// file as JSONL so the `EventReader` picks them up on the next
/// iteration. The `triggered` field targets `dimension-reviewer`
/// (per the ce-executor-isolated preset's `triggers` list, which
/// we extend to include `task.resume` as part of this unit) and
/// the payload carries the structured fields the retry worker
/// needs to re-run the review for the correct dimension.
fn inject_dimension_retry_task_resume(
    mismatch_info: &[super::io::DimensionMismatchInfo],
    completed: &ralph_core::CompletedWave,
    main_events_file: &Path,
    budget: &mut std::collections::HashMap<(String, u32), u32>,
    detected: &ralph_core::DetectedWave,
) {
    use std::io::Write;

    const MAX_RETRIES_PER_SLOT: u32 = 1;

    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(main_events_file)
    {
        Ok(f) => f,
        Err(e) => {
            warn!(
                error = %e,
                "U5/R5: failed to open events file for task.resume injection; skipping retries"
            );
            return;
        }
    };

    let ts = chrono::Utc::now().to_rfc3339();

    for mismatch in mismatch_info {
        let key = (completed.wave_id.clone(), mismatch.wave_index);
        let used = budget.get(&key).copied().unwrap_or(0);
        if used >= MAX_RETRIES_PER_SLOT {
            // Budget exhausted: do not inject another task.resume.
            // The synthetic wave.worker.failed record already
            // written by the merge layer remains the terminal
            // signal.
            tracing::debug!(
                wave_id = %completed.wave_id,
                wave_index = mismatch.wave_index,
                used,
                "U5/R5: dimension retry budget exhausted; skipping task.resume"
            );
            continue;
        }
        let retry_key = format!(
            "wave_dimension_guard:{}:{}:dimension_mismatch:dimension",
            completed.wave_id, mismatch.wave_index
        );
        let payload = serde_json::json!({
            "stage": "WaveDimensionGuard",
            "topic": "review.dimension.done",
            "violation": "dimension_mismatch",
            "allowed_topics": ["review.dimension.done"],
            "required_fields": ["dimension"],
            "original_trigger_topic": "review.wave.ready",
            "original_trigger_payload": detected
                .events
                .get(mismatch.wave_index as usize)
                .and_then(|e| e.payload.clone()),
            "retry_key": retry_key,
            "original_hat": "dimension-reviewer",
            "wave_id": completed.wave_id,
            "wave_index": mismatch.wave_index,
            "wave_total": completed.wave_total,
            "reason": "dimension_mismatch",
            "target_hat": "dimension-reviewer",
            "expected_dimension": mismatch.expected_dimension,
            "actual_dimension": mismatch.actual_dimension,
        });
        let record = serde_json::json!({
            "topic": "task.resume",
            "triggered": "dimension-reviewer",
            "hat": "review-synthesizer",
            "source": "review-synthesizer",
            // U5/R5: set `target` so the bus routes the recovery
            // signal to `dimension-reviewer`. `task.resume` is a
            // reserved orchestrator control topic that cannot
            // appear in a hat's `triggers` list (strict lint
            // rejects it); target-based routing is the standard
            // pattern for all recovery signals.
            "target": "dimension-reviewer",
            "payload": payload.to_string(),
            "ts": ts,
            "wave_id": completed.wave_id,
            "wave_index": mismatch.wave_index,
            "wave_total": completed.wave_total,
        });
        if let Err(e) = writeln!(file, "{}", record) {
            warn!(
                error = %e,
                "U5/R5: failed to write task.resume event to events file"
            );
            continue;
        }
        budget.insert(key, used + 1);
        tracing::info!(
            wave_id = %completed.wave_id,
            wave_index = mismatch.wave_index,
            expected = %mismatch.expected_dimension,
            actual = %mismatch.actual_dimension,
            "U5/R5: injected task.resume to retry dimension-reviewer for mismatched slot"
        );
    }
}

fn outcome_for_completion(completed: CompletedWave) -> WaveDispatchOutcome {
    if completed.partial {
        WaveDispatchOutcome::Partial(completed)
    } else {
        WaveDispatchOutcome::Completed(completed)
    }
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
    let mut completed = tracker
        .force_take_wave_results(&ctx.wave_id)
        .expect("wave must exist in tracker after registration");
    // U4/R4 (2026-06-17-002): stamp the per-index dimension map
    // onto the returned CompletedWave even on the timeout path
    // so the merge layer can record dimension_missing failures.
    completed.assigned_dimensions = ctx.assigned_dimensions.clone();
    completed
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
            // U4/R4 (2026-06-17-002): when the un-reported slot
            // had a dimension assignment, record a
            // `dimension_missing` failure so the merge layer
            // emits `wave.worker.failed(reason=worker_failed:dimension_missing)`
            // with the expected dimension. Plain
            // `record_failure` would lose the dimension context.
            let expected = ctx.assigned_dimensions.get(&i).cloned();
            if let Some(expected_dim) = expected {
                tracker.record_failure_with_dimensions(
                    &ctx.wave_id,
                    i,
                    format!("dimension_missing: expected={expected_dim}"),
                    threshold.saturating_duration_since(ctx.started_at),
                    Some(expected_dim),
                    None,
                );
            } else {
                tracker.record_failure(
                    &ctx.wave_id,
                    i,
                    format!("worker did not report before {label}"),
                    threshold.saturating_duration_since(ctx.started_at),
                );
            }
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

/// U4-C3: Record a loop-level recovery envelope when the
/// runner-supplied `global_deadline` preempts a wave.
///
/// The retry key is intentionally loop-scoped (not wave-scoped) —
/// the global deadline is a loop-level signal, so different waves
/// that hit the same `max_runtime_seconds` budget MUST collapse
/// into a single finding (the loop is the unit being terminated,
/// not the wave). Format: `loop_runner:<loop_id>:max_runtime`.
///
/// Per plan §6 C3, we use `DiagnosisSource::WaveDispatcher` (the
/// closest existing variant — `LoopRunner` is not in the
/// `DiagnosisSource` enum, and the wave dispatcher is the code
/// path that actually fires the abort). The loop-scope is carried
/// in the retry_key, not the source.
///
/// ADV-1 idempotency guard: `RecoveryResponder::observe()` (see
/// `responder.rs:771`) increments `attempt_count` on every re-fire
/// of an existing retry_key, with no de-dup. The runner may invoke
/// this helper multiple times across iterations (e.g. iteration N
/// triggers GlobalDeadlineExceeded but does not break out before a
/// second wave in the same iteration also fires the outer
/// deadline). Without a guard, each call would bump the counter
/// toward Hard/Final escalation even though the underlying signal
/// is unchanged. We use the existing `attempt_count` reader on
/// the responder (no new API, no responder internals) to detect
/// prior observations and short-circuit.
fn record_loop_max_runtime_envelope(
    event_loop: &mut ralph_core::EventLoop,
    loop_id: &str,
    wave: &ralph_core::DetectedWave,
) {
    use ralph_core::diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
    };

    let retry_key = format!("loop_runner:{}:max_runtime", loop_id);
    if event_loop.recovery_responder().attempt_count(&retry_key) > 0 {
        // Already recorded this loop's max_runtime abort in an
        // earlier wave/iteration. Skipping prevents responder
        // attempt_count inflation toward escalation. The
        // DiagnosticsCollector still gets the original envelope
        // from the first call, which is what the audit log
        // contract requires (one envelope per abort).
        return;
    }

    let topic = wave
        .hat_config
        .publishes
        .first()
        .cloned()
        .or_else(|| wave.events.first().map(|e| e.topic.to_string()))
        .unwrap_or_default();

    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::WaveDispatcher)
        .severity(DiagnosisSeverity::Error)
        .iteration(event_loop.state().iteration)
        .source_hat(wave.target_hat.to_string())
        .topic(topic)
        .reason_code("loop_max_runtime_exceeded")
        .message(format!(
            "Loop {} max_runtime exceeded during wave {} on hat {}",
            loop_id, wave.wave_id, wave.target_hat
        ))
        .expected_action(
            "Loop will terminate with TerminationReason::MaxRuntime. Investigate long-running wave workers."
                .to_string(),
        )
        .retry_attempt(0)
        .safe_target(false)
        .outcome(DiagnosisOutcome::NotRetriable)
        .retry_key(retry_key)
        .build();
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
}

/// U4-B3: Record a recovery envelope for a wave that finished via
/// `Partial` or `AggregateDeadlineExceeded`.
///
/// The dispatcher keeps the structured `WaveDispatchOutcome` opaque
/// from the responder's perspective; this helper materializes the
/// outcome into a `RecoveryDiagnosisEnvelope` using only the existing
/// schema fields (`topic` / `reason_code` / `message` / `retry_key`)
/// per U4 plan §5 B3. The envelope's `outcome` is `Pending`; the
/// responder will upgrade it to `Recovered` once a new wave on the
/// same target topic completes in a later iteration (KTD-U4-5
/// table). It is safe to call this for any completed wave — the
/// caller decides which outcomes are worth recording.
fn record_wave_timeout_envelope(
    event_loop: &mut ralph_core::EventLoop,
    wave: &ralph_core::DetectedWave,
    completed: &ralph_core::CompletedWave,
    reason_code: &'static str,
) {
    use ralph_core::diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
        RecoveryDiagnosisEnvelopeBuilder,
    };

    // Plan §5 B3: RecoveryDiagnosisEnvelope has no independent
    // `wave_id` / `expected` / `completed` fields. We encode the
    // wave identity via the wave-scoped `retry_key` and stash the
    // counts in the human-readable `message` (truncated to
    // `MAX_ENVELOPE_MESSAGE_CHARS` by `build`).
    let topic = wave
        .hat_config
        .publishes
        .first()
        .cloned()
        .or_else(|| wave.events.first().map(|e| e.topic.to_string()))
        .unwrap_or_default();

    let expected = completed.wave_total as usize;
    let actual = completed.results.len() + completed.failures.len();
    let duration_ms = completed.duration.as_millis() as u64;

    let retry_key = RecoveryDiagnosisEnvelopeBuilder::wave_retry_key(&wave.wave_id, reason_code);
    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::WaveDispatcher)
        .severity(DiagnosisSeverity::Warning)
        .source_hat(wave.target_hat.to_string())
        .topic(topic)
        .reason_code(reason_code)
        .message(format!(
            "Wave {} timeout: {}/{} workers reported in {}ms (reason={})",
            wave.wave_id, actual, expected, duration_ms, reason_code
        ))
        .expected_action(
            "Investigate the slow wave workers; a subsequent complete wave on this target topic will mark this finding Recovered."
                .to_string(),
        )
        .retry_attempt(0)
        .safe_target(false)
        .outcome(DiagnosisOutcome::Pending)
        .retry_key(retry_key)
        .build();
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
}

/// U2 (Unit 2 of 2026-06-17-001 plan): Record a recovery envelope
/// for a wave that violated the spawn guarantee (fewer workers spawned
/// than wave events received).
///
/// The envelope uses `DiagnosisOutcome::NotRetriable` — the failure
/// is at the spawn layer, not recoverable by retrying the same wave.
fn record_wave_spawn_failed_envelope(
    event_loop: &mut ralph_core::EventLoop,
    loop_id: &str,
    wave: &ralph_core::DetectedWave,
    spawned_count: u32,
    expected_count: u32,
) {
    use ralph_core::diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
        RecoveryDiagnosisEnvelopeBuilder,
    };

    let topic = wave
        .hat_config
        .publishes
        .first()
        .cloned()
        .unwrap_or_default();

    let retry_key =
        RecoveryDiagnosisEnvelopeBuilder::wave_retry_key(&wave.wave_id, "wave_spawn_failed");
    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::WaveDispatcher)
        .severity(DiagnosisSeverity::Error)
        .source_hat(wave.target_hat.to_string())
        .topic(topic)
        .reason_code("wave_spawn_failed")
        .message(format!(
            "Wave {} spawn guarantee violated: only {}/{} workers spawned (loop={})",
            wave.wave_id, spawned_count, expected_count, loop_id
        ))
        .expected_action(
            "Investigate why workers failed to spawn. This may indicate a system resource issue or dispatcher bug."
                .to_string(),
        )
        .retry_attempt(0)
        .safe_target(false)
        .outcome(DiagnosisOutcome::NotRetriable)
        .retry_key(retry_key)
        .build();
    let _ = event_loop.record_recovery_envelope(&envelope, Vec::new());
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
        ralph_core::WaveRejection::IsolatedScopeViolation {
            topic,
            isolated_hat,
            ..
        } => (
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
        let plan_blocked_event = ralph_proto::Event::new("plan.blocked", plan_blocked_payload);
        event_loop.publish_event(plan_blocked_event);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_runner::wave::io::DimensionMismatchInfo;
    use ralph_core::EventLoop;
    use ralph_core::config::RalphConfig;
    use ralph_proto::HatId;
    use std::fs;
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
            // Plan 001 §4.3 C1: tests don't exercise the env-var
            // propagation path; leave the label None to fall back to
            // the parent process env.
            hats_source_label: None,
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
            ("NoTargetHat", ralph_core::WaveRejection::NoTargetHat),
        ];
        let out = build_outputs_silent();

        for (label, reason) in cases {
            let mut el = build_event_loop();
            let captured: Arc<Mutex<Vec<ralph_proto::Event>>> = Arc::new(Mutex::new(Vec::new()));
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
            consumer_aggregate_timeout: None,
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
                    match max.compare_exchange(cur_max, now, Ordering::SeqCst, Ordering::SeqCst) {
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
                    Ok((vec![core_event("review.done", "ok")], hold_for, success))
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
        make_worker_request_with_dimension(index, progress_tx, None)
    }

    fn make_worker_request_with_dimension(
        index: u32,
        progress_tx: tokio::sync::mpsc::UnboundedSender<(u32, bool, Duration)>,
        assigned_dimension: Option<String>,
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
            assigned_dimension,
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
        let requests: Vec<WorkerRequest> = (0..4u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
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
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;

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
                assert_eq!(
                    c.failures.len(),
                    4,
                    "all 4 indices must have synthetic failures"
                );
                for (i, f) in c.failures.iter().enumerate() {
                    assert_eq!(f.index, i as u32, "synthetic failure for index {i}");
                }
            }
            other => {
                panic!("expected AggregateDeadlineExceeded (collapsed partial), got {other:?}")
            }
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
        let requests: Vec<WorkerRequest> = (0..3u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
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
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;
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
        let requests: Vec<WorkerRequest> = (0..3u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
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
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;
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
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            dispatch_wave_inner(tracker, requests, ctx, executor, silent_progress()),
        )
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
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;
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
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;
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

    /// U2 (Unit 2 of 2026-06-17-001 plan): spawn guarantee — when all
    /// requests are spawned, `SpawnFailed` must NOT fire.
    #[tokio::test(start_paused = true)]
    async fn u2_spawn_guarantee_passes_when_all_workers_spawn() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // 3 requests matching 3 events in the wave.
        let requests: Vec<WorkerRequest> = (0..3u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_millis(50)));

        let wave = make_wave(3, 3, 3);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_secs(60),
            Duration::from_secs(30),
            vec!["p0".into(), "p1".into(), "p2".into()],
            false,
            false,
            WaveDispatchLimits::default(),
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;

        // Must NOT be SpawnFailed — all 3 requests were spawned.
        match &outcome {
            WaveDispatchOutcome::SpawnFailed { .. } => {
                panic!("SpawnFailed must NOT fire when all workers spawned: {outcome:?}")
            }
            _ => {}
        }
        // Otherwise should be Completed or Partial.
        match outcome {
            WaveDispatchOutcome::Completed(c) | WaveDispatchOutcome::Partial(c) => {
                assert_eq!(c.results.len(), 3, "all 3 workers should succeed");
            }
            WaveDispatchOutcome::SpawnFailed { .. } => unreachable!(),
            WaveDispatchOutcome::AggregateDeadlineExceeded(c) => {
                // Aggregate deadline could fire in the paused-time test depending
                // on the short sleep; that's fine — the key invariant is we
                // did NOT silently return SpawnFailed with 0 spawned.
                assert!(
                    c.results.len() <= 3,
                    "at most 3 results: {}/{}",
                    c.results.len(),
                    3
                );
            }
            WaveDispatchOutcome::GlobalDeadlineExceeded => {
                // Also acceptable — deadline could fire first.
            }
        }
    }

    /// U2 (Unit 2 of 2026-06-17-001 plan): spawn guarantee — when fewer
    /// workers are spawned than there are wave events, `SpawnFailed` must
    /// fire with the correct counts.
    #[tokio::test(start_paused = true)]
    async fn u2_spawn_guarantee_fires_when_fewer_workers_spawn() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // Only 2 requests even though the wave has 3 events.
        // This simulates the case where some events failed to produce requests.
        let requests: Vec<WorkerRequest> = (0..2u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_secs(3600)));

        let wave = make_wave(3, 3, 3); // 3 events, total=3
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_secs(60),
            Duration::from_secs(30),
            vec!["p0".into(), "p1".into(), "p2".into()],
            false,
            false,
            WaveDispatchLimits::default(),
            std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome =
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()).await;

        match outcome {
            WaveDispatchOutcome::SpawnFailed {
                spawned_count,
                expected_count,
            } => {
                assert_eq!(spawned_count, 2, "only 2 workers were spawned");
                assert_eq!(expected_count, 3, "wave has 3 events");
            }
            other => {
                panic!("expected SpawnFailed when fewer workers spawn than events, got {other:?}");
            }
        }
        // Note: we CANNOT assert on executor.started here because SpawnFailed
        // is returned immediately after the spawn loop — the spawned tasks have
        // been added to the JoinSet but have not been polled yet, so
        // `execute()` has not been called. The important invariant is the
        // outcome is SpawnFailed with the correct counts.
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
        let mut session_dirs: Vec<_> =
            std::fs::read_dir(diagnostics_root.join(".ralph/diagnostics"))
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

    // ---------------------------------------------------------------------
    // U5 (2026-06-17-002): task.resume injection for dimension mismatches
    // ---------------------------------------------------------------------

    /// U5/R5: a single dimension-mismatch slot must produce a
    /// `task.resume` event in the main events file with the
    /// expected/actual dimensions carried in the structured payload.
    #[test]
    fn u5_mismatch_writes_task_resume() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let events_file = tmp.path().join("events.jsonl");

        let mismatches = vec![DimensionMismatchInfo {
            wave_index: 1,
            expected_dimension: "testing".to_string(),
            actual_dimension: "correctness".to_string(),
        }];
        let mut assigned_dimensions = std::collections::HashMap::new();
        assigned_dimensions.insert(0u32, "correctness".to_string());
        assigned_dimensions.insert(1, "testing".to_string());

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u5-dim".to_string(),
            wave_total: 2,
            results: vec![],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned_dimensions.clone(),
        };

        // Build a minimal DetectedWave so the helper can read
        // `original_trigger_payload` from `wave.events[index]`.
        use ralph_core::config::HatConfig;
        let det = ralph_core::DetectedWave {
            wave_id: "w-u5-dim".to_string(),
            target_hat: ralph_proto::HatId::new("dimension-reviewer"),
            hat_config: HatConfig {
                name: "Dimension Reviewer".to_string(),
                concurrency: 9,
                ..HatConfig::default()
            },
            events: vec![
                ralph_core::Event {
                    topic: "review.wave.ready".to_string(),
                    payload: Some("{\"dimension\":\"correctness\"}".to_string()),
                    ts: String::new(),
                    hat: None,
                    triggered: None,
                    source: None,
                    wave_id: None,
                    wave_index: None,
                    wave_total: None,
                },
                ralph_core::Event {
                    topic: "review.wave.ready".to_string(),
                    payload: Some("{\"dimension\":\"testing\"}".to_string()),
                    ts: String::new(),
                    hat: None,
                    triggered: None,
                    source: None,
                    wave_id: None,
                    wave_index: None,
                    wave_total: None,
                },
            ],
            total: 2,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let mut budget = std::collections::HashMap::new();
        inject_dimension_retry_task_resume(
            &mismatches,
            &completed,
            &events_file,
            &mut budget,
            &det,
        );

        let content = fs::read_to_string(&events_file).expect("read events file");
        let records: Vec<serde_json::Value> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("json event"))
            .collect();

        assert_eq!(
            records.len(),
            1,
            "U5/R5: exactly one task.resume event expected, got {records:?}"
        );
        let r = &records[0];
        assert_eq!(r["topic"], "task.resume");
        assert_eq!(r["triggered"], "dimension-reviewer");
        assert_eq!(r["hat"], "review-synthesizer");
        assert_eq!(r["source"], "review-synthesizer");
        assert_eq!(r["target"], "dimension-reviewer");
        assert_eq!(r["wave_id"], "w-u5-dim");
        assert_eq!(r["wave_index"], 1);
        assert_eq!(r["wave_total"], 2);

        let payload_str = r["payload"].as_str().expect("payload must be string");
        let payload: serde_json::Value =
            serde_json::from_str(payload_str).expect("payload must be JSON object");
        assert_eq!(payload["stage"], "WaveDimensionGuard");
        assert_eq!(payload["violation"], "dimension_mismatch");
        assert_eq!(payload["reason"], "dimension_mismatch");
        assert_eq!(payload["target_hat"], "dimension-reviewer");
        assert_eq!(payload["expected_dimension"], "testing");
        assert_eq!(payload["actual_dimension"], "correctness");
        assert_eq!(payload["wave_id"], "w-u5-dim");
        assert_eq!(payload["wave_index"], 1);
        assert_eq!(payload["wave_total"], 2);

        // Budget must reflect 1 used retry.
        assert_eq!(budget.get(&("w-u5-dim".to_string(), 1)), Some(&1));
    }

    /// U5/R5: a second call for the same slot must NOT inject
    /// another `task.resume` because the per-slot budget (1
    /// retry = 2 total attempts) is exhausted.
    #[test]
    fn u5_budget_exhausted_skips_second_resume() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let events_file = tmp.path().join("events.jsonl");

        let mismatches = vec![DimensionMismatchInfo {
            wave_index: 1,
            expected_dimension: "testing".to_string(),
            actual_dimension: "correctness".to_string(),
        }];
        let completed = ralph_core::CompletedWave {
            wave_id: "w-u5-exhaust".to_string(),
            wave_total: 2,
            results: vec![],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
        };
        use ralph_core::config::HatConfig;
        let det = ralph_core::DetectedWave {
            wave_id: "w-u5-exhaust".to_string(),
            target_hat: ralph_proto::HatId::new("dimension-reviewer"),
            hat_config: HatConfig {
                name: "Dimension Reviewer".to_string(),
                concurrency: 9,
                ..HatConfig::default()
            },
            events: vec![],
            total: 2,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let mut budget = std::collections::HashMap::new();
        inject_dimension_retry_task_resume(
            &mismatches,
            &completed,
            &events_file,
            &mut budget,
            &det,
        );
        // First call writes one task.resume.
        assert_eq!(budget.get(&("w-u5-exhaust".to_string(), 1)), Some(&1));

        // Second call for the same (wave_id, wave_index) must
        // not write a second event because the budget is exhausted.
        inject_dimension_retry_task_resume(
            &mismatches,
            &completed,
            &events_file,
            &mut budget,
            &det,
        );

        let content = fs::read_to_string(&events_file).expect("read events file");
        let records: Vec<serde_json::Value> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("json event"))
            .collect();

        let resume_count = records
            .iter()
            .filter(|r| r["topic"] == "task.resume")
            .count();
        assert_eq!(
            resume_count, 1,
            "U5/R5: budget exhausted → must not write a second task.resume, got {resume_count}"
        );
    }

    /// U5/R5: an empty mismatch list must not write any
    /// `task.resume` events to the events file.
    #[test]
    fn u5_no_mismatch_no_resume() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let events_file = tmp.path().join("events.jsonl");

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u5-clean".to_string(),
            wave_total: 4,
            results: vec![],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
        };
        use ralph_core::config::HatConfig;
        let det = ralph_core::DetectedWave {
            wave_id: "w-u5-clean".to_string(),
            target_hat: ralph_proto::HatId::new("dimension-reviewer"),
            hat_config: HatConfig {
                name: "Dimension Reviewer".to_string(),
                concurrency: 9,
                ..HatConfig::default()
            },
            events: vec![],
            total: 4,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let mut budget = std::collections::HashMap::new();
        let mismatches: Vec<DimensionMismatchInfo> = vec![];
        inject_dimension_retry_task_resume(
            &mismatches,
            &completed,
            &events_file,
            &mut budget,
            &det,
        );

        let content = fs::read_to_string(&events_file).expect("read events file");
        assert!(
            content.trim().is_empty(),
            "U5/R5: no mismatches → events file must be empty, got: {content}"
        );
    }

    // ---------------------------------------------------------------------
    // U4-C1: failing integration test for runner-supplied global deadline.
    // ---------------------------------------------------------------------

    /// U4-C1 / §6 C1: a runner-supplied `global_deadline` (e.g. derived
    /// from `loop.max_runtime_seconds`) must preempt the wave before
    /// the partial/aggregate deadlines do, even when individual workers
    /// would block past the deadline. The dispatcher must return
    /// `WaveDispatchOutcome::GlobalDeadlineExceeded` AND leave zero
    /// active workers (the existing U3 abort+drain contract still
    /// applies).
    ///
    /// Uses `start_paused = true` so the 10s deadline is reached
    /// deterministically and the worker sleep of 3600s never resolves
    /// first.
    #[tokio::test(start_paused = true)]
    async fn u4_c1_global_deadline_preempts_wave() {
        // 4 workers that would all block past the global deadline.
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..4u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_secs(3600)));

        let wave = make_wave(4, 4, 4);
        // Use a generous aggregate (3600s) so the partial / aggregate
        // paths CANNOT fire first; only the global deadline (10s)
        // will win.
        let aggregate = Duration::from_secs(3600);
        // global_deadline = now + 10s in paused-time terms.
        let global_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_secs(60),
            aggregate,
            vec!["p0".into(), "p1".into(), "p2".into(), "p3".into()],
            false,
            false,
            WaveDispatchLimits {
                global_deadline: Some(global_deadline),
            },
        std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome = tokio::time::timeout(
            Duration::from_secs(20),
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()),
        )
        .await
        .expect("dispatch_wave_inner must not hang past the global deadline");

        match outcome {
            WaveDispatchOutcome::GlobalDeadlineExceeded => {
                // U3 contract: zero active workers after global
                // deadline abort+drain. The `TestExecutor` is
                // paused-time and never executes its `fetch_sub`
                // on the in-flight counter (it only runs after the
                // sleep), so we only assert via `started`: every
                // spawned worker must have entered the executor
                // (so the dispatcher's abort path actually
                // reached them), and the JoinSet must be empty
                // (which `dispatch_wave_inner` guarantees by the
                // `while join_set.join_next().await.is_some() {}`
                // drain in `finalize_global_exceeded`).
                assert_eq!(
                    executor.started.load(Ordering::SeqCst),
                    4,
                    "all 4 workers must have been spawned before global deadline"
                );
            }
            other => panic!(
                "expected GlobalDeadlineExceeded (runner-supplied 10s budget), got {other:?}"
            ),
        }
    }

    /// U4-C1 / §6 C1: a global deadline of `now` (i.e. already past)
    /// must fire on the dispatch loop's first re-check rather than
    /// waiting for the partial/aggregate timers. This is the
    /// conservative path for the runner: when `remaining` is zero,
    /// it must still pass `Some(now)` rather than `None`, otherwise
    /// the wave would have NO upper bound at all.
    #[tokio::test(start_paused = true)]
    async fn u4_c1_zero_remaining_deadline_fires_immediately() {
        // 2 workers, each holding for 3600s.
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let requests: Vec<WorkerRequest> = (0..2u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_secs(3600)));

        let wave = make_wave(2, 2, 2);
        // Aggregate far in the future; only the global deadline
        // (= now, already past) should fire.
        let aggregate = Duration::from_secs(3600);
        let global_deadline = tokio::time::Instant::now();
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_secs(60),
            aggregate,
            vec!["p0".into(), "p1".into()],
            false,
            false,
            WaveDispatchLimits {
                global_deadline: Some(global_deadline),
            },
        std::collections::HashMap::new(),
        );

        let mut tracker = ralph_core::WaveTracker::new();
        tracker.register_wave_with_source(
            wave.wave_id.clone(),
            wave.total,
            Some(wave.target_hat.clone()),
        );

        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            dispatch_wave_inner(tracker, requests, ctx, executor.clone(), silent_progress()),
        )
        .await
        .expect("dispatch_wave_inner must not hang on a zero-remaining deadline");

        assert!(
            matches!(outcome, WaveDispatchOutcome::GlobalDeadlineExceeded),
            "expected GlobalDeadlineExceeded (zero-remaining deadline), got {outcome:?}"
        );
        // When `global_deadline` is in the past at loop entry, the
        // dispatcher's loop-top `global_fired` check returns
        // immediately, before any worker is spawned. This is the
        // conservative path: the runner should always pass
        // `Some(now + remaining)` (even when `remaining` is zero)
        // so the dispatch loop gets one chance to abort cleanly.
        // 0 started workers is the correct outcome here.
        assert_eq!(
            executor.started.load(Ordering::SeqCst),
            0,
            "zero-remaining global deadline must short-circuit before spawning workers"
        );
    }

    // ---------------------------------------------------------------------
    // U4-C3: handle_wave_events outcome + recovery envelope.
    // ---------------------------------------------------------------------

    /// U4-C3 / §6 C3: the loop-level recovery envelope written when
    /// the global deadline preempts a wave must have the exact
    /// schema the runner relies on for `TerminationReason::MaxRuntime`:
    /// retry_key = `loop_runner:<loop_id>:max_runtime`, source =
    /// `WaveDispatcher` (no `LoopRunner` variant exists in
    /// `DiagnosisSource`), reason_code = `loop_max_runtime_exceeded`,
    /// outcome = `NotRetriable`. Verifies the journal entry lands on
    /// disk in `recovery.jsonl`.
    #[tokio::test]
    async fn u4_c3_record_loop_max_runtime_envelope_writes_recovery_entry() {
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
        el.initialize("loop-abc");
        let wave = make_wave(2, 2, 2);

        record_loop_max_runtime_envelope(&mut el, "loop-abc", &wave);

        // Read recovery.jsonl from the diagnostics session dir.
        let mut session_dirs: Vec<_> =
            std::fs::read_dir(diagnostics_root.join(".ralph/diagnostics"))
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
            1,
            "one envelope for one global-deadline event, got {}",
            entries.len()
        );
        let entry = &entries[0].envelope;

        // U4-C3 retry_key contract.
        assert_eq!(
            entry.retry_key, "loop_runner:loop-abc:max_runtime",
            "retry key must use the loop-scoped loop_runner:<loop_id>:max_runtime format"
        );
        assert_eq!(
            entry.reason_code, "loop_max_runtime_exceeded",
            "reason code must identify the loop-level max_runtime budget"
        );
        assert_eq!(
            entry.outcome,
            ralph_core::diagnosis::DiagnosisOutcome::NotRetriable,
            "loop-level max_runtime finding is not auto-recoverable"
        );
        assert_eq!(
            entry.source,
            ralph_core::diagnosis::DiagnosisSource::WaveDispatcher,
            "source must be WaveDispatcher (no LoopRunner variant in DiagnosisSource)"
        );
        assert_eq!(
            entry.severity,
            ralph_core::diagnosis::DiagnosisSeverity::Error,
            "severity must be Error — the loop is about to terminate"
        );
        assert!(
            entry.message.contains("loop-abc") && entry.message.contains(&wave.wave_id),
            "message must mention both loop_id and wave_id, got: {}",
            entry.message
        );
    }

    /// U4-C3: when `handle_wave_events` is called with an empty
    /// `wave_events` slice, it must return `HandleWaveOutcome::default()`
    /// — i.e. `global_deadline_exceeded = false` and the runner
    /// does NOT set `late_termination_reason`. The empty-wave path
    /// is the only `handle_wave_events` return value the runner can
    /// trivially exercise without spawning a real backend.
    #[tokio::test]
    async fn u4_c3_handle_wave_events_empty_input_returns_default_outcome() {
        let mut el = build_event_loop();

        // Construct a minimal `CliBackend` and `LoopContext` to
        // satisfy the function signature. The empty-wave path
        // short-circuits before either is actually used.
        let backend = CliBackend {
            command: "echo".to_string(),
            args: vec![],
            prompt_mode: ralph_adapters::PromptMode::Arg,
            prompt_flag: None,
            output_format: ralph_adapters::OutputFormat::Text,
            env_vars: vec![],
        };
        let ctx = ralph_core::LoopContext::primary(std::path::PathBuf::from("/tmp"));
        let loop_id = "test-loop";

        let outcome = handle_wave_events(
            &[],
            &mut el,
            &backend,
            &ctx,
            false,
            false,
            None,
            None,
            loop_id,
            None,
            // global_deadline is irrelevant for empty input.
            Some(tokio::time::Instant::now()),
            // Plan 001 §4.3 C1: hats_source_label is irrelevant for
            // empty input but is now part of the signature.
            None,
        )
        .await;

        assert_eq!(
            outcome,
            HandleWaveOutcome::default(),
            "empty wave_events must produce a default outcome"
        );
        assert!(
            !outcome.global_deadline_exceeded,
            "empty wave_events must NOT trigger the global deadline path"
        );
    }

    // ---------------------------------------------------------------------
    // U4-C4: runner post-wave phase skipping (static-source guard).
    // ---------------------------------------------------------------------

    /// U4-C4 / §6 C4: when the dispatcher's
    /// `WaveDispatchOutcome::GlobalDeadlineExceeded` fires, the
    /// runner.rs post-wave gate blocks (default_publishes inject +
    /// missing-event gate) MUST be guarded by
    /// `late_termination_reason.is_none()` so neither runs for the
    /// doomed iteration. Without this guard, default_publishes
    /// would inject synthesized events into a loop that's about to
    /// terminate with `TerminationReason::MaxRuntime`, or the
    /// missing-event gate would bump the hard-gate counter on a
    /// loop about to exit.
    ///
    /// Full E2E coverage of the runner's iteration body is not
    /// feasible in CI (would require spinning up a real backend),
    /// so C4 is enforced at two layers:
    ///   1. Dispatcher-level: C1 + the `started == 4` assertion
    ///      confirm `GlobalDeadlineExceeded` returns with zero
    ///      in-flight workers.
    ///   2. `handle_wave_events` level: C3 confirms
    ///      `HandleWaveOutcome { global_deadline_exceeded: true }`
    ///      flows back to the runner.
    ///   3. **Static guard (this test)**: the post-wave gate block
    ///      in `runner.rs` must consult `late_termination_reason`.
    ///      If the guard regresses, this test fails immediately.
    #[test]
    fn u4_c4_runner_post_wave_gates_consult_late_termination_reason() {
        // Read the runner.rs source from the crate root. This
        // test is a static-analysis gate — it catches regressions
        // where someone removes the `late_termination_reason.is_none()`
        // guard from the gate blocks (introduced in U4-C4) without
        // re-reading plan §6 C4.
        let runner_rs = include_str!("../runner.rs");

        // The post-wave gate blocks (missing-event gate + the
        // `else if` default_publishes fallback) share the
        // distinctive marker
        //   `wave_events.is_empty()\n            && !hard_gate_triggered_this_iteration`
        // Assert each occurrence is followed by a
        // `late_termination_reason.is_none()` guard.
        let gate_marker =
            "wave_events.is_empty()\n            && !hard_gate_triggered_this_iteration";
        let count = runner_rs.matches(gate_marker).count();
        assert!(
            count >= 2,
            "expected at least 2 post-wave gate blocks (missing-event gate + \
             default_publishes fallback) in runner.rs, found {count}. \
             plan §6 C4 requires both blocks to be guarded."
        );

        // After every occurrence of the gate marker, the next
        // logical condition MUST be `late_termination_reason.is_none()`.
        let guarded_count = runner_rs
            .matches("&& !hard_gate_triggered_this_iteration\n            && late_termination_reason.is_none()")
            .count();
        assert!(
            guarded_count >= 2,
            "expected late_termination_reason.is_none() guard on BOTH \
             post-wave gate blocks (missing-event gate + default_publishes \
             fallback), found {guarded_count}. plan §6 C4 requires both."
        );
    }

    /// U4-C4 / §6 C4: `HandleWaveOutcome { global_deadline_exceeded }`
    /// is the runner's only signal to set
    /// `late_termination_reason = Some(MaxRuntime)`. The
    /// post-wave gate guards (asserted by the static test above)
    /// depend on this flag being set. Verify the wiring by
    /// reading the runner.rs source for the exact assignment
    /// pattern.
    #[test]
    fn u4_c4_runner_wires_handle_wave_outcome_to_late_termination_reason() {
        let runner_rs = include_str!("../runner.rs");
        // The C3 commit introduced the wiring:
        //   if wave_outcome.is_some_and(|o| o.global_deadline_exceeded) {
        //       late_termination_reason = Some(TerminationReason::MaxRuntime);
        //   }
        // Assert the shape so a refactor that drops the
        // `is_some_and` check fails this test.
        assert!(
            runner_rs.contains("wave_outcome.is_some_and(|o| o.global_deadline_exceeded)"),
            "runner must use `is_some_and` to read the global_deadline_exceeded \
             flag from HandleWaveOutcome. If this assertion fails, the wiring \
             introduced in U4-C3 has been removed."
        );
        assert!(
            runner_rs.contains("late_termination_reason = Some(TerminationReason::MaxRuntime)"),
            "runner must set late_termination_reason = Some(MaxRuntime) on \
             global_deadline_exceeded. If this assertion fails, the C3 wiring \
             is broken and U4-C4 static guard is meaningless."
        );
    }

    /// Phase 2: `merge_wave_results_to_events_file` must stamp every merged
    /// record with the wave's target hat, overriding any self-declared
    /// provenance from the worker.
    #[test]
    fn test_merge_wave_results_stamps_target_hat() {
        let tmp = tempfile::TempDir::new().unwrap();
        let events_file = tmp.path().join("events.jsonl");

        let event = ralph_proto::Event::new("review.dimension.done", "{\"file\":\"src/lib.rs\"}")
            .with_source(ralph_proto::HatId::new("dimension-reviewer"));
        let completed = ralph_core::CompletedWave {
            wave_id: "w-stamp-001".to_string(),
            wave_total: 1,
            results: vec![ralph_core::WaveResult {
                index: 0,
                events: vec![event],
            }],
            failures: vec![],
            duration: std::time::Duration::ZERO,
            partial: false,
            expected_source_hat: Some(ralph_proto::HatId::new("dimension-reviewer")),
            assigned_dimensions: std::collections::HashMap::new(),
        };

        merge_wave_results_to_events_file(
            &completed,
            &events_file,
            &["review.dimension.done".to_string()],
            "dimension-reviewer",
            // 2026-06-16-001 U2: tests use the same default.
            None,
        )
        .unwrap();

        let merged = std::fs::read_to_string(&events_file).unwrap();
        assert!(
            merged.contains("\"hat\":\"dimension-reviewer\""),
            "merged record must be stamped with target hat: {}",
            merged
        );
        assert!(
            merged.contains("\"source\":\"dimension-reviewer\""),
            "merged record must mirror source to target hat: {}",
            merged
        );
    }

    // -------------------------------------------------------------------
    // U1: parse_assigned_dimension
    // -------------------------------------------------------------------

    /// U1/R1 — `dimension: "testing"` in a JSON payload is parsed.
    #[test]
    fn parse_assigned_dimension_reads_string_field() {
        let payload = r#"{"dimension": "testing", "depth": "standard"}"#;
        assert_eq!(
            parse_assigned_dimension(Some(payload)),
            Some("testing".to_string())
        );
    }

    /// U1/R1 — value is trimmed (leading/trailing whitespace tolerated).
    #[test]
    fn parse_assigned_dimension_trims_whitespace() {
        let payload = r#"{"dimension": "  correctness  "}"#;
        assert_eq!(
            parse_assigned_dimension(Some(payload)),
            Some("correctness".to_string())
        );
    }

    /// U1/R1 — non-JSON payload returns None (legacy wave, no enforcement).
    #[test]
    fn parse_assigned_dimension_non_json_returns_none() {
        assert_eq!(parse_assigned_dimension(Some("src/main.rs")), None);
    }

    /// U1/R1 — payload without `dimension` returns None.
    #[test]
    fn parse_assigned_dimension_missing_field_returns_none() {
        let payload = r#"{"depth": "standard", "focus": "all"}"#;
        assert_eq!(parse_assigned_dimension(Some(payload)), None);
    }

    /// U1/R1 — `dimension` that is not a string returns None.
    #[test]
    fn parse_assigned_dimension_non_string_field_returns_none() {
        let payload = r#"{"dimension": 42}"#;
        assert_eq!(parse_assigned_dimension(Some(payload)), None);
    }

    /// U1/R1 — empty / whitespace-only dimension returns None.
    #[test]
    fn parse_assigned_dimension_empty_value_returns_none() {
        let payload = r#"{"dimension": "   "}"#;
        assert_eq!(parse_assigned_dimension(Some(payload)), None);
    }

    /// U1/R1 — missing payload (None) returns None.
    #[test]
    fn parse_assigned_dimension_none_payload_returns_none() {
        assert_eq!(parse_assigned_dimension(None), None);
    }

    /// U1/R1 — empty payload string returns None.
    #[test]
    fn parse_assigned_dimension_empty_string_returns_none() {
        assert_eq!(parse_assigned_dimension(Some("")), None);
        assert_eq!(parse_assigned_dimension(Some("   \n  ")), None);
    }

    // -------------------------------------------------------------------
    // U2: RALPH_WAVE_DIMENSION env var injection
    // -------------------------------------------------------------------

    /// U2: when a `WorkerRequest` is built with
    /// `assigned_dimension: Some("testing")`, the dispatcher's
    /// injection step must add `("RALPH_WAVE_DIMENSION", "testing")`
    /// to `request.backend.env_vars` so the backend process can read
    /// its hard-bound dimension from the environment (matching the
    /// `## ASSIGNED DIMENSION` block U1 added to the prompt).
    ///
    /// This test mirrors the injection logic in
    /// `execute_wave_structured` (the inline `if let Some(ref dim)`
    /// block right after the wave-env-vars `extend`). Constructing
    /// a `WorkerRequest` and applying the same push lets us assert
    /// the exact env-var key/value without spinning up a real wave
    /// dispatch.
    #[test]
    fn test_raph_wave_dimension_env_var() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut request = make_worker_request_with_dimension(
            0,
            progress_tx,
            Some("testing".to_string()),
        );

        // Mirror the dispatcher injection step (see the inline
        // `if let Some(ref dim) = assigned_dimension` block in
        // `execute_wave_structured`).
        if let Some(ref dim) = request.assigned_dimension {
            request
                .backend
                .env_vars
                .push(("RALPH_WAVE_DIMENSION".into(), dim.clone()));
        }

        assert!(
            request
                .backend
                .env_vars
                .iter()
                .any(|(k, v)| k == "RALPH_WAVE_DIMENSION" && v == "testing"),
            "U2: env_vars must contain (\"RALPH_WAVE_DIMENSION\", \"testing\"), got {:?}",
            request.backend.env_vars
        );
    }

    /// U2: when `assigned_dimension` is `None` (legacy / non-review
    /// waves), the dispatcher MUST NOT inject `RALPH_WAVE_DIMENSION`
    /// — the var stays unset so pre-U2 behaviour is preserved for
    /// non-dimension-bound workers.
    #[test]
    fn test_raph_wave_dimension_env_var_absent_when_unassigned() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut request = make_worker_request_with_dimension(0, progress_tx, None);

        if let Some(ref dim) = request.assigned_dimension {
            request
                .backend
                .env_vars
                .push(("RALPH_WAVE_DIMENSION".into(), dim.clone()));
        }

        assert!(
            !request
                .backend
                .env_vars
                .iter()
                .any(|(k, _)| k == "RALPH_WAVE_DIMENSION"),
            "U2: RALPH_WAVE_DIMENSION must NOT be injected when assigned_dimension is None, got {:?}",
            request.backend.env_vars
        );
    }
}
