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

use crate::event_loop::flow_declaration::FlowDeclaration;
pub use crate::event_loop::repair_flow::RepairStateMachine;
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
    /// Repair state machine snapshot. Mutable so the
    /// `RepairDispatchStage` (U5) can advance the budget
    /// during `check`.
    pub repair_state: &'a mut RepairStateMachine,
    /// Stage pipeline reference for the emit-gate facade
    /// (U1 / 2026-06-27-002 plan). The facade needs to
    /// call `pipeline.run` from inside the gate so the
    /// caller (`publish_event` / `process_parse_result`)
    /// does not need to thread the pipeline separately.
    pub pipeline: Option<&'a StagePipeline>,
}

impl<'a> StageContext<'a> {
    /// Build a context for tests and early wiring.
    pub fn new(
        current_step: FlowStep,
        loop_id: impl Into<String>,
        expected_version: u64,
        repair_state: &'a mut RepairStateMachine,
    ) -> Self {
        Self {
            current_step,
            loop_id: loop_id.into(),
            expected_version,
            repair_state,
            pipeline: None,
        }
    }

    /// Build a context that carries a pipeline reference
    /// for the emit-gate facade. Used by `EventLoop` at
    /// every gate entry point.
    pub fn with_pipeline(
        current_step: FlowStep,
        loop_id: impl Into<String>,
        expected_version: u64,
        repair_state: &'a mut RepairStateMachine,
        pipeline: &'a StagePipeline,
    ) -> Self {
        Self {
            current_step,
            loop_id: loop_id.into(),
            expected_version,
            repair_state,
            pipeline: Some(pipeline),
        }
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
    pub fn with_default_stages(flow: FlowDeclaration) -> Self {
        Self::new(vec![
            Box::new(crate::event_loop::stages::repair_dispatch_stage::RepairDispatchStage::default()),
            Box::new(crate::event_loop::stages::emit_schema_gate_stage::EmitSchemaGateStage::with_defaults()),
            Box::new(crate::event_loop::stages::flow_step_scope_stage::FlowStepScopeStage::new(flow.clone())),
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
