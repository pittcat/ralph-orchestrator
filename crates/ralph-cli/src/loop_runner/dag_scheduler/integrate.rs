//! 2026-09-03-0959 plan, wiring step E2 — the `dag`-mode
//! integration + completion face of [`DagSchedulerRuntime`].
//!
//! A unit whose Verify stage passed (`forge.unit.verified` accepted)
//! is queued here; each tick integrates at most one pending unit,
//! in declared `(integration_order, unit_id)` order, through the U7
//! [`IntegrationOrchestrator`] (lane lease → squash on lane-time
//! head → targeted gate → persisted intent → CAS FF → integration
//! record). On success the seam appends `forge.unit.integrated` as a
//! runtime coordination event; the read-back of that accepted event
//! is the projection-acknowledged signal (acceptance runs the
//! `close_task` state projection), and only then does the unit join
//! the admission input's `integrated_units` so dependents unlock.
//! When every unit of a plan is integrated-and-acked, the seam emits
//! `forge.exec.development.done` exactly once, fenced by the
//! `dag_terminal_emits` table (S15).
//!
//! Invariants:
//!   - **Fail-closed inputs.** The unit commit is read from git
//!     (`refs/heads/ralph/<loop>/<unit>`), never from the agent's
//!     payload. An empty `tests` gate set never reaches the
//!     orchestrator (the port would fail closed anyway; the seam
//!     fails earlier with a clearer reason).
//!   - **Idempotent re-drive.** The integration record's natural-key
//!     idempotency plus the terminal-emit fence make a repeated tick
//!     (or an emit-IO failure re-drive) safe: no double CAS, no
//!     double event.
//!   - **No new topics.** Integration failures reuse
//!     `forge.unit.execution_failed` (the `forge.unit.*` family is
//!     pinned at six members by TG-S06).
//!   - **No wave-path effect.** Every entry point is gated on
//!     `mode == Dag` plus an attached execution context.

use std::process::Command;

use ralph_core::supervisor::changed_path_guard::parse_name_status_z;
use ralph_core::supervisor::dag_integration::IntegrationStore;
use ralph_core::supervisor::dag_store_rusqlite::RusqliteIntegrationStore;
use ralph_core::supervisor::integration_lane::{GateCommandSpec, LaneError};
use serde_json::Value;
use sha2::Digest as _;
use tracing::{debug, warn};

use super::integration::{
    IntegrationError, IntegrationOutcome, IntegrationRequest, real_orchestrator,
};
use super::spawn::SpawnKind;
use super::{DagSchedulerRuntime, now_ms};
use crate::loop_runner::wave::dispatcher::coordination::append_supervisor_coord_event;

/// Runtime-emitted coordination topics. No hat publishes them; the
/// schema marks both as runtime-sourced.
pub(crate) const UNIT_INTEGRATED: &str = "forge.unit.integrated";
pub(crate) const DEVELOPMENT_DONE: &str = "forge.exec.development.done";
/// Runtime-injected terminal block (adversarial review R5/R7): the
/// runtime emits `forge.plan.blocked` itself when a plan-level
/// precondition is violated or the execution face wedges.
pub(crate) const PLAN_BLOCKED: &str = "forge.plan.blocked";

/// Loop-scoped integration lane (adversarial review R4): every unit
/// squash lands on `ralph/<loop>/integration`, a branch that is NEVER
/// checked out by any worktree. The finalizer fast-forwards the lane
/// into the operator's target branch at the delivery boundary, which
/// keeps the host index/worktree consistent — the pre-R4 lane CAS'd
/// the host's own checked-out branch via `update-ref` and left a
/// stale index behind (every subsequent worktree acquisition then
/// failed `HostDirty`).
pub(crate) fn integration_branch(loop_id: &str) -> String {
    format!("ralph/{loop_id}/integration")
}

/// Create the integration lane at `base` when it does not already
/// exist (idempotent across restarts). `git update-ref` only fires on
/// the missing-ref path; an existing lane is left untouched.
pub(crate) fn ensure_integration_branch(
    workspace: &std::path::Path,
    loop_id: &str,
    base: &str,
) -> Result<(), String> {
    let reference = format!("refs/heads/{}", integration_branch(loop_id));
    let exists = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["show-ref", "--verify", "--quiet", &reference])
        .status()
        .map_err(|err| format!("git show-ref integration lane: {err}"))?
        .success();
    if exists {
        return Ok(());
    }
    let status = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["update-ref", &reference, base])
        .status()
        .map_err(|err| format!("git update-ref integration lane: {err}"))?;
    if !status.success() {
        return Err(format!(
            "integration lane {reference} could not be created at {base}: exit {:?}",
            status.code()
        ));
    }
    Ok(())
}

/// CAS-stale retries before the unit is failed: a moving target head
/// is transient (sibling FF), a persistent one is an operator race.
const MAX_STALE_RETRIES: u32 = 3;

/// Canonical host-status fingerprint (adversarial review O2): the
/// same contract the `forge.audit.done` gate recomputes —
/// `git status --porcelain=v1 --untracked-files=all` with every
/// `.ralph/` line dropped, kept lines joined by `\n` plus one
/// trailing `\n` when non-empty, hex(SHA-256) of the UTF-8 bytes.
/// Recorded in `worktree-map.yml` at development-done time so the
/// auditor can detect target-worktree drift between integration
/// completion and the final audit.
fn host_status_fingerprint(repo_root: &std::path::Path) -> String {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .output()
    else {
        return String::new();
    };
    if !out.status.success() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut kept: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.len() < 3 {
            continue;
        }
        let path = line[3..].trim_matches('"');
        let path = path.rsplit(" -> ").next().unwrap_or(path);
        if path == ".ralph" || path.starts_with(".ralph/") {
            continue;
        }
        kept.push(line);
    }
    let joined = if kept.is_empty() {
        String::new()
    } else {
        format!("{}\n", kept.join("\n"))
    };
    format!("{:x}", sha2::Sha256::digest(joined.as_bytes()))
}

/// Resolve a branch tip (`git rev-parse <ref>`), empty string when
/// unresolvable. Used to pin `integration_head_sha` into the
/// delivery projection.
fn branch_tip(repo_root: &std::path::Path, reference: &str) -> String {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args([
            "rev-parse",
            "--verify",
            &format!("refs/heads/{reference}^{{commit}}"),
        ])
        .output()
    else {
        return String::new();
    };
    if !out.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A verified unit waiting for its integration slot.
#[derive(Debug)]
pub(crate) struct PendingIntegration {
    pub plan_key: String,
    pub unit_id: String,
    pub stale_retries: u32,
}

impl DagSchedulerRuntime {
    /// Reconcile durable integration records with the trusted ledger after a
    /// restart. An unacked record is safe to re-emit only when its prior
    /// coordination event is absent; if the event is present, the missing
    /// acknowledgement means the task projection is uncertain and the plan
    /// is blocked instead of being acknowledged speculatively.
    #[cfg(feature = "supervisor-db")]
    pub(super) fn reconcile_after_restart(&mut self) {
        let Some((main_events_file, lane)) = self.exec.as_ref().map(|exec| {
            (
                exec.main_events_file.clone(),
                integration_branch(&exec.loop_id),
            )
        }) else {
            return;
        };
        let ledger = crate::loop_runner::wave::io::read_worker_events(&main_events_file);
        let Some(journal) = self.journal() else {
            return;
        };
        let integration_store = journal.shared_with_integration();
        let plans: Vec<(String, String, Vec<String>)> = self
            .plans
            .iter()
            .map(|(key, plan)| {
                (
                    key.clone(),
                    lane.clone(),
                    plan.units.iter().map(|unit| unit.unit_id.clone()).collect(),
                )
            })
            .collect();
        for (plan_key, target_branch, unit_ids) in plans {
            for unit_id in unit_ids {
                let Ok(records) = integration_store.list_for_unit(&unit_id) else {
                    self.block_plan(&plan_key, "integration reconciliation read failed");
                    continue;
                };
                let matching_record = records
                    .iter()
                    .find(|record| record.target_branch == target_branch);
                let event_present = ledger.iter().any(|event| {
                    if event.topic != UNIT_INTEGRATED {
                        return false;
                    }
                    event
                        .payload
                        .as_deref()
                        .and_then(|payload| serde_json::from_str::<Value>(payload).ok())
                        .is_some_and(|payload| {
                            payload.get("plan_key").and_then(Value::as_str) == Some(&plan_key)
                                && payload.get("unit_id").and_then(Value::as_str) == Some(&unit_id)
                        })
                });
                if matching_record.is_some_and(|record| record.acked) && !event_present {
                    self.block_plan(
                        &plan_key,
                        "durable integration acknowledgement exists but trusted event is missing",
                    );
                    continue;
                }
                let Some(record) = records
                    .into_iter()
                    .find(|record| record.target_branch == target_branch && !record.acked)
                else {
                    continue;
                };
                if event_present {
                    self.block_plan(
                        &plan_key,
                        "integrated event exists but durable acknowledgement is missing",
                    );
                    continue;
                }
                let task_key = format!("forge:{plan_key}:{unit_id}");
                let payload = serde_json::json!({
                    "unit_id": unit_id,
                    "plan_key": plan_key,
                    "task_id": self.resolve_task_id(&task_key),
                    "task_key": task_key,
                    "integrated_commit": record.integrated_commit,
                });
                if let Err(err) =
                    append_supervisor_coord_event(&main_events_file, UNIT_INTEGRATED, &payload)
                {
                    self.block_plan(&plan_key, "integration event re-emission failed");
                    warn!(plan_key, error = %err, "DAG recovery: integrated event re-emit failed");
                }
            }
            if journal
                .has_terminal_emit(&plan_key, DEVELOPMENT_DONE)
                .unwrap_or(false)
                && !ledger.iter().any(|event| {
                    event.topic == DEVELOPMENT_DONE
                        && event
                            .payload
                            .as_deref()
                            .and_then(|payload| serde_json::from_str::<Value>(payload).ok())
                            .and_then(|payload| {
                                (payload.get("plan_key").and_then(Value::as_str) == Some(&plan_key))
                                    .then_some(())
                            })
                            .is_some()
                })
            {
                self.block_plan(
                    &plan_key,
                    "development.done fence exists but trusted ledger event is missing",
                );
            }
        }
    }

    #[cfg(not(feature = "supervisor-db"))]
    pub(super) fn reconcile_after_restart(&mut self) {}

    /// Queue a verified unit for integration. Deduped against the
    /// pending queue and the already-integrated set.
    ///
    /// `unit_id` may arrive plan-namespaced (`forge:{plan}:{unit}`)
    /// from the accepted-event seam; the durable record, the unit
    /// branch name (`ralph/<loop>/<unit>` — a `:` would make the ref
    /// invalid), and `plan.integrated` all key on the BARE id, so the
    /// namespace is stripped once at this boundary.
    pub(crate) fn queue_integration(&mut self, plan_key: &str, unit_id: &str) {
        let unit_id = super::spawn::strip_plan_namespace(plan_key, unit_id);
        let already_pending = self
            .pending_integrations
            .iter()
            .any(|p| p.plan_key == plan_key && p.unit_id == unit_id);
        let already_integrated = self
            .plans
            .get(plan_key)
            .is_some_and(|plan| plan.integrated.contains(unit_id));
        if already_pending || already_integrated {
            debug!(
                plan_key,
                unit_id, "DAG integration: duplicate queue request ignored"
            );
            return;
        }
        self.pending_integrations.push(PendingIntegration {
            plan_key: plan_key.to_string(),
            unit_id: unit_id.to_string(),
            stale_retries: 0,
        });
    }

    /// Integrate at most one pending unit per tick, in declared
    /// `(integration_order, unit_id)` order (serial-integrator
    /// semantics; the lane CAS is the cross-process backstop).
    pub(super) fn maybe_integrate_one(&mut self) {
        if self.exec.is_none() || self.pending_integrations.is_empty() {
            return;
        }
        let order_of = |p: &PendingIntegration| {
            self.plans
                .get(&p.plan_key)
                .and_then(|plan| plan.units.iter().find(|u| u.unit_id == p.unit_id))
                .map(|u| u.integration_order)
                .unwrap_or(u32::MAX)
        };
        let idx = self
            .pending_integrations
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| (order_of(a), &a.unit_id).cmp(&(order_of(b), &b.unit_id)))
            .map(|(i, _)| i)
            .expect("non-empty");
        let pending = self.pending_integrations.remove(idx);
        self.integrate_unit(pending);
    }

    /// Read-back arm: the accepted `forge.unit.integrated` event means
    /// the close-task state projection ran (acceptance applies
    /// projections before the seam sees the event). Ack the durable
    /// record and admit the unit into the admission input.
    pub(super) fn on_unit_integrated_accepted(&mut self, payload: &Value) {
        let unit_id = payload
            .get("unit_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let plan_key = payload
            .get("plan_key")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if unit_id.is_empty() || plan_key.is_empty() {
            debug!("DAG integration: integrated event without unit identity; ignored");
            return;
        }
        let Some(plan) = self.plans.get_mut(plan_key) else {
            debug!(plan_key, "DAG integration: ack for unknown plan; ignored");
            return;
        };
        if !plan.units.iter().any(|unit| unit.unit_id == unit_id) {
            debug!(
                plan_key,
                unit_id, "DAG integration: ack for unknown unit; ignored"
            );
            return;
        }
        // The durable record keys on the lane branch the orchestrator
        // CAS'd (adversarial review R4), not the plan's declared
        // target_branch.
        let lane = self
            .exec
            .as_ref()
            .map(|exec| integration_branch(&exec.loop_id))
            .unwrap_or_else(|| plan.target_branch.clone());
        let Some(store) = self.integration_store() else {
            // Non-durable builds never reach here (spawn refuses
            // without a journal); stay fail-closed anyway.
            warn!(
                unit_id,
                "DAG integration: no durable store; unit stays unacked"
            );
            return;
        };
        if let Err(err) = store.ack(unit_id, &lane) {
            warn!(
                unit_id,
                target_branch = %lane,
                error = %err,
                "DAG integration: record ack failed; unit stays unacked (no development.done)"
            );
            return;
        }
        self.plans
            .get_mut(plan_key)
            .expect("plan present")
            .integrated
            .insert(unit_id.to_string());
        self.write_forge_projection(plan_key);
    }

    /// S15: when every unit of a plan is integrated AND acknowledged,
    /// emit `forge.exec.development.done` exactly once. The
    /// `dag_terminal_emits` PRIMARY KEY is the durable fence: the
    /// first inserter wins the emit permit; replays (second tick,
    /// restarted loop) lose and emit nothing.
    pub(super) fn maybe_emit_development_done(&mut self) {
        if self.exec.is_none() {
            return;
        }
        // Snapshot the completion candidates before taking the
        // journal (&mut self).
        let candidates: Vec<(String, String, String, usize)> = self
            .plans
            .iter()
            // A blocked plan must never emit completion: recovery can
            // restore acked units into `plan.integrated` (mod.rs) while
            // `reconcile_after_restart` blocks the same plan for a
            // missing trusted ledger event — both states coexist, so
            // the candidate filter must consult `blocked_plans`.
            .filter(|(plan_key, plan)| {
                !self.blocked_plans.contains(plan_key.as_str())
                    && !plan.units.is_empty()
                    && plan
                        .units
                        .iter()
                        .all(|u| plan.integrated.contains(&u.unit_id))
            })
            .map(|(plan_key, plan)| {
                (
                    plan_key.clone(),
                    plan.artifact_path.clone(),
                    plan.artifact_digest.clone(),
                    plan.units.len(),
                )
            })
            .collect();
        for (plan_key, artifact_path, digest, unit_count) in candidates {
            let fence_key = format!(
                "{:x}",
                sha2::Sha256::digest(format!("{plan_key}|{DEVELOPMENT_DONE}|{digest}").as_bytes())
            );
            let Some(journal) = self.journal() else {
                return;
            };
            let main_events_file = self
                .exec
                .as_ref()
                .expect("checked")
                .main_events_file
                .clone();
            let already_in_ledger =
                crate::loop_runner::wave::io::read_worker_events(&main_events_file)
                    .iter()
                    .any(|event| {
                        event.topic == DEVELOPMENT_DONE
                            && event
                                .payload
                                .as_deref()
                                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                                .is_some_and(|payload| {
                                    payload.get("plan_key").and_then(Value::as_str)
                                        == Some(&plan_key)
                                })
                    });
            if !already_in_ledger {
                self.write_forge_projection(&plan_key);
                let (target_branch, target_start_sha, integration_lane) = self
                    .plans
                    .get(&plan_key)
                    .map(|plan| {
                        (
                            plan.target_branch.clone(),
                            plan.verified_base_commit.clone().unwrap_or_default(),
                            self.exec
                                .as_ref()
                                .map(|exec| integration_branch(&exec.loop_id))
                                .unwrap_or_default(),
                        )
                    })
                    .unwrap_or_default();
                let payload = serde_json::json!({
                    "execution_plan_path": artifact_path,
                    "completed_unit_count": unit_count,
                    "failed_unit_count": 0,
                    "plan_key": plan_key,
                    // Adversarial review O2: downstream hats (auditor /
                    // finalizer / cleanup / reporter) read the delivery
                    // coordinates from this payload and from
                    // `.ralph/forge/<plan>/worktree-map.yml` instead of a
                    // wave-era `forge.worktrees.ready` projection.
                    "target_branch": target_branch,
                    "target_start_sha": target_start_sha,
                    "integration_branch": integration_lane.clone(),
                    "integration_head_sha": branch_tip(&self.workspace, &integration_lane),
                    "target_status_fingerprint": host_status_fingerprint(&self.workspace),
                    "worktree_map_path": format!(".ralph/forge/{plan_key}/worktree-map.yml"),
                });
                if let Err(err) =
                    append_supervisor_coord_event(&main_events_file, DEVELOPMENT_DONE, &payload)
                {
                    // Do not win the fence until the event is durable. A
                    // retry can therefore recover an append failure.
                    warn!(
                        plan_key,
                        error = %err,
                        "DAG integration: development.done append failed; delivery remains pending"
                    );
                    continue;
                }
            }
            match journal.try_record_terminal_emit(
                &plan_key,
                DEVELOPMENT_DONE,
                &fence_key,
                now_ms() as i64,
            ) {
                Ok(true) => {
                    debug!(
                        plan_key,
                        "DAG integration: development.done delivery fenced"
                    );
                }
                Ok(false) => {
                    debug!(
                        plan_key,
                        "DAG integration: development.done already fenced; replay is a no-op"
                    );
                }
                Err(err) => {
                    warn!(plan_key, error = %err, "DAG integration: terminal-emit fence error");
                }
            }
        }
    }

    /// Durable integration store, shared-connection with the job
    /// journal. `None` outside durable dag mode (spawn refuses to
    /// launch there too, so no unit can legitimately arrive here).
    #[cfg(feature = "supervisor-db")]
    fn integration_store(&mut self) -> Option<RusqliteIntegrationStore> {
        self.journal()
            .map(|journal| journal.shared_with_integration())
    }

    #[cfg(not(feature = "supervisor-db"))]
    fn integration_store(&mut self) -> Option<RusqliteIntegrationStore> {
        None
    }

    /// Drive one pending unit through the U7 orchestrator. Every
    /// failure path either requeues (transient lane race) or
    /// synthesises `forge.unit.execution_failed` — never a panic,
    /// never a silent drop.
    fn integrate_unit(&mut self, pending: PendingIntegration) {
        let plan_key = pending.plan_key.clone();
        let unit_id = pending.unit_id.clone();
        let lookup = self.plans.get(&plan_key).and_then(|plan| {
            let unit = plan.units.iter().find(|u| u.unit_id == unit_id)?;
            Some((
                plan.target_branch.clone(),
                plan.verified_base_commit.clone()?,
                unit.tests.clone(),
                unit.allowed_paths.clone(),
                unit.forbidden_paths.clone(),
            ))
        });
        let Some((_plan_target_branch, plan_base, tests, allowed_paths, forbidden_paths)) = lookup
        else {
            self.fail_integration(
                &pending,
                "integration_unavailable",
                "plan topology or verified base commit missing",
            );
            return;
        };
        // Adversarial review R4/R6: the lane target is the dedicated
        // `ralph/<loop>/integration` branch (never the host's checked-out
        // branch), and the unit-diff anchor is the first-admission base
        // pin — for a dependent unit that pin already contains every
        // acked dependency, so the diff covers only this unit's work.
        let loop_id = self
            .exec
            .as_ref()
            .expect("checked by caller")
            .loop_id
            .clone();
        let target_branch = integration_branch(&loop_id);
        let base_commit = self
            .ensure_stores()
            .ok()
            .and_then(|stores| {
                stores
                    .plans
                    .get_unit_base(&plan_key, &unit_id)
                    .ok()
                    .flatten()
            })
            .map(|pin| pin.base_commit)
            .unwrap_or(plan_base);
        let integration_order = self
            .plans
            .get(&plan_key)
            .and_then(|plan| plan.units.iter().find(|u| u.unit_id == unit_id))
            .map(|u| u.integration_order)
            .unwrap_or(u32::MAX);

        // A Unit without an explicit path policy has no authorization
        // boundary. Never derive one from the observed diff: that would let
        // the candidate choose its own allowlist after the fact.
        if allowed_paths.is_empty() {
            self.fail_integration(
                &pending,
                "integration_path_policy_unconfigured",
                "unit declares no `allowed_paths` in the execution plan",
            );
            return;
        }

        // Fail-closed BEFORE the orchestrator when the unit declares
        // no targeted gate commands (PMI-0041: an empty gate set must
        // never silently pass).
        if tests.is_empty() {
            self.fail_integration(
                &pending,
                "integration_gate_unconfigured",
                "unit declares no `tests` gate commands in the execution plan",
            );
            return;
        }
        // `tests` entries are argv strings; quotes only group an argument and
        // shell metacharacters are never interpreted by the runtime.
        let mut gate_commands = Vec::with_capacity(tests.len());
        for command in &tests {
            let Some((program, args)) = parse_gate_command(command) else {
                self.fail_integration(
                    &pending,
                    "integration_gate_invalid",
                    "unit `tests` contains an unterminated quote or escape",
                );
                return;
            };
            gate_commands.push(GateCommandSpec { program, args });
        }
        if gate_commands.is_empty() {
            self.fail_integration(
                &pending,
                "integration_gate_unconfigured",
                "unit `tests` entries are all empty",
            );
            return;
        }

        // The lane is created at plan approval, but a restart can
        // lose the window between approval and the first CAS. Ensure
        // it here too — idempotent, one `git show-ref` probe per
        // integration — so the lane invariant holds at the CAS seam
        // regardless of which boundary ran first.
        if let Err(err) = ensure_integration_branch(&self.workspace, &loop_id, &base_commit) {
            self.fail_integration(&pending, "integration_lane_unavailable", &err);
            return;
        }
        let unit_commit = match read_unit_branch_tip(&self.workspace, &loop_id, &unit_id) {
            Ok(sha) => sha,
            Err(reason) => {
                self.fail_integration(&pending, "integration_missing_unit_commit", &reason);
                return;
            }
        };
        let changed_paths = match diff_entries(&self.workspace, &base_commit, &unit_commit) {
            Ok(entries) => entries,
            Err(reason) => {
                self.fail_integration(&pending, "integration_error", &reason);
                return;
            }
        };
        let request = IntegrationRequest {
            unit_id: unit_id.clone(),
            integration_order,
            target_branch: target_branch.clone(),
            base_commit,
            unit_commit,
            changed_paths,
            allowlist: allowed_paths.clone(),
            declared_paths: allowed_paths,
            forbidden_paths,
            created_at_ms: now_ms() as i64,
        };
        let orchestrator = match real_orchestrator(self.workspace.clone(), gate_commands) {
            Ok(orchestrator) => orchestrator,
            Err(err) => {
                self.fail_integration(
                    &pending,
                    "integration_store_unavailable",
                    &format!("durable integration store open failed: {err}"),
                );
                return;
            }
        };
        match orchestrator.integrate(request) {
            Ok(IntegrationOutcome::Integrated { record, .. }) => {
                self.emit_integrated(&plan_key, &unit_id, &record.integrated_commit, &pending);
            }
            Ok(IntegrationOutcome::GateFailed { reason }) => {
                self.fail_integration(&pending, "integration_gate_failed", &reason);
            }
            Ok(IntegrationOutcome::StaleExpected { expected, actual }) => {
                let mut retry = pending;
                retry.stale_retries += 1;
                if retry.stale_retries >= MAX_STALE_RETRIES {
                    self.fail_integration(
                        &retry,
                        "integration_cas_stale",
                        &format!(
                            "target head moved {MAX_STALE_RETRIES} times (expected {expected}, actual {actual})"
                        ),
                    );
                } else {
                    debug!(
                        unit_id,
                        attempt = retry.stale_retries,
                        "DAG integration: CAS stale; requeued"
                    );
                    self.pending_integrations.push(retry);
                }
            }
            Ok(IntegrationOutcome::CasRefused { reason }) => {
                self.fail_integration(&pending, "integration_cas_refused", &reason);
            }
            Err(IntegrationError::Lane(LaneError::TargetBusy)) => {
                debug!(unit_id, "DAG integration: target lane busy; requeued");
                self.pending_integrations.push(pending);
            }
            Err(err) => {
                self.fail_integration(
                    &pending,
                    "integration_error",
                    &format!("integration orchestrator error: {err}"),
                );
            }
        }
    }

    /// Append the runtime `forge.unit.integrated` coordination event.
    /// The integration record is already durable, so an append
    /// failure requeues the unit — the re-drive is idempotent
    /// (`record_integrated` replays, the event is rewritten).
    fn emit_integrated(
        &mut self,
        plan_key: &str,
        unit_id: &str,
        integrated_commit: &str,
        pending: &PendingIntegration,
    ) {
        let task_key = format!("forge:{plan_key}:{unit_id}");
        let task_id = self.resolve_task_id(&task_key);
        let payload = serde_json::json!({
            "unit_id": unit_id,
            "plan_key": plan_key,
            "task_id": task_id,
            "task_key": task_key,
            "integrated_commit": integrated_commit,
        });
        let main_events_file = self
            .exec
            .as_ref()
            .expect("checked by caller")
            .main_events_file
            .clone();
        if let Err(err) =
            append_supervisor_coord_event(&main_events_file, UNIT_INTEGRATED, &payload)
        {
            warn!(
                unit_id,
                error = %err,
                "DAG integration: integrated-event append failed; unit requeued (idempotent re-drive)"
            );
            self.pending_integrations.push(PendingIntegration {
                plan_key: plan_key.to_string(),
                unit_id: unit_id.to_string(),
                stale_retries: pending.stale_retries,
            });
        }
    }

    /// Persist the DAG forge projection (adversarial review O2): the
    /// machine-readable replacement for the wave-era
    /// `forge.worktrees.ready` artifact. Downstream hats
    /// (tester/auditor/finalizer/cleanup/reporter) read
    /// `.ralph/forge/<plan>/worktree-map.yml` for delivery
    /// coordinates — the integration lane, unit branches/worktrees,
    /// and the verified start SHA — instead of reconstructing them
    /// from wave payloads.
    pub(super) fn write_forge_projection(&self, plan_key: &str) {
        let Some(exec) = self.exec.as_ref() else {
            return;
        };
        let Some(plan) = self.plans.get(plan_key) else {
            return;
        };
        let relative = format!(".ralph/forge/{plan_key}/worktree-map.yml");
        let path = self.workspace.join(&relative);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let lane = integration_branch(&exec.loop_id);
        let mut doc = String::from("version: 1\n");
        doc.push_str(&format!("plan_key: {plan_key}\n"));
        doc.push_str(&format!("target_branch: {}\n", plan.target_branch));
        doc.push_str(&format!("integration_branch: {lane}\n"));
        doc.push_str(&format!(
            "target_start_sha: {}\n",
            plan.verified_base_commit.clone().unwrap_or_default()
        ));
        doc.push_str(&format!(
            "target_status_fingerprint: {}\n",
            host_status_fingerprint(&self.workspace)
        ));
        doc.push_str(&format!(
            "integration_head_sha: {}\n",
            branch_tip(&self.workspace, &lane)
        ));
        doc.push_str("units:\n");
        for unit in &plan.units {
            let branch = format!("ralph/{}/{}", exec.loop_id, unit.unit_id);
            let worktree = super::worktree::UnitWorktree::worktree_path_for(
                &self.workspace,
                &exec.loop_id,
                &unit.unit_id,
            );
            let rel_worktree = worktree
                .strip_prefix(&self.workspace)
                .unwrap_or(&worktree)
                .to_string_lossy()
                .replace('\\', "/");
            doc.push_str(&format!(
                "  - unit_id: {}\n    branch: {branch}\n    worktree: {rel_worktree}\n    integrated: {}\n",
                unit.unit_id,
                plan.integrated.contains(&unit.unit_id)
            ));
        }
        if let Err(err) = std::fs::write(&path, doc) {
            warn!(
                plan_key,
                error = %err,
                "DAG projection: worktree-map write failed"
            );
        }
    }

    /// Emit `forge.plan.blocked` as a runtime-injected coordination
    /// event (adversarial review R5/R7). The block note lands under
    /// the plan's forge artifact root first so
    /// `context_artifact_path` is a real file downstream hats can
    /// read. Callers are responsible for `blocked_plans` fencing via
    /// `block_plan` so the same wedge emits once.
    pub(crate) fn emit_plan_blocked(&mut self, plan_key: &str, reason: &str) {
        let Some(exec) = self.exec.as_ref() else {
            warn!(
                plan_key, %reason,
                "DAG runtime: plan blocked before execution context; event not emitted"
            );
            return;
        };
        let context_rel = format!(".ralph/forge/{plan_key}/blocks/runtime-blocked.md");
        let context_path = self.workspace.join(&context_rel);
        if let Some(parent) = context_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            &context_path,
            format!("# DAG runtime block\n\n- plan_key: {plan_key}\n- reason: {reason}\n"),
        );
        let plan_path = self
            .plans
            .get(plan_key)
            .map(|plan| plan.artifact_path.clone())
            .unwrap_or_default();
        let payload = serde_json::json!({
            "reason": reason,
            "plan_path": plan_path,
            "context_artifact_path": context_rel,
            "forge_artifact_root": format!(".ralph/forge/{plan_key}/"),
            "plan_key": plan_key,
        });
        if let Err(err) =
            append_supervisor_coord_event(&exec.main_events_file, PLAN_BLOCKED, &payload)
        {
            warn!(
                plan_key,
                error = %err,
                "DAG runtime: forge.plan.blocked append failed"
            );
        }
    }

    /// Synthesise the unit's failure event. No journal terminal: the
    /// verify-stage job already closed accepted; the integration
    /// failure is a business fact, not a job-attempt transition.
    fn fail_integration(
        &mut self,
        pending: &PendingIntegration,
        failure_class: &str,
        reason: &str,
    ) {
        let task_key = format!("forge:{}:{}", pending.plan_key, pending.unit_id);
        let task_id = self.resolve_task_id(&task_key);
        let payload = serde_json::json!({
            "unit_id": pending.unit_id,
            "plan_key": pending.plan_key,
            "task_id": task_id,
            "task_key": task_key,
            "reason": reason,
            "failure_class": failure_class,
        });
        self.queue_result_event(
            SpawnKind::Execute.failure_topic(),
            SpawnKind::Execute.hat().to_string(),
            payload,
            None,
        );
    }
}

/// Parse one execution-plan test entry into argv without invoking a shell.
/// Whitespace separates words outside quotes; single/double quotes group
/// characters and backslash escapes the following character. Shell operators
/// remain ordinary argument characters, so the gate cannot gain shell
/// expansion as a side effect of supporting quoted paths or labels.
fn parse_gate_command(command: &str) -> Option<(String, Vec<String>)> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut quote = Quote::None;
    let mut escaped = false;
    let mut token = String::new();
    let mut words = Vec::new();
    let mut token_started = false;

    for ch in command.chars() {
        if escaped {
            token.push(ch);
            token_started = true;
            escaped = false;
            continue;
        }
        match quote {
            Quote::Single => {
                if ch == '\'' {
                    quote = Quote::None;
                } else {
                    token.push(ch);
                }
                token_started = true;
            }
            Quote::Double => match ch {
                '"' => quote = Quote::None,
                '\\' => escaped = true,
                _ => token.push(ch),
            },
            Quote::None => match ch {
                '\'' => {
                    quote = Quote::Single;
                    token_started = true;
                }
                '"' => {
                    quote = Quote::Double;
                    token_started = true;
                }
                '\\' => {
                    escaped = true;
                    token_started = true;
                }
                c if c.is_whitespace() => {
                    if token_started {
                        words.push(std::mem::take(&mut token));
                        token_started = false;
                    }
                }
                _ => {
                    token.push(ch);
                    token_started = true;
                }
            },
        }
    }

    if escaped || quote != Quote::None {
        return None;
    }
    if token_started {
        words.push(token);
    }
    let program = words.first()?.clone();
    Some((program, words.into_iter().skip(1).collect()))
}

/// Read the unit branch tip: the executor's commit is the ONLY
/// trusted source of `unit_commit` (the agent payload's content_hash
/// is advisory).
fn read_unit_branch_tip(
    workspace: &std::path::Path,
    loop_id: &str,
    unit_id: &str,
) -> Result<String, String> {
    let reference = format!("refs/heads/ralph/{loop_id}/{unit_id}^{{commit}}");
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("rev-parse")
        .arg("--verify")
        .arg(&reference)
        .output()
        .map_err(|err| format!("git rev-parse spawn: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "unit branch ralph/{loop_id}/{unit_id} not resolvable: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `git diff-tree --no-commit-id --name-status -z -r base..tip`,
/// parsed by the shared changed-path guard parser.
fn diff_entries(
    workspace: &std::path::Path,
    base_commit: &str,
    unit_commit: &str,
) -> Result<Vec<ralph_core::supervisor::changed_path_guard::DiffPathEntry>, String> {
    let range = format!("{base_commit}..{unit_commit}");
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .arg("diff-tree")
        .arg("--no-commit-id")
        .arg("--name-status")
        .arg("-z")
        .arg("-r")
        .arg(&range)
        .output()
        .map_err(|err| format!("git diff-tree spawn: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "git diff-tree {range} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_name_status_z(&output.stdout).map_err(|err| format!("diff-tree parse: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_runner::dag_scheduler::spawn::DagExecutionContext;
    use crate::loop_runner::dag_scheduler::{PlanTopology, UnitTopology};
    use ralph_adapters::CliBackend;
    use ralph_core::config::{CliConfig, HatConfig, RalphConfig, ResolvedDagPools, SchedulerMode};
    use std::path::Path;
    use tempfile::TempDir;

    const ARTIFACT_REL: &str = ".ralph/forge/pf-test/execution-plan.yml";
    const PLAN_ARTIFACT: &str = r#"version: 1
plan_key: pf-test
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1-foundation
    allowed_paths: [u1.txt]
    tests:
      - "true"
  - id: U2
    title: Feature
    depends_on: []
    execution_wave: 1
    integration_order: 2
    target_branch: feat/u2-feature
    allowed_paths: [u2.txt]
    tests:
      - "true"
"#;
    /// U2 declares no gate commands (PMI-0041 fail-closed probe) —
    /// expressed as a directly-injected topology because `forge.plan.ready`
    /// now rejects such artifacts before they can reach integration.
    fn u2_no_tests_units() -> Vec<UnitTopology> {
        vec![
            unit_topology("U1", 1, &["u1.txt"], &["true"]),
            unit_topology("U2", 2, &["u2.txt"], &[]),
        ]
    }

    fn no_path_policy_units() -> Vec<UnitTopology> {
        vec![
            unit_topology("U1", 1, &[], &["true"]),
            unit_topology("U2", 2, &["u2.txt"], &["true"]),
        ]
    }

    /// U2 has one valid-looking gate and one malformed entry. The malformed
    /// entry must not be silently discarded while the valid gate runs.
    fn u2_invalid_test_units() -> Vec<UnitTopology> {
        vec![
            unit_topology("U1", 1, &["u1.txt"], &["true"]),
            unit_topology("U2", 2, &["u2.txt"], &["true", "cargo test 'unterminated"]),
        ]
    }

    fn exec_context(workspace: &Path) -> DagExecutionContext {
        let mut config = RalphConfig::default();
        for hat in ["executor", "reviewer", "verifier"] {
            let hat_config = HatConfig {
                name: hat.to_string(),
                instructions: format!("{hat} instructions"),
                ..HatConfig::default()
            };
            config.hats.insert(hat.to_string(), hat_config);
        }
        let backend =
            CliBackend::from_config(&CliConfig::default()).expect("default backend builds");
        DagExecutionContext::new(
            &config,
            &backend,
            "loop-test",
            workspace.join("events.jsonl"),
            None,
        )
    }

    fn runtime_with_plan(workspace: &Path) -> DagSchedulerRuntime {
        let pools = ResolvedDagPools {
            global: 4,
            executor: 2,
            reviewer: 2,
            verifier: 2,
            fixer: 2,
        };
        let mut runtime =
            DagSchedulerRuntime::new(SchedulerMode::Dag, pools, workspace.to_path_buf());
        let bytes = std::fs::read(workspace.join(ARTIFACT_REL)).expect("read artifact");
        let digest = ralph_core::artifact_canonicalizer::canonicalize(&bytes)
            .expect("canonicalize")
            .digest;
        let plan_ready = ralph_proto::Event::new(
            super::super::seam_topics::PLAN_READY,
            serde_json::json!({
                "plan_key": "pf-test",
                "execution_plan_path": ARTIFACT_REL,
                "plan_digest": digest,
            })
            .to_string(),
        );
        let approved = ralph_proto::Event::new(
            super::super::seam_topics::CONCURRENCY_APPROVED,
            serde_json::json!({
                "execution_plan_path": ARTIFACT_REL,
                "approval_report_path": ".ralph/forge/pf-test/concurrency-approval.md",
                "approved": true,
            })
            .to_string(),
        );
        // No exec context yet: tick cannot auto-spawn executors.
        runtime.observe_accepted_events(&[plan_ready, approved]);
        runtime
    }

    fn write_artifact(workspace: &Path, text: &str) {
        let artifact = workspace.join(ARTIFACT_REL);
        std::fs::create_dir_all(artifact.parent().expect("parent")).expect("mkdir");
        std::fs::write(&artifact, text).expect("write artifact");
    }

    fn git(workspace: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(args)
            .status()
            .expect("git spawn");
        assert!(status.success(), "git {:?} failed", args);
    }

    /// Direct topology registration for second-line defense tests:
    /// `forge.plan.ready` (O1) legitimately rejects artifacts whose
    /// units lack `tests`/`allowed_paths`, so a fixture probing the
    /// integration-time guard must bypass the plan boundary and
    /// inject the topology straight into `runtime.plans`.
    fn unit_topology(
        unit_id: &str,
        integration_order: u32,
        allowed: &[&str],
        tests: &[&str],
    ) -> UnitTopology {
        UnitTopology {
            unit_id: unit_id.to_string(),
            integration_order,
            depends_on: Vec::new(),
            tests: tests.iter().map(|s| (*s).to_string()).collect(),
            resource_claims: Vec::new(),
            allowed_paths: allowed.iter().map(std::path::PathBuf::from).collect(),
            forbidden_paths: Vec::new(),
        }
    }

    fn runtime_with_units(
        workspace: &Path,
        units: Vec<UnitTopology>,
        verified_base: Option<String>,
    ) -> DagSchedulerRuntime {
        let pools = ResolvedDagPools {
            global: 4,
            executor: 2,
            reviewer: 2,
            verifier: 2,
            fixer: 2,
        };
        let mut runtime =
            DagSchedulerRuntime::new(SchedulerMode::Dag, pools, workspace.to_path_buf());
        runtime.plans.insert(
            "pf-test".to_string(),
            PlanTopology {
                artifact_path: ARTIFACT_REL.to_string(),
                artifact_digest: "fixture".to_string(),
                target_branch: "main".to_string(),
                verified_base_commit: verified_base,
                units,
                resource_capacities: Vec::new(),
                integrated: std::collections::HashSet::new(),
            },
        );
        runtime
    }

    /// Git workspace on `main` with a base commit; optionally a unit
    /// branch `ralph/loop-test/U1` carrying one business commit.
    /// Returns the base SHA so callers can pin `verified_base_commit`.
    fn git_workspace(with_u1_branch: bool) -> (TempDir, String) {
        let tmp = TempDir::new().expect("temp workspace");
        let ws = tmp.path();
        git(ws, &["init", "-b", "main"]);
        git(ws, &["config", "user.email", "test@example.com"]);
        git(ws, &["config", "user.name", "test"]);
        std::fs::write(ws.join("base.txt"), "base\n").expect("write base");
        git(ws, &["add", "base.txt"]);
        git(ws, &["commit", "-m", "base"]);
        let base = Command::new("git")
            .arg("-C")
            .arg(ws)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("rev-parse")
            .stdout;
        let base = String::from_utf8_lossy(&base).trim().to_string();
        if with_u1_branch {
            git(ws, &["checkout", "-b", "ralph/loop-test/U1"]);
            std::fs::write(ws.join("u1.txt"), "u1\n").expect("write u1");
            git(ws, &["add", "u1.txt"]);
            git(ws, &["commit", "-m", "u1 work"]);
            git(ws, &["checkout", "main"]);
        }
        (tmp, base)
    }

    fn git_fixture(artifact_text: &str, with_u1_branch: bool) -> (TempDir, DagSchedulerRuntime) {
        let (tmp, _base) = git_workspace(with_u1_branch);
        let ws = tmp.path();
        write_artifact(ws, artifact_text);
        let mut runtime = runtime_with_plan(ws);
        runtime.attach_execution_context(exec_context(ws));
        (tmp, runtime)
    }

    /// Same git workspace, but the plan topology is injected directly
    /// (see `unit_topology`'s comment about the O1 plan boundary).
    fn git_fixture_with_units(
        units: Vec<UnitTopology>,
        with_u1_branch: bool,
    ) -> (TempDir, DagSchedulerRuntime) {
        let (tmp, base) = git_workspace(with_u1_branch);
        let mut runtime = runtime_with_units(tmp.path(), units, Some(base));
        runtime.attach_execution_context(exec_context(tmp.path()));
        (tmp, runtime)
    }

    fn integrated_payload(unit_id: &str) -> Value {
        serde_json::json!({
            "unit_id": unit_id,
            "plan_key": "pf-test",
            "task_id": "unresolved",
            "task_key": format!("forge:pf-test:{unit_id}"),
            "integrated_commit": "0".repeat(40),
        })
    }

    fn ledger_contains(workspace: &Path, needle: &str) -> usize {
        std::fs::read_to_string(workspace.join("events.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(needle))
            .count()
    }

    /// Empty `tests` gate set fails closed before any git side
    /// effect: one synthesised execution_failed, nothing integrated.
    #[test]
    fn empty_gate_set_fails_closed() {
        let (_tmp, mut runtime) = git_fixture_with_units(u2_no_tests_units(), false);
        runtime.queue_integration("pf-test", "U2");
        runtime.maybe_integrate_one();

        assert!(runtime.pending_integrations.is_empty());
        assert_eq!(runtime.merge_queue.len(), 1);
        let event = &runtime.merge_queue.front().expect("queued").event;
        assert_eq!(event.topic.as_str(), "forge.unit.execution_failed");
        assert!(event.payload.contains("integration_gate_unconfigured"));
    }

    /// A verified Unit without an explicit path policy must not derive
    /// authorization from its observed diff. Missing policy is a contract
    /// failure, not permission to integrate every changed path.
    #[test]
    fn missing_path_policy_fails_closed() {
        let (_tmp, mut runtime) = git_fixture_with_units(no_path_policy_units(), true);
        runtime.queue_integration("pf-test", "U1");
        runtime.maybe_integrate_one();

        assert_eq!(runtime.merge_queue.len(), 1);
        let event = &runtime.merge_queue.front().expect("queued").event;
        assert_eq!(event.topic.as_str(), "forge.unit.execution_failed");
        assert!(
            event.payload.contains("no `allowed_paths`"),
            "unexpected failure payload: {}",
            event.payload
        );
    }

    /// A malformed gate entry fails closed even when another gate entry is
    /// valid; silently dropping it would weaken the declared integration
    /// contract.
    #[test]
    fn malformed_gate_entry_fails_closed() {
        let (_tmp, mut runtime) = git_fixture_with_units(u2_invalid_test_units(), false);
        runtime.queue_integration("pf-test", "U2");
        runtime.maybe_integrate_one();

        assert!(runtime.pending_integrations.is_empty());
        assert_eq!(runtime.merge_queue.len(), 1);
        let event = &runtime.merge_queue.front().expect("queued").event;
        assert_eq!(event.topic.as_str(), "forge.unit.execution_failed");
        assert!(event.payload.contains("integration_gate_invalid"));
    }

    /// Duplicate queue requests for the same unit collapse to one
    /// pending entry.
    #[test]
    fn duplicate_queue_requests_dedup() {
        let (_tmp, mut runtime) = git_fixture(PLAN_ARTIFACT, false);
        runtime.queue_integration("pf-test", "U2");
        runtime.queue_integration("pf-test", "U2");
        assert_eq!(runtime.pending_integrations.len(), 1);
    }

    /// A verified unit whose executor branch does not exist fails
    /// closed — the agent payload can never substitute for git.
    #[test]
    fn missing_unit_branch_fails_closed() {
        let (_tmp, mut runtime) = git_fixture(PLAN_ARTIFACT, false);
        runtime.queue_integration("pf-test", "U2");
        runtime.maybe_integrate_one();

        assert_eq!(runtime.merge_queue.len(), 1);
        let event = &runtime.merge_queue.front().expect("queued").event;
        assert!(event.payload.contains("integration_missing_unit_commit"));
    }

    /// Happy path: real lane FF + durable record + coord event;
    /// the accepted read-back acks the record and admits the unit.
    #[test]
    fn verified_unit_integrates_and_ack_unlocks() {
        let (tmp, mut runtime) = git_fixture(PLAN_ARTIFACT, true);
        runtime.queue_integration("pf-test", "U1");
        runtime.maybe_integrate_one();

        assert!(
            runtime.merge_queue.is_empty(),
            "no failure: {:?}",
            runtime.merge_queue.front().map(|p| p.event.payload.clone())
        );
        assert_eq!(
            ledger_contains(tmp.path(), "forge.unit.integrated"),
            1,
            "one coord event appended"
        );
        let store = runtime
            .journal()
            .expect("durable journal")
            .shared_with_integration();
        let records = store.list_for_unit("U1").expect("list");
        assert_eq!(records.len(), 1);
        assert!(!records[0].acked, "record stays unacked until read-back");

        // Repeated ticks do not re-integrate: the unit left the
        // pending queue and the durable record is idempotent.
        runtime.maybe_integrate_one();
        assert_eq!(store.list_for_unit("U1").expect("list").len(), 1);
        assert_eq!(ledger_contains(tmp.path(), "forge.unit.integrated"), 1);

        runtime.on_unit_integrated_accepted(&integrated_payload("U1"));
        assert!(
            runtime
                .plans
                .get("pf-test")
                .expect("plan")
                .integrated
                .contains("U1")
        );
        assert!(store.list_for_unit("U1").expect("list")[0].acked);
    }

    /// Crash window: the durable integration record was written, but the
    /// coordination event was not. Recovery re-emits the event and leaves the
    /// record unacked until the real acceptance projection runs.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn recovery_reemits_unacked_integration_without_prior_event() {
        let (tmp, mut runtime) = git_fixture(PLAN_ARTIFACT, false);
        let target_branch = integration_branch("loop-test");
        let store = runtime
            .journal()
            .expect("durable journal")
            .shared_with_integration();
        store
            .record_integrated(&ralph_core::supervisor::dag_integration::IntegrationInput {
                unit_id: "U1".to_string(),
                target_branch,
                base_commit: "base".to_string(),
                integrated_commit: "integrated".to_string(),
                expected_head_before: "base".to_string(),
                created_at_ms: 1,
            })
            .expect("record integration");

        runtime.reconcile_after_restart();

        assert_eq!(store.list_for_unit("U1").expect("list").len(), 1);
        assert!(runtime.blocked_plans.is_empty());
        assert_eq!(ledger_contains(tmp.path(), UNIT_INTEGRATED), 1);
        assert!(!store.list_for_unit("U1").expect("list")[0].acked);
    }

    /// Crash window: the integration event reached the trusted ledger, but
    /// the task projection/ack did not. Recovery must block rather than ack a
    /// possibly incomplete projection or emit a duplicate event.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn recovery_blocks_event_present_but_unacked_integration() {
        let (tmp, mut runtime) = git_fixture(PLAN_ARTIFACT, false);
        let target_branch = integration_branch("loop-test");
        let store = runtime
            .journal()
            .expect("durable journal")
            .shared_with_integration();
        store
            .record_integrated(&ralph_core::supervisor::dag_integration::IntegrationInput {
                unit_id: "U1".to_string(),
                target_branch,
                base_commit: "base".to_string(),
                integrated_commit: "integrated".to_string(),
                expected_head_before: "base".to_string(),
                created_at_ms: 1,
            })
            .expect("record integration");
        append_supervisor_coord_event(
            &tmp.path().join("events.jsonl"),
            UNIT_INTEGRATED,
            &serde_json::json!({
                "plan_key": "pf-test",
                "unit_id": "U1",
                "task_key": "forge:pf-test:U1",
                "integrated_commit": "integrated",
            }),
        )
        .expect("append integration event");

        runtime.reconcile_after_restart();

        assert_eq!(store.list_for_unit("U1").expect("list").len(), 1);
        assert!(runtime.blocked_plans.contains("pf-test"));
        assert_eq!(ledger_contains(tmp.path(), UNIT_INTEGRATED), 1);
        assert!(!store.list_for_unit("U1").expect("list")[0].acked);
    }

    /// Crash window: the durable acknowledgement committed, but the trusted
    /// coordination event is absent from the ledger. Recovery must not treat
    /// the receipt as proof that the event was accepted; it blocks the plan
    /// before a terminal event can be emitted.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn recovery_blocks_acked_integration_without_ledger_event() {
        let (tmp, mut first) = git_fixture(PLAN_ARTIFACT, false);
        let target_branch = integration_branch("loop-test");
        let store = first
            .journal()
            .expect("durable journal")
            .shared_with_integration();
        store
            .record_integrated(&ralph_core::supervisor::dag_integration::IntegrationInput {
                unit_id: "U1".to_string(),
                target_branch: target_branch.clone(),
                base_commit: "base".to_string(),
                integrated_commit: "integrated".to_string(),
                expected_head_before: "base".to_string(),
                created_at_ms: 1,
            })
            .expect("record integration");
        store.ack("U1", &target_branch).expect("ack integration");
        drop(first);

        let mut recovered = DagSchedulerRuntime::new(
            SchedulerMode::Dag,
            ResolvedDagPools {
                global: 4,
                executor: 2,
                reviewer: 2,
                verifier: 2,
                fixer: 2,
            },
            tmp.path().to_path_buf(),
        );
        recovered.attach_execution_context(exec_context(tmp.path()));
        recovered.recover_after_restart();
        recovered.maybe_emit_development_done();

        assert!(recovered.blocked_plans.contains("pf-test"));
        assert_eq!(ledger_contains(tmp.path(), UNIT_INTEGRATED), 0);
        assert_eq!(ledger_contains(tmp.path(), DEVELOPMENT_DONE), 0);
    }

    /// Crash-window combination the test above misses: EVERY unit of the
    /// plan holds a durable ack (so recovery restores all of them into
    /// `plan.integrated`) while the trusted ledger events are missing (so
    /// reconcile blocks the plan). A blocked plan must never emit
    /// `forge.exec.development.done` even when it looks fully integrated.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn blocked_plan_with_full_acks_never_emits_development_done() {
        let (tmp, mut first) = git_fixture(PLAN_ARTIFACT, false);
        let target_branch = integration_branch("loop-test");
        let store = first
            .journal()
            .expect("durable journal")
            .shared_with_integration();
        for unit_id in ["U1", "U2"] {
            store
                .record_integrated(&ralph_core::supervisor::dag_integration::IntegrationInput {
                    unit_id: unit_id.to_string(),
                    target_branch: target_branch.clone(),
                    base_commit: "base".to_string(),
                    integrated_commit: "integrated".to_string(),
                    expected_head_before: "base".to_string(),
                    created_at_ms: 1,
                })
                .expect("record integration");
            store.ack(unit_id, &target_branch).expect("ack integration");
        }
        drop(first);

        let mut recovered = DagSchedulerRuntime::new(
            SchedulerMode::Dag,
            ResolvedDagPools {
                global: 4,
                executor: 2,
                reviewer: 2,
                verifier: 2,
                fixer: 2,
            },
            tmp.path().to_path_buf(),
        );
        recovered.attach_execution_context(exec_context(tmp.path()));
        recovered.recover_after_restart();

        assert!(recovered.blocked_plans.contains("pf-test"));
        assert_eq!(
            recovered
                .plans
                .get("pf-test")
                .expect("plan")
                .integrated
                .len(),
            2,
            "both acked units are restored into plan.integrated"
        );
        recovered.maybe_emit_development_done();
        assert_eq!(
            ledger_contains(tmp.path(), DEVELOPMENT_DONE),
            0,
            "a blocked plan must never emit forge.exec.development.done"
        );
    }

    /// Crash window: the terminal fence was committed, but the final event
    /// append was interrupted. Recovery must block rather than re-emit after
    /// the fence has already made the side effect irreversible.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn recovery_blocks_terminal_fence_without_ledger_event() {
        let (tmp, mut runtime) = git_fixture(PLAN_ARTIFACT, false);
        runtime
            .journal()
            .expect("durable journal")
            .try_record_terminal_emit("pf-test", DEVELOPMENT_DONE, "development-done-key", 1)
            .expect("record terminal fence");

        runtime.reconcile_after_restart();

        assert!(runtime.blocked_plans.contains("pf-test"));
        assert_eq!(ledger_contains(tmp.path(), DEVELOPMENT_DONE), 0);
    }

    /// An accepted integrated event cannot ack a different Unit merely by
    /// reusing a valid plan key; the projection identity is plan-topology
    /// scoped as well.
    #[test]
    fn integrated_ack_for_unknown_unit_is_ignored() {
        let (_tmp, mut runtime) = git_fixture(PLAN_ARTIFACT, false);
        runtime.on_unit_integrated_accepted(&serde_json::json!({
            "plan_key": "pf-test",
            "unit_id": "U-not-in-plan",
            "integrated_commit": "integrated",
        }));

        assert!(
            !runtime
                .plans
                .get("pf-test")
                .expect("plan")
                .integrated
                .contains("U-not-in-plan")
        );
    }

    /// S15: development.done waits for every unit's ack, then emits
    /// exactly once; the durable fence makes replays no-ops.
    #[test]
    fn development_done_waits_for_acks_then_emits_exactly_once() {
        let (tmp, mut runtime) = git_fixture(PLAN_ARTIFACT, true);
        runtime.queue_integration("pf-test", "U1");
        runtime.maybe_integrate_one();
        assert_eq!(ledger_contains(tmp.path(), "forge.unit.integrated"), 1);

        // U1 unacked (and U2 not integrated): no terminal emit.
        runtime.maybe_emit_development_done();
        assert_eq!(
            ledger_contains(tmp.path(), "forge.exec.development.done"),
            0
        );

        // Ack U1, simulate U2 integrated+acked.
        runtime.on_unit_integrated_accepted(&integrated_payload("U1"));
        runtime
            .plans
            .get_mut("pf-test")
            .expect("plan")
            .integrated
            .insert("U2".to_string());
        runtime.maybe_emit_development_done();
        assert_eq!(
            ledger_contains(tmp.path(), "forge.exec.development.done"),
            1
        );

        // Replay: the fence row makes every later pass a no-op.
        runtime.maybe_emit_development_done();
        runtime.maybe_emit_development_done();
        assert_eq!(
            ledger_contains(tmp.path(), "forge.exec.development.done"),
            1,
            "terminal event emitted exactly once"
        );
    }

    /// A second runtime rebuilt from the same durable store must not repeat
    /// either the real Git integration event or the final completion event.
    #[cfg(feature = "supervisor-db")]
    #[test]
    fn restart_keeps_integration_and_development_done_counts_exactly_once() {
        let (tmp, mut first) = git_fixture(PLAN_ARTIFACT, true);
        first.attach_execution_context(exec_context(tmp.path()));
        let base = ralph_core::get_head_sha(tmp.path()).expect("approval-time Git base");
        git(tmp.path(), &["checkout", "-b", "ralph/loop-test/U2"]);
        std::fs::write(tmp.path().join("u2.txt"), "u2\n").expect("write u2 work");
        git(tmp.path(), &["add", "u2.txt"]);
        git(tmp.path(), &["commit", "-m", "u2 work"]);
        git(tmp.path(), &["checkout", "main"]);
        first
            .plans
            .get_mut("pf-test")
            .expect("fixture plan")
            .verified_base_commit = Some(base.clone());
        first
            .journal()
            .expect("durable journal")
            .record_verified_base("pf-test", &base, 1)
            .expect("persist verified base");
        first.queue_integration("pf-test", "U1");
        first.maybe_integrate_one();
        assert_eq!(
            ledger_contains(tmp.path(), UNIT_INTEGRATED),
            1,
            "real integration must append exactly one event"
        );
        first.on_unit_integrated_accepted(&integrated_payload("U1"));
        first.queue_integration("pf-test", "U2");
        first.maybe_integrate_one();
        assert_eq!(ledger_contains(tmp.path(), UNIT_INTEGRATED), 2);
        first.on_unit_integrated_accepted(&integrated_payload("U2"));
        first.maybe_emit_development_done();
        assert_eq!(ledger_contains(tmp.path(), DEVELOPMENT_DONE), 1);
        drop(first);

        let mut recovered = DagSchedulerRuntime::new(
            SchedulerMode::Dag,
            ResolvedDagPools {
                global: 4,
                executor: 2,
                reviewer: 2,
                verifier: 2,
                fixer: 2,
            },
            tmp.path().to_path_buf(),
        );
        recovered.attach_execution_context(exec_context(tmp.path()));
        recovered.recover_after_restart();
        recovered.maybe_emit_development_done();

        assert!(recovered.blocked_plans.is_empty());
        assert_eq!(ledger_contains(tmp.path(), UNIT_INTEGRATED), 2);
        assert_eq!(ledger_contains(tmp.path(), DEVELOPMENT_DONE), 1);
    }

    #[test]
    fn gate_command_parser_preserves_quoted_arguments() {
        assert_eq!(
            parse_gate_command("cargo nextest run --package 'ralph core' -- 'test name'")
                .expect("quoted command parses"),
            (
                "cargo".to_string(),
                vec![
                    "nextest".to_string(),
                    "run".to_string(),
                    "--package".to_string(),
                    "ralph core".to_string(),
                    "--".to_string(),
                    "test name".to_string(),
                ]
            )
        );
        assert_eq!(
            parse_gate_command(r#"tool "path with spaces" escaped\ value"#)
                .expect("double quotes and escapes parse"),
            (
                "tool".to_string(),
                vec!["path with spaces".to_string(), "escaped value".to_string()]
            )
        );
    }

    #[test]
    fn gate_command_parser_rejects_unterminated_quotes_or_escapes() {
        assert!(parse_gate_command("cargo test '").is_none());
        assert!(parse_gate_command("cargo test \\").is_none());
        assert!(parse_gate_command("   ").is_none());
    }
}
