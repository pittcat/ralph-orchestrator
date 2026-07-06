//! State projection — single-writer for `.ralph/agent/tasks.jsonl`
//! and `.ralph/agent/progress.md`.
//!
//! Plan ref: `docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md`.
//!
//! The module has four public entry points:
//! - [`StateProjector::apply`] — main hook. Called from
//!   `process_parse_result` **after** the state machine validates
//!   the batch and **before** the unified `StepHandoffRule` runs
//!   (SP-R8).
//!   Returns an [`ApplyReport`] so the caller can fail-closed on
//!   individual events.
//! - [`StateProjector::bootstrap_from_disk`] — Unit 6 entry point
//!   used on loop resume. Loads the canonical state into the
//!   in-memory cache so the first emit only applies *deltas*.
//! - [`StateProjector::apply_from_ledger`] — U2 (plan
//!   2026-06-21-002) entry point. Drives the projector from a
//!   [`crate::state::StateLedger`] commit log: the projector
//!   writes the same canonical ledgers but reads the
//!   authoritative state from [`crate::state::LedgerSnapshot`]
//!   rather than its own `tasks_cache` / `progress_cache`. The
//!   legacy caches become write-through mirrors that the
//!   projector refreshes on every successful commit.
//! - [`StateProjector::project_ledger_snapshot`] — U2 helper.
//!   Equivalent to `bootstrap_from_disk` but reads from a
//!   [`crate::state::LedgerSnapshot`], aligning the projector
//!   caches with the ledger's view of the world before any
//!   ledger-derived commits are applied.
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
//! ## U2 (plan 2026-06-21-002): ledger-driven reads
//!
//! `tasks_cache` / `progress_cache` remain on the
//! [`ProjectionContext`] for backward compatibility with the
//! legacy `StateProjector::apply` path and its ~150 tests, but
//! they are now marked `#[deprecated]`. New read APIs
//! ([`ProjectionContext::task_snapshot`] /
//! [`ProjectionContext::progress_snapshot`]) return references
//! to the underlying [`crate::state::LedgerSnapshot`], which is
//! the unified source of truth. Callers wiring the U2 path
//! populate the read-only view via
//! [`ProjectionContext::set_ledger_snapshot`] before invoking
//! [`StateProjector::apply_from_ledger`] /
//! [`StateProjector::project_ledger_snapshot`].
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
//! partially mitigates the risk for the most common path. The U2
//! ledger path closes this gap entirely: the snapshot is the
//! authoritative state, the caches are write-through mirrors.

// This module **owns** the deprecated `tasks_cache` /
// `progress_cache` mirrors and is responsible for keeping them
// in sync with the canonical `LedgerSnapshot`. Touching the
// deprecated fields is therefore intentional and unavoidable
// here; the suppression is module-local so external callers
// (production code in `event_loop`, `runtime_state`, and tests)
// are not silenced.
#![allow(deprecated)]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::config::StateProjectionConfig;
use crate::event_reader::Event;
use crate::state::LedgerSnapshot;
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
    // U3 of plan 2026-07-05-005: `review.dimensions.complete` is
    // routed by the event_policy dedup chain but the projector
    // was previously INVISIBLE to it (the event was rejected
    // upstream as a duplicate without the projector ever
    // seeing it). Adding it here lets the projector update its
    // "last dimensions-complete" view, which feeds the
    // `## ORCHESTRATOR CONTEXT` block.
    //
    // R10 (backward compatibility): presets that do not declare
    // `review.dimensions.complete` in their `state_projection`
    // chain continue to work — this entry is added to the
    // projector whitelist, not the chain. A preset without the
    // matching `StateProjectionAction::ReviewDimensionsComplete`
    // variant in its `actions_chain` simply has the new topic
    // pass through without projection (same as the legacy
    // behaviour for any non-chain topic).
    "review.dimensions.complete",
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
///
/// ## U2 (plan 2026-06-21-002): read-side split
///
/// `tasks_cache` / `progress_cache` are now `#[deprecated]` —
/// they survive only as write-through mirrors of the canonical
/// ledgers, kept in sync by [`StateProjector::apply`]. New read
/// code goes through [`Self::task_snapshot`] / [`Self::progress_snapshot`],
/// which borrow from the underlying [`LedgerSnapshot`] (set via
/// [`Self::set_ledger_snapshot`]).
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
    /// P0-2 (plan 2026-06-29-006): current loop id marker used as
    /// a fallback for the task `loop_id` field when the event
    /// payload does not carry one. The runtime reads the canonical
    /// `current_loop_id` from `EventLoop::current_loop_id_for_contract`
    /// and threads it through every projector call so tasks
    /// projected from a loop-scoped event always get a `loop_id`
    /// — even when the agent's payload is `from_key:...` legacy
    /// or `""` (which otherwise would trigger `TaskWrongLoop`
    /// in `validate_task`).
    pub current_loop_id: Option<String>,
    /// In-memory cache of the tasks ledger. Populated by
    /// [`StateProjector::bootstrap_from_disk`] on loop resume; kept
    /// in sync by [`task::project`] on every apply.
    ///
    /// U2 (plan 2026-06-21-002): deprecated as the read-side
    /// source of truth. The projector still refreshes this field
    /// on every write so legacy callers and the ~150 pre-U2 tests
    /// continue to observe the same in-memory view. New reads
    /// must go through [`Self::task_snapshot`].
    ///
    /// P1-3 (plan 2026-06-23-003): visibility demoted from `pub`
    /// to `pub(crate)` so external crates (event_loop, runtime_state,
    /// etc.) cannot accidentally read the deprecated mirror. The
    /// legacy mirror contract is still tested by
    /// `state_projector/tests.rs` and `state_projector/u2_tests.rs`,
    /// both of which live in this crate and have module-level
    /// `#[allow(deprecated)]`. New code must use
    /// [`Self::task_snapshot`] / [`Self::progress_snapshot`]
    /// (or [`crate::state::LedgerSnapshot::tasks`] /
    /// [`crate::state::LedgerSnapshot::progress`]).
    #[deprecated(
        since = "0.2.0",
        note = "U2: read from ProjectionContext::task_snapshot (LedgerSnapshot) instead"
    )]
    pub(crate) tasks_cache: Vec<crate::task::Task>,
    /// In-memory cache of the progress ledger. Same lifecycle as
    /// `tasks_cache`.
    ///
    /// U2 (plan 2026-06-21-002): deprecated as the read-side
    /// source of truth. See [`Self::tasks_cache`] for the
    /// rationale.
    ///
    /// P1-3 (plan 2026-06-23-003): visibility demoted from `pub`
    /// to `pub(crate)` — see [`Self::tasks_cache`].
    #[deprecated(
        since = "0.2.0",
        note = "U2: read from ProjectionContext::progress_snapshot (LedgerSnapshot) instead"
    )]
    pub(crate) progress_cache: ProgressSnapshot,
    /// Optional reference to the unified [`LedgerSnapshot`] that
    /// the U2 path reads from. `None` keeps the legacy path fully
    /// working. The projector never mutates this field — the
    /// caller (`EventLoop` or a test) is responsible for seeding
    /// it via [`Self::set_ledger_snapshot`].
    ledger_snapshot: Option<Box<LedgerSnapshot>>,
    /// U3 of plan 2026-07-05-005: in-memory view of the latest
    /// `review.dimensions.complete` event. A direct
    /// `Mutex<Option<...>>` keeps the read path simple and stays
    /// consistent with the legacy `tasks_cache` / `progress_cache`
    /// mirrors.
    pub(crate) review_dimensions_view: std::sync::Mutex<Option<review::ReviewDimensionsView>>,
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
            // P0-2 (plan 2026-06-29-006): default `None` preserves
            // pre-fix behaviour (no fallback injection). Loop
            // callers should use `with_current_loop_id` to wire
            // the marker.
            current_loop_id: None,
            tasks_cache: Vec::new(),
            progress_cache: ProgressSnapshot::default(),
            ledger_snapshot: None,
            // U3 of plan 2026-07-05-005: the review summary view
            // slot starts empty; the projector fills it on the
            // first `review.dimensions.complete` event.
            review_dimensions_view: std::sync::Mutex::new(None),
        }
    }

    /// P0-2 (plan 2026-06-29-006): thread the loop's canonical
    /// `current_loop_id` into the context so the projector can
    /// fall back to it when an event payload omits `loop_id`.
    /// Mirrors `EventLoop::current_loop_id_for_contract`.
    pub fn with_current_loop_id(mut self, loop_id: impl Into<String>) -> Self {
        self.current_loop_id = Some(loop_id.into());
        self
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

    /// U2 (plan 2026-06-21-002): wire the projector to read from
    /// a [`LedgerSnapshot`]. The projector never mutates the
    /// snapshot; subsequent writes via [`StateProjector::apply`]
    /// only update the legacy `tasks_cache` / `progress_cache`
    /// mirrors. Callers that want a fully ledger-driven view
    /// must use [`StateProjector::apply_from_ledger`].
    ///
    /// The helper is idempotent; calling it twice replaces the
    /// previous snapshot reference.
    pub fn set_ledger_snapshot(&mut self, snapshot: LedgerSnapshot) {
        self.ledger_snapshot = Some(Box::new(snapshot));
    }

    /// Borrow the wired [`LedgerSnapshot`]. Returns `None` when
    /// the U2 path has not been enabled (legacy mode) or when
    /// [`Self::set_ledger_snapshot`] has not been called.
    pub fn ledger_snapshot(&self) -> Option<&LedgerSnapshot> {
        self.ledger_snapshot.as_deref()
    }

    /// Read the projector's view of the task ledger. When the
    /// U2 path is wired, returns the ledger snapshot's tasks;
    /// otherwise falls back to the legacy `tasks_cache` mirror.
    ///
    /// Returns `(tasks, from_ledger)`: `from_ledger=true` means
    /// the data is the unified authoritative state; `false`
    /// means it is the legacy mirror. U2 callers should treat
    /// `false` as "stale relative to the ledger" and prefer
    /// `ledger_snapshot().tasks()` when available.
    pub fn task_snapshot(&self) -> (&[crate::task::Task], bool) {
        if let Some(snap) = self.ledger_snapshot.as_deref() {
            (snap.tasks(), true)
        } else {
            #[allow(deprecated)]
            let cache = &self.tasks_cache;
            (cache, false)
        }
    }

    /// Read the projector's view of the progress ledger. Same
    /// dual-source pattern as [`Self::task_snapshot`].
    pub fn progress_snapshot(&self) -> (&ProgressSnapshot, bool) {
        if let Some(snap) = self.ledger_snapshot.as_deref() {
            (snap.progress(), true)
        } else {
            #[allow(deprecated)]
            let cache = &self.progress_cache;
            (cache, false)
        }
    }

    /// U3 of plan 2026-07-05-005: latest `review.dimensions.complete`
    /// view for `## REVIEW SUMMARY` prompt injection.
    pub fn review_dimensions_snapshot(&self) -> Option<review::ReviewDimensionsView> {
        self.review_dimensions_view
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
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

    /// U2 (plan 2026-06-21-002): project a ledger snapshot onto
    /// the canonical ledgers. Equivalent to
    /// [`Self::bootstrap_from_disk`] but reads the authoritative
    /// state from a [`LedgerSnapshot`] rather than re-parsing the
    /// on-disk `tasks.jsonl` / `progress.md`. The legacy
    /// `tasks_cache` / `progress_cache` mirrors are refreshed so
    /// pre-U2 callers continue to observe the same view.
    ///
    /// Returns an [`ApplyReport`] describing how many rows were
    /// written (or, in the cold-start case, zero — the ledgers
    /// are already in sync with the snapshot).
    ///
    /// Callers that want the projector to keep reading from the
    /// snapshot on every apply must wire it via
    /// [`ProjectionContext::set_ledger_snapshot`] before invoking
    /// this method; otherwise the projector falls back to the
    /// legacy cache path.
    pub fn project_ledger_snapshot(
        &mut self,
        snapshot: &LedgerSnapshot,
    ) -> Result<ApplyReport, String> {
        // Refresh the legacy mirrors so callers that still read
        // `tasks_cache` / `progress_cache` see the same view.
        #[allow(deprecated)]
        {
            self.ctx.tasks_cache = snapshot.tasks().to_vec();
            self.ctx.progress_cache = snapshot.progress().clone();
        }

        // Replay-write the canonical ledgers from the snapshot.
        // The atomic temp-file + rename pattern keeps partial
        // writes from corrupting either file.
        let mut report = ApplyReport::default();
        if !snapshot.tasks().is_empty() {
            let mut store = crate::task_store::TaskStore::load(&self.ctx.tasks_path)
                .map_err(|e| format!("tasks_load: {e}"))?;
            let current_ids: std::collections::HashSet<String> =
                store.all().iter().map(|t| t.id.clone()).collect();
            for task in snapshot.tasks() {
                if !current_ids.contains(&task.id) {
                    store.ensure(task.clone());
                }
            }
            store.save().map_err(|e| format!("tasks_save: {e}"))?;
            debug!(
                tasks = snapshot.tasks().len(),
                "project_ledger_snapshot wrote tasks.jsonl"
            );
        }

        // The progress ledger is the only "single file" view, so
        // we re-emit it verbatim. The `write_progress` helper
        // already round-trips the existing dialect.
        self::progress::write_progress_external(&self.ctx.progress_path, snapshot.progress())?;
        report.applied = 1;
        Ok(report)
    }

    /// U2 (plan 2026-06-21-002): apply a single ledger commit to
    /// the canonical ledgers.
    ///
    /// The projector reads the authoritative state from the
    /// [`LedgerSnapshot`] passed alongside the commit (the
    /// snapshot is whatever `StateLedger::snapshot()` returns at
    /// the call site — typically the *post-commit* snapshot so
    /// the projector writes the up-to-date view).
    ///
    /// Returns an [`ApplyReport`] describing how many rows were
    /// written (zero when the commit delta has no on-disk
    /// effect, e.g. `RejectionRecorded`).
    ///
    /// Write failures surface as `Err(String)` so the caller can
    /// publish a `state_projection.rejected` diagnostic and the
    /// ledger can decide whether to roll back the commit.
    pub fn apply_from_ledger(
        &mut self,
        commit: &crate::state::Commit,
        snapshot: &LedgerSnapshot,
    ) -> Result<ApplyReport, String> {
        use crate::state::CommitDelta;

        let mut report = ApplyReport::default();

        // Refresh the legacy mirrors so pre-U2 callers see the
        // same view the U2 path is writing.
        #[allow(deprecated)]
        {
            self.ctx.tasks_cache = snapshot.tasks().to_vec();
            self.ctx.progress_cache = snapshot.progress().clone();
        }

        match &commit.delta {
            CommitDelta::NoOp => {
                // No-op commits never reach this site in
                // production; defensive no-op.
            }
            CommitDelta::TaskInserted { task } => {
                let mut store = crate::task_store::TaskStore::load(&self.ctx.tasks_path)
                    .map_err(|e| format!("tasks_load: {e}"))?;
                store.ensure(task.clone());
                store.save().map_err(|e| format!("tasks_save: {e}"))?;
                debug!(
                    task_id = %task.id,
                    "apply_from_ledger inserted task"
                );
                report.applied = 1;
            }
            CommitDelta::TaskLifecycle {
                task_id,
                transition,
            } => {
                use crate::state::TaskTransition;
                let mut store = crate::task_store::TaskStore::load(&self.ctx.tasks_path)
                    .map_err(|e| format!("tasks_load: {e}"))?;
                // Materialize the snapshot's tasks into the
                // disk ledger before applying the delta. This
                // closes the gap between the snapshot
                // (authoritative) and the disk ledger
                // (derived): when the loop resumes from a
                // commit log, the disk may not yet know
                // about the task that the snapshot says
                // exists.
                let pre_count = store.all().len();
                materialize_snapshot_tasks(&mut store, snapshot.tasks());
                let inserted = store.all().len() - pre_count;
                let changed = match transition {
                    TaskTransition::Closed => {
                        // 2026-06-30-001 P0-4: the new
                        // `TaskStore::close` / `close_by_key`
                        // guard refuses to close never-started
                        // rows. The projector was the only
                        // legitimate path that closes a task
                        // without an explicit `start` call
                        // (executor picks the task up between
                        // `work.ready` and `work.done`), so we
                        // mark the row started here, mirroring
                        // the `project_close_task` event
                        // path. Fix-unit rows are exempt by
                        // design (see `is_fix_unit_id` /
                        // `is_fix_unit_key`).
                        if let Some(row) = store.get_mut(task_id)
                            && row.started.is_none()
                            && !crate::task_store::is_fix_unit_id(task_id)
                        {
                            row.start();
                        }
                        store.close(task_id).is_some()
                    }
                    TaskTransition::Failed => {
                        // Same as `Closed`: the new guard
                        // requires `started.is_some()` for
                        // non-fix-unit rows. We mark started
                        // here so a task that never had an
                        // explicit start but hit `work.failed`
                        // can still be marked Failed.
                        if let Some(row) = store.get_mut(task_id)
                            && row.started.is_none()
                            && !crate::task_store::is_fix_unit_id(task_id)
                        {
                            row.start();
                        }
                        store.fail(task_id).is_some()
                    }
                    TaskTransition::Started | TaskTransition::Reopened | TaskTransition::Opened => {
                        // Opened/Started/Reopened on an existing
                        // task are pass-throughs; the projector
                        // refreshes the row but does not change
                        // the cache delta. `TaskInserted` is the
                        // path for new tasks.
                        store.all().iter().any(|t| t.id == *task_id)
                    }
                };
                if changed || inserted > 0 {
                    store.save().map_err(|e| format!("tasks_save: {e}"))?;
                    debug!(
                        task_id = %task_id,
                        "apply_from_ledger updated task lifecycle"
                    );
                    report.applied = 1;
                }
            }
            CommitDelta::ProgressUpdate {
                completed_step,
                current_step,
            } => {
                let mut snap = snapshot.progress().clone();
                if let Some(done) = completed_step {
                    let trimmed = done.trim();
                    if !trimmed.is_empty() && !snap.completed_steps.iter().any(|s| s == trimmed) {
                        snap.completed_steps.push(trimmed.to_string());
                    }
                }
                if let Some(step) = current_step {
                    snap.current_step = Some(step.clone());
                }
                self::progress::write_progress_external(&self.ctx.progress_path, &snap)?;
                report.applied = 1;
            }
            CommitDelta::PlanComplete {
                final_step,
                closed_count: _,
            } => {
                let mut store = crate::task_store::TaskStore::load(&self.ctx.tasks_path)
                    .map_err(|e| format!("tasks_load: {e}"))?;
                materialize_snapshot_tasks(&mut store, snapshot.tasks());
                let mut closed = 0usize;
                for task in store.all().to_vec() {
                    if !task.status.is_terminal() {
                        store.close(&task.id);
                        closed += 1;
                    }
                }
                // Persist the materialized + closed tasks
                // whenever the snapshot carries any tasks
                // (materialized_snapshot_tasks may have
                // inserted rows that need to reach disk, even
                // when no further close happens).
                if closed > 0 || !snapshot.tasks().is_empty() {
                    store.save().map_err(|e| format!("tasks_save: {e}"))?;
                }
                let mut snap = snapshot.progress().clone();
                if let Some(step) = final_step {
                    let trimmed = step.trim();
                    if !trimmed.is_empty() && !snap.completed_steps.iter().any(|s| s == trimmed) {
                        snap.completed_steps.push(trimmed.to_string());
                    }
                    snap.current_step = Some(step.clone());
                }
                self::progress::write_progress_external(&self.ctx.progress_path, &snap)?;
                report.applied = 1;
            }
            // The remaining deltas have no on-disk effect on the
            // canonical `tasks.jsonl` / `progress.md` ledgers; the
            // ledger is already the source of truth for them.
            CommitDelta::RejectionRecorded { .. }
            | CommitDelta::RejectionBudgetTripped { .. }
            | CommitDelta::WorkflowPhaseAdvanced { .. }
            | CommitDelta::CounterChanged { .. }
            | CommitDelta::SeenTopic { .. }
            | CommitDelta::CompletionRequested
            | CommitDelta::CompletionHonored
            | CommitDelta::CancellationRequested
            | CommitDelta::StewardWoken
            | CommitDelta::HatActivationCounted { .. }
            | CommitDelta::HatExhausted { .. }
            | CommitDelta::RejectionLastIteration { .. }
            | CommitDelta::StallRecoveryCounted { .. }
            | CommitDelta::NoProgressTurnObserved { .. }
            | CommitDelta::TaskBlockCounted { .. }
            | CommitDelta::TaskAbandoned { .. }
            | CommitDelta::ReviewStepUpdated { .. }
            | CommitDelta::FlowLifecycleUpdated { .. }
            | CommitDelta::RejectionDigestUpdated { .. } => {
                debug!(
                    delta_kind = ?std::mem::discriminant(&commit.delta),
                    "apply_from_ledger no-op for non-ledger delta"
                );
            }
        }

        // Refresh the legacy mirrors from the post-write state
        // so the next apply() (in either path) sees the latest
        // canonical view.
        #[allow(deprecated)]
        {
            self.ctx.tasks_cache = snapshot.tasks().to_vec();
            self.ctx.progress_cache = snapshot.progress().clone();
        }
        Ok(report)
    }

    /// U2 (plan 2026-06-21-002): build the `## ORCHESTRATOR
    /// CONTEXT` block from a [`LedgerSnapshot`] rather than from
    /// the projector's in-memory cache. The rendering shape
    /// matches [`RuntimeStateSnapshot::to_prompt_block`] (defined
    /// in `runtime_state.rs`) so the prompt is byte-identical
    /// between the legacy and U2 paths.
    pub fn build_orchestrator_context_from_ledger(
        &self,
        snapshot: &LedgerSnapshot,
        loop_start_sha: Option<&str>,
        plan_baseline_sha: Option<&str>,
    ) -> String {
        let review = self.ctx.review_dimensions_snapshot();
        self::orchestrator_context::build_block(
            snapshot,
            &self.ctx.config,
            loop_start_sha,
            plan_baseline_sha,
            review.as_ref(),
        )
    }

    /// U11-T9 (P0-3 follow-up): push the projector's in-memory
    /// `tasks_cache` and `progress_cache` into a [`LedgerSnapshot`].
    ///
    /// The unified pre-commit `StepHandoffRule` reads
    /// `ledger_snapshot.tasks` / `ledger_snapshot.progress` (rather
    /// than the projector's private caches), so the cache view
    /// must be mirrored into the snapshot **after every batch's
    /// `projector.apply`** and **before** the unified pre-commit
    /// filter runs in [`EventLoop::process_parse_result`].
    ///
    /// This is the inverse of [`Self::project_ledger_snapshot`]:
    ///   * `project_ledger_snapshot`: ledger → disk (refresh
    ///     caches from a `LedgerSnapshot`, then replay-write disk)
    ///   * `sync_to_ledger_snapshot`: projector cache → ledger
    ///     (refresh the snapshot from the just-written cache)
    ///
    /// The helper is pure: it does not touch disk, does not call
    /// `commit()`, and does not set `bypass_active`. Callers that
    /// need a persistent record must commit a `CommitDelta`
    /// separately.
    pub fn sync_to_ledger_snapshot(&self, snapshot: &mut LedgerSnapshot) {
        #[allow(deprecated)]
        {
            snapshot.tasks = self.ctx.tasks_cache.clone();
            snapshot.progress = self.ctx.progress_cache.clone();
        }
        // Mirror `ProgressSnapshot::parse`'s `empty_headings`
        // recomputation so downstream rules that read
        // `snapshot.progress` see the same view as the disk-side
        // `ProgressSnapshot::parse(&content)` call would produce.
        // Without this, a `push_completed` that adds a step without
        // rewriting `empty_headings` would carry `true` from the
        // cold-start default and `StepHandoffRule` would reject
        // with `progress_missing_headings` even though the
        // projected `progress.md` is well-formed on disk.
        snapshot.progress.empty_headings = snapshot.progress.current_step.is_none()
            && snapshot.progress.completed_steps.is_empty();
    }

    /// Apply a batch of events to the ledgers. Events whose topic
    /// is not in [`PROJECTED_TOPICS`] (or for which the config has
    /// no matching action) are passed through without touching
    /// disk. Events that fail to project are recorded as
    /// `rejected` and the function returns an [`ApplyReport`]; the
    /// caller decides whether to drop those events from the bus
    /// (Phase 1: drop + emit `event.state_projection.rejected`).
    pub fn apply(&mut self, events: &[Event]) -> ApplyReport {
        if !self.ctx.config.enabled
            || (self.ctx.config.actions.is_empty() && self.ctx.config.actions_chain.is_empty())
        {
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
                    crate::config::StateProjectionAction::ReviewDimensionsComplete {
                        task_key,
                        fix_round,
                        dimensions,
                        summary,
                    } => crate::state_projector::review::project_review_dimensions_complete(
                        &mut self.ctx,
                        &parsed,
                        task_key.as_deref(),
                        fix_round.as_deref(),
                        dimensions.as_deref(),
                        summary.as_deref(),
                    ),
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

/// U2 (plan 2026-06-21-002): bring the disk task ledger into
/// sync with the snapshot's authoritative task list. Tasks
/// whose `id` is already in the store are left untouched
/// (their status transitions are governed by
/// [`StateProjector::apply_from_ledger`] matching deltas).
/// Tasks in the snapshot but missing on disk are inserted via
/// `TaskStore::ensure`, which is idempotent.
fn materialize_snapshot_tasks(
    store: &mut crate::task_store::TaskStore,
    snapshot_tasks: &[crate::task::Task],
) {
    let known: std::collections::HashSet<String> =
        store.all().iter().map(|t| t.id.clone()).collect();
    for task in snapshot_tasks {
        if !known.contains(&task.id) {
            store.ensure(task.clone());
        }
    }
}

pub mod orchestrator_context;
pub mod progress;
pub mod review;
pub mod task;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod u2_tests;
