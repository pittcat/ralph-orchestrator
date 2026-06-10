//! CLI commands for the `ralph task` namespace.
//!
//! Provides subcommands for managing tasks:
//! - `add`: Create a new task
//! - `ensure`: Create or reuse a keyed task
//! - `list`: List all tasks
//! - `ready`: Show unblocked tasks
//! - `start`: Mark a task as in progress
//! - `close`: Mark a task as complete
//! - `reopen`: Reopen a closed/failed task
//! - `show`: Show a single task by ID

use crate::{
    display::colors, operation_guard::OperationContext, resolve_path_from_workspace,
    resolve_workspace_root,
};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use ralph_core::{Task, TaskStatus, TaskStore};
use std::path::PathBuf;

/// Output format for task commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table format
    #[default]
    Table,
    /// JSON format for programmatic access
    Json,
    /// ID-only output for scripting
    Quiet,
}

/// Task management commands for tracking work items.
#[derive(Parser, Debug)]
pub struct TaskArgs {
    #[command(subcommand)]
    pub command: TaskCommands,

    /// Working directory (default: current directory)
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum TaskCommands {
    /// Create a new task
    Add(AddArgs),

    /// Create or reuse a task by stable key
    Ensure(EnsureArgs),

    /// List all tasks
    List(ListArgs),

    /// Show unblocked tasks
    Ready(ReadyArgs),

    /// Mark a task as in progress
    Start(StartArgs),

    /// Mark a task as complete
    Close(CloseArgs),

    /// Mark a task as failed
    Fail(FailArgs),

    /// Reopen a closed or failed task
    Reopen(ReopenArgs),

    /// Show a single task by ID
    Show(ShowArgs),
}

/// Arguments for the `task add` command.
#[derive(Parser, Debug)]
pub struct AddArgs {
    /// Task title
    pub title: String,

    /// Priority (1-5, default 3)
    #[arg(short = 'p', long, default_value = "3")]
    pub priority: u8,

    /// Task description
    #[arg(short = 'd', long)]
    pub description: Option<String>,

    /// Task IDs that must complete first (comma-separated)
    #[arg(long)]
    pub blocked_by: Option<String>,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `task ensure` command.
#[derive(Parser, Debug)]
pub struct EnsureArgs {
    /// Task title
    pub title: String,

    /// Stable key used to deduplicate orchestrator-managed tasks
    #[arg(long)]
    pub key: String,

    /// Priority (1-5, default 3)
    #[arg(short = 'p', long, default_value = "3")]
    pub priority: u8,

    /// Task description
    #[arg(short = 'd', long)]
    pub description: Option<String>,

    /// Task IDs that must complete first (comma-separated)
    #[arg(long)]
    pub blocked_by: Option<String>,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `task list` command.
#[derive(Parser, Debug)]
pub struct ListArgs {
    /// Filter by status: open, in_progress, closed, failed
    #[arg(short = 's', long)]
    pub status: Option<String>,

    /// Show only tasks from the last N days
    #[arg(long, short = 'd')]
    pub days: Option<i64>,

    /// Limit the number of tasks displayed
    #[arg(long, short = 'l')]
    pub limit: Option<usize>,

    /// Show all tasks including closed and failed (hidden by default)
    #[arg(long, short = 'a')]
    pub all: bool,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `task ready` command.
#[derive(Parser, Debug)]
pub struct ReadyArgs {
    /// Show tasks from all loops, not just the current one
    #[arg(long, short = 'a')]
    pub all: bool,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Arguments for the `task start` command.
#[derive(Parser, Debug)]
pub struct StartArgs {
    /// Task ID to mark as in progress
    pub id: String,
}

/// Arguments for the `task close` command.
#[derive(Parser, Debug)]
pub struct CloseArgs {
    /// Task ID to close
    pub id: String,
}

/// Arguments for the `task fail` command.
#[derive(Parser, Debug)]
pub struct FailArgs {
    /// Task ID to mark as failed
    pub id: String,
}

/// Arguments for the `task reopen` command.
#[derive(Parser, Debug)]
pub struct ReopenArgs {
    /// Task ID to reopen
    pub id: String,
}

/// Arguments for the `task show` command.
#[derive(Parser, Debug)]
pub struct ShowArgs {
    /// Task ID
    pub id: String,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub format: OutputFormat,
}

/// Gets the tasks file path.
fn get_tasks_path(root: Option<&PathBuf>) -> PathBuf {
    resolve_path_from_workspace(".ralph/agent/tasks.jsonl", root)
}

#[cfg(test)]
fn read_current_loop_id(root: Option<&PathBuf>) -> Option<String> {
    operation_context_for(root).current_loop_id
}

fn operation_context_for(root: Option<&PathBuf>) -> OperationContext {
    OperationContext::detect(resolve_workspace_root(root))
}

/// Authorize a lifecycle mutation on `task` from the given context.
///
/// Returns `Ok(())` when the caller may mutate, `Err` with a clear
/// message otherwise. In agent context, the caller must own the task
/// or be listed as a coordinator hat. In human context, only an
/// out-of-loop warning is printed (no error).
fn authorize_lifecycle(
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

    if task.owner_hat_id.as_deref() == Some(caller_hat) {
        return Ok(());
    }
    if coordinator_hats.iter().any(|h| h == caller_hat) {
        return Ok(());
    }
    bail!(
        "{operation}: task {tid} is owned by hat '{owner}' but caller is '{caller}' (not in coordinator_hats)",
        tid = task.id,
        owner = task.owner_hat_id.as_deref().unwrap_or("?"),
        caller = caller_hat
    )
}

fn add_common_task_fields(
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
fn validate_owner_hat_id(task: &Task, coordinator_hats: &[String]) -> Result<()> {
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

fn status_matches_filter(status: TaskStatus, filter: &str) -> bool {
    let normalized = filter.to_lowercase().replace(['_', '-'], "");
    match status {
        TaskStatus::Open => normalized == "open",
        TaskStatus::InProgress => normalized == "inprogress",
        TaskStatus::Closed => normalized == "closed",
        TaskStatus::Failed => normalized == "failed",
    }
}

fn filter_tasks_for_list(store: &TaskStore, args: &ListArgs) -> Vec<Task> {
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

fn filter_tasks_for_ready(
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

/// Executes task CLI commands.
pub fn execute(args: TaskArgs, use_colors: bool) -> Result<()> {
    let root = args.root.clone();
    let coordinator_hats = load_coordinator_hats(root.as_ref());

    match args.command {
        TaskCommands::Add(add_args) => {
            execute_add(add_args, root.as_ref(), &coordinator_hats, use_colors)
        }
        TaskCommands::Ensure(ensure_args) => {
            execute_ensure(ensure_args, root.as_ref(), &coordinator_hats, use_colors)
        }
        TaskCommands::List(list_args) => execute_list(list_args, root.as_ref(), use_colors),
        TaskCommands::Ready(ready_args) => execute_ready(ready_args, root.as_ref(), use_colors),
        TaskCommands::Start(start_args) => {
            execute_start(start_args, root.as_ref(), &coordinator_hats, use_colors)
        }
        TaskCommands::Close(close_args) => {
            execute_close(close_args, root.as_ref(), &coordinator_hats, use_colors)
        }
        TaskCommands::Fail(fail_args) => {
            execute_fail(fail_args, root.as_ref(), &coordinator_hats, use_colors)
        }
        TaskCommands::Reopen(reopen_args) => {
            execute_reopen(reopen_args, root.as_ref(), &coordinator_hats, use_colors)
        }
        TaskCommands::Show(show_args) => execute_show(show_args, root.as_ref(), use_colors),
    }
}

/// Loads `tasks.coordinator_hats` from the workspace config, if any.
///
/// Returns an empty vec when the config file is missing, unreadable,
/// or does not declare coordinator hats. Errors are swallowed because
/// coordinator-hats lookup is best-effort — security defaults to
/// "no coordinator" rather than failing the whole command.
fn load_coordinator_hats(root: Option<&PathBuf>) -> Vec<String> {
    let workspace = resolve_workspace_root(root);
    for name in ["ralph.yml", "ralph.yaml"] {
        let path = workspace.join(name);
        if !path.exists() {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&raw) else {
            continue;
        };
        let Some(tasks) = value.get("tasks") else {
            continue;
        };
        let Some(arr) = tasks.get("coordinator_hats").and_then(|v| v.as_sequence()) else {
            continue;
        };
        return arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }
    Vec::new()
}

fn execute_add(
    args: AddArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let path = get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);

    add_task_with_args(&mut store, &args, &ctx, coordinator_hats, use_colors)?;
    Ok(())
}

#[cfg_attr(test, allow(dead_code))]
fn add_task_with_args(
    store: &mut TaskStore,
    args: &AddArgs,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let task = add_common_task_fields(
        Task::new(args.title.clone(), args.priority),
        ctx,
        args.description.clone(),
        args.blocked_by.clone(),
    );

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
    // existing `add_common_task_fields` only stamps an owner when
    // `ctx.current_hat_id` is set, so `None` here is a reliable signal
    // that the call is human-driven.
    validate_owner_hat_id(&task, coordinator_hats)?;

    let invalid_blockers = store.invalid_blockers(&task);
    if !invalid_blockers.is_empty() {
        bail!(
            "task blocked_by references missing or out-of-loop tasks: {}",
            invalid_blockers.join(", ")
        );
    }

    let task_id = task.id.clone();
    store.add(task.clone());
    store.save().context("Failed to save tasks")?;

    print_added_task(&task, &task_id, args.format, use_colors);
    Ok(())
}

fn execute_ensure(
    args: EnsureArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let path = get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);

    ensure_task_with_args(&mut store, &args, &ctx, coordinator_hats, use_colors)?;
    Ok(())
}

#[cfg_attr(test, allow(dead_code))]
fn ensure_task_with_args(
    store: &mut TaskStore,
    args: &EnsureArgs,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let task = add_common_task_fields(
        Task::new(args.title.clone(), args.priority).with_key(Some(args.key.clone())),
        ctx,
        args.description.clone(),
        args.blocked_by.clone(),
    );
    validate_owner_hat_id(&task, coordinator_hats)?;
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

    let ensured = store
        .with_exclusive_lock(|s| s.ensure(task).clone())
        .context("Failed to ensure task")?;

    print_ensured_task(&ensured, &key, existed, args.format, use_colors);
    Ok(())
}

fn print_added_task(task: &Task, task_id: &str, format: OutputFormat, use_colors: bool) {
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

fn print_ensured_task(
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

fn execute_list(args: ListArgs, root: Option<&PathBuf>, use_colors: bool) -> Result<()> {
    let path = get_tasks_path(root);
    let store = TaskStore::load(&path).context("Failed to load tasks")?;

    let tasks = filter_tasks_for_list(&store, &args);

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

fn execute_ready(args: ReadyArgs, root: Option<&PathBuf>, use_colors: bool) -> Result<()> {
    let path = get_tasks_path(root);
    let store = TaskStore::load(&path).context("Failed to load tasks")?;

    let ready = filter_tasks_for_ready(&store, &args, root);

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

fn execute_start(
    args: StartArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let path = get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);
    start_task_with_context(&mut store, &args.id, &ctx, coordinator_hats, use_colors)
}

#[cfg_attr(test, allow(dead_code))]
fn start_task_with_context(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let snapshot = store
        .get(task_id)
        .cloned()
        .context(format!("Task {} not found", task_id))?;
    authorize_lifecycle(&snapshot, ctx, coordinator_hats, "start")?;

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

fn execute_close(
    args: CloseArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let path = get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);
    close_task_with_context(&mut store, &args.id, &ctx, coordinator_hats, use_colors)
}

#[cfg_attr(test, allow(dead_code))]
fn close_task_with_context(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let snapshot = store
        .get(task_id)
        .cloned()
        .context(format!("Task {} not found", task_id))?;
    authorize_lifecycle(&snapshot, ctx, coordinator_hats, "close")?;

    let title = store
        .close(task_id)
        .context(format!("Task {} not found", task_id))?
        .title
        .clone();

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
    Ok(())
}

fn execute_fail(
    args: FailArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let path = get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);
    fail_task_with_context(&mut store, &args.id, &ctx, coordinator_hats, use_colors)
}

#[cfg_attr(test, allow(dead_code))]
fn fail_task_with_context(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let snapshot = store
        .get(task_id)
        .cloned()
        .context(format!("Task {} not found", task_id))?;
    authorize_lifecycle(&snapshot, ctx, coordinator_hats, "fail")?;

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

fn execute_show(args: ShowArgs, root: Option<&PathBuf>, use_colors: bool) -> Result<()> {
    let path = get_tasks_path(root);
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

fn execute_reopen(
    args: ReopenArgs,
    root: Option<&PathBuf>,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let path = get_tasks_path(root);
    let mut store = TaskStore::load(&path).context("Failed to load tasks")?;
    let ctx = operation_context_for(root);
    reopen_task_with_context(&mut store, &args.id, &ctx, coordinator_hats, use_colors)
}

#[cfg_attr(test, allow(dead_code))]
fn reopen_task_with_context(
    store: &mut TaskStore,
    task_id: &str,
    ctx: &OperationContext,
    coordinator_hats: &[String],
    use_colors: bool,
) -> Result<()> {
    let snapshot = store
        .get(task_id)
        .cloned()
        .context(format!("Task {} not found", task_id))?;
    authorize_lifecycle(&snapshot, ctx, coordinator_hats, "reopen")?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_tasks(root: &Path, tasks: Vec<Task>) -> TaskStore {
        let root_buf = root.to_path_buf();
        let path = get_tasks_path(Some(&root_buf));
        let mut store = TaskStore::load(&path).expect("load task store");
        for task in tasks {
            store.add(task);
        }
        store.save().expect("save task store");
        TaskStore::load(&path).expect("reload task store")
    }

    #[test]
    fn test_list_status_filter_accepts_in_progress() {
        let temp_dir = TempDir::new().expect("temp dir");
        let mut open_task = Task::new("Open".to_string(), 2);
        open_task.status = TaskStatus::Open;
        let mut in_progress = Task::new("In progress".to_string(), 2);
        in_progress.status = TaskStatus::InProgress;

        let store = write_tasks(temp_dir.path(), vec![open_task, in_progress]);

        let args = ListArgs {
            status: Some("in_progress".to_string()),
            days: None,
            limit: None,
            all: true,
            format: OutputFormat::Quiet,
        };

        let filtered = filter_tasks_for_list(&store, &args);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].status, TaskStatus::InProgress);
    }

    #[test]
    fn test_ready_filters_by_loop_id_marker() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path().to_path_buf();

        let mut task_loop_a = Task::new("Loop A task".to_string(), 1);
        task_loop_a.loop_id = Some("loop-a".to_string());
        let mut task_loop_b = Task::new("Loop B task".to_string(), 1);
        task_loop_b.loop_id = Some("loop-b".to_string());

        let store = write_tasks(temp_dir.path(), vec![task_loop_a, task_loop_b]);

        let marker_dir = root.join(".ralph");
        std::fs::create_dir_all(&marker_dir).expect("marker dir");
        std::fs::write(marker_dir.join("current-loop-id"), "loop-a").expect("write marker");

        let args = ReadyArgs {
            all: false,
            format: OutputFormat::Quiet,
        };

        let ready = filter_tasks_for_ready(&store, &args, Some(&root));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].loop_id.as_deref(), Some("loop-a"));
    }

    #[test]
    fn test_read_current_loop_id_ignores_empty_marker() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path().to_path_buf();
        let marker_dir = root.join(".ralph");
        std::fs::create_dir_all(&marker_dir).expect("marker dir");
        std::fs::write(marker_dir.join("current-loop-id"), "  ").expect("write marker");

        assert_eq!(read_current_loop_id(Some(&root)), None);
    }

    #[test]
    fn test_get_tasks_path_discovers_workspace_root_from_nested_dir() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ralph/agent")).expect("agent dir");

        assert_eq!(
            get_tasks_path(Some(&root)),
            root.join(".ralph/agent/tasks.jsonl")
        );
    }

    fn ctx_for(workspace: &Path, loop_id: Option<&str>, hat_id: Option<&str>) -> OperationContext {
        OperationContext::detect_with_env(workspace.to_path_buf(), |key| match key {
            "RALPH_CURRENT_HAT" => hat_id.map(|s| s.to_string()),
            "RALPH_CURRENT_LOOP_ID" => loop_id.map(|s| s.to_string()),
            "RALPH_EVENTS_FILE" => None,
            "RALPH_WAVE_WORKER" => None,
            _ => None,
        })
    }

    fn write_marker(root: &Path, file: &str, value: &str) {
        let dir = root.join(".ralph");
        std::fs::create_dir_all(&dir).expect("marker dir");
        std::fs::write(dir.join(file), value).expect("write marker");
    }

    fn add_args(title: &str, blocked_by: Option<&str>) -> AddArgs {
        AddArgs {
            title: title.to_string(),
            priority: 2,
            description: None,
            blocked_by: blocked_by.map(|s| s.to_string()),
            format: OutputFormat::Quiet,
        }
    }

    fn ensure_args(title: &str, key: &str, blocked_by: Option<&str>) -> EnsureArgs {
        EnsureArgs {
            title: title.to_string(),
            key: key.to_string(),
            priority: 2,
            description: None,
            blocked_by: blocked_by.map(|s| s.to_string()),
            format: OutputFormat::Quiet,
        }
    }

    fn open_store(root: &Path) -> TaskStore {
        let path = get_tasks_path(Some(&root.to_path_buf()));
        TaskStore::load(&path).expect("load task store")
    }

    // ---- P2 plan 列举测试 (18 项) ----

    #[test]
    fn test_task_add_stamps_loop_and_owner_hat() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let mut store = open_store(root);

        add_task_with_args(
            &mut store,
            &add_args("Do work", None),
            &ctx,
            &["executor".to_string()],
            false,
        )
        .unwrap();

        let saved = store.all();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].loop_id.as_deref(), Some("loop-a"));
        assert_eq!(saved[0].owner_hat_id.as_deref(), Some("executor"));
    }

    #[test]
    fn test_task_add_without_hat_keeps_owner_none_for_human() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-h");
        let ctx = ctx_for(root, Some("loop-h"), None);
        let mut store = open_store(root);

        add_task_with_args(
            &mut store,
            &add_args("Do work", None),
            &ctx,
            &["executor".to_string()],
            false,
        )
        .unwrap();

        let saved = store.all();
        assert_eq!(saved[0].loop_id.as_deref(), Some("loop-h"));
        assert!(saved[0].owner_hat_id.is_none());
    }

    // U3: owner_hat_id must be on tasks.coordinator_hats. The five scenarios
    // below exercise the create-side complement to the JSONL origin guard
    // (which already rejects ralph at the read path).

    #[test]
    fn test_task_add_allows_executor_when_in_coordinator_hats() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let mut store = open_store(root);

        add_task_with_args(
            &mut store,
            &add_args("ok task", None),
            &ctx,
            &["coordinator".to_string(), "executor".to_string()],
            false,
        )
        .expect("executor in coordinator_hats should be accepted");
    }

    #[test]
    fn test_task_add_rejects_ralph_when_not_in_coordinator_hats() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx = ctx_for(root, Some("loop-a"), Some("ralph"));
        let mut store = open_store(root);

        let err = add_task_with_args(
            &mut store,
            &add_args("rogue task", None),
            &ctx,
            &["coordinator".to_string(), "executor".to_string()],
            false,
        )
        .expect_err("ralph must be rejected (not in coordinator_hats)");

        let msg = err.to_string();
        assert!(
            msg.contains("'ralph'") && msg.contains("not in tasks.coordinator_hats"),
            "error should name the rejected owner and the allowlist, got: {msg}"
        );

        // And nothing was persisted.
        assert!(store.all().is_empty());
    }

    #[test]
    fn test_task_add_rejects_any_owner_when_coordinator_hats_empty() {
        // Fail-closed: an empty allowlist must not let any agent persist a
        // task. This catches the misconfigured-preset failure mode where
        // `coordinator_hats` is unset (or accidentally cleared) and the
        // orchestrator would otherwise default-open the gate.
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let mut store = open_store(root);

        let err = add_task_with_args(
            &mut store,
            &add_args("anything", None),
            &ctx,
            &[],
            false,
        )
        .expect_err("empty coordinator_hats must reject any owner (fail-closed)");

        assert!(err.to_string().contains("not in tasks.coordinator_hats"));
    }

    #[test]
    fn test_task_add_allows_human_call_without_owner() {
        // When `ctx.current_hat_id` is None, the task has no owner and
        // `validate_owner_hat_id` is a no-op. This is the human CLI
        // path — operators must not be locked out by the owner check.
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-h");
        let ctx = ctx_for(root, Some("loop-h"), None);
        let mut store = open_store(root);

        add_task_with_args(
            &mut store,
            &add_args("human task", None),
            &ctx,
            &[],
            false,
        )
        .expect("human CLI call (no owner) must not be blocked by empty allowlist");
    }

    #[test]
    fn test_task_ensure_rejects_off_allowlist_owner() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx = ctx_for(root, Some("loop-a"), Some("rogue-hat"));
        let mut store = open_store(root);

        let err = ensure_task_with_args(
            &mut store,
            &ensure_args("x", "k:v", None),
            &ctx,
            &["executor".to_string()],
            false,
        )
        .expect_err("ensure must also reject off-allowlist owner");
        assert!(err.to_string().contains("not in tasks.coordinator_hats"));
    }

    #[test]
    fn test_task_start_rejects_other_loop_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let other_loop = Task::new("Other loop".to_string(), 1)
            .with_loop_id(Some("loop-b".to_string()))
            .with_owner_hat(Some("reviewer".to_string()));
        let other_id = other_loop.id.clone();
        store.add(other_loop);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = start_task_with_context(&mut store, &other_id, &ctx, &[], false)
            .expect_err("cross-loop start should fail");
        assert!(err.to_string().contains("loop-b"));
    }

    #[test]
    fn test_task_close_rejects_other_loop_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let other_loop = Task::new("Other".to_string(), 1)
            .with_loop_id(Some("loop-b".to_string()))
            .with_owner_hat(Some("reviewer".to_string()));
        let other_id = other_loop.id.clone();
        store.add(other_loop);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = close_task_with_context(&mut store, &other_id, &ctx, &[], false)
            .expect_err("cross-loop close should fail");
        assert!(err.to_string().contains("loop-b"));
    }

    #[test]
    fn test_task_fail_rejects_other_loop_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let other = Task::new("Other".to_string(), 1)
            .with_loop_id(Some("loop-b".to_string()))
            .with_owner_hat(Some("reviewer".to_string()));
        let other_id = other.id.clone();
        store.add(other);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = fail_task_with_context(&mut store, &other_id, &ctx, &[], false)
            .expect_err("cross-loop fail should fail");
        assert!(err.to_string().contains("loop-b"));
    }

    #[test]
    fn test_task_reopen_rejects_other_loop_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let other = Task::new("Other".to_string(), 1)
            .with_loop_id(Some("loop-b".to_string()))
            .with_owner_hat(Some("reviewer".to_string()));
        let other_id = other.id.clone();
        store.add(other);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = reopen_task_with_context(&mut store, &other_id, &ctx, &[], false)
            .expect_err("cross-loop reopen should fail");
        assert!(err.to_string().contains("loop-b"));
    }

    #[test]
    fn test_task_start_rejects_other_hat_task() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let owned = Task::new("Owned".to_string(), 1)
            .with_loop_id(Some("loop-a".to_string()))
            .with_owner_hat(Some("reviewer".to_string()));
        let owned_id = owned.id.clone();
        store.add(owned);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = start_task_with_context(&mut store, &owned_id, &ctx, &[], false)
            .expect_err("cross-hat start should fail");
        assert!(err.to_string().contains("reviewer"));
    }

    #[test]
    fn test_task_close_allows_owner_hat() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let owned = Task::new("Owned".to_string(), 1)
            .with_loop_id(Some("loop-a".to_string()))
            .with_owner_hat(Some("executor".to_string()));
        let owned_id = owned.id.clone();
        store.add(owned);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        close_task_with_context(&mut store, &owned_id, &ctx, &[], false)
            .expect("owner hat should be allowed to close");
        assert_eq!(store.get(&owned_id).unwrap().status, TaskStatus::Closed);
    }

    #[test]
    fn test_task_operation_allows_configured_task_coordinator() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let owned = Task::new("Owned".to_string(), 1)
            .with_loop_id(Some("loop-a".to_string()))
            .with_owner_hat(Some("reviewer".to_string()));
        let owned_id = owned.id.clone();
        store.add(owned);

        let ctx = ctx_for(root, Some("loop-a"), Some("coordinator"));
        let coordinators = vec!["coordinator".to_string()];
        close_task_with_context(&mut store, &owned_id, &ctx, &coordinators, false)
            .expect("coordinator should be allowed to close any task");
        assert_eq!(store.get(&owned_id).unwrap().status, TaskStatus::Closed);
    }

    #[test]
    fn test_task_operation_rejects_missing_current_hat_in_agent_context() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let owned = Task::new("Owned".to_string(), 1)
            .with_loop_id(Some("loop-a".to_string()))
            .with_owner_hat(Some("executor".to_string()));
        let owned_id = owned.id.clone();
        store.add(owned);

        // Agent context (loop marker set) but no RALPH_CURRENT_HAT.
        let ctx = ctx_for(root, Some("loop-a"), None);
        let err = close_task_with_context(&mut store, &owned_id, &ctx, &[], false)
            .expect_err("missing hat in agent context should fail closed");
        assert!(err.to_string().to_lowercase().contains("hat"));
    }

    #[test]
    fn test_task_ensure_key_scoped_by_loop() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx_a = ctx_for(root, Some("loop-a"), Some("executor"));
        let mut store = open_store(root);
        ensure_task_with_args(
            &mut store,
            &ensure_args("First", "shared:task", None),
            &ctx_a,
            &["executor".to_string()],
            false,
        )
        .unwrap();
        ensure_task_with_args(
            &mut store,
            &ensure_args("Second", "shared:task", None),
            &ctx_a,
            &["executor".to_string()],
            false,
        )
        .unwrap();
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn test_task_ensure_same_key_same_loop_reuses() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx_a = ctx_for(root, Some("loop-a"), Some("executor"));
        let mut store = open_store(root);
        ensure_task_with_args(
            &mut store,
            &ensure_args("First", "shared:task", None),
            &ctx_a,
            &["executor".to_string()],
            false,
        )
        .unwrap();
        let first_id = store
            .get_by_key_in_loop("shared:task", Some("loop-a"))
            .unwrap()
            .id
            .clone();
        ensure_task_with_args(
            &mut store,
            &ensure_args("Second", "shared:task", None),
            &ctx_a,
            &["executor".to_string()],
            false,
        )
        .unwrap();
        let reused_id = store
            .get_by_key_in_loop("shared:task", Some("loop-a"))
            .unwrap()
            .id
            .clone();
        assert_eq!(first_id, reused_id);
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn test_task_ensure_same_key_different_loop_creates_new() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let ctx_a = ctx_for(root, Some("loop-a"), Some("executor"));
        let mut store = open_store(root);
        ensure_task_with_args(
            &mut store,
            &ensure_args("First", "shared:task", None),
            &ctx_a,
            &["executor".to_string()],
            false,
        )
        .unwrap();
        // Switch marker to loop-b so the next ensure is stamped loop-b.
        write_marker(root, "current-loop-id", "loop-b");
        let ctx_b = ctx_for(root, Some("loop-b"), Some("executor"));
        ensure_task_with_args(
            &mut store,
            &ensure_args("Second", "shared:task", None),
            &ctx_b,
            &["executor".to_string()],
            false,
        )
        .unwrap();
        assert_eq!(store.all().len(), 2);
    }

    #[test]
    fn test_task_add_rejects_blocker_from_other_loop() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        // Blocker exists but belongs to a different loop.
        let other_blocker =
            Task::new("Other loop blocker".to_string(), 1).with_loop_id(Some("loop-b".to_string()));
        let blocker_id = other_blocker.id.clone();
        store.add(other_blocker);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = add_task_with_args(
            &mut store,
            &add_args("Do work", Some(&blocker_id)),
            &ctx,
            &["executor".to_string()],
            false,
        )
        .expect_err("cross-loop blocker should be rejected");
        assert!(err.to_string().contains(&blocker_id));
    }

    #[test]
    fn test_task_add_rejects_missing_blocker() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = add_task_with_args(
            &mut store,
            &add_args("Do work", Some("task-9999-deadbeef")),
            &ctx,
            &["executor".to_string()],
            false,
        )
        .expect_err("missing blocker should be rejected");
        assert!(err.to_string().contains("task-9999-deadbeef"));
    }

    #[test]
    fn test_task_ready_defaults_to_current_loop() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");

        let mut t_a = Task::new("Loop A task".to_string(), 1);
        t_a.loop_id = Some("loop-a".to_string());
        let mut t_b = Task::new("Loop B task".to_string(), 1);
        t_b.loop_id = Some("loop-b".to_string());
        let store = write_tasks(root, vec![t_a, t_b]);

        let args = ReadyArgs {
            all: false,
            format: OutputFormat::Quiet,
        };
        let ready = filter_tasks_for_ready(&store, &args, Some(&root.to_path_buf()));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].loop_id.as_deref(), Some("loop-a"));
    }

    #[test]
    fn test_task_ready_all_includes_other_loops() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");

        let mut t_a = Task::new("Loop A task".to_string(), 1);
        t_a.loop_id = Some("loop-a".to_string());
        let mut t_b = Task::new("Loop B task".to_string(), 1);
        t_b.loop_id = Some("loop-b".to_string());
        let store = write_tasks(root, vec![t_a, t_b]);

        let args = ReadyArgs {
            all: true,
            format: OutputFormat::Quiet,
        };
        let ready = filter_tasks_for_ready(&store, &args, Some(&root.to_path_buf()));
        assert_eq!(ready.len(), 2);
    }

    #[test]
    fn test_legacy_task_without_loop_not_mutable_by_agent() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        write_marker(root, "current-loop-id", "loop-a");
        let mut store = open_store(root);
        // Legacy task has neither loop_id nor owner_hat_id.
        let legacy = Task::new("Legacy".to_string(), 1);
        let legacy_id = legacy.id.clone();
        store.add(legacy);

        let ctx = ctx_for(root, Some("loop-a"), Some("executor"));
        let err = close_task_with_context(&mut store, &legacy_id, &ctx, &[], false)
            .expect_err("agent must not mutate legacy task");
        assert!(err.to_string().contains("legacy"));
    }

    // ---- 协调员配置加载辅助函数测试 ----

    #[test]
    fn test_load_coordinator_hats_reads_yaml() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path().to_path_buf();
        std::fs::write(
            root.join("ralph.yml"),
            "tasks:\n  coordinator_hats:\n    - coordinator\n    - executor\n",
        )
        .expect("write ralph.yml");

        let hats = load_coordinator_hats(Some(&root));
        assert_eq!(
            hats,
            vec!["coordinator".to_string(), "executor".to_string()]
        );
    }

    #[test]
    fn test_load_coordinator_hats_missing_config_is_empty() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path().to_path_buf();
        let hats = load_coordinator_hats(Some(&root));
        assert!(hats.is_empty());
    }
}
