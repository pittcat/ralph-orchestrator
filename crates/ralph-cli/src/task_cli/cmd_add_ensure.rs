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
use super::validation;
pub(super) fn execute_add(
    args: AddArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    use_colors: bool,
    config_sources: &[ConfigSource],
) -> Result<()> {
    let path = validation::get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = validation::operation_context_for(root);
    let workspace = resolve_workspace_root(root);
    let config = validation::load_config_or_default(root, config_sources);
    // U7 (2026-07-04-003 plan): two-step gate. If the agent
    // invoked `task verify add` first, a matching ticket is on
    // disk; the gate claims it and lets the mutation proceed.
    // Without verify (or with a stale ticket), the gate denies.
    // U1 (STAB-OPAC-GATES-001): claim first, settle after the
    // store mutation — only a successful Apply consumes the
    // ticket; a failed Apply restores it for retry.
    let canonical = validation::canonical_add_payload(&args);

    validation::enforce_command_policy(
        &ctx,
        coordinator_hats,
        coordinator_err,
        Some(&config),
        "add",
        false,
    )?;
    // Unit 1 (task confirmation): a same-scope pending confirmation
    // blocks the mutation before the ticket claim, so the prepared
    // ticket survives the denial for the post-confirm retry.
    crate::task_verify_gate::pending_confirmation_precheck(&store, &config.tasks, &ctx, "add")?;
    // Unit 1 (task confirmation): a gate-active Apply mints a pending
    // confirmation for the row it is about to write. The digest is the
    // very same mutation fingerprint the verify gate claims below, so a
    // later `task confirm` replays exactly what was verified + applied.
    let confirmation = if crate::task_verify_gate::gate_is_active(&ctx, &config.tasks) {
        let (loop_id, hat_id) = validation::gate_identifiers(&ctx);
        let digest =
            crate::task_verify_gate::mutation_fingerprint("add", &canonical, loop_id, hat_id);
        Some(TaskConfirmation::new_pending(
            digest,
            loop_id.to_string(),
            hat_id.to_string(),
        ))
    } else {
        None
    };
    validation::verify_gate_claim(&workspace, &config, &ctx, "add", &canonical)?;
    let result = add_task_with_confirmation(
        &mut store,
        &args,
        &ctx,
        coordinator_hats,
        use_colors,
        confirmation,
    );
    validation::settle_gate_claim(&workspace, &ctx, "add", &canonical, result)
}

/// Test-only shim: production `execute_add` always goes through
/// [`add_task_with_confirmation`] with an explicit confirmation slot.
#[cfg(test)]
pub(crate) fn add_task_with_args(
    store: &mut TaskStore,
    args: &AddArgs,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    add_task_with_confirmation(store, args, ctx, coordinator_hats, use_colors, None)
}

/// Unit 1 (task confirmation): same as [`add_task_with_args`] but
/// attaches a gate-minted [`TaskConfirmation`] to the written row
/// when `confirmation` is `Some` (gate-active protected Apply). The
/// confirmation lands in the same `store.save()` atomic snapshot as
/// the business row — there is no second write after the mutation.
pub(crate) fn add_task_with_confirmation(
    store: &mut TaskStore,
    args: &AddArgs,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
    confirmation: Option<TaskConfirmation>,
) -> Result<()> {
    let mut task = validation::add_common_task_fields(
        Task::new(args.title.clone(), args.priority),
        ctx,
        args.description.clone(),
        args.blocked_by.clone(),
    );
    if let Some(cfm) = confirmation.as_ref() {
        task.confirmation = Some(Box::new(cfm.clone()));
    }

    // U3: owner_hat_id must come from `tasks.coordinator_hats`.
    //
    // The stall recovery path in the worktree loop (the ce-executor
    // impersonation bug fixed by the P0 origin guard) let the `ralph`
    // fallback hat silently create tasks attributed to workflow hats,
    // polluting the merge queue and corrupting plan-gate correlation.
    // Backing the create-side check with a `coordinator_hats` allowlist
    // closes the gap: any hat that is not on the allowlist cannot
    // create a task. When `owner_hat_id` is absent (human CLI usage,
    // where `ctx.current_hat_id` is None), the check is skipped — the
    // existing `validation::add_common_task_fields` only stamps an owner when
    // `ctx.current_hat_id` is set, so `None` here is a reliable signal
    // that the call is human-driven.
    validation::validate_owner_hat_id(&task, coordinator_hats)?;

    let invalid_blockers = store.invalid_blockers(&task);
    if !invalid_blockers.is_empty() {
        bail!(
            "task blocked_by references missing or out-of-loop tasks: {}",
            invalid_blockers.join(", ")
        );
    }

    if let Some(key) = task.key.as_deref()
        && let Some(locus) = ralph_core::task_store::live_task_locus(key)
        && let Some(existing) = store.find_by_locus_in_loop(&locus, task.loop_id.as_deref())
    {
        bail!(
            "task add rejected: live identity already exists for loop {:?} step locus \
                     '{locus}' (task_id={}). Use `ralph tools task ensure` instead of add.",
            task.loop_id,
            existing.id
        );
    }

    let task_id = task.id.clone();
    let added_id = store
        .add_checked(task.clone())
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .id
        .clone();
    // Idempotent re-add under an existing (id, key) row returns the
    // persisted row instead of pushing; refresh its confirmation so
    // disk matches the printed task in both branches.
    if confirmation.is_some()
        && let Some(row) = store.get_mut(&added_id)
    {
        // Unit 1 follow-up (cross-scope overwrite hole): never mint
        // over a pending confirmation recorded by a different loop/hat
        // — that would silently release the recorder's confirm
        // obligation. The bail propagates through validation::settle_gate_claim so
        // the prepared ticket is restored and nothing is persisted.
        if let Some(existing_cfm) = row.confirmation.as_ref()
            && existing_cfm.state == ConfirmationState::Pending
            && (existing_cfm.loop_id.as_str(), existing_cfm.hat_id.as_str())
                != validation::gate_identifiers(ctx)
        {
            return Err(validation::confirmation_scope_conflict(
                "add",
                &row.id,
                &existing_cfm.loop_id,
                &existing_cfm.hat_id,
            ));
        }
        row.confirmation = confirmation.map(Box::new);
    }
    store.save().context("Failed to save tasks")?;

    print_added_task(&task, &task_id, args.format, use_colors);
    Ok(())
}

pub(super) fn execute_ensure(
    args: EnsureArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    coordinator_err: Option<&CoordinatorHatsError>,
    use_colors: bool,
    config_sources: &[ConfigSource],
) -> Result<()> {
    let path = validation::get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = validation::operation_context_for(root);
    let workspace = resolve_workspace_root(root);
    let config = validation::load_config_or_default(root, config_sources);
    let is_for_fix_unit = args.for_fix_unit.is_some();

    validation::enforce_command_policy(
        &ctx,
        coordinator_hats,
        coordinator_err,
        Some(&config),
        "ensure",
        is_for_fix_unit,
    )?;

    // R4 (2026-06-14-003 plan): opt into the single-U contract.
    // Two signals are accepted (env var takes precedence; the
    // marker file is the safe fallback for `ralph run` because the
    // workspace `forbid(unsafe_code)` lint forbids `set_var` from
    // lib code):
    //   1. `RALPH_ENFORCE_CURRENT_UNIT` env var (set by operators
    //      for standalone CLI use).
    //   2. `<workspace>/.ralph/agent/.ralph-enforce-current-unit`
    //      marker file (written by `ralph run`'s bootstrap when the
    //      preset opts in).
    if std::env::var_os("RALPH_ENFORCE_CURRENT_UNIT").is_some() {
        store.set_enforce_current_unit(true);
    } else if let Some(workspace) = root {
        let marker = workspace
            .join(".ralph")
            .join("agent")
            .join(".ralph-enforce-current-unit");
        if marker.exists() {
            store.set_enforce_current_unit(true);
        }
    }

    // U7 (2026-07-04-003 plan): two-step gate for ensure. Use
    // the same canonical payload as verify so the fingerprint
    // matches a preceding `task verify ensure` call.
    let derived_key = if let Some(spec) = args.for_fix_unit.as_deref() {
        let mut parts = spec.split(':');
        let plan = parts.next().unwrap_or("").to_string();
        let fix_step = parts.next().unwrap_or("").to_string();
        let slug = parts.next().unwrap_or("").to_string();
        if plan.is_empty() || fix_step.is_empty() || slug.is_empty() || parts.next().is_some() {
            bail!(
                "--for-fix-unit expects exactly 3 colon-separated segments: \
                 PLAN:FIX_STEP:SLUG, got '{spec}'"
            );
        }
        Some(format!("ce-executor:{plan}:{fix_step}:{slug}"))
    } else {
        None
    };
    let canonical = validation::canonical_ensure_payload(&args, derived_key.as_deref());
    let (loop_id, hat_id) = validation::gate_identifiers(&ctx);
    let fingerprint =
        crate::task_verify_gate::mutation_fingerprint("ensure", &canonical, loop_id, hat_id);
    let config = validation::load_config_or_default(root, config_sources);
    let path = crate::task_verify_gate::scoped_ticket_path(
        &workspace, "ensure", &canonical, loop_id, hat_id,
    );
    // Unit 1 (task confirmation): a same-scope pending confirmation
    // blocks the mutation before the ticket claim, so the prepared
    // ticket survives the denial for the post-confirm retry.
    crate::task_verify_gate::pending_confirmation_precheck(&store, &config.tasks, &ctx, "ensure")?;
    // U1 (STAB-OPAC-GATES-001): claim first, settle after the
    // store mutation — only a successful Apply consumes the
    // ticket; a failed Apply restores it for retry.
    crate::task_verify_gate::try_claim_matching_ticket(
        &path,
        &config.tasks,
        &ctx,
        "ensure",
        &fingerprint,
    )?;

    // Unit 1 (task confirmation): a gate-active Apply mints a pending
    // confirmation for the row it ensures. The digest is the same
    // mutation fingerprint claimed above.
    let confirmation = if crate::task_verify_gate::gate_is_active(&ctx, &config.tasks) {
        Some(TaskConfirmation::new_pending(
            fingerprint.clone(),
            loop_id.to_string(),
            hat_id.to_string(),
        ))
    } else {
        None
    };
    let result = ensure_task_with_confirmation(
        &mut store,
        &args,
        &ctx,
        coordinator_hats,
        use_colors,
        config_sources,
        confirmation,
    );
    validation::settle_gate_claim(&workspace, &ctx, "ensure", &canonical, result)
}

/// Test-only shim: production `execute_ensure` always goes through
/// [`ensure_task_with_confirmation`] with an explicit confirmation slot.
#[cfg(test)]
pub(crate) fn ensure_task_with_args(
    store: &mut TaskStore,
    args: &EnsureArgs,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
    config_sources: &[ConfigSource],
) -> Result<()> {
    ensure_task_with_confirmation(
        store,
        args,
        ctx,
        coordinator_hats,
        use_colors,
        config_sources,
        None,
    )
}

/// Unit 1 (task confirmation): same as [`ensure_task_with_args`] but
/// overwrites the confirmation on the ensured row (newly created or
/// refreshed) when `confirmation` is `Some` (gate-active protected
/// Apply). The write happens inside the same `with_exclusive_lock`
/// snapshot that persists the mutation — no second save.
pub(crate) fn ensure_task_with_confirmation(
    store: &mut TaskStore,
    args: &EnsureArgs,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
    _config_sources: &[ConfigSource],
    confirmation: Option<TaskConfirmation>,
) -> Result<()> {
    // 2026-06-28-002 U8: `--for-fix-unit plan:fix_step:slug` builds
    // the canonical fix-unit task and pins the owner to
    // `coordinator`. The returned task_id is then used in the
    // follow-up `work.ready` emit so `work.done` no longer
    // collides with the legacy `task-fix-01-placeholder`
    // contract. When the flag is set we ignore the `--key`
    // argument to avoid silent double-sourcing of the key.
    let derived_key = if let Some(spec) = args.for_fix_unit.as_deref() {
        let mut parts = spec.split(':');
        let plan = parts.next().unwrap_or("").to_string();
        let fix_step = parts.next().unwrap_or("").to_string();
        let slug = parts.next().unwrap_or("").to_string();
        if plan.is_empty() || fix_step.is_empty() || slug.is_empty() || parts.next().is_some() {
            bail!(
                "--for-fix-unit expects exactly 3 colon-separated segments: \
                 PLAN:FIX_STEP:SLUG, got '{spec}'"
            );
        }
        Some(format!("ce-executor:{plan}:{fix_step}:{slug}"))
    } else {
        None
    };
    let key_value = derived_key
        .clone()
        .or_else(|| args.key.clone())
        .expect("ensure requires either --key or --for-fix-unit");
    let mut task = validation::add_common_task_fields(
        Task::new(args.title.clone(), args.priority).with_key(Some(key_value)),
        ctx,
        args.description.clone(),
        args.blocked_by.clone(),
    );
    // 2026-06-28-002 U8: pin the owner to `coordinator` so the
    // legacy execution contract (`TaskWrongLoop` / loop scope)
    // validates the follow-up `work.ready` / `work.done`
    // payload against the canonical fix-unit hat.
    if args.for_fix_unit.is_some() {
        task = task.with_owner_hat(Some("coordinator".to_string()));
    }
    validation::validate_owner_hat_id(&task, coordinator_hats)?;
    let key = task.key.clone().expect("ensure key should be set");
    let loop_id = task.loop_id.clone();
    let existed = store.get_by_key_in_loop(&key, loop_id.as_deref()).is_some();

    let invalid_blockers = store.invalid_blockers(&task);
    if !invalid_blockers.is_empty() {
        bail!(
            "task blocked_by references missing or out-of-loop tasks: {}",
            invalid_blockers.join(", ")
        );
    }

    let (caller_loop, caller_hat) = validation::gate_identifiers(ctx);
    let ensured = store
        .with_exclusive_lock(|s| -> anyhow::Result<Task> {
            // Unit 1 follow-up (cross-scope overwrite hole): check the
            // target row's existing confirmation BEFORE minting. When
            // the row that `ensure` will dedup into still carries a
            // pending confirmation recorded by a different loop/hat,
            // minting over it would silently release the recorder's
            // confirm obligation (fail-open). The check runs inside the
            // exclusive lock so it cannot race a concurrent ensure; the
            // aborted RMW persists no mutation and validation::settle_gate_claim
            // restores the prepared ticket for the post-confirm retry.
            if confirmation.is_some()
                && let Some(existing) = s.get_by_key_in_loop(&key, loop_id.as_deref())
                && let Some(existing_cfm) = existing.confirmation.as_ref()
                && existing_cfm.state == ConfirmationState::Pending
                && (existing_cfm.loop_id.as_str(), existing_cfm.hat_id.as_str())
                    != (caller_loop, caller_hat)
            {
                return Err(validation::confirmation_scope_conflict(
                    "ensure",
                    &existing.id,
                    &existing_cfm.loop_id,
                    &existing_cfm.hat_id,
                ));
            }
            let mut ensured = s.ensure(task).clone();
            // Unit 1 (task confirmation): overwrite the confirmation on
            // the ensured row within the same exclusive-lock snapshot
            // that persists the mutation. Skip the R4 collision shape
            // (ensured key differs from the requested key) — that row
            // belongs to a different unit and gets no confirmation.
            if let Some(cfm) = confirmation.as_ref()
                && ensured.key.as_deref() == Some(key.as_str())
                && let Some(row) = s.get_mut(&ensured.id)
            {
                row.confirmation = Some(Box::new(cfm.clone()));
                ensured.confirmation = Some(Box::new(cfm.clone()));
            }
            Ok(ensured)
        })
        .context("Failed to ensure task")??;

    // R4 (2026-06-14-003 plan): when the single-U contract is active
    // and the requested key differs from the ensured task's key, the
    // contract rejected the new unit in favour of an open sibling
    // task.  Surface the collision via a non-zero exit + stderr so
    // the agent's `ralph tools task ensure` invocation is not a
    // silent surprise.  Without this check the CLI prints
    // 'Ensured task <existing> <uM-...>' for the new uN- key and
    // exits 0, which masks the rejection.
    if store.enforce_current_unit() && ensured.key.as_deref() != Some(&key) {
        bail!(
            "rejected by R4 single-U contract: ensure key '{key}' conflicts with \
             existing task id={} key={} (only one open task per (loop_id, plan, step) \
             is allowed). Close the existing task first or use a non-uN- key suffix.",
            ensured.id,
            ensured.key.as_deref().unwrap_or("?"),
        );
    }

    print_ensured_task(&ensured, &key, existed, args.format, use_colors);
    Ok(())
}

pub(crate) fn print_added_task(task: &Task, task_id: &str, format: OutputFormat, use_colors: bool) {
    match format {
        OutputFormat::Table => {
            if use_colors {
                println!("{}Created task {}{}", colors::GREEN, task_id, colors::RESET);
            } else {
                println!("Created task {}", task_id);
            }
            println!("  Title: {}", task.title);
            println!("  Priority: {}", task.priority);
            if let Some(key) = &task.key {
                println!("  Key: {}", key);
            }
            if !task.blocked_by.is_empty() {
                println!("  Blocked by: {}", task.blocked_by.join(", "));
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string(task).expect("task serializes"));
        }
        OutputFormat::Quiet => {
            println!("{}", task_id);
        }
    }
}

pub(crate) fn print_ensured_task(
    ensured: &Task,
    key: &str,
    existed: bool,
    format: OutputFormat,
    use_colors: bool,
) {
    match format {
        OutputFormat::Table => {
            let verb = if existed { "Reused" } else { "Ensured" };
            if use_colors {
                println!(
                    "{}{} task {}{}",
                    colors::GREEN,
                    verb,
                    ensured.id,
                    colors::RESET
                );
            } else {
                println!("{} task {}", verb, ensured.id);
            }
            println!("  Title: {}", ensured.title);
            println!("  Key: {}", key);
            println!("  Priority: {}", ensured.priority);
            if !ensured.blocked_by.is_empty() {
                println!("  Blocked by: {}", ensured.blocked_by.join(", "));
            }
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string(ensured).expect("task serializes")
            );
        }
        OutputFormat::Quiet => {
            println!("{}", ensured.id);
        }
    }
}
