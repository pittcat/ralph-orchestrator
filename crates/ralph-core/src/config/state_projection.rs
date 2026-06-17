//! State projection configuration (Phase 1 of the north-star plan).
//!
//! Plan ref: `docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md`
//!
//! When `enabled` is `true`, the event loop projects the canonical
//! `.ralph/agent/tasks.jsonl` and `.ralph/agent/progress.md` ledgers
//! from the inbound event batch **before** the `progress_task_gate`
//! runs. Both ledgers stay aligned with the event stream and the
//! agent never has to hand-write either file (the agent sees
//! the projected state through the `## ORCHESTRATOR CONTEXT` block
//! in the prompt — see U4).
//!
//! Phase 1 keeps the surface area deliberately narrow:
//! - The mapping is **preset-driven** (declarative YAML), not hard-coded.
//! - The projection runs **only** on the topics in
//!   [`crate::state_projector::PROJECTED_TOPICS`]; all others are
//!   no-ops.
//! - Failures are **fail-closed** — a malformed event payload that
//!   cannot be projected drops the event with a diagnostic (see U1
//!   risk note).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Top-level state-projection configuration. Embeds inside
/// `EventLoopConfig` (see U1 of the plan).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateProjectionConfig {
    /// Master switch. When `false` (the default) the projector is
    /// not invoked and existing behaviour is preserved. The
    /// `ce-executor-isolated` and `ce-executor-serial` presets opt
    /// in via `event_loop.state_projection.enabled: true`.
    #[serde(default)]
    pub enabled: bool,

    /// Per-topic action table. Keys are event topic names (e.g.
    /// `work.ready`, `work.done`, `queue.advance`, `plan.complete`).
    /// Topics absent from this map are inert (the projector does not
    /// touch the ledgers for them). This keeps the Phase 1 surface
    /// narrow: we can ship the projector with an empty map and
    /// presets opt into specific topics only.
    ///
    /// See [`StateProjectionAction`] for the available actions.
    #[serde(default)]
    pub actions: HashMap<String, StateProjectionAction>,
}

/// One projection rule. Each variant maps a topic to a single
/// ledger mutation. Keep this enum small and explicit — the
/// projector (not the config) is the source of truth for *how*
/// the mutation is performed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StateProjectionAction {
    /// Create a task with the given fields, or refresh its metadata
    /// if a task with the same `key` already exists. Driven by
    /// `work.ready` (typical) or any topic that announces a new
    /// unit of work.
    EnsureTask {
        /// JSON pointer (relative to the event payload) for the
        /// stable task key. The projector reads the value at
        /// `payload[<pointer>]` and passes it to
        /// `TaskStore::ensure`. Required: a missing key is a
        /// fail-closed reject.
        key: String,
        /// JSON pointer for the task `title` (defaults to `step`
        /// when absent, falling back to `key`).
        #[serde(default)]
        title: Option<String>,
    },
    /// Mark a task as closed. Driven by `work.done`. The
    /// projector also flips progress's `Completed Steps`
    /// when a `step` payload field is present.
    CloseTask {
        /// JSON pointer for `task_id`. **Required** — a `work.done`
        /// without `task_id` is rejected (fail-closed, see U2).
        task_id: String,
        /// JSON pointer for the step label to mark in progress
        /// (defaults to `step`). When absent the projector only
        /// updates the task ledger and skips the progress write.
        #[serde(default)]
        step: Option<String>,
    },
    /// Advance the plan's `Current Step` heading in
    /// `progress.md`. Driven by `queue.advance`.
    AdvanceStep {
        /// JSON pointer for the new current step (default `step`).
        #[serde(default)]
        current_step: Option<String>,
        /// JSON pointer for the step that was just completed
        /// (default `completed_step`, falling back to `step`).
        #[serde(default)]
        completed_step: Option<String>,
    },
    /// Finalize the plan: closes all open tasks, marks the last
    /// step as completed, writes a Plan Status banner. Driven by
    /// `plan.complete`.
    PlanComplete {
        /// Optional JSON pointer for the final step label.
        #[serde(default)]
        final_step: Option<String>,
    },
}
