//! Dispatch module — wave entry points, per-wave context, worker request
//! plumbing, and the dispatch loop. Originally part of `wave/dispatcher.rs`
//! (plan `2026-08-07-008`). Public surface and behaviour preserved verbatim.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ralph_adapters::CliBackend;
use ralph_core::CompletedWave;
use ralph_core::diagnostics::DiagnosticsCollector;

pub(crate) const WORKER_TIMEOUT_ERR_PREFIX: &str = "Worker timed out after";

use ralph_proto::RpcEvent;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use tracing::{info, warn};

use super::super::io::{merge_wave_results_to_events_file, push_to_tui_iteration};
use super::super::worker::{WaveWorkerOutcome, run_wave_worker};
use super::super::{BindingInput, WaveChannelRegistry};
use super::deadlines::{
    PARTIAL_THRESHOLD_DEN, PARTIAL_THRESHOLD_NUM, aggregate_timeout_for,
    effective_detected_aggregate_deadline_secs, open_default_supervisor_store,
    parse_assigned_dimension,
};
use super::fan_in::{
    SupervisorFanInOutcome, TerminalFanInContext, drain_pending_compensations,
    run_supervisor_fan_in,
};
use super::outcomes::{
    ClassifiedReason, classify_slot_attempt, classify_slot_result, compute_slot_batch_fingerprint,
    finalize_global_exceeded, finalize_timeout, inject_synthetic_failures, merge_round_into,
    outcome_for_completion, record_loop_max_runtime_envelope, record_outcome,
    record_wave_spawn_failed_envelope, record_wave_timeout_envelope, reported_failure_detail,
    take_results, wait_for_progress_reporter,
};
use super::salvage::{append_wave_channel_to_marker, workspace_root_from_events};
use super::worker_lifecycle::SupervisorSlotRelease;
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
    /// 2026-07-13-001 plan U2: project config file path forwarded
    /// to each wave worker so its in-process `ralph tools task` /
    /// `ralph emit` discover the same project config the loop was
    /// started with via `RALPH_CONFIG`. `None` means do not inject
    /// (the worker keeps falling back to the parent process env).
    pub config_path: Option<&'a std::path::Path>,
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
    /// Atomic channel-registry preparation failed before any executor call.
    PreparationFailed {
        reason: &'static str,
        wave_id: String,
        source: super::super::channel_registry::ChannelRegistryError,
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
///
/// U1 (Green 7): `fan_in_failure` is true when the production
/// fan-in reached a terminal `ContinueCollect` that could not
/// converge (exhausted retry budget) or encountered a persistent
/// store/merge error. The runner maps this to a typed termination
/// reason distinct from `MaxRuntime`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HandleWaveOutcome {
    pub global_deadline_exceeded: bool,
    /// U1 (Green 7): terminal fan-in failure — persistent store/merge
    /// error or unresolvable `ContinueCollect`. The runner must NOT
    /// map this to `MaxRuntime`; it uses a separate typed reason.
    pub fan_in_failure: bool,
}

/// Per-worker request handed to a `WaveWorkerExecutor`.
///
/// The dispatcher is responsible for assembling the request (backend
/// resolved, prompt built, env vars injected, events file path
/// resolved). The executor only runs the future.
pub(crate) struct WorkerRequest {
    // 2026-07-28-003 plan U5 (E15 / KTD7): `#[derive(Clone)]`
    // so the supervisor task body can re-execute the request
    // in-place after a retryable failure. `CliBackend` derives
    // Clone; sender / Arc / PathBuf / Duration / String / Option
    // are all Clone; `tokio::sync::mpsc::Sender` is Clone.
    pub(crate) index: u32,
    pub(crate) backend: CliBackend,
    pub(crate) prompt: String,
    pub(crate) worker_events_path: PathBuf,
    pub(crate) worker_timeout: Duration,
    pub(crate) progress_tx: tokio::sync::mpsc::UnboundedSender<(u32, bool, Duration)>,
    /// Shared RPC channel used by `run_wave_worker` to push stream
    /// deltas. The production executor moves this out before
    /// running; the test executor leaves it as None.
    pub(crate) worker_rpc_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    /// Shared TUI state used by `run_wave_worker` to push per-line
    /// deltas. Same ownership semantics as `worker_rpc_tx`.
    pub(crate) worker_tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
    /// Dimension this worker is hard-bound to (R1). Parsed from the
    /// `review.wave.ready` payload's `dimension` field. `None` for
    /// waves that do not carry a dimension assignment (legacy
    /// waves, or non-review waves). When `Some`, the worker prompt
    /// and the `RALPH_WAVE_DIMENSION` env var surface this value,
    /// the CLI precheck enforces it (R3), and the merge layer drops
    /// any emitted `review.dimension.done` with a mismatched
    /// dimension (R4).
    pub(crate) assigned_dimension: Option<String>,
    /// 2026-07-03-001 supervisor real-wiring: per-worker cwd
    /// sourced from `SlotBinding.worktree_path`. `None` keeps the
    /// legacy `std::env::current_dir()` behaviour (the non-supervisor
    /// dispatcher path always sets `None` here).
    pub(crate) cwd: Option<PathBuf>,
    /// 2026-07-25-006 plan U6: idle heartbeat duration.
    /// `None` disables the dual-clock lease (legacy wall-clock only).
    /// `Some(0s)` is also treated as disabled by `DetectedWave`.
    pub(crate) idle_heartbeat: Option<Duration>,
    /// 2026-07-25-006 plan U6: weak-signal renewal cap.
    /// Only meaningful when `idle_heartbeat` is `Some`; the default
    /// (8) is set by `DetectedWave::idle_weak_signal_cap()`.
    pub(crate) idle_weak_signal_cap: u32,
    /// 2026-07-28-003 plan U3 (R1): wave worker startup grace window.
    /// Resolution mirrors `idle_heartbeat`: `None` disables the
    /// grace half of the dual-clock lease (`seen_first_signal`
    /// stays `false` forever in practice because no grace deadline
    /// is computed). `Some(0)` is collapsed upstream by
    /// `DetectedWave::startup_grace_secs()`. Effective only when
    /// `idle_heartbeat` is also `Some` (KTD1).
    pub(crate) startup_grace: Option<Duration>,
    /// 2026-07-30-001 plan U1: the supervisor wave kind this slot
    /// belongs to. `None` means "not dispatched through the
    /// supervisor" (the legacy `WaveTracker` path), which keeps the
    /// pre-plan attempt semantics: a worker-reported `*.unit.failed`
    /// terminal stays a Completed slot and is never retried.
    pub(crate) wave_kind: Option<ralph_core::supervisor::WaveKind>,
}

// 2026-07-28-003 plan U5 (E15 / KTD7): manual `Clone` impl
// instead of `#[derive(Clone)]` because `WorkerRequest` carries
// `progress_tx: tokio::sync::mpsc::UnboundedSender<…>` (cheap
// clone) and the rest is plain data. Keeping the manual impl
// explicit lets the type's SSoT for the dispatcher stay on a
// single readable block without `#[derive(...)]` macro clutter.
impl Clone for WorkerRequest {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            backend: self.backend.clone(),
            prompt: self.prompt.clone(),
            worker_events_path: self.worker_events_path.clone(),
            worker_timeout: self.worker_timeout,
            progress_tx: self.progress_tx.clone(),
            worker_rpc_tx: self.worker_rpc_tx.clone(),
            worker_tui_state: self.worker_tui_state.clone(),
            assigned_dimension: self.assigned_dimension.clone(),
            cwd: self.cwd.clone(),
            idle_heartbeat: self.idle_heartbeat,
            idle_weak_signal_cap: self.idle_weak_signal_cap,
            startup_grace: self.startup_grace,
            wave_kind: self.wave_kind,
        }
    }
}

/// 2026-07-28-003 plan U5 (A1 / A3): for **intermediate** retry
/// attempts the worker must NOT push progress / RPC / TUI
/// side-effects to the live dispatcher / TUI / RPC subscribers —
/// only the final attempt's outcome escapes. We swap each
/// shared sender for a fresh one whose receiver we drop on the
/// floor: `try_send` / `send` / `blocking_send` on those senders
/// remain infallible no-ops (the underlying `mpsc` buffer
/// accepts up to capacity, then no-ops the rest).
fn silent_request(orig: &WorkerRequest) -> WorkerRequest {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<(u32, bool, Duration)>();
    let (rpc_tx, _rpc_rx) = tokio::sync::mpsc::channel::<RpcEvent>(8);
    let tui_state = Arc::new(std::sync::Mutex::new(ralph_tui::TuiState::default()));
    let mut r = orig.clone();
    r.progress_tx = tx;
    r.worker_rpc_tx = Some(rpc_tx);
    r.worker_tui_state = Some(tui_state);
    r
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
            // 2026-07-28-003 plan U3 (R1): forward `startup_grace`
            // out of the request so the worker can plug it into
            // `LeaseConfig.startup_grace_ms`. `take()` is fine
            // because the request is moved into `run_wave_worker`
            // and never reused (the executor owns the dispatch).
            let startup_grace = request.startup_grace.take();
            run_wave_worker(
                request.index,
                &request.backend,
                &request.prompt,
                &request.worker_events_path,
                request.worker_timeout,
                request.idle_heartbeat,
                request.idle_weak_signal_cap,
                request.progress_tx,
                request.worker_rpc_tx.take(),
                request.worker_tui_state.take(),
                // 2026-07-03-001 supervisor real-wiring: forward
                // the per-worker cwd (from `SlotBinding.worktree_path`)
                // to the spawned worker process. `None` keeps the
                // legacy `std::env::current_dir()` behaviour.
                request.cwd.as_deref(),
                startup_grace,
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
    pub(crate) rpc_event_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    pub(crate) tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
}

/// Dispatch context shared by all workers in a wave.
///
/// KTD-U3-1: `started_at` is the single begin-time for the wave;
/// every other deadline (partial, aggregate, global) is derived from
/// it, so permit queue, worker execution, and result collection all
/// consume the same budget.
#[derive(Clone)]
pub(crate) struct DispatchContext {
    pub(crate) started_at: tokio::time::Instant,
    pub(crate) partial_deadline: tokio::time::Instant,
    pub(crate) aggregate_deadline: tokio::time::Instant,
    pub(crate) global_deadline: Option<tokio::time::Instant>,
    pub(crate) concurrency: usize,
    /// The declared wave total (may exceed events.len() in malformed partial waves).
    pub(crate) expected_total: u32,
    /// U2: actual number of events in this wave. Used for the spawn guarantee
    /// check — we must spawn exactly `events_len` workers, not `expected_total`.
    pub(crate) events_len: u32,
    /// 2026-07-23-001 plan U9: indices the synthetic-failure sweep
    /// in `dispatch_wave_inner_with_release` inspects. Legacy
    /// `build()` sets this to `0..wave.total` so partial waves
    /// still mark slots that never got a worker event. Supervisor
    /// per-round builders overwrite it with the indices actually
    /// spawned in that round — pending slots stay pending in the
    /// store and are dispatched in a later round, not marked
    /// failed here.
    pub(crate) sweep_indices: Vec<u32>,
    pub(crate) wave_id: String,
    pub(crate) payload_previews: Vec<String>,
    pub(crate) show_progress: bool,
    pub(crate) use_colors: bool,
    /// U1/R1 (2026-06-17-002): per-worker dimension assignment
    /// parsed from each `review.wave.ready` payload. Carried on
    /// the context so `execute_wave_structured` can stamp it
    /// onto the `CompletedWave` for the merge layer to read.
    pub(crate) assigned_dimensions: std::collections::HashMap<u32, String>,
}

impl DispatchContext {
    pub(crate) fn build(
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
        let partial_threshold = Duration::from_secs(
            aggregate_timeout
                .as_secs()
                .saturating_mul(PARTIAL_THRESHOLD_NUM)
                .div_ceil(PARTIAL_THRESHOLD_DEN),
        );
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
            // 2026-07-23-001 plan U9: legacy / non-supervisor waves
            // sweep `0..expected_total` so partial waves still
            // surface missing-slot synthetic failures. Supervisor
            // rounds construct via `build_supervisor_round` (below)
            // with a per-round `sweep_indices`.
            sweep_indices: (0..wave.total).collect(),
        }
    }

    /// 2026-07-23-001 plan U9: build a per-round supervisor
    /// dispatch context. `sweep_indices` is the round's spawned
    /// slot indices so the synthetic-failure sweep never marks a
    /// still-pending slot as failed. `expected_total` is the
    /// round's spawned count (must match the per-round tracker
    /// registration so spawned_count / expected_count stay
    /// consistent).
    fn build_supervisor_round(
        wave: &ralph_core::DetectedWave,
        worker_timeout: Duration,
        aggregate_timeout: Duration,
        expected_total: u32,
        sweep_indices: Vec<u32>,
        payload_previews: Vec<String>,
        show_progress: bool,
        use_colors: bool,
        limits: WaveDispatchLimits,
        assigned_dimensions: std::collections::HashMap<u32, String>,
    ) -> Self {
        let mut ctx = Self::build(
            wave,
            worker_timeout,
            aggregate_timeout,
            payload_previews,
            show_progress,
            use_colors,
            limits,
            assigned_dimensions,
        );
        ctx.expected_total = expected_total;
        ctx.events_len = expected_total;
        ctx.sweep_indices = sweep_indices;
        ctx
    }
}

/// Publish the wave-start state before any worker future is polled.
///
/// Worker output is emitted while `execute_wave_structured` is awaiting the
/// backends. Initializing the TUI/RPC state here keeps the live wave visible
/// during that wait instead of only after the wave has completed.
fn announce_wave_start(detected: &ralph_core::DetectedWave, out: &WaveOutputs<'_>) {
    let wave_timeout_secs = detected.timeout_secs();

    info!(
        wave_id = %detected.wave_id,
        total = detected.total,
        hat = %detected.target_hat,
        concurrency = detected.hat_config.concurrency,
        "Wave detected, executing parallel workers"
    );

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
    // 2026-07-13-001 plan U2: project config file path forwarded
    // to each wave worker so its in-process `ralph tools task` /
    // `ralph emit` discover the same project config the loop was
    // started with via `RALPH_CONFIG`. `None` means do not inject
    // (the worker keeps falling back to the parent process env).
    _config_path: Option<&std::path::Path>,
    // 2026-07-03-001 supervisor real-wiring: when `Some`, the
    // dispatcher takes the supervisor path (`register_wave_if_absent`
    // → `bind_slot` per slot → `dispatch_wave_inner` with per-worker
    // cwd → `run_supervisor_fan_in`). When `None`, the legacy
    // `WaveTracker` path runs unchanged (R3 / KTD-7).
    supervisor_bridge: Option<&Arc<dyn ralph_core::supervisor::SupervisorBridge>>,
) -> HandleWaveOutcome {
    let max_wave_total = event_loop.config().event_loop.max_wave_total;
    // U4-C3: accumulator for the per-wave outcomes. Set to true
    // when any wave hits the runner-supplied global deadline so
    // the runner can map to `TerminationReason::MaxRuntime`.
    let mut result = HandleWaveOutcome::default();
    // U5/R5 (2026-06-17-002): the per-slot retry budget is
    // persisted on the WaveTracker itself (see
    // `try_consume_dimension_retry` / `dimension_retry_count` in
    // `wave_tracker.rs`). The previous process-local HashMap was
    // reset on every `handle_wave_events` call, allowing a
    // permanently-mismatched worker to loop indefinitely. The
    // tracker-owned budget survives across dispatch rounds, so
    // once `MAX_DIMENSION_RETRIES_PER_SLOT` is consumed for a
    // given `(wave_id, wave_index)` slot the merge layer's
    // `wave.worker.failed(reason=dimension_mismatch)` is the
    // terminal signal — see plan line 101.
    let outcome = ralph_core::detect_all_wave_events_capped(
        wave_events,
        event_loop.registry(),
        ralph_core::PartialWavePolicy::RequireComplete,
        max_wave_total,
    );

    // 2026-07-13-001 plan U2: read the loop's resolved project
    // config path once and forward it to every wave worker. Clone
    // the `PathBuf` so the immutable borrow on `event_loop` is
    // released before later mutable calls.
    let config_path: Option<std::path::PathBuf> = event_loop
        .config()
        .config_path
        .as_deref()
        .and_then(|path| (!path.as_os_str().is_empty()).then_some(path))
        .map(|p| p.to_path_buf());
    let config_path_ref = config_path.as_deref();

    let out = WaveOutputs {
        use_colors,
        show_cli: tui_state.is_none() && !enable_rpc,
        rpc_tx: rpc_event_tx,
        tui: tui_state,
        hats_source_label,
        config_path: config_path_ref,
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

    // 2026-07-22-001 plan U2 (KTD-1 / KTD-3): default wave path must
    // route through `SupervisorStore` so cancellation, idempotency,
    // and content-hash dedup are uniformly available — not the
    // legacy `WaveTracker` island. The runner only constructs a
    // production bridge when `supervisor.enabled: true`; for any
    // other preset that emits a DetectedWave we **lazily** construct
    // an in-memory bridge here. Pure pipeline runs (no
    // DetectedWave) never reach this line, so the 023 R1
    // "no wave → no DB / no bridge" invariant still holds.
    //
    // The bridge is intentionally `Arc::clone`'d into a local
    // owned `Option<Arc<dyn SupervisorBridge>>` so the borrow on
    // `supervisor_bridge` is released before downstream mutable
    // calls. The cloned bridge is shared across iterations so the
    // store accumulates the full wave history of this loop run.
    let accepted_len = waves.len();
    let supervisor_cfg = event_loop.config().event_loop.supervisor.clone();
    let lazy_bridge: Option<Arc<dyn ralph_core::supervisor::SupervisorBridge>> =
        if supervisor_bridge.is_some() {
            supervisor_bridge.cloned()
        } else if accepted_len > 0 {
            use crate::loop_runner::wave::{CoordinatorSupervisorBridge, ProductionBridgeContext};
            use ralph_core::supervisor::worktree_bind::DefaultWorktreeFactory;
            let cap = u32::MAX; // default path uses the per-wave cap; U5 refines.
            // 2026-07-22-001 plan U3 (KTD-2): prefer the rusqlite
            // store when the `supervisor-db` feature is on AND the
            // operator configured a `db_path`. The runner has
            // already attempted `recover_active_waves_at_startup`
            // on the same store during startup, so any in-flight
            // waves from a prior crash are reconciled by the time
            // we get here. Falls back to `InMemorySupervisorStore`
            // with a `wave_ledger_ephemeral` stderr warning when
            // the rusqlite path is unavailable so an operator can
            // see exactly why ledger writes do not survive a
            // restart.
            let store: Arc<dyn ralph_core::supervisor::SupervisorStore> =
                match open_default_supervisor_store(&supervisor_cfg, ctx, &main_events_file) {
                    Ok(s) => s,
                    Err(err) => {
                        tracing::error!(
                            error = %err,
                            "default wave path failed to open supervisor store; aborting wave (fail-closed per 2026-07-22-001 plan U3 / KTD-2)"
                        );
                        result.global_deadline_exceeded = true;
                        return result;
                    }
                };
            let bridge = CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
                store,
                ProductionBridgeContext {
                    loop_id: loop_id.to_string(),
                    repo_root: std::path::PathBuf::from("."),
                    events_path: Some(main_events_file.clone()),
                    // 2026-07-23-007 plan U4 (R-W5): hand the bridge the
                    // loop's `tasks.jsonl` path so the default wave path
                    // projects slot transitions onto the runtime task
                    // ledger (same derivation as `runner.rs`: events
                    // file's parent `.ralph` dir + `agent/tasks.jsonl`).
                    tasks_path: main_events_file
                        .parent()
                        .map(|p| p.join("agent").join("tasks.jsonl"))
                        .or_else(|| Some(std::path::PathBuf::from(".ralph/agent/tasks.jsonl"))),
                },
                Arc::new(DefaultWorktreeFactory),
                cap,
                // 2026-07-28-003 plan U4 (KTD6): default wave path
                // is a feature-flag / BDD seam, not operator-facing;
                // pin at the historical default (1) so legacy tests
                // stay bit-for-bit identical.
                1,
            );
            Some(Arc::new(bridge) as Arc<dyn ralph_core::supervisor::SupervisorBridge>)
        } else {
            None
        };
    let supervisor_bridge_owned: Option<Arc<dyn ralph_core::supervisor::SupervisorBridge>> =
        lazy_bridge.or_else(|| supervisor_bridge.cloned());
    let supervisor_bridge: Option<&Arc<dyn ralph_core::supervisor::SupervisorBridge>> =
        supervisor_bridge_owned.as_ref();

    // Announce every wave before starting its workers. This must happen
    // before the concurrent `join_all` below: workers can emit progress as
    // soon as they start, and the TUI needs `wave_active` initialized before
    // those deltas arrive (otherwise the user sees a silent wait and `w`
    // cannot enter Wave View).
    for detected in &waves {
        announce_wave_start(detected, &out);
    }

    // Execute independent detected waves concurrently. The per-wave
    // executor owns its worker lifecycle; supervisor fan-in is protected by
    // the bridge's shared lock so the main event stream remains serialized.
    // Keep the outcomes keyed by the public wave id so the existing
    // deterministic reporting path below can process them in detection order.
    let mut wave_outcomes = futures::future::join_all(waves.iter().map(|detected| {
        let rpc_tx = out.rpc_tx.cloned();
        let tui = out.tui.map(Arc::clone);
        let hats_source = out.hats_source_label;
        let config_path = out.config_path;
        let bridge = supervisor_bridge;
        let events_file = main_events_file.clone();
        async move {
            let outcome = execute_wave_structured(
                detected,
                backend,
                &events_file,
                out.show_cli,
                out.use_colors,
                rpc_tx,
                tui,
                loop_id,
                WaveDispatchLimits { global_deadline },
                hats_source,
                config_path,
                bridge,
            )
            .await;
            (detected.wave_id.clone(), outcome)
        }
    }))
    .await
    .into_iter()
    .collect::<std::collections::HashMap<_, _>>();

    for detected in waves {
        let wave_outcome = wave_outcomes
            .remove(&detected.wave_id)
            .expect("every detected wave must have one execution outcome");

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

        // U1 (Green 1 / R1): every dispatch completion is terminal for fan-in —
        // Completed / Partial / AggregateDeadlineExceeded all preserve real
        // elapsed/timeout context so coordinator can leave Collect.
        let wave_is_terminal = matches!(
            &wave_outcome,
            WaveDispatchOutcome::Completed(_)
                | WaveDispatchOutcome::Partial(_)
                | WaveDispatchOutcome::AggregateDeadlineExceeded(_)
        );
        let wave_cancel_requested = matches!(
            &wave_outcome,
            WaveDispatchOutcome::AggregateDeadlineExceeded(_)
        );

        // U4-B3: timeout / partial outcomes must feed the recovery
        // responder, not just be folded into a generic Err. We
        // dispatch on the structured `WaveDispatchOutcome` here.
        match wave_outcome {
            WaveDispatchOutcome::Completed(completed)
            | WaveDispatchOutcome::Partial(completed)
            | WaveDispatchOutcome::AggregateDeadlineExceeded(completed) => {
                // U5/R5 (P0#1 fix): we must mutate
                // `dimension_retry_counts` on the CompletedWave
                // to consume the per-slot retry quota before
                // the merge call (which uses the tracker-side
                // view). Take the CompletedWave by `mut`
                // binding so the filter below can bump the
                // counts and append the JSONL in one shot.
                let mut completed = completed;
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
                // mismatch list and the pre-rendered `task.resume`
                // JSONL records from the merge layer. The merge
                // call already wrote the synthetic
                // `wave.worker.failed` records and the per-event
                // records in a single `write_all`. The dispatcher
                // filters the pending `task.resume` records through
                // the WaveTracker's per-slot retry budget and
                // appends the survivors to the SAME events file in
                // a single `write_all` (no separate
                // `inject_dimension_retry_task_resume` file
                // open / `writeln!` interleaving — fixes P0#4).
                //
                // **U7b (plan 2026-06-21-002):** the
                // `pending_task_resumes` path is preserved for
                // backwards compatibility (the feature flag
                // `UNIFIED_DETERMINISTIC_CORRECTION` is off by
                // default). When the flag is on, dimension
                // mismatch should be expressed as a
                // [`crate::correction::CorrectionContext`] block
                // in the next hat prompt; U9 will migrate the
                // production code to the new API.
                // U6: the supervisor path merges the per-slot business
                // events through the bridge's production sink and injects
                // the unique `*.wave.complete` / `*.wave.failed`
                // coordination event (with the success_slots payload).
                // It replaces the legacy `merge_wave_results_to_events_file`
                // path below so the business events are not double-written.
                if let Some(bridge) = supervisor_bridge {
                    let aggregate_timeout_secs =
                        effective_detected_aggregate_deadline_secs(&detected, bridge.as_ref());
                    // U1 (Green 1): build TerminalFanInContext for terminal dispatches.
                    // Completed / Partial / AggregateDeadlineExceeded require the fan-in
                    // to drive to convergence rather than returning ContinueCollect
                    // with no owner (this call site runs only once per dispatch).
                    let terminal_ctx = wave_is_terminal.then_some(TerminalFanInContext {
                        cancel_requested: wave_cancel_requested,
                        elapsed: completed.duration,
                    });
                    let fan_in = run_supervisor_fan_in(
                        bridge,
                        &completed,
                        &detected,
                        &main_events_file,
                        aggregate_timeout_secs,
                        terminal_ctx,
                    );
                    info!(
                        wave_id = %completed.wave_id,
                        fan_in = ?fan_in,
                        "U6: supervisor fan-in tick completed"
                    );
                    // U1 (Green 7 / S5): store/merge failure OR orphan ContinueCollect
                    // after a terminal dispatch must stop the run — there is no
                    // next wave-detection tick that owns retry.
                    if matches!(
                        fan_in,
                        SupervisorFanInOutcome::StoreError
                            | SupervisorFanInOutcome::MergeFailed
                            | SupervisorFanInOutcome::ContinueCollect
                    ) {
                        result.fan_in_failure = true;
                    }
                } else {
                    let (mismatch_info, pending_task_resumes) =
                        match merge_wave_results_to_events_file(
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
                            Ok(parts) => parts,
                            Err(e) => {
                                warn!(error = %e, "Failed to merge wave results to events file");
                                (Vec::new(), Vec::new())
                            }
                        };

                    // U5/R5: filter the pending `task.resume` records
                    // through the per-slot retry budget carried on the
                    // CompletedWave (the tracker transferred it via
                    // `take_wave_results`, so the budget persists
                    // across dispatch rounds — P0#1 fix). Survivors
                    // are appended to the events file in a single
                    // `write_all` (P0#4 fix — no separate
                    // file-open/`writeln!` interleaving). A write
                    // failure does NOT roll back the budget: the count
                    // was already bumped before the disk syscall, so a
                    // future dispatch sees the slot as exhausted and
                    // the wave terminates via the existing
                    // `wave.worker.failed` (P1#11 fix).
                    if !pending_task_resumes.is_empty() {
                        use std::io::Write;
                        let mut resume_buf = String::new();
                        let mut injected = 0usize;
                        // Build a per-round increment map and apply
                        // it to the CompletedWave counts after we've
                        // chosen which records to inject. This keeps
                        // the budget consistent even when the file
                        // write fails.
                        let mut round_increments: std::collections::HashMap<u32, u32> =
                            std::collections::HashMap::new();
                        for pending in &pending_task_resumes {
                            let used = completed
                                .dimension_retry_counts
                                .get(&pending.wave_index)
                                .copied()
                                .unwrap_or(0);
                            if used >= ralph_core::MAX_DIMENSION_RETRIES_PER_SLOT {
                                tracing::debug!(
                                    wave_id = %completed.wave_id,
                                    wave_index = pending.wave_index,
                                    used,
                                    "U5/R5: dimension retry budget exhausted; skipping task.resume"
                                );
                                continue;
                            }
                            resume_buf.push_str(&pending.jsonl_line);
                            resume_buf.push('\n');
                            *round_increments.entry(pending.wave_index).or_insert(0) += 1;
                            injected += 1;
                        }
                        // Apply the increments up-front. If the file
                        // write fails below, the budget still reflects
                        // the consumed retries — the slot is now
                        // exhausted, so subsequent dispatches will
                        // skip the task.resume injection (no
                        // infinite-loop on disk failure).
                        for (idx, inc) in &round_increments {
                            let prev = completed
                                .dimension_retry_counts
                                .get(idx)
                                .copied()
                                .unwrap_or(0);
                            completed.dimension_retry_counts.insert(*idx, prev + inc);
                        }
                        if injected > 0 {
                            let write_result = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&main_events_file)
                                .map_err(anyhow::Error::from)
                                .and_then(|mut f| {
                                    f.write_all(resume_buf.as_bytes())
                                        .map_err(anyhow::Error::from)
                                });
                            if let Err(e) = write_result {
                                warn!(
                                    error = %e,
                                    injected,
                                    "U5/R5: failed to write task.resume events to events file; \
                                     retry budget already consumed, slot is now exhausted"
                                );
                            } else {
                                tracing::info!(
                                    wave_id = %completed.wave_id,
                                    injected,
                                    mismatched = mismatch_info.len(),
                                    "U5/R5: injected task.resume events to retry dimension-reviewer"
                                );
                            }
                        }
                    }
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
            WaveDispatchOutcome::PreparationFailed {
                reason,
                wave_id,
                source,
            } => {
                warn!(%wave_id, reason, error = %source, "Wave channel preparation failed before spawn");
                if let Some(state) = out.tui
                    && let Ok(mut s) = state.lock()
                {
                    s.wave_active_iteration_idx.take();
                    s.wave_active.take();
                }
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
    if any_success
        && let Ok(reread) = event_loop.process_events_from_jsonl_with_waves()
        && reread.processed.had_events
    {
        info!("Published wave result events to bus for aggregator");
        // Wave results legitimately share the same topic (e.g.
        // 3x review.done). Reset the stale-loop counter so
        // this batch doesn't trigger LoopStale termination.
        event_loop.reset_stale_topic_counter();
    }
    result
}

/// Atomically authorize every slot channel before any worker executor can run.
pub(crate) fn prepare_wave_worker_channels(
    main_events_file: &Path,
    loop_id: &str,
    wave_id: &str,
    slots: impl IntoIterator<Item = (u32, PathBuf)>,
) -> std::result::Result<
    super::super::channel_registry::WaveChannelRegistryGuard,
    super::super::channel_registry::ChannelRegistryError,
> {
    let workspace_root = workspace_root_from_events(main_events_file);
    let bindings = slots
        .into_iter()
        .map(|(slot_index, channel_path)| {
            super::super::channel_registry::BindingInput::new(slot_index, channel_path)
        })
        .collect::<Vec<_>>();
    super::super::channel_registry::WaveChannelRegistry::prepare(
        &workspace_root,
        loop_id,
        wave_id,
        &bindings,
    )
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
        // 2026-07-13-001 plan U2: legacy wrapper has no
        // runner-supplied config path; the dispatcher keeps
        // falling back to the parent process env.
        None,
        // 2026-07-03-001 supervisor real-wiring: legacy wrapper
        // has no supervisor bridge; the dispatcher takes the
        // `WaveTracker` path.
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
        WaveDispatchOutcome::PreparationFailed {
            reason,
            wave_id,
            source,
        } => Err(anyhow::anyhow!(
            "Wave {wave_id} preparation failed ({reason}): {source}"
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
    // 2026-07-13-001 plan U2: project config file path forwarded
    // to each wave worker so its in-process `ralph tools task` /
    // `ralph emit` discover the same project config the loop was
    // started with via `RALPH_CONFIG`. `None` means do not inject
    // the env var (worker keeps falling back to its own discovery).
    config_path: Option<&std::path::Path>,
    // 2026-07-03-001 supervisor real-wiring: when `Some`, the
    // dispatcher delegates to `execute_wave_via_supervisor`
    // (per-slot worktree + `register_wave_if_absent` + `bind_slot`).
    // When `None`, the legacy `WaveTracker` path runs unchanged.
    supervisor_bridge: Option<&Arc<dyn ralph_core::supervisor::SupervisorBridge>>,
) -> WaveDispatchOutcome {
    use ralph_core::{WaveTracker, WaveWorkerContext, build_wave_worker_prompt};

    // 2026-07-03-001 supervisor real-wiring: take the supervisor
    // branch when a bridge is supplied. The legacy path below
    // stays unchanged (R3 / KTD-7 — `supervisor.enabled: false`
    // keeps the `WaveTracker` shape).
    if let Some(bridge) = supervisor_bridge {
        let executor: Arc<ProductionExecutor> = Arc::new(ProductionExecutor);
        return execute_wave_via_supervisor_with_executor(
            wave,
            global_backend,
            main_events_file,
            show_progress,
            use_colors,
            rpc_event_tx,
            tui_state,
            loop_id,
            limits,
            hats_source_label,
            config_path,
            bridge,
            executor as Arc<dyn WaveWorkerExecutor>,
            None, // pre_registered_id: not pre-registered in normal dispatch path
            None, // slot_index_override: events-array position is authoritative
        )
        .await;
    }

    let concurrency = wave.hat_config.concurrency as usize;
    let wave_timeout = Duration::from_secs(wave.per_worker_timeout_secs());
    // 2026-07-25-006 plan U6: resolve idle heartbeat config from DetectedWave.
    // `idle_heartbeat_secs() == None` disables the dual-clock lease.
    // `Some(0s)` is also disabled per DetectedWave semantics.
    // `idle_heartbeat_secs()` returns `Option<u32>`; widen to `u64` for
    // `Duration::from_secs` (which takes `u64`). `None` / `Some(0)` is
    // already collapsed to `None` by `DetectedWave::idle_heartbeat_secs`.
    let idle_heartbeat: Option<Duration> = wave
        .idle_heartbeat_secs()
        .map(|secs| Duration::from_secs(secs as u64));
    let idle_weak_signal_cap = wave.idle_weak_signal_cap();
    // 2026-07-28-003 plan U3 (R1): startup_grace resolved through
    // `DetectedWave::startup_grace_secs` — `None` / `Some(0)`
    // already collapsed to `None` upstream.
    let startup_grace: Option<Duration> = wave
        .startup_grace_secs()
        .map(|secs| Duration::from_secs(secs as u64));
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

        // Create per-worker events file. Channel authorization is committed
        // once for the complete request set after this loop.
        let worker_events_file = wave_dir.join(format!("wave-{}-{}.jsonl", wave_id, index_u32));

        // Build worker prompt
        let ctx = WaveWorkerContext {
            wave_id: wave_id.clone(),
            wave_index: index_u32,
            wave_total: wave.total,
            result_topics: hat_config.publishes.clone(),
            assigned_dimension: assigned_dimension.clone(),
            // 2026-07-30-001 plan U2: the first attempt has no retry
            // history. The dispatcher appends the rendered retry block
            // to this prompt when it re-dispatches the slot.
            retry: None,
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
            main_events_file
                .parent()
                .and_then(Path::parent)
                .unwrap_or(Path::new(".")),
            &worker_events_file,
            None,
            hats_source_label,
            config_path,
        );

        // Apply hat backend args
        if let Some(ref args) = hat_config.backend_args {
            worker_backend.args.extend(args.iter().cloned());
        }

        ralph_adapters::apply_hat_tool_policy(&mut worker_backend, &hat_config.disallowed_tools);

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
            // 2026-07-03-001 supervisor real-wiring: legacy path
            // has no per-worker worktree; `None` keeps the
            // `std::env::current_dir()` behaviour. The supervisor
            // path overrides this via `execute_wave_via_supervisor`.
            cwd: None,
            // 2026-07-30-001 plan U1: the legacy dispatcher never
            // enables the Exec reported-failure retry.
            wave_kind: None,
            idle_heartbeat,
            idle_weak_signal_cap,
            // 2026-07-28-003 plan U3 (R1): forward the hat-configured
            // startup grace to the worker. Both `None` and `Some(0)`
            // were collapsed to `None` by `DetectedWave::startup_grace_secs`.
            startup_grace,
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

    let mut registry_guard = match prepare_wave_worker_channels(
        main_events_file,
        loop_id,
        &wave.wave_id,
        worker_requests
            .iter()
            .map(|request| (request.index, request.worker_events_path.clone())),
    ) {
        Ok(guard) => guard,
        Err(source) => {
            return WaveDispatchOutcome::PreparationFailed {
                reason:
                    ralph_core::supervisor::worker_outcome::REASON_WAVE_CHANNEL_REGISTRATION_FAILED,
                wave_id: wave.wave_id.clone(),
                source,
            };
        }
    };

    let outcome = dispatch_wave_inner(
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
    .await;
    let _ = registry_guard.cleanup();
    outcome
}

/// 2026-07-03-001 supervisor real-wiring: dispatch a wave
/// through the supervisor path. This mirrors
/// `execute_wave_structured`'s worker-build loop but:
/// 1. calls `bridge.register_wave_if_absent` once per wave
/// 2. calls `bridge.bind_slot` per slot to obtain the
///    `SlotBinding { env, worktree_path }`
/// 3. merges `binding.env` into `worker_backend.env_vars`
///    (overwriting same-name keys)
/// 4. sets `WorkerRequest.cwd = binding.worktree_path`
/// 5. uses an empty `WaveTracker::new()` (spawn mechanism is
///    fully reused via `dispatch_wave_inner`; the supervisor
///    path does not consume the tracker's count)
///
/// The fan-in (`run_supervisor_fan_in`) is invoked by
/// `handle_wave_events` AFTER this function returns, so this
/// function only owns spawn + collect.
/// Thin compatibility wrapper that constructs a
/// `ProductionExecutor` and delegates to
/// `execute_wave_via_supervisor_with_executor`. Kept as a
/// separate function so the production call site
/// (`execute_wave_structured`) stays readable and the test
/// path can inject a counting executor via the
/// `*_with_executor` variant.
#[allow(dead_code)]
async fn execute_wave_via_supervisor(
    wave: &ralph_core::DetectedWave,
    global_backend: &CliBackend,
    main_events_file: &Path,
    show_progress: bool,
    use_colors: bool,
    rpc_event_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
    loop_id: &str,
    limits: WaveDispatchLimits,
    hats_source_label: Option<&str>,
    config_path: Option<&std::path::Path>,
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
) -> WaveDispatchOutcome {
    let executor: Arc<ProductionExecutor> = Arc::new(ProductionExecutor);
    execute_wave_via_supervisor_with_executor(
        wave,
        global_backend,
        main_events_file,
        show_progress,
        use_colors,
        rpc_event_tx,
        tui_state,
        loop_id,
        limits,
        hats_source_label,
        config_path,
        bridge,
        executor as Arc<dyn WaveWorkerExecutor>,
        None, // pre_registered_id: not pre-registered in normal dispatch path
        None, // slot_index_override: events-array position is authoritative
    )
    .await
}

/// 2026-07-23-001 plan U3: same as `execute_wave_via_supervisor`
/// but accepts an injected `WaveWorkerExecutor` so tests can
/// count how many workers actually spawn without spawning real
/// processes. The public `execute_wave_structured` always passes
/// `Arc::new(ProductionExecutor)`; tests substitute their own
/// executor (e.g. `U3CountingExecutor`) to drive the gate under
/// test.
///
/// 2026-07-28-002 plan U4 (S6): when `pre_registered_id` is `Some`,
/// the caller has already registered the wave in the store (e.g. a
/// redrive child created by `create_redrive_wave` at boot). The
/// dispatcher skips `register_wave_if_absent` and uses the provided
/// `store_wave_id` directly, verifying the wave exists to fail-closed
/// on a missing row.
pub(crate) async fn execute_wave_via_supervisor_with_executor(
    wave: &ralph_core::DetectedWave,
    global_backend: &CliBackend,
    main_events_file: &Path,
    show_progress: bool,
    use_colors: bool,
    rpc_event_tx: Option<tokio::sync::mpsc::Sender<RpcEvent>>,
    tui_state: Option<Arc<std::sync::Mutex<ralph_tui::TuiState>>>,
    loop_id: &str,
    limits: WaveDispatchLimits,
    hats_source_label: Option<&str>,
    config_path: Option<&std::path::Path>,
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    executor: Arc<dyn WaveWorkerExecutor>,
    // U4 S6: when Some, the wave was already registered in the store
    // (e.g. a redrive child). The dispatcher skips register_wave_if_absent
    // and verifies the wave exists to fail-closed on a missing row.
    pre_registered_id: Option<&str>,
    // 2026-07-28-002 plan U3/U4 (C3): when `Some`, every slot in this
    // dispatch uses this store slot index instead of its position in
    // `wave.events`. Redrive child dispatch synthesizes a single-event
    // wave whose real store slot is the child slot index (which can be
    // 1 or 2 in a multi-slot child wave), not the events-array position
    // (always 0). Without this override every child slot would mis-bind
    // to slot 0.
    slot_index_override: Option<u32>,
) -> WaveDispatchOutcome {
    use ralph_core::supervisor::{SupervisorBridge as _, WaveKind};
    use ralph_core::{WaveTracker, WaveWorkerContext, build_wave_worker_prompt};

    // 2026-08-06-002 plan U2: the previous inline formula computed
    // `effective_cap` and `aggregate_timeout` from `wave.hat_config.concurrency`,
    // `wave.events.len()`, and bridge runtime params. That logic now lives
    // in `effective_detected_aggregate_deadline_secs` and is invoked below;
    // any future helper change must keep the helper's signature in lock-step.
    let wave_timeout = Duration::from_secs(wave.per_worker_timeout_secs());
    // 2026-07-25-006 plan U6: resolve idle heartbeat config from DetectedWave.
    // `idle_heartbeat_secs()` returns `Option<u32>`; convert to `Duration` here
    // so the worker signature stays in `Duration` (compatible with the legacy
    // `wave_timeout: Duration` path). `None` / `Some(0)` is already collapsed to
    // `None` by `DetectedWave::idle_heartbeat_secs`.
    let idle_heartbeat: Option<Duration> = wave
        .idle_heartbeat_secs()
        .map(|secs| Duration::from_secs(secs as u64));
    let idle_weak_signal_cap = wave.idle_weak_signal_cap();
    // 2026-07-28-003 plan U3 (R1): symmetric to the legacy path.
    // Mirrors `idle_heartbeat` resolution above.
    let startup_grace: Option<Duration> = wave
        .startup_grace_secs()
        .map(|secs| Duration::from_secs(secs as u64));
    // 2026-07-30-001 plan U3 / U3 KTD-5: the local effective cap is
    // `min(hat.concurrency, bridge.max_concurrent_workers())`. The deadline
    // helper computes its own copy of this value internally; both call sites
    // share the same formula. The local is also consumed downstream as the
    // per-round admission gate.
    let effective_cap: u32 = wave
        .hat_config
        .concurrency
        .min(bridge.max_concurrent_workers())
        .max(1);
    let aggregate_timeout = Duration::from_secs(effective_detected_aggregate_deadline_secs(
        wave,
        bridge.as_ref(),
    ));

    // 2026-07-03-001 supervisor real-wiring: infer the wave
    // kind from the first event's topic. `review.*` (both
    // `review.wave.ready` and `review.unit.ready`) → Review;
    // `fix.*` → Fix; everything else → Exec (the default for
    // `exec.unit.ready` / `exec.wave.ready`).
    //
    // 2026-07-23-001 plan U9: widened `review.wave.` → `review.`
    // so the builtin `parallel-forge` preset's review
    // wave (trigger topic `review.unit.ready`, emitted by the
    // `review-coordinator` hat) is correctly classified Review
    // and dispatches via `SharedReadonly` instead of accidentally
    // taking the Exec path's per-slot worktree binding.
    let trigger_topic = wave.events.first().map(|e| e.topic.as_str()).unwrap_or("");
    let wave_kind = if trigger_topic.starts_with("review.") {
        WaveKind::Review
    } else if trigger_topic.starts_with("fix.") {
        WaveKind::Fix
    } else {
        WaveKind::Exec
    };

    // Idempotently register the wave in the supervisor store.
    // The store allocates its own `w-{seq}` id; we keep using
    // the dispatcher's wave_id for logs but use the store id
    // for subsequent `bind_slot` / `record_slot_result` / `tick`
    // calls so the coordinator reads the same row.
    //
    // 2026-07-22-001 plan U2: the previous `register_wave_if_absent`
    // failure path fell back to the legacy `WaveTracker` dispatch —
    // that re-opened the OPAC / register-double-spawn gap the
    // supervisor store was designed to close. Register errors now
    // fail closed so callers see the root cause (DB open failure,
    // constraint conflict, etc.) instead of a silently different
    // dispatch shape.
    //
    // 2026-07-28-002 plan U4 (S6): when `pre_registered_id` is `Some`,
    // the wave was already registered by the boot redrive scan. Skip
    // `register_wave_if_absent` and verify the wave exists to fail-closed
    // on a missing row.
    //
    // 2026-07-28-003 plan U4 (R8 / R14 / S13): in the register branch,
    // read the budget from the bridge (which surfaces
    // `SupervisorConfig::slot_retry_budget`) so this call and the mirror
    // call in `run_supervisor_fan_in` (fan-in path) always agree on the
    // exact same value (memory.rs:388-400 reports an inconsistency as a
    // store error).
    let store_wave_id = if let Some(pre_id) = pre_registered_id {
        // Verify the pre-registered wave exists in the store.
        // `fan_in_status` returns the wave snapshot; an error means
        // the row is absent or corrupted → fail-closed.
        match bridge.fan_in_status(pre_id) {
            Ok(_) => pre_id.to_string(),
            Err(err) => {
                tracing::warn!(
                    wave_id = %wave.wave_id,
                    pre_registered_id = %pre_id,
                    error = %err,
                    "pre-registered wave not found in store; aborting redrive (fail-closed per 2026-07-28-002 plan U4 S6)"
                );
                return WaveDispatchOutcome::SpawnFailed {
                    spawned_count: 0,
                    expected_count: wave.total,
                };
            }
        }
    } else {
        match bridge.register_wave_if_absent(
            wave_kind,
            &wave.wave_id,
            wave.total,
            bridge.slot_retry_budget(),
        ) {
            Ok(id) => id,
            Err(err) => {
                // 2026-07-22-001 plan U2: register errors fail closed.
                // Map to `SpawnFailed { expected = total, spawned = 0 }`
                // so the runner can write a `wave_spawn_failed`
                // RecoveryDiagnosisEnvelope and the outer dispatcher can
                // convert the error uniformly. The previous code's
                // fallback to legacy `WaveTracker` dispatch re-opened
                // the OPAC register-double-spawn gap; surfacing the
                // error keeps the supervisor as the single source of
                // truth.
                tracing::warn!(
                    wave_id = %wave.wave_id,
                    error = %err,
                    "supervisor register_wave_if_absent failed; aborting wave (fail-closed per 2026-07-22-001 plan U2)"
                );
                return WaveDispatchOutcome::SpawnFailed {
                    spawned_count: 0,
                    expected_count: wave.total,
                };
            }
        }
    };

    // Supervisor path uses an empty tracker; the spawn mechanism
    // 2026-07-03-001 supervisor real-wiring: the join / permit
    // release / progress plumbing in `dispatch_wave_inner_with_release`
    // drives batched dispatch below. `sweep_indices` (set by
    // per-round builders) keeps the synthetic-failure sweep from
    // marking still-pending slots as failed; failing that, the
    // outer batch loop in this function dispatches the leftover
    // slots in subsequent rounds once their permits are released.
    //
    // 2026-07-23-001 plan U9 (closes the U4 "fifth slot starts
    // after release" contract): the slot loop below PREPARES every
    // slot (prompt / backend / worktree binding) without consulting
    // the store. The wave-level round loop then asks the store to
    // approve up to `effective_cap` pending slots per round,
    // spawns + joins that round's workers, and repeats. Each
    // released permit lets the next round approve its FIFO slot,
    // so a wave wider than the cap (e.g. 5-slot exec wave under
    // cap=4) does not silently drop the trailing slots.

    let wave_dir = main_events_file
        .parent()
        .unwrap_or(Path::new(".ralph"))
        .to_path_buf();

    struct PreparedSlot {
        index: u32,
        request: Option<WorkerRequest>,
        preview: String,
        dimension: Option<String>,
    }

    let mut prepared: Vec<PreparedSlot> = Vec::with_capacity(wave.events.len());

    for (index, event) in wave.events.iter().enumerate() {
        let wave_id = wave.wave_id.clone();
        // 2026-07-28-002 plan U3/U4 (C3): redrive single-slot dispatch
        // passes the true child slot index; normal waves use the
        // events-array position.
        let index_u32 = slot_index_override.unwrap_or(index as u32);
        let hat_config = wave.hat_config.clone();
        let assigned_dimension = parse_assigned_dimension(event.payload.as_deref());
        let preview = event.payload.as_deref().unwrap_or("").replace('\n', " ");

        let worker_events_file = wave_dir.join(format!("wave-{}-{}.jsonl", wave_id, index_u32));
        let ctx = WaveWorkerContext {
            wave_id: wave_id.clone(),
            wave_index: index_u32,
            wave_total: wave.total,
            result_topics: hat_config.publishes.clone(),
            assigned_dimension: assigned_dimension.clone(),
            // 2026-07-30-001 plan U2: the first attempt has no retry
            // history. The dispatcher appends the rendered retry block
            // to this prompt when it re-dispatches the slot.
            retry: None,
        };
        let prompt = build_wave_worker_prompt(&hat_config, event, &ctx);

        let mut worker_backend = if let Some(ref hat_backend) = hat_config.backend {
            CliBackend::from_hat_backend(hat_backend).unwrap_or_else(|_| global_backend.clone())
        } else {
            global_backend.clone()
        };

        worker_backend.env_vars.extend([
            ("RALPH_WAVE_WORKER".into(), "1".into()),
            ("RALPH_WAVE_ID".into(), wave_id.clone()),
            ("RALPH_WAVE_INDEX".into(), index_u32.to_string()),
            (
                "RALPH_EVENTS_FILE".into(),
                worker_events_file.display().to_string(),
            ),
        ]);

        if let Some(ref dim) = assigned_dimension {
            worker_backend
                .env_vars
                .push(("RALPH_WAVE_DIMENSION".into(), dim.clone()));
        }

        inject_hat_execution_env(
            &mut worker_backend,
            wave.target_hat.as_str(),
            loop_id,
            main_events_file
                .parent()
                .and_then(Path::parent)
                .unwrap_or(Path::new(".")),
            &worker_events_file,
            None,
            hats_source_label,
            config_path,
        );

        if let Some(ref args) = hat_config.backend_args {
            worker_backend.args.extend(args.iter().cloned());
        }

        ralph_adapters::apply_hat_tool_policy(&mut worker_backend, &hat_config.disallowed_tools);

        // U1: per-slot binding (fail-closed on error; SharedReadonly
        // for review kinds returns Ok(None)).
        let binding = match bridge.bind_slot(wave_kind, &store_wave_id, index_u32) {
            Ok(opt) => opt,
            Err(err) => {
                use crate::loop_runner::wave::fail_closed_on_bind_error;
                let closed = fail_closed_on_bind_error(&err, &wave.wave_id, index_u32);
                debug_assert!(
                    closed.is_some(),
                    "bind_slot error must be fail-closed; got {err:?}"
                );
                continue;
            }
        };

        // U1 KTD-4: Exec/Fix MUST get a worktree binding.
        if binding.is_none() && !matches!(wave_kind, WaveKind::Review) {
            warn!(
                wave_id = %wave.wave_id,
                slot_index = index_u32,
                wave_kind = ?wave_kind,
                "supervisor bind_slot returned Ok(None) for Exec/Fix slot; \
                 failing closed (slot skipped, no main-workspace spawn)"
            );
            continue;
        }

        let slot_cwd = binding.as_ref().and_then(|b| b.worktree_path.clone());

        // 2026-07-23-007 plan U2 (R-W1): validate the per-worker
        // events channel against the primary control plane so the
        // spawned worker can never write a JSONL ledger inside
        // its own slot subtree or escape the workspace via a
        // symlink. Review slots still get a binding-less / shared
        // read-only path (the validator's slot_worktree_root arg
        // stays None so the slot-subtree rule is exempt).
        let workspace_root = bridge.repo_root();
        let validated_events_path = match workspace_root {
            Some(root) => {
                let slot_root_for_validate = match wave_kind {
                    WaveKind::Review => None,
                    _ => slot_cwd.as_deref(),
                };
                match ralph_core::control_plane::validate_control_plane_binding(
                    &worker_events_file,
                    slot_root_for_validate,
                    root,
                ) {
                    Ok(p) => Some(p),
                    Err(err) => {
                        // Fail-closed: a slot whose channel binding
                        // is invalid MUST NOT spawn. Mark this
                        // index as "already spawned" so the next
                        // approval round skips it; the actual
                        // record_slot_failure is the inner
                        // dispatcher's responsibility — but since
                        // we never push a WorkerRequest here, the
                        // slot will simply have no worker, which
                        // the wave's terminal cleanup will pick up
                        // as a never-reported slot (mirrors the
                        // bind-failure path).
                        let reason = ralph_core::control_plane::reason_for(&err);
                        let reason_string = reason.to_string();
                        warn!(
                            wave_id = %wave.wave_id,
                            slot_index = index_u32,
                            wave_kind = ?wave_kind,
                            events_path = %worker_events_file.display(),
                            error = %err,
                            "U2: control-plane binding rejected; failing closed (slot skipped, no spawn)"
                        );
                        // Best-effort: try to record the failure on
                        // the bridge so the store sees a structured
                        // reason even though no worker ran. If the
                        // store / bridge is unavailable, fall back
                        // to skipping silently (the synthetic-failure
                        // sweep would catch it later anyway).
                        let _ =
                            bridge.record_slot_failure(&store_wave_id, index_u32, &reason_string);
                        continue;
                    }
                }
            }
            None => None,
        };

        if let Some(ref b) = binding {
            // Merge binding env (last-write-wins), BUT the dispatcher
            // already injected `RALPH_WAVE_ID = public wave id` at
            // line ~1582 (the value the agent saw in
            // `DetectedWave.wave_id`). `bind_slot`'s `binding.env`
            // carries the **store** id (the `w-{seq}` the store
            // allocated) because `bind_worktree` needs the store id
            // to find the row. Plan 2026-07-25-003 U4 (R6) requires
            // the worker's `RALPH_WAVE_ID` to be the public id so
            // envelope / business record wave_id stays consistent
            // with what the operator and the agent see. We therefore
            // exclude `RALPH_WAVE_ID` from the binding-env merge:
            // the dispatcher's earlier public-id injection is the
            // last word, and the store id never leaks into the
            // spawned worker's environment.
            let binding_env: std::collections::HashMap<String, String> = b
                .env
                .iter()
                .filter(|(k, _)| k.as_str() != "RALPH_WAVE_ID")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            worker_backend
                .env_vars
                .retain(|(k, _)| !binding_env.contains_key(k));
            worker_backend
                .env_vars
                .extend(binding_env.iter().map(|(k, v)| (k.clone(), v.clone())));
        }

        // 2026-07-23-007 plan U2 (R-W1): inject the
        // `RALPH_WORKSPACE_ROOT` + `RALPH_EVENTS_FILE` binding as
        // the SSOT for the spawned worker. `merge_event_channel_env`
        // is the canonical validator; on success the validated
        // absolute paths land in `worker_backend.env_vars`. On
        // failure (relative path that escaped earlier checks) the
        // slot is fail-closed the same way as a binding rejection.
        if let (Some(root), Some(events_path)) = (workspace_root, validated_events_path.as_ref()) {
            let mut extras: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            if let Err(err) =
                ralph_core::control_plane::merge_event_channel_env(root, events_path, &mut extras)
            {
                let reason = ralph_core::control_plane::reason_for(&err);
                let reason_string = reason.to_string();
                warn!(
                    wave_id = %wave.wave_id,
                    slot_index = index_u32,
                    error = %err,
                    "U2: merge_event_channel_env rejected binding; failing closed"
                );
                let _ = bridge.record_slot_failure(&store_wave_id, index_u32, &reason_string);
                continue;
            }
            for (k, v) in extras {
                worker_backend
                    .env_vars
                    .retain(|(existing, _)| existing != &k);
                worker_backend.env_vars.push((k, v));
            }
        }

        let (progress_tx, _) = tokio::sync::mpsc::unbounded_channel::<(u32, bool, Duration)>();
        let worker_rpc_tx = rpc_event_tx.clone();
        let worker_tui_state = tui_state.clone();

        // 2026-07-28-002 plan U3 (R3 / S2a): persist the SlotDescriptor
        // after bind_slot succeeds and all pre-spawn validation has passed.
        // This makes the bounded activation record available for redrive
        // before the worker process is actually spawned. Fail-closed: if
        // the store is unavailable or returns an error, skip this slot
        // entirely (no WorkerRequest pushed).
        //
        // U4 (S6): skip persistence for pre-registered redrive dispatches —
        // the descriptor was taken FROM the store to synthesize this wave,
        // so re-persisting would be redundant (and would drop the
        // `slot_index_in_parent` anchor that `take` already consumed).
        if pre_registered_id.is_none()
            && let Some(store) = bridge.store()
        {
            use ralph_core::supervisor::{SlotDescriptor, fingerprint_payload};
            // `wave.events[i].payload` is `Option<String>` from
            // `ralph_core::event_reader::Event` (not `ralph_proto::Event`).
            let payload_json = event.payload.clone().unwrap_or_default();
            let descriptor = SlotDescriptor {
                slot_index: index_u32,
                topic: event.topic.clone(),
                payload_json: payload_json.clone(),
                wave_kind,
                payload_digest: fingerprint_payload(&payload_json),
                slot_index_in_parent: None,
            };
            if let Err(err) = store.persist_slot_descriptor(&store_wave_id, &descriptor) {
                // Fail-closed: a slot whose descriptor cannot be persisted
                // MUST NOT spawn. Record the failure on the bridge so the
                // store sees a structured reason.
                warn!(
                    wave_id = %wave.wave_id,
                    slot_index = index_u32,
                    wave_kind = ?wave_kind,
                    error = %err,
                    "U3: persist_slot_descriptor failed; failing closed (slot skipped)"
                );
                let _ = bridge.record_slot_failure(
                    &store_wave_id,
                    index_u32,
                    &format!("persist_slot_descriptor failed: {err}"),
                );
                continue;
            }
        }

        prepared.push(PreparedSlot {
            index: index_u32,
            request: Some(WorkerRequest {
                index: index_u32,
                backend: worker_backend,
                prompt,
                worker_events_path: worker_events_file,
                worker_timeout: wave_timeout,
                progress_tx,
                worker_rpc_tx,
                worker_tui_state,
                assigned_dimension: assigned_dimension.clone(),
                cwd: slot_cwd,
                // 2026-07-30-001 plan U1: supervisor slots carry their
                // typed wave kind so the attempt classifier can treat
                // an Exec worker's own `exec.unit.failed` as a
                // retryable attempt without touching Review / Fix.
                wave_kind: Some(wave_kind),
                idle_heartbeat,
                idle_weak_signal_cap,
                // 2026-07-28-003 plan U3 (R1): forward startup grace
                // through the supervisor dispatch path. Same accessors
                // as the legacy path.
                startup_grace,
            }),
            preview,
            dimension: assigned_dimension,
        });
    }

    let _registry_guard = match prepare_wave_worker_channels(
        main_events_file,
        loop_id,
        &wave.wave_id,
        wave.events.iter().enumerate().map(|(index, _)| {
            (
                index as u32,
                wave_dir.join(format!("wave-{}-{}.jsonl", wave.wave_id, index)),
            )
        }),
    ) {
        Ok(guard) => guard,
        Err(source) => {
            let reason =
                ralph_core::supervisor::worker_outcome::REASON_WAVE_CHANNEL_REGISTRATION_FAILED;
            for index in 0..wave.total {
                let _ = bridge.record_slot_failure(&store_wave_id, index, reason);
            }
            return WaveDispatchOutcome::PreparationFailed {
                reason,
                wave_id: wave.wave_id.clone(),
                source,
            };
        }
    };

    // Approval rounds: each round lets up to `effective_cap`
    // pending slots dispatch in parallel; permits released by
    // earlier rounds (via inner's per-worker SlotGuard) make the
    // FIFO slots waiting in the next round approvable.
    let mut spawned: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    let mut merged_wave: Option<ralph_core::CompletedWave> = None;
    let mut terminal_outcome: Option<WaveDispatchOutcome> = None;

    loop {
        let mut round_requests: Vec<WorkerRequest> = Vec::new();
        let mut round_previews: Vec<String> = Vec::new();
        let mut round_dims: std::collections::HashMap<u32, String> =
            std::collections::HashMap::new();
        let mut round_indices: Vec<u32> = Vec::new();
        let mut approved_this_round: u32 = 0;

        for slot in prepared.iter_mut() {
            if spawned.contains(&slot.index) {
                continue;
            }
            if approved_this_round >= effective_cap {
                // Remaining pending slots wait for the next round
                // after released permits free up.
                break;
            }
            // U3 KTD-1..KTD-4: store approval gates every spawn
            // (FIFO/backpressure). `Ok(false)` keeps the slot
            // pending for the next round instead of dropping it
            // (the U4 contract: "the fifth slot starts after
            // release"). `Err` is fail-closed: never retry the
            // slot.
            match bridge.try_dispatch_next(&store_wave_id, slot.index) {
                Ok(true) => {
                    approved_this_round += 1;
                    spawned.insert(slot.index);
                    round_indices.push(slot.index);
                    round_previews.push(slot.preview.clone());
                    if let Some(ref dim) = slot.dimension {
                        round_dims.insert(slot.index, dim.clone());
                    }
                    round_requests.push(
                        slot.request
                            .take()
                            .expect("prepared slot's request is taken once on first approval"),
                    );
                }
                Ok(false) => continue,
                Err(err) => {
                    warn!(
                        wave_id = %wave.wave_id,
                        slot_index = slot.index,
                        store_wave_id = %store_wave_id,
                        error = %err,
                        "supervisor dispatch: try_dispatch_next returned Err; \
                         failing closed (slot skipped, no spawn)"
                    );
                    spawned.insert(slot.index);
                    continue;
                }
            }
        }

        if round_requests.is_empty() {
            // Either every prepared slot was spawned (or fail-closed
            // at bind time) or the store approved nothing this round
            // (U3 "dispatcher awaits store approval" gate).
            break;
        }

        let mut round_tracker = WaveTracker::new();
        round_tracker.register_wave_with_source(
            wave.wave_id.clone(),
            round_requests.len() as u32,
            Some(wave.target_hat.clone()),
        );
        let round_ctx = DispatchContext::build_supervisor_round(
            wave,
            wave_timeout,
            aggregate_timeout,
            round_requests.len() as u32,
            round_indices,
            round_previews,
            show_progress,
            use_colors,
            limits,
            round_dims,
        );
        let round_outcome = dispatch_wave_inner_with_release(
            round_tracker,
            round_requests,
            round_ctx,
            executor.clone(),
            ProgressChannels {
                rpc_event_tx: rpc_event_tx.clone(),
                tui_state: tui_state.clone(),
            },
            Some(Arc::clone(bridge)),
            Some(store_wave_id.clone()),
            Some(loop_id.to_string()),
        )
        .await;

        match round_outcome {
            WaveDispatchOutcome::Completed(round) | WaveDispatchOutcome::Partial(round) => {
                merge_round_into(&mut merged_wave, round);
            }
            WaveDispatchOutcome::AggregateDeadlineExceeded(round) => {
                merge_round_into(&mut merged_wave, round);
                // 2026-07-22-001 plan U4 (KTD-8): aggregate-timeout
                // teardown is the canonical cancel signal. We mark
                // the store wave as cancelled so any subsequent
                // coordinator tick observes the new phase and so
                // / inspect surfaces it. The store-level cancel
                // does not itself kill the spawned worker child;
                // dispatch_wave_inner_with_release's deadline path
                // owns the process kill (kill-on-deadline is wired
                // up there already).
                if let Err(err) = bridge.cancel_wave(&store_wave_id) {
                    tracing::warn!(
                        wave_id = %store_wave_id,
                        error = %err,
                        "supervisor cancel_wave on aggregate timeout failed; \
                         the dispatcher will still kill in-flight workers"
                    );
                }
                // 2026-07-22-001 plan U6 (KTD-7): enqueue a
                // compensation hook so a subsequent coordinator
                // tick observes the failure mode and runs the
                // diagnostic / cleanup record.
                if let Err(err) = bridge.enqueue_compensation(
                    &store_wave_id,
                    ralph_core::supervisor::CompensationKind::OnTimeout,
                ) {
                    tracing::debug!(
                        wave_id = %store_wave_id,
                        error = %err,
                        "supervisor enqueue_compensation(OnTimeout) no-op"
                    );
                }
                terminal_outcome = merged_wave
                    .take()
                    .map(WaveDispatchOutcome::AggregateDeadlineExceeded);
                break;
            }
            WaveDispatchOutcome::GlobalDeadlineExceeded => {
                // U4: same cancel marker on global deadline so the
                // ledger reflects the operator-visible "cancelled"
                // state, not the implicit "failed" state.
                if let Err(err) = bridge.cancel_wave(&store_wave_id) {
                    tracing::warn!(
                        wave_id = %store_wave_id,
                        error = %err,
                        "supervisor cancel_wave on global deadline failed"
                    );
                }
                if let Err(err) = bridge.enqueue_compensation(
                    &store_wave_id,
                    ralph_core::supervisor::CompensationKind::OnCancel,
                ) {
                    tracing::debug!(
                        wave_id = %store_wave_id,
                        error = %err,
                        "supervisor enqueue_compensation(OnCancel) no-op"
                    );
                }
                terminal_outcome = Some(WaveDispatchOutcome::GlobalDeadlineExceeded);
                break;
            }
            preparation_failed @ WaveDispatchOutcome::PreparationFailed { .. } => {
                terminal_outcome = Some(preparation_failed);
                break;
            }
            spawn_failed @ WaveDispatchOutcome::SpawnFailed { .. } => {
                // 2026-07-22-001 plan U4: also mark the store
                // wave as cancelled so a subsequent inspect /
                // diagnose call shows the abort, not a phantom
                // "in-flight" entry.
                if let Err(err) = bridge.cancel_wave(&store_wave_id) {
                    tracing::debug!(
                        wave_id = %store_wave_id,
                        error = %err,
                        "supervisor cancel_wave on spawn failure no-op"
                    );
                }
                // 2026-07-22-001 plan U6: spawn failure is also a
                // compensation candidate (diagnostics hook).
                if let Err(err) = bridge.enqueue_compensation(
                    &store_wave_id,
                    ralph_core::supervisor::CompensationKind::OnCancel,
                ) {
                    tracing::debug!(
                        wave_id = %store_wave_id,
                        error = %err,
                        "supervisor enqueue_compensation(OnCancel) on spawn failure no-op"
                    );
                }
                terminal_outcome = Some(spawn_failed);
                break;
            }
        }
    }

    if let Some(outcome) = terminal_outcome {
        return outcome;
    }

    let mut completed = match merged_wave {
        Some(merged) => merged,
        None => {
            // No round spawned anything (every slot fail-closed at
            // bind time, or the store never approved — U3 gate).
            // Replay the legacy empty-wave pass so the tracker
            // surfaces the full-wave synthetic failures, mirroring
            // the pre-U9 single-pass behaviour on this edge case.
            let mut tracker = WaveTracker::new();
            tracker.register_wave_with_source(
                wave.wave_id.clone(),
                wave.total,
                Some(wave.target_hat.clone()),
            );
            let full_indices: Vec<u32> = (0..wave.total).collect();
            let full_previews: Vec<String> = wave
                .events
                .iter()
                .map(|e| e.payload.as_deref().unwrap_or("").replace('\n', " "))
                .collect();
            return dispatch_wave_inner_with_release(
                tracker,
                Vec::new(),
                DispatchContext::build_supervisor_round(
                    wave,
                    wave_timeout,
                    aggregate_timeout,
                    wave.total,
                    full_indices,
                    full_previews,
                    show_progress,
                    use_colors,
                    limits,
                    std::collections::HashMap::new(),
                ),
                executor,
                ProgressChannels {
                    rpc_event_tx,
                    tui_state,
                },
                Some(Arc::clone(bridge)),
                Some(store_wave_id),
                Some(loop_id.to_string()),
            )
            .await;
        }
    };
    // Normalize back to the full wave's totals before handing off
    // (per-round CompletedWaves carry round-scoped totals).
    completed.wave_total = wave.total;
    completed.partial = completed.partial || (completed.results.len() as u32) < wave.total;
    outcome_for_completion(completed)
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
    tracker: ralph_core::WaveTracker,
    worker_requests: Vec<WorkerRequest>,
    ctx: DispatchContext,
    executor: Arc<E>,
    progress: ProgressChannels,
) -> WaveDispatchOutcome {
    dispatch_wave_inner_with_release(
        tracker,
        worker_requests,
        ctx,
        executor,
        progress,
        None,
        None,
        None,
    )
    .await
}

/// Supervisor variant of the dispatch loop. The terminal bridge is
/// notified from each joined worker path so store capacity is returned
/// before the next wave asks for approval.
async fn dispatch_wave_inner_with_release<E: WaveWorkerExecutor + ?Sized>(
    mut tracker: ralph_core::WaveTracker,
    worker_requests: Vec<WorkerRequest>,
    ctx: DispatchContext,
    executor: Arc<E>,
    progress: ProgressChannels,
    terminal_bridge: Option<Arc<dyn ralph_core::supervisor::SupervisorBridge>>,
    terminal_wave_id: Option<String>,
    terminal_loop_id: Option<String>,
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
    // worker_request MUST produce a worker task. Track the count so we can
    // assert after the loop and return SpawnFailed if any requests were
    // silently dropped.
    //
    // 2026-07-23-001 plan U3: under the supervisor gate, the
    // caller's `worker_requests` may be a strict subset of the
    // wave's events (skipped slots are not pushed). The spawn
    // guarantee still runs against `worker_requests.len()` so
    // we only fail when the spawn loop itself drops requests —
    // not when the supervisor gate intentionally skipped slots.
    let worker_request_count = worker_requests.len();
    let mut join_set: tokio::task::JoinSet<(u32, WaveWorkerOutcome)> = tokio::task::JoinSet::new();
    let mut spawned_count = 0u32;
    for request in worker_requests {
        let semaphore = Arc::clone(&semaphore);
        let executor = Arc::clone(&executor);
        let terminal_bridge = terminal_bridge.clone();
        let terminal_wave_id = terminal_wave_id.clone();
        let terminal_loop_id = terminal_loop_id.clone();
        let request_index = request.index;
        // Replace the placeholder progress_tx with the real sender.
        let mut request = request;
        request.progress_tx = progress_tx.clone();
        // 2026-07-28-003 plan U5 (A2): bring the wave-level
        // partial / aggregate deadlines into the worker task so
        // the in-task retry loop can stop retrying once the
        // dispatcher-level budget expires (instead of letting a
        // single retryable slot burn the entire wave_timeout).
        let retry_partial_deadline = ctx.partial_deadline;
        let retry_aggregate_deadline = ctx.aggregate_deadline;

        join_set.spawn(async move {
            // The Drop guard is installed before waiting on the local
            // semaphore, so JoinSet abort/cancellation also releases
            // the store-side permit for an approved slot.
            //
            // 2026-07-28-003 plan U5: capture the retry budget
            // BEFORE moving `terminal_bridge` into the guard so
            // the attempt loop can read it. `None` legacy path
            // collapses to a budget of `0` (no retries), keeping
            // the pre-U5 bit-for-bit semantics.
            let retry_budget: u32 = terminal_bridge
                .as_ref()
                .map(|b| b.slot_retry_budget())
                .unwrap_or(0);
            // 2026-08-07-009 plan U2 (KTD5): clone the store +
            // wave-id from the bridge BEFORE the existing
            // release_guard move consumes them. The receipt API
            // takes `&Arc<dyn SupervisorStore>` so a cheap clone
            // keeps both alive for the rest of the task.
            let slot_attempt_store: Option<std::sync::Arc<dyn ralph_core::supervisor::SupervisorStore>> =
                terminal_bridge.as_ref().and_then(|b| b.store());
            let slot_wave_id = terminal_wave_id.clone();
            let slot_index_local = request_index;
            let cwd_path: Option<std::path::PathBuf> = request.cwd.clone();
            let mut release_guard =
                terminal_bridge
                    .zip(terminal_wave_id)
                    .map(|(bridge, wave_id)| SupervisorSlotRelease {
                        bridge,
                        wave_id,
                        slot_index: request_index,
                        outcome: ralph_core::supervisor::DispatchOutcome::Failed,
                    });
            // 2026-07-23-007 plan U4 (R-W5): bring the loop id into
            // the worker task so the post-terminal task projection
            // can build the stable task_key. The clone is moved
            // into the task; the outer `terminal_loop_id` is
            // captured-by-move so each iteration needs its own
            // `clone()` above.
            let terminal_loop_id = terminal_loop_id;
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
            // until the worker future completes. We don't expose
            // the permit to the executor; the executor does not
            // need it.
            let _permit = permit;

            // 2026-07-28-003 plan U5 (R9 / R10 / R11 / R12 / R13):
            // wrapper around `executor.execute(request).await` that
            // retries on a retryable, frozen-code failure as long
            // as the bridge's `slot_retry_budget()` has not been
            // exhausted. The loop runs IN this task (KTD7) so
            // harvest / store / tracker never see the intermediate
            // attempt — the dispatcher only consumes the final
            // attempt's outcome (KTD9). `WorkerRequest: Clone`
            // (U5 E15) lets us re-enter `execute()` after a retry
            // decision without consuming the request.
            //
            // Mid-attempt contract (A1 / A3): each non-final
            // attempt's WorkerRequest has its `progress_tx`,
            // `worker_rpc_tx` and `worker_tui_state` swapped for
            // a no-op channel so the dispatcher / TUI / RPC
            // subscribers only see the FINAL attempt's outcome.
            //
            // Deadline contract (A2): before each retry the loop
            // checks the wave-level partial / aggregate deadlines;
            // once either fires we break out of the loop and let
            // the final attempt's outcome take the natural record
            // path — we never extend a single slot beyond the
            // wave's budget.
            // 2026-07-30-001 plan U1: the wave kind decides whether a
            // worker-reported `*.unit.failed` terminal counts as a
            // failed attempt (Exec) or as a Completed slot (Review /
            // Fix / legacy). Captured before the request is moved into
            // the attempt loop.
            let slot_wave_kind = request.wave_kind;
            let result = {
                use ralph_core::supervisor::worker_outcome::REASON_EXECUTOR_REPORTED_FAILURE;
                let mut attempt: u32 = 1;
                // 2026-07-28-003 plan U5: use the budget we captured
                // before moving `terminal_bridge` into the guard.
                let budget = retry_budget;
                // 2026-07-30-001 plan U2: a retried worker gets the
                // ORIGINAL prompt plus a rendered retry block, so the
                // block never stacks across attempts.
                //
                // 2026-08-07-009 plan U3 (R5 / S7 / S10 / S12): when
                // this slot is a redrive child whose parent
                // attempts are durable in the store, append the
                // bounded `# Recovery Context` to the base
                // prompt so the child sees the cross-restart
                // history ONCE (not per retry). A query failure
                // collapses to an empty context (the renderer
                // returns "") so the dispatcher never fabricates
                // a row (S12). History is bounded to
                // `RECOVERY_MAX_PARENT_ATTEMPTS` by the renderer
                // constructor.
                let mut base_prompt = request.prompt.clone();
                if let (Some(store), Some(wid)) =
                    (slot_attempt_store.as_ref(), slot_wave_id.as_ref())
                {
                    match store.parent_slot_attempts(
                        wid,
                        slot_index_local,
                        Some(ralph_core::wave_prompt::RECOVERY_MAX_PARENT_ATTEMPTS as u32),
                    ) {
                        Ok(history) => {
                            // Detect "Worktree reused" by comparing
                            // the parent's resource path to the
                            // current request cwd. The bridge
                            // already decided whether reuse was
                            // safe; this branch is best-effort
                            // so the renderer can label the
                            // prompt accurately.
                            let reuse = match store.parent_slot_resource(wid, slot_index_local) {
                                Ok(Some(res)) => match (&res.worktree_path, &cwd_path) {
                                    (Some(p), Some(cwd)) if p == &cwd.to_string_lossy() => {
                                        ralph_core::wave_prompt::WorktreeReuse::Reused
                                    }
                                    _ => ralph_core::wave_prompt::WorktreeReuse::Fresh,
                                },
                                _ => ralph_core::wave_prompt::WorktreeReuse::Fresh,
                            };
                            let ctx = ralph_core::wave_prompt::RecoveryContext::new(
                                reuse,
                                history.attempts,
                            );
                            let rendered =
                                ralph_core::wave_prompt::render_recovery_context(&ctx);
                            if !rendered.is_empty() {
                                base_prompt.push_str(&rendered);
                            }
                        }
                        Err(err) => {
                            tracing::warn!(
                                slot_index = slot_index_local,
                                wave_id = %wid,
                                error = %err,
                                "parent_slot_attempts query failed; rendering without Recovery Context"
                            );
                        }
                    }
                }
                let max_attempts = budget.saturating_add(1);
                let mut prior_attempts: Vec<ralph_core::PriorAttempt> = Vec::new();
                let mut current_request = request;
                let last_outcome: (u32, WaveWorkerOutcome) = loop {
                    // 2026-08-07-009 plan U2 (R1 / R3 / KTD5):
                    // begin a fresh attempt receipt (if the bridge
                    // exposes a store + a wave id). Fail-soft:
                    // any IO error is logged at warn level and
                    // the loop continues without the receipt —
                    // the Worker outcome is unchanged. The
                    // receipt's `attempt_seq` is owned by the
                    // store inside its transaction, so concurrent
                    // attempts on the same slot would have
                    // distinct seqs (memory: Mutex; rusqlite:
                    // BEGIN IMMEDIATE).
                    let current_attempt_seq: Option<u32> = if let (Some(store), Some(wid)) =
                        (slot_attempt_store.as_ref(), slot_wave_id.as_ref())
                    {
                        let start_cp = match cwd_path.as_ref() {
                            Some(p) => tokio::task::spawn_blocking({
                                let p = p.clone();
                                move || ralph_core::worktree::capture_git_checkpoint(&p)
                            })
                            .await
                            .ok()
                            .flatten(),
                            None => None,
                        };
                        let started_at_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        match store.begin_slot_attempt(
                            wid,
                            slot_index_local,
                            start_cp,
                            started_at_ms,
                        ) {
                            Ok(receipt) => Some(receipt.attempt_seq),
                            Err(err) => {
                                tracing::warn!(
                                    slot_index = slot_index_local,
                                    wave_id = %wid,
                                    attempt,
                                    error = %err,
                                    "begin_slot_attempt failed; continuing without receipt"
                                );
                                None
                            }
                        }
                    } else {
                        None
                    };
                    let outcome = executor.execute(current_request.clone()).await;
                    let (_idx, res) = &outcome;
                    let classified = classify_slot_attempt(res, slot_wave_kind);
                    // 2026-08-07-009 plan U2 (R1 / KTD3-KTD5):
                    // finish the attempt receipt using the
                    // classifier's verdict. Running receipt → no
                    // finish (the dispatcher never sees this
                    // branch in the production path because the
                    // Worker has just returned). Successful →
                    // status=succeeded + no failure_code.
                    // Retryable failure → status=failed +
                    // frozen_code from the classifier
                    // (`ClassifiedReason::Static`). Idempotent
                    // finish on dispatcher-side panic between
                    // begin and finish leaves a `running` row —
                    // R6/S6 reads it as "interrupted".
                    if let (Some(store), Some(wid), Some(seq)) = (
                        slot_attempt_store.as_ref(),
                        slot_wave_id.as_ref(),
                        current_attempt_seq,
                    ) {
                        let frozen_code: Option<String> =
                            match (&classified.outcome, &classified.reason) {
                                (
                                    ralph_core::supervisor::worker_outcome::SlotOutcome::Failed { .. },
                                    Some(ClassifiedReason::Static(code)),
                                ) => Some((*code).to_string()),
                                _ => None,
                            };
                        let (status, code) = match &classified.outcome {
                            ralph_core::supervisor::worker_outcome::SlotOutcome::Failed { .. } => {
                                (ralph_core::supervisor::AttemptStatus::Failed, frozen_code.clone())
                            }
                            _ => (ralph_core::supervisor::AttemptStatus::Succeeded, None),
                        };
                        let end_cp = match cwd_path.as_ref() {
                            Some(p) => tokio::task::spawn_blocking({
                                let p = p.clone();
                                move || ralph_core::worktree::capture_git_checkpoint(&p)
                            })
                            .await
                            .ok()
                            .flatten(),
                            None => None,
                        };
                        let finished_at_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0);
                        if let Err(err) = store.finish_slot_attempt(
                            wid,
                            slot_index_local,
                            seq,
                            status,
                            end_cp,
                            code.as_deref(),
                            finished_at_ms,
                        ) {
                            tracing::warn!(
                                slot_index = slot_index_local,
                                wave_id = %wid,
                                attempt_seq = seq,
                                attempt,
                                error = %err,
                                "finish_slot_attempt failed; worker outcome unchanged"
                            );
                        }
                    }
                    let mut should_retry = false;
                    // 2026-07-30-001 plan U2: the stable code of the
                    // attempt we are about to abandon, owned so it
                    // outlives the borrow of `outcome`.
                    let mut retry_failure_code: Option<String> = None;
                    if let Some(_guard) = release_guard.as_ref() {
                        use ralph_core::supervisor::worker_outcome::{
                            SlotOutcome, is_retryable_slot_reason,
                        };
                        // KTD8: retry decision uses the FROZEN static
                        // code (the typed reason from
                        // `classify_slot_result`), not the worker's
                        // dynamic Err message. `ClassifiedReason` is
                        // the dispatcher-local helper enum declared
                        // near `classify_slot_result` below; we match
                        // on its `Static` arm to read the frozen code.
                        // `ClassifiedReason::Static(_)` carries a
                        // `&'a str` we dereference into a `&str`.
                        let frozen_code: Option<&str> =
                            match (&classified.outcome, &classified.reason) {
                                (
                                    SlotOutcome::Failed { .. },
                                    Some(ClassifiedReason::Static(code)),
                                ) => Some(*code),
                                _ => None,
                            };
                        if let Some(code) = frozen_code {
                            // C1: drop the `attempt < u32::MAX` dead
                            // guard — `attempt.saturating_add(1)` plus
                            // the `attempt <= budget` check already
                            // bound the loop; the u32::MAX check is
                            // unreachable once `budget <= 2`.
                            // A2: bail out of the retry loop the
                            // moment the wave-level partial /
                            // aggregate deadline has passed, so a
                            // single retryable slot cannot burn
                            // `budget * wave_timeout` of wall time.
                            let now = tokio::time::Instant::now();
                            let deadline_passed = now >= retry_partial_deadline
                                || now >= retry_aggregate_deadline;
                            if !deadline_passed
                                && is_retryable_slot_reason(code)
                                && attempt <= budget
                            {
                                tracing::warn!(
                                    slot_index = request_index,
                                    attempt,
                                    budget,
                                    code = %code,
                                    "U5: retrying slot after frozen-code failure"
                                );
                                should_retry = true;
                                retry_failure_code = Some(code.to_string());
                            } else if deadline_passed {
                                tracing::warn!(
                                    slot_index = request_index,
                                    attempt,
                                    code = %code,
                                    "U5: skipping retry because wave-level partial/aggregate deadline has passed"
                                );
                            }
                        }
                    }
                    if !should_retry {
                        // 2026-09-01-001 plan U1 (R1 / S1.1 / S1.3):
                        // persist the FINAL attempt's accepted event
                        // list BEFORE the channel file is removed.
                        // Final-attempt-only: intermediate attempts
                        // are not persisted (KTD9: only the final
                        // attempt escapes the retry block). Use
                        // `current_attempt_seq` from the
                        // `begin_slot_attempt` receipt so the row
                        // is addressable by recovery (U2). A store
                        // write failure degrades to warn and leaves
                        // the channel file in place — fan-in still
                        // runs from memory so the healthy path is
                        // unaffected.
                        let mut persist_ok = true;
                        if let Ok((events, _, _, _)) = &outcome.1 {
                            if let (Some(store), Some(wid)) = (
                                slot_attempt_store.as_ref(),
                                slot_wave_id.as_ref(),
                            ) {
                                let seq = current_attempt_seq.unwrap_or(1);
                                if current_attempt_seq.is_none() {
                                    tracing::warn!(
                                        wave_id = %wid,
                                        slot_index = slot_index_local,
                                        "U1: missing attempt_seq; persisting with seq=1"
                                    );
                                }
                                if let Err(error) = store.record_slot_event_payloads(
                                    wid,
                                    slot_index_local,
                                    seq,
                                    events,
                                ) {
                                    persist_ok = false;
                                    tracing::warn!(
                                        wave_id = %wid,
                                        slot_index = slot_index_local,
                                        attempt_seq = seq,
                                        error = %error,
                                        "U1: supervisor record_slot_event_payloads failed; \
                                         leaving channel file in place"
                                    );
                                } else if !events.is_empty() {
                                    // S1.3: a no-op/default store impl can
                                    // return Ok without writing. Only
                                    // delete the live channel when the
                                    // payload is actually readable back.
                                    let wrote = store
                                        .load_slot_event_payloads(wid)
                                        .ok()
                                        .is_some_and(|rows| {
                                            rows.iter().any(|(slot, _, ev)| {
                                                *slot == slot_index_local && !ev.is_empty()
                                            })
                                        });
                                    if !wrote {
                                        persist_ok = false;
                                        tracing::warn!(
                                            wave_id = %wid,
                                            slot_index = slot_index_local,
                                            "U1: payload persist reported Ok but store has no \
                                             events for this slot; leaving channel file in place"
                                        );
                                    }
                                }
                            }
                        }
                        // Persist-before-delete: only after the store
                        // write above has had a chance to succeed do
                        // we drop the channel file. The worker.rs
                        // side no longer deletes — the dispatcher
                        // owns the channel lifecycle from worker exit
                        // onward.
                        //
                        // 2026-09-01-001 plan U6 (R6 / S6.1-S6.2):
                        // failed / empty / timeout outcomes move the
                        // channel file into
                        // `.ralph/diagnostics/failed-activations/`
                        // so post-mortem investigation can see what
                        // the worker wrote before the channel was
                        // closed; clean success keeps the delete
                        // path. Quarantine IO failure degrades to
                        // warn + delete so the live channel directory
                        // is always cleaned up (the "channel must
                        // leave the live dir" invariant is preserved
                        // without blocking dispatch).
                        //
                        // S1.3: a store write failure must leave the
                        // live channel path untouched so a crash in
                        // the fan-in window can still recover events
                        // from disk.
                        if !persist_ok {
                            break outcome;
                        }
                        let classified = classify_slot_attempt(&outcome.1, slot_wave_kind);
                        let keep_for_postmortem = matches!(
                            classified.outcome,
                            ralph_core::supervisor::worker_outcome::SlotOutcome::Failed { .. }
                        );
                        if keep_for_postmortem {
                            if let Err(err) = quarantine_worker_channel(
                                &current_request.worker_events_path,
                                slot_index_local,
                                attempt,
                            ) {
                                tracing::warn!(
                                    slot_index = slot_index_local,
                                    path = %current_request.worker_events_path.display(),
                                    error = %err,
                                    "U6: quarantine_worker_channel failed; \
                                     falling back to delete to clear the live channel dir"
                                );
                                let _ = std::fs::remove_file(&current_request.worker_events_path);
                            }
                        } else if persist_ok {
                            if let Err(err) =
                                std::fs::remove_file(&current_request.worker_events_path)
                            {
                                tracing::debug!(
                                    slot_index = slot_index_local,
                                    path = %current_request.worker_events_path.display(),
                                    error = %err,
                                    "U6: failed to remove channel file after persistence (likely already absent)"
                                );
                            }
                        }
                        break outcome;
                    }
                    // A1 / A3: hand intermediate attempts a request
                    // whose `progress_tx` / `worker_rpc_tx` /
                    // `worker_tui_state` are no-op channels. The
                    // FINAL attempt below keeps the real senders
                    // so the dispatcher / TUI / RPC subscribers
                    // observe exactly one Done / Failed notification
                    // per (wave, slot).
                    // 2026-07-30-001 plan U2: record what this attempt
                    // hit before handing the slot to a fresh worker.
                    // Only the first `RETRY_MAX_PRIOR_ATTEMPTS` are
                    // kept so the prompt stays bounded.
                    if let Some(code) = retry_failure_code
                        && prior_attempts.len() < ralph_core::RETRY_MAX_PRIOR_ATTEMPTS
                    {
                        prior_attempts.push(ralph_core::PriorAttempt::new(
                            attempt,
                            code,
                            reported_failure_detail(&outcome.1).as_deref(),
                        ));
                    }
                    attempt = attempt.saturating_add(1);
                    current_request = silent_request(&current_request);
                    // The next attempt runs in the SAME worktree, so it
                    // must be told what is already there and what the
                    // earlier attempts hit.
                    current_request.prompt = format!(
                        "{base_prompt}{}",
                        ralph_core::render_retry_context(&ralph_core::RetryContext {
                            attempt,
                            max_attempts,
                            prior_attempts: prior_attempts.clone(),
                        })
                    );
                    // Loop continues; **only** the final attempt's
                    // outcome escapes (KTD9: do not salvage
                    // intermediate batches).
                };
                // 2026-07-30-001 plan U1 (D15): an Exec worker's final
                // `exec.unit.failed` batch is a *valid* terminal, so
                // `record_outcome` would happily file it as a tracker
                // RESULT — the store would say Failed while the wave
                // said "this slot produced events". Normalize it into
                // the stable failure the store already recorded and
                // drop the business batch, so the failed unit can
                // never activate a downstream consumer.
                let mut last_outcome = last_outcome;
                if let Ok((_events, duration, _success, _pid)) = &last_outcome.1 {
                    let duration = *duration;
                    let is_reported_failure = matches!(
                        classify_slot_attempt(&last_outcome.1, slot_wave_kind).reason,
                        Some(ClassifiedReason::Static(REASON_EXECUTOR_REPORTED_FAILURE))
                    );
                    if is_reported_failure {
                        last_outcome.1 =
                            Err((REASON_EXECUTOR_REPORTED_FAILURE.to_string(), duration));
                    }
                }
                last_outcome
            };

            // 2026-07-23-001 plan U5 (R8): record the terminal slot
            // outcome into the supervisor store at the structured
            // worker-outcome boundary. Success → the batch fingerprint
            // (content_hash + event_count); failure → the worker's
            // error reason. The store's `record_slot_*` is idempotent
            // per `(wave, slot)`, so a re-reported slot does not
            // double-count (the dispatcher does not assume single-call).
            // A record error is logged but does not change the dispatch
            // outcome — the Drop guard below still returns the
            // store-side permit. Cancellation (JoinSet abort) never
            // reaches this point; the Drop guard releases the slot as
            // `Failed` for those. This is persistence only: no sink,
            // no `wave.complete` injection, no `tick` (all U6).
            //
            // 2026-07-23-007 plan U1 (R-W2): classify the worker's
            // exit + accepted event stream through
            // `classify_worker_outcome` BEFORE recording — an
            // exit-0 + zero-events worker is `empty_worker_result`,
            // not a success. `WorkerExit::Cancelled` always wins
            // over a terminal marker that slipped through (R-W4).
            // The Err path from `run_wave_worker` already carries
            // the original error reason; that path bypasses the
            // classifier so the operator-facing message is
            // preserved (the classifier would otherwise map it to
            // `empty_worker_result` because the worker did not
            // emit accepted events before it died).
            // 2026-07-23-007 plan U1 (R-W2) + U5 (M1): classify the
            // worker's exit + accepted event stream through a
            // single helper BEFORE recording. A worker that emits
            // a Done marker and is then cancelled
            // (`WorkerExit::Cancelled`) still maps to
            // `Failed{worker_cancelled}` (R-W4) at the helper
            // boundary. Both the per-slot `record_slot_*` write
            // and the drop-guard `outcome = Completed` arm read
            // the same classification — no duplicated loop, no
            // redundant `result.1.as_ref().unwrap().2`, no dead
            // tuple.
            // 2026-07-30-001 plan U1: the store write and the drop
            // guard read the SAME wave-kind-aware classification the
            // attempt loop used, so an exhausted Exec slot can never
            // be recorded as Completed.
            let classified = classify_slot_attempt(&result.1, slot_wave_kind);
            if let Some(guard) = release_guard.as_ref() {
                use ralph_core::supervisor::worker_outcome::SlotOutcome;
                match (&classified.outcome, &classified.reason) {
                    (SlotOutcome::Completed(_), _) => {
                        // Re-derive the events reference for the
                        // fingerprint; the events live in the Ok
                        // branch of `result.1` and the classifier
                        // helper already iterated them. We re-borrow
                        // here to keep the helper signature
                        // borrow-free of `events`.
                        if let Ok((events, _duration, _success, _pid)) = &result.1 {
                            let (content_hash, event_count) =
                                compute_slot_batch_fingerprint(events);
                            if let Err(error) = guard.bridge.record_slot_result(
                                &guard.wave_id,
                                guard.slot_index,
                                &content_hash,
                                event_count,
                            ) {
                                warn!(
                                    wave_id = %guard.wave_id,
                                    slot_index = guard.slot_index,
                                    %error,
                                    "U5: supervisor record_slot_result failed"
                                );
                            }
                            // 2026-09-01-001 plan U5 (R5 / D6): record
                            // the spawn-time worker pid into
                            // `dispatch_records.pid` so the
                            // operator-facing `ralph diagnose` shows
                            // the real OS-level pid (closes the
                            // 08-29 incident where every dispatch
                            // row had pid=NULL). Best-effort: a
                            // non-PTY backend, a legacy fake executor,
                            // or a store write failure all leave the
                            // pid NULL — the warn log lets operators
                            // notice the gap without blocking dispatch.
                            if let Some(pid) = _pid {
                                if let Err(error) = guard.bridge.record_slot_pid(
                                    &guard.wave_id,
                                    guard.slot_index,
                                    *pid,
                                ) {
                                    warn!(
                                        wave_id = %guard.wave_id,
                                        slot_index = guard.slot_index,
                                        pid = pid,
                                        %error,
                                        "U5: supervisor record_slot_pid failed"
                                    );
                                }
                            }
                            // 2026-07-26-004 plan U2/U3 (KTD3): persist
                            // bounded terminal evidence for the Completed
                            // slot so fan-in reconciliation can prove the
                            // slot produced a real terminal event. Prefer
                            // the `review.unit.done` record; fall back to
                            // the first accepted event for non-review
                            // wave kinds. Failures only warn — the slot is
                            // already Completed and reconciliation treats
                            // missing evidence as fail-closed.
                            if let Some(terminal) = events
                                .iter()
                                .find(|e| e.topic == "review.unit.done")
                                .or_else(|| events.first())
                            {
                                let evidence =
                                    ralph_core::supervisor::TerminalEvidence::from_event(
                                        &terminal.topic,
                                        terminal.payload.as_deref().unwrap_or(""),
                                    );
                                if let Err(error) = guard.bridge.record_slot_terminal_evidence(
                                    &guard.wave_id,
                                    guard.slot_index,
                                    &evidence,
                                ) {
                                    warn!(
                                        wave_id = %guard.wave_id,
                                        slot_index = guard.slot_index,
                                        %error,
                                        "U2: supervisor record_slot_terminal_evidence failed"
                                    );
                                }
                            }
                            // 2026-09-01-001 plan U1 (R1 / S1.1):
                            // payload persistence happens at the
                            // break-outcome boundary inside the
                            // worker retry block above; this
                            // post-retry loop only sees the FINAL
                            // attempt's events (KTD9) so the store
                            // write here would race against the
                            // earlier write. We rely on the retry
                            // block's persist-before-delete
                            // sequencing and skip re-writing here.
                        }
                    }
                    (SlotOutcome::Failed { .. }, Some(reason_str)) => {
                        let reason_str: &str = match reason_str {
                            ClassifiedReason::Static(s) => s,
                            ClassifiedReason::Dynamic(s) => s,
                        };
                        if let Err(error) = guard.bridge.record_slot_failure(
                            &guard.wave_id,
                            guard.slot_index,
                            reason_str,
                        ) {
                            warn!(
                                wave_id = %guard.wave_id,
                                slot_index = guard.slot_index,
                                %error,
                                "U1/007: supervisor record_slot_failure failed"
                            );
                        }
                    }
                    (SlotOutcome::Failed { .. }, None) => {
                        // Unreachable: `classify_slot_result` always
                        // populates `reason` for a `Failed` outcome.
                        // Surface as a warning so a future refactor
                        // cannot silently drop the failure record.
                        warn!(
                            wave_id = %guard.wave_id,
                            slot_index = guard.slot_index,
                            "U5: classified SlotOutcome::Failed without a reason; skipping record_slot_failure"
                        );
                    }
                }
            }

            // U4/007 projection is applied AFTER the classifier has set the
            // drop guard's outcome (Completed vs Failed); see below.

            // U1/007: drop-guard outcome follows the classifier —
            // only an explicitly Completed outcome releases the
            // slot as Completed. Failed / empty / cancel outcomes
            // all release as Failed so a subsequent bridge iteration
            // sees a consistent permit return.
            if let Some(guard) = release_guard.as_mut() {
                use ralph_core::supervisor::worker_outcome::SlotOutcome;
                if matches!(classified.outcome, SlotOutcome::Completed(_)) {
                    guard.outcome = ralph_core::supervisor::DispatchOutcome::Completed;
                }
            }

            // 2026-07-23-007 plan U4 (R-W5): project the slot's
            // terminal status onto `tasks.jsonl`. Done after the
            // classifier above so the projection sees the correct
            // Completed / Failed outcome. Idempotent — repeated
            // projections (re-report + recovery replay) do not
            // duplicate rows.
            let projection_input = release_guard.as_ref().map(|g| {
                (
                    g.bridge.tasks_path().map(|p| p.to_path_buf()),
                    g.outcome,
                    g.wave_id.clone(),
                    g.slot_index,
                )
            });
            if let Some((Some(tasks_path), outcome, wave_id, slot_index)) = projection_input {
                use super::super::task_projection::{SlotProjection, project_slot};
                let projection = match outcome {
                    ralph_core::supervisor::DispatchOutcome::Completed => SlotProjection::Completed,
                    ralph_core::supervisor::DispatchOutcome::Failed => SlotProjection::Failed,
                };
                project_slot(
                    &tasks_path,
                    terminal_loop_id.as_deref().unwrap_or(""),
                    &wave_id,
                    slot_index,
                    projection,
                );
            }
            result
        });
        spawned_count += 1;
    }

    // U2: spawn guarantee — 0-worker silent is forbidden.
    // We spawn one worker per `worker_request` (which may be
    // fewer than `events_len` under the supervisor gate — see
    // 2026-07-23-001 plan U3 — when some slots are skipped
    // because the store did not approve them). The check
    // compares against `worker_request_count` so the
    // guarantee is "the spawn loop ran to completion", not
    // "every event became a worker request".
    if spawned_count < worker_request_count as u32 {
        warn!(
            wave_id = %ctx.wave_id,
            spawned_count,
            expected_count = worker_request_count,
            "wave_spawn_failed: fewer workers spawned than worker_requests"
        );
        return WaveDispatchOutcome::SpawnFailed {
            spawned_count,
            expected_count: worker_request_count as u32,
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
    // 2026-07-23-001 plan U9: sweep over `sweep_indices`
    // (round-scoped for the supervisor's batch dispatch) instead
    // of `0..expected_total`, so a still-pending slot from a
    // larger wave is never mis-classified as failed here — it
    // is dispatched by the next round instead.
    for &i in &ctx.sweep_indices {
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

/// U2: Handle a single rejected wave.
///
/// Emits a structured `plan.blocked` event with the typed reason and
/// records a `RecoveryDiagnosisEnvelope` so the responder can escalate
/// after a stable retry window. **No** worker, TUI update, or backend
/// call is performed — the wave is short-circuited before any of those
/// side-effects.
#[allow(clippy::unused_async)]
pub(crate) async fn handle_wave_rejection(
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

// 2026-07-28-002 plan U4 (R9 / S3 / S6): synthesise a one-slot
// `DetectedWave` from a `SlotDescriptor` previously persisted by
// the dispatcher at spawn time, and dispatch it via the supervisor
// path using the pre-registered child wave id (skipping the
// `register_wave_if_absent` step inside
// `execute_wave_via_supervisor_with_executor`).
//
// `consumer_aggregate_timeout` is left `None`: the dispatcher
// already falls back to the per-worker-timeout formula in that
// case (see `aggregate_timeout_for` above).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_redrive_child_wave(
    bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    executor: Arc<dyn WaveWorkerExecutor>,
    cli_backend: &CliBackend,
    loop_id: &str,
    hat_registry: &ralph_core::HatRegistry,
    child_wave_id: String,
    descriptor: ralph_core::supervisor::SlotDescriptor,
    child_slot_index: u32,
    expected_total: u32,
    main_events_file: &std::path::Path,
) -> WaveDispatchOutcome {
    use ralph_core::Event;

    let target_hat = match hat_registry.find_by_trigger(&descriptor.topic) {
        Some(hat_id) => hat_id.clone(),
        None => {
            tracing::warn!(
                wave_id = %child_wave_id,
                topic = %descriptor.topic,
                "U4 dispatch_redrive_child_wave: no hat registered for descriptor topic; \
                 failing closed (no spawn)"
            );
            return WaveDispatchOutcome::SpawnFailed {
                spawned_count: 0,
                expected_count: expected_total,
            };
        }
    };
    let hat_config = hat_registry
        .get_config(&target_hat)
        .cloned()
        .unwrap_or_default();

    let synthesized = ralph_core::DetectedWave {
        // A redrive child is dispatched one slot at a time, but all of
        // those dispatches share the same persisted child wave. Channel
        // registration is keyed by the public wave id, so give each
        // synthetic slot dispatch its own channel identity while keeping
        // `pre_registered_id` below anchored to the real store wave.
        wave_id: format!("{child_wave_id}-slot-{child_slot_index}"),
        target_hat,
        hat_config,
        events: vec![Event {
            topic: descriptor.topic.clone(),
            payload: Some(descriptor.payload_json.clone()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: Some(child_wave_id.clone()),
            // 2026-07-28-002 plan G5 / R-F7: the synthesized event
            // carries the child slot position and wave total so the
            // worker prompt / context see the true redrive coordinates
            // instead of empty fields.
            wave_index: Some(child_slot_index),
            wave_total: Some(expected_total),
            system_injected: None,
        }],
        total: expected_total,
        partial: false,
        consumer_aggregate_timeout: None,
    };

    execute_wave_via_supervisor_with_executor(
        &synthesized,
        cli_backend,
        main_events_file,
        false,
        false,
        None,
        None,
        loop_id,
        WaveDispatchLimits::default(),
        None,
        None,
        &bridge,
        executor,
        Some(&child_wave_id),
        // 2026-07-28-002 plan U3/U4 (C3): the synthesized wave carries
        // exactly one event, but its store slot is the child slot index
        // (which differs from the events-array position 0 for multi-slot
        // child waves).
        Some(child_slot_index),
    )
    .await
}

// 2026-07-28-002 plan U4 (R7 / S3 / S4 / S5 / S6): at boot, scan
// the supervisor store for pending redrive child waves, take each
// slot's descriptor (fail-closed on `DescriptorUnavailable` /
// `DescriptorConflict`), and dispatch each dispatchable slot as a
// one-shot wave. The dispatcher code that handles regular waves
// already covers the per-slot binding, slot result, and worker
// spawn; this routine just orchestrates the scan + take + per-slot
// dispatch using `dispatch_redrive_child_wave` above.
//
// Returns the number of slots that actually produced a worker /
// terminal dispatch outcome. Wired in
// `runner.rs::run_loop_impl_inner` (both supervisor boot seams),
// gated on `--resume` via `boot_dispatch_pending_redrive_if_resuming`,
// after `recover_active_waves_at_startup`
// and backend construction — so an operator-driven `ralph run
// --resume` catches redrive children that the previous loop did
// not finish spawning. A fresh boot never calls this.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_pending_redrive_waves(
    store: &std::sync::Arc<dyn ralph_core::supervisor::SupervisorStore>,
    loop_id: &str,
    hat_registry: &ralph_core::HatRegistry,
    cli_backend: &CliBackend,
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    main_events_file: &std::path::Path,
    worker_executor: std::sync::Arc<dyn WaveWorkerExecutor>,
) -> usize {
    use ralph_core::supervisor::RedriveTakeOutcome;

    let pending = match store.list_redrive_pending_child_waves() {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "U4 dispatch_pending_redrive_waves: list_redrive_pending_child_waves failed; \
                 skipping (no spawn)"
            );
            return 0;
        }
    };

    let mut dispatched = 0usize;
    for child in pending {
        for slot in &child.slots {
            // S4: expected_digest = None means the parent slot never
            // started (no persisted descriptor — pre-U4 legacy row).
            // Fail-closed: skip without calling take.
            let expected_digest = match slot.expected_digest.as_deref() {
                Some(d) => d.to_string(),
                None => {
                    tracing::warn!(
                        child_wave_id = %child.child_wave_id,
                        child_slot_index = slot.child_slot_index,
                        parent_wave_id = %child.parent_wave_id,
                        parent_slot_index = slot.parent_slot_index,
                        "U4 dispatch_pending_redrive_waves: slot has no persisted descriptor; \
                         skipping (slot_never_started fail-close)"
                    );
                    // A3 / R-F1: mark the slot failed on the bridge so
                    // the store carries a structured `slot_never_started`
                    // reason instead of leaving a silent Pending row.
                    let _ = bridge.record_slot_failure(
                        &child.child_wave_id,
                        slot.child_slot_index,
                        "redrive_slot_never_started: parent slot had no persisted descriptor",
                    );
                    continue;
                }
            };

            let outcome = match store.take_dispatchable_redrive_descriptor(
                &child.child_wave_id,
                slot.child_slot_index,
                &expected_digest,
            ) {
                Ok(o) => o,
                Err(err) => {
                    tracing::warn!(
                        child_wave_id = %child.child_wave_id,
                        child_slot_index = slot.child_slot_index,
                        error = %err,
                        "U4 dispatch_pending_redrive_waves: take failed; skipping (fail-close)"
                    );
                    continue;
                }
            };

            match outcome {
                RedriveTakeOutcome::Dispatchable { descriptor } => {
                    // 2026-07-28-002 plan R9: wave_total / DetectedWave.total
                    // must be the child wave's store expected_total, not 1.
                    // Each iteration still synthesizes one event for one
                    // Pending child slot; the coordinates carry the true
                    // multi-slot total so worker prompt/env see the real
                    // wave shape.
                    let expected_total = child.expected_total.max(1);
                    let dispatch_outcome = dispatch_redrive_child_wave(
                        bridge.clone(),
                        worker_executor.clone(),
                        cli_backend,
                        loop_id,
                        hat_registry,
                        child.child_wave_id.clone(),
                        descriptor,
                        slot.child_slot_index,
                        expected_total,
                        main_events_file,
                    )
                    .await;
                    // Only count slots that actually produced a worker
                    // (or reached a terminal dispatch outcome). Spawn /
                    // preparation failures must not inflate the boot
                    // scan counter.
                    match dispatch_outcome {
                        WaveDispatchOutcome::Completed(_)
                        | WaveDispatchOutcome::Partial(_)
                        | WaveDispatchOutcome::AggregateDeadlineExceeded(_) => {
                            dispatched += 1;
                        }
                        WaveDispatchOutcome::SpawnFailed { spawned_count, .. }
                            if spawned_count > 0 =>
                        {
                            dispatched += 1;
                        }
                        WaveDispatchOutcome::SpawnFailed { .. }
                        | WaveDispatchOutcome::PreparationFailed { .. }
                        | WaveDispatchOutcome::GlobalDeadlineExceeded => {}
                    }
                }
                RedriveTakeOutcome::DescriptorUnavailable => {
                    tracing::warn!(
                        child_wave_id = %child.child_wave_id,
                        child_slot_index = slot.child_slot_index,
                        "U4 dispatch_pending_redrive_waves: descriptor unavailable; \
                         skipping (fail-close)"
                    );
                    let _ = bridge.record_slot_failure(
                        &child.child_wave_id,
                        slot.child_slot_index,
                        "redrive_slot_never_started: descriptor unavailable at boot",
                    );
                }
                RedriveTakeOutcome::DescriptorConflict => {
                    tracing::warn!(
                        child_wave_id = %child.child_wave_id,
                        child_slot_index = slot.child_slot_index,
                        "U4 dispatch_pending_redrive_waves: descriptor digest conflict; \
                         skipping (fail-close)"
                    );
                    let _ = bridge.record_slot_failure(
                        &child.child_wave_id,
                        slot.child_slot_index,
                        "redrive_descriptor_conflict: expected digest mismatch (fail-closed)",
                    );
                }
            }
        }
    }
    dispatched
}

/// Runner boot seam for redrive: only `--resume` consumes pending
/// child waves (plan U4 S6 / fresh-boot exclusion). Extracted so
/// tests can pin the resume gate without driving the full loop.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn boot_dispatch_pending_redrive_if_resuming(
    resume: bool,
    store: &std::sync::Arc<dyn ralph_core::supervisor::SupervisorStore>,
    loop_id: &str,
    hat_registry: &ralph_core::HatRegistry,
    cli_backend: &CliBackend,
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    main_events_file: &std::path::Path,
    worker_executor: std::sync::Arc<dyn WaveWorkerExecutor>,
) -> usize {
    if !resume {
        return 0;
    }
    dispatch_pending_redrive_waves(
        store,
        loop_id,
        hat_registry,
        cli_backend,
        bridge,
        main_events_file,
        worker_executor,
    )
    .await
}

/// 2026-09-01-001 plan U6 (R6 / S6.1): move a failed slot's
/// channel file into the post-mortem quarantine directory so
/// operators can inspect what the worker wrote before the
/// channel was closed. The quarantine directory lives under
/// the channel file's grandparent `.ralph/diagnostics/failed-
/// activations/` and the destination filename encodes the
/// slot index + attempt sequence so multiple failures on the
/// same slot do not collide.
///
/// `worker_events_path` is the per-slot channel file
/// (`.ralph/wave-<wave_id>-<slot>.jsonl`); its parent is the
/// `.ralph/` directory used as the quarantine root.
fn quarantine_worker_channel(
    worker_events_path: &std::path::Path,
    slot_index: u32,
    attempt: u32,
) -> std::io::Result<std::path::PathBuf> {
    let ralph_dir = worker_events_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "worker events path has no parent: {}",
                    worker_events_path.display()
                ),
            )
        })?;
    let quarantine_dir = ralph_dir.join("diagnostics").join("failed-activations");
    std::fs::create_dir_all(&quarantine_dir)?;
    let file_name = worker_events_path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "worker events path has no filename: {}",
                worker_events_path.display()
            ),
        )
    })?;
    let suffix = std::process::id();
    let destination = quarantine_dir.join(format!(
        "{}-slot{}-attempt{}-pid{}",
        file_name.to_string_lossy(),
        slot_index,
        attempt,
        suffix,
    ));
    std::fs::rename(worker_events_path, &destination)?;
    Ok(destination)
}

#[cfg(test)]
mod quarantine_tests {
    use super::*;

    fn fake_worker_events_file(
        dir: &std::path::Path,
        wave_id: &str,
        slot_index: u32,
    ) -> std::path::PathBuf {
        let path = dir.join(format!("wave-{wave_id}-{slot_index}.jsonl"));
        std::fs::write(&path, "fake events\n").expect("seed channel file");
        path
    }

    #[test]
    fn quarantine_moves_failed_channel_under_diagnostics() {
        let workspace = tempfile::TempDir::new().expect("tempdir");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let channel = fake_worker_events_file(&ralph_dir, "u6-quarantine", 0);

        let destination =
            quarantine_worker_channel(&channel, 0, 1).expect("quarantine must succeed");

        assert!(
            !channel.exists(),
            "U6/S6.1: source path must not exist after rename"
        );
        assert!(
            destination.exists(),
            "U6/S6.1: destination must hold the moved file"
        );
        assert!(
            destination.starts_with(ralph_dir.join("diagnostics").join("failed-activations")),
            "U6/S6.1: quarantine destination must live under .ralph/diagnostics/failed-activations; \
             got {}",
            destination.display()
        );
        assert!(
            destination.to_string_lossy().contains("slot0"),
            "U6/S6.1: quarantine name must encode slot index for post-mortem navigation"
        );
    }
}
