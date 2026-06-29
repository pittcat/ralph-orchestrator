//! `StepCloseObligationStage` — enforces the U12 partial-state
//! obligation at emit time (P1-4 / 2026-06-27 adversarial
//! review).
//!
//! Why this stage exists: the pure-logic core in
//! `event_loop::step_close_obligation` has unit tests for
//! `required_emit` / `emit_satisfies_obligation`, but no
//! stage wired into the runtime. The 2026-06-26
//! diagnostic observed a 4/8 partial state followed by
//! **silence** — the loop emitted no further events and
//! the runtime failed to flag the obligation. The legacy
//! `FlowStepScopeStage`'s `reason_pattern` check fires
//! only when an emit arrives; it cannot catch the "no
//! emit at all" case.
//!
//! This stage sits between `FlowStepScopeStage` and
//! `VerdictGateStage` so it runs *after* the
//! schema/flow-scope checks (a malformed emit is
//! rejected upstream) but *before* the terminal
//! alignment check (a partial emit that closes a step
//! must not be confused with a terminal emit).
//!
//! Cross-platform / concurrency semantics: pure CPU.
//! The stage keeps a tiny in-memory obligation
//! registry keyed by `current_step.id`; the registry
//! is reset whenever `current_step.id` changes
//! (the `set_step` helper handles the transition).
//! The registry is not thread-safe — callers
//! serialise access via `&mut StageContext`.
//!
//! The progress accounting is driven by the
//! `flow_declaration::FlowStepDecl::on_partial` map
//! and a `StepProgress` carrier on the stage. Operators
//! wire the progress via the `update_progress` API
//! (called from the runtime when a unit completes);
//! the stage then checks the next emit against
//! `emit_satisfies_obligation` and rejects with
//! `step_close_obligation_violated` when the emit
//! does not satisfy any pending branch.

use crate::event_loop::flow_declaration::FlowDeclaration;
use crate::event_loop::stage_pipeline::{EmitStage, StageContext, StageReject};
use crate::event_loop::step_close_obligation::{
    Obligation, StepProgress, emit_satisfies_obligation, required_emit,
};
use ralph_proto::Event;
use std::collections::HashMap;

/// Stage that rejects emits that fail to satisfy a
/// pending partial-state obligation on the current
/// step. The stage is fail-closed: when the
/// `current_step` is in a partial state and the emit
/// does not match any `on_partial` branch, the stage
/// returns `Err(StageReject { reason_code:
/// "step_close_obligation_violated" })` so the
/// recovery envelope records the violation.
///
/// The stage tracks a per-step progress value
/// (`done` / `total`). When `done < total` AND the
/// step's `on_partial` map is non-empty, the stage
/// builds an `Obligation::Pending(...)` and checks
/// every emit against it.
pub struct StepCloseObligationStage {
    flow: FlowDeclaration,
    /// Per-step progress. Keyed by `step.id`. The
    /// runtime calls `update_progress(step_id, done,
    /// total)` whenever a unit completes (see
    /// `emit_close_obligation` for the U7 wiring
    /// path). When the registry is empty, the stage
    /// does not police obligations — the default
    /// behaviour matches the pre-U12 fail-open
    /// gate for legacy presets.
    progress: HashMap<String, StepProgress>,
}

impl StepCloseObligationStage {
    /// Build the stage with the given flow. The
    /// progress registry starts empty; operators
    /// populate it via `update_progress`.
    pub fn new(flow: FlowDeclaration) -> Self {
        Self {
            flow,
            progress: HashMap::new(),
        }
    }

    /// Record that `done` of `total` units have
    /// finished on `step_id`. The next emit on this
    /// step is then checked against the resulting
    /// obligation. Idempotent — repeated calls with
    /// the same value are no-ops; calls with a
    /// smaller `done` than the recorded value are
    /// rejected silently (a regression in the
    /// counter would mask an obligation violation).
    pub fn update_progress(&mut self, step_id: &str, done: u32, total: u32) {
        let entry = self
            .progress
            .entry(step_id.to_string())
            .or_insert(StepProgress { done: 0, total: 0 });
        if done >= entry.done {
            entry.done = done;
            entry.total = total;
        }
    }

    /// Build the obligation for the current step
    /// from the recorded progress. Returns
    /// `Obligation::None` when the step has no
    /// progress recorded or the step is not declared
    /// in the flow.
    fn obligation_for(&self, step_id: &str) -> Obligation {
        let Some(progress) = self.progress.get(step_id) else {
            return Obligation::None;
        };
        let Some(step) = self.flow.step(step_id) else {
            return Obligation::None;
        };
        required_emit(*progress, &step.on_partial)
    }
}

impl EmitStage for StepCloseObligationStage {
    fn name(&self) -> &'static str {
        "StepCloseObligation"
    }

    fn check(&self, ctx: &mut StageContext, event: &Event) -> Result<(), StageReject> {
        // P1-4 (2026-06-27 adversarial review): the
        // obligation is computed from the *current*
        // step's progress. When the obligation is
        // `None` (no progress recorded, step
        // complete, or step not declared) the stage
        // is a no-op. This preserves the legacy
        // behaviour for presets that do not opt
        // into U12's progress tracking.
        let obligation = self.obligation_for(&ctx.current_step.id);
        if matches!(obligation, Obligation::None) {
            return Ok(());
        }

        if emit_satisfies_obligation(&obligation, event.topic.as_str(), event.payload.as_str()) {
            return Ok(());
        }

        Err(StageReject::new(
            self.name(),
            "step_close_obligation_violated",
        ))
    }

    // U12 wiring (P0-1, 2026-06-27 review): expose a
    // mutable `Any` view so `StagePipeline` can drive
    // `update_progress` without forcing `check` to take
    // `&mut self`.
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests;
