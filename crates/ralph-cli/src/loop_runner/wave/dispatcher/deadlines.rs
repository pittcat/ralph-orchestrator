//! Deadlines module — store selection (`open_default_supervisor_store`),
//! budget / threshold constants (`PARTIAL_THRESHOLD_NUM`,
//! `PARTIAL_THRESHOLD_DEN`, `WAVE_WORK_BUDGET_SLACK_SECS`) and helpers
//! (`wave_work_budget`, `aggregate_timeout_for`,
//! `aggregate_floor_for_attempts`, `attempt_aware_aggregate_timeout`,
//! `effective_detected_aggregate_deadline_secs`), and the per-worker
//! dimension parser (`parse_assigned_dimension`).
//!
//! Originally part of `wave/dispatcher.rs` (plan `2026-08-07-008`).
//! Public surface and behaviour preserved verbatim.

use std::sync::Arc;
use std::time::Duration;

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
use tracing::warn;
pub(crate) fn open_default_supervisor_store(
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

/// Fraction of the aggregate timeout after which the dispatcher stops
/// admitting new work and starts winding the wave down (the "partial"
/// threshold), expressed as `PARTIAL_THRESHOLD_NUM / PARTIAL_THRESHOLD_DEN`.
pub const PARTIAL_THRESHOLD_NUM: u64 = 8;
pub const PARTIAL_THRESHOLD_DEN: u64 = 10;

/// Slack added to every work budget so a wave that uses its full
/// per-worker budget still has room to collect and record results.
const WAVE_WORK_BUDGET_SLACK_SECS: u64 = 30;

/// How long a wave legitimately needs: every batch, every attempt, plus
/// the collection slack.
///
/// KTD-U3 §4: `actual_worker_count = wave.events.len()` (NOT
/// `wave.total`, which is the protocol-declared count and may
/// exceed actual events for malformed partial waves).
///
/// 2026-07-30-001 plan U3: `max_attempts` folds the supervisor's retry
/// budget in. `max_attempts = 1` reproduces the pre-plan formula
/// exactly, which is what the legacy (non-supervisor) dispatch path
/// still uses.
pub(crate) fn wave_work_budget(
    wave_timeout: Duration,
    events_count: usize,
    concurrency: usize,
    max_attempts: u32,
) -> Duration {
    let events_count = events_count.max(1) as u64;
    let concurrency = concurrency.max(1) as u64;
    let batches = events_count.div_ceil(concurrency);
    Duration::from_secs(
        wave_timeout
            .as_secs()
            .saturating_mul(batches)
            .saturating_mul(u64::from(max_attempts.max(1)))
            .saturating_add(WAVE_WORK_BUDGET_SLACK_SECS),
    )
}

/// Compute the aggregate timeout from per-worker timeout and the
/// number of concurrent batches (single attempt, legacy path).
pub(crate) fn aggregate_timeout_for(
    wave_timeout: Duration,
    events_count: usize,
    concurrency: usize,
) -> Duration {
    wave_work_budget(wave_timeout, events_count, concurrency, 1)
}

/// 2026-07-30-001 plan U3: the smallest aggregate timeout that keeps
/// the partial threshold from firing before the wave's legitimate work
/// budget has been spent.
///
/// The dispatcher stops admitting work at 80% of the aggregate timeout,
/// so an aggregate equal to the work budget would preempt the last 20%
/// of legal work — including a slot's final retry. Inverting the
/// threshold gives `ceil(work_budget * 10 / 8)`.
///
/// The multiply is done after dividing so a caller-supplied timeout
/// near `u64::MAX` cannot wrap: `ceil(w * 10 / 8) == ceil(w * 5 / 4)`,
/// evaluated as `(w / 4) * 5 + ceil((w % 4) * 5 / 4)`.
pub(crate) fn aggregate_floor_for_attempts(
    wave_timeout: Duration,
    events_count: usize,
    concurrency: usize,
    retry_budget: u32,
) -> Duration {
    let work = wave_work_budget(
        wave_timeout,
        events_count,
        concurrency,
        retry_budget.saturating_add(1),
    )
    .as_secs();
    let whole = work / PARTIAL_THRESHOLD_NUM;
    let remainder = work % PARTIAL_THRESHOLD_NUM;
    Duration::from_secs(
        whole.saturating_mul(PARTIAL_THRESHOLD_DEN).saturating_add(
            remainder
                .saturating_mul(PARTIAL_THRESHOLD_DEN)
                .div_ceil(PARTIAL_THRESHOLD_NUM),
        ),
    )
}

pub(crate) fn attempt_aware_aggregate_timeout(
    configured_aggregate_timeout: Duration,
    wave_timeout: Duration,
    events_count: usize,
    effective_concurrency: usize,
    retry_budget: u32,
) -> Duration {
    configured_aggregate_timeout.max(aggregate_floor_for_attempts(
        wave_timeout,
        events_count,
        effective_concurrency,
        retry_budget,
    ))
}

/// Effective wave-derived aggregate deadline in seconds.
///
/// Mirrors the inline computation in `execute_wave_via_supervisor_with_executor`
/// so fan-in and the supervisor execution path agree on the same budget.
/// Returns seconds (not Duration) so the fan-in call site (`run_supervisor_fan_in`)
/// and the supervisor execution path can both consume it.
pub(crate) fn effective_detected_aggregate_deadline_secs(
    wave: &ralph_core::DetectedWave,
    bridge: &dyn ralph_core::supervisor::SupervisorBridge,
) -> u64 {
    let wave_timeout = Duration::from_secs(wave.per_worker_timeout_secs());
    let concurrency = wave.hat_config.concurrency as usize;
    let configured =
        if wave.has_explicit_aggregate_timeout() || wave.consumer_aggregate_timeout.is_some() {
            Duration::from_secs(wave.aggregate_timeout_secs())
        } else {
            aggregate_timeout_for(wave_timeout, wave.events.len(), concurrency)
        };
    let effective_cap = wave
        .hat_config
        .concurrency
        .min(bridge.max_concurrent_workers())
        .max(1) as usize;
    attempt_aware_aggregate_timeout(
        configured,
        wave_timeout,
        wave.events.len(),
        effective_cap,
        bridge.slot_retry_budget(),
    )
    .as_secs()
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
pub(crate) fn parse_assigned_dimension(payload: Option<&str>) -> Option<String> {
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
