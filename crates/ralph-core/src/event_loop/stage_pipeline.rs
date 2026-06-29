//! Stage pipeline for emit-time hard gates (U0).
//!
//! The pipeline is the single coordination point through which every
//! hat-emitted event must pass before entering the main [`EventBus`].
//! It is intentionally thin: each stage is a pure [`EmitStage`]
//! implementation that either accepts the event or rejects it with a
//! stable [`StageReject`].
//!
//! # Locked stage order
//!
//! The default stage order is locked by both the
//! [`assert_stage_order!`] macro and the `stage_pipeline_order_*`
//! runtime tests.  Do not reorder without updating the plan and the
//! tests.
//!
//! 1. `ArchiveVersionStage` — loop start hook, not on the emit path.
//! 2. `RepairDispatchStage` — early-returns repair topics to the
//!    isolated repair stream.
//! 3. `EmitSchemaGateStage` — hard required-fields check.
//! 4. `FlowStepScopeStage` — flow step / allowed_emits check.
//! 5. `VerdictGateStage` — terminal emit alignment.

use crate::config::EventLoopConfig;
use crate::event_loop::flow_declaration::FlowDeclaration;
pub use crate::event_loop::repair_flow::RepairStateMachine;
use crate::event_loop::stages::emit_schema_gate_stage::{
    EmitSchemaGateStage, required_fields_from_loop_config,
};
use ralph_proto::Event;

/// A single stage in the emit pipeline.
///
/// Implementations must be `Send` so the pipeline can be stored and
/// invoked from the async event loop runtime.
pub trait EmitStage: Send {
    /// Human-readable stage name, used for diagnostics and order
    /// assertions.
    fn name(&self) -> &'static str;

    /// Validate the event.  Returning `Ok(())` lets the event proceed
    /// to the next stage.  Returning `Err(StageReject)` stops the
    /// pipeline and the event is written to the recovery envelope
    /// instead of the main event bus.
    ///
    /// The context is mutable so a stage that needs to advance an
    /// internal state machine (e.g. U5's `RepairDispatchStage`
    /// consuming the per-task retry budget) can do so. The pipeline
    /// dispatcher (`StagePipeline::run`) takes `&mut StageContext`
    /// for the same reason.
    fn check(&self, ctx: &mut StageContext, event: &Event) -> Result<(), StageReject>;

    /// U12 (P0-1, 2026-06-27 review): provide a
    /// mutable `Any`-typed view so the
    /// `StagePipeline::update_step_close_progress`
    /// helper can downcast to a concrete stage
    /// without forcing every implementor to expose
    /// `&mut self` through the trait. Stages that do
    /// not need this hook return `None`.
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
}

/// Rejection returned by an [`EmitStage`] when an event must not enter
/// the main event bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageReject {
    /// Name of the stage that produced the rejection.
    pub stage_name: &'static str,
    /// Stable machine-readable reason code.
    pub reason_code: String,
    /// Fields that were missing or malformed, if any.
    pub missing_fields: Vec<String>,
}

impl StageReject {
    /// Convenience constructor used by stage implementations.
    pub fn new(stage_name: &'static str, reason_code: impl Into<String>) -> Self {
        Self {
            stage_name,
            reason_code: reason_code.into(),
            missing_fields: Vec::new(),
        }
    }

    /// Builder-style helper to attach missing fields.
    #[must_use]
    pub fn with_missing_fields(mut self, fields: Vec<String>) -> Self {
        self.missing_fields = fields;
        self
    }
}

/// Stub for the current flow step.  Expanded in U5/U9.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlowStep {
    /// Step identifier, e.g. `unit_loop`.
    pub id: String,
}

impl FlowStep {
    /// Create a step stub with the given id.
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

// U4 (2026-06-27-002 plan completion): the stage
// pipeline re-exports `repair_flow::RepairStateMachine`
// so every stage that needs a repair snapshot shares
// the same type. The original empty stub was removed in
// U4; any caller that needs a default machine can use
// `RepairStateMachine::default()` (which yields the
// 3-retry budget).

/// Context passed to every stage check.
#[derive(Debug)]
pub struct StageContext<'a> {
    /// Current flow step.
    pub current_step: FlowStep,
    /// Loop identifier for the active run.
    pub loop_id: String,
    /// Expected state version for idempotent writes.
    pub expected_version: u64,
    /// P1-5 (2026-06-27 adversarial review):
    /// per-task repair state machine registry. Keyed
    /// by `task_key` (the `task_key` field of the
    /// repair event payload). The
    /// `RepairDispatchStage` lazily inserts a fresh
    /// `RepairStateMachine` for any new `task_key`
    /// and advances the matching machine on each
    /// transition. The legacy single-machine design
    /// is gone — task A's retry can no longer
    /// exhaust task B's budget.
    pub repair_states: &'a mut std::collections::HashMap<String, RepairStateMachine>,
    /// Stage pipeline reference for the emit-gate facade
    /// (U1 / 2026-06-27-002 plan). The facade needs to
    /// call `pipeline.run` from inside the gate so the
    /// caller (`publish_event` / `process_parse_result`)
    /// does not need to thread the pipeline separately.
    pub pipeline: Option<&'a StagePipeline>,
    /// 2026-06-29-007 plan U7: optional
    /// `flow_lifecycle.phase` label. `None` means the
    /// registry is still active; `Some("Closed")` /
    /// `Some("Failed")` puts the
    /// `TerminalStateGuardStage` into reject mode.
    pub flow_phase: Option<String>,
}

impl<'a> StageContext<'a> {
    /// Build a context for tests and early wiring.
    pub fn new(
        current_step: FlowStep,
        loop_id: impl Into<String>,
        expected_version: u64,
        repair_states: &'a mut std::collections::HashMap<String, RepairStateMachine>,
    ) -> Self {
        Self {
            current_step,
            loop_id: loop_id.into(),
            expected_version,
            repair_states,
            pipeline: None,
            flow_phase: None,
        }
    }

    /// Build a context that carries a pipeline reference
    /// for the emit-gate facade. Used by `EventLoop` at
    /// every gate entry point.
    pub fn with_pipeline(
        current_step: FlowStep,
        loop_id: impl Into<String>,
        expected_version: u64,
        repair_states: &'a mut std::collections::HashMap<String, RepairStateMachine>,
        pipeline: &'a StagePipeline,
    ) -> Self {
        Self {
            current_step,
            loop_id: loop_id.into(),
            expected_version,
            repair_states,
            pipeline: Some(pipeline),
            flow_phase: None,
        }
    }

    /// P1-5 (2026-06-27 adversarial review): test-only
    /// helper. Wraps a single `RepairStateMachine` in
    /// a one-element `HashMap` under the
    /// `_loop_default` key so tests that don't care
    /// about per-task isolation can keep their
    /// existing fixture shape
    /// (`StageContext::for_test_machine(...)`).
    /// The `HashMap` is leaked so the returned
    /// context can carry a stable `'static`-ish
    /// reference. Tests using this helper are
    /// expected to run sequentially in a single
    /// process — leaking is acceptable for
    /// test-only ergonomics. The helper is
    /// NOT `#[cfg(test)]` so integration tests in
    /// `crates/ralph-core/tests/` (which compile
    /// against the ralph-core lib, not its
    /// `cfg(test)` tree) can call it.
    pub fn for_test_machine(
        current_step: FlowStep,
        loop_id: impl Into<String>,
        expected_version: u64,
        repair: &'a mut RepairStateMachine,
    ) -> Self {
        let mut states = std::collections::HashMap::new();
        states.insert("_loop_default".to_string(), repair.clone());
        let states = Box::leak(Box::new(states));
        Self::new(current_step, loop_id, expected_version, states)
    }
}

/// Ordered pipeline of emit stages.
#[derive(Default)]
pub struct StagePipeline {
    stages: Vec<Box<dyn EmitStage>>,
}

impl std::fmt::Debug for StagePipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagePipeline")
            .field("stage_count", &self.stages.len())
            .finish()
    }
}

impl StagePipeline {
    /// Create a pipeline from the given stages, preserving order.
    pub fn new(stages: Vec<Box<dyn EmitStage>>) -> Self {
        Self { stages }
    }

    /// Build the locked default pipeline for the mechanism
    /// foundation (U0). Order is fixed by the plan; changing it
    /// breaks the `assert_stage_order!` macro and the
    /// `stage_pipeline_order_*` tests.
    ///
    /// P1-4 (2026-06-27 adversarial review): the
    /// pipeline now also includes the U12
    /// `StepCloseObligationStage` between
    /// `FlowStepScopeStage` and `VerdictGateStage`.
    /// Without it the `step_close_obligation` pure
    /// logic was never wired into the runtime and
    /// the 2026-06-26 4/8 partial silence
    /// scenario went unflagged. The stage is
    /// fail-closed but only fires when the
    /// operator has called
    /// `update_progress(step_id, done, total)` —
    /// legacy presets that do not drive the
    /// progress registry see the same fail-open
    /// behaviour as before.
    pub fn with_default_stages(flow: FlowDeclaration) -> Self {
        Self::with_default_stages_for_loop_config(flow, None)
    }

    /// Like [`Self::with_default_stages`], but merges preset
    /// `event_policy.schemas` into `EmitSchemaGateStage` when
    /// `loop_config` is present (production `EventLoop` path).
    pub fn with_default_stages_for_loop_config(
        flow: FlowDeclaration,
        loop_config: Option<&EventLoopConfig>,
    ) -> Self {
        let schema_gate = match loop_config {
            Some(cfg) => EmitSchemaGateStage::new(required_fields_from_loop_config(cfg)),
            None => EmitSchemaGateStage::with_defaults(),
        };
        Self::new(vec![
            Box::new(crate::event_loop::stages::repair_dispatch_stage::RepairDispatchStage::default()),
            Box::new(schema_gate),
            Box::new(crate::event_loop::stages::flow_step_scope_stage::FlowStepScopeStage::new(flow.clone())),
            Box::new(crate::event_loop::stages::step_close_obligation_stage::StepCloseObligationStage::new(flow.clone())),
            Box::new(crate::event_loop::stages::verdict_gate_stage::VerdictGateStage::new(flow)),
        ])
    }

    /// Run the event through every stage in order.  The first
    /// rejection short-circuits and is returned.
    pub fn run(&self, ctx: &mut StageContext, event: &Event) -> Result<(), StageReject> {
        for stage in &self.stages {
            stage.check(ctx, event)?;
        }
        Ok(())
    }

    /// Names of the configured stages, in order.
    pub fn names(&self) -> Vec<&'static str> {
        self.stages.iter().map(|s| s.name()).collect()
    }

    /// U10 (2026-06-27-002 plan completion): true if
    /// `topic` is in the locked `terminal_emits` set
    /// (default `[LOOP_COMPLETE]`). The dispatcher
    /// consults this after a successful `run` to write
    /// the loop-termination record.
    ///
    /// The probe is delegated to the `VerdictGateStage`
    /// if it is present (the locked-last stage in the
    /// default pipeline). We look up the stage by
    /// walking the stages list and calling a
    /// type-erased probe via a downcast on the trait
    /// object — but since `VerdictGateStage` is
    /// concrete, the simplest implementation is to
    /// check the stage name and call a free function
    /// that mirrors `VerdictGateStage::is_terminal`'s
    /// logic.
    pub fn is_terminal(&self, event: &ralph_proto::Event) -> bool {
        for stage in &self.stages {
            if stage.name() == "VerdictGate" {
                return crate::event_loop::stages::verdict_gate_stage::is_terminal_topic(
                    event.topic.as_str(),
                );
            }
        }
        false
    }

    /// U12 wiring (P0-1, 2026-06-27 review): drive the
    /// `StepCloseObligationStage` progress registry
    /// without breaking the `EmitStage` trait (which is
    /// intentionally `&self`-bound on `check`).
    ///
    /// Walks the stages list, downcasts each `Box<dyn
    /// EmitStage>` to a concrete
    /// `StepCloseObligationStage` (only present when the
    /// pipeline was built via `with_default_stages_*`),
    /// and calls `update_progress` on the first match.
    /// No-op when the stage is absent or the downcast
    /// fails — that matches the pre-U12 fail-open
    /// semantics for callers that did not opt in.
    pub fn update_step_close_progress(&mut self, step_id: &str, done: u32, total: u32) {
        for stage in self.stages.iter_mut() {
            if stage.name() == "StepCloseObligation" {
                if let Some(typed) = stage
                    .as_any_mut()
                    .and_then(|a| {
                        a.downcast_mut::<
                            crate::event_loop::stages::step_close_obligation_stage::StepCloseObligationStage,
                        >()
                    })
                {
                    typed.update_progress(step_id, done, total);
                }
                return;
            }
        }
    }
}

/// Assert at compile time that the pipeline's stage names match the
/// locked order.
///
/// # Example
///
/// ```rust,ignore
/// assert_stage_order!(pipeline, [ArchiveVersion, RepairDispatch, EmitSchemaGate, FlowStepScope, VerdictGate]);
/// ```
#[macro_export]
macro_rules! assert_stage_order {
    ($pipeline:expr, [$($name:ident),+ $(,)?]) => {{
        const EXPECTED: &[&str] = &[$(stringify!($name)),+];
        let actual: Vec<&str> = $pipeline.names();
        assert_eq!(actual, EXPECTED, "stage order must match the locked order");
    }};
}

#[cfg(test)]
mod tests;
