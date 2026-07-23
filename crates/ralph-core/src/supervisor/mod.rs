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
use std::time::SystemTime;

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
    pub kind: WaveKind,
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
    /// 2026-07-03-001 plan U6: wall-clock instant the wave
    /// was registered. Recovery (U11) uses this to decide
    /// the `Failed` timeout verdict; both stores populate
    /// it from their `created_at` source.
    #[serde(default = "default_started_at")]
    pub started_at: SystemTime,
    /// 2026-07-03-001 plan U3 / F-003: per-slot status list.
    /// The phase-decision pure function reads this to populate
    /// the `blocking_slots` payload instead of fabricating a
    /// range from `expected_total - completed_count` (which
    /// mis-classified legitimately completed slots as blocking
    /// in the pre-fix code, dropping the actual failed slot).
    /// Both stores populate this via JOIN against
    /// `wave_slots.status`.
    #[serde(default)]
    pub slots: Vec<(u32, SlotStatus)>,
}

fn default_started_at() -> SystemTime {
    // `UNIX_EPOCH` keeps the snapshot serializable across
    // the existing JSONL envelope without forcing callers
    // to plumb a `None` analog. Tests construct the
    // snapshot directly and overwrite the field.
    SystemTime::UNIX_EPOCH
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
    fn try_dispatch_next(
        &self,
        max_concurrent_workers: u32,
    ) -> SupervisorStoreResult<Option<(String, u32)>>;

    /// Release a dispatched slot when its worker reaches a terminal
    /// outcome. This is separate from result persistence so the
    /// dispatcher can return capacity before U5 records event batches
    /// and content hashes. Repeated release calls are idempotent: a
    /// slot already in a terminal state remains terminal and returns
    /// `Ok(())`.
    fn release_slot_dispatch(
        &self,
        wave_id: &str,
        slot_index: u32,
        outcome: DispatchOutcome,
    ) -> SupervisorStoreResult<()>;

    /// Record a slot's worktree/resource binding before spawn
    /// (U10's helper calls this). Implementations MUST reject
    /// dispatch of a `Worktree`-isolation slot whose
    /// `slot_resources` is unbound (returns `InvalidTransition`).
    fn bind_worktree(
        &self,
        wave_id: &str,
        slot_index: u32,
        binding: SlotResource,
    ) -> SupervisorStoreResult<()>;

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

    /// 2026-07-03-001 plan U7: record the OS-level PID of
    /// a worker the dispatcher spawned for a slot. U12 uses
    /// this so `cancel_wave` can call
    /// `nix::sys::signal::kill` (R-B4). Idempotent:
    /// re-recording the same `(wave, slot)` overwrites the
    /// prior PID (the dispatch loop may respawn the worker
    /// after a backoff).
    fn record_slot_pid(
        &self,
        wave_id: &str,
        slot_index: u32,
        pid: u32,
    ) -> SupervisorStoreResult<()>;

    /// 2026-07-03-001 plan U7: look up the PID the runtime
    /// recorded for `(wave, slot)`. Returns `None` when no
    /// worker was recorded (test fixtures, completed slots
    /// where the dispatch row was reclaimed, etc.). The
    /// cancel path uses this to walk the slot → PID
    /// mapping without re-reading the snapshot.
    fn pid_for_slot(&self, wave_id: &str, slot_index: u32) -> SupervisorStoreResult<Option<u32>>;

    /// Return the current slot/lifecycle snapshot for the phase
    /// decision pure function (U6).
    fn fan_in_status(&self, wave_id: &str) -> SupervisorStoreResult<WaveSnapshot>;

    /// Mark the wave's merged-to-events row so recovery (U11) does
    /// not double-inject `*.wave.complete`. Idempotent: repeated
    /// calls return `Ok(())`.
    fn mark_merge_to_events(&self, wave_id: &str) -> SupervisorStoreResult<()>;

    /// List every wave id known to the store, including Done/Failed.
    /// Used by the terminal cleanup finalizer (KTD8 / R13) so
    /// worktrees allocated by completed waves are still released.
    fn list_wave_ids(&self) -> SupervisorStoreResult<Vec<String>>;

    /// 2026-07-23-004 plan U2 (R-A2): resolve the
    /// store-assigned `wave_id` from the caller-supplied
    /// idempotency key, returning `None` when no wave was ever
    /// registered under that key. Implementations MUST back this
    /// with their persistent idempotency_key index (Memory:
    /// `waves_by_key`; rusqlite: `SELECT wave_id FROM waves`),
    /// so a process restart can rebuild the public→store map
    /// without observing the in-memory bridge cache.
    fn wave_id_for_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> SupervisorStoreResult<Option<String>>;

    /// Recover active waves on loop startup. Returns waves whose
    /// slot rows survived a crash (R-C3). Used by U11; does not
    /// touch DB state.
    fn recover_active_waves(&self) -> SupervisorStoreResult<Vec<WaveSnapshot>>;

    /// List the resource bindings a wave allocated (used by the
    /// integrator and the worktree cleanup at loop end).
    fn list_worktree_paths(&self, wave_id: &str) -> SupervisorStoreResult<Vec<SlotResource>>;

    /// 2026-07-03-001 plan U8: read a single slot's
    /// resource binding. Used by the rebind path to fetch
    /// the prior worktree path before calling
    /// `cleanup_worktree` (F-008). Returns `None` when the
    /// slot has not been bound yet (fresh wave, never
    /// bound) or when the slot doesn't exist on the wave.
    fn get_slot_resource(
        &self,
        wave_id: &str,
        slot_index: u32,
    ) -> SupervisorStoreResult<Option<SlotResource>>;

    /// Set the wave's phase directly. Used by the
    /// recovery path (U11) to mark in-flight waves past
    /// `aggregate_timeout_secs` as `Failed` (F-006 / R-C3).
    /// The store MUST NOT mutate the phase from any other
    /// entry point (KTD-8 — phase ownership is coordinator
    /// only).
    fn set_wave_phase(&self, wave_id: &str, phase: WavePhase) -> SupervisorStoreResult<()>;
}

pub use crate::worktree::Worktree;
pub use coordinator::{CoordinatorAction, SupervisorCoordinator};
pub use memory::InMemorySupervisorStore;
pub use merge_sink::{EventMergeSink, FileEventMergeSink, InMemoryMergeSink, MergeSinkError};
pub use phase::{FailedReason, PhaseDecision, PhaseInputs, evaluate_phase};
#[cfg(feature = "supervisor-db")]
pub use rusqlite::RusqliteSupervisorStore;
pub use worktree_bind::{
    DefaultWorktreeFactory, WorktreeBinding, WorktreeError, WorktreeFactory,
    assert_isolation_matches, bind_slot_worktree, env_keys as worktree_env_keys,
};

mod bridge;
mod coordinator;
mod memory;
#[cfg(test)]
mod memory_protocol_tests;
mod merge_sink;
#[cfg(feature = "supervisor-db")]
mod migrations;
mod phase;
mod recover;
#[cfg(feature = "supervisor-db")]
mod rusqlite;
#[cfg(test)]
mod types_tests;
pub mod worktree_bind;

// 2026-07-03-001 supervisor real-wiring: re-export the sunk-down
// bridge surface so `ralph-cli` and the BDD scenarios can depend on
// `ralph_core::supervisor::bridge::*` without duplicating the trait.
pub use bridge::{
    BridgeDispatchOutcome, BridgeError, InMemoryCoordinatorBridge, SlotBinding, SupervisorBridge,
    is_supervisor_path_enabled,
};

// 2026-07-03-001 supervisor real-wiring: expose the recovery
// entrypoint so the runner can call it once at startup before the
// loop accepts new events (U11 R-C3). The module itself stays
// private so its helpers (`merged_waves_skip_recovery`,
// `restore_unmerged_completed_slot`) do not leak into the public
// API; only the top-level orchestrator function is re-exported.
pub use recover::recover_active_waves_at_startup;

// U22: agent-safe supervisor summary surface for `ralph inspect loop`.
// Reads ONLY via the SupervisorStore trait (so the implementation
// decides where the data lives) and emits no internal paths or db
// handles — agents see `{ active_waves, queue_depth, slot_summary[],
// last_coordination_topics[] }` and nothing else.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SupervisorInspectSummary {
    /// Waves that have not yet reached a terminal phase.
    pub active_waves: Vec<ActiveWaveSummary>,
    /// Total non-terminal slot count across all active waves.
    pub queue_depth: u32,
    /// Per-slot digest (no `wave_id` map leaks beyond what the agent
    /// already knows via `task_id`). Empty when the supervisor
    /// reports zero or many active waves (the U8 contract surfaces
    /// "what's blocking my slot" only when the agent is reasonably
    /// looking at a single wave).
    pub slot_summary: Vec<SlotSummaryEntry>,
    /// Names of the supervisor coordination topics the active waves
    /// may emit (e.g. `exec.wave.complete` for an Exec wave).
    /// Derived purely from the `SUPERVISOR_COORDINATION_TOPICS`
    /// whitelist crossed with each active wave's kind — never read
    /// from the runtime event log or the store's internal ledger.
    /// Empty when no waves are active or when the active wave's
    /// kind has no coordination topics (currently all three kinds
    /// have entries, so the empty case only arises when there are
    /// zero active waves).
    pub last_coordination_topics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ActiveWaveSummary {
    pub wave_id: String,
    pub phase: crate::supervisor::WavePhase,
    pub pending_units: u32,
    pub done_units: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SlotSummaryEntry {
    pub slot_id: u32,
    pub hat: String,
    pub status: String,
}

/// Build a SupervisorInspectSummary from any SupervisorStore. Pure
/// reader — never mutates state. Hat-aware callers can post-filter
/// `slot_summary` by their own hat_id before display.
///
/// Field population contract (U8 of plan 2026-07-04-002):
/// - `active_waves`: one entry per non-terminal wave; mirrors the
///   `WaveSnapshot` rows from `recover_active_waves`.
/// - `queue_depth`: total non-terminal slot count across all active
///   waves (same definition as before).
/// - `slot_summary`: populated ONLY when exactly one wave is active
///   (the agent-safe "what is blocking my slot" contract). Each
///   entry carries `slot_id` (from `WaveSnapshot.slots`),
///   `status` (stringified `SlotStatus`) and a stable `hat` label
///   derived from the wave's `WaveKind`. When multiple waves are
///   active the field stays empty to avoid leaking a wave_id →
///   slot map the agent already knows through other channels.
/// - `last_coordination_topics`: derived from the
///   `SUPERVISOR_COORDINATION_TOPICS` whitelist crossed with each
///   active wave's `WaveKind`. This is the agent-safe summary of
///   "what coordination topics the supervisor may emit next" — it
///   intentionally does NOT read the runtime event log or any db
///   ledger (R11 output safety rule: no internal paths, no event
///   payload contents).
pub fn summarize(store: &dyn SupervisorStore) -> SupervisorInspectSummary {
    let snapshots = match store.recover_active_waves() {
        Ok(ws) => ws,
        Err(_) => return SupervisorInspectSummary::default(),
    };
    let mut out = SupervisorInspectSummary::default();
    for snap in &snapshots {
        let pending = snap.pending_count + snap.in_flight_count;
        let done = snap.completed_count + snap.failed_count;
        out.active_waves.push(ActiveWaveSummary {
            wave_id: snap.wave_id.clone(),
            phase: snap.phase,
            pending_units: pending,
            done_units: done,
        });
        out.queue_depth += pending;
        // Coordinate topics are derived deterministically from each
        // active wave's kind crossed with the supervisor coordination
        // whitelist — no store reads, no event-log reads, no internal
        // paths.
        for topic in coordination_topics_for_kind(snap.kind) {
            if !out.last_coordination_topics.iter().any(|t| t == topic) {
                out.last_coordination_topics.push((*topic).to_string());
            }
        }
    }
    // `slot_summary` requires a per-wave read; only populate when a
    // single active wave is present (the agent-safe `inspect loop`
    // contract is "what's blocking my slot", not "full state dump").
    if out.active_waves.len() == 1
        && let Some(snap) = snapshots.first()
    {
        let hat_label = wave_kind_hat_label(snap.kind);
        out.slot_summary = snap
            .slots
            .iter()
            .map(|(idx, status)| SlotSummaryEntry {
                slot_id: *idx,
                hat: hat_label.to_string(),
                status: status.to_string(),
            })
            .collect();
    }
    out
}

/// Stable string label for the "hat" side of a `SlotSummaryEntry`.
///
/// The supervisor store does not persist a per-slot hat id (slots are
/// identified by index + status only); the agent sees the wave kind as
/// the umbrella under which the slot lives. The mapping is a public
/// label — not an internal path — so it is safe to surface verbatim.
fn wave_kind_hat_label(kind: WaveKind) -> &'static str {
    match kind {
        WaveKind::Exec => "exec-worker",
        WaveKind::Fix => "fix-worker",
        WaveKind::Review => "review-worker",
    }
}

/// Subset of `SUPERVISOR_COORDINATION_TOPICS` that an active wave of
/// the given kind may emit. Pure derivation from the whitelist;
/// never reads the store.
fn coordination_topics_for_kind(kind: WaveKind) -> &'static [&'static str] {
    match kind {
        WaveKind::Exec => &["exec.wave.complete", "exec.wave.failed"],
        WaveKind::Fix => &["fix.wave.complete", "fix.wave.failed"],
        WaveKind::Review => &["review.wave.complete", "review.wave.failed"],
    }
}
