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
    fn check(&self, ctx: &StageContext, event: &Event) -> Result<(), StageReject>;
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

/// Stub for the repair state machine.  Expanded in U2/U7.
#[derive(Debug, Clone, Default)]
pub struct RepairStateMachine;

/// Context passed to every stage check.
#[derive(Debug)]
pub struct StageContext<'a> {
    /// Current flow step.
    pub current_step: FlowStep,
    /// Loop identifier for the active run.
    pub loop_id: String,
    /// Expected state version for idempotent writes.
    pub expected_version: u64,
    /// Repair state machine snapshot.
    pub repair_state: &'a RepairStateMachine,
}

impl<'a> StageContext<'a> {
    /// Build a context for tests and early wiring.
    pub fn new(
        current_step: FlowStep,
        loop_id: impl Into<String>,
        expected_version: u64,
        repair_state: &'a RepairStateMachine,
    ) -> Self {
        Self {
            current_step,
            loop_id: loop_id.into(),
            expected_version,
            repair_state,
        }
    }
}

/// Ordered pipeline of emit stages.
#[derive(Default)]
pub struct StagePipeline {
    stages: Vec<Box<dyn EmitStage>>,
}

impl StagePipeline {
    /// Create a pipeline from the given stages, preserving order.
    pub fn new(stages: Vec<Box<dyn EmitStage>>) -> Self {
        Self { stages }
    }

    /// Run the event through every stage in order.  The first
    /// rejection short-circuits and is returned.
    pub fn run(&self, ctx: &StageContext, event: &Event) -> Result<(), StageReject> {
        for stage in &self.stages {
            stage.check(ctx, event)?;
        }
        Ok(())
    }

    /// Names of the configured stages, in order.
    pub fn names(&self) -> Vec<&'static str> {
        self.stages.iter().map(|s| s.name()).collect()
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
