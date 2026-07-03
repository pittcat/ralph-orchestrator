//! Supervisor domain types — `WaveKind`, `IsolationMode`, `WavePhase`,
//! slot lifecycle enums, and the `SupervisorStore` trait.
//!
//! 2026-07-03-001 plan U2 scope: this module declares the **shapes** only.
//! Memory and rusqlite stores arrive in U3/U4/U5; the coordinator and
//! dispatcher bridge are introduced in U8/U12. No `SupervisorStore`
//! implementation lives here on purpose — implementations must round-trip
//! through the same trait contract that the in-memory and SQL stores
//! share.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Which wave category a registered wave belongs to.
///
/// The U13 preset uses `Exec` and `Fix` for parallel implementation and
/// parallel remediation; `Review` is the fan-in review batch whose
/// results feed the `--payloads` review-coordinator single batch emit
/// (see `docs/solutions/integration-issues/ce-executor-wave-emission-must-batch-in-single-emit-2026-06-09.md`).
/// The runtime injects coordination events per `WaveKind`
/// (`exec.wave.complete` vs `fix.wave.complete` vs `review.wave.complete`)
/// so hats that only react to one band stay isolated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaveKind {
    Exec,
    Fix,
    Review,
}

impl fmt::Display for WaveKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WaveKind::Exec => write!(f, "exec"),
            WaveKind::Fix => write!(f, "fix"),
            WaveKind::Review => write!(f, "review"),
        }
    }
}

/// How the wave's slots are isolated from the workspace at dispatch
/// time.
///
/// The KTD-5 / R-WT-1 contract: exec and fix waves always use
/// `Worktree` (`isolation_mode=worktree`) so each slot gets a
/// dedicated branch; review waves use `SharedReadonly` because the
/// snapshot of the integrator's worktree is sufficient and the
/// reviewer should not mutate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationMode {
    /// Each slot gets its own git worktree branch; dispatch attaches
    /// `RALPH_WAVE_WORKTREE_PATH` etc. (U10 helper builds the env).
    Worktree,
    /// Reviewers share the integrator's worktree read-only. The
    /// `slot_resources` table has no `worktree_path` for these slots.
    SharedReadonly,
}

impl fmt::Display for IsolationMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IsolationMode::Worktree => write!(f, "worktree"),
            IsolationMode::SharedReadonly => write!(f, "shared_readonly"),
        }
    }
}

/// Phase of a wave's lifetime, mirroring the state-machine in the
/// requirements doc (R-C-state / KTD-5). The coordinator does NOT
/// promote a wave to `Done` until `work.done` (exec/fix paths) lands;
/// staying in `Integrate` after fan-in lets a crash double-inject
/// `*.wave.complete` be detected by `merged_to_events` on recovery
/// (U11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WavePhase {
    /// `register_wave` is processing; slots are being dispatched.
    Dispatch,
    /// At least one slot is still pending or running. `fan_in_status`
    /// is consulted each time a slot transitions to `completed` or
    /// `failed`.
    Collect,
    /// Fan-in completed (all required slots reached a terminal
    /// state). The coordinator now integrates results into the
    /// main JSONL stream and prepares the `*.wave.complete`
    /// payload.
    Integrate,
    /// Integrator finished and `work.done` (or `fix.done`) reached
    /// the runtime. The wave is fully closed.
    Done,
    /// Permanent failure: timeout, cancel, or required-slot
    /// failure exhausted retries (R-B3, KTD-8). The wave is dead.
    Failed,
}

impl fmt::Display for WavePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WavePhase::Dispatch => write!(f, "dispatch"),
            WavePhase::Collect => write!(f, "collect"),
            WavePhase::Integrate => write!(f, "integrate"),
            WavePhase::Done => write!(f, "done"),
            WavePhase::Failed => write!(f, "failed"),
        }
    }
}

/// Lifecycle status of a single slot inside a wave.
///
/// `Pending` is "registered but not yet dispatched (e.g. waiting for
/// a backpressure slot to free up)"; `Dispatched` is "process spawned
/// or in worker pool"; `Running` differs from `Dispatched` only when
/// the worker process emits a heartbeat (currently tracked via the
/// worker's process PID + dispatcher poll — out of scope for U2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotStatus {
    Pending,
    Dispatched,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl fmt::Display for SlotStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlotStatus::Pending => write!(f, "pending"),
            SlotStatus::Dispatched => write!(f, "dispatched"),
            SlotStatus::Running => write!(f, "running"),
            SlotStatus::Completed => write!(f, "completed"),
            SlotStatus::Failed => write!(f, "failed"),
            SlotStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Outcome of a slot dispatch attempt reported back through
/// `record_slot_result` / `record_slot_failure`. The coordinator
/// updates `WavePhase` accordingly (see `phase.rs` in U6 for the
/// pure-function transition rules).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchOutcome {
    /// Slot merged successfully: `content_hash` recorded, events
    /// are ready to be appended to JSONL when fan-in completes.
    Completed,
    /// Slot completed but the dispatcher / supervisor detected a
    /// structural problem (e.g. dimension mismatch R5 of plan
    /// 2026-06-17-002). The retry budget is consumed upstream
    /// before this outcome is reported.
    Failed,
}

/// Stable, OS-level idempotency key. Used by both `register_wave`
/// (whole-wave idempotency) and the slot-level dispatch unique
/// constraint. Visible string for diagnostics.
pub type IdempotencyKey = String;

/// Per-slot resource binding produced by the worktree bind helper
/// (U10). Relative paths are resolved against the loop workspace
/// when set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotResource {
    pub slot_index: u32,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
}

impl SlotResource {
    /// True for `IsolationMode::SharedReadonly` slots (no bound
    /// worktree).
    pub fn is_shared_readonly(&self) -> bool {
        self.worktree_path.is_none() && self.branch.is_none()
    }
}

/// Snapshot of the wave's slot counts at a moment in time. The
/// coordinator passes this to the U6 phase-decision pure function
/// alongside the `cancel_requested` and timeout flags.
///
/// Counting contract:
/// - `completed_count` slots reached `Completed`
/// - `failed_count` slots reached `Failed`
/// - `in_flight_count` slots are `Dispatched` or `Running`
/// - `pending_count` slots are `Pending` or `Cancelled`
/// (cancelled slots never advance on their own)
/// - `expected_total == completed + failed + in_flight + pending`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaveSnapshot {
    pub wave_id: String,
    pub phase: WavePhase,
    pub expected_total: u32,
    pub completed_count: u32,
    pub failed_count: u32,
    pub pending_count: u32,
    /// 2026-07-03-001 plan U3: slots currently Dispatched or
    /// Running. Surfaced so the U6 phase pure function can
    /// distinguish "actively in flight" from "still pending"
    /// when computing aggregate_timeout.
    #[serde(default)]
    pub in_flight_count: u32,
    pub cancel_requested: bool,
    pub merged_to_events: bool,
}

/// Errors a store implementation may return. `SupervisorStore`
/// implementations are fallible: the runtime must fail-closed on
/// open (R-C4) and convert these to `task.resume` payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorStoreError {
    #[error("supervisor is disabled in event_loop config")]
    Disabled,
    #[error("failed to open supervisor database: {0}")]
    Open(String),
    #[error("idempotency key already registered: {0}")]
    DuplicateKey(String),
    #[error("wave not found: {0}")]
    UnknownWave(String),
    #[error("slot {slot_index} not found on wave {wave_id}")]
    UnknownSlot { wave_id: String, slot_index: u32 },
    #[error("backpressure limit reached; wave {0} enqueued")]
    BackpressureEnqueued(String),
    #[error("invalid transition: {0}")]
    InvalidTransition(String),
    #[error("storage error: {0}")]
    Storage(String),
}

/// Result alias for the trait surface.
pub type SupervisorStoreResult<T> = Result<T, SupervisorStoreError>;

/// The supervisor persistence + dispatch-decision trait. Both the
/// in-memory (U3/U4) and rusqlite (U5) implementations satisfy this
/// contract; the coordinator (U8) depends only on the trait so
/// `MockSupervisorStore` can drive unit tests without spinning up a
/// real DB.
///
/// The trait is intentionally **not** an `async` trait: rusqlite is
/// synchronous and the runtime wraps calls in `spawn_blocking`. All
/// methods are pure persistence + dispatch decision — no JSONL
/// merge or runtime event injection lives here (that is the
/// coordinator's job, see KTD-6).
pub trait SupervisorStore: fmt::Debug + Send + Sync {
    /// Register a new wave. Implementations MUST:
    /// - check `idempotency_key` uniqueness (R-D1) and return
    ///   `DuplicateKey` on conflict
    /// - create `expected_total` slot rows with `Pending` status
    /// - in absence of an `IsolationMode` override, default each
    ///   slot's isolation per `WaveKind` (exec/fix=Worktree,
    ///   review=SharedReadonly)
    fn register_wave(
        &self,
        idempotency_key: &str,
        kind: WaveKind,
        expected_total: u32,
    ) -> SupervisorStoreResult<String>;

    /// Enqueue a wave that exceeded the backpressure ceiling. The
    /// store records the wave in `wave_queue` and the dispatcher
    /// drains FIFO via `try_dispatch_next`. Returns the assigned
    /// `wave_id` on success.
    fn enqueue_wave(
        &self,
        idempotency_key: &str,
        kind: WaveKind,
        expected_total: u32,
    ) -> SupervisorStoreResult<String>;

    /// Try to take the next pending slot for dispatch. Returns
    /// `(wave_id, slot_index)` or `None` when the queue is empty.
    /// Implementations MUST respect backpressure: if
    /// `active_workers >= max_concurrent_workers`, the call returns
    /// `None` without consuming from the queue (R-A2 / KTD-11
    /// applies the soft cap).
    fn try_dispatch_next(&self, max_concurrent_workers: u32) -> SupervisorStoreResult<Option<(String, u32)>>;

    /// Record a slot's worktree/resource binding before spawn
    /// (U10's helper calls this). Implementations MUST reject
    /// dispatch of a `Worktree`-isolation slot whose
    /// `slot_resources` is unbound (returns `InvalidTransition`).
    fn bind_worktree(&self, wave_id: &str, slot_index: u32, binding: SlotResource) -> SupervisorStoreResult<()>;

    /// Mark a slot completed with the produced worker result
    /// (events + content_hash for dedup, R-E1).
    fn record_slot_result(
        &self,
        wave_id: &str,
        slot_index: u32,
        content_hash: &str,
        event_count: usize,
    ) -> SupervisorStoreResult<()>;

    /// Mark a slot permanently failed. The phase-decision pure
    /// function (U6) consumes this alongside the snapshot.
    fn record_slot_failure(
        &self,
        wave_id: &str,
        slot_index: u32,
        reason: &str,
    ) -> SupervisorStoreResult<()>;

    /// Set the wave's cancel-requested flag. Pending slots become
    /// `Cancelled`; running slots are killed by the runtime via
    /// PID (out of scope for the store layer, R-B4).
    fn cancel_wave(&self, wave_id: &str) -> SupervisorStoreResult<()>;

    /// Return the current slot/lifecycle snapshot for the phase
    /// decision pure function (U6).
    fn fan_in_status(&self, wave_id: &str) -> SupervisorStoreResult<WaveSnapshot>;

    /// Mark the wave's merged-to-events row so recovery (U11) does
    /// not double-inject `*.wave.complete`. Idempotent: repeated
    /// calls return `Ok(())`.
    fn mark_merge_to_events(&self, wave_id: &str) -> SupervisorStoreResult<()>;

    /// Recover active waves on loop startup. Returns waves whose
    /// slot rows survived a crash (R-C3). Used by U11; does not
    /// touch DB state.
    fn recover_active_waves(&self) -> SupervisorStoreResult<Vec<WaveSnapshot>>;

    /// List the resource bindings a wave allocated (used by the
    /// integrator and the worktree cleanup at loop end).
    fn list_worktree_paths(&self, wave_id: &str) -> SupervisorStoreResult<Vec<SlotResource>>;
}

pub use memory::InMemorySupervisorStore;
#[cfg(feature = "supervisor-db")]
pub use rusqlite::RusqliteSupervisorStore;

mod memory;
#[cfg(feature = "supervisor-db")]
mod migrations;
#[cfg(feature = "supervisor-db")]
mod rusqlite;
#[cfg(test)]
mod memory_protocol_tests;
#[cfg(test)]
mod types_tests;
