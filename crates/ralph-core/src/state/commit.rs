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
    /// a dedicated variant. The `counter` key is the field name
    /// in the snapshot (e.g. `"consecutive_failures"`,
    /// `"cumulative_cost"`).
    CounterChanged {
        counter: String,
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

    /// A coarse-grained `SnapshotReset` marker emitted on cold
    /// start (no data; just an anchor for log inspection).
    ///
    /// Not currently produced by the runtime; reserved for U3
    /// migration where the legacy `tasks.jsonl` will be replayed
    /// to seed the ledger and the resulting commits will be
    /// folded into a single `SnapshotReset` for forensic
    /// accounting.
    SnapshotReset,

    // ---- HashMap-shaped state: serialized as deltas on top of an
    // always-empty default snapshot. The replay loop applies the
    // delta on top of the running snapshot.
    /// Increment a per-hat activation counter.
    HatActivationCounted { hat: HatId, new_count: u32 },

    /// Mark a hat as exhausted (emitted `<hat>.exhausted`).
    HatExhausted { hat: HatId },

    /// Update a per-rejection-key last-iteration dedup entry.
    RejectionLastIteration {
        key: String,
        iteration: u32,
    },

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
    FlowLifecycleUpdated {
        flow_unit_id: String,
        phase: String,
    },

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
