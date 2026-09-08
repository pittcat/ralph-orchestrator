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
use ralph_core::supervisor::integration_lane::{GateCommandSpec, LaneError};
use ralph_core::supervisor::dag_store_rusqlite::RusqliteIntegrationStore;
use serde_json::Value;
use sha2::Digest as _;
use tracing::{debug, warn};

use super::integration::{IntegrationError, IntegrationOutcome, IntegrationRequest, real_orchestrator};
use super::spawn::SpawnKind;
use super::{DagSchedulerRuntime, now_ms};
use crate::loop_runner::wave::dispatcher::coordination::append_supervisor_coord_event;

/// Runtime-emitted coordination topics. No hat publishes them; the
/// schema marks both as runtime-sourced.
pub(crate) const UNIT_INTEGRATED: &str = "forge.unit.integrated";
pub(crate) const DEVELOPMENT_DONE: &str = "forge.exec.development.done";

/// CAS-stale retries before the unit is failed: a moving target head
/// is transient (sibling FF), a persistent one is an operator race.
const MAX_STALE_RETRIES: u32 = 3;

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
        let Some(main_events_file) = self.exec.as_ref().map(|exec| exec.main_events_file.clone()) else {
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
                    plan.target_branch.clone(),
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
                let Some(record) = records
                    .into_iter()
                    .find(|record| record.target_branch == target_branch && !record.acked)
                else {
                    continue;
                };
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
                if let Err(err) = append_supervisor_coord_event(
                    &main_events_file,
                    UNIT_INTEGRATED,
                    &payload,
                ) {
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
                                (payload.get("plan_key").and_then(Value::as_str)
                                    == Some(&plan_key))
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
    pub(crate) fn queue_integration(&mut self, plan_key: &str, unit_id: &str) {
        let already_pending = self
            .pending_integrations
            .iter()
            .any(|p| p.plan_key == plan_key && p.unit_id == unit_id);
        let already_integrated = self
            .plans
            .get(plan_key)
            .is_some_and(|plan| plan.integrated.contains(unit_id));
        if already_pending || already_integrated {
            debug!(plan_key, unit_id, "DAG integration: duplicate queue request ignored");
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
            .min_by(|(_, a), (_, b)| {
                (order_of(a), &a.unit_id).cmp(&(order_of(b), &b.unit_id))
            })
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
        let unit_id = payload.get("unit_id").and_then(Value::as_str).unwrap_or_default();
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
        let target_branch = plan.target_branch.clone();
        let Some(store) = self.integration_store() else {
            // Non-durable builds never reach here (spawn refuses
            // without a journal); stay fail-closed anyway.
            warn!(unit_id, "DAG integration: no durable store; unit stays unacked");
            return;
        };
        if let Err(err) = store.ack(unit_id, &target_branch) {
            warn!(
                unit_id,
                target_branch,
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
            .filter(|(_, plan)| {
                !plan.units.is_empty()
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
            match journal.try_record_terminal_emit(
                &plan_key,
                DEVELOPMENT_DONE,
                &fence_key,
                now_ms() as i64,
            ) {
                Ok(true) => {
                    let payload = serde_json::json!({
                        "execution_plan_path": artifact_path,
                        "completed_unit_count": unit_count,
                        "failed_unit_count": 0,
                        "plan_key": plan_key,
                    });
                    let main_events_file = self
                        .exec
                        .as_ref()
                        .expect("checked")
                        .main_events_file
                        .clone();
                    if let Err(err) =
                        append_supervisor_coord_event(&main_events_file, DEVELOPMENT_DONE, &payload)
                    {
                        // The fence row already records the emit
                        // permit; a lost append is a recovery (E3)
                        // concern, not a re-emit license.
                        warn!(
                            plan_key,
                            error = %err,
                            "DAG integration: development.done append failed after fence win"
                        );
                    }
                }
                Ok(false) => {
                    debug!(plan_key, "DAG integration: development.done already fenced; replay is a no-op");
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
        self.journal().map(|journal| journal.shared_with_integration())
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
            ))
        });
        let Some((target_branch, base_commit, tests)) = lookup
        else {
            self.fail_integration(
                &pending,
                "integration_unavailable",
                "plan topology or verified base commit missing",
            );
            return;
        };
        let integration_order = self
            .plans
            .get(&plan_key)
            .and_then(|plan| plan.units.iter().find(|u| u.unit_id == unit_id))
            .map(|u| u.integration_order)
            .unwrap_or(u32::MAX);

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

        let loop_id = self
            .exec
            .as_ref()
            .expect("checked by caller")
            .loop_id
            .clone();
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
        // The artifact declares no path policy, so the guard's
        // allowlist and the declared set both degenerate to the
        // actual changed paths: the second authorisation still
        // rejects forbidden top-level prefixes, symlinks, and
        // submodules, but cannot enforce a per-unit scope the plan
        // never declared.
        let paths: Vec<std::path::PathBuf> =
            changed_paths.iter().map(|e| e.path.clone()).collect();

        let request = IntegrationRequest {
            unit_id: unit_id.clone(),
            integration_order,
            target_branch: target_branch.clone(),
            base_commit,
            unit_commit,
            changed_paths,
            allowlist: paths.clone(),
            declared_paths: paths,
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
        if let Err(err) = append_supervisor_coord_event(&main_events_file, UNIT_INTEGRATED, &payload)
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
    tests:
      - "true"
  - id: U2
    title: Feature
    depends_on: []
    execution_wave: 1
    integration_order: 2
    target_branch: feat/u2-feature
    tests:
      - "true"
"#;
    /// U2 declares no gate commands (PMI-0041 fail-closed probe).
    const PLAN_ARTIFACT_U2_NO_TESTS: &str = r#"version: 1
plan_key: pf-test
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1-foundation
    tests:
      - "true"
  - id: U2
    title: Feature
    depends_on: []
    execution_wave: 1
    integration_order: 2
    target_branch: feat/u2-feature
"#;

    /// U2 has one valid-looking gate and one malformed entry. The malformed
    /// entry must not be silently discarded while the valid gate runs.
    const PLAN_ARTIFACT_U2_INVALID_TEST: &str = r#"version: 1
plan_key: pf-test
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1-foundation
    tests:
      - "true"
  - id: U2
    title: Feature
    depends_on: []
    execution_wave: 1
    integration_order: 2
    target_branch: feat/u2-feature
    tests:
      - "true"
      - "cargo test 'unterminated"
"#;

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

    /// Git workspace on `main` with a base commit; optionally a unit
    /// branch `ralph/loop-test/U1` carrying one business commit.
    /// Activation captures the base as `verified_base_commit`.
    fn git_fixture(artifact_text: &str, with_u1_branch: bool) -> (TempDir, DagSchedulerRuntime) {
        let tmp = TempDir::new().expect("temp workspace");
        let ws = tmp.path();
        git(ws, &["init", "-b", "main"]);
        git(ws, &["config", "user.email", "test@example.com"]);
        git(ws, &["config", "user.name", "test"]);
        std::fs::write(ws.join("base.txt"), "base\n").expect("write base");
        git(ws, &["add", "base.txt"]);
        git(ws, &["commit", "-m", "base"]);
        write_artifact(ws, artifact_text);
        let mut runtime = runtime_with_plan(ws);
        runtime.attach_execution_context(exec_context(ws));
        if with_u1_branch {
            git(ws, &["checkout", "-b", "ralph/loop-test/U1"]);
            std::fs::write(ws.join("u1.txt"), "u1\n").expect("write u1");
            git(ws, &["add", "u1.txt"]);
            git(ws, &["commit", "-m", "u1 work"]);
            git(ws, &["checkout", "main"]);
        }
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
        let (_tmp, mut runtime) = git_fixture(PLAN_ARTIFACT_U2_NO_TESTS, false);
        runtime.queue_integration("pf-test", "U2");
        runtime.maybe_integrate_one();

        assert!(runtime.pending_integrations.is_empty());
        assert_eq!(runtime.merge_queue.len(), 1);
        let event = &runtime.merge_queue.front().expect("queued").event;
        assert_eq!(event.topic.as_str(), "forge.unit.execution_failed");
        assert!(event.payload.contains("integration_gate_unconfigured"));
    }

    /// A malformed gate entry fails closed even when another gate entry is
    /// valid; silently dropping it would weaken the declared integration
    /// contract.
    #[test]
    fn malformed_gate_entry_fails_closed() {
        let (_tmp, mut runtime) = git_fixture(PLAN_ARTIFACT_U2_INVALID_TEST, false);
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
        let target_branch = runtime
            .plans
            .get("pf-test")
            .expect("plan")
            .target_branch
            .clone();
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
        let target_branch = runtime
            .plans
            .get("pf-test")
            .expect("plan")
            .target_branch
            .clone();
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
            .try_record_terminal_emit(
                "pf-test",
                DEVELOPMENT_DONE,
                "development-done-key",
                1,
            )
            .expect("record terminal fence");

        runtime.reconcile_after_restart();

        assert!(runtime.blocked_plans.contains("pf-test"));
        assert_eq!(ledger_contains(tmp.path(), DEVELOPMENT_DONE), 0);
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
        assert_eq!(ledger_contains(tmp.path(), "forge.exec.development.done"), 0);

        // Ack U1, simulate U2 integrated+acked.
        runtime.on_unit_integrated_accepted(&integrated_payload("U1"));
        runtime
            .plans
            .get_mut("pf-test")
            .expect("plan")
            .integrated
            .insert("U2".to_string());
        runtime.maybe_emit_development_done();
        assert_eq!(ledger_contains(tmp.path(), "forge.exec.development.done"), 1);

        // Replay: the fence row makes every later pass a no-op.
        runtime.maybe_emit_development_done();
        runtime.maybe_emit_development_done();
        assert_eq!(
            ledger_contains(tmp.path(), "forge.exec.development.done"),
            1,
            "terminal event emitted exactly once"
        );
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
