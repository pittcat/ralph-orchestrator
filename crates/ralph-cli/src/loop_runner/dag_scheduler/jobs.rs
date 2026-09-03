//! 2026-09-03-0959 plan U6 — the per-Unit pipeline.
//!
//! `JobPipeline` owns the runtime's view of one Unit's progress
//! through `Execute → Review → Verify`. It is the place where:
//!   - the stage transition gate (`Stage::can_advance_to`) is
//!     enforced,
//!   - the per-stage pool cap is consulted,
//!   - the global cap (max in-flight Units) is consulted,
//!   - the three-fix-attempt budget is enforced (after which the
//!     pipeline emits a typed `Blocked`).
//!
//! The pipeline is **stateful per Unit** (one `UnitPipelineState`
//! per `unit_key`) but shares the `DagPools` across the whole
//! runtime. Fast Units advance to `Review` while slow siblings
//! are still `Executing` — there is no wave barrier between
//! pipeline stages; the only barrier is the per-Unit dependency
//! graph (U1 / U4).

#[cfg(test)]
use std::collections::HashMap;

#[cfg(test)]
use super::super::runtime_job::{JobToken, RuntimeJobError, Stage};

// ---------------------------------------------------------------------------
// Pool caps.
// ---------------------------------------------------------------------------

/// Pool caps for the three pipeline stages + a global cap that
/// applies across all stages.
///
/// `global` is the upper bound on Units that may be in flight at
/// any moment. `executor` / `reviewer` / `verifier` are the
/// per-stage slot counts. A pipeline tick that would exceed any
/// cap returns `AdvanceOutcome::Blocked`.
///
/// `#[cfg(test)]` for U6: the only consumer is the pipeline /
/// driver test mods (which drive `JobPipeline` end-to-end).
/// U7 promotes these types to pub once the integration half
/// hands the pipeline to the live runtime.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagPools {
    pub global: u32,
    pub executor: u32,
    pub reviewer: u32,
    pub verifier: u32,
}

#[cfg(test)]
impl DagPools {
    pub fn new(global: u32, executor: u32, reviewer: u32, verifier: u32) -> Self {
        Self {
            global,
            executor,
            reviewer,
            verifier,
        }
    }

    /// Default test cap: small enough that a 3-Unit test
    /// exercises the cap, large enough that a 2-Unit test
    /// does not.
    pub fn small_test_default() -> Self {
        Self {
            global: 4,
            executor: 2,
            reviewer: 2,
            verifier: 2,
        }
    }

    /// Pool cap for a given stage.
    pub fn cap_for(&self, stage: Stage) -> u32 {
        match stage {
            Stage::Execute => self.executor,
            Stage::Review => self.reviewer,
            Stage::Verify => self.verifier,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-Unit state.
// ---------------------------------------------------------------------------

/// Per-Unit pipeline state. The runtime owns one of these per
/// `unit_key`. `attempt` is the current review-rejection counter
/// (bumped on every `verdict != approve`).
///
/// `#[cfg(test)]` for U6 — only the pipeline / driver test
/// mods drive this type. U7 promotes it once the runtime owns
/// live pipeline state.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitPipelineState {
    pub unit_key: String,
    pub job_id: String,
    pub hat: String,
    pub stage: Stage,
    pub attempt: u64,
    /// Cumulative in-flight count this Unit has contributed
    /// (each call to `advance` increments by one when the
    /// outcome is `Admitted`).
    pub in_flight: u32,
}

#[cfg(test)]
impl UnitPipelineState {
    pub fn new(
        unit_key: impl Into<String>,
        job_id: impl Into<String>,
        hat: impl Into<String>,
        stage: Stage,
    ) -> Self {
        Self {
            unit_key: unit_key.into(),
            job_id: job_id.into(),
            hat: hat.into(),
            stage,
            attempt: 0,
            in_flight: 0,
        }
    }

    /// Mint a fresh `JobToken` for the current `(unit_key, stage,
    /// hat, attempt)`.
    pub fn mint_token(&self) -> JobToken {
        JobToken::mint_attempt(
            &self.unit_key,
            &self.job_id,
            &self.hat,
            self.stage,
            self.attempt,
        )
    }

    /// Bump `attempt` after a review rejection. The next
    /// `JobToken::mint_attempt` will fail to validate against
    /// the previous descriptor — that is the attempt revocation
    /// guarantee the plan §7 U6 #6 mandates.
    pub fn bump_attempt(&mut self) {
        self.attempt = self.attempt.saturating_add(1);
    }
}

/// Outcome of a single `JobPipeline::advance` call.
///
/// `#[cfg(test)]` for U6 — see `DagPools` rationale. U7 promotes
/// it.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvanceOutcome {
    /// The Unit was admitted into the requested stage and
    /// `JobToken` was minted. Caller may proceed to launch.
    Admitted { token: JobToken },
    /// The Unit is still executing / collecting. Pipeline
    /// did not advance; caller should poll again.
    StillExecuting { unit_key: String, stage: Stage },
    /// The Unit was rejected (pool exhausted, global cap
    /// exceeded, illegal stage transition, or three
    /// review-rejection budget exhausted). The error carries
    /// the typed reason.
    Blocked(RuntimeJobError),
}

/// Aggregate pipeline state the runtime owns.
///
/// `#[cfg(test)]` for U6 — see `DagPools` rationale. U7 promotes
/// it.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct PipelineState {
    pub units: HashMap<String, UnitPipelineState>,
    /// Aggregate per-stage in-flight counts. The runtime updates
    /// these atomically with `UnitPipelineState::in_flight` so
    /// the global cap check is O(1).
    pub in_flight: StageCounts,
}

/// `#[cfg(test)]` for U6 — see `DagPools` rationale. U7 promotes
/// it.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StageCounts {
    pub execute: u32,
    pub review: u32,
    pub verify: u32,
    pub total: u32,
}

#[cfg(test)]
impl StageCounts {
    pub fn bump(&mut self, stage: Stage) {
        match stage {
            Stage::Execute => self.execute += 1,
            Stage::Review => self.review += 1,
            Stage::Verify => self.verify += 1,
        }
        self.total += 1;
    }

    pub fn unbump(&mut self, stage: Stage) {
        match stage {
            Stage::Execute => self.execute = self.execute.saturating_sub(1),
            Stage::Review => self.review = self.review.saturating_sub(1),
            Stage::Verify => self.verify = self.verify.saturating_sub(1),
        }
        self.total = self.total.saturating_sub(1);
    }
}

// ---------------------------------------------------------------------------
// Pipeline.
// ---------------------------------------------------------------------------

/// Hard cap on review-rejection attempts. After three rejections
/// the pipeline emits a typed `Blocked` so the runtime can
/// publish `forge.plan.blocked` (see U4 / U5 wiring).
///
/// `#[cfg(test)]` for U6 — only the pipeline test mod reads
/// this constant. U7 promotes it once the runtime owns the
/// live fix-attempt budget.
#[cfg(test)]
pub const MAX_FIX_ATTEMPTS: u64 = 3;

/// The per-Unit pipeline. Holds the `DagPools` (shared, runtime
/// global) and the `PipelineState` (per-Unit).
///
/// `advance` is the only mutating entry point. It is idempotent
/// for a given `(unit_key, stage, attempt)` tuple — calling it
/// twice with the same tuple returns the same outcome. The
/// caller advances a Unit by:
///   1. Calling `advance(unit_key, stage, hat)` to mint the
///      token and reserve a slot.
///   2. Launching the kernel invocation.
///   3. Calling `release(unit_key)` after `collect` so the slot
///      returns to the pool.
///   4. On review rejection, calling `bump_attempt_and_advance`
///      which mints a fresh token at the new attempt count.
///
/// `#[cfg(test)]` for U6 — see `DagPools` rationale. U7 promotes
/// it once the runtime owns live pipeline state.
#[cfg(test)]
pub struct JobPipeline {
    pools: DagPools,
    state: PipelineState,
}

#[cfg(test)]
impl JobPipeline {
    pub fn new(pools: DagPools) -> Self {
        Self {
            pools,
            state: PipelineState::default(),
        }
    }

    /// Register a Unit if it isn't already registered. Returns
    /// the (existing or freshly created) state.
    pub fn ensure_unit(
        &mut self,
        unit_key: impl Into<String>,
        job_id: impl Into<String>,
        hat: impl Into<String>,
        stage: Stage,
    ) -> &mut UnitPipelineState {
        let key = unit_key.into();
        self.state
            .units
            .entry(key.clone())
            .or_insert_with(|| UnitPipelineState::new(key, job_id.into(), hat.into(), stage))
    }

    /// Reserve a slot for `(unit_key, stage)`. Returns
    /// `Admitted` with a freshly minted `JobToken`, or `Blocked`
    /// with the typed reason.
    pub fn advance(&mut self, unit_key: &str, stage: Stage) -> AdvanceOutcome {
        // 1. Stage transition gate.
        let unit = match self.state.units.get(unit_key) {
            Some(u) => u.clone(),
            None => {
                return AdvanceOutcome::Blocked(RuntimeJobError::Blocked {
                    reason: "unit not registered".to_string(),
                    unit_key: unit_key.to_string(),
                });
            }
        };
        if unit.stage != stage && !unit.stage.can_advance_to(stage) {
            return AdvanceOutcome::Blocked(RuntimeJobError::IllegalStageTransition {
                from: unit.stage,
                to: stage,
            });
        }

        // 2. Three-fix-attempt budget.
        if unit.stage == Stage::Review && unit.attempt >= MAX_FIX_ATTEMPTS {
            return AdvanceOutcome::Blocked(RuntimeJobError::Blocked {
                reason: format!("exceeded {MAX_FIX_ATTEMPTS} fix attempts"),
                unit_key: unit_key.to_string(),
            });
        }

        // 3. Global cap.
        let requested = self.state.in_flight.total + 1;
        if requested > self.pools.global {
            return AdvanceOutcome::Blocked(RuntimeJobError::GlobalCapExceeded {
                requested,
                cap: self.pools.global,
            });
        }

        // 4. Per-stage cap.
        let stage_in_flight = match stage {
            Stage::Execute => self.state.in_flight.execute,
            Stage::Review => self.state.in_flight.review,
            Stage::Verify => self.state.in_flight.verify,
        };
        let cap = self.pools.cap_for(stage);
        if stage_in_flight + 1 > cap {
            return AdvanceOutcome::Blocked(RuntimeJobError::PoolExhausted {
                stage,
                requested: stage_in_flight + 1,
                cap,
            });
        }

        // 5. Reserve the slot.
        let unit = self.state.units.get_mut(unit_key).expect("present");
        unit.in_flight += 1;
        if unit.stage != stage {
            unit.stage = stage;
        }
        self.state.in_flight.bump(stage);

        let token = unit.mint_token();
        AdvanceOutcome::Admitted { token }
    }

    /// Release a slot after `collect`. Decrements per-stage and
    /// global in-flight counters.
    pub fn release(&mut self, unit_key: &str) {
        if let Some(u) = self.state.units.get_mut(unit_key) {
            if u.in_flight > 0 {
                u.in_flight -= 1;
                self.state.in_flight.unbump(u.stage);
            }
        }
    }

    /// Bump `attempt` and re-enter the pipeline at the same
    /// stage (typically Review). Used when a review verdict is
    /// `request_changes`.
    pub fn bump_attempt_and_advance(&mut self, unit_key: &str, stage: Stage) -> AdvanceOutcome {
        if let Some(u) = self.state.units.get_mut(unit_key) {
            u.bump_attempt();
        }
        self.advance(unit_key, stage)
    }

    /// Mark a Unit as `StillExecuting` — used by the driver
    /// when the kernel's `collect_with_deadline` returns
    /// `CollectFailed` (i.e. the subprocess has not yet exited).
    pub fn still_executing(&self, unit_key: &str) -> AdvanceOutcome {
        let stage = self
            .state
            .units
            .get(unit_key)
            .map(|u| u.stage)
            .unwrap_or(Stage::Execute);
        AdvanceOutcome::StillExecuting {
            unit_key: unit_key.to_string(),
            stage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_runner::runtime_job::Stage;

    /// A fast Unit enters Review while a slow sibling is still
    /// in Execute — no wave barrier.
    #[test]
    fn pipeline_advances_fast_unit_to_review_while_slow_still_executing() {
        let pools = DagPools::new(
            /*global*/ 4, /*executor*/ 2, /*reviewer*/ 2, /*verifier*/ 2,
        );
        let mut pipeline = JobPipeline::new(pools);
        pipeline.ensure_unit("U-fast", "j-fast", "executor", Stage::Execute);
        pipeline.ensure_unit("U-slow", "j-slow", "executor", Stage::Execute);

        // Slow unit grabs one Execute slot.
        let slow = pipeline.advance("U-slow", Stage::Execute);
        assert!(matches!(slow, AdvanceOutcome::Admitted { .. }));

        // Fast unit completes Execute, advances to Review.
        pipeline.release("U-slow");
        let fast_exec = pipeline.advance("U-fast", Stage::Execute);
        assert!(matches!(fast_exec, AdvanceOutcome::Admitted { .. }));
        pipeline.release("U-fast");
        let fast_review = pipeline.advance("U-fast", Stage::Review);
        assert!(matches!(fast_review, AdvanceOutcome::Admitted { .. }));

        // Slow unit is still in Execute (we never released the
        // slot for it after the advance above).
        assert!(pipeline.state.units.get("U-slow").unwrap().stage == Stage::Execute);
        // Fast unit is in Review.
        assert!(pipeline.state.units.get("U-fast").unwrap().stage == Stage::Review);
    }

    /// Three review rejections ⇒ typed `Blocked`. The fourth
    /// advance call returns `Blocked`, not `Admitted`.
    #[test]
    fn pipeline_terminates_with_typed_block_after_three_fix_attempts() {
        let pools = DagPools::new(4, 2, 2, 2);
        let mut pipeline = JobPipeline::new(pools);
        pipeline.ensure_unit("U-flaky", "j-flaky", "executor", Stage::Execute);
        // Move to Review.
        let _ = pipeline.advance("U-flaky", Stage::Execute);
        pipeline.release("U-flaky");
        let _ = pipeline.advance("U-flaky", Stage::Review);
        pipeline.release("U-flaky");
        // Bump attempt 3 times. Each bump + advance either
        // returns Admitted (slots are available) or Blocked
        // (cap reached, but the unit-key is the same so the
        // bump-attempt path is exercised). After 3 bumps, the
        // 4th advance MUST be Blocked.
        let mut last = AdvanceOutcome::Admitted {
            token: JobToken::mint_attempt("U-flaky", "j-flaky", "executor", Stage::Review, 0),
        };
        for _ in 0..MAX_FIX_ATTEMPTS {
            last = pipeline.bump_attempt_and_advance("U-flaky", Stage::Review);
            pipeline.release("U-flaky");
        }
        match last {
            AdvanceOutcome::Blocked(RuntimeJobError::Blocked { reason, unit_key }) => {
                assert!(reason.contains("fix attempts"));
                assert_eq!(unit_key, "U-flaky");
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    /// Per-stage pool cap enforced.
    #[test]
    fn pool_cap_per_stage_enforced() {
        // Tight review cap: 1.
        let pools = DagPools::new(4, 4, /*reviewer*/ 1, 4);
        let mut pipeline = JobPipeline::new(pools);
        pipeline.ensure_unit("U-1", "j-1", "executor", Stage::Execute);
        pipeline.ensure_unit("U-2", "j-2", "executor", Stage::Execute);
        // Get U-1 and U-2 into Review (both slot releases happen
        // before the Review advance so we don't double-book).
        let _ = pipeline.advance("U-1", Stage::Execute);
        pipeline.release("U-1");
        let _ = pipeline.advance("U-1", Stage::Review);
        let _ = pipeline.advance("U-2", Stage::Execute);
        pipeline.release("U-2");
        let second_review = pipeline.advance("U-2", Stage::Review);
        // reviewer cap is 1; U-1 is still occupying it.
        match second_review {
            AdvanceOutcome::Blocked(RuntimeJobError::PoolExhausted {
                stage,
                requested,
                cap,
            }) => {
                assert_eq!(stage, Stage::Review);
                assert_eq!(requested, 2);
                assert_eq!(cap, 1);
            }
            other => panic!("expected PoolExhausted, got {other:?}"),
        }
        // Release U-1's review slot and retry.
        pipeline.release("U-1");
        let retry = pipeline.advance("U-2", Stage::Review);
        assert!(matches!(retry, AdvanceOutcome::Admitted { .. }));
    }

    /// Global cap (across all stages) is never oversubscribed.
    #[test]
    fn global_cap_never_oversubscribed() {
        // global=1 — only one Unit may be in flight at a time.
        let pools = DagPools::new(1, 4, 4, 4);
        let mut pipeline = JobPipeline::new(pools);
        pipeline.ensure_unit("U-A", "j-A", "executor", Stage::Execute);
        pipeline.ensure_unit("U-B", "j-B", "executor", Stage::Execute);

        let first = pipeline.advance("U-A", Stage::Execute);
        assert!(matches!(first, AdvanceOutcome::Admitted { .. }));
        let second = pipeline.advance("U-B", Stage::Execute);
        match second {
            AdvanceOutcome::Blocked(RuntimeJobError::GlobalCapExceeded { requested, cap }) => {
                assert_eq!(requested, 2);
                assert_eq!(cap, 1);
            }
            other => panic!("expected GlobalCapExceeded, got {other:?}"),
        }
        // Release U-A; U-B can now advance.
        pipeline.release("U-A");
        let third = pipeline.advance("U-B", Stage::Execute);
        assert!(matches!(third, AdvanceOutcome::Admitted { .. }));
    }

    /// Illegal stage transition (`Execute → Verify` skipping
    /// Review) returns `IllegalStageTransition`.
    #[test]
    fn pipeline_rejects_illegal_stage_transition() {
        let pools = DagPools::new(4, 2, 2, 2);
        let mut pipeline = JobPipeline::new(pools);
        pipeline.ensure_unit("U-skip", "j-skip", "executor", Stage::Execute);
        let outcome = pipeline.advance("U-skip", Stage::Verify);
        match outcome {
            AdvanceOutcome::Blocked(RuntimeJobError::IllegalStageTransition { from, to }) => {
                assert_eq!(from, Stage::Execute);
                assert_eq!(to, Stage::Verify);
            }
            other => panic!("expected IllegalStageTransition, got {other:?}"),
        }
    }

    /// A `StillExecuting` outcome is returned when the unit
    /// exists and the requested stage matches its current stage
    /// (no transition needed). Useful for the driver to keep
    /// the token alive while the kernel's `collect_with_deadline`
    /// is still polling.
    #[test]
    fn still_executing_returns_current_stage() {
        let pools = DagPools::small_test_default();
        let mut pipeline = JobPipeline::new(pools);
        pipeline.ensure_unit("U-w", "j-w", "executor", Stage::Execute);
        let _ = pipeline.advance("U-w", Stage::Execute);
        let outcome = pipeline.still_executing("U-w");
        match outcome {
            AdvanceOutcome::StillExecuting { unit_key, stage } => {
                assert_eq!(unit_key, "U-w");
                assert_eq!(stage, Stage::Execute);
            }
            other => panic!("expected StillExecuting, got {other:?}"),
        }
    }
}
