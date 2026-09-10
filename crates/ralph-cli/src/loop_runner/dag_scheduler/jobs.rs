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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use super::super::runtime_job::{JobToken, RuntimeJobError, Stage};

// ---------------------------------------------------------------------------
// Path-prefix allowlist (U26, fix-plan 2026-09-09-0917).
// ---------------------------------------------------------------------------

/// Canonical-path-prefix containment check used to defend the
/// per-job `events_file` against symlink-chain escapes
/// (`/workspace/.ralph/dag/...` redirected outside the worktree).
///
/// Returns `Ok(true)` when `target` resolves to a path strictly
/// inside `workspace` after canonicalization, `Ok(false)` when
/// `target` escapes the workspace, and an error when
/// canonicalization itself fails (treated as fail-closed). For
/// non-existent targets (e.g. the events file before it is
/// created), the parent directory is canonicalized instead, which
/// still resolves any symlink-chain attack against the workspace
/// prefix because intermediate components of the path are walked
/// through `canonicalize`.
pub fn path_within_workspace(workspace: &Path, target: &Path) -> std::io::Result<bool> {
    let canonical_workspace = workspace.canonicalize()?;
    let resolved_target = match target.canonicalize() {
        Ok(p) => p,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = target.parent() {
                parent.canonicalize()?
            } else {
                return Err(err);
            }
        }
        Err(err) => return Err(err),
    };
    Ok(resolved_target.starts_with(&canonical_workspace))
}

// ---------------------------------------------------------------------------
// SHA validator (U5, fix-plan 2026-09-09-0917 F10/R5).
// ---------------------------------------------------------------------------

/// Lower-level SHA character-set + length check. Returns `true` for
/// 40-char SHA-1 or 64-char SHA-256 lowercase hex; rejects empty,
/// single-character, `0x`-prefixed, uppercase, or any other length.
///
/// U5 sync note: this is the single validator the runtime
/// persists on for OID character-set / length. The CLI-side
/// `JobIdentity` struct lives in
/// `crates/ralph-core/src/supervisor/dag_store_rusqlite/jobs.rs`;
/// any field it exposes that carries a commit OID (planned for
/// future `evidence` / `forge.exec.*` payload extensions) MUST
/// route through this helper to keep the F10 fail-closed
/// invariant intact across both crates. Mirror the regex here if
/// the validator moves to a shared location.
#[allow(dead_code)] // sync target for core's `JobIdentity::validate()`
pub fn is_valid_sha(value: &str) -> bool {
    let len = value.len();
    if len != 40 && len != 64 {
        return false;
    }
    value
        .bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

// ---------------------------------------------------------------------------
// Pool caps.
// ---------------------------------------------------------------------------

/// Pool caps for the three pipeline stages + a global cap that
/// applies across all stages, plus the D16 fourth pool for fixer
/// jobs.
///
/// `global` is the upper bound on Units that may be in flight at
/// any moment. `executor` / `reviewer` / `verifier` are the
/// per-stage slot counts. A pipeline tick that would exceed any
/// cap returns `AdvanceOutcome::Blocked`.
///
/// `fixer` caps the correction jobs spawned after a REJECTED
/// review. The pipeline `Stage` enum has no `Fix` variant (the
/// fix loop reuses the `Review` slot), so the fixer cap is
/// enforced by the spawn seam (`dag_scheduler` runtime) against
/// its in-flight fixer count, not by `JobPipeline::advance`.
/// Defaults to `reviewer` when not explicitly set so the legacy
/// 4-arg `new` call sites keep a sensible four-pool semantic.
///
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见。容量模型
/// 与 `max_concurrent_workers` 的单一权威收敛(promote 义务 #2,见
/// `presets/en/parallel-forge-preset-author-notes.md`「promote 前置
/// 义务清单」)属后续 Step;收敛前无 bin 侧生产调用方,故 item 级
/// `#[allow(dead_code)]`,接线落地后移除。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagPools {
    pub global: u32,
    pub executor: u32,
    pub reviewer: u32,
    pub verifier: u32,
    pub fixer: u32,
}

#[allow(dead_code)] // 同 `DagPools` 的 Step 1+2 promote 注释。
impl DagPools {
    pub fn new(global: u32, executor: u32, reviewer: u32, verifier: u32) -> Self {
        Self {
            global,
            executor,
            reviewer,
            verifier,
            // D16: an unset fixer pool inherits the reviewer cap so
            // the four-pool config degrades gracefully for callers
            // that predate the fixer field.
            fixer: reviewer,
        }
    }

    /// Set the D16 fixer-pool cap (builder form so existing `new`
    /// call sites stay untouched).
    pub fn with_fixer(mut self, fixer: u32) -> Self {
        self.fixer = fixer;
        self
    }

    /// Cap for fixer (correction) jobs — enforced by the spawn
    /// seam, see the struct-level doc.
    pub fn fixer_cap(&self) -> u32 {
        self.fixer
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
            fixer: 2,
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
/// (bumped on every `verdict == REJECTED`).
///
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;runtime
/// 接管 live pipeline state 前无 bin 侧消费方,item 级
/// `#[allow(dead_code)]`(同 `DagPools` 的 promote 注释)。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitPipelineState {
    pub unit_key: String,
    pub job_id: String,
    pub hat: String,
    pub stage: Stage,
    pub attempt: u64,
    /// Cumulative in-flight count this Unit has contributed
    /// (each call to `advance` increments by one when the
    /// outcome is `Admitted`). A Unit holds at most one slot at
    /// a time, so this is 0 or 1 in practice.
    pub in_flight: u32,
    /// The stage the currently held slot was reserved under.
    /// `release` unbumps THIS stage (not the Unit's current
    /// `stage`, which may already have migrated), so a
    /// slot can never leak into the wrong pool counter.
    /// `Some` iff `in_flight > 0`.
    pub in_flight_stage: Option<Stage>,
}

#[allow(dead_code)] // 同 `DagPools` 的 Step 1+2 promote 注释。
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
            in_flight_stage: None,
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
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;无 bin 侧
/// 生产调用方,item 级 `#[allow(dead_code)]`(同 `DagPools` 的
/// promote 注释)。
#[allow(dead_code)]
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
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;无 bin 侧
/// 生产调用方,item 级 `#[allow(dead_code)]`(同 `DagPools` 的
/// promote 注释)。
#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct PipelineState {
    pub units: HashMap<String, UnitPipelineState>,
    /// Aggregate per-stage in-flight counts. The runtime updates
    /// these atomically with `UnitPipelineState::in_flight` so
    /// the global cap check is O(1).
    pub in_flight: StageCounts,
}

/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;无 bin 侧
/// 生产调用方,item 级 `#[allow(dead_code)]`(同 `DagPools` 的
/// promote 注释)。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StageCounts {
    pub execute: u32,
    pub review: u32,
    pub verify: u32,
    pub total: u32,
}

#[allow(dead_code)] // 同 `DagPools` 的 Step 1+2 promote 注释。
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
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;runtime
/// 接管 fix-attempt 预算前无 bin 侧消费方,item 级
/// `#[allow(dead_code)]`(同 `DagPools` 的 promote 注释)。
#[allow(dead_code)]
pub const MAX_FIX_ATTEMPTS: u64 = 3;

/// The per-Unit pipeline. Holds the `DagPools` (shared, runtime
/// global) and the `PipelineState` (per-Unit).
///
/// `advance` is the only mutating entry point. Its slot
/// semantics:
///   - **Idempotent replay**: if the Unit already holds a slot
///     at the requested stage, `advance` returns `Admitted`
///     with the same deterministic `JobToken` (re-minted from
///     the unchanged `(unit_key, stage, hat, attempt)` tuple)
///     and touches NO counter. Calling it twice with the same
///     tuple is free.
///   - **Stage migration**: if the Unit holds a slot at a
///     different stage, the slot migrates atomically — the old
///     stage counter is unbumped and the new one bumped, so the
///     global total is unchanged and no cap check applies to
///     the global pool (the per-stage cap of the target stage
///     is still enforced).
///   - **Fresh reservation**: otherwise the global and
///     per-stage caps are consulted before any counter moves.
///   - **Budget first**: the three-fix-attempt budget is
///     checked for EVERY stage, before any bump; once the
///     budget is exhausted every further `advance` /
///     `bump_attempt_and_advance` returns the typed `Blocked`.
///
/// The caller advances a Unit by:
///   1. Calling `advance(unit_key, stage)` to mint the
///      token and reserve a slot.
///   2. Launching the kernel invocation.
///   3. Calling `release(unit_key)` after `collect` so the
///      slot returns to the pool it was reserved from.
///   4. On review rejection, calling `bump_attempt_and_advance`
///      which mints a fresh token at the new attempt count.
///
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;runtime
/// 接管 live pipeline state 前无 bin 侧消费方,item 级
/// `#[allow(dead_code)]`(同 `DagPools` 的 promote 注释)。
#[allow(dead_code)]
pub struct JobPipeline {
    pools: DagPools,
    state: PipelineState,
}

#[allow(dead_code)] // 同 `DagPools` 的 Step 1+2 promote 注释。
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

    /// Restore a unit from the durable launch journal after a process
    /// restart. Recovery deliberately restores no in-flight slot: an
    /// unresolved launch is settled by the recovery planner first, and an
    /// adopted child is the only path that reserves a live slot again.
    pub fn restore_unit(
        &mut self,
        unit_key: impl Into<String>,
        job_id: impl Into<String>,
        hat: impl Into<String>,
        stage: Stage,
        attempt: u64,
    ) -> &mut UnitPipelineState {
        let key = unit_key.into();
        let job_id = job_id.into();
        let hat = hat.into();
        let state = self
            .state
            .units
            .entry(key.clone())
            .or_insert_with(|| UnitPipelineState::new(key, job_id.clone(), hat.clone(), stage));
        state.job_id = job_id;
        state.hat = hat;
        state.stage = stage;
        state.attempt = attempt;
        state.in_flight = 0;
        state.in_flight_stage = None;
        state
    }

    /// Reserve a slot for `(unit_key, stage)`. Returns
    /// `Admitted` with a freshly minted `JobToken`, or `Blocked`
    /// with the typed reason. See the struct-level doc for the
    /// replay / migration / fresh-reservation semantics.
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

        // 2. Three-fix-attempt budget — checked for EVERY stage,
        //    before any counter or attempt is touched.
        if unit.attempt >= MAX_FIX_ATTEMPTS {
            return AdvanceOutcome::Blocked(RuntimeJobError::Blocked {
                reason: format!("exceeded {MAX_FIX_ATTEMPTS} fix attempts"),
                unit_key: unit_key.to_string(),
            });
        }

        // 3. Idempotent replay: the Unit already holds a slot at
        //    this exact stage. Re-mint the deterministic token
        //    (same `(unit_key, stage, hat, attempt)` tuple) and
        //    return without touching any counter.
        if unit.in_flight > 0 && unit.in_flight_stage == Some(stage) {
            let token = unit.mint_token();
            return AdvanceOutcome::Admitted { token };
        }

        // A Unit holding a slot at a DIFFERENT stage migrates that
        // slot; the global total is unchanged by a migration.
        let migrating = unit.in_flight > 0;

        // 4. Global cap — skipped on migration (the total does
        //    not grow).
        if !migrating {
            let requested = self.state.in_flight.total + 1;
            if requested > self.pools.global {
                return AdvanceOutcome::Blocked(RuntimeJobError::GlobalCapExceeded {
                    requested,
                    cap: self.pools.global,
                });
            }
        }

        // 5. Per-stage cap (always consulted for the target stage).
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

        // 6. Reserve the slot — or migrate it from the stage it
        //    was originally reserved under.
        let unit = self.state.units.get_mut(unit_key).expect("present");
        if migrating {
            if let Some(old) = unit.in_flight_stage {
                self.state.in_flight.unbump(old);
            }
        } else {
            unit.in_flight += 1;
        }
        self.state.in_flight.bump(stage);
        unit.in_flight_stage = Some(stage);
        if unit.stage != stage {
            unit.stage = stage;
        }

        let token = unit.mint_token();
        AdvanceOutcome::Admitted { token }
    }

    /// Release a slot after `collect`. Decrements the per-stage
    /// counter the slot was RESERVED under (recorded at
    /// `advance` time in `in_flight_stage`) plus the global
    /// counter — never the Unit's current stage, which may
    /// already have migrated.
    pub fn release(&mut self, unit_key: &str) {
        if let Some(u) = self.state.units.get_mut(unit_key)
            && u.in_flight > 0
        {
            u.in_flight -= 1;
            if let Some(reserved_stage) = u.in_flight_stage.take() {
                self.state.in_flight.unbump(reserved_stage);
            }
        }
    }

    /// Bump `attempt` and re-enter the pipeline at the same
    /// stage (typically Review). Used when a review verdict is
    /// `REJECTED`. The fix-attempt budget is checked
    /// BEFORE the bump, so `attempt` can never exceed
    /// `MAX_FIX_ATTEMPTS`.
    pub fn bump_attempt_and_advance(&mut self, unit_key: &str, stage: Stage) -> AdvanceOutcome {
        if let Some(u) = self.state.units.get(unit_key)
            && u.attempt >= MAX_FIX_ATTEMPTS
        {
            return AdvanceOutcome::Blocked(RuntimeJobError::Blocked {
                reason: format!("exceeded {MAX_FIX_ATTEMPTS} fix attempts"),
                unit_key: unit_key.to_string(),
            });
        }
        if let Some(u) = self.state.units.get_mut(unit_key) {
            u.bump_attempt();
        }
        self.advance(unit_key, stage)
    }

    /// Read-only access to the pool caps — the Step 4 observation
    /// tick translates them into `AdmissionCaps`.
    pub fn pools(&self) -> &DagPools {
        &self.pools
    }

    /// Snapshot of per-stage in-flight counts keyed by stable
    /// stage name (`"execute"` / `"review"` / `"verify"`). The
    /// admission engine seeds `global_cap` accounting with the
    /// sum of these values so a slot freed by `release()` (U1)
    /// is immediately refillable. Returns owned `String` keys
    /// because the snapshot crosses the ralph-core admission
    /// boundary, which takes owned map keys for ergonomics.
    pub fn live_stage_counts(&self) -> BTreeMap<String, u32> {
        let mut out: BTreeMap<String, u32> = BTreeMap::new();
        let inflight = self.state.in_flight;
        let pairs: [(Stage, u32); 3] = [
            (Stage::Execute, inflight.execute),
            (Stage::Review, inflight.review),
            (Stage::Verify, inflight.verify),
        ];
        for (stage, count) in pairs {
            if count > 0 {
                out.insert(stage.as_str().to_string(), count);
            }
        }
        out
    }

    /// Snapshot of `unit_key`s currently holding at least one
    /// in-flight slot (`in_flight > 0`). The admission engine
    /// uses this to short-circuit `BlockedDependencies` on units
    /// already running — without it, a fast unit that already
    /// passed admission could be re-admitted next tick while it
    /// is still live, double-booking its executor slot.
    pub fn live_unit_ids(&self) -> HashSet<String> {
        self.state
            .units
            .iter()
            .filter(|(_, u)| u.in_flight > 0)
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// Read-only access to a Unit's current stage. Used by the
    /// Step 4 seam's test introspection.
    #[cfg(test)]
    pub fn stage_of(&self, unit_key: &str) -> Option<Stage> {
        self.state.units.get(unit_key).map(|u| u.stage)
    }

    #[cfg(test)]
    pub fn attempt_of(&self, unit_key: &str) -> Option<u64> {
        self.state.units.get(unit_key).map(|u| u.attempt)
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

    #[test]
    fn restore_unit_rehydrates_state_without_reserving_a_slot() {
        let mut pipeline = JobPipeline::new(DagPools::small_test_default());
        let state = pipeline.restore_unit("forge:plan:U1", "job-1", "reviewer", Stage::Review, 2);
        assert_eq!(state.job_id, "job-1");
        assert_eq!(state.attempt, 2);
        assert_eq!(state.in_flight, 0);
        assert_eq!(state.in_flight_stage, None);
        assert_eq!(pipeline.state.in_flight.total, 0);
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

    /// P1-2 (true idempotence): calling `advance` twice with the
    /// same `(unit_key, stage, attempt)` tuple returns the same
    /// token and bumps NO counter a second time.
    #[test]
    fn advance_same_tuple_is_idempotent() {
        let pools = DagPools::small_test_default();
        let mut pipeline = JobPipeline::new(pools);
        pipeline.ensure_unit("U-idem", "j-idem", "executor", Stage::Execute);

        let first = pipeline.advance("U-idem", Stage::Execute);
        let second = pipeline.advance("U-idem", Stage::Execute);
        let (AdvanceOutcome::Admitted { token: t1 }, AdvanceOutcome::Admitted { token: t2 }) =
            (first, second)
        else {
            panic!("both calls must be Admitted");
        };
        assert_eq!(t1, t2, "same tuple must re-mint the same token");
        assert_eq!(
            pipeline.state.in_flight,
            StageCounts {
                execute: 1,
                review: 0,
                verify: 0,
                total: 1,
            },
            "a replayed advance must not double-count the slot"
        );

        // Release once; every counter returns to zero.
        pipeline.release("U-idem");
        assert_eq!(pipeline.state.in_flight, StageCounts::default());
    }

    /// P1-1 (slot leak): a stage migration (`advance(Execute)` →
    /// `advance(Review)` with no release in between) moves the
    /// slot atomically — the Execute counter is unbumped, Review
    /// is bumped, the global total is unchanged, and `release`
    /// afterwards unbumps the stage the slot was reserved under.
    #[test]
    fn stage_migration_moves_slot_without_leak() {
        let pools = DagPools::small_test_default();
        let mut pipeline = JobPipeline::new(pools);
        pipeline.ensure_unit("U-mig", "j-mig", "executor", Stage::Execute);

        let exec = pipeline.advance("U-mig", Stage::Execute);
        assert!(matches!(exec, AdvanceOutcome::Admitted { .. }));
        assert_eq!(
            pipeline.state.in_flight,
            StageCounts {
                execute: 1,
                review: 0,
                verify: 0,
                total: 1,
            }
        );

        // Migrate Execute → Review WITHOUT a release in between.
        let review = pipeline.advance("U-mig", Stage::Review);
        assert!(matches!(review, AdvanceOutcome::Admitted { .. }));
        assert_eq!(
            pipeline.state.in_flight,
            StageCounts {
                execute: 0,
                review: 1,
                verify: 0,
                total: 1,
            },
            "migration must unbump Execute and bump Review; total stays 1"
        );

        // Release frees the Review slot (the one actually held),
        // not the Unit's historical Execute stage.
        pipeline.release("U-mig");
        assert_eq!(
            pipeline.state.in_flight,
            StageCounts::default(),
            "no counter may leak after migrate + release"
        );
        // The freed global slot is immediately reusable.
        pipeline.ensure_unit("U-next", "j-next", "executor", Stage::Execute);
        let next = pipeline.advance("U-next", Stage::Execute);
        assert!(matches!(next, AdvanceOutcome::Admitted { .. }));
    }

    /// P1-3 (budget before bump, all stages): `attempt` can never
    /// exceed `MAX_FIX_ATTEMPTS`, and once the budget is exhausted
    /// EVERY stage advance is `Blocked`.
    #[test]
    fn fix_attempt_budget_checked_before_bump_for_all_stages() {
        let pools = DagPools::small_test_default();
        let mut pipeline = JobPipeline::new(pools);
        pipeline.ensure_unit("U-budget", "j-budget", "executor", Stage::Execute);

        // Drive the unit to Review, then reject three times.
        let _ = pipeline.advance("U-budget", Stage::Execute);
        pipeline.release("U-budget");
        let _ = pipeline.advance("U-budget", Stage::Review);
        pipeline.release("U-budget");
        for expected_attempt in 1..=MAX_FIX_ATTEMPTS {
            let out = pipeline.bump_attempt_and_advance("U-budget", Stage::Review);
            pipeline.release("U-budget");
            assert_eq!(
                pipeline.state.units.get("U-budget").unwrap().attempt,
                expected_attempt
            );
            if expected_attempt < MAX_FIX_ATTEMPTS {
                assert!(matches!(out, AdvanceOutcome::Admitted { .. }));
            } else {
                // The bump to MAX_FIX_ATTEMPTS succeeds, but the
                // advance itself is Blocked by the budget gate.
                assert!(matches!(out, AdvanceOutcome::Blocked(_)));
            }
        }

        // Attempt is pinned at MAX — further bumps are refused
        // BEFORE the counter moves.
        let out = pipeline.bump_attempt_and_advance("U-budget", Stage::Review);
        assert!(matches!(out, AdvanceOutcome::Blocked(_)));
        assert_eq!(
            pipeline.state.units.get("U-budget").unwrap().attempt,
            MAX_FIX_ATTEMPTS,
            "attempt must never exceed MAX_FIX_ATTEMPTS"
        );

        // The budget gate applies to every stage, not just Review.
        // (Review → Verify is the only legal forward move left.)
        let out = pipeline.advance("U-budget", Stage::Verify);
        match out {
            AdvanceOutcome::Blocked(RuntimeJobError::Blocked { reason, .. }) => {
                assert!(reason.contains("fix attempts"));
            }
            other => panic!("expected budget Blocked on Verify, got {other:?}"),
        }
    }

    /// D16 four-pool semantics: `new` without an explicit fixer cap
    /// inherits the reviewer cap; `with_fixer` overrides it. The
    /// fixer pool is enforced by the spawn seam (the pipeline's
    /// `Stage` has no Fix variant), so this test pins the carried
    /// value, not a pipeline transition.
    #[test]
    fn fixer_pool_defaults_to_reviewer_and_overrides() {
        let default = DagPools::new(4, 2, 3, 2);
        assert_eq!(default.fixer_cap(), 3, "unset fixer inherits reviewer");
        let explicit = default.clone().with_fixer(1);
        assert_eq!(explicit.fixer_cap(), 1);
        // The three pipeline-stage caps are untouched by the fixer
        // override.
        assert_eq!(explicit.cap_for(Stage::Review), 3);
        assert_eq!(default, DagPools::new(4, 2, 3, 2).with_fixer(3));
    }

    // ---- U5 SHA validator (F10 fail-closed) ----

    /// U5/R5: 40-char SHA-1 and 64-char SHA-256 lowercase hex
    /// both accepted; uppercase / non-hex / wrong length rejected.
    /// The validator is the F10 fail-closed backbone: any field
    /// that could carry an OID must route through it.
    #[test]
    fn is_valid_sha_accepts_40_and_64_lowercase_hex() {
        assert!(is_valid_sha(&"a".repeat(40)));
        assert!(is_valid_sha(&"0".repeat(40)));
        assert!(is_valid_sha(&"f".repeat(40)));
        assert!(is_valid_sha(&"0123456789abcdef".repeat(4)));
        assert!(is_valid_sha(&"f".repeat(64)));
        assert!(is_valid_sha(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
    }

    #[test]
    fn is_valid_sha_rejects_invalid_lengths_and_chars() {
        assert!(!is_valid_sha(""));
        assert!(!is_valid_sha("a"));
        assert!(!is_valid_sha(&"a".repeat(39)));
        assert!(!is_valid_sha(&"a".repeat(41)));
        assert!(!is_valid_sha(&"a".repeat(63)));
        assert!(!is_valid_sha(&"a".repeat(65)));
        assert!(!is_valid_sha(&"A".repeat(40)), "uppercase rejected");
        assert!(!is_valid_sha(&"g".repeat(40)), "non-hex rejected");
        assert!(!is_valid_sha("0x1234"), "0x prefix rejected");
    }
}
