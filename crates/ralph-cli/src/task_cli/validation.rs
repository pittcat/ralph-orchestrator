use crate::{
    ConfigSource, config_resolution, display::colors, hat_command_policy::HatCommandPolicy,
    operation_guard::OperationContext, resolve_path_from_workspace, resolve_workspace_root,
};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use ralph_core::{ConfirmMatch, ConfirmationState, Task, TaskConfirmation, TaskStatus, TaskStore};
use std::path::{Path, PathBuf};

use super::args::*;
/// Gets the tasks file path.
pub(crate) fn get_tasks_path(root: Option<&PathBuf>) -> PathBuf {
    resolve_path_from_workspace(".ralph/agent/tasks.jsonl", root)
}

#[cfg(test)]
pub(crate) fn read_current_loop_id(root: Option<&PathBuf>) -> Option<String> {
    operation_context_for(root).current_loop_id
}

pub(crate) fn operation_context_for(root: Option<&PathBuf>) -> OperationContext {
    OperationContext::detect(resolve_workspace_root(root))
}

/// Reject empty or whitespace-only task ids early with a clear error.
///
/// All `ralph tools task` subcommands that take a task id as input call
/// this guard before touching the store. This prevents the agent from
/// accidentally passing `""` (e.g. copied from an empty `work.ready`)
/// and getting the misleading "Task not found" message.
pub(crate) fn validate_task_id(id: &str) -> Result<()> {
    if id.trim().is_empty() {
        bail!("task_id cannot be empty");
    }
    Ok(())
}

/// Authorize a lifecycle mutation on `task` from the given context.
///
/// Returns `Ok(())` when the caller may mutate, `Err` with a clear
/// message otherwise. In agent context, the caller must own the task
/// or be listed as a coordinator hat. In human context, only an
/// out-of-loop warning is printed (no error).
pub(crate) fn authorize_lifecycle(
    task: &Task,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    operation: &str,
) -> Result<()> {
    if !ctx.is_agent_context {
        if let (Some(current), Some(target)) = (ctx.current_loop_id.as_ref(), task.loop_id.as_ref())
            && current != target
        {
            eprintln!(
                "warning: {operation} targets task in loop '{target}' but current loop is '{current}' (human CLI bypass)"
            );
        }
        return Ok(());
    }

    if ctx.current_loop_id.is_none() {
        bail!(
            "{operation}: agent context requires a current loop marker (set .ralph/current-loop-id)"
        );
    }
    if let (Some(current), Some(target)) = (ctx.current_loop_id.as_ref(), task.loop_id.as_ref()) {
        if current != target {
            bail!(
                "{operation}: task {tid} belongs to loop '{target}' but current loop is '{current}'",
                tid = task.id
            );
        }
    } else {
        bail!(
            "{operation}: legacy task {tid} has no loop_id; not mutable from agent context",
            tid = task.id
        );
    }

    let caller_hat = ctx.current_hat_id.as_deref().ok_or_else(|| {
        anyhow::anyhow!("{operation}: agent context requires a current hat (set RALPH_CURRENT_HAT)")
    })?;

    let capability = ralph_core::execution_contract::evaluate_task_capability(
        task,
        Some(caller_hat),
        ctx.current_loop_id.as_deref(),
        coordinator_hats,
    );
    if operation == "start" {
        if capability.actionable_now {
            return Ok(());
        }
        bail!(
            "start: capability denied for task {task_id}: constraint={} (caller_hat={caller_hat}, owner_hat={owner})",
            capability.deny_reason.unwrap_or("not_actionable"),
            task_id = task.id,
            owner = task.owner_hat_id.as_deref().unwrap_or("none"),
        );
    }
    if capability.lifecycle_administration {
        return Ok(());
    }
    bail!(ralph_core::task::task_lifecycle_denied_message(
        task,
        caller_hat,
        coordinator_hats,
        operation,
    ))
}

/// U7 (2026-07-04-003 plan): canonicalize a task mutation payload
/// for the verify-gate fingerprint.
///
/// The same canonical string MUST be produced by both the verify
/// path and the apply path so a `verify` followed by an `add` /
/// `ensure` of the *same* intent matches. Fields that do not
/// affect the written task (format, blocked_by parsed into
/// individual ids, etc.) are normalized to a stable shape.
///
/// Returned as a `String` (the same input the gate's
/// `mutation_fingerprint` expects). The schema is intentionally
/// a small hand-rolled JSON object so it does not depend on the
/// Task struct's serde representation (which can drift).
pub(crate) fn canonical_add_payload(args: &AddArgs) -> String {
    let mut blockers: Vec<&str> = Vec::new();
    if let Some(b) = args.blocked_by.as_deref() {
        for piece in b.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            blockers.push(piece);
        }
    }
    blockers.sort_unstable();
    serde_json::json!({
        "verb": "add",
        "title": args.title,
        "priority": args.priority,
        "description": args.description,
        "blocked_by": blockers,
    })
    .to_string()
}

pub(crate) fn canonical_ensure_payload(args: &EnsureArgs, derived_key: Option<&str>) -> String {
    let key = derived_key
        .map(str::to_string)
        .or_else(|| args.key.clone())
        .unwrap_or_default();
    let mut blockers: Vec<&str> = Vec::new();
    if let Some(b) = args.blocked_by.as_deref() {
        for piece in b.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            blockers.push(piece);
        }
    }
    blockers.sort_unstable();
    serde_json::json!({
        "verb": "ensure",
        "key": key,
        "title": args.title,
        "priority": args.priority,
        "description": args.description,
        "blocked_by": blockers,
    })
    .to_string()
}

/// Compute the loop_id/hat_id tuple used by the gate fingerprint.
/// Empty strings are substituted when the field is missing so the
/// fingerprint is stable across verify → apply.
pub(crate) fn gate_identifiers(ctx: &OperationContext) -> (&str, &str) {
    (
        ctx.current_loop_id.as_deref().unwrap_or(""),
        ctx.current_hat_id.as_deref().unwrap_or(""),
    )
}

/// Stable error for a cross-scope confirmation overwrite attempt
/// (Unit 1 follow-up): the target row still carries a `pending`
/// confirmation recorded by a different loop/hat, so minting over it
/// would silently release the recorder's confirm obligation
/// (fail-open). Only the recording loop/hat may clear the record via
/// `ralph tools task confirm`; overwriting a `confirmed` record or a
/// same-scope pending record is not affected.
pub(crate) fn confirmation_scope_conflict(
    verb: &str,
    task_id: &str,
    recorded_loop: &str,
    recorded_hat: &str,
) -> anyhow::Error {
    anyhow::anyhow!(
        "task {verb} rejected: confirmation_scope_conflict — task '{task_id}' still carries \
         a pending confirmation recorded by loop '{recorded_loop}' hat '{recorded_hat}'. \
         Only the recording loop/hat may clear it: run \
         `ralph tools task confirm {task_id} --reference <reference> --digest <digest>` \
         from that loop/hat first (if the Apply output that recorded it is no longer in the \
         current context, read `confirmation.reference` / `confirmation.digest` via \
         `ralph tools task show {task_id} --format json`), then retry this mutation."
    )
}

/// Compute the canonical fingerprint for a pending mutation and
/// claim the matching verify ticket. Encapsulates the (verb,
/// canonical_payload, loop_id, hat_id) → fingerprint pipeline so
/// `execute_add` / `execute_ensure` can call this with a single
/// line and so tests can call it directly without going through
/// the unsafe `set_var` env path.
///
/// U1 (2026-08-03-001-fix-opac-high-confidence-gates-plan): the
/// gate claims without consuming. The caller MUST settle the
/// claim with [`settle_gate_claim`] after the store mutation:
/// only a successful Apply consumes the ticket; a failed Apply
/// restores it so the agent can retry without a fresh verify.
pub(crate) fn verify_gate_claim(
    workspace: &std::path::Path,
    config: &ralph_core::config::RalphConfig,
    ctx: &OperationContext,
    verb: &str,
    canonical_payload: &str,
) -> anyhow::Result<()> {
    let (loop_id, hat_id) = gate_identifiers(ctx);
    let fingerprint =
        crate::task_verify_gate::mutation_fingerprint(verb, canonical_payload, loop_id, hat_id);
    let path = crate::task_verify_gate::scoped_ticket_path(
        workspace,
        verb,
        canonical_payload,
        loop_id,
        hat_id,
    );
    crate::task_verify_gate::try_claim_matching_ticket(
        &path,
        &config.tasks,
        ctx,
        verb,
        &fingerprint,
    )
}

/// Settle a claimed verify ticket after the Apply-side mutation
/// (U1: "只有成功 Apply 才 consume,Apply 前失败可 restore").
///
/// On `Ok` the claim marker is removed (one-shot ticket burned
/// after the side effect committed). On `Err` the prepared record
/// is restored from the claim marker so a corrected Apply can
/// re-claim without a fresh verify; the original mutation error is
/// returned unchanged. No-op when the gate was inactive for this
/// caller (human CLI / gate off / unsafe hatch) because no claim
/// marker was created.
pub(crate) fn settle_gate_claim(
    workspace: &std::path::Path,
    ctx: &OperationContext,
    verb: &str,
    canonical_payload: &str,
    result: anyhow::Result<()>,
) -> anyhow::Result<()> {
    let (loop_id, hat_id) = gate_identifiers(ctx);
    let path = crate::task_verify_gate::scoped_ticket_path(
        workspace,
        verb,
        canonical_payload,
        loop_id,
        hat_id,
    );
    match result {
        Ok(()) => crate::task_verify_gate::consume_claimed_ticket(&path),
        Err(err) => {
            if let Err(restore_err) = crate::task_verify_gate::restore_ticket_from_claim(&path) {
                return Err(anyhow::anyhow!(
                    "{err}\nadditionally, restoring the verify ticket for retry failed: {restore_err}"
                ));
            }
            Err(err)
        }
    }
}

/// Bridge `HatCommandPolicy::PolicyDecision` to the `anyhow::Result`
/// exit used by the rest of the task CLI.
///
/// On `Allow` we proceed silently (human warnings are not yet wired —
/// the existing `authorize_lifecycle` handles the human cross-loop
/// warning path). On `Deny` we `bail!` with a stable, machine-grepable
/// prefix that the agent can match on to recover.
pub(crate) fn enforce_command_policy(
    ctx: &OperationContext,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    config: Option<&ralph_core::config::RalphConfig>,
    verb: &str,
    is_for_fix_unit: bool,
) -> Result<()> {
    use crate::hat_command_policy::PolicyDecision;
    match HatCommandPolicy::check_task_with_config(
        ctx,
        coordinator_hats,
        coordinator_err,
        config,
        verb,
        is_for_fix_unit,
    ) {
        PolicyDecision::Allow { .. } => Ok(()),
        PolicyDecision::Deny { reason, hint } => bail!(
            "hat_command_policy denied '{verb}' for hat '{hat}': [{reason}] {hint}",
            verb = verb,
            hat = ctx.current_hat_id.as_deref().unwrap_or("<none>"),
            reason = reason,
            hint = hint,
        ),
    }
}

pub(crate) fn add_common_task_fields(
    mut task: Task,
    ctx: &OperationContext,
    description: Option<String>,
    blocked_by: Option<String>,
) -> Task {
    if let Some(loop_id) = ctx.current_loop_id.clone() {
        task = task.with_loop_id(Some(loop_id));
    }

    if let Some(hat_id) = ctx.current_hat_id.clone() {
        task = task.with_owner_hat(Some(hat_id));
    }

    if let Some(desc) = description {
        task = task.with_description(Some(desc));
    }

    if let Some(blockers) = blocked_by {
        for blocker_id in blockers
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            task = task.with_blocker(blocker_id.to_string());
        }
    }

    task
}

/// U3: enforce that a task's `owner_hat_id` is on the workspace's
/// `tasks.coordinator_hats` allowlist.
///
/// This is the create-side complement to the JSONL origin guard: even if a
/// rogue `ralph` hat somehow got into the loop, it cannot persist a task
/// to disk that is attributed to a workflow hat. Without this check the
/// stall-recovery path could silently create tasks under the wrong owner,
/// corrupting plan-gate's task correlation and the merge queue.
///
/// When `owner_hat_id` is `None` the call is human-driven (the CLI does
/// not stamp an owner when `ctx.current_hat_id` is unset) and the check
/// is skipped — humans operating the CLI must not be locked out.
///
/// When the allowlist is empty AND the task carries an owner, the call
/// is rejected (fail-closed): an empty allowlist is a misconfiguration
/// and we must not let an agent bypass owner validation by being the
/// only hat in scope.
pub(crate) fn validate_owner_hat_id(task: &Task, coordinator_hats: &[String]) -> Result<()> {
    let Some(owner) = task.owner_hat_id.as_deref() else {
        return Ok(());
    };
    if coordinator_hats.iter().any(|h| h == owner) {
        Ok(())
    } else {
        bail!(
            "owner_hat_id '{owner}' is not in tasks.coordinator_hats. \
             Allowed: {coordinator_hats:?}. \
             The owner is set from $RALPH_CURRENT_HAT at task creation; \
             either run the task command from a hat in coordinator_hats, \
             or add the hat to tasks.coordinator_hats in ralph.yml."
        )
    }
}

pub(crate) fn status_matches_filter(status: TaskStatus, filter: &str) -> bool {
    let normalized = filter.to_lowercase().replace(['_', '-'], "");
    match status {
        TaskStatus::Open => normalized == "open",
        TaskStatus::InProgress => normalized == "inprogress",
        TaskStatus::Closed => normalized == "closed",
        TaskStatus::Failed => normalized == "failed",
    }
}

pub(crate) fn filter_tasks_for_list(store: &TaskStore, args: &ListArgs) -> Vec<Task> {
    let mut tasks: Vec<_> = if let Some(status_str) = args.status.as_deref() {
        store
            .all()
            .iter()
            .filter(|t| status_matches_filter(t.status, status_str))
            .cloned()
            .collect()
    } else if args.all {
        store.all().to_vec()
    } else {
        store
            .all()
            .iter()
            .filter(|t| !matches!(t.status, TaskStatus::Closed | TaskStatus::Failed))
            .cloned()
            .collect()
    };

    if let Some(days) = args.days {
        let cutoff = Utc::now() - chrono::Duration::days(days);
        tasks.retain(|t| {
            if DateTime::parse_from_rfc3339(&t.created)
                .map(|c| c.with_timezone(&Utc) > cutoff)
                .unwrap_or(false)
            {
                return true;
            }

            if t.closed.as_ref().is_some_and(|closed_str| {
                DateTime::parse_from_rfc3339(closed_str)
                    .map(|c| c.with_timezone(&Utc) > cutoff)
                    .unwrap_or(false)
            }) {
                return true;
            }
            false
        });
    }

    tasks.sort_by(|a, b| {
        let status_rank = |s: TaskStatus| match s {
            TaskStatus::InProgress => 0,
            TaskStatus::Open => 1,
            TaskStatus::Closed => 2,
            TaskStatus::Failed => 3,
        };

        let rank_a = status_rank(a.status);
        let rank_b = status_rank(b.status);

        if rank_a != rank_b {
            return rank_a.cmp(&rank_b);
        }

        if a.priority != b.priority {
            return a.priority.cmp(&b.priority);
        }

        a.created.cmp(&b.created)
    });

    if let Some(limit) = args.limit {
        tasks.truncate(limit);
    }

    tasks
}

pub(crate) fn filter_tasks_for_ready(
    store: &TaskStore,
    args: &ReadyArgs,
    root: Option<&PathBuf>,
) -> Vec<Task> {
    let mut ready: Vec<Task> = store.ready().into_iter().cloned().collect();

    if !args.all
        && let Some(current_loop_id) =
            crate::operation_guard::OperationContext::detect(resolve_workspace_root(root))
                .current_loop_id
    {
        ready.retain(|t| t.loop_id.as_ref() == Some(&current_loop_id));
    }

    ready
}

/// Loads the full `RalphConfig` from the workspace, falling back to
/// an empty default when the file is missing or unreadable.
///
/// The fallback is intentionally silent because the L2 CLI ACL is
/// best-effort: a human operator without a `ralph.yml` must not be
/// locked out of task tooling. The downstream `HatCommandPolicy`
/// reads the same allowlist (`tasks.coordinator_hats`), so missing
/// config yields an empty allowlist → fail-closed for agent add/ensure.
///
/// 2026-07-13-001 plan U3 + review #C1: pass `config_sources` so a
/// `-c custom.yml` style project config is honored before falling
/// back to the workspace `ralph.yml` / `ralph.yaml` filenames.
pub(crate) fn load_config_or_default(
    root: Option<&PathBuf>,
    config_sources: &[ConfigSource],
) -> ralph_core::config::RalphConfig {
    if let Some(path) = config_resolution::resolve_project_config_path(
        &resolve_workspace_root(root),
        config_sources,
    ) && let Ok(raw) = std::fs::read_to_string(&path)
        && let Ok(cfg) = serde_yaml::from_str::<ralph_core::config::RalphConfig>(&raw)
    {
        return cfg;
    }
    serde_yaml::from_str(
        "event_loop:
  execution_mode: isolated
",
    )
    .unwrap_or_default()
}
