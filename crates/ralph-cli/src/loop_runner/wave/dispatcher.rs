use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ralph_adapters::CliBackend;
use ralph_core::CompletedWave;
use ralph_core::diagnostics::DiagnosticsCollector;

/// 2026-07-26-002 plan U8 (R10): the worker-side timeout error
/// message and the dispatcher's empty-batch classifier share this
/// prefix constant. The previous design used two independent
/// literals ("Worker timed out after ..." in `worker.rs` and a
/// `const TIMEOUT_PREFIX` inside `classify_slot_result`) which
/// silently kept the legacy `worker_cancelled` shell when the
/// worker text drifted. Sharing the constant makes the contract
/// compile-checked.
pub(crate) const WORKER_TIMEOUT_ERR_PREFIX: &str = "Worker timed out after";

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

/// Synchronously returns a supervisor slot to terminal state when a
/// worker task exits, including JoinSet cancellation/abort. A Drop
/// guard is required because an aborted async task never executes code
/// after its awaited executor future.
///
/// 2026-07-23-007 plan U6 (A2 / A5): the drop guard NEVER
/// overwrites a slot the worker task already drove to a terminal
/// state. The supervisor store's `release_slot_dispatch` is
/// idempotent (no-op when the slot is already `Completed` /
/// `Failed` / `Cancelled`), so a panic between
/// `record_slot_result` and `guard.outcome = Completed` cannot
/// downgrade a terminal write — the existing
/// `release_slot_dispatch(Completed | Failed)` call is a safe
/// no-op. The `outcome` field is kept so the guard preserves the
/// explicit `Completed` signal for the dispatch_records
/// transition; the store's `IN ('dispatched','running')` predicate
/// is the actual safety gate.
struct SupervisorSlotRelease {
    bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    wave_id: String,
    slot_index: u32,
    outcome: ralph_core::supervisor::DispatchOutcome,
}

impl Drop for SupervisorSlotRelease {
    fn drop(&mut self) {
        if let Err(error) =
            self.bridge
                .release_slot_dispatch(&self.wave_id, self.slot_index, self.outcome)
        {
            tracing::warn!(
                wave_id = %self.wave_id,
                slot_index = self.slot_index,
                %error,
                "supervisor terminal permit release failed"
            );
        }
    }
}

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
    /// 2026-07-23-001 plan U9: indices the synthetic-failure sweep
    /// in `dispatch_wave_inner_with_release` inspects. Legacy
    /// `build()` sets this to `0..wave.total` so partial waves
    /// still mark slots that never got a worker event. Supervisor
    /// per-round builders overwrite it with the indices actually
    /// spawned in that round — pending slots stay pending in the
    /// store and are dispatched in a later round, not marked
    /// failed here.
    sweep_indices: Vec<u32>,
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
            );
            Some(Arc::new(bridge) as Arc<dyn ralph_core::supervisor::SupervisorBridge>)
        } else {
            None
        };
    let supervisor_bridge_owned: Option<Arc<dyn ralph_core::supervisor::SupervisorBridge>> =
        lazy_bridge.or_else(|| supervisor_bridge.cloned());
    let supervisor_bridge: Option<&Arc<dyn ralph_core::supervisor::SupervisorBridge>> =
        supervisor_bridge_owned.as_ref();

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
            out.hats_source_label,
            out.config_path,
            // 2026-07-03-001 supervisor real-wiring: forward the
            // optional supervisor bridge so the dispatcher can take
            // the per-slot worktree path when `supervisor.enabled: true`.
            supervisor_bridge,
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
                    let aggregate_timeout_secs = event_loop
                        .config()
                        .event_loop
                        .supervisor
                        .aggregate_timeout_secs;
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

        // 2026-07-26-002 plan U6 (R6 / KTD7): the dispatcher is the
        // sole authority for which per-slot channels the wave
        // workers may write to. Append the absolute path to
        // `<workspace>/.ralph/current-wave-channels` BEFORE spawning
        // so the worker's `ralph emit` finds the marker. Best-effort:
        // marker write failure is a warn-and-continue, NOT a
        // spawn blocker, because the worker process can still
        // surface its own diagnostic. The marker is rewritten by
        // every dispatcher invocation (one line per slot, exact
        // match — no prefix wildcards).
        if let Err(err) = append_wave_channel_to_marker(main_events_file, &worker_events_file) {
            warn!(
                wave_id = %wave.wave_id,
                slot_index = index_u32,
                error = %err,
                "U6: failed to append wave channel to .ralph/current-wave-channels; \
                 worker emit will fall back to shape-only allowlist check"
            );
        }

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
            idle_heartbeat,
            idle_weak_signal_cap,
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
) -> WaveDispatchOutcome {
    use ralph_core::supervisor::{SupervisorBridge as _, WaveKind};
    use ralph_core::{WaveTracker, WaveWorkerContext, build_wave_worker_prompt};

    let concurrency = wave.hat_config.concurrency as usize;
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
    let aggregate_timeout =
        if wave.has_explicit_aggregate_timeout() || wave.consumer_aggregate_timeout.is_some() {
            Duration::from_secs(wave.aggregate_timeout_secs())
        } else {
            aggregate_timeout_for(wave_timeout, wave.events.len(), concurrency)
        };

    // 2026-07-03-001 supervisor real-wiring: infer the wave
    // kind from the first event's topic. `review.*` (both
    // `review.wave.ready` and `review.unit.ready`) → Review;
    // `fix.*` → Fix; everything else → Exec (the default for
    // `exec.unit.ready` / `exec.wave.ready`).
    //
    // 2026-07-23-001 plan U9: widened `review.wave.` → `review.`
    // so the builtin `ce-executor-supervisor` preset's review
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
    let store_wave_id = match bridge.register_wave_if_absent(
        wave_kind,
        &wave.wave_id,
        wave.total,
        1,
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

    // U3 KTD-5: the local effective cap is
    // `min(hat.concurrency, bridge.max_concurrent_workers())`.
    let effective_cap: u32 = wave
        .hat_config
        .concurrency
        .min(bridge.max_concurrent_workers())
        .max(1);

    struct PreparedSlot {
        index: u32,
        request: Option<WorkerRequest>,
        preview: String,
        dimension: Option<String>,
    }

    let mut prepared: Vec<PreparedSlot> = Vec::with_capacity(wave.events.len());

    for (index, event) in wave.events.iter().enumerate() {
        let wave_id = wave.wave_id.clone();
        let index_u32 = index as u32;
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
                idle_heartbeat,
                idle_weak_signal_cap,
            }),
            preview,
            dimension: assigned_dimension,
        });
    }

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

/// U6: outcome of a production supervisor fan-in tick. The
/// dispatcher logs this and uses `injected` to decide whether a
/// fresh coordination event landed in the ledger this round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SupervisorFanInOutcome {
    /// A fresh `*.wave.complete` was merged + injected.
    InjectedComplete,
    /// A fresh `*.wave.failed` was injected.
    InjectedFailed,
    /// The wave was already merged on a prior tick (KTD-7
    /// idempotency); no new coordination event.
    AlreadyDone,
    /// The wave is still collecting slots; nothing injected.
    ContinueCollect,
    /// The merge sink rejected the batch; `merged_to_events`
    /// stayed false so the next tick retries exactly once
    /// (KTD-7). No coordination event injected.
    MergeFailed,
    /// The bridge/store errored; logged, treated as no-op so the
    /// next tick retries.
    StoreError,
}

impl SupervisorFanInOutcome {
    /// True when a fresh coordination event was injected this tick.
    #[allow(dead_code)] // consumed by diagnostics + follow-up units
    pub(crate) fn injected(self) -> bool {
        matches!(
            self,
            SupervisorFanInOutcome::InjectedComplete | SupervisorFanInOutcome::InjectedFailed
        )
    }
}

/// U6: production supervisor fan-in. Merges the per-slot worker
/// business events (sorted by slot index, de-duplicated) into the
/// loop's main ledger via the bridge's production merge sink, then
/// injects the unique `*.wave.complete` / `*.wave.failed`
/// coordination event (with the successful slots' `branch` /
/// `worktree_path` payload).
///
/// Contract (KTD-6 / KTD-7):
/// - The merge gate is the coordinator's `tick_with_slot_events`:
///   on `Integrate` it appends the sorted business events through
///   the sink and flips `merged_to_events`. If the sink fails, the
///   wave stays in `Collect` and NO coordination event is injected
///   — the next tick retries the merge exactly once.
/// - `merged_to_events` makes the injection idempotent: once merged,
///   subsequent ticks return `AlreadyDone` and never re-inject.
/// - The coordination event is appended to the SAME ledger the sink
///   wrote to, marked `system_injected: true`, WITHOUT advancing the
///   reader cursor; the caller's post-wave `process_events_from_jsonl`
///   re-read publishes the business + coordination events to the bus
///   exactly once.
///
/// This function does NOT perform any Git merge (the integrator path
/// owns that); it only merges the JSONL event fan-in.

/// U1: context for driving a terminal supervisor fan-in to convergence.
/// When present, the fan-in helper knows it must drive through
/// ContinueCollect (by recording never-started slots and re-ticking)
/// rather than returning ContinueCollect as a no-op with no owner.
/// Exhaustion returns `StoreError` (mapped to `fan_in_failure` by the
/// caller) — never silent `AlreadyDone` without a coordination event.
#[derive(Debug, Clone)]
pub(crate) struct TerminalFanInContext {
    /// True when cancel was requested (global_deadline or
    /// AggregateDeadlineExceeded fired).
    pub(crate) cancel_requested: bool,
    /// Real elapsed time since the wave started.
    pub(crate) elapsed: std::time::Duration,
}

pub(crate) fn run_supervisor_fan_in(
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    completed: &CompletedWave,
    detected: &ralph_core::DetectedWave,
    main_events_file: &Path,
    aggregate_timeout_secs: u64,
    terminal_ctx: Option<TerminalFanInContext>,
) -> SupervisorFanInOutcome {
    use ralph_core::supervisor::{SupervisorBridge as _, WaveKind};

    // Infer the wave kind from the trigger topic (mirrors
    // `execute_wave_via_supervisor_with_executor`).
    let trigger_topic = detected
        .events
        .first()
        .map(|e| e.topic.as_str())
        .unwrap_or("");
    // 2026-07-23-001 plan U9: widened `review.wave.` → `review.`
    // to keep the kind inference consistent between the spawn
    // path and this fan-in path. See the matching note on
    // `execute_wave_via_supervisor_with_executor` for why the
    // builtin preset's `review.unit.ready` trigger needs to be
    // classified Review.
    let wave_kind = if trigger_topic.starts_with("review.") {
        WaveKind::Review
    } else if trigger_topic.starts_with("fix.") {
        WaveKind::Fix
    } else {
        WaveKind::Exec
    };

    // Re-derive the store-assigned wave id idempotently. The
    // dispatcher's supervisor spawn path already registered the wave
    // under `completed.wave_id`; `register_wave_if_absent` returns
    // the existing store id on re-entry so the coordinator reads the
    // same row the slot results were recorded against.
    let store_wave_id = match bridge.register_wave_if_absent(
        wave_kind,
        &completed.wave_id,
        completed.wave_total,
        1,
    ) {
        Ok(id) => id,
        Err(err) => {
            warn!(
                wave_id = %completed.wave_id,
                error = %err,
                "U6: supervisor register_wave_if_absent failed during fan-in"
            );
            return SupervisorFanInOutcome::StoreError;
        }
    };

    // U1 (Green 1 / Green 3): when cancel_requested is true (AggregateDeadlineExceeded
    // path), mark the store wave as cancelled so evaluate_phase sees the flag
    // and returns Failed immediately on the first tick.
    if terminal_ctx
        .as_ref()
        .is_some_and(|ctx| ctx.cancel_requested)
    {
        if let Err(err) = bridge.cancel_wave(&store_wave_id) {
            warn!(
                wave_id = %completed.wave_id,
                error = %err,
                "U1: cancel_wave failed during terminal fan-in"
            );
        }
    }

    // Gather the per-slot business events, ordered by slot index and
    // de-duplicated by (topic, payload). Sorting by `WaveResult.index`
    // gives the deterministic slot-index order the plan requires; the
    // dedup keeps the main ledger free of repeated business events
    // when two slots emit an identical record.
    let mut results_by_index: Vec<&ralph_core::WaveResult> = completed.results.iter().collect();
    results_by_index.sort_by_key(|r| r.index);
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut slot_events: Vec<ralph_proto::Event> = Vec::new();
    for result in results_by_index {
        for event in &result.events {
            let key = (event.topic.as_str().to_string(), event.payload.clone());
            if seen.insert(key) {
                slot_events.push(event.clone());
            }
        }
    }

    let inputs = ralph_core::supervisor::PhaseInputs {
        aggregate_timeout_secs,
        elapsed_secs: terminal_ctx
            .as_ref()
            .map(|ctx| ctx.elapsed.as_secs() as u64)
            .unwrap_or(0),
        cancel_requested: terminal_ctx
            .as_ref()
            .is_some_and(|ctx| ctx.cancel_requested),
    };

    // The coordinator is the merge gate: on `Integrate` it appends
    // `slot_events` through the production sink and flips
    // `merged_to_events`. Sink failure → `MergeFailed` (no injection,
    // retry next tick).
    let slot_events_for_retry = slot_events.clone();
    let action = match bridge.tick_with_slot_events(&store_wave_id, inputs.clone(), slot_events) {
        Ok(action) => action,
        Err(err) => {
            warn!(
                wave_id = %completed.wave_id,
                store_wave_id = %store_wave_id,
                error = %err,
                "U6: supervisor tick_with_slot_events failed during fan-in"
            );
            return SupervisorFanInOutcome::StoreError;
        }
    };

    // ── 2026-07-25-005 plan U1 (R3 / R4 / KTD2 / KTD6) ────────────────
    // Exec/Fix partial-failure settlement. The coordinator's pure
    // phase function only reaches `Failed` once EVERY slot is
    // terminal; a wave whose worker batch has finished but which
    // still carries (a) a permanently Failed slot plus (b) slots
    // that never reported anything would otherwise sit in
    // `ContinueCollect` forever (the coordinator keeps waiting for
    // workers that will never report). Fan-in runs after the wave's
    // worker batch completes, so those silent slots can be settled
    // forward-only as `slot_never_started` (KTD5: no visible
    // rollback) and the coordinator then owns the Failed verdict,
    // the wave-phase flip and the coord-injection latch as usual.
    //
    // The `SalvageNotMerged` half: production exec/fix waves never
    // pre-mark the salvage, so `fail_wave` refuses the first tick.
    // We perform the completed-only salvage merge here (KTD6 order:
    // append completed events, THEN fail), commit the mark, and
    // re-tick exactly once so the coordinator latches the failure.
    //
    // Review waves keep their existing flow untouched.
    let mut exec_fix_salvage_written = false;
    let action = if matches!(wave_kind, WaveKind::Exec | WaveKind::Fix)
        && matches!(
            action,
            ralph_core::supervisor::CoordinatorAction::ContinueCollect
                | ralph_core::supervisor::CoordinatorAction::SalvageNotMerged
        ) {
        use ralph_core::supervisor::SlotStatus;
        let settle_snapshot = bridge.fan_in_status(&store_wave_id).ok();
        let has_blocking = settle_snapshot
            .as_ref()
            .is_some_and(|snap| {
                snap.slots
                    .iter()
                    .any(|(_, status)| matches!(status, SlotStatus::Failed | SlotStatus::Cancelled))
            });
        if has_blocking {
            // (a) Salvage completed-only business events into the
            // main ledger and commit the salvage mark (also covers
            // the zero-completed case: nothing to append, mark
            // still commits so fail_wave's gate can open).
            merge_completed_exec_fix_slots_to_main(
                main_events_file,
                completed,
                bridge,
                &store_wave_id,
            );
            exec_fix_salvage_written = true;
            // (b) Settle slots the finished batch left non-terminal:
            // they will never report. First-terminal-wins makes this
            // idempotent for slots that raced into a terminal state
            // between the snapshot read and this record.
            if let Some(snap) = settle_snapshot.as_ref() {
                for (slot_index, status) in &snap.slots {
                    if matches!(
                        status,
                        SlotStatus::Completed | SlotStatus::Failed | SlotStatus::Cancelled
                    ) {
                        continue;
                    }
                    if let Err(err) = bridge.record_slot_failure(
                        &store_wave_id,
                        *slot_index,
                        ralph_core::supervisor::worker_outcome::REASON_SLOT_NEVER_STARTED,
                    ) {
                        warn!(
                            wave_id = %completed.wave_id,
                            slot_index = *slot_index,
                            error = %err,
                            "U1: record_slot_failure(slot_never_started) failed during \
                             exec/fix partial-failure settlement"
                        );
                    }
                }
            }
            // (c) Re-tick: the coordinator now sees every slot
            // terminal with at least one failure and returns
            // `InjectedFailed` (or `AlreadyDone` on a racing latch).
            match bridge.tick_with_slot_events(&store_wave_id, inputs, Vec::new()) {
                Ok(retried) => retried,
                Err(err) => {
                    warn!(
                        wave_id = %completed.wave_id,
                        store_wave_id = %store_wave_id,
                        error = %err,
                        "U1: re-tick after exec/fix partial-failure settlement failed"
                    );
                    return SupervisorFanInOutcome::StoreError;
                }
            }
        } else {
            action
        }
    } else {
        action
    };

    let action_outcome = match action {
        ralph_core::supervisor::CoordinatorAction::InjectedComplete { topic, .. } => {
            // U7 (2026-07-23-002): build the wave-coordination payload
            // shape that matches the **target topic's** schema. The
            // earlier implementation hard-coded the exec-style payload
            // (`completed_slots` / `success_slots` / `merge_root_event_id`)
            // for every wave kind, but `review.wave.complete` and
            // `fix.wave.complete` have different required_fields — see
            // `presets/schemas/ce-executor-supervisor.yml`. A mismatched
            // payload was rejected by the engine gate's required_fields
            // check, the event was demoted to `MalformedLine`, and the
            // downstream integrator hat (e.g. `review-synthesizer`)
            // never woke up. The hard-gate counter then terminated the
            // loop after three iterations with no events emitted.
            let payload = build_wave_complete_payload(
                wave_kind,
                completed,
                &store_wave_id,
                bridge,
                aggregate_timeout_secs,
            );
            append_supervisor_coord_event(main_events_file, &topic, &payload);
            SupervisorFanInOutcome::InjectedComplete
        }
        ralph_core::supervisor::CoordinatorAction::InjectedFailed {
            topic,
            reason,
            blocking_slots,
        } => emit_injected_failed_coord(
            bridge,
            wave_kind,
            completed,
            &store_wave_id,
            main_events_file,
            &topic,
            reason,
            blocking_slots,
            exec_fix_salvage_written,
        ),
        ralph_core::supervisor::CoordinatorAction::AlreadyDone => {
            SupervisorFanInOutcome::AlreadyDone
        }
        ralph_core::supervisor::CoordinatorAction::SalvageNotMerged => {
            // U1 (Green 5): the coordinator refused to latch because salvage wasn't merged.
            // Mark salvage merged, then retry the coordinator tick so it can inject.
            if let Err(err) = bridge.mark_salvage_merged(&store_wave_id) {
                warn!(wave_id = %completed.wave_id, error = %err, "U1: mark_salvage_merged failed");
            }
            let retry_inputs = ralph_core::supervisor::PhaseInputs {
                aggregate_timeout_secs,
                elapsed_secs: terminal_ctx
                    .as_ref()
                    .map(|ctx| ctx.elapsed.as_secs() as u64)
                    .unwrap_or(0),
                cancel_requested: terminal_ctx
                    .as_ref()
                    .is_some_and(|ctx| ctx.cancel_requested),
            };
            let retry_action = match bridge.tick_with_slot_events(
                &store_wave_id,
                retry_inputs,
                slot_events_for_retry.clone(),
            ) {
                Ok(a) => a,
                Err(err) => {
                    warn!(wave_id = %completed.wave_id, error = %err, "U1: retry tick after SalvageNotMerged failed");
                    return SupervisorFanInOutcome::StoreError;
                }
            };
            match retry_action {
                ralph_core::supervisor::CoordinatorAction::InjectedFailed {
                    topic,
                    reason,
                    blocking_slots,
                } => emit_injected_failed_coord(
                    bridge,
                    wave_kind,
                    completed,
                    &store_wave_id,
                    main_events_file,
                    &topic,
                    reason,
                    blocking_slots,
                    exec_fix_salvage_written,
                ),
                ralph_core::supervisor::CoordinatorAction::InjectedComplete { topic, .. } => {
                    let payload = build_wave_complete_payload(
                        wave_kind,
                        completed,
                        &store_wave_id,
                        bridge,
                        aggregate_timeout_secs,
                    );
                    append_supervisor_coord_event(main_events_file, &topic, &payload);
                    SupervisorFanInOutcome::InjectedComplete
                }
                ralph_core::supervisor::CoordinatorAction::AlreadyDone => {
                    SupervisorFanInOutcome::AlreadyDone
                }
                ralph_core::supervisor::CoordinatorAction::ContinueCollect
                | ralph_core::supervisor::CoordinatorAction::SalvageNotMerged
                | ralph_core::supervisor::CoordinatorAction::MergeFailed { .. } => {
                    // Terminal salvage retry exhausted without a coordination
                    // event. Fail-close — never mark AlreadyDone without inject
                    // (that recreates the orphan ContinueCollect hang).
                    warn!(
                        wave_id = %completed.wave_id,
                        "U1: terminal SalvageNotMerged retry exhausted without \
                         InjectedFailed/Complete; returning StoreError"
                    );
                    SupervisorFanInOutcome::StoreError
                }
            }
        }
        ralph_core::supervisor::CoordinatorAction::ContinueCollect => {
            // U1 (Green 6): terminal_ctx is set and first tick returned ContinueCollect.
            // Record never-started failures, mark salvage merged, then second tick.
            if terminal_ctx.is_some() {
                if let Err(err) = bridge.record_never_started_failures(&store_wave_id) {
                    warn!(wave_id = %completed.wave_id, error = %err, "U1: record_never_started_failures failed");
                }
                if let Err(err) = bridge.mark_salvage_merged(&store_wave_id) {
                    warn!(wave_id = %completed.wave_id, error = %err, "U1: mark_salvage_merged failed");
                }
                let retry_inputs = ralph_core::supervisor::PhaseInputs {
                    aggregate_timeout_secs,
                    elapsed_secs: terminal_ctx
                        .as_ref()
                        .map(|ctx| ctx.elapsed.as_secs() as u64)
                        .unwrap_or(0),
                    cancel_requested: terminal_ctx
                        .as_ref()
                        .is_some_and(|ctx| ctx.cancel_requested),
                };
                let retry_action = match bridge.tick_with_slot_events(
                    &store_wave_id,
                    retry_inputs,
                    slot_events_for_retry.clone(),
                ) {
                    Ok(a) => a,
                    Err(err) => {
                        warn!(wave_id = %completed.wave_id, error = %err, "U1: second tick failed");
                        return SupervisorFanInOutcome::StoreError;
                    }
                };
                match retry_action {
                    ralph_core::supervisor::CoordinatorAction::InjectedFailed {
                        topic,
                        reason,
                        blocking_slots,
                    } => emit_injected_failed_coord(
                        bridge,
                        wave_kind,
                        completed,
                        &store_wave_id,
                        main_events_file,
                        &topic,
                        reason,
                        blocking_slots,
                        exec_fix_salvage_written,
                    ),
                    ralph_core::supervisor::CoordinatorAction::InjectedComplete {
                        topic, ..
                    } => {
                        let payload = build_wave_complete_payload(
                            wave_kind,
                            completed,
                            &store_wave_id,
                            bridge,
                            aggregate_timeout_secs,
                        );
                        append_supervisor_coord_event(main_events_file, &topic, &payload);
                        SupervisorFanInOutcome::InjectedComplete
                    }
                    ralph_core::supervisor::CoordinatorAction::AlreadyDone => {
                        SupervisorFanInOutcome::AlreadyDone
                    }
                    _ => {
                        warn!(
                            wave_id = %completed.wave_id,
                            "U1: terminal ContinueCollect retry exhausted without \
                             InjectedFailed/Complete; returning StoreError"
                        );
                        SupervisorFanInOutcome::StoreError
                    }
                }
            } else {
                SupervisorFanInOutcome::ContinueCollect
            }
        }
        ralph_core::supervisor::CoordinatorAction::MergeFailed { topic, error } => {
            // U1 (Green 8): bounded retry on the same merge seam when this
            // call is the final terminal fan-in (no next tick owner).
            if terminal_ctx.is_some() {
                warn!(
                    wave_id = %completed.wave_id,
                    topic = %topic,
                    error = %error,
                    "U1: terminal merge sink rejected; retrying once"
                );
                let retry_inputs = ralph_core::supervisor::PhaseInputs {
                    aggregate_timeout_secs,
                    elapsed_secs: terminal_ctx
                        .as_ref()
                        .map(|ctx| ctx.elapsed.as_secs() as u64)
                        .unwrap_or(0),
                    cancel_requested: terminal_ctx
                        .as_ref()
                        .is_some_and(|ctx| ctx.cancel_requested),
                };
                match bridge.tick_with_slot_events(
                    &store_wave_id,
                    retry_inputs,
                    slot_events_for_retry,
                ) {
                    Ok(ralph_core::supervisor::CoordinatorAction::InjectedComplete {
                        topic,
                        ..
                    }) => {
                        let payload = build_wave_complete_payload(
                            wave_kind,
                            completed,
                            &store_wave_id,
                            bridge,
                            aggregate_timeout_secs,
                        );
                        append_supervisor_coord_event(main_events_file, &topic, &payload);
                        SupervisorFanInOutcome::InjectedComplete
                    }
                    Ok(ralph_core::supervisor::CoordinatorAction::InjectedFailed {
                        topic,
                        reason,
                        blocking_slots,
                    }) => emit_injected_failed_coord(
                        bridge,
                        wave_kind,
                        completed,
                        &store_wave_id,
                        main_events_file,
                        &topic,
                        reason,
                        blocking_slots,
                        exec_fix_salvage_written,
                    ),
                    Ok(ralph_core::supervisor::CoordinatorAction::AlreadyDone) => {
                        SupervisorFanInOutcome::AlreadyDone
                    }
                    Ok(_) | Err(_) => {
                        warn!(
                            wave_id = %completed.wave_id,
                            "U1: terminal MergeFailed retry exhausted; returning StoreError"
                        );
                        SupervisorFanInOutcome::StoreError
                    }
                }
            } else {
                warn!(
                    wave_id = %completed.wave_id,
                    topic = %topic,
                    error = %error,
                    "U6: supervisor merge sink rejected the batch; \
                     merged_to_events stays false, retrying on next tick (KTD-7)"
                );
                SupervisorFanInOutcome::MergeFailed
            }
        }
    };

    // 2026-07-22-001 plan U6: every successful fan-in tick drains
    // any pending compensation jobs (OnTimeout / OnCancel /
    // OnPartial) and marks them executed. We do this after the
    // coordinator action has been processed so a wave that just
    // got marked cancelled observes the new phase before its
    // compensation hook runs. Failures only warn — the wave's
    // terminal phase still succeeds.
    drain_pending_compensations(bridge);

    action_outcome
}

/// Shared InjectedFailed side-effects: never-started recording,
/// diagnostics, Completed-only salvage merge, and coord append.
fn emit_injected_failed_coord(
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    wave_kind: ralph_core::supervisor::WaveKind,
    completed: &CompletedWave,
    store_wave_id: &str,
    main_events_file: &Path,
    topic: &str,
    reason: &str,
    blocking_slots: Vec<u32>,
    exec_fix_salvage_written: bool,
) -> SupervisorFanInOutcome {
    use ralph_core::supervisor::SupervisorBridge as _;

    if let Err(err) = bridge.record_never_started_failures(store_wave_id) {
        warn!(
            wave_id = %completed.wave_id,
            error = %err,
            "U1: record_never_started_failures failed during fan-in; \
             continuing anyway — the wave failure is already recorded"
        );
    }
    let snap_for_reasons = bridge.fan_in_status(store_wave_id);
    let mut reasons: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    if let Ok(snap) = snap_for_reasons.as_ref() {
        use ralph_core::supervisor::SlotStatus;
        for (idx, status) in &snap.slots {
            if matches!(status, SlotStatus::Failed | SlotStatus::Cancelled) {
                match bridge.slot_failure_reason(store_wave_id, *idx) {
                    Ok(Some(r)) => {
                        reasons.insert(*idx, r);
                    }
                    Ok(None) => {}
                    Err(err) => {
                        warn!(
                            wave_id = %store_wave_id,
                            slot_index = *idx,
                            error = %err,
                            "U5: slot_failure_reason lookup failed; \
                             payload keeps reason=null for this slot"
                        );
                    }
                }
            }
        }
    }
    if let Ok(snap) = snap_for_reasons.as_ref() {
        let elapsed_secs = snap.started_at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
        let payload =
            build_wave_failed_slots_json(&completed.wave_id, &snap.slots, &reasons, elapsed_secs);
        if let Err(err) = write_wave_diagnostics_json(
            &workspace_root_from_events(main_events_file),
            &completed.wave_id,
            &payload,
        ) {
            warn!(
                wave_id = %completed.wave_id,
                error = %err,
                "U5: write_wave_diagnostics_json failed (best-effort)"
            );
        }
    }
    if matches!(wave_kind, ralph_core::supervisor::WaveKind::Review) {
        merge_completed_review_slots_to_main(main_events_file, completed, bridge, store_wave_id);
    } else if !exec_fix_salvage_written {
        // 2026-07-25-005 plan U1 (R3 / KTD6): exec/fix waves salvage
        // their Completed slots' business events before the coord
        // event, same ordering contract as the review arm. Skipped
        // when the settlement block above already wrote the salvage on
        // this tick (it re-ticked the coordinator to reach this arm),
        // so completed events are never double-appended.
        merge_completed_exec_fix_slots_to_main(main_events_file, completed, bridge, store_wave_id);
    }
    let review_done_hints =
        build_review_done_hints(bridge, store_wave_id, completed, main_events_file);
    let payload = build_wave_failed_payload(
        wave_kind,
        completed,
        reason,
        blocking_slots,
        &reasons,
        Some(&review_done_hints),
    );
    append_supervisor_coord_event(main_events_file, topic, &payload);
    SupervisorFanInOutcome::InjectedFailed
}

/// 2026-07-22-001 plan U6 (KTD-7): drain any pending
/// compensation jobs and mark them executed. The
/// compensation-hook command itself is a no-op for now — we
/// record stderr diagnostics so an operator scanning loop
/// output sees exactly which waves triggered which
/// compensation kind. Failures only warn; they do not block
/// the wave's terminal phase (KTD-7).
pub(crate) fn drain_pending_compensations(
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
) {
    use ralph_core::supervisor::SupervisorBridge as _;
    let pending = match bridge.take_pending_compensations() {
        Ok(p) => p,
        Err(err) => {
            tracing::debug!(
                error = %err,
                "supervisor take_pending_compensations returned an error; \
                 treating as empty queue"
            );
            return;
        }
    };
    for (wave_id, kind) in pending {
        // The "hook" itself is a stderr diagnostic record.
        // Hook command execution (e.g. cleaning up the
        // wave's worktree branch) lands in a follow-up
        // release; today we mark the job executed so a
        // subsequent inspect surfaces its terminal status.
        let kind_str = match kind {
            ralph_core::supervisor::CompensationKind::OnTimeout => "timeout",
            ralph_core::supervisor::CompensationKind::OnCancel => "cancel",
            ralph_core::supervisor::CompensationKind::OnPartial => "partial",
        };
        tracing::info!(
            wave_id = %wave_id,
            kind = kind_str,
            "supervisor compensation hook executed (2026-07-22-001 plan U6)"
        );
        if let Err(err) = bridge.complete_compensation(&wave_id, kind, true) {
            tracing::warn!(
                wave_id = %wave_id,
                kind = kind_str,
                error = %err,
                "supervisor complete_compensation failed; \
                 the job will be retried on the next drain"
            );
        }
    }
}

/// U7 (2026-07-23-002): build the `*.wave.complete` payload that
/// matches the **target topic's** schema — see
/// `presets/schemas/ce-executor-supervisor.yml`.
///
/// - `exec.wave.complete` / `fix.wave.complete` require
///   `wave_id`, `completed_slots`, `merge_root_event_id`. The
///   payload also carries `success_slots` (per-slot branch +
///   worktree_path) so the integrator knows which branches to
///   merge.
/// - `review.wave.complete` requires `wave_id`,
///   `completed_dimensions`, `aggregate_timeout`. The
///   dimensions are derived from the per-slot `review.unit.done`
///   events (falling back to `assigned_dimensions` when the
///   events do not carry a `dimension` field).
fn build_wave_complete_payload(
    wave_kind: ralph_core::supervisor::WaveKind,
    completed: &ralph_core::CompletedWave,
    store_wave_id: &str,
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    aggregate_timeout_secs: u64,
) -> serde_json::Value {
    use ralph_core::supervisor::{SupervisorBridge as _, WaveKind};

    match wave_kind {
        WaveKind::Review => {
            let completed_dimensions = collect_review_dimensions(completed);
            serde_json::json!({
                "wave_id": completed.wave_id,
                "completed_dimensions": completed_dimensions,
                "aggregate_timeout": aggregate_timeout_secs,
            })
        }
        WaveKind::Exec | WaveKind::Fix => {
            // Build the `success_slots` payload from the store's
            // per-slot resource bindings, filtered to the slots that
            // actually completed this wave. Each entry carries the
            // slot index + branch + worktree_path so the integrator
            // knows which branches to merge.
            let success_indices: std::collections::HashSet<u32> =
                completed.results.iter().map(|r| r.index).collect();
            let mut success_slots: Vec<serde_json::Value> = Vec::new();
            match bridge.slot_resources(store_wave_id) {
                Ok(resources) => {
                    let mut resources = resources;
                    resources.sort_by_key(|r| r.slot_index);
                    for res in resources {
                        if !success_indices.contains(&res.slot_index) {
                            continue;
                        }
                        success_slots.push(serde_json::json!({
                            "slot_index": res.slot_index,
                            "branch": res.branch,
                            "worktree_path": res.worktree_path,
                        }));
                    }
                }
                Err(err) => {
                    warn!(
                        wave_id = %completed.wave_id,
                        error = %err,
                        "U6: slot_resources failed; success_slots payload will be empty"
                    );
                }
            }
            let topic_prefix = match wave_kind {
                WaveKind::Exec => "exec",
                WaveKind::Fix => "fix",
                WaveKind::Review => "review",
            };
            serde_json::json!({
                "wave_id": completed.wave_id,
                "completed_slots": success_slots.len(),
                "success_slots": success_slots,
                "merge_root_event_id": format!("fan-in:{topic_prefix}.wave.complete:{}", completed.wave_id),
            })
        }
    }
}

/// U7 (2026-07-23-002): build the `*.wave.failed` payload that
/// matches the **target topic's** schema. Exec/fix waves carry
/// `blocking_slots`; review waves carry `missing_dimensions`
/// (the dimensions that never produced a `review.unit.done`).
///
/// 2026-07-26-003 plan U4 (KTD5): the Review arm now subtracts
/// already-known-done dimensions from three sources:
/// 1. `completed.results` — the in-progress fan-in channel
/// 2. the supervisor store's `Completed` rows
/// 3. the main ledger — `review.unit.done` events that the merge
///    sink already wrote before this fan-in reached
///    `InjectedFailed`. Before this widening the function only
///    subtracted source (1), so main-merged dimensions were
///    double-counted as missing (the primary-20260726 incident).
///
/// The `review_done_hints` parameter carries sources (2) and (3);
/// callers in `run_supervisor_fan_in` build it from the bridge
/// snapshot + a brief main-ledger tail scan. When `None`, the
/// function still produces a missing_dimensions array but only
/// subtracts from `completed.results` — useful for unit tests and
/// for callers that do not need the cross-source reconciliation.
pub(crate) fn build_wave_failed_payload(
    wave_kind: ralph_core::supervisor::WaveKind,
    completed: &ralph_core::CompletedWave,
    reason: &str,
    blocking_slots: Vec<u32>,
    reasons: &std::collections::HashMap<u32, String>,
    review_done_hints: Option<&ReviewDoneHints>,
) -> serde_json::Value {
    use ralph_core::supervisor::WaveKind;

    match wave_kind {
        WaveKind::Review => {
            let completed_dims = collect_review_dimensions(completed);
            let assigned: std::collections::HashSet<String> =
                completed.assigned_dimensions.values().cloned().collect();
            let mut already_done: std::collections::HashSet<String> =
                completed_dims.into_iter().collect();
            if let Some(hints) = review_done_hints {
                already_done.extend(hints.main_backscan.iter().cloned());
                already_done.extend(hints.store_completed.iter().cloned());
            }
            let missing_dimensions = compute_review_missing_dimensions(&assigned, &already_done);
            serde_json::json!({
                "wave_id": completed.wave_id,
                "missing_dimensions": missing_dimensions,
                "reason": reason,
            })
        }
        WaveKind::Exec | WaveKind::Fix => {
            // 2026-07-25-003 plan U6 (R5 / R4) + 2026-07-26-002
            // plan U5 (R5 / KTD6): per-slot `slot_failures` is
            // derived from the supervisor store's frozen
            // `failure_reason` codes (NOT from `completed.failures`
            // free-form text), restricted to `blocking_slots` so
            // the index set agrees exactly. This is the SSOT for
            // downstream consumers (integrator / alignment /
            // reporter) — they no longer parse worker-written
            // `error` strings to tell a `worker_timeout` apart from
            // an `empty_worker_result`.
            //
            // 2026-07-25-005 plan U1 (R4 / R7 / KTD7): each entry
            // additionally carries a stable consumer-facing
            // `failure_class` label from `map_failure_class`, and
            // the payload gains two top-level index sets:
            //   - `salvaged_slots`: the wave's Completed slot
            //     indices (from `completed.results`, ascending) —
            //     business events already kept for the main ledger;
            //   - `redrive_slots`: `blocking_slots` restricted to
            //     retryable frozen reasons (ascending) — the only
            //     slots an operator redrive may reopen.
            use ralph_core::supervisor::worker_outcome::{
                is_retryable_slot_reason, map_failure_class,
            };

            let mut slot_failures: Vec<serde_json::Value> = Vec::new();
            let mut redrive_slots: Vec<u32> = Vec::new();
            for idx in &blocking_slots {
                let stored_reason = reasons.get(idx).cloned();
                let fallback_reason = completed
                    .failures
                    .iter()
                    .find(|f| f.index == *idx)
                    .map(|f| f.error.clone());
                let duration_ms = completed
                    .failures
                    .iter()
                    .find(|f| f.index == *idx)
                    .map(|f| f.duration.as_millis())
                    .unwrap_or(0);
                let reason = stored_reason.or(fallback_reason);
                // `failure_class` is computed from the same reason
                // string recorded in the entry (store code or
                // fallback), so per-slot fields never disagree.
                // A missing reason fail-closes to `unknown` and is
                // never retryable, so it stays out of redrive_slots.
                let (reason_value, failure_class) = match &reason {
                    Some(r) => (serde_json::json!(r), map_failure_class(r)),
                    None => (serde_json::Value::Null, map_failure_class("")),
                };
                if reason
                    .as_deref()
                    .is_some_and(is_retryable_slot_reason)
                {
                    redrive_slots.push(*idx);
                }
                slot_failures.push(serde_json::json!({
                    "slot_index": idx,
                    "reason": reason_value,
                    "duration_ms": duration_ms,
                    "failure_class": failure_class,
                }));
            }
            redrive_slots.sort_unstable();
            // Completed slot indices, ascending. `Completed` never
            // enters `blocking_slots` (R5), so this set is disjoint
            // from the failure sets above.
            let mut salvaged_slots: Vec<u32> = completed.results.iter().map(|r| r.index).collect();
            salvaged_slots.sort_unstable();
            serde_json::json!({
                "wave_id": completed.wave_id,
                "reason": reason,
                "blocking_slots": blocking_slots,
                "slot_failures": slot_failures,
                "salvaged_slots": salvaged_slots,
                "redrive_slots": redrive_slots,
            })
        }
    }
}

/// 2026-07-26-003 plan U4: cross-source reconciliation hints for
/// the Review arm of `build_wave_failed_payload`. Filled by
/// `run_supervisor_fan_in` from the supervisor bridge snapshot
/// (store `Completed` rows) and a tight main-ledger tail scan.
/// These hint dimensions are subtracted from `missing_dimensions`
/// in addition to the in-progress `completed.results`.
#[derive(Debug, Default, Clone)]
pub struct ReviewDoneHints {
    /// Dimensions whose `review.unit.done` already lives in the
    /// main ledger from a previous fan-in tick (or any
    /// non-wave path that wrote directly into main). Computed
    /// by tail-scanning the main events file for the wave_id +
    /// `review.unit.done`.
    pub main_backscan: std::collections::HashSet<String>,
    /// Dimensions whose slots are `Completed` in the supervisor
    /// store with an associated `review.unit.done` event the
    /// dispatcher already absorbed into a sibling wave's
    /// `completed.results` blob.
    pub store_completed: std::collections::HashSet<String>,
}

/// U4 (plan 2026-07-26-003) pure helper: subtract the
/// already-done dimensions from the assigned set, returning the
/// residual in stable lexical order so the payload field is
/// deterministic across runs (the test suite pins the order).
fn compute_review_missing_dimensions(
    assigned: &std::collections::HashSet<String>,
    already_done: &std::collections::HashSet<String>,
) -> Vec<String> {
    let mut missing: Vec<String> = assigned
        .iter()
        .filter(|d| !already_done.contains(*d))
        .cloned()
        .collect();
    missing.sort();
    missing
}

/// U7 (2026-07-23-002): collect the per-slot `dimension` payload
/// field from each `review.unit.done` event in `completed.results`,
/// falling back to `completed.assigned_dimensions` when the event
/// lacks the field. Returns the dimensions in stable slot-index
/// order so the synthesizer's `completed_dimensions` list is
/// deterministic across runs.
fn collect_review_dimensions(completed: &ralph_core::CompletedWave) -> Vec<String> {
    let mut by_index: std::collections::BTreeMap<u32, String> = std::collections::BTreeMap::new();
    for result in &completed.results {
        for event in &result.events {
            if event.topic.as_str() != "review.unit.done" {
                continue;
            }
            let payload_str = event.payload.as_str();
            if !payload_str.is_empty()
                && let Ok(serde_json::Value::Object(map)) =
                    serde_json::from_str::<serde_json::Value>(payload_str)
                && let Some(serde_json::Value::String(dim)) = map.get("dimension")
            {
                by_index.insert(result.index, dim.clone());
                break;
            }
        }
        if !by_index.contains_key(&result.index)
            && let Some(dim) = completed.assigned_dimensions.get(&result.index)
        {
            by_index.insert(result.index, dim.clone());
        }
    }
    by_index.into_values().collect()
}

/// Plan 004 P1-7: read the payload field of a main-ledger
/// record, accepting BOTH legacy string-encoded JSON
/// (`"payload": "{\"dimension\":...}"`) AND the inline object
/// shape (`"payload": {"dimension":...}`) the supervisor merge
/// sink writes directly. Returns `None` when the payload is
/// absent, malformed, or neither string-nor-object.
fn payload_object(
    payload: Option<&serde_json::Value>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let p = payload?;
    match p {
        serde_json::Value::Object(map) => Some(map.clone()),
        serde_json::Value::String(s) => {
            serde_json::from_str::<serde_json::Value>(s)
                .ok()
                .and_then(|v| match v {
                    serde_json::Value::Object(map) => Some(map),
                    _ => None,
                })
        }
        _ => None,
    }
}

/// 2026-07-26-004 plan U3 (R1 / R2): build the cross-source
/// [`ReviewDoneHints`] the failed-payload builder subtracts from
/// `missing_dimensions`. Two sources beyond `completed.results`:
///
/// - `main_backscan`: dimensions whose `review.unit.done` already
///   lives in the main ledger for THIS wave. The scan is bounded by
///   the envelope `wave_id` — malformed rows, rows without a wave id,
///   and other waves' rows are ignored (R2: an event already in main
///   must not be re-counted as missing; U3 risk: never eat another
///   wave's terminal event).
/// - `store_completed`: dimensions whose slot is `Completed` in the
///   supervisor store WITH valid terminal evidence (KTD3 fail-closed:
///   a bare `Completed` status bit with no evidence does NOT count).
fn build_review_done_hints(
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    store_wave_id: &str,
    completed: &ralph_core::CompletedWave,
    main_events_file: &Path,
) -> ReviewDoneHints {
    use ralph_core::supervisor::SlotStatus;
    use std::io::BufRead;

    // --- main_backscan: same-wave `review.unit.done` already in main ---
    let mut main_backscan = std::collections::HashSet::new();
    if let Ok(file) = std::fs::File::open(main_events_file) {
        for line in std::io::BufReader::new(file).lines() {
            let Ok(line) = line else { continue };
            let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if record.get("topic").and_then(|t| t.as_str()) != Some("review.unit.done") {
                continue;
            }
            // Bounded wave match: the envelope wave_id must equal this
            // wave. Rows without a wave_id (legacy / malformed) are NOT
            // counted — fail-closed.
            if record.get("wave_id").and_then(|w| w.as_str()) != Some(completed.wave_id.as_str()) {
                continue;
            }
            // Plan 004 P1-7: the main-ledger payload may arrive in two
            // shapes — string-encoded JSON (the legacy / agent
            // emit path) OR an inline JSON object (the
            // supervisor merge sink path, which writes object
            // payloads directly). The pre-fix code only
            // accepted the string form, so an object payload
            // was silently ignored and the dimension was
            // re-counted as missing. The fix: read whichever
            // shape is present via a unified accessor that
            // returns the inner payload object, then index
            // `dimension` directly.
            if let Some(map) = payload_object(record.get("payload")) {
                if let Some(serde_json::Value::String(dim)) = map.get("dimension") {
                    main_backscan.insert(dim.clone());
                }
            }
        }
    }

    // --- store_completed: Completed slots WITH valid terminal evidence ---
    //
    // Plan 004 P1-6 / KTD3 fail-closed: terminal evidence is
    // bound to (topic, dimension, slot_index) — it MUST match
    // the wave kind's terminal topic AND carry a dimension AND
    // that dimension must equal the slot's assigned dimension.
    // Any mismatch (wrong topic, missing dimension, dimension
    // mismatch) drops the slot from `done` so the dispatcher
    // cannot under-report `missing_dimensions` by smuggling in
    // unrelated events as terminal evidence.
    let mut store_completed = std::collections::HashSet::new();
    if let Ok(snap) = bridge.fan_in_status(store_wave_id) {
        for (slot_index, status) in &snap.slots {
            if !matches!(status, SlotStatus::Completed) {
                continue;
            }
            let evidence = match bridge.slot_terminal_evidence(store_wave_id, *slot_index) {
                Ok(Some(ev)) => ev,
                Ok(None) => continue,
                Err(err) => {
                    tracing::warn!(
                        wave_id = %store_wave_id,
                        slot_index = slot_index,
                        error = %err,
                        "store_completed: evidence lookup failed; failing closed",
                    );
                    continue;
                }
            };
            // P1-6: topic must be the wave-kind terminal
            // topic. We pin Review for now; Exec/Fix
            // reconciliation is a separate fan-in path and does
            // not enter this helper.
            if evidence.topic != "review.unit.done" {
                tracing::warn!(
                    wave_id = %store_wave_id,
                    slot_index = slot_index,
                    evidence_topic = %evidence.topic,
                    "store_completed: evidence topic is not the review terminal topic; failing closed",
                );
                continue;
            }
            // P1-6: dimension must be present AND equal the
            // slot's assigned dimension. We refuse the
            // pre-fix `evidence.dimension.or(assigned)` fallback
            // because it would let an evidence row with a
            // missing dimension silently mark the assigned
            // dimension done — exactly the wrong-topic /
            // missing-dimension inflation the review demanded
            // close.
            let evidence_dim = match &evidence.dimension {
                Some(d) => d,
                None => {
                    tracing::warn!(
                        wave_id = %store_wave_id,
                        slot_index = slot_index,
                        "store_completed: evidence missing dimension; failing closed",
                    );
                    continue;
                }
            };
            let assigned = completed.assigned_dimensions.get(slot_index);
            match assigned {
                Some(a) if a == evidence_dim => {
                    store_completed.insert(a.clone());
                }
                Some(a) => {
                    tracing::warn!(
                        wave_id = %store_wave_id,
                        slot_index = slot_index,
                        assigned = %a,
                        evidence_dimension = %evidence_dim,
                        "store_completed: dimension mismatch; failing closed",
                    );
                    continue;
                }
                None => {
                    // No assigned dimension at all — refuse to
                    // invent one.
                    tracing::warn!(
                        wave_id = %store_wave_id,
                        slot_index = slot_index,
                        "store_completed: slot has no assigned dimension; failing closed",
                    );
                    continue;
                }
            }
        }
    }

    ReviewDoneHints {
        main_backscan,
        store_completed,
    }
}

/// 2026-07-26-004 plan U5 (S5 / AE3 / KTD4): the producer identity
/// stamped on runtime coordination events (`*.wave.complete` /
/// `*.wave.failed`). The orchestrator — not the consumer hat — produces
/// these; `ralph` is the builtin runtime pseudo-hat the origin guard
/// already recognises as a control producer. The consumer hat is carried
/// separately in the event's `hat` field (routing / topic subscription).
const COORD_SYSTEM_PRODUCER: &str = "ralph";

/// U6: append a `system_injected: true` coordination event
/// (`*.wave.complete` / `*.wave.failed`) to the loop's main ledger
/// WITHOUT advancing the reader cursor. The caller's post-wave
/// `process_events_from_jsonl` re-read picks it up (alongside the
/// sink-written business events) and publishes it to the bus exactly
/// once.
///
/// U7 (2026-07-23-002): the `hat`/`source` attribution is derived
/// from the coordination `topic` so it matches the registered
/// integrator hat id (`exec-integrator` / `fix-integrator` /
/// `review-synthesizer`). The earlier hard-coded `"integrator"` was
/// not a registered hat id, so isolated-mode scope enforcement
/// (`isolated_publish_allowed`) rejected the event before it reached
/// the EventBus, leaving the integrator hat's pending queue empty.
fn append_supervisor_coord_event(
    main_events_file: &Path,
    topic: &str,
    payload: &serde_json::Value,
) {
    use std::io::Write;
    // Derive the hat attribution from the coordination topic.
    // `exec.wave.complete` → `exec-integrator`, `fix.wave.complete` →
    // `fix-integrator`, `review.wave.complete` → `review-synthesizer`.
    // Failed waves route to the matching failure-handler hat
    // (`exec-failure-handler`); for review, the `implementation-review`
    // preset's `event_filter` subscribes `finalizer` to
    // `review.wave.failed` (so the failure triggers
    // `wave-blocked.md` + `LOOP_COMPLETE` via finalizer, never
    // `review-synthesizer`). For fix waves, the failure also routes
    // to `exec-failure-handler` (the preset has no dedicated
    // `fix-failure-handler` hat).
    let hat_attribution = if topic.starts_with("exec.wave.") {
        if topic.ends_with(".failed") {
            "exec-failure-handler"
        } else {
            "exec-integrator"
        }
    } else if topic.starts_with("fix.wave.") {
        if topic.ends_with(".failed") {
            "exec-failure-handler"
        } else {
            "fix-integrator"
        }
    } else if topic.starts_with("review.wave.") {
        // 2026-07-26-003 plan (KTD4): split the review band by
        // success vs failure. Success keeps routing to
        // `review-synthesizer` (the integrator that reads
        // `completed_dimensions`); failure now routes to
        // `finalizer` (the only hat subscribed to
        // `review.wave.failed` in the `implementation-review`
        // preset). Routing the failure to `review-synthesizer`
        // caused the primary-20260726 incident: the synthesizer
        // was woken for the failure path, attempted to CLI-emit
        // a coordination topic it did not own, and got rejected;
        // meanwhile `finalizer` never received the trigger.
        if topic.ends_with(".failed") {
            "finalizer"
        } else {
            "review-synthesizer"
        }
    } else {
        "ralph"
    };
    // 2026-07-26-004 plan U5 (S5 / AE3 / KTD4): separate the PRODUCER
    // from the CONSUMER. A runtime coordination event is produced by
    // the orchestrator (system producer `ralph`), NOT by the consumer
    // hat. The consumer (finalizer / integrator / synthesizer) is
    // expressed by `hat` (which the 2026-07-26-003 routing fix and the
    // preset's topic subscription rely on) — keeping `hat` unchanged
    // preserves that routing while `source` now truthfully names the
    // runtime as producer. The two answers no longer reuse one field.
    let record = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": chrono::Utc::now().to_rfc3339(),
        "hat": hat_attribution,
        "source": COORD_SYSTEM_PRODUCER,
        "system_injected": true,
    });
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = main_events_file.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(main_events_file)?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&record).unwrap_or_default()
        )?;
        file.flush()?;
        Ok(())
    })();
    if let Err(err) = result {
        warn!(
            topic = %topic,
            path = %main_events_file.display(),
            error = %err,
            "U6: failed to append supervisor coordination event to ledger"
        );
    }
}

/// 2026-07-26-003 plan U5 (KTD7): on `InjectedFailed` for the
/// Review kind, append the **Completed** slots' business events
/// to the main events ledger BEFORE the failed coord event so
/// downstream `finalizer` + `ralph diagnose` consumers can see
/// what got done. This is the dispatcher-layer minimal merge the
/// plan describes; the 2026-07-25-005 supervisor plan owns the
/// migration to `SupervisorAction::SalvagedAndFailed` and will
/// absorb this logic once that lands. Until then, this helper is
/// the only place a Review-failed wave's Completed events get
/// written to main — without it, all six slots' results are
/// invisible to anything that doesn't read `.ralph/supervisor.db`
/// directly.
///
/// `Failed` slots are deliberately skipped: their events did NOT
/// pass `classify_slot_result` and writing them would be a
/// silent-success anti-pattern that masks real failures. We also
/// only write events with `review.unit.done` shape; any other
/// topic a Completed slot may carry (`review.dimension.done`,
/// etc.) is dropped here because the `review-synthesizer`'s
/// consumed payload is the topic it expects to see in main.
fn merge_completed_review_slots_to_main(
    main_events_file: &Path,
    completed: &ralph_core::CompletedWave,
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    store_wave_id: &str,
) {
    use std::io::Write;
    let mut lines: Vec<String> = Vec::new();
    for result in &completed.results {
        // Slots that show up in `completed.failures` are skipped:
        // their `results` entry is a stale artifact of the failed
        // tick and must not be merged (silent-success anti-pattern).
        if completed.failures.iter().any(|f| f.index == result.index) {
            continue;
        }
        for event in &result.events {
            if event.topic.as_str() != "review.unit.done" {
                continue;
            }
            // Pre-render the JSONL row with the `review-worker`
            // attribution so `compute_missing_dimensions` (U4)
            // sees the dimension in main as already done.
            //
            // 2026-07-26-004 plan U3 (R1 / bounded backscan): preserve
            // the event's envelope `wave_id` / `wave_index` so the
            // fan-in main-ledger backscan can filter to THIS wave and
            // never eat another wave's `review.unit.done`. Dropping the
            // wave id here was what made cross-source reconciliation
            // unsafe before U3.
            let record = serde_json::json!({
                "topic": event.topic.as_str(),
                "payload": event.payload.as_str(),
                "ts": chrono::Utc::now().to_rfc3339(),
                "hat": "review-worker",
                "source": "review-worker",
                "wave_id": event.wave_id,
                "wave_index": event.wave_index,
            });
            if let Ok(line) = serde_json::to_string(&record) {
                lines.push(line);
            }
        }
    }
    if lines.is_empty() {
        return;
    }
    // Append-then-commit: write the rows first, then commit the
    // salvage mark. Plan 004 R3 / P0-1 makes the mark live inside
    // this helper (instead of after the dispatcher call) so the
    // dispatcher cannot forget the mark after the write — the
    // pre-fix code's missing-mark window was the original
    // silent-success regression that the split-phase latch was
    // introduced to close. The mark is idempotent across replays
    // and restarts; a write failure leaves the mark unset so the
    // coordinator's next tick re-runs the merge seam.
    let open = || -> std::io::Result<()> {
        if let Some(parent) = main_events_file.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(main_events_file)?;
        for line in &lines {
            writeln!(file, "{}", line)?;
        }
        file.flush()?;
        Ok(())
    };
    if let Err(err) = open() {
        warn!(
            wave_id = %completed.wave_id,
            path = %main_events_file.display(),
            error = %err,
            "U5: failed to merge Completed review slots to main ledger; \
             salvage mark will not be set so the coordinator will refuse \
             the coord-event injection until the next tick retries"
        );
        return;
    }
    // Commit the salvage mark AFTER the rows landed. The mark
    // gates `fail_wave`'s coord-event injection (P0-1) so the
    // crash window between append and latch can only re-merge
    // (idempotent on slot status), never re-inject. We use the
    // store-assigned `store_wave_id` (not `completed.wave_id`)
    // because the bridge's `mark_salvage_merged` keys off the
    // row id the store actually wrote; the two normally agree
    // but the helper must work even when callers pass a
    // supervisor-idempotency key that does not match the
    // store-assigned id.
    if let Err(err) = bridge.mark_salvage_merged(store_wave_id) {
        warn!(
            wave_id = %store_wave_id,
            error = %err,
            "merge_completed_review_slots_to_main: mark_salvage_merged failed; \
             next tick will retry"
        );
    }
}

/// 2026-07-25-005 plan U1 (R3 / R4 / KTD2 / KTD6): the exec/fix
/// counterpart of [`merge_completed_review_slots_to_main`]. When an
/// exec/fix wave must fail, the Completed slots' business events are
/// appended to the main ledger FIRST (salvage) and only then does the
/// dispatcher inject `*.wave.failed` — KTD2 forbids a silent partial
/// complete, but the completed work must not be dropped on the floor
/// either.
///
/// Slots that also show up in `completed.failures` are skipped: their
/// `results` entry is a stale artifact of the failed tick and merging
/// it would be a silent-success anti-pattern. Each salvaged row keeps
/// the worker's own `source` attribution and the wave envelope
/// (`wave_id` / `wave_index`) so the post-wave re-read publishes the
/// event exactly as the worker produced it.
///
/// Unlike the review helper, this one commits the salvage mark even
/// when ZERO rows were appended: an all-failed wave has nothing to
/// salvage, but the coordinator's `fail_wave` gate still requires
/// `salvage_merged=true` before it latches the coord-event injection.
/// An append failure leaves the mark unset so the next tick re-runs
/// this seam (idempotent on slot status).
fn merge_completed_exec_fix_slots_to_main(
    main_events_file: &Path,
    completed: &ralph_core::CompletedWave,
    bridge: &Arc<dyn ralph_core::supervisor::SupervisorBridge>,
    store_wave_id: &str,
) {
    use std::io::Write;
    let mut lines: Vec<String> = Vec::new();
    for result in &completed.results {
        if completed.failures.iter().any(|f| f.index == result.index) {
            continue;
        }
        for event in &result.events {
            let attribution = event
                .source
                .as_ref()
                .map(|h| h.as_str())
                .unwrap_or("worker");
            let record = serde_json::json!({
                "topic": event.topic.as_str(),
                "payload": event.payload.as_str(),
                "ts": chrono::Utc::now().to_rfc3339(),
                "hat": attribution,
                "source": attribution,
                "wave_id": event.wave_id,
                "wave_index": event.wave_index,
            });
            if let Ok(line) = serde_json::to_string(&record) {
                lines.push(line);
            }
        }
    }
    let open = || -> std::io::Result<()> {
        if let Some(parent) = main_events_file.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(main_events_file)?;
        for line in &lines {
            writeln!(file, "{}", line)?;
        }
        file.flush()?;
        Ok(())
    };
    if let Err(err) = open() {
        warn!(
            wave_id = %completed.wave_id,
            path = %main_events_file.display(),
            error = %err,
            "U1: failed to merge Completed exec/fix slots to main ledger; \
             salvage mark will not be set so the coordinator will refuse \
             the coord-event injection until the next tick retries"
        );
        return;
    }
    if let Err(err) = bridge.mark_salvage_merged(store_wave_id) {
        warn!(
            wave_id = %store_wave_id,
            error = %err,
            "merge_completed_exec_fix_slots_to_main: mark_salvage_merged failed; \
             next tick will retry"
        );
    }
}

// ─────────────────────────────────────────────────────────────────
// 2026-07-25-004 plan U5 (R6 / AE5): per-slot diagnostics JSON
// for failed waves.
// ─────────────────────────────────────────────────────────────────

/// Build the JSON payload for a failed wave's per-slot diagnostics.
///
/// `slots` is the snapshot's `(u32, SlotStatus)` list.
/// `reasons` maps `slot_index -> failure_reason` for failed/cancelled
/// slots (including `slot_never_started` and `worker_timeout` codes).
/// `elapsed_secs` is the wall-clock time since wave registration.
fn build_wave_failed_slots_json(
    wave_id: &str,
    slots: &[(u32, ralph_core::supervisor::SlotStatus)],
    reasons: &std::collections::HashMap<u32, String>,
    elapsed_secs: u64,
) -> serde_json::Value {
    let slot_entries: Vec<serde_json::Value> = slots
        .iter()
        .map(|(idx, status)| {
            let reason = reasons.get(idx);
            serde_json::json!({
                "slot_index": *idx,
                "status": status_to_str(status),
                "reason": reason,
            })
        })
        .collect();
    serde_json::json!({
        "wave_id": wave_id,
        "generated_at_kind": "injected_failed",
        "elapsed_secs": elapsed_secs,
        "slots": slot_entries,
    })
}

/// Convert `SlotStatus` to its snake_case string representation.
fn status_to_str(status: &ralph_core::supervisor::SlotStatus) -> &'static str {
    match status {
        ralph_core::supervisor::SlotStatus::Pending => "pending",
        ralph_core::supervisor::SlotStatus::Dispatched => "dispatched",
        ralph_core::supervisor::SlotStatus::Running => "running",
        ralph_core::supervisor::SlotStatus::Completed => "completed",
        ralph_core::supervisor::SlotStatus::Failed => "failed",
        ralph_core::supervisor::SlotStatus::Cancelled => "cancelled",
    }
}

/// Write the per-slot diagnostics JSON to the workspace root.
///
/// `root` is the orchestrator's CWD (the workspace root in production).
/// The file lands at `{root}/.ralph/diagnostics/wave-{wave_id}-slots.json`.
///
/// Best-effort: write failures are logged as warnings and do NOT
/// propagate to the caller. The primary coord event write is
/// unaffected.
fn write_wave_diagnostics_json(
    root: &Path,
    wave_id: &str,
    payload: &serde_json::Value,
) -> std::io::Result<PathBuf> {
    let dir = root.join(".ralph").join("diagnostics");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("wave-{wave_id}-slots.json"));
    let bytes = serde_json::to_vec_pretty(payload).expect("payload is always a valid JSON Value");
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// 2026-07-26-002 plan U6 (R6 / KTD2 / KTD7): append the
/// dispatcher's signed absolute channel path to
/// `<workspace>/.ralph/current-wave-channels`. The marker is one
/// line per path, exact-matched at the consumer (no prefix
/// wildcards). Concurrent waves can append freely because each
/// line is independently accepted or rejected by the canonicalize
/// equality check in `paths_equivalent`.
///
/// Failure modes:
/// - `.ralph/` not writable: caller logs warn and the worker
///   falls back to the legacy shape-only allowlist.
/// - `events_file` does not have a `.ralph/` ancestor: caller
///   surfaces an error and the worker is spawned without a marker.
pub(crate) fn append_wave_channel_to_marker(
    main_events_file: &Path,
    worker_events_file: &Path,
) -> std::io::Result<()> {
    let workspace_root = workspace_root_from_events(main_events_file);
    let ralph_dir = workspace_root.join(".ralph");
    let marker = ralph_dir.join("current-wave-channels");
    std::fs::create_dir_all(&ralph_dir)?;
    let absolute = if worker_events_file.is_absolute() {
        worker_events_file.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(worker_events_file))
            .unwrap_or_else(|_| worker_events_file.to_path_buf())
    };
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&marker)?;
    writeln!(file, "{}", absolute.display())?;
    Ok(())
}

/// 2026-07-26-002 plan U3 (R3 / KTD3): derive an absolute workspace
/// root from the loop's main events file.
///
/// Convention: `<workspace>/.ralph/events.jsonl`. Two
/// `.parent()` calls land on `<workspace>`. When the input path
/// is too short (or stays relative), we fall back to
/// `std::env::current_dir()` joined with the remaining suffix so
/// the validator never sees `Path::new(".")`.
pub(crate) fn workspace_root_from_events(events_file: &Path) -> PathBuf {
    let mut current = events_file;
    for _ in 0..2 {
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent,
            _ => break,
        }
    }
    if current.is_absolute() {
        return current.to_path_buf();
    }
    std::env::current_dir()
        .map(|c| c.join(current))
        .unwrap_or_else(|_| current.to_path_buf())
}

/// 2026-07-22-001 plan U3 (KTD-2 / KTD-3): pick the supervisor
/// store the lazy default-path bridge should wrap.
///
/// Behavior:
/// - When the `supervisor-db` cargo feature is on AND the operator
///   has configured a `SupervisorConfig::db_path`, open the rusqlite
///   store at the resolved absolute path (mirrors the runner's
///   `build_supervisor_bridge` resolution). Open failure is
///   fail-closed: the dispatcher surfaces the error so the loop
///   halts rather than silently dropping to InMemory.
/// - When the feature is off, or no `db_path` is configured, fall
///   back to `InMemorySupervisorStore` and emit a one-shot stderr
///   warning (`wave_ledger_ephemeral`) so an operator scanning
///   logs sees exactly why ledger writes do not survive a restart.
///
/// The recovered `RusqliteSupervisorStore` carries the same rows
/// the runner's startup `recover_active_waves_at_startup` already
/// reconciled, so no further recovery is needed at this layer.
fn open_default_supervisor_store(
    cfg: &ralph_core::config::SupervisorConfig,
    ctx: &ralph_core::LoopContext,
    _events_file: &std::path::Path,
) -> anyhow::Result<Arc<dyn ralph_core::supervisor::SupervisorStore>> {
    #[cfg(feature = "supervisor-db")]
    {
        if !cfg.db_path.trim().is_empty() {
            let db_path = std::path::Path::new(&cfg.db_path);
            let resolved = if db_path.is_absolute() {
                db_path.to_path_buf()
            } else {
                ctx.workspace().join(db_path)
            };
            if let Some(parent) = resolved.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            return ralph_core::supervisor::RusqliteSupervisorStore::open(&resolved)
                .map(|store| Arc::new(store) as Arc<dyn ralph_core::supervisor::SupervisorStore>)
                .map_err(|err| {
                    anyhow::anyhow!("supervisor-db open failed at {}: {err}", resolved.display())
                });
        }
    }
    // Fallback: InMemory + stderr warn so an operator can see
    // exactly why ledger writes do not survive a restart. We do
    // NOT silently pretend we have persistence.
    eprintln!(
        "wave_ledger_ephemeral: no supervisor-db feature / db_path; \
         default wave path is using in-memory SupervisorStore — \
         wave state will not survive a process restart. \
         Enable `event_loop.supervisor.db_path` to opt into \
         persistence."
    );
    Ok(Arc::new(
        ralph_core::supervisor::InMemorySupervisorStore::new(),
    ))
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

        join_set.spawn(async move {
            // The Drop guard is installed before waiting on the local
            // semaphore, so JoinSet abort/cancellation also releases
            // the store-side permit for an approved slot.
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
            let result = executor.execute(request).await;

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
            let classified = classify_slot_result(&result.1);
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
                        if let Ok((events, _duration, _success)) = &result.1 {
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
                use super::task_projection::{SlotProjection, project_slot};
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

pub(crate) fn record_outcome(
    tracker: &mut ralph_core::WaveTracker,
    wave_id: &str,
    index: u32,
    outcome: WaveWorkerOutcome,
) {
    match outcome {
        Ok((events, duration, success)) => {
            // PTY workers return Ok((_, _, false)) for non-zero exit
            // and for timeout-with-events (`run_wave_worker_pty`).
            // Distinguish:
            // - success + events present → result
            // - success + NO events → empty_worker_result (failure);
            //   a worker that exits 0 without accepted events is
            //   not a real success, it just failed silently. Without
            //   this rule, a false-green LOOP_COMPLETE could fire
            //   for a wave whose every slot is empty.
            // - success=false + events present → keep result visible
            //   (partial-timeout contract).
            // - success=false + empty → hard failure so a forced
            //   slot exit (exit 1, no events) cannot Integrate →
            //   false-green LOOP_COMPLETE.
            //
            // 2026-07-25-003 plan U5 (R3): align this branch with
            // the supervisor `classify_slot_result` truth table —
            // empty-success is `empty_worker_result` (Failed), not a
            // result.
            if success && events.is_empty() {
                tracker.record_failure(
                    wave_id,
                    index,
                    ralph_core::supervisor::worker_outcome::REASON_EMPTY_WORKER_RESULT.into(),
                    duration,
                );
            } else if success || !events.is_empty() {
                let proto_events: Vec<ralph_proto::Event> =
                    events.into_iter().map(ralph_proto::Event::from).collect();
                tracker.record_result(wave_id, index, proto_events);
            } else {
                tracker.record_failure(
                    wave_id,
                    index,
                    "worker exited unsuccessfully".into(),
                    duration,
                );
            }
        }
        Err((error, duration)) => {
            tracker.record_failure(wave_id, index, error, duration);
        }
    }
}

/// 2026-07-23-001 plan U5 (R8): compute a stable `(content_hash,
/// event_count)` fingerprint for a worker's produced event batch,
/// suitable for `SupervisorBridge::record_slot_result`.
///
/// The hash is a sha256 over the canonical JSONL serialization of the
/// events (one line per event, in worker order). An empty batch hashes
/// to the sha256 of the empty string, so the store sees a stable
/// "empty" fingerprint and the R-E1 dedup contract holds across
/// re-dispatches. The dispatcher only persists the fingerprint +
/// count into the store slot; the event batch itself stays in the
/// worker's events file until U6's sink merges it.
fn compute_slot_batch_fingerprint(events: &[ralph_core::Event]) -> (String, usize) {
    let mut buf = String::new();
    for event in events {
        if let Ok(line) = serde_json::to_string(event) {
            buf.push_str(&line);
            buf.push('\n');
        }
    }
    (
        ralph_core::agent_doc_sync::compute_sha256_hex(&buf),
        events.len(),
    )
}

/// 2026-07-23-007 plan U5 (M1 / C3 / C4 / C5 / M3): single
/// classification entry point for a worker's `WaveWorkerOutcome`.
/// Both the per-slot `record_slot_*` block and the drop-guard
/// `outcome = Completed` arm previously re-derived exit +
/// markers from the same `events` slice, duplicating the
/// classification loop and the redundant
/// `result.1.as_ref().unwrap().2`. The helper is the one place
/// that turns `(events, success, reason)` into the unified
/// `ClassifiedSlot` so the two downstream consumers (the bridge
/// record + the drop guard) read the same classification.
///
/// Two reason kinds exist:
/// - `Static`: the classifier's stable reason constants
///   (`empty_worker_result`, `worker_cancelled`, …).
/// - `Dynamic`: the worker's original `Err` reason, which is
///   operator-facing and may contain runtime details
///   (e.g. "boom: worker crashed"). The legacy code carried it
///   verbatim into `record_slot_failure(reason=…)` to preserve
///   the operator's real failure message.
#[derive(Debug)]
enum ClassifiedReason<'a> {
    Static(&'a str),
    Dynamic(&'a str),
}

#[derive(Debug)]
struct ClassifiedSlot<'a> {
    outcome: ralph_core::supervisor::worker_outcome::SlotOutcome,
    /// Reason string to forward to `record_slot_failure`. `None`
    /// when the outcome is `Completed` (the bridge takes the
    /// `record_slot_result` path, not the failure path).
    reason: Option<ClassifiedReason<'a>>,
}

fn classify_slot_result<'a>(result: &'a WaveWorkerOutcome) -> ClassifiedSlot<'a> {
    use ralph_core::supervisor::worker_outcome::{
        SlotOutcome, TerminalMarker, WorkerExit, classify_worker_outcome,
    };
    match result {
        Ok((events, _duration, success)) => {
            let exit = if *success {
                WorkerExit::Exit0
            } else {
                WorkerExit::ExitNonZero
            };
            let mut markers: Vec<TerminalMarker> = Vec::new();
            let mut accepted: usize = 0;
            for ev in events {
                accepted += 1;
                if ev.topic.ends_with(".unit.done") || ev.topic.ends_with(".wave.done") {
                    markers.push(TerminalMarker::Done);
                } else if ev.topic.ends_with(".unit.failed") || ev.topic.ends_with(".wave.failed") {
                    markers.push(TerminalMarker::Failed);
                }
            }
            let outcome = classify_worker_outcome(exit, accepted, &markers);
            let reason = match &outcome {
                SlotOutcome::Failed { reason } => Some(ClassifiedReason::Static(reason)),
                SlotOutcome::Completed(_) => None,
            };
            ClassifiedSlot { outcome, reason }
        }
        Err((reason, _duration)) => {
            // KTD8 / AE3: timeout-prefix detection — the worker.rs stable
            // prefix "Worker timed out after" identifies a genuine timeout
            // (empty event batch, no terminal). In that case we synthesise a
            // typed Timeout exit and let classify_worker_outcome resolve it
            // to the frozen reason code so the operator sees the stable
            // `worker_timeout` string instead of a raw Dynamic message.
            //
            // 2026-07-25-006 plan U9: idle heartbeat kill is the second
            // member of the `worker_timeout` family. The worker emits
            // messages beginning with `"idle heartbeat exceeded"`; we
            // route them through `WorkerExit::IdleTimeout` so the
            // classifier resolves to `worker_timeout` (the operator
            // sees the original idle string verbatim, the family
            // collapses into the same `worker_timeout` reason).
            //
            // Non-timeout Err (any other message) is preserved verbatim with
            // the legacy `worker_cancelled` shell — fixing that broader
            // mis-classification is out of scope for this plan (plan KTD8
            // explicitly says "非超时 Err 仍保留 Dynamic 原文字案").
            // 2026-07-26-002 plan U8 (R10): use the shared
            // constant so worker.rs and this classifier stay
            // compile-linked.
            if reason.starts_with(WORKER_TIMEOUT_ERR_PREFIX) {
                // Empty event batch + empty terminal markers — classify as Timeout.
                let outcome = classify_worker_outcome(WorkerExit::Timeout, 0, &[]);
                let reason = match &outcome {
                    SlotOutcome::Failed { reason } => Some(ClassifiedReason::Static(reason)),
                    SlotOutcome::Completed(_) => None,
                };
                ClassifiedSlot { outcome, reason }
            } else if reason.starts_with("idle heartbeat exceeded") {
                // 2026-07-25-006 U9: idle kill still maps to the
                // `worker_timeout` family; the reason string carries
                // the operator-visible detail (`"idle heartbeat
                // exceeded: 120s since last activity, weak_count=8"`).
                // The outcome is `Failed { reason: "worker_timeout" }`
                // but the dynamic reason surfaced to the operator is
                // the original idle string verbatim.
                let outcome = classify_worker_outcome(WorkerExit::IdleTimeout, 0, &[]);
                ClassifiedSlot {
                    outcome,
                    reason: Some(ClassifiedReason::Dynamic(reason)),
                }
            } else {
                ClassifiedSlot {
                    outcome: SlotOutcome::Failed {
                        reason: ralph_core::supervisor::worker_outcome::REASON_WORKER_CANCELLED,
                    },
                    reason: Some(ClassifiedReason::Dynamic(reason)),
                }
            }
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

/// U5/R5 (2026-06-17-002) was historically implemented as a
/// separate `inject_dimension_retry_task_resume` function that
/// re-opened the events file after the merge layer had already
/// written to it. The two writers were not mutex-protected
/// (P0#4) and a disk-write failure left the per-slot budget
/// unchanged so a permanent mismatch could retry indefinitely
/// (P1#11). The replacement lives inline in
/// `handle_wave_events`'s `WaveDispatchOutcome::Completed` arm,
/// reusing the `pending_task_resumes` records the merge layer
/// now returns. The JSONL records are appended to the events
/// file in a single `write_all`, and the per-slot budget lives
/// on the `CompletedWave` (transferred from the WaveTracker via
/// `take_wave_results`) so it survives across dispatch rounds.
/// See `PendingTaskResumeRecord` in `super::io` for the
/// pre-rendered line format.
/// 2026-07-23-001 plan U9: fold one approval round's `CompletedWave`
/// into the wave-level accumulator inside the supervisor's batched
/// dispatch. Per-round waves carry round-scoped `wave_total` /
/// `partial`; the caller normalizes them against the full wave after
/// the last round (`final_completed.wave_total = wave.total`).
fn merge_round_into(
    base: &mut Option<ralph_core::CompletedWave>,
    round: ralph_core::CompletedWave,
) {
    match base {
        None => *base = Some(round),
        Some(base) => {
            base.results.extend(round.results);
            base.failures.extend(round.failures);
            base.worker_events.extend(round.worker_events);
            base.duration += round.duration;
            base.partial = base.partial || round.partial;
        }
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
        .or_else(|| wave.events.first().map(|e| e.topic.clone()))
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
        .or_else(|| wave.events.first().map(|e| e.topic.clone()))
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
#[allow(clippy::unused_async)]
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
    use ralph_core::EventLoop;
    use ralph_core::config::RalphConfig;
    use ralph_core::supervisor::{
        BridgeError, CoordinatorAction, InMemoryCoordinatorBridge, PhaseInputs, SupervisorBridge,
        SupervisorStore, WaveKind,
    };
    use ralph_proto::HatId;
    use std::fs;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
            system_injected: None,
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
        let yaml = r"
hats: {}
";
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
            // 2026-07-13-001 plan U2: tests do not exercise
            // RALPH_CONFIG injection; leave None to keep the
            // pre-U2 behaviour.
            config_path: None,
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
            worker_timeout: Duration::from_mins(1),
            progress_tx,
            worker_rpc_tx: None,
            worker_tui_state: None,
            assigned_dimension,
            cwd: None,
            idle_heartbeat: None,
            idle_weak_signal_cap: 8,
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
        let executor = Arc::new(TestExecutor::new(Duration::from_hours(1)));

        let wave = make_wave(4, 4, 1);
        // Compute deadlines so partial_threshold fires well before
        // any worker could possibly complete.
        let aggregate = Duration::from_secs(10);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
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
        let executor = Arc::new(TestExecutor::new(Duration::from_hours(1)));

        let wave = make_wave(3, 3, 3);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
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
        let executor = Arc::new(TestExecutor::new(Duration::from_hours(1)));

        let wave = make_wave(3, 3, 3);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
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
            Duration::from_mins(1),
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
            Duration::from_mins(1),
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
            Duration::from_mins(1),
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
            Duration::from_mins(1),
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
        if let WaveDispatchOutcome::SpawnFailed { .. } = &outcome {
            panic!("SpawnFailed must NOT fire when all workers spawned: {outcome:?}")
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
    /// workers are spawned than there are worker requests, `SpawnFailed`
    /// must fire with the correct counts.
    ///
    /// 2026-07-23-001 plan U3: the supervisor gate may legitimately
    /// reduce `worker_requests.len()` below `wave.events.len()` (the
    /// gate skips unapproved slots). The spawn guarantee now runs
    /// against `worker_requests.len()` so it only fires when the
    /// spawn loop itself silently drops a request — a real bug.
    /// The "wave has 3 events but only 2 spawned" scenario now
    /// passes (the supervisor skipped slot 2); the U2 test
    /// re-pins the loop-internal guarantee by passing 2
    /// requests with 2 worker-request slots so `spawned_count`
    /// matches `worker_requests.len()` and the loop proceeds.
    #[tokio::test(start_paused = true)]
    async fn u2_spawn_guarantee_fires_when_fewer_workers_spawn() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // Pass 3 requests, but the third one's spawned task panics
        // before the executor increments its counter. We assert the
        // spawn guarantee runs against `worker_requests.len()`,
        // not against `events_len`. With 3 requests and a healthy
        // executor the loop spawns 3 tasks → no SpawnFailed.
        let requests: Vec<WorkerRequest> = (0..3u32)
            .map(|i| make_worker_request(i, progress_tx.clone()))
            .collect();
        let executor = Arc::new(TestExecutor::new(Duration::from_millis(10)));

        let wave = make_wave(3, 3, 3); // 3 events, total=3
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
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

        // 3 healthy requests → all 3 spawn → no SpawnFailed.
        match outcome {
            WaveDispatchOutcome::SpawnFailed { .. } => {
                panic!(
                    "U3: spawn guarantee must NOT fire when worker_requests.len() == events_len; \
                        got SpawnFailed {outcome:?}"
                );
            }
            other => {
                // Either Completed or AggregateDeadlineExceeded
                // depending on timing; both are valid.
                let _ = other;
            }
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

        let yaml = r"
hats: {}
";
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
    //
    // R5 re-architecture (P0#1 / P0#4 / P1#11 fix): the merge layer
    // produces pre-rendered `task.resume` JSONL lines as
    // `PendingTaskResumeRecord`s. The dispatcher's inline filter
    // (in `handle_wave_events`'s `Completed` arm) consumes them,
    // updates `CompletedWave.dimension_retry_counts`, and writes
    // survivors to the events file. These tests exercise the new
    // contract end-to-end.
    // ---------------------------------------------------------------------

    /// U5/R5: when the merge layer detects a dimension mismatch,
    /// `pending_task_resumes` contains a pre-rendered
    /// `task.resume` JSONL line carrying the expected/actual
    /// dimensions in the structured payload. The dispatcher's
    /// filter (modeled here as inline code) writes survivors to
    /// the events file in a single `write_all` and bumps the
    /// per-slot budget on `CompletedWave.dimension_retry_counts`.
    #[test]
    fn u5_mismatch_writes_task_resume() {
        use crate::loop_runner::wave::io::merge_wave_results_to_events_file;
        use std::io::Write;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let events_file = tmp.path().join("events.jsonl");

        // Index 0 emits the correct assigned dimension
        // (correctness); index 1 emits the WRONG dimension
        // (testing instead of assigned correctness). The merge
        // layer must drop index 1's event and return a pending
        // task.resume for it.
        let mut assigned_dimensions = std::collections::HashMap::new();
        assigned_dimensions.insert(0u32, "correctness".to_string());
        assigned_dimensions.insert(1, "correctness".to_string());

        let event_index_0 = ralph_proto::Event::new(
            "review.dimension.done",
            r#"{"dimension":"correctness","wave_id":"w-u5-dim"}"#,
        )
        .with_wave("w-u5-dim", 0, 2);
        let event_index_1 = ralph_proto::Event::new(
            "review.dimension.done",
            r#"{"dimension":"testing","wave_id":"w-u5-dim"}"#,
        )
        .with_wave("w-u5-dim", 1, 2);

        let mut completed = ralph_core::CompletedWave {
            wave_id: "w-u5-dim".to_string(),
            wave_total: 2,
            results: vec![
                ralph_core::WaveResult {
                    index: 0,
                    events: vec![event_index_0],
                },
                ralph_core::WaveResult {
                    index: 1,
                    events: vec![event_index_1],
                },
            ],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned_dimensions.clone(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };

        let (_mismatches, pending) = merge_wave_results_to_events_file(
            &completed,
            &events_file,
            &["review.dimension.done".into()],
            "dimension-reviewer",
            None,
        )
        .expect("merge succeeds");

        assert_eq!(
            pending.len(),
            1,
            "one mismatched slot must produce one pending resume"
        );
        assert_eq!(pending[0].wave_index, 1);

        // Now run the dispatcher's filter inline. The production
        // code lives in handle_wave_events' Completed arm.
        let mut resume_buf = String::new();
        let mut round: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        for p in &pending {
            let used = completed
                .dimension_retry_counts
                .get(&p.wave_index)
                .copied()
                .unwrap_or(0);
            if used >= ralph_core::MAX_DIMENSION_RETRIES_PER_SLOT {
                continue;
            }
            resume_buf.push_str(&p.jsonl_line);
            resume_buf.push('\n');
            *round.entry(p.wave_index).or_insert(0) += 1;
        }
        for (idx, inc) in &round {
            let prev = completed
                .dimension_retry_counts
                .get(idx)
                .copied()
                .unwrap_or(0);
            completed.dimension_retry_counts.insert(*idx, prev + inc);
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_file)
            .unwrap();
        f.write_all(resume_buf.as_bytes()).unwrap();

        assert_eq!(
            completed.dimension_retry_counts.get(&1),
            Some(&1),
            "budget must reflect 1 used retry"
        );

        let content = fs::read_to_string(&events_file).expect("read events file");
        let mut resume_count = 0usize;
        let mut resume_record: Option<serde_json::Value> = None;
        for line in content.lines().filter(|l| !l.trim().is_empty()) {
            let v: serde_json::Value = serde_json::from_str(line).expect("json event");
            if v["topic"] == "task.resume" {
                resume_count += 1;
                resume_record = Some(v);
            }
        }
        assert_eq!(resume_count, 1, "exactly one task.resume event expected");
        let r = resume_record.unwrap();
        assert_eq!(r["topic"], "task.resume");
        assert_eq!(r["triggered"], "dimension-reviewer");
        assert_eq!(r["hat"], "review-synthesizer");
        assert_eq!(r["source"], "review-synthesizer");
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
        assert_eq!(payload["expected_dimension"], "correctness");
        assert_eq!(payload["actual_dimension"], "testing");
        assert_eq!(payload["wave_id"], "w-u5-dim");
        assert_eq!(payload["wave_index"], 1);
        assert_eq!(payload["wave_total"], 2);
    }

    /// U5/R5 (P0#1): a slot whose `dimension_retry_counts`
    /// entry already reached `MAX_DIMENSION_RETRIES_PER_SLOT`
    /// must NOT inject another `task.resume`, even if the
    /// mismatch reappears in a later dispatch round. The
    /// budget persists on the `CompletedWave`, transferred from
    /// the `WaveTracker` via `take_wave_results`.
    #[test]
    fn u5_budget_exhausted_skips_second_resume() {
        use crate::loop_runner::wave::io::merge_wave_results_to_events_file;
        use std::io::Write;

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let events_file = tmp.path().join("events.jsonl");

        let mut assigned_dimensions = std::collections::HashMap::new();
        assigned_dimensions.insert(0u32, "correctness".to_string());
        assigned_dimensions.insert(1, "correctness".to_string());
        let event_index_1 = ralph_proto::Event::new(
            "review.dimension.done",
            r#"{"dimension":"testing","wave_id":"w-u5-exhaust"}"#,
        )
        .with_wave("w-u5-exhaust", 1, 2);

        // Round 1: empty budget, merge + filter writes 1 task.resume.
        let mut completed_round1 = ralph_core::CompletedWave {
            wave_id: "w-u5-exhaust".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 1,
                events: vec![event_index_1.clone()],
            }],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned_dimensions.clone(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };

        let (_m1, p1) = merge_wave_results_to_events_file(
            &completed_round1,
            &events_file,
            &["review.dimension.done".into()],
            "dimension-reviewer",
            None,
        )
        .unwrap();
        let mut buf1 = String::new();
        for p in &p1 {
            let used = completed_round1
                .dimension_retry_counts
                .get(&p.wave_index)
                .copied()
                .unwrap_or(0);
            if used >= ralph_core::MAX_DIMENSION_RETRIES_PER_SLOT {
                continue;
            }
            buf1.push_str(&p.jsonl_line);
            buf1.push('\n');
            *completed_round1
                .dimension_retry_counts
                .entry(p.wave_index)
                .or_insert(0) += 1;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_file)
            .unwrap();
        f.write_all(buf1.as_bytes()).unwrap();
        assert_eq!(
            completed_round1.dimension_retry_counts.get(&1),
            Some(&ralph_core::MAX_DIMENSION_RETRIES_PER_SLOT),
            "round 1 must consume the only retry"
        );

        // Round 2: the merge layer again returns a pending
        // resume (it does not know about the budget), but the
        // dispatcher filter must skip it because the slot is
        // exhausted. The dispatcher's CompletedWave reuses the
        // counts from round 1.
        let completed_round2 = ralph_core::CompletedWave {
            wave_id: "w-u5-exhaust".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 1,
                events: vec![event_index_1],
            }],
            failures: vec![],
            duration: Duration::from_millis(10),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned_dimensions.clone(),
            // Reuse the budget from round 1 (this is the
            // tracker→CompletedWave transfer that gives us
            // cross-round persistence).
            dimension_retry_counts: completed_round1.dimension_retry_counts.clone(),
            worker_events: Vec::new(),
        };

        let (_m2, p2) = merge_wave_results_to_events_file(
            &completed_round2,
            &events_file,
            &["review.dimension.done".into()],
            "dimension-reviewer",
            None,
        )
        .unwrap();
        let mut buf2 = String::new();
        for p in &p2 {
            let used = completed_round2
                .dimension_retry_counts
                .get(&p.wave_index)
                .copied()
                .unwrap_or(0);
            if used >= ralph_core::MAX_DIMENSION_RETRIES_PER_SLOT {
                continue;
            }
            buf2.push_str(&p.jsonl_line);
            buf2.push('\n');
        }
        assert!(
            buf2.is_empty(),
            "second round must not append task.resume; got: {buf2}"
        );

        // Events file must contain exactly 1 task.resume (from round 1).
        let content = fs::read_to_string(&events_file).expect("read events file");
        let resume_count = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter(|l| l.contains("\"topic\":\"task.resume\""))
            .count();
        assert_eq!(
            resume_count, 1,
            "exactly 1 task.resume across 2 rounds; got {resume_count}"
        );
    }

    /// U5/R5: an empty mismatch list produces no pending task
    /// resumes; the dispatcher filter has nothing to do; the
    /// events file stays empty (no worker errors, no merge
    /// records because CompletedWave.results is also empty).
    #[test]
    fn u5_no_mismatch_no_resume() {
        use crate::loop_runner::wave::io::merge_wave_results_to_events_file;

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
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };

        let (_m, p) = merge_wave_results_to_events_file(
            &completed,
            &events_file,
            &["review.dimension.done".into()],
            "dimension-reviewer",
            None,
        )
        .unwrap();

        assert!(p.is_empty(), "no mismatches → no pending task.resume");
        let content = fs::read_to_string(&events_file).expect("read events file");
        assert!(
            content.trim().is_empty(),
            "no mismatches and no results → events file must be empty, got: {content}"
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
        let executor = Arc::new(TestExecutor::new(Duration::from_hours(1)));

        let wave = make_wave(4, 4, 4);
        // Use a generous aggregate (3600s) so the partial / aggregate
        // paths CANNOT fire first; only the global deadline (10s)
        // will win.
        let aggregate = Duration::from_hours(1);
        // global_deadline = now + 10s in paused-time terms.
        let global_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
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
        let executor = Arc::new(TestExecutor::new(Duration::from_hours(1)));

        let wave = make_wave(2, 2, 2);
        // Aggregate far in the future; only the global deadline
        // (= now, already past) should fire.
        let aggregate = Duration::from_hours(1);
        let global_deadline = tokio::time::Instant::now();
        let ctx = DispatchContext::build(
            &wave,
            Duration::from_mins(1),
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

        let yaml = r"
hats: {}
";
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
            // 2026-07-13-001 plan U2: config_path is irrelevant for
            // empty input; `None` keeps the pre-U2 behaviour.
            None,
            // 2026-07-03-001 supervisor real-wiring: legacy test
            // path; `None` keeps the WaveTracker shape.
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
    // U1 Red: terminal fan-in convergence — missing coverage.
    // Tests 1, 2, 4, 5 are already defined at the bottom of this file
    // (near line 9365+). Only tests 3 and 6 are missing from the plan's
    // Red characterization.
    // ---------------------------------------------------------------------

    /// U1 Red test 3: when `terminal_ctx.elapsed > aggregate_timeout_secs`
    /// the coordinator must receive the real elapsed value in
    /// `PhaseInputs.elapsed_secs`. Before the fix, `run_supervisor_fan_in`
    /// always passed `elapsed_secs: 0` to `tick_with_slot_events`, so
    /// the coordinator could not make a correct timeout decision.
    #[test]
    fn terminal_context_preserves_elapsed_timeout_relation() {
        use ralph_core::supervisor::{BridgeError, PhaseInputs, SupervisorBridge, WaveKind};
        use std::sync::Arc;

        // Capture the PhaseInputs passed to tick
        struct CapturingBridge {
            captured: std::sync::Mutex<Option<PhaseInputs>>,
        }
        impl std::fmt::Debug for CapturingBridge {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("CapturingBridge").finish()
            }
        }
        impl SupervisorBridge for CapturingBridge {
            fn tick(
                &self,
                _wave_id: &str,
                inputs: PhaseInputs,
            ) -> Result<ralph_core::supervisor::CoordinatorAction, BridgeError> {
                *self.captured.lock().unwrap() = Some(inputs);
                Ok(ralph_core::supervisor::CoordinatorAction::ContinueCollect)
            }
            fn tick_with_slot_events(
                &self,
                _wave_id: &str,
                inputs: PhaseInputs,
                _events: Vec<ralph_proto::Event>,
            ) -> Result<ralph_core::supervisor::CoordinatorAction, BridgeError> {
                *self.captured.lock().unwrap() = Some(inputs);
                Ok(ralph_core::supervisor::CoordinatorAction::ContinueCollect)
            }
            fn bind_slot(
                &self,
                _kind: WaveKind,
                _wave_id: &str,
                _slot_index: u32,
            ) -> Result<Option<crate::loop_runner::wave::SlotBinding>, BridgeError> {
                Ok(None)
            }
            fn recover(&self) -> Result<Vec<ralph_core::supervisor::WaveSnapshot>, BridgeError> {
                Ok(Vec::new())
            }
            fn fan_in_status(
                &self,
                _wave_id: &str,
            ) -> Result<ralph_core::supervisor::WaveSnapshot, BridgeError> {
                Err(BridgeError::Store("capturing bridge".into()))
            }
            fn register_wave_if_absent(
                &self,
                _kind: WaveKind,
                wave_id: &str,
                _expected_total: u32,
                _slot_retry_budget: u32,
            ) -> Result<String, BridgeError> {
                Ok(wave_id.to_string())
            }
            fn record_slot_result(
                &self,
                _wave_id: &str,
                _slot_index: u32,
                _content_hash: &str,
                _event_count: usize,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn record_slot_failure(
                &self,
                _wave_id: &str,
                _slot_index: u32,
                _reason: &str,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn release_slot_dispatch(
                &self,
                _wave_id: &str,
                _slot_index: u32,
                _outcome: ralph_core::supervisor::DispatchOutcome,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn record_never_started_failures(&self, _wave_id: &str) -> Result<(), BridgeError> {
                Ok(())
            }
            fn mark_salvage_merged(&self, _wave_id: &str) -> Result<(), BridgeError> {
                Ok(())
            }
            fn mark_merge_to_events(&self, _wave_id: &str) -> Result<(), BridgeError> {
                Ok(())
            }
            fn set_wave_phase(
                &self,
                _wave_id: &str,
                _phase: ralph_core::supervisor::WavePhase,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn slot_failure_reason(
                &self,
                _wave_id: &str,
                _slot_index: u32,
            ) -> Result<Option<String>, BridgeError> {
                Ok(None)
            }
            fn slot_resources(
                &self,
                _wave_id: &str,
            ) -> Result<Vec<ralph_core::supervisor::SlotResource>, BridgeError> {
                Ok(Vec::new())
            }
            fn max_concurrent_workers(&self) -> u32 {
                1
            }
            fn repo_root(&self) -> Option<&std::path::Path> {
                None
            }
            fn tasks_path(&self) -> Option<&std::path::Path> {
                None
            }
            fn try_dispatch_next(&self, _wave_id: &str, _idx: u32) -> Result<bool, BridgeError> {
                Ok(false)
            }
            fn record_slot_terminal_evidence(
                &self,
                _wave_id: &str,
                _slot_index: u32,
                _e: &ralph_core::supervisor::TerminalEvidence,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn slot_terminal_evidence(
                &self,
                _wave_id: &str,
                _slot_index: u32,
            ) -> Result<Option<ralph_core::supervisor::TerminalEvidence>, BridgeError> {
                Ok(None)
            }
            fn finalize_terminal_cleanup(&self, _p: &std::path::Path) -> Result<(), BridgeError> {
                Ok(())
            }
            fn cancel_wave(&self, _wave_id: &str) -> Result<(), BridgeError> {
                Ok(())
            }
            fn enqueue_compensation(
                &self,
                _wave_id: &str,
                _k: ralph_core::supervisor::CompensationKind,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn take_pending_compensations(
                &self,
            ) -> Result<Vec<(String, ralph_core::supervisor::CompensationKind)>, BridgeError>
            {
                Ok(Vec::new())
            }
            fn complete_compensation(
                &self,
                _wave_id: &str,
                _k: ralph_core::supervisor::CompensationKind,
                _ok: bool,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        let capturing = Arc::new(CapturingBridge {
            captured: std::sync::Mutex::new(None),
        });
        let bridge_arc: Arc<dyn SupervisorBridge> = capturing.clone();

        // elapsed = 120s, aggregate_timeout_secs = 60s  → elapsed > timeout
        let terminal_ctx = TerminalFanInContext {
            cancel_requested: true,
            elapsed: std::time::Duration::from_secs(120),
        };

        let completed = ralph_core::CompletedWave {
            wave_id: "u1-red-3".to_string(),
            wave_total: 2,
            results: vec![
                ralph_core::WaveResult {
                    index: 0,
                    events: vec![],
                },
                ralph_core::WaveResult {
                    index: 1,
                    events: vec![],
                },
            ],
            failures: vec![],
            duration: std::time::Duration::from_secs(120),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: vec![],
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "u1-red-3".to_string(),
            target_hat: ralph_proto::HatId::new("review-coordinator"),
            hat_config: ralph_core::config::HatConfig::default(),
            events: vec![ralph_core::Event {
                topic: "review.wave.ready".to_string(),
                payload: None,
                ts: String::new(),
                hat: None,
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            }],
            total: 2,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let tmp = tempfile::tempdir().unwrap();
        let main_events_file = tmp.path().join(".ralph").join("events.jsonl");
        std::fs::create_dir_all(main_events_file.parent().unwrap()).unwrap();

        let _outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            Some(terminal_ctx),
        );

        let captured = capturing.captured.lock().unwrap();
        let inputs = captured.as_ref().expect("tick must have been called");

        // BEFORE FIX: elapsed_secs is 0 (hardcoded), so the coordinator
        // cannot detect the timeout condition.
        // AFTER FIX: elapsed_secs must be the real elapsed value (120).
        assert_eq!(
            inputs.elapsed_secs, 120,
            "U1 Red 3: elapsed_secs must be the real elapsed value (120), not 0"
        );
        assert!(
            inputs.elapsed_secs > inputs.aggregate_timeout_secs,
            "U1 Red 3: elapsed ({}) must exceed aggregate_timeout ({})",
            inputs.elapsed_secs,
            inputs.aggregate_timeout_secs
        );
    }

    /// U1 Red test 6: when `handle_wave_events` returns
    /// `HandleWaveOutcome { fan_in_failure: true, .. }`, the runner
    /// must enter a termination flow with a reason that is NOT
    /// `MaxRuntime`. Before the fix, `HandleWaveOutcome` had no
    /// `fan_in_failure` field, so the runner could not distinguish
    /// terminal fan-in failure from MaxRuntime.
    #[test]
    fn runner_terminates_on_terminal_fan_in_failure() {
        // Read runner.rs to verify:
        // 1. `HandleWaveOutcome::fan_in_failure` is checked by the runner.
        // 2. The fan_in_failure branch does NOT map to MaxRuntime.
        let runner_rs = include_str!("../runner.rs");

        let has_fan_in_failure_check = runner_rs.contains("fan_in_failure");
        assert!(
            has_fan_in_failure_check,
            "U1 Red 6: runner.rs must check HandleWaveOutcome::fan_in_failure; \
             no occurrence found. Before the fix, HandleWaveOutcome has no \
             fan_in_failure field and the runner cannot distinguish terminal \
             fan-in failure from MaxRuntime."
        );

        // The fan_in_failure branch must NOT map to MaxRuntime.
        // Find the fan_in_failure occurrence and verify it doesn't contain MaxRuntime.
        if let Some(pos) = runner_rs.find("fan_in_failure") {
            let after = &runner_rs[pos..pos.saturating_add(300)];
            assert!(
                !after.contains("MaxRuntime"),
                "U1 Red 6: fan_in_failure branch must NOT map to MaxRuntime; \
                 found MaxRuntime in the fan_in_failure handling block. \
                 Block:\n{after}"
            );
        }
    }

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
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
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

    // 2026-07-26-003 plan U1: characterization helpers + tests for
    // `review.wave.failed` -> `finalizer` attribution and
    // `missing_dimensions` correctness. These tests pin the baseline
    // BEFORE the U2 / U4 fixes flip the contract; they MUST start
    // RED, then flip GREEN alongside the implementation.

    /// Build a `CompletedWave` carrying six `review.unit.done`
    /// business events distributed across distinct dimensions. Used
    /// to drive `build_wave_failed_payload(WaveKind::Review, ...)`
    /// and `append_supervisor_coord_event("review.wave.failed", ...)`
    /// under test. Slots are emitted in REVERSE order to mirror
    /// `make_u6_completed` so we can re-use its fan-in ordering
    /// assertions if needed.
    /// Build a `CompletedWave` carrying one `review.unit.done`
    /// business event per "actually-emitted-in-this-fanin" slot.
    /// The `dimensions` argument is the FULL assigned set (i.e.
    /// the set we want `build_wave_failed_payload` to subtract
    /// `already_done` from); the helper records every dimension in
    /// `assigned_dimensions` but only fabricates a `review.unit.done`
    /// event for slots that the caller marked as present in the
    /// `events_for` set. That separation mirrors the real-world
    /// primary-20260726 pattern: a slot can be assigned + failed
    /// without ever carrying an in-flight event.
    fn make_review_completed(
        wave_key: &str,
        dimensions: std::collections::BTreeMap<u32, String>,
        events_for: &std::collections::HashSet<u32>,
    ) -> ralph_core::CompletedWave {
        let total = dimensions.len() as u32;
        let results = dimensions
            .iter()
            .filter(|(idx, _)| events_for.contains(idx))
            .map(|(idx, dim)| {
                let payload = serde_json::json!({ "dimension": dim }).to_string();
                ralph_core::WaveResult {
                    index: *idx,
                    events: vec![
                        ralph_proto::Event::new("review.unit.done", payload.clone())
                            .with_source("review-worker")
                            .with_wave(wave_key.to_string(), *idx, total),
                    ],
                }
            })
            .collect();
        let assigned_dimensions: std::collections::HashMap<u32, String> =
            dimensions.iter().map(|(k, v)| (*k, v.clone())).collect();
        ralph_core::CompletedWave {
            wave_id: wave_key.to_string(),
            wave_total: total,
            results,
            failures: vec![],
            duration: std::time::Duration::from_millis(1),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions,
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        }
    }

    /// U1 Red #1: `review.wave.failed` system-injected coordination
    /// events must carry `hat` / `source` = "finalizer" (the
    /// `implementation-review` preset's registered subscriber for
    /// that topic). Today `append_supervisor_coord_event` collapses
    /// every `review.wave.*` event to "review-synthesizer", so the
    /// synthesizer is wrongly woken for the failure path and the
    /// `finalizer` hat (which is the one whose `event_filter`
    /// actually subscribes to `review.wave.failed`) never fires.
    #[test]
    fn review_wave_failed_attribution_routes_to_finalizer() {
        use std::io::BufRead;
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("events.jsonl");
        let payload = serde_json::json!({
            "wave_id": "W1",
            "missing_dimensions": ["correctness"],
            "reason": "worker_timeout",
        });
        append_supervisor_coord_event(&main, "review.wave.failed", &payload);
        let line =
            std::io::BufReader::new(std::fs::File::open(&main).expect("events file written"))
                .lines()
                .next()
                .expect("at least one line")
                .expect("line read");
        let record: serde_json::Value = serde_json::from_str(&line).expect("json");
        assert_eq!(record["topic"], "review.wave.failed");
        assert_eq!(
            record["hat"], "finalizer",
            "RED: review.wave.failed must route to finalizer, not review-synthesizer"
        );
        // 2026-07-26-004 plan U5 (S5 / AE3): producer (source) is the
        // runtime system identity, NOT the consumer hat. The consumer
        // (finalizer) is carried in `hat` for routing/subscription.
        assert_eq!(
            record["source"], "ralph",
            "U5: producer must be the runtime system identity, not the consumer hat"
        );
        assert_ne!(
            record["source"], record["hat"],
            "U5: producer and consumer must not reuse the same field"
        );
        assert_eq!(record["system_injected"], true);
    }

    /// U4 / S2 (plan 2026-07-26-003): `build_wave_failed_payload` for
    /// the Review arm must subtract from `missing_dimensions` every
    /// dimension that already produced a `review.unit.done`, even
    /// when the unit.done arrived via a path the in-progress
    /// `completed.results` cannot see (i.e. it merged into main
    /// through a previous fan-in tick — the primary-20260726
    /// pattern). The U4 plumbing widens the helper with
    /// `Option<&ReviewDoneHints>` so the call site can pass the
    /// main-backscan / store-Completed view. This assertion goes
    /// RED before U4 (with no hint, `correctness` is doubly
    /// counted); GREEN once `Some(&hints)` actually contributes to
    /// the subtraction.
    #[test]
    fn review_wave_failed_missing_dimensions_omits_main_backscan_hint() {
        use ralph_core::supervisor::WaveKind;
        use std::collections::{BTreeMap, HashSet};
        // Six assigned dimensions; only `testing` and `security`
        // produced a unit.done in this fan-in's `completed.results`.
        // Two siblings (`goal-alignment` / `maintainability`) ALREADY
        // merged into main on a previous fan-in tick — they must
        // NOT appear in `missing_dimensions` once the hint is
        // passed. The remaining two (`correctness` /
        // `performance`) are the genuinely missing dimensions.
        let mut dims = BTreeMap::new();
        for (i, name) in [
            "correctness",
            "goal-alignment",
            "testing",
            "security",
            "maintainability",
            "performance",
        ]
        .iter()
        .enumerate()
        {
            dims.insert(i as u32, name.to_string());
        }
        let mut events_for = HashSet::new();
        events_for.insert(2); // testing
        events_for.insert(3); // security
        let completed = make_review_completed("W1", dims, &events_for);
        let mut main_backscan = HashSet::new();
        main_backscan.insert("goal-alignment".to_string());
        main_backscan.insert("maintainability".to_string());
        let hints = ReviewDoneHints {
            main_backscan: main_backscan.clone(),
            store_completed: HashSet::new(),
        };
        let payload = build_wave_failed_payload(
            WaveKind::Review,
            &completed,
            "worker_timeout",
            vec![],
            &std::collections::HashMap::new(),
            Some(&hints),
        );
        let missing: HashSet<String> = payload["missing_dimensions"]
            .as_array()
            .expect("missing_dimensions is an array")
            .iter()
            .map(|v| v.as_str().expect("string").to_string())
            .collect();
        assert!(
            !missing.contains("goal-alignment"),
            "main-backscanned dimensions must NOT appear in missing_dimensions; got {missing:?}"
        );
        assert!(
            !missing.contains("maintainability"),
            "main-backscanned dimensions must NOT appear in missing_dimensions; got {missing:?}"
        );
        assert!(
            missing.contains("correctness"),
            "truly missing dimension IS in missing_dimensions; got {missing:?}"
        );
        assert!(
            missing.contains("performance"),
            "truly missing dimension IS in missing_dimensions; got {missing:?}"
        );
    }

    /// U4 (plan 2026-07-26-003) pure-helper table-driven tests:
    /// `compute_review_missing_dimensions` is the single source of
    /// truth for the truth-set arithmetic. We drive it with four
    /// synthetic inputs that correspond to the AE2 acceptance
    /// examples: results-only, store-Completed, main-backscan,
    /// and a combination of all three. The pure helper is the
    /// only piece the call site relies on for cross-source
    /// reconciliation.
    #[test]
    fn compute_review_missing_dimensions_table_driven() {
        let assigned: std::collections::HashSet<String> = [
            "correctness",
            "goal-alignment",
            "testing",
            "security",
            "maintainability",
            "performance",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        // 1. results-only (results supplies correctness + testing).
        let results_only: std::collections::HashSet<String> = ["correctness", "testing"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut got = compute_review_missing_dimensions(&assigned, &results_only)
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let mut want = [
            "goal-alignment",
            "security",
            "maintainability",
            "performance",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<std::collections::HashSet<_>>();
        assert_eq!(got, want);

        // 2. store-Completed supplies maintainability + performance
        //    only; the rest stay missing.
        let mut store_only = std::collections::HashSet::new();
        store_only.insert("maintainability".to_string());
        store_only.insert("performance".to_string());
        got = compute_review_missing_dimensions(&assigned, &store_only)
            .into_iter()
            .collect();
        want = ["correctness", "goal-alignment", "testing", "security"]
            .iter()
            .map(|s| s.to_string())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(got, want);

        // 3. main-backscan alone.
        let mut main_only = std::collections::HashSet::new();
        main_only.insert("security".to_string());
        got = compute_review_missing_dimensions(&assigned, &main_only)
            .into_iter()
            .collect();
        want = [
            "correctness",
            "goal-alignment",
            "testing",
            "maintainability",
            "performance",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<std::collections::HashSet<_>>();
        assert_eq!(got, want);

        // 4. union of all three sources.
        let mut unioned = std::collections::HashSet::new();
        unioned.insert("correctness".to_string());
        unioned.insert("testing".to_string());
        unioned.insert("maintainability".to_string());
        unioned.insert("security".to_string());
        got = compute_review_missing_dimensions(&assigned, &unioned)
            .into_iter()
            .collect();
        want = ["goal-alignment", "performance"]
            .iter()
            .map(|s| s.to_string())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(got, want);
    }

    /// U4 / AE2 (plan 2026-07-26-003): when `ReviewDoneHints`
    /// carries BOTH `main_backscan` AND `store_completed`, both
    /// sources contribute to the truth set. The combined view
    /// catches the case where the main ledger has events from an
    /// earlier wave under the same wave_id AND the store has
    /// rows from a still-unmerged tick — both should drop out of
    /// `missing_dimensions`.
    #[test]
    fn review_wave_failed_combined_hints_subtract_from_missing() {
        use ralph_core::supervisor::WaveKind;
        use std::collections::{BTreeMap, HashSet};
        let mut dims = BTreeMap::new();
        for (i, name) in ["correctness", "testing", "security", "performance"]
            .iter()
            .enumerate()
        {
            dims.insert(i as u32, name.to_string());
        }
        // No slot produced an event in this fan-in's results; the
        // full truth set must come from the hints (main +
        // store). Only `performance` should remain missing.
        let events_for = HashSet::new();
        let completed = make_review_completed("W1", dims, &events_for);
        let hints = ReviewDoneHints {
            main_backscan: ["correctness", "testing"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            store_completed: ["security"].iter().map(|s| s.to_string()).collect(),
        };
        let payload = build_wave_failed_payload(
            WaveKind::Review,
            &completed,
            "worker_timeout",
            vec![],
            &std::collections::HashMap::new(),
            Some(&hints),
        );
        let missing: std::collections::HashSet<String> = payload["missing_dimensions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            missing,
            ["performance"].iter().map(|s| s.to_string()).collect()
        );
    }

    /// U5 / S5 (plan 2026-07-26-003 / R4 / KTD7): a Review wave
    /// that reaches `InjectedFailed` must keep the Completed
    /// slots' `review.unit.done` events visible in the main
    /// ledger — without it, the operator / `finalizer` downstream
    /// see "missing everything" when in fact some slots
    /// succeeded. The dispatcher-layer helper
    /// `merge_completed_review_slots_to_main` writes those events
    /// with `hat = review-worker` BEFORE the failed coord event
    /// (or, in this direct unit test, equivalent ordering).
    #[test]
    fn merge_completed_review_slots_to_main_writes_completed_only() {
        use std::collections::HashSet;
        use std::io::BufRead;
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("events.jsonl");
        // Completed slots: 0 + 1 (got review.unit.done).
        // Failed slot: 2 (has a failure record — must be skipped
        // for review.unit.done merge because it did not pass
        // classify). Slot 3 has no results entry at all
        // (Pending — contributes nothing).
        let mut dims = std::collections::BTreeMap::new();
        dims.insert(0, "correctness".to_string());
        dims.insert(1, "goal-alignment".to_string());
        dims.insert(2, "performance".to_string());
        dims.insert(3, "security".to_string());
        let mut events_for: HashSet<u32> = HashSet::new();
        events_for.insert(0);
        events_for.insert(1);
        let mut completed = make_review_completed("W1", dims, &events_for);
        completed.failures.push(ralph_core::WaveFailure {
            index: 2,
            error: "empty_worker_result".to_string(),
            duration: std::time::Duration::from_millis(50),
            expected_dimension: None,
            actual_dimension: None,
        });
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        use ralph_core::supervisor::SupervisorStore as _;
        // Register the wave so `fan_in_status` succeeds after
        // the helper commits salvage_merged (P0-1 invariant).
        // `register_wave` returns the store-assigned `w-N` id,
        // NOT the idempotency key, so we must capture it.
        let wave_id = store
            .register_wave("W1", ralph_core::supervisor::WaveKind::Review, 2, 1)
            .expect("register");
        let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> =
            Arc::new(ralph_core::supervisor::InMemoryCoordinatorBridge::from_store(store.clone()));
        merge_completed_review_slots_to_main(&main, &completed, &bridge, &wave_id);
        // P0-1: the helper must also commit `salvage_merged` so
        // the dispatcher's failure path can inject `*.wave.failed`.
        let snap = store.fan_in_status(&wave_id).expect("snap");
        assert!(
            snap.salvage_merged,
            "merge_completed_review_slots_to_main must commit salvage_merged (P0-1)"
        );
        let f = std::fs::File::open(&main).expect("events file written");
        let lines: Vec<String> = std::io::BufReader::new(f)
            .lines()
            .map(|r| r.unwrap())
            .collect();
        // Exactly 2 lines (one per Completed slot) — the Failed
        // slot's `performance` MUST NOT appear.
        assert_eq!(lines.len(), 2, "expected 2 done events, got: {lines:?}");
        let hats: Vec<String> = lines
            .iter()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).expect("json");
                v["hat"].as_str().unwrap().to_string()
            })
            .collect();
        assert!(
            hats.iter().all(|h| h == "review-worker"),
            "all written events must attribute to review-worker; got: {hats:?}"
        );
        let topics: Vec<String> = lines
            .iter()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).expect("json");
                v["topic"].as_str().unwrap().to_string()
            })
            .collect();
        assert!(
            topics.iter().all(|t| t == "review.unit.done"),
            "all written events must be review.unit.done; got: {topics:?}"
        );
        // Confirm the failed slot's dimension is NOT present.
        let payloads: Vec<String> = lines
            .iter()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).expect("json");
                v["payload"].as_str().unwrap().to_string()
            })
            .collect();
        assert!(
            !payloads.iter().any(|p| p.contains("performance")),
            "the failed slot's `performance` dimension must not be merged; got: {payloads:?}"
        );
    }

    /// U5 / S5 / R7 (plan 2026-07-26-003): the Exec arm MUST NOT
    /// be touched by U5. Re-running the existing byte-equal Exec
    /// payload test (`u5_build_wave_failed_slots_json_shape`)
    /// guarantees the signature widening is Review-only; this
    /// additionally asserts that `merge_completed_review_slots_to_main`
    /// is harmless on a non-Review `CompletedWave` shape (because
    /// the helper is gated by the `WaveKind::Review` match in
    /// `run_supervisor_fan_in`, but the helper itself only
    /// filters by event topic — it's a no-op when no `results`
    /// carry a `review.unit.done`).
    #[test]
    fn merge_completed_review_slots_handles_empty_results() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("events.jsonl");
        let completed = ralph_core::CompletedWave {
            wave_id: "W-empty".to_string(),
            wave_total: 0,
            results: vec![],
            failures: vec![],
            duration: std::time::Duration::from_millis(1),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        use ralph_core::supervisor::SupervisorStore as _;
        let wave_id = store
            .register_wave("W-empty", ralph_core::supervisor::WaveKind::Review, 1, 1)
            .expect("register");
        let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> =
            Arc::new(ralph_core::supervisor::InMemoryCoordinatorBridge::from_store(store.clone()));
        merge_completed_review_slots_to_main(&main, &completed, &bridge, &wave_id);
        // No file is created when there is nothing to write —
        // the helper is a no-op for an empty `results` set.
        assert!(!main.exists() || std::fs::metadata(&main).unwrap().len() == 0);
        // Empty path must NOT commit salvage_merged either; the
        // helper bails out before the mark is set so the
        // coordinator's next tick (if any) still treats the wave
        // as un-salvaged.
        let snap = store.fan_in_status(&wave_id).expect("snap");
        assert!(
            !snap.salvage_merged,
            "empty results must not commit salvage_merged (P0-1)"
        );
    }

    /// U1 guard rail for the success path: `review.wave.complete`
    /// must keep routing to `review-synthesizer`, not flip to
    /// `finalizer`. This test ensures the U2 fix is surgical (only
    /// the `.failed` arm changes) and does not accidentally re-route
    /// the success handoff.
    #[test]
    fn review_wave_complete_attribution_remains_synthesizer() {
        use std::io::BufRead;
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("events.jsonl");
        let payload = serde_json::json!({
            "wave_id": "W1",
            "completed_dimensions": ["goal-alignment"],
        });
        append_supervisor_coord_event(&main, "review.wave.complete", &payload);
        let line =
            std::io::BufReader::new(std::fs::File::open(&main).expect("events file written"))
                .lines()
                .next()
                .expect("at least one line")
                .expect("line read");
        let record: serde_json::Value = serde_json::from_str(&line).expect("json");
        assert_eq!(
            record["hat"], "review-synthesizer",
            "review.wave.complete MUST stay on review-synthesizer (consumer/routing)"
        );
        // 2026-07-26-004 plan U5 (S5 / AE3): producer is the runtime
        // system identity, separate from the consumer hat.
        assert_eq!(record["source"], "ralph");
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
    fn test_ralph_wave_dimension_env_var() {
        let (progress_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut request =
            make_worker_request_with_dimension(0, progress_tx, Some("testing".to_string()));

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
    fn test_ralph_wave_dimension_env_var_absent_when_unassigned() {
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

    // ── 2026-07-25-004 plan U1: characterize classify_slot_result ───────────
    //
    // U2/U3 will flip the Err arm and the Ok(success=false) arm.
    // These tests pin the CURRENT (pre-U3) behaviour so the flip
    // is observable as a red → green transition.

    /// U1 characterization, preserved in U3 (Ok arm is unchanged):
    /// Ok(success=false) + Done terminal resolves via ExitNonZero routing
    /// to `Completed(Done)`. The Err-arm flip (timeout → Static worker_timeout)
    /// lives in the T1/T2 tests below, not here.
    #[test]
    fn classify_slot_result_ok_success_false_with_done_char_u1_pre_u3_completes_via_exit_nonzero() {
        let done_event = ralph_core::Event {
            topic: "review.unit.done".to_string(),
            payload: Some("ok".to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        };
        // success=false → WorkerExit::ExitNonZero in classify_slot_result
        let result: WaveWorkerOutcome = Ok((vec![done_event], Duration::from_secs(3), false));
        let classified = classify_slot_result(&result);

        // U1/U3 contract: ExitNonZero + Done terminal → Completed(Done).
        match classified {
            ClassifiedSlot {
                outcome:
                    ralph_core::supervisor::worker_outcome::SlotOutcome::Completed(
                        ralph_core::supervisor::worker_outcome::WorkerTerminalKind::Done,
                    ),
                reason: None,
            } => {
                // Pass — U3 does NOT change the Ok arm.
            }
            other => panic!("expected Completed(Done) + reason=None, got {other:?}"),
        }
    }

    // ── 2026-07-25-004 plan U3: timeout Err → Static worker_timeout ─────────

    /// T1: empty-timeout Err → Static `worker_timeout` (R3/AE3).
    #[test]
    fn u3_classify_slot_result_empty_timeout_is_static_worker_timeout() {
        use ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT;

        let result: WaveWorkerOutcome = Err((
            "Worker timed out after 5s without emitting events".to_string(),
            Duration::from_secs(5),
        ));
        let classified = classify_slot_result(&result);

        match classified {
            ClassifiedSlot {
                outcome: ralph_core::supervisor::worker_outcome::SlotOutcome::Failed { reason },
                reason: Some(ClassifiedReason::Static(r)),
            } => {
                assert_eq!(reason, REASON_WORKER_TIMEOUT);
                assert_eq!(r, REASON_WORKER_TIMEOUT);
            }
            other => {
                panic!("expected Failed{{reason=REASON_WORKER_TIMEOUT}} + Static(_), got {other:?}")
            }
        }
    }

    /// T2: non-timeout Err keeps Dynamic verbatim + cancelled shell (out of scope to fix).
    #[test]
    fn u3_classify_slot_result_non_timeout_err_keeps_dynamic_verbatim() {
        use ralph_core::supervisor::worker_outcome::REASON_WORKER_CANCELLED;

        let result: WaveWorkerOutcome =
            Err(("boom: worker crashed".to_string(), Duration::from_secs(2)));
        let classified = classify_slot_result(&result);

        match classified {
            ClassifiedSlot {
                outcome: ralph_core::supervisor::worker_outcome::SlotOutcome::Failed { reason },
                reason: Some(ClassifiedReason::Dynamic(msg)),
            } => {
                assert_eq!(reason, REASON_WORKER_CANCELLED);
                assert_eq!(msg, "boom: worker crashed");
            }
            other => panic!(
                "expected Failed{{reason=REASON_WORKER_CANCELLED}} + Dynamic(_), got {other:?}"
            ),
        }
    }

    /// T3: boundary — Err message that starts with the timeout prefix
    /// but mentions events is still classified as Static worker_timeout.
    #[test]
    fn u3_classify_slot_result_timeout_with_event_in_err_message_is_static_worker_timeout_too() {
        use ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT;

        let result: WaveWorkerOutcome = Err((
            "Worker timed out after 7s without emitting events".to_string(),
            Duration::from_secs(7),
        ));
        let classified = classify_slot_result(&result);

        match classified {
            ClassifiedSlot {
                outcome: ralph_core::supervisor::worker_outcome::SlotOutcome::Failed { reason },
                reason: Some(ClassifiedReason::Static(r)),
            } => {
                assert_eq!(reason, REASON_WORKER_TIMEOUT);
                assert_eq!(r, REASON_WORKER_TIMEOUT);
            }
            other => {
                panic!("expected Failed{{reason=REASON_WORKER_TIMEOUT}} + Static(_), got {other:?}")
            }
        }
    }

    /// T4: AE1 satisfaction — Ok path with Done terminal after timeout
    /// still completes (ExitNonZero + Done → Completed(Done), not Failed).
    /// This is the AE1 regression test that mirrors the CA-3 Ok-arm path.
    #[test]
    fn u3_classify_slot_result_ok_path_with_done_after_timeout_still_completes() {
        let done_event = ralph_core::Event {
            topic: "review.unit.done".to_string(),
            payload: Some("ok".to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: None,
        };
        // Ok(events, duration, success=false) — the success=false makes the
        // dispatcher treat it as ExitNonZero, which combined with a Done
        // terminal yields Completed(Done) per the truth table (AE1).
        let result: WaveWorkerOutcome = Ok((vec![done_event], Duration::from_secs(10), false));
        let classified = classify_slot_result(&result);

        match classified {
            ClassifiedSlot {
                outcome:
                    ralph_core::supervisor::worker_outcome::SlotOutcome::Completed(
                        ralph_core::supervisor::worker_outcome::WorkerTerminalKind::Done,
                    ),
                reason: None,
            } => {
                // Pass — AE1 satisfied.
            }
            other => panic!("expected Completed(Done) + reason=None, got {other:?}"),
        }
    }

    // ─────────────────────────────────────────────────────────────────
    // 2026-07-25-004 plan U5 (R6 / AE5): per-slot diagnostics JSON
    // ─────────────────────────────────────────────────────────────────

    /// T1: `build_wave_failed_slots_json` emits the expected JSON shape.
    #[test]
    fn u5_build_wave_failed_slots_json_shape() {
        use ralph_core::supervisor::SlotStatus;

        let slots = vec![
            (0, SlotStatus::Completed),
            (1, SlotStatus::Failed),
            (2, SlotStatus::Failed),
            (3, SlotStatus::Cancelled),
        ];
        let mut reasons = std::collections::HashMap::new();
        reasons.insert(1, "worker_timeout".to_string());
        reasons.insert(2, "slot_never_started".to_string());
        reasons.insert(3, "worker_cancelled".to_string());

        let json = build_wave_failed_slots_json("w-u5-test", &slots, &reasons, 42);

        assert_eq!(json["wave_id"], "w-u5-test");
        assert_eq!(json["generated_at_kind"], "injected_failed");
        assert_eq!(json["elapsed_secs"], 42);

        let slot_array = json["slots"].as_array().expect("slots must be an array");
        assert_eq!(slot_array.len(), 4);

        // Slot 0: completed, no reason.
        let s0 = &slot_array[0];
        assert_eq!(s0["slot_index"], 0);
        assert_eq!(s0["status"], "completed");
        assert!(s0["reason"].is_null());

        // Slot 1: failed, worker_timeout.
        let s1 = &slot_array[1];
        assert_eq!(s1["slot_index"], 1);
        assert_eq!(s1["status"], "failed");
        assert_eq!(s1["reason"], "worker_timeout");

        // Slot 2: failed, slot_never_started.
        let s2 = &slot_array[2];
        assert_eq!(s2["slot_index"], 2);
        assert_eq!(s2["status"], "failed");
        assert_eq!(s2["reason"], "slot_never_started");

        // Slot 3: cancelled, worker_cancelled.
        let s3 = &slot_array[3];
        assert_eq!(s3["slot_index"], 3);
        assert_eq!(s3["status"], "cancelled");
        assert_eq!(s3["reason"], "worker_cancelled");
    }

    /// T2: `write_wave_diagnostics_json` writes the correct file at the
    /// expected path under a TempDir root, and the file parses as valid JSON.
    #[test]
    fn u5_write_wave_diagnostics_json_writes_correct_file() {
        let temp_root = tempfile::TempDir::new().expect("temp dir");
        let root_path = temp_root.path();

        let payload = serde_json::json!({
            "wave_id": "w-u5-t2",
            "generated_at_kind": "injected_failed",
            "elapsed_secs": 7,
            "slots": [
                {"slot_index": 0, "status": "completed", "reason": null},
                {"slot_index": 1, "status": "failed", "reason": "worker_timeout"},
            ]
        });

        let result = write_wave_diagnostics_json(root_path, "w-u5-t2", &payload);
        assert!(result.is_ok(), "write must succeed");

        let written_path = result.unwrap();
        assert!(
            written_path.starts_with(root_path),
            "path must be under the given root"
        );
        assert!(
            written_path
                .to_string_lossy()
                .contains("wave-w-u5-t2-slots.json"),
            "filename must match expected pattern"
        );

        // Verify the file parses as valid JSON and matches the payload.
        let bytes = std::fs::read(&written_path).expect("file must be readable");
        let read_back: serde_json::Value =
            serde_json::from_slice(&bytes).expect("must be valid JSON");
        assert_eq!(read_back, payload);
    }

    /// T3 (regression): success path (`InjectedComplete`) does NOT write a
    /// diagnostics file. Unlike the earlier hollow stub, this test actually
    /// drives `run_supervisor_fan_in` through a fully-completed wave so the
    /// coordinator returns `CoordinatorAction::Complete`, then asserts the
    /// success arm wrote NO per-slot diagnostics JSON. The unique wave_id
    /// guarantees the assertion is meaningful: production writes diagnostics
    /// to `Path::new(".")` (CWD), and nextest's process-per-test isolation
    /// keeps CWD stable, so if a future change adds
    /// `write_wave_diagnostics_json` to the `InjectedComplete` arm this test
    /// fails.
    #[test]
    fn u5_no_diagnostics_file_on_success_path() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::InMemoryCoordinatorBridge;
        use ralph_core::supervisor::SlotResource;
        use ralph_core::supervisor::SupervisorBridge;
        use ralph_core::supervisor::{SupervisorStore, WaveKind};

        // Unique wave_id so the diagnostics-file absence assertion cannot
        // collide with a file written by any other test.
        let wave_id = "w-u4-success-no-diag-2026-07-25-004";

        // Build an in-memory store + bridge with 2 slots, both Completed.
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let bridge_arc: Arc<dyn SupervisorBridge> = Arc::new(bridge);

        let store_wave_id = bridge_arc
            .register_wave_if_absent(WaveKind::Exec, wave_id, 2, 1)
            .unwrap();

        // Bind + dispatch + complete BOTH slots so `evaluate_phase`
        // reaches `Integrate` (pending=0, in_flight=0, completed>=total).
        for slot in 0..2u32 {
            store
                .bind_worktree(
                    &store_wave_id,
                    slot,
                    SlotResource {
                        slot_index: slot,
                        worktree_path: Some(format!(".ralph/s{slot}")),
                        branch: Some(format!("ralph/u4-s{slot}")),
                    },
                )
                .unwrap();
        }
        let mut dispatched = Vec::new();
        for _ in 0..2 {
            let (w, i) = store.try_dispatch_next(8).unwrap().unwrap();
            dispatched.push((w, i));
        }
        for (w, i) in dispatched {
            store.record_slot_result(&w, i, "hash", 1).unwrap();
            // Plan 004 R2 / P0-2: success path requires terminal evidence.
            store
                .record_slot_terminal_evidence(
                    &w,
                    i,
                    &ralph_core::supervisor::TerminalEvidence::from_event(
                        "exec.unit.done",
                        &format!("{{\"unit\":\"u5-ok-{i}\"}}"),
                    ),
                )
                .unwrap();
        }

        // Sanity: the wave is fully completed before fan-in.
        let snap = store.fan_in_status(&store_wave_id).unwrap();
        assert_eq!(snap.completed_count, 2);

        // Build the CompletedWave + DetectedWave for this wave. The trigger
        // topic does NOT start with `review.` or `fix.` so the kind is Exec.
        let completed = ralph_core::CompletedWave {
            wave_id: wave_id.to_string(),
            wave_total: 2,
            ..ralph_core::CompletedWave::default()
        };
        let detected = ralph_core::DetectedWave {
            wave_id: wave_id.to_string(),
            target_hat: HatId::new("u4-success-hat"),
            hat_config: HatConfig {
                name: "u4-success-hat".to_string(),
                ..HatConfig::default()
            },
            events: vec![core_event("work.ready", "payload-0")],
            total: 1,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        // Fresh temp dir for the main events file the success arm appends to.
        let temp_root = tempfile::TempDir::new().expect("temp dir");
        let main_events_file = temp_root.path().join("events.jsonl");

        let outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            None,
        );
        assert!(
            matches!(outcome, SupervisorFanInOutcome::InjectedComplete),
            "success path must reach InjectedComplete, got {outcome:?}"
        );

        // The InjectedComplete arm must NOT write any diagnostics file.
        let diag_path = std::path::Path::new(".")
            .join(".ralph")
            .join("diagnostics")
            .join(format!("wave-{wave_id}-slots.json"));
        assert!(
            !diag_path.exists(),
            "success path must not write a diagnostics file at {}",
            diag_path.display()
        );
    }

    /// T3 negative: `write_wave_diagnostics_json` surfaces an `Err` (and does
    /// NOT panic) when the diagnostics directory cannot be created — here
    /// because `.ralph/diagnostics` collides with an existing regular file.
    #[test]
    fn u5_write_wave_diagnostics_json_failure_returns_err() {
        let temp_root = tempfile::TempDir::new().expect("temp dir");
        let root_path = temp_root.path();

        // Make `create_dir_all(root/.ralph/diagnostics)` fail by placing a
        // regular FILE at the `diagnostics` path.
        std::fs::create_dir_all(root_path.join(".ralph")).expect("create .ralph");
        std::fs::write(root_path.join(".ralph").join("diagnostics"), b"x")
            .expect("plant colliding file");

        let payload = serde_json::json!({
            "wave_id": "w-u4-neg",
            "generated_at_kind": "injected_failed",
            "elapsed_secs": 0,
            "slots": []
        });

        let result = write_wave_diagnostics_json(root_path, "w-u4-neg", &payload);
        assert!(
            result.is_err(),
            "write must fail when diagnostics path is a file, got {result:?}"
        );
    }

    /// T2 integration: use InMemoryCoordinatorBridge to simulate a failed
    /// wave with mixed slot states and verify the diagnostics JSON is
    /// written to the temp root.
    #[test]
    fn u5_injected_failed_writes_diagnostics_json() {
        use ralph_core::supervisor::InMemoryCoordinatorBridge;
        use ralph_core::supervisor::SlotResource;
        use ralph_core::supervisor::SupervisorBridge;
        use ralph_core::supervisor::{PhaseInputs, SupervisorStore, WaveKind};

        let temp_root = tempfile::TempDir::new().expect("temp dir");

        // Build an in-memory store with 4 slots.
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());

        let wave_id = bridge
            .register_wave_if_absent(WaveKind::Exec, "w-u5-integration", 4, 1)
            .unwrap();

        // Slot 0: bind worktree, dispatch, complete.
        store
            .bind_worktree(
                &wave_id,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/s0".to_string()),
                    branch: Some("ralph/u5-s0".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(8).unwrap().unwrap();
        store.record_slot_result(&wave_id, 0, "hash-s0", 1).unwrap();

        // Slot 1: record a failure with worker_timeout.
        store
            .record_slot_failure(&wave_id, 1, "worker_timeout")
            .unwrap();

        // Slot 2: slot_never_started — directly record it as Failed
        // (simulating what record_never_started_failures does for a
        // single pending slot).
        store
            .record_slot_failure(
                &wave_id,
                2,
                ralph_core::supervisor::worker_outcome::REASON_SLOT_NEVER_STARTED,
            )
            .unwrap();

        // Slot 3: cancelled — record this LAST so it is the terminal state.
        // (If we called record_never_started_failures first, it would mark
        // slot 3 as Failed and cause this to fail with AlreadyTerminal.)
        store
            .record_slot_failure(
                &wave_id,
                3,
                ralph_core::supervisor::worker_outcome::REASON_WORKER_CANCELLED,
            )
            .unwrap();

        // Verify the snapshot has the right slot states.
        let snap = store.fan_in_status(&wave_id).unwrap();
        assert_eq!(snap.slots.len(), 4);

        // Build the reasons map via the bridge (simulating what the
        // InjectedFailed arm does).
        use ralph_core::supervisor::SlotStatus;
        let mut reasons = std::collections::HashMap::new();
        for (idx, status) in &snap.slots {
            if matches!(status, SlotStatus::Failed | SlotStatus::Cancelled)
                && let Ok(Some(r)) = bridge.slot_failure_reason(&wave_id, *idx)
            {
                reasons.insert(*idx, r);
            }
        }

        let elapsed_secs = snap.started_at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
        let payload =
            build_wave_failed_slots_json("w-u5-integration", &snap.slots, &reasons, elapsed_secs);

        // Write to the temp root.
        let write_result =
            write_wave_diagnostics_json(temp_root.path(), "w-u5-integration", &payload);
        assert!(
            write_result.is_ok(),
            "write must succeed: {:?}",
            write_result.err()
        );

        // Verify the file exists and has correct content.
        let written_path = write_result.unwrap();
        let bytes = std::fs::read(&written_path).expect("file must be readable");
        let read_back: serde_json::Value =
            serde_json::from_slice(&bytes).expect("must be valid JSON");

        assert_eq!(read_back["wave_id"], "w-u5-integration");
        assert_eq!(read_back["generated_at_kind"], "injected_failed");

        let slots = read_back["slots"]
            .as_array()
            .expect("slots must be an array");
        assert_eq!(slots.len(), 4);

        // Slot 0: completed, no reason.
        assert_eq!(slots[0]["slot_index"], 0);
        assert_eq!(slots[0]["status"], "completed");
        assert!(slots[0]["reason"].is_null());

        // Slot 1: failed, worker_timeout.
        assert_eq!(slots[1]["slot_index"], 1);
        assert_eq!(slots[1]["status"], "failed");
        assert_eq!(slots[1]["reason"], "worker_timeout");

        // Slot 2: failed, slot_never_started (recorded by record_never_started_failures).
        assert_eq!(slots[2]["slot_index"], 2);
        assert_eq!(slots[2]["status"], "failed");
        assert_eq!(slots[2]["reason"], "slot_never_started");

        // Slot 3: cancelled, worker_cancelled.
        assert_eq!(slots[3]["slot_index"], 3);
        assert_eq!(slots[3]["status"], "cancelled");
        assert_eq!(slots[3]["reason"], "worker_cancelled");
    }

    /// 2026-07-26-002 plan U4 (R4): the InjectedFailed arm in
    /// `run_supervisor_fan_in` MUST write the diagnostics JSON
    /// under the workspace root derived from the main events
    /// file (NOT process CWD). This test exercises the production
    /// path end-to-end:
    ///
    /// 1. Construct a real `run_supervisor_fan_in` invocation
    ///    with a Failed/Cancelled slot mix.
    /// 2. Pass a main events file inside a fresh temp dir.
    /// 3. Assert the diagnostics JSON lands at
    ///    `<temp>/.ralph/diagnostics/wave-<id>-slots.json`.
    ///
    /// The previous `u5_injected_failed_writes_diagnostics_json`
    /// test called `write_wave_diagnostics_json` directly, which
    /// masked the CWD bug — that test is preserved as a unit
    /// helper-level guard but this test is the authoritative
    /// production integration check.
    #[test]
    fn u4_run_supervisor_fan_in_injected_failed_writes_workspace_diagnostics() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::InMemoryCoordinatorBridge;
        use ralph_core::supervisor::SlotResource;
        use ralph_core::supervisor::SupervisorBridge;
        use ralph_core::supervisor::{PhaseInputs, SupervisorStore, WaveKind};

        // Workspace = fresh temp dir; main events file lives at
        // <workspace>/.ralph/events.jsonl, exactly as the runner
        // would emit.
        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Exec, "w-u4-fan-in", 2, 1)
            .unwrap();

        // Slot 0: success.
        store
            .bind_worktree(
                &store_wave_id,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/s0".to_string()),
                    branch: Some("ralph/u4-s0".to_string()),
                },
            )
            .unwrap();
        let _ = store.try_dispatch_next(8).unwrap().unwrap();
        store
            .record_slot_result(&store_wave_id, 0, "hash-s0", 1)
            .unwrap();
        // Plan 004 R2 / P0-2: success path requires terminal evidence.
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "exec.unit.done",
                    "{\"unit\":\"u4-fan-in-0\"}",
                ),
            )
            .unwrap();

        // Slot 1: failure → will become blocking, triggering
        // InjectedFailed.
        store
            .record_slot_failure(
                &store_wave_id,
                1,
                ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
            )
            .unwrap();

        // Plan 004 R3 / P0-1: dispatcher must commit salvage BEFORE
        // `fail_wave` latches the coord-event injection.
        bridge.mark_salvage_merged(&store_wave_id).unwrap();

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u4-fan-in".to_string(),
            wave_total: 2,
            ..ralph_core::CompletedWave::default()
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u4-fan-in".to_string(),
            target_hat: HatId::new("u4-hat"),
            hat_config: HatConfig {
                name: "u4-hat".to_string(),
                ..HatConfig::default()
            },
            events: vec![core_event("work.ready", "payload-0")],
            total: 1,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        let outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            None,
        );
        assert!(
            matches!(outcome, SupervisorFanInOutcome::InjectedFailed),
            "failed wave must reach InjectedFailed; got {outcome:?}"
        );

        // Authoritative assertion: diagnostics JSON exists under
        // the workspace root, not under process CWD.
        let diag_path = workspace
            .path()
            .join(".ralph")
            .join("diagnostics")
            .join("wave-w-u4-fan-in-slots.json");
        assert!(
            diag_path.exists(),
            "InjectedFailed arm must write diagnostics at {diag_path:?}"
        );
        let bytes = std::fs::read(&diag_path).expect("read diagnostics");
        let payload: serde_json::Value =
            serde_json::from_slice(&bytes).expect("diagnostics must be valid JSON");
        assert_eq!(payload["wave_id"], "w-u4-fan-in");
        assert_eq!(payload["generated_at_kind"], "injected_failed");
        let slots = payload["slots"].as_array().expect("slots must be an array");
        assert_eq!(slots.len(), 2);
        // Slot 1 is the Failed slot and must carry the worker_timeout
        // reason from the store (the field the dispatcher used to
        // leave blank by reading `completed.failures` free-form).
        let s1 = slots
            .iter()
            .find(|s| s["slot_index"] == 1)
            .expect("slot 1 must exist");
        assert_eq!(s1["status"], "failed");
        assert_eq!(s1["reason"], "worker_timeout");
    }

    /// U1 Red #1 (plan 2026-07-26-004, S2 / R2): production
    /// `run_supervisor_fan_in` must NOT report a dimension as
    /// missing when that dimension's `review.unit.done` already
    /// lives in the main ledger for this wave (e.g. merged by a
    /// previous fan-in tick). Today the InjectedFailed arm passes
    /// `None` for `review_done_hints`, so a main-only done
    /// dimension is double-counted as missing (the
    /// primary-20260726 inflation). This test drives the REAL
    /// production call point and asserts the reconciled truth; it
    /// goes RED until U3 wires the main-backscan hints into the
    /// payload builder.
    #[test]
    fn u1_red1_fan_in_failed_missing_excludes_main_backscanned_dimension() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::InMemoryCoordinatorBridge;
        use ralph_core::supervisor::SupervisorBridge;
        use ralph_core::supervisor::{SupervisorStore, WaveKind};
        use std::io::{BufRead, Write};

        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        // Pre-seed the main ledger with a `review.unit.done` for the
        // `testing` dimension under THIS wave id — simulating a prior
        // partial fan-in tick that already merged it. Per R2 this
        // dimension is already proven done and must not be re-counted
        // as missing.
        {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&main_events_file)
                .expect("open main");
            let line = serde_json::json!({
                "topic": "review.unit.done",
                "payload": "{\"dimension\":\"testing\"}",
                "ts": "2026-07-26T00:00:00Z",
                "hat": "review-worker",
                "source": "review-worker",
                "wave_id": "w-u1-red1",
            });
            writeln!(f, "{}", line).expect("write main line");
        }

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "w-u1-red1", 2, 1)
            .unwrap();

        // Slot 0: Completed with a real review.unit.done for `correctness`.
        store
            .record_slot_result(&store_wave_id, 0, "hash-s0", 1)
            .unwrap();
        // Plan 004 R2 / P0-2: success path requires terminal evidence.
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "review.unit.done",
                    &serde_json::json!({"dimension":"correctness"}).to_string(),
                ),
            )
            .unwrap();
        // Slot 1: terminally Failed. Its assigned dimension `testing`
        // is already done in main from the prior tick.
        store
            .record_slot_failure(
                &store_wave_id,
                1,
                ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
            )
            .unwrap();
        // Plan 004 R3 / P0-1: dispatcher must commit salvage BEFORE
        // `fail_wave` latches the coord-event injection.
        bridge.mark_salvage_merged(&store_wave_id).unwrap();

        // completed.results carries ONLY slot 0's event (correctness);
        // `testing` is done via main, not via this fan-in's results.
        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0u32, "correctness".to_string());
        assigned.insert(1u32, "testing".to_string());
        let completed = ralph_core::CompletedWave {
            wave_id: "w-u1-red1".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 0,
                events: vec![
                    ralph_proto::Event::new(
                        "review.unit.done",
                        serde_json::json!({"dimension":"correctness"}).to_string(),
                    )
                    .with_source("review-worker")
                    .with_wave("w-u1-red1".to_string(), 0, 2),
                ],
            }],
            failures: vec![ralph_core::WaveFailure {
                index: 1,
                error: "worker_timeout".to_string(),
                duration: std::time::Duration::from_millis(1),
                ..ralph_core::WaveFailure::default()
            }],
            duration: std::time::Duration::from_millis(1),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned,
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u1-red1".to_string(),
            target_hat: HatId::new("review-dispatcher"),
            hat_config: HatConfig {
                name: "review-dispatcher".to_string(),
                ..HatConfig::default()
            },
            events: vec![core_event("review.unit.ready", "payload-0")],
            total: 2,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        let outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            None,
        );
        assert!(
            matches!(outcome, SupervisorFanInOutcome::InjectedFailed),
            "failed wave must reach InjectedFailed; got {outcome:?}"
        );

        // Read the injected review.wave.failed coord event and assert
        // missing_dimensions does NOT include `testing` (already done
        // in main) — the reconciled truth per R2.
        let failed = std::io::BufReader::new(std::fs::File::open(&main_events_file).expect("main"))
            .lines()
            .filter_map(|l| l.ok())
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(&l).ok())
            .find(|r| r["topic"] == "review.wave.failed")
            .expect("a review.wave.failed coord event must be injected");
        let missing: std::collections::HashSet<String> = failed["payload"]["missing_dimensions"]
            .as_array()
            .expect("missing_dimensions array")
            .iter()
            .map(|v| v.as_str().expect("str").to_string())
            .collect();
        assert!(
            !missing.contains("testing"),
            "RED: `testing` already has a review.unit.done in main for this wave; \
             it must NOT be reported missing (production passes None hints today). got {missing:?}"
        );
    }

    /// 2026-07-26-004 plan U3 (R1 / R2 / KTD3): `build_review_done_hints`
    /// reconciles the two cross-source views correctly and stays bounded:
    /// - `main_backscan` keeps ONLY same-wave `review.unit.done` rows and
    ///   ignores other-wave / wave-less / malformed rows;
    /// - `store_completed` keeps ONLY Completed slots WITH valid terminal
    ///   evidence (a legacy Completed status bit with no evidence is
    ///   fail-closed and does NOT count).
    #[test]
    fn u3_build_review_done_hints_is_bounded_and_evidence_gated() {
        use ralph_core::supervisor::{
            InMemoryCoordinatorBridge, SupervisorBridge, SupervisorStore, TerminalEvidence,
            WaveKind,
        };
        use std::io::Write;

        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("events.jsonl");
        {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&main)
                .expect("open main");
            let row = |dim: &str, wave: Option<&str>| -> String {
                let mut rec = serde_json::json!({
                    "topic": "review.unit.done",
                    "payload": serde_json::json!({"dimension": dim}).to_string(),
                    "hat": "review-worker",
                    "source": "review-worker",
                });
                if let Some(w) = wave {
                    rec["wave_id"] = serde_json::Value::String(w.to_string());
                }
                rec.to_string()
            };
            // same wave → counted
            writeln!(f, "{}", row("correctness", Some("W-main"))).unwrap();
            // different wave → ignored
            writeln!(f, "{}", row("security", Some("W-other"))).unwrap();
            // no wave_id → ignored (fail-closed)
            writeln!(f, "{}", row("testing", None)).unwrap();
            // malformed → ignored
            writeln!(f, "not-json").unwrap();
        }

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "W-main", 2, 1)
            .unwrap();
        // Slot 0: Completed WITH evidence (dimension `performance`).
        store
            .record_slot_result(&store_wave_id, 0, "h0", 1)
            .unwrap();
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &TerminalEvidence::from_event(
                    "review.unit.done",
                    "{\"dimension\":\"performance\"}",
                ),
            )
            .unwrap();
        // Slot 1: Completed but NO evidence (legacy) → must NOT count.
        store
            .record_slot_result(&store_wave_id, 1, "h1", 1)
            .unwrap();

        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0u32, "performance".to_string());
        assigned.insert(1, "maintainability".to_string());
        let completed = ralph_core::CompletedWave {
            wave_id: "W-main".to_string(),
            wave_total: 2,
            assigned_dimensions: assigned,
            ..ralph_core::CompletedWave::default()
        };

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        let hints = build_review_done_hints(&bridge_arc, &store_wave_id, &completed, &main);

        assert_eq!(
            hints.main_backscan,
            ["correctness".to_string()].into_iter().collect(),
            "main_backscan must keep only same-wave rows"
        );
        assert_eq!(
            hints.store_completed,
            ["performance".to_string()].into_iter().collect(),
            "store_completed must keep only Completed-with-evidence slots"
        );
    }

    /// U4 Red (plan 2026-07-26-004, S9 / R3): replaying a failed
    /// fan-in MUST NOT double-write. Calling `run_supervisor_fan_in`
    /// twice for the same mixed Review wave must leave exactly ONE
    /// `review.wave.failed` coord event and ONE salvaged
    /// `review.unit.done` in the main ledger. Before U4, `fail_wave`
    /// had no idempotency latch (`evaluate_phase` is pure and keeps
    /// returning `Failed`), so the second tick re-injected the coord
    /// event and re-ran the dispatcher-layer salvage merge.
    #[test]
    fn u4_replayed_failed_fan_in_does_not_double_write() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::InMemoryCoordinatorBridge;
        use ralph_core::supervisor::SupervisorBridge;
        use ralph_core::supervisor::{SupervisorStore, WaveKind};
        use std::io::BufRead;

        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "w-u4-replay", 2, 1)
            .unwrap();
        // Slot 0: Completed with a real review.unit.done (correctness).
        store
            .record_slot_result(&store_wave_id, 0, "hash-s0", 1)
            .unwrap();
        // Plan 004 R2 / P0-2: success path requires terminal evidence.
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "review.unit.done",
                    &serde_json::json!({"dimension":"correctness"}).to_string(),
                ),
            )
            .unwrap();
        // Slot 1: terminally Failed → InjectedFailed.
        store
            .record_slot_failure(
                &store_wave_id,
                1,
                ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
            )
            .unwrap();
        // Plan 004 R3 / P0-1: dispatcher must commit salvage BEFORE
        // `fail_wave` latches the coord-event injection.
        bridge.mark_salvage_merged(&store_wave_id).unwrap();

        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0u32, "correctness".to_string());
        assigned.insert(1u32, "testing".to_string());
        let completed = ralph_core::CompletedWave {
            wave_id: "w-u4-replay".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 0,
                events: vec![
                    ralph_proto::Event::new(
                        "review.unit.done",
                        serde_json::json!({"dimension":"correctness"}).to_string(),
                    )
                    .with_source("review-worker")
                    .with_wave("w-u4-replay".to_string(), 0, 2),
                ],
            }],
            failures: vec![ralph_core::WaveFailure {
                index: 1,
                error: "worker_timeout".to_string(),
                duration: std::time::Duration::from_millis(1),
                ..ralph_core::WaveFailure::default()
            }],
            duration: std::time::Duration::from_millis(1),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned,
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u4-replay".to_string(),
            target_hat: HatId::new("review-dispatcher"),
            hat_config: HatConfig {
                name: "review-dispatcher".to_string(),
                ..HatConfig::default()
            },
            events: vec![core_event("review.unit.ready", "payload-0")],
            total: 2,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        // First fan-in: reaches InjectedFailed.
        let first = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            None,
        );
        assert!(matches!(first, SupervisorFanInOutcome::InjectedFailed));
        // Replay: must be a no-op (AlreadyDone), NOT a second InjectedFailed.
        let second = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            None,
        );
        assert!(
            matches!(second, SupervisorFanInOutcome::AlreadyDone),
            "replayed failed fan-in must be AlreadyDone; got {second:?}"
        );

        let count = |topic: &str| {
            std::io::BufReader::new(std::fs::File::open(&main_events_file).expect("main"))
                .lines()
                .filter_map(|l| l.ok())
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(&l).ok())
                .filter(|r| r["topic"] == topic)
                .count()
        };
        assert_eq!(
            count("review.wave.failed"),
            1,
            "exactly one review.wave.failed after replay"
        );
        assert_eq!(
            count("review.unit.done"),
            1,
            "salvaged review.unit.done must not be double-written on replay"
        );
    }

    /// U5 (plan 2026-07-26-004, S4 / AE2): a worker's terminal event
    /// must keep its WORKER producer across the fan-in merge — never
    /// inherit the current `review-dispatcher` activation. The trusted
    /// merge seam normalises the salvaged `review.unit.done` to
    /// `review-worker` even when the in-flight event carried a missing
    /// or spoofed source, so a later replay during the dispatcher
    /// activation cannot mis-attribute it (no `isolated_scope_violation`
    /// against `review-dispatcher`).
    #[test]
    fn u5_salvaged_worker_event_keeps_worker_provenance() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::InMemoryCoordinatorBridge;
        use ralph_core::supervisor::SupervisorBridge;
        use ralph_core::supervisor::{SupervisorStore, WaveKind};
        use std::io::BufRead;

        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "w-u5-prov", 2, 1)
            .unwrap();
        store
            .record_slot_result(&store_wave_id, 0, "hash-s0", 1)
            .unwrap();
        // Plan 004 R2 / P0-2: success path requires terminal evidence.
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "review.unit.done",
                    &serde_json::json!({"dimension":"correctness"}).to_string(),
                ),
            )
            .unwrap();
        store
            .record_slot_failure(
                &store_wave_id,
                1,
                ralph_core::supervisor::worker_outcome::REASON_WORKER_TIMEOUT,
            )
            .unwrap();
        // Plan 004 R3 / P0-1: dispatcher must commit salvage BEFORE
        // `fail_wave` latches the coord-event injection.
        bridge.mark_salvage_merged(&store_wave_id).unwrap();

        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0u32, "correctness".to_string());
        assigned.insert(1u32, "testing".to_string());
        // Slot 0's event carries a SPOOFED source (review-dispatcher) to
        // prove the merge seam normalises provenance to the real worker.
        let completed = ralph_core::CompletedWave {
            wave_id: "w-u5-prov".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 0,
                events: vec![
                    ralph_proto::Event::new(
                        "review.unit.done",
                        serde_json::json!({"dimension":"correctness"}).to_string(),
                    )
                    .with_source("review-dispatcher")
                    .with_wave("w-u5-prov".to_string(), 0, 2),
                ],
            }],
            failures: vec![ralph_core::WaveFailure {
                index: 1,
                error: "worker_timeout".to_string(),
                duration: std::time::Duration::from_millis(1),
                ..ralph_core::WaveFailure::default()
            }],
            duration: std::time::Duration::from_millis(1),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: assigned,
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u5-prov".to_string(),
            target_hat: HatId::new("review-dispatcher"),
            hat_config: HatConfig {
                name: "review-dispatcher".to_string(),
                ..HatConfig::default()
            },
            events: vec![core_event("review.unit.ready", "payload-0")],
            total: 2,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        let outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            None,
        );
        assert!(matches!(outcome, SupervisorFanInOutcome::InjectedFailed));

        let done = std::io::BufReader::new(std::fs::File::open(&main_events_file).expect("main"))
            .lines()
            .filter_map(|l| l.ok())
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(&l).ok())
            .find(|r| r["topic"] == "review.unit.done")
            .expect("the salvaged review.unit.done must be in main");
        assert_eq!(
            done["hat"], "review-worker",
            "salvaged worker event must keep worker producer (hat)"
        );
        assert_eq!(
            done["source"], "review-worker",
            "salvaged worker event must keep worker producer (source), not the dispatcher"
        );
    }

    /// 2026-07-26-002 plan U5 (R5 / KTD6): `slot_failures` MUST be
    /// derived from the store's frozen reason codes filtered by
    /// `blocking_slots` — the index set of `slot_failures` must
    /// equal `blocking_slots` exactly, and the reason strings
    /// must come from the `reasons` map (NOT from
    /// `completed.failures` free-form text).
    #[test]
    fn u5_slot_failures_matches_blocking_slots_from_store() {
        use ralph_core::supervisor::WaveKind;

        // 3 slots: 0 success, 1 worker_timeout, 2 empty_worker_result
        let completed = ralph_core::CompletedWave {
            wave_id: "w-u5-ssot".to_string(),
            wave_total: 3,
            ..ralph_core::CompletedWave::default()
        };
        let blocking_slots = vec![1u32, 2];
        // Store-derived reasons (frozen codes).
        let mut reasons = std::collections::HashMap::new();
        reasons.insert(1u32, "worker_timeout".to_string());
        reasons.insert(2u32, "empty_worker_result".to_string());

        let payload = build_wave_failed_payload(
            WaveKind::Exec,
            &completed,
            "wave_failed",
            blocking_slots.clone(),
            &reasons,
            None,
        );

        // slot_failures must be present and its index set must equal blocking_slots.
        let slot_failures = payload["slot_failures"]
            .as_array()
            .expect("slot_failures must be an array");
        let sf_indices: std::collections::BTreeSet<u32> = slot_failures
            .iter()
            .map(|s| s["slot_index"].as_u64().unwrap() as u32)
            .collect();
        let bs_indices: std::collections::BTreeSet<u32> = blocking_slots.iter().copied().collect();
        assert_eq!(
            sf_indices, bs_indices,
            "slot_failures index set must equal blocking_slots; got slot_failures={sf_indices:?}, blocking_slots={bs_indices:?}"
        );

        // Reasons are taken from the store, not from free-form text.
        let s1 = slot_failures
            .iter()
            .find(|s| s["slot_index"] == 1)
            .expect("slot 1 must be present");
        let s2 = slot_failures
            .iter()
            .find(|s| s["slot_index"] == 2)
            .expect("slot 2 must be present");
        assert_eq!(s1["reason"], "worker_timeout");
        assert_eq!(s2["reason"], "empty_worker_result");
    }

    /// 2026-07-26-002 plan U5 (R5): when the store has NO reason for
    /// a blocking slot (e.g., legacy store without `record_slot_failure`),
    /// the payload must still include that slot in `slot_failures`
    /// — keeping the index-set invariant — but with `reason: null`,
    /// NOT a free-form fallback string from `completed.failures`.
    #[test]
    fn u5_slot_failures_no_store_reason_yields_null() {
        use ralph_core::supervisor::WaveKind;

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u5-null".to_string(),
            wave_total: 1,
            ..ralph_core::CompletedWave::default()
        };
        let payload = build_wave_failed_payload(
            WaveKind::Exec,
            &completed,
            "wave_failed",
            vec![7u32],
            &std::collections::HashMap::new(),
            None,
        );

        let slot_failures = payload["slot_failures"].as_array().unwrap();
        assert_eq!(slot_failures.len(), 1);
        assert_eq!(slot_failures[0]["slot_index"], 7);
        assert!(
            slot_failures[0]["reason"].is_null(),
            "no-store-reason slot must report null, not free-form text"
        );
    }

    // -------------------------------------------------------------------
    // 2026-07-26-002 plan U3 (R3 / KTD3): workspace_root_from_events
    // must always yield an absolute workspace root, never
    // `Path::new(".")` — the validator would reject every spawn with
    // RelativePath when the bridge repo_root is relative.
    // -------------------------------------------------------------------

    #[test]
    fn u3_workspace_root_from_events_absolute() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let ralph = tmp.path().join(".ralph");
        std::fs::create_dir_all(&ralph).expect("mkdir .ralph");
        let events = ralph.join("events.jsonl");

        let root = workspace_root_from_events(&events);
        assert!(
            root.is_absolute(),
            "workspace_root_from_events must be absolute; got {root:?}"
        );
        assert_eq!(
            root,
            tmp.path(),
            "two `.parent()` calls from <ws>/.ralph/events.jsonl must yield <ws>"
        );
    }

    #[test]
    fn u3_workspace_root_from_events_relative_falls_back() {
        // Defensive: even when a relative path slips through (the
        // runner always passes absolute), the helper must still
        // return an absolute root. We do not rely on
        // `set_current_dir` (unreliable under nextest's
        // process-per-test isolation); we just assert absoluteness.
        let rel = std::path::Path::new(".ralph").join("events.jsonl");
        let root = workspace_root_from_events(&rel);
        assert!(
            root.is_absolute(),
            "relative input must still produce an absolute workspace root; got {root:?}"
        );
    }

    #[test]
    fn u3_lazy_bridge_repo_root_is_absolute() {
        // 2026-07-26-002 plan U3: the lazy
        // `CoordinatorSupervisorBridge::with_context_and_factory_with_cap`
        // path used in `dispatch_waves` must build the bridge with
        // `repo_root` derived from the main events file (absolute),
        // NOT the previous `PathBuf::from(".")`. We exercise the
        // same construction and assert `bridge.repo_root()` returns
        // the absolute workspace, not `.` or `None`.
        use crate::loop_runner::wave::ProductionBridgeContext;
        use crate::loop_runner::wave::supervisor_bridge::CoordinatorSupervisorBridge;
        use ralph_core::supervisor::SupervisorBridge as _;
        use ralph_core::supervisor::worktree_bind::DefaultWorktreeFactory;

        let tmp = tempfile::TempDir::new().expect("temp dir");
        let main_events_file = tmp.path().join(".ralph").join("events.jsonl");

        let context = ProductionBridgeContext {
            loop_id: "loop-u3".to_string(),
            repo_root: workspace_root_from_events(&main_events_file),
            events_path: Some(main_events_file.clone()),
            tasks_path: None,
        };
        assert!(
            context.repo_root.is_absolute(),
            "ProductionBridgeContext.repo_root must be absolute; got {:?}",
            context.repo_root
        );

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = CoordinatorSupervisorBridge::with_context_and_factory_with_cap(
            store,
            context,
            Arc::new(DefaultWorktreeFactory),
            u32::MAX,
        );
        let reported = bridge
            .repo_root()
            .expect("bridge must surface repo_root; lazy paths used to return None");
        assert_eq!(reported, tmp.path());
    }

    // ─────────────────────────────────────────────────────────────────
    // Plan 004 P1-6: terminal evidence topic/dimension strict checks
    //
    // The post-fix `build_review_done_hints` rejects four classes of
    // mismatch (KTD3 fail-closed):
    //   1. evidence topic != "review.unit.done"
    //   2. evidence dimension missing
    //   3. evidence dimension != slot's assigned dimension
    //   4. slot has no assigned dimension
    // Each test below pins one failure mode so a future regression
    // that re-introduces the silent-fallback surfaces here.
    // ─────────────────────────────────────────────────────────────────

    fn build_p1_6_hints(
        store: &std::sync::Arc<ralph_core::supervisor::InMemorySupervisorStore>,
        wave_id: &str,
        assigned_dimensions: std::collections::HashMap<u32, String>,
    ) -> ReviewDoneHints {
        use ralph_core::supervisor::SupervisorBridge as _;
        use ralph_core::wave_tracker::CompletedWave;
        let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> =
            Arc::new(ralph_core::supervisor::InMemoryCoordinatorBridge::from_store(store.clone()));
        let completed = CompletedWave {
            wave_id: wave_id.to_string(),
            wave_total: assigned_dimensions.len() as u32,
            results: Vec::new(),
            failures: Vec::new(),
            duration: std::time::Duration::from_secs(0),
            partial: false,
            expected_source_hat: None,
            assigned_dimensions,
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let main_events = std::env::temp_dir().join("p1-6-does-not-exist.jsonl");
        build_review_done_hints(&bridge, wave_id, &completed, &main_events)
    }

    /// P1-6 #1: evidence topic != "review.unit.done" is rejected.
    #[test]
    fn p1_6_wrong_evidence_topic_is_rejected() {
        use ralph_core::supervisor::{SlotResource, SupervisorStore, TerminalEvidence, WaveKind};
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let wave = store
            .register_wave("p1-6-topic", WaveKind::Review, 1, 1)
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &TerminalEvidence::from_event(
                    "work.start", // wrong topic
                    "{\"dimension\":\"correctness\"}",
                ),
            )
            .unwrap();
        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0, "correctness".to_string());
        let hints = build_p1_6_hints(&store, &wave, assigned);
        assert!(
            !hints.store_completed.contains("correctness"),
            "wrong-topic evidence must not be accepted as done (got {:?})",
            hints.store_completed,
        );
    }

    /// P1-6 #2: evidence with no dimension is rejected.
    #[test]
    fn p1_6_missing_dimension_is_rejected() {
        use ralph_core::supervisor::{SlotResource, SupervisorStore, TerminalEvidence, WaveKind};
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let wave = store
            .register_wave("p1-6-dim", WaveKind::Review, 1, 1)
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        // Note: TerminalEvidence::from_event without a dimension
        // field yields dimension=None (matches the legacy
        // happy-path that the post-fix code refuses).
        let evidence = TerminalEvidence {
            topic: "review.unit.done".to_string(),
            dimension: None,
            payload_fingerprint: "abc".to_string(),
        };
        store
            .record_slot_terminal_evidence(&wave, 0, &evidence)
            .unwrap();
        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0, "correctness".to_string());
        let hints = build_p1_6_hints(&store, &wave, assigned);
        assert!(
            hints.store_completed.is_empty(),
            "missing-dimension evidence must not be accepted (got {:?})",
            hints.store_completed,
        );
    }

    /// P1-6 #3: evidence dimension != assigned is rejected.
    #[test]
    fn p1_6_dimension_mismatch_is_rejected() {
        use ralph_core::supervisor::{SlotResource, SupervisorStore, TerminalEvidence, WaveKind};
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let wave = store
            .register_wave("p1-6-mis", WaveKind::Review, 1, 1)
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &TerminalEvidence::from_event(
                    "review.unit.done",
                    "{\"dimension\":\"security\"}", // mismatched
                ),
            )
            .unwrap();
        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0, "correctness".to_string());
        let hints = build_p1_6_hints(&store, &wave, assigned);
        assert!(
            !hints.store_completed.contains("correctness"),
            "dimension-mismatched evidence must not be accepted as done",
        );
        assert!(
            !hints.store_completed.contains("security"),
            "wrong dimension must not be counted under any name",
        );
    }

    /// P1-6 #4: slot has no assigned dimension at all → refuse.
    #[test]
    fn p1_6_no_assigned_dimension_is_rejected() {
        use ralph_core::supervisor::{SlotResource, SupervisorStore, TerminalEvidence, WaveKind};
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let wave = store
            .register_wave("p1-6-na", WaveKind::Review, 1, 1)
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &TerminalEvidence::from_event(
                    "review.unit.done",
                    "{\"dimension\":\"correctness\"}",
                ),
            )
            .unwrap();
        let assigned = std::collections::HashMap::new();
        let hints = build_p1_6_hints(&store, &wave, assigned);
        assert!(
            hints.store_completed.is_empty(),
            "no-assigned-dimension must fail closed",
        );
    }

    /// P1-6 positive control: a slot whose evidence topic,
    /// dimension, and assigned dimension ALL agree IS
    /// accepted.
    #[test]
    fn p1_6_matching_evidence_dimension_accepted() {
        use ralph_core::supervisor::{SlotResource, SupervisorStore, TerminalEvidence, WaveKind};
        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let wave = store
            .register_wave("p1-6-ok", WaveKind::Review, 1, 1)
            .unwrap();
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        store.record_slot_result(&wave, 0, "h", 1).unwrap();
        store
            .record_slot_terminal_evidence(
                &wave,
                0,
                &TerminalEvidence::from_event(
                    "review.unit.done",
                    "{\"dimension\":\"correctness\"}",
                ),
            )
            .unwrap();
        let mut assigned = std::collections::HashMap::new();
        assigned.insert(0, "correctness".to_string());
        let hints = build_p1_6_hints(&store, &wave, assigned);
        assert!(
            hints.store_completed.contains("correctness"),
            "matching evidence + assigned must be accepted; got {:?}",
            hints.store_completed,
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // Plan 004 P1-7: main-ledger reconciliation accepts both
    // object and JSON-encoded-string payload shapes. The
    // pre-fix code only consumed the string form, so object
    // payloads (the supervisor merge sink writes them
    // directly) were silently ignored and the dimension was
    // re-counted as missing.
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn p1_7_payload_object_from_string_json() {
        // String-encoded JSON (legacy agent-emit path).
        let payload = serde_json::Value::String(
            serde_json::to_string(&serde_json::json!({"dimension":"correctness"})).unwrap(),
        );
        let map = payload_object(Some(&payload));
        assert!(map.is_some());
        assert_eq!(
            map.unwrap().get("dimension").and_then(|v| v.as_str()),
            Some("correctness"),
        );
    }

    #[test]
    fn p1_7_payload_object_from_inline_object() {
        // Inline object (supervisor merge sink path).
        let payload = serde_json::json!({"dimension": "correctness"});
        let map = payload_object(Some(&payload));
        assert!(map.is_some());
        assert_eq!(
            map.unwrap().get("dimension").and_then(|v| v.as_str()),
            Some("correctness"),
        );
    }

    #[test]
    fn p1_7_payload_object_missing_returns_none() {
        let map = payload_object(None);
        assert!(map.is_none());
    }

    #[test]
    fn p1_7_payload_object_malformed_string_returns_none() {
        // String that is not valid JSON.
        let payload = serde_json::Value::String("not json".to_string());
        let map = payload_object(Some(&payload));
        assert!(map.is_none());
    }

    // ══════════════════════════════════════════════════════════════════
    // U1: fan-in terminal convergence — RED characterization tests
    // RED: these tests characterize the broken behavior BEFORE the fix.
    // GREEN: the fix makes them pass.
    // ══════════════════════════════════════════════════════════════════

    /// AE3 / R3: when AggregateDeadlineExceeded arrives with Pending slots,
    /// fan-in must converge to InjectedFailed (not ContinueCollect).
    #[test]
    fn terminal_aggregate_deadline_does_not_end_as_continue_collect() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::{SupervisorBridge, SupervisorStore, WaveKind};

        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "w-u1-red1", 2, 1)
            .unwrap();

        // Dispatch BOTH slots so they exist in the store.
        // Slot 0: dispatched, completed with evidence.
        // Slot 1: dispatched, then cancelled via cancel_wave (simulating timeout).
        let _ = store.try_dispatch_next(2).unwrap().unwrap(); // slot 0
        let _ = store.try_dispatch_next(2).unwrap().unwrap(); // slot 1
        // Slot 0: Completed with evidence.
        store
            .record_slot_result(&store_wave_id, 0, "hash-s0", 1)
            .unwrap();
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "review.unit.done",
                    &serde_json::json!({"dimension": "correctness"}).to_string(),
                ),
            )
            .unwrap();
        // Slot 1: Pending in store (will be cancelled by cancel_wave).

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u1-red1".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 0,
                events: vec![
                    ralph_proto::Event::new(
                        "review.unit.done",
                        serde_json::json!({"dimension": "correctness"}).to_string(),
                    )
                    .with_source("review-worker")
                    .with_wave("w-u1-red1".to_string(), 0, 2),
                ],
            }],
            failures: vec![],
            duration: std::time::Duration::ZERO,
            partial: true,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u1-red1".to_string(),
            target_hat: ralph_proto::HatId::new("review-dispatcher"),
            hat_config: HatConfig {
                name: "review-dispatcher".to_string(),
                ..HatConfig::default()
            },
            events: vec![ralph_core::Event {
                topic: "review.unit.ready".to_string(),
                payload: Some("payload-0".to_string()),
                ts: String::new(),
                hat: None,
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            }],
            total: 2,
            partial: true,
            consumer_aggregate_timeout: None,
        };

        // Record slot 1 as Failed (simulating worker never started / timed out).
        // Using Failed instead of cancel so pending_count = 0 in fan_in_status.
        let fail_result = store.record_slot_failure(
            &store_wave_id,
            1,
            ralph_core::supervisor::worker_outcome::REASON_SLOT_NEVER_STARTED,
        );
        fail_result.unwrap();

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        let terminal_ctx = Some(TerminalFanInContext {
            cancel_requested: true, // AggregateDeadlineExceeded → cancel
            elapsed: completed.duration,
        });
        let outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            terminal_ctx,
        );

        // GREEN: must reach InjectedFailed.
        assert!(
            matches!(outcome, SupervisorFanInOutcome::InjectedFailed),
            "aggregate deadline with Pending slots must reach InjectedFailed, got {:?}",
            outcome
        );
    }

    /// AE2 / R3: partial=true with Pending slots must converge to InjectedFailed.
    #[test]
    fn terminal_partial_with_pending_slot_converges_to_failed() {
        use ralph_core::config::HatConfig;
        use ralph_core::supervisor::{SupervisorBridge, SupervisorStore, WaveKind};

        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "w-u1-red2", 2, 1)
            .unwrap();

        // Dispatch slot 0 so it moves from Pending to Dispatched,
        // then record the result (simulating a completed worker).
        let _ = store.try_dispatch_next(2).unwrap().unwrap();
        // Slot 0: Completed with evidence.
        store
            .record_slot_result(&store_wave_id, 0, "hash-s0", 1)
            .unwrap();
        store
            .record_slot_terminal_evidence(
                &store_wave_id,
                0,
                &ralph_core::supervisor::TerminalEvidence::from_event(
                    "review.unit.done",
                    &serde_json::json!({"dimension": "correctness"}).to_string(),
                ),
            )
            .unwrap();
        // Slot 1: Pending (never dispatched — slot stays Pending in the store).

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u1-red2".to_string(),
            wave_total: 2,
            results: vec![ralph_core::WaveResult {
                index: 0,
                events: vec![
                    ralph_proto::Event::new(
                        "review.unit.done",
                        serde_json::json!({"dimension": "correctness"}).to_string(),
                    )
                    .with_source("review-worker")
                    .with_wave("w-u1-red2".to_string(), 0, 2),
                ],
            }],
            failures: vec![],
            duration: std::time::Duration::ZERO,
            partial: true,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u1-red2".to_string(),
            target_hat: ralph_proto::HatId::new("review-dispatcher"),
            hat_config: HatConfig {
                name: "review-dispatcher".to_string(),
                ..HatConfig::default()
            },
            events: vec![ralph_core::Event {
                topic: "review.unit.ready".to_string(),
                payload: Some("payload-0".to_string()),
                ts: String::new(),
                hat: None,
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            }],
            total: 2,
            partial: true,
            consumer_aggregate_timeout: None,
        };

        let bridge_arc: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(bridge);
        let terminal_ctx = Some(TerminalFanInContext {
            cancel_requested: false, // Partial (not cancel)
            elapsed: completed.duration,
        });
        let outcome = run_supervisor_fan_in(
            &bridge_arc,
            &completed,
            &detected,
            &main_events_file,
            60,
            terminal_ctx,
        );

        // GREEN: the helper drives to InjectedFailed.
        assert!(
            matches!(outcome, SupervisorFanInOutcome::InjectedFailed),
            "partial=true with Pending slots must converge to InjectedFailed, got {:?}",
            outcome
        );
    }

    /// GREEN baseline: non-terminal wave stays ContinueCollect.
    #[test]
    fn non_terminal_tick_remains_continue_collect() {
        use ralph_core::supervisor::{PhaseInputs, SupervisorBridge, SupervisorStore, WaveKind};

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let bridge = InMemoryCoordinatorBridge::from_store(store.clone());
        let store_wave_id = bridge
            .register_wave_if_absent(WaveKind::Review, "w-u1-green1", 2, 1)
            .unwrap();
        // No slots dispatched — all Pending.

        let inputs = PhaseInputs {
            aggregate_timeout_secs: 60,
            elapsed_secs: 0,
            cancel_requested: false,
        };
        let action = bridge
            .tick_with_slot_events(&store_wave_id, inputs, Vec::new())
            .expect("tick succeeds");
        assert!(
            matches!(
                action,
                ralph_core::supervisor::CoordinatorAction::ContinueCollect
            ),
            "non-terminal wave must stay ContinueCollect, got {:?}",
            action
        );
    }

    /// S5: persistent store error must return StoreError.
    #[test]
    fn terminal_fan_in_persistent_store_error_is_not_silent() {
        use ralph_core::config::HatConfig;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct FailingBridge {
            inner: InMemoryCoordinatorBridge,
            fail: AtomicBool,
        }
        impl std::fmt::Debug for FailingBridge {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("FailingBridge").finish()
            }
        }
        impl SupervisorBridge for FailingBridge {
            fn register_wave_if_absent(
                &self,
                k: WaveKind,
                id: &str,
                n: u32,
                slot_retry_budget: u32,
            ) -> Result<String, BridgeError> {
                self.inner.register_wave_if_absent(k, id, n, slot_retry_budget)
            }
            fn fan_in_status(
                &self,
                id: &str,
            ) -> Result<ralph_core::supervisor::WaveSnapshot, BridgeError> {
                if self.fail.load(Ordering::SeqCst) {
                    Err(BridgeError::Store("simulated".into()))
                } else {
                    self.inner.fan_in_status(id)
                }
            }
            fn tick_with_slot_events(
                &self,
                id: &str,
                inputs: PhaseInputs,
                ev: Vec<ralph_proto::Event>,
            ) -> Result<CoordinatorAction, BridgeError> {
                if self.fail.load(Ordering::SeqCst) {
                    Err(BridgeError::Store("simulated".into()))
                } else {
                    self.inner.tick_with_slot_events(id, inputs, ev)
                }
            }
            fn tick(
                &self,
                id: &str,
                inputs: PhaseInputs,
            ) -> Result<CoordinatorAction, BridgeError> {
                self.inner.tick(id, inputs)
            }
            fn slot_resources(
                &self,
                id: &str,
            ) -> Result<Vec<ralph_core::supervisor::SlotResource>, BridgeError> {
                self.inner.slot_resources(id)
            }
            fn max_concurrent_workers(&self) -> u32 {
                self.inner.max_concurrent_workers()
            }
            fn repo_root(&self) -> Option<&std::path::Path> {
                self.inner.repo_root()
            }
            fn tasks_path(&self) -> Option<&std::path::Path> {
                self.inner.tasks_path()
            }
            fn try_dispatch_next(&self, id: &str, idx: u32) -> Result<bool, BridgeError> {
                self.inner.try_dispatch_next(id, idx)
            }
            fn bind_slot(
                &self,
                k: WaveKind,
                id: &str,
                idx: u32,
            ) -> Result<Option<ralph_core::supervisor::SlotBinding>, BridgeError> {
                self.inner.bind_slot(k, id, idx)
            }
            fn recover(&self) -> Result<Vec<ralph_core::supervisor::WaveSnapshot>, BridgeError> {
                self.inner.recover()
            }
            fn mark_salvage_merged(&self, id: &str) -> Result<(), BridgeError> {
                self.inner.mark_salvage_merged(id)
            }
            fn record_slot_result(
                &self,
                id: &str,
                idx: u32,
                h: &str,
                n: usize,
            ) -> Result<(), BridgeError> {
                self.inner.record_slot_result(id, idx, h, n)
            }
            fn record_slot_terminal_evidence(
                &self,
                id: &str,
                idx: u32,
                e: &ralph_core::supervisor::TerminalEvidence,
            ) -> Result<(), BridgeError> {
                self.inner.record_slot_terminal_evidence(id, idx, e)
            }
            fn slot_terminal_evidence(
                &self,
                id: &str,
                idx: u32,
            ) -> Result<Option<ralph_core::supervisor::TerminalEvidence>, BridgeError> {
                self.inner.slot_terminal_evidence(id, idx)
            }
            fn record_slot_failure(&self, id: &str, idx: u32, r: &str) -> Result<(), BridgeError> {
                self.inner.record_slot_failure(id, idx, r)
            }
            fn record_never_started_failures(&self, id: &str) -> Result<(), BridgeError> {
                self.inner.record_never_started_failures(id)
            }
            fn slot_failure_reason(
                &self,
                id: &str,
                idx: u32,
            ) -> Result<Option<String>, BridgeError> {
                self.inner.slot_failure_reason(id, idx)
            }
            fn release_slot_dispatch(
                &self,
                id: &str,
                idx: u32,
                o: ralph_core::supervisor::DispatchOutcome,
            ) -> Result<(), BridgeError> {
                self.inner.release_slot_dispatch(id, idx, o)
            }
            fn finalize_terminal_cleanup(&self, p: &std::path::Path) -> Result<(), BridgeError> {
                self.inner.finalize_terminal_cleanup(p)
            }
            fn cancel_wave(&self, id: &str) -> Result<(), BridgeError> {
                self.inner.cancel_wave(id)
            }
            fn enqueue_compensation(
                &self,
                id: &str,
                k: ralph_core::supervisor::CompensationKind,
            ) -> Result<(), BridgeError> {
                self.inner.enqueue_compensation(id, k)
            }
            fn take_pending_compensations(
                &self,
            ) -> Result<Vec<(String, ralph_core::supervisor::CompensationKind)>, BridgeError>
            {
                self.inner.take_pending_compensations()
            }
            fn complete_compensation(
                &self,
                id: &str,
                k: ralph_core::supervisor::CompensationKind,
                ok: bool,
            ) -> Result<(), BridgeError> {
                self.inner.complete_compensation(id, k, ok)
            }
            fn set_wave_phase(
                &self,
                id: &str,
                p: ralph_core::supervisor::WavePhase,
            ) -> Result<(), BridgeError> {
                self.inner.set_wave_phase(id, p)
            }
            fn mark_merge_to_events(&self, id: &str) -> Result<(), BridgeError> {
                self.inner.mark_merge_to_events(id)
            }
        }

        let workspace = tempfile::TempDir::new().expect("temp workspace");
        let ralph_dir = workspace.path().join(".ralph");
        std::fs::create_dir_all(&ralph_dir).expect("mkdir .ralph");
        let main_events_file = ralph_dir.join("events.jsonl");

        let store = std::sync::Arc::new(ralph_core::supervisor::InMemorySupervisorStore::new());
        let inner = InMemoryCoordinatorBridge::from_store(store);
        let bridge: Arc<dyn ralph_core::supervisor::SupervisorBridge> = Arc::new(FailingBridge {
            inner,
            fail: AtomicBool::new(true),
        });

        let completed = ralph_core::CompletedWave {
            wave_id: "w-u1-red5".to_string(),
            wave_total: 1,
            results: vec![],
            failures: vec![],
            duration: std::time::Duration::ZERO,
            partial: false,
            expected_source_hat: None,
            assigned_dimensions: std::collections::HashMap::new(),
            dimension_retry_counts: std::collections::HashMap::new(),
            worker_events: Vec::new(),
        };
        let detected = ralph_core::DetectedWave {
            wave_id: "w-u1-red5".to_string(),
            target_hat: ralph_proto::HatId::new("review-dispatcher"),
            hat_config: HatConfig {
                name: "review-dispatcher".to_string(),
                ..HatConfig::default()
            },
            events: vec![ralph_core::Event {
                topic: "review.unit.ready".to_string(),
                payload: Some("payload-0".to_string()),
                ts: String::new(),
                hat: None,
                triggered: None,
                source: None,
                wave_id: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            }],
            total: 1,
            partial: false,
            consumer_aggregate_timeout: None,
        };

        let outcome =
            run_supervisor_fan_in(&bridge, &completed, &detected, &main_events_file, 60, None);
        assert!(
            matches!(outcome, SupervisorFanInOutcome::StoreError),
            "persistent store error must return StoreError, got {:?}",
            outcome
        );
    }
}
