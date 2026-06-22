//! Commit log types for [`StateLedger`].
//!
//! Plan ref: U1 of
//! `docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md`.
//!
//! Each [`Commit`] captures a single state mutation in the
//! orchestrator lifecycle. The commit is the *primary* persistence
//! unit — the snapshot in [`crate::state::LedgerSnapshot`] is a
//! pure projection derived by replaying the commit log. This module
//! defines the on-disk wire format (one JSONL record per line in
//! `.ralph/ledger.jsonl`).
//!
//! ## Conventions
//!
//! - The `sequence` field is monotonically increasing per ledger and
//!   is the canonical ordering key for replay. `iteration` is a
//!   secondary ordering axis (it is the per-loop iteration number
//!   from the `LoopState::iteration` field).
//! - `event_topic` is `Some` for commits that originate from a
//!   parsed event. `None` for engine-internal state changes
//!   (e.g. `StewardWoken`).
//! - All variants store only the *delta* — full state lives in the
//!   [`LedgerSnapshot`]. Do not introduce variants that re-snapshot
//!   whole sub-trees; that defeats the commit-log design.

use serde::{Deserialize, Serialize};

use ralph_proto::HatId;

/// One persisted state mutation.
///
/// Serialized as a single JSONL line in `.ralph/ledger.jsonl`. The
/// `(sequence)` field is the primary ordering key for replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commit {
    /// Loop iteration at which the commit was applied. Mirrors
    /// `LoopState::iteration` and resets to 0 across loop resumes.
    pub iteration: u32,
    /// Monotonically increasing commit sequence number. Unique per
    /// `StateLedger` instance. Starts at 0 for the first commit
    /// after `new()`. The value of `sequence` at any point equals
    /// the number of commits that have been applied to the ledger.
    pub sequence: u64,
    /// Wall-clock timestamp (RFC3339) when the commit was applied.
    pub timestamp: String,
    /// Topic of the originating event, if any. `None` for
    /// engine-internal state changes that are not derived from a
    /// parsed event (e.g. `StewardWoken`).
    pub event_topic: Option<String>,
    /// The delta describing what state changed.
    pub delta: CommitDelta,
}

impl Commit {
    /// Returns a no-op commit used as a sentinel for the
    /// `feature_enabled = false` path. The sentinel is never
    /// persisted.
    pub fn empty() -> Self {
        Self {
            iteration: 0,
            sequence: 0,
            timestamp: String::new(),
            event_topic: None,
            delta: CommitDelta::NoOp,
        }
    }
}

/// The kind of state mutation captured by a [`Commit`].
///
/// New variants are added as new state moves into the ledger. The
/// exhaustive match in [`super::snapshot::LedgerSnapshot::apply_delta`]
/// (and tests) is the SSOT for "every delta must be handled".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommitDelta {
    /// Marker for the no-op commit used when
    /// `feature_enabled = false`. Never serialized to disk in
    /// production paths.
    NoOp,

    /// Task lifecycle transition. Emitted by the state projector
    /// when a `work.ready` / `work.done` / `work.failed` event
    /// mutates the task ledger.
    TaskLifecycle {
        task_id: String,
        transition: TaskTransition,
    },

    /// Insert a brand-new task into the task ledger. Emitted by
    /// the projector on `work.ready` for tasks that the snapshot
    /// does not yet know about. U2 of plan
    /// 2026-06-21-002 closes the loop between
    /// `apply_from_ledger` and the projector — the projector
    /// previously inserted tasks implicitly via `TaskStore::ensure`
    /// inside the event-batch path; the ledger path needs an
    /// explicit delta so `replay_from_disk` can rebuild the same
    /// state from the commit log alone.
    TaskInserted { task: crate::task::Task },

    /// Progress marker update. The `step` is appended to
    /// `LedgerSnapshot::progress.completed_steps` (idempotent).
    ProgressUpdate {
        completed_step: Option<String>,
        current_step: Option<String>,
    },

    /// Plan complete: every open task is closed in the task
    /// snapshot and the progress snapshot is finalised.
    PlanComplete {
        final_step: Option<String>,
        closed_count: u32,
    },

    /// A rejection was recorded. The key is the rejection class
    /// string (e.g. `stage:source_hat:topic:violation`).
    RejectionRecorded {
        key: String,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        topic: Option<String>,
    },

    /// A rejection's retry budget was exhausted; the runner must
    /// fail-closed.
    RejectionBudgetTripped {
        key: String,
        terminal_reason: String,
    },

    /// A `*.handoff.accepted` event was processed.
    HandoffAccepted {
        from: HatId,
        to: HatId,
        #[serde(default)]
        handoff_path: Option<String>,
    },

    /// A workflow chain advanced to a new phase.
    WorkflowPhaseAdvanced {
        chain_name: String,
        #[serde(default)]
        instance_key: Option<String>,
        new_phase: u32,
    },

    /// Generic counter mutation. Used for all `u32` / `i64` /
    /// `f64` scalar fields in `LedgerSnapshot` that do not warrant
    /// a dedicated variant. The `counter` is the field name
    /// in the snapshot (e.g. `"consecutive_failures"`,
    /// `"cumulative_cost"`); the runtime uses [`CounterKind`] so
    /// callers cannot fat-finger a counter name. The on-disk
    /// representation is still a snake_case string (one-per-line
    /// in `ledger.jsonl`) so the wire format is unchanged across
    /// the U1→U1.1 migration; only the Rust API becomes
    /// type-safe.
    CounterChanged {
        counter: CounterKind,
        new_value: i64,
    },

    /// A new topic was observed by the loop.
    SeenTopic { topic: String },

    /// The `## ORCHESTRATOR CONTEXT` `completion_requested` flag
    /// flipped to `true`.
    CompletionRequested,

    /// The `completion_honored` flag flipped to `true`.
    CompletionHonored,

    /// A `loop.cancel` event was observed.
    CancellationRequested,

    /// The progress-steward hat was woken this turn.
    StewardWoken,

    // ---- HashMap-shaped state: serialized as deltas on top of an
    // always-empty default snapshot. The replay loop applies the
    // delta on top of the running snapshot.
    /// Increment a per-hat activation counter.
    HatActivationCounted { hat: HatId, new_count: u32 },

    /// Mark a hat as exhausted (emitted `<hat>.exhausted`).
    HatExhausted { hat: HatId },

    /// Update a per-rejection-key last-iteration dedup entry.
    RejectionLastIteration { key: String, iteration: u32 },

    /// Increment a per-stall recovery counter.
    StallRecoveryCounted { key: String, new_count: u32 },

    /// Increment a per-task thrash counter.
    TaskBlockCounted { task_id: String, new_count: u32 },

    /// Mark a task as abandoned.
    TaskAbandoned { task_id: String },

    /// Per-step review terminal state mutation. The exact state
    /// is reconstructed by `LedgerSnapshot::apply_delta` from the
    /// encoded fields; a more compact encoding will be added once
    /// U5 lands and reveals the hot fields.
    ReviewStepUpdated {
        plan_name: String,
        task_id: String,
        step: String,
        synth_pass: bool,
        #[serde(default)]
        synth_terminal: Option<String>,
    },

    /// Per-loop handoff deadline tracker mutation.
    HandoffTrackerUpdated {
        event_id: String,
        accepted: bool,
        #[serde(default)]
        escalation_reason: Option<String>,
    },

    /// FlowLifecycle registry mutation.
    FlowLifecycleUpdated { flow_unit_id: String, phase: String },

    /// `recent_rejection_digest` mutation. Carries the full
    /// entry (the digest is BTreeMap-shaped with at most 5 keys,
    /// so per-entry storage is cheap).
    RejectionDigestUpdated {
        reason_code: String,
        count: u32,
        last_message: String,
        last_ts: String,
        last_topic: String,
    },
}

/// Task state transition kind. Mirrors the lifecycle in
/// [`crate::task::TaskStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTransition {
    /// Task was inserted (mirrors `work.ready`).
    Opened,
    /// Task was started (transitioned to in-progress).
    Started,
    /// Task was closed successfully (mirrors `work.done`).
    Closed,
    /// Task was failed (mirrors `work.failed`).
    Failed,
    /// Task was reopened (transient failure → retry).
    Reopened,
}

/// Type-safe handle on a [`CommitDelta::CounterChanged`] target
/// field.
///
/// Each variant corresponds 1:1 to a counter on
/// [`crate::state::snapshot::LedgerSnapshot`]. The on-disk wire
/// format is the snake_case variant name (one string per
/// commit), so old log files keep replaying without a migration
/// step. The Rust API now rejects mistyped counter names at
/// compile time — a regression observed in
/// `2026-06-21-002-adversarial-review.md` P2-#3 where the
/// previous `&str` dispatch silently no-op'd on unknown
/// strings.
///
/// Use [`CounterKind::from_str_lossy`] to convert a raw string
/// (e.g. one read from a log file outside the typed API) into
/// the enum; unknown strings map to [`CounterKind::Unknown`]
/// (also serialized as the original string) so the rule of
/// "unknown counter is best-effort no-op" is preserved.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CounterKind {
    /// `LedgerSnapshot::iteration`.
    Iteration,
    /// `LedgerSnapshot::hat_handoff_seq`.
    HatHandoffSeq,
    /// `LedgerSnapshot::consecutive_failures`.
    ConsecutiveFailures,
    /// `LedgerSnapshot::consecutive_blocked`.
    ConsecutiveBlocked,
    /// `LedgerSnapshot::abandoned_task_redispatches`.
    AbandonedTaskRedispatches,
    /// `LedgerSnapshot::consecutive_malformed_events`.
    ConsecutiveMalformedEvents,
    /// `LedgerSnapshot::consecutive_hard_gates`.
    ConsecutiveHardGates,
    /// `LedgerSnapshot::consecutive_same_signature`.
    ConsecutiveSameSignature,
    /// `LedgerSnapshot::consecutive_no_progress_turns`.
    ConsecutiveNoProgressTurns,
    /// `LedgerSnapshot::consecutive_steward_activations`.
    ConsecutiveStewardActivations,
    /// `LedgerSnapshot::consecutive_completion_rejections`.
    ConsecutiveCompletionRejections,
    /// `LedgerSnapshot::consecutive_engine_gate_rejections`.
    ConsecutiveEngineGateRejections,
    /// `LedgerSnapshot::invariant_violation_count`.
    InvariantViolationCount,
    /// `LedgerSnapshot::last_rejection_fingerprint`.
    LastRejectionFingerprint,
    /// `LedgerSnapshot::cumulative_cost`. Stored as `i64` in the
    /// commit log and widened to `f64` on apply.
    CumulativeCost,
    /// Catch-all for unknown / forward-compat counter names.
    /// Serialized as the original snake_case string so the
    /// replay is lossless even when the typed enum gains new
    /// variants in a future release.
    #[serde(untagged)]
    Unknown(String),
}

impl CounterKind {
    /// Parse a counter name from a raw `&str`. Unknown names map
    /// to [`CounterKind::Unknown`].
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "iteration" => Self::Iteration,
            "hat_handoff_seq" => Self::HatHandoffSeq,
            "consecutive_failures" => Self::ConsecutiveFailures,
            "consecutive_blocked" => Self::ConsecutiveBlocked,
            "abandoned_task_redispatches" => Self::AbandonedTaskRedispatches,
            "consecutive_malformed_events" => Self::ConsecutiveMalformedEvents,
            "consecutive_hard_gates" => Self::ConsecutiveHardGates,
            "consecutive_same_signature" => Self::ConsecutiveSameSignature,
            "consecutive_no_progress_turns" => Self::ConsecutiveNoProgressTurns,
            "consecutive_steward_activations" => Self::ConsecutiveStewardActivations,
            "consecutive_completion_rejections" => Self::ConsecutiveCompletionRejections,
            "consecutive_engine_gate_rejections" => Self::ConsecutiveEngineGateRejections,
            "invariant_violation_count" => Self::InvariantViolationCount,
            "last_rejection_fingerprint" => Self::LastRejectionFingerprint,
            "cumulative_cost" => Self::CumulativeCost,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// Borrow the wire-format string. For `Unknown` variants the
    /// original string is returned verbatim.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Iteration => "iteration",
            Self::HatHandoffSeq => "hat_handoff_seq",
            Self::ConsecutiveFailures => "consecutive_failures",
            Self::ConsecutiveBlocked => "consecutive_blocked",
            Self::AbandonedTaskRedispatches => "abandoned_task_redispatches",
            Self::ConsecutiveMalformedEvents => "consecutive_malformed_events",
            Self::ConsecutiveHardGates => "consecutive_hard_gates",
            Self::ConsecutiveSameSignature => "consecutive_same_signature",
            Self::ConsecutiveNoProgressTurns => "consecutive_no_progress_turns",
            Self::ConsecutiveStewardActivations => "consecutive_steward_activations",
            Self::ConsecutiveCompletionRejections => "consecutive_completion_rejections",
            Self::ConsecutiveEngineGateRejections => "consecutive_engine_gate_rejections",
            Self::InvariantViolationCount => "invariant_violation_count",
            Self::LastRejectionFingerprint => "last_rejection_fingerprint",
            Self::CumulativeCost => "cumulative_cost",
            Self::Unknown(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for CounterKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for CounterKind {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from_str_lossy(s))
    }
}
