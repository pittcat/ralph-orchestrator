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
//! - The mapping is **preset-driven** (declarative YAML via
//!   `actions` / `actions_chain`), not hard-coded. Topics outside
//!   the configured key set are inert no-ops per the action-key
//!   authority gate (see plan §4.2).
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
    /// `ce-executor-serial` and `ce-executor-serial` presets opt
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
    /// `actions` is the legacy single-action form: the projector
    /// wraps each entry in a one-element chain and dispatches in
    /// insertion order. The chain form (`actions_chain`) is
    /// preferred for any topic that needs ordered multi-step
    /// projection (e.g. `work.done` → `close_task` →
    /// `mark_step_completed`); plan 2026-06-20-001 U3b.
    ///
    /// See [`StateProjectionAction`] for the available actions.
    #[serde(default)]
    pub actions: HashMap<String, StateProjectionAction>,

    /// Per-topic action chain (plan 2026-06-20-001 U3b).
    /// When present for a topic, the projector dispatches each
    /// `StateProjectionAction` in array order. Order is semantic:
    /// for `work.done`, `close_task` MUST run before
    /// `mark_step_completed` so the progress gate always finds the
    /// step. `preset_lint` asserts the order at build time
    /// (KTD-3).
    ///
    /// Topics present in `actions_chain` take precedence over the
    /// legacy `actions` map (engine reads `actions_chain` first
    /// and falls back to `actions` for missing keys only).
    #[serde(default)]
    pub actions_chain: HashMap<String, Vec<StateProjectionAction>>,
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
    /// Atomically create or reuse a task DAG from one event payload.
    ///
    /// U2 of plan 2026-07-29-001
    /// (`fix-parallel-forge-static-wave-settlement-plan`): when
    /// `execution_wave` and `integration_order` pointers are supplied,
    /// the projector validates the static wave schedule before any task
    /// is written. Unset pointers keep the legacy DAG-only behaviour so
    /// non-Parallel-Forge presets continue to work without changes.
    EnsureTaskBatch {
        /// JSON pointer for the non-empty array of task specifications.
        items: String,
        /// JSON pointer for the declared item count.
        count: String,
        /// Item-relative JSON pointer for each stable task key.
        key: String,
        /// Item-relative JSON pointer for each non-empty title.
        title: String,
        /// Item-relative JSON pointer for dependency task keys.
        blocked_by_keys: String,
        /// U2: optional item-relative JSON pointer for the static
        /// `execution_wave` (positive integer). When present together
        /// with `integration_order`, the projector validates the wave
        /// schedule before any task write. When `None`, the projector
        /// skips wave/order validation entirely (legacy path).
        #[serde(default)]
        execution_wave: Option<String>,
        /// U2: optional item-relative JSON pointer for the static
        /// `integration_order` (positive integer). See
        /// `execution_wave` for the dual-pointer semantics.
        #[serde(default)]
        integration_order: Option<String>,
        /// U2: optional top-level JSON pointer for the execution
        /// plan digest. When the pointer resolves, the projector
        /// cross-checks the digest against the payload's declared
        /// value; a mismatch rejects the batch atomically. `None`
        /// skips the digest cross-check.
        #[serde(default)]
        execution_plan_digest: Option<String>,
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
    /// U3 of plan 2026-07-29-001
    /// (`fix-parallel-forge-static-wave-settlement-plan`):
    /// atomically close all tasks listed in a single
    /// `forge.wave.settled` payload. Distinct from `CloseTask`
    /// (which closes a single task per event): `CloseTaskBatch`
    /// is the **only** state-authority path that closes more
    /// than one task at once, and it must keep the ledger
    /// consistent under failure — empty input, duplicate IDs,
    /// unknown IDs, mixed open/closed identities, or replay
    /// mismatches all reject before any task row is touched.
    /// Re-applying the same already-closed payload is a
    /// idempotent no-op.
    CloseTaskBatch {
        /// JSON pointer for the non-empty array of task IDs to
        /// close. Duplicate IDs fail-close; empty arrays fail-close.
        task_ids: String,
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
    /// Mark a step as completed in `progress.md`'s
    /// `## Completed Steps` heading without closing any task.
    ///
    /// Plan ref: U3a of `docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md`
    /// (P0-A fix for `ce-executor-serial-primary-20260619`).
    ///
    /// Distinct from `CloseTask` (which closes a task AND appends
    /// the step). `MarkStepCompleted` is the orchestrator's safety
    /// net: even when a `CloseTask` action would have written the
    /// step, a subsequent `MarkStepCompleted` re-pins it so
    /// `progress_task_gate` always finds the step. The `work.done`
    /// action chain is `close_task` → `mark_step_completed`
    /// (R3/R4, KTD-3 order assertion).
    MarkStepCompleted {
        /// JSON pointer for the step label. Defaults to `step`.
        #[serde(default)]
        step: Option<String>,
    },
    /// U3 of plan 2026-07-05-005: `review.dimensions.complete`
    /// lands on the projector. The action records the latest
    /// review summary so the next `## ORCHESTRATOR CONTEXT`
    /// block can include a `## REVIEW SUMMARY` section without
    /// re-reading `events.jsonl`. The projector never dedups —
    /// `event_policy` already de-duplicates by
    /// `(task_key, fix_round)` upstream, so the action runs at
    /// most once per dedup window.
    ///
    /// Schema fields are JSON pointers (default = literal field
    /// name) so the action is payload-shape agnostic. `task_key`
    /// is the stable identity (NOT `task_id`, which can be reused
    /// across fix-rounds); `fix_round` is the loop counter; the
    /// remaining pointers feed the review summary block.
    ReviewDimensionsComplete {
        /// JSON pointer for the stable task key.
        #[serde(default)]
        task_key: Option<String>,
        /// JSON pointer for the fix-round counter.
        #[serde(default)]
        fix_round: Option<String>,
        /// JSON pointer for the per-dimension verdict map.
        #[serde(default)]
        dimensions: Option<String>,
        /// JSON pointer for the human-readable summary line.
        #[serde(default)]
        summary: Option<String>,
    },
}
