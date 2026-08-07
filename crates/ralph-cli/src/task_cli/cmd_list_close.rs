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

pub(super) fn execute_list(args: ListArgs, root: Option<&PathBuf>, use_colors: bool) -> Result<()> {
    let path = validation::get_tasks_path(root);
    let store = TaskStore::load(&path).context("Failed to load tasks")?;

    let tasks = validation::filter_tasks_for_list(&store, &args);

    match args.format {
        OutputFormat::Table => {
            if tasks.is_empty() {
                println!("No tasks found");
            } else {
                if use_colors {
                    println!(
                        "{}{:<20} {:<15} {:<8} {:<60} {:<24}{}",
                        colors::DIM,
                        "ID",
                        "Status",
                        "Priority",
                        "Title",
                        "Key",
                        colors::RESET
                    );
                    println!("{}{}{}", colors::DIM, "-".repeat(131), colors::RESET);
                } else {
                    println!(
                        "{:<20} {:<15} {:<8} {:<60} {:<24}",
                        "ID", "Status", "Priority", "Title", "Key"
                    );
                    println!("{}", "-".repeat(131));
                }

                for task in &tasks {
                    let (status_str, status_color) = match task.status {
                        TaskStatus::Open => ("open", colors::GREEN),
                        TaskStatus::InProgress => ("in_progress", colors::BLUE),
                        TaskStatus::Closed => ("closed", colors::DIM),
                        TaskStatus::Failed => ("failed", colors::RED),
                    };

                    let priority_color = match task.priority {
                        1 => colors::RED,
                        2 => colors::YELLOW,
                        _ => colors::RESET,
                    };

                    let title_truncated = if task.title.len() > 60 {
                        crate::display::truncate(&task.title, 60)
                    } else {
                        task.title.clone()
                    };

                    if use_colors {
                        println!(
                            "{}{:<20}{} {}{:<15}{} {}{:<8}{} {:<60} {:<24}",
                            colors::DIM,
                            task.id,
                            colors::RESET,
                            status_color,
                            status_str,
                            colors::RESET,
                            priority_color,
                            task.priority,
                            colors::RESET,
                            title_truncated,
                            task.key.as_deref().unwrap_or("-")
                        );
                    } else {
                        println!(
                            "{:<20} {:<15} {:<8} {:<60} {:<24}",
                            task.id,
                            status_str,
                            task.priority,
                            title_truncated,
                            task.key.as_deref().unwrap_or("-")
                        );
                    }
                }
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&tasks)?);
        }
        OutputFormat::Quiet => {
            for task in &tasks {
                println!("{}", task.id);
            }
        }
    }

    Ok(())
}

pub(super) fn execute_ready(
    args: ReadyArgs,
    root: Option<&PathBuf>,
    use_colors: bool,
) -> Result<()> {
    let path = validation::get_tasks_path(root);
    let store = TaskStore::load(&path).context("Failed to load tasks")?;

    let ready = validation::filter_tasks_for_ready(&store, &args, root);

    match args.format {
        OutputFormat::Table => {
            if ready.is_empty() {
                println!("No ready tasks");
            } else {
                if use_colors {
                    println!(
                        "{}{:<20} {:<8} {:<60} {:<24}{}",
                        colors::DIM,
                        "ID",
                        "Priority",
                        "Title",
                        "Key",
                        colors::RESET
                    );
                    println!("{}{}{}", colors::DIM, "-".repeat(115), colors::RESET);
                } else {
                    println!(
                        "{:<20} {:<8} {:<60} {:<24}",
                        "ID", "Priority", "Title", "Key"
                    );
                    println!("{}", "-".repeat(115));
                }

                for task in &ready {
                    let title_truncated = if task.title.len() > 60 {
                        crate::display::truncate(&task.title, 60)
                    } else {
                        task.title.clone()
                    };

                    let priority_color = match task.priority {
                        1 => colors::RED,
                        2 => colors::YELLOW,
                        _ => colors::RESET,
                    };

                    if use_colors {
                        println!(
                            "{}{:<20}{} {}{:<8}{} {:<60} {:<24}",
                            colors::DIM,
                            task.id,
                            colors::RESET,
                            priority_color,
                            task.priority,
                            colors::RESET,
                            title_truncated,
                            task.key.as_deref().unwrap_or("-")
                        );
                    } else {
                        println!(
                            "{:<20} {:<8} {:<60} {:<24}",
                            task.id,
                            task.priority,
                            title_truncated,
                            task.key.as_deref().unwrap_or("-")
                        );
                    }
                }
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&ready)?);
        }
        OutputFormat::Quiet => {
            for task in &ready {
                println!("{}", task.id);
            }
        }
    }

    Ok(())
}

pub(super) fn execute_start(
    args: StartArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    use_colors: bool,
    _config_sources: &[ConfigSource],
) -> Result<()> {
    let path = validation::get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = validation::operation_context_for(root);
    start_task_with_context(&mut store, &args.id, &ctx, coordinator_hats, use_colors)
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn start_task_with_context(
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
    validation::authorize_lifecycle(&snapshot, ctx, coordinator_hats, "start")?;

    let started = store
        .with_exclusive_lock(|s| s.start(task_id).cloned())
        .context("Failed to save tasks")?
        .context(format!("Task {} not found", task_id))?;

    if use_colors {
        println!(
            "{}Started task: {} - {}{}",
            colors::BLUE,
            task_id,
            started.title,
            colors::RESET
        );
    } else {
        println!("Started task: {} - {}", task_id, started.title);
    }
    Ok(())
}

pub(super) fn execute_close(
    args: CloseArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    config: &ralph_core::config::RalphConfig,
    use_colors: bool,
    _config_sources: &[ConfigSource],
) -> Result<()> {
    let path = validation::get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = validation::operation_context_for(root);
    // U3 close-time role gate (mirrors add/ensure). close itself
    // is not in `COORDINATOR_ONLY` so the gate is permissive —
    // `validation::authorize_lifecycle` below still enforces ownership. The
    // call here keeps the entry-point message shape uniform with
    // add/ensure for future policy tightening.
    let coordinator_err: Option<CoordinatorHatsError> = if config.tasks.coordinator_hats.is_empty()
    {
        Some(CoordinatorHatsError::CoordinatorHatsEmpty)
    } else {
        None
    };
    validation::enforce_command_policy(
        &ctx,
        coordinator_hats,
        coordinator_err.as_ref(),
        None,
        "close",
        false,
    )?;
    close_task_with_context_and_config(
        &mut store,
        &args.id,
        &ctx,
        coordinator_hats,
        use_colors,
        Some(config),
        root,
    )
}

// 2026-07-16 cleanup U4 (KTD-3): reserved wrapper for the
// new ACL surface (`close_task_with_context_and_config`). The
// runtime currently calls `*_and_config` directly; this wrapper
// stays as a stable entry point so downstream callers (e.g.
// the tui's `verify` flow) can opt in without a churn round-trip.
#[allow(dead_code)]
pub(crate) fn close_task_with_context(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    close_task_with_context_and_config(
        store,
        task_id,
        ctx,
        coordinator_hats,
        use_colors,
        None,
        None,
    )
}

/// U7-aware variant of close. When `config` + `root` are provided and
/// the caller is in agent context, the function reads the hat-channel
/// (`current-hat-events`) tail after saving the close and emits a
/// warning stderr JSON when no completion-class topic is present there.
#[allow(clippy::too_many_arguments)]
pub(crate) fn close_task_with_context_and_config(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
    config: Option<&ralph_core::config::RalphConfig>,
    root: Option<&PathBuf>,
) -> Result<()> {
    validation::validate_task_id(task_id)?;
    let snapshot = store
        .get(task_id)
        .cloned()
        .context(format!("Task {} not found", task_id))?;
    let _owner_hat = snapshot.owner_hat_id.clone();
    validation::authorize_lifecycle(&snapshot, ctx, coordinator_hats, "close")?;

    let title = store
        .get_mut(task_id)
        .map(|t| {
            // 2026-06-30-001 P0-4: the `ralph task close`
            // CLI path is the only legitimate way a task
            // gets closed without an explicit
            // `TaskStore::start` call (operator explicitly
            // retires a row that never picked up). Mark
            // the row started here, mirroring the
            // `project_close_task` event path. The new
            // `TaskStore::close` / `close_by_key`
            // `started.is_none()` guard (added in P0-4
            // to prevent orphan closed tasks for
            // placeholder rows) accepts the close.
            t.start();
            t.status = TaskStatus::Closed;
            t.closed = Some(chrono::Utc::now().to_rfc3339());
            t.title.clone()
        })
        .context(format!("Task {} not found", task_id))?;

    store.save().context("Failed to save tasks")?;

    if use_colors {
        println!(
            "{}Closed task: {} - {}{}",
            colors::GREEN,
            task_id,
            title,
            colors::RESET
        );
    } else {
        println!("Closed task: {} - {}", task_id, title);
    }

    // U7: completion-emit guard. Only fires in agent context AND only
    // when the CLI has a config + workspace root to derive completion
    // topics from. The legacy CLI callers (which pass `None`) keep the
    // pre-U7 behaviour: silent close, no warning. The caller hat is
    // taken from `ctx.current_hat_id` (not the task owner) so a
    // coordinator hat that closes someone else's task still warns based
    // on its own completion contract.
    if let (Some(cfg), Some(root_path)) = (config, root)
        && let Some(caller_hat) = ctx.current_hat_id.clone()
    {
        emit_close_completion_warning(root_path, cfg, &caller_hat, task_id);
    }
    Ok(())
}

/// U7 helper: if the agent caller has completion-class topics they
/// should publish after closing, scan the hat-channel tail for any of
/// those topics and emit a stderr JSON warning when none are present.
///
/// Design notes:
///
/// - **warn-only, not deny**: per the plan (`Non-goals: 不 hard-block
///   executor \`task close\``). The function never returns an error
///   and never alters the exit code; agents that intentionally skip
///   the completion emit (e.g. cancel paths) keep their close.
/// - **hat-channel only**: the merge happens *after* the backend
///   exits, so the same-activation Confirm can only see the
///   `current-hat-events` marker. Reading main events here would
///   duplicate work `ralph events --events-source auto` already does.
/// - **fail-closed on empty / unreadable channel**: agents still get a
///   `hint: run ralph inspect loop` so they can self-diagnose.
pub(crate) fn emit_close_completion_warning(
    root: &Path,
    config: &ralph_core::config::RalphConfig,
    caller_hat: &str,
    task_id: &str,
) {
    let expected = if config.event_loop.event_policy.is_some() {
        ralph_core::completion_emit::derive_completion_publishes(config, caller_hat)
    } else {
        Vec::new()
    };
    if expected.is_empty() {
        return; // nothing for this hat to emit; no warning.
    }
    let channel_hint = "hat-channel file is empty or missing; \
                        run `ralph inspect loop` to confirm the marker is set";
    let Some((channel_path, exists)) = crate::cli::resolve_hat_channel_file(root) else {
        eprintln!(
            "{} {{ \"code\": \"close_without_completion_emit\", \
             \"hat\": \"{caller_hat}\", \"task_id\": \"{task_id}\", \
             \"expected_topics\": {expected:?}, \
             \"reason\": \"hat_channel_missing_marker\", \
             \"hint\": \"{channel_hint}\" }}",
            ralph_core::completion_emit::CLOSE_WITHOUT_COMPLETION_PREFIX
        );
        return;
    };
    if !exists {
        eprintln!(
            "{} {{ \"code\": \"close_without_completion_emit\", \
             \"hat\": \"{caller_hat}\", \"task_id\": \"{task_id}\", \
             \"expected_topics\": {expected:?}, \
             \"reason\": \"hat_channel_unreadable\", \
             \"hint\": \"{channel_hint}\" }}",
            ralph_core::completion_emit::CLOSE_WITHOUT_COMPLETION_PREFIX
        );
        return;
    }
    let Ok(content) = std::fs::read_to_string(&channel_path) else {
        eprintln!(
            "{} {{ \"code\": \"close_without_completion_emit\", \
             \"hat\": \"{caller_hat}\", \"task_id\": \"{task_id}\", \
             \"expected_topics\": {expected:?}, \
             \"reason\": \"hat_channel_unreadable\", \
             \"hint\": \"{channel_hint}\" }}",
            ralph_core::completion_emit::CLOSE_WITHOUT_COMPLETION_PREFIX
        );
        return;
    };
    let tail_topics = parse_topics_from_jsonl_tail(&content, TAIL_SCAN_LINES);
    if tail_topics.iter().any(|t| expected.iter().any(|e| e == t)) {
        return; // close + completion topics both recorded.
    }

    let next = ralph_core::completion_emit::next_step_hint(&expected);
    eprintln!(
        "{} {{ \"code\": \"close_without_completion_emit\", \
             \"hat\": \"{}\", \"task_id\": \"{}\", \
             \"expected_topics\": {expected:?}, \
             \"observed_topics\": {tail_topics:?}, \
             \"next_step\": \"{}\" }}",
        ralph_core::completion_emit::CLOSE_WITHOUT_COMPLETION_PREFIX,
        caller_hat,
        task_id,
        next,
    );
}

/// Build the success-path stderr payload (extracted for testability).
/// Public to `#[cfg(test)]` modules; non-test callers should use
/// `emit_close_completion_warning` instead.
#[doc(hidden)]
#[cfg(test)]
pub fn build_close_warning_payload(
    caller_hat: &str,
    task_id: &str,
    expected: &[String],
    tail_topics: &[String],
    next: &str,
) -> String {
    format!(
        "{} {{ \"code\": \"close_without_completion_emit\", \
         \"hat\": \"{}\", \"task_id\": \"{}\", \
         \"expected_topics\": {expected:?}, \
         \"observed_topics\": {tail_topics:?}, \
         \"next_step\": \"{}\" }}",
        ralph_core::completion_emit::CLOSE_WITHOUT_COMPLETION_PREFIX,
        caller_hat,
        task_id,
        next,
    )
}

/// Build the missing-marker early-return stderr payload (test helper).
#[doc(hidden)]
#[cfg(test)]
pub fn build_close_warning_payload_missing_marker(
    caller_hat: &str,
    task_id: &str,
    expected: &[String],
) -> String {
    let channel_hint = "hat-channel file is empty or missing; \
                        run `ralph inspect loop` to confirm the marker is set";
    format!(
        "{} {{ \"code\": \"close_without_completion_emit\", \
         \"hat\": \"{caller_hat}\", \"task_id\": \"{task_id}\", \
         \"expected_topics\": {expected:?}, \
         \"reason\": \"hat_channel_missing_marker\", \
         \"hint\": \"{channel_hint}\" }}",
        ralph_core::completion_emit::CLOSE_WITHOUT_COMPLETION_PREFIX
    )
}

/// How many trailing JSONL lines `emit_close_completion_warning` scans
/// when looking for the expected completion topic. The agent's
/// same-activation write channel rarely grows large — a worker hat may
/// emit a handful of events before closing — so a small fixed window
/// is enough for Confirm. Reading the whole file was P1 #7's source of
/// false negatives on multi-hour activations.
const TAIL_SCAN_LINES: usize = 50;

/// Light-weight JSONL topic extractor — pulls the `topic` field from
/// each of the trailing N lines that look like a valid event envelope.
/// Tolerant of malformed lines (skipped silently) because the
/// hat-channel may carry lines from multiple sources when working-tree
/// features are toggled.
pub(crate) fn parse_topics_from_jsonl_tail(content: &str, max_lines: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in content
        .lines()
        .rev()
        .take(max_lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
            && let Some(topic) = v.get("topic").and_then(|t| t.as_str())
        {
            out.push(topic.to_string());
        }
    }
    out
}
