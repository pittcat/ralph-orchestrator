//! 2026-09-03-0959 plan U6 — DAG scheduler module root.
//!
//! This module is the **integration layer** that wires the
//! generic job kernel (`runtime_job`) into the per-Unit
//! pipeline. Files:
//!   - `jobs` — `JobPipeline::advance` drives one Unit through
//!     `Execute → Review → Verify` with token CAS, pool caps,
//!     and a typed three-fix-attempt budget.
//!   - `driver` — `DagSchedulerDriver::observe_accepted` is the
//!     hook the `EventLoop` calls when an accepted result lands;
//!     it routes the event into the right pipeline slot.
//!   - `shadow` — `ShadowSinkReader::read` is the read-only
//!     projection used by the inspect command (mirrors U5's
//!     `dag_shadow::ShadowSink`).
//!   - `worktree` (U7) — `UnitWorktree::acquire` binds each
//!     Unit's trusted worktree to a verified base commit.
//!     Reuses the existing worktree if its branch tip matches
//!     the verified base; rejects on host-dirty / host-untracked
//!     or base-mismatch.
//!   - `integration` (U7) — `IntegrationOrchestrator::integrate`
//!     runs the per-target CAS fast-forward pipeline: second
//!     changed-path check, lane lease acquire, targeted gate,
//!     CAS FF, idempotent integration record.
//!
//! U6 intentionally does NOT own the integration-half
//! authorisation gate (the changed-path check that runs again
//! before integrator's FF pass). That gate is U7's concern;
//! U6 computes the changed-path *result* and stores it on the
//! descriptor so U7 can authorise against the same value.
//!
//! Step 4 (2026-09-03-0959 DAG 接线) adds [`DagSchedulerRuntime`]
//! below: the seam the loop runner feeds accepted (post-policy)
//! events into. In `wave` / `dag_shadow` mode it stays
//! **observability-only** (no spawn, no worktree, no merge, no
//! emit). Step E1 adds the `dag`-mode execution face (`spawn`
//! module): per-unit executor → reviewer → verifier jobs fenced by
//! the durable launch journal, with results merged back into the
//! main ledger at most one business event per tick (OPAC).

pub mod driver;
mod integrate;
pub mod integration;
pub mod jobs;
pub mod recovery;
pub mod shadow;
mod spawn;
pub mod worktree;

pub use spawn::DagExecutionContext;

// Step 1+2(2026-09-03-0959 DAG 接线):driver/jobs 与 runtime_job
// 全量 promote 为生产可见;EventLoop 接线前尚无 bin 侧生产调用方,
// 各 item 以最小粒度 `#[allow(dead_code)]` 标注并指向
// `presets/en/parallel-forge-preset-author-notes.md`「promote 前置
// 义务清单」。本模块仍不做 `dag_scheduler::*` 平铺 re-export——
// 测试经 `super::*` / 完整模块路径访问,接线 Step 引入生产调用方时
// 再按需 re-export。

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use ralph_core::config::{ResolvedDagPools, SchedulerMode};
use ralph_core::parallel_forge_handoff::{ResourceCapacity, ResourceClaim};
use ralph_core::supervisor::dag_plan_receipt::{
    DagPlanReceipt, DagPlanReceiptRegistry, InMemoryDagPlanReceiptStore,
};
use ralph_core::supervisor::dag_scheduler::{AdmissionCaps, AdmissionSnapshot, UnitAdmissionInput};
use ralph_core::supervisor::dag_shadow::{ShadowSink, compute_shadow_observation};
use ralph_core::supervisor::dag_store::{CanonicalPlanRecord, DagSchedulerStore, DagStoreError};
use ralph_core::supervisor::dag_store_memory::InMemoryDagSchedulerStore;
use serde_json::Value;
use tracing::{debug, error, warn};

use self::driver::{DagSchedulerDriver, DriverOutcome, topics};
use self::jobs::{DagPools, JobPipeline};
use crate::loop_runner::runtime_job::Stage;

/// Topics (beyond the driver's per-unit family) the seam consumes.
mod seam_topics {
    pub const PLAN_READY: &str = "forge.plan.ready";
    pub const CONCURRENCY_APPROVED: &str = "forge.concurrency.approved";
    pub const CORRECTION_REQUESTED: &str = "forge.correction.requested";
}

/// Durable (or in-memory) store pair the seam writes through.
/// `dag` mode with the `supervisor-db` feature opens the rusqlite
/// pair over `<workspace>/.ralph/dag.db`; `dag_shadow` mode (or a
/// build without the feature) uses the in-memory variants — the
/// seam is observation-only, so a missing durable backend degrades
/// to a diagnostic log, never a loop failure.
struct StoreHandles {
    plans: Arc<dyn DagSchedulerStore>,
    receipts: DagPlanReceiptRegistry,
    /// E1: the durable launch journal (`reserve_job` /
    /// `record_job_pid` / `accept_job_terminal`) lives on the
    /// concrete rusqlite store, not on the `DagSchedulerStore`
    /// trait. Present only in `dag` mode with the `supervisor-db`
    /// feature; every other combination spawns nothing.
    #[cfg(feature = "supervisor-db")]
    journal: Option<Arc<ralph_core::supervisor::dag_store_rusqlite::RusqliteDagSchedulerStore>>,
}

/// In-memory topology mirror for one registered plan, derived from
/// the verified artifact at the accepted boundary (never from raw
/// payload claims). Drives the per-tick admission snapshot.
#[derive(Debug, Clone)]
struct PlanTopology {
    artifact_path: String,
    artifact_digest: String,
    target_branch: String,
    /// Trusted base commit captured at the `forge.concurrency.approved`
    /// boundary (workspace HEAD at approval time). Unit worktrees are
    /// acquired against this base. `None` until approval (or when the
    /// workspace git state is unresolvable — spawn then fails closed).
    verified_base_commit: Option<String>,
    units: Vec<UnitTopology>,
    resource_capacities: Vec<ResourceCapacity>,
    /// Units whose `forge.unit.integrated` event was accepted (the
    /// close-task projection ran) and whose durable integration
    /// record is acked. Fed into the admission snapshot so
    /// dependents unlock (E2); hydrated from the durable store at
    /// activation.
    integrated: std::collections::HashSet<String>,
}

#[derive(Debug, Clone)]
struct UnitTopology {
    unit_id: String,
    integration_order: u32,
    depends_on: Vec<String>,
    /// Targeted gate commands declared by the artifact's `tests`
    /// field (E1 verifier prompt / E2 integration gate). Empty when
    /// the unit declares none.
    tests: Vec<String>,
    resource_claims: Vec<ResourceClaim>,
}

/// Step 4 (2026-09-03-0959 DAG 接线): observation-only DAG
/// scheduler runtime seam; Step E1 adds the `dag`-mode execution
/// face (see `spawn`).
///
/// Wired into `loop_runner::inner` at the post-acceptance boundary:
/// every event that already passed origin → policy → schema →
/// contract validation is routed here BEFORE the activation-outcome
/// write. Behaviour:
///
///   - `forge.plan.ready` accepted → re-verify the artifact digest
///     through the existing `parallel_forge_handoff` /
///     `artifact_canonicalizer` path (no re-implementation), then
///     record a bounded durable receipt (plan key / artifact path /
///     digest / target identity) in `dag_plan_receipts`.
///   - `forge.concurrency.approved` accepted → activate the receipt
///     and idempotently `register_plan` + `activate_plan` in the DAG
///     store. A digest conflict fails closed into a diagnostic log —
///     never a panic, never a loop abort.
///   - `forge.unit.executed` / `forge.unit.reviewed` → forwarded to
///     [`DagSchedulerDriver::observe_accepted`] so the in-memory
///     `JobPipeline` mirrors unit progress.
///   - every other topic → untouched.
///   - after each batch: one observation tick — a pure
///     `compute_admissions` snapshot per tracked plan recorded into
///     the [`ShadowSink`]. In `dag` mode the durable decision record
///     is the store's plan/receipt rows plus the `dag_jobs` /
///     `dag_units` launch journal (`reserve_job` → `record_job_pid`
///     → `accept_job_terminal`); a separate per-tick decision journal
///     was evaluated and rejected (E1 decision) — the launch journal
///     already carries every admission that became a launch, and
///     recovery (`unresolved_jobs`) reads exactly that. The
///     [`ShadowSink`] observation stays in-memory diagnostics only.
///
/// **Lazy durability (TG-S05):** `.ralph/dag.db` is opened (and thus
/// created) only when the FIRST relevant accepted event arrives. A
/// run whose loop never sees a `forge.*` seam topic leaves no DAG
/// files behind, keeping the `dag ≡ wave` `.ralph` file-listing
/// equivalence pin green.
///
/// All error paths log through `tracing` and return; the seam must
/// never panic or block the wave path.
pub struct DagSchedulerRuntime {
    mode: SchedulerMode,
    pipeline: JobPipeline,
    workspace: PathBuf,
    db_path: PathBuf,
    stores: Option<StoreHandles>,
    sink: ShadowSink,
    plans: BTreeMap<String, PlanTopology>,
    /// E1 execution face, attached by `inner` once the global
    /// backend exists. `Some` only in `dag` mode.
    exec: Option<DagExecutionContext>,
    completion_tx: tokio::sync::mpsc::UnboundedSender<spawn::JobCompletion>,
    completion_rx: tokio::sync::mpsc::UnboundedReceiver<spawn::JobCompletion>,
    /// In-flight job ids (`JobIdentity.job_id`).
    active_jobs: HashSet<String>,
    /// Completed results awaiting merge into the main ledger.
    /// OPAC: at most one entry is merged per tick.
    merge_queue: VecDeque<spawn::PendingMerge>,
    /// Merged events whose journal terminal is written once the
    /// real `EventLoop` accepts them (keyed by pipeline unit key).
    awaiting_acceptance: HashMap<String, spawn::PostAcceptance>,
    /// Advances/spawns deferred by transient pool caps; retried
    /// each tick.
    pending_advances: VecDeque<spawn::PendingAdvance>,
    pending_spawns: VecDeque<spawn::PendingSpawn>,
    /// Verified units awaiting integration (E2); at most one is
    /// integrated per tick, in declared integration order.
    pending_integrations: Vec<integrate::PendingIntegration>,
    /// Units whose execute-stage job was already reserved (dedup
    /// against the pure admission snapshot, which re-reports Ready
    /// units every tick).
    executors_launched: HashSet<String>,
    /// D16: in-flight fixer jobs; the cap is enforced here because
    /// the pipeline `Stage` enum has no Fix variant.
    fixer_in_flight: u32,
    /// Plans whose durable state was ambiguous during restart recovery.
    /// A blocked plan is never admitted or allowed to emit completion.
    blocked_plans: HashSet<String>,
    /// Monotonic worker index for kernel log lines.
    job_seq: u32,
}

impl DagSchedulerRuntime {
    /// Build the seam. Pure bookkeeping — no I/O, no store open.
    pub fn new(mode: SchedulerMode, resolved: ResolvedDagPools, workspace: PathBuf) -> Self {
        let pools = DagPools::new(
            resolved.global,
            resolved.executor,
            resolved.reviewer,
            resolved.verifier,
        )
        // D16 four-pool semantics: the fixer cap comes from the
        // supervisor config (`dag_pools.fixer`) and is enforced by
        // the spawn seam, not by `JobPipeline::advance` (the fix
        // loop reuses the Review-stage slot).
        .with_fixer(resolved.fixer);
        let db_path = integration::dag_store_path(&workspace);
        let (completion_tx, completion_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            mode,
            pipeline: JobPipeline::new(pools),
            workspace,
            db_path,
            stores: None,
            sink: ShadowSink::new(),
            plans: BTreeMap::new(),
            exec: None,
            completion_tx,
            completion_rx,
            active_jobs: HashSet::new(),
            merge_queue: VecDeque::new(),
            awaiting_acceptance: HashMap::new(),
            pending_advances: VecDeque::new(),
            pending_spawns: VecDeque::new(),
            pending_integrations: Vec::new(),
            executors_launched: HashSet::new(),
            fixer_in_flight: 0,
            blocked_plans: HashSet::new(),
            job_seq: 0,
        }
    }

    /// Attach the E1 execution context. No-op outside `dag` mode:
    /// wave / `dag_shadow` keep the seam observability-only.
    pub fn attach_execution_context(&mut self, ctx: DagExecutionContext) {
        if self.mode == SchedulerMode::Dag {
            self.exec = Some(ctx);
        }
    }

    /// Rebuild the process-local DAG view from the durable store after a
    /// restart. Ambiguous launches are blocked; they are never respawned.
    /// Result files are fed through the ordinary completion drain so event
    /// validation and journal fencing remain identical to a live worker.
    #[cfg(feature = "supervisor-db")]
    pub(crate) fn recover_after_restart(&mut self) {
        use ralph_core::supervisor::dag_integration::IntegrationStore as _;

        if self.mode != SchedulerMode::Dag || self.exec.is_none() || !self.db_path.exists() {
            return;
        }
        let active = match self
            .ensure_stores()
            .and_then(|s| s.plans.list_active_plans())
        {
            Ok(plans) => plans,
            Err(err) => {
                warn!(error = %err, "DAG recovery: active-plan scan failed");
                return;
            }
        };
        for registration in active {
            let plan_key = registration.plan_key.clone();
            let receipt = match self
                .stores
                .as_ref()
                .and_then(|stores| stores.receipts.get(&plan_key).ok().flatten())
            {
                Some(receipt) => receipt,
                None => {
                    self.block_plan(&plan_key, "durable plan receipt is missing");
                    continue;
                }
            };
            if !self.plans.contains_key(&plan_key)
                && !self.rebuild_topology(&plan_key, &receipt.artifact_path)
            {
                self.block_plan(&plan_key, "execution-plan artifact could not be rebuilt");
                continue;
            }
            if let Some(plan) = self.plans.get_mut(&plan_key) {
                plan.target_branch = registration.target_branch.clone();
            }
            let Some(journal) = self.journal() else {
                self.block_plan(&plan_key, "durable DAG journal is unavailable");
                continue;
            };
            let Some(base) = journal.verified_base(&plan_key).ok().flatten() else {
                self.block_plan(&plan_key, "approval-time verified base is missing");
                continue;
            };
            if let Some(plan) = self.plans.get_mut(&plan_key) {
                plan.verified_base_commit = Some(base);
            }
            let integration_store = journal.shared_with_integration();
            let unit_ids = self
                .plans
                .get(&plan_key)
                .map(|plan| {
                    plan.units
                        .iter()
                        .map(|unit| unit.unit_id.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for unit_id in &unit_ids {
                if let Ok(records) =
                    ralph_core::supervisor::dag_integration::IntegrationStore::list_for_unit(
                        &integration_store,
                        unit_id,
                    )
                    && records.iter().any(|record| {
                        record.target_branch == registration.target_branch && record.acked
                    })
                    && let Some(plan) = self.plans.get_mut(&plan_key)
                {
                    plan.integrated.insert(unit_id.clone());
                }
            }
            let jobs = match journal.list_jobs(&plan_key) {
                Ok(jobs) => jobs,
                Err(err) => {
                    self.block_plan(&plan_key, "DAG job journal could not be read");
                    warn!(plan_key, error = %err, "DAG recovery: job scan failed");
                    continue;
                }
            };
            // A verify terminal may have been persisted immediately before
            // the integration enqueue. Re-drive that seam from durable facts
            // instead of waiting for a historical event that resume does not
            // replay.
            for unit_id in &unit_ids {
                let verified = jobs.iter().rev().any(|job| {
                    job.identity.unit_id == *unit_id
                        && job.identity.stage == "verify"
                        && job.terminal.as_deref() == Some("accepted")
                });
                let has_record =
                    integration_store
                        .list_for_unit(unit_id)
                        .ok()
                        .is_some_and(|records| {
                            records
                                .iter()
                                .any(|record| record.target_branch == registration.target_branch)
                        });
                if verified && !has_record {
                    self.queue_integration(&plan_key, unit_id);
                }
            }
            for job in jobs {
                let Some(stage) = recovery::stage_from_str(&job.identity.stage) else {
                    self.block_plan(&plan_key, "DAG journal contains an unknown stage");
                    continue;
                };
                let unit_key = job.identity.unit_key();
                self.pipeline.restore_unit(
                    unit_key.clone(),
                    job.identity.job_id.clone(),
                    job.identity.hat.clone(),
                    stage,
                    u64::from(job.identity.attempt),
                );
                if job.identity.stage == "execute" {
                    self.executors_launched.insert(job.identity.unit_id.clone());
                }
                if job.terminal.is_some() {
                    continue;
                }
                let events_file = self
                    .workspace
                    .join(".ralph")
                    .join("dag")
                    .join(&plan_key)
                    .join(&job.identity.unit_id)
                    .join(format!("{}.events.jsonl", job.identity.job_id));
                if events_file.exists()
                    && !crate::loop_runner::wave::io::read_worker_events(&events_file).is_empty()
                {
                    self.active_jobs.insert(job.identity.job_id.clone());
                    let _ = self
                        .completion_tx
                        .send(spawn::JobCompletion::recovered(job.identity, events_file));
                } else {
                    let Some(pid) = job.pid else {
                        // NULL PID is the spawn-before-handshake window: an
                        // orphan may exist, so do not guess or relaunch.
                        self.block_plan(
                            &plan_key,
                            "unresolved DAG launch has no trustworthy PID or result",
                        );
                        continue;
                    };
                    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None) {
                        Ok(()) | Err(nix::errno::Errno::EPERM) => {
                            let timeout = self
                                .exec
                                .as_ref()
                                .map(|exec| exec.timeout_for_hat(&job.identity.hat))
                                .unwrap_or_else(|| std::time::Duration::from_secs(3600));
                            if !self.adopt_recovered_job(
                                job.identity.clone(),
                                pid,
                                events_file,
                                timeout,
                            ) {
                                self.block_plan(
                                    &plan_key,
                                    "recovered live job could not be adopted",
                                );
                            }
                        }
                        Err(nix::errno::Errno::ESRCH) => {
                            self.settle_recovered_dead_job(&job.identity);
                        }
                        Err(_) => {
                            self.block_plan(
                                &plan_key,
                                "recovered job liveness probe was inconclusive",
                            );
                        }
                    }
                }
            }
        }
        self.reconcile_after_restart();
        self.write_recovery_report();
    }

    #[cfg(not(feature = "supervisor-db"))]
    pub(crate) fn recover_after_restart(&mut self) {}

    /// Route one iteration's accepted events, then run one
    /// observation tick. Called once per loop iteration from
    /// `loop_runner::inner` after acceptance; never fails.
    pub fn observe_accepted_events(&mut self, events: &[ralph_proto::Event]) {
        for event in events {
            self.route_event(event.topic.as_str(), &event.payload);
        }
        self.tick();
    }

    fn route_event(&mut self, topic: &str, payload_raw: &str) {
        match topic {
            seam_topics::PLAN_READY | seam_topics::CONCURRENCY_APPROVED => {
                let payload: Value = match serde_json::from_str(payload_raw) {
                    Ok(value) => value,
                    Err(err) => {
                        warn!(
                            topic,
                            error = %err,
                            "DAG seam: accepted event payload is not valid JSON; skipping"
                        );
                        return;
                    }
                };
                if topic == seam_topics::PLAN_READY {
                    self.on_plan_ready(&payload);
                } else {
                    self.on_concurrency_approved(&payload);
                }
            }
            topics::UNIT_EXECUTED | topics::UNIT_REVIEWED => {
                let payload: Value = match serde_json::from_str(payload_raw) {
                    Ok(value) => value,
                    Err(err) => {
                        warn!(
                            topic,
                            error = %err,
                            "DAG seam: accepted unit event payload is not valid JSON; skipping"
                        );
                        return;
                    }
                };
                self.observe_unit_event(topic, &payload);
            }
            seam_topics::CORRECTION_REQUESTED if self.mode == SchedulerMode::Dag => {
                match serde_json::from_str::<Value>(payload_raw) {
                    Ok(payload) => self.on_correction_requested(&payload),
                    Err(err) => warn!(
                        topic,
                        error = %err,
                        "DAG seam: correction payload is not valid JSON; skipping"
                    ),
                }
            }
            // E1: verify-stage results only drive the execution face
            // (journal terminal + slot release); the pipeline driver
            // intentionally does not consume them.
            spawn::topics_ext::UNIT_VERIFIED | spawn::topics_ext::UNIT_VERIFICATION_FAILED
                if self.mode == SchedulerMode::Dag =>
            {
                let payload: Value = match serde_json::from_str(payload_raw) {
                    Ok(value) => value,
                    Err(err) => {
                        warn!(
                            topic,
                            error = %err,
                            "DAG seam: accepted verify event payload is not valid JSON; skipping"
                        );
                        return;
                    }
                };
                self.observe_unit_event(topic, &payload);
            }
            // E2: the runtime-emitted integrated event came back
            // through real acceptance (the close-task projection ran)
            // — ack the durable record and unlock dependents.
            integrate::UNIT_INTEGRATED if self.mode == SchedulerMode::Dag => {
                match serde_json::from_str::<Value>(payload_raw) {
                    Ok(payload) => self.on_unit_integrated_accepted(&payload),
                    Err(err) => {
                        warn!(
                            topic,
                            error = %err,
                            "DAG seam: accepted integrated event payload is not valid JSON; skipping ack"
                        );
                    }
                }
            }
            _ => {}
        }
    }

    /// Accepted failure-handler correction: advance every affected Unit to a
    /// fresh fixer attempt. The runtime owns the attempt budget and stage
    /// admission; the fixer hat only supplies the repair decision and result.
    fn on_correction_requested(&mut self, payload: &Value) {
        let plan_key = payload
            .get("plan_key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let feedback = payload
            .get("failure_observation_path")
            .or_else(|| payload.get("correction_request_path"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(unit_ids) = payload.get("affected_unit_ids").and_then(Value::as_array) else {
            warn!(plan_key, "DAG correction request has no affected_unit_ids");
            return;
        };
        for unit_id in unit_ids.iter().filter_map(Value::as_str) {
            let outcome = self
                .pipeline
                .bump_attempt_and_advance(unit_id, Stage::Review);
            if let self::jobs::AdvanceOutcome::Admitted { token } = outcome {
                self.queue_spawn(spawn::PendingSpawn {
                    unit_key: unit_id.to_string(),
                    kind: spawn::SpawnKind::Fix,
                    attempt: u32::try_from(token.attempt()).unwrap_or(u32::MAX),
                    plan_key: plan_key.clone(),
                    feedback: feedback.clone(),
                });
            } else {
                debug!(
                    unit_id,
                    ?outcome,
                    "DAG correction admission deferred or blocked"
                );
            }
        }
    }

    /// Lazily open the store pair on first use. `dag` mode with the
    /// `supervisor-db` feature opens (creating if needed) the
    /// durable rusqlite stores; every other combination falls back
    /// to the in-memory variants with a diagnostic.
    fn ensure_stores(&mut self) -> Result<&StoreHandles, DagStoreError> {
        if self.stores.is_none() {
            let handles = self.open_stores()?;
            self.stores = Some(handles);
        }
        Ok(self.stores.as_ref().expect("stores just initialised"))
    }

    #[cfg(feature = "supervisor-db")]
    fn open_stores(&self) -> Result<StoreHandles, DagStoreError> {
        if self.mode.uses_legacy_authority() || self.mode == SchedulerMode::DagShadow {
            return Ok(Self::in_memory_stores());
        }
        if let Some(parent) = self.db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                DagStoreError::IoError(format!("create {}: {err}", parent.display()))
            })?;
        }
        let store = ralph_core::supervisor::dag_store_rusqlite::RusqliteDagSchedulerStore::open(
            &self.db_path,
        )?;
        let receipts = DagPlanReceiptRegistry::new(Arc::new(store.shared_with_receipts()));
        debug!(mode = self.mode.as_str(), "DAG seam: durable store opened");
        let store = Arc::new(store);
        Ok(StoreHandles {
            plans: store.clone(),
            receipts,
            journal: Some(store),
        })
    }

    #[cfg(not(feature = "supervisor-db"))]
    fn open_stores(&self) -> Result<StoreHandles, DagStoreError> {
        if self.mode == SchedulerMode::Dag {
            warn!(
                "DAG seam: supervisor-db feature is off; \
                 observations are process-local only"
            );
        }
        Ok(Self::in_memory_stores())
    }

    fn in_memory_stores() -> StoreHandles {
        StoreHandles {
            plans: Arc::new(InMemoryDagSchedulerStore::new()),
            receipts: DagPlanReceiptRegistry::new(Arc::new(InMemoryDagPlanReceiptStore::new())),
            #[cfg(feature = "supervisor-db")]
            journal: None,
        }
    }

    /// Accepted `forge.plan.ready`: verify the artifact, record the
    /// bounded receipt, mirror the unit topology in memory.
    fn on_plan_ready(&mut self, payload: &Value) {
        let handoff = match ralph_core::parallel_forge_handoff::load_plan_handoff(
            payload,
            &self.workspace,
        ) {
            Ok(handoff) => handoff,
            Err(err) => {
                warn!(
                    error = %err,
                    "DAG seam: forge.plan.ready artifact verification failed; receipt not recorded"
                );
                return;
            }
        };
        let artifact_path = payload
            .get("execution_plan_path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let target_branch = ralph_core::get_current_branch(&self.workspace).unwrap_or_else(|err| {
            debug!(
                error = %err,
                "DAG seam: current branch unresolved; recording target identity as 'unknown'"
            );
            "unknown".to_string()
        });
        let receipt = DagPlanReceipt::new(
            handoff.plan_key.clone(),
            artifact_path.clone(),
            handoff.artifact.digest.clone(),
            target_branch.clone(),
            now_ms(),
        );
        let recorded = {
            let stores = match self.ensure_stores() {
                Ok(stores) => stores,
                Err(err) => {
                    warn!(
                        error = %err,
                        "DAG seam: store open failed; receipt not recorded"
                    );
                    return;
                }
            };
            stores.receipts.record(receipt)
        };
        match recorded {
            Ok(true) => debug!(plan_key = %handoff.plan_key, "DAG seam: receipt recorded"),
            Ok(false) => debug!(plan_key = %handoff.plan_key, "DAG seam: receipt replay (no-op)"),
            Err(err) => {
                // Fail-closed: a digest conflict on an already-recorded
                // plan key is a diagnostic, never a panic and never an
                // overwrite.
                error!(
                    plan_key = %handoff.plan_key,
                    error = %err,
                    "DAG seam: receipt record rejected (fail-closed); keeping original registration"
                );
                return;
            }
        }
        self.plans.insert(
            handoff.plan_key.clone(),
            topology_from_handoff(&handoff, artifact_path, target_branch),
        );
    }

    /// Accepted `forge.concurrency.approved`: activate the receipt,
    /// register + activate the plan in the store, and seed the
    /// in-memory pipeline slots. `approved != true` (guardian
    /// declined) is a no-op — the preset routes that to
    /// `forge.plan.blocked` instead.
    fn on_concurrency_approved(&mut self, payload: &Value) {
        let approved = payload
            .get("approved")
            .map(|v| v.as_bool().unwrap_or_else(|| v.as_str() == Some("true")))
            .unwrap_or(false);
        if !approved {
            debug!("DAG seam: concurrency approval is not affirmative; skipping activation");
            return;
        }
        let artifact_path = match payload
            .get("execution_plan_path")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            Some(path) => path.to_string(),
            None => {
                warn!("DAG seam: forge.concurrency.approved missing execution_plan_path; skipping");
                return;
            }
        };
        // Correlate the approval back to the plan recorded at the
        // `forge.plan.ready` boundary. The in-memory topology is the
        // fast path; after a restart the durable receipt (keyed by
        // artifact path) still identifies the plan.
        let plan_key = match self.plan_key_for_artifact(&artifact_path) {
            Some(key) => key,
            None => {
                warn!(
                    "DAG seam: approval references an unrecorded execution plan; skipping activation"
                );
                return;
            }
        };
        if !self.plans.contains_key(&plan_key) && !self.rebuild_topology(&plan_key, &artifact_path)
        {
            return;
        }
        let topology = self.plans.get(&plan_key).expect("topology present");
        let record = CanonicalPlanRecord {
            plan_key: plan_key.clone(),
            artifact_digest: topology.artifact_digest.clone(),
            target_branch: topology.target_branch.clone(),
            unit_ids: topology.units.iter().map(|u| u.unit_id.clone()).collect(),
            created_at_ms: now_ms(),
        };
        let target_branch = topology.target_branch.clone();
        let outcome = {
            let stores = match self.ensure_stores() {
                Ok(stores) => stores,
                Err(err) => {
                    warn!(
                        error = %err,
                        "DAG seam: store open failed; plan not activated"
                    );
                    return;
                }
            };
            stores
                .receipts
                .activate(&plan_key, now_ms())
                .and_then(|_| stores.plans.register_plan(&record))
                .and_then(|_| stores.plans.activate_plan(&plan_key, &target_branch))
        };
        match outcome {
            Ok(()) => {
                debug!(plan_key, "DAG seam: plan registered + activated");
            }
            Err(err) => {
                error!(
                    plan_key,
                    error = %err,
                    "DAG seam: plan activation rejected (fail-closed); wave path unaffected"
                );
                return;
            }
        }
        // E1: pin the trusted base commit for unit worktrees at the
        // approval boundary. An unresolvable git state stays `None`
        // and the spawn seam fails closed on it.
        let verified_base = ralph_core::get_head_sha(&self.workspace).ok();
        if let Some(plan) = self.plans.get_mut(&plan_key) {
            plan.verified_base_commit = verified_base.clone();
        }
        #[cfg(feature = "supervisor-db")]
        if let Some(base) = verified_base.as_deref()
            && let Some(journal) = self.journal()
            && let Err(err) = journal.record_verified_base(&plan_key, base, now_ms() as i64)
        {
            self.block_plan(&plan_key, "verified base could not be durably pinned");
            warn!(plan_key, error = %err, "DAG approval: verified base persistence failed");
        }
        // Seed the in-memory pipeline so per-unit events route. The
        // job id is a deterministic observation label — Step 4 never
        // launches a process under it.
        let unit_ids: Vec<String> = self
            .plans
            .get(&plan_key)
            .expect("topology present")
            .units
            .iter()
            .map(|u| u.unit_id.clone())
            .collect();
        for unit_id in unit_ids {
            let job_id = format!("job-{plan_key}-{unit_id}");
            self.pipeline
                .ensure_unit(unit_id, job_id, "executor", Stage::Execute);
        }
        // E2: hydrate the integrated set from the durable store so a
        // restarted loop does not re-admit dependents of units that
        // already integrated before the restart.
        #[cfg(feature = "supervisor-db")]
        {
            use ralph_core::supervisor::dag_integration::IntegrationStore as _;
            let acked: Vec<String> = match self.journal() {
                Some(journal) => {
                    let store = journal.shared_with_integration();
                    self.plans
                        .get(&plan_key)
                        .map(|plan| {
                            plan.units
                                .iter()
                                .filter(|u| {
                                    store
                                        .list_for_unit(&u.unit_id)
                                        .map(|records| records.iter().any(|r| r.acked))
                                        .unwrap_or(false)
                                })
                                .map(|u| u.unit_id.clone())
                                .collect()
                        })
                        .unwrap_or_default()
                }
                None => Vec::new(),
            };
            if let Some(plan) = self.plans.get_mut(&plan_key) {
                plan.integrated.extend(acked);
            }
        }
    }

    /// Look up the plan key for an artifact path: in-memory topology
    /// first, durable receipts second (restart path).
    fn plan_key_for_artifact(&mut self, artifact_path: &str) -> Option<String> {
        if let Some((key, _)) = self
            .plans
            .iter()
            .find(|(_, t)| t.artifact_path == artifact_path)
        {
            return Some(key.clone());
        }
        // Laziness guard: in durable mode an orphan approval (no prior
        // accepted `forge.plan.ready`) must NOT create the db file just
        // to discover there is nothing to correlate.
        if self.stores.is_none() && self.mode == SchedulerMode::Dag && !self.db_path.exists() {
            debug!("DAG seam: approval for an unrecorded plan and no DAG store on disk; skipping");
            return None;
        }
        let stores = match self.ensure_stores() {
            Ok(stores) => stores,
            Err(err) => {
                warn!(error = %err, "DAG seam: store open failed during approval correlation");
                return None;
            }
        };
        match stores.receipts.list_all() {
            Ok(receipts) => receipts
                .into_iter()
                .find(|r| r.artifact_path == artifact_path)
                .map(|r| r.plan_key),
            Err(err) => {
                warn!(error = %err, "DAG seam: receipt listing failed during approval correlation");
                None
            }
        }
    }

    /// Rebuild the in-memory topology from the verified artifact
    /// after a restart (receipt durable, topology process-local).
    /// Returns `false` when verification fails.
    fn rebuild_topology(&mut self, plan_key: &str, artifact_path: &str) -> bool {
        let mut payload = serde_json::json!({
            "execution_plan_path": artifact_path,
            "plan_key": plan_key,
        });
        if let Some(digest) = self
            .stores
            .as_ref()
            .and_then(|s| s.receipts.cached(plan_key))
            .map(|r| r.artifact_digest)
        {
            payload["plan_digest"] = Value::String(digest);
        }
        let handoff = match ralph_core::parallel_forge_handoff::load_plan_handoff(
            &payload,
            &self.workspace,
        ) {
            Ok(handoff) => handoff,
            Err(err) => {
                warn!(
                    plan_key,
                    error = %err,
                    "DAG seam: topology rebuild failed; approval not applied"
                );
                return false;
            }
        };
        let target_branch = ralph_core::get_current_branch(&self.workspace)
            .unwrap_or_else(|_| "unknown".to_string());
        self.plans.insert(
            plan_key.to_string(),
            topology_from_handoff(&handoff, artifact_path.to_string(), target_branch),
        );
        true
    }

    /// Forward a per-unit accepted event into the pipeline driver.
    /// `unit_key` comes from the payload's `unit_id` field (the
    /// schema-declared unit identity; `unit_key` accepted as a
    /// fallback alias).
    fn observe_unit_event(&mut self, topic: &str, payload: &Value) {
        let unit_key = payload
            .get("unit_id")
            .or_else(|| payload.get("unit_key"))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty());
        let Some(unit_key) = unit_key else {
            debug!(topic, "DAG seam: unit event without unit_id; ignored");
            return;
        };
        // E1: in `dag` mode the execution face owns the follow-up —
        // journal terminal for the completing job, slot release, and
        // the next-stage spawn. Shadow mode keeps the pure
        // observation path below untouched.
        if self.mode == SchedulerMode::Dag {
            self.observe_unit_event_dag(topic, unit_key, payload);
            return;
        }
        let outcome = {
            let mut driver = DagSchedulerDriver::new(&mut self.pipeline);
            driver.observe_accepted(topic, unit_key, payload)
        };
        match outcome {
            DriverOutcome::Routed {
                unit_key,
                next_stage,
                ..
            } => {
                debug!(unit_key, ?next_stage, "DAG seam: unit event routed");
            }
            DriverOutcome::Blocked { unit_key, error } => {
                warn!(
                    unit_key,
                    error = %error,
                    "DAG seam: pipeline blocked the unit event"
                );
            }
            DriverOutcome::StillExecuting { unit_key, stage } => {
                debug!(unit_key, ?stage, "DAG seam: unit still executing");
            }
            DriverOutcome::Ignored { topic } => {
                debug!(topic, "DAG seam: unit event ignored by driver");
            }
        }
    }

    /// One observation tick: for every tracked plan, compute the
    /// pure admission decision set and record it into the shadow
    /// sink. Zero execution side effects — no spawn, no worktree,
    /// no merge, no emit, no task close.
    fn tick(&mut self) {
        if self.plans.is_empty() {
            return;
        }
        // The integration target head is the loop workspace's current
        // tip: parallel-forge integrates onto the branch the loop
        // checked out. Unresolvable git state degrades to `None`,
        // which the admission engine reports as `BlockedNoTargetHead`.
        let target_head = ralph_core::get_head_sha(&self.workspace).ok();
        let mut admitted: Vec<(String, String)> = Vec::new();
        for (plan_key, plan) in &self.plans {
            if self.blocked_plans.contains(plan_key) {
                continue;
            }
            let caps = AdmissionCaps {
                global_cap: self.pipeline.pools().global,
                executor_pool_cap: self.pipeline.pools().executor,
                // Resource capacities come from the verified artifact
                // handoff, never from the event payload.
                resource_capacities: plan.resource_capacities.clone(),
            };
            let inputs: Vec<UnitAdmissionInput> = plan
                .units
                .iter()
                .map(|u| UnitAdmissionInput {
                    unit_id: u.unit_id.clone(),
                    integration_order: u.integration_order,
                    depends_on: u.depends_on.clone(),
                    // E2: units whose integrated event was accepted
                    // (projection acknowledged) unlock their
                    // dependents.
                    integrated_units: plan.integrated.clone(),
                    resource_claims: u.resource_claims.clone(),
                })
                .collect();
            let snapshot = AdmissionSnapshot {
                units: &inputs,
                integration_target_head: target_head.as_deref(),
            };
            let mut observation = compute_shadow_observation(&snapshot, &caps, &self.sink);
            observation.plan_key = plan_key.clone();
            if self.mode == SchedulerMode::Dag {
                admitted.extend(
                    observation
                        .decisions
                        .iter()
                        .filter(|(_, reason)| reason == "Admitted")
                        .map(|(unit_id, _)| (plan_key.clone(), unit_id.clone())),
                );
            }
            self.sink.record(observation);
        }
        // E1 execution passes (dag mode only): retry deferred
        // follow-ups, then admit Ready units into Execute. Both are
        // fail-closed — a spawn error strands the unit with a
        // journaled `failed` terminal, never a panic.
        if self.mode == SchedulerMode::Dag && self.exec.is_some() {
            self.retry_pending();
            self.maybe_integrate_one();
            for (plan_key, unit_id) in admitted {
                self.maybe_spawn_executor(&plan_key, &unit_id);
            }
            self.maybe_emit_development_done();
        }
    }

    /// Mark a plan blocked after recovery finds an ambiguous launch or
    /// projection state. This is intentionally monotonic for the lifetime of
    /// the runtime; an operator must resolve the durable evidence before a
    /// fresh loop can safely retry it.
    pub(crate) fn block_plan(&mut self, plan_key: impl Into<String>, reason: &str) {
        let plan_key = plan_key.into();
        if self.blocked_plans.insert(plan_key.clone()) {
            error!(plan_key, reason, "DAG recovery blocked plan");
        }
    }

    #[cfg(feature = "supervisor-db")]
    fn write_recovery_report(&self) {
        let Some(loop_id) = self.exec.as_ref().map(|exec| exec.loop_id.as_str()) else {
            return;
        };
        let diagnostics = self.workspace.join(".ralph").join("diagnostics");
        if let Err(err) = std::fs::create_dir_all(&diagnostics) {
            warn!(error = %err, "DAG recovery: diagnostics directory unavailable");
            return;
        }
        let report = serde_json::json!({
            "loop_id": loop_id,
            "recovered_at_ms": now_ms(),
            "blocked_plans": self.blocked_plans.iter().collect::<Vec<_>>(),
            "recovered_plan_count": self.plans.len(),
        });
        let path = diagnostics.join(format!("dag-recovery-{loop_id}.json"));
        if let Err(err) = std::fs::write(&path, format!("{}\n", report)) {
            warn!(path = %path.display(), error = %err, "DAG recovery: report write failed");
        }
    }

    /// Test-only introspection: current pipeline stage of a unit.
    #[cfg(test)]
    fn pipeline_stage(&self, unit_key: &str) -> Option<Stage> {
        self.pipeline.stage_of(unit_key)
    }

    /// Test-only introspection: receipt snapshot for a plan key.
    #[cfg(test)]
    fn receipt(&self, plan_key: &str) -> Option<DagPlanReceipt> {
        self.stores.as_ref()?.receipts.cached(plan_key)
    }

    /// Test-only introspection: number of recorded observations.
    #[cfg(test)]
    fn observation_count(&self) -> usize {
        self.sink.observation_count()
    }
}

/// Derive the in-memory topology mirror from a verified handoff.
/// `depends_on` task keys carry the `forge:<plan_key>:<unit_id>`
/// registration form; the mirror stores bare unit ids.
fn topology_from_handoff(
    handoff: &ralph_core::parallel_forge_handoff::CanonicalPlanHandoff,
    artifact_path: String,
    target_branch: String,
) -> PlanTopology {
    let prefix = format!("forge:{}:", handoff.plan_key);
    PlanTopology {
        artifact_path,
        artifact_digest: handoff.artifact.digest.clone(),
        target_branch,
        verified_base_commit: None,
        integrated: std::collections::HashSet::new(),
        resource_capacities: handoff.resource_capacities.clone(),
        units: handoff
            .tasks
            .iter()
            .map(|task| UnitTopology {
                unit_id: task.unit_id.clone(),
                integration_order: task.integration_order,
                depends_on: task
                    .depends_on_task_keys
                    .iter()
                    .map(|key| key.strip_prefix(&prefix).unwrap_or(key).to_string())
                    .collect(),
                tests: handoff
                    .unit_tests
                    .get(&task.unit_id)
                    .cloned()
                    .unwrap_or_default(),
                resource_claims: handoff
                    .unit_resource_claims
                    .get(&task.unit_id)
                    .cloned()
                    .unwrap_or_default(),
            })
            .collect(),
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Step 4 tests — the facade IS the production code path inner.rs calls, so
// these exercise the seam end-to-end minus the outer loop.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_adapters::CliBackend;
    use ralph_core::config::{CliConfig, HatConfig, RalphConfig};
    use ralph_core::parallel_forge_handoff::load_plan_handoff;
    use ralph_core::supervisor::dag_store_rusqlite::jobs::JobIdentity;
    use tempfile::TempDir;

    /// Two independent units in one wave (the handoff contract
    /// requires a parallel wave) plus one dependent unit.
    const PLAN_ARTIFACT: &str = r#"version: 1
plan_key: pf-test
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1-foundation
    tests:
      - cargo nextest run -p ralph-core -- u1
  - id: U2
    title: Feature
    depends_on: []
    execution_wave: 1
    integration_order: 2
    target_branch: feat/u2-feature
  - id: U3
    title: Dependent
    depends_on: [U1]
    execution_wave: 2
    integration_order: 3
    target_branch: feat/u3-dependent
"#;

    const ARTIFACT_REL: &str = ".ralph/forge/pf-test/execution-plan.yml";

    const RESOURCE_ARTIFACT: &str = r#"version: 2
plan_key: pf-test
resource_capacities:
  - key: gpu
    capacity: 1
units:
  - id: U1
    title: GPU unit one
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1
    resource_claims:
      - key: gpu
        permits: 1
  - id: U2
    title: GPU unit two
    depends_on: []
    execution_wave: 1
    integration_order: 2
    target_branch: feat/u2
    resource_claims:
      - key: gpu
        permits: 1
"#;

    fn resolved_pools() -> ResolvedDagPools {
        ResolvedDagPools {
            global: 4,
            executor: 2,
            reviewer: 2,
            verifier: 2,
            fixer: 2,
        }
    }

    fn fixture(mode: SchedulerMode) -> (TempDir, DagSchedulerRuntime) {
        let tmp = TempDir::new().expect("temp workspace");
        let artifact = tmp.path().join(ARTIFACT_REL);
        std::fs::create_dir_all(artifact.parent().expect("parent")).expect("mkdir");
        std::fs::write(&artifact, PLAN_ARTIFACT).expect("write artifact");
        let runtime = DagSchedulerRuntime::new(mode, resolved_pools(), tmp.path().to_path_buf());
        (tmp, runtime)
    }

    fn plan_ready_event(workspace: &std::path::Path) -> ralph_proto::Event {
        let bytes = std::fs::read(workspace.join(ARTIFACT_REL)).expect("read artifact");
        let digest = ralph_core::artifact_canonicalizer::canonicalize(&bytes)
            .expect("canonicalize")
            .digest;
        let payload = serde_json::json!({
            "plan_key": "pf-test",
            "execution_plan_path": ARTIFACT_REL,
            "plan_digest": digest,
        });
        ralph_proto::Event::new(seam_topics::PLAN_READY, payload.to_string())
    }

    fn init_git_head(workspace: &std::path::Path) {
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "dag-test@example.invalid"][..],
            &["config", "user.name", "DAG Test"][..],
            &["add", "."][..],
            &["commit", "-qm", "fixture"][..],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(workspace)
                .status()
                .expect("run git fixture command");
            assert!(status.success(), "git command failed: git {args:?}");
        }
    }

    fn approved_event() -> ralph_proto::Event {
        let payload = serde_json::json!({
            "execution_plan_path": ARTIFACT_REL,
            "approval_report_path": ".ralph/forge/pf-test/concurrency-approval.md",
            "approved": true,
        });
        ralph_proto::Event::new(seam_topics::CONCURRENCY_APPROVED, payload.to_string())
    }

    #[cfg(feature = "supervisor-db")]
    fn execution_context(workspace: &std::path::Path) -> DagExecutionContext {
        let mut config = RalphConfig::default();
        for hat in ["executor", "reviewer", "verifier"] {
            config.hats.insert(
                hat.to_string(),
                HatConfig {
                    name: hat.to_string(),
                    instructions: format!("{hat} instructions"),
                    ..HatConfig::default()
                },
            );
        }
        let backend = CliBackend::from_config(&CliConfig::default()).expect("backend builds");
        DagExecutionContext::new(
            &config,
            &backend,
            "loop-test",
            workspace.join("events.jsonl"),
            None,
        )
    }

    #[cfg(feature = "supervisor-db")]
    fn execute_identity() -> JobIdentity {
        JobIdentity {
            plan_key: "pf-test".to_string(),
            unit_id: "U1".to_string(),
            job_id: "job-execute-0".to_string(),
            hat: "executor".to_string(),
            stage: "execute".to_string(),
            attempt: 0,
            token: "token-execute-0".to_string(),
        }
    }

    #[cfg(feature = "supervisor-db")]
    fn reopen_receipts(workspace: &std::path::Path) -> Vec<DagPlanReceipt> {
        let registry = DagPlanReceiptRegistry::open(workspace.join(".ralph/dag.db"))
            .expect("reopen receipt registry");
        let mut receipts = registry.list_all().expect("list receipts");
        receipts.sort_by(|a, b| a.plan_key.cmp(&b.plan_key));
        receipts
    }

    /// Accepted `forge.plan.ready` → bounded durable receipt lands in
    /// `.ralph/dag.db` with the re-verified canonical digest, and one
    /// observation tick is recorded.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn plan_ready_records_durable_receipt() {
        let (tmp, mut runtime) = fixture(SchedulerMode::Dag);
        let event = plan_ready_event(tmp.path());
        let expected_digest: String = serde_json::from_str::<Value>(&event.payload)
            .expect("payload json")["plan_digest"]
            .as_str()
            .expect("digest")
            .to_string();

        runtime.observe_accepted_events(&[event]);

        let db = tmp.path().join(".ralph/dag.db");
        assert!(db.exists(), "first relevant accepted event opens dag.db");
        let receipts = reopen_receipts(tmp.path());
        assert_eq!(receipts.len(), 1);
        let receipt = &receipts[0];
        assert_eq!(receipt.plan_key, "pf-test");
        assert_eq!(receipt.artifact_path, ARTIFACT_REL);
        assert_eq!(receipt.artifact_digest, expected_digest);
        assert_eq!(
            receipt.status,
            ralph_core::supervisor::dag_plan_receipt::ReceiptStatus::Pending
        );
        assert_eq!(runtime.observation_count(), 1, "one tick per batch");
    }

    /// Replaying the same accepted `forge.plan.ready` is an idempotent
    /// no-op — one durable row, no error.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn plan_ready_replay_is_idempotent() {
        let (tmp, mut runtime) = fixture(SchedulerMode::Dag);
        let event = plan_ready_event(tmp.path());
        runtime.observe_accepted_events(std::slice::from_ref(&event));
        runtime.observe_accepted_events(&[event]);
        let receipts = reopen_receipts(tmp.path());
        assert_eq!(receipts.len(), 1, "replay must not duplicate the receipt");
    }

    /// Accepted `forge.concurrency.approved` activates the receipt and
    /// registers + activates the plan in the store.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn approved_activates_receipt_and_registers_plan() {
        use ralph_core::supervisor::dag_store::DagSchedulerStore as _;
        let (tmp, mut runtime) = fixture(SchedulerMode::Dag);
        runtime.observe_accepted_events(&[plan_ready_event(tmp.path())]);
        runtime.observe_accepted_events(&[approved_event()]);

        let receipts = reopen_receipts(tmp.path());
        assert_eq!(
            receipts[0].status,
            ralph_core::supervisor::dag_plan_receipt::ReceiptStatus::Active,
            "approval activates the receipt"
        );
        let plans = ralph_core::supervisor::dag_store_rusqlite::RusqliteDagSchedulerStore::open(
            tmp.path().join(".ralph/dag.db"),
        )
        .expect("reopen plan store");
        let plan = plans
            .get_plan("pf-test")
            .expect("get plan")
            .expect("plan registered");
        assert_eq!(
            plan.status,
            ralph_core::supervisor::dag_store::PlanStatus::Active
        );
        assert_eq!(plan.unit_ids, vec!["U1", "U2", "U3"]);
        // Pipeline slots are seeded so per-unit events can route.
        assert_eq!(runtime.pipeline_stage("U1"), Some(Stage::Execute));
    }

    /// Double approval is an idempotent no-op (re-activate Active).
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn approved_replay_is_idempotent() {
        let (tmp, mut runtime) = fixture(SchedulerMode::Dag);
        runtime.observe_accepted_events(&[plan_ready_event(tmp.path())]);
        runtime.observe_accepted_events(std::slice::from_ref(&approved_event()));
        runtime.observe_accepted_events(&[approved_event()]);
        let receipts = reopen_receipts(tmp.path());
        assert_eq!(receipts.len(), 1);
        assert_eq!(
            receipts[0].status,
            ralph_core::supervisor::dag_plan_receipt::ReceiptStatus::Active
        );
    }

    /// Digest drift on a re-emitted `forge.plan.ready` fails closed:
    /// the original receipt stays, no panic, no overwrite.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn digest_conflict_keeps_original_receipt() {
        let (tmp, mut runtime) = fixture(SchedulerMode::Dag);
        let event = plan_ready_event(tmp.path());
        let original_digest: String = serde_json::from_str::<Value>(&event.payload)
            .expect("payload json")["plan_digest"]
            .as_str()
            .expect("digest")
            .to_string();
        runtime.observe_accepted_events(&[event]);

        // Rewrite the artifact (same plan_key, different bytes) and
        // re-emit with the new digest.
        let mutated = PLAN_ARTIFACT.replacen("Foundation", "Foundation v2", 1);
        std::fs::write(tmp.path().join(ARTIFACT_REL), mutated).expect("rewrite artifact");
        runtime.observe_accepted_events(&[plan_ready_event(tmp.path())]);

        let receipts = reopen_receipts(tmp.path());
        assert_eq!(receipts.len(), 1);
        assert_eq!(
            receipts[0].artifact_digest, original_digest,
            "digest conflict must not overwrite the recorded identity"
        );
    }

    /// TG-S05 laziness: a batch with no seam topic leaves no dag.db
    /// behind and records no observation.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn unrelated_events_leave_no_dag_db() {
        let (tmp, mut runtime) = fixture(SchedulerMode::Dag);
        let events = vec![
            ralph_proto::Event::new("exec.unit.ready", "{}"),
            ralph_proto::Event::new("exec.unit.done", "{}"),
            ralph_proto::Event::new("work.done", "{}"),
        ];
        runtime.observe_accepted_events(&events);
        assert!(
            !tmp.path().join(".ralph/dag.db").exists(),
            "no forge seam event → no dag.db (TG-S05 lazy-open contract)"
        );
        assert_eq!(runtime.observation_count(), 0);
    }

    /// Orphan approval (no prior plan.ready) must not create the db
    /// file just to correlate.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn orphan_approval_does_not_create_db() {
        let (tmp, mut runtime) = fixture(SchedulerMode::Dag);
        runtime.observe_accepted_events(&[approved_event()]);
        assert!(!tmp.path().join(".ralph/dag.db").exists());
    }

    /// `dag_shadow` mode keeps everything in-process: receipts work,
    /// but no durable file is ever created.
    #[test]
    fn shadow_mode_records_in_memory_without_disk() {
        let (tmp, mut runtime) = fixture(SchedulerMode::DagShadow);
        runtime.observe_accepted_events(&[plan_ready_event(tmp.path())]);
        runtime.observe_accepted_events(&[approved_event()]);
        assert!(
            !tmp.path().join(".ralph/dag.db").exists(),
            "dag_shadow must never open the durable store"
        );
        let receipt = runtime.receipt("pf-test").expect("in-memory receipt");
        assert_eq!(
            receipt.status,
            ralph_core::supervisor::dag_plan_receipt::ReceiptStatus::Active
        );
    }

    /// Per-unit accepted events route through the driver: executed →
    /// Review, reviewed ACCEPTED → Verify; unknown units are a
    /// diagnostic, not a panic.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn unit_events_route_through_driver() {
        let (tmp, mut runtime) = fixture(SchedulerMode::Dag);
        runtime.observe_accepted_events(&[plan_ready_event(tmp.path())]);
        runtime.observe_accepted_events(&[approved_event()]);

        let executed = ralph_proto::Event::new(
            topics::UNIT_EXECUTED,
            serde_json::json!({
                "unit_id": "U1",
                "plan_key": "pf-test",
                "task_id": "t-1",
                "task_key": "forge:pf-test:U1",
                "content_hash": "abc",
                "unit_report_path": ".ralph/forge/pf-test/units/U1-completion.md",
            })
            .to_string(),
        );
        runtime.observe_accepted_events(&[executed]);
        assert_eq!(runtime.pipeline_stage("U1"), Some(Stage::Review));

        let reviewed = ralph_proto::Event::new(
            topics::UNIT_REVIEWED,
            serde_json::json!({
                "unit_id": "U1",
                "plan_key": "pf-test",
                "task_id": "t-1",
                "task_key": "forge:pf-test:U1",
                "verdict": "ACCEPTED",
                "review_report_path": ".ralph/forge/pf-test/units/U1-review.md",
            })
            .to_string(),
        );
        runtime.observe_accepted_events(&[reviewed]);
        assert_eq!(runtime.pipeline_stage("U1"), Some(Stage::Verify));

        // Unknown unit: blocked by the pipeline, logged, no panic.
        let unknown = ralph_proto::Event::new(
            topics::UNIT_EXECUTED,
            serde_json::json!({"unit_id": "U-nope"}).to_string(),
        );
        runtime.observe_accepted_events(&[unknown]);
        assert_eq!(runtime.pipeline_stage("U-nope"), None);
    }

    #[test]
    fn correction_request_reenters_unit_at_fixer_attempt() {
        let (tmp, mut runtime) = fixture(SchedulerMode::Dag);
        runtime.observe_accepted_events(&[plan_ready_event(tmp.path())]);
        runtime.observe_accepted_events(&[approved_event()]);

        let correction = ralph_proto::Event::new(
            seam_topics::CORRECTION_REQUESTED,
            serde_json::json!({
                "plan_key": "pf-test",
                "affected_unit_ids": ["U1"],
                "correction_request_path": ".ralph/forge/pf-test/failures/U1.md",
            })
            .to_string(),
        );
        runtime.observe_accepted_events(&[correction]);

        assert_eq!(runtime.pipeline_stage("U1"), Some(Stage::Review));
        assert_eq!(runtime.pipeline.attempt_of("U1"), Some(1));
    }

    /// The observation tick reflects dependency gating: with no
    /// integration head (fixture is not a git repo) every unit is
    /// reported `BlockedNoTargetHead`.
    #[test]
    fn tick_records_dependency_aware_observation() {
        let (tmp, mut runtime) = fixture(SchedulerMode::DagShadow);
        runtime.observe_accepted_events(&[plan_ready_event(tmp.path())]);
        let mut reasons: Vec<(String, String)> = Vec::new();
        runtime.sink.with_observations(|obs| {
            assert_eq!(obs.len(), 1);
            assert_eq!(obs[0].plan_key, "pf-test");
            assert_eq!(obs[0].candidate_count, 3);
            reasons = obs[0].decisions.clone();
        });
        // No git repo → no target head → dependency-free units block
        // at the head gate; U3 blocks on its un-integrated dependency
        // first (the dependency gate precedes the head gate).
        let reason_of = |unit: &str| {
            reasons
                .iter()
                .find(|(id, _)| id == unit)
                .map(|(_, r)| r.as_str())
                .unwrap_or("missing")
        };
        assert_eq!(reason_of("U1"), "BlockedNoTargetHead");
        assert_eq!(reason_of("U2"), "BlockedNoTargetHead");
        assert_eq!(reason_of("U3"), "BlockedDependencies");
    }

    #[test]
    fn tick_enforces_verified_resource_capacity_and_claims() {
        let (tmp, mut runtime) = fixture(SchedulerMode::DagShadow);
        std::fs::write(tmp.path().join(ARTIFACT_REL), RESOURCE_ARTIFACT)
            .expect("write resource artifact");
        init_git_head(tmp.path());
        runtime.observe_accepted_events(&[plan_ready_event(tmp.path())]);
        assert_eq!(runtime.plans["pf-test"].resource_capacities.len(), 1);
        assert_eq!(runtime.plans["pf-test"].units[0].resource_claims.len(), 1);

        runtime.sink.with_observations(|observations| {
            let observation = observations.last().expect("resource observation");
            assert_eq!(observation.candidate_count, 2);
            let reason_of = |unit: &str| {
                observation
                    .decisions
                    .iter()
                    .find(|(id, _)| id == unit)
                    .map(|(_, reason)| reason.as_str())
                    .unwrap_or("missing")
            };
            assert_eq!(reason_of("U1"), "Admitted");
            assert_eq!(reason_of("U2"), "BlockedResources");
        });
    }

    /// Sanity: the fixture artifact passes the real handoff loader
    /// (guards the other tests against a silently broken fixture).
    #[test]
    fn fixture_artifact_loads_through_real_handoff() {
        let (tmp, _runtime) = fixture(SchedulerMode::DagShadow);
        let payload = serde_json::json!({
            "plan_key": "pf-test",
            "execution_plan_path": ARTIFACT_REL,
        });
        let handoff = load_plan_handoff(&payload, tmp.path()).expect("handoff loads");
        assert_eq!(handoff.tasks.len(), 3);
        assert_eq!(handoff.wave_total, 2);
    }

    /// Crash window: the durable reservation exists but the process has not
    /// reached the PID handshake. Recovery must block instead of guessing
    /// that no child was spawned and launching a duplicate.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn recovery_blocks_reservation_without_pid() {
        let (tmp, mut runtime) = fixture(SchedulerMode::Dag);
        init_git_head(tmp.path());
        runtime.observe_accepted_events(&[plan_ready_event(tmp.path())]);
        runtime.observe_accepted_events(&[approved_event()]);
        runtime.attach_execution_context(execution_context(tmp.path()));
        runtime
            .journal()
            .expect("durable journal")
            .reserve_job(&execute_identity(), 1)
            .expect("reserve job");
        drop(runtime);

        let mut recovered = DagSchedulerRuntime::new(
            SchedulerMode::Dag,
            resolved_pools(),
            tmp.path().to_path_buf(),
        );
        recovered.attach_execution_context(execution_context(tmp.path()));
        recovered.recover_after_restart();

        assert!(
            recovered.blocked_plans.contains("pf-test"),
            "NULL PID launch reservation must fail closed"
        );
        assert!(recovered.active_jobs.is_empty());
        assert!(recovered.pending_spawns.is_empty());
    }

    /// Crash window: the PID handshake was durable, but the child died before
    /// producing a result. Recovery must settle the same typed failure path
    /// and must not reserve a replacement in the same pass.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn recovery_settles_dead_pid_as_typed_failure() {
        let (tmp, mut runtime) = fixture(SchedulerMode::Dag);
        init_git_head(tmp.path());
        runtime.observe_accepted_events(&[plan_ready_event(tmp.path())]);
        runtime.observe_accepted_events(&[approved_event()]);
        runtime.attach_execution_context(execution_context(tmp.path()));
        let identity = execute_identity();
        runtime
            .journal()
            .expect("durable journal")
            .reserve_job(&identity, 1)
            .expect("reserve job");
        runtime
            .journal()
            .expect("durable journal")
            .record_job_pid(&identity, 999_999, 2)
            .expect("record dead child pid");
        drop(runtime);

        let mut recovered = DagSchedulerRuntime::new(
            SchedulerMode::Dag,
            resolved_pools(),
            tmp.path().to_path_buf(),
        );
        recovered.attach_execution_context(execution_context(tmp.path()));
        recovered.recover_after_restart();

        assert!(recovered.blocked_plans.is_empty());
        assert_eq!(recovered.merge_queue.len(), 1);
        let failure = &recovered.merge_queue.front().expect("failure event").event;
        assert_eq!(failure.topic.as_str(), "forge.unit.execution_failed");
        assert!(failure.payload.contains("orphan_or_empty_result"));
        assert!(
            recovered
                .journal()
                .expect("durable journal")
                .list_jobs("pf-test")
                .expect("list jobs")
                .iter()
                .any(|job| job.identity == identity && job.terminal.as_deref() == Some("failed"))
        );
    }
}
