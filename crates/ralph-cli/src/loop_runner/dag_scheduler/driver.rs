//! 2026-09-03-0959 plan U6 — the `EventLoop` → pipeline driver.
//!
//! The runtime's `EventLoop` calls `DagSchedulerDriver::observe_accepted`
//! when a worker-emitted event passes the existing acceptance
//! gate. The driver routes the event into the right pipeline
//! slot:
//!   - `forge.exec.unit.completed` → `JobPipeline::advance(unit, Review)`
//!   - `forge.review.verdict` (approve) → `JobPipeline::advance(unit, Verify)`
//!   - `forge.review.verdict` (request_changes) →
//!     `JobPipeline::bump_attempt_and_advance(unit, Review)`
//!   - any other topic → ignored (driver is observation-only)
//!
//! On `Block`, the driver returns the typed reason so the caller
//! can publish `forge.plan.blocked` (or `forge.final.correction.settled`,
//! per U4 / U5 wiring).
//!
//! The driver does NOT spawn subprocesses. It only routes
//! events. Subprocesses are launched by the runtime's job
//! kernel (`runtime_job::worker`), which the runtime invokes
//! after the driver returns `Admitted`.

#[cfg(test)]
use serde_json::Value;

#[cfg(test)]
use super::jobs::{AdvanceOutcome, JobPipeline};
#[cfg(test)]
use crate::loop_runner::runtime_job::JobToken;
#[cfg(test)]
use crate::loop_runner::runtime_job::{RuntimeJobError, Stage};

/// Topics the driver recognises. The list is intentionally
/// narrow — anything outside it is a no-op so the driver never
/// silently corrupts a pipeline slot.
///
/// `#[cfg(test)]` for U6: only the driver test mod and the
/// `inspect` integration test reference these constants.
/// U7 promotes them to pub once the integration half hands the
/// driver to the live runtime.
#[cfg(test)]
pub mod topics {
    pub const EXEC_UNIT_COMPLETED: &str = "forge.exec.unit.completed";
    pub const REVIEW_VERDICT: &str = "forge.review.verdict";
}

/// `#[cfg(test)]` for U6 — see `topics` rationale. U7 promotes
/// it.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
}

#[cfg(test)]
impl ReviewVerdict {
    /// Parse from the event payload's `verdict` field. Returns
    /// `None` if the field is missing or unrecognised — the
    /// driver treats that as a no-op so a malformed event does
    /// not advance state.
    pub fn from_payload(payload: &Value) -> Option<Self> {
        let s = payload.get("verdict")?.as_str()?;
        match s {
            "approve" => Some(Self::Approve),
            "request_changes" => Some(Self::RequestChanges),
            _ => None,
        }
    }
}

/// Outcome of a single `observe_accepted` call.
///
/// `#[cfg(test)]` for U6 — see `topics` rationale. U7 promotes
/// it.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverOutcome {
    /// Driver routed the event into the pipeline; the caller
    /// can proceed with the (token, next_stage) pair.
    Routed {
        unit_key: String,
        next_stage: Stage,
        token: JobToken,
    },
    /// Event did not match any driver topic — the runtime
    /// should leave the pipeline alone.
    Ignored { topic: String },
    /// Driver routed but the pipeline rejected (pool exhausted,
    /// global cap, illegal transition, fix budget exhausted).
    Blocked {
        unit_key: String,
        error: RuntimeJobError,
    },
    /// Driver routed but the kernel's collect returned
    /// `CollectFailed`; pipeline state is unchanged.
    StillExecuting { unit_key: String, stage: Stage },
}

/// The driver is a thin handle over a `JobPipeline` so the
/// `EventLoop` can hand accepted events to it without owning
/// the pipeline's mutable state.
///
/// `#[cfg(test)]` for U6 — see `topics` rationale. U7 promotes
/// it once the live runtime drives the driver.
#[cfg(test)]
pub struct DagSchedulerDriver<'a> {
    pipeline: &'a mut JobPipeline,
}

#[cfg(test)]
impl<'a> DagSchedulerDriver<'a> {
    pub fn new(pipeline: &'a mut JobPipeline) -> Self {
        Self { pipeline }
    }

    /// Route one accepted event into the pipeline.
    pub fn observe_accepted(
        &mut self,
        topic: &str,
        unit_key: &str,
        payload: &Value,
    ) -> DriverOutcome {
        match topic {
            topics::EXEC_UNIT_COMPLETED => {
                // Executor finished — advance to Review.
                match self.pipeline.advance(unit_key, Stage::Review) {
                    AdvanceOutcome::Admitted { token } => DriverOutcome::Routed {
                        unit_key: unit_key.to_string(),
                        next_stage: Stage::Review,
                        token,
                    },
                    AdvanceOutcome::StillExecuting { unit_key, stage } => {
                        DriverOutcome::StillExecuting { unit_key, stage }
                    }
                    AdvanceOutcome::Blocked(error) => DriverOutcome::Blocked {
                        unit_key: unit_key.to_string(),
                        error,
                    },
                }
            }
            topics::REVIEW_VERDICT => match ReviewVerdict::from_payload(payload) {
                Some(ReviewVerdict::Approve) => {
                    match self.pipeline.advance(unit_key, Stage::Verify) {
                        AdvanceOutcome::Admitted { token } => DriverOutcome::Routed {
                            unit_key: unit_key.to_string(),
                            next_stage: Stage::Verify,
                            token,
                        },
                        AdvanceOutcome::StillExecuting { unit_key, stage } => {
                            DriverOutcome::StillExecuting { unit_key, stage }
                        }
                        AdvanceOutcome::Blocked(error) => DriverOutcome::Blocked {
                            unit_key: unit_key.to_string(),
                            error,
                        },
                    }
                }
                Some(ReviewVerdict::RequestChanges) => {
                    match self
                        .pipeline
                        .bump_attempt_and_advance(unit_key, Stage::Review)
                    {
                        AdvanceOutcome::Admitted { token } => DriverOutcome::Routed {
                            unit_key: unit_key.to_string(),
                            next_stage: Stage::Review,
                            token,
                        },
                        AdvanceOutcome::StillExecuting { unit_key, stage } => {
                            DriverOutcome::StillExecuting { unit_key, stage }
                        }
                        AdvanceOutcome::Blocked(error) => DriverOutcome::Blocked {
                            unit_key: unit_key.to_string(),
                            error,
                        },
                    }
                }
                None => DriverOutcome::Ignored {
                    topic: topic.to_string(),
                },
            },
            _ => DriverOutcome::Ignored {
                topic: topic.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_runner::dag_scheduler::jobs::DagPools;
    use serde_json::json;

    fn driver_fixture() -> (DagPools, JobPipeline) {
        let pools = DagPools::new(4, 2, 2, 2);
        let pipeline = JobPipeline::new(pools.clone());
        (pools, pipeline)
    }

    /// Exec-complete routes to Review.
    #[test]
    fn exec_complete_routes_to_review() {
        let (_pools, mut pipeline) = driver_fixture();
        pipeline.ensure_unit("U-1", "j-1", "executor", Stage::Execute);
        let _ = pipeline.advance("U-1", Stage::Execute);
        pipeline.release("U-1");
        let mut driver = DagSchedulerDriver::new(&mut pipeline);
        let out = driver.observe_accepted(topics::EXEC_UNIT_COMPLETED, "U-1", &json!({}));
        match out {
            DriverOutcome::Routed {
                unit_key,
                next_stage,
                ..
            } => {
                assert_eq!(unit_key, "U-1");
                assert_eq!(next_stage, Stage::Review);
            }
            other => panic!("expected Routed, got {other:?}"),
        }
    }

    /// Review-approve routes to Verify.
    #[test]
    fn review_approve_routes_to_verify() {
        let (_pools, mut pipeline) = driver_fixture();
        pipeline.ensure_unit("U-2", "j-2", "executor", Stage::Execute);
        let _ = pipeline.advance("U-2", Stage::Execute);
        pipeline.release("U-2");
        let _ = pipeline.advance("U-2", Stage::Review);
        let mut driver = DagSchedulerDriver::new(&mut pipeline);
        let out = driver.observe_accepted(
            topics::REVIEW_VERDICT,
            "U-2",
            &json!({"verdict": "approve"}),
        );
        match out {
            DriverOutcome::Routed { next_stage, .. } => {
                assert_eq!(next_stage, Stage::Verify);
            }
            other => panic!("expected Routed, got {other:?}"),
        }
    }

    /// Review-request_changes bumps attempt and re-routes to
    /// Review with a fresh token.
    #[test]
    fn review_request_changes_bumps_attempt() {
        let (_pools, mut pipeline) = driver_fixture();
        pipeline.ensure_unit("U-3", "j-3", "executor", Stage::Execute);
        let _ = pipeline.advance("U-3", Stage::Execute);
        pipeline.release("U-3");
        let _ = pipeline.advance("U-3", Stage::Review);
        pipeline.release("U-3");
        let mut driver = DagSchedulerDriver::new(&mut pipeline);
        let out = driver.observe_accepted(
            topics::REVIEW_VERDICT,
            "U-3",
            &json!({"verdict": "request_changes"}),
        );
        match out {
            DriverOutcome::Routed { token, .. } => {
                assert_eq!(token.attempt(), 1);
            }
            other => panic!("expected Routed, got {other:?}"),
        }
    }

    /// Unrecognised topics are no-ops.
    #[test]
    fn unrecognised_topic_is_ignored() {
        let (_pools, mut pipeline) = driver_fixture();
        let mut driver = DagSchedulerDriver::new(&mut pipeline);
        let out = driver.observe_accepted("ralph.unknown.topic", "U-x", &json!({}));
        assert!(matches!(out, DriverOutcome::Ignored { .. }));
    }
}
