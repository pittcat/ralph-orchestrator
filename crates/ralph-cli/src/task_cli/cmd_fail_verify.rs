use crate::{
    ConfigSource, display::colors, hat_command_policy::HatCommandPolicy,
    operation_guard::OperationContext, resolve_workspace_root,
};
use anyhow::{Context, Result, bail};
use ralph_core::{ConfirmMatch, ConfirmationState, Task, TaskStatus, TaskStore};
use std::path::PathBuf;

use super::args::*;
use super::validation;

pub(super) fn execute_fail(
    args: FailArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    use_colors: bool,
    _config_sources: &[ConfigSource],
) -> Result<()> {
    let path = validation::get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = validation::operation_context_for(root);
    fail_task_with_context(&mut store, &args.id, &ctx, coordinator_hats, use_colors)
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn fail_task_with_context(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    validation::validate_task_id(task_id)?;
    let snapshot = store
        .get(task_id)
        .cloned()
        .context(format!("Task {} not found", task_id))?;
    validation::authorize_lifecycle(&snapshot, ctx, coordinator_hats, "fail")?;

    let title = store
        .fail(task_id)
        .context(format!("Task {} not found", task_id))?
        .title
        .clone();

    store.save().context("Failed to save tasks")?;

    if use_colors {
        println!(
            "{}Failed task: {} - {}{}",
            colors::RED,
            task_id,
            title,
            colors::RESET
        );
    } else {
        println!("Failed task: {} - {}", task_id, title);
    }
    Ok(())
}

pub(super) fn execute_show(args: ShowArgs, root: Option<&PathBuf>, use_colors: bool) -> Result<()> {
    validation::validate_task_id(&args.id)?;
    let path = validation::get_tasks_path(root);
    let store = TaskStore::load(&path).context("Failed to load tasks")?;

    let task = store
        .get(&args.id)
        .context(format!("Task {} not found", args.id))?;

    match args.format {
        OutputFormat::Table => {
            let status_str = match task.status {
                TaskStatus::Open => "open",
                TaskStatus::InProgress => "in_progress",
                TaskStatus::Closed => "closed",
                TaskStatus::Failed => "failed",
            };

            if use_colors {
                let status_color = match task.status {
                    TaskStatus::Open => colors::GREEN,
                    TaskStatus::InProgress => colors::BLUE,
                    TaskStatus::Closed => colors::DIM,
                    TaskStatus::Failed => colors::RED,
                };
                let priority_color = match task.priority {
                    1 => colors::RED,
                    2 => colors::YELLOW,
                    _ => colors::RESET,
                };

                println!("{}ID:          {}{}", colors::DIM, task.id, colors::RESET);
                println!("Title:       {}", task.title);
                if let Some(desc) = &task.description {
                    println!("Description: {}", desc);
                }
                println!(
                    "Status:      {}{}{}",
                    status_color,
                    status_str,
                    colors::RESET
                );
                println!(
                    "Priority:    {}{}{}",
                    priority_color,
                    task.priority,
                    colors::RESET
                );
                if let Some(key) = &task.key {
                    println!("Key:         {}", key);
                }
                if let Some(loop_id) = &task.loop_id {
                    println!("Loop:        {}", loop_id);
                }
                if let Some(owner) = &task.owner_hat_id {
                    println!("Owner hat:   {}", owner);
                }
                if !task.blocked_by.is_empty() {
                    println!("Blocked by:  {}", task.blocked_by.join(", "));
                }
                println!("Created:     {}", task.created);
                if let Some(started) = &task.started {
                    println!("Started:     {}", started);
                }
                if let Some(closed) = &task.closed {
                    println!("Closed:      {}", closed);
                }
            } else {
                println!("ID:          {}", task.id);
                println!("Title:       {}", task.title);
                if let Some(desc) = &task.description {
                    println!("Description: {}", desc);
                }
                println!("Status:      {}", status_str);
                println!("Priority:    {}", task.priority);
                if let Some(key) = &task.key {
                    println!("Key:         {}", key);
                }
                if let Some(loop_id) = &task.loop_id {
                    println!("Loop:        {}", loop_id);
                }
                if let Some(owner) = &task.owner_hat_id {
                    println!("Owner hat:   {}", owner);
                }
                if !task.blocked_by.is_empty() {
                    println!("Blocked by:  {}", task.blocked_by.join(", "));
                }
                println!("Created:     {}", task.created);
                if let Some(started) = &task.started {
                    println!("Started:     {}", started);
                }
                if let Some(closed) = &task.closed {
                    println!("Closed:      {}", closed);
                }
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&task)?);
        }
        OutputFormat::Quiet => {
            println!("{}", task.id);
        }
    }

    Ok(())
}

/// Unit 1 (task confirmation): consume a pending confirmation recorded
/// by a gate-protected Apply.
///
/// Contract: `confirm` is a pure state transition — no command-policy
/// gate, no verify ticket, no event. The caller must present the exact
/// `reference` and `digest` printed by the protected Apply, from the
/// same loop/hat that applied the mutation. Wrong reference →
/// `confirmation_unavailable`; matching reference with wrong digest or
/// scope → `confirmation_mismatch` (state stays as recorded).
/// Repeating a successful confirm from the same loop/hat is idempotent
/// (exit 0, no disk rewrite); a cross-scope repeat of a confirmed
/// record is a `confirmation_mismatch`.
///
/// The decision is re-validated under the exclusive store lock: a
/// concurrent Apply may replace the row's confirmation between the
/// outer match and the lock acquisition, so the under-lock outcome —
/// not the outer decision — drives the exit code and the output.
pub(super) fn execute_confirm(args: ConfirmArgs, root: Option<&PathBuf>) -> Result<()> {
    validation::validate_task_id(&args.id)?;
    let path = validation::get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = validation::operation_context_for(root);
    let (loop_id, hat_id) = validation::gate_identifiers(&ctx);

    let decision = {
        let Some(task) = store.get(&args.id) else {
            bail!(
                "task confirm denied: confirmation_unavailable — task '{}' does not exist in the store.",
                args.id
            );
        };
        let Some(cfm) = task.confirmation.as_ref() else {
            bail!(
                "task confirm denied: confirmation_unavailable — task '{}' has no confirmation record; \
                 only gate-protected agent mutations (task add / ensure with the verify gate active) create one.",
                args.id
            );
        };
        cfm.match_confirm(&args.reference, &args.digest, loop_id, hat_id)
    };

    match decision {
        ConfirmMatch::Unavailable => bail!(
            "task confirm denied: confirmation_unavailable — reference '{}' does not match the \
             confirmation recorded for task '{}'. Re-read the Apply output (confirmation.reference) \
             and retry.",
            args.reference,
            args.id
        ),
        ConfirmMatch::Mismatch => bail!(
            "task confirm denied: confirmation_mismatch — the reference matches but the digest or the \
             loop/hat scope differs from the pending confirmation on task '{}'. Confirmations are bound \
             to the mutation fingerprint and to the loop/hat that applied it; the state stays 'pending'.",
            args.id
        ),
        ConfirmMatch::AlreadyConfirmed => {
            // Idempotent repeat: report, but never rewrite the store.
            let task = store
                .get(&args.id)
                .context(format!("Task {} not found", args.id))?;
            print_confirmed_task(task, args.format);
            Ok(())
        }
        ConfirmMatch::Apply => {
            let id = args.id.clone();
            let reference = args.reference.clone();
            let digest = args.digest.clone();
            let outcome = store
                .with_exclusive_lock(|s| {
                    // Re-validate under the lock so concurrent confirms
                    // serialize to a single transition. A concurrent
                    // Apply may have replaced the row's confirmation
                    // between the outer decision and the lock
                    // acquisition, so the under-lock outcome (not the
                    // outer decision) drives the exit code and output.
                    let Some(row) = s.get_mut(&id) else {
                        return ConfirmMatch::Unavailable;
                    };
                    let Some(cfm) = row.confirmation.as_mut() else {
                        return ConfirmMatch::Unavailable;
                    };
                    let outcome = cfm.match_confirm(&reference, &digest, loop_id, hat_id);
                    if matches!(outcome, ConfirmMatch::Apply) {
                        cfm.mark_confirmed();
                    }
                    outcome
                })
                .context("Failed to save tasks")?;
            match outcome {
                ConfirmMatch::Apply | ConfirmMatch::AlreadyConfirmed => {
                    // Apply: this call performed the transition.
                    // AlreadyConfirmed: a concurrent same-scope confirm
                    // won the race — idempotent success either way.
                    let task = store
                        .get(&args.id)
                        .context(format!("Task {} not found", args.id))?;
                    print_confirmed_task(task, args.format);
                    Ok(())
                }
                ConfirmMatch::Unavailable => bail!(
                    "task confirm denied: confirmation_unavailable — the confirmation recorded \
                     for task '{}' changed while this confirm was being prepared (reference '{}' \
                     no longer matches). Re-read the latest Apply output (confirmation.reference) \
                     and retry.",
                    args.id,
                    args.reference
                ),
                ConfirmMatch::Mismatch => bail!(
                    "task confirm denied: confirmation_mismatch — the confirmation recorded for \
                     task '{}' changed while this confirm was being prepared (the digest or the \
                     loop/hat scope no longer matches). The recorded state stays untouched.",
                    args.id
                ),
            }
        }
    }
}

pub(crate) fn print_confirmed_task(task: &Task, format: OutputFormat) {
    match format {
        OutputFormat::Table => {
            println!("Confirmed task {}", task.id);
            if let Some(cfm) = task.confirmation.as_ref() {
                println!("  Reference: {}", cfm.reference);
                // Read the actual confirmation state — never hardcode
                // the printed state (the caller drives the success
                // semantics; the row carries the source of truth).
                let state = match cfm.state {
                    ConfirmationState::Pending => "pending",
                    ConfirmationState::Confirmed => "confirmed",
                };
                println!("  State: {state}");
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string(task).expect("task serializes"));
        }
        OutputFormat::Quiet => {
            println!("{}", task.id);
        }
    }
}

pub(super) fn execute_reopen(
    args: ReopenArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    use_colors: bool,
    _config_sources: &[ConfigSource],
) -> Result<()> {
    let path = validation::get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = validation::operation_context_for(root);
    reopen_task_with_context(&mut store, &args.id, &ctx, coordinator_hats, use_colors)
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn reopen_task_with_context(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    validation::validate_task_id(task_id)?;
    let snapshot = store
        .get(task_id)
        .cloned()
        .context(format!("Task {} not found", task_id))?;
    validation::authorize_lifecycle(&snapshot, ctx, coordinator_hats, "reopen")?;

    let reopened = store
        .with_exclusive_lock(|s| s.reopen(task_id).cloned())
        .context("Failed to save tasks")?
        .context(format!("Task {} not found", task_id))?;

    if use_colors {
        println!(
            "{}Reopened task: {} - {}{}",
            colors::YELLOW,
            task_id,
            reopened.title,
            colors::RESET
        );
    } else {
        println!("Reopened task: {} - {}", task_id, reopened.title);
    }
    Ok(())
}

/// U4: route `task verify <verb>` to a verb-specific dry-run helper.
///
/// This is the entry point of the OPAC Precheck stage for task
/// mutations. The function never writes to `tasks.jsonl`; it only
/// exercises the same authorization gates as the real mutation so the
/// agent can deterministically observe the outcome without committing.
pub(super) fn execute_verify(
    args: VerifyArgs,
    use_colors: bool,
    config_sources: &[ConfigSource],
) -> Result<()> {
    let root = args.root.clone();
    let _config = validation::load_config_or_default(root.as_ref(), config_sources);
    let workspace = resolve_workspace_root(root.as_ref());
    let (coordinator_hats, coordinator_err) =
        match load_coordinator_hats(&workspace, config_sources) {
            Ok(hats) => (hats, None),
            Err(err) => (Vec::new(), Some(err)),
        };
    let cmd = args.command;

    let path = validation::get_tasks_path(root.as_ref());
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = validation::operation_context_for(root.as_ref());

    let outcome = match &cmd {
        VerifyCommands::Add(a) => verify_add(
            &mut store,
            &ctx,
            &coordinator_hats,
            coordinator_err.as_ref(),
            a,
            config_sources,
        )?,
        VerifyCommands::Ensure(e) => verify_ensure(
            &mut store,
            &ctx,
            &coordinator_hats,
            coordinator_err.as_ref(),
            e,
            config_sources,
        )?,
        VerifyCommands::Start(s) => verify_lifecycle(
            &store,
            &ctx,
            &coordinator_hats,
            coordinator_err.as_ref(),
            "start",
            &s.id,
        )?,
        VerifyCommands::Close(c) => verify_lifecycle(
            &store,
            &ctx,
            &coordinator_hats,
            coordinator_err.as_ref(),
            "close",
            &c.id,
        )?,
        VerifyCommands::Fail(f) => verify_lifecycle(
            &store,
            &ctx,
            &coordinator_hats,
            coordinator_err.as_ref(),
            "fail",
            &f.id,
        )?,
        VerifyCommands::Reopen(r) => verify_lifecycle(
            &store,
            &ctx,
            &coordinator_hats,
            coordinator_err.as_ref(),
            "reopen",
            &r.id,
        )?,
    };

    let verb = match &cmd {
        VerifyCommands::Add(_) => "add",
        VerifyCommands::Ensure(_) => "ensure",
        VerifyCommands::Start(_) => "start",
        VerifyCommands::Close(_) => "close",
        VerifyCommands::Fail(_) => "fail",
        VerifyCommands::Reopen(_) => "reopen",
    };
    match outcome {
        VerifyOutcome::Allow => {
            let format = match &cmd {
                VerifyCommands::Add(a) => a.format.format,
                VerifyCommands::Ensure(e) => e.format.format,
                _ => OutputFormat::Table,
            };
            match format {
                OutputFormat::Json => {
                    let json = serde_json::json!({
                        "verified": true,
                        "verb": verb,
                        "would_succeed": true,
                        "no_write": true,
                    });
                    println!("{}", serde_json::to_string_pretty(&json)?);
                }
                OutputFormat::Quiet => println!("ok"),
                OutputFormat::Table => {
                    let msg = VerifyOutcome::allowed_message(verb);
                    if use_colors {
                        println!(
                            "{}verified (no write):{} {msg}",
                            colors::GREEN,
                            colors::RESET
                        );
                    } else {
                        println!("verified (no write): {msg}");
                    }
                }
            }
            Ok(())
        }
        VerifyOutcome::Deny { reason, hint } => {
            let payload = serde_json::json!({
                "verified": false,
                "verb": verb,
                "would_succeed": false,
                "no_write": true,
                "reason": reason,
                "hint": hint,
                "stable_prefix": VerifyOutcome::DENY_PREFIX,
            });
            let err = anyhow::Error::msg(format!(
                "{} '{verb}': [{reason}] {hint}",
                VerifyOutcome::DENY_PREFIX,
                verb = verb,
                reason = reason,
                hint = hint,
            ));
            Err(err.context(payload.to_string()))
        }
    }
}

/// Dry-run `task add` — exercises the same gates as `add_task_with_args`
/// but returns `VerifyOutcome` instead of writing to `tasks.jsonl`.
pub(crate) fn verify_add(
    store: &mut TaskStore,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    args: &VerifyAddArgs,
    config_sources: &[ConfigSource],
) -> Result<VerifyOutcome> {
    let config = validation::load_config_or_default(Some(&ctx.workspace_root), config_sources);
    if let Err(outcome) = gate_outcome(
        ctx,
        coordinator_hats,
        coordinator_err,
        Some(&config),
        "add",
        false,
    )? {
        return Ok(outcome);
    }
    let Some(title) = args.title.clone() else {
        return Ok(VerifyOutcome::Deny {
            reason: "missing_title".to_string(),
            hint: "`task verify add` requires a positional TITLE argument (same as `task add`)."
                .to_string(),
        });
    };
    let task = validation::add_common_task_fields(
        Task::new(title, args.priority),
        ctx,
        args.description.clone(),
        args.blocked_by.clone(),
    );

    if let Err(message) = validation::validate_owner_hat_id(&task, coordinator_hats) {
        return Ok(VerifyOutcome::Deny {
            reason: "non_coordinator_owner".to_string(),
            hint: format!("{message}"),
        });
    }

    let invalid_blockers = store.invalid_blockers(&task);
    if !invalid_blockers.is_empty() {
        return Ok(VerifyOutcome::Deny {
            reason: "missing_blockers".to_string(),
            hint: format!(
                "task blocked_by references missing or out-of-loop tasks: {}",
                invalid_blockers.join(", ")
            ),
        });
    }

    // U7 (2026-07-04-003 plan): record a verify ticket so the
    // subsequent `task add` for the same payload can pass the
    // two-step gate. U2 (2026-08-03-001-fix-opac-high-confidence-gates-plan):
    // the ticket lives in a per-operation/intent/activation
    // namespace under `.ralph/agent/task-tickets/` so concurrent
    // verify add/ensure (or different loop/hat) cannot overwrite
    // each other.
    //
    // Reconstruct the canonical AddArgs shape from VerifyAddArgs
    // (they share field names) so the same `validation::canonical_add_payload`
    // helper produces an identical fingerprint on both sides.
    let real_args = AddArgs {
        title: task.title.clone(),
        priority: args.priority,
        description: args.description.clone(),
        blocked_by: args.blocked_by.clone(),
        format: OutputFormat::Quiet,
    };
    let canonical = validation::canonical_add_payload(&real_args);
    let (loop_id, hat_id) = validation::gate_identifiers(ctx);
    let fingerprint =
        crate::task_verify_gate::mutation_fingerprint("add", &canonical, loop_id, hat_id);
    let path = crate::task_verify_gate::scoped_ticket_path(
        &ctx.workspace_root,
        "add",
        &canonical,
        loop_id,
        hat_id,
    );
    let _ = crate::task_verify_gate::record_ticket(&path, &fingerprint, loop_id, hat_id);

    Ok(VerifyOutcome::Allow)
}

/// Dry-run `task ensure` — mirrors `ensure_task_with_args` but emits
/// `VerifyOutcome` instead of writing.
pub(crate) fn verify_ensure(
    store: &mut TaskStore,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    args: &VerifyEnsureArgs,
    config_sources: &[ConfigSource],
) -> Result<VerifyOutcome> {
    let config = validation::load_config_or_default(Some(&ctx.workspace_root), config_sources);
    let is_for_fix_unit = args.for_fix_unit.is_some();
    if let Err(outcome) = gate_outcome(
        ctx,
        coordinator_hats,
        coordinator_err,
        Some(&config),
        "ensure",
        is_for_fix_unit,
    )? {
        return Ok(outcome);
    }
    if args.key.is_none() && args.for_fix_unit.is_none() {
        return Ok(VerifyOutcome::Deny {
            reason: "missing_key".to_string(),
            hint: "`task verify ensure` requires either --key <KEY> or --for-fix-unit <PLAN:FIX_STEP:SLUG>.".to_string(),
        });
    }
    let Some(title) = args.title.clone() else {
        return Ok(VerifyOutcome::Deny {
            reason: "missing_title".to_string(),
            hint:
                "`task verify ensure` requires a positional TITLE argument (same as `task ensure`)."
                    .to_string(),
        });
    };

    // Mirror the derive_key logic from ensure_task_with_args.
    let derived_key = if let Some(spec) = args.for_fix_unit.as_deref() {
        let mut parts = spec.split(':');
        let plan = parts.next().unwrap_or("").to_string();
        let fix_step = parts.next().unwrap_or("").to_string();
        let slug = parts.next().unwrap_or("").to_string();
        if plan.is_empty() || fix_step.is_empty() || slug.is_empty() || parts.next().is_some() {
            return Ok(VerifyOutcome::Deny {
                reason: "malformed_for_fix_unit".to_string(),
                hint: format!(
                    "--for-fix-unit expects exactly 3 colon-separated segments: PLAN:FIX_STEP:SLUG, got '{spec}'"
                ),
            });
        }
        Some(format!("ce-executor:{plan}:{fix_step}:{slug}"))
    } else {
        None
    };
    let key_value = derived_key
        .clone()
        .unwrap_or_else(|| args.key.clone().unwrap_or_default());
    let mut task = validation::add_common_task_fields(
        Task::new(title, args.priority).with_key(Some(key_value)),
        ctx,
        args.description.clone(),
        args.blocked_by.clone(),
    );
    if args.for_fix_unit.is_some() {
        task = task.with_owner_hat(Some("coordinator".to_string()));
    }
    if let Err(message) = validation::validate_owner_hat_id(&task, coordinator_hats) {
        return Ok(VerifyOutcome::Deny {
            reason: "non_coordinator_owner".to_string(),
            hint: format!("{message}"),
        });
    }

    let invalid_blockers = store.invalid_blockers(&task);
    if !invalid_blockers.is_empty() {
        return Ok(VerifyOutcome::Deny {
            reason: "missing_blockers".to_string(),
            hint: format!(
                "task blocked_by references missing or out-of-loop tasks: {}",
                invalid_blockers.join(", ")
            ),
        });
    }

    // U7 (2026-07-04-003 plan): record a verify ticket so the
    // subsequent `task ensure` for the same payload can pass
    // the two-step gate. U2
    // (2026-08-03-001-fix-opac-high-confidence-gates-plan): the
    // ticket lives in a per-operation/intent/activation
    // namespace so `verify add` and `verify ensure` (and
    // different loop/hat) do not share the same on-disk file.
    let real_args = EnsureArgs {
        title: task.title.clone(),
        key: args.key.clone(),
        priority: args.priority,
        description: args.description.clone(),
        blocked_by: args.blocked_by.clone(),
        for_fix_unit: args.for_fix_unit.clone(),
        format: OutputFormat::Quiet,
    };
    let canonical = validation::canonical_ensure_payload(&real_args, derived_key.as_deref());
    let (loop_id, hat_id) = validation::gate_identifiers(ctx);
    let fingerprint =
        crate::task_verify_gate::mutation_fingerprint("ensure", &canonical, loop_id, hat_id);
    let path = crate::task_verify_gate::scoped_ticket_path(
        &ctx.workspace_root,
        "ensure",
        &canonical,
        loop_id,
        hat_id,
    );
    let _ = crate::task_verify_gate::record_ticket(&path, &fingerprint, loop_id, hat_id);

    Ok(VerifyOutcome::Allow)
}

/// Dry-run a lifecycle mutation (start/close/fail/reopen).
pub(crate) fn verify_lifecycle(
    store: &TaskStore,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    verb: &str,
    task_id: &str,
) -> Result<VerifyOutcome> {
    if let Err(outcome) = gate_outcome(ctx, coordinator_hats, coordinator_err, None, verb, false)? {
        return Ok(outcome);
    }
    if let Err(message) = validation::validate_task_id(task_id) {
        return Ok(VerifyOutcome::Deny {
            reason: "invalid_task_id".to_string(),
            hint: format!("{message}"),
        });
    }
    let snapshot = match store.get(task_id) {
        Some(t) => t.clone(),
        None => {
            return Ok(VerifyOutcome::Deny {
                reason: "task_not_found".to_string(),
                hint: format!("task {task_id} not found"),
            });
        }
    };
    if let Err(message) = validation::authorize_lifecycle(&snapshot, ctx, coordinator_hats, verb) {
        return Ok(VerifyOutcome::Deny {
            reason: "authorize_lifecycle_failed".to_string(),
            hint: format!("{message}"),
        });
    }
    Ok(VerifyOutcome::Allow)
}

/// Convert `HatCommandPolicy::check_task` (which returns `PolicyDecision`)
/// to the local `VerifyOutcome` so verify can keep its stable success /
/// failure exit contract instead of bailing early.
pub(crate) fn gate_outcome(
    ctx: &OperationContext,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    config: Option<&ralph_core::config::RalphConfig>,
    verb: &str,
    is_for_fix_unit: bool,
) -> Result<std::result::Result<(), VerifyOutcome>> {
    use crate::hat_command_policy::PolicyDecision;
    match HatCommandPolicy::check_task_with_config(
        ctx,
        coordinator_hats,
        coordinator_err,
        config,
        verb,
        is_for_fix_unit,
    ) {
        PolicyDecision::Allow { .. } => Ok(Ok(())),
        PolicyDecision::Deny { reason, hint } => Ok(Err(VerifyOutcome::Deny {
            reason: reason.to_string(),
            hint,
        })),
    }
}

/// Build a structured `anyhow::Error` for the three emit-bridge
/// denial paths whose only difference is the stage label, the
/// reason code, the hint text, and the underlying message.
///
/// Centralizing the JSON-shape construction keeps the three error
/// branches identical (which matters because `ralph` test agents
/// will grep the JSON payload for the `stages` array to drive
/// their recovery logic).
pub(crate) fn emit_bridge_deny(
    stage: &str,
    reason: &str,
    hint: String,
    message: String,
) -> anyhow::Error {
    let payload = serde_json::json!({
        "verified": false,
        "stages": [stage],
        "reason": reason,
        "hint": hint,
    });
    anyhow::Error::msg(message).context(payload.to_string())
}

/// U4 emit-bridge: verify three-field task_id/task_key/step consistency
/// for the upcoming `ralph emit` payload (R16). Walks the live task
/// store to confirm:
/// - `task_id` resolves to an open (non-terminal) task in the current loop.
/// - `task_key` matches the registered key on that task.
/// - `step` matches the `:step-<n>:` segment inside `task_key`
///   (per the `ralph-tools-tasks.md` red-box convention).
pub(crate) fn execute_verify_emit_bridge(
    args: VerifyEmitBridgeArgs,
    root: Option<&PathBuf>,
) -> Result<()> {
    let path = validation::get_tasks_path(root);
    let store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = validation::operation_context_for(root);

    // 1. task_id must resolve to a live, non-terminal task.
    let snapshot = store.get(&args.task_id).cloned();
    let Some(task) = snapshot else {
        return Err(emit_bridge_deny(
            "task_id_resolution",
            "task_not_found",
            format!(
                "task_id '{}' does not exist in the live task store; never hand-construct task_id — read it back via `ralph tools task list` immediately before emit.",
                args.task_id
            ),
            format!(
                "task_verify_emit_bridge: task_id '{}' not found in store (never hand-construct task_id)",
                args.task_id
            ),
        ));
    };

    if task.status.is_terminal() {
        return Err(emit_bridge_deny(
            "task_id_resolution",
            "task_is_terminal",
            format!(
                "task '{}' is in terminal state {:?}; close-then-emit is rejected. Open a fresh task or reuse an existing open one.",
                args.task_id, task.status
            ),
            format!(
                "task_verify_emit_bridge: task '{}' is in terminal state ({:?}); reuse a live task instead",
                args.task_id, task.status
            ),
        ));
    }

    if ctx.is_agent_context
        && let (Some(current), Some(target)) = (ctx.current_loop_id.as_ref(), task.loop_id.as_ref())
        && current != target
    {
        return Err(emit_bridge_deny(
            "loop_scope",
            "wrong_loop",
            format!(
                "task '{}' belongs to loop '{}' but caller is in loop '{}'; open or pick a task from the current loop",
                args.task_id, target, current
            ),
            format!(
                "task_verify_emit_bridge: task '{}' belongs to loop '{}' but current loop is '{}'",
                args.task_id, target, current
            ),
        ));
    }

    // 2. task_key must match the registered key on the task.
    let Some(registered_key) = task.key.clone() else {
        return Err(emit_bridge_deny(
            "task_key_match",
            "task_has_no_key",
            format!(
                "task '{}' has no registered key; the emit-bridge requires a key — re-create via `ralph tools task ensure --for-fix-unit` or `--key`",
                args.task_id
            ),
            format!(
                "task_verify_emit_bridge: task '{}' has no registered key; the emit-bridge requires a key",
                args.task_id
            ),
        ));
    };

    if registered_key != args.task_key {
        let payload = serde_json::json!({
            "verified": false,
            "stages": ["task_key_match"],
            "reason": "task_key_mismatch",
            "expected_key": registered_key,
            "provided_key": args.task_key,
            "hint": "task_key on the emit payload must match the registered key returned by `ralph tools task show`",
        });
        let err = anyhow::Error::msg(format!(
            "task_verify_emit_bridge: task_key mismatch — registered key is '{registered_key}' but emit payload carries '{}'",
            args.task_key
        ));
        return Err(err.context(payload.to_string()));
    }

    // 3. step must match the `:step-<n>:` segment inside task_key.
    let step_segment = registered_key
        .split(':')
        .find(|seg| seg.starts_with("step-"));
    let Some(step_segment) = step_segment else {
        return Err(emit_bridge_deny(
            "step_match",
            "task_key_missing_step_segment",
            format!(
                "registered key '{registered_key}' contains no `:step-<n>:` segment; the emit-bridge requires task_key in the canonical `<plan>:<step-N>:<slug>` form per ralph-tools-tasks.md red box"
            ),
            format!(
                "task_verify_emit_bridge: registered key '{registered_key}' contains no `:step-<n>:` segment"
            ),
        ));
    };

    if step_segment != args.step {
        let payload = serde_json::json!({
            "verified": false,
            "stages": ["step_match"],
            "reason": "step_segment_mismatch",
            "expected_step": step_segment,
            "provided_step": args.step,
            "hint": "the `step` value on the emit payload must match the `:step-<n>:` segment of task_key exactly",
        });
        let err = anyhow::Error::msg(format!(
            "task_verify_emit_bridge: step mismatch — task_key contains '{step_segment}' but emit payload carries '{}'",
            args.step
        ));
        return Err(err.context(payload.to_string()));
    }

    match args.format {
        OutputFormat::Json => {
            let payload = serde_json::json!({
                "verified": true,
                "task_id": args.task_id,
                "task_key": args.task_key,
                "step": args.step,
                "registered_key": registered_key,
                "task_status": task.status,
                "loop_id": task.loop_id,
                "hint": "safe to emit; close the emit-payload round-trip with `ralph events --events-source hat-channel`",
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        OutputFormat::Quiet => println!("ok"),
        OutputFormat::Table => {
            println!(
                "verified emit-bridge (no write): task_id='{}' task_key='{}' step='{}' (loop={})",
                args.task_id,
                args.task_key,
                args.step,
                task.loop_id.as_deref().unwrap_or("<unscoped>")
            );
        }
    }
    Ok(())
}
