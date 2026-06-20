//! State projection — single-writer for `.ralph/agent/tasks.jsonl`
//! and `.ralph/agent/progress.md`.
//!
//! Plan ref: `docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md`.
//!
//! The module has three public entry points:
//! - [`StateProjector::apply`] — main hook. Called from
//!   `process_parse_result` **after** the state machine validates
//!   the batch and **before** `apply_step_handoff_gate` (SP-R8).
//!   Returns an [`ApplyReport`] so the caller can fail-closed on
//!   individual events.
//! - [`StateProjector::bootstrap_from_disk`] — Unit 6 entry point
//!   used on loop resume. Loads the canonical state into the
//!   in-memory cache so the first emit only applies *deltas*.
//! - [`StateProjector::snapshot`] — Unit 4 entry point. Builds a
//!   read-only [`RuntimeStateSnapshot`] for the
//!   `## ORCHESTRATOR CONTEXT` block. (Implemented in U4; here we
//!   expose the type only.)
//!
//! The projector never owns the ledgers' on-disk format. It
//! delegates writing to [`task::project`] (Unit 2) and
//! [`progress::project`] (Unit 3). Both submodules share the same
//! [`ProjectionContext`] so they see the same view of the
//! workspace.
//!
//! ## Known limitation: cross-loop cache staleness
//!
//! The in-memory `tasks_cache` / `progress_cache` live on the
//! loop's `LoopState` and survive across iterations. The cold-start
//! read in `apply` only fires when the cache is empty. If a
//! separate loop (e.g. an operator CLI) mutates the ledger out of
//! band, this loop's cache will lag the disk until the next
//! `bootstrap_from_disk` call. Phase 1 assumes worktree-isolated
//! loops; cross-loop invalidation is a Phase 2 concern (see plan
//! "Risks & Dependencies" table). The `project_ensure_task` and
//! `project_close_task` helpers re-read disk on every call, which
//! partially mitigates the risk for the most common path.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::config::StateProjectionConfig;
use crate::event_reader::Event;
use crate::step_handoff::ProgressSnapshot;

/// Topics the projector inspects. Other topics are inert (no
/// projection). The list is locked in a unit test so a future
/// refactor cannot silently widen the surface.
///
/// R6 (2026-06-17-005 fix plan): `review.passed` / `review.failed`
/// / `plan.blocked` were declared in this list during Phase 1 but
/// have **no** `StateProjectionAction` mapping — they would have
/// been inert no-ops in practice. They are removed to keep the
/// declared surface in lock-step with the implementation. A future
/// Phase 2 unit that needs them must add the matching
/// `StateProjectionAction` variant *and* the topic here in the
/// same commit.
pub const PROJECTED_TOPICS: &[&str] = &[
    "work.ready",
    "work.done",
    "queue.advance",
    "plan.complete",
];

/// Build the canonical path to the task ledger under a workspace
/// root. Phase 1 mandates the legacy `.ralph/agent/tasks.jsonl`
/// path; we do **not** introduce a new path here.
pub fn tasks_path(workspace: &Path) -> PathBuf {
    workspace.join(".ralph").join("agent").join("tasks.jsonl")
}

/// Build the canonical path to the progress ledger. Phase 1
/// deprecates the legacy `.agents/scratchpad/.../progress.md`
/// path that older presets used.
pub fn progress_path(workspace: &Path) -> PathBuf {
    workspace.join(".ralph").join("agent").join("progress.md")
}

/// Context passed to every projection call. Holds paths to the
/// canonical ledgers, the in-memory task/progress caches, and
/// the projection config. The caches let the projector emit
/// diff-only writes and feed the `ORCHESTRATOR CONTEXT` snapshot
/// without re-reading disk.
#[derive(Debug)]
pub struct ProjectionContext {
    /// Workspace root (used to derive ledger paths when callers
    /// pass an explicit override).
    pub workspace_root: PathBuf,
    /// Tasks ledger path. Defaults to
    /// `workspace_root/.ralph/agent/tasks.jsonl`.
    pub tasks_path: PathBuf,
    /// Progress ledger path. Defaults to
    /// `workspace_root/.ralph/agent/progress.md`.
    pub progress_path: PathBuf,
    /// Projection config from the loop config.
    pub config: StateProjectionConfig,
    /// Whether R4 (current unit gating) is enforced in this loop.
    /// Mirrors `EventLoopConfig.enforce_current_unit` so the
    /// projector matches loop behaviour: a `work.ready` for a
    /// non-current U is rejected when this is `true`. Default is
    /// `false` to preserve pre-Phase-1 behaviour.
    ///
    /// Plan ref: R1 in
    /// `docs/plans/2026-06-17-005-fix-state-projection-phase1-review-findings-plan.md`.
    pub enforce_current_unit: bool,
    /// In-memory cache of the tasks ledger. Populated by
    /// [`StateProjector::bootstrap_from_disk`] on loop resume; kept
    /// in sync by [`task::project`] on every apply.
    pub tasks_cache: Vec<crate::task::Task>,
    /// In-memory cache of the progress ledger. Same lifecycle as
    /// `tasks_cache`.
    pub progress_cache: ProgressSnapshot,
}

impl ProjectionContext {
    /// Build a context rooted at the given workspace, with default
    /// ledger paths and an empty cache. The caller is responsible
    /// for calling [`StateProjector::bootstrap_from_disk`] before
    /// applying any events (otherwise the cache is rebuilt on
    /// demand from the live ledger).
    ///
    /// `enforce_current_unit` mirrors the loop's R4 setting so
    /// `TaskStore::ensure` rejects sibling-unit `work.ready`
    /// payloads when the loop does. The 2-arg overload keeps
    /// pre-Phase-1 behaviour (R4 disabled inside the projector)
    /// for tests and any caller that has not yet threaded the
    /// loop config through.
    pub fn new(
        workspace_root: &Path,
        config: StateProjectionConfig,
        enforce_current_unit: bool,
    ) -> Self {
        Self {
            workspace_root: workspace_root.to_path_buf(),
            tasks_path: tasks_path(workspace_root),
            progress_path: progress_path(workspace_root),
            config,
            enforce_current_unit,
            tasks_cache: Vec::new(),
            progress_cache: ProgressSnapshot::default(),
        }
    }

    /// Backward-compatible constructor with `enforce_current_unit=false`.
    /// Used by tests and any caller that has not yet threaded the
    /// loop config through. The loop's primary entry point uses
    /// [`Self::new`] with the live `EventLoopConfig.enforce_current_unit`.
    ///
    /// Not marked `#[deprecated]` because the project standard
    /// (`CLAUDE.md`) is "Backwards compatibility doesn't matter";
    /// the plan-authorising call is the `ProjectionContext` doc
    /// comment in `2026-06-17-005`. Phase 2 should remove this
    /// helper alongside the `enforce_current_unit` field itself.
    pub fn new_legacy(workspace_root: &Path, config: StateProjectionConfig) -> Self {
        Self::new(workspace_root, config, false)
    }
}

/// Result of a single `apply` call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyReport {
    /// Number of events that successfully updated a ledger.
    pub applied: usize,
    /// Number of events that were dropped (fail-closed) due to a
    /// projection error (missing field, malformed payload, etc.).
    pub rejected: usize,
    /// Per-event rejection reasons for the bus diagnostic. The
    /// caller publishes one `event.state_projection.rejected` event
    /// per entry.
    pub rejections: Vec<Rejection>,
}

/// One row of [`ApplyReport::rejections`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    pub topic: String,
    pub reason: String,
    /// Per-event payload snapshot used as a tie-breaker when
    /// several events of the same topic appear in a single batch.
    /// `None` when the source event had no payload (e.g. a bare
    /// `task.resume`). The event loop uses
    /// `(topic, payload_text)` as the unique key for
    /// `events.retain` so a single reject does not drop sibling
    /// events of the same topic in the same batch — a regression
    /// that previously wiped out the whole batch on one bad
    /// event. P0 fix — see review notes.
    pub payload: Option<String>,
}

/// Top-level projector. Cheap to construct; holds the running
/// context.
#[derive(Debug)]
pub struct StateProjector {
    ctx: ProjectionContext,
}

impl StateProjector {
    /// Create a new projector. The context is initialized with
    /// empty caches; call [`Self::bootstrap_from_disk`] before the
    /// first `apply` if you need the cache to reflect on-disk
    /// state (Unit 6).
    pub fn new(ctx: ProjectionContext) -> Self {
        Self { ctx }
    }

    /// Replace the in-memory caches with the on-disk ledgers.
    ///
    /// Called once on loop resume (Unit 6). When the caches are
    /// empty, the projector falls back to reading the live
    /// ledger for every apply — that is correct but slower, so
    /// production code should bootstrap eagerly.
    pub fn bootstrap_from_disk(&mut self) -> std::io::Result<()> {
        let store = crate::task_store::TaskStore::load(&self.ctx.tasks_path)?;
        self.ctx.tasks_cache = store.all().to_vec();
        let content = std::fs::read_to_string(&self.ctx.progress_path).unwrap_or_default();
        self.ctx.progress_cache = ProgressSnapshot::parse(&content);
        debug!(
            tasks = self.ctx.tasks_cache.len(),
            "state projector bootstrap complete"
        );
        Ok(())
    }

    /// Borrow the running context. Used by the snapshot builder
    /// (U4) and by tests.
    pub fn context(&self) -> &ProjectionContext {
        &self.ctx
    }

    /// Override the R4 (`enforce_current_unit`) flag after
    /// construction. Production code sets this via
    /// [`ProjectionContext::new`]; this helper exists so the R1
    /// regression matrix can flip the flag in tests without
    /// rebuilding the context from scratch.
    #[doc(hidden)]
    pub fn with_enforce_current_unit(mut self, enforce_current_unit: bool) -> Self {
        self.ctx.enforce_current_unit = enforce_current_unit;
        self
    }

    /// Apply a batch of events to the ledgers. Events whose topic
    /// is not in [`PROJECTED_TOPICS`] (or for which the config has
    /// no matching action) are passed through without touching
    /// disk. Events that fail to project are recorded as
    /// `rejected` and the function returns an [`ApplyReport`]; the
    /// caller decides whether to drop those events from the bus
    /// (Phase 1: drop + emit `event.state_projection.rejected`).
    pub fn apply(&mut self, events: &[Event]) -> ApplyReport {
        if !self.ctx.config.enabled || self.ctx.config.actions.is_empty() {
            return ApplyReport::default();
        }

        let mut report = ApplyReport::default();
        for event in events {
            // Cold-start the cache lazily. We deliberately do not
            // call `bootstrap_from_disk` here because that returns
            // an `io::Error` we cannot surface per-event; the
            // apply loop must remain infallible.
            if self.ctx.tasks_cache.is_empty() {
                if let Ok(store) = crate::task_store::TaskStore::load(&self.ctx.tasks_path) {
                    self.ctx.tasks_cache = store.all().to_vec();
                }
            }
            if self.ctx.progress_cache.completed_steps.is_empty()
                && self.ctx.progress_cache.current_step.is_none()
            {
                let content = std::fs::read_to_string(&self.ctx.progress_path).unwrap_or_default();
                self.ctx.progress_cache = ProgressSnapshot::parse(&content);
            }

            if !PROJECTED_TOPICS.contains(&event.topic.as_str()) {
                continue;
            }
            // Resolve the action chain for this topic.
            // `actions_chain` (plan 2026-06-20-001 U3b) takes
            // precedence over the legacy `actions` map; missing
            // keys fall back to wrapping the legacy single action
            // in a one-element vec. Order of the chain is
            // semantic — `preset_lint` asserts `work.done`'s
            // `close_task` precedes `mark_step_completed`.
            let topic_str = event.topic.to_string();
            let chain: Vec<crate::config::StateProjectionAction> =
                if let Some(chain) = self.ctx.config.actions_chain.get(&topic_str) {
                    chain.clone()
                } else if let Some(single) = self.ctx.config.actions.get(&topic_str) {
                    vec![single.clone()]
                } else {
                    continue;
                };
            let payload = event.payload.as_deref().unwrap_or("");
            let parsed: serde_json::Value = match serde_json::from_str(payload) {
                Ok(v) => v,
                Err(e) => {
                    report.rejections.push(Rejection {
                        topic: event.topic.clone(),
                        reason: format!("payload_parse_error: {e}"),
                        payload: event.payload.clone(),
                    });
                    report.rejected += 1;
                    continue;
                }
            };
            // Dispatch the chain in order. Failure of any step
            // short-circuits the rest of the chain (best-effort
            // commit) and is recorded against the topic. KTD-3
            // ensures the YAML order is correct, so the typical
            // chain (close_task → mark_step_completed) only
            // reaches `mark_step_completed` after the task close
            // succeeded.
            let mut chain_failed = false;
            for action in chain {
                if chain_failed {
                    break;
                }
                let outcome = match action {
                    crate::config::StateProjectionAction::EnsureTask { key, title } => {
                        crate::state_projector::task::project_ensure_task(
                            &mut self.ctx,
                            &parsed,
                            &key,
                            title.as_deref(),
                        )
                    }
                    crate::config::StateProjectionAction::CloseTask { task_id, step } => {
                        crate::state_projector::task::project_close_task(
                            &mut self.ctx,
                            &parsed,
                            &task_id,
                            step.as_deref(),
                        )
                    }
                    crate::config::StateProjectionAction::AdvanceStep {
                        current_step,
                        completed_step,
                    } => crate::state_projector::progress::project_advance_step(
                        &mut self.ctx,
                        &parsed,
                        current_step.as_deref(),
                        completed_step.as_deref(),
                    ),
                    crate::config::StateProjectionAction::PlanComplete { final_step } => {
                        crate::state_projector::progress::project_plan_complete(
                            &mut self.ctx,
                            &parsed,
                            final_step.as_deref(),
                        )
                    }
                    crate::config::StateProjectionAction::MarkStepCompleted { step } => {
                        crate::state_projector::progress::project_mark_step_completed(
                            &mut self.ctx,
                            &parsed,
                            step.as_deref(),
                        )
                    }
                };
                match outcome {
                    Ok(()) => {
                        report.applied += 1;
                    }
                    Err(reason) => {
                        warn!(topic = %event.topic, reason, "state projection rejected event");
                        report.rejections.push(Rejection {
                            topic: event.topic.clone(),
                            reason,
                            payload: event.payload.clone(),
                        });
                        report.rejected += 1;
                        chain_failed = true;
                    }
                }
            }
        }
        report
    }
}

/// Read the canonical ledgers from disk and return a fresh
/// `(tasks, progress)` pair. Used by [`RuntimeStateSnapshot`]
/// when its in-memory cache is cold, and by tests that need a
/// "load everything" helper without going through the projector.
/// Best-effort: missing files → empty `(Vec::new(), default)`.
/// Real I/O errors (permissions, corruption) on tasks.jsonl
/// surface as an empty task list — same as
/// [`TaskStore::load`]'s contract — so the snapshot degrades
/// gracefully rather than panicking in the prompt path.
pub fn read_state_from_disk(workspace: &Path) -> (Vec<crate::task::Task>, ProgressSnapshot) {
    let tasks = crate::task_store::TaskStore::load(&tasks_path(workspace))
        .map(|s| s.all().to_vec())
        .unwrap_or_default();
    let content = std::fs::read_to_string(&progress_path(workspace)).unwrap_or_default();
    let progress = ProgressSnapshot::parse(&content);
    (tasks, progress)
}

/// Read a JSON pointer (e.g. `"step"`, `"payload.title"`) from a
/// parsed JSON value. Returns `None` when the path is missing or
/// the value is not a string. Empty pointer returns the whole
/// payload as a string (best-effort via `to_string`).
pub(crate) fn json_pointer<'a>(value: &'a serde_json::Value, pointer: &str) -> Option<&'a str> {
    if pointer.is_empty() {
        return value.as_str();
    }
    let mut current = value;
    for segment in pointer.split('.') {
        current = current.get(segment)?;
    }
    current.as_str()
}

pub mod progress;
pub mod task;

#[cfg(test)]
mod tests;
